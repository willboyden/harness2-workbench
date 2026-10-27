//! Offline install bundles: the managed harness2 installation (its checkout, virtual environment
//! and `dependencies/`, benchmark data included) as one gzip-compressed tar with a manifest, so a
//! Workbench can be set up on a host where neither it nor its jobs may reach the internet.
//!
//! Layout, in this order: [`MANIFEST_NAME`] (JSON, a [`BundleManifest`]); optionally
//! [`RUNNER_IMAGE_NAME`] (the runner image as the container runtime's `save` wrote it); then the
//! installation under [`TREE_PREFIX`]`/`. Only regular files, directories and symbolic links are
//! written or accepted.
//!
//! A bundle holds executable code, so importing one is as much trust as running
//! `install_harness2` and `setup` against the same sources. The importer checks the archive's
//! SHA-256 against a value the administrator supplies, and [`scan`] / [`unpack`] refuse every
//! entry that could write outside the target directory or smuggle in something else: absolute or
//! `..` paths, entries outside the three names above, duplicates, a path below a symbolic link,
//! links whose target leaves the tree, hard links, devices and FIFOs. Absolute link targets are
//! accepted only for a virtual environment's interpreter (`.venv/bin/python*`), which points at
//! the Python of the host or image the environment was built for. Setuid, setgid and sticky bits
//! are dropped. This module only reads and writes files; the server runs the container runtime.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{CoreError, Result};

/// Bundle format version this build writes and reads.
pub const FORMAT: u32 = 1;
/// The manifest: always the first entry.
pub const MANIFEST_NAME: &str = "h2w-bundle.json";
/// The runner image (`docker save` / `podman save` output), when included: the second entry.
pub const RUNNER_IMAGE_NAME: &str = "runner-image.tar";
/// Directory the installation is stored under.
pub const TREE_PREFIX: &str = "harness2";
/// Largest manifest accepted.
const MANIFEST_MAX_BYTES: u64 = 1024 * 1024;

/// What a bundle says about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleManifest {
    /// [`FORMAT`] of the writer.
    pub format: u32,
    /// Version of the Workbench that wrote it.
    pub workbench_version: String,
    /// RFC 3339 time it was written.
    pub created_at: String,
    /// harness2 commit of the installation.
    pub harness2_commit: String,
    /// harness2 package version, when known.
    pub harness2_version: Option<String>,
    /// Python version of the installation's virtual environment, when known.
    pub python: Option<String>,
    /// `home` of that environment's `pyvenv.cfg`: the interpreter it needs.
    pub python_home: Option<String>,
    /// CPU architecture of the writer (`std::env::consts::ARCH`); Bun and wheels are native.
    pub arch: String,
    /// Benchmarks present in the installation.
    pub benchmarks: Vec<BundleBenchmark>,
    /// Regular files in the tree.
    pub files: u64,
    /// Bytes of those files.
    pub bytes: u64,
    /// The runner image, when included.
    pub runner_image: Option<BundleImage>,
}

/// One installed benchmark in a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleBenchmark {
    /// `lab`, `jb` or `wb`.
    pub bench: String,
    /// WorkBuddy subset, if any.
    pub subset: Option<String>,
    /// The checkout is at the commit harness2 pins.
    pub at_pinned_commit: bool,
    /// Its task data is present.
    pub data_present: bool,
}

/// The runner image stored in a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleImage {
    /// Reference the configuration names, such as `harness2-workbench-runner:0.1.1`.
    pub reference: String,
    /// Image id the runtime reported (`sha256:...`).
    pub id: String,
    /// Size of the saved image in the bundle.
    pub bytes: u64,
}

/// Counts of a tree, as [`survey`] finds it and [`write`] writes it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeStats {
    pub files: u64,
    pub bytes: u64,
    pub dirs: u64,
    pub symlinks: u64,
    /// Sockets, FIFOs and devices, which are never written.
    pub skipped_special: u64,
}

/// Walk an installation without following links and count what [`write`] would store.
///
/// # Errors
/// I/O errors reading the tree.
pub fn survey(home: &Path) -> Result<TreeStats> {
    let mut stats = TreeStats::default();
    walk(home, Path::new(TREE_PREFIX), &mut |path, _, kind| {
        count(&mut stats, path, kind)
    })?;
    Ok(stats)
}

