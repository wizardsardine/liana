//! Connect stream health, panel routing and device registration renewal.
use super::*;

impl App {
    /// Register the current Connect device and bring its signing stream online.
    pub(super) fn ensure_connect_ready(&mut self) -> Task<Message> {
        // The user is signed in to Connect but the signing stream
        // isn't ready. Run the same device-registration + stream
        // bootstrap the in-app login flow does, sourcing tokens
        // from `connect.json`. The async block decides the next
        // message itself so we can give accurate feedback: some
        // networks (e.g. testnet4) have no `grpc_url` in their
        // ServiceConfig, meaning Connect/Keychain relay signing is
        // unavailable there no matter how often the user retries —
        // we say so and point them at local Wi-Fi pairing instead.
        let network = self.cache.network;
        let datadir = self.cache.datadir_path.clone();
        let expected_email = self
            .connect_email
            .clone()
            .or_else(|| self.cache.connect_email.clone());
        let shared_auth = if self.connect_email == expected_email {
            self.connect_auth.clone()
        } else {
            None
        };
        let cube_uuid = if self.cube_settings.id.is_empty() {
            None
        } else {
            Some(self.cube_settings.id.clone())
        };
        // The device/stream bootstrap below populates grpc_url /
        // tokens / device_id but NOT the cube's server-side id. When
        // that id is the missing Keychain prerequisite, register it
        // here too — otherwise "Sign with Connect" can never resolve
        // it. A live panel session registers through its authenticated
        // client; a restored connect.json (or remote) session whose
        // panel hasn't reached Dashboard has no panel client, so the
        // panel path would no-op — register directly from the restored
        // tokens (the same source the device bootstrap uses) and feed
        // the result back through the normal `CubeRegistered` path so
        // `current_cube_server_id` still populates.
        let cube_registration = if self.cache.current_cube_server_id.is_some() {
            Task::none()
        } else if self.panels.connect.account.is_authenticated() {
            self.panels.connect.ensure_cube_registered()
        } else if let Some(generation) = self.panels.connect.cube.begin_external_registration() {
            let datadir = datadir.clone();
            let net_str = settings::network_to_api_string(network);
            let cube_name = self.cube_settings.name.clone();
            // Report this Cube's Vault presence so other devices can
            // evaluate the duress vault gate (PLAN-duress-vault-gate
            // PR 3). Monotonic upgrade-only: report `true` only when
            // this device holds the Vault, else omit (never clobber a
            // `true` reported by another device). Captured before the
            // async move.
            let cube_has_vault = self.cube_settings.vault_wallet_id.is_some().then_some(true);
            // Same for this device's Spark Stable Balance decision:
            // `None` is omitted, so it never clobbers another device's.
            let cube_spark_stable_balance = self.cube_settings.spark_stable_balance;
            let cube_uuid = cube_uuid.clone();
            let registration_email = expected_email.clone();
            Task::perform(
                async move {
                    use crate::services::coincube::{CoincubeClient, RegisterCubeRequest};
                    use crate::services::connect::client::cache::Account;
                    let uuid = cube_uuid.ok_or_else(|| "no cube uuid".to_string())?;
                    let email = registration_email.ok_or_else(|| {
                        "EnsureConnectReady: no active Connect email to register the cube"
                            .to_string()
                    })?;
                    let account = Account::from_cache(&datadir.network_directory(network), &email)
                        .ok()
                        .flatten()
                        .ok_or_else(|| {
                            "EnsureConnectReady: no cached Connect session to \
                                 register the cube"
                                .to_string()
                        })?;
                    let mut client = CoincubeClient::new();
                    client.set_token(&account.tokens.access_token);
                    client
                        .register_cube(RegisterCubeRequest {
                            uuid,
                            name: cube_name,
                            network: net_str,
                            has_vault: cube_has_vault,
                            spark_stable_balance: cube_spark_stable_balance,
                        }) // upgrade-only Option<bool>
                        .await
                        .map_err(|e| {
                            crate::app::state::connect::cube::CubeRegistrationError::from(&e)
                        })
                },
                move |r| {
                    Message::View(view::Message::ConnectCube(
                        view::ConnectCubeMessage::CubeRegistered {
                            generation,
                            result: r,
                        },
                    ))
                },
            )
        } else {
            // A registration is already in flight — claimed either by
            // this branch on an earlier press or by the panel. Skip the
            // duplicate; the bootstrap below still runs, which is the
            // part a second press is actually asking for.
            Task::none()
        };
        let bootstrap = Task::perform(
            async move {
                use crate::services::connect::client::cache::Account;
                use crate::services::connect::grpc::bootstrap::ensure_device_registered_best_effort;

                let info_toast = |msg: &str| {
                    Message::View(view::Message::ShowToast(log::Level::Info, msg.to_string()))
                };

                let network_dir = datadir.network_directory(network);
                let Some(expected_email) = expected_email else {
                    tracing::warn!(
                        "EnsureConnectReady: no active Connect email to match connect.json"
                    );
                    return info_toast(
                        "Couldn't find a Connect session. Sign in to Connect, or pair \
                         a phone over Wi-Fi to sign locally.",
                    );
                };
                let Some(account) = Account::from_cache(&network_dir, &expected_email)
                    .ok()
                    .flatten()
                else {
                    tracing::warn!(
                        "EnsureConnectReady: no cached Connect account for active email"
                    );
                    return info_toast(
                        "Couldn't find this Connect session. Sign in to Connect, or pair \
                         a phone over Wi-Fi to sign locally.",
                    );
                };
                let email = account.email.clone();
                let tokens_arc =
                    connect_registration_auth(shared_auth, account.tokens, &network_dir, &email)
                        .await;

                let Some(grpc_url) =
                    crate::services::connect::client::resolve_connect_grpc_url().await
                else {
                    // No gRPC endpoint for this network — Connect
                    // relay signing can't work here. Don't pretend
                    // a retry will help.
                    tracing::info!(
                        "EnsureConnectReady: no Connect gRPC URL available for {network}"
                    );
                    return info_toast(
                        "Mobile signing over Connect isn't available on this network. \
                         Pair a phone over Wi-Fi to sign locally instead.",
                    );
                };

                // Same helper as every other registration path — see
                // the note on the in-app login path above.
                let device_name = crate::utils::device::device_label();
                let Some(_device_id) = ensure_device_registered_best_effort(
                    &grpc_url,
                    tokens_arc.clone(),
                    &network_dir,
                    &email,
                    device_name,
                    env!("CARGO_PKG_VERSION").to_string(),
                    std::env::consts::OS.to_string(),
                )
                .await
                else {
                    return info_toast(
                        "Couldn't register this device with Connect. Try again, or pair a \
                         phone over Wi-Fi to sign locally.",
                    );
                };

                // gRPC is available and the device is registered —
                // bring the stream up so the cache fields populate,
                // then the user can retry Sign via Keychain.
                Message::TriggerConnectStreamReady {
                    network,
                    datadir,
                    tokens: tokens_arc,
                    email,
                    cube_uuid,
                }
            },
            |m| m,
        );
        Task::batch([cube_registration, bootstrap])
    }

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
            M::SessionEvent(session_event) => self.handle_connect_session_event(session_event),
            M::DuressActivated { unlock_at, source } => {
                self.handle_connect_duress_activated(unlock_at, source)
            }
            M::DuressCleared => self.handle_connect_duress_cleared(),
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

    fn handle_connect_session_event(
        &self,
        session_event: crate::services::connect::grpc::connect_v1::SessionEvent,
    ) -> Task<Message> {
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

    fn handle_connect_duress_activated(
        &self,
        unlock_at: Option<chrono::DateTime<chrono::Utc>>,
        source: String,
    ) -> Task<Message> {
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

    fn handle_connect_duress_cleared(&self) -> Task<Message> {
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
