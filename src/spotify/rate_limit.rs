//! Backoff after `429 Too Many Requests`.
//!
//! After a 429, the client sends no request until the wait ends. A caller during the wait gets
//! [`ApiError::RateLimited`](super::ApiError::RateLimited) immediately. Each consecutive 429
//! doubles the wait, which is never shorter than the value of `Retry-After`.

use std::time::{Duration, Instant};

/// The wait after the first 429, when it has no `Retry-After`.
const BASE_WAIT: Duration = Duration::from_secs(2);
/// The wait stops to double here.
const MAX_BACKOFF: Duration = Duration::from_secs(120);
/// The longest `Retry-After` that the client accepts, as protection against very large values.
/// One request each hour is not a problem for Spotify.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

/// The state of the backoff, which all clones of the client share.
#[derive(Debug, Default)]
pub(crate) struct RateLimit {
    /// The number of consecutive 429s, without a successful response between them.
    strikes: u32,
    /// The client sends no request before this time.
    until: Option<Instant>,
}

impl RateLimit {
    /// The time that requests must still wait, if they must wait.
    pub(crate) fn remaining(&self, now: Instant) -> Option<Duration> {
        self.until
            .and_then(|until| until.checked_duration_since(now))
            .filter(|wait| !wait.is_zero())
    }

    /// Records a 429 and returns the time to wait: the exponential backoff or `retry_after`,
    /// whichever is longer.
    pub(crate) fn on_limited(&mut self, retry_after: Option<Duration>, now: Instant) -> Duration {
        let backoff = BASE_WAIT
            .saturating_mul(2u32.saturating_pow(self.strikes))
            .min(MAX_BACKOFF);
        let asked = retry_after.unwrap_or_default().min(MAX_RETRY_AFTER);
        let wait = backoff.max(asked);
        self.strikes = self.strikes.saturating_add(1);
        // A slower request can bring an older, shorter wait: keep the longest wait.
        let until = now + wait;
        self.until = Some(self.until.map_or(until, |current| current.max(until)));
        wait
    }

    /// A request succeeded: the next 429 starts from the base wait again.
    pub(crate) fn on_success(&mut self) {
        self.strikes = 0;
    }
}

/// The seconds in a `Retry-After` header. Spotify sends seconds. The client does not understand an
/// HTTP date, and then only the backoff applies.
pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    #[test]
    fn waits_double_with_each_429_in_a_row() {
        let now = Instant::now();
        let mut limit = RateLimit::default();
        let waits: Vec<_> = (0..8).map(|_| limit.on_limited(None, now)).collect();
        assert_eq!(
            waits,
            [2, 4, 8, 16, 32, 64, 120, 120].map(secs),
            "doubling up to the cap"
        );
    }

    #[test]
    fn retry_after_is_never_cut_short() {
        let now = Instant::now();
        let mut limit = RateLimit::default();
        assert_eq!(limit.on_limited(Some(secs(7)), now), secs(7));
        // The backoff (4 s) is shorter than the value from Spotify.
        assert_eq!(limit.on_limited(Some(secs(30)), now), secs(30));
        // Spotify asks for less than the backoff (8 s): the backoff applies.
        assert_eq!(limit.on_limited(Some(secs(1)), now), secs(8));
        assert_eq!(limit.on_limited(Some(secs(86_400)), now), secs(3600));
    }

    #[test]
    fn requests_wait_until_the_time_is_up() {
        let now = Instant::now();
        let mut limit = RateLimit::default();
        assert_eq!(limit.remaining(now), None);
        limit.on_limited(Some(secs(10)), now);
        assert_eq!(limit.remaining(now + secs(4)), Some(secs(6)));
        assert_eq!(limit.remaining(now + secs(10)), None);
        assert_eq!(limit.remaining(now + secs(11)), None);
    }

    #[test]
    fn a_shorter_wait_does_not_shorten_the_current_one() {
        let now = Instant::now();
        let mut limit = RateLimit::default();
        limit.on_limited(Some(secs(60)), now);
        limit.on_success();
        limit.on_limited(None, now);
        assert_eq!(limit.remaining(now), Some(secs(60)));
    }

    #[test]
    fn a_success_resets_the_backoff() {
        let now = Instant::now();
        let mut limit = RateLimit::default();
        limit.on_limited(None, now);
        limit.on_limited(None, now);
        limit.on_success();
        assert_eq!(limit.on_limited(None, now + secs(10)), secs(2));
    }

    #[test]
    fn reads_retry_after_in_seconds() {
        assert_eq!(parse_retry_after(" 7 "), Some(secs(7)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    }
}
