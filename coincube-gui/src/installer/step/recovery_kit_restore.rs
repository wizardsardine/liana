//! Installer step: restore a Cube from its Connect-hosted Recovery Kit.
//!
//! Mounts into three entry points:
//!   * W13 — full restore from scratch (fresh install, no local state).
//!     The step populates `ctx.recovered_signer` + `ctx.descriptor`
//!     from the decrypted blobs. Runs inside
//!     `UserFlow::RestoreFromRecoveryKit`.
//!   * W14 — after the user enters a mnemonic in `AddWallet`, offer to
//!     fetch the wallet descriptor from Connect instead of (or in
//!     addition to) importing from a file. Only the descriptor
//!     half is applied; the seed half is ignored because the user
//!     just retyped their mnemonic.
//!   * W15 — running-app "Restore Vault from Connect". Same shape as
//!     W14 but without an accompanying mnemonic entry — the
//!     current Cube already has its seed on disk; we just need
//!     the descriptor.
//!
//! The step is a single Iced `Step` with an internal state machine
//! (`Phase`). It reuses the reusable restore helpers in
//! `services::recovery::restore` so the crypto + API sequencing is
//! audited in one place.

use crate::installer::context::RestoreSource;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use coincube_core::{
    descriptors::CoincubeDescriptor, miniscript::bitcoin::Network, signer::MasterSigner,
};
use coincube_ui::{component::form, widget::Element};
use iced::Task;
use zeroize::Zeroizing;

use crate::{
    hw::HardwareWallets,
    installer::{
        context::Context,
        message::{Message, RecoveryKitRestoreMsg},
        step::Step,
        view,
    },
    services::{
        coincube::{
            CoincubeClient, CoincubeError, CubeResponse, OtpRequest, OtpVerifyRequest,
            RecoveryKitStatus,
        },
        recovery::{
            restore::{fetch_and_decrypt_wallet_kit, RecoveredKit, RestoreError},
            score_password, DescriptorBlob, PasswordStrength, RecoveredWalletBlob, SeedBlob,
        },
    },
    signer::Signer,
};

/// Scope controls which halves the step tries to apply to `Context`
/// and determines the user-visible copy. A mode mismatch with the
/// server-side kit surfaces as `RestoreError::HalfMissing` at decrypt
/// time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreScope {
    /// W13 — full installer restore: populate both `recovered_signer`
    /// and `descriptor` from the kit.
    Full,
    /// W14 / W15 — descriptor-only: populate `descriptor` only, ignore
    /// any seed half on the server (the user already has a seed).
    DescriptorOnly,
}

#[derive(Debug)]
pub enum Phase {
    /// Collect email + send OTP (reuses the shape of CoincubeConnectStep).
    Email,
    /// OTP sent; user types the 6-digit code.
    OtpEntry,
    /// Fetching the cube list to let the user pick.
    LoadingCubes,
    /// Show cubes; selection is confirmed before opening its password form.
    CubePicker {
        cubes: Vec<RestoreCubeCandidate>,
        selected: Option<u64>,
    },
    /// Selected; prompting for the recovery password.
    PasswordEntry {
        selected: RestoreCubeCandidate,
        /// Attempts since last server fetch. Drives the client-side
        /// backoff hint (plan §2.8).
        attempts: u8,
    },
    /// Fetch + decrypt in flight.
    Decrypting { selected: RestoreCubeCandidate },
    /// Decrypted cleanly; waiting for the user to click Next to
    /// advance the installer flow. `apply` pulls these out into
    /// `Context`.
    Ready {
        selected: RestoreCubeCandidate,
        seed: Option<SeedBlob>,
        descriptor: Option<RecoveredWalletBlob>,
    },
    /// Terminal error state. User can either retry (back to Email) or
    /// Skip to proceed without restoration.
    Error { message: String },
}

/// What we remember about each cube the user can pick from. Carries
/// `status` so we can disable entries that don't have the half we're
/// trying to restore.
#[derive(Debug, Clone)]
pub struct RestoreCubeCandidate {
    pub id: u64,
    pub uuid: String,
    pub name: String,
    pub network: String,
    pub status: RecoveryKitStatus,
}

/// The step itself — self-contained with its own internal state.
static NEXT_RESTORE_UI_INSTANCE: AtomicU64 = AtomicU64::new(1);

pub struct RecoveryKitRestoreStep {
    ui_instance: u64,
    scope: RestoreScope,
    /// Network filter applied to the cube picker. The user can only
    /// restore into the same network they're installing for.
    network_filter: String,
    /// When set (W13 launched from a home "Your Cubes" row), the cube
    /// list is filtered to just this uuid so `phase_after_cubes_loaded`
    /// shows the matching Cube already selected for confirmation.
    preselect_uuid: Option<String>,
    client: CoincubeClient,
    phase: Phase,
    /// Where the last error was raised, for Import or restore's stepper.
    error_stage: crate::installer::RestoreStage,
    // Transient UI inputs held outside Phase so typing during an
    // async transition (unlikely but possible) doesn't crash.
    email: form::Value<String>,
    otp: form::Value<String>,
    password: Zeroizing<String>,
    /// `true` when the user hit Skip; `apply` returns `true` without
    /// touching `Context`. Cleared on re-entry (e.g. going Back).
    skipped: bool,
    processing: bool,
    error: Option<String>,
    /// JWT bearer token captured on successful OTP verify. Stays
    /// valid for the session, so wrapping it in `Zeroizing` is the
    /// difference between key material lingering on the heap after
    /// this step drops versus being scrubbed on drop.
    jwt: Option<Zeroizing<String>>,
}

impl RecoveryKitRestoreStep {
    pub fn new(
        scope: RestoreScope,
        network_filter: String,
        preselect_uuid: Option<String>,
    ) -> Self {
        Self {
            ui_instance: NEXT_RESTORE_UI_INSTANCE.fetch_add(1, Ordering::Relaxed),
            scope,
            network_filter,
            preselect_uuid,
            client: CoincubeClient::new(),
            phase: Phase::Email,
            error_stage: crate::installer::RestoreStage::SignIn,
            email: form::Value {
                valid: false,
                ..form::Value::default()
            },
            otp: form::Value::default(),
            password: Zeroizing::new(String::new()),
            skipped: false,
            processing: false,
            error: None,
            jwt: None,
        }
    }

    /// Observable scope — the parent installer uses this to decide
    /// whether to let the user skip (W14/W15 allow skip; W13 does not,
    /// since the whole point of W13 is the restore).
    pub fn scope(&self) -> RestoreScope {
        self.scope
    }

