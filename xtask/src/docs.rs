//! Documentation link checker: `cargo xtask docs`.
//!
//! Every relative link and `#anchor` in the repository's Markdown files must resolve, so a
//! renamed file or heading cannot leave a dead link behind. For each link the checker
//! verifies that
//!
//! - the target file or directory exists in the repository (tracked by git, or untracked but
//!   not ignored: it is what a push would publish; a generated file under an ignored
//!   directory does not count), and
//! - a `#fragment` names a heading of the target Markdown file under GitHub's anchor rules
//!   (lowercase, punctuation dropped, spaces become hyphens, repeated headings get `-1`,
//!   `-2`, ...), an explicit HTML anchor (`<a name="x">`, `id="x"`), or, for a code file, a
//!   line reference such as `#L10` or `#L10-L20`.
//!
//! A short list of documents must also exist whether or not anything links to them: the
//! release workflow copies `README.md` into every archive, and the guides are what
//! `SECURITY.md`, `SUPPORT.md`, `CONTRIBUTING.md` and `docs/ARCHITECTURE.md` point readers at.
//!
//! Links with a scheme (`https:`, `mailto:`, ...) are not fetched. Fenced code, inline code
//! and HTML comments are ignored. `tests/fixtures/` is skipped: it holds harness2 output
//! captured for tests, not documentation.
//!
//! The scanner is deliberately small (the xtask crate takes no Markdown dependency): it does
//! not model indented code blocks, and a link whose destination it cannot parse is ignored
//! rather than reported.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Files under these prefixes are not checked for outgoing links (they stay valid targets).
const SKIP_PREFIXES: &[&str] = &["tests/fixtures/"];

/// Files that must exist (see the module comment). A missing one is reported even if no link
/// to it exists, which is how a deleted or never-written guide would otherwise go unnoticed.
const REQUIRED: &[&str] = &[
    "README.md",
    "LICENSE",
    "CHANGELOG.md",
    "SECURITY.md",
    "SUPPORT.md",
    "CONTRIBUTING.md",
    "docs/ARCHITECTURE.md",
    "docs/USER-GUIDE.md",
    "docs/ADMIN-GUIDE.md",
    "docs/API.md",
    "docs/MCP.md",
    "docs/MODELS.md",
    "docs/CONCEPTS.md",
    "docs/THREAT-MODEL.md",
    "docs/TROUBLESHOOTING.md",
    "docs/adr/README.md",
];

/// Entry point of `cargo xtask docs`.
pub(crate) fn check(root: &Path) -> Result<()> {
    let files = repo_files(root)?;
    let mut docs = BTreeMap::new();
    for rel in files.iter().filter(|f| is_markdown(f)) {
        let path = root.join(rel);
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        docs.insert(
            rel.clone(),
            String::from_utf8_lossy(&bytes).replace("\r\n", "\n"),
        );
    }
    let repo = Repo::new(files, docs);
    let report = repo.check(|p| !SKIP_PREFIXES.iter().any(|s| p.starts_with(s)));
    let missing = repo.missing_required(REQUIRED);
    for p in &report.problems {
        eprintln!("{}:{}: {}: {}", p.file, p.line, p.link, p.why);
    }
    for m in &missing {
        eprintln!(
            "{m}: required file is missing (release archives and the other guides rely on it)"
        );
    }
    if !report.problems.is_empty() || !missing.is_empty() {
        let files_with_problems: BTreeSet<&str> =
            report.problems.iter().map(|p| p.file.as_str()).collect();
        bail!(
            "{} broken link(s) in {} Markdown file(s), {} required file(s) missing",
            report.problems.len(),
            files_with_problems.len(),
            missing.len()
        );
    }
    eprintln!(
        "docs: {} Markdown files, {} relative links and anchors checked: ok",
        report.files, report.links
    );
    Ok(())
}

