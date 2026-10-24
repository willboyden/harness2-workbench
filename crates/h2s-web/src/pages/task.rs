//! One task: the recursion timeline per mode, the harness trees (by component, with the
//! always-on budget), diffs between any two harnesses, exports, and the rollout browser.

use h2s_api::{
    ExportBundle, ExportTarget, FileContent, FileEntry, FileKind, HarnessDiff, HarnessRole,
    HarnessTree, Mode, ModeRecord, Selection, Stage, TaskDetail,
};
use leptos::prelude::*;

use crate::api;
use crate::browser;
use crate::components::form::{RadioCards, SelectField};
use crate::components::icons::{Icon, icon};
use crate::components::modal::Modal;
use crate::components::ui::{
    Badge, Card, CodeBlock, Crumb, EmptyState, ErrorPanel, Meter, Notice, PageHeader, Skeleton,
    Term, loaded,
};
use crate::logic::diff::{LineKind, change_label, file_lines, totals};
use crate::logic::format;
use crate::logic::harness::{
    self, BudgetLevel, budget, default_diff_pair, group_files, harness_labels, rollout_labels,
};
use crate::logic::jobs::Tone;
use crate::logic::report::judge_count;
use crate::logic::tasks::{dim_label, ledger_problems, phases, selection_summary, stage_outcome};
use crate::logic::trajectory::{self, TrajKind};
use crate::logic::urls::{api as paths, route};
use crate::nav::param;

/// Strip the improver's `<notes>` wrapper for display.
fn clean_notes(notes: &str) -> String {
    notes
        .trim()
        .trim_start_matches("<notes>")
        .trim_end_matches("</notes>")
        .trim()
        .to_owned()
}

fn role_tone(r: HarnessRole) -> Tone {
    match r {
        HarnessRole::Base => Tone::Neutral,
        HarnessRole::Candidate => Tone::Info,
        HarnessRole::Final => Tone::Success,
    }
}

/// Identifies the task for API calls.
#[derive(Clone, Copy)]
struct TaskKey {
    exp: Memo<String>,
    sweep: Memo<String>,
    task: Memo<String>,
}

#[component]
fn StageCard(stage: Stage) -> impl IntoView {
    let (outcome, tone) = stage_outcome(&stage);
    let notes = clean_notes(&stage.notes);
    let long = notes.lines().count() > 4 || notes.len() > 320;
    let expanded = RwSignal::new(false);
    view! {
        <li class="stage">
            <div class="stage-head">
                <span class="stage-label">{stage.label.clone()}</span>
                <Badge tone=Tone::Neutral text=dim_label(&stage.dim).to_owned() />
                <Badge tone=tone text=outcome />
                {stage.cached.then(|| view! { <Badge tone=Tone::Info text="cached" /> })}
            </div>
            <p class="muted small">
                {format!("{} · {}", format::plural(u64::from(stage.attempts), "attempt", "attempts"), format::usd(stage.usd))}
            </p>
            {(!notes.is_empty())
                .then(|| {
                    view! {
                        <p class="stage-notes" class:clamp=move || long && !expanded.get()>{notes.clone()}</p>
                        {long
                            .then(|| {
                                view! {
                                    <button type="button" class="link-btn small" on:click=move |_| expanded.update(|e| *e = !*e)>
                                        {move || if expanded.get() { "Show less" } else { "Show full notes" }}
                                    </button>
                                }
                            })}
                    }
                })}
            {(!stage.error.is_empty()).then(|| view! { <p class="stage-error">{stage.error.clone()}</p> })}
        </li>
    }
}

