//! The one error type of `h2s-core`.

use thiserror::Error;

/// Result alias used throughout `h2s-core`.
pub type Result<T, E = CoreError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum CoreError {
    /// One invalid input value. `problem` is a user-facing sentence.
    #[error("invalid {field}: {problem}")]
    Invalid { field: String, problem: String },

    /// Several invalid input values, as `(field, problem)` pairs.
    #[error("invalid input: {}", .0.iter().map(|(f, p)| format!("{f}: {p}")).collect::<Vec<_>>().join("; "))]
    InvalidMany(Vec<(String, String)>),

    /// The thing asked for does not exist (experiment, sweep, task, label, file).
    #[error("not found: {0}")]
    NotFound(String),

    /// Refused for safety: a path that would leave its root, a symlink, an oversized read,
    /// a reserved environment variable. The message is safe to show.
    #[error("refused: {0}")]
    Refused(String),

    /// A file harness2 wrote could not be understood.
    #[error("malformed {what}: {detail}")]
    Malformed { what: String, detail: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Interface stub not yet implemented. Never reachable in a release build: the release
    /// gate greps for this variant.
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}

impl CoreError {
    /// Shorthand for [`CoreError::Invalid`].
    pub fn invalid(field: impl Into<String>, problem: impl Into<String>) -> Self {
        CoreError::Invalid {
            field: field.into(),
            problem: problem.into(),
        }
    }
}
