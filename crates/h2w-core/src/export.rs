//! Turn an adopted harness into files a user can drop into their own project.
//!
//! - `raw`: the tree as harness2 stores it.
//! - `opencode`: `AGENTS.md` = the always-on files joined in upstream order, skills under
//!   `.opencode/skills/<name>/SKILL.md` with their helpers in `skills/<name>/` (where harness2's
//!   OpenCode runner puts them), sub-agents under `.opencode/agent/<name>.md`, plugins under
//!   `.opencode/plugin/<name>.ts`, scripts under `scripts/`.
//! - `codex`: `AGENTS.md`, `.agents/skills/`, `.codex/hooks.json`, `scripts/`, sub-agents
//!   as `.codex/agents/<name>.md` with a note that Codex has no native sub-agent loader.
//! - `hermes`: a Hermes Agent home: `SOUL.md` (the identity text and the other always-on
//!   instructions), `memories/MEMORY.md` (the long-term memory), `skills/<name>/`, plus the
//!   workspace's `scripts/`.
//! - `turnstone`: `persona.md` (the persona's base prompt), `instructions.md` (the
//!   `session.instructions` setting: the other always-on text), `skills/<name>/`, `scripts/`
//!   and `IMPORT.md`, because Turnstone keeps a harness in its database.
//!
//! Any tree exports to any target, except what has no counterpart there: OpenCode plugins
//! and Codex hooks act inside the tool loop, and Hermes Agent and Turnstone have no
//! file-defined sub-agents, so those combinations are refused; so are Turnstone skill names it
//! would refuse. Export never changes file contents except to join the always-on files and to
//! move the paths of skill files where a framework keeps them elsewhere ([`skill_home`]:
//! Hermes' skills reach their own folder through `${HERMES_SKILL_DIR}`; the others by a path
//! from the workspace), in skill files and in the always-on text. Trigger lines that name a
//! framework's skill tool are not rewritten: a note says so.
//! It takes at most
//! a harness's worth of files (the limits of [`tree::read_tree`]), and every path must be a
//! clean relative path without control characters ([`names::relative_path`]), so no
//! terminal escape sequence can ship in a file name.

use std::io::Write;

use h2w_api::{Component, ExportBundle, ExportTarget, FileText, HarnessTree, Surface};

use crate::tree::{self, TURNSTONE_TEXT_MAX, Tree};
use crate::{CoreError, Result, names, pyfmt};

/// The joined always-on text: non-empty always-on files in upstream order, separated by a
/// blank line.
fn joined_always_on(surface: Surface, tree: &Tree) -> String {
    tree::always_on_files(surface)
        .iter()
        .filter_map(|a| tree.get(*a))
        .filter(|t| !pyfmt::strip(t).is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn refuse(problem: &str) -> CoreError {
    CoreError::invalid("target", problem)
}

/// Hermes Agent substitutes the skill's own folder for this in its skill files.
const HERMES_SKILL_DIR: &str = "${HERMES_SKILL_DIR}";

/// How each framework's agent loads a skill, for notes on trigger lines.
fn skill_loader(surface: Surface) -> &'static str {
    match surface {
        Surface::Opencode => "OpenCode's `skill` tool",
        Surface::Codex => "reading its SKILL.md (Codex)",
        Surface::Hermes => "Hermes Agent's `skill_view` tool",
        Surface::Turnstone => "Turnstone's `skills(action=\"load\", name=...)` tool",
    }
}

/// A tree's always-on text by role: the identity (persona) text, further always-on
/// instructions, and long-term memory. Empty files are left out.
struct AlwaysOn {
    identity: Option<String>,
    rest: Vec<String>,
    memory: Option<String>,
}

fn split_always_on(surface: Surface, src: &Tree) -> AlwaysOn {
    let get = |p: &str| src.get(p).filter(|t| !pyfmt::strip(t).is_empty()).cloned();
    match surface {
        Surface::Opencode => AlwaysOn {
            identity: get("systemprompt.md"),
            rest: ["guardrails.md", "tool_descriptions/tool_guidance.md"]
                .into_iter()
                .filter_map(get)
                .collect(),
            memory: get("LongTermMEMORY.md"),
        },
        Surface::Codex => AlwaysOn {
            identity: get("AGENTS.md"),
            rest: Vec::new(),
            memory: None,
        },
        Surface::Hermes => AlwaysOn {
            identity: get("SOUL.md"),
            rest: Vec::new(),
            memory: get("memories/MEMORY.md"),
        },
        Surface::Turnstone => AlwaysOn {
            identity: get("systemprompt.md"),
            rest: get("guardrails.md").into_iter().collect(),
            memory: None,
        },
    }
}

/// Replace `from` with `to` where `from` starts a path: at the start of the text or after a
/// character that cannot be part of a path (so `skills/a/` inside `.opencode/skills/a/` is
/// left alone).
fn replace_path_prefix(text: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(j) = text[i..].find(from) {
        let at = i + j;
        out.push_str(&text[i..at]);
        out.push_str(if starts_path(text, at) { to } else { from });
        i = at + from.len();
    }
    out.push_str(&text[i..]);
    out
}

/// Whether a path can start at byte `at` of `text`: at its start, or after a character that
/// cannot be part of a path.
fn starts_path(text: &str, at: usize) -> bool {
    text[..at]
        .chars()
        .next_back()
        .is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '$' | '{')))
}

/// A skill file's path relative to the skill folder, with the skill's name.
fn skill_parts(surface: Surface, path: &str) -> (&str, &str) {
    let rest = path
        .strip_prefix(tree::skill_dir(surface))
        .and_then(|r| r.strip_prefix('/'))
        .unwrap_or(path);
    let name = rest.split('/').next().unwrap_or_default();
    (name, rest)
}

/// Where a target keeps skill `name`, as its texts name it: the skill's `SKILL.md`, and the
/// folder its helpers run from. OpenCode reads `SKILL.md` from `.opencode/skills/`, but
/// harness2's OpenCode runner copies a skill's helpers into the workspace's `skills/<name>/`
/// (`third_party/lab_runner/opencode/render.py`), so an OpenCode project keeps them there.
/// Turnstone keeps skills in its database and its runner copies helpers into the workspace's
/// `skills/<name>/`; Hermes Agent substitutes the skill's own folder for `${HERMES_SKILL_DIR}`.
fn skill_home(target: ExportTarget, name: &str) -> (String, String) {
    match target {
        ExportTarget::Opencode => (
            format!(".opencode/skills/{name}/SKILL.md"),
            format!("skills/{name}"),
        ),
        ExportTarget::Codex => (
            format!(".agents/skills/{name}/SKILL.md"),
            format!(".agents/skills/{name}"),
        ),
        ExportTarget::Hermes => (
            format!("{HERMES_SKILL_DIR}/SKILL.md"),
            HERMES_SKILL_DIR.to_owned(),
        ),
        ExportTarget::Turnstone | ExportTarget::Raw => {
            (format!("skills/{name}/SKILL.md"), format!("skills/{name}"))
        }
    }
}

