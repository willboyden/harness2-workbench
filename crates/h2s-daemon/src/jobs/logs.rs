//! Job logs: bounded line reading from child pipes, the JSON-lines log file, paging, and
//! path masking.
//!
//! Each line is at most [`MAX_LINE_BYTES`] (longer lines are cut and marked), decoded as
//! lossy UTF-8, and stored as one JSON [`LogLine`] per file line in
//! `<data_dir>/logs/jobs/<id>.log` (0600).

use std::borrow::Cow;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use h2s_api::LogLine;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};

/// Longest line kept (bytes of text); the rest of a longer line is dropped.
pub const MAX_LINE_BYTES: usize = 64 * 1024;
/// Appended to a cut line.
pub const TRUNCATED_MARK: &str = " … [line truncated]";

/// Reads `\n`-terminated lines without ever buffering more than [`MAX_LINE_BYTES`] of one.
pub struct LineReader<R> {
    inner: BufReader<R>,
    eof: bool,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    pub fn new(r: R) -> Self {
        LineReader {
            inner: BufReader::with_capacity(16 * 1024, r),
            eof: false,
        }
    }

    /// The next line without its terminator (`\n` or `\r\n`); `None` at end of input.
    ///
    /// # Errors
    /// Read errors from the pipe.
    pub async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        if self.eof {
            return Ok(None);
        }
        let mut line: Vec<u8> = Vec::new();
        let mut truncated = false;
        loop {
            let available = self.inner.fill_buf().await?;
            if available.is_empty() {
                self.eof = true;
                if line.is_empty() && !truncated {
                    return Ok(None);
                }
                break;
            }
            let (chunk, consumed, done) = match available.iter().position(|&b| b == b'\n') {
                Some(i) => (&available[..i], i + 1, true),
                None => (available, available.len(), false),
            };
            let room = MAX_LINE_BYTES.saturating_sub(line.len());
            if chunk.len() > room {
                line.extend_from_slice(&chunk[..room]);
                truncated = true;
            } else {
                line.extend_from_slice(chunk);
            }
            self.inner.consume(consumed);
            if done {
                break;
            }
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let mut text = String::from_utf8_lossy(&line).into_owned();
        if truncated {
            // Do not leave half a character before the marker.
            while text.ends_with('\u{fffd}') && text.len() > MAX_LINE_BYTES - 4 {
                text.pop();
            }
            text.push_str(TRUNCATED_MARK);
        }
        Ok(Some(text))
    }
}

/// The log file of a job.
#[must_use]
pub fn log_path(logs_dir: &Path, job_id: &str) -> PathBuf {
    logs_dir.join(format!("{job_id}.log"))
}

/// Appends JSON lines to a job's log file.
pub struct LogFile {
    file: tokio::io::BufWriter<tokio::fs::File>,
}

impl LogFile {
    /// Open for appending (created 0600). Returns the file and the number of complete lines
    /// already in it (non-zero only if the id was reused, which job ids never are).
    ///
    /// # Errors
    /// I/O errors.
    pub async fn open(path: &Path) -> std::io::Result<(LogFile, u64)> {
        let p = path.to_path_buf();
        let (file, existing) = tokio::task::spawn_blocking(move || {
            let existing = count_lines(&p).unwrap_or(0);
            crate::util::open_private_append(&p).map(|f| (f, existing))
        })
        .await
        .map_err(std::io::Error::other)??;
        Ok((
            LogFile {
                file: tokio::io::BufWriter::new(tokio::fs::File::from_std(file)),
            },
            existing,
        ))
    }

    /// Append one line (buffered; call [`LogFile::flush`] to make it visible to readers).
    ///
    /// # Errors
    /// I/O or serialization errors.
    pub async fn append(&mut self, line: &LogLine) -> std::io::Result<()> {
        let mut json = serde_json::to_vec(line).map_err(std::io::Error::other)?;
        json.push(b'\n');
        self.file.write_all(&json).await
    }

    /// Flush buffered lines to the file.
    ///
    /// # Errors
    /// I/O errors.
    pub async fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush().await
    }
}

fn count_lines(path: &Path) -> std::io::Result<u64> {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut r = std::io::BufReader::new(f);
    let mut n = 0u64;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let read = r.read_until(b'\n', &mut buf)?;
        if read == 0 || buf.last() != Some(&b'\n') {
            return Ok(n);
        }
        n += 1;
    }
}

/// Lines `from..from+limit` of a log file (blocking; call from `spawn_blocking`). Returns
/// the lines and the next line number. A partly written last line is not returned.
///
/// # Errors
/// I/O errors other than a missing file (which reads as empty).
pub fn read_page(path: &Path, from: u64, limit: u64) -> std::io::Result<(Vec<LogLine>, u64)> {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), from)),
        Err(e) => return Err(e),
    };
    let mut r = std::io::BufReader::with_capacity(64 * 1024, f);
    let mut buf = Vec::new();
    let mut n = 0u64;
    let mut out = Vec::new();
    while out.len() < usize::try_from(limit).unwrap_or(usize::MAX) {
        buf.clear();
        let read = r.read_until(b'\n', &mut buf)?;
        if read == 0 || buf.last() != Some(&b'\n') {
            break;
        }
        if n >= from {
            match serde_json::from_slice::<LogLine>(&buf) {
                Ok(line) => out.push(line),
                Err(_) => out.push(LogLine {
                    n,
                    ts: String::new(),
                    stream: h2s_api::LogStream::System,
                    text: "[unreadable log line]".into(),
                }),
            }
        }
        n += 1;
    }
    let next = from + u64::try_from(out.len()).unwrap_or(0);
    Ok((out, next))
}

