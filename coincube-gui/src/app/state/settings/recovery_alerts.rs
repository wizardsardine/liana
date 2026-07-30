//! Vault Recovery Alerts settings card (Estate Notifications — PR 2, cleaned up
//! per `PLAN-recovery-alerts-cleanup.md`).
//!
//! Per-vault opt-in for recovery-path monitoring. Modeled on the Cube Recovery
//! Kit card (`recovery_kit.rs`): the state container lives on the outer
//! `SettingsState` so `App::update` can inject the authenticated
//! `CoincubeClient`, the Connect cube id, the live wallet descriptor, the
//! keyholder list, and the entitlements — none of which are plumbed through
//! `State::update`.
//!
//! The card exposes **two independent controls** backed by two server records:
//!
//! - **Recovery alerts** (the monitoring record) — a timelock heartbeat so
//!   COINCUBE emails keyholders when the recovery window opens. The server only
//!   ever learns one block height and that this desktop checked in — never
//!   addresses or balances. Not Estate-gated.
//! - **What keyholders can recover** (the ECIES escrow set) — **Nothing —
//!   alerts only** / **Vault only** (descriptor sealed to each keyholder's key)
//!   / **Full Cube** (seed + descriptor). Estate-gated (`inheritanceEscrow`).
//!
//! The escrow **tier is derived from the server**, not tracked per-session:
//! `VaultMonitoringStatus::escrowed_artifacts` reports exactly which kinds are
//! escrowed, so a reload or a restart shows the true selection. See
//! [`RecoveryAlerts::escrow_tier`]. Invariant: escrow present ⟹ alerts on.

use std::sync::Arc;

use iced::Task;
use zeroize::Zeroizing;

use crate::{
    app::{cache::Cache, message::Message, settings, view, wallet::Wallet},
    services::coincube::{CoincubeClient, CubeMember, VaultMonitoringLevel, VaultMonitoringStatus},
    services::inheritance::{
        disable_alerts, disable_escrow, enable_alerts, enroll_escrow, EscrowTier,
    },
    services::recovery::{SeedBlob, SeedBlobCube, SeedBlobMnemonic, BLOB_VERSION},
};

use view::{EscrowPin, RecoveryAlertsMessage, SettingsMessage};

/// Settings-card state for Vault Recovery Alerts. Held on `SettingsState`.
#[derive(Debug)]
pub struct RecoveryAlerts {
    /// Connect vault id, resolved from the cube on first load and cached so
    /// alerts/escrow changes don't re-resolve it.
    pub vault_id: Option<u64>,
    /// Last-known monitoring status from Connect. `None` until first load.
    /// Both the alerts state ([`Self::alerts_on`]) and the escrow tier
    /// ([`Self::escrow_tier`]) are derived from this — no session-tracked guess.
    pub status: Option<VaultMonitoringStatus>,
    pub loading: bool,
    pub submitting: bool,
    pub error: Option<String>,
    /// Keyholder emails (the cube members who'd be notified), snapshotted on
    /// each load so the card can show exactly who would receive alerts.
    pub keyholders: Vec<String>,
    /// Whether the account carries the `recovery_alerts` entitlement (gates the
    /// alerts toggle / unlocks the card). Universal after API PR 3; kept as a
    /// defense-in-depth check.
    pub entitled: bool,
    /// Whether the account carries the Estate `inheritanceEscrow` entitlement
    /// (gates the escrow tier selector, separately from the alerts toggle).
    pub escrow_entitled: bool,
    /// True once a load resolved that there's no Connect vault to monitor
    /// (no cube registered / no vault created yet).
    pub no_vault: bool,
    /// True once at least one load has been attempted — lets the card pick
    /// the right loading vs. empty copy.
    pub loaded_once: bool,
    /// True while the alerts-off confirm dialog is open. Turning alerts off is
    /// never silent: when a recovery kit is escrowed the dialog discloses that
    /// it will also be deleted (escrow can't outlive alerts).
    pub confirming_alerts_off: bool,
    /// True while the card is collecting the owner's PIN to unlock the seed for
    /// a Full-Cube enrolment (the only tier that escrows the seed).
    pub awaiting_pin: bool,
    /// PIN digits being entered for a Full-Cube enrolment. Held in a redacting,
    /// zeroizing [`EscrowPin`] so it never prints via the struct's `Debug` and
    /// each keystroke's previous buffer is wiped. Cleared as soon as it's
    /// consumed (or the flow is cancelled).
    pub pin: EscrowPin,
}

