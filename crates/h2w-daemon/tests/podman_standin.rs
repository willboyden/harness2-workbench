//! The runner image's `podman` stand-in (`deploy/docker/podman-standin.sh`): it runs LAB's
//! document parser for exactly the call LAB's `read_doc.py` makes, and refuses everything else
//! with podman's own status, 125. The script runs under `sh` with a fake LAB checkout and a
//! fake `python3` that prints its arguments, so the test needs neither a container runtime nor
//! Python.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/docker/podman-standin.sh")
}

struct Fixture {
    dir: tempfile::TempDir,
    bin: PathBuf,
    lab: PathBuf,
    doc: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let py = bin.join("python3");
        fs::write(&py, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
        fs::set_permissions(&py, fs::Permissions::from_mode(0o755)).unwrap();
        let lab = dir.path().join("harvey-labs");
        fs::create_dir_all(lab.join("sandbox/parsers")).unwrap();
        fs::write(lab.join("sandbox/parsers/parse_doc.py"), "").unwrap();
        let docs = dir.path().join("workspace/documents");
        fs::create_dir_all(&docs).unwrap();
        // A space, parentheses and a glob character: the path must pass through unharmed.
        let doc = docs.join("Brief (draft) [v2].pdf");
        fs::write(&doc, b"%PDF-1.7").unwrap();
        Fixture { dir, bin, lab, doc }
    }

    fn run(&self, args: &[String], lab_root: Option<&Path>) -> Output {
        let mut c = Command::new("sh");
        c.arg(script())
            .args(args)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()));
        if let Some(root) = lab_root {
            c.env("LAB_BENCH_ROOT", root);
        }
        c.output().unwrap()
    }
}

/// The arguments LAB's `read_doc.py` (`parse_in_sandbox`) gives `podman`, with `path` already
/// resolved to an absolute file.
fn lab_call(path: &Path, ext: &str) -> Vec<String> {
    let name = path.file_name().unwrap().to_str().unwrap();
    [
        "run",
        "--rm",
        "--network=none",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "-v",
        &format!("{}:/doc/{name}:ro", path.display()),
        "lab-sandbox:latest",
        "parse-doc",
        ext,
        &format!("/doc/{name}"),
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

fn assert_refused(out: &Output, case: &str) {
    assert_eq!(out.status.code(), Some(125), "{case}: {out:?}");
    assert!(out.stdout.is_empty(), "{case}: printed {:?}", out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.starts_with("podman: "), "{case}: {err}");
    assert!(
        err.contains("only runs LAB's document parser"),
        "{case}: {err}"
    );
}

#[test]
fn runs_labs_parser_for_the_call_read_doc_makes() {
    let f = Fixture::new();
    let parser = f.lab.join("sandbox/parsers/parse_doc.py");
    for ext in ["docx", "pdf", "pptx", "xlsx"] {
        let out = f.run(&lab_call(&f.doc, ext), Some(&f.lab));
        assert!(out.status.success(), "{ext}: {out:?}");
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            format!("-I\n{}\n{ext}\n{}\n", parser.display(), f.doc.display()),
            "{ext}"
        );
    }
}

#[test]
fn refuses_every_other_call() {
    let f = Fixture::new();
    let doc = f.doc.display().to_string();
    let tail = |mount: &str, target: &str, ext: &str| {
        let mut v = args(&["run", "--rm", "-v"]);
        v.push(mount.to_owned());
        v.extend(args(&["img", "parse-doc", ext]));
        v.push(target.to_owned());
        v
    };
    let with = |extra: &[&str]| {
        let mut v = lab_call(&f.doc, "pdf");
        v.extend(args(extra));
        v
    };
    let mut privileged = lab_call(&f.doc, "pdf");
    privileged.insert(1, "--privileged".to_owned());
    let mut two_mounts = lab_call(&f.doc, "pdf");
    two_mounts.splice(1..1, args(&["-v", "/:/host:ro"]));
    let mut shell = lab_call(&f.doc, "pdf");
    shell.splice(8.., args(&["sh", "-c", "id", "x"]));
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("no command", vec![]),
        ("ps", args(&["ps"])),
        ("version", args(&["--version"])),
        ("exec", args(&["exec", "x", "id"])),
        ("privileged", privileged),
        ("second mount", two_mounts),
        ("other program", shell),
        ("extra argument", with(&["more"])),
        (
            "writable mount",
            tail(&format!("{doc}:/doc/a.pdf"), "/doc/a.pdf", "pdf"),
        ),
        (
            "mount for another file",
            tail(&format!("{doc}:/doc/a.pdf:ro"), "/doc/b.pdf", "pdf"),
        ),
        (
            "nested target",
            tail(&format!("{doc}:/doc/a/b.pdf:ro"), "/doc/a/b.pdf", "pdf"),
        ),
        (
            "target outside /doc",
            tail(&format!("{doc}:/etc/a.pdf:ro"), "/etc/a.pdf", "pdf"),
        ),
        (
            "dot target",
            tail(&format!("{doc}:/doc/..:ro"), "/doc/..", "pdf"),
        ),
        (
            "relative source",
            tail("a.pdf:/doc/a.pdf:ro", "/doc/a.pdf", "pdf"),
        ),
        (
            "directory source",
            tail(
                &format!("{}:/doc/w:ro", f.dir.path().display()),
                "/doc/w",
                "pdf",
            ),
        ),
        (
            "missing source",
            tail("/nonexistent.pdf:/doc/n.pdf:ro", "/doc/n.pdf", "pdf"),
        ),
        (
            "other document type",
            tail(&format!("{doc}:/doc/a.pdf:ro"), "/doc/a.pdf", "txt"),
        ),
        ("option after the image", with(&["--rm"])),
    ];
    for (case, a) in &cases {
        assert_refused(&f.run(a, Some(&f.lab)), case);
    }
}

#[test]
fn needs_a_lab_checkout() {
    let f = Fixture::new();
    let call = lab_call(&f.doc, "pdf");
    assert_refused(&f.run(&call, None), "LAB_BENCH_ROOT unset");
    assert_refused(
        &f.run(&call, Some(f.dir.path())),
        "LAB_BENCH_ROOT without the parser",
    );
}
