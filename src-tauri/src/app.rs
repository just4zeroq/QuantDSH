//! Main-process state machine, mirroring the upstream desktop main process.
//!
//! Every Electron behavior is ported one-to-one: the carrier surfaces (entry, enterprise
//! login, loading) are served through a registered `quantdsh://` protocol, the official
//! Host lifecycle (Profile materialization, port reservation, readiness detection,
//! owned-process cleanup) is reproduced exactly, and the enterprise admission, menu,
//! quit choreography and window rules follow the original control flow.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde_json::{json, Map, Value};
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::utils::config::Color;
use tauri::{
    AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;

use crate::browser_worker::{browser_worker_request, start_browser_worker};
use crate::command_management::manage_command;
use crate::connection::{enterprise_connection, DesktopConnection};
use crate::deployment_config::parse_deployment_config;
use crate::enterprise_auth::{start_enterprise_authority, Authority, EnterpriseLogin};
use crate::enterprise_plugins::{compose_installed_enterprise_plugins, installed_enterprise_plugins};
use crate::local_runtime::{
    desktop_enterprise_patch, enterprise_environment, enterprise_space, mark_profile_updated,
    materialize_runtime_profile, official_host_launcher, official_launcher,
};
use crate::official_command::{run_official_command, OfficialCommandOptions};
use crate::owned_process::{
    clean_owned_process_tree, note_child_exit, stop_owned_process, track_owned_process,
};
use crate::pages::{
    carrier_init_script, connection_entry_html, connection_loading_html, enterprise_login_html,
    runtime_missing_html,
};
use crate::paths::{RuntimePaths, PROFILE_NAME};
use crate::runtime_diagnostics::{
    redact_runtime_output, runtime_failure_log, RuntimeOutputRedactor,
};
use crate::runtime_port::{
    prepare_runtime_port, runtime_port_conflict, RuntimePort, RuntimePortOptions,
};

const READY_PATTERN: &str = r"dsh web:\s+(http://127\.0\.0\.1:\d+/?\?token=[^\s]+)";
const READY_TIMEOUT: Duration = Duration::from_secs(90);
const VERIFY_INTERVAL: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: usize = 16_384;
/// wry reports no load failure, so a bounded watchdog is the only way to reach
/// the `page.loadURL(...).catch(...)` rollback branch. The official Host has
/// already printed its ready URL by then, so a loopback surface that has not
/// finished loading in this window is a dead one.
const WINDOW_LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// Electron's `viewMenu` zoom steps: 1.2× per press, clamped to Chromium's range.
const ZOOM_STEP: f64 = 1.2;
const ZOOM_MIN: f64 = 0.25;
const ZOOM_MAX: f64 = 5.0;
const IDENTITY_LOCAL_BUNDLE: &str = "quantdsh-provider-identity-local";
const COLLABORATION_BUNDLE: &str = "quantdsh-plugin-enterprise-collaboration";

/// The single official Host child process, with a generation for identity checks
/// (`runtime !== child` in the Electron carrier). Readiness lives in the local
/// `Arc<AtomicBool>` captured by the stdout reader, exactly like the Electron
/// `ready` closure variable — the runtime record itself carries no ready flag.
pub struct RuntimeProcess {
    pub pid: u32,
    pub generation: u64,
}

/// quantdsh `enterprise`: `{ login, root, home, deviceId, authority?, authFile? }`.
/// `id` replaces JavaScript object identity for `enterprise !== session`.
pub struct EnterpriseSession {
    pub id: u64,
    pub login: Arc<EnterpriseLogin>,
    pub root: PathBuf,
    pub home: PathBuf,
    pub device_id: String,
    pub authority: Option<Authority>,
    pub auth_file: Option<PathBuf>,
}

/// The carrier: one managed state object replacing the Electron main-process globals.
pub struct Carrier<R: Runtime> {
    pub paths: RuntimePaths,
    pub app: AppHandle<R>,
    /// Label of the current window (`window`).
    pub window: Mutex<Option<String>>,
    /// Origin of the current runtime window, for same-origin navigation checks.
    pub window_origin: Mutex<Option<String>>,
    /// `entrySurface`: the current window is a carrier surface.
    pub entry_surface: AtomicBool,
    pub quitting: AtomicBool,
    pub switching: AtomicBool,
    pub quit_requested: AtomicBool,
    pub return_requested: AtomicBool,
    pub runtime_starting: AtomicBool,
    pub verifying: AtomicBool,
    pub runtime: Mutex<Option<RuntimeProcess>>,
    pub installer: Mutex<Option<u32>>,
    pub enterprise_windows: Mutex<HashSet<String>>,
    pub enterprise_portal: Mutex<String>,
    pub managed_backend: Mutex<Option<String>>,
    pub pending_connection: Mutex<Option<DesktopConnection>>,
    pub enterprise: Mutex<Option<EnterpriseSession>>,
    pub verify_cancel: Mutex<Option<CancellationToken>>,
    pub next_window_id: AtomicU32,
    pub next_generation: AtomicU64,
    pub next_session_id: AtomicU64,
    /// Electron `viewMenu` zoom: a fresh webContents starts at 1.0.
    pub zoom: Mutex<f64>,
}

/// Snapshot restored by the `page.loadURL(...).catch(...)` rollback:
/// `previous`, `previousEntrySurface`, and the previous window's origin.
#[derive(Clone)]
struct RollbackState {
    previous: Option<String>,
    previous_origin: Option<String>,
    previous_entry_surface: bool,
    /// Everything needed to rebuild the window once. A WebView2 controller can
    /// come up with no working document (the navigation never commits and the
    /// window stays `about:blank`) while its siblings are fine; a rebuilt window
    /// loads normally, so one retry is enough.
    url: String,
    connection: DesktopConnection,
    carrier: bool,
    retried: bool,
}

fn ready_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(READY_PATTERN).unwrap())
}

/// Percent-encode a message for the `quantdsh://localhost/loading` query.
fn percent_encode(value: &str) -> String {
    let mut result = String::new();
    for byte in value.as_bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
        if keep {
            result.push(*byte as char);
        } else {
            result.push('%');
            result.push_str(&format!("{byte:02X}"));
        }
    }
    result
}

/// Percent-decode a query value (UTF-8 bytes).
fn percent_decode(value: &str) -> String {
    let mut result = Vec::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(byte) => {
                        result.push(byte);
                        index += 3;
                        continue;
                    }
                    None => {
                        result.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                result.push(b' ');
                index += 1;
            }
            byte => {
                result.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&result).to_string()
}

/// Carrier surface URL. WebView2 (Windows) cannot navigate to a non-standard scheme, so
/// there the custom protocol is addressed through wry's workaround host
/// `http://quantdsh.localhost/...`, which the registered protocol serves identically.
pub fn carrier_url(host: &str, query: Option<&str>) -> String {
    #[cfg(windows)]
    let base = format!("http://quantdsh.localhost/{host}");
    #[cfg(not(windows))]
    let base = format!("quantdsh://localhost/{host}");
    match query {
        Some(query) => format!("{base}?{query}"),
        None => base,
    }
}

fn entry_url() -> String {
    carrier_url("entry", None)
}

fn login_url() -> String {
    carrier_url("login", None)
}

/// Tauri-only repair surface, shown when no complete runtime pack exists.
fn runtime_url() -> String {
    carrier_url("runtime", None)
}

pub fn loading_url(message: &str) -> String {
    carrier_url("loading", Some(&format!("message={}", percent_encode(message))))
}

/// Normalize a navigation target to the `quantdsh://...` form the Electron carrier
/// compares against. On Windows a user gesture such as `window.location.assign`
/// arrives verbatim, while carrier loads arrive through the workaround host.
fn normalized_quantdsh_url(url: &tauri::Url) -> Option<tauri::Url> {
    if url.scheme() == "quantdsh" {
        return Some(url.clone());
    }
    if matches!(url.scheme(), "http" | "https") {
        if url.host_str() == Some("quantdsh.localhost") {
            let mut normalized = format!("quantdsh://localhost{}", url.path());
            if let Some(query) = url.query() {
                normalized.push('?');
                normalized.push_str(query);
            }
            return normalized.parse().ok();
        }
    }
    None
}

/// Carrier surfaces are the only navigation targets admitted inside a carrier window.
fn carrier_navigation_allowed(url: &tauri::Url) -> bool {
    if url.scheme() == "quantdsh" {
        // Only the carrier surfaces themselves; `quantdsh://personal` and friends are
        // commands, dispatched by the caller and never loaded as documents.
        return url.host_str() == Some("localhost");
    }
    matches!(url.scheme(), "http" | "https") && url.host_str() == Some("quantdsh.localhost")
}

/// Electron compares the literal `quantdsh://entry`; the Tauri carrier also addresses
/// its surfaces through `quantdsh://localhost/<surface>` (and the WebView2 workaround
/// host), so both spellings must land on the same entry flow.
fn is_entry_navigation(command: &tauri::Url) -> bool {
    match command.host_str() {
        Some(host) if host.eq_ignore_ascii_case("entry") => matches!(command.path(), "" | "/"),
        Some(host) if host.eq_ignore_ascii_case("localhost") => {
            matches!(command.path(), "/entry" | "entry")
        }
        _ => false,
    }
}

/// Electron `output = `${output}${text}`.slice(-16_384)`.
///
/// The cut must land on a character boundary: slicing bytes at an arbitrary
/// offset panics the moment the offset falls inside a multi-byte character,
/// which is the normal case for Chinese Host output — and that panic runs inside
/// the stdout reader task, so the carrier would silently stop reading the Host
/// instead of reporting a failure.
fn tail_limit_append(buffer: &mut String, text: &str) {
    if buffer.len() + text.len() <= OUTPUT_LIMIT {
        buffer.push_str(text);
        return;
    }
    buffer.push_str(text);
    crate::runtime_diagnostics::keep_tail_utf16(buffer, OUTPUT_LIMIT);
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true).mode(0o600);
    options
        .open(path)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(bytes)
        })
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

/// JSON.stringify(value, null, 2) + '\n' with mode 0o600.
fn write_json_private(path: &Path, value: &Value) -> Result<(), String> {
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    write_private(path, text.as_bytes())
}

