//! Token buckets per client, grouped by owner (login attempts, failed authentication, audit
//! records of failures and of refusals).
//!
//! A key is `(owner, client)`. For address limiters the owner is the TCP peer and the client
//! the address the request is attributed to: the peer itself, or, behind a trusted reverse
//! proxy, the `X-Forwarded-For` client. Every address is bucketed per IPv6 /64 (one host
//! usually controls a whole /64). For principal limiters the owner is the principal and there
//! is one key per owner.
//!
//! The table is bounded and never fails open, and no owner can starve another:
//! - an owner holds at most `per_owner` keys; its further clients share one overflow bucket
//!   of that owner, so a peer that invents `X-Forwarded-For` addresses (a job on the proxy's
//!   network, say) only limits itself;
//! - when the whole table is full, buckets that have refilled completely (they hold no state
//!   a fresh bucket would not) are dropped first, at most once a second; a newcomer that still
//!   does not fit uses its owner's overflow bucket, so a fresh client from another peer starts
//!   with a full bucket of its own;
//! - overflow buckets are bounded too; past that bound the remaining owners share one last
//!   bucket.

use std::collections::HashMap;
use std::hash::Hash;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Keys tracked individually (all owners together).
pub const MAX_ENTRIES: usize = 10_000;
/// Keys one owner may hold in an address limiter (its other clients share one bucket).
pub const MAX_PER_PEER: usize = 1_024;
/// Owners with an overflow bucket of their own.
const MAX_OWNER_OVERFLOWS: usize = 4_096;
/// Shortest time between two sweeps of a full table (each sweep scans every entry).
const SWEEP_EVERY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Debug)]
struct Table<O, C> {
    map: HashMap<(O, C), Bucket>,
    /// Keys in `map` per owner.
    owned: HashMap<O, usize>,
    /// Shared by an owner's clients that do not fit.
    overflow: HashMap<O, Bucket>,
    /// Shared by the owners that do not fit in `overflow`.
    last_resort: Option<Bucket>,
    last_sweep: Option<Instant>,
}

/// Token buckets keyed by `(owner, client)`: `per_minute` attempts each, refilled
/// continuously.
#[derive(Debug)]
pub struct RateLimiter<O = IpAddr, C = IpAddr> {
    table: Mutex<Table<O, C>>,
    capacity: f64,
    per_sec: f64,
    max_entries: usize,
    per_owner: usize,
}

/// A limiter with one bucket per principal (the owner is the principal's key).
pub type PrincipalLimiter = RateLimiter<String, ()>;

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

impl RateLimiter<IpAddr, IpAddr> {
    /// An address limiter: [`MAX_ENTRIES`] keys, [`MAX_PER_PEER`] per peer.
    #[must_use]
    pub fn new(per_minute: u32) -> Self {
        Self::with_limits(per_minute, MAX_ENTRIES, MAX_PER_PEER)
    }

    /// Take one attempt for `client` reached through the TCP peer `peer` (the same address
    /// when there is no proxy). `Err(retry_after)` when its bucket is empty.
    ///
    /// # Errors
    /// The wait until the next attempt is allowed.
    pub fn check_from(&self, peer: IpAddr, client: IpAddr) -> Result<(), Duration> {
        self.check_at((bucket_key(peer), bucket_key(client)), Instant::now())
    }

    /// [`RateLimiter::check_from`] for a client that is its own peer.
    ///
    /// # Errors
    /// The wait until the next attempt is allowed.
    pub fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        self.check_from(ip, ip)
    }
}

impl RateLimiter<String, ()> {
    /// A principal limiter: [`MAX_ENTRIES`] principals, one key each.
    #[must_use]
    pub fn per_principal(per_minute: u32) -> Self {
        Self::with_limits(per_minute, MAX_ENTRIES, 1)
    }

    /// Take one attempt for `principal`.
    ///
    /// # Errors
    /// The wait until the next attempt is allowed.
    pub fn check_principal(&self, principal: &str) -> Result<(), Duration> {
        self.check_at((principal.to_owned(), ()), Instant::now())
    }
}

