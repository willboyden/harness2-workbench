//! Reading and editing `h2workbench.toml` as TEXT, for the configuration editor's form view.
//!
//! The form edits the draft in place: setting a key replaces only its value (comments, order
//! and layout stay as they were), a new key goes at the end of its table, and a missing table
//! is appended. The scanner understands the TOML the file uses (tables, `[[arrays of
//! tables]]`, `key = value` with strings, numbers, booleans and possibly multi-line arrays and
//! inline tables) and never fails: what it cannot read, the form leaves to the Text view.
//!
//! This is not the validator. The server's own loader checks the text
//! (`POST /api/v1/system/config/validate`).

/// One `key = value` the scanner found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// Dotted path: table + key (`auth.proxy.user_header`).
    pub(crate) path: String,
    /// The value as written (may span lines), without a trailing comment.
    pub(crate) raw: String,
    /// 1-based line of the key.
    pub(crate) line: usize,
    /// Byte offset where the key's line starts.
    line_start: usize,
    /// Byte range of the value.
    start: usize,
    end: usize,
}

/// A `[table]` header the scanner found.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Table {
    name: String,
    /// Byte offset just past the header line (its newline included).
    body_start: usize,
}

/// What the scanner found in a text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Scan {
    pub(crate) entries: Vec<Entry>,
    tables: Vec<Table>,
    /// Names of `[[arrays of tables]]` (their keys are not form fields).
    pub(crate) table_arrays: Vec<String>,
}

impl Scan {
    pub(crate) fn get(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

/// `at`, moved back to the start of the character it falls in (and to at most the text's
/// length), so slicing there cannot split a character.
fn char_floor(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Byte offset just past the end of the line containing `at` (or the text's end). `at` may
/// fall inside a character (callers pass "one byte before the value's end").
fn line_end(text: &str, at: usize) -> usize {
    let at = char_floor(text, at);
    text[at..].find('\n').map_or(text.len(), |i| at + i + 1)
}

/// Where a value starting at `from` ends: the end of the scalar, string, array or inline
/// table (which may span lines), before a trailing comment and whitespace.
fn value_end(text: &str, from: usize) -> usize {
    value_end_in(text, from, false)
}

/// [`value_end`]; with `item`, also stop at a `,` or `]` outside brackets (an array item).
fn value_end_in(text: &str, from: usize, item: bool) -> usize {
    let b = text.as_bytes();
    let mut i = from;
    let mut depth = 0usize;
    let mut last_content = from;
    while i < b.len() {
        let c = b[i];
        if b[i..].starts_with(b"\"\"\"") || b[i..].starts_with(b"'''") {
            // `i` is at an ASCII quote, so `i + 3` is a character boundary.
            let delim = if c == b'"' { "\"\"\"" } else { "'''" };
            let close = text[i + 3..]
                .find(delim)
                .map_or(text.len(), |j| i + 3 + j + 3);
            i = close.min(text.len());
            last_content = i;
            continue;
        }
        match c {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' && b[i] != b'\n' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i = (i + 1).min(b.len());
                last_content = i;
            }
            b'\'' => {
                i += 1;
                while i < b.len() && b[i] != b'\'' && b[i] != b'\n' {
                    i += 1;
                }
                i = (i + 1).min(b.len());
                last_content = i;
            }
            b'#' => {
                // A comment runs to the end of the line.
                i = line_end(text, i);
                if depth == 0 {
                    break;
                }
            }
            b'\n' => {
                if depth == 0 {
                    break;
                }
                i += 1;
            }
            b',' | b']' if item && depth == 0 => break,
            b'[' | b'{' => {
                depth += 1;
                i += 1;
                last_content = i;
            }
            b']' | b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
                last_content = i;
            }
            b' ' | b'\t' | b'\r' => i += 1,
            _ => {
                i += 1;
                last_content = i;
            }
        }
    }
    last_content
}

fn is_bare_key(k: &str) -> bool {
    !k.is_empty()
        && k.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
}

/// The header name of `[name]` / `[[name]]` lines (trailing comments allowed).
fn header(line: &str) -> Option<(String, bool)> {
    let l = line.trim();
    let (inner, array) = if let Some(rest) = l.strip_prefix("[[") {
        (rest.split_once("]]")?.0, true)
    } else {
        (l.strip_prefix('[')?.split_once(']')?.0, false)
    };
    let name: String = inner
        .split('.')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(".");
    is_bare_key(&name).then_some((name, array))
}

/// Scan `text`. Never fails; lines it cannot read are skipped.
pub(crate) fn scan(text: &str) -> Scan {
    let mut out = Scan::default();
    let mut table = String::new();
    let mut in_array = false;
    let mut pos = 0;
    let mut line_no = 1;
    while pos < text.len() {
        let end = line_end(text, pos);
        let line = &text[pos..end];
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if let Some((name, array)) = header(trimmed) {
                if array {
                    if !out.table_arrays.contains(&name) {
                        out.table_arrays.push(name.clone());
                    }
                } else {
                    out.tables.push(Table {
                        name: name.clone(),
                        body_start: end,
                    });
                }
                table = name;
                in_array = array;
            }
            line_no += 1;
            pos = end;
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            line_no += 1;
            pos = end;
            continue;
        }
        let Some(eq) = line.find('=') else {
            line_no += 1;
            pos = end;
            continue;
        };
        let key: String = line[..eq]
            .trim()
            .split('.')
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(".");
        let mut vstart = pos + eq + 1;
        while vstart < text.len() && matches!(text.as_bytes()[vstart], b' ' | b'\t') {
            vstart += 1;
        }
        let vend = value_end(text, vstart).max(vstart);
        if is_bare_key(&key) && !in_array {
            let path = if table.is_empty() {
                key
            } else {
                format!("{table}.{key}")
            };
            out.entries.push(Entry {
                path,
                raw: text[vstart..vend].to_owned(),
                line: line_no,
                line_start: pos,
                start: vstart,
                end: vend,
            });
        }
        let next = line_end(text, vend.saturating_sub(1).max(pos));
        line_no += text[pos..next].matches('\n').count().max(1);
        pos = next.max(end);
    }
    out
}

fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('.').unwrap_or(("", path))
}

