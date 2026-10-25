//! Hidden browser worker mode, mirroring `startBrowserWorker` from workdsh-main.ts.
//!
//! The browser-session provider spawns this executable again with
//! `--workdsh-browser-worker-port=<port> --workdsh-browser-worker-profile=<dir>`
//! and drives the hidden page over the Chrome DevTools Protocol.
//!
//! Platform notes:
//! - Windows/WebView2: `additional_browser_args` enables the CDP endpoint
//!   (`--remote-debugging-port`), exactly like Electron's `remote-debugging-port`
//!   Chromium switch. The worker gets its own WebView2 user data folder (the
//!   requested profile), which isolates it from the primary application windows.
//! - macOS/WKWebView: no CDP TCP endpoint exists; the hidden window is still
//!   created and the ready line printed, matching the Electron worker. The
//!   provider falls back to its managed-Chromium path on this platform.
//!   WKWebView data stores are identified by UUID rather than path, so the
//!   requested profile directory is created but the store uses a derived
//!   identifier (still ephemeral and isolated).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::webview::PageLoadEvent;
use tauri::{webview::WebviewWindowBuilder, Manager, Runtime, WebviewUrl, WindowEvent};

pub struct BrowserWorkerRequest {
    pub port: u16,
    pub profile: PathBuf,
}

/// workdsh-main.ts `browserWorkerRequest`.
pub fn browser_worker_request(args: &[String]) -> Result<Option<BrowserWorkerRequest>, String> {
    let port_arg = args.iter().find(|arg| arg.starts_with("--workdsh-browser-worker-port="));
    let Some(port_arg) = port_arg else { return Ok(None) };
    let port: i64 = port_arg["--workdsh-browser-worker-port=".len()..]
        .parse()
        .map_err(|_| "Invalid WorkDSH browser worker arguments".to_string())?;
    let profile = args
        .iter()
        .find(|arg| arg.starts_with("--workdsh-browser-worker-profile="))
        .map(|arg| PathBuf::from(&arg["--workdsh-browser-worker-profile=".len()..]));
    let profile = profile.ok_or_else(|| "Invalid WorkDSH browser worker arguments".to_string())?;
    if !(1024..=65535).contains(&port) || !profile.is_absolute() || !profile.exists() {
        return Err("Invalid WorkDSH browser worker arguments".to_string());
    }
    Ok(Some(BrowserWorkerRequest { port: port as u16, profile }))
}

fn announce_ready(announced: &AtomicBool) {
    if announced.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        use std::io::Write;
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(b"WORKDSH_BROWSER_WORKER_READY\n");
        let _ = stdout.flush();
    }
}

/// workdsh-main.ts `startBrowserWorker`.
pub fn start_browser_worker<R: Runtime, M: Manager<R>>(
    manager: &M,
    request: &BrowserWorkerRequest,
) -> Result<(), String> {
    // Accessory permits the hidden browser window without a Dock icon or menu bar.
    // Set before the window exists, like Electron's comment explains.
    #[cfg(target_os = "macos")]
    {
        let _ = manager.app_handle().set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
    let handle = manager.app_handle().clone();
    let url: url::Url = "about:blank".parse().expect("about:blank is a valid URL");
    let mut builder = WebviewWindowBuilder::new(&handle, "workdsh-browser-worker", WebviewUrl::External(url))
        .title("QuantDSH Browser Worker")
        .inner_size(1280.0, 800.0)
        .visible(false)
        .focused(false)
        .devtools(false)
        .zoom_hotkeys_enabled(false)
        // The worker page is offscreen: never throttle its timers.
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        // Keep the hidden worker out of the taskbar and app switcher.
        .skip_taskbar(true);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        builder = builder.data_directory(request.profile.clone());
    }
    #[cfg(target_os = "macos")]
    {
        // WKWebView identifies data stores by UUID; derive a stable one from the
        // *whole* profile path. Truncating it would let two profiles that share a
        // prefix collide on the same store.
        let digest = sha2::Sha256::digest(request.profile.to_string_lossy().as_bytes());
        let mut identifier = [0u8; 16];
        identifier.copy_from_slice(&digest[..16]);
        builder = builder.data_store_identifier(identifier);
        let _ = std::fs::create_dir_all(&request.profile);
    }
    #[cfg(target_os = "windows")]
    {
        // wry replaces the default feature block when arguments are supplied.
        // `remote-debugging-address` mirrors Electron's explicit loopback bind.
        // The three `disable-background*` switches are the Chromium spelling of
        // `webPreferences.backgroundThrottling: false`: Tauri's `background_throttling`
        // only reaches WKWebView, so on WebView2 the hidden worker would otherwise be
        // throttled exactly like the page Electron keeps awake.
        let args = format!(
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows --remote-debugging-address=127.0.0.1 --remote-debugging-port={}",
            request.port
        );
        builder = builder.additional_browser_args(&args);
    }
    let announced = Arc::new(AtomicBool::new(false));
    let announced_load = announced.clone();
    // Page-load events are registered on the builder: `WebviewWindow` itself has
    // no `on_page_load` accessor in Tauri v2.
    builder = builder.on_page_load(move |_window, payload| {
        if matches!(payload.event(), PageLoadEvent::Finished) {
            announce_ready(&announced_load);
        }
    });
    let window = builder.build().map_err(|error| error.to_string())?;
    window.on_window_event(move |event| match event {
        WindowEvent::Destroyed => {
            // The provider owns the worker lifetime; a destroyed window ends the process.
            std::process::exit(0);
        }
        _ => {}
    });
    // Some platforms may not deliver a page-load event for about:blank; the
    // provider polls the CDP endpoint, so a late announcement keeps diagnostics
    // faithful without affecting the handshake.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(10));
        announce_ready(&announced);
    });
    Ok(())
}
