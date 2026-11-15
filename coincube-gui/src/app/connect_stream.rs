//! Connect stream health, panel routing and device registration renewal.
use super::*;

impl App {
    /// Top-level handler for `Message::ConnectStream`. PR A logs and
    /// persists the latest event seq; PR B's session-routing logic is
    /// folded in here when the per-modal dispatch lands.
    pub(super) fn handle_connect_stream(
        &mut self,
        event: crate::services::connect::grpc::ConnectStreamMessage,
    ) -> Task<Message> {
        use crate::services::connect::grpc::ConnectStreamMessage as M;
        match event {
            M::Connected => {
                log::info!("[CONNECT GRPC] Stream connected");
                self.connect_renewal = Renewal::default();
                self.set_connect_status(ConnectionStatus::Connected)
            }
            M::Disconnected(reason) => {
                log::warn!("[CONNECT GRPC] Stream disconnected: {}", reason);
                // The stream reports a failure before announcing its retry;
                // keep that failure until a connection succeeds. A drop from
                // a live stream reads as Reconnecting…, the first attempt
                // as Connecting….
                match self.cache.connect_stream_status {
                    ConnectionStatus::Connected => {
                        self.set_connect_status(ConnectionStatus::Reconnecting)
                    }
                    ConnectionStatus::Inactive => {
                        self.set_connect_status(ConnectionStatus::Connecting)
                    }
                    _ => Task::none(),
                }
            }
            M::Trouble(trouble) => {
                use crate::services::connect::grpc::trouble::Trouble;
                // A renewal in progress or one that needs the user outranks
                // the stream's own trouble.
                if matches!(
                    self.cache.connect_stream_status,
                    ConnectionStatus::Renewing | ConnectionStatus::RenewFailed
                ) {
                    return Task::none();
                }
                self.set_connect_status(match trouble {
                    Trouble::Offline => ConnectionStatus::Offline,
                    Trouble::Unreachable => ConnectionStatus::Reconnecting,
                })
            }
            M::Refused(refusal) => {
                use crate::services::connect::grpc::trouble::Refusal;
                match refusal {
                    // Connect has no record of this id: renew it (C1).
                    Refusal::DeviceUnknown => self.renew_connect_device(false),
                    // Removed from the account: never renewed (C3). Signing
                    // out drops the stream.
                    Refusal::DeviceRevoked => {
                        log::warn!("[CONNECT] This computer was removed from the Connect account");
                        Task::batch([
                            Task::done(Message::ConnectSignedOutBy(SignedOutBy::ComputerRemoved)),
                            Task::done(Message::View(view::Message::ConnectAccount(
                                view::ConnectAccountMessage::LogOut,
                            ))),
                        ])
                    }
                    // A server without reasons: renewing could undo a
                    // removal, so the user decides (Try again).
                    Refusal::Unexplained => self.set_connect_status(ConnectionStatus::RenewFailed),
                }
            }
            M::Error(err) => {
                // An in-stream error or an expired sign-in. The REST client
                // refreshes the shared token (or signs out if Connect
                // refuses), and the stream retries with it.
                log::warn!("[CONNECT GRPC] Stream error: {}", err);
                if matches!(
                    self.cache.connect_stream_status,
                    ConnectionStatus::Renewing | ConnectionStatus::RenewFailed
                ) {
                    return Task::none();
                }
                self.set_connect_status(ConnectionStatus::Reconnecting)
            }
            M::SessionEvent(session_event) => {
                log::info!(
                    "[CONNECT GRPC] SessionEvent seq={} type={:?} session={}",
                    session_event.event_seq,
                    session_event.event_type,
                    session_event.session_id,
                );
                // Persist the latest seq so a restart resumes from the
                // right cursor. Best-effort — log and continue on error.
                let seq = session_event.event_seq;
                let persist_task = if let Some(email) = self.connect_email.clone() {
                    let network_dir = self.datadir.network_directory(self.cache.chain());
                    Task::perform(
                        async move {
                            if let Err(e) =
                                crate::services::connect::client::cache::set_last_seen_event_seq_for_email(
                                    &network_dir,
                                    &email,
                                    seq,
                                )
                                .await
                            {
                                log::warn!(
                                    "[CONNECT GRPC] Failed to persist last_seen_event_seq={}: {}",
                                    seq,
                                    e,
                                );
                            }
                        },
                        |_| Message::CacheUpdated,
                    )
                } else {
                    Task::none()
                };
                // Fan the event out via Message::KeychainSign(StreamEvent).
                // It travels through the standard update path and is
                // delegated to the active PSBT modal (if any) by
                // `PsbtState`'s catchall arm — modals that don't
                // recognise the session_id are no-ops.
                let dispatch_task = Task::done(Message::KeychainSign(
                    crate::app::state::vault::keychain_sign::KeychainSignMessage::StreamEvent(
                        session_event,
                    ),
                ));
                Task::batch([persist_task, dispatch_task])
            }
            M::DuressActivated { unlock_at, source } => {
                // Phase 7b: remote duress activation. Persist active state to
                // DuressLocalState (at the data-dir root) WITHOUT wiping —
                // remote activation can be accidental, so local Cube data is
                // left intact. The UI lock is sequenced AFTER the persist (it's
                // the task's completion message, not a parallel batch) so the
                // cryptic screen never appears before `active` is durable; if
                // the app exits right after seeing the lock, the relaunch
                // reconcile still routes back into duress.
                log::warn!(
                    "[CONNECT GRPC] Duress activated remotely (source={})",
                    source
                );
                let root = self.datadir.path().to_path_buf();
                Task::perform(
                    async move {
                        // load() already maps a missing file to Ok(default); a
                        // real parse/IO error must NOT be papered over with a
                        // default that save() then writes back, clobbering valid
                        // state (enrolled / account_id / encrypted code). Skip
                        // the persist on a real read error — the UI still locks
                        // below, and the DuressLockRemote backstop / session-check
                        // re-sync handle durability.
                        let mut st = match crate::services::duress::DuressLocalState::load(&root) {
                            Ok(st) => st,
                            Err(e) => {
                                log::warn!(
                                    "[CONNECT GRPC] reading duress state failed; \
                                     not overwriting: {e}"
                                );
                                return;
                            }
                        };
                        st.active = true;
                        st.unlock_at = unlock_at;
                        // Retry: a failed persist would let a relaunch reconcile
                        // (which keys off st.active) drop back to the normal Home
                        // flow with Cube data intact while the server is still in
                        // duress. The DuressLockRemote handler re-persists as a
                        // final backstop tied to the UI lock.
                        for attempt in 1..=3 {
                            match st.save(&root) {
                                Ok(()) => break,
                                Err(e) => log::warn!(
                                    "[CONNECT GRPC] persist remote duress state \
                                     attempt {attempt}/3 failed: {e}"
                                ),
                            }
                        }
                    },
                    |_| Message::View(view::Message::DuressLockRemote),
                )
            }
            M::DuressCleared => {
                log::info!("[CONNECT GRPC] Duress cleared remotely");
                let root = self.datadir.path().to_path_buf();
                Task::perform(
                    async move {
                        // As above: skip on a real read error so a transient
                        // failure can't clobber valid state with a default. The
                        // cryptic screen's own server poll re-syncs the cleared
                        // state on the next check.
                        let mut st = match crate::services::duress::DuressLocalState::load(&root) {
                            Ok(st) => st,
                            Err(e) => {
                                log::warn!(
                                    "[CONNECT GRPC] reading duress state failed; \
                                     not overwriting: {e}"
                                );
                                return;
                            }
                        };
                        st.active = false;
                        st.unlock_at = None;
                        if let Err(e) = st.save(&root) {
                            log::warn!("[CONNECT GRPC] Failed to clear remote duress state: {e}");
                        }
                    },
                    |_| Message::CacheUpdated,
                )
            }
            M::DuressDisabled { account_id } => {
                // Issue 2: duress disabled account-wide on another device. Disarm
                // THIS device locally (clear Cube PIN hashes + local state +
                // queue) — no UI lock, no wipe. `reconcile_duress_disarm` only
                // touches a matching Connect enrollment, so a sovereign local
                // enrollment (or an unrelated account) is left untouched. Refresh
                // the Duress panel afterwards if the user is sitting in it.
                log::info!("[CONNECT GRPC] Duress disabled remotely");
                let datadir = self.datadir.clone();
                let gen = self.panels.connect.account.session_generation();
                Task::perform(
                    async move { reconcile_duress_disarm(datadir, account_id).await },
                    move |res| {
                        Message::View(view::Message::ConnectAccount(
                            view::ConnectAccountMessage::Duress(
                                view::DuressMessage::DisarmComplete(res, gen),
                            ),
                        ))
                    },
                )
            }
        }
    }

