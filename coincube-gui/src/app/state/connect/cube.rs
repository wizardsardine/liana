use std::collections::HashMap;
use std::sync::Arc;

use coincube_spark_protocol::LightningAddressInfo;
use iced::task::Handle as TaskHandle;

use crate::{
    app::{
        breez_spark::SparkClient,
        message::Message,
        view::{self, ConnectCubeMessage},
    },
    services::coincube::{
        AvatarGenerateRequest, AvatarSelectRequest, AvatarUserTraits, CoincubeClient,
        LightningAddress, PatchConnectVaultRequest, RegisterCubeRequest, UpdateCubeRequest,
    },
};

/// Phase 4g claim-flow rollback helper.
///
/// Tears down whichever of (SDK registration, API reservation)
/// actually succeeded, logging each delete failure with cube id +
/// username so manual cleanup has enough context. Returns the
/// suffix to splice onto the user-facing error: `""` on clean
/// rollback, the bracketed "partial rollback failure" note when
/// at least one delete call errored.
///
/// Pass `spark = None` at rollback sites where the SDK register
/// hadn't succeeded yet (i.e. rolling back only the API
/// reservation).
async fn rollback_partial_claim(
    client: &CoincubeClient,
    spark: Option<&SparkClient>,
    cube_id: &str,
    username: &str,
) -> &'static str {
    let mut partial = false;
    if let Some(spark) = spark {
        if let Err(e) = spark.delete_lightning_address().await {
            log::error!(
                "[CONNECT-CUBE] rollback of Spark lightning-address registration \
                 failed (cube={}, username={}): {}",
                cube_id,
                username,
                e
            );
            partial = true;
        }
    }
    if let Err(e) = client.delete_lightning_address(cube_id).await {
        log::error!(
            "[CONNECT-CUBE] rollback of API lightning-address reservation failed \
             (cube={}, username={}): {}",
            cube_id,
            username,
            e
        );
        partial = true;
    }
    if partial {
        " (partial rollback failure — please contact support)"
    } else {
        ""
    }
}

/// Outcome of [`ConnectCubePanel::reconcile_spark_lightning_address`].
///
/// Splits "SDK already in sync / we fixed it" from the two error
/// shapes so the UI can react differently: a query failure is
/// transient and retryable on the next trigger, while
/// [`ReconcileOutcome::NeedsReRegistration`] is a persistent
/// API↔SDK divergence that needs the user's attention (the claim
/// record is in our DB but the Breez LNURL server doesn't know
/// about this device).
#[derive(Debug, Clone)]
pub enum ReconcileOutcome {
    /// SDK's local cache already holds the expected record.
    AlreadyBound(LightningAddressInfo),
    /// SDK cache was empty; we re-registered the DB-confirmed
    /// username on this device.
    ReRegistered(LightningAddressInfo),
    /// Querying the SDK failed. Transient — the next
    /// `LightningAddressChanged { info: None }` event or the next
    /// cube-registered reload will retry.
    QueryFailed(String),
    /// SDK had no record and the register call failed. API and SDK
    /// are now out of sync; surface this so the user can re-claim
    /// from settings.
    NeedsReRegistration(String),
}

/// Terminal outcome of the username-change chain (PUT + SDK delete +
/// SDK register). Carried in
/// [`crate::app::view::ConnectCubeMessage::LightningAddressUpdated`].
#[derive(Debug, Clone)]
pub enum LightningAddressChangeOutcome {
    /// Server committed and the SDK rebound to the new name.
    Ok(LightningAddress),
    /// Server rejected the request (409 conflict / 422 invalid /
    /// 5xx). The cube's existing address is unchanged everywhere.
    ServerError(String),
    /// Server committed the rename but the SDK rebind failed
    /// (delete or register errored). The DB-confirmed address is
    /// the new one; the SDK is either still bound to the old name
    /// (delete failed) or empty (register failed). The
    /// re-registration prompt surfaces in either case.
    SdkSyncFailed {
        addr: LightningAddress,
        message: String,
    },
}

use super::{cube_members, AvatarFlowStep, ConnectCubeMembersState};

/// Per-Cube Connect panel handling Lightning Address and Avatar.
/// The Lightning Address claim flow is fulfilled by the Cube's
/// Spark wallet via Breez-hosted LNURL.
pub struct ConnectCubePanel {
    /// The Cube's client-side UUID (from CubeSettings.id)
    pub cube_uuid: String,
    /// The Cube's display name (for registration)
    pub cube_name: String,
    /// The Cube's network ("mainnet" or "testnet")
    pub cube_network: String,
    /// Whether this Cube currently has a Vault wallet
    /// (`vault_wallet_id.is_some()`). Reported to the server on
    /// registration so other devices can evaluate the duress vault gate
    /// (PLAN-duress-vault-gate PR 3). Flipped to `true` in-session when a
    /// Vault is created (see `App::WalletUpdated`).
    pub cube_has_vault: bool,
    /// This Cube's Connect-blinding encryption **public** key (33-byte
    /// compressed secp256k1, lowercase hex), read from
    /// `CubeSettings::connect_encryption_pubkey`. `None` for a Cube whose seed
    /// hasn't been unlocked on a build that derives it (e.g. a passkey Cube) —
    /// registration then simply doesn't run, and Contacts can't enrol enveloped
    /// keys against this Cube until it does.
    pub cube_encryption_pubkey: Option<String>,
    /// True once this session has published [`Self::cube_encryption_pubkey`] to
    /// Connect. The endpoint is idempotent, so this only avoids a redundant
    /// round-trip per launch — correctness doesn't depend on it.
    pub(super) enc_pubkey_registered: bool,
    /// This Cube's Vault descriptor fingerprint (8 lowercase hex), read from
    /// `CubeSettings::vault_fingerprint`. `None` when this device holds no
    /// Vault for the Cube, or holds one whose fingerprint hasn't been computed
    /// yet — either way there is nothing to assert.
    pub vault_fingerprint: Option<String>,
    /// True once this session has asserted [`Self::vault_fingerprint`] to
    /// Connect. The endpoint no-ops an unchanged value, so this only avoids a
    /// redundant round-trip per launch — correctness doesn't depend on it.
    pub(super) vault_fingerprint_asserted: bool,
    /// The server-side numeric ID — set after registering with the backend.
    /// Used in API paths: /connect/cubes/{server_cube_id}/...
    pub server_cube_id: Option<u64>,
    /// Set when the last cube registration attempt failed.
    pub registration_error: Option<String>,
    // Lightning Address
    pub lightning_address: Option<LightningAddress>,
    pub ln_username_input: String,
    pub ln_username_available: Option<bool>,
    pub ln_username_error: Option<String>,
    pub ln_claim_error: Option<String>,
    pub ln_claiming: bool,
    pub ln_checking: bool,
    /// True when the claimed-address card is rendered in its in-place
    /// edit form. The unclaimed flow leaves this `false`.
    pub ln_editing: bool,
    /// `Some(new_username)` while the destructive-action confirmation
    /// modal is showing for a proposed username change.
    pub ln_change_confirm_pending: Option<String>,
    /// True during the PUT + SDK delete + SDK register chain. Distinct
    /// from `ln_claiming` so view code can spinner-gate the
    /// edit form independently of the unclaimed-flow claim button.
    pub ln_changing: bool,
    /// True while a manual retry of the SDK rebind (delete + register
    /// against the DB-confirmed username) is in flight, kicked off
    /// from the re-registration prompt on the claimed-address card.
    pub ln_reregistering: bool,
    ln_check_version: u32,
    ln_check_abort: Option<TaskHandle>,
    /// Spark subprocess client. Phase 4g routes the Lightning
    /// Address claim flow through `register_lightning_address` on
    /// the Breez-hosted LNURL server; the API's own reserve/confirm
    /// endpoints bracket the SDK call. `None` for cubes without a
    /// Spark signer (those cubes can't claim Lightning Addresses
    /// under the new flow — the UI hides the claim button in that
    /// case).
    spark_client: Option<Arc<SparkClient>>,
    /// Persistent divergence-between-API-and-SDK signal, populated
    /// when `reconcile_spark_lightning_address` couldn't bind the
    /// DB-confirmed username on this device. Displayed to the user
    /// as "Lightning address needs re-registration" so they can
    /// act on it — the reconcile task can't retry the Breez server
    /// forever on its own. Cleared whenever the SDK next reports a
    /// bound address.
    pub ln_reconcile_needs_reregister: Option<String>,
    /// API client with JWT set — obtained from ConnectAccountPanel after login.
    pub client: Option<CoincubeClient>,
    // Avatar
    pub avatar_step: AvatarFlowStep,
    pub avatar_data: Option<crate::services::coincube::GetAvatarData>,
    pub avatar_generating: bool,
    pub avatar_error: Option<String>,
    pub avatar_image_cache: HashMap<u64, (Vec<u8>, iced::widget::image::Handle)>,
    pub avatar_draft: AvatarUserTraits,
    // Members (W8)
    pub members: ConnectCubeMembersState,
}

