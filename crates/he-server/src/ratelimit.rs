//! Exponential backoff for failed authentication, keyed on both the device and
//! the account it is aiming at.
//!
//! PLAN §11 asks for rate limiting *per `EndpointId` and per username*, and
//! both halves are load-bearing:
//!
//! - Per **username**, an attacker cannot spread a guessing run for one account
//!   across a thousand freshly generated keys — an `EndpointId` costs nothing
//!   to make, so it is worthless on its own as an identifier to limit.
//! - Per **`EndpointId`**, one machine cannot walk the account list trying
//!   `password123` against each in turn.
//!
//! State is in memory, so a restart forgives everyone. That is the right
//! trade: persisting it would let a stranger lock an account out of its own
//! database, and the attack this stops is measured in minutes.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::{Result, ServerError};

/// What a failure counter is attached to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// The device, as proven by iroh's key exchange.
    Endpoint(String),
    /// The account being aimed at, lowercased.
    Username(String),
}

#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// Failures allowed before any waiting starts. Typos are normal.
    pub free_attempts: u32,
    /// Wait after the first non-free failure; doubles from there.
    pub base_delay: Duration,
    /// Ceiling, so a forgotten client retrying forever is not locked out for
    /// days.
    pub max_delay: Duration,
    /// How long an untouched counter is kept before it may be pruned.
    pub idle_ttl: Duration,
    /// Upper bound on tracked keys. Without it, an attacker who can mint
    /// `EndpointId`s could make us allocate until we die — which would be a
    /// denial of service delivered *by* the denial-of-service defence.
    pub max_tracked: usize,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            free_attempts: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(15 * 60),
            idle_ttl: Duration::from_secs(60 * 60),
            max_tracked: 4096,
        }
    }
}

#[derive(Debug)]
struct Entry {
    failures: u32,
    blocked_until: Option<Instant>,
    last_touched: Instant,
}

#[derive(Debug)]
pub struct RateLimiter {
    policy: Policy,
    entries: Mutex<HashMap<Key, Entry>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(Policy::default())
    }
}

impl RateLimiter {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Refuses the attempt if any of `keys` is currently backing off.
    ///
    /// Call this **before** hashing a password: the point is to not spend
    /// 100 ms of CPU on an attempt we have already decided to reject.
    pub fn check(&self, keys: &[Key]) -> Result<()> {
        self.check_at(keys, Instant::now())
    }

    /// Records a failed attempt against every key involved.
    pub fn record_failure(&self, keys: &[Key]) {
        self.record_failure_at(keys, Instant::now());
    }

    /// Clears the counters after a success, so a user who eventually remembers
    /// their password is not still serving a sentence.
    pub fn record_success(&self, keys: &[Key]) {
        let mut entries = self.lock();
        for key in keys {
            entries.remove(key);
        }
    }

    fn check_at(&self, keys: &[Key], now: Instant) -> Result<()> {
        let entries = self.lock();
        let mut longest = Duration::ZERO;
        for key in keys {
            if let Some(blocked_until) = entries.get(key).and_then(|e| e.blocked_until)
                && blocked_until > now
            {
                longest = longest.max(blocked_until - now);
            }
        }
        if longest > Duration::ZERO {
            return Err(ServerError::RateLimited {
                retry_after: longest,
            });
        }
        Ok(())
    }

    fn record_failure_at(&self, keys: &[Key], now: Instant) {
        let mut entries = self.lock();
        if entries.len() >= self.policy.max_tracked {
            prune(&mut entries, now, self.policy.idle_ttl);
        }
        for key in keys {
            let entry = entries.entry(key.clone()).or_insert(Entry {
                failures: 0,
                blocked_until: None,
                last_touched: now,
            });
            entry.failures = entry.failures.saturating_add(1);
            entry.last_touched = now;
            entry.blocked_until = self
                .delay_for(entry.failures)
                .map(|delay| now + delay)
                // Never shorten an existing block: two keys failing at
                // different rates must not let the faster one reset the slower.
                .map(|until| entry.blocked_until.map_or(until, |prev| prev.max(until)));
        }
    }

    /// `None` while the attempt is still free, otherwise `base * 2^n`, capped.
    fn delay_for(&self, failures: u32) -> Option<Duration> {
        let over = failures.checked_sub(self.policy.free_attempts)?;
        if over == 0 {
            return None;
        }
        let doublings = over - 1;
        let delay = self
            .policy
            .base_delay
            .checked_mul(1u32.checked_shl(doublings.min(31))?)
            .unwrap_or(self.policy.max_delay);
        Some(delay.min(self.policy.max_delay))
    }

