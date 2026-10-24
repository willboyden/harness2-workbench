//! harness2 run namespaces: `<bench>-<substrate>-<mode><k>[-s<seed>][-<tag>]`.
//!
//! Upstream builds them in `core.schedule.ns()`. The *summary namespace* of a sweep is the
//! parallel-mode one (`agg` token): every task's `summary.json` and the sweep's
//! `judged.json` live there for both modes.

use std::fmt;

use h2w_api::{Bench, Cell, ExperimentKind, Mode, Substrate, WbSubset};

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

/// Bench names a namespace can start with, and the directory (relative to the experiment
/// root) that holds that benchmark's `adapt/` and `library/` (`None` for the demo).
const BENCHES: [(&str, Option<&str>); 6] = [
    ("wb-office", Some("wb/office")),
    ("wb-web", Some("wb/web")),
    ("wb-code", Some("wb/code")),
    ("jb", Some("jb")),
    ("lab", Some("lab")),
    ("demo", None),
];

/// The directory holding a standard benchmark's `adapt/`, `library/` (`jb`, `lab`,
/// `wb/office`, ...). `None` for the demo or an unknown name.
pub(crate) fn bench_dir(bench_name: &str) -> Option<&'static str> {
    BENCHES
        .iter()
        .find(|(n, _)| *n == bench_name)
        .and_then(|(_, d)| *d)
}

/// The benchmark cell a standard bench name stands for (`None` for the demo).
pub(crate) fn cell_of(bench_name: &str, substrate: Substrate) -> Option<Cell> {
    let (bench, subset) = match bench_name {
        "jb" => (Bench::Jb, None),
        "lab" => (Bench::Lab, None),
        "wb-office" => (Bench::Wb, Some(WbSubset::Office)),
        "wb-web" => (Bench::Wb, Some(WbSubset::Web)),
        "wb-code" => (Bench::Wb, Some(WbSubset::Code)),
        _ => return None,
    };
    Some(Cell {
        bench,
        subset,
        substrate,
    })
}

/// Upstream `schedule.validate_tag`: `[A-Za-z0-9_.][A-Za-z0-9_.-]*`, and not spelling a seed
/// segment (`s<digits>` followed by `-` or the end).
pub(crate) fn tag_is_valid(tag: &str) -> bool {
    let b = tag.as_bytes();
    let charset = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'.';
    let shape = match b.split_first() {
        Some((f, rest)) => charset(*f) && rest.iter().all(|&c| charset(c) || c == b'-'),
        None => false,
    };
    if !shape {
        return false;
    }
    if let Some(r) = tag.strip_prefix('s') {
        let digits = r.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && matches!(r.as_bytes().get(digits), None | Some(b'-')) {
            return false;
        }
    }
    true
}

/// Canonical decimal digits (no sign, no leading zero unless the value is 0).
fn canonical_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
}

