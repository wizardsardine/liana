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
use coincube_core::signer::MasterSigner;
use iced::Task;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::app::cache::Cache;
use crate::app::message::Message;
use crate::app::settings::{self, update_settings_file};
use crate::app::view;
use crate::app::view::{RecoveryKitMessage, RecoveryKitMode, RecoveryKitUploadOutcome};
use crate::app::wallet::Wallet;
use crate::pin_input::PinInput;
use crate::services::coincube::{
    CoincubeClient, CoincubeError, RecoveryKit as ApiRecoveryKit, RecoveryKitStatus,
    RECOVERY_KIT_SCHEME_AES_256_GCM,
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

/// The actual UI state of the wizard. `None` = card view; any other
/// variant = wizard is taking over the settings page.
#[derive(Debug)]
pub enum RecoveryKitState {
    None,
    PinEntry {
        mode: RecoveryKitMode,
        error: Option<String>,
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
    Uploading,
    Completed {
        updated_at: String,
        now_has_seed: bool,
        now_has_descriptor: bool,
    },
    Removing,
    Error {
        message: String,
    },
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
}

impl RecoveryKit {
    pub fn new() -> Self {
        Self {
            status: None,
            status_loading: false,
            flow: RecoveryKitState::None,
            pin: PinInput::new(),
            nudge_on_next_status_load: false,
        }
    }

    /// Drop any in-flight flow state (PIN, password, decrypted
    /// mnemonic). Called on Cancel and on wizard completion.
    pub fn reset_flow(&mut self) {
        self.flow = RecoveryKitState::None;
        self.pin.clear();
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
                                  You can sign in from Settings → Connect."
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
                    rk.flow = RecoveryKitState::PasswordEntry {
                        mode,
                        mnemonic: None,
                        password: Zeroizing::new(String::new()),
                        confirm: Zeroizing::new(String::new()),
                        acknowledged: false,
                        error: None,
                    };
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
                    rk.flow = RecoveryKitState::PasswordEntry {
                        mode,
                        mnemonic: Some(Zeroizing::new(words)),
                        password: Zeroizing::new(String::new()),
                        confirm: Zeroizing::new(String::new()),
                        acknowledged: false,
                        error: None,
                    };
                }
                Err(e) => {
                    rk.flow = RecoveryKitState::PinEntry {
                        mode,
                        error: Some(e),
                    };
                }
            }
            Task::none()
        }

        RecoveryKitMessage::PasswordChanged(value) => {
            if let RecoveryKitState::PasswordEntry {
                password, error, ..
            } = &mut rk.flow
            {
                *password = Zeroizing::new(value);
                *error = None;
            }
            Task::none()
        }

        RecoveryKitMessage::ConfirmChanged(value) => {
            if let RecoveryKitState::PasswordEntry { confirm, error, .. } = &mut rk.flow {
                *confirm = Zeroizing::new(value);
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

        RecoveryKitMessage::SubmitPassword => {
            submit_password(rk, cache, client, server_cube_id, wallet)
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
                    rk.flow = RecoveryKitState::Completed {
                        updated_at,
                        now_has_seed,
                        now_has_descriptor,
                    };
                    if let Some(fp) = fp_to_persist {
                        persist_descriptor_fingerprint(cache, local_cube_id, Some(fp))
                    } else {
                        Task::none()
                    }
                }
                Err(e) => {
                    // Preserve any already-entered password so the user
                    // doesn't have to retype it. Error shows inline.
                    if let RecoveryKitState::PasswordEntry { error, .. } = &mut rk.flow {
                        *error = Some(e.clone());
                    } else {
                        // Mid-flight cancel or state drift — fall back
                        // to the Error screen.
                        rk.flow = RecoveryKitState::Error { message: e.clone() };
                    }
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
            rk.flow = RecoveryKitState::Removing;
            Task::perform(
                async move {
                    client
                        .delete_recovery_kit(cube_id)
                        .await
                        .map_err(|e| e.to_string())
                },
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
                    // Clear local drift fingerprint cache — there's
                    // nothing backed up to compare against any more.
                    let persist = persist_descriptor_fingerprint(cache, local_cube_id, None);
                    let reload = load_status(rk, client, server_cube_id);
                    Task::batch([persist, reload])
                }
                Err(e) => {
                    rk.flow = RecoveryKitState::Error { message: e.clone() };
                    Task::done(Message::View(view::Message::ShowError(e)))
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

    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                if !cube.verify_pin(&pin) {
                    return Err("Incorrect PIN. Please try again.".to_string());
                }
                load_mnemonic_words(&datadir, network, fingerprint, &pin)
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

fn load_mnemonic_words(
    datadir: &std::path::Path,
    network: Network,
    fingerprint: coincube_core::miniscript::bitcoin::bip32::Fingerprint,
    pin: &str,
) -> Result<Vec<String>, String> {
    let signer =
        MasterSigner::from_datadir_by_fingerprint(datadir, network, fingerprint, Some(pin))
            .map_err(|e| e.to_string())?;
    Ok(signer.words().iter().map(|w| (*w).to_string()).collect())
}

fn submit_password(
    rk: &mut RecoveryKit,
    cache: &Cache,
    client: Option<CoincubeClient>,
    server_cube_id: Option<u64>,
    wallet: Option<Arc<Wallet>>,
) -> Task<Message> {
    // Pull validation + cloneable inputs out of the state without
    // moving fields — we'll set `rk.flow = Uploading` once all checks
    // pass and the task is on its way.
    let (mode, password_copy, mnemonic_clone_opt) = match &rk.flow {
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
            (*m, Zeroizing::new(password.to_string()), mnemonic.clone())
        }
        _ => return Task::none(),
    };

    // Pull cube-scoped metadata from settings so the blob is complete.
    let network_dir = cache.datadir_path.network_directory(cache.network);
    let Ok(s) = settings::Settings::from_file(&network_dir) else {
        set_pw_error(rk, "Failed to read settings file.");
        return Task::none();
    };
    let Some(cube) = s.cubes.iter().find(|c| c.id == cache.cube_id).cloned() else {
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

    rk.flow = RecoveryKitState::Uploading;

    let mnemonic = mnemonic_clone_opt;
    Task::perform(
        async move {
            encrypt_and_upload(
                client,
                cube_id_num,
                mode,
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

fn descriptor_blob_from_wallet(wallet: &Wallet, cube_uuid: &str, network: &str) -> DescriptorBlob {
    // The descriptor string embeds the full signer xpubs inline, so
    // restore is self-contained from `vault.descriptor` alone. The
    // separate `signers` array is metadata the UI can show pre-restore
    // (fingerprint, friendly name) — xpubs are extracted by the
    // restorer from the descriptor itself. Leaving `xpub` empty here
    // is deliberate: we'd need a miniscript-level traversal to pull
    // per-fingerprint xpubs out cleanly, which is a larger change.
    let signers: Vec<DescriptorBlobSigner> = wallet
        .descriptor_keys()
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

async fn encrypt_and_upload(
    client: CoincubeClient,
    cube_id_num: u64,
    _mode: RecoveryKitMode,
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

    // Seed blob.
    let seed_ct = if include_seed {
        let words = mnemonic.as_ref().unwrap();
        let phrase = words.join(" ");
        let blob = SeedBlob {
            version: BLOB_VERSION,
            cube: cube_meta.clone(),
            mnemonic: SeedBlobMnemonic {
                phrase,
                language: "en".to_string(),
            },
        };
        let bytes = serde_json::to_vec(&blob).map_err(|e| format!("serialize seed: {}", e))?;
        Some(
            recovery::encrypt(&bytes, &password, KdfParams::DEFAULT_V1)
                .map_err(|e| format!("encrypt seed: {}", e))?,
        )
    } else {
        None
    };

    // Descriptor blob.
    let (desc_ct, desc_fp) = if include_descriptor {
        let blob = descriptor_blob.as_ref().unwrap();
        let bytes = serde_json::to_vec(blob).map_err(|e| format!("serialize descriptor: {}", e))?;
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

fn persist_descriptor_fingerprint(
    cache: &Cache,
    local_cube_id: &str,
    fingerprint: Option<String>,
) -> Task<Message> {
    let network_dir = cache.datadir_path.network_directory(cache.network);
    let cube_id = local_cube_id.to_string();
    Task::perform(
        async move {
            update_settings_file(&network_dir, |mut s| {
                if let Some(cube) = s.cubes.iter_mut().find(|c| c.id == cube_id) {
                    cube.recovery_kit_last_backed_up_descriptor_fingerprint = fingerprint.clone();
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
        }
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
}
