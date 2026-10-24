//! Experiment directories: `<experiments_dir>/<name>`, each one `HARNESS2_RUNS_DIR`.
//!
//! Names are validated by `h2s_core::names` before they get here; this module adds
//! containment anyway: it only accepts a single normal path component, creates and checks
//! the directory through a `cap_std` handle on the experiments root, and refuses symlinks.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::Dir;

use crate::util::is_single_component;

/// Errors locating or creating an experiment directory.
#[derive(Debug, thiserror::Error)]
pub enum DirError {
    #[error("invalid experiment directory name")]
    BadName,
    #[error("{0} is not a plain directory (symlinks are refused)")]
    NotADirectory(String),
    #[error("the experiment directory {0} does not exist")]
    Missing(String),
    // The message includes the cause; it is not also a `source()` (see `db::DbError`).
    #[error("experiment directory I/O: {0}")]
    Io(std::io::Error),
}

impl From<std::io::Error> for DirError {
    fn from(e: std::io::Error) -> Self {
        DirError::Io(e)
    }
}

fn root(experiments_dir: &Path) -> Result<Dir, DirError> {
    crate::util::ensure_private_dir(experiments_dir)?;
    Ok(Dir::open_ambient_dir(experiments_dir, ambient_authority())?)
}

/// Create `<root>/<name>` (0700) or adopt it when it already exists as a plain directory.
/// Returns the path and whether it was newly created.
///
/// # Errors
/// Bad names, symlinks, non-directories, I/O errors.
pub fn create_or_adopt(experiments_dir: &Path, name: &str) -> Result<(PathBuf, bool), DirError> {
    if !is_single_component(name) || name.starts_with('.') {
        return Err(DirError::BadName);
    }
    let dir = root(experiments_dir)?;
    let created = match dir.create_dir(name) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(e.into()),
    };
    let meta = dir.symlink_metadata(name)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(DirError::NotADirectory(name.to_owned()));
    }
    let path = experiments_dir.join(name);
    if created {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok((path, created))
}

/// The path of an existing experiment directory.
///
/// # Errors
/// Bad names, missing directories, symlinks.
pub fn existing(experiments_dir: &Path, name: &str) -> Result<PathBuf, DirError> {
    if !is_single_component(name) || name.starts_with('.') {
        return Err(DirError::BadName);
    }
    let dir = Dir::open_ambient_dir(experiments_dir, ambient_authority())?;
    let meta = match dir.symlink_metadata(name) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(DirError::Missing(name.to_owned()));
        }
        Err(e) => return Err(e.into()),
    };
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(DirError::NotADirectory(name.to_owned()));
    }
    Ok(experiments_dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_adopt_and_refuse() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("experiments");
        let (p, created) = create_or_adopt(&root, "e1").unwrap();
        assert!(created);
        assert!(p.is_dir());
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let (_, created) = create_or_adopt(&root, "e1").unwrap();
        assert!(!created);
        assert_eq!(existing(&root, "e1").unwrap(), p);
        for bad in ["", ".", "..", "a/b", "../x", ".hidden"] {
            assert!(
                matches!(create_or_adopt(&root, bad), Err(DirError::BadName)),
                "{bad}"
            );
        }
        std::fs::write(root.join("file"), b"x").unwrap();
        assert!(matches!(
            create_or_adopt(&root, "file"),
            Err(DirError::NotADirectory(_)) | Err(DirError::Io(_))
        ));
        let outside = d.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert!(matches!(
            existing(&root, "link"),
            Err(DirError::NotADirectory(_))
        ));
        assert!(create_or_adopt(&root, "link").is_err());
        assert!(matches!(existing(&root, "nope"), Err(DirError::Missing(_))));
    }
}
