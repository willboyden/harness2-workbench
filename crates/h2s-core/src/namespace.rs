//! harness2 run namespaces: `<bench>-<substrate>-<mode><k>[-s<seed>][-<tag>]`.
//!
//! Upstream builds them in `core.schedule.ns()`. The *summary namespace* of a sweep is the
//! parallel-mode one (`agg` token): every task's `summary.json` and the sweep's
//! `judged.json` live there for both modes.

use std::fmt;

use h2s_api::{Cell, ExperimentKind, Mode, Substrate};

/// A parsed namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Namespace {
    /// `jb`, `lab`, `wb-office`, `wb-web`, `wb-code` or `demo`.
    pub bench_name: String,
    pub substrate: Substrate,
    pub mode: Mode,
    pub k: u32,
    pub seed: Option<u64>,
    pub tag: Option<String>,
}

impl Namespace {
    /// Parse a namespace directory name. `None` for anything else (`_sweeps`, `_audit`,
    /// stray files). Must round-trip with `Display` for every value it accepts.
    #[must_use]
    pub fn parse(s: &str) -> Option<Namespace> {
        let _ = s;
        None
    }

    /// The namespace a sweep of `cell` writes for `mode`.
    #[must_use]
    pub fn for_cell(cell: &Cell, mode: Mode, k: u32, seed: Option<u64>) -> Namespace {
        Namespace {
            bench_name: cell.bench_name(),
            substrate: cell.substrate,
            mode,
            k,
            seed,
            tag: None,
        }
    }

    /// The same namespace with another mode.
    #[must_use]
    pub fn with_mode(&self, mode: Mode) -> Namespace {
        Namespace {
            mode,
            ..self.clone()
        }
    }

    /// The summary namespace (parallel-mode token) of this namespace's sweep.
    #[must_use]
    pub fn summary(&self) -> Namespace {
        self.with_mode(Mode::Parallel)
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}-{}{}",
            self.bench_name,
            self.substrate.as_cli(),
            self.mode.token(),
            self.k
        )?;
        if let Some(seed) = self.seed {
            write!(f, "-s{seed}")?;
        }
        if let Some(tag) = &self.tag {
            write!(f, "-{tag}")?;
        }
        Ok(())
    }
}

/// Directory, relative to the experiment root, that holds the namespaces of `bench_name`:
/// `lab/adapt`, `jb/adapt`, `wb/office/adapt`, or `runs` for a demo experiment.
/// `None` for an unknown bench name.
#[must_use]
pub fn namespaces_root(kind: ExperimentKind, bench_name: &str) -> Option<String> {
    let _ = (kind, bench_name);
    None
}

/// Library root of a sweep, relative to the experiment root:
/// standard `lab/library/lab/k1-oc[-s7]`, demo `library/k2`. Mode and domain directories
/// sit below it (`<root>/<agg|seq>/<domain>/`).
#[must_use]
pub fn library_root(kind: ExperimentKind, ns: &Namespace) -> Option<String> {
    let _ = (kind, ns);
    None
}
