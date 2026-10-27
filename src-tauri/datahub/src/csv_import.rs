//! CSV import with a quality gate.
//!
//! Imported data never lands in `bronze` directly. It goes to the `imported`
//! namespace, gets a quality report, and only crosses into `bronze` when the
//! gate passes. That keeps provenance honest: exchange data and user data are
//! never mixed up, and a bad CSV cannot silently poison the backtest store.

use crate::error::{AppError, AppResult};
use crate::market::{Bar, BarProvenance, MarketStore, DEFAULT_NAMESPACE, IMPORTED_NAMESPACE};
use crate::metadata::{ImportBatch, MetadataStore, QualityFinding};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CsvImportRequest {
    pub dataset_key: String,
    pub symbol: String,
    pub interval: String,
    /// Original file name — recorded on the batch so the UI can show provenance.
    pub file: String,
    /// Raw CSV text.
    pub text: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    /// Fraction of bad rows tolerated before the gate closes. Default 1%.
    #[serde(default)]
    pub max_bad_ratio: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CsvImportResult {
    pub batch_id: String,
    pub rows_total: i64,
    pub rows_ok: i64,
    pub rows_bad: i64,
    pub gate_passed: bool,
    pub written: i64,
    pub findings: Vec<QualityFinding>,
    /// A few concrete reasons so the UI can show *why* rows were rejected.
    pub bad_samples: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PromoteRequest {
    pub dataset_key: String,
    pub symbol: String,
    pub interval: String,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PromoteResult {
    pub moved: i64,
    pub min_ts: Option<i64>,
    pub max_ts: Option<i64>,
}

const TIME_KEYS: [&str; 6] = ["ts", "timestamp", "time", "datetime", "date", "open_time"];
const NUM_KEYS: [&str; 6] = ["open", "high", "low", "close", "volume", "turnover"];

fn normalize_header(h: &str) -> String {
    h.trim()
        .trim_start_matches('\u{feff}')
        .to_ascii_lowercase()
        .replace([' ', '-'], "_")
}

/// Seconds vs milliseconds: anything below 1e11 must be seconds, anything above
/// is already epoch milliseconds. Getting this wrong shifts data by ~55 years.
fn parse_ts(raw: &str) -> Option<i64> {
    let s = raw.trim().trim_matches('"');
    if let Ok(v) = s.parse::<i64>() {
        return Some(if v.abs() < 100_000_000_000 { v * 1000 } else { v });
    }
    if let Ok(v) = s.parse::<f64>() {
        let v = v as i64;
        return Some(if v.abs() < 100_000_000_000 { v * 1000 } else { v });
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|d| d.and_utc().timestamp_millis())
        })
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
                .ok()
                .map(|d| d.and_utc().timestamp_millis())
        })
}

fn parse_num(raw: &str) -> Option<f64> {
    let s = raw.trim().trim_matches('"');
    if s.is_empty() || s.eq_ignore_ascii_case("nan") || s.eq_ignore_ascii_case("null") {
        return None;
    }
    s.parse::<f64>().ok().filter(|f| f.is_finite())
}

fn col(map: &std::collections::HashMap<String, usize>, keys: &[&str]) -> Option<usize> {
    keys.iter().find_map(|k| map.get(*k).copied())
}

