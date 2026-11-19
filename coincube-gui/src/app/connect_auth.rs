//! In-app Connect authentication completion and cache-commit lifecycle.
use super::*;
use crate::{
    dir::NetworkDirectory,
    services::connect::client::{auth::AccessTokenResponse, cache},
};

impl App {
    pub(super) fn connect_auth_ready(
        &mut self,
        login: crate::services::coincube::LoginResponse,
        epoch: u64,
        previous_refresh_token: Option<String>,
    ) -> Task<Message> {
        if epoch != self.panels.connect.account.auth_epoch() {
            return Task::none();
        }
        if previous_refresh_token.is_some() && self.in_app_refresh_commit.is_some() {
            return Task::none();
        }
        let restore_snapshot = if previous_refresh_token.is_some() {
            self.in_app_restore_cache_snapshot.take()
        } else {
            None
        };
        if !self.connect_restore_can_commit(
            restore_snapshot.as_ref(),
            previous_refresh_token.as_deref(),
            &login.user.email,
        ) {
            return Task::done(Message::View(view::Message::ConnectAccount(
                view::ConnectAccountMessage::AuthCommitRejected { epoch },
            )));
        }
        let snapshot = if previous_refresh_token.is_none() {
            self.in_app_otp_cache_snapshot.take()
        } else {
            None
        };
        let current_auth = previous_refresh_token.as_ref().and_then(|_| {
            (self.connect_email.as_deref() == Some(login.user.email.as_str()))
                .then(|| self.connect_auth.clone())
                .flatten()
        });
        let datadir = self.cache.datadir_path.clone();
        let network = self.cache.network;
        if previous_refresh_token.is_some() {
            self.in_app_refresh_commit = Some((epoch, self.connect_auth.clone()));
        }
        Task::perform(
            async move {
                let tokens = AccessTokenResponse {
                    access_token: login.token.clone(),
                    refresh_token: login.refresh_token.clone(),
                    expires_at: chrono::Utc::now().timestamp() + 30 * 24 * 60 * 60,
                };
                let network_dir = datadir.network_directory(network);
                let persisted =
                    if let Err(error) = tokio::fs::create_dir_all(network_dir.path()).await {
                        Err(cache::ConnectCacheError::WritingFile(error.to_string()))
                    } else if let Some(previous) = previous_refresh_token.as_deref() {
                        persist_in_app_refresh(
                            &network_dir,
                            &login.user.email,
                            &tokens,
                            previous,
                            current_auth,
                            restore_snapshot.as_ref(),
                        )
                        .await
                    } else if let Some(snapshot) = snapshot.as_ref() {
                        cache::update_connect_cache_for_login(
                            &network_dir,
                            &tokens,
                            &login.user.email,
                            snapshot,
                        )
                        .await
                    } else {
                        Err(cache::ConnectCacheError::Unexpected(
                            "OTP cache snapshot unavailable".into(),
                        ))
                    };
                if let Err(error) = &persisted {
                    tracing::warn!("In-app Connect auth cache commit rejected: {error}");
                }
                Message::InAppConnectAuthPersisted {
                    login,
                    epoch,
                    previous_refresh_token,
                    tokens: persisted.ok(),
                }
            },
            |message| message,
        )
    }

    fn connect_restore_can_commit(
        &self,
        restore_snapshot: Option<&RestoreAuthSnapshot>,
        previous_refresh_token: Option<&str>,
        email: &str,
    ) -> bool {
        if let Some(snapshot) = restore_snapshot {
            let same_arc = match (&snapshot.auth, &self.connect_auth) {
                (Some((captured, _)), Some(current)) => Arc::ptr_eq(captured, current),
                (None, None) => true,
                _ => false,
            };
            if snapshot.email != self.connect_email || !same_arc {
                return false;
            }
        }
        if previous_refresh_token.is_some()
            && restore_snapshot.is_none()
            && self
                .connect_email
                .as_deref()
                .is_some_and(|current_email| current_email != email)
        {
            return false;
        }
        true
    }

    pub(super) fn connect_auth_persisted(
        &mut self,
        login: crate::services::coincube::LoginResponse,
        epoch: u64,
        previous_refresh_token: Option<String>,
        tokens: Option<crate::services::connect::client::auth::AccessTokenResponse>,
    ) -> Task<Message> {
        let pending = self
            .in_app_refresh_commit
            .as_ref()
            .filter(|(id, _)| *id == epoch);
        let same_arc = pending.is_some_and(|(_, auth)| match (auth, &self.connect_auth) {
            (Some(captured), Some(current)) => Arc::ptr_eq(captured, current),
            (None, None) => true,
            _ => false,
        });
        let deferred = if pending.is_some() {
            self.in_app_refresh_commit = None;
            std::mem::take(&mut self.deferred_connect_stream_triggers)
        } else {
            Vec::new()
        };
        let next = if epoch != self.panels.connect.account.auth_epoch() {
            Task::none()
        } else if tokens.is_none() || (previous_refresh_token.is_some() && !same_arc) {
            Task::done(Message::View(view::Message::ConnectAccount(
                view::ConnectAccountMessage::AuthCommitRejected { epoch },
            )))
        } else {
            Task::done(Message::View(view::Message::ConnectAccount(
                view::ConnectAccountMessage::AuthCommitAccepted {
                    login,
                    epoch,
                    refreshed: previous_refresh_token.is_some(),
                },
            )))
        };
        Task::batch(std::iter::once(next).chain(deferred.into_iter().map(Task::done)))
    }

