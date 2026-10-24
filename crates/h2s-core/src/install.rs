//! Managed installation of harness2: the steps, and reading what is installed.
//!
//! Install = `git init` + `git fetch --depth 1 origin <commit>` + `git checkout --detach
//! FETCH_HEAD` (the commit id is the integrity check), `uv venv --python 3.12 .venv`,
//! `uv pip install --python .venv/bin/python --exclude-newer <cutoff> -e .[render(,bridge)]`,
//! then an import check.
//!
//! harness2 has no lock file, so its dependencies are resolved at install time; the cutoff
//! ([`crate::TESTED_HARNESS2_DEPS_CUTOFF`], the tested commit's date) keeps that resolution
//! to packages uploaded before the tested commit. It applies to every commit: one that
//! needs newer packages fails to install instead of pulling releases nobody tested.

use std::path::Path;

use cap_std::fs::Dir;
use h2s_api::{Harness2Status, InstallExtra};

use crate::safefs::{self, EntryKind};
use crate::{
    CoreError, HARNESS2_PYTHON, HARNESS2_REPO_URL, Result, TESTED_HARNESS2_DEPS_CUTOFF, names,
};

/// One install step: run `program args...` in `cwd` (absolute) with no shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// `init`, `remote`, `fetch`, `checkout`, `verify-commit`, `venv`, `install`, `import`.
    pub name: &'static str,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
}

fn step(name: &'static str, program: &str, args: &[&str], cwd: &str) -> Step {
    Step {
        name,
        program: program.to_owned(),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        cwd: cwd.to_owned(),
    }
}

/// An absolute directory path without `..`, control characters or a trailing `/`, and not
/// the filesystem root.
fn home_ok(home: &str) -> bool {
    home.starts_with('/')
        && home.len() > 1
        && home.len() <= 4096
        && !home.ends_with('/')
        && !home.chars().any(char::is_control)
        && !home.split('/').any(|c| c == ".." || c == ".")
}

fn open_home(home: &Path) -> Option<Dir> {
    Dir::open_ambient_dir(home, cap_std::ambient_authority()).ok()
}

/// The commit `.git/HEAD` names: a detached id, or a branch ref followed one level
/// (loose ref file, else `packed-refs`).
pub(crate) fn read_head(home: &Dir) -> Option<String> {
    let head = safefs::read_text_opt(home, ".git/HEAD", 4096, "HEAD").ok()??;
    let head = head.trim();
    if names::commit(head).is_ok() {
        return Some(head.to_owned());
    }
    let reference = head.strip_prefix("ref: ")?.trim();
    if names::relative_path(reference).is_err() || !reference.starts_with("refs/") {
        return None;
    }
    if let Ok(Some(id)) = safefs::read_text_opt(home, &format!(".git/{reference}"), 4096, "ref") {
        let id = id.trim();
        return names::commit(id).is_ok().then(|| id.to_owned());
    }
    let packed =
        safefs::read_text_opt(home, ".git/packed-refs", 4 * 1024 * 1024, "packed-refs").ok()??;
    packed.lines().find_map(|l| {
        let (id, name) = l.split_once(' ')?;
        (name == reference && names::commit(id).is_ok()).then(|| id.to_owned())
    })
}

