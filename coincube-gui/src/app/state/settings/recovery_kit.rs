//! Cube Recovery Kit state machine + message handler.
//!
//! The state is held as a sub-struct on `GeneralSettingsState` so that
//! the existing local-paper-backup flow (`BackupSeedState`) coexists
//! untouched — the two features live side-by-side per the plan (§1).
//!
//! Dispatch uses the same pattern as `state::connect::cube_members`:
//! the handler is a free function, and the parent (App-level) injects
//! the authenticated `CoincubeClient`, the Connect-numeric cube id,
//! and the live `Wallet` because those are not reachable from the
//! generic `State::update` trait signature.
//!
//! See `plans/PLAN-cube-recovery-kit-desktop.md` §2.3 (state machine)
//! and §2.4 (card rendering).

use std::sync::Arc;

use coincube_core::miniscript::bitcoin::Network;
use iced::Task;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::app::cache::Cache;
use crate::app::message::Message;
use crate::app::settings::{self, update_settings_file};
use crate::app::view;
use crate::app::view::settings::general::backup_overview;
use crate::app::view::{
    PhoneKeyOutcome, RecoveryKitMessage, RecoveryKitMode, RecoveryKitUploadOutcome,
    RecoveryProtectionMode, RemoveFailure,
};
use crate::app::wallet::Wallet;
use crate::feature_flags;
use crate::pin_input::PinInput;
use crate::services::coincube::{
    CoincubeClient, CoincubeError, RecoveryKit as ApiRecoveryKit, RecoveryKitStatus,
    RECOVERY_KIT_SCHEME_AES_256_GCM,
};
use crate::services::inheritance::{
    find_owner_self_recipient, seal_and_upload_owner_self, OwnerSelfError,
};
use crate::services::recovery::{
    self, DescriptorBlob, DescriptorBlobCube, DescriptorBlobSigner, DescriptorBlobVault, KdfParams,
    SeedBlob, SeedBlobCube, SeedBlobMnemonic, BLOB_VERSION, MIN_PASSWORD_LEN,
};

/// Whether this cube's seed is extractable on-device (mnemonic cubes
/// can extract via PIN; passkey cubes cannot — their seed derives from
/// a WebAuthn PRF re-ceremony). Drives whether PIN entry is part of
/// the flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedSource {
    Mnemonic,
    Passkey,
}

/// Snapshot of the user's `PasswordEntry` inputs at the moment they
/// hit Submit. Carried through `Uploading` so a transient upload
/// failure can restore the entry screen without making the user
/// re-type their password (and, for mnemonic cubes, re-enter their
/// PIN to re-decrypt the mnemonic).
///
/// `Debug` is manual — the derived version would deref through
/// `Zeroizing<String>` / `Zeroizing<Vec<String>>` and print the
/// password and mnemonic words in plaintext to any `{:?}` site,
/// which defeats the whole reason those fields are Zeroizing-wrapped
/// in the first place. Keep the manual impl in sync with the fields.
pub struct PasswordEntryPending {
    pub mode: RecoveryKitMode,
    pub mnemonic: Option<Zeroizing<Vec<String>>>,
    pub password: Zeroizing<String>,
    pub confirm: Zeroizing<String>,
    pub acknowledged: bool,
}

impl std::fmt::Debug for PasswordEntryPending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordEntryPending")
            .field("mode", &self.mode)
            .field("mnemonic", &self.mnemonic.as_ref().map(|_| "<redacted>"))
            .field("password", &"<redacted>")
            .field("confirm", &"<redacted>")
            .field("acknowledged", &self.acknowledged)
            .finish()
    }
}

/// Auto-detection status of the owner's `owner-self` Keychain recovery key,
/// shown on the protection-choice screen. The check runs automatically on
/// entry (and via the "Check again" link); its result gates the phone options —
/// "Use my phone" / "Use both" are only selectable while `Present`. Registration
/// is phone-initiated (COIN-390): the desktop only *detects* whether the
/// Keychain app has minted + registered the key yet.
#[derive(Debug, Clone)]
pub enum PhoneKey {
    /// Detection request in flight.
    Checking,
    /// A registered `owner-self` key exists — phone options are enabled.
    Present,
    /// No key registered yet — phone options disabled, with guidance to create
    /// one in the Keychain app under the same Connect account.
    Absent,
    /// Detection failed (network / auth / not signed in). Shown inline; the
    /// "Check again" link lets the user retry.
    Error(String),
}

/// The actual UI state of the wizard. `None` = card view; any other
/// variant = wizard is taking over the settings page.
///
/// `Debug` is manual — the `PasswordEntry` variant holds the live
/// password/confirm inputs and (for mnemonic cubes) the decrypted
/// mnemonic; a derived `Debug` would print all three in plaintext.
/// `Uploading` is also manual because it carries `PasswordEntryPending`
/// which has its own redacting impl we want to delegate to. Other
/// variants are non-sensitive and get straightforward formatting.
pub enum RecoveryKitState {
    None,
    PinEntry {
        mode: RecoveryKitMode,
        error: Option<String>,
    },
    /// Protection-mode choice (PLAN-owner-keychain-recovery PR 2). Only reached
    /// when `OWNER_KEYCHAIN_RECOVERY_ENABLED`; with the flag off the wizard goes
    /// straight to `PasswordEntry` as before. Carries the unlocked mnemonic
    /// (mnemonic cubes) so the phone seal can build the seed blob without
    /// re-asking for the PIN. `Debug` is manual to redact the mnemonic.
    ProtectionChoice {
        mode: RecoveryKitMode,
        mnemonic: Option<Zeroizing<Vec<String>>>,
        /// Auto-detection status of the owner's phone (`owner-self`) key. Gates
        /// whether the phone/both options are selectable.
        phone: PhoneKey,
        /// Seal-preflight error (e.g. a settings read failing after the user
        /// picks Phone/Both). Distinct from a `PhoneKey::Error` detection
        /// failure — this is a later-stage error surfaced inline.
        error: Option<String>,
    },
    /// In-flight phone seal+upload (PR 2). Reached from `ProtectionChoice` (phone
    /// only) or chained after a password upload (the "Both" mode).
    PhoneSealing {
        mode: RecoveryKitMode,
    },
    PasswordEntry {
        mode: RecoveryKitMode,
        /// Present for mnemonic cubes once PIN has been verified. `None`
        /// for passkey cubes (descriptor-only upload).
        mnemonic: Option<Zeroizing<Vec<String>>>,
        password: Zeroizing<String>,
        confirm: Zeroizing<String>,
        /// "I've written this down" gate — Submit is inert until true.
        acknowledged: bool,
        error: Option<String>,
    },
    /// In-flight upload. Carries the pending `PasswordEntry` snapshot
    /// so we can restore the entry screen intact if the upload fails —
    /// previously the user had to re-type their password (and PIN) on
    /// every transient network error.
    Uploading {
        pending: PasswordEntryPending,
    },
    Completed {
        updated_at: String,
        now_has_seed: bool,
        now_has_descriptor: bool,
    },
    /// Full-page confirmation before any delete happens (master F5). Lists which
    /// backup paths will be torn down (derived from the current `rk.status` in
    /// the view) and waits for an explicit `ConfirmRemove`. No payload — the
    /// delete set is rebuilt from `rk.status` at confirm time.
    ConfirmRemove,
    Removing,
    Error {
        message: String,
    },
}

impl std::fmt::Debug for RecoveryKitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::PinEntry { mode, error } => f
                .debug_struct("PinEntry")
                .field("mode", mode)
                .field("error", error)
                .finish(),
            Self::ProtectionChoice {
                mode,
                mnemonic,
                phone,
                error,
            } => f
                .debug_struct("ProtectionChoice")
                .field("mode", mode)
                .field("mnemonic", &mnemonic.as_ref().map(|_| "<redacted>"))
                .field("phone", phone)
                .field("error", error)
                .finish(),
            Self::PhoneSealing { mode } => {
                f.debug_struct("PhoneSealing").field("mode", mode).finish()
            }
            Self::PasswordEntry {
                mode,
                mnemonic,
                acknowledged,
                error,
                // `password` and `confirm` intentionally omitted
                // from the Debug output — printing them would
                // leak the live recovery password. The `..`
                // pattern keeps this impl compilable if fields
                // are added later, but the "add new field" path
                // should deliberately decide whether to expose it.
                ..
            } => f
                .debug_struct("PasswordEntry")
                .field("mode", mode)
                .field("mnemonic", &mnemonic.as_ref().map(|_| "<redacted>"))
                .field("password", &"<redacted>")
                .field("confirm", &"<redacted>")
                .field("acknowledged", acknowledged)
                .field("error", error)
                .finish(),
            Self::Uploading { pending } => f
                .debug_struct("Uploading")
                // `pending`'s own Debug is redacting — delegate.
                .field("pending", pending)
                .finish(),
            Self::Completed {
                updated_at,
                now_has_seed,
                now_has_descriptor,
            } => f
                .debug_struct("Completed")
                .field("updated_at", updated_at)
                .field("now_has_seed", now_has_seed)
                .field("now_has_descriptor", now_has_descriptor)
                .finish(),
            Self::ConfirmRemove => write!(f, "ConfirmRemove"),
            Self::Removing => write!(f, "Removing"),
            Self::Error { message } => f.debug_struct("Error").field("message", message).finish(),
        }
    }
}

/// Top-level container held on `GeneralSettingsState`. Separates the
/// Recovery-Kit concern from the local-paper-backup state so each has
/// its own independent lifecycle.
pub struct RecoveryKit {
    /// Last-known status from Connect. Drives the card copy. `None`
    /// until the first `LoadStatus` fires and resolves.
    pub status: Option<RecoveryKitStatus>,
    /// True while a `LoadStatus` round-trip is in flight. Card can
    /// show a skeleton while this is set.
    pub status_loading: bool,
    /// Wizard state; `None` when the card is visible.
    pub flow: RecoveryKitState,
    /// PIN widget — separate from `backup_pin` so the two flows can
    /// be active at once without stepping on each other (in practice
    /// they won't be, but keeping them isolated avoids accidental
    /// coupling).
    pub pin: PinInput,
    /// One-shot flag: when the next `StatusLoaded` message resolves
    /// (regardless of which code path fired the corresponding
    /// `LoadStatus`), emit the post-vault-creation nudge toast iff
    /// the loaded status indicates the user doesn't yet have a
    /// descriptor backed up. Set by the W10 code path in
    /// `App::update`; cleared on handle so subsequent Settings-page
    /// entries don't re-nag.
    pub nudge_on_next_status_load: bool,
    /// "Both" protection mode (PR 2): after a successful password upload, also
    /// seal the same material to the owner's phone. Set when the user picks
    /// `RecoveryProtectionMode::Both`; consumed in the `UploadResult` handler;
    /// cleared on `reset_flow`.
    pub also_phone_seal: bool,
}

impl RecoveryKit {
    pub fn new() -> Self {
        Self {
            status: None,
            status_loading: false,
            flow: RecoveryKitState::None,
            pin: PinInput::new(),
            nudge_on_next_status_load: false,
            also_phone_seal: false,
        }
    }

    /// Drop any in-flight flow state (PIN, password, decrypted
    /// mnemonic). Called on Cancel and on wizard completion.
    pub fn reset_flow(&mut self) {
        self.flow = RecoveryKitState::None;
        self.pin.clear();
        self.also_phone_seal = false;
    }
}

impl Default for RecoveryKit {
    fn default() -> Self {
        Self::new()
    }
}

