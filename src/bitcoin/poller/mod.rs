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

    pub fn trigger_stop(&self) {
        self.shutdown.store(true, atomic::Ordering::Relaxed);
    }

    pub fn stop(self) {
        self.trigger_stop();
        self.handle.join().expect("The poller loop must not fail");
    }

    #[cfg(feature = "nonblocking_shutdown")]
    pub fn is_stopped(&self) -> bool {
        // Doc says "This might return true for a brief moment after the thread’s main function has
        // returned, but before the thread itself has stopped running.". But it's not an issue for
        // us, as long as the main poller function has returned we are good.
        self.handle.is_finished()
    }

    #[cfg(test)]
    pub fn test_stop(&mut self) {
        self.shutdown.store(true, atomic::Ordering::Relaxed);
    }
}
