//! One job, live: status, steps, progress, the result, and the log streamed over
//! server-sent events (with backfill from `GET /log`, reconnect with backoff, and polling
//! when the stream is unavailable).

use std::time::Duration;

use h2s_api::{Job, JobEvent, JobResult, JobSpec, LogChunk, LogStream, Role};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;
use web_sys::{EventSource, MessageEvent};

use crate::actions;
use crate::api;
use crate::browser;
use crate::components::icons::{Icon, icon};
use crate::components::ui::{
    Ago, Badge, Card, CodeBlock, Crumb, EmptyState, ErrorPanel, Notice, PageHeader, ProgressBar,
    Skeleton, StatusBadge, Term,
};
use crate::logic::format;
use crate::logic::jobs::{
    self, LiveMode, SSE_FAILURES_BEFORE_POLLING, Tone, apply_event, backoff_ms, can_cancel,
    kind_label, produced_sweep, progress_fraction, progress_text, spec_title,
};
use crate::logic::logbuf::{LogBuffer, Push, highlight, line_class};
use crate::logic::urls::{api as paths, route};
use crate::nav::param;
use crate::state::{ConfirmRequest, session, toasts, ui};

/// Browser handles of the live connection (not `Send`, kept in local storage).
#[derive(Default)]
struct Live {
    es: Option<EventSource>,
    on_message: Option<Closure<dyn FnMut(MessageEvent)>>,
    on_error: Option<Closure<dyn FnMut(web_sys::Event)>>,
    on_open: Option<Closure<dyn FnMut(web_sys::Event)>>,
    retry: Option<TimeoutHandle>,
    poll: Option<IntervalHandle>,
}

/// Everything the live controller needs; all handles, so it is `Copy`.
#[derive(Clone, Copy)]
struct Ctl {
    id: StoredValue<String>,
    job: RwSignal<Option<Job>>,
    log: RwSignal<LogBuffer>,
    mode: RwSignal<LiveMode>,
    failures: RwSignal<u32>,
    backfilling: StoredValue<bool>,
    live: StoredValue<Live, LocalStorage>,
}

impl Ctl {
    fn terminal(self) -> bool {
        self.job
            .with_untracked(|j| j.as_ref().is_some_and(|j| j.status.is_terminal()))
    }

