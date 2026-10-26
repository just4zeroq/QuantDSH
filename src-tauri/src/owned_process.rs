//! Stop only the process tree created by this carrier. A parent exit is not a tree exit.
//! Mirrors the upstream desktop's owned-process.ts. On Windows the carrier
//! additionally assigns each spawned root to a kill-on-close Job Object, which
//! tracks the whole descendant tree by handle (immune to PID reuse); a
//! creation-time rechecked `taskkill /T /F` remains the fallback.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

const CLEANUP_DEADLINE: Duration = Duration::from_secs(12);
// Only the graceful stop path (non-Windows) waits; Windows stops the Job Object.
#[cfg(not(windows))]
const GRACEFUL_WINDOW: Duration = Duration::from_secs(8);
#[cfg(not(windows))]
const OVERALL_TIMEOUT: Duration = Duration::from_secs(12);

struct Ownership {
    /// Mirrors `Ownership.pid` in the Electron carrier; the registry is keyed by
    /// the same value, so this copy is only read by the Unix tree clean.
    #[allow(dead_code)]
    pid: u32,
    started_at: SystemTime,
    registered_at: SystemTime,
    /// Written on the child's exit event. Windows kills the Job Object by handle
    /// and never needs the timestamp, but it is recorded on every platform so the
    /// ownership record stays identical to the Electron one.
    #[allow(dead_code)]
    exited_at: Mutex<Option<SystemTime>>,
    #[cfg(windows)]
    job: Mutex<Option<win32::JobHandle>>,
    cleanup: Mutex<Option<Arc<CleanupCell>>>,
}

struct CleanupCell {
    cell: tokio::sync::OnceCell<Result<(), String>>,
}

static REGISTRY: OnceLock<Mutex<HashMap<u32, Arc<Ownership>>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<u32, Arc<Ownership>>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Call immediately after spawn, with the time captured immediately before it.
pub fn track_owned_process(pid: u32, started_at: SystemTime) {
    if pid == 0 {
        return;
    }
    let registered_at = SystemTime::now();
    let ownership = Arc::new(Ownership {
        pid,
        started_at,
        registered_at,
        exited_at: Mutex::new(None),
        #[cfg(windows)]
        job: Mutex::new(None),
        cleanup: Mutex::new(None),
    });
    #[cfg(windows)]
    {
        // Assign the root to a kill-on-close Job Object so every descendant dies with it.
        if let Some(handle) = win32::assign_to_kill_job(pid, started_at, registered_at) {
            *ownership.job.lock().unwrap_or_else(|error| error.into_inner()) = Some(handle);
        }
    }
    registry()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(pid, ownership);
}

/// `child.once('exit')` in Electron's `trackOwnedProcess`.
pub fn note_child_exit(pid: u32) {
    if let Some(ownership) = ownership_of(pid) {
        *ownership.exited_at.lock().unwrap_or_else(|error| error.into_inner()) = Some(SystemTime::now());
    }
}

fn ownership_of(pid: u32) -> Option<Arc<Ownership>> {
    registry().lock().unwrap_or_else(|error| error.into_inner()).get(&pid).cloned()
}

/// `child.exitCode !== null` on the Electron side; the Unix graceful stop reads it.
#[cfg(not(windows))]
fn exited(pid: u32) -> bool {
    ownership_of(pid)
        .and_then(|ownership| ownership.exited_at.lock().unwrap_or_else(|error| error.into_inner()).clone())
        .is_some()
}

fn cleanup_cell(pid: u32) -> Arc<CleanupCell> {
    let Some(ownership) = ownership_of(pid) else {
        return Arc::new(CleanupCell { cell: tokio::sync::OnceCell::new() });
    };
    let mut cleanup = ownership.cleanup.lock().unwrap_or_else(|error| error.into_inner());
    cleanup
        .get_or_insert_with(|| Arc::new(CleanupCell { cell: tokio::sync::OnceCell::new() }))
        .clone()
}

fn release_cleanup_cell(pid: u32) {
    if let Some(ownership) = ownership_of(pid) {
        *ownership.cleanup.lock().unwrap_or_else(|error| error.into_inner()) = None;
    }
}

