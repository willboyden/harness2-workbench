//! Environment checks: which probes to run, and what their results mean.
//!
//! The server runs each [`Probe`] (no shell, 10 s timeout) and hands the results to
//! [`evaluate`], which knows harness2's requirements: Python 3.12 through uv, Node >= 20
//! and npm (setup), git, Docker (WorkBuddy), Pandoc + Poppler (LAB), the Google Cloud CLI
//! and Application Default Credentials (Vertex profiles), and the container runtime and
//! runner image (container isolation).

use h2w_api::{CheckLevel, DoctorCheck, DoctorReport, Harness2Status, Isolation};

/// A command to run for a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub id: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
}

/// Every probe, in display order.
pub const PROBES: &[Probe] = &[
    Probe {
        id: "uv",
        program: "uv",
        args: &["--version"],
    },
    Probe {
        id: "python312",
        program: "uv",
        args: &["python", "find", "3.12"],
    },
    Probe {
        id: "git",
        program: "git",
        args: &["--version"],
    },
    Probe {
        id: "node",
        program: "node",
        args: &["--version"],
    },
    Probe {
        id: "npm",
        program: "npm",
        args: &["--version"],
    },
    Probe {
        id: "docker",
        program: "docker",
        args: &["info", "--format", "{{.ServerVersion}}"],
    },
    Probe {
        id: "podman",
        program: "podman",
        args: &["version", "--format", "{{.Client.Version}}"],
    },
    Probe {
        id: "pandoc",
        program: "pandoc",
        args: &["--version"],
    },
    Probe {
        id: "pdftoppm",
        program: "pdftoppm",
        args: &["-v"],
    },
    Probe {
        id: "gcloud",
        program: "gcloud",
        args: &["--version"],
    },
    Probe {
        id: "curl",
        program: "curl",
        args: &["--version"],
    },
];

/// Outcome of running one probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub id: String,
    /// The program was found on PATH.
    pub found: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Facts the server knows without probing.
#[derive(Debug, Clone)]
pub struct DoctorContext {
    pub harness2: Harness2Status,
    pub isolation: Isolation,
    /// Free bytes on the data directory's filesystem.
    pub free_disk_bytes: Option<u64>,
    /// Some profile uses Vertex AI (so gcloud/ADC matter).
    pub uses_vertex: bool,
    pub generated_at: String,
}

/// Free space below which the disk check warns.
const LOW_DISK_BYTES: u64 = 20 * 1024 * 1024 * 1024;

fn probe<'a>(results: &'a [ProbeResult], id: &str) -> Option<&'a ProbeResult> {
    results.iter().find(|r| r.id == id)
}

fn works(p: Option<&ProbeResult>) -> bool {
    p.is_some_and(|p| p.found && p.exit_code == Some(0))
}

/// First non-empty output line (stdout, else stderr), at most 120 characters. Lines that
/// contain an absolute path are not echoed (reports reach non-admins).
fn version_line(p: Option<&ProbeResult>) -> Option<String> {
    let p = p?;
    let line = p
        .stdout
        .lines()
        .chain(p.stderr.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())?;
    if line.contains('/') && line.split_whitespace().any(|w| w.starts_with('/')) {
        return None;
    }
    Some(line.chars().take(120).collect())
}

/// Major version from `v22.3.0` / `22.3.0`.
fn major(line: &str) -> Option<u32> {
    let t = line.trim().trim_start_matches('v');
    t.split(['.', ' ']).next()?.parse().ok()
}

struct Checks(Vec<DoctorCheck>);

impl Checks {
    fn push(
        &mut self,
        id: &str,
        label: &str,
        level: CheckLevel,
        detail: impl Into<String>,
        remedy: Option<&str>,
        needed_for: &[&str],
    ) {
        self.0.push(DoctorCheck {
            id: id.to_owned(),
            label: label.to_owned(),
            level,
            detail: detail.into(),
            remedy: remedy.map(str::to_owned),
            needed_for: needed_for.iter().map(|s| (*s).to_owned()).collect(),
        });
    }

