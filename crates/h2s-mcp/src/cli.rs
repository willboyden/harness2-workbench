//! Command line and the two run modes.

use std::{net::SocketAddr, path::PathBuf};

use clap::{Parser, ValueEnum};
use rmcp::ServiceExt;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::{
    client::{ApiClient, ClientOptions},
    http::{self, HttpOptions, MCP_PATH},
    limits::WaitLimiter,
    server::{Credentials, H2sMcp},
    token::Token,
};

/// Environment variable holding the API token for stdio mode.
pub const TOKEN_ENV: &str = "H2S_TOKEN";

const LONG_ABOUT: &str = "Harness² Studio MCP server: every harness2 operation the Studio web \
UI offers, as MCP tools, resources and prompts for AI agents (Claude Code, opencode, Codex, \
Cursor, VS Code, ...).

It is a thin client of a running Harness² Studio server (`h2studio serve`): it keeps no state, \
and every call uses an API token, so the server's role checks and audit log apply.

  stdio (default): the MCP client starts this program; the token comes from H2S_TOKEN or a \
token file that only you can read (--token-file / H2S_TOKEN_FILE). Tokens are never accepted \
as command-line arguments.
  http: streamable HTTP on --listen at path /mcp; every request must carry its own \
`Authorization: Bearer h2s_...`, which is forwarded to the Studio server.

Models cannot set acknowledge_risks=true unless --allow-risk-acknowledgement is given.

Logs go to stderr (RUST_LOG adjusts them).";

/// How MCP clients connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TransportKind {
    /// The client starts this program and talks over stdin/stdout.
    Stdio,
    /// Streamable HTTP; each client sends its own bearer token.
    Http,
}

/// `h2studio-mcp` command line.
#[derive(Debug, Parser)]
#[command(
    name = "h2studio-mcp",
    version,
    about = "Harness² Studio MCP server: harness2 for AI agents",
    long_about = LONG_ABOUT
)]
pub struct Cli {
    /// How MCP clients connect.
    #[arg(long, value_enum, default_value_t = TransportKind::Stdio)]
    pub transport: TransportKind,

    /// Address to listen on with --transport http.
    #[arg(long, default_value = "127.0.0.1:8723")]
    pub listen: SocketAddr,

    /// URL of the Harness² Studio server.
    #[arg(
        long,
        env = "H2S_SERVER_URL",
        default_value = "http://127.0.0.1:8722",
        value_name = "URL"
    )]
    pub server_url: String,

    /// File holding the API token for --transport stdio; it must not be readable by group or
    /// others (chmod 600). Alternatively set H2S_TOKEN.
    #[arg(long, env = "H2S_TOKEN_FILE", value_name = "PATH")]
    pub token_file: Option<PathBuf>,

    /// Extra Host value accepted with --transport http (host or host:port; repeatable), for
    /// example the name a reverse proxy forwards. Loopback forms of --listen are always
    /// accepted.
    #[arg(long = "allowed-host", value_name = "HOST[:PORT]")]
    pub allowed_hosts: Vec<String>,

    /// Extra browser Origin accepted with --transport http (repeatable), e.g.
    /// `https://inspector.example.com`.
    #[arg(long = "allowed-origin", value_name = "ORIGIN")]
    pub allowed_origins: Vec<String>,

    /// Largest MCP request body accepted with --transport http, in bytes.
    #[arg(long, default_value_t = 1024 * 1024, value_name = "BYTES")]
    pub max_body_bytes: usize,

    /// Most MCP requests handled at once with --transport http; more are refused with 503 and
    /// `Retry-After: 1`.
    #[arg(
        long,
        default_value_t = 32,
        value_name = "N",
        value_parser = clap::value_parser!(u32).range(1..=10_000)
    )]
    pub max_concurrent_requests: u32,

    /// Most waits (wait_for_job, or job tools with wait_secs) one API token may run at once.
    #[arg(
        long,
        default_value_t = 4,
        value_name = "N",
        value_parser = clap::value_parser!(u32).range(1..=1_000)
    )]
    pub max_waits_per_token: u32,

    /// Let MCP clients set acknowledge_risks=true on start_run and start_pipeline, accepting for
    /// the user that a host-isolation executor gets the server user's files and network. Off by
    /// default: then a human starts such runs from the web UI, or the Studio server
    /// pre-acknowledges host risks. Turn it on only if a human reviews every run an agent starts.
    #[arg(
        long,
        env = "H2S_MCP_ALLOW_RISK_ACK",
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    pub allow_risk_acknowledgement: bool,

    /// Allow a plain-http --server-url on a non-loopback host (API tokens would cross the
    /// network unencrypted).
    #[arg(long)]
    pub allow_insecure_remote: bool,
}

