//! The New-run wizard: experiment and benchmark → options → preview → estimate → confirm.

use h2s_api::{
    Bench, BenchmarkStatus, CostEstimate, DomainCount, Experiment, Job, JobResult, JobSpec,
    JobStatus, ModelProfile, PreviewResult, Role, Split, SplitInfo, Substrate, WbSubset,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;

use crate::actions;
use crate::api::{self, ApiError};
use crate::components::form::{Checkbox, RadioCards, SelectField, TextField};
use crate::components::icons::{Icon, icon};
use crate::components::ui::{
    Card, EmptyState, ErrorPanel, Notice, PageHeader, Skeleton, Stat, Term,
};
use crate::components::widgets::ProfileCheckFor;
use crate::logic::format;
use crate::logic::jobs::{Tone, cell_label, modes_label};
use crate::logic::urls::{api as paths, route};
use crate::logic::wizard::{Preset, RunForm, Step, describe, expected_tasks, quick_counts};
use crate::nav::query_param;
use crate::pages::experiments::CreateExperiment;
use crate::state::{Toasts, session, toasts};

fn bench_of(s: &str) -> Bench {
    match s {
        "jb" => Bench::Jb,
        "wb" => Bench::Wb,
        _ => Bench::Lab,
    }
}

fn subset_of(s: &str) -> WbSubset {
    match s {
        "web" => WbSubset::Web,
        "code" => WbSubset::Code,
        _ => WbSubset::Office,
    }
}

fn split_of(s: &str) -> Split {
    match s {
        "train" => Split::Train,
        "test" => Split::Test,
        _ => Split::All,
    }
}

/// The outcome of the inline preview job.
#[derive(Clone, Debug, PartialEq)]
struct PreviewRun {
    spec: JobSpec,
    job: Job,
}

/// All wizard state; handles only, so it is `Copy`.
#[derive(Clone, Copy)]
struct Wiz {
    step: RwSignal<Step>,
    show_errors: RwSignal<bool>,
    experiment: RwSignal<String>,
    profile: RwSignal<String>,
    bench: RwSignal<String>,
    subset: RwSignal<String>,
    substrate: RwSignal<String>,
    k: RwSignal<String>,
    parallel: RwSignal<bool>,
    sequential: RwSignal<bool>,
    seed: RwSignal<String>,
    split: RwSignal<String>,
    split_file: RwSignal<String>,
    workers: RwSignal<String>,
    only_domains: RwSignal<Vec<String>>,
    max_tasks: RwSignal<String>,
    budget: RwSignal<String>,
    run_kind: RwSignal<String>,
    judge_bases: RwSignal<bool>,
    acknowledge: RwSignal<bool>,
    experiments: LocalResource<Result<Vec<Experiment>, ApiError>>,
    profiles: LocalResource<Result<Vec<ModelProfile>, ApiError>>,
    benchmarks: LocalResource<Result<Vec<BenchmarkStatus>, ApiError>>,
    splits: LocalResource<Result<Vec<SplitInfo>, ApiError>>,
    loaded_domains: RwSignal<Option<Vec<DomainCount>>>,
    domains_busy: RwSignal<bool>,
    preview: RwSignal<Option<PreviewRun>>,
    preview_busy: RwSignal<bool>,
    preview_error: RwSignal<Option<ApiError>>,
    form: Memo<RunForm>,
    create_open: RwSignal<bool>,
    toasts: Toasts,
}

impl Wiz {
    fn pinned(self) -> Option<String> {
        let name = self.experiment.get();
        self.experiments
            .get()
            .and_then(Result::ok)
            .and_then(|list| list.into_iter().find(|e| e.name == name))
            .and_then(|e| e.profile)
    }

    /// Domains known for the selected cell: from a loaded preview, else the benchmark's
    /// last preview as the server remembers it.
    fn domains(self) -> Vec<DomainCount> {
        if let Some(d) = self.loaded_domains.get() {
            return d;
        }
        let cell = self.form.with(RunForm::cell);
        self.benchmarks
            .get()
            .and_then(Result::ok)
            .and_then(|list| {
                list.into_iter().find(|b| {
                    b.bench == cell.bench && (cell.bench != Bench::Wb || b.subset == cell.subset)
                })
            })
            .map(|b| b.domains)
            .unwrap_or_default()
    }

    fn preview_result(self) -> Option<PreviewResult> {
        self.preview.with(|p| {
            p.as_ref().and_then(|p| match &p.job.result {
                Some(JobResult::Preview(r)) if p.job.status == JobStatus::Succeeded => {
                    Some(r.clone())
                }
                _ => None,
            })
        })
    }

    fn preview_fresh(self) -> bool {
        let current = self.form.with(RunForm::preview_spec).ok();
        self.preview.with(|p| {
            p.as_ref()
                .is_some_and(|p| Some(&p.spec) == current.as_ref())
        })
    }

    fn problems(self, step: Step) -> Vec<String> {
        let f = self.form.get();
        match step {
            Step::Cell => describe(&f.cell_problems()),
            Step::Options => f.options().err().map(|p| describe(&p)).unwrap_or_default(),
            Step::Preview => {
                if self.preview_result().is_some() && self.preview_fresh() {
                    Vec::new()
                } else {
                    vec!["Run the preview for the current options first.".to_owned()]
                }
            }
            Step::Estimate | Step::Confirm => Vec::new(),
        }
    }

    fn apply_preset(self, preset: Preset) {
        let mut f = self.form.get_untracked();
        f.apply_preset(preset, &self.domains());
        self.k.set(f.k);
        self.parallel.set(f.parallel);
        self.sequential.set(f.sequential);
        self.max_tasks.set(f.max_tasks);
        self.only_domains.set(f.only_domains);
        self.workers.set(f.workers);
        self.split.set("all".to_owned());
        self.split_file.set(String::new());
    }

    fn load_domains(self) {
        let spec = self.form.get_untracked().domains_preview_spec();
        self.domains_busy.set(true);
        spawn_local(async move {
            let r = actions::run_and_wait(&spec, 180).await;
            self.domains_busy.set(false);
            match r {
                Ok(job) => {
                    if let Some(JobResult::Preview(p)) = job.result {
                        self.loaded_domains.set(Some(p.domains));
                    } else {
                        self.toasts.warn(
                            "Could not load the domains",
                            &job.error.unwrap_or_else(|| {
                                format!("The preview job ended as {}.", job.status.as_str())
                            }),
                        );
                    }
                }
                Err(e) => self.toasts.error("Could not load the domains", &e),
            }
        });
    }

    fn run_preview(self) {
        let Ok(spec) = self.form.get_untracked().preview_spec() else {
            return;
        };
        self.preview_busy.set(true);
        self.preview_error.set(None);
        spawn_local(async move {
            let r = actions::run_and_wait(&spec, 300).await;
            self.preview_busy.set(false);
            match r {
                Ok(job) => self.preview.set(Some(PreviewRun { spec, job })),
                Err(e) => self.preview_error.set(Some(e)),
            }
        });
    }
}

#[component]
fn Problems(wiz: Wiz, step: Step) -> impl IntoView {
    move || {
        let list = wiz.problems(step);
        (wiz.show_errors.get() && !list.is_empty()).then(|| {
            view! {
                <Notice tone=Tone::Danger title="Before you continue">
                    <ul>{list.into_iter().map(|p| view! { <li>{p}</li> }).collect_view()}</ul>
                </Notice>
            }
        })
    }
}

#[component]
fn CellStep(wiz: Wiz) -> impl IntoView {
    let exp_options = Signal::derive(move || {
        let mut v = vec![(String::new(), "Choose an experiment…".to_owned())];
        if let Some(Ok(list)) = wiz.experiments.get() {
            v.extend(
                list.into_iter()
                    .filter(|e| e.kind == h2s_api::ExperimentKind::Standard)
                    .map(|e| {
                        let label = match &e.profile {
                            Some(p) => format!("{} (profile {p})", e.name),
                            None => e.name.clone(),
                        };
                        (e.name, label)
                    }),
            );
        }
        v
    });
    let profile_options = Signal::derive(move || {
        let mut v = vec![(String::new(), "Choose a model profile…".to_owned())];
        if let Some(Ok(list)) = wiz.profiles.get() {
            v.extend(list.into_iter().map(|p| {
                let missing: Vec<String> = p
                    .secrets
                    .iter()
                    .filter(|s| !s.set)
                    .map(|s| s.name.clone())
                    .collect();
                let label = if missing.is_empty() {
                    p.name.clone()
                } else {
                    format!("{} (secret {} not set)", p.name, missing.join(", "))
                };
                (p.name, label)
            }));
        }
        v
    });
    Effect::new(move |_| {
        if let Some(p) = wiz.pinned() {
            wiz.profile.set(p);
        }
    });
    let bench_status = move |b: Bench, sub: Option<WbSubset>| -> Option<bool> {
        wiz.benchmarks.get().and_then(Result::ok).map(|list| {
            list.iter()
                .filter(|s| s.bench == b && (sub.is_none() || s.subset == sub))
                .any(|s| s.installed)
        })
    };
    view! {
        <h2>"Experiment and benchmark"</h2>
        <div class="form-grid">
            <div>
                <SelectField
                    id="run-experiment"
                    label="Experiment"
                    value=wiz.experiment
                    options=exp_options
                    help="Results go into this experiment's folder."
                />
                <button type="button" class="btn btn-ghost btn-sm" on:click=move |_| wiz.create_open.set(true)>
                    {icon(Icon::Plus)}
                    " New experiment"
                </button>
            </div>
            <div>
                <SelectField
                    id="run-profile"
                    label="Model profile"
                    value=wiz.profile
                    options=profile_options
                    disabled=Signal::derive(move || wiz.pinned().is_some())
                    help=Signal::derive(move || match wiz.pinned() {
                        Some(p) => format!("Locked to {p}: harness2 refuses to resume or judge results under different models."),
                        None => "Which models execute, improve and judge. This experiment has no pinned profile yet; the first run pins it.".to_owned(),
                    })
                />
                <a class="small" href="/settings/profiles">"Manage profiles"</a>
                <ProfileCheckFor profile=wiz.profile />
            </div>
        </div>
        <RadioCards
            name="run-bench"
            legend="Benchmark"
            value=wiz.bench
            options=vec![
                ("jb".to_owned(), "JobBench".to_owned(), "Professional work tasks, grouped by occupation.".to_owned()),
                ("lab".to_owned(), "LAB".to_owned(), "Legal drafting and review, grouped by practice area.".to_owned()),
                ("wb".to_owned(), "WorkBuddy-Bench".to_owned(), "Office, web and code tasks in Docker environments.".to_owned()),
            ]
        />
        {move || {
            let b = bench_of(&wiz.bench.get());
            let sub = (b == Bench::Wb).then(|| subset_of(&wiz.subset.get()));
            (bench_status(b, sub) == Some(false))
                .then(|| {
                    view! {
                        <Notice tone=Tone::Warning title="Not set up yet">
                            <p>
                                "This benchmark is not installed on the server. "
                                <a href="/benchmarks">"Set it up on the Benchmarks page"</a>
                                " first (an administrator can do this)."
                            </p>
                        </Notice>
                    }
                })
        }}
        {move || {
            (wiz.bench.get() == "wb")
                .then(|| {
                    view! {
                        <RadioCards
                            name="run-subset"
                            legend="WorkBuddy subset"
                            value=wiz.subset
                            compact=true
                            options=vec![
                                ("office".to_owned(), "Office".to_owned(), "Documents, spreadsheets, slides.".to_owned()),
                                ("web".to_owned(), "Web".to_owned(), "Browser tasks.".to_owned()),
                                ("code".to_owned(), "Code".to_owned(), "Programming tasks.".to_owned()),
                            ]
                        />
                        <Notice tone=Tone::Warning title="Licence with a territorial limitation">
                            <p>"WorkBuddy-Bench is under a custom Tencent licence that does not grant rights in some territories. Make sure it covers your use."</p>
                        </Notice>
                    }
                })
        }}
        <RadioCards
            name="run-substrate"
            legend="Executor"
            value=wiz.substrate
            compact=true
            options=vec![
                ("oc".to_owned(), "OpenCode".to_owned(), "Upstream default; 24,000-character always-on budget.".to_owned()),
                ("cx".to_owned(), "Codex".to_owned(), "Codex CLI on a Responses-API endpoint; 8,000-character budget.".to_owned()),
            ]
        />
        <Problems wiz=wiz step=Step::Cell />
    }
}

#[component]
fn OptionsStep(wiz: Wiz) -> impl IntoView {
    let split_options = Signal::derive(move || {
        let b = bench_of(&wiz.bench.get());
        let mut v = vec![(String::new(), "Choose a split file…".to_owned())];
        if let Some(Ok(list)) = wiz.splits.get() {
            v.extend(list.into_iter().filter(|s| s.bench == b).map(|s| {
                let label = format!(
                    "{} ({} train / {} test, seed {})",
                    s.name, s.train, s.test, s.seed
                );
                (s.name, label)
            }));
        }
        v
    });
    let field_error = move |name: &'static str| {
        Signal::derive(move || {
            wiz.form
                .with(|f| f.options().err())
                .and_then(|p| p.into_iter().find(|(f, _)| f == name).map(|(_, msg)| msg))
        })
    };
    let counts = move || {
        let f = wiz.form.get();
        let domains = wiz.domains();
        let tasks = expected_tasks(&domains, &f.only_domains, f.max_tasks_value());
        let modes = u32::try_from(f.modes().len()).unwrap_or(0);
        let k = f.k_value().unwrap_or(1);
        let (r, s_min, s_max) = quick_counts(tasks, k, modes);
        (domains.is_empty(), tasks, r, s_min, s_max)
    };
    view! {
        <h2>"Options"</h2>
        <p class="muted">"Start from a preset, then adjust. Every option maps to a harness2 flag."</p>
        <div class="presets">
            {Preset::ALL
                .iter()
                .map(|p| {
                    let p = *p;
                    view! {
                        <button type="button" class="preset" on:click=move |_| wiz.apply_preset(p)>
                            <strong>{p.title()}</strong>
                            <span>{p.description()}</span>
                        </button>
                    }
                })
                .collect_view()}
        </div>
        <div class="form-grid">
            <TextField
                id="run-k"
                label="Recursion budget k"
                value=wiz.k
                kind="number"
                inputmode="numeric"
                error=field_error("k")
                help="Per task and mode: 4k executor rollouts and 3k improver sessions. 1 to 32."
            />
            <fieldset>
                <legend>"Recursion modes"</legend>
                <Checkbox id="run-parallel" label="Parallel (width)" checked=wiz.parallel help="k general and k domain candidates, then k compositions." />
                <Checkbox id="run-sequential" label="Sequential (depth)" checked=wiz.sequential help="k successive Propose–Probe–Compose steps." />
                {move || field_error("modes").get().map(|e| view! { <p class="field-error" role="alert">{e}</p> })}
            </fieldset>
            <TextField
                id="run-seed"
                label="Seed (optional)"
                value=wiz.seed
                kind="number"
                inputmode="numeric"
                error=field_error("seed")
                help="Shuffles the task order reproducibly. It does not control model sampling. Empty = upstream's sorted order."
            />
            <TextField
                id="run-workers"
                label="Workers"
                value=wiz.workers
                kind="number"
                inputmode="numeric"
                error=field_error("workers")
                help="Domains run concurrently; tasks within a domain stay in order because each reads the library entries of the ones before it."
            />
            <TextField
                id="run-max-tasks"
                label="Tasks per domain (optional)"
                value=wiz.max_tasks
                kind="number"
                inputmode="numeric"
                error=field_error("max_tasks")
                help="Run only the first N tasks of every domain. Empty = all."
            />
            <div>
                <RadioCards
                    name="run-split"
                    legend="Task split"
                    value=wiz.split
                    compact=true
                    options=vec![
                        ("all".to_owned(), "All tasks".to_owned(), "No split.".to_owned()),
                        ("train".to_owned(), "Train part".to_owned(), "Needs a split file.".to_owned()),
                        ("test".to_owned(), "Test part".to_owned(), "Needs a split file.".to_owned()),
                    ]
                />
                {move || {
                    (wiz.split.get() != "all")
                        .then(|| {
                            view! {
                                <SelectField
                                    id="run-split-file"
                                    label="Split file"
                                    value=wiz.split_file
                                    options=split_options
                                    error=field_error("split_file")
                                    help="Create splits on the Benchmarks page."
                                />
                            }
                        })
                }}
            </div>
        </div>
        <fieldset class="domain-picker">
            <legend>"Only these domains (optional)"</legend>
            {move || {
                let domains = wiz.domains();
                if domains.is_empty() {
                    view! {
                        <p class="muted small">"The domain list comes from a preview without filters."</p>
                    }
                    .into_any()
                } else {
                    view! {
                        <div class="chips">
                            {domains
                                .into_iter()
                                .map(|d| {
                                    let name = d.domain.clone();
                                    let n2 = name.clone();
                                    let n3 = name.clone();
                                    view! {
                                        <label class="chip" class:selected=move || wiz.only_domains.with(|v| v.contains(&n2))>
                                            <input
                                                type="checkbox"
                                                prop:checked=move || wiz.only_domains.with(|v| v.contains(&n3))
                                                on:change=move |_| {
                                                    let n = name.clone();
                                                    wiz.only_domains.update(|v| {
                                                        if let Some(i) = v.iter().position(|x| *x == n) {
                                                            v.remove(i);
                                                        } else {
                                                            v.push(n);
                                                        }
                                                    });
                                                }
                                            />
                                            {format!("{} ({})", d.domain, d.tasks)}
                                        </label>
                                    }
                                })
                                .collect_view()}
                        </div>
                        <p class="muted small">"None selected = every domain."</p>
                    }
                    .into_any()
                }
            }}
            <button
                type="button"
                class="btn btn-secondary btn-sm"
                disabled=move || wiz.domains_busy.get()
                on:click=move |_| wiz.load_domains()
            >
                {icon(Icon::Refresh)}
                {move || if wiz.domains_busy.get() { " Loading domains…" } else { " Load domains from a preview" }}
            </button>
        </fieldset>
        <div class="counts" aria-live="polite">
            {move || {
                let (unknown, tasks, r, s_min, s_max) = counts();
                if unknown {
                    view! { <p class="muted small">"Load the domains to see a rough size; the preview step gives the exact numbers."</p> }.into_any()
                } else {
                    view! {
                        <Stat label="Tasks (rough)" value=format::int(u64::from(tasks)) />
                        <Stat label="Executor rollouts" value=format::int(r) hint="k shared base runs + 3k per mode" />
                        <Stat label="Improver sessions" value=format!("{}–{}", format::int(s_min), format::int(s_max)) hint="3k per mode, plus selection and reflection" />
                    }
                    .into_any()
                }
            }}
        </div>
        <Problems wiz=wiz step=Step::Options />
    }
}