async fn perform_clean(pid: u32) -> Result<(), String> {
    #[cfg(windows)]
    {
        win32::clean_windows_tree(pid).await
    }
    #[cfg(not(windows))]
    {
        // A detached spawn made the child its own group leader; the group is the tree.
        match unsafe { libc::kill(-(pid as i32), libc::SIGKILL) } {
            0 => Ok(()),
            error if error == libc::ESRCH => Ok(()),
            error => Err(format!("kill(-{pid}) failed: {error}")),
        }
    }
}

pub async fn clean_owned_process_tree(pid: u32) -> Result<(), String> {
    if pid == 0 {
        return Ok(());
    }
    let cell = cleanup_cell(pid);
    // The cell stores the clean outcome itself, so a failed clean is retained
    // (and reported) until the caller releases it for a retry.
    match cell.cell.get_or_try_init(|| async { Ok::<_, String>(perform_clean(pid).await) }).await {
        Ok(result) => {
            let outcome = result.clone();
            // A failed clean may be retried, like the Electron carrier.
            if outcome.is_err() {
                release_cleanup_cell(pid);
            }
            outcome
        }
        Err(error) => {
            release_cleanup_cell(pid);
            Err(error)
        }
    }
}

pub async fn stop_owned_process(pid: u32) -> Result<(), String> {
    #[cfg(windows)]
    {
        return clean_owned_process_tree(pid).await;
    }
    #[cfg(not(windows))]
    {
        if exited(pid) {
            return clean_owned_process_tree(pid).await;
        }
        // Signal the whole group first; a group leader exit is not a tree exit.
        let signal = unsafe { libc::kill(-(pid as i32), libc::SIGTERM) };
        if signal != 0 && signal != libc::ESRCH {
            return Err("本机进程尚未停止".to_string());
        }
        let force = tokio::time::sleep(GRACEFUL_WINDOW);
        let timeout = tokio::time::sleep(OVERALL_TIMEOUT);
        tokio::pin!(force, timeout);
        let mut poll = Box::pin(tokio::time::sleep(Duration::from_millis(50)));
        loop {
            tokio::select! {
                _ = &mut poll => {
                    if exited(pid) {
                        break;
                    }
                    poll = Box::pin(tokio::time::sleep(Duration::from_millis(50)));
                }
                _ = &mut force => {
                    // The graceful window elapsed without exit: force the tree.
                    let _ = clean_owned_process_tree(pid).await;
                    break;
                }
                _ = &mut timeout => return Err("本机进程尚未停止".to_string()),
            }
        }
        clean_owned_process_tree(pid).await
    }
}

#[cfg(windows)]
mod win32 {
    use super::*;

    /// A job handle is a raw kernel handle; ownership is shared so the tree clean
    /// can snapshot it without holding the registry lock across an await, and the
    /// handle is closed exactly once, when the last owner drops.
    #[derive(Clone)]
    pub struct JobHandle(Arc<JobHandleInner>);

    struct JobHandleInner(windows::Win32::Foundation::HANDLE);

    unsafe impl Send for JobHandle {}
    unsafe impl Sync for JobHandle {}

    impl JobHandle {
        fn raw(&self) -> windows::Win32::Foundation::HANDLE {
            self.0 .0
        }
    }

