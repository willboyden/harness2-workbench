//! Structural validation of a [`JobSpec`]: the spec's own rules (`Cell::validate`,
//! `RunOptions::validate`) plus the bounds the server enforces. Identifier rules that need
//! `h2w-core` run in the backend; state checks (experiment exists, profile pinned, secrets
//! set, ...) run in the job manager.

use h2w_api::{
    Bench, Cell, DEMO_K_MAX, FieldProblem, JobSpec, K_MAX, Mode, RunOptions, Substrate, WbSubset,
};

fn p(out: &mut Vec<FieldProblem>, field: impl Into<String>, problem: impl Into<String>) {
    out.push(FieldProblem {
        field: field.into(),
        problem: problem.into(),
    });
}

fn cell(out: &mut Vec<FieldProblem>, field: &str, c: &Cell) {
    if let Err(e) = c.validate() {
        p(out, field, e);
    }
}

fn options(out: &mut Vec<FieldProblem>, o: &RunOptions) {
    if let Err(errs) = o.validate() {
        for (f, problem) in errs {
            p(out, format!("params.options.{f}"), problem);
        }
    }
    if o.only_domains.len() > 1000 {
        p(out, "params.options.only_domains", "at most 1000 domains");
    }
}

fn modes(out: &mut Vec<FieldProblem>, field: &str, m: &[Mode]) {
    if m.is_empty() {
        p(out, field, "choose at least one recursion mode");
    }
    let mut seen = Vec::new();
    for x in m {
        if seen.contains(x) {
            p(out, field, format!("{} listed twice", x.as_cli()));
        }
        seen.push(*x);
    }
}

fn k(out: &mut Vec<FieldProblem>, field: &str, k: u32, max: u32) {
    if k == 0 || k > max {
        p(out, field, format!("must be between 1 and {max}"));
    }
}

fn budget(out: &mut Vec<FieldProblem>, b: Option<f64>) {
    if let Some(b) = b
        && !(b.is_finite() && b > 0.0 && b <= 1_000_000.0)
    {
        p(
            out,
            "params.budget_usd",
            "must be a positive amount up to 1000000",
        );
    }
}

fn subset_rules(
    out: &mut Vec<FieldProblem>,
    bench: Bench,
    subset: Option<WbSubset>,
    required: bool,
) {
    match (bench, subset) {
        (Bench::Wb, None) if required => p(
            out,
            "params.subset",
            "WorkBuddy-Bench needs a subset: office, web or code",
        ),
        (b, Some(_)) if b != Bench::Wb => p(
            out,
            "params.subset",
            format!("subsets apply to WorkBuddy-Bench only, not {}", b.as_cli()),
        ),
        _ => {}
    }
}

/// Every structural problem of `spec`.
#[must_use]
pub fn structural(spec: &JobSpec) -> Vec<FieldProblem> {
    let mut out = Vec::new();
    match spec {
        JobSpec::InstallHarness2(_) => {}
        JobSpec::Demo(d) => k(&mut out, "params.k", d.k, DEMO_K_MAX),
        JobSpec::Setup(s) => {
            subset_rules(&mut out, s.bench, s.subset, false);
            if s.substrate.lab_only() && s.bench != Bench::Lab {
                p(
                    &mut out,
                    "params.substrate",
                    format!("{} runs LAB only so far", s.substrate.display_name()),
                );
            }
        }
        JobSpec::Check(c) => cell(&mut out, "params.cell", &c.cell),
        JobSpec::Preview(pv) => {
            cell(&mut out, "params.cell", &pv.cell);
            options(&mut out, &pv.options);
        }
        JobSpec::Run(r) => {
            cell(&mut out, "params.cell", &r.cell);
            options(&mut out, &r.options);
            budget(&mut out, r.budget_usd);
        }
        JobSpec::Judge(j) => {
            cell(&mut out, "params.cell", &j.cell);
            k(&mut out, "params.k", j.k, K_MAX);
            modes(&mut out, "params.modes", &j.modes);
        }
        JobSpec::Report(r) => {
            cell(&mut out, "params.cell", &r.cell);
            if r.ks.is_empty() {
                p(&mut out, "params.ks", "list at least one recursion budget");
            }
            if r.ks.len() > usize::try_from(K_MAX).unwrap_or(32) {
                p(&mut out, "params.ks", format!("at most {K_MAX} values"));
            }
            let mut seen = Vec::new();
            for (i, v) in r.ks.iter().enumerate() {
                k(&mut out, &format!("params.ks[{i}]"), *v, K_MAX);
                if seen.contains(v) {
                    p(
                        &mut out,
                        format!("params.ks[{i}]"),
                        format!("{v} listed twice"),
                    );
                }
                seen.push(*v);
            }
            modes(&mut out, "params.modes", &r.modes);
        }
        JobSpec::Split(s) => {
            subset_rules(&mut out, s.bench, s.subset, true);
            if !(s.train_fraction.is_finite() && s.train_fraction > 0.0 && s.train_fraction < 1.0) {
                p(
                    &mut out,
                    "params.train_fraction",
                    "must be strictly between 0 and 1",
                );
            }
        }
        JobSpec::Bridge(b) => {
            if b.port == Some(0) {
                p(
                    &mut out,
                    "params.port",
                    "must be between 1 and 65535 (omit it for a free port)",
                );
            }
        }
        JobSpec::Pipeline(pl) => {
            cell(&mut out, "params.cell", &pl.cell);
            options(&mut out, &pl.options);
            budget(&mut out, pl.budget_usd);
        }
    }
    out
}

