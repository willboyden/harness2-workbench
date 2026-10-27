//! `h2workbench bundle export | verify | import`: offline install bundles of the managed harness2
//! installation. [`h2w_core::bundle`] owns the format and every check on an archive's contents;
//! this module adds what needs the configuration, the database and the container runtime:
//! refusing while jobs could be using the installation, the runner image (`save` / `load`),
//! the swap into place (the previous installation is kept), and audit records.

// Printing to stdout/stderr is this module's job (stdout: machine-readable results only).
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Seek, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, bail};
use h2w_api::{AuditOutcome, Isolation, JobKind, JobStatus};
use h2w_core::bundle::{self, BundleBenchmark, BundleImage, BundleManifest};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::db::audit::AuditEntry;

/// `bundle export`: write the installation to `output` and `<output>.sha256`; print the
/// `sha256sum` line on stdout.
///
/// # Errors
/// harness2 not installed, an install or setup job running, `output` existing without
/// `force`, the runtime failing to save the image, I/O errors.
pub fn export(
    cfg: &Config,
    output: &Path,
    with_runner_image: bool,
    force: bool,
) -> anyhow::Result<ExitCode> {
    let home = &cfg.paths.harness2_home;
    let status = h2w_core::install::read_status(home, cfg.paths.harness2_managed);
    let Some(commit) = status.commit.clone().filter(|_| status.installed) else {
        bail!(
            "harness2 is not installed at {}; install it (an install_harness2 job) and set up the \
             benchmarks you need first",
            home.display()
        );
    };
    if let Some(job) = jobs_in(cfg, &[JobStatus::Running])?
        .into_iter()
        .find(|j| matches!(j.kind, JobKind::InstallHarness2 | JobKind::Setup))
    {
        bail!(
            "job {} ({}) is changing the installation; export when it has finished",
            job.id,
            kind_name(job.kind)
        );
    }
    if output.exists() && !force {
        bail!("{} exists; pass --force to replace it", output.display());
    }
    let dir = parent_dir(output)?;
    let name = file_name(output)?;
    let work = Scratch::new(&dir, &format!(".{name}.export"))?;
    let image = if with_runner_image {
        Some(save_image(cfg, &work.path.join(bundle::RUNNER_IMAGE_NAME))?)
    } else {
        None
    };
    eprintln!("Surveying {} ...", home.display());
    let stats = bundle::survey(home)?;
    if stats.skipped_special > 0 {
        eprintln!(
            "note: {} sockets, FIFOs or devices are left out",
            stats.skipped_special
        );
    }
    let manifest = BundleManifest {
        format: bundle::FORMAT,
        workbench_version: env!("CARGO_PKG_VERSION").to_owned(),
        created_at: crate::util::now_rfc3339(),
        harness2_commit: commit,
        harness2_version: status.version,
        python: status.python,
        python_home: python_home(home),
        arch: std::env::consts::ARCH.to_owned(),
        benchmarks: h2w_core::bench::status(home)
            .into_iter()
            .filter(|b| b.installed)
            .map(|b| BundleBenchmark {
                bench: b.bench.as_cli().to_owned(),
                subset: b.subset.map(|s| s.as_cli().to_owned()),
                at_pinned_commit: b.at_pinned_commit.unwrap_or(false),
                data_present: b.data_present,
            })
            .collect(),
        files: stats.files,
        bytes: stats.bytes,
        runner_image: image.as_ref().map(|(info, _)| info.clone()),
    };
    eprintln!(
        "Writing {} files ({}){} to {} ...",
        stats.files,
        human_bytes(stats.bytes),
        if image.is_some() {
            " and the runner image"
        } else {
            ""
        },
        output.display()
    );
    let partial = work.path.join("bundle.partial");
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&partial)
        .with_context(|| format!("creating {}", partial.display()))?;
    let mut out = HashingWriter::new(BufWriter::with_capacity(1 << 20, file));
    let written = bundle::write(
        &mut out,
        home,
        &manifest,
        image.as_ref().map(|(_, p)| p.as_path()),
    )?;
    let (hex, writer) = out.finish();
    writer
        .into_inner()
        .map_err(io::IntoInnerError::into_error)?
        .sync_all()?;
    if written.files != stats.files || written.bytes != stats.bytes {
        bail!("the installation changed while it was being written; run the export again");
    }
    fs::rename(&partial, output)
        .with_context(|| format!("moving the bundle to {}", output.display()))?;
    let line = bundle::sha256_line(&hex, &name);
    fs::write(sidecar(output), format!("{line}\n"))?;
    audit(
        cfg,
        "bundle.export",
        &output.display().to_string(),
        &format!(
            "sha256 {hex}; harness2 {}; {} files",
            manifest.harness2_commit, manifest.files
        ),
    )?;
    println!("{line}");
    eprintln!(
        "Wrote {} ({}) and {}. Give the SHA-256 to whoever imports it through a channel you \
         trust: `h2workbench bundle import {name} --sha256 {hex}`.",
        output.display(),
        human_bytes(fs::metadata(output)?.len()),
        sidecar(output).display()
    );
    Ok(ExitCode::SUCCESS)
}