impl<R: Runtime> Carrier<R> {
    pub fn new(app: AppHandle<R>, paths: RuntimePaths) -> Self {
        let portal = std::env::var_os("QUANTDSH_ENTERPRISE_PORTAL")
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        Self {
            paths,
            app,
            window: Mutex::new(None),
            window_origin: Mutex::new(None),
            entry_surface: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
            switching: AtomicBool::new(false),
            quit_requested: AtomicBool::new(false),
            return_requested: AtomicBool::new(false),
            runtime_starting: AtomicBool::new(false),
            verifying: AtomicBool::new(false),
            runtime: Mutex::new(None),
            installer: Mutex::new(None),
            enterprise_windows: Mutex::new(HashSet::new()),
            enterprise_portal: Mutex::new(portal),
            managed_backend: Mutex::new(None),
            pending_connection: Mutex::new(None),
            enterprise: Mutex::new(None),
            verify_cancel: Mutex::new(None),
            next_window_id: AtomicU32::new(1),
            next_generation: AtomicU64::new(1),
            next_session_id: AtomicU64::new(1),
            zoom: Mutex::new(1.0),
        }
    }

    // ---------------------------------------------------------------- paths

    pub fn runtime_home(&self) -> &Path {
        &self.paths.dsh_home
    }

    pub fn profile_directory(&self) -> PathBuf {
        self.runtime_home().join("profiles").join(PROFILE_NAME)
    }

