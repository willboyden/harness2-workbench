//! Harness component trees (c1..c8): read, classify, validate, diff, fingerprint.
//!
//! A port of upstream `harness2/core/tree.py` and the two `SurfaceSpec`s in
//! `harness2/substrate/{opencode,codex}.py` at [`crate::TESTED_HARNESS2_COMMIT`].
//! Validation messages are upstream's wording, because harness2 feeds them back to the
//! improver verbatim and users will see the same sentences in its logs.
//! (One deliberate gap: upstream also `compile()`s Python scripts; that needs a Python
//! interpreter and is reported by harness2 itself when it matters.)

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::LazyLock;
use std::time::Duration;

use h2s_api::{Census, ChangeKind, Component, FileDiff, HarnessDiff, Surface, ValidationReport};
use regex::Regex;
use sha1::{Digest, Sha1};

use crate::pyfmt;
use crate::safefs;
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

/// The data on upstream's `SurfaceSpec` that this port needs.
struct Spec {
    always_on: &'static [&'static str],
    cap_reason: &'static str,
    skill_dir: &'static str,
    /// `must_match` (OpenCode) vs `overrides_dir` (Codex).
    name_must_match: bool,
    /// Upstream `trigger_hint` with `{name}` already split out: `(before, middle, after)`.
    trigger_hint: fn(&str) -> String,
    subagent_requires_mode: bool,
    plugin_dir: Option<&'static str>,
    hooks_file: Option<&'static str>,
    hook_events: &'static [&'static str],
    reject_empty_always_on: bool,
    stubs: &'static [&'static str],
}

fn oc_hint(name: &str) -> String {
    format!(
        "Fix it by adding a trigger line naming `{name}` to systemprompt.md or guardrails.md, \
         e.g. \"When <the situation this skill handles>, load the `{name}` skill.\""
    )
}

fn cx_hint(name: &str) -> String {
    format!(
        "Fix it by adding a trigger line naming `{name}` to AGENTS.md, e.g. \"When <the \
         situation this skill handles>, read .agents/skills/{name}/SKILL.md and follow it.\""
    )
}

static OPENCODE: Spec = Spec {
    always_on: &[
        "systemprompt.md",
        "guardrails.md",
        "LongTermMEMORY.md",
        "tool_descriptions/tool_guidance.md",
    ],
    cap_reason: "every token here competes with the agent's own output budget",
    skill_dir: "skills",
    name_must_match: true,
    trigger_hint: oc_hint,
    subagent_requires_mode: true,
    plugin_dir: Some("plugin"),
    hooks_file: None,
    hook_events: &[],
    reject_empty_always_on: false,
    stubs: &[
        "scripts/README.md",
        "sub_agents/README.md",
        "plugin/README.md",
        "skills/README.md",
    ],
};

static CODEX: Spec = Spec {
    always_on: &["AGENTS.md"],
    cap_reason: "AGENTS.md is injected whole into EVERY turn, and codex additionally \
                 truncates it silently at 32,768 bytes",
    skill_dir: ".agents/skills",
    name_must_match: false,
    trigger_hint: cx_hint,
    subagent_requires_mode: false,
    plugin_dir: None,
    hooks_file: Some(".codex/hooks.json"),
    hook_events: &[
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "PreCompact",
        "PostCompact",
        "SessionStart",
        "SubagentStart",
        "SubagentStop",
        "UserPromptSubmit",
        "Stop",
    ],
    reject_empty_always_on: true,
    stubs: &[
        "scripts/README.md",
        "sub_agents/README.md",
        ".codex/README.md",
        ".agents/skills/README.md",
    ],
};

fn spec(surface: Surface) -> &'static Spec {
    match surface {
        Surface::Opencode => &OPENCODE,
        Surface::Codex => &CODEX,
    }
}

/// The README stubs harness2 seeds into every tree (never part of the editable surface).
pub(crate) fn stub_paths(surface: Surface) -> &'static [&'static str] {
    spec(surface).stubs
}

/// The skill directory of a surface (`skills` / `.agents/skills`).
pub(crate) fn skill_dir(surface: Surface) -> &'static str {
    spec(surface).skill_dir
}

/// Upstream `_NAME`: `[a-z0-9][a-z0-9_\-]*`.
fn is_name(s: &str) -> bool {
    let b = s.as_bytes();
    match b.split_first() {
        Some((f, rest)) => {
            (f.is_ascii_lowercase() || f.is_ascii_digit())
                && rest.iter().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-'
                })
        }
        None => false,
    }
}

/// `{sd}/({_NAME})/SKILL\.md` -> the name.
pub(crate) fn match_skill(surface: Surface, rel: &str) -> Option<&str> {
    let rest = rel
        .strip_prefix(spec(surface).skill_dir)?
        .strip_prefix('/')?;
    let name = rest.strip_suffix("/SKILL.md")?;
    is_name(name).then_some(name)
}

