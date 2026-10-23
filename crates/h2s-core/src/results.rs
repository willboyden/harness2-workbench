//! A read-only, traversal-proof view of one experiment directory (one `HARNESS2_RUNS_DIR`).
//!
//! All access goes through a `cap_std::fs::Dir` opened at the experiment root, so neither a
//! request parameter nor a path recorded inside a results file (harness2 records absolute
//! `run_dir`s) can reach outside it. Symlinks are listed but never followed. Absolute paths
//! found in results files are mapped back under the root by stripping a known prefix: the
//! experiment's host path, or [`crate::isolation::EXPERIMENT_MOUNT`] for jobs that ran in a
//! container; anything else is refused.

use std::collections::BTreeMap;
use std::path::Path;

use h2s_api::{
    AuditSession, Comparison, ExperimentKind, FileContent, FileEntry, HarnessDiff, HarnessTree,
    LibraryDomain, Mode, ScoreReport, Scores, SweepIdentity, SweepSummary, TaskDetail, TaskRow,
};

use crate::namespace::Namespace;
use crate::{CoreError, Result};

/// Default cap for [`ExperimentDir::rollout_file`] and audit text reads.
pub const DEFAULT_READ_CAP: u64 = 2 * 1024 * 1024;

/// An open experiment directory.
pub struct ExperimentDir {
    dir: cap_std::fs::Dir,
    kind: ExperimentKind,
    /// Absolute path prefixes that refer to the root inside results files.
    roots: Vec<String>,
}

impl std::fmt::Debug for ExperimentDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExperimentDir")
            .field("kind", &self.kind)
            .field("roots", &self.roots)
            .finish()
    }
}

impl ExperimentDir {
    /// Open `path` (must be an existing directory). `extra_roots` lists other absolute
    /// prefixes results files may use for this root (the container mount point).
    pub fn open(path: &Path, kind: ExperimentKind, extra_roots: &[String]) -> Result<Self> {
        let dir = cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
        let mut roots = vec![path.to_string_lossy().into_owned()];
        roots.extend(extra_roots.iter().cloned());
        Ok(ExperimentDir { dir, kind, roots })
    }

    #[must_use]
    pub fn kind(&self) -> ExperimentKind {
        self.kind
    }

    /// The underlying capability handle (for modules in this crate).
    #[must_use]
    pub fn dir(&self) -> &cap_std::fs::Dir {
        &self.dir
    }

    /// Absolute prefixes recognised as this root.
    #[must_use]
    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    /// Every sweep on disk, newest first. Unparseable directory names are skipped.
    pub fn sweeps(&self) -> Result<Vec<SweepSummary>> {
        Err(CoreError::Unimplemented("results::sweeps"))
    }

    /// Task rows of one sweep (`sweep` is the summary namespace), in stream order when a
    /// sweep record exists, else sorted by task id.
    pub fn tasks(&self, sweep: &Namespace) -> Result<Vec<TaskRow>> {
        let _ = sweep;
        Err(CoreError::Unimplemented("results::tasks"))
    }

    /// Full detail of one task.
    pub fn task(&self, sweep: &Namespace, slug: &str) -> Result<TaskDetail> {
        let _ = (sweep, slug);
        Err(CoreError::Unimplemented("results::task"))
    }

    /// One harness tree with file contents.
    pub fn harness(
        &self,
        sweep: &Namespace,
        slug: &str,
        mode: Mode,
        label: &str,
    ) -> Result<HarnessTree> {
        let _ = (sweep, slug, mode, label);
        Err(CoreError::Unimplemented("results::harness"))
    }

    /// Diff between two harness labels of the same task and mode.
    pub fn diff(
        &self,
        sweep: &Namespace,
        slug: &str,
        mode: Mode,
        from: &str,
        to: &str,
    ) -> Result<HarnessDiff> {
        let _ = (sweep, slug, mode, from, to);
        Err(CoreError::Unimplemented("results::diff"))
    }