    /// The bundled DSH version, read from the immutable installation.
    pub fn bundled_dsh_version(&self) -> Result<String, String> {
        let manifest = self
            .paths
            .bundled_profile
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh")
            .join("package.json");
        let text =
            std::fs::read_to_string(&manifest).map_err(|error| format!("{}: {error}", manifest.display()))?;
        let value: Value =
            serde_json::from_str(&text).map_err(|error| format!("{}: {error}", manifest.display()))?;
        value
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "Bundled DSH version is missing".to_string())
    }

    /// quantdsh `enterprisePluginAvailable`.
    pub fn enterprise_plugin_available(&self) -> bool {
        match self.bundled_dsh_version() {
            Ok(version) => installed_enterprise_plugins(&self.profile_directory(), &version)
                .map(|names| names.iter().any(|name| name == "quantdsh-provider-identity-enterprise"))
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    fn current_generation(&self) -> Option<u64> {
        self.runtime.lock().unwrap_or_else(|error| error.into_inner()).as_ref().map(|runtime| runtime.generation)
    }

    fn is_current_window(&self, label: &str) -> bool {
        self.window
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_deref()
            == Some(label)
    }

    // ------------------------------------------------------------- dialogs

    /// quantdsh `dialog.showErrorBox(title, content)`: non-blocking, modal.
    pub fn error_box(&self, title: &str, message: &str) {
        let app = self.app.clone();
        let title = title.to_string();
        let message = message.to_string();
        // The dialog plugin blocks the calling thread; never the main thread.
        std::thread::spawn(move || {
            app.dialog()
                .message(message)
                .title(title)
                .kind(tauri_plugin_dialog::MessageDialogKind::Error)
                .buttons(tauri_plugin_dialog::MessageDialogButtons::Ok)
                .blocking_show();
        });
    }

    // -------------------------------------------------------------- windows

    /// quantdsh `openWindow(url, connection, carrier)` — main thread only.
    pub fn open_window(self: &Arc<Self>, url: &str, connection: DesktopConnection, carrier: bool) {
        self.open_window_inner(url, connection, carrier, false)
    }

    /// `retried` marks the rebuild after a window whose WebView2 controller never
    /// produced a document; a second failure takes the rollback branch.
    fn open_window_inner(
        self: &Arc<Self>,
        url: &str,
        connection: DesktopConnection,
        carrier: bool,
        retried: bool,
    ) {
        let previous = self
            .window
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let label = format!(
            "quantdsh-window-{}",
            self.next_window_id.fetch_add(1, Ordering::SeqCst)
        );
        let parsed: tauri::Url = url.parse().expect("window url");
        let mut builder = WebviewWindowBuilder::new(
            &self.app,
            &label,
            WebviewUrl::External(parsed),
        )
        .title("QuantDSH")
        .min_inner_size(960.0, 640.0)
        .background_color(Color(17, 17, 19, 255))
        .visible(false);
        if let Some(previous_label) = &previous {
            if let Some(previous_window) = self.app.get_webview_window(previous_label) {
                // Restore the previous bounds, like Electron's getBounds().
                if let (Ok(position), Ok(size)) =
                    (previous_window.outer_position(), previous_window.outer_size())
                {
                    builder = builder
                        .position(position.x as f64, position.y as f64)
                        .inner_size(size.width as f64, size.height as f64);
                }
            }
        } else {
            builder = builder.inner_size(1440.0, 960.0);
        }
        // A per-window icon is a Linux/Windows concern in Electron; `icon` consumes
        // the builder, so it is applied last, right before `build`.
        let window_icon = tauri::image::Image::from_bytes(include_bytes!("../../assets/app-icon.png")).ok();
        if carrier {
            builder = builder.initialization_script(carrier_init_script("carrier"));
        }
        // Electron assigns a per-partition session to enterprise windows.
        if let Some(partition) = crate::connection::connection_partition(&connection) {
            let directory = self.paths.user_data.join("Partitions").join(&partition);
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            {
                builder = builder.data_directory(directory);
            }
            #[cfg(target_os = "macos")]
            {
                // WKWebView identifies data stores by UUID; derive a stable one from
                // the *whole* partition name. Taking the first 16 bytes would give
                // every enterprise account the same identifier, because the name
                // starts with the constant `quantdsh-enterprise-`.
                let digest = sha2::Sha256::digest(partition.as_bytes());
                let mut identifier = [0u8; 16];
                identifier.copy_from_slice(&digest[..16]);
                builder = builder.data_store_identifier(identifier);
                let _ = std::fs::create_dir_all(&directory);
            }
        }
        // `previousEntrySurface` and the previous window's origin, kept for the
        // `page.loadURL(...).catch(...)` rollback further down.
        let previous_entry_surface = self.entry_surface.load(Ordering::SeqCst);
        let previous_origin = self
            .window_origin
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        *self.window.lock().unwrap_or_else(|error| error.into_inner()) = Some(label.clone());
        // A new webContents starts at the default zoom factor.
        *self.zoom.lock().unwrap_or_else(|error| error.into_inner()) = 1.0;
        self.entry_surface.store(carrier, Ordering::SeqCst);
        if !carrier {
            *self.window_origin.lock().unwrap_or_else(|error| error.into_inner()) = url
                .parse::<tauri::Url>()
                .ok()
                .map(|parsed| parsed.origin().ascii_serialization());
        } else {
            *self.window_origin.lock().unwrap_or_else(|error| error.into_inner()) = None;
        }
        // window.on('page-title-updated'): the title always stays 'QuantDSH'.
        {
            let title_window_label = label.clone();
            builder = builder.on_document_title_changed(move |window, _title| {
                if window.label() == title_window_label {
                    let _ = window.set_title("QuantDSH");
                }
            });
        }
        // window.webContents.setWindowOpenHandler
        {
            let app = self.app.clone();
            builder = builder.on_new_window(move |url, _features| {
                if matches!(url.scheme(), "http" | "https") {
                    let _ = app.opener().open_url(url.as_str(), None::<String>);
                }
                tauri::webview::NewWindowResponse::Deny
            });
        }
        if carrier {
            // Electron `connectEntryNavigation`: every carrier navigation is prevented
            // and the `quantdsh:` command is dispatched instead. The carrier surfaces
            // themselves stay loadable, including the WebView2 workaround host.
            let this = Arc::clone(self);
            let navigation_label = label.clone();
            builder = builder.on_navigation(move |url| {
                if url.scheme() == "quantdsh" && url.host_str() != Some("localhost") {
                    let this = Arc::clone(&this);
                    let label = navigation_label.clone();
                    let target = url.as_str().to_string();
                    tauri::async_runtime::spawn(async move {
                        this.handle_navigation(&label, &target).await;
                    });
                    return false;
                }
                carrier_navigation_allowed(url)
            });
        } else {
            // page.webContents.on('will-navigate')
            let this = Arc::clone(self);
            let navigation_label = label.clone();
            let origin = self
                .window_origin
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            builder = builder.on_navigation(move |url| {
                if let Some(command) = normalized_quantdsh_url(url) {
                    if is_entry_navigation(&command)
                        && this.is_current_window(&navigation_label)
                        && !this.switching.load(Ordering::SeqCst)
                    {
                        if !this.enterprise_plugin_available() {
                            this.error_box("企业插件不可用", "请先安装兼容的企业账号插件。");
                            return false;
                        }
                        this.return_to_entry_spawned();
                    }
                    return false;
                }
                let allowed = match &origin {
                    Some(origin) => url.origin().ascii_serialization() == *origin,
                    None => false,
                };
                allowed
            });
        }
        // page.loadURL(...).then(show, destroy previous).catch(rollback)
        let loaded = Arc::new(AtomicBool::new(false));
        {
            let this = Arc::clone(self);
            let load_label = label.clone();
            let previous = previous.clone();
            let loaded = Arc::clone(&loaded);
            builder = builder.on_page_load(move |window, payload| {
                if window.label() != load_label {
                    return;
                }
                // Electron reveals on `ready-to-show` — the first paint, not
                // `did-finish-load`. wry's `Started` (ContentLoading) is that moment, so
                // the previous window goes away as soon as this one has something on
                // screen. Waiting for `Finished` is wrong twice over: a SPA keeps
                // loading long past its first paint, and `Finished` is not guaranteed at
                // all. The window is already visible by now (see `open_window`); repeat
                // the call so a window that lost a race still surfaces.
                if loaded.swap(true, Ordering::SeqCst) {
                    return;
                }
                eprintln!("[quantdsh] window {load_label} ready at {url}", url = payload.url());
                let _ = window.show();
                if let Some(previous_label) = &previous {
                    if let Some(previous_window) = this.app.get_webview_window(previous_label) {
                        let _ = previous_window.destroy();
                    }
                }
            });
        }
        if is_enterprise_window(&connection) {
            self.enterprise_windows
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(label.clone());
        }
        let window = {
            let built = match window_icon {
                Some(icon) => builder
                    .icon(icon)
                    .map_err(|error| error.to_string())
                    .and_then(|with_icon| with_icon.build().map_err(|error| error.to_string())),
                None => builder.build().map_err(|error| error.to_string()),
            };
            match built {
                Ok(window) => window,
                Err(error) => {
                    eprintln!("[quantdsh] window build failed: {error}");
                    self.error_box("无法打开工作区", &error);
                    return;
                }
            }
        };
        // Show as soon as the window exists. Electron builds the window hidden and
        // reveals it on `ready-to-show`, and a hidden window is harmless there. It is
        // not harmless on WebView2: a webview whose controller reports
        // `IsVisible(false)` is suspended, so the navigation wry issues while building
        // gets serialised (`on_navigation` runs, even redirects resolve) but the
        // document is never committed and `ContentLoading` never fires. The window then
        // sits on `about:blank` forever and the load watchdog tears it down. The
        // background colour is already our shell colour, so there is no white flash.
        let _ = window.show();
        // page.on('closed') — WebviewWindow::on_window_event, after build.
        {
            let this = Arc::clone(self);
            let closed_label = label.clone();
            window.on_window_event(move |event| {
                if !matches!(event, WindowEvent::Destroyed) {
                    return;
                }
                this.enterprise_windows
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&closed_label);
                if this.is_current_window(&closed_label) {
                    *this.window.lock().unwrap_or_else(|error| error.into_inner()) = None;
                    *this.window_origin.lock().unwrap_or_else(|error| error.into_inner()) = None;
                }
                // app.on('window-all-closed')
                if !cfg!(target_os = "macos") && !this.switching.load(Ordering::SeqCst) {
                    if this.app.webview_windows().is_empty() {
                        this.app.exit(0);
                    }
                }
            });
        }
        // page.loadURL(...).catch(...) — roll back to the previous window when this
        // one never finishes loading.
        {
            let this = Arc::clone(self);
            let rollback_label = label.clone();
            let rollback = RollbackState {
                previous,
                previous_origin,
                previous_entry_surface,
                url: url.to_string(),
                connection,
                carrier,
                retried,
            };
            std::thread::spawn(move || {
                std::thread::sleep(WINDOW_LOAD_TIMEOUT);
                if loaded.load(Ordering::SeqCst) {
                    return;
                }
                eprintln!("[quantdsh] window {rollback_label} never loaded; rolling back");
                let handler = Arc::clone(&this);
                let _ = this.app.run_on_main_thread(move || {
                    handler.roll_back_failed_window(&rollback_label, rollback);
                });
            });
        }
    }

    /// The `page.loadURL(...).catch(...)` branch: drop the dead window, restore the
    /// previous one, report, and leave a running Host's workspace.
    fn roll_back_failed_window(self: &Arc<Self>, label: &str, rollback: RollbackState) {
        // A window that is already gone never failed to load (the user closed it, or
        // `page.on('closed')` already cleared the state): nothing to roll back.
        let Some(window) = self.app.get_webview_window(label) else {
            return;
        };
        if !self.is_current_window(label) {
            // Superseded by a newer window within the timeout: drop the dead one but
            // leave the newer window's state alone.
            let _ = window.destroy();
            return;
        }
        // A WebView2 controller that never produced a document does not recover from
        // `navigate()` or `reload()` — the window stays `about:blank` forever — but a
        // freshly built window loads normally, so rebuild once before reporting a
        // failure the user cannot act on.
        if !rollback.retried {
            if let Some(previous_label) = &rollback.previous {
                if self.app.get_webview_window(previous_label).is_some() {
                    *self.window.lock().unwrap_or_else(|error| error.into_inner()) =
                        Some(previous_label.clone());
                    *self.window_origin.lock().unwrap_or_else(|error| error.into_inner()) =
                        rollback.previous_origin.clone();
                    self.entry_surface
                        .store(rollback.previous_entry_surface, Ordering::SeqCst);
                }
            }
            let _ = window.destroy();
            self.open_window_inner(&rollback.url, rollback.connection, rollback.carrier, true);
            return;
        }
        // Electron's `page.on('closed')` guard reads `window === page`, and the
        // reassignment below happens before that event is delivered; restore the
        // current window first so destroying it does not clear the restored state.
        if let Some(previous_label) = &rollback.previous {
            if self.app.get_webview_window(previous_label).is_some() {
                *self.window.lock().unwrap_or_else(|error| error.into_inner()) =
                    Some(previous_label.clone());
                *self.window_origin.lock().unwrap_or_else(|error| error.into_inner()) =
                    rollback.previous_origin.clone();
                self.entry_surface
                    .store(rollback.previous_entry_surface, Ordering::SeqCst);
            }
        }
        let _ = window.destroy();
        self.error_box("无法打开工作区", "请检查企业地址、网络或本地运行状态。");
        if self
            .runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
        {
            self.return_to_entry_spawned();
        }
    }

    /// Queue `open_window` on the main thread; window creation is main-thread only.
    pub fn open_window_dispatch(self: &Arc<Self>, url: String, connection: DesktopConnection, carrier: bool) {
        let this = Arc::clone(self);
        let _ = self.app.run_on_main_thread(move || {
            this.open_window(&url, connection, carrier);
        });
    }

    /// Navigate the current window to a carrier loading surface.
    fn show_loading(&self, message: &str) -> Result<(), String> {
        let label = self
            .window
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let Some(label) = label else { return Ok(()) };
        let Some(window) = self.app.get_webview_window(&label) else {
            return Ok(());
        };
        let url = loading_url(message)
            .parse::<tauri::Url>()
            .map_err(|error| error.to_string())?;
        window.navigate(url).map_err(|error| error.to_string())
    }

    // ------------------------------------------------------------ environment

    /// quantdsh `runtimeEnvironment(home)`.
    pub fn runtime_environment(&self, home: &Path) -> HashMap<String, String> {
        let parent: HashMap<String, String> = std::env::vars().collect();
        let mut env = {
            let enterprise = self.enterprise.lock().unwrap_or_else(|error| error.into_inner());
            match enterprise.as_ref() {
                Some(session) => {
                    enterprise_environment(&session.root, &self.paths.node_executable, &parent)
                }
                None => parent,
            }
        };
        env.insert(
            "DSH_HOME".to_string(),
            home.display().to_string(),
        );
        let agents = {
            let enterprise = self.enterprise.lock().unwrap_or_else(|error| error.into_inner());
            match enterprise.as_ref() {
                Some(session) => session.root.join("agents"),
                None => home.join("agents"),
            }
        };
        env.insert("DSH_AGENTS_HOME".to_string(), agents.display().to_string());
        env.insert(
            "DSH_BUNDLED_PRIMARY_RUNTIME".to_string(),
            self.paths.primary_runtime.display().to_string(),
        );
        if let Ok(executable) = std::env::current_exe() {
            env.insert(
                "DSH_ELECTRON_EXECUTABLE".to_string(),
                executable.display().to_string(),
            );
        }
        // The provider treats QUANTDSH_ELECTRON_APP as the application path it spawns;
        // the Tauri executable is the application, so the override must never be set.
        env.remove("QUANTDSH_ELECTRON_APP");
        env.remove("ELECTRON_RUN_AS_NODE");
        env
    }

    /// quantdsh `officialCommand(home, args)`.
    pub async fn official_command(
        self: &Arc<Self>,
        home: &Path,
        args: &[&str],
    ) -> Result<(), String> {
        let launcher = official_launcher(
            &self.paths.bundled_profile,
            home,
            &self.paths.node_executable,
            &self.paths.pnpm_cli,
        )?;
        let this = Arc::clone(self);
        let process_owner = Arc::clone(self);
        let options = OfficialCommandOptions {
            executable: self.paths.node_executable.clone(),
            launcher,
            args: args.iter().map(|arg| arg.to_string()).collect(),
            home: home.to_path_buf(),
            env: self.runtime_environment(home),
            log_directory: self.paths.log_directory.clone(),
            timeout_ms: None,
            cancelled: Box::new(move || this.quit_requested.load(Ordering::SeqCst)),
            on_process: Box::new(move |pid| {
                *process_owner.installer.lock().unwrap_or_else(|error| error.into_inner()) = pid;
            }),
        };
        run_official_command(options).await
    }

    /// quantdsh `prepareProfile(home)`.
    pub async fn prepare_profile(self: &Arc<Self>, home: &Path) -> Result<(PathBuf, bool), String> {
        let profile_directory = home.join("profiles").join(PROFILE_NAME);
        let existing_home = [
            profile_directory.join("package.json"),
            profile_directory.join("cordis.yml"),
            home.join("cordis.yml"),
            home.join(".quantdsh-host.mjs"),
        ]
        .iter()
        .any(|path| path.exists());
        let source = &self.paths.bundled_profile;
        let materialized = materialize_runtime_profile(source, home)?;
        if materialized.changed {
            let entry = self
                .window
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            if let Some(label) = &entry {
                if self.app.get_webview_window(label).is_some() {
                    self.show_loading("正在更新工作区插件依赖")?;
                }
            }
            self.official_command(home, &["plugin", "--profile", PROFILE_NAME, "install"])
                .await?;
            mark_profile_updated(source, &materialized.profile)?;
            let message = {
                let enterprise = self.enterprise.lock().unwrap_or_else(|error| error.into_inner());
                if enterprise.is_some() {
                    "正在启动企业工作区"
                } else {
                    "正在启动个人工作区"
                }
            };
            if let Some(label) = &entry {
                if self.app.get_webview_window(label).is_some() {
                    self.show_loading(message)?;
                }
            }
        }
        official_launcher(
            source,
            home,
            &self.paths.node_executable,
            &self.paths.pnpm_cli,
        )?;
        Ok((materialized.profile, existing_home))
    }

    /// quantdsh `rememberCommandSpace(home)`.
    pub fn remember_command_space(&self, home: Option<&Path>) {
        let active = home.map(|home| {
            let enterprise = self.enterprise.lock().unwrap_or_else(|error| error.into_inner());
            let (mode, root, label) = match enterprise.as_ref() {
                Some(session) => (
                    "enterprise",
                    Some(session.root.display().to_string()),
                    format!(
                        "{} / {}",
                        session.login.actor.organization_name, session.login.actor.display_name
                    ),
                ),
                None => ("personal", None, "个人工作区".to_string()),
            };
            let mut object = Map::new();
            object.insert("mode".to_string(), json!(mode));
            object.insert("home".to_string(), json!(home.display().to_string()));
            if let Some(root) = root {
                object.insert("root".to_string(), json!(root));
            }
            object.insert("label".to_string(), json!(label));
            Value::Object(object)
        });
        let payload = json!({
            "personalHome": self.paths.dsh_home.display().to_string(),
            "active": active,
        });
        let _ = std::fs::create_dir_all(&self.paths.user_data);
        let _ = write_json_private(&self.paths.command_context_file(), &payload);
    }

    /// quantdsh `startRuntime(home, connection, patches, existingHome)`.
    pub async fn start_runtime(
        self: &Arc<Self>,
        home: &Path,
        connection: DesktopConnection,
        patches: &[PathBuf],
        existing_home: bool,
    ) -> Result<(), String> {
        self.remember_command_space(Some(home));
        if self.quit_requested.load(Ordering::SeqCst) {
            return Err("启动已取消".to_string());
        }
        // Belt and braces: `start()` already refuses to show the entry surface,
        // so this only fires if a navigation reaches the carrier anyway.
        if !self.paths.runtime_problems.is_empty() {
            self.show_runtime_guide();
            return Err("本机运行时不完整，请先修复运行时".to_string());
        }
        let executable = &self.paths.node_executable;
        if !executable.exists() {
            return Err(format!(
                "Bundled Node.js runtime is missing: {}",
                executable.display()
            ));
        }
        let base_patch = self.paths.bundled_profile.join("cordis.patch.yml");
        let mut user_patches = vec![
            home.join("profiles").join(PROFILE_NAME).join("cordis.yml"),
            home.join("profiles").join(PROFILE_NAME).join("cordis.patch.yml"),
            home.join("cordis.yml"),
            home.join("cordis.patch.yml"),
        ];
        user_patches.extend(patches.iter().cloned());
        let port = prepare_runtime_port(home, &user_patches, RuntimePortOptions { existing_home }).await?;
        if self.quit_requested.load(Ordering::SeqCst) {
            return Err("启动已取消".to_string());
        }
        let mut layers: Vec<PathBuf> = Vec::new();
        if base_patch.exists() {
            layers.push(base_patch);
        }
        layers.extend(patches.iter().cloned());
        if let RuntimePort::Managed { patch, .. } = &port {
            layers.push(patch.clone());
        }
        let launcher = official_host_launcher(
            &self.paths.bundled_profile,
            home,
            executable,
            &self.paths.pnpm_cli,
            &layers,
        )?;
        let cwd = {
            let enterprise = self.enterprise.lock().unwrap_or_else(|error| error.into_inner());
            match enterprise.as_ref() {
                Some(session) => session.root.join("workspace"),
                None => home.to_path_buf(),
            }
        };
        let env = self.runtime_environment(home);
        let started_at = SystemTime::now();
        let mut command = tokio::process::Command::new(executable);
        command.arg(&launcher).arg("--no-open");
        command.current_dir(&cwd);
        command.env_clear();
        for (name, value) in &env {
            command.env(name, value);
        }
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        {
            // detached: the child becomes its own group leader, like the Electron carrier.
            unsafe {
                command.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }
        #[cfg(windows)]
        {
            // windowsHide: true
            command.creation_flags(0x0800_0000);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                // child.once('error')
                if !self.quitting.load(Ordering::SeqCst) {
                    self.error_box("启动失败", &redact_runtime_output(&error.to_string()));
                    self.return_to_entry_spawned();
                }
                return Ok(());
            }
        };
        let pid = child.id().unwrap_or(0);
        track_owned_process(pid, started_at);
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        let ready = Arc::new(AtomicBool::new(false));
        {
            let mut runtime = self.runtime.lock().unwrap_or_else(|error| error.into_inner());
            *runtime = Some(RuntimeProcess {
                pid,
                generation,
            });
        }
        self.runtime_starting.store(true, Ordering::SeqCst);
        let diagnostic_output = Arc::new(Mutex::new(String::new()));
        let detected_output = Arc::new(Mutex::new(String::new()));

        // child.stdout.on('data', consume)
        if let Some(stdout) = child.stdout.take() {
            let this = Arc::clone(self);
            let ready = ready.clone();
            let connection = connection.clone();
            let diagnostic_output = diagnostic_output.clone();
            let detected_output = detected_output.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut reader = stdout;
                let mut redactor = RuntimeOutputRedactor::new();
                let mut chunk = [0u8; 8192];
                loop {
                    match reader.read(&mut chunk).await {
                        Ok(0) => break,
                        Ok(count) => {
                            let bytes = &chunk[..count];
                            let text = String::from_utf8_lossy(bytes).to_string();
                            let mut diagnostic = diagnostic_output
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            let mut redacted = String::new();
                            redactor.push(&mut |line| redacted.push_str(line), bytes);
                            for line in redacted.lines() {
                                eprintln!("{line}");
                            }
                            tail_limit_append(&mut diagnostic, &redacted);
                            drop(diagnostic);
                            if ready.load(Ordering::SeqCst)
                                || this.switching.load(Ordering::SeqCst)
                                || this.current_generation() != Some(generation)
                            {
                                continue;
                            }
                            let mut output = detected_output
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            tail_limit_append(&mut output, &text);
                            if let Some(found) = ready_pattern()
                                .captures(&output)
                                .and_then(|captures| captures.get(1))
                            {
                                let target = found.as_str().to_string();
                                ready.store(true, Ordering::SeqCst);
                                this.runtime_starting.store(false, Ordering::SeqCst);
                                this.open_window_dispatch(target, connection.clone(), false);
                            }
                        }
                        Err(_) => break,
                    }
                }
                let mut redacted = String::new();
                redactor.end(&mut |line| redacted.push_str(line));
                let mut diagnostic = diagnostic_output
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                tail_limit_append(&mut diagnostic, &redacted);
            });
        }
        // child.stderr.on('data') — diagnostics only, never readiness.
        if let Some(stderr) = child.stderr.take() {
            let diagnostic_output = diagnostic_output.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut reader = stderr;
                let mut redactor = RuntimeOutputRedactor::new();
                let mut chunk = [0u8; 8192];
                loop {
                    match reader.read(&mut chunk).await {
                        Ok(0) => break,
                        Ok(count) => {
                            let mut redacted = String::new();
                            redactor.push(&mut |line| redacted.push_str(line), &chunk[..count]);
                            for line in redacted.lines() {
                                eprintln!("{line}");
                            }
                            let mut diagnostic = diagnostic_output
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            tail_limit_append(&mut diagnostic, &redacted);
                        }
                        Err(_) => break,
                    }
                }
                let mut redacted = String::new();
                redactor.end(&mut |line| redacted.push_str(line));
                let mut diagnostic = diagnostic_output
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                tail_limit_append(&mut diagnostic, &redacted);
            });
        }
        // child.once('exit')
        {
            let this = Arc::clone(self);
            let exit_ready = ready.clone();
            let port = port.clone();
            let diagnostic_output = diagnostic_output.clone();
            let exit_cancel = CancellationToken::new();
            let timeout_cancel = exit_cancel.clone();
            tokio::spawn(async move {
                let code = child.wait().await.ok().and_then(|status| status.code());
                // trackOwnedProcess() records `exitedAt` on the child's exit event;
                // the Unix graceful-stop path reads it.
                note_child_exit(pid);
                exit_cancel.cancel();
                if this.current_generation() != Some(generation) {
                    return;
                }
                match clean_owned_process_tree(pid).await {
                    Ok(()) => {
                        if this.current_generation() == Some(generation) {
                            *this.runtime.lock().unwrap_or_else(|error| error.into_inner()) = None;
                            this.runtime_starting.store(false, Ordering::SeqCst);
                        }
                        if !this.quitting.load(Ordering::SeqCst)
                            && !this.switching.load(Ordering::SeqCst)
                        {
                            let detail = this.runtime_failure_detail(
                                exit_ready.load(Ordering::SeqCst),
                                &port,
                                &diagnostic_output
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner()),
                                code,
                                false,
                            );
                            // quantdsh `dialog.showErrorBox('本机运行已停止', failureDetail(code))`;
                            // `failureDetail` has already written `logs/runtime-failure.log`.
                            this.error_box("本机运行已停止", &detail);
                            this.return_to_entry_spawned();
                        }
                    }
                    Err(_) => {
                        this.runtime_starting.store(false, Ordering::SeqCst);
                        let detail = this.runtime_failure_detail(
                            exit_ready.load(Ordering::SeqCst),
                            &port,
                            &diagnostic_output
                                .lock()
                                .unwrap_or_else(|error| error.into_inner()),
                            code,
                            true,
                        );
                        // quantdsh reports this one without returning to the entry
                        // surface: the process tree is still there.
                        this.error_box("本机运行失败，进程清理未确认", &detail);
                    }
                }
            });
            // setTimeout(readyTimeout): cleared when the child exits.
            let this = Arc::clone(self);
            let ready = ready.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = tokio::time::sleep(READY_TIMEOUT) => {}
                    _ = timeout_cancel.cancelled() => return,
                }
                if !ready.load(Ordering::SeqCst) && this.current_generation() == Some(generation) {
                    this.error_box("启动超时", "官方 DSH 尚未就绪，已停止本机进程");
                    this.return_to_entry_spawned();
                }
            });
        }
        Ok(())
    }

    /// quantdsh `failureDetail` closure.
    fn runtime_failure_detail(
        &self,
        ready: bool,
        port: &RuntimePort,
        diagnostic: &str,
        code: Option<i32>,
        cleanup_failed: bool,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !ready {
            if let Some(conflict) = runtime_port_conflict(port, diagnostic) {
                parts.push(conflict);
            }
        }
        parts.push(crate::runtime_diagnostics::runtime_failure_detail(
            ready,
            code,
            diagnostic,
            cleanup_failed,
        ));
        let detail = parts.join("\n\n");
        let directory = &self.paths.log_directory;
        let path = directory.join("runtime-failure.log");
        if std::fs::create_dir_all(directory).is_ok() {
            let text = runtime_failure_log(ready, code, diagnostic, cleanup_failed);
            if write_private(&path, text.as_bytes()).is_ok() {
                return format!("{detail}\n\n错误记录：{}", path.display());
            }
        }
        detail
    }

    /// quantdsh `stopRuntime`.
    pub fn stop_runtime(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        let pid = self
            .runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|runtime| runtime.pid);
        if let Some(pid) = pid {
            tauri::async_runtime::spawn(async move {
                let _ = clean_owned_process_tree(pid).await;
            });
        }
        *self.runtime.lock().unwrap_or_else(|error| error.into_inner()) = None;
        self.runtime_starting.store(false, Ordering::SeqCst);
    }

    /// quantdsh `leaveWorkspace`.
    pub async fn leave_workspace(&self) -> Result<(), String> {
        {
            let mut cancel = self.verify_cancel.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(token) = cancel.take() {
                token.cancel();
            }
        }
        let mut session = self
            .enterprise
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        let page = self
            .window
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        // Shut admission before waiting for any graceful process exit.
        if let Some(authority) = session.as_mut().and_then(|session| session.authority.take()) {
            authority.close().await;
        }
        if let Some(session) = &session {
            session.login.cancel_requests();
        }
        let current = {
            let runtime = self.runtime.lock().unwrap_or_else(|error| error.into_inner());
            runtime.as_ref().map(|runtime| (runtime.pid, runtime.generation))
        };
        if let Some((pid, generation)) = current {
            stop_owned_process(pid).await?;
            let mut runtime = self.runtime.lock().unwrap_or_else(|error| error.into_inner());
            if runtime.as_ref().map(|runtime| runtime.generation) == Some(generation) {
                *runtime = None;
                self.runtime_starting.store(false, Ordering::SeqCst);
            }
        }
        *self.enterprise.lock().unwrap_or_else(|error| error.into_inner()) = None;
        self.remember_command_space(None);
        if let Some(session) = &session {
            if let Some(auth_file) = &session.auth_file {
                let _ = std::fs::remove_file(auth_file);
            }
            let revoked = session.login.logout().await;
            if !revoked {
                self.error_box(
                    "本机已退出",
                    "本机企业运行和登录已停止；后台连接失败，远端撤销尚未确认。",
                );
            }
            if let Some(label) = &page {
                if let Some(window) = self.app.get_webview_window(label) {
                    let _ = window.clear_all_browsing_data();
                }
            }
        }
        let enterprise_labels: Vec<String> = self
            .enterprise_windows
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .cloned()
            .collect();
        for label in enterprise_labels {
            if let Some(window) = self.app.get_webview_window(&label) {
                let _ = window.destroy();
            }
        }
        self.enterprise_windows
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        if let Some(label) = &page {
            if let Some(window) = self.app.get_webview_window(label) {
                let _ = window.destroy();
            }
        }
        *self.pending_connection.lock().unwrap_or_else(|error| error.into_inner()) = None;
        self.entry_surface.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// The `before-quit` choreography, shared by `RunEvent::ExitRequested` and the
    /// installer's `--dsh-installer-quit` handoff (which reaches the running
    /// instance through the single-instance channel).
    pub fn quit_now(self: &Arc<Self>) {
        if self.quitting.load(Ordering::SeqCst) {
            self.stop_runtime();
            self.app.exit(0);
            return;
        }
        self.switching.store(true, Ordering::SeqCst);
        let carrier_task = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            match carrier_task.leave_workspace().await {
                Ok(()) => {
                    carrier_task.quitting.store(true, Ordering::SeqCst);
                    carrier_task.app.exit(0);
                }
                Err(error) => {
                    carrier_task.error_box("暂时无法退出", &error);
                    carrier_task.switching.store(false, Ordering::SeqCst);
                }
            }
        });
    }

    /// quantdsh `returnToEntry`.
    pub async fn return_to_entry(self: &Arc<Self>) {
        if self.quitting.load(Ordering::SeqCst) {
            return;
        }
        if self.switching.load(Ordering::SeqCst) {
            self.return_requested.store(true, Ordering::SeqCst);
            return;
        }
        self.switching.store(true, Ordering::SeqCst);
        match self.leave_workspace().await {
            Ok(()) => self.show_connection_entry(),
            Err(error) => self.error_box("暂时无法切换", &error),
        }
        self.end_switch();
    }

    pub fn return_to_entry_spawned(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            this.return_to_entry().await;
        });
    }

    /// quantdsh `endSwitch`.
    fn end_switch(self: &Arc<Self>) {
        self.switching.store(false, Ordering::SeqCst);
        if self.quit_requested.load(Ordering::SeqCst) {
            self.app.exit(0);
        } else if self.return_requested.swap(false, Ordering::SeqCst) {
            self.return_to_entry_spawned();
        }
    }

    // --------------------------------------------------------------- backend

    pub fn saved_backend_file(&self) -> PathBuf {
        self.paths.enterprise_backend_file()
    }

    /// quantdsh `rememberBackend`.
    pub fn remember_backend(&self, backend: &str) {
        if self
            .managed_backend
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
        {
            return;
        }
        let _ = std::fs::create_dir_all(&self.paths.user_data);
        let _ = write_json_private(
            &self.saved_backend_file(),
            &json!({ "backend": backend }),
        );
    }

    // ------------------------------------------------------------ entry flow

    /// quantdsh `showConnectionEntry`.
    pub fn show_connection_entry(self: &Arc<Self>) {
        self.open_window_dispatch(
            entry_url(),
            DesktopConnection::Personal,
            true,
        );
    }

    /// Repair surface for an incomplete runtime pack. Tauri-only: the Electron
    /// carrier threw `Bundled Node.js runtime is missing` and left a dead window.
    pub fn show_runtime_guide(self: &Arc<Self>) {
        self.open_window_dispatch(runtime_url(), DesktopConnection::Personal, true);
    }

    /// quantdsh `connectEntryNavigation` will-navigate body.
    pub async fn handle_navigation(self: &Arc<Self>, label: &str, target: &str) {
        if self.switching.load(Ordering::SeqCst) {
            return;
        }
        if !self.is_current_window(label) || !self.entry_surface.load(Ordering::SeqCst) {
            return;
        }
        self.switching.store(true, Ordering::SeqCst);
        match self.navigate_command(target).await {
            Ok(()) => {}
            Err(error) => {
                self.error_box("无法进入", &error);
                self.show_connection_entry();
            }
        }
        self.end_switch();
    }

    async fn navigate_command(self: &Arc<Self>, target: &str) -> Result<(), String> {
        let command: tauri::Url = target.parse().map_err(|_| "无效的使用方式指令".to_string())?;
        if command.scheme() != "quantdsh" {
            return Ok(());
        }
        match command.host_str() {
            Some("enterprise") => {
                if self
                    .enterprise
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .is_some()
                {
                    return Err("请先退出当前企业账号再修改后台地址".to_string());
                }
                let backend = {
                    let managed = self
                        .managed_backend
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .clone();
                    match managed {
                        Some(backend) => backend,
                        None => command
                            .query_pairs()
                            .find(|(key, _)| key == "portal")
                            .map(|(_, value)| value.to_string())
                            .unwrap_or_default(),
                    }
                };
                let connection = enterprise_connection(&backend)?;
                *self
                    .pending_connection
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some(connection.clone());
                let portal = match &connection {
                    DesktopConnection::Enterprise { portal_origin, .. } => portal_origin.clone(),
                    DesktopConnection::Personal => unreachable!(),
                };
                *self
                    .enterprise_portal
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = portal.clone();
                self.remember_backend(&portal);
                self.open_window_dispatch(login_url(), DesktopConnection::Personal, true);
                Ok(())
            }
            Some("personal") => {
                if self
                    .enterprise
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .is_some()
                {
                    self.leave_workspace().await?;
                }
                let home = self.paths.dsh_home.clone();
                self.show_loading("正在启动个人工作区")?;
                let (_profile, existing_home) = self.prepare_profile(&home).await?;
                self.entry_surface.store(false, Ordering::SeqCst);
                self.start_runtime(&home, DesktopConnection::Personal, &[], existing_home)
                    .await
            }
            Some("entry") => {
                self.leave_workspace().await?;
                self.show_connection_entry();
                Ok(())
            }
            _ => Ok(()),
        }
    }

    // -------------------------------------------------------- enterprise flow

    /// quantdsh `activateEnterprise(existingHome)`.
    pub async fn activate_enterprise(
        self: &Arc<Self>,
        session_id: u64,
        existing_home: bool,
    ) -> Result<(), String> {
        let (login, root, home, device_id) = {
            let enterprise = self.enterprise.lock().unwrap_or_else(|error| error.into_inner());
            let session = enterprise
                .as_ref()
                .filter(|session| session.id == session_id)
                .ok_or_else(|| "请重新登录企业账号".to_string())?;
            (
                session.login.clone(),
                session.root.clone(),
                session.home.clone(),
                session.device_id.clone(),
            )
        };
        login.verify().await?;
        if self.quit_requested.load(Ordering::SeqCst) {
            return Err("启动已取消".to_string());
        }
        let version = self.bundled_dsh_version()?;
        let plugin_names = compose_installed_enterprise_plugins(
            &self.profile_directory(),
            &home.join("profiles").join(PROFILE_NAME),
            &version,
            &self.paths.node_executable,
            &self.paths.pnpm_cli,
        )
        .await?;
        {
            let enterprise = self.enterprise.lock().unwrap_or_else(|error| error.into_inner());
            if self.quit_requested.load(Ordering::SeqCst)
                || enterprise.as_ref().map(|session| session.id) != Some(session_id)
            {
                return Err("企业启动已取消".to_string());
            }
        }
        // Disable personal identity at composition, rather than allow an identity fallback.
        let manifest = home.join("profiles").join(PROFILE_NAME).join("package.json");
        let text = std::fs::read_to_string(&manifest)
            .map_err(|error| format!("{}: {error}", manifest.display()))?;
        let mut selected: Value = serde_json::from_str(&text)
            .map_err(|error| format!("{}: {error}", manifest.display()))?;
        {
            let object = selected
                .as_object_mut()
                .ok_or_else(|| "Profile 清单无效".to_string())?;
            let dsh = object
                .entry("dsh".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            let profile = dsh
                .as_object_mut()
                .map(|object| {
                    object
                        .entry("profile".to_string())
                        .or_insert_with(|| Value::Object(Map::new()))
                });
            if let Some(profile) = profile.and_then(|profile| profile.as_object_mut()) {
                let bundles = profile
                    .entry("bundles".to_string())
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Some(bundles) = bundles.as_array_mut() {
                    bundles.retain(|name| name.as_str() != Some(IDENTITY_LOCAL_BUNDLE));
                    for name in &plugin_names {
                        let value = json!(name);
                        if !bundles.contains(&value) {
                            bundles.push(value);
                        }
                    }
                }
            }
        }
        write_json_private(&manifest, &selected)?;
        // Record before writing either file, so a partial patch failure is also cleaned.
        {
            let mut enterprise = self.enterprise.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(session) = enterprise
                .as_mut()
                .filter(|session| session.id == session_id)
            {
                session.auth_file = Some(home.join("enterprise-auth.json"));
            }
        }
        let on_event: Arc<dyn Fn() + Send + Sync> = {
            let this = Arc::clone(self);
            Arc::new(move || {
                this.return_to_entry_spawned();
            })
        };
        let authority = start_enterprise_authority(
            login.clone(),
            device_id.clone(),
            on_event.clone(),
            on_event,
        )
        .await?;
        let patch = desktop_enterprise_patch(
            &home,
            &authority,
            &login.actor,
            login.backend_url(),
            &device_id,
            plugin_names
                .iter()
                .any(|name| name == COLLABORATION_BUNDLE),
        )?;
        {
            let mut enterprise = self.enterprise.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(session) = enterprise
                .as_mut()
                .filter(|session| session.id == session_id)
            {
                session.authority = Some(authority);
                session.auth_file = Some(patch.auth_file.clone());
            }
        }
        let _ = root;
        let connection = DesktopConnection::Enterprise {
            portal_origin: login.backend_url().to_string(),
            organization_id: Some(login.actor.organization_id.clone()),
            principal_id: Some(login.actor.id.clone()),
        };
        self.show_loading("正在启动企业工作区")?;
        self.entry_surface.store(false, Ordering::SeqCst);
        self.start_runtime(
            &home,
            connection,
            std::slice::from_ref(&patch.patch),
            existing_home,
        )
        .await?;
        // verifyTimer = setInterval(..., 30_000)
        let token = CancellationToken::new();
        *self
            .verify_cancel
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(token.clone());
        let this = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(VERIFY_INTERVAL) => {}
                }
                if token.is_cancelled() {
                    break;
                }
                if this.verifying.load(Ordering::SeqCst) || this.switching.load(Ordering::SeqCst) {
                    continue;
                }
                this.verifying.store(true, Ordering::SeqCst);
                let outcome = async {
                    login.verify().await.map_err(|error| error.to_string())?;
                    let current = installed_enterprise_plugins(
                        &this.profile_directory(),
                        &version,
                    )
                    .map_err(|error| error)?;
                    if plugin_names
                        .iter()
                        .any(|name| !current.contains(name))
                    {
                        return Err("企业插件已卸载，请重新安装后登录".to_string());
                    }
                    Ok(())
                }
                .await;
                if outcome.is_err() {
                    this.error_box(
                        "企业认证不可用",
                        "企业账号已过期、被撤销、后台不可达或企业插件已卸载。本机企业运行将停止，请检查后重新登录。",
                    );
                    this.return_to_entry_spawned();
                }
                this.verifying.store(false, Ordering::SeqCst);
            }
        });
        Ok(())
    }

    /// quantdsh `installLoginHandlers` body for `quantdsh:enterprise-login`.
    pub async fn enterprise_login(self: &Arc<Self>, label: &str, input: &Value) -> Value {
        let current = self
            .window
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if !self.entry_surface.load(Ordering::SeqCst)
            || current.as_deref() != Some(label)
            || self.switching.load(Ordering::SeqCst)
            || self
                .pending_connection
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_none()
            || self
                .enterprise
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_some()
        {
            return json!({ "error": "此页面不能发起企业登录" });
        }
        self.switching.store(true, Ordering::SeqCst);
        let mut admitted: Option<Arc<EnterpriseLogin>> = None;
        let outcome = async {
            let account = input.get("account").and_then(Value::as_str);
            let password = input.get("password").and_then(Value::as_str);
            let (account, password) = match (account, password) {
                (Some(account), Some(password)) => (account, password),
                _ => return Err("请输入账号和密码".to_string()),
            };
            let portal = {
                let pending = self
                    .pending_connection
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                match pending {
                    Some(DesktopConnection::Enterprise { portal_origin, .. }) => portal_origin,
                    _ => return Err("请先输入企业后台地址".to_string()),
                }
            };
            let login = Arc::new(EnterpriseLogin::login(&portal, account, password).await?);
            admitted = Some(login.clone());
            let space = enterprise_space(&self.paths.user_data, login.backend_url(), &login.actor)?;
            let session_id = self.next_session_id.fetch_add(1, Ordering::SeqCst);
            *self.enterprise.lock().unwrap_or_else(|error| error.into_inner()) =
                Some(EnterpriseSession {
                    id: session_id,
                    login: login.clone(),
                    root: space.root,
                    home: space.home.clone(),
                    device_id: space.device_id,
                    authority: None,
                    auth_file: None,
                });
            let (_profile, existing_home) = self.prepare_profile(&space.home).await?;
            self.activate_enterprise(session_id, existing_home).await
        }
        .await;
        let result = match outcome {
            Ok(()) => json!({}),
            Err(message) => {
                let has_enterprise = self
                    .enterprise
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .is_some();
                if has_enterprise {
                    let _ = self.leave_workspace().await;
                    self.show_connection_entry();
                    self.error_box("无法进入企业", &message);
                } else if let Some(login) = admitted {
                    if !login.logout().await {
                        self.error_box(
                            "企业启动未完成",
                            "本机未进入企业运行；后台连接失败，远端登录撤销尚未确认。",
                        );
                    }
                }
                json!({ "error": message })
            }
        };
        self.end_switch();
        result
    }

    // ------------------------------------------------------------------ menu

    /// quantdsh `installConnectionMenu`.
    pub fn install_connection_menu(&self) {
        let app = &self.app;
        let workspace = MenuItem::with_id(app, "quantdsh-return", "切换使用方式／退出企业", true, None::<&str>)
            .expect("workspace menu item");
        let workspace_menu: Submenu<R> = Submenu::new(app, "工作区", true)
            .expect("workspace submenu");
        let _ = workspace_menu.append(&workspace);
        let tools_item = MenuItem::with_id(app, "quantdsh-command", "终端命令 dsh（可选）…", true, None::<&str>)
            .expect("tools menu item");
        let tools_menu: Submenu<R> =
            Submenu::new(app, "工具", true).expect("tools submenu");
        let _ = tools_menu.append(&tools_item);
        // Electron `viewMenu` role: reload, forceReload, toggleDevTools, separator,
        // resetZoom, zoomIn, zoomOut, separator, togglefullscreen.
        let reload_item = MenuItem::with_id(app, "quantdsh-reload", "重新加载", true, Some("CmdOrCtrl+R"))
            .expect("reload menu item");
        let force_reload_item = MenuItem::with_id(
            app,
            "quantdsh-force-reload",
            "强制重新加载",
            true,
            Some("CmdOrCtrl+Shift+R"),
        )
        .expect("force reload menu item");
        let devtools_item = MenuItem::with_id(
            app,
            "quantdsh-devtools",
            "开发者工具",
            true,
            Some("CmdOrCtrl+Shift+I"),
        )
        .expect("devtools menu item");
        let reset_zoom_item = MenuItem::with_id(app, "quantdsh-zoom-reset", "实际大小", true, Some("CmdOrCtrl+0"))
            .expect("reset zoom menu item");
        let zoom_in_item = MenuItem::with_id(app, "quantdsh-zoom-in", "放大", true, Some("CmdOrCtrl+="))
            .expect("zoom in menu item");
        let zoom_out_item = MenuItem::with_id(app, "quantdsh-zoom-out", "缩小", true, Some("CmdOrCtrl+-"))
            .expect("zoom out menu item");
        let view_menu: Submenu<R> =
            Submenu::new(app, "视图", true).expect("view submenu");
        let _ = view_menu.append(&reload_item);
        let _ = view_menu.append(&force_reload_item);
        let _ = view_menu.append(&devtools_item);
        let _ = view_menu.append(&PredefinedMenuItem::separator(app).expect("separator"));
        let _ = view_menu.append(&reset_zoom_item);
        let _ = view_menu.append(&zoom_in_item);
        let _ = view_menu.append(&zoom_out_item);
        let _ = view_menu.append(&PredefinedMenuItem::separator(app).expect("separator"));
        let _ = view_menu.append(&PredefinedMenuItem::fullscreen(app, None).expect("fullscreen"));
        let edit_menu: Submenu<R> =
            Submenu::new(app, "编辑", true).expect("edit submenu");
        let _ = edit_menu.append(&PredefinedMenuItem::undo(app, None).expect("undo"));
        let _ = edit_menu.append(&PredefinedMenuItem::redo(app, None).expect("redo"));
        let _ = edit_menu.append(&PredefinedMenuItem::separator(app).expect("separator"));
        let _ = edit_menu.append(&PredefinedMenuItem::cut(app, None).expect("cut"));
        let _ = edit_menu.append(&PredefinedMenuItem::copy(app, None).expect("copy"));
        let _ = edit_menu.append(&PredefinedMenuItem::paste(app, None).expect("paste"));
        let _ = edit_menu.append(&PredefinedMenuItem::select_all(app, None).expect("select all"));
        let window_menu: Submenu<R> =
            Submenu::new(app, "窗口", true).expect("window submenu");
        let _ = window_menu.append(&PredefinedMenuItem::minimize(app, None).expect("minimize"));
        let _ = window_menu.append(&PredefinedMenuItem::maximize(app, None).expect("maximize"));
        let _ = window_menu.append(&PredefinedMenuItem::separator(app).expect("separator"));
        let _ = window_menu.append(&PredefinedMenuItem::close_window(app, None).expect("close window"));
        #[allow(unused_mut)]
        let mut roots: Vec<&dyn tauri::menu::IsMenuItem<R>> =
            vec![&workspace_menu, &tools_menu, &edit_menu, &view_menu, &window_menu];
        #[cfg(target_os = "macos")]
        {
            let app_menu: Submenu<R> =
                Submenu::new(app, "QuantDSH", true).expect("app submenu");
            let _ = app_menu.append(&PredefinedMenuItem::about(app, Some("关于 QuantDSH"), None).expect("about"));
            let _ = app_menu.append(&PredefinedMenuItem::separator(app).expect("separator"));
            let _ = app_menu.append(&PredefinedMenuItem::services(app, None).expect("services"));
            let _ = app_menu.append(&PredefinedMenuItem::separator(app).expect("separator"));
            let _ = app_menu.append(&PredefinedMenuItem::hide(app, None).expect("hide"));
            let _ = app_menu.append(&PredefinedMenuItem::hide_others(app, None).expect("hide others"));
            let _ = app_menu.append(&PredefinedMenuItem::show_all(app, None).expect("show all"));
            let _ = app_menu.append(&PredefinedMenuItem::separator(app).expect("separator"));
            let _ = app_menu.append(&PredefinedMenuItem::quit(app, None).expect("quit"));
            roots.insert(0, &app_menu);
        }
        if let Ok(menu) = Menu::with_items(app, &roots) {
            let _ = menu.set_as_app_menu();
        }
    }

    /// Menu event dispatch for the carrier menu items.
    pub fn on_menu_event(self: &Arc<Self>, app: &AppHandle<R>, event: &MenuEvent) {
        match event.id().as_ref() {
            "quantdsh-return" => {
                self.return_to_entry_spawned();
            }
            "quantdsh-command" => {
                let scope = {
                    let enterprise = self.enterprise.lock().unwrap_or_else(|error| error.into_inner());
                    match enterprise.as_ref() {
                        Some(session) => format!(
                            "{} / {}",
                            session.login.actor.organization_name,
                            session.login.actor.display_name
                        ),
                        None => "个人工作区".to_string(),
                    }
                };
                manage_command(app, &self.paths.runtime_root, &scope);
            }
            "quantdsh-reload" | "quantdsh-force-reload" => {
                // WebView2 exposes no cache-bypassing reload; both entries perform the
                // same reload, which is what the loopback runtime serves anyway.
                if let Some(window) = self.current_window() {
                    let _ = window.reload();
                }
            }
            "quantdsh-zoom-reset" => self.zoom_to(1.0),
            "quantdsh-zoom-in" => {
                let current = *self.zoom.lock().unwrap_or_else(|error| error.into_inner());
                self.zoom_to(current * ZOOM_STEP);
            }
            "quantdsh-zoom-out" => {
                let current = *self.zoom.lock().unwrap_or_else(|error| error.into_inner());
                self.zoom_to(current / ZOOM_STEP);
            }
            "quantdsh-devtools" => {
                if let Some(window) = self.current_window() {
                    if window.is_devtools_open() {
                        window.close_devtools();
                    } else {
                        window.open_devtools();
                    }
                }
            }
            _ => {}
        }
    }

    fn current_window(&self) -> Option<WebviewWindow<R>> {
        let label = self
            .window
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()?;
        self.app.get_webview_window(&label)
    }

    /// Apply a zoom factor to the current window and remember it for the next step.
    fn zoom_to(&self, factor: f64) {
        let factor = factor.clamp(ZOOM_MIN, ZOOM_MAX);
        *self.zoom.lock().unwrap_or_else(|error| error.into_inner()) = factor;
        if let Some(window) = self.current_window() {
            set_window_zoom(&window, factor);
        }
    }

    // ---------------------------------------------------------------- startup

    /// quantdsh `app.whenReady()` body (minus the menu and handlers, which are
    /// installed by the Tauri builder).
    pub async fn start(self: &Arc<Self>) {
        let deployment_file = self.paths.deployment_config_file();
        if deployment_file.exists() {
            if let Ok(text) = std::fs::read_to_string(&deployment_file) {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    if let Ok(parsed) = parse_deployment_config(&value) {
                        if let Some(backend) = parsed
                            .get("enterprise")
                            .and_then(|enterprise| enterprise.get("backendUrl"))
                            .and_then(Value::as_str)
                        {
                            *self
                                .managed_backend
                                .lock()
                                .unwrap_or_else(|error| error.into_inner()) = Some(backend.to_string());
                            *self
                                .enterprise_portal
                                .lock()
                                .unwrap_or_else(|error| error.into_inner()) = backend.to_string();
                        }
                    }
                }
            }
        }
        if self
            .enterprise_portal
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty()
        {
            let backend_file = self.saved_backend_file();
            if backend_file.exists() {
                if let Ok(text) = std::fs::read_to_string(&backend_file) {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        if let Some(backend) = value.get("backend").and_then(Value::as_str) {
                            if let Ok(DesktopConnection::Enterprise { portal_origin, .. }) =
                                enterprise_connection(backend)
                            {
                                *self
                                    .enterprise_portal
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner()) = portal_origin;
                            }
                        }
                    }
                }
            }
        }
        self.install_connection_menu();
        // Debug-only shortcut for unattended smoke tests: the shipped app always
        // opens the usage-mode picker, which no script can operate (the entry page
        // is served by the `quantdsh://` protocol, not by the OS shell). Setting
        // `QUANTDSH_DESKTOP_DEBUG_AUTOSTART=personal` walks the same personal path
        // `quantdsh://personal` would, so a debug build can be exercised end to end.
        // `debug_assertions` keeps this out of release builds entirely.
        #[cfg(debug_assertions)]
        if self.paths.runtime_problems.is_empty()
            && std::env::var("QUANTDSH_DESKTOP_DEBUG_AUTOSTART").as_deref() == Ok("personal")
        {
            eprintln!("[quantdsh] debug autostart: entering the personal workspace");
            self.show_connection_entry();
            let this = Arc::clone(self);
            tauri::async_runtime::spawn(async move {
                let home = this.paths.dsh_home.clone();
                let _ = this.show_loading("正在启动个人工作区");
                match this.prepare_profile(&home).await {
                    Ok((_profile, existing_home)) => {
                        this.entry_surface.store(false, Ordering::SeqCst);
                        if let Err(error) = this
                            .start_runtime(&home, DesktopConnection::Personal, &[], existing_home)
                            .await
                        {
                            this.error_box("无法启动个人工作区", &error);
                        }
                    }
                    Err(error) => this.error_box("无法准备工作区", &error),
                }
            });
            return;
        }
        // Fail closed exactly like the Electron carrier — no host process is ever
        // started against a partial tree — but with a surface that can repair it.
        if self.paths.runtime_problems.is_empty() {
            self.show_connection_entry();
        } else {
            self.show_runtime_guide();
        }
    }
}

