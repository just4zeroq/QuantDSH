//! End-to-end tests for the data hub.
//!
//! These exercise the paths that actually matter to the product, not the
//! individual helpers: a sync produces bars, a re-run does not duplicate, a
//! failed sync leaves the cursor alone so it can retry, imported data cannot
//! reach `bronze` until it passes the gate, and user SQL cannot write.

use std::sync::Arc;

use quantdsh_datahub::api::{self, AppState};
use quantdsh_datahub::csv_import::{CsvImportRequest, PromoteRequest};
use quantdsh_datahub::domain::Market;
use quantdsh_datahub::market::QueryOptions;
use quantdsh_datahub::metadata::{DatasetRecord, SyncJob};
use quantdsh_datahub::quality::QualityOptions;
use quantdsh_datahub::service::DataHub;
use quantdsh_datahub::sync::RunOptions;

/// 2026-01-01T00:00Z, aligned to every interval we support.
const T0: i64 = 1_767_225_600_000;

fn dataset() -> DatasetRecord {
    DatasetRecord {
        dataset_key: quantdsh_datahub::domain::make_dataset_key(
            Market::Crypto,
            "binance",
            "BTCUSDT",
            "1m",
            None,
        ),
        market: Market::Crypto.as_str().into(),
        exchange: "binance".into(),
        symbol: "BTCUSDT".into(),
        interval: "1m".into(),
        source: "replay".into(),
        namespace: "bronze".into(),
        as_of: None,
        ingested_at: None,
        closed_only: true,
        row_count: 0,
        created_at: T0,
        updated_at: T0,
    }
}

fn job(id: &str) -> SyncJob {
    SyncJob {
        id: id.into(),
        dataset_key: dataset().dataset_key,
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
        created_at: T0,
    }
}

fn opts(now: i64) -> RunOptions {
    RunOptions {
        dry_run: false,
        now: Some(now),
        limit: Some(500),
    }
}

