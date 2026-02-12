use super::{app::AppState, message::Msg, views, State, View};
use crate::{
    backend::{Backend, Error, Notification},
    client::{ws_url, PROTOCOL_VERSION},
    state::views::modals::{ConflictModalState, ConflictType},
    state::views::registration::RegistrationModalStep,
};
use iced::Task;
use liana_connect::ws_business::{
    self, Key, KeyIdentity, PolicyTemplate, SecondaryPath, SpendingPath, Timelock, UserRole,
    Wallet, WalletStatus, BLOCKS_PER_DAY,
};
use liana_ui::widget::text_input;
use miniscript::bitcoin::bip32::Fingerprint;
use tracing::{debug, error, trace};
use uuid::Uuid;

// Update routing logic
impl State {
    #[rustfmt::skip]
    pub fn update(&mut self, message: Msg) -> Task<Msg> {
        if !matches!(message, Msg::Update) {
            debug!("received message");
        } else {
            trace!("received message");
        }
        match message {
            // Login/Auth
            Msg::LoginUpdateEmail(email) => self.views.login.on_update_email(email),
            Msg::LoginUpdateCode(code) => self.on_login_update_code(code),
            Msg::LoginSendToken => self.on_login_send_token(),
            Msg::LoginResendToken => self.on_login_resend_token(),
            Msg::LoginSendAuthCode => self.on_login_send_auth_code(),

            // Account selection (cached token login)
            Msg::AccountSelectConnect(email) => return self.on_account_select_connect(email),
            Msg::AccountSelectDelete(email) => self.on_account_select_delete(email),
            Msg::AccountSelectNewEmail => return self.on_account_select_new_email(),

            // Org management
            Msg::OrgSelected(id) => self.on_org_selected(id),
            Msg::OrgWalletSelected(id) => return self.on_org_wallet_selected(id),

            // Wallet selection
            Msg::WalletSelectUpdateSearchFilter(filter) => {
                self.views.wallet_select.search_filter = filter;
            }

            // Organization selection
            Msg::OrgSelectUpdateSearchFilter(filter) => {
                self.views.org_select.search_filter = filter;
            }

            // Keys management
            Msg::KeyCancelModal => self.views.keys.on_key_cancel_modal(),
            Msg::KeyUpdateAlias(value) => self.views.keys.on_key_update_alias(value),
            Msg::KeyUpdateDescr(value) => self.views.keys.on_key_update_descr(value),
            Msg::KeyUpdateEmail(value) => self.views.keys.on_key_update_email(value),
            Msg::KeyUpdateType(key_type) => self.views.keys.on_key_update_type(key_type),
            Msg::KeyAdd => self.on_key_add(),
            Msg::KeyEdit(key_id) => self.on_key_edit(key_id),
            Msg::KeyDelete(key_id) => self.on_key_delete(key_id),
            Msg::KeySave => self.on_key_save(),

            // Template management
            Msg::TemplateCancelPathModal => self.views.paths.on_template_cancel_path_modal(),
            Msg::TemplateUpdateThreshold(v) => self.views.paths.on_template_update_threshold(v),
            Msg::TemplateUpdateTimelock(v) => self.views.paths.on_template_update_timelock(v),
            Msg::TemplateUpdateTimelockUnit(u) => self.views.paths.on_template_update_timelock_unit(u),
            Msg::TemplateToggleKeyInPath(id) => self.views.paths.on_template_toggle_key_in_path(id),
            Msg::TemplateAddKeyToPrimary(id) => self.on_template_add_key_to_primary(id),
            Msg::TemplateDelKeyFromPrimary(id) => self.on_template_del_key_from_primary(id),
            Msg::TemplateAddKeyToSecondary(i, id) => self.on_template_add_key_to_secondary(i, id),
            Msg::TemplateDelKeyFromSecondary(i, id) => self.on_template_del_key_from_secondary(i, id),
            Msg::TemplateAddSecondaryPath => self.on_template_add_secondary_path(),
            Msg::TemplateDeleteSecondaryPath(i) => self.on_template_delete_secondary_path(i),
            Msg::TemplateEditPath(primary, i) => self.on_template_edit_path(primary, i),
            Msg::TemplateNewPathModal => self.on_template_new_path_modal(),
            Msg::TemplateSavePath => self.on_template_save_path(),
            Msg::TemplateLock => self.on_template_lock(),
            Msg::TemplateUnlock => self.on_template_unlock(),
            Msg::TemplateValidate => self.on_template_validate(),

            // Navigation
            Msg::NavigateToHome => self.on_navigate_to_home(),
            Msg::NavigateToKeys => self.on_navigate_to_keys(),
            Msg::NavigateToOrgSelect => self.on_navigate_to_org_select(),
            Msg::NavigateToWalletSelect => self.on_navigate_to_wallet_select(),
            Msg::NavigateBack => return self.on_navigate_back(),

            // Backend
            Msg::BackendNotif(notif) => return self.on_backend_notif(notif),
            Msg::BackendDisconnected => self.on_backend_disconnected(),

            // Hardware Wallets
            Msg::HardwareWallets(msg) => return self.on_hw_message(msg),

            // Xpub management
            Msg::XpubSelectKey(key_id) => self.on_xpub_select_key(key_id),
            Msg::XpubUpdateInput(input) => self.on_xpub_update_input(input),
            Msg::XpubSelectDevice(fingerprint) => return self.on_xpub_select_device(fingerprint),
            Msg::XpubDeviceBack => self.on_xpub_device_back(),
            Msg::XpubFetchFromDevice(fingerprint, account) => return self.on_xpub_fetch_from_device(fingerprint, account),
            Msg::XpubRetry => return self.on_xpub_retry(),
            Msg::XpubLoadFromFile => return self.on_xpub_load_from_file(),
            Msg::XpubFileLoaded(result) => self.on_xpub_file_loaded(result),
            Msg::XpubPaste => return self.on_xpub_paste(),
            Msg::XpubPasted(xpub) => self.on_xpub_pasted(xpub),
            Msg::XpubUpdateAccount(account) => return self.on_xpub_update_account(account),
            Msg::XpubSave => return self.on_xpub_save(),
            Msg::XpubClear => return self.on_xpub_clear(),
            Msg::XpubCancelModal => self.on_xpub_cancel_modal(),
            Msg::XpubToggleOptions => self.on_xpub_toggle_options(),

            // Registration
            Msg::RegistrationSelectDevice(fingerprint) => return self.on_registration_select_device(fingerprint),
            Msg::RegistrationResult(result) => return self.on_registration_result(result),
            Msg::RegistrationCancelModal => self.on_registration_cancel_modal(),
            Msg::RegistrationRetry => return self.on_registration_retry(),
            Msg::RegistrationConfirmYes => return self.on_registration_confirm_yes(),
            Msg::RegistrationConfirmNo => self.on_registration_confirm_no(),
            Msg::RegistrationSkip(fingerprint) => return self.on_registration_skip(fingerprint),
            Msg::RegistrationSkipAll => return self.on_registration_skip_all(),

            // Warnings
            Msg::WarningShowModal(title, message) => self.on_warning_show_modal(title, message),
            Msg::WarningCloseModal => self.on_warning_close_modal(),

            // Conflict resolution
            Msg::ConflictReload => return self.on_conflict_reload(),
            Msg::ConflictKeepLocal => self.on_conflict_keep_local(),
            Msg::ConflictDismiss => self.on_conflict_dismiss(),

            // Logout
            Msg::Logout => return self.on_logout(),

            // No-op: just triggers a view refresh
            Msg::Update => {}
        }
        Task::none()
    }

    #[rustfmt::skip]
    fn on_backend_notif(&mut self, response: Notification) -> Task<Msg> {
        match response {
            Notification::Connected => self.on_backend_connected(),
            Notification::Disconnected => self.on_backend_disconnected(),
            Notification::AuthCodeSent => return self.on_backend_auth_code_sent(),
            Notification::InvalidEmail => self.on_backend_invalid_email(),
            Notification::AuthCodeFail => self.on_backend_auth_code_fail(),
            Notification::LoginSuccess => self.on_backend_login_success(),
            Notification::LoginFail => self.on_backend_login_fail(),
            Notification::Error(error) => self.on_backend_error(error),
            Notification::Org(_) => { /* Cache already updated, no action needed */ }
            Notification::Wallet(wallet_id) => return self.on_backend_wallet(wallet_id),
            Notification::User(user_id) => {
                // Check if this user matches the logged-in user's email
                // If so, set their global role
                let logged_in_email = self.views.login.email.form.value.to_lowercase();
                if let Some(user) = self.backend.get_user(user_id) {
                    if user.email.to_lowercase() == logged_in_email {
                        self.app.global_user_role = Some(user.role);
                    }
                }
            }
            Notification::Update => { /* Update view */ }
        }
        Task::none()
    }
}

// Login/Auth
impl State {
    fn on_login_update_code(&mut self, code: String) {
        self.views.login.on_update_code(code);
        if self.views.login.code.can_send() {
            let code = self.views.login.code.form.value.clone();
            self.backend.auth_code(code);
            self.views.login.code.processing = true;
        }
    }
    fn on_login_send_token(&mut self) {
        let email = self.views.login.email.form.value.clone();
        tracing::debug!(
            "on_login_send_token: email={} valid={}",
            email,
            self.views.login.email.form.valid
        );
        if self.views.login.email.form.valid && !email.is_empty() {
            tracing::debug!("on_login_send_token: calling auth_request");
            self.views.login.email.processing = true;
            self.backend.auth_request(email);
        } else {
            tracing::debug!("on_login_send_token: skipped - invalid or empty");
        }
    }
    fn on_login_resend_token(&mut self) {
        // FIXME: should we "rate limit" here or only on server?
        self.on_login_send_token();
    }
    fn on_login_send_auth_code(&mut self) {
        // Trim the code value before sending to backend
        let code = self.views.login.code.form.value.trim().to_string();
        self.backend.auth_code(code);
    }
}