impl Default for RecoveryAlerts {
    fn default() -> Self {
        Self::new()
    }
}

impl RecoveryAlerts {
    pub fn new() -> Self {
        Self {
            vault_id: None,
            status: None,
            loading: false,
            submitting: false,
            error: None,
            keyholders: Vec::new(),
            entitled: false,
            escrow_entitled: false,
            no_vault: false,
            loaded_once: false,
            confirming_alerts_off: false,
            awaiting_pin: false,
            pin: EscrowPin::default(),
        }
    }

    /// Whether recovery **alerts** (the monitoring record) are on — any non-Off
    /// monitoring level. The alerts toggle and the heartbeat both key off this.
    pub fn alerts_on(&self) -> bool {
        !matches!(self.level(), VaultMonitoringLevel::Off)
    }

    /// The escrow tier keyholders can recover, **derived from the server's
    /// reported escrowed-artifact kinds** rather than a session guess:
    /// - `["…","seed"]` → `Some(FullCube)`;
    /// - `["descriptor"]` → `Some(VaultOnly)`;
    /// - `[]` (present, empty) → `Some(Off)` — nothing escrowed (alerts-only);
    /// - field **absent** (older API that predates the report): `Some(Off)` when
    ///   monitoring is off (nothing can be escrowed then — confident), else
    ///   `None` ("on, tier unknown"). Returning `None` avoids asserting — and
    ///   mis-stating — a tier the server didn't confirm (C2).
    pub fn escrow_tier(&self) -> Option<EscrowTier> {
        match self
            .status
            .as_ref()
            .and_then(|s| s.escrowed_artifacts.as_ref())
        {
            Some(kinds) if kinds.iter().any(|k| k == "seed") => Some(EscrowTier::FullCube),
            Some(kinds) if kinds.iter().any(|k| k == "descriptor") => Some(EscrowTier::VaultOnly),
            Some(_) => Some(EscrowTier::Off),
            None => {
                if matches!(self.level(), VaultMonitoringLevel::Off) {
                    Some(EscrowTier::Off)
                } else {
                    None
                }
            }
        }
    }

    /// Whether an escrow set is currently stored (Vault-only or Full-Cube). Used
    /// to decide whether an alerts-off must also delete the kit, and whether the
    /// confirm dialog discloses that deletion.
    pub fn has_escrow(&self) -> bool {
        matches!(
            self.escrow_tier(),
            Some(EscrowTier::VaultOnly | EscrowTier::FullCube)
        )
    }

    /// Current monitoring level (Off when unloaded).
    pub fn level(&self) -> VaultMonitoringLevel {
        self.status
            .as_ref()
            .map(|s| s.level)
            .unwrap_or(VaultMonitoringLevel::Off)
    }

    /// Fold a change result into state.
    ///
    /// **Ok:** cache the returned `status`. Each owner operation stamps the
    /// resulting `escrowed_artifacts` onto that status, so caching it *is*
    /// applying the confirmed tier immediately — the card reflects the change
    /// without flicker while the next `LoadStatus` reconciles against the
    /// server. Also closes the alerts-off dialog.
    ///
    /// **Err:** surface the (display-safe) error. A failed **Full-Cube** enrol
    /// also restores `awaiting_pin`: `ConfirmFullCube` clears it before the
    /// blocking verify, so a wrong PIN would otherwise hide the PIN entry and
    /// force the owner to re-pick Full Cube. Restoring it re-shows the (now
    /// empty — the buffer was taken) PIN field alongside the error for an inline
    /// retry. `tier_change` is carried only to identify that Full-Cube case.
    fn apply_change(
        &mut self,
        res: Result<VaultMonitoringStatus, String>,
        tier_change: Option<EscrowTier>,
    ) {
        match res {
            Ok(status) => {
                self.status = Some(status);
                self.error = None;
                self.confirming_alerts_off = false;
            }
            Err(e) => {
                self.error = Some(e);
                if matches!(tier_change, Some(EscrowTier::FullCube)) {
                    self.awaiting_pin = true;
                }
            }
        }
    }

