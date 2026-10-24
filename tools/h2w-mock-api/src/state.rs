//! The mock's in-memory state and its fixture population.

use std::collections::{BTreeMap, HashMap};

use h2w_api::{
    AuditEvent, AuditOutcome, Bench, BenchmarkStatus, Cell, CheckLevel, CheckParams, CheckResult,
    DemoMode, DemoParams, DemoResult, DoctorCheck, DoctorReport, DomainCount, Endpoint, Experiment,
    ExperimentKind, InstallExtra, InstallParams, InstallResult, Isolation, Job, JobEvent,
    JobResult, JobSpec, JobStatus, JobStep, JudgeParams, JudgeResult, LogLine, LogStream, Mode,
    ModelProfile, PipelineParams, PipelineResult, Prices, Principal, ProfileKind, ReportParams,
    ReportResult, Role, RunOptions, RunParams, RunResult, SecretInfo, SetupParams, SetupResult,
    SplitInfo, SplitParams, SplitResult, Substrate, TokenInfo, VertexSettings, WbSubset,
};
use tokio::sync::broadcast;

use crate::sweeps::{self, SweepSpec, TaskSpec, TaskState};
use crate::util::{ago, fake_hash, now_secs, rfc3339};

/// The commit this mock pretends the Workbench was tested against (fictional).
pub(crate) const TESTED_COMMIT: &str = "1cfe2540a8d4c7e19b3f6a2d5e8c0b47f9a13d6e";

pub(crate) struct ExpData {
    pub(crate) exp: Experiment,
    pub(crate) sweeps: Vec<SweepSpec>,
}

impl ExpData {
    /// The experiment as the API returns it (sweep summaries regenerated).
    pub(crate) fn view(&self) -> Experiment {
        let mut e = self.exp.clone();
        e.sweeps = self.sweeps.iter().map(sweeps::summary).collect();
        e.size_bytes = Some(1_048_576 * (3 + 11 * self.sweeps.len() as u64));
        e
    }

    pub(crate) fn sweep(&self, id: &str) -> Option<&SweepSpec> {
        self.sweeps.iter().find(|s| s.id == id)
    }
}

pub(crate) struct JobData {
    pub(crate) job: Job,
    pub(crate) log: Vec<LogLine>,
    pub(crate) tx: broadcast::Sender<JobEvent>,
    pub(crate) cancel: bool,
}

pub(crate) struct Inner {
    pub(crate) installed: bool,
    pub(crate) isolation: Isolation,
    /// `isolation.preacknowledge_host_risks`: runs on the host need no acknowledgement.
    pub(crate) host_risks_preacknowledged: bool,
    pub(crate) experiments: BTreeMap<String, ExpData>,
    pub(crate) jobs: Vec<JobData>,
    pub(crate) profiles: BTreeMap<String, ModelProfile>,
    pub(crate) secrets: BTreeMap<String, SecretInfo>,
    pub(crate) tokens: Vec<TokenInfo>,
    pub(crate) sessions: HashMap<String, Principal>,
    pub(crate) audit: Vec<AuditEvent>,
    pub(crate) splits: Vec<SplitInfo>,
    pub(crate) benches: Vec<BenchmarkStatus>,
}

impl Inner {
    pub(crate) fn job_index(&self, id: &str) -> Option<usize> {
        self.jobs.iter().position(|j| j.job.id == id)
    }

    /// Append a line to a job's log and broadcast it.
    pub(crate) fn log(&mut self, idx: usize, stream: LogStream, text: &str) {
        let Some(j) = self.jobs.get_mut(idx) else {
            return;
        };
        let line = LogLine {
            n: j.log.len() as u64,
            ts: rfc3339(now_secs()),
            stream,
            text: text.to_owned(),
        };
        j.log.push(line.clone());
        j.job.log_lines = j.log.len() as u64;
        let _ = j.tx.send(JobEvent::Log { line });
    }

    pub(crate) fn emit(&mut self, idx: usize, ev: JobEvent) {
        if let Some(j) = self.jobs.get_mut(idx) {
            let _ = j.tx.send(ev);
        }
    }

    pub(crate) fn add_audit(
        &mut self,
        actor: &str,
        action: &str,
        target: &str,
        outcome: AuditOutcome,
        detail: Option<String>,
    ) {
        let seq = self.audit.len() as u64 + 1;
        let prev_hash = self
            .audit
            .last()
            .map_or_else(|| "0".repeat(64), |a| a.hash.clone());
        let ts = rfc3339(now_secs());
        let hash = fake_hash(&format!("{prev_hash}{seq}{ts}{actor}{action}{target}"));
        self.audit.push(AuditEvent {
            seq,
            ts,
            actor: actor.to_owned(),
            action: action.to_owned(),
            target: target.to_owned(),
            outcome,
            ip: Some("127.0.0.1".to_owned()),
            request_id: Some(crate::util::hex(8)),
            detail,
            prev_hash,
            hash,
        });
    }

