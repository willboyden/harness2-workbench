//! Support for the web UI's configuration editor: read the configuration file for an
//! administrator, sending it only when every key is a known setting and nothing in it looks
//! like a credential, and check a configuration text with the server's own loader.
//!
//! Nothing here writes a file, changes a setting or runs a command: the editor drafts,
//! validates and exports, and an administrator installs the file by hand.

use std::io::Read;
use std::path::Path;

use h2w_api::{ConfigParseError, ConfigValidation};

use crate::config::{Config, EnvLookup, FileConfig};

/// The largest configuration text the server reads for the editor or validates.
pub const CONFIG_TEXT_MAX: usize = 256 * 1024;

/// The environment variables that override keys of the file (one list, in `config.rs`).
pub use crate::config::ENV_OVERRIDES;

/// The keys of one `[[benchmarks.not_offered]]` entry (`config::NotOfferedEntry`).
const NOT_OFFERED_FIELDS: [&str; 3] = ["bench", "note", "url"];

/// The keys of a detailed `egress.allow` entry (`config::AllowEntryDetailed`).
const ALLOW_FIELDS: [&str; 3] = ["host", "allow_private", "ports"];

/// Words that mark a name as holding a credential when a comment (or a text value) assigns
/// it a value (`name = value`). Names are compared in lowercase with `-` read as `_`.
const CREDENTIAL_WORDS: [&str; 18] = [
    "pass",
    "pwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "private_key",
    "privatekey",
    "access_key",
    "credential",
    "auth",
    "bearer",
    "jwt",
    "cookie",
    "dsn",
    "hmac",
    "signing",
    "salt",
];

/// A run this long of token characters that mixes letters and digits looks like a token.
const TOKEN_RUN_MIN: usize = 20;

/// A URL path or fragment segment this long that mixes letters and digits looks like a token.
const URL_SEGMENT_MIN: usize = 16;

const R_NOT_A_CONFIG: &str = "it does not parse as the server's configuration (a syntax error, an unknown key or a value of the wrong type)";
const R_UNKNOWN_KEY: &str = "it has a key that is not a known setting";
const R_PEM: &str = "it contains a private key or certificate block";
const R_API_TOKEN: &str = "it contains a Workbench API token";
const R_URL: &str =
    "it contains a URL with user information, a query string or a token-like path or fragment";
const R_TOKEN: &str = "it contains a long token-like string of mixed letters and digits";
const R_ASSIGNMENT: &str =
    "it assigns a value to a name that looks like a password, token, key or secret";
const R_STORED_SECRET: &str = "it contains the value of a stored secret";

/// The overrides set (non-empty) in `env`, by name.
#[must_use]
pub fn overrides_in_force(env: EnvLookup<'_>) -> Vec<String> {
    ENV_OVERRIDES
        .iter()
        .filter(|n| env(n).is_some_and(|v| !v.is_empty()))
        .map(|n| (*n).to_owned())
        .collect()
}

/// Why `text` must not be sent to a browser, or `None` when it may be. The reason never
/// quotes the text.
///
/// An allowlist: the file is sent only when
/// - it parses as the server's configuration (TOML, every key known, every value of its type);
/// - every key is in the reference `h2w_api::server_config` (plus the fields of
///   `[[benchmarks.not_offered]]` and of detailed `egress.allow` entries);
/// - no text value (string, list item, name of a header, variable or secret) breaks a rule
///   the server's validator applies to that key: names have the shape of names, addresses
///   are addresses, URLs carry no credentials;
/// - no line (comments included) and no text value holds something secret-shaped:
///   a private-key or certificate block, a Workbench API token, a URL with user information,
///   a query string or a token-like path or fragment segment, a long run of token characters
///   that mixes letters and digits (a path is judged one directory at a time, and an image
///   digest `sha256:<64 hex>` is a known shape), or a credential-like name given a value.
///
/// [`refusal_or_stored_secret`] also refuses a file that holds the value of a stored secret.
#[must_use]
pub fn refusal(text: &str, env: EnvLookup<'_>) -> Option<String> {
    let Ok(table) = toml::from_str::<toml::Table>(text) else {
        return Some(R_NOT_A_CONFIG.to_owned());
    };
    let Ok(file) = Config::parse_str(text, "-") else {
        return Some(R_NOT_A_CONFIG.to_owned());
    };
    let mut strings = Vec::new();
    if !known_keys_only(&table, "", &mut strings) {
        return Some(R_UNKNOWN_KEY.to_owned());
    }
    if let Some(key) = rejected_text_value(file, env) {
        return Some(format!("the value of {key} is not one the server accepts"));
    }
    text.lines()
        .chain(strings.iter().flat_map(|s| s.lines()))
        .find_map(secret_shaped)
        .map(str::to_owned)
}

/// [`refusal`], then, only when it passes, whether the text or any string value in it (with
/// its escapes decoded) holds the value of a stored secret (`holds_secret`). The shape check
/// comes first, so a file it withholds answers the same whatever secrets are stored.
#[must_use]
pub fn refusal_or_stored_secret(
    text: &str,
    env: EnvLookup<'_>,
    holds_secret: impl Fn(&str) -> bool,
) -> Option<String> {
    if let Some(why) = refusal(text, env) {
        return Some(why);
    }
    let mut strings = Vec::new();
    if let Ok(table) = toml::from_str::<toml::Table>(text) {
        table.values().for_each(|v| strings_in(v, &mut strings));
        if holds_secret(text) || strings.iter().any(|s| holds_secret(s)) {
            return Some(R_STORED_SECRET.to_owned());
        }
        return None;
    }
    // Not reached: `refusal` refuses a text that does not parse.
    Some(R_NOT_A_CONFIG.to_owned())
}

