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
}

impl RecoveryKit {
    pub fn new() -> Self {
        Self {
            status: None,
            status_loading: false,
            flow: RecoveryKitState::None,
            pin: PinInput::new(),
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
                    // no state transition.
                    return Task::done(Message::View(view::Message::ShowError(format!(
                        "Couldn't load Recovery Kit status: {}",
                        e
                    ))));
                }
            }
            Task::none()
        }

        RecoveryKitMessage::Start(mode) => {
            rk.pin.clear();
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
                    let persist_fp_task = persist_descriptor_fingerprint(
                        cache,
                        local_cube_id,
                        outcome.descriptor_fingerprint,
                    );
                    rk.flow = RecoveryKitState::Completed {
                        updated_at,
                        now_has_seed,
                        now_has_descriptor,
                    };
                    persist_fp_task
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
    // Destructure without moving fields out of the state — we'll set
    // rk.flow to `Uploading` if all validation passes.
    let (
        mode,
        mnemonic_opt,
        password_copy,
        mnemonic_clone_opt,
        cube_uuid,
        cube_name,
        network,
        lightning_address,
        created_at_str,
    );
    match &rk.flow {
        RecoveryKitState::PasswordEntry {
            mode: m,
            mnemonic,
            password,
            confirm,
            acknowledged,
            ..
        } => {
            // Validate.
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

            mode = *m;
            mnemonic_opt = mnemonic.as_ref().map(|z| z.to_vec());
            mnemonic_clone_opt = mnemonic.clone();
            password_copy = Zeroizing::new(password.to_string());
        }
        _ => return Task::none(),
    }
    let _ = mnemonic_opt; // silence unused warning before shadowing below

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
    cube_uuid = cube.id.clone();
    cube_name = cube.name.clone();
    network = network_str(cube.network);
    lightning_address = cache.lightning_address.clone();
    created_at_str = chrono::DateTime::<chrono::Utc>::from_timestamp(cube.created_at, 0)
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

fn network_str(n: Network) -> String {
    match n {
        Network::Bitcoin => "bitcoin",
        Network::Testnet => "testnet",
        Network::Signet => "signet",
        Network::Regtest => "regtest",
        _ => "unknown",
    }
    .to_string()
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
