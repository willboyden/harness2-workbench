//! Classify the lines of a harness diff for colouring and line numbers.
//!
//! `FileDiff.unified` holds a unified diff (3 lines of context) for modified files, and the
//! full text for added or removed files.

use h2w_api::{ChangeKind, FileDiff};

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

/// Largest middle section (old lines x new lines, after the common start and end are
/// removed) [`line_diff`] aligns line by line; beyond it the middle is shown as replaced.
const LCS_CELLS_MAX: usize = 4_000_000;

/// A line diff of two texts, computed in the browser (the configuration editor's export):
/// hunks with `context` unchanged lines around each change. Empty when the texts are equal.
pub(crate) fn line_diff(old: &str, new: &str, context: usize) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    // ops over the whole texts: (kind, old index, new index).
    let mut ops: Vec<(LineKind, usize, usize)> =
        (0..prefix).map(|i| (LineKind::Context, i, i)).collect();
    if am.len().saturating_mul(bm.len()) <= LCS_CELLS_MAX {
        // Longest common subsequence, table of suffix lengths.
        let (n, m) = (am.len(), bm.len());
        let mut t = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * (m + 1) + j] = if am[i] == bm[j] {
                    t[(i + 1) * (m + 1) + j + 1] + 1
                } else {
                    t[(i + 1) * (m + 1) + j].max(t[i * (m + 1) + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && am[i] == bm[j] {
                ops.push((LineKind::Context, prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if i < n && (j == m || t[(i + 1) * (m + 1) + j] >= t[i * (m + 1) + j + 1]) {
                // Removals first, as unified diffs show them.
                ops.push((LineKind::Removed, prefix + i, prefix + j));
                i += 1;
            } else {
                ops.push((LineKind::Added, prefix + i, prefix + j));
                j += 1;
            }
        }
    } else {
        ops.extend((0..am.len()).map(|i| (LineKind::Removed, prefix + i, prefix)));
        ops.extend((0..bm.len()).map(|j| (LineKind::Added, prefix + am.len(), prefix + j)));
    }
    let (ao, bo) = (a.len() - suffix, b.len() - suffix);
    ops.extend((0..suffix).map(|k| (LineKind::Context, ao + k, bo + k)));
    // Keep changes and `context` lines around them, with a hunk header per group.
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| o.0 != LineKind::Context)
        .map(|(k, _)| k)
        .collect();
    let mut out = Vec::new();
    let mut k = 0;
    while k < changed.len() {
        let start = changed[k].saturating_sub(context);
        let mut end = changed[k];
        while k < changed.len() && changed[k] <= end + 2 * context + 1 {
            end = changed[k];
            k += 1;
        }
        let end = (end + context).min(ops.len() - 1);
        let first = ops[start];
        out.push(DiffLine {
            kind: LineKind::Hunk,
            old_no: None,
            new_no: None,
            text: format!("@@ line {} @@", first.2 + 1),
        });
        for &(kind, i, j) in &ops[start..=end] {
            let num = |x: usize| u32::try_from(x + 1).ok();
            out.push(match kind {
                LineKind::Removed => DiffLine {
                    kind,
                    old_no: num(i),
                    new_no: None,
                    text: a[i].to_owned(),
                },
                LineKind::Added => DiffLine {
                    kind,
                    old_no: None,
                    new_no: num(j),
                    text: b[j].to_owned(),
                },
                _ => DiffLine {
                    kind,
                    old_no: num(i),
                    new_no: num(j),
                    text: a[i].to_owned(),
                },
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2w_api::Component;

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

    #[test]
    fn line_diff_of_texts() {
        assert!(line_diff("a\nb\n", "a\nb\n", 3).is_empty());
        let d = line_diff(
            "a\nb\nc\nd\ne\nf\ng\nh\ni\n",
            "a\nb\nc\nd\nE\nf\ng\nh\ni\nj\n",
            1,
        );
        let kinds: Vec<LineKind> = d.iter().map(|l| l.kind).collect();
        use LineKind::*;
        assert_eq!(
            kinds,
            vec![Hunk, Context, Removed, Added, Context, Hunk, Context, Added]
        );
        assert_eq!((d[2].old_no, d[2].text.as_str()), (Some(5), "e"));
        assert_eq!((d[3].new_no, d[3].text.as_str()), (Some(5), "E"));
        assert_eq!(d[7].new_no, Some(10));
        // Nearby changes share a hunk.
        let d = line_diff("1\n2\n3\n4\n", "x\n2\n3\ny\n", 1);
        assert_eq!(d.iter().filter(|l| l.kind == Hunk).count(), 1);
        // From and to empty.
        assert!(line_diff("", "a\n", 3).iter().any(|l| l.kind == Added));
        assert!(line_diff("a\n", "", 3).iter().any(|l| l.kind == Removed));
    }
}
