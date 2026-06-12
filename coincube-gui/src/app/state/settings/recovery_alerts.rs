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

use crate::{
    app::{message::Message, view, wallet::Wallet},
    services::coincube::{
        CoincubeClient, CubeMember, KeyholderDownloadPolicy, SetVaultMonitoringRequest,
        VaultMonitoringLevel, VaultMonitoringStatus,
    },
};

use view::{RecoveryAlertsMessage, SettingsMessage};

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
}

/// Wrap a [`RecoveryAlertsMessage`] in the settings-message envelope.
fn ra_msg(m: RecoveryAlertsMessage) -> Message {
    Message::View(view::Message::Settings(SettingsMessage::RecoveryAlerts(m)))
}

/// App-level dispatcher. `client` / `server_cube_id` / `wallet` / `members`
/// are injected by `App::update`; `entitled` is the account's
/// `recovery_alerts` entitlement.
#[allow(clippy::too_many_arguments)]
pub fn update(
    ra: &mut RecoveryAlerts,
    msg: RecoveryAlertsMessage,
    client: Option<CoincubeClient>,
    server_cube_id: Option<u64>,
    wallet: Option<Arc<Wallet>>,
    entitled: bool,
    members: &[CubeMember],
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
                ra.no_vault = server_cube_id.is_none();
                ra.loaded_once = true;
                return Task::none();
            };
            if !entitled {
                // Non-Estate: don't hit the network; the card shows the
                // locked affordance.
                ra.loaded_once = true;
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
                |res| ra_msg(RecoveryAlertsMessage::StatusLoaded(res)),
            )
        }
        RecoveryAlertsMessage::StatusLoaded(res) => {
            ra.loading = false;
            ra.loaded_once = true;
            match res {
                Ok((vid, status)) => {
                    ra.vault_id = Some(vid);
                    ra.status = Some(status);
                    ra.no_vault = false;
                    ra.error = None;
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
        RecoveryAlertsMessage::SelectLevel(level) => {
            if !entitled {
                ra.error = Some("Recovery alerts require an Estate plan.".to_string());
                return Task::none();
            }
            if ra.level() == level {
                return Task::none();
            }
            let (Some(client), Some(vault_id)) = (client, ra.vault_id) else {
                ra.error = Some(
                    "Couldn't find this Vault on Connect yet — try again in a moment.".to_string(),
                );
                return Task::none();
            };
            let current_policy = ra.status.as_ref().map(|s| s.crk_keyholder_download);
            ra.submitting = true;
            ra.error = None;
            match level {
                VaultMonitoringLevel::Off => Task::perform(
                    async move {
                        client
                            .delete_vault_monitoring(vault_id)
                            .await
                            .map(|_| VaultMonitoringStatus {
                                level: VaultMonitoringLevel::Off,
                                crk_keyholder_download: current_policy.unwrap_or_default(),
                                last_notified_state: None,
                                updated_at: None,
                            })
                            .map_err(|e| e.to_string())
                    },
                    |res| ra_msg(RecoveryAlertsMessage::ChangeResult(res)),
                ),
                VaultMonitoringLevel::Heartbeat => {
                    let req = SetVaultMonitoringRequest {
                        level,
                        descriptor: None,
                        gap_limit: None,
                        crk_keyholder_download: current_policy,
                    };
                    Task::perform(
                        async move {
                            client
                                .set_vault_monitoring(vault_id, req)
                                .await
                                .map_err(|e| e.to_string())
                        },
                        |res| ra_msg(RecoveryAlertsMessage::ChangeResult(res)),
                    )
                }
                VaultMonitoringLevel::Full => {
                    // Full needs the descriptor to escrow. It only exists on
                    // a device holding the live wallet.
                    let Some(descriptor) = wallet.as_ref().map(|w| w.main_descriptor.to_string())
                    else {
                        ra.submitting = false;
                        ra.error = Some(
                            "This Vault's descriptor isn't available on this device, so full \
                             monitoring can't be enabled here."
                                .to_string(),
                        );
                        return Task::none();
                    };
                    let req = SetVaultMonitoringRequest {
                        level,
                        descriptor: Some(descriptor),
                        gap_limit: None,
                        crk_keyholder_download: current_policy,
                    };
                    Task::perform(
                        async move {
                            client
                                .set_vault_monitoring(vault_id, req)
                                .await
                                .map_err(|e| e.to_string())
                        },
                        |res| ra_msg(RecoveryAlertsMessage::ChangeResult(res)),
                    )
                }
            }
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
                |res| ra_msg(RecoveryAlertsMessage::ChangeResult(res)),
            )
        }
        RecoveryAlertsMessage::ChangeResult(res) => {
            ra.submitting = false;
            match res {
                Ok(status) => {
                    ra.status = Some(status);
                    ra.error = None;
                }
                Err(e) => ra.error = Some(e),
            }
            Task::none()
        }
    }
}
