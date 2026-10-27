//! HTTP surface of the daemon.
//!
//! Two deliberate choices:
//!   * **Bound to 127.0.0.1 only.** No LAN exposure, ever.
//!   * **Token on everything except `/health`.** A bound-to-localhost port is
//!     still reachable by any process and by any page the user visits; the
//!     token is what stops a random local tab from driving the daemon. The
//!     token is generated at start and written to the port file, which the
//!     launcher hands to the frontend.

use std::sync::Arc;

use axum::extract::{Path, Query, Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};

use crate::csv_import::{CsvImportRequest, PromoteRequest};
use crate::error::{AppError, AppResult};
use crate::market::{Bar, BarCorrection, QueryOptions};
use crate::metadata::{DatasetRecord, JobPatch, SyncJob};
use crate::quality::QualityOptions;
use crate::service::{DataHub, TableMeta};
use crate::sync::RunOptions;

pub struct AppState {
    pub hub: Arc<DataHub>,
    pub token: String,
}

// ─────────────────────────────── auth ───────────────────────────────

async fn require_token(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| constant_time_eq(t.as_bytes(), state.token.as_bytes()))
        .unwrap_or(false);

    if ok {
        next.run(req).await
    } else {
        AppError::Unauthorized.into_response()
    }
}

/// Length-independent comparison. Not cryptographically critical for a
/// localhost token, but comparing byte-by-byte with an early exit leaks length
/// and it costs nothing to do it properly.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ─────────────────────────────── helpers ───────────────────────────────

/// Run a blocking storage call off the async executor.
async fn blocking<T, F>(f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Internal(format!("task panicked: {e}")))?
}

// ─────────────────────────────── handlers ───────────────────────────────

async fn health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "startedAt": state.hub.started_at,
        "dataDir": state.hub.data_dir.display().to_string(),
    }))
}

async fn list_sources() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "sources": crate::service::source_names() }))
}

async fn list_markets(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "markets": state.hub.markets() }))
}

async fn schema(State(state): State<Arc<AppState>>) -> AppResult<Json<Vec<TableMeta>>> {
    let hub = state.hub.clone();
    blocking(move || hub.schema()).await.map(Json)
}

#[derive(Debug, Deserialize)]
pub struct DatasetQuery {
    pub market: Option<String>,
    pub namespace: Option<String>,
}

async fn list_datasets(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DatasetQuery>,
) -> AppResult<Json<Vec<DatasetRecord>>> {
    let hub = state.hub.clone();
    blocking(move || hub.list_datasets(q.market.as_deref(), q.namespace.as_deref()))
        .await
        .map(Json)
}

