//! Vault Recovery Alerts settings card (Estate Notifications — PR 2).
//!
//! Estate-only, per-vault opt-in for recovery-path monitoring. Modeled on
//! the Cube Recovery Kit card (`recovery_kit.rs`): the state container lives
//! on the outer `SettingsState` so `App::update` can inject the
//! authenticated `CoincubeClient`, the Connect cube id, the live wallet
//! descriptor, the keyholder list, and the entitlement — none of which are
//! plumbed through `State::update`.
//!
//! Three tiers (`VaultMonitoringLevel`): **Off** (true-delete any escrowed
//! descriptor), **Alerts only / Heartbeat** (timelock heartbeat only — the
//! server never sees addresses/balances), **Full** (a service-encrypted
//! copy of the descriptor is escrowed so keyholders can recover without the
//! owner's password). A separate keyholder download policy governs when
//! keyholders may pull the encrypted recovery kit.

use std::sync::Arc;

use iced::Task;
use zeroize::Zeroizing;

use crate::{
    app::{cache::Cache, message::Message, settings, view, wallet::Wallet},
    services::coincube::{
        CoincubeClient, CubeMember, KeyholderDownloadPolicy, VaultMonitoringLevel,
        VaultMonitoringStatus,
    },
    services::inheritance::{disable_escrow, enroll_escrow, EscrowTier},
    services::recovery::{SeedBlob, SeedBlobCube, SeedBlobMnemonic, BLOB_VERSION},
};

use view::{EscrowPin, RecoveryAlertsMessage, SettingsMessage};