    /// The current phase's stage; an error stays on the stage that raised it.
    fn stage(&self) -> crate::installer::RestoreStage {
        use crate::installer::RestoreStage;
        match &self.phase {
            Phase::Email | Phase::OtpEntry => RestoreStage::SignIn,
            Phase::LoadingCubes | Phase::CubePicker { .. } => RestoreStage::Cube,
            Phase::PasswordEntry { .. } | Phase::Decrypting { .. } | Phase::Ready { .. } => {
                RestoreStage::Password
            }
            Phase::Error { .. } => self.error_stage,
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        if matches!(phase, Phase::Error { .. }) {
            self.error_stage = self.stage();
        }
        self.phase = phase;
        self.error = None;
    }

    fn send_otp_task(&self) -> Task<Message> {
        let client = self.client.clone();
        let email = self.email.value.clone();
        self.processing_task(Task::perform(
            async move {
                client
                    .login_send_otp(OtpRequest { email })
                    .await
                    .map_err(|e| e.to_string())
            },
            |res| Message::RecoveryKitRestore(RecoveryKitRestoreMsg::OtpSent(res)),
        ))
    }

    fn processing_task(&self, task: Task<Message>) -> Task<Message> {
        task
    }

    fn verify_otp_task(&self) -> Task<Message> {
        let client = self.client.clone();
        let email = self.email.value.clone();
        let otp = self.otp.value.clone();
        Task::perform(
            async move {
                client
                    .login_verify_otp(OtpVerifyRequest { email, otp })
                    .await
                    .map(|r| Zeroizing::new(r.token))
                    .map_err(|e| e.to_string())
            },
            |res| Message::RecoveryKitRestore(RecoveryKitRestoreMsg::OtpVerified(res)),
        )
    }

    fn list_cubes_task(&self) -> Task<Message> {
        let client = self.client.clone();
        let network = self.network_filter.clone();
        let preselect_uuid = self.preselect_uuid.clone();
        let entered_email = self.email.value.clone();
        Task::perform(
            async move {
                // A Home-launched restore inherits a verified Connect token
                // and skips this step's email form. Resolve that token's
                // account before showing a picker that asks the user to
                // confirm the account and network.
                let account_email = account_email_for_picker(&client, entered_email).await?;
                let all = client.list_cubes().await.map_err(|e| e.to_string())?;
                // Filter to cubes on the requested network. We don't
                // filter by `has_recovery_kit` at list time because the
                // kit status lives on a separate endpoint — fetch
                // status in parallel.
                //
                // When a specific cube was preselected (home restore
                // row), narrow to just that uuid so the loaded list has
                // a single entry and `phase_after_cubes_loaded`
                // shows that Cube selected in the picker.
                let matches: Vec<CubeResponse> = all
                    .into_iter()
                    .filter(|c| {
                        cube_passes_filter(&c.network, &c.uuid, &network, preselect_uuid.as_deref())
                    })
                    .collect();
                let mut out = Vec::with_capacity(matches.len());
                for cube in matches {
                    // Only NotFound is a signal that this particular
                    // cube has no kit yet — that's a legitimate state
                    // and we render it as a disabled row in the picker.
                    // Auth / rate-limit / 5xx / network errors are
                    // *probe failures*, not "no kit"; silently
                    // mapping them to `has_recovery_kit: false` would
                    // mislead the user into thinking they need to
                    // back up on a different device when the real
                    // issue is transient. Surface those instead so
                    // the picker's error path (`Phase::Error`) can
                    // show the cause and let the user retry.
                    let status = match client.get_recovery_kit_status(cube.id).await {
                        Ok(s) => s,
                        Err(CoincubeError::NotFound) => RecoveryKitStatus {
                            has_recovery_kit: false,
                            has_encrypted_seed: false,
                            has_encrypted_wallet_descriptor: false,
                            encryption_scheme: String::new(),
                            created_at: None,
                            updated_at: None,
                            owner_self: None,
                        },
                        Err(e) => {
                            return Err(format!(
                                "Couldn't load Recovery Kit status for \"{}\": {}",
                                cube.name, e
                            ));
                        }
                    };
                    out.push(RestoreCubeCandidate {
                        id: cube.id,
                        uuid: cube.uuid,
                        name: cube.name,
                        network: cube.network,
                        status,
                    });
                }
                Ok((out, account_email))
            },
            |res: Result<(Vec<RestoreCubeCandidate>, String), String>| {
                Message::RecoveryKitRestore(RecoveryKitRestoreMsg::CubesLoaded(res))
            },
        )
    }

    fn decrypt_task(&self, cube_id: u64) -> Task<Message> {
        let client = self.client.clone();
        let password = Zeroizing::new(self.password.to_string());
        Task::perform(
            async move {
                // Pass the typed `RestoreError` through unchanged so
                // the handler (see `DecryptResult` arm in `update`)
                // can branch on `BadPasswordOrCorrupt` vs. terminal
                // variants like `RateLimited` / `Api` / `NotFound` /
                // `BlobParse`. Stringifying here would collapse them
                // into an untyped bag and force the wrong UX.
                fetch_and_decrypt_wallet_kit(&client, cube_id, &password)
                    .await
                    .map(|RecoveredKit { seed, wallet }| (seed, wallet))
            },
            |res| Message::RecoveryKitRestore(RecoveryKitRestoreMsg::DecryptResult(res)),
        )
    }

    fn edit_otp(&mut self, value: Zeroizing<String>) -> Task<Message> {
        // Trim-and-store at the form level. The message-level
        // Zeroizing wrapper protected the in-flight copies;
        // the step state holds only the trimmed value briefly
        // until auto-submit or clear-on-verify.
        self.otp.value = value.trim().to_string();
        self.otp.valid = self.otp.value.len() == 6;
        // Auto-submit when 6 digits — matches
        // CoincubeConnectStep UX.
        if self.otp.valid && !self.processing {
            self.processing = true;
            self.verify_otp_task()
        } else {
            Task::none()
        }
    }

    fn select_cube(&mut self, id: u64) -> Task<Message> {
        let scope = self.scope;
        if let Phase::CubePicker { cubes, selected } = &mut self.phase {
            // Re-check availability in state even when a stale view
            // sends a selection for a disabled row.
            if cubes
                .iter()
                .any(|c| c.id == id && has_required_half(scope, c))
            {
                *selected = Some(id);
            }
        }
        Task::none()
    }

    fn confirm_cube(&mut self) -> Task<Message> {
        if let Phase::CubePicker { cubes, selected } = &self.phase {
            if let Some(c) = selected.and_then(|id| {
                cubes
                    .iter()
                    .find(|c| c.id == id && has_required_half(self.scope, c))
                    .cloned()
            }) {
                self.set_phase(Phase::PasswordEntry {
                    selected: c,
                    attempts: 0,
                });
            }
        }
        Task::none()
    }

    fn submit_password(&mut self) -> Task<Message> {
        if self.processing || self.password.is_empty() {
            return Task::none();
        }
        // Do NOT enforce `MIN_PASSWORD_LEN` here. That floor
        // belongs on the backup side — it's about picking a
        // strong password. On restore the user enters what
        // they chose at backup time, which may pre-date this
        // client's minimum (or come from another client with
        // different rules). A hardcoded gate here silently
        // blocks a correct password from ever reaching the
        // AES-GCM tag check, which is the only authoritative
        // validator of "is this the right password".
        // A nonempty input is still required before decrypting.
        let Phase::PasswordEntry { selected, attempts } = &self.phase else {
            return Task::none();
        };
        let cube_id = selected.id;
        let selected = selected.clone();
        let _ = attempts; // reserved for future backoff UI
        self.processing = true;
        self.set_phase(Phase::Decrypting { selected });
        self.decrypt_task(cube_id)
    }

    fn handle_decrypt_result(
        &mut self,
        res: Result<(Option<SeedBlob>, Option<RecoveredWalletBlob>), RestoreError>,
    ) -> Task<Message> {
        self.processing = false;
        let Phase::Decrypting { selected } = &self.phase else {
            // Message arrived after the user navigated away —
            // drop it.
            return Task::none();
        };
        let selected = selected.clone();
        match res {
            Ok((seed, descriptor)) => {
                // Validate we actually got the half we need.
                let missing_half = match self.scope {
                    RestoreScope::Full => seed.is_none(),
                    RestoreScope::DescriptorOnly => descriptor.is_none(),
                };
                if missing_half {
                    self.set_phase(Phase::Error {
                        message: "This Cube's Recovery Kit doesn't include the data needed \
                             for this restore."
                            .to_string(),
                    });
                    return Task::none();
                }
                // Cross-check the decrypted blob(s) against the
                // cube the user picked: the blob carries its
                // own `cube.uuid` + `cube.network` in plaintext,
                // and either mismatching those would mean we're
                // about to restore data belonging to a different
                // Cube/network. AES-GCM authentication already
                // ruled out tampering of *our* blob, but a
                // backend routing bug (wrong kit served for a
                // cube id) could still ship us the wrong payload;
                // the defensive identity check costs ~nothing.
                if let Err(msg) = validate_wallet_blobs_match_selected(
                    seed.as_ref(),
                    descriptor.as_ref(),
                    &selected,
                ) {
                    self.set_phase(Phase::Error { message: msg });
                    return Task::none();
                }
                self.set_phase(Phase::Ready {
                    selected,
                    seed,
                    descriptor,
                });
                // Let the user review the matched Cube and network
                // before the installer's Next action applies the kit.
                Task::none()
            }
            Err(e) => {
                if is_retryable_on_password_screen(&e) {
                    self.set_phase(Phase::PasswordEntry {
                        selected,
                        attempts: 1, // TODO wire backoff counter
                    });
                    self.error = Some(e.to_string());
                } else {
                    // Approach C (Phase 7): a 423 from the download is a
                    // terminal, plausibly-deniable cue — not a password
                    // retry. DURESS_LOCKED acknowledges the activation
                    // landed; TRUSTED_DEVICE_DELAY shows the wait time.
                    let message = match &e {
                        RestoreError::TrustedDeviceDelay {
                            available_at: Some(at),
                        } => coincube_ui::localized_format!(
                            "Recovery kit download is delayed on new devices. \
                             Available at {}.",
                            "La descarga del Kit de recuperación se retrasa en \
                             dispositivos nuevos. Disponible el {}.",
                            {
                                let local = at.with_timezone(&chrono::Local);
                                local
                                    .format(&coincube_ui::locale::date_pattern(
                                        "%b %d, %Y %H:%M %Z",
                                        &local,
                                    ))
                                    .to_string()
                            }
                        ),
                        // DURESS_LOCKED and a timeless delay use the
                        // typed Display message.
                        _ => e.to_string(),
                    };
                    self.set_phase(Phase::Error { message });
                }
                Task::none()
            }
        }
    }
}

/// Classify a decrypt error for UI routing. Returns `true` iff the
/// error is one the user can fix by retyping the password — i.e.,
/// `BadPasswordOrCorrupt`, the only variant where keeping the user
/// on the `PasswordEntry` screen with an inline banner is the right
/// UX. Every other variant (`RateLimited`, `Api`, `NotFound`,
/// `BlobParse`, `HalfMissing`) is terminal for this attempt and
/// routes to `Phase::Error`, because retyping the password won't
/// help.
fn is_retryable_on_password_screen(e: &RestoreError) -> bool {
    matches!(e, RestoreError::BadPasswordOrCorrupt)
}

/// After decrypting a kit, verify that any blobs we got back are
/// actually for the cube the user picked. The blob plaintexts carry
/// their own `cube.uuid` + `cube.network` identifiers — if either
/// disagrees with `selected`, something upstream is misrouted
/// (wrong kit served for a cube id, cross-device mix-up) and we
/// should refuse to apply the payload.
///
/// Returns `Ok(())` when every present blob matches, `Err(message)`
/// with a user-visible string otherwise.
fn validate_blobs_match_selected(
    seed: Option<&SeedBlob>,
    descriptor: Option<&DescriptorBlob>,
    selected: &RestoreCubeCandidate,
) -> Result<(), String> {
    let mismatch_msg = || {
        "This Recovery Kit contains data for a different Cube or network. \
         Sign out and retry — or contact support if the problem persists."
            .to_string()
    };
    if let Some(s) = seed {
        if s.cube.uuid != selected.uuid || s.cube.network != selected.network {
            return Err(mismatch_msg());
        }
    }
    if let Some(d) = descriptor {
        if d.cube.uuid != selected.uuid || d.cube.network != selected.network {
            return Err(mismatch_msg());
        }
    }
    Ok(())
}

fn validate_wallet_blobs_match_selected(
    seed: Option<&SeedBlob>,
    wallet: Option<&RecoveredWalletBlob>,
    selected: &RestoreCubeCandidate,
) -> Result<(), String> {
    match wallet {
        Some(RecoveredWalletBlob::Vault(descriptor)) => {
            validate_blobs_match_selected(seed, Some(descriptor), selected)
        }
        Some(RecoveredWalletBlob::Imported(imported)) => {
            validate_blobs_match_selected(seed, None, selected)?;
            if imported.cube.uuid != selected.uuid || imported.cube.network != selected.network {
                return Err("This Recovery Kit contains an imported wallet for a different Cube or network.".into());
            }
            Ok(())
        }
        None => validate_blobs_match_selected(seed, None, selected),
    }
}

/// The installer-context material derived from a decrypted kit: the parsed
/// descriptor (any scope) and, for `Full`, the seed signer. Shared by the owner
/// Recovery-Kit restore and the heir inheritance restore (`InheritanceRestoreStep`)
/// so the descriptor-parse + seed-derive path is audited in one place.
pub(crate) struct StagedRestore {
    pub descriptor: Option<CoincubeDescriptor>,
    pub signer: Option<Signer>,
    /// The restored Vault's creation time, when the kit recorded one. Kits
    /// written before `DescriptorBlobVault::birthday` existed have none, and
    /// the restore then asks the user how far back to scan instead of guessing.
    pub birthday: Option<u32>,
}

impl StagedRestore {
    /// Commit a staged restore into the installer context.
    ///
    /// Shared by all three restore steps — Recovery Kit, owner-keychain and
    /// inheritance — because all three install a descriptor that **already has
    /// on-chain history**, and every one of them therefore owes the new
    /// watchonly wallet a rescan. Two of them used to commit the descriptor and
    /// the signer by hand and record nothing about the rescan, so the Vault came
    /// up unable to see its own past. Putting the whole commit here is what
    /// stops a fourth path repeating that.
    ///
    /// Deliberately not the *whole* of any step's `apply`: the Recovery Kit and
    /// inheritance steps also carry a Cube identity out of their payload, which
    /// is theirs alone. This covers only what all three share.
    pub(crate) fn commit(self, ctx: &mut Context, source: RestoreSource) {
        // The rescan fields are **assigned**, not merely set when present, so a
        // commit that stages no descriptor clears them instead of leaving
        // whatever an earlier pass wrote. Half-assigning was the inconsistency:
        // `restored_wallet_birthday` was already unconditional while
        // `restore_source` was not, so a re-apply staging nothing could end up
        // with a source and no date — reading as `DateUnknown` for a Vault that
        // had a date, or as a rescan owed by a Vault that owes none.
        //
        // Tracked off the descriptor because the descriptor is precisely what
        // carries the history: the seed half does not, and a descriptor-only
        // restore has no seed at all. Each step names itself, so a reader of
        // `restore_source` never has to guess which restore produced it.
        let restored_descriptor = self.descriptor.is_some();
        // Provenance follows *either* half. A seed-only kit — a Cube that had no
        // Vault when it was backed up — restores a signer and no descriptor, and
        // it came out of a Recovery Kit just as much as a Vault restore did.
        // Keying provenance on the descriptor alone left those Cubes reporting
        // "no recovery kit" on the Home card about the very kit they were
        // rebuilt from.
        let restored_anything = restored_descriptor || self.signer.is_some();

        if let Some(d) = self.descriptor {
            ctx.descriptor = Some(d);
        }
        ctx.restore_source = restored_anything.then_some(source);
        // The rescan, by contrast, follows the descriptor and only the
        // descriptor: that is what carries on-chain history. A seed-only restore
        // has no Vault and no wallet to scan, so it must not acquire a birthday
        // — nor, via `pending_rescan`, an undated rescan for a Vault that does
        // not exist.
        ctx.restored_wallet_birthday = if restored_descriptor {
            self.birthday
        } else {
            None
        };

        if let Some(s) = self.signer {
            ctx.recovered_signer = Some(Arc::new(s));
        }
    }

