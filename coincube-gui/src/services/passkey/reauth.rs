//! Re-derive an **open** passkey Cube's master seed from a fresh assertion.
//!
//! Two screens inside an open Cube need the master seed in hand, and both are
//! deliberate, consequential acts rather than incidental reads:
//!
//! * **Cube Recovery Kit** — the Kit must carry the encrypted master seed for a
//!   passkey Cube (**I11**), and the seed for it comes from a re-auth ceremony
//!   at Kit time.
//! * **Settings → Backup Master Seed** — displaying the recovery phrase.
//!
//! # Why re-authenticate at all
//!
//! The signer is already in [`crate::app::session`] — the assertion at unlock
//! put it there, and the Liquid client and the Spark bridge have held a copy
//! ever since. So a second Touch ID prompt is not buying secret hygiene; it is
//! buying **consent**. Exporting your master seed to a server-side Kit, or
//! putting it on screen, is not the same act as opening your wallet, and it
//! should not ride on a biometric you gave ten minutes ago.
//!
//! It is exactly what the PIN path already does: `recovery_kit::verify_pin`
//! re-asks for the PIN and re-decrypts the seed file even though
//! `app::session` is holding both.
//!
//! # Why this is not a subscription
//!
//! `NativePasskeyCeremony` is main-thread-only — it holds retained Objective-C
//! objects, so it is not `Send` and cannot move into a task. Its **result
//! channel** is, though. So the ceremony is started on the main thread and
//! parked in a thread-local ([`LIVE`]) to keep the request alive, and only the
//! receiver crosses into a blocking task that waits on it. The caller gets a
//! plain future and needs no polling subscription of its own.
//!
//! Dropping the parked ceremony cancels the system prompt *and* drops the
//! sender, which is what makes [`cancel`] wake the waiting task.

// Both this import and `CEREMONY_DEADLINE` below are referenced only from the
// `#[cfg(target_os = "macos")]` `begin` — the non-macOS `begin`
// deliberately short-circuits and never starts a ceremony, so there is nothing
// to time out. Ungated, they would be dead code on every other platform, which
// CI's ubuntu `linter` job treats as an error (`-D warnings`).
#[cfg(target_os = "macos")]
use std::time::Duration;

use zeroize::Zeroizing;

use crate::app::settings::CubeSettings;
use crate::services::passkey::PasskeyError;

/// How long the blocking waiter will sit on the channel before giving up.
///
/// The system sheet has no deadline of its own, and a user who walks away
/// without cancelling would otherwise hold a blocking-pool thread for the life
/// of the process. Generous enough that nobody hits it while actually looking
/// at the prompt.
#[cfg(target_os = "macos")]
const CEREMONY_DEADLINE: Duration = Duration::from_secs(180);

#[cfg(target_os = "macos")]
thread_local! {
    /// The in-flight ceremony, parked so its request stays alive after
    /// [`begin`] returns.
    ///
    /// A `thread_local` rather than a global because the value is not `Send`.
    /// That is not a workaround — it is the invariant made mechanical: every
    /// caller runs on iced's main thread, which is the only thread
    /// AuthenticationServices may be driven from, and any other thread simply
    /// sees `None`.
    static LIVE: std::cell::RefCell<
        Option<crate::services::passkey::macos::NativePasskeyCeremony>,
    > = const { std::cell::RefCell::new(None) };
}

/// Cancel and drop any parked ceremony. Idempotent.
///
/// Dropping the sender is what lets [`begin`]'s future resolve, so a caller
/// that abandons a re-auth should call this rather than leaving the waiter to
/// time out.
pub fn cancel() {
    #[cfg(target_os = "macos")]
    LIVE.with(|c| {
        let _ = c.borrow_mut().take();
    });
}

/// Start a re-authentication for `cube` and return a future that resolves to
/// its master seed words.
///
/// The `Result` is split in two on purpose: the outer one reports "the ceremony
/// could not even be started" (no credential recorded, wrong thread), which the
/// caller handles synchronously; the inner one reports how the ceremony ended.
///
/// The returned words are `Zeroizing`-wrapped before they leave the task, so
/// every clone iced makes of the message carrying them is wiped on drop.
#[cfg(target_os = "macos")]
pub fn begin(
    cube: &CubeSettings,
) -> Result<
    impl std::future::Future<Output = Result<Zeroizing<Vec<String>>, PasskeyError>>,
    PasskeyError,