/// Walk `table` (at dotted `prefix`): `true` when every key is known. Collects every string
/// value into `out`.
fn known_keys_only<'a>(table: &'a toml::Table, prefix: &str, out: &mut Vec<&'a str>) -> bool {
    use h2w_api::server_config::{config_key, config_section};
    table.iter().all(|(k, v)| {
        let path = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            toml::Value::Table(t) if config_section(&path).is_some() => {
                known_keys_only(t, &path, out)
            }
            _ if config_key(&path).is_none() => false,
            toml::Value::Array(items) if path == "egress.allow" => items.iter().all(|i| match i {
                toml::Value::String(s) => {
                    out.push(s);
                    true
                }
                toml::Value::Table(t) => fields_only(t, &ALLOW_FIELDS, out),
                _ => false,
            }),
            toml::Value::Array(items) if path == "benchmarks.not_offered" => {
                items.iter().all(|i| match i {
                    toml::Value::Table(t) => fields_only(t, &NOT_OFFERED_FIELDS, out),
                    _ => false,
                })
            }
            toml::Value::Table(_) => false,
            other => {
                strings_in(other, out);
                true
            }
        }
    })
}

/// `true` when `t` has only `fields` (and no nested table); collects its strings.
fn fields_only<'a>(t: &'a toml::Table, fields: &[&str], out: &mut Vec<&'a str>) -> bool {
    t.iter().all(|(k, v)| {
        if !fields.contains(&k.as_str()) || matches!(v, toml::Value::Table(_)) {
            return false;
        }
        strings_in(v, out);
        true
    })
}

fn strings_in<'a>(v: &'a toml::Value, out: &mut Vec<&'a str>) {
    match v {
        toml::Value::String(s) => out.push(s),
        toml::Value::Array(items) => items.iter().for_each(|i| strings_in(i, out)),
        toml::Value::Table(t) => t.values().for_each(|i| strings_in(i, out)),
        _ => {}
    }
}

/// The first key holding text (a string, list, option or table entry) that the server's
/// validator refuses, by its reference path. Numbers and switches out of range are not a
/// reason to withhold the file.
fn rejected_text_value(file: FileConfig, env: EnvLookup<'_>) -> Option<&'static str> {
    use h2w_api::server_config::{ConfigValueKind, config_key};
    let without_overrides = |name: &str| {
        if ENV_OVERRIDES.contains(&name) {
            None
        } else {
            env(name)
        }
    };
    let problems = match Config::from_file(file, None, &without_overrides) {
        Ok(_) => return None,
        Err(crate::config::ConfigError::Invalid { problems, .. }) => problems,
        // Not expected without overrides; refuse rather than guess.
        Err(_) => return Some("the file"),
    };
    problems.iter().find_map(|p| {
        // Problems start with the key: `key: ...`, `key[i]: ...` or `key[i].field: ...`.
        let end = p
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.')))
            .unwrap_or(p.len());
        if !matches!(p[end..].chars().next(), Some(':' | '[')) {
            return None;
        }
        config_key(&p[..end])
            .filter(|k| {
                !matches!(
                    k.kind,
                    ConfigValueKind::Bool | ConfigValueKind::Integer | ConfigValueKind::Float
                )
            })
            .map(|k| k.path)
    })
}

/// Why `line` looks like it holds a secret, if it does.
fn secret_shaped(line: &str) -> Option<&'static str> {
    if line.contains("-----BEGIN") || line.contains("PRIVATE KEY") {
        return Some(R_PEM);
    }
    if contains_api_token(line) {
        return Some(R_API_TOKEN);
    }
    if url_with_credentials(line) {
        return Some(R_URL);
    }
    if has_token_run(line) {
        return Some(R_TOKEN);
    }
    if credential_assignment(line) {
        return Some(R_ASSIGNMENT);
    }
    None
}

/// `h2w_` + 8 lowercase letters or digits + `_` + at least 32 token characters.
fn contains_api_token(text: &str) -> bool {
    let b = text.as_bytes();
    let mut from = 0;
    while let Some(i) = text[from..].find("h2w_").map(|i| i + from) {
        let id = b.get(i + 4..i + 12);
        let sep = b.get(i + 12);
        let id_ok = id.is_some_and(|id| {
            id.iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        });
        if id_ok && sep == Some(&b'_') {
            let secret = b[i + 13..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
                .count();
            if secret >= 32 {
                return true;
            }
        }
        from = i + 4;
    }
    false
}

/// `s` (at least `min` characters) mixes letters and digits the way a token does: two or
/// more digits, or one digit among letters of both cases. One digit in lowercase words
/// (`harness2-workbench-runner`) is a name, not a token.
fn token_like(s: &str, min: usize) -> bool {
    if s.chars().count() < min {
        return false;
    }
    let digits = s.chars().filter(char::is_ascii_digit).count();
    let upper = s.chars().any(|c| c.is_ascii_uppercase());
    let lower = s.chars().any(|c| c.is_ascii_lowercase());
    (upper || lower) && (digits >= 2 || (digits == 1 && upper && lower))
}

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '_' | '=' | '-')
}