/// Jobs that write into their experiment take its single-writer lock.
#[must_use]
pub fn writes_experiment(spec: &JobSpec) -> bool {
    matches!(
        spec,
        JobSpec::Demo(_) | JobSpec::Run(_) | JobSpec::Judge(_) | JobSpec::Pipeline(_)
    )
}

/// Jobs that change the harness2 checkout run alone.
#[must_use]
pub fn is_exclusive(spec: &JobSpec) -> bool {
    matches!(spec, JobSpec::InstallHarness2(_) | JobSpec::Setup(_))
}

/// The cell a spec runs on, when it has one.
#[must_use]
pub fn cell_of(spec: &JobSpec) -> Option<Cell> {
    match spec {
        JobSpec::Check(c) => Some(c.cell),
        JobSpec::Preview(p) => Some(p.cell),
        JobSpec::Run(r) => Some(r.cell),
        JobSpec::Judge(j) => Some(j.cell),
        JobSpec::Report(r) => Some(r.cell),
        JobSpec::Pipeline(p) => Some(p.cell),
        _ => None,
    }
}

/// The benchmark a spec runs, sets up or splits, if any.
#[must_use]
pub fn bench_of(spec: &JobSpec) -> Option<Bench> {
    cell_of(spec).map(|c| c.bench).or(match spec {
        JobSpec::Setup(s) => Some(s.bench),
        JobSpec::Split(s) => Some(s.bench),
        _ => None,
    })
}

/// The executor a spec runs or sets up, if any.
#[must_use]
pub fn substrate_of(spec: &JobSpec) -> Option<(&'static str, Substrate)> {
    match spec {
        JobSpec::Setup(s) => Some(("params.substrate", s.substrate)),
        other => cell_of(other).map(|c| ("params.cell.substrate", c.substrate)),
    }
}

/// The profile a spec names.
#[must_use]
pub fn profile_of(spec: &JobSpec) -> Option<&str> {
    match spec {
        JobSpec::Setup(s) => s.profile.as_deref(),
        JobSpec::Check(c) => c.profile.as_deref(),
        JobSpec::Preview(p) => p.profile.as_deref(),
        JobSpec::Run(r) => Some(&r.profile),
        JobSpec::Judge(j) => Some(&j.profile),
        JobSpec::Bridge(b) => Some(&b.profile),
        JobSpec::Pipeline(p) => Some(&p.profile),
        JobSpec::InstallHarness2(_) | JobSpec::Demo(_) | JobSpec::Report(_) | JobSpec::Split(_) => {
            None
        }
    }
}

/// The run options of a spec, when it has them.
#[must_use]
pub fn options_of(spec: &JobSpec) -> Option<&RunOptions> {
    match spec {
        JobSpec::Preview(p) => Some(&p.options),
        JobSpec::Run(r) => Some(&r.options),
        JobSpec::Pipeline(p) => Some(&p.options),
        _ => None,
    }
}

