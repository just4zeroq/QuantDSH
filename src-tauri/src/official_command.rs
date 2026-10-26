//! Official command execution with bounded diagnostics, mirroring
//! the upstream desktop's official-command.ts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::owned_process::{clean_owned_process_tree, stop_owned_process, track_owned_process};
use crate::runtime_diagnostics::{
    format_iso8601, official_command_failure_detail, RuntimeOutputRedactor,
};

const OUTPUT_LIMIT: usize = 16_384;
const DEFAULT_TIMEOUT_MS: u64 = 240_000;

pub struct OfficialCommandOptions {
    pub executable: PathBuf,
    pub launcher: PathBuf,
    pub args: Vec<String>,
    pub home: PathBuf,
    pub env: HashMap<String, String>,
    pub log_directory: PathBuf,
    pub timeout_ms: Option<u64>,
    pub cancelled: Box<dyn Fn() -> bool + Send + Sync>,
    pub on_process: Box<dyn Fn(Option<u32>) + Send + Sync>,
}

struct Shared {
    output: Mutex<String>,
    cause: Mutex<Option<String>>,
}

fn record_output(shared: &Shared, text: &str) {
    let mut output = shared.output.lock().unwrap_or_else(|error| error.into_inner());
    // `${output}${text}`.slice(-16_384): the junction is kept, and the cut never
    // splits a character (Host output is routinely Chinese).
    output.push_str(text);
    crate::runtime_diagnostics::keep_tail_utf16(&mut output, OUTPUT_LIMIT);
}

fn set_cause(shared: &Shared, cause: &str) {
    let mut guard = shared.cause.lock().unwrap_or_else(|error| error.into_inner());
    if guard.is_none() {
        *guard = Some(cause.to_string());
    }
}

fn append_cause(shared: &Shared, suffix: &str) {
    let mut guard = shared.cause.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(cause) = guard.as_mut() {
        cause.push_str(suffix);
    }
}

/// Preserve the actual CLI failure before returning control to the entry screen.
pub async fn run_official_command(options: OfficialCommandOptions) -> Result<(), String> {
    let started_at = SystemTime::now();
    let mut command = tokio::process::Command::new(&options.executable);
    command.arg(&options.launcher).args(&options.args);
    command.current_dir(&options.home);
    command.env_clear();
    for (name, value) in &options.env {
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
            let detail = official_command_failure_detail(None, "", Some(&format!("无法启动插件安装工具：{error}")));
            return Err(write_failure(&options.log_directory, &detail, ""));
        }
    };
    let pid = child.id().unwrap_or(0);
    track_owned_process(pid, started_at);
    (options.on_process)(Some(pid));

    let shared = Arc::new(Shared { output: Mutex::new(String::new()), cause: Mutex::new(None) });
    let cleanup_confirmed = Arc::new(AtomicBool::new(false));

    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let shared = shared.clone();
        readers.push(tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut reader = stdout;
            let mut redactor = RuntimeOutputRedactor::new();
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(count) => redactor.push(&mut |line| record_output(&shared, line), &chunk[..count]),
                    Err(_) => break,
                }
            }
            redactor.end(&mut |line| record_output(&shared, line));
        }));
    }
    if let Some(stderr) = child.stderr.take() {
        let shared = shared.clone();
        readers.push(tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut reader = stderr;
            let mut redactor = RuntimeOutputRedactor::new();
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(count) => redactor.push(&mut |line| record_output(&shared, line), &chunk[..count]),
                    Err(_) => break,
                }
            }
            redactor.end(&mut |line| record_output(&shared, line));
        }));
    }
    // Node's `close` fires once the process exited and both stdio pipes reached EOF.
    let closed = tokio::spawn(async move {
        for reader in readers {
            let _ = reader.await;
        }
    });

    let timeout = Duration::from_millis(options.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
    let mut timed_out = false;
    let status = tokio::select! {
        status = child.wait() => {
            match status {
                Ok(status) => {
                    // `exit`: kill only our descendants promptly.
                    match clean_owned_process_tree(pid).await {
                        Ok(()) => cleanup_confirmed.store(true, Ordering::SeqCst),
                        Err(_) => set_cause(&shared, "本机安装进程停止尚未确认"),
                    }
                    Some(status)
                }
                Err(error) => {
                    set_cause(&shared, &format!("无法启动插件安装工具：{error}"));
                    None
                }
            }
        }
        _ = tokio::time::sleep(timeout) => {
            timed_out = true;
            None
        }
    };
    let code = if timed_out {
        set_cause(&shared, "插件依赖更新超时，请检查网络后重试");
        match stop_owned_process(pid).await {
            Ok(()) => cleanup_confirmed.store(true, Ordering::SeqCst),
            Err(_) => append_cause(&shared, "；本机安装进程停止尚未确认"),
        }
        let _ = closed.await;
        child.wait().await.ok().and_then(|status| status.code())
    } else {
        let _ = closed.await;
        status.and_then(|status| status.code())
    };
    if cleanup_confirmed.load(Ordering::SeqCst) {
        (options.on_process)(None);
    }
    let cancelled = (options.cancelled)();
    let cause = shared.cause.lock().unwrap_or_else(|error| error.into_inner()).clone();
    if cause.is_none() && !cancelled && code == Some(0) {
        return Ok(());
    }
    let output = shared.output.lock().unwrap_or_else(|error| error.into_inner()).clone();
    let detail = official_command_failure_detail(code, &output, if cancelled { Some("插件安装已取消") } else { cause.as_deref() });
    Err(write_failure(&options.log_directory, &detail, &output))
}

fn write_failure(log_directory: &std::path::Path, detail: &str, output: &str) -> String {
    let stamp = format_iso8601(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or_default(),
    );
    let text = format!("{stamp}\n{detail}\n\n最近的工具输出：\n{output}\n");
    let path = log_directory.join("plugin-operation-failure.log");
    if std::fs::create_dir_all(log_directory).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true).mode(0o600);
            if options.open(&path).and_then(|mut file| {
                use std::io::Write;
                file.write_all(text.as_bytes())
            }).is_ok() {
                return format!("{detail}\n\n错误记录：{}", path.display());
            }
        }
        #[cfg(not(unix))]
        {
            if std::fs::write(&path, text).is_ok() {
                return format!("{detail}\n\n错误记录：{}", path.display());
            }
        }
    }
    // Keep the real cause even if its local record cannot be written.
    detail.to_string()
}