    /// A poisoned lock means some other thread panicked while holding it. The
    /// data behind it is a counter map; carrying on with it is strictly better
    /// than taking the whole server down over a failed login count.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn prune(entries: &mut HashMap<Key, Entry>, now: Instant, idle_ttl: Duration) {
    entries.retain(|_, entry| {
        let still_blocked = entry.blocked_until.is_some_and(|until| until > now);
        // Dropping a *blocked* entry would hand an attacker a free reset, so
        // those stay regardless of age.
        still_blocked || now.duration_since(entry.last_touched) < idle_ttl
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<Key> {
        vec![
            Key::Endpoint("endpoint-a".into()),
            Key::Username("justin".into()),
        ]
    }

    #[test]
    fn typos_are_free_then_the_wait_doubles() {
        let limiter = RateLimiter::new(Policy::default());
        let t0 = Instant::now();
        let keys = keys();

        for _ in 0..3 {
            assert!(limiter.check_at(&keys, t0).is_ok(), "typos must be free");
            limiter.record_failure_at(&keys, t0);
        }

        // Three failures cost nothing; the fourth starts the clock at 1s, the
        // fifth at 2s.
        assert!(limiter.check_at(&keys, t0).is_ok());
        limiter.record_failure_at(&keys, t0);
        assert!(limiter.check_at(&keys, t0).is_err());
        assert!(
            limiter
                .check_at(&keys, t0 + Duration::from_millis(1100))
                .is_ok()
        );

        limiter.record_failure_at(&keys, t0 + Duration::from_millis(1100));
        let t1 = t0 + Duration::from_millis(1100);
        assert!(
            limiter
                .check_at(&keys, t1 + Duration::from_millis(1900))
                .is_err()
        );
        assert!(
            limiter
                .check_at(&keys, t1 + Duration::from_millis(2100))
                .is_ok()
        );
    }

    #[test]
    fn the_wait_is_capped() {
        let limiter = RateLimiter::new(Policy::default());
        let t0 = Instant::now();
        let keys = keys();
        for _ in 0..64 {
            limiter.record_failure_at(&keys, t0);
        }
        let err = limiter.check_at(&keys, t0).expect_err("must be blocked");
        let ServerError::RateLimited { retry_after } = err else {
            panic!("wrong error");
        };
        assert!(retry_after <= Policy::default().max_delay);
        assert!(retry_after > Duration::from_secs(60), "{retry_after:?}");
    }

    #[test]
    fn a_success_forgives() {
        let limiter = RateLimiter::new(Policy::default());
        let t0 = Instant::now();
        let keys = keys();
        for _ in 0..6 {
            limiter.record_failure_at(&keys, t0);
        }
        assert!(limiter.check_at(&keys, t0).is_err());
        limiter.record_success(&keys);
        assert!(limiter.check_at(&keys, t0).is_ok());
    }

    #[test]
    fn a_username_is_limited_across_devices() {
        // The attack this stops: one guessing run spread over a thousand
        // freshly generated keys, which cost nothing to make.
        let limiter = RateLimiter::new(Policy::default());
        let t0 = Instant::now();
        for attempt in 0..6 {
            let keys = vec![
                Key::Endpoint(format!("throwaway-endpoint-{attempt}")),
                Key::Username("justin".into()),
            ];
            limiter.record_failure_at(&keys, t0);
        }

        let fresh_device = vec![
            Key::Endpoint("brand-new-key".into()),
            Key::Username("justin".into()),
        ];
        assert!(
            limiter.check_at(&fresh_device, t0).is_err(),
            "a new EndpointId must not reset the username's counter"
        );

        // A different account from that new device is unaffected.
        let other = vec![
            Key::Endpoint("brand-new-key".into()),
            Key::Username("someone-else".into()),
        ];
        assert!(limiter.check_at(&other, t0).is_ok());
    }

    #[test]
    fn pruning_keeps_blocked_entries_and_drops_idle_ones() {
        let policy = Policy {
            max_tracked: 4,
            idle_ttl: Duration::from_secs(60),
            ..Policy::default()
        };
        let limiter = RateLimiter::new(policy);
        let t0 = Instant::now();

        // One key that ends up deep enough into backoff to still be blocked
        // two minutes later.
        let blocked = vec![Key::Username("blocked".into())];
        for _ in 0..12 {
            limiter.record_failure_at(&blocked, t0);
        }
        // Several that only ever failed once, long ago.
        for i in 0..8 {
            limiter.record_failure_at(&[Key::Endpoint(format!("idle-{i}"))], t0);
        }

        let later = t0 + Duration::from_secs(120);
        limiter.record_failure_at(&[Key::Endpoint("fresh".into())], later);

        let entries = limiter.lock();
        assert!(
            entries.len() <= policy.max_tracked + 1,
            "unbounded growth is a denial of service: {}",
            entries.len()
        );
        assert!(
            entries.contains_key(&Key::Username("blocked".into())),
            "pruning must not hand out free resets"
        );
    }
}