    impl Drop for JobHandleInner {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                unsafe {
                    let _ = windows::Win32::Foundation::CloseHandle(self.0);
                }
            }
        }
    }

    fn filetime_to_unix_ms(time: &windows::Win32::Foundation::FILETIME) -> u64 {
        let value = ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64;
        // 100ns intervals since 1601-01-01 → unix milliseconds
        value / 10_000 - 11_644_473_600_000
    }

    fn process_creation_time(process: windows::Win32::Foundation::HANDLE) -> Option<u64> {
        let mut creation = windows::Win32::Foundation::FILETIME::default();
        let mut exit = windows::Win32::Foundation::FILETIME::default();
        let mut kernel = windows::Win32::Foundation::FILETIME::default();
        let mut user = windows::Win32::Foundation::FILETIME::default();
        let ok = unsafe {
            windows::Win32::System::Threading::GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user)
        };
        ok.ok().map(|()| filetime_to_unix_ms(&creation))
    }

    pub fn system_time_to_unix_ms(time: SystemTime) -> u64 {
        time.duration_since(std::time::UNIX_EPOCH).map(|value| value.as_millis() as u64).unwrap_or_default()
    }

    /// Open the spawned root and assign it to a kill-on-close Job Object.
    /// PID reuse is rejected by rechecking the creation-time generation window.
    pub fn assign_to_kill_job(pid: u32, started_at: SystemTime, registered_at: SystemTime) -> Option<JobHandle> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::JobObjects::*;
        use windows::Win32::System::Threading::*;
        unsafe {
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let creation = process_creation_time(process);
            let started = system_time_to_unix_ms(started_at);
            let registered = system_time_to_unix_ms(registered_at);
            let in_window = creation.is_some_and(|created| created >= started && created <= registered);
            if !in_window {
                let _ = CloseHandle(process);
                return None;
            }
            let job = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok()?;
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let assigned = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .is_ok()
                && AssignProcessToJobObject(job, process).is_ok();
            let _ = CloseHandle(process);
            if !assigned {
                let _ = CloseHandle(job);
                return None;
            }
            Some(JobHandle(Arc::new(JobHandleInner(job))))
        }
    }

    fn process_missing(pid: u32) -> bool {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::*;
        unsafe {
            match OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(process) => {
                    let mut code = 0u32;
                    let exited = GetExitCodeProcess(process, &mut code).is_ok() && code != 259; // STILL_ACTIVE
                    let _ = CloseHandle(process);
                    exited
                }
                Err(_) => true,
            }
        }
    }

    async fn wait_for_exit(pid: u32, deadline: SystemTime) -> Result<(), String> {
        use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows::Win32::System::Threading::*;
        loop {
            let now = SystemTime::now();
            if now >= deadline {
                return Err("Windows 本机进程树清理超时".to_string());
            }
            let remaining = deadline.duration_since(now).unwrap_or_default();
            let timeout = remaining.min(Duration::from_secs(4)).as_millis() as u32;
            let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) };
            let Ok(process) = process else {
                return if process_missing(pid) {
                    Ok(())
                } else {
                    Err("Windows 本机进程树清理未确认".to_string())
                };
            };
            let waited = unsafe { WaitForSingleObject(process, timeout) };
            unsafe {
                let _ = CloseHandle(process);
            }
            match waited {
                WAIT_OBJECT_0 => return Ok(()),
                WAIT_TIMEOUT => continue,
                _ => return Err("Windows 本机进程树清理未确认".to_string()),
            }
        }
    }

    /// Kill the tracked tree with the Job Object; fall back to a generation-checked
    /// `taskkill /T /F` when the job could not be assigned.
    pub async fn clean_windows_tree(pid: u32) -> Result<(), String> {
        let deadline = SystemTime::now() + CLEANUP_DEADLINE;
        let ownership = ownership_of(pid).ok_or_else(|| "Windows 本机进程缺少所有权记录，未停止未知进程".to_string())?;
        let job = ownership.job.lock().unwrap_or_else(|error| error.into_inner()).clone();
        if let Some(job) = job {
            let terminated = unsafe { windows::Win32::System::JobObjects::TerminateJobObject(job.raw(), 1) };
            if terminated.is_err() && !process_missing(pid) {
                return Err("Windows 本机进程树清理未确认".to_string());
            }
            return wait_for_exit(pid, deadline).await;
        }
        // Fallback: recheck the root generation, then force the tree.
        let started = system_time_to_unix_ms(ownership.started_at);
        let registered = system_time_to_unix_ms(ownership.registered_at);
        if let Some(created) = root_creation_time(pid) {
            if created < started || created > registered {
                return Ok(()); // New generation, never ours.
            }
        } else if !process_missing(pid) {
            return Err("Windows 本机进程树清理未确认".to_string());
        }
        let system_root = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_default();
        let status = tokio::process::Command::new(system_root.join("System32").join("taskkill.exe"))
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
        match status {
            Ok(status) if status.success() || process_missing(pid) => {}
            _ => return Err("Windows 本机进程树清理未确认".to_string()),
        }
        wait_for_exit(pid, deadline).await
    }

    fn root_creation_time(pid: u32) -> Option<u64> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::*;
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let creation = process_creation_time(process);
            let _ = CloseHandle(process);
            creation
        }
    }
}