/// Entry point. Intended to be called from `App::update` for every
/// `SettingsMessage::RecoveryKit(_)` it sees. The caller injects the
/// authenticated Connect client, the numeric cube id, and the current
/// Wallet (if any) — these aren't reachable from the generic
/// `State::update` trait signature.
#[allow(clippy::too_many_arguments)]
pub fn update(
    rk: &mut RecoveryKit,
    msg: RecoveryKitMessage,
    cache: &Cache,
    local_cube_id: &str,
    seed_source: SeedSource,
    client: Option<CoincubeClient>,
    server_cube_id: Option<u64>,
    wallet: Option<Arc<Wallet>>,
) -> Task<Message> {
    match msg {
        RecoveryKitMessage::LoadStatus => load_status(rk, client, server_cube_id),

        RecoveryKitMessage::StatusLoaded(res) => {
            rk.status_loading = false;
            match res {
                Ok(Some(s)) => {
                    rk.status = Some(s);
                }
                Ok(None) => {
                    // 404 — no kit on the server; render card in
                    // "Create" state.
                    rk.status = Some(RecoveryKitStatus {
                        has_recovery_kit: false,
                        has_encrypted_seed: false,
                        has_encrypted_wallet_descriptor: false,
                        encryption_scheme: String::new(),
                        created_at: None,
                        updated_at: None,
                        owner_self: None,
                    });
                }
                Err(e) => {
                    tracing::warn!("get_recovery_kit_status failed: {}", e);
                    // Keep the prior cached status (if any) so the
                    // card doesn't flicker; surface a toast only —
                    // no state transition. Also clear the nudge
                    // flag — we don't want to defer the toast
                    // indefinitely because of a transient error.
                    rk.nudge_on_next_status_load = false;
                    return Task::done(Message::View(view::Message::ShowError(format!(
                        "Couldn't load Recovery Kit status: {}",
                        e
                    ))));
                }
            }
            // W10 post-vault-creation nudge: fires only against a
            // freshly-loaded status so we don't nag users whose kit
            // already has a descriptor backed up. One-shot flag is
            // cleared here regardless of outcome.
            if rk.nudge_on_next_status_load {
                rk.nudge_on_next_status_load = false;
                if should_nudge_for_status(rk.status.as_ref()) {
                    return Task::done(Message::View(view::Message::ShowToast(
                        log::Level::Info,
                        "Your new Vault is ready — back up your Wallet Descriptor in \
                         Settings → General → Cube Recovery Kit."
                            .to_string(),
                    )));
                }
            }
            Task::none()
        }

        RecoveryKitMessage::Start(mode) => {
            rk.pin.clear();
            // Gate the whole wizard on Connect auth + cube
            // registration *before* we collect anything sensitive
            // (PIN, password). Without those, the final upload in
            // `submit_password` would always fail — and the user
            // would've re-entered their PIN and typed a password
            // for nothing. Fail fast and point them at sign-in.
            match start_guard(client.as_ref(), server_cube_id) {
                StartGuard::Ok => {}
                StartGuard::NotSignedIn => {
                    rk.flow = RecoveryKitState::Error {
                        message: "Sign in to Connect to back up your Recovery Kit. \
                                  You can sign in from Home → Sign In."
                            .to_string(),
                    };
                    return Task::none();
                }
                StartGuard::CubeNotRegistered => {
                    rk.flow = RecoveryKitState::Error {
                        message: "This Cube isn't registered with Connect yet. \
                                  Open the Connect panel to finish setup, then try again."
                            .to_string(),
                    };
                    return Task::none();
                }
            }
            match seed_source {
                SeedSource::Mnemonic => {
                    rk.flow = RecoveryKitState::PinEntry { mode, error: None };
                }
                SeedSource::Passkey => {
                    // Passkey cubes skip PIN entry — the seed is not
                    // extractable on-device; we can only upload the
                    // descriptor blob. Guard: a descriptor must exist.
                    if wallet.is_none() {
                        rk.flow = RecoveryKitState::Error {
                            message: "Create a Vault first — a passkey Cube can only back up its \
                                      Wallet Descriptor, and there's no Vault yet."
                                .to_string(),
                        };
                        return Task::none();
                    }
                    // Passkey cubes have no on-device seed, so there's no PIN
                    // gate; offer the protection-mode choice (flag on, with the
                    // phone key auto-detected on entry) or jump straight to the
                    // password entry (flag off).
                    return enter_after_seed_unlock(rk, mode, None, client, server_cube_id);
                }
            }
            Task::none()
        }

        RecoveryKitMessage::Cancel => {
            rk.reset_flow();
            Task::none()
        }

        RecoveryKitMessage::PinInput(pin_msg) => {
            if let RecoveryKitState::PinEntry { error, .. } = &mut rk.flow {
                *error = None;
            }
            rk.pin.update(pin_msg).map(|m| {
                Message::View(view::Message::Settings(view::SettingsMessage::RecoveryKit(
                    RecoveryKitMessage::PinInput(m),
                )))
            })
        }

        RecoveryKitMessage::VerifyPin => verify_pin(rk, cache, local_cube_id),

        RecoveryKitMessage::PinVerified(res) => {
            rk.pin.clear();
            let mode = match &rk.flow {
                RecoveryKitState::PinEntry { mode, .. } => *mode,
                _ => return Task::none(),
            };
            match res {
                Ok(words) => {
                    // `words: Zeroizing<Vec<String>>` — the wrap
                    // already happened at the async boundary in
                    // `verify_pin`'s task. Just move it into state. When the
                    // owner-keychain flag is on, the seed is now unlocked so we
                    // offer the protection-mode choice (PR 2), auto-detecting the
                    // phone key on entry; otherwise go straight to the password
                    // entry as before.
                    enter_after_seed_unlock(rk, mode, Some(words), client, server_cube_id)
                }
                Err(e) => {
                    rk.flow = RecoveryKitState::PinEntry {
                        mode,
                        error: Some(e),
                    };
                    Task::none()
                }
            }
        }

        RecoveryKitMessage::PasswordChanged(value) => {
            if let RecoveryKitState::PasswordEntry {
                password, error, ..
            } = &mut rk.flow
            {
                // `value` is already `Zeroizing<String>` from the
                // message payload — move it straight in. The
                // previous `password` drops here, wiping its heap.
                *password = value;
                *error = None;
            }
            Task::none()
        }

        RecoveryKitMessage::ConfirmChanged(value) => {
            if let RecoveryKitState::PasswordEntry { confirm, error, .. } = &mut rk.flow {
                *confirm = value;
                *error = None;
            }
            Task::none()
        }

        RecoveryKitMessage::AcknowledgeToggled(checked) => {
            if let RecoveryKitState::PasswordEntry { acknowledged, .. } = &mut rk.flow {
                *acknowledged = checked;
            }
            Task::none()
        }

        RecoveryKitMessage::SelectProtectionMode(protection) => {
            // Pull mode + the unlocked mnemonic out of the choice screen (ends
            // the borrow before we reassign `rk.flow`).
            let (mode, mnemonic) = match &mut rk.flow {
                RecoveryKitState::ProtectionChoice { mode, mnemonic, .. } => {
                    (*mode, mnemonic.take())
                }
                _ => return Task::none(),
            };
            match protection {
                RecoveryProtectionMode::Password => {
                    rk.also_phone_seal = false;
                    rk.flow = password_entry(mode, mnemonic);
                    Task::none()
                }
                RecoveryProtectionMode::Both => {
                    // Run the password path now; chain the phone seal after the
                    // upload succeeds (see the `UploadResult` handler).
                    rk.also_phone_seal = true;
                    rk.flow = password_entry(mode, mnemonic);
                    Task::none()
                }
                RecoveryProtectionMode::Phone => {
                    rk.also_phone_seal = false;
                    start_phone_seal(
                        rk,
                        cache,
                        local_cube_id,
                        client,
                        server_cube_id,
                        wallet,
                        mode,
                        mnemonic,
                    )
                }
            }
        }

        RecoveryKitMessage::ProvisionPhone => {
            // Re-run detection (the "Check again" link). Two guards:
            //   * Off the choice screen → no-op.
            //   * Already `Checking` → no-op. Dropping a duplicate request
            //     (e.g. a double-click) keeps a single detection outstanding,
            //     so a late `ProvisionResult` can't be a stale duplicate that
            //     clobbers a newer one. Results that arrive after the user has
            //     left the choice screen are already discarded by
            //     `set_phone_detection` (the same flow guard `PhoneSealResult`
            //     uses). `begin_phone_detection` sets `PhoneKey::Checking` and
            //     handles the not-signed-in / unregistered cases inline.
            match &rk.flow {
                RecoveryKitState::ProtectionChoice {
                    phone: PhoneKey::Checking,
                    ..
                } => Task::none(),
                RecoveryKitState::ProtectionChoice { .. } => {
                    begin_phone_detection(rk, client, server_cube_id)
                }
                _ => Task::none(),
            }
        }

        RecoveryKitMessage::ProvisionResult(outcome) => {
            let status = match outcome {
                // The `owner-self` recipient exists; the phone/both options unlock.
                PhoneKeyOutcome::Present => PhoneKey::Present,
                // No key yet — show guidance rather than an error.
                PhoneKeyOutcome::Absent => PhoneKey::Absent,
                PhoneKeyOutcome::Failed(e) => PhoneKey::Error(e),
            };
            set_phone_detection(rk, status);
            Task::none()
        }

        RecoveryKitMessage::PhoneSealResult(res) => {
            // Ignore a stale result: the seal runs async, so the user may have
            // cancelled or started a new flow while it was in flight. Only a
            // wizard still in `PhoneSealing` should react — otherwise a late
            // result would show a false "backed up" toast or replace an
            // unrelated in-progress wizard with an error screen. Mirrors the
            // `ProvisionResult` staleness guard.
            if !matches!(rk.flow, RecoveryKitState::PhoneSealing { .. }) {
                return Task::none();
            }
            match res {
                Ok(descriptor_fingerprint) => {
                    rk.reset_flow();
                    // Persist the keychain drift slot from the descriptor that was
                    // actually sealed (per-method drift, PR 3). Mirror
                    // `next_fingerprint_to_persist`'s rule: only a seal that
                    // included a descriptor writes the slot — never wipe a stored
                    // fingerprint on a descriptor-less seal (the seal path always
                    // seals a descriptor today, so this is normally `Some`).
                    let persist = if let Some(fp) = descriptor_fingerprint {
                        persist_descriptor_fingerprint(
                            cache,
                            local_cube_id,
                            DescriptorFingerprintSlot::Keychain,
                            Some(fp),
                        )
                    } else {
                        Task::none()
                    };
                    let reload = load_status(rk, client, server_cube_id);
                    let toast = Task::done(Message::View(view::Message::ShowToast(
                        log::Level::Info,
                        "Backed up to your phone. You can restore this Cube by approving on \
                         your Keychain."
                            .to_string(),
                    )));
                    Task::batch([toast, persist, reload])
                }
                Err(e) => {
                    rk.flow = RecoveryKitState::Error { message: e.clone() };
                    // "Both" mode chains this seal *after* a successful password
                    // upload, so the kit on Connect has already changed even though
                    // the phone half failed. Refresh status now (it only updates
                    // `rk.status`, not the `Error` flow) so the inline card reflects
                    // the real server state once the user dismisses the error —
                    // otherwise it keeps showing the pre-upload cache.
                    let reload = load_status(rk, client, server_cube_id);
                    Task::batch([
                        Task::done(Message::View(view::Message::ShowError(e))),
                        reload,
                    ])
                }
            }
        }

        RecoveryKitMessage::SubmitPassword => {
            submit_password(rk, cache, local_cube_id, client, server_cube_id, wallet)
        }

        RecoveryKitMessage::UploadResult(res) => {
            match res {
                Ok(outcome) => {
                    let updated_at = outcome.updated_at.clone();
                    let now_has_seed = outcome.now_has_seed;
                    let now_has_descriptor = outcome.now_has_descriptor;
                    // Only refresh the cached drift fingerprint when
                    // *this* upload actually included a descriptor half.
                    // Passing through `None` would wipe a previously-
                    // stored fingerprint on a seed-only upload (e.g.
                    // `AddSeed`), which then silently disables drift
                    // detection for the descriptor that's still on
                    // the server. The `Remove` path clears the
                    // fingerprint through its own dedicated call.
                    let fp_to_persist = next_fingerprint_to_persist(&outcome);
                    let persist = if let Some(fp) = fp_to_persist {
                        persist_descriptor_fingerprint(
                            cache,
                            local_cube_id,
                            DescriptorFingerprintSlot::Password,
                            Some(fp),
                        )
                    } else {
                        Task::none()
                    };
                    // "Both" protection mode (PR 2): the password kit is now
                    // uploaded; chain the phone seal with the same material. Pull
                    // the unlocked mnemonic out of the `Uploading` snapshot (ends
                    // the borrow before `start_phone_seal` reborrows `rk`).
                    if rk.also_phone_seal {
                        rk.also_phone_seal = false;
                        let (mode, mnemonic) = match &mut rk.flow {
                            RecoveryKitState::Uploading { pending } => {
                                (pending.mode, pending.mnemonic.take())
                            }
                            _ => (RecoveryKitMode::Create, None),
                        };
                        let phone = start_phone_seal(
                            rk,
                            cache,
                            local_cube_id,
                            client,
                            server_cube_id,
                            wallet,
                            mode,
                            mnemonic,
                        );
                        Task::batch([persist, phone])
                    } else {
                        rk.flow = RecoveryKitState::Completed {
                            updated_at,
                            now_has_seed,
                            now_has_descriptor,
                        };
                        persist
                    }
                }
                Err(e) => {
                    // Restore the user's password-entry inputs from
                    // the `Uploading` snapshot so a transient network
                    // failure doesn't force them to retype their
                    // password (and re-enter their PIN to decrypt
                    // the mnemonic again). See `restore_entry_on_upload_error`.
                    restore_entry_on_upload_error(&mut rk.flow, &e);
                    Task::done(Message::View(view::Message::ShowError(e)))
                }
            }
        }

        RecoveryKitMessage::DismissCompleted => {
            rk.reset_flow();
            // Re-fetch status so the card shows the new state.
            load_status(rk, client, server_cube_id)
        }

        RecoveryKitMessage::Remove => {
            // Removal is destructive and irreversible (master F5): don't delete
            // on the first press. Take over the page with a confirmation screen
            // that names exactly which backup paths will be torn down; the
            // actual delete runs only on `ConfirmRemove`. Cancel → `reset_flow`.
            rk.flow = RecoveryKitState::ConfirmRemove;
            Task::none()
        }

        RecoveryKitMessage::ConfirmRemove => {
            let Some(client) = client else {
                return Task::done(Message::View(view::Message::ShowError(
                    "Sign in to Connect to remove your Recovery Kit.".to_string(),
                )));
            };
            let Some(cube_id) = server_cube_id else {
                return Task::done(Message::View(view::Message::ShowError(
                    "This Cube isn't registered with Connect yet.".to_string(),
                )));
            };
            // Build the delete set from the current status — the same
            // per-method view the confirm screen showed the user. The password
            // kit is always torn down (idempotent, 404-tolerant); the keychain
            // half only when a phone envelope set is present.
            let overview = backup_overview(rk.status.as_ref());
            let delete_keychain = overview.keychain.is_some();
            if !overview.any_enabled() {
                // No-op guard: nothing enabled (e.g. status changed underneath
                // us). Return to the card rather than firing empty deletes.
                rk.reset_flow();
                return load_status(rk, Some(client), server_cube_id);
            }
            rk.flow = RecoveryKitState::Removing;
            Task::perform(
                async move { remove_backups(client, cube_id, delete_keychain).await },
                |res| {
                    Message::View(view::Message::Settings(view::SettingsMessage::RecoveryKit(
                        RecoveryKitMessage::RemoveResult(res),
                    )))
                },
            )
        }

        RecoveryKitMessage::RemoveResult(res) => {
            match res {
                Ok(()) => {
                    rk.reset_flow();
                    // Clear both local drift fingerprint slots — Remove-all left
                    // nothing backed up to compare against, by either method
                    // (per-method drift, PR 3).
                    let persist = clear_descriptor_fingerprints(cache, local_cube_id);
                    let reload = load_status(rk, client, server_cube_id);
                    Task::batch([persist, reload])
                }
                Err(RemoveFailure {
                    message,
                    password_kit_deleted,
                }) => {
                    rk.flow = RecoveryKitState::Error {
                        message: message.clone(),
                    };
                    // Removal runs sequentially and can partially succeed — e.g.
                    // the password kit was deleted, then the phone (Keychain)
                    // step failed. The server state has already changed even
                    // though the overall op reports failure, so refresh status
                    // now (it only updates `rk.status`, not the `Error` flow) —
                    // otherwise the card keeps showing a password backup Connect
                    // no longer has once the user dismisses the error. Mirrors
                    // the `PhoneSealResult` partial-failure handling.
                    let reload = load_status(rk, client, server_cube_id);
                    // `load_status` only fixes the in-memory `rk.status`; the
                    // persisted per-method fingerprint slots drive
                    // `has_recovery_kit()` / `connect_state()`. When the password
                    // kit was already torn down before the failure, its local
                    // fingerprint now points at a kit Connect no longer holds, so
                    // clear just that slot. The keychain slot stays: its copy may
                    // well survive the failure (the very reason we failed loudly),
                    // and a retry — or the phone-seal path — reconciles it.
                    let clear_stale_password = if password_kit_deleted {
                        persist_descriptor_fingerprint(
                            cache,
                            local_cube_id,
                            DescriptorFingerprintSlot::Password,
                            None,
                        )
                    } else {
                        Task::none()
                    };
                    Task::batch([
                        Task::done(Message::View(view::Message::ShowError(message))),
                        clear_stale_password,
                        reload,
                    ])
                }
            }
        }
    }
}