fn is_enterprise_window(connection: &DesktopConnection) -> bool {
    matches!(connection, DesktopConnection::Enterprise { .. })
}

/// Electron `webContents.setZoomFactor`. Windows uses the WebView2 controller's
/// zoom factor; elsewhere wry exposes no zoom API, so the layout root is scaled,
/// which is the closest equivalent the system webview offers.
fn set_window_zoom<R: Runtime>(window: &WebviewWindow<R>, factor: f64) {
    let factor = factor.clamp(ZOOM_MIN, ZOOM_MAX);
    #[cfg(windows)]
    {
        let _ = window.with_webview(move |webview| {
            unsafe {
                let _ = webview.controller().SetZoomFactor(factor);
            }
        });
    }
    #[cfg(not(windows))]
    {
        let _ = window.eval(&format!(
            "document.documentElement.style.zoom = '{factor}'"
        ));
    }
}

/// The registered `quantdsh://` protocol: carrier surfaces only.
pub fn quantdsh_protocol_handler<R: Runtime>(
    context: tauri::UriSchemeContext<'_, R>,
    request: http::Request<Vec<u8>>,
) -> http::Response<Vec<u8>> {
    let uri = request.uri().clone();
    let path = uri.path().trim_start_matches('/').to_string();
    let host = uri.host().unwrap_or_default().to_string();
    let carrier = context
        .app_handle()
        .try_state::<Arc<Carrier<R>>>()
        .map(|carrier| carrier.inner().clone());
    let html = match (host.as_str(), path.as_str()) {
        ("localhost", "entry") => {
            let (portal, managed, available) = match &carrier {
                Some(carrier) => (
                    carrier
                        .enterprise_portal
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .clone(),
                    carrier
                        .managed_backend
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .is_some(),
                    carrier.enterprise_plugin_available(),
                ),
                None => (String::new(), false, false),
            };
            connection_entry_html(&portal, managed, available)
        }
        ("localhost", "login") => {
            let portal = match &carrier {
                Some(carrier) => carrier
                    .enterprise_portal
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone(),
                None => String::new(),
            };
            enterprise_login_html(&portal)
        }
        ("localhost", "loading") => {
            let message = uri
                .query()
                .and_then(|query| {
                    query
                        .split('&')
                        .find_map(|pair| pair.strip_prefix("message="))
                })
                .map(percent_decode)
                .unwrap_or_default();
            connection_loading_html(&message)
        }
        ("localhost", "runtime") => match &carrier {
            Some(carrier) => runtime_missing_html(
                carrier.paths.runtime_source.label(),
                &carrier.paths.runtime_root.display().to_string(),
                &carrier.paths.runtime_problems,
                crate::runtime_pack::download_url().as_deref(),
                &carrier.paths.runtime_install_root.display().to_string(),
            ),
            None => String::new(),
        },
        _ => String::new(),
    };
    if html.is_empty() {
        return http::Response::builder()
            .status(http::StatusCode::NOT_FOUND)
            .header("content-type", "text/plain; charset=utf-8")
            .header("cache-control", "no-store")
            .body(b"Not Found".to_vec())
            .expect("not found response");
    }
    http::Response::builder()
        .status(http::StatusCode::OK)
        .header("content-type", "text/html; charset=utf-8")
        .header("cache-control", "no-store")
        .body(html.into_bytes())
        .expect("html response")
}

