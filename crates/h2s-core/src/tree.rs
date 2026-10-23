//! Harness component trees (c1..c8): read, classify, validate, diff, fingerprint.
//!
//! A port of upstream `harness2/core/tree.py` and the two `SurfaceSpec`s in
//! `harness2/substrate/{opencode,codex}.py` at [`crate::TESTED_HARNESS2_COMMIT`].
//! Validation messages are upstream's wording, because harness2 feeds them back to the
//! improver verbatim and users will see the same sentences in its logs.
//! (One deliberate gap: upstream also `compile()`s Python scripts; that needs a Python
//! interpreter and is reported by harness2 itself when it matters.)

use std::collections::BTreeMap;

use h2s_api::{Census, Component, HarnessDiff, Surface, ValidationReport};

use crate::{CoreError, Result};

/// relative path -> file content.
pub type Tree = BTreeMap<String, String>;

/// Always-on character cap: OpenCode 24,000; Codex 8,000.
#[must_use]
pub fn always_on_max(surface: Surface) -> u32 {
    match surface {
        Surface::Opencode => 24_000,
        Surface::Codex => 8_000,
    }
}

/// The always-on files of a surface, in upstream order.
#[must_use]
pub fn always_on_files(surface: Surface) -> &'static [&'static str] {
    match surface {
        Surface::Opencode => &[
            "systemprompt.md",
            "guardrails.md",
            "LongTermMEMORY.md",
            "tool_descriptions/tool_guidance.md",
        ],
        Surface::Codex => &["AGENTS.md"],
    }
}

/// Which component a path belongs to, or `None` when it is off the editable surface
/// (README stubs are off-surface).
#[must_use]
pub fn classify(surface: Surface, path: &str) -> Option<Component> {
    let _ = (surface, path);
    None
}

/// `AGENTS.md` at the root means Codex; otherwise OpenCode.
#[must_use]
pub fn detect_surface(tree: &Tree) -> Surface {
    if tree.contains_key("AGENTS.md") {
        Surface::Codex
    } else {
        Surface::Opencode
    }
}

/// Read every on-surface file under `dir` (README stubs and `__pycache__` excluded).
/// Returns the tree and the off-surface paths found. Never follows symlinks.
pub fn read_tree(dir: &cap_std::fs::Dir, surface: Surface) -> Result<(Tree, Vec<String>)> {
    let _ = (dir, surface);
    Err(CoreError::Unimplemented("tree::read_tree"))
}

/// Component census (upstream `validate()`'s return value, plus the cap).
#[must_use]
pub fn census(surface: Surface, tree: &Tree) -> Census {
    let _ = (surface, tree);
    Census::default()
}

/// All of upstream's checks: always-on cap, empty always-on (Codex), skill frontmatter
/// (`description:`, `name:` rule per surface), untriggered skills, sub-agent rules,
/// hooks schema (Codex), scripts invoked but not shipped. Every problem is collected.
#[must_use]
pub fn validate(surface: Surface, tree: &Tree, allow_subagents: bool) -> ValidationReport {
    let _ = (surface, tree, allow_subagents);
    ValidationReport {
        ok: false,
        errors: vec!["not implemented".into()],
        off_surface: Vec::new(),
        census: Census::default(),
    }
}

/// File-by-file diff with unified hunks (3 lines of context).
#[must_use]
pub fn diff(
    surface: Surface,
    from_label: &str,
    from: &Tree,
    to_label: &str,
    to: &Tree,
) -> HarnessDiff {
    let _ = (surface, from, to);
    HarnessDiff {
        from: from_label.to_owned(),
        to: to_label.to_owned(),
        files: Vec::new(),
        always_on_char_delta: 0,
    }
}

/// Upstream `runmark.fingerprint`: SHA-1 over every regular file under `dir` in sorted
/// relative-path order (`path\0bytes\0`), skipping `.adapt_complete`, `__pycache__`,
/// `*.pyc` and `.DS_Store`; first 16 hex digits. An empty or missing dir is `absent`.
pub fn fingerprint(dir: &cap_std::fs::Dir) -> Result<String> {
    let _ = dir;
    Err(CoreError::Unimplemented("tree::fingerprint"))
}