/// Every file git would publish, as `/`-separated paths relative to the root.
fn repo_files(root: &Path) -> Result<BTreeSet<String>> {
    let out = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .context("could not run git")?;
    if !out.status.success() {
        bail!(
            "`git ls-files` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mut set = BTreeSet::new();
    for raw in out.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let rel = String::from_utf8_lossy(raw).into_owned();
        // `--cached` still lists a file deleted in the working tree until the deletion is staged.
        if std::fs::symlink_metadata(root.join(&rel)).is_ok() {
            set.insert(rel);
        }
    }
    Ok(set)
}

fn is_markdown(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

// ---------------------------------------------------------------------------------------
// The checker proper (pure: no file system access, so the tests need no fixtures)
// ---------------------------------------------------------------------------------------

/// One broken link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) link: String,
    pub(crate) why: String,
}

/// What a run found.
#[derive(Debug, Default)]
pub(crate) struct Report {
    /// Markdown files whose links were checked.
    pub(crate) files: usize,
    /// Relative links and anchors checked.
    pub(crate) links: usize,
    pub(crate) problems: Vec<Problem>,
}

/// The repository as the checker sees it.
pub(crate) struct Repo {
    /// Every published file.
    files: BTreeSet<String>,
    /// Every directory that holds a published file.
    dirs: HashSet<String>,
    /// Text of every Markdown file (the targets of anchors may be among the skipped ones).
    docs: BTreeMap<String, String>,
    anchors: HashMap<String, HashSet<String>>,
}

impl Repo {
    pub(crate) fn new(files: BTreeSet<String>, docs: BTreeMap<String, String>) -> Repo {
        let mut dirs = HashSet::new();
        for f in &files {
            let mut cur = f.as_str();
            while let Some((parent, _)) = cur.rsplit_once('/') {
                dirs.insert(parent.to_owned());
                cur = parent;
            }
        }
        let anchors = docs
            .iter()
            .map(|(path, text)| (path.clone(), anchors_of(text)))
            .collect();
        Repo {
            files,
            dirs,
            docs,
            anchors,
        }
    }

    /// The entries of `required` that are not published files.
    pub(crate) fn missing_required(&self, required: &[&'static str]) -> Vec<&'static str> {
        required
            .iter()
            .copied()
            .filter(|f| !self.files.contains(*f))
            .collect()
    }

    /// Check the outgoing links of every Markdown file `include` accepts.
    pub(crate) fn check(&self, include: impl Fn(&str) -> bool) -> Report {
        let mut report = Report::default();
        for (path, text) in &self.docs {
            if !include(path) {
                continue;
            }
            report.files += 1;
            for link in extract_links(text) {
                if let Some(checked) = self.check_link(path, &link) {
                    report.links += 1;
                    if let Err(why) = checked {
                        report.problems.push(Problem {
                            file: path.clone(),
                            line: link.line,
                            link: link.dest.clone(),
                            why,
                        });
                    }
                }
            }
        }
        report
    }

    /// `None` for links that are not checked (they have a scheme), else the verdict.
    fn check_link(&self, from: &str, link: &Link) -> Option<Result<(), String>> {
        let dest = link.dest.trim();
        if dest.is_empty() {
            return Some(Err("empty link destination".to_owned()));
        }
        if has_scheme(dest) || dest.starts_with("//") {
            return None;
        }
        let (path_part, fragment) = match dest.split_once('#') {
            Some((p, f)) => (p, Some(f)),
            None => (dest, None),
        };
        let path_part = path_part.split('?').next().unwrap_or("");
        let target = if path_part.is_empty() {
            from.to_owned()
        } else {
            match resolve(from, &percent_decode(path_part)) {
                Ok(t) => t,
                Err(why) => return Some(Err(why)),
            }
        };
        let is_file = self.files.contains(&target);
        let is_dir = target.is_empty() || self.dirs.contains(&target);
        if !is_file && !is_dir {
            return Some(Err("no such file or directory".to_owned()));
        }
        let Some(fragment) = fragment.filter(|f| !f.is_empty()) else {
            return Some(Ok(()));
        };
        if !is_file {
            // GitHub lists a directory; there is nothing to anchor to, and nothing to break.
            return Some(Ok(()));
        }
        let fragment = percent_decode(fragment);
        Some(self.check_fragment(&target, &fragment))
    }

    fn check_fragment(&self, target: &str, fragment: &str) -> Result<(), String> {
        if is_markdown(target) {
            let Some(anchors) = self.anchors.get(target) else {
                return Ok(());
            };
            let bare = fragment.strip_prefix("user-content-").unwrap_or(fragment);
            if anchors.contains(fragment) || anchors.contains(bare) {
                return Ok(());
            }
            return Err(format!("no heading or anchor `#{fragment}` in {target}"));
        }
        if is_line_reference(fragment) {
            return Ok(());
        }
        Err(format!(
            "`#{fragment}` on a non-Markdown file: only line references such as #L10 or #L10-L20 work"
        ))
    }
}

/// `L10`, `L10-L20`, `L10C5-L12C3`.
fn is_line_reference(f: &str) -> bool {
    fn position(s: &str) -> bool {
        let Some(rest) = s.strip_prefix('L') else {
            return false;
        };
        let (line, col) = match rest.split_once('C') {
            Some((l, c)) => (l, Some(c)),
            None => (rest, None),
        };
        !line.is_empty()
            && line.bytes().all(|b| b.is_ascii_digit())
            && col.is_none_or(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_digit()))
    }
    match f.split_once('-') {
        Some((a, b)) => position(a) && position(b),
        None => position(f),
    }
}

/// `scheme:` per RFC 3986 (`https:`, `mailto:`, `tel:`, ...).
fn has_scheme(dest: &str) -> bool {
    let mut chars = dest.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    for c in chars {
        match c {
            ':' => return true,
            c if c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-') => {}
            _ => return false,
        }
    }
    false
}

/// Join `rel` to the directory of `from` (or the root for a leading `/`), resolving `.` and
/// `..`. Errors when the result leaves the repository.
fn resolve(from: &str, rel: &str) -> Result<String, String> {
    let mut parts: Vec<&str> = if rel.starts_with('/') {
        Vec::new()
    } else {
        let mut p: Vec<&str> = from.split('/').collect();
        p.pop(); // the file name
        p
    };
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("the link leaves the repository".to_owned());
                }
            }
            s => parts.push(s),
        }
    }
    Ok(parts.join("/"))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).and_then(|c| hex_val(*c)),
                b.get(i + 2).and_then(|c| hex_val(*c)),
            )
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// Masking: code and comments must not produce links or headings
// ---------------------------------------------------------------------------------------