/// Build the carrier for the primary application instance.
pub fn build_carrier<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<Carrier<R>>, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| error.to_string())?;
    let user_data = crate::paths::user_data_dir()?;
    let paths = RuntimePaths::resolve(&resource_dir, &user_data)?;
    Ok(Arc::new(Carrier::new(app.clone(), paths)))
}

/// Entry point for both modes: dispatch to the hidden browser worker or the app.
pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    let worker = match browser_worker_request(&args) {
        Ok(worker) => worker,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    if let Some(request) = worker {
        let result = tauri::Builder::default()
            .plugin(tauri_plugin_dialog::init())
            .plugin(tauri_plugin_opener::init())
            .register_uri_scheme_protocol("quantdsh", quantdsh_protocol_handler::<tauri::Wry>)
            .invoke_handler(tauri::generate_handler![
                crate::app::quantdsh_navigate,
                crate::app::quantdsh_enterprise_login
            ])
            .setup(move |app| {
                start_browser_worker(app, &request)?;
                Ok(())
            })
            .run(tauri::generate_context!());
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    tauri::Builder::default()
        // Registered first: a second instance must be detected before the rest of setup runs.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // app.on('second-instance')
            // The Windows installer asks a running instance to quit through this
            // channel (`QuantDSH.exe --dsh-installer-quit`), so an upgrade can
            // replace files only after an orderly shutdown; see
            // `src-tauri/installer-hooks.nsh` and quantdsh's `build/installer.nsh`.
            if args.iter().any(|argument| argument == "--dsh-installer-quit") {
                if let Some(carrier) = app.try_state::<Arc<Carrier<tauri::Wry>>>() {
                    let carrier = carrier.inner().clone();
                    if carrier.quitting.load(Ordering::SeqCst) {
                        carrier.stop_runtime();
                    } else {
                        carrier.quit_now();
                    }
                } else {
                    app.exit(0);
                }
                return;
            }
            if let Some(carrier) = app.try_state::<Arc<Carrier<tauri::Wry>>>() {
                let label = carrier
                    .window
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                if let Some(label) = label {
                    if let Some(window) = app.get_webview_window(&label) {
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                }
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_uri_scheme_protocol("quantdsh", quantdsh_protocol_handler::<tauri::Wry>)
        .invoke_handler(tauri::generate_handler![
            crate::app::quantdsh_navigate,
            crate::app::quantdsh_enterprise_login,
            crate::app::quantdsh_runtime_import,
            crate::app::quantdsh_runtime_open_download,
            crate::app::quantdsh_runtime_restart,
            crate::app::quantdsh_runtime_exit
        ])
        .setup(|app| {
            // Electron parity: the bundle declares `LSUIElement` (agent) so a
            // spawned `--quantdsh-browser-worker-*` child — the same executable —
            // cannot flash a Dock icon before JavaScript runs; the primary
            // process then calls `app.setActivationPolicy('regular')`. Tauri has
            // no extendInfo, so `scripts/patch-macos-info-plist.mjs` writes
            // LSUIElement after bundling and this line restores the Dock icon.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Regular);
            let carrier = build_carrier(app.handle())?;
            app.manage(carrier.clone());
            let started = carrier.clone();
            tauri::async_runtime::spawn(async move {
                started.start().await;
            });
            Ok(())
        })
        .on_menu_event(|app, event| {
            if let Some(carrier) = app.try_state::<Arc<Carrier<tauri::Wry>>>() {
                carrier.on_menu_event(app, &event);
            }
        })
        .build(tauri::generate_context!())
        .expect("tauri application")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                let Some(carrier) = app.try_state::<Arc<Carrier<tauri::Wry>>>() else {
                    return;
                };
                let carrier = carrier.inner().clone();
                // app.on('before-quit')
                if carrier.quitting.load(Ordering::SeqCst) {
                    carrier.stop_runtime();
                    return;
                }
                api.prevent_exit();
                // Two Electron events land here: `before-quit` (the user asked to
                // quit) and `window-all-closed`. They differ while a switch is in
                // flight — the former remembers the request and cancels the
                // installer, the latter does nothing and lets the switch finish.
                let windowless = app.webview_windows().is_empty();
                if carrier.switching.load(Ordering::SeqCst) {
                    if !windowless {
                        carrier.quit_requested.store(true, Ordering::SeqCst);
                        let installer = *carrier
                            .installer
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        if let Some(pid) = installer {
                            tauri::async_runtime::spawn(async move {
                                if stop_owned_process(pid).await.is_err() {
                                    eprintln!("进程清理失败：插件安装进程停止尚未确认，请稍后检查。");
                                }
                            });
                        }
                    }
                    return;
                }
                carrier.quit_now();
            }
            tauri::RunEvent::Exit => {
                if let Some(carrier) = app.try_state::<Arc<Carrier<tauri::Wry>>>() {
                    carrier.inner().stop_runtime();
                }
            }
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { has_visible_windows: false } => {
                // app.on('activate')
                let Some(carrier) = app.try_state::<Arc<Carrier<tauri::Wry>>>() else {
                    return;
                };
                let carrier = carrier.inner().clone();
                if carrier.switching.load(Ordering::SeqCst)
                    || carrier.runtime_starting.load(Ordering::SeqCst)
                    || carrier
                        .installer
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .is_some()
                    || carrier.quitting.load(Ordering::SeqCst)
                    || carrier.quit_requested.load(Ordering::SeqCst)
                {
                    return;
                }
                if carrier
                    .window
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .is_none()
                {
                    carrier.app.restart();
                }
            }
            _ => {}
        });
}

