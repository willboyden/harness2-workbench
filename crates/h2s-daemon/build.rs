//! Rebuild the server when the web UI it embeds changes.
//!
//! `http::static_ui` embeds `dist/web` (built by `cargo xtask web`) with rust-embed. In builds
//! without debug assertions (release) the files are compiled into the binary, and Cargo would
//! not notice `dist/web` appearing, or files being added to it, after a build: the server
//! kept serving the "UI not built" page until something else changed. Watching the directory
//! fixes that (Cargo scans a watched directory recursively).
//!
//! With debug assertions rust-embed reads the files at run time, but only when `dist/web`
//! existed as the crate was compiled; `cargo xtask web` therefore marks `http/static_ui.rs`
//! changed, so the next build of either kind embeds the UI it just built. Watching the
//! directory in debug builds would only cost rebuilds: a watched path that does not exist
//! makes Cargo rerun this script, and rebuild the crate, on every build. Release builds pay
//! that until the UI is built once (`cargo xtask dist` builds it first).

// Build scripts talk to Cargo on stdout.
#![allow(clippy::print_stdout)]

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_CFG_DEBUG_ASSERTIONS").is_none() {
        println!("cargo:rerun-if-changed=../../dist/web");
    }
}