> {
    use base64::Engine;
    use coincube_core::signer::MasterSigner;

    use crate::services::passkey::macos::{NativeOutcome, NativePasskeyCeremony};

    let Some(meta) = cube.passkey_metadata.as_ref() else {
        return Err(PasskeyError::CredentialNotFound(
            "this Cube has no passkey metadata".to_string(),
        ));
    };
    let credential_id = base64::engine::general_purpose::STANDARD
        .decode(&meta.credential_id)
        .map_err(|e| {
            PasskeyError::CredentialNotFound(format!(
                "this Cube's stored credential id is not valid base64: {e}"
            ))
        })?;

    // Any previous ceremony is dropped (and so cancelled) by the replace below,
    // so a second Start can never leave two system sheets stacked.
    let mut ceremony = NativePasskeyCeremony::authenticate(&meta.rp_id, &credential_id)?;
    let receiver = ceremony
        .take_receiver()
        .expect("a freshly started ceremony still owns its result channel");
    LIVE.with(|c| *c.borrow_mut() = Some(ceremony));

    let network = cube.network;
    let expected = cube.master_signer_fingerprint;
    let cube_id = cube.id.clone();

    Ok(async move {
        let outcome =
            tokio::task::spawn_blocking(move || receiver.recv_timeout(CEREMONY_DEADLINE)).await;

        let outcome = match outcome {
            Ok(Ok(outcome)) => outcome,
            // The sender was dropped (the caller cancelled) or the deadline
            // passed. Either way the user did not complete the prompt, which is
            // the same thing as cancelling from their point of view — and per
            // **I12** must never read as a lost Cube.
            Ok(Err(_)) => return Err(PasskeyError::Cancelled),
            Err(e) => return Err(PasskeyError::CeremonyFailed(e.to_string())),
        };

        let prf_output = match outcome {
            NativeOutcome::Authenticated { prf_output } => prf_output,
            NativeOutcome::Failed(e) => return Err(e),
            NativeOutcome::Registered { .. } => {
                return Err(PasskeyError::InvalidResponse(
                    "the authenticator answered a re-authentication with a registration"
                        .to_string(),
                ))
            }
        };

        let signer = MasterSigner::from_prf_output(network, &prf_output).map_err(|e| {
            tracing::error!(%cube_id, error = %e, "passkey re-auth PRF output would not derive");
            PasskeyError::InvalidPrfOutput
        })?;

        // The same guard the unlock screen applies: if another credential
        // answered, its seed is not this Cube's, and backing *it* up would
        // produce a Recovery Kit that restores the wrong wallet — a failure
        // nobody would notice until they tried to recover.
        if let Some(expected) = expected {
            let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
            let derived = signer.fingerprint(&secp);
            if derived != expected {
                tracing::error!(
                    %cube_id, %expected, %derived,
                    "passkey re-auth derived a seed that is not this Cube's"
                );
                return Err(PasskeyError::CredentialNotFound(
                    "the passkey that answered belongs to a different Cube".to_string(),
                ));
            }
        }

        Ok(Zeroizing::new(
            signer.words().into_iter().map(String::from).collect(),
        ))
    })
}

/// Non-macOS: passkey Cubes cannot be created here, so the only way to reach
/// this is a datadir carried across from a Mac. Fail with copy that says the
/// Cube is fine and this OS cannot help — never anything that reads as a lost
/// wallet (**I12**).
#[cfg(not(target_os = "macos"))]
pub fn begin(
    _cube: &CubeSettings,
) -> Result<
    impl std::future::Future<Output = Result<Zeroizing<Vec<String>>, PasskeyError>>,
    PasskeyError,
> {
    Err::<std::future::Ready<Result<Zeroizing<Vec<String>>, PasskeyError>>, _>(
        PasskeyError::CeremonyFailed(
            "COINCUBE supports passkey Cubes on macOS only. Open this Cube on your Mac, \
             or restore it here from your Recovery Kit and its recovery password."
                .to_string(),
        ),
    )
}
