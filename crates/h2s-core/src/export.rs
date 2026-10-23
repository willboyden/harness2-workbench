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

use std::io::Write;

use h2s_api::{Component, ExportBundle, ExportTarget, FileText, HarnessTree, Surface};

use crate::tree::{self, Tree};
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

/// Build the bundle for `target`. Fails when the tree's surface cannot express `target`
/// faithfully (e.g. OpenCode plugins exported to Codex) unless the problem is only a note.
///
/// Refused: OpenCode plugins to Codex, and Codex hooks to OpenCode (each acts inside the
/// tool loop and has no counterpart). Notes cover sub-agents on Codex, frontmatter OpenCode
/// requires but a Codex tree may lack, the AGENTS.md size on Codex, and plugin versions.
pub fn export(tree: &HarnessTree, target: ExportTarget) -> Result<ExportBundle> {
    names::harness_label(&tree.label)?;
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

    let skill_dir = tree::skill_dir(surface);
    let always_on = tree::always_on_files(surface);
    let mut skipped = 0_usize;
    let mut has_plugins = false;
    let mut sub_agents: Vec<String> = Vec::new();
    for (path, content) in &src {
        if always_on.contains(&path.as_str()) {
            continue;
        }
        match tree::classify(surface, path) {
            Some(Component::Skills) => {
                let rest = path
                    .strip_prefix(skill_dir)
                    .and_then(|r| r.strip_prefix('/'))
                    .unwrap_or(path);
                let dest = match target {
                    ExportTarget::Opencode => format!(".opencode/skills/{rest}"),
                    _ => format!(".agents/skills/{rest}"),
                };
                if target == ExportTarget::Opencode
                    && surface == Surface::Codex
                    && let Some(name) = tree::match_skill(surface, path)
                    && pyfmt::name_field(pyfmt::first_chars(content, 600)) != Some(name)
                {
                    notes.push(format!(
                        "Skill {name} has no `name: {name}` frontmatter, which OpenCode \
                         requires (it drops such skills silently); add it before use."
                    ));
                }
                files.push((dest, content.clone()));
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

    let agents = joined_always_on(surface, &src);
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
    if surface == Surface::Opencode {
        notes.push(
            "AGENTS.md joins systemprompt.md, guardrails.md, LongTermMEMORY.md and \
             tool_descriptions/tool_guidance.md in that order."
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
            "Text that names component paths (for example a skill's SKILL.md path) was not \
             rewritten for the other framework."
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
/// Refused: an invalid label, an unsafe or duplicate path.
pub fn tar_gz(bundle: &ExportBundle) -> Result<Vec<u8>> {
    names::harness_label(&bundle.label)?;
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
    use h2s_api::{Census, HarnessFile, HarnessRole};
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
                ".opencode/skills/doc/scripts/x.py",
                "AGENTS.md",
                "scripts/sum.py",
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
}