    /// Profiles with their secret references resolved against the stored secrets.
    pub(crate) fn profile_view(&self, p: &ModelProfile) -> ModelProfile {
        let mut out = p.clone();
        let mut names: Vec<String> = Vec::new();
        for e in [&p.endpoint, &p.judge_endpoint, &p.codex_endpoint]
            .into_iter()
            .flatten()
        {
            if let Some(s) = &e.api_key_secret {
                names.push(s.clone());
            }
        }
        if let Some(v) = &p.vertex
            && let Some(s) = &v.credentials_secret
        {
            names.push(s.clone());
        }
        for e in &p.extra_env {
            if let Some(s) = &e.secret {
                names.push(s.clone());
            }
        }
        names.sort();
        names.dedup();
        out.secrets = names
            .into_iter()
            .map(|n| {
                self.secrets.get(&n).cloned().unwrap_or(SecretInfo {
                    name: n,
                    set: false,
                    chars: None,
                    updated_at: None,
                })
            })
            .collect();
        out
    }
}

fn cell(bench: Bench, subset: Option<WbSubset>, substrate: Substrate) -> Cell {
    Cell {
        bench,
        subset,
        substrate,
    }
}

pub(crate) fn lab_domains() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        (
            "Contracts",
            vec![
                "nda-review-014",
                "supply-terms-022",
                "saas-renewal-031",
                "license-scope-040",
            ],
        ),
        (
            "Employment",
            vec![
                "handbook-update-003",
                "noncompete-memo-017",
                "leave-policy-026",
            ],
        ),
        (
            "Litigation",
            vec![
                "discovery-plan-008",
                "motion-outline-019",
                "deposition-prep-033",
            ],
        ),
        ("Corporate", vec!["board-minutes-005", "share-transfer-012"]),
    ]
}

pub(crate) fn jb_domains() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        (
            "Accountants and Auditors",
            vec![
                "reconcile-ledger-101",
                "variance-memo-114",
                "audit-sample-120",
            ],
        ),
        (
            "Market Research Analysts",
            vec!["survey-brief-205", "competitor-scan-211"],
        ),
        (
            "Software Developers",
            vec![
                "design-doc-302",
                "migration-plan-318",
                "incident-review-327",
            ],
        ),
        ("Paralegals", vec!["case-index-410", "exhibit-list-415"]),
    ]
}

pub(crate) fn wb_domains(sub: WbSubset) -> Vec<(&'static str, Vec<&'static str>)> {
    match sub {
        WbSubset::Office => vec![
            (
                "Spreadsheets",
                vec!["budget-rollup-01", "pivot-summary-02", "forecast-03"],
            ),
            ("Documents", vec!["policy-redline-04", "minutes-05"]),
            (
                "Presentations",
                vec!["quarterly-deck-06", "pitch-outline-07"],
            ),
        ],
        WbSubset::Web => vec![
            ("Forms", vec!["signup-flow-01", "expense-claim-02"]),
            ("Research", vec!["price-check-03", "vendor-compare-04"]),
        ],
        WbSubset::Code => vec![
            (
                "Bug fixes",
                vec!["off-by-one-01", "null-guard-02", "timezone-03"],
            ),
            ("Features", vec!["csv-export-04", "pagination-05"]),
        ],
    }
}

pub(crate) fn domains_for(c: &Cell) -> Vec<(&'static str, Vec<&'static str>)> {
    match (c.bench, c.subset) {
        (Bench::Jb, _) => jb_domains(),
        (Bench::Lab, _) => lab_domains(),
        (Bench::Wb, Some(s)) => wb_domains(s),
        (Bench::Wb, None) => wb_domains(WbSubset::Office),
    }
}

fn tasks_from(domains: &[(&str, Vec<&str>)], states: &dyn Fn(usize) -> TaskState) -> Vec<TaskSpec> {
    let mut out = Vec::new();
    for (d, ids) in domains {
        let prefix = d.to_lowercase().replace(' ', "-");
        for id in ids {
            let n = out.len();
            out.push(TaskSpec {
                id: format!("{prefix}/{id}"),
                domain: (*d).to_owned(),
                state: states(n),
            });
        }
    }
    out
}

fn experiment(
    name: &str,
    kind: ExperimentKind,
    description: &str,
    tags: &[&str],
    profile: Option<&str>,
    created_ago: i64,
    archived: bool,
) -> Experiment {
    Experiment {
        name: name.to_owned(),
        kind,
        description: description.to_owned(),
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        profile: profile.map(str::to_owned),
        created_at: ago(created_ago),
        created_by: "admin".to_owned(),
        archived,
        sweeps: Vec::new(),
        size_bytes: None,
    }
}

const DAY: i64 = 86_400;

fn vertex_models() -> (String, String) {
    (
        "google-vertex/gemini-3.5-flash".to_owned(),
        "google-vertex/gemini-3.5-pro".to_owned(),
    )
}

fn local_models() -> (String, String) {
    (
        "h2w/open-model-32b".to_owned(),
        "h2w/open-model-32b".to_owned(),
    )
}

fn demo_sweep() -> SweepSpec {
    SweepSpec {
        id: "demo-oc-agg2".to_owned(),
        bench_name: "demo".to_owned(),
        cell: None,
        k: 2,
        seed: None,
        modes: vec![Mode::Parallel, Mode::Sequential],
        tasks: tasks_from(
            &[
                ("writing", vec!["release-notes", "status-update"]),
                ("analysis", vec!["latency-table", "cost-summary"]),
            ],
            &|_| TaskState::Ok,
        ),
        judged: false,
        base_judged: false,
        updated_at: ago(3 * DAY),
        models: (
            "demo/local-editor".to_owned(),
            "demo/local-editor".to_owned(),
        ),
    }
}