/// The ordered steps to install `commit` into `home` (absolute; must not exist or be empty
/// unless `reinstall`, in which case the venv step recreates `.venv` and the git steps are
/// skipped when HEAD already equals `commit`).
///
/// A reinstall over a checkout at another commit fetches and checks out `commit` in place;
/// a non-empty directory that is not a git checkout is refused. `verify-commit` exits
/// non-zero unless HEAD contains `commit` (in the depth-1 clone: is that commit).
pub fn plan(
    home: &str,
    commit: &str,
    extras: &[InstallExtra],
    reinstall: bool,
) -> Result<Vec<Step>> {
    if !home_ok(home) {
        return Err(CoreError::invalid(
            "home",
            "must be an absolute directory path without '..', '.', control characters or a \
             trailing '/'",
        ));
    }
    names::commit(commit)?;
    let parent = Path::new(home)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| p.starts_with('/'))
        .ok_or_else(|| CoreError::invalid("home", "needs a parent directory"))?;
    let dir = open_home(Path::new(home));
    let entries = match &dir {
        Some(d) => safefs::list(d)?,
        None => Vec::new(),
    };
    let has_git = entries
        .iter()
        .any(|e| e.name == ".git" && e.kind == EntryKind::Dir);
    let head = dir.as_ref().and_then(read_head);
    if !reinstall && !entries.is_empty() {
        return Err(CoreError::Refused(
            "the harness2 directory is not empty; reinstall or choose another directory".into(),
        ));
    }
    if reinstall && !entries.is_empty() && !has_git {
        return Err(CoreError::Refused(
            "the harness2 directory is not a git checkout; move it aside first".into(),
        ));
    }
    let mut steps = Vec::new();
    if has_git {
        if head.as_deref() != Some(commit) {
            steps.push(step(
                "fetch",
                "git",
                &["fetch", "--depth", "1", HARNESS2_REPO_URL, commit],
                home,
            ));
            steps.push(step(
                "checkout",
                "git",
                &["checkout", "-q", "--detach", "FETCH_HEAD"],
                home,
            ));
            steps.push(step(
                "verify-commit",
                "git",
                &["merge-base", "--is-ancestor", commit, "HEAD"],
                home,
            ));
        }
    } else {
        steps.push(step("init", "git", &["init", "-q", home], &parent));
        steps.push(step(
            "remote",
            "git",
            &["remote", "add", "origin", HARNESS2_REPO_URL],
            home,
        ));
        steps.push(step(
            "fetch",
            "git",
            &["fetch", "--depth", "1", "origin", commit],
            home,
        ));
        steps.push(step(
            "checkout",
            "git",
            &["checkout", "-q", "--detach", "FETCH_HEAD"],
            home,
        ));
        steps.push(step(
            "verify-commit",
            "git",
            &["merge-base", "--is-ancestor", commit, "HEAD"],
            home,
        ));
    }
    let mut venv = vec!["venv"];
    if reinstall {
        venv.push("--clear");
    }
    venv.extend(["--python", HARNESS2_PYTHON, ".venv"]);
    steps.push(step("venv", "uv", &venv, home));
    let bridge = extras.contains(&InstallExtra::Bridge);
    let target = if bridge {
        ".[render,bridge]"
    } else {
        ".[render]"
    };
    steps.push(step(
        "install",
        "uv",
        &[
            "pip",
            "install",
            "--python",
            ".venv/bin/python",
            "--exclude-newer",
            TESTED_HARNESS2_DEPS_CUTOFF,
            "-e",
            target,
        ],
        home,
    ));
    let mut imports = String::from(
        "import harness2, harness2.cli.run_sweep, harness2.cli.judge, harness2.cli.report, \
         harness2.cli.demo, harness2.cli.setup, harness2.cli.check, openpyxl, pptx",
    );
    if bridge {
        imports.push_str(", litellm");
    }
    steps.push(Step {
        name: "import",
        program: format!("{home}/.venv/bin/python"),
        args: vec!["-I".into(), "-c".into(), imports],
        cwd: home.to_owned(),
    });
    Ok(steps)
}

/// The `site-packages` directory of the venv (`.venv/lib/python3.*/site-packages`).
fn site_packages(home: &Dir) -> Option<String> {
    let libs = safefs::list_opt(home, ".venv/lib", "venv").ok()?;
    libs.into_iter()
        .filter(|e| e.kind == EntryKind::Dir && e.name.starts_with("python3"))
        .map(|e| format!(".venv/lib/{}/site-packages", e.name))
        .find(|p| safefs::is_dir(home, p))
}

/// The most precise of `version_info` / `version` in `pyvenv.cfg` (uv writes `3.12`, the
/// `venv` module `3.12.14`).
fn pyvenv_version(cfg: &str) -> Option<String> {
    let mut best: Option<String> = None;
    for line in cfg.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let (k, v) = (k.trim(), v.trim());
            if (k == "version_info" || k == "version") && !v.is_empty() {
                let better = best
                    .as_ref()
                    .is_none_or(|b| v.matches('.').count() > b.matches('.').count());
                if better {
                    best = Some(v.to_owned());
                }
            }
        }
    }
    best
}

