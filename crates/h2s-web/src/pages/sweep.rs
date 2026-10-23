//! One sweep: tasks, the score report, the experience library and the improver audit trail.

use h2s_api::{
    AuditSession, Experiment, JobSpec, JudgeParams, LibraryDomain, Mode, Page, Role, ScoreReport,
    SweepSummary, TaskRow,
};
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::actions;
use crate::api;
use crate::browser;
use crate::components::charts::{ProvenanceBar, ScoreBars};
use crate::components::form::SelectField;
use crate::components::icons::{Icon, icon};
use crate::components::modal::Modal;
use crate::components::ui::{
    Badge, Card, CodeBlock, CopyButton, Crumb, Cursor, EmptyState, ErrorPanel, Notice, PageHeader,
    Pager, Skeleton, Term, loaded,
};
use crate::components::widgets::{HeadlineScores, mode_label};
use crate::logic::format;
use crate::logic::jobs::Tone;
use crate::logic::report::{NoteLevel, note_level};
use crate::logic::urls::{SweepRef, api as paths, route};
use crate::nav::{param, query_param, query_setter};
use crate::state::{session, toasts};

const TABS: [(&str, &str); 4] = [
    ("tasks", "Tasks"),
    ("report", "Report"),
    ("library", "Library"),
    ("audit", "Audit"),
];

fn status_tone(status: &str) -> Tone {
    match status {
        "ok" | "skipped" => Tone::Success,
        "partial" => Tone::Warning,
        "base-failed" => Tone::Danger,
        "running" => Tone::Info,
        _ => Tone::Neutral,
    }
}

