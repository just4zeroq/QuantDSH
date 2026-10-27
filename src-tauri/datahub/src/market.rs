//! DuckDB market store — the single writer for OHLCV bars.
//!
//! DuckDB is doing three jobs here at once, which is why it beat Parquet as the
//! primary store for bars:
//!   * columnar analytics (range scans, aggregates over millions of rows)
//!   * real `UPDATE` / `DELETE` / upsert, so corrections and re-pulls are one
//!     statement instead of a hand-rolled override table
//!   * a single embedded file, no server — same deployment shape as SQLite
//!
//! The connection is owned exclusively by this struct and every call site goes
//! through the daemon's mutex. DuckDB's MVCC handles concurrent readers *within*
//! one process; the cross-process single-writer limit is why we funnel
//! everything through one daemon rather than letting each plugin open the file.

use crate::error::{AppError, AppResult};
use crate::sql_guard::{guard_sql, GuardVerdict};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const DEFAULT_NAMESPACE: &str = "bronze";
pub const IMPORTED_NAMESPACE: &str = "imported";

const BARS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS bars (
  symbol      VARCHAR NOT NULL,
  "interval"  VARCHAR NOT NULL,
  ts          BIGINT  NOT NULL,
  open        DOUBLE,
  high        DOUBLE,
  low         DOUBLE,
  close       DOUBLE,
  volume      DOUBLE,
  turnover    DOUBLE,
  closed      BOOLEAN NOT NULL DEFAULT TRUE,
  source      VARCHAR NOT NULL DEFAULT 'unknown',
  namespace   VARCHAR NOT NULL DEFAULT 'bronze',
  ingested_at BIGINT  NOT NULL,
  -- namespace MUST be part of the key. Without it an imported row would overwrite
  -- the exchange row for the same ts and the provenance would be lost silently.
  PRIMARY KEY (namespace, symbol, "interval", ts)
);
CREATE INDEX IF NOT EXISTS idx_bars_interval_ts ON bars("interval", ts);
CREATE INDEX IF NOT EXISTS idx_bars_symbol ON bars(symbol);
"#;

// ─────────────────────────────── Types ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bar {
    pub symbol: String,
    pub interval: String,
    pub ts: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    #[serde(default)]
    pub volume: f64,
    #[serde(default)]
    pub turnover: f64,
    #[serde(default = "default_true")]
    pub closed: bool,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default = "default_ns")]
    pub namespace: String,
    #[serde(default)]
    pub ingested_at: i64,
}

fn default_true() -> bool {
    true
}
fn default_source() -> String {
    "unknown".into()
}
fn default_ns() -> String {
    DEFAULT_NAMESPACE.into()
}