fn pkg_version(home: &Dir, dist_info: Option<&str>) -> Option<String> {
    if let Ok(Some(info)) =
        safefs::read_text_opt(home, "harness2.egg-info/PKG-INFO", 1024 * 1024, "PKG-INFO")
        && let Some(v) = info.lines().find_map(|l| l.strip_prefix("Version: "))
    {
        return Some(v.trim().to_owned());
    }
    if let Some(v) = dist_info
        .and_then(|d| d.strip_prefix("harness2-"))
        .and_then(|d| d.strip_suffix(".dist-info"))
    {
        return Some(v.to_owned());
    }
    let pyproject =
        safefs::read_text_opt(home, "pyproject.toml", 1024 * 1024, "pyproject.toml").ok()??;
    let mut in_project = false;
    for line in pyproject.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_project = t == "[project]";
        } else if in_project
            && let Some(rest) = t.strip_prefix("version")
            && let Some(v) = rest.trim_start().strip_prefix('=')
        {
            return Some(v.trim().trim_matches('"').trim_matches('\'').to_owned());
        }
    }
    None
}

/// What is installed at `home`, from files only (no processes): `.git/HEAD` (detached
/// commit or a ref followed one level), `harness2.egg-info/PKG-INFO` / `pyproject.toml`
/// for the version, `.venv/pyvenv.cfg` for the Python version, installed extras detected
/// from the venv's site-packages (`openpyxl` -> render, `litellm` -> bridge).
///
/// `installed` means the checkout's commit is known, the venv exists and harness2 is
/// installed into it (a `harness2-*.dist-info` in its site-packages). Symlinks inside the
/// checkout are not followed.
#[must_use]
pub fn read_status(home: &Path, managed: bool) -> Harness2Status {
    let mut status = Harness2Status {
        installed: false,
        managed,
        commit: None,
        tested_commit: crate::TESTED_HARNESS2_COMMIT.to_owned(),
        version: None,
        python: None,
        extras: Vec::new(),
        home: Some(home.to_string_lossy().into_owned()),
    };
    let Some(dir) = open_home(home) else {
        return status;
    };
    status.commit = read_head(&dir);
    status.python = safefs::read_text_opt(&dir, ".venv/pyvenv.cfg", 64 * 1024, "pyvenv.cfg")
        .ok()
        .flatten()
        .and_then(|cfg| pyvenv_version(&cfg));
    let site = site_packages(&dir);
    let entries = site
        .as_deref()
        .map(|s| safefs::list_opt(&dir, s, "site-packages").unwrap_or_default())
        .unwrap_or_default();
    let dist_info = entries
        .iter()
        .find(|e| e.name.starts_with("harness2-") && e.name.ends_with(".dist-info"))
        .map(|e| e.name.clone());
    status.version = pkg_version(&dir, dist_info.as_deref());
    if entries.iter().any(|e| e.name == "openpyxl") {
        status.extras.push(InstallExtra::Render);
    }
    if entries.iter().any(|e| e.name == "litellm") {
        status.extras.push(InstallExtra::Bridge);
    }
    status.installed = status.commit.is_some() && status.python.is_some() && dist_info.is_some();
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: &str = crate::TESTED_HARNESS2_COMMIT;

    #[test]
    fn fresh_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("harness2");
        let home = home.to_str().unwrap();
        let steps = plan(home, C, &[InstallExtra::Bridge], false).unwrap();
        let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec![
                "init",
                "remote",
                "fetch",
                "checkout",
                "verify-commit",
                "venv",
                "install",
                "import"
            ]
        );
        assert_eq!(steps[0].args, vec!["init", "-q", home]);
        assert_eq!(steps[0].cwd, tmp.path().to_str().unwrap());
        assert_eq!(steps[2].args, vec!["fetch", "--depth", "1", "origin", C]);
        assert_eq!(steps[5].args, vec!["venv", "--python", "3.12", ".venv"]);
        assert_eq!(
            steps[6].args,
            vec![
                "pip",
                "install",
                "--python",
                ".venv/bin/python",
                "--exclude-newer",
                "2026-10-14T02:22:49Z",
                "-e",
                ".[render,bridge]"
            ]
        );
        assert!(steps[7].args[2].ends_with(", litellm"));
        assert!(plan("relative", C, &[], false).is_err());
        assert!(plan(home, "abc", &[], false).is_err());
        assert!(plan("/", C, &[], false).is_err());
    }

    /// The cutoff is the tested commit's committer time in UTC: `git show -s --format=%cI
    /// 1cfe254` gives 2026-10-13T21:22:49-05:00.
    #[test]
    fn deps_cutoff_is_the_tested_commit_time_in_utc() {
        let committed = time::OffsetDateTime::parse(
            "2026-10-13T21:22:49-05:00",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let cutoff = time::OffsetDateTime::parse(
            crate::TESTED_HARNESS2_DEPS_CUTOFF,
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        assert_eq!(committed, cutoff);
        assert_eq!(cutoff.offset(), time::UtcOffset::UTC);
    }

    #[test]
    fn reinstall_and_status() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("h");
        let p = |rel: &str| home.join(rel);
        std::fs::create_dir_all(p(".git/refs/heads")).unwrap();
        std::fs::write(p(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(p(".git/refs/heads/main"), format!("{C}\n")).unwrap();
        std::fs::create_dir_all(p(".venv/lib/python3.12/site-packages/openpyxl")).unwrap();
        std::fs::create_dir_all(p(
            ".venv/lib/python3.12/site-packages/harness2-0.1.0.dist-info",
        ))
        .unwrap();
        std::fs::write(
            p(".venv/pyvenv.cfg"),
            "home = /x\nversion_info = 3.12.14\ninclude-system-site-packages = false\n",
        )
        .unwrap();
        std::fs::write(
            p("pyproject.toml"),
            "[project]\nname = \"harness2\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let h = home.to_str().unwrap();
        assert!(matches!(plan(h, C, &[], false), Err(CoreError::Refused(_))));
        let steps = plan(h, C, &[], true).unwrap();
        let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["venv", "install", "import"]);
        assert_eq!(
            steps[0].args,
            vec!["venv", "--clear", "--python", "3.12", ".venv"]
        );
        assert_eq!(
            &steps[1].args[4..],
            [
                "--exclude-newer",
                crate::TESTED_HARNESS2_DEPS_CUTOFF,
                "-e",
                ".[render]"
            ]
        );
        let other = "0".repeat(40);
        let steps = plan(h, &other, &[], true).unwrap();
        assert_eq!(steps[0].name, "fetch");
        assert_eq!(
            steps[2].args,
            vec!["merge-base", "--is-ancestor", other.as_str(), "HEAD"]
        );
        // The cutoff applies to every commit, not only the tested one.
        let install = steps.iter().find(|s| s.name == "install").unwrap();
        assert!(
            install
                .args
                .windows(2)
                .any(|w| w == ["--exclude-newer", crate::TESTED_HARNESS2_DEPS_CUTOFF])
        );

        let st = read_status(&home, true);
        assert!(st.installed);
        assert_eq!(st.commit.as_deref(), Some(C));
        assert_eq!(st.version.as_deref(), Some("0.1.0"));
        assert_eq!(st.python.as_deref(), Some("3.12.14"));
        assert_eq!(st.extras, vec![InstallExtra::Render]);
        assert_eq!(st.tested_commit, C);

        let missing = read_status(&tmp.path().join("nope"), false);
        assert!(!missing.installed && missing.commit.is_none());

        let not_git = tmp.path().join("plain");
        std::fs::create_dir_all(&not_git).unwrap();
        std::fs::write(not_git.join("x"), "").unwrap();
        assert!(matches!(
            plan(not_git.to_str().unwrap(), C, &[], true),
            Err(CoreError::Refused(_))
        ));
    }

    #[test]
    fn packed_refs_and_pyvenv() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git/HEAD"), "ref: refs/heads/master\n").unwrap();
        std::fs::write(
            tmp.path().join(".git/packed-refs"),
            format!("# pack-refs with: peeled\n{C} refs/heads/master\n"),
        )
        .unwrap();
        let d = open_home(tmp.path()).unwrap();
        assert_eq!(read_head(&d).as_deref(), Some(C));
        assert_eq!(
            pyvenv_version("version = 3.12.1\n").as_deref(),
            Some("3.12.1")
        );
        assert_eq!(
            pyvenv_version("version_info = 3.12\nversion = 3.12.9\n").as_deref(),
            Some("3.12.9")
        );
    }
}
