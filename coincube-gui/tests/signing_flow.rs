//! End-to-end signing-flow integration tests.
//!
//! This file is a **placeholder**: a true E2E happy-path test requires a
//! phone-side mock (or CLI) that can approve a session and submit a
//! partial signature against the dev API. We don't have that fixture
//! yet, so the tests below are currently skeletons that document the
//! expected shape.
//!
//! When the fixture lands:
//!
//! 1. `create_signing_session_round_trip` — desktop creates a session,
//!    the mock approves + signs, desktop receives the SIGNATURE_SUBMITTED
//!    event, fetches the session, decodes the signed PSBT, and verifies
//!    the signature is present at the expected `bip32_derivation` slot.
//! 2. `cancel_signing_session` — desktop creates a session, mock waits,
//!    desktop cancels, mock observes SESSION_CANCELLED.
//! 3. `session_expired` — desktop creates a session with a 1s TTL, no
//!    one signs, mock observes SESSION_EXPIRED.

#![cfg(feature = "integration-tests")]

#[tokio::test]
async fn create_signing_session_round_trip() {
    eprintln!(
        "Skipping: requires phone-side mock to submit partial signatures. \
         See `plans/PLAN-phase-4-production-polish.md` PR 7 notes."
    );
}

#[tokio::test]
async fn cancel_signing_session() {
    eprintln!(
        "Skipping: requires phone-side mock to observe SESSION_CANCELLED. \
         See `plans/PLAN-phase-4-production-polish.md` PR 7 notes."
    );
}

#[tokio::test]
async fn session_expired() {
    eprintln!(
        "Skipping: requires phone-side mock to observe SESSION_EXPIRED. \
         See `plans/PLAN-phase-4-production-polish.md` PR 7 notes."
    );
}