/// Run the MCP server until the client disconnects (stdio) or a shutdown signal (http).
///
/// # Errors
/// Bad configuration (server URL, token), or a transport failure.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    let options = ClientOptions {
        allow_insecure_remote: cli.allow_insecure_remote,
        ..ClientOptions::default()
    };
    let api = ApiClient::new(&cli.server_url, &options)?;
    let env_token = std::env::var(TOKEN_ENV).ok();
    let max_waits = usize::try_from(cli.max_waits_per_token).unwrap_or(usize::MAX);
    if cli.allow_risk_acknowledgement {
        tracing::warn!(
            "--allow-risk-acknowledgement: MCP clients may set acknowledge_risks=true, so a model \
             can start host-isolation runs that give the executor this server user's files and \
             network"
        );
    }
    match cli.transport {
        TransportKind::Stdio => {
            let token = Token::resolve(env_token.as_deref(), cli.token_file.as_deref())?;
            tracing::info!(server = api.server_url(), "serving MCP over stdio");
            let service = H2sMcp::new(api, Credentials::Fixed(token))
                .with_wait_limiter(WaitLimiter::new(max_waits))
                .with_risk_acknowledgement(cli.allow_risk_acknowledgement)
                .serve(rmcp::transport::stdio())
                .await?;
            let reason = service.waiting().await?;
            tracing::info!(?reason, "MCP client disconnected");
        }
        TransportKind::Http => {
            if env_token.is_some() || cli.token_file.is_some() {
                tracing::warn!(
                    "H2S_TOKEN / --token-file are not used with --transport http: every MCP \
                     request must carry its caller's own bearer token"
                );
            }
            let listener = TcpListener::bind(cli.listen).await?;
            let local = listener.local_addr()?;
            if !local.ip().is_loopback() {
                tracing::warn!(
                    %local,
                    "listening beyond loopback: serve it through a TLS reverse proxy and \
                     list its host name with --allowed-host"
                );
            }
            tracing::info!(
                url = %format!("http://{local}{MCP_PATH}"),
                server = api.server_url(),
                "serving MCP over streamable HTTP"
            );
            let shutdown = CancellationToken::new();
            tokio::spawn(stop_on_signal(shutdown.clone()));
            let http_options = HttpOptions {
                allowed_hosts: cli.allowed_hosts,
                allowed_origins: cli.allowed_origins,
                max_body_bytes: cli.max_body_bytes,
                max_concurrent_requests: usize::try_from(cli.max_concurrent_requests)
                    .unwrap_or(usize::MAX),
                max_waits_per_token: max_waits,
                allow_risk_acknowledgement: cli.allow_risk_acknowledgement,
            };
            http::serve(listener, api, &http_options, shutdown).await?;
        }
    }
    Ok(())
}

/// Cancel `shutdown` on Ctrl-C or SIGTERM.
async fn stop_on_signal(shutdown: CancellationToken) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot watch SIGTERM; only Ctrl-C stops the server");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    tracing::info!("shutting down");
    shutdown.cancel();
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn tokens_are_never_command_line_arguments() {
        for flag in ["--token", "--h2s-token", "--bearer"] {
            assert!(
                Cli::try_parse_from(["h2studio-mcp", flag, "h2s_alice_s3cret"]).is_err(),
                "{flag} must not exist"
            );
        }
    }

    #[test]
    fn defaults() {
        let cli = Cli::try_parse_from(["h2studio-mcp"]).unwrap();
        assert_eq!(cli.transport, TransportKind::Stdio);
        assert_eq!(cli.listen.to_string(), "127.0.0.1:8723");
        assert_eq!(cli.max_body_bytes, 1024 * 1024);
        assert_eq!(
            (cli.max_concurrent_requests, cli.max_waits_per_token),
            (32, 4)
        );
        assert!(Cli::try_parse_from(["h2studio-mcp", "--max-waits-per-token", "0"]).is_err());
        assert!(
            !cli.allow_risk_acknowledgement,
            "off unless a human turns it on"
        );
        let on = Cli::try_parse_from(["h2studio-mcp", "--allow-risk-acknowledgement"]).unwrap();
        assert!(on.allow_risk_acknowledgement);
    }
}
