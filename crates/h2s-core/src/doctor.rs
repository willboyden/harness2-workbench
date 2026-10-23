//! Environment checks: which probes to run, and what their results mean.
//!
//! The server runs each [`Probe`] (no shell, 10 s timeout) and hands the results to
//! [`evaluate`], which knows harness2's requirements: Python 3.12 through uv, Node >= 20
//! and npm (setup), git, Docker (WorkBuddy), Pandoc + Poppler (LAB), the Google Cloud CLI
//! and Application Default Credentials (Vertex profiles), and the container runtime and
//! runner image (container isolation).

use h2s_api::{DoctorReport, Harness2Status, Isolation};

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

/// Turn probe results into checks with remedies (`ok` = no `fail`).
#[must_use]
pub fn evaluate(results: &[ProbeResult], ctx: &DoctorContext) -> DoctorReport {
    let _ = results;
    DoctorReport {
        checks: Vec::new(),
        ok: false,
        generated_at: ctx.generated_at.clone(),
    }
}
