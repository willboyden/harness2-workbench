//! The production [`JobBackend`]: harness2's CLI contract as implemented by `h2s-core`.
//!
//! Host jobs run `<harness2_home>/.venv/bin/<program>`; container jobs run the same path
//! inside the runner image (`/h2s/harness2/...`) through `h2s_core::isolation::run_argv`,
//! with the experiment, the harness2 checkout and the job's files bind-mounted at fixed
//! points. Secret values travel only in the child environment (`-e NAME` for containers).

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use h2s_api::{
    ExperimentKind, FieldProblem, InstallExtra, InstallResult, Isolation, JobResult, JobSpec,
    JudgeParams, Mode, PipelineParams, PreviewResult, ReportParams, ReportResult, RunOptions,
    SplitResult,
};
use h2s_core::cli::{self, Invocation, Program};
use h2s_core::isolation::{ContainerSpec, EXPERIMENT_MOUNT, HARNESS2_MOUNT, JOBFILES_MOUNT, Mount};
use h2s_core::parse::{BridgeSettings, TaskProgress};
use h2s_core::results::ExperimentDir;
use zeroize::Zeroizing;

use super::backend::{
    ContainerRef, ExitSemantics, JobBackend, JobPlan, ParseKind, PlanInput, StepOutput, StepPlan,
};

/// The backend the server uses.
#[derive(Debug, Clone, Copy, Default)]
pub struct CoreBackend;

/// Directory (inside the job dir) a split job writes its file to.
pub const SPLIT_OUT_DIR: &str = "out";
/// The split file name there.
pub const SPLIT_FILE: &str = "split.json";

fn name_problem(out: &mut Vec<FieldProblem>, field: &str, r: h2s_core::Result<()>) {
    match r {
        Ok(()) => {}
        Err(h2s_core::CoreError::Invalid { problem, .. }) => out.push(FieldProblem {
            field: field.to_owned(),
            problem,
        }),
        Err(h2s_core::CoreError::Unimplemented(what)) => out.push(FieldProblem {
            field: field.to_owned(),
            problem: format!("cannot be validated by this build ({what})"),
        }),
        Err(e) => out.push(FieldProblem {
            field: field.to_owned(),
            problem: e.to_string(),
        }),
    }
}

/// Measured spend per (task, mode) from an experiment's earlier sweeps.
#[must_use]
pub fn history(dir: &Path, kind: ExperimentKind) -> Option<h2s_core::estimate::History> {
    let exp = ExperimentDir::open(dir, kind, &[EXPERIMENT_MOUNT.to_owned()]).ok()?;
    let sweeps = exp.sweeps().ok()?;
    let mut usd = 0.0;
    let mut samples: u64 = 0;
    for s in &sweeps {
        let n = u64::from(s.tasks) * u64::try_from(s.modes.len()).unwrap_or(0);
        if n > 0 && s.usd > 0.0 {
            usd += s.usd;
            samples += n;
        }
    }
    (samples > 0).then(|| h2s_core::estimate::History {
        usd_per_task_mode: usd / f64::from(u32::try_from(samples).unwrap_or(u32::MAX)),
        samples: u32::try_from(samples).unwrap_or(u32::MAX),
    })
}

/// A path as text for argv/env; refuses non-UTF-8 paths.
fn text(p: &Path) -> Result<String, String> {
    p.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{} is not valid UTF-8", p.display()))
}

/// Validate a bind-mount source: absolute, a real directory (not a symlink), and free of
/// the characters `--volume` syntax gives meaning to.
fn mount_source(p: &Path) -> Result<String, String> {
    let s = text(p)?;
    if !p.is_absolute()
        || s.bytes()
            .any(|b| b == b':' || b == b',' || b.is_ascii_control())
        || p.as_os_str().as_bytes().is_empty()
    {
        return Err(format!("{s} cannot be bind-mounted"));
    }
    let meta = std::fs::symlink_metadata(p).map_err(|e| format!("{s}: {e}"))?;
    if !meta.is_dir() {
        return Err(format!("{s} must be a directory, not a symlink or file"));
    }
    Ok(s)
}

/// Where things are as the job's process sees them.
struct View {
    home: String,
    experiment: Option<String>,
    jobfiles: String,
}

struct Planner<'a> {
    input: &'a PlanInput<'a>,
    view: View,
    env: Vec<(String, String)>,
}

