//! harness2 vocabulary: benchmarks, executors ("substrates"), recursion modes, task splits.
//!
//! Every enum here maps 1:1 onto a harness2 CLI choice; `as_cli()` returns the exact token
//! the upstream CLI accepts, so the server never builds argv from free text.

use serde::{Deserialize, Serialize};

/// Bounds the server enforces on run identity knobs. harness2 itself only requires k >= 1;
/// the upper bounds stop a typo from launching a sweep that costs orders of magnitude more
/// than intended (R = 4k rollouts and P = 3k improver sessions per task and mode).
pub const K_MAX: u32 = 32;
pub const WORKERS_MAX: u32 = 64;
pub const MAX_TASKS_MAX: u32 = 100_000;

/// A benchmark harness2 can install and run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Bench {
    /// JobBench: professional work tasks, grouped by occupation.
    Jb,
    /// LAB (Harvey Labs): legal work, grouped by practice area. Needs Pandoc + Poppler.
    Lab,
    /// WorkBuddy-Bench: office, web and code subsets. Needs Docker. Custom Tencent license.
    Wb,
}

impl Bench {
    pub const ALL: [Bench; 3] = [Bench::Jb, Bench::Lab, Bench::Wb];

    /// Token accepted by `--bench` / the `harness2-setup` positional argument.
    #[must_use]
    pub fn as_cli(self) -> &'static str {
        match self {
            Bench::Jb => "jb",
            Bench::Lab => "lab",
            Bench::Wb => "wb",
        }
    }

    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Bench::Jb => "JobBench",
            Bench::Lab => "LAB",
            Bench::Wb => "WorkBuddy-Bench",
        }
    }

    /// Name of the upstream checkout under harness2's `dependencies/`.
    #[must_use]
    pub fn upstream_checkout(self) -> &'static str {
        match self {
            Bench::Jb => "job-bench-eval",
            Bench::Lab => "harvey-labs",
            Bench::Wb => "workbuddy-bench",
        }
    }

    /// What harness2 calls one grouping unit of this benchmark's tasks.
    #[must_use]
    pub fn domain_word(self) -> &'static str {
        match self {
            Bench::Jb => "occupation",
            Bench::Lab => "practice area",
            Bench::Wb => "category",
        }
    }
}

/// WorkBuddy-Bench subset. Only valid with [`Bench::Wb`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum WbSubset {
    Office,
    Web,
    Code,
}

impl WbSubset {
    pub const ALL: [WbSubset; 3] = [WbSubset::Office, WbSubset::Web, WbSubset::Code];

    #[must_use]
    pub fn as_cli(self) -> &'static str {
        match self {
            WbSubset::Office => "office",
            WbSubset::Web => "web",
            WbSubset::Code => "code",
        }
    }
}

/// The agent framework that executes task rollouts ("substrate" upstream).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Substrate {
    /// OpenCode (upstream default), run from a pinned source checkout through Bun.
    Oc,
    /// OpenAI Codex CLI against a Responses-API endpoint. Upstream runs it with its sandbox
    /// disabled.
    Cx,
}

impl Substrate {
    #[must_use]
    pub fn as_cli(self) -> &'static str {
        match self {
            Substrate::Oc => "oc",
            Substrate::Cx => "cx",
        }
    }

    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Substrate::Oc => "OpenCode",
            Substrate::Cx => "Codex",
        }
    }
}

/// Task-level recursion mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Width: probe k general + k domain candidates, then k independent compositions.
    Parallel,
    /// Depth: k successive Propose–Probe–Compose steps carrying the harness forward.
    Sequential,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Parallel, Mode::Sequential];

    /// Value for `--mode` / `--modes`.
    #[must_use]
    pub fn as_cli(self) -> &'static str {
        match self {
            Mode::Parallel => "parallel",
            Mode::Sequential => "sequential",
        }
    }

    /// The on-disk token harness2 uses in namespaces and summaries (`agg` / `seq`).
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Mode::Parallel => "agg",
            Mode::Sequential => "seq",
        }
    }

    /// Parse an on-disk token (`agg`/`seq`) or a CLI name (`parallel`/`sequential`).
    #[must_use]
    pub fn from_token(s: &str) -> Option<Mode> {
        match s {
            "agg" | "parallel" => Some(Mode::Parallel),
            "seq" | "sequential" => Some(Mode::Sequential),
            _ => None,
        }
    }
}

