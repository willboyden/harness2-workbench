//! Managed installation of harness2: the steps, and reading what is installed.
//!
//! Install = `git init` + `git fetch --depth 1 origin <commit>` + `git checkout --detach
//! FETCH_HEAD` (the commit id is the integrity check), `uv venv --python 3.12 .venv`,
//! `uv pip install --python .venv/bin/python -e .[render(,bridge)]`, then an import check.

use std::path::Path;

use h2s_api::{Harness2Status, InstallExtra};

use crate::{CoreError, Result};

/// One install step: run `program args...` in `cwd` (absolute) with no shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// `init`, `remote`, `fetch`, `checkout`, `verify-commit`, `venv`, `install`, `import`.
    pub name: &'static str,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
}

/// The ordered steps to install `commit` into `home` (absolute; must not exist or be empty
/// unless `reinstall`, in which case the venv step recreates `.venv` and the git steps are
/// skipped when HEAD already equals `commit`).
pub fn plan(
    home: &str,
    commit: &str,
    extras: &[InstallExtra],
    reinstall: bool,
) -> Result<Vec<Step>> {
    let _ = (home, commit, extras, reinstall);
    Err(CoreError::Unimplemented("install::plan"))
}

/// What is installed at `home`, from files only (no processes): `.git/HEAD` (detached
/// commit or a ref followed one level), `harness2.egg-info/PKG-INFO` / `pyproject.toml`
/// for the version, `.venv/pyvenv.cfg` for the Python version, installed extras detected
/// from the venv's site-packages (`openpyxl` -> render, `litellm` -> bridge).
#[must_use]
pub fn read_status(home: &Path, managed: bool) -> Harness2Status {
    let _ = (home, managed);
    Harness2Status {
        installed: false,
        managed,
        commit: None,
        tested_commit: crate::TESTED_HARNESS2_COMMIT.to_owned(),
        version: None,
        python: None,
        extras: Vec::new(),
        home: None,
    }
}
