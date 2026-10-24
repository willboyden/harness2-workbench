//! Sign in with an API token; the server answers with an HttpOnly session cookie. The token
//! itself is never stored by the browser.

use h2s_api::{LoginRequest, Principal, TOKEN_PREFIX};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;

use crate::api;
use crate::app::load_system;
use crate::browser;
use crate::logic::urls::{api as paths, route};
use crate::nav::query_param;
use crate::state::session;

#[component]
pub(crate) fn Login() -> impl IntoView {
    let s = session();
    let navigate = use_navigate();
    let next = query_param("next");
    let token = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let show = RwSignal::new(false);
    browser::set_title("Sign in");

    // Already signed in (or signed in just now): continue to where the user was going.
    {
        let navigate = navigate.clone();
        Effect::new(move |_| {
            if s.principal.with(Option::is_some) {
                let to = route::safe_next(
                    Some(&next.get())
                        .filter(|n| !n.is_empty())
                        .map(String::as_str),
                );
                navigate(
                    &to,
                    NavigateOptions {
                        replace: true,
                        ..NavigateOptions::default()
                    },
                );
            }
        });
    }

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let value = token.get_untracked().trim().to_owned();
        if value.is_empty() {
            error.set(Some("Paste your API token first.".to_owned()));
            return;
        }
        if !value.starts_with(TOKEN_PREFIX) {
            error.set(Some(format!(
                "That does not look like a Studio token: tokens start with “{TOKEN_PREFIX}”."
            )));
            return;
        }
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let result =
                api::post::<_, Principal>(&paths::login(), &LoginRequest { token: value }).await;
            busy.set(false);
            match result {
                Ok(p) => {
                    token.set(String::new());
                    s.principal.set(Some(p));
                    s.checked.set(true);
                    load_system(s);
                }
                Err(e) => {
                    let msg = if e.is_unauthorized() {
                        "The server did not accept this token. Check that it was copied completely and has not been revoked or expired.".to_owned()
                    } else if e.status == 429 {
                        format!(
                            "Too many sign-in attempts from this address. Wait a minute before trying again. ({})",
                            e.message
                        )
                    } else {
                        e.to_string()
                    };
                    error.set(Some(msg));
                }
            }
        });
    };

    view! {
        <div class="login-page">
            <div class="login-card card">
                <div class="login-brand">
                    <img src="/favicon.svg" alt="" width="40" height="40" />
                    <div>
                        <h1>"Sign in to Harness² Studio"</h1>
                        <p class="muted">"An unofficial workbench for google-research/harness2."</p>
                    </div>
                </div>
                <form on:submit=submit novalidate=true>
                    <div class="field" class:has-error=move || error.get().is_some()>
                        <label for="login-token">"API token"</label>
                        <div class="input-with-button">
                            <input
                                id="login-token"
                                class="input mono"
                                type=move || if show.get() { "text" } else { "password" }
                                autocomplete="off"
                                spellcheck="false"
                                placeholder="h2s_…"
                                data-testid="login-token"
                                aria-describedby="login-help"
                                aria-invalid=move || if error.get().is_some() { "true" } else { "false" }
                                bind:value=token
                            />
                            <button
                                type="button"
                                class="btn btn-secondary"
                                aria-pressed=move || if show.get() { "true" } else { "false" }
                                on:click=move |_| show.update(|v| *v = !*v)
                            >
                                {move || if show.get() { "Hide" } else { "Show" }}
                            </button>
                        </div>
                        {move || {
                            error
                                .get()
                                .map(|e| {
                                    view! {
                                        <p class="field-error" role="alert" data-testid="login-error">
                                            {e}
                                        </p>
                                    }
                                })
                        }}
                    </div>
                    <button
                        type="submit"
                        class="btn btn-primary btn-block"
                        data-testid="login-submit"
                        disabled=move || busy.get()
                    >
                        {move || if busy.get() { "Signing in…" } else { "Sign in" }}
                    </button>
                </form>
                <div id="login-help" class="login-help">
                    <h2>"Where do tokens come from?"</h2>
                    <ul>
                        <li>
                            "The first administrator token is printed once by "
                            <code>"h2studio init"</code>
                            " on the server (and saved to "<code>"admin.token"</code>
                            " in its configuration directory, readable only by the server's user)."
                        </li>
                        <li>
                            "Administrators create more tokens, for each person or tool, under "
                            <strong>"Settings → Tokens"</strong>"."
                        </li>
                        <li>
                            "Signing in exchanges the token for a session cookie the page cannot read. The token is not saved in this browser."
                        </li>
                    </ul>
                </div>
            </div>
        </div>
    }
}