    /// Fetch any lines the stream has not delivered (start-up, gaps, the end of a job).
    fn backfill(self) {
        if self.backfilling.try_get_value().unwrap_or(true) {
            return;
        }
        self.backfilling.set_value(true);
        spawn_local(async move {
            for _ in 0..50 {
                let Some(from) = self.log.try_with_untracked(LogBuffer::next) else {
                    break;
                };
                let url = paths::job_log(&self.id.get_value(), from, 2000);
                match api::get::<LogChunk>(&url).await {
                    Ok(chunk) => {
                        let empty = chunk.lines.is_empty();
                        let complete = chunk.complete;
                        self.log.try_update(|l| {
                            l.extend(chunk.lines);
                        });
                        if complete && self.terminal() {
                            self.mode.try_set(LiveMode::Ended);
                        }
                        if empty || complete {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            self.backfilling.try_set_value(false);
            // Lines that arrived while we were fetching may have left a new gap.
            if self
                .log
                .try_with_untracked(LogBuffer::has_gap)
                .unwrap_or(false)
                && !self.backfilling.try_get_value().unwrap_or(true)
            {
                self.backfill();
            }
        });
    }

    fn on_event(self, ev: JobEvent) {
        match ev {
            JobEvent::Snapshot { job } => {
                let behind = self.log.with_untracked(LogBuffer::next) < job.log_lines;
                let terminal = job.status.is_terminal();
                self.job.set(Some(*job));
                if behind {
                    self.backfill();
                }
                if terminal {
                    self.finish();
                }
            }
            JobEvent::Log { line } => {
                let n = line.n;
                let mut gap = false;
                self.log
                    .update(|l| gap = matches!(l.push(line), Push::Gap { .. }));
                self.job.update(|j| {
                    if let Some(j) = j {
                        j.log_lines = j.log_lines.max(n + 1);
                    }
                });
                if gap {
                    self.backfill();
                }
            }
            other => {
                let terminal =
                    matches!(&other, JobEvent::Status { status, .. } if status.is_terminal());
                self.job.update(|j| {
                    if let Some(j) = j {
                        apply_event(j, &other);
                    }
                });
                if terminal {
                    // Refresh the full job (result, finished time) and drain the log.
                    self.finish();
                    let id = self.id.get_value();
                    let job = self.job;
                    spawn_local(async move {
                        if let Ok(j) = api::get::<Job>(&paths::job(&id)).await {
                            job.try_set(Some(j));
                        }
                    });
                }
            }
        }
    }

    /// The job is over: stop listening and fetch the rest of the log.
    fn finish(self) {
        self.close();
        self.mode.set(LiveMode::Ended);
        self.backfill();
    }

    fn close(self) {
        self.live.try_update_value(|l| {
            if let Some(es) = l.es.take() {
                es.set_onmessage(None);
                es.set_onerror(None);
                es.set_onopen(None);
                es.close();
            }
            l.on_message = None;
            l.on_error = None;
            l.on_open = None;
            if let Some(t) = l.retry.take() {
                t.clear();
            }
            if let Some(p) = l.poll.take() {
                p.clear();
            }
        });
    }

    fn connect(self) {
        self.close();
        if self.terminal() {
            self.mode.set(LiveMode::Ended);
            return;
        }
        let url = paths::job_events(&self.id.get_value());
        let Ok(es) = EventSource::new(&url) else {
            self.start_polling();
            return;
        };
        if self.failures.get_untracked() == 0 {
            self.mode.set(LiveMode::Connecting);
        }
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |m: MessageEvent| {
            if let Some(text) = m.data().as_string()
                && let Ok(ev) = serde_json::from_str::<JobEvent>(&text)
            {
                self.on_event(ev);
            }
        });
        let on_open = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            self.failures.set(0);
            self.mode.set(LiveMode::Live);
        });
        let on_error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            if self.terminal() {
                self.finish();
                return;
            }
            let state = self
                .live
                .try_with_value(|l| l.es.as_ref().map(EventSource::ready_state))
                .flatten()
                .unwrap_or(EventSource::CLOSED);
            let failures = self.failures.get_untracked() + 1;
            self.failures.set(failures);
            self.refresh_once();
            if failures >= SSE_FAILURES_BEFORE_POLLING {
                self.start_polling();
            } else if state == EventSource::CLOSED {
                // The browser gave up (HTTP error): retry ourselves with backoff.
                self.mode.set(LiveMode::Reconnecting(failures));
                self.schedule_reconnect(failures);
            } else {
                // CONNECTING: the browser retries on its own.
                self.mode.set(LiveMode::Reconnecting(failures));
            }
        });
        es.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        es.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        self.live.update_value(|l| {
            l.es = Some(es);
            l.on_message = Some(on_message);
            l.on_error = Some(on_error);
            l.on_open = Some(on_open);
        });
    }

    fn schedule_reconnect(self, attempt: u32) {
        if let Some(es) = self.live.try_update_value(|l| l.es.take()).flatten() {
            es.close();
        }
        let delay = Duration::from_millis(u64::from(backoff_ms(attempt.saturating_sub(1))));
        if let Ok(h) = set_timeout_with_handle(move || self.connect(), delay) {
            self.live.update_value(|l| l.retry = Some(h));
        }
    }

    /// One `GET /jobs/{id}` plus a log backfill: keeps the page current while the stream
    /// is down (between reconnect attempts, and on every polling tick).
    fn refresh_once(self) {
        let id = self.id.get_value();
        spawn_local(async move {
            if let Ok(j) = api::get::<Job>(&paths::job(&id)).await {
                let terminal = j.status.is_terminal();
                self.job.try_set(Some(j));
                self.backfill();
                if terminal {
                    self.close();
                    self.mode.try_set(LiveMode::Ended);
                }
            }
        });
    }

    fn start_polling(self) {
        self.close();
        self.mode.set(LiveMode::Polling);
        let tick = move || self.refresh_once();
        tick();
        if let Ok(h) = set_interval_with_handle(tick, Duration::from_millis(2500)) {
            self.live.update_value(|l| l.poll = Some(h));
        }
    }
}