/// Write a bundle of the installation at `home` to `out`: the manifest, the runner image
/// saved at `runner_image` (if any), then the tree. Returns what was written; the caller
/// compares it with the manifest's counts (a difference means the tree changed meanwhile).
///
/// # Errors
/// I/O errors, or a manifest that cannot be serialised.
pub fn write<W: Write>(
    out: W,
    home: &Path,
    manifest: &BundleManifest,
    runner_image: Option<&Path>,
) -> Result<TreeStats> {
    let gz = flate2::write::GzEncoder::new(out, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.follow_symlinks(false);
    let json = serde_json::to_vec_pretty(manifest)
        .map_err(|e| CoreError::invalid("manifest", e.to_string()))?;
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o644);
    header.set_size(json.len() as u64);
    header.set_mtime(now_secs());
    tar.append_data(&mut header, MANIFEST_NAME, json.as_slice())?;
    if let Some(image) = runner_image {
        tar.append_path_with_name(image, RUNNER_IMAGE_NAME)?;
    }
    let mut stats = TreeStats::default();
    walk(home, Path::new(TREE_PREFIX), &mut |path, name, kind| {
        match kind {
            Kind::Dir => tar.append_dir(name, path)?,
            Kind::File | Kind::Symlink => tar.append_path_with_name(path, name)?,
            Kind::Special => {}
        }
        count(&mut stats, path, kind)
    })?;
    tar.into_inner()?.finish()?;
    Ok(stats)
}

/// The manifest alone: reads only the first entry, so it is cheap on a bundle of gigabytes.
/// Nothing else is checked; [`scan`] and [`unpack`] check everything.
///
/// # Errors
/// [`CoreError::Malformed`] when the first entry is not a readable manifest, I/O errors.
pub fn read_manifest<R: Read>(input: R) -> Result<BundleManifest> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(input));
    let malformed = |detail: String| CoreError::Malformed {
        what: "bundle".into(),
        detail,
    };
    let mut entry = archive
        .entries()?
        .next()
        .ok_or_else(|| malformed("the archive is empty".into()))??;
    let is_manifest = entry.path()?.to_str() == Some(MANIFEST_NAME);
    if !is_manifest || !entry.header().entry_type().is_file() {
        return Err(malformed(format!(
            "the first entry must be {MANIFEST_NAME}"
        )));
    }
    if entry.size() > MANIFEST_MAX_BYTES {
        return Err(CoreError::Refused(format!(
            "{MANIFEST_NAME} is larger than {MANIFEST_MAX_BYTES} bytes"
        )));
    }
    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    serde_json::from_str(&text).map_err(|e| CoreError::Malformed {
        what: MANIFEST_NAME.into(),
        detail: e.to_string(),
    })
}

/// Read a bundle without extracting it: every entry is checked as [`unpack`] checks it, and
/// the manifest's counts must match the tree.
///
/// # Errors
/// [`CoreError::Refused`] for an entry that is not allowed, [`CoreError::Malformed`] for a
/// missing or unreadable manifest or counts that do not match, I/O errors.
pub fn scan<R: Read>(input: R) -> Result<BundleManifest> {
    process(input, None, None)
}

/// Extract a bundle into `dest`, an empty directory: the tree to `dest/harness2`, the runner
/// image (if the bundle has one and `image_out` is given) to `image_out`. Directory modes are
/// applied last, file modes without setuid, setgid and sticky bits. On an error, whatever was
/// written stays in `dest` for the caller to remove.
///
/// # Errors
/// As [`scan`], plus [`CoreError::Refused`] when `dest` is not an empty directory.
pub fn unpack<R: Read>(input: R, dest: &Path, image_out: Option<&Path>) -> Result<BundleManifest> {
    let meta = fs::symlink_metadata(dest)?;
    if !meta.is_dir() || fs::read_dir(dest)?.next().is_some() {
        return Err(CoreError::Refused(format!(
            "{} must be an empty directory",
            dest.display()
        )));
    }
    process(input, Some(dest), image_out)
}