/// Who produced a batch and when. Stamped onto every written row so the UI can
/// always answer "where did this number come from".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarProvenance {
    pub source: String,
    #[serde(default = "default_ns")]
    pub namespace: String,
    #[serde(default)]
    pub ingested_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryOptions {
    pub symbol: Option<String>,
    pub interval: Option<String>,
    pub namespace: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangeInfo {
    pub min_ts: Option<i64>,
    pub max_ts: Option<i64>,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadOnlyResult {
    pub status: String, // ok | blocked | error | timeout
    pub columns: Vec<String>,
    pub rows: Vec<serde_json::Value>,
    pub truncated: bool,
    pub duration_ms: u64,
    pub reason: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
}

// ─────────────────────────────── Helpers ───────────────────────────────

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

/// Wall-clock milliseconds. Public because the metadata store stamps rows with
/// the same clock — two different "now" sources would make `updated_at` and
/// `ingested_at` drift apart in the UI.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// DuckDB hands back a typed `Value`; flatten it to JSON. Getting `i64` here
/// (rather than a JS `BigInt`) is the whole reason BIGINT is safe for `ts`:
/// serialisation can never blow up on a big integer.
fn value_to_json(v: duckdb::types::Value) -> serde_json::Value {
    use duckdb::types::Value as V;
    match v {
        V::Null => serde_json::Value::Null,
        V::Boolean(b) => json!(b),
        V::TinyInt(n) => json!(n),
        V::SmallInt(n) => json!(n),
        V::Int(n) => json!(n),
        V::BigInt(n) => json!(n),
        V::UTinyInt(n) => json!(n),
        V::USmallInt(n) => json!(n),
        V::UInt(n) => json!(n),
        V::UBigInt(n) => json!(n),
        V::Float(n) => json!(n),
        V::Double(n) => json!(n),
        V::Text(s) => json!(s),
        V::Blob(b) => json!(format!("0x{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())),
        other => json!(format!("{other:?}")),
    }
}

/// Result column names for a query that returned no rows.
///
/// `DESCRIBE <query>` resolves the output shape without touching the data, so
/// an empty `SELECT` still reports its real column names instead of `col0..colN`.
fn describe_columns(conn: &duckdb::Connection, sql: &str) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare(&format!("DESCRIBE {sql}"))?;
    let mapped = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for name in mapped {
        out.push(name?);
    }
    Ok(out)
}

// ─────────────────────────────── Store ───────────────────────────────

pub struct MarketStore {
    conn: duckdb::Connection,
}

// duckdb::Connection is Send but not Sync; the daemon wraps this in a Mutex,
// which is what makes it safe to share across axum worker tasks.
unsafe impl Send for MarketStore {}

impl MarketStore {
    pub fn open(path: &str) -> AppResult<Self> {
        let conn = duckdb::Connection::open(path)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    pub fn open_in_memory() -> AppResult<Self> {
        Self::open(":memory:")
    }

    fn init_schema(&self) -> AppResult<()> {
        self.conn.execute_batch(BARS_DDL)?;
        Ok(())
    }

    /// Idempotent upsert. `INSERT OR REPLACE` + the composite primary key means
    /// re-running a sync over an overlapping window can never duplicate rows —
    /// this is what makes "write data, then advance the cursor" safe to retry.
    pub fn upsert_bars(&self, bars: &[Bar], prov: &BarProvenance) -> AppResult<usize> {
        if bars.is_empty() {
            return Ok(0);
        }
        let ingested = prov.ingested_at.unwrap_or_else(now_ms);
        let sql = r#"INSERT OR REPLACE INTO bars
            (symbol, "interval", ts, open, high, low, close, volume, turnover, closed, source, namespace, ingested_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#;
        let mut stmt = self.conn.prepare(sql)?;
        let mut written = 0usize;
        for b in bars {
            stmt.execute(duckdb::params![
                b.symbol,
                b.interval,
                b.ts,
                b.open,
                b.high,
                b.low,
                b.close,
                b.volume,
                b.turnover,
                b.closed,
                prov.source,
                prov.namespace,
                ingested,
            ])?;
            written += 1;
        }
        Ok(written)
    }

    pub fn query_bars(&self, opts: &QueryOptions) -> AppResult<Vec<Bar>> {
        let ns = opts.namespace.clone().unwrap_or_else(default_ns);
        let mut sql = String::from(
            r#"SELECT symbol, "interval", ts, open, high, low, close, volume, turnover, closed, source, namespace, ingested_at FROM bars WHERE 1=1"#,
        );
        sql.push_str(&format!(" AND namespace = '{}'", esc(&ns)));
        if let Some(s) = &opts.symbol {
            sql.push_str(&format!(" AND symbol = '{}'", esc(s)));
        }
        if let Some(i) = &opts.interval {
            sql.push_str(&format!(r#" AND "interval" = '{}'"#, esc(i)));
        }
        if let Some(f) = opts.from {
            sql.push_str(&format!(" AND ts >= {f}"));
        }
        if let Some(t) = opts.to {
            sql.push_str(&format!(" AND ts <= {t}"));
        }
        sql.push_str(" ORDER BY ts ASC");
        if let Some(l) = opts.limit {
            sql.push_str(&format!(" LIMIT {l}"));
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            Ok(Bar {
                symbol: r.get(0)?,
                interval: r.get(1)?,
                ts: r.get(2)?,
                open: r.get(3)?,
                high: r.get(4)?,
                low: r.get(5)?,
                close: r.get(6)?,
                volume: r.get(7).unwrap_or(0.0),
                turnover: r.get(8).unwrap_or(0.0),
                closed: r.get(9).unwrap_or(true),
                source: r.get(10).unwrap_or_else(|_| "unknown".into()),
                namespace: r.get(11).unwrap_or_else(|_| DEFAULT_NAMESPACE.into()),
                ingested_at: r.get(12).unwrap_or(0),
            })
        })?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Timestamps present in `[from, to]` — the input to gap detection and the
    /// coverage matrix.
    pub fn timestamps(
        &self,
        symbol: &str,
        interval: &str,
        from: i64,
        to: i64,
        namespace: Option<&str>,
    ) -> AppResult<Vec<i64>> {
        let ns = namespace.unwrap_or(DEFAULT_NAMESPACE);
        let sql = format!(
            "SELECT ts FROM bars WHERE symbol = '{}' AND \"interval\" = '{}' AND namespace = '{}' AND ts >= {} AND ts <= {} ORDER BY ts ASC",
            esc(symbol),
            esc(interval),
            esc(ns),
            from,
            to
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn range_of(
        &self,
        symbol: &str,
        interval: &str,
        namespace: Option<&str>,
    ) -> AppResult<RangeInfo> {
        let ns = namespace.unwrap_or(DEFAULT_NAMESPACE);
        let sql = format!(
            "SELECT min(ts), max(ts), count(*) FROM bars WHERE symbol = '{}' AND \"interval\" = '{}' AND namespace = '{}'",
            esc(symbol),
            esc(interval),
            esc(ns)
        );
        let r = self.conn.query_row(&sql, [], |r| {
            Ok(RangeInfo {
                min_ts: r.get(0).ok(),
                max_ts: r.get(1).ok(),
                count: r.get::<_, i64>(2).unwrap_or(0),
            })
        });
        match r {
            Ok(info) => Ok(info),
            Err(duckdb::Error::QueryReturnedNoRows) => Ok(RangeInfo {
                min_ts: None,
                max_ts: None,
                count: 0,
            }),
            Err(e) => Err(AppError::DuckDb(e.to_string())),
        }
    }

    /// Apply an exchange correction to one bar. This is the whole point of
    /// storing bars in DuckDB rather than immutable Parquet: a late revision is
    /// a single statement, not a rewrite of a file plus an override table.
    pub fn correct_bar(
        &self,
        symbol: &str,
        interval: &str,
        ts: i64,
        patch: &BarCorrection,
        namespace: Option<&str>,
    ) -> AppResult<bool> {
        let ns = namespace.unwrap_or(DEFAULT_NAMESPACE);
        let mut sets: Vec<String> = Vec::new();
        if let Some(v) = patch.open {
            sets.push(format!("open = {v}"));
        }
        if let Some(v) = patch.high {
            sets.push(format!("high = {v}"));
        }
        if let Some(v) = patch.low {
            sets.push(format!("low = {v}"));
        }
        if let Some(v) = patch.close {
            sets.push(format!("close = {v}"));
        }
        if let Some(v) = patch.volume {
            sets.push(format!("volume = {v}"));
        }
        if let Some(v) = patch.turnover {
            sets.push(format!("turnover = {v}"));
        }
        if sets.is_empty() {
            return Ok(false);
        }
        let sql = format!(
            "UPDATE bars SET {} WHERE symbol = '{}' AND \"interval\" = '{}' AND namespace = '{}' AND ts = {}",
            sets.join(", "),
            esc(symbol),
            esc(interval),
            esc(ns),
            ts
        );
        let changed = self.conn.execute(&sql, [])?;
        Ok(changed > 0)
    }

    /// Delete a range **within one namespace**. Not scoping this by namespace
    /// was a real bug in the TypeScript version: promoting imported data to
    /// bronze would have deleted the bronze rows just written.
    pub fn delete_range(
        &self,
        namespace: &str,
        symbol: &str,
        interval: &str,
        from: i64,
        to: i64,
    ) -> AppResult<usize> {
        let sql = format!(
            "DELETE FROM bars WHERE namespace = '{}' AND symbol = '{}' AND \"interval\" = '{}' AND ts >= {} AND ts <= {}",
            esc(namespace),
            esc(symbol),
            esc(interval),
            from,
            to
        );
        Ok(self.conn.execute(&sql, [])?)
    }

    pub fn count_bars(&self, symbol: Option<&str>, interval: Option<&str>) -> AppResult<i64> {
        let mut sql = String::from("SELECT count(*) FROM bars WHERE 1=1");
        if let Some(s) = symbol {
            sql.push_str(&format!(" AND symbol = '{}'", esc(s)));
        }
        if let Some(i) = interval {
            sql.push_str(&format!(" AND \"interval\" = '{}'", esc(i)));
        }
        Ok(self.conn.query_row(&sql, [], |r| r.get::<_, i64>(0))?)
    }

    pub fn distinct_symbols(&self, interval: Option<&str>) -> AppResult<Vec<String>> {
        let mut sql = String::from("SELECT DISTINCT symbol FROM bars WHERE 1=1");
        if let Some(i) = interval {
            sql.push_str(&format!(" AND \"interval\" = '{}'", esc(i)));
        }
        sql.push_str(" ORDER BY symbol");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// The only place user SQL is executed: guard → cap → run → flatten.
    ///
    /// The cap is deliberately `max_rows + 1` so we can tell "exactly max_rows"
    /// from "there was more" without a second COUNT query.
    pub fn run_read_only(&self, raw_sql: &str, max_rows: u64) -> AppResult<ReadOnlyResult> {
        let started = std::time::Instant::now();
        let elapsed = || started.elapsed().as_millis() as u64;

        let verdict = guard_sql(raw_sql, max_rows + 1);
        let sql = match verdict {
            GuardVerdict::Ok { sql, .. } => sql,
            GuardVerdict::Reject { code, reason } => {
                return Ok(ReadOnlyResult {
                    status: "blocked".into(),
                    columns: vec![],
                    rows: vec![],
                    truncated: false,
                    duration_ms: elapsed(),
                    reason: Some(reason),
                    code: Some(code.to_string()),
                    message: None,
                });
            }
        };

        let mut stmt = match self.conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                return Ok(ReadOnlyResult {
                    status: "error".into(),
                    columns: vec![],
                    rows: vec![],
                    truncated: false,
                    duration_ms: elapsed(),
                    reason: None,
                    code: None,
                    message: Some(e.to_string()),
                });
            }
        };

        // Both `column_names()` and `column_count()` read the arrow schema off
        // the *executed* result. Before execution they panic, and after the row
        // iterator is dropped the crate has already cleared the result — so the
        // only safe window is *while* we are still iterating. `Row: AsRef<
        // Statement>` hands us the live statement at exactly that moment.
        let mut columns: Option<Vec<String>> = None;
        let mut ncol: usize = 0;
        let mut raw_rows: Vec<Vec<serde_json::Value>> = Vec::new();
        let outcome = (|| -> AppResult<()> {
            let mapped = stmt.query_map([], |r| {
                if columns.is_none() {
                    let s = r.as_ref();
                    ncol = s.column_count();
                    columns = Some(s.column_names());
                }
                let mut vals = Vec::with_capacity(ncol);
                for i in 0..ncol {
                    let v: duckdb::types::Value = r.get(i)?;
                    vals.push(value_to_json(v));
                }
                Ok(vals)
            })?;
            for row in mapped {
                raw_rows.push(row?);
            }
            Ok(())
        })();

        if let Err(e) = outcome {
            return Ok(ReadOnlyResult {
                status: "error".into(),
                columns: vec![],
                rows: vec![],
                truncated: false,
                duration_ms: elapsed(),
                reason: None,
                code: None,
                message: Some(e.to_string()),
            });
        }

        // An empty result never yields a row, so the names were never captured.
        // `DESCRIBE` gives us the output shape without running the query twice
        // against the data; if even that fails we degrade to positional names
        // rather than dropping a perfectly good empty result set.
        let columns: Vec<String> = match columns {
            Some(c) => c,
            None => describe_columns(&self.conn, &sql)
                .unwrap_or_else(|_| (0..ncol).map(|i| format!("col{i}")).collect()),
        };
        let mut rows: Vec<serde_json::Value> = raw_rows
            .into_iter()
            .map(|vals| {
                let mut obj = serde_json::Map::with_capacity(columns.len());
                for (i, v) in vals.into_iter().enumerate() {
                    let name = columns
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("col{i}"));
                    obj.insert(name, v);
                }
                serde_json::Value::Object(obj)
            })
            .collect();

        let truncated = rows.len() as u64 > max_rows;
        if truncated {
            rows.truncate(max_rows as usize);
        }

        Ok(ReadOnlyResult {
            status: "ok".into(),
            columns,
            rows,
            truncated,
            duration_ms: elapsed(),
            reason: None,
            code: None,
            message: None,
        })
    }

    /// Schema browser backing the SQL editor.
    pub fn table_names(&self) -> AppResult<Vec<String>> {
        let sql = "SELECT table_name FROM information_schema.tables WHERE table_schema = 'main' ORDER BY table_name";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn describe(&self, table: &str) -> AppResult<Vec<ColumnMeta>> {
        let sql = format!(
            "SELECT column_name, data_type, is_nullable FROM information_schema.columns WHERE table_name = '{}' ORDER BY ordinal_position",
            esc(table)
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            Ok(ColumnMeta {
                column: r.get(0)?,
                r#type: r.get(1)?,
                nullable: r.get::<_, String>(2).unwrap_or_else(|_| "NO".into()) == "YES",
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMeta {
    pub column: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub nullable: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BarCorrection {
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub close: Option<f64>,
    pub volume: Option<f64>,
    pub turnover: Option<f64>,
}
