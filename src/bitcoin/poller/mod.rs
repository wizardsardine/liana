mod looper;

use crate::{
    bitcoin::{poller::looper::looper, BitcoinInterface},
    database::DatabaseInterface,
    descriptors,
};

use std::{
    sync::{self, atomic},
    thread, time,
};

/// The Bitcoin poller handler.
pub struct Poller {
    handle: thread::JoinHandle<()>,
    shutdown: sync::Arc<atomic::AtomicBool>,
}

impl Poller {
    pub fn start(
        bit: sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
        db: sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
        poll_interval: time::Duration,
        desc: descriptors::LianaDescriptor,
    ) -> Poller {
        let shutdown = sync::Arc::from(atomic::AtomicBool::from(false));
        let handle = thread::Builder::new()
            .name("Bitcoin poller".to_string())
            .spawn({
                let shutdown = shutdown.clone();
                move || looper(bit, db, shutdown, poll_interval, desc)
            })
            .expect("Must not fail");

        Poller { shutdown, handle }
    }

    pub fn stop(self) {
        self.shutdown.store(true, atomic::Ordering::Relaxed);
        self.handle.join().expect("The poller loop must not fail");
    }

    #[cfg(test)]
    pub fn test_stop(&mut self) {
        self.shutdown.store(true, atomic::Ordering::Relaxed);
    }
}