/// Problems that make a bundle unusable for a Workbench tested with `tested_commit`; empty
/// when it can be imported. `allow_untested` mirrors `paths.allow_untested_harness2_rev`.
#[must_use]
pub fn compatibility(
    manifest: &BundleManifest,
    tested_commit: &str,
    allow_untested: bool,
) -> Vec<String> {
    let mut problems = Vec::new();
    if manifest.format != FORMAT {
        problems.push(format!(
            "bundle format {} is not the format this Workbench reads ({FORMAT})",
            manifest.format
        ));
    }
    if manifest.arch != std::env::consts::ARCH {
        problems.push(format!(
            "the bundle was made on {}, this host is {}: Bun and compiled packages are native code",
            manifest.arch,
            std::env::consts::ARCH
        ));
    }
    if manifest.harness2_commit != tested_commit && !allow_untested {
        problems.push(format!(
            "the bundle holds harness2 {}, but this Workbench is tested with {}; an administrator \
             can allow it with paths.allow_untested_harness2_rev = true",
            short(&manifest.harness2_commit),
            short(tested_commit)
        ));
    }
    problems
}

/// SHA-256 of everything `input` yields, as lowercase hex.
///
/// # Errors
/// I/O errors reading `input`.
pub fn sha256_hex<R: Read>(mut input: R) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// The `sha256sum`-format line for a bundle file name: `<hex>  <name>`.
#[must_use]
pub fn sha256_line(hex: &str, file_name: &str) -> String {
    format!("{hex}  {file_name}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dir,
    File,
    Symlink,
    Special,
}

fn count(stats: &mut TreeStats, path: &Path, kind: Kind) -> Result<()> {
    match kind {
        Kind::Dir => stats.dirs += 1,
        Kind::File => {
            stats.files += 1;
            stats.bytes += fs::symlink_metadata(path)?.size();
        }
        Kind::Symlink => stats.symlinks += 1,
        Kind::Special => stats.skipped_special += 1,
    }
    Ok(())
}

/// Depth-first, entries sorted by name, links never followed. `f` gets the path on disk, the
/// name in the archive and the kind.
fn walk(
    path: &Path,
    name: &Path,
    f: &mut dyn FnMut(&Path, &Path, Kind) -> Result<()>,
) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    let ft = meta.file_type();
    let kind = if ft.is_dir() {
        Kind::Dir
    } else if ft.is_file() {
        Kind::File
    } else if ft.is_symlink() {
        Kind::Symlink
    } else {
        Kind::Special
    };
    f(path, name, kind)?;
    if kind == Kind::Dir {
        let mut entries: Vec<_> = fs::read_dir(path)?.collect::<io::Result<_>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for e in entries {
            walk(&e.path(), &name.join(e.file_name()), f)?;
        }
    }
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// Shared by [`scan`] and [`unpack`]: check every entry; with `dest`, also write it.
fn process<R: Read>(
    input: R,
    dest: Option<&Path>,
    image_out: Option<&Path>,
) -> Result<BundleManifest> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(input));
    let mut manifest: Option<BundleManifest> = None;
    let mut seen: HashSet<String> = HashSet::new();
    let mut links: HashSet<String> = HashSet::new();
    let mut dir_modes: Vec<(PathBuf, u32)> = Vec::new();
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut has_image = false;
    for (index, entry) in archive.entries()?.enumerate() {
        let mut entry = entry?;
        let raw = entry.path()?;
        let name = raw
            .to_str()
            .ok_or_else(|| CoreError::Refused("an entry name is not UTF-8".into()))?
            .trim_end_matches('/')
            .to_owned();
        let etype = entry.header().entry_type();
        let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
        if index == 0 {
            if name != MANIFEST_NAME || !etype.is_file() {
                return Err(CoreError::Malformed {
                    what: "bundle".into(),
                    detail: format!("the first entry must be {MANIFEST_NAME}, found {name:?}"),
                });
            }
            if entry.size() > MANIFEST_MAX_BYTES {
                return Err(CoreError::Refused(format!(
                    "{MANIFEST_NAME} is larger than {MANIFEST_MAX_BYTES} bytes"
                )));
            }
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            let m: BundleManifest =
                serde_json::from_str(&text).map_err(|e| CoreError::Malformed {
                    what: MANIFEST_NAME.into(),
                    detail: e.to_string(),
                })?;
            if m.format != FORMAT {
                return Err(CoreError::Refused(format!(
                    "bundle format {} is not the format this Workbench reads ({FORMAT})",
                    m.format
                )));
            }
            manifest = Some(m);
            continue;
        }
        if name == RUNNER_IMAGE_NAME {
            if index != 1 || !etype.is_file() {
                return Err(CoreError::Refused(format!(
                    "{RUNNER_IMAGE_NAME} must be the second entry and a regular file"
                )));
            }
            if manifest.as_ref().is_some_and(|m| m.runner_image.is_none()) {
                return Err(CoreError::Refused(format!(
                    "the bundle carries {RUNNER_IMAGE_NAME} but its manifest names no runner image"
                )));
            }
            has_image = true;
            if let Some(out) = image_out.filter(|_| dest.is_some()) {
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(out)?;
                io::copy(&mut entry, &mut file)?;
            }
            continue;
        }
        check_name(&name)?;
        if !seen.insert(name.clone()) {
            return Err(CoreError::Refused(format!("{name} appears twice")));
        }
        if let Some(parent) = ancestors(&name).find(|a| links.contains(*a)) {
            return Err(CoreError::Refused(format!(
                "{name} lies below the symbolic link {parent}"
            )));
        }
        let target = dest.map(|d| d.join(&name));
        match etype {
            tar::EntryType::Directory => {
                if let Some(t) = &target {
                    fs::create_dir_all(t)?;
                    dir_modes.push((t.clone(), mode));
                }
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if name == TREE_PREFIX {
                    return Err(CoreError::Refused(format!(
                        "{TREE_PREFIX} must be a directory"
                    )));
                }
                files += 1;
                bytes += entry.size();
                if let Some(t) = &target {
                    write_file(&mut entry, t, mode)?;
                }
            }
            tar::EntryType::Symlink => {
                let link = entry.link_name()?.ok_or_else(|| {
                    CoreError::Refused(format!("{name}: a link without a target"))
                })?;
                let link = link
                    .to_str()
                    .ok_or_else(|| CoreError::Refused(format!("{name}: target is not UTF-8")))?
                    .to_owned();
                check_link(&name, &link)?;
                links.insert(name.clone());
                if let Some(t) = &target {
                    if let Some(parent) = t.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    std::os::unix::fs::symlink(&link, t)?;
                }
            }
            other => {
                return Err(CoreError::Refused(format!(
                    "{name}: only files, directories and symbolic links are allowed, not {other:?}"
                )));
            }
        }
    }
    let manifest = manifest.ok_or_else(|| CoreError::Malformed {
        what: "bundle".into(),
        detail: "the archive is empty".into(),
    })?;
    if manifest.files != files || manifest.bytes != bytes {
        return Err(CoreError::Malformed {
            what: "bundle".into(),
            detail: format!(
                "the manifest lists {} files and {} bytes, the archive holds {files} and {bytes}",
                manifest.files, manifest.bytes
            ),
        });
    }
    if !seen.contains(TREE_PREFIX) {
        return Err(CoreError::Malformed {
            what: "bundle".into(),
            detail: format!("no {TREE_PREFIX}/ directory"),
        });
    }
    if manifest.runner_image.is_some() && !has_image {
        return Err(CoreError::Malformed {
            what: "bundle".into(),
            detail: format!(
                "the manifest names a runner image but there is no {RUNNER_IMAGE_NAME}"
            ),
        });
    }
    // Directory modes last, deepest first, so a read-only directory cannot block its contents.
    for (path, mode) in dir_modes.iter().rev() {
        fs::set_permissions(path, fs::Permissions::from_mode(*mode | 0o700))?;
    }
    Ok(manifest)
}