// Account selection (cached token login)
impl State {
    /// Connect with a cached account's token
    fn on_account_select_connect(&mut self, email: String) -> Task<Msg> {
        // Find token for this email
        let token = self
            .views
            .login
            .account_select
            .accounts
            .iter()
            .find(|a| a.email == email)
            .map(|a| a.tokens.access_token.clone());

        if let Some(token) = token {
            // Set processing state
            self.views.login.account_select.processing = true;
            self.views.login.account_select.selected_email = Some(email.clone());

            // Store email for later use (e.g., in exit_maybe)
            self.views.login.email.form.value = email;

            // Set token and connect
            self.backend.set_token(token);
            self.backend.connect_ws(
                ws_url(self.network),
                PROTOCOL_VERSION,
                self.notif_sender.clone(),
            );
        }

        Task::none()
    }

    /// User wants to login with a new email (start fresh auth flow)
    fn on_account_select_new_email(&mut self) -> Task<Msg> {
        self.views.login.current = views::LoginState::EmailEntry;
        self.views.login.email.form.value.clear();
        self.views.login.email.form.valid = false;
        self.views.login.email.form.warning = None;
        text_input::focus("login_email")
    }

    /// Delete a cached account (remove from UI and cache)
    fn on_account_select_delete(&mut self, email: String) {
        self.backend.clear_invalid_tokens(&[email.clone()]);
        self.views
            .login
            .account_select
            .accounts
            .retain(|a| a.email != email);
        if self.views.login.account_select.accounts.is_empty() {
            self.views.login.current = views::LoginState::EmailEntry;
        }
    }
}

// Org management
impl State {
    fn on_org_selected(&mut self, id: Uuid) {
        if self.backend.get_org(id).is_some() {
            self.app.selected_org = Some(id);
            self.current_view = View::WalletSelect;
        }
    }
    fn on_org_wallet_selected(&mut self, wallet_id: Uuid) -> Task<Msg> {
        // Extract user_id first to avoid borrow conflict with mutex guard
        let user_id = *self.backend.user_id.lock().expect("poisoned");
        let user = match user_id {
            Some(id) => self.backend.get_user(id),
            None => {
                tracing::error!(
                    "BUG: State::on_org_wallet_selected() selected but user_id unknown."
                );
                self.on_warning_show_modal(
                    "Error",
                    "User session not found. Please log in again or contact WizardSardine",
                );
                return Task::none();
            }
        };
        // Get wallet and check access before loading
        let user_email = self.views.login.email.form.value.clone();
        let (wallet_status, user_role) = {
            if let (Some(wallet), Some(user)) = (self.backend.get_wallet(wallet_id), user) {
                let role = user.role(&wallet);
                (Some(wallet.effective_status(&user_email)), role)
            } else {
                (None, None)
            }
        };

        // Log error if user has no role in this wallet
        if user_role.is_none() {
            tracing::error!(
                "User has no role in wallet {}. Access may be restricted.",
                wallet_id
            );
            self.on_warning_show_modal(
                "Access Error",
                "You do not have access to this wallet. Contact WizardSardine.",
            );
            return Task::none();
        }

        // Store user role for the selected wallet
        self.app.current_user_role = user_role;

        // Load wallet template into AppState
        self.app.selected_wallet = Some(wallet_id);
        if let Some(wallet) = self.backend.get_wallet(wallet_id) {
            let wallet_template = wallet.template.clone().unwrap_or(PolicyTemplate::new());
            self.app.current_wallet_template = Some(wallet_template.clone());
            // Convert template to AppState
            let app_state: AppState = wallet_template.clone().into();
            self.app.keys = app_state.keys;
            self.app.primary_path = app_state.primary_path;
            self.app.secondary_paths = app_state.secondary_paths;
            self.app.next_key_id = app_state.next_key_id;
        }

        // Check if wallet is in registration
        if let Some(wallet) = self.backend.get_wallet(wallet_id) {
            if wallet.effective_status(&user_email) == WalletStatus::Registration {
                // Registration -> Device Registration view
                self.current_view = View::Registration;

                // Start hardware wallet listening
                self.start_hw();

                // Populate registration state from wallet data
                self.views.registration.descriptor = wallet.descriptor.clone();
                self.views.registration.user_devices = wallet.user_devices(&user_email);
                return Task::none();
            }
        }

        // Route based on wallet status
        match wallet_status {
            Some(WalletStatus::Registration) => {
                // Registration handled above with early return
                unreachable!();
            }
            Some(WalletStatus::Validated) => {
                // Validated -> Add Key Information (xpub entry)
                self.current_view = View::Xpub;
            }
            Some(WalletStatus::Finalized) => {
                // Final -> Signal exit to Wallet GUI
                // Return Task::done to generate a follow-up message that triggers exit_maybe
                self.app.exit = true;
                return Task::done(Msg::Update);
            }
            Some(WalletStatus::Created)
            | Some(WalletStatus::Drafted)
            | Some(WalletStatus::Locked) => {
                // Draft/Locked + (WS Admin|Wallet Manager) -> Edit Template
                self.current_view = View::WalletEdit;
            }
            None => {
                // Fallback -> Edit Template
                self.current_view = View::WalletEdit;
            }
        }
        Task::none()
    }
}

// Key management
impl State {
    fn on_key_add(&mut self) {
        // Open modal for creating a new key
        let key_id = self.app.next_key_id;
        self.views.keys.edit_key_modal = Some(views::EditKeyModalState {
            key_id,
            alias: String::new(),
            description: String::new(),
            email: String::new(),
            key_type: ws_business::KeyType::Internal,
            is_new: true,
        });
    }

    fn on_key_edit(&mut self, key_id: u8) {
        if let Some(key) = self.app.keys.get(&key_id) {
            // Extract email from KeyIdentity
            let email = key.identity.to_string();
            self.views.keys.edit_key_modal = Some(views::EditKeyModalState {
                key_id,
                alias: key.alias.clone(),
                description: key.description.clone(),
                email,
                key_type: key.key_type,
                is_new: false,
            });
        }
    }

    fn on_key_delete(&mut self, key_id: u8) {
        // Remove key from all paths first
        self.app.primary_path.key_ids.retain(|&id| id != key_id);
        for secondary in &mut self.app.secondary_paths {
            secondary.path.key_ids.retain(|&id| id != key_id);
        }
        // Then remove the key itself
        self.app.keys.remove(&key_id);
        // Close modal if it was open for this key
        if let Some(modal_state) = &self.views.keys.edit_key_modal {
            if modal_state.key_id == key_id {
                self.views.keys.edit_key_modal = None;
            }
        }

        // Auto-save
        if matches!(
            self.app.current_user_role,
            Some(UserRole::WizardSardineAdmin) | Some(UserRole::WalletManager)
        ) {
            if let Some(wallet) = self.build_wallet_from_app_state(WalletStatus::Drafted) {
                self.backend.edit_wallet(wallet);
            }
        }
    }

    fn on_key_save(&mut self) {
        if let Some(modal_state) = &self.views.keys.edit_key_modal {
            if modal_state.is_new {
                // Creating a new key
                let key = Key {
                    id: modal_state.key_id,
                    alias: modal_state.alias.clone(),
                    description: modal_state.description.clone(),
                    identity: KeyIdentity::Email(modal_state.email.clone()),
                    key_type: modal_state.key_type,
                    xpub: None,
                    xpub_source: None,
                    xpub_device_kind: None,
                    xpub_device_version: None,
                    xpub_file_name: None,
                    last_edited: None,
                    last_editor: None,
                };
                self.app.keys.insert(modal_state.key_id, key);
                self.app.next_key_id = self.app.next_key_id.wrapping_add(1);
            } else {
                // Editing existing key
                if let Some(key) = self.app.keys.get_mut(&modal_state.key_id) {
                    key.alias = modal_state.alias.clone();
                    key.description = modal_state.description.clone();
                    key.identity = KeyIdentity::Email(modal_state.email.clone());
                    key.key_type = modal_state.key_type;
                }
            }
            self.views.keys.edit_key_modal = None;

            // Auto-save
            if matches!(
                self.app.current_user_role,
                Some(UserRole::WizardSardineAdmin) | Some(UserRole::WalletManager)
            ) {
                if let Some(wallet) = self.build_wallet_from_app_state(WalletStatus::Drafted) {
                    self.backend.edit_wallet(wallet);
                }
            }
        }
    }
}

// Template management
impl State {
    /// Build a Wallet from current AppState with the specified status.
    /// Returns None if wallet cannot be found or built.
    fn build_wallet_from_app_state(&self, status: WalletStatus) -> Option<Wallet> {
        let wallet_id = self.app.selected_wallet?;
        let wallet = self.backend.get_wallet(wallet_id)?;

        // Build template from AppState
        let template = PolicyTemplate {
            keys: self.app.keys.clone(),
            primary_path: self.app.primary_path.clone(),
            secondary_paths: self.app.secondary_paths.clone(),
        };

        Some(Wallet {
            id: wallet.id,
            alias: wallet.alias.clone(),
            org: wallet.org,
            owner: wallet.owner,
            status,
            template: Some(template),
            last_edited: None,
            last_editor: None,
            descriptor: wallet.descriptor.clone(),
            devices: wallet.devices.clone(),
        })
    }
    fn on_template_add_key_to_primary(&mut self, key_id: u8) {
        if !self.app.primary_path.contains_key(key_id) {
            self.app.primary_path.key_ids.push(key_id);
        }
    }