fn load_status(
    rk: &mut RecoveryKit,
    client: Option<CoincubeClient>,
    server_cube_id: Option<u64>,
) -> Task<Message> {
    let Some(client) = client else {
        return Task::none();
    };
    let Some(cube_id) = server_cube_id else {
        return Task::none();
    };
    if rk.status_loading {
        return Task::none();
    }
    rk.status_loading = true;
    Task::perform(
        async move {
            match client.get_recovery_kit_status(cube_id).await {
                Ok(s) => Ok(Some(s)),
                // 404 is the "no kit yet" signal — not an error for
                // our card logic. Collapse it to Ok(None).
                Err(CoincubeError::NotFound) => Ok(None),
                Err(e) => Err(e.to_string()),
            }
        },
        |res| {
            Message::View(view::Message::Settings(view::SettingsMessage::RecoveryKit(
                RecoveryKitMessage::StatusLoaded(res),
            )))
        },
    )
}

fn verify_pin(rk: &mut RecoveryKit, cache: &Cache, local_cube_id: &str) -> Task<Message> {
    let mode = match &rk.flow {
        RecoveryKitState::PinEntry { mode, .. } => *mode,
        _ => return Task::none(),
    };
    if !rk.pin.is_complete() {
        rk.flow = RecoveryKitState::PinEntry {
            mode,
            error: Some("Please enter all 4 PIN digits".to_string()),
        };
        return Task::none();
    }
    let pin = rk.pin.value();

    // Reach into the on-disk settings for this cube to get the
    // fingerprint + PIN hash. Matches `handle_backup_message`.
    let network_dir = cache.datadir_path.network_directory(cache.network);
    let Ok(s) = settings::Settings::from_file(&network_dir) else {
        rk.flow = RecoveryKitState::PinEntry {
            mode,
            error: Some("Failed to read settings file".to_string()),
        };
        return Task::none();
    };
    let Some(cube) = s.cubes.iter().find(|c| c.id == local_cube_id).cloned() else {
        rk.flow = RecoveryKitState::PinEntry {
            mode,
            error: Some("Cube not found in settings".to_string()),
        };
        return Task::none();
    };
    let Some(fingerprint) = cube.master_signer_fingerprint else {
        rk.flow = RecoveryKitState::PinEntry {
            mode,
            error: Some("This Cube has no master signer.".to_string()),
        };
        return Task::none();
    };
    let datadir = cache.datadir_path.path().to_path_buf();
    let network = cache.network;
    let cube_id = cube.id.clone();

    // Same reasoning as the Backup-Master-Seed flow in `general.rs`: this is a
    // second route to the same permanent secret, so guesses accumulate against
    // the shared per-Cube unlock counter rather than starting fresh here.
    let throttle_root = datadir.clone();
    let remaining = crate::services::unlock::throttle::ThrottleState::load(&throttle_root)
        .remaining_lockout(&cube_id);
    if !remaining.is_zero() {
        return Task::done(Message::View(view::Message::Settings(
            view::SettingsMessage::RecoveryKit(RecoveryKitMessage::PinVerified(Err(
                crate::services::unlock::throttle::lockout_message(remaining),
            ))),
        )));
    }

    Task::perform(
        async move {
            // Wrap the decrypted mnemonic in `Zeroizing` at the
            // async boundary — before the value enters the message
            // queue — so every in-flight copy of the subsequent
            // `PinVerified` message gets wiped on drop, not just
            // the final one stashed in state. `load_mnemonic_words`
            // returns a plain `Vec<String>` because the local
            // paper-backup flow doesn't need Zeroizing wrapping
            // (its final consumer is a `Zeroizing<Vec<String>>`
            // state field); here we promote at the earliest
            // reachable point.
            tokio::task::spawn_blocking(move || {
                // Decrypting the seed file *is* the PIN check (I1); the cheap
                // `verify_pin` hash that used to gate this is gone.
                let mut throttle =
                    crate::services::unlock::throttle::ThrottleState::load(&throttle_root);
                match super::general::load_mnemonic_words(
                    &datadir,
                    network,
                    fingerprint,
                    &pin,
                    &cube_id,
                ) {
                    Ok(words) => {
                        throttle.record_success(&throttle_root, &cube_id);
                        Ok(Zeroizing::new(words))
                    }
                    // Same split as the Backup-Master-Seed flow: a locked
                    // keychain or a missing seed file is not a guess, so it
                    // keeps its own message and spends no throttle budget.
                    Err(e) if !super::general::is_wrong_pin(&e) => Err(e.to_string()),
                    Err(_) => {
                        let penalty = throttle.record_failure(&throttle_root, &cube_id);
                        Err(if penalty.is_zero() {
                            "Incorrect PIN. Please try again.".to_string()
                        } else {
                            crate::services::unlock::throttle::lockout_message(penalty)
                        })
                    }
                }
            })
            .await
            .map_err(|e| format!("PIN verification task failed: {}", e))?
        },
        |res| {
            Message::View(view::Message::Settings(view::SettingsMessage::RecoveryKit(
                RecoveryKitMessage::PinVerified(res),
            )))
        },
    )
}

fn submit_password(
    rk: &mut RecoveryKit,
    cache: &Cache,
    // Use the dispatcher-injected local cube id rather than
    // `cache.cube_id`. Both come from `self.cube_settings.id` today,
    // but they refresh on different cadences — routing everything in
    // this module through the same `local_cube_id` parameter keeps
    // `verify_pin`, `submit_password`, and the UploadResult
    // `persist_descriptor_fingerprint` call all pointing at the same
    // cube even if a mid-session settings reload ever desynchronizes
    // the two sources.
    local_cube_id: &str,
    client: Option<CoincubeClient>,
    server_cube_id: Option<u64>,
    wallet: Option<Arc<Wallet>>,
) -> Task<Message> {
    // Pull validation + cloneable inputs out of the state without
    // moving fields — we'll set `rk.flow = Uploading { pending }`
    // once all checks pass and the task is on its way. The `pending`
    // snapshot lets us restore `PasswordEntry` intact if the upload
    // fails, so a transient network error doesn't force the user to
    // re-type their password (and re-enter their PIN to unlock the
    // mnemonic again).
    let (password_copy, mnemonic_clone_opt, pending) = match &rk.flow {
        RecoveryKitState::PasswordEntry {
            mode: m,
            mnemonic,
            password,
            confirm,
            acknowledged,
            ..
        } => {
            if password.as_str() != confirm.as_str() {
                set_pw_error(rk, "Passwords don't match.");
                return Task::none();
            }
            if password.len() < MIN_PASSWORD_LEN {
                set_pw_error(
                    rk,
                    &format!("Password must be at least {} characters.", MIN_PASSWORD_LEN),
                );
                return Task::none();
            }
            let (strength, _) = recovery::score_password(password, &[]);
            if !strength.is_acceptable() {
                set_pw_error(
                    rk,
                    &format!(
                        "Password is too weak ({}). Try a longer passphrase or add complexity.",
                        strength.label()
                    ),
                );
                return Task::none();
            }
            if !*acknowledged {
                set_pw_error(rk, "Please confirm that you've written down this password.");
                return Task::none();
            }
            let pending = PasswordEntryPending {
                mode: *m,
                mnemonic: mnemonic.clone(),
                password: password.clone(),
                confirm: confirm.clone(),
                acknowledged: *acknowledged,
            };
            (
                Zeroizing::new(password.to_string()),
                mnemonic.clone(),
                pending,
            )
        }
        _ => return Task::none(),
    };

    // Pull cube-scoped metadata from settings so the blob is complete.
    let network_dir = cache.datadir_path.network_directory(cache.network);
    let Ok(s) = settings::Settings::from_file(&network_dir) else {
        set_pw_error(rk, "Failed to read settings file.");
        return Task::none();
    };
    let Some(cube) = s.cubes.iter().find(|c| c.id == local_cube_id).cloned() else {
        set_pw_error(rk, "Cube not found in settings.");
        return Task::none();
    };
    let cube_uuid = cube.id.clone();
    let cube_name = cube.name.clone();
    let network = network_str(cube.network);
    let lightning_address = cache.lightning_address.clone();
    let created_at_str = chrono::DateTime::<chrono::Utc>::from_timestamp(cube.created_at, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());

    let Some(client) = client else {
        set_pw_error(rk, "Sign in to Connect to back up your Recovery Kit.");
        return Task::none();
    };
    let Some(cube_id_num) = server_cube_id else {
        set_pw_error(rk, "This Cube isn't registered with Connect yet.");
        return Task::none();
    };

    // Build descriptor blob when we have a live wallet.
    let descriptor_blob = wallet
        .as_ref()
        .map(|w| descriptor_blob_from_wallet(w, &cube_uuid, &network));

    rk.flow = RecoveryKitState::Uploading { pending };

    let mnemonic = mnemonic_clone_opt;
    Task::perform(
        async move {
            encrypt_and_upload(
                client,
                cube_id_num,
                mnemonic,
                descriptor_blob,
                SeedBlobCube {
                    uuid: cube_uuid,
                    name: cube_name,
                    network,
                    created_at: created_at_str,
                    lightning_address,
                },
                password_copy,
            )
            .await
        },
        |res| {
            Message::View(view::Message::Settings(view::SettingsMessage::RecoveryKit(
                RecoveryKitMessage::UploadResult(res),
            )))
        },
    )
}

fn set_pw_error(rk: &mut RecoveryKit, msg: &str) {
    if let RecoveryKitState::PasswordEntry { error, .. } = &mut rk.flow {
        *error = Some(msg.to_string());
    }
}

// ── Owner keychain recovery — protection mode (PR 1 + PR 2) ──────────────────

/// Where the wizard goes once the seed is unlocked. With the owner-keychain flag
/// on, the owner picks a protection mode (PR 2); with it off, straight to the
/// password entry (unchanged behaviour).
fn next_after_seed_unlock(
    mode: RecoveryKitMode,
    mnemonic: Option<Zeroizing<Vec<String>>>,
) -> RecoveryKitState {
    if feature_flags::OWNER_KEYCHAIN_RECOVERY_ENABLED {
        RecoveryKitState::ProtectionChoice {
            mode,
            mnemonic,
            // Detection is kicked off immediately on entry (see
            // `enter_after_seed_unlock`), so start in `Checking` rather than
            // showing the phone options in an indeterminate state.
            phone: PhoneKey::Checking,
            error: None,
        }
    } else {
        password_entry(mode, mnemonic)
    }
}

/// Move into the post-seed-unlock state and, when that's the protection-choice
/// screen, immediately kick off the owner-self phone-key detection so the phone
/// options reflect reality without the user having to click anything. With the
/// flag off (`password_entry`) there's nothing to detect.
fn enter_after_seed_unlock(
    rk: &mut RecoveryKit,
    mode: RecoveryKitMode,
    mnemonic: Option<Zeroizing<Vec<String>>>,
    client: Option<CoincubeClient>,
    server_cube_id: Option<u64>,
) -> Task<Message> {
    rk.flow = next_after_seed_unlock(mode, mnemonic);
    if matches!(rk.flow, RecoveryKitState::ProtectionChoice { .. }) {
        begin_phone_detection(rk, client, server_cube_id)
    } else {
        Task::none()
    }
}

/// Set `PhoneKey::Checking` and spawn the detection task. Shared by the auto-run
/// on entry and the "Check again" link. On a missing client / cube id, records
/// the reason as a `PhoneKey::Error` inline (retryable) rather than spawning.
fn begin_phone_detection(
    rk: &mut RecoveryKit,
    client: Option<CoincubeClient>,
    server_cube_id: Option<u64>,
) -> Task<Message> {
    let Some(client) = client else {
        set_phone_detection(
            rk,
            PhoneKey::Error("Sign in to Connect to check for your phone key.".to_string()),
        );
        return Task::none();
    };
    let Some(cube_id) = server_cube_id else {
        set_phone_detection(
            rk,
            PhoneKey::Error("This Cube isn't registered with Connect yet.".to_string()),
        );
        return Task::none();
    };
    set_phone_detection(rk, PhoneKey::Checking);
    Task::perform(
        async move { detect_owner_self_recipient(client, cube_id).await },
        |res| {
            Message::View(view::Message::Settings(view::SettingsMessage::RecoveryKit(
                RecoveryKitMessage::ProvisionResult(res),
            )))
        },
    )
}

