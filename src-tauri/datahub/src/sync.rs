//! Cron scheduling and the sync runner.
//!
//! The one ordering rule that matters: **write the data first, advance the
//! cursor second.** If the write fails the cursor stays put, so the next run
//! simply repeats the same window. Combined with an idempotent upsert that
//! costs nothing but a little work; the reverse order would silently skip data.

use crate::error::{AppError, AppResult};
use crate::market::{BarProvenance, MarketStore};
use crate::metadata::{JobPatch, MetadataStore, SyncJob};
use crate::sources::{DataSource, FetchRequest};

// ─────────────────────────────── Cron ───────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct CronExpr {
    pub minutes: Vec<u32>,
    pub hours: Vec<u32>,
    pub doms: Vec<u32>,
    pub months: Vec<u32>,
    /// 0 = Sunday … 6 = Saturday (standard cron).
    pub dows: Vec<u32>,
}

/// Parse one cron field. Supports `*`, `*/n`, `a-b`, `a-b/n`, and comma lists.
fn parse_field(raw: &str, min: u32, max: u32) -> AppResult<Vec<u32>> {
    let mut values = std::collections::BTreeSet::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(AppError::BadRequest("cron/empty-field".into()));
        }
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, Some(s.parse::<u32>().map_err(|_| AppError::BadRequest(format!("cron/bad-step: {s}")))?)),
            None => (part, None),
        };
        if step == Some(0) {
            return Err(AppError::BadRequest("cron/zero-step".into()));
        }
        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (
                a.parse::<u32>()
                    .map_err(|_| AppError::BadRequest(format!("cron/bad-range: {range}")))?,
                b.parse::<u32>()
                    .map_err(|_| AppError::BadRequest(format!("cron/bad-range: {range}")))?,
            )
        } else {
            let v = range
                .parse::<u32>()
                .map_err(|_| AppError::BadRequest(format!("cron/bad-value: {range}")))?;
            (v, v)
        };
        if lo < min || hi > max || lo > hi {
            return Err(AppError::BadRequest(format!("cron/out-of-range: {range}")));
        }
        let step = step.unwrap_or(1);
        let mut v = lo;
        while v <= hi {
            values.insert(v);
            v += step;
        }
    }
    if values.is_empty() {
        return Err(AppError::BadRequest("cron/no-values".into()));
    }
    Ok(values.into_iter().collect())
}

impl std::str::FromStr for CronExpr {
    type Err = AppError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(AppError::BadRequest(format!(
                "cron/expected-5-fields: got {}",
                parts.len()
            )));
        }
        Ok(CronExpr {
            minutes: parse_field(parts[0], 0, 59)?,
            hours: parse_field(parts[1], 0, 23)?,
            doms: parse_field(parts[2], 1, 31)?,
            months: parse_field(parts[3], 1, 12)?,
            dows: parse_field(parts[4], 0, 6)?,
        })
    }
}

/// Days since the Unix epoch for a civil date (Hinnant's algorithm). Pure
/// arithmetic so the search loop stays cheap — constructing calendar types
/// millions of times is what makes naive cron walkers slow.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - (if m <= 2 { 1 } else { 0 });
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m as i64 - 3 } else { m as i64 + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 0 = Sunday … 6 = Saturday, matching cron.
fn weekday_from_days(days: i64) -> u32 {
    (((days + 4) % 7 + 7) % 7) as u32
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

impl CronExpr {
    /// First match strictly after `now_ms`. Searches at most ~5 years; returns
    /// `None` when nothing matches (e.g. `0 0 30 2 *` in a non-leap stretch).
    pub fn next_after(&self, now_ms: i64) -> Option<i64> {
        let start_day = now_ms.div_euclid(86_400_000);
        let start_min_of_day = (now_ms.rem_euclid(86_400_000)) / 60_000;

        // Seed the year from `now` so we never scan from year 0. Note the
        // `719_468` offset is already baked into the epoch conversion, so
        // `yoe + era * 400` IS the absolute year — adding 1970 on top of it
        // (an earlier bug) put us in the year 3993.
        let mut y = {
            let z = start_day + 719_468;
            let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
            let doe = z - era * 146_097;
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            yoe + era * 400
        };
        // Walk back a year to be safe about the approximation above.
        y -= 1;

        for _ in 0..6 {
            for &m in &self.months {
                let dim = days_in_month(y, m);
                if dim == 0 {
                    continue;
                }
                for d in 1..=dim {
                    if !self.doms.contains(&d) {
                        continue;
                    }
                    let day_no = days_from_civil(y, m, d);
                    if !self.dows.contains(&weekday_from_days(day_no)) {
                        continue;
                    }
                    if day_no < start_day {
                        continue;
                    }
                    for &h in &self.hours {
                        for &mi in &self.minutes {
                            let min_of_day = h as i64 * 60 + mi as i64;
                            if day_no == start_day && min_of_day <= start_min_of_day {
                                continue;
                            }
                            return Some(day_no * 86_400_000 + min_of_day * 60_000);
                        }
                    }
                }
            }
            y += 1;
        }
        None
    }
}

/// Jobs whose schedule has come due.
pub fn due_jobs<'a>(jobs: &'a [SyncJob], now: i64) -> Vec<&'a SyncJob> {
    jobs.iter()
        .filter(|j| {
            let Some(cron) = &j.cron else { return false };
            let Ok(expr) = cron.parse::<CronExpr>() else {
                return false;
            };
            let since = j.last_run_at.unwrap_or(j.created_at);
            match expr.next_after(since) {
                Some(next) => next <= now,
                None => false,
            }
        })
        .collect()
}

