//! A JSON reader that accepts exactly what CPython 3.12's `json.loads` accepts and fails with
//! the same message (`Expecting value: line 1 column 1 (char 0)`), because harness2 shows those
//! messages to the improver verbatim (`.codex/hooks.json is not valid JSON: ...`).
//!
//! Objects keep insertion order with Python `dict` semantics: a repeated key keeps its first
//! position and takes the last value. Lone surrogates from `\u` escapes become U+FFFD.

/// A decoded JSON value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PyJson {
    Null,
    Bool(bool),
    /// Numbers are kept as their source text; callers here only need their type.
    Number(String),
    Str(String),
    Array(Vec<PyJson>),
    Object(Vec<(String, PyJson)>),
}

impl PyJson {
    /// `obj.get(key)` for an object, `None` otherwise or when absent.
    pub(crate) fn get(&self, key: &str) -> Option<&PyJson> {
        match self {
            PyJson::Object(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// `json.loads(text)`; the error is the full `JSONDecodeError` string.
pub(crate) fn loads(text: &str) -> Result<PyJson, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut p = Parser { s: &chars };
    if chars.first() == Some(&'\u{feff}') {
        return Err(p.message("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0));
    }
    let start = p.skip_ws(0);
    let (value, end) = match p.scan_once(start) {
        Ok(v) => v,
        Err(Fail::Stop(idx)) => return Err(p.message("Expecting value", idx)),
        Err(Fail::Error(msg, idx)) => return Err(p.message(msg, idx)),
    };
    let end = p.skip_ws(end);
    if end != chars.len() {
        return Err(p.message("Extra data", end));
    }
    Ok(value)
}

enum Fail {
    /// `StopIteration(idx)`: no value starts here (reported as "Expecting value").
    Stop(usize),
    Error(&'static str, usize),
}

struct Parser<'a> {
    s: &'a [char],
}

impl Parser<'_> {
    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn skip_ws(&self, mut i: usize) -> usize {
        while matches!(self.at(i), Some(' ' | '\t' | '\n' | '\r')) {
            i += 1;
        }
        i
    }

    fn message(&self, msg: &str, pos: usize) -> String {
        let before = self.s.get(..pos).unwrap_or(self.s);
        let lineno = before.iter().filter(|&&c| c == '\n').count() + 1;
        let colno = match before.iter().rposition(|&c| c == '\n') {
            Some(nl) => pos - nl,
            None => pos + 1,
        };
        format!("{msg}: line {lineno} column {colno} (char {pos})")
    }

    fn literal(&self, idx: usize, word: &str) -> bool {
        word.chars()
            .enumerate()
            .all(|(k, c)| self.at(idx + k) == Some(c))
    }

    fn scan_once(&mut self, idx: usize) -> Result<(PyJson, usize), Fail> {
        let len = self.s.len();
        let Some(c) = self.at(idx) else {
            return Err(Fail::Stop(idx));
        };
        match c {
            '"' => {
                let (s, end) = self.scan_string(idx + 1)?;
                return Ok((PyJson::Str(s), end));
            }
            '{' => return self.parse_object(idx + 1),
            '[' => return self.parse_array(idx + 1),
            'n' if idx + 3 < len && self.literal(idx, "null") => {
                return Ok((PyJson::Null, idx + 4));
            }
            't' if idx + 3 < len && self.literal(idx, "true") => {
                return Ok((PyJson::Bool(true), idx + 4));
            }
            'f' if idx + 4 < len && self.literal(idx, "false") => {
                return Ok((PyJson::Bool(false), idx + 5));
            }
            'N' if idx + 2 < len && self.literal(idx, "NaN") => {
                return Ok((PyJson::Number("NaN".into()), idx + 3));
            }
            'I' if idx + 7 < len && self.literal(idx, "Infinity") => {
                return Ok((PyJson::Number("Infinity".into()), idx + 8));
            }
            '-' if idx + 8 < len && self.literal(idx, "-Infinity") => {
                return Ok((PyJson::Number("-Infinity".into()), idx + 9));
            }
            _ => {}
        }
        self.match_number(idx)
    }

    fn is_digit(&self, i: usize) -> bool {
        self.at(i).is_some_and(|c| c.is_ascii_digit())
    }

    fn match_number(&self, start: usize) -> Result<(PyJson, usize), Fail> {
        let len = self.s.len();
        let end_idx = len.wrapping_sub(1);
        let mut idx = start;
        if self.at(idx) == Some('-') {
            idx += 1;
            if idx > end_idx || len == 0 {
                return Err(Fail::Stop(start));
            }
        }
        match self.at(idx) {
            Some('1'..='9') => {
                idx += 1;
                while self.is_digit(idx) {
                    idx += 1;
                }
            }
            Some('0') => idx += 1,
            _ => return Err(Fail::Stop(start)),
        }
        if idx < end_idx && self.at(idx) == Some('.') && self.is_digit(idx + 1) {
            idx += 2;
            while self.is_digit(idx) {
                idx += 1;
            }
        }
        if idx < end_idx && matches!(self.at(idx), Some('e' | 'E')) {
            let e_start = idx;
            idx += 1;
            if idx < end_idx && matches!(self.at(idx), Some('-' | '+')) {
                idx += 1;
            }
            while self.is_digit(idx) {
                idx += 1;
            }
            if !self.is_digit(idx - 1) {
                idx = e_start;
            }
        }
        let text: String = self.s.get(start..idx).unwrap_or_default().iter().collect();
        Ok((PyJson::Number(text), idx))
    }

    fn hex4(&self, from: usize) -> Option<u32> {
        (from..from + 4).try_fold(0_u32, |acc, i| {
            self.at(i)
                .and_then(|c| c.to_digit(16))
                .map(|d| (acc << 4) | d)
        })
    }

    /// `scanstring`: `begin` is the index right after the opening quote.
    fn scan_string(&self, begin: usize) -> Result<(String, usize), Fail> {
        let len = self.s.len();
        let mut out = String::new();
        let mut next = begin;
        loop {
            // Plain characters up to a quote or backslash.
            let c = loop {
                let Some(c) = self.at(next) else {
                    return Err(Fail::Error("Unterminated string starting at", begin - 1));
                };
                if c == '"' || c == '\\' {
                    break c;
                }
                if u32::from(c) <= 0x1f {
                    return Err(Fail::Error("Invalid control character at", next));
                }
                out.push(c);
                next += 1;
            };
            next += 1;
            if c == '"' {
                return Ok((out, next));
            }
            if next == len {
                return Err(Fail::Error("Unterminated string starting at", begin - 1));
            }
            let esc = self.at(next).unwrap_or('\0');
            if esc != 'u' {
                let decoded = match esc {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    _ => return Err(Fail::Error("Invalid \\escape", next - 1)),
                };
                out.push(decoded);
                next += 1;
                continue;
            }
            next += 1;
            let mut end = next + 4;
            if end >= len {
                return Err(Fail::Error("Invalid \\uXXXX escape", next - 1));
            }
            let Some(mut code) = self.hex4(next) else {
                return Err(Fail::Error("Invalid \\uXXXX escape", end - 5));
            };
            next = end;
            if (0xd800..=0xdbff).contains(&code)
                && end + 6 < len
                && self.at(next) == Some('\\')
                && self.at(next + 1) == Some('u')
            {
                end += 6;
                let Some(low) = self.hex4(next + 2) else {
                    return Err(Fail::Error("Invalid \\uXXXX escape", end - 5));
                };
                if (0xdc00..=0xdfff).contains(&low) {
                    code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                    next = end;
                } else {
                    end -= 6;
                    next = end;
                }
            }
            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
        }
    }

    fn parse_object(&mut self, start: usize) -> Result<(PyJson, usize), Fail> {
        let mut items: Vec<(String, PyJson)> = Vec::new();
        let mut idx = self.skip_ws(start);
        if self.at(idx) != Some('}') {
            loop {
                if self.at(idx) != Some('"') {
                    return Err(Fail::Error(
                        "Expecting property name enclosed in double quotes",
                        idx,
                    ));
                }
                let (key, after) = self.scan_string(idx + 1)?;
                idx = self.skip_ws(after);
                if self.at(idx) != Some(':') {
                    return Err(Fail::Error("Expecting ':' delimiter", idx));
                }
                idx = self.skip_ws(idx + 1);
                let (value, after) = self.scan_once(idx)?;
                match items.iter_mut().find(|(k, _)| *k == key) {
                    Some(slot) => slot.1 = value,
                    None => items.push((key, value)),
                }
                idx = self.skip_ws(after);
                if self.at(idx) == Some('}') {
                    break;
                }
                if self.at(idx) != Some(',') {
                    return Err(Fail::Error("Expecting ',' delimiter", idx));
                }
                idx = self.skip_ws(idx + 1);
            }
        }
        Ok((PyJson::Object(items), idx + 1))
    }

    fn parse_array(&mut self, start: usize) -> Result<(PyJson, usize), Fail> {
        let mut items = Vec::new();
        let mut idx = self.skip_ws(start);
        if self.at(idx) != Some(']') {
            loop {
                let (value, after) = self.scan_once(idx)?;
                items.push(value);
                idx = self.skip_ws(after);
                if self.at(idx) == Some(']') {
                    break;
                }
                if self.at(idx) != Some(',') {
                    return Err(Fail::Error("Expecting ',' delimiter", idx));
                }
                idx = self.skip_ws(idx + 1);
            }
        }
        Ok((PyJson::Array(items), idx + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_what_python_accepts() {
        let v =
            loads(r#" {"a": [1, -2.5e3, true, false, null, NaN, -Infinity], "a": "xé"} "#).unwrap();
        assert_eq!(v.get("a"), Some(&PyJson::Str("xé".into())));
        assert!(matches!(loads("[]"), Ok(PyJson::Array(_))));
        assert_eq!(loads(r#""😀""#), Ok(PyJson::Str("😀".into())));
    }

    #[test]
    fn errors_match_python_wording() {
        // Expected strings produced by CPython 3.12 json.loads (see tests/fixtures/golden).
        assert_eq!(
            loads("").unwrap_err(),
            "Expecting value: line 1 column 1 (char 0)"
        );
        assert_eq!(
            loads("{\"a\": 1,}").unwrap_err(),
            "Expecting property name enclosed in double quotes: line 1 column 9 (char 8)"
        );
        assert_eq!(
            loads("[1 2]").unwrap_err(),
            "Expecting ',' delimiter: line 1 column 4 (char 3)"
        );
        assert_eq!(
            loads("{\"a\" 1}").unwrap_err(),
            "Expecting ':' delimiter: line 1 column 6 (char 5)"
        );
        assert_eq!(
            loads("{}\n{}").unwrap_err(),
            "Extra data: line 2 column 1 (char 3)"
        );
        assert_eq!(
            loads("\"abc").unwrap_err(),
            "Unterminated string starting at: line 1 column 1 (char 0)"
        );
    }
}
