use regex::Regex;
use std::sync::OnceLock;

fn redact_patterns() -> &'static [Regex; 4] {
    static PATTERNS: OnceLock<[Regex; 4]> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            // token|key|api_key|access_token query parameters
            Regex::new(r"(?i)([?&](?:token|key|api_key|access_token)=)[^\s&]+").unwrap(),
            // sk- style keys
            Regex::new(r"(?i)\bsk-[a-z0-9_-]+").unwrap(),
            // bearer tokens
            Regex::new(r"(?i)(Bearer\s+)\S+").unwrap(),
            // json-ish credential fields
            Regex::new(r#"(?i)((?:"|')?(?:bootstrapToken|apiKey|password|accessToken|authorization)(?:"|')?\s*[:=]\s*)["']?[^\s,"'}]+["']?"#).unwrap(),
        ]
    })
}

/// Bounded, credential-redacted diagnostics for GUI-only runtime startup failures.
pub fn redact_runtime_output(text: &str) -> String {
    let patterns = redact_patterns();
    let mut result = text.to_string();
    result = patterns[0].replace_all(&result, "${1}[redacted]").to_string();
    result = patterns[1].replace_all(&result, "[redacted]").to_string();
    result = patterns[2].replace_all(&result, "${1}[redacted]").to_string();
    result = patterns[3].replace_all(&result, "${1}[redacted]").to_string();
    result
}

/// Child-process chunks are not credential boundaries. Buffer complete lines
/// before redacting; discard oversized lines instead of emitting a secret prefix.
pub struct RuntimeOutputRedactor {
    pending: Vec<u8>,
    oversized: bool,
}

const LINE_LIMIT: usize = 16_384;

/// JavaScript `value.slice(-limit)` over a mutable buffer.
///
/// Two details matter. The bound counts UTF-16 code units, exactly like a
/// JavaScript string, so a byte-length fast path is safe (`units <= bytes`) but
/// the cut itself has to count units. And the cut must land on a character
/// boundary: slicing bytes at an arbitrary offset panics the moment the offset
/// falls inside a multi-byte character, which is the normal case for Chinese
/// Host output — and that panic runs inside a stdout reader task, so the carrier
/// would silently stop reading the Host instead of reporting a failure.
pub fn keep_tail_utf16(value: &mut String, limit: usize) {
    let units = value.encode_utf16().count();
    if units <= limit {
        return;
    }
    let excess = units - limit;
    let mut dropped = 0usize;
    let mut cut = 0usize;
    for (index, character) in value.char_indices() {
        if dropped >= excess {
            cut = index;
            break;
        }
        dropped += character.len_utf16();
        cut = index + character.len_utf8();
    }
    value.drain(..cut);
}

/// `value.trim().slice(-limit)` as an owned string.
pub fn tail_utf16(value: &str, limit: usize) -> String {
    let mut owned = value.trim().to_string();
    keep_tail_utf16(&mut owned, limit);
    owned
}

impl RuntimeOutputRedactor {
    pub fn new() -> Self {
        Self { pending: Vec::new(), oversized: false }
    }

    fn consume(&mut self, write: &mut dyn FnMut(&str), bytes: &[u8]) {
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            let complete = part.ends_with(b"\n");
            if !self.oversized {
                if self.pending.len() + part.len() > LINE_LIMIT {
                    self.pending.clear();
                    self.oversized = true;
                } else {
                    self.pending.extend_from_slice(part);
                }
            }
            if complete {
                if self.oversized {
                    write("[runtime output line omitted: size limit]\n");
                } else {
                    write(&redact_runtime_output(&String::from_utf8_lossy(&self.pending)));
                }
                self.pending.clear();
                self.oversized = false;
            }
        }
    }

    pub fn push(&mut self, write: &mut dyn FnMut(&str), chunk: &[u8]) {
        self.consume(write, chunk);
    }

    pub fn end(&mut self, write: &mut dyn FnMut(&str)) {
        if self.oversized {
            write("[runtime output line omitted: size limit]\n");
        } else if !self.pending.is_empty() {
            write(&redact_runtime_output(&String::from_utf8_lossy(&self.pending)));
        }
        self.pending.clear();
        self.oversized = false;
    }
}

fn port_patterns() -> &'static [Regex; 2] {
    static PATTERNS: OnceLock<[Regex; 2]> = OnceLock::new();
    PATTERNS.get_or_init(|| [
        Regex::new(r"(?:127\.0\.0\.1|localhost|\[::1\]):(\d+)").unwrap(),
        Regex::new(r"\bport:\s*(\d+)").unwrap(),
    ])
}

fn missing_module_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r#"Cannot find (?:module|package) ['"]([^'"\n]+)['"]"#).unwrap())
}

fn module_name_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"^(?:@[\w.-]+/)?[\w.-]+(?:/[\w.-]+)*$").unwrap())
}

/// `/\b(?:Error|failed|missing|ENOENT)\b/iu` — the `i` flag matters: Host output
/// is full of lowercase `error:` lines that a case-sensitive `contains` misses.
fn relevant_line_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?i)\b(?:Error|failed|missing|ENOENT)\b").unwrap())
}

fn failure_line_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)(?:ERR_|\bError\b|\bfailed\b|EACCES|EPERM|ENOENT|ENOTFOUND|EAI_AGAIN)").unwrap()
    })
}

