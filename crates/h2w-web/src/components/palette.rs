//! The command palette (Ctrl/Cmd+K), the keyboard-shortcuts dialog and the global key
//! handler (`?`, `g` chords, `/`).

use h2w_api::{Experiment, Job, Page, Role};
use leptos::ev;
use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;

use super::modal::Modal;
use crate::actions;
use crate::api;
use crate::browser;
use crate::logic::glossary::{self, SHORTCUTS};
use crate::logic::jobs::spec_title;
use crate::logic::palette::rank;
use crate::logic::urls::{api as paths, route};
use crate::state::{session, toasts, ui};

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Go(String),
    Demo,
    Theme,
    SignOut,
    Shortcuts,
}

#[derive(Clone, Debug, PartialEq)]
struct Command {
    label: String,
    group: &'static str,
    action: Action,
    role: Role,
}

fn go(label: &str, path: &str, role: Role) -> Command {
    Command {
        label: label.to_owned(),
        group: "Go to",
        action: Action::Go(path.to_owned()),
        role,
    }
}

fn static_commands() -> Vec<Command> {
    vec![
        go("Dashboard", "/", Role::Viewer),
        go("Experiments", "/experiments", Role::Viewer),
        go("Jobs", "/jobs", Role::Viewer),
        go("New run", "/new", Role::Operator),
        go("Compare sweeps", "/compare", Role::Viewer),
        go("Benchmarks", "/benchmarks", Role::Viewer),
        go("Welcome & onboarding", "/welcome", Role::Viewer),
        go(
            "Settings: model profiles",
            "/settings/profiles",
            Role::Viewer,
        ),
        go("Settings: secrets", "/settings/secrets", Role::Admin),
        go("Settings: API tokens", "/settings/tokens", Role::Admin),
        go("Settings: audit log", "/settings/audit", Role::Admin),
        go("Settings: system", "/settings/system", Role::Viewer),
        go("Help & glossary", "/help", Role::Viewer),
        Command {
            label: "Run the demo".to_owned(),
            group: "Actions",
            action: Action::Demo,
            role: Role::Operator,
        },
        Command {
            label: "Switch theme (light / dark / system)".to_owned(),
            group: "Actions",
            action: Action::Theme,
            role: Role::Viewer,
        },
        Command {
            label: "Show keyboard shortcuts".to_owned(),
            group: "Actions",
            action: Action::Shortcuts,
            role: Role::Viewer,
        },
        Command {
            label: "Sign out".to_owned(),
            group: "Actions",
            action: Action::SignOut,
            role: Role::Viewer,
        },
    ]
}

async fn dynamic_commands() -> Vec<Command> {
    let mut out = Vec::new();
    if let Ok(exps) = api::get::<Vec<Experiment>>(&paths::experiments(false)).await {
        for e in exps.into_iter().take(30) {
            out.push(Command {
                label: format!("Experiment {}", e.name),
                group: "Experiments",
                action: Action::Go(route::experiment(&e.name)),
                role: Role::Viewer,
            });
        }
    }
    if let Ok(page) = api::get::<Page<Job>>(&paths::jobs("", "", "", 15, None)).await {
        for j in page.items {
            out.push(Command {
                label: format!("Job: {} ({})", spec_title(&j.spec), j.status.as_str()),
                group: "Recent jobs",
                action: Action::Go(route::job(&j.id)),
                role: Role::Viewer,
            });
        }
    }
    out
}