    fn on_template_del_key_from_primary(&mut self, key_id: u8) {
        self.app.primary_path.key_ids.retain(|&id| id != key_id);
        // Adjust threshold_n if needed
        let m = self.app.primary_path.key_ids.len();
        if self.app.primary_path.threshold_n as usize > m && m > 0 {
            self.app.primary_path.threshold_n = m as u8;
        }
    }

    fn on_template_add_key_to_secondary(&mut self, path_index: usize, key_id: u8) {
        if let Some(secondary) = self.app.secondary_paths.get_mut(path_index) {
            if !secondary.path.contains_key(key_id) {
                secondary.path.key_ids.push(key_id);
            }
        }
    }

    fn on_template_del_key_from_secondary(&mut self, path_index: usize, key_id: u8) {
        if let Some(secondary) = self.app.secondary_paths.get_mut(path_index) {
            secondary.path.key_ids.retain(|&id| id != key_id);
            // Adjust threshold_n if needed
            let m = secondary.path.key_ids.len();
            if secondary.path.threshold_n as usize > m && m > 0 {
                secondary.path.threshold_n = m as u8;
            }
        }
    }

    fn on_template_add_secondary_path(&mut self) {
        // Create a new secondary path with default values
        // threshold_n defaults to 1, timelock defaults to 0 blocks (must be set later)
        let path = SpendingPath::new(false, 1, Vec::new());
        let timelock = Timelock::new(0);
        self.app
            .secondary_paths
            .push(SecondaryPath { path, timelock });
        self.app.sort_secondary_paths();
    }

    fn on_template_delete_secondary_path(&mut self, path_index: usize) {
        if path_index < self.app.secondary_paths.len() {
            self.app.secondary_paths.remove(path_index);

            // Auto-save for WS Admin only: push changes to server with status = Drafted
            if matches!(
                self.app.current_user_role,
                Some(UserRole::WizardSardineAdmin)
            ) {
                if let Some(wallet) = self.build_wallet_from_app_state(WalletStatus::Drafted) {
                    self.backend.edit_wallet(wallet);
                }
            }
        }
    }

    fn on_template_edit_path(&mut self, is_primary: bool, path_index: Option<usize>) {
        use views::path::TimelockUnit;

        if is_primary {
            self.views.paths.edit_path_modal = Some(views::EditPathModalState {
                is_primary: true,
                path_index: None,
                selected_key_ids: self.app.primary_path.key_ids.clone(),
                threshold: self.app.primary_path.threshold_n.to_string(),
                timelock_value: None,
                timelock_unit: TimelockUnit::default(),
            });
        } else if let Some(index) = path_index {
            if let Some(secondary) = self.app.secondary_paths.get(index) {
                // Determine the best unit for display (largest unit that divides evenly)
                let blocks = secondary.timelock.blocks;
                let (unit, value) = if blocks >= TimelockUnit::Months.blocks_per_unit()
                    && blocks % TimelockUnit::Months.blocks_per_unit() == 0
                {
                    (
                        TimelockUnit::Months,
                        TimelockUnit::Months.from_blocks(blocks),
                    )
                } else if blocks >= TimelockUnit::Days.blocks_per_unit()
                    && blocks % TimelockUnit::Days.blocks_per_unit() == 0
                {
                    (TimelockUnit::Days, TimelockUnit::Days.from_blocks(blocks))
                } else if blocks >= TimelockUnit::Hours.blocks_per_unit()
                    && blocks % TimelockUnit::Hours.blocks_per_unit() == 0
                {
                    (TimelockUnit::Hours, TimelockUnit::Hours.from_blocks(blocks))
                } else {
                    (TimelockUnit::Blocks, blocks)
                };

                self.views.paths.edit_path_modal = Some(views::EditPathModalState {
                    is_primary: false,
                    path_index: Some(index),
                    selected_key_ids: secondary.path.key_ids.clone(),
                    threshold: secondary.path.threshold_n.to_string(),
                    timelock_value: Some(value.to_string()),
                    timelock_unit: unit,
                });
            }
        }
    }

    fn on_template_new_path_modal(&mut self) {
        use views::path::TimelockUnit;

        // Open modal for creating a new recovery path (all keys deselected)
        self.views.paths.edit_path_modal = Some(views::EditPathModalState {
            is_primary: false,
            path_index: None, // None indicates a new path
            selected_key_ids: Vec::new(),
            threshold: String::new(),
            timelock_value: Some("1".to_string()),
            timelock_unit: TimelockUnit::Days,
        });
    }

    fn on_template_save_path(&mut self) {
        if let Some(modal_state) = &self.views.paths.edit_path_modal {
            let selected_keys = modal_state.selected_key_ids.clone();
            let selected_count = selected_keys.len();

            if modal_state.is_primary {
                // Apply key changes to primary path
                self.app.primary_path.key_ids = selected_keys;

                // Handle threshold - parse and validate
                if let Ok(threshold_n) = modal_state.threshold.parse::<u8>() {
                    if threshold_n > 0
                        && (threshold_n as usize) <= selected_count
                        && selected_count > 0
                    {
                        self.app.primary_path.threshold_n = threshold_n;
                    } else if selected_count > 0 {
                        // Default to all keys required if threshold invalid
                        self.app.primary_path.threshold_n = selected_count as u8;
                    }
                } else if selected_count > 0 {
                    // Default to all keys required if parse fails
                    self.app.primary_path.threshold_n = selected_count as u8;
                }
            } else if let Some(path_index) = modal_state.path_index {
                // Editing existing secondary path
                if let Some(secondary) = self.app.secondary_paths.get_mut(path_index) {
                    // Apply key changes to secondary path
                    secondary.path.key_ids = selected_keys;

                    // Handle threshold - parse and validate
                    if let Ok(threshold_n) = modal_state.threshold.parse::<u8>() {
                        if threshold_n > 0
                            && (threshold_n as usize) <= selected_count
                            && selected_count > 0
                        {
                            secondary.path.threshold_n = threshold_n;
                        } else if selected_count > 0 {
                            // Default to all keys required if threshold invalid
                            secondary.path.threshold_n = selected_count as u8;
                        }
                    } else if selected_count > 0 {
                        // Default to all keys required if parse fails
                        secondary.path.threshold_n = selected_count as u8;
                    }

                    // Handle timelock (only for secondary paths)
                    if let Some(value_str) = &modal_state.timelock_value {
                        if let Ok(value) = value_str.parse::<u64>() {
                            secondary.timelock.blocks =
                                modal_state.timelock_unit.to_blocks_capped(value);
                        }
                    }
                }
                // Re-sort paths after timelock change
                self.app.sort_secondary_paths();
            } else {
                // Creating new secondary path (path_index is None)
                let threshold_n = if let Ok(n) = modal_state.threshold.parse::<u8>() {
                    if n > 0 && (n as usize) <= selected_count && selected_count > 0 {
                        n
                    } else if selected_count > 0 {
                        selected_count as u8
                    } else {
                        1
                    }
                } else if selected_count > 0 {
                    selected_count as u8
                } else {
                    1
                };

                let blocks = if let Some(value_str) = &modal_state.timelock_value {
                    if let Ok(value) = value_str.parse::<u64>() {
                        modal_state.timelock_unit.to_blocks_capped(value)
                    } else {
                        BLOCKS_PER_DAY // Default 1 day
                    }
                } else {
                    BLOCKS_PER_DAY // Default 1 day
                };

                let new_path = ws_business::SpendingPath::new(false, threshold_n, selected_keys);
                let new_timelock = ws_business::Timelock::new(blocks);
                self.app.secondary_paths.push(SecondaryPath {
                    path: new_path,
                    timelock: new_timelock,
                });
                self.app.sort_secondary_paths();
            }

            self.views.paths.edit_path_modal = None;

            // Auto-save for WS Admin only: push changes to server with status = Drafted
            if matches!(
                self.app.current_user_role,
                Some(UserRole::WizardSardineAdmin)
            ) {
                if let Some(wallet) = self.build_wallet_from_app_state(WalletStatus::Drafted) {
                    self.backend.edit_wallet(wallet);
                }
            }
        }
    }

    fn on_template_lock(&mut self) {
        // Only WS Admin can lock
        if !matches!(
            self.app.current_user_role,
            Some(UserRole::WizardSardineAdmin)
        ) {
            return;
        }

        if self.is_template_valid() {
            // Push template to server with status = Locked
            if let Some(wallet) = self.build_wallet_from_app_state(WalletStatus::Locked) {
                self.backend.edit_wallet(wallet);
            }
        }
    }

    fn on_template_unlock(&mut self) {
        // Only WS Admin can unlock
        if !matches!(
            self.app.current_user_role,
            Some(UserRole::WizardSardineAdmin)
        ) {
            return;
        }

        // Push template to server with status = Drafted (unlocking)
        if let Some(wallet) = self.build_wallet_from_app_state(WalletStatus::Drafted) {
            self.backend.edit_wallet(wallet);
        }
    }

