//! Validators for every identifier the server accepts from a request or reads from disk.
//!
//! All use whole-string matching (never a `$` anchor, which lets a trailing newline
//! through). Each returns [`CoreError::Invalid`] with a user-facing problem sentence.

use crate::{CoreError, Result};

fn bad(field: &str, problem: impl Into<String>) -> CoreError {
    CoreError::invalid(field, problem)
}

/// `^[first][rest]{0,max_len-1}$` over ASCII bytes.
fn simple(
    s: &str,
    field: &str,
    first: fn(u8) -> bool,
    rest: fn(u8) -> bool,
    max_len: usize,
    rule: &str,
) -> Result<()> {
    let b = s.as_bytes();
    match b.split_first() {
        Some((f, r)) if first(*f) && r.iter().all(|&c| rest(c)) && b.len() <= max_len => Ok(()),
        _ => Err(bad(field, rule)),
    }
}

fn lower_digit(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

fn upper_digit_us(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_'
}

const NAME_RULE: &str =
    "use 1-63 lowercase letters, digits, '_' or '-', starting with a letter or digit";

/// Experiment and split-file names: `^[a-z0-9][a-z0-9_-]{0,62}$`.
pub fn experiment_name(s: &str) -> Result<()> {
    simple(
        s,
        "name",
        lower_digit,
        |c| lower_digit(c) || c == b'_' || c == b'-',
        63,
        NAME_RULE,
    )
}

/// Model profile names: same rule as experiment names.
pub fn profile_name(s: &str) -> Result<()> {
    experiment_name(s).map_err(|_| bad("profile", NAME_RULE))
}

/// Secret names: `^[A-Z][A-Z0-9_]{0,63}$`.
pub fn secret_name(s: &str) -> Result<()> {
    simple(
        s,
        "secret",
        |c| c.is_ascii_uppercase(),
        upper_digit_us,
        64,
        "use 1-64 uppercase letters, digits or '_', starting with a letter",
    )
}

/// Token display names: 1..=64 chars of letters, digits, space, `._-@`.
pub fn token_name(s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b' ' | b'.' | b'_' | b'-' | b'@'));
    if ok {
        Ok(())
    } else {
        Err(bad(
            "name",
            "use 1-64 letters, digits, spaces or the characters . _ - @",
        ))
    }
}

/// Tags on experiments: `^[a-z0-9][a-z0-9_.-]{0,31}$`.
pub fn tag(s: &str) -> Result<()> {
    simple(
        s,
        "tag",
        lower_digit,
        |c| lower_digit(c) || matches!(c, b'_' | b'.' | b'-'),
        32,
        "use 1-32 lowercase letters, digits, '_', '.' or '-', starting with a letter or digit",
    )
}

/// A domain name as harness2 reports it (passed to `--only-domains`): 1..=200 chars, no
/// comma (the list separator), no control characters, no leading `-`.
///
/// Leading or trailing whitespace is refused too: harness2 strips each `--only-domains`
/// item, so such a name could never be selected.
pub fn domain(s: &str) -> Result<()> {
    let n = s.chars().count();
    if n == 0 || n > 200 {
        return Err(bad("domain", "must be 1-200 characters"));
    }
    if s.contains(',') {
        return Err(bad("domain", "must not contain a comma"));
    }
    if s.chars().any(char::is_control) {
        return Err(bad("domain", "must not contain control characters"));
    }
    if s.starts_with('-') {
        return Err(bad("domain", "must not start with '-'"));
    }
    if crate::pyfmt::strip(s) != s {
        return Err(bad(
            "domain",
            "must not start or end with whitespace (harness2 strips it)",
        ));
    }
    Ok(())
}

/// A task directory name (task id with `/` replaced by `__`): 1..=255 bytes, no `/`, `\`,
/// NUL or control characters, not `.` or `..`.
pub fn task_slug(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 255 {
        return Err(bad("task", "must be 1-255 bytes"));
    }
    if s == "." || s == ".." {
        return Err(bad("task", "must not be '.' or '..'"));
    }
    if s.contains(['/', '\\']) || s.chars().any(char::is_control) {
        return Err(bad(
            "task",
            "must not contain '/', '\\' or control characters",
        ));
    }
    Ok(())
}

