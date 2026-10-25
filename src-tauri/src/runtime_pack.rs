//! Runtime pack location, verification and offline import.
//!
//! The Electron carrier never tolerates a partial runtime: `startRuntime` throws
//! `Bundled Node.js runtime is missing: <path>` and the window dies with a bare
//! error box. This module keeps that fail-closed contract and adds exactly one
//! recovery route — pick the first *verified* pack on disk, and let the user
//! import a release archive when none of them is complete.
//!
//! Nothing here changes the default start-up path. When the bundled pack is
//! complete, `RuntimePaths::resolve` returns the same paths it always returned;
//! the extra candidates are only consulted after the bundled tree fails its
//! checks. The system `node`/`python`/`pnpm` are deliberately never probed: the
//! Profile's `node_modules` is built for one exact Node ABI and one exact pnpm
//! store layout, so a version-equal but differently-built interpreter would fail
//! opaquely at `dlopen` time instead of at a readable check.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Must match `productName` and the `manifest.product` the release pipeline
/// writes; a pack for another product is rejected.
pub const PRODUCT: &str = "WorkDSH";
/// Marker file the official release ships at `<root>/primary-runtime/runtime.json`.
pub const RUNTIME_JSON: &str = "primary-runtime/runtime.json";
pub const MANIFEST_NAME: &str = "manifest.json";
pub const CURRENT_NAME: &str = "current.json";
pub const LOCK_NAME: &str = "install.lock";
pub const MANIFEST_PROTOCOL: u32 = 1;
/// A pack with more entries than this is not a WorkDSH runtime.
const MAX_ENTRIES: usize = 200_000;

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// The five paths the carrier dereferences before it can start a host process.
/// Mirrors `officialLauncher`/`officialHostLauncher` in workdsh's local-runtime.ts,
/// including the three component overrides the Electron carrier honours.
pub struct RuntimeLayout {
    pub root: PathBuf,
    pub profile: PathBuf,
    pub primary_runtime: PathBuf,
    pub node: PathBuf,
    pub pnpm_cli: PathBuf,
    pub python: PathBuf,
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn node_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "node.exe"
    } else {
        "node"
    }
}

fn python_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "python.exe"
    } else {
        "python3"
    }
}

/// `runtime.json`/manifest use Node's `process.arch` vocabulary.
fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        other => other,
    }
}

/// `runtime.json` and the manifest carry Node's `process.platform` (`win32`,
/// `darwin`), not `std::env::consts::OS` (`windows`, `macos`). Comparing the two
/// vocabularies directly would reject every genuine pack.
fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

fn target_label() -> String {
    let platform = match std::env::consts::OS {
        "windows" => "win",
        "macos" => "mac",
        other => other,
    };
    format!("{platform}-{}", host_arch())
}

impl RuntimeLayout {
    /// Resolve relative to a pack root, applying the same environment overrides
    /// as workdsh (`WORKDSH_BUNDLED_PROFILE`, `WORKDSH_PRIMARY_RUNTIME`,
    /// `WORKDSH_NODE_EXECUTABLE`). The overrides stay honoured so an administrator
    /// can move a single component without replacing the whole tree.
    pub fn resolve(root: &Path) -> Self {
        // Same `\\?\` normalisation as `RuntimePaths::resolve`; this entry point is
        // also reached directly (archive import, install promotion, tests).
        let root = crate::local_runtime::simplify_verbatim(root);
        let primary_runtime =
            env_dir("WORKDSH_PRIMARY_RUNTIME").unwrap_or_else(|| root.join("primary-runtime"));
        let profile = env_dir("WORKDSH_BUNDLED_PROFILE")
            .unwrap_or_else(|| root.join("profiles").join(crate::paths::PROFILE_NAME));
        let node = env_dir("WORKDSH_NODE_EXECUTABLE").unwrap_or_else(|| {
            primary_runtime
                .join("dependencies")
                .join("node")
                .join("bin")
                .join(node_file_name())
        });
        // Windows ships the interpreter directly under `dependencies/python`
        // (`python.exe`); macOS and Linux use the CPython `bin/python3` layout.
        // Appending the file name to the Windows *directory* produced
        // `python\python.exe\python.exe`, so every Windows launch judged a
        // complete runtime damaged and opened the repair surface instead of the
        // application.
        let python = if cfg!(target_os = "windows") {
            primary_runtime.join("dependencies").join("python").join(python_file_name())
        } else {
            primary_runtime
                .join("dependencies")
                .join("python")
                .join("bin")
                .join(python_file_name())
        };
        let pnpm_cli = primary_runtime
            .join("dependencies")
            .join("pnpm")
            .join("bin")
            .join("pnpm.cjs");
        Self {
            root,
            profile,
            primary_runtime,
            node,
            pnpm_cli,
            python,
        }
    }