pub(crate) fn demo_experiment() -> ExpData {
    ExpData {
        exp: experiment(
            "demo",
            ExperimentKind::Demo,
            "harness2-demo output: the real recursion with deterministic local editors.",
            &["demo"],
            None,
            3 * DAY,
            false,
        ),
        sweeps: vec![demo_sweep()],
    }
}

fn experiments() -> BTreeMap<String, ExpData> {
    let mut m = BTreeMap::new();
    m.insert("demo".to_owned(), demo_experiment());
    let lab = cell(Bench::Lab, None, Substrate::Oc);
    m.insert(
        "lab-pilot".to_owned(),
        ExpData {
            exp: experiment(
                "lab-pilot",
                ExperimentKind::Standard,
                "First LAB pilot: does k=1 help on legal drafting at all?",
                &["pilot", "lab"],
                Some("vertex-flash"),
                2 * DAY,
                false,
            ),
            sweeps: vec![
                SweepSpec {
                    id: "lab-oc-agg1".to_owned(),
                    bench_name: "lab".to_owned(),
                    cell: Some(lab),
                    k: 1,
                    seed: None,
                    modes: vec![Mode::Parallel, Mode::Sequential],
                    tasks: tasks_from(&lab_domains(), &|i| match i {
                        5 => TaskState::Partial,
                        9 => TaskState::BaseFailed,
                        _ => TaskState::Ok,
                    }),
                    judged: true,
                    base_judged: true,
                    updated_at: ago(DAY + 3600),
                    models: vertex_models(),
                },
                SweepSpec {
                    id: "lab-oc-agg3-s7".to_owned(),
                    bench_name: "lab".to_owned(),
                    cell: Some(lab),
                    k: 3,
                    seed: Some(7),
                    modes: vec![Mode::Parallel],
                    tasks: tasks_from(&lab_domains()[..3], &|i| match i {
                        0..=5 => TaskState::Ok,
                        6 => TaskState::Partial,
                        _ => TaskState::Running,
                    }),
                    judged: false,
                    base_judged: false,
                    updated_at: ago(600),
                    models: vertex_models(),
                },
            ],
        },
    );
    let jb = cell(Bench::Jb, None, Substrate::Cx);
    m.insert(
        "jobbench-baseline".to_owned(),
        ExpData {
            exp: experiment(
                "jobbench-baseline",
                ExperimentKind::Standard,
                "JobBench with the Codex executor on a self-hosted model.",
                &["baseline", "codex"],
                Some("local-model"),
                6 * DAY,
                false,
            ),
            sweeps: vec![SweepSpec {
                id: "jb-cx-agg2".to_owned(),
                bench_name: "jb".to_owned(),
                cell: Some(jb),
                k: 2,
                seed: None,
                modes: vec![Mode::Parallel, Mode::Sequential],
                tasks: tasks_from(&jb_domains(), &|i| {
                    if i == 4 {
                        TaskState::Partial
                    } else {
                        TaskState::Ok
                    }
                }),
                judged: true,
                base_judged: false,
                updated_at: ago(4 * DAY),
                models: local_models(),
            }],
        },
    );
    m.insert(
        "wb-office-trial".to_owned(),
        ExpData {
            exp: experiment(
                "wb-office-trial",
                ExperimentKind::Standard,
                "WorkBuddy office subset, waiting for Docker access on the server.",
                &["workbuddy"],
                Some("local-model"),
                DAY,
                false,
            ),
            sweeps: Vec::new(),
        },
    );
    m.insert(
        "old-smoke".to_owned(),
        ExpData {
            exp: experiment(
                "old-smoke",
                ExperimentKind::Standard,
                "Early smoke test (archived).",
                &["smoke"],
                Some("vertex-flash"),
                20 * DAY,
                true,
            ),
            sweeps: vec![SweepSpec {
                id: "lab-oc-agg1-smoke".to_owned(),
                bench_name: "lab".to_owned(),
                cell: Some(lab),
                k: 1,
                seed: None,
                modes: vec![Mode::Parallel],
                tasks: tasks_from(&lab_domains()[..1], &|_| TaskState::Ok)[..1].to_vec(),
                judged: true,
                base_judged: false,
                updated_at: ago(19 * DAY),
                models: vertex_models(),
            }],
        },
    );
    m
}