/// `line` with every character but the line terminator replaced by a space.
fn blank_line(line: &str) -> String {
    line.chars()
        .map(|c| if c == '\n' || c == '\r' { c } else { ' ' })
        .collect()
}

/// `` ``` `` or `~~~` opener: the fence character and its length.
fn fence_open(probe: &str) -> Option<(char, usize)> {
    let ch = probe.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = probe.chars().take_while(|c| *c == ch).count();
    if len < 3 {
        return None;
    }
    let info: String = probe.chars().skip(len).collect();
    if ch == '`' && info.contains('`') {
        return None;
    }
    Some((ch, len))
}

fn fence_close(probe: &str, ch: char, len: usize) -> bool {
    let run = probe.chars().take_while(|c| *c == ch).count();
    run >= len && probe.chars().skip(run).all(char::is_whitespace)
}

/// Blank out fenced code blocks (also inside lists and block quotes).
fn mask_fenced_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fence: Option<(char, usize)> = None;
    for line in text.split_inclusive('\n') {
        let probe = line.trim_start_matches([' ', '\t', '>']);
        match fence {
            None => {
                if let Some(open) = fence_open(probe) {
                    fence = Some(open);
                    out.push_str(&blank_line(line));
                } else {
                    out.push_str(line);
                }
            }
            Some((ch, len)) => {
                out.push_str(&blank_line(line));
                if fence_close(probe.trim_end(), ch, len) {
                    fence = None;
                }
            }
        }
    }
    out
}

/// Blank out `<!-- ... -->`, which may span lines.
fn mask_html_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        let (before, from_comment) = rest.split_at(start);
        out.push_str(before);
        match from_comment.find("-->") {
            Some(end) => {
                let (comment, after) = from_comment.split_at(end + 3);
                out.push_str(&blank_line(comment));
                rest = after;
            }
            None => {
                out.push_str(from_comment);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Blank out inline code spans. A span ends at the next backtick run of the same length and
/// never crosses a blank line; an opener without a closer is literal text.
fn mask_inline_code(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = chars.clone();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] != '`' {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && chars[i] == '`' {
            i += 1;
        }
        let run = i - start;
        let mut j = i;
        let mut closer = None;
        while j < n {
            if chars[j] == '\n' && blank_line_follows(&chars, j) {
                break;
            }
            if chars[j] == '`' {
                let s = j;
                while j < n && chars[j] == '`' {
                    j += 1;
                }
                if j - s == run {
                    closer = Some(j);
                    break;
                }
            } else {
                j += 1;
            }
        }
        if let Some(end) = closer {
            for c in &mut out[start..end] {
                if *c != '\n' {
                    *c = ' ';
                }
            }
            i = end;
        }
    }
    out.into_iter().collect()
}

/// After the newline at `at`: is the next line empty or whitespace only?
fn blank_line_follows(chars: &[char], at: usize) -> bool {
    chars
        .iter()
        .skip(at + 1)
        .take_while(|c| **c != '\n')
        .all(|c| c.is_whitespace())
}

// ---------------------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------------------

/// A link destination as written, with its 1-based line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Link {
    pub(crate) line: usize,
    pub(crate) dest: String,
}

fn is_ascii_punct(c: char) -> bool {
    c.is_ascii_punctuation()
}

