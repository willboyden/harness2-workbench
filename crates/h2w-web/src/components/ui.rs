//! Small presentational building blocks shared by every page.

use std::cell::Cell;

use h2w_api::JobStatus;
use leptos::prelude::*;

use crate::api::ApiError;
use crate::browser;
use crate::logic::glossary;
use crate::logic::jobs::{self, Tone};
use crate::logic::urls::route;
use crate::state::{toasts, ui};

thread_local! {
    static NEXT_ID: Cell<u32> = const { Cell::new(1) };
}

/// A document-unique element id with a readable prefix (`term-7`, `dlg-3`).
pub(crate) fn uid(prefix: &str) -> String {
    let n = NEXT_ID.with(|c| {
        let n = c.get();
        c.set(n.wrapping_add(1));
        n
    });
    format!("{prefix}-{n}")
}

/// A breadcrumb: label and in-app link.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Crumb(pub(crate) String, pub(crate) String);

/// The page title row; also sets the document title.
#[component]
pub(crate) fn PageHeader(
    #[prop(into)] title: Signal<String>,
    #[prop(optional, into)] subtitle: MaybeProp<String>,
    #[prop(optional)] crumbs: Vec<Crumb>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    Effect::new(move |_| browser::set_title(&title.get()));
    let has_crumbs = !crumbs.is_empty();
    view! {
        <header class="page-header">
            {has_crumbs
                .then(|| {
                    view! {
                        <nav class="crumbs" aria-label="Breadcrumb">
                            <ol>
                                {crumbs
                                    .into_iter()
                                    .map(|Crumb(label, href)| {
                                        view! {
                                            <li>
                                                <a href=href>{label}</a>
                                            </li>
                                        }
                                    })
                                    .collect_view()}
                            </ol>
                        </nav>
                    }
                })}
            <div class="page-header-row">
                <div class="page-titles">
                    <h1 id="page-title">{move || title.get()}</h1>
                    {move || subtitle.get().map(|s| view! { <p class="page-subtitle">{s}</p> })}
                </div>
                {children.map(|c| view! { <div class="page-actions">{c()}</div> })}
            </div>
        </header>
    }
}

#[component]
pub(crate) fn Badge(tone: Tone, #[prop(into)] text: String) -> impl IntoView {
    view! { <span class=format!("badge {}", tone.class())>{text}</span> }
}

/// A job status badge whose text is exactly the status string (`succeeded`, ...).
#[component]
pub(crate) fn StatusBadge(
    #[prop(into)] status: Signal<JobStatus>,
    #[prop(optional)] testid: Option<&'static str>,
) -> impl IntoView {
    view! {
        <span
            class=move || format!("badge status {}", jobs::status_tone(status.get()).class())
            title=move || jobs::status_help(status.get())
            data-testid=testid
        >
            {move || status.get().as_str()}
        </span>
    }
}

/// A titled panel.
#[component]
pub(crate) fn Card(
    #[prop(optional, into)] title: MaybeProp<String>,
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] testid: Option<&'static str>,
    children: Children,
) -> impl IntoView {
    let class = format!("card {}", class.unwrap_or_default());
    view! {
        <section class=class data-testid=testid>
            {move || title.get().map(|t| view! { <h2 class="card-title">{t}</h2> })}
            {children()}
        </section>
    }
}

/// What to do when a list is empty: say why and offer the next step.
#[component]
pub(crate) fn EmptyState(
    #[prop(into)] title: String,
    #[prop(into)] body: String,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="empty-state" role="status">
            <svg class="empty-icon" viewBox="0 0 48 48" aria-hidden="true">
                <rect x="8" y="10" width="32" height="28" rx="4" fill="none" stroke="currentColor" stroke-width="2"></rect>
                <path d="M8 18h32" stroke="currentColor" stroke-width="2"></path>
                <circle cx="24" cy="29" r="3" fill="currentColor"></circle>
            </svg>
            <h2>{title}</h2>
            <p>{body}</p>
            {children.map(|c| view! { <div class="empty-actions">{c()}</div> })}
        </div>
    }
}

