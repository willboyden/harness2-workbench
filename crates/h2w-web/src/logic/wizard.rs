//! The New-run wizard: form state, presets, validation mirrors and the job specs it sends.

use h2w_api::{
    Bench, Cell, DomainCount, EstimateRequest, JobSpec, K_MAX, MAX_TASKS_MAX, Mode, PipelineParams,
    PreviewParams, RunOptions, RunParams, Split, Substrate, WORKERS_MAX, WbSubset,
};

/// `(field, problem)` pairs, as `RunOptions::validate` reports them.
pub(crate) type Problems = Vec<(String, String)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Cell,
    Options,
    Preview,
    Estimate,
    Confirm,
}

impl Step {
    pub(crate) const ALL: [Step; 5] = [
        Step::Cell,
        Step::Options,
        Step::Preview,
        Step::Estimate,
        Step::Confirm,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Step::Cell => "Experiment & benchmark",
            Step::Options => "Options",
            Step::Preview => "Preview",
            Step::Estimate => "Estimate",
            Step::Confirm => "Confirm",
        }
    }

    pub(crate) fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    pub(crate) fn next(self) -> Step {
        Step::ALL.get(self.index() + 1).copied().unwrap_or(self)
    }

    pub(crate) fn prev(self) -> Step {
        self.index()
            .checked_sub(1)
            .and_then(|i| Step::ALL.get(i).copied())
            .unwrap_or(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Preset {
    Smoke,
    Pilot,
    Paper,
}

impl Preset {
    pub(crate) const ALL: [Preset; 3] = [Preset::Smoke, Preset::Pilot, Preset::Paper];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Preset::Smoke => "Smoke test",
            Preset::Pilot => "Pilot",
            Preset::Paper => "Paper-style",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Preset::Smoke => {
                "1 domain, 1 task, k = 1, parallel only. Proves the setup works for a few cents."
            }
            Preset::Pilot => {
                "k = 1, both modes, the first 3 tasks of every domain. A cheap first signal."
            }
            Preset::Paper => {
                "k = 3, both modes, every task. The configuration the paper reports; costs the most."
            }
        }
    }
}

/// What a run asks about host isolation before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostRisk {
    /// Container isolation: nothing to acknowledge.
    Contained,
    /// Host isolation: the user ticks the acknowledgement, sent as `acknowledge_risks: true`.
    AskUser,
    /// Host isolation that an administrator pre-acknowledged for everyone
    /// (`isolation.preacknowledge_host_risks`): no checkbox, a one-line note, and
    /// `acknowledge_risks: false` because the server does not require it.
    PreAcknowledged,
}

impl HostRisk {
    /// From the server's isolation and its `host_risks_preacknowledged` flag; unknown
    /// isolation (system info not loaded) asks, which is the safe side.
    pub(crate) fn from_system(host_isolation: bool, preacknowledged: bool) -> HostRisk {
        match (host_isolation, preacknowledged) {
            (false, _) => HostRisk::Contained,
            (true, false) => HostRisk::AskUser,
            (true, true) => HostRisk::PreAcknowledged,
        }
    }
}

/// Everything the wizard collects. Numeric fields stay text so a half-typed value is kept.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RunForm {
    pub(crate) experiment: String,
    pub(crate) profile: String,
    pub(crate) bench: Bench,
    pub(crate) subset: WbSubset,
    pub(crate) substrate: Substrate,
    pub(crate) k: String,
    pub(crate) parallel: bool,
    pub(crate) sequential: bool,
    pub(crate) seed: String,
    pub(crate) split: Split,
    pub(crate) split_file: String,
    pub(crate) workers: String,
    pub(crate) only_domains: Vec<String>,
    pub(crate) max_tasks: String,
    pub(crate) budget_usd: String,
    /// Run check -> run -> judge -> report instead of the sweep alone.
    pub(crate) pipeline: bool,
    pub(crate) judge_bases: bool,
    pub(crate) acknowledge_risks: bool,
}