/// Backslash escapes of ASCII punctuation, resolved.
fn unescape(s: &[char]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == '\\' && s.get(i + 1).copied().is_some_and(is_ascii_punct) {
            out.push(s[i + 1]);
            i += 2;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// Parse `dest "title")` after the `(` of an inline link, starting at `i`. Returns the
/// destination and the index just past the closing parenthesis.
fn parse_inline_dest(chars: &[char], mut i: usize) -> Option<(String, usize)> {
    let n = chars.len();
    let skip_space = |mut i: usize| {
        let mut newlines = 0;
        while i < n && chars[i].is_whitespace() {
            if chars[i] == '\n' {
                newlines += 1;
                if newlines > 1 {
                    break;
                }
            }
            i += 1;
        }
        i
    };
    i = skip_space(i);
    let dest;
    if chars.get(i) == Some(&'<') {
        i += 1;
        let start = i;
        while i < n && chars[i] != '>' && chars[i] != '\n' {
            if chars[i] == '\\' {
                i += 1;
            }
            i += 1;
        }
        if chars.get(i) != Some(&'>') {
            return None;
        }
        dest = unescape(&chars[start..i]);
        i += 1;
    } else {
        let start = i;
        let mut depth = 0usize;
        while i < n {
            let c = chars[i];
            if c == '\\' && chars.get(i + 1).copied().is_some_and(is_ascii_punct) {
                i += 2;
                continue;
            }
            if c.is_whitespace() || c.is_control() {
                break;
            }
            if c == '(' {
                depth += 1;
            } else if c == ')' {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            i += 1;
        }
        if depth != 0 {
            return None;
        }
        dest = unescape(&chars[start..i]);
    }
    i = skip_space(i);
    if let Some(&open) = chars.get(i)
        && matches!(open, '"' | '\'' | '(')
    {
        let close = if open == '(' { ')' } else { open };
        i += 1;
        while i < n && chars[i] != close {
            if chars[i] == '\\' {
                i += 1;
            }
            i += 1;
        }
        if i >= n {
            return None;
        }
        i = skip_space(i + 1);
    }
    if chars.get(i) == Some(&')') {
        Some((dest, i + 1))
    } else {
        None
    }
}

/// `[label]: destination` reference definitions (not footnotes).
fn reference_definition(line: &str) -> Option<String> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let rest = trimmed.strip_prefix('[')?;
    let (label, after) = rest.split_once("]:")?;
    if label.is_empty() || label.starts_with('^') || label.contains('[') {
        return None;
    }
    let after = after.trim_start();
    let dest = if let Some(inner) = after.strip_prefix('<') {
        inner.split_once('>')?.0
    } else {
        after.split_whitespace().next()?
    };
    Some(dest.to_owned())
}

/// The value of attribute `name` (`href`, `src`, `id`, `name`) in every HTML tag of `text`.
fn html_attribute_values(text: &str, name: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(char::to_ascii_lowercase).collect();
    let pat: Vec<char> = format!("{name}=").chars().collect();
    let mut found = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\n' {
            line += 1;
        }
        let boundary = i > 0 && chars[i - 1].is_whitespace();
        if boundary && lower[i..].starts_with(&pat) {
            let q = i + pat.len();
            if let Some(&quote) = chars.get(q)
                && matches!(quote, '"' | '\'')
            {
                let mut j = q + 1;
                while j < chars.len() && chars[j] != quote && chars[j] != '\n' {
                    j += 1;
                }
                if chars.get(j) == Some(&quote) {
                    found.push((line, chars[q + 1..j].iter().collect()));
                    i = j;
                }
            }
        }
        i += 1;
    }
    found
}

/// Every link destination of `text`, outside code and comments.
pub(crate) fn extract_links(text: &str) -> Vec<Link> {
    let masked = mask_inline_code(&mask_html_comments(&mask_fenced_code(text)));
    let chars: Vec<char> = masked.chars().collect();
    let mut links = Vec::new();

    let mut line = 1;
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            line += 1;
            continue;
        }
        if *c == ']'
            && chars.get(i + 1) == Some(&'(')
            && let Some((dest, _)) = parse_inline_dest(&chars, i + 2)
        {
            links.push(Link { line, dest });
        }
    }
    for (n, l) in masked.lines().enumerate() {
        if let Some(dest) = reference_definition(l) {
            links.push(Link { line: n + 1, dest });
        }
    }
    for attr in ["href", "src"] {
        for (line, dest) in html_attribute_values(&masked, attr) {
            links.push(Link { line, dest });
        }
    }
    links.sort_by_key(|l| l.line);
    links
}

// ---------------------------------------------------------------------------------------
// Anchors
// ---------------------------------------------------------------------------------------

/// Characters GitHub's slugger removes from a heading although they count as numbers: the
/// "other number" category (superscripts, fractions, circled digits).
fn is_other_number(c: char) -> bool {
    matches!(c as u32,
        0x00B2 | 0x00B3 | 0x00B9 | 0x00BC..=0x00BE
        | 0x09F4..=0x09F9 | 0x0B72..=0x0B77 | 0x0BF0..=0x0BF2 | 0x0C78..=0x0C7E
        | 0x0D58..=0x0D5E | 0x0D70..=0x0D78 | 0x0F2A..=0x0F33 | 0x1369..=0x137C
        | 0x17F0..=0x17F9 | 0x19DA | 0x2070 | 0x2074..=0x2079 | 0x2080..=0x2089
        | 0x2150..=0x215F | 0x2189 | 0x2460..=0x249B | 0x24EA..=0x24FF
        | 0x2776..=0x2793 | 0x2CFD | 0x3192..=0x3195 | 0x3220..=0x3229
        | 0x3248..=0x324F | 0x3251..=0x325F | 0x3280..=0x3289 | 0x32B1..=0x32BF)
}

