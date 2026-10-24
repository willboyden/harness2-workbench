//! Per-client token buckets (login attempts, failed authentication, audit records of
//! failures).
//!
//! IPv6 clients are bucketed per /64, because one host usually controls a whole /64. The
//! table is bounded and never fails open: when it is full, buckets that have refilled
//! completely (they hold no state a fresh bucket would not) are dropped first; clients that
//! still do not fit share one overflow bucket, so flooding the table with addresses cannot
//! reset anyone's limit.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Clients tracked individually.
pub const MAX_ENTRIES: usize = 10_000;
/// Shortest time between two sweeps of a full table (each sweep scans every entry).
const SWEEP_EVERY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Debug)]
struct Table {
    map: HashMap<IpAddr, Bucket>,
    /// Shared by every client that does not fit in `map`.
    overflow: Option<Bucket>,
    last_sweep: Option<Instant>,
}

/// A token bucket per client address: `per_minute` attempts, refilled continuously.
#[derive(Debug)]
pub struct RateLimiter {
    table: Mutex<Table>,
    capacity: f64,
    per_sec: f64,
    max_entries: usize,
}

/// The bucket key for an address (IPv6 truncated to its /64).
#[must_use]
pub fn bucket_key(ip: IpAddr) -> IpAddr {
    match crate::config::canonical_ip(ip) {
        IpAddr::V6(v6) => {
            let mut seg = v6.segments();
            for s in &mut seg[4..] {
                *s = 0;
            }
            IpAddr::V6(std::net::Ipv6Addr::from(seg))
        }
        v4 => v4,
    }
}

impl RateLimiter {
    #[must_use]
    pub fn new(per_minute: u32) -> RateLimiter {
        Self::with_max_entries(per_minute, MAX_ENTRIES)
    }

    /// Like [`RateLimiter::new`] with a smaller table (tests).
    #[must_use]
    pub fn with_max_entries(per_minute: u32, max_entries: usize) -> RateLimiter {
        let capacity = f64::from(per_minute.max(1));
        RateLimiter {
            table: Mutex::new(Table {
                map: HashMap::new(),
                overflow: None,
                last_sweep: None,
            }),
            capacity,
            per_sec: capacity / 60.0,
            max_entries: max_entries.max(1),
        }
    }

    /// Take one attempt for `ip`. `Err(retry_after)` when the bucket is empty.
    ///
    /// # Errors
    /// The wait until the next attempt is allowed.
    pub fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        self.check_at(ip, Instant::now())
    }

    fn check_at(&self, ip: IpAddr, now: Instant) -> Result<(), Duration> {
        let key = bucket_key(ip);
        let mut t = match self.table.lock() {
            Ok(m) => m,
            Err(poisoned) => poisoned.into_inner(),
        };
        let fresh = Bucket {
            tokens: self.capacity,
            last: now,
        };
        if !t.map.contains_key(&key) && t.map.len() >= self.max_entries {
            let due = t
                .last_sweep
                .is_none_or(|s| now.saturating_duration_since(s) >= SWEEP_EVERY);
            if due {
                t.last_sweep = Some(now);
                let full_after = Duration::from_secs_f64(self.capacity / self.per_sec);
                t.map
                    .retain(|_, b| now.saturating_duration_since(b.last) < full_after);
            }
        }
        let fits = t.map.contains_key(&key) || t.map.len() < self.max_entries;
        let b = if fits {
            t.map.entry(key).or_insert(fresh)
        } else {
            t.overflow.get_or_insert(fresh)
        };
        self.take(b, now)
    }

    fn take(&self, b: &mut Bucket, now: Instant) -> Result<(), Duration> {
        let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * self.per_sec).min(self.capacity);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            let wait = (1.0 - b.tokens) / self.per_sec;
            Err(Duration::from_secs_f64(wait.max(0.0)))
        }
    }

    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.table.lock().map_or(0, |t| t.map.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(i: u32) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::from(i.wrapping_add(1 << 24)))
    }

    #[test]
    fn allows_the_configured_burst_then_refills() {
        let rl = RateLimiter::new(3);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let t0 = Instant::now();
        for _ in 0..3 {
            rl.check_at(ip, t0).unwrap();
        }
        let wait = rl.check_at(ip, t0).unwrap_err();
        assert!(
            wait <= Duration::from_secs(20) && wait > Duration::from_secs(19),
            "{wait:?}"
        );
        // Another client is unaffected.
        rl.check_at("10.0.0.2".parse().unwrap(), t0).unwrap();
        // 20 s later one attempt is back (3 per minute).
        rl.check_at(ip, t0 + Duration::from_secs(20)).unwrap();
        assert!(rl.check_at(ip, t0 + Duration::from_secs(20)).is_err());
    }

    #[test]
    fn ipv6_is_bucketed_per_64() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::2".parse().unwrap();
        assert_eq!(bucket_key(a), bucket_key(b));
        let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert_eq!(bucket_key(mapped), "127.0.0.1".parse::<IpAddr>().unwrap());
        let rl = RateLimiter::new(1);
        let t0 = Instant::now();
        rl.check_at(a, t0).unwrap();
        assert!(rl.check_at(b, t0).is_err());
    }

    #[test]
    fn table_stays_bounded() {
        let rl = RateLimiter::new(5);
        let t0 = Instant::now();
        for i in 0..(u32::try_from(MAX_ENTRIES).unwrap() + 10) {
            let _ = rl.check_at(v4(i), t0);
        }
        assert!(rl.tracked() <= MAX_ENTRIES);
    }

    /// A full table must not reset anyone's limit: clients that do not fit share one
    /// overflow bucket instead of the table being cleared.
    #[test]
    fn a_full_table_shares_one_overflow_bucket_instead_of_failing_open() {
        let rl = RateLimiter::with_max_entries(2, 3);
        let t0 = Instant::now();
        let victim = v4(0);
        rl.check_at(victim, t0).unwrap();
        rl.check_at(victim, t0).unwrap();
        assert!(rl.check_at(victim, t0).is_err());
        rl.check_at(v4(1), t0).unwrap();
        rl.check_at(v4(2), t0).unwrap();
        // The table is full of buckets that have not refilled: newcomers share the
        // overflow bucket (capacity 2), and the victim stays limited.
        rl.check_at(v4(3), t0).unwrap();
        rl.check_at(v4(4), t0).unwrap();
        assert!(rl.check_at(v4(5), t0).is_err());
        assert!(rl.check_at(v4(6), t0).is_err());
        assert!(rl.check_at(victim, t0).is_err());
        assert_eq!(rl.tracked(), 3);
    }

    #[test]
    fn refilled_buckets_make_room_first() {
        let rl = RateLimiter::with_max_entries(2, 3);
        let t0 = Instant::now();
        for i in 0..3 {
            rl.check_at(v4(i), t0).unwrap();
        }
        // A minute later every bucket is full again and carries no state: dropping them
        // gives the newcomer a bucket of its own.
        let later = t0 + Duration::from_secs(61);
        rl.check_at(v4(10), later).unwrap();
        rl.check_at(v4(10), later).unwrap();
        assert!(rl.check_at(v4(10), later).is_err());
        rl.check_at(v4(11), later).unwrap();
        assert!(rl.tracked() <= 3);
    }
}