/// `{sd}/{_NAME}/scripts/[A-Za-z0-9_.\-]+`.
fn match_skill_script(surface: Surface, rel: &str) -> bool {
    let Some(rest) = rel
        .strip_prefix(spec(surface).skill_dir)
        .and_then(|r| r.strip_prefix('/'))
    else {
        return false;
    };
    let Some((name, file)) = rest.split_once("/scripts/") else {
        return false;
    };
    is_name(name)
        && !file.is_empty()
        && file
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// `sub_agents/({_NAME})/AGENT\.md` -> the name.
pub(crate) fn match_subagent(rel: &str) -> Option<&str> {
    let name = rel.strip_prefix("sub_agents/")?.strip_suffix("/AGENT.md")?;
    is_name(name).then_some(name)
}

/// `scripts/[a-z0-9_][a-z0-9_]*\.py`.
pub(crate) fn match_script(rel: &str) -> bool {
    rel.strip_prefix("scripts/")
        .and_then(|r| r.strip_suffix(".py"))
        .is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        })
}

/// `{plugin_dir}/({_NAME})\.ts` -> the name.
pub(crate) fn match_plugin(surface: Surface, rel: &str) -> Option<&str> {
    let pd = spec(surface).plugin_dir?;
    let name = rel
        .strip_prefix(pd)?
        .strip_prefix('/')?
        .strip_suffix(".ts")?;
    is_name(name).then_some(name)
}

/// Upstream `normalize_rel`; `None` where upstream raises.
fn normalize_rel(rel: &str) -> Option<String> {
    let r = pyfmt::strip(rel).replace('\\', "/");
    if r.starts_with('/') || r.split('/').any(|c| c == "..") {
        return None;
    }
    let mut r = r.as_str();
    while let Some(rest) = r.strip_prefix("./") {
        r = rest;
    }
    Some(r.trim_matches('/').to_owned())
}

/// Upstream `is_allowed` on an already-normalised path.
fn allowed_normalized(surface: Surface, rel: &str) -> bool {
    let s = spec(surface);
    s.always_on.contains(&rel)
        || s.hooks_file == Some(rel)
        || match_skill(surface, rel).is_some()
        || match_skill_script(surface, rel)
        || match_subagent(rel).is_some()
        || match_script(rel)
        || match_plugin(surface, rel).is_some()
}

/// Upstream `is_allowed`: whether this path is on the editable surface.
pub(crate) fn is_allowed(surface: Surface, rel: &str) -> bool {
    normalize_rel(rel).is_some_and(|r| allowed_normalized(surface, &r))
}

/// Which component a path belongs to, or `None` when it is off the editable surface
/// (README stubs are off-surface).
#[must_use]
pub fn classify(surface: Surface, path: &str) -> Option<Component> {
    let rel = normalize_rel(path)?;
    let s = spec(surface);
    if let Some(i) = s.always_on.iter().position(|a| *a == rel) {
        return Some(match (surface, i) {
            (Surface::Opencode, 1) => Component::Guardrails,
            (Surface::Opencode, 2) => Component::Memory,
            (Surface::Opencode, 3) => Component::ToolGuidance,
            _ => Component::SystemPrompt,
        });
    }
    if s.hooks_file == Some(rel.as_str()) || match_plugin(surface, &rel).is_some() {
        return Some(Component::Plugins);
    }
    if match_skill(surface, &rel).is_some() || match_skill_script(surface, &rel) {
        return Some(Component::Skills);
    }
    if match_subagent(&rel).is_some() {
        return Some(Component::Subagents);
    }
    if match_script(&rel) {
        return Some(Component::Scripts);
    }
    None
}

/// The component a diff row is filed under: [`classify`], or for off-surface paths the
/// closest directory convention (they never occur in trees read by [`read_tree`]).
fn component_for_display(surface: Surface, path: &str) -> Component {
    if let Some(c) = classify(surface, path) {
        return c;
    }
    let s = spec(surface);
    if path.starts_with(&format!("{}/", s.skill_dir)) {
        Component::Skills
    } else if path.starts_with("sub_agents/") {
        Component::Subagents
    } else if path.starts_with("scripts/") {
        Component::Scripts
    } else if s
        .plugin_dir
        .is_some_and(|p| path.starts_with(&format!("{p}/")))
        || path.starts_with(".codex/")
    {
        Component::Plugins
    } else {
        Component::SystemPrompt
    }
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

/// Largest single harness file read.
const MAX_HARNESS_FILE: u64 = 4 * 1024 * 1024;
/// Most directory entries walked in one harness tree.
const MAX_TREE_ENTRIES: usize = 20_000;

/// Python's `Path.read_text(encoding="utf-8", errors="replace")`: invalid UTF-8 replaced and
/// universal newlines (`\r\n` and `\r` become `\n`).
pub(crate) fn decode_text(bytes: Vec<u8>) -> String {
    let s = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };
    if s.contains('\r') {
        s.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        s
    }
}