// ─────────────────────────────── Runner ───────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Fetch and report but do not write.
    pub dry_run: bool,
    /// Injectable clock for deterministic tests.
    pub now: Option<i64>,
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SyncOutcome {
    pub job_id: String,
    pub status: String,
    pub rows_written: i64,
    pub cursor: Option<i64>,
    pub from: i64,
    pub to: i64,
    pub error: Option<String>,
}

/// Default look-back when a job has never run.
const DEFAULT_BACKFILL_MS: i64 = 7 * 86_400_000;

/// The window to request for this job.
///
/// `to` stops one bar short of the current one on purpose: the bar that is
/// still forming has a moving high/low/close, and writing it would store a
/// value that is wrong by construction.
pub fn plan_window(job: &SyncJob, now: i64) -> AppResult<(i64, i64)> {
    let step = crate::domain::interval_ms(&job.interval)?;
    let to = (now.div_euclid(step) * step) - step;
    let from = match job.cursor {
        Some(c) => c + step,
        None => std::cmp::max(0, to - DEFAULT_BACKFILL_MS),
    };
    Ok((from, to))
}

/// Async half: talk to the venue. Never holds a database lock.
pub async fn fetch_all(
    source: &DataSource,
    job: &SyncJob,
    from: i64,
    to: i64,
    limit: u64,
) -> AppResult<Vec<crate::market::Bar>> {
    let mut all = Vec::new();
    for symbol in &job.symbols {
        let fetched = source
            .fetch_bars(&FetchRequest {
                symbol: symbol.clone(),
                interval: job.interval.clone(),
                from,
                to,
                limit,
            })
            .await?;
        all.extend(fetched);
    }
    Ok(all)
}

/// Blocking half: write the bars, then move the cursor. Splitting it this way
/// lets the HTTP handler release the lock around the network call.
pub fn commit(
    market: &MarketStore,
    meta: &MetadataStore,
    job: &SyncJob,
    bars: &[crate::market::Bar],
    now: i64,
    dry_run: bool,
) -> AppResult<SyncOutcome> {
    let mut written = 0i64;
    let mut max_ts: Option<i64> = None;

    if !bars.is_empty() {
        max_ts = bars.iter().map(|b| b.ts).max();
        if !dry_run {
            written = market.upsert_bars(
                bars,
                &BarProvenance {
                    source: job.source.clone(),
                    namespace: crate::market::DEFAULT_NAMESPACE.into(),
                    ingested_at: Some(now),
                },
            )? as i64;
        } else {
            written = bars.len() as i64;
        }
    }

    // Only now does the cursor move.
    let new_cursor = max_ts.or(job.cursor);
    meta.update_job(
        &job.id,
        &JobPatch {
            status: Some("ok".into()),
            cursor: new_cursor,
            last_run_at: Some(now),
            last_error: None,
            rows_written: Some(written),
            ..Default::default()
        },
    )?;

    Ok(SyncOutcome {
        job_id: job.id.clone(),
        status: "ok".into(),
        rows_written: written,
        cursor: new_cursor,
        from: bars.first().map(|b| b.ts).unwrap_or(0),
        to: max_ts.unwrap_or(0),
        error: None,
    })
}

pub async fn run_job(
    market: &MarketStore,
    meta: &MetadataStore,
    job: &SyncJob,
    source: &DataSource,
    opts: &RunOptions,
) -> AppResult<SyncOutcome> {
    let now = opts.now.unwrap_or_else(crate::market::now_ms);
    let (from, to) = plan_window(job, now)?;
    if to < from {
        return Ok(SyncOutcome {
            job_id: job.id.clone(),
            status: "idle".into(),
            rows_written: 0,
            cursor: job.cursor,
            from,
            to,
            error: None,
        });
    }
    let bars = fetch_all(source, job, from, to, opts.limit.unwrap_or(1000)).await?;
    commit(market, meta, job, &bars, now, opts.dry_run)
}