/// `bundle verify`: the archive's SHA-256 (compared when `expected` is given), every entry, and
/// whether this Workbench can use it. Installs nothing.
///
/// # Errors
/// A SHA-256 that differs from `expected`, an archive that fails the checks, I/O errors.
pub fn verify(cfg: &Config, path: &Path, expected: Option<&str>) -> anyhow::Result<ExitCode> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    eprintln!("Computing the SHA-256 of {} ...", path.display());
    let hex = bundle::sha256_hex(BufReader::with_capacity(1 << 20, &file))?;
    match expected {
        Some(e) => check_sha(e, &hex)?,
        None => eprintln!(
            "SHA-256 {hex} (not compared: pass --sha256 with the value you received with the bundle)"
        ),
    }
    file.rewind()?;
    eprintln!("Checking every entry ...");
    let manifest = bundle::scan(BufReader::with_capacity(1 << 20, &file))?;
    describe(&manifest);
    let problems = problems(cfg, &manifest);
    if problems.is_empty() {
        println!("bundle OK: {hex}");
        Ok(ExitCode::SUCCESS)
    } else {
        for p in &problems {
            println!("problem: {p}");
        }
        Ok(ExitCode::FAILURE)
    }
}

/// `bundle import`: replace the managed installation with the bundle's. The previous one is
/// kept as `<harness2_home>.previous` (an older one there is removed first).
///
/// # Errors
/// An unmanaged installation, a running job, a SHA-256 that differs, a bundle this Workbench
/// cannot use, too little disk space, the runtime failing to load the image, I/O errors. On
/// any error the current installation is left as it was.
pub fn import(cfg: &Config, path: &Path, expected: &str) -> anyhow::Result<ExitCode> {
    if !cfg.paths.harness2_managed {
        bail!(
            "paths.harness2_managed is false: a bundle can only replace an installation the \
             Workbench manages"
        );
    }
    // A queued job could start between the two renames below, so the queue must be empty too.
    if let Some(job) = jobs_in(cfg, &[JobStatus::Running, JobStatus::Queued])?.first() {
        bail!(
            "job {} ({}) is {}; import when no job is queued or running",
            job.id,
            kind_name(job.kind),
            if job.status == JobStatus::Queued {
                "queued"
            } else {
                "running"
            }
        );
    }
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    eprintln!("Checking the SHA-256 of {} ...", path.display());
    let hex = bundle::sha256_hex(BufReader::with_capacity(1 << 20, &file))?;
    check_sha(expected, &hex)?;
    file.rewind()?;
    let manifest = bundle::read_manifest(BufReader::new(&file))?;
    let problems = problems(cfg, &manifest);
    if !problems.is_empty() {
        bail!(
            "this Workbench cannot use the bundle:\n  - {}",
            problems.join("\n  - ")
        );
    }
    let home = &cfg.paths.harness2_home;
    let parent = parent_dir(home)?;
    fs::create_dir_all(&parent)?;
    let needed = manifest.bytes + manifest.runner_image.as_ref().map_or(0, |i| i.bytes);
    let free = free_bytes(&parent)?;
    if free < needed + needed / 10 {
        bail!(
            "{} has {} free; the bundle needs {} and a margin",
            parent.display(),
            human_bytes(free),
            human_bytes(needed)
        );
    }
    let home_name = file_name(home)?;
    let work = Scratch::new(&parent, &format!(".{home_name}.import"))?;
    let tree = work.path.join("tree");
    fs::DirBuilder::new().mode(0o700).create(&tree)?;
    let image_file = work.path.join(bundle::RUNNER_IMAGE_NAME);
    file.rewind()?;
    eprintln!(
        "Checking and unpacking {} files ({}) ...",
        manifest.files,
        human_bytes(manifest.bytes)
    );
    bundle::unpack(
        BufReader::with_capacity(1 << 20, &file),
        &tree,
        Some(&image_file),
    )?;
    if let Some(image) = &manifest.runner_image {
        if cfg.isolation() == Isolation::Container {
            load_image(cfg, &image_file, image)?;
        } else {
            eprintln!(
                "note: the bundle carries the runner image {}; not loaded (isolation.mode is host)",
                image.reference
            );
        }
    }
    let previous = parent.join(format!("{home_name}.previous"));
    if home.exists() {
        if previous.exists() {
            fs::remove_dir_all(&previous)
                .with_context(|| format!("removing the older {}", previous.display()))?;
        }
        fs::rename(home, &previous).with_context(|| format!("moving {} aside", home.display()))?;
    }
    if let Err(e) = fs::rename(tree.join(bundle::TREE_PREFIX), home) {
        // Put the previous installation back before reporting.
        if previous.exists() && !home.exists() {
            let _ = fs::rename(&previous, home);
        }
        return Err(e)
            .with_context(|| format!("moving the new installation to {}", home.display()));
    }
    audit(
        cfg,
        "bundle.import",
        &path.display().to_string(),
        &format!(
            "sha256 {hex}; harness2 {}; {} files",
            manifest.harness2_commit, manifest.files
        ),
    )?;
    println!("installed {hex}");
    eprintln!(
        "Installed harness2 {} ({} files) in {}.{} Next: `h2workbench doctor`, then a `check` job \
         for each benchmark you use.",
        short(&manifest.harness2_commit),
        manifest.files,
        home.display(),
        if previous.exists() {
            format!(" The previous installation is in {}.", previous.display())
        } else {
            String::new()
        }
    );
    Ok(ExitCode::SUCCESS)
}