fn profiles() -> BTreeMap<String, ModelProfile> {
    let mut m = BTreeMap::new();
    m.insert(
        "vertex-flash".to_owned(),
        ModelProfile {
            name: "vertex-flash".to_owned(),
            kind: ProfileKind::Vertex,
            description: "Upstream setup: Gemini Flash executes, Gemini Pro improves and judges."
                .to_owned(),
            solver_model: "google-vertex/gemini-3.5-flash".to_owned(),
            improver_model: Some("google-vertex/gemini-3.5-pro".to_owned()),
            judge_model: Some("gemini-3.5-pro".to_owned()),
            codex_model: None,
            vertex: Some(VertexSettings {
                project: "example-research-project".to_owned(),
                location: "global".to_owned(),
                credentials_secret: Some("VERTEX_SA_KEY".to_owned()),
            }),
            endpoint: None,
            judge_endpoint: None,
            codex_endpoint: None,
            extra_env: Vec::new(),
            prices: Some(Prices {
                rollout_usd_low: 0.02,
                rollout_usd_high: 0.09,
                session_usd_low: 0.08,
                session_usd_high: 0.35,
            }),
            egress_hosts: vec![
                "aiplatform.googleapis.com".to_owned(),
                "oauth2.googleapis.com".to_owned(),
            ],
            secrets: Vec::new(),
        },
    );
    m.insert(
        "local-model".to_owned(),
        ModelProfile {
            name: "local-model".to_owned(),
            kind: ProfileKind::OpenaiCompatible,
            description: "A self-hosted model behind an OpenAI-compatible gateway on this server."
                .to_owned(),
            solver_model: "open-model-32b".to_owned(),
            improver_model: None,
            judge_model: Some("open-model-32b".to_owned()),
            codex_model: Some("open-model-32b".to_owned()),
            vertex: None,
            endpoint: Some(Endpoint {
                base_url: "http://127.0.0.1:8000/v1".to_owned(),
                api_key_secret: Some("MODEL_GATEWAY_KEY".to_owned()),
            }),
            judge_endpoint: Some(Endpoint {
                base_url: "http://127.0.0.1:8000/v1".to_owned(),
                api_key_secret: Some("MODEL_GATEWAY_KEY".to_owned()),
            }),
            codex_endpoint: Some(Endpoint {
                base_url: "http://127.0.0.1:8000/v1".to_owned(),
                api_key_secret: Some("MODEL_GATEWAY_KEY".to_owned()),
            }),
            extra_env: vec![h2w_api::EnvVar {
                name: "HARNESS2_JUDGE_TIMEOUT".to_owned(),
                value: Some("600".to_owned()),
                secret: None,
            }],
            prices: None,
            egress_hosts: Vec::new(),
            secrets: Vec::new(),
        },
    );
    m.insert(
        "codex-bridge".to_owned(),
        ModelProfile {
            name: "codex-bridge".to_owned(),
            kind: ProfileKind::Custom,
            description: "Codex executor through harness2-bridge (start the bridge first)."
                .to_owned(),
            solver_model: "google-vertex/gemini-3.5-flash".to_owned(),
            improver_model: Some("google-vertex/gemini-3.5-pro".to_owned()),
            judge_model: None,
            codex_model: Some("gemini-3.5-flash".to_owned()),
            vertex: None,
            endpoint: None,
            judge_endpoint: None,
            codex_endpoint: Some(Endpoint {
                base_url: "http://127.0.0.1:4100/v1".to_owned(),
                api_key_secret: Some("HARNESS2_CODEX_API_KEY".to_owned()),
            }),
            extra_env: Vec::new(),
            prices: None,
            egress_hosts: Vec::new(),
            secrets: Vec::new(),
        },
    );
    m
}

fn secrets() -> BTreeMap<String, SecretInfo> {
    let mut m = BTreeMap::new();
    for (name, chars, age) in [
        ("MODEL_GATEWAY_KEY", 40, 6 * DAY),
        ("VERTEX_SA_KEY", 2_311, 9 * DAY),
        ("SLACK_WEBHOOK_URL", 77, 12 * DAY),
    ] {
        m.insert(
            name.to_owned(),
            SecretInfo {
                name: name.to_owned(),
                set: true,
                chars: Some(chars),
                updated_at: Some(ago(age)),
            },
        );
    }
    m
}

fn tokens() -> Vec<TokenInfo> {
    vec![
        TokenInfo {
            id: "k3v9q2".to_owned(),
            name: "admin (init)".to_owned(),
            role: Role::Admin,
            created_at: ago(10 * DAY),
            created_by: "h2workbench init".to_owned(),
            last_used_at: Some(ago(120)),
            expires_at: None,
            revoked: false,
        },
        TokenInfo {
            id: "p8m1x4".to_owned(),
            name: "ci-runner".to_owned(),
            role: Role::Operator,
            created_at: ago(7 * DAY),
            created_by: "admin".to_owned(),
            last_used_at: Some(ago(5 * 3600)),
            expires_at: Some(ago(-83 * DAY)),
            revoked: false,
        },
        TokenInfo {
            id: "r2d7w5".to_owned(),
            name: "reviewer".to_owned(),
            role: Role::Viewer,
            created_at: ago(5 * DAY),
            created_by: "admin".to_owned(),
            last_used_at: None,
            expires_at: Some(ago(-25 * DAY)),
            revoked: false,
        },
        TokenInfo {
            id: "z0n4b6".to_owned(),
            name: "old laptop".to_owned(),
            role: Role::Operator,
            created_at: ago(30 * DAY),
            created_by: "admin".to_owned(),
            last_used_at: Some(ago(21 * DAY)),
            expires_at: None,
            revoked: true,
        },
    ]
}