async fn upsert_dataset(
    State(state): State<Arc<AppState>>,
    Json(rec): Json<DatasetRecord>,
) -> AppResult<Json<serde_json::Value>> {
    let hub = state.hub.clone();
    blocking(move || hub.upsert_dataset(&rec)).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct KeyBody {
    pub dataset_key: String,
}

async fn refresh_dataset(
    State(state): State<Arc<AppState>>,
    Json(body): Json<KeyBody>,
) -> AppResult<Json<serde_json::Value>> {
    let hub = state.hub.clone();
    blocking(move || hub.refresh_dataset_stats(&body.dataset_key)).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct BarsQuery {
    pub symbol: Option<String>,
    pub interval: Option<String>,
    pub namespace: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub limit: Option<u64>,
}

async fn get_bars(
    State(state): State<Arc<AppState>>,
    Query(q): Query<BarsQuery>,
) -> AppResult<Json<Vec<Bar>>> {
    let opts = QueryOptions {
        symbol: q.symbol,
        interval: q.interval,
        namespace: q.namespace,
        from: q.from,
        to: q.to,
        limit: q.limit,
    };
    let hub = state.hub.clone();
    blocking(move || hub.bars(opts)).await.map(Json)
}

#[derive(Debug, Deserialize)]
pub struct TimestampsQuery {
    pub symbol: String,
    pub interval: String,
    pub namespace: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
}

async fn get_timestamps(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TimestampsQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let hub = state.hub.clone();
    blocking(move || {
        let ts = hub.timestamps(
            &q.symbol,
            &q.interval,
            q.from.unwrap_or(i64::MIN),
            q.to.unwrap_or(i64::MAX),
            q.namespace.as_deref(),
        )?;
        Ok(serde_json::json!({ "timestamps": ts }))
    })
    .await
    .map(Json)
}

#[derive(Debug, Deserialize)]
pub struct CorrectBody {
    pub symbol: String,
    pub interval: String,
    pub ts: i64,
    pub namespace: Option<String>,
    #[serde(flatten)]
    pub patch: BarCorrection,
}

async fn correct_bar(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CorrectBody>,
) -> AppResult<Json<serde_json::Value>> {
    let hub = state.hub.clone();
    blocking(move || {
        hub.correct_bar(
            &body.symbol,
            &body.interval,
            body.ts,
            &body.patch,
            body.namespace.as_deref(),
        )
    })
    .await
    .map(|changed| Json(serde_json::json!({ "changed": changed })))
}

#[derive(Debug, Deserialize)]
pub struct DeleteQuery {
    pub namespace: Option<String>,
    pub symbol: String,
    pub interval: String,
    pub from: i64,
    pub to: i64,
}

async fn delete_bars(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DeleteQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let ns = q.namespace.unwrap_or_else(|| "bronze".into());
    let hub = state.hub.clone();
    blocking(move || hub.delete_range(&ns, &q.symbol, &q.interval, q.from, q.to))
        .await
        .map(|n| Json(serde_json::json!({ "deleted": n })))
}

#[derive(Debug, Deserialize)]
pub struct CoverageQuery {
    pub dataset_key: String,
}

async fn get_coverage(
    State(state): State<Arc<AppState>>,
    Query(q): Query<CoverageQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let hub = state.hub.clone();
    blocking(move || {
        let cells = hub.coverage(&q.dataset_key)?;
        Ok(serde_json::json!({ "datasetKey": q.dataset_key, "cells": cells }))
    })
    .await
    .map(Json)
}

async fn get_coverage_summary(
    State(state): State<Arc<AppState>>,
    Query(q): Query<CoverageQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let hub = state.hub.clone();
    blocking(move || hub.coverage_summary(&q.dataset_key))
        .await
        .map(|s| Json(serde_json::json!({
            "present": s.present, "expected": s.expected, "ratio": s.ratio
        })))
}

#[derive(Debug, Deserialize)]
pub struct CoverageRefreshBody {
    pub dataset_key: String,
    pub year: i32,
    pub month: u32,
}

async fn refresh_coverage(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CoverageRefreshBody>,
) -> AppResult<Json<serde_json::Value>> {
    let hub = state.hub.clone();
    blocking(move || hub.refresh_coverage(&body.dataset_key, body.year, body.month))
        .await
        .map(|cells| Json(serde_json::json!({ "cells": cells })))
}

#[derive(Debug, Deserialize)]
pub struct JobsQuery {
    pub enabled_only: Option<bool>,
    pub dataset_key: Option<String>,
}

async fn list_jobs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<JobsQuery>,
) -> AppResult<Json<Vec<SyncJob>>> {
    let hub = state.hub.clone();
    blocking(move || hub.list_jobs(q.enabled_only.unwrap_or(false), q.dataset_key.as_deref()))
        .await
        .map(Json)
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    Json(job): Json<SyncJob>,
) -> AppResult<Json<serde_json::Value>> {
    let id = job.id.clone();
    let hub = state.hub.clone();
    blocking(move || hub.create_job(&job)).await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(patch): Json<JobPatch>,
) -> AppResult<Json<serde_json::Value>> {
    let hub = state.hub.clone();
    blocking(move || hub.update_job(&id, &patch)).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let hub = state.hub.clone();
    blocking(move || hub.delete_job(&id)).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct RunBody {
    pub dry_run: Option<bool>,
    pub now: Option<i64>,
    pub limit: Option<u64>,
}

async fn run_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<RunBody>>,
) -> AppResult<Json<crate::sync::SyncOutcome>> {
    let opts = match body {
        Some(Json(b)) => RunOptions {
            dry_run: b.dry_run.unwrap_or(false),
            now: b.now,
            limit: b.limit,
        },
        None => RunOptions::default(),
    };
    state.hub.run_sync(&id, opts).await.map(Json)
}

#[derive(Debug, Deserialize)]
pub struct DueQuery {
    pub now: Option<i64>,
}

async fn due_jobs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DueQuery>,
) -> AppResult<Json<Vec<SyncJob>>> {
    let now = q.now.unwrap_or_else(crate::market::now_ms);
    let hub = state.hub.clone();
    blocking(move || hub.list_due(now)).await.map(Json)
}

