//! `h2workbench-mcp`: the Harness² Workbench MCP server (see `h2workbench-mcp --help`).

use std::{
    io::{IsTerminal, Write},
    process::ExitCode,
};

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = h2w_mcp::Cli::parse();
    // stdout is the MCP channel in stdio mode: every log line goes to stderr.
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,h2w_mcp=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();

    match h2w_mcp::run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Written directly as well, so the reason a server refused to start is visible
            // even when RUST_LOG filters error logs out.
            let _ = writeln!(std::io::stderr().lock(), "h2workbench-mcp: {e:#}");
            ExitCode::FAILURE
        }
    }
}
