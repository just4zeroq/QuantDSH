//! Home-owned loopback origin, configured through the official webserver row.
//! Mirrors workdsh apps/desktop/src/runtime-port.ts.

use regex::Regex;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use yaml_rust::{Yaml, YamlLoader};

const HOST: &str = "127.0.0.1";
const STATE_FILE: &str = ".workdsh-loopback.json";
const PATCH_FILE: &str = ".workdsh-loopback.patch.yml";
const PREFERRED_PORT: u16 = 3080;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimePort {
    Unmanaged,
    Managed { host: String, port: u16, patch: PathBuf },
}

/// js-yaml emits `!!js "expression"` for Harness expressions; the carrier's
/// lightweight loader keeps them as opaque scalars. Strip the tag and keep the value.
fn strip_expression_tags(text: &str) -> String {
    static TAG: OnceLock<Regex> = OnceLock::new();
    TAG.get_or_init(|| Regex::new(r"(:\s*)!![A-Za-z0-9:_-]+\s+").unwrap())
        .replace_all(text, "$1")
        .to_string()
}

fn has_server_override(value: &Yaml) -> bool {
    match value {
        Yaml::Array(items) => items.iter().any(has_server_override),
        Yaml::Hash(map) => {
            // A user's row is authoritative, including expressions, TLS, compression,
            // disabling or replacing the server. Never evaluate or rewrite that layer.
            for (key, item) in map {
                if matches!(key, Yaml::String(text) | Yaml::Real(text) if text == "id") {
                    if matches!(item, Yaml::String(text) | Yaml::Real(text) if text == "webserver") {
                        return true;
                    }
                }
            }
            map.values().any(has_server_override)
        }
        _ => false,
    }
}

fn load_lenient(text: &str) -> Result<Vec<Yaml>, String> {
    YamlLoader::load_from_str(&strip_expression_tags(text)).map_err(|error| error.to_string())
}

async fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

async fn saved_port(file: &Path) -> Result<Option<u16>, String> {
    let Some(text) = read_optional(file).await? else { return Ok(None) };
    let state: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return Err("工作区端口记录无效，请核对本机配置；未更改原有访问地址。".to_string()),
    };
    let valid = state.get("version").and_then(Value::as_i64) == Some(1)
        && state.get("host").and_then(Value::as_str) == Some(HOST)
        && state
            .get("port")
            .and_then(Value::as_u64)
            .is_some_and(|port| (1024..=65535).contains(&port));
    if !valid {
        return Err("工作区端口记录无效，请核对本机配置；未更改原有访问地址。".to_string());
    }
    Ok(state.get("port").and_then(Value::as_u64).map(|port| port as u16))
}

/** Reserve briefly to check a bind; the official server remains the sole owner. */
async fn available_port(port: u16) -> Result<Option<u16>, String> {
    let probe = match TcpListener::bind((HOST, port)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let selected = probe.local_addr().map(|address| address.port()).ok();
    drop(probe);
    Ok(Some(selected.unwrap_or(port)))
}

/** Existing Homes keep their origin; a conflicting owner is never stopped. */
pub async fn prepare_runtime_port(
    home: &Path,
    user_patch_files: &[PathBuf],
    options: RuntimePortOptions,
) -> Result<RuntimePort, String> {
    for file in user_patch_files {
        if let Some(text) = read_optional(file).await? {
            let documents = load_lenient(&text)?;
            if documents.iter().any(has_server_override) {
                return Ok(RuntimePort::Unmanaged);
            }
        }
    }
    // `mkdirSync(home, { recursive: true, mode: 0o700 })`: the mode applies only
    // when the directory is created, so an existing Home keeps its permissions.
    #[cfg(unix)]
    let created = !home.exists();
    fs::create_dir_all(home).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(home, fs::Permissions::from_mode(0o700));
    }
    let state = home.join(STATE_FILE);
    let saved = saved_port(&state).await?;
    let port = if let Some(existing) = saved {
        if available_port(existing).await?.is_none() {
            return Err(format!(
                "此工作区原有地址 {HOST}:{existing} 正被占用。请先关闭占用该地址的应用后重试；为保留页面数据，本次未切换地址，也未停止其他进程。"
            ));
        }
        existing
    } else {
        let preferred = available_port(PREFERRED_PORT).await?;
        // Before Home-owned records existed, this Desktop used 3080. Preserve
        // that origin for an existing Profile instead of hiding its localStorage.
        if preferred.is_none() && options.existing_home {
            return Err(format!(
                "已有工作区原地址 {HOST}:{PREFERRED_PORT} 正被占用。为保留页面数据，本次未切换地址，也未停止其他进程；请关闭占用该地址的应用后重试。"
            ));
        }
        let mut allocated = match preferred {
            Some(port) => port,
            None => available_port(0).await?.ok_or_else(|| "无法为工作区分配本机端口，请稍后重试。".to_string())?,
        };
        match write_state_once(&state, allocated) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another launch created this Home's origin while we probed. Reuse that
                // record; never replace it with a second origin chosen concurrently.
                let concurrent = saved_port(&state).await?;
                let concurrent = match concurrent {
                    Some(port) if available_port(port).await.ok() == Some(Some(port)) => Some(port),
                    _ => None,
                };
                allocated = concurrent
                    .ok_or_else(|| "此工作区正在由另一个启动操作使用，请关闭重复窗口后重试。".to_string())?;
            }
            Err(error) => return Err(error.to_string()),
        }
        allocated
    };
    let patch = home.join(PATCH_FILE);
    let temporary = format!("{}.{}", patch.display(), uuid::Uuid::new_v4());
    let dumped = dump_webserver_patch(HOST, port);
    let write = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(dumped.as_bytes())?;
        Ok(())
    })();
    let renamed = write.and_then(|()| fs::rename(&temporary, &patch));
    let _ = fs::remove_file(&temporary);
    renamed.map_err(|error| error.to_string())?;
    Ok(RuntimePort::Managed { host: HOST.to_string(), port, patch })
}

