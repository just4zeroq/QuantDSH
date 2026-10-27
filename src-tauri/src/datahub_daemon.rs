//! Launch the market-data service and surface its endpoint to the webview.
//!
//! The service used to be a Rust sidecar binary; it is now a FastAPI app run by
//! a **bundled CPython**, so the user never needs Python installed. Everything
//! else stays the same on purpose: the service is still the single writer for
//! the DuckDB store, it still publishes `datahub.json`, and the carrier still
//! injects `{baseUrl, token}` into every webview before page scripts run.
//!
//! Resolution order for the interpreter:
//!   1. `QUANTDSH_PYTHON` (+ `QUANTDSH_DATAHUB_DIR`) — developer override
//!   2. `<resource>/runtime/python/python.exe` + `<resource>/datahub-api`
//!   3. debug builds: `services/datahub-api/.venv` inside the repo
//!
//! A missing interpreter degrades the data workbench to its error state; it must
//! never whitescreen the app.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tauri::Manager;

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
/// elsewhere — mirrors the service's own `default_data_dir`.
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

fn python_exe(root: &std::path::Path) -> PathBuf {
    if cfg!(windows) {
        root.join("python.exe")
    } else {
        root.join("bin").join("python3")
    }
}

/// Directory that directly contains the `app` package, or `None`.
///
/// Tauri's `resources` map copies a directory either into the target path or
/// under it (target/`app`/...) depending on how the mapping is read, so probe
/// both shapes instead of hard-coding one and shipping an installer whose
/// service cannot be found.
fn backend_dir(root: &std::path::Path) -> Option<PathBuf> {
    let direct = root.join("app");
    if direct.join("__main__.py").is_file() {
        return Some(root.to_path_buf());
    }
    // <root>/app/app/__main__.py — the mapping landed one level deeper.
    if direct.join("app").join("__main__.py").is_file() {
        return Some(direct);
    }
    None
}