    /// The Connect control, its panel and renewal; everything else routes on.
    pub(super) fn update_connect_chrome(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::View(view::Message::Settings(
                view::SettingsMessage::ReregisterConnectDevice,
            )) => self.renew_connect_device(true),
            Message::View(view::Message::Settings(
                view::SettingsMessage::ConnectDeviceReregistered(result),
            )) => self.settle_connect_renewal(result),
            Message::ConnectLightTick => {
                self.cache.connect_light_dim = !self.cache.connect_light_dim;
                Task::none()
            }
            Message::ConnectRenewalRetry => {
                if self.cache.connect_stream_status == ConnectionStatus::Renewing {
                    self.renew_connect_device(false)
                } else {
                    Task::none()
                }
            }
            Message::ConnectSignedOutBy(reason) => {
                self.cache.connect_signed_out_by = Some(reason);
                self.cache.connect_panel_open = false;
                Task::none()
            }
            Message::View(view::Message::ToggleConnectPanel) => {
                self.cache.connect_panel_open = !self.cache.connect_panel_open;
                self.cache.cube_switcher_open = false;
                self.cache.cube_lock_confirmation = false;
                if self.cache.connect_panel_open {
                    iced::widget::operation::focus(view::nav::shell::connect_panel_first_focus(
                        &self.cache,
                    ))
                } else {
                    iced::widget::operation::focus(view::nav::shell::CONNECTION_ID)
                }
            }
            Message::View(view::Message::CloseConnectPanel) => {
                if !self.cache.connect_panel_open {
                    return Task::none();
                }
                self.cache.connect_panel_open = false;
                iced::widget::operation::focus(view::nav::shell::CONNECTION_ID)
            }
            Message::View(view::Message::DismissConnectSignOut) => {
                self.cache.connect_signed_out_by = None;
                self.cache.connect_panel_open = false;
                iced::widget::operation::focus(view::nav::shell::CONNECTION_ID)
            }
            message => self.route(message),
        }
    }

    /// Sets the live-updates status and tells an open Keychain signing modal.
    pub(super) fn set_connect_status(&mut self, status: ConnectionStatus) -> Task<Message> {
        if self.cache.connect_stream_status == status {
            return Task::none();
        }
        self.cache.connect_stream_status = status;
        stream_health_dispatch(status)
    }

    /// Registers this computer again (C1). Automatic attempts stop after
    /// [`RENEWAL_ATTEMPTS`]; Try again (`manual`) always makes one more.
    pub(super) fn renew_connect_device(&mut self, manual: bool) -> Task<Message> {
        if self.connect_renewal.in_flight {
            return Task::none();
        }
        if manual {
            self.connect_renewal.attempts = RENEWAL_ATTEMPTS - 1;
        } else if self.connect_renewal.attempts >= RENEWAL_ATTEMPTS {
            return self.set_connect_status(ConnectionStatus::RenewFailed);
        }
        let Some(task) = crate::app::state::settings::connections::renewal_task(&self.cache) else {
            log::warn!("[CONNECT] Can't renew this computer's registration: Connect isn't set up");
            return self.set_connect_status(ConnectionStatus::RenewFailed);
        };
        self.connect_renewal.attempts += 1;
        self.connect_renewal.in_flight = true;
        log::info!(
            "[CONNECT] Renewing this computer's registration (attempt {}{})",
            self.connect_renewal.attempts,
            if manual { ", Try again" } else { "" }
        );
        Task::batch([self.set_connect_status(ConnectionStatus::Renewing), task])
    }

    /// A renewal settled. Success swaps the id into the stream; a failure
    /// retries after [`RENEWAL_RETRY`] until the attempts run out.
    pub(super) fn settle_connect_renewal(
        &mut self,
        result: Result<view::ReregisteredDevice, String>,
    ) -> Task<Message> {
        self.connect_renewal.in_flight = false;
        match result {
            Ok(registered) => {
                // Logged for support instead of shown (C6).
                log::info!(
                    "[CONNECT] Renewed this computer's registration: {} -> {}",
                    registered.replaces,
                    registered.device_id
                );
                self.adopt_reregistered_device(&registered)
                    .unwrap_or_else(Task::none)
            }
            Err(e) => {
                log::warn!(
                    "[CONNECT] Renewing this computer's registration failed: {}",
                    e
                );
                if self.connect_renewal.attempts >= RENEWAL_ATTEMPTS {
                    self.set_connect_status(ConnectionStatus::RenewFailed)
                } else {
                    Task::perform(tokio::time::sleep(RENEWAL_RETRY), |_| {
                        Message::ConnectRenewalRetry
                    })
                }
            }
        }
    }

    /// A successful renewal hands back the device id. Swap it into the stream
    /// config: the subscription is keyed on `device_id`, so a stream stopped
    /// by a refusal (or one still using the stale id) is replaced by one that
    /// says hello with the new id.
    /// The swap only applies to the stream the request was started from: a
    /// result landing after sign-out (which clears the stream config), for
    /// another account, or after a newer sign-in already moved the stream to
    /// a different id is ignored, so it can't bring the device card back or
    /// replace a newer registration.
    /// An unchanged id restarts the stream too (through the subscription's
    /// restart count): the running one may be sleeping on a refusal
    /// re-check for minutes after Connect accepted the registration.
    pub(super) fn adopt_reregistered_device(
        &mut self,
        registered: &view::ReregisteredDevice,
    ) -> Option<Task<Message>> {
        if self.connect_email.as_deref() != Some(registered.email.as_str()) {
            return None;
        }
        let cfg = self.connect_stream_config.as_mut()?;
        if cfg.device_id != registered.replaces {
            return None;
        }
        if cfg.device_id == registered.device_id {
            self.connect_stream_restart = self.connect_stream_restart.wrapping_add(1);
        } else {
            cfg.device_id = registered.device_id.clone();
            self.cache.connect_device_id = Some(registered.device_id.clone());
        }
        // The new subscription reports Connected or a refusal; until then
        // the light keeps pulsing amber.
        Some(self.set_connect_status(ConnectionStatus::Renewing))
    }
}

