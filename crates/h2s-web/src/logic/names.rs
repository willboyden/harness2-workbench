//! Client-side mirrors of the server's identifier rules (`h2s_core::names`), so forms can
//! explain a problem before submitting. The server validates again and has the final word.
//!
//! Each validator returns `Err` with a sentence the form shows next to the field.

/// Environment variable names a profile's `extra_env` may never set: an exact mirror of
/// `h2s_core::env::RESERVED_ENV` (a native test compares the two).
pub(crate) const RESERVED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "SHELL",
    "IFS",
    "ENV",
    "BASH_ENV",
    "SHELLOPTS",
    "BASHOPTS",
    "PS4",
    "GCONV_PATH",
    "GLIBC_TUNABLES",
    "NODE_OPTIONS",
    "NODE_PATH",
    "NODE_TLS_REJECT_UNAUTHORIZED",
    "JAVA_TOOL_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "_JAVA_OPTIONS",
    "RUBYOPT",
    "RUBYLIB",
    "PERL5OPT",
    "PERL5LIB",
    "PERLLIB",
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_PERMISSION",
    "HARNESS2_RUNS_DIR",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "DOCKER_HOST",
    "SSH_AUTH_SOCK",
    "CONTAINER_HOST",
    "REGISTRY_AUTH_FILE",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "CURL_CA_BUNDLE",
];

/// Prefixes a profile's `extra_env` may never use: an exact mirror of
/// `h2s_core::env::RESERVED_ENV_PREFIXES`.
pub(crate) const RESERVED_ENV_PREFIXES: &[&str] = &[
    "LD_",
    "DYLD_",
    "PYTHON",
    "H2S_",
    "UV_",
    "BUN_",
    "NPM_CONFIG_",
    "DOCKER_",
    "CONTAINERS_",
    "XDG_",
    "GIT_",
    "OPENCODE_",
];

type Check = Result<(), String>;

fn lower_slug(s: &str, max_rest: usize, extra: &[char], what: &str) -> Check {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return Err(format!("Enter a {what}."));
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(format!(
            "A {what} must start with a lowercase letter or a digit."
        ));
    }
    let rest: Vec<char> = chars.collect();
    if rest.len() > max_rest {
        return Err(format!(
            "A {what} can be at most {} characters long.",
            max_rest + 1
        ));
    }
    if let Some(bad) = rest
        .iter()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || extra.contains(c)))
    {
        let allowed: String = extra.iter().map(|c| format!(" {c}")).collect();
        return Err(format!(
            "“{bad}” is not allowed in a {what}: use lowercase letters, digits and{allowed}."
        ));
    }
    Ok(())
}

/// Experiment and split-file names: `^[a-z0-9][a-z0-9_-]{0,62}$` (also the directory name).
pub(crate) fn experiment_name(s: &str) -> Check {
    lower_slug(s, 62, &['_', '-'], "name")
}

/// Model profile names: the same rule as experiment names.
pub(crate) fn profile_name(s: &str) -> Check {
    lower_slug(s, 62, &['_', '-'], "profile name")
}

/// Experiment tags: `^[a-z0-9][a-z0-9_.-]{0,31}$`.
pub(crate) fn tag(s: &str) -> Check {
    lower_slug(s, 31, &['_', '.', '-'], "tag")
}

/// Secret names: `^[A-Z][A-Z0-9_]{0,63}$`.
pub(crate) fn secret_name(s: &str) -> Check {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return Err("Enter a secret name.".to_owned());
    };
    if !first.is_ascii_uppercase() {
        return Err("A secret name starts with an uppercase letter (A–Z).".to_owned());
    }
    if s.chars().count() > 64 {
        return Err("A secret name can be at most 64 characters long.".to_owned());
    }
    if chars.any(|c| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')) {
        return Err(
            "Use only uppercase letters, digits and underscores, like OPENAI_API_KEY.".to_owned(),
        );
    }
    Ok(())
}

/// Token display names: 1..=64 characters of letters, digits, space and `._-@`.
pub(crate) fn token_name(s: &str) -> Check {
    let n = s.chars().count();
    if n == 0 || s.trim().is_empty() {
        return Err("Give the token a name, such as “ci-runner” or “alice laptop”.".to_owned());
    }
    if n > 64 {
        return Err("A token name can be at most 64 characters long.".to_owned());
    }
    if let Some(bad) = s
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-' | '@')))
    {
        return Err(format!(
            "“{bad}” is not allowed: use letters, digits, spaces and . _ - @."
        ));
    }
    Ok(())
}

