//! Small helpers the mock needs without extra dependencies: UTC timestamps, ids, a line
//! diff, the sign test, a fake audit hash and a minimal `.tar.gz` writer.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch.
pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

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

/// RFC 3339 UTC timestamp for Unix seconds.
pub(crate) fn rfc3339(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// RFC 3339 timestamp `ago` seconds before now.
pub(crate) fn ago(ago: i64) -> String {
    rfc3339(now_secs() - ago)
}

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// A process-unique pseudo-random 64-bit value (dev mock only, not for security).
pub(crate) fn next_u64() -> u64 {
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| {
        u64::try_from(d.as_nanos() % u128::from(u64::MAX)).unwrap_or(0)
    });
    let mut x = t ^ c.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x
}

/// A UUIDv7-shaped id (time-ordered).
pub(crate) fn job_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
    let r = next_u64();
    format!(
        "{:08x}-{:04x}-7{:03x}-{:04x}-{:012x}",
        (ms >> 16) & 0xffff_ffff,
        ms & 0xffff,
        r & 0xfff,
        0x8000 | ((r >> 12) & 0x3fff),
        (r >> 26) & 0xffff_ffff_ffff
    )
}

/// A hex string of `n` pseudo-random bytes.
pub(crate) fn hex(n: usize) -> String {
    let mut s = String::with_capacity(n * 2);
    while s.len() < n * 2 {
        let _ = write!(s, "{:016x}", next_u64());
    }
    s.truncate(n * 2);
    s
}

/// A deterministic 64-hex "hash" for the fake audit chain (FNV-1a based, not SHA-256).
pub(crate) fn fake_hash(input: &str) -> String {
    let mut out = String::with_capacity(64);
    for round in 0_u64..4 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ round;
        for b in input.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        let _ = write!(out, "{h:016x}");
    }
    out
}

/// A tiny deterministic generator for fixture variety.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D).max(1))
    }

    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in `[0, 1)`.
    pub(crate) fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 53) as f64
    }

    pub(crate) fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

pub(crate) fn seed_of(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Line diff with 3 lines of context, as unified diff text. Returns
/// `(unified, lines_added, lines_removed)`.
pub(crate) fn unified_diff(old: &str, new: &str) -> (String, u32, u32) {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    // LCS table (fixture files are small).
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0_u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    // Edit script: (' ', old_idx, new_idx) / ('-', i, _) / ('+', _, j)
    let mut ops: Vec<(char, usize, usize)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push((' ', i, j));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            ops.push(('-', i, j));
            i += 1;
        } else {
            ops.push(('+', i, j));
            j += 1;
        }
    }
    let added = u32::try_from(ops.iter().filter(|o| o.0 == '+').count()).unwrap_or(u32::MAX);
    let removed = u32::try_from(ops.iter().filter(|o| o.0 == '-').count()).unwrap_or(u32::MAX);
    if added == 0 && removed == 0 {
        return (String::new(), 0, 0);
    }
    // Group changes into hunks with 3 lines of context.
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| o.0 != ' ')
        .map(|(k, _)| k)
        .collect();
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for &k in &changed {
        let start = k.saturating_sub(3);
        let end = (k + 3).min(ops.len() - 1);
        match hunks.last_mut() {
            Some(h) if start <= h.1 + 1 => h.1 = h.1.max(end),
            _ => hunks.push((start, end)),
        }
    }
    let mut out = String::new();
    for (s, e) in hunks {
        let slice = &ops[s..=e];
        let old_start = slice
            .iter()
            .find(|o| o.0 != '+')
            .map_or(slice[0].1, |o| o.1)
            + 1;
        let new_start = slice
            .iter()
            .find(|o| o.0 != '-')
            .map_or(slice[0].2, |o| o.2)
            + 1;
        let old_len = slice.iter().filter(|o| o.0 != '+').count();
        let new_len = slice.iter().filter(|o| o.0 != '-').count();
        let _ = writeln!(out, "@@ -{old_start},{old_len} +{new_start},{new_len} @@");
        for (op, oi, nj) in slice {
            match op {
                ' ' => {
                    let _ = writeln!(out, " {}", a[*oi]);
                }
                '-' => {
                    let _ = writeln!(out, "-{}", a[*oi]);
                }
                _ => {
                    let _ = writeln!(out, "+{}", b[*nj]);
                }
            }
        }
    }
    (out, added, removed)
}