/// Update the phone-key detection status on the choice screen. No-op off it
/// (e.g. a stale detection result arriving after the user navigated away).
fn set_phone_detection(rk: &mut RecoveryKit, status: PhoneKey) {
    if let RecoveryKitState::ProtectionChoice { phone, .. } = &mut rk.flow {
        *phone = status;
    }
}

/// A fresh `PasswordEntry` state carrying the unlocked mnemonic (if any).
fn password_entry(
    mode: RecoveryKitMode,
    mnemonic: Option<Zeroizing<Vec<String>>>,
) -> RecoveryKitState {
    RecoveryKitState::PasswordEntry {
        mode,
        mnemonic,
        password: Zeroizing::new(String::new()),
        confirm: Zeroizing::new(String::new()),
        acknowledged: false,
        error: None,
    }
}

/// Set the inline error on the protection-choice screen (no-op off that screen).
fn set_protection_error(rk: &mut RecoveryKit, msg: &str) {
    match &mut rk.flow {
        // On the protection-choice screen, surface the failure inline so the
        // user stays put and can pick another mode or retry.
        RecoveryKitState::ProtectionChoice { error, .. } => {
            *error = Some(msg.to_string());
        }
        // Reached from the "Both" chain (the password kit already uploaded; the
        // phone seal couldn't even start). The flow is `Uploading`, so an inline
        // choice-screen error would be a silent no-op and leave the wizard stuck
        // on the uploading screen forever. Fall back to the terminal error state.
        _ => {
            rk.flow = RecoveryKitState::Error {
                message: msg.to_string(),
            };
        }
    }
}

