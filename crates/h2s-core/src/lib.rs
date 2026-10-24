//! Harness² Studio domain logic, free of any server or UI framework.
//!
//! **The public signatures in this crate are a LOCKED interface** (docs/ARCHITECTURE.md §6).
//! `h2s-daemon` codes against them while they are implemented. Changes are additive only and
//! go through the coordinator.
//!
//! What lives here:
//! - [`cli`]: the exact argv for every harness2 command, from validated API types.
//! - [`parse`]: typed results from harness2's stdout and the files it writes.
//! - [`results`]: a read-only, traversal-proof view of an experiment directory.
//! - [`tree`]: harness component trees, upstream's validation rules, diffs, fingerprints.
//! - [`report`], [`estimate`], [`export`], [`manifest`]: analysis on top of results.
//! - [`env`], [`redact`], [`isolation`], [`install`], [`doctor`], [`bench`]: what the server
//!   needs to run jobs safely.
//!
//! Nothing here spawns processes or opens sockets; filesystem access goes through
//! `cap_std` handles so a path from a request or a results file cannot leave its root.

#![forbid(unsafe_code)]

pub mod bench;
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

/// Upstream repository harness2 is installed from.
pub const HARNESS2_REPO_URL: &str = "https://github.com/google-research/harness2.git";

/// Python version harness2's benchmarks require (`requires-python = ">=3.12,<3.14"`).
pub const HARNESS2_PYTHON: &str = "3.12";
