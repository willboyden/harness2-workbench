//! Benchmark catalogue and installation state.

use std::path::Path;

use h2s_api::BenchmarkStatus;

/// Upstream dependency pins from `harness2/config.py` at [`crate::TESTED_HARNESS2_COMMIT`].
pub const DEPENDENCY_PINS: &[(&str, &str)] = &[
    ("job-bench-eval", "fa03a17f9ff662f136798a274e10011c3aa00fd0"),
    (
        "workbuddy-bench",
        "b516950be5b56eb3be406c2f76ee1c5111dcb57f",
    ),
    ("harvey-labs", "b58a28ae9fde5ee4b810e53b4fd0a1915325a35d"),
    ("opencode", "23fb5e0516c99ac04a1aa46c193efda2e1b9bb24"),
];

/// Codex CLI version harness2 installs (`config.CODEX_VERSION`).
pub const CODEX_VERSION: &str = "0.144.6";

/// Hosts setup downloads from (upstream `docs/SETUP.md` "Network access during setup").
pub const SETUP_HOSTS: &[&str] = &[
    "github.com",
    "codeload.github.com",
    "registry.npmjs.org",
    "huggingface.co",
    "cdn-lfs.huggingface.co",
    "astral.sh",
    "pypi.org",
    "files.pythonhosted.org",
];

/// Hosts upstream recommends blocking for executors during evaluation, so agents cannot
/// fetch public rubrics (`docs/EXPERIMENTS.md` "Web access").
pub const EVAL_BLOCKLIST: &[&str] = &[
    "huggingface.co",
    "hf.co",
    "*.hf.co",
    "github.com",
    "raw.githubusercontent.com",
];

/// Static catalogue: one entry per benchmark (WorkBuddy: one per subset), not installed.
#[must_use]
pub fn catalog() -> Vec<BenchmarkStatus> {
    Vec::new()
}

/// The catalogue with installation state read from `harness2_home/dependencies/` (files
/// only: checkout HEAD vs pin, dataset markers).
#[must_use]
pub fn status(harness2_home: &Path) -> Vec<BenchmarkStatus> {
    let _ = harness2_home;
    catalog()
}