/// The `quantdsh_navigate` command: user-gesture carrier navigation.
#[tauri::command]
async fn quantdsh_navigate(
    webview: WebviewWindow<tauri::Wry>,
    app: AppHandle<tauri::Wry>,
    url: String,
    surface: String,
) -> Result<Value, String> {
    let _ = surface;
    let carrier = app
        .try_state::<Arc<Carrier<tauri::Wry>>>()
        .ok_or_else(|| "carrier unavailable".to_string())?
        .inner()
        .clone();
    carrier
        .handle_navigation(webview.label(), &url)
        .await;
    Ok(json!({}))
}

/// The `quantdsh_enterprise_login` command: enterprise admission from the login page.
#[tauri::command]
async fn quantdsh_enterprise_login(
    webview: WebviewWindow<tauri::Wry>,
    app: AppHandle<tauri::Wry>,
    input: Value,
) -> Result<Value, String> {
    let carrier = app
        .try_state::<Arc<Carrier<tauri::Wry>>>()
        .ok_or_else(|| "carrier unavailable".to_string())?
        .inner()
        .clone();
    Ok(carrier.enterprise_login(webview.label(), &input).await)
}

/// Pick and install an offline runtime archive.
///
/// The dialog and the extraction both run off the main thread: a blocking file
/// dialog on the UI thread deadlocks, and hashing hundreds of megabytes would
/// freeze the window. Nothing is written outside `<userData>/runtime`.
#[tauri::command]
async fn quantdsh_runtime_import(app: AppHandle<tauri::Wry>) -> Result<Value, String> {
    let install_root = app
        .try_state::<Arc<Carrier<tauri::Wry>>>()
        .ok_or_else(|| "carrier unavailable".to_string())?
        .inner()
        .clone()
        .paths
        .runtime_install_root
        .clone();
    let picker = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        picker
            .dialog()
            .file()
            .set_title("选择 QuantDSH 运行时离线包")
            .add_filter("QuantDSH 运行时离线包", &["zst"])
            .blocking_pick_file()
    })
    .await
    .map_err(|error| error.to_string())?;
    let Some(picked) = picked else {
        return Ok(json!({ "cancelled": true }));
    };
    let archive = picked
        .into_path()
        .map_err(|_| "无法读取所选文件".to_string())?;
    let imported = tauri::async_runtime::spawn_blocking(move || {
        crate::runtime_pack::import_archive(&archive, &install_root)
    })
    .await
    .map_err(|error| error.to_string())?;
    match imported {
        Ok(report) => Ok(json!({
            "ok": true,
            "version": report.version,
            "digest": report.digest,
            "root": report.root.display().to_string(),
        })),
        Err(error) => Ok(json!({ "error": error })),
    }
}