    /// Fold a successful `LoadStatus` into state. The status carries the
    /// server-reported `escrowed_artifacts`, so caching it is enough — both the
    /// alerts state and the escrow tier are derived from it. No session tier to
    /// reset.
    fn apply_status_loaded(&mut self, vault_id: u64, status: VaultMonitoringStatus) {
        self.vault_id = Some(vault_id);
        self.status = Some(status);
        self.no_vault = false;
        self.error = None;
    }
}

/// Wrap a [`RecoveryAlertsMessage`] in the settings-message envelope.
fn ra_msg(m: RecoveryAlertsMessage) -> Message {
    Message::View(view::Message::Settings(SettingsMessage::RecoveryAlerts(m)))
}

/// App-level dispatcher. `client` / `server_cube_id` / `wallet` / `members`
/// are injected by `App::update`; `entitled` is the account's `recovery_alerts`
/// entitlement (alerts toggle) and `escrow_entitled` the `inheritanceEscrow` one
/// (escrow tier selector). `session_generation` is the connect account's current
/// session counter (bumped on login / logout / reset): it's stamped into spawned
/// async results so a load / change that lands after the session changed is
/// dropped instead of writing a prior account's vault id + status into the reset
/// state — the same guard the duress-contacts handlers use.
#[allow(clippy::too_many_arguments)]
pub fn update(
    ra: &mut RecoveryAlerts,
    msg: RecoveryAlertsMessage,
    client: Option<CoincubeClient>,
    server_cube_id: Option<u64>,
    wallet: Option<Arc<Wallet>>,
    entitled: bool,
    escrow_entitled: bool,
    members: &[CubeMember],
    session_generation: u64,
    cache: &Cache,
    local_cube_id: &str,
) -> Task<Message> {
    ra.entitled = entitled;
    ra.escrow_entitled = escrow_entitled;
    match msg {
        RecoveryAlertsMessage::LoadStatus => {
            // Refresh the keyholder snapshot every load so the card reflects
            // the current cube membership.
            ra.keyholders = members.iter().map(|m| m.user.email.clone()).collect();
            ra.error = None;

            let (Some(client), Some(cube_id)) = (client, server_cube_id) else {
                // No Connect session or no registered cube → nothing to
                // monitor. Mark `no_vault` so the card shows the right copy.
                // Clear any cached vault id/status too: the cube-deregistered
                // case lands here (not the not-found arm, which only runs
                // after a fetch), and a stale id + non-Off level would keep
                // `recovery_heartbeat_task` POSTing to a vault that's gone.
                ra.no_vault = server_cube_id.is_none();
                ra.vault_id = None;
                ra.status = None;
                ra.loaded_once = true;
                return Task::none();
            };
            if !entitled {
                // Non-Estate: don't hit the network; the card shows the
                // locked affordance. Clear any in-flight `loading`: a prior
                // entitled fetch can still be marked loading here — e.g. after
                // an in-place account switch to a non-Estate account, where the
                // session bump drops the old fetch's `StatusLoaded` without
                // resetting this state — and a stuck `loading` would keep the
                // card on "Loading…" and block the heartbeat's `!loading`
                // hydration gate.
                ra.loading = false;
                // Deliberately leave `loaded_once` false: nothing was resolved
                // here, and the heartbeat's lazy hydration is gated on
                // `!loaded_once && entitled`. If we marked it loaded, a later
                // upgrade to Estate would never re-fire that hydration and
                // background heartbeats would never start until Settings is
                // reopened. The entitlement gate keeps this from dispatching
                // spuriously while unentitled.
                return Task::none();
            }
            ra.loading = true;
            Task::perform(
                async move {
                    let vault = client
                        .get_connect_vault(cube_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    let status = client
                        .get_vault_monitoring(cube_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok((vault.id, status))
                },
                move |res| ra_msg(RecoveryAlertsMessage::StatusLoaded(res, session_generation)),
            )
        }
        RecoveryAlertsMessage::StatusLoaded(res, gen) => {
            // Drop a load that resolved after the session changed (logout /
            // account switch) so we never write the prior account's vault id +
            // status into the freshly-reset state.
            if gen != session_generation {
                return Task::none();
            }
            ra.loading = false;
            ra.loaded_once = true;
            match res {
                Ok((vid, status)) => {
                    // Cache the freshly-loaded status. Both the alerts state and
                    // the escrow tier derive from its `escrowed_artifacts`, so
                    // the reload reflects the true server selection (including
                    // changes made on another device). See `escrow_tier`.
                    ra.apply_status_loaded(vid, status);
                }
                Err(e) => {
                    // A missing Connect vault (the cube has no vault yet, or a
                    // previously-resolved vault was removed) surfaces as a
                    // not-found error; treat it as "nothing to monitor" rather
                    // than a hard error so the card degrades gracefully. Clear
                    // any cached vault id and status too: otherwise
                    // `recovery_heartbeat_task` would keep POSTing heartbeats
                    // (level != Off && vault_id set) to a vault Connect no
                    // longer has, even though the card now shows nothing to
                    // monitor. A later successful load re-resolves both.
                    if e.to_lowercase().contains("not found") {
                        ra.no_vault = true;
                        ra.vault_id = None;
                        ra.status = None;
                    } else {
                        ra.error = Some(e);
                    }
                }
            }
            Task::none()
        }
        RecoveryAlertsMessage::ToggleAlerts(on) => {
            // The alerts toggle keys off the (universal after API PR 3)
            // `recovery_alerts` entitlement — kept as defense-in-depth. The card
            // only renders the toggle when entitled, so this guard is belt-and-braces.
            if !entitled {
                ra.error = Some("Recovery alerts aren't available on this plan.".to_string());
                return Task::none();
            }
            ra.error = None;
            if on {
                // Turning alerts ON alone: no escrow involved. No-op if already on.
                if ra.alerts_on() {
                    return Task::none();
                }
                let (Some(client), Some(_vault_id), Some(server_cube_id)) =
                    (client, ra.vault_id, server_cube_id)
                else {
                    ra.error = Some(
                        "Couldn't find this Vault on Connect yet — try again in a moment."
                            .to_string(),
                    );
                    return Task::none();
                };
                ra.submitting = true;
                Task::perform(
                    async move {
                        enable_alerts(&client, server_cube_id)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    move |res| {
                        // No escrow change → `None` (no PIN re-show on failure).
                        ra_msg(RecoveryAlertsMessage::ChangeResult(
                            res,
                            session_generation,
                            None,
                        ))
                    },
                )
            } else {
                // Turning alerts OFF is destructive when a kit is escrowed, so
                // open the confirm dialog rather than acting silently. No-op if
                // already off.
                if !ra.alerts_on() {
                    return Task::none();
                }
                ra.confirming_alerts_off = true;
                Task::none()
            }
        }
        RecoveryAlertsMessage::CancelAlertsOff => {
            ra.confirming_alerts_off = false;
            Task::none()
        }
        RecoveryAlertsMessage::ConfirmAlertsOff => {
            if !entitled || !ra.confirming_alerts_off {
                return Task::none();
            }
            // Escrow can't outlive alerts (invariant: escrow present ⟹ alerts
            // on), so turning alerts off deletes any kit too. Delete unless we're
            // *certain* nothing is escrowed (`Some(Off)`): when the tier is
            // unknown (older API that doesn't report it), delete anyway — the
            // escrow DELETE is idempotent (404 = success), so this is free
            // insurance against ever stranding escrow with monitoring off.
            let also_delete_escrow = !matches!(ra.escrow_tier(), Some(EscrowTier::Off));
            let (Some(client), Some(_vault_id), Some(server_cube_id)) =
                (client, ra.vault_id, server_cube_id)
            else {
                ra.error = Some(
                    "Couldn't find this Vault on Connect yet — try again in a moment.".to_string(),
                );
                return Task::none();
            };
            ra.submitting = true;
            ra.confirming_alerts_off = false;
            ra.error = None;
            Task::perform(
                async move {
                    disable_alerts(&client, server_cube_id, also_delete_escrow)
                        .await
                        .map_err(|e| e.to_string())
                },
                move |res| {
                    // Resulting escrow tier is Off; `Some(Off)` (not FullCube) so
                    // a failure never spuriously re-shows the PIN entry.
                    ra_msg(RecoveryAlertsMessage::ChangeResult(
                        res,
                        session_generation,
                        Some(EscrowTier::Off),
                    ))
                },
            )
        }
        RecoveryAlertsMessage::SelectEscrow(tier) => {
            ra.awaiting_pin = false;
            ra.pin.clear();
            if !escrow_entitled {
                ra.error =
                    Some("An encrypted recovery kit is part of the Estate plan.".to_string());
                return Task::none();
            }
            // Skip a no-op re-select of the *known* current tier. When it's
            // unknown on this device (`None`, older API), any selection proceeds
            // so the owner can confirm/change it.
            if ra.escrow_tier() == Some(tier) {
                return Task::none();
            }
            // `ra.vault_id` is checked (not used further) as the "does Connect
            // know about a Vault for this cube yet" gate — escrow/monitoring
            // calls below are cube-scoped and never need the vault id itself.
            let (Some(client), Some(_vault_id), Some(server_cube_id)) =
                (client, ra.vault_id, server_cube_id)
            else {
                ra.error = Some(
                    "Couldn't find this Vault on Connect yet — try again in a moment.".to_string(),
                );
                return Task::none();
            };
            match tier {
                EscrowTier::Off => {
                    // "Nothing — alerts only": delete the escrow set, keep alerts.
                    ra.submitting = true;
                    ra.error = None;
                    Task::perform(
                        async move {
                            disable_escrow(&client, server_cube_id)
                                .await
                                .map_err(|e| e.to_string())
                        },
                        move |res| {
                            ra_msg(RecoveryAlertsMessage::ChangeResult(
                                res,
                                session_generation,
                                Some(EscrowTier::Off),
                            ))
                        },
                    )
                }
                EscrowTier::VaultOnly => {
                    // Descriptor-only escrow needs no seed (no PIN). Build the
                    // descriptor blob from the live wallet and enrol. `enroll_escrow`
                    // turns alerts on too, so this auto-enables alerts when off.
                    let Some(descriptor_json) =
                        descriptor_blob_json(wallet.as_deref(), local_cube_id, cache.network)
                    else {
                        ra.error = Some(
                            "This Vault's descriptor isn't available on this device, so recovery \
                             escrow can't be set up here."
                                .to_string(),
                        );
                        return Task::none();
                    };
                    ra.submitting = true;
                    ra.error = None;
                    Task::perform(
                        async move {
                            enroll_escrow(&client, server_cube_id, descriptor_json, None)
                                .await
                                .map_err(|e| e.to_string())
                        },
                        move |res| {
                            ra_msg(RecoveryAlertsMessage::ChangeResult(
                                res,
                                session_generation,
                                Some(EscrowTier::VaultOnly),
                            ))
                        },
                    )
                }
                EscrowTier::FullCube => {
                    // Full-Cube escrows the seed too — re-confirm the PIN before
                    // exporting it. Collect the PIN; `ConfirmFullCube` enrols.
                    if wallet.is_none() {
                        ra.error = Some(
                            "This Vault isn't available on this device, so full-Cube escrow can't \
                             be set up here."
                                .to_string(),
                        );
                        return Task::none();
                    }
                    ra.error = None;
                    ra.awaiting_pin = true;
                    ra.pin.clear();
                    Task::none()
                }
            }
        }
        RecoveryAlertsMessage::EscrowPinChanged(pin) => {
            ra.pin = pin;
            Task::none()
        }
        RecoveryAlertsMessage::CancelFullCube => {
            ra.awaiting_pin = false;
            ra.pin.clear();
            ra.error = None;
            Task::none()
        }
        RecoveryAlertsMessage::ConfirmFullCube => {
            // Full-Cube is an escrow tier, so it's gated on `inheritanceEscrow`.
            if !escrow_entitled || !ra.awaiting_pin {
                return Task::none();
            }
            // We still require a resolved vault as a fast precondition, but
            // `enroll_escrow` re-fetches and uses that vault's id, so we don't
            // thread a (possibly stale) cached id through.
            let (Some(client), Some(_), Some(server_cube_id)) =
                (client, ra.vault_id, server_cube_id)
            else {
                ra.error = Some(
                    "Couldn't find this Vault on Connect yet — try again in a moment.".to_string(),
                );
                return Task::none();
            };
            let Some(descriptor_json) =
                descriptor_blob_json(wallet.as_deref(), local_cube_id, cache.network)
            else {
                ra.error =
                    Some("This Vault's descriptor isn't available on this device.".to_string());
                return Task::none();
            };
            // Unlock the seed with the entered PIN, build the seed blob, then
            // enrol descriptor + seed. The mnemonic is wrapped in `Zeroizing`
            // at the async boundary and never rides a message.
            // `EscrowPin` is already zeroizing-backed; take it out (leaving an
            // empty buffer) and let it drop inside the blocking task, wiping the
            // PIN once the seed blob is built.
            let pin = std::mem::take(&mut ra.pin);
            let seed_cube = match seed_blob_cube(cache, local_cube_id) {
                Ok(c) => c,
                Err(e) => {
                    ra.error = Some(e);
                    return Task::none();
                }
            };
            let network_dir = cache.datadir_path.network_directory(cache.network);
            let datadir = cache.datadir_path.path().to_path_buf();
            let network = cache.network;
            let local_cube_id = local_cube_id.to_string();
            ra.submitting = true;
            ra.awaiting_pin = false;
            ra.error = None;
            Task::perform(
                async move {
                    let seed_json = tokio::task::spawn_blocking(move || {
                        build_seed_blob_json(
                            &network_dir,
                            &datadir,
                            network,
                            &local_cube_id,
                            pin.as_str(),
                            seed_cube,
                        )
                    })
                    .await
                    .map_err(|e| format!("PIN task failed: {}", e))??;
                    enroll_escrow(&client, server_cube_id, descriptor_json, Some(seed_json))
                        .await
                        .map_err(|e| e.to_string())
                },
                move |res| {
                    ra_msg(RecoveryAlertsMessage::ChangeResult(
                        res,
                        session_generation,
                        Some(EscrowTier::FullCube),
                    ))
                },
            )
        }
        RecoveryAlertsMessage::ChangeResult(res, gen, tier_change) => {
            // Drop a change that resolved after the session changed so a stale
            // result can't clobber a newer session's state.
            if gen != session_generation {
                return Task::none();
            }
            ra.submitting = false;
            // Fold the result into state: caches the status on success, or
            // surfaces the error and re-shows the PIN entry on a failed
            // Full-Cube enrol.
            ra.apply_change(res, tier_change);
            Task::none()
        }
    }
}

/// Serialises the descriptor blob JSON for escrow from the live wallet (the
/// same `DescriptorBlob` the Cube Recovery Kit uses, so the heir restore reuses
/// its parsing). `None` when no wallet is loaded on this device.
fn descriptor_blob_json(
    wallet: Option<&Wallet>,
    cube_uuid: &str,
    network: coincube_core::miniscript::bitcoin::Network,
) -> Option<Vec<u8>> {
    let wallet = wallet?;
    let net = settings::network_to_api_string(network);
    let blob = super::recovery_kit::descriptor_blob_from_wallet(wallet, cube_uuid, &net);
    serde_json::to_vec(&blob).ok()
}

/// Builds the cube-metadata half of the seed blob from on-disk settings + the
/// live cache. Read on the main thread before the PIN task so a settings/cube
/// lookup failure surfaces synchronously.
fn seed_blob_cube(cache: &Cache, local_cube_id: &str) -> Result<SeedBlobCube, String> {
    let network_dir = cache.datadir_path.network_directory(cache.network);
    let s = settings::Settings::from_file(&network_dir)
        .map_err(|_| "Failed to read settings file.".to_string())?;
    let cube = s
        .cubes
        .iter()
        .find(|c| c.id == local_cube_id)
        .ok_or_else(|| "Cube not found in settings.".to_string())?;
    let created_at = chrono::DateTime::<chrono::Utc>::from_timestamp(cube.created_at, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    Ok(SeedBlobCube {
        uuid: local_cube_id.to_string(),
        name: cube.name.clone(),
        network: settings::network_to_api_string(cache.network),
        created_at,
        lightning_address: cache.lightning_address.clone(),
    })
}

/// Verifies the PIN, unlocks the mnemonic, and serialises the full seed blob
/// JSON for Full-Cube escrow. Runs on a blocking thread (Argon2 PIN verify +
/// disk read). The mnemonic is wiped (`Zeroizing`) as soon as the phrase string
/// is built, and the serialised JSON — which itself contains the plaintext seed
/// — is returned in a `Zeroizing` buffer so it's wiped once escrow sealing is
/// done rather than lingering on the heap.
fn build_seed_blob_json(
    network_dir: &crate::dir::NetworkDirectory,
    datadir: &std::path::Path,
    network: coincube_core::miniscript::bitcoin::Network,
    local_cube_id: &str,
    pin: &str,
    cube: SeedBlobCube,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let s =
        settings::Settings::from_file(network_dir).map_err(|_| "Failed to read settings file.")?;
    let cube_settings = s
        .cubes
        .iter()
        .find(|c| c.id == local_cube_id)
        .ok_or("Cube not found in settings.")?;
    if !cube_settings.verify_pin(pin) {
        return Err("Incorrect PIN. Please try again.".to_string());
    }
    let fingerprint = cube_settings
        .master_signer_fingerprint
        .ok_or("This Cube has no master signer.")?;
    let words = super::general::load_mnemonic_words(datadir, network, fingerprint, pin)?;
    let phrase = Zeroizing::new(words.join(" "));
    let blob = SeedBlob {
        version: BLOB_VERSION,
        cube,
        mnemonic: SeedBlobMnemonic {
            phrase: phrase.to_string(),
            language: "en".to_string(),
        },
    };
    serde_json::to_vec(&blob)
        .map(Zeroizing::new)
        .map_err(|e| format!("seed blob: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monitoring on with the given escrowed-artifact kinds reported (new API).
    fn monitoring_on_with(artifacts: Option<Vec<&str>>) -> VaultMonitoringStatus {
        VaultMonitoringStatus {
            level: VaultMonitoringLevel::Heartbeat,
            escrowed_artifacts: artifacts.map(|v| v.into_iter().map(String::from).collect()),
            ..VaultMonitoringStatus::default()
        }
    }

    fn with_status(status: VaultMonitoringStatus) -> RecoveryAlerts {
        let mut ra = RecoveryAlerts::new();
        ra.status = Some(status);
        ra
    }

    // ── Server-derived escrow tier: the PR-1 derivation matrix ──────────────

    #[test]
    fn escrow_tier_off_when_monitoring_off() {
        // No status loaded / monitoring off → Nothing escrowed, confident Off.
        assert_eq!(RecoveryAlerts::new().escrow_tier(), Some(EscrowTier::Off));
        assert!(!RecoveryAlerts::new().alerts_on());
    }

    #[test]
    fn escrow_tier_vault_only_from_descriptor_kind() {
        let ra = with_status(monitoring_on_with(Some(vec!["descriptor"])));
        assert_eq!(ra.escrow_tier(), Some(EscrowTier::VaultOnly));
        assert!(ra.alerts_on());
        assert!(ra.has_escrow());
    }

    #[test]
    fn escrow_tier_full_cube_from_seed_kind() {
        let ra = with_status(monitoring_on_with(Some(vec!["descriptor", "seed"])));
        assert_eq!(ra.escrow_tier(), Some(EscrowTier::FullCube));
        assert!(ra.has_escrow());
    }

    #[test]
    fn escrow_tier_nothing_when_alerts_only() {
        // Monitoring on, artifacts present but empty → alerts-only (Off/Nothing).
        let ra = with_status(monitoring_on_with(Some(vec![])));
        assert_eq!(ra.escrow_tier(), Some(EscrowTier::Off));
        assert!(ra.alerts_on());
        assert!(!ra.has_escrow());
    }

    #[test]
    fn escrow_tier_unknown_when_field_absent_and_on() {
        // Old API (no `escrowedArtifacts`) with monitoring on: we must report
        // `None` ("on, tier unknown"), NOT guess — guessing Vault-only would
        // render the false "seed is never escrowed" copy for a maybe-Full vault.
        let ra = with_status(monitoring_on_with(None));
        assert_eq!(ra.escrow_tier(), None);
        assert!(ra.alerts_on());
    }

    #[test]
    fn pin_is_redacted_in_debug_output() {
        // Canary PIN unlikely to collide with any other field's Debug.
        let mut ra = RecoveryAlerts::new();
        ra.pin = EscrowPin::from("9173".to_string());

        // The wrapper redacts itself...
        assert_eq!(format!("{:?}", ra.pin), "EscrowPin(<redacted>)");
        // ...so the struct's derived Debug can't leak the PIN.
        let dump = format!("{:?}", ra);
        assert!(!dump.contains("9173"), "PIN leaked in Debug: {}", dump);
        assert!(dump.contains("<redacted>"));
    }

    #[test]
    fn pin_clear_empties_the_buffer() {
        let mut pin = EscrowPin::from("1234".to_string());
        assert_eq!(pin.as_str(), "1234");
        pin.clear();
        assert_eq!(pin.as_str(), "");
    }

    #[test]
    fn enroll_caches_returned_status_and_derives_tier() {
        // A (re-)enrol returns a status whose `escrowed_artifacts` the owner op
        // stamped; caching it derives the confirmed tier immediately, no session
        // state. Here a Full-Cube enrol returns descriptor+seed.
        let mut ra = with_status(monitoring_on_with(Some(vec![])));
        let returned = monitoring_on_with(Some(vec!["descriptor", "seed"]));
        ra.apply_change(Ok(returned), Some(EscrowTier::FullCube));
        assert_eq!(ra.escrow_tier(), Some(EscrowTier::FullCube));
        assert!(ra.alerts_on());
    }

    #[test]
    fn confirm_change_closes_alerts_off_dialog() {
        // An Ok change result must also dismiss the alerts-off confirm dialog.
        let mut ra = RecoveryAlerts::new();
        ra.confirming_alerts_off = true;
        ra.apply_change(
            Ok(VaultMonitoringStatus {
                level: VaultMonitoringLevel::Off,
                escrowed_artifacts: Some(Vec::new()),
                ..VaultMonitoringStatus::default()
            }),
            Some(EscrowTier::Off),
        );
        assert!(!ra.confirming_alerts_off);
        assert!(!ra.alerts_on());
    }

    #[test]
    fn failed_full_cube_enrol_reshows_pin_entry() {
        // ConfirmFullCube clears `awaiting_pin` before the blocking PIN verify;
        // a wrong PIN surfaces as an Err here and must re-show the PIN entry for
        // an inline retry (not force the owner to re-pick Full Cube).
        let mut ra = RecoveryAlerts::new();
        ra.awaiting_pin = false;
        ra.apply_change(
            Err("Incorrect PIN. Please try again.".to_string()),
            Some(EscrowTier::FullCube),
        );
        assert!(
            ra.awaiting_pin,
            "a failed Full-Cube enrol must re-show the PIN entry"
        );
        assert_eq!(
            ra.error.as_deref(),
            Some("Incorrect PIN. Please try again.")
        );
    }

    #[test]
    fn failed_non_full_cube_change_leaves_pin_hidden() {
        // Off / Vault-only have no PIN entry, so a failure must not flip
        // `awaiting_pin` on.
        let mut ra = RecoveryAlerts::new();
        ra.awaiting_pin = false;
        ra.apply_change(Err("network".to_string()), Some(EscrowTier::VaultOnly));
        assert!(!ra.awaiting_pin);
        assert_eq!(ra.error.as_deref(), Some("network"));
    }

    #[test]
    fn reload_reflects_server_reported_tier() {
        // A device that just enrolled Full-Cube... then another device downgrades
        // to Vault-only. A reload reports `["descriptor"]`, and the card derives
        // Vault-only straight from the server — no stale Full-Cube assertion.
        let mut ra = with_status(monitoring_on_with(Some(vec!["descriptor", "seed"])));
        assert_eq!(ra.escrow_tier(), Some(EscrowTier::FullCube));
        ra.apply_status_loaded(7, monitoring_on_with(Some(vec!["descriptor"])));
        assert_eq!(ra.escrow_tier(), Some(EscrowTier::VaultOnly));
        assert_eq!(ra.vault_id, Some(7));
        assert!(!ra.no_vault);
    }

    #[test]
    fn reload_against_old_api_reports_unknown_not_a_wrong_tier() {
        // An older API omits `escrowedArtifacts`. A reload of an *on* vault must
        // report `None` ("on, tier unknown") — the fallback banner — rather than
        // asserting whatever this device last enrolled.
        let mut ra = with_status(monitoring_on_with(Some(vec!["descriptor", "seed"])));
        ra.apply_status_loaded(7, monitoring_on_with(None));
        assert_eq!(ra.escrow_tier(), None);
    }

    #[test]
    fn reload_reports_off_when_monitoring_off() {
        let mut ra = with_status(monitoring_on_with(Some(vec!["descriptor", "seed"])));
        ra.apply_status_loaded(
            7,
            VaultMonitoringStatus {
                level: VaultMonitoringLevel::Off,
                ..VaultMonitoringStatus::default()
            },
        );
        assert_eq!(ra.escrow_tier(), Some(EscrowTier::Off));
        assert!(!ra.alerts_on());
    }
}