    /// Cheap structural check: existence plus a platform match. No hashing — this
    /// runs on every launch, and the bundled tree is hundreds of megabytes.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if !self.root.is_dir() {
            problems.push(format!("运行时目录不存在：{}", self.root.display()));
            return problems;
        }
        match fs::read(self.root.join(RUNTIME_JSON)) {
            Err(_) => problems.push(format!("缺少 {RUNTIME_JSON}（安装包不完整或运行时已被删除）")),
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Err(_) => problems.push(format!("{RUNTIME_JSON} 无法解析")),
                Ok(value) => {
                    if let Some(platform) = value.get("platform").and_then(Value::as_str) {
                        if platform != node_platform() {
                            problems.push(format!(
                                "运行时平台为 {platform}，本机为 {}",
                                node_platform()
                            ));
                        }
                    }
                    if let Some(arch) = value.get("arch").and_then(Value::as_str) {
                        if arch != host_arch() {
                            problems.push(format!("运行时架构为 {arch}，本机为 {}", host_arch()));
                        }
                    }
                }
            },
        }
        for (label, path) in self.required_files() {
            if !path.exists() {
                problems.push(format!("缺少 {label}：{}", self.display_relative(&path)));
            }
        }
        problems
    }

    /// Everything `officialLauncher`/`officialHostLauncher` dereferences. The
    /// three `node_modules` entries are exactly the ones whose absence makes the
    /// Electron carrier throw `安装包缺少官方 Profile 启动接口`.
    fn required_files(&self) -> Vec<(&'static str, PathBuf)> {
        let packages = self.profile.join("node_modules");
        vec![
            ("Profile 清单", self.profile.join("package.json")),
            ("Profile 补丁基线", self.profile.join("cordis.patch.yml")),
            ("安装锚点", self.profile.join("profile-installation.json")),
            (
                "官方 DSH 启动器",
                packages
                    .join("@deepseek-ai")
                    .join("dsh")
                    .join("lib")
                    .join("bin.js"),
            ),
            (
                "官方 Profile 启动接口",
                packages
                    .join("@deepseek-ai")
                    .join("dsh")
                    .join("lib")
                    .join("profile-boot.js"),
            ),
            (
                "官方应用启动接口",
                packages
                    .join("@deepseek-ai")
                    .join("dsh-app-boot")
                    .join("lib")
                    .join("index.js"),
            ),
            ("Node.js 运行时", self.node.clone()),
            ("pnpm 安装工具", self.pnpm_cli.clone()),
            ("Python 运行时", self.python.clone()),
        ]
    }

    fn display_relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }
}

// ---------------------------------------------------------------------------
// Location
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// `WORKDSH_RUNTIME_DIR` — an explicit administrator choice.
    Override,
    /// `<resources>/workdsh-runtime`, shipped inside the installer.
    Bundled,
    /// `<userData>/runtime/versions/<digest>`, produced by an offline import.
    Installed,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Override => "WORKDSH_RUNTIME_DIR 指定的运行时",
            Self::Bundled => "安装包内置运行时",
            Self::Installed => "已导入的运行时",
        }
    }
}

pub struct RuntimePack {
    pub layout: RuntimeLayout,
    pub source: Source,
    /// Empty means the pack is usable.
    pub problems: Vec<String>,
    /// Where an import would land (`<userData>/runtime`).
    pub install_root: PathBuf,
}