/// Placeholder bars while data loads.
#[component]
pub(crate) fn Skeleton(#[prop(default = 3)] lines: usize) -> impl IntoView {
    view! {
        <div class="skeleton" aria-busy="true" aria-live="polite">
            <span class="visually-hidden">"Loading…"</span>
            {(0..lines)
                .map(|i| {
                    view! {
                        <div class=if i == 0 { "skeleton-line wide" } else { "skeleton-line" }></div>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// An API error with an optional retry.
#[component]
pub(crate) fn ErrorPanel(
    error: ApiError,
    #[prop(optional)] retry: Option<Callback<()>>,
) -> impl IntoView {
    let headline = error.headline();
    let details = error.details.clone();
    let request_id = error.request_id.clone();
    view! {
        <div class="notice tone-danger" role="alert">
            <p class="notice-title">{headline}</p>
            <p>{error.message.clone()}</p>
            {(!details.is_empty())
                .then(|| {
                    view! {
                        <ul class="problem-list">
                            {details
                                .into_iter()
                                .map(|d| view! { <li><code>{d.field}</code>": "{d.problem}</li> })
                                .collect_view()}
                        </ul>
                    }
                })}
            {request_id.map(|r| view! { <p class="muted small">"Request id: "<code>{r}</code></p> })}
            {retry
                .map(|r| {
                    view! {
                        <button type="button" class="btn btn-secondary btn-sm" on:click=move |_| r.run(())>
                            "Try again"
                        </button>
                    }
                })}
        </div>
    }
}

/// An informational, warning or danger box.
#[component]
pub(crate) fn Notice(
    tone: Tone,
    #[prop(optional, into)] title: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    let role = if tone == Tone::Danger {
        "alert"
    } else {
        "note"
    };
    view! {
        <div class=format!("notice {}", tone.class()) role=role>
            {move || title.get().map(|t| view! { <p class="notice-title">{t}</p> })}
            {children()}
        </div>
    }
}

/// A horizontal bar for a 0..1 value, drawn in SVG (no inline styles under the CSP).
#[component]
pub(crate) fn Meter(
    #[prop(into)] fraction: Signal<f64>,
    #[prop(into)] label: Signal<String>,
    #[prop(optional, into)] tone: Option<Signal<Tone>>,
) -> impl IntoView {
    let width = move || format!("{:.2}", (fraction.get().clamp(0.0, 1.0) * 100.0));
    let class = move || {
        format!(
            "meter-fill {}",
            tone.map_or(Tone::Info, |t| t.get()).class()
        )
    };
    view! {
        <svg class="meter" viewBox="0 0 100 8" preserveAspectRatio="none" role="img" aria-label=move || label.get()>
            <rect class="meter-track" x="0" y="0" width="100" height="8" rx="4"></rect>
            <rect class=class x="0" y="0" width=width height="8" rx="4"></rect>
        </svg>
    }
}

/// A progress bar; indeterminate (animated) when the fraction is unknown.
#[component]
pub(crate) fn ProgressBar(
    #[prop(into)] fraction: Signal<Option<f64>>,
    #[prop(into)] label: Signal<String>,
) -> impl IntoView {
    let pct = move || fraction.get().map(|f| (f.clamp(0.0, 1.0) * 100.0).round());
    view! {
        <div
            class=move || if fraction.get().is_some() { "progress" } else { "progress indeterminate" }
            role="progressbar"
            aria-valuemin="0"
            aria-valuemax="100"
            aria-valuenow=move || pct().map(|p| format!("{p:.0}"))
            aria-valuetext=move || label.get()
            aria-label="Progress"
        >
            <svg viewBox="0 0 100 6" preserveAspectRatio="none" aria-hidden="true">
                <rect class="progress-track" x="0" y="0" width="100" height="6" rx="3"></rect>
                <rect
                    class="progress-fill"
                    x="0"
                    y="0"
                    width=move || pct().map_or_else(|| "30".to_owned(), |p| format!("{p:.0}"))
                    height="6"
                    rx="3"
                ></rect>
            </svg>
        </div>
    }
}

/// Copy text to the clipboard with feedback.
#[component]
pub(crate) fn CopyButton(
    #[prop(into)] text: Signal<String>,
    #[prop(default = "Copy")] label: &'static str,
    #[prop(optional)] testid: Option<&'static str>,
) -> impl IntoView {
    let t = toasts();
    let copied = RwSignal::new(false);
    view! {
        <button
            type="button"
            class="btn btn-secondary btn-sm"
            data-testid=testid
            on:click=move |_| {
                browser::copy_text(
                    &text.get_untracked(),
                    move |ok| {
                        if ok {
                            copied.set(true);
                            set_timeout(move || copied.set(false), std::time::Duration::from_secs(2));
                        } else {
                            t.warn(
                                "Copy failed",
                                "The browser refused clipboard access. Select the text and copy it by hand.",
                            );
                        }
                    },
                );
            }
        >
            {move || if copied.get() { "Copied" } else { label }}
        </button>
    }
}

/// Preformatted text with an optional copy button.
#[component]
pub(crate) fn CodeBlock(
    #[prop(into)] text: Signal<String>,
    #[prop(optional)] copy: bool,
    #[prop(optional)] label: Option<&'static str>,
) -> impl IntoView {
    view! {
        <div class="code-block">
            {copy.then(|| view! { <div class="code-actions"><CopyButton text=text /></div> })}
            <pre tabindex="0" aria-label=label>
                <code>{move || text.get()}</code>
            </pre>
        </div>
    }
}

/// A glossary term: a link to its Help entry with the short definition as a tooltip.
#[component]
pub(crate) fn Term(
    id: &'static str,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let term = glossary::find(id);
    let tip_id = uid("term-tip");
    let short = term.map(|t| t.short).unwrap_or_default();
    let fallback = term.map(|t| t.term).unwrap_or(id);
    let label = children.map_or_else(|| fallback.into_any(), |c| c());
    view! {
        <span class="term">
            <a class="term-link" href=route::help_term(id) aria-describedby=tip_id.clone()>
                {label}
            </a>
            <span class="term-tip" role="tooltip" id=tip_id>
                {short}
            </span>
        </span>
    }
}

/// A list pager over cursor-paged results: keeps a stack of cursors for "Previous".
#[derive(Clone, Copy)]
pub(crate) struct Cursor {
    stack: RwSignal<Vec<String>>,
    pub(crate) current: RwSignal<Option<String>>,
}

impl Cursor {
    pub(crate) fn new() -> Cursor {
        Cursor {
            stack: RwSignal::new(Vec::new()),
            current: RwSignal::new(None),
        }
    }

    pub(crate) fn reset(&self) {
        self.stack.set(Vec::new());
        self.current.set(None);
    }

    pub(crate) fn next(&self, cursor: String) {
        let cur = self.current.get_untracked().unwrap_or_default();
        self.stack.update(|s| s.push(cur));
        self.current.set(Some(cursor));
    }

    pub(crate) fn prev(&self) {
        let mut popped = None;
        self.stack.update(|s| popped = s.pop());
        self.current.set(popped.filter(|c| !c.is_empty()));
    }

    pub(crate) fn has_prev(&self) -> bool {
        self.stack.with(|s| !s.is_empty())
    }

    pub(crate) fn page(&self) -> usize {
        self.stack.with(Vec::len) + 1
    }
}

#[component]
pub(crate) fn Pager(cursor: Cursor, #[prop(into)] next: Signal<Option<String>>) -> impl IntoView {
    view! {
        <nav class="pager" aria-label="Pages">
            <button
                type="button"
                class="btn btn-secondary btn-sm"
                disabled=move || !cursor.has_prev()
                on:click=move |_| cursor.prev()
            >
                "Previous"
            </button>
            <span class="muted small">{move || format!("Page {}", cursor.page())}</span>
            <button
                type="button"
                class="btn btn-secondary btn-sm"
                disabled=move || next.get().is_none()
                on:click=move |_| {
                    if let Some(c) = next.get_untracked() {
                        cursor.next(c);
                    }
                }
            >
                "Next"
            </button>
        </nav>
    }
}

/// Render a loading resource: skeleton while pending, error panel with retry on failure.
pub(crate) fn loaded<T, V>(
    res: LocalResource<Result<T, ApiError>>,
    lines: usize,
    view_fn: impl Fn(T) -> V + Send + Sync + 'static,
) -> impl IntoView
where
    T: Clone + Send + Sync + 'static,
    V: IntoView + 'static,
{
    move || match res.get() {
        None => view! { <Skeleton lines=lines /> }.into_any(),
        Some(Ok(v)) => view_fn(v).into_any(),
        Some(Err(e)) => {
            let retry = Callback::new(move |()| res.refetch());
            view! { <ErrorPanel error=e retry=retry /> }.into_any()
        }
    }
}

/// A stat tile: label, big value, optional hint.
#[component]
pub(crate) fn Stat(
    #[prop(into)] label: String,
    #[prop(into)] value: Signal<String>,
    #[prop(optional, into)] hint: MaybeProp<String>,
) -> impl IntoView {
    view! {
        <div class="stat">
            <span class="stat-label">{label}</span>
            <span class="stat-value">{move || value.get()}</span>
            {move || hint.get().map(|h| view! { <span class="stat-hint">{h}</span> })}
        </div>
    }
}

/// Relative time that stays current (`5 min ago`), with the exact time as a tooltip.
#[component]
pub(crate) fn Ago(#[prop(into)] ts: String) -> impl IntoView {
    let u = ui();
    let exact = crate::logic::format::timestamp(&ts);
    let ts2 = ts.clone();
    view! {
        <time datetime=ts title=exact>
            {move || crate::logic::format::relative_ts(u.now.get(), &ts2)}
        </time>
    }
}