impl App {
    pub(super) fn connect_stream_ready(
        &mut self,
        cfg: Option<(
            crate::services::connect::grpc::stream::ConnectStreamConfig,
            String,
        )>,
    ) -> Task<Message> {
        match cfg {
            Some((mut cfg, email)) => {
                if self.connect_email.as_deref() != Some(email.as_str())
                    || !self
                        .connect_auth
                        .as_ref()
                        .is_some_and(|auth| Arc::ptr_eq(auth, &cfg.tokens))
                {
                    return Task::none();
                }
                let current = match cfg.tokens.try_read() {
                    Ok(current) => current,
                    Err(_) => {
                        let tokens = cfg.tokens.clone();
                        let retry_cfg = cfg.clone();
                        return Task::perform(
                            async move {
                                drop(tokens.read().await);
                                Message::ConnectStreamReady(Some((retry_cfg, email)))
                            },
                            |message| message,
                        );
                    }
                };
                let network_dir = self
                    .cache
                    .datadir_path
                    .network_directory(self.cache.network);
                let cached_account =
                    crate::services::connect::client::cache::ConnectCache::from_file(&network_dir)
                        .ok()
                        .and_then(|cache| cache.active_account().cloned())
                        .filter(|account| {
                            account.email == email
                                && account.tokens.access_token == current.access_token
                                && account.tokens.refresh_token == current.refresh_token
                        });
                drop(current);
                let Some(account) = cached_account else {
                    return Task::none();
                };
                let Some(device_id) = account.device_id.filter(|id| !id.trim().is_empty()) else {
                    return Task::none();
                };
                // About may have completed re-registration while the
                // async vault lookup ran. Install its current id, not
                // the one captured before that await.
                cfg.device_id = device_id;
                cfg.last_seen_seq = cfg
                    .last_seen_seq
                    .max(account.last_seen_event_seq.unwrap_or(0));
                tracing::info!(
                    "Connect stream ready (device_id={}, last_seen_seq={})",
                    cfg.device_id,
                    cfg.last_seen_seq,
                );
                // Mirror into Cache so deep panels (the open
                // PSBT modal in particular) can spin up a
                // GrpcSessionClient on demand.
                self.cache.connect_grpc_url = Some(cfg.grpc_url.clone());
                self.cache.connect_tokens = Some(cfg.tokens.clone());
                self.cache.connect_device_id = Some(cfg.device_id.clone());
                self.cache.connect_email = Some(email);
                self.connect_stream_config = Some(cfg);
                // Subscription will mount on the next render
                // tick — show `Connecting` until the first
                // `ConnectStreamMessage::Connected` lands.
                self.cache.connect_stream_status = ConnectionStatus::Connecting;
            }
            None => {
                tracing::debug!("Connect stream not started: missing grpc_url or device_id",);
            }
        }
        Task::none()
    }

