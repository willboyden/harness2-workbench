//! Display formatting shared by every page: money, percentages, counts, sizes, durations and
//! timestamps. Pure functions; the clock is passed in, never read here.

/// Placeholder shown for a value that is unknown or not a number.
pub(crate) const DASH: &str = "—";

/// Clamp a float into `[0, 1e15]` and round it to the nearest whole number.
fn to_u64(v: f64) -> u64 {
    if !v.is_finite() || v <= 0.0 {
        return 0;
    }
    let clamped = v.round().min(1e15);
    // Clamped to [0, 1e15] above, so the conversion is exact and cannot wrap.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = clamped as u64;
    n
}

/// `1234567` -> `1,234,567`. Works on the digits of any non-negative integer string.
fn group_digits(digits: &str) -> String {
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A whole number with thousands separators: `24000` -> `24,000`.
pub(crate) fn int(n: u64) -> String {
    group_digits(&n.to_string())
}

/// US dollars as harness2 records them: `$1,234.50`; tiny non-zero amounts read `<$0.01`.
pub(crate) fn usd(v: f64) -> String {
    if !v.is_finite() {
        return DASH.to_owned();
    }
    let sign = if v < 0.0 { "-" } else { "" };
    let a = v.abs();
    if a > 0.0 && a < 0.005 {
        return format!("{sign}<$0.01");
    }
    let s = format!("{a:.2}");
    let (whole, cents) = s.split_once('.').unwrap_or((s.as_str(), "00"));
    format!("{sign}${}.{cents}", group_digits(whole))
}

/// An optional USD range from an estimate: `$1.20 – $3.40`, a single figure when equal.
pub(crate) fn usd_range(low: Option<f64>, high: Option<f64>) -> Option<String> {
    match (low, high) {
        (Some(l), Some(h)) if (l - h).abs() < 0.005 => Some(usd(l)),
        (Some(l), Some(h)) => Some(format!("{} – {}", usd(l), usd(h))),
        (Some(v), None) | (None, Some(v)) => Some(usd(v)),
        (None, None) => None,
    }
}

/// A percentage that is already on the 0..100 scale: `62.5%`.
pub(crate) fn pct(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.1}%")
    } else {
        DASH.to_owned()
    }
}

/// An optional percentage (0..100); `—` when absent.
pub(crate) fn opt_pct(v: Option<f64>) -> String {
    v.map_or_else(|| DASH.to_owned(), pct)
}

/// A fraction on the 0..1 scale shown as a percentage: `0.625` -> `62.5%`.
pub(crate) fn frac_pct(v: f64) -> String {
    pct(v * 100.0)
}

/// A signed difference in percentage points: `+4.2 pts`, `−3.1 pts`, `±0.0 pts`.
pub(crate) fn signed_points(points: f64) -> String {
    if !points.is_finite() {
        return DASH.to_owned();
    }
    let rounded = (points * 10.0).round() / 10.0;
    if rounded == 0.0 {
        "±0.0 pts".to_owned()
    } else if rounded > 0.0 {
        format!("+{rounded:.1} pts")
    } else {
        format!("\u{2212}{:.1} pts", rounded.abs())
    }
}

/// A byte count in binary units: `512 B`, `1.5 KiB`, `12.0 MiB`.
pub(crate) fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.0;
    let mut unit = UNITS[0];
    for next in &UNITS[1..] {
        if v < 1024.0 {
            break;
        }
        v /= 1024.0;
        unit = next;
    }
    format!("{v:.1} {unit}")
}

/// A wall-clock duration in seconds: `850 ms`, `12.4 s`, `2m 05s`, `1h 02m`.
pub(crate) fn duration_secs(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return DASH.to_owned();
    }
    if secs < 1.0 {
        return format!("{} ms", to_u64(secs * 1000.0));
    }
    if secs < 60.0 {
        return format!("{secs:.1} s");
    }
    let total = to_u64(secs);
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h == 0 {
        format!("{m}m {s:02}s")
    } else if h < 48 {
        format!("{h}h {m:02}m")
    } else {
        format!("{}d {:02}h", h / 24, h % 24)
    }
}