/// Replaces known absolute host paths in log text with short placeholders, so job logs
/// shown to viewers do not reveal the server's directory layout or user name.
#[derive(Debug, Clone, Default)]
pub struct PathMasker {
    /// (prefix, placeholder), longest prefix first.
    pairs: Vec<(String, String)>,
}

impl PathMasker {
    #[must_use]
    pub fn new(mut pairs: Vec<(String, String)>) -> PathMasker {
        pairs.retain(|(p, _)| p.len() > 1 && p.starts_with('/'));
        for (p, _) in &mut pairs {
            while p.len() > 1 && p.ends_with('/') {
                p.pop();
            }
        }
        pairs.sort_by_key(|p| std::cmp::Reverse(p.0.len()));
        pairs.dedup_by(|a, b| a.0 == b.0);
        PathMasker { pairs }
    }

    /// `text` with every boundary-delimited occurrence of a prefix replaced.
    #[must_use]
    pub fn mask<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if !self.pairs.iter().any(|(p, _)| text.contains(p.as_str())) {
            return Cow::Borrowed(text);
        }
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        'outer: while !rest.is_empty() {
            for (prefix, placeholder) in &self.pairs {
                if let Some(after) = rest.strip_prefix(prefix.as_str()) {
                    let boundary = after
                        .chars()
                        .next()
                        .is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.')));
                    let before_ok = out.chars().last().is_none_or(|c| {
                        !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
                    });
                    if boundary && before_ok {
                        out.push_str(placeholder);
                        rest = after;
                        continue 'outer;
                    }
                }
            }
            let mut chars = rest.chars();
            if let Some(c) = chars.next() {
                out.push(c);
            }
            rest = chars.as_str();
        }
        Cow::Owned(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    async fn lines_of(input: &[u8]) -> Vec<String> {
        let mut r = LineReader::new(input);
        let mut out = Vec::new();
        while let Some(l) = r.next_line().await.unwrap() {
            out.push(l);
        }
        out
    }

    #[tokio::test]
    async fn splits_lines_and_handles_crlf_and_tail() {
        assert_eq!(
            lines_of(b"a\r\nb\n\nc").await,
            vec!["a".to_owned(), "b".into(), String::new(), "c".into()]
        );
        assert!(lines_of(b"").await.is_empty());
        assert_eq!(
            lines_of(b"\xff\xfeok\n").await,
            vec!["\u{fffd}\u{fffd}ok".to_owned()]
        );
    }

    #[tokio::test]
    async fn caps_long_lines() {
        let mut input = vec![b'x'; MAX_LINE_BYTES * 3];
        input.extend_from_slice(b"\nnext\n");
        let lines = lines_of(&input).await;
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(TRUNCATED_MARK));
        assert!(lines[0].len() <= MAX_LINE_BYTES + TRUNCATED_MARK.len());
        assert_eq!(lines[1], "next");
    }

    #[tokio::test]
    async fn file_round_trip_and_paging() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(dir.path(), "job");
        let (mut f, existing) = LogFile::open(&path).await.unwrap();
        assert_eq!(existing, 0);
        for n in 0..10u64 {
            f.append(&LogLine {
                n,
                ts: "t".into(),
                stream: h2s_api::LogStream::Stdout,
                text: format!("line {n}"),
            })
            .await
            .unwrap();
        }
        f.flush().await.unwrap();
        let (page, next) = read_page(&path, 3, 4).unwrap();
        assert_eq!(
            page.iter().map(|l| l.n).collect::<Vec<_>>(),
            vec![3, 4, 5, 6]
        );
        assert_eq!(next, 7);
        let (page, next) = read_page(&path, 8, 100).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(next, 10);
        let (page, next) = read_page(&path, 50, 10).unwrap();
        assert!(page.is_empty());
        assert_eq!(next, 50);
        let (page, _) = read_page(&dir.path().join("missing.log"), 0, 10).unwrap();
        assert!(page.is_empty());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // A partly written line is invisible to readers.
        use std::io::Write;
        let mut raw = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        raw.write_all(b"{\"n\":10").unwrap();
        let (page, _) = read_page(&path, 9, 100).unwrap();
        assert_eq!(page.len(), 1);
        let (_f, existing) = LogFile::open(&path).await.unwrap();
        assert_eq!(existing, 10);
    }

    #[test]
    fn masks_paths_on_boundaries_only() {
        let m = PathMasker::new(vec![
            ("/srv/h2s/experiments/demo".into(), "<experiment>".into()),
            ("/srv/h2s".into(), "<data>".into()),
            ("/var/lib/alice/".into(), "~".into()),
        ]);
        assert_eq!(
            m.mask("record: /srv/h2s/experiments/demo/lab/adapt/x.json"),
            "record: <experiment>/lab/adapt/x.json"
        );
        assert_eq!(m.mask("db at /srv/h2s/studio.db"), "db at <data>/studio.db");
        assert_eq!(m.mask("/srv/h2s2/other"), "/srv/h2s2/other");
        assert_eq!(m.mask("x/srv/h2s/y"), "x/srv/h2s/y");
        assert_eq!(m.mask("cd /var/lib/alice"), "cd ~");
        assert!(matches!(m.mask("nothing here"), Cow::Borrowed(_)));
    }

    proptest! {
        #[test]
        fn masking_never_panics(s in ".{0,100}") {
            let m = PathMasker::new(vec![("/a/b".into(), "<x>".into())]);
            let _ = m.mask(&s);
        }

        #[test]
        fn reader_preserves_short_lines(lines in proptest::collection::vec("[^\n\r]{0,50}", 0..20)) {
            let mut input = lines.join("\n").into_bytes();
            if !lines.is_empty() { input.push(b'\n'); }
            let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
            let got = rt.block_on(lines_of(&input));
            prop_assert_eq!(got, lines);
        }
    }
}
