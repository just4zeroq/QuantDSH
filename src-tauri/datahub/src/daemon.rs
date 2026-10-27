//! Daemon lifecycle: port discovery, single instance, graceful shutdown.
//!
//! The desktop app spawns this binary as a sidecar and needs to find it. It
//! could parse stdout, but a **port file** is more robust — it survives the
//! parent being restarted, and it lets a second launch reuse the running
//! instance instead of fighting over the DuckDB file.
//!
//! Reuse matters: two processes writing the same `.duckdb` is exactly the
//! cross-process single-writer situation DuckDB refuses to serialise.

use std::net::TcpListener;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

pub const DEFAULT_PORT: u16 = 17821;
/// How many ports above the preferred one we are willing to try.
const PORT_SCAN: u16 = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub started_at: i64,
    pub data_dir: String,
}

pub enum Acquired {
    /// We started a new daemon and own the port file.
    Fresh(Endpoint),
    /// One was already healthy; reuse it rather than opening the DB twice.
    Existing(Endpoint),
}

pub fn portfile_path(data_dir: &Path) -> PathBuf {
    data_dir.join("datahub.json")
}

pub fn default_data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("QUANTDSH_DATA_DIR") {
        return PathBuf::from(d);
    }
    let base = dirs_fallback();
    base.join("QuantDSH").join("data")
}

/// `%LOCALAPPDATA%` on Windows, `$HOME/.local/share` elsewhere. Mirrors what
/// the desktop carrier uses so the CLI and the app agree on one location.
fn dirs_fallback() -> PathBuf {
    if let Ok(p) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("HOME") {
        return PathBuf::from(p).join(".local").join("share");
    }
    PathBuf::from(".")
}

pub fn generate_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn read_endpoint(path: &Path) -> Option<Endpoint> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Endpoint>(&text).ok()
}

/// The recorded pid is informational only. Aliveness is decided by the HTTP
/// probe, which is both portable and strictly more reliable: a pid can be
/// recycled, but only a real daemon answers `/health` with our port file.
fn port_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Take over or reuse. When an existing daemon answers `/health` we return
/// `Existing` and the caller should exit rather than open the database.
pub async fn acquire(data_dir: &Path, preferred_port: u16, token: Option<String>) -> AppResult<Acquired> {
    std::fs::create_dir_all(data_dir)
        .map_err(|e| AppError::Internal(format!("data dir {}: {e}", data_dir.display())))?;

    let pf = portfile_path(data_dir);
    if let Some(ep) = read_endpoint(&pf) {
        if crate::api::probe(ep.port).await {
            return Ok(Acquired::Existing(ep));
        }
        // Stale: nothing answers on that port. Drop the file and start fresh.
        let _ = std::fs::remove_file(&pf);
    }

    let mut port = preferred_port;
    for _ in 0..PORT_SCAN {
        if port_free(port) {
            break;
        }
        port += 1;
    }
    if !port_free(port) {
        return Err(AppError::Internal(format!(
            "no free port in {preferred_port}..{}",
            preferred_port + PORT_SCAN
        )));
    }

    let ep = Endpoint {
        port,
        token: token.unwrap_or_else(generate_token),
        pid: std::process::id(),
        started_at: crate::market::now_ms(),
        data_dir: data_dir.display().to_string(),
    };
    write_endpoint(&pf, &ep)?;
    Ok(Acquired::Fresh(ep))
}

pub fn write_endpoint(path: &Path, ep: &Endpoint) -> AppResult<()> {
    let text = serde_json::to_string_pretty(ep)
        .map_err(|e| AppError::Internal(format!("portfile encode: {e}")))?;
    std::fs::write(path, text)
        .map_err(|e| AppError::Internal(format!("portfile write {}: {e}", path.display())))?;
    Ok(())
}

/// Best-effort cleanup. If it fails the next launch detects a stale file via
/// the pid + health probe, so a failure here is not fatal.
pub fn release(data_dir: &Path) {
    let _ = std::fs::remove_file(portfile_path(data_dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert!(a.len() >= 32, "token must be unguessable");
    }

    #[test]
    fn endpoint_round_trips_through_the_port_file() {
        let dir = std::env::temp_dir().join(format!("dsh-portfile-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pf = portfile_path(&dir);
        let ep = Endpoint {
            port: 12345,
            token: "t".into(),
            pid: std::process::id(),
            started_at: 1,
            data_dir: dir.display().to_string(),
        };
        write_endpoint(&pf, &ep).unwrap();
        let back = read_endpoint(&pf).expect("readable");
        assert_eq!(back.port, 12345);
        assert_eq!(back.token, "t");
        release(&dir);
        assert!(read_endpoint(&pf).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_free_port_is_reported_as_free() {
        // Port 0 lets the OS pick; a real bind must succeed.
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        assert!(port_free(port));
    }
}
