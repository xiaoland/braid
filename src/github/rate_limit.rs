//! One admission lock per installation, shared explicitly by clients in an instance.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::http::HeaderMap;
use tokio::sync::Mutex as AsyncMutex;

#[derive(Default)]
pub struct RateLimits(Mutex<HashMap<u64, Arc<AsyncMutex<Budget>>>>);

impl RateLimits {
    pub fn installation(&self, id: u64) -> Arc<AsyncMutex<Budget>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(id)
            .or_default()
            .clone()
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[derive(Debug, Default)]
pub struct Budget {
    pub requests: u64,
    pub used: Option<u64>,
    pub remaining: Option<u64>,
    pub reset: Option<u64>,
    until: u64,
    failures: u32,
}

impl Budget {
    /// # Errors
    /// Returns the reason admission is delayed, without waiting or issuing a request.
    pub fn admit(&mut self, now: u64, background: bool) -> Result<(), String> {
        self.check(now, background)?;
        self.requests += 1;
        Ok(())
    }

    /// # Errors
    /// Reports local cooldown/reserve delay without counting or issuing a request.
    pub fn check(&mut self, now: u64, background: bool) -> Result<(), String> {
        if now < self.until {
            return Err(format!("installation cooldown until {}", self.until));
        }
        if self.reset.is_some_and(|reset| now >= reset) {
            self.remaining = None;
            self.used = None;
            self.reset = None;
        }
        if background && self.remaining.is_some_and(|remaining| remaining <= 100) {
            if self.reset.is_none() {
                // A partial/malformed header set must not park scanning forever.
                self.remaining = None;
                self.defer(now, None);
            }
            return Err(
                "background scan delayed: reserving last 100 GraphQL points for foreground".into(),
            );
        }
        Ok(())
    }

    /// Headers describe actual responses, not an estimated per-query cost.
    pub fn observe(&mut self, headers: &HeaderMap) -> Option<u64> {
        let number = |name| {
            headers.get(name).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok())
        };
        let used = number("x-ratelimit-used");
        let delta = used.zip(self.used).and_then(|(new, old)| new.checked_sub(old));
        self.used = used.or(self.used);
        self.remaining = number("x-ratelimit-remaining").or(self.remaining);
        self.reset = number("x-ratelimit-reset").or(self.reset);
        delta
    }

    pub fn success(&mut self) {
        self.failures = 0;
        self.until = 0;
    }

    pub fn defer(&mut self, now: u64, retry_after: Option<u64>) {
        self.failures = self.failures.saturating_add(1);
        let backoff = (5_u64 << self.failures.min(6)).min(300);
        let jitter = rand::random::<u64>() % (backoff / 4 + 1);
        let until = if let Some(seconds) = retry_after {
            let retry = now.saturating_add(seconds.max(1));
            if self.remaining == Some(0) { retry.max(self.reset.unwrap_or(retry)) } else { retry }
        } else if self.remaining == Some(0) && self.reset.is_some_and(|reset| reset > now) {
            self.reset.unwrap_or(now)
        } else {
            now.saturating_add(backoff + jitter)
        };
        // A later transport/body failure must not shorten Retry-After or
        // primary-reset guidance already received in the response headers.
        self.until = self.until.max(until);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn installation_sharing_and_isolation() {
        let limits = RateLimits::default();
        let a = limits.installation(1);
        let same = limits.installation(1);
        let other = limits.installation(2);
        a.lock().await.defer(100, Some(60));
        assert!(same.lock().await.admit(159, false).is_err());
        assert!(other.lock().await.admit(101, false).is_ok());
        assert!(same.lock().await.admit(160, false).is_ok());
        same.lock().await.success();
        assert!(a.lock().await.admit(161, false).is_ok());
    }

    #[test]
    fn primary_reset_reserve_and_used_delta() {
        let mut budget = Budget::default();
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-used", "4900".parse().unwrap());
        headers.insert("x-ratelimit-remaining", "100".parse().unwrap());
        headers.insert("x-ratelimit-reset", "500".parse().unwrap());
        assert_eq!(budget.observe(&headers), None);
        assert!(budget.admit(100, true).is_err());
        assert!(budget.admit(100, false).is_ok());
        headers.insert("x-ratelimit-used", "5000".parse().unwrap());
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        assert_eq!(budget.observe(&headers), Some(100));
        budget.defer(100, None);
        assert!(budget.admit(499, false).is_err());
        assert!(budget.admit(500, true).is_ok());
    }

    #[test]
    fn missing_reset_does_not_park_reserve_and_primary_respects_both_hints() {
        let mut budget = Budget::default();
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "10".parse().unwrap());
        budget.observe(&headers);
        assert!(budget.admit(100, true).is_err());
        assert!(budget.admit(101, true).is_err());
        assert!(budget.admit(113, true).is_ok());
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        headers.insert("x-ratelimit-reset", "500".parse().unwrap());
        budget.observe(&headers);
        budget.defer(200, Some(10));
        assert!(budget.admit(499, false).is_err());
        assert!(budget.admit(500, true).is_ok());
    }

    #[test]
    fn subsequent_unhinted_failure_cannot_shorten_server_cooldown() {
        let mut budget = Budget::default();
        budget.defer(100, Some(600));
        budget.defer(130, None);
        assert!(budget.admit(699, false).is_err());
        assert!(budget.admit(700, false).is_ok());
    }

    #[test]
    fn missing_hints_backoff_is_bounded_and_recovers() {
        let mut budget = Budget::default();
        for _ in 0..100 {
            budget.defer(1000, None);
            assert!((1010..=1375).contains(&budget.until));
            assert!(budget.admit(1001, false).is_err());
        }
        assert!(budget.admit(1375, false).is_ok());
        budget.success();
        budget.defer(1400, None);
        assert!((1410..=1412).contains(&budget.until));
    }
}