/// Set `path` to the TOML literal `value`: replace the value where the key is written, else
/// add `key = value` at the end of its table, else append the table.
pub(crate) fn set(text: &str, path: &str, value: &str) -> String {
    let s = scan(text);
    if let Some(e) = s.get(path) {
        return format!("{}{value}{}", &text[..e.start], &text[e.end..]);
    }
    let (table, key) = split_path(path);
    let line = format!("{key} = {value}\n");
    if let Some(t) = s.tables.iter().find(|t| t.name == table) {
        // After the table's last key, or right after its header.
        let prefix = format!("{table}.");
        let after = s
            .entries
            .iter()
            .filter(|e| {
                e.start >= t.body_start
                    && e.path
                        .strip_prefix(&prefix)
                        .is_some_and(|k| !k.contains('.'))
            })
            .map(|e| line_end(text, e.end.saturating_sub(1).max(e.line_start)))
            .max()
            .unwrap_or(t.body_start);
        let mut out = String::with_capacity(text.len() + line.len() + 1);
        out.push_str(&text[..after]);
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&line);
        out.push_str(&text[after..]);
        return out;
    }
    let mut out = text.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    if table.is_empty() {
        out.push_str(&line);
    } else {
        out.push_str(&format!("[{table}]\n{line}"));
    }
    out
}

/// Remove the line(s) where `path` is set, so its default applies. Unchanged when it is not set.
pub(crate) fn remove(text: &str, path: &str) -> String {
    let s = scan(text);
    match s.get(path) {
        Some(e) => {
            let end = line_end(text, e.end.saturating_sub(1).max(e.line_start));
            format!("{}{}", &text[..e.line_start], &text[end..])
        }
        None => text.to_owned(),
    }
}

