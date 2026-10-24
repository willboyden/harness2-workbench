//! Python semantics the ports must reproduce byte for byte.
//!
//! harness2 is Python 3.12; its reports and validation messages depend on Python's float
//! rounding, `sum()` (Neumaier-compensated since 3.12), `statistics.mean` (exact rational
//! arithmetic), `str.isspace()`/`str.strip()`, `re` `\s` classes, and `repr(str)`. Each helper
//! here mirrors one of those and is checked against values produced by the real interpreter
//! (`tests/fixtures/golden/`).

use std::cmp::Ordering;

/// A float that holds a non-negative whole number that fits a `u32`, as that number.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn exact_u32(f: f64) -> Option<u32> {
    // The range and integrality checks make the cast exact.
    (f.is_finite() && f >= 0.0 && f <= f64::from(u32::MAX) && f.fract() == 0.0).then_some(f as u32)
}

/// `str.isspace()` for one character (also what `re`'s `\s` matches in a `str` pattern).
/// Unicode `White_Space` plus the ASCII separators U+001C..U+001F, which Python counts.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()` with no argument.
pub(crate) fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `s[:n]` on a Python `str`: the first `n` code points.
pub(crate) fn first_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => s.get(..i).unwrap_or(s),
        None => s,
    }
}

/// `len(s)` on a Python `str` (code points), saturating at `u32::MAX`.
pub(crate) fn char_len_u32(s: &str) -> u32 {
    u32::try_from(s.chars().count()).unwrap_or(u32::MAX)
}

/// Start offsets (bytes) of every line in `s`, as `re.MULTILINE` `^` sees them: 0 and every
/// position right after a `\n`.
fn line_starts(s: &str) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0).chain(s.match_indices('\n').map(|(i, _)| i + 1))
}

/// Byte offset of the first non-space character at or after `from` (or `s.len()`).
fn skip_spaces(s: &str, from: usize) -> usize {
    s.get(from..)
        .and_then(|rest| rest.char_indices().find(|&(_, c)| !is_space(c)))
        .map_or(s.len(), |(i, _)| from + i)
}

/// Byte offset of the first space character at or after `from` (or `s.len()`).
fn skip_non_spaces(s: &str, from: usize) -> usize {
    s.get(from..)
        .and_then(|rest| rest.char_indices().find(|&(_, c)| is_space(c)))
        .map_or(s.len(), |(i, _)| from + i)
}

/// `\s*$` (MULTILINE) after a token ending at `from`: the following run of spaces reaches the
/// end of the text or contains a newline (where `$` can match).
fn tail_is_line_end(s: &str, from: usize) -> bool {
    let end = skip_spaces(s, from);
    end == s.len() || s.get(from..end).is_some_and(|run| run.contains('\n'))
}

/// `re.search(r"^<key>:\s*\S", fm, re.M)`: some line starts with `<key>:` followed (possibly
/// across newlines) by a non-space character.
pub(crate) fn has_field(fm: &str, key: &str) -> bool {
    line_starts(fm).any(|p| {
        fm.get(p..).is_some_and(|rest| {
            rest.starts_with(key)
                && rest.get(key.len()..).is_some_and(|r| r.starts_with(':'))
                && skip_spaces(fm, p + key.len() + 1) < fm.len()
        })
    })
}

/// `re.search(r"^name:\s*(\S+)\s*$", fm, re.M).group(1)`, with Python's backtracking.
pub(crate) fn name_field(fm: &str) -> Option<&str> {
    line_starts(fm).find_map(|p| {
        let rest = fm.get(p..)?;
        if !rest.starts_with("name:") {
            return None;
        }
        let start = skip_spaces(fm, p + 5);
        if start >= fm.len() {
            return None;
        }
        let end = skip_non_spaces(fm, start);
        if tail_is_line_end(fm, end) {
            fm.get(start..end)
        } else {
            None
        }
    })
}

/// `re.search(r"^mode:\s*subagent\s*$", fm, re.M)`.
pub(crate) fn has_mode_subagent(fm: &str) -> bool {
    line_starts(fm).any(|p| {
        fm.get(p..).is_some_and(|rest| rest.starts_with("mode:")) && {
            let start = skip_spaces(fm, p + 5);
            fm.get(start..).is_some_and(|r| r.starts_with("subagent"))
                && tail_is_line_end(fm, start + "subagent".len())
        }
    })
}