#[component]
fn PreviewStep(wiz: Wiz) -> impl IntoView {
    let s = session();
    view! {
        <h2>"Preview"</h2>
        <p class="muted">
            "A " <Term id="preview">"preview"</Term>
            " runs harness2 with --dry-run: the exact task stream, namespaces and domains. No model is called and nothing is written."
        </p>
        {move || {
            let fresh = wiz.preview_fresh();
            let has = wiz.preview.with(Option::is_some);
            (has && !fresh)
                .then(|| view! { <Notice tone=Tone::Warning><p>"The options changed since this preview. Run it again."</p></Notice> })
        }}
        <button
            type="button"
            class="btn btn-primary"
            disabled=move || wiz.preview_busy.get() || !s.can(Role::Operator)
            on:click=move |_| wiz.run_preview()
        >
            {icon(Icon::Play)}
            {move || if wiz.preview_busy.get() { " Previewing…" } else { " Run the preview" }}
        </button>
        {move || wiz.preview_error.get().map(|e| view! { <ErrorPanel error=e /> })}
        {move || {
            wiz.preview
                .get()
                .map(|p| {
                    let job_link = route::job(&p.job.id);
                    match p.job.result.clone() {
                        Some(JobResult::Preview(r)) => view! {
                            <div class="preview-result">
                                <p class="mono small">{r.header.clone()}</p>
                                <div class="stats">
                                    <Stat label="Tasks" value=format::int(u64::from(r.total_tasks)) />
                                    <Stat label="Domains" value=r.total_domains.to_string() />
                                    <Stat label="Workers" value=r.workers.to_string() />
                                </div>
                                <p class="small">
                                    "Namespaces: "
                                    {r.namespaces.iter().map(|n| format!("{} ({})", n.namespace, n.mode.as_cli())).collect::<Vec<_>>().join(", ")}
                                    " · summary namespace "<code>{r.summary_namespace.clone()}</code>
                                </p>
                                <div class="table-wrap">
                                    <table class="table">
                                        <thead><tr><th scope="col">"Domain"</th><th scope="col" class="num">"Tasks"</th></tr></thead>
                                        <tbody>
                                            {r.domains.iter().map(|d| view! { <tr><td>{d.domain.clone()}</td><td class="num">{d.tasks}</td></tr> }).collect_view()}
                                        </tbody>
                                    </table>
                                </div>
                                <details class="disclosure">
                                    <summary>{format!("Task stream in run order ({} tasks)", r.stream.len())}</summary>
                                    <ol class="stream-list">
                                        {r.stream.iter().map(|e| view! { <li><span class="muted">{format!("[{}] ", e.domain)}</span><span class="mono">{e.task.clone()}</span></li> }).collect_view()}
                                    </ol>
                                </details>
                                <p class="small"><a href=job_link>"Open the preview job and its log"</a></p>
                            </div>
                        }
                        .into_any(),
                        _ => view! {
                            <Notice tone=Tone::Danger title="The preview did not succeed">
                                <p>{p.job.error.clone().unwrap_or_else(|| format!("Status: {}", p.job.status.as_str()))}</p>
                                <p><a href=job_link>"Open the job log"</a></p>
                            </Notice>
                        }
                        .into_any(),
                    }
                })
        }}
        <Problems wiz=wiz step=Step::Preview />
    }
}