impl Planner<'_> {
    fn program_path(&self, program: Program) -> String {
        format!("{}/.venv/bin/{}", self.view.home, program.bin_name())
    }

    /// One harness2 step from a core invocation.
    #[allow(clippy::too_many_arguments)] // a private constructor with named call sites
    fn h2_step(
        &self,
        name: &str,
        inv: &Invocation,
        parse: ParseKind,
        progress: bool,
        budget: bool,
        bridge: bool,
        home_rw: bool,
    ) -> Result<StepPlan, String> {
        let program = self.program_path(inv.program);
        let mut display = vec![inv.program.bin_name().to_owned()];
        display.extend(inv.args.iter().cloned());
        self.wrap(
            name,
            &program,
            &inv.args,
            &self.view.home.clone(),
            display,
            (parse, ExitSemantics::Harness2),
            (progress, budget, bridge),
            (home_rw, None),
        )
    }

    /// Run `program args` on the host, or inside the runner container. `out_dir`, when
    /// given, is mounted read-write at the experiment mount point in place of an
    /// experiment (the job-files mount is always read-only).
    #[allow(clippy::too_many_arguments)]
    fn wrap(
        &self,
        name: &str,
        program: &str,
        args: &[String],
        cwd: &str,
        display: Vec<String>,
        (parse, exit): (ParseKind, ExitSemantics),
        (progress, budget, bridge): (bool, bool, bool),
        (home_rw, out_dir): (bool, Option<&Path>),
    ) -> Result<StepPlan, String> {
        let input = self.input;
        let base = StepPlan {
            name: name.to_owned(),
            program: program.to_owned(),
            args: args.to_vec(),
            cwd: PathBuf::from(cwd),
            env: self.env.clone(),
            runtime_env: false,
            container: None,
            exit,
            parse,
            progress,
            budget,
            bridge,
            display,
        };
        let Some(c) = input
            .container
            .filter(|_| input.isolation == Isolation::Container)
        else {
            return Ok(base);
        };
        let mut mounts = vec![Mount {
            host: mount_source(input.harness2_home)?,
            container: HARNESS2_MOUNT.to_owned(),
            read_only: !home_rw,
        }];
        if let Some(exp) = out_dir.or(input.experiment.map(|(p, _)| p)) {
            mounts.push(Mount {
                host: mount_source(exp)?,
                container: EXPERIMENT_MOUNT.to_owned(),
                read_only: false,
            });
        }
        mounts.push(Mount {
            host: mount_source(input.job_dir)?,
            container: JOBFILES_MOUNT.to_owned(),
            read_only: true,
        });
        let spec = ContainerSpec {
            runtime: c.runtime.core(),
            image: c.image.clone(),
            job_id: input.job_id.to_owned(),
            mounts,
            env_names: self.env.iter().map(|(n, _)| n.clone()).collect(),
            network: c.network.clone(),
            cpus: Some(c.cpus),
            memory_mb: Some(c.memory_mb),
            pids_limit: c.pids_limit,
            workdir: cwd.to_owned(),
            user: Some(c.user.clone()),
        };
        let argv =
            h2s_core::isolation::run_argv(&spec, program, args).map_err(|e| e.to_string())?;
        let (runtime_program, runtime_args) = argv
            .split_first()
            .ok_or_else(|| "empty container command".to_owned())?;
        Ok(StepPlan {
            program: runtime_program.clone(),
            args: runtime_args.to_vec(),
            cwd: input.job_dir.to_path_buf(),
            runtime_env: true,
            container: Some(ContainerRef {
                runtime: c.runtime,
                job_id: input.job_id.to_owned(),
            }),
            ..base
        })
    }
}

fn judge_of(p: &PipelineParams, bases: bool) -> JudgeParams {
    JudgeParams {
        experiment: p.experiment.clone(),
        cell: p.cell,
        k: p.options.k,
        seed: p.options.seed,
        modes: p.options.modes.clone(),
        bases,
        profile: p.profile.clone(),
    }
}

fn report_of(p: &PipelineParams) -> ReportParams {
    ReportParams {
        experiment: p.experiment.clone(),
        cell: p.cell,
        ks: vec![p.options.k],
        seed: p.options.seed,
        modes: p.options.modes.clone(),
    }
}

