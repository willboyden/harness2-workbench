//! API tokens: parsing, loading for stdio mode, and a type that never prints its value.
//!
//! The MCP server never accepts a token on the command line (it would show up in `ps` and
//! shell history). stdio mode reads `H2S_TOKEN` or a token file that must not be readable by
//! group or others; HTTP mode forwards each caller's own bearer token instead.

use std::{fmt, io::Read, path::Path, sync::Arc};

use h2s_api::TOKEN_PREFIX;

/// Longest token accepted. Real tokens are far shorter; the cap bounds what we hold and send.
pub const TOKEN_MAX_LEN: usize = 512;

/// Largest token file read (one token plus a trailing newline fits easily).
const TOKEN_FILE_MAX_BYTES: u64 = 4096;

/// A Harness² Studio API token (`h2s_<id>_<secret>`).
///
/// `Debug` is redacted and there is no `Display`, so a token cannot end up in a log line or
/// an error message by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(Arc<str>);

impl Token {
    /// Validate the shape of a token: the `h2s_` prefix, URL-safe characters only, bounded
    /// length. Surrounding whitespace (a trailing newline in a file) is ignored. This does not
    /// prove the token is valid: only the Studio server can say that.
    ///
    /// # Errors
    /// [`TokenError::Empty`] or [`TokenError::Malformed`].
    pub fn parse(raw: &str) -> Result<Self, TokenError> {
        let token = raw.trim();
        if token.is_empty() {
            return Err(TokenError::Empty);
        }
        let well_formed = token.len() > TOKEN_PREFIX.len()
            && token.len() <= TOKEN_MAX_LEN
            && token.starts_with(TOKEN_PREFIX)
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if well_formed {
            Ok(Self(Arc::from(token)))
        } else {
            Err(TokenError::Malformed)
        }
    }

    /// Read a token from a file that only its owner can access.
    ///
    /// On Unix the file must be a regular file with no permission bits for group or others
    /// (`chmod 600`); the check uses the metadata of the opened file, so it cannot be raced by
    /// swapping the path between the check and the read.
    ///
    /// # Errors
    /// The file cannot be read, is not a regular file, is too large, is group/world
    /// accessible, or does not hold a well-formed token.
    pub fn from_file(path: &Path) -> Result<Self, TokenError> {
        let shown = path.display().to_string();
        let read_err = |source| TokenError::Read {
            path: shown.clone(),
            source,
        };
        let file = std::fs::File::open(path).map_err(read_err)?;
        let meta = file.metadata().map_err(read_err)?;
        if !meta.is_file() {
            return Err(TokenError::NotAFile { path: shown });
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(TokenError::Permissions { path: shown, mode });
            }
        }
        if meta.len() > TOKEN_FILE_MAX_BYTES {
            return Err(TokenError::TooLarge { path: shown });
        }
        let mut raw = String::new();
        file.take(TOKEN_FILE_MAX_BYTES + 1)
            .read_to_string(&mut raw)
            .map_err(read_err)?;
        Self::parse(&raw)
    }

    /// Resolve the stdio-mode credential: exactly one of `H2S_TOKEN` (passed in as `env_token`)
    /// or a token file.
    ///
    /// # Errors
    /// Neither or both are set, or the chosen source is invalid.
    pub fn resolve(env_token: Option<&str>, token_file: Option<&Path>) -> Result<Self, TokenError> {
        let env_token = env_token.filter(|t| !t.trim().is_empty());
        match (env_token, token_file) {
            (Some(_), Some(_)) => Err(TokenError::Ambiguous),
            (Some(raw), None) => Self::parse(raw),
            (None, Some(path)) => Self::from_file(path),
            (None, None) => Err(TokenError::Missing),
        }
    }

    /// The token value, for the `Authorization` header only.
    pub(crate) fn secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// Why a token could not be used. Messages name files and environment variables, never the
/// token itself.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("the token is empty")]
    Empty,
    #[error(
        "the token is not a Harness² Studio API token (expected `h2s_<id>_<secret>`, letters, \
         digits, '_' and '-' only)"
    )]
    Malformed,
    #[error("cannot read the token file {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the token file {path} is not a regular file")]
    NotAFile { path: String },
    #[error(
        "the token file {path} is accessible by group or others (mode {mode:03o}); \
         restrict it first: chmod 600 {path}"
    )]
    Permissions { path: String, mode: u32 },
    #[error("the token file {path} is larger than 4 KiB; it should hold a single token")]
    TooLarge { path: String },
    #[error("both H2S_TOKEN and a token file (--token-file / H2S_TOKEN_FILE) are set; keep one")]
    Ambiguous,
    #[error(
        "no API token: set H2S_TOKEN, or point --token-file / H2S_TOKEN_FILE at a file \
         readable only by you (create a token with `h2studio token create`)"
    )]
    Missing,
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const GOOD: &str = "h2s_alice_s3cret-value";

    #[test]
    fn parses_and_trims() {
        let t = Token::parse(&format!("  {GOOD}\n")).unwrap();
        assert_eq!(t.secret(), GOOD);
    }

    #[test]
    fn rejects_malformed_tokens() {
        for bad in [
            "",
            "   ",
            "h2s_",
            "token",
            "Bearer h2s_x",
            "h2s_a b",
            "h2s_é",
        ] {
            assert!(Token::parse(bad).is_err(), "{bad:?} accepted");
        }
        let long = format!("h2s_{}", "a".repeat(TOKEN_MAX_LEN));
        assert!(matches!(Token::parse(&long), Err(TokenError::Malformed)));
    }

    #[test]
    fn debug_is_redacted() {
        let t = Token::parse(GOOD).unwrap();
        let shown = format!("{t:?}");
        assert!(!shown.contains("s3cret"), "{shown}");
    }

    #[test]
    fn resolve_needs_exactly_one_source() {
        assert!(matches!(
            Token::resolve(None, None),
            Err(TokenError::Missing)
        ));
        assert!(matches!(
            Token::resolve(Some(GOOD), Some(Path::new("x"))),
            Err(TokenError::Ambiguous)
        ));
        assert!(Token::resolve(Some(GOOD), None).is_ok());
        // An empty variable counts as unset.
        assert!(matches!(
            Token::resolve(Some(""), None),
            Err(TokenError::Missing)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn token_file_must_be_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.token");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "{GOOD}").unwrap();
        drop(f);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = Token::from_file(&path).unwrap_err();
        assert!(
            matches!(err, TokenError::Permissions { mode: 0o644, .. }),
            "{err}"
        );
        assert!(err.to_string().contains("chmod 600"));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(Token::from_file(&path).unwrap().secret(), GOOD);
    }

    #[test]
    fn token_file_errors_name_the_file_not_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let err = Token::from_file(&dir.path().join("missing")).unwrap_err();
        assert!(err.to_string().contains("missing"));
        let err = Token::from_file(dir.path()).unwrap_err();
        assert!(matches!(err, TokenError::NotAFile { .. }), "{err}");
    }
}