/// Run a job and, on failure, record the error **without** moving the cursor.
/// That is what makes a failed sync retryable rather than lossy.
pub async fn run_job_guarded(
    market: &MarketStore,
    meta: &MetadataStore,
    job: &SyncJob,
    source: &DataSource,
    opts: &RunOptions,
) -> SyncOutcome {
    match run_job(market, meta, job, source, opts).await {
        Ok(o) => o,
        Err(e) => {
            let msg = e.to_string();
            let _ = meta.update_job(
                &job.id,
                &JobPatch {
                    status: Some("error".into()),
                    last_error: Some(msg.clone()),
                    last_run_at: Some(opts.now.unwrap_or_else(crate::market::now_ms)),
                    ..Default::default()
                },
            );
            SyncOutcome {
                job_id: job.id.clone(),
                status: "error".into(),
                rows_written: 0,
                cursor: job.cursor,
                from: 0,
                to: 0,
                error: Some(msg),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(s: &str) -> CronExpr {
        s.parse().unwrap()
    }

    #[test]
    fn parses_steps_lists_and_ranges() {
        assert_eq!(expr("*/15 * * * *").minutes, vec![0, 15, 30, 45]);
        assert_eq!(expr("0 9 * * 1-5").hours, vec![9]);
        assert_eq!(expr("0 9 * * 1-5").dows, vec![1, 2, 3, 4, 5]);
        assert_eq!(expr("5,10 * * * *").minutes, vec![5, 10]);
        assert_eq!(expr("0 0 1 1 *").months, vec![1]);
        assert!(
            "*/0 * * * *".parse::<CronExpr>().is_err(),
            "zero step must be rejected"
        );
        assert!("* * * *".parse::<CronExpr>().is_err(), "needs 5 fields");
        assert!("99 * * * *".parse::<CronExpr>().is_err(), "out of range");
    }

    #[test]
    fn next_after_advances_to_the_next_boundary() {
        // 2026-01-01T00:00Z = 1767225600000
        let base: i64 = 1_767_225_600_000;
        let e = expr("*/15 * * * *");
        assert_eq!(e.next_after(base), Some(base + 15 * 60_000));
        // One minute before the boundary still yields the boundary.
        assert_eq!(e.next_after(base + 14 * 60_000), Some(base + 15 * 60_000));
        // Strictly after: at the boundary itself we move on.
        assert_eq!(e.next_after(base + 15 * 60_000), Some(base + 30 * 60_000));
    }

    #[test]
    fn daily_schedule_rolls_over_midnight() {
        let base: i64 = 1_767_225_600_000; // 2026-01-01T00:00Z
        let e = expr("30 9 * * *");
        let next = e.next_after(base).unwrap();
        assert_eq!(next, base + 9 * 3_600_000 + 30 * 60_000);
    }

    #[test]
    fn weekday_schedule_skips_non_matching_days() {
        // 2026-01-01 is a Thursday (dow 4).
        let base: i64 = 1_767_225_600_000;
        let e = expr("0 0 * * 0"); // Sundays only
        let next = e.next_after(base).unwrap();
        let days = next / 86_400_000;
        assert_eq!(weekday_from_days(days), 0, "must land on a Sunday");
        assert!(next > base);
    }

    #[test]
    fn impossible_schedule_returns_none_instead_of_looping() {
        // Feb 30th never exists.
        let base: i64 = 1_767_225_600_000;
        assert!(expr("0 0 30 2 *").next_after(base).is_none());
    }

    #[test]
    fn only_due_jobs_are_selected() {
        let base: i64 = 1_767_225_600_000;
        let mk = |id: &str, cron: Option<&str>, last: Option<i64>| SyncJob {
            id: id.into(),
            dataset_key: "k".into(),
            source: "replay".into(),
            symbols: vec!["BTCUSDT".into()],
            interval: "1m".into(),
            cron: cron.map(|c| c.into()),
            mode: "incremental".into(),
            cursor: None,
            status: "idle".into(),
            last_run_at: last,
            last_error: None,
            rows_written: 0,
            enabled: true,
            created_at: base,
        };
        // `base` is itself a multiple of 5 minutes, so:
        //   never-ran  → its slot (base) is already in the past  → due
        //   just-ran   → ran at base, next slot is base+5m       → not due yet at base+6m...
        //                only if it actually ran *on* a slot boundary, which is
        //                why `last_run_at` is base+5m and not base.
        let jobs = vec![
            mk("never-ran", Some("*/5 * * * *"), None),
            mk("just-ran", Some("*/5 * * * *"), Some(base + 5 * 60_000)),
            mk("no-cron", None, None),
        ];
        let due = due_jobs(&jobs, base + 6 * 60_000);
        let ids: Vec<&str> = due.iter().map(|j| j.id.as_str()).collect();
        assert_eq!(ids, vec!["never-ran"]);
    }
}
