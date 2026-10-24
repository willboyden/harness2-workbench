//! The `h2studio` command line.
//!
//! Output discipline (scripts depend on it): human-readable progress and explanations go
//! to stderr; stdout carries only machine-readable results — the `listening` line of
//! `serve`, the token of `init`/`token create` (as the only line), tables or JSON of the
//! listing commands.

// Printing to stdout/stderr is this module's job.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use h2s_api::{AuditOutcome, Role};

use crate::config::{self, Config, process_env};
use crate::db::audit::AuditEntry;
use crate::db::tokens::TokenRecord;
use crate::http::server::{Server, ServerOptions};
use crate::jobs::core_backend::CoreBackend;
use crate::util::now_unix;

/// Name of the bootstrap token `init` creates.
pub const BOOTSTRAP_TOKEN_NAME: &str = "initial admin";
/// File (in the configuration directory) `init` writes the bootstrap token to.
pub const ADMIN_TOKEN_FILE: &str = "admin.token";

#[derive(Debug, Parser)]
#[command(
    name = "h2studio",
    version,
    about = "Harness² Studio: GUI, REST API and job runner for google-research/harness2"
)]
pub struct Cli {
    /// Configuration file (default: $H2S_CONFIG, else
    /// $XDG_CONFIG_HOME/harness2-studio/h2studio.toml).
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the server (prints `h2studio listening on http://<ip>:<port>` once bound).
    Serve,
    /// Create the configuration, data directories, database and the first admin token
    /// (printed once on stdout and written 0600 to <config dir>/admin.token).
    Init {
        /// Create another bootstrap admin token even though an admin token exists.
        #[arg(long)]
        force: bool,
        /// Data directory written into a new configuration (default: $H2S_DATA_DIR, else
        /// $XDG_DATA_HOME/harness2-studio).
        #[arg(long, value_name = "PATH")]
        data_dir: Option<PathBuf>,
    },
    /// Manage API tokens.
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
    /// Check the tools and services harness2 needs.
    Doctor {
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Configuration commands.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Audit log commands.
    Audit {
        #[command(subcommand)]
        command: AuditCommand,
    },
    /// Print the OpenAPI 3.1 document of the REST API.
    Openapi,
    /// Print the version.
    Version,
    /// Run the allowlisting egress proxy for container jobs (`[egress]` section).
    EgressProxy,
}

#[derive(Debug, Subcommand)]
pub enum TokenCommand {
    /// Create a token; the token is the only line printed on stdout.
    Create {
        #[arg(long)]
        name: String,
        /// viewer, operator or admin.
        #[arg(long, value_parser = parse_role)]
        role: Role,
        /// Expiry in days (default: never).
        #[arg(long)]
        expires_in_days: Option<u32>,
    },
    /// List tokens (never their secrets).
    List {
        #[arg(long)]
        json: bool,
    },
    /// Revoke a token by id; its browser sessions end at once.
    Revoke { id: String },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Load and validate the configuration and show the resolved paths.
    Check,
}

#[derive(Debug, Subcommand)]
pub enum AuditCommand {
    /// Re-walk the audit hash chain.
    Verify,
}

fn parse_role(s: &str) -> Result<Role, String> {
    crate::db::tokens::parse_role(s).ok_or_else(|| "must be viewer, operator or admin".to_owned())
}

/// Entry point of the binary.
#[must_use]
pub fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("h2studio: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Run one command.
///
/// # Errors
/// Any failure, with context, for the caller to print.
pub fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    let config = cli.config.as_deref();
    match cli.command {
        Command::Serve => serve(config),
        Command::Init { force, data_dir } => init(config, force, data_dir.as_deref()),
        Command::Token { command } => match command {
            TokenCommand::Create {
                name,
                role,
                expires_in_days,
            } => token_create(config, &name, role, expires_in_days),
            TokenCommand::List { json } => token_list(config, json),
            TokenCommand::Revoke { id } => token_revoke(config, &id),
        },
        Command::Doctor { json } => doctor(config, json),
        Command::Config {
            command: ConfigCommand::Check,
        } => config_check(config),
        Command::Audit {
            command: AuditCommand::Verify,
        } => audit_verify(config),
        Command::Openapi => {
            let doc = crate::http::openapi::document_json();
            println!("{}", serde_json::to_string_pretty(&doc)?);
            Ok(ExitCode::SUCCESS)
        }
        Command::Version => {
            match crate::http::routes::system::BUILD_COMMIT {
                Some(c) => println!("h2studio {} ({c})", env!("CARGO_PKG_VERSION")),
                None => println!("h2studio {}", env!("CARGO_PKG_VERSION")),
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::EgressProxy => egress_proxy(config),
    }
}

fn load(config: Option<&Path>) -> anyhow::Result<Config> {
    Ok(Config::load(config, &process_env)?)
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).ok();
    let terminate = async {
        match term.as_mut() {
            Some(t) => {
                t.recv().await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        () = terminate => {}
    }
}

fn serve(config: Option<&Path>) -> anyhow::Result<ExitCode> {
    // First of all: the environment holds secrets (H2S_SECRET_*, the proxy secret) and
    // host-mode jobs run as this user.
    let dumpable = crate::util::make_non_dumpable();
    let cfg = load(config)?;
    let rt = runtime()?;
    rt.block_on(Box::pin(async move {
        let _telemetry = crate::telemetry::init(&cfg).map_err(anyhow::Error::msg)?;
        if let Err(e) = dumpable {
            tracing::warn!(error = %e, "could not make the server non-dumpable; processes of this user can read its environment");
        }
        for (k, v) in cfg.describe_paths() {
            tracing::info!("{k}: {v}");
        }
        for w in crate::telemetry::environment_warnings(&process_env) {
            tracing::warn!("{w}");
        }
        for w in cfg.warnings() {
            tracing::warn!("{w}");
        }
        if cfg.isolation() == h2s_api::Isolation::Host {
            tracing::warn!(
                "isolation.mode = \"host\": jobs run as this user with its files and network (upstream behaviour)"
            );
        }
        let server = Server::bind(cfg, ServerOptions::new(Arc::new(CoreBackend)))
            .await
            .context("starting the server")?;
        let addr = server.local_addr();
        let mut out = std::io::stdout().lock();
        writeln!(out, "h2studio listening on http://{addr}")?;
        out.flush()?;
        drop(out);
        tracing::info!(%addr, "listening");
        server.run(shutdown_signal()).await;
        Ok(ExitCode::SUCCESS)
    }))
}

/// Insert a token and its audit record; returns the token string.
fn insert_token(
    conn: &mut rusqlite::Connection,
    name: &str,
    role: Role,
    expires_in_days: Option<u32>,
    action_detail: &str,
) -> anyhow::Result<(String, TokenRecord)> {
    let now = now_unix();
    for _ in 0..5 {
        let id = crate::auth::new_token_id()?;
        let minted = crate::auth::mint(&id)?;
        let rec = TokenRecord {
            id: minted.id.clone(),
            secret_hash: minted.secret_hash,
            name: name.to_owned(),
            role,
            created_at: now,
            created_by: "cli".to_owned(),
            expires_at: expires_in_days.map(|d| now + i64::from(d) * 86_400),
            revoked_at: None,
            last_used_at: None,
        };
        let entry = AuditEntry::new("cli", "token.create", &rec.id, AuditOutcome::Success)
            .detail(format!("{name} ({}); {action_detail}", role.as_str()));
        let inserted = crate::db::write_tx(conn, |tx| {
            if crate::db::tokens::exists(tx, &rec.id)? {
                return Ok(false);
            }
            crate::db::tokens::insert(tx, &rec)?;
            crate::db::audit::append(tx, &entry)?;
            Ok(true)
        })?;
        if inserted {
            return Ok((minted.token.as_str().to_owned(), rec));
        }
    }
    bail!("could not allocate a token id")
}

fn init(config: Option<&Path>, force: bool, data_dir: Option<&Path>) -> anyhow::Result<ExitCode> {
    let loc = config::locate(config, &process_env)?;
    let config_dir = loc
        .path
        .parent()
        .context("the configuration path has no directory")?
        .to_path_buf();
    if loc.path.exists() {
        eprintln!(
            "Using the existing configuration {} (it is not modified).",
            loc.path.display()
        );
    } else {
        let data = match data_dir {
            Some(d) => config::expand_tilde(&d.to_string_lossy(), &process_env)?,
            None => match process_env("H2S_DATA_DIR").filter(|v| !v.is_empty()) {
                Some(d) => config::expand_tilde(&d, &process_env)?,
                None => config::default_data_dir(&process_env)?,
            },
        };
        if !data.is_absolute() {
            bail!("the data directory {} must be absolute", data.display());
        }
        crate::util::ensure_private_dir(&config_dir)
            .with_context(|| format!("creating {}", config_dir.display()))?;
        let text = config::render_initial(&data)?;
        crate::util::write_private_new(&loc.path, text.as_bytes())
            .with_context(|| format!("writing {}", loc.path.display()))?;
        eprintln!("Wrote {}.", loc.path.display());
    }
    let cfg = Config::load(Some(&loc.path), &process_env)?;
    cfg.paths
        .ensure()
        .context("creating the data directories")?;
    let mut conn = crate::db::open_connection(&cfg.paths.db_path)?;
    let admins = crate::db::tokens::count_active_admins(&conn, now_unix())?;
    if admins > 0 && !force {
        bail!(
            "already initialised: an admin token exists in {}. Create more with `h2studio token create`, or pass --force for another bootstrap token",
            cfg.paths.db_path.display()
        );
    }
    let (token, rec) = insert_token(
        &mut conn,
        BOOTSTRAP_TOKEN_NAME,
        Role::Admin,
        None,
        "bootstrap token from h2studio init",
    )?;
    // The token is in the database now, so print it before anything else can fail: a second
    // `init` refuses, and a token nobody saw would lock the administrator out.
    let mut out = std::io::stdout().lock();
    writeln!(out, "{token}")?;
    out.flush()?;
    // The saved copy is a convenience. The configuration directory may be read-only (a
    // config file bind-mounted into a container, as in deploy/compose.yaml).
    let token_file = config_dir.join(ADMIN_TOKEN_FILE);
    match crate::util::write_private_replace(&token_file, format!("{token}\n").as_bytes()) {
        Ok(()) => eprintln!(
            "Created admin token {} (also saved to {}, mode 0600).\nStart the server with `h2studio serve` and sign in with this token.",
            rec.id,
            token_file.display()
        ),
        Err(e) => eprintln!(
            "Created admin token {}. It is shown only above: it could not be saved to {} ({e}).\nStart the server with `h2studio serve` and sign in with this token.",
            rec.id,
            token_file.display()
        ),
    }
    Ok(ExitCode::SUCCESS)
}

fn open_db(cfg: &Config) -> anyhow::Result<rusqlite::Connection> {
    cfg.paths
        .ensure()
        .context("creating the data directories")?;
    Ok(crate::db::open_connection(&cfg.paths.db_path)?)
}

fn token_create(
    config: Option<&Path>,
    name: &str,
    role: Role,
    expires_in_days: Option<u32>,
) -> anyhow::Result<ExitCode> {
    match h2s_core::names::token_name(name) {
        Ok(()) => {}
        Err(h2s_core::CoreError::Invalid { problem, .. }) => bail!("--name: {problem}"),
        Err(e) => bail!("--name: {e}"),
    }
    if let Some(d) = expires_in_days
        && !(1..=3650).contains(&d)
    {
        bail!("--expires-in-days must be between 1 and 3650");
    }
    let cfg = load(config)?;
    let mut conn = open_db(&cfg)?;
    let (token, rec) = insert_token(
        &mut conn,
        name,
        role,
        expires_in_days,
        "created with the CLI",
    )?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{token}")?;
    out.flush()?;
    eprintln!(
        "Created {} token {} ({}){}.",
        role.as_str(),
        rec.id,
        rec.name,
        rec.expires_at
            .map(|e| format!(", expires {}", crate::util::rfc3339(e)))
            .unwrap_or_default()
    );
    Ok(ExitCode::SUCCESS)
}

fn token_list(config: Option<&Path>, json: bool) -> anyhow::Result<ExitCode> {
    let cfg = load(config)?;
    let conn = open_db(&cfg)?;
    let tokens: Vec<_> = crate::db::tokens::list(&conn)?
        .iter()
        .map(TokenRecord::info)
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&tokens)?);
    } else {
        println!(
            "{:<10} {:<9} {:<8} {:<21} {:<21} NAME",
            "ID", "ROLE", "STATE", "CREATED", "EXPIRES"
        );
        for t in tokens {
            println!(
                "{:<10} {:<9} {:<8} {:<21} {:<21} {}",
                t.id,
                t.role.as_str(),
                if t.revoked { "revoked" } else { "active" },
                t.created_at,
                t.expires_at.as_deref().unwrap_or("never"),
                t.name
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn token_revoke(config: Option<&Path>, id: &str) -> anyhow::Result<ExitCode> {
    let cfg = load(config)?;
    let mut conn = open_db(&cfg)?;
    let now = now_unix();
    let entry = AuditEntry::new("cli", "token.revoke", id, AuditOutcome::Success);
    let revoked = crate::db::write_tx(&mut conn, |tx| {
        let r = crate::db::tokens::revoke(tx, id, now)?;
        if r {
            crate::db::audit::append(tx, &entry)?;
        }
        Ok(r)
    })?;
    if !revoked {
        bail!("no active token with id {id}");
    }
    let admins = crate::db::tokens::count_active_admins(&conn, now)?;
    eprintln!("Revoked token {id}.");
    if admins == 0 {
        eprintln!(
            "Warning: no active admin token remains; create one with `h2studio token create --role admin`."
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn doctor(config: Option<&Path>, json: bool) -> anyhow::Result<ExitCode> {
    let cfg = load(config)?;
    let rt = runtime()?;
    let report = rt.block_on(async {
        let uses_vertex = if cfg.paths.db_path.exists() {
            crate::db::open_connection(&cfg.paths.db_path)
                .ok()
                .and_then(|c| crate::db::profiles::list(&c).ok())
                .is_some_and(|ps| ps.iter().any(|p| p.kind == h2s_api::ProfileKind::Vertex))
        } else {
            false
        };
        crate::doctor::fresh_report(crate::doctor::ContextFacts {
            harness2: h2s_core::install::read_status(
                &cfg.paths.harness2_home,
                cfg.paths.harness2_managed,
            ),
            isolation: cfg.isolation(),
            data_dir: cfg.paths.data_dir.clone(),
            uses_vertex,
            container: (cfg.isolation() == h2s_api::Isolation::Container)
                .then(|| cfg.file.isolation.container.clone()),
        })
        .await
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for c in &report.checks {
            let level = serde_json::to_value(c.level)?
                .as_str()
                .unwrap_or("?")
                .to_uppercase();
            println!("{level:<5} {:<18} {}", c.id, c.detail);
            if let Some(r) = &c.remedy {
                println!("      remedy: {r}");
            }
        }
        if report.checks.is_empty() {
            println!("no checks were evaluated by this build");
        }
        println!("{}", if report.ok { "OK" } else { "problems found" });
    }
    Ok(if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn config_check(config: Option<&Path>) -> anyhow::Result<ExitCode> {
    let cfg = load(config)?;
    println!("configuration OK");
    for (k, v) in cfg.describe_paths() {
        println!("  {k}: {v}");
    }
    println!("  listen: {}", cfg.listen);
    println!("  public url: {}", cfg.public_url);
    for w in crate::telemetry::environment_warnings(&process_env)
        .into_iter()
        .chain(cfg.warnings())
    {
        eprintln!("warning: {w}");
    }
    if cfg.file.auth.mode == h2s_api::AuthMode::Proxy {
        let name = &cfg.file.auth.proxy.secret_env;
        if process_env(name).is_none_or(|v| v.is_empty()) {
            eprintln!("warning: auth.mode = \"proxy\" but {name} is not set in this environment");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn audit_verify(config: Option<&Path>) -> anyhow::Result<ExitCode> {
    let cfg = load(config)?;
    if !cfg.paths.db_path.exists() {
        bail!("no database at {}", cfg.paths.db_path.display());
    }
    let conn = crate::db::open_connection(&cfg.paths.db_path)?;
    let v = crate::db::audit::verify(&conn)?;
    let head = crate::db::audit::head(&conn)?;
    if v.ok {
        match head {
            Some((seq, hash)) => {
                println!("audit chain OK: {} entries; head {seq} {hash}", v.entries);
            }
            None => println!("audit chain OK: empty"),
        }
        Ok(ExitCode::SUCCESS)
    } else {
        println!(
            "audit chain BROKEN: first bad record {} of {} entries",
            v.first_bad_seq.unwrap_or(0),
            v.entries
        );
        Ok(ExitCode::FAILURE)
    }
}

fn egress_proxy(config: Option<&Path>) -> anyhow::Result<ExitCode> {
    // The proxy may share a user (and an environment with secrets) with the server.
    let dumpable = crate::util::make_non_dumpable();
    let cfg = load(config)?;
    let rt = runtime()?;
    rt.block_on(Box::pin(async move {
        let _telemetry = crate::telemetry::init(&cfg).map_err(anyhow::Error::msg)?;
        if let Err(e) = dumpable {
            tracing::warn!(error = %e, "could not make the egress proxy non-dumpable");
        }
        let listener = tokio::net::TcpListener::bind(cfg.egress_listen)
            .await
            .with_context(|| format!("binding {}", cfg.egress_listen))?;
        let addr = listener.local_addr()?;
        for r in &cfg.egress_allow {
            tracing::info!(
                host = %if r.wildcard { format!("*.{}", r.host) } else { r.host.clone() },
                allow_private = r.allow_private,
                "egress allowlist entry"
            );
        }
        if cfg.egress_allow.is_empty() {
            tracing::warn!("the egress allowlist is empty: every request will be refused");
        }
        let mut out = std::io::stdout().lock();
        writeln!(out, "h2studio egress proxy listening on {addr}")?;
        out.flush()?;
        drop(out);
        let (tx, rx) = tokio::sync::watch::channel(false);
        let serve = tokio::spawn(crate::egress::serve(
            listener,
            crate::egress::ProxySettings::new(cfg.egress_allow.clone()),
            rx,
        ));
        shutdown_signal().await;
        let _ = tx.send(true);
        let _ = serve.await;
        Ok(ExitCode::SUCCESS)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn config_flag_is_global() {
        let cli = Cli::try_parse_from([
            "h2studio",
            "token",
            "create",
            "--name",
            "ci",
            "--role",
            "admin",
            "--config",
            "/tmp/x.toml",
        ])
        .unwrap();
        assert_eq!(cli.config.as_deref(), Some(Path::new("/tmp/x.toml")));
        assert!(
            Cli::try_parse_from([
                "h2studio", "token", "create", "--name", "x", "--role", "root"
            ])
            .is_err()
        );
    }
}
