use std::time::Duration;

pub const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackoffPolicy {
    next: Duration,
}

impl BackoffPolicy {
    pub fn new() -> Self {
        Self {
            next: BACKOFF_INITIAL,
        }
    }

    pub fn advance(&mut self) -> Duration {
        let current = self.next;
        self.next = (self.next * 2).min(BACKOFF_MAX);
        current
    }

    pub fn reset(&mut self) {
        self.next = BACKOFF_INITIAL;
    }
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn advance_doubles_until_max() {
        let mut backoff = BackoffPolicy::new();

        let delays: Vec<_> = (0..7).map(|_| backoff.advance()).collect();

        assert_eq!(
            delays,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(30),
                Duration::from_secs(30),
            ]
        );
    }

    #[test]
    fn reset_returns_to_initial_delay() {
        let mut backoff = BackoffPolicy::new();

        let _ = backoff.advance();
        let _ = backoff.advance();
        backoff.reset();

        assert_eq!(backoff.advance(), BACKOFF_INITIAL);
    }

    proptest! {
        #[test]
        fn advance_never_exceeds_max(steps in 0usize..256) {
            let mut backoff = BackoffPolicy::new();

            for _ in 0..steps {
                prop_assert!(backoff.advance() <= BACKOFF_MAX);
            }
        }
    }
}
