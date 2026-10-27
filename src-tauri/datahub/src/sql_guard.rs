//! Read-only SQL guard.
//!
//! Why this exists: the data page lets users type SQL. A bare
//! `SELECT * FROM bars` can exhaust the process, and one stray DDL can destroy
//! the market database. **Every** user-supplied statement passes through here.
//!
//! Four guarantees:
//!   1. read-only  — only SELECT / WITH / EXPLAIN / DESCRIBE / SHOW heads
//!   2. single     — `;`-joined statements are rejected (`SELECT 1; DROP ...`)
//!   3. row cap    — missing or oversized LIMIT is tightened
//!   4. timeout    — enforced by the executor; this module is pure static analysis
//!
//! Ported from the TypeScript guard. Two bugs fixed there are deliberately
//! preserved here — see the notes on [`strip_comments`] and [`guard_sql`].

/// Rejection codes are part of the HTTP contract; the UI branches on them.
pub const CODE_EMPTY: &str = "empty";
pub const CODE_MULTI: &str = "multi_statement";
pub const CODE_NOT_READ_ONLY: &str = "not_read_only";
pub const CODE_FORBIDDEN: &str = "forbidden_keyword";
pub const CODE_UNTERMINATED: &str = "unterminated_string";

#[derive(Debug, Clone, PartialEq)]
pub enum GuardVerdict {
    Ok { sql: String, rewritten: bool },
    Reject { code: &'static str, reason: String },
}

/// Allowed statement heads. `PRAGMA` is intentionally absent: it is a side door
/// around `SET`, and the schema browser already uses `information_schema`.
const ALLOWED_HEAD: [&str; 5] = ["select", "with", "explain", "describe", "show"];

/// Whole-word matches only, so `updated_at` is not read as `UPDATE`.
/// Literals and comments are stripped before this runs, so
/// `WHERE note = 'drop it'` is safe.
const FORBIDDEN: &[&str] = &[
    "insert", "update", "delete", "merge", "replace", "upsert", "create", "drop", "alter",
    "truncate", "attach", "detach", "copy", "export", "import", "install", "load", "vacuum",
    "grant", "revoke", "begin", "commit", "rollback", "transaction", "set", "reset", "call",
    "exec", "execute", "pragma", "system", "write",
    // blocks `SELECT ... INTO tbl` (DuckDB's CTAS spelling)
    "into", "temp", "temporary", "function", "macro", "secret",
];

/// Maximal runs of `[A-Za-z0-9_]` with byte offsets. This is our stand-in for
/// regex `\bword\b` — every keyword above is purely alphanumeric, so token
/// equality is exactly word-boundary matching.
fn word_tokens(s: &str) -> Vec<(&str, usize, usize)> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        if is_word(bytes[i]) {
            let start = i;
            while i < bytes.len() && is_word(bytes[i]) {
                i += 1;
            }
            out.push((&s[start..i], start, i));
        } else {
            i += 1;
        }
    }
    out
}