/// A run of at least [`TOKEN_RUN_MIN`] token characters (`[A-Za-z0-9+/_=-]`) that is
/// [`token_like`]. A word that starts with `/` or `~` is a path and is judged one directory
/// at a time (user names and temporary directories have digits); `sha256:` followed by 64
/// hex digits is an image digest.
fn has_token_run(line: &str) -> bool {
    let words = line.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\'' | '`' | ',' | '[' | ']' | '{' | '}' | '(' | ')' | '<' | '>'
            )
    });
    for word in words {
        let parts: Vec<&str> = if word.starts_with('/') || word.starts_with('~') {
            word.split('/').collect()
        } else {
            vec![word]
        };
        for part in parts {
            let mut at = 0;
            while at < part.len() {
                let start = part[at..].find(token_char).map_or(part.len(), |i| at + i);
                let len = part[start..]
                    .find(|c: char| !token_char(c))
                    .unwrap_or(part.len() - start);
                let run = &part[start..start + len];
                let digest = part[..start].ends_with("sha256:")
                    && run.len() == 64
                    && run.chars().all(|c| c.is_ascii_hexdigit());
                if !digest && token_like(run, TOKEN_RUN_MIN) {
                    return true;
                }
                at = start + len;
            }
        }
    }
    false
}

/// A `scheme://` URL with user information (`user:pass@host`), a query string (tokens are
/// often passed as `?key=...`), or a token-like path or fragment segment.
fn url_with_credentials(line: &str) -> bool {
    let mut rest = line;
    while let Some(i) = rest.find("://") {
        let after = &rest[i + 3..];
        let end = after
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ']' | '}' | ')' | '>' | '`')
            })
            .unwrap_or(after.len());
        let url = &after[..end];
        let authority_end = url.find(['/', '?', '#']).unwrap_or(url.len());
        if url[..authority_end].contains('@') || url.contains('?') {
            return true;
        }
        if url[authority_end..]
            .split(['/', '#', ';', '&', '=', ':', '.'])
            .any(|seg| token_like(seg, URL_SEGMENT_MIN))
        {
            return true;
        }
        rest = &after[end..];
    }
    false
}

/// A key of the file or a field of a list entry, by full path or last part.
fn known_key_name(name: &str) -> bool {
    use h2w_api::server_config::CONFIG_KEYS;
    CONFIG_KEYS
        .iter()
        .any(|k| k.path == name || k.path.rsplit('.').next() == Some(name))
        || NOT_OFFERED_FIELDS.contains(&name)
        || ALLOW_FIELDS.contains(&name)
}

/// `name = <non-empty value>` anywhere on the line (a commented-out setting, `a = 1, b = 2`,
/// an inline table), or `name: <one word>` (a YAML or header-style note: `# token: abc`, but
/// not prose such as `# "token": API tokens; ...`), where the name is not a known key and
/// contains a credential word or a `key` part. The name is what follows the start, `#`, `,`,
/// `{`, `[`, `;`, `&`, `?` or the previous `=` / `:`. Linear in the line's length.
fn credential_assignment(line: &str) -> bool {
    let rest = rest_shapes(line);
    let mut name_start = 0;
    for (i, c) in line.char_indices() {
        match c {
            '#' | ',' | '{' | '[' | ';' | '&' | '?' => name_start = i + 1,
            '=' | ':' => {
                let value_ok = c == '=' || matches!(rest[i + 1], Rest::Word | Rest::SpacedWord);
                if value_ok && assigns_a_credential(&line[name_start..i], &line[i + 1..]) {
                    return true;
                }
                name_start = i + 1;
            }
            _ => {}
        }
    }
    false
}

/// What the line holds from a byte offset up to its end, a `,`, a `;` or a `#` comment.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rest {
    /// Nothing but blanks.
    Empty,
    /// One word, then blanks.
    Word,
    /// Blanks, one word, then blanks.
    SpacedWord,
    /// Anything else (two words or more).
    Words,
}

/// [`Rest`] at every byte offset of `line` (and at its end), in one pass from the right.
fn rest_shapes(line: &str) -> Vec<Rest> {
    let b = line.as_bytes();
    let mut out = vec![Rest::Empty; b.len() + 1];
    for i in (0..b.len()).rev() {
        let next = out[i + 1];
        out[i] = match b[i] {
            b',' | b';' | b'#' => Rest::Empty,
            c if c.is_ascii_whitespace() => match next {
                Rest::Empty => Rest::Empty,
                Rest::Word | Rest::SpacedWord => Rest::SpacedWord,
                Rest::Words => Rest::Words,
            },
            _ => match next {
                Rest::Empty | Rest::Word => Rest::Word,
                Rest::SpacedWord | Rest::Words => Rest::Words,
            },
        };
    }
    out
}

/// `name` (as written before `=` or `:`) looks like a credential's name and `value` (what
/// follows) is not empty.
fn assigns_a_credential(name: &str, value: &str) -> bool {
    let name = name.trim().trim_matches(['"', '\'']);
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return false;
    }
    let name = name.to_ascii_lowercase().replace('-', "_");
    if known_key_name(&name) {
        return false;
    }
    let value = value.trim_start();
    let empty = value.is_empty()
        || value.starts_with(['=', ':'])
        || ["\"\"", "''", "[]"].iter().any(|e| value.starts_with(e));
    let credential = CREDENTIAL_WORDS.iter().any(|w| name.contains(w))
        || name
            .split(['_', '.'])
            .any(|part| part == "key" || part.ends_with("key"));
    credential && !empty
}

/// Read the configuration file for the editor, at most [`CONFIG_TEXT_MAX`] bytes.
///
/// # Errors
/// A sentence saying why the text cannot be sent (never the text itself).
pub fn read_capped(path: &Path) -> Result<String, String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("it cannot be read ({})", e.kind()))?;
    let mut buf = Vec::new();
    file.take(CONFIG_TEXT_MAX as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("it cannot be read ({})", e.kind()))?;
    if buf.len() > CONFIG_TEXT_MAX {
        return Err(format!("it is larger than {} KiB", CONFIG_TEXT_MAX / 1024));
    }
    String::from_utf8(buf).map_err(|_| "it is not UTF-8 text".to_owned())
}