/// One side of a comparison (`GET /compare?a=<experiment>/<sweep>/<side>`): the harnesses a
/// recursion mode adopted, or the starting harness, through its reference rollouts as
/// `harness2-judge --bases` graded them (`base`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompareSide {
    Mode(Mode),
    Base,
}

impl CompareSide {
    /// Parse `base` or a mode (`parallel`/`agg`, `sequential`/`seq`).
    #[must_use]
    pub fn parse(s: &str) -> Option<CompareSide> {
        if s == "base" {
            Some(CompareSide::Base)
        } else {
            Mode::from_token(s).map(CompareSide::Mode)
        }
    }

    /// The canonical spelling: `base`, `parallel` or `sequential`.
    #[must_use]
    pub fn as_param(self) -> &'static str {
        match self {
            CompareSide::Mode(m) => m.as_cli(),
            CompareSide::Base => "base",
        }
    }
}

/// Which part of the task stream to run (`--split`). `train`/`test` need a split file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Split {
    #[default]
    All,
    Train,
    Test,
}

impl Split {
    #[must_use]
    pub fn as_cli(self) -> &'static str {
        match self {
            Split::All => "all",
            Split::Train => "train",
            Split::Test => "test",
        }
    }
}

/// `harness2-demo --mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum DemoMode {
    Parallel,
    Sequential,
    #[default]
    Both,
}

impl DemoMode {
    #[must_use]
    pub fn as_cli(self) -> &'static str {
        match self {
            DemoMode::Parallel => "parallel",
            DemoMode::Sequential => "sequential",
            DemoMode::Both => "both",
        }
    }
}

/// One (benchmark, subset, executor) combination: the "cell" every harness2 command takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Cell {
    pub bench: Bench,
    /// Required for `wb`, forbidden otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subset: Option<WbSubset>,
    /// Executor framework (`oc` OpenCode or `cx` Codex).
    #[serde(default = "default_substrate")]
    pub substrate: Substrate,
}

fn default_substrate() -> Substrate {
    Substrate::Oc
}

impl Cell {
    /// Mirrors upstream `make_adapter`: wb needs a subset, the others must not have one.
    ///
    /// # Errors
    /// A user-facing sentence naming the problem.
    pub fn validate(&self) -> Result<(), String> {
        match (self.bench, self.subset) {
            (Bench::Wb, None) => Err("WorkBuddy-Bench needs a subset: office, web or code".into()),
            (b, Some(_)) if b != Bench::Wb => Err(format!(
                "subsets apply to WorkBuddy-Bench only, not {}",
                b.as_cli()
            )),
            _ => Ok(()),
        }
    }

    /// The benchmark's own name in namespaces: `jb`, `lab`, `wb-office`, `wb-web`, `wb-code`.
    #[must_use]
    pub fn bench_name(&self) -> String {
        match (self.bench, self.subset) {
            (Bench::Wb, Some(s)) => format!("wb-{}", s.as_cli()),
            (b, _) => b.as_cli().to_owned(),
        }
    }
}

/// The run-identity and run-control options of `harness2` (the refinement sweep).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunOptions {
    /// Task-level recursion budget (`--k`, 1 to 32). Per task and mode: 4k rollouts and
    /// 3k improver sessions.
    pub k: u32,
    /// Stream-order seed (`--seed`). Shuffles task order reproducibly; does not control
    /// model sampling. `None` = upstream's sorted reference order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Recursion modes to run (`--modes`), non-empty, no duplicates.
    pub modes: Vec<Mode>,
    /// Which partition to run (`train`/`test` need `split_file`).
    #[serde(default)]
    pub split: Split,
    /// Name of a split file managed by the server (see `GET /api/v1/splits`). Required for
    /// `train`/`test`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_file: Option<String>,
    /// Domains run concurrently (`--workers`, 1 to 64). Tasks within a domain stay
    /// ordered because each reads the library entries of the ones before it.
    #[serde(default = "default_workers")]
    pub workers: u32,
    /// Restrict to these domains (`--only-domains`). Empty = all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only_domains: Vec<String>,
    /// Per-domain task cap (`--max-tasks`): the first N tasks of every domain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tasks: Option<u32>,
}