/// `n` with the right noun: `1 task`, `3 tasks`.
pub(crate) fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", int(n))
    }
}

/// A sign-test p-value: `p = 0.031`, `p < 0.001`.
pub(crate) fn p_value(p: f64) -> String {
    if !p.is_finite() {
        DASH.to_owned()
    } else if p < 0.001 {
        "p < 0.001".to_owned()
    } else {
        format!("p = {p:.3}")
    }
}

/// The first 10 characters of a git commit (or the whole id when shorter).
pub(crate) fn short_commit(commit: &str) -> String {
    commit.chars().take(10).collect()
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date for a day count since 1970-01-01: `(year, month, day)`.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn digits(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Parse an RFC 3339 timestamp (`2026-10-23T12:00:00Z`, fractional seconds and numeric
/// offsets allowed) into Unix seconds. `None` for anything else.
pub(crate) fn parse_rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once(['T', 't', ' '])?;
    let mut dp = date.splitn(3, '-');
    let (y, mo, d) = (
        digits(dp.next()?)?,
        digits(dp.next()?)?,
        digits(dp.next()?)?,
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    // Split the offset off the time: `Z`, `+hh:mm` or `-hh:mm`.
    let (time, offset_secs) = if let Some(t) = rest.strip_suffix(['Z', 'z']) {
        (t, 0)
    } else {
        let idx = rest.rfind(['+', '-'])?;
        let (t, off) = rest.split_at(idx);
        let sign = if off.starts_with('-') { -1 } else { 1 };
        let (oh, om) = off[1..].split_once(':')?;
        (t, sign * (digits(oh)? * 3600 + digits(om)? * 60))
    };
    let time = time.split('.').next()?;
    let mut tp = time.splitn(3, ':');
    let (h, mi, sec) = (
        digits(tp.next()?)?,
        digits(tp.next()?)?,
        digits(tp.next()?)?,
    );
    if h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset_secs)
}

/// Unix seconds as `2026-10-23 12:00 UTC`.
pub(crate) fn utc_minutes(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// Unix seconds as `2026-10-23`.
pub(crate) fn utc_date(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// An RFC 3339 timestamp as `2026-10-23 12:00 UTC`; the raw string when it does not parse.
pub(crate) fn timestamp(s: &str) -> String {
    parse_rfc3339(s).map_or_else(|| s.to_owned(), utc_minutes)
}

/// How long ago `then` was, relative to `now` (both Unix seconds): `just now`, `5 min ago`,
/// `3 h ago`, `yesterday`, `4 days ago`, else the date.
pub(crate) fn relative(now: i64, then: i64) -> String {
    let diff = now - then;
    if diff < 45 {
        "just now".to_owned()
    } else if diff < 90 {
        "1 min ago".to_owned()
    } else if diff < 3600 {
        format!("{} min ago", (diff + 30) / 60)
    } else if diff < 86_400 {
        format!("{} h ago", diff / 3600)
    } else if diff < 2 * 86_400 {
        "yesterday".to_owned()
    } else if diff < 7 * 86_400 {
        format!("{} days ago", diff / 86_400)
    } else {
        utc_date(then)
    }
}

/// `relative()` for an RFC 3339 string; the raw string when it does not parse.
pub(crate) fn relative_ts(now: i64, ts: &str) -> String {
    parse_rfc3339(ts).map_or_else(|| ts.to_owned(), |t| relative(now, t))
}

/// Seconds between two RFC 3339 timestamps, when both parse and are ordered.
pub(crate) fn seconds_between(start: &str, end: &str) -> Option<f64> {
    let (a, b) = (parse_rfc3339(start)?, parse_rfc3339(end)?);
    (b >= a).then(|| (b - a) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money() {
        assert_eq!(usd(0.0), "$0.00");
        assert_eq!(usd(0.004), "<$0.01");
        assert_eq!(usd(1.5), "$1.50");
        assert_eq!(usd(1234.5), "$1,234.50");
        assert_eq!(usd(-2.25), "-$2.25");
        assert_eq!(usd(f64::NAN), DASH);
        assert_eq!(usd_range(Some(1.2), Some(3.4)).unwrap(), "$1.20 – $3.40");
        assert_eq!(usd_range(Some(2.0), Some(2.0)).unwrap(), "$2.00");
        assert_eq!(usd_range(None, None), None);
    }

    #[test]
    fn numbers_and_sizes() {
        assert_eq!(int(0), "0");
        assert_eq!(int(999), "999");
        assert_eq!(int(24_000), "24,000");
        assert_eq!(int(1_234_567), "1,234,567");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(12 * 1024 * 1024), "12.0 MiB");
        assert_eq!(plural(1, "task", "tasks"), "1 task");
        assert_eq!(plural(1200, "task", "tasks"), "1,200 tasks");
    }

    #[test]
    fn percentages() {
        assert_eq!(pct(62.5), "62.5%");
        assert_eq!(frac_pct(0.625), "62.5%");
        assert_eq!(opt_pct(None), DASH);
        assert_eq!(signed_points(4.24), "+4.2 pts");
        assert_eq!(signed_points(-3.06), "\u{2212}3.1 pts");
        assert_eq!(signed_points(0.01), "±0.0 pts");
        assert_eq!(p_value(0.0312), "p = 0.031");
        assert_eq!(p_value(0.0001), "p < 0.001");
    }

    #[test]
    fn durations() {
        assert_eq!(duration_secs(0.85), "850 ms");
        assert_eq!(duration_secs(12.44), "12.4 s");
        assert_eq!(duration_secs(125.0), "2m 05s");
        assert_eq!(duration_secs(119.6), "2m 00s");
        assert_eq!(duration_secs(3720.0), "1h 02m");
        assert_eq!(duration_secs(-1.0), DASH);
    }

    #[test]
    fn rfc3339_round_trip() {
        let t = parse_rfc3339("2026-10-23T12:34:56Z").unwrap();
        assert_eq!(utc_minutes(t), "2026-10-23 12:34 UTC");
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339("2026-10-23T14:34:56.123+02:00"),
            parse_rfc3339("2026-10-23T12:34:56Z")
        );
        assert_eq!(
            parse_rfc3339("2026-10-23T10:00:00-02:30"),
            parse_rfc3339("2026-10-23T12:30:00Z")
        );
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(parse_rfc3339("not a date"), None);
        assert_eq!(parse_rfc3339("2026-13-01T00:00:00Z"), None);
        assert_eq!(timestamp("garbage"), "garbage");
        assert_eq!(utc_date(1_709_164_800), "2024-02-29");
    }

    #[test]
    fn relative_times() {
        let now = 1_000_000;
        assert_eq!(relative(now, now - 10), "just now");
        assert_eq!(relative(now, now - 60), "1 min ago");
        assert_eq!(relative(now, now - 600), "10 min ago");
        assert_eq!(relative(now, now - 7200), "2 h ago");
        assert_eq!(relative(now, now - 90_000), "yesterday");
        assert_eq!(relative(now, now - 3 * 86_400), "3 days ago");
        assert_eq!(
            relative(now, now - 30 * 86_400),
            utc_date(now - 30 * 86_400)
        );
        assert_eq!(
            seconds_between("2026-10-23T12:00:00Z", "2026-10-23T12:01:30Z"),
            Some(90.0)
        );
        assert_eq!(
            seconds_between("2026-10-23T12:01:30Z", "2026-10-23T12:00:00Z"),
            None
        );
    }

    #[test]
    fn commits() {
        assert_eq!(
            short_commit("1cfe2540000000000000000000000000000000ab"),
            "1cfe254000"
        );
        assert_eq!(short_commit("abc"), "abc");
    }
}