/// A TOML basic string literal.
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The string a TOML string literal holds (basic or literal, single line).
pub(crate) fn unquote(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        return (!inner.contains('\'')).then(|| inner.to_owned());
    }
    let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '"' {
            return None;
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'u' => {
                let hex: String = chars.by_ref().take(4).collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            'U' => {
                let hex: String = chars.by_ref().take(8).collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

/// The strings of a one-dimensional array of strings (comments and a trailing comma
/// allowed). `None` for anything else.
pub(crate) fn string_list(raw: &str) -> Option<Vec<String>> {
    let inner = raw.trim().strip_prefix('[')?.strip_suffix(']')?;
    let mut out = Vec::new();
    let mut rest = inner;
    loop {
        rest = skip_space_and_comments(rest);
        if rest.is_empty() {
            return Some(out);
        }
        let end = value_end_in(rest, 0, true);
        let item = &rest[..end];
        out.push(unquote(item)?);
        rest = skip_space_and_comments(&rest[end..]);
        match rest.strip_prefix(',') {
            Some(r) => rest = r,
            None => return rest.is_empty().then_some(out),
        }
    }
}

fn skip_space_and_comments(mut s: &str) -> &str {
    loop {
        s = s.trim_start();
        match s.strip_prefix('#') {
            Some(r) => s = r.find('\n').map_or("", |i| &r[i + 1..]),
            None => return s,
        }
    }
}

/// `["a", "b"]`.
pub(crate) fn render_string_list(items: &[String]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|s| quote(s))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// `true`/`false`.
pub(crate) fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// A TOML integer as the form shows it (underscores removed); `None` when it is not one.
pub(crate) fn parse_int(raw: &str) -> Option<i64> {
    let r = raw.trim().replace('_', "");
    r.parse::<i64>().ok()
}

/// A TOML float or integer.
pub(crate) fn parse_float(raw: &str) -> Option<f64> {
    let r = raw.trim().replace('_', "");
    r.parse::<f64>().ok().filter(|f| f.is_finite())
}

/// `s` as one POSIX shell word: in single quotes, each `'` written as `'\''`.
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The install commands for the exported file, at `path` (the server's file) when it is known.
/// The path is quoted for the shell, so a path with spaces or quotes is copied safely.
pub(crate) fn install_commands(path: Option<&str>) -> String {
    // Without a known path, a placeholder the administrator replaces (no quote in it).
    let target = path.unwrap_or("/path/to/the/server/h2workbench.toml");
    let dest = shell_quote(target);
    let backup = shell_quote(&format!("{target}.bak"));
    format!(
        "# 1. Check the new file with the server's own loader (changes nothing):\n\
         h2workbench --config ./h2workbench.toml config check\n\
         \n\
         # 2. Keep the current file, then install the new one, readable only by the server's user:\n\
         cp -p {dest} {backup}\n\
         install -m 600 ./h2workbench.toml {dest}\n\
         \n\
         # 3. Restart the server; it reads its configuration only when it starts. With the\n\
         #    systemd user unit from deploy/systemd:\n\
         systemctl --user restart h2workbench\n\
         #    [egress] keys are read by `h2workbench egress-proxy`: restart that process too.\n\
         #    In the compose deployment (deploy/compose.yaml, the server reads\n\
         #    /config/h2workbench.toml), steps 2 and 3 differ: replace deploy/h2workbench.toml on\n\
         #    the host (chmod 644, the container's user reads it), then:\n\
         #    docker compose -f deploy/compose.yaml up -d --force-recreate workbench egress\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_survives_single_quotes() {
        assert_eq!(shell_quote("/srv/h2w.toml"), "'/srv/h2w.toml'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
        // A path that tries to end the quote and run a command stays one word.
        let path = "/srv/x'; rm -rf ~; echo '.toml";
        let cmds = install_commands(Some(path));
        assert!(
            cmds.contains(
                "install -m 600 ./h2workbench.toml '/srv/x'\\''; rm -rf ~; echo '\\''.toml'\n"
            ),
            "{cmds}"
        );
        assert!(cmds.contains("cp -p '/srv/x'\\''; rm -rf ~; echo '\\''.toml' '/srv/x'\\''; rm -rf ~; echo '\\''.toml.bak'\n"), "{cmds}");
        assert!(install_commands(None).contains(
            "install -m 600 ./h2workbench.toml '/path/to/the/server/h2workbench.toml'\n"
        ));
    }

    const EXAMPLE: &str = include_str!("../../../../deploy/examples/h2workbench.toml");

    #[test]
    fn scans_the_shipped_example() {
        let s = scan(EXAMPLE);
        let get = |p: &str| s.get(p).map(|e| e.raw.clone()).unwrap_or_default();
        assert_eq!(get("server.listen"), "\"127.0.0.1:8722\"");
        assert_eq!(get("auth.proxy.secret_env"), "\"H2W_PROXY_SECRET\"");
        assert_eq!(get("jobs.timeout_hours"), "48");
        assert_eq!(get("isolation.container.cpus"), "4.0");
        assert_eq!(
            string_list(&get("egress.allow")),
            Some(vec![
                "aiplatform.googleapis.com".to_owned(),
                "oauth2.googleapis.com".to_owned()
            ])
        );
        assert_eq!(get("judge.cloud"), "\"acknowledge\"");
        // Every key of the example is found once, under its own table.
        let mut paths: Vec<&str> = s.entries.iter().map(|e| e.path.as_str()).collect();
        let n = paths.len();
        paths.dedup();
        assert_eq!(paths.len(), n);
        assert!(n >= 50, "{n}");
        // Line numbers point at the key.
        let e = s.get("server.listen").unwrap();
        assert!(
            EXAMPLE
                .lines()
                .nth(e.line - 1)
                .unwrap()
                .starts_with("listen")
        );
    }

    #[test]
    fn multi_line_values_and_comments() {
        let text = "[egress]\n# hosts\nallow = [\n  \"a.example.com\", # first\n  { host = \"gw.example.com\", ports = [4000] },\n]  # end\nlisten = \"127.0.0.1:3128\"\n";
        let s = scan(text);
        let allow = s.get("egress.allow").unwrap();
        assert!(allow.raw.starts_with('[') && allow.raw.ends_with(']'));
        assert_eq!(allow.line, 3);
        assert_eq!(s.get("egress.listen").unwrap().line, 7);
        assert_eq!(string_list(&allow.raw), None);
        let replaced = set(text, "egress.allow", "[]");
        assert_eq!(
            replaced,
            "[egress]\n# hosts\nallow = []  # end\nlisten = \"127.0.0.1:3128\"\n"
        );
    }

    #[test]
    fn set_replaces_inserts_and_appends() {
        let text =
            "# top\n[server]\nlisten = \"127.0.0.1:8722\" # keep\n\n[jobs]\nmax_queued = 100\n";
        assert_eq!(
            set(text, "server.listen", "\"0.0.0.0:8722\""),
            "# top\n[server]\nlisten = \"0.0.0.0:8722\" # keep\n\n[jobs]\nmax_queued = 100\n"
        );
        assert_eq!(
            set(text, "server.max_body_bytes", "2048"),
            "# top\n[server]\nlisten = \"127.0.0.1:8722\" # keep\nmax_body_bytes = 2048\n\n[jobs]\nmax_queued = 100\n"
        );
        assert_eq!(
            set(text, "auth.mode", "\"proxy\""),
            format!("{text}\n[auth]\nmode = \"proxy\"\n")
        );
        assert_eq!(
            set("", "jobs.max_concurrent", "3"),
            "[jobs]\nmax_concurrent = 3\n"
        );
        assert_eq!(
            set("[jobs]", "jobs.max_concurrent", "3"),
            "[jobs]\nmax_concurrent = 3\n"
        );
        // A key of a sub-table is not "the end of" the parent table.
        let nested = "[auth]\nmode = \"token\"\n[auth.proxy]\nuser_header = \"x\"\n";
        assert_eq!(
            set(nested, "auth.session_ttl_hours", "4"),
            "[auth]\nmode = \"token\"\nsession_ttl_hours = 4\n[auth.proxy]\nuser_header = \"x\"\n"
        );
        // Values are re-scanned correctly after an edit.
        let edited = set(EXAMPLE, "jobs.max_concurrent", "6");
        assert_eq!(scan(&edited).get("jobs.max_concurrent").unwrap().raw, "6");
        assert_eq!(edited.len(), EXAMPLE.len());
    }

    #[test]
    fn remove_drops_only_that_key() {
        let text = "[jobs]\n# how many\nmax_concurrent = 2\nmax_queued = 100\n";
        assert_eq!(
            remove(text, "jobs.max_concurrent"),
            "[jobs]\n# how many\nmax_queued = 100\n"
        );
        assert_eq!(remove(text, "jobs.nope"), text);
        let multi = "[egress]\nallow = [\n  \"a\",\n]\nlisten = \"x\"\n";
        assert_eq!(remove(multi, "egress.allow"), "[egress]\nlisten = \"x\"\n");
    }

    #[test]
    fn arrays_of_tables_are_not_form_fields() {
        let text = "[benchmarks]\n\n[[benchmarks.not_offered]]\nbench = \"wb\"\nnote = \"elsewhere\"\n\n[judge]\ncloud = \"refuse\"\n";
        let s = scan(text);
        assert_eq!(s.table_arrays, vec!["benchmarks.not_offered".to_owned()]);
        assert!(s.get("benchmarks.not_offered.bench").is_none());
        assert_eq!(s.get("judge.cloud").unwrap().raw, "\"refuse\"");
    }

    #[test]
    fn strings_round_trip() {
        for s in [
            "",
            "plain",
            "with \"quotes\" and \\ and\nnewline",
            "tab\there",
            "é ü",
        ] {
            assert_eq!(unquote(&quote(s)).as_deref(), Some(s));
        }
        assert_eq!(unquote("'C:\\path'").as_deref(), Some("C:\\path"));
        assert_eq!(unquote("\"\\u00e9\"").as_deref(), Some("é"));
        assert_eq!(unquote("bare"), None);
        assert_eq!(unquote("\"a\"b\""), None);
        let list = vec!["a.example.com".to_owned(), "b \"q\"".to_owned()];
        assert_eq!(string_list(&render_string_list(&list)), Some(list));
        assert_eq!(string_list("[]"), Some(vec![]));
        assert_eq!(
            string_list("[\n  # c\n  \"x\",\n]"),
            Some(vec!["x".to_owned()])
        );
        assert_eq!(string_list("[1, 2]"), None);
        assert_eq!(string_list("\"x\""), None);
    }

    #[test]
    fn numbers_and_bools() {
        assert_eq!(parse_int("1_048_576"), Some(1_048_576));
        assert_eq!(parse_int("4.0"), None);
        assert_eq!(parse_float("48"), Some(48.0));
        assert_eq!(parse_float("0.5"), Some(0.5));
        assert_eq!(parse_float("nan"), None);
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool("yes"), None);
    }

    /// Scan, set and remove on `text`; slicing must never split a character.
    fn exercise(text: &str) {
        let s = scan(text);
        for e in &s.entries {
            assert!(e.start <= e.end && e.end <= text.len(), "{text:?}");
            assert!(text.is_char_boundary(e.start) && text.is_char_boundary(e.end));
        }
        for path in ["a", "b", "server.listen", "auth.mode", "x.y"] {
            let _ = set(text, path, "\"y\"");
            let _ = remove(text, path);
        }
    }

    #[test]
    fn odd_input_never_panics() {
        for text in [
            "[",
            "[[",
            "= 1",
            "a = \"unterminated\nb = 2",
            "a = [1, 2\n[t]\n",
            "a = '''\nmulti\n'''\nb = 1",
            "\u{feff}[server]\nlisten = \"x\"",
            "a = \"é\"#c",
            // Values that end in a character of more than one byte.
            "a = é",
            "a = é\nb = 2",
            "[server]\nlisten = “x”",
            "[server]\nlisten = “x”\nb = 1\n",
            "a = \"abcé",
            "a = [é",
            "a = [é\n[server]\nlisten = 1",
            "a = {k = 'ü'}",
            "a = \"\\é\"",
        ] {
            exercise(text);
        }
        // Every value from a set of unicode strings, in every position the scanner handles.
        let values = [
            "é",
            "ü",
            "“x”",
            "日本語",
            "🦀",
            "a🦀",
            "\u{301}",
            "e\u{301}",
            "\u{feff}",
            "ß=",
            "\"é",
            "[é",
            "{é",
            "'é",
            "\"\"\"é",
            "é#",
            "é,",
            "\\é",
        ];
        for v in values {
            for text in [
                format!("a = {v}"),
                format!("a = {v}\n"),
                format!("a = {v}\nb = 1\n"),
                format!("[server]\nlisten = {v}"),
                format!("[server]\nlisten = {v}\n[auth]\nmode = {v}"),
                format!("a = [{v}, {v}]\n"),
                format!("a = \"{v}\"\nb = '{v}'"),
                format!("{v} = {v}"),
            ] {
                exercise(&text);
            }
        }
        let s = scan("a = '''\nmulti\n'''\nb = 1");
        assert_eq!(s.get("b").map(|e| e.line), Some(4));
    }
}
