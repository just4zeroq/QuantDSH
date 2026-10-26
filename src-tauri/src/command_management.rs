//! Native menu adapter for the optional `dsh` terminal command, mirroring
//! the upstream desktop's command-management.ts.
//! Official workers own all command installation mutations.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::AppHandle;
use tauri::Runtime;
use tauri_plugin_dialog::DialogExt;

#[derive(Debug, Deserialize)]
pub struct CommandState {
    pub fingerprint: String,
    pub managed: bool,
    pub available: bool,
    #[serde(default)]
    pub destination: Option<String>,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default, rename = "activeCommand")]
    pub active_command: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

/// quantdsh `operation`: a second menu invocation while one is running is a no-op.
static BUSY: AtomicBool = AtomicBool::new(false);

pub fn manage_command<R: Runtime>(app: &AppHandle<R>, resources: &Path, scope: &str) {
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let resources = resources.to_path_buf();
    let scope = scope.to_string();
    std::thread::spawn(move || {
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("无法管理 dsh 命令：{error}"))
            .and_then(|runtime| runtime.block_on(show(&app, &resources, &scope)));
        if let Err(error) = outcome {
            let reporting = app.clone();
            let target = app.clone();
            let _ = reporting.run_on_main_thread(move || {
                error_box(&target, "无法管理 dsh 命令", &error);
            });
        }
        BUSY.store(false, Ordering::SeqCst);
    });
}

fn node_executable(resources: &Path) -> PathBuf {
    resources
        .join("primary-runtime")
        .join("dependencies")
        .join("node")
        .join("bin")
        .join(if cfg!(target_os = "windows") { "node.exe" } else { "node" })
}

fn command_manager_entry(resources: &Path) -> PathBuf {
    resources.join("runtime").join("cli").join("command-manager.js")
}

fn sanitized_env() -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(name, _)| {
            let upper = name.to_ascii_uppercase();
            !upper.contains("KEY")
                && !upper.contains("SECRET")
                && !upper.contains("TOKEN")
                && !upper.contains("PASSWORD")
                && name != "NODE_OPTIONS"
                && name != "NODE_PATH"
        })
        .collect()
}

const MAX_BUFFER: usize = 65_536;

