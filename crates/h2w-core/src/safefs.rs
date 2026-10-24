//! Symlink-proof, size-capped access to a `cap_std` directory.
//!
//! `cap_std` already guarantees nothing escapes the root. On top of that, results browsing must
//! never *follow* a symlink at all (an executor can plant one in its run directory), so every
//! path is walked one component at a time: `lstat` the entry, refuse symlinks, open it, and
//! check the opened handle is the same inode that was `lstat`ed (a swap in between is refused).

use std::io::Read;

use cap_std::fs::{Dir, File, Metadata, MetadataExt};

use crate::{CoreError, Result};

/// Most entries [`list`] returns for one directory; more is [`CoreError::TooLarge`], so a
/// directory listing never grows without bound.
pub(crate) const MAX_DIR_ENTRIES: usize = 250_000;

/// Kind of a directory entry, never following symlinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// One directory entry (name is valid UTF-8; others are skipped by [`list`]).
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) kind: EntryKind,
    pub(crate) len: u64,
    pub(crate) mtime: Option<i64>,
}

fn kind_of(meta: &Metadata) -> EntryKind {
    let ft = meta.file_type();
    if ft.is_symlink() {
        EntryKind::Symlink
    } else if ft.is_dir() {
        EntryKind::Dir
    } else if ft.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

fn same_inode(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}

fn component_ok(c: &str) -> bool {
    !c.is_empty() && c != "." && c != ".." && !c.contains(['\0', '\\'])
}

fn not_found(what: &str) -> CoreError {
    CoreError::NotFound(what.to_owned())
}

fn map_io(e: std::io::Error, what: &str) -> CoreError {
    if e.kind() == std::io::ErrorKind::NotFound {
        not_found(what)
    } else {
        CoreError::Io(e)
    }
}

/// Open one directory component below `dir` without following a symlink.
fn open_child_dir(dir: &Dir, name: &str, what: &str) -> Result<Dir> {
    if !component_ok(name) {
        return Err(CoreError::Refused(format!(
            "invalid path component in {what}"
        )));
    }
    let meta = dir.symlink_metadata(name).map_err(|e| map_io(e, what))?;
    match kind_of(&meta) {
        EntryKind::Dir => {}
        EntryKind::Symlink => {
            return Err(CoreError::Refused(format!(
                "{what} goes through a symbolic link, which is never followed"
            )));
        }
        EntryKind::File | EntryKind::Other => return Err(not_found(what)),
    }
    let child = dir.open_dir(name).map_err(|e| map_io(e, what))?;
    let opened = child.dir_metadata()?;
    if !same_inode(&meta, &opened) {
        return Err(CoreError::Refused(format!(
            "{what} changed while it was opened"
        )));
    }
    Ok(child)
}

/// Open the directory at `rel` (`/`-separated, empty = `dir` itself) without following
/// symlinks. `what` names the thing for error messages (never an absolute path).
pub(crate) fn open_dir(dir: &Dir, rel: &str, what: &str) -> Result<Dir> {
    let mut cur = dir.try_clone()?;
    for comp in rel.split('/').filter(|c| !c.is_empty()) {
        cur = open_child_dir(&cur, comp, what)?;
    }
    Ok(cur)
}

/// Like [`open_dir`] but `Ok(None)` when any component does not exist.
pub(crate) fn open_dir_opt(dir: &Dir, rel: &str, what: &str) -> Result<Option<Dir>> {
    match open_dir(dir, rel, what) {
        Ok(d) => Ok(Some(d)),
        Err(CoreError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

fn split_parent(rel: &str) -> (&str, &str) {
    match rel.rfind('/') {
        Some(i) => (rel.get(..i).unwrap_or(""), rel.get(i + 1..).unwrap_or("")),
        None => ("", rel),
    }
}

/// Open the regular file at `rel` without following symlinks.
pub(crate) fn open_file(dir: &Dir, rel: &str, what: &str) -> Result<(File, u64)> {
    let (parent, name) = split_parent(rel);
    let parent = open_dir(dir, parent, what)?;
    if !component_ok(name) {
        return Err(CoreError::Refused(format!(
            "invalid path component in {what}"
        )));
    }
    let meta = parent.symlink_metadata(name).map_err(|e| map_io(e, what))?;
    match kind_of(&meta) {
        EntryKind::File => {}
        EntryKind::Symlink => {
            return Err(CoreError::Refused(format!(
                "{what} is a symbolic link, which is never followed"
            )));
        }
        EntryKind::Dir | EntryKind::Other => {
            return Err(CoreError::Refused(format!("{what} is not a regular file")));
        }
    }
    let file = parent.open(name).map_err(|e| map_io(e, what))?;
    let opened = file.metadata()?;
    if !same_inode(&meta, &opened) {
        return Err(CoreError::Refused(format!(
            "{what} changed while it was opened"
        )));
    }
    Ok((file, opened.len()))
}

/// Read at most `cap` bytes of a regular file: `(bytes, total size, truncated)`.
pub(crate) fn read_capped(
    dir: &Dir,
    rel: &str,
    cap: u64,
    what: &str,
) -> Result<(Vec<u8>, u64, bool)> {
    let (file, len) = open_file(dir, rel, what)?;
    let mut buf = Vec::with_capacity(usize::try_from(len.min(cap)).unwrap_or(0));
    // One byte past the cap tells a full read from a truncated one even if the file grew.
    file.take(cap.saturating_add(1)).read_to_end(&mut buf)?;
    let got = u64::try_from(buf.len()).unwrap_or(u64::MAX);
    let truncated = got > cap;
    if truncated {
        buf.truncate(usize::try_from(cap).unwrap_or(usize::MAX));
    }
    Ok((buf, len.max(got.min(cap)), truncated))
}

fn over_cap(what: &str, len: u64, cap: u64) -> CoreError {
    CoreError::TooLarge(format!("{what} is {len} bytes, over the {cap}-byte limit"))
}

/// Read a whole regular file, refusing it ([`CoreError::TooLarge`], before reading) when
/// larger than `cap`. A file that grows past `cap` while it is read is refused too.
pub(crate) fn read_whole(dir: &Dir, rel: &str, cap: u64, what: &str) -> Result<Vec<u8>> {
    let (file, len) = open_file(dir, rel, what)?;
    if len > cap {
        return Err(over_cap(what, len, cap));
    }
    let mut buf = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(cap.saturating_add(1)).read_to_end(&mut buf)?;
    let got = u64::try_from(buf.len()).unwrap_or(u64::MAX);
    if got > cap {
        return Err(over_cap(what, got, cap));
    }
    Ok(buf)
}

/// [`read_whole`], `Ok(None)` when the file (or a parent) does not exist.
pub(crate) fn read_whole_opt(
    dir: &Dir,
    rel: &str,
    cap: u64,
    what: &str,
) -> Result<Option<Vec<u8>>> {
    match read_whole(dir, rel, cap, what) {
        Ok(b) => Ok(Some(b)),
        Err(CoreError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Read a whole file as UTF-8 text with invalid sequences replaced (Python's
/// `errors="replace"`).
pub(crate) fn read_text(dir: &Dir, rel: &str, cap: u64, what: &str) -> Result<String> {
    let bytes = read_whole(dir, rel, cap, what)?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

/// `Ok(None)` when the file (or a parent) does not exist.
pub(crate) fn read_text_opt(dir: &Dir, rel: &str, cap: u64, what: &str) -> Result<Option<String>> {
    match read_text(dir, rel, cap, what) {
        Ok(s) => Ok(Some(s)),
        Err(CoreError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Parse a JSON file of at most `cap` bytes, `Ok(None)` when missing.
pub(crate) fn read_json_opt(
    dir: &Dir,
    rel: &str,
    cap: u64,
    what: &str,
) -> Result<Option<serde_json::Value>> {
    let Some(bytes) = read_whole_opt(dir, rel, cap, what)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| CoreError::Malformed {
            what: what.to_owned(),
            detail: e.to_string(),
        })
}

/// Every entry of `dir` with a UTF-8 name, sorted by name (byte order). Never follows
/// symlinks: a link is reported as [`EntryKind::Symlink`]. More than [`MAX_DIR_ENTRIES`]
/// entries is [`CoreError::TooLarge`].
pub(crate) fn list(dir: &Dir) -> Result<Vec<Entry>> {
    list_capped(dir, MAX_DIR_ENTRIES)
}

/// [`list`] with a smaller entry cap: more than `max` entries is [`CoreError::TooLarge`].
pub(crate) fn list_capped(dir: &Dir, max: usize) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for entry in dir.entries()? {
        if out.len() >= max {
            return Err(CoreError::TooLarge(format!(
                "a directory has more than {max} entries"
            )));
        }
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        out.push(Entry {
            name,
            kind: kind_of(&meta),
            len: meta.len(),
            mtime: Some(meta.mtime()),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Like [`list`] for a sub-directory; empty when it does not exist.
pub(crate) fn list_opt(dir: &Dir, rel: &str, what: &str) -> Result<Vec<Entry>> {
    match open_dir_opt(dir, rel, what)? {
        Some(d) => list(&d),
        None => Ok(Vec::new()),
    }
}

/// `(relative path, size)` of regular files.
pub(crate) type FileList = Vec<(String, u64)>;

/// Regular files below `dir`, recursively, as `/`-joined relative paths with their size.
/// Symlinks are skipped (reported in `symlinks`), directories named in `skip_dirs` are not
/// entered. Iterative, so deep trees cannot overflow the stack. `max_entries` bounds the walk.
pub(crate) fn walk_files(
    dir: &Dir,
    skip_dirs: &[&str],
    max_entries: usize,
) -> Result<(FileList, Vec<String>)> {
    let mut files = Vec::new();
    let mut symlinks = Vec::new();
    walk(dir, skip_dirs, max_entries, |rel, e| match e.kind {
        EntryKind::File => files.push((rel, e.len)),
        EntryKind::Symlink => symlinks.push(rel),
        EntryKind::Dir | EntryKind::Other => {}
    })?;
    Ok((files, symlinks))
}

/// Visit every entry below `dir` (depth first, children of a directory in name order).
/// Directories are reopened from the root when visited, so only one handle per level is
/// held at a time no matter how wide the tree is.
fn walk(
    dir: &Dir,
    skip_dirs: &[&str],
    max_entries: usize,
    mut visit: impl FnMut(String, &Entry),
) -> Result<()> {
    let mut stack: Vec<String> = vec![String::new()];
    let mut seen = 0_usize;
    while let Some(prefix) = stack.pop() {
        let d = open_dir(dir, &prefix, "directory")?;
        let mut subdirs = Vec::new();
        for e in list(&d)? {
            seen += 1;
            if seen > max_entries {
                return Err(CoreError::TooLarge(format!(
                    "a directory tree has more than {max_entries} entries"
                )));
            }
            let rel = if prefix.is_empty() {
                e.name.clone()
            } else {
                format!("{prefix}/{}", e.name)
            };
            if e.kind == EntryKind::Dir && !skip_dirs.contains(&e.name.as_str()) {
                subdirs.push(rel.clone());
            }
            visit(rel, &e);
        }
        stack.extend(subdirs.into_iter().rev());
    }
    Ok(())
}

/// Total size of the regular files below `dir` (symlinks not followed).
pub(crate) fn total_size(dir: &Dir, max_entries: usize) -> Result<u64> {
    let mut total = 0_u64;
    walk(dir, &[], max_entries, |_, e| {
        if e.kind == EntryKind::File {
            total = total.saturating_add(e.len);
        }
    })?;
    Ok(total)
}

/// `mtime` (seconds since the epoch) of a regular file, without following symlinks.
pub(crate) fn mtime(dir: &Dir, rel: &str, what: &str) -> Result<Option<i64>> {
    let (parent, name) = split_parent(rel);
    let Some(parent) = open_dir_opt(dir, parent, what)? else {
        return Ok(None);
    };
    match parent.symlink_metadata(name) {
        Ok(m) if kind_of(&m) == EntryKind::File => Ok(Some(m.mtime())),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CoreError::Io(e)),
    }
}

/// Whether `rel` names an existing directory (no symlinks anywhere on the way).
pub(crate) fn is_dir(dir: &Dir, rel: &str) -> bool {
    open_dir(dir, rel, "directory").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> (tempfile::TempDir, Dir) {
        let t = tempfile::tempdir().unwrap();
        let d = Dir::open_ambient_dir(t.path(), cap_std::ambient_authority()).unwrap();
        (t, d)
    }

    #[test]
    fn refuses_symlinks_and_caps_reads() {
        let (t, d) = root();
        std::fs::create_dir_all(t.path().join("a/b")).unwrap();
        std::fs::write(t.path().join("a/b/f.txt"), "hello world").unwrap();
        std::os::unix::fs::symlink(t.path().join("a"), t.path().join("link")).unwrap();
        std::os::unix::fs::symlink("b/f.txt", t.path().join("a/flink")).unwrap();
        let (bytes, len, truncated) = read_capped(&d, "a/b/f.txt", 5, "f").unwrap();
        assert_eq!(
            (bytes.as_slice(), len, truncated),
            (&b"hello"[..], 11, true)
        );
        let (_, _, truncated) = read_capped(&d, "a/b/f.txt", 64, "f").unwrap();
        assert!(!truncated);
        assert!(matches!(
            read_capped(&d, "link/b/f.txt", 64, "f"),
            Err(CoreError::Refused(_))
        ));
        assert!(matches!(
            read_capped(&d, "a/flink", 64, "f"),
            Err(CoreError::Refused(_))
        ));
        assert!(matches!(
            read_whole(&d, "a/b/f.txt", 3, "f"),
            Err(CoreError::TooLarge(_))
        ));
        assert!(matches!(
            read_whole(&d, "a/missing", 3, "f"),
            Err(CoreError::NotFound(_))
        ));
        assert!(matches!(
            open_dir(&d, "a/../a", "x"),
            Err(CoreError::Refused(_))
        ));
        let entries = list(&d).unwrap();
        let kinds: Vec<_> = entries.iter().map(|e| (e.name.as_str(), e.kind)).collect();
        assert_eq!(
            kinds,
            vec![("a", EntryKind::Dir), ("link", EntryKind::Symlink)]
        );
        let (files, links) = walk_files(&d, &[], 100).unwrap();
        assert_eq!(files, vec![("a/b/f.txt".to_owned(), 11)]);
        assert_eq!(links.len(), 2);
    }

    /// Listings and walks stop with `TooLarge` at their caps instead of growing.
    #[test]
    fn listings_and_walks_are_capped() {
        let (t, d) = root();
        for i in 0..5 {
            std::fs::create_dir_all(t.path().join(format!("d{i}"))).unwrap();
            std::fs::write(t.path().join(format!("d{i}/f")), "x").unwrap();
        }
        assert_eq!(list_capped(&d, 5).unwrap().len(), 5);
        assert!(matches!(list_capped(&d, 4), Err(CoreError::TooLarge(_))));
        // 5 directories + 5 files.
        assert_eq!(walk_files(&d, &[], 10).unwrap().0.len(), 5);
        assert!(matches!(
            walk_files(&d, &[], 9),
            Err(CoreError::TooLarge(_))
        ));
        assert!(matches!(total_size(&d, 9), Err(CoreError::TooLarge(_))));
        // A file over the read cap is refused before it is read; at the cap it reads.
        std::fs::write(t.path().join("five"), "12345").unwrap();
        assert_eq!(read_whole(&d, "five", 5, "f").unwrap(), b"12345");
        let err = read_whole(&d, "five", 4, "f").unwrap_err();
        assert!(matches!(err, CoreError::TooLarge(_)), "{err}");
        assert!(err.to_string().contains("4-byte limit"));
        assert!(read_whole_opt(&d, "nope", 4, "f").unwrap().is_none());
    }
}
