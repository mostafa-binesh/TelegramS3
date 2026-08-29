use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    RetryAfter(Duration),
    RespectFloodWait(Duration),
    GiveUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_backoff: Duration,
    pub respect_flood_wait: bool,
}

impl RetryPolicy {
    pub fn new(max_attempts: u32, base_backoff: Duration, respect_flood_wait: bool) -> Self {
        Self {
            max_attempts: max_attempts.max(1),
            base_backoff,
            respect_flood_wait,
        }
    }

    pub fn backoff_for_attempt(&self, attempt: u32) -> Duration {
        if attempt <= 1 {
            return self.base_backoff;
        }

        let multiplier = 1u128 << attempt.saturating_sub(1).min(20);
        let millis = self.base_backoff.as_millis().saturating_mul(multiplier);
        let capped = millis.min(Duration::from_secs(10).as_millis());
        Duration::from_millis(capped as u64)
    }

    pub fn retry_decision(&self, attempt: u32, flood_wait_seconds: Option<u64>) -> RetryDecision {
        if let Some(seconds) = flood_wait_seconds
            && self.respect_flood_wait
        {
            return RetryDecision::RespectFloodWait(Duration::from_secs(seconds.max(1)));
        }

        if attempt >= self.max_attempts {
            RetryDecision::GiveUp
        } else {
            RetryDecision::RetryAfter(self.backoff_for_attempt(attempt + 1))
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new(5, Duration::from_millis(500), true)
    }
}

pub fn parse_flood_wait_seconds(error: impl AsRef<str>) -> Option<u64> {
    let error = error.as_ref();

    if let Some(index) = error.find("FLOOD_WAIT_") {
        let start = index + "FLOOD_WAIT_".len();
        let digits: String = error[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        if let Ok(value) = digits.parse::<u64>() {
            return Some(value);
        }
    }

    if let Some(start) = error.find("(value: ") {
        let tail = &error[start + "(value: ".len()..];
        let digits: String = tail.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        if let Ok(value) = digits.parse::<u64>() {
            return Some(value);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flood_wait_variants() {
        assert_eq!(parse_flood_wait_seconds("FLOOD_WAIT_17"), Some(17));
        assert_eq!(parse_flood_wait_seconds("rpc error (value: 42)"), Some(42));
        assert_eq!(parse_flood_wait_seconds("no flood wait"), None);
    }

    #[test]
    fn calculates_progressive_backoff() {
        let policy = RetryPolicy::new(5, Duration::from_millis(500), true);
        assert_eq!(policy.backoff_for_attempt(1), Duration::from_millis(500));
        assert_eq!(policy.backoff_for_attempt(2), Duration::from_millis(1_000));
        assert_eq!(policy.backoff_for_attempt(3), Duration::from_millis(2_000));
    }
}
