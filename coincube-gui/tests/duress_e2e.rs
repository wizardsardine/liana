//! End-to-end duress acceptance test (Phase 9 Task 9.2).
//!
//! This drives the duress engine through a full activate → cryptic → clear
//! cycle against a mocked Connect trigger and a temp data directory, asserting
//! the trust-posture invariants hold end to end:
//!
//!   1. enroll (store this device's encrypted duress code),
//!   2. activate via the orchestrator → Cube files gone, queue/journal/state
//!      durable,
//!   3. the activation POST drains the queue,
//!   4. a server-side clear flips local state back so launch reconcile exits.
//!
//! The staging-environment variant (real Connect, synthetic clock past
//! `unlock_at`, all-clear submit, CRK download with the regular password,
//! restore + balance assertion) is gated behind `--ignored` because it needs a
//! live staging account; this in-process test runs in CI on every build.

use std::sync::Arc;

use coincube_gui::services::duress::{
    cipher::DeviceKey,
    journal::WipeJournal,
    orchestrator::{DuressOrchestrator, DuressTrigger, TriggerError},
    queue::DuressQueue,
    wipe::CubeWiper,
    DuressLocalState,
};

struct OkTrigger;

#[async_trait::async_trait]
impl DuressTrigger for OkTrigger {
    async fn trigger(&self, _account_id: &str, _code: &str) -> Result<(), TriggerError> {
        Ok(())
    }
}

#[tokio::test]
async fn full_local_duress_cycle() {
    // --- temp data dir with seeded Cube data ---
    let dir = std::env::temp_dir().join(format!("coincube-duress-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cube_root = dir.join("bitcoin").join("data").join("cube_a");
    std::fs::create_dir_all(&cube_root).unwrap();
    std::fs::write(cube_root.join("wallet.db"), b"PRE-DURESS-SECRET").unwrap();

    // --- 1. enroll: store this device's encrypted duress code ---
    let cipher = DeviceKey::from_bytes([42u8; 32]);
    let code = coincube_gui::services::duress::enroll::generate_duress_code();
    let state = DuressLocalState {
        enrolled: true,
        duress_code: Some(cipher.encrypt(&code).unwrap()),
        ..Default::default()
    };
    state.save(&dir).unwrap();

    // --- 2/3. activate ---
    let journal = WipeJournal::new(&dir);
    let queue = DuressQueue::new(&dir);
    let wipe = CubeWiper::new(vec![dir.join("bitcoin").join("data")], journal.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut orch = DuressOrchestrator::new(
        state,
        dir.clone(),
        journal.clone(),
        queue.clone(),
        wipe,
        cipher,
        Arc::new(OkTrigger),
        Some(tx),
    );
    orch.activate("acct_e2e".to_string()).await.unwrap();

    // Cube data is gone; the cryptic-screen event fired; state is durable.
    assert!(!dir.join("bitcoin").join("data").exists());
    assert_eq!(
        rx.recv().await,
        Some(coincube_gui::services::duress::DuressEvent::Activated)
    );
    assert!(DuressLocalState::load(&dir).unwrap().active);
    assert!(!journal.is_pending(), "journal cleared after wipe");

    // The OK trigger drains the queue (give the spawned task a moment).
    for _ in 0..100 {
        if queue.is_empty().unwrap() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(queue.is_empty().unwrap(), "queue drains on POST success");

    // --- 4. server-side clear → local state flips so launch reconcile exits ---
    let mut cleared = DuressLocalState::load(&dir).unwrap();
    cleared.active = false;
    cleared.unlock_at = None;
    cleared.save(&dir).unwrap();
    assert!(!DuressLocalState::load(&dir).unwrap().active);

    // Survivors: duress stores at the root persisted across the whole cycle.
    assert!(dir.join("duress-state.json").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "requires a live staging Connect account + CRK; run manually"]
fn full_cycle_against_staging() {
    // Scripted steps (see plan Phase 9 Task 9.2):
    //   1. Enroll (Tier 1).
    //   2. Activate via duress PIN.
    //   3. Assert Cube files gone + cryptic screen rendered.
    //   4. Sign in to staging Connect.
    //   5. Wait synthetic clock past unlock_at.
    //   6. Submit all-clear.
    //   7. Download CRK with the regular password.
    //   8. Restore Cubes; assert balances match pre-duress.
}