/// Accessible tabs bound to the `?tab=` query parameter (arrow keys move between tabs).
#[component]
fn SweepTabs(active: Memo<String>) -> impl IntoView {
    let set_query = query_setter();
    let select = move |name: &str| {
        let value = if name == "tasks" { "" } else { name };
        set_query(&[("tab", value)]);
    };
    view! {
        <div class="tabs" role="tablist" aria-label="Sweep sections">
            {TABS
                .iter()
                .enumerate()
                .map(|(i, (name, label))| {
                    let name = *name;
                    let select = select.clone();
                    let select_key = select.clone();
                    view! {
                        <button
                            type="button"
                            role="tab"
                            class="tab"
                            id=format!("tab-{name}")
                            data-testid=format!("tab-{name}")
                            aria-controls=format!("panel-{name}")
                            aria-selected=move || if active.get() == name { "true" } else { "false" }
                            tabindex=move || if active.get() == name { "0" } else { "-1" }
                            on:click=move |_| select(name)
                            on:keydown=move |ev| {
                                let next = match ev.key().as_str() {
                                    "ArrowRight" => Some((i + 1) % TABS.len()),
                                    "ArrowLeft" => Some((i + TABS.len() - 1) % TABS.len()),
                                    "Home" => Some(0),
                                    "End" => Some(TABS.len() - 1),
                                    _ => None,
                                };
                                if let Some(n) = next {
                                    ev.prevent_default();
                                    select_key(TABS[n].0);
                                    browser::focus_id(&format!("tab-{}", TABS[n].0));
                                }
                            }
                        >
                            {*label}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
fn TasksTab(exp: Memo<String>, sweep: Memo<String>, modes: Vec<Mode>) -> impl IntoView {
    let domain = query_param("domain");
    let status = query_param("status");
    let set_query = query_setter();
    let cursor = Cursor::new();
    Effect::new(move |_| {
        domain.track();
        status.track();
        cursor.reset();
    });
    let all = LocalResource::new(move || {
        let (e, s) = (exp.get(), sweep.get());
        async move { api::get::<Page<TaskRow>>(&paths::tasks(&e, &s, "", "", 200, None)).await }
    });
    let page = LocalResource::new(move || {
        let (e, s, d, st, c) = (
            exp.get(),
            sweep.get(),
            domain.get(),
            status.get(),
            cursor.current.get(),
        );
        async move { api::get::<Page<TaskRow>>(&paths::tasks(&e, &s, &d, &st, 25, c.as_deref())).await }
    });
    let domain_sel = RwSignal::new(domain.get_untracked());
    let status_sel = RwSignal::new(status.get_untracked());
    {
        let set_query = set_query.clone();
        Effect::new(move |prev: Option<()>| {
            let d = domain_sel.get();
            if prev.is_some() {
                set_query(&[("domain", &d)]);
            }
        });
    }
    Effect::new(move |prev: Option<()>| {
        let st = status_sel.get();
        if prev.is_some() {
            set_query(&[("status", &st)]);
        }
    });
    let domain_options = Signal::derive(move || {
        let mut v = vec![(String::new(), "All domains".to_owned())];
        if let Some(Ok(p)) = all.get() {
            let mut ds: Vec<String> = p.items.into_iter().map(|t| t.domain).collect();
            ds.sort();
            ds.dedup();
            v.extend(ds.into_iter().map(|d| (d.clone(), d)));
        }
        v
    });
    let status_options = Signal::derive(|| {
        [
            ("", "Any status"),
            ("ok", "ok"),
            ("skipped", "skipped"),
            ("partial", "partial"),
            ("base-failed", "base failed"),
            ("running", "running"),
        ]
        .iter()
        .map(|(v, l)| ((*v).to_owned(), (*l).to_owned()))
        .collect::<Vec<_>>()
    });
    let next = Signal::derive(move || page.get().and_then(Result::ok).and_then(|p| p.next_cursor));
    view! {
        <div class="toolbar">
            <SelectField id="task-domain" label="Domain" value=domain_sel options=domain_options />
            <SelectField id="task-status" label="Status" value=status_sel options=status_options />
        </div>
        {move || {
            let modes = modes.clone();
            match page.get() {
                None => view! { <Skeleton lines=8 /> }.into_any(),
                Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
                Some(Ok(p)) if p.items.is_empty() => view! {
                    <EmptyState title="No tasks match" body="Clear the filters, or wait for the sweep to record its first task." />
                }
                .into_any(),
                Some(Ok(p)) => {
                    let (e, s) = (exp.get(), sweep.get());
                    view! {
                        <div class="table-wrap">
                            <table class="table">
                                <thead>
                                    <tr>
                                        <th scope="col">"Task"</th>
                                        <th scope="col">"Status"</th>
                                        {modes
                                            .iter()
                                            .map(|m| view! { <th scope="col">{format!("{} adopted", mode_label(*m))}</th> <th scope="col" class="num">{format!("{} score", mode_label(*m))}</th> })
                                            .collect_view()}
                                        <th scope="col" class="num">"Spend"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {p
                                        .items
                                        .into_iter()
                                        .map(|row| {
                                            let spend: f64 = row.modes.iter().map(|m| m.usd).sum();
                                            view! {
                                                <tr data-testid="task-row" data-task=row.slug.clone()>
                                                    <td>
                                                        <a class="cell-title mono" href=route::task(&e, &s, &row.slug)>{row.task_id.clone()}</a>
                                                        <span class="cell-sub">{row.domain.clone()}</span>
                                                    </td>
                                                    <td>
                                                        <Badge tone=status_tone(&row.status) text=row.status.clone() />
                                                        {(row.errors > 0).then(|| view! { <span class="cell-sub">{format::plural(u64::from(row.errors), "error", "errors")}</span> })}
                                                    </td>
                                                    {modes
                                                        .iter()
                                                        .map(|m| {
                                                            let o = row.modes.iter().find(|o| o.mode == *m).cloned();
                                                            match o {
                                                                Some(o) => view! {
                                                                    <td>
                                                                        <span class="mono">{o.chosen_label.clone().unwrap_or_else(|| format::DASH.to_owned())}</span>
                                                                        {o.chosen_is_base.then(|| view! { " " <Badge tone=Tone::Neutral text="kept base" /> })}
                                                                        {(o.fell_back > 0).then(|| view! { <span class="cell-sub">{format!("{} fell back", o.fell_back)}</span> })}
                                                                    </td>
                                                                    <td class="num">
                                                                        {o.score.map_or_else(|| format::DASH.to_owned(), format::frac_pct)}
                                                                        {o.all_pass.map(|a| view! { <span class="cell-sub">{if a { "all pass" } else { "not all pass" }}</span> })}
                                                                    </td>
                                                                }
                                                                .into_any(),
                                                                None => view! { <td>{format::DASH}</td> <td class="num">{format::DASH}</td> }.into_any(),
                                                            }
                                                        })
                                                        .collect_view()}
                                                    <td class="num">{format::usd(spend)}</td>
                                                </tr>
                                            }
                                        })
                                        .collect_view()}
                                </tbody>
                            </table>
                        </div>
                    }
                    .into_any()
                }
            }
        }}
        <Pager cursor=cursor next=next />
    }
}

#[component]
fn ReportTab(
    exp: Memo<String>,
    sweep: Memo<String>,
    summary: SweepSummary,
    profile: Option<String>,
) -> impl IntoView {
    let s = session();
    let t = toasts();
    let navigate = use_navigate();
    let report = LocalResource::new(move || {
        let (e, sw) = (exp.get(), sweep.get());
        async move { api::get::<ScoreReport>(&paths::report(&e, &sw)).await }
    });
    let busy = RwSignal::new(false);
    let judge = move |bases: bool| {
        let (Some(cell), Some(profile)) = (summary.cell, profile.clone()) else {
            t.warn(
                "Cannot judge from here",
                "This sweep has no benchmark cell or the experiment has no pinned profile.",
            );
            return;
        };
        actions::start_job(
            JobSpec::Judge(JudgeParams {
                experiment: exp.get_untracked(),
                cell,
                k: summary.k,
                seed: summary.seed,
                modes: summary.modes.clone(),
                bases,
                profile,
            }),
            t,
            Some(busy),
            navigate.clone(),
        );
    };
    let judge2 = judge.clone();
    view! {
        <div data-testid="report-view">
            {move || {
                let judge = judge.clone();
                let judge2 = judge2.clone();
                match report.get() {
                    None => view! { <Skeleton lines=6 /> }.into_any(),
                    Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
                    Some(Ok(r)) if r.summaries.iter().all(|m| m.n == 0 && m.n_ungraded == 0) => view! {
                        <EmptyState
                            title="Not judged yet"
                            body="Scores come from judging the run each task adopted. Judging happens after the sweep and never influences selection. Judge the base runs too to see the score to beat."
                        >
                            {move || {
                                let judge = judge.clone();
                                let judge2 = judge2.clone();
                                s.can(Role::Operator)
                                    .then(|| {
                                        view! {
                                            <button type="button" class="btn btn-primary" disabled=move || busy.get() on:click=move |_| judge(false)>
                                                "Judge this sweep"
                                            </button>
                                            <button type="button" class="btn btn-secondary" disabled=move || busy.get() on:click=move |_| judge2(true)>
                                                "Judge the base runs"
                                            </button>
                                        }
                                    })
                            }}
                        </EmptyState>
                    }
                    .into_any(),
                    Some(Ok(r)) => {
                        let text = r.text.clone();
                        let file = format!("{}-report.txt", sweep.get_untracked());
                        let text_dl = text.clone();
                        let summaries = r.summaries.clone();
                        let rows = r.summaries.clone();
                        let notes = r.notes.clone();
                        let best = r.summaries.iter().filter_map(|s| s.score).fold(f64::NAN, f64::max);
                        let base_line = r.base_score.map(|b| {
                            view! {
                                <p class="small">
                                    "The unmodified harness ("<Term id="base">"base"</Term>") scored "<strong>{format::pct(b)}</strong>
                                    {best.is_finite().then(|| format!("; the best mode is {} from it.", format::signed_points(best - b)))}
                                </p>
                            }
                        });
                        let notes_view = if notes.is_empty() {
                            view! { <p class="muted">"No notes or warnings."</p> }.into_any()
                        } else {
                            notes
                                .into_iter()
                                .map(|n| {
                                    let tone = match note_level(&n) {
                                        NoteLevel::Warning => Tone::Warning,
                                        NoteLevel::Note => Tone::Info,
                                        NoteLevel::Other => Tone::Neutral,
                                    };
                                    view! { <Notice tone=tone><p>{n}</p></Notice> }
                                })
                                .collect_view()
                                .into_any()
                        };
                        view! {
                            <div class="split">
                                <Card title="Scores">
                                    <ScoreBars summaries=summaries base_score=r.base_score />
                                    {base_line}
                                    <p class="small muted">
                                        <Term id="partial-credit">"Score"</Term>" is mean partial credit over graded tasks; "
                                        <Term id="all-pass">"all-pass"</Term>" counts tasks with every criterion met. "
                                        <Term id="ungraded">"Ungraded tasks"</Term>" are excluded, not counted as zero."
                                    </p>
                                </Card>
                                <Card title="Notes from the report">
                                    {notes_view}
                                </Card>
                            </div>
                            <Card title="By mode">
                                <div class="table-wrap">
                                    <table class="table">
                                        <thead>
                                            <tr>
                                                <th scope="col">"Mode"</th>
                                                <th scope="col" class="num">"Graded"</th>
                                                <th scope="col" class="num">"Ungraded"</th>
                                                <th scope="col" class="num">"Score"</th>
                                                <th scope="col" class="num">"All-pass"</th>
                                                <th scope="col">"Adopted harness came from"</th>
                                                <th scope="col" class="num">"Fell back"</th>
                                                <th scope="col" class="num">"Spend"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {rows
                                                .into_iter()
                                                .map(|m| {
                                                    view! {
                                                        <tr>
                                                            <th scope="row">{format!("{} · k={}", mode_label(m.mode), m.k)}</th>
                                                            <td class="num">{m.n}</td>
                                                            <td class="num">{m.n_ungraded}</td>
                                                            <td class="num">{format::opt_pct(m.score)}</td>
                                                            <td class="num">{format::opt_pct(m.all_pass)}</td>
                                                            <td>
                                                                <ProvenanceBar summary=m.clone() />
                                                                <span class="cell-sub">
                                                                    {format!("{} final · {} candidate · {} kept base", m.chose_final, m.chose_candidate, m.kept_base)}
                                                                </span>
                                                            </td>
                                                            <td class="num">{m.fell_back}</td>
                                                            <td class="num">{format::usd(m.usd)}</td>
                                                        </tr>
                                                    }
                                                })
                                                .collect_view()}
                                        </tbody>
                                    </table>
                                </div>
                                <div class="legend" aria-hidden="true">
                                    <span><i class="swatch score"></i>"composed final"</span>
                                    <span><i class="swatch allpass"></i>"single candidate"</span>
                                    <span><i class="swatch neutral"></i>"kept base"</span>
                                </div>
                            </Card>
                            <Card title="Report text">
                                <p class="muted small">"The block in harness2-report's format, for papers and tickets."</p>
                                <CodeBlock text=text copy=true label="Report text" />
                                <div class="pill-row">
                                    <button
                                        type="button"
                                        class="btn btn-secondary btn-sm"
                                        on:click=move |_| {
                                            browser::download_text(&file, "text/plain", &text_dl);
                                        }
                                    >
                                        {icon(Icon::Download)}
                                        " Download .txt"
                                    </button>
                                    <a
                                        class="btn btn-secondary btn-sm"
                                        href=paths::scores(&exp.get_untracked(), &sweep.get_untracked())
                                        download=format!("{}-judged.json", sweep.get_untracked())
                                        rel="external"
                                    >
                                        {icon(Icon::Download)}
                                        " Judged rows (JSON)"
                                    </a>
                                </div>
                            </Card>
                        }
                        .into_any()
                    }
                }
            }}
        </div>
    }
}

#[component]
fn LibraryTab(exp: Memo<String>, sweep: Memo<String>) -> impl IntoView {
    let lib = LocalResource::new(move || {
        let (e, s) = (exp.get(), sweep.get());
        async move { api::get::<Vec<LibraryDomain>>(&paths::library(&e, &s)).await }
    });
    let mode_filter = RwSignal::new(String::new());
    let mode_options = Signal::derive(|| {
        vec![
            (String::new(), "Both modes".to_owned()),
            ("parallel".to_owned(), "parallel".to_owned()),
            ("sequential".to_owned(), "sequential".to_owned()),
        ]
    });
    view! {
        <div data-testid="library-view">
            <p class="muted">
                "The "<Term id="library">"experience library"</Term>
                " of each mode and domain: which harness every task adopted, why, and the "
                <Term id="reflection">"lesson"</Term>" the reflector drew from it. Later tasks of the same domain start from these."
            </p>
            <div class="toolbar">
                <SelectField id="lib-mode" label="Mode" value=mode_filter options=mode_options />
            </div>
            {move || match lib.get() {
                None => view! { <Skeleton lines=6 /> }.into_any(),
                Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
                Some(Ok(list)) if list.is_empty() => view! {
                    <EmptyState
                        title="The library is empty"
                        body="Entries appear as tasks adopt harnesses. Tasks that kept the base harness add no entry."
                    />
                }
                .into_any(),
                Some(Ok(list)) => {
                    let mf = mode_filter.get();
                    let (e, s) = (exp.get(), sweep.get());
                    list.into_iter()
                        .filter(|d| mf.is_empty() || d.mode.as_cli() == mf)
                        .map(|d| {
                            let (e, s) = (e.clone(), s.clone());
                            view! {
                                <Card title=format!("{} · {} ({} entries)", d.domain, mode_label(d.mode), d.entries.len())>
                                    {d.entries
                                        .into_iter()
                                        .map(|entry| {
                                            view! {
                                                <article class="lib-entry">
                                                    <div class="stage-head">
                                                        <a class="mono" href=route::task(&e, &s, &entry.task_slug)>{entry.task_slug.clone()}</a>
                                                        <Badge tone=Tone::Info text=format!("adopted {}", entry.adopted_label) />
                                                        <span class="muted small">{entry.components.clone()}</span>
                                                    </div>
                                                    <p class="small"><strong>"Why: "</strong>{entry.why.clone()}</p>
                                                    {entry
                                                        .experience
                                                        .map(|x| {
                                                            view! {
                                                                <dl class="lesson">
                                                                    <dt>"Cue"</dt>
                                                                    <dd>{x.cue}</dd>
                                                                    <dt>"Lesson"</dt>
                                                                    <dd>{x.lesson}</dd>
                                                                    <dt>"Evidence"</dt>
                                                                    <dd>{x.evidence}</dd>
                                                                    <dt>"Confidence"</dt>
                                                                    <dd>{x.confidence}</dd>
                                                                </dl>
                                                                {(!x.edits.is_empty())
                                                                    .then(|| {
                                                                        view! {
                                                                            <details class="disclosure">
                                                                                <summary>"Per-edit account"</summary>
                                                                                <pre class="text-view">{x.edits}</pre>
                                                                            </details>
                                                                        }
                                                                    })}
                                                            }
                                                        })}
                                                </article>
                                            }
                                        })
                                        .collect_view()}
                                </Card>
                            }
                        })
                        .collect_view()
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
fn AuditTab(exp: Memo<String>, sweep: Memo<String>) -> impl IntoView {
    let sessions = LocalResource::new(move || {
        let (e, s) = (exp.get(), sweep.get());
        async move { api::get::<Vec<AuditSession>>(&paths::sweep_audit(&e, &s)).await }
    });
    let viewing = RwSignal::new(None::<(String, String)>);
    let open = Signal::derive(move || viewing.with(Option::is_some));
    let close = Callback::new(move |()| viewing.set(None));
    let text = LocalResource::new(move || {
        let v = viewing.get();
        let (e, s) = (exp.get(), sweep.get());
        async move {
            match v {
                Some((session, part)) => {
                    api::get_text(&paths::sweep_audit_part(&e, &s, &session, &part))
                        .await
                        .map(Some)
                }
                None => Ok(None),
            }
        }
    });
    let title = Signal::derive(move || {
        viewing
            .get()
            .map(|(s, p)| format!("{p} · {s}"))
            .unwrap_or_default()
    });
    view! {
        <p class="muted">
            "Every "<Term id="improver-session">"improver session"</Term>" and selector session of this sweep: the model, cost, attempts and wall time, and the exact mission and response text."
        </p>
        {loaded(
            sessions,
            6,
            move |list| {
                if list.is_empty() {
                    return view! { <EmptyState title="No sessions recorded" body="The audit trail fills as improver sessions run." /> }.into_any();
                }
                let total: f64 = list.iter().map(|s| s.cost_usd).sum();
                let wall: f64 = list.iter().map(|s| s.wall_s).sum();
                let n = list.len();
                view! {
                    <p class="small">
                        {format!("{n} sessions · {} · {} wall time in total", format::usd(total), format::duration_secs(wall))}
                    </p>
                    <div class="table-wrap">
                        <table class="table">
                            <thead>
                                <tr>
                                    <th scope="col">"Session"</th>
                                    <th scope="col">"Channel"</th>
                                    <th scope="col">"Model"</th>
                                    <th scope="col" class="num">"Attempts"</th>
                                    <th scope="col" class="num">"Wall time"</th>
                                    <th scope="col" class="num">"Cost"</th>
                                    <th scope="col">"Text"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {list
                                    .into_iter()
                                    .map(|s| {
                                        let id = s.id.clone();
                                        view! {
                                            <tr>
                                                <td>
                                                    <span class="cell-title mono">{s.label.clone()}</span>
                                                    <span class="cell-sub mono">{s.id.clone()}</span>
                                                    {(!s.ok).then(|| view! { <Badge tone=Tone::Danger text="failed" /> })}
                                                    {(!s.error.is_empty()).then(|| view! { <span class="cell-sub">{s.error.clone()}</span> })}
                                                </td>
                                                <td>{s.channel.clone()}<span class="cell-sub">{format!("effort {}", s.effort)}</span></td>
                                                <td class="mono small">{s.model.clone()}</td>
                                                <td class="num">{s.attempts}</td>
                                                <td class="num">{format::duration_secs(s.wall_s)}</td>
                                                <td class="num">{format::usd(s.cost_usd)}</td>
                                                <td>
                                                    <span class="pill-row">
                                                        {s.parts
                                                            .iter()
                                                            .map(|p| {
                                                                let (id, p2) = (id.clone(), p.clone());
                                                                view! {
                                                                    <button type="button" class="btn btn-secondary btn-sm" on:click=move |_| viewing.set(Some((id.clone(), p2.clone())))>
                                                                        {p.clone()}
                                                                    </button>
                                                                }
                                                            })
                                                            .collect_view()}
                                                    </span>
                                                </td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()}
                            </tbody>
                        </table>
                    </div>
                }
                .into_any()
            },
        )}
        <Modal open=open on_close=close title=title wide=true>
            {move || match text.get() {
                None => view! { <Skeleton lines=8 /> }.into_any(),
                Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
                Some(Ok(None)) => ().into_any(),
                Some(Ok(Some(body))) => {
                    let copy = body.clone();
                    view! {
                        <div class="code-actions-row"><CopyButton text=copy /></div>
                        <pre class="text-view" tabindex="0">{body}</pre>
                    }
                    .into_any()
                }
            }}
        </Modal>
    }
}

#[component]
pub(crate) fn SweepPage() -> impl IntoView {
    let exp = param("exp");
    let sweep = param("sweep");
    let tab_q = query_param("tab");
    let active = Memo::new(move |_| {
        let t = tab_q.get();
        if TABS.iter().any(|(n, _)| *n == t) {
            t
        } else {
            "tasks".to_owned()
        }
    });
    let experiment = LocalResource::new(move || {
        let e = exp.get();
        async move { api::get::<Experiment>(&paths::experiment(&e)).await }
    });
    let info = Memo::new(move |_| {
        experiment.get().and_then(Result::ok).and_then(|e| {
            let sw = e.sweeps.iter().find(|s| s.id == sweep.get())?.clone();
            Some((sw, e.profile))
        })
    });
    let subtitle = Signal::derive(move || {
        info.get()
            .map(|(s, _)| {
                format!(
                    "{} · {} · k={}{} · {}",
                    s.bench_name,
                    s.substrate.display_name(),
                    s.k,
                    s.seed.map(|x| format!(" · seed {x}")).unwrap_or_default(),
                    s.modes
                        .iter()
                        .map(|m| mode_label(*m))
                        .collect::<Vec<_>>()
                        .join(" + ")
                )
            })
            .unwrap_or_default()
    });
    let crumbs = move || {
        vec![
            Crumb("Experiments".into(), "/experiments".into()),
            Crumb(exp.get_untracked(), route::experiment(&exp.get_untracked())),
        ]
    };
    view! {
        <div class="page">
            <PageHeader title=Signal::derive(move || sweep.get()) subtitle=subtitle crumbs=crumbs()>
                {move || {
                    info.get()
                        .map(|(s, _)| {
                            let a = SweepRef { experiment: exp.get(), sweep: s.id.clone(), mode: s.modes.first().copied().unwrap_or(Mode::Parallel) };
                            let href = format!("/compare?a={}", crate::logic::urls::enc(&a.to_param()));
                            view! {
                                <a class="btn btn-secondary" href=href>
                                    {icon(Icon::Compare)}
                                    " Compare…"
                                </a>
                            }
                        })
                }}
            </PageHeader>
            {move || match experiment.get() {
                None => view! { <Skeleton lines=3 /> }.into_any(),
                Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
                Some(Ok(_)) => match info.get() {
                    None => view! {
                        <EmptyState title="Sweep not found" body="This experiment has no sweep with that id. It may have been renamed, or not started yet.">
                            <a class="btn btn-primary" href=route::experiment(&exp.get())>"Back to the experiment"</a>
                        </EmptyState>
                    }
                    .into_any(),
                    Some((s, profile)) => {
                        let modes = s.modes.clone();
                        view! {
                            <div class="stats">
                                <div class="stat">
                                    <span class="stat-label">"Tasks"</span>
                                    <span class="stat-value">{s.tasks}</span>
                                    <span class="stat-hint">{format!("{} ok · {} partial · {} failed", s.ok, s.partial, s.failed)}</span>
                                </div>
                                <div class="stat">
                                    <span class="stat-label">"Recorded spend"</span>
                                    <span class="stat-value">{format::usd(s.usd)}</span>
                                    <span class="stat-hint"><Term id="spend">"improver and selection only"</Term></span>
                                </div>
                                <div class="stat stat-wide">
                                    <span class="stat-label">"Headline"</span>
                                    <HeadlineScores scores=s.headline.clone() />
                                </div>
                            </div>
                            <SweepTabs active=active />
                            <div
                                class="tab-panel"
                                role="tabpanel"
                                id=move || format!("panel-{}", active.get())
                                aria-labelledby=move || format!("tab-{}", active.get())
                            >
                                {move || match active.get().as_str() {
                                    "report" => view! { <ReportTab exp=exp sweep=sweep summary=s.clone() profile=profile.clone() /> }.into_any(),
                                    "library" => view! { <LibraryTab exp=exp sweep=sweep /> }.into_any(),
                                    "audit" => view! { <AuditTab exp=exp sweep=sweep /> }.into_any(),
                                    _ => view! { <TasksTab exp=exp sweep=sweep modes=modes.clone() /> }.into_any(),
                                }}
                            </div>
                        }
                        .into_any()
                    }
                },
            }}
        </div>
    }
}