/// `repr(s)` for a Python `str`. Exact for ASCII; non-ASCII characters are treated as
/// printable except C1 controls, line/paragraph separators and a few format characters
/// (Python consults the full Unicode database, which this crate does not carry).
pub(crate) fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if is_printable(c) => out.push(c),
            c => {
                let v = u32::from(c);
                if v < 0x100 {
                    out.push_str(&format!("\\x{v:02x}"));
                } else if v < 0x10000 {
                    out.push_str(&format!("\\u{v:04x}"));
                } else {
                    out.push_str(&format!("\\U{v:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

fn is_printable(c: char) -> bool {
    let v = u32::from(c);
    if v < 0x20 || v == 0x7f || (0x80..0xa0).contains(&v) {
        return false;
    }
    if v < 0x7f {
        return true;
    }
    // Separators other than the ASCII space, and common format characters.
    !(c.is_whitespace()
        || matches!(v, 0xad | 0x200b..=0x200f | 0x2028..=0x202e | 0x2060..=0x2064 | 0xfeff)
        || (0xd800..=0xdfff).contains(&v)
        || (0xe000..=0xf8ff).contains(&v))
}

/// `round(x, ndigits)` for a float: correctly rounded, ties to even on the exact binary value
/// (Rust's fixed-precision formatting rounds the same way; checked against Python).
pub(crate) fn round(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.ndigits$}").parse::<f64>().unwrap_or(x)
}

/// `sum(xs)` over floats in CPython 3.12: start from the int 0, then Neumaier-compensated
/// addition, adding the compensation once at the end when it is finite and nonzero.
pub(crate) fn sum(xs: impl IntoIterator<Item = f64>) -> f64 {
    let mut acc = PySum::default();
    for x in xs {
        acc.add(x);
    }
    acc.value()
}

/// [`sum`] fed one value at a time, for values read while a file is parsed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct PySum {
    total: f64,
    comp: f64,
}

impl PySum {
    pub(crate) fn add(&mut self, x: f64) {
        let t = self.total + x;
        if self.total.abs() >= x.abs() {
            self.comp += (self.total - t) + x;
        } else {
            self.comp += (x - t) + self.total;
        }
        self.total = t;
    }

    pub(crate) fn value(self) -> f64 {
        if self.comp != 0.0 && self.comp.is_finite() {
            self.total + self.comp
        } else {
            self.total
        }
    }
}

/// `statistics.mean(xs)` over floats: the exact rational mean, correctly rounded to `f64`.
/// `None` for an empty input. Non-finite inputs fall back to plain arithmetic.
pub(crate) fn mean(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let n = u64::try_from(xs.len()).ok()?;
    if xs.iter().any(|x| !x.is_finite()) {
        return Some(xs.iter().sum::<f64>() / xs.len() as f64);
    }
    Some(exact_mean(xs, n).unwrap_or_else(|| xs.iter().sum::<f64>() / xs.len() as f64))
}

/// A finite float as `sign, mantissa, exponent` with value `mantissa * 2^exponent`.
fn decompose(x: f64) -> (bool, u64, i32) {
    let bits = x.to_bits();
    let negative = bits >> 63 == 1;
    let exp_bits = i32::try_from((bits >> 52) & 0x7ff).unwrap_or(0);
    let frac = bits & ((1_u64 << 52) - 1);
    if exp_bits == 0 {
        (negative, frac, -1074)
    } else {
        (negative, frac | (1_u64 << 52), exp_bits - 1075)
    }
}

fn exact_mean(xs: &[f64], n: u64) -> Option<f64> {
    let parts: Vec<(bool, u64, i32)> = xs
        .iter()
        .map(|&x| decompose(x))
        .filter(|&(_, m, _)| m != 0)
        .collect();
    let Some(emin) = parts.iter().map(|&(_, _, e)| e).min() else {
        return Some(0.0);
    };
    let mut pos = BigNat::default();
    let mut neg = BigNat::default();
    for &(negative, m, e) in &parts {
        let shift = u32::try_from(e - emin).ok()?;
        if negative {
            neg.add_shifted(m, shift);
        } else {
            pos.add_shifted(m, shift);
        }
    }
    let (negative, total) = match pos.cmp(&neg) {
        Ordering::Equal => return Some(0.0),
        Ordering::Greater => (false, pos.sub(&neg)),
        Ordering::Less => (true, neg.sub(&pos)),
    };
    // value = total * 2^emin / n. Scale so the quotient has at least 56 significant bits.
    let n_bits = 64 - n.leading_zeros();
    let t = (56 + n_bits).saturating_sub(total.bit_len()) + 1;
    let (q, rem) = total.shl(t).div_small(n);
    let len = q.bit_len();
    if len < 54 {
        return None;
    }
    let mut shift = len - 53;
    let mut mant = q.extract(shift, 53);
    let low = q.low_bits(shift);
    let half = 1_u64 << (shift - 1).min(63);
    let round_up = if shift > 64 {
        // Not reachable with the scaling above (shift <= 3 + bits of n); stay total.
        q.bit(shift - 1) && (q.any_below(shift - 1) || rem != 0 || mant & 1 == 1)
    } else {
        low > half || (low == half && (rem != 0 || mant & 1 == 1))
    };
    if round_up {
        mant += 1;
        if mant == 1_u64 << 53 {
            mant >>= 1;
            shift += 1;
        }
    }
    let exp2 = i64::from(emin) - i64::from(t) + i64::from(shift);
    let biased = exp2 + 52 + 1023;
    if !(1..=2046).contains(&biased) {
        return None;
    }
    let biased = u64::try_from(biased).ok()?;
    let bits = (biased << 52) | (mant & ((1_u64 << 52) - 1)) | (u64::from(negative) << 63);
    Some(f64::from_bits(bits))
}

/// Minimal arbitrary-precision natural number (little-endian 64-bit limbs) for
/// [`exact_mean`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BigNat(Vec<u64>);

impl BigNat {
    fn trim(&mut self) {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }

    fn bit_len(&self) -> u32 {
        match self.0.last() {
            None => 0,
            Some(&top) => {
                let limbs = u32::try_from(self.0.len()).unwrap_or(u32::MAX);
                (limbs - 1).saturating_mul(64) + (64 - top.leading_zeros())
            }
        }
    }

    fn add_shifted(&mut self, m: u64, shift: u32) {
        let limb = usize::try_from(shift / 64).unwrap_or(usize::MAX);
        let bit = shift % 64;
        let wide = u128::from(m) << bit;
        let mut carry: u128 = 0;
        let mut i = limb;
        let parts = [wide & u128::from(u64::MAX), wide >> 64];
        let mut k = 0;
        while k < 2 || carry != 0 {
            if self.0.len() <= i {
                self.0.resize(i + 1, 0);
            }
            let add = parts.get(k).copied().unwrap_or(0);
            let cur = u128::from(self.0[i]) + add + carry;
            self.0[i] = u64::try_from(cur & u128::from(u64::MAX)).unwrap_or(0);
            carry = cur >> 64;
            i += 1;
            k += 1;
        }
        self.trim();
    }

    /// `self - other`, requires `self >= other`.
    fn sub(&self, other: &BigNat) -> BigNat {
        let mut out = Vec::with_capacity(self.0.len());
        let mut borrow = false;
        for (i, &a) in self.0.iter().enumerate() {
            let b = other.0.get(i).copied().unwrap_or(0);
            let (d1, o1) = a.overflowing_sub(b);
            let (d2, o2) = d1.overflowing_sub(u64::from(borrow));
            out.push(d2);
            borrow = o1 || o2;
        }
        let mut r = BigNat(out);
        r.trim();
        r
    }

    fn shl(&self, t: u32) -> BigNat {
        let limbs = usize::try_from(t / 64).unwrap_or(0);
        let bit = t % 64;
        let mut out = vec![0_u64; limbs];
        let mut carry = 0_u64;
        for &a in &self.0 {
            if bit == 0 {
                out.push(a);
            } else {
                out.push((a << bit) | carry);
                carry = a >> (64 - bit);
            }
        }
        if carry != 0 {
            out.push(carry);
        }
        let mut r = BigNat(out);
        r.trim();
        r
    }

    /// `(self / d, self % d)` for a nonzero single-limb divisor.
    fn div_small(&self, d: u64) -> (BigNat, u64) {
        let d = u128::from(d.max(1));
        let mut out = vec![0_u64; self.0.len()];
        let mut rem: u128 = 0;
        for (i, &a) in self.0.iter().enumerate().rev() {
            let cur = (rem << 64) | u128::from(a);
            out[i] = u64::try_from(cur / d).unwrap_or(u64::MAX);
            rem = cur % d;
        }
        let mut q = BigNat(out);
        q.trim();
        (q, u64::try_from(rem).unwrap_or(u64::MAX))
    }

    fn bit(&self, i: u32) -> bool {
        let limb = usize::try_from(i / 64).unwrap_or(usize::MAX);
        self.0.get(limb).is_some_and(|&w| (w >> (i % 64)) & 1 == 1)
    }

    fn any_below(&self, i: u32) -> bool {
        (0..i).any(|j| self.bit(j))
    }

    /// Bits `[from, from + count)` as a `u64` (`count <= 64`).
    fn extract(&self, from: u32, count: u32) -> u64 {
        (0..count.min(64)).fold(0_u64, |acc, j| acc | (u64::from(self.bit(from + j)) << j))
    }

    /// The lowest `count` bits (`count <= 64`; larger counts are only used by the
    /// unreachable branch above and saturate).
    fn low_bits(&self, count: u32) -> u64 {
        self.extract(0, count.min(64))
    }
}

impl PartialOrd for BigNat {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BigNat {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }
}

/// Python's sort order for paths from `sorted(Path.rglob(...))`: component-wise comparison
/// (so `a/b` sorts before `a-b/x`, unlike a plain string sort).
pub(crate) fn cmp_paths(a: &str, b: &str) -> Ordering {
    a.split('/').cmp(b.split('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isspace_matches_python_extras() {
        for c in [
            '\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}', ' ', '\t', '\u{85}', '\u{a0}',
        ] {
            assert!(is_space(c), "{c:?}");
        }
        for c in ['\u{200b}', 'a', '\u{0}'] {
            assert!(!is_space(c), "{c:?}");
        }
        assert_eq!(strip("\u{1f} x \n"), "x");
    }

    #[test]
    fn frontmatter_matchers_follow_python_backtracking() {
        assert!(has_field("---\ndescription: x\n", "description"));
        assert!(has_field("description:\n\n  later", "description"));
        assert!(!has_field("description:   \n  ", "description"));
        assert!(!has_field("xdescription: y", "description"));
        assert!(!has_field("description y", "description"));
        assert_eq!(name_field("---\nname: foo\n"), Some("foo"));
        assert_eq!(name_field("name: foo"), Some("foo"));
        assert_eq!(name_field("name:\nfoo\n"), Some("foo"));
        assert_eq!(name_field("name: foo bar\nname: baz\n"), Some("baz"));
        assert_eq!(name_field("name: foo bar"), None);
        assert_eq!(name_field("name:   "), None);
        assert!(has_mode_subagent("mode: subagent\n"));
        assert!(has_mode_subagent("x\nmode:subagent"));
        assert!(!has_mode_subagent("mode: subagents\n"));
        assert!(!has_mode_subagent("mode: subagent x\n"));
    }

    #[test]
    fn repr_quotes_like_python() {
        assert_eq!(repr_str("PostToolUse"), "'PostToolUse'");
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(repr_str("tab\there\n"), "'tab\\there\\n'");
        assert_eq!(repr_str("\u{1}é"), "'\\x01é'");
    }

    #[test]
    fn neumaier_sum_matches_python_312() {
        assert_eq!(sum([0.1; 10]), 1.0);
        assert_eq!(sum(Vec::<f64>::new()), 0.0);
        assert_eq!(sum([1e100, 1.0, -1e100]), 1.0);
    }

    #[test]
    fn exact_mean_is_correctly_rounded() {
        assert_eq!(mean(&[0.1, 0.2, 0.3]), Some(0.2));
        assert_eq!(mean(&[]), None);
        assert_eq!(mean(&[0.0, -0.0]), Some(0.0));
        assert_eq!(mean(&[1.0, 2.0]), Some(1.5));
        assert_eq!(mean(&[1.0 / 3.0, 2.0 / 3.0, 1.0]), Some(2.0 / 3.0));
        assert_eq!(mean(&[-1.0, -2.0]), Some(-1.5));
        assert_eq!(mean(&[1e-300, 1e300]), Some(5e299));
    }

    #[test]
    fn rounding_ties_to_even_on_binary_value() {
        assert_eq!(round(0.125, 2), 0.12);
        assert_eq!(round(0.375, 2), 0.38);
        assert_eq!(round(2.675, 2), 2.67);
        assert_eq!(round(66.665, 2), 66.67);
    }

    #[test]
    fn path_order_is_componentwise() {
        let mut v = vec!["a-b/x", "a/b", "a/a", "a.b"];
        v.sort_by(|x, y| cmp_paths(x, y));
        assert_eq!(v, vec!["a/a", "a/b", "a-b/x", "a.b"]);
    }
}