/// Harness labels: `^(base[0-9]*|inv[0-9]+|dom[0-9]+|agg[0-9]+|c[0-9]+)$`.
///
/// Also at most 32 bytes, so a label cannot carry an absurd path segment.
pub fn harness_label(s: &str) -> Result<()> {
    let digits = |rest: &str, allow_empty: bool| {
        (allow_empty || !rest.is_empty()) && rest.bytes().all(|c| c.is_ascii_digit())
    };
    let ok = s.len() <= 32
        && if let Some(r) = s.strip_prefix("base") {
            digits(r, true)
        } else if let Some(r) = s
            .strip_prefix("inv")
            .or_else(|| s.strip_prefix("dom"))
            .or_else(|| s.strip_prefix("agg"))
            .or_else(|| s.strip_prefix('c'))
        {
            digits(r, false)
        } else {
            false
        };
    if ok {
        Ok(())
    } else {
        Err(bad(
            "label",
            "expected base, base<N>, inv<N>, dom<N>, agg<N> or c<N>",
        ))
    }
}

/// Model ids as harness2 takes them (`google-vertex/gemini-3.5-flash`, `glm-5`):
/// `^[A-Za-z0-9][A-Za-z0-9._:/@+-]{0,199}$`.
pub fn model_id(s: &str) -> Result<()> {
    simple(
        s,
        "model",
        |c| c.is_ascii_alphanumeric(),
        |c| {
            c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'/' | b'@' | b'+' | b'-')
        },
        200,
        "use 1-200 letters, digits or . _ : / @ + -, starting with a letter or digit",
    )
}

/// A full 40-hex lowercase git commit id.
pub fn commit(s: &str) -> Result<()> {
    if s.len() == 40
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        Ok(())
    } else {
        Err(bad(
            "commit",
            "expected a full 40-character lowercase hex commit id",
        ))
    }
}

/// Environment variable names for profile `extra_env`: `^[A-Z_][A-Z0-9_]{0,127}$` and not
/// in [`crate::env::RESERVED_ENV`] nor starting with a reserved prefix.
pub fn env_name(s: &str) -> Result<()> {
    simple(
        s,
        "env",
        |c| c.is_ascii_uppercase() || c == b'_',
        upper_digit_us,
        128,
        "use 1-128 uppercase letters, digits or '_', not starting with a digit",
    )?;
    if crate::env::RESERVED_ENV.contains(&s) {
        return Err(bad(
            "env",
            format!("{s} is managed by the server and cannot be set by a profile"),
        ));
    }
    if let Some(p) = crate::env::RESERVED_ENV_PREFIXES
        .iter()
        .find(|p| s.starts_with(*p))
    {
        return Err(bad(
            "env",
            format!("names starting with {p} are reserved and cannot be set by a profile"),
        ));
    }
    Ok(())
}

fn ldh_label(l: &str) -> bool {
    let b = l.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b.iter().all(|&c| lower_digit(c) || c == b'-')
        && b.first() != Some(&b'-')
        && b.last() != Some(&b'-')
}

/// A DNS hostname for egress allowlists (lowercase LDH labels, optional leading `*.`).
pub fn hostname(s: &str) -> Result<()> {
    let body = s.strip_prefix("*.").unwrap_or(s);
    let ok = !body.is_empty() && body.len() <= 253 && body.split('.').all(ldh_label);
    if ok {
        Ok(())
    } else {
        Err(bad(
            "host",
            "expected a lowercase host name such as api.example.com or *.example.com",
        ))
    }
}

/// Host part of an authority: a DNS name / IPv4 (letters, digits, `.`, `-`) or a bracketed
/// IPv6 literal.
fn authority_host_ok(host: &str) -> bool {
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return !inner.is_empty()
            && inner.len() <= 64
            && inner.contains(':')
            && inner
                .bytes()
                .all(|c| c.is_ascii_hexdigit() || c == b':' || c == b'.');
    }
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
}