impl Default for RunForm {
    fn default() -> Self {
        RunForm {
            experiment: String::new(),
            profile: String::new(),
            bench: Bench::Lab,
            subset: WbSubset::Office,
            substrate: Substrate::Oc,
            k: "1".to_owned(),
            parallel: true,
            sequential: false,
            seed: String::new(),
            split: Split::All,
            split_file: String::new(),
            workers: "1".to_owned(),
            only_domains: Vec::new(),
            max_tasks: String::new(),
            budget_usd: String::new(),
            pipeline: true,
            judge_bases: false,
            acknowledge_risks: false,
        }
    }
}

fn parse_u32(field: &str, s: &str, min: u32, max: u32, problems: &mut Problems) -> Option<u32> {
    match s.trim().parse::<u32>() {
        Ok(v) if (min..=max).contains(&v) => Some(v),
        _ => {
            problems.push((
                field.to_owned(),
                format!("must be a whole number between {min} and {max}"),
            ));
            None
        }
    }
}

impl RunForm {
    pub(crate) fn cell(&self) -> Cell {
        Cell {
            bench: self.bench,
            subset: (self.bench == Bench::Wb).then_some(self.subset),
            substrate: self.substrate,
        }
    }

    pub(crate) fn modes(&self) -> Vec<Mode> {
        let mut v = Vec::new();
        if self.parallel {
            v.push(Mode::Parallel);
        }
        if self.sequential {
            v.push(Mode::Sequential);
        }
        v
    }

    /// The `k` value when it parses, for live arithmetic while typing.
    pub(crate) fn k_value(&self) -> Option<u32> {
        self.k
            .trim()
            .parse()
            .ok()
            .filter(|k| (1..=K_MAX).contains(k))
    }

    pub(crate) fn max_tasks_value(&self) -> Option<u32> {
        self.max_tasks.trim().parse().ok().filter(|n| *n >= 1)
    }

    /// Parse and validate the options (mirrors `RunOptions::validate`, plus number parsing).
    pub(crate) fn options(&self) -> Result<RunOptions, Problems> {
        let mut problems = Problems::new();
        let k = parse_u32("k", &self.k, 1, K_MAX, &mut problems);
        let workers = parse_u32("workers", &self.workers, 1, WORKERS_MAX, &mut problems);
        let seed = if self.seed.trim().is_empty() {
            None
        } else if let Ok(v) = self.seed.trim().parse::<u64>() {
            Some(v)
        } else {
            problems.push(("seed".into(), "must be a whole number, or empty".into()));
            None
        };
        let max_tasks = if self.max_tasks.trim().is_empty() {
            None
        } else {
            parse_u32(
                "max_tasks",
                &self.max_tasks,
                1,
                MAX_TASKS_MAX,
                &mut problems,
            )
        };
        let opts = RunOptions {
            k: k.unwrap_or(1),
            seed,
            modes: self.modes(),
            split: self.split,
            split_file: match self.split {
                Split::All => None,
                Split::Train | Split::Test => {
                    Some(self.split_file.trim().to_owned()).filter(|s| !s.is_empty())
                }
            },
            workers: workers.unwrap_or(1),
            only_domains: self.only_domains.clone(),
            max_tasks,
        };
        if let Err(more) = opts.validate() {
            for p in more {
                if !problems.iter().any(|(f, _)| *f == p.0) {
                    problems.push(p);
                }
            }
        }
        if problems.is_empty() {
            Ok(opts)
        } else {
            Err(problems)
        }
    }

    /// Problems with the first step (experiment, profile, benchmark cell).
    pub(crate) fn cell_problems(&self) -> Problems {
        let mut p = Problems::new();
        if self.experiment.is_empty() {
            p.push(("experiment".into(), "choose or create an experiment".into()));
        }
        if self.profile.is_empty() {
            p.push(("profile".into(), "choose a model profile".into()));
        }
        if let Err(e) = self.cell().validate() {
            p.push(("cell".into(), e));
        }
        p
    }

    /// `harness2 --dry-run` with the chosen options.
    pub(crate) fn preview_spec(&self) -> Result<JobSpec, Problems> {
        Ok(JobSpec::Preview(PreviewParams {
            experiment: self.experiment.clone(),
            cell: self.cell(),
            options: self.options()?,
            profile: Some(self.profile.clone()).filter(|p| !p.is_empty()),
        }))
    }