    pub(super) fn trigger_connect_stream_ready(
        &mut self,
        network: coincube_core::miniscript::bitcoin::Network,
        datadir: CoincubeDirectory,
        tokens: Arc<
            tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>,
        >,
        email: String,
        cube_uuid: Option<String>,
    ) -> Task<Message> {
        if self.in_app_refresh_commit.is_some() {
            self.deferred_connect_stream_triggers
                .push(Message::TriggerConnectStreamReady {
                    network,
                    datadir,
                    tokens,
                    email,
                    cube_uuid,
                });
            return Task::none();
        }
        let current = match tokens.try_read() {
            Ok(current) => current,
            Err(_) => {
                let wait_tokens = tokens.clone();
                let retry_tokens = tokens.clone();
                return Task::perform(
                    async move {
                        drop(wait_tokens.read().await);
                        Message::TriggerConnectStreamReady {
                            network,
                            datadir,
                            tokens: retry_tokens,
                            email,
                            cube_uuid,
                        }
                    },
                    |message| message,
                );
            }
        };
        let network_dir = datadir.network_directory(network);
        let owns_cached_session =
            crate::services::connect::client::cache::ConnectCache::from_file(&network_dir)
                .ok()
                .and_then(|cache| cache.active_account().cloned())
                .is_some_and(|account| {
                    account.email == email
                        && account.tokens.access_token == current.access_token
                        && account.tokens.refresh_token == current.refresh_token
                });
        drop(current);
        if !owns_cached_session {
            return Task::none();
        }
        self.connect_auth = Some(tokens.clone());
        self.connect_email = Some(email.clone());
        self.cache.connect_tokens = Some(tokens.clone());
        self.cache.connect_email = Some(email.clone());
        self.cache.has_connect_session = true;
        connect_stream_ready_task(network, datadir, tokens, email, cube_uuid)
    }
}
