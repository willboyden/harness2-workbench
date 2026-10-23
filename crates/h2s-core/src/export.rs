//! Turn an adopted harness into files a user can drop into their own project.
//!
//! - `raw`: the tree as harness2 stores it.
//! - `opencode`: `AGENTS.md` = the always-on files joined in upstream order, skills under
//!   `.opencode/skills/<name>/SKILL.md`, sub-agents under `.opencode/agent/<name>.md`,
//!   plugins under `.opencode/plugin/<name>.ts`, scripts under `scripts/`.
//! - `codex`: `AGENTS.md`, `.agents/skills/`, `.codex/hooks.json`, `scripts/`, sub-agents
//!   as `.codex/agents/<name>.md` with a note that Codex has no native sub-agent loader.
//!
//! Export never changes file contents except to join the always-on files.

use h2s_api::{ExportBundle, ExportTarget, HarnessTree};

use crate::{CoreError, Result};

/// Build the bundle for `target`. Fails when the tree's surface cannot express `target`
/// faithfully (e.g. OpenCode plugins exported to Codex) unless the problem is only a note.
pub fn export(tree: &HarnessTree, target: ExportTarget) -> Result<ExportBundle> {
    let _ = (tree, target);
    Err(CoreError::Unimplemented("export::export"))
}

/// A deterministic `.tar.gz` of the bundle (sorted paths, mode 0644, mtime 0, top-level
/// directory `harness-<label>/`).
pub fn tar_gz(bundle: &ExportBundle) -> Result<Vec<u8>> {
    let _ = bundle;
    Err(CoreError::Unimplemented("export::tar_gz"))
}