async fn run_quality(
    State(state): State<Arc<AppState>>,
    Json(opts): Json<QualityOptions>,
) -> AppResult<Json<crate::quality::QualityReport>> {
    let hub = state.hub.clone();
    blocking(move || hub.quality(&opts)).await.map(Json)
}

async fn latest_quality(
    State(state): State<Arc<AppState>>,
    Query(q): Query<CoverageQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let hub = state.hub.clone();
    blocking(move || hub.latest_quality(&q.dataset_key))
        .await
        .map(|r| Json(serde_json::json!({ "run": r })))
}

async fn import_csv(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CsvImportRequest>,
) -> AppResult<Json<crate::csv_import::CsvImportResult>> {
    let hub = state.hub.clone();
    blocking(move || hub.import_csv(&req)).await.map(Json)
}

async fn promote(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PromoteRequest>,
) -> AppResult<Json<crate::csv_import::PromoteResult>> {
    let hub = state.hub.clone();
    blocking(move || hub.promote(&req)).await.map(Json)
}

#[derive(Debug, Deserialize)]
pub struct ImportsQuery {
    pub dataset_key: Option<String>,
}

async fn list_imports(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ImportsQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let hub = state.hub.clone();
    blocking(move || hub.list_imports(q.dataset_key.as_deref()))
        .await
        .map(|v| Json(serde_json::json!({ "imports": v })))
}

#[derive(Debug, Deserialize)]
pub struct SqlBody {
    pub sql: String,
    pub max_rows: Option<u64>,
}

async fn run_sql(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SqlBody>,
) -> AppResult<Json<crate::market::ReadOnlyResult>> {
    let max_rows = body.max_rows.unwrap_or(crate::service::DEFAULT_MAX_ROWS);
    let hub = state.hub.clone();
    blocking(move || hub.sql_query(&body.sql, max_rows))
        .await
        .map(Json)
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<i64>,
}

async fn query_history(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HistoryQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let hub = state.hub.clone();
    blocking(move || hub.query_history(q.limit.unwrap_or(50)))
        .await
        .map(|v| Json(serde_json::json!({ "history": v })))
}

// ─────────────────────────────── router ───────────────────────────────

pub fn build_router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/sources", get(list_sources))
        .route("/markets", get(list_markets))
        .route("/schema", get(schema))
        .route("/datasets", get(list_datasets).post(upsert_dataset))
        .route("/datasets/refresh", post(refresh_dataset))
        .route("/bars", get(get_bars).delete(delete_bars))
        .route("/bars/timestamps", get(get_timestamps))
        .route("/bars/correct", post(correct_bar))
        .route("/coverage", get(get_coverage))
        .route("/coverage/summary", get(get_coverage_summary))
        .route("/coverage/refresh", post(refresh_coverage))
        .route("/jobs", get(list_jobs).post(create_job))
        .route("/jobs/due", get(due_jobs))
        // axum 0.8 dropped the `:param` syntax — captures are `{param}` now.
        // Keeping the old spelling panics at router build time, i.e. at the
        // first request rather than at startup.
        .route("/jobs/{id}", post(update_job).delete(delete_job))
        .route("/jobs/{id}/run", post(run_job))
        .route("/quality", post(run_quality))
        .route("/quality/latest", get(latest_quality))
        .route("/import/csv", post(import_csv))
        .route("/import/promote", post(promote))
        .route("/imports", get(list_imports))
        .route("/query", post(run_sql))
        .route("/query/history", get(query_history))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_token,
        ));

    Router::new()
        // /health is unauthenticated on purpose: the launcher probes it to
        // discover whether an instance is already alive.
        .route("/health", get(health))
        .nest("/api", api)
        .with_state(state)
        .layer(CorsLayer::new().allow_origin(Any).allow_headers(Any))
}

/// A tiny readiness probe used by the launcher after spawning the daemon.
pub async fn probe(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/health");
    match reqwest::Client::new().get(url).send().await {
        Ok(r) => r.status().is_success(),
        Err(_) => false,
    }
}

#[derive(Debug, Serialize)]
pub struct DaemonInfo {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub started_at: i64,
}
