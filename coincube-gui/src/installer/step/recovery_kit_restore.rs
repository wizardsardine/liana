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

use std::sync::Arc;

use coincube_core::{descriptors::CoincubeDescriptor, signer::MasterSigner};
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
        coincube::{CoincubeClient, CubeResponse, OtpRequest, OtpVerifyRequest, RecoveryKitStatus},
        recovery::{
            restore::{fetch_and_decrypt_kit, DecryptedKit, RestoreError},
            score_password, DescriptorBlob, PasswordStrength, SeedBlob,
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
    /// Show cubes; user selects one to restore from. `selected` is
    /// reserved for a future "highlight-then-confirm" UX; today the
    /// picker advances on click, so it's always `None` in practice.
    #[allow(dead_code)]
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
        descriptor: Option<DescriptorBlob>,
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
pub struct RecoveryKitRestoreStep {
    scope: RestoreScope,
    /// Network filter applied to the cube picker. The user can only
    /// restore into the same network they're installing for.
    network_filter: String,
    client: CoincubeClient,
    phase: Phase,
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
    pub fn new(scope: RestoreScope, network_filter: String) -> Self {
        Self {
            scope,
            network_filter,
            client: CoincubeClient::new(),
            phase: Phase::Email,
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

    fn set_phase(&mut self, phase: Phase) {
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
        Task::perform(
            async move {
                let all = client.list_cubes().await.map_err(|e| e.to_string())?;
                // Filter to cubes on the requested network. We don't
                // filter by `has_recovery_kit` at list time because the
                // kit status lives on a separate endpoint — fetch
                // status in parallel.
                let matches: Vec<CubeResponse> =
                    all.into_iter().filter(|c| c.network == network).collect();
                let mut out = Vec::with_capacity(matches.len());
                for cube in matches {
                    let status = client.get_recovery_kit_status(cube.id).await.unwrap_or(
                        RecoveryKitStatus {
                            has_recovery_kit: false,
                            has_encrypted_seed: false,
                            has_encrypted_wallet_descriptor: false,
                            encryption_scheme: String::new(),
                            created_at: None,
                            updated_at: None,
                        },
                    );
                    out.push(RestoreCubeCandidate {
                        id: cube.id,
                        uuid: cube.uuid,
                        name: cube.name,
                        network: cube.network,
                        status,
                    });
                }
                Ok(out)
            },
            |res: Result<Vec<RestoreCubeCandidate>, String>| {
                Message::RecoveryKitRestore(RecoveryKitRestoreMsg::CubesLoaded(res))
            },
        )
    }

    fn decrypt_task(&self, cube_id: u64) -> Task<Message> {
        let client = self.client.clone();
        let password = Zeroizing::new(self.password.to_string());
        Task::perform(
            async move {
                fetch_and_decrypt_kit(&client, cube_id, &password)
                    .await
                    .map(|DecryptedKit { seed, descriptor }| (seed, descriptor))
                    .map_err(map_restore_error)
            },
            |res| Message::RecoveryKitRestore(RecoveryKitRestoreMsg::DecryptResult(res)),
        )
    }
}

fn map_restore_error(e: RestoreError) -> String {
    // `RestoreError` already formats cleanly — just stringify. The
    // UI uses the string for the inline banner; typed branching on
    // specific errors (retry_after, bad password) happens at the
    // message dispatch level if we want to add it later.
    e.to_string()
}

impl From<RecoveryKitRestoreStep> for Box<dyn Step> {
    fn from(s: RecoveryKitRestoreStep) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl Step for RecoveryKitRestoreStep {
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
            RecoveryKitRestoreMsg::OtpEdited(value) => {
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
                    Ok(cubes) => {
                        if cubes.len() == 1 {
                            // Auto-select the unambiguous case and
                            // immediately advance to password entry.
                            let c = cubes.into_iter().next().unwrap();
                            self.set_phase(Phase::PasswordEntry {
                                selected: c,
                                attempts: 0,
                            });
                        } else {
                            self.set_phase(Phase::CubePicker {
                                cubes,
                                selected: None,
                            });
                        }
                    }
                    Err(e) => {
                        self.set_phase(Phase::Error {
                            message: format!("Couldn't load cubes: {}", e),
                        });
                    }
                }
                Task::none()
            }
            RecoveryKitRestoreMsg::SelectCube(id) => {
                if let Phase::CubePicker { cubes, .. } = &self.phase {
                    if let Some(c) = cubes.iter().find(|c| c.id == id).cloned() {
                        self.set_phase(Phase::PasswordEntry {
                            selected: c,
                            attempts: 0,
                        });
                    }
                }
                Task::none()
            }
            RecoveryKitRestoreMsg::PasswordEdited(v) => {
                // Move the `Zeroizing<String>` wrapper straight into
                // state; the old `self.password` is dropped here
                // (its heap zeroes via Zeroizing's Drop).
                self.password = v;
                Task::none()
            }
            RecoveryKitRestoreMsg::SubmitPassword => {
                if self.processing {
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
                // validator of "is this the right password". The
                // view disables Submit when the input is empty, so
                // no input-required guard is needed here either.
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
            RecoveryKitRestoreMsg::DecryptResult(res) => {
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
                                message:
                                    "This Cube's Recovery Kit doesn't include the data needed \
                                     for this restore."
                                        .to_string(),
                            });
                            return Task::none();
                        }
                        self.set_phase(Phase::Ready {
                            selected,
                            seed,
                            descriptor,
                        });
                        // Auto-advance — the user clicked Submit and the
                        // decrypt succeeded; no reason to make them
                        // click Next again.
                        Task::done(Message::Next)
                    }
                    Err(e) => {
                        // Wrong password / corrupt envelope — keep the
                        // user on PasswordEntry with an inline banner.
                        self.set_phase(Phase::PasswordEntry {
                            selected,
                            attempts: 1, // TODO wire backoff counter
                        });
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }
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

        // Apply the descriptor blob. The blob carries the full
        // descriptor string including inline xpubs, so parsing it
        // rebuilds a live `CoincubeDescriptor` without needing the
        // signer set yet.
        if let Some(desc_blob) = descriptor {
            match desc_blob.vault.descriptor.parse::<CoincubeDescriptor>() {
                Ok(parsed) => {
                    ctx.descriptor = Some(parsed);
                }
                Err(e) => {
                    self.set_phase(Phase::Error {
                        message: format!(
                            "Recovery Kit descriptor failed to parse: {}. Kit may be from a \
                             newer client version.",
                            e
                        ),
                    });
                    return false;
                }
            }
        }

        // Apply the seed blob (W13 only). We install the mnemonic into
        // a fresh `MasterSigner` and stash it on the context the same
        // way `RecoverMnemonic::apply` does for a typed-in mnemonic.
        if matches!(self.scope, RestoreScope::Full) {
            let Some(seed_blob) = seed else {
                // `Ready` phase should have had the seed already —
                // defensive check in case the phase was populated
                // with a DescriptorOnly kit.
                self.set_phase(Phase::Error {
                    message: "Seed blob missing from decrypted kit.".to_string(),
                });
                return false;
            };
            match MasterSigner::from_str(ctx.bitcoin_config.network, &seed_blob.mnemonic.phrase) {
                Ok(master) => {
                    let signer = Signer::new(master);
                    ctx.recovered_signer = Some(Arc::new(signer));
                }
                Err(e) => {
                    self.set_phase(Phase::Error {
                        message: format!("Restored mnemonic failed to derive a signer: {}", e),
                    });
                    return false;
                }
            }
        }

        true
    }

    fn revert(&self, ctx: &mut Context) {
        // Back-out: drop anything this step wrote to context so the
        // user can re-do the restore cleanly.
        if matches!(self.scope, RestoreScope::Full) {
            ctx.recovered_signer = None;
        }
        // Always clear descriptor — if the user navigates back from a
        // later step through this one, keeping a stale descriptor would
        // leak into a subsequent decision.
        ctx.descriptor = None;
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        _email: Option<&'a str>,
    ) -> Element<'a, Message> {
        // Placeholder view — re-uses existing installer text layout.
        // A polished design pass is a follow-up; what matters here is
        // that each Phase has a reachable interaction so the flow
        // progresses.
        view::recovery_kit_restore(
            progress,
            self.scope,
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