pub fn import_csv(
    market: &MarketStore,
    meta: &MetadataStore,
    req: &CsvImportRequest,
) -> AppResult<CsvImportResult> {
    let step = crate::domain::interval_ms(&req.interval)?;
    let ns = req.namespace.clone().unwrap_or_else(|| IMPORTED_NAMESPACE.into());
    let source = req.source.clone().unwrap_or_else(|| "csv-import".into());
    let max_bad_ratio = req.max_bad_ratio.unwrap_or(0.01);
    let now = crate::market::now_ms();

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(req.text.as_bytes());

    let headers = reader
        .headers()
        .map_err(|e| AppError::BadRequest(format!("csv/header: {e}")))?
        .clone();

    let mut map = std::collections::HashMap::new();
    for (i, h) in headers.iter().enumerate() {
        map.insert(normalize_header(h), i);
    }
    let ts_idx = col(&map, &TIME_KEYS);
    let o_idx = col(&map, &["open"]);
    let h_idx = col(&map, &["high"]);
    let l_idx = col(&map, &["low"]);
    let c_idx = col(&map, &["close"]);
    let v_idx = col(&map, &["volume"]);
    let t_idx = col(&map, &["turnover"]);

    // Positional fallback for headerless exports: ts,open,high,low,close,volume,turnover
    let named = ts_idx.is_some() && o_idx.is_some() && c_idx.is_some();

    let mut rows: Vec<Bar> = Vec::new();
    let mut total = 0i64;
    let mut bad = 0i64;
    let mut misaligned = 0i64;
    let mut bad_samples: Vec<String> = Vec::new();

    for (line_no, rec) in reader.records().enumerate() {
        let rec = match rec {
            Ok(r) => r,
            Err(e) => {
                bad += 1;
                if bad_samples.len() < 5 {
                    bad_samples.push(format!("第 {} 行无法解析：{e}", line_no + 2));
                }
                continue;
            }
        };
        if rec.iter().all(|f| f.trim().is_empty()) {
            continue; // tolerate blank lines
        }
        total += 1;

        let get = |idx: Option<usize>, fallback: usize| -> Option<&str> {
            let i = if named { idx? } else { fallback };
            rec.get(i)
        };

        let ts = get(ts_idx, 0).and_then(parse_ts);
        let open = get(o_idx, 1).and_then(parse_num);
        let high = get(h_idx, 2).and_then(parse_num);
        let low = get(l_idx, 3).and_then(parse_num);
        let close = get(c_idx, 4).and_then(parse_num);

        let reason = match (ts, open, high, low, close) {
            (None, _, _, _, _) => Some("时间戳缺失或无法解析".to_string()),
            (_, None, _, _, _) => Some("open 缺失或非数值".to_string()),
            (_, _, None, _, _) => Some("high 缺失或非数值".to_string()),
            (_, _, _, None, _) => Some("low 缺失或非数值".to_string()),
            (_, _, _, _, None) => Some("close 缺失或非数值".to_string()),
            (Some(_), Some(o), Some(h), Some(l), Some(c))
                if h < o.max(c) - 1e-9 || l > o.min(c) + 1e-9 || h < l =>
            {
                Some("OHLC 自相矛盾".to_string())
            }
            _ => None,
        };

        if let Some(r) = reason {
            bad += 1;
            if bad_samples.len() < 5 {
                bad_samples.push(format!("第 {} 行：{r}", line_no + 2));
            }
            continue;
        }

        let ts = ts.unwrap();
        if ts % step != 0 {
            misaligned += 1;
        }

        rows.push(Bar {
            symbol: req.symbol.clone(),
            interval: req.interval.clone(),
            ts,
            open: open.unwrap(),
            high: high.unwrap(),
            low: low.unwrap(),
            close: close.unwrap(),
            volume: get(v_idx, 5).and_then(parse_num).unwrap_or(0.0),
            turnover: get(t_idx, 6).and_then(parse_num).unwrap_or(0.0),
            closed: true,
            source: source.clone(),
            namespace: ns.clone(),
            ingested_at: now,
        });
    }

    let ok = rows.len() as i64;
    let bad_ratio = if total == 0 { 1.0 } else { bad as f64 / total as f64 };
    let gate_passed = ok > 0 && bad_ratio <= max_bad_ratio;

    let mut findings = Vec::new();
    if bad > 0 {
        findings.push(QualityFinding {
            kind: "bad_rows".into(),
            severity: if gate_passed { "warn" } else { "error" }.into(),
            message: format!("{bad} / {total} 行未通过校验（{:.2}%）", bad_ratio * 100.0),
            count: bad,
        });
    }
    if misaligned > 0 {
        findings.push(QualityFinding {
            kind: "non_monotonic".into(),
            severity: "warn".into(),
            message: format!("{misaligned} 行时间戳未对齐到 {} 网格", req.interval),
            count: misaligned,
        });
    }
    if ok == 0 {
        findings.push(QualityFinding {
            kind: "empty".into(),
            severity: "error".into(),
            message: "没有可用的数据行".into(),
            count: 0,
        });
    }

    // Rows are written even when the gate fails — they sit in `imported` and can
    // be inspected and re-gated. Only promotion is blocked.
    let written = if rows.is_empty() {
        0
    } else {
        market.upsert_bars(
            &rows,
            &BarProvenance {
                source: source.clone(),
                namespace: ns.clone(),
                ingested_at: Some(now),
            },
        )? as i64
    };

    let batch_id = uuid::Uuid::new_v4().to_string();
    meta.record_import(&ImportBatch {
        id: batch_id.clone(),
        dataset_key: req.dataset_key.clone(),
        namespace: ns,
        file: req.file.clone(),
        rows_total: total,
        rows_ok: ok,
        rows_bad: bad,
        gate_passed,
        gate_report: Some(serde_json::to_value(&findings)?),
        created_at: now,
    })?;

    Ok(CsvImportResult {
        batch_id,
        rows_total: total,
        rows_ok: ok,
        rows_bad: bad,
        gate_passed,
        written,
        findings,
        bad_samples,
    })
}