pub(crate) fn benchmarks(installed: bool) -> Vec<BenchmarkStatus> {
    let counts = |d: Vec<(&str, Vec<&str>)>| -> Vec<DomainCount> {
        d.into_iter()
            .map(|(n, t)| DomainCount {
                domain: n.to_owned(),
                tasks: u32::try_from(t.len()).unwrap_or(0),
            })
            .collect()
    };
    let mut v = vec![
        BenchmarkStatus {
            bench: Bench::Jb,
            subset: None,
            display_name: "JobBench".to_owned(),
            installed,
            at_pinned_commit: installed.then_some(true),
            data_present: installed,
            domains: if installed { counts(jb_domains()) } else { Vec::new() },
            requirements: vec!["git".to_owned(), "uv".to_owned(), "Node.js 20+".to_owned()],
            upstream_url: "https://example.org/job-bench-eval".to_owned(),
            license_note: "Benchmark code and task data are distributed under the upstream licences; the task dataset is downloaded from its own host during setup.".to_owned(),
            description: "Professional work tasks grouped by occupation. A rubric-based judge checks each deliverable against task-specific criteria; the score is the share of criteria met.".to_owned(),
        },
        BenchmarkStatus {
            bench: Bench::Lab,
            subset: None,
            display_name: "LAB (legal work)".to_owned(),
            installed,
            at_pinned_commit: installed.then_some(true),
            data_present: installed,
            domains: if installed { counts(lab_domains()) } else { Vec::new() },
            requirements: vec!["git".to_owned(), "uv".to_owned(), "Pandoc".to_owned(), "Poppler (pdftoppm)".to_owned()],
            upstream_url: "https://example.org/harvey-labs".to_owned(),
            license_note: "Legal task data and rubrics under the upstream licence; review it before redistributing outputs.".to_owned(),
            description: "Legal drafting and review tasks grouped by practice area. Deliverables are rendered to text (Pandoc, Poppler) and judged against expert criteria.".to_owned(),
        },
    ];
    for sub in WbSubset::ALL {
        v.push(BenchmarkStatus {
            bench: Bench::Wb,
            subset: Some(sub),
            display_name: format!("WorkBuddy-Bench {}", sub.as_cli()),
            installed: false,
            at_pinned_commit: None,
            data_present: false,
            domains: Vec::new(),
            requirements: vec!["Docker (with Compose)".to_owned(), "git".to_owned(), "uv".to_owned()],
            upstream_url: "https://example.org/workbuddy-bench".to_owned(),
            license_note: "Custom Tencent licence with a TERRITORIAL LIMITATION: it does not grant rights in some territories. Read the upstream licence and confirm it permits your use before running setup.".to_owned(),
            description: format!(
                "Office, web and code tasks run inside Docker environments; the {} subset is graded by checkers and a model judge.",
                sub.as_cli()
            ),
        });
    }
    v
}

pub(crate) fn doctor(installed: bool) -> DoctorReport {
    let c = |id: &str,
             label: &str,
             level: CheckLevel,
             detail: &str,
             remedy: Option<&str>,
             needed: &[&str]| DoctorCheck {
        id: id.to_owned(),
        label: label.to_owned(),
        level,
        detail: detail.to_owned(),
        remedy: remedy.map(str::to_owned),
        needed_for: needed.iter().map(|s| (*s).to_owned()).collect(),
    };
    let checks = vec![
        c(
            "uv",
            "uv",
            CheckLevel::Ok,
            "uv 0.9.4",
            None,
            &["demo", "lab", "jb", "wb"],
        ),
        c(
            "python312",
            "Python 3.12",
            CheckLevel::Ok,
            "Python 3.12.11 available to uv",
            None,
            &["demo", "lab", "jb", "wb"],
        ),
        c(
            "node20",
            "Node.js 20+",
            CheckLevel::Ok,
            "node v22.18.0",
            None,
            &["jb", "codex"],
        ),
        c(
            "git",
            "git",
            CheckLevel::Ok,
            "git 2.51.0",
            None,
            &["lab", "jb", "wb"],
        ),
        if installed {
            c(
                "harness2",
                "harness2",
                CheckLevel::Ok,
                "installed at the tested commit",
                None,
                &["demo", "lab", "jb", "wb"],
            )
        } else {
            c(
                "harness2",
                "harness2",
                CheckLevel::Fail,
                "not installed",
                Some(
                    "Install it from the dashboard (administrators) or run the install_harness2 job.",
                ),
                &["demo", "lab", "jb", "wb"],
            )
        },
        c(
            "pandoc",
            "Pandoc",
            CheckLevel::Warn,
            "pandoc 2.9.2 found; 3.1 or newer renders tables more faithfully",
            Some("Install Pandoc 3.1+ from your package manager or the Pandoc releases page."),
            &["lab"],
        ),
        c(
            "pdftoppm",
            "Poppler (pdftoppm)",
            CheckLevel::Ok,
            "pdftoppm 24.02",
            None,
            &["lab"],
        ),
        c(
            "docker",
            "Docker",
            CheckLevel::Fail,
            "permission denied on the Docker socket for the server's user",
            Some(
                "Add the server's user to a group that may use Docker, or configure rootless Docker, then restart the server.",
            ),
            &["wb"],
        ),
        c(
            "gcloud",
            "gcloud CLI",
            CheckLevel::Ok,
            "Google Cloud SDK 540.0.0",
            None,
            &["vertex"],
        ),
        c(
            "adc",
            "Application Default Credentials",
            CheckLevel::Warn,
            "no ADC file; the vertex-flash profile uses a stored key instead",
            Some(
                "Run `gcloud auth application-default login` as the server's user if a profile relies on ADC.",
            ),
            &["vertex"],
        ),
        c(
            "container_runtime",
            "Container runtime",
            CheckLevel::Skip,
            "isolation mode is host",
            None,
            &["container"],
        ),
        c(
            "disk",
            "Free disk space",
            CheckLevel::Ok,
            "212 GiB free in the data directory",
            None,
            &["demo", "lab", "jb", "wb"],
        ),
    ];
    DoctorReport {
        ok: !checks.iter().any(|c| c.level == CheckLevel::Fail),
        checks,
        generated_at: ago(40),
    }
}