#[component]
pub(crate) fn CommandPalette() -> impl IntoView {
    let u = ui();
    let s = session();
    let t = toasts();
    let navigate = use_navigate();
    let query = RwSignal::new(String::new());
    let selected = RwSignal::new(0_usize);
    let extra = LocalResource::new(move || {
        let open = u.palette.get();
        async move {
            if open {
                dynamic_commands().await
            } else {
                Vec::new()
            }
        }
    });
    let commands = Memo::new(move |_| {
        let role = s.role().unwrap_or(Role::Viewer);
        let mut all: Vec<Command> = static_commands();
        all.extend(extra.get().unwrap_or_default());
        all.retain(|c| role.allows(c.role));
        let q = query.get();
        let order = rank(&q, all.iter().map(|c| c.label.as_str()));
        order
            .into_iter()
            .take(40)
            .filter_map(|i| all.get(i).cloned())
            .collect::<Vec<_>>()
    });
    Effect::new(move |_| {
        query.track();
        selected.set(0);
    });
    Effect::new(move |_| {
        if !u.palette.get() {
            query.set(String::new());
        }
    });
    let close = Callback::new(move |()| u.palette.set(false));
    let run = StoredValue::new_local(move |cmd: Command| {
        u.palette.set(false);
        match cmd.action {
            Action::Go(path) => navigate(&path, NavigateOptions::default()),
            Action::Demo => actions::start_job(actions::demo_spec(), t, None, navigate.clone()),
            Action::Theme => u.set_theme(u.theme.get_untracked().next()),
            Action::SignOut => actions::sign_out(s, t, navigate.clone()),
            Action::Shortcuts => u.shortcuts.set(true),
        }
    });
    let on_key = move |ev: ev::KeyboardEvent| {
        let n = commands.with_untracked(Vec::len);
        match ev.key().as_str() {
            "ArrowDown" if n > 0 => {
                ev.prevent_default();
                selected.update(|i| *i = (*i + 1) % n);
            }
            "ArrowUp" if n > 0 => {
                ev.prevent_default();
                selected.update(|i| *i = (*i + n - 1) % n);
            }
            "Enter" => {
                ev.prevent_default();
                let i = selected.get_untracked();
                if let Some(cmd) = commands.with_untracked(|c| c.get(i).cloned()) {
                    run.with_value(|f| f(cmd));
                }
            }
            _ => {}
        }
        let id = format!("palette-opt-{}", selected.get_untracked());
        if let Some(el) = document().get_element_by_id(&id) {
            el.scroll_into_view_with_bool(false);
        }
    };
    view! {
        <Modal open=u.palette on_close=close title="Go to…" testid="command-palette">
            <div class="palette">
                <input
                    class="input palette-input"
                    type="text"
                    role="combobox"
                    aria-expanded="true"
                    aria-controls="palette-list"
                    aria-autocomplete="list"
                    aria-label="Search pages, experiments, jobs and actions"
                    aria-activedescendant=move || format!("palette-opt-{}", selected.get())
                    placeholder="Type to search pages, experiments, jobs and actions"
                    data-autofocus=""
                    autocomplete="off"
                    spellcheck="false"
                    bind:value=query
                    on:keydown=on_key
                />
                <ul id="palette-list" class="palette-list" role="listbox" aria-label="Commands">
                    {move || {
                        let cmds = commands.get();
                        if cmds.is_empty() {
                            return view! {
                                <li class="palette-empty" role="option" aria-selected="false" aria-disabled="true">
                                    "Nothing matches. Try another word."
                                </li>
                            }
                                .into_any();
                        }
                        cmds.into_iter()
                            .enumerate()
                            .map(|(i, cmd)| {
                                let group = cmd.group;
                                let label = cmd.label.clone();
                                view! {
                                    <li
                                        id=format!("palette-opt-{i}")
                                        role="option"
                                        class="palette-item"
                                        aria-selected=move || if selected.get() == i { "true" } else { "false" }
                                        on:mousemove=move |_| {
                                            if selected.get_untracked() != i {
                                                selected.set(i);
                                            }
                                        }
                                        on:click=move |_| run.with_value(|f| f(cmd.clone()))
                                    >
                                        <span class="palette-label">{label}</span>
                                        <span class="palette-group">{group}</span>
                                    </li>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </ul>
                <p class="palette-help muted small">
                    "↑ ↓ to choose · Enter to open · Esc to close"
                </p>
            </div>
        </Modal>
    }
}

#[component]
pub(crate) fn ShortcutsDialog() -> impl IntoView {
    let u = ui();
    let close = Callback::new(move |()| u.shortcuts.set(false));
    view! {
        <Modal open=u.shortcuts on_close=close title="Keyboard shortcuts">
            <table class="table shortcuts">
                <thead>
                    <tr>
                        <th scope="col">"Keys"</th>
                        <th scope="col">"Action"</th>
                    </tr>
                </thead>
                <tbody>
                    {SHORTCUTS
                        .iter()
                        .map(|s| {
                            view! {
                                <tr>
                                    <td>
                                        <kbd>{s.keys}</kbd>
                                    </td>
                                    <td>{s.action}</td>
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
            <p class="muted small">"Single-key shortcuts are ignored while you type in a field."</p>
        </Modal>
    }
}

/// Installs the window-level keyboard shortcuts. Renders nothing.
#[component]
pub(crate) fn GlobalKeys() -> impl IntoView {
    let u = ui();
    let navigate = use_navigate();
    let chord_at = StoredValue::new(None::<f64>);
    let handle = window_event_listener(ev::keydown, move |ev| {
        let key = ev.key();
        let ctrl_or_meta = ev.ctrl_key() || ev.meta_key();
        if ctrl_or_meta && key.eq_ignore_ascii_case("k") {
            ev.prevent_default();
            u.palette.update(|o| *o = !*o);
            return;
        }
        if key == "Escape" && u.nav_open.get_untracked() {
            u.nav_open.set(false);
            return;
        }
        if ctrl_or_meta || ev.alt_key() || u.modal_open() || browser::typing_in_field(&ev) {
            return;
        }
        let now = js_sys::Date::now();
        match key.as_str() {
            "?" => {
                ev.prevent_default();
                u.shortcuts.set(true);
            }
            "/" => {
                if browser::focus_id("page-search") {
                    ev.prevent_default();
                }
            }
            "g" => chord_at.set_value(Some(now)),
            k => {
                if let Some(t0) = chord_at.get_value() {
                    chord_at.set_value(None);
                    if now - t0 < 1500.0
                        && let Some(path) = glossary::chord_target(k)
                    {
                        ev.prevent_default();
                        navigate(path, NavigateOptions::default());
                    }
                }
            }
        }
    });
    on_cleanup(move || handle.remove());
}
