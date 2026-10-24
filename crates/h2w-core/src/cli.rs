//! The exact argv of every harness2 command, built only from validated API types.
//!
//! The server resolves [`Program`] to an executable inside the harness2 virtualenv
//! (`<home>/.venv/bin/<bin_name>`) and runs it without a shell. Every free-text value is
//! validated before it reaches argv, and no value can start with `-` (so none can be read
//! as an option).

use h2w_api::{
    Bench, BridgeParams, Cell, DEMO_K_MAX, DemoParams, JudgeParams, K_MAX, Mode, ReportParams,
    RunOptions, SetupParams, SplitParams,
};

use crate::{CoreError, Result, names};

/// Which harness2 entry point to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Program {
    /// `harness2` (refinement sweep; with `--dry-run`, the preview).
    Harness2,
    Setup,
    Check,
    Judge,
    Report,
    Demo,
    Bridge,
    /// `python scripts/split_tasks.py` run with the venv's interpreter, cwd = harness2 home.
    SplitScript,
}

impl Program {
    /// Executable name in `<venv>/bin/` (`python` for [`Program::SplitScript`]).
    #[must_use]
    pub fn bin_name(self) -> &'static str {
        match self {
            Program::Harness2 => "harness2",
            Program::Setup => "harness2-setup",
            Program::Check => "harness2-check",
            Program::Judge => "harness2-judge",
            Program::Report => "harness2-report",
            Program::Demo => "harness2-demo",
            Program::Bridge => "harness2-bridge",
            Program::SplitScript => "python",
        }
    }
}

/// A command ready to run: program + arguments (no shell, no environment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: Program,
    pub args: Vec<String>,
}

/// Collected `(field, problem)` pairs -> one error.
fn fail(mut errs: Vec<(String, String)>) -> CoreError {
    if errs.len() == 1 {
        let (field, problem) = errs.remove(0);
        CoreError::Invalid { field, problem }
    } else {
        CoreError::InvalidMany(errs)
    }
}

fn push_err(errs: &mut Vec<(String, String)>, field: &str, r: Result<()>) {
    match r {
        Ok(()) => {}
        Err(CoreError::Invalid { problem, .. }) => errs.push((field.to_owned(), problem)),
        Err(other) => errs.push((field.to_owned(), other.to_string())),
    }
}

/// An absolute path as the child process will see it: starts with `/`, no NUL or control
/// characters, at most 4096 bytes. (It can never be read as an option.)
fn abs_path(field: &str, p: &str) -> Result<()> {
    if !p.starts_with('/') || p.len() > 4096 || p.chars().any(char::is_control) {
        return Err(CoreError::invalid(
            field,
            "must be an absolute path without control characters",
        ));
    }
    Ok(())
}

fn k_ok(errs: &mut Vec<(String, String)>, field: &str, k: u32, max: u32) {
    if k == 0 || k > max {
        errs.push((field.to_owned(), format!("must be between 1 and {max}")));
    }
}

fn modes_ok(errs: &mut Vec<(String, String)>, modes: &[Mode]) {
    if modes.is_empty() {
        errs.push(("modes".into(), "choose at least one recursion mode".into()));
    }
    for (i, m) in modes.iter().enumerate() {
        if modes.get(..i).is_some_and(|before| before.contains(m)) {
            errs.push(("modes".into(), format!("{} listed twice", m.as_cli())));
        }
    }
}

/// `--modes` value in upstream's canonical order (parallel first).
fn modes_cli(modes: &[Mode]) -> String {
    Mode::ALL
        .iter()
        .filter(|m| modes.contains(m))
        .map(|m| m.as_cli())
        .collect::<Vec<_>>()
        .join(",")
}