fn splits() -> Vec<SplitInfo> {
    vec![SplitInfo {
        name: "lab-half".to_owned(),
        bench: Bench::Lab,
        subset: None,
        seed: 42,
        train_fraction: 0.5,
        train: 6,
        test: 6,
        created_at: ago(2 * DAY),
    }]
}

fn system_line(n: u64, ts: &str, text: &str) -> LogLine {
    LogLine {
        n,
        ts: ts.to_owned(),
        stream: LogStream::System,
        text: text.to_owned(),
    }
}

/// A finished historical job with a plausible log.
fn past_job(
    spec: JobSpec,
    status: JobStatus,
    created_ago: i64,
    wall: i64,
    result: Option<JobResult>,
    error: Option<&str>,
    lines: &[(&str, LogStream)],
) -> JobData {
    let created = ago(created_ago);
    let finished = ago(created_ago - wall);
    let mut log = vec![system_line(
        0,
        &created,
        &format!("job started ({}, isolation: host)", spec.kind().as_str()),
    )];
    for (i, (text, stream)) in lines.iter().enumerate() {
        log.push(LogLine {
            n: i as u64 + 1,
            ts: ago(created_ago - (wall * (i as i64 + 1)) / (lines.len() as i64 + 2)),
            stream: *stream,
            text: (*text).to_owned(),
        });
    }
    let n = log.len() as u64;
    log.push(system_line(
        n,
        &finished,
        &format!("job {}", status.as_str()),
    ));
    let steps = match &spec {
        JobSpec::Pipeline(p) => {
            let mut names = vec!["preview", "check", "run", "judge"];
            if p.judge_bases {
                names.push("judge-bases");
            }
            names.push("report");
            names
                .into_iter()
                .map(|s| JobStep {
                    name: s.to_owned(),
                    status,
                    exit_code: Some(0),
                    started_at: Some(created.clone()),
                    finished_at: Some(finished.clone()),
                })
                .collect()
        }
        _ => Vec::new(),
    };
    let (tx, _) = broadcast::channel(256);
    let may_spend = spec.may_spend();
    JobData {
        job: Job {
            id: crate::util::job_id(),
            kind: spec.kind(),
            spec,
            status,
            created_by: "admin".to_owned(),
            created_at: created.clone(),
            started_at: Some(created),
            finished_at: Some(finished),
            exit_code: Some(match status {
                JobStatus::Succeeded => 0,
                JobStatus::Partial => 2,
                _ => 1,
            }),
            isolation: Isolation::Host,
            steps,
            progress: None,
            result,
            error: error.map(str::to_owned),
            log_lines: log.len() as u64,
            spent_usd: may_spend.then_some(1.84),
        },
        log,
        tx,
        cancel: false,
    }
}

fn run_options(k: u32, modes: Vec<Mode>, seed: Option<u64>) -> RunOptions {
    RunOptions {
        k,
        seed,
        modes,
        workers: 2,
        ..RunOptions::default()
    }
}