/// How the source framework's texts name skill `name`'s folder.
fn skill_folder(surface: Surface, name: &str) -> String {
    if surface == Surface::Hermes {
        HERMES_SKILL_DIR.to_owned()
    } else {
        format!("{}/{name}", tree::skill_dir(surface))
    }
}

/// `text` with skill `name`'s paths moved from the source framework's form to the target's:
/// its `SKILL.md` path first, then every other path under the skill folder. A framework that
/// already names them the target's way is left as it is.
fn move_skill_paths(surface: Surface, name: &str, text: &str, target: ExportTarget) -> String {
    let (skill_md, to_dir) = skill_home(target, name);
    let from_dir = skill_folder(surface, name);
    if from_dir == to_dir {
        return text.to_owned();
    }
    if surface == Surface::Hermes {
        return text
            .replace(&format!("{HERMES_SKILL_DIR}/SKILL.md"), &skill_md)
            .replace(HERMES_SKILL_DIR, &to_dir);
    }
    let moved = replace_path_prefix(text, &format!("{from_dir}/SKILL.md"), &skill_md);
    replace_path_prefix(&moved, &format!("{from_dir}/"), &format!("{to_dir}/"))
}

/// The always-on text with every skill's paths moved to the target's form. A Hermes Agent
/// tree's always-on text cannot reach skill folders (`${HERMES_SKILL_DIR}` is substituted only
/// in skill files), so it is left as it is.
fn move_always_on_paths(
    surface: Surface,
    skills: &[String],
    text: String,
    target: ExportTarget,
) -> String {
    if surface == Surface::Hermes {
        return text;
    }
    skills
        .iter()
        .fold(text, |t, name| move_skill_paths(surface, name, &t, target))
}

