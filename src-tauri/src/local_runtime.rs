//! Writable Profile/configuration over the official installation anchor. No custom Loader.
//! Mirrors the upstream desktop's local-runtime.ts.

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use yaml_rust::{Yaml, YamlEmitter, YamlLoader};

use crate::plugin_archive_cache::cached_package_manager;
use crate::runtime_compatibility::sync_bundled_compatibility;

pub const BASE_STATE: &str = ".quantdsh-desktop-base.json";

pub struct MaterializedProfile {
    pub profile: PathBuf,
    pub changed: bool,
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// `mkdirSync(path, { recursive: true, mode: 0o700 })`: the mode applies only
/// when the directory is created, so an existing directory keeps its mode.
fn make_private_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let created = !path.exists();
    fs::create_dir_all(path).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let text = format!("{}\n", serde_json::to_string_pretty(value).map_err(|error| error.to_string())?);
    write_private(path, text.as_bytes())
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true).mode(0o600);
    options.open(path).and_then(|mut file| {
        use std::io::Write;
        file.write_all(bytes)
    }).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|error| error.to_string())
}

fn member_defaults(release: &Value) -> Value {
    // JSON.stringify omits keys whose value is undefined, so an absent
    // packageManager must not appear in the serialized defaults.
    let mut object = Map::new();
    object.insert("name".to_string(), json!("quantdsh-member-profile"));
    object.insert("private".to_string(), json!(true));
    object.insert("type".to_string(), json!("module"));
    if let Some(package_manager) = release.get("packageManager") {
        if !package_manager.is_null() {
            object.insert("packageManager".to_string(), package_manager.clone());
        }
    }
    object.insert("dependencies".to_string(), Value::Object(Map::new()));
    if let Some(dsh) = release.get("dsh") {
        object.insert("dsh".to_string(), dsh.clone());
    }
    Value::Object(object)
}

fn base_digest(source: &Path, base: &Value, version: &str) -> Result<String, String> {
    let marker = source.join(".quantdsh-desktop-release.json");
    let workspace = source.join("pnpm-workspace.yaml");
    let mut hasher = Sha256::new();
    hasher.update(b"installation-owned-defaults-v2");
    hasher.update(serde_json::to_string(base).map_err(|error| error.to_string())?.as_bytes());
    hasher.update(version.as_bytes());
    if marker.exists() {
        hasher.update(fs::read(&marker).map_err(|error| error.to_string())?);
    }
    if workspace.exists() {
        hasher.update(fs::read(&workspace).map_err(|error| error.to_string())?);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// JSON.stringify semantics over a YAML tree, preserving mapping insertion order.
fn ordered_json(value: &Yaml) -> String {
    match value {
        Yaml::Real(text) | Yaml::String(text) => serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string()),
        Yaml::Boolean(value) => value.to_string(),
        Yaml::Integer(value) => value.to_string(),
        Yaml::Null => "null".to_string(),
        Yaml::Array(items) => {
            let rendered: Vec<String> = items.iter().map(ordered_json).collect();
            format!("[{}]", rendered.join(","))
        }
        Yaml::Hash(map) => {
            let rendered: Vec<String> = map
                .iter()
                .map(|(key, value)| {
                    let key = match key {
                        Yaml::String(text) | Yaml::Real(text) => serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string()),
                        other => ordered_json(other),
                    };
                    format!("{key}:{}", ordered_json(value))
                })
                .collect();
            format!("{{{}}}", rendered.join(","))
        }
        Yaml::Alias(_) | Yaml::BadValue => "null".to_string(),
    }
}