fn under_pycache(rel: &str) -> bool {
    rel.split('/').any(|c| c == "__pycache__")
}

/// Read every on-surface file under `dir` (README stubs and `__pycache__` excluded).
/// Returns the tree and the off-surface paths found. Never follows symlinks.
///
/// Symbolic links are never read; they are reported among the off-surface paths.
pub fn read_tree(dir: &cap_std::fs::Dir, surface: Surface) -> Result<(Tree, Vec<String>)> {
    let (mut files, symlinks) = safefs::walk_files(dir, &["__pycache__"], MAX_TREE_ENTRIES)?;
    files.sort_by(|a, b| pyfmt::cmp_paths(&a.0, &b.0));
    let stubs = stub_paths(surface);
    let mut tree = Tree::new();
    let mut off = Vec::new();
    for (rel, _) in files {
        if under_pycache(&rel) || stubs.contains(&rel.as_str()) {
            continue;
        }
        if !is_allowed(surface, &rel) {
            off.push(rel);
            continue;
        }
        let bytes = safefs::read_whole(dir, &rel, MAX_HARNESS_FILE, "harness file")?;
        tree.insert(rel, decode_text(bytes));
    }
    for link in symlinks {
        if !under_pycache(&link) && !stubs.contains(&link.as_str()) {
            off.push(link);
        }
    }
    off.sort_by(|a, b| pyfmt::cmp_paths(a, b));
    Ok((tree, off))
}

/// Upstream `always_on_text`: the always-on files joined by blank lines.
pub(crate) fn always_on_text(surface: Surface, tree: &Tree) -> String {
    spec(surface)
        .always_on
        .iter()
        .map(|a| tree.get(*a).map_or("", String::as_str))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn skill_names(surface: Surface, tree: &Tree) -> Vec<String> {
    let set: BTreeSet<&str> = tree
        .keys()
        .filter_map(|r| match_skill(surface, r))
        .collect();
    set.into_iter().map(str::to_owned).collect()
}

fn subagent_names(tree: &Tree) -> Vec<String> {
    let set: BTreeSet<&str> = tree.keys().filter_map(|r| match_subagent(r)).collect();
    set.into_iter().map(str::to_owned).collect()
}

/// Component census (upstream `validate()`'s return value, plus the cap).
#[must_use]
pub fn census(surface: Surface, tree: &Tree) -> Census {
    let s = spec(surface);
    Census {
        always_on_chars: pyfmt::char_len_u32(&always_on_text(surface, tree)),
        always_on_max: always_on_max(surface),
        n_files: u32::try_from(tree.len()).unwrap_or(u32::MAX),
        skills: u32::try_from(skill_names(surface, tree).len()).unwrap_or(u32::MAX),
        sub_agents: u32::try_from(subagent_names(tree).len()).unwrap_or(u32::MAX),
        scripts: u32::try_from(tree.keys().filter(|r| match_script(r)).count()).unwrap_or(u32::MAX),
        plugins: u32::try_from(
            tree.keys()
                .filter(|r| match_plugin(surface, r).is_some())
                .count(),
        )
        .unwrap_or(u32::MAX),
        hooks: u32::from(s.hooks_file.is_some_and(|h| tree.contains_key(h))),
    }
}

/// Upstream `script_ref`: `(?:python3?\s+)((?:{sd}/[\w\-]+/)?scripts/[\w.\-]+\.py)`, with
/// `\s` widened to Python's `str.isspace()` set.
fn script_ref_regex(surface: Surface) -> Option<&'static Regex> {
    static OC: LazyLock<Option<Regex>> = LazyLock::new(|| build_script_ref("skills"));
    static CX: LazyLock<Option<Regex>> = LazyLock::new(|| build_script_ref(".agents/skills"));
    match surface {
        Surface::Opencode => OC.as_ref(),
        Surface::Codex => CX.as_ref(),
    }
}

/// Every script `src` invokes (upstream `script_ref.findall`), in order.
pub(crate) fn script_refs(surface: Surface, src: &str) -> Vec<&str> {
    script_ref_regex(surface).map_or_else(Vec::new, |rx| {
        rx.captures_iter(src)
            .filter_map(|c| c.get(1).map(|m| m.as_str()))
            .collect()
    })
}

fn build_script_ref(skill_dir: &str) -> Option<Regex> {
    let sd = regex::escape(skill_dir);
    Regex::new(&format!(
        r"python3?[\s\x1c-\x1f]+((?:{sd}/[\w\-]+/)?scripts/[\w.\-]+\.py)"
    ))
    .ok()
}

/// Upstream `RESERVED_SCRIPTS`: runtime helpers a harness may invoke without shipping.
const RESERVED_SCRIPTS: &[&str] = &["read_doc.py", "pack.py", "unpack.py"];

