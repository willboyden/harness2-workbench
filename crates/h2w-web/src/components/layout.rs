//! The app shell: skip link, sidebar navigation, top bar and the page outlet.

use h2w_api::Role;
use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate};

use super::icons::{Icon, icon};
use super::modal::ConfirmHost;
use super::palette::{CommandPalette, ShortcutsDialog};
use super::toast::ToastHost;
use crate::actions;
use crate::browser;
use crate::logic::theme::Theme;
use crate::state::{session, toasts, ui};

/// Whether a nav item is the current section.
fn is_active(current: &str, href: &str) -> bool {
    if href == "/" {
        current == "/"
    } else {
        current == href || current.starts_with(&format!("{href}/"))
    }
}

#[component]
fn NavLink(
    href: &'static str,
    label: &'static str,
    glyph: Icon,
    testid: &'static str,
    /// Section prefix used for highlighting (defaults to `href`).
    #[prop(optional)]
    section: Option<&'static str>,
) -> impl IntoView {
    let loc = use_location();
    let section = section.unwrap_or(href);
    view! {
        <li>
            <a
                href=href
                class="nav-link"
                data-testid=testid
                aria-current=move || is_active(&loc.pathname.get(), section).then_some("page")
            >
                {icon(glyph)}
                <span>{label}</span>
            </a>
        </li>
    }
}

#[component]
fn Sidebar() -> impl IntoView {
    let s = session();
    let version = move || {
        s.system
            .with(|i| i.as_ref().map(|i| format!("v{}", i.studio_version)))
            .unwrap_or_default()
    };
    view! {
        <nav id="sidebar" class="sidebar" aria-label="Main">
            <a class="brand" href="/">
                <img src="/favicon.svg" alt="" width="28" height="28" />
                <span class="brand-name">"Harness² Workbench"</span>
            </a>
            <ul class="nav-list">
                <NavLink href="/" label="Dashboard" glyph=Icon::Dashboard testid="nav-dashboard" />
                <NavLink href="/experiments" label="Experiments" glyph=Icon::Flask testid="nav-experiments" />
                <NavLink href="/jobs" label="Jobs" glyph=Icon::Jobs testid="nav-jobs" />
                // Starting anything (even a preview) needs the operator role.
                {move || {
                    s.can(Role::Operator)
                        .then(|| view! { <NavLink href="/new" label="New run" glyph=Icon::Plus testid="nav-new-run" /> })
                }}
                <NavLink href="/compare" label="Compare" glyph=Icon::Compare testid="nav-compare" />
                <NavLink href="/benchmarks" label="Benchmarks" glyph=Icon::Box testid="nav-benchmarks" />
                <NavLink
                    href="/settings/profiles"
                    section="/settings"
                    label="Settings"
                    glyph=Icon::Gear
                    testid="nav-settings"
                />
                <NavLink href="/help" label="Help" glyph=Icon::Help testid="nav-help" />
            </ul>
            <div class="sidebar-foot">
                <a href="/welcome" class="nav-link small">
                    {icon(Icon::Sparkle)}
                    <span>"Getting started"</span>
                </a>
                <p class="muted small">
                    "Unofficial UI for google-research/harness2 "{version}
                </p>
            </div>
        </nav>
    }
}

#[component]
fn ThemeToggle() -> impl IntoView {
    let u = ui();
    let glyph = move || match u.theme.get() {
        Theme::Light => icon(Icon::Sun).into_any(),
        Theme::Dark => icon(Icon::Moon).into_any(),
        Theme::System => icon(Icon::Monitor).into_any(),
    };
    let label = move || {
        format!(
            "{} (switch to {})",
            u.theme.get().label(),
            u.theme.get().next().label().to_lowercase()
        )
    };
    view! {
        <button
            type="button"
            class="icon-btn"
            data-testid="theme-toggle"
            aria-label=label
            title=label
            on:click=move |_| u.set_theme(u.theme.get_untracked().next())
        >
            {glyph}
        </button>
    }
}

#[component]
fn TopBar() -> impl IntoView {
    let s = session();
    let u = ui();
    let t = toasts();
    let navigate = use_navigate();
    let user = move || {
        s.principal.with(|p| {
            p.as_ref()
                .map(|p| (p.name.clone(), p.role.as_str()))
                .unwrap_or_default()
        })
    };
    view! {
        <header class="topbar">
            <button
                type="button"
                class="icon-btn nav-toggle"
                aria-label="Open navigation"
                aria-controls="sidebar"
                aria-expanded=move || if u.nav_open.get() { "true" } else { "false" }
                on:click=move |_| {
                    let opening = !u.nav_open.get_untracked();
                    u.nav_open.set(opening);
                    if opening {
                        // Move focus into the drawer once it is visible.
                        request_animation_frame(|| {
                            if let Some(el) = document()
                                .query_selector("#sidebar .nav-link")
                                .ok()
                                .flatten()
                                .and_then(|e| wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(e).ok())
                            {
                                let _ = el.focus();
                            }
                        });
                    }
                }
            >
                {icon(Icon::Menu)}
            </button>
            <button
                type="button"
                class="palette-trigger"
                aria-label="Open the command palette (Ctrl+K)"
                on:click=move |_| u.palette.set(true)
            >
                {icon(Icon::Search)}
                <span class="palette-trigger-text">"Search or jump to…"</span>
                <kbd>"Ctrl K"</kbd>
            </button>
            <div class="topbar-right">
                <ThemeToggle />
                <span class="user" data-testid="current-user">
                    <span class="user-name">{move || user().0}</span>
                    <span class="badge tone-neutral user-role">{move || user().1}</span>
                </span>
                <button
                    type="button"
                    class="icon-btn"
                    aria-label="Sign out"
                    title="Sign out"
                    on:click=move |_| actions::sign_out(s, t, navigate.clone())
                >
                    {icon(Icon::Logout)}
                </button>
            </div>
        </header>
    }
}

/// Wraps every page. The sign-in page renders without navigation chrome.
#[component]
pub(crate) fn Shell(children: Children) -> impl IntoView {
    let u = ui();
    let s = session();
    let loc = use_location();
    let bare = move || loc.pathname.get() == "/login" || s.principal.with(Option::is_none);
    // Close the drawer and move focus to the page after each navigation.
    Effect::new(move |prev: Option<String>| {
        let path = loc.pathname.get();
        u.nav_open.set(false);
        if prev.is_some_and(|p| p != path) {
            request_animation_frame(|| {
                browser::focus_id("main");
            });
        }
        path
    });
    view! {
        <div
            id="app-shell"
            class="shell"
            class:bare=bare
            class:nav-open=move || u.nav_open.get()
            inert=move || u.modal_open()
        >
            <a
                class="skip-link"
                href="#main"
                on:click=move |ev| {
                    ev.prevent_default();
                    browser::focus_id("main");
                }
            >
                "Skip to content"
            </a>
            {move || {
                (!bare())
                    .then(|| {
                        view! {
                            <Sidebar />
                            <div class="nav-scrim" on:click=move |_| u.nav_open.set(false)></div>
                            <TopBar />
                        }
                    })
            }}
            <main id="main" class="main" tabindex="-1">
                {children()}
            </main>
        </div>
        <ConfirmHost />
        <CommandPalette />
        <ShortcutsDialog />
        <ToastHost />
    }
}