fn load_yaml_file(path: &Path) -> Result<Vec<Yaml>, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    YamlLoader::load_from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn update_package_policy(source: &Path, target: &Path) -> Result<bool, String> {
    let policy = source.join("pnpm-workspace.yaml");
    let current = target.join("pnpm-workspace.yaml");
    if !policy.exists() {
        return Ok(false);
    }
    let parse = |path: &Path| -> Result<Yaml, String> {
        let documents = load_yaml_file(path)?;
        let value = documents.into_iter().next().ok_or_else(|| "Profile 包管理配置必须是 YAML 对象".to_string())?;
        if !matches!(value, Yaml::Hash(_)) {
            return Err("Profile 包管理配置必须是 YAML 对象".to_string());
        }
        Ok(value)
    };
    if !current.exists() {
        let mut defaults = parse(&policy)?;
        // Like the official external Profile initializer, use installation-owned service peers.
        if let Yaml::Hash(map) = &mut defaults {
            map.insert(Yaml::String("autoInstallPeers".to_string()), Yaml::Boolean(false));
        }
        let output = dump_yaml(&defaults)?;
        write_private(&current, output.as_bytes())?;
        return Ok(true);
    }
    let owned = parse(&policy)?;
    let mut user = parse(&current)?;
    let user_overrides = overrides_of(&user);
    let owned_overrides = overrides_of(&owned);
    if ordered_json_option(&user_overrides) == ordered_json_option(&owned_overrides) {
        return Ok(false);
    }
    if let Some(value) = &user_overrides {
        if !matches!(value, Yaml::Hash(_)) {
            return Err("Profile 包版本约束无效".to_string());
        }
    }
    let Some(value) = &owned_overrides else {
        return Err("Profile 包版本约束无效".to_string());
    };
    if !matches!(value, Yaml::Hash(_)) {
        return Err("Profile 包版本约束无效".to_string());
    }
    // Official runtime/Cordis versions are product-owned. Other install preferences and overrides are user-owned.
    let Yaml::Hash(owned_map) = value.clone() else { unreachable!() };
    let mut merged = match &user_overrides {
        Some(Yaml::Hash(map)) => map.clone(),
        _ => yaml_rust::yaml::Hash::new(),
    };
    for (key, value) in owned_map {
        merged.insert(key, value);
    }
    if ordered_json(&Yaml::Hash(merged.clone())) == ordered_json_option(&user_overrides) {
        return Ok(false);
    }
    if let Yaml::Hash(map) = &mut user {
        map.insert(Yaml::String("overrides".to_string()), Yaml::Hash(merged));
    }
    let output = dump_yaml(&user)?;
    write_private(&current, output.as_bytes())?;
    Ok(true)
}

/// js-yaml's `dump` never emits a document marker; `YamlEmitter` always does.
/// Keeping the file free of `---` matters because this file is user-visible and
/// the official Profile initializer writes it without one.
fn dump_yaml(document: &Yaml) -> Result<String, String> {
    let mut output = String::new();
    YamlEmitter::new(&mut output).dump(document).map_err(|error| error.to_string())?;
    let trimmed = output.strip_prefix("---").unwrap_or(&output).trim_start_matches('\n').to_string();
    Ok(if trimmed.ends_with('\n') { trimmed } else { format!("{trimmed}\n") })
}

fn overrides_of(value: &Yaml) -> Option<Yaml> {
    if let Yaml::Hash(map) = value {
        for (key, item) in map {
            if matches!(key, Yaml::String(text) | Yaml::Real(text) if text == "overrides") {
                return Some(item.clone());
            }
        }
    }
    None
}

fn ordered_json_option(value: &Option<Yaml>) -> String {
    match value {
        Some(value) => ordered_json(value),
        None => "undefined".to_string(),
    }
}

/// Whether the member profile declares packages of its own.
///
/// The installation-owned defaults are never written here, so any entry in
/// `dependencies`/`optionalDependencies` is a user (or plugin-manager) addition
/// that only the profile's own `node_modules` can satisfy.
fn declares_dependencies(manifest: &Path) -> bool {
    let Ok(current) = read_json(manifest) else { return false };
    ["dependencies", "optionalDependencies"].iter().any(|section| {
        current
            .get(*section)
            .and_then(Value::as_object)
            .map(|map| !map.is_empty())
            .unwrap_or(false)
    })
}