/// First *verified* candidate wins. Electron would simply throw on the bundled
/// tree; verifying each candidate in turn is what makes a damaged installation
/// recoverable without weakening the all-or-nothing contract.
pub fn locate(resource_dir: &Path, user_data: &Path) -> RuntimePack {
    let install_root = user_data.join("runtime");

    // An explicit override must not silently fall through: if an administrator
    // pointed WORKDSH_RUNTIME_DIR at something, starting from a different tree
    // would hide the mistake.
    if let Some(root) = env_dir("WORKDSH_RUNTIME_DIR") {
        let layout = RuntimeLayout::resolve(&root);
        let problems = layout.problems();
        return RuntimePack {
            layout,
            source: Source::Override,
            problems,
            install_root,
        };
    }

    let bundled = RuntimeLayout::resolve(&resource_dir.join("workdsh-runtime"));
    let bundled_problems = bundled.problems();
    if bundled_problems.is_empty() {
        return RuntimePack {
            layout: bundled,
            source: Source::Bundled,
            problems: bundled_problems,
            install_root,
        };
    }

    if let Some(root) = installed_root(&install_root) {
        let layout = RuntimeLayout::resolve(&root);
        let problems = layout.problems();
        if problems.is_empty() {
            return RuntimePack {
                layout,
                source: Source::Installed,
                problems,
                install_root,
            };
        }
    }

    // Nothing usable: report against the bundled layout, which is what an
    // import would repair for every user who did not set an override.
    RuntimePack {
        layout: bundled,
        source: Source::Bundled,
        problems: bundled_problems,
        install_root,
    }
}

/// Resolve `current.json` → `versions/<digest>`. A dangling pointer is not an
/// error here: it simply means "no installed pack", and the caller falls back.
fn installed_root(install_root: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(install_root.join(CURRENT_NAME)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    if value.get("protocol").and_then(Value::as_u64) != Some(MANIFEST_PROTOCOL as u64) {
        return None;
    }
    let digest = value.get("digest").and_then(Value::as_str)?;
    if !is_hex_digest(digest) {
        return None;
    }
    let directory = install_root.join("versions").join(digest);
    directory.is_dir().then_some(directory)
}

pub fn is_hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Where the guide page sends users when no pack can be found. Deliberately not
/// hard-coded: a wrong URL is worse than an honest "not configured".
pub fn download_url() -> Option<String> {
    env_dir("WORKDSH_RUNTIME_DOWNLOAD_URL")
        .map(|path| path.display().to_string())
        .filter(|value| value.starts_with("https://") || value.starts_with("http://"))
}

// ---------------------------------------------------------------------------
// Offline import
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Manifest {
    protocol: u32,
    product: String,
    version: String,
    target: String,
    runtime: ManifestRuntime,
    entries: Vec<ManifestEntry>,
}

#[derive(Serialize, Deserialize)]
struct ManifestRuntime {
    platform: String,
    arch: String,
    node: String,
    pnpm: String,
    python: String,
}

#[derive(Serialize, Deserialize)]
struct ManifestEntry {
    path: String,
    size: u64,
    sha256: String,
}

#[derive(Serialize)]
struct Current {
    protocol: u32,
    digest: String,
    version: String,
    target: String,
    installed_at: u64,
}

#[derive(Debug)]
pub struct ImportReport {
    pub digest: String,
    pub version: String,
    pub root: PathBuf,
}

/// Extract, verify and promote an offline archive into `<install_root>/versions`.
///
/// Order matters: everything lands in a staging directory on the *same volume*,
/// every byte is checked against `manifest.json`, only then is the directory
/// renamed into place and `current.json` replaced atomically. A crash at any
/// point leaves either the old pointer or no pointer — never a half-installed
/// pack that would later pass the structural check.
pub fn import_archive(archive: &Path, install_root: &Path) -> Result<ImportReport, String> {
    if !archive.is_file() {
        return Err(format!("找不到离线包：{}", archive.display()));
    }
    fs::create_dir_all(install_root).map_err(|error| format!("无法创建运行时目录：{error}"))?;
    let _lock = InstallLock::acquire(&install_root.join(LOCK_NAME))?;

    let staging = install_root.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    if let Err(error) = fs::create_dir_all(&staging) {
        return Err(format!("无法创建暂存目录：{error}"));
    }
    let outcome = install_into(archive, install_root, &staging);
    if outcome.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    outcome
}

fn install_into(
    archive: &Path,
    install_root: &Path,
    staging: &Path,
) -> Result<ImportReport, String> {
    extract(archive, staging)?;

    let manifest_bytes = fs::read(staging.join(MANIFEST_NAME))
        .map_err(|_| format!("离线包缺少 {MANIFEST_NAME}，可能不是 WorkDSH 运行时包"))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("{MANIFEST_NAME} 无法解析：{error}"))?;
    validate_manifest(&manifest)?;
    verify_entries(&manifest, staging)?;

    // The archive must actually satisfy the launcher, not merely hash-match.
    let staged = RuntimeLayout::resolve(staging);
    let problems = staged.problems();
    if !problems.is_empty() {
        return Err(format!("离线包内容不完整：{}", problems.join("；")));
    }

    let digest = hex::encode(Sha256::digest(&manifest_bytes));
    let target = install_root.join("versions").join(&digest);
    if target.is_dir() {
        // Same content already promoted: keep the verified copy.
        let _ = fs::remove_dir_all(staging);
    } else {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建版本目录：{error}"))?;
        }
        fs::rename(staging, &target)
            .map_err(|error| format!("无法写入版本目录 {}：{error}", target.display()))?;
    }

    let current = Current {
        protocol: MANIFEST_PROTOCOL,
        digest: digest.clone(),
        version: manifest.version.clone(),
        target: manifest.target.clone(),
        installed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default(),
    };
    let body = serde_json::to_vec_pretty(&current)
        .map_err(|error| format!("无法序列化 {CURRENT_NAME}：{error}"))?;
    write_atomic(&install_root.join(CURRENT_NAME), &body)?;
    prune_versions(install_root, &digest);

    Ok(ImportReport {
        digest,
        version: manifest.version,
        root: target,
    })
}