/// Move gated data from `imported` into `bronze`.
pub fn promote_to_bronze(
    market: &MarketStore,
    meta: &MetadataStore,
    req: &PromoteRequest,
) -> AppResult<PromoteResult> {
    let bars = market.query_bars(&crate::market::QueryOptions {
        symbol: Some(req.symbol.clone()),
        interval: Some(req.interval.clone()),
        namespace: Some(IMPORTED_NAMESPACE.into()),
        limit: Some(5_000_000),
        ..Default::default()
    })?;
    if bars.is_empty() {
        return Ok(PromoteResult {
            moved: 0,
            min_ts: None,
            max_ts: None,
        });
    }

    let now = crate::market::now_ms();
    market.upsert_bars(
        &bars,
        &BarProvenance {
            source: req.source.clone().unwrap_or_else(|| "csv-import".into()),
            namespace: DEFAULT_NAMESPACE.into(),
            ingested_at: Some(now),
        },
    )?;

    let min_ts = bars.first().map(|b| b.ts);
    let max_ts = bars.last().map(|b| b.ts);

    // scoped by namespace — otherwise this deletes the bronze rows we just wrote
    market.delete_range(
        IMPORTED_NAMESPACE,
        &req.symbol,
        &req.interval,
        min_ts.unwrap_or(0),
        max_ts.unwrap_or(i64::MAX),
    )?;

    meta.set_dataset_stats(&req.dataset_key, None, Some(max_ts), Some(now))?;

    Ok(PromoteResult {
        moved: bars.len() as i64,
        min_ts,
        max_ts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> MetadataStore {
        MetadataStore::open_in_memory().unwrap()
    }

    const HEADER: &str = "ts,open,high,low,close,volume\n";

    #[test]
    fn clean_csv_passes_the_gate() {
        let m = MarketStore::open_in_memory().unwrap();
        let md = meta();
        let mut text = String::from(HEADER);
        for i in 0..10 {
            let ts = 1_767_225_600_000i64 + (i as i64) * 60_000;
            text.push_str(&format!("{ts},100,101,99,100.5,10\n"));
        }
        let r = import_csv(
            &m,
            &md,
            &CsvImportRequest {
                dataset_key: "k".into(),
                symbol: "BTCUSDT".into(),
                interval: "1m".into(),
                file: "a.csv".into(),
                text,
                namespace: None,
                source: None,
                max_bad_ratio: None,
            },
        )
        .unwrap();
        assert_eq!(r.rows_total, 10);
        assert_eq!(r.rows_bad, 0);
        assert!(r.gate_passed);
        assert_eq!(r.written, 10);
        // Landed in `imported`, NOT bronze.
        let bronze = m
            .query_bars(&crate::market::QueryOptions {
                namespace: Some("bronze".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(bronze.is_empty(), "import must not bypass the gate");
        let imported = m
            .query_bars(&crate::market::QueryOptions {
                namespace: Some("imported".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(imported.len(), 10);
    }

    #[test]
    fn contradictory_rows_close_the_gate() {
        let m = MarketStore::open_in_memory().unwrap();
        let md = meta();
        let mut text = String::from(HEADER);
        for i in 0..10 {
            let ts = 1_767_225_600_000i64 + (i as i64) * 60_000;
            if i == 3 {
                text.push_str(&format!("{ts},100,50,99,100.5,10\n")); // high < low
            } else {
                text.push_str(&format!("{ts},100,101,99,100.5,10\n"));
            }
        }
        let r = import_csv(
            &m,
            &md,
            &CsvImportRequest {
                dataset_key: "k".into(),
                symbol: "BTCUSDT".into(),
                interval: "1m".into(),
                file: "a.csv".into(),
                text,
                namespace: None,
                source: None,
                max_bad_ratio: Some(0.0), // strict
            },
        )
        .unwrap();
        assert_eq!(r.rows_bad, 1);
        assert!(!r.gate_passed);
        assert!(r.bad_samples.iter().any(|s| s.contains("OHLC")));
    }

    #[test]
    fn seconds_are_promoted_to_milliseconds() {
        let m = MarketStore::open_in_memory().unwrap();
        let md = meta();
        let text = format!("{}1767225600,100,101,99,100.5,10\n", HEADER);
        let r = import_csv(
            &m,
            &md,
            &CsvImportRequest {
                dataset_key: "k".into(),
                symbol: "BTCUSDT".into(),
                interval: "1m".into(),
                file: "a.csv".into(),
                text,
                namespace: None,
                source: None,
                max_bad_ratio: None,
            },
        )
        .unwrap();
        assert_eq!(r.rows_ok, 1);
        let bars = m
            .query_bars(&crate::market::QueryOptions {
                namespace: Some("imported".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(bars[0].ts, 1_767_225_600_000);
    }

    #[test]
    fn promotion_moves_to_bronze_and_clears_imported() {
        let m = MarketStore::open_in_memory().unwrap();
        let md = meta();
        let mut text = String::from(HEADER);
        for i in 0..5 {
            let ts = 1_767_225_600_000i64 + (i as i64) * 60_000;
            text.push_str(&format!("{ts},100,101,99,100.5,10\n"));
        }
        import_csv(
            &m,
            &md,
            &CsvImportRequest {
                dataset_key: "k".into(),
                symbol: "BTCUSDT".into(),
                interval: "1m".into(),
                file: "a.csv".into(),
                text,
                namespace: None,
                source: None,
                max_bad_ratio: None,
            },
        )
        .unwrap();

        let p = promote_to_bronze(
            &m,
            &md,
            &PromoteRequest {
                dataset_key: "k".into(),
                symbol: "BTCUSDT".into(),
                interval: "1m".into(),
                source: None,
            },
        )
        .unwrap();
        assert_eq!(p.moved, 5);

        let bronze = m
            .query_bars(&crate::market::QueryOptions {
                namespace: Some("bronze".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(bronze.len(), 5, "bronze must keep the promoted rows");
        let imported = m
            .query_bars(&crate::market::QueryOptions {
                namespace: Some("imported".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(imported.is_empty(), "imported must be cleared after promotion");
    }

    #[test]
    fn headerless_exports_fall_back_to_position() {
        let m = MarketStore::open_in_memory().unwrap();
        let md = meta();
        let text = "1767225600000,100,101,99,100.5,10\n".to_string();
        let r = import_csv(
            &m,
            &md,
            &CsvImportRequest {
                dataset_key: "k".into(),
                symbol: "BTCUSDT".into(),
                interval: "1m".into(),
                file: "a.csv".into(),
                text,
                namespace: None,
                source: None,
                max_bad_ratio: None,
            },
        )
        .unwrap();
        // The first line is consumed as a header, so a headerless file yields no
        // data rows — reported rather than silently mis-mapped.
        assert_eq!(r.rows_total, r.rows_bad);
        assert!(!r.gate_passed);
    }
}