fn check_skills(surface: Surface, tree: &Tree, body: &str, errs: &mut Vec<String>) -> usize {
    let s = spec(surface);
    let skills = skill_names(surface, tree);
    for name in &skills {
        let path = format!("{}/{name}/SKILL.md", s.skill_dir);
        let fm = pyfmt::first_chars(tree.get(&path).map_or("", String::as_str), 600);
        if !pyfmt::has_field(fm, "description") {
            errs.push(format!(
                "{path} needs a `description:` in its frontmatter, or the skill is dropped \
                 SILENTLY at load"
            ));
        }
        let m_name = pyfmt::name_field(fm);
        if s.name_must_match {
            if m_name != Some(name.as_str()) {
                errs.push(format!(
                    "{path} needs frontmatter `name: {name}` matching its directory (a mismatch \
                     is dropped SILENTLY at load)"
                ));
            }
        } else if let Some(m) = m_name.filter(|m| *m != name) {
            errs.push(format!(
                "{path} frontmatter `name: {m}` OVERRIDES the directory name at load, so every \
                 trigger line naming `{name}` stops matching; drop the `name:` line or make it \
                 `name: {name}`"
            ));
        }
        if !body.contains(name.as_str()) {
            errs.push(format!(
                "skill '{name}' is never named in the always-on text, so the agent will not \
                 know to invoke it — an untriggered skill is dead weight. {}",
                (s.trigger_hint)(name)
            ));
        }
    }
    skills.len()
}

fn check_subagents(
    surface: Surface,
    tree: &Tree,
    allow_subagents: bool,
    errs: &mut Vec<String>,
) -> usize {
    let s = spec(surface);
    let subs = subagent_names(tree);
    if !subs.is_empty() && !allow_subagents {
        let listed = subs
            .iter()
            .map(|n| pyfmt::repr_str(n))
            .collect::<Vec<_>>()
            .join(", ");
        errs.push(format!(
            "sub_agents are not available on this deployment: [{listed}]"
        ));
    }
    for name in &subs {
        let path = format!("sub_agents/{name}/AGENT.md");
        let fm = pyfmt::first_chars(tree.get(&path).map_or("", String::as_str), 600);
        if s.subagent_requires_mode && !pyfmt::has_mode_subagent(fm) {
            errs.push(format!("{path} needs frontmatter `mode: subagent`"));
        }
        if !pyfmt::has_field(fm, "description") {
            errs.push(format!("{path} needs a `description:`"));
        }
    }
    subs.len()
}

/// Upstream `check_hooks`: the hooks file must parse and match the schema codex dispatches.
fn check_hooks(surface: Surface, src: &str, errs: &mut Vec<String>) {
    use crate::pyjson::{PyJson, loads};
    let s = spec(surface);
    let hf = s.hooks_file.unwrap_or(".codex/hooks.json");
    let doc = match loads(src) {
        Ok(d) => d,
        Err(e) => {
            errs.push(format!("{hf} is not valid JSON: {e}"));
            return;
        }
    };
    let hooks = match doc.get("hooks") {
        Some(PyJson::Object(items)) if !items.is_empty() => items,
        _ => {
            errs.push(format!(
                "{hf} needs a top-level {{\"hooks\": {{\"<Event>\": [...]}}}} object"
            ));
            return;
        }
    };
    let mut events: Vec<&str> = s.hook_events.to_vec();
    events.sort_unstable();
    for (event, entries) in hooks {
        if !s.hook_events.contains(&event.as_str()) {
            errs.push(format!(
                "{hf}: unknown event {}; the framework dispatches {} (PascalCase)",
                pyfmt::repr_str(event),
                events.join(", ")
            ));
            continue;
        }
        let PyJson::Array(entries) = entries else {
            errs.push(format!(
                "{hf}: {event} must map to a LIST of matcher entries"
            ));
            continue;
        };
        for (i, entry) in entries.iter().enumerate() {
            let Some(PyJson::Array(list)) = entry.get("hooks") else {
                errs.push(format!("{hf}: {event}[{i}] needs a `hooks` list"));
                continue;
            };
            for h in list {
                let is_command = matches!(h, PyJson::Object(_))
                    && matches!(h.get("type"), Some(PyJson::Str(t)) if t == "command");
                if !is_command {
                    errs.push(format!(
                        "{hf}: {event}[{i}] hook type must be \"command\" — async/prompt/agent \
                         hooks are skipped as 'not supported yet'"
                    ));
                } else if match h.get("command") {
                    None => true,
                    Some(PyJson::Str(c)) => pyfmt::strip(c).is_empty(),
                    Some(_) => false,
                } {
                    errs.push(format!("{hf}: {event}[{i}] has an empty `command`"));
                }
            }
        }
    }
}

