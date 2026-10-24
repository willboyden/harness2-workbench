//! The newest lines of a job log.
//!
//! The server pages a log forward from a line number and ends each page near 1 MiB, so "the
//! last N lines" can need more than one page when the lines are long. [`tail`] pages forward
//! from `total - N`, keeps the newest lines that fit in `max_bytes` of text, and, when a page
//! does not reach the end, jumps to where the rest should fit, judging by the average line
//! size seen so far. A tail therefore costs a few requests, not one per megabyte.

use std::collections::VecDeque;

use h2w_api::{LogChunk, LogLine};

use crate::client::{ApiClient, ApiError};
use crate::token::Token;

/// Pages one tail read requests at most.
const MAX_PAGES: usize = 8;

/// The last `lines` lines of job `job_id`'s log (of `total` lines), within `max_bytes` of
/// line text; older lines are dropped first. `next` and `complete` are the last page's.
///
/// # Errors
/// What the server answered when a page could not be read.
pub(crate) async fn tail(
    api: &ApiClient,
    token: &Token,
    job_id: &str,
    total: u64,
    lines: u32,
    max_bytes: usize,
) -> Result<LogChunk, ApiError> {
    let mut from = total.saturating_sub(u64::from(lines));
    let mut kept: VecDeque<LogLine> = VecDeque::new();
    let mut bytes = 0usize;
    let mut last = LogChunk {
        lines: Vec::new(),
        next: from,
        complete: false,
    };
    for _ in 0..MAX_PAGES {
        let query = [("from", from.to_string()), ("limit", lines.to_string())];
        let page: LogChunk = api.get(token, &["jobs", job_id, "log"], &query).await?;
        let page_bytes: usize = page.lines.iter().map(|l| l.text.len()).sum();
        let page_lines = page.lines.len();
        let reached_end = page.complete || page.next >= total || page_lines == 0;
        for line in &page.lines {
            bytes += line.text.len();
            kept.push_back(line.clone());
        }
        while bytes > max_bytes && kept.len() > 1 {
            if let Some(old) = kept.pop_front() {
                bytes -= old.text.len();
            }
        }
        last = LogChunk {
            lines: Vec::new(),
            next: page.next,
            complete: page.complete,
        };
        if reached_end {
            break;
        }
        from = next_from(page.next, total, page_bytes, page_lines, max_bytes);
        if from > page.next {
            // Skipping ahead: what was kept is older than what comes next.
            kept.clear();
            bytes = 0;
        }
    }
    last.lines = kept.into();
    Ok(last)
}

/// Where to continue after a page that ended at `next` short of `total`: just after it,
/// unless the lines left are many more than `max_bytes` can hold at the page's average line
/// size, in which case start where that many lines from the end begin.
fn next_from(next: u64, total: u64, page_bytes: usize, page_lines: usize, max_bytes: usize) -> u64 {
    let average = (page_bytes / page_lines.max(1)).max(1);
    let fit = u64::try_from(max_bytes / average)
        .unwrap_or(u64::MAX)
        .max(1);
    total.saturating_sub(fit).max(next)
}

#[cfg(test)]
mod tests {
    use super::next_from;

    #[test]
    fn skips_ahead_only_when_the_rest_cannot_fit() {
        // 1 MiB page of 16 lines (64 KiB each); 100 KiB fit about one line: jump near the end.
        assert_eq!(next_from(16, 5000, 1 << 20, 16, 100_000), 4999);
        // Short lines: the rest fits, so continue right after the page.
        assert_eq!(next_from(4000, 5000, 1 << 20, 4000, 100_000_000), 4000);
        // Never step back before the page's end.
        assert_eq!(next_from(4990, 5000, 1 << 20, 16, 100_000), 4999);
        assert_eq!(next_from(10, 10, 100, 0, 100), 10);
    }
}