/// Problems that keep this Workbench from using a bundle: the format, architecture and
/// harness2 commit, then whether the interpreter its virtual environment needs exists where
/// jobs run.
fn problems(cfg: &Config, m: &BundleManifest) -> Vec<String> {
    let mut problems = bundle::compatibility(
        m,
        h2w_core::TESTED_HARNESS2_COMMIT,
        cfg.file.paths.allow_untested_harness2_rev,
    );
    let Some(py) = m.python_home.as_deref() else {
        return problems;
    };
    match cfg.isolation() {
        Isolation::Host => {
            if !Path::new(py).is_dir() {
                problems.push(format!(
                    "the bundle's Python environment needs the interpreter in {py}, which this host \
                     does not have; a bundle made for container isolation goes to a Workbench with \
                     isolation.mode = \"container\" and the same runner image"
                ));
            }
        }
        // An image the bundle brings is loaded on import; otherwise ask the configured one.
        Isolation::Container if m.runner_image.is_none() => {
            let c = &cfg.file.isolation.container;
            if !matches!(image_id(c.runtime.as_str(), &c.image), Ok(Some(_))) {
                problems.push(format!(
                    "the runner image {} is not present for {} (build it, or export the bundle \
                     with --with-runner-image)",
                    c.image,
                    c.runtime.as_str()
                ));
            } else if !image_has_dir(cfg, py) {
                problems.push(format!(
                    "the runner image {} has no {py}, the interpreter the bundle's Python \
                     environment needs (export with --with-runner-image, or build the same image)",
                    c.image
                ));
            }
        }
        Isolation::Container => {}
    }
    problems
}

fn describe(m: &BundleManifest) {
    eprintln!(
        "bundle: harness2 {} (Python {}), {} files ({}), made by Workbench {} on {} for {}",
        short(&m.harness2_commit),
        m.python.as_deref().unwrap_or("?"),
        m.files,
        human_bytes(m.bytes),
        m.workbench_version,
        m.created_at,
        m.arch
    );
    for b in &m.benchmarks {
        eprintln!(
            "  benchmark {}{}: {}, {}",
            b.bench,
            b.subset
                .as_deref()
                .map_or(String::new(), |s| format!(" ({s})")),
            if b.at_pinned_commit {
                "at its pinned commit"
            } else {
                "NOT at its pinned commit"
            },
            if b.data_present {
                "data present"
            } else {
                "no data"
            }
        );
    }
    if let Some(i) = &m.runner_image {
        eprintln!(
            "  runner image {} ({}, {})",
            i.reference,
            i.id,
            human_bytes(i.bytes)
        );
    }
}