/// Model ids as harness2 takes them: `^[A-Za-z0-9][A-Za-z0-9._:/@+-]{0,199}$`.
pub(crate) fn model_id(s: &str) -> Check {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return Err("Enter a model id.".to_owned());
    };
    if !first.is_ascii_alphanumeric() {
        return Err("A model id starts with a letter or a digit.".to_owned());
    }
    if s.chars().count() > 200 {
        return Err("A model id can be at most 200 characters long.".to_owned());
    }
    if let Some(bad) = chars.find(|c| {
        !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '@' | '+' | '-'))
    }) {
        return Err(format!(
            "“{bad}” is not allowed in a model id (use letters, digits and . _ : / @ + -)."
        ));
    }
    Ok(())
}

/// A full 40-character lowercase hexadecimal git commit id.
pub(crate) fn commit(s: &str) -> Check {
    if s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err("Enter the full 40-character lowercase commit id.".to_owned())
    }
}

/// Environment variable names for a profile's extra environment.
pub(crate) fn env_name(s: &str) -> Check {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return Err("Enter a variable name.".to_owned());
    };
    if !(first.is_ascii_uppercase() || first == '_') {
        return Err("A variable name starts with an uppercase letter or “_”.".to_owned());
    }
    if s.chars().count() > 128 {
        return Err("A variable name can be at most 128 characters long.".to_owned());
    }
    if chars.any(|c| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')) {
        return Err("Use only uppercase letters, digits and underscores.".to_owned());
    }
    if RESERVED_ENV.contains(&s) {
        return Err(format!(
            "{s} is managed by the server and cannot be set by a profile."
        ));
    }
    if let Some(p) = RESERVED_ENV_PREFIXES.iter().find(|p| s.starts_with(*p)) {
        return Err(format!(
            "Variables starting with {p} control the runtime and cannot be set by a profile."
        ));
    }
    Ok(())
}

/// A DNS hostname for egress allowlists: lowercase LDH labels, optional leading `*.`.
pub(crate) fn hostname(s: &str) -> Check {
    let host = s.strip_prefix("*.").unwrap_or(s);
    let problem = || {
        Err(format!(
            "“{s}” is not a valid host name (lowercase letters, digits, hyphens and dots; \
             an optional leading “*.” for subdomains)."
        ))
    };
    if host.is_empty() || host.len() > 253 {
        return problem();
    }
    for label in host.split('.') {
        let ok = !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !ok {
            return problem();
        }
    }
    Ok(())
}

/// An `http`/`https` base URL with a host and no credentials, query or fragment.
pub(crate) fn base_url(s: &str) -> Check {
    let rest = if let Some(r) = s.strip_prefix("https://") {
        r
    } else if let Some(r) = s.strip_prefix("http://") {
        r
    } else {
        return Err("Start the URL with http:// or https://.".to_owned());
    };
    if s.contains('?') || s.contains('#') {
        return Err("A base URL cannot have a query (?) or fragment (#).".to_owned());
    }
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.contains('@') {
        return Err("Do not put credentials in the URL; reference a secret instead.".to_owned());
    }
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        stripped.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    if host.is_empty() {
        return Err("The URL needs a host, e.g. http://127.0.0.1:4000/v1.".to_owned());
    }
    if s.chars().any(char::is_whitespace) {
        return Err("A URL cannot contain spaces.".to_owned());
    }
    Ok(())
}

