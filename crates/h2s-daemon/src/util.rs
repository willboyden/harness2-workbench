//! Small helpers shared by every module: time, randomness, hashing and private files.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use base64::Engine;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Seconds since the Unix epoch, now.
#[must_use]
pub fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// RFC 3339 UTC with second precision (`2026-10-23T12:00:00Z`). Out-of-range input
/// (never produced by this server) renders as the epoch.
#[must_use]
pub fn rfc3339(unix: i64) -> String {
    let t = OffsetDateTime::from_unix_timestamp(unix).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    t.format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// The current time as RFC 3339 UTC, second precision.
#[must_use]
pub fn now_rfc3339() -> String {
    rfc3339(now_unix())
}

/// The current time as RFC 3339 UTC with milliseconds (log lines).
#[must_use]
pub fn now_rfc3339_millis() -> String {
    let now = OffsetDateTime::now_utc();
    let millis = now.millisecond();
    let t = now.replace_millisecond(millis).unwrap_or(now);
    // `replace_millisecond` zeroes the sub-millisecond part, so RFC 3339 prints 3 digits
    // unless the value is a whole second, in which case we add them for a fixed width.
    let s = t.format(&Rfc3339).unwrap_or_default();
    if s.contains('.') {
        s
    } else {
        s.replacen('Z', ".000Z", 1)
    }
}

/// Parse an RFC 3339 timestamp into Unix seconds.
#[must_use]
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    OffsetDateTime::parse(s, &Rfc3339)
        .ok()
        .map(OffsetDateTime::unix_timestamp)
}

/// Fill `N` bytes from the operating system's CSPRNG.
///
/// # Errors
/// When the OS refuses to provide randomness (no safe fallback exists).
pub fn random_bytes<const N: usize>() -> std::io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|e| std::io::Error::other(format!("getrandom: {e}")))?;
    Ok(buf)
}

/// SHA-256 of `data`.
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let out = Sha256::digest(data);
    let mut arr = [0u8; 32];
    arr.copy_from_slice(out.as_ref());
    arr
}

/// Lowercase hex SHA-256 of `data`.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(sha256(data))
}

/// URL-safe base64 without padding.
#[must_use]
pub fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode URL-safe base64 without padding.
#[must_use]
pub fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .ok()
}

/// Create `dir` (and parents) and set its mode to 0700 explicitly.
///
/// # Errors
/// I/O errors, or when `dir` exists and is not a directory.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        return Err(std::io::Error::other(format!(
            "{} exists and is not a directory",
            dir.display()
        )));
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Write `bytes` to `path` as a new private file (0600, set explicitly on the open file
/// before any data is written). Refuses to follow or replace an existing file.
///
/// # Errors
/// I/O errors, including `AlreadyExists`.
pub fn write_private_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(path)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Write `bytes` to `path`, replacing any previous file atomically (temp file + rename in
/// the same directory). The result is 0600.
///
/// # Errors
/// I/O errors.
pub fn write_private_replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("path has no parent directory"))?;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("path has no file name"))?;
    let suffix = hex::encode(random_bytes::<6>()?);
    let tmp = dir.join(format!(".{}.{suffix}.tmp", name.to_string_lossy()));
    let result = write_private_new(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Open (creating if needed) a private append-only file: mode 0600 set explicitly.
///
/// # Errors
/// I/O errors.
pub fn open_private_append(path: &Path) -> std::io::Result<File> {
    let f = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(path)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(f)
}

/// Set `path`'s mode when it exists (used to tighten files other code created).
///
/// # Errors
/// I/O errors other than `NotFound`.
pub fn chmod_if_exists(path: &Path, mode: u32) -> std::io::Result<()> {
    match std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Truncate `s` to at most `max` bytes on a character boundary, appending `…` when cut.
#[must_use]
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// `true` when `s` is a single, normal path component (no separators, not `.`/`..`).
#[must_use]
pub fn is_single_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_has_second_precision() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_792_756_800), "2026-10-23T12:00:00Z");
        assert_eq!(parse_rfc3339("2026-10-23T12:00:00Z"), Some(1_792_756_800));
    }

    #[test]
    fn millis_timestamps_have_fixed_width() {
        let s = now_rfc3339_millis();
        assert!(s.ends_with('Z'), "{s}");
        let frac = s.rsplit('.').next().unwrap_or_default();
        assert_eq!(frac.len(), 4, "{s}"); // three digits + Z
    }

    #[test]
    fn private_files_are_0600() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x");
        write_private_new(&p, b"hi").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(write_private_new(&p, b"again").is_err());
        write_private_replace(&p, b"new").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let sub = dir.path().join("a/b");
        ensure_private_dir(&sub).unwrap();
        let mode = std::fs::metadata(&sub).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn private_files_refuse_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, b"t").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(open_private_append(&link).is_err());
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("héllo", 2), "h…");
        assert_eq!(truncate_chars("héllo", 3), "hé…");
    }

    #[test]
    fn single_components() {
        assert!(is_single_component("demo"));
        for bad in ["", ".", "..", "a/b", "a\\b", "a\0b"] {
            assert!(!is_single_component(bad), "{bad:?}");
        }
    }

    #[test]
    fn hashing_and_base64() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let s = b64url(&[0xff, 0xfe, 0x00]);
        assert_eq!(b64url_decode(&s).unwrap(), vec![0xff, 0xfe, 0x00]);
        assert!(b64url_decode("!!").is_none());
    }
}
