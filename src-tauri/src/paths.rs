use std::path::{Path, PathBuf};

pub const PROFILE_NAME: &str = "quantdsh";

/// Electron parity: `app.getPath('userData')` = %APPDATA%\QuantDSH on Windows and
/// ~/Library/Application Support/QuantDSH on macOS. The installed `dsh` command
/// (runtime/cli/command-cli.js) locates this exact directory, so the carrier must
/// resolve it identically. `QUANTDSH_DESKTOP_USER_DATA` must be absolute.
///
/// Electron also honours Chromium's `--user-data-dir` switch, which quantdsh's own
/// installer probe relies on to isolate a run; the flag is accepted here too.
pub fn user_data_dir() -> Result<PathBuf, String> {
    if let Some(overridden) = env_path("QUANTDSH_DESKTOP_USER_DATA") {
        if !overridden.is_absolute() {
            return Err("Desktop user data override must be absolute".to_string());
        }
        return Ok(overridden);
    }
    if let Some(from_flag) = user_data_flag() {
        return Ok(from_flag);
    }
    if cfg!(target_os = "windows") {
        let appdata = std::env::var_os("APPDATA").ok_or_else(|| "APPDATA is not set".to_string())?;
        return Ok(PathBuf::from(appdata).join("QuantDSH"));
    }
    if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
        return Ok(PathBuf::from(home).join("Library").join("Application Support").join("QuantDSH"));
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        });
    Ok(config.join("QuantDSH"))
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Chromium's `--user-data-dir=<dir>` / `--user-data-dir <dir>`.
fn user_data_flag() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if let Some(value) = argument.strip_prefix("--user-data-dir=") {
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
        if argument == "--user-data-dir" {
            if let Some(value) = args.next() {
                if !value.is_empty() {
                    return Some(PathBuf::from(value));
                }
            }
        }
    }
    None
}

/// Every filesystem location the carrier touches, resolved once at startup.
/// Covers the bundled profile directory, the bundled primary runtime, the bundled
/// Node executable and the runtime home, including the environment overrides.
///
/// `runtime_root` is no longer hard-wired to `<resources>/quantdsh-runtime`: it is
/// whichever pack [`crate::runtime_pack::locate`] verified first. When the bundled
/// tree is complete that is exactly the same path as before, so start-up is
/// unchanged.
pub struct RuntimePaths {
    pub resource_dir: PathBuf,
    pub user_data: PathBuf,
    pub runtime_root: PathBuf,
    pub bundled_profile: PathBuf,
    pub primary_runtime: PathBuf,
    pub node_executable: PathBuf,
    /// Bundled CPython, resolved for parity with the released runtime layout.
    /// The Electron carrier never reads it either — only the release verifiers
    /// assert that the interpreter shipped with the profile.
    #[allow(dead_code)]
    pub python_executable: PathBuf,
    pub pnpm_cli: PathBuf,
    pub dsh_home: PathBuf,
    pub log_directory: PathBuf,
    /// Non-empty when no complete runtime pack could be located. The carrier then
    /// opens the repair surface instead of starting a host process against a
    /// partial tree, which is what the Electron carrier's `existsSync` throw did.
    pub runtime_problems: Vec<String>,
    /// Which candidate produced `runtime_root`, for the repair surface.
    pub runtime_source: crate::runtime_pack::Source,
    /// `<userData>/runtime`, where an offline import lands.
    pub runtime_install_root: PathBuf,
}

impl RuntimePaths {
    pub fn resolve(resource_dir: &Path, user_data: &Path) -> Result<Self, String> {
        // Tauri canonicalizes the executable before deriving `resource_dir`, so on
        // Windows this arrives as `\\?\D:\…\quantdsh-runtime`. Every location below
        // is derived from it and every one of them ends up as an argument to Node
        // (profile path, install anchor, package manager), so normalise once here
        // instead of patching each consumer.
        let resource_dir = crate::local_runtime::simplify_verbatim(resource_dir);
        let user_data = crate::local_runtime::simplify_verbatim(user_data);
        let pack = crate::runtime_pack::locate(&resource_dir, &user_data);
        Self::from_pack(pack, &resource_dir, &user_data)
    }

    fn from_pack(
        pack: crate::runtime_pack::RuntimePack,
        resource_dir: &Path,
        user_data: &Path,
    ) -> Result<Self, String> {
        let crate::runtime_pack::RuntimePack {
            layout,
            source,
            problems,
            install_root,
        } = pack;
        let dsh_home = env_path("QUANTDSH_DSH_HOME").unwrap_or_else(|| user_data.join("dsh-home"));
        Ok(Self {
            resource_dir: resource_dir.to_path_buf(),
            user_data: user_data.to_path_buf(),
            python_executable: layout.python,
            pnpm_cli: layout.pnpm_cli,
            runtime_root: layout.root,
            bundled_profile: layout.profile,
            primary_runtime: layout.primary_runtime,
            node_executable: layout.node,
            dsh_home,
            log_directory: user_data.join("logs"),
            runtime_problems: problems,
            runtime_source: source,
            runtime_install_root: install_root,
        })
    }

    pub fn command_context_file(&self) -> PathBuf {
        self.user_data.join("command-context.json")
    }

    pub fn enterprise_backend_file(&self) -> PathBuf {
        self.user_data.join("enterprise-backend.json")
    }

    pub fn deployment_config_file(&self) -> PathBuf {
        self.resource_dir.join("quantdsh-config.json")
    }
}