fn kv(label: &'static str, value: String) -> impl IntoView {
    view! {
        <dt>{label}</dt>
        <dd>{value}</dd>
    }
}

/// Render a job's parsed result by kind (pipelines render each finished step).
fn result_view(r: JobResult, experiment: Option<String>) -> AnyView {
    match r {
        JobResult::Install(x) => view! {
            <dl class="kv">
                {kv("harness2 commit", x.commit)}
                {kv("Version", x.version)}
                {kv("Python", x.python)}
                {kv("Extras", x.extras.iter().map(|e| format!("{e:?}").to_lowercase()).collect::<Vec<_>>().join(", "))}
            </dl>
        }
        .into_any(),
        JobResult::Demo(x) => view! {
            <dl class="kv">
                {kv("Tasks done", x.done.to_string())}
                {kv("Tasks failed", x.failed.to_string())}
                {kv("Executions", x.execution_count.to_string())}
                {kv("Editor calls", x.editor_calls.to_string())}
            </dl>
            {experiment.map(|e| view! { <a class="btn btn-primary" href=route::experiment(&e)>"Explore the demo results"</a> })}
        }
        .into_any(),
        JobResult::Setup(x) => view! {
            <p>{if x.dry_run { "Plan (nothing was changed):" } else { "Setup ran these steps:" }}</p>
            <ol class="plan">{x.plan.into_iter().map(|p| view! { <li><code>{p}</code></li> }).collect_view()}</ol>
            {(!x.notes.is_empty()).then(|| view! { <Notice tone=Tone::Info><ul>{x.notes.into_iter().map(|n| view! { <li>{n}</li> }).collect_view()}</ul></Notice> })}
        }
        .into_any(),
        JobResult::Check(x) => view! {
            <p>
                {if x.ready {
                    view! { <Badge tone=Tone::Success text="ready" /> }.into_any()
                } else {
                    view! { <Badge tone=Tone::Danger text="not ready" /> }.into_any()
                }}
                {x.tasks.map(|t| format!(" {} installed tasks", format::int(u64::from(t))))}
            </p>
            {(!x.failures.is_empty()).then(|| view! { <ul class="problem-list">{x.failures.into_iter().map(|f| view! { <li>{f}</li> }).collect_view()}</ul> })}
        }
        .into_any(),
        JobResult::Preview(x) => {
            let shown: Vec<_> = x.stream.iter().take(60).cloned().collect();
            let more = x.stream.len().saturating_sub(shown.len());
            view! {
                <p class="mono small">{x.header.clone()}</p>
                <dl class="kv">
                    {kv("Tasks", format!("{} in {} domains", x.total_tasks, x.total_domains))}
                    {kv("Namespaces", x.namespaces.iter().map(|n| format!("{} ({})", n.namespace, n.mode.as_cli())).collect::<Vec<_>>().join(", "))}
                    {kv("Workers", x.workers.to_string())}
                    {kv("Estimate", format!(
                        "{} rollouts, {} improver sessions{}",
                        format::int(x.estimate.rollouts),
                        format::int(x.estimate.improver_sessions),
                        format::usd_range(x.estimate.usd_low, x.estimate.usd_high).map(|u| format!(", {u}")).unwrap_or_default()
                    ))}
                </dl>
                <details class="disclosure">
                    <summary>{format!("Task stream ({} tasks)", x.stream.len())}</summary>
                    <ol class="stream-list">
                        {shown.into_iter().map(|e| view! { <li><span class="muted">{format!("[{}] ", e.domain)}</span><span class="mono">{e.task}</span></li> }).collect_view()}
                    </ol>
                    {(more > 0).then(|| view! { <p class="muted small">{format!("…and {more} more.")}</p> })}
                </details>
            }
            .into_any()
        }
        JobResult::Run(x) => {
            let link = experiment.map(|e| route::sweep(&e, &x.namespace));
            view! {
                <dl class="kv">
                    {kv("Sweep", x.namespace.clone())}
                    {kv("Tasks done", x.done.to_string())}
                    {kv("Tasks failed", x.failed.to_string())}
                    {(!x.dead_domains.is_empty()).then(|| kv("Domains that stopped", x.dead_domains.join(", ")))}
                    {x.record.map(|r| kv("Sweep record", r))}
                </dl>
                {link.map(|l| view! { <a class="btn btn-primary" href=l>"Open the sweep"</a> })}
                {x.next.map(|n| view! { <p class="small">"harness2 suggests next:"</p> <CodeBlock text=n copy=true /> })}
            }
            .into_any()
        }
        JobResult::Judge(x) => view! {
            <dl class="kv">
                {kv("Scored", if x.bases { "base (reference) executions".to_owned() } else { "the selected runs".to_owned() })}
                {kv("Rows", x.rows.to_string())}
                {kv("Graded", x.graded.to_string())}
                {kv("Ungraded", x.ungraded.to_string())}
            </dl>
            {(x.ungraded > 0).then(|| view! { <p class="small muted"><Term id="ungraded">"Ungraded"</Term>" rows are excluded from the score, not counted as zero."</p> })}
        }
        .into_any(),
        JobResult::Report(x) => view! { <CodeBlock text=x.text copy=true label="Report" /> }.into_any(),
        JobResult::Split(x) => view! {
            <dl class="kv">
                {kv("Split file", x.name)}
                {kv("Train tasks", x.train.to_string())}
                {kv("Test tasks", x.test.to_string())}
            </dl>
        }
        .into_any(),
        JobResult::Bridge(x) => view! {
            <dl class="kv">
                {kv("HARNESS2_CODEX_BASE_URL", x.base_url.clone())}
                {kv("Model", x.model)}
                {x.container_url.map(|c| kv("URL for containers", c))}
            </dl>
            <p class="small muted">"The bridge's key is stored as the secret HARNESS2_CODEX_API_KEY and never shown."</p>
        }
        .into_any(),
        JobResult::Pipeline(x) => view! {
            <ol class="pipeline-results">
                {x.steps
                    .into_iter()
                    .map(|step| {
                        let name = match &step {
                            JobResult::Check(_) => "check",
                            JobResult::Run(_) => "run",
                            JobResult::Judge(j) if j.bases => "judge (bases)",
                            JobResult::Judge(_) => "judge",
                            JobResult::Report(_) => "report",
                            JobResult::Preview(_) => "preview",
                            _ => "step",
                        };
                        view! {
                            <li>
                                <h3>{name}</h3>
                                {result_view(step, experiment.clone())}
                            </li>
                        }
                    })
                    .collect_view()}
            </ol>
        }
        .into_any(),
    }
}