fn extract(archive: &Path, staging: &Path) -> Result<(), String> {
    let file = fs::File::open(archive).map_err(|error| format!("无法读取离线包：{error}"))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(io::BufReader::new(file)));
    let entries = tar
        .entries()
        .map_err(|error| format!("离线包不是有效的 tar 数据：{error}"))?;
    let mut count = 0usize;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("读取离线包条目失败：{error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("读取离线包路径失败：{error}"))?
            .into_owned();
        // `tar` records the archive root itself as `./`; it carries no content.
        if is_archive_root(&path) {
            continue;
        }
        let relative =
            safe_relative(&path).ok_or_else(|| format!("离线包含有非法路径：{}", path.display()))?;
        let destination = staging.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建目录：{error}"))?;
        }
        entry
            .unpack(&destination)
            .map_err(|error| format!("解包 {} 失败：{error}", relative.display()))?;
        count += 1;
        if count > MAX_ENTRIES {
            return Err("离线包条目数异常，已中止".to_string());
        }
    }
    Ok(())
}

fn validate_manifest(manifest: &Manifest) -> Result<(), String> {
    if manifest.protocol != MANIFEST_PROTOCOL {
        return Err(format!(
            "离线包协议版本为 {}，本版本仅支持 {MANIFEST_PROTOCOL}",
            manifest.protocol
        ));
    }
    if manifest.product != PRODUCT {
        return Err(format!(
            "离线包属于 {}，不能用于 {PRODUCT}",
            manifest.product
        ));
    }
    let expected = target_label();
    if manifest.target != expected {
        return Err(format!(
            "离线包目标平台为 {}，本机为 {expected}",
            manifest.target
        ));
    }
    if manifest.runtime.platform != node_platform() || manifest.runtime.arch != host_arch() {
        return Err(format!(
            "离线包运行时为 {}/{}，本机为 {}/{}",
            manifest.runtime.platform,
            manifest.runtime.arch,
            node_platform(),
            host_arch()
        ));
    }
    if manifest.entries.is_empty() {
        return Err(format!("{MANIFEST_NAME} 没有列出任何文件"));
    }
    Ok(())
}

