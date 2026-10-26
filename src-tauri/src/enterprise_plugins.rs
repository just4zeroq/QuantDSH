//! Enterprise plugin inventory, mirroring the upstream desktop's enterprise-plugins.ts.
//! Enterprise delivery uses explicitly installed packages; never copies account data.

use serde_json::{Map, Value};
use std::path::Path;
use tokio::process::Command;

pub const ENTERPRISE_PLUGIN_NAMES: [&str; 2] =
    ["quantdsh-provider-identity-enterprise", "quantdsh-plugin-enterprise-collaboration"];

const CONNECTION_BUNDLE: &str = "quantdsh-enterprise-connection";
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(240);

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(_)) => true,
        Some(Value::Object(_)) | Some(Value::Array(_)) => true,
        _ => false,
    }
}

fn section<'a>(manifest: &'a Value, name: &str) -> Option<&'a Map<String, Value>> {
    manifest.get(name).and_then(Value::as_object)
}

/// quantdsh `installedEnterprisePlugins`.
pub fn installed_enterprise_plugins(profile: &Path, version: &str) -> Result<Vec<String>, String> {
    let manifest_file = profile.join("package.json");
    if !manifest_file.exists() {
        return Ok(Vec::new());
    }
    let manifest = read_json(&manifest_file)?;
    let bundle = section(&manifest, "dependencies").and_then(|dependencies| dependencies.get(CONNECTION_BUNDLE)).filter(|value| truthy(Some(value)));
    let bundle_root = profile.join("node_modules").join(CONNECTION_BUNDLE);
    if bundle.is_some() && !bundle_root.join("package.json").exists() {
        return Err("企业连接包尚未完成安装".to_string());
    }
    let mut names: Vec<String> = Vec::new();
    for name in ENTERPRISE_PLUGIN_NAMES {
        // `section?.[name]` is a *truthiness* test: an empty string does not count.
        let declared = bundle.is_some()
            || [section(&manifest, "dependencies"), section(&manifest, "optionalDependencies")]
                .iter()
                .any(|section| section.map(|section| truthy(section.get(name))).unwrap_or(false));
        if !declared {
            continue;
        }
        let root = if bundle.is_some() { &bundle_root } else { profile };
        let package_file = root.join("node_modules").join(name).join("package.json");
        if !package_file.exists() {
            return Err(format!("企业插件尚未完成安装：{name}"));
        }
        let package = read_json(&package_file)?;
        let package_name = package.get("name").and_then(Value::as_str);
        let desktop_export = package.get("exports").and_then(|exports| exports.get("./desktop"));
        let peer = package
            .get("peerDependencies")
            .and_then(|peers| peers.get("@deepseek-ai/dsh"))
            .and_then(Value::as_str);
        if package_name != Some(name) || !truthy(desktop_export) || peer != Some(version) {
            return Err(format!("企业插件与当前 DSH 不兼容：{name}"));
        }
        names.push(name.to_string());
    }
    Ok(names)
}

async fn execute(
    node: &Path,
    args: &[&str],
    cwd: &Path,
) -> Result<(), String> {
    let mut command = Command::new(node);
    command.args(args).current_dir(cwd).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| "企业插件操作超时".to_string())?
        .map_err(|error| format!("无法启动企业插件操作：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() { stderr.trim() } else { stdout.trim() };
        return Err(if detail.is_empty() {
            format!("企业插件操作失败（退出码 {}）", output.status.code().unwrap_or(-1))
        } else {
            format!("企业插件操作失败：{detail}")
        });
    }
    Ok(())
}