impl JobBackend for CoreBackend {
    fn check_names(&self, spec: &JobSpec) -> Vec<FieldProblem> {
        use h2s_core::names;
        let mut out = Vec::new();
        if let Some(e) = spec.experiment() {
            name_problem(&mut out, "params.experiment", names::experiment_name(e));
        }
        if let Some(p) = super::validate::profile_of(spec) {
            name_problem(&mut out, "params.profile", names::profile_name(p));
        }
        if let Some(o) = super::validate::options_of(spec) {
            if let Some(sf) = &o.split_file {
                name_problem(
                    &mut out,
                    "params.options.split_file",
                    names::experiment_name(sf),
                );
            }
            for (i, d) in o.only_domains.iter().enumerate() {
                name_problem(
                    &mut out,
                    &format!("params.options.only_domains[{i}]"),
                    names::domain(d),
                );
            }
        }
        match spec {
            JobSpec::InstallHarness2(p) => {
                if let Some(rev) = &p.rev {
                    name_problem(&mut out, "params.rev", names::commit(rev));
                }
            }
            JobSpec::Split(s) => {
                name_problem(&mut out, "params.name", names::experiment_name(&s.name));
            }
            JobSpec::Bridge(b) => {
                if let Some(m) = &b.model {
                    name_problem(&mut out, "params.model", names::model_id(m));
                }
            }
            _ => {}
        }
        out
    }