    /// A preview without domain or task filters, to list every domain for the picker.
    pub(crate) fn domains_preview_spec(&self) -> JobSpec {
        JobSpec::Preview(PreviewParams {
            experiment: self.experiment.clone(),
            cell: self.cell(),
            options: RunOptions {
                k: self.k_value().unwrap_or(1),
                modes: if self.modes().is_empty() {
                    vec![Mode::Parallel]
                } else {
                    self.modes()
                },
                ..RunOptions::default()
            },
            profile: Some(self.profile.clone()).filter(|p| !p.is_empty()),
        })
    }

    pub(crate) fn budget_value(&self) -> Result<Option<f64>, String> {
        let t = self.budget_usd.trim().trim_start_matches('$');
        if t.is_empty() {
            return Ok(None);
        }
        match t.parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 => Ok(Some(v)),
            _ => Err("enter a positive amount in US dollars, or leave it empty".to_owned()),
        }
    }

    /// The job to start: a `pipeline` (check, run, judge, report) or a bare `run`.
    pub(crate) fn submit_spec(&self, risk: HostRisk) -> Result<JobSpec, Problems> {
        let mut problems = self.cell_problems();
        let options = match self.options() {
            Ok(o) => Some(o),
            Err(p) => {
                problems.extend(p);
                None
            }
        };
        let budget = match self.budget_value() {
            Ok(b) => b,
            Err(e) => {
                problems.push(("budget_usd".into(), e));
                None
            }
        };
        if risk == HostRisk::AskUser && !self.acknowledge_risks {
            problems.push((
                "acknowledge_risks".into(),
                "confirm that you understand the executor runs directly on the server".into(),
            ));
        }
        let Some(options) = options.filter(|_| problems.is_empty()) else {
            return Err(problems);
        };
        // Only an acknowledgement the user gave is sent; a pre-acknowledging server needs none.
        let acknowledge_risks = risk == HostRisk::AskUser && self.acknowledge_risks;
        Ok(if self.pipeline {
            JobSpec::Pipeline(PipelineParams {
                experiment: self.experiment.clone(),
                cell: self.cell(),
                options,
                profile: self.profile.clone(),
                judge_bases: self.judge_bases,
                budget_usd: budget,
                acknowledge_risks,
            })
        } else {
            JobSpec::Run(RunParams {
                experiment: self.experiment.clone(),
                cell: self.cell(),
                options,
                profile: self.profile.clone(),
                budget_usd: budget,
                acknowledge_risks,
            })
        })
    }

    pub(crate) fn estimate_request(&self, tasks: u32) -> EstimateRequest {
        EstimateRequest {
            tasks,
            k: self.k_value().unwrap_or(1),
            modes: self.modes(),
            profile: Some(self.profile.clone()).filter(|p| !p.is_empty()),
            experiment: Some(self.experiment.clone()).filter(|e| !e.is_empty()),
        }
    }

    /// Apply a preset. `domains` are the known domains in stream order (may be empty).
    pub(crate) fn apply_preset(&mut self, preset: Preset, domains: &[DomainCount]) {
        self.split = Split::All;
        self.split_file.clear();
        match preset {
            Preset::Smoke => {
                self.k = "1".into();
                self.parallel = true;
                self.sequential = false;
                self.max_tasks = "1".into();
                self.only_domains = domains
                    .first()
                    .map(|d| d.domain.clone())
                    .into_iter()
                    .collect();
                self.workers = "1".into();
            }
            Preset::Pilot => {
                self.k = "1".into();
                self.parallel = true;
                self.sequential = true;
                self.max_tasks = "3".into();
                self.only_domains.clear();
            }
            Preset::Paper => {
                self.k = "3".into();
                self.parallel = true;
                self.sequential = true;
                self.max_tasks.clear();
                self.only_domains.clear();
            }
        }
    }
}

/// Tasks a sweep would run, from known per-domain counts (before a preview has run).
pub(crate) fn expected_tasks(domains: &[DomainCount], only: &[String], max: Option<u32>) -> u32 {
    domains
        .iter()
        .filter(|d| only.is_empty() || only.contains(&d.domain))
        .map(|d| max.map_or(d.tasks, |m| d.tasks.min(m)))
        .sum()
}

