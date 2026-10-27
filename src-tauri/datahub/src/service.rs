//! The service facade — the only thing the HTTP layer talks to.
//!
//! Two storage engines, one lock each:
//!   * `market` — DuckDB, the bars
//!   * `meta`   — SQLite, everything about the bars
//!
//! Every method is **blocking**. The async HTTP handlers wrap them in
//! `spawn_blocking`, and the async parts (talking to an exchange) happen
//! outside the locks entirely. Holding a storage lock across an `await` is the
//! one mistake that would turn a slow venue into a frozen daemon.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::csv_import::{self, CsvImportRequest, CsvImportResult, PromoteRequest, PromoteResult};
use crate::domain::{CoverageCell, Market};
use crate::error::{AppError, AppResult};
use crate::market::{
    Bar, BarCorrection, ColumnMeta, MarketStore, QueryOptions, ReadOnlyResult, DEFAULT_NAMESPACE,
};
use crate::metadata::{
    DatasetFilter, DatasetRecord, JobFilter, JobPatch, MetadataStore, QualityRun, SyncJob,
};
use crate::quality::{check_quality, QualityOptions, QualityReport};
use crate::sources::{self, DataSource};
use crate::sync::{self, RunOptions, SyncOutcome};

pub const DEFAULT_MAX_ROWS: u64 = 10_000;

pub struct DataHub {
    market: Mutex<MarketStore>,
    meta: Mutex<MetadataStore>,
    pub data_dir: PathBuf,
    pub started_at: i64,
}

impl DataHub {
    pub fn open(data_dir: &Path) -> AppResult<Self> {
        std::fs::create_dir_all(data_dir).map_err(|e| {
            AppError::Internal(format!("cannot create data dir {}: {e}", data_dir.display()))
        })?;
        let market = MarketStore::open(&data_dir.join("market.duckdb").to_string_lossy())?;
        let meta = MetadataStore::open(&data_dir.join("meta.sqlite").to_string_lossy())?;
        Ok(Self {
            market: Mutex::new(market),
            meta: Mutex::new(meta),
            data_dir: data_dir.to_path_buf(),
            started_at: crate::market::now_ms(),
        })
    }

    /// In-memory twin used by tests — same behaviour, no files.
    pub fn open_ephemeral() -> AppResult<Self> {
        Ok(Self {
            market: Mutex::new(MarketStore::open_in_memory()?),
            meta: Mutex::new(MetadataStore::open_in_memory()?),
            data_dir: PathBuf::from(":memory:"),
            started_at: crate::market::now_ms(),
        })
    }

    fn with_market<T>(&self, f: impl FnOnce(&MarketStore) -> AppResult<T>) -> AppResult<T> {
        // A poisoned lock means a previous write panicked; the database file is
        // still consistent (DuckDB/SQLite are crash-safe) so we keep serving.
        let guard = self.market.lock().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }

    fn with_meta<T>(&self, f: impl FnOnce(&MetadataStore) -> AppResult<T>) -> AppResult<T> {
        let guard = self.meta.lock().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }

    // ── datasets ──

    pub fn list_datasets(
        &self,
        market: Option<&str>,
        namespace: Option<&str>,
    ) -> AppResult<Vec<DatasetRecord>> {
        self.with_meta(|m| m.list_datasets(DatasetFilter { market, namespace }))
    }

    pub fn upsert_dataset(&self, rec: &DatasetRecord) -> AppResult<()> {
        self.with_meta(|m| m.upsert_dataset(rec))
    }

    /// Recompute counters from the actual bars. The catalog must not keep
    /// serving the numbers written at creation time.
    pub fn refresh_dataset_stats(&self, key: &str) -> AppResult<()> {
        let rec = self.with_meta(|m| m.get_dataset(key))?;
        let Some(rec) = rec else {
            return Err(AppError::NotFound(format!("dataset/{key}")));
        };
        let range = self.with_market(|mk| {
            mk.range_of(&rec.symbol, &rec.interval, Some(&rec.namespace))
        })?;
        self.with_meta(|m| {
            m.set_dataset_stats(key, Some(range.count), Some(range.max_ts), Some(crate::market::now_ms()))
        })
    }

    // ── bars ──

    pub fn bars(&self, opts: QueryOptions) -> AppResult<Vec<Bar>> {
        self.with_market(|mk| mk.query_bars(&opts))
    }

