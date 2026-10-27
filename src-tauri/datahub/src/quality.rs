//! Data quality checks.
//!
//! Principle: **report, never auto-repair.** Silently "fixing" bad bars hides
//! the problem and lets a backtest produce beautiful results from a wrong
//! database. Every finding is surfaced with a severity so the gate can block
//! promotion while still letting a human see warnings.
//!
//! Checks: gaps / staleness / OHLC contradictions / nulls / duplicates /
//! misaligned timestamps.

use crate::domain::detect_gaps;
use crate::error::AppResult;
use crate::market::{MarketStore, DEFAULT_NAMESPACE};
use crate::metadata::QualityFinding;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QualityOptions {
    pub symbol: String,
    pub interval: String,
    pub from_ts: i64,
    pub to_ts: i64,
    #[serde(default)]
    pub namespace: Option<String>,
    /// Injectable clock so staleness is testable.
    #[serde(default)]
    pub now: Option<i64>,
    /// No new bar within this many ms counts as stale. Defaults to 10 intervals.
    #[serde(default)]
    pub max_stale_ms: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QualityReport {
    pub findings: Vec<QualityFinding>,
    pub passed: bool,
    pub expected: i64,
    pub present: i64,
    pub coverage_ratio: f64,
    pub min_ts: Option<i64>,
    pub max_ts: Option<i64>,
}

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

/// Single value from a guarded read-only query. Returns `None` when the query
/// yields nothing — a failed helper must not take the whole report down.
fn scalar(market: &MarketStore, sql: &str) -> Option<i64> {
    let r = market.run_read_only(sql, 1).ok()?;
    if r.status != "ok" {
        return None;
    }
    let first = r.rows.first()?;
    let v = first.as_object()?.values().next()?;
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

pub fn check_quality(market: &MarketStore, opts: &QualityOptions) -> AppResult<QualityReport> {
    let step = crate::domain::interval_ms(&opts.interval)?;
    let ns = opts.namespace.clone().unwrap_or_else(|| DEFAULT_NAMESPACE.into());
    let now = opts.now.unwrap_or_else(crate::market::now_ms);
    let max_stale = opts.max_stale_ms.unwrap_or(step * 10);

    let ts = market.timestamps(&opts.symbol, &opts.interval, opts.from_ts, opts.to_ts, Some(&ns))?;
    let gaps = detect_gaps(opts.from_ts, opts.to_ts, step, &ts)?;

    let mut findings: Vec<QualityFinding> = Vec::new();

    // 1) gaps
    if !gaps.missing.is_empty() {
        let severe = gaps.coverage_ratio < 0.95;
        findings.push(QualityFinding {
            kind: "gap".into(),
            severity: if severe { "error" } else { "warn" }.into(),
            message: format!(
                "区间内缺失 {} 根 bar（覆盖率 {:.1}%）",
                gaps.missing.len(),
                gaps.coverage_ratio * 100.0
            ),
            count: gaps.missing.len() as i64,
        });
    }

    let range = market.range_of(&opts.symbol, &opts.interval, Some(&ns))?;

    // 2) staleness
    if let Some(max_ts) = range.max_ts {
        let lag = now - max_ts;
        if lag > max_stale {
            findings.push(QualityFinding {
                kind: "stale".into(),
                severity: if lag > max_stale * 6 { "error" } else { "warn" }.into(),
                message: format!(
                    "数据滞后 {} 个周期（最新 bar 距今 {}s）",
                    lag / step,
                    lag / 1000
                ),
                count: 1,
            });
        }
    }

    let scope = format!(
        "symbol = '{}' AND \"interval\" = '{}' AND namespace = '{}' AND ts >= {} AND ts <= {}",
        esc(&opts.symbol),
        esc(&opts.interval),
        esc(&ns),
        opts.from_ts,
        opts.to_ts
    );

    // 3) OHLC self-consistency
    let ohlc_bad = scalar(
        market,
        &format!(
            "SELECT count(*) AS c FROM bars WHERE {scope} \
             AND (high < low OR high < greatest(open, close) OR low > least(open, close))"
        ),
    )
    .unwrap_or(0);
    if ohlc_bad > 0 {
        findings.push(QualityFinding {
            kind: "ohlc_violation".into(),
            severity: "error".into(),
            message: format!("{ohlc_bad} 根 bar 的 OHLC 自相矛盾（high/low 未包住 open/close）"),
            count: ohlc_bad,
        });
    }

    // 4) nulls
    let nulls = scalar(
        market,
        &format!(
            "SELECT count(*) AS c FROM bars WHERE {scope} \
             AND (open IS NULL OR high IS NULL OR low IS NULL OR close IS NULL)"
        ),
    )
    .unwrap_or(0);
    if nulls > 0 {
        findings.push(QualityFinding {
            kind: "null_value".into(),
            severity: "error".into(),
            message: format!("{nulls} 根 bar 存在空值字段"),
            count: nulls,
        });
    }

    // 5) duplicates — the primary key already guarantees uniqueness within a
    // namespace; this is the belt-and-braces check for future schema drift.
    let dup = scalar(
        market,
        &format!("SELECT count(*) - count(DISTINCT ts) AS d FROM bars WHERE {scope}"),
    )
    .unwrap_or(0);
    if dup > 0 {
        findings.push(QualityFinding {
            kind: "duplicate".into(),
            severity: "error".into(),
            message: format!("{dup} 个重复时间戳"),
            count: dup,
        });
    }

    // 6) grid alignment — off-grid timestamps break aggregation and coverage.
    let misaligned = ts.iter().filter(|t| *t % step != 0).count() as i64;
    if misaligned > 0 {
        findings.push(QualityFinding {
            kind: "non_monotonic".into(),
            severity: "warn".into(),
            message: format!("{misaligned} 根 bar 的时间戳未对齐到 {} 网格", opts.interval),
            count: misaligned,
        });
    }

    let passed = !findings.iter().any(|f| f.severity == "error");
    Ok(QualityReport {
        findings,
        passed,
        expected: gaps.expected,
        present: gaps.present,
        coverage_ratio: gaps.coverage_ratio,
        min_ts: range.min_ts,
        max_ts: range.max_ts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::{Bar, BarProvenance};

    fn bars(symbol: &str, from: i64, n: i64) -> Vec<Bar> {
        (0..n)
            .map(|i| {
                let ts = from + i * 60_000;
                Bar {
                    symbol: symbol.into(),
                    interval: "1m".into(),
                    ts,
                    open: 100.0,
                    high: 101.0,
                    low: 99.0,
                    close: 100.5,
                    volume: 10.0,
                    turnover: 1000.0,
                    closed: true,
                    source: "test".into(),
                    namespace: DEFAULT_NAMESPACE.into(),
                    ingested_at: 0,
                }
            })
            .collect()
    }

    fn prov() -> BarProvenance {
        BarProvenance {
            source: "test".into(),
            namespace: DEFAULT_NAMESPACE.into(),
            ingested_at: Some(0),
        }
    }

    #[test]
    fn clean_data_passes() {
        let m = MarketStore::open_in_memory().unwrap();
        let from = 1_767_225_600_000;
        m.upsert_bars(&bars("BTCUSDT", from, 10), &prov()).unwrap();
        let r = check_quality(
            &m,
            &QualityOptions {
                symbol: "BTCUSDT".into(),
                interval: "1m".into(),
                from_ts: from,
                to_ts: from + 9 * 60_000,
                namespace: None,
                now: Some(from + 10 * 60_000),
                max_stale_ms: None,
            },
        )
        .unwrap();
        assert!(r.passed, "findings: {:?}", r.findings);
        assert_eq!(r.present, 10);
        assert!((r.coverage_ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_hole_in_the_middle_is_reported_as_a_gap() {
        let m = MarketStore::open_in_memory().unwrap();
        let from = 1_767_225_600_000;
        let mut b = bars("BTCUSDT", from, 10);
        b.remove(4); // punch out one minute
        m.upsert_bars(&b, &prov()).unwrap();
        let r = check_quality(
            &m,
            &QualityOptions {
                symbol: "BTCUSDT".into(),
                interval: "1m".into(),
                from_ts: from,
                to_ts: from + 9 * 60_000,
                namespace: None,
                now: Some(from + 10 * 60_000),
                max_stale_ms: None,
            },
        )
        .unwrap();
        let gap = r.findings.iter().find(|f| f.kind == "gap").expect("gap reported");
        assert_eq!(gap.count, 1);
        // 9/10 coverage is below the 95% bar, so it must be an error.
        assert_eq!(gap.severity, "error");
    }

    #[test]
    fn contradictory_ohlc_fails_the_gate() {
        let m = MarketStore::open_in_memory().unwrap();
        let from = 1_767_225_600_000;
        let mut b = bars("BTCUSDT", from, 3);
        b[1].high = 50.0; // below low/close
        m.upsert_bars(&b, &prov()).unwrap();
        let r = check_quality(
            &m,
            &QualityOptions {
                symbol: "BTCUSDT".into(),
                interval: "1m".into(),
                from_ts: from,
                to_ts: from + 2 * 60_000,
                namespace: None,
                now: Some(from + 3 * 60_000),
                max_stale_ms: None,
            },
        )
        .unwrap();
        assert!(r.findings.iter().any(|f| f.kind == "ohlc_violation"));
        assert!(!r.passed);
    }

    #[test]
    fn stale_data_is_flagged_when_the_clock_moves_past_it() {
        let m = MarketStore::open_in_memory().unwrap();
        let from = 1_767_225_600_000;
        m.upsert_bars(&bars("BTCUSDT", from, 3), &prov()).unwrap();
        let r = check_quality(
            &m,
            &QualityOptions {
                symbol: "BTCUSDT".into(),
                interval: "1m".into(),
                from_ts: from,
                to_ts: from + 2 * 60_000,
                namespace: None,
                now: Some(from + 600 * 60_000), // ten hours later
                max_stale_ms: None,
            },
        )
        .unwrap();
        let stale = r.findings.iter().find(|f| f.kind == "stale").expect("stale");
        assert_eq!(stale.severity, "error", "way past 6x the stale window");
    }
}
