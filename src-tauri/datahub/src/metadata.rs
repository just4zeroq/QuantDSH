//! SQLite metadata store — the catalog around the bars.
//!
//! DuckDB holds the bars; everything *about* the bars lives here: the dataset
//! directory, sync jobs and their cursors, coverage, quality runs, import
//! batches and query history. SQLite (not DuckDB) because this is small,
//! row-oriented, write-heavy bookkeeping — exactly SQLite's shape.
//!
//! Schema version lives in `user_version`, the SQLite convention, so no extra
//! bookkeeping table is needed.

use crate::domain::CoverageCell;
use crate::error::AppResult;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

pub const CURRENT_VERSION: i64 = 1;

const BASE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS datasets (
  dataset_key  TEXT PRIMARY KEY,
  market       TEXT NOT NULL,
  exchange     TEXT NOT NULL,
  symbol       TEXT NOT NULL,
  interval     TEXT NOT NULL,
  source       TEXT NOT NULL,
  namespace    TEXT NOT NULL DEFAULT 'bronze',
  as_of        INTEGER,
  ingested_at  INTEGER,
  closed_only  INTEGER NOT NULL DEFAULT 1,
  row_count    INTEGER NOT NULL DEFAULT 0,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_datasets_market ON datasets(market);
CREATE INDEX IF NOT EXISTS idx_datasets_ns ON datasets(namespace);

CREATE TABLE IF NOT EXISTS sync_jobs (
  id           TEXT PRIMARY KEY,
  dataset_key  TEXT NOT NULL,
  source       TEXT NOT NULL,
  symbols      TEXT NOT NULL,
  interval     TEXT NOT NULL,
  cron         TEXT,
  mode         TEXT NOT NULL DEFAULT 'incremental',
  cursor       INTEGER,
  status       TEXT NOT NULL DEFAULT 'idle',
  last_run_at  INTEGER,
  last_error   TEXT,
  rows_written INTEGER NOT NULL DEFAULT 0,
  enabled      INTEGER NOT NULL DEFAULT 1,
  created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_jobs_enabled ON sync_jobs(enabled);
CREATE INDEX IF NOT EXISTS idx_jobs_dataset ON sync_jobs(dataset_key);

CREATE TABLE IF NOT EXISTS coverage (
  dataset_key TEXT NOT NULL,
  day         TEXT NOT NULL,
  hour        INTEGER NOT NULL,
  present     INTEGER NOT NULL,
  expected    INTEGER NOT NULL,
  PRIMARY KEY (dataset_key, day, hour)
);
CREATE INDEX IF NOT EXISTS idx_cov_day ON coverage(dataset_key, day);

CREATE TABLE IF NOT EXISTS quality_runs (
  id          TEXT PRIMARY KEY,
  dataset_key TEXT NOT NULL,
  ran_at      INTEGER NOT NULL,
  passed      INTEGER NOT NULL,
  findings    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_qr_dataset ON quality_runs(dataset_key, ran_at DESC);

CREATE TABLE IF NOT EXISTS imports (
  id          TEXT PRIMARY KEY,
  dataset_key TEXT NOT NULL,
  namespace   TEXT NOT NULL DEFAULT 'imported',
  file        TEXT NOT NULL,
  rows_total  INTEGER NOT NULL DEFAULT 0,
  rows_ok     INTEGER NOT NULL DEFAULT 0,
  rows_bad    INTEGER NOT NULL DEFAULT 0,
  gate_passed INTEGER NOT NULL DEFAULT 0,
  gate_report TEXT,
  created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_imp_dataset ON imports(dataset_key, created_at DESC);

CREATE TABLE IF NOT EXISTS query_history (
  id          TEXT PRIMARY KEY,
  sql         TEXT NOT NULL,
  ran_at      INTEGER NOT NULL,
  status      TEXT NOT NULL,
  row_count   INTEGER NOT NULL DEFAULT 0,
  duration_ms INTEGER NOT NULL DEFAULT 0,
  error       TEXT
);
CREATE INDEX IF NOT EXISTS idx_qh_ran ON query_history(ran_at DESC);
"#;

// ─────────────────────────────── Types ───────────────────────────────

fn default_ns() -> String {
    "bronze".into()
}
fn default_imported() -> String {
    "imported".into()
}
fn default_true() -> bool {
    true
}
fn default_mode() -> String {
    "incremental".into()
}
fn default_status() -> String {
    "idle".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetRecord {
    pub dataset_key: String,
    pub market: String,
    pub exchange: String,
    pub symbol: String,
    pub interval: String,
    pub source: String,
    #[serde(default = "default_ns")]
    pub namespace: String,
    #[serde(default)]
    pub as_of: Option<i64>,
    #[serde(default)]
    pub ingested_at: Option<i64>,
    #[serde(default = "default_true")]
    pub closed_only: bool,
    #[serde(default)]
    pub row_count: i64,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncJob {
    pub id: String,
    pub dataset_key: String,
    pub source: String,
    pub symbols: Vec<String>,
    pub interval: String,
    #[serde(default)]
    pub cron: Option<String>,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub cursor: Option<i64>,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub last_run_at: Option<i64>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub rows_written: i64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: i64,
}

/// Only the fields the runner owns. Anything not set here is left untouched.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobPatch {
    pub status: Option<String>,
    pub cursor: Option<i64>,
    pub last_run_at: Option<i64>,
    pub last_error: Option<String>,
    pub rows_written: Option<i64>,
    pub cron: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityFinding {
    pub kind: String,
    pub severity: String, // error | warn
    pub message: String,
    #[serde(default)]
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityRun {
    pub id: String,
    pub dataset_key: String,
    pub ran_at: i64,
    pub passed: bool,
    pub findings: Vec<QualityFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportBatch {
    pub id: String,
    pub dataset_key: String,
    #[serde(default = "default_imported")]
    pub namespace: String,
    pub file: String,
    #[serde(default)]
    pub rows_total: i64,
    #[serde(default)]
    pub rows_ok: i64,
    #[serde(default)]
    pub rows_bad: i64,
    #[serde(default)]
    pub gate_passed: bool,
    #[serde(default)]
    pub gate_report: Option<serde_json::Value>,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryHistoryEntry {
    pub id: String,
    pub sql: String,
    pub ran_at: i64,
    pub status: String,
    #[serde(default)]
    pub row_count: i64,
    #[serde(default)]
    pub duration_ms: i64,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageSummary {
    pub present: i64,
    pub expected: i64,
    pub ratio: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DatasetFilter<'a> {
    pub market: Option<&'a str>,
    pub namespace: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct JobFilter<'a> {
    pub enabled_only: bool,
    pub dataset_key: Option<&'a str>,
}

// ─────────────────────────────── Row mappers ───────────────────────────────

fn row_to_dataset(r: &Row) -> rusqlite::Result<DatasetRecord> {
    Ok(DatasetRecord {
        dataset_key: r.get(0)?,
        market: r.get(1)?,
        exchange: r.get(2)?,
        symbol: r.get(3)?,
        interval: r.get(4)?,
        source: r.get(5)?,
        namespace: r.get(6)?,
        as_of: r.get(7)?,
        ingested_at: r.get(8)?,
        closed_only: r.get::<_, i64>(9)? != 0,
        row_count: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

fn row_to_job(r: &Row) -> rusqlite::Result<SyncJob> {
    let symbols_raw: String = r.get(3)?;
    Ok(SyncJob {
        id: r.get(0)?,
        dataset_key: r.get(1)?,
        source: r.get(2)?,
        symbols: serde_json::from_str(&symbols_raw).unwrap_or_default(),
        interval: r.get(4)?,
        cron: r.get(5)?,
        mode: r.get(6)?,
        cursor: r.get(7)?,
        status: r.get(8)?,
        last_run_at: r.get(9)?,
        last_error: r.get(10)?,
        rows_written: r.get(11)?,
        enabled: r.get::<_, i64>(12)? != 0,
        created_at: r.get(13)?,
    })
}

fn row_to_import(r: &Row) -> rusqlite::Result<ImportBatch> {
    let report: Option<String> = r.get(8)?;
    Ok(ImportBatch {
        id: r.get(0)?,
        dataset_key: r.get(1)?,
        namespace: r.get(2)?,
        file: r.get(3)?,
        rows_total: r.get(4)?,
        rows_ok: r.get(5)?,
        rows_bad: r.get(6)?,
        gate_passed: r.get::<_, i64>(7)? != 0,
        gate_report: report.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: r.get(9)?,
    })
}

fn row_to_query(r: &Row) -> rusqlite::Result<QueryHistoryEntry> {
    Ok(QueryHistoryEntry {
        id: r.get(0)?,
        sql: r.get(1)?,
        ran_at: r.get(2)?,
        status: r.get(3)?,
        row_count: r.get(4)?,
        duration_ms: r.get(5)?,
        error: r.get(6)?,
    })
}

fn ratio(present: i64, expected: i64) -> f64 {
    if expected == 0 {
        0.0
    } else {
        present as f64 / expected as f64
    }
}

// ─────────────────────────────── Store ───────────────────────────────

pub struct MetadataStore {
    conn: Connection,
}

impl MetadataStore {
    pub fn open(path: &str) -> AppResult<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> AppResult<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Apply pending migrations; returns the version we landed on.
    pub fn migrate(&self) -> AppResult<i64> {
        let current: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(0);
        self.conn.execute_batch(BASE_DDL)?;
        if current < CURRENT_VERSION {
            self.conn
                .pragma_update(None, "user_version", CURRENT_VERSION)?;
        }
        Ok(CURRENT_VERSION)
    }

    // ── datasets ──

    pub fn upsert_dataset(&self, rec: &DatasetRecord) -> AppResult<()> {
        self.conn.execute(
            r#"INSERT INTO datasets
               (dataset_key, market, exchange, symbol, interval, source, namespace,
                as_of, ingested_at, closed_only, row_count, created_at, updated_at)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
               ON CONFLICT(dataset_key) DO UPDATE SET
                 market=excluded.market, exchange=excluded.exchange, symbol=excluded.symbol,
                 interval=excluded.interval, source=excluded.source, namespace=excluded.namespace,
                 closed_only=excluded.closed_only, updated_at=excluded.updated_at"#,
            params![
                rec.dataset_key,
                rec.market,
                rec.exchange,
                rec.symbol,
                rec.interval,
                rec.source,
                rec.namespace,
                rec.as_of,
                rec.ingested_at,
                if rec.closed_only { 1 } else { 0 },
                rec.row_count,
                rec.created_at,
                rec.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_dataset(&self, key: &str) -> AppResult<Option<DatasetRecord>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM datasets WHERE dataset_key = ?1",
                [key],
                row_to_dataset,
            )
            .optional()?)
    }

    pub fn list_datasets(&self, filter: DatasetFilter) -> AppResult<Vec<DatasetRecord>> {
        let mut sql = String::from("SELECT * FROM datasets WHERE 1=1");
        let mut args: Vec<String> = Vec::new();
        if let Some(m) = filter.market {
            sql.push_str(" AND market = ?");
            args.push(m.to_string());
        }
        if let Some(n) = filter.namespace {
            sql.push_str(" AND namespace = ?");
            args.push(n.to_string());
        }
        sql.push_str(" ORDER BY updated_at DESC");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), row_to_dataset)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Touch only the derived counters. The catalog must not keep serving the
    /// numbers written at creation time — the data keeps growing.
    pub fn set_dataset_stats(
        &self,
        key: &str,
        row_count: Option<i64>,
        as_of: Option<Option<i64>>,
        ingested_at: Option<i64>,
    ) -> AppResult<()> {
        let mut sets: Vec<&str> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(c) = row_count {
            sets.push("row_count = ?");
            args.push(Box::new(c));
        }
        if let Some(a) = as_of {
            sets.push("as_of = ?");
            args.push(Box::new(a));
        }
        if let Some(i) = ingested_at {
            sets.push("ingested_at = ?");
            args.push(Box::new(i));
        }
        if sets.is_empty() {
            return Ok(());
        }
        sets.push("updated_at = ?");
        args.push(Box::new(crate::market::now_ms()));
        args.push(Box::new(key.to_string()));

        let sql = format!("UPDATE datasets SET {} WHERE dataset_key = ?", sets.join(", "));
        self.conn.execute(
            &sql,
            rusqlite::params_from_iter(args.iter().map(|a| a.as_ref())),
        )?;
        Ok(())
    }

    pub fn delete_dataset(&self, key: &str) -> AppResult<()> {
        self.conn
            .execute("DELETE FROM datasets WHERE dataset_key = ?1", [key])?;
        self.conn
            .execute("DELETE FROM coverage WHERE dataset_key = ?1", [key])?;
        Ok(())
    }

    // ── sync jobs ──

    pub fn create_job(&self, job: &SyncJob) -> AppResult<()> {
        self.conn.execute(
            r#"INSERT INTO sync_jobs
               (id, dataset_key, source, symbols, interval, cron, mode, cursor, status,
                last_run_at, last_error, rows_written, enabled, created_at)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)"#,
            params![
                job.id,
                job.dataset_key,
                job.source,
                serde_json::to_string(&job.symbols)?,
                job.interval,
                job.cron,
                job.mode,
                job.cursor,
                job.status,
                job.last_run_at,
                job.last_error,
                job.rows_written,
                if job.enabled { 1 } else { 0 },
                job.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_job(&self, id: &str) -> AppResult<Option<SyncJob>> {
        Ok(self
            .conn
            .query_row("SELECT * FROM sync_jobs WHERE id = ?1", [id], row_to_job)
            .optional()?)
    }

    pub fn list_jobs(&self, filter: JobFilter) -> AppResult<Vec<SyncJob>> {
        let mut sql = String::from("SELECT * FROM sync_jobs WHERE 1=1");
        let mut args: Vec<String> = Vec::new();
        if filter.enabled_only {
            sql.push_str(" AND enabled = 1");
        }
        if let Some(k) = filter.dataset_key {
            sql.push_str(" AND dataset_key = ?");
            args.push(k.to_string());
        }
        sql.push_str(" ORDER BY created_at DESC");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), row_to_job)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Note the camelCase → snake_case mapping. Writing JS-style names straight
    /// into SQL was a real bug in the TypeScript version
    /// (`no such column: lastRunAt`).
    pub fn update_job(&self, id: &str, patch: &JobPatch) -> AppResult<()> {
        let mut sets: Vec<&str> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(v) = &patch.status {
            sets.push("status = ?");
            args.push(Box::new(v.clone()));
        }
        if let Some(v) = patch.cursor {
            sets.push("cursor = ?");
            args.push(Box::new(v));
        }
        if let Some(v) = patch.last_run_at {
            sets.push("last_run_at = ?");
            args.push(Box::new(v));
        }
        if let Some(v) = &patch.last_error {
            sets.push("last_error = ?");
            args.push(Box::new(v.clone()));
        }
        if let Some(v) = patch.rows_written {
            sets.push("rows_written = ?");
            args.push(Box::new(v));
        }
        if let Some(v) = &patch.cron {
            sets.push("cron = ?");
            args.push(Box::new(v.clone()));
        }
        if let Some(v) = patch.enabled {
            sets.push("enabled = ?");
            args.push(Box::new(if v { 1 } else { 0 }));
        }
        if sets.is_empty() {
            return Ok(());
        }
        args.push(Box::new(id.to_string()));

        let sql = format!("UPDATE sync_jobs SET {} WHERE id = ?", sets.join(", "));
        self.conn.execute(
            &sql,
            rusqlite::params_from_iter(args.iter().map(|a| a.as_ref())),
        )?;
        Ok(())
    }

    pub fn delete_job(&self, id: &str) -> AppResult<()> {
        self.conn.execute("DELETE FROM sync_jobs WHERE id = ?1", [id])?;
        Ok(())
    }

    // ── coverage ──

    pub fn replace_coverage(&self, dataset_key: &str, cells: &[CoverageCell]) -> AppResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM coverage WHERE dataset_key = ?1", [dataset_key])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO coverage (dataset_key, day, hour, present, expected) VALUES (?1,?2,?3,?4,?5)",
            )?;
            for c in cells {
                stmt.execute(params![dataset_key, c.day, c.hour, c.present, c.expected])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_coverage(&self, dataset_key: &str) -> AppResult<Vec<CoverageCell>> {
        let mut stmt = self.conn.prepare(
            "SELECT day, hour, present, expected FROM coverage WHERE dataset_key = ?1 ORDER BY day, hour",
        )?;
        let rows = stmt.query_map([dataset_key], |r| {
            let present: i64 = r.get(2)?;
            let expected: i64 = r.get(3)?;
            Ok(CoverageCell {
                day: r.get(0)?,
                hour: r.get(1)?,
                present,
                expected,
                ratio: ratio(present, expected),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn coverage_summary(&self, dataset_key: &str) -> AppResult<CoverageSummary> {
        self.conn
            .query_row(
                "SELECT COALESCE(sum(present),0), COALESCE(sum(expected),0) FROM coverage WHERE dataset_key = ?1",
                [dataset_key],
                |r| {
                    let present: i64 = r.get(0)?;
                    let expected: i64 = r.get(1)?;
                    Ok(CoverageSummary {
                        present,
                        expected,
                        ratio: ratio(present, expected),
                    })
                },
            )
            .map_err(crate::error::AppError::from)
    }

    // ── quality ──

    pub fn record_quality_run(&self, run: &QualityRun) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO quality_runs (id, dataset_key, ran_at, passed, findings) VALUES (?1,?2,?3,?4,?5)",
            params![
                run.id,
                run.dataset_key,
                run.ran_at,
                if run.passed { 1 } else { 0 },
                serde_json::to_string(&run.findings)?,
            ],
        )?;
        Ok(())
    }

    pub fn latest_quality_run(&self, dataset_key: &str) -> AppResult<Option<QualityRun>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, dataset_key, ran_at, passed, findings FROM quality_runs WHERE dataset_key = ?1 ORDER BY ran_at DESC LIMIT 1",
                [dataset_key],
                |r| {
                    let findings: String = r.get(4)?;
                    Ok(QualityRun {
                        id: r.get(0)?,
                        dataset_key: r.get(1)?,
                        ran_at: r.get(2)?,
                        passed: r.get::<_, i64>(3)? != 0,
                        findings: serde_json::from_str(&findings).unwrap_or_default(),
                    })
                },
            )
            .optional()?)
    }

    // ── imports ──

    pub fn record_import(&self, b: &ImportBatch) -> AppResult<()> {
        self.conn.execute(
            r#"INSERT INTO imports (id, dataset_key, namespace, file, rows_total, rows_ok, rows_bad, gate_passed, gate_report, created_at)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"#,
            params![
                b.id,
                b.dataset_key,
                b.namespace,
                b.file,
                b.rows_total,
                b.rows_ok,
                b.rows_bad,
                if b.gate_passed { 1 } else { 0 },
                b.gate_report.as_ref().map(serde_json::to_string).transpose()?,
                b.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn list_imports(&self, dataset_key: Option<&str>) -> AppResult<Vec<ImportBatch>> {
        const COLS: &str = "id, dataset_key, namespace, file, rows_total, rows_ok, rows_bad, gate_passed, gate_report, created_at";
        let mut out = Vec::new();
        match dataset_key {
            Some(k) => {
                let mut stmt = self.conn.prepare(&format!(
                    "SELECT {COLS} FROM imports WHERE dataset_key = ?1 ORDER BY created_at DESC"
                ))?;
                let rows = stmt.query_map([k], row_to_import)?;
                for r in rows {
                    out.push(r?);
                }
            }
            None => {
                let mut stmt = self
                    .conn
                    .prepare(&format!("SELECT {COLS} FROM imports ORDER BY created_at DESC"))?;
                let rows = stmt.query_map([], row_to_import)?;
                for r in rows {
                    out.push(r?);
                }
            }
        }
        Ok(out)
    }

    // ── query history ──

    /// Blocked statements are recorded too, so "who tried to run what" stays
    /// auditable rather than silently vanishing.
    pub fn record_query(&self, e: &QueryHistoryEntry) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO query_history (id, sql, ran_at, status, row_count, duration_ms, error) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![e.id, e.sql, e.ran_at, e.status, e.row_count, e.duration_ms, e.error],
        )?;
        Ok(())
    }

    pub fn list_query_history(&self, limit: i64) -> AppResult<Vec<QueryHistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, sql, ran_at, status, row_count, duration_ms, error FROM query_history ORDER BY ran_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], row_to_query)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str) -> SyncJob {
        SyncJob {
            id: id.into(),
            dataset_key: "crypto:binance:BTCUSDT:1m".into(),
            source: "replay".into(),
            symbols: vec!["BTCUSDT".into()],
            interval: "1m".into(),
            cron: None,
            mode: "incremental".into(),
            cursor: None,
            status: "idle".into(),
            last_run_at: None,
            last_error: None,
            rows_written: 0,
            enabled: true,
            created_at: 1,
        }
    }

    #[test]
    fn schema_migrates_to_current_version() {
        let m = MetadataStore::open_in_memory().unwrap();
        assert_eq!(m.migrate().unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn dataset_round_trips_and_filters() {
        let m = MetadataStore::open_in_memory().unwrap();
        let rec = DatasetRecord {
            dataset_key: "crypto:binance:BTCUSDT:1m".into(),
            market: "CRYPTO".into(),
            exchange: "binance".into(),
            symbol: "BTCUSDT".into(),
            interval: "1m".into(),
            source: "replay".into(),
            namespace: "bronze".into(),
            as_of: None,
            ingested_at: None,
            closed_only: true,
            row_count: 0,
            created_at: 1,
            updated_at: 1,
        };
        m.upsert_dataset(&rec).unwrap();
        let got = m.get_dataset(&rec.dataset_key).unwrap().unwrap();
        assert_eq!(got.symbol, "BTCUSDT");
        assert_eq!(m.list_datasets(DatasetFilter::default()).unwrap().len(), 1);
        // A namespace filter that matches nothing must exclude, not ignore.
        assert_eq!(
            m.list_datasets(DatasetFilter { market: None, namespace: Some("imported") })
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn stats_update_touches_only_the_counters() {
        let m = MetadataStore::open_in_memory().unwrap();
        let key = "crypto:binance:ETHUSDT:1m";
        let d = DatasetRecord {
            dataset_key: key.into(),
            market: "CRYPTO".into(),
            exchange: "binance".into(),
            symbol: "ETHUSDT".into(),
            interval: "1m".into(),
            source: "replay".into(),
            namespace: "bronze".into(),
            as_of: None,
            ingested_at: None,
            closed_only: true,
            row_count: 0,
            created_at: 1,
            updated_at: 1,
        };
        m.upsert_dataset(&d).unwrap();
        m.set_dataset_stats(key, Some(1440), Some(Some(9_000)), Some(10)).unwrap();
        let got = m.get_dataset(key).unwrap().unwrap();
        assert_eq!(got.row_count, 1440);
        assert_eq!(got.as_of, Some(9_000));
    }

    /// Regression: patch keys used to be interpolated as camelCase column names.
    #[test]
    fn job_patch_writes_snake_case_columns() {
        let m = MetadataStore::open_in_memory().unwrap();
        m.create_job(&job("j1")).unwrap();
        m.update_job(
            "j1",
            &JobPatch {
                status: Some("running".into()),
                cursor: Some(1234),
                last_run_at: Some(999),
                rows_written: Some(60),
                enabled: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        let j = m.get_job("j1").unwrap().unwrap();
        assert_eq!(j.status, "running");
        assert_eq!(j.cursor, Some(1234));
        assert_eq!(j.last_run_at, Some(999));
        assert_eq!(j.rows_written, 60);
        assert!(!j.enabled);
    }

    #[test]
    fn coverage_replaces_cleanly() {
        let m = MetadataStore::open_in_memory().unwrap();
        let cells = vec![CoverageCell {
            day: "2026-01-01".into(),
            hour: 0,
            present: 60,
            expected: 60,
            ratio: 1.0,
        }];
        m.replace_coverage("k", &cells).unwrap();
        m.replace_coverage("k", &cells).unwrap(); // must not duplicate
        let got = m.get_coverage("k").unwrap();
        assert_eq!(got.len(), 1);
        let s = m.coverage_summary("k").unwrap();
        assert_eq!(s.present, 60);
        assert!((s.ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn quality_and_imports_round_trip() {
        let m = MetadataStore::open_in_memory().unwrap();
        m.record_quality_run(&QualityRun {
            id: "q1".into(),
            dataset_key: "k".into(),
            ran_at: 5,
            passed: false,
            findings: vec![QualityFinding {
                kind: "gap".into(),
                severity: "error".into(),
                message: "missing".into(),
                count: 3,
            }],
        })
        .unwrap();
        let q = m.latest_quality_run("k").unwrap().unwrap();
        assert!(!q.passed);
        assert_eq!(q.findings[0].count, 3);

        m.record_import(&ImportBatch {
            id: "i1".into(),
            dataset_key: "k".into(),
            namespace: "imported".into(),
            file: "a.csv".into(),
            rows_total: 10,
            rows_ok: 9,
            rows_bad: 1,
            gate_passed: false,
            gate_report: None,
            created_at: 1,
        })
        .unwrap();
        assert_eq!(m.list_imports(Some("k")).unwrap().len(), 1);
        assert_eq!(m.list_imports(None).unwrap().len(), 1);
    }

    #[test]
    fn blocked_queries_are_still_recorded() {
        let m = MetadataStore::open_in_memory().unwrap();
        m.record_query(&QueryHistoryEntry {
            id: "x".into(),
            sql: "DROP TABLE bars".into(),
            ran_at: 1,
            status: "blocked".into(),
            row_count: 0,
            duration_ms: 0,
            error: Some("禁止使用关键字：DROP".into()),
        })
        .unwrap();
        let h = m.list_query_history(10).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].status, "blocked");
    }
}
