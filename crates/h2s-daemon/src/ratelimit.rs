//! Per-client token buckets (login attempts).
//!
//! IPv6 clients are bucketed per /64, because one host usually controls a whole /64. The
//! table is bounded: when it is full, idle buckets are dropped first, then everything (the
//! limiter is defence in depth on top of 256-bit token secrets, so it fails open rather
//! than locking every new client out).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// A token bucket per client address: `per_minute` attempts, refilled continuously.
#[derive(Debug)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
    capacity: f64,
    per_sec: f64,
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
        let capacity = f64::from(per_minute.max(1));
        RateLimiter {
            buckets: Mutex::new(HashMap::new()),
            capacity,
            per_sec: capacity / 60.0,
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
        let mut map = match self.buckets.lock() {
            Ok(m) => m,
            Err(poisoned) => poisoned.into_inner(),
        };
        if map.len() >= MAX_ENTRIES && !map.contains_key(&key) {
            let full_after = Duration::from_secs_f64(self.capacity / self.per_sec);
            map.retain(|_, b| now.saturating_duration_since(b.last) < full_after);
            if map.len() >= MAX_ENTRIES {
                map.clear();
            }
        }
        let b = map.entry(key).or_insert(Bucket {
            tokens: self.capacity,
            last: now,
        });
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let ip = IpAddr::V4(std::net::Ipv4Addr::from(i.wrapping_add(1 << 24)));
            rl.check_at(ip, t0).unwrap();
        }
        assert!(rl.buckets.lock().unwrap().len() <= MAX_ENTRIES);
    }
}