fn check_sha(expected: &str, actual: &str) -> anyhow::Result<()> {
    let e = expected.trim().to_ascii_lowercase();
    if e.len() != 64 || !e.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("--sha256 must be 64 hexadecimal characters");
    }
    if e != actual {
        bail!(
            "SHA-256 mismatch: the file is {actual}, expected {e}; the bundle was changed or \
             damaged, or the value belongs to another file"
        );
    }
    eprintln!("SHA-256 matches.");
    Ok(())
}

/// Jobs in any of `states`, per the database (none when it does not exist yet).
fn jobs_in(cfg: &Config, states: &[JobStatus]) -> anyhow::Result<Vec<h2w_api::Job>> {
    if !cfg.paths.db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = crate::db::open_connection(&cfg.paths.db_path)?;
    let mut jobs = Vec::new();
    for state in states {
        jobs.extend(
            crate::db::jobs::with_status(&conn, *state)?
                .into_iter()
                .map(|r| r.job),
        );
    }
    Ok(jobs)
}

fn audit(cfg: &Config, action: &str, target: &str, detail: &str) -> anyhow::Result<()> {
    if !cfg.paths.db_path.exists() {
        return Ok(());
    }
    let mut conn = crate::db::open_connection(&cfg.paths.db_path)?;
    let entry = AuditEntry::new("cli", action, target, AuditOutcome::Success).detail(detail);
    crate::db::write_tx(&mut conn, |tx| crate::db::audit::append(tx, &entry))?;
    Ok(())
}

/// `<runtime> save` of the configured runner image, with the id the runtime reports.
fn save_image(cfg: &Config, out: &Path) -> anyhow::Result<(BundleImage, PathBuf)> {
    let runtime = cfg.file.isolation.container.runtime.as_str();
    let image = &cfg.file.isolation.container.image;
    let id = image_id(runtime, image)?.with_context(|| {
        format!("the runner image {image} is not present for {runtime}; build it first")
    })?;
    eprintln!("Saving the runner image {image} ...");
    let status = Command::new(runtime)
        .args(["save", "-o"])
        .arg(out)
        .arg(image)
        .status()
        .with_context(|| format!("running {runtime} save"))?;
    if !status.success() {
        bail!("{runtime} save {image} failed ({status})");
    }
    let bytes = fs::metadata(out)?.len();
    Ok((
        BundleImage {
            reference: image.clone(),
            id,
            bytes,
        },
        out.to_path_buf(),
    ))
}

/// `<runtime> load` of a bundle's image, unless the runtime already has that exact image; the
/// loaded image must carry the id the manifest names.
fn load_image(cfg: &Config, file: &Path, image: &BundleImage) -> anyhow::Result<()> {
    let runtime = cfg.file.isolation.container.runtime.as_str();
    if image.reference != cfg.file.isolation.container.image {
        eprintln!(
            "note: the bundle's runner image is {}, the configuration names {}; loading it anyway",
            image.reference, cfg.file.isolation.container.image
        );
    }
    if image_id(runtime, &image.reference)?.as_deref() == Some(image.id.as_str()) {
        eprintln!("The runner image {} is already present.", image.reference);
        return Ok(());
    }
    eprintln!("Loading the runner image {} ...", image.reference);
    let status = Command::new(runtime)
        .args(["load", "-q", "-i"])
        .arg(file)
        .status()
        .with_context(|| format!("running {runtime} load"))?;
    if !status.success() {
        bail!("{runtime} load failed ({status})");
    }
    match image_id(runtime, &image.reference)? {
        Some(id) if id == image.id => Ok(()),
        other => bail!(
            "after loading, {} has id {}, but the bundle's manifest names {}",
            image.reference,
            other.as_deref().unwrap_or("(none)"),
            image.id
        ),
    }
}

/// The id of a local image, `None` when the runtime has no such image.
fn image_id(runtime: &str, image: &str) -> anyhow::Result<Option<String>> {
    let out = Command::new(runtime)
        .args(["image", "inspect", "--format", "{{.Id}}", image])
        .output()
        .with_context(|| format!("running {runtime} image inspect"))?;
    if !out.status.success() {
        return Ok(None);
    }
    let id = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Ok((!id.is_empty()).then_some(id))
}