fn check_script_refs(surface: Surface, tree: &Tree, errs: &mut Vec<String>) {
    let s = spec(surface);
    let have: BTreeSet<&str> = tree
        .keys()
        .filter(|r| match_script(r) || match_skill_script(surface, r))
        .map(String::as_str)
        .collect();
    let mut keys: Vec<&String> = tree.keys().collect();
    keys.sort_by(|a, b| pyfmt::cmp_paths(a, b));
    for rel in keys {
        if !(match_skill(surface, rel).is_some() || s.always_on.contains(&rel.as_str())) {
            continue;
        }
        let src = tree.get(rel).map_or("", String::as_str);
        for r in script_refs(surface, src) {
            let file = r.rsplit('/').next().unwrap_or(r);
            if RESERVED_SCRIPTS.contains(&file) {
                continue;
            }
            if !have.contains(r) {
                errs.push(format!(
                    "{rel} invokes `{r}` but that file is not in the harness"
                ));
            }
        }
    }
}

/// Upstream `validate()` on an already on-surface tree; returns every problem in upstream's
/// order and wording.
fn validate_on_surface(surface: Surface, tree: &Tree, allow_subagents: bool) -> Vec<String> {
    let s = spec(surface);
    let mut errs = Vec::new();
    let body = always_on_text(surface, tree);
    let cap = always_on_max(surface);
    let len = pyfmt::char_len_u32(&body);
    if len > cap {
        errs.push(format!(
            "always-on content is {len} chars, over the {cap} cap; {}",
            s.cap_reason
        ));
    }
    if s.reject_empty_always_on && pyfmt::strip(&body).is_empty() {
        errs.push(format!(
            "{} is empty — it is the only always-on file on this framework, so an empty one is \
             a harness that does not exist",
            s.always_on.first().copied().unwrap_or("AGENTS.md")
        ));
    }
    check_skills(surface, tree, &body, &mut errs);
    check_subagents(surface, tree, allow_subagents, &mut errs);
    // Upstream compiles Python scripts here (`compile(src, rel, "exec")`); not ported.
    if let Some(hf) = s.hooks_file
        && let Some(src) = tree.get(hf)
    {
        check_hooks(surface, src, &mut errs);
    }
    check_script_refs(surface, tree, &mut errs);
    errs
}

/// All of upstream's checks: always-on cap, empty always-on (Codex), skill frontmatter
/// (`description:`, `name:` rule per surface), untriggered skills, sub-agent rules,
/// hooks schema (Codex), scripts invoked but not shipped. Every problem is collected.
///
/// Paths are normalised as upstream's `is_allowed` does; files off the editable surface are
/// listed in `off_surface` (harness2 ignores them) and the rest is validated. README stubs
/// are ignored silently. `ok` means no problem was found.
#[must_use]
pub fn validate(surface: Surface, tree: &Tree, allow_subagents: bool) -> ValidationReport {
    let stubs = stub_paths(surface);
    let mut on = Tree::new();
    let mut off = Vec::new();
    for (rel, content) in tree {
        match normalize_rel(rel) {
            Some(n) if stubs.contains(&n.as_str()) => {}
            Some(n) if allowed_normalized(surface, &n) => {
                on.insert(n, content.clone());
            }
            _ => off.push(rel.clone()),
        }
    }
    off.sort_by(|a, b| pyfmt::cmp_paths(a, b));
    let errors = validate_on_surface(surface, &on, allow_subagents);
    ValidationReport {
        ok: errors.is_empty(),
        errors,
        off_surface: off,
        census: census(surface, &on),
    }
}

/// Lines added and removed between two texts (line diff, as in the unified view).
fn line_counts(diff: &similar::TextDiff<'_, '_, str>) -> (u32, u32) {
    let (mut added, mut removed) = (0_u32, 0_u32);
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => added = added.saturating_add(1),
            similar::ChangeTag::Delete => removed = removed.saturating_add(1),
            similar::ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

fn text_diff<'a>(old: &'a str, new: &'a str) -> similar::TextDiff<'a, 'a, str> {
    similar::TextDiff::configure()
        .algorithm(similar::Algorithm::Myers)
        .timeout(Duration::from_secs(2))
        .diff_lines(old, new)
}

