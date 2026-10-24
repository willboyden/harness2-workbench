//! The root component: contexts, router, authentication gate and routes.

use h2s_api::{Principal, SystemInfo};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::NavigateOptions;
use leptos_router::components::{Redirect, Route, Router, Routes};
use leptos_router::hooks::{use_location, use_navigate};
use leptos_router::path;

use crate::api::{self, ApiError};
use crate::components::layout::Shell;
use crate::components::palette::GlobalKeys;
use crate::components::ui::ErrorPanel;
use crate::logic::urls::{api as paths, route};
use crate::pages;
use crate::state::{self, Session, session};

/// Load `/system/info` into the session (versions, isolation, harness2 status).
pub(crate) fn load_system(s: Session) {
    spawn_local(async move {
        if let Ok(info) = api::get::<SystemInfo>(&paths::system_info()).await {
            s.system.set(Some(info));
        }
    });
}

/// Checks the session at start-up and sends signed-out users to the sign-in page.
#[component]
fn AuthGate(boot_error: RwSignal<Option<ApiError>>) -> impl IntoView {
    let s = session();
    let loc = use_location();
    let navigate = use_navigate();
    let here = move || {
        let path = loc.pathname.get_untracked();
        let search = loc.search.get_untracked();
        if search.is_empty() {
            path
        } else {
            format!("{path}?{search}")
        }
    };
    {
        let navigate = navigate.clone();
        api::on_unauthorized(move || {
            s.principal.set(None);
            let current = here();
            if !current.starts_with("/login") {
                navigate(
                    &route::login_next(&current),
                    NavigateOptions {
                        replace: true,
                        ..NavigateOptions::default()
                    },
                );
            }
        });
    }
    let check = move || {
        spawn_local(async move {
            match api::get::<Principal>(&paths::me()).await {
                Ok(p) => {
                    s.principal.set(Some(p));
                    boot_error.set(None);
                    load_system(s);
                }
                Err(e) if e.is_unauthorized() => boot_error.set(None),
                Err(e) => boot_error.set(Some(e)),
            }
            s.checked.set(true);
        });
    };
    check();
    Effect::new(move |_| {
        let signed_out = s.checked.get() && s.principal.with(Option::is_none);
        let path = loc.pathname.get();
        if signed_out && boot_error.with(Option::is_none) && path != "/login" {
            navigate(
                &route::login_next(&here()),
                NavigateOptions {
                    replace: true,
                    ..NavigateOptions::default()
                },
            );
        }
    });
}

/// Shown while the session check is in flight, or when the server cannot be reached.
#[component]
fn Boot(boot_error: RwSignal<Option<ApiError>>) -> impl IntoView {
    move || match boot_error.get() {
        Some(e) => {
            let retry = Callback::new(move |()| {
                if let Some(w) = web_sys::window() {
                    let _ = w.location().reload();
                }
            });
            view! {
                <div class="boot">
                    <ErrorPanel error=e retry=retry />
                </div>
            }
            .into_any()
        }
        None => view! {
            <div class="boot" aria-busy="true">
                <p class="muted">"Connecting to the server…"</p>
            </div>
        }
        .into_any(),
    }
}

/// Everything inside the router: gate, shell and routes.
#[component]
fn Root() -> impl IntoView {
    let s = session();
    let loc = use_location();
    let boot_error = RwSignal::new(None::<ApiError>);
    view! {
        <AuthGate boot_error=boot_error />
        <GlobalKeys />
        <Shell>
            <Show
                when=move || {
                    (s.checked.get() && boot_error.with(Option::is_none))
                        || loc.pathname.get() == "/login"
                }
                fallback=move || view! { <Boot boot_error=boot_error /> }
            >
                <Routes fallback=|| view! { <pages::not_found::NotFound /> }>
                    <Route path=path!("/login") view=pages::login::Login />
                    <Route path=path!("/") view=pages::dashboard::Dashboard />
                    <Route path=path!("/welcome") view=pages::welcome::Welcome />
                    <Route path=path!("/experiments") view=pages::experiments::Experiments />
                    <Route path=path!("/experiments/:exp") view=pages::experiment::ExperimentPage />
                    <Route path=path!("/experiments/:exp/sweeps/:sweep") view=pages::sweep::SweepPage />
                    <Route
                        path=path!("/experiments/:exp/sweeps/:sweep/tasks/:task")
                        view=pages::task::TaskPage
                    />
                    <Route path=path!("/new") view=pages::new_run::NewRun />
                    <Route path=path!("/jobs") view=pages::jobs::Jobs />
                    <Route path=path!("/jobs/:id") view=pages::job::JobPage />
                    <Route path=path!("/compare") view=pages::compare::Compare />
                    <Route path=path!("/benchmarks") view=pages::benchmarks::Benchmarks />
                    <Route path=path!("/settings") view=|| view! { <Redirect path="/settings/profiles" /> } />
                    <Route path=path!("/settings/:section") view=pages::settings::Settings />
                    <Route path=path!("/help") view=pages::help::Help />
                </Routes>
            </Show>
        </Shell>
    }
}

#[component]
pub(crate) fn App() -> impl IntoView {
    state::provide_all();
    view! {
        <Router>
            <Root />
        </Router>
    }
}
