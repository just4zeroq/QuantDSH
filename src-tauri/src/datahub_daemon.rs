//! Launch the `quantdsh-datahub` sidecar and surface its endpoint to the webview.
//!
//! The datahub is the **single writer** for the DuckDB market store, so exactly
//! one process may own it. The carrier starts it once at launch, reads the port
//! file it writes, and injects that endpoint into every webview so the in-browser
//! data workbench can talk to it. On quit the owned-process tracker (a
//! kill-on-close Job Object on Windows) tears the whole tree down.
//!
//! Why a port file and not stdout: the file survives a parent restart and lets a
//! second launch reuse the running instance instead of fighting over the
//! `.duckdb` file — DuckDB refuses two concurrent writers, so reuse is mandatory.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::owned_process::track_owned_process;

#[derive(Clone, Debug)]
pub struct DatahubEndpoint {
    pub port: u16,
    pub token: String,
}

#[derive(Deserialize, Serialize)]
struct EndpointFile {
    port: u16,
    token: String,
    #[allow(dead_code)]
    pid: u32,
    #[allow(dead_code)]
    started_at: i64,
    #[allow(dead_code)]
    data_dir: String,
}

struct Active {
    endpoint: DatahubEndpoint,
    pid: u32,
}

static ACTIVE: OnceLock<Option<Active>> = OnceLock::new();

/// `%LOCALAPPDATA%/QuantDSH/data` on Windows, `$HOME/.local/share/QuantDSH/data`
/// elsewhere — mirrors the daemon's own `default_data_dir` so the two agree.
pub fn default_data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("QUANTDSH_DATA_DIR") {
        return PathBuf::from(d);
    }
    let base = if let Ok(p) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(p)
    } else if let Ok(p) = std::env::var("HOME") {
        PathBuf::from(p).join(".local").join("share")
    } else {
        PathBuf::from(".")
    };
    base.join("QuantDSH").join("data")
}

/// Where the bundled sidecar lives. Tauri strips the `<name>-<triple>` suffix at
/// runtime, so the running file is plain `quantdsh-datahub[.exe]` next to the
/// carrier executable. `QUANTDSH_DATAHUB_BIN` overrides for local development.
fn binary_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("QUANTDSH_DATAHUB_BIN") {
        return Some(PathBuf::from(p));
    }
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let ext = if cfg!(windows) { ".exe" } else { "" };
    let plain = exe_dir.join(format!("quantdsh-datahub{ext}"));
    if plain.exists() {
        return Some(plain);
    }
    // Be forgiving: some packaging keeps the target-triple suffix.
    let candidates = if cfg!(windows) {
        vec!["quantdsh-datahub-x86_64-pc-windows-msvc.exe", "quantdsh-datahub-aarch64-pc-windows-msvc.exe"]
    } else if cfg!(target_os = "macos") {
        vec!["quantdsh-datahub-x86_64-apple-darwin", "quantdsh-datahub-aarch64-apple-darwin"]
    } else {
        vec!["quantdsh-datahub-x86_64-unknown-linux-gnu", "quantdsh-datahub-aarch64-unknown-linux-gnu"]
    };
    for name in candidates {
        let p = exe_dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Start the daemon if it is not already running, wait for its port file, and
/// remember the endpoint. Idempotent: a second call is a no-op. Returns `None`
/// only when the binary is missing or the port file never appears — the webview
/// then falls back to the default port (and shows an error state), so a missing
/// data service never whitescreens the app.
pub fn spawn_datahub() -> Option<DatahubEndpoint> {
    // Already started in this process.
    if let Some(existing) = ACTIVE.get() {
        return existing.as_ref().map(|a| a.endpoint.clone());
    }

    let bin = match binary_path() {
        Some(b) => b,
        None => {
            eprintln!("[datahub] sidecar binary not found; data service unavailable");
            let _ = ACTIVE.set(None);
            return None;
        }
    };

    let data_dir = default_data_dir();
    let portfile = data_dir.join("datahub.json");

    let started = SystemTime::now();
    let child = std::process::Command::new(&bin)
        .arg("--data-dir")
        .arg(&data_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok();
    let Some(mut child) = child else {
        eprintln!("[datahub] failed to spawn sidecar; data service unavailable");
        let _ = ACTIVE.set(None);
        return None;
    };
    let pid = child.id();
    track_owned_process(pid, started);

    // The daemon writes the port file as soon as it has acquired a port. Poll
    // briefly; the daemon itself handles a stale file (probes /health, rewrites).
    let deadline = started + Duration::from_secs(10);
    let endpoint = loop {
        if let Ok(text) = std::fs::read_to_string(&portfile) {
            if let Ok(file) = serde_json::from_str::<EndpointFile>(&text) {
                break Some(DatahubEndpoint {
                    port: file.port,
                    token: file.token,
                });
            }
        }
        if SystemTime::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(150));
    };

    match &endpoint {
        Some(ep) => {
            println!("[datahub] sidecar ready on 127.0.0.1:{}", ep.port);
            let _ = ACTIVE.set(Some(Active {
                endpoint: ep.clone(),
                pid,
            }));
        }
        None => {
            eprintln!("[datahub] sidecar did not publish a port file in time");
            let _ = child.kill();
            let _ = ACTIVE.set(None);
        }
    }
    endpoint
}

/// JavaScript injected into every webview before page scripts run, so the
/// datahub HTTP client (`globalThis.__QUANTDSH_DATAHUB__`) finds the live
/// endpoint without any environment plumbing. Returns an empty string when the
/// daemon is not up, letting the client fall back to the default port.
pub fn datahub_init_script() -> String {
    match ACTIVE.get().and_then(|a| a.as_ref()) {
        Some(active) => {
            let base_url = format!("http://127.0.0.1:{}", active.endpoint.port);
            let token_json =
                serde_json::to_string(&active.endpoint.token).unwrap_or_else(|_| "\"\"".into());
            format!(
                "window.__QUANTDSH_DATAHUB__ = {{baseUrl:\"{base_url}\", token:{token_json}}};",
                base_url = base_url,
                token_json = token_json
            )
        }
        None => String::new(),
    }
}

/// Stop the sidecar tree on carrier exit.
pub fn stop_datahub() {
    if let Some(Some(active)) = ACTIVE.get() {
        let pid = active.pid;
        tauri::async_runtime::spawn(async move {
            if crate::owned_process::stop_owned_process(pid).await.is_err() {
                eprintln!("[datahub] sidecar stop not confirmed");
            }
        });
    }
}
