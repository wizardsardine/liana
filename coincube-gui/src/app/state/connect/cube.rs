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
        LightningAddress, RegisterCubeRequest,
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
    ) -> Self {
        ConnectCubePanel {
            cube_uuid,
            cube_name,
            cube_network,
            server_cube_id: None,
            registration_error: None,
            lightning_address: None,
            ln_username_input: String::new(),
            ln_username_available: None,
            ln_username_error: None,
            ln_claim_error: None,
            ln_claiming: false,
            ln_checking: false,
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
        };
        iced::Task::perform(async move { client.register_cube(req).await }, |res| {
            Message::View(view::Message::ConnectCube(
                ConnectCubeMessage::CubeRegistered(res.map_err(|e| e.to_string())),
            ))
        })
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
                        match (reconcile_task, avatar_task) {
                            (Some(r), Some(a)) => return iced::Task::batch([r, a]),
                            (Some(t), None) | (None, Some(t)) => return t,
                            (None, None) => {}
                        }
                    }
                    Err(e) => {
                        log::error!("[CONNECT-CUBE] Failed to register cube: {}", e);
                        self.registration_error = Some(e);
                    }
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

                // Debounced availability check against the Breez-hosted
                // LNURL server. This is the UX hint during typing — the
                // authoritative conflict check is what our Go API does
                // on reserve. Abort any previous in-flight task.
                if let Some(handle) = self.ln_check_abort.take() {
                    handle.abort();
                }
                self.ln_check_version += 1;
                let version = self.ln_check_version;
                let username = self.ln_username_input.clone();
                self.ln_checking = true;
                let (task, abort_handle) = iced::Task::perform(
                    async move {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        let res = spark.check_lightning_address_available(username).await;
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