fn write_file<R: Read>(entry: &mut tar::Entry<'_, R>, target: &Path, mode: u32) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(target)?;
    io::copy(entry, &mut file)?;
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    if let Ok(mtime) = entry.header().mtime() {
        // Best effort: a wrong timestamp does not make the file wrong.
        let _ = file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(mtime));
    }
    Ok(())
}

/// Entry names: relative, `/`-separated, no empty, `.` or `..` component, no NUL, and either
/// [`TREE_PREFIX`] or below it.
fn check_name(name: &str) -> Result<()> {
    let bad = |why: &str| Err(CoreError::Refused(format!("entry {name:?}: {why}")));
    if name.is_empty() || name.starts_with('/') || name.contains('\0') || name.contains('\\') {
        return bad("not a relative path");
    }
    if name
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return bad("empty, . or .. component");
    }
    if name != TREE_PREFIX && !name.starts_with(&format!("{TREE_PREFIX}/")) {
        return bad(&format!("outside {TREE_PREFIX}/"));
    }
    Ok(())
}

/// Link targets: relative ones must stay inside [`TREE_PREFIX`] once resolved from the link's
/// directory; absolute ones are allowed only for a virtual environment's interpreter.
fn check_link(name: &str, target: &str) -> Result<()> {
    let bad = |why: &str| {
        Err(CoreError::Refused(format!(
            "link {name} -> {target:?}: {why}"
        )))
    };
    if target.is_empty() || target.contains('\0') || target.contains('\\') {
        return bad("not a usable target");
    }
    if let Some(abs) = target.strip_prefix('/') {
        let interpreter = name
            .rsplit_once('/')
            .is_some_and(|(dir, file)| dir.ends_with(".venv/bin") && file.starts_with("python"));
        if !interpreter {
            return bad("absolute targets are allowed only for a .venv/bin/python* interpreter");
        }
        if abs.split('/').any(|c| c == "..") {
            return bad("a .. component");
        }
        return Ok(());
    }
    let mut parts: Vec<&str> = name.split('/').collect();
    parts.pop();
    for c in target.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
                if parts.is_empty() {
                    return bad(&format!("it leaves {TREE_PREFIX}/"));
                }
            }
            other => parts.push(other),
        }
    }
    if parts.first() != Some(&TREE_PREFIX) {
        return bad(&format!("it leaves {TREE_PREFIX}/"));
    }
    Ok(())
}

