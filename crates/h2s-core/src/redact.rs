//! Replace secret values in text before it is stored or shown.
//!
//! Every job log line, error message and audit detail passes through a [`Redactor`] built
//! from the secrets that job can see. Matching is exact substring; values shorter than
//! [`MIN_SECRET_LEN`] are still redacted but flagged by the server as weak.
//!
//! Besides each value itself, a few derived spellings are redacted under the same name,
//! because logs rarely print a secret exactly as stored: its JSON-escaped form, each
//! substantial line of a multi-line value (logs are line-split), each substantial string
//! inside a JSON value (a service-account key's `private_key`), and the standard and
//! URL-safe base64 encodings of the whole value, padded and unpadded.
//!
//! **Redaction is best effort.** It catches the spellings above and nothing else. A value
//! that is transformed in any other way is shown as it was printed: reversed, split across
//! lines or log records, spaced out, percent- or hex-encoded, base64-encoded together with
//! other text (an HTTP Basic `user:password` header) or wrapped onto several lines, hashed,
//! compressed. A job that holds a secret can print it in a form no redactor recognises, so
//! the logs of such a job are as sensitive as the secret.

use std::borrow::Cow;
use std::collections::BTreeSet;

/// Values shorter than this are unusual for credentials; the server warns when one is set.
pub const MIN_SECRET_LEN: usize = 8;

/// Derived fragments (lines, JSON strings) shorter than this are not redacted on their own,
/// so `{` or `"type": "x"` lines do not blank out unrelated log text.
const MIN_FRAGMENT_LEN: usize = 16;

/// Replaces each known secret value with `[REDACTED:<NAME>]`. Longer values are matched
/// first, so a secret that contains another is redacted whole.
#[derive(Clone, Default)]
pub struct Redactor {
    /// (name, value), sorted by value length descending. Never printed.
    entries: Vec<(String, String)>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: BTreeSet<&str> = self.entries.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("Redactor").field("names", &names).finish()
    }
}

/// The JSON string-literal body of `s` (escaped, without the quotes).
fn json_escaped(s: &str) -> Option<String> {
    let quoted = serde_json::to_string(s).ok()?;
    quoted
        .strip_prefix('"')
        .and_then(|q| q.strip_suffix('"'))
        .map(str::to_owned)
}

fn push_with_escape(out: &mut Vec<String>, v: &str) {
    out.push(v.to_owned());
    if let Some(e) = json_escaped(v).filter(|e| e != v) {
        out.push(e);
    }
}

fn push_lines(out: &mut Vec<String>, v: &str) {
    if v.contains('\n') {
        for line in v.lines().map(str::trim) {
            if line.chars().count() >= MIN_FRAGMENT_LEN {
                push_with_escape(out, line);
            }
        }
    }
}

fn json_strings(v: &serde_json::Value, out: &mut Vec<String>, depth: usize) {
    if depth > 8 {
        return;
    }
    match v {
        serde_json::Value::String(s) if s.chars().count() >= MIN_FRAGMENT_LEN => {
            push_with_escape(out, s);
            push_lines(out, s);
        }
        serde_json::Value::Array(items) => {
            for i in items {
                json_strings(i, out, depth + 1);
            }
        }
        serde_json::Value::Object(map) => {
            for i in map.values() {
                json_strings(i, out, depth + 1);
            }
        }
        _ => {}
    }
}

const BASE64_STANDARD: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_URL_SAFE: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// RFC 4648 base64 of `bytes` in `alphabet`, with `=` padding when `pad`.
fn base64(bytes: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let byte = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let group = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        // 1, 2 or 3 input bytes carry 2, 3 or 4 symbols.
        let symbols = chunk.len() + 1;
        for i in 0..4_usize {
            if i < symbols {
                let index = usize::try_from((group >> (18 - 6 * i)) & 63).unwrap_or(0);
                out.push(char::from(alphabet.get(index).copied().unwrap_or(b'=')));
            } else if pad {
                out.push('=');
            }
        }
    }
    out
}