#[component]
fn LogPanel(ctl: Ctl) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let out = RwSignal::new(true);
    let err = RwSignal::new(true);
    let sys = RwSignal::new(true);
    let follow = RwSignal::new(true);
    let limit = RwSignal::new(1000_usize);
    let view_lines = Memo::new(move |_| {
        let mut streams = Vec::new();
        if out.get() {
            streams.push(LogStream::Stdout);
        }
        if err.get() {
            streams.push(LogStream::Stderr);
        }
        if sys.get() {
            streams.push(LogStream::System);
        }
        let q = query.get();
        let lim = limit.get();
        ctl.log.with(|l| l.search(&q, &streams, lim))
    });
    Effect::new(move |_| {
        view_lines.track();
        if follow.get_untracked() {
            request_animation_frame(|| browser::scroll_to_bottom("job-log"));
        }
    });
    let download = move |_| {
        let id = ctl.id.get_value();
        let text = ctl.log.with_untracked(LogBuffer::to_text);
        browser::download_text(&format!("job-{id}.log"), "text/plain", &text);
    };
    view! {
        <Card title="Log">
            <div class="log-toolbar">
                <div class="field grow">
                    <label for="page-search">"Search the log"</label>
                    <input id="page-search" class="input" type="search" placeholder="Text to find (case-insensitive)" bind:value=query />
                </div>
                <fieldset class="chips" aria-label="Streams">
                    <label class="chip" class:selected=move || out.get()><input type="checkbox" bind:checked=out />"stdout"</label>
                    <label class="chip" class:selected=move || err.get()><input type="checkbox" bind:checked=err />"stderr"</label>
                    <label class="chip" class:selected=move || sys.get()><input type="checkbox" bind:checked=sys />"server"</label>
                </fieldset>
                <div class="field field-check">
                    <input id="log-follow" type="checkbox" bind:checked=follow />
                    <label for="log-follow">"Follow new lines"</label>
                </div>
                <button type="button" class="btn btn-secondary btn-sm" on:click=download>
                    {icon(Icon::Download)}
                    " Download"
                </button>
            </div>
            <p class="small muted" aria-live="polite">
                {move || {
                    let (shown, total) = (view_lines.with(|v| v.0.len()), view_lines.with(|v| v.1));
                    let dropped = ctl.log.with(LogBuffer::dropped);
                    let mut s = if query.get().is_empty() {
                        format!("{} lines", format::int(total as u64))
                    } else {
                        format!("{} matching lines", format::int(total as u64))
                    };
                    if shown < total {
                        s.push_str(&format!(", showing the last {}", format::int(shown as u64)));
                    }
                    if dropped > 0 {
                        s.push_str(&format!("; {} older lines are not kept in the browser (download keeps what is here)", format::int(dropped)));
                    }
                    s
                }}
            </p>
            <div
                id="job-log"
                class="log"
                role="log"
                aria-label="Job log"
                tabindex="0"
                data-testid="job-log"
                on:scroll=move |_| {
                    let at_bottom = browser::is_at_bottom("job-log");
                    if follow.get_untracked() != at_bottom {
                        follow.set(at_bottom);
                    }
                }
            >
                {move || {
                    view_lines.with(|v| v.0.is_empty()).then(|| view! { <p class="log-empty">"No log lines yet."</p> })
                }}
                <For each=move || view_lines.get().0 key=|l| l.n let:line>
                    {
                        let q = query.get_untracked();
                        let class = line_class(&line);
                        view! {
                            <div class=class>
                                <span class="log-n">{line.n + 1}</span>
                                <span class="log-text">
                                    {highlight(&line.text, &q)
                                        .into_iter()
                                        .map(|(seg, hit)| {
                                            if hit {
                                                view! { <mark>{seg}</mark> }.into_any()
                                            } else {
                                                seg.into_any()
                                            }
                                        })
                                        .collect_view()}
                                </span>
                            </div>
                        }
                    }
                </For>
            </div>
            {move || {
                (view_lines.with(|v| v.1 > v.0.len()))
                    .then(|| {
                        view! {
                            <button type="button" class="btn btn-ghost btn-sm" on:click=move |_| limit.update(|l| *l += 2000)>
                                "Show earlier lines"
                            </button>
                        }
                    })
            }}
        </Card>
    }
}