    pub(super) fn connect_login_completed(
        &mut self,
        token: String,
        refresh_token: String,
        email: String,
        epoch: u64,
    ) -> Task<Message> {
        if epoch != self.panels.connect.account.auth_epoch()
            || self.panels.connect.account.client.token() != Some(token.as_str())
        {
            return Task::none();
        }
        // The OTP or keyring-restore cache commit happened before
        // panel/keyring adoption. Only device registration and stream
        // bootstrap remain, both bound to that accepted token.
        let shared_auth = if self.connect_email.as_deref() == Some(email.as_str()) {
            self.connect_auth.clone()
        } else {
            None
        };
        self.connect_email = Some(email.clone());
        self.cache.connect_email = Some(email.clone());
        self.cache.has_connect_session = true;

        let network = self.cache.network;
        let datadir = self.cache.datadir_path.clone();
        // Use the authoritative Cube settings id. The cache mirror can
        // still be empty during local-vault startup.
        let cube_uuid = if self.cube_settings.id.is_empty() {
            None
        } else {
            Some(self.cube_settings.id.clone())
        };
        let email_for_task = email.clone();

        Task::perform(
            async move {
                use crate::services::connect::client::auth::AccessTokenResponse;
                use crate::services::connect::grpc::bootstrap::ensure_device_registered_best_effort;

                // Coincube backend issues 30-day JWTs (see
                // `CLAUDE.md`); we approximate expires_at as
                // now + 30d so the AccessTokenResponse shape
                // matches what the home path produces.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let tokens = AccessTokenResponse {
                    access_token: token,
                    refresh_token,
                    expires_at: now + 30 * 24 * 60 * 60,
                };

                let network_dir = datadir.network_directory(network);
                let owns_cached_session =
                    crate::services::connect::client::cache::ConnectCache::from_file(&network_dir)
                        .ok()
                        .and_then(|cache| cache.active_account().cloned())
                        .is_some_and(|account| {
                            account.email == email_for_task
                                && account.tokens.refresh_token == tokens.refresh_token
                        });
                if !owns_cached_session {
                    return (network, datadir, None, email_for_task, cube_uuid);
                }

                // Fetch grpc_url and register the device.
                let tokens_arc =
                    connect_registration_auth(shared_auth, tokens, &network_dir, &email_for_task)
                        .await;
                if let Some(grpc_url) =
                    crate::services::connect::client::resolve_connect_grpc_url().await
                {
                    // Same helper as every other registration path.
                    // `ensure_device_registered` short-circuits on a
                    // cached device_id, so whichever path registers
                    // first owns this machine's name permanently —
                    // all four call sites must derive it identically.
                    let device_name = crate::utils::device::device_label();
                    ensure_device_registered_best_effort(
                        &grpc_url,
                        tokens_arc.clone(),
                        &network_dir,
                        &email_for_task,
                        device_name,
                        env!("CARGO_PKG_VERSION").to_string(),
                        std::env::consts::OS.to_string(),
                    )
                    .await;
                } else {
                    tracing::warn!("InAppConnectLoginCompleted: no Connect gRPC URL available");
                }

                (
                    network,
                    datadir,
                    Some(tokens_arc),
                    email_for_task,
                    cube_uuid,
                )
            },
            |(network, datadir, tokens_opt, email, cube_uuid)| {
                // Chain the existing stream bootstrap so the
                // cache fields populate without an app restart.
                match tokens_opt {
                    Some(tokens) => Message::TriggerConnectStreamReady {
                        network,
                        datadir,
                        tokens,
                        email,
                        cube_uuid,
                    },
                    None => Message::ConnectStreamReady(None),
                }
            },
        )
    }
}

async fn persist_in_app_refresh(
    network_dir: &NetworkDirectory,
    email: &str,
    tokens: &AccessTokenResponse,
    previous: &str,
    current_auth: Option<Arc<tokio::sync::RwLock<AccessTokenResponse>>>,
    restore_snapshot: Option<&RestoreAuthSnapshot>,
) -> Result<AccessTokenResponse, cache::ConnectCacheError> {
    if let Some(current_auth) = current_auth {
        let mut current = current_auth.write().await;
        let unchanged = restore_snapshot.map_or_else(
            || current.refresh_token == previous,
            |snapshot| {
                snapshot.auth.as_ref().is_some_and(|(_, captured)| {
                    current.access_token == captured.access_token
                        && current.refresh_token == captured.refresh_token
                        && current.expires_at == captured.expires_at
                })
            },
        );
        if !unchanged {
            Err(cache::ConnectCacheError::Unexpected(
                "Connect refresh was superseded in memory".into(),
            ))
        } else {
            let result = persist_in_app_refresh_tokens(
                network_dir,
                email,
                tokens,
                previous,
                restore_snapshot,
            )
            .await;
            if let Ok(accepted) = &result {
                *current = accepted.clone();
            }
            result
        }
    } else {
        persist_in_app_refresh_tokens(network_dir, email, tokens, previous, restore_snapshot).await
    }
}

async fn persist_in_app_refresh_tokens(
    network_dir: &NetworkDirectory,
    email: &str,
    tokens: &AccessTokenResponse,
    previous: &str,
    restore_snapshot: Option<&RestoreAuthSnapshot>,
) -> Result<AccessTokenResponse, cache::ConnectCacheError> {
    if let Some(snapshot) = restore_snapshot {
        cache::persist_restored_tokens(network_dir, email, tokens, &snapshot.cache).await
    } else {
        cache::persist_refreshed_tokens(network_dir, email, previous, tokens).await
    }
}