fn write_state_once(path: &Path, port: u16) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(format!("{{\"version\":1,\"host\":\"{HOST}\",\"port\":{port}}}\n").as_bytes())
}

/// js-yaml JSON_SCHEMA dump of [{ id: 'webserver', config: { host, port } }].
fn dump_webserver_patch(host: &str, port: u16) -> String {
    let mut output = String::new();
    let document = Yaml::Array(vec![Yaml::Hash(
        [
            (Yaml::String("id".to_string()), Yaml::String("webserver".to_string())),
            (
                Yaml::String("config".to_string()),
                Yaml::Hash(
                    [
                        (Yaml::String("host".to_string()), Yaml::String(host.to_string())),
                        (Yaml::String("port".to_string()), Yaml::Integer(port as i64)),
                    ]
                    .into_iter()
                    .collect(),
                ),
            ),
        ]
        .into_iter()
        .collect(),
    )]);
    let mut emitter = yaml_rust::YamlEmitter::new(&mut output);
    if emitter.dump(&document).is_err() {
        // Deterministic fallback: the exact js-yaml output for this declaration.
        return format!("- id: webserver\n  config:\n    host: {host}\n    port: {port}\n");
    }
    // `YamlEmitter` prefixes a document marker; js-yaml's `dump` does not.
    if let Some(rest) = output.strip_prefix("---\n") {
        output = rest.to_string();
    } else if let Some(rest) = output.strip_prefix("---") {
        output = rest.trim_start_matches('\n').to_string();
    }
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimePortOptions {
    pub existing_home: bool,
}

/** Probe/launch is a narrow race; report its actual bind error without scanning. */
pub fn runtime_port_conflict(selection: &RuntimePort, output: &str) -> Option<String> {
    let RuntimePort::Managed { host, port, .. } = selection else { return None };
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| Regex::new(r"(?i)\bEADDRINUSE\b|address already in use").unwrap());
    if !pattern.is_match(output) {
        return None;
    }
    Some(format!(
        "工作区端口 {host}:{port} 在启动时被占用，请关闭占用该地址的应用后重试。已保留原地址和数据，未停止其他进程。"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_webserver_override() {
        let documents = load_lenient("- id: webserver\n  config:\n    host: 127.0.0.1\n").unwrap();
        assert!(documents.iter().any(has_server_override));
        let documents = load_lenient("- id: other\n  config:\n    host: 127.0.0.1\n").unwrap();
        assert!(!documents.iter().any(has_server_override));
        let documents = load_lenient("- id: webserver\n  config:\n    host: !!js \"process.env.HOST\"\n").unwrap();
        assert!(documents.iter().any(has_server_override));
    }

    #[test]
    fn dumps_the_expected_patch() {
        assert_eq!(
            dump_webserver_patch("127.0.0.1", 3080),
            "- id: webserver\n  config:\n    host: 127.0.0.1\n    port: 3080\n"
        );
    }
}
