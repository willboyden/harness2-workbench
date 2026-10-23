//! Development-only mock of the Harness² Studio REST API for UI work. It serves every
//! route the web UI uses with fixture data built from the `h2s_api` wire types (so the
//! shapes always match the contract), simulates jobs with live logs over server-sent
//! events, enforces the same security headers and request checks as the real server, and
//! serves `dist/web` with the SPA fallback. Never shipped.
//!
//! Sign in with any token that starts with `h2s_dev_` (`h2s_dev_viewer...` and
//! `h2s_dev_operator...` give those roles; anything else is an admin).

// A development tool reports to the terminal.
#![allow(clippy::print_stderr, clippy::print_stdout)]

mod api;
mod sim;
mod state;
mod sweeps;
mod util;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "h2s-mock-api",
    about = "Mock Harness² Studio API for UI development (never ship)"
)]
struct Args {
    /// Port to listen on (loopback only).
    #[arg(long, default_value_t = 8790)]
    port: u16,
    /// The built UI (`cargo xtask web` writes it to dist/web).
    #[arg(long, default_value = "dist/web")]
    dist: PathBuf,
    /// Start like a brand-new server: harness2 not installed, nothing created yet.
    #[arg(long)]
    fresh: bool,
    /// Append `report-uri /__csp-report` to the CSP and print every violation report.
    #[arg(long)]
    csp_report: bool,
    /// Milliseconds a simulated run spends per task.
    #[arg(long, default_value_t = 1500)]
    task_ms: u64,
    /// Milliseconds per task of the long-running job the populated mock starts with.
    #[arg(long, default_value_t = 15_000)]
    live_task_ms: u64,
    /// Pretend jobs run in containers (no risk acknowledgement needed).
    #[arg(long)]
    container: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut inner = if args.fresh {
        state::Inner::fresh()
    } else {
        state::Inner::populated()
    };
    if args.container {
        inner.isolation = h2s_api::Isolation::Container;
    }
    let st = api::AppState {
        s: Arc::new(Mutex::new(inner)),
        dist: args.dist.clone(),
        csp_report: args.csp_report,
        task_ms: args.task_ms,
        port: args.port,
    };
    if !args.fresh {
        api::start_live_jobs(&st, args.live_task_ms);
    }
    let app = api::router(st);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", args.port))
        .await
        .with_context(|| format!("binding 127.0.0.1:{}", args.port))?;
    eprintln!(
        "h2s-mock-api (development only) on http://127.0.0.1:{}/  serving {}",
        args.port,
        args.dist.display()
    );
    eprintln!(
        "sign in with any token starting h2s_dev_ (h2s_dev_viewer / h2s_dev_operator for those roles)"
    );
    axum::serve(listener, app).await.context("serving")?;
    Ok(())
}
