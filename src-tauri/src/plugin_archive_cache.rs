use std::fs;
use std::path::{Path, PathBuf};

/// Delegate to the bundled pnpm after retaining local archives in this account's Home.
pub fn cached_package_manager(home: &Path, pnpm: &Path) -> Result<PathBuf, String> {
    let manifest_path = pnpm
        .parent()
        .and_then(Path::parent)
        .map(|parent| parent.join("package.json"))
        .ok_or_else(|| "内置插件安装工具清单无效".to_string())?;
    let manifest: serde_json::Value = serde_json::from_str(&fs::read_to_string(&manifest_path).map_err(|_| "内置插件安装工具清单无效".to_string())?)
        .map_err(|_| "内置插件安装工具清单无效".to_string())?;
    let name = manifest.get("name").and_then(serde_json::Value::as_str);
    let version = manifest.get("version").and_then(serde_json::Value::as_str);
    if name != Some("pnpm") || !version.is_some_and(|value| semver_like(value)) {
        return Err("内置插件安装工具清单无效".to_string());
    }
    let version = version.unwrap_or_default();
    // The application supplies this executable. A Profile's packageManager pin
    // must not make pnpm fetch or run another engine before the official command.
    let fixed_manager: &[&str] = if version.split('.').next().and_then(|major| major.parse::<u32>().ok()).unwrap_or(0) >= 11 {
        &["--pm-on-fail=ignore", "--config.verify-deps-before-run=false"]
    } else {
        &["--config.manage-package-manager-versions=false", "--config.package-manager-strict=false"]
    };
    let runner = home.join(".quantdsh-package-manager.mjs");
    let pnpm_literal = js_string(pnpm);
    let fixed_literal = js_string_array(fixed_manager);
    let script = format!(
        r#"import {{ createHash }} from 'node:crypto';
import {{ mkdirSync, readFileSync, writeFileSync, statSync }} from 'node:fs';
import {{ join, resolve }} from 'node:path';
import {{ fileURLToPath }} from 'node:url';
import {{ spawn }} from 'node:child_process';
const cache = join(fileURLToPath(new URL('.', import.meta.url)), 'plugin-archives');
const args = process.argv.slice(2);
if (args.includes('add')) {{
  for (let i = 0; i < args.length; i++) {{
    const spec = args[i];
    if (!/\.(tgz|tar\.gz)$/i.test(spec) || /^(https?:|npm:)/i.test(spec)) continue;
    const source = spec.startsWith('file://') ? fileURLToPath(spec) : resolve(spec.startsWith('file:') ? spec.slice(5) : spec);
    if (!statSync(source).isFile()) throw new Error('Plugin archive is not a file: ' + source);
    const bytes = readFileSync(source);
    const digest = createHash('sha256').update(bytes).digest('hex');
    mkdirSync(cache, {{ recursive: true, mode: 0o700 }});
    const destination = join(cache, digest + '.tgz');
    try {{ writeFileSync(destination, bytes, {{ flag: 'wx', mode: 0o600 }}); }}
    catch (error) {{ if (error.code !== 'EEXIST') throw error; }}
    if (createHash('sha256').update(readFileSync(destination)).digest('hex') !== digest) throw new Error('Plugin archive cache integrity failure');
    args[i] = destination;
  }}
}}
const child = spawn(process.execPath, [{pnpm_literal}, ...{fixed_literal}, ...args], {{ stdio: 'inherit', windowsHide: true }});
for (const signal of ['SIGTERM', 'SIGINT']) process.on(signal, () => child.kill(signal));
child.on('error', error => {{ console.error(error.message); process.exitCode = 1; }});
child.on('exit', (code, signal) => {{ process.exitCode = code ?? (signal ? 1 : 0); }});
"#
    );
    fs::write(&runner, script).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&runner, fs::Permissions::from_mode(0o600));
    }
    Ok(runner)
}

fn semver_like(value: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN
        .get_or_init(|| regex::Regex::new(r"^\d+\.\d+\.\d+(?:-[\w.-]+)?$").unwrap())
        .is_match(value)
}

/// JSON.stringify for a filesystem path or string, without depending on the
/// value's own escaping. Accepts `&str`, `&Path` and `&PathBuf` alike, because
/// the embedded script interpolates both plain literals and resolved paths.
pub fn js_string<P: AsRef<Path>>(value: P) -> String {
    let path: &Path = value.as_ref();
    serde_json::to_string(&path.to_string_lossy()).unwrap_or_else(|_| "\"\"".to_string())
}

pub fn js_string_array(values: &[&str]) -> String {
    let items: Vec<String> = values.iter().map(|value| js_string(value)).collect();
    format!("[{}]", items.join(", "))
}
