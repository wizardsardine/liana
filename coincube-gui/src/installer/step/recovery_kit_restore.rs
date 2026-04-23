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
        coincube::{
            CoincubeClient, CoincubeError, CubeResponse, OtpRequest, OtpVerifyRequest,
            RecoveryKitStatus,
        },
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
                // Pass the typed `RestoreError` through unchanged so
                // the handler (see `DecryptResult` arm in `update`)
                // can branch on `BadPasswordOrCorrupt` vs. terminal
                // variants like `RateLimited` / `Api` / `NotFound` /
                // `BlobParse`. Stringifying here would collapse them
                // into an untyped bag and force the wrong UX.
                fetch_and_decrypt_kit(&client, cube_id, &password)
                    .await
                    .map(|DecryptedKit { seed, descriptor }| (seed, descriptor))
            },
            |res| Message::RecoveryKitRestore(RecoveryKitRestoreMsg::DecryptResult(res)),
        )
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
/// so the auto-select vs picker vs error decision is unit-testable
/// without standing up the whole installer step.
///
/// Branches:
/// - Exactly one cube with the required half → auto-select it
///   (common restore path)
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
            Phase::PasswordEntry {
                selected: c,
                attempts: 0,
            }
        } else {
            let (needed, hint) = match scope {
                RestoreScope::Full => (
                    "a Master Seed Phrase",
                    "the kit was backed up without the seed half (passkey Cubes can't \
                     include it), and Full restore needs it",
                ),
                RestoreScope::DescriptorOnly => (
                    "a Wallet Descriptor",
                    "the kit was backed up seed-only (the Cube had no Vault at backup \
                     time)",
                ),
            };
            Phase::Error {
                message: format!(
                    "\"{}\" doesn't have {} backed up on Connect — {}. Finish the \
                     backup from a device where the Cube is already installed, then \
                     return here to restore.",
                    c.name, needed, hint,
                ),
            }
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
    /// Pick up an already-authenticated Connect session from
    /// `Context` the first time this step becomes active.
    ///
    /// Today the Recovery-Kit restore flow is typically launched from
    /// a launcher row whose "remote cubes" list only exists because
    /// the user already signed into Connect. That session lives on
    /// the launcher's `ConnectAccountPanel` and gets forwarded into
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
        // base URL / HTTP plumbing the launcher has already configured.
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
                    Ok(cubes) => self.set_phase(phase_after_cubes_loaded(self.scope, cubes)),
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
                    // Re-check `has_required_half` in case the view's
                    // disabled-row rendering ever falls out of sync
                    // with the state-machine gate. Defensive — the
                    // picker's `on_press` is gated on the same
                    // predicate.
                    if let Some(c) = cubes
                        .iter()
                        .find(|c| c.id == id && has_required_half(self.scope, c))
                        .cloned()
                    {
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
                        if let Err(msg) = validate_blobs_match_selected(
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
                        // Auto-advance — the user clicked Submit and the
                        // decrypt succeeded; no reason to make them
                        // click Next again.
                        Task::done(Message::Next)
                    }
                    Err(e) => {
                        if is_retryable_on_password_screen(&e) {
                            self.set_phase(Phase::PasswordEntry {
                                selected,
                                attempts: 1, // TODO wire backoff counter
                            });
                            self.error = Some(e.to_string());
                        } else {
                            self.set_phase(Phase::Error {
                                message: e.to_string(),
                            });
                        }
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

        // Stage 1 — parse the descriptor into a local. If this fails
        // we transition to `Phase::Error` and bail without touching
        // `ctx`. Keeping the parse result in a local instead of
        // writing it straight through to `ctx.descriptor` means a
        // subsequent seed-derivation failure won't leave a partially-
        // applied context behind, which would survive until the user
        // re-enters the step and could be picked up by a downstream
        // step that wasn't expecting a populated `ctx.descriptor`.
        let staged_descriptor: Option<CoincubeDescriptor> = match descriptor {
            Some(desc_blob) => match desc_blob.vault.descriptor.parse::<CoincubeDescriptor>() {
                Ok(parsed) => Some(parsed),
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
            },
            None => None,
        };

        // Stage 2 — derive the seed signer into a local (W13 only).
        // Same all-or-nothing discipline as stage 1.
        let staged_signer: Option<Signer> = if matches!(self.scope, RestoreScope::Full) {
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
                Ok(master) => Some(Signer::new(master)),
                Err(e) => {
                    self.set_phase(Phase::Error {
                        message: format!("Restored mnemonic failed to derive a signer: {}", e),
                    });
                    return false;
                }
            }
        } else {
            None
        };

        // Stage 3 — commit both results atomically. We only reach
        // here after every fallible step above succeeded, so `ctx`
        // transitions from "nothing applied" to "fully applied" in
        // one go. `Arc::new` happens here rather than at staging so
        // we don't allocate the refcounted handle for a signer that
        // ultimately gets dropped on the error path.
        if let Some(d) = staged_descriptor {
            ctx.descriptor = Some(d);
        }
        if let Some(s) = staged_signer {
            ctx.recovered_signer = Some(Arc::new(s));
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
            ctx.connect_jwt = Some(jwt.to_string());
            ctx.use_coincube_connect = true;
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

#[cfg(test)]
mod tests {
    use super::*;

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
            },
        }
    }

    // Regression tests for the auto-select-skips-kit-check bug. Before
    // this fix, a single cube without a kit would land the user on
    // the password screen, which would then 404 at decrypt time —
    // a worse UX than a direct "no kit yet" message up front.

    #[test]
    fn single_cube_with_kit_auto_advances_to_password() {
        let phase = phase_after_cubes_loaded(RestoreScope::Full, vec![candidate("My Cube", true)]);
        match phase {
            Phase::PasswordEntry { selected, attempts } => {
                assert_eq!(selected.name, "My Cube");
                assert_eq!(attempts, 0);
            }
            other => panic!("expected PasswordEntry, got {:?}", other),
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
}