/// Split a free-text tag list (`a, b c`) into tags, reporting the first invalid one.
pub(crate) fn parse_tags(s: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for t in s.split([',', ' ']).map(str::trim).filter(|t| !t.is_empty()) {
        tag(t).map_err(|e| format!("Tag “{t}”: {e}"))?;
        if !out.iter().any(|x| x == t) {
            out.push(t.to_owned());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experiment_names() {
        for ok in ["demo", "lab-pilot", "a", "x_1-2", &"a".repeat(63)] {
            assert!(experiment_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "-x",
            "_x",
            "Lab",
            "lab pilot",
            "lab/x",
            "é",
            &"a".repeat(64),
        ] {
            assert!(experiment_name(bad).is_err(), "{bad}");
        }
        assert!(profile_name("vertex-flash").is_ok());
        assert!(profile_name("Vertex").is_err());
    }

    #[test]
    fn tags_and_secrets() {
        assert!(tag("v1.2").is_ok());
        assert!(tag(&"a".repeat(33)).is_err());
        assert!(tag(".x").is_err());
        assert_eq!(parse_tags("pilot, k1  pilot").unwrap(), vec!["pilot", "k1"]);
        assert!(parse_tags("ok, Bad").is_err());
        assert!(secret_name("OPENAI_API_KEY").is_ok());
        assert!(secret_name("A").is_ok());
        for bad in [
            "",
            "openai",
            "_X",
            "1X",
            "A-B",
            &format!("A{}", "B".repeat(64)),
        ] {
            assert!(secret_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn token_names() {
        assert!(token_name("alice laptop").is_ok());
        assert!(token_name("ci.runner_1@host-a").is_ok());
        assert!(token_name("").is_err());
        assert!(token_name("   ").is_err());
        assert!(token_name("a/b").is_err());
        assert!(token_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn model_ids_and_commits() {
        for ok in [
            "glm-5",
            "google-vertex/gemini-3.5-flash",
            "org/model@v1+q:8",
        ] {
            assert!(model_id(ok).is_ok(), "{ok}");
        }
        for bad in ["", "/x", "a b", "x;y"] {
            assert!(model_id(bad).is_err(), "{bad}");
        }
        assert!(commit(&"a".repeat(40)).is_ok());
        assert!(commit(&"A".repeat(40)).is_err());
        assert!(commit("1cfe254").is_err());
    }

    #[test]
    fn env_names() {
        assert!(env_name("HARNESS2_JUDGE_MODEL").is_ok());
        assert!(env_name("_PRIVATE").is_ok());
        for bad in [
            "",
            "lower",
            "1X",
            "A-B",
            "PATH",
            "LD_PRELOAD",
            "PYTHONPATH",
            "H2S_X",
            "GIT_SSH_COMMAND",
            "XDG_RUNTIME_DIR",
            "DOCKER_CONFIG",
            "CONTAINERS_CONF",
            "OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX",
            "CONTAINER_HOST",
            "REGISTRY_AUTH_FILE",
            "NODE_TLS_REJECT_UNAUTHORIZED",
            "GLIBC_TUNABLES",
            "PS4",
            "CURL_CA_BUNDLE",
        ] {
            assert!(env_name(bad).is_err(), "{bad}");
        }
    }

    /// The mirrors must equal the server's lists exactly (native test; h2s-core is a
    /// dev-dependency only, never part of the wasm build).
    #[test]
    fn reserved_env_matches_the_server() {
        assert_eq!(RESERVED_ENV, h2s_core::env::RESERVED_ENV);
        assert_eq!(RESERVED_ENV_PREFIXES, h2s_core::env::RESERVED_ENV_PREFIXES);
        // And the client accepts exactly what the server accepts.
        let mut names: Vec<String> = h2s_core::env::RESERVED_ENV
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        names.extend(
            h2s_core::env::RESERVED_ENV_PREFIXES
                .iter()
                .map(|p| format!("{p}X")),
        );
        names.extend(
            [
                "HARNESS2_JUDGE_MODEL",
                "_PRIVATE",
                "OPENAI_API_KEY",
                "GITHUB_TOKEN",
                "XDG",
                "lower",
                "1X",
                "",
                "PATH2",
            ]
            .map(str::to_owned),
        );
        for n in &names {
            assert_eq!(
                env_name(n).is_ok(),
                h2s_core::names::env_name(n).is_ok(),
                "{n:?}"
            );
        }
    }

    #[test]
    fn hosts_and_urls() {
        for ok in [
            "aiplatform.googleapis.com",
            "*.googleapis.com",
            "litellm",
            "a-1.b",
        ] {
            assert!(hostname(ok).is_ok(), "{ok}");
        }
        for bad in ["", "UPPER.com", "-a.com", "a..b", "a_b.com", "*.", "a b"] {
            assert!(hostname(bad).is_err(), "{bad}");
        }
        for ok in [
            "http://127.0.0.1:4000/v1",
            "https://gateway.example.com",
            "http://[::1]:8000/v1",
        ] {
            assert!(base_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "ftp://x",
            "127.0.0.1:4000",
            "http://",
            "http://u:p@host/v1",
            "http://host/v1?key=1",
            "http://host/#x",
            "http://ho st/",
        ] {
            assert!(base_url(bad).is_err(), "{bad}");
        }
    }
}