/// Split an http(s) URL into its lowercase host and optional port; `None` when it is not a
/// valid [`base_url`].
pub(crate) fn url_host(s: &str) -> Option<(String, Option<u16>)> {
    let rest = s
        .strip_prefix("http://")
        .or_else(|| s.strip_prefix("https://"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (rest.get(..i)?, rest.get(i..)?),
        None => (rest, ""),
    };
    if authority.contains('@') {
        return None;
    }
    let (host, port) = if authority.starts_with('[') {
        let close = authority.find(']')?;
        let host = authority.get(..=close)?;
        match authority.get(close + 1..)? {
            "" => (host, None),
            p => (host, Some(p.strip_prefix(':')?)),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    if !authority_host_ok(host) {
        return None;
    }
    let port = match port {
        None => None,
        Some(p) => {
            if p.is_empty() || p.len() > 5 || !p.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let v: u16 = p.parse().ok()?;
            if v == 0 {
                return None;
            }
            Some(v)
        }
    };
    let path_ok = path
        .chars()
        .all(|c| !c.is_control() && !c.is_whitespace() && !matches!(c, '?' | '#' | '\\' | '@'));
    if !path_ok {
        return None;
    }
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    Some((host, port))
}

/// An `http`/`https` base URL with a host and no credentials, query or fragment.
pub fn base_url(s: &str) -> Result<()> {
    if s.len() > 2048 || url_host(s).is_none() {
        return Err(bad(
            "base_url",
            "expected an http:// or https:// URL with a host and no credentials, query or \
             fragment, e.g. http://127.0.0.1:4000/v1",
        ));
    }
    Ok(())
}

/// A relative path inside a browsed directory (`a/b/c.txt`): no absolute paths, no `..`,
/// no backslash, no control characters (NUL, newlines, the escape bytes of terminal
/// sequences), at most 1024 bytes. Empty string = the root.
///
/// Also refused: empty and `.` components (`a//b`, `./a`, `a/`), so every accepted path has
/// exactly one spelling.
pub fn relative_path(s: &str) -> Result<()> {
    if s.is_empty() {
        return Ok(());
    }
    if s.len() > 1024 {
        return Err(bad("path", "must be at most 1024 bytes"));
    }
    if s.starts_with('/') {
        return Err(bad("path", "must be relative (no leading '/')"));
    }
    if s.contains('\\') || s.chars().any(char::is_control) {
        return Err(bad("path", "must not contain '\\' or control characters"));
    }
    for comp in s.split('/') {
        match comp {
            "" => {
                return Err(bad(
                    "path",
                    "must not contain empty components ('//' or a trailing '/')",
                ));
            }
            "." | ".." => return Err(bad("path", "must not contain '.' or '..' components")),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn examples() {
        assert!(experiment_name("demo").is_ok());
        assert!(experiment_name("a_b-9").is_ok());
        assert!(experiment_name("Demo").is_err());
        assert!(experiment_name("-x").is_err());
        assert!(experiment_name("demo\n").is_err());
        assert!(experiment_name(&"a".repeat(63)).is_ok());
        assert!(experiment_name(&"a".repeat(64)).is_err());
        assert!(profile_name("local-glm").is_ok());
        assert!(secret_name("HARNESS2_CODEX_API_KEY").is_ok());
        assert!(secret_name("_X").is_err());
        assert!(token_name("ci bot@example.com").is_ok());
        assert!(token_name("").is_err());
        assert!(tag("v1.2").is_ok());
        assert!(tag(".x").is_err());
        assert!(domain("practice area").is_ok());
        assert!(domain("a,b").is_err());
        assert!(domain("-a").is_err());
        assert!(domain(" a").is_err());
        assert!(domain("a\u{7}").is_err());
        assert!(task_slug("contracts__nda-1").is_ok());
        assert!(task_slug("..").is_err());
        assert!(task_slug("a/b").is_err());
        for ok in ["base", "base1", "inv12", "dom1", "agg3", "c1"] {
            assert!(harness_label(ok).is_ok(), "{ok}");
        }
        for no in [
            "inv", "c", "agg1x", "basex", "c-1", "../c1", "Base", "base1\n",
        ] {
            assert!(harness_label(no).is_err(), "{no}");
        }
        assert!(model_id("google-vertex/gemini-3.5-flash").is_ok());
        assert!(model_id("-x").is_err());
        assert!(commit(crate::TESTED_HARNESS2_COMMIT).is_ok());
        assert!(commit("ABCDEF").is_err());
        assert!(env_name("MY_VAR").is_ok());
        assert!(env_name("PATH").is_err());
        assert!(env_name("LD_PRELOAD").is_err());
        assert!(env_name("PYTHONPATH").is_err());
        assert!(env_name("H2S_X").is_err());
        assert!(env_name("lower").is_err());
        assert!(hostname("aiplatform.googleapis.com").is_ok());
        assert!(hostname("*.googleapis.com").is_ok());
        assert!(hostname("litellm").is_ok());
        assert!(hostname("*.").is_err());
        assert!(hostname("a..b").is_err());
        assert!(hostname("-a.com").is_err());
        assert!(hostname("A.com").is_err());
        assert!(base_url("http://127.0.0.1:4000/v1").is_ok());
        assert!(base_url("https://example.com").is_ok());
        assert!(base_url("http://[::1]:8000/v1").is_ok());
        assert!(base_url("ftp://example.com").is_err());
        assert!(base_url("http://user:pw@example.com").is_err());
        assert!(base_url("http://example.com/v1?x=1").is_err());
        assert!(base_url("http://example.com/#f").is_err());
        assert!(base_url("http://:80").is_err());
        assert!(base_url("http://example.com:0").is_err());
        assert!(base_url("http://example.com:99999").is_err());
        assert!(base_url("http://exa mple.com").is_err());
        assert_eq!(
            url_host("http://Gateway.Example:8080/v1"),
            Some(("gateway.example".to_owned(), Some(8080)))
        );
        assert!(relative_path("").is_ok());
        assert!(relative_path("a/b/c.txt").is_ok());
        assert!(relative_path("/etc").is_err());
        assert!(relative_path("a/../b").is_err());
        assert!(relative_path("a//b").is_err());
        assert!(relative_path("a/").is_err());
        assert!(relative_path("./a").is_err());
        assert!(relative_path("a\\b").is_err());
    }

    /// Terminal escape sequences (ESC, the C1 CSI byte) and other control characters can
    /// never reach a file name in an export or a listing request.
    #[test]
    fn paths_refuse_control_characters() {
        for bad in [
            "a\u{1b}[31m.py",
            "skills/x/\u{9b}2J.md",
            "a\nb",
            "a\tb",
            "a\0b",
            "a\u{7f}",
            "\rfile",
        ] {
            let err = relative_path(bad).unwrap_err();
            assert!(err.to_string().contains("control characters"), "{bad:?}");
        }
        assert!(relative_path("skills/résumé/SKILL.md").is_ok());
        assert!(relative_path("a b/c-d_e.f").is_ok());
    }

    proptest! {
        #[test]
        fn accepted_experiment_names_are_path_safe(s in "\\PC{0,70}") {
            if experiment_name(&s).is_ok() {
                prop_assert!(!s.contains('/') && !s.contains("..") && !s.contains('\0'));
                prop_assert!(!s.starts_with('-') && s.len() <= 63);
            }
        }

        #[test]
        fn generated_experiment_names_are_accepted(s in "[a-z0-9][a-z0-9_-]{0,62}") {
            prop_assert!(experiment_name(&s).is_ok());
            prop_assert!(profile_name(&s).is_ok());
        }

        #[test]
        fn accepted_slugs_are_single_components(s in "\\PC{0,40}|[./a-z]{0,6}") {
            if task_slug(&s).is_ok() {
                prop_assert!(!s.contains('/') && !s.contains('\\') && !s.contains('\0'));
                prop_assert!(s != "." && s != "..");
            }
        }

        #[test]
        fn accepted_domains_have_no_list_separator(s in "\\PC{0,30}|[ ,a-z-]{0,8}") {
            if domain(&s).is_ok() {
                prop_assert!(!s.contains(',') && !s.starts_with('-') && !s.is_empty());
            }
        }

        #[test]
        fn accepted_relative_paths_never_escape(s in "[a-z./\\\\]{0,12}") {
            if relative_path(&s).is_ok() {
                prop_assert!(!s.starts_with('/'));
                prop_assert!(s.split('/').all(|c| c != ".." && c != "."));
                prop_assert!(!s.contains('\\'));
            }
        }

        #[test]
        fn accepted_relative_paths_have_no_control_characters(
            s in "\\PC{0,12}|[a-z/\\x00-\\x1F\\x7F-\\x{9F}]{0,12}"
        ) {
            if relative_path(&s).is_ok() {
                prop_assert!(!s.chars().any(char::is_control));
            }
        }

        #[test]
        fn accepted_labels_are_safe(s in "[a-z0-9./]{0,8}") {
            if harness_label(&s).is_ok() {
                prop_assert!(!s.contains('/') && !s.contains('.'));
            }
        }

        #[test]
        fn accepted_base_urls_have_no_credentials(s in "https?://[a-z@:/?#.0-9]{0,20}") {
            if base_url(&s).is_ok() {
                let rest = s.split_once("://").map(|(_, r)| r).unwrap_or("");
                prop_assert!(!rest.contains('@') && !rest.contains('?') && !rest.contains('#'));
            }
        }

        #[test]
        fn accepted_model_ids_cannot_be_options(s in "\\PC{0,20}") {
            if model_id(&s).is_ok() {
                prop_assert!(!s.starts_with('-'));
            }
        }

        #[test]
        fn accepted_env_names_are_never_reserved(s in "[A-Z_][A-Z0-9_]{0,12}") {
            if env_name(&s).is_ok() {
                prop_assert!(!crate::env::RESERVED_ENV.contains(&s.as_str()));
                prop_assert!(crate::env::RESERVED_ENV_PREFIXES.iter().all(|p| !s.starts_with(p)));
            }
        }
    }
}
