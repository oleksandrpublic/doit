//! Shared HTTP rate limiting helpers.

use lazy_static::lazy_static;
use std::collections::HashMap;
use tokio::sync::Mutex;

/// Simple rate limiter for HTTP requests.
/// Tracks last request time and enforces minimum delay between requests.
pub struct RateLimiter {
    last_request: std::time::Instant,
    min_delay: std::time::Duration,
}

lazy_static! {
    static ref RATE_LIMITERS: Mutex<HashMap<String, RateLimiter>> = Mutex::new(HashMap::new());
}

impl RateLimiter {
    /// Create a new rate limiter with specified minimum delay between requests.
    pub fn new(min_delay_secs: u64) -> Self {
        Self {
            last_request: std::time::Instant::now(),
            min_delay: std::time::Duration::from_secs(min_delay_secs),
        }
    }

    /// Rate limit by name, creating or reusing a limiter for the given name.
    pub async fn limit(name: &str, min_delay_secs: u64) {
        let mut limiters = RATE_LIMITERS.lock().await;
        let limiter = limiters
            .entry(name.to_string())
            .or_insert_with(|| RateLimiter::new(min_delay_secs));
        limiter.wait().await;
    }

    /// Wait if necessary to respect rate limit, then update timestamp.
    pub async fn wait(&mut self) {
        let elapsed = self.last_request.elapsed();
        if elapsed < self.min_delay {
            let remaining = self.min_delay - elapsed;
            tokio::time::sleep(remaining).await;
        }
        self.last_request = std::time::Instant::now();
    }

    /// Check if a request can be made immediately without waiting.
    pub fn can_request_now(&self) -> bool {
        self.last_request.elapsed() >= self.min_delay
    }
}

/// Default rate limiter for web requests.
pub fn default_web_rate_limiter() -> RateLimiter {
    RateLimiter::new(1)
}

/// Conservative rate limiter for GitHub API.
pub fn github_rate_limiter() -> RateLimiter {
    RateLimiter::new(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_creation() {
        let limiter = RateLimiter::new(1);
        assert_eq!(limiter.min_delay, std::time::Duration::from_secs(1));
    }

    #[test]
    fn test_can_request_now_initially_false() {
        let limiter = RateLimiter::new(1);
        assert!(!limiter.can_request_now());
    }

    #[tokio::test]
    async fn test_wait_respects_delay() {
        let mut limiter = RateLimiter::new(0);
        limiter.wait().await;
        assert!(limiter.can_request_now());
    }
}
