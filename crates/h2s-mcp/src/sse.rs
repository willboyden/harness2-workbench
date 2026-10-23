//! Incremental `text/event-stream` parser for `GET /api/v1/jobs/{id}/events`.
//!
//! Contract v1.1: frames are unnamed (no `event:` line), each `data:` field carries exactly
//! one `JobEvent` JSON object, `id:` may carry a log line number, and comment frames
//! (`: keep-alive`) may appear. The parser follows the WHATWG event-stream rules for the parts
//! that matter here: `LF`, `CRLF` and `CR` line endings, comments, multi-line `data:` joined
//! with `\n`, a blank line dispatching the frame, and an optional single space after the colon.

/// Largest event payload accepted; a single `JobEvent` is a few KiB at most (a snapshot holds
/// one job).
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum SseError {
    #[error("an event in the job event stream exceeded {MAX_EVENT_BYTES} bytes")]
    TooLarge,
}

/// Feeds raw bytes in, returns the `data` payload of every completed frame.
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    /// Bytes of the current, not yet terminated line.
    line: Vec<u8>,
    /// The previous chunk ended with CR: a leading LF in the next chunk belongs to it.
    pending_cr: bool,
    /// Data lines of the current frame, joined with `\n`.
    data: String,
    /// The current frame has at least one `data` field.
    has_data: bool,
}

impl SseParser {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Parse the next chunk of the stream.
    ///
    /// # Errors
    /// A frame or line grew beyond the size cap.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, SseError> {
        let mut events = Vec::new();
        let mut rest = chunk;
        if self.pending_cr {
            self.pending_cr = false;
            if let Some(stripped) = rest.strip_prefix(b"\n") {
                rest = stripped;
            }
        }
        while let Some(pos) = rest.iter().position(|&b| b == b'\n' || b == b'\r') {
            self.line.extend_from_slice(&rest[..pos]);
            if self.line.len() > MAX_EVENT_BYTES {
                return Err(SseError::TooLarge);
            }
            let terminator = rest[pos];
            rest = &rest[pos + 1..];
            if terminator == b'\r' {
                if rest.is_empty() {
                    self.pending_cr = true;
                } else if let Some(stripped) = rest.strip_prefix(b"\n") {
                    rest = stripped;
                }
            }
            let line = std::mem::take(&mut self.line);
            if let Some(event) = self.process_line(&line)? {
                events.push(event);
            }
        }
        self.line.extend_from_slice(rest);
        if self.line.len() > MAX_EVENT_BYTES {
            return Err(SseError::TooLarge);
        }
        Ok(events)
    }

    fn process_line(&mut self, line: &[u8]) -> Result<Option<String>, SseError> {
        if line.is_empty() {
            // Blank line: dispatch the frame (frames without data are ignored).
            if !self.has_data {
                return Ok(None);
            }
            self.has_data = false;
            let mut data = std::mem::take(&mut self.data);
            if data.ends_with('\n') {
                data.pop();
            }
            return Ok(Some(data));
        }
        if line.first() == Some(&b':') {
            return Ok(None); // comment, e.g. a keep-alive
        }
        let line = String::from_utf8_lossy(line);
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line.as_ref(), ""),
        };
        if field == "data" {
            if self.data.len() + value.len() + 1 > MAX_EVENT_BYTES {
                return Err(SseError::TooLarge);
            }
            self.data.push_str(value);
            self.data.push('\n');
            self.has_data = true;
        }
        // `event`, `id` and `retry` do not change how job events are handled.
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(chunks: &[&[u8]]) -> Vec<String> {
        let mut parser = SseParser::new();
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend(parser.push(chunk).unwrap());
        }
        out
    }

    #[test]
    fn unnamed_frames_with_ids_and_comments() {
        let stream = b": keep-alive\n\ndata: {\"type\":\"heartbeat\",\"ts\":\"t\"}\n\nid: 7\ndata: {\"a\":1}\n\n";
        assert_eq!(
            feed(&[stream]),
            vec![
                r#"{"type":"heartbeat","ts":"t"}"#.to_owned(),
                r#"{"a":1}"#.to_owned()
            ]
        );
    }

    #[test]
    fn frames_split_across_chunks_and_line_endings() {
        let out = feed(&[b"da", b"ta: x\r", b"\n\r\n", b"data:y\rdata: z\r\r"]);
        assert_eq!(out, vec!["x".to_owned(), "y\nz".to_owned()]);
    }

    #[test]
    fn named_events_and_empty_frames() {
        // A named frame still carries a JobEvent; frames without data are skipped.
        let out = feed(&[b"event: message\ndata: 1\n\nretry: 10\n\n\n\ndata\n\n"]);
        assert_eq!(out, vec!["1".to_owned(), String::new()]);
    }

    #[test]
    fn incomplete_frame_is_held_back() {
        let mut parser = SseParser::new();
        assert!(parser.push(b"data: partial").unwrap().is_empty());
        assert_eq!(parser.push(b"\n\n").unwrap(), vec!["partial".to_owned()]);
    }

    #[test]
    fn oversized_events_are_refused() {
        let mut parser = SseParser::new();
        let big = vec![b'a'; MAX_EVENT_BYTES + 1];
        assert_eq!(parser.push(&big), Err(SseError::TooLarge));
    }
}
