//! Validators for every identifier the server accepts from a request or reads from disk.
//!
//! All use whole-string matching (never a `$` anchor, which lets a trailing newline
//! through). Each returns [`CoreError::Invalid`] with a user-facing problem sentence.

use crate::{CoreError, Result};

/// Experiment and split-file names: `^[a-z0-9][a-z0-9_-]{0,62}$`.
pub fn experiment_name(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::experiment_name"))
}

/// Model profile names: same rule as experiment names.
pub fn profile_name(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::profile_name"))
}

/// Secret names: `^[A-Z][A-Z0-9_]{0,63}$`.
pub fn secret_name(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::secret_name"))
}

/// Token display names: 1..=64 chars of letters, digits, space, `._-@`.
pub fn token_name(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::token_name"))
}

/// Tags on experiments: `^[a-z0-9][a-z0-9_.-]{0,31}$`.
pub fn tag(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::tag"))
}

/// A domain name as harness2 reports it (passed to `--only-domains`): 1..=200 chars, no
/// comma (the list separator), no control characters, no leading `-`.
pub fn domain(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::domain"))
}

/// A task directory name (task id with `/` replaced by `__`): 1..=255 bytes, no `/`, `\`,
/// NUL or control characters, not `.` or `..`.
pub fn task_slug(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::task_slug"))
}

/// Harness labels: `^(base[0-9]*|inv[0-9]+|dom[0-9]+|agg[0-9]+|c[0-9]+)$`.
pub fn harness_label(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::harness_label"))
}

/// Model ids as harness2 takes them (`google-vertex/gemini-3.5-flash`, `glm-5`):
/// `^[A-Za-z0-9][A-Za-z0-9._:/@+-]{0,199}$`.
pub fn model_id(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::model_id"))
}

/// A full 40-hex lowercase git commit id.
pub fn commit(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::commit"))
}

/// Environment variable names for profile `extra_env`: `^[A-Z_][A-Z0-9_]{0,127}$` and not
/// in [`crate::env::RESERVED_ENV`] nor starting with a reserved prefix.
pub fn env_name(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::env_name"))
}

/// A DNS hostname for egress allowlists (lowercase LDH labels, optional leading `*.`).
pub fn hostname(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::hostname"))
}

/// An `http`/`https` base URL with a host and no credentials, query or fragment.
pub fn base_url(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::base_url"))
}

/// A relative path inside a browsed directory (`a/b/c.txt`): no absolute paths, no `..`,
/// no backslash, no NUL, at most 1024 bytes. Empty string = the root.
pub fn relative_path(s: &str) -> Result<()> {
    let _ = s;
    Err(CoreError::Unimplemented("names::relative_path"))
}