pub fn materialize_runtime_profile(source: &Path, home: &Path) -> Result<MaterializedProfile, String> {
    let target = home.join("profiles").join("quantdsh");
    let release = read_json(&source.join("package.json"))?;
    // Defaults are already installed in the immutable runtime. The member project owns only explicit additions.
    let base = member_defaults(&release);
    let version = read_json(&source.join("node_modules").join("@deepseek-ai").join("dsh").join("package.json"))?
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| "Bundled DSH version is missing".to_string())?
        .to_string();
    let state = target.join(BASE_STATE);
    let manifest = target.join("package.json");
    let modules = target.join("node_modules");
    make_private_dir(&target)?;
    // A previous shared symlink is not safe for plugin writes. Refuse to mutate it.
    if modules.exists() && fs::symlink_metadata(&modules).map(|meta| meta.file_type().is_symlink()).unwrap_or(false) {
        return Err("此 Profile 的依赖仍链接到安装包，请选择独立的可写空间后再启动".to_string());
    }
    let digest = base_digest(source, &base, &version)?;
    let previous: Option<Value> = if state.exists() { Some(read_json(&state)?) } else { None };
    let mut changed = previous
        .as_ref()
        .and_then(|value| value.get("pendingInstallation"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !manifest.exists() {
        write_json(&manifest, &base)?;
        write_private(&target.join("cordis.patch.yml"), b"[]\n")?;
    } else if previous.as_ref().and_then(|value| value.get("digest")).and_then(Value::as_str) != Some(digest.as_str()) {
        let Some(previous) = previous.as_ref() else {
            return Err("已有 Profile 缺少 Desktop 版本记录；请在官方插件管理中核对后使用独立空间".to_string());
        };
        let mut current = read_json(&manifest)?;
        // Only the unchanged recorded default remains product-owned. A user may
        // pin another manager/version or remove the declaration altogether.
        let previous_base = previous.get("base").cloned().unwrap_or(Value::Null);
        if current.get("packageManager") == previous_base.get("packageManager") {
            match base.get("packageManager") {
                Some(Value::Null) | None => {
                    if let Some(object) = current.as_object_mut() {
                        object.remove("packageManager");
                    }
                }
                Some(value) => {
                    if let Some(object) = current.as_object_mut() {
                        object.insert("packageManager".to_string(), value.clone());
                    }
                }
            }
        }
        for section in ["dependencies", "optionalDependencies"] {
            let owned = previous_base.get(section).and_then(Value::as_object).cloned().unwrap_or_default();
            let next = base.get(section).and_then(Value::as_object).cloned().unwrap_or_default();
            let current_section = current
                .as_object_mut()
                .map(|object| object.entry(section.to_string()).or_insert_with(|| Value::Object(Map::new())))
                .and_then(|value| value.as_object_mut());
            let Some(current_section) = current_section else { continue };
            for (name, value) in &owned {
                if current_section.get(name) != Some(value) {
                    continue; // Explicit user removal/change remains owned by the user.
                }
                match next.get(name) {
                    None => {
                        current_section.remove(name);
                    }
                    Some(value) => {
                        current_section.insert(name.clone(), value.clone());
                    }
                }
            }
            for (name, value) in &next {
                if !owned.contains_key(name) && !current_section.contains_key(name) {
                    current_section.insert(name.clone(), value.clone());
                }
            }
        }
        let selected = current
            .get("dsh")
            .and_then(|dsh| dsh.get("profile"))
            .and_then(|profile| profile.get("bundles"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let old_bundles = previous_base
            .get("dsh")
            .and_then(|dsh| dsh.get("profile"))
            .and_then(|profile| profile.get("bundles"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let next_bundles = base
            .get("dsh")
            .and_then(|dsh| dsh.get("profile"))
            .and_then(|profile| profile.get("bundles"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut bundles: Vec<Value> = selected
            .into_iter()
            .filter(|name| !old_bundles.contains(name) || next_bundles.contains(name))
            .collect();
        for name in &next_bundles {
            if !old_bundles.contains(name) && !bundles.contains(name) {
                bundles.push(name.clone());
            }
        }
        {
            let object = current.as_object_mut().ok_or_else(|| "Profile 清单无效".to_string())?;
            let dsh = object.entry("dsh".to_string()).or_insert_with(|| Value::Object(Map::new()));
            let profile = dsh.as_object_mut().map(|object| object.entry("profile".to_string()).or_insert_with(|| Value::Object(Map::new())));
            if let Some(profile) = profile.and_then(|value| value.as_object_mut()) {
                profile.insert("bundles".to_string(), Value::Array(bundles));
            }
        }
        if serde_json::to_string(&read_json(&manifest)?).map_err(|error| error.to_string())? != serde_json::to_string(&current).map_err(|error| error.to_string())? {
            write_json(&manifest, &current)?;
            // Only a changed local dependency graph requires package installation, not a refreshed bundle selection.
            let dependencies_changed = ["dependencies", "optionalDependencies"]
                .iter()
                .any(|section| previous_base.get(*section) != base.get(*section));
            changed |= dependencies_changed && modules.exists();
        }
    }
    // The member profile owns only explicit additions; the installation anchor
    // supplies every default, so a profile with an empty `dependencies` map is a
    // complete steady state *without* a `node_modules` tree (materializing one
    // would duplicate the whole 646 MB installation graph). Every other gate
    // above only reinstalls when a tree already exists, which leaves one hole: a
    // profile that once declared packages and then lost its tree would be stuck
    // forever — the Host boots off the installation graph, silently loads fewer
    // plugins, and reports nothing. Treat "declared but absent" as pending so the
    // caller installs, and if that fails the real error reaches the entry screen.
    if !modules.exists() && declares_dependencies(&manifest) {
        changed = true;
    }
    let policy_changed = update_package_policy(source, &target)?;
    if policy_changed && modules.exists() {
        changed = true;
    }
    let cache = home.join("package-cache");
    let source_cache = source.join("..").join("..").join("package-cache");
    if source_cache.exists() {
        copy_tree(&source_cache, &cache, false)?;
    }
    sync_bundled_compatibility(source, &target, &digest)?;
    // Caller installs changed dependencies with the pinned official package manager before marking success.
    if changed {
        let state_value = json!({
            "digest": previous.as_ref().and_then(|value| value.get("digest")).cloned().unwrap_or_else(|| Value::String(digest.clone())),
            "base": previous.as_ref().and_then(|value| value.get("base")).cloned().unwrap_or_else(|| base.clone()),
            "pendingInstallation": true,
        });
        write_json(&state, &state_value)?;
    } else {
        write_json(&state, &json!({ "digest": digest, "base": base }))?;
    }
    Ok(MaterializedProfile { profile: target, changed })
}

pub fn mark_profile_updated(source: &Path, profile: &Path) -> Result<(), String> {
    let release = read_json(&source.join("package.json"))?;
    let base = member_defaults(&release);
    let version = read_json(&source.join("node_modules").join("@deepseek-ai").join("dsh").join("package.json"))?
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| "Bundled DSH version is missing".to_string())?
        .to_string();
    let digest = base_digest(source, &base, &version)?;
    write_json(&profile.join(BASE_STATE), &json!({ "digest": digest, "base": base }))
}

/// cpSync(source, cache, { recursive: true, force: false, errorOnExist: false }):
/// existing files are kept, missing files are copied.
fn copy_tree(source: &Path, destination: &Path, force: bool) -> Result<(), String> {
    if !destination.exists() {
        fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    }
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let target = destination.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() {
            copy_tree(&path, &target, force)?;
        } else if force || !target.exists() {
            fs::copy(&path, &target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

pub struct EnterpriseSpace {
    pub root: PathBuf,
    pub home: PathBuf,
    pub device_id: String,
}

pub fn enterprise_space(user_data: &Path, backend: &str, actor: &crate::enterprise_auth::EnterpriseMember) -> Result<EnterpriseSpace, String> {
    let root = user_data
        .join("enterprise")
        .join(crate::connection::enterprise_namespace(
            &match crate::connection::enterprise_connection(backend)? {
                crate::connection::DesktopConnection::Enterprise { portal_origin, .. } => portal_origin,
                _ => unreachable!(),
            },
            &actor.organization_id,
            &actor.id,
        ));
    make_private_dir(&root)?;
    let device_file = root.join("device.json");
    if !device_file.exists() {
        write_json(&device_file, &json!({ "deviceId": uuid::Uuid::new_v4().to_string() }))?;
    }
    let parsed = read_json(&device_file)?;
    let device_id = parsed
        .get("deviceId")
        .and_then(Value::as_str)
        // `^[0-9a-f-]{36}$`: the Electron carrier checks shape, not UUID structure.
        .filter(|value| {
            value.len() == 36
                && value
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'-'))
        })
        .ok_or_else(|| "企业本机设备记录无效".to_string())?
        .to_string();
    for name in ["home", "dsh", "agents", "workspace", "tmp", "appdata", "localappdata"] {
        make_private_dir(&root.join(name))?;
    }
    Ok(EnterpriseSpace { home: root.join("dsh"), root, device_id })
}

pub fn enterprise_environment(root: &Path, executable: &Path, parent: &std::collections::HashMap<String, String>) -> std::collections::HashMap<String, String> {
    let mut result = std::collections::HashMap::new();
    for key in ["SystemRoot", "WINDIR", "ComSpec", "PATHEXT", "LANG", "LC_ALL", "TZ", "DISPLAY", "WAYLAND_DISPLAY"] {
        if let Some(value) = parent.get(key) {
            result.insert(key.to_string(), value.clone());
        }
    }
    let separator = if cfg!(target_os = "windows") { ";" } else { ":" };
    let node_directory = executable.parent().map(|path| path.display().to_string()).unwrap_or_default();
    result.insert(
        "PATH".to_string(),
        format!("{node_directory}{separator}{}", parent.get("PATH").cloned().unwrap_or_default()),
    );
    let home = root.join("home");
    result.insert("HOME".to_string(), home.display().to_string());
    result.insert("USERPROFILE".to_string(), home.display().to_string());
    result.insert("DSH_HOME".to_string(), root.join("dsh").display().to_string());
    result.insert("DSH_AGENTS_HOME".to_string(), root.join("agents").display().to_string());
    let tmp = root.join("tmp");
    result.insert("TMPDIR".to_string(), tmp.display().to_string());
    result.insert("TMP".to_string(), tmp.display().to_string());
    result.insert("TEMP".to_string(), tmp.display().to_string());
    result.insert("APPDATA".to_string(), root.join("appdata").display().to_string());
    result.insert("LOCALAPPDATA".to_string(), root.join("localappdata").display().to_string());
    result
}

/// Drop the Windows `\\?\` verbatim prefix.
///
/// The prefix is a filesystem-API detail, not part of a path: `\\?\D:\a\b`
/// names the same file as `D:\a\b`, but it is not a valid `file:` URL path, and
/// it leaked into every string handed to Node. Tauri creates this shape
/// unconditionally because `tauri_utils::platform::resource_dir` canonicalizes
/// the executable path on Windows, so the carrier sees `\\?\` on every launch.
pub fn simplify_verbatim(path: &Path) -> PathBuf {
    if !cfg!(target_os = "windows") {
        return path.to_path_buf();
    }
    let text = path.as_os_str().to_string_lossy();
    let Some(rest) = text.strip_prefix(r"\\?\") else {
        return path.to_path_buf();
    };
    let plain = match rest.strip_prefix("UNC\\") {
        Some(share) => format!(r"\\{share}"),
        None => rest.to_string(),
    };
    // Verbatim paths exist to lift MAX_PATH. When dropping the prefix would put
    // the path back over the limit, keep it: a long plain path fails in more
    // places than it fixes.
    if plain.chars().count() >= 260 {
        return path.to_path_buf();
    }
    PathBuf::from(plain)
}

/// file:// URL for an absolute path, matching Node's pathToFileURL.
pub fn path_to_file_url(path: &Path) -> String {
    let simplified = simplify_verbatim(path);
    let normalized = simplified.display().to_string().replace('\\', "/");
    // A UNC share already carries its own `//server/share` authority; a drive
    // path (`D:/…`) and a POSIX path (`/home/…`) need the third slash.
    let unc = cfg!(target_os = "windows") && normalized.starts_with("//");
    let mut text = String::from("file://");
    let body = if unc {
        normalized.trim_start_matches("//").to_string()
    } else {
        if cfg!(target_os = "windows") {
            text.push('/');
        }
        normalized
    };
    for byte in body.bytes() {
        // Path-safe ASCII stays literal — `:` in particular: Node's
        // `getPathFromURLWin32` checks `pathname[1] === ':'` *before* decoding,
        // so an escaped `%3A` drive letter still fails with 'must be absolute'.
        // Everything else is percent-encoded as UTF-8 bytes.
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'/'
                    | b':'
                    | b'@'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
            );
        if keep {
            text.push(byte as char);
        } else {
            text.push('%');
            text.push_str(&format!("{byte:02X}"));
        }
    }
    text
}

pub fn official_launcher(source: &Path, home: &Path, executable: &Path, package_manager: &Path) -> Result<PathBuf, String> {
    // Every argument below becomes a JavaScript literal. `\\?\` is a filesystem-API
    // detail: it breaks `file:` URLs and makes paths compare unequal to the same path
    // produced anywhere else (`process.cwd()`, `resolve()`, `__dirname`). Callers pass
    // already-normalized paths, but this is the boundary, so enforce it here too.
    let source = simplify_verbatim(source);
    let home = simplify_verbatim(home);
    let executable = simplify_verbatim(executable);
    let package_manager = simplify_verbatim(package_manager);
    let (source, home) = (source.as_path(), home.as_path());
    let cli = source.join("node_modules").join("@deepseek-ai").join("dsh").join("lib").join("bin.js");
    if !cli.exists() || !package_manager.exists() {
        return Err("安装包缺少官方 DSH 或锁定的插件安装工具".to_string());
    }
    let pnpm = cached_package_manager(home, &package_manager)?;
    let launcher = home.join(".quantdsh-launch.mjs");
    let script = format!(
        "import {{ runCli }} from {};\nawait runCli({{ manageDesktopProfile: true, packageManager: {{ command: {}, args: [{}] }} }});\n",
        crate::plugin_archive_cache::js_string(&path_to_file_url(&cli)),
        crate::plugin_archive_cache::js_string(&executable),
        // `JSON.stringify(pnpm)`: the runner is passed as a *path*, not a file URL.
        crate::plugin_archive_cache::js_string(&pnpm),
    );
    write_private(&launcher, script.as_bytes())?;
    Ok(launcher)
}

/** The official application-owned Profile API supplies the full installation dependency graph. */
pub fn official_host_launcher(source: &Path, home: &Path, executable: &Path, package_manager: &Path, patches: &[PathBuf]) -> Result<PathBuf, String> {
    // See `official_launcher`: these paths end up as JavaScript literals.
    let source = simplify_verbatim(source);
    let home = simplify_verbatim(home);
    let executable = simplify_verbatim(executable);
    let package_manager = simplify_verbatim(package_manager);
    let patches: Vec<PathBuf> = patches.iter().map(|path| simplify_verbatim(path)).collect();
    let (source, home) = (source.as_path(), home.as_path());
    let boot = source.join("node_modules").join("@deepseek-ai").join("dsh").join("lib").join("profile-boot.js");
    let app_boot = source.join("node_modules").join("@deepseek-ai").join("dsh-app-boot").join("lib").join("index.js");
    if !boot.exists() || !app_boot.exists() || !package_manager.exists() {
        return Err("安装包缺少官方 Profile 启动接口".to_string());
    }
    let anchor = source.join("profile-installation.json");
    if !anchor.exists() {
        return Err("安装包缺少完整官方与 QuantDSH 安装清单".to_string());
    }
    let profile = home.join("profiles").join("quantdsh");
    let launcher = home.join(".quantdsh-host.mjs");
    let pnpm = cached_package_manager(home, &package_manager)?;
    let patches_literal = {
        let items: Vec<String> = patches.iter().map(|path| crate::plugin_archive_cache::js_string(path)).collect();
        format!("[{}]", items.join(", "))
    };
    let script = format!(
        "import {{ runProfile }} from {boot};\nimport {{ loadProfileDirectory, loadLayeredEnv, reportSkippedBundles }} from {app_boot};\nconst installAnchor = {anchor};\nconst profile = loadProfileDirectory('dsh', {profile}, installAnchor);\nreportSkippedBundles('dsh', profile);\nawait runProfile({{ environment: loadLayeredEnv('dsh'), profile: 'quantdsh', resolvedProfile: {{ profile, installAnchor }}, patchFiles: {patches}, args: process.argv.slice(2), packageManager: {{ command: {executable}, args: [{pnpm}] }} }});\n",
        boot = crate::plugin_archive_cache::js_string(&path_to_file_url(&boot)),
        app_boot = crate::plugin_archive_cache::js_string(&path_to_file_url(&app_boot)),
        anchor = crate::plugin_archive_cache::js_string(&anchor),
        profile = crate::plugin_archive_cache::js_string(&profile),
        patches = patches_literal,
        executable = crate::plugin_archive_cache::js_string(&executable),
        // `JSON.stringify(packageManager)` after it was reassigned to the runner path.
        pnpm = crate::plugin_archive_cache::js_string(&pnpm),
    );
    write_private(&launcher, script.as_bytes())?;
    Ok(launcher)
}

pub struct EnterprisePatch {
    pub auth_file: PathBuf,
    pub patch: PathBuf,
}

pub fn desktop_enterprise_patch(
    home: &Path,
    authority: &crate::enterprise_auth::Authority,
    actor: &crate::enterprise_auth::EnterpriseMember,
    backend_url: &str,
    device_id: &str,
    collaboration: bool,
) -> Result<EnterprisePatch, String> {
    // The root enterprise bundle contributes the shared account client; only ./desktop supplies identity.
    let auth_file = home.join("enterprise-auth.json");
    write_json(
        &auth_file,
        &json!({
            "authorityUrl": authority.url,
            "authorityKey": authority.key,
            "backendUrl": backend_url,
            "principalId": actor.id,
            "organizationId": actor.organization_id,
            "deviceId": device_id,
        }),
    )?;
    let patch = home.join(".enterprise-desktop.patch.yml");
    // `join(home, '..', 'workspace')`: Node normalizes the `..` away, so the
    // Patch must not carry a dangling segment.
    let workspace = home
        .parent()
        .map(|parent| parent.join("workspace"))
        .unwrap_or_else(|| home.join("..").join("workspace"));
    let mut text = format!(
        "- id: quantdsh-identity-local\n  disabled: true\n- insert:\n    - id: quantdsh-enterprise-account-client\n      name: quantdsh-provider-identity-enterprise\n    - id: quantdsh-desktop-identity\n      name: quantdsh-provider-identity-enterprise/desktop\n      config:\n        authFile: {auth_file}\n        principalId: {principal_id}\n        organizationId: {organization_id}\n        deviceId: {device_id}\n",
        auth_file = crate::plugin_archive_cache::js_string(&auth_file),
        principal_id = crate::plugin_archive_cache::js_string(&actor.id),
        organization_id = crate::plugin_archive_cache::js_string(&actor.organization_id),
        device_id = crate::plugin_archive_cache::js_string(device_id),
    );
    if collaboration {
        text.push_str(
            "- insert:\n    - id: quantdsh-desktop-collaboration\n      name: quantdsh-plugin-enterprise-collaboration\n      config:\n        desktop: true\n",
        );
    }
    text.push_str(&format!(
        "- id: quantdsh-session-access\n  config:\n    autoBindFixedMemberSessions: true\n- id: quantdsh-tool-access\n  config:\n    autoBindPersonalSessions: false\n    autoBindFixedMemberSessions: true\n- id: workspace-controller\n  config:\n    documentsDirectory: {workspace}\n",
        workspace = crate::plugin_archive_cache::js_string(&workspace),
    ));
    write_private(&patch, text.as_bytes())?;
    Ok(EnterprisePatch { auth_file, patch })
}

#[cfg(test)]
mod tests {
    use super::{path_to_file_url, simplify_verbatim};
    use std::path::{Path, PathBuf};

    #[test]
    #[cfg(target_os = "windows")]
    fn verbatim_prefix_is_dropped() {
        assert_eq!(
            simplify_verbatim(Path::new(r"\\?\D:\code\dsh\profiles\quantdsh")),
            PathBuf::from(r"D:\code\dsh\profiles\quantdsh")
        );
        assert_eq!(
            simplify_verbatim(Path::new(r"\\?\UNC\server\share\quantdsh")),
            PathBuf::from(r"\\server\share\quantdsh")
        );
        // A plain path must pass through untouched.
        assert_eq!(
            simplify_verbatim(Path::new(r"D:\code\dsh")),
            PathBuf::from(r"D:\code\dsh")
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn file_url_of_a_verbatim_path_stays_absolute() {
        // Regression: `\\?\D:\…` used to encode as `file://///%3F/D%3A/…`, which
        // Node rejects with ERR_INVALID_FILE_URL_PATH ('must be absolute') the
        // moment the generated host launcher imports profile-boot.js — so every
        // "个人使用" click failed with 官方 DSH 未能完成启动.
        assert_eq!(
            path_to_file_url(Path::new(
                r"\\?\D:\code\github\dsh-tauri\src-tauri\target\release\quantdsh-runtime\profiles\quantdsh\node_modules\@deepseek-ai\dsh\lib\profile-boot.js"
            )),
            "file:///D:/code/github/dsh-tauri/src-tauri/target/release/quantdsh-runtime/profiles/quantdsh/node_modules/@deepseek-ai/dsh/lib/profile-boot.js"
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn file_url_shapes() {
        assert_eq!(
            path_to_file_url(Path::new(r"D:\a b\c#d.js")),
            "file:///D:/a%20b/c%23d.js"
        );
        // A verbatim UNC share keeps its authority instead of gaining a fourth slash.
        assert_eq!(
            path_to_file_url(Path::new(r"\\?\UNC\server\share\x")),
            "file://server/share/x"
        );
    }

    #[test]
    #[cfg(unix)]
    fn file_url_shapes() {
        assert_eq!(path_to_file_url(Path::new("/opt/a b/c.js")), "file:///opt/a%20b/c.js");
    }

    /// The generated host launcher is the exact text Node parses, so it — not
    /// `path_to_file_url` in isolation — is the real contract. Every `file:` URL in it
    /// must survive a `fileURLToPath` round trip, or the "个人使用" click fails with
    /// `ERR_INVALID_FILE_URL_PATH` before the Host is ever spawned.
    #[test]
    fn host_launcher_file_urls_resolve() {
        let root = std::env::temp_dir().join(format!("quantdsh-launcher-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source = root.join("quantdsh-runtime");
        let home = root.join("home");
        let dsh_lib = source.join("node_modules").join("@deepseek-ai").join("dsh").join("lib");
        let app_lib = source.join("node_modules").join("@deepseek-ai").join("dsh-app-boot").join("lib");
        std::fs::create_dir_all(&dsh_lib).expect("fixture");
        std::fs::create_dir_all(&app_lib).expect("fixture");
        std::fs::create_dir_all(&home).expect("fixture");
        std::fs::write(dsh_lib.join("profile-boot.js"), "").expect("fixture");
        std::fs::write(app_lib.join("index.js"), "").expect("fixture");
        std::fs::write(source.join("profile-installation.json"), "{}").expect("fixture");
        let pnpm_dir = source.join("dependencies").join("pnpm");
        std::fs::create_dir_all(pnpm_dir.join("bin")).expect("fixture");
        std::fs::write(pnpm_dir.join("package.json"), r#"{"name":"pnpm","version":"11.0.0"}"#)
            .expect("fixture");
        let pnpm = pnpm_dir.join("bin").join("pnpm.cjs");
        std::fs::write(&pnpm, "").expect("fixture");
        let executable = source.join("dependencies").join("node").join("node.exe");

        // Shipping shape on Windows: Tauri hands out canonicalized (verbatim) paths.
        let shipped = |path: &Path| {
            if cfg!(target_os = "windows") {
                PathBuf::from(format!(r"\\?\{}", path.display()))
            } else {
                path.to_path_buf()
            }
        };
        let launcher = super::official_host_launcher(
            &shipped(&source),
            &shipped(&home),
            &executable,
            &pnpm,
            &[PathBuf::from("patch.yml")],
        )
        .expect("host launcher");
        let script = std::fs::read_to_string(&launcher).expect("launcher script");

        assert!(!script.contains("%3F"), "verbatim prefix leaked: {script}");
        assert!(!script.contains("%3A"), "drive colon must stay literal: {script}");
        assert!(!script.contains(r"\\?\"), "verbatim path leaked: {script}");

        let mut urls = 0;
        let mut rest = script.as_str();
        while let Some(index) = rest.find("file://") {
            let tail = &rest[index..];
            let end = tail.find('"').expect("a file URL is always quoted");
            let candidate = &tail[..end];
            let url = url::Url::parse(candidate).expect("file URL parses");
            let path = url
                .to_file_path()
                .unwrap_or_else(|_| panic!("not an absolute local path: {candidate}"));
            assert!(path.is_absolute());
            urls += 1;
            rest = &tail[end..];
        }
        assert!(urls >= 2, "both boot imports must be file URLs: {script}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A profile whose declared packages lost their tree must ask for an
    /// installation instead of booting with silently missing plugins, and a
    /// profile that never declared anything must stay install-free — otherwise
    /// every launch would materialize a 646 MB duplicate of the installation.
    #[test]
    fn declared_dependencies_without_a_tree_force_installation() {
        let root = std::env::temp_dir().join(format!("quantdsh-profile-changed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source = root.join("quantdsh-runtime").join("profiles").join("quantdsh");
        let home = root.join("home");
        let dsh = source.join("node_modules").join("@deepseek-ai").join("dsh");
        std::fs::create_dir_all(&dsh).expect("fixture");
        std::fs::create_dir_all(&home).expect("fixture");
        std::fs::write(
            source.join("package.json"),
            r#"{"name":"quantdsh-desktop-installation","packageManager":"pnpm@11.28.5","dependencies":{}}"#,
        )
        .expect("fixture");
        std::fs::write(dsh.join("package.json"), r#"{"version":"0.2.1-rc.2"}"#).expect("fixture");

        // Fresh profile: the installation graph already covers every default.
        let fresh = super::materialize_runtime_profile(&source, &home).expect("materialize");
        assert!(!fresh.changed, "an empty dependency set must not trigger an installation");
        assert!(!fresh.profile.join("node_modules").exists());

        // The user adds a package, then the tree disappears (cleanup, failed move).
        std::fs::write(
            fresh.profile.join("package.json"),
            r#"{"name":"quantdsh-member-profile","private":true,"type":"module","packageManager":"pnpm@11.28.5","dependencies":{"left-pad":"1.3.0"}}"#,
        )
        .expect("fixture");
        let degraded = super::materialize_runtime_profile(&source, &home).expect("materialize");
        assert!(degraded.changed, "a declared package without node_modules must reinstall");

        // Once the install succeeds the profile is a steady state again
        // (`prepare_profile` marks it right after `plugin install` returns 0).
        std::fs::create_dir_all(degraded.profile.join("node_modules")).expect("fixture");
        super::mark_profile_updated(&source, &degraded.profile).expect("mark updated");
        let settled = super::materialize_runtime_profile(&source, &home).expect("materialize");
        assert!(!settled.changed, "an installed tree must not reinstall on every launch");

        // A failed install must keep the profile pending so the next launch retries.
        let pending: serde_json::Value =
            super::read_json(&settled.profile.join(super::BASE_STATE)).expect("state");
        assert_eq!(pending.get("pendingInstallation"), None);

        let _ = std::fs::remove_dir_all(&root);
    }
}