#[component]
pub(crate) fn JobPage() -> impl IntoView {
    let s = session();
    let t = toasts();
    let u = ui();
    let navigate = use_navigate();
    let id = param("id");
    let ctl = Ctl {
        id: StoredValue::new(id.get_untracked()),
        job: RwSignal::new(None),
        log: RwSignal::new(LogBuffer::default()),
        mode: RwSignal::new(LiveMode::Connecting),
        failures: RwSignal::new(0),
        backfilling: StoredValue::new(false),
        live: StoredValue::new_local(Live::default()),
    };
    let load_error = RwSignal::new(None::<api::ApiError>);
    let start = move || {
        ctl.close();
        ctl.id.set_value(id.get_untracked());
        ctl.job.set(None);
        ctl.log.set(LogBuffer::default());
        ctl.failures.set(0);
        load_error.set(None);
        spawn_local(async move {
            match api::get::<Job>(&paths::job(&ctl.id.get_value())).await {
                Ok(j) => {
                    let terminal = j.status.is_terminal();
                    ctl.job.set(Some(j));
                    ctl.backfill();
                    if terminal {
                        ctl.mode.set(LiveMode::Ended);
                    } else {
                        ctl.connect();
                    }
                }
                Err(e) => load_error.set(Some(e)),
            }
        });
    };
    Effect::new(move |_| {
        id.track();
        start();
    });
    on_cleanup(move || ctl.close());

    let cancel = move |_| {
        let jid = ctl.id.get_value();
        u.ask(ConfirmRequest {
            title: "Cancel this job?".to_owned(),
            body: "harness2 is stopped (SIGTERM to its process group, SIGKILL after 10 seconds). Finished tasks are kept; the task in flight is lost and starts over if you resume the sweep.".to_owned(),
            confirm_label: "Cancel job".to_owned(),
            danger: true,
            on_confirm: Callback::new(move |()| {
                let jid = jid.clone();
                spawn_local(async move {
                    match api::post_empty::<Job>(&paths::job_cancel(&jid)).await {
                        Ok(j) => {
                            t.info("Cancellation requested");
                            ctl.job.update(|cur| {
                                if let Some(cur) = cur {
                                    cur.status = j.status;
                                }
                            });
                        }
                        Err(e) => t.error("Could not cancel", &e),
                    }
                });
            }),
        });
    };
    let rerun_setup = {
        let navigate = navigate.clone();
        move |spec: JobSpec| {
            let navigate = navigate.clone();
            u.ask(ConfirmRequest {
                title: "Run setup for real?".to_owned(),
                body: "Setup clones the pinned benchmark code, applies patches, builds environments and downloads task data. It needs network access and can take a while.".to_owned(),
                confirm_label: "Run setup".to_owned(),
                danger: false,
                on_confirm: Callback::new(move |()| {
                    let mut spec = spec.clone();
                    if let JobSpec::Setup(p) = &mut spec {
                        p.dry_run = false;
                    }
                    actions::start_job(spec, t, None, navigate.clone());
                }),
            });
        }
    };

    let title = Signal::derive(move || {
        ctl.job.with(|j| {
            j.as_ref()
                .map_or_else(|| "Job".to_owned(), |j| spec_title(&j.spec))
        })
    });
    view! {
        <div class="page">
            <PageHeader
                title=title
                subtitle=Signal::derive(move || {
                    ctl.job.with(|j| j.as_ref().map(|j| format!("{} · {}", kind_label(j.kind), j.id)).unwrap_or_default())
                })
                crumbs=vec![Crumb("Jobs".into(), "/jobs".into())]
            >
                {move || {
                    let job = ctl.job.get()?;
                    let role = s.role()?;
                    let sweep = produced_sweep(&job);
                    let exp = job.spec.experiment().map(str::to_owned);
                    Some(view! {
                        {sweep.map(|(e, sw)| view! { <a class="btn btn-secondary" href=route::sweep(&e, &sw)>"Open the sweep"</a> })}
                        {exp.map(|e| view! { <a class="btn btn-secondary" href=route::experiment(&e)>"Experiment"</a> })}
                        {can_cancel(role, &job)
                            .then(|| view! { <button type="button" class="btn btn-danger" data-testid="job-cancel" on:click=cancel>"Cancel"</button> })}
                    })
                }}
            </PageHeader>
            {move || {
                if let Some(e) = load_error.get() {
                    if e.is_not_found() {
                        return view! {
                            <EmptyState title="Job not found" body="It may have been removed when its logs expired.">
                                <a class="btn btn-primary" href="/jobs">"All jobs"</a>
                            </EmptyState>
                        }
                        .into_any();
                    }
                    let retry = Callback::new(move |()| start());
                    return view! { <ErrorPanel error=e retry=retry /> }.into_any();
                }
                let Some(job) = ctl.job.get() else {
                    return view! { <Skeleton lines=6 /> }.into_any();
                };
                let status = job.status;
                let progress = job.progress.clone();
                let fraction = progress.as_ref().and_then(progress_fraction);
                let ptext = progress.as_ref().map(progress_text).unwrap_or_default();
                let current = progress.as_ref().and_then(|p| p.current.clone());
                let duration = match (&job.started_at, &job.finished_at) {
                    (Some(a), Some(b)) => format::span_between(a, b),
                    (Some(a), None) => format::parse_rfc3339(a).map(|t| format::span_secs((u.now.get() - t).max(0) as f64)),
                    _ => None,
                };
                let exp = job.spec.experiment().map(str::to_owned);
                let exp_link = exp.clone();
                let exp_result = exp;
                let is_dry_setup = matches!(&job.spec, JobSpec::Setup(p) if p.dry_run) && status == h2s_api::JobStatus::Succeeded;
                let spec_json = serde_json::to_string_pretty(&job.spec).unwrap_or_default();
                let rerun = rerun_setup.clone();
                let spec_for_rerun = job.spec.clone();
                view! {
                    <div class="split">
                        <Card title="Status">
                            <div class="pill-row">
                                <StatusBadge status=status testid="job-status" />
                                <span class=move || format!("live {}", ctl.mode.get().tone().class())>
                                    <span class="live-dot" aria-hidden="true"></span>
                                    <span role="status" aria-live="polite">{move || ctl.mode.get().label()}</span>
                                </span>
                            </div>
                            <p class="small muted">{jobs::status_help(status)}</p>
                            {(status == h2s_api::JobStatus::Running || progress.is_some())
                                .then(|| {
                                    // A finished job shows no "busy" bar and no current task.
                                    let finished = status.is_terminal();
                                    let current = current.filter(|_| !finished);
                                    view! {
                                        {(!finished || fraction.is_some())
                                            .then(|| view! { <ProgressBar fraction=fraction label=ptext.clone() /> })}
                                        <p class="small">{ptext.clone()}{current.map(|c| format!(" · now: {c}"))}</p>
                                    }
                                })}
                            <dl class="kv">
                                <dt>"Created"</dt>
                                <dd>{job.created_by.clone()}" · "<Ago ts=job.created_at.clone() /></dd>
                                {duration.map(|d| view! { <dt>"Duration"</dt><dd>{d}</dd> })}
                                {job.exit_code.map(|c| view! { <dt>"Exit code"</dt><dd>{c}</dd> })}
                                <dt>"Isolation"</dt>
                                <dd>
                                    <Term id="isolation">{match job.isolation { h2s_api::Isolation::Host => "host", h2s_api::Isolation::Container => "container" }}</Term>
                                </dd>
                                {job.spent_usd.map(|u| view! {
                                    <dt>"Recorded spend"</dt>
                                    <dd>{format::usd(u)}<span class="cell-sub"><Term id="spend">"improver and selection only"</Term></span></dd>
                                })}
                                {exp_link.map(|e| view! { <dt>"Experiment"</dt><dd><a href=route::experiment(&e)>{e.clone()}</a></dd> })}
                            </dl>
                            {job.error.clone().map(|e| view! { <Notice tone=Tone::Danger title="Error"><p>{e}</p></Notice> })}
                        </Card>
                        <Card title="Steps">
                            {if job.steps.is_empty() {
                                view! { <p class="muted small">"This job runs a single step."</p> }.into_any()
                            } else {
                                view! {
                                    <ol class="steps">
                                        {job.steps.iter().map(|st| {
                                            let d = match (&st.started_at, &st.finished_at) {
                                                (Some(a), Some(b)) => format::span_between(a, b),
                                                _ => None,
                                            };
                                            view! {
                                                <li>
                                                    <strong>{st.name.clone()}</strong>
                                                    <StatusBadge status=st.status />
                                                    {d.map(|d| view! { <span class="muted small">{d}</span> })}
                                                </li>
                                            }
                                        }).collect_view()}
                                    </ol>
                                }.into_any()
                            }}
                        </Card>
                    </div>
                    {job.result.map(|r| view! {
                        <Card title="Result">
                            {result_view(r, exp_result.clone())}
                            {(is_dry_setup && s.can(Role::Admin)).then(|| {
                                let spec = spec_for_rerun.clone();
                                let rerun = rerun.clone();
                                view! { <button type="button" class="btn btn-primary" on:click=move |_| rerun(spec.clone())>"Run setup for real"</button> }
                            })}
                        </Card>
                    })}
                    <details class="disclosure">
                        <summary>"Job specification (JSON)"</summary>
                        <CodeBlock text=spec_json copy=true label="Job specification" />
                    </details>
                }
                .into_any()
            }}
            <LogPanel ctl=ctl />
        </div>
    }
}
