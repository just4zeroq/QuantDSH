//! `quantdsh-datahub` — the QuantDSH data daemon.
//!
//! One process owns both databases:
//!   * **DuckDB** (`market.duckdb`) — OHLCV bars, with real `UPDATE` support
//!   * **SQLite** (`meta.sqlite`) — catalog, sync jobs, coverage, quality, history
//!
//! Everything else (React pages, AI tools, CLI) talks to it over HTTP on
//! loopback. That single-writer shape is what keeps DuckDB happy: its
//! cross-process single-writer limit never bites because there is exactly one
//! process, and its in-process MVCC handles concurrent readers for free.

pub mod api;
pub mod csv_import;
pub mod daemon;
pub mod domain;
pub mod error;
pub mod market;
pub mod metadata;
pub mod quality;
pub mod service;
pub mod sources;
pub mod sql_guard;
pub mod sync;

pub use error::{AppError, AppResult};
pub use service::DataHub;

/// DuckDB engine this build is pinned to. Exposed so `/health` and the desktop
/// carrier can assert the sidecar matches what the release was built against.
pub const DUCKDB_ENGINE: &str = "1.5.6";
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
