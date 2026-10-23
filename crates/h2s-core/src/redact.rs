//! Replace secret values in text before it is stored or shown.
//!
//! Every job log line, error message and audit detail passes through a [`Redactor`] built
//! from the secrets that job can see. Matching is exact substring; values shorter than
//! [`MIN_SECRET_LEN`] are still redacted but flagged by the server as weak.

use std::borrow::Cow;

/// Values shorter than this are unusual for credentials; the server warns when one is set.
pub const MIN_SECRET_LEN: usize = 8;

/// Replaces each known secret value with `[REDACTED:<NAME>]`. Longer values are matched
/// first, so a secret that contains another is redacted whole.
#[derive(Clone, Default)]
pub struct Redactor {
    /// (name, value), sorted by value length descending. Never printed.
    entries: Vec<(String, String)>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.entries.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("Redactor").field("names", &names).finish()
    }
}

impl Redactor {
    /// Build from `(name, value)` pairs. Empty values are ignored.
    #[must_use]
    pub fn new(secrets: Vec<(String, String)>) -> Self {
        // STUB (core agent replaces): keep the values, longest first.
        let mut entries: Vec<(String, String)> =
            secrets.into_iter().filter(|(_, v)| !v.is_empty()).collect();
        entries.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
        Redactor { entries }
    }

    /// Add one more secret (e.g. the bridge key once it is known).
    pub fn add(&mut self, name: String, value: String) {
        let _ = (name, value);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `text` with every secret value replaced. Borrowed when nothing matched.
    #[must_use]
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        Cow::Borrowed(text)
    }
}