impl Namespace {
    /// Parse a namespace directory name. `None` for anything else (`_sweeps`, `_audit`,
    /// stray files). Must round-trip with `Display` for every value it accepts.
    #[must_use]
    pub fn parse(s: &str) -> Option<Namespace> {
        if s.is_empty() || s.len() > 255 {
            return None;
        }
        let (bench_name, rest) = BENCHES.iter().find_map(|(b, _)| {
            s.strip_prefix(b)
                .and_then(|r| r.strip_prefix('-'))
                .map(|r| (*b, r))
        })?;
        let (substrate, rest) = [("oc-", Substrate::Oc), ("cx-", Substrate::Cx)]
            .into_iter()
            .find_map(|(p, sub)| rest.strip_prefix(p).map(|r| (sub, r)))?;
        let (mode, rest) = [("agg", Mode::Parallel), ("seq", Mode::Sequential)]
            .into_iter()
            .find_map(|(p, m)| rest.strip_prefix(p).map(|r| (m, r)))?;
        let k_len = rest.bytes().take_while(u8::is_ascii_digit).count();
        let (k_digits, mut rest) = rest.split_at(k_len);
        if !canonical_digits(k_digits) {
            return None;
        }
        let k: u32 = k_digits.parse().ok()?;
        if k == 0 {
            return None;
        }
        let mut seed = None;
        if let Some(r) = rest.strip_prefix("-s") {
            let n = r.bytes().take_while(u8::is_ascii_digit).count();
            let (digits, after) = r.split_at(n);
            if n > 0 && (after.is_empty() || after.starts_with('-')) {
                if !canonical_digits(digits) {
                    return None;
                }
                seed = Some(digits.parse::<u64>().ok()?);
                rest = after;
            }
        }
        let tag = if rest.is_empty() {
            None
        } else {
            let t = rest.strip_prefix('-')?;
            if !tag_is_valid(t) {
                return None;
            }
            Some(t.to_owned())
        };
        Some(Namespace {
            bench_name: bench_name.to_owned(),
            substrate,
            mode,
            k,
            seed,
            tag,
        })
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
    match kind {
        ExperimentKind::Demo => (bench_name == "demo").then(|| "runs".to_owned()),
        ExperimentKind::Standard => bench_dir(bench_name).map(|d| format!("{d}/adapt")),
    }
}

/// Library root of a sweep, relative to the experiment root:
/// standard `lab/library/lab/k1-oc[-s7]`, demo `library/k2`. Mode and domain directories
/// sit below it (`<root>/<agg|seq>/<domain>/`).
#[must_use]
pub fn library_root(kind: ExperimentKind, ns: &Namespace) -> Option<String> {
    match kind {
        ExperimentKind::Demo => (ns.bench_name == "demo").then(|| format!("library/k{}", ns.k)),
        ExperimentKind::Standard => {
            // Upstream cli/_common.library_root_for + core/library.root_for.
            let dir = bench_dir(&ns.bench_name)?;
            let mut variant = ns.substrate.as_cli().to_owned();
            if let Some(seed) = ns.seed {
                variant.push_str(&format!("-s{seed}"));
            }
            if let Some(tag) = &ns.tag {
                variant.push('-');
                variant.push_str(tag);
            }
            Some(format!(
                "{dir}/library/{}/k{}-{variant}",
                ns.bench_name, ns.k
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parses_upstream_examples() {
        let ns = Namespace::parse("lab-oc-agg1-s7").unwrap();
        assert_eq!(ns.bench_name, "lab");
        assert_eq!(ns.substrate, Substrate::Oc);
        assert_eq!(ns.mode, Mode::Parallel);
        assert_eq!((ns.k, ns.seed, ns.tag.as_deref()), (1, Some(7), None));
        let ns = Namespace::parse("wb-office-cx-seq12-s3-v2.b").unwrap();
        assert_eq!(ns.bench_name, "wb-office");
        assert_eq!(
            (ns.k, ns.seed, ns.tag.as_deref()),
            (12, Some(3), Some("v2.b"))
        );
        let ns = Namespace::parse("demo-oc-agg2").unwrap();
        assert_eq!(ns.to_string(), "demo-oc-agg2");
        assert_eq!(
            Namespace::parse("lab-oc-agg1-s7x").unwrap().tag.as_deref(),
            Some("s7x")
        );
        for bad in [
            "_sweeps",
            "_audit",
            "lab-oc-agg0",
            "lab-oc-agg01",
            "lab-oc-agg1-s07",
            "lab-oc-agg1-s7-s8",
            "lab-oc-agg1-",
            "lab-oc-agg1--x",
            "lab-xx-agg1",
            "lab-oc-mix1",
            "wb-oc-agg1",
            "lab-oc-agg",
            "lab-oc-agg1/x",
            "lab-oc-agg1-s7\n",
            "summary.json",
        ] {
            assert_eq!(Namespace::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn roots_follow_upstream_layout() {
        let ns = Namespace::parse("lab-oc-seq1-s7").unwrap();
        assert_eq!(
            namespaces_root(ExperimentKind::Standard, "lab").as_deref(),
            Some("lab/adapt")
        );
        assert_eq!(
            namespaces_root(ExperimentKind::Standard, "wb-web").as_deref(),
            Some("wb/web/adapt")
        );
        assert_eq!(namespaces_root(ExperimentKind::Standard, "demo"), None);
        assert_eq!(
            namespaces_root(ExperimentKind::Demo, "demo").as_deref(),
            Some("runs")
        );
        assert_eq!(namespaces_root(ExperimentKind::Standard, "xx"), None);
        assert_eq!(
            library_root(ExperimentKind::Standard, &ns).as_deref(),
            Some("lab/library/lab/k1-oc-s7")
        );
        let wb = Namespace::parse("wb-code-cx-agg2").unwrap();
        assert_eq!(
            library_root(ExperimentKind::Standard, &wb).as_deref(),
            Some("wb/code/library/wb-code/k2-cx")
        );
        let demo = Namespace::parse("demo-oc-seq2").unwrap();
        assert_eq!(
            library_root(ExperimentKind::Demo, &demo).as_deref(),
            Some("library/k2")
        );
        assert_eq!(ns.summary().to_string(), "lab-oc-agg1-s7");
        let cell = cell_of("wb-web", Substrate::Cx).unwrap();
        assert_eq!(cell.bench_name(), "wb-web");
        assert_eq!(
            Namespace::for_cell(&cell, Mode::Sequential, 3, None).to_string(),
            "wb-web-cx-seq3"
        );
    }

    #[test]
    fn tags_follow_upstream_validate_tag() {
        for ok in ["v2", "a.b", "_x", "s", "s7x", "x-s7", ".", "s-5"] {
            assert!(tag_is_valid(ok), "{ok}");
        }
        for bad in ["", "-x", "s7", "s7-x", "a/b", "a b", "é"] {
            assert!(!tag_is_valid(bad), "{bad}");
        }
    }

    fn arb_ns() -> impl Strategy<Value = Namespace> {
        (
            prop::sample::select(vec!["jb", "lab", "wb-office", "wb-web", "wb-code", "demo"]),
            prop::bool::ANY,
            prop::bool::ANY,
            1_u32..100_000,
            prop::option::of(0_u64..u64::MAX),
            prop::option::of("[A-Za-z0-9_.][A-Za-z0-9_.-]{0,10}"),
        )
            .prop_map(|(b, oc, agg, k, seed, tag)| Namespace {
                bench_name: b.to_owned(),
                substrate: if oc { Substrate::Oc } else { Substrate::Cx },
                mode: if agg {
                    Mode::Parallel
                } else {
                    Mode::Sequential
                },
                k,
                seed,
                tag: tag.filter(|t| tag_is_valid(t)),
            })
    }

    proptest! {
        #[test]
        fn display_then_parse_round_trips(ns in arb_ns()) {
            prop_assert_eq!(Namespace::parse(&ns.to_string()), Some(ns));
        }

        #[test]
        fn parse_then_display_round_trips(s in "(lab|jb|wb-web|demo)-(oc|cx)-(agg|seq)[0-9]{1,3}(-s[0-9]{1,3})?(-[a-zA-Z0-9_.-]{0,6})?|\\PC{0,24}") {
            if let Some(ns) = Namespace::parse(&s) {
                prop_assert_eq!(ns.to_string(), s.clone());
                prop_assert!(!s.contains('/') && !s.contains('\0'));
            }
        }
    }
}