fn cell_args(cell: &Cell, errs: &mut Vec<(String, String)>) -> Vec<String> {
    if let Err(problem) = cell.validate() {
        errs.push(("cell".into(), problem));
    }
    let mut a = vec!["--bench".to_owned(), cell.bench.as_cli().to_owned()];
    if let Some(s) = cell.subset {
        a.push("--subset".into());
        a.push(s.as_cli().into());
    }
    a.push("--substrate".into());
    a.push(cell.substrate.as_cli().into());
    a
}

fn finish(program: Program, args: Vec<String>, errs: Vec<(String, String)>) -> Result<Invocation> {
    if errs.is_empty() {
        Ok(Invocation { program, args })
    } else {
        Err(fail(errs))
    }
}

/// `harness2-demo --output <output_dir> --k <k> --mode <mode>`.
/// `output_dir` is the experiment directory as the process will see it.
pub fn demo(p: &DemoParams, output_dir: &str) -> Result<Invocation> {
    let mut errs = Vec::new();
    push_err(
        &mut errs,
        "experiment",
        names::experiment_name(&p.experiment),
    );
    k_ok(&mut errs, "k", p.k, DEMO_K_MAX);
    push_err(&mut errs, "output_dir", abs_path("output_dir", output_dir));
    let args = vec![
        "--output".to_owned(),
        output_dir.to_owned(),
        "--k".to_owned(),
        p.k.to_string(),
        "--mode".to_owned(),
        p.mode.as_cli().to_owned(),
    ];
    finish(Program::Demo, args, errs)
}

/// `harness2-setup <bench> [--subset S] [--substrate X] [--dry-run]`.
pub fn setup(p: &SetupParams) -> Result<Invocation> {
    let mut errs = Vec::new();
    if p.subset.is_some() && p.bench != Bench::Wb {
        errs.push(("subset".into(), "--subset only applies to wb".into()));
    }
    if let Some(profile) = &p.profile {
        push_err(&mut errs, "profile", names::profile_name(profile));
    }
    let mut args = vec![p.bench.as_cli().to_owned()];
    if let Some(s) = p.subset {
        args.push("--subset".into());
        args.push(s.as_cli().into());
    }
    args.push("--substrate".into());
    args.push(p.substrate.as_cli().into());
    if p.dry_run {
        args.push("--dry-run".into());
    }
    finish(Program::Setup, args, errs)
}

/// `harness2-check --bench B [--subset S] [--substrate X]`.
pub fn check(cell: &Cell) -> Result<Invocation> {
    let mut errs = Vec::new();
    let args = cell_args(cell, &mut errs);
    finish(Program::Check, args, errs)
}

fn sweep_args(
    cell: &Cell,
    options: &RunOptions,
    split_file: Option<&str>,
    errs: &mut Vec<(String, String)>,
) -> Vec<String> {
    let mut args = cell_args(cell, errs);
    if let Err(problems) = options.validate() {
        errs.extend(problems);
    }
    for d in &options.only_domains {
        if let Err(e) = names::domain(d) {
            let problem = match e {
                CoreError::Invalid { problem, .. } => problem,
                other => other.to_string(),
            };
            errs.push(("only_domains".into(), format!("{d:?} {problem}")));
        }
    }
    if let Some(name) = &options.split_file {
        push_err(errs, "split_file", names::experiment_name(name));
    }
    match (&options.split_file, split_file) {
        (Some(_), None) => errs.push((
            "split_file".into(),
            "the split file was not resolved to a path".into(),
        )),
        (None, Some(_)) => errs.push((
            "split_file".into(),
            "a split file path was given but the options name none".into(),
        )),
        _ => {}
    }
    args.push("--k".into());
    args.push(options.k.to_string());
    if let Some(seed) = options.seed {
        args.push("--seed".into());
        args.push(seed.to_string());
    }
    args.push("--modes".into());
    args.push(options.modes_cli());
    args.push("--split".into());
    args.push(options.split.as_cli().into());
    if let Some(path) = split_file {
        push_err(errs, "split_file", abs_path("split_file", path));
        args.push("--split-file".into());
        args.push(path.to_owned());
    }
    args.push("--workers".into());
    args.push(options.workers.to_string());
    if !options.only_domains.is_empty() {
        args.push("--only-domains".into());
        args.push(options.only_domains.join(","));
    }
    if let Some(n) = options.max_tasks {
        args.push("--max-tasks".into());
        args.push(n.to_string());
    }
    args
}