#[component]
fn EstimateView(est: CostEstimate) -> impl IntoView {
    let usd = format::usd_range(est.usd_low, est.usd_high);
    view! {
        <div class="stats">
            <Stat label="Tasks × modes × k" value=format!("{} × {} × {}", est.tasks, est.modes, est.k) />
            <Stat label="Executor rollouts" value=format::int(est.rollouts) />
            <Stat label="Improver sessions" value=format::int(est.improver_sessions) />
            <Stat
                label="Estimated cost"
                value=usd.unwrap_or_else(|| "unknown".to_owned())
                hint=if est.basis == "history" { "from earlier runs in this experiment" } else { "from the profile's prices" }
            />
        </div>
        {(!est.notes.is_empty())
            .then(|| view! { <ul class="small">{est.notes.into_iter().map(|n| view! { <li>{n}</li> }).collect_view()}</ul> })}
    }
}

#[component]
fn EstimateStep(wiz: Wiz) -> impl IntoView {
    let estimate = LocalResource::new(move || {
        let tasks = wiz.preview_result().map(|p| p.total_tasks);
        let f = wiz.form.get();
        async move {
            match tasks {
                Some(t) => api::post::<_, CostEstimate>(&paths::estimate(), &f.estimate_request(t))
                    .await
                    .map(Some),
                None => Ok(None),
            }
        }
    });
    view! {
        <h2>"Estimate"</h2>
        <p class="muted">
            "What the sweep will consume, from upstream's budget identity and the profile's prices (or this experiment's history). "
            <Term id="spend">"Recorded spend"</Term>" during the run covers improver sessions only."
        </p>
        {move || match estimate.get() {
            None => view! { <Skeleton lines=3 /> }.into_any(),
            Some(Err(e)) => view! { <ErrorPanel error=e /> <p class="muted small">"You can still continue without an estimate."</p> }.into_any(),
            Some(Ok(None)) => view! { <p class="muted">"Run the preview first."</p> }.into_any(),
            Some(Ok(Some(est))) => view! { <EstimateView est=est /> }.into_any(),
        }}
    }
}