/// `true` when `text` (with `env`'s overrides) gives different settings than `running`.
#[must_use]
pub fn differs_from(text: &str, running: &FileConfig, env: EnvLookup<'_>) -> bool {
    match Config::parse_str(text, "-") {
        Ok(mut file) => {
            let _ = crate::config::apply_env_overrides(&mut file, env);
            &file != running
        }
        Err(_) => true,
    }
}

/// 1-based line and column (in characters) of byte offset `at` in `text`.
fn line_col(text: &str, at: usize) -> (u32, u32) {
    let at = at.min(text.len());
    let mut start = at;
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    let before = &text[..start];
    let line = before.matches('\n').count() + 1;
    let col = before
        .rsplit_once('\n')
        .map_or(before, |(_, l)| l)
        .chars()
        .count()
        + 1;
    (
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(col).unwrap_or(u32::MAX),
    )
}

/// `true` for a backticked fragment of a parser message that is safe to show: punctuation
/// (`expected \`.\`, \`]\``) or a name of the file format (a table or key name, an option of
/// a key with fixed options, a benchmark), never something only the posted text holds.
fn known_word(frag: &str) -> bool {
    if frag
        .chars()
        .all(|c| c.is_ascii_punctuation() || c.is_whitespace())
    {
        return true;
    }
    let segment = |path: &str| path.split('.').any(|p| p == frag);
    h2w_api::server_config::CONFIG_SECTIONS
        .iter()
        .any(|s| segment(s.path))
        || h2w_api::server_config::CONFIG_KEYS.iter().any(|k| {
            segment(k.path)
                || matches!(k.kind, h2w_api::server_config::ConfigValueKind::Enum(opts)
                    if opts.contains(&frag))
        })
        || NOT_OFFERED_FIELDS.contains(&frag)
        || ALLOW_FIELDS.contains(&frag)
        || h2w_api::Bench::ALL.iter().any(|b| b.as_cli() == frag)
}