pub fn runtime_failure_detail(ready: bool, code: Option<i32>, output: &str, cleanup_failed: bool) -> String {
    let cause = format!(
        "官方 DSH {} ({})",
        if ready { "进程已退出" } else { "未能完成启动" },
        code.map(|value| value.to_string()).unwrap_or_else(|| "null".to_string())
    );
    let diagnostic = redact_runtime_output(output);
    let diagnostic = diagnostic.trim();
    let mut reasons: Vec<String> = Vec::new();
    if diagnostic.contains("EADDRINUSE") {
        let mut port = None;
        for pattern in port_patterns().iter() {
            if let Some(found) = pattern.captures(diagnostic).and_then(|captures| captures.get(1)) {
                port = Some(found.as_str().to_string());
                break;
            }
        }
        reasons.push(format!(
            "本机服务端口{}已被占用。请关闭此前的 QuantDSH 工作区后重试。",
            port.map(|value| format!(" {value}")).unwrap_or_default()
        ));
    }
    for captures in missing_module_pattern().captures_iter(diagnostic) {
        let module = captures.get(1).map(|value| value.as_str()).unwrap_or("");
        if module_name_pattern().is_match(module) {
            reasons.push(format!("缺少运行依赖：{module}。请使用包含完整依赖的安装包。"));
        }
    }
    if reasons.is_empty() && (diagnostic.contains("EACCES") || diagnostic.contains("EPERM")) {
        reasons.push("无法访问运行所需的文件或目录，请检查运行目录的权限。".to_string());
    }
    if reasons.is_empty() && !diagnostic.is_empty() {
        let lines: Vec<&str> = diagnostic
            .split('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("at ") && !line.starts_with("file://") && !line.starts_with("[errors]") && !matches!(line.as_bytes().first(), Some(b'{') | Some(b'}') | Some(b'[') | Some(b']')))
            .collect();
        let relevant: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|line| relevant_line_pattern().is_match(line))
            .collect();
        let chosen = if !relevant.is_empty() { relevant } else { lines };
        for line in chosen.iter().take(3) {
            reasons.push(line.chars().take(200).collect());
        }
    }
    let detail = if reasons.is_empty() {
        "没有取得运行输出，请检查插件版本和 Profile 配置。".to_string()
    } else {
        let mut unique: Vec<String> = Vec::new();
        for reason in reasons {
            if !unique.contains(&reason) {
                unique.push(reason);
            }
        }
        unique.iter().take(3).cloned().collect::<Vec<_>>().join("\n")
    };
    let mut parts = vec![cause, detail];
    if cleanup_failed {
        parts.push("另外，本机工具进程停止尚未确认，请退出后检查。".to_string());
    }
    parts.join("\n\n")
}

/// Preserve the bounded redacted trace in the local record, not in a giant alert.
pub fn runtime_failure_log(ready: bool, code: Option<i32>, output: &str, cleanup_failed: bool) -> String {
    format!(
        "{}\n{}\n\n最近的运行输出：\n{}\n",
        chrono_stamp(),
        runtime_failure_detail(ready, code, output, cleanup_failed),
        // `redactRuntimeOutput(output).trim().slice(-16_384)`: the *tail* is kept.
        tail_utf16(&redact_runtime_output(output), 16_384)
    )
}

fn chrono_stamp() -> String {
    // new Date().toISOString()
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default();
    format_iso8601(now as i64)
}

/// Seconds since the epoch → `YYYY-MM-DDTHH:MM:SS.sssZ` without extra dependencies.
pub fn format_iso8601(epoch_seconds: i64) -> String {
    let days = epoch_seconds.div_euclid(86_400);
    let seconds_of_day = epoch_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.subsec_millis())
        .unwrap_or_default();
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Howard Hinnant's civil_from_days algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn ansi_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new("\u{1b}\\[[0-9;]*m").unwrap())
}

pub fn official_command_failure_detail(code: Option<i32>, output: &str, cause: Option<&str>) -> String {
    let redacted = redact_runtime_output(output);
    static INCOMPATIBLE: OnceLock<Regex> = OnceLock::new();
    // The compatibility banner is parsed from the raw trace, like the Electron carrier.
    let incompatible = INCOMPATIBLE.get_or_init(|| Regex::new(r"Plugin (\S+) is incompatible with dsh ([\w.+-]+)").unwrap()).captures(output);
    if let Some(captures) = incompatible {
        if cause.is_none() {
            let plugin = captures.get(1).map(|value| value.as_str()).unwrap_or("");
            let dsh = captures.get(2).map(|value| value.as_str()).unwrap_or("");
            let restored = output.contains("restored package.json, pnpm-lock.yaml, and node_modules");
            return format!(
                "插件 {plugin} 与当前 DSH {dsh} 不兼容。\n\n请安装适配当前底座的插件版本。{}",
                if restored { "原有插件清单和依赖已恢复。" } else { "本次插件更新未完成。" }
            );
        }
    }
    let plain = ansi_pattern().replace_all(&redacted, "").to_string();
    let lines: Vec<&str> = plain
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("at ") && !line.starts_with("file://"))
        .collect();
    let errors: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| failure_line_pattern().is_match(line))
        .collect();
    let chosen = if !errors.is_empty() { errors } else { lines };
    let reasons: Vec<String> = chosen.iter().rev().take(3).map(|line| line.chars().take(240).collect::<String>()).collect();
    let reasons = reasons.into_iter().rev().collect::<Vec<_>>();
    format!(
        "{}\n\n{}",
        cause.map(|value| value.to_string()).unwrap_or_else(|| format!("插件依赖更新失败 ({})", code.map(|value| value.to_string()).unwrap_or_else(|| "null".to_string()))),
        if reasons.is_empty() { "未取得工具输出，请查看本机错误记录后重试。".to_string() } else { reasons.join("\n") }
    )
}
