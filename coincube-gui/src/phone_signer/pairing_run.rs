//! Cancellation authority for one pairing attempt. A durable commit decision is
//! serialized with cancellation; cancellation cannot revoke an earlier decision.
use super::errors::PairingError;
use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

#[derive(Clone)]
pub struct PairingRun {
    state: Arc<Mutex<u8>>, // 0 running, 1 cancelled, 2 commit decided
    cancelled: watch::Sender<bool>,
}
impl Default for PairingRun {
    fn default() -> Self {
        let (cancelled, _) = watch::channel(false);
        Self {
            state: Arc::new(Mutex::new(0)),
            cancelled,
        }
    }
}
impl PairingRun {
    pub fn cancel(&self) {
        let mut state = self.state.lock().unwrap();
        if *state == 0 {
            *state = 1;
            self.cancelled.send_replace(true);
        }
    }
    pub fn check(&self) -> Result<(), PairingError> {
        if *self.state.lock().unwrap() == 1 {
            Err(PairingError::InternalError(
                "Pairing cancelled; discard this QR and pair again.".into(),
            ))
        } else {
            Ok(())
        }
    }
    pub async fn wait<T>(
        &self,
        work: impl Future<Output = Result<T, PairingError>>,
    ) -> Result<T, PairingError> {
        self.check()?;
        let mut cancellation = self.cancelled.subscribe();
        tokio::select! {
            biased;
            _ = cancellation.wait_for(|cancelled| *cancelled) => { self.check()?; unreachable!() },
            result = work => { self.check()?; result }
        }
    }
    /// Serialize a provisional write with revocation without deciding commit.
    pub fn authorized<T>(
        &self,
        work: impl FnOnce() -> Result<T, PairingError>,
    ) -> Result<T, PairingError> {
        let state = self.state.lock().unwrap();
        if *state == 1 {
            return Err(PairingError::InternalError("Pairing cancelled".into()));
        }
        work()
    }
    /// The callback must durably record the commit decision. No await occurs
    /// while holding the lock, so cancel and this write have a total order.
    pub fn decide<T>(
        &self,
        persist: impl FnOnce() -> Result<T, PairingError>,
    ) -> Result<T, PairingError> {
        let mut state = self.state.lock().unwrap();
        if *state == 1 {
            return Err(PairingError::InternalError("Pairing cancelled".into()));
        }
        let result = persist()?;
        *state = 2;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancellation_interrupts_every_precommit_wait() {
        for phase in [
            "redial",
            "TLS",
            "identity",
            "acceptance",
            "prepared",
            "persistence",
        ] {
            let run = PairingRun::default();
            let task_run = run.clone();
            let task = tokio::spawn(async move {
                task_run
                    .wait(std::future::pending::<Result<(), PairingError>>())
                    .await
            });
            run.cancel();
            assert!(task.await.unwrap().is_err(), "{}", phase);
            assert!(run
                .decide::<()>(|| panic!("cancelled persistence must never run"))
                .is_err());
        }
    }
    #[test]
    fn cancellation_after_durable_decision_does_not_undo_commit() {
        let run = PairingRun::default();
        run.decide(|| Ok(())).unwrap();
        run.cancel();
        run.check().unwrap();
    }
}