fn bronze(hub: &DataHub, limit: u64) -> Vec<quantdsh_datahub::market::Bar> {
    hub.bars(QueryOptions {
        namespace: Some("bronze".into()),
        limit: Some(limit),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn sync_writes_bars_and_records_the_cursor() {
    let hub = DataHub::open_ephemeral().unwrap();
    hub.upsert_dataset(&dataset()).unwrap();
    hub.create_job(&job("j1")).unwrap();

    let out = hub.run_sync("j1", opts(T0)).await.unwrap();
    assert_eq!(out.status, "ok");
    assert_eq!(out.rows_written, 500);
    assert!(out.cursor.is_some());

    let bars = bronze(&hub, 10_000);
    assert_eq!(bars.len(), 500);
    assert_eq!(bars[0].ts, bars[0].ts / 60_000 * 60_000, "grid aligned");
    assert!(bars.windows(2).all(|w| w[0].ts < w[1].ts), "strictly ascending");

    // The catalog must reflect reality, not the zeros it was created with.
    hub.refresh_dataset_stats(&dataset().dataset_key).unwrap();
    let rec = hub.list_datasets(None, None).unwrap();
    assert_eq!(rec[0].row_count, 500);
}

#[tokio::test]
async fn rerunning_the_same_window_is_idempotent() {
    let hub = DataHub::open_ephemeral().unwrap();
    hub.upsert_dataset(&dataset()).unwrap();
    hub.create_job(&job("j1")).unwrap();

    hub.run_sync("j1", opts(T0)).await.unwrap();
    let after_first = bronze(&hub, 100_000).len();

    // A second job with no cursor asks for the *identical* window. Reusing the
    // first job would be circular: its cursor has moved, so the runner would
    // just report "idle" and the upsert would never be exercised. What has to
    // absorb a replay is `INSERT OR REPLACE`, not the cursor guard.
    hub.create_job(&job("j2")).unwrap();
    let replay = hub.run_sync("j2", opts(T0)).await.unwrap();
    assert_eq!(replay.rows_written, after_first as i64, "the window is re-read in full");
    let after_second = bronze(&hub, 100_000).len();

    assert_eq!(after_first, after_second, "a replayed window must not duplicate");
}

#[tokio::test]
async fn incremental_sync_continues_from_the_cursor() {
    let hub = DataHub::open_ephemeral().unwrap();
    hub.upsert_dataset(&dataset()).unwrap();
    hub.create_job(&job("j1")).unwrap();

    let first = hub.run_sync("j1", opts(T0)).await.unwrap();
    let c1 = first.cursor.unwrap();

    // Move the clock two hours forward: the job should pick up new bars only.
    let second = hub.run_sync("j1", opts(T0 + 2 * 3_600_000)).await.unwrap();
    let c2 = second.cursor.unwrap();
    assert!(c2 > c1, "cursor must advance");

    let all = bronze(&hub, 100_000);
    assert_eq!(all.len(), 1000, "500 + 500 distinct bars");
    assert_eq!(all.last().unwrap().ts, c2);
}

#[tokio::test]
async fn a_failed_sync_leaves_the_cursor_untouched() {
    let hub = DataHub::open_ephemeral().unwrap();
    hub.upsert_dataset(&dataset()).unwrap();
    let mut j = job("j1");
    j.cursor = Some(T0);
    j.source = "does-not-exist".into();
    hub.create_job(&j).unwrap();

    assert!(hub.run_sync("j1", opts(T0)).await.is_err(), "unknown source must fail loudly");

    let stored = hub.list_jobs(false, None).unwrap();
    assert_eq!(stored[0].cursor, Some(T0), "cursor must survive a failure");
}

#[tokio::test]
async fn coverage_and_quality_reflect_what_is_present() {
    let hub = DataHub::open_ephemeral().unwrap();
    hub.upsert_dataset(&dataset()).unwrap();
    hub.create_job(&job("j1")).unwrap();
    // The runner backfills 7 days ending one bar before `now`, so a clock of
    // T0 would land every bar in late December and the January grid would be
    // legitimately empty. Push the clock into January.
    hub.run_sync("j1", opts(T0 + 8 * 86_400_000)).await.unwrap();

    let cells = hub.refresh_coverage(&dataset().dataset_key, 2026, 1).unwrap();
    assert_eq!(cells.len(), 31 * 24, "a full month grid");
    let summary = hub.coverage_summary(&dataset().dataset_key).unwrap();
    assert_eq!(summary.present, 500);

    let bars = bronze(&hub, 100_000);
    let report = hub
        .quality(&QualityOptions {
            symbol: "BTCUSDT".into(),
            interval: "1m".into(),
            from_ts: bars.first().unwrap().ts,
            to_ts: bars.last().unwrap().ts,
            namespace: None,
            now: Some(bars.last().unwrap().ts + 60_000),
            max_stale_ms: None,
        })
        .unwrap();
    assert!(report.passed, "findings: {:?}", report.findings);
    assert!((report.coverage_ratio - 1.0).abs() < 1e-9);
}

#[tokio::test]
async fn imported_data_stays_out_of_bronze_until_promoted() {
    let hub = DataHub::open_ephemeral().unwrap();
    hub.upsert_dataset(&dataset()).unwrap();

    let mut text = String::from("ts,open,high,low,close,volume\n");
    for i in 0..120 {
        let ts = T0 + i * 60_000;
        text.push_str(&format!("{ts},100,101,99,100.5,10\n"));
    }

    let r = hub
        .import_csv(&CsvImportRequest {
            dataset_key: dataset().dataset_key,
            symbol: "BTCUSDT".into(),
            interval: "1m".into(),
            file: "manual.csv".into(),
            text,
            namespace: None,
            source: None,
            max_bad_ratio: Some(0.0),
        })
        .unwrap();
    assert!(r.gate_passed);
    assert_eq!(r.rows_ok, 120);

    assert!(bronze(&hub, 100_000).is_empty(), "import must not land in bronze");

    let p = hub
        .promote(&PromoteRequest {
            dataset_key: dataset().dataset_key,
            symbol: "BTCUSDT".into(),
            interval: "1m".into(),
            source: None,
        })
        .unwrap();
    assert_eq!(p.moved, 120);

    assert_eq!(bronze(&hub, 100_000).len(), 120);
    let imported = hub
        .bars(QueryOptions {
            namespace: Some("imported".into()),
            limit: Some(100_000),
            ..Default::default()
        })
        .unwrap();
    assert!(imported.is_empty(), "imported cleared after promotion");
}

#[tokio::test]
async fn user_sql_cannot_write_and_history_is_recorded() {
    let hub = DataHub::open_ephemeral().unwrap();
    hub.upsert_dataset(&dataset()).unwrap();
    hub.create_job(&job("j1")).unwrap();
    hub.run_sync("j1", opts(T0)).await.unwrap();

    let ok = hub.sql_query("SELECT count(*) AS n FROM bars", 100).unwrap();
    assert_eq!(ok.status, "ok");
    assert_eq!(ok.rows.len(), 1);

    for bad in [
        "DROP TABLE bars",
        "DELETE FROM bars",
        "UPDATE bars SET close = 1",
        "SELECT * FROM bars; DROP TABLE bars",
    ] {
        let r = hub.sql_query(bad, 100).unwrap();
        assert_eq!(r.status, "blocked", "{bad} must be blocked");
    }

    let capped = hub.sql_query("SELECT * FROM bars", 10).unwrap();
    assert_eq!(capped.status, "ok");
    assert_eq!(capped.rows.len(), 10);
    assert!(capped.truncated, "must signal there was more");

    let history = hub.query_history(50).unwrap();
    assert!(history.iter().any(|h| h.status == "blocked"));
    assert!(history.iter().any(|h| h.status == "ok"));
}

// ─────────────────────────────── HTTP layer ───────────────────────────────

async fn spawn_server(hub: Arc<DataHub>, token: &str) -> u16 {
    let state = Arc::new(AppState {
        hub,
        token: token.to_string(),
    });
    let app = api::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    port
}

#[tokio::test]
async fn health_is_open_but_the_api_requires_the_token() {
    let hub = Arc::new(DataHub::open_ephemeral().unwrap());
    let port = spawn_server(hub, "secret-token").await;
    let base = format!("http://127.0.0.1:{port}");
    let c = reqwest::Client::new();

    assert!(c.get(format!("{base}/health")).send().await.unwrap().status().is_success());

    let no_token = c.get(format!("{base}/api/datasets")).send().await.unwrap();
    assert_eq!(no_token.status(), 401, "unauthenticated must be refused");

    let wrong = c
        .get(format!("{base}/api/datasets"))
        .header("Authorization", "Bearer nope")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    let good = c
        .get(format!("{base}/api/datasets"))
        .header("Authorization", "Bearer secret-token")
        .send()
        .await
        .unwrap();
    assert!(good.status().is_success());
}

#[tokio::test]
async fn the_full_http_round_trip_works() {
    let hub = Arc::new(DataHub::open_ephemeral().unwrap());
    let port = spawn_server(hub, "tok").await;
    let base = format!("http://127.0.0.1:{port}");
    let c = reqwest::Client::new();
    let auth = |r: reqwest::RequestBuilder| r.header("Authorization", "Bearer tok");

    auth(c.post(format!("{base}/api/datasets")))
        .json(&dataset())
        .send()
        .await
        .unwrap();

    auth(c.post(format!("{base}/api/jobs")))
        .json(&job("http-job"))
        .send()
        .await
        .unwrap();

    let out: serde_json::Value = auth(c.post(format!("{base}/api/jobs/http-job/run")))
        .json(&serde_json::json!({ "now": T0, "limit": 300 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Wire format is snake_case end to end — the Rust structs carry no
    // `rename_all`, so the browser client must not expect camelCase.
    assert_eq!(out["rows_written"], 300);

    let bars: serde_json::Value = auth(
        c.get(format!("{base}/api/bars"))
            .query(&[("symbol", "BTCUSDT"), ("interval", "1m"), ("limit", "5")]),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(bars.as_array().unwrap().len(), 5);

    // Gap detection needs the timestamps, not the whole bars — the point of the
    // dedicated route is that a row count cannot tell you *which* bars are gone.
    let ts: serde_json::Value = auth(
        c.get(format!("{base}/api/bars/timestamps"))
            .query(&[("symbol", "BTCUSDT"), ("interval", "1m")]),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(ts["timestamps"].as_array().unwrap().len(), 300);

    let schema: serde_json::Value = auth(c.get(format!("{base}/api/schema")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(schema.as_array().unwrap().iter().any(|t| t["table"] == "bars"));

    let q: serde_json::Value = auth(c.post(format!("{base}/api/query")))
        .json(&serde_json::json!({ "sql": "SELECT count(*) AS n FROM bars" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(q["status"], "ok");

    let blocked: serde_json::Value = auth(c.post(format!("{base}/api/query")))
        .json(&serde_json::json!({ "sql": "DROP TABLE bars" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(blocked["status"], "blocked");
}