#[component]
fn ConfirmStep(wiz: Wiz) -> impl IntoView {
    let s = session();
    let t = toasts();
    let navigate = use_navigate();
    let busy = RwSignal::new(false);
    let submit_error = RwSignal::new(None::<String>);
    let budget_error = Signal::derive(move || wiz.form.with(RunForm::budget_value).err());
    let host = move || s.host_isolation();
    let start = move |_| {
        submit_error.set(None);
        match wiz.form.get_untracked().submit_spec(s.host_isolation()) {
            Ok(spec) => actions::start_job(spec, t, Some(busy), navigate.clone()),
            Err(p) => submit_error.set(Some(describe(&p).join(" "))),
        }
    };
    view! {
        <h2>"Confirm"</h2>
        {move || {
            let f = wiz.form.get();
            let tasks = wiz.preview_result().map(|p| p.total_tasks);
            view! {
                <dl class="kv">
                    <dt>"Experiment"</dt><dd>{f.experiment.clone()}</dd>
                    <dt>"Profile"</dt><dd>{f.profile.clone()}</dd>
                    <dt>"Benchmark"</dt><dd>{cell_label(&f.cell())}</dd>
                    <dt>"Budget k"</dt><dd>{f.k.clone()}</dd>
                    <dt>"Modes"</dt><dd>{modes_label(&f.modes())}</dd>
                    <dt>"Seed"</dt><dd>{if f.seed.is_empty() { "none (sorted order)".to_owned() } else { f.seed.clone() }}</dd>
                    <dt>"Split"</dt><dd>{match split_of(&wiz.split.get()) { Split::All => "all tasks".to_owned(), s => format!("{} part of {}", s.as_cli(), f.split_file) }}</dd>
                    <dt>"Domains"</dt><dd>{if f.only_domains.is_empty() { "all".to_owned() } else { f.only_domains.join(", ") }}</dd>
                    <dt>"Tasks"</dt><dd>{tasks.map_or_else(|| format::DASH.to_owned(), |t| format::int(u64::from(t)))}</dd>
                </dl>
            }
        }}
        <RadioCards
            name="run-kind"
            legend="What to start"
            value=wiz.run_kind
            options=vec![
                ("pipeline".to_owned(), "Full pipeline (recommended)".to_owned(), "check → run → judge → report, stopping at the first failure.".to_owned()),
                ("run".to_owned(), "Sweep only".to_owned(), "Just the refinement sweep; judge and report later from the sweep page.".to_owned()),
            ]
        />
        {move || {
            (wiz.run_kind.get() == "pipeline")
                .then(|| view! {
                    <Checkbox
                        id="run-judge-bases"
                        label="Also judge the base runs"
                        checked=wiz.judge_bases
                        help="Scores the unmodified harness too, so the report can show the improvement over it."
                    />
                })
        }}
        <TextField
            id="run-budget"
            label="Stop when recorded spend reaches (USD, optional)"
            value=wiz.budget
            inputmode="decimal"
            placeholder="25"
            error=budget_error
            help="Checked as task summaries land; a task in flight finishes first. Executor spend is not recorded, so real cost is higher."
        />
        {move || {
            host()
                .then(|| {
                    view! {
                        <Notice tone=Tone::Warning title="Jobs run on the server host">
                            <p>"Without container isolation the executor agent runs as the server's user, with that user's files and network access (upstream behaviour). That includes the Studio's own data: a malicious or prompt-injected executor can read and change its database (stored secrets, token hashes), so it is effectively a Studio administrator. Container isolation is the boundary."</p>
                            <Checkbox
                                id="run-ack"
                                label="I understand and want to run on the host"
                                checked=wiz.acknowledge
                            />
                        </Notice>
                    }
                })
        }}
        {move || submit_error.get().map(|e| view! { <Notice tone=Tone::Danger><p>{e}</p></Notice> })}
        <button
            type="button"
            class="btn btn-primary"
            data-testid="run-start"
            disabled=move || busy.get() || !s.can(Role::Operator) || (host() && !wiz.acknowledge.get())
            on:click=start
        >
            {icon(Icon::Play)}
            {move || if wiz.run_kind.get() == "pipeline" { " Start the pipeline" } else { " Start the sweep" }}
        </button>
    }
}