/// File-by-file diff with unified hunks (3 lines of context).
///
/// Files are grouped changed-first; within a group they are ordered c1..c8, then by path.
/// Added and removed files carry their full text and count every line.
#[must_use]
pub fn diff(
    surface: Surface,
    from_label: &str,
    from: &Tree,
    to_label: &str,
    to: &Tree,
) -> HarnessDiff {
    let paths: BTreeSet<&String> = from.keys().chain(to.keys()).collect();
    let mut changed = Vec::new();
    let mut same = Vec::new();
    for path in paths {
        let component = component_for_display(surface, path);
        let row = match (from.get(path), to.get(path)) {
            (None, Some(new)) => {
                let (added, _) = line_counts(&text_diff("", new));
                FileDiff {
                    path: path.clone(),
                    component,
                    change: ChangeKind::Added,
                    unified: new.clone(),
                    lines_added: added,
                    lines_removed: 0,
                }
            }
            (Some(old), None) => {
                let (_, removed) = line_counts(&text_diff(old, ""));
                FileDiff {
                    path: path.clone(),
                    component,
                    change: ChangeKind::Removed,
                    unified: old.clone(),
                    lines_added: 0,
                    lines_removed: removed,
                }
            }
            (Some(old), Some(new)) if old != new => {
                let d = text_diff(old, new);
                let (added, removed) = line_counts(&d);
                let unified = d
                    .unified_diff()
                    .context_radius(3)
                    .header(
                        &format!("{from_label}/{path}"),
                        &format!("{to_label}/{path}"),
                    )
                    .to_string();
                FileDiff {
                    path: path.clone(),
                    component,
                    change: ChangeKind::Modified,
                    unified,
                    lines_added: added,
                    lines_removed: removed,
                }
            }
            _ => FileDiff {
                path: path.clone(),
                component,
                change: ChangeKind::Unchanged,
                unified: String::new(),
                lines_added: 0,
                lines_removed: 0,
            },
        };
        if row.change == ChangeKind::Unchanged {
            same.push(row);
        } else {
            changed.push(row);
        }
    }
    let order = |a: &FileDiff, b: &FileDiff| {
        a.component
            .cmp(&b.component)
            .then_with(|| pyfmt::cmp_paths(&a.path, &b.path))
    };
    changed.sort_by(order);
    same.sort_by(order);
    changed.extend(same);
    let before = i64::from(pyfmt::char_len_u32(&always_on_text(surface, from)));
    let after = i64::from(pyfmt::char_len_u32(&always_on_text(surface, to)));
    HarnessDiff {
        from: from_label.to_owned(),
        to: to_label.to_owned(),
        files: changed,
        always_on_char_delta: after - before,
    }
}

/// Upstream `runmark.MARK`, excluded from fingerprints.
const MARK: &str = ".adapt_complete";

/// Upstream `runmark._ignored`, plus the marker itself.
fn fingerprint_ignored(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    under_pycache(rel)
        || name == MARK
        || name == ".DS_Store"
        // `Path.suffix == ".pyc"`: a leading dot alone does not start a suffix.
        || (name.len() > 4 && name.ends_with(".pyc"))
}

