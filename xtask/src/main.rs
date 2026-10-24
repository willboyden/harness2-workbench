//! Build orchestration for Harness² Workbench: `cargo xtask <command>`.
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
mod docs;

#[derive(Parser)]
#[command(name = "xtask", about = "Harness² Workbench build tasks")]
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
    /// Check every relative link and #anchor in the Markdown files (fails on a broken one).
    Docs,
    /// Run every quality gate: fmt, docs links, clippy (native + wasm), tests, cargo-deny, cooldown.
    Check,
    /// Point git at the repository's hooks in .githooks/.
    Hooks,
    /// End-to-end suites: real server + real harness2 demo + headless Firefox + MCP client.
    E2e {
        /// harness2 checkout with its .venv (default: $H2W_E2E_HARNESS2_HOME).
        #[arg(long)]
        harness2_home: Option<String>,
        /// Run one suite: server_demo, browser or mcp (default: all).
        #[arg(long)]
        suite: Option<String>,
        /// Run only test names containing this filter (test names, not file names).
        #[arg(long)]
        filter: Option<String>,
    },
}

/// The end-to-end suites (`crates/h2w-e2e/tests/<name>.rs`).
const E2E_SUITES: &[&str] = &["server_demo", "browser", "mcp"];

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
                &["build", "--release", "-p", "h2w-daemon", "-p", "h2w-mcp"],
            )
        }
        Cmd::Cooldown { days } => cooldown::check(&root, days),
        Cmd::Docs => docs::check(&root),
        Cmd::Check => check(&root),
        Cmd::Hooks => run(&root, "git", &["config", "core.hooksPath", ".githooks"]),
        Cmd::E2e {
            harness2_home,
            suite,
            filter,
        } => e2e(&root, harness2_home, suite.as_deref(), filter.as_deref()),
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

/// Build `h2w-web` for wasm32, generate the JS bindings, and assemble `dist/web`.
fn build_web(root: &Path, dev: bool) -> Result<()> {
    let profile = if dev { "dev" } else { "release-web" };
    let profile_dir = if dev { "debug" } else { "release-web" };
    run(
        root,
        "cargo",
        &[
            "build",
            "-p",
            "h2w-web",
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
        .join("h2w_web.wasm");
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
    // rust-embed decides when the server is compiled whether `dist/web` exists, so a server
    // compiled without it keeps the "UI not built" page. Marking the embedding module changed
    // makes the next server build, debug or release, embed the UI just built.
    let embed = root.join("crates/h2w-daemon/src/http/static_ui.rs");
    std::fs::File::options()
        .write(true)
        .open(&embed)
        .and_then(|f| f.set_modified(std::time::SystemTime::now()))
        .with_context(|| format!("marking {} changed", embed.display()))?;
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
    docs::check(root)?;
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
            "h2w-web",
            "-p",
            "h2w-api",
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

/// Build the UI (embedded by the server at compile time), then the binaries, then run the
/// ignored end-to-end tests against them. Zero tests run is a failure: a filter that
/// matches nothing must not report success.
fn e2e(
    root: &Path,
    harness2_home: Option<String>,
    suite: Option<&str>,
    filter: Option<&str>,
) -> Result<()> {
    if let Some(s) = suite
        && !E2E_SUITES.contains(&s)
    {
        bail!("unknown suite {s:?}; use one of {}", E2E_SUITES.join(", "));
    }
    let home = harness2_home
        .or_else(|| std::env::var("H2W_E2E_HARNESS2_HOME").ok())
        .context("pass --harness2-home <checkout with .venv> or set H2W_E2E_HARNESS2_HOME")?;
    if !Path::new(&home).join(".venv/bin/harness2-demo").is_file() {
        bail!("{home} has no .venv/bin/harness2-demo; see crates/h2w-e2e/README.md");
    }
    build_web(root, true)?;
    run(
        root,
        "cargo",
        &["build", "--locked", "-p", "h2w-daemon", "-p", "h2w-mcp"],
    )?;
    let bin_dir = root.join("target/debug");
    let mut args = vec!["test", "--locked", "-p", "h2w-e2e"];
    if let Some(s) = suite {
        args.extend(["--test", s]);
    }
    args.push("--");
    if let Some(f) = filter {
        args.push(f);
    }
    args.extend(["--ignored", "--test-threads=2"]);
    eprintln!("$ cargo {}", args.join(" "));
    let mut child = Command::new("cargo")
        .args(&args)
        .current_dir(root)
        .env("H2W_E2E_BIN_DIR", &bin_dir)
        .env("H2W_E2E_HARNESS2_HOME", &home)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("could not start cargo test")?;
    let stdout = child.stdout.take().context("no stdout from cargo test")?;
    let mut passed = 0;
    for line in std::io::BufRead::lines(std::io::BufReader::new(stdout)) {
        let line = line.context("reading cargo test output")?;
        println!("{line}");
        passed += passed_count(&line).unwrap_or(0);
    }
    let status = child.wait().context("waiting for cargo test")?;
    if !status.success() {
        bail!("end-to-end tests failed ({status}); screenshots are in target/e2e-artifacts/");
    }
    if passed == 0 {
        bail!(
            "no end-to-end test ran: --filter matches test names (e.g. \
             login_browse_the_demo_and_diff_a_harness); use --suite for a file"
        );
    }
    eprintln!("{passed} end-to-end tests passed; screenshots in target/e2e-artifacts/");
    Ok(())
}

/// `N` from libtest's summary line `test result: ok. N passed; ...`.
fn passed_count(line: &str) -> Option<u64> {
    line.strip_prefix("test result: ")?
        .split_once(". ")?
        .1
        .split(' ')
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::passed_count;

    #[test]
    fn libtest_summaries_are_counted() {
        let ok = "test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.94s";
        assert_eq!(passed_count(ok), Some(4));
        let none = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out";
        assert_eq!(passed_count(none), Some(0));
        assert_eq!(
            passed_count("test result: FAILED. 1 passed; 1 failed; 0 ignored"),
            Some(1)
        );
        assert_eq!(passed_count("test login ... ok"), None);
    }
}