async fn execute(program: &Path, args: &[&str], env: &[(String, String)]) -> Result<String, String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).env_clear();
    for (name, value) in env {
        command.env(name, value);
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .map_err(|_| "命令管理超时".to_string())?
        .map_err(|error| format!("无法启动命令管理工具：{error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() || stdout.len() > MAX_BUFFER {
        // Like the Electron carrier: a captured stdout is still the answer.
        if !stdout.is_empty() {
            return Ok(stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(if stderr.trim().is_empty() {
            format!("命令管理失败（退出码 {}）", output.status.code().unwrap_or(-1))
        } else {
            format!("命令管理失败：{}", stderr.trim())
        });
    }
    Ok(stdout)
}

/// quantdsh `worker`.
async fn worker(
    resources: &Path,
    action: &str,
    expected: Option<&str>,
    elevated: bool,
) -> Result<CommandState, String> {
    let node = node_executable(resources);
    let entry = command_manager_entry(resources);
    let entry_arg = entry.to_string_lossy().to_string();
    let env = sanitized_env();
    let expected = expected.unwrap_or("");
    let stdout = if elevated {
        let script = "on run argv\nset cmd to \"/usr/bin/env -i \" & quoted form of (item 1 of argv) & \" \" & quoted form of (item 2 of argv) & \" \" & quoted form of (item 3 of argv) & \" \" & quoted form of (item 4 of argv)\ndo shell script cmd with administrator privileges\nend run";
        let node_arg = node.to_string_lossy().to_string();
        execute(Path::new("/usr/bin/osascript"), &["-e", script, &node_arg, &entry_arg, action, expected], &env).await?
    } else {
        let mut args: Vec<&str> = vec![&entry_arg, action];
        if !expected.is_empty() {
            args.push(expected);
        }
        execute(&node, &args, &env).await?
    };
    let result: serde_json::Value =
        serde_json::from_str(&stdout).map_err(|_| "命令管理返回了无效的结果".to_string())?;
    if !result.get("ok").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        let code = result.get("code").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        let message = result
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("命令管理失败")
            .to_string();
        if !elevated && cfg!(target_os = "macos") && action != "inspect" && (code == "EACCES" || code == "EPERM") {
            return Box::pin(worker(resources, action, Some(expected), true)).await;
        }
        return Err(message);
    }
    let state: CommandState =
        serde_json::from_value(result.get("state").cloned().unwrap_or(serde_json::Value::Null))
            .map_err(|_| "命令管理返回无效状态".to_string())?;
    if !is_fingerprint(&state.fingerprint) {
        return Err("命令管理返回无效状态".to_string());
    }
    Ok(state)
}

fn is_fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// quantdsh `show`.
async fn show<R: Runtime>(app: &AppHandle<R>, resources: &Path, scope: &str) -> Result<(), String> {
    if !is_packaged() {
        message_box(app, "请安装正式 Desktop 包后管理 dsh 命令", None, &["关闭"], 0);
        return Ok(());
    }
    let state = worker(resources, "inspect", None, false).await?;
    let location = state.destination.clone().unwrap_or_else(|| {
        // `join(state.directory ?? '', 'dsh.cmd')`: the separator is part of the
        // joined path, so a bare string concat would print `...npmdsh.cmd`.
        let directory = state.directory.clone().unwrap_or_default();
        PathBuf::from(directory).join("dsh.cmd").display().to_string()
    });
    let buttons: Vec<String> = if state.managed {
        vec!["关闭".into(), "修复命令".into(), "移除命令".into()]
    } else {
        vec!["安装终端命令".into(), "暂不安装".into()]
    };
    let cancel_id = if state.managed { 0 } else { 1 };
    let detail = format!(
        "安装后，可在系统终端输入 dsh，运行官方命令或管理插件。桌面聊天无需安装此命令。\n\n这只创建终端命令入口，不会重新下载 DSH、Node 或 pnpm。\n命令位置：{location}\n当前插件管理空间：{scope}\n移除命令不会卸载桌面应用或删除工作区。"
    );
    let message = if state.managed {
        if state.available {
            "终端命令 dsh 已安装".to_string()
        } else {
            "终端命令 dsh 需要修复".to_string()
        }
    } else {
        "在终端中使用 QuantDSH（可选）".to_string()
    };
    let choice = choose_box(
        app,
        None,
        &message,
        &detail,
        &buttons,
        cancel_id,
        tauri_plugin_dialog::MessageDialogKind::Info,
    );
    let action = if state.managed {
        match choice {
            1 => Some("install"),
            2 => Some("remove"),
            _ => None,
        }
    } else if choice == 0 {
        Some("install")
    } else {
        None
    };
    let Some(action) = action else { return Ok(()) };
    if action == "install"
        && (state.active_command.is_some()
            || (state.kind.as_deref().map(|kind| kind != "missing").unwrap_or(false) && !state.managed))
    {
        let confirm = choose_box(
            app,
            Some("确认切换 dsh 命令到 QuantDSH？"),
            "",
            "已有命令会由官方安装逻辑保留；移除时恢复未被修改的原命令。其他安装与 shell 启动文件保持原样。",
            &["取消".to_string(), "继续".to_string()],
            0,
            tauri_plugin_dialog::MessageDialogKind::Warning,
        );
        if confirm != 1 {
            return Ok(());
        }
    }
    worker(resources, action, Some(&state.fingerprint), false).await?;
    message_box(
        app,
        if action == "remove" {
            "QuantDSH 命令已移除"
        } else {
            "QuantDSH 命令已安装，请打开新终端使用"
        },
        None,
        &["关闭"],
        0,
    );
    Ok(())
}

/// Tauri has no packaged flag: the `dev` cfg is only set for dev builds, and a
/// release binary is the closest equivalent of a packaged Electron carrier.
fn is_packaged() -> bool {
    !cfg!(dev)
}

fn dialog_buttons(labels: &[String]) -> tauri_plugin_dialog::MessageDialogButtons {
    use tauri_plugin_dialog::MessageDialogButtons;
    match labels.len() {
        0 | 1 => MessageDialogButtons::OkCustom(labels.first().cloned().unwrap_or_else(|| "关闭".to_string())),
        2 => MessageDialogButtons::OkCancelCustom(labels[0].clone(), labels[1].clone()),
        _ => MessageDialogButtons::YesNoCancelCustom(labels[0].clone(), labels[1].clone(), labels[2].clone()),
    }
}

/// Map the pressed button back to the Electron `response` index, with ESC
/// (Cancel) resolving to the configured `cancelId`.
fn dialog_result_index(result: &tauri_plugin_dialog::MessageDialogResult, labels: &[String], cancel_id: usize) -> usize {
    use tauri_plugin_dialog::MessageDialogResult;
    match result {
        MessageDialogResult::Ok => 0,
        MessageDialogResult::Yes => 0,
        MessageDialogResult::No => 1,
        MessageDialogResult::Cancel => cancel_id.min(labels.len().saturating_sub(1)),
        MessageDialogResult::Custom(label) => labels.iter().position(|item| item == label).unwrap_or(0),
    }
}

fn message_box<R: Runtime>(app: &AppHandle<R>, message: &str, title: Option<&str>, buttons: &[&str], cancel_id: usize) -> usize {
    let labels: Vec<String> = buttons.iter().map(|label| label.to_string()).collect();
    let mut builder = app.dialog().message(message.to_string());
    if let Some(title) = title {
        builder = builder.title(title.to_string());
    }
    builder = builder.kind(tauri_plugin_dialog::MessageDialogKind::Info).buttons(dialog_buttons(&labels));
    let result = builder.blocking_show_with_result();
    dialog_result_index(&result, &labels, cancel_id)
}

/// Electron `showMessageBox` with a separate `detail` block: the message and the
/// detail are joined the way the native dialog renders them, and `title` stays
/// optional because most QuantDSH boxes only carry a message.
fn choose_box<R: Runtime>(
    app: &AppHandle<R>,
    title: Option<&str>,
    message: &str,
    detail: &str,
    buttons: &[String],
    cancel_id: usize,
    kind: tauri_plugin_dialog::MessageDialogKind,
) -> usize {
    let body = match (message.is_empty(), detail.is_empty()) {
        (true, true) => String::new(),
        (true, false) => detail.to_string(),
        (false, true) => message.to_string(),
        (false, false) => format!("{message}\n\n{detail}"),
    };
    let mut builder = app.dialog().message(body);
    if let Some(title) = title {
        builder = builder.title(title.to_string());
    }
    builder = builder.kind(kind).buttons(dialog_buttons(buttons));
    let result = builder.blocking_show_with_result();
    dialog_result_index(&result, buttons, cancel_id)
}

fn error_box<R: Runtime>(app: &AppHandle<R>, title: &str, message: &str) {
    app.dialog()
        .message(message.to_string())
        .title(title.to_string())
        .kind(tauri_plugin_dialog::MessageDialogKind::Error)
        .buttons(tauri_plugin_dialog::MessageDialogButtons::Ok)
        .blocking_show();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_validation() {
        assert!(is_fingerprint(&"a".repeat(64)));
        assert!(is_fingerprint(&"0123456789abcdef".repeat(4)));
        assert!(!is_fingerprint(&"g".repeat(64)));
        assert!(!is_fingerprint(&"a".repeat(63)));
        assert!(!is_fingerprint(&"A".repeat(64)));
    }
}