/// Read cube-scoped metadata from settings: the `SeedBlobCube` (for a seed blob)
/// plus the cube uuid + canonical network string (for the descriptor blob).
/// Mirrors the gathering `submit_password` does inline.
fn gather_cube_meta(cache: &Cache, local_cube_id: &str) -> Option<(SeedBlobCube, String, String)> {
    let network_dir = cache.datadir_path.network_directory(cache.network);
    let s = settings::Settings::from_file(&network_dir).ok()?;
    let cube = s.cubes.iter().find(|c| c.id == local_cube_id).cloned()?;
    let cube_uuid = cube.id.clone();
    let network = network_str(cube.network);
    let created_at_str = chrono::DateTime::<chrono::Utc>::from_timestamp(cube.created_at, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    let meta = SeedBlobCube {
        uuid: cube_uuid.clone(),
        name: cube.name.clone(),
        network: network.clone(),
        created_at: created_at_str,
        lightning_address: cache.lightning_address.clone(),
    };
    Some((meta, cube_uuid, network))
}

/// Bail out of `start_phone_seal` after a synchronous preflight failure
/// (missing client / cube / settings), branching by the caller's flow:
///   * Direct Phone path — the flow is still `ProtectionChoice` and the unlocked
///     `mnemonic` was lifted out for the seal. Put it back so a retry can still
///     escrow the seed instead of silently downgrading to descriptor-only.
///   * "Both" chain — the flow is `Uploading` (the password kit already
///     uploaded); the lifted mnemonic is no longer needed and is dropped.
///
/// `set_protection_error` then shows the message inline on the choice screen, or
/// routes the `Uploading` flow to the terminal error state so the wizard isn't
/// stranded mid-upload.
fn fail_phone_preflight(
    rk: &mut RecoveryKit,
    mnemonic: Option<Zeroizing<Vec<String>>>,
    msg: &str,
) -> Task<Message> {
    if let RecoveryKitState::ProtectionChoice { mnemonic: slot, .. } = &mut rk.flow {
        *slot = mnemonic;
    }
    set_protection_error(rk, msg);
    Task::none()
}

/// Transition into `PhoneSealing` and spawn the phone seal+upload task. Used by
/// both the direct phone path and the "Both" chain. On a missing client / cube /
/// settings, hands off to `fail_phone_preflight` (inline error + mnemonic
/// restored on the choice screen, or terminal error for the "Both" chain).
#[allow(clippy::too_many_arguments)]
fn start_phone_seal(
    rk: &mut RecoveryKit,
    cache: &Cache,
    local_cube_id: &str,
    client: Option<CoincubeClient>,
    server_cube_id: Option<u64>,
    wallet: Option<Arc<Wallet>>,
    mode: RecoveryKitMode,
    mnemonic: Option<Zeroizing<Vec<String>>>,
) -> Task<Message> {
    let Some(client) = client else {
        return fail_phone_preflight(rk, mnemonic, "Sign in to Connect to back up to your phone.");
    };
    let Some(cube_id) = server_cube_id else {
        return fail_phone_preflight(rk, mnemonic, "This Cube isn't registered with Connect yet.");
    };
    let Some((cube_meta, cube_uuid, network)) = gather_cube_meta(cache, local_cube_id) else {
        return fail_phone_preflight(rk, mnemonic, "Failed to read settings file.");
    };
    let descriptor_blob = wallet
        .as_ref()
        .map(|w| descriptor_blob_from_wallet(w, &cube_uuid, &network));
    rk.flow = RecoveryKitState::PhoneSealing { mode };
    Task::perform(
        async move { seal_phone(client, cube_id, descriptor_blob, mnemonic, cube_meta).await },
        |res| {
            Message::View(view::Message::Settings(view::SettingsMessage::RecoveryKit(
                RecoveryKitMessage::PhoneSealResult(res),
            )))
        },
    )
}

/// Seal the owner's recovery material to their `owner-self` Keychain key and
/// upload the envelope set (PR 2). Owner-side desktop crypto only. The seed half
/// is included iff the recipient's registered tier is Full-Cube. The seed
/// plaintext stays `Zeroizing` exactly as the password path keeps it.
///
/// On success returns the SHA-256 fingerprint of the descriptor blob that was
/// sealed — the same helper the password path uses — so the handler can persist
/// the keychain drift slot (per-method drift, PR 3). `None` only if the
/// fingerprint couldn't be computed; the seal always includes a descriptor.
async fn seal_phone(
    client: CoincubeClient,
    cube_id: u64,
    descriptor_blob: Option<DescriptorBlob>,
    mnemonic: Option<Zeroizing<Vec<String>>>,
    cube_meta: SeedBlobCube,
) -> Result<Option<String>, String> {
    let recipient = find_owner_self_recipient(&client, cube_id)
        .await
        .map_err(|e| e.to_string())?;
    let descriptor_blob = descriptor_blob.ok_or_else(|| {
        "Create a Vault first — phone recovery needs a Wallet Descriptor to back up.".to_string()
    })?;
    // Fingerprint the exact descriptor being sealed, before it's serialized into
    // the envelope, so the keychain drift slot reflects what the phone now holds.
    let descriptor_fingerprint = descriptor_blob_fingerprint(&descriptor_blob);
    let descriptor_json: Zeroizing<Vec<u8>> = Zeroizing::new(
        serde_json::to_vec(&descriptor_blob).map_err(|e| format!("serialize descriptor: {}", e))?,
    );
    let include_seed = recipient.tier.map(|t| t.includes_seed()).unwrap_or(false);
    let seed_json = if include_seed {
        let words = mnemonic
            .ok_or_else(|| "This Cube can't back up its seed to your phone.".to_string())?;
        let blob = SeedBlob {
            version: BLOB_VERSION,
            cube: cube_meta,
            mnemonic: SeedBlobMnemonic {
                phrase: words.join(" "),
                language: "en".to_string(),
            },
        };
        Some(Zeroizing::new(
            serde_json::to_vec(&blob).map_err(|e| format!("serialize seed: {}", e))?,
        ))
    } else {
        None
    };
    seal_and_upload_owner_self(&client, cube_id, &recipient, &descriptor_json, seed_json)
        .await
        .map_err(|e| e.to_string())?;
    Ok(descriptor_fingerprint)
}

/// Detect the owner's phone-registered `owner-self` recovery recipient (PR 1).
/// Provisioning is phone-initiated (COIN-390): the Keychain app mints + attaches
/// the key and registers the recipient (tier `full_cube`). The desktop never
/// mints or registers — it polls the recipients list and reports whether the
/// `owner-self` row exists yet. Maps the absent case
/// ([`OwnerSelfError::NoRecipient`]) to [`PhoneKeyOutcome::Absent`] — a normal
/// "not set up yet" state that drives guidance copy, *not* an error — and any
/// other failure to [`PhoneKeyOutcome::Failed`].
async fn detect_owner_self_recipient(client: CoincubeClient, cube_id: u64) -> PhoneKeyOutcome {
    match find_owner_self_recipient(&client, cube_id).await {
        Ok(_) => PhoneKeyOutcome::Present,
        Err(OwnerSelfError::NoRecipient) => PhoneKeyOutcome::Absent,
        Err(e) => PhoneKeyOutcome::Failed(e.to_string()),
    }
}

/// Network string used inside `SeedBlob`/`DescriptorBlob`. Routed
/// through the canonical `settings::network_to_api_string` so blobs
/// written here agree with the Connect API's convention (`"mainnet"`
/// for Bitcoin mainnet). A mismatch would leak into `DescriptorBlob.
/// cube.network`, the fingerprint hash, and the restore-side
/// network-filter — and silently break cross-client interop.
fn network_str(n: Network) -> String {
    settings::network_to_api_string(n)
}

/// Result of the pre-flight check run at the top of
/// `RecoveryKitMessage::Start`. Split out so the branch table is
/// unit-testable without an authenticated Connect client or a full
/// `App` instance.
#[derive(Debug, PartialEq, Eq)]
enum StartGuard {
    /// User is signed in and the cube is registered with Connect —
    /// proceed into the PIN/password wizard.
    Ok,
    /// No authenticated client available. Before we collect any
    /// secrets (PIN, password), route the user to sign in.
    NotSignedIn,
    /// User is signed in but this Cube hasn't been registered with
    /// Connect yet (missing `server_cube_id`). Rare — happens when
    /// the `register_cube` call is still pending at the time the
    /// user hits the Recovery-Kit CTA.
    CubeNotRegistered,
}

fn start_guard(client: Option<&CoincubeClient>, server_cube_id: Option<u64>) -> StartGuard {
    if client.is_none() {
        return StartGuard::NotSignedIn;
    }
    if server_cube_id.is_none() {
        return StartGuard::CubeNotRegistered;
    }
    StartGuard::Ok
}

/// After a successful upload, what fingerprint (if any) should be
/// written to the local drift cache? `None` means "leave the existing
/// cache untouched" — the key distinction from "clear it", which the
/// Remove path handles explicitly via `persist_descriptor_fingerprint(
/// ..., None)`. Seed-only uploads don't compute a descriptor
/// fingerprint, so they must not overwrite the previously-stored one
/// — otherwise the still-on-server descriptor would become invisible
/// to drift detection.
fn next_fingerprint_to_persist(outcome: &RecoveryKitUploadOutcome) -> Option<String> {
    outcome.descriptor_fingerprint.clone()
}

/// Should the post-vault-creation nudge toast be shown, given a
/// `RecoveryKitStatus` loaded fresh from Connect?
///
/// Returns `true` when the user clearly needs to back up a descriptor:
/// no kit at all, or a kit without the descriptor half. `None` is
/// treated as "needs nudge" — the only way to reach `None` after a
/// successful `StatusLoaded(Ok(...))` is if the in-memory slot was
/// never populated, which means the 404 fallback also didn't fire,
/// and nudging is the conservative choice.
fn should_nudge_for_status(status: Option<&RecoveryKitStatus>) -> bool {
    status
        .map(|s| !s.has_recovery_kit || !s.has_encrypted_wallet_descriptor)
        .unwrap_or(true)
}

pub(crate) fn descriptor_blob_from_wallet(
    wallet: &Wallet,
    cube_uuid: &str,
    network: &str,
) -> DescriptorBlob {
    // The descriptor string embeds the full signer xpubs inline, so
    // restore is self-contained from `vault.descriptor` alone. The
    // separate `signers` array is metadata the UI can show pre-restore
    // (fingerprint, friendly name) — xpubs are extracted by the
    // restorer from the descriptor itself. Leaving `xpub` empty here
    // is deliberate: we'd need a miniscript-level traversal to pull
    // per-fingerprint xpubs out cleanly, which is a larger change.
    //
    // `descriptor_keys()` returns a `HashSet`, whose iteration order
    // is randomised per-process by Rust's default hasher. We sort
    // the fingerprints by their lowercase hex form before building
    // the `DescriptorBlobSigner` list — otherwise every `App` tick
    // serialises `signers` in a different order, making the
    // SHA-256 descriptor fingerprint (W12 drift detection) flip
    // randomly and fire a bogus drift banner on every refresh.
    let mut fps: Vec<_> = wallet.descriptor_keys().into_iter().collect();
    fps.sort_by_key(|fp| format!("{}", fp).to_lowercase());
    let signers: Vec<DescriptorBlobSigner> = fps
        .into_iter()
        .map(|fp| DescriptorBlobSigner {
            name: wallet
                .keys_aliases
                .get(&fp)
                .cloned()
                .unwrap_or_else(|| "Signer".to_string()),
            fingerprint: format!("{}", fp).to_lowercase(),
            xpub: String::new(),
        })
        .collect();
    DescriptorBlob {
        version: BLOB_VERSION,
        cube: DescriptorBlobCube {
            uuid: cube_uuid.to_string(),
            network: network.to_string(),
        },
        vault: DescriptorBlobVault {
            name: wallet.name.clone(),
            descriptor: wallet.main_descriptor.to_string(),
            change_descriptor: None,
            signers,
        },
    }
}

/// SHA-256 over the canonical JSON of the descriptor blob. Used as
/// the drift fingerprint cached on `CubeSettings`. Deterministic so
/// long as `serde_json::to_vec` is called consistently.
pub fn descriptor_blob_fingerprint(blob: &DescriptorBlob) -> Option<String> {
    let bytes = serde_json::to_vec(blob).ok()?;
    let digest = Sha256::digest(&bytes);
    Some(hex::encode(digest))
}

/// Compute the live descriptor fingerprint for a wallet. Used by
/// `App::update` to populate `cache.current_descriptor_fingerprint`
/// each tick, which the Settings card compares against the last-
/// backed-up fingerprint to surface the drift banner (W12).
pub fn live_descriptor_fingerprint(
    wallet: &Wallet,
    cube_uuid: &str,
    network: &str,
) -> Option<String> {
    let blob = descriptor_blob_from_wallet(wallet, cube_uuid, network);
    descriptor_blob_fingerprint(&blob)
}

/// Decide which halves to include in the upload. Per master plan §5,
/// every update re-encrypts **all present blobs** under the new
/// password — otherwise the kit's two halves end up encrypted under
/// different passwords and a single-password restore becomes
/// impossible. The mode is therefore **not** a filter here: it only
/// gates earlier UI decisions (whether to prompt for PIN, wizard
/// copy). At upload time we always send every half for which we have
/// plaintext available.
///
/// - `mnemonic.is_some()` → we have a mnemonic (mnemonic cubes after
///   PIN verify). Passkey cubes never reach this branch because they
///   never populate `mnemonic` in state.
/// - `descriptor_blob.is_some()` → a live Vault exists on this
///   device. Cubes without a Vault legitimately upload seed-only.
fn include_halves(
    mnemonic: &Option<Zeroizing<Vec<String>>>,
    descriptor_blob: &Option<DescriptorBlob>,
) -> (bool, bool) {
    (mnemonic.is_some(), descriptor_blob.is_some())
}

/// Treat a `CoincubeError::NotFound` from `delete_recovery_kit` as
/// success — the server state we wanted (no kit) is already what the
/// server reports, so DELETE is idempotent from the caller's
/// perspective. This lets the `RemoveResult(Ok)` branch run
/// `persist_descriptor_fingerprint(None)` + `load_status(...)` just
/// as it would on a first-time-successful delete, instead of hitting
/// the Error screen and leaving a stale local fingerprint cache
/// behind. Other errors (auth, 429, 5xx) propagate as `Err(String)`
/// so the user can retry.
fn normalize_delete_result(res: Result<(), CoincubeError>) -> Result<(), String> {
    match res {
        Ok(()) | Err(CoincubeError::NotFound) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Tear down every backup path for this Cube (master F5). "Remove clears
/// everything", so the password-kit delete runs **unconditionally** — it's
/// idempotent and a `404` (envelope-only Cube, or a kit already removed from
/// another device) counts as success — while the keychain teardown runs only
/// when a phone envelope set is present. The deletes run **sequentially** and
/// **fail loudly** on the first hard error, naming the path that's still
/// restorable so the caller never reports success while recovery material
/// survives:
///
///   * always → `DELETE /recovery-kit` (the password kit), `404`-tolerant;
///   * `delete_keychain` → resolve the `owner-self` recipient and
///     `DELETE .../recipients/{keyId}`, which cascade-deletes its envelopes
///     server-side. A `404` on the recipient DELETE is idempotent success; a
///     *missing* recipient is **verified** gone via [`verify_phone_copy_gone`]
///     rather than assumed (F5).
///
/// A password-kit failure returns **before** touching the keychain half, so the
/// envelopes stay untouched and the user can retry cleanly. Both deletes are
/// idempotent, so a retry after a partial success finishes the teardown.
async fn remove_backups(
    client: CoincubeClient,
    cube_id: u64,
    delete_keychain: bool,
) -> Result<(), RemoveFailure> {
    normalize_delete_result(client.delete_recovery_kit(cube_id).await).map_err(|e| {
        RemoveFailure {
            message: format!(
                "Couldn't remove your password-encrypted Recovery Kit ({}). It's still \
                 restorable — try Remove again.",
                e
            ),
            // The delete itself failed, so the password kit is still on Connect
            // and the local fingerprint slot is still accurate — don't drop it.
            password_kit_deleted: false,
        }
    })?;
    // The password kit is now gone (deleted, or a 404 meaning it was already
    // absent). The phone-half work below can still fail, but the password half
    // is torn down — tag any failure so the caller clears the now-stale local
    // password fingerprint slot instead of leaving it pointing at a dead kit.
    remove_phone_copy(&client, cube_id, delete_keychain)
        .await
        .map_err(|message| RemoveFailure {
            message,
            password_kit_deleted: true,
        })
}

/// Tear down (or verify absent) the owner-keychain (phone) copy — the second
/// half of [`remove_backups`], split out so the password-half and phone-half
/// failures can carry different `password_kit_deleted` flags. Returns the
/// user-facing error string on failure; the caller wraps it in [`RemoveFailure`].
async fn remove_phone_copy(
    client: &CoincubeClient,
    cube_id: u64,
    delete_keychain: bool,
) -> Result<(), String> {
    if delete_keychain {
        match find_owner_self_recipient(client, cube_id).await {
            Ok(recipient) => match client
                .delete_recovery_kit_recipient(cube_id, recipient.key_id)
                .await
            {
                // 404 → the recipient was already gone; the cascade left nothing
                // to remove. Idempotent success.
                Ok(()) | Err(CoincubeError::NotFound) => {}
                Err(e) => {
                    return Err(format!(
                        "Couldn't remove the copy sealed to your phone (Keychain) ({}). The \
                         phone copy is still restorable — try Remove again.",
                        e
                    ))
                }
            },
            // No owner-self recipient to delete. The cached status said the
            // phone copy existed, so this normally means it was already torn
            // down elsewhere (the recipient DELETE cascades its envelopes) — but
            // F5 forbids *assuming* a restorable path is gone. Re-fetch status
            // and only succeed if the phone envelopes are confirmed absent; a
            // survivor (or a state we can't confirm) fails loudly so we never
            // clear the drift slots over a live phone copy. A survivor here has
            // no recipient key left to target, so it can't be removed from this
            // device — the user must contact support.
            Err(OwnerSelfError::NoRecipient) => {
                verify_phone_copy_gone(
                    client,
                    cube_id,
                    "The copy sealed to your phone (Keychain) is still restorable but its \
                     recovery key is no longer registered, so it couldn't be removed from this \
                     device. Contact support to clear it.",
                )
                .await?;
            }
            Err(e) => {
                return Err(format!(
                    "Couldn't reach the copy sealed to your phone (Keychain) ({}). The phone \
                     copy may still be restorable — try Remove again.",
                    e
                ))
            }
        }
    } else {
        // The cached overview showed no phone copy, so we skipped the keychain
        // teardown — but that view can be stale (a phone copy sealed elsewhere,
        // or a status race). F5 forbids *assuming* a restorable path is gone,
        // and the caller is about to clear the keychain drift slot on the
        // strength of this removal. Confirm no phone envelopes survive before
        // returning success; a survivor fails loudly so we never wipe the local
        // fingerprint over a live copy. Unlike the orphaned case above, a
        // recipient may well still exist — a fresh Remove (with up-to-date
        // status) would target it — so the message says to retry.
        verify_phone_copy_gone(
            client,
            cube_id,
            "The copy sealed to your phone (Keychain) is still restorable. Reopen Settings and \
             try Remove again to clear it.",
        )
        .await?;
    }
    Ok(())
}

/// Confirm the owner-keychain (phone) copy is actually gone before a removal
/// reports success and the caller clears the keychain drift slot (F5). Reached
/// from two branches of [`remove_backups`]:
///   * the `NoRecipient` branch, where the cached status said envelopes existed
///     but no owner-self recipient remains to delete; and
///   * the `delete_keychain == false` branch, where the cached overview showed
///     no phone copy at all and the teardown was skipped — that view can be
///     stale, so we verify rather than assume.
///
/// Re-fetches `/recovery-kit/status`:
///   * envelopes still present → hard error with the caller-supplied
///     `present_msg` (the advice differs: orphaned recipient → contact support;
///     stale overview → retry Remove);
///   * no envelopes / no kit at all → the copy is confirmed gone → `Ok`;
///   * the check itself fails → uncertain → hard error, so we never claim a
///     removal we couldn't confirm.
async fn verify_phone_copy_gone(
    client: &CoincubeClient,
    cube_id: u64,
    present_msg: &str,
) -> Result<(), String> {
    match client.get_recovery_kit_status(cube_id).await {
        Ok(status) => {
            let phone_present = status
                .owner_self
                .as_ref()
                .map(|o| o.has_envelope())
                .unwrap_or(false);
            if phone_present {
                Err(present_msg.to_string())
            } else {
                Ok(())
            }
        }
        // No kit at all on the server → nothing restorable survives.
        Err(CoincubeError::NotFound) => Ok(()),
        Err(e) => Err(format!(
            "Removed your password kit, but couldn't confirm the copy sealed to your phone \
             (Keychain) was cleared ({}). Reopen Settings to check before relying on removal.",
            e
        )),
    }
}

/// On upload failure, restore the `PasswordEntry` screen from the
/// snapshot we stashed inside `Uploading { pending }` at submit
/// time. The user keeps their password, confirm, acknowledge flag,
/// and (for mnemonic cubes) the decrypted mnemonic — so a transient
/// network error doesn't throw away their PIN-decryption work.
///
/// Uses `mem::replace` so the `Zeroizing`-wrapped fields stay
/// wrapped the whole time (no plain-`String` copy on the stack).
///
/// If the incoming state isn't `Uploading` — which would only
/// happen on a stale message arriving after `Cancel` or similar —
/// we fall back to a terminal `Error` screen, preserving the old
/// behavior for the edge case.
fn restore_entry_on_upload_error(flow: &mut RecoveryKitState, err: &str) {
    let prev = std::mem::replace(
        flow,
        RecoveryKitState::Error {
            message: err.to_string(),
        },
    );
    if let RecoveryKitState::Uploading { pending } = prev {
        *flow = RecoveryKitState::PasswordEntry {
            mode: pending.mode,
            mnemonic: pending.mnemonic,
            password: pending.password,
            confirm: pending.confirm,
            acknowledged: pending.acknowledged,
            error: Some(err.to_string()),
        };
    }
    // Otherwise `flow` has already been set to the Error variant by
    // the `mem::replace` above — leave it.
}

async fn encrypt_and_upload(
    client: CoincubeClient,
    cube_id_num: u64,
    mnemonic: Option<Zeroizing<Vec<String>>>,
    descriptor_blob: Option<DescriptorBlob>,
    cube_meta: SeedBlobCube,
    password: Zeroizing<String>,
) -> Result<RecoveryKitUploadOutcome, String> {
    let (include_seed, include_descriptor) = include_halves(&mnemonic, &descriptor_blob);

    if !include_seed && !include_descriptor {
        return Err(
            "Nothing to back up — mnemonic cubes need a PIN, passkey cubes need a Vault."
                .to_string(),
        );
    }

    // Seed blob. Zeroisation invariants we maintain here:
    //
    // 1. The `phrase` string is constructed inline into the
    //    `SeedBlobMnemonic` initializer (no named binding) so it
    //    moves straight into the field. `SeedBlobMnemonic` derives
    //    `ZeroizeOnDrop`, so when `blob` drops at the end of this
    //    block the phrase's heap allocation is wiped.
    //
    // 2. `serde_json::to_vec` returns a plain `Vec<u8>` containing
    //    the JSON-serialised mnemonic — i.e. a second copy of the
    //    phrase bytes. Wrapping in `Zeroizing<Vec<u8>>` ensures those
    //    bytes are also wiped on drop. Without this wrap the JSON
    //    copy would linger on the heap past the end of this scope.
    //    `recovery::encrypt` takes `&[u8]` and `Zeroizing<Vec<u8>>`
    //    derefs cleanly.
    let seed_ct = if include_seed {
        let words = mnemonic.as_ref().unwrap();
        let blob = SeedBlob {
            version: BLOB_VERSION,
            cube: cube_meta.clone(),
            mnemonic: SeedBlobMnemonic {
                phrase: words.join(" "),
                language: "en".to_string(),
            },
        };
        let bytes: Zeroizing<Vec<u8>> = Zeroizing::new(
            serde_json::to_vec(&blob).map_err(|e| format!("serialize seed: {}", e))?,
        );
        Some(
            recovery::encrypt(&bytes, &password, KdfParams::DEFAULT_V1)
                .map_err(|e| format!("encrypt seed: {}", e))?,
        )
    } else {
        None
    };

    // Descriptor blob. Wrapping the serialised JSON in `Zeroizing<Vec<u8>>`
    // mirrors the seed-blob site above and ensures the plaintext bytes —
    // which embed the wallet's xpubs — are wiped when `bytes` drops at
    // the end of this block. The descriptor halves aren't as sensitive
    // as the mnemonic, but xpubs give watch-only spend access to every
    // address on the wallet and match the privacy discipline applied
    // to `DescriptorBlob`'s redacting Debug impl.
    let (desc_ct, desc_fp) = if include_descriptor {
        let blob = descriptor_blob.as_ref().unwrap();
        let bytes: Zeroizing<Vec<u8>> = Zeroizing::new(
            serde_json::to_vec(blob).map_err(|e| format!("serialize descriptor: {}", e))?,
        );
        let ct = recovery::encrypt(&bytes, &password, KdfParams::DEFAULT_V1)
            .map_err(|e| format!("encrypt descriptor: {}", e))?;
        let fp = descriptor_blob_fingerprint(blob);
        (Some(ct), fp)
    } else {
        (None, None)
    };

    let seed_ref = seed_ct.as_deref();
    let desc_ref = desc_ct.as_deref();
    let kit: ApiRecoveryKit = client
        .put_recovery_kit(
            cube_id_num,
            seed_ref,
            desc_ref,
            RECOVERY_KIT_SCHEME_AES_256_GCM,
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(RecoveryKitUploadOutcome {
        updated_at: kit.updated_at,
        now_has_seed: !kit.encrypted_cube_seed.is_empty(),
        now_has_descriptor: !kit.encrypted_wallet_descriptor.is_empty(),
        descriptor_fingerprint: desc_fp,
    })
}

/// Which per-method descriptor drift slot a persist targets (per-method drift,
/// PR 3). The password kit and the phone (Keychain) envelope each cache their
/// own last-sealed descriptor fingerprint so drift is judged independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DescriptorFingerprintSlot {
    Password,
    Keychain,
}

/// Write one method's cached descriptor fingerprint onto a `CubeSettings`. Pure
/// (no IO) so the slot routing is unit-testable; `persist_descriptor_fingerprint`
/// and `clear_descriptor_fingerprints` apply it inside the settings-file update.
fn apply_descriptor_fingerprint(
    cube: &mut settings::CubeSettings,
    slot: DescriptorFingerprintSlot,
    fingerprint: Option<String>,
) {
    match slot {
        DescriptorFingerprintSlot::Password => {
            cube.recovery_kit_last_backed_up_descriptor_fingerprint = fingerprint;
        }
        DescriptorFingerprintSlot::Keychain => {
            cube.recovery_kit_last_backed_up_keychain_descriptor_fingerprint = fingerprint;
        }
    }
}

fn persist_descriptor_fingerprint(
    cache: &Cache,
    local_cube_id: &str,
    slot: DescriptorFingerprintSlot,
    fingerprint: Option<String>,
) -> Task<Message> {
    let network_dir = cache.datadir_path.network_directory(cache.network);
    let cube_id = local_cube_id.to_string();
    Task::perform(
        async move {
            update_settings_file(&network_dir, |mut s| {
                if let Some(cube) = s.cubes.iter_mut().find(|c| c.id == cube_id) {
                    apply_descriptor_fingerprint(cube, slot, fingerprint.clone());
                }
                Some(s)
            })
            .await
            .map_err(|e| format!("Failed to update settings: {}", e))
        },
        |res: Result<(), String>| match res {
            Ok(()) => Message::SettingsSaved,
            Err(e) => Message::View(view::Message::ShowError(e)),
        },
    )
}

/// Clear **both** per-method descriptor drift slots in a single settings write —
/// the Remove-all path leaves nothing backed up to compare against, by either
/// method (per-method drift, PR 3).
fn clear_descriptor_fingerprints(cache: &Cache, local_cube_id: &str) -> Task<Message> {
    let network_dir = cache.datadir_path.network_directory(cache.network);
    let cube_id = local_cube_id.to_string();
    Task::perform(
        async move {
            update_settings_file(&network_dir, |mut s| {
                if let Some(cube) = s.cubes.iter_mut().find(|c| c.id == cube_id) {
                    apply_descriptor_fingerprint(cube, DescriptorFingerprintSlot::Password, None);
                    apply_descriptor_fingerprint(cube, DescriptorFingerprintSlot::Keychain, None);
                }
                Some(s)
            })
            .await
            .map_err(|e| format!("Failed to update settings: {}", e))
        },
        |res: Result<(), String>| match res {
            Ok(()) => Message::SettingsSaved,
            Err(e) => Message::View(view::Message::ShowError(e)),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::recovery::{
        DescriptorBlob, DescriptorBlobCube, DescriptorBlobSigner, DescriptorBlobVault,
    };

    fn status_complete() -> RecoveryKitStatus {
        RecoveryKitStatus {
            has_recovery_kit: true,
            has_encrypted_seed: true,
            has_encrypted_wallet_descriptor: true,
            encryption_scheme: "aes-256-gcm".into(),
            created_at: Some("2026-04-23T00:00:00Z".into()),
            updated_at: Some("2026-04-23T00:00:00Z".into()),
            owner_self: None,
        }
    }

    fn status_seed_only() -> RecoveryKitStatus {
        RecoveryKitStatus {
            has_recovery_kit: true,
            has_encrypted_seed: true,
            has_encrypted_wallet_descriptor: false,
            encryption_scheme: "aes-256-gcm".into(),
            created_at: Some("2026-04-23T00:00:00Z".into()),
            updated_at: Some("2026-04-23T00:00:00Z".into()),
            owner_self: None,
        }
    }

    fn status_absent() -> RecoveryKitStatus {
        RecoveryKitStatus {
            has_recovery_kit: false,
            has_encrypted_seed: false,
            has_encrypted_wallet_descriptor: false,
            encryption_scheme: String::new(),
            created_at: None,
            updated_at: None,
            owner_self: None,
        }
    }

    // Regression tests for the delete-is-idempotent behaviour: a
    // second delete of a already-removed kit (or any race where the
    // server no longer has the kit) should count as success so the
    // local-fingerprint cache gets cleared and the card refreshes —
    // not dead-end on an error screen that would leave a stale cache.

    #[test]
    fn normalize_delete_ok_stays_ok() {
        assert!(matches!(normalize_delete_result(Ok(())), Ok(())));
    }

    #[test]
    fn normalize_delete_not_found_becomes_ok() {
        // Second-delete / race case — treat as the desired end state.
        assert!(matches!(
            normalize_delete_result(Err(CoincubeError::NotFound)),
            Ok(())
        ));
    }

    #[test]
    fn normalize_delete_rate_limited_propagates() {
        // Server is asking us to back off — not idempotent success.
        let err = normalize_delete_result(Err(CoincubeError::RateLimited {
            retry_after: std::time::Duration::from_secs(30),
        }));
        assert!(err.is_err());
    }

    #[test]
    fn normalize_delete_api_error_propagates() {
        let err = normalize_delete_result(Err(CoincubeError::Api("boom".to_string())));
        assert!(matches!(err, Err(ref msg) if msg.contains("boom")));
    }

    // Regression tests for the upload-error-preserves-password-entry
    // flow: a transient upload failure must restore the user's
    // `PasswordEntry` screen with inputs intact, not drop them on
    // the floor and force a full PIN + password re-type.

    fn sample_pending() -> PasswordEntryPending {
        PasswordEntryPending {
            mode: RecoveryKitMode::Create,
            mnemonic: Some(Zeroizing::new(vec!["abandon".to_string(); 12])),
            password: Zeroizing::new("correct-horse-battery-staple".to_string()),
            confirm: Zeroizing::new("correct-horse-battery-staple".to_string()),
            acknowledged: true,
        }
    }

    // Regression tests for the Debug redaction on
    // `PasswordEntryPending` and `RecoveryKitState`. Without manual
    // impls, the derived versions would deref through `Zeroizing`
    // and print the password + mnemonic words to any `{:?}` site,
    // defeating the whole reason those fields are Zeroizing-wrapped.
    // Canary strings are unlikely to collide with `<redacted>` or
    // field names.

    const PASSWORD_CANARY: &str = "pwd-canary-XYZZY-XYZZY-XYZZY";
    const MNEMONIC_CANARY: &str = "mnemonic-canary-word-XYZZY";

    fn pending_with_canaries() -> PasswordEntryPending {
        PasswordEntryPending {
            mode: RecoveryKitMode::Create,
            mnemonic: Some(Zeroizing::new(vec![MNEMONIC_CANARY.to_string(); 12])),
            password: Zeroizing::new(PASSWORD_CANARY.to_string()),
            confirm: Zeroizing::new(PASSWORD_CANARY.to_string()),
            acknowledged: true,
        }
    }

    #[test]
    fn password_entry_pending_debug_redacts_secrets() {
        let p = pending_with_canaries();
        let r = format!("{:?}", p);
        assert!(
            !r.contains(PASSWORD_CANARY),
            "password leaked through PasswordEntryPending Debug: {}",
            r
        );
        assert!(
            !r.contains(MNEMONIC_CANARY),
            "mnemonic leaked through PasswordEntryPending Debug: {}",
            r
        );
        // Non-sensitive metadata is preserved for diagnostics.
        assert!(r.contains("Create"), "mode should be visible: {}", r);
        assert!(
            r.contains("acknowledged: true"),
            "acknowledged should be visible: {}",
            r
        );
    }

    #[test]
    fn recovery_kit_state_password_entry_debug_redacts_secrets() {
        let state = RecoveryKitState::PasswordEntry {
            mode: RecoveryKitMode::Create,
            mnemonic: Some(Zeroizing::new(vec![MNEMONIC_CANARY.to_string(); 12])),
            password: Zeroizing::new(PASSWORD_CANARY.to_string()),
            confirm: Zeroizing::new(PASSWORD_CANARY.to_string()),
            acknowledged: false,
            error: Some("wrong password".to_string()),
        };
        let r = format!("{:?}", state);
        assert!(
            !r.contains(PASSWORD_CANARY),
            "password leaked through PasswordEntry Debug: {}",
            r
        );
        assert!(
            !r.contains(MNEMONIC_CANARY),
            "mnemonic leaked through PasswordEntry Debug: {}",
            r
        );
        // Error messages are diagnostic and non-secret; surface them.
        assert!(
            r.contains("wrong password"),
            "error field should be visible: {}",
            r
        );
    }

    #[test]
    fn recovery_kit_state_uploading_debug_redacts_via_pending() {
        // The `Uploading { pending }` branch delegates to
        // `PasswordEntryPending`'s redacting Debug; verify the
        // composite doesn't re-expose the secrets.
        let state = RecoveryKitState::Uploading {
            pending: pending_with_canaries(),
        };
        let r = format!("{:?}", state);
        assert!(!r.contains(PASSWORD_CANARY), "pwd leaked: {}", r);
        assert!(!r.contains(MNEMONIC_CANARY), "mnemonic leaked: {}", r);
    }

    #[test]
    fn upload_error_restores_password_entry_from_uploading_snapshot() {
        let mut flow = RecoveryKitState::Uploading {
            pending: sample_pending(),
        };
        restore_entry_on_upload_error(&mut flow, "Connect timed out");
        match flow {
            RecoveryKitState::PasswordEntry {
                mode,
                mnemonic,
                password,
                confirm,
                acknowledged,
                error,
            } => {
                assert_eq!(mode, RecoveryKitMode::Create);
                assert!(mnemonic.is_some(), "mnemonic must survive the round-trip");
                assert_eq!(mnemonic.as_ref().unwrap().len(), 12);
                assert_eq!(password.as_str(), "correct-horse-battery-staple");
                assert_eq!(confirm.as_str(), "correct-horse-battery-staple");
                assert!(acknowledged);
                assert_eq!(error.as_deref(), Some("Connect timed out"));
            }
            other => panic!("expected PasswordEntry, got {:?}", other),
        }
    }

    #[test]
    fn upload_error_from_non_uploading_state_falls_back_to_error() {
        // State drift — e.g. a stale `UploadResult` arrives after
        // the user hit Cancel and the flow is already `None`.
        // Don't crash or silently revert; surface the error.
        let mut flow = RecoveryKitState::None;
        restore_entry_on_upload_error(&mut flow, "stale upload result");
        assert!(matches!(
            flow,
            RecoveryKitState::Error { ref message } if message == "stale upload result"
        ));
    }

    #[test]
    fn set_protection_error_shows_inline_on_choice_screen() {
        // On the protection-choice screen a synchronous phone-seal
        // bail-out should surface inline so the user stays put.
        let mut rk = RecoveryKit::new();
        rk.flow = RecoveryKitState::ProtectionChoice {
            mode: RecoveryKitMode::Create,
            mnemonic: None,
            phone: PhoneKey::Absent,
            error: None,
        };
        set_protection_error(&mut rk, "Sign in to Connect to back up to your phone.");
        assert!(matches!(
            rk.flow,
            RecoveryKitState::ProtectionChoice { ref error, .. }
                if error.as_deref() == Some("Sign in to Connect to back up to your phone.")
        ));
    }

    #[test]
    fn set_protection_error_from_uploading_falls_back_to_error() {
        // The "Both" chain reaches `start_phone_seal` with the flow in
        // `Uploading` (the password kit already uploaded). A synchronous
        // bail-out there must NOT silently no-op — that left the wizard
        // stuck on the uploading screen forever — but transition to the
        // terminal error state so the failure is visible.
        let mut rk = RecoveryKit::new();
        rk.flow = RecoveryKitState::Uploading {
            pending: sample_pending(),
        };
        set_protection_error(&mut rk, "Failed to read settings file.");
        assert!(matches!(
            rk.flow,
            RecoveryKitState::Error { ref message } if message == "Failed to read settings file."
        ));
    }

    #[test]
    fn fail_phone_preflight_restores_mnemonic_on_choice_screen() {
        // Direct Phone path: `SelectProtectionMode` lifts the mnemonic
        // out of the choice screen for the seal, so the flow is
        // `ProtectionChoice` with `mnemonic: None`. A preflight bail-out
        // must put the mnemonic back (so a retry can still escrow the
        // seed) AND surface the error inline.
        let mut rk = RecoveryKit::new();
        rk.flow = RecoveryKitState::ProtectionChoice {
            mode: RecoveryKitMode::Create,
            mnemonic: None,
            phone: PhoneKey::Absent,
            error: None,
        };
        let mnemonic = Some(Zeroizing::new(vec!["word".to_string(); 12]));
        let _ = fail_phone_preflight(
            &mut rk,
            mnemonic,
            "Sign in to Connect to back up to your phone.",
        );
        match &rk.flow {
            RecoveryKitState::ProtectionChoice {
                mnemonic, error, ..
            } => {
                assert_eq!(
                    mnemonic.as_ref().map(|m| m.len()),
                    Some(12),
                    "mnemonic must be restored so a retry can still escrow the seed"
                );
                assert_eq!(
                    error.as_deref(),
                    Some("Sign in to Connect to back up to your phone.")
                );
            }
            other => panic!("expected ProtectionChoice, got {:?}", other),
        }
    }

    #[test]
    fn fail_phone_preflight_from_uploading_transitions_to_error() {
        // "Both" chain: the password kit already uploaded and the flow is
        // `Uploading`. A preflight bail-out must not strand the wizard
        // there — it transitions to the terminal error state (the lifted
        // mnemonic is no longer needed and is dropped).
        let mut rk = RecoveryKit::new();
        rk.flow = RecoveryKitState::Uploading {
            pending: sample_pending(),
        };
        let mnemonic = Some(Zeroizing::new(vec!["word".to_string(); 12]));
        let _ = fail_phone_preflight(&mut rk, mnemonic, "Failed to read settings file.");
        assert!(matches!(
            rk.flow,
            RecoveryKitState::Error { ref message } if message == "Failed to read settings file."
        ));
    }

    // Regression tests for the Start-handler auth gate: the wizard
    // must not push the user into PIN/password entry when the
    // upload is guaranteed to fail at the end (not signed in, or
    // the Cube isn't registered yet). Without these, users waste
    // re-entering their PIN + typing a password only to hit "Sign
    // in to Connect" on Submit.

    #[test]
    fn start_guard_fails_without_client() {
        assert_eq!(start_guard(None, Some(42)), StartGuard::NotSignedIn);
        assert_eq!(start_guard(None, None), StartGuard::NotSignedIn);
    }

    #[test]
    fn start_guard_fails_without_cube_id() {
        let client = CoincubeClient::for_test("http://localhost");
        assert_eq!(
            start_guard(Some(&client), None),
            StartGuard::CubeNotRegistered
        );
    }

    #[test]
    fn start_guard_ok_when_authed_and_registered() {
        let client = CoincubeClient::for_test("http://localhost");
        assert_eq!(start_guard(Some(&client), Some(42)), StartGuard::Ok);
    }

    // Regression tests for the seed-only-upload fingerprint-clobber
    // bug: a seed-only upload (AddSeed mode on a mnemonic cube) must
    // not wipe a previously-stored drift fingerprint. Otherwise the
    // descriptor that's still on the server becomes invisible to
    // W12 drift detection.

    #[test]
    fn next_fingerprint_none_on_seed_only_upload() {
        let outcome = RecoveryKitUploadOutcome {
            updated_at: "2026-04-23T00:00:00Z".into(),
            now_has_seed: true,
            now_has_descriptor: false,
            descriptor_fingerprint: None,
        };
        assert!(
            next_fingerprint_to_persist(&outcome).is_none(),
            "seed-only upload must signal 'skip persist' so the \
             previously-stored fingerprint is preserved",
        );
    }

    #[test]
    fn next_fingerprint_some_when_descriptor_uploaded() {
        let outcome = RecoveryKitUploadOutcome {
            updated_at: "2026-04-23T00:00:00Z".into(),
            now_has_seed: true,
            now_has_descriptor: true,
            descriptor_fingerprint: Some("a".repeat(64)),
        };
        assert_eq!(next_fingerprint_to_persist(&outcome), Some("a".repeat(64)));
    }

    // ── Per-method descriptor drift slots (plan §PR3) ───────────────────────
    //
    // The password kit and the phone (Keychain) envelope each cache their own
    // last-sealed descriptor fingerprint so drift is judged independently. These
    // pin the slot routing `persist_descriptor_fingerprint` /
    // `clear_descriptor_fingerprints` apply inside the settings-file update.

    fn cube_with_both_slots() -> settings::CubeSettings {
        let mut cube = settings::CubeSettings::new("Slots".to_string(), Network::Bitcoin);
        cube.recovery_kit_last_backed_up_descriptor_fingerprint = Some("pw-fp".to_string());
        cube.recovery_kit_last_backed_up_keychain_descriptor_fingerprint =
            Some("kc-fp".to_string());
        cube
    }

    #[test]
    fn keychain_persist_writes_only_the_keychain_slot() {
        // A phone seal persists the keychain slot and must not touch the
        // password slot (they're independent restore paths).
        let mut cube = settings::CubeSettings::new("C".to_string(), Network::Bitcoin);
        cube.recovery_kit_last_backed_up_descriptor_fingerprint = Some("pw-fp".to_string());
        apply_descriptor_fingerprint(
            &mut cube,
            DescriptorFingerprintSlot::Keychain,
            Some("kc-fp".to_string()),
        );
        assert_eq!(
            cube.recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                .as_deref(),
            Some("kc-fp")
        );
        assert_eq!(
            cube.recovery_kit_last_backed_up_descriptor_fingerprint
                .as_deref(),
            Some("pw-fp"),
            "keychain persist must leave the password slot intact"
        );
    }

    #[test]
    fn password_persist_writes_only_the_password_slot() {
        let mut cube = settings::CubeSettings::new("C".to_string(), Network::Bitcoin);
        cube.recovery_kit_last_backed_up_keychain_descriptor_fingerprint =
            Some("kc-fp".to_string());
        apply_descriptor_fingerprint(
            &mut cube,
            DescriptorFingerprintSlot::Password,
            Some("pw-fp".to_string()),
        );
        assert_eq!(
            cube.recovery_kit_last_backed_up_descriptor_fingerprint
                .as_deref(),
            Some("pw-fp")
        );
        assert_eq!(
            cube.recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                .as_deref(),
            Some("kc-fp"),
            "password persist must leave the keychain slot intact"
        );
    }

    #[test]
    fn remove_clears_both_slots() {
        // `clear_descriptor_fingerprints` applies both clears in one write —
        // Remove-all leaves nothing to drift against by either method.
        let mut cube = cube_with_both_slots();
        apply_descriptor_fingerprint(&mut cube, DescriptorFingerprintSlot::Password, None);
        apply_descriptor_fingerprint(&mut cube, DescriptorFingerprintSlot::Keychain, None);
        assert!(cube
            .recovery_kit_last_backed_up_descriptor_fingerprint
            .is_none());
        assert!(cube
            .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
            .is_none());
    }

    #[test]
    fn seal_persists_the_sealed_descriptor_fingerprint() {
        // The keychain slot the seal writes is exactly `descriptor_blob_fingerprint`
        // of the sealed blob (the same SHA-256 helper the password path uses), so
        // a later re-render compares like-for-like and reports no spurious drift.
        let blob = descriptor_some().unwrap();
        let sealed_fp = descriptor_blob_fingerprint(&blob).unwrap();
        let mut cube = settings::CubeSettings::new("C".to_string(), Network::Bitcoin);
        apply_descriptor_fingerprint(
            &mut cube,
            DescriptorFingerprintSlot::Keychain,
            Some(sealed_fp.clone()),
        );
        assert_eq!(
            cube.recovery_kit_last_backed_up_keychain_descriptor_fingerprint,
            Some(sealed_fp)
        );
    }

    // Regression tests for the W10 post-vault-creation nudge: the
    // decision must be based on a *freshly-loaded* status, not the
    // in-memory cache at the moment the vault transition fires. A
    // pre-fetch `None` used to produce a spurious nudge for users
    // with a complete kit.

    #[test]
    fn nudge_when_no_kit_on_server() {
        assert!(should_nudge_for_status(Some(&status_absent())));
    }

    #[test]
    fn nudge_when_kit_has_seed_but_no_descriptor() {
        assert!(should_nudge_for_status(Some(&status_seed_only())));
    }

    #[test]
    fn no_nudge_when_kit_already_has_descriptor() {
        assert!(!should_nudge_for_status(Some(&status_complete())));
    }

    #[test]
    fn nudge_when_status_is_none_defensively() {
        // `None` should not normally reach this function after a
        // successful `StatusLoaded(Ok(_))` (the handler assigns
        // `rk.status` before calling). Defensive default is to
        // nudge rather than silently drop.
        assert!(should_nudge_for_status(None));
    }

    // Regression test for the Bitcoin-mainnet restore bug: the local
    // network string used by the restore step's cube-picker filter
    // (and by blob content) must match the Connect API's canonical
    // form — otherwise mainnet users see zero matching cubes when
    // trying to restore.
    #[test]
    fn network_str_matches_api_for_all_networks() {
        use coincube_core::miniscript::bitcoin::Network as BtcNet;
        assert_eq!(network_str(BtcNet::Bitcoin), "mainnet");
        assert_eq!(network_str(BtcNet::Testnet), "testnet");
        assert_eq!(network_str(BtcNet::Testnet4), "testnet4");
        assert_eq!(network_str(BtcNet::Signet), "signet");
        assert_eq!(network_str(BtcNet::Regtest), "regtest");
    }

    #[test]
    fn descriptor_fingerprint_is_deterministic() {
        let blob = DescriptorBlob {
            version: BLOB_VERSION,
            cube: DescriptorBlobCube {
                uuid: "u".into(),
                network: "bitcoin".into(),
            },
            vault: DescriptorBlobVault {
                name: "n".into(),
                descriptor: "d".into(),
                change_descriptor: None,
                signers: vec![DescriptorBlobSigner {
                    name: "s".into(),
                    fingerprint: "deadbeef".into(),
                    xpub: String::new(),
                }],
            },
        };
        let a = descriptor_blob_fingerprint(&blob).unwrap();
        let b = descriptor_blob_fingerprint(&blob).unwrap();
        assert_eq!(a, b, "fingerprint must be deterministic");
        assert_eq!(a.len(), 64, "SHA-256 hex is 64 chars");
    }

    fn mnemonic_some() -> Option<Zeroizing<Vec<String>>> {
        Some(Zeroizing::new(vec!["word".to_string(); 12]))
    }

    fn descriptor_some() -> Option<DescriptorBlob> {
        Some(DescriptorBlob {
            version: BLOB_VERSION,
            cube: DescriptorBlobCube {
                uuid: "u".into(),
                network: "bitcoin".into(),
            },
            vault: DescriptorBlobVault {
                name: "n".into(),
                descriptor: "d".into(),
                change_descriptor: None,
                signers: vec![],
            },
        })
    }

    // Regression tests for the AddDescriptor/AddSeed re-encryption
    // bug: the upload decision must NOT be gated on mode, otherwise
    // the two halves end up encrypted under different passwords.
    // Plan §5 requires every update to re-encrypt all present blobs
    // under the new password.

    #[test]
    fn include_halves_add_descriptor_uploads_seed_too() {
        // User is in AddDescriptor mode on a mnemonic cube. The PIN
        // verify populated `mnemonic`, and the local Vault provides
        // `descriptor_blob`. Both halves must go up so the server
        // state ends up with BOTH encrypted under the new password.
        let m = mnemonic_some();
        let d = descriptor_some();
        assert_eq!(include_halves(&m, &d), (true, true));
    }

    #[test]
    fn include_halves_add_seed_uploads_descriptor_too() {
        // User is in AddSeed mode on a mnemonic cube with a Vault.
        // Both halves must go up — the seed under the new password
        // (the whole point of AddSeed) AND the descriptor re-encrypted
        // from the live wallet under the new password.
        let m = mnemonic_some();
        let d = descriptor_some();
        assert_eq!(include_halves(&m, &d), (true, true));
    }

    #[test]
    fn include_halves_passkey_no_seed() {
        // Passkey cubes never populate the mnemonic — the seed is
        // unextractable on-device. Only the descriptor goes up.
        let m: Option<Zeroizing<Vec<String>>> = None;
        let d = descriptor_some();
        assert_eq!(include_halves(&m, &d), (false, true));
    }

    #[test]
    fn include_halves_no_vault_seed_only() {
        // Mnemonic cube without a Vault (or user hasn't created one
        // yet). Seed goes up alone; descriptor will be added later
        // via AddDescriptor.
        let m = mnemonic_some();
        let d: Option<DescriptorBlob> = None;
        assert_eq!(include_halves(&m, &d), (true, false));
    }

    #[test]
    fn include_halves_nothing_is_rejected() {
        let m: Option<Zeroizing<Vec<String>>> = None;
        let d: Option<DescriptorBlob> = None;
        assert_eq!(include_halves(&m, &d), (false, false));
    }

    #[test]
    fn descriptor_fingerprint_differs_on_content_change() {
        let mut blob = DescriptorBlob {
            version: BLOB_VERSION,
            cube: DescriptorBlobCube {
                uuid: "u".into(),
                network: "bitcoin".into(),
            },
            vault: DescriptorBlobVault {
                name: "n".into(),
                descriptor: "d".into(),
                change_descriptor: None,
                signers: vec![],
            },
        };
        let before = descriptor_blob_fingerprint(&blob).unwrap();
        blob.vault.descriptor = "different".into();
        let after = descriptor_blob_fingerprint(&blob).unwrap();
        assert_ne!(before, after);
    }

    // Regression test for the HashSet-order determinism bug:
    // `Wallet::descriptor_keys()` returns a `HashSet<Fingerprint>`
    // whose iteration order is randomised per-process. If
    // `descriptor_blob_from_wallet` had serialised signers in
    // HashSet order, each App tick would compute a different
    // fingerprint and fire spurious W12 drift banners every time
    // the live fingerprint was re-hashed. The fix is to sort the
    // fingerprints by lowercase hex before building the signer
    // list; this test pins the underlying invariant that justifies
    // that sort.
    fn signer(fp_hex: &str) -> DescriptorBlobSigner {
        DescriptorBlobSigner {
            name: "Signer".into(),
            fingerprint: fp_hex.to_string(),
            xpub: String::new(),
        }
    }

    #[test]
    fn descriptor_fingerprint_depends_on_signer_order() {
        // Two blobs with identical signer *sets* but different
        // orderings must produce different fingerprints — this is
        // precisely why `descriptor_blob_from_wallet` sorts before
        // building the list.
        let base_vault = DescriptorBlobVault {
            name: "n".into(),
            descriptor: "d".into(),
            change_descriptor: None,
            signers: vec![signer("aaaaaaaa"), signer("bbbbbbbb")],
        };
        let a = DescriptorBlob {
            version: BLOB_VERSION,
            cube: DescriptorBlobCube {
                uuid: "u".into(),
                network: "bitcoin".into(),
            },
            vault: base_vault.clone(),
        };
        let mut b = a.clone();
        b.vault.signers.reverse();
        assert_ne!(
            descriptor_blob_fingerprint(&a).unwrap(),
            descriptor_blob_fingerprint(&b).unwrap(),
            "signer order changes the fingerprint — production must sort before hashing",
        );
    }

    #[test]
    fn descriptor_fingerprint_stable_when_signers_are_presented_in_sorted_order() {
        // Two independent Vec<DescriptorBlobSigner> constructions
        // that both follow the "sort by lowercase hex" convention
        // produce identical fingerprints — confirms the sort
        // strategy used by `descriptor_blob_from_wallet`.
        fn build_sorted(fps: &[&str]) -> DescriptorBlob {
            let mut v: Vec<&str> = fps.to_vec();
            v.sort_by_key(|s| s.to_lowercase());
            DescriptorBlob {
                version: BLOB_VERSION,
                cube: DescriptorBlobCube {
                    uuid: "u".into(),
                    network: "bitcoin".into(),
                },
                vault: DescriptorBlobVault {
                    name: "n".into(),
                    descriptor: "d".into(),
                    change_descriptor: None,
                    signers: v.into_iter().map(signer).collect(),
                },
            }
        }
        // Two callers pass fingerprints in different "observed"
        // orderings (simulating two HashSet iteration orders), but
        // both sort before building — fingerprints must match.
        let a = build_sorted(&["cafebabe", "deadbeef", "aabbccdd"]);
        let b = build_sorted(&["aabbccdd", "deadbeef", "cafebabe"]);
        assert_eq!(
            descriptor_blob_fingerprint(&a).unwrap(),
            descriptor_blob_fingerprint(&b).unwrap(),
        );
    }

    // ── Remove-clears-everything behind confirmation (plan §PR2, master F5) ──

    use crate::services::coincube::OwnerSelfRecoverySummary;
    use httpmock::{Method as MockMethod, MockServer};
    use serde_json::json;

    fn status_both_modes() -> RecoveryKitStatus {
        RecoveryKitStatus {
            owner_self: Some(OwnerSelfRecoverySummary {
                has_recipient: true,
                tier: "full_cube".into(),
                envelope_kinds: vec!["seed".into(), "descriptor".into()],
                updated_at: Some("2026-06-01T00:00:00Z".into()),
            }),
            ..status_complete()
        }
    }

    /// One `owner-self` recipient row (keyId 7) as the recipients endpoint
    /// returns it; `remove_backups` reads `keyId` to target the DELETE.
    fn owner_self_recipients_body() -> serde_json::Value {
        json!({
            "success": true,
            "data": [{ "id": 1, "keyId": 7, "role": "owner-self", "tier": "full_cube" }]
        })
    }

    // ---- Remove is confirm-gated: it deletes nothing, Cancel is intact ----

    #[test]
    fn remove_enters_confirm_and_deletes_nothing() {
        // Pressing Remove must NOT fire a delete — it only takes over the page
        // with the confirmation screen. The handler never touches the client,
        // so no network call is possible from this transition.
        let mut rk = RecoveryKit::new();
        rk.status = Some(status_both_modes());
        let _ = update(
            &mut rk,
            RecoveryKitMessage::Remove,
            &Cache::default(),
            "cube-1",
            SeedSource::Mnemonic,
            None,
            Some(42),
            None,
        );
        assert!(matches!(rk.flow, RecoveryKitState::ConfirmRemove));
        // Status is untouched — the confirm view reads it to name the paths.
        assert!(rk.status.is_some());
    }

    #[test]
    fn cancel_from_confirm_restores_card_with_status_intact() {
        let mut rk = RecoveryKit::new();
        rk.status = Some(status_both_modes());
        rk.flow = RecoveryKitState::ConfirmRemove;
        let _ = update(
            &mut rk,
            RecoveryKitMessage::Cancel,
            &Cache::default(),
            "cube-1",
            SeedSource::Mnemonic,
            None,
            Some(42),
            None,
        );
        assert!(matches!(rk.flow, RecoveryKitState::None));
        let s = rk.status.as_ref().expect("status must survive Cancel");
        assert!(s.has_encrypted_seed && s.has_encrypted_wallet_descriptor);
    }

    #[test]
    fn remove_failure_refreshes_status_for_partial_removal() {
        // A partial removal (password kit deleted, then the phone step failed)
        // leaves the cached status stale — Connect no longer has the password
        // kit. The error handler must kick off a status refresh so the card
        // reflects reality once the user dismisses the error, rather than keep
        // showing a password backup that's already gone.
        let mut rk = RecoveryKit::new();
        rk.status = Some(status_both_modes());
        rk.flow = RecoveryKitState::Removing;
        assert!(!rk.status_loading);
        let client = CoincubeClient::for_test("http://localhost");
        let _ = update(
            &mut rk,
            RecoveryKitMessage::RemoveResult(Err(RemoveFailure {
                message: "phone step failed".to_string(),
                password_kit_deleted: true,
            })),
            &Cache::default(),
            "cube-1",
            SeedSource::Mnemonic,
            Some(client),
            Some(42),
            None,
        );
        assert!(matches!(rk.flow, RecoveryKitState::Error { .. }));
        assert!(
            rk.status_loading,
            "RemoveResult(Err) must refresh status so the card drops the deleted kit"
        );
    }

    // ---- remove_backups delete set (the ConfirmRemove teardown) ----

    #[tokio::test]
    async fn remove_backups_keychain_only_tolerates_kit_404_and_deletes_recipient() {
        // Envelope-only Cube: the password-kit DELETE 404s (tolerated), then the
        // owner-self recipient is resolved and DELETEd (cascading its envelopes).
        let server = MockServer::start();
        let kit = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(404);
        });
        let list = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(owner_self_recipients_body());
        });
        let recip = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients/7");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({ "success": true, "data": { "deleted": true } }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        remove_backups(client, 42, true)
            .await
            .expect("keychain-only remove should succeed");
        kit.assert();
        list.assert();
        recip.assert();
    }

    #[tokio::test]
    async fn remove_backups_both_modes_deletes_kit_and_recipient() {
        let server = MockServer::start();
        let kit = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({ "success": true, "data": { "status": "deleted" } }));
        });
        let list = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(owner_self_recipients_body());
        });
        let recip = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients/7");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({ "success": true, "data": { "deleted": true } }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        remove_backups(client, 42, true)
            .await
            .expect("both-modes remove should succeed");
        kit.assert();
        list.assert();
        recip.assert();
    }

    #[tokio::test]
    async fn remove_backups_password_only_deletes_kit_and_skips_recipient() {
        let server = MockServer::start();
        let kit = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({ "success": true, "data": { "status": "deleted" } }));
        });
        // No keychain half → the recipients endpoint must never be touched. We
        // still verify no phone copy survives before reporting success (F5), so
        // `/status` is fetched and confirms nothing is sealed to the phone.
        let list = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients");
            then.status(200).json_body(owner_self_recipients_body());
        });
        let status = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/status");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "hasRecoveryKit": false,
                        "hasEncryptedSeed": false,
                        "hasEncryptedWalletDescriptor": false,
                        "encryptionScheme": "",
                        "ownerSelf": null
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        remove_backups(client, 42, false)
            .await
            .expect("password-only remove should succeed");
        kit.assert();
        status.assert();
        assert_eq!(list.hits(), 0, "recipient path must not be resolved");
    }

    #[tokio::test]
    async fn remove_backups_password_only_surviving_phone_copy_fails_loudly() {
        // `delete_keychain == false` because the cached overview showed no phone
        // copy — but the status re-fetch reveals owner-self envelopes still
        // present (a stale/racy overview hid a live copy). F5: this must fail
        // loudly rather than report success and let the caller clear the
        // keychain drift slot over a restorable copy. The recipients endpoint is
        // never touched (we skipped the teardown), and the message tells the
        // user to retry — a fresh Remove would target the now-visible copy.
        let server = MockServer::start();
        let kit = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({ "success": true, "data": { "status": "deleted" } }));
        });
        let list = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients");
            then.status(200).json_body(owner_self_recipients_body());
        });
        let status = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/status");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "hasRecoveryKit": false,
                        "hasEncryptedSeed": false,
                        "hasEncryptedWalletDescriptor": false,
                        "encryptionScheme": "",
                        "ownerSelf": {
                            "hasRecipient": true,
                            "tier": "",
                            "envelopeKinds": ["descriptor"]
                        }
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = remove_backups(client, 42, false)
            .await
            .expect_err("a stale-overview-hidden phone copy must fail loudly (F5)");
        kit.assert();
        status.assert();
        assert_eq!(list.hits(), 0, "recipient path must not be resolved");
        assert!(
            err.password_kit_deleted,
            "password kit was deleted before the verify failed — the caller must drop its slot"
        );
        assert!(
            err.message.contains("phone") || err.message.contains("Keychain"),
            "error must name the surviving phone path: {}",
            err.message
        );
        assert!(
            err.message.contains("try Remove again"),
            "stale-overview survivor must advise a retry, not contact support: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn remove_backups_kit_delete_failure_names_password_and_leaves_envelopes() {
        // A hard (non-404) kit-delete error must fail loudly, name the password
        // path, and return BEFORE touching the recipient — envelopes untouched.
        let server = MockServer::start();
        let kit = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(500);
        });
        let list = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients");
            then.status(200).json_body(owner_self_recipients_body());
        });
        let recip = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients/7");
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = remove_backups(client, 42, true)
            .await
            .expect_err("hard kit-delete error must fail loudly");
        kit.assert();
        assert!(
            !err.password_kit_deleted,
            "the delete itself failed, so the kit is still on Connect — the local slot stays"
        );
        assert!(
            err.message.contains("password"),
            "error must name the password path: {}",
            err.message
        );
        assert_eq!(
            list.hits(),
            0,
            "must not resolve recipient after kit failure"
        );
        assert_eq!(recip.hits(), 0, "envelopes must be left untouched");
    }

    #[tokio::test]
    async fn remove_backups_recipient_delete_failure_after_kit_names_phone() {
        // Kit deleted OK, then the recipient DELETE fails — the error must name
        // the phone (Keychain) path that's still restorable (F5).
        let server = MockServer::start();
        let kit = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(200)
                .json_body(json!({ "success": true, "data": { "status": "deleted" } }));
        });
        let list = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(owner_self_recipients_body());
        });
        let recip = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients/7");
            then.status(500);
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = remove_backups(client, 42, true)
            .await
            .expect_err("recipient-delete failure must fail loudly");
        kit.assert();
        list.assert();
        recip.assert();
        assert!(
            err.password_kit_deleted,
            "password kit was deleted before the recipient step failed — its slot must drop"
        );
        assert!(
            err.message.contains("phone") || err.message.contains("Keychain"),
            "error must name the phone path: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn remove_backups_missing_recipient_verified_gone_is_success() {
        // Keychain flagged enabled but the recipient is already gone (404 on the
        // list). The status re-fetch confirms no phone envelopes survive (the
        // recipient DELETE cascaded them elsewhere) → success.
        let server = MockServer::start();
        let kit = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(404);
        });
        let list = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients");
            then.status(404);
        });
        // Verification: no owner-self envelopes remain → the phone copy is gone.
        let status = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/status");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "hasRecoveryKit": false,
                        "hasEncryptedSeed": false,
                        "hasEncryptedWalletDescriptor": false,
                        "encryptionScheme": "",
                        "ownerSelf": null
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        remove_backups(client, 42, true)
            .await
            .expect("verified-gone phone copy must be idempotent success");
        kit.assert();
        list.assert();
        status.assert();
    }

    #[tokio::test]
    async fn remove_backups_orphaned_envelopes_fail_loudly() {
        // The recipient is gone (404 on the list) but the status re-fetch shows
        // owner-self envelopes still present — an orphaned, still-restorable
        // phone copy the desktop can't delete (no recipient key to target). F5:
        // this must fail loudly, not report success while a path survives.
        let server = MockServer::start();
        let kit = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(404);
        });
        let list = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients");
            then.status(404);
        });
        let status = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/status");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "hasRecoveryKit": false,
                        "hasEncryptedSeed": false,
                        "hasEncryptedWalletDescriptor": false,
                        "encryptionScheme": "",
                        "ownerSelf": {
                            "hasRecipient": false,
                            "tier": "",
                            "envelopeKinds": ["descriptor"]
                        }
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = remove_backups(client, 42, true)
            .await
            .expect_err("a surviving phone copy must fail loudly (F5)");
        kit.assert();
        list.assert();
        status.assert();
        assert!(
            err.password_kit_deleted,
            "the kit 404 counts as torn down (idempotent), so its slot must drop"
        );
        assert!(
            err.message.contains("phone") || err.message.contains("Keychain"),
            "error must name the surviving phone path: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn remove_backups_missing_recipient_unverifiable_fails() {
        // Recipient gone, and the verification status fetch itself fails (500) —
        // we can't confirm the phone copy was cleared, so we must not claim
        // success (F5); the error tells the user to re-check.
        let server = MockServer::start();
        let kit = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(404);
        });
        let list = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients");
            then.status(404);
        });
        let status = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/status");
            then.status(500);
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = remove_backups(client, 42, true)
            .await
            .expect_err("an unconfirmable phone copy must not report success");
        kit.assert();
        list.assert();
        status.assert();
        assert!(
            err.password_kit_deleted,
            "the kit 404 counts as torn down (idempotent), so its slot must drop"
        );
        assert!(
            err.message.contains("confirm"),
            "error must flag the unconfirmed removal: {}",
            err.message
        );
    }
}
