//! Route helpers: path params and URL query state (so filters and tabs survive reloads and
//! can be linked).

use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::{use_location, use_navigate, use_params_map, use_query_map};

use crate::logic::urls::{query, route};

/// A path parameter of the current route (`:exp`), decoded.
pub(crate) fn param(name: &'static str) -> Memo<String> {
    let params = use_params_map();
    Memo::new(move |_| params.with(|p| p.get(name).unwrap_or_default()))
}

/// A query parameter of the current URL (`?tab=report`), decoded; empty when absent.
pub(crate) fn query_param(name: &'static str) -> Memo<String> {
    let q = use_query_map();
    Memo::new(move |_| q.with(|p| p.get(name).unwrap_or_default()))
}

/// A setter that rewrites query parameters of the current page in place (history replace).
/// Pairs with an empty value are removed.
pub(crate) fn query_setter() -> impl Fn(&[(&str, &str)]) + Clone + 'static {
    let navigate = use_navigate();
    let loc = use_location();
    let q = use_query_map();
    move |changes: &[(&str, &str)]| {
        let mut pairs: Vec<(String, String)> = q.with_untracked(|m| {
            m.latest_values()
                .map(|(k, v)| (k.to_string(), v.to_owned()))
                .collect()
        });
        for (k, v) in changes {
            pairs.retain(|(pk, _)| pk != k);
            if !v.is_empty() {
                pairs.push(((*k).to_owned(), (*v).to_owned()));
            }
        }
        let refs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let path = format!("{}{}", loc.pathname.get_untracked(), query(&refs));
        navigate(
            &path,
            NavigateOptions {
                replace: true,
                scroll: false,
                ..NavigateOptions::default()
            },
        );
    }
}

/// Click handling for table rows that lead somewhere: a click anywhere on the row opens
/// `href`, except on the row's own links and controls (keyboard users use the link in the
/// first cell). Call inside a component (it needs the router).
pub(crate) fn row_navigator() -> impl Fn(&leptos::ev::MouseEvent, &str) + Clone + 'static {
    use wasm_bindgen::JsCast;
    let navigate = use_navigate();
    move |ev: &leptos::ev::MouseEvent, href: &str| {
        let on_control = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| {
                el.closest("a, button, input, select, textarea, label, summary")
                    .ok()
                    .flatten()
            })
            .is_some();
        if !on_control && !ev.default_prevented() {
            navigate(href, NavigateOptions::default());
        }
    }
}

/// The page to open after sign-in: `next` when it is a clean path on this origin, else "/".
/// The pure check runs first, then the browser's URL parser must keep the result on this
/// origin, and the parsed path is checked again (normalisation must not produce a
/// protocol-relative "//host" path).
pub(crate) fn follow_next(next: Option<&str>) -> String {
    let candidate = route::safe_next(next);
    match crate::browser::same_origin_path(&candidate) {
        Some(parsed) => route::safe_next(Some(&parsed)),
        None => "/".to_owned(),
    }
}