#[component]
fn SelectionStep(sel: Selection, n: usize) -> impl IntoView {
    let summary = selection_summary(&sel);
    view! {
        <li class="timeline-step">
            <span class="timeline-marker" aria-hidden="true">{n}</span>
            <h3>
                <Term id="selection">"Blind selection"</Term>
            </h3>
            <p>{summary}</p>
            {(!sel.trivial)
                .then(|| {
                    view! {
                        <p class="small muted">"What the selector saw → which harness it was:"</p>
                        <ul class="idmap" aria-label="Anonymised candidate map">
                            {sel
                                .id_map
                                .iter()
                                .map(|a| {
                                    let chosen = a.label == sel.chosen_label;
                                    view! {
                                        <li class:chosen=chosen>
                                            {format!("{} → {}", a.id, a.label)}
                                            {chosen.then_some(" (adopted)")}
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                })}
            {(!sel.why.is_empty())
                .then(|| view! { <blockquote class="why">{sel.why.clone()}</blockquote> })}
            <p class="small muted">
                {format!(
                    "Compared: {} · cost {}{}",
                    sel.evidence,
                    format::usd(sel.usd),
                    sel.harness_fingerprint.as_ref().map(|f| format!(" · adopted fingerprint {f}")).unwrap_or_default()
                )}
            </p>
            {(!sel.dropped.is_empty())
                .then(|| view! { <p class="small">{format!("Dropped before selection: {}", sel.dropped.join(", "))}</p> })}
            {(!sel.error.is_empty()).then(|| view! { <p class="stage-error">{sel.error.clone()}</p> })}
        </li>
        <li class="timeline-step">
            <span class="timeline-marker" aria-hidden="true">{n + 1}</span>
            <h3>
                <Term id="reflection">"Reflection lesson"</Term>
            </h3>
            {match sel.experience.clone() {
                Some(x) => view! {
                    <p>"What the reflector stored in the experience library for later tasks of this domain:"</p>
                    <dl class="lesson">
                        <dt>"Cue"</dt>
                        <dd>{x.cue}</dd>
                        <dt>"Lesson"</dt>
                        <dd>{x.lesson}</dd>
                        <dt>"Evidence"</dt>
                        <dd>{x.evidence}</dd>
                        <dt>"Confidence"</dt>
                        <dd>{if x.confidence.is_empty() { format::DASH.to_owned() } else { x.confidence }}</dd>
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
                .into_any(),
                None => view! {
                    <p class="muted">
                        {if sel.chosen_is_base {
                            "No lesson: the selector kept the base harness, so nothing was adopted."
                        } else {
                            "No lesson was recorded for this adoption."
                        }}
                    </p>
                }
                .into_any(),
            }}
        </li>
    }
}

#[component]
fn Timeline(detail: TaskDetail, record: ModeRecord) -> impl IntoView {
    let ph = phases(&record);
    let n_phases = ph.len();
    let phase_views = ph
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let stages: Vec<Stage> = p.stages.into_iter().cloned().collect();
            view! {
                <li class="timeline-step">
                    <span class="timeline-marker" aria-hidden="true">{i + 2}</span>
                    <h3>{p.title.clone()}</h3>
                    <p>{p.explanation}</p>
                    <ul class="stage-list">
                        {stages.into_iter().map(|s| view! { <StageCard stage=s /> }).collect_view()}
                    </ul>
                </li>
            }
        })
        .collect_view();
    let ledger = record.ledger.clone();
    let similarity = record.similarity.clone();
    view! {
        <ol class="timeline">
            <li class="timeline-step">
                <span class="timeline-marker" aria-hidden="true">"1"</span>
                <h3>
                    <Term id="base">"Base rollouts"</Term>
                </h3>
                <p>{format!("{} reference executions of the unmodified harness (shared by both modes).", detail.k)}</p>
                <div class="pill-row">
                    {detail
                        .base
                        .iter()
                        .map(|b| {
                            let tone = if b.available { Tone::Success } else { Tone::Danger };
                            let text = if b.available { format!("{} ✓", b.label) } else { format!("{} missing", b.label) };
                            view! { <Badge tone=tone text=text /> }
                        })
                        .collect_view()}
                </div>
            </li>
            {phase_views}
            {match record.selection.clone() {
                Some(sel) => view! { <SelectionStep sel=sel n=n_phases + 2 /> }.into_any(),
                None => view! {
                    <li class="timeline-step">
                        <span class="timeline-marker" aria-hidden="true">{n_phases + 2}</span>
                        <h3>"Selection"</h3>
                        <p class="muted">"No selection was recorded (the mode did not finish)."</p>
                    </li>
                }
                .into_any(),
            }}
        </ol>
        <div class="grid-2">
            {ledger
                .map(|l| {
                    let problems = ledger_problems(&l);
                    view! {
                        <div class="budget">
                            <div class="budget-head">
                                <strong><Term id="ledger">"Ledger"</Term></strong>
                                <span>{format!("recorded {}", format::usd(l.usd))}</span>
                            </div>
                            <span class="small">{format!("Rollouts R = {} of {} expected · improver sessions P = {} of {} expected · {} session attempts", l.r, l.expected_r, l.p, l.expected_p, l.session_attempts)}</span>
                            {if problems.is_empty() {
                                view! { <span class="small muted">"The ledger balances the budget identity (R = 4k, P = 3k)."</span> }.into_any()
                            } else {
                                problems.into_iter().map(|p| view! { <span class="small field-error">{p}</span> }).collect_view().into_any()
                            }}
                        </div>
                    }
                })}
            {similarity
                .map(|s| {
                    view! {
                        <div class="budget">
                            <div class="budget-head"><strong>"Diversity of the finals"</strong></div>
                            <span class="small">{format!("{} finals, {} distinct; mean pairwise line-Jaccard similarity {:.2} (1.00 = identical).", s.n, s.n_distinct, s.mean_line_jaccard)}</span>
                        </div>
                    }
                })}
        </div>
        {(!record.scores.is_empty())
            .then(|| {
                view! {
                    <p class="small">
                        <strong>"Judged: "</strong>
                        {record
                            .scores
                            .iter()
                            .map(|r| {
                                if r.graded {
                                    let pct = r.frac.map_or_else(|| format::DASH.to_owned(), format::frac_pct);
                                    let what = match judge_count(r.n_passed, r.n_criteria, r.points_passed, r.points_max, r.metric.as_deref()) {
                                        Some(count) => format!("{count} ({pct})"),
                                        None => pct,
                                    };
                                    format!("{} scored {what}{}", r.label, if r.all_pass { ", all pass" } else { "" })
                                } else {
                                    format!("{} ungraded: {}", r.label, r.error)
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("; ")}
                    </p>
                }
            })}
    }
}

#[component]
fn HarnessViewer(tree: HarnessTree) -> impl IntoView {
    let b = budget(&tree.census);
    let tone = match b.level {
        BudgetLevel::Ok => Tone::Success,
        BudgetLevel::Near => Tone::Warning,
        BudgetLevel::Over => Tone::Danger,
    };
    let c = tree.census.clone();
    let files = tree.files.clone();
    let first = files.first().map(|f| f.path.clone()).unwrap_or_default();
    let selected = RwSignal::new(first);
    let groups: Vec<(h2s_api::Component, Vec<(String, u64)>)> = group_files(&files)
        .into_iter()
        .map(|(comp, fs)| {
            (
                comp,
                fs.into_iter().map(|f| (f.path.clone(), f.bytes)).collect(),
            )
        })
        .collect();
    let files_for_view = files;
    let budget_label = format!(
        "Always-on text: {} of {} characters ({})",
        format::int(u64::from(b.used)),
        format::int(u64::from(b.cap)),
        match b.level {
            BudgetLevel::Ok => "within budget",
            BudgetLevel::Near => "close to the cap",
            BudgetLevel::Over => "over the cap: harness2 rejects this tree",
        }
    );
    view! {
        <div class="budget">
            <div class="budget-head">
                <strong><Term id="always-on">"Always-on budget"</Term></strong>
                <span>{format!("{} / {} chars", format::int(u64::from(b.used)), format::int(u64::from(b.cap)))}</span>
            </div>
            <Meter fraction=b.fraction label=budget_label tone=tone />
            <div class="census">
                <span>{format!("{} on {}", format::plural(u64::from(c.n_files), "file", "files"), harness::surface_label(tree.surface))}</span>
                <span>{format!("{} skills", c.skills)}</span>
                <span>{format!("{} sub-agents", c.sub_agents)}</span>
                <span>{format!("{} scripts", c.scripts)}</span>
                <span>{format!("{} plugins", c.plugins)}</span>
                <span>{format!("{} hooks", c.hooks)}</span>
                <span class="mono">{format!("fingerprint {}", tree.fingerprint)}</span>
            </div>
        </div>
        <div class="harness-layout">
            <div class="component-groups" role="group" aria-label="Files by component">
                {groups
                    .into_iter()
                    .map(|(comp, fs)| {
                        let empty = fs.is_empty();
                        view! {
                            <div class="component-group" class:empty=empty>
                                <div class="component-head">
                                    <span class="component-code">{harness::code(comp)}</span>
                                    <span>{harness::name(comp)}</span>
                                    {harness::always_on(comp).then(|| view! { <span class="badge tone-info">"always on"</span> })}
                                </div>
                                {if empty {
                                    view! { <p class="small muted">"none"</p> }.into_any()
                                } else {
                                    view! {
                                        <ul class="file-list">
                                            {fs
                                                .into_iter()
                                                .map(|(path, bytes)| {
                                                    let p2 = path.clone();
                                                    let p3 = path.clone();
                                                    view! {
                                                        <li>
                                                            <button
                                                                type="button"
                                                                class="file-btn"
                                                                aria-pressed=move || if selected.get() == p2 { "true" } else { "false" }
                                                                on:click=move |_| selected.set(p3.clone())
                                                            >
                                                                <span>{path}</span>
                                                                <span class="muted">{format::bytes(bytes)}</span>
                                                            </button>
                                                        </li>
                                                    }
                                                })
                                                .collect_view()}
                                        </ul>
                                    }
                                    .into_any()
                                }}
                            </div>
                        }
                    })
                    .collect_view()}
            </div>
            <div class="viewer">
                {move || {
                    let path = selected.get();
                    match files_for_view.iter().find(|f| f.path == path) {
                        Some(f) => {
                            let text = f.content.clone().unwrap_or_default();
                            view! {
                                <div class="file-view-head">
                                    <code>{f.path.clone()}</code>
                                    <span class="muted small">
                                        {format!("{} · {} {}", format::bytes(f.bytes), harness::code(f.component), harness::name(f.component))}
                                    </span>
                                </div>
                                <p class="small muted">{harness::blurb(f.component)}</p>
                                <CodeBlock text=text copy=true label="File content" />
                            }
                            .into_any()
                        }
                        None => view! { <p class="muted">"Choose a file on the left."</p> }.into_any(),
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn ExportDialog(
    open: RwSignal<bool>,
    key: TaskKey,
    mode: Mode,
    label: Signal<String>,
) -> impl IntoView {
    let target = RwSignal::new("opencode".to_owned());
    let target_enum = move || match target.get().as_str() {
        "raw" => ExportTarget::Raw,
        "codex" => ExportTarget::Codex,
        _ => ExportTarget::Opencode,
    };
    let bundle = LocalResource::new(move || {
        let (o, l, tg) = (open.get(), label.get(), target_enum());
        let (e, s, t) = (key.exp.get(), key.sweep.get(), key.task.get());
        async move {
            if !o {
                return Ok(None);
            }
            api::get::<ExportBundle>(&paths::export(&e, &s, &t, mode, &l, tg, false))
                .await
                .map(Some)
        }
    });
    let preview = RwSignal::new(String::new());
    let close = Callback::new(move |()| open.set(false));
    let title = Signal::derive(move || format!("Export harness {}", label.get()));
    view! {
        <Modal open=open on_close=close title=title wide=true testid="export-dialog">
            <RadioCards
                name="export-target"
                legend="Target layout"
                value=target
                compact=true
                options=vec![
                    ("raw".to_owned(), "Raw".to_owned(), "The tree exactly as harness2 stores it.".to_owned()),
                    ("opencode".to_owned(), "OpenCode project".to_owned(), "AGENTS.md plus .opencode/ skills, agents and plugins.".to_owned()),
                    ("codex".to_owned(), "Codex project".to_owned(), "AGENTS.md, .agents/skills and .codex/hooks.json.".to_owned()),
                ]
            />
            {move || match bundle.get() {
                None | Some(Ok(None)) => view! { <Skeleton lines=4 /> }.into_any(),
                Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
                Some(Ok(Some(b))) => {
                    let files = b.files.clone();
                    let first = files.first().map(|f| f.path.clone()).unwrap_or_default();
                    if preview.get_untracked().is_empty() || !files.iter().any(|f| f.path == preview.get_untracked()) {
                        preview.set(first);
                    }
                    let json = serde_json::to_string_pretty(&b).unwrap_or_default();
                    let file_name = format!("{}-{}-{}.json", key.task.get_untracked(), b.label, target.get_untracked());
                    let tar = paths::export(&key.exp.get_untracked(), &key.sweep.get_untracked(), &key.task.get_untracked(), mode, &b.label, b.target, true);
                    let tar_name = format!("{}-{}-{}.tar.gz", key.task.get_untracked(), b.label, target.get_untracked());
                    let files_for_view = files.clone();
                    view! {
                        {(!b.notes.is_empty())
                            .then(|| view! { <Notice tone=Tone::Info><ul>{b.notes.iter().map(|n| view! { <li>{n.clone()}</li> }).collect_view()}</ul></Notice> })}
                        <div class="harness-layout">
                            <ul class="file-list" aria-label="Exported files">
                                {files
                                    .iter()
                                    .map(|f| {
                                        let (p2, p3) = (f.path.clone(), f.path.clone());
                                        view! {
                                            <li>
                                                <button
                                                    type="button"
                                                    class="file-btn"
                                                    aria-pressed=move || if preview.get() == p2 { "true" } else { "false" }
                                                    on:click=move |_| preview.set(p3.clone())
                                                >
                                                    <span>{f.path.clone()}</span>
                                                    <span class="muted">{format::bytes(f.content.len() as u64)}</span>
                                                </button>
                                            </li>
                                        }
                                    })
                                    .collect_view()}
                            </ul>
                            <div class="viewer">
                                {move || {
                                    let p = preview.get();
                                    files_for_view
                                        .iter()
                                        .find(|f| f.path == p)
                                        .map(|f| view! { <pre class="text-view" tabindex="0">{f.content.clone()}</pre> })
                                }}
                            </div>
                        </div>
                        <div class="modal-actions">
                            <button
                                type="button"
                                class="btn btn-secondary"
                                on:click=move |_| {
                                    browser::download_text(&file_name, "application/json", &json);
                                }
                            >
                                {icon(Icon::Download)}
                                " JSON"
                            </button>
                            <a class="btn btn-primary" href=tar download=tar_name rel="external">
                                {icon(Icon::Download)}
                                " Download .tar.gz"
                            </a>
                        </div>
                    }
                    .into_any()
                }
            }}
        </Modal>
    }
}

#[component]
fn HarnessPanel(key: TaskKey, record: ModeRecord) -> impl IntoView {
    let mode = record.mode;
    let labels = harness_labels(&record);
    let initial = labels
        .iter()
        .find(|(_, _, chosen)| *chosen)
        .or_else(|| labels.first())
        .map(|(l, _, _)| l.clone())
        .unwrap_or_default();
    let selected = RwSignal::new(initial);
    let tree = LocalResource::new(move || {
        let (e, s, t, l) = (
            key.exp.get(),
            key.sweep.get(),
            key.task.get(),
            selected.get(),
        );
        async move { api::get::<HarnessTree>(&paths::harness(&e, &s, &t, mode, &l)).await }
    });
    let export_open = RwSignal::new(false);
    view! {
        <Card title="Harnesses">
            <p class="muted small">
                "Every harness this mode produced, as files grouped by "<Term id="components">"component"</Term>
                ". The adopted one is marked with a dot."
            </p>
            <div class="section-head">
                <div class="label-picker" role="group" aria-label="Harness label">
                    {labels
                        .into_iter()
                        .map(|(label, role, chosen)| {
                            let (l2, l3) = (label.clone(), label.clone());
                            view! {
                                <button
                                    type="button"
                                    class="label-btn"
                                    data-testid="harness-label"
                                    data-label=label.clone()
                                    title=format!("{} ({})", label, harness::role_label(role))
                                    aria-pressed=move || if selected.get() == l2 { "true" } else { "false" }
                                    on:click=move |_| selected.set(l3.clone())
                                >
                                    {chosen.then(|| view! { <span class="chosen-dot" aria-label="adopted"></span> })}
                                    {label.clone()}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
                <button type="button" class="btn btn-secondary btn-sm" data-testid="export-button" on:click=move |_| export_open.set(true)>
                    {icon(Icon::Download)}
                    " Export…"
                </button>
            </div>
            {loaded(tree, 6, |t| {
                let role = t.role;
                view! {
                    <p class="small">
                        <Badge tone=role_tone(role) text=harness::role_label(role) />
                    </p>
                    <HarnessViewer tree=t />
                }
            })}
            <ExportDialog open=export_open key=key mode=mode label=Signal::derive(move || selected.get()) />
        </Card>
    }
}

#[component]
fn DiffPanel(key: TaskKey, record: ModeRecord) -> impl IntoView {
    let mode = record.mode;
    let labels: Vec<(String, String)> = harness_labels(&record)
        .into_iter()
        .map(|(l, role, chosen)| {
            let text = format!(
                "{l} ({}{})",
                harness::role_label(role),
                if chosen { ", adopted" } else { "" }
            );
            (l, text)
        })
        .collect();
    let (from0, to0) = default_diff_pair(&record).unwrap_or_default();
    let from = RwSignal::new(from0);
    let to = RwSignal::new(to0);
    let diff = LocalResource::new(move || {
        let (e, s, t, f, to) = (
            key.exp.get(),
            key.sweep.get(),
            key.task.get(),
            from.get(),
            to.get(),
        );
        async move {
            if f.is_empty() || to.is_empty() {
                return Ok(None);
            }
            api::get::<HarnessDiff>(&paths::diff(&e, &s, &t, mode, &f, &to))
                .await
                .map(Some)
        }
    });
    let opts = Signal::stored(labels);
    view! {
        <Card title="Compare two harnesses">
            <div class="diff-controls">
                <SelectField id=format!("diff-from-{}", mode.as_cli()) label="From" value=from options=opts testid="diff-from" />
                <SelectField id=format!("diff-to-{}", mode.as_cli()) label="To" value=to options=opts testid="diff-to" />
                <button
                    type="button"
                    class="btn btn-ghost btn-sm"
                    on:click=move |_| {
                        let (f, t) = (from.get_untracked(), to.get_untracked());
                        from.set(t);
                        to.set(f);
                    }
                >
                    "Swap"
                </button>
            </div>
            <div class="diff-view" data-testid="diff-view">
                {move || match diff.get() {
                    None => view! { <Skeleton lines=6 /> }.into_any(),
                    Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
                    Some(Ok(None)) => view! { <p class="muted">"Choose two harnesses."</p> }.into_any(),
                    Some(Ok(Some(d))) => {
                        let (n, added, removed) = totals(&d.files);
                        let unchanged: Vec<String> = d.files.iter().filter(|f| f.change == h2s_api::ChangeKind::Unchanged).map(|f| f.path.clone()).collect();
                        let delta = d.always_on_char_delta;
                        view! {
                            <div class="diff-summary">
                                <span>{format!("{} → {}:", d.from, d.to)}</span>
                                <span>{format::plural(u64::from(n), "file changed", "files changed")}</span>
                                <span class="add-count">{format!("+{added}")}</span>
                                <span class="del-count">{format!("\u{2212}{removed}")}</span>
                                <span>
                                    <Term id="always-on">"always-on"</Term>
                                    {format!(" {}{} chars", if delta >= 0 { "+" } else { "\u{2212}" }, format::int(delta.unsigned_abs()))}
                                </span>
                            </div>
                            {if n == 0 {
                                view! { <p class="muted">"The two harnesses are identical."</p> }.into_any()
                            } else {
                                d.files
                                    .iter()
                                    .filter(|f| f.change != h2s_api::ChangeKind::Unchanged)
                                    .map(|f| {
                                        let lines = file_lines(f);
                                        let tone = match f.change {
                                            h2s_api::ChangeKind::Added => Tone::Success,
                                            h2s_api::ChangeKind::Removed => Tone::Danger,
                                            _ => Tone::Info,
                                        };
                                        view! {
                                            <section class="diff-file" aria-label=format!("Changes to {}", f.path)>
                                                <div class="diff-file-head">
                                                    <code>{f.path.clone()}</code>
                                                    <Badge tone=tone text=change_label(f.change) />
                                                    <span class="muted small">{format!("{} {}", harness::code(f.component), harness::name(f.component))}</span>
                                                    <span class="add-count small">{format!("+{}", f.lines_added)}</span>
                                                    <span class="del-count small">{format!("\u{2212}{}", f.lines_removed)}</span>
                                                </div>
                                                <div class="diff-body">
                                                    <table class="diff-table">
                                                        <tbody>
                                                            {lines
                                                                .into_iter()
                                                                .map(|l| {
                                                                    let sign = match l.kind {
                                                                        LineKind::Added => "+",
                                                                        LineKind::Removed => "\u{2212}",
                                                                        _ => "",
                                                                    };
                                                                    let sr = l.kind.sr_label();
                                                                    view! {
                                                                        <tr class=l.kind.class()>
                                                                            <td class="ln">{l.old_no.map(|n| n.to_string()).unwrap_or_default()}</td>
                                                                            <td class="ln">{l.new_no.map(|n| n.to_string()).unwrap_or_default()}</td>
                                                                            <td class="sign">
                                                                                <span aria-hidden="true">{sign}</span>
                                                                                {(!sr.is_empty()).then(|| view! { <span class="visually-hidden">{sr}</span> })}
                                                                            </td>
                                                                            <td class="code">{l.text}</td>
                                                                        </tr>
                                                                    }
                                                                })
                                                                .collect_view()}
                                                        </tbody>
                                                    </table>
                                                </div>
                                            </section>
                                        }
                                    })
                                    .collect_view()
                                    .into_any()
                            }}
                            {(!unchanged.is_empty())
                                .then(|| view! { <p class="small muted">{format!("Unchanged: {}", unchanged.join(", "))}</p> })}
                        }
                        .into_any()
                    }
                }}
            </div>
        </Card>
    }
}

#[component]
fn FileViewer(content: FileContent) -> impl IntoView {
    let is_traj = trajectory::is_trajectory_path(&content.path)
        || content
            .text
            .as_deref()
            .is_some_and(trajectory::looks_like_trajectory);
    view! {
        <div class="file-view-head">
            <code>{content.path.clone()}</code>
            <span class="muted small">{format!("{} · {}", format::bytes(content.bytes), content.mime)}</span>
        </div>
        {content
            .truncated
            .then(|| {
                view! {
                    <Notice tone=Tone::Warning>
                        <p>{format!("Showing the first part only: the file is {}, and the server reads at most 2 MiB.", format::bytes(content.bytes))}</p>
                    </Notice>
                }
            })}
        {match content.text.clone() {
            None => view! {
                <Notice tone=Tone::Neutral title="Binary file">
                    <p>{format!("{} ({}) is not text, so it is not shown here.", content.path, content.mime)}</p>
                </Notice>
            }
            .into_any(),
            Some(text) if is_traj => {
                let blocks = trajectory::segment(&text);
                let (a, o, e) = trajectory::counts(&blocks);
                view! {
                    <div class="traj-legend">
                        <span><i class="swatch score"></i>{format!("{a} actions")}</span>
                        <span><i class="swatch pos"></i>{format!("{o} observations")}</span>
                        <span><i class="swatch neg"></i>{format!("{e} errors")}</span>
                    </div>
                    <div class="traj" tabindex="0" role="log" aria-label="Trajectory">
                        {blocks
                            .into_iter()
                            .map(|b| {
                                let label = b.kind.label();
                                view! {
                                    <div class=format!("traj-block {}", b.kind.class())>
                                        {(b.kind != TrajKind::Plain).then(|| view! { <span class="traj-kind">{label}</span> })}
                                        {b.text}
                                    </div>
                                }
                            })
                            .collect_view()}
                    </div>
                }
                .into_any()
            }
            Some(text) => view! { <pre tabindex="0">{text}</pre> }.into_any(),
        }}
    }
}

#[component]
fn RolloutBrowser(key: TaskKey, detail: TaskDetail, record: ModeRecord) -> impl IntoView {
    let mode = record.mode;
    let labels = rollout_labels(&detail, &record);
    if labels.is_empty() {
        return view! {
            <Card title="Rollouts">
                <p class="muted">"No rollouts are available for this mode."</p>
            </Card>
        }
        .into_any();
    }
    let chosen = record.selection.as_ref().map(|s| s.chosen_label.clone());
    let initial = chosen
        .filter(|c| labels.contains(c))
        .unwrap_or_else(|| labels[0].clone());
    let label = RwSignal::new(initial);
    let dir = RwSignal::new(String::new());
    let file = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        label.track();
        dir.set(String::new());
        file.set(None);
    });
    let listing = LocalResource::new(move || {
        let (e, s, t, l, d) = (
            key.exp.get(),
            key.sweep.get(),
            key.task.get(),
            label.get(),
            dir.get(),
        );
        async move { api::get::<Vec<FileEntry>>(&paths::rollout_files(&e, &s, &t, mode, &l, &d)).await }
    });
    let content = LocalResource::new(move || {
        let (e, s, t, l, f) = (
            key.exp.get(),
            key.sweep.get(),
            key.task.get(),
            label.get(),
            file.get(),
        );
        async move {
            match f {
                Some(p) => api::get::<FileContent>(&paths::rollout_file(&e, &s, &t, mode, &l, &p))
                    .await
                    .map(Some),
                None => Ok(None),
            }
        }
    });
    let options = Signal::stored(
        labels
            .into_iter()
            .map(|l| (l.clone(), l))
            .collect::<Vec<_>>(),
    );
    view! {
        <Card title="Rollouts">
            <p class="muted small">
                "Each "<Term id="rollout">"rollout"</Term>" is one execution of the task: browse its files, the deliverable and the "
                <Term id="trajectory">"trajectory"</Term>"."
            </p>
            <div class="toolbar">
                <SelectField id=format!("rollout-label-{}", mode.as_cli()) label="Rollout" value=label options=options />
            </div>
            <div class="browser">
                <div>
                    <nav class="tree-path" aria-label="Folder">
                        <button type="button" class="link-btn" on:click=move |_| { dir.set(String::new()); }>
                            {move || label.get()}
                        </button>
                        {move || {
                            let d = dir.get();
                            let mut acc = String::new();
                            d.split('/')
                                .filter(|s| !s.is_empty())
                                .map(|seg| {
                                    if !acc.is_empty() {
                                        acc.push('/');
                                    }
                                    acc.push_str(seg);
                                    let target = acc.clone();
                                    view! {
                                        <span aria-hidden="true">"/"</span>
                                        <button type="button" class="link-btn" on:click=move |_| dir.set(target.clone())>
                                            {seg.to_owned()}
                                        </button>
                                    }
                                })
                                .collect_view()
                        }}
                    </nav>
                    {move || match listing.get() {
                        None => view! { <Skeleton lines=4 /> }.into_any(),
                        Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
                        Some(Ok(entries)) if entries.is_empty() => view! { <p class="muted">"Empty folder."</p> }.into_any(),
                        Some(Ok(entries)) => view! {
                            <ul class="tree" aria-label="Files">
                                {entries
                                    .into_iter()
                                    .map(|en| {
                                        let path = en.path.clone();
                                        let current = path.clone();
                                        match en.kind {
                                            FileKind::Dir => view! {
                                                <li>
                                                    <button type="button" class="file-btn" on:click=move |_| dir.set(path.clone())>
                                                        <span>{icon(Icon::Folder)}" "{en.name}</span>
                                                        <span class="muted">"folder"</span>
                                                    </button>
                                                </li>
                                            }
                                            .into_any(),
                                            FileKind::File => view! {
                                                <li>
                                                    <button
                                                        type="button"
                                                        class="file-btn"
                                                        aria-pressed=move || if file.get().as_deref() == Some(current.as_str()) { "true" } else { "false" }
                                                        on:click=move |_| file.set(Some(path.clone()))
                                                    >
                                                        <span>{icon(Icon::File)}" "{en.name.clone()}</span>
                                                        <span class="muted">{format::bytes(en.bytes)}</span>
                                                    </button>
                                                </li>
                                            }
                                            .into_any(),
                                            FileKind::Symlink | FileKind::Other => view! {
                                                <li class="file-btn" title="Symbolic links are listed but never followed.">
                                                    <span>{en.name.clone()}</span>
                                                    <span class="muted">{if en.kind == FileKind::Symlink { "link (not followed)" } else { "special file" }}</span>
                                                </li>
                                            }
                                            .into_any(),
                                        }
                                    })
                                    .collect_view()}
                            </ul>
                        }
                        .into_any(),
                    }}
                </div>
                <div class="viewer">
                    {move || match content.get() {
                        None => view! { <Skeleton lines=6 /> }.into_any(),
                        Some(Err(e)) => view! { <ErrorPanel error=e /> }.into_any(),
                        Some(Ok(None)) => view! { <p class="muted">"Choose a file to read it here. Trajectories are highlighted by step."</p> }.into_any(),
                        Some(Ok(Some(c))) => view! { <FileViewer content=c /> }.into_any(),
                    }}
                </div>
            </div>
        </Card>
    }
    .into_any()
}

#[component]
fn ModeSection(key: TaskKey, detail: TaskDetail, record: ModeRecord) -> impl IntoView {
    let mode = record.mode;
    let chosen = record
        .selection
        .as_ref()
        .map(|s| (s.chosen_label.clone(), s.chosen_is_base));
    let errors = record.errors.clone();
    view! {
        <section class="mode-section card" data-testid="mode-section" data-mode=mode.as_cli() aria-labelledby=format!("mode-{}", mode.as_cli())>
            <div class="mode-head">
                <h2 id=format!("mode-{}", mode.as_cli())>
                    {match mode {
                        Mode::Parallel => view! { <Term id="parallel">"Parallel mode"</Term> }.into_any(),
                        Mode::Sequential => view! { <Term id="sequential">"Sequential mode"</Term> }.into_any(),
                    }}
                </h2>
                {if record.ok {
                    view! { <Badge tone=Tone::Success text="finished" /> }.into_any()
                } else {
                    view! { <Badge tone=Tone::Danger text="did not finish" /> }.into_any()
                }}
                {chosen.map(|(l, base)| {
                    let text = if base { format!("kept base ({l})") } else { format!("adopted {l}") };
                    view! { <Badge tone=Tone::Info text=text /> }
                })}
            </div>
            {(!errors.is_empty())
                .then(|| {
                    view! {
                        <Notice tone=Tone::Danger title="Errors in this mode">
                            <ul>{errors.into_iter().map(|e| view! { <li>{e}</li> }).collect_view()}</ul>
                        </Notice>
                    }
                })}
            <Timeline detail=detail.clone() record=record.clone() />
            {if record.harnesses.is_empty() {
                view! { <p class="muted">"No harnesses are on disk for this mode."</p> }.into_any()
            } else {
                view! {
                    <HarnessPanel key=key record=record.clone() />
                    <DiffPanel key=key record=record.clone() />
                }
                .into_any()
            }}
            <RolloutBrowser key=key detail=detail record=record />
        </section>
    }
}

#[component]
pub(crate) fn TaskPage() -> impl IntoView {
    let key = TaskKey {
        exp: param("exp"),
        sweep: param("sweep"),
        task: param("task"),
    };
    let detail = LocalResource::new(move || {
        let (e, s, t) = (key.exp.get(), key.sweep.get(), key.task.get());
        async move { api::get::<TaskDetail>(&paths::task(&e, &s, &t)).await }
    });
    let show = RwSignal::new("both".to_owned());
    let title = Signal::derive(move || {
        detail
            .get()
            .and_then(Result::ok)
            .map_or_else(|| key.task.get(), |d| d.task_id)
    });
    let subtitle = Signal::derive(move || {
        detail
            .get()
            .and_then(Result::ok)
            .map(|d| {
                format!(
                    "{} · status {} · k={}{}",
                    d.domain,
                    d.status,
                    d.k,
                    d.seed.map(|s| format!(" · seed {s}")).unwrap_or_default()
                )
            })
            .unwrap_or_default()
    });
    let crumbs = vec![
        Crumb("Experiments".into(), "/experiments".into()),
        Crumb(
            key.exp.get_untracked(),
            route::experiment(&key.exp.get_untracked()),
        ),
        Crumb(
            key.sweep.get_untracked(),
            route::sweep(&key.exp.get_untracked(), &key.sweep.get_untracked()),
        ),
    ];
    view! {
        <div class="page">
            <PageHeader title=title subtitle=subtitle crumbs=crumbs />
            {move || match detail.get() {
                None => view! { <Skeleton lines=8 /> }.into_any(),
                Some(Err(e)) if e.is_not_found() => view! {
                    <EmptyState title="Task not found" body="This sweep has no task with that id.">
                        <a class="btn btn-primary" href=route::sweep(&key.exp.get(), &key.sweep.get())>"Back to the sweep"</a>
                    </EmptyState>
                }
                .into_any(),
                Some(Err(e)) => {
                    let retry = Callback::new(move |()| detail.refetch());
                    view! { <ErrorPanel error=e retry=retry /> }.into_any()
                }
                Some(Ok(d)) => {
                    let modes: Vec<Mode> = d.modes.iter().map(|m| m.mode).collect();
                    let several = modes.len() > 1;
                    let task_errors = d.errors.clone();
                    let prompt = d.task_prompt.clone();
                    let records = d.modes.clone();
                    view! {
                        {prompt
                            .map(|p| {
                                view! {
                                    <details class="disclosure">
                                        <summary>"Task statement, as the agent saw it"</summary>
                                        <pre class="text-view">{p}</pre>
                                    </details>
                                }
                            })}
                        {(!task_errors.is_empty())
                            .then(|| {
                                view! {
                                    <Notice tone=Tone::Danger title="Task errors">
                                        <ul>{task_errors.into_iter().map(|e| view! { <li>{e}</li> }).collect_view()}</ul>
                                    </Notice>
                                }
                            })}
                        {several
                            .then(|| {
                                view! {
                                    <div class="toolbar">
                                        <SelectField
                                            id="task-mode-filter"
                                            label="Show"
                                            value=show
                                            options=Signal::stored(vec![
                                                ("both".to_owned(), "Both modes".to_owned()),
                                                ("parallel".to_owned(), "Parallel only".to_owned()),
                                                ("sequential".to_owned(), "Sequential only".to_owned()),
                                            ])
                                        />
                                    </div>
                                }
                            })}
                        {records
                            .into_iter()
                            .map(|m| {
                                let detail = d.clone();
                                let name = m.mode.as_cli();
                                view! {
                                    <div class:hidden-mode=move || { let s = show.get(); s != "both" && s != name }>
                                        <ModeSection key=key detail=detail record=m />
                                    </div>
                                }
                            })
                            .collect_view()}
                        {modes
                            .is_empty()
                            .then(|| view! { <EmptyState title="No mode records" body="The task has not produced a mode record yet. It may still be running." /> })}
                    }
                    .into_any()
                }
            }}
        </div>
    }
}