    #[allow(clippy::too_many_lines)] // one arm per job kind
    fn plan(&self, input: &PlanInput<'_>) -> Result<JobPlan, String> {
        let container = input.isolation == Isolation::Container;
        let view = if container {
            View {
                home: HARNESS2_MOUNT.to_owned(),
                experiment: input.experiment.map(|_| EXPERIMENT_MOUNT.to_owned()),
                jobfiles: JOBFILES_MOUNT.to_owned(),
            }
        } else {
            View {
                home: text(input.harness2_home)?,
                experiment: input.experiment.map(|(p, _)| text(p)).transpose()?,
                jobfiles: text(input.job_dir)?,
            }
        };
        let runs_dir = view
            .experiment
            .clone()
            .unwrap_or_else(|| format!("{}/runs", view.jobfiles));
        let mut plan = JobPlan::default();
        let mut env: Vec<(String, String)> = Vec::new();
        if let Some(profile) = input.profile {
            let ctx = h2s_core::env::EnvContext {
                runs_dir: runs_dir.clone(),
                opencode_config_path: format!("{}/opencode.json", view.jobfiles),
                gcp_credentials_path: format!("{}/gcp-credentials.json", view.jobfiles),
            };
            let je =
                h2s_core::env::build(profile, input.secrets, &ctx).map_err(|e| e.to_string())?;
            env.extend(je.vars.iter().cloned());
            if let Some(c) = &je.opencode_config {
                plan.files
                    .push(("opencode.json".to_owned(), Zeroizing::new(c.clone())));
            }
            if let Some(c) = &je.gcp_credentials_json {
                plan.files
                    .push(("gcp-credentials.json".to_owned(), Zeroizing::new(c.clone())));
            }
            plan.secrets = je
                .secrets
                .iter()
                .map(|(n, v)| (n.clone(), Zeroizing::new(v.clone())))
                .collect();
            plan.egress_hosts.clone_from(&je.egress_hosts);
        }
        if !env.iter().any(|(n, _)| n == "HARNESS2_RUNS_DIR") {
            env.push(("HARNESS2_RUNS_DIR".to_owned(), runs_dir));
        }
        let split_view = input
            .split_file
            .map(|_| format!("{}/split.json", view.jobfiles));
        let planner = Planner { input, view, env };
        let v = &planner.view;
        let step = |name: &str, inv: Result<Invocation, h2s_core::CoreError>, parse: ParseKind| {
            let inv = inv.map_err(|e| e.to_string())?;
            planner.h2_step(name, &inv, parse, false, false, false, false)
        };

        match input.spec {
            JobSpec::InstallHarness2(p) => {
                if !input.harness2_managed {
                    return Err("harness2_home is not managed by this server".into());
                }
                crate::util::ensure_private_dir(input.harness2_home)
                    .map_err(|e| format!("harness2 home: {e}"))?;
                let commit = p
                    .rev
                    .clone()
                    .unwrap_or_else(|| h2s_core::TESTED_HARNESS2_COMMIT.to_owned());
                let mut extras = vec![InstallExtra::Render];
                for e in &p.extras {
                    if !extras.contains(e) {
                        extras.push(*e);
                    }
                }
                // The plan inspects the checkout on the host; container jobs then see the
                // same paths under the fixed mount point.
                let host_home = text(input.harness2_home)?;
                let steps = h2s_core::install::plan(&host_home, &commit, &extras, p.reinstall)
                    .map_err(|e| e.to_string())?;
                let host_parent = input
                    .harness2_home
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let mount_parent = std::path::Path::new(HARNESS2_MOUNT)
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let map = |x: &str| -> String {
                    if !container {
                        return x.to_owned();
                    }
                    if x == host_home {
                        HARNESS2_MOUNT.to_owned()
                    } else if let Some(rest) = x.strip_prefix(&format!("{host_home}/")) {
                        format!("{HARNESS2_MOUNT}/{rest}")
                    } else if x == host_parent {
                        mount_parent.clone()
                    } else {
                        x.to_owned()
                    }
                };
                let last = steps.len().saturating_sub(1);
                for (i, s) in steps.iter().enumerate() {
                    let program = map(&s.program);
                    let args: Vec<String> = s.args.iter().map(|a| map(a)).collect();
                    let mut display = vec![program.clone()];
                    display.extend(args.iter().cloned());
                    plan.steps.push(planner.wrap(
                        s.name,
                        &program,
                        &args,
                        &map(&s.cwd),
                        display,
                        (
                            if i == last {
                                ParseKind::Install
                            } else {
                                ParseKind::None
                            },
                            ExitSemantics::Plain,
                        ),
                        (false, false, false),
                        (true, None),
                    )?);
                }
            }
            JobSpec::Demo(p) => {
                let out = v.experiment.clone().ok_or("the demo needs an experiment")?;
                let inv = cli::demo(p, &out).map_err(|e| e.to_string())?;
                // The demo prints the same `[domain] i/n status task` lines as a sweep.
                plan.steps.push(planner.h2_step(
                    "demo",
                    &inv,
                    ParseKind::Demo,
                    true,
                    false,
                    false,
                    false,
                )?);
            }
            JobSpec::Setup(p) => {
                let inv = cli::setup(p).map_err(|e| e.to_string())?;
                plan.steps.push(planner.h2_step(
                    "setup",
                    &inv,
                    ParseKind::Setup { dry_run: p.dry_run },
                    false,
                    false,
                    false,
                    true,
                )?);
            }
            JobSpec::Check(p) => {
                plan.steps
                    .push(step("check", cli::check(&p.cell), ParseKind::Check)?);
            }
            JobSpec::Preview(p) => plan.steps.push(step(
                "preview",
                cli::preview(&p.cell, &p.options, split_view.as_deref()),
                ParseKind::Preview,
            )?),
            JobSpec::Run(p) => {
                plan.steps.push(step(
                    "preview",
                    cli::preview(&p.cell, &p.options, split_view.as_deref()),
                    ParseKind::Preview,
                )?);
                let inv = cli::run(&p.cell, &p.options, split_view.as_deref())
                    .map_err(|e| e.to_string())?;
                plan.steps.push(planner.h2_step(
                    "run",
                    &inv,
                    ParseKind::Run,
                    true,
                    true,
                    false,
                    false,
                )?);
            }
            JobSpec::Judge(p) => plan.steps.push(step(
                if p.bases { "judge-bases" } else { "judge" },
                cli::judge(p),
                ParseKind::Judge { bases: p.bases },
            )?),
            JobSpec::Report(p) => {
                plan.steps
                    .push(step("report", cli::report(p), ParseKind::Report)?);
            }
            JobSpec::Split(p) => {
                // The script writes into <job dir>/out (the job-files mount stays
                // read-only; in a container `out` is mounted at the experiment point).
                let out_dir = input.job_dir.join(SPLIT_OUT_DIR);
                crate::util::ensure_private_dir(&out_dir)
                    .map_err(|e| format!("split output: {e}"))?;
                let out = if container {
                    format!("{EXPERIMENT_MOUNT}/{SPLIT_FILE}")
                } else {
                    format!("{}/{SPLIT_FILE}", text(&out_dir)?)
                };
                let inv = cli::split(p, &out).map_err(|e| e.to_string())?;
                let program = format!("{}/.venv/bin/python", v.home);
                let mut display = vec!["python".to_owned()];
                display.extend(inv.args.iter().cloned());
                plan.steps.push(planner.wrap(
                    "split",
                    &program,
                    &inv.args,
                    &v.home.clone(),
                    display,
                    (ParseKind::Split, ExitSemantics::Plain),
                    (false, false, false),
                    (false, Some(&out_dir)),
                )?);
            }
            JobSpec::Bridge(p) => {
                let model = p
                    .model
                    .clone()
                    .or_else(|| input.profile.and_then(|pr| pr.codex_model.clone()))
                    .ok_or("the bridge needs a model")?;
                let inv = cli::bridge(p, &model).map_err(|e| e.to_string())?;
                plan.steps.push(planner.h2_step(
                    "bridge",
                    &inv,
                    ParseKind::Bridge,
                    false,
                    false,
                    true,
                    false,
                )?);
            }
            JobSpec::Pipeline(p) => {
                plan.steps.push(step(
                    "preview",
                    cli::preview(&p.cell, &p.options, split_view.as_deref()),
                    ParseKind::Preview,
                )?);
                plan.steps
                    .push(step("check", cli::check(&p.cell), ParseKind::Check)?);
                let inv = cli::run(&p.cell, &p.options, split_view.as_deref())
                    .map_err(|e| e.to_string())?;
                plan.steps.push(planner.h2_step(
                    "run",
                    &inv,
                    ParseKind::Run,
                    true,
                    true,
                    false,
                    false,
                )?);
                plan.steps.push(step(
                    "judge",
                    cli::judge(&judge_of(p, false)),
                    ParseKind::Judge { bases: false },
                )?);
                if p.judge_bases {
                    plan.steps.push(step(
                        "judge-bases",
                        cli::judge(&judge_of(p, true)),
                        ParseKind::Judge { bases: true },
                    )?);
                }
                plan.steps.push(step(
                    "report",
                    cli::report(&report_of(p)),
                    ParseKind::Report,
                )?);
            }
        }
        Ok(plan)
    }