impl<O: Eq + Hash + Clone, C: Eq + Hash + Clone> RateLimiter<O, C> {
    /// A limiter holding at most `max_entries` keys, `per_owner` of them per owner.
    #[must_use]
    pub fn with_limits(per_minute: u32, max_entries: usize, per_owner: usize) -> Self {
        let capacity = f64::from(per_minute.max(1));
        RateLimiter {
            table: Mutex::new(Table {
                map: HashMap::new(),
                owned: HashMap::new(),
                overflow: HashMap::new(),
                last_resort: None,
                last_sweep: None,
            }),
            capacity,
            per_sec: capacity / 60.0,
            max_entries: max_entries.max(1),
            per_owner: per_owner.max(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Table<O, C>> {
        match self.table.lock() {
            Ok(m) => m,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn check_at(&self, key: (O, C), now: Instant) -> Result<(), Duration> {
        let mut t = self.lock();
        let fresh = Bucket {
            tokens: self.capacity,
            last: now,
        };
        if let Some(b) = t.map.get_mut(&key) {
            return self.take(b, now);
        }
        if t.map.len() >= self.max_entries {
            self.sweep(&mut t, now);
        }
        let owner = key.0.clone();
        let owned = t.owned.get(&owner).copied().unwrap_or(0);
        if t.map.len() < self.max_entries && owned < self.per_owner {
            *t.owned.entry(owner).or_insert(0) += 1;
            let b = t.map.entry(key).or_insert(fresh);
            return self.take(b, now);
        }
        // The owner's own overflow bucket, or the last resort when there are too many.
        if t.overflow.contains_key(&owner) || t.overflow.len() < MAX_OWNER_OVERFLOWS {
            let b = t.overflow.entry(owner).or_insert(fresh);
            return self.take(b, now);
        }
        let b = t.last_resort.get_or_insert(fresh);
        self.take(b, now)
    }

    /// Drop buckets that have refilled completely (they are indistinguishable from fresh
    /// ones), at most once per [`SWEEP_EVERY`].
    fn sweep(&self, t: &mut Table<O, C>, now: Instant) {
        if t.last_sweep
            .is_some_and(|s| now.saturating_duration_since(s) < SWEEP_EVERY)
        {
            return;
        }
        t.last_sweep = Some(now);
        let full_after = Duration::from_secs_f64(self.capacity / self.per_sec);
        let refilled = |b: &Bucket| now.saturating_duration_since(b.last) >= full_after;
        let mut freed: Vec<O> = Vec::new();
        t.map.retain(|(owner, _), b| {
            let keep = !refilled(b);
            if !keep {
                freed.push(owner.clone());
            }
            keep
        });
        for owner in freed {
            if let Some(n) = t.owned.get_mut(&owner) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    t.owned.remove(&owner);
                }
            }
        }
        t.overflow.retain(|_, b| !refilled(b));
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
        self.lock().map.len()
    }

    #[cfg(test)]
    fn owned_by(&self, owner: &O) -> usize {
        self.lock().owned.get(owner).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(i: u32) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::from(i.wrapping_add(1 << 24)))
    }

    /// Single-address keys, as for a client that is its own peer.
    fn own(ip: IpAddr) -> (IpAddr, IpAddr) {
        (ip, ip)
    }

    #[test]
    fn allows_the_configured_burst_then_refills() {
        let rl = RateLimiter::new(3);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let t0 = Instant::now();
        for _ in 0..3 {
            rl.check_at(own(ip), t0).unwrap();
        }
        let wait = rl.check_at(own(ip), t0).unwrap_err();
        assert!(
            wait <= Duration::from_secs(20) && wait > Duration::from_secs(19),
            "{wait:?}"
        );
        // Another client is unaffected.
        rl.check_at(own("10.0.0.2".parse().unwrap()), t0).unwrap();
        // 20 s later one attempt is back (3 per minute).
        rl.check_at(own(ip), t0 + Duration::from_secs(20)).unwrap();
        assert!(rl.check_at(own(ip), t0 + Duration::from_secs(20)).is_err());
    }

    #[test]
    fn ipv6_is_bucketed_per_64() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::2".parse().unwrap();
        assert_eq!(bucket_key(a), bucket_key(b));
        let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert_eq!(bucket_key(mapped), "127.0.0.1".parse::<IpAddr>().unwrap());
        let rl = RateLimiter::new(1);
        rl.check(a).unwrap();
        assert!(rl.check(b).is_err());
    }

    #[test]
    fn table_stays_bounded() {
        let rl = RateLimiter::new(5);
        let t0 = Instant::now();
        for i in 0..(u32::try_from(MAX_ENTRIES).unwrap() + 10) {
            let _ = rl.check_at(own(v4(i)), t0);
        }
        assert!(rl.tracked() <= MAX_ENTRIES);
    }

    /// A full table must not reset anyone's limit: clients that do not fit share their
    /// owner's overflow bucket instead of the table being cleared.
    #[test]
    fn a_full_table_shares_overflow_buckets_instead_of_failing_open() {
        let rl = RateLimiter::with_limits(2, 3, 3);
        let t0 = Instant::now();
        let peer = v4(1000);
        let key = |i: u32| (peer, v4(i));
        rl.check_at(key(0), t0).unwrap();
        rl.check_at(key(0), t0).unwrap();
        assert!(rl.check_at(key(0), t0).is_err());
        rl.check_at(key(1), t0).unwrap();
        rl.check_at(key(2), t0).unwrap();
        // The table is full of buckets that have not refilled: this peer's newcomers share
        // its overflow bucket (capacity 2), and the limited client stays limited.
        rl.check_at(key(3), t0).unwrap();
        rl.check_at(key(4), t0).unwrap();
        assert!(rl.check_at(key(5), t0).is_err());
        assert!(rl.check_at(key(0), t0).is_err());
        assert_eq!(rl.tracked(), 3);
    }

    #[test]
    fn refilled_buckets_make_room_first() {
        let rl = RateLimiter::with_limits(2, 3, 3);
        let t0 = Instant::now();
        for i in 0..3 {
            rl.check_at(own(v4(i)), t0).unwrap();
        }
        // A minute later every bucket is full again and carries no state: dropping them
        // gives the newcomer a bucket of its own.
        let later = t0 + Duration::from_secs(61);
        rl.check_at(own(v4(10)), later).unwrap();
        rl.check_at(own(v4(10)), later).unwrap();
        assert!(rl.check_at(own(v4(10)), later).is_err());
        rl.check_at(own(v4(11)), later).unwrap();
        assert!(rl.tracked() <= 3);
        assert_eq!(rl.owned_by(&v4(0)), 0, "swept keys are no longer counted");
    }

    /// A trusted peer that invents `X-Forwarded-For` clients holds at most its quota of keys
    /// and then limits only itself; a client behind another peer still gets a full bucket.
    #[test]
    fn one_peer_cannot_fill_the_table_or_starve_another() {
        let rl = RateLimiter::with_limits(1, 100, 10);
        let t0 = Instant::now();
        let flooder = v4(5000);
        let honest = v4(6000);
        let mut allowed = 0;
        for i in 0..1000 {
            if rl.check_at((flooder, v4(i)), t0).is_ok() {
                allowed += 1;
            }
        }
        // Its quota of keys, plus one attempt from its shared overflow bucket.
        assert_eq!(allowed, 10 + 1);
        assert_eq!(rl.owned_by(&flooder), 10);
        assert_eq!(rl.tracked(), 10);
        rl.check_at((honest, v4(1)), t0).unwrap();
        rl.check_at((honest, v4(2)), t0).unwrap();
        assert_eq!(rl.owned_by(&honest), 2);
    }

    /// With the whole table full, a fresh client behind another peer is not starved either:
    /// it uses that peer's own overflow bucket, not the flooders'.
    #[test]
    fn a_full_table_still_gives_each_peer_a_bucket() {
        let rl = RateLimiter::with_limits(1, 20, 10);
        let t0 = Instant::now();
        // Two flooding peers fill the table (10 keys each).
        for p in [v4(7000), v4(7001)] {
            for i in 0..50 {
                let _ = rl.check_at((p, v4(i)), t0);
            }
        }
        assert_eq!(rl.tracked(), 20);
        let honest = v4(8000);
        rl.check_at((honest, v4(1)), t0).unwrap();
        assert!(
            rl.check_at((honest, v4(2)), t0).is_err(),
            "one shared bucket"
        );
        assert!(rl.check_at((v4(8001), v4(3)), t0).is_ok(), "another peer");
    }

    #[test]
    fn principals_have_one_bucket_each() {
        let rl = PrincipalLimiter::per_principal(2);
        rl.check_principal("token:abc").unwrap();
        rl.check_principal("token:abc").unwrap();
        assert!(rl.check_principal("token:abc").is_err());
        rl.check_principal("token:def").unwrap();
    }
}