    /// Undo what [`Self::commit`] writes that follows the descriptor.
    ///
    /// Every restore step's `revert` already drops `ctx.descriptor`, because a
    /// stale one leaks into a later decision. The rescan metadata is set from
    /// the same descriptor and has to go with it — otherwise a user who backs
    /// out of a restore and completes a *fresh* install carries a rescan that
    /// Vault does not owe, banner and all, and an inherited birthday could
    /// start a multi-hour scan of a chain the new wallet has no history on.
    pub(crate) fn revert_commit(ctx: &mut Context) {
        ctx.descriptor = None;
        ctx.restored_imported_wallet = None;
        ctx.restore_source = None;
        ctx.restored_wallet_birthday = None;
    }
}

/// Convert decrypted kit halves into [`StagedRestore`], applying the scope's
/// all-or-nothing discipline (Full needs the seed; DescriptorOnly ignores any
/// seed half). Returns a user-visible error string on descriptor-parse or
/// signer-derive failure; the caller surfaces it and leaves its context
/// untouched. Pure (no context mutation) so it's unit-testable in isolation and
/// reusable across both restore sources.
pub(crate) fn stage_restore(
    network: Network,
    scope: RestoreScope,
    seed: Option<&SeedBlob>,
    descriptor: Option<&DescriptorBlob>,
) -> Result<StagedRestore, String> {
    // Keep the blob before `descriptor` is shadowed by the parsed value; the
    // birthday lives on the blob, not on the parsed descriptor.
    let descriptor_blob = descriptor;

    // Parse the descriptor into an owned value first; an error here aborts
    // before we touch any seed, so a partial result is never committed.
    let descriptor = match descriptor {
        Some(desc_blob) => Some(
            desc_blob
                .vault
                .descriptor
                .parse::<CoincubeDescriptor>()
                .map_err(|e| {
                    coincube_ui::localized_format!(
                        "Recovery Kit descriptor failed to parse: {}. Kit may be from a newer \
                         client version.",
                        "No se pudo leer el descriptor del Kit de recuperación: {}. Es posible \
                         que el kit venga de una versión más reciente de la aplicación.",
                        e
                    )
                })?,
        ),
        None => None,
    };

    // Derive the seed signer (Full only). DescriptorOnly ignores any seed half.
    let signer = if matches!(scope, RestoreScope::Full) {
        let seed_blob = seed.ok_or_else(|| "Seed blob missing from decrypted kit.".to_string())?;
        let master = MasterSigner::from_str(network, &seed_blob.mnemonic.phrase).map_err(|e| {
            coincube_ui::localized_format!(
                "Restored mnemonic failed to derive a signer: {}",
                "No se pudo obtener un firmante de la frase restaurada: {}",
                e
            )
        })?;
        Some(Signer::new(master))
    } else {
        None
    };

    let birthday = descriptor_blob.and_then(|b| b.vault.birthday);

    Ok(StagedRestore {
        descriptor,
        signer,
        birthday,
    })
}

/// Filter applied to the server cube list before status probing:
/// keep cubes on the network we're installing for, and — when a
/// specific cube was preselected from a home "Your Cubes" row — only
/// the row with that uuid. Both conditions must hold; a preselected
/// uuid that lives on a different network is still excluded, so the
/// caller never probes or displays a cross-network cube.
/// Extracted as a pure fn so the network×preselect matrix is
/// unit-testable without standing up the async task.
fn cube_passes_filter(
    cube_network: &str,
    cube_uuid: &str,
    network: &str,
    preselect_uuid: Option<&str>,
) -> bool {
    cube_network == network && preselect_uuid.is_none_or(|uuid| cube_uuid == uuid)
}

/// Use the verified in-step email, or resolve the account represented by an
/// inherited Home token before showing the picker.
async fn account_email_for_picker(
    client: &CoincubeClient,
    entered_email: String,
) -> Result<String, String> {
    if !entered_email.is_empty() {
        return Ok(entered_email);
    }
    let user = client
        .get_user()
        .await
        .map_err(|e| format!("Couldn't identify Connect account: {e}"))?;
    let email = user.email.trim();
    if !email_address::EmailAddress::is_valid(email) {
        return Err("Connect account has no valid email to confirm.".into());
    }
    Ok(email.to_string())
}

/// Does `c` carry the specific encrypted half this restore flow
/// needs? `has_recovery_kit` alone isn't enough — a cube can have
/// a seed-only kit (mnemonic cube without a Vault when the backup
/// was made) or a descriptor-only kit (passkey cube, or the
/// descriptor half uploaded first). Routing a Full restore to a
/// descriptor-only kit, or a DescriptorOnly restore to a seed-only
/// kit, would decrypt successfully and then hit the post-decrypt
/// `missing_half` guard — making the user enter their password
/// only to be told the kit can't satisfy this flow. Gating at
/// selection time catches it up front.
fn has_required_half(scope: RestoreScope, c: &RestoreCubeCandidate) -> bool {
    c.status.has_recovery_kit
        && match scope {
            RestoreScope::Full => c.status.has_encrypted_seed,
            RestoreScope::DescriptorOnly => c.status.has_encrypted_wallet_descriptor,
        }
}

/// Transition function called after `list_cubes` + per-cube status
/// probes resolve. Extracted as a pure function over the Phase enum
/// so the picker vs error decision is unit-testable
/// without standing up the whole installer step.
///
/// Branches:
/// - Exactly one cube with the required half → show it selected in the picker
///   so the account, network and kit availability are visible before Next
/// - Exactly one cube **without** the required half → `Phase::Error`
///   with a specific message naming the missing half; the picker's
///   disabled-row affordance doesn't help when there's only one row
/// - Zero or 2+ cubes → `Phase::CubePicker` (the picker view already
///   handles the empty-list and mixed-kit cases cleanly, and the
///   picker's per-row availability label narrates scope mismatches)
fn phase_after_cubes_loaded(scope: RestoreScope, cubes: Vec<RestoreCubeCandidate>) -> Phase {
    if cubes.len() == 1 {
        // Can't `into_iter().next().unwrap()` inside the `if` guard
        // without moving; handle with `match` on length-1 form.
        let c = cubes.into_iter().next().expect("len == 1");
        if has_required_half(scope, &c) {
            Phase::CubePicker {
                selected: Some(c.id),
                cubes: vec![c],
            }
        } else {
            let message = match scope {
                RestoreScope::Full => coincube_ui::localized_format!(
                    "\"{}\" doesn't have a Master Seed Phrase backed up on Connect — the kit \
                     was backed up without the seed half (passkey Cubes can't include it), \
                     and Full restore needs it. Finish the backup from a device where the \
                     Cube is already installed, then return here to restore.",
                    "\"{}\" no tiene una frase de recuperación maestra guardada en Connect: \
                     el kit se guardó sin la frase (los Cubes con llave de acceso no pueden \
                     incluirla) y la restauración completa la necesita. Termina la copia \
                     desde un dispositivo donde el Cube ya esté instalado y vuelve aquí \
                     para restaurarlo.",
                    c.name,
                ),
                RestoreScope::DescriptorOnly => coincube_ui::localized_format!(
                    "\"{}\" doesn't have a Wallet Descriptor backed up on Connect — the kit \
                     was backed up seed-only (the Cube had no Vault at backup time). Finish \
                     the backup from a device where the Cube is already installed, then \
                     return here to restore.",
                    "\"{}\" no tiene un descriptor de cartera guardado en Connect: el kit \
                     se guardó solo con la frase (el Cube no tenía Vault cuando se hizo la \
                     copia). Termina la copia desde un dispositivo donde el Cube ya esté \
                     instalado y vuelve aquí para restaurarlo.",
                    c.name,
                ),
            };
            Phase::Error { message }
        }
    } else {
        Phase::CubePicker {
            cubes,
            selected: None,
        }
    }
}

impl From<RecoveryKitRestoreStep> for Box<dyn Step> {
    fn from(s: RecoveryKitRestoreStep) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl Step for RecoveryKitRestoreStep {
    fn restore_stage(&self) -> Option<crate::installer::RestoreStage> {
        Some(self.stage())
    }

    /// Pick up an already-authenticated Connect session from
    /// `Context` the first time this step becomes active.
    ///
    /// Today the Recovery-Kit restore flow is typically launched from
    /// a home row whose "remote cubes" list only exists because
    /// the user already signed into Connect. That session lives on
    /// the home's `ConnectAccountPanel` and gets forwarded into
    /// the installer as `ctx.coincube_client`. Without this hook the
    /// step would start at `Phase::Email` and ask the user to retype
    /// their email + OTP for the same account — the "two sign-ins"
    /// bug reported in the app.
    ///
    /// Guards:
    ///   * Only triggers on the *initial* `Phase::Email` and when we
    ///     haven't captured our own JWT yet (i.e. the user hasn't
    ///     already completed the in-step auth form). That means a
    ///     late `load_context` — e.g. after the user hit
    ///     `RetryFromStart`, which re-enters `Phase::Email` on
    ///     purpose — won't silently teleport them back into
    ///     `LoadingCubes` with whatever stale client `ctx` still
    ///     carries.
    ///   * Requires `client.token().is_some()` because an
    ///     unauthenticated `CoincubeClient` is useless for
    ///     `list_cubes` and would just surface as a 401 further down.
    fn load_context(&mut self, ctx: &Context) {
        if !matches!(self.phase, Phase::Email) || self.jwt.is_some() {
            return;
        }
        let Some(client) = &ctx.coincube_client else {
            return;
        };
        let Some(token) = client.token() else {
            return;
        };
        // Clone the client (not just the token) so we inherit whatever
        // base URL / HTTP plumbing the home has already configured.
        // Stash the JWT in `Zeroizing` so it gets scrubbed from the
        // heap when the step drops — matches the handling of tokens
        // captured via the in-step OTP path.
        self.client = client.clone();
        self.jwt = Some(Zeroizing::new(token.to_string()));
        self.set_phase(Phase::LoadingCubes);
    }

    /// Kick off the cube-list fetch as soon as we've been promoted
    /// into `Phase::LoadingCubes` by `load_context`. Without this the
    /// UI would sit on a loading spinner indefinitely — the step
    /// machine otherwise relies on user-driven messages to fire tasks
    /// (OTP submit etc.), which this pre-authed path skips entirely.
    fn load(&self) -> Task<Message> {
        if matches!(self.phase, Phase::LoadingCubes) {
            self.list_cubes_task()
        } else {
            Task::none()
        }
    }

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        let Message::RecoveryKitRestore(msg) = message else {
            return Task::none();
        };
        match msg {
            RecoveryKitRestoreMsg::EmailEdited(value) => {
                self.email.value = value;
                self.email.valid = !self.email.value.is_empty() && self.email.value.contains('@');
                Task::none()
            }
            RecoveryKitRestoreMsg::RequestOtp => {
                if !self.email.valid || self.processing {
                    return Task::none();
                }
                self.processing = true;
                self.send_otp_task()
            }
            RecoveryKitRestoreMsg::OtpSent(res) => {
                self.processing = false;
                match res {
                    Ok(()) => {
                        self.otp = form::Value::default();
                        self.set_phase(Phase::OtpEntry);
                    }
                    Err(e) => self.error = Some(e),
                }
                Task::none()
            }
            RecoveryKitRestoreMsg::OtpEdited(value) => self.edit_otp(value),
            RecoveryKitRestoreMsg::OtpVerified(res) => {
                self.processing = false;
                match res {
                    Ok(token) => {
                        // `token` is `Zeroizing<String>` — deref to
                        // `&str` for `set_token`, then stash the
                        // Zeroizing wrapper so the JWT heap bytes are
                        // wiped when the step drops or re-auths.
                        self.client.set_token(token.as_str());
                        self.jwt = Some(token);
                        self.set_phase(Phase::LoadingCubes);
                        self.list_cubes_task()
                    }
                    Err(e) => {
                        self.otp.valid = false;
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }
            RecoveryKitRestoreMsg::CubesLoaded(res) => {
                match res {
                    Ok((cubes, account_email)) => {
                        self.email.value = account_email;
                        self.email.valid = true;
                        self.set_phase(phase_after_cubes_loaded(self.scope, cubes));
                    }
                    Err(e) => {
                        self.set_phase(Phase::Error {
                            message: coincube_ui::localized_format!(
                                "Couldn't load cubes: {}",
                                "No se pudieron cargar los Cubes: {}",
                                e
                            ),
                        });
                    }
                }
                Task::none()
            }
            RecoveryKitRestoreMsg::SelectCube(id) => self.select_cube(id),
            RecoveryKitRestoreMsg::ConfirmCube => self.confirm_cube(),
            RecoveryKitRestoreMsg::PasswordEdited(v) => {
                // Move the `Zeroizing<String>` wrapper straight into
                // state; the old `self.password` is dropped here
                // (its heap zeroes via Zeroizing's Drop).
                self.password = v;
                Task::none()
            }
            RecoveryKitRestoreMsg::SubmitPassword => self.submit_password(),
            RecoveryKitRestoreMsg::DecryptResult(res) => self.handle_decrypt_result(res),
            RecoveryKitRestoreMsg::RetryFromStart => {
                // Full reset: drop JWT, clear transient inputs, back to
                // email entry.
                self.client = CoincubeClient::new();
                self.jwt = None;
                self.email.value.clear();
                self.email.valid = false;
                self.otp = form::Value::default();
                self.password = Zeroizing::new(String::new());
                self.skipped = false;
                self.set_phase(Phase::Email);
                Task::none()
            }
            RecoveryKitRestoreMsg::Skip => {
                // Only makes sense for DescriptorOnly (W14/W15); the
                // full-restore W13 step should hide this in the view.
                if matches!(self.scope, RestoreScope::DescriptorOnly) {
                    self.skipped = true;
                    Task::done(Message::Next)
                } else {
                    Task::none()
                }
            }
        }
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        // Skip path: the user opted out (W14/W15 only). Leave `ctx`
        // alone and let the installer proceed.
        if self.skipped {
            self.skipped = false;
            return true;
        }
        // Must have a `Ready` phase to apply. Otherwise the user is
        // trying to advance before the decrypt resolved — keep them
        // where they are.
        let Phase::Ready {
            seed, descriptor, ..
        } = &self.phase
        else {
            return false;
        };

        // Stage 1+2 — parse the descriptor and derive the seed signer (Full
        // only) into locals via the shared [`stage_restore`] seam. All-or-
        // nothing: any failure transitions to `Phase::Error` and bails without
        // touching `ctx`, so a partial result never survives into a later step.
        let (vault_blob, imported_blob) = match descriptor.as_ref() {
            Some(RecoveredWalletBlob::Vault(vault)) => (Some(vault), None),
            Some(RecoveredWalletBlob::Imported(imported)) => (None, Some(imported.clone())),
            None => (None, None),
        };
        if let Some(imported) = &imported_blob {
            if let Err(message) = imported.validate(ctx.bitcoin_config.chain) {
                self.set_phase(Phase::Error { message });
                return false;
            }
            // Imported keys remain external. A descriptor-only restore must
            // attach to the same already-open Cube named by the Kit.
            if imported.wallet.descriptor.chain() != ctx.bitcoin_config.chain
                || (matches!(self.scope, RestoreScope::DescriptorOnly)
                    && ctx.cube_id.as_deref() != Some(imported.cube.uuid.as_str()))
            {
                self.set_phase(Phase::Error {
                    message:
                        "This imported wallet Recovery Kit belongs to a different Cube or chain."
                            .into(),
                });
                return false;
            }
        }
        let staged = match stage_restore(
            ctx.bitcoin_config.network,
            self.scope,
            seed.as_ref(),
            vault_blob,
        ) {
            Ok(staged) => staged,
            Err(message) => {
                self.set_phase(Phase::Error { message });
                return false;
            }
        };

        // Stage 3 — commit both results atomically. We only reach here after
        // every fallible step above succeeded, so `ctx` transitions from
        // "nothing applied" to "fully applied" in one go. `Arc::new` happens
        // here rather than at staging so we don't allocate the refcounted handle
        // for a signer that ultimately gets dropped on the error path.
        staged.commit(ctx, RestoreSource::PasswordKit);
        ctx.restored_imported_wallet = imported_blob;
        if let Some(imported) = &ctx.restored_imported_wallet {
            ctx.descriptor = None;
            ctx.restore_source = Some(RestoreSource::PasswordKit);
            ctx.restored_wallet_birthday = Some(imported.wallet.birthday);
            ctx.wallet_alias = imported.wallet.name.clone();
        }

        // Carry the *original* Cube identity (UUID + name) out of the
        // decrypted kit so the post-install `find_or_create_cube` re-mints
        // the same Cube instead of a fresh one. Reusing the UUID makes the
        // Connect `register_cube` call (idempotent on UUID) reactivate the
        // deleted Cube rather than create a duplicate — without this, a
        // Recovery-Kit restore spawned a brand-new Cube and left the
        // original still listed as recoverable, so the flow could be
        // repeated indefinitely.
        //
        // Full scope only: `DescriptorOnly` (W14/W15) restores into a Cube
        // that already exists on disk with its own identity, so we must not
        // overwrite `cube_id`/`cube_name` there.
        if matches!(self.scope, RestoreScope::Full) {
            if let Some(seed) = seed {
                ctx.cube_id = Some(seed.cube.uuid.clone());
                ctx.cube_name = Some(seed.cube.name.clone());
            }
        }

        // Thread the JWT we captured during the OTP step into the
        // context so the downstream `CoincubeConnectStep` can skip
        // re-authentication. Without this, the user has to type their
        // email + OTP a second time — same account, same session —
        // which is both confusing and a soft footgun (users can
        // accidentally auth as a different account and register the
        // restored Cube under the wrong Connect user).
        //
        // JWT is only present when we went through the successful
        // `Phase::Ready` path above; the `skipped` branch short-circuits
        // before reaching here so we never push a stale/empty JWT.
        if let Some(jwt) = &self.jwt {
            // `self.jwt: Option<Zeroizing<String>>` — cloning preserves
            // the wrapper so `Context` carries the scrub-on-drop
            // guarantee all the way to the Esplora-config copy.
            ctx.connect_jwt = Some(jwt.clone());
            ctx.use_coincube_connect = true;
        }

        true
    }

    fn revert(&self, ctx: &mut Context) {
        // Back-out: drop anything this step wrote to context so the
        // user can re-do the restore cleanly.
        if matches!(self.scope, RestoreScope::Full) {
            ctx.recovered_signer = None;
            // Drop the original Cube identity we stamped in `apply` (Full
            // scope only, matching where we set it) so a subsequent forward
            // pass doesn't re-mint the restored Cube from stale values.
            ctx.cube_id = None;
            ctx.cube_name = None;
        }
        // Always clear the descriptor and the rescan metadata set from it —
        // if the user navigates back from a later step through this one,
        // keeping either would leak into a subsequent decision.
        StagedRestore::revert_commit(ctx);
        // Also drop the Connect auth bits we pushed in `apply`.
        // `CoincubeConnectStep` skips itself when `connect_jwt` is
        // `Some`, so failing to clear here would "teleport" the user
        // past the auth step on a subsequent forward pass with a stale
        // token. Clearing is harmless when this step never populated
        // them (idempotent).
        ctx.connect_jwt = None;
        ctx.use_coincube_connect = false;
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
    ) -> Element<'a, Message> {
        view::recovery_kit_restore(
            progress,
            self.ui_instance,
            self.scope,
            &self.network_filter,
            &self.phase,
            &self.email,
            &self.otp,
            self.password.as_str(),
            self.processing,
            self.error.as_deref(),
        )
    }
}

// Public re-exports for the view module — `Phase` and
// `RestoreCubeCandidate` are consumed there when rendering.
pub use self::Phase as RestorePhase;

/// Strength hint (for the password-entry sub-screen) — forwarded from
/// the recovery-kit password module so the view module doesn't have to
/// import two namespaces. Currently unused by the placeholder view but
/// kept here so a follow-up UI pass can surface the meter without
/// reaching across crates.
#[allow(dead_code)]
pub fn classify_password(password: &str) -> PasswordStrength {
    let z = Zeroizing::new(password.to_string());
    score_password(&z, &[]).0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server-side Bitcoin Blake2b Cube never passes a Bitcoin-family
    /// network filter, and vice versa: the Connect strings differ on purpose
    /// (`ChainId::api_str`), so a restore flow installing for mainnet can
    /// neither list nor select the BTCB2 twin — its identity is not
    /// collapsed to "mainnet" at this boundary. (BTCB2 plan PR 2.)
    #[test]
    fn a_blake2b_server_cube_never_passes_a_bitcoin_family_filter() {
        use crate::chain::ChainId;
        for (server, installing) in [
            (ChainId::BitcoinBlake2b, ChainId::Bitcoin),
            (ChainId::Bitcoin, ChainId::BitcoinBlake2b),
            (ChainId::BitcoinBlake2bTestnet4, ChainId::Testnet4),
            (ChainId::Testnet4, ChainId::BitcoinBlake2bTestnet4),
        ] {
            assert!(
                !cube_passes_filter(server.api_str(), "u", installing.api_str(), None),
                "{:?} passed a {:?} filter",
                server,
                installing
            );
            assert!(
                !cube_passes_filter(server.api_str(), "u", installing.api_str(), Some("u")),
                "{:?} passed a preselected {:?} filter",
                server,
                installing
            );
        }
        // Same chain still matches, so the filter is not simply broken.
        assert!(cube_passes_filter(
            "bitcoin-blake2b",
            "u",
            "bitcoin-blake2b",
            Some("u")
        ));
        assert!(cube_passes_filter("mainnet", "u", "mainnet", None));
    }

    fn candidate(name: &str, has_kit: bool) -> RestoreCubeCandidate {
        RestoreCubeCandidate {
            id: 1,
            uuid: "uuid".to_string(),
            name: name.to_string(),
            network: "mainnet".to_string(),
            status: RecoveryKitStatus {
                has_recovery_kit: has_kit,
                has_encrypted_seed: has_kit,
                has_encrypted_wallet_descriptor: has_kit,
                encryption_scheme: "aes-256-gcm".to_string(),
                created_at: None,
                updated_at: None,
                owner_self: None,
            },
        }
    }

    /// Fine-grained variant of `candidate` that lets tests control
    /// each half independently — needed to exercise the scope-aware
    /// gating where `has_recovery_kit` is true but only one half is
    /// present.
    fn candidate_halves(name: &str, has_seed: bool, has_descriptor: bool) -> RestoreCubeCandidate {
        RestoreCubeCandidate {
            id: 1,
            uuid: "uuid".to_string(),
            name: name.to_string(),
            network: "mainnet".to_string(),
            status: RecoveryKitStatus {
                has_recovery_kit: has_seed || has_descriptor,
                has_encrypted_seed: has_seed,
                has_encrypted_wallet_descriptor: has_descriptor,
                encryption_scheme: "aes-256-gcm".to_string(),
                created_at: None,
                updated_at: None,
                owner_self: None,
            },
        }
    }

    #[tokio::test]
    async fn inherited_connect_session_resolves_account_email() {
        use httpmock::{Method, MockServer};

        let server = MockServer::start();
        let user = server.mock(|when, then| {
            when.method(Method::GET).path("/api/v1/user");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":7,"email":"session@example.com","emailVerified":true}"#);
        });
        let mut client = CoincubeClient::for_test(server.base_url());
        client.set_token("synthetic-token");

        assert_eq!(
            account_email_for_picker(&client, String::new()).await,
            Ok("session@example.com".to_string())
        );
        user.assert_hits(1);
    }

    #[tokio::test]
    async fn entered_email_does_not_request_account_again() {
        use httpmock::{Method, MockServer};

        let server = MockServer::start();
        let user = server.mock(|when, then| {
            when.method(Method::GET).path("/api/v1/user");
            then.status(500);
        });
        let client = CoincubeClient::for_test(server.base_url());

        assert_eq!(
            account_email_for_picker(&client, "entered@example.com".into()).await,
            Ok("entered@example.com".to_string())
        );
        user.assert_hits(0);
    }

    #[tokio::test]
    async fn inherited_session_with_invalid_account_email_stops_before_picker() {
        use httpmock::{Method, MockServer};

        let server = MockServer::start();
        let user = server.mock(|when, then| {
            when.method(Method::GET).path("/api/v1/user");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":7,"email":" ","emailVerified":true}"#);
        });
        let client = CoincubeClient::for_test(server.base_url());

        assert!(account_email_for_picker(&client, String::new())
            .await
            .is_err());
        user.assert_hits(1);
    }

    #[test]
    fn loaded_cubes_bind_resolved_account_to_picker() {
        let mut step = RecoveryKitRestoreStep::new(RestoreScope::Full, "mainnet".into(), None);
        step.phase = Phase::LoadingCubes;
        let mut hws = HardwareWallets::new(
            crate::dir::CoincubeDirectory::new("/synthetic/recovery-account".into()),
            Network::Bitcoin,
        );

        let _ = step.update(
            &mut hws,
            Message::RecoveryKitRestore(RecoveryKitRestoreMsg::CubesLoaded(Ok((
                vec![candidate("My Cube", true)],
                "session@example.com".into(),
            )))),
        );
        assert_eq!(step.email.value, "session@example.com");
        assert!(matches!(
            step.phase,
            Phase::CubePicker {
                selected: Some(1),
                ..
            }
        ));
    }

    /// Import or restore's stepper keeps an error on the stage that raised
    /// it: a failed Cube list is not a Password error.
    #[test]
    fn an_error_keeps_the_stage_that_raised_it() {
        use crate::installer::RestoreStage;
        let mut step = RecoveryKitRestoreStep::new(RestoreScope::Full, "mainnet".into(), None);
        assert_eq!(step.restore_stage(), Some(RestoreStage::SignIn));
        step.phase = Phase::LoadingCubes;
        let mut hws = HardwareWallets::new(
            crate::dir::CoincubeDirectory::new("/synthetic/recovery-stage".into()),
            Network::Bitcoin,
        );
        let _ = step.update(
            &mut hws,
            Message::RecoveryKitRestore(RecoveryKitRestoreMsg::CubesLoaded(Err(
                "synthetic outage".into(),
            ))),
        );
        assert!(matches!(step.phase, Phase::Error { .. }));
        assert_eq!(step.restore_stage(), Some(RestoreStage::Cube));
        // A lone Cube whose kit lacks the seed also fails at Cube.
        step.phase = Phase::LoadingCubes;
        let _ = step.update(
            &mut hws,
            Message::RecoveryKitRestore(RecoveryKitRestoreMsg::CubesLoaded(Ok((
                vec![candidate("No seed", false)],
                "session@example.com".into(),
            )))),
        );
        assert!(matches!(step.phase, Phase::Error { .. }));
        assert_eq!(step.restore_stage(), Some(RestoreStage::Cube));
    }

    // --- `cube_passes_filter`: the network × preselect matrix that
    // `list_cubes_task` uses to narrow the server cube list. ---

    #[test]
    fn filter_keeps_matching_cube_on_network_when_preselected() {
        // Preselected uuid on the requested network → kept.
        assert!(cube_passes_filter(
            "mainnet",
            "cube-a",
            "mainnet",
            Some("cube-a")
        ));
    }

    #[test]
    fn filter_drops_mismatched_uuid_when_preselected() {
        // Right network, wrong uuid → excluded so only the clicked cube
        // survives to the picker.
        assert!(!cube_passes_filter(
            "mainnet",
            "cube-b",
            "mainnet",
            Some("cube-a")
        ));
    }

    #[test]
    fn filter_drops_preselected_uuid_on_wrong_network() {
        // Matching uuid but a different network → still excluded; we
        // never restore into a network we're not installing for.
        assert!(!cube_passes_filter(
            "testnet",
            "cube-a",
            "mainnet",
            Some("cube-a")
        ));
    }

    #[test]
    fn filter_without_preselect_keeps_all_on_network() {
        // No preselection (W14/W15, or a home launch without a uuid):
        // network is the only gate.
        assert!(cube_passes_filter("mainnet", "cube-a", "mainnet", None));
        assert!(cube_passes_filter("mainnet", "cube-b", "mainnet", None));
    }

    #[test]
    fn filter_without_preselect_drops_off_network() {
        assert!(!cube_passes_filter("testnet", "cube-a", "mainnet", None));
    }

    #[test]
    fn home_preselection_still_waits_for_picker_confirmation() {
        let mut other = candidate("Other", true);
        other.uuid = "other-uuid".into();
        let survivors: Vec<RestoreCubeCandidate> = vec![candidate("Preselected", true), other]
            .into_iter()
            .filter(|c| cube_passes_filter(&c.network, &c.uuid, "mainnet", Some("uuid")))
            .collect();
        assert_eq!(survivors.len(), 1);
        let phase = phase_after_cubes_loaded(RestoreScope::Full, survivors);
        assert!(
            matches!(phase, Phase::CubePicker { cubes, selected: Some(1) } if cubes.len() == 1 && cubes[0].name == "Preselected")
        );
    }

    // Regression tests for the single-cube-skips-kit-check bug. Before
    // this fix, a single cube without a kit would land the user on
    // the password screen, which would then 404 at decrypt time —
    // a worse UX than a direct "no kit yet" message up front.

    #[test]
    fn single_cube_with_kit_waits_for_picker_confirmation() {
        let phase = phase_after_cubes_loaded(RestoreScope::Full, vec![candidate("My Cube", true)]);
        match phase {
            Phase::CubePicker { cubes, selected } => {
                assert_eq!(cubes[0].name, "My Cube");
                assert_eq!(selected, Some(cubes[0].id));
            }
            other => panic!("expected CubePicker, got {:?}", other),
        }
    }

    #[test]
    fn single_cube_without_kit_surfaces_error_not_password_screen() {
        let phase = phase_after_cubes_loaded(RestoreScope::Full, vec![candidate("My Cube", false)]);
        match phase {
            Phase::Error { message } => {
                assert!(
                    message.contains("My Cube"),
                    "error message should name the cube, got: {}",
                    message
                );
                assert!(
                    message.contains("doesn't have"),
                    "error message should explain the cause, got: {}",
                    message
                );
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }

    // Regression tests for the scope-aware half-gating: a kit that
    // exists but lacks the required half must NOT advance to the
    // password screen. Without this gate, a Full-scope restore
    // against a descriptor-only kit (or DescriptorOnly against a
    // seed-only kit) would silently decrypt, land on `Ready`, and
    // only then hit the post-decrypt `missing_half` error — making
    // the user re-enter their password for nothing.

    #[test]
    fn full_scope_on_descriptor_only_kit_routes_to_error() {
        // Kit has descriptor but not seed; Full restore needs seed.
        let phase = phase_after_cubes_loaded(
            RestoreScope::Full,
            vec![candidate_halves("Passkey Cube", false, true)],
        );
        match phase {
            Phase::Error { message } => {
                assert!(message.contains("Passkey Cube"));
                assert!(
                    message.contains("Master Seed"),
                    "error should name the missing half (seed): {}",
                    message
                );
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[test]
    fn descriptor_only_scope_on_seed_only_kit_routes_to_error() {
        // Kit has seed but no descriptor; DescriptorOnly needs descriptor.
        let phase = phase_after_cubes_loaded(
            RestoreScope::DescriptorOnly,
            vec![candidate_halves("No-Vault Cube", true, false)],
        );
        match phase {
            Phase::Error { message } => {
                assert!(message.contains("No-Vault Cube"));
                assert!(
                    message.contains("Wallet Descriptor"),
                    "error should name the missing half (descriptor): {}",
                    message
                );
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[test]
    fn has_required_half_matches_scope() {
        let seed_only = candidate_halves("s", true, false);
        let desc_only = candidate_halves("d", false, true);
        let both = candidate_halves("b", true, true);
        let none = candidate_halves("n", false, false);

        assert!(has_required_half(RestoreScope::Full, &seed_only));
        assert!(!has_required_half(RestoreScope::Full, &desc_only));
        assert!(has_required_half(RestoreScope::Full, &both));
        assert!(!has_required_half(RestoreScope::Full, &none));

        assert!(!has_required_half(RestoreScope::DescriptorOnly, &seed_only));
        assert!(has_required_half(RestoreScope::DescriptorOnly, &desc_only));
        assert!(has_required_half(RestoreScope::DescriptorOnly, &both));
        assert!(!has_required_half(RestoreScope::DescriptorOnly, &none));
    }

    // Regression tests for the decrypt-error branching: the
    // handler must route `BadPasswordOrCorrupt` back to the
    // password screen (retryable) but every other variant to
    // `Phase::Error` (terminal for this attempt). Before this
    // fix the handler stringified all errors and treated them
    // uniformly as "keep the user on PasswordEntry", which made
    // rate limits / NotFound / BlobParse read as wrong-password
    // errors that retyping couldn't fix.

    #[test]
    fn bad_password_is_retryable_on_password_screen() {
        assert!(is_retryable_on_password_screen(
            &RestoreError::BadPasswordOrCorrupt
        ));
    }

    #[test]
    fn rate_limited_is_not_retryable_on_password_screen() {
        assert!(!is_retryable_on_password_screen(
            &RestoreError::RateLimited {
                retry_after: std::time::Duration::from_secs(30)
            }
        ));
    }

    #[test]
    fn not_found_is_not_retryable_on_password_screen() {
        assert!(!is_retryable_on_password_screen(&RestoreError::NotFound));
    }

    #[test]
    fn api_error_is_not_retryable_on_password_screen() {
        assert!(!is_retryable_on_password_screen(&RestoreError::Api(
            "5xx".to_string()
        )));
    }

    #[test]
    fn blob_parse_is_not_retryable_on_password_screen() {
        assert!(!is_retryable_on_password_screen(&RestoreError::BlobParse(
            "future version".to_string()
        )));
    }

    #[test]
    fn half_missing_is_not_retryable_on_password_screen() {
        // HalfMissing gets its own dedicated pre-check elsewhere
        // (the `missing_half` guard) so it shouldn't normally reach
        // this branch; pinning the non-retryable classification
        // here guards against a future refactor that collapses the
        // two sites.
        assert!(!is_retryable_on_password_screen(&RestoreError::HalfMissing));
    }

    // Regression tests for the cube-identity cross-check performed
    // after decrypt. The blob plaintext carries `cube.uuid` +
    // `cube.network`; if those disagree with the cube the user
    // picked, we refuse to restore — this catches backend
    // misrouting and any future class of "wrong kit served for
    // this cube id" bugs before on-disk state gets corrupted.

    fn seed_blob_for(uuid: &str, network: &str) -> crate::services::recovery::SeedBlob {
        use crate::services::recovery::{
            plaintext::BLOB_VERSION, SeedBlob, SeedBlobCube, SeedBlobMnemonic,
        };
        SeedBlob {
            version: BLOB_VERSION,
            cube: SeedBlobCube {
                uuid: uuid.to_string(),
                name: "name".to_string(),
                network: network.to_string(),
                created_at: "2026-04-23T00:00:00Z".to_string(),
                lightning_address: None,
            },
            mnemonic: SeedBlobMnemonic {
                phrase: "word ".repeat(12).trim().to_string(),
                language: "en".to_string(),
            },
        }
    }

    fn descriptor_blob_for(uuid: &str, network: &str) -> crate::services::recovery::DescriptorBlob {
        use crate::services::recovery::{
            plaintext::BLOB_VERSION, DescriptorBlob, DescriptorBlobCube, DescriptorBlobVault,
        };
        DescriptorBlob {
            version: BLOB_VERSION,
            cube: DescriptorBlobCube {
                uuid: uuid.to_string(),
                network: network.to_string(),
            },
            vault: DescriptorBlobVault {
                name: "v".into(),
                descriptor: "d".into(),
                change_descriptor: None,
                signers: vec![],
                birthday: None,
            },
        }
    }

    #[test]
    fn validate_passes_when_both_blobs_match_selected() {
        let sel = candidate("My Cube", true);
        let s = seed_blob_for(&sel.uuid, &sel.network);
        let d = descriptor_blob_for(&sel.uuid, &sel.network);
        assert!(validate_blobs_match_selected(Some(&s), Some(&d), &sel).is_ok());
    }

    #[test]
    fn validate_passes_when_only_one_blob_present_and_matches() {
        let sel = candidate("My Cube", true);
        let d = descriptor_blob_for(&sel.uuid, &sel.network);
        assert!(validate_blobs_match_selected(None, Some(&d), &sel).is_ok());
    }

    #[test]
    fn validate_rejects_seed_blob_with_wrong_uuid() {
        let sel = candidate("My Cube", true);
        let s = seed_blob_for("some-other-uuid", &sel.network);
        let err = validate_blobs_match_selected(Some(&s), None, &sel)
            .expect_err("expected mismatch error");
        assert!(err.contains("different Cube"), "got: {}", err);
    }

    #[test]
    fn validate_rejects_descriptor_blob_with_wrong_network() {
        let sel = candidate("My Cube", true);
        // selected is "mainnet"; blob is on testnet — legitimate
        // cross-cube mixup we must refuse (e.g. the cube-picker
        // filter failed upstream, or backend misrouted).
        let d = descriptor_blob_for(&sel.uuid, "testnet");
        assert!(validate_blobs_match_selected(None, Some(&d), &sel).is_err());
    }

    #[test]
    fn validate_rejects_when_either_blob_mismatches() {
        let sel = candidate("My Cube", true);
        // Seed matches but descriptor doesn't — still a reject.
        let s = seed_blob_for(&sel.uuid, &sel.network);
        let d = descriptor_blob_for("different-uuid", &sel.network);
        assert!(validate_blobs_match_selected(Some(&s), Some(&d), &sel).is_err());
    }

    #[test]
    fn validate_passes_when_both_blobs_absent() {
        // Trivial case — no blobs to mismatch against.
        let sel = candidate("My Cube", true);
        assert!(validate_blobs_match_selected(None, None, &sel).is_ok());
    }

    #[test]
    fn multiple_cubes_route_to_picker_regardless_of_kit_status() {
        // The picker view itself disables no-kit rows; we hand the
        // whole list through so the user can see context (e.g.
        // "that one's the one without a kit").
        let phase = phase_after_cubes_loaded(
            RestoreScope::Full,
            vec![candidate("Alice", true), candidate("Bob", false)],
        );
        match phase {
            Phase::CubePicker { cubes, selected } => {
                assert_eq!(cubes.len(), 2);
                assert!(selected.is_none());
            }
            other => panic!("expected CubePicker, got {:?}", other),
        }
    }

    #[test]
    fn picker_requires_available_selection_then_explicit_confirmation() {
        let mut available = candidate("Available", true);
        available.id = 7;
        let mut unavailable = candidate("No Kit", false);
        unavailable.id = 8;
        let mut step = RecoveryKitRestoreStep::new(RestoreScope::Full, "mainnet".into(), None);
        step.phase = Phase::CubePicker {
            cubes: vec![available, unavailable],
            selected: None,
        };
        let mut hws = HardwareWallets::new(
            crate::dir::CoincubeDirectory::new("/synthetic/recovery-picker".into()),
            Network::Bitcoin,
        );

        let _ = step.update(
            &mut hws,
            Message::RecoveryKitRestore(RecoveryKitRestoreMsg::ConfirmCube),
        );
        assert!(matches!(
            &step.phase,
            Phase::CubePicker { selected: None, .. }
        ));
        let _ = step.update(
            &mut hws,
            Message::RecoveryKitRestore(RecoveryKitRestoreMsg::SelectCube(8)),
        );
        assert!(matches!(
            &step.phase,
            Phase::CubePicker { selected: None, .. }
        ));
        let _ = step.update(
            &mut hws,
            Message::RecoveryKitRestore(RecoveryKitRestoreMsg::SelectCube(7)),
        );
        assert!(matches!(
            &step.phase,
            Phase::CubePicker {
                selected: Some(7),
                ..
            }
        ));
        let _ = step.update(
            &mut hws,
            Message::RecoveryKitRestore(RecoveryKitRestoreMsg::ConfirmCube),
        );
        assert!(matches!(&step.phase, Phase::PasswordEntry { selected, .. } if selected.id == 7));
    }

    #[test]
    fn successful_decrypt_waits_on_ready_without_advancing() {
        let selected = candidate("Recovered Cube", true);
        let seed = seed_blob_for(&selected.uuid, &selected.network);
        let mut step = RecoveryKitRestoreStep::new(RestoreScope::Full, "mainnet".into(), None);
        step.phase = Phase::Decrypting {
            selected: selected.clone(),
        };
        let mut hws = HardwareWallets::new(
            crate::dir::CoincubeDirectory::new("/synthetic/recovery-ready".into()),
            Network::Bitcoin,
        );

        let task = step.update(
            &mut hws,
            Message::RecoveryKitRestore(RecoveryKitRestoreMsg::DecryptResult(Ok((
                Some(seed),
                None,
            )))),
        );
        assert!(iced_runtime::task::into_stream(task).is_none());
        assert!(
            matches!(&step.phase, Phase::Ready { selected: ready, .. } if ready.uuid == selected.uuid)
        );
    }

    #[test]
    fn empty_password_message_does_not_start_decrypt() {
        let mut step = RecoveryKitRestoreStep::new(RestoreScope::Full, "mainnet".into(), None);
        step.phase = Phase::PasswordEntry {
            selected: candidate("Recovered Cube", true),
            attempts: 0,
        };
        let mut hws = HardwareWallets::new(
            crate::dir::CoincubeDirectory::new("/synthetic/recovery-empty-password".into()),
            Network::Bitcoin,
        );

        let task = step.update(
            &mut hws,
            Message::RecoveryKitRestore(RecoveryKitRestoreMsg::SubmitPassword),
        );
        assert!(iced_runtime::task::into_stream(task).is_none());
        assert!(matches!(step.phase, Phase::PasswordEntry { .. }));
        assert!(!step.processing);
    }

    #[test]
    fn zero_cubes_route_to_picker_which_renders_empty_state() {
        // Empty picker is the right surface — the picker view
        // renders "No Cubes found on this Connect account..." which
        // is more actionable than a generic error.
        let phase = phase_after_cubes_loaded(RestoreScope::Full, vec![]);
        match phase {
            Phase::CubePicker { cubes, .. } => assert!(cubes.is_empty()),
            other => panic!("expected CubePicker, got {:?}", other),
        }
    }

    // ── stage_restore: the shared DecryptedKit → installer-context seam ──
    // Reused by both the owner CRK restore (apply, above) and the heir
    // inheritance restore. These pin the scope discipline + error mapping
    // without standing up a full installer Context.

    /// A SeedBlob carrying a specific (valid) BIP39 phrase.
    fn seed_blob_with(phrase: &str) -> SeedBlob {
        use crate::services::recovery::{SeedBlobCube, SeedBlobMnemonic};
        SeedBlob {
            version: crate::services::recovery::plaintext::BLOB_VERSION,
            cube: SeedBlobCube {
                uuid: "uuid".to_string(),
                name: "n".to_string(),
                network: "bitcoin".to_string(),
                created_at: "2026-04-23T00:00:00Z".to_string(),
                lightning_address: None,
            },
            mnemonic: SeedBlobMnemonic {
                phrase: phrase.to_string(),
                language: "en".to_string(),
            },
        }
    }

    const VALID_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn stage_restore_descriptor_only_with_nothing_is_empty() {
        let staged =
            stage_restore(Network::Bitcoin, RestoreScope::DescriptorOnly, None, None).unwrap();
        assert!(staged.descriptor.is_none());
        assert!(staged.signer.is_none());
    }

    #[test]
    fn stage_restore_full_without_seed_errors() {
        // `StagedRestore` holds a `Signer` (no `Debug`), so match instead of
        // `unwrap_err`.
        let Err(err) = stage_restore(Network::Bitcoin, RestoreScope::Full, None, None) else {
            panic!("expected an error");
        };
        assert!(err.contains("Seed blob missing"), "got: {}", err);
    }

    #[test]
    fn stage_restore_unparseable_descriptor_errors() {
        // `descriptor_blob_for` uses the placeholder descriptor "d", which is
        // not a valid CoincubeDescriptor → parse failure.
        let d = descriptor_blob_for("uuid", "bitcoin");
        let Err(err) = stage_restore(
            Network::Bitcoin,
            RestoreScope::DescriptorOnly,
            None,
            Some(&d),
        ) else {
            panic!("expected an error");
        };
        assert!(err.contains("descriptor failed to parse"), "got: {}", err);
    }

    #[test]
    fn stage_restore_full_derives_signer_from_valid_mnemonic() {
        let seed = seed_blob_with(VALID_MNEMONIC);
        let staged =
            stage_restore(Network::Bitcoin, RestoreScope::Full, Some(&seed), None).unwrap();
        assert!(staged.signer.is_some(), "Full scope must derive a signer");
        assert!(staged.descriptor.is_none());
    }

    #[test]
    fn stage_restore_descriptor_only_ignores_present_seed() {
        let seed = seed_blob_with(VALID_MNEMONIC);
        let staged = stage_restore(
            Network::Bitcoin,
            RestoreScope::DescriptorOnly,
            Some(&seed),
            None,
        )
        .unwrap();
        assert!(
            staged.signer.is_none(),
            "DescriptorOnly must ignore the seed half"
        );
    }

    fn imported_blob_for_restore(uuid: &str) -> crate::services::recovery::ImportedDescriptorBlob {
        use coincube_core::{
            miniscript::bitcoin::{
                bip32::{DerivationPath, Xpriv, Xpub},
                secp256k1::Secp256k1,
            },
            wallet_descriptor::ImportedWalletDescriptor,
        };
        use std::str::FromStr;
        let secp = Secp256k1::new();
        let root = Xpriv::new_master(Network::Bitcoin, &[55; 32]).unwrap();
        let path = DerivationPath::from_str("m/84'/0'/0'").unwrap();
        let account = root.derive_priv(&secp, &path).unwrap();
        let key = format!(
            "[{}/84'/0'/0']{}",
            root.fingerprint(&secp),
            Xpub::from_priv(&secp, &account)
        );
        let descriptor = ImportedWalletDescriptor::parse(
            &format!("wpkh({key}/0/*)"),
            Some(&format!("wpkh({key}/1/*)")),
            crate::chain::ChainId::Bitcoin,
        )
        .unwrap();
        crate::services::recovery::ImportedDescriptorBlob::new(
            uuid.into(),
            crate::chain::ChainId::Bitcoin,
            crate::services::recovery::ImportedDescriptorBlobWallet {
                name: "Existing Bitcoin".into(),
                descriptor,
                birthday: 0,
                initial_lookahead: 200,
                receive_index: 37,
                change_index: 19,
            },
        )
        .unwrap()
    }

    #[test]
    fn imported_full_restore_stages_original_wallet_and_separate_cube_seed() {
        let blob = imported_blob_for_restore("restored-cube");
        let mut seed = seed_blob_with(VALID_MNEMONIC);
        seed.cube.uuid = blob.cube.uuid.clone();
        seed.cube.network = "mainnet".into();
        let mut selected = candidate("Recovered Cube", true);
        selected.uuid = blob.cube.uuid.clone();
        selected.network = "mainnet".into();
        let wallet = RecoveredWalletBlob::Imported(blob.clone());
        validate_wallet_blobs_match_selected(Some(&seed), Some(&wallet), &selected).unwrap();
        let mut step = RecoveryKitRestoreStep::new(RestoreScope::Full, "mainnet".into(), None);
        step.phase = Phase::Ready {
            selected,
            seed: Some(seed),
            descriptor: Some(wallet),
        };
        let mut ctx = Context::new(
            Network::Bitcoin,
            crate::dir::CoincubeDirectory::new(
                std::env::temp_dir().join("imported-restore-context-only"),
            ),
            None,
            None,
        );
        assert!(step.apply(&mut ctx));
        assert_eq!(ctx.cube_id.as_deref(), Some("restored-cube"));
        assert!(ctx.recovered_signer.is_some());
        assert!(ctx.descriptor.is_none());
        let restored = ctx.restored_imported_wallet.as_ref().unwrap();
        assert_eq!(restored.wallet.descriptor, blob.wallet.descriptor);
        assert_eq!(
            (restored.wallet.receive_index, restored.wallet.change_index),
            (37, 19)
        );
        assert_eq!(ctx.restore_source, Some(RestoreSource::PasswordKit));
        step.revert(&mut ctx);
        assert!(ctx.restored_imported_wallet.is_none());
        assert!(ctx.recovered_signer.is_none());
    }

    #[test]
    fn imported_descriptor_restore_requires_exact_existing_cube_and_ignores_seed() {
        let blob = imported_blob_for_restore("existing-cube");
        let mut selected = candidate("Existing Cube", true);
        selected.uuid = blob.cube.uuid.clone();
        selected.network = "mainnet".into();
        let mut ctx = Context::new(
            Network::Bitcoin,
            crate::dir::CoincubeDirectory::new(
                std::env::temp_dir().join("imported-restore-context-only"),
            ),
            None,
            None,
        );
        ctx.cube_id = Some("wrong-cube".into());
        let mut step =
            RecoveryKitRestoreStep::new(RestoreScope::DescriptorOnly, "mainnet".into(), None);
        step.phase = Phase::Ready {
            selected: selected.clone(),
            seed: Some(seed_blob_with(VALID_MNEMONIC)),
            descriptor: Some(RecoveredWalletBlob::Imported(blob.clone())),
        };
        assert!(!step.apply(&mut ctx));
        assert!(ctx.restored_imported_wallet.is_none());
        assert!(ctx.recovered_signer.is_none());
        ctx.cube_id = Some("existing-cube".into());
        step.phase = Phase::Ready {
            selected,
            seed: Some(seed_blob_with(VALID_MNEMONIC)),
            descriptor: Some(RecoveredWalletBlob::Imported(blob)),
        };
        assert!(step.apply(&mut ctx));
        assert!(ctx.recovered_signer.is_none());
        assert!(ctx.descriptor.is_none());
        assert!(ctx.restored_imported_wallet.is_some());
        assert_eq!(ctx.cube_id.as_deref(), Some("existing-cube"));
    }
}