impl ConnectCubePanel {
    pub fn new(
        spark_client: Option<Arc<SparkClient>>,
        cube_uuid: String,
        cube_name: String,
        cube_network: String,
        cube_has_vault: bool,
    ) -> Self {
        ConnectCubePanel {
            cube_uuid,
            cube_name,
            cube_network,
            cube_has_vault,
            cube_encryption_pubkey: None,
            enc_pubkey_registered: false,
            vault_fingerprint: None,
            vault_fingerprint_asserted: false,
            server_cube_id: None,
            registration_error: None,
            lightning_address: None,
            ln_username_input: String::new(),
            ln_username_available: None,
            ln_username_error: None,
            ln_claim_error: None,
            ln_claiming: false,
            ln_checking: false,
            ln_editing: false,
            ln_change_confirm_pending: None,
            ln_changing: false,
            ln_reregistering: false,
            ln_check_version: 0,
            ln_check_abort: None,
            spark_client,
            ln_reconcile_needs_reregister: None,
            client: None,
            avatar_step: AvatarFlowStep::Idle,
            avatar_data: None,
            avatar_generating: false,
            avatar_error: None,
            avatar_image_cache: HashMap::new(),
            avatar_draft: AvatarUserTraits::default(),
            members: ConnectCubeMembersState::new(),
        }
    }

    /// Set the authenticated API client (called after account login).
    pub fn set_client(&mut self, client: CoincubeClient) {
        self.client = Some(client);
    }