/// `a/b/c` -> `a`, `a/b` (proper ancestors).
fn ancestors(name: &str) -> impl Iterator<Item = &str> {
    name.match_indices('/').map(move |(i, _)| &name[..i])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(stats: TreeStats) -> BundleManifest {
        BundleManifest {
            format: FORMAT,
            workbench_version: "0.0.0-test".into(),
            created_at: "2026-10-26T00:00:00Z".into(),
            harness2_commit: crate::TESTED_HARNESS2_COMMIT.into(),
            harness2_version: Some("0.1.0".into()),
            python: Some("3.12.15".into()),
            python_home: Some("/opt/uv/python/cpython-3.12-linux-x86_64-gnu/bin".into()),
            arch: std::env::consts::ARCH.into(),
            benchmarks: vec![],
            files: stats.files,
            bytes: stats.bytes,
            runner_image: None,
        }
    }

    /// A small installation: nested files, an executable, a relative link, a venv
    /// interpreter link, a long path.
    fn fake_home(root: &Path) -> PathBuf {
        let home = root.join("harness2");
        fs::create_dir_all(home.join(".venv/bin")).unwrap();
        fs::create_dir_all(home.join("dependencies/tools/node_modules/.bin")).unwrap();
        fs::write(
            home.join("pyproject.toml"),
            "[project]\nname = \"harness2\"\n",
        )
        .unwrap();
        let script = home.join("dependencies/tools/node_modules/bun/bin/bun");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, "#!/bin/sh\necho bun\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(
            "../bun/bin/bun",
            home.join("dependencies/tools/node_modules/.bin/bun"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            "/opt/uv/python/cpython-3.12-linux-x86_64-gnu/bin/python3.12",
            home.join(".venv/bin/python"),
        )
        .unwrap();
        let deep = home.join(format!("dependencies/{}/file.txt", "d".repeat(120)));
        fs::create_dir_all(deep.parent().unwrap()).unwrap();
        fs::write(&deep, "long path").unwrap();
        home
    }

    fn bundle_of(home: &Path) -> (Vec<u8>, BundleManifest) {
        let stats = survey(home).unwrap();
        assert_eq!(
            (stats.files, stats.symlinks, stats.skipped_special),
            (3, 2, 0)
        );
        let m = manifest(stats);
        let mut out = Vec::new();
        let written = write(&mut out, home, &m, None).unwrap();
        assert_eq!(written, stats);
        (out, m)
    }

    #[test]
    fn round_trip_keeps_content_modes_and_links() {
        let src = tempfile::tempdir().unwrap();
        let home = fake_home(src.path());
        let (bytes, m) = bundle_of(&home);
        assert_eq!(read_manifest(bytes.as_slice()).unwrap(), m);
        assert_eq!(scan(bytes.as_slice()).unwrap(), m);
        let dst = tempfile::tempdir().unwrap();
        let got = unpack(bytes.as_slice(), dst.path(), None).unwrap();
        assert_eq!(got, m);
        let out = dst.path().join(TREE_PREFIX);
        let bun = out.join("dependencies/tools/node_modules/bun/bin/bun");
        assert_eq!(fs::read_to_string(&bun).unwrap(), "#!/bin/sh\necho bun\n");
        assert_eq!(fs::metadata(&bun).unwrap().mode() & 0o777, 0o755);
        assert_eq!(
            fs::read_link(out.join("dependencies/tools/node_modules/.bin/bun")).unwrap(),
            Path::new("../bun/bin/bun")
        );
        assert_eq!(
            fs::read_link(out.join(".venv/bin/python")).unwrap(),
            Path::new("/opt/uv/python/cpython-3.12-linux-x86_64-gnu/bin/python3.12")
        );
        let deep = out.join(format!("dependencies/{}/file.txt", "d".repeat(120)));
        assert_eq!(fs::read_to_string(deep).unwrap(), "long path");
        // An unpack target must be empty.
        assert!(matches!(
            unpack(bytes.as_slice(), dst.path(), None),
            Err(CoreError::Refused(_))
        ));
    }

    #[test]
    fn runner_image_travels_second() {
        let src = tempfile::tempdir().unwrap();
        let home = fake_home(src.path());
        let image = src.path().join("image.tar");
        fs::write(&image, b"not really an image").unwrap();
        let mut m = manifest(survey(&home).unwrap());
        m.runner_image = Some(BundleImage {
            reference: "harness2-workbench-runner:0.1.1".into(),
            id: "sha256:abc".into(),
            bytes: 19,
        });
        let mut out = Vec::new();
        write(&mut out, &home, &m, Some(&image)).unwrap();
        let dst = tempfile::tempdir().unwrap();
        let image_out = src.path().join("loaded.tar");
        let got = unpack(out.as_slice(), dst.path(), Some(&image_out)).unwrap();
        assert_eq!(
            got.runner_image.unwrap().reference,
            "harness2-workbench-runner:0.1.1"
        );
        assert_eq!(fs::read(&image_out).unwrap(), b"not really an image");
        // A manifest without an image may not carry one.
        let mut m2 = m.clone();
        m2.runner_image = None;
        let mut out2 = Vec::new();
        write(&mut out2, &home, &m2, Some(&image)).unwrap();
        assert!(matches!(scan(out2.as_slice()), Err(CoreError::Refused(_))));
    }

    #[test]
    fn counts_must_match_the_manifest() {
        let src = tempfile::tempdir().unwrap();
        let home = fake_home(src.path());
        let mut m = manifest(survey(&home).unwrap());
        m.bytes += 1;
        let mut out = Vec::new();
        write(&mut out, &home, &m, None).unwrap();
        assert!(matches!(
            scan(out.as_slice()),
            Err(CoreError::Malformed { .. })
        ));
    }

    /// An archive built entry by entry, names written raw so that `tar` cannot clean them.
    fn crafted(entries: &[(&str, tar::EntryType, &str)]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        let json = serde_json::to_vec(&manifest(TreeStats {
            files: entries
                .iter()
                .filter(|(_, t, _)| *t == tar::EntryType::Regular)
                .count() as u64,
            ..TreeStats::default()
        }))
        .unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Regular);
        h.set_size(json.len() as u64);
        h.set_mode(0o644);
        h.set_path(MANIFEST_NAME).unwrap();
        h.set_cksum();
        b.append(&h, json.as_slice()).unwrap();
        for (name, kind, link) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(*kind);
            h.set_size(0);
            h.set_mode(0o4755);
            let raw = &mut h.as_gnu_mut().unwrap().name;
            raw[..name.len()].copy_from_slice(name.as_bytes());
            if !link.is_empty() {
                let l = &mut h.as_gnu_mut().unwrap().linkname;
                l[..link.len()].copy_from_slice(link.as_bytes());
            }
            h.set_cksum();
            b.append(&h, io::empty()).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    fn refused(entries: &[(&str, tar::EntryType, &str)]) -> bool {
        let bytes = crafted(entries);
        let dst = tempfile::tempdir().unwrap();
        let outcome = unpack(bytes.as_slice(), dst.path(), None);
        matches!(outcome, Err(CoreError::Refused(_)))
    }

    #[test]
    fn hostile_entries_are_refused() {
        use tar::EntryType::{Block, Char, Directory, Fifo, Link, Regular, Symlink};
        let dir = ("harness2", Directory, "");
        assert!(refused(&[dir, ("/etc/passwd", Regular, "")]));
        assert!(refused(&[dir, ("harness2/../../evil", Regular, "")]));
        assert!(refused(&[dir, ("other/file", Regular, "")]));
        assert!(refused(&[dir, ("harness2/./x", Regular, "")]));
        assert!(refused(&[dir, ("harness2/hard", Link, "harness2/x")]));
        assert!(refused(&[dir, ("harness2/fifo", Fifo, "")]));
        assert!(refused(&[dir, ("harness2/tty", Char, "")]));
        assert!(refused(&[dir, ("harness2/disk", Block, "")]));
        assert!(refused(&[dir, ("harness2/up", Symlink, "../../etc")]));
        assert!(refused(&[dir, ("harness2/abs", Symlink, "/etc/passwd")]));
        assert!(refused(&[
            dir,
            ("harness2/bin/python", Symlink, "/usr/bin/python3")
        ]));
        assert!(refused(&[
            dir,
            ("harness2/.venv/bin/python", Symlink, "/opt/../etc/x")
        ]));
        // A path below a link, a duplicate, the root as a file.
        assert!(refused(&[
            dir,
            ("harness2/link", Symlink, "sub"),
            ("harness2/link/f", Regular, "")
        ]));
        assert!(refused(&[
            dir,
            ("harness2/f", Regular, ""),
            ("harness2/f", Regular, "")
        ]));
        assert!(refused(&[("harness2", Regular, "")]));
        // Allowed: a link inside the tree and an interpreter link.
        let ok = crafted(&[
            dir,
            ("harness2/a", Directory, ""),
            ("harness2/a/l", Symlink, "../b"),
            (
                "harness2/.venv/bin/python3.12",
                Symlink,
                "/opt/uv/python/bin/python3.12",
            ),
            ("harness2/b", Regular, ""),
        ]);
        let dst = tempfile::tempdir().unwrap();
        unpack(ok.as_slice(), dst.path(), None).unwrap();
        // Setuid bits never survive.
        let b = dst.path().join("harness2/b");
        assert_eq!(fs::metadata(b).unwrap().mode() & 0o7777, 0o755);
    }

    #[test]
    fn the_manifest_comes_first() {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Directory);
        h.set_size(0);
        h.set_path("harness2").unwrap();
        h.set_cksum();
        b.append(&h, io::empty()).unwrap();
        let bytes = b.into_inner().unwrap().finish().unwrap();
        assert!(matches!(
            scan(bytes.as_slice()),
            Err(CoreError::Malformed { .. })
        ));
        assert!(scan(&b""[..]).is_err());
    }

    #[test]
    fn compatibility_names_each_problem() {
        let mut m = manifest(TreeStats::default());
        assert!(compatibility(&m, crate::TESTED_HARNESS2_COMMIT, false).is_empty());
        m.harness2_commit = "0".repeat(40);
        assert_eq!(
            compatibility(&m, crate::TESTED_HARNESS2_COMMIT, false).len(),
            1
        );
        assert!(compatibility(&m, crate::TESTED_HARNESS2_COMMIT, true).is_empty());
        m.arch = "sparc".into();
        let p = compatibility(&m, crate::TESTED_HARNESS2_COMMIT, true);
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("sparc"));
    }

    #[test]
    fn sha256_and_its_line() {
        assert_eq!(
            sha256_hex(&b"abc"[..]).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(sha256_line("ab", "b.tar.gz"), "ab  b.tar.gz");
    }
}