#[component]
pub(crate) fn NewRun() -> impl IntoView {
    let s = session();
    let exp_q = query_param("experiment");
    let preset_q = query_param("preset");
    let experiments = LocalResource::new(|| async {
        api::get::<Vec<Experiment>>(&paths::experiments(false)).await
    });
    let profiles =
        LocalResource::new(|| async { api::get::<Vec<ModelProfile>>(&paths::profiles()).await });
    let benchmarks = LocalResource::new(|| async {
        api::get::<Vec<BenchmarkStatus>>(&paths::benchmarks()).await
    });
    let splits =
        LocalResource::new(|| async { api::get::<Vec<SplitInfo>>(&paths::splits()).await });
    let experiment = RwSignal::new(exp_q.get_untracked());
    let profile = RwSignal::new(String::new());
    let bench = RwSignal::new("lab".to_owned());
    let subset = RwSignal::new("office".to_owned());
    let substrate = RwSignal::new("oc".to_owned());
    let k = RwSignal::new("1".to_owned());
    let parallel = RwSignal::new(true);
    let sequential = RwSignal::new(false);
    let seed = RwSignal::new(String::new());
    let split = RwSignal::new("all".to_owned());
    let split_file = RwSignal::new(String::new());
    let workers = RwSignal::new("1".to_owned());
    let only_domains = RwSignal::new(Vec::new());
    let max_tasks = RwSignal::new(String::new());
    let budget = RwSignal::new(String::new());
    let run_kind = RwSignal::new("pipeline".to_owned());
    let judge_bases = RwSignal::new(true);
    let acknowledge = RwSignal::new(false);
    let form = Memo::new(move |_| RunForm {
        experiment: experiment.get(),
        profile: profile.get(),
        bench: bench_of(&bench.get()),
        subset: subset_of(&subset.get()),
        substrate: if substrate.get() == "cx" {
            Substrate::Cx
        } else {
            Substrate::Oc
        },
        k: k.get(),
        parallel: parallel.get(),
        sequential: sequential.get(),
        seed: seed.get(),
        split: split_of(&split.get()),
        split_file: split_file.get(),
        workers: workers.get(),
        only_domains: only_domains.get(),
        max_tasks: max_tasks.get(),
        budget_usd: budget.get(),
        pipeline: run_kind.get() == "pipeline",
        judge_bases: judge_bases.get(),
        acknowledge_risks: acknowledge.get(),
    });
    let wiz = Wiz {
        step: RwSignal::new(Step::Cell),
        show_errors: RwSignal::new(false),
        experiment,
        profile,
        bench,
        subset,
        substrate,
        k,
        parallel,
        sequential,
        seed,
        split,
        split_file,
        workers,
        only_domains,
        max_tasks,
        budget,
        run_kind,
        judge_bases,
        acknowledge,
        experiments,
        profiles,
        benchmarks,
        splits,
        loaded_domains: RwSignal::new(None),
        domains_busy: RwSignal::new(false),
        preview: RwSignal::new(None),
        preview_busy: RwSignal::new(false),
        preview_error: RwSignal::new(None),
        form,
        create_open: RwSignal::new(false),
        toasts: toasts(),
    };
    // A different cell has different domains.
    Effect::new(move |prev: Option<(String, String)>| {
        let key = (bench.get(), subset.get());
        if prev.is_some_and(|p| p != key) {
            wiz.loaded_domains.set(None);
            wiz.only_domains.set(Vec::new());
        }
        key
    });
    // Choose a default profile once profiles load (when the experiment pins none).
    Effect::new(move |_| {
        if profile.get_untracked().is_empty()
            && let Some(Ok(list)) = profiles.get()
            && let Some(first) = list.first()
            && wiz.pinned().is_none()
        {
            profile.set(first.name.clone());
        }
    });
    // `?preset=smoke|pilot|paper` from the onboarding page.
    Effect::new(move |done: Option<bool>| {
        if done == Some(true) {
            return true;
        }
        let p = match preset_q.get().as_str() {
            "smoke" => Some(Preset::Smoke),
            "pilot" => Some(Preset::Pilot),
            "paper" => Some(Preset::Paper),
            _ => None,
        };
        let ready = benchmarks.get().is_some();
        if let (Some(p), true) = (p, ready) {
            wiz.apply_preset(p);
            true
        } else {
            p.is_none()
        }
    });
    let created = Callback::new(move |name: String| {
        experiments.refetch();
        experiment.set(name);
    });
    let go = move |to: Step| {
        let current = wiz.step.get_untracked();
        if to.index() > current.index() {
            for st in Step::ALL.iter().filter(|st| st.index() < to.index()) {
                if !wiz.problems(*st).is_empty() {
                    wiz.show_errors.set(true);
                    wiz.step.set(*st);
                    return;
                }
            }
        }
        wiz.show_errors.set(false);
        wiz.step.set(to);
        crate::browser::focus_id("wizard-body");
    };
    view! {
        <div class="page">
            <PageHeader
                title="New run"
                subtitle="Preview the exact task stream and the cost before anything runs."
            />
            {move || {
                (!s.can(Role::Operator))
                    .then(|| {
                        view! {
                            <Notice tone=Tone::Info title="Read-only">
                                <p>"Your role (viewer) can explore the options and estimates, but previews and runs need the operator role."</p>
                            </Notice>
                        }
                    })
            }}
            <ol class="stepper" aria-label="Steps">
                {Step::ALL
                    .iter()
                    .map(|st| {
                        let st = *st;
                        view! {
                            <li
                                aria-current=move || (wiz.step.get() == st).then_some("step")
                                class:done=move || st.index() < wiz.step.get().index()
                            >
                                <button type="button" class="link-btn step-btn" on:click=move |_| go(st)>
                                    {st.title()}
                                </button>
                            </li>
                        }
                    })
                    .collect_view()}
            </ol>
            <section class="card wizard-card" id="wizard-body" tabindex="-1" aria-live="polite">
                {move || match wiz.step.get() {
                    Step::Cell => view! { <CellStep wiz=wiz /> }.into_any(),
                    Step::Options => view! { <OptionsStep wiz=wiz /> }.into_any(),
                    Step::Preview => view! { <PreviewStep wiz=wiz /> }.into_any(),
                    Step::Estimate => view! { <EstimateStep wiz=wiz /> }.into_any(),
                    Step::Confirm => view! { <ConfirmStep wiz=wiz /> }.into_any(),
                }}
                <div class="wizard-actions">
                    <button
                        type="button"
                        class="btn btn-secondary"
                        disabled=move || wiz.step.get() == Step::Cell
                        on:click=move |_| go(wiz.step.get_untracked().prev())
                    >
                        "Back"
                    </button>
                    <div class="right">
                        {move || {
                            (wiz.step.get() != Step::Confirm)
                                .then(|| {
                                    view! {
                                        <button
                                            type="button"
                                            class="btn btn-primary"
                                            on:click=move |_| go(wiz.step.get_untracked().next())
                                        >
                                            {move || format!("Next: {}", wiz.step.get().next().title())}
                                        </button>
                                    }
                                })
                        }}
                    </div>
                </div>
            </section>
            <CreateExperiment open=wiz.create_open on_created=created />
            {move || {
                wiz.experiments.get().and_then(Result::ok).filter(|l| l.is_empty()).map(|_| {
                    view! {
                        <Card title="No experiments yet">
                            <EmptyState title="Create an experiment first" body="Runs write into an experiment. Create one, then come back here.">
                                <button type="button" class="btn btn-primary" on:click=move |_| wiz.create_open.set(true)>"New experiment"</button>
                            </EmptyState>
                        </Card>
                    }
                })
            }}
        </div>
    }
}
