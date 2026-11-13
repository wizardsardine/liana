//! Backoff for polling loops, vendored from the `bwk-backoff` crate.

use std::{thread, time::Duration};

pub struct Backoff {
    current_step: u32,
    max_sleep: Duration,
}

impl Backoff {
    pub fn new_ms(max_sleep: u64) -> Self {
        Self {
            current_step: 0,
            max_sleep: Duration::from_millis(max_sleep),
        }
    }

    /// Resets the backoff when work is done.
    pub fn reset(&mut self) {
        self.current_step = 0;
    }

    /// Yields for the first 10 steps, then sleeps exponentially longer, up to `max_sleep`.
    pub fn snooze(&mut self) {
        if self.current_step < 10 {
            thread::yield_now();
        } else {
            // base sleep = 2^(n - 10) * 10μs, plus up to 50% jitter
            let base = 1u64 << (self.current_step - 10).min(10);
            let sleep_micros = base * 10;
            let jitter = getrandom::u64().unwrap_or(0) % (sleep_micros / 2);
            let total_sleep = Duration::from_micros(
                (sleep_micros + jitter).min(self.max_sleep.as_micros() as u64),
            );
            thread::sleep(total_sleep);
        }

        self.current_step = self.current_step.saturating_add(1);
    }
}