    /// List `sub` (relative path, empty = root) of the rollout run directory of `label`.
    pub fn rollout_files(
        &self,
        sweep: &Namespace,
        slug: &str,
        mode: Mode,
        label: &str,
        sub: &str,
    ) -> Result<Vec<FileEntry>> {
        let _ = (sweep, slug, mode, label, sub);
        Err(CoreError::Unimplemented("results::rollout_files"))
    }

    /// Read one file of a rollout run directory, at most `max_bytes` of it.
    pub fn rollout_file(
        &self,
        sweep: &Namespace,
        slug: &str,
        mode: Mode,
        label: &str,
        sub: &str,
        max_bytes: u64,
    ) -> Result<FileContent> {
        let _ = (sweep, slug, mode, label, sub, max_bytes);
        Err(CoreError::Unimplemented("results::rollout_file"))
    }

    /// `judged.json` and `base_judged.json` of the sweep (empty when not judged yet).
    pub fn scores(&self, sweep: &Namespace) -> Result<Scores> {
        let _ = sweep;
        Err(CoreError::Unimplemented("results::scores"))
    }

    /// The score report computed from the cached rows (port of `harness2-report`).
    pub fn report(&self, sweep: &Namespace) -> Result<ScoreReport> {
        let _ = sweep;
        Err(CoreError::Unimplemented("results::report"))
    }

    /// Experience libraries of the sweep, one per (mode, domain).
    pub fn library(&self, sweep: &Namespace) -> Result<Vec<LibraryDomain>> {
        let _ = sweep;
        Err(CoreError::Unimplemented("results::library"))
    }

    /// Improver/selector sessions recorded under `adapt/_audit/<sweep>/`.
    pub fn audit(&self, sweep: &Namespace) -> Result<Vec<AuditSession>> {
        let _ = sweep;
        Err(CoreError::Unimplemented("results::audit"))
    }

    /// One text part (`mission`, `system`, `response`) of an audit session.
    pub fn audit_text(
        &self,
        sweep: &Namespace,
        id: &str,
        part: &str,
        max_bytes: u64,
    ) -> Result<String> {
        let _ = (sweep, id, part, max_bytes);
        Err(CoreError::Unimplemented("results::audit_text"))
    }

    /// Spend recorded in the sweep's summaries so far (stages + selections), USD. Used by
    /// the budget guard while a run is in progress; tolerant of half-written files.
    pub fn spend_usd(&self, sweep: &Namespace) -> Result<f64> {
        let _ = sweep;
        Err(CoreError::Unimplemented("results::spend_usd"))
    }

    /// `config.json` identities of every sweep (for the manifest).
    pub fn sweep_identities(&self) -> Result<Vec<SweepIdentity>> {
        Err(CoreError::Unimplemented("results::sweep_identities"))
    }

    /// SHA-256 (hex) over every `summary.json`, `judged.json` and `base_judged.json`,
    /// in sorted path order, each as `path\0bytes\0`.
    pub fn results_digest(&self) -> Result<String> {
        Err(CoreError::Unimplemented("results::results_digest"))
    }

    /// Total size of regular files under the root (symlinks not followed).
    pub fn size_bytes(&self) -> Result<u64> {
        Err(CoreError::Unimplemented("results::size_bytes"))
    }
}

/// Per-task deltas between two score sets. `a_mode`/`b_mode` pick the rows of each side;
/// `domains` maps task id to domain for display.
#[must_use]
pub fn compare(
    a_label: &str,
    a: &Scores,
    a_mode: Mode,
    b_label: &str,
    b: &Scores,
    b_mode: Mode,
    domains: &BTreeMap<String, String>,
) -> Comparison {
    let _ = (a, a_mode, b, b_mode, domains);
    Comparison {
        a: a_label.to_owned(),
        b: b_label.to_owned(),
        tasks: Vec::new(),
        n_common: 0,
        mean_a: None,
        mean_b: None,
        mean_delta: None,
        wins: 0,
        losses: 0,
        ties: 0,
        sign_test_p: None,
    }
}