/// Settings-card state for Vault Recovery Alerts. Held on `SettingsState`.
#[derive(Debug)]
pub struct RecoveryAlerts {
    /// Connect vault id, resolved from the cube on first load and cached so
    /// level/policy changes don't re-resolve it.
    pub vault_id: Option<u64>,
    /// Last-known monitoring status from Connect. `None` until first load.
    pub status: Option<VaultMonitoringStatus>,
    pub loading: bool,
    pub submitting: bool,
    pub error: Option<String>,
    /// Keyholder emails (the cube members who'd be notified), snapshotted on
    /// each load so the card can show exactly who would receive alerts.
    pub keyholders: Vec<String>,
    /// Whether the account carries the Estate `recovery_alerts` entitlement.
    pub entitled: bool,
    /// True once a load resolved that there's no Connect vault to monitor
    /// (no cube registered / no vault created yet).
    pub no_vault: bool,
    /// True once at least one load has been attempted — lets the card pick
    /// the right loading vs. empty copy.
    pub loaded_once: bool,
    /// The escrow tier this session tracked from an enrol/disable we performed.
    /// The owner monitoring status reports on/off, not which tier, so this is
    /// the only tier signal we have — and it resets to `Off` on restart. `Off`
    /// therefore means *either* escrow is off *or* it's on but untracked on this
    /// device; use [`Self::tier`] (the method), which combines this with
    /// [`Self::level`], to disambiguate (it returns `None` for the latter).
    pub tier: EscrowTier,
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
            no_vault: false,
            loaded_once: false,
            tier: EscrowTier::Off,
            awaiting_pin: false,
            pin: EscrowPin::default(),
        }
    }

    /// Current tier for the view and the no-op guards:
    /// - `Some(Off)` when monitoring is off;
    /// - `Some(tier)` when this session tracked the enrolled tier;
    /// - `None` when escrow is on but this device doesn't know which tier —
    ///   e.g. after a restart, since the owner monitoring status doesn't report
    ///   it. We surface "on, tier unknown" rather than guess: guessing
    ///   Vault-only for a Full-Cube vault would print a false claim (its copy
    ///   says "the seed is never escrowed") for a seed that *is* escrowed.
    pub fn tier(&self) -> Option<EscrowTier> {
        if matches!(self.level(), VaultMonitoringLevel::Off) {
            Some(EscrowTier::Off)
        } else if self.tier == EscrowTier::Off {
            None
        } else {
            Some(self.tier)
        }
    }

    /// Current monitoring level (Off when unloaded).
    pub fn level(&self) -> VaultMonitoringLevel {
        self.status
            .as_ref()
            .map(|s| s.level)
            .unwrap_or(VaultMonitoringLevel::Off)
    }

    /// Current keyholder download policy (privacy-preserving default when
    /// unloaded).
    pub fn download_policy(&self) -> KeyholderDownloadPolicy {
        self.status
            .as_ref()
            .map(|s| s.crk_keyholder_download)
            .unwrap_or_default()
    }

    /// Fold a change result into state.
    ///
    /// **Ok:** apply the confirmed `tier_change` (if any) and cache `status`. On
    /// a **disable** (`Some(Off)`), `status` is the synthetic Off status
    /// [`disable_escrow`](crate::services::inheritance::disable_escrow) returns,
    /// which carries the *default* download policy — so we carry the owner's
    /// prior keyholder download choice forward instead, keeping it for a later
    /// re-enrol (the policy is an independent vault setting, not reset by turning
    /// escrow off).
    ///
    /// **Err:** surface the (display-safe) error. A failed **Full-Cube** enrol
    /// also restores `awaiting_pin`: `ConfirmFullCube` clears it before the
    /// blocking verify, so a wrong PIN would otherwise hide the PIN entry and
    /// force the owner to re-pick Full Cube. Restoring it re-shows the (now
    /// empty — the buffer was taken) PIN field alongside the error for an inline
    /// retry.
    fn apply_change(
        &mut self,
        res: Result<VaultMonitoringStatus, String>,
        tier_change: Option<EscrowTier>,
    ) {
        match res {
            Ok(mut status) => {
                if let Some(t) = tier_change {
                    self.tier = t;
                }
                if matches!(tier_change, Some(EscrowTier::Off)) {
                    if let Some(prev) = self.status.as_ref() {
                        status.crk_keyholder_download = prev.crk_keyholder_download;
                    }
                }
                self.status = Some(status);
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e);
                if matches!(tier_change, Some(EscrowTier::FullCube)) {
                    self.awaiting_pin = true;
                }
            }
        }
    }

    /// Fold a successful `LoadStatus` into state.
    ///
    /// The monitoring status reports on/off (`level`) — **not** which escrow
    /// tier. So we reset our session-tracked tier on every (re)load: the status
    /// can't confirm it, and another device may have changed it (e.g. upgraded
    /// Vault-only → Full-Cube) while monitoring stayed on. [`Self::tier`] then
    /// reports an on vault as `None` ("on, tier unknown") until *this* device
    /// performs an enrol/disable, so the card never re-asserts a stale tier (and
    /// its false seed-escrow copy) it can't actually confirm.
    fn apply_status_loaded(&mut self, vault_id: u64, status: VaultMonitoringStatus) {
        self.vault_id = Some(vault_id);
        self.status = Some(status);
        self.no_vault = false;
        self.error = None;
        self.tier = EscrowTier::Off;
    }
}

/// Wrap a [`RecoveryAlertsMessage`] in the settings-message envelope.
fn ra_msg(m: RecoveryAlertsMessage) -> Message {
    Message::View(view::Message::Settings(SettingsMessage::RecoveryAlerts(m)))
}