    fn on_template_validate(&mut self) {
        // Only Wallet Manager can validate
        if !matches!(self.app.current_user_role, Some(UserRole::WalletManager)) {
            return;
        }

        // Wallet Manager can only validate Locked wallets
        let wallet_status = self
            .app
            .selected_wallet
            .and_then(|id| self.backend.get_wallet(id))
            .map(|w| w.status);

        if !matches!(wallet_status, Some(WalletStatus::Locked)) {
            return;
        }

        // Push template to server with status = Validated
        if let Some(wallet) = self.build_wallet_from_app_state(WalletStatus::Validated) {
            self.backend.edit_wallet(wallet);
        }
    }
}

// Warnings
impl State {
    fn on_warning_show_modal<T: Into<String>, M: Into<String>>(&mut self, title: T, message: M) {
        let title: String = title.into();
        let message: String = message.into();
        self.views.modals.warning = Some(crate::state::views::modals::WarningModalState::new(
            title, message,
        ));
    }

    fn on_warning_close_modal(&mut self) {
        self.views.modals.warning = None;
    }
}

// Navigation
impl State {
    fn on_navigate_to_home(&mut self) {
        self.current_view = View::WalletEdit;
    }

    fn on_navigate_to_keys(&mut self) {
        self.current_view = View::Keys;
    }

    fn on_navigate_to_org_select(&mut self) {
        if self.views.login.current == views::LoginState::Authenticated {
            self.current_view = View::OrgSelect;
        }
    }

    fn on_navigate_to_wallet_select(&mut self) {
        if self.app.selected_org.is_some() {
            // Stop HW listening if leaving Registration view
            if self.current_view == View::Registration {
                self.stop_hw();
            }
            self.current_view = View::WalletSelect;
        }
    }

    fn on_navigate_back(&mut self) -> Task<Msg> {
        match self.current_view {
            View::WalletSelect => {
                self.current_view = View::OrgSelect;
                Task::none()
            }
            View::WalletEdit => {
                self.current_view = View::WalletSelect;
                Task::none()
            }
            View::Keys => {
                self.current_view = View::WalletEdit;
                Task::none()
            }
            View::Xpub => {
                self.current_view = View::WalletSelect;
                Task::none()
            }
            View::OrgSelect => {
                self.current_view = View::Login;
                // Focus email input when returning to login
                text_input::focus("login_email")
            }
            View::Login => {
                if self.views.login.current == views::LoginState::CodeEntry {
                    self.views.login.email.processing = false;
                    self.views.login.current = views::LoginState::EmailEntry;
                    // Focus email input when going back from code entry
                    text_input::focus("login_email")
                } else {
                    Task::none()
                }
            }
            View::Registration => {
                self.stop_hw();
                self.current_view = View::WalletSelect;
                Task::none()
            }
        }
    }
}

// Backend updates
impl State {
    fn on_backend_connected(&mut self) {
        // Check if this connection came from cached token login (AccountSelect flow)
        if self.views.login.account_select.processing {
            // Success! Transition to authenticated state
            self.views.login.current = views::LoginState::Authenticated;
            self.views.login.account_select.processing = false;
            self.views.login.account_select.selected_email = None;
            self.current_view = View::OrgSelect;
        }

        // Start background token refresh thread
        self.backend.start_token_refresh_thread();
    }

    fn on_backend_auth_code_sent(&mut self) -> Task<Msg> {
        tracing::debug!("on_backend_auth_code_sent: transitioning to CodeEntry");
        self.views.login.current = views::LoginState::CodeEntry;
        // Clear any previous errors
        self.views.login.code.form.warning = None;
        self.views.login.code.form.valid = true;
        // Reset code field
        self.views.login.code.form = liana_ui::component::form::Value {
            value: String::new(),
            warning: None,
            valid: true,
        };
        self.views.login.email.processing = false;
        // Focus the code input field
        text_input::focus("login_code")
    }

    fn on_backend_invalid_email(&mut self) {
        self.views.login.email.form.valid = false;
        self.views.login.email.form.warning = Some("Email is invalid!");
        self.views.login.email.processing = false;
    }

    fn on_backend_auth_code_fail(&mut self) {
        self.views.login.email.form.valid = false;
        self.views.login.email.form.warning =
            Some("Fail to request authentication code from server!");
        self.views.login.email.processing = false;
    }

    fn on_backend_login_success(&mut self) {
        self.views.login.current = views::LoginState::Authenticated;
        self.views.login.code.form.value = String::new();
        self.current_view = View::OrgSelect;
        self.views.login.code.processing = false;

        // Token was already set by auth_code() after successful verify

        // Mark that we're intentionally reconnecting (old channel will close)
        self.app.reconnecting = true;

        // Connect WebSocket immediately after login success
        // This will establish the connection now that we have a token
        // (token refresh thread is started in on_backend_connected)
        self.backend.connect_ws(
            ws_url(self.network),
            PROTOCOL_VERSION,
            self.notif_sender.clone(),
        );
    }

    fn on_backend_login_fail(&mut self) {
        self.views.login.code.form.valid = false;
        self.views.login.code.form.warning = Some("Login fail!");
        self.views.login.code.processing = false;
    }

    fn on_backend_error(&mut self, error: Error) {
        error!("on_backend_error: received error={:?}", error);

        // Check if error occurred during cached token connection
        if self.views.login.account_select.processing {
            self.handle_cached_token_connection_failure();
            return;
        }

        // Reset login processing flags so user can retry
        // This handles errors that bypass the specific InvalidEmail/AuthCodeFail handlers
        if self.views.login.email.processing {
            self.views.login.email.processing = false;
        }
        if self.views.login.code.processing {
            self.views.login.code.processing = false;
        }

        if error.show_warning() {
            self.on_warning_show_modal("Backend error", error.to_string());
        }
    }

    fn on_backend_disconnected(&mut self) {
        // Check if disconnect occurred during cached token connection
        if self.views.login.account_select.processing {
            self.handle_cached_token_connection_failure();
            return;
        }

        // // If we're intentionally reconnecting, don't show error
        // if self.app.reconnecting {
        //     self.app.reconnecting = false;
        //     return;
        // }

        // Show error modal - don't retry connection
        self.on_warning_show_modal(
            "Connection Error",
            "Lost connection to the server. Please restart the application.",
        );
    }

    /// Handle failure when connecting with a cached token
    fn handle_cached_token_connection_failure(&mut self) {
        let failed_email = self
            .views
            .login
            .account_select
            .selected_email
            .take()
            .unwrap_or_default();

        // Reset processing state
        self.views.login.account_select.processing = false;

        // Clear the failed token from cache
        if !failed_email.is_empty() {
            self.backend.clear_invalid_tokens(&[failed_email.clone()]);
        }

        // Remove the failed account from the current list
        self.views
            .login
            .account_select
            .accounts
            .retain(|a| a.email != failed_email);

        // Show warning modal
        self.on_warning_show_modal(
            "Connection Failed",
            format!(
                "Failed to connect with account {}. The session may have expired.",
                failed_email
            ),
        );

        // Decide next state based on remaining accounts
        if self.views.login.account_select.accounts.is_empty() {
            // No valid tokens left, go to EmailEntry
            self.views.login.current = views::LoginState::EmailEntry;
        } else {
            // Still have valid accounts, stay on AccountSelect
            self.views.login.current = views::LoginState::AccountSelect;
        }
    }

    fn on_logout(&mut self) -> Task<Msg> {
        // Call backend logout to clear token, close connection
        self.backend.logout();

        // Clear email and code form values
        self.views.login.email.form.value = String::new();
        self.views.login.email.form.valid = false;
        self.views.login.email.form.warning = None;
        self.views.login.email.processing = false;
        self.views.login.code.form.value = String::new();
        self.views.login.code.form.valid = false;
        self.views.login.code.form.warning = None;
        self.views.login.code.processing = false;

        // Reset account select state
        self.views.login.account_select.processing = false;
        self.views.login.account_select.selected_email = None;

        // Decide next login state based on remaining cached accounts
        if self.views.login.account_select.accounts.is_empty() {
            self.views.login.current = views::LoginState::EmailEntry;
        } else {
            self.views.login.current = views::LoginState::AccountSelect;
        }

        // Reset application state
        self.app.selected_org = None;
        self.app.selected_wallet = None;
        self.app.current_wallet_template = None;
        self.app.global_user_role = None;
        self.app.reconnecting = false;
        self.app.exit = false;

        // Navigate to login view
        self.current_view = View::Login;

        // Focus email input if going to EmailEntry
        if self.views.login.current == views::LoginState::EmailEntry {
            text_input::focus("login_email")
        } else {
            Task::none()
        }
    }
}

