//! The job page's log: lines from the live stream and from `GET /jobs/{id}/log` merged in
//! line-number order, without duplicates, with a bounded memory footprint.

use std::collections::{BTreeMap, VecDeque};

use h2s_api::{LogLine, LogStream};

/// Lines kept in the browser; older ones are dropped from the front.
pub(crate) const DEFAULT_CAP: usize = 20_000;
/// Out-of-order lines held while a gap is being filled.
const PENDING_CAP: usize = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Push {
    Appended,
    /// Already have it (or it is older than what is kept).
    Duplicate,
    /// Arrived ahead of a missing range starting at `from`; held until the gap fills.
    Gap {
        from: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LogBuffer {
    lines: VecDeque<LogLine>,
    /// Line number expected next (one past the last appended line).
    next: u64,
    /// Lines dropped from the front because of the cap.
    dropped: u64,
    pending: BTreeMap<u64, LogLine>,
    cap: usize,
}

impl Default for LogBuffer {
    fn default() -> Self {
        LogBuffer::with_cap(DEFAULT_CAP)
    }
}

impl LogBuffer {
    pub(crate) fn with_cap(cap: usize) -> LogBuffer {
        LogBuffer {
            lines: VecDeque::new(),
            next: 0,
            dropped: 0,
            pending: BTreeMap::new(),
            cap: cap.max(1),
        }
    }

    /// The line number to request from `GET /log?from=` to continue.
    pub(crate) fn next(&self) -> u64 {
        self.next
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lines.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Lines no longer held in the browser.
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    /// True while lines past a gap wait for the gap to be filled.
    pub(crate) fn has_gap(&self) -> bool {
        !self.pending.is_empty()
    }

    fn append(&mut self, line: LogLine) {
        self.next = line.n + 1;
        self.lines.push_back(line);
        while self.lines.len() > self.cap {
            self.lines.pop_front();
            self.dropped += 1;
        }
    }

    fn drain_pending(&mut self) {
        while let Some(line) = self.pending.remove(&self.next) {
            self.append(line);
        }
        // Anything older than `next` that is still pending is a duplicate.
        let next = self.next;
        self.pending.retain(|n, _| *n >= next);
    }

    pub(crate) fn push(&mut self, line: LogLine) -> Push {
        if line.n < self.next {
            return Push::Duplicate;
        }
        if line.n > self.next {
            let from = self.next;
            if self.pending.len() < PENDING_CAP {
                self.pending.insert(line.n, line);
            }
            return Push::Gap { from };
        }
        self.append(line);
        self.drain_pending();
        Push::Appended
    }

    /// Merge a chunk from `GET /log` (or any batch). Returns how many lines were appended.
    pub(crate) fn extend(&mut self, lines: impl IntoIterator<Item = LogLine>) -> usize {
        let before = self.next;
        for l in lines {
            let _ = self.push(l);
        }
        usize::try_from(self.next - before).unwrap_or(usize::MAX)
    }

    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &LogLine> + ExactSizeIterator {
        self.lines.iter()
    }

    #[cfg(test)]
    /// The last `n` lines, oldest first.
    pub(crate) fn tail(&self, n: usize) -> Vec<LogLine> {
        let skip = self.lines.len().saturating_sub(n);
        self.lines.iter().skip(skip).cloned().collect()
    }

    /// Lines matching a case-insensitive query and the stream filter, at most `limit` of
    /// them (the newest), oldest first.
    pub(crate) fn search(
        &self,
        query: &str,
        streams: &[LogStream],
        limit: usize,
    ) -> (Vec<LogLine>, usize) {
        let q = query.to_lowercase();
        let hits: Vec<&LogLine> = self
            .lines
            .iter()
            .filter(|l| streams.contains(&l.stream))
            .filter(|l| q.is_empty() || l.text.to_lowercase().contains(&q))
            .collect();
        let total = hits.len();
        let skip = total.saturating_sub(limit);
        (hits.into_iter().skip(skip).cloned().collect(), total)
    }

    /// The whole retained log as plain text (for "download log").
    pub(crate) fn to_text(&self) -> String {
        let mut out = String::new();
        for l in &self.lines {
            out.push_str(&l.ts);
            out.push(' ');
            out.push_str(stream_name(l.stream));
            out.push_str(": ");
            out.push_str(&l.text);
            out.push('\n');
        }
        out
    }
}

pub(crate) fn stream_name(s: LogStream) -> &'static str {
    match s {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
        LogStream::System => "system",
    }
}

/// CSS class for a log line: the stream, plus `log-alert` for lines that look like errors.
pub(crate) fn line_class(l: &LogLine) -> &'static str {
    let t = l.text.trim_start();
    let alarming = t.starts_with("Traceback")
        || t.starts_with("ERROR")
        || t.starts_with("FAIL")
        || t.contains("Error:");
    match (l.stream, alarming) {
        (LogStream::System, _) => "log-line log-system",
        (_, true) => "log-line log-alert",
        (LogStream::Stderr, false) => "log-line log-stderr",
        (LogStream::Stdout, false) => "log-line log-stdout",
    }
}

/// Split `text` into `(segment, is_match)` pieces for highlighting a case-insensitive query.
pub(crate) fn highlight(text: &str, query: &str) -> Vec<(String, bool)> {
    if query.is_empty() {
        return vec![(text.to_owned(), false)];
    }
    let lower = text.to_lowercase();
    let q = query.to_lowercase();
    // Lowercasing can change byte lengths for some scripts; only highlight when it did not.
    if lower.len() != text.len() {
        return vec![(text.to_owned(), false)];
    }
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find(&q) {
        let start = pos + i;
        let end = start + q.len();
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            break;
        }
        if start > pos {
            out.push((text[pos..start].to_owned(), false));
        }
        out.push((text[start..end].to_owned(), true));
        pos = end;
    }
    if pos < text.len() {
        out.push((text[pos..].to_owned(), false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(n: u64) -> LogLine {
        LogLine {
            n,
            ts: format!("t{n}"),
            stream: if n.is_multiple_of(2) {
                LogStream::Stdout
            } else {
                LogStream::Stderr
            },
            text: format!("line {n}"),
        }
    }

    #[test]
    fn appends_in_order_and_drops_duplicates() {
        let mut b = LogBuffer::default();
        assert!(b.is_empty());
        assert_eq!(b.push(l(0)), Push::Appended);
        assert_eq!(b.push(l(1)), Push::Appended);
        assert_eq!(b.push(l(1)), Push::Duplicate);
        assert_eq!(b.next(), 2);
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn holds_lines_past_a_gap_until_it_fills() {
        let mut b = LogBuffer::default();
        b.push(l(0));
        assert_eq!(b.push(l(3)), Push::Gap { from: 1 });
        assert_eq!(b.push(l(4)), Push::Gap { from: 1 });
        assert!(b.has_gap());
        assert_eq!(b.len(), 1);
        // The backfill brings 1..=3; 3 is a duplicate of the pending one.
        assert_eq!(b.extend([l(1), l(2), l(3)]), 4);
        assert_eq!(b.next(), 5);
        assert!(!b.has_gap());
        let ns: Vec<u64> = b.iter().map(|x| x.n).collect();
        assert_eq!(ns, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn caps_memory() {
        let mut b = LogBuffer::with_cap(3);
        b.extend((0..10).map(l));
        assert_eq!(b.len(), 3);
        assert_eq!(b.dropped(), 7);
        assert_eq!(
            b.tail(2).iter().map(|x| x.n).collect::<Vec<_>>(),
            vec![8, 9]
        );
        assert_eq!(b.push(l(5)), Push::Duplicate);
    }

    #[test]
    fn searches_by_text_and_stream() {
        let mut b = LogBuffer::default();
        b.extend((0..10).map(l));
        let (hits, total) = b.search("LINE", &[LogStream::Stderr], 2);
        assert_eq!(total, 5);
        assert_eq!(hits.iter().map(|x| x.n).collect::<Vec<_>>(), vec![7, 9]);
        let (hits, total) = b.search("line 4", &[LogStream::Stdout, LogStream::Stderr], 100);
        assert_eq!((hits.len(), total), (1, 1));
        assert!(b.to_text().contains("t3 stderr: line 3\n"));
    }

    #[test]
    fn highlights_matches() {
        assert_eq!(
            highlight("Error: an error", "error"),
            vec![
                ("Error".to_owned(), true),
                (": an ".to_owned(), false),
                ("error".to_owned(), true)
            ]
        );
        assert_eq!(highlight("abc", ""), vec![("abc".to_owned(), false)]);
        assert_eq!(highlight("abc", "x"), vec![("abc".to_owned(), false)]);
    }

    #[test]
    fn classes() {
        let mut x = l(1);
        assert_eq!(line_class(&x), "log-line log-stderr");
        x.text = "Traceback (most recent call last):".into();
        assert_eq!(line_class(&x), "log-line log-alert");
        x.stream = LogStream::System;
        assert_eq!(line_class(&x), "log-line log-system");
    }
}