/// App-level dispatcher. `client` / `server_cube_id` / `wallet` / `members`
/// are injected by `App::update`; `entitled` is the account's
/// `recovery_alerts` entitlement. `session_generation` is the connect
/// account's current session counter (bumped on login / logout / reset):
/// it's stamped into spawned async results so a load / change that lands
/// after the session changed is dropped instead of writing a prior account's
/// vault id + status into the reset state — the same guard the duress-contacts
/// handlers use.
#[allow(clippy::too_many_arguments)]
pub fn update(
    ra: &mut RecoveryAlerts,
    msg: RecoveryAlertsMessage,
    client: Option<CoincubeClient>,
    server_cube_id: Option<u64>,
    wallet: Option<Arc<Wallet>>,
    entitled: bool,
    members: &[CubeMember],
    session_generation: u64,
    cache: &Cache,
    local_cube_id: &str,
) -> Task<Message> {
    ra.entitled = entitled;
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
                        .get_vault_monitoring(vault.id)
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
                    // Cache the freshly-loaded status and reset our tracked tier
                    // (the status confirms on/off only, not the tier — so we
                    // never re-assert a tier we can't confirm). See the method.
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
        RecoveryAlertsMessage::SelectTier(tier) => {
            ra.awaiting_pin = false;
            ra.pin.clear();
            if !entitled {
                ra.error = Some("Recovery alerts require an Estate plan.".to_string());
                return Task::none();
            }
            // Skip a no-op re-select of the *known* current tier. When the tier
            // is unknown on this device (`None`), any selection proceeds so the
            // owner can confirm/change it.
            if ra.tier() == Some(tier) {
                return Task::none();
            }
            let (Some(client), Some(vault_id), Some(server_cube_id)) =
                (client, ra.vault_id, server_cube_id)
            else {
                ra.error = Some(
                    "Couldn't find this Vault on Connect yet — try again in a moment.".to_string(),
                );
                return Task::none();
            };
            match tier {
                EscrowTier::Off => {
                    ra.submitting = true;
                    ra.error = None;
                    Task::perform(
                        async move {
                            disable_escrow(&client, server_cube_id, vault_id)
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
                    // descriptor blob from the live wallet and enrol.
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
                    // Preserve the vault's current download policy across the
                    // enrol (omitting it would reset to the server default).
                    let download_policy = ra.download_policy();
                    Task::perform(
                        async move {
                            enroll_escrow(
                                &client,
                                server_cube_id,
                                descriptor_json,
                                None,
                                download_policy,
                            )
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
            if !entitled || !ra.awaiting_pin {
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
            // Preserve the vault's current download policy across the enrol
            // (omitting it would reset to the server default).
            let download_policy = ra.download_policy();
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
                    enroll_escrow(
                        &client,
                        server_cube_id,
                        descriptor_json,
                        Some(seed_json),
                        download_policy,
                    )
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
        RecoveryAlertsMessage::SetDownloadPolicy(policy) => {
            if !entitled {
                return Task::none();
            }
            if ra.download_policy() == policy {
                return Task::none();
            }
            let (Some(client), Some(vault_id)) = (client, ra.vault_id) else {
                ra.error = Some(
                    "Couldn't find this Vault on Connect yet — try again in a moment.".to_string(),
                );
                return Task::none();
            };
            ra.submitting = true;
            ra.error = None;
            Task::perform(
                async move {
                    client
                        .set_keyholder_download_policy(vault_id, policy)
                        .await
                        .map_err(|e| e.to_string())
                },
                // A policy save carries no tier change (`None`) so its result
                // never touches the tracked escrow tier — even if it resolves
                // while a tier change is still in flight.
                move |res| {
                    ra_msg(RecoveryAlertsMessage::ChangeResult(
                        res,
                        session_generation,
                        None,
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
            // Fold the result into state: caches the status (preserving the
            // download policy across a disable) on success, or surfaces the
            // error and re-shows the PIN entry on a failed Full-Cube enrol.
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

    fn monitoring_on() -> VaultMonitoringStatus {
        VaultMonitoringStatus {
            level: VaultMonitoringLevel::Heartbeat,
            ..VaultMonitoringStatus::default()
        }
    }

    #[test]
    fn tier_is_off_when_monitoring_off() {
        // No status loaded yet → level Off → tier is a confirmed Off.
        assert_eq!(RecoveryAlerts::new().tier(), Some(EscrowTier::Off));
    }

    #[test]
    fn tier_is_unknown_when_on_but_untracked() {
        // Monitoring on but `tier` still at its default (e.g. fresh after a
        // restart): we must report `None` ("on, tier unknown"), NOT guess
        // Vault-only — guessing would render the false "the seed is never
        // escrowed" copy for a vault that may be Full-Cube.
        let mut ra = RecoveryAlerts::new();
        ra.status = Some(monitoring_on());
        assert_eq!(ra.tier(), None);
    }

    #[test]
    fn tier_is_reported_when_on_and_tracked() {
        // A tier we applied this session is reported as-is while monitoring is on.
        let mut ra = RecoveryAlerts::new();
        ra.status = Some(monitoring_on());
        ra.tier = EscrowTier::FullCube;
        assert_eq!(ra.tier(), Some(EscrowTier::FullCube));
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
    fn disable_preserves_prior_download_policy() {
        // Owner had escrow on with a non-default (Anytime) download policy.
        let mut ra = RecoveryAlerts::new();
        ra.status = Some(VaultMonitoringStatus {
            level: VaultMonitoringLevel::Heartbeat,
            crk_keyholder_download: KeyholderDownloadPolicy::Anytime,
            ..VaultMonitoringStatus::default()
        });
        ra.tier = EscrowTier::VaultOnly;

        // disable_escrow returns a synthetic Off status carrying the *default*
        // (AtApproaching) policy.
        let synthetic_off = VaultMonitoringStatus {
            level: VaultMonitoringLevel::Off,
            ..VaultMonitoringStatus::default()
        };
        ra.apply_change(Ok(synthetic_off), Some(EscrowTier::Off));

        // Tier goes Off, but the owner's Anytime choice must survive so a later
        // re-enrol forwards it — not silently revert to the default.
        assert_eq!(ra.tier, EscrowTier::Off);
        assert_eq!(ra.download_policy(), KeyholderDownloadPolicy::Anytime);
    }

    #[test]
    fn enroll_caches_returned_download_policy_verbatim() {
        // A (re-)enrol returns the real monitoring status; it is cached as-is
        // (no disable-preservation override for non-Off changes).
        let mut ra = RecoveryAlerts::new();
        ra.status = Some(VaultMonitoringStatus {
            level: VaultMonitoringLevel::Heartbeat,
            crk_keyholder_download: KeyholderDownloadPolicy::Anytime,
            ..VaultMonitoringStatus::default()
        });
        let returned = VaultMonitoringStatus {
            level: VaultMonitoringLevel::Heartbeat,
            crk_keyholder_download: KeyholderDownloadPolicy::AtApproaching,
            ..VaultMonitoringStatus::default()
        };
        ra.apply_change(Ok(returned), Some(EscrowTier::FullCube));
        assert_eq!(ra.tier, EscrowTier::FullCube);
        assert_eq!(ra.download_policy(), KeyholderDownloadPolicy::AtApproaching);
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
    fn reload_clears_stale_session_tier_for_on_vault() {
        // This device thinks it's Full-Cube from an earlier enrol...
        let mut ra = RecoveryAlerts::new();
        ra.tier = EscrowTier::FullCube;
        // ...but a reload only confirms monitoring is *on* (level), not the tier
        // — another device may have changed it. The card must NOT keep asserting
        // Full-Cube ("seed is escrowed"); it must report "on, tier unknown".
        ra.apply_status_loaded(
            7,
            VaultMonitoringStatus {
                level: VaultMonitoringLevel::Heartbeat,
                ..VaultMonitoringStatus::default()
            },
        );
        assert_eq!(ra.tier(), None);
        assert_eq!(ra.vault_id, Some(7));
        assert!(!ra.no_vault);
    }

    #[test]
    fn reload_reports_off_when_monitoring_off() {
        // A stale tracked tier must collapse to Off when the load says off.
        let mut ra = RecoveryAlerts::new();
        ra.tier = EscrowTier::FullCube;
        ra.apply_status_loaded(
            7,
            VaultMonitoringStatus {
                level: VaultMonitoringLevel::Off,
                ..VaultMonitoringStatus::default()
            },
        );
        assert_eq!(ra.tier(), Some(EscrowTier::Off));
    }
}
