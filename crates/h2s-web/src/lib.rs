//! The Harness² Studio web UI: Leptos client-side rendering compiled to WebAssembly.
//!
//! `logic` holds every pure derivation (natively unit-tested); `components` and `pages` are
//! thin views over it; `api` and `browser` wrap the network and browser APIs.

#![forbid(unsafe_code)]
#![allow(dead_code)]

mod actions;
mod api;
// Two lints misfire on Leptos components, so they are relaxed for the view modules only
// (they stay on for `logic`, `api`, `state` and the rest):
// - `#[component]` expands to `pub` items next to the `pub(crate)` function it annotates,
//   which trips `unreachable_pub` on generated code;
// - component props are owned by design (they become a `'static` props struct), which
//   trips `needless_pass_by_value` whenever a prop is only read.
#[allow(unreachable_pub, clippy::needless_pass_by_value)]
mod app;
mod browser;
#[allow(unreachable_pub, clippy::needless_pass_by_value)]
mod components;
mod logic;
mod nav;
#[allow(unreachable_pub, clippy::needless_pass_by_value)]
mod pages;
mod state;

use wasm_bindgen::prelude::wasm_bindgen;

/// Entry point called by `/app.js` once the module is instantiated.
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