fn past_jobs() -> Vec<JobData> {
    use LogStream::{Stderr, Stdout};
    let lab = cell(Bench::Lab, None, Substrate::Oc);
    let jb = cell(Bench::Jb, None, Substrate::Cx);
    let mut v = vec![
        past_job(
            JobSpec::InstallHarness2(InstallParams { rev: None, extras: vec![InstallExtra::Render], reinstall: false }),
            JobStatus::Succeeded,
            10 * DAY,
            95,
            Some(JobResult::Install(InstallResult {
                commit: TESTED_COMMIT.to_owned(),
                version: "0.4.1".to_owned(),
                python: "3.12.11".to_owned(),
                extras: vec![InstallExtra::Render],
            })),
            None,
            &[("fetching google-research/harness2 at the tested commit", Stdout), ("creating .venv with Python 3.12.11", Stdout), ("Resolved 61 packages in 1.2s", Stderr), ("Installed 61 packages in 3.4s", Stderr), ("harness2 0.4.1 installed", Stdout)],
        ),
        past_job(
            JobSpec::Setup(SetupParams { bench: Bench::Lab, subset: None, substrate: Substrate::Oc, dry_run: false, profile: Some("vertex-flash".into()) }),
            JobStatus::Succeeded,
            9 * DAY,
            420,
            Some(JobResult::Setup(SetupResult {
                dry_run: false,
                plan: vec!["git clone harvey-labs (pinned)".into(), "apply 2 patches".into(), "uv sync (lab environment)".into(), "build OpenCode from the pinned checkout".into()],
                notes: vec!["Note: LAB renders deliverables with Pandoc and Poppler.".into()],
            })),
            None,
            &[("cloning harvey-labs at the pinned commit", Stdout), ("applying patches 0001-0002", Stdout), ("building OpenCode (bun install)", Stdout), ("setup complete", Stdout)],
        ),
        past_job(
            JobSpec::Check(CheckParams { cell: lab, profile: Some("vertex-flash".into()) }),
            JobStatus::Succeeded,
            9 * DAY - 600,
            12,
            Some(JobResult::Check(CheckResult { ready: true, tasks: Some(12), failures: Vec::new() })),
            None,
            &[("checking lab with OpenCode", Stdout), ("Ready: lab-oc; 12 tasks.", Stdout)],
        ),
        past_job(
            JobSpec::Demo(DemoParams { experiment: "demo".into(), k: 2, mode: DemoMode::Both }),
            JobStatus::Succeeded,
            3 * DAY,
            2,
            Some(JobResult::Demo(DemoResult { done: 4, failed: 0, execution_count: 28, editor_calls: 24 })),
            None,
            &[("[writing] 1/2 ok release-notes", Stdout), ("[writing] 2/2 ok status-update", Stdout), ("[analysis] 1/2 ok latency-table", Stdout), ("[analysis] 2/2 ok cost-summary", Stdout), ("demo: 4 done, 0 failed, 28 executions, 24 editor calls", Stdout)],
        ),
        past_job(
            JobSpec::Pipeline(PipelineParams {
                experiment: "lab-pilot".into(),
                cell: lab,
                options: run_options(1, vec![Mode::Parallel, Mode::Sequential], None),
                profile: "vertex-flash".into(),
                judge_bases: true,
                budget_usd: Some(40.0),
                acknowledge_risks: true,
            }),
            JobStatus::Partial,
            2 * DAY,
            5 * 3600,
            Some(JobResult::Pipeline(PipelineResult {
                steps: vec![
                    JobResult::Check(CheckResult { ready: true, tasks: Some(12), failures: Vec::new() }),
                    JobResult::Run(RunResult {
                        namespace: "lab-oc-agg1".into(),
                        done: 10,
                        failed: 2,
                        dead_domains: Vec::new(),
                        record: Some("lab-oc-agg1-20261021T0912.json".into()),
                        next: Some("harness2-judge --bench lab --k 1 --modes parallel,sequential".into()),
                    }),
                    JobResult::Judge(JudgeResult { bases: false, rows: 20, graded: 19, ungraded: 1 }),
                    JobResult::Judge(JudgeResult { bases: true, rows: 12, graded: 12, ungraded: 0 }),
                    JobResult::Report(ReportResult { text: "lab · OpenCode · k=1\n(see the sweep's Report tab)".into() }),
                ],
            })),
            None,
            &[("[Contracts] 1/4 ok contracts/nda-review-014", Stdout), ("[Employment] 1/3 ok employment/handbook-update-003", Stdout), ("[Employment] 3/3 partial employment/leave-policy-026", Stdout), ("Traceback (most recent call last):", Stderr), ("  bun: out of memory while starting the executor", Stderr), ("[Litigation] 2/3 base-failed litigation/motion-outline-019", Stdout), ("sweep finished: 10 done, 2 failed", Stdout), ("judge: 20 rows, 19 graded, 1 ungraded", Stdout)],
        ),
        past_job(
            JobSpec::Judge(JudgeParams { experiment: "jobbench-baseline".into(), cell: jb, k: 2, seed: None, modes: vec![Mode::Parallel, Mode::Sequential], bases: false, profile: "local-model".into() }),
            JobStatus::Succeeded,
            4 * DAY,
            900,
            Some(JobResult::Judge(JudgeResult { bases: false, rows: 20, graded: 19, ungraded: 1 })),
            None,
            &[("judging 20 selected runs", Stdout), ("judge: 20 rows, 19 graded, 1 ungraded", Stdout)],
        ),
        past_job(
            JobSpec::Report(ReportParams { experiment: "jobbench-baseline".into(), cell: jb, ks: vec![2], seed: None, modes: vec![Mode::Parallel, Mode::Sequential] }),
            JobStatus::Succeeded,
            4 * DAY - 1200,
            3,
            Some(JobResult::Report(ReportResult { text: "jb · Codex · k=2\nmode          n     score  all-pass\nparallel     10     72.4%     30.0%\nsequential    9     69.1%     22.2%\n".into() })),
            None,
            &[("reading judged.json", Stdout)],
        ),
        past_job(
            JobSpec::Split(SplitParams { bench: Bench::Lab, subset: None, seed: 42, train_fraction: 0.5, name: "lab-half".into() }),
            JobStatus::Succeeded,
            2 * DAY + 3600,
            2,
            Some(JobResult::Split(SplitResult { name: "lab-half".into(), train: 6, test: 6 })),
            None,
            &[("split lab: 6 train, 6 test (seed 42)", Stdout)],
        ),
        past_job(
            JobSpec::Setup(SetupParams { bench: Bench::Wb, subset: Some(WbSubset::Office), substrate: Substrate::Oc, dry_run: false, profile: Some("local-model".into()) }),
            JobStatus::Failed,
            DAY,
            40,
            None,
            Some("docker: permission denied while trying to connect to the Docker daemon socket"),
            &[("cloning workbuddy-bench at the pinned commit", Stdout), ("building office environment image", Stdout), ("permission denied while trying to connect to the Docker daemon socket", Stderr)],
        ),
        past_job(
            JobSpec::Preview(h2w_api::PreviewParams { experiment: "lab-pilot".into(), cell: lab, options: run_options(3, vec![Mode::Parallel], Some(7)), profile: Some("vertex-flash".into()) }),
            JobStatus::Cancelled,
            DAY - 3600,
            5,
            None,
            Some("cancelled by admin"),
            &[("# lab-oc split=all seed=7 k=3 modes=parallel", Stdout)],
        ),
    ];
    v.sort_by(|a, b| a.job.created_at.cmp(&b.job.created_at));
    v
}