fn default_workers() -> u32 {
    1
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            k: 1,
            seed: None,
            modes: vec![Mode::Parallel],
            split: Split::All,
            split_file: None,
            workers: 1,
            only_domains: Vec::new(),
            max_tasks: None,
        }
    }
}

impl RunOptions {
    /// Structural checks only (bounds, duplicates). The server additionally checks domain
    /// names and split files against what is installed.
    ///
    /// # Errors
    /// Every problem found, as `(field, problem)` pairs.
    pub fn validate(&self) -> Result<(), Vec<(String, String)>> {
        let mut errs = Vec::new();
        if self.k == 0 || self.k > K_MAX {
            errs.push(("k".into(), format!("must be between 1 and {K_MAX}")));
        }
        if self.modes.is_empty() {
            errs.push(("modes".into(), "choose at least one recursion mode".into()));
        }
        let mut seen = Vec::new();
        for m in &self.modes {
            if seen.contains(m) {
                errs.push(("modes".into(), format!("{} listed twice", m.as_cli())));
            }
            seen.push(*m);
        }
        if self.workers == 0 || self.workers > WORKERS_MAX {
            errs.push((
                "workers".into(),
                format!("must be between 1 and {WORKERS_MAX}"),
            ));
        }
        if let Some(n) = self.max_tasks
            && (n == 0 || n > MAX_TASKS_MAX)
        {
            errs.push((
                "max_tasks".into(),
                format!("must be between 1 and {MAX_TASKS_MAX}"),
            ));
        }
        if matches!(self.split, Split::Train | Split::Test) && self.split_file.is_none() {
            errs.push((
                "split_file".into(),
                "train/test splits need a split file".into(),
            ));
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    /// Comma list for `--modes`, in upstream's canonical order (parallel first).
    #[must_use]
    pub fn modes_cli(&self) -> String {
        Mode::ALL
            .iter()
            .filter(|m| self.modes.contains(m))
            .map(|m| m.as_cli())
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_rules_match_upstream() {
        let wb = Cell {
            bench: Bench::Wb,
            subset: None,
            substrate: Substrate::Oc,
        };
        assert!(wb.validate().is_err());
        let lab_sub = Cell {
            bench: Bench::Lab,
            subset: Some(WbSubset::Web),
            substrate: Substrate::Oc,
        };
        assert!(lab_sub.validate().is_err());
        let ok = Cell {
            bench: Bench::Wb,
            subset: Some(WbSubset::Code),
            substrate: Substrate::Cx,
        };
        assert!(ok.validate().is_ok());
        assert_eq!(ok.bench_name(), "wb-code");
        let lab = Cell {
            bench: Bench::Lab,
            subset: None,
            substrate: Substrate::Oc,
        };
        assert_eq!(lab.bench_name(), "lab");
    }

    #[test]
    fn mode_tokens_round_trip() {
        for m in Mode::ALL {
            assert_eq!(Mode::from_token(m.token()), Some(m));
            assert_eq!(Mode::from_token(m.as_cli()), Some(m));
        }
        assert_eq!(Mode::from_token("both"), None);
    }

    #[test]
    fn run_options_validation() {
        assert!(RunOptions::default().validate().is_ok());
        let bad = RunOptions {
            k: 0,
            modes: vec![Mode::Parallel, Mode::Parallel],
            workers: 0,
            max_tasks: Some(0),
            split: Split::Train,
            ..RunOptions::default()
        };
        let errs = bad.validate().unwrap_err();
        let fields: Vec<_> = errs.iter().map(|(f, _)| f.as_str()).collect();
        for f in ["k", "modes", "workers", "max_tasks", "split_file"] {
            assert!(fields.contains(&f), "missing {f} in {fields:?}");
        }
    }

    #[test]
    fn modes_cli_is_canonical_order() {
        let o = RunOptions {
            modes: vec![Mode::Sequential, Mode::Parallel],
            ..RunOptions::default()
        };
        assert_eq!(o.modes_cli(), "parallel,sequential");
    }

    #[test]
    fn cell_defaults_substrate() {
        let c: Cell = serde_json::from_str(r#"{"bench":"lab"}"#).unwrap();
        assert_eq!(c.substrate, Substrate::Oc);
    }
}