/// Rough counts from upstream's budget identity: per task, k shared base rollouts plus 3k
/// per mode; improver sessions 3k per task and mode, plus up to one selection (when k > 1)
/// and one reflection per task and mode. Returns `(rollouts, sessions_min, sessions_max)`.
pub(crate) fn quick_counts(tasks: u32, k: u32, n_modes: u32) -> (u64, u64, u64) {
    let (t, k, m) = (u64::from(tasks), u64::from(k), u64::from(n_modes));
    if m == 0 {
        return (0, 0, 0);
    }
    let rollouts = t * (k + 3 * k * m);
    let sessions_min = t * m * 3 * k;
    let extra = t * m * (u64::from(k > 1) + 1);
    (rollouts, sessions_min, sessions_min + extra)
}

/// Turn `(field, problem)` pairs into readable sentences.
pub(crate) fn describe(problems: &Problems) -> Vec<String> {
    problems
        .iter()
        .map(|(f, p)| {
            let field = match f.as_str() {
                "k" => "Budget k",
                "modes" => "Modes",
                "workers" => "Workers",
                "max_tasks" => "Tasks per domain",
                "split_file" => "Split file",
                "seed" => "Seed",
                "budget_usd" => "Spend cap",
                "experiment" => "Experiment",
                "profile" => "Profile",
                "cell" => "Benchmark",
                "acknowledge_risks" => "Risk acknowledgement",
                other => other,
            };
            format!("{field}: {p}")
        })
        .collect()
}

