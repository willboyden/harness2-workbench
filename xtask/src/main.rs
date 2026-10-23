//! Build orchestration for Harness² Studio: `cargo xtask <command>`.
//!
//! Commands run other programs with explicit argv (no shell) and stop at the first failure,
//! printing the command that failed.

// A build tool reports to the terminal; the workspace-wide print lints target the server.
#![allow(clippy::print_stderr, clippy::print_stdout)]

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

mod cooldown;

#[derive(Parser)]
#[command(name = "xtask", about = "Harness² Studio build tasks")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build the web UI into dist/web (WASM + wasm-bindgen + static assets).
    Web {
        /// Debug build (faster, larger, with debug assertions).
        #[arg(long)]
        dev: bool,
    },
    /// Build the web UI, then release binaries of the server and the MCP server.
    Dist,
    /// Check every locked crate version against the dependency cooldown.
    Cooldown {
        /// Minimum age in days.
        #[arg(long, default_value_t = 14)]
        days: i64,
    },
    /// Run every quality gate: fmt, clippy (native + wasm), tests, cargo-deny, cooldown.
    Check,
    /// Point git at the repository's hooks in .githooks/.
    Hooks,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = workspace_root()?;
    match cli.command {
        Cmd::Web { dev } => build_web(&root, dev),
        Cmd::Dist => {
            build_web(&root, false)?;
            run(
                &root,
                "cargo",
                &["build", "--release", "-p", "h2s-daemon", "-p", "h2s-mcp"],
            )
        }
        Cmd::Cooldown { days } => cooldown::check(&root, days),
        Cmd::Check => check(&root),
        Cmd::Hooks => run(&root, "git", &["config", "core.hooksPath", ".githooks"]),
    }
}

fn workspace_root() -> Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .context("xtask must live one level below the workspace root")
}

fn run(cwd: &Path, program: &str, args: &[&str]) -> Result<()> {
    eprintln!("$ {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .with_context(|| format!("could not start {program}"))?;
    if !status.success() {
        bail!("`{program} {}` failed with {status}", args.join(" "));
    }
    Ok(())
}

/// Build `h2s-web` for wasm32, generate the JS bindings, and assemble `dist/web`.
fn build_web(root: &Path, dev: bool) -> Result<()> {
    let profile = if dev { "dev" } else { "release-web" };
    let profile_dir = if dev { "debug" } else { "release-web" };
    run(
        root,
        "cargo",
        &[
            "build",
            "-p",
            "h2s-web",
            "--lib",
            "--target",
            "wasm32-unknown-unknown",
            "--profile",
            profile,
        ],
    )?;
    let wasm = root
        .join("target/wasm32-unknown-unknown")
        .join(profile_dir)
        .join("h2s_web.wasm");
    if !wasm.is_file() {
        bail!("expected {} after the build", wasm.display());
    }
    let dist = root.join("dist/web");
    if dist.exists() {
        std::fs::remove_dir_all(&dist).with_context(|| format!("removing {}", dist.display()))?;
    }
    let pkg = dist.join("pkg");
    std::fs::create_dir_all(&pkg)?;
    let wasm_s = wasm.to_string_lossy().into_owned();
    let pkg_s = pkg.to_string_lossy().into_owned();
    run(
        root,
        "wasm-bindgen",
        &[
            "--target",
            "web",
            "--no-typescript",
            "--out-dir",
            &pkg_s,
            &wasm_s,
        ],
    )?;
    let assets = root.join("web");
    if !assets.is_dir() {
        bail!("missing static assets directory {}", assets.display());
    }
    copy_dir(&assets, &dist)?;
    eprintln!("web UI built into {}", dist.display());
    Ok(())
}

/// Copy regular files from `src` into `dst` recursively (symlinks are refused).
fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if kind.is_symlink() {
            bail!("refusing to copy symlink {}", entry.path().display());
        } else if kind.is_dir() {
            std::fs::create_dir_all(&to)?;
            copy_dir(&entry.path(), &to)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

fn check(root: &Path) -> Result<()> {
    run(root, "cargo", &["fmt", "--all", "--", "--check"])?;
    run(
        root,
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run(
        root,
        "cargo",
        &[
            "clippy",
            "-p",
            "h2s-web",
            "-p",
            "h2s-api",
            "--target",
            "wasm32-unknown-unknown",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run(root, "cargo", &["test", "--workspace"])?;
    run(root, "cargo", &["deny", "check"])?;
    cooldown::check(root, 14)?;
    eprintln!("all gates passed");
    Ok(())
}
