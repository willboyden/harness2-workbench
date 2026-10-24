//! The job page's log: lines from the live stream and from `GET /jobs/{id}/log` merged in
//! line-number order, without duplicates, with a bounded memory footprint.

use std::collections::{BTreeMap, VecDeque};

use h2w_api::{LogLine, LogStream};

/// Lines kept in the browser; older ones are dropped from the front.
pub(crate) const DEFAULT_CAP: usize = 20_000;
/// Bytes of line text kept in the browser (the newest line is kept whatever its size).
pub(crate) const DEFAULT_BYTE_CAP: usize = 16 * 1024 * 1024;
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
    /// Lines before the first one held: dropped from the front because of the caps, or
    /// skipped by [`LogBuffer::catch_up`].
    dropped: u64,
    pending: BTreeMap<u64, LogLine>,
    cap: usize,
    /// Bytes of text in `lines`.
    bytes: usize,
    byte_cap: usize,
}

impl Default for LogBuffer {
    fn default() -> Self {
        LogBuffer::with_caps(DEFAULT_CAP, DEFAULT_BYTE_CAP)
    }
}

impl LogBuffer {
    #[cfg(test)]
    pub(crate) fn with_cap(cap: usize) -> LogBuffer {
        LogBuffer::with_caps(cap, DEFAULT_BYTE_CAP)
    }

    pub(crate) fn with_caps(cap: usize, byte_cap: usize) -> LogBuffer {
        LogBuffer {
            lines: VecDeque::new(),
            next: 0,
            dropped: 0,
            pending: BTreeMap::new(),
            cap: cap.max(1),
            bytes: 0,
            byte_cap,
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
        self.bytes += line.text.len();
        self.lines.push_back(line);
        while self.lines.len() > self.cap || (self.bytes > self.byte_cap && self.lines.len() > 1) {
            let Some(old) = self.lines.pop_front() else {
                break;
            };
            self.bytes -= old.text.len();
            self.dropped += 1;
        }
    }

    /// Where to fetch from next for a log of `total` lines. When the lines before the
    /// newest ones that fit the caps would only be dropped again, skip them (and what is
    /// held, which is older still) instead of downloading them. `avg_line_bytes`, the
    /// average size in a page already fetched, lets the byte cap narrow the jump.
    pub(crate) fn catch_up(&mut self, total: u64, avg_line_bytes: Option<usize>) -> u64 {
        let fit = avg_line_bytes.map_or(self.cap, |avg| {
            self.cap.min(self.byte_cap / avg.max(1)).max(1)
        });
        let target = total.saturating_sub(u64::try_from(fit).unwrap_or(u64::MAX));
        if target > self.next {
            self.lines.clear();
            self.bytes = 0;
            self.dropped = target;
            self.next = target;
            self.drain_pending();
        }
        self.next
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
    fn caps_bytes_but_keeps_the_newest_line() {
        // "line N" is 6 bytes: 20 bytes hold three lines.
        let mut b = LogBuffer::with_caps(100, 20);
        b.extend((0..10).map(l));
        assert_eq!(b.len(), 3);
        assert_eq!(b.dropped(), 7);
        let mut big = l(10);
        big.text = "x".repeat(50);
        b.push(big);
        assert_eq!(b.iter().map(|x| x.n).collect::<Vec<_>>(), vec![10]);
        assert_eq!(b.dropped(), 10);
    }

    #[test]
    fn catching_up_skips_only_lines_it_would_drop() {
        let mut full = LogBuffer::with_cap(3);
        full.extend((0..10).map(l));
        let mut b = LogBuffer::with_cap(3);
        b.extend((0..2).map(l));
        // Ten lines and room for three: fetch from 7, as if 0..7 had come and gone.
        assert_eq!(b.catch_up(10, None), 7);
        assert_eq!((b.len(), b.dropped()), (0, 7));
        b.extend((7..10).map(l));
        assert_eq!(b, full);
        // Nothing to skip when the rest fits.
        assert_eq!(b.catch_up(12, None), 10);
        assert_eq!(b.len(), 3);
    }

    #[test]
    fn catching_up_uses_the_byte_cap_and_keeps_pending_lines() {
        // 60 bytes at 6 bytes a line hold ten of the cap's hundred lines.
        let mut b = LogBuffer::with_caps(100, 60);
        b.push(l(0));
        assert_eq!(b.push(l(95)), Push::Gap { from: 1 });
        assert_eq!(b.push(l(3)), Push::Gap { from: 1 });
        assert_eq!(b.catch_up(100, Some(6)), 90);
        assert!(b.has_gap(), "line 95 still waits for 90..95");
        b.extend((90..95).map(l));
        assert_eq!(b.next(), 96);
        assert!(!b.has_gap());
        assert_eq!(b.dropped(), 90);
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