/// The budget of a spec, when it has one.
#[must_use]
pub fn budget_of(spec: &JobSpec) -> Option<f64> {
    match spec {
        JobSpec::Run(r) => r.budget_usd,
        JobSpec::Pipeline(p) => p.budget_usd,
        _ => None,
    }
}

/// `acknowledge_risks` of `run`/`pipeline`.
#[must_use]
pub fn acknowledged(spec: &JobSpec) -> Option<bool> {
    match spec {
        JobSpec::Run(r) => Some(r.acknowledge_risks),
        JobSpec::Pipeline(p) => Some(p.acknowledge_risks),
        _ => None,
    }
}

/// `acknowledge_cloud_judge` of the jobs that judge (`judge`, `pipeline`); `None` for the
/// others, which never call the judge.
#[must_use]
pub fn cloud_judge_acknowledged(spec: &JobSpec) -> Option<bool> {
    match spec {
        JobSpec::Judge(j) => Some(j.acknowledge_cloud_judge),
        JobSpec::Pipeline(p) => Some(p.acknowledge_cloud_judge),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2w_api::{
        BridgeParams, DemoMode, DemoParams, JudgeParams, ReportParams, RunParams, SplitParams,
        Substrate,
    };

    fn lab() -> Cell {
        Cell {
            bench: Bench::Lab,
            subset: None,
            substrate: Substrate::Oc,
        }
    }

    fn fields(spec: &JobSpec) -> Vec<String> {
        structural(spec).into_iter().map(|f| f.field).collect()
    }

    #[test]
    fn demo_k_bounds() {
        let ok = JobSpec::Demo(DemoParams {
            experiment: "d".into(),
            k: 8,
            mode: DemoMode::Both,
        });
        assert!(structural(&ok).is_empty());
        let bad = JobSpec::Demo(DemoParams {
            experiment: "d".into(),
            k: 9,
            mode: DemoMode::Both,
        });
        assert_eq!(fields(&bad), vec!["params.k"]);
    }

    #[test]
    fn run_collects_cell_options_and_budget() {
        let spec = JobSpec::Run(RunParams {
            experiment: "e".into(),
            cell: Cell {
                bench: Bench::Wb,
                subset: None,
                substrate: Substrate::Oc,
            },
            options: RunOptions {
                k: 0,
                ..RunOptions::default()
            },
            profile: "p".into(),
            budget_usd: Some(-1.0),
            acknowledge_risks: true,
        });
        let f = fields(&spec);
        assert!(f.contains(&"params.cell".to_owned()), "{f:?}");
        assert!(f.contains(&"params.options.k".to_owned()), "{f:?}");
        assert!(f.contains(&"params.budget_usd".to_owned()), "{f:?}");
    }

    #[test]
    fn judge_report_split_bridge() {
        let j = JobSpec::Judge(JudgeParams {
            experiment: "e".into(),
            cell: lab(),
            k: 0,
            seed: None,
            modes: vec![],
            bases: false,
            profile: "p".into(),
            acknowledge_cloud_judge: false,
        });
        assert_eq!(fields(&j), vec!["params.k", "params.modes"]);
        let r = JobSpec::Report(ReportParams {
            experiment: "e".into(),
            cell: lab(),
            ks: vec![1, 1, 99],
            seed: None,
            modes: vec![Mode::Parallel],
        });
        assert_eq!(fields(&r), vec!["params.ks[1]", "params.ks[2]"]);
        let s = JobSpec::Split(SplitParams {
            bench: Bench::Lab,
            subset: Some(WbSubset::Web),
            seed: 1,
            train_fraction: 1.0,
            name: "x".into(),
        });
        assert_eq!(fields(&s), vec!["params.subset", "params.train_fraction"]);
        let b = JobSpec::Bridge(BridgeParams {
            model: None,
            expose_to_containers: false,
            port: Some(0),
            profile: "p".into(),
        });
        assert_eq!(fields(&b), vec!["params.port"]);
    }

    #[test]
    fn classification() {
        let d = JobSpec::Demo(DemoParams {
            experiment: "d".into(),
            k: 1,
            mode: DemoMode::Both,
        });
        assert!(writes_experiment(&d));
        assert!(!is_exclusive(&d));
        assert!(profile_of(&d).is_none());
    }
}