    /// Just the timestamps in a window.
    ///
    /// Gaps are detected on the client from this list, and the whole point of a
    /// dedicated route is that "which bars are missing" cannot be answered from
    /// a row count — while shipping full bars to answer it would move megabytes
    /// through a localhost socket for a handful of numbers.
    pub fn timestamps(
        &self,
        symbol: &str,
        interval: &str,
        from: i64,
        to: i64,
        namespace: Option<&str>,
    ) -> AppResult<Vec<i64>> {
        self.with_market(|mk| mk.timestamps(symbol, interval, from, to, namespace))
    }

    /// Apply an exchange correction — the reason bars live in DuckDB rather
    /// than an immutable file format.
    pub fn correct_bar(
        &self,
        symbol: &str,
        interval: &str,
        ts: i64,
        patch: &BarCorrection,
        namespace: Option<&str>,
    ) -> AppResult<bool> {
        self.with_market(|mk| mk.correct_bar(symbol, interval, ts, patch, namespace))
    }

    pub fn delete_range(
        &self,
        namespace: &str,
        symbol: &str,
        interval: &str,
        from: i64,
        to: i64,
    ) -> AppResult<usize> {
        self.with_market(|mk| mk.delete_range(namespace, symbol, interval, from, to))
    }

    // ── coverage ──

    pub fn coverage(&self, key: &str) -> AppResult<Vec<CoverageCell>> {
        self.with_meta(|m| m.get_coverage(key))
    }

    pub fn coverage_summary(&self, key: &str) -> AppResult<crate::metadata::CoverageSummary> {
        self.with_meta(|m| m.coverage_summary(key))
    }

    /// Recompute a month of coverage from the bars actually present.
    pub fn refresh_coverage(
        &self,
        key: &str,
        year: i32,
        month: u32,
    ) -> AppResult<Vec<CoverageCell>> {
        let rec = self.with_meta(|m| m.get_dataset(key))?;
        let Some(rec) = rec else {
            return Err(AppError::NotFound(format!("dataset/{key}")));
        };
        let step = crate::domain::interval_ms(&rec.interval)?;
        let from = chrono::NaiveDate::from_ymd_opt(year, month, 1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc().timestamp_millis())
            .ok_or_else(|| AppError::BadRequest(format!("coverage/bad-month {year}-{month}")))?;
        let to = {
            let (ny, nm) = if month == 12 {
                (year + 1, 1)
            } else {
                (year, month + 1)
            };
            chrono::NaiveDate::from_ymd_opt(ny, nm, 1)
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc().timestamp_millis() - 1)
                .unwrap_or(from + 31 * 86_400_000)
        };