fn audit_seed(inner: &mut Inner) {
    let entries: [(&str, &str, &str, AuditOutcome, Option<&str>); 14] = [
        (
            "h2workbench init",
            "token.create",
            "k3v9q2",
            AuditOutcome::Success,
            Some("role=admin"),
        ),
        (
            "admin (init)",
            "auth.login",
            "k3v9q2",
            AuditOutcome::Success,
            None,
        ),
        (
            "admin (init)",
            "job.create",
            "install_harness2",
            AuditOutcome::Success,
            None,
        ),
        (
            "admin (init)",
            "secret.set",
            "VERTEX_SA_KEY",
            AuditOutcome::Success,
            None,
        ),
        (
            "admin (init)",
            "profile.put",
            "vertex-flash",
            AuditOutcome::Success,
            None,
        ),
        (
            "admin (init)",
            "token.create",
            "p8m1x4",
            AuditOutcome::Success,
            Some("role=operator"),
        ),
        (
            "anonymous",
            "auth.login",
            "unknown",
            AuditOutcome::Denied,
            Some("invalid token"),
        ),
        (
            "admin (init)",
            "experiment.create",
            "lab-pilot",
            AuditOutcome::Success,
            None,
        ),
        (
            "ci-runner",
            "job.create",
            "pipeline lab-pilot",
            AuditOutcome::Success,
            None,
        ),
        (
            "reviewer",
            "job.create",
            "run lab-pilot",
            AuditOutcome::Denied,
            Some("role viewer < operator"),
        ),
        (
            "admin (init)",
            "secret.set",
            "MODEL_GATEWAY_KEY",
            AuditOutcome::Success,
            None,
        ),
        (
            "admin (init)",
            "token.revoke",
            "z0n4b6",
            AuditOutcome::Success,
            None,
        ),
        (
            "admin (init)",
            "job.cancel",
            "preview lab-pilot",
            AuditOutcome::Success,
            None,
        ),
        (
            "ci-runner",
            "job.create",
            "run lab-pilot",
            AuditOutcome::Success,
            None,
        ),
    ];
    for (i, (actor, action, target, outcome, detail)) in entries.into_iter().enumerate() {
        inner.add_audit(actor, action, target, outcome, detail.map(str::to_owned));
        if let Some(last) = inner.audit.last_mut() {
            last.ts = ago(10 * DAY - i as i64 * 15_000);
        }
    }
}

impl Inner {
    /// A lab that has been used for a while: experiments, jobs, profiles, tokens.
    pub(crate) fn populated() -> Inner {
        let mut inner = Inner {
            installed: true,
            isolation: Isolation::Host,
            host_risks_preacknowledged: false,
            experiments: experiments(),
            jobs: past_jobs(),
            profiles: profiles(),
            secrets: secrets(),
            tokens: tokens(),
            sessions: HashMap::new(),
            audit: Vec::new(),
            splits: splits(),
            benches: benchmarks(true),
        };
        audit_seed(&mut inner);
        inner
    }

    /// A brand-new server: nothing installed, no experiments, no profiles.
    pub(crate) fn fresh() -> Inner {
        let mut inner = Inner {
            installed: false,
            isolation: Isolation::Host,
            host_risks_preacknowledged: false,
            experiments: BTreeMap::new(),
            jobs: Vec::new(),
            profiles: BTreeMap::new(),
            secrets: BTreeMap::new(),
            tokens: tokens().into_iter().take(1).collect(),
            sessions: HashMap::new(),
            audit: Vec::new(),
            splits: Vec::new(),
            benches: benchmarks(false),
        };
        inner.add_audit(
            "h2workbench init",
            "token.create",
            "k3v9q2",
            AuditOutcome::Success,
            Some("role=admin".into()),
        );
        inner
    }
}

/// The running job the populated mock starts with, so live views have something to show.
pub(crate) fn live_run_spec() -> JobSpec {
    JobSpec::Run(RunParams {
        experiment: "lab-pilot".to_owned(),
        cell: cell(Bench::Lab, None, Substrate::Oc),
        options: run_options(3, vec![Mode::Parallel], Some(7)),
        profile: "vertex-flash".to_owned(),
        budget_usd: Some(60.0),
        acknowledge_risks: true,
    })
}

/// A queued pipeline waiting behind the live run on the same experiment.
pub(crate) fn queued_spec() -> JobSpec {
    JobSpec::Pipeline(PipelineParams {
        experiment: "lab-pilot".to_owned(),
        cell: cell(Bench::Lab, None, Substrate::Oc),
        options: run_options(1, vec![Mode::Sequential], Some(11)),
        profile: "vertex-flash".to_owned(),
        judge_bases: false,
        budget_usd: Some(15.0),
        acknowledge_risks: true,
    })
}