/// Task ids typed one per line (blank lines and `#` comments ignored, spaces trimmed), in order.
pub(crate) fn task_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn task_lines_skip_blanks_and_comments() {
        assert_eq!(
            task_lines("  producers/task1 \n\n# self-contained\ndata_entry_keyers/task1\n"),
            ["producers/task1", "data_entry_keyers/task1"]
        );
        assert!(task_lines(" \n#\n").is_empty());
    }

    use super::*;

    fn domains() -> Vec<DomainCount> {
        vec![
            DomainCount {
                domain: "Contracts".into(),
                tasks: 5,
            },
            DomainCount {
                domain: "Employment".into(),
                tasks: 2,
            },
        ]
    }

    fn filled() -> RunForm {
        RunForm {
            experiment: "lab-pilot".into(),
            profile: "vertex".into(),
            ..RunForm::default()
        }
    }

    #[test]
    fn steps_walk() {
        assert_eq!(Step::Cell.next(), Step::Options);
        assert_eq!(Step::Confirm.next(), Step::Confirm);
        assert_eq!(Step::Cell.prev(), Step::Cell);
        assert_eq!(Step::Estimate.prev(), Step::Preview);
    }

    #[test]
    fn presets() {
        let mut f = filled();
        f.apply_preset(Preset::Smoke, &domains());
        let o = f.options().unwrap();
        assert_eq!((o.k, o.max_tasks), (1, Some(1)));
        assert_eq!(o.modes, vec![Mode::Parallel]);
        assert_eq!(o.only_domains, vec!["Contracts"]);
        f.apply_preset(Preset::Pilot, &domains());
        let o = f.options().unwrap();
        assert_eq!(o.modes, vec![Mode::Parallel, Mode::Sequential]);
        assert_eq!(o.max_tasks, Some(3));
        assert!(o.only_domains.is_empty());
        f.apply_preset(Preset::Paper, &[]);
        let o = f.options().unwrap();
        assert_eq!((o.k, o.max_tasks), (3, None));
    }

    #[test]
    fn option_problems() {
        let f = RunForm {
            k: "0".into(),
            workers: "abc".into(),
            seed: "-1".into(),
            max_tasks: "0".into(),
            parallel: false,
            split: Split::Test,
            ..filled()
        };
        let fields: Vec<String> = f
            .options()
            .unwrap_err()
            .into_iter()
            .map(|(f, _)| f)
            .collect();
        for want in ["k", "workers", "seed", "max_tasks", "modes", "split_file"] {
            assert!(
                fields.iter().any(|f| f == want),
                "missing {want}: {fields:?}"
            );
        }
        let ok = RunForm {
            split: Split::Train,
            split_file: "lab-half".into(),
            seed: "7".into(),
            ..filled()
        };
        let o = ok.options().unwrap();
        assert_eq!(o.split_file.as_deref(), Some("lab-half"));
        assert_eq!(o.seed, Some(7));
    }

    #[test]
    fn specs() {
        let mut f = filled();
        f.bench = Bench::Wb;
        f.subset = WbSubset::Web;
        assert!(
            matches!(f.preview_spec().unwrap(), JobSpec::Preview(p) if p.cell.subset == Some(WbSubset::Web))
        );
        f.bench = Bench::Jb;
        assert_eq!(f.cell().subset, None);
        assert!(f.submit_spec(HostRisk::AskUser).is_err());
        f.acknowledge_risks = true;
        f.budget_usd = "$25".into();
        match f.submit_spec(HostRisk::AskUser).unwrap() {
            JobSpec::Pipeline(p) => {
                assert_eq!(p.budget_usd, Some(25.0));
                assert!(p.acknowledge_risks);
            }
            other => panic!("expected pipeline, got {other:?}"),
        }
        f.pipeline = false;
        assert!(matches!(
            f.submit_spec(HostRisk::Contained).unwrap(),
            JobSpec::Run(_)
        ));
        f.budget_usd = "-3".into();
        assert!(f.submit_spec(HostRisk::Contained).is_err());
        let empty = RunForm {
            acknowledge_risks: true,
            ..RunForm::default()
        };
        let problems = empty.submit_spec(HostRisk::Contained).unwrap_err();
        assert!(
            describe(&problems)
                .iter()
                .any(|s| s.starts_with("Experiment:"))
        );
    }

    #[test]
    fn host_risk_acknowledgement() {
        assert_eq!(HostRisk::from_system(false, false), HostRisk::Contained);
        assert_eq!(HostRisk::from_system(false, true), HostRisk::Contained);
        assert_eq!(HostRisk::from_system(true, false), HostRisk::AskUser);
        assert_eq!(HostRisk::from_system(true, true), HostRisk::PreAcknowledged);
        let ack = |spec: JobSpec| match spec {
            JobSpec::Pipeline(p) => p.acknowledge_risks,
            JobSpec::Run(r) => r.acknowledge_risks,
            other => panic!("unexpected {other:?}"),
        };
        let f = filled();
        assert!(!f.acknowledge_risks);
        // Asked: refused until ticked, then sent.
        assert!(f.submit_spec(HostRisk::AskUser).is_err());
        let ticked = RunForm {
            acknowledge_risks: true,
            ..f.clone()
        };
        assert!(ack(ticked.submit_spec(HostRisk::AskUser).unwrap()));
        // Pre-acknowledged by the server: no tick needed and none sent, even if one lingers.
        assert!(!ack(f.submit_spec(HostRisk::PreAcknowledged).unwrap()));
        assert!(!ack(ticked.submit_spec(HostRisk::PreAcknowledged).unwrap()));
        // Container isolation: nothing to acknowledge.
        assert!(!ack(f.submit_spec(HostRisk::Contained).unwrap()));
    }

    #[test]
    fn domain_preview_has_no_filters() {
        let mut f = filled();
        f.apply_preset(Preset::Smoke, &domains());
        match f.domains_preview_spec() {
            JobSpec::Preview(p) => {
                assert!(p.options.only_domains.is_empty());
                assert_eq!(p.options.max_tasks, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn counts() {
        assert_eq!(expected_tasks(&domains(), &[], None), 7);
        assert_eq!(expected_tasks(&domains(), &[], Some(3)), 5);
        assert_eq!(
            expected_tasks(&domains(), &["Employment".into()], Some(1)),
            1
        );
        // k = 1, one mode: 4k rollouts and 3k sessions per task (+ reflection).
        assert_eq!(quick_counts(1, 1, 1), (4, 3, 4));
        // k = 3, both modes: 3 + 18 rollouts; 18 sessions plus 2 x (selection + reflection).
        assert_eq!(quick_counts(1, 3, 2), (21, 18, 22));
        assert_eq!(quick_counts(10, 2, 0), (0, 0, 0));
        let f = filled();
        let r = f.estimate_request(12);
        assert_eq!((r.tasks, r.k), (12, 1));
        assert_eq!(r.experiment.as_deref(), Some("lab-pilot"));
    }
}