/// A parser or validator message without anything it quotes from the posted text: double-
/// quoted fragments become `"…"` ([`crate::error::scrub_quoted`]), and backticked fragments
/// become `` `…` `` unless they are punctuation or a name of the file format.
///
/// A backticked value is not escaped (`` unknown field `x`TAIL`, expected ... ``), so the
/// fragment of an unknown word ends at the first backtick after which the rest of the message
/// reads as serde's own ending ([`serde_tail`]); when none does, everything after the
/// opening backtick is dropped.
#[must_use]
pub fn scrub_echo(msg: &str) -> String {
    let msg = crate::error::scrub_quoted(msg);
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg.as_str();
    while let Some(i) = rest.find('`') {
        out.push_str(&rest[..=i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('`') else {
            out.push_str("…`");
            rest = "";
            break;
        };
        let frag = &after[..j];
        if known_word(frag) {
            out.push_str(frag);
            out.push('`');
            rest = &after[j + 1..];
            continue;
        }
        out.push_str("…`");
        let close = after
            .match_indices('`')
            .map(|(k, _)| k)
            .find(|&k| serde_tail(&after[k + 1..]));
        match close {
            Some(k) => rest = &after[k + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `true` when `tail` is how serde ends a message after a quoted value: nothing,
/// `, there are no fields|variants`, or `, expected` + known names (`` `a` ``, `` `a` or `b` ``,
/// `` one of `a`, `b` ``) or a plain description (`, expected a string`).
fn serde_tail(tail: &str) -> bool {
    if tail.is_empty() || tail == ", there are no fields" || tail == ", there are no variants" {
        return true;
    }
    let Some(list) = tail.strip_prefix(", expected ") else {
        return false;
    };
    if !list.contains('`') {
        return list
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == ' ');
    }
    let list = list.strip_prefix("one of ").unwrap_or(list);
    let parts: Vec<&str> = list.split('`').collect();
    // `a`, `b` or `c` splits into "", a, ", ", b, " or ", c, "".
    parts.len() % 2 == 1
        && parts.iter().enumerate().all(|(n, p)| {
            if n % 2 == 1 {
                known_word(p)
            } else if n == 0 || n == parts.len() - 1 {
                p.is_empty()
            } else {
                matches!(*p, ", " | " or ")
            }
        })
}

/// Check `text` as `h2workbench serve` would load it: parse (unknown keys are errors), then
/// validate. `env` supplies `HOME` and the XDG variables for paths; the `H2W_*` overrides are
/// NOT applied, so the answer is about the text. Writes nothing, and reads no secret.
///
/// Messages never quote the posted text ([`scrub_echo`]): the line and column, the key and
/// the rule say what is wrong.
#[must_use]
pub fn check_text(text: &str, env: EnvLookup<'_>) -> ConfigValidation {
    let mut v = check_text_raw(text, env);
    if let Some(p) = v.parse_error.as_mut() {
        p.message = scrub_echo(&p.message);
    }
    for s in v.problems.iter_mut().chain(v.warnings.iter_mut()) {
        *s = scrub_echo(s);
    }
    v
}

fn check_text_raw(text: &str, env: EnvLookup<'_>) -> ConfigValidation {
    let file = match toml::from_str::<FileConfig>(text) {
        Ok(f) => f,
        Err(e) => {
            let (line, column) = e
                .span()
                .map(|s| line_col(text, s.start))
                .map_or((None, None), |(l, c)| (Some(l), Some(c)));
            return ConfigValidation {
                valid: false,
                parse_error: Some(ConfigParseError {
                    message: e.message().trim().to_owned(),
                    line,
                    column,
                }),
                problems: Vec::new(),
                warnings: Vec::new(),
            };
        }
    };
    let without_overrides = |name: &str| {
        if ENV_OVERRIDES.contains(&name) {
            None
        } else {
            env(name)
        }
    };
    match Config::from_file(file, None, &without_overrides) {
        Ok(cfg) => ConfigValidation {
            valid: true,
            parse_error: None,
            problems: Vec::new(),
            warnings: cfg.warnings(),
        },
        Err(crate::config::ConfigError::Invalid { problems, .. }) => ConfigValidation {
            valid: false,
            parse_error: None,
            problems,
            warnings: Vec::new(),
        },
        Err(e) => ConfigValidation {
            valid: false,
            parse_error: None,
            problems: vec![e.to_string()],
            warnings: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn env(name: &str) -> Option<String> {
        match name {
            "HOME" => Some("/srv/h2w-test-home".to_owned()),
            "H2W_LISTEN" => Some("not an address".to_owned()),
            _ => None,
        }
    }

    const EXAMPLE: &str = include_str!("../../../deploy/examples/h2workbench.toml");
    const COMPOSE_EXAMPLE: &str = include_str!("../../../deploy/examples/h2workbench.compose.toml");

    #[test]
    fn shipped_examples_validate_and_are_not_refused() {
        for text in [EXAMPLE, COMPOSE_EXAMPLE] {
            let v = check_text(text, &env);
            assert!(v.valid, "{v:?}");
            assert_eq!(refusal(text, &env), None);
        }
        let empty = check_text("", &env);
        assert!(empty.valid, "{empty:?}");
    }

    #[test]
    fn overrides_are_not_applied() {
        // H2W_LISTEN is invalid in `env`, yet the text is judged on its own.
        assert!(check_text("[server]\nlisten = \"127.0.0.1:9000\"\n", &env).valid);
        assert_eq!(overrides_in_force(&env), vec!["H2W_LISTEN".to_owned()]);
    }

    /// Every override, each set to a value the server would refuse, changes neither the
    /// verdict on a valid text nor the problems of an invalid one.
    #[test]
    fn validate_ignores_all_five_overrides() {
        let hostile = |n: &str| match n {
            "HOME" => Some("/srv/h2w-test-home".to_owned()),
            _ if ENV_OVERRIDES.contains(&n) => Some("~nobody/not valid".to_owned()),
            _ => None,
        };
        let plain = |n: &str| (n == "HOME").then(|| "/srv/h2w-test-home".to_owned());
        assert_eq!(overrides_in_force(&hostile).len(), 5);
        for text in [EXAMPLE, COMPOSE_EXAMPLE, "[jobs]\nmax_concurrent = 0\n"] {
            let with = check_text(text, &hostile);
            let without = check_text(text, &plain);
            assert_eq!(with.valid, without.valid, "{text}");
            assert_eq!(with.problems, without.problems, "{text}");
        }
        assert!(check_text(EXAMPLE, &hostile).valid);
    }

    #[test]
    fn parse_errors_have_a_position_and_no_echo_of_the_line() {
        let v = check_text("[server]\nlisten = \"127.0.0.1:1\"\nlistne = 1\n", &env);
        assert!(!v.valid);
        let p = v.parse_error.unwrap();
        assert_eq!(p.line, Some(3));
        assert_eq!(p.column, Some(1));
        // The unknown key is not echoed; the known names it could have been are.
        assert!(p.message.contains("unknown field `…`"), "{}", p.message);
        assert!(p.message.contains("`listen`"), "{}", p.message);
        assert!(!p.message.contains("listne"), "{}", p.message);
        let v = check_text("[server\n", &env);
        assert_eq!(v.parse_error.unwrap().line, Some(1));
        let v = check_text("a = \"é\"\nb = ", &env);
        assert_eq!(v.parse_error.unwrap().line, Some(2));
    }

    #[test]
    fn messages_never_echo_posted_values() {
        let guess = "gk-0123456789abcdefghij";
        for text in [
            format!("[server]\nlisten = \"{guess}\"\n"),
            format!("[server]\nlisten = 7\npublic_url = \"{guess}\"\n"),
            format!("[auth]\nmode = \"{guess}\"\n"),
            format!("[jobs]\nmax_concurrent = \"{guess}\"\n"),
            format!("[paths]\ndata_dir = \"{guess}\"\n"),
            format!("[paths]\ndata_dir = \"~{guess}\"\n"),
            format!("[telemetry]\nmetrics_token_secret = \"{guess}\"\n"),
            format!("[isolation.container]\nimage = \"{guess}\"\n"),
            format!("[[benchmarks.not_offered]]\nbench = \"{guess}\"\n"),
            format!("[server]\n{guess} = 1\n"),
        ] {
            let v = check_text(&text, &env);
            assert!(!v.valid, "{text}");
            let all = format!("{v:?}");
            assert!(!all.contains("0123456789"), "{text}: {all}");
        }
        // A quote or backtick inside the value does not end the hidden part early.
        for text in [
            "[server]\n\"x`TAIL\" = 1\n",
            "[server]\n\"x`, TAIL\" = 1\n",
            "[\"x`TAIL\"]\n",
            "[auth]\nmode = \"x`TAIL\"\n",
            "[jobs]\nmax_concurrent = \"x\\\"TAIL\"\n",
            "[server]\nlisten = 'x\"TAIL'\n",
            "[server]\nlisten = 'x\\\"TAIL'\n",
        ] {
            let v = check_text(text, &env);
            assert!(!v.valid, "{text}");
            let all = format!("{v:?}");
            assert!(!all.contains("TAIL"), "{text}: {all}");
        }
        // The answer is the same whatever the value: no way to tell a stored secret apart.
        let a = check_text(&format!("[server]\nlisten = \"{guess}\"\n"), &env);
        let b = check_text("[server]\nlisten = \"something else\"\n", &env);
        assert_eq!(a, b);
    }

    #[test]
    fn scrub_echo_keeps_names_and_punctuation() {
        assert_eq!(
            scrub_echo("unknown variant `tok`, expected `token` or `proxy`"),
            "unknown variant `…`, expected `token` or `proxy`"
        );
        assert_eq!(
            scrub_echo("invalid type: string \"x\", expected `.`, `]`"),
            "invalid type: string \"…\", expected `.`, `]`"
        );
        assert_eq!(scrub_echo("open `abc"), "open `…`");
        // A backtick or quote inside the value does not end it early.
        for (msg, want) in [
            (
                "unknown field `x`TAIL`, expected one of `listen`, `public_url`",
                "unknown field `…`, expected one of `listen`, `public_url`",
            ),
            (
                "unknown field `x`, TAIL`, expected `listen` or `public_url`",
                "unknown field `…`, expected `listen` or `public_url`",
            ),
            (
                "unknown field `x`, expected `listen`TAIL`, expected `listen`",
                "unknown field `…`, expected `listen`",
            ),
            (
                "unknown variant `x`TAIL`, expected `token` or `proxy`",
                "unknown variant `…`, expected `token` or `proxy`",
            ),
            (
                "unknown field `x`TAIL`, there are no fields",
                "unknown field `…`, there are no fields",
            ),
            (
                "invalid type: integer `7`, expected a string",
                "invalid type: integer `…`, expected a string",
            ),
            ("unknown field `x`TAIL` and more", "unknown field `…`"),
            (
                "invalid type: string \"x\\\"TAIL\", expected u32",
                "invalid type: string \"…\", expected u32",
            ),
        ] {
            assert_eq!(scrub_echo(msg), want, "{msg}");
        }
        assert_eq!(scrub_echo("no quotes"), "no quotes");
    }

    #[test]
    fn validation_problems_are_listed() {
        let v = check_text(
            "[auth]\nmode = \"proxy\"\n[jobs]\nmax_concurrent = 0\n",
            &env,
        );
        assert!(!v.valid);
        assert!(v.parse_error.is_none());
        assert!(
            v.problems
                .iter()
                .any(|p| p.starts_with("jobs.max_concurrent")),
            "{v:?}"
        );
        assert!(
            v.problems.iter().any(|p| p.contains("trusted_proxies")),
            "{v:?}"
        );
    }

    #[test]
    fn warnings_are_reported() {
        let v = check_text("[server]\npublic_url = \"https://wb.example.com\"\n", &env);
        assert!(v.valid);
        assert_eq!(v.warnings.len(), 1);
    }

    /// A refusal is one of the fixed sentences, never a piece of the file.
    fn refused(text: &str) -> String {
        let why = refusal(text, &env).unwrap_or_else(|| panic!("not refused: {text}"));
        let fixed = [
            R_NOT_A_CONFIG,
            R_UNKNOWN_KEY,
            R_PEM,
            R_API_TOKEN,
            R_URL,
            R_TOKEN,
            R_ASSIGNMENT,
        ];
        assert!(
            fixed.contains(&why.as_str())
                || (why.starts_with("the value of ")
                    && why.ends_with(" is not one the server accepts")),
            "{why}"
        );
        why
    }

    const HEX64: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn files_the_server_writes_or_ships_are_sent() {
        let defaults = toml::to_string(&FileConfig::default()).unwrap();
        let init = crate::config::render_initial(Path::new(
            "/srv/user123/.local/share/harness2-workbench",
        ))
        .unwrap();
        for text in [
            defaults.as_str(),
            init.as_str(),
            EXAMPLE,
            COMPOSE_EXAMPLE,
            "",
        ] {
            assert_eq!(refusal(text, &env), None, "{text}");
        }
    }

    #[test]
    fn ordinary_settings_are_sent() {
        for ok in [
            // Paths are judged one directory at a time: user names and temporary
            // directories have digits.
            "[paths]\ndata_dir = \"/srv/alice2024/.local/share/harness2-workbench\"\n\
             experiments_dir = \"/tmp/.tmpXy3AbQ/experiments\"\n\
             harness2_home = \"~/src/Projects2024/harness2\"\n",
            &format!(
                "[isolation.container]\nimage = \"ghcr.io/example/harness2-workbench-runner@sha256:{HEX64}\"\n"
            ),
            "[auth.proxy]\nsecret_env = \"H2W_PROXY_SECRET\"\nsecret_header = \"x-h2w-proxy-secret\"\n",
            "[telemetry]\nmetrics_token_secret = \"METRICS\"\n",
            "[server]\npublic_url = \"https://wb.example.com:8443/\"\n",
            // A quoted key that is a known setting.
            "[server]\n\"listen\" = \"127.0.0.1:8722\"\n",
            "[egress]\nallow = [\"api.example.com\", { host = \"litellm\", allow_private = true, ports = [4000] }]\n",
            "[[benchmarks.not_offered]]\nbench = \"wb\"\nnote = \"Runs on the host-isolation Workbench.\"\n\
             url = \"https://github.com/example/harness2-workbench#readme\"\n",
            "# the token = what `init` prints\n",
            "# token = \"\"\n",
            "# h2w_abcd1234_short\n",
            "# mail someone@example.com\n",
            // Numbers out of range are a validation problem, not a reason to withhold.
            "[jobs]\nmax_concurrent = 0\n",
        ] {
            assert_eq!(refusal(ok, &env), None, "{ok}");
        }
    }

    #[test]
    fn unknown_keys_and_unreadable_files_are_refused() {
        for (text, why) in [
            ("password = \"hunter2\"\n", R_NOT_A_CONFIG),
            ("[x]\nmy_token = 3\n", R_NOT_A_CONFIG),
            ("[server]\nlisten_token = \"x\"\n", R_NOT_A_CONFIG),
            ("[telemetry]\n\"api-key\" = \"x\"\n", R_NOT_A_CONFIG),
            ("[server]\n\"pass word\" = 1\n", R_NOT_A_CONFIG),
            (
                "[egress]\nallow = [{ host = \"a.example.com\", token = \"x\" }]\n",
                R_NOT_A_CONFIG,
            ),
            ("[server\n", R_NOT_A_CONFIG),
        ] {
            assert_eq!(refused(text), why, "{text}");
        }
        // The walk refuses on its own what the schema would let through.
        let table: toml::Table =
            toml::from_str("[server]\nlisten = \"a\"\n[nope]\nx = 1\n").unwrap();
        assert!(!known_keys_only(&table, "", &mut Vec::new()));
        let table: toml::Table =
            toml::from_str("[[benchmarks.not_offered]]\nbench = \"wb\"\ntoken = \"x\"\n").unwrap();
        assert!(!known_keys_only(&table, "", &mut Vec::new()));
        let table: toml::Table = toml::from_str("[server.listen]\nx = 1\n").unwrap();
        assert!(!known_keys_only(&table, "", &mut Vec::new()));
    }

    #[test]
    fn list_entry_fields_match_the_server() {
        let entry = crate::config::NotOfferedEntry {
            bench: h2w_api::Bench::Wb,
            note: "n".into(),
            url: "u".into(),
        };
        let t: toml::Table = toml::from_str(&toml::to_string(&entry).unwrap()).unwrap();
        assert_eq!(t.keys().map(String::as_str).collect::<Vec<_>>().len(), 3);
        assert!(t.keys().all(|k| NOT_OFFERED_FIELDS.contains(&k.as_str())));
        let d = crate::config::AllowEntryDetailed {
            host: "h".into(),
            allow_private: true,
            ports: vec![1],
        };
        let t: toml::Table = toml::from_str(&toml::to_string(&d).unwrap()).unwrap();
        assert_eq!(t.len(), 3);
        assert!(t.keys().all(|k| ALLOW_FIELDS.contains(&k.as_str())));
    }

    #[test]
    fn name_only_keys_must_hold_names() {
        for text in [
            "[auth.proxy]\nsecret_env = \"hunter2 hunter2\"\n",
            "[auth.proxy]\nsecret_env = \"lower_case\"\n",
            "[auth.proxy]\nsecret_header = \"p@ss word\"\n",
            "[telemetry]\nmetrics_token_secret = \"not-a-name\"\n",
            "[server]\nlisten = \"hunter2\"\n",
            "[server]\nallowed_hosts = [\"pass:word\"]\n",
            "[server]\ntrusted_proxies = [\"hunter2\"]\n",
            "[isolation.container]\nproxy = \"http://u:p@proxy:3128\"\n",
            "[[benchmarks.not_offered]]\nbench = \"wb\"\nurl = \"ftp://x\"\n",
        ] {
            let why = refused(text);
            assert!(why.starts_with("the value of "), "{text}: {why}");
        }
        assert_eq!(
            refused("[auth.proxy]\nsecret_env = \"hunter2\"\n"),
            "the value of auth.proxy.secret_env is not one the server accepts"
        );
    }

    #[test]
    fn credential_names_in_comments_are_refused() {
        // `-` is read as `_`, case is ignored, every `=` on a line is looked at.
        for text in [
            "# api-key = abc\n",
            "# Api-Key = abc\n",
            "# api_key = \"x\"\n",
            "# a = 1, password = hunter2\n",
            "# x = { user = \"a\", password = \"b\" }\n",
            "# PASSWORD=hunter2\n",
            // `name: value` with a one-word value, as in YAML or a header.
            "# password: hunter2\n",
            "# db-password:hunter2\n",
            "# \"api_key\": abc, \"user\": x\n",
            "# x-auth-token: abc # old\n",
            "[server]\nlisten = \"127.0.0.1:8722\" # token: abc\n",
        ] {
            assert_eq!(refused(text), R_ASSIGNMENT, "{text}");
        }
        // Prose after a `:` is not a value, and known keys are not credentials.
        for text in [
            "# \"token\": API tokens; the web UI exchanges one for a cookie.\n",
            "# auth: see the admin guide\n",
            "# secret_header: the header name\n",
            "# token:\n",
            "[server]\npublic_url = \"https://wb.example.com:8443/\"\n",
        ] {
            assert_eq!(refusal(text, &env), None, "{text}");
        }
        // One pass: a long line with many `=` and `:` is still read quickly.
        let long = format!("# {}\n", "a".repeat(100_000) + &"=:".repeat(50_000));
        assert_eq!(refusal(&long, &env), None);
        for name in [
            "passphrase",
            "db_pass",
            "pwd",
            "credentials",
            "basic_auth",
            "bearer",
            "jwt",
            "session_cookie",
            "sentry_dsn",
            "hmac",
            "signing_key",
            "salt",
            "access_key",
            "license_key",
        ] {
            let text = format!("# {name} = hunter2\n");
            assert_eq!(refused(&text), R_ASSIGNMENT, "{text}");
        }
    }

    #[test]
    fn secret_shapes_are_refused_anywhere() {
        // Built at run time so the source holds no key-shaped literal.
        let token = ["AbCdEf", "0123456789", "XyZ0123"].concat();
        for (text, why) in [
            // Comments, free-text values, list items, inline tables.
            (format!("# old key {token}\n"), R_TOKEN),
            (format!("# {token}=\n"), R_TOKEN),
            (format!("[telemetry]\nservice_name = \"svc {token}\"\n"), R_TOKEN),
            (format!("[auth.proxy]\nadmin_groups = [\"admins\", \"{token}\"]\n"), R_TOKEN),
            (
                format!("[[benchmarks.not_offered]]\nbench = \"wb\"\nnote = \"key {token}\"\n"),
                R_TOKEN,
            ),
            // Multi-line strings: every line is looked at.
            (format!("[telemetry]\nservice_name = \"\"\"\nsvc\n{token}\n\"\"\"\n"), R_TOKEN),
            // An escape that splits the token in the file is decoded before the check.
            (
                "[telemetry]\nservice_name = \"AbCdEf0123\\u0034567XyZ0123\"\n".to_owned(),
                R_TOKEN,
            ),
            // A path is judged one directory at a time, a long token in one is still found.
            (format!("[paths]\ndata_dir = \"/srv/{token}/data\"\n"), R_TOKEN),
            // URLs.
            (
                "[telemetry]\notlp_endpoint = \"https://user:pw@otel.example.com\"\n".to_owned(),
                R_URL,
            ),
            ("# collector at http://u@192.0.2.1:4318\n".to_owned(), R_URL),
            (
                "[telemetry]\notlp_endpoint = \"https://otel.example.com/v1/traces?api_key=abc\"\n"
                    .to_owned(),
                R_URL,
            ),
            (
                "[telemetry]\notlp_endpoint = \"https://otel.example.com/v1/Tk9aX2k3Lm8pQr5s/traces\"\n"
                    .to_owned(),
                R_URL,
            ),
            (
                "[[benchmarks.not_offered]]\nbench = \"wb\"\nurl = \"https://wb.example.com/#Tk9aX2k3Lm8pQr5s\"\n"
                    .to_owned(),
                R_URL,
            ),
        ] {
            assert_eq!(refused(&text), why, "{text}");
        }
        // Built at run time so the source holds no key- or token-shaped literal (the
        // repository's pre-commit check refuses those).
        let pem = format!("# -----BEGIN {} KEY-----\n", "PRIVATE");
        let api = format!(
            "# h2w_{}_{}\n",
            "abcd1234", "0123456789abcdefghijABCDEFGHIJ0123456789x"
        );
        assert_eq!(refused(&pem), R_PEM);
        assert_eq!(refused(&api), R_API_TOKEN);
    }

    #[test]
    fn stored_secrets_are_checked_after_the_shapes_and_in_decoded_strings() {
        let stored = ["correct", "horse", "battery", "staple"].join("-");
        let holds = |s: &str| s.contains(&stored);
        // In a comment, and in a string value whose escapes split it in the file.
        let comment = format!("# old {stored}\n");
        let escaped = "[telemetry]\nservice_name = \"correct-horse-batt\\u0065ry-staple\"\n";
        assert!(!escaped.contains(&stored));
        for text in [comment.as_str(), escaped] {
            assert_eq!(refusal(text, &env), None, "{text}");
            assert_eq!(
                refusal_or_stored_secret(text, &env, holds).as_deref(),
                Some(R_STORED_SECRET),
                "{text}"
            );
        }
        // A file the shapes refuse gives that reason, secret or not.
        let both = format!("[extra]\nnote = \"{stored}\"\n");
        assert_eq!(
            refusal_or_stored_secret(&both, &env, holds).as_deref(),
            Some(R_NOT_A_CONFIG)
        );
        assert_eq!(refusal_or_stored_secret(EXAMPLE, &env, holds), None);
    }

    #[test]
    fn token_shape_rules() {
        assert!(token_like("AbCdEf0123456789XyZ0123", TOKEN_RUN_MIN));
        assert!(token_like("0123456789abcdef01234567", TOKEN_RUN_MIN));
        assert!(token_like("Abcdefghijklmnopqrs1", TOKEN_RUN_MIN));
        assert!(!token_like("harness2-workbench-runner", TOKEN_RUN_MIN));
        assert!(!token_like("allow_untested_harness2_rev", TOKEN_RUN_MIN));
        assert!(!token_like("HTTPS_PROXY/HTTP_PROXY", TOKEN_RUN_MIN));
        assert!(!token_like("short12", TOKEN_RUN_MIN));
        assert!(!token_like("01234567890123456789012", TOKEN_RUN_MIN));
        assert!(has_token_run(&format!("x = \"sha1:{HEX64}\"")));
        assert!(!has_token_run(&format!("x = \"r@sha256:{HEX64}\"")));
    }

    #[test]
    fn read_capped_refuses_large_and_binary_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let big = dir.join("big.toml");
        std::fs::write(&big, vec![b'#'; CONFIG_TEXT_MAX + 1]).unwrap();
        assert!(read_capped(&big).unwrap_err().contains("larger"));
        let bin = dir.join("bin.toml");
        std::fs::write(&bin, [0xff, 0xfe]).unwrap();
        assert!(read_capped(&bin).unwrap_err().contains("UTF-8"));
        let ok = dir.join("ok.toml");
        std::fs::write(&ok, "[server]\n").unwrap();
        assert_eq!(read_capped(&ok).unwrap(), "[server]\n");
        assert!(
            read_capped(&dir.join("missing.toml"))
                .unwrap_err()
                .contains("cannot be read")
        );
    }

    #[test]
    fn differs_from_running() {
        let running = Config::parse_str("[jobs]\nmax_concurrent = 3\n", "-").unwrap();
        let none = |_: &str| None;
        assert!(!differs_from(
            "[jobs]\nmax_concurrent = 3\n",
            &running,
            &none
        ));
        assert!(differs_from(
            "[jobs]\nmax_concurrent = 4\n",
            &running,
            &none
        ));
        assert!(differs_from("[jobs\n", &running, &none));
    }

    #[test]
    fn line_col_counts_characters() {
        assert_eq!(line_col("ab\ncé d", 7), (2, 4));
        assert_eq!(line_col("", 5), (1, 1));
    }
}