/// Combining marks (the `Mn`/`Me` blocks that std cannot classify).
fn is_combining_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

fn keeps_in_slug(c: char) -> bool {
    c == '-' || c == '_' || (c.is_alphanumeric() && !is_other_number(c)) || is_combining_mark(c)
}

/// GitHub's anchor for a heading's text (github-slugger): lowercase, drop everything that is
/// not a letter, a decimal number, a mark, `-` or `_`, then turn each space into `-`.
pub(crate) fn github_slug(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .filter_map(|c| {
            if c == ' ' {
                Some('-')
            } else if keeps_in_slug(c) {
                Some(c)
            } else {
                None
            }
        })
        .collect()
}

/// The text a heading renders as, approximately: inline code, links, images, tags and
/// emphasis resolved to their content.
fn heading_text(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let n = chars.len();
    let mut out: Vec<(char, bool)> = Vec::new(); // (character, literal: written as `\x` or in code)
    let mut i = 0;
    while i < n {
        let c = chars[i];
        match c {
            '\\' if chars.get(i + 1).copied().is_some_and(is_ascii_punct) => {
                out.push((chars[i + 1], true));
                i += 2;
            }
            '`' => {
                let start = i;
                while i < n && chars[i] == '`' {
                    i += 1;
                }
                let run = i - start;
                let mut j = i;
                let mut close = None;
                while j < n {
                    if chars[j] == '`' {
                        let s = j;
                        while j < n && chars[j] == '`' {
                            j += 1;
                        }
                        if j - s == run {
                            close = Some(s);
                            break;
                        }
                    } else {
                        j += 1;
                    }
                }
                match close {
                    Some(end) => {
                        out.extend(chars[i..end].iter().map(|c| (*c, true)));
                        i = end + run;
                    }
                    None => {
                        out.extend(chars[start..i].iter().map(|c| (*c, true)));
                    }
                }
            }
            '<' => {
                // An HTML tag: drop it, keep the text around it.
                let tag_like = chars
                    .get(i + 1)
                    .is_some_and(|c| c.is_ascii_alphabetic() || *c == '/');
                match (tag_like, chars[i..].iter().position(|c| *c == '>')) {
                    (true, Some(len)) => i += len + 1,
                    _ => {
                        out.push((c, false));
                        i += 1;
                    }
                }
            }
            '!' if chars.get(i + 1) == Some(&'[') => i += 1, // an image: its alt text follows
            ']' if chars.get(i + 1) == Some(&'(') => {
                // The destination of an inline link: not part of the heading text.
                match parse_inline_dest(&chars, i + 2) {
                    Some((_, end)) => i = end,
                    None => {
                        out.push((c, false));
                        i += 1;
                    }
                }
            }
            _ => {
                out.push((c, false));
                i += 1;
            }
        }
    }
    drop_emphasis_underscores(&out)
}

/// Remove `_` that opens or closes emphasis (`_word_`); keep `snake_case` and escaped ones.
fn drop_emphasis_underscores(chars: &[(char, bool)]) -> String {
    let mut out = String::new();
    let mut open = 0usize;
    for (i, (c, literal)) in chars.iter().enumerate() {
        if *c != '_' || *literal {
            out.push(*c);
            continue;
        }
        let prev = i.checked_sub(1).and_then(|p| chars.get(p)).map(|(c, _)| *c);
        let next = chars.get(i + 1).map(|(c, _)| *c);
        let prev_word = prev.is_some_and(char::is_alphanumeric);
        let next_word = next.is_some_and(char::is_alphanumeric);
        if !prev_word && next_word {
            open += 1; // opens emphasis
        } else if prev_word && !next_word && open > 0 {
            open -= 1; // closes it
        } else {
            out.push(*c);
        }
    }
    out
}