// Wallet notifications and conflict resolution
impl State {
    /// Handle wallet notification - check for modal conflicts and refresh state
    fn on_backend_wallet(&mut self, wallet_id: Uuid) -> Task<Msg> {
        debug!(
            "on_backend_wallet: received wallet update for wallet_id={}",
            wallet_id
        );

        // Get the updated wallet from cache first (needed to check org)
        let Some(wallet) = self.backend.get_wallet(wallet_id) else {
            debug!("on_backend_wallet: wallet not found in cache");
            return Task::none();
        };

        // Check if wallet belongs to selected org
        let is_selected_org = self.app.selected_org == Some(wallet.org);

        // If not the selected wallet, only log and return
        // The cache is already updated by handle_wallet(), and iced will
        // re-render the wallet list view automatically
        if self.app.selected_wallet != Some(wallet_id) {
            if is_selected_org {
                debug!("on_backend_wallet: wallet in selected org but not selected, cache updated");
            } else {
                debug!(
                    "on_backend_wallet: ignoring - wallet not in selected org (wallet_org={}, selected_org={:?})",
                    wallet.org,
                    self.app.selected_org
                );
            }
            return Task::none();
        }

        debug!(
            "on_backend_wallet: wallet '{}' status={:?}, current_view={:?}",
            wallet.alias, wallet.status, self.current_view
        );

        // Check for conflicts with open modals before updating state
        self.check_modal_conflicts(&wallet, wallet_id);

        // If no conflict modal was shown, refresh local state
        if self.views.modals.conflict.is_none() {
            self.load_wallet_into_app_state(&wallet);
        }

        // Redirect views on WalletStatus change
        let user_id = *self.backend.user_id.lock().expect("poisoned");
        let current_user = user_id.and_then(|id| self.backend.get_user(id));
        if let Some(user) = current_user {
            if let Some(role) = user.role(&wallet) {
                let email = &self.views.login.email.form.value;
                let status = wallet.effective_status(email);
                match role {
                    UserRole::WalletManager => match (self.current_view, status) {
                        (View::Keys, WalletStatus::Locked) => {
                            debug!("on_backend_wallet: wallet locked while on Keys view, redirecting to wallet edit");
                            self.current_view = View::WalletEdit;
                            return Task::none();
                        }
                        (View::WalletEdit, WalletStatus::Validated) => {
                            debug!("on_backend_wallet: wallet validated, redirecting to Xpub view");
                            self.current_view = View::Xpub;
                            return Task::none();
                        }
                        _ => {}
                    },
                    UserRole::WizardSardineAdmin => {
                        if let (View::WalletEdit, WalletStatus::Validated) =
                            (self.current_view, status)
                        {
                            debug!(
                                "on_backend_wallet: wallet validated, redirecting to wallet list"
                            );
                            self.current_view = View::WalletSelect;
                            self.app.selected_wallet = None;
                        }
                    }
                    _ => {}
                }
            }
        }

        // Update registration state if on Registration view
        if self.current_view == View::Registration {
            let email = &self.views.login.email.form.value;
            let wallet_status = wallet.effective_status(email);
            debug!("on_backend_wallet: on Registration view, checking status");
            if wallet_status == WalletStatus::Registration {
                let devices = wallet.user_devices(email);
                let descriptor_len = wallet.descriptor.as_ref().map(|d| d.len()).unwrap_or(0);
                debug!(
                    "on_backend_wallet: Registration with {} devices, descriptor_len={}",
                    devices.len(),
                    descriptor_len
                );
                self.views.registration.descriptor = wallet.descriptor.clone();
                self.views.registration.user_devices = devices;
                debug!(
                    "on_backend_wallet: user_devices: {:?}",
                    self.views.registration.user_devices
                );
            } else if wallet_status == WalletStatus::Finalized {
                debug!("on_backend_wallet: wallet is Finalized, signaling exit to open wallet");

                // Close registration modal if open and stop HW listening
                if self.views.registration.modal.is_some() {
                    self.views.registration.close_modal();
                }
                self.stop_hw();

                // Signal exit to open the main wallet application
                // Return Task::done to generate a follow-up message that triggers exit_maybe
                self.app.exit = true;
                return Task::done(Msg::Update);
            } else {
                debug!(
                    "on_backend_wallet: wallet not in Registration status: {:?}",
                    wallet.status
                );
            }
        }

        // Handle Xpub view status changes when wallet becomes Finalized
        if self.current_view == View::Xpub {
            let email = &self.views.login.email.form.value;
            let wallet_status = wallet.effective_status(email);
            debug!(
                "on_backend_wallet: on Xpub view, effective status: {:?}",
                wallet_status
            );

            match wallet_status {
                WalletStatus::Registration => {
                    debug!(
                        "on_backend_wallet: wallet moved to Registration, transitioning from Xpub"
                    );

                    // Close xpub modal if open
                    if self.views.xpub.modal.is_some() {
                        self.views.xpub.close_modal();
                    }

                    // Set up registration state (mirrors on_org_wallet_selected pattern)
                    self.views.registration.descriptor = wallet.descriptor.clone();
                    self.views.registration.user_devices = wallet.user_devices(email);

                    // Start hardware wallet listening for registration
                    self.start_hw();

                    // Navigate to Registration view
                    self.current_view = View::Registration;
                    return Task::none();
                }
                WalletStatus::Finalized => {
                    debug!("on_backend_wallet: wallet is Finalized, signaling exit to open wallet");

                    // Close xpub modal if open and stop HW listening
                    if self.views.xpub.modal.is_some() {
                        self.views.xpub.close_modal();
                        self.stop_hw();
                    }

                    // Signal exit to open the main wallet application
                    // Return Task::done to generate a follow-up message that triggers exit_maybe
                    self.app.exit = true;
                    return Task::done(Msg::Update);
                }
                _ => {
                    // Other statuses - stay on Xpub view
                }
            }
        }

        // Close xpub modal if open (edit was successful and wallet updated)
        // The modal should be closed after successful save/clear operations
        if self.views.xpub.modal.is_some() {
            self.views.xpub.close_modal();
            self.stop_hw();
        }

        Task::none()
    }