/// Whether the configured runner image has `dir` (a throw-away container, no network).
fn image_has_dir(cfg: &Config, dir: &str) -> bool {
    let c = &cfg.file.isolation.container;
    Command::new(c.runtime.as_str())
        .args(["run", "--rm", "--network", "none", "--entrypoint", "test"])
        .arg(&c.image)
        .args(["-d", dir])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// `home` of the virtual environment's `pyvenv.cfg`.
fn python_home(harness2_home: &Path) -> Option<String> {
    let cfg = fs::read_to_string(harness2_home.join(".venv/pyvenv.cfg")).ok()?;
    cfg.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "home").then(|| v.trim().to_owned())
    })
}

fn free_bytes(dir: &Path) -> anyhow::Result<u64> {
    let st = nix::sys::statvfs::statvfs(dir)
        .with_context(|| format!("reading free space of {}", dir.display()))?;
    #[allow(clippy::unnecessary_cast)] // the field types differ between platforms
    Ok(st.blocks_available() as u64 * st.fragment_size() as u64)
}

fn sidecar(output: &Path) -> PathBuf {
    let mut s = output.as_os_str().to_owned();
    s.push(".sha256");
    PathBuf::from(s)
}

fn parent_dir(path: &Path) -> anyhow::Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok(parent.to_path_buf())
}

fn file_name(path: &Path) -> anyhow::Result<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .with_context(|| format!("{} has no usable file name", path.display()))
}

fn kind_name(kind: JobKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{kind:?}"))
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

#[allow(clippy::cast_precision_loss)] // a display figure
fn human_bytes(b: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let f = b as f64;
    if f >= GIB {
        format!("{:.1} GiB", f / GIB)
    } else {
        format!("{:.1} MiB", f / MIB)
    }
}

/// A private directory next to a target, removed with everything in it when dropped.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(parent: &Path, prefix: &str) -> anyhow::Result<Scratch> {
        let path = parent.join(format!("{prefix}-{}", std::process::id()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        Ok(Scratch { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A writer that hashes what passes through it.
struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        HashingWriter {
            inner,
            hasher: Sha256::new(),
        }
    }

    /// The hex digest and the inner writer (flushed).
    fn finish(mut self) -> (String, W) {
        let _ = self.inner.flush();
        (hex::encode(self.hasher.finalize()), self.inner)
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashing_writer_matches_sha256() {
        let mut w = HashingWriter::new(Vec::new());
        w.write_all(b"abc").unwrap();
        let (hex, inner) = w.finish();
        assert_eq!(inner, b"abc");
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha_check_is_strict_and_case_blind() {
        let h = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(check_sha(&h.to_uppercase(), h).is_ok());
        assert!(check_sha(&format!(" {h}\n"), h).is_ok());
        assert!(check_sha("abc", h).is_err());
        let other = format!("{}0", &h[..63]);
        assert!(check_sha(&other, h).is_err());
    }

    #[test]
    fn sidecar_and_names() {
        assert_eq!(
            sidecar(Path::new("/b/x.tar.gz")),
            PathBuf::from("/b/x.tar.gz.sha256")
        );
        assert_eq!(
            parent_dir(Path::new("x.tar.gz")).unwrap(),
            PathBuf::from(".")
        );
        assert_eq!(file_name(Path::new("/b/x.tar.gz")).unwrap(), "x.tar.gz");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn python_home_reads_pyvenv_cfg() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".venv")).unwrap();
        fs::write(
            tmp.path().join(".venv/pyvenv.cfg"),
            "home = /opt/uv/python/cpython-3.12-linux-x86_64-gnu/bin\nversion_info = 3.12.15\n",
        )
        .unwrap();
        assert_eq!(
            python_home(tmp.path()).as_deref(),
            Some("/opt/uv/python/cpython-3.12-linux-x86_64-gnu/bin")
        );
        assert_eq!(python_home(&tmp.path().join("missing")), None);
    }

    #[test]
    fn scratch_is_private_and_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = {
            let s = Scratch::new(tmp.path(), ".x").unwrap();
            fs::write(s.path.join("f"), "x").unwrap();
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&s.path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            s.path.clone()
        };
        assert!(!path.exists());
    }
}
