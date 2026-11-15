//! App-owned recovery context, consent and off-page result routing.
use super::*;

impl App {
    pub(super) fn route_recovery_kit(&mut self, msg: view::RecoveryKitMessage) -> Task<Message> {
        let seed_source = if self.cube_settings.is_passkey_cube() {
            crate::app::state::settings::recovery_kit::SeedSource::Passkey
        } else {
            crate::app::state::settings::recovery_kit::SeedSource::Mnemonic
        };
        let client = self.authenticated_coincube_client();
        let server_cube_id = self.panels.connect.cube.server_cube_id;
        let wallet = self
            .wallet
            .clone()
            .filter(|wallet| wallet.supports_inheritance());
        let local_cube_id = self.cube_settings.id.clone();
        let imported = self
            .cube_settings
            .bitcoin_import
            .as_deref()
            .and_then(|import| {
                let attachment = import.active_attachment()?;
                let daemon = self.panels.imported_bitcoin.as_ref()?.backup_daemon()?;
                Some(
                    crate::app::state::settings::recovery_kit::ImportedBackupSource {
                        import_operation_id: import.operation_id,
                        imported_claim_source: self.cube_settings.imported_claim_source.clone(),
                        signer_metadata: Some(
                            crate::services::recovery::imported::encrypted_signer_metadata(
                                &self.datadir.network_directory(self.cube_settings.network),
                                &attachment.wallet_id,
                            )
                            .ok()?,
                        ),
                        descriptor: attachment.descriptor.clone(),
                        name: self.cube_settings.name.clone(),
                        birthday: attachment.birthday,
                        initial_lookahead: attachment.initial_lookahead,
                        chain: self.cube_settings.network,
                        daemon,
                    },
                )
            });
        crate::app::state::settings::recovery_kit::update_with_imported(
            &mut self.panels.global_settings.recovery_kit,
            msg,
            &self.cache,
            &local_cube_id,
            seed_source,
            client,
            server_cube_id,
            wallet,
            imported,
        )
    }

    pub(super) fn route_recovery_alerts(
        &mut self,
        msg: view::RecoveryAlertsMessage,
    ) -> Task<Message> {
        let client = self.authenticated_coincube_client();
        let server_cube_id = self.panels.connect.cube.server_cube_id;
        let wallet = self.wallet.clone();
        let entitled = self.panels.connect.account.is_recovery_alerts_entitled();
        let escrow_entitled = self.panels.connect.account.is_inheritance_escrow_entitled();
        let session_generation = self.panels.connect.account.session_generation();
        let local_cube_id = self.cube_settings.id.clone();
        // A *successful, current-session* status load is the only message
        // that reveals an eligible Vault for the one-time prompt; a
        // *successful user change* means the owner has engaged with the
        // alerts decision, so the prompt must never re-fire for this Cube
        // (else turning alerts off would immediately re-nudge — the
        // residual nudge is the banner, not the modal). Classify before
        // `msg` is moved into `update`.
        //
        // Gate the load on `Ok` + a matching generation: a failed load
        // leaves the monitoring state unknown (or resets it to no-vault),
        // and `update` drops a stale-generation load without hydrating
        // anything (see its guard), so neither reveals a fresh eligible
        // state to prompt off.
        let is_status_load = matches!(
            msg,
            view::RecoveryAlertsMessage::StatusLoaded(Ok(_), gen)
                if gen == session_generation
        );
        // Only an *accepted* change counts as engagement. `update` drops
        // a `ChangeResult` whose captured generation no longer matches the
        // active session (see its stale-result guard), so gate on the same
        // match here — otherwise a stale success that never touched state
        // would still permanently mark the prompt answered.
        let is_ok_change = matches!(
            msg,
            view::RecoveryAlertsMessage::ChangeResult(Ok(_), gen, _)
                if gen == session_generation
        );
        let task = crate::app::state::settings::recovery_alerts::update(
            &mut self.panels.global_settings.recovery_alerts,
            msg,
            client,
            server_cube_id,
            wallet,
            entitled,
            escrow_entitled,
            session_generation,
            &self.cache,
            &local_cube_id,
        );
        // The owner engaged with the alerts decision via the card → mark
        // the prompt answered (durably) so the modal never fires for this
        // Cube again.
        let engaged = if is_ok_change && !self.cube_settings.recovery_alerts_prompt_answered {
            self.cube_settings.recovery_alerts_prompt_answered = true;
            self.persist_recovery_alerts_answered()
        } else {
            Task::none()
        };
        // Existing-Vault trigger (PR 3): a just-resolved `StatusLoaded`
        // may have revealed an eligible (keyholders, monitoring off,
        // unanswered) Vault. Offer the one-time consent prompt now.
        let prompt = if is_status_load {
            self.maybe_show_recovery_alerts_prompt()
        } else {
            Task::none()
        };
        Task::batch([task, engaged, prompt])
    }
}