/// `# Heading`, `## Heading ##`: the heading text, or `None`.
fn atx_heading(line: &str) -> Option<String> {
    let body = line.trim_start_matches([' ', '>']);
    if line.len() - line.trim_start_matches(' ').len() > 3 && !line.trim_start().starts_with('>') {
        return None;
    }
    let hashes = body.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &body[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let mut text = rest.trim();
    // An optional closing sequence of `#` preceded by a space.
    let without = text.trim_end_matches('#');
    if without.len() != text.len() && (without.is_empty() || without.ends_with([' ', '\t'])) {
        text = without.trim_end();
    }
    Some(text.to_owned())
}

fn is_setext_underline(line: &str) -> bool {
    let t = line.trim();
    let lead = line.len() - line.trim_start().len();
    lead <= 3 && t.len() >= 2 && (t.chars().all(|c| c == '=') || t.chars().all(|c| c == '-'))
}

fn starts_block(line: &str) -> bool {
    let t = line.trim_start();
    t.is_empty()
        || t.starts_with(['#', '>', '|'])
        || t.starts_with("- ")
        || t.starts_with("* ")
        || t.starts_with("+ ")
        || t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains(". ")
}

/// Every anchor `text` offers: heading slugs (with GitHub's `-1`, `-2` suffixes for repeats)
/// and explicit HTML `id`/`name` attributes.
pub(crate) fn anchors_of(text: &str) -> HashSet<String> {
    let masked = mask_html_comments(&mask_fenced_code(text));
    let lines: Vec<&str> = masked.lines().collect();
    let mut anchors = HashSet::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut add = |heading: &str, anchors: &mut HashSet<String>| {
        let base = github_slug(&heading_text(heading));
        let mut slug = base.clone();
        while let Some(count) = seen.get_mut(&slug) {
            *count += 1;
            slug = format!("{base}-{count}");
        }
        seen.insert(slug.clone(), 0);
        anchors.insert(slug);
    };
    for (i, line) in lines.iter().enumerate() {
        if let Some(h) = atx_heading(line) {
            add(&h, &mut anchors);
            continue;
        }
        let previous_is_paragraph = i == 0 || lines.get(i - 1).is_none_or(|p| p.trim().is_empty());
        if previous_is_paragraph
            && !starts_block(line)
            && lines
                .get(i + 1)
                .is_some_and(|next| is_setext_underline(next))
        {
            add(line.trim(), &mut anchors);
        }
    }
    for attr in ["id", "name"] {
        for (_, value) in html_attribute_values(&masked, attr) {
            anchors.insert(value);
        }
    }
    anchors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[(&str, &str)]) -> Repo {
        let paths: BTreeSet<String> = files.iter().map(|(p, _)| (*p).to_owned()).collect();
        let docs: BTreeMap<String, String> = files
            .iter()
            .filter(|(p, _)| is_markdown(p))
            .map(|(p, t)| ((*p).to_owned(), (*t).to_owned()))
            .collect();
        Repo::new(paths, docs)
    }

    fn problems(files: &[(&str, &str)]) -> Vec<String> {
        repo(files)
            .check(|_| true)
            .problems
            .into_iter()
            .map(|p| format!("{}:{}: {}: {}", p.file, p.line, p.link, p.why))
            .collect()
    }

    #[test]
    fn required_files_are_reported_even_when_nothing_links_to_them() {
        let r = repo(&[("README.md", "# Hi\n"), ("docs/API.md", "# API\n")]);
        assert_eq!(
            r.missing_required(&["README.md", "docs/USER-GUIDE.md", "docs/API.md", "LICENSE"]),
            vec!["docs/USER-GUIDE.md", "LICENSE"]
        );
        assert!(r.missing_required(&["README.md"]).is_empty());
    }

    #[test]
    fn the_real_required_list_names_the_guides_the_governance_files_link_to() {
        for must in [
            "README.md",
            "docs/THREAT-MODEL.md",
            "docs/ADMIN-GUIDE.md",
            "docs/USER-GUIDE.md",
            "docs/TROUBLESHOOTING.md",
            "docs/adr/README.md",
        ] {
            assert!(REQUIRED.contains(&must), "{must}");
        }
    }

    fn dests(text: &str) -> Vec<String> {
        extract_links(text).into_iter().map(|l| l.dest).collect()
    }

    // --- anchors ---------------------------------------------------------------------

    #[test]
    fn slugs_follow_github() {
        let cases = [
            ("Installation", "installation"),
            ("The REST API", "the-rest-api"),
            ("16. Contract addenda", "16-contract-addenda"),
            (
                "Harness² Workbench — Architecture and Build Contract",
                "harness-workbench--architecture-and-build-contract",
            ),
            (
                "`h2workbench init` and the first token",
                "h2workbench-init-and-the-first-token",
            ),
            ("What's new?", "whats-new"),
            ("snake_case stays", "snake_case-stays"),
            ("Already-hyphenated words", "already-hyphenated-words"),
            ("C++ & Rust", "c--rust"),
            ("Über cool", "über-cool"),
            ("日本語 heading", "日本語-heading"),
            ("Trailing space ", "trailing-space-"),
            ("1½ cups", "1-cups"),
        ];
        for (heading, want) in cases {
            assert_eq!(github_slug(&heading_text(heading)), want, "{heading}");
        }
    }

    #[test]
    fn heading_markup_is_resolved_before_slugging() {
        let cases = [
            ("[Link text](https://example.com/x)", "link-text"),
            ("![alt text](img.png) caption", "alt-text-caption"),
            ("**Bold** and *italic*", "bold-and-italic"),
            ("_emphasis_ here", "emphasis-here"),
            ("a <code>tag</code> inside", "a-tag-inside"),
            ("literal \\_underscore\\_", "literal-_underscore_"),
            ("`code with _under_ score`", "code-with-_under_-score"),
            ("1 < 2 > 0", "1--2--0"),
        ];
        for (heading, want) in cases {
            assert_eq!(github_slug(&heading_text(heading)), want, "{heading}");
        }
    }

    #[test]
    fn repeated_headings_get_numeric_suffixes() {
        let a = anchors_of("# Setup\n\n## Notes\n\n## Notes\n\n## Notes\n\n# Setup\n");
        for want in ["setup", "setup-1", "notes", "notes-1", "notes-2"] {
            assert!(a.contains(want), "missing {want} in {a:?}");
        }
    }

    #[test]
    fn headings_in_code_and_comments_are_not_anchors() {
        let text =
            "# Real\n\n```md\n# Not a heading\n```\n\n<!--\n# Hidden\n-->\n\n~~~\n## Tilde\n~~~\n";
        let a = anchors_of(text);
        assert!(a.contains("real"));
        for no in ["not-a-heading", "hidden", "tilde"] {
            assert!(!a.contains(no), "{no}");
        }
    }

    #[test]
    fn atx_forms() {
        assert_eq!(atx_heading("## Two ##"), Some("Two".to_owned()));
        assert_eq!(atx_heading("#   Spaced   "), Some("Spaced".to_owned()));
        assert_eq!(atx_heading("### C#"), Some("C#".to_owned()));
        assert_eq!(
            atx_heading("> ## In a quote"),
            Some("In a quote".to_owned())
        );
        assert_eq!(atx_heading("#hashtag"), None);
        assert_eq!(atx_heading("####### seven"), None);
        assert_eq!(atx_heading("    # indented code"), None);
        assert_eq!(atx_heading("#"), Some(String::new()));
    }

    #[test]
    fn setext_headings() {
        let a = anchors_of("Title text\n==========\n\nSecond one\n---\n\n- list item\n---\n");
        assert!(a.contains("title-text"));
        assert!(a.contains("second-one"));
        assert!(!a.contains("list-item"));
    }

    #[test]
    fn explicit_html_anchors() {
        let a = anchors_of("<a name=\"old-name\"></a>\n\n<h2 id='custom'>x</h2>\n");
        assert!(a.contains("old-name"));
        assert!(a.contains("custom"));
    }

    // --- extraction ------------------------------------------------------------------

    #[test]
    fn extracts_inline_links_images_and_nested_text() {
        let text = "See [a](one.md), ![img](pic.png \"title\"), [`code`](two.md#x),\n\
                    [nested [brackets]](three.md) and [angle](<four five.md>).\n";
        assert_eq!(
            dests(text),
            ["one.md", "pic.png", "two.md#x", "three.md", "four five.md"]
        );
        let lines: Vec<usize> = extract_links(text).iter().map(|l| l.line).collect();
        assert_eq!(lines, [1, 1, 1, 2, 2]);
    }

    #[test]
    fn balanced_parentheses_and_escapes_in_destinations() {
        assert_eq!(dests("[x](a_(b).md) [y](c\\).md)"), ["a_(b).md", "c).md"]);
        assert!(dests("[x](unclosed.md").is_empty());
    }

    #[test]
    fn links_inside_code_and_comments_are_ignored() {
        let text = "```sh\n[x](fenced.md)\n```\n`[y](inline.md)` ``[z](double`tick.md)``\n\
                    <!-- [w](comment.md) -->\n[real](real.md)\n";
        assert_eq!(dests(text), ["real.md"]);
    }

    #[test]
    fn unmatched_backticks_are_literal() {
        assert_eq!(dests("a ` b [x](kept.md)\n"), ["kept.md"]);
        // A span cannot cross a blank line.
        assert_eq!(dests("`open\n\n[x](kept.md) close`\n"), ["kept.md"]);
    }

    #[test]
    fn reference_definitions_and_html_attributes() {
        let text = "[ref]: docs/a.md \"Title\"\n[^1]: footnote text\n   [angle]: <b c.md>\n\
                    <a href=\"d.md\">x</a> <img src='e.png'>\n";
        assert_eq!(dests(text), ["docs/a.md", "b c.md", "d.md", "e.png"]);
    }

    // --- resolution ------------------------------------------------------------------

    #[test]
    fn relative_root_and_parent_paths() {
        let files = [
            (
                "README.md",
                "[a](docs/A.md) [b](/docs/A.md) [c](./docs/../docs/A.md)\n",
            ),
            (
                "docs/A.md",
                "[up](../README.md) [sib](B.md) [root](/README.md)\n",
            ),
            ("docs/B.md", "x\n"),
        ];
        assert_eq!(problems(&files), Vec::<String>::new());
    }

    #[test]
    fn reports_missing_targets_with_file_and_line() {
        let files = [(
            "docs/A.md",
            "ok\n\n[gone](missing.md) and [esc](../../x.md)\n",
        )];
        let p = problems(&files);
        assert_eq!(
            p,
            [
                "docs/A.md:3: missing.md: no such file or directory",
                "docs/A.md:3: ../../x.md: the link leaves the repository",
            ]
        );
    }

    #[test]
    fn directories_are_valid_targets() {
        let files = [
            ("README.md", "[d](docs) [e](docs/) [f](./)\n"),
            ("docs/X.md", "x\n"),
        ];
        assert_eq!(problems(&files), Vec::<String>::new());
    }

    #[test]
    fn same_file_and_cross_file_anchors() {
        let files = [
            (
                "A.md",
                "# One\n\n## Two words\n\n[1](#one) [2](#two-words) [3](B.md#sub-b) [4](#nope) [5](B.md#nope)\n",
            ),
            ("B.md", "## Sub B\n"),
        ];
        let p = problems(&files);
        assert_eq!(
            p,
            [
                "A.md:5: #nope: no heading or anchor `#nope` in A.md",
                "A.md:5: B.md#nope: no heading or anchor `#nope` in B.md",
            ]
        );
    }

    #[test]
    fn anchors_match_exactly_but_accept_github_prefix_and_percent_escapes() {
        let files = [(
            "A.md",
            "## Über uns\n\n[1](#%C3%BCber-uns) [2](#user-content-%C3%BCber-uns) [3](#Über-uns)\n",
        )];
        assert_eq!(
            problems(&files),
            ["A.md:3: #Über-uns: no heading or anchor `#Über-uns` in A.md"]
        );
    }

    #[test]
    fn code_files_take_line_references_only() {
        let files = [
            (
                "README.md",
                "[1](src/lib.rs#L10) [2](src/lib.rs#L10-L20) [3](src/lib.rs#L1C2-L3C4) [4](src/lib.rs#frag)\n",
            ),
            ("src/lib.rs", "fn main() {}\n"),
        ];
        let p = problems(&files);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].starts_with("README.md:1: src/lib.rs#frag: `#frag` on a non-Markdown file"));
    }

    #[test]
    fn external_links_are_not_checked_and_do_not_count() {
        let files = [(
            "A.md",
            "[a](https://example.com/x) [b](mailto:a@example.com) [c](//cdn.example.com/x) [d](HTTP://EXAMPLE.COM)\n",
        )];
        let r = repo(&files).check(|_| true);
        assert!(r.problems.is_empty());
        assert_eq!(r.links, 0);
        assert_eq!(r.files, 1);
    }

    #[test]
    fn empty_destinations_and_queries() {
        let files = [
            ("A.md", "[a]() [b](B.md?plain=1) [c](B.md?x=1#top)\n"),
            ("B.md", "# Top\n"),
        ];
        assert_eq!(problems(&files), ["A.md:1: : empty link destination"]);
    }

    #[test]
    fn skipped_files_are_still_valid_targets() {
        let files = [
            ("docs/A.md", "[f](../tests/fixtures/F.md#h)\n"),
            ("tests/fixtures/F.md", "# H\n\n[broken](nowhere.md)\n"),
        ];
        let r = repo(&files).check(|p| !p.starts_with("tests/fixtures/"));
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert_eq!(r.files, 1);
    }

    #[test]
    fn crlf_and_odd_input_do_not_panic() {
        for text in [
            "",
            "](",
            "[x](",
            "[x](<",
            "```",
            "`",
            "<!--",
            "# ",
            "#\n===",
            "[x](a \"unterminated",
            "[x]: ",
            "\u{feff}# Title\n",
            "[é](ü.md#ñ) `é` <!-- ü -->",
        ] {
            let _ = extract_links(text);
            let _ = anchors_of(text);
        }
    }

    #[test]
    fn line_reference_forms() {
        for ok in ["L1", "L10-L20", "L3C4", "L3C4-L5C6"] {
            assert!(is_line_reference(ok), "{ok}");
        }
        for bad in ["", "L", "l1", "L1-", "L1C", "L1-2", "x"] {
            assert!(!is_line_reference(bad), "{bad}");
        }
    }

    #[test]
    fn scheme_detection() {
        for yes in [
            "https://x",
            "mailto:a@b",
            "tel:123",
            "a+b.c-d:x",
            "file:///x",
        ] {
            assert!(has_scheme(yes), "{yes}");
        }
        for no in ["docs/a.md", "./a:b.md", "1abc:x", "#frag", "", "a b:c"] {
            assert!(!has_scheme(no), "{no}");
        }
    }
}