/// quantdsh `composeInstalledEnterprisePlugins`.
pub async fn compose_installed_enterprise_plugins(
    source: &Path,
    target: &Path,
    version: &str,
    node: &Path,
    pnpm: &Path,
) -> Result<Vec<String>, String> {
    let names = installed_enterprise_plugins(source, version)?;
    if !names.iter().any(|name| name == ENTERPRISE_PLUGIN_NAMES[0]) {
        return Err("请先在个人空间的插件管理中安装企业账号插件，再进行企业登录".to_string());
    }
    let manifest_file = target.join("package.json");
    let mut manifest = read_json(&manifest_file)?;
    let archives = target.join("enterprise-plugin-archives");
    // `mkdirSync(archives, { recursive: true, mode: 0o700 })` only sets the mode
    // when the directory is created.
    #[cfg(unix)]
    let archives_created = !archives.exists();
    std::fs::create_dir_all(&archives).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    if archives_created {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&archives, std::fs::Permissions::from_mode(0o700));
    }
    {
        let object = manifest.as_object_mut().ok_or_else(|| "Profile 清单无效".to_string())?;
        object.entry("dependencies".to_string()).or_insert_with(|| Value::Object(Map::new()));
    }
    // Pack installed code through the bundled package manager; do not copy Profile files or credentials.
    let pnpm_path = pnpm.to_string_lossy().to_string();
    let archives_path = archives.to_string_lossy().to_string();
    for name in &names {
        let source_manifest = read_json(&source.join("package.json"))?;
        let bundled = source_manifest
            .get("dependencies")
            .and_then(|dependencies| dependencies.get(CONNECTION_BUNDLE))
            .filter(|value| truthy(Some(value)));
        let root = if bundled.is_some() { source.join("node_modules").join(CONNECTION_BUNDLE) } else { source.to_path_buf() };
        let directory = root.join("node_modules").join(name);
        let package = read_json(&directory.join("package.json"))?;
        let package_version = package.get("version").and_then(Value::as_str).unwrap_or("");
        execute(node, &[&pnpm_path, "pack", "--pack-destination", &archives_path], &directory).await?;
        let filename = format!("{name}-{package_version}.tgz");
        if !archives.join(&filename).exists() {
            return Err(format!("企业插件导出失败：{name}"));
        }
        if let Some(dependencies) = manifest.get_mut("dependencies").and_then(Value::as_object_mut) {
            dependencies.insert(name.clone(), Value::String(format!("file:./enterprise-plugin-archives/{filename}")));
        }
    }
    for name in ENTERPRISE_PLUGIN_NAMES {
        if names.iter().any(|installed| installed == name) {
            continue;
        }
        if let Some(dependencies) = manifest.get_mut("dependencies").and_then(Value::as_object_mut) {
            dependencies.remove(name);
        }
        if let Some(optional) = manifest.get_mut("optionalDependencies").and_then(Value::as_object_mut) {
            optional.remove(name);
        }
    }
    let text = format!("{}\n", serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true).mode(0o600);
        options.open(&manifest_file).and_then(|mut file| {
            use std::io::Write;
            file.write_all(text.as_bytes())
        }).map_err(|error| error.to_string())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&manifest_file, text).map_err(|error| error.to_string())?;
    }
    let target_path = target.to_string_lossy().to_string();
    execute(node, &[&pnpm_path, "--dir", &target_path, "install", "--prod", "--force"], target).await?;
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("quantdsh-plugin-test-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_manifest_reports_no_plugins() {
        let dir = temp_dir("missing");
        assert_eq!(installed_enterprise_plugins(&dir, "1.0.0").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn declared_plugin_without_install_errors() {
        let dir = temp_dir("declared");
        std::fs::write(dir.join("package.json"), r#"{"dependencies":{"quantdsh-provider-identity-enterprise":"1.0.0"}}"#).unwrap();
        let error = installed_enterprise_plugins(&dir, "1.0.0").unwrap_err();
        assert_eq!(error, "企业插件尚未完成安装：quantdsh-provider-identity-enterprise");
    }

    #[test]
    fn incompatible_peer_rejected() {
        let dir = temp_dir("peer");
        std::fs::write(dir.join("package.json"), r#"{"dependencies":{"quantdsh-provider-identity-enterprise":"1.0.0"}}"#).unwrap();
        let plugin = dir.join("node_modules").join("quantdsh-provider-identity-enterprise");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("package.json"),
            format!(r#"{{"name":"quantdsh-provider-identity-enterprise","exports":{{"./desktop":"./desktop.js"}},"peerDependencies":{{"@deepseek-ai/dsh":"9.9.9"}}}}"#),
        )
        .unwrap();
        let error = installed_enterprise_plugins(&dir, "1.0.0").unwrap_err();
        assert_eq!(error, "企业插件与当前 DSH 不兼容：quantdsh-provider-identity-enterprise");
    }

    #[test]
    fn installed_plugin_listed() {
        let dir = temp_dir("installed");
        std::fs::write(dir.join("package.json"), r#"{"dependencies":{"quantdsh-provider-identity-enterprise":"1.0.0"}}"#).unwrap();
        let plugin = dir.join("node_modules").join("quantdsh-provider-identity-enterprise");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("package.json"),
            format!(r#"{{"name":"quantdsh-provider-identity-enterprise","exports":{{"./desktop":"./desktop.js"}},"peerDependencies":{{"@deepseek-ai/dsh":"1.0.0"}}}}"#),
        )
        .unwrap();
        let names = installed_enterprise_plugins(&dir, "1.0.0").unwrap();
        assert_eq!(names, vec!["quantdsh-provider-identity-enterprise".to_string()]);
    }
}