    /// Returns a task to load avatar if conditions are met (client available, cube registered, not already loaded).
    pub fn load_avatar_if_needed(&self) -> Option<iced::Task<Message>> {
        if self.client.is_some() && self.server_cube_id.is_some() && self.avatar_data.is_none() {
            let client = self.client.clone().unwrap();
            let cid = self.api_cube_id().unwrap();
            return Some(iced::Task::perform(
                async move { client.get_avatar(&cid).await },
                |res| {
                    Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                        crate::app::view::AvatarMessage::Loaded(res.map_err(|e| e.to_string())),
                    )))
                },
            ));
        }
        None
    }

    /// Returns the active avatar image handle for the sidebar, if available.
    pub fn get_active_avatar_handle(&self) -> Option<iced::widget::image::Handle> {
        self.avatar_data.as_ref().and_then(|d| {
            let url = d.active_avatar_url.as_deref().unwrap_or("");
            // Extract the last path segment for exact ID matching
            // (avoids false matches like ".../112" matching ID "12")
            let active_id = url.rsplit('/').next()?.split('.').next()?;
            d.variants
                .iter()
                .find(|v| v.id.to_string() == active_id)
                .and_then(|v| self.avatar_image_cache.get(&v.id))
                .map(|(_, handle)| handle.clone())
        })
    }

    /// Clear the API client and all session-scoped state (called on account logout).
    pub fn clear_client(&mut self) {
        self.client = None;
        self.server_cube_id = None;
        self.registration_error = None;
        self.lightning_address = None;
        self.ln_username_input.clear();
        self.ln_username_available = None;
        self.ln_username_error = None;
        self.ln_claim_error = None;
        self.ln_claiming = false;
        self.ln_checking = false;
        self.ln_editing = false;
        self.ln_change_confirm_pending = None;
        self.ln_changing = false;
        self.ln_reregistering = false;
        self.ln_check_version += 1;
        if let Some(handle) = self.ln_check_abort.take() {
            handle.abort();
        }
        self.ln_reconcile_needs_reregister = None;
        self.avatar_step = AvatarFlowStep::Idle;
        self.avatar_data = None;
        self.avatar_generating = false;
        self.avatar_error = None;
        self.avatar_image_cache.clear();
        self.avatar_draft = AvatarUserTraits::default();
        self.members.clear();
    }

    /// Returns the server-side cube ID as a string for API paths.
    fn api_cube_id(&self) -> Option<String> {
        self.server_cube_id.map(|id| id.to_string())
    }

    /// Phase 4g: reconcile the Spark SDK's Lightning Address state
    /// against our DB-reserved record.
    ///
    /// Fires `get_lightning_address()` on the Spark bridge. The
    /// matching handler in [`Self::update_message`] auto-re-registers
    /// the DB-confirmed username when the SDK reports `None` (device
    /// reinstall, SDK storage wipe, multi-device identity swap).
    ///
    /// Returns `None` when there's nothing to do — no Spark backend,
    /// or no DB-confirmed address. A DB `lightning_address` that
    /// can't be split on `@` is logged as a malformed record (the
    /// API shouldn't persist these) and skipped — reconcile can't
    /// do anything with a row the user would have to clean up
    /// manually anyway.
    pub fn reconcile_spark_lightning_address(&self) -> Option<iced::Task<Message>> {
        let spark = self.spark_client.clone()?;
        let db_addr = self
            .lightning_address
            .as_ref()
            .and_then(|la| la.lightning_address.as_ref())?;
        // Split `user@domain` → `user`. A row without `@` or with
        // an empty username portion is a partially-confirmed /
        // malformed record — log loudly so it surfaces for cleanup
        // instead of silently bailing.
        let db_username = db_addr.split('@').next().unwrap_or("");
        if db_username.is_empty() || !db_addr.contains('@') {
            log::warn!(
                "[CONNECT-CUBE] skipping reconcile: malformed DB \
                 lightning address {:?} (expected user@domain)",
                db_addr
            );
            return None;
        }
        let db_username = db_username.to_string();
        let db_addr = db_addr.clone();
        Some(iced::Task::perform(
            async move {
                match spark.get_lightning_address().await {
                    Ok(Some(info)) => {
                        // Only treat as "in sync" when the SDK's
                        // full `user@domain` matches the DB-confirmed
                        // reservation. Matching on username alone
                        // would miss `COINCUBE_LNURL_DOMAIN` drift
                        // (staging/prod env flip) — the SDK would
                        // hold `user@staging.coincube.io` while the
                        // DB has `user@coincube.io` and we'd display
                        // the wrong address. `register_lightning_address`
                        // can't retarget domains (the SDK's
                        // `lnurl_domain` is fixed at init), so
                        // surface the divergence for the operator
                        // instead of silently papering over it.
                        if info.lightning_address == db_addr {
                            ReconcileOutcome::AlreadyBound(info)
                        } else {
                            ReconcileOutcome::NeedsReRegistration(format!(
                                "Spark SDK is bound to '{}' but the confirmed \
                                 reservation is '{}'",
                                info.lightning_address, db_addr
                            ))
                        }
                    }
                    Ok(None) => {
                        // SDK has no record — try to bind the
                        // DB-confirmed username on this device.
                        match spark.register_lightning_address(db_username, None).await {
                            Ok(info) => {
                                // Same guard as the `AlreadyBound`
                                // branch: the SDK's `lnurl_domain` is
                                // fixed at init from
                                // `COINCUBE_LNURL_DOMAIN`, so a
                                // staging/prod env flip would let the
                                // register call succeed against the
                                // wrong domain (e.g. binds
                                // `user@staging.coincube.io` while the
                                // DB-confirmed reservation is
                                // `user@coincube.io`). Surface the
                                // divergence instead of returning a
                                // mismatched `ReRegistered`.
                                if info.lightning_address == db_addr {
                                    ReconcileOutcome::ReRegistered(info)
                                } else {
                                    // Roll back the orphan binding we
                                    // just created on the LNURL server
                                    // — leaving it would squat the
                                    // wrong-domain record forever.
                                    // Best-effort: log if the cleanup
                                    // itself fails so an operator can
                                    // remove it manually.
                                    let bound = info.lightning_address.clone();
                                    let mut suffix = "";
                                    if let Err(e) = spark.delete_lightning_address().await {
                                        log::error!(
                                            "[CONNECT-CUBE] failed to roll back \
                                             orphan Spark registration {:?} after \
                                             reconcile domain mismatch (expected {:?}): {}",
                                            bound,
                                            db_addr,
                                            e
                                        );
                                        suffix = " (orphan SDK registration left behind \
                                                  — please contact support)";
                                    }
                                    ReconcileOutcome::NeedsReRegistration(format!(
                                        "Spark SDK registered '{}' but the confirmed \
                                         reservation is '{}'{}",
                                        bound, db_addr, suffix
                                    ))
                                }
                            }
                            Err(e) => ReconcileOutcome::NeedsReRegistration(e.to_string()),
                        }
                    }
                    Err(e) => ReconcileOutcome::QueryFailed(e.to_string()),
                }
            },
            |outcome| {
                Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::LightningAddressReconciled(outcome),
                ))
            },
        ))
    }

    /// Register this cube with the backend. Called after login.
    /// Returns a task that sends CubeRegistered on completion.
    pub fn register_cube(&self) -> iced::Task<Message> {
        let Some(client) = self.client.clone() else {
            return iced::Task::none();
        };
        let req = RegisterCubeRequest {
            uuid: self.cube_uuid.clone(),
            name: self.cube_name.clone(),
            network: self.cube_network.clone(),
            // Monotonic upgrade-only: assert the Vault only when this device
            // holds it, else omit so a re-register never clobbers a `true`
            // reported elsewhere (PLAN-duress-vault-gate PR 3).
            has_vault: self.cube_has_vault.then_some(true),
        };
        iced::Task::perform(async move { client.register_cube(req).await }, |res| {
            Message::View(view::Message::ConnectCube(
                ConnectCubeMessage::CubeRegistered(res.map_err(|e| e.to_string())),
            ))
        })
    }

    /// Publishes this Cube's Connect-blinding encryption pubkey to the API
    /// (`PLAN-connect-blinding` PR D2 — the "registration wave" that also
    /// unblocks the server-side migration, api PR A5).
    ///
    /// This is what lets an invited Contact's Keychain seal its xpub to us: the
    /// API attaches the registered key to invite payloads and refuses
    /// envelope-mode enrolment for owners who haven't registered. So it must
    /// run **before** any invite is created — hence firing it as soon as the
    /// server cube id is known, not lazily at Vault-build time.
    ///
    /// Idempotent server-side; no-ops without a live client, a server cube id,
    /// or a derived pubkey. Failure is logged and retried on the next launch:
    /// nothing the user is doing right now depends on it, and surfacing a toast
    /// for a background hygiene call would be noise.
    pub fn register_encryption_pubkey(&mut self) -> iced::Task<Message> {
        if self.enc_pubkey_registered {
            return iced::Task::none();
        }
        let (Some(client), Some(server_id), Some(pubkey)) = (
            self.client.clone(),
            self.server_cube_id,
            self.cube_encryption_pubkey.clone(),
        ) else {
            return iced::Task::none();
        };
        self.enc_pubkey_registered = true;
        iced::Task::perform(
            async move {
                match client.put_cube_encryption_pubkey(server_id, &pubkey).await {
                    Ok(_) => {
                        log::info!(
                            "[CONNECT-CUBE] registered encryption pubkey {} for cube {}",
                            pubkey,
                            server_id
                        );
                        true
                    }
                    Err(e) => {
                        log::warn!(
                            "[CONNECT-CUBE] registering encryption pubkey for cube {} failed: {e}",
                            server_id
                        );
                        false
                    }
                }
            },
            |ok| {
                Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::EncryptionKeyRegistered(ok),
                ))
            },
        )
    }

    /// Asserts this Vault's descriptor fingerprint to Connect
    /// (`plans/PLAN-vault-identity-unification.md` D3/D4) — the id Keychain
    /// renders for the vault on its Cubes list and its paired-desktops row.
    ///
    /// The desktop is the only party that can supply it: the server holds no
    /// plaintext descriptor and by design never can. So every vault that
    /// predates this has a blank identity until the device holding it opens the
    /// Cube once, which is what this call converges.
    ///
    /// PATCH rather than a create-time field alone, because the common case is
    /// a vault shell that already exists. Re-asserting the same value is a
    /// server-side no-op that writes no audit row, so it is safe to fire on
    /// every launch without reading the current value back first.
    ///
    /// No-ops without a live client, a server cube id, or a fingerprint. A
    /// failure is logged and retried next launch: a 404 here just means this
    /// Cube's Vault is local-only (never registered with Connect), and a
    /// 403 means the account's Estate entitlement lapsed — the vault then keeps
    /// whatever identity it already had. Neither is worth a toast.
    pub fn assert_vault_fingerprint(&mut self) -> iced::Task<Message> {
        if self.vault_fingerprint_asserted {
            return iced::Task::none();
        }
        let (Some(client), Some(server_id), Some(fingerprint)) = (
            self.client.clone(),
            self.server_cube_id,
            self.vault_fingerprint.clone(),
        ) else {
            return iced::Task::none();
        };
        self.vault_fingerprint_asserted = true;
        iced::Task::perform(
            async move {
                let req = PatchConnectVaultRequest {
                    fingerprint: Some(fingerprint.clone()),
                };
                match client.patch_connect_vault(server_id, req).await {
                    Ok(_) => {
                        log::info!(
                            "[CONNECT-CUBE] asserted vault fingerprint {} for cube {}",
                            fingerprint,
                            server_id
                        );
                        true
                    }
                    Err(e) => {
                        log::warn!(
                            "[CONNECT-CUBE] asserting vault fingerprint for cube {} failed: {e}",
                            server_id
                        );
                        false
                    }
                }
            },
            |ok| {
                Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::VaultFingerprintAsserted(ok),
                ))
            },
        )
    }

    /// Re-report this Cube's Vault presence to the server when a Vault is
    /// created mid-session on an already-registered Cube, so its `hasVault`
    /// flips without waiting for a fresh registration (the duress vault gate;
    /// PLAN-duress-vault-gate PR 3). Sets the local flag, then fires an
    /// idempotent update against the known `server_cube_id`. No-op when the
    /// Cube isn't registered yet (the next `register_cube` carries the flag)
    /// or the panel has no live client.
    pub fn report_vault_created(&mut self) -> iced::Task<Message> {
        self.cube_has_vault = true;
        let (Some(client), Some(server_id)) = (self.client.clone(), self.server_cube_id) else {
            return iced::Task::none();
        };
        let req = UpdateCubeRequest {
            name: None,
            status: None,
            has_vault: Some(true),
        };
        iced::Task::perform(
            async move {
                if let Err(e) = client.update_cube(&server_id.to_string(), req).await {
                    log::warn!("[CONNECT-CUBE] re-report has_vault after Vault creation: {e}");
                }
            },
            |()| Message::CubeVaultReported,
        )
    }

    pub fn update_message(&mut self, msg: ConnectCubeMessage) -> iced::Task<Message> {
        match msg {
            ConnectCubeMessage::CubeRegistered(result) => {
                match result {
                    Ok(cube_resp) => {
                        log::info!(
                            "[CONNECT-CUBE] Registered cube {} (server ID: {})",
                            cube_resp.uuid,
                            cube_resp.id
                        );
                        self.server_cube_id = Some(cube_resp.id);
                        self.registration_error = None;
                        // Store the lightning address from the backend (or clear if None)
                        if cube_resp.lightning_address.is_some() {
                            self.lightning_address = Some(LightningAddress {
                                lightning_address: cube_resp.lightning_address,
                            });
                        } else {
                            self.lightning_address = None;
                        }
                        // Phase 4g: if the DB has a confirmed address,
                        // reconcile against the Spark SDK's local
                        // state. Covers device reinstall / SDK storage
                        // wipe / multi-device identity swap.
                        let reconcile_task = self.reconcile_spark_lightning_address();
                        // Trigger avatar load now that cube is registered.
                        let avatar_task = self.load_avatar_if_needed();
                        // The server cube id just became known, which is the
                        // only thing the encryption-pubkey PUT was waiting on.
                        // Publish now so invites created in this session carry
                        // the key (PLAN-connect-blinding PR D2). Skip it when
                        // the server already reports the key we'd send.
                        if cube_resp.encryption_pubkey.is_some()
                            && cube_resp.encryption_pubkey == self.cube_encryption_pubkey
                        {
                            self.enc_pubkey_registered = true;
                        }
                        let enc_key_task = self.register_encryption_pubkey();
                        // Same trigger, same reason: the server cube id was the
                        // only thing the vault-fingerprint PATCH was waiting on.
                        // Skip it when the registration response already carries
                        // the value we'd send.
                        if cube_resp.vault.as_ref().is_some_and(|v| {
                            Some(&v.fingerprint) == self.vault_fingerprint.as_ref()
                        }) {
                            self.vault_fingerprint_asserted = true;
                        }
                        let vault_fp_task = self.assert_vault_fingerprint();
                        let mut tasks = vec![enc_key_task, vault_fp_task];
                        tasks.extend(reconcile_task);
                        tasks.extend(avatar_task);
                        return iced::Task::batch(tasks);
                    }
                    Err(e) => {
                        log::error!("[CONNECT-CUBE] Failed to register cube: {}", e);
                        self.registration_error = Some(e);
                    }
                }
            }

            ConnectCubeMessage::EncryptionKeyRegistered(ok) => {
                // Background hygiene: on failure, clear the in-session latch so
                // a later trigger (e.g. the next `ensure_cube_registered`) can
                // retry rather than waiting for a relaunch.
                if !ok {
                    self.enc_pubkey_registered = false;
                }
            }

            ConnectCubeMessage::VaultFingerprintAsserted(ok) => {
                // Same background-hygiene contract as the encryption pubkey:
                // clear the in-session latch on failure so a later trigger can
                // retry without waiting for a relaunch.
                if !ok {
                    self.vault_fingerprint_asserted = false;
                }
            }

            ConnectCubeMessage::LightningAddressLoaded(ln_addr) => {
                self.lightning_address = ln_addr;
            }

            ConnectCubeMessage::LnUsernameChanged(input) => {
                self.ln_username_input = input.to_lowercase();
                self.ln_username_available = None;
                self.ln_username_error = None;
                self.ln_claim_error = None;

                // Client-side validation
                if let Some(err) = validate_ln_username(&self.ln_username_input) {
                    self.ln_check_version += 1;
                    if let Some(handle) = self.ln_check_abort.take() {
                        handle.abort();
                    }
                    self.ln_checking = false;
                    self.ln_username_error = Some(err);
                    return iced::Task::none();
                }

                let Some(spark) = self.spark_client.clone() else {
                    // No Spark backend means no way to claim — skip
                    // the debounced hint entirely. The claim button
                    // surfaces the same "Spark unavailable" error.
                    self.ln_check_version += 1;
                    if let Some(handle) = self.ln_check_abort.take() {
                        handle.abort();
                    }
                    self.ln_checking = false;
                    return iced::Task::none();
                };

                // Debounced availability check. Two sources, ANDed:
                // the Breez-hosted LNURL server (catches names
                // registered outside our DB) and our own Go API
                // (authoritative for @coincube.io — the same conflict
                // source the reserve step hits, including unconfirmed
                // or orphaned reservations Breez has never heard of;
                // Breez-only checking showed "Available" for names the
                // reserve then 409'd). The API result is still just a
                // hint — reserve remains the authoritative gate — so
                // an API *error* falls back to the Breez-only answer
                // rather than blocking typing. Abort any previous
                // in-flight task.
                if let Some(handle) = self.ln_check_abort.take() {
                    handle.abort();
                }
                self.ln_check_version += 1;
                let version = self.ln_check_version;
                let username = self.ln_username_input.clone();
                let api_client = self.client.clone();
                let api_cube_id = self.api_cube_id();
                self.ln_checking = true;
                let (task, abort_handle) = iced::Task::perform(
                    async move {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        let res = spark
                            .check_lightning_address_available(username.clone())
                            .await;
                        let res = match res {
                            Ok(true) => {
                                if let (Some(client), Some(cube_id)) = (api_client, api_cube_id) {
                                    match client
                                        .check_lightning_address_available(&cube_id, &username)
                                        .await
                                    {
                                        Ok(api_available) => Ok(api_available),
                                        Err(e) => {
                                            log::warn!(
                                                "[CONNECT-CUBE] API availability check \
                                                 failed, falling back to Breez-only \
                                                 hint: {}",
                                                e
                                            );
                                            Ok(true)
                                        }
                                    }
                                } else {
                                    Ok(true)
                                }
                            }
                            other => other,
                        };
                        (res, version)
                    },
                    move |(res, v)| match res {
                        Ok(available) => Message::View(view::Message::ConnectCube(
                            ConnectCubeMessage::LnUsernameChecked {
                                available,
                                error_message: if available {
                                    None
                                } else {
                                    Some("Username is taken".to_string())
                                },
                                version: v,
                            },
                        )),
                        Err(e) => Message::View(view::Message::ConnectCube(
                            ConnectCubeMessage::LnUsernameChecked {
                                available: false,
                                error_message: Some(e.to_string()),
                                version: v,
                            },
                        )),
                    },
                )
                .abortable();
                self.ln_check_abort = Some(abort_handle);
                return task;
            }

            ConnectCubeMessage::LnUsernameChecked {
                available,
                error_message,
                version,
            } => {
                if version == self.ln_check_version {
                    self.ln_checking = false;
                    self.ln_username_available = Some(available);
                    if !available {
                        self.ln_username_error =
                            Some(error_message.unwrap_or_else(|| "Username is taken".to_string()));
                    }
                }
            }

            ConnectCubeMessage::ClaimLightningAddress => {
                if self.ln_claiming {
                    return iced::Task::none();
                }
                let Some(client) = self.client.clone() else {
                    return iced::Task::none();
                };
                let Some(spark) = self.spark_client.clone() else {
                    self.ln_claim_error =
                        Some("Spark wallet is not available on this cube".to_string());
                    return iced::Task::none();
                };
                self.ln_claiming = true;
                self.ln_claim_error = None;
                let username = self.ln_username_input.clone();
                let Some(cube_id) = self.api_cube_id() else {
                    self.ln_claiming = false;
                    self.ln_claim_error = Some(
                        self.registration_error
                            .clone()
                            .unwrap_or_else(|| "Cube registration pending".to_string()),
                    );
                    return iced::Task::none();
                };
                return iced::Task::perform(
                    async move {
                        // Step 1: reserve in our DB. If the username is
                        // already taken (409) the API surfaces it here.
                        client
                            .reserve_lightning_address(&cube_id, &username)
                            .await
                            .map_err(|e| format!("Reserve failed: {}", e))?;

                        // Step 2: register against the Breez-hosted LNURL
                        // server via the Spark bridge. On failure, release
                        // the reservation (SDK side never succeeded, so
                        // only roll back the API reservation).
                        let register_result = spark
                            .register_lightning_address(username.clone(), None)
                            .await;
                        if let Err(e) = register_result {
                            let rb_note =
                                rollback_partial_claim(&client, None, &cube_id, &username).await;
                            return Err(format!("Register failed: {}{}", e, rb_note));
                        }

                        // Step 3: commit. API stamps
                        // `lightning_address_confirmed_at` on the
                        // existing reservation. Empty body — the
                        // reserve step already carried all the data
                        // the API needs. On failure roll back both
                        // the SDK registration and the reservation.
                        match client.confirm_lightning_address(&cube_id).await {
                            Ok(ln_addr) => Ok(ln_addr),
                            Err(e) => {
                                let rb_note = rollback_partial_claim(
                                    &client,
                                    Some(&spark),
                                    &cube_id,
                                    &username,
                                )
                                .await;
                                Err(format!("Confirm failed: {}{}", e, rb_note))
                            }
                        }
                    },
                    |res| match res {
                        Ok(ln_addr) => Message::View(view::Message::ConnectCube(
                            ConnectCubeMessage::LightningAddressClaimed(ln_addr),
                        )),
                        Err(e) => {
                            Message::View(view::Message::ConnectCube(ConnectCubeMessage::Error(e)))
                        }
                    },
                );
            }

            ConnectCubeMessage::LightningAddressClaimed(ln_addr) => {
                self.ln_claiming = false;
                self.lightning_address = Some(ln_addr);
                self.ln_username_input.clear();
                self.ln_username_available = None;
                self.ln_username_error = None;
                self.ln_reconcile_needs_reregister = None;
            }

            ConnectCubeMessage::BeginEditLightningAddress => {
                if self.ln_changing {
                    return iced::Task::none();
                }
                self.ln_editing = true;
                self.ln_username_input.clear();
                self.ln_username_available = None;
                self.ln_username_error = None;
                self.ln_claim_error = None;
                self.ln_change_confirm_pending = None;
            }

            ConnectCubeMessage::CancelEditLightningAddress => {
                if self.ln_changing {
                    return iced::Task::none();
                }
                self.ln_editing = false;
                self.ln_change_confirm_pending = None;
                self.ln_username_input.clear();
                self.ln_username_available = None;
                self.ln_username_error = None;
                self.ln_claim_error = None;
                self.ln_check_version += 1;
                if let Some(handle) = self.ln_check_abort.take() {
                    handle.abort();
                }
                self.ln_checking = false;
            }

            ConnectCubeMessage::RequestChangeLightningAddress => {
                if self.ln_changing {
                    return iced::Task::none();
                }
                let proposed = self.ln_username_input.trim().to_string();
                let valid_format = self.ln_username_error.is_none() && !proposed.is_empty();
                let available = self.ln_username_available == Some(true);
                let current_username = self
                    .lightning_address
                    .as_ref()
                    .and_then(|la| la.lightning_address.as_deref())
                    .and_then(|addr| addr.split('@').next())
                    .map(|u| u.to_string());
                let differs = current_username
                    .as_deref()
                    .map(|u| u != proposed)
                    .unwrap_or(true);
                if valid_format && available && differs {
                    self.ln_change_confirm_pending = Some(proposed);
                }
            }

            ConnectCubeMessage::DismissChangeConfirmation => {
                self.ln_change_confirm_pending = None;
            }

            ConnectCubeMessage::ConfirmChangeLightningAddress => {
                if self.ln_changing {
                    return iced::Task::none();
                }
                let Some(new_username) = self.ln_change_confirm_pending.take() else {
                    return iced::Task::none();
                };
                let Some(client) = self.client.clone() else {
                    return iced::Task::none();
                };
                let Some(spark) = self.spark_client.clone() else {
                    self.ln_claim_error =
                        Some("Spark wallet is not available on this cube".to_string());
                    return iced::Task::none();
                };
                let Some(cube_id) = self.api_cube_id() else {
                    self.ln_claim_error = Some(
                        self.registration_error
                            .clone()
                            .unwrap_or_else(|| "Cube registration pending".to_string()),
                    );
                    return iced::Task::none();
                };
                self.ln_changing = true;
                self.ln_claim_error = None;
                return iced::Task::perform(
                    async move {
                        // Step 1: server-side atomic username swap.
                        // Only the DB row changes — no DNS work.
                        let new_addr = match client
                            .update_lightning_address(&cube_id, &new_username)
                            .await
                        {
                            Ok(addr) => addr,
                            Err(e) => {
                                return LightningAddressChangeOutcome::ServerError(format!(
                                    "{}",
                                    e
                                ));
                            }
                        };

                        // Step 2: release the old SDK binding from
                        // Breez. Required because the SDK's register
                        // call doesn't replace an existing binding —
                        // see the reconciler at
                        // `reconcile_spark_lightning_address`.
                        if let Err(e) = spark.delete_lightning_address().await {
                            log::error!(
                                "[CONNECT-CUBE] change: SDK delete failed after \
                                 server commit (cube={}, new={}): {}",
                                cube_id,
                                new_username,
                                e
                            );
                            return LightningAddressChangeOutcome::SdkSyncFailed {
                                addr: new_addr,
                                message: format!(
                                    "Could not release the previous Lightning Address \
                                     binding on this device: {}. Tap Retry to finish \
                                     switching to {}.",
                                    e, new_username
                                ),
                            };
                        }

                        // Step 3: bind the new username on the
                        // Breez-hosted LNURL server.
                        if let Err(e) = spark
                            .register_lightning_address(new_username.clone(), None)
                            .await
                        {
                            log::error!(
                                "[CONNECT-CUBE] change: SDK register failed after \
                                 server commit + SDK delete (cube={}, new={}): {}",
                                cube_id,
                                new_username,
                                e
                            );
                            return LightningAddressChangeOutcome::SdkSyncFailed {
                                addr: new_addr,
                                message: format!(
                                    "Could not register {} with the Lightning Address \
                                     server: {}. Tap Retry to finish.",
                                    new_username, e
                                ),
                            };
                        }

                        LightningAddressChangeOutcome::Ok(new_addr)
                    },
                    |outcome| {
                        Message::View(view::Message::ConnectCube(
                            ConnectCubeMessage::LightningAddressUpdated(outcome),
                        ))
                    },
                );
            }

            ConnectCubeMessage::LightningAddressUpdated(outcome) => {
                self.ln_changing = false;
                match outcome {
                    LightningAddressChangeOutcome::Ok(addr) => {
                        self.lightning_address = Some(addr);
                        self.ln_editing = false;
                        self.ln_change_confirm_pending = None;
                        self.ln_username_input.clear();
                        self.ln_username_available = None;
                        self.ln_username_error = None;
                        self.ln_claim_error = None;
                        self.ln_reconcile_needs_reregister = None;
                    }
                    LightningAddressChangeOutcome::ServerError(msg) => {
                        // Stay in edit mode so the user can correct
                        // and retry; the address itself is unchanged.
                        self.ln_claim_error = Some(msg);
                    }
                    LightningAddressChangeOutcome::SdkSyncFailed { addr, message } => {
                        // Server committed; mirror that locally and
                        // surface the existing re-registration prompt
                        // so the user can retry the SDK side.
                        self.lightning_address = Some(addr);
                        self.ln_editing = false;
                        self.ln_change_confirm_pending = None;
                        self.ln_username_input.clear();
                        self.ln_username_available = None;
                        self.ln_username_error = None;
                        self.ln_claim_error = None;
                        self.ln_reconcile_needs_reregister = Some(message);
                    }
                }
            }

            ConnectCubeMessage::RetryLightningAddressReregister => {
                if self.ln_reregistering {
                    return iced::Task::none();
                }
                let Some(spark) = self.spark_client.clone() else {
                    return iced::Task::none();
                };
                let Some(db_addr) = self
                    .lightning_address
                    .as_ref()
                    .and_then(|la| la.lightning_address.as_ref())
                    .cloned()
                else {
                    return iced::Task::none();
                };
                // Same malformed-record guard as
                // `reconcile_spark_lightning_address`: without
                // `@domain` in the DB record, the post-register
                // comparison against the SDK's full `user@domain`
                // can never match, and the user would be stuck in
                // a retry loop they can't clear.
                let db_username = match db_addr.split('@').next() {
                    Some(u) if !u.is_empty() && db_addr.contains('@') => u.to_string(),
                    _ => {
                        log::warn!(
                            "[CONNECT-CUBE] skipping retry rebind: malformed DB \
                             lightning address {:?} (expected user@domain)",
                            db_addr
                        );
                        self.ln_reconcile_needs_reregister = Some(format!(
                            "Stored lightning address {:?} is malformed \
                             (expected user@domain) — please re-claim a username",
                            db_addr
                        ));
                        return iced::Task::none();
                    }
                };
                self.ln_reregistering = true;
                return iced::Task::perform(
                    async move {
                        // Idempotent: tolerates an already-empty SDK.
                        // Ignore failure here — the register that
                        // follows is the authoritative success
                        // signal, and a Breez-side "already gone"
                        // error shouldn't block recovery.
                        if let Err(e) = spark.delete_lightning_address().await {
                            log::warn!(
                                "[CONNECT-CUBE] retry rebind: SDK delete \
                                 returned {} (continuing to register)",
                                e
                            );
                        }
                        spark
                            .register_lightning_address(db_username, None)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    |res| {
                        Message::View(view::Message::ConnectCube(
                            ConnectCubeMessage::LightningAddressReregistered(res),
                        ))
                    },
                );
            }

            ConnectCubeMessage::LightningAddressReregistered(result) => {
                self.ln_reregistering = false;
                match result {
                    Ok(info) => {
                        let db_addr = self
                            .lightning_address
                            .as_ref()
                            .and_then(|la| la.lightning_address.as_deref());
                        if db_addr == Some(info.lightning_address.as_str()) {
                            self.ln_reconcile_needs_reregister = None;
                            log::info!(
                                "[CONNECT-CUBE] manual rebind succeeded: {}",
                                info.lightning_address
                            );
                        } else {
                            // Domain-drift guard, mirroring the
                            // reconciler. Don't claim "rebound" when
                            // the SDK's full address doesn't match
                            // the DB record.
                            self.ln_reconcile_needs_reregister = Some(format!(
                                "Spark SDK registered '{}' but the confirmed \
                                 reservation is '{}'",
                                info.lightning_address,
                                db_addr.unwrap_or("")
                            ));
                        }
                    }
                    Err(e) => {
                        self.ln_reconcile_needs_reregister = Some(e);
                    }
                }
            }

            ConnectCubeMessage::SparkLightningAddressChanged(info) => {
                match info {
                    Some(info) => {
                        // A `Some` payload means the SDK observed a
                        // register/change — on this device at the
                        // tail of the claim flow, or cross-device
                        // via realtime-sync replay. Only treat it
                        // as authoritative when it matches the
                        // DB-confirmed reservation; a mismatched
                        // payload means the SDK holds a binding we
                        // haven't confirmed server-side (pre-confirm
                        // claim-flow race, cross-device identity
                        // swap, stale cache), and mirroring it to
                        // the display would show the wrong address.
                        let db_addr = self
                            .lightning_address
                            .as_ref()
                            .and_then(|la| la.lightning_address.as_deref());
                        if db_addr == Some(info.lightning_address.as_str()) {
                            log::info!(
                                "[CONNECT-CUBE] Spark lightning address confirmed: {}",
                                info.lightning_address
                            );
                            // SDK matches the DB — any stale "needs
                            // re-registration" state is resolved.
                            self.ln_reconcile_needs_reregister = None;
                        } else {
                            log::warn!(
                                "[CONNECT-CUBE] Spark reports {:?} but DB record \
                                 is {:?} — triggering reconcile",
                                info.lightning_address,
                                db_addr
                            );
                            if let Some(task) = self.reconcile_spark_lightning_address() {
                                return task;
                            }
                        }
                    }
                    None => {
                        // If the DB still has a confirmed username
                        // (i.e. the user didn't initiate the delete
                        // on this device), trigger the same
                        // auto-reconcile path as on startup so the
                        // address rebinds without user action.
                        if let Some(task) = self.reconcile_spark_lightning_address() {
                            return task;
                        }
                    }
                }
            }

            ConnectCubeMessage::LightningAddressReconciled(outcome) => match outcome {
                ReconcileOutcome::AlreadyBound(info) => {
                    log::info!(
                        "[CONNECT-CUBE] Spark reports lightning address {}",
                        info.lightning_address
                    );
                    self.ln_reconcile_needs_reregister = None;
                }
                ReconcileOutcome::ReRegistered(info) => {
                    log::info!(
                        "[CONNECT-CUBE] Spark re-registered lightning address {}",
                        info.lightning_address
                    );
                    self.ln_reconcile_needs_reregister = None;
                }
                ReconcileOutcome::QueryFailed(e) => {
                    log::warn!(
                        "[CONNECT-CUBE] Spark lightning-address query failed \
                         (transient, will retry on next trigger): {}",
                        e
                    );
                }
                ReconcileOutcome::NeedsReRegistration(e) => {
                    log::error!(
                        "[CONNECT-CUBE] Spark register failed during reconcile — \
                         API and SDK are out of sync until the user re-claims: {}",
                        e
                    );
                    self.ln_reconcile_needs_reregister = Some(e);
                }
            },

            ConnectCubeMessage::RetryRegistration => {
                self.registration_error = None;
                return self.register_cube();
            }

            ConnectCubeMessage::CopyToClipboard(text) => {
                return iced::clipboard::write(text);
            }

            ConnectCubeMessage::Error(e) => {
                log::error!("[CONNECT-CUBE] Error: {}", e);
                if self.ln_claiming {
                    self.ln_claim_error = Some(e);
                    self.ln_claiming = false;
                } else if self.ln_checking {
                    self.ln_username_error = Some(e);
                    self.ln_checking = false;
                } else {
                    self.ln_claim_error = Some(e);
                }
            }

            ConnectCubeMessage::Avatar(avatar_msg) => {
                return self.update_avatar(avatar_msg);
            }

            ConnectCubeMessage::Members(msg) => {
                return cube_members::update(
                    &mut self.members,
                    msg,
                    self.client.clone(),
                    self.server_cube_id,
                );
            }
        }

        iced::Task::none()
    }

    fn update_avatar(&mut self, msg: crate::app::view::AvatarMessage) -> iced::Task<Message> {
        use crate::app::view::AvatarMessage;

        match msg {
            AvatarMessage::Enter => {
                self.avatar_error = None;
                let Some(client) = self.client.clone() else {
                    self.avatar_error = Some("Not signed in".to_string());
                    return iced::Task::none();
                };
                let Some(cid) = self.api_cube_id() else {
                    if let Some(ref e) = self.registration_error {
                        self.avatar_error = Some(e.clone());
                    }
                    return iced::Task::none();
                };
                return iced::Task::perform(async move { client.get_avatar(&cid).await }, |res| {
                    Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                        AvatarMessage::Loaded(res.map_err(|e| e.to_string())),
                    )))
                });
            }

            AvatarMessage::Loaded(result) => match result {
                Ok(data) => {
                    let has = data.has_avatar;
                    let active_id = data
                        .variants
                        .iter()
                        .find(|v| {
                            data.active_avatar_url
                                .as_deref()
                                .map(|u| u.ends_with(&v.id.to_string()))
                                .unwrap_or(false)
                        })
                        .map(|v| v.id);
                    self.avatar_data = Some(data);
                    if has {
                        self.avatar_step = AvatarFlowStep::Settings;
                        if let Some(id) = active_id {
                            if !self.avatar_image_cache.contains_key(&id) {
                                if let Some(client) = self.client.clone() {
                                    return iced::Task::perform(
                                        async move { client.fetch_avatar_image(id).await },
                                        move |res| {
                                            Message::View(view::Message::ConnectCube(
                                                ConnectCubeMessage::Avatar(
                                                    AvatarMessage::ImageLoaded {
                                                        variant_id: id,
                                                        result: res.map_err(|e| e.to_string()),
                                                    },
                                                ),
                                            ))
                                        },
                                    );
                                }
                            }
                        }
                    } else {
                        self.avatar_step = AvatarFlowStep::Questionnaire;
                    }
                }
                Err(e) => {
                    log::error!("[AVATAR] Load error: {}", e);
                    self.avatar_error = Some(e);
                }
            },

            AvatarMessage::SetStep(step) => {
                self.avatar_step = step;
            }

            AvatarMessage::GenderChanged(v) => self.avatar_draft.gender = v,
            AvatarMessage::ArchetypeChanged(v) => self.avatar_draft.archetype = v,
            AvatarMessage::AgeFeelChanged(v) => self.avatar_draft.age_feel = v,
            AvatarMessage::DemeanorChanged(v) => self.avatar_draft.demeanor = v,
            AvatarMessage::ArmorStyleChanged(v) => self.avatar_draft.armor_style = v,
            AvatarMessage::AccentMotifChanged(v) => self.avatar_draft.accent_motif = v,
            AvatarMessage::LaserEyesToggled(v) => self.avatar_draft.laser_eyes = v,

            AvatarMessage::Generate => {
                if self.avatar_generating {
                    return iced::Task::none();
                }
                let Some(client) = self.client.clone() else {
                    self.avatar_error = Some("Not signed in".to_string());
                    return iced::Task::none();
                };
                let req = AvatarGenerateRequest {
                    user_traits: self.avatar_draft.clone(),
                };
                let Some(cid) = self.api_cube_id() else {
                    self.avatar_generating = false;
                    self.avatar_error = Some(
                        self.registration_error
                            .clone()
                            .unwrap_or_else(|| "Cube registration pending".to_string()),
                    );
                    return iced::Task::none();
                };
                self.avatar_generating = true;
                self.avatar_error = None;
                self.avatar_step = AvatarFlowStep::Generating;
                return iced::Task::perform(
                    async move { client.post_avatar_generate(&cid, req).await },
                    |res| {
                        Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                            AvatarMessage::GenerateComplete(res.map_err(|e| e.to_string())),
                        )))
                    },
                );
            }

            AvatarMessage::GenerateComplete(result) => {
                self.avatar_generating = false;
                match result {
                    Ok(data) => {
                        let variant_id = data.variant.id;
                        let new_variant = data.variant.clone();
                        if let Some(ref mut ad) = self.avatar_data {
                            ad.has_avatar = true;
                            ad.active_avatar_url = Some(new_variant.image_url.clone());
                            if !ad.variants.iter().any(|v| v.id == new_variant.id) {
                                ad.variants.push(new_variant);
                            }
                            ad.identity = Some(data.identity);
                            // Decrement local regeneration count
                            if ad.regenerations_remaining > 0 {
                                ad.regenerations_remaining -= 1;
                            }
                        } else {
                            self.avatar_data = Some(crate::services::coincube::GetAvatarData {
                                has_avatar: true,
                                active_avatar_url: Some(data.variant.image_url.clone()),
                                identity: Some(data.identity),
                                variants: vec![data.variant],
                                regenerations_remaining: 0,
                                created_at: None,
                                updated_at: None,
                            });
                        }
                        self.avatar_step = AvatarFlowStep::Reveal;
                        if let Some(ref ad) = self.avatar_data {
                            if let Some(ref identity) = ad.identity {
                                self.avatar_draft = identity.user_traits.clone();
                            }
                        }
                        // Fetch image + refresh regeneration count in parallel
                        if let Some(client) = self.client.clone() {
                            let client2 = client.clone();
                            let Some(cid) = self.api_cube_id() else {
                                return iced::Task::none();
                            };
                            return iced::Task::batch([
                                iced::Task::perform(
                                    async move { client.fetch_avatar_image(variant_id).await },
                                    move |res| {
                                        Message::View(view::Message::ConnectCube(
                                            ConnectCubeMessage::Avatar(
                                                AvatarMessage::ImageLoaded {
                                                    variant_id,
                                                    result: res.map_err(|e| e.to_string()),
                                                },
                                            ),
                                        ))
                                    },
                                ),
                                iced::Task::perform(
                                    async move { client2.get_avatar_regenerations(&cid).await },
                                    |res| {
                                        Message::View(view::Message::ConnectCube(
                                            ConnectCubeMessage::Avatar(
                                                AvatarMessage::RegenerationsLoaded(
                                                    res.map_err(|e| e.to_string()),
                                                ),
                                            ),
                                        ))
                                    },
                                ),
                            ]);
                        }
                    }
                    Err(e) => {
                        log::error!("[AVATAR] Generate error: {}", e);
                        self.avatar_error = Some(e);
                        self.avatar_step = AvatarFlowStep::Questionnaire;
                    }
                }
            }

            AvatarMessage::SelectVariant(variant_id) => {
                let Some(client) = self.client.clone() else {
                    self.avatar_error = Some("Not signed in".to_string());
                    return iced::Task::none();
                };
                let Some(cid) = self.api_cube_id() else {
                    self.avatar_error = Some(
                        self.registration_error
                            .clone()
                            .unwrap_or_else(|| "Cube registration pending".to_string()),
                    );
                    return iced::Task::none();
                };
                return iced::Task::perform(
                    async move {
                        client
                            .post_avatar_select(&cid, AvatarSelectRequest { variant_id })
                            .await
                    },
                    |res| {
                        Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                            AvatarMessage::VariantSelected(res.map_err(|e| e.to_string())),
                        )))
                    },
                );
            }

            AvatarMessage::VariantSelected(result) => match result {
                Ok(data) => {
                    if let Some(ref mut ad) = self.avatar_data {
                        ad.active_avatar_url = Some(data.active_avatar_url);
                    }
                    let variant_id = data.variant_id;
                    if !self.avatar_image_cache.contains_key(&variant_id) {
                        if let Some(client) = self.client.clone() {
                            return iced::Task::perform(
                                async move { client.fetch_avatar_image(variant_id).await },
                                move |res| {
                                    Message::View(view::Message::ConnectCube(
                                        ConnectCubeMessage::Avatar(AvatarMessage::ImageLoaded {
                                            variant_id,
                                            result: res.map_err(|e| e.to_string()),
                                        }),
                                    ))
                                },
                            );
                        }
                    }
                }
                Err(e) => {
                    log::error!("[AVATAR] Select error: {}", e);
                    self.avatar_error = Some(e);
                }
            },

            AvatarMessage::RegenerationsLoaded(result) => match result {
                Ok(data) => {
                    if let Some(ref mut ad) = self.avatar_data {
                        ad.regenerations_remaining = data.remaining;
                    }
                }
                Err(e) => {
                    log::warn!("[AVATAR] Regenerations fetch error: {}", e);
                }
            },

            AvatarMessage::ImageLoaded { variant_id, result } => match result {
                Ok(bytes) => {
                    let handle = iced::widget::image::Handle::from_bytes(bytes.clone());
                    self.avatar_image_cache.insert(variant_id, (bytes, handle));
                }
                Err(e) => {
                    log::warn!(
                        "[AVATAR] Image load error for variant {}: {}",
                        variant_id,
                        e
                    );
                }
            },

            AvatarMessage::Retry => {
                self.avatar_error = None;
                self.avatar_step = AvatarFlowStep::Questionnaire;
            }

            AvatarMessage::DownloadAvatar => {
                let active_id = self.avatar_data.as_ref().and_then(|d| {
                    let url = d.active_avatar_url.as_deref().unwrap_or("");
                    d.variants
                        .iter()
                        .find(|v| url.ends_with(&v.id.to_string()))
                        .map(|v| v.id)
                });
                if let Some(id) = active_id {
                    if let Some((bytes, _)) = self.avatar_image_cache.get(&id) {
                        let bytes = bytes.clone();
                        return iced::Task::perform(
                            async move {
                                let Some(handle) = rfd::AsyncFileDialog::new()
                                    .set_title("Save Avatar")
                                    .set_file_name("coincube-avatar.png")
                                    .add_filter("PNG Image", &["png"])
                                    .save_file()
                                    .await
                                else {
                                    return Ok(());
                                };
                                std::fs::write(handle.path(), &bytes).map_err(|e| e.to_string())
                            },
                            |res| match res {
                                Ok(()) => Message::View(view::Message::ConnectCube(
                                    ConnectCubeMessage::Avatar(AvatarMessage::Noop),
                                )),
                                Err(e) => Message::View(view::Message::ConnectCube(
                                    ConnectCubeMessage::Avatar(AvatarMessage::SaveError(e)),
                                )),
                            },
                        );
                    }
                }
            }

            AvatarMessage::SaveError(e) => {
                log::error!("[AVATAR] Failed to save avatar to disk: {}", e);
                self.avatar_error = Some(e);
            }

            AvatarMessage::Noop => {}
        }

        iced::Task::none()
    }
}

