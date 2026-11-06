//! Every page's *Getting started* and *FAQ*: a collapsible panel under the page title with
//! two tabs. The content lives in `logic::page_help`; it is plain text (inline `code` spans
//! only), rendered as text nodes. Whether the panel is open is remembered per page in this
//! browser (`localStorage`, best effort).

use leptos::prelude::*;

use super::icons::{Icon, icon};
use super::ui::{CodeBlock, uid};
use crate::browser;
use crate::logic::page_help::{self, Faq, Link, Run, Step};

/// Plain help text as paragraphs, with `code` spans.
pub(crate) fn rich_text(text: &'static str, class: &'static str) -> impl IntoView {
    page_help::paragraphs(text)
        .into_iter()
        .map(|runs| {
            let parts = runs
                .into_iter()
                .map(|r| match r {
                    Run::Text(t) => t.to_owned().into_any(),
                    Run::Code(c) => view! { <code>{c.to_owned()}</code> }.into_any(),
                })
                .collect_view();
            view! { <p class=class>{parts}</p> }
        })
        .collect_view()
}

fn extras(cmd: Option<&'static str>, link: Option<Link>) -> impl IntoView {
    view! {
        {cmd.map(|c| view! { <CodeBlock text=Signal::stored(c.to_owned()) copy=true label="Command" /> })}
        {link.map(|l| view! { <a class="btn btn-secondary btn-sm help-link" href=l.href>{l.label}</a> })}
    }
}

fn step_view(s: &'static Step) -> impl IntoView {
    view! {
        <li class="help-step">
            <h3 class="help-step-title">{s.title}</h3>
            <div class="help-step-body">{rich_text(s.body, "")} {extras(s.cmd, s.link)}</div>
        </li>
    }
}

fn faq_view(f: &'static Faq, open: bool) -> impl IntoView {
    view! {
        <details class="faq" open=open>
            <summary>{rich_text(f.q, "faq-q")}</summary>
            <div class="faq-a">{rich_text(f.a, "")} {extras(f.cmd, f.link)}</div>
        </details>
    }
}

fn storage_key(key: &str) -> String {
    format!("h2w.help.{key}")
}

/// The help panel of the page `key` (a `logic::page_help` key). Renders nothing for an
/// unknown key.
#[component]
pub(crate) fn PageHelp(key: &'static str) -> impl IntoView {
    let Some(help) = page_help::find(key) else {
        return ().into_any();
    };
    let open = RwSignal::new(browser::storage_get(&storage_key(key)).as_deref() == Some("open"));
    let tab = RwSignal::new("start");
    let query = RwSignal::new(String::new());
    let base = uid("page-help");
    let body_id = format!("{base}-body");
    let start_tab = format!("{base}-tab-start");
    let faq_tab = format!("{base}-tab-faq");
    let start_panel = format!("{base}-panel-start");
    let faq_panel = format!("{base}-panel-faq");
    let toggle = move |_| {
        let now = !open.get_untracked();
        open.set(now);
        browser::storage_set(&storage_key(key), if now { "open" } else { "closed" });
    };
    // Arrow keys move between the two tabs (WAI-ARIA tabs pattern, automatic activation).
    let tab_keys = {
        let (s, f) = (start_tab.clone(), faq_tab.clone());
        move |e: web_sys::KeyboardEvent| {
            let next = match e.key().as_str() {
                "ArrowRight" | "ArrowLeft" => {
                    if tab.get_untracked() == "start" {
                        "faq"
                    } else {
                        "start"
                    }
                }
                "Home" => "start",
                "End" => "faq",
                _ => return,
            };
            e.prevent_default();
            tab.set(next);
            browser::focus_id(if next == "start" { &s } else { &f });
        }
    };
    let tab_keys2 = tab_keys.clone();
    let matches = Memo::new(move |_| query.with(|q| page_help::filter_faq(help.faq, q).len()));
    view! {
        <section class="page-help card" aria-label=format!("Help for {}", help.name) data-testid="page-help">
            <button
                type="button"
                class="page-help-toggle"
                aria-expanded=move || if open.get() { "true" } else { "false" }
                aria-controls=body_id.clone()
                data-testid="page-help-toggle"
                on:click=toggle
            >
                {icon(Icon::Help)}
                <span>"Getting started and FAQ"</span>
            </button>
            <div class="page-help-body" id=body_id hidden=move || !open.get()>
                <div class="tabs page-help-tabs" role="tablist" aria-label=format!("Help for {}", help.name)>
                    <button
                        type="button"
                        role="tab"
                        class="tab"
                        id=start_tab.clone()
                        aria-selected=move || if tab.get() == "start" { "true" } else { "false" }
                        aria-controls=start_panel.clone()
                        tabindex=move || if tab.get() == "start" { "0" } else { "-1" }
                        data-testid="help-tab-start"
                        on:click=move |_| tab.set("start")
                        on:keydown=tab_keys
                    >
                        "Getting started"
                    </button>
                    <button
                        type="button"
                        role="tab"
                        class="tab"
                        id=faq_tab.clone()
                        aria-selected=move || if tab.get() == "faq" { "true" } else { "false" }
                        aria-controls=faq_panel.clone()
                        tabindex=move || if tab.get() == "faq" { "0" } else { "-1" }
                        data-testid="help-tab-faq"
                        on:click=move |_| tab.set("faq")
                        on:keydown=tab_keys2
                    >
                        {format!("FAQ ({})", help.faq.len())}
                    </button>
                </div>
                <div
                    role="tabpanel"
                    class="page-help-panel"
                    id=start_panel
                    aria-labelledby=start_tab
                    tabindex="0"
                    hidden=move || tab.get() != "start"
                >
                    {rich_text(help.lead, "help-lead")}
                    <ol class="help-steps">{help.steps.iter().map(step_view).collect_view()}</ol>
                </div>
                <div
                    role="tabpanel"
                    class="page-help-panel"
                    id=faq_panel
                    aria-labelledby=faq_tab
                    tabindex="0"
                    hidden=move || tab.get() != "faq"
                >
                    <div class="field">
                        <label for=format!("{base}-filter")>"Search these questions"</label>
                        <input
                            id=format!("{base}-filter")
                            class="input"
                            type="search"
                            autocomplete="off"
                            bind:value=query
                        />
                    </div>
                    <p class="muted small" role="status" aria-live="polite">
                        {move || {
                            let q = query.get();
                            if q.trim().is_empty() {
                                String::new()
                            } else {
                                match matches.get() {
                                    1 => "1 question matches.".to_owned(),
                                    n => format!("{n} questions match."),
                                }
                            }
                        }}
                    </p>
                    {move || {
                        let q = query.get();
                        let hits = page_help::filter_faq(help.faq, &q);
                        if hits.is_empty() {
                            return view! { <p class="muted">"No question on this page matches. Try the Help page."</p> }
                                .into_any();
                        }
                        let expand = !q.trim().is_empty();
                        hits.into_iter().map(|f| faq_view(f, expand)).collect_view().into_any()
                    }}
                    <p class="small">
                        <a href="/help#faq">"More questions and the glossary on the Help page"</a>
                    </p>
                </div>
            </div>
        </section>
    }
    .into_any()
}
