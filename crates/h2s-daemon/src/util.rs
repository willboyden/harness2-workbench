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

/// RFC 3339 UTC with exactly three fractional digits.
const MILLIS_FORMAT: &[time::format_description::BorrowedFormatItem<'static>] = time::macros::format_description!(
    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
);

/// The current time as RFC 3339 UTC with milliseconds, fixed width (log lines).
#[must_use]
pub fn now_rfc3339_millis() -> String {
    OffsetDateTime::now_utc()
        .format(MILLIS_FORMAT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00.000Z".to_owned())
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

/// Make this process non-dumpable (`prctl(PR_SET_DUMPABLE, 0)`): no core dumps, and no
/// other process of the same user (a host-mode job) can read its `/proc/<pid>/environ` or
/// memory, or ptrace it. Children are not affected: `execve` resets the attribute.
///
/// # Errors
/// When the kernel refuses (not expected on Linux).
pub fn make_non_dumpable() -> std::io::Result<()> {
    nix::sys::prctl::set_dumpable(false).map_err(std::io::Error::from)
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

/// Largest result file a job leaves for the server to read (`demo.json`, `split.json`).
pub const MAX_RESULT_FILE: u64 = 16 * 1024 * 1024;

/// Read a regular file a job may have written, at most `max` bytes: the last component is
/// never followed when it is a symlink, a FIFO or device is refused without blocking (it is
/// opened non-blocking and checked before any read), and a larger file is refused.
///
/// # Errors
/// I/O errors (`NotFound` passes through), or `InvalidData` for a symlink, a file that is
/// not regular, or one over `max`.
pub fn read_job_file(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let refused = |why: String| std::io::Error::new(std::io::ErrorKind::InvalidData, why);
    let flags = nix::fcntl::OFlag::O_NOFOLLOW | nix::fcntl::OFlag::O_NONBLOCK;
    let f = match OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits())
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.raw_os_error() == Some(nix::errno::Errno::ELOOP as i32) => {
            return Err(refused("is a symbolic link; it is not followed".to_owned()));
        }
        Err(e) => return Err(e),
    };
    let meta = f.metadata()?;
    if !meta.is_file() {
        return Err(refused("is not a regular file".to_owned()));
    }
    if meta.len() > max {
        return Err(refused(format!("is larger than {max} bytes")));
    }
    let mut buf = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    f.take(max.saturating_add(1)).read_to_end(&mut buf)?;
    if u64::try_from(buf.len()).unwrap_or(u64::MAX) > max {
        return Err(refused(format!("is larger than {max} bytes")));
    }
    Ok(buf)
}

/// `"<name> is a symbolic link; ..."` for [`read_job_file`]'s refusals, `"<name>: <error>"`
/// for other I/O errors.
#[must_use]
pub fn job_file_error(name: &str, e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::InvalidData {
        format!("{name} {e}")
    } else {
        format!("{name}: {e}")
    }
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
        for _ in 0..2000 {
            let s = now_rfc3339_millis();
            assert_eq!(s.len(), 24, "{s}");
            assert!(s.ends_with('Z'), "{s}");
            assert!(parse_rfc3339(&s).is_some(), "{s}");
        }
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
    fn the_process_can_be_made_non_dumpable() {
        // Run in a child process: the attribute is process-wide and would also apply to
        // the other tests of this binary.
        let me = std::env::current_exe().unwrap();
        if std::env::var_os("H2S_TEST_DUMPABLE_CHILD").is_some() {
            make_non_dumpable().unwrap();
            assert!(!nix::sys::prctl::get_dumpable().unwrap());
            return;
        }
        let out = std::process::Command::new(me)
            .args([
                "--exact",
                "util::tests::the_process_can_be_made_non_dumpable",
                "--nocapture",
            ])
            .env("H2S_TEST_DUMPABLE_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(nix::sys::prctl::get_dumpable().unwrap());
    }

    #[test]
    fn job_files_are_read_safely() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("demo.json");
        std::fs::write(&file, b"{}").unwrap();
        assert_eq!(read_job_file(&file, 16).unwrap(), b"{}");
        // Over the cap.
        std::fs::write(&file, vec![b'x'; 17]).unwrap();
        let e = read_job_file(&file, 16).unwrap_err();
        assert!(e.to_string().contains("larger than 16"), "{e}");
        // A symlink is not followed, whatever it points at.
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        let e = read_job_file(&link, 1024).unwrap_err();
        assert!(e.to_string().contains("symbolic link"), "{e}");
        // A FIFO is refused at once instead of blocking the reader forever.
        let fifo = dir.path().join("fifo.json");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).unwrap();
        let e = read_job_file(&fifo, 1024).unwrap_err();
        assert!(e.to_string().contains("not a regular file"), "{e}");
        // A directory, and a missing file.
        assert!(read_job_file(dir.path(), 1024).is_err());
        let missing = read_job_file(&dir.path().join("none"), 1024).unwrap_err();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
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