/// `harness2 --bench ... --k K [--seed N] --modes M --split S [--split-file F] --workers W
/// [--only-domains a,b] [--max-tasks N] --dry-run`.
pub fn preview(cell: &Cell, options: &RunOptions, split_file: Option<&str>) -> Result<Invocation> {
    let mut errs = Vec::new();
    let mut args = sweep_args(cell, options, split_file, &mut errs);
    args.push("--dry-run".into());
    finish(Program::Harness2, args, errs)
}

/// The same as [`preview`] without `--dry-run`.
pub fn run(cell: &Cell, options: &RunOptions, split_file: Option<&str>) -> Result<Invocation> {
    let mut errs = Vec::new();
    let args = sweep_args(cell, options, split_file, &mut errs);
    finish(Program::Harness2, args, errs)
}

/// `harness2-judge --bench ... --k K [--seed N] --modes M [--bases]`.
pub fn judge(p: &JudgeParams) -> Result<Invocation> {
    let mut errs = Vec::new();
    push_err(
        &mut errs,
        "experiment",
        names::experiment_name(&p.experiment),
    );
    push_err(&mut errs, "profile", names::profile_name(&p.profile));
    let mut args = cell_args(&p.cell, &mut errs);
    k_ok(&mut errs, "k", p.k, K_MAX);
    modes_ok(&mut errs, &p.modes);
    args.push("--k".into());
    args.push(p.k.to_string());
    if let Some(seed) = p.seed {
        args.push("--seed".into());
        args.push(seed.to_string());
    }
    args.push("--modes".into());
    args.push(modes_cli(&p.modes));
    if p.bases {
        args.push("--bases".into());
    }
    finish(Program::Judge, args, errs)
}