        let ts = self.with_market(|mk| {
            mk.timestamps(&rec.symbol, &rec.interval, from, to, Some(&rec.namespace))
        })?;
        let cells = crate::domain::coverage_matrix(year, month, step, &ts)?;
        self.with_meta(|m| m.replace_coverage(key, &cells))?;
        Ok(cells)
    }

    // ── sync jobs ──

    pub fn list_jobs(&self, enabled_only: bool, dataset_key: Option<&str>) -> AppResult<Vec<SyncJob>> {
        self.with_meta(|m| m.list_jobs(JobFilter { enabled_only, dataset_key }))
    }

    pub fn create_job(&self, job: &SyncJob) -> AppResult<()> {
        self.with_meta(|m| m.create_job(job))
    }

    pub fn update_job(&self, id: &str, patch: &JobPatch) -> AppResult<()> {
        self.with_meta(|m| m.update_job(id, patch))
    }

    pub fn delete_job(&self, id: &str) -> AppResult<()> {
        self.with_meta(|m| m.delete_job(id))
    }

    pub fn list_due(&self, now: i64) -> AppResult<Vec<SyncJob>> {
        let jobs = self.with_meta(|m| m.list_jobs(JobFilter { enabled_only: true, dataset_key: None }))?;
        Ok(sync::due_jobs(&jobs, now).into_iter().cloned().collect())
    }

    /// Fetch from the venue first (async, no lock), then write and advance the
    /// cursor (blocking, under lock).
    pub async fn run_sync(&self, job_id: &str, opts: RunOptions) -> AppResult<SyncOutcome> {
        let job = self.with_meta(|m| m.get_job(job_id))?;
        let Some(job) = job else {
            return Err(AppError::NotFound(format!("job/{job_id}")));
        };
        let Some(source) = sources::resolve(&job.source) else {
            return Err(AppError::BadRequest(format!("source/unknown: {}", job.source)));
        };

        let now = opts.now.unwrap_or_else(crate::market::now_ms);
        let (from, to) = sync::plan_window(&job, now)?;
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

        let bars = sync::fetch_all(&source, &job, from, to, opts.limit.unwrap_or(1000)).await?;
        self.with_market(|_| Ok(()))?; // touch once to surface a poisoned lock early
        let outcome = {
            let market = self.market.lock().unwrap_or_else(|e| e.into_inner());
            let meta = self.meta.lock().unwrap_or_else(|e| e.into_inner());
            sync::commit(&market, &meta, &job, &bars, now, opts.dry_run)?
        };
        Ok(outcome)
    }

    // ── quality ──

    pub fn quality(&self, opts: &QualityOptions) -> AppResult<QualityReport> {
        let report = self.with_market(|mk| check_quality(mk, opts))?;
        self.with_meta(|m| {
            m.record_quality_run(&QualityRun {
                id: uuid::Uuid::new_v4().to_string(),
                dataset_key: format!("{}:{}", opts.symbol, opts.interval),
                ran_at: opts.now.unwrap_or_else(crate::market::now_ms),
                passed: report.passed,
                findings: report.findings.clone(),
            })
        })?;
        Ok(report)
    }

    pub fn latest_quality(&self, dataset_key: &str) -> AppResult<Option<QualityRun>> {
        self.with_meta(|m| m.latest_quality_run(dataset_key))
    }

    // ── import ──

    pub fn import_csv(&self, req: &CsvImportRequest) -> AppResult<CsvImportResult> {
        let market = self.market.lock().unwrap_or_else(|e| e.into_inner());
        let meta = self.meta.lock().unwrap_or_else(|e| e.into_inner());
        csv_import::import_csv(&market, &meta, req)
    }

    pub fn promote(&self, req: &PromoteRequest) -> AppResult<PromoteResult> {
        let market = self.market.lock().unwrap_or_else(|e| e.into_inner());
        let meta = self.meta.lock().unwrap_or_else(|e| e.into_inner());
        csv_import::promote_to_bronze(&market, &meta, req)
    }

    pub fn list_imports(&self, dataset_key: Option<&str>) -> AppResult<Vec<crate::metadata::ImportBatch>> {
        self.with_meta(|m| m.list_imports(dataset_key))
    }

    // ── read-only SQL ──

    /// The single exit point for user SQL: guard → execute → record.
    /// Blocked statements are recorded too, so attempts stay auditable.
    pub fn sql_query(&self, sql: &str, max_rows: u64) -> AppResult<ReadOnlyResult> {
        let started = crate::market::now_ms();
        let res = self.with_market(|mk| mk.run_read_only(sql, max_rows))?;
        let duration = (crate::market::now_ms() - started).max(0) as u64;

        self.with_meta(|m| {
            m.record_query(&crate::metadata::QueryHistoryEntry {
                id: uuid::Uuid::new_v4().to_string(),
                sql: sql.to_string(),
                ran_at: started,
                status: res.status.clone(),
                row_count: res.rows.len() as i64,
                duration_ms: duration as i64,
                error: res
                    .message
                    .clone()
                    .or_else(|| res.reason.clone()),
            })
        })?;
        Ok(res)
    }

    pub fn schema(&self) -> AppResult<Vec<TableMeta>> {
        self.with_market(|mk| {
            let names = mk.table_names()?;
            let mut out = Vec::new();
            for t in names {
                out.push(TableMeta {
                    table: t.clone(),
                    columns: mk.describe(&t)?,
                });
            }
            Ok(out)
        })
    }

    pub fn query_history(&self, limit: i64) -> AppResult<Vec<crate::metadata::QueryHistoryEntry>> {
        self.with_meta(|m| m.list_query_history(limit))
    }

    // ── misc ──

    pub fn markets(&self) -> Vec<&'static str> {
        Market::ALL.iter().map(|m| m.as_str()).collect()
    }

    pub fn table_row_counts(&self) -> AppResult<serde_json::Value> {
        let n = self.with_market(|mk| mk.count_bars(None, None))?;
        Ok(serde_json::json!({ "bars": n, "namespace": DEFAULT_NAMESPACE }))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TableMeta {
    pub table: String,
    pub columns: Vec<ColumnMeta>,
}

/// Convenience for callers that just want the default namespace name.
pub fn default_namespace() -> &'static str {
    DEFAULT_NAMESPACE
}

/// Resolve a source name without borrowing a live source (used by /api/sources).
pub fn source_names() -> Vec<&'static str> {
    sources::SOURCE_NAMES.to_vec()
}

pub fn resolve_source(name: &str) -> Option<DataSource> {
    sources::resolve(name)
}
