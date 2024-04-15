use chrono::prelude::*;

#[cfg(test)]
pub mod mock_time {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static MOCK_TIME: RefCell<Option<DateTime<Utc>>> = RefCell::new(None);
    }

    pub fn now() -> DateTime<Utc> {
        MOCK_TIME.with(|cell| cell.borrow().as_ref().cloned().unwrap_or_else(Utc::now))
    }

    pub fn set_mock_time(time: DateTime<Utc>) {
        MOCK_TIME.with(|cell| *cell.borrow_mut() = Some(time));
    }

    pub fn clear_mock_time() {
        MOCK_TIME.with(|cell| *cell.borrow_mut() = None);
    }
}

#[cfg(test)]
pub use mock_time::now;

#[cfg(not(test))]
pub fn now() -> DateTime<Utc> {
    Utc::now()
}
