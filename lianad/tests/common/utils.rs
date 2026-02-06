use std::thread::sleep;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(30);

pub fn wait_for_while_condition_holds<S, C>(success: S, condition: C) -> bool
where
    S: Fn() -> bool,
    C: Fn() -> bool,
{
    let start = Instant::now();
    let mut interval = Duration::from_millis(250);

    loop {
        if start.elapsed() > TIMEOUT {
            return false;
        }

        if !condition() {
            return false;
        }

        if success() {
            return true;
        }

        sleep(interval);
        interval = (interval * 2).min(Duration::from_secs(5));
    }
}

pub fn wait_for<S>(success: S) -> bool
where
    S: Fn() -> bool,
{
    wait_for_while_condition_holds(success, || true)
}
