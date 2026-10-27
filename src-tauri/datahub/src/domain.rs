//! Market / interval vocabulary and the dataset-key scheme.
//!
//! This is the Rust twin of the plugin's `src/domain/market.ts`. The React
//! client still bundles the TypeScript copy, so **the two must agree** on
//! interval spellings and millisecond values — otherwise the coverage heatmap
//! and the quality report would disagree with the daemon that produced them.

use crate::error::{AppError, AppResult};
use chrono::Datelike;
use serde::{Deserialize, Serialize};

// ─────────────────────────────── Market ───────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Market {
    Crypto,
    AShare,
    Us,
    Hk,
}

impl Market {
    pub const ALL: [Market; 4] = [Market::Crypto, Market::AShare, Market::Us, Market::Hk];

    /// Wire form, matching `Market` in `domain/market.ts`.
    pub fn as_str(self) -> &'static str {
        match self {
            Market::Crypto => "CRYPTO",
            Market::AShare => "A_SHARE",
            Market::Us => "US",
            Market::Hk => "HK",
        }
    }

    pub fn parse(s: &str) -> AppResult<Market> {
        Market::ALL
            .iter()
            .find(|m| m.as_str().eq_ignore_ascii_case(s))
            .copied()
            .ok_or_else(|| AppError::BadRequest(format!("market/unknown: {s}")))
    }

    /// Markets without a trading calendar. Anything else must be given an
    /// explicit calendar later — never silently filled with a 24/7 grid.
    pub fn is_24_7(self) -> bool {
        matches!(self, Market::Crypto)
    }
}

// ─────────────────────────────── Interval ───────────────────────────────

pub const INTERVALS: [(&str, i64); 9] = [
    ("1m", 60_000),
    ("3m", 180_000),
    ("5m", 300_000),
    ("15m", 900_000),
    ("30m", 1_800_000),
    ("1h", 3_600_000),
    ("2h", 7_200_000),
    ("4h", 14_400_000),
    ("1d", 86_400_000),
];

pub fn is_interval(s: &str) -> bool {
    INTERVALS.iter().any(|(n, _)| *n == s)
}

/// Interval → milliseconds. Unknown intervals are a hard error (fail-closed):
/// silently defaulting to 1m would make coverage and quality report nonsense
/// numbers for daily data.
pub fn interval_ms(i: &str) -> AppResult<i64> {
    INTERVALS
        .iter()
        .find(|(n, _)| *n == i)
        .map(|(_, ms)| *ms)
        .ok_or_else(|| AppError::BadRequest(format!("interval/unknown: {i}")))
}

/// Reverse lookup; `None` for non-standard spacings.
pub fn interval_of(ms: i64) -> Option<&'static str> {
    INTERVALS.iter().find(|(_, v)| *v == ms).map(|(n, _)| *n)
}

/// Expected bars in one calendar day (24/7 markets).
pub fn bars_per_day(i: &str) -> AppResult<i64> {
    let ms = interval_ms(i)?;
    Ok((86_400_000 + ms / 2) / ms)
}

// ─────────────────────────────── Dataset key ───────────────────────────────

/// Globally unique, stable, readable: `crypto:binance:BTCUSDT:1m`.
/// A non-bronze namespace appends `:imported` so imported data never collides
/// with exchange data in the catalog.
pub fn make_dataset_key(
    market: Market,
    exchange: &str,
    symbol: &str,
    interval: &str,
    namespace: Option<&str>,
) -> String {
    let ns = match namespace {
        Some(n) if !n.is_empty() && n != "bronze" => format!(":{n}"),
        _ => String::new(),
    };
    format!(
        "{}:{}:{}:{}{}",
        market.as_str().to_ascii_lowercase(),
        exchange,
        symbol,
        interval,
        ns
    )
}

// ─────────────────────────────── Coverage ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoverageCell {
    /// `YYYY-MM-DD`
    pub day: String,
    /// 0..23
    pub hour: i64,
    pub present: i64,
    pub expected: i64,
    pub ratio: f64,
}