/// Strip `--` line and `/* */` block comments.
///
/// **String literals are preserved verbatim, including the opening quote.**
/// An earlier version dropped the opening quote, turning `'BTCUSDT'` into
/// `BTCUSDT'`, which then looked like an unterminated string and rejected
/// perfectly valid queries.
pub fn strip_comments(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0usize;
    let mut in_line = false;
    let mut in_block = false;
    let mut in_str: Option<char> = None;

    while i < chars.len() {
        let c = chars[i];
        let n = chars.get(i + 1).copied();

        if in_line {
            if c == '\n' {
                in_line = false;
                out.push('\n');
            }
            i += 1;
            continue;
        }
        if in_block {
            if c == '*' && n == Some('/') {
                in_block = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if let Some(q) = in_str {
            // Keep literal content as-is: this function strips comments, it does
            // not sanitise. Keyword checks run against a separate masked copy.
            out.push(c);
            if c == q {
                if n == Some(q) {
                    // SQL escapes a quote by doubling it
                    out.push(q);
                    i += 2;
                    continue;
                }
                in_str = None;
            }
            i += 1;
            continue;
        }

        if c == '\'' || c == '"' {
            in_str = Some(c);
            out.push(c); // the opening quote must survive
            i += 1;
            continue;
        }
        if c == '-' && n == Some('-') {
            in_line = true;
            i += 2;
            continue;
        }
        if c == '/' && n == Some('*') {
            in_block = true;
            i += 2;
            continue;
        }

        out.push(c);
        i += 1;
    }
    out
}

/// Replace string-literal contents with an empty literal so literal text can
/// never be mistaken for a keyword. Returns `(masked, unterminated)`.
pub fn mask_string_literals(sql: &str) -> (String, bool) {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0usize;
    let mut in_str: Option<char> = None;

    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_str {
            if c == q {
                let n = chars.get(i + 1).copied();
                if n == Some(q) {
                    i += 2;
                    continue;
                }
                in_str = None;
                out.push_str("''");
                i += 1;
                continue;
            }
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            in_str = Some(c);
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    (out, in_str.is_some())
}

/// True when a `;` is followed by more content (a bare trailing `;` is fine).
pub fn is_multi_statement(sql: &str) -> bool {
    let (masked, _) = mask_string_literals(&strip_comments(sql));
    match masked.find(';') {
        None => false,
        Some(idx) => masked[idx + 1..].trim().len() > 0,
    }
}

/// Tighten the top-level LIMIT. Returns `(sql, rewritten)`.
pub fn enforce_limit(sql: &str, max_rows: u64) -> (String, bool) {
    let tokens = word_tokens(sql);
    let mut last: Option<(usize, usize, u64)> = None; // (start, end, value)
    for (i, (word, start, _end)) in tokens.iter().enumerate() {
        if !word.eq_ignore_ascii_case("limit") {
            continue;
        }
        if let Some((nxt, _, nend)) = tokens.get(i + 1) {
            if let Ok(v) = nxt.parse::<u64>() {
                last = Some((*start, *nend, v));
            }
        }
    }

    match last {
        None => (format!("{sql} LIMIT {max_rows}"), true),
        Some((s, e, v)) if v > max_rows => {
            let before = &sql[..s];
            let after = &sql[e..];
            (format!("{before}LIMIT {max_rows}{after}"), true)
        }
        Some(_) => (sql.to_string(), false),
    }
}

/// Main entry point: validate and, if needed, rewrite user SQL.
///
/// Two copies of the statement are kept for different jobs — mixing them up was
/// an earlier bug that silently emptied the user's `WHERE` clause:
///   * `cleaned` — comments stripped, **literals intact** → what we execute
///   * `masked`  — literals blanked → what we keyword-check
pub fn guard_sql(raw: &str, max_rows: u64) -> GuardVerdict {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return GuardVerdict::Reject {
            code: CODE_EMPTY,
            reason: "SQL 为空".into(),
        };
    }

    let cleaned = strip_comments(trimmed);
    let (masked, unterminated) = mask_string_literals(&cleaned);
    if unterminated {
        return GuardVerdict::Reject {
            code: CODE_UNTERMINATED,
            reason: "字符串字面量未闭合".into(),
        };
    }

    let body = masked.trim().trim_end_matches(';').trim();
    if body.is_empty() {
        return GuardVerdict::Reject {
            code: CODE_EMPTY,
            reason: "SQL 为空".into(),
        };
    }

    if is_multi_statement(trimmed) {
        return GuardVerdict::Reject {
            code: CODE_MULTI,
            reason: "只允许单条语句，禁止用分号拼接多条".into(),
        };
    }

    let head_ok = word_tokens(body)
        .first()
        .map(|(w, _, _)| ALLOWED_HEAD.iter().any(|h| w.eq_ignore_ascii_case(h)))
        .unwrap_or(false);
    if !head_ok {
        return GuardVerdict::Reject {
            code: CODE_NOT_READ_ONLY,
            reason: "只允许 SELECT / WITH / EXPLAIN / DESCRIBE / SHOW 开头的只读查询".into(),
        };
    }

    for (word, _, _) in word_tokens(body) {
        if let Some(hit) = FORBIDDEN.iter().find(|f| word.eq_ignore_ascii_case(f)) {
            return GuardVerdict::Reject {
                code: CODE_FORBIDDEN,
                reason: format!("禁止使用关键字：{}", hit.to_uppercase()),
            };
        }
    }

    // Act on `cleaned`, never on `masked` — otherwise the user's literal
    // predicates get blanked out before execution.
    let executable = cleaned.trim().trim_end_matches(';').trim();
    let (sql, rewritten) = enforce_limit(executable, max_rows);
    GuardVerdict::Ok { sql, rewritten }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_select_gets_a_limit_appended() {
        match guard_sql("SELECT * FROM bars", 100) {
            GuardVerdict::Ok { sql, rewritten } => {
                assert_eq!(sql, "SELECT * FROM bars LIMIT 100");
                assert!(rewritten);
            }
            other => panic!("expected ok, got {other:?}"),
        }
    }

    /// Regression: the opening quote used to be dropped, making a valid query
    /// look like an unterminated string.
    #[test]
    fn literal_predicate_survives_comment_stripping() {
        let v = guard_sql("SELECT symbol, close FROM bars WHERE symbol = 'BTCUSDT'", 50);
        match v {
            GuardVerdict::Ok { sql, .. } => {
                assert!(
                    sql.contains("'BTCUSDT'"),
                    "literal must survive: {sql}"
                );
                assert!(sql.ends_with("LIMIT 50"));
            }
            other => panic!("must not be rejected: {other:?}"),
        }
    }

    /// Regression: executing the masked copy would empty the WHERE clause.
    #[test]
    fn executed_sql_keeps_the_real_literal_not_the_placeholder() {
        let v = guard_sql("SELECT * FROM bars WHERE symbol = 'BTC'", 10);
        match v {
            GuardVerdict::Ok { sql, .. } => assert!(!sql.contains("''"), "got {sql}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn ddl_and_dml_are_rejected() {
        for bad in [
            "DROP TABLE bars",
            "DELETE FROM bars",
            "UPDATE bars SET close = 1",
            "INSERT INTO bars VALUES (1)",
            "SELECT * FROM bars INTO copy_tbl",
            "PRAGMA version",
            "SET memory_limit='1GB'",
            "SELECT 1; DROP TABLE bars",
        ] {
            match guard_sql(bad, 100) {
                GuardVerdict::Reject { code, .. } => {
                    assert!(
                        matches!(
                            code,
                            CODE_NOT_READ_ONLY | CODE_FORBIDDEN | CODE_MULTI
                        ),
                        "{bad} rejected with unexpected code {code}"
                    );
                }
                GuardVerdict::Ok { .. } => panic!("{bad} must be rejected"),
            }
        }
    }

    #[test]
    fn column_names_containing_keywords_are_not_false_positives() {
        // `updated_at` is a whole token, so `\bupdate\b` does not match.
        match guard_sql("SELECT updated_at, deleted_flag FROM bars", 10) {
            GuardVerdict::Ok { .. } => {}
            other => panic!("false positive: {other:?}"),
        }
    }

    #[test]
    fn keyword_inside_a_literal_is_not_a_false_positive() {
        match guard_sql("SELECT * FROM bars WHERE note = 'drop it'", 10) {
            GuardVerdict::Ok { .. } => {}
            other => panic!("false positive: {other:?}"),
        }
    }

    #[test]
    fn an_oversized_limit_is_tightened() {
        match guard_sql("SELECT * FROM bars LIMIT 999999", 100) {
            GuardVerdict::Ok { sql, rewritten } => {
                assert_eq!(sql, "SELECT * FROM bars LIMIT 100");
                assert!(rewritten);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_small_limit_is_left_alone() {
        match guard_sql("SELECT * FROM bars LIMIT 5", 100) {
            GuardVerdict::Ok { sql, rewritten } => {
                assert_eq!(sql, "SELECT * FROM bars LIMIT 5");
                assert!(!rewritten);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn comments_are_stripped_but_do_not_break_matching() {
        let v = guard_sql("SELECT * FROM bars -- trailing comment\nWHERE ts > 0", 10);
        match v {
            GuardVerdict::Ok { sql, .. } => assert!(sql.starts_with("SELECT")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unterminated_literal_is_rejected() {
        match guard_sql("SELECT * FROM bars WHERE symbol = 'BTC", 10) {
            GuardVerdict::Reject { code, .. } => assert_eq!(code, CODE_UNTERMINATED),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn empty_and_blank_input_are_rejected() {
        assert_eq!(guard_sql("", 10).code(), Some(CODE_EMPTY));
        assert_eq!(guard_sql("   ", 10).code(), Some(CODE_EMPTY));
        assert_eq!(guard_sql("-- only a comment", 10).code(), Some(CODE_EMPTY));
    }

    #[test]
    fn trailing_semicolon_alone_is_allowed() {
        match guard_sql("SELECT * FROM bars;", 10) {
            GuardVerdict::Ok { sql, .. } => assert!(!sql.contains(';')),
            other => panic!("{other:?}"),
        }
    }

    impl GuardVerdict {
        fn code(&self) -> Option<&'static str> {
            match self {
                GuardVerdict::Reject { code, .. } => Some(code),
                GuardVerdict::Ok { .. } => None,
            }
        }
    }
}