impl App {
    /// Show the one-time recovery-alerts consent prompt (PR 3) when this device
    /// holds the Vault and that Vault has keyholders, a Connect session, and no
    /// monitoring yet — and the prompt hasn't already been answered for this
    /// Cube. A walletless instance (`App::new_without_wallet`) can still have a
    /// Connect cube with keyholders, so gate on the local wallet or it would
    /// consent on behalf of a Vault it doesn't hold. Idempotent within a
    /// session (the overlay flag guards re-entry); the durable answer lives in
    /// `CubeSettings::recovery_alerts_prompt_answered`.
    ///
    /// Only ever called off the back of a monitoring-status load, because that
    /// load is what resolves the recipient list this gates on. Calling it any
    /// earlier reads an empty list and silently declines to prompt — which is
    /// exactly how the prompt used to be unreachable in every build.
    ///
    /// The returned task clears keyboard focus when the prompt opens: the
    /// content beneath it stays mounted (so it keeps its scroll offsets and
    /// other widget state), and a control focused before the prompt appeared
    /// must not go on taking Enter or Space behind it.
    pub(super) fn maybe_show_recovery_alerts_prompt(&mut self) -> Task<Message> {
        if should_show_recovery_alerts_prompt(
            self.show_recovery_alerts_prompt,
            self.cube_settings.recovery_alerts_prompt_answered,
            self.panels.connect.account.is_authenticated(),
            self.panels.connect.cube.server_cube_id.is_some(),
            self.panels.connect.account.is_recovery_alerts_entitled(),
            !self
                .panels
                .global_settings
                .recovery_alerts
                .recipients
                .notified
                .is_empty(),
            self.panels.global_settings.recovery_alerts.alerts_on(),
            self.wallet
                .as_ref()
                .is_some_and(|wallet| wallet.supports_inheritance()),
        ) {
            self.show_recovery_alerts_prompt = true;
            return iced_runtime::task::widget(
                iced::advanced::widget::operation::focusable::unfocus(),
            );
        }
        Task::none()
    }

    /// Persist this Cube's "answered the consent prompt" flag to the settings
    /// file (PR 3). The in-memory `cube_settings` copy is set by the caller; this
    /// just makes it durable so the prompt never re-fires across restarts.
    pub(super) fn persist_recovery_alerts_answered(&self) -> Task<Message> {
        let network_dir = self
            .cache
            .datadir_path
            .network_directory(self.cache.chain());
        let cube_id = self.cube_settings.id.clone();
        Task::perform(
            async move {
                settings::update_settings_file(&network_dir, |mut s| {
                    if let Some(cube) = s.cubes.iter_mut().find(|c| c.id == cube_id) {
                        cube.recovery_alerts_prompt_answered = true;
                    }
                    Some(s)
                })
                .await
                .map_err(|e| {
                    crate::user_error::UserError::logged(
                        "Couldn't save that preference",
                        "Try again. If it keeps failing, contact support and quote the reference below.",
                        crate::user_error::CC_CONFIG,
                        true,
                        e,
                    )
                    .toast()
                })
            },
            |res: Result<(), String>| match res {
                Ok(()) => Message::SettingsSaved,
                Err(e) => Message::View(view::Message::ShowError(e)),
            },
        )
    }
    pub(super) fn handle_recovery_alerts_consent(&mut self, accept: bool) -> Task<Message> {
        // Answer the one-time consent prompt (PR 3). Dismiss the overlay
        // either way.
        self.show_recovery_alerts_prompt = false;
        if !accept {
            // Decline: record durably and move on — never re-prompt.
            self.cube_settings.recovery_alerts_prompt_answered = true;
            return self.persist_recovery_alerts_answered();
        }
        // Accept: turn alerts on now (cube-scoped POST). Route the result
        // through the settings card's `ChangeResult`, which reflects
        // alerts-on on the card AND — on `Ok` — marks + persists the prompt
        // answered (the shared `is_ok_change` handler below). So the answer
        // is recorded only after the enable actually succeeds: a
        // post-dispatch API failure surfaces there and leaves the prompt
        // un-answered, so it re-fires later. `enable_alerts` is idempotent,
        // so a redundant accept is safe.
        //
        // If the Connect session dropped while the overlay was up (token
        // expiry / account switch) there's nothing to dispatch: leave the
        // prompt un-answered (it re-fires once Connect is back) and surface
        // a visible error rather than failing silently.
        let gen = self.panels.connect.account.session_generation();
        match (
            self.authenticated_coincube_client(),
            self.panels.connect.cube.server_cube_id,
        ) {
            (Some(client), Some(cube_id)) => Task::perform(
                async move {
                    crate::services::inheritance::enable_alerts(&client, cube_id)
                        .await
                        .map_err(|e| e.to_string())
                },
                move |res| {
                    Message::View(view::Message::Settings(
                        view::SettingsMessage::RecoveryAlerts(
                            view::RecoveryAlertsMessage::ChangeResult(res, gen, None),
                        ),
                    ))
                },
            ),
            _ => Task::done(Message::View(view::Message::ShowError(
                "Couldn't reach your Connect account — recovery alerts weren't turned \
                     on. We'll ask again."
                    .to_string(),
            ))),
        }
    }
}
