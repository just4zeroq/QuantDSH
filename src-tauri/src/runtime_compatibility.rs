use regex::Regex;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

fn package_version_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"^(?:@[a-z0-9][a-z0-9._-]*/)?[a-z0-9][a-z0-9._-]*@(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$").unwrap()
    })
}

fn exact_version_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$").unwrap()
    })
}

type Grants = BTreeMap<String, Vec<String>>;

fn read_grants(filename: &Path) -> Result<Grants, String> {
    let text = fs::read_to_string(filename).map_err(|error| error.to_string())?;
    let parsed: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let Some(object) = parsed.as_object() else {
        return Err(format!("{} must contain an object of exact-version grants", filename.display()));
    };
    let mut grants = Grants::new();
    for (key, versions) in object {
        if !package_version_pattern().is_match(key) {
            return Err(format!("{} has an invalid grant for {key}", filename.display()));
        }
        let Some(versions) = versions.as_array() else {
            return Err(format!("{} has an invalid grant for {key}", filename.display()));
        };
        let mut list = Vec::new();
        for version in versions {
            let Some(version) = version.as_str() else {
                return Err(format!("{} has an invalid grant for {key}", filename.display()));
            };
            if !exact_version_pattern().is_match(version) {
                return Err(format!("{} has an invalid grant for {key}", filename.display()));
            }
            list.push(version.to_string());
        }
        grants.insert(key.clone(), list);
    }
    Ok(grants)
}

fn write_grants(filename: &Path, grants: &Grants) -> Result<(), String> {
    // `${filename}.${pid}.${randomUUID()}.tmp`: a constant suffix would let two
    // concurrent launches write through the same temporary file.
    let temporary = format!(
        "{}.{}.{}.tmp",
        filename.display(),
        std::process::id(),
        uuid::Uuid::new_v4()
    );
    let value = Value::Object(
        grants
            .iter()
            .map(|(key, versions)| (key.clone(), Value::Array(versions.iter().cloned().map(Value::String).collect())))
            .collect::<Map<String, Value>>(),
    );
    let text = format!("{}\n", serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?);
    let write = write_private(Path::new(&temporary), text.as_bytes())
        .and_then(|()| fs::rename(&temporary, filename));
    let _ = fs::remove_file(&temporary);
    write.map_err(|error| error.to_string())
}

/// `writeFileSync(file, text, { mode: 0o600 })`.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).and_then(|mut file| {
        use std::io::Write;
        file.write_all(bytes)
    })
}

/**
 * Merge the release's grants once per bundled runtime version. A same-version
 * restart must not regrant an exemption the user explicitly revoked.
 * A malformed user file remains untouched so DSH can report its own warnings.
 */
pub fn sync_bundled_compatibility(source_profile: &Path, target_profile: &Path, runtime_version: &str) -> Result<(), String> {
    let marker = target_profile.join(".workdsh-desktop-compatibility");
    let previous_bundled_file = target_profile.join(".workdsh-desktop-bundled-grants.json");
    if let Ok(marker) = fs::read_to_string(&marker) {
        if marker.trim() == runtime_version {
            return Ok(());
        }
    }
    let source = source_profile.join("compatibility.json");
    let target = target_profile.join("compatibility.json");
    let bundled = if source.exists() { read_grants(&source)? } else { Grants::new() };
    let current_result = if target.exists() { Some(read_grants(&target)) } else { None };
    let previous_result = if previous_bundled_file.exists() { Some(read_grants(&previous_bundled_file)) } else { None };
    if current_result.as_ref().is_some_and(|result| result.is_err()) || previous_result.as_ref().is_some_and(|result| result.is_err()) {
        // A malformed user file remains untouched so DSH can report its own warnings.
        eprintln!(
            "QuantDSH cannot update {}; repair its compatibility metadata to receive bundled plugin grants",
            target.display()
        );
        return Ok(());
    }
    let mut current = current_result.and_then(|result| result.ok()).unwrap_or_default();
    let previous_bundled = previous_result.and_then(|result| result.ok()).unwrap_or_default();
    for (key, versions) in &bundled {
        let previous = previous_bundled.get(key).cloned().unwrap_or_default();
        let mut granted = current.get(key).cloned().unwrap_or_default();
        for version in versions {
            // If a user removed a previously bundled exact pair, keep it revoked.
            if !previous.contains(version) && !granted.contains(version) {
                granted.push(version.clone());
            }
        }
        if !granted.is_empty() {
            current.insert(key.clone(), granted);
        }
    }
    write_grants(&target, &current)?;
    write_grants(&previous_bundled_file, &bundled)?;
    fs::write(marker, format!("{runtime_version}\n")).map_err(|error| error.to_string())
}