/// Every spelling of `value` to redact.
fn variants(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    push_with_escape(&mut out, value);
    push_lines(&mut out, value);
    let trimmed = value.trim_start();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(doc) = serde_json::from_str::<serde_json::Value>(value)
    {
        json_strings(&doc, &mut out, 0);
    }
    for alphabet in [BASE64_STANDARD, BASE64_URL_SAFE] {
        for pad in [true, false] {
            out.push(base64(value.as_bytes(), alphabet, pad));
        }
    }
    out
}

impl Redactor {
    /// Build from `(name, value)` pairs. Empty values are ignored.
    #[must_use]
    pub fn new(secrets: Vec<(String, String)>) -> Self {
        let mut r = Redactor::default();
        for (name, value) in secrets {
            r.insert(name, value);
        }
        r.normalise();
        r
    }

    fn insert(&mut self, name: String, value: String) {
        if value.is_empty() {
            return;
        }
        let derived = variants(&value);
        for v in derived.into_iter().skip(1) {
            if !v.is_empty() {
                self.entries.push((name.clone(), v));
            }
        }
        self.entries.push((name, value));
    }

    /// Longest first; one entry per distinct value (the first name wins).
    fn normalise(&mut self) {
        self.entries
            .sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.1.cmp(&b.1)));
        self.entries.dedup_by(|b, a| a.1 == b.1);
    }

    /// Add one more secret (e.g. the bridge key once it is known).
    pub fn add(&mut self, name: String, value: String) {
        self.insert(name, value);
        self.normalise();
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `text` with every secret value replaced. Borrowed when nothing matched.
    #[must_use]
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if self.entries.is_empty() || text.is_empty() {
            return Cow::Borrowed(text);
        }
        let mut first = [false; 256];
        for (_, v) in &self.entries {
            if let Some(&b) = v.as_bytes().first() {
                first[usize::from(b)] = true;
            }
        }
        let bytes = text.as_bytes();
        let mut out: Option<String> = None;
        let mut last = 0;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            let hit = if first[usize::from(b)] {
                let rest = bytes.get(i..).unwrap_or_default();
                // Entries are sorted longest first, so the first match is the longest.
                self.entries
                    .iter()
                    .find(|(_, v)| rest.starts_with(v.as_bytes()))
            } else {
                None
            };
            match hit {
                Some((name, v)) => {
                    let o = out.get_or_insert_with(|| String::with_capacity(text.len()));
                    o.push_str(text.get(last..i).unwrap_or_default());
                    o.push_str("[REDACTED:");
                    o.push_str(name);
                    o.push(']');
                    i += v.len();
                    last = i;
                }
                None => i += 1,
            }
        }
        match out {
            None => Cow::Borrowed(text),
            Some(mut o) => {
                o.push_str(text.get(last..).unwrap_or_default());
                Cow::Owned(o)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn r(pairs: &[(&str, &str)]) -> Redactor {
        Redactor::new(
            pairs
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn replaces_longest_first_and_borrows_otherwise() {
        let red = r(&[("SHORT", "abc123"), ("LONG", "abc123def456"), ("EMPTY", "")]);
        assert_eq!(
            red.redact("k=abc123def456 and abc123."),
            "k=[REDACTED:LONG] and [REDACTED:SHORT]."
        );
        assert!(matches!(red.redact("nothing here"), Cow::Borrowed(_)));
        assert!(!red.is_empty());
        assert!(Redactor::default().is_empty());
        let dbg = format!("{red:?}");
        assert!(dbg.contains("LONG") && !dbg.contains("abc123"));
    }

    #[test]
    fn add_and_unicode() {
        let mut red = Redactor::default();
        red.add("BRIDGE".into(), "kéy-ünïcode-1234".into());
        assert_eq!(red.redact("→kéy-ünïcode-1234←"), "→[REDACTED:BRIDGE]←");
    }

    #[test]
    fn derived_spellings() {
        let key = "{\"type\": \"service_account\", \"private_key\": \"-----BEGIN TEST KEY-----\\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\\n-----END TEST KEY-----\\n\"}";
        let red = r(&[("GCP_KEY", key)]);
        assert_eq!(
            red.redact("line MIIEvQIBADANBgkqhkiG9w0BAQEFAASC end"),
            "line [REDACTED:GCP_KEY] end"
        );
        let quoted = r(&[("Q", "pa\"ss\\word-123")]);
        assert_eq!(
            quoted.redact(r#"{"v":"pa\"ss\\word-123"}"#),
            r#"{"v":"[REDACTED:Q]"}"#
        );
    }

    #[test]
    fn base64_matches_rfc_4648() {
        let vectors = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in vectors {
            assert_eq!(base64(plain.as_bytes(), BASE64_STANDARD, true), encoded);
            assert_eq!(
                base64(plain.as_bytes(), BASE64_STANDARD, false),
                encoded.trim_end_matches('=')
            );
        }
        // Python: base64.b64encode / urlsafe_b64encode(bytes([0xfb, 0xff, 0xbf])).
        assert_eq!(base64(&[0xfb, 0xff, 0xbf], BASE64_STANDARD, true), "+/+/");
        assert_eq!(base64(&[0xfb, 0xff, 0xbf], BASE64_URL_SAFE, true), "-_-_");
    }

    #[test]
    fn base64_spellings_are_redacted() {
        // Python: base64.b64encode(b">>>???ab") == b"Pj4+Pz8/YWI=", and the URL-safe
        // alphabet swaps + and / for - and _.
        let red = r(&[("S", ">>>???ab")]);
        for form in ["Pj4+Pz8/YWI=", "Pj4+Pz8/YWI", "Pj4-Pz8_YWI=", "Pj4-Pz8_YWI"] {
            assert_eq!(
                red.redact(&format!("Authorization: Bearer {form} end")),
                "Authorization: Bearer [REDACTED:S] end",
                "{form}"
            );
        }
        // Base64 of the UTF-8 bytes.
        let red = r(&[("U", "kéy-ünïcode-1234")]);
        assert_eq!(
            red.redact("x=a8OpeS3DvG7Dr2NvZGUtMTIzNA=="),
            "x=[REDACTED:U]"
        );
    }

    /// Best effort, as documented: spellings other than the listed ones are not caught.
    #[test]
    fn other_transformations_are_not_caught() {
        let red = r(&[("S", "abc123def456")]);
        for text in ["654fed321cba", "abc123 def456", "abc123\ndef456"] {
            assert_eq!(red.redact(text), text);
        }
    }

    proptest! {
        #[test]
        fn base64_of_a_secret_never_survives(
            secret in "\\PC{1,40}",
            pre in "[ :=a-z]{0,10}",
            post in "[ ;,a-z]{0,10}",
            url in any::<bool>(),
            pad in any::<bool>(),
        ) {
            let red = r(&[("S", &secret)]);
            let alphabet = if url { BASE64_URL_SAFE } else { BASE64_STANDARD };
            let form = base64(secret.as_bytes(), alphabet, pad);
            let text = format!("{pre}{form}{post}");
            let out = red.redact(&text);
            prop_assert!(!out.contains(&form));
            prop_assert!(out.contains("[REDACTED:S]"));
        }

        #[test]
        fn secret_never_survives(secret in "[A-Za-z0-9+/=_-]{8,40}", pre in "\\PC{0,20}", post in "\\PC{0,20}") {
            let red = r(&[("S", &secret)]);
            let text = format!("{pre}{secret}{post}");
            let out = red.redact(&text);
            prop_assert!(!out.contains(&secret));
            prop_assert!(out.contains("[REDACTED:S]"));
        }

        #[test]
        fn text_without_secrets_is_unchanged(text in "[a-z ]{0,40}") {
            let red = r(&[("S", "ZZZZ-9999-ZZZZ")]);
            let out = red.redact(&text);
            prop_assert_eq!(out.as_ref(), text.as_str());
        }
    }
}