/// Validate a lightning address username client-side.
fn validate_ln_username(username: &str) -> Option<String> {
    if username.is_empty() {
        return Some("Username is required".to_string());
    }
    if username.len() < 3 {
        return Some("Username must be at least 3 characters".to_string());
    }
    if username.len() > 64 {
        return Some("Username must be at most 64 characters".to_string());
    }
    if !username.chars().next().unwrap().is_ascii_alphanumeric() {
        return Some("Must start with a letter or number".to_string());
    }
    if !username.chars().last().unwrap().is_ascii_alphanumeric() {
        return Some("Must end with a letter or number".to_string());
    }
    let special = ['.', '_', '-'];
    for c in username.chars() {
        if !c.is_ascii_alphanumeric() && !special.contains(&c) {
            return Some(format!("Invalid character: '{}'", c));
        }
    }
    let chars: Vec<char> = username.chars().collect();
    for w in chars.windows(2) {
        if special.contains(&w[0]) && special.contains(&w[1]) {
            return Some("No consecutive special characters allowed".to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use coincube_spark_protocol::LightningAddressInfo;

    use crate::{
        app::view::AvatarMessage,
        services::coincube::{
            AvatarAccentMotif, AvatarAgeFeel, AvatarArmorStyle, AvatarDemeanor,
            AvatarDerivedTraits, AvatarGender, AvatarGenerateData, AvatarIdentity,
            AvatarResolvedDirectives, AvatarSelectData, AvatarVariant, CubeResponse, GetAvatarData,
            RegenerationData,
        },
    };

    fn panel() -> ConnectCubePanel {
        ConnectCubePanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "bitcoin".to_string(),
            false,
        )
    }

    fn lightning(address: &str) -> LightningAddress {
        LightningAddress {
            lightning_address: Some(address.to_string()),
        }
    }

    fn lightning_info(address: &str) -> LightningAddressInfo {
        let username = address.split('@').next().unwrap_or(address).to_string();
        LightningAddressInfo {
            lightning_address: address.to_string(),
            username: username.clone(),
            description: Some("Coincube".to_string()),
            lnurl_url: format!("https://example.com/.well-known/lnurlp/{username}"),
            lnurl_bech32: "lnurl1test".to_string(),
        }
    }

    fn cube_response(lightning_address: Option<&str>) -> CubeResponse {
        CubeResponse {
            encryption_pubkey: None,
            id: 42,
            uuid: "cube-uuid".to_string(),
            name: "Family Vault".to_string(),
            network: "bitcoin".to_string(),
            lightning_address: lightning_address.map(str::to_string),
            status: "active".to_string(),
            has_recovery_kit: false,
            has_vault: Some(true),
            members: Vec::new(),
            pending_invites: Vec::new(),
            vault: None,
        }
    }

    fn avatar_variant(id: u64) -> AvatarVariant {
        AvatarVariant {
            id,
            index: id as u32,
            image_url: format!("https://cdn.example/avatar/{id}.png"),
        }
    }

    fn avatar_data(has_avatar: bool, active_id: Option<u64>) -> GetAvatarData {
        GetAvatarData {
            has_avatar,
            active_avatar_url: active_id.map(|id| format!("https://cdn.example/avatar/{id}.png")),
            identity: None,
            variants: vec![avatar_variant(7), avatar_variant(8)],
            regenerations_remaining: 3,
            created_at: None,
            updated_at: None,
        }
    }

    fn identity() -> AvatarIdentity {
        AvatarIdentity {
            version: 1,
            seed_version: 1,
            seed_hash: "seed-hash".to_string(),
            lightning_address: "founder@example.com".to_string(),
            archetype: "ronin".to_string(),
            user_traits: AvatarUserTraits {
                gender: AvatarGender::Woman,
                archetype: crate::services::coincube::AvatarArchetype::Shogun,
                age_feel: AvatarAgeFeel::Elder,
                demeanor: AvatarDemeanor::Fierce,
                armor_style: AvatarArmorStyle::Heavy,
                accent_motif: AvatarAccentMotif::OrangeSun,
                laser_eyes: true,
            },
            derived_traits: AvatarDerivedTraits {
                pose: "front".to_string(),
                crop_style: "portrait".to_string(),
                hat_style: "none".to_string(),
                face_visibility: "visible".to_string(),
                eye_visibility: "visible".to_string(),
                weapon_mode: "none".to_string(),
                shoulder_profile: "square".to_string(),
                cloak_presence: "none".to_string(),
                armor_wear: "clean".to_string(),
                enso_style: "brush".to_string(),
                ink_density: "medium".to_string(),
                brush_texture: "dry".to_string(),
                splash_intensity: "low".to_string(),
                orange_placement: "accent".to_string(),
                ornament_level: "simple".to_string(),
            },
            resolved_directives: AvatarResolvedDirectives {
                composition: "composition".to_string(),
                silhouette: "silhouette".to_string(),
                face_treatment: "face".to_string(),
                armor_treatment: "armor".to_string(),
                mood: "mood".to_string(),
                orange_treatment: "orange".to_string(),
                ink_treatment: "ink".to_string(),
                eyes_treatment: "eyes".to_string(),
                background: "background".to_string(),
                archetype_flavor: "flavor".to_string(),
            },
        }
    }

    #[test]
    fn lightning_username_validation_covers_client_side_rules() {
        assert_eq!(
            validate_ln_username("").as_deref(),
            Some("Username is required")
        );
        assert_eq!(
            validate_ln_username("ab").as_deref(),
            Some("Username must be at least 3 characters")
        );
        assert_eq!(
            validate_ln_username(&"a".repeat(65)).as_deref(),
            Some("Username must be at most 64 characters")
        );
        assert_eq!(
            validate_ln_username("-abc").as_deref(),
            Some("Must start with a letter or number")
        );
        assert_eq!(
            validate_ln_username("abc_").as_deref(),
            Some("Must end with a letter or number")
        );
        assert_eq!(
            validate_ln_username("ab$c").as_deref(),
            Some("Invalid character: '$'")
        );
        assert_eq!(
            validate_ln_username("ab..c").as_deref(),
            Some("No consecutive special characters allowed")
        );
        assert!(validate_ln_username("abc-123_def.xyz").is_none());
    }

    #[test]
    fn registration_client_and_avatar_helpers_reset_session_state() {
        let mut panel = panel();
        assert!(panel.api_cube_id().is_none());
        assert!(panel.load_avatar_if_needed().is_none());
        let (_, handle) = panel.register_cube().abortable();
        handle.abort();

        panel.set_client(CoincubeClient::new());
        panel.server_cube_id = Some(42);
        assert_eq!(panel.api_cube_id().as_deref(), Some("42"));
        assert!(panel.load_avatar_if_needed().is_some());
        let (_, handle) = panel.register_cube().abortable();
        handle.abort();
        let _ = panel.report_vault_created();
        assert!(panel.cube_has_vault);

        panel.avatar_data = Some(avatar_data(true, Some(7)));
        panel.avatar_image_cache.insert(
            7,
            (
                vec![137, 80, 78, 71],
                iced::widget::image::Handle::from_bytes(vec![137, 80, 78, 71]),
            ),
        );
        assert!(panel.get_active_avatar_handle().is_some());
        if let Some(data) = panel.avatar_data.as_mut() {
            data.active_avatar_url = Some("https://cdn.example/avatar/70.png".to_string());
        }
        assert!(panel.get_active_avatar_handle().is_none());

        panel.ln_username_input = "founder".to_string();
        panel.ln_username_available = Some(true);
        panel.ln_username_error = Some("old".to_string());
        panel.ln_claim_error = Some("old".to_string());
        panel.lightning_address = Some(lightning("founder@example.com"));
        panel.ln_reconcile_needs_reregister = Some("retry".to_string());
        panel.avatar_error = Some("old".to_string());
        panel.clear_client();

        assert!(panel.client.is_none());
        assert!(panel.server_cube_id.is_none());
        assert!(panel.lightning_address.is_none());
        assert!(panel.ln_username_input.is_empty());
        assert!(panel.ln_username_available.is_none());
        assert!(panel.ln_username_error.is_none());
        assert!(panel.ln_claim_error.is_none());
        assert!(panel.ln_reconcile_needs_reregister.is_none());
        assert!(panel.avatar_data.is_none());
        assert!(panel.avatar_image_cache.is_empty());
    }

    #[test]
    fn lightning_address_state_machine_handles_non_network_branches() {
        let mut panel = panel();

        let _ = panel.update_message(ConnectCubeMessage::CubeRegistered(Ok(cube_response(Some(
            "founder@example.com",
        )))));
        assert_eq!(panel.server_cube_id, Some(42));
        assert_eq!(
            panel
                .lightning_address
                .as_ref()
                .and_then(|a| a.lightning_address.as_deref()),
            Some("founder@example.com")
        );

        let _ = panel.update_message(ConnectCubeMessage::CubeRegistered(Err(
            "register failed".to_string()
        )));
        assert_eq!(panel.registration_error.as_deref(), Some("register failed"));

        let _ = panel.update_message(ConnectCubeMessage::LnUsernameChanged("Bad$".to_string()));
        assert_eq!(panel.ln_username_input, "bad$");
        assert!(panel.ln_username_error.is_some());
        assert!(!panel.ln_checking);

        let _ = panel.update_message(ConnectCubeMessage::LnUsernameChanged(
            "Fresh_Name".to_string(),
        ));
        assert_eq!(panel.ln_username_input, "fresh_name");
        assert!(panel.ln_username_error.is_none());
        assert!(!panel.ln_checking);

        let stale_version = panel.ln_check_version + 1;
        let _ = panel.update_message(ConnectCubeMessage::LnUsernameChecked {
            available: false,
            error_message: Some("taken".to_string()),
            version: stale_version,
        });
        assert!(panel.ln_username_available.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LnUsernameChecked {
            available: false,
            error_message: None,
            version: panel.ln_check_version,
        });
        assert_eq!(panel.ln_username_available, Some(false));
        assert_eq!(
            panel.ln_username_error.as_deref(),
            Some("Username is taken")
        );

        let _ = panel.update_message(ConnectCubeMessage::ClaimLightningAddress);
        assert!(!panel.ln_claiming);

        panel.set_client(CoincubeClient::new());
        let _ = panel.update_message(ConnectCubeMessage::ClaimLightningAddress);
        assert_eq!(
            panel.ln_claim_error.as_deref(),
            Some("Spark wallet is not available on this cube")
        );

        let _ = panel.update_message(ConnectCubeMessage::BeginEditLightningAddress);
        assert!(panel.ln_editing);
        panel.ln_username_input = "newname".to_string();
        panel.ln_username_available = Some(true);
        panel.ln_username_error = None;
        let _ = panel.update_message(ConnectCubeMessage::RequestChangeLightningAddress);
        assert_eq!(panel.ln_change_confirm_pending.as_deref(), Some("newname"));
        let _ = panel.update_message(ConnectCubeMessage::DismissChangeConfirmation);
        assert!(panel.ln_change_confirm_pending.is_none());
        let _ = panel.update_message(ConnectCubeMessage::CancelEditLightningAddress);
        assert!(!panel.ln_editing);
        assert!(panel.ln_username_input.is_empty());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressUpdated(
            LightningAddressChangeOutcome::ServerError("conflict".to_string()),
        ));
        assert_eq!(panel.ln_claim_error.as_deref(), Some("conflict"));

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressUpdated(
            LightningAddressChangeOutcome::SdkSyncFailed {
                addr: lightning("new@example.com"),
                message: "sdk failed".to_string(),
            },
        ));
        assert_eq!(
            panel.ln_reconcile_needs_reregister.as_deref(),
            Some("sdk failed")
        );

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressUpdated(
            LightningAddressChangeOutcome::Ok(lightning("ok@example.com")),
        ));
        assert!(panel.ln_reconcile_needs_reregister.is_none());
        assert!(!panel.ln_editing);
    }

    #[test]
    fn reconciliation_messages_update_prompt_state_without_spark() {
        let mut panel = panel();
        panel.lightning_address = Some(lightning("founder@example.com"));

        let _ = panel.update_message(ConnectCubeMessage::SparkLightningAddressChanged(Some(
            lightning_info("founder@example.com"),
        )));
        assert!(panel.ln_reconcile_needs_reregister.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReconciled(
            ReconcileOutcome::AlreadyBound(lightning_info("founder@example.com")),
        ));
        assert!(panel.ln_reconcile_needs_reregister.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReconciled(
            ReconcileOutcome::ReRegistered(lightning_info("founder@example.com")),
        ));
        assert!(panel.ln_reconcile_needs_reregister.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReconciled(
            ReconcileOutcome::QueryFailed("offline".to_string()),
        ));
        assert!(panel.ln_reconcile_needs_reregister.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReconciled(
            ReconcileOutcome::NeedsReRegistration("needs retry".to_string()),
        ));
        assert_eq!(
            panel.ln_reconcile_needs_reregister.as_deref(),
            Some("needs retry")
        );

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReregistered(Ok(
            lightning_info("founder@example.com"),
        )));
        assert!(panel.ln_reconcile_needs_reregister.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReregistered(Ok(
            lightning_info("wrong@example.com"),
        )));
        assert!(panel.ln_reconcile_needs_reregister.is_some());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReregistered(Err(
            "still down".to_string(),
        )));
        assert_eq!(
            panel.ln_reconcile_needs_reregister.as_deref(),
            Some("still down")
        );
    }

    #[test]
    fn avatar_state_machine_handles_local_transitions_and_results() {
        let mut panel = panel();

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Enter));
        assert_eq!(panel.avatar_error.as_deref(), Some("Not signed in"));

        panel.set_client(CoincubeClient::new());
        panel.registration_error = Some("registration pending".to_string());
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Enter));
        assert_eq!(panel.avatar_error.as_deref(), Some("registration pending"));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Loaded(Ok(
            avatar_data(false, None),
        ))));
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Questionnaire));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Loaded(Ok(
            avatar_data(true, Some(7)),
        ))));
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Settings));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Loaded(Err(
            "load failed".to_string(),
        ))));
        assert_eq!(panel.avatar_error.as_deref(), Some("load failed"));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::SetStep(
            AvatarFlowStep::Reveal,
        )));
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Reveal));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::GenderChanged(
            AvatarGender::Woman,
        )));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::AgeFeelChanged(
            AvatarAgeFeel::Young,
        )));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::DemeanorChanged(
            AvatarDemeanor::Calm,
        )));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(
            AvatarMessage::ArmorStyleChanged(AvatarArmorStyle::Standard),
        ));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(
            AvatarMessage::AccentMotifChanged(AvatarAccentMotif::Seal),
        ));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::LaserEyesToggled(
            true,
        )));
        assert_eq!(panel.avatar_draft.gender, AvatarGender::Woman);
        assert_eq!(panel.avatar_draft.age_feel, AvatarAgeFeel::Young);
        assert!(panel.avatar_draft.laser_eyes);

        panel.avatar_generating = true;
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Generate));
        assert!(panel.avatar_generating);

        panel.avatar_generating = false;
        panel.client = None;
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Generate));
        assert_eq!(panel.avatar_error.as_deref(), Some("Not signed in"));

        panel.set_client(CoincubeClient::new());
        panel.server_cube_id = None;
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Generate));
        assert_eq!(panel.avatar_error.as_deref(), Some("registration pending"));

        let generated = AvatarGenerateData {
            identity: identity(),
            variant: avatar_variant(9),
        };
        panel.avatar_data = Some(avatar_data(true, Some(7)));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::GenerateComplete(
            Ok(generated),
        )));
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Reveal));
        assert_eq!(panel.avatar_draft.gender, AvatarGender::Woman);
        assert_eq!(
            panel.avatar_data.as_ref().unwrap().regenerations_remaining,
            2
        );

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::GenerateComplete(
            Err("generation failed".to_string()),
        )));
        assert_eq!(panel.avatar_error.as_deref(), Some("generation failed"));
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Questionnaire));

        panel.client = None;
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::SelectVariant(8)));
        assert_eq!(panel.avatar_error.as_deref(), Some("Not signed in"));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::VariantSelected(
            Ok(AvatarSelectData {
                active_avatar_url: "https://cdn.example/avatar/8.png".to_string(),
                variant_id: 8,
            }),
        )));
        assert_eq!(
            panel
                .avatar_data
                .as_ref()
                .unwrap()
                .active_avatar_url
                .as_deref(),
            Some("https://cdn.example/avatar/8.png")
        );

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::VariantSelected(
            Err("select failed".to_string()),
        )));
        assert_eq!(panel.avatar_error.as_deref(), Some("select failed"));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(
            AvatarMessage::RegenerationsLoaded(Ok(RegenerationData {
                total_allowed: 5,
                used: 4,
                remaining: 1,
            })),
        ));
        assert_eq!(
            panel.avatar_data.as_ref().unwrap().regenerations_remaining,
            1
        );

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::ImageLoaded {
            variant_id: 8,
            result: Ok(vec![137, 80, 78, 71]),
        }));
        assert!(panel.avatar_image_cache.contains_key(&8));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::SaveError(
            "disk full".to_string(),
        )));
        assert_eq!(panel.avatar_error.as_deref(), Some("disk full"));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Retry));
        assert!(panel.avatar_error.is_none());
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Questionnaire));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Noop));
    }
}