    /// Check for conflicts between open modals and the new wallet state
    fn check_modal_conflicts(&mut self, wallet: &Wallet, wallet_id: Uuid) {
        let new_template = wallet.template.as_ref();

        // Check if key modal is open
        if let Some(modal) = &self.views.keys.edit_key_modal {
            if !modal.is_new {
                let key_id = modal.key_id;
                // Check if the key still exists in the new wallet
                let key_exists = new_template
                    .map(|t| t.keys.contains_key(&key_id))
                    .unwrap_or(false);

                if !key_exists {
                    // Key was deleted
                    self.views.keys.edit_key_modal = None;
                    self.views.modals.conflict = Some(ConflictModalState {
                        conflict_type: ConflictType::KeyDeleted,
                        title: "Key Deleted".to_string(),
                        message: "The key you were editing was deleted by another user."
                            .to_string(),
                    });
                    return;
                }

                // Check if the key was modified
                if let Some(template) = new_template {
                    if let Some(server_key) = template.keys.get(&key_id) {
                        if let Some(local_key) = self.app.keys.get(&key_id) {
                            if server_key != local_key {
                                // Key was modified
                                self.views.modals.conflict = Some(ConflictModalState {
                                    conflict_type: ConflictType::KeyModified { key_id, wallet_id },
                                    title: "Key Modified".to_string(),
                                    message: "This key was modified by another user. Would you like to reload the server version or keep your changes?".to_string(),
                                });
                                return;
                            }
                        }
                    }
                }
            }
        }

        // Check if path modal is open
        if let Some(modal) = &mut self.views.paths.edit_path_modal {
            // Check for deleted keys in the path being edited
            if let Some(template) = new_template {
                let mut deleted_keys = Vec::new();
                for &key_id in &modal.selected_key_ids {
                    if !template.keys.contains_key(&key_id) {
                        let key_alias = self
                            .app
                            .keys
                            .get(&key_id)
                            .map(|k| k.alias.clone())
                            .unwrap_or_else(|| format!("Key {}", key_id));
                        deleted_keys.push((key_id, key_alias));
                    }
                }

                // Remove deleted keys from selection and show warning
                if !deleted_keys.is_empty() {
                    for (key_id, _) in &deleted_keys {
                        modal.selected_key_ids.retain(|&id| id != *key_id);
                    }

                    // Validate threshold after key removal
                    let selected_count = modal.selected_key_ids.len();
                    if let Ok(threshold) = modal.threshold.parse::<u8>() {
                        if (threshold as usize) > selected_count && selected_count > 0 {
                            modal.threshold = selected_count.to_string();
                        }
                    }

                    let (_first_key_id, first_key_alias) = deleted_keys[0].clone();
                    self.views.modals.conflict = Some(ConflictModalState {
                        conflict_type: ConflictType::KeyInPathDeleted,
                        title: "Key Removed".to_string(),
                        message: format!(
                            "\"{}\" was deleted by another user and has been removed from your path selection.",
                            first_key_alias
                        ),
                    });
                    return;
                }

                // Check if the path being edited was modified or deleted
                if modal.is_primary {
                    // Check if primary path was modified
                    if let Some(_local_primary) =
                        self.app.keys.is_empty().then_some(()).or_else(|| {
                            // Compare modal's selected keys with server's primary path
                            let modal_keys: std::collections::HashSet<u8> =
                                modal.selected_key_ids.iter().copied().collect();
                            let server_keys: std::collections::HashSet<u8> =
                                template.primary_path.key_ids.iter().copied().collect();
                            let modal_threshold = modal.threshold.parse::<u8>().unwrap_or(0);
                            if modal_keys != server_keys
                                || modal_threshold != template.primary_path.threshold_n
                            {
                                Some(())
                            } else {
                                None
                            }
                        })
                    {
                        self.views.modals.conflict = Some(ConflictModalState {
                            conflict_type: ConflictType::PathModified {
                                is_primary: true,
                                path_index: None,
                                wallet_id,
                            },
                            title: "Path Modified".to_string(),
                            message: "The primary path was modified by another user. Would you like to reload the server version or keep your changes?".to_string(),
                        });
                    }
                } else if let Some(path_index) = modal.path_index {
                    if path_index >= template.secondary_paths.len() {
                        // Path was deleted
                        self.views.paths.edit_path_modal = None;
                        self.views.modals.conflict = Some(ConflictModalState {
                            conflict_type: ConflictType::PathDeleted,
                            title: "Path Deleted".to_string(),
                            message: "The path you were editing was deleted by another user."
                                .to_string(),
                        });
                        return;
                    }

                    // Check if secondary path was modified
                    let server_secondary = &template.secondary_paths[path_index];
                    let modal_keys: std::collections::HashSet<u8> =
                        modal.selected_key_ids.iter().copied().collect();
                    let server_keys: std::collections::HashSet<u8> =
                        server_secondary.path.key_ids.iter().copied().collect();
                    let modal_threshold = modal.threshold.parse::<u8>().unwrap_or(0);
                    // Convert modal timelock from display units to blocks for comparison
                    let modal_timelock_blocks = modal
                        .timelock_value
                        .as_ref()
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|v| modal.timelock_unit.to_blocks_capped(v))
                        .unwrap_or(0);

                    if modal_keys != server_keys
                        || modal_threshold != server_secondary.path.threshold_n
                        || modal_timelock_blocks != server_secondary.timelock.blocks
                    {
                        self.views.modals.conflict = Some(ConflictModalState {
                            conflict_type: ConflictType::PathModified {
                                is_primary: false,
                                path_index: Some(path_index),
                                wallet_id,
                            },
                            title: "Path Modified".to_string(),
                            message: "This recovery path was modified by another user. Would you like to reload the server version or keep your changes?".to_string(),
                        });
                        return;
                    }
                }
            }
        }
    }

    /// Load wallet data into AppState
    fn load_wallet_into_app_state(&mut self, wallet: &Wallet) {
        if let Some(template) = &wallet.template {
            self.app.keys = template.keys.clone();
            self.app.primary_path = template.primary_path.clone();
            self.app.secondary_paths = template.secondary_paths.clone();
            self.app.next_key_id = template.keys.keys().copied().max().unwrap_or(0) + 1;
        } else {
            self.app.keys.clear();
            self.app.primary_path = SpendingPath {
                is_primary: true,
                threshold_n: 0,
                key_ids: vec![],
                last_edited: None,
                last_editor: None,
            };
            self.app.secondary_paths.clear();
            self.app.next_key_id = 0;
        }
    }

    /// Handle conflict reload - user chose to reload from server
    fn on_conflict_reload(&mut self) -> Task<Msg> {
        if let Some(conflict) = self.views.modals.conflict.take() {
            match conflict.conflict_type {
                ConflictType::KeyModified { key_id, wallet_id } => {
                    // Refresh from cache (already updated) and update modal
                    if let Some(wallet) = self.backend.get_wallet(wallet_id) {
                        if let Some(template) = &wallet.template {
                            if let Some(key) = template.keys.get(&key_id) {
                                // Extract email from KeyIdentity
                                let email = match &key.identity {
                                    KeyIdentity::Email(e) => e.clone(),
                                    KeyIdentity::Token(t) => t.clone(),
                                    KeyIdentity::Other(o) => o.clone(),
                                };
                                // Update the modal with server data
                                if let Some(modal) = &mut self.views.keys.edit_key_modal {
                                    modal.alias = key.alias.clone();
                                    modal.description = key.description.clone();
                                    modal.email = email;
                                    modal.key_type = key.key_type;
                                }
                                // Also update local AppState
                                self.app.keys.insert(key_id, key.clone());
                            }
                        }
                    }
                }
                ConflictType::PathModified {
                    is_primary,
                    path_index,
                    wallet_id,
                } => {
                    // Refresh from cache and update modal
                    if let Some(wallet) = self.backend.get_wallet(wallet_id) {
                        if let Some(template) = &wallet.template {
                            if is_primary {
                                // Update modal with primary path data
                                if let Some(modal) = &mut self.views.paths.edit_path_modal {
                                    modal.selected_key_ids = template.primary_path.key_ids.clone();
                                    modal.threshold = template.primary_path.threshold_n.to_string();
                                }
                                self.app.primary_path = template.primary_path.clone();
                            } else if let Some(idx) = path_index {
                                if let Some(secondary) = template.secondary_paths.get(idx) {
                                    if let Some(modal) = &mut self.views.paths.edit_path_modal {
                                        modal.selected_key_ids = secondary.path.key_ids.clone();
                                        modal.threshold = secondary.path.threshold_n.to_string();
                                        modal.timelock_value =
                                            Some(secondary.timelock.blocks.to_string());
                                    }
                                    if idx < self.app.secondary_paths.len() {
                                        self.app.secondary_paths[idx] = secondary.clone();
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {
                    // For delete conflicts, just dismiss (they're info-only)
                }
            }
        }
        Task::none()
    }

    /// Handle conflict keep local - user chose to keep local changes
    fn on_conflict_keep_local(&mut self) {
        // Just close the conflict modal, keep local changes
        self.views.modals.conflict = None;
    }

    /// Handle conflict dismiss - dismiss info-only conflict (deletion notice)
    fn on_conflict_dismiss(&mut self) {
        self.views.modals.conflict = None;
    }
}

// Hardware wallet handlers
impl State {
    /// Handle hardware wallet messages from async-hwi service
    fn on_hw_message(&mut self, msg: async_hwi::service::SigningDeviceMsg) -> Task<Msg> {
        use async_hwi::service::SigningDeviceMsg;
        use miniscript::bitcoin::bip32::DerivationPath;
        use miniscript::descriptor::{DescriptorPublicKey, DescriptorXKey, Wildcard};

        match msg {
            SigningDeviceMsg::Update => {
                // Device list changed - UI will redraw automatically with new state.hw content
            }
            SigningDeviceMsg::XPub(_id, fingerprint, path, xpub) => {
                // xpub fetch completed - populate input
                // Look up device info for audit
                let device_info = self.hw.list().values().find_map(|dev| {
                    if dev.fingerprint() == Some(fingerprint) {
                        let kind = format!("{:?}", dev.kind());
                        let version = match dev {
                            async_hwi::service::SigningDevice::Supported(s) => {
                                s.version().map(|v| v.to_string())
                            }
                            async_hwi::service::SigningDevice::Unsupported { version, .. } => {
                                version.clone().map(|v| v.to_string())
                            }
                            _ => None,
                        };
                        Some((kind, version))
                    } else {
                        None
                    }
                });

                // Log xpub fetch source info
                debug!(
                    source = "device",
                    device_kind = device_info.as_ref().map(|(k, _)| k.as_str()).unwrap_or("Unknown"),
                    device_fingerprint = %fingerprint,
                    device_version = device_info.as_ref().and_then(|(_, v)| v.as_deref()).unwrap_or("Unknown"),
                    derivation_path = %path,
                    "Fetched xpub from hardware device"
                );

                if let Some(modal) = self.views.xpub.modal_mut() {
                    modal.set_processing(false);
                    // Convert xpub to DescriptorPublicKey and populate input
                    let desc_xpub = DescriptorPublicKey::XPub(DescriptorXKey {
                        origin: Some((fingerprint, path)),
                        derivation_path: DerivationPath::master(),
                        wildcard: Wildcard::None,
                        xkey: xpub,
                    });
                    modal.update_input(desc_xpub.to_string());
                    // Set source for audit
                    modal.input_source = Some(views::XpubInputSource::Device {
                        kind: device_info
                            .as_ref()
                            .map(|(k, _): &(String, Option<String>)| k.clone())
                            .unwrap_or_else(|| "Unknown".to_string()),
                        fingerprint: fingerprint.to_string(),
                        version: device_info.and_then(|(_, v)| v),
                    });
                }
            }
            SigningDeviceMsg::Error(_id, e) => {
                // Show error in modal (use fetch_error for Details step)
                if let Some(modal) = self.views.xpub.modal_mut() {
                    modal.set_processing(false);
                    modal.set_fetch_error(e);
                }
            }
            // Ignore other messages (Version, WalletRegistered, etc.)
            _ => {}
        }
        Task::none()
    }
}

// Xpub management handlers
impl State {
    /// Open xpub entry modal for a key
    fn on_xpub_select_key(&mut self, key_id: u8) {
        if let Some(key) = self.app.keys.get(&key_id) {
            self.views
                .xpub
                .open_modal(key_id, key.alias.clone(), key.xpub.clone(), self.network);
            // Start hardware wallet listening when modal opens
            self.start_hw();
        }
    }

    /// Update xpub input text
    fn on_xpub_update_input(&mut self, input: String) {
        if let Some(modal) = self.views.xpub.modal_mut() {
            modal.update_input(input);
        }
    }

    /// Select hardware wallet device - transitions to Details step and triggers fetch
    fn on_xpub_select_device(
        &mut self,
        fingerprint: miniscript::bitcoin::bip32::Fingerprint,
    ) -> Task<Msg> {
        let account = if let Some(modal) = self.views.xpub.modal_mut() {
            modal.select_device(fingerprint); // This sets step to Details and processing=true
            modal.selected_account
        } else {
            return Task::none();
        };
        // Trigger the initial fetch with default account
        Task::done(Msg::XpubFetchFromDevice(fingerprint, account))
    }

    /// Go back from Details to Select step
    fn on_xpub_device_back(&mut self) {
        if let Some(modal) = self.views.xpub.modal_mut() {
            modal.go_back();
        }
    }

    /// Retry fetch after error
    fn on_xpub_retry(&mut self) -> Task<Msg> {
        if let Some(modal) = self.views.xpub.modal_mut() {
            if let Some(fp) = modal.selected_device {
                modal.clear_fetch_error();
                modal.set_processing(true);
                let account = modal.selected_account;
                return Task::done(Msg::XpubFetchFromDevice(fp, account));
            }
        }
        Task::none()
    }

    /// Fetch xpub from hardware wallet device
    fn on_xpub_fetch_from_device(
        &mut self,
        fingerprint: miniscript::bitcoin::bip32::Fingerprint,
        account: miniscript::bitcoin::bip32::ChildNumber,
    ) -> Task<Msg> {
        use async_hwi::service::SigningDevice;
        #[allow(unused_imports)]
        use miniscript::bitcoin::bip32::{ChildNumber, DerivationPath};

        // Set processing state
        if let Some(modal) = self.views.xpub.modal_mut() {
            modal.set_processing(true);
            modal.clear_fetch_error();
        }

        // Find supported device with matching fingerprint
        let devices = self.hw.list();
        let device = devices
            .values()
            .find(|dev| dev.is_supported() && dev.fingerprint() == Some(fingerprint));

        match device {
            Some(SigningDevice::Supported(supported)) => {
                // Build derivation path: m/48'/network'/account'/2'
                // network' is 0' for mainnet, 1' for testnet/signet/regtest
                let network_idx = if self.network == miniscript::bitcoin::Network::Bitcoin {
                    ChildNumber::Hardened { index: 0 }
                } else {
                    ChildNumber::Hardened { index: 1 }
                };

                let derivation_path = DerivationPath::from(vec![
                    ChildNumber::Hardened { index: 48 },
                    network_idx,
                    account,
                    ChildNumber::Hardened { index: 2 },
                ]);

                // Non-blocking! Result comes via SigningDeviceMsg::XPub
                supported.get_extended_pubkey((), &derivation_path);
            }
            Some(SigningDevice::Locked { .. }) => {
                if let Some(modal) = self.views.xpub.modal_mut() {
                    modal.set_processing(false);
                    modal.set_fetch_error("Device is locked. Please unlock it first.".to_string());
                }
            }
            Some(SigningDevice::Unsupported { .. }) => {
                if let Some(modal) = self.views.xpub.modal_mut() {
                    modal.set_processing(false);
                    modal.set_fetch_error("Device is not supported".to_string());
                }
            }
            None => {
                // Device not found
                if let Some(modal) = self.views.xpub.modal_mut() {
                    modal.set_processing(false);
                    modal.set_fetch_error("Hardware wallet not found".to_string());
                }
            }
        }
        Task::none()
    }

    /// Trigger file picker for xpub
    fn on_xpub_load_from_file(&mut self) -> Task<Msg> {
        // Use async file dialog by spawning a thread with Tokio runtime
        // This avoids "no reactor running" panic with Iced's ThreadPool executor
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(async {
                let file_handle = rfd::AsyncFileDialog::new()
                    .set_title("Select xpub file")
                    .add_filter("Text files", &["txt"])
                    .add_filter("All files", &["*"])
                    .pick_file()
                    .await;

                if let Some(handle) = file_handle {
                    let filename = handle.file_name();
                    // Read the file content
                    match tokio::fs::read_to_string(handle.path()).await {
                        Ok(content) => {
                            // Get the first non-empty line as xpub
                            let xpub = content
                                .lines()
                                .find(|line| !line.trim().is_empty())
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            Ok((xpub, filename))
                        }
                        Err(e) => Err(format!("Failed to read file: {}", e)),
                    }
                } else {
                    // User cancelled - return empty error to do nothing
                    Err(String::new())
                }
            });
            let _ = tx.send(result);
        });

        // Poll the channel for the result
        Task::perform(
            async move {
                // Block until result is available
                rx.recv().unwrap_or_else(|_| {
                    Err("Failed to receive result from file dialog thread".to_string())
                })
            },
            |result| {
                // Only send message if there was an actual error (non-empty)
                match result {
                    Ok((xpub, filename)) => Msg::XpubFileLoaded(Ok((xpub, filename))),
                    Err(e) if !e.is_empty() => Msg::XpubFileLoaded(Err(e)),
                    Err(_) => Msg::XpubFileLoaded(Err(String::new())), // User cancelled
                }
            },
        )
    }

    /// Handle file loaded result
    fn on_xpub_file_loaded(&mut self, result: Result<(String, String), String>) {
        if let Some(modal) = self.views.xpub.modal_mut() {
            match result {
                Ok((content, filename)) => {
                    debug!(
                        source = "file",
                        filename = %filename,
                        "Loaded xpub from file"
                    );
                    modal.update_input(content);
                    modal.input_source = Some(views::XpubInputSource::File { name: filename });
                }
                Err(error) if !error.is_empty() => {
                    // Log error (user cancelled shows empty string)
                    debug!(error = %error, "Failed to load xpub file");
                }
                Err(_) => {
                    // User cancelled - do nothing
                }
            }
        }
    }

    /// Handle paste xpub from clipboard
    fn on_xpub_paste(&mut self) -> Task<Msg> {
        use iced::clipboard;

        clipboard::read().map(|contents| {
            if let Some(text) = contents {
                // Get the first non-empty line as xpub
                let xpub = text
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                Msg::XpubPasted(xpub)
            } else {
                Msg::XpubFileLoaded(Err("Clipboard is empty".to_string()))
            }
        })
    }

    /// Handle pasted xpub (sets source to Pasted)
    fn on_xpub_pasted(&mut self, xpub: String) {
        debug!(source = "pasted", "Xpub pasted from clipboard");
        if let Some(modal) = self.views.xpub.modal_mut() {
            modal.update_input(xpub);
            modal.input_source = Some(views::XpubInputSource::Pasted);
        }
    }

    /// Update derivation account for HW wallet - triggers re-fetch if in Details step
    fn on_xpub_update_account(
        &mut self,
        account: miniscript::bitcoin::bip32::ChildNumber,
    ) -> Task<Msg> {
        if let Some(modal) = self.views.xpub.modal_mut() {
            modal.update_account(account);
            // If in Details step with a selected device, trigger re-fetch
            if modal.step == crate::state::views::ModalStep::Details {
                if let Some(fp) = modal.selected_device {
                    modal.clear_fetch_error();
                    modal.set_processing(true);
                    return Task::done(Msg::XpubFetchFromDevice(fp, account));
                }
            }
        }
        Task::none()
    }

    /// Save xpub to backend
    fn on_xpub_save(&mut self) -> Task<Msg> {
        use liana_connect::ws_business::{DeviceKind, Xpub, XpubSource};
        use std::str::FromStr;

        // Validate and save xpub
        if let Some(modal) = &mut self.views.xpub.modal {
            match modal.validate() {
                Ok(xpub) => {
                    let key_id = modal.key_id;

                    // Build Xpub with source info
                    let xpub_data = {
                        let (source, device_kind, device_version, file_name) =
                            match &modal.input_source {
                                Some(views::XpubInputSource::Device {
                                    kind,
                                    fingerprint: _,
                                    version,
                                }) => {
                                    // Parse device kind string to enum
                                    let dk = DeviceKind::from_str(kind).ok();
                                    (XpubSource::Device, dk, version.clone(), None)
                                }
                                Some(views::XpubInputSource::File { name }) => {
                                    (XpubSource::File, None, None, Some(name.clone()))
                                }
                                Some(views::XpubInputSource::Pasted) | None => {
                                    (XpubSource::Pasted, None, None, None)
                                }
                            };
                        Xpub {
                            value: xpub.to_string(),
                            source,
                            device_kind,
                            device_version,
                            file_name,
                        }
                    };

                    // Log key update with xpub source info
                    if let Some(key) = self.app.keys.get(&key_id) {
                        debug!(
                            key_id = key_id,
                            key_alias = %key.alias,
                            key_identity = %key.identity,
                            key_type = ?key.key_type,
                            xpub_source = %xpub_data.source,
                            xpub_device_kind = ?xpub_data.device_kind,
                            xpub_device_version = xpub_data.device_version.as_deref().unwrap_or("N/A"),
                            xpub_file_name = xpub_data.file_name.as_deref().unwrap_or("N/A"),
                            "Key updated with xpub"
                        );
                    }

                    // Send to backend
                    if let Some(wallet_id) = self.app.selected_wallet {
                        self.backend.edit_xpub(wallet_id, Some(xpub_data), key_id);
                    }

                    // Close modal on success and stop HW listening
                    self.views.xpub.close_modal();
                    self.stop_hw();
                }
                Err(error) => {
                    // Validation errors are shown inline by the view
                    debug!(error = %error, "Xpub validation failed on save");
                }
            }
        }
        Task::none()
    }

    /// Clear xpub (set to null)
    fn on_xpub_clear(&mut self) -> Task<Msg> {
        if let Some(modal) = &self.views.xpub.modal {
            let key_id = modal.key_id;

            // Send to backend (None means clear/delete the xpub)
            if let Some(wallet_id) = self.app.selected_wallet {
                self.backend.edit_xpub(wallet_id, None, key_id);
            }

            // Close modal and stop HW listening
            self.views.xpub.close_modal();
            self.stop_hw();
        }
        Task::none()
    }

    /// Close xpub modal
    fn on_xpub_cancel_modal(&mut self) {
        self.views.xpub.close_modal();
        self.stop_hw();
    }

    fn on_xpub_toggle_options(&mut self) {
        if let Some(modal) = self.views.xpub.modal_mut() {
            modal.options_collapsed = !modal.options_collapsed;
        }
    }

    // ========================================================================
    // Registration handlers
    // ========================================================================

    /// User clicked on a connected device to register descriptor
    fn on_registration_select_device(
        &mut self,
        fingerprint: miniscript::bitcoin::bip32::Fingerprint,
    ) -> Task<Msg> {
        use async_hwi::service::SigningDevice;

        debug!("on_registration_select_device: fingerprint={}", fingerprint);

        // Find the device in HwiService
        let devices = self.hw.list();
        trace!(
            "on_registration_select_device: found {} devices",
            devices.len()
        );

        let device = devices.values().find(|d| {
            if let SigningDevice::Supported(hw) = d {
                hw.fingerprint() == &fingerprint
            } else {
                false
            }
        });

        let Some(SigningDevice::Supported(hw)) = device else {
            error!(
                "on_registration_select_device: device not found or not supported for fingerprint={}",
                fingerprint
            );
            return Task::none();
        };

        let device_kind = hw.kind();
        debug!(
            "on_registration_select_device: found device kind={:?}, fingerprint={}",
            device_kind, fingerprint
        );

        // Open modal with device kind
        self.views
            .registration
            .open_modal(fingerprint, Some(*device_kind));

        // Get descriptor from registration status
        let Some(descriptor) = self.views.registration.descriptor.clone() else {
            error!("on_registration_select_device: no descriptor available");
            self.views
                .registration
                .set_modal_error("No descriptor available".to_string());
            return Task::none();
        };

        trace!(
            "on_registration_select_device: descriptor length={}, first 100 chars: {}",
            descriptor.len(),
            &descriptor[..descriptor.len().min(100)]
        );

        // Get wallet name from backend
        let wallet_name = self
            .app
            .selected_wallet
            .and_then(|id| self.backend.get_wallet(id))
            .map(|w| w.alias.clone())
            .unwrap_or_else(|| "Liana".to_string());

        debug!(
            "on_registration_select_device: wallet_name='{}', starting registration on {:?}",
            wallet_name, device_kind
        );

        // Clone device handle for async task
        let hw_arc = hw.device().clone();

        // Start registration via async-hwi
        Task::perform(
            async move {
                trace!(
                    "register_wallet: calling hw.register_wallet(name='{}', descriptor_len={})",
                    wallet_name,
                    descriptor.len()
                );
                match hw_arc.register_wallet(&wallet_name, &descriptor).await {
                    Ok(hmac) => {
                        debug!(
                            "register_wallet: success for fingerprint={}, hmac={:?}",
                            fingerprint,
                            hmac.map(hex::encode)
                        );
                        Ok((fingerprint, hmac, wallet_name))
                    }
                    Err(e) => {
                        error!(
                            "register_wallet: failed for fingerprint={}, error={}",
                            fingerprint, e
                        );
                        Err(e.to_string())
                    }
                }
            },
            Msg::RegistrationResult,
        )
    }

    /// Handle registration result from async-hwi
    fn on_registration_result(
        &mut self,
        result: Result<
            (
                miniscript::bitcoin::bip32::Fingerprint,
                Option<[u8; 32]>,
                String,
            ),
            String,
        >,
    ) -> Task<Msg> {
        debug!("on_registration_result: received result");

        match result {
            Ok((fingerprint, hmac, registered_alias)) => {
                debug!(
                    "on_registration_result: success for fingerprint={}, alias='{}', hmac={:?}",
                    fingerprint,
                    registered_alias,
                    hmac.map(hex::encode)
                );

                // Check if device is Coldcard - it doesn't block during registration
                // so we need user confirmation before sending to server
                let is_coldcard = self
                    .views
                    .registration
                    .modal
                    .as_ref()
                    .and_then(|m| m.device_kind)
                    .map(|k| matches!(k, async_hwi::DeviceKind::Coldcard))
                    .unwrap_or(false);

                if is_coldcard {
                    debug!(
                        "on_registration_result: Coldcard detected, showing confirmation dialog"
                    );
                    // Show confirmation dialog - Coldcard returns immediately without blocking
                    if let Some(modal) = &mut self.views.registration.modal {
                        modal.step = RegistrationModalStep::ConfirmColdcard {
                            hmac,
                            wallet_name: registered_alias,
                        };
                    }
                    return Task::none();
                }

                // For other devices, send immediately
                self.send_device_registered(fingerprint, hmac, registered_alias);
            }
            Err(error) => {
                error!("on_registration_result: registration failed - {}", error);
                // Show error in modal
                self.views.registration.set_modal_error(error);
            }
        }
        Task::none()
    }

    /// Send DeviceRegistered to server and close modal
    fn send_device_registered(
        &mut self,
        fingerprint: miniscript::bitcoin::bip32::Fingerprint,
        hmac: Option<[u8; 32]>,
        registered_alias: String,
    ) {
        // Convert HMAC to Vec<u8> for proof
        let proof = hmac.map(|h| h.to_vec());

        // Get wallet ID
        let Some(wallet_id) = self.app.selected_wallet else {
            error!("send_device_registered: no wallet selected");
            self.views
                .registration
                .set_modal_error("No wallet selected".to_string());
            return;
        };

        // Get current user ID
        let Some(user_id) = *self.backend.user_id.lock().expect("poisoned") else {
            error!("send_device_registered: no user ID available");
            self.views
                .registration
                .set_modal_error("No user ID available".to_string());
            return;
        };

        // Build RegistrationInfos
        let infos = liana_connect::ws_business::RegistrationInfos {
            user: user_id,
            fingerprint,
            registered: true,
            registered_alias: Some(registered_alias.clone()),
            proof_of_registration: proof.clone(),
        };

        debug!(
            "send_device_registered: sending DeviceRegistered to server - wallet_id={}, user={}, fingerprint={}, alias='{}', proof_len={:?}",
            wallet_id,
            user_id,
            fingerprint,
            registered_alias,
            proof.as_ref().map(|p| p.len())
        );

        // Send DeviceRegistered request to server
        self.backend.device_registered(wallet_id, infos);

        // Close modal (keep HW listening for next device)
        self.views.registration.close_modal();
    }

    /// Close registration modal (keep HW listening for next device)
    fn on_registration_cancel_modal(&mut self) {
        self.views.registration.close_modal();
    }

    /// Retry registration after error
    fn on_registration_retry(&mut self) -> Task<Msg> {
        if let Some(modal) = &self.views.registration.modal {
            let fingerprint = modal.fingerprint;
            self.views.registration.close_modal();
            return self.on_registration_select_device(fingerprint);
        }
        Task::none()
    }

    /// User confirms Coldcard registration succeeded
    fn on_registration_confirm_yes(&mut self) -> Task<Msg> {
        if let Some(modal) = &self.views.registration.modal {
            let fingerprint = modal.fingerprint;
            if let RegistrationModalStep::ConfirmColdcard { hmac, wallet_name } = &modal.step {
                debug!(
                    "on_registration_confirm_yes: user confirmed Coldcard registration for fingerprint={}",
                    fingerprint
                );
                let hmac = *hmac;
                let wallet_name = wallet_name.clone();
                self.send_device_registered(fingerprint, hmac, wallet_name);
            }
        }
        Task::none()
    }

    /// User says Coldcard registration failed
    fn on_registration_confirm_no(&mut self) {
        debug!("on_registration_confirm_no: user says Coldcard registration failed");
        // Close modal - user can retry if they want
        self.views.registration.close_modal();
    }

    /// User skips device registration
    fn on_registration_skip(&mut self, fingerprint: Fingerprint) -> Task<Msg> {
        debug!("on_registration_skip: user skipping device {}", fingerprint);

        let Some(wallet_id) = self.app.selected_wallet else {
            error!("on_registration_skip: no wallet selected");
            return Task::none();
        };

        let Some(user_id) = *self.backend.user_id.lock().expect("poisoned") else {
            error!("on_registration_skip: no user_id");
            return Task::none();
        };

        // Send DeviceRegistered with registered=false (skipped)
        let infos = liana_connect::ws_business::RegistrationInfos {
            user: user_id,
            fingerprint,
            registered: false, // Skipped
            registered_alias: None,
            proof_of_registration: None,
        };

        self.backend.device_registered(wallet_id, infos);

        // Remove device from local list (server will update us)
        self.views
            .registration
            .user_devices
            .retain(|fp| *fp != fingerprint);

        Task::none()
    }

    /// Skip all remaining devices (only disconnected ones - connected devices can still be registered)
    fn on_registration_skip_all(&mut self) -> Task<Msg> {
        debug!("on_registration_skip_all: skipping all remaining devices");

        let Some(wallet_id) = self.app.selected_wallet else {
            error!("on_registration_skip_all: no wallet selected");
            return Task::none();
        };

        let Some(user_id) = *self.backend.user_id.lock().expect("poisoned") else {
            error!("on_registration_skip_all: no user_id");
            return Task::none();
        };

        // Send DeviceRegistered with registered=false for all remaining devices
        for fingerprint in &self.views.registration.user_devices {
            debug!("on_registration_skip_all: skipping device {}", fingerprint);
            let infos = liana_connect::ws_business::RegistrationInfos {
                user: user_id,
                fingerprint: *fingerprint,
                registered: false, // Skipped
                registered_alias: None,
                proof_of_registration: None,
            };
            self.backend.device_registered(wallet_id, infos);
        }

        // Clear the device list
        self.views.registration.user_devices.clear();

        Task::none()
    }
}