#[tauri::command]
async fn quantdsh_runtime_open_download(app: AppHandle<tauri::Wry>) -> Result<Value, String> {
    let Some(url) = crate::runtime_pack::download_url() else {
        return Ok(json!({
            "error": "本安装包未配置运行时下载地址，请联系管理员获取离线包。"
        }));
    };
    match app.opener().open_url(url, None::<String>) {
        Ok(()) => Ok(json!({ "ok": true })),
        Err(error) => Ok(json!({ "error": format!("无法打开下载页：{error}") })),
    }
}

/// Re-exec so the newly promoted pack is resolved from scratch. Hot-swapping the
/// runtime under a live process tree is exactly the half-state this design avoids.
#[tauri::command]
async fn quantdsh_runtime_restart(app: AppHandle<tauri::Wry>) -> Result<Value, String> {
    if let Some(carrier) = app.try_state::<Arc<Carrier<tauri::Wry>>>() {
        carrier.inner().stop_runtime();
    }
    // `AppHandle::restart` re-execs this executable and never returns.
    app.restart()
}

#[tauri::command]
async fn quantdsh_runtime_exit(app: AppHandle<tauri::Wry>) -> Result<Value, String> {
    if let Some(carrier) = app.try_state::<Arc<Carrier<tauri::Wry>>>() {
        carrier.inner().stop_runtime();
    }
    app.exit(0);
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carrier_urls_use_the_platform_form() {
        assert_eq!(entry_url().as_str(), carrier_url("entry", None).as_str());
        let loading = loading_url("正在启动个人工作区");
        assert!(loading.contains("loading"));
        assert!(loading.contains("message="));
    }

    #[test]
    fn percent_round_trip() {
        let encoded = percent_encode("正在启动个人工作区");
        assert!(!encoded.contains("正"));
        assert_eq!(percent_decode(&encoded), "正在启动个人工作区");
    }

    #[test]
    fn tail_buffer_keeps_the_newest_utf16_units() {
        let mut buffer = String::new();
        tail_limit_append(&mut buffer, &"错".repeat(OUTPUT_LIMIT + 10));
        // A JavaScript string counts UTF-16 units, so 16,384 CJK characters fit
        // even though they occupy three bytes each.
        assert_eq!(buffer.encode_utf16().count(), OUTPUT_LIMIT);
        assert!(buffer.chars().all(|character| character == '错'));
    }

    #[test]
    fn tail_buffer_keeps_the_junction_when_the_chunk_is_short() {
        let mut buffer = "a".repeat(OUTPUT_LIMIT - 2);
        tail_limit_append(&mut buffer, "bcd");
        // `${output}${text}`.slice(-16_384) keeps the tail of the *pair*, not just
        // the newest chunk.
        assert_eq!(buffer.encode_utf16().count(), OUTPUT_LIMIT);
        assert!(buffer.starts_with('a'));
        assert!(buffer.ends_with("abcd"));
    }

    #[test]
    fn tail_buffer_never_splits_a_surrogate_pair() {
        let mut buffer = String::new();
        tail_limit_append(&mut buffer, &"🦀".repeat(OUTPUT_LIMIT));
        assert!(buffer.encode_utf16().count() <= OUTPUT_LIMIT);
        assert!(buffer.chars().all(|character| character == '🦀'));
    }

    #[test]
    fn normalizes_the_windows_workaround_form() {
        let url: tauri::Url = "http://quantdsh.localhost/loading?message=x".parse().unwrap();
        let normalized = normalized_quantdsh_url(&url).expect("normalized");
        assert_eq!(normalized.scheme(), "quantdsh");
        assert_eq!(normalized.path(), "/loading");
        let url: tauri::Url = "https://example.com/".parse().unwrap();
        assert!(normalized_quantdsh_url(&url).is_none());
    }

    #[test]
    fn carrier_navigation_admits_only_carrier_targets() {
        let entry: tauri::Url = "quantdsh://localhost/entry".parse().unwrap();
        assert!(carrier_navigation_allowed(&entry));
        let workaround: tauri::Url = "http://quantdsh.localhost/loading?message=x".parse().unwrap();
        assert!(carrier_navigation_allowed(&workaround));
        let external: tauri::Url = "https://example.com/".parse().unwrap();
        assert!(!carrier_navigation_allowed(&external));
        // Command targets are dispatched, never loaded as documents.
        let command: tauri::Url = "quantdsh://personal".parse().unwrap();
        assert!(!carrier_navigation_allowed(&command));
    }

    #[test]
    fn entry_navigation_accepts_both_spellings() {
        for target in ["quantdsh://entry", "quantdsh://entry/", "quantdsh://localhost/entry"] {
            let url: tauri::Url = target.parse().unwrap();
            assert!(is_entry_navigation(&url), "{target}");
        }
        for target in ["quantdsh://personal", "quantdsh://localhost/login", "https://example.com/entry"] {
            let url: tauri::Url = target.parse().unwrap();
            assert!(!is_entry_navigation(&url), "{target}");
        }
        // The Windows workaround form normalizes to the carrier spelling first.
        let workaround: tauri::Url = "http://quantdsh.localhost/entry".parse().unwrap();
        let normalized = normalized_quantdsh_url(&workaround).expect("normalized");
        assert!(is_entry_navigation(&normalized));
    }
}