/// Continuous expected grid for a 24/7 market, aligned to interval boundaries.
/// Bounds are snapped outward so a partial bar is never reported as missing.
pub fn expected_bars(start_ts: i64, end_ts: i64, step_ms: i64) -> AppResult<Vec<i64>> {
    if step_ms <= 0 {
        return Err(AppError::BadRequest("coverage/invalid-input".into()));
    }
    if end_ts < start_ts {
        return Ok(Vec::new());
    }
    let first = start_ts.div_euclid(step_ms) * step_ms;
    let last = end_ts.div_euclid(step_ms) * step_ms;
    let mut out = Vec::with_capacity(((last - first) / step_ms + 1) as usize);
    let mut t = first;
    while t <= last {
        out.push(t);
        t += step_ms;
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GapReport {
    pub start_ts: i64,
    pub end_ts: i64,
    pub interval_ms: i64,
    pub expected: i64,
    pub present: i64,
    pub missing: Vec<i64>,
    pub coverage_ratio: f64,
}

pub fn detect_gaps(start_ts: i64, end_ts: i64, step_ms: i64, actual: &[i64]) -> AppResult<GapReport> {
    let expected = expected_bars(start_ts, end_ts, step_ms)?;
    let have: std::collections::HashSet<i64> = actual.iter().copied().collect();
    let missing: Vec<i64> = expected.iter().copied().filter(|t| !have.contains(t)).collect();
    let present = expected.len() as i64 - missing.len() as i64;
    let coverage_ratio = if expected.is_empty() {
        1.0
    } else {
        present as f64 / expected.len() as f64
    };
    Ok(GapReport {
        start_ts,
        end_ts,
        interval_ms: step_ms,
        expected: expected.len() as i64,
        present,
        missing,
        coverage_ratio,
    })
}

fn days_in_month(year: i32, month: u32) -> u32 {
    // First day of the next month, minus one day.
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    match chrono::NaiveDate::from_ymd_opt(ny, nm, 1).and_then(|d| d.pred_opt()) {
        Some(d) => d.day(),
        None => 31,
    }
}

/// Day × hour matrix behind the coverage heatmap. Missing hours come back with
/// `present = 0` rather than being skipped, so the UI renders empty cells
/// instead of inventing data.
pub fn coverage_matrix(
    year: i32,
    month: u32,
    step_ms: i64,
    actual: &[i64],
) -> AppResult<Vec<CoverageCell>> {
    if step_ms <= 0 {
        return Err(AppError::BadRequest("coverage/invalid-input".into()));
    }
    // For intervals coarser than an hour the grid still buckets by hour, but
    // "expected" is how many bars that hour should hold (1 for 1h, <1 → clamp 1).
    let bars_per_hour = std::cmp::max(1, (3_600_000 + step_ms / 2) / step_ms);
    let have: std::collections::HashSet<i64> = actual.iter().copied().collect();

    let mut cells = Vec::new();
    for day in 1..=days_in_month(year, month) {
        for hour in 0..24 {
            let hour_start = chrono::NaiveDate::from_ymd_opt(year, month, day)
                .and_then(|d| d.and_hms_opt(hour, 0, 0))
                .map(|dt| dt.and_utc().timestamp_millis())
                .ok_or_else(|| AppError::BadRequest(format!("coverage/bad-date {year}-{month}-{day}")))?;

            // Snap down to the interval grid. Bars are stored grid-aligned, so
            // comparing raw hour_start against them would miss everything.
            let first_bar = hour_start.div_euclid(step_ms) * step_ms;
            let mut present = 0i64;
            for b in 0..bars_per_hour {
                if have.contains(&(first_bar + b * step_ms)) {
                    present += 1;
                }
            }
            cells.push(CoverageCell {
                day: format!("{:04}-{:02}-{:02}", year, month, day),
                hour: hour as i64,
                present,
                expected: bars_per_hour,
                ratio: present as f64 / bars_per_hour as f64,
            });
        }
    }
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_ms_matches_the_typescript_table() {
        assert_eq!(interval_ms("1m").unwrap(), 60_000);
        assert_eq!(interval_ms("1d").unwrap(), 86_400_000);
        assert!(interval_ms("7m").is_err(), "unknown interval must fail closed");
    }

    #[test]
    fn dataset_key_is_stable_and_namespace_aware() {
        assert_eq!(
            make_dataset_key(Market::Crypto, "binance", "BTCUSDT", "1m", None),
            "crypto:binance:BTCUSDT:1m"
        );
        assert_eq!(
            make_dataset_key(Market::Crypto, "binance", "BTCUSDT", "1m", Some("imported")),
            "crypto:binance:BTCUSDT:1m:imported"
        );
    }

    #[test]
    fn expected_bars_snaps_outward_to_the_grid() {
        let g = expected_bars(90_000, 210_000, 60_000).unwrap();
        assert_eq!(g, vec![60_000, 120_000, 180_000]);
    }

    #[test]
    fn gaps_are_detected_and_ratio_is_right() {
        let r = detect_gaps(0, 180_000, 60_000, &[0, 60_000]).unwrap();
        assert_eq!(r.expected, 4);
        assert_eq!(r.present, 2);
        assert_eq!(r.missing, vec![120_000, 180_000]);
        assert!((r.coverage_ratio - 0.5).abs() < 1e-9);
    }

    #[test]
    fn empty_range_is_full_coverage_not_zero() {
        let r = detect_gaps(100, 0, 60_000, &[]).unwrap();
        assert_eq!(r.expected, 0);
        assert_eq!(r.coverage_ratio, 1.0);
    }

    #[test]
    fn coverage_matrix_reports_missing_hours_as_zero() {
        // Jan 2026 has 31 days. Feed only the very first minute of the month
        // (2026-01-01T00:00Z) — a bar at ts 0 belongs to 1970, not this grid.
        let first_minute = 1_767_225_600_000;
        let cells = coverage_matrix(2026, 1, 60_000, &[first_minute]).unwrap();
        assert_eq!(cells.len(), 31 * 24);
        let first = &cells[0];
        assert_eq!(first.day, "2026-01-01");
        assert_eq!(first.hour, 0);
        assert_eq!(first.present, 1, "the fed minute must land in hour 0 of day 1");
        assert_eq!(first.expected, 60);
        assert_eq!(cells[1].present, 0, "absent hours are present=0, not skipped");
    }
}