fn verify_entries(manifest: &Manifest, staging: &Path) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for entry in &manifest.entries {
        let relative = safe_relative(Path::new(&entry.path))
            .ok_or_else(|| format!("{MANIFEST_NAME} 含有非法路径：{}", entry.path))?;
        if !seen.insert(relative.clone()) {
            return Err(format!("{MANIFEST_NAME} 重复列出 {}", entry.path));
        }
        if !is_hex_digest(&entry.sha256) {
            return Err(format!("{} 的摘要格式不正确", entry.path));
        }
        let path = staging.join(&relative);
        let metadata =
            fs::metadata(&path).map_err(|_| format!("离线包缺少清单中的 {}", entry.path))?;
        if !metadata.is_file() {
            return Err(format!("{} 不是普通文件", entry.path));
        }
        if metadata.len() != entry.size {
            return Err(format!(
                "{} 大小不符（清单 {}，实际 {}）",
                entry.path,
                entry.size,
                metadata.len()
            ));
        }
        let actual = sha256_file(&path)?;
        if !actual.eq_ignore_ascii_case(&entry.sha256) {
            return Err(format!("{} 校验失败，离线包可能已损坏", entry.path));
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn write_atomic(path: &Path, body: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&temporary)
            .map_err(|error| format!("无法写入 {}：{error}", temporary.display()))?;
        file.write_all(body)
            .map_err(|error| format!("无法写入 {}：{error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("无法刷新 {}：{error}", temporary.display()))?;
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("无法替换 {}：{error}", path.display())
    })
}

/// Keep exactly the promoted pack. Rollback is "import the previous archive",
/// which is a deliberate user action rather than an automatic guess.
fn prune_versions(install_root: &Path, keep: &str) {
    let Ok(entries) = fs::read_dir(install_root.join("versions")) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == keep || !is_hex_digest(&name) {
            continue;
        }
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Reject absolute paths, `..`, prefixes and platform prefixes. `tar` already
/// refuses to escape the destination, but the manifest paths are ours to police.
fn safe_relative(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// `./` — the archive root entry `tar` always writes. It has no content and no
/// name, so it is skipped rather than rejected.
fn is_archive_root(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::CurDir))
}

struct InstallLock {
    path: PathBuf,
}

impl InstallLock {
    /// `create_new` is the whole protocol: a crashed install leaves the lock
    /// behind on purpose. Guessing whether the owner is alive is how two
    /// extractors end up writing the same directory.
    fn acquire(path: &Path) -> Result<Self, String> {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => {
                let _ = writeln!(file, "{}", std::process::id());
                Ok(Self {
                    path: path.to_path_buf(),
                })
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(format!(
                "另一个安装进程正在写入运行时（锁文件：{}）。确认没有安装在进行后，删除该文件再重试。",
                path.display()
            )),
            Err(error) => Err(format!("无法创建安装锁：{error}")),
        }
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "workdsh-runtime-pack-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).expect("temp dir");
        directory
    }

    fn write(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(path, b"x").expect("write");
    }

    /// Build the smallest tree that satisfies `RuntimeLayout::problems`.
    fn complete_pack(root: &Path) {
        fs::create_dir_all(root).expect("root");
        let marker = root.join(RUNTIME_JSON);
        if let Some(parent) = marker.parent() {
            fs::create_dir_all(parent).expect("runtime dir");
        }
        fs::write(
            &marker,
            format!(
                "{{\"platform\":\"{}\",\"arch\":\"{}\"}}",
                node_platform(),
                host_arch()
            ),
        )
        .expect("runtime.json");
        let layout = RuntimeLayout::resolve(root);
        for (_, path) in layout.required_files() {
            write(&path);
        }
    }

    fn write_current(install_root: &Path, digest: &str) {
        fs::create_dir_all(install_root).expect("install root");
        fs::write(
            install_root.join(CURRENT_NAME),
            format!("{{\"protocol\":{MANIFEST_PROTOCOL},\"digest\":\"{digest}\"}}"),
        )
        .expect("current.json");
    }

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        assert!(safe_relative(Path::new("profiles/workdsh/package.json")).is_some());
        assert!(safe_relative(Path::new("./manifest.json")).is_some());
        assert!(safe_relative(Path::new("../escape")).is_none());
        assert!(safe_relative(Path::new("/etc/passwd")).is_none());
        assert!(safe_relative(Path::new("")).is_none());
    }

    #[test]
    fn digest_detection_is_strict() {
        assert!(is_hex_digest(&"a".repeat(64)));
        assert!(!is_hex_digest(&"a".repeat(63)));
        assert!(!is_hex_digest(&"g".repeat(64)));
    }

    #[test]
    fn complete_pack_has_no_problems() {
        let root = temp_dir("complete");
        complete_pack(&root);
        let layout = RuntimeLayout::resolve(&root);
        assert_eq!(layout.problems(), Vec::<String>::new());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn layout_resolves_the_shipped_interpreter() {
        // Derived from documentation, not from `required_files()`: the existing
        // completeness test writes whatever the layout names, so it cannot notice a
        // wrong path. Joining `python.exe` as a directory and then appending the file
        // name again gave `python/python.exe/python.exe` on Windows, and every launch
        // then rejected a complete runtime and opened the repair surface.
        let root = temp_dir("layout");
        let layout = RuntimeLayout::resolve(&root);
        let python = layout.python.to_string_lossy().replace('\\', "/");
        let expected = if cfg!(target_os = "windows") {
            "/dependencies/python/python.exe"
        } else {
            "/dependencies/python/bin/python3"
        };
        assert!(python.ends_with(expected), "unexpected interpreter path: {python}");
        let pnpm = layout.pnpm_cli.to_string_lossy().replace('\\', "/");
        assert!(pnpm.ends_with("/dependencies/pnpm/bin/pnpm.cjs"), "unexpected pnpm path: {pnpm}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_component_is_named() {
        let root = temp_dir("missing");
        complete_pack(&root);
        let layout = RuntimeLayout::resolve(&root);
        fs::remove_file(&layout.node).expect("remove node");
        let problems = RuntimeLayout::resolve(&root).problems();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("Node.js"), "{problems:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn wrong_platform_is_reported() {
        let root = temp_dir("platform");
        complete_pack(&root);
        let foreign = if node_platform() == "win32" {
            "darwin"
        } else {
            "win32"
        };
        fs::write(
            root.join(RUNTIME_JSON),
            format!("{{\"platform\":\"{foreign}\",\"arch\":\"{}\"}}", host_arch()),
        )
        .expect("runtime.json");
        let problems = RuntimeLayout::resolve(&root).problems();
        assert!(
            problems.iter().any(|item| item.contains("平台")),
            "{problems:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn located_pack_prefers_the_bundled_tree() {
        let resource = temp_dir("resource");
        let user_data = temp_dir("user-data");
        complete_pack(&resource.join("workdsh-runtime"));
        let pack = locate(&resource, &user_data);
        assert!(pack.problems.is_empty(), "{:?}", pack.problems);
        assert_eq!(pack.source, Source::Bundled);
        let _ = fs::remove_dir_all(&resource);
        let _ = fs::remove_dir_all(&user_data);
    }

    #[test]
    fn located_pack_falls_back_to_an_imported_version() {
        let resource = temp_dir("resource-fallback");
        let user_data = temp_dir("user-data-fallback");
        let digest = "b".repeat(64);
        let installed = user_data.join("runtime").join("versions").join(&digest);
        complete_pack(&installed);
        write_current(&user_data.join("runtime"), &digest);
        let pack = locate(&resource, &user_data);
        assert!(pack.problems.is_empty(), "{:?}", pack.problems);
        assert_eq!(pack.source, Source::Installed);
        assert_eq!(pack.layout.root, installed);
        let _ = fs::remove_dir_all(&resource);
        let _ = fs::remove_dir_all(&user_data);
    }

    #[test]
    fn dangling_pointer_does_not_hide_a_broken_bundle() {
        let resource = temp_dir("resource-dangling");
        let user_data = temp_dir("user-data-dangling");
        write_current(&user_data.join("runtime"), &"c".repeat(64));
        let pack = locate(&resource, &user_data);
        assert!(!pack.problems.is_empty());
        assert_eq!(pack.source, Source::Bundled);
        let _ = fs::remove_dir_all(&resource);
        let _ = fs::remove_dir_all(&user_data);
    }

    #[test]
    fn manifest_for_another_product_is_rejected() {
        let manifest = Manifest {
            protocol: MANIFEST_PROTOCOL,
            product: "QuantStudio".to_string(),
            version: "1.0.0".to_string(),
            target: target_label(),
            runtime: ManifestRuntime {
                platform: node_platform().to_string(),
                arch: host_arch().to_string(),
                node: "24.21.0".to_string(),
                pnpm: "11.28.5".to_string(),
                python: "3.12.14".to_string(),
            },
            entries: vec![ManifestEntry {
                path: MANIFEST_NAME.to_string(),
                size: 1,
                sha256: "d".repeat(64),
            }],
        };
        assert!(validate_manifest(&manifest).is_err());
        let mut local = manifest;
        local.product = PRODUCT.to_string();
        assert!(validate_manifest(&local).is_ok());
    }

    #[test]
    fn import_lock_is_exclusive_and_released() {
        let root = temp_dir("lock");
        let path = root.join(LOCK_NAME);
        let first = InstallLock::acquire(&path).expect("first lock");
        assert!(InstallLock::acquire(&path).is_err());
        drop(first);
        assert!(InstallLock::acquire(&path).is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    /// Build `<pack>/manifest.json` plus a `.tar.gz` around it. `tamper` writes a
    /// digest that cannot match, so the verification stage must refuse.
    fn build_archive(pack: &Path, archive: &Path, tamper: bool) {
        let mut files = Vec::new();
        let mut stack = vec![pack.to_path_buf()];
        while let Some(directory) = stack.pop() {
            for entry in fs::read_dir(&directory).expect("read_dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files.sort();
        let mut entries = Vec::new();
        for path in &files {
            let relative = path
                .strip_prefix(pack)
                .expect("relative")
                .to_string_lossy()
                .replace('\\', "/");
            let mut digest = sha256_file(path).expect("digest");
            if tamper && relative.ends_with("package.json") {
                digest = "0".repeat(64);
            }
            entries.push(serde_json::json!({
                "path": relative,
                "size": fs::metadata(path).expect("metadata").len(),
                "sha256": digest,
            }));
        }
        let manifest = serde_json::json!({
            "protocol": MANIFEST_PROTOCOL,
            "product": PRODUCT,
            "version": "3.0.0-alpha.3",
            "target": target_label(),
            "runtime": {
                "platform": node_platform(),
                "arch": host_arch(),
                "node": "24.21.0",
                "pnpm": "11.28.5",
                "python": "3.12.14",
            },
            "entries": entries,
        });
        fs::write(
            pack.join(MANIFEST_NAME),
            serde_json::to_vec_pretty(&manifest).expect("manifest"),
        )
        .expect("write manifest");

        let file = fs::File::create(archive).expect("archive");
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        builder.append_dir_all(".", pack).expect("append");
        builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip");
    }

    #[test]
    fn import_promotes_a_verified_archive() {
        let workspace = temp_dir("import");
        let pack = workspace.join("pack");
        complete_pack(&pack);
        let archive = workspace.join("runtime.tar.gz");
        build_archive(&pack, &archive, false);

        let user_data = workspace.join("user-data");
        let install_root = user_data.join("runtime");
        let report = import_archive(&archive, &install_root).expect("import");
        assert_eq!(report.version, "3.0.0-alpha.3");
        assert!(is_hex_digest(&report.digest));
        assert_eq!(report.root, install_root.join("versions").join(&report.digest));
        assert!(report.root.join(MANIFEST_NAME).is_file());
        assert!(report.root.join(RUNTIME_JSON).is_file());
        // No staging directory survives a successful import.
        let leftovers: Vec<_> = fs::read_dir(&install_root)
            .expect("install root")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".staging-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        // The promoted pack is what a fresh launch would pick, with no bundled tree.
        let empty_resource = workspace.join("resources");
        fs::create_dir_all(&empty_resource).expect("resources");
        let located = locate(&empty_resource, &user_data);
        assert!(located.problems.is_empty(), "{:?}", located.problems);
        assert_eq!(located.source, Source::Installed);

        // Re-importing the same archive is idempotent.
        let again = import_archive(&archive, &install_root).expect("re-import");
        assert_eq!(again.digest, report.digest);
        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn import_rejects_a_tampered_archive() {
        let workspace = temp_dir("tamper");
        let pack = workspace.join("pack");
        complete_pack(&pack);
        let archive = workspace.join("runtime.tar.gz");
        build_archive(&pack, &archive, true);
        let install_root = workspace.join("runtime");
        let error = import_archive(&archive, &install_root).expect_err("must reject");
        assert!(error.contains("校验失败"), "{error}");
        assert!(!install_root.join(CURRENT_NAME).exists());
        let _ = fs::remove_dir_all(&workspace);
    }
}