/// Two-sided sign test p-value for `wins` vs `losses` (ties dropped).
pub(crate) fn sign_test_p(wins: u32, losses: u32) -> Option<f64> {
    let n = wins + losses;
    if n == 0 {
        return None;
    }
    let k = wins.min(losses);
    // P(X <= k) for X ~ Binomial(n, 0.5), doubled and capped at 1.
    let mut coeff = 1.0_f64;
    let mut cdf = 0.0_f64;
    for i in 0..=k {
        if i > 0 {
            coeff = coeff * f64::from(n - i + 1) / f64::from(i);
        }
        cdf += coeff;
    }
    let p = 2.0 * cdf / 2f64.powi(i32::try_from(n).unwrap_or(i32::MAX));
    Some(p.min(1.0))
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// A ustar archive of `(path, content)` files.
fn tar(files: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (path, content) in files {
        let mut header = [0_u8; 512];
        let name = path.as_bytes();
        let len = name.len().min(100);
        header[..len].copy_from_slice(&name[..len]);
        let write_octal = |h: &mut [u8; 512], off: usize, width: usize, v: u64| {
            let s = format!("{v:0w$o}\0", w = width - 1);
            h[off..off + width].copy_from_slice(&s.as_bytes()[..width]);
        };
        write_octal(&mut header, 100, 8, 0o644);
        write_octal(&mut header, 108, 8, 0);
        write_octal(&mut header, 116, 8, 0);
        write_octal(&mut header, 124, 12, content.len() as u64);
        write_octal(&mut header, 136, 12, 1_700_000_000);
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|b| u32::from(*b)).sum();
        let chk = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(chk.as_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(content.as_bytes());
        let pad = (512 - content.len() % 512) % 512;
        out.extend(std::iter::repeat_n(0_u8, pad));
    }
    out.extend(std::iter::repeat_n(0_u8, 1024));
    out
}

/// `files` as a gzip-compressed tar (deflate "stored" blocks: valid, not compressed).
pub(crate) fn tar_gz(files: &[(String, String)]) -> Vec<u8> {
    let raw = tar(files);
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    let chunks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (idx, chunk) in chunks.iter().enumerate() {
        out.push(u8::from(idx + 1 == chunks.len()));
        let len = u16::try_from(chunk.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&crc32(&raw).to_le_bytes());
    out.extend_from_slice(&(u32::try_from(raw.len() % (1 << 32)).unwrap_or(0)).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn diff_counts_and_hunks() {
        let (u, a, r) = unified_diff("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!((a, r), (2, 1));
        assert!(u.starts_with("@@ -1,3 +1,4 @@\n a\n"), "{u}");
        assert!(u.contains("-b\n+B\n"));
        assert_eq!(unified_diff("x\n", "x\n"), (String::new(), 0, 0));
    }

    #[test]
    fn sign_test_values() {
        assert_eq!(sign_test_p(0, 0), None);
        let p = sign_test_p(9, 1).unwrap();
        assert!((p - 0.021_484_375).abs() < 1e-9, "{p}");
        assert!((sign_test_p(3, 3).unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn crc_and_gzip_shape() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let gz = tar_gz(&[("a.txt".into(), "hello".into())]);
        assert_eq!(&gz[..3], &[0x1f, 0x8b, 8]);
        assert!(gz.len() > 1024);
    }

    #[test]
    fn ids() {
        let id = job_id();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "7");
        assert_ne!(job_id(), id);
        assert_eq!(hex(16).len(), 32);
    }
}