    fn parse_step(
        &self,
        step: &StepPlan,
        out: &StepOutput<'_>,
    ) -> Result<Option<JobResult>, String> {
        use h2s_core::parse;
        let ok = out.exit_code == Some(0);
        match step.parse {
            ParseKind::None | ParseKind::Bridge => Ok(None),
            ParseKind::Install => {
                if !ok {
                    return Ok(None);
                }
                let st = h2s_core::install::read_status(out.harness2_home, out.harness2_managed);
                match (st.installed, st.commit) {
                    (true, Some(commit)) => Ok(Some(JobResult::Install(InstallResult {
                        commit,
                        version: st.version.unwrap_or_default(),
                        python: st.python.unwrap_or_default(),
                        extras: st.extras,
                    }))),
                    _ => Err("harness2 is not importable after the install".to_owned()),
                }
            }
            ParseKind::Demo => {
                let Some((dir, _)) = out.experiment else {
                    return Ok(None);
                };
                match std::fs::read_to_string(dir.join("demo.json")) {
                    Ok(t) => parse::demo_json(&t)
                        .map(|d| Some(JobResult::Demo(d)))
                        .map_err(|e| e.to_string()),
                    Err(_) if !ok => Ok(None),
                    Err(e) => Err(format!("demo.json: {e}")),
                }
            }
            ParseKind::Setup { dry_run } => Ok(Some(JobResult::Setup(parse::setup_output(
                out.stdout, dry_run,
            )))),
            ParseKind::Check => Ok(Some(JobResult::Check(parse::check_output(out.stdout)))),
            ParseKind::Preview => {
                if !ok {
                    return Ok(None);
                }
                let pv = parse::preview_output(out.stdout).map_err(|e| e.to_string())?;
                let (k, modes): (u32, Vec<Mode>) = match out.spec {
                    JobSpec::Preview(p) => (p.options.k, p.options.modes.clone()),
                    JobSpec::Run(p) => (p.options.k, p.options.modes.clone()),
                    JobSpec::Pipeline(p) => (p.options.k, p.options.modes.clone()),
                    _ => {
                        let d = RunOptions::default();
                        (d.k, d.modes)
                    }
                };
                let hist = out.experiment.and_then(|(d, kind)| history(d, kind));
                let prices = out.profile.and_then(|p| p.prices.as_ref());
                let estimate =
                    h2s_core::estimate::estimate(pv.total_tasks, k, &modes, prices, hist);
                Ok(Some(JobResult::Preview(PreviewResult {
                    header: pv.header,
                    namespaces: pv.namespaces,
                    summary_namespace: pv.summary_namespace,
                    workers: pv.workers,
                    stream: pv.stream,
                    domains: pv.domains,
                    total_tasks: pv.total_tasks,
                    total_domains: pv.total_domains,
                    estimate,
                })))
            }
            ParseKind::Run => match parse::run_output(out.stdout) {
                Some(r) => Ok(Some(JobResult::Run(r))),
                None if ok => Err("the sweep printed no summary".to_owned()),
                None => Ok(None),
            },
            ParseKind::Judge { bases } => match parse::judge_output(out.stdout, bases) {
                Some(r) => Ok(Some(JobResult::Judge(r))),
                None if ok => Err("the judge printed no summary".to_owned()),
                None => Ok(None),
            },
            ParseKind::Report => Ok(ok.then(|| {
                JobResult::Report(ReportResult {
                    text: out.stdout.trim_end().to_owned(),
                })
            })),
            ParseKind::Split => {
                if !ok {
                    return Ok(None);
                }
                let JobSpec::Split(p) = out.spec else {
                    return Ok(None);
                };
                let t = std::fs::read_to_string(out.job_dir.join(SPLIT_OUT_DIR).join(SPLIT_FILE))
                    .map_err(|e| format!("split file: {e}"))?;
                let (train, test) = parse::split_file(&t).map_err(|e| e.to_string())?;
                Ok(Some(JobResult::Split(SplitResult {
                    name: p.name.clone(),
                    train,
                    test,
                })))
            }
        }
    }