/// `(interpreter, backend directory)` — the backend directory must contain the
/// `app` package, because the service is started as `python -m app`.
fn resolve_interpreter(app: &tauri::AppHandle) -> Option<(PathBuf, PathBuf)> {
    if let Ok(python) = std::env::var("QUANTDSH_PYTHON") {
        let python = PathBuf::from(python);
        if python.exists() {
            let dir = std::env::var("QUANTDSH_DATAHUB_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| python.parent().unwrap_or(&python).to_path_buf());
            return Some((python, dir));
        }
        eprintln!("[datahub] QUANTDSH_PYTHON points at a missing file: {}", python.display());
    }

    if let Ok(resources) = app.path().resource_dir() {
        let runtime = python_exe(&resources.join("runtime").join("python"));
        if runtime.exists() {
            if let Some(backend) = backend_dir(&resources.join("datahub-api")) {
                return Some((runtime, backend));
            }
        }
    }

    if cfg!(debug_assertions) {
        // `CARGO_MANIFEST_DIR` is `<repo>/src-tauri`, so the service lives one
        // level up. Debug-only: in a bundle this path does not exist and we
        // must not accidentally run an interpreter from a developer machine.
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let backend = repo.parent()?.join("services").join("datahub-api");
        let candidate = python_exe(&backend.join(".venv").join(if cfg!(windows) { "Scripts" } else { "bin" }));
        if candidate.exists() && backend.join("app").is_dir() {
            return Some((candidate, backend));
        }
    }

    None
}

/// Start the service if it is not already running, wait for its port file, and
/// remember the endpoint. Idempotent: a second call is a no-op.
pub fn spawn_datahub(app: &tauri::AppHandle) -> Option<DatahubEndpoint> {
    if let Some(existing) = ACTIVE.get() {
        return existing.as_ref().map(|a| a.endpoint.clone());
    }

    let Some((python, backend)) = resolve_interpreter(app) else {
        eprintln!("[datahub] no Python runtime found (bundle runtime or QUANTDSH_PYTHON); data service unavailable");
        let _ = ACTIVE.set(None);
        return None;
    };

    let data_dir = default_data_dir();
    let portfile = data_dir.join("datahub.json");
    if let Err(error) = std::fs::create_dir_all(&data_dir) {
        eprintln!("[datahub] cannot create {}: {error}", data_dir.display());
    }

    // Keep the service's stderr on disk: a bundled interpreter that fails to
    // import (missing wheel, wrong arch) is otherwise invisible in a release
    // build, where there is no console to read.
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir.join("datahub.log"))
        .ok();
    let stderr = match log.as_ref() {
        Some(file) => Stdio::from(file.try_clone().unwrap_or_else(|_| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(std::env::temp_dir().join("quantdsh-datahub.log"))
                .expect("log file")
        })),
        None => Stdio::null(),
    };

    let started = SystemTime::now();
    let child = std::process::Command::new(&python)
        .arg("-s") // ignore the user's site-packages: use the bundled wheels
        .arg("-B") // never write .pyc next to the bundled sources
        .arg("-m")
        .arg("app")
        .arg("--data-dir")
        .arg(&data_dir)
        .current_dir(&backend)
        .env("PYTHONUTF8", "1")
        .env("PYTHONUNBUFFERED", "1")
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .ok();
    let Some(mut child) = child else {
        eprintln!("[datahub] failed to spawn {} (cwd {})", python.display(), backend.display());
        let _ = ACTIVE.set(None);
        return None;
    };
    let pid = child.id();
    track_owned_process(pid, started);

    // Cold start imports fastapi + ccxt + duckdb, which is several seconds on a
    // machine where Defender scans the freshly extracted files.
    let deadline = started + Duration::from_secs(45);
    let endpoint = loop {
        if let Ok(text) = std::fs::read_to_string(&portfile) {
            if let Ok(file) = serde_json::from_str::<EndpointFile>(&text) {
                break Some(DatahubEndpoint {
                    port: file.port,
                    token: file.token,
                });
            }
        }
        // A crashed interpreter would otherwise burn the whole timeout.
        if let Ok(Some(status)) = child.try_wait() {
            eprintln!("[datahub] service exited early with {status}");
            break None;
        }
        if SystemTime::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(150));
    };

    match &endpoint {
        Some(ep) => {
            println!("[datahub] service ready on 127.0.0.1:{}", ep.port);
            let _ = ACTIVE.set(Some(Active {
                endpoint: ep.clone(),
                pid,
            }));
        }
        None => {
            eprintln!("[datahub] service did not publish a port file in time");
            let _ = child.kill();
            let _ = ACTIVE.set(None);
        }
    }
    endpoint
}

/// JavaScript injected into every webview before page scripts run, so the
/// datahub HTTP client (`globalThis.__QUANTDSH_DATAHUB__`) finds the live
/// endpoint without any environment plumbing. Returns an empty string when the
/// service is not up, letting the client fall back to the default port.
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

/// Stop the service tree on carrier exit.
pub fn stop_datahub() {
    if let Some(Some(active)) = ACTIVE.get() {
        let pid = active.pid;
        tauri::async_runtime::spawn(async move {
            if crate::owned_process::stop_owned_process(pid).await.is_err() {
                eprintln!("[datahub] service stop not confirmed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique, empty scratch directory. Nothing here is on the personal-file
    /// path: everything lives under the OS temp dir and is removed by the guard.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "quantdsh-daemon-test-{}-{tag}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }

        fn write(&self, relative: &str) {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
            std::fs::write(path, b"").expect("file");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn backend_dir_finds_the_mapped_layout() {
        let scratch = Scratch::new("flat");
        scratch.write("app/__main__.py");
        assert_eq!(backend_dir(&scratch.0), Some(scratch.0.clone()));
    }

    #[test]
    fn backend_dir_finds_the_nested_layout() {
        // Tauri's resource mapping may land the directory one level deeper;
        // both shapes have to start the service, otherwise the installer ships
        // an app whose data workbench never connects.
        let scratch = Scratch::new("nested");
        scratch.write("app/app/__main__.py");
        assert_eq!(backend_dir(&scratch.0), Some(scratch.0.join("app")));
    }

    #[test]
    fn backend_dir_rejects_a_tree_without_the_package() {
        let scratch = Scratch::new("empty");
        scratch.write("app/README.md");
        assert_eq!(backend_dir(&scratch.0), None);
    }
}
