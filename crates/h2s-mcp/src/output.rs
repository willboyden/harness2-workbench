//! Tool results: pretty JSON text (truncated with a note above ~100 KB) plus structured
//! content, and tool errors that carry the server's message.

use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;

/// Largest text block a tool returns; beyond it the text is cut with an explicit note.
pub(crate) const MAX_TEXT_BYTES: usize = 100_000;

/// Largest value also attached as structured content (for programmatic clients). Text for the
/// model is capped separately at [`MAX_TEXT_BYTES`].
const MAX_STRUCTURED_BYTES: usize = 1024 * 1024;

/// A successful result: the value as pretty JSON in the first text block, an optional note in
/// a second block, and the value as structured content when it is a JSON object of sane size.
///
/// `advice` tells the reader how to get the rest when the text has to be truncated.
pub(crate) fn json<T: Serialize>(value: &T, note: Option<String>, advice: &str) -> CallToolResult {
    let value = match serde_json::to_value(value) {
        Ok(value) => value,
        Err(e) => return error(format!("internal: cannot encode the result: {e}")),
    };
    let pretty = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    let full_len = pretty.len();
    let mut content = vec![ContentBlock::text(truncate(pretty, advice))];
    if let Some(note) = note {
        content.push(ContentBlock::text(note));
    }
    let mut result = CallToolResult::success(content);
    if value.is_object() && full_len <= MAX_STRUCTURED_BYTES {
        result.structured_content = Some(value);
    }
    result
}

/// A successful plain-text result (truncated like JSON).
pub(crate) fn text(text: String, advice: &str) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(truncate(text, advice))])
}

/// A tool error the caller sees (`is_error: true`). `message` starts with the error code word.
pub(crate) fn error(message: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

/// Cut `text` to [`MAX_TEXT_BYTES`] at a character boundary and say so.
pub(crate) fn truncate(text: String, advice: &str) -> String {
    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    let total = text.len();
    let mut cut = MAX_TEXT_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = String::with_capacity(cut + 256);
    out.push_str(&text[..cut]);
    out.push_str(&format!(
        "\n\n[output truncated: the full result is {total} bytes and only the first {cut} are \
         shown, so the JSON above is incomplete. {advice}]"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_text(result: &CallToolResult) -> &str {
        result.content[0]
            .as_text()
            .map(|t| t.text.as_str())
            .unwrap()
    }

    #[test]
    fn small_objects_are_text_and_structured() {
        let result = json(&serde_json::json!({"a": 1}), Some("note".into()), "");
        assert_eq!(result.is_error, Some(false));
        assert_eq!(first_text(&result), "{\n  \"a\": 1\n}");
        assert_eq!(result.content.len(), 2);
        assert_eq!(result.structured_content, Some(serde_json::json!({"a": 1})));
    }

    #[test]
    fn arrays_are_text_only() {
        let result = json(&vec![1, 2], None, "");
        assert!(result.structured_content.is_none());
        assert_eq!(result.content.len(), 1);
    }

    #[test]
    fn large_output_is_truncated_with_a_note() {
        let big = "é".repeat(MAX_TEXT_BYTES);
        let result = json(&serde_json::json!({ "text": big }), None, "Use paging.");
        let text = first_text(&result);
        assert!(text.len() < MAX_TEXT_BYTES + 400);
        assert!(text.contains("[output truncated: the full result is"));
        assert!(text.ends_with("Use paging.]"));
        // Programmatic clients still get the whole value.
        assert!(result.structured_content.is_some());
    }

    #[test]
    fn errors_are_flagged() {
        let result = error("forbidden: nope".into());
        assert_eq!(result.is_error, Some(true));
        assert_eq!(first_text(&result), "forbidden: nope");
    }
}