    /// A tool some features need: ok with its version, else `missing_level`.
    #[allow(clippy::too_many_arguments)]
    fn tool(
        &mut self,
        results: &[ProbeResult],
        id: &str,
        label: &str,
        missing_level: CheckLevel,
        missing: &str,
        remedy: &str,
        needed_for: &[&str],
    ) {
        let p = probe(results, id);
        if works(p) {
            let detail = version_line(p).unwrap_or_else(|| format!("{label} is available"));
            self.push(id, label, CheckLevel::Ok, detail, None, needed_for);
        } else {
            self.push(id, label, missing_level, missing, Some(remedy), needed_for);
        }
    }

    /// Under container isolation every job runs in the runner image, so this host's copy of a
    /// toolchain program decides nothing: the check is skipped (`runner_image` covers the image).
    fn in_image(&mut self, id: &str, label: &str, needed_for: &[&str]) {
        self.push(
            id,
            label,
            CheckLevel::Skip,
            format!("container isolation: jobs use the runner image's {label}, not this host's"),
            None,
            needed_for,
        );
    }
}

const ALL_BENCHES: &[&str] = &["demo", "jb", "lab", "wb"];

/// Turn probe results into checks with remedies (`ok` = no `fail`).
///
/// Levels: `fail` when nothing can run (harness2 not installed; uv, Python 3.12 or git
/// missing; gcloud missing while a profile uses Vertex; no container runtime under
/// container isolation), `warn` when only some benchmarks or features are affected, `skip`
/// when a check does not apply. Under container isolation the toolchain jobs use (uv, Python
/// 3.12, git, Node.js, npm, curl, Pandoc, Poppler) comes from the runner image, so those host
/// checks are skipped. The `dotenv` check looks for a `.env` file in the harness2
/// checkout (harness2 loads it for every variable a job does not set).
#[must_use]
pub fn evaluate(results: &[ProbeResult], ctx: &DoctorContext) -> DoctorReport {
    let mut c = Checks(Vec::new());
    let image = ctx.isolation == Isolation::Container;
    let h = &ctx.harness2;
    let short = |s: &str| s.chars().take(12).collect::<String>();
    if h.installed {
        let commit = h.commit.as_deref().unwrap_or("");
        let detail = format!(
            "harness2 {} at {}, Python {}",
            h.version.as_deref().unwrap_or("(unknown version)"),
            short(commit),
            h.python.as_deref().unwrap_or("(unknown)")
        );
        if commit == h.tested_commit {
            c.push(
                "harness2",
                "harness2",
                CheckLevel::Ok,
                detail,
                None,
                ALL_BENCHES,
            );
        } else {
            let remedy = if h.managed {
                "Reinstall harness2 at the tested commit (Settings → System → Install harness2)."
            } else {
                "Check out the tested commit in the configured harness2 checkout."
            };
            c.push(
                "harness2",
                "harness2",
                CheckLevel::Warn,
                format!(
                    "{detail}; this release was tested with {}",
                    short(&h.tested_commit)
                ),
                Some(remedy),
                ALL_BENCHES,
            );
        }
    } else {
        let remedy = if h.managed {
            "Install harness2 (Settings → System → Install harness2, an install_harness2 job)."
        } else {
            "Point paths.harness2_home at an installed harness2 checkout (with its .venv), or \
             set paths.harness2_managed = true."
        };
        c.push(
            "harness2",
            "harness2",
            CheckLevel::Fail,
            "harness2 is not installed",
            Some(remedy),
            ALL_BENCHES,
        );
    }
    if image {
        c.in_image("uv", "uv", &["install", "jb", "lab", "wb"]);
    } else {
        c.tool(
            results,
            "uv",
            "uv",
            CheckLevel::Fail,
            "uv is not installed; it creates harness2's Python 3.12 environment",
            "Install uv: https://docs.astral.sh/uv/getting-started/installation/",
            &["install", "jb", "lab", "wb"],
        );
    }
    let uv_ok = works(probe(results, "uv"));
    if image {
        c.in_image("python312", "Python 3.12", &["install"]);
    } else if !uv_ok {
        c.push(
            "python312",
            "Python 3.12",
            CheckLevel::Fail,
            "cannot look for Python 3.12 without uv",
            Some("Install uv first, then run: uv python install 3.12"),
            &["install"],
        );
    } else if works(probe(results, "python312")) {
        c.push(
            "python312",
            "Python 3.12",
            CheckLevel::Ok,
            "Python 3.12 is available to uv",
            None,
            &["install"],
        );
    } else {
        c.push(
            "python312",
            "Python 3.12",
            CheckLevel::Fail,
            "uv cannot find Python 3.12, which harness2's benchmarks require",
            Some("uv python install 3.12"),
            &["install"],
        );
    }
    if image {
        c.in_image("git", "git", &["install", "jb", "lab", "wb"]);
    } else {
        c.tool(
            results,
            "git",
            "git",
            CheckLevel::Fail,
            "git is not installed; it fetches harness2 and the benchmark checkouts",
            "Install git (Debian/Ubuntu: sudo apt-get install -y git).",
            &["install", "jb", "lab", "wb"],
        );
    }
    let node = probe(results, "node");
    let node_remedy = "Install Node.js 20 or newer (https://nodejs.org/en/download); \
                       harness2-setup uses npm to install Bun and the Codex CLI.";
    let node_needed = &["jb", "lab", "wb", "codex"];
    if image {
        c.in_image("node20", "Node.js 20+", node_needed);
    } else if works(node) {
        let line = version_line(node).unwrap_or_default();
        match major(&line) {
            Some(m) if m >= 20 => c.push(
                "node20",
                "Node.js 20+",
                CheckLevel::Ok,
                format!("Node.js {line}"),
                None,
                node_needed,
            ),
            Some(_) => c.push(
                "node20",
                "Node.js 20+",
                CheckLevel::Warn,
                format!("Node.js {line} is older than 20, which harness2-setup requires"),
                Some(node_remedy),
                node_needed,
            ),
            None => c.push(
                "node20",
                "Node.js 20+",
                CheckLevel::Warn,
                "could not read the Node.js version",
                Some(node_remedy),
                node_needed,
            ),
        }
    } else {
        c.push(
            "node20",
            "Node.js 20+",
            CheckLevel::Warn,
            "Node.js is not installed; benchmark setup needs it",
            Some(node_remedy),
            node_needed,
        );
    }
    if image {
        c.in_image("npm", "npm", node_needed);
        c.in_image("curl", "curl", &["wb"]);
    } else {
        c.tool(
            results,
            "npm",
            "npm",
            CheckLevel::Warn,
            "npm is not installed; benchmark setup needs it",
            "Install npm with Node.js 20 or newer (https://nodejs.org/en/download).",
            node_needed,
        );
        c.tool(
            results,
            "curl",
            "curl",
            CheckLevel::Warn,
            "curl is not installed; WorkBuddy setup downloads its datasets with it",
            "Install curl (Debian/Ubuntu: sudo apt-get install -y curl).",
            &["wb"],
        );
    }
    let docker = probe(results, "docker");
    if works(docker) {
        let v = version_line(docker).unwrap_or_default();
        c.push(
            "docker",
            "Docker",
            CheckLevel::Ok,
            format!("Docker daemon {v}"),
            None,
            &["wb", "container"],
        );
    } else if docker.is_some_and(|d| d.found) {
        c.push(
            "docker",
            "Docker",
            CheckLevel::Warn,
            "docker is installed but `docker info` failed (daemon not running, or no \
             permission for this user)",
            Some("Start the Docker daemon and allow this user to run `docker info`."),
            &["wb", "container"],
        );
    } else {
        c.push(
            "docker",
            "Docker",
            CheckLevel::Warn,
            "Docker is not installed; WorkBuddy-Bench runs its tasks in containers",
            Some("Install Docker Engine: https://docs.docker.com/engine/install/"),
            &["wb", "container"],
        );
    }
    let podman = probe(results, "podman");
    if works(podman) {
        let v = version_line(podman).unwrap_or_default();
        c.push(
            "podman",
            "Podman",
            CheckLevel::Ok,
            format!("Podman {v}"),
            None,
            &["container"],
        );
    } else {
        c.push(
            "podman",
            "Podman",
            CheckLevel::Skip,
            "Podman is not available (optional: an alternative runtime for container isolation)",
            None,
            &["container"],
        );
    }
    if image {
        c.in_image("pandoc", "Pandoc", &["lab"]);
    } else {
        c.tool(
            results,
            "pandoc",
            "Pandoc",
            CheckLevel::Warn,
            "Pandoc is not installed; LAB converts documents with it",
            "Install Pandoc (Debian/Ubuntu: sudo apt-get install -y pandoc).",
            &["lab"],
        );
    }
    let pdf = probe(results, "pdftoppm");
    let pdf_ok = pdf.is_some_and(|p| {
        p.found
            && (p.exit_code == Some(0) || format!("{}{}", p.stdout, p.stderr).contains("version"))
    });
    if image {
        c.in_image("pdftoppm", "Poppler (pdftoppm)", &["lab"]);
    } else if pdf_ok {
        let v = version_line(pdf).unwrap_or_else(|| "pdftoppm is available".into());
        c.push(
            "pdftoppm",
            "Poppler (pdftoppm)",
            CheckLevel::Ok,
            v,
            None,
            &["lab"],
        );
    } else {
        c.push(
            "pdftoppm",
            "Poppler (pdftoppm)",
            CheckLevel::Warn,
            "pdftoppm is not installed; LAB renders PDFs with it",
            Some("Install Poppler (Debian/Ubuntu: sudo apt-get install -y poppler-utils)."),
            &["lab"],
        );
    }
    if ctx.uses_vertex {
        c.tool(
            results,
            "gcloud",
            "Google Cloud CLI",
            CheckLevel::Fail,
            "the Google Cloud CLI is not installed; harness2-check runs `gcloud auth \
             application-default print-access-token` for Vertex AI profiles",
            "Install the Google Cloud CLI: https://cloud.google.com/sdk/docs/install",
            &["vertex"],
        );
        c.push(
            "adc",
            "Application Default Credentials",
            CheckLevel::Warn,
            "not verified here (checking prints an access token); harness2-check verifies them \
             before every run",
            Some(
                "Run `gcloud auth application-default login`, or give the profile a \
                 service-account key secret.",
            ),
            &["vertex"],
        );
    } else {
        c.push(
            "gcloud",
            "Google Cloud CLI",
            CheckLevel::Skip,
            "no model profile uses Vertex AI",
            None,
            &["vertex"],
        );
        c.push(
            "adc",
            "Application Default Credentials",
            CheckLevel::Skip,
            "no model profile uses Vertex AI",
            None,
            &["vertex"],
        );
    }
    if ctx.isolation == Isolation::Container {
        if works(docker) || works(podman) {
            let which = if works(docker) { "docker" } else { "podman" };
            c.push(
                "container_runtime",
                "Container runtime",
                CheckLevel::Ok,
                format!("{which} answers; jobs run in containers"),
                None,
                &["container"],
            );
        } else {
            c.push(
                "container_runtime",
                "Container runtime",
                CheckLevel::Fail,
                "container isolation is configured but neither docker nor podman answers",
                Some("Install and start Docker or Podman, or set isolation.mode = \"host\"."),
                &["container"],
            );
        }
    } else {
        c.push(
            "container_runtime",
            "Container runtime",
            CheckLevel::Skip,
            "jobs run on the host (isolation.mode = \"host\")",
            None,
            &["container"],
        );
    }
    match ctx.free_disk_bytes {
        None => c.push(
            "disk",
            "Free disk space",
            CheckLevel::Skip,
            "free space on the data directory is unknown",
            None,
            ALL_BENCHES,
        ),
        Some(b) => {
            let gib = b as f64 / (1024.0 * 1024.0 * 1024.0);
            if b < LOW_DISK_BYTES {
                c.push(
                    "disk",
                    "Free disk space",
                    CheckLevel::Warn,
                    format!("{gib:.1} GiB free on the data directory's filesystem"),
                    Some(
                        "Free up space: benchmark setup downloads several GB and every rollout \
                         keeps its outputs in the experiment directory.",
                    ),
                    ALL_BENCHES,
                );
            } else {
                c.push(
                    "disk",
                    "Free disk space",
                    CheckLevel::Ok,
                    format!("{gib:.0} GiB free"),
                    None,
                    ALL_BENCHES,
                );
            }
        }
    }
    let dotenv = h
        .home
        .as_deref()
        .map(|home| std::path::Path::new(home).join(".env"))
        .map(|p| std::fs::symlink_metadata(p).is_ok());
    match dotenv {
        Some(true) => c.push(
            "dotenv",
            "harness2 .env file",
            CheckLevel::Warn,
            "a .env file in the harness2 checkout supplies every variable a job does not set, \
             bypassing Workbench's profiles and secrets",
            Some("Move .env out of the harness2 checkout and put its values in a model profile."),
            ALL_BENCHES,
        ),
        Some(false) => c.push(
            "dotenv",
            "harness2 .env file",
            CheckLevel::Ok,
            "no .env file in the harness2 checkout",
            None,
            ALL_BENCHES,
        ),
        None => c.push(
            "dotenv",
            "harness2 .env file",
            CheckLevel::Skip,
            "the harness2 checkout location is unknown",
            None,
            ALL_BENCHES,
        ),
    }
    let ok = !c.0.iter().any(|x| x.level == CheckLevel::Fail);
    DoctorReport {
        checks: c.0,
        ok,
        generated_at: ctx.generated_at.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str, found: bool, code: i32, out: &str) -> ProbeResult {
        ProbeResult {
            id: id.into(),
            found,
            exit_code: Some(code),
            stdout: out.into(),
            stderr: String::new(),
        }
    }

    fn status(installed: bool, home: Option<String>) -> Harness2Status {
        Harness2Status {
            installed,
            managed: true,
            commit: installed.then(|| crate::TESTED_HARNESS2_COMMIT.to_owned()),
            tested_commit: crate::TESTED_HARNESS2_COMMIT.into(),
            version: Some("0.1.0".into()),
            python: Some("3.12.14".into()),
            extras: vec![],
            home,
        }
    }

    fn level(rep: &DoctorReport, id: &str) -> CheckLevel {
        rep.checks
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.level)
            .unwrap()
    }

    #[test]
    fn healthy_host() {
        let tmp = tempfile::tempdir().unwrap();
        let results = vec![
            r("uv", true, 0, "uv 0.11.25 (x86_64-unknown-linux-gnu)\n"),
            r(
                "python312",
                true,
                0,
                "/opt/uv/python/cpython-3.12/bin/python3.12\n",
            ),
            r("git", true, 0, "git version 2.43.0\n"),
            r("node", true, 0, "v22.3.0\n"),
            r("npm", true, 0, "10.8.1\n"),
            r("docker", true, 1, ""),
            r("podman", false, 127, ""),
            r("pandoc", true, 0, "pandoc 3.1.3\nFeatures: +server\n"),
            ProbeResult {
                id: "pdftoppm".into(),
                found: true,
                exit_code: Some(99),
                stdout: String::new(),
                stderr: "pdftoppm version 24.02.0\n".into(),
            },
            r("curl", true, 0, "curl 8.5.0\n"),
        ];
        let ctx = DoctorContext {
            harness2: status(true, Some(tmp.path().to_string_lossy().into_owned())),
            isolation: Isolation::Host,
            free_disk_bytes: Some(10 * 1024 * 1024 * 1024),
            uses_vertex: false,
            generated_at: "2026-10-23T00:00:00Z".into(),
        };
        let rep = evaluate(&results, &ctx);
        assert!(rep.ok, "{:?}", rep.checks);
        assert_eq!(level(&rep, "harness2"), CheckLevel::Ok);
        assert_eq!(level(&rep, "node20"), CheckLevel::Ok);
        assert_eq!(level(&rep, "docker"), CheckLevel::Warn);
        assert_eq!(level(&rep, "pdftoppm"), CheckLevel::Ok);
        assert_eq!(level(&rep, "gcloud"), CheckLevel::Skip);
        assert_eq!(level(&rep, "container_runtime"), CheckLevel::Skip);
        assert_eq!(level(&rep, "disk"), CheckLevel::Warn);
        assert_eq!(level(&rep, "dotenv"), CheckLevel::Ok);
        // No absolute path leaks into any detail.
        assert!(rep.checks.iter().all(|c| !c.detail.contains("/opt/")));
        std::fs::write(tmp.path().join(".env"), "X=1\n").unwrap();
        let rep = evaluate(&results, &ctx);
        assert_eq!(level(&rep, "dotenv"), CheckLevel::Warn);
    }

    #[test]
    fn failures_and_versions() {
        let results = vec![
            r("node", true, 0, "v18.19.0\n"),
            r("gcloud", false, 127, ""),
        ];
        let mut ctx = DoctorContext {
            harness2: status(false, None),
            isolation: Isolation::Host,
            free_disk_bytes: None,
            uses_vertex: true,
            generated_at: "t".into(),
        };
        let rep = evaluate(&results, &ctx);
        assert!(!rep.ok);
        for id in ["harness2", "uv", "python312", "git", "gcloud"] {
            assert_eq!(level(&rep, id), CheckLevel::Fail, "{id}");
        }
        assert_eq!(level(&rep, "node20"), CheckLevel::Warn);
        assert_eq!(level(&rep, "adc"), CheckLevel::Warn);
        assert_eq!(level(&rep, "dotenv"), CheckLevel::Skip);
        assert_eq!(level(&rep, "container_runtime"), CheckLevel::Skip);
        assert!(rep.checks.iter().all(|c| c.level == CheckLevel::Ok
            || c.level == CheckLevel::Skip
            || c.remedy.is_some()));
        // Under container isolation a missing runtime fails; the host toolchain does not count.
        ctx.isolation = Isolation::Container;
        let rep = evaluate(&results, &ctx);
        assert!(!rep.ok);
        assert_eq!(level(&rep, "container_runtime"), CheckLevel::Fail);
        assert_eq!(level(&rep, "uv"), CheckLevel::Skip);
        assert_eq!(major("v20.0.1"), Some(20));
        assert_eq!(major("garbage"), None);
    }

    #[test]
    fn container_isolation_skips_the_host_toolchain() {
        // A host with Docker and nothing else: uv, Python, git, Node, npm, curl, Pandoc and
        // Poppler are all missing here, and none of it matters, because jobs run in the image.
        let results = vec![
            r("docker", true, 0, "29.6.0\n"),
            r("podman", false, 127, ""),
        ];
        let ctx = DoctorContext {
            harness2: status(true, None),
            isolation: Isolation::Container,
            free_disk_bytes: Some(100 * 1024 * 1024 * 1024),
            uses_vertex: false,
            generated_at: "t".into(),
        };
        let rep = evaluate(&results, &ctx);
        assert!(rep.ok, "{:?}", rep.checks);
        for id in [
            "uv",
            "python312",
            "git",
            "node20",
            "npm",
            "curl",
            "pandoc",
            "pdftoppm",
        ] {
            assert_eq!(level(&rep, id), CheckLevel::Skip, "{id}");
            let check = rep.checks.iter().find(|c| c.id == id).unwrap();
            assert!(check.detail.starts_with("container isolation:"), "{id}");
            assert!(check.remedy.is_none(), "{id}");
        }
        assert_eq!(level(&rep, "docker"), CheckLevel::Ok);
        assert_eq!(level(&rep, "container_runtime"), CheckLevel::Ok);
        // The same host under host isolation fails: there the host's toolchain is what runs.
        let host = DoctorContext {
            isolation: Isolation::Host,
            ..ctx
        };
        let rep = evaluate(&results, &host);
        assert!(!rep.ok);
        assert_eq!(level(&rep, "uv"), CheckLevel::Fail);
        assert_eq!(level(&rep, "pandoc"), CheckLevel::Warn);
    }
}