/// The skill names of a tree (from its `SKILL.md` files), sorted.
fn skill_names(surface: Surface, src: &Tree) -> Vec<String> {
    let mut names: Vec<String> = src
        .keys()
        .filter_map(|p| tree::match_skill(surface, p))
        .map(str::to_owned)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Whether `text` names a path under the source's folder for skill `name` (where a path can
/// start).
fn names_skill_folder(surface: Surface, name: &str, text: &str) -> bool {
    let folder = format!("{}/", skill_folder(surface, name));
    text.match_indices(&folder)
        .any(|(at, _)| starts_path(text, at))
}

/// Refuse more than one harness's worth of files: at most [`tree::MAX_TREE_FILES`] plus
/// `extra_files` (room for the joined `AGENTS.md`), and at most [`tree::MAX_TREE_BYTES`]
/// (plus the blank lines that join adds, when there is room for it).
fn check_size(files: usize, sizes: impl Iterator<Item = usize>, extra_files: usize) -> Result<()> {
    let max_files = tree::MAX_TREE_FILES + extra_files;
    if files > max_files {
        return Err(CoreError::TooLarge(format!(
            "the export has {files} files, over the limit of {max_files}"
        )));
    }
    // The always-on join adds at most a blank line between each of its files.
    let slack = if extra_files > 0 { 64 } else { 0 };
    let max_bytes = tree::MAX_TREE_BYTES + slack;
    let total = sizes.fold(0_u64, |acc, n| {
        acc.saturating_add(u64::try_from(n).unwrap_or(u64::MAX))
    });
    if total > max_bytes {
        return Err(CoreError::TooLarge(format!(
            "the export's files total {total} bytes, over the {max_bytes}-byte limit"
        )));
    }
    Ok(())
}

/// Build the bundle for `target`. Fails when the tree's surface cannot express `target`
/// faithfully (e.g. OpenCode plugins exported to Codex) unless the problem is only a note.
///
/// Refused: OpenCode plugins to any other framework, Codex hooks to any other framework (each
/// acts inside the tool loop and has no counterpart), sub-agents to Hermes Agent or Turnstone
/// (neither defines them in files), and for Turnstone skill names it refuses, an empty
/// identity text, and texts over its caps. Notes cover sub-agents on Codex, frontmatter a
/// framework requires but the tree may lack, sizes, trigger lines that name another
/// framework's skill tool, plugin versions, and where each part of a bundle goes.
/// More files or bytes than [`tree::read_tree`] would read is [`CoreError::TooLarge`].
pub fn export(tree: &HarnessTree, target: ExportTarget) -> Result<ExportBundle> {
    names::harness_label(&tree.label)?;
    check_size(
        tree.files.len(),
        tree.files
            .iter()
            .map(|f| f.content.as_ref().map_or(0, String::len)),
        0,
    )?;
    let mut src = Tree::new();
    for f in &tree.files {
        names::relative_path(&f.path)?;
        if f.path.is_empty() {
            return Err(CoreError::invalid("files", "a file has an empty path"));
        }
        let Some(content) = &f.content else {
            return Err(CoreError::invalid(
                "files",
                "harness file contents are missing; load the harness with its contents",
            ));
        };
        src.insert(f.path.clone(), content.clone());
    }
    let surface = tree.surface;
    let mut files: Vec<(String, String)> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    if target == ExportTarget::Raw {
        files.extend(src);
        return Ok(bundle(target, &tree.label, files, notes));
    }
    match target {
        ExportTarget::Hermes => return hermes_home(&tree.label, surface, &src),
        ExportTarget::Turnstone => return turnstone_bundle(&tree.label, surface, &src),
        _ => {}
    }

    let always_on = tree::always_on_files(surface);
    let skills = skill_names(surface, &src);
    let mut skipped = 0_usize;
    let mut has_plugins = false;
    let mut helpers = false;
    let mut sub_agents: Vec<String> = Vec::new();
    for (path, content) in &src {
        if always_on.contains(&path.as_str()) {
            continue;
        }
        match tree::classify(surface, path) {
            Some(Component::Skills) => {
                let (name, rest) = skill_parts(surface, path);
                let is_skill_md = rest == format!("{name}/SKILL.md");
                if target == ExportTarget::Opencode
                    && surface == Surface::Codex
                    && is_skill_md
                    && pyfmt::name_field(pyfmt::first_chars(content, 600)) != Some(name)
                {
                    notes.push(format!(
                        "Skill {name} has no `name: {name}` frontmatter, which OpenCode \
                         requires (it drops such skills silently); add it before use."
                    ));
                }
                let (skill_md, helper_dir) = skill_home(target, name);
                let dest = if is_skill_md {
                    skill_md
                } else {
                    let inside = rest.strip_prefix(name).unwrap_or(rest);
                    helpers |= target == ExportTarget::Opencode;
                    format!("{helper_dir}{inside}")
                };
                files.push((dest, move_skill_paths(surface, name, content, target)));
            }
            Some(Component::Subagents) => {
                let name = tree::match_subagent(path).unwrap_or("agent").to_owned();
                let dest = match target {
                    ExportTarget::Opencode => format!(".opencode/agent/{name}.md"),
                    _ => format!(".codex/agents/{name}.md"),
                };
                if target == ExportTarget::Opencode
                    && !pyfmt::has_mode_subagent(pyfmt::first_chars(content, 600))
                {
                    notes.push(format!(
                        "Sub-agent {name} has no `mode: subagent` frontmatter, which OpenCode \
                         needs to run it as a sub-agent; add it before use."
                    ));
                }
                sub_agents.push(name);
                files.push((dest, content.clone()));
            }
            Some(Component::Scripts) => files.push((path.clone(), content.clone())),
            Some(Component::Plugins) if surface == Surface::Opencode => {
                if target == ExportTarget::Codex {
                    return Err(refuse(
                        "OpenCode plugins (plugin/*.ts) act inside the tool loop and have no \
                         Codex counterpart; export as opencode or raw",
                    ));
                }
                has_plugins = true;
                let name = path.strip_prefix("plugin/").unwrap_or(path);
                files.push((format!(".opencode/plugin/{name}"), content.clone()));
            }
            Some(Component::Plugins) => {
                if target == ExportTarget::Opencode {
                    return Err(refuse(
                        "Codex hooks (.codex/hooks.json) act inside the tool loop and have no \
                         OpenCode counterpart; export as codex or raw",
                    ));
                }
                files.push((path.clone(), content.clone()));
            }
            _ => skipped += 1,
        }
    }

    let agents = move_always_on_paths(surface, &skills, joined_always_on(surface, &src), target);
    if agents.is_empty() {
        notes.push("The always-on files are empty, so no AGENTS.md was written.".to_owned());
    } else {
        let chars = agents.chars().count();
        if target == ExportTarget::Codex
            && chars > usize::try_from(tree::always_on_max(Surface::Codex)).unwrap_or(usize::MAX)
        {
            notes.push(format!(
                "AGENTS.md is {chars} characters; Codex injects it into every turn and \
                 harness2 caps it at 8,000 (Codex truncates it at 32 KiB)."
            ));
        }
        files.push(("AGENTS.md".to_owned(), agents));
    }
    match surface {
        Surface::Opencode => notes.push(
            "AGENTS.md joins systemprompt.md, guardrails.md, LongTermMEMORY.md and \
             tool_descriptions/tool_guidance.md in that order."
                .to_owned(),
        ),
        Surface::Hermes => notes.push(
            "AGENTS.md joins SOUL.md (Hermes Agent's identity text) and memories/MEMORY.md; \
             skill helpers reached through ${HERMES_SKILL_DIR} now use the skill folder's path."
                .to_owned(),
        ),
        Surface::Turnstone => notes.push(
            "AGENTS.md joins systemprompt.md (the persona's base prompt) and guardrails.md (the \
             session instructions)."
                .to_owned(),
        ),
        Surface::Codex => {}
    }
    if matches!(surface, Surface::Hermes | Surface::Turnstone) && !skills.is_empty() {
        notes.push(format!(
            "Trigger lines name {}; {} loads a skill by {}: adjust them.",
            skill_loader(surface),
            if target == ExportTarget::Opencode {
                "OpenCode"
            } else {
                "Codex"
            },
            if target == ExportTarget::Opencode {
                "its `skill` tool"
            } else {
                "reading its SKILL.md"
            }
        ));
    }
    if helpers {
        notes.push(
            "Skill helpers are in skills/<name>/ at the project root, where harness2's OpenCode \
             runner puts them, and each SKILL.md is in .opencode/skills/<name>/."
                .to_owned(),
        );
    }
    if has_plugins {
        notes.push(
            "Plugins were written for OpenCode 1.14 (the version harness2 runs); check them \
             against your OpenCode version."
                .to_owned(),
        );
    }
    if target == ExportTarget::Codex && !sub_agents.is_empty() {
        notes.push(format!(
            "Codex has no native sub-agent loader: install .codex/agents/{{{}}}.md as \
             [agents.<name>] roles in your Codex configuration (harness2's runner does this).",
            sub_agents.join(",")
        ));
    }
    let crosses = matches!(
        (surface, target),
        (Surface::Opencode, ExportTarget::Codex) | (Surface::Codex, ExportTarget::Opencode)
    );
    if crosses {
        notes.push(
            "Paths to skill files and their helpers were moved to where the other framework \
             keeps them; other component paths named in text (sub-agents, plugins, tool \
             guidance) were not rewritten."
                .to_owned(),
        );
    }
    if skipped > 0 {
        notes.push(format!(
            "{skipped} file(s) outside the editable surface were left out."
        ));
    }
    Ok(bundle(target, &tree.label, files, notes))
}

/// The refusal for tool-loop components a framework has no counterpart for.
fn no_counterpart(surface: Surface, component: Component, framework: &str) -> CoreError {
    match (surface, component) {
        (_, Component::Subagents) => refuse(&format!(
            "{framework} has no sub-agents defined in files; export as raw, opencode or codex"
        )),
        (Surface::Opencode, _) => refuse(&format!(
            "OpenCode plugins (plugin/*.ts) act inside the tool loop and have no {framework} \
             counterpart; export as opencode or raw"
        )),
        _ => refuse(&format!(
            "Codex hooks (.codex/hooks.json) act inside the tool loop and have no {framework} \
             counterpart; export as codex or raw"
        )),
    }
}

/// A Hermes Agent home: `SOUL.md` (identity text and the other always-on instructions),
/// `memories/MEMORY.md` (long-term memory), `skills/<name>/` with helpers reached through
/// `${HERMES_SKILL_DIR}`, and the workspace's `scripts/`.
fn hermes_home(label: &str, surface: Surface, src: &Tree) -> Result<ExportBundle> {
    let mut files: Vec<(String, String)> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut skipped = 0_usize;
    let always_on = tree::always_on_files(surface);
    for (path, content) in src {
        if always_on.contains(&path.as_str()) {
            continue;
        }
        match tree::classify(surface, path) {
            Some(Component::Skills) => {
                let (name, rest) = skill_parts(surface, path);
                let content = move_skill_paths(surface, name, content, ExportTarget::Hermes);
                if rest == format!("{name}/SKILL.md")
                    && pyfmt::name_field(pyfmt::first_chars(&content, 600)) != Some(name)
                {
                    notes.push(format!(
                        "Skill {name} has no `name: {name}` frontmatter, which Hermes Agent \
                         lists skills by; add it before use."
                    ));
                }
                files.push((format!("skills/{rest}"), content));
            }
            Some(Component::Scripts) => files.push((path.clone(), content.clone())),
            Some(c @ (Component::Subagents | Component::Plugins)) => {
                return Err(no_counterpart(surface, c, "Hermes Agent"));
            }
            _ => skipped += 1,
        }
    }
    let parts = split_always_on(surface, src);
    let soul = parts
        .identity
        .iter()
        .chain(parts.rest.iter())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n\n");
    let unreachable: Vec<String> = skill_names(surface, src)
        .into_iter()
        .filter(|n| surface != Surface::Hermes && names_skill_folder(surface, n, &soul))
        .collect();
    if !unreachable.is_empty() {
        notes.push(format!(
            "SOUL.md names files in the folders of skill(s) {} by workspace paths, which do not \
             exist where Hermes Agent runs (its skills live in HERMES_HOME): run a helper from \
             its skill as ${{HERMES_SKILL_DIR}}/scripts/<file>, or ship it as scripts/<file>.py.",
            unreachable.join(", ")
        ));
    }
    if soul.is_empty() {
        notes.push(
            "The always-on files are empty, so no SOUL.md was written: Hermes Agent then runs \
             as its built-in assistant."
                .to_owned(),
        );
    } else {
        files.push(("SOUL.md".to_owned(), soul.clone()));
    }
    let memory_chars = parts.memory.as_ref().map_or(0, |m| m.chars().count());
    if let Some(m) = parts.memory {
        files.push(("memories/MEMORY.md".to_owned(), m));
        if surface != Surface::Hermes {
            notes.push(
                "memories/MEMORY.md holds the long-term memory as one entry (Hermes Agent \
                 separates entries with a line holding only §)."
                    .to_owned(),
            );
        }
    }
    let chars = soul.chars().count() + memory_chars;
    if chars > usize::try_from(tree::always_on_max(Surface::Hermes)).unwrap_or(usize::MAX) {
        notes.push(format!(
            "SOUL.md and MEMORY.md hold {chars} characters; Hermes Agent cuts SOUL.md down past \
             20,000 characters at a 64K-token window."
        ));
    }
    match surface {
        Surface::Opencode => notes.push(
            "SOUL.md joins systemprompt.md, guardrails.md and \
             tool_descriptions/tool_guidance.md in that order; LongTermMEMORY.md became \
             memories/MEMORY.md."
                .to_owned(),
        ),
        Surface::Codex => notes.push("SOUL.md is AGENTS.md.".to_owned()),
        Surface::Turnstone => notes.push(
            "SOUL.md joins systemprompt.md (the persona's base prompt) and guardrails.md (the \
             session instructions)."
                .to_owned(),
        ),
        Surface::Hermes => {}
    }
    if surface != Surface::Hermes && !skill_names(surface, src).is_empty() {
        notes.push(format!(
            "Trigger lines name {}; Hermes Agent loads a skill with its `skill_view` tool: \
             adjust them. Skill helpers now use ${{HERMES_SKILL_DIR}}.",
            skill_loader(surface)
        ));
    }
    notes.push(
        "SOUL.md, memories/ and skills/ go into the Hermes home (HERMES_HOME, ~/.hermes by \
         default); scripts/ goes into the project Hermes Agent works in."
            .to_owned(),
    );
    notes.push(
        "Hermes Agent replaces a whole context file when its prompt-injection scanner \
         matches, and ordinary wording can match: check SOUL.md and MEMORY.md with Hermes \
         Agent's own scanner before use."
            .to_owned(),
    );
    if skipped > 0 {
        notes.push(format!(
            "{skipped} file(s) outside the editable surface were left out."
        ));
    }
    Ok(bundle(ExportTarget::Hermes, label, files, notes))
}

/// A Turnstone bundle: `persona.md`, `instructions.md`, `skills/<name>/`, `scripts/` and
/// `IMPORT.md` (Turnstone keeps a harness in its database, so the bundle says how to apply it).
fn turnstone_bundle(label: &str, surface: Surface, src: &Tree) -> Result<ExportBundle> {
    let mut files: Vec<(String, String)> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut skipped = 0_usize;
    let mut bad_names: Vec<String> = Vec::new();
    let mut skills: Vec<String> = Vec::new();
    let mut helpers = false;
    let always_on = tree::always_on_files(surface);
    for (path, content) in src {
        if always_on.contains(&path.as_str()) {
            continue;
        }
        match tree::classify(surface, path) {
            Some(Component::Skills) => {
                let (name, rest) = skill_parts(surface, path);
                if let Some(problem) = tree::turnstone_name_problem(name) {
                    let entry = format!("{name} ({problem})");
                    if !bad_names.contains(&entry) {
                        bad_names.push(entry);
                    }
                    continue;
                }
                let content = move_skill_paths(surface, name, content, ExportTarget::Turnstone);
                if rest == format!("{name}/SKILL.md") {
                    if content.chars().count() > TURNSTONE_TEXT_MAX {
                        return Err(refuse(&format!(
                            "skill {name}'s SKILL.md is over Turnstone's {TURNSTONE_TEXT_MAX}-\
                             character cap"
                        )));
                    }
                    skills.push(name.to_owned());
                } else {
                    helpers = true;
                }
                files.push((format!("skills/{rest}"), content));
            }
            Some(Component::Scripts) => {
                helpers = true;
                files.push((path.clone(), content.clone()));
            }
            Some(c @ (Component::Subagents | Component::Plugins)) => {
                return Err(no_counterpart(surface, c, "Turnstone"));
            }
            _ => skipped += 1,
        }
    }
    if !bad_names.is_empty() {
        return Err(refuse(&format!(
            "Turnstone refuses these skill names (lowercase letters, digits and single hyphens \
             only): {}; rename the skills, or export as raw, opencode, codex or hermes",
            bad_names.join(", ")
        )));
    }
    let parts = split_always_on(surface, src);
    let names = skill_names(surface, src);
    let moved = |t: String| move_always_on_paths(surface, &names, t, ExportTarget::Turnstone);
    let Some(persona) = parts.identity.map(moved) else {
        return Err(refuse(
            "a Turnstone persona needs a base prompt, and this harness's identity text is empty",
        ));
    };
    if persona.chars().count() > TURNSTONE_TEXT_MAX {
        return Err(refuse(&format!(
            "the persona's base prompt would be {} characters, over Turnstone's \
             {TURNSTONE_TEXT_MAX}-character cap",
            persona.chars().count()
        )));
    }
    files.push(("persona.md".to_owned(), persona));
    let instructions = moved(
        parts
            .rest
            .into_iter()
            .chain(parts.memory)
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    let has_instructions = !instructions.is_empty();
    if has_instructions {
        files.push(("instructions.md".to_owned(), instructions));
    }
    let mut steps = vec![
        "1. `persona.md`: create a persona with this text as its base prompt (console: \
         Personas). It replaces Turnstone's built-in software-engineer prompt."
            .to_owned(),
    ];
    if has_instructions {
        steps.push(format!(
            "{}. `instructions.md`: put it in the `session.instructions` setting; Turnstone \
             appends it to the end of every system message.",
            steps.len() + 1
        ));
    }
    if !skills.is_empty() {
        steps.push(format!(
            "{}. `skills/<name>/SKILL.md`: add each as a named skill (console: Skills): {}. \
             The agent loads one with `skills(action=\"load\", name=\"<name>\")`; one is active \
             at a time.",
            steps.len() + 1,
            skills.join(", ")
        ));
    }
    if helpers {
        steps.push(format!(
            "{}. `scripts/` and `skills/<name>/scripts/`: copy them into the workspace the \
             node works in; the files run them by those paths.",
            steps.len() + 1
        ));
    }
    files.push((
        "IMPORT.md".to_owned(),
        format!(
            "# Turnstone bundle: harness {label}\n\nTurnstone keeps a harness in its database, \
             so apply this bundle by hand (or with a script against Turnstone's storage API):\n\n{}\n",
            steps.join("\n")
        ),
    ));
    let (persona_from, instructions_from) = match surface {
        Surface::Opencode => (
            "systemprompt.md",
            "guardrails.md, tool_descriptions/tool_guidance.md and LongTermMEMORY.md",
        ),
        Surface::Codex => ("AGENTS.md", ""),
        Surface::Hermes => ("SOUL.md", "memories/MEMORY.md"),
        Surface::Turnstone => ("", ""),
    };
    if surface != Surface::Turnstone {
        notes.push(if has_instructions {
            format!("persona.md is {persona_from}; instructions.md is {instructions_from}.")
        } else {
            format!("persona.md is {persona_from}.")
        });
    }
    if surface == Surface::Hermes && src.values().any(|c| c.contains(HERMES_SKILL_DIR)) {
        notes.push(
            "Skill helpers reached through ${HERMES_SKILL_DIR} now use skills/<name>/.".to_owned(),
        );
    }
    if surface != Surface::Turnstone && !skills.is_empty() {
        notes.push(format!(
            "Trigger lines name {}; Turnstone loads a skill with its `skills` tool: adjust \
             them.",
            skill_loader(surface)
        ));
    }
    notes.push("IMPORT.md says how to apply the bundle.".to_owned());
    if skipped > 0 {
        notes.push(format!(
            "{skipped} file(s) outside the editable surface were left out."
        ));
    }
    Ok(bundle(ExportTarget::Turnstone, label, files, notes))
}

fn bundle(
    target: ExportTarget,
    label: &str,
    mut files: Vec<(String, String)>,
    notes: Vec<String>,
) -> ExportBundle {
    files.sort_by(|a, b| pyfmt::cmp_paths(&a.0, &b.0));
    ExportBundle {
        target,
        label: label.to_owned(),
        files: files
            .into_iter()
            .map(|(path, content)| FileText { path, content })
            .collect(),
        notes,
    }
}

/// A deterministic `.tar.gz` of the bundle (sorted paths, mode 0644, mtime 0, top-level
/// directory `harness-<label>/`).
///
/// Owner and group are 0 with empty names; the gzip header carries no name and mtime 0.
/// Refused: an invalid label, an unsafe or duplicate path, a path with a control character,
/// and more than one harness's worth of files ([`CoreError::TooLarge`]).
pub fn tar_gz(bundle: &ExportBundle) -> Result<Vec<u8>> {
    names::harness_label(&bundle.label)?;
    check_size(
        bundle.files.len(),
        bundle.files.iter().map(|f| f.content.len()),
        1,
    )?;
    let mut files: Vec<&FileText> = bundle.files.iter().collect();
    files.sort_by(|a, b| pyfmt::cmp_paths(&a.path, &b.path));
    for pair in files.windows(2) {
        if let [a, b] = pair
            && a.path == b.path
        {
            return Err(CoreError::invalid("files", "a path appears twice"));
        }
    }
    let prefix = format!("harness-{}", bundle.label);
    let mut builder = tar::Builder::new(Vec::new());
    for f in files {
        names::relative_path(&f.path)?;
        if f.path.is_empty() {
            return Err(CoreError::invalid("files", "a file has an empty path"));
        }
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_username("")?;
        header.set_groupname("")?;
        header.set_size(u64::try_from(f.content.len()).unwrap_or(u64::MAX));
        builder.append_data(
            &mut header,
            format!("{prefix}/{}", f.path),
            f.content.as_bytes(),
        )?;
    }
    let tar_bytes = builder.into_inner()?;
    let mut gz = flate2::GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), flate2::Compression::default());
    gz.write_all(&tar_bytes)?;
    Ok(gz.finish()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2w_api::{Census, HarnessFile, HarnessRole};
    use regex::Regex;
    use std::io::Read;

    fn harness(surface: Surface, files: &[(&str, &str)]) -> HarnessTree {
        HarnessTree {
            label: "agg1".into(),
            role: HarnessRole::Final,
            surface,
            files: files
                .iter()
                .map(|(p, c)| HarnessFile {
                    path: (*p).into(),
                    component: tree::classify(surface, p).unwrap_or(Component::SystemPrompt),
                    bytes: c.len() as u64,
                    content: Some((*c).into()),
                })
                .collect(),
            census: Census::default(),
            fingerprint: "0123456789abcdef".into(),
        }
    }

    fn paths(b: &ExportBundle) -> Vec<&str> {
        b.files.iter().map(|f| f.path.as_str()).collect()
    }

    #[test]
    fn opencode_layouts() {
        let h = harness(
            Surface::Opencode,
            &[
                ("systemprompt.md", "Be careful."),
                ("guardrails.md", "Use the `doc` skill."),
                ("LongTermMEMORY.md", ""),
                (
                    "skills/doc/SKILL.md",
                    "---\nname: doc\ndescription: d\n---\n",
                ),
                ("skills/doc/scripts/x.py", "print(1)\n"),
                (
                    "sub_agents/check/AGENT.md",
                    "---\nmode: subagent\ndescription: c\n---\n",
                ),
                ("plugin/guard.ts", "export default {}\n"),
                ("scripts/sum.py", "print(2)\n"),
            ],
        );
        let raw = export(&h, ExportTarget::Raw).unwrap();
        assert_eq!(raw.files.len(), 8);
        let oc = export(&h, ExportTarget::Opencode).unwrap();
        assert_eq!(
            paths(&oc),
            vec![
                ".opencode/agent/check.md",
                ".opencode/plugin/guard.ts",
                ".opencode/skills/doc/SKILL.md",
                "AGENTS.md",
                "scripts/sum.py",
                "skills/doc/scripts/x.py",
            ]
        );
        let agents = oc.files.iter().find(|f| f.path == "AGENTS.md").unwrap();
        assert_eq!(agents.content, "Be careful.\n\nUse the `doc` skill.");
        assert!(oc.notes.iter().any(|n| n.contains("OpenCode 1.14")));
        assert!(matches!(
            export(&h, ExportTarget::Codex),
            Err(CoreError::Invalid { .. })
        ));
    }

    #[test]
    fn codex_layouts() {
        let h = harness(
            Surface::Codex,
            &[
                ("AGENTS.md", "Read .agents/skills/doc/SKILL.md when needed."),
                (".agents/skills/doc/SKILL.md", "---\ndescription: d\n---\n"),
                ("sub_agents/check/AGENT.md", "---\ndescription: c\n---\n"),
                (".codex/hooks.json", "{\"hooks\": {}}"),
            ],
        );
        let cx = export(&h, ExportTarget::Codex).unwrap();
        assert_eq!(
            paths(&cx),
            vec![
                ".agents/skills/doc/SKILL.md",
                ".codex/agents/check.md",
                ".codex/hooks.json",
                "AGENTS.md"
            ]
        );
        assert!(
            cx.notes
                .iter()
                .any(|n| n.contains("no native sub-agent loader"))
        );
        assert!(export(&h, ExportTarget::Opencode).is_err());
        let no_hooks = harness(
            Surface::Codex,
            &[
                ("AGENTS.md", "x"),
                (".agents/skills/doc/SKILL.md", "---\ndescription: d\n---\n"),
                ("sub_agents/check/AGENT.md", "---\ndescription: c\n---\n"),
            ],
        );
        let oc = export(&no_hooks, ExportTarget::Opencode).unwrap();
        assert!(oc.notes.iter().any(|n| n.contains("no `name: doc`")));
        assert!(oc.notes.iter().any(|n| n.contains("no `mode: subagent`")));
        assert!(oc.notes.iter().any(|n| n.contains("not rewritten")));
    }

    #[test]
    fn tar_gz_is_deterministic() {
        let h = harness(
            Surface::Opencode,
            &[("systemprompt.md", "a"), ("scripts/b.py", "print()\n")],
        );
        let b = export(&h, ExportTarget::Raw).unwrap();
        let one = tar_gz(&b).unwrap();
        let two = tar_gz(&b).unwrap();
        assert_eq!(one, two);
        let mut tar_bytes = Vec::new();
        flate2::read::GzDecoder::new(one.as_slice())
            .read_to_end(&mut tar_bytes)
            .unwrap();
        let mut ar = tar::Archive::new(tar_bytes.as_slice());
        let mut seen = Vec::new();
        for e in ar.entries().unwrap() {
            let e = e.unwrap();
            let h = e.header();
            assert_eq!(h.mode().unwrap(), 0o644);
            assert_eq!(h.mtime().unwrap(), 0);
            assert_eq!(h.uid().unwrap(), 0);
            seen.push(e.path().unwrap().to_string_lossy().into_owned());
        }
        assert_eq!(
            seen,
            vec!["harness-agg1/scripts/b.py", "harness-agg1/systemprompt.md"]
        );
        let mut bad = b.clone();
        bad.files[0].path = "../evil".into();
        assert!(tar_gz(&bad).is_err());
        let mut dup = b;
        dup.files[1].path = dup.files[0].path.clone();
        assert!(tar_gz(&dup).is_err());
    }

    /// A file name carrying a terminal escape sequence never reaches an export.
    #[test]
    fn names_with_control_characters_are_refused() {
        let h = harness(
            Surface::Opencode,
            &[
                ("systemprompt.md", "a"),
                ("scripts/x\u{1b}]0;pwned\u{7}.py", ""),
            ],
        );
        for target in [ExportTarget::Raw, ExportTarget::Opencode] {
            let err = export(&h, target).unwrap_err();
            assert!(err.to_string().contains("control characters"), "{err}");
        }
        let clean = harness(Surface::Opencode, &[("systemprompt.md", "a")]);
        let mut b = export(&clean, ExportTarget::Raw).unwrap();
        b.files[0].path = "a\u{9b}31m.md".into();
        assert!(
            tar_gz(&b)
                .unwrap_err()
                .to_string()
                .contains("control characters")
        );
    }

    fn file<'a>(b: &'a ExportBundle, path: &str) -> &'a str {
        b.files.iter().find(|f| f.path == path).map_or_else(
            || panic!("no {path} in {:?}", paths(b)),
            |f| f.content.as_str(),
        )
    }

    #[test]
    fn a_hermes_tree_exports_to_every_framework() {
        let skill = "---\nname: memo-check\ndescription: Check a memo\n---\n\
                     Run python3 ${HERMES_SKILL_DIR}/scripts/check.py output/memo.docx\n";
        let h = harness(
            Surface::Hermes,
            &[
                (
                    "SOUL.md",
                    "Role. Load the `memo-check` skill with skill_view.",
                ),
                ("memories/MEMORY.md", "Clients prefer short memos."),
                ("skills/memo-check/SKILL.md", skill),
                ("skills/memo-check/scripts/check.py", "print(1)\n"),
                ("scripts/sum.py", "print(2)\n"),
            ],
        );
        assert_eq!(export(&h, ExportTarget::Raw).unwrap().files.len(), 5);
        // OpenCode and Codex: the helper path becomes the path the project runs it by
        // (OpenCode: the workspace's skills/<name>/, as harness2's runner lays it out).
        let oc = export(&h, ExportTarget::Opencode).unwrap();
        assert_eq!(
            paths(&oc),
            vec![
                ".opencode/skills/memo-check/SKILL.md",
                "AGENTS.md",
                "scripts/sum.py",
                "skills/memo-check/scripts/check.py",
            ]
        );
        assert!(
            file(&oc, ".opencode/skills/memo-check/SKILL.md")
                .contains("python3 skills/memo-check/scripts/check.py")
        );
        assert_eq!(
            file(&oc, "AGENTS.md"),
            "Role. Load the `memo-check` skill with skill_view.\n\nClients prefer short memos."
        );
        assert!(
            oc.notes.iter().any(|n| n.contains("`skill_view`")),
            "{:?}",
            oc.notes
        );
        let cx = export(&h, ExportTarget::Codex).unwrap();
        assert!(
            file(&cx, ".agents/skills/memo-check/SKILL.md")
                .contains("python3 .agents/skills/memo-check/scripts/check.py")
        );
        // Its own framework: the home as it is.
        let hm = export(&h, ExportTarget::Hermes).unwrap();
        assert_eq!(file(&hm, "skills/memo-check/SKILL.md"), skill);
        assert_eq!(
            file(&hm, "memories/MEMORY.md"),
            "Clients prefer short memos."
        );
        assert!(hm.notes.iter().any(|n| n.contains("HERMES_HOME")));
        // Turnstone: persona, instructions, helpers from the workspace, and how to apply it.
        let ts = export(&h, ExportTarget::Turnstone).unwrap();
        assert_eq!(
            paths(&ts),
            vec![
                "IMPORT.md",
                "instructions.md",
                "persona.md",
                "scripts/sum.py",
                "skills/memo-check/SKILL.md",
                "skills/memo-check/scripts/check.py",
            ]
        );
        assert_eq!(
            file(&ts, "persona.md"),
            "Role. Load the `memo-check` skill with skill_view."
        );
        assert_eq!(file(&ts, "instructions.md"), "Clients prefer short memos.");
        assert!(
            file(&ts, "skills/memo-check/SKILL.md")
                .contains("python3 skills/memo-check/scripts/check.py")
        );
        let import = file(&ts, "IMPORT.md");
        for step in [
            "persona.md",
            "session.instructions",
            "memo-check",
            "scripts/",
        ] {
            assert!(import.contains(step), "{step}: {import}");
        }
    }

    #[test]
    fn an_opencode_tree_exports_to_hermes_and_turnstone() {
        let skill = "---\nname: doc\ndescription: d\n---\nRun python3 skills/doc/scripts/x.py\n";
        let h = harness(
            Surface::Opencode,
            &[
                ("systemprompt.md", "Role."),
                ("guardrails.md", "Use the `doc` skill."),
                ("LongTermMEMORY.md", "Fact."),
                ("tool_descriptions/tool_guidance.md", "Tools."),
                ("skills/doc/SKILL.md", skill),
                ("skills/doc/scripts/x.py", "print(1)\n"),
            ],
        );
        let hm = export(&h, ExportTarget::Hermes).unwrap();
        assert_eq!(
            file(&hm, "SOUL.md"),
            "Role.\n\nUse the `doc` skill.\n\nTools."
        );
        assert_eq!(file(&hm, "memories/MEMORY.md"), "Fact.");
        assert!(
            file(&hm, "skills/doc/SKILL.md").contains("python3 ${HERMES_SKILL_DIR}/scripts/x.py")
        );
        assert!(
            hm.notes
                .iter()
                .any(|n| n.contains("OpenCode's `skill` tool"))
        );
        let ts = export(&h, ExportTarget::Turnstone).unwrap();
        assert_eq!(file(&ts, "persona.md"), "Role.");
        assert_eq!(
            file(&ts, "instructions.md"),
            "Use the `doc` skill.\n\nTools.\n\nFact."
        );
        assert_eq!(file(&ts, "skills/doc/SKILL.md"), skill);
        // A Codex tree's helper path moves to the Turnstone workspace path.
        let cx = harness(
            Surface::Codex,
            &[
                ("AGENTS.md", "Role."),
                (
                    ".agents/skills/doc/SKILL.md",
                    "---\ndescription: d\n---\nRun python3 .agents/skills/doc/scripts/x.py\n",
                ),
            ],
        );
        let ts = export(&cx, ExportTarget::Turnstone).unwrap();
        assert!(file(&ts, "skills/doc/SKILL.md").contains("python3 skills/doc/scripts/x.py"));
        let hm = export(&cx, ExportTarget::Hermes).unwrap();
        assert!(
            hm.notes.iter().any(|n| n.contains("no `name: doc`")),
            "{:?}",
            hm.notes
        );
    }

    /// Every framework's tree to every layout: the identity text and the memory land where the
    /// target keeps them, and every helper a skill runs is in the bundle at the path it uses.
    #[test]
    fn every_framework_exports_to_every_layout() {
        const ROLE: &str = "You review employment documents. Load the `memo-check` skill.";
        const FACT: &str = "Clients prefer short memos.";
        let fm = "---\nname: memo-check\ndescription: Check a memo\n---\n";
        let oc_skill = format!("{fm}Run python3 skills/memo-check/scripts/check.py\n");
        let cx_skill = format!("{fm}Run python3 .agents/skills/memo-check/scripts/check.py\n");
        let hm_skill = format!("{fm}Run python3 ${{HERMES_SKILL_DIR}}/scripts/check.py\n");
        let sources = [
            harness(
                Surface::Opencode,
                &[
                    ("systemprompt.md", ROLE),
                    ("LongTermMEMORY.md", FACT),
                    ("skills/memo-check/SKILL.md", &oc_skill),
                    ("skills/memo-check/scripts/check.py", "print(1)\n"),
                    ("scripts/sum.py", "print(2)\n"),
                ],
            ),
            harness(
                Surface::Codex,
                &[
                    ("AGENTS.md", ROLE),
                    (".agents/skills/memo-check/SKILL.md", &cx_skill),
                    (".agents/skills/memo-check/scripts/check.py", "print(1)\n"),
                    ("scripts/sum.py", "print(2)\n"),
                ],
            ),
            harness(
                Surface::Hermes,
                &[
                    ("SOUL.md", ROLE),
                    ("memories/MEMORY.md", FACT),
                    ("skills/memo-check/SKILL.md", &hm_skill),
                    ("skills/memo-check/scripts/check.py", "print(1)\n"),
                    ("scripts/sum.py", "print(2)\n"),
                ],
            ),
            harness(
                Surface::Turnstone,
                &[
                    ("systemprompt.md", ROLE),
                    ("guardrails.md", FACT),
                    ("skills/memo-check/SKILL.md", &oc_skill),
                    ("skills/memo-check/scripts/check.py", "print(1)\n"),
                    ("scripts/sum.py", "print(2)\n"),
                ],
            ),
        ];
        let python = Regex::new(r"python3 (\S+)").unwrap();
        for src in &sources {
            for target in ExportTarget::ALL {
                let b = export(src, target)
                    .unwrap_or_else(|e| panic!("{:?} -> {target:?}: {e}", src.surface));
                let at = format!("{:?} -> {target:?}", src.surface);
                let has = |p: &str| b.files.iter().any(|f| f.path == p);
                // Where the identity text and the memory go.
                let (identity, memory) = match target {
                    ExportTarget::Raw => {
                        assert_eq!(b.files.len(), src.files.len(), "{at}");
                        continue;
                    }
                    ExportTarget::Opencode | ExportTarget::Codex => ("AGENTS.md", "AGENTS.md"),
                    ExportTarget::Hermes => ("SOUL.md", "memories/MEMORY.md"),
                    ExportTarget::Turnstone => ("persona.md", "instructions.md"),
                };
                assert!(file(&b, identity).starts_with(ROLE), "{at}: {identity}");
                let memory = match src.surface {
                    // Codex's AGENTS.md has no memory; Turnstone's guardrails.md is further
                    // instructions, which Hermes Agent keeps in SOUL.md.
                    Surface::Codex => None,
                    Surface::Turnstone if target == ExportTarget::Hermes => Some(identity),
                    _ => Some(memory),
                };
                if let Some(m) = memory {
                    assert!(file(&b, m).contains(FACT), "{at}: {m}");
                }
                assert!(has("scripts/sum.py"), "{at}: {:?}", paths(&b));
                // Every helper a skill runs ships at the path it is run by.
                let skills: Vec<_> = b
                    .files
                    .iter()
                    .filter(|f| f.path.ends_with("/SKILL.md"))
                    .collect();
                assert_eq!(skills.len(), 1, "{at}: {:?}", paths(&b));
                for f in skills {
                    let dir = f.path.trim_end_matches("SKILL.md");
                    let runs: Vec<String> = python
                        .captures_iter(&f.content)
                        .map(|c| c[1].replace("${HERMES_SKILL_DIR}/", dir))
                        .collect();
                    assert_eq!(runs.len(), 1, "{at}: {}", f.content);
                    for r in runs {
                        assert!(has(&r), "{at}: {} runs {r}, not in {:?}", f.path, paths(&b));
                    }
                }
            }
        }
    }

    /// Paths to skill files in the always-on text move with the skills, except in a Hermes
    /// Agent home, where nothing reaches a skill folder from SOUL.md (a note says so).
    #[test]
    fn always_on_paths_follow_the_skills() {
        let md = "---\nname: doc\ndescription: d\n---\nbody\n";
        let oc = harness(
            Surface::Opencode,
            &[
                (
                    "systemprompt.md",
                    "Run python3 skills/doc/scripts/x.py; read skills/doc/SKILL.md.",
                ),
                ("skills/doc/SKILL.md", md),
                ("skills/doc/scripts/x.py", "print(1)\n"),
            ],
        );
        assert_eq!(
            file(&export(&oc, ExportTarget::Codex).unwrap(), "AGENTS.md"),
            "Run python3 .agents/skills/doc/scripts/x.py; read .agents/skills/doc/SKILL.md."
        );
        assert_eq!(
            file(&export(&oc, ExportTarget::Turnstone).unwrap(), "persona.md"),
            "Run python3 skills/doc/scripts/x.py; read skills/doc/SKILL.md."
        );
        let hm = export(&oc, ExportTarget::Hermes).unwrap();
        assert_eq!(
            file(&hm, "SOUL.md"),
            "Run python3 skills/doc/scripts/x.py; read skills/doc/SKILL.md."
        );
        assert!(
            hm.notes.iter().any(|n| n.contains("skill(s) doc")),
            "{:?}",
            hm.notes
        );
        let cx = harness(
            Surface::Codex,
            &[
                (
                    "AGENTS.md",
                    "Read .agents/skills/doc/SKILL.md, then run python3 \
                     .agents/skills/doc/scripts/x.py.",
                ),
                (".agents/skills/doc/SKILL.md", md),
                (".agents/skills/doc/scripts/x.py", "print(1)\n"),
            ],
        );
        assert_eq!(
            file(&export(&cx, ExportTarget::Opencode).unwrap(), "AGENTS.md"),
            "Read .opencode/skills/doc/SKILL.md, then run python3 skills/doc/scripts/x.py."
        );
        assert_eq!(
            file(&export(&cx, ExportTarget::Turnstone).unwrap(), "persona.md"),
            "Read skills/doc/SKILL.md, then run python3 skills/doc/scripts/x.py."
        );
    }

    /// Notes name only what the bundle holds: no trigger-line advice without skills, no
    /// instructions.md when nothing went into it (the shapes of two real e2e harnesses).
    #[test]
    fn notes_name_only_what_the_bundle_holds() {
        let ts = harness(
            Surface::Turnstone,
            &[
                ("systemprompt.md", "Role."),
                ("guardrails.md", ""),
                ("scripts/md_to_docx.py", "print(1)\n"),
            ],
        );
        for target in [
            ExportTarget::Opencode,
            ExportTarget::Codex,
            ExportTarget::Hermes,
        ] {
            let b = export(&ts, target).unwrap();
            assert!(
                !b.notes.iter().any(|n| n.contains("Trigger lines")),
                "{target:?}: {:?}",
                b.notes
            );
        }
        let hm = harness(
            Surface::Hermes,
            &[("SOUL.md", "Role."), ("memories/MEMORY.md", "")],
        );
        let b = export(&hm, ExportTarget::Turnstone).unwrap();
        assert_eq!(paths(&b), vec!["IMPORT.md", "persona.md"]);
        assert!(
            b.notes.iter().any(|n| n == "persona.md is SOUL.md."),
            "{:?}",
            b.notes
        );
        assert!(
            !b.notes
                .iter()
                .any(|n| n.contains("instructions.md") || n.contains("HERMES_SKILL_DIR"))
        );
    }

    #[test]
    fn what_a_framework_cannot_hold_is_refused() {
        let err = |h: &HarnessTree, t: ExportTarget| export(h, t).unwrap_err().to_string();
        let with_plugin = harness(
            Surface::Opencode,
            &[
                ("systemprompt.md", "Role."),
                ("plugin/guard.ts", "export default {}\n"),
            ],
        );
        for t in [ExportTarget::Hermes, ExportTarget::Turnstone] {
            assert!(err(&with_plugin, t).contains("OpenCode plugins"), "{t:?}");
        }
        let with_hooks = harness(
            Surface::Codex,
            &[
                ("AGENTS.md", "Role."),
                (".codex/hooks.json", "{\"hooks\": {}}"),
            ],
        );
        assert!(err(&with_hooks, ExportTarget::Hermes).contains("Codex hooks"));
        let with_agents = harness(
            Surface::Opencode,
            &[
                ("systemprompt.md", "Role."),
                (
                    "sub_agents/check/AGENT.md",
                    "---\nmode: subagent\ndescription: c\n---\n",
                ),
            ],
        );
        assert!(
            err(&with_agents, ExportTarget::Turnstone).contains("no sub-agents defined in files")
        );
        let underscored = harness(
            Surface::Opencode,
            &[
                ("systemprompt.md", "Role."),
                (
                    "skills/doc_check/SKILL.md",
                    "---\nname: doc_check\ndescription: d\n---\n",
                ),
            ],
        );
        let e = err(&underscored, ExportTarget::Turnstone);
        assert!(e.contains("doc_check (contains an underscore)"), "{e}");
        assert!(export(&underscored, ExportTarget::Hermes).is_ok());
        let no_identity = harness(
            Surface::Turnstone,
            &[("systemprompt.md", " "), ("guardrails.md", "Rules.")],
        );
        assert!(err(&no_identity, ExportTarget::Turnstone).contains("needs a base prompt"));
    }

    #[test]
    fn paths_are_moved_only_where_they_start() {
        let text = "Run skills/a/x.py, not .opencode/skills/a/y.py or myskills/a/z.";
        assert_eq!(
            replace_path_prefix(text, "skills/a/", "${HERMES_SKILL_DIR}/"),
            "Run ${HERMES_SKILL_DIR}/x.py, not .opencode/skills/a/y.py or myskills/a/z."
        );
        assert_eq!(replace_path_prefix("skills/a/b", "skills/a/", "Z/"), "Z/b");
    }

    #[test]
    fn exports_take_at_most_one_harness() {
        let paths: Vec<String> = (0..=tree::MAX_TREE_FILES)
            .map(|i| format!("scripts/f{i}.py"))
            .collect();
        let files: Vec<(&str, &str)> = paths.iter().map(|p| (p.as_str(), "x")).collect();
        let err = export(&harness(Surface::Opencode, &files), ExportTarget::Raw).unwrap_err();
        assert!(matches!(err, CoreError::TooLarge(_)), "{err}");
        let fits = harness(Surface::Opencode, &files[1..]);
        assert_eq!(
            export(&fits, ExportTarget::Raw).unwrap().files.len(),
            tree::MAX_TREE_FILES
        );
        // Past the cap and the room the AGENTS.md join may add.
        let big = "x".repeat(usize::try_from(tree::MAX_TREE_BYTES).unwrap() + 65);
        let err = export(
            &harness(Surface::Opencode, &[("systemprompt.md", &big)]),
            ExportTarget::Raw,
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::TooLarge(_)), "{err}");
        let mut b = export(
            &harness(Surface::Opencode, &[("systemprompt.md", "a")]),
            ExportTarget::Raw,
        )
        .unwrap();
        b.files[0].content = big;
        assert!(matches!(tar_gz(&b), Err(CoreError::TooLarge(_))));
    }
}
