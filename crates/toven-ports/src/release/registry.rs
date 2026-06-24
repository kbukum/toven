//! Value-only helpers shared by semver registry release targets.

use std::time::{Duration, SystemTime};

/// Fallback retry cadence for a semver registry.
///
/// The adapter owns registry I/O and response parsing; this helper only carries
/// generic timing policy that multiple semver registries can share.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct RegistryCadence {
    /// Fallback delay when publishing a crate/package name for the first time.
    pub new_release: Duration,
    /// Fallback delay when publishing another version of an existing name.
    pub update: Duration,
}

impl RegistryCadence {
    /// Construct a fallback cadence.
    #[must_use]
    pub const fn new(new_release: Duration, update: Duration) -> Self {
        Self {
            new_release,
            update,
        }
    }

    /// Select the fallback delay for a publish attempt.
    #[must_use]
    pub const fn fallback_delay(self, is_new_release: bool) -> Duration {
        if is_new_release {
            self.new_release
        } else {
            self.update
        }
    }

    /// Convert the fallback delay into a concrete retry time.
    #[must_use]
    pub fn fallback_retry_after(self, is_new_release: bool, now: SystemTime) -> Option<SystemTime> {
        now.checked_add(self.fallback_delay(is_new_release))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::RegistryCadence;

    #[test]
    fn cadence_selects_new_release_or_update_delay() {
        let cadence = RegistryCadence::new(Duration::from_mins(10), Duration::from_mins(1));

        assert_eq!(cadence.fallback_delay(true), Duration::from_mins(10));
        assert_eq!(cadence.fallback_delay(false), Duration::from_mins(1));
    }

    #[test]
    fn fallback_retry_after_offsets_from_now() {
        let now = SystemTime::UNIX_EPOCH;
        let cadence = RegistryCadence::new(Duration::from_mins(10), Duration::from_mins(1));

        assert_eq!(
            cadence.fallback_retry_after(true, now),
            Some(now + Duration::from_mins(10))
        );
        assert_eq!(
            cadence.fallback_retry_after(false, now),
            Some(now + Duration::from_mins(1))
        );
    }
}
