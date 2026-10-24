//! Classify the lines of a harness diff for colouring and line numbers.
//!
//! `FileDiff.unified` holds a unified diff (3 lines of context) for modified files, and the
//! full text for added or removed files.

use h2s_api::{ChangeKind, FileDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineKind {
    /// `@@ -a,b +c,d @@` hunk header.
    Hunk,
    Added,
    Removed,
    Context,
    /// `---`/`+++` file headers, `\ No newline at end of file`, anything unrecognised.
    Meta,
}

impl LineKind {
    /// CSS class suffix for the line.
    pub(crate) fn class(self) -> &'static str {
        match self {
            LineKind::Hunk => "diff-hunk",
            LineKind::Added => "diff-add",
            LineKind::Removed => "diff-del",
            LineKind::Context => "diff-ctx",
            LineKind::Meta => "diff-meta",
        }
    }

    /// The marker a screen reader announces before the text.
    pub(crate) fn sr_label(self) -> &'static str {
        match self {
            LineKind::Added => "added",
            LineKind::Removed => "removed",
            LineKind::Hunk | LineKind::Context | LineKind::Meta => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffLine {
    pub(crate) kind: LineKind,
    /// Line number in the old file (context and removed lines).
    pub(crate) old_no: Option<u32>,
    /// Line number in the new file (context and added lines).
    pub(crate) new_no: Option<u32>,
    /// The text without the leading `+`/`-`/space marker.
    pub(crate) text: String,
}

/// `@@ -12,5 +12,7 @@ optional` -> `(12, 12)`.
fn hunk_starts(line: &str) -> Option<(u32, u32)> {
    let body = line.strip_prefix("@@ ")?;
    let end = body.find(" @@")?;
    let mut parts = body[..end].split(' ');
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let start = |s: &str| s.split(',').next()?.parse::<u32>().ok();
    Some((start(old)?, start(new)?))
}

/// Parse a unified diff into classified lines with old/new line numbers.
pub(crate) fn parse_unified(text: &str) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let (mut old_no, mut new_no) = (0_u32, 0_u32);
    let mut in_hunk = false;
    for raw in text.lines() {
        if raw.starts_with("@@") {
            if let Some((o, n)) = hunk_starts(raw) {
                old_no = o;
                new_no = n;
                in_hunk = true;
            }
            out.push(DiffLine {
                kind: LineKind::Hunk,
                old_no: None,
                new_no: None,
                text: raw.to_owned(),
            });
            continue;
        }
        if !in_hunk || raw.starts_with('\\') {
            out.push(DiffLine {
                kind: LineKind::Meta,
                old_no: None,
                new_no: None,
                text: raw.to_owned(),
            });
            continue;
        }
        let (kind, body) = match raw.chars().next() {
            Some('+') => (LineKind::Added, &raw[1..]),
            Some('-') => (LineKind::Removed, &raw[1..]),
            Some(' ') => (LineKind::Context, &raw[1..]),
            // An empty line inside a hunk is an empty context line.
            None => (LineKind::Context, ""),
            Some(_) => (LineKind::Meta, raw),
        };
        let (o, n) = match kind {
            LineKind::Added => {
                new_no += 1;
                (None, Some(new_no - 1))
            }
            LineKind::Removed => {
                old_no += 1;
                (Some(old_no - 1), None)
            }
            LineKind::Context => {
                old_no += 1;
                new_no += 1;
                (Some(old_no - 1), Some(new_no - 1))
            }
            LineKind::Hunk | LineKind::Meta => (None, None),
        };
        out.push(DiffLine {
            kind,
            old_no: o,
            new_no: n,
            text: body.to_owned(),
        });
    }
    out
}

/// Lines to display for one file of a `HarnessDiff`: the parsed hunks for a modified file,
/// every line marked added/removed for an added/removed file, nothing when unchanged.
pub(crate) fn file_lines(file: &FileDiff) -> Vec<DiffLine> {
    match file.change {
        ChangeKind::Modified => parse_unified(&file.unified),
        ChangeKind::Added | ChangeKind::Removed => {
            let added = file.change == ChangeKind::Added;
            file.unified
                .lines()
                .zip(1_u32..)
                .map(|(l, i)| DiffLine {
                    kind: if added {
                        LineKind::Added
                    } else {
                        LineKind::Removed
                    },
                    old_no: (!added).then_some(i),
                    new_no: added.then_some(i),
                    text: l.to_owned(),
                })
                .collect()
        }
        ChangeKind::Unchanged => Vec::new(),
    }
}

pub(crate) fn change_label(c: ChangeKind) -> &'static str {
    match c {
        ChangeKind::Added => "added",
        ChangeKind::Removed => "removed",
        ChangeKind::Modified => "modified",
        ChangeKind::Unchanged => "unchanged",
    }
}

