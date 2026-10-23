//! The exact argv of every harness2 command, built only from validated API types.
//!
//! The server resolves [`Program`] to an executable inside the harness2 virtualenv
//! (`<home>/.venv/bin/<bin_name>`) and runs it without a shell. Every free-text value is
//! validated before it reaches argv, and no value can start with `-` (so none can be read
//! as an option).

use h2s_api::{
    BridgeParams, Cell, DemoParams, JudgeParams, ReportParams, RunOptions, SetupParams, SplitParams,
};

use crate::{CoreError, Result};

/// Which harness2 entry point to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Program {
    /// `harness2` (refinement sweep; with `--dry-run`, the preview).
    Harness2,
    Setup,
    Check,
    Judge,
    Report,
    Demo,
    Bridge,
    /// `python scripts/split_tasks.py` run with the venv's interpreter, cwd = harness2 home.
    SplitScript,
}

impl Program {
    /// Executable name in `<venv>/bin/` (`python` for [`Program::SplitScript`]).
    #[must_use]
    pub fn bin_name(self) -> &'static str {
        match self {
            Program::Harness2 => "harness2",
            Program::Setup => "harness2-setup",
            Program::Check => "harness2-check",
            Program::Judge => "harness2-judge",
            Program::Report => "harness2-report",
            Program::Demo => "harness2-demo",
            Program::Bridge => "harness2-bridge",
            Program::SplitScript => "python",
        }
    }
}

/// A command ready to run: program + arguments (no shell, no environment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: Program,
    pub args: Vec<String>,
}

/// `harness2-demo --output <output_dir> --k <k> --mode <mode>`.
/// `output_dir` is the experiment directory as the process will see it.
pub fn demo(p: &DemoParams, output_dir: &str) -> Result<Invocation> {
    let _ = (p, output_dir);
    Err(CoreError::Unimplemented("cli::demo"))
}

/// `harness2-setup <bench> [--subset S] [--substrate X] [--dry-run]`.
pub fn setup(p: &SetupParams) -> Result<Invocation> {
    let _ = p;
    Err(CoreError::Unimplemented("cli::setup"))
}

/// `harness2-check --bench B [--subset S] [--substrate X]`.
pub fn check(cell: &Cell) -> Result<Invocation> {
    let _ = cell;
    Err(CoreError::Unimplemented("cli::check"))
}

/// `harness2 --bench ... --k K [--seed N] --modes M --split S [--split-file F] --workers W
/// [--only-domains a,b] [--max-tasks N] --dry-run`.
pub fn preview(cell: &Cell, options: &RunOptions, split_file: Option<&str>) -> Result<Invocation> {
    let _ = (cell, options, split_file);
    Err(CoreError::Unimplemented("cli::preview"))
}

/// The same as [`preview`] without `--dry-run`.
pub fn run(cell: &Cell, options: &RunOptions, split_file: Option<&str>) -> Result<Invocation> {
    let _ = (cell, options, split_file);
    Err(CoreError::Unimplemented("cli::run"))
}

/// `harness2-judge --bench ... --k K [--seed N] --modes M [--bases]`.
pub fn judge(p: &JudgeParams) -> Result<Invocation> {
    let _ = p;
    Err(CoreError::Unimplemented("cli::judge"))
}

/// `harness2-report --bench ... --k 1,2 [--seed N] --modes M`.
pub fn report(p: &ReportParams) -> Result<Invocation> {
    let _ = p;
    Err(CoreError::Unimplemented("cli::report"))
}

/// `python scripts/split_tasks.py --bench B [--subset S] --seed N --train-fraction F
/// --output <output_path>`.
pub fn split(p: &SplitParams, output_path: &str) -> Result<Invocation> {
    let _ = (p, output_path);
    Err(CoreError::Unimplemented("cli::split"))
}

/// `harness2-bridge --model M [--host 0.0.0.0] [--port N]`.
pub fn bridge(p: &BridgeParams, model: &str) -> Result<Invocation> {
    let _ = (p, model);
    Err(CoreError::Unimplemented("cli::bridge"))
}