/// Upstream `runmark.fingerprint`: SHA-1 over every regular file under `dir` in sorted
/// relative-path order (`path\0bytes\0`), skipping `.adapt_complete`, `__pycache__`,
/// `*.pyc` and `.DS_Store`; first 16 hex digits. An empty or missing dir is `absent`.
///
/// Like upstream, an existing directory without files digests to `da39a3ee5e6b4b0d` (SHA-1
/// of nothing); only a directory that does not exist is `absent`, and a handle always
/// refers to one that does. Symbolic links are skipped (upstream would hash a linked file's
/// target); trees harness2 writes never contain them.
pub fn fingerprint(dir: &cap_std::fs::Dir) -> Result<String> {
    let (mut files, _symlinks) = safefs::walk_files(dir, &[], MAX_TREE_ENTRIES * 5)?;
    files.retain(|(rel, _)| !fingerprint_ignored(rel));
    files.sort_by(|a, b| pyfmt::cmp_paths(&a.0, &b.0));
    let mut h = Sha1::new();
    let mut buf = vec![0_u8; 64 * 1024];
    for (rel, _) in files {
        h.update(rel.as_bytes());
        h.update(b"\0");
        match safefs::open_file(dir, &rel, "harness file") {
            Ok((mut file, _)) => loop {
                match file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => h.update(buf.get(..n).unwrap_or_default()),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        h.update(b"<unreadable>");
                        break;
                    }
                }
            },
            Err(CoreError::Refused(_) | CoreError::NotFound(_) | CoreError::Io(_)) => {
                h.update(b"<unreadable>");
            }
            Err(e) => return Err(e),
        }
        h.update(b"\0");
    }
    let digest = hex::encode(h.finalize());
    Ok(digest.get(..16).unwrap_or(&digest).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(items: &[(&str, &str)]) -> Tree {
        items
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn classification_follows_surface_patterns() {
        use Component::*;
        let oc = Surface::Opencode;
        assert_eq!(classify(oc, "systemprompt.md"), Some(SystemPrompt));
        assert_eq!(classify(oc, "guardrails.md"), Some(Guardrails));
        assert_eq!(classify(oc, "LongTermMEMORY.md"), Some(Memory));
        assert_eq!(
            classify(oc, "tool_descriptions/tool_guidance.md"),
            Some(ToolGuidance)
        );
        assert_eq!(classify(oc, "skills/doc_review/SKILL.md"), Some(Skills));
        assert_eq!(classify(oc, "skills/doc_review/scripts/x.py"), Some(Skills));
        assert_eq!(classify(oc, "sub_agents/checker/AGENT.md"), Some(Subagents));
        assert_eq!(classify(oc, "scripts/check_totals.py"), Some(Scripts));
        assert_eq!(classify(oc, "plugin/guard.ts"), Some(Plugins));
        assert_eq!(classify(oc, "./scripts/a.py"), Some(Scripts));
        assert_eq!(classify(oc, "skills/README.md"), None);
        assert_eq!(classify(oc, "scripts/Check.py"), None);
        assert_eq!(classify(oc, "skills/Doc/SKILL.md"), None);
        assert_eq!(classify(oc, "../systemprompt.md"), None);
        assert_eq!(classify(oc, "AGENTS.md"), None);
        let cx = Surface::Codex;
        assert_eq!(classify(cx, "AGENTS.md"), Some(SystemPrompt));
        assert_eq!(classify(cx, ".agents/skills/x/SKILL.md"), Some(Skills));
        assert_eq!(classify(cx, ".codex/hooks.json"), Some(Plugins));
        assert_eq!(classify(cx, "plugin/x.ts"), None);
        assert_eq!(classify(cx, "systemprompt.md"), None);
    }

    #[test]
    fn opencode_validation_messages() {
        let tree = t(&[
            (
                "systemprompt.md",
                "Use the `known` skill. python3 scripts/missing.py",
            ),
            ("guardrails.md", ""),
            (
                "skills/known/SKILL.md",
                "---\nname: known\ndescription: does x\n---\n",
            ),
            ("skills/silent/SKILL.md", "---\nname: other\n---\n"),
            (
                "sub_agents/helper/AGENT.md",
                "---\ndescription: helps\n---\n",
            ),
            ("scripts/README.md", "stub"),
            ("notes.txt", "off surface"),
        ]);
        let r = validate(Surface::Opencode, &tree, false);
        assert!(!r.ok);
        assert_eq!(r.off_surface, vec!["notes.txt".to_owned()]);
        assert_eq!(
            r.errors,
            vec![
                "skills/silent/SKILL.md needs a `description:` in its frontmatter, or the skill \
                 is dropped SILENTLY at load",
                "skills/silent/SKILL.md needs frontmatter `name: silent` matching its directory \
                 (a mismatch is dropped SILENTLY at load)",
                "skill 'silent' is never named in the always-on text, so the agent will not know \
                 to invoke it — an untriggered skill is dead weight. Fix it by adding a trigger \
                 line naming `silent` to systemprompt.md or guardrails.md, e.g. \"When <the \
                 situation this skill handles>, load the `silent` skill.\"",
                "sub_agents are not available on this deployment: ['helper']",
                "sub_agents/helper/AGENT.md needs frontmatter `mode: subagent`",
                "systemprompt.md invokes `scripts/missing.py` but that file is not in the \
                 harness",
            ]
        );
        assert_eq!(r.census.skills, 2);
        assert_eq!(r.census.sub_agents, 1);
        assert_eq!(r.census.n_files, 5);
    }

    #[test]
    fn codex_validation_messages() {
        let tree = t(&[
            ("AGENTS.md", "  \n"),
            (".agents/skills/a/SKILL.md", "name: b\ndescription: x\n"),
            (
                ".codex/hooks.json",
                r#"{"hooks": {"Nope": [], "PostToolUse": [{"hooks": [{"type": "prompt"}, {"type": "command", "command": " "}]}, 3], "Stop": {}}}"#,
            ),
        ]);
        let r = validate(Surface::Codex, &tree, true);
        assert_eq!(
            r.errors,
            vec![
                "AGENTS.md is empty — it is the only always-on file on this framework, so an \
                 empty one is a harness that does not exist",
                ".agents/skills/a/SKILL.md frontmatter `name: b` OVERRIDES the directory name at \
                 load, so every trigger line naming `a` stops matching; drop the `name:` line or \
                 make it `name: a`",
                "skill 'a' is never named in the always-on text, so the agent will not know to \
                 invoke it — an untriggered skill is dead weight. Fix it by adding a trigger line \
                 naming `a` to AGENTS.md, e.g. \"When <the situation this skill handles>, read \
                 .agents/skills/a/SKILL.md and follow it.\"",
                ".codex/hooks.json: unknown event 'Nope'; the framework dispatches \
                 PermissionRequest, PostCompact, PostToolUse, PreCompact, PreToolUse, \
                 SessionStart, Stop, SubagentStart, SubagentStop, UserPromptSubmit (PascalCase)",
                ".codex/hooks.json: PostToolUse[0] hook type must be \"command\" — \
                 async/prompt/agent hooks are skipped as 'not supported yet'",
                ".codex/hooks.json: PostToolUse[0] has an empty `command`",
                ".codex/hooks.json: PostToolUse[1] needs a `hooks` list",
                ".codex/hooks.json: Stop must map to a LIST of matcher entries",
            ]
        );
        let bad_json = t(&[("AGENTS.md", "x"), (".codex/hooks.json", "{")]);
        assert_eq!(
            validate(Surface::Codex, &bad_json, true).errors,
            vec![
                ".codex/hooks.json is not valid JSON: Expecting property name enclosed in \
                 double quotes: line 1 column 2 (char 1)"
            ]
        );
    }

    #[test]
    fn cap_and_census() {
        let big = "x".repeat(8_001);
        let tree = t(&[("AGENTS.md", &big)]);
        let r = validate(Surface::Codex, &tree, true);
        assert_eq!(
            r.errors,
            vec![
                "always-on content is 8001 chars, over the 8000 cap; AGENTS.md is injected whole \
                 into EVERY turn, and codex additionally truncates it silently at 32,768 bytes"
            ]
        );
        let oc = t(&[
            ("systemprompt.md", "ab"),
            ("guardrails.md", "c"),
            ("plugin/x.ts", ""),
            ("scripts/a.py", ""),
        ]);
        let c = census(Surface::Opencode, &oc);
        // "ab" + "\n\n" + "c" + "\n\n" + "" + "\n\n" + "" = 9 chars.
        assert_eq!(c.always_on_chars, 9);
        assert_eq!(
            (c.plugins, c.scripts, c.hooks, c.always_on_max),
            (1, 1, 0, 24_000)
        );
        assert!(validate(Surface::Opencode, &oc, true).ok);
    }

    #[test]
    fn diff_orders_changed_first() {
        let a = t(&[
            ("systemprompt.md", "one\ntwo\n"),
            ("guardrails.md", "same\n"),
            ("scripts/old.py", "print(1)\n"),
        ]);
        let b = t(&[
            ("systemprompt.md", "one\nTWO\nthree\n"),
            ("guardrails.md", "same\n"),
            ("scripts/new.py", "a\nb\n"),
        ]);
        let d = diff(Surface::Opencode, "base", &a, "agg1", &b);
        let rows: Vec<_> = d
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.change))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("systemprompt.md", ChangeKind::Modified),
                ("scripts/new.py", ChangeKind::Added),
                ("scripts/old.py", ChangeKind::Removed),
                ("guardrails.md", ChangeKind::Unchanged),
            ]
        );
        let m = &d.files[0];
        assert_eq!((m.lines_added, m.lines_removed), (2, 1));
        assert!(
            m.unified
                .starts_with("--- base/systemprompt.md\n+++ agg1/systemprompt.md\n@@")
        );
        assert_eq!(d.files[1].lines_added, 2);
        assert_eq!(d.always_on_char_delta, 6);
    }

    #[test]
    fn read_tree_and_fingerprint() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        std::fs::create_dir_all(p.join("scripts/__pycache__")).unwrap();
        std::fs::create_dir_all(p.join("tool_descriptions")).unwrap();
        std::fs::write(p.join("systemprompt.md"), "a\r\nb\rc").unwrap();
        std::fs::write(p.join("tool_descriptions/tool_guidance.md"), "").unwrap();
        std::fs::write(p.join("scripts/README.md"), "stub").unwrap();
        std::fs::write(p.join("scripts/a.py"), "x").unwrap();
        std::fs::write(p.join("scripts/__pycache__/a.cpython-312.pyc"), "x").unwrap();
        std::fs::write(p.join("notes.txt"), "x").unwrap();
        std::fs::write(p.join(MARK), "{}").unwrap();
        std::os::unix::fs::symlink("/etc/hostname", p.join("guardrails.md")).unwrap();
        let dir = cap_std::fs::Dir::open_ambient_dir(p, cap_std::ambient_authority()).unwrap();
        let (tree, off) = read_tree(&dir, Surface::Opencode).unwrap();
        assert_eq!(
            tree.keys().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "scripts/a.py",
                "systemprompt.md",
                "tool_descriptions/tool_guidance.md"
            ]
        );
        assert_eq!(tree["systemprompt.md"], "a\nb\nc");
        assert_eq!(
            off,
            vec![MARK.to_owned(), "guardrails.md".into(), "notes.txt".into()]
        );
        let fp = fingerprint(&dir).unwrap();
        assert_eq!(fp.len(), 16);
        let empty = tempfile::tempdir().unwrap();
        let ed =
            cap_std::fs::Dir::open_ambient_dir(empty.path(), cap_std::ambient_authority()).unwrap();
        assert_eq!(fingerprint(&ed).unwrap(), "da39a3ee5e6b4b0d");
    }

    #[test]
    fn script_ref_regex_compiles() {
        assert!(script_ref_regex(Surface::Opencode).is_some());
        assert!(script_ref_regex(Surface::Codex).is_some());
    }
}