/// `harness2-report --bench ... --k 1,2 [--seed N] --modes M`.
pub fn report(p: &ReportParams) -> Result<Invocation> {
    let mut errs = Vec::new();
    push_err(
        &mut errs,
        "experiment",
        names::experiment_name(&p.experiment),
    );
    let mut args = cell_args(&p.cell, &mut errs);
    if p.ks.is_empty() {
        errs.push(("ks".into(), "name at least one recursion budget".into()));
    }
    for &k in &p.ks {
        k_ok(&mut errs, "ks", k, K_MAX);
    }
    modes_ok(&mut errs, &p.modes);
    let mut ks = p.ks.clone();
    ks.sort_unstable();
    ks.dedup();
    args.push("--k".into());
    args.push(ks.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
    if let Some(seed) = p.seed {
        args.push("--seed".into());
        args.push(seed.to_string());
    }
    args.push("--modes".into());
    args.push(modes_cli(&p.modes));
    finish(Program::Report, args, errs)
}

/// `python scripts/split_tasks.py --bench B [--subset S] --seed N --train-fraction F
/// --output <output_path>`.
pub fn split(p: &SplitParams, output_path: &str) -> Result<Invocation> {
    let mut errs = Vec::new();
    if (p.bench == Bench::Wb) != p.subset.is_some() {
        errs.push((
            "subset".into(),
            "--subset is required for wb and only applies to wb".into(),
        ));
    }
    if !(p.train_fraction.is_finite() && p.train_fraction > 0.0 && p.train_fraction < 1.0) {
        errs.push((
            "train_fraction".into(),
            "must be between 0 and 1 (exclusive)".into(),
        ));
    }
    push_err(&mut errs, "name", names::experiment_name(&p.name));
    push_err(&mut errs, "output", abs_path("output", output_path));
    let mut args = vec![
        "scripts/split_tasks.py".to_owned(),
        "--bench".to_owned(),
        p.bench.as_cli().to_owned(),
    ];
    if let Some(s) = p.subset {
        args.push("--subset".into());
        args.push(s.as_cli().into());
    }
    args.push("--seed".into());
    args.push(p.seed.to_string());
    args.push("--train-fraction".into());
    args.push(format!("{}", p.train_fraction));
    args.push("--output".into());
    args.push(output_path.to_owned());
    finish(Program::SplitScript, args, errs)
}

/// `harness2-bridge --model M [--host 0.0.0.0] [--port N]`.
pub fn bridge(p: &BridgeParams, model: &str) -> Result<Invocation> {
    let mut errs = Vec::new();
    push_err(&mut errs, "model", names::model_id(model));
    push_err(&mut errs, "profile", names::profile_name(&p.profile));
    if p.port == Some(0) {
        errs.push((
            "port".into(),
            "must be 1-65535 (leave it empty to reuse the saved or a free port)".into(),
        ));
    }
    let mut args = vec!["--model".to_owned(), model.to_owned()];
    if p.expose_to_containers {
        args.push("--host".into());
        args.push("0.0.0.0".into());
    }
    if let Some(port) = p.port {
        args.push("--port".into());
        args.push(port.to_string());
    }
    finish(Program::Bridge, args, errs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2w_api::{DemoMode, Split, Substrate, WbSubset};

    fn lab() -> Cell {
        Cell {
            bench: Bench::Lab,
            subset: None,
            substrate: Substrate::Oc,
        }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn demo_argv() {
        let p = DemoParams {
            experiment: "demo".into(),
            k: 2,
            mode: DemoMode::Both,
        };
        let inv = demo(&p, "/srv/exp/demo").unwrap();
        assert_eq!(inv.program.bin_name(), "harness2-demo");
        assert_eq!(
            inv.args,
            strs(&["--output", "/srv/exp/demo", "--k", "2", "--mode", "both"])
        );
        assert!(demo(&p, "relative").is_err());
        let bad = DemoParams { k: 9, ..p };
        assert!(demo(&bad, "/x").is_err());
    }

    #[test]
    fn setup_and_check_argv() {
        let p = SetupParams {
            bench: Bench::Wb,
            subset: Some(WbSubset::Office),
            substrate: Substrate::Cx,
            dry_run: true,
            profile: None,
        };
        assert_eq!(
            setup(&p).unwrap().args,
            strs(&["wb", "--subset", "office", "--substrate", "cx", "--dry-run"])
        );
        let bad = SetupParams {
            bench: Bench::Lab,
            ..p
        };
        assert!(setup(&bad).is_err());
        assert_eq!(
            check(&lab()).unwrap().args,
            strs(&["--bench", "lab", "--substrate", "oc"])
        );
        let wb_no_subset = Cell {
            bench: Bench::Wb,
            subset: None,
            substrate: Substrate::Oc,
        };
        assert!(check(&wb_no_subset).is_err());
    }

    #[test]
    fn preview_and_run_argv() {
        let o = RunOptions {
            k: 2,
            seed: Some(7),
            modes: vec![Mode::Sequential, Mode::Parallel],
            split: Split::Train,
            split_file: Some("half".into()),
            workers: 3,
            only_domains: vec!["contracts".into(), "practice area".into()],
            max_tasks: Some(2),
        };
        let inv = preview(&lab(), &o, Some("/data/splits/half.json")).unwrap();
        assert_eq!(inv.program, Program::Harness2);
        assert_eq!(
            inv.args,
            strs(&[
                "--bench",
                "lab",
                "--substrate",
                "oc",
                "--k",
                "2",
                "--seed",
                "7",
                "--modes",
                "parallel,sequential",
                "--split",
                "train",
                "--split-file",
                "/data/splits/half.json",
                "--workers",
                "3",
                "--only-domains",
                "contracts,practice area",
                "--max-tasks",
                "2",
                "--dry-run"
            ])
        );
        let r = run(&lab(), &o, Some("/data/splits/half.json")).unwrap();
        assert_eq!(r.args.last().map(String::as_str), Some("2"));
        assert!(run(&lab(), &o, None).is_err());
        let bad_domain = RunOptions {
            only_domains: vec!["-x".into()],
            split: Split::All,
            split_file: None,
            ..o
        };
        match run(&lab(), &bad_domain, None) {
            Err(CoreError::Invalid { field, .. }) => assert_eq!(field, "only_domains"),
            other => panic!("{other:?}"),
        }
        let defaults = run(&lab(), &RunOptions::default(), None).unwrap();
        assert_eq!(
            defaults.args,
            strs(&[
                "--bench",
                "lab",
                "--substrate",
                "oc",
                "--k",
                "1",
                "--modes",
                "parallel",
                "--split",
                "all",
                "--workers",
                "1"
            ])
        );
    }

    #[test]
    fn judge_report_split_bridge_argv() {
        let j = JudgeParams {
            experiment: "exp".into(),
            cell: lab(),
            k: 1,
            seed: None,
            modes: vec![Mode::Parallel, Mode::Sequential],
            bases: true,
            profile: "p".into(),
        };
        assert_eq!(
            judge(&j).unwrap().args,
            strs(&[
                "--bench",
                "lab",
                "--substrate",
                "oc",
                "--k",
                "1",
                "--modes",
                "parallel,sequential",
                "--bases"
            ])
        );
        let dup = JudgeParams {
            modes: vec![Mode::Parallel, Mode::Parallel],
            ..j
        };
        assert!(judge(&dup).is_err());
        let r = ReportParams {
            experiment: "exp".into(),
            cell: lab(),
            ks: vec![2, 1, 2],
            seed: Some(3),
            modes: vec![Mode::Parallel],
        };
        assert_eq!(
            report(&r).unwrap().args,
            strs(&[
                "--bench",
                "lab",
                "--substrate",
                "oc",
                "--k",
                "1,2",
                "--seed",
                "3",
                "--modes",
                "parallel"
            ])
        );
        let s = SplitParams {
            bench: Bench::Wb,
            subset: Some(WbSubset::Code),
            seed: 42,
            train_fraction: 0.3,
            name: "half".into(),
        };
        let inv = split(&s, "/data/splits/half.json").unwrap();
        assert_eq!(inv.program.bin_name(), "python");
        assert_eq!(
            inv.args,
            strs(&[
                "scripts/split_tasks.py",
                "--bench",
                "wb",
                "--subset",
                "code",
                "--seed",
                "42",
                "--train-fraction",
                "0.3",
                "--output",
                "/data/splits/half.json"
            ])
        );
        let bad = SplitParams {
            subset: None,
            train_fraction: 1.0,
            ..s
        };
        assert!(matches!(split(&bad, "/x"), Err(CoreError::InvalidMany(v)) if v.len() == 2));
        let b = BridgeParams {
            model: None,
            expose_to_containers: true,
            port: Some(4100),
            profile: "p".into(),
        };
        assert_eq!(
            bridge(&b, "gemini-3.5-flash").unwrap().args,
            strs(&[
                "--model",
                "gemini-3.5-flash",
                "--host",
                "0.0.0.0",
                "--port",
                "4100"
            ])
        );
        assert!(bridge(&b, "--evil").is_err());
    }

    #[test]
    fn no_value_starts_with_a_dash() {
        let o = RunOptions {
            only_domains: vec!["a".into()],
            ..RunOptions::default()
        };
        let inv = run(&lab(), &o, None).unwrap();
        for pair in inv.args.chunks(2) {
            if let [flag, value] = pair {
                assert!(flag.starts_with("--"), "{flag}");
                assert!(!value.starts_with('-'), "{value}");
            }
        }
    }
}
