//! Typed results from harness2's output.
//!
//! Every parser is total: unexpected input yields `None`/defaults or
//! [`CoreError::Malformed`], never a panic, because the text comes from a subprocess.
//! Formats are those of harness2 at [`crate::TESTED_HARNESS2_COMMIT`]; tests use captured
//! real output under `tests/fixtures/`.

use h2s_api::{
    CheckResult, DemoResult, DomainCount, JudgeResult, ModeNamespace, RunResult, SetupResult,
    StreamEntry,
};

use crate::{CoreError, Result};

/// `harness2-check` stdout: `FAIL: ...` lines, then `Setup incomplete: ...` or
/// `Ready: <bench>/<sub>; N tasks. ...`.
#[must_use]
pub fn check_output(stdout: &str) -> CheckResult {
    let _ = stdout;
    CheckResult {
        ready: false,
        tasks: None,
        failures: Vec::new(),
    }
}

/// Parsed `harness2 --dry-run` stdout (the server adds the cost estimate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub header: String,
    pub namespaces: Vec<ModeNamespace>,
    pub summary_namespace: String,
    pub library_root: Option<String>,
    pub workers: u32,
    pub stream: Vec<StreamEntry>,
    pub domains: Vec<DomainCount>,
    pub total_tasks: u32,
    pub total_domains: u32,
}

/// Parse `harness2 --dry-run` stdout (see upstream `cli/run_sweep.py::_print_stream`).
pub fn preview_output(stdout: &str) -> Result<Preview> {
    let _ = stdout;
    Err(CoreError::Unimplemented("parse::preview_output"))
}

/// One `  [<domain>] i/n <status> <task>[  :: <error>]` progress line from a sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProgress {
    pub domain: String,
    pub index: u32,
    pub total: u32,
    /// `ok`, `skipped`, `partial`, `base-failed`, `exception`, ...
    pub status: String,
    pub task: String,
    pub error: Option<String>,
}

#[must_use]
pub fn progress_line(line: &str) -> Option<TaskProgress> {
    let _ = line;
    None
}

/// The tail of a sweep: `sweep <ns>: X tasks ok, Y failed, Z domain(s) died[: a, b]`,
/// `record: <path>`, `next:   <command>`. Also recognises the zero-tasks refusal.
#[must_use]
pub fn run_output(stdout: &str) -> Option<RunResult> {
    let _ = stdout;
    None
}

/// The tail of `harness2-judge`: `judged.json: N row(s), G graded, U ungraded (...)` or,
/// with `--bases`, `wrote <path>: N row(s), G graded`.
#[must_use]
pub fn judge_output(stdout: &str, bases: bool) -> Option<JudgeResult> {
    let _ = (stdout, bases);
    None
}

/// `demo.json` written by `harness2-demo` (`done`, `failed`, `execution_count`,
/// `editor_calls`).
pub fn demo_json(text: &str) -> Result<DemoResult> {
    let _ = text;
    Err(CoreError::Unimplemented("parse::demo_json"))
}

/// `harness2-setup` stdout: each printed command (`[cwd] cmd ...` or `cmd ...`) becomes a
/// plan line; `Note: ...` and `Configure WorkBuddy ...` lines become notes.
#[must_use]
pub fn setup_output(stdout: &str, dry_run: bool) -> SetupResult {
    let _ = stdout;
    SetupResult {
        dry_run,
        plan: Vec::new(),
        notes: Vec::new(),
    }
}

/// The split file `scripts/split_tasks.py` wrote: `(train, test)` counts.
pub fn split_file(text: &str) -> Result<(u32, u32)> {
    let _ = text;
    Err(CoreError::Unimplemented("parse::split_file"))
}

/// Settings `harness2-bridge` prints before it starts serving. **Contains the bridge's
/// API key**: the server must store it as a secret and add it to the job's redactor
/// before the line reaches any log. `Debug` must not print the key.
#[derive(Clone, PartialEq, Eq)]
pub struct BridgeSettings {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub container_url: Option<String>,
}

impl std::fmt::Debug for BridgeSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeSettings")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("container_url", &self.container_url)
            .finish()
    }
}

/// Parse one stdout line of `harness2-bridge` into the settings accumulated so far.
/// Returns `true` when the line carried a setting (so the caller can keep it out of logs).
pub fn bridge_line(line: &str, acc: &mut BridgeSettings) -> bool {
    let _ = (line, acc);
    false
}
