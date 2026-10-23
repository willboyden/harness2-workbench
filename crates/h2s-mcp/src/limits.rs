//! Bounds on what callers can hold open.
//!
//! Waiting is the expensive thing an MCP caller can do: one `wait_for_job` call (or a job tool
//! with `wait_secs`) holds a request and a job event stream open for up to 600 s. The
//! [`WaitLimiter`] caps how many of those each API token may run at once. (The HTTP transport
//! also caps concurrent requests overall; see [`crate::http`].)

use std::{
    collections::HashMap,
    fmt,
    hash::{BuildHasher, RandomState},
    sync::{Arc, Mutex, PoisonError},
};

use crate::token::Token;

/// Default cap on concurrent waits per API token.
pub const DEFAULT_WAITS_PER_TOKEN: usize = 4;

/// Caps how many waits each API token may run at once.
///
/// Tokens are not kept: each is reduced to a hash keyed with a random per-process key, which is
/// enough to tell callers apart and useless for anything else.
#[derive(Clone)]
pub struct WaitLimiter {
    inner: Arc<Inner>,
}

struct Inner {
    max_per_token: usize,
    keys: RandomState,
    active: Mutex<HashMap<u64, usize>>,
}

impl WaitLimiter {
    /// A limiter allowing `max_per_token` concurrent waits per token (at least 1).
    #[must_use]
    pub fn new(max_per_token: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                max_per_token: max_per_token.max(1),
                keys: RandomState::new(),
                active: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// The cap per token.
    #[must_use]
    pub fn max_per_token(&self) -> usize {
        self.inner.max_per_token
    }

    /// Take a wait slot for `token`, or `None` when it already runs the maximum. The slot is
    /// given back when the returned guard is dropped.
    pub(crate) fn try_acquire(&self, token: &Token) -> Option<WaitSlot> {
        let key = self.inner.keys.hash_one(token.secret());
        let mut active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let running = active.entry(key).or_insert(0);
        if *running >= self.inner.max_per_token {
            return None;
        }
        *running += 1;
        Some(WaitSlot {
            inner: Arc::clone(&self.inner),
            key,
        })
    }

    /// Waits currently running for `token` (tests and diagnostics).
    #[cfg(test)]
    fn running(&self, token: &Token) -> usize {
        let key = self.inner.keys.hash_one(token.secret());
        let active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        active.get(&key).copied().unwrap_or(0)
    }
}

impl Default for WaitLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_WAITS_PER_TOKEN)
    }
}

impl fmt::Debug for WaitLimiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WaitLimiter")
            .field("max_per_token", &self.inner.max_per_token)
            .finish_non_exhaustive()
    }
}

/// One running wait; dropping it frees the slot.
pub(crate) struct WaitSlot {
    inner: Arc<Inner>,
    key: u64,
}

impl Drop for WaitSlot {
    fn drop(&mut self) {
        let mut active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(running) = active.get_mut(&self.key) {
            *running = running.saturating_sub(1);
            if *running == 0 {
                active.remove(&self.key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_each_token_separately_and_frees_slots_on_drop() {
        let limiter = WaitLimiter::new(2);
        let alice = Token::parse("h2s_alice_s3cret").unwrap();
        let bob = Token::parse("h2s_bobby_s3cret").unwrap();

        let a1 = limiter.try_acquire(&alice).unwrap();
        let a2 = limiter.try_acquire(&alice).unwrap();
        assert!(limiter.try_acquire(&alice).is_none(), "third wait refused");
        let b1 = limiter
            .try_acquire(&bob)
            .expect("another token is not affected");
        assert_eq!((limiter.running(&alice), limiter.running(&bob)), (2, 1));

        drop(a1);
        let a3 = limiter
            .try_acquire(&alice)
            .expect("a freed slot can be reused");
        drop((a2, a3, b1));
        assert_eq!((limiter.running(&alice), limiter.running(&bob)), (0, 0));
        assert!(
            limiter.inner.active.lock().unwrap().is_empty(),
            "no entries left behind"
        );
    }

    #[test]
    #[allow(clippy::redundant_clone)] // the clone is what is being tested
    fn clones_share_the_count_and_the_cap_is_at_least_one() {
        let limiter = WaitLimiter::new(0);
        assert_eq!(limiter.max_per_token(), 1);
        let token = Token::parse("h2s_alice_s3cret").unwrap();
        let _slot = limiter.clone().try_acquire(&token).unwrap();
        assert!(limiter.try_acquire(&token).is_none());
    }

    #[test]
    fn debug_shows_no_keys() {
        let limiter = WaitLimiter::new(3);
        let token = Token::parse("h2s_alice_s3cret").unwrap();
        let _slot = limiter.try_acquire(&token);
        assert_eq!(
            format!("{limiter:?}"),
            "WaitLimiter { max_per_token: 3, .. }"
        );
    }
}