/// Totals over a diff: `(files changed, lines added, lines removed)`.
pub(crate) fn totals(files: &[FileDiff]) -> (u32, u32, u32) {
    files
        .iter()
        .filter(|f| f.change != ChangeKind::Unchanged)
        .fold((0, 0, 0), |(n, a, r), f| {
            (n + 1, a + f.lines_added, r + f.lines_removed)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2s_api::Component;

    const SAMPLE: &str = "--- a/systemprompt.md\n+++ b/systemprompt.md\n@@ -3,4 +3,5 @@ Intro\n keep one\n-old line\n+new line\n+another\n keep two\n\\ No newline at end of file\n@@ -20 +21 @@\n-x\n+y\n";

    #[test]
    fn classifies_and_numbers_lines() {
        let lines = parse_unified(SAMPLE);
        let kinds: Vec<LineKind> = lines.iter().map(|l| l.kind).collect();
        use LineKind::*;
        assert_eq!(
            kinds,
            vec![
                Meta, Meta, Hunk, Context, Removed, Added, Added, Context, Meta, Hunk, Removed,
                Added
            ]
        );
        assert_eq!((lines[3].old_no, lines[3].new_no), (Some(3), Some(3)));
        assert_eq!((lines[4].old_no, lines[4].new_no), (Some(4), None));
        assert_eq!((lines[5].old_no, lines[5].new_no), (None, Some(4)));
        assert_eq!((lines[6].old_no, lines[6].new_no), (None, Some(5)));
        assert_eq!((lines[7].old_no, lines[7].new_no), (Some(5), Some(6)));
        assert_eq!(lines[5].text, "new line");
        assert_eq!((lines[10].old_no, lines[11].new_no), (Some(20), Some(21)));
    }

    #[test]
    fn empty_line_in_hunk_is_context() {
        let lines = parse_unified("@@ -1,3 +1,3 @@\n a\n\n b\n");
        assert_eq!(lines[2].kind, LineKind::Context);
        assert_eq!(lines[2].old_no, Some(2));
        assert_eq!(lines[3].new_no, Some(3));
    }

    #[test]
    fn whole_file_changes() {
        let f = FileDiff {
            path: "skills/x/SKILL.md".into(),
            component: Component::Skills,
            change: ChangeKind::Added,
            unified: "line 1\nline 2".into(),
            lines_added: 2,
            lines_removed: 0,
        };
        let lines = file_lines(&f);
        assert!(lines.iter().all(|l| l.kind == LineKind::Added));
        assert_eq!(lines[1].new_no, Some(2));
        let removed = FileDiff {
            change: ChangeKind::Removed,
            ..f.clone()
        };
        assert!(
            file_lines(&removed)
                .iter()
                .all(|l| l.kind == LineKind::Removed)
        );
        let same = FileDiff {
            change: ChangeKind::Unchanged,
            unified: String::new(),
            ..f.clone()
        };
        assert!(file_lines(&same).is_empty());
        assert_eq!(totals(&[f, removed, same]), (2, 4, 0));
    }
}
