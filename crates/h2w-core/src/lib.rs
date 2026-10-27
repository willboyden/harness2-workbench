//! Harness² Workbench domain logic, free of any server or UI framework.
//!
//! **The public signatures in this crate are a stable interface** (docs/ARCHITECTURE.md §6)
//! that `h2w-daemon` codes against. Changes are additive only and are recorded in
//! `docs/ARCHITECTURE.md` §16.
//!
//! What lives here:
//! - [`cli`]: the exact argv for every harness2 command, from validated API types.
//! - [`parse`]: typed results from harness2's stdout and the files it writes.
//! - [`results`]: a read-only, traversal-proof view of an experiment directory.
//! - [`tree`]: harness component trees, upstream's validation rules, diffs, fingerprints.
//! - [`report`], [`estimate`], [`export`], [`manifest`]: analysis on top of results.
//! - [`env`], [`redact`], [`isolation`], [`install`], [`doctor`], [`bench`]: what the server
//!   needs to run jobs safely.
//! - [`bundle`]: offline install bundles of the managed harness2 installation.
//!
//! Nothing here spawns processes or opens sockets; filesystem access goes through
//! `cap_std` handles so a path from a request or a results file cannot leave its root.
//! The exception is [`bundle`], which works on paths an administrator gives the command line
//! (never a request) and checks every name in an archive before it becomes a path.

#![forbid(unsafe_code)]

pub mod bench;
pub mod bundle;
pub mod cli;
pub mod doctor;
pub mod env;
pub mod error;
pub mod estimate;
pub mod export;
pub mod install;
pub mod isolation;
pub mod manifest;
pub mod names;
pub mod namespace;
pub mod parse;
pub mod redact;
pub mod report;
pub mod results;
pub mod tree;

mod lite;
mod pyfmt;
mod pyjson;
mod safefs;

#[cfg(test)]
mod golden_tests;

pub use error::{CoreError, Result};

/// The google-research/harness2 commit this release is tested against (2026-10-14,
/// "Fix arXiv link", tree edc756a4d03a2826a248115482084a176e60e751).
pub const TESTED_HARNESS2_COMMIT: &str = "1cfe254e8d25b15f37048c3730134686abe60335";

/// Latest upload time harness2's Python dependencies are resolved against at install
/// (`uv pip install --exclude-newer`): the commit time of [`TESTED_HARNESS2_COMMIT`]
/// (2026-10-13T21:22:49-05:00) in UTC. harness2 ships no lock file; with this cutoff an
/// install never picks up a package released after the tested commit, and the resolution
/// does not drift with new uploads.
pub const TESTED_HARNESS2_DEPS_CUTOFF: &str = "2026-10-14T02:22:49Z";

/// Upstream repository harness2 is installed from.
pub const HARNESS2_REPO_URL: &str = "https://github.com/google-research/harness2.git";

/// Python version harness2's benchmarks require (`requires-python = ">=3.12,<3.14"`).
pub const HARNESS2_PYTHON: &str = "3.12";
