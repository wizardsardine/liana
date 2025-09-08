use std::time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH};

pub mod serde;

#[cfg(test)]
pub mod sandbox;

#[cfg(test)]
pub mod mock;

/// Returns the current time as a [`Duration`] since the UNIX epoch.
pub fn now() -> Duration {
    now_fallible().expect("cannot fail")
}

/// Faliible version of [`now`].
pub fn now_fallible() -> Result<Duration, SystemTimeError> {
    SystemTime::now().duration_since(UNIX_EPOCH)
}