    fn progress_line(&self, line: &str) -> Option<TaskProgress> {
        h2s_core::parse::progress_line(line)
    }

    fn bridge_line(&self, line: &str, acc: &mut BridgeSettings) -> bool {
        h2s_core::parse::bridge_line(line, acc)
    }

    fn spend_usd(
        &self,
        experiment_dir: &Path,
        kind: ExperimentKind,
        summary_ns: &str,
    ) -> Option<f64> {
        let ns = h2s_core::namespace::Namespace::parse(summary_ns)?;
        ExperimentDir::open(experiment_dir, kind, &[EXPERIMENT_MOUNT.to_owned()])
            .ok()?
            .spend_usd(&ns)
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2s_api::{Bench, Cell, CheckParams, DemoMode, DemoParams, Substrate};

    #[test]
    fn names_fail_closed_until_validated() {
        let spec = JobSpec::Demo(DemoParams {
            experiment: "x".into(),
            k: 1,
            mode: DemoMode::Both,
        });
        // With the core stubs this reports "cannot be validated"; with core it is empty.
        let problems = CoreBackend.check_names(&spec);
        assert!(problems.iter().all(|p| p.field == "params.experiment"));
    }

    #[test]
    fn mount_sources_are_checked() {
        let d = tempfile::tempdir().unwrap();
        assert!(mount_source(d.path()).is_ok());
        let colon = d.path().join("a:b");
        std::fs::create_dir(&colon).unwrap();
        assert!(mount_source(&colon).is_err());
        let link = d.path().join("link");
        std::os::unix::fs::symlink(d.path(), &link).unwrap();
        assert!(mount_source(&link).is_err());
        assert!(mount_source(Path::new("relative")).is_err());
    }

    #[test]
    fn plans_a_host_check() {
        let d = tempfile::tempdir().unwrap();
        let snapshot = crate::secrets::SecretSnapshot::default();
        let spec = JobSpec::Check(CheckParams {
            cell: Cell {
                bench: Bench::Lab,
                subset: None,
                substrate: Substrate::Oc,
            },
            profile: None,
        });
        let input = PlanInput {
            job_id: "j",
            spec: &spec,
            isolation: Isolation::Host,
            harness2_home: d.path(),
            harness2_managed: true,
            experiment: None,
            job_dir: d.path(),
            split_file: None,
            profile: None,
            secrets: &snapshot,
            container: None,
        };
        let plan = CoreBackend.plan(&input).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert!(plan.steps[0].program.ends_with("/.venv/bin/harness2-check"));
        assert!(plan.steps[0].args.contains(&"lab".to_owned()));
    }
}
