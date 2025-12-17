use super::{app::AppState, message::Msg, views, State, View};
use crate::backend::{Backend, Error, Notification, UserRole, Wallet, WalletStatus};
use iced::Task;
use liana_connect::{Key, PolicyTemplate, SpendingPath, Timelock};
use liana_ui::widget::text_input;
use uuid::Uuid;

/// Derive the user's role for a specific wallet based on wallet data
fn derive_user_role(wallet: &Wallet, current_user_email: &str) -> UserRole {
    let email_lower = current_user_email.to_lowercase();
    // Check if user is wallet owner
    if wallet.owner.email.to_lowercase() == email_lower {
        return UserRole::Owner;
    }
    // Check if user is a participant (has keys with matching email)
    if let Some(template) = &wallet.template {
        for key in template.keys.values() {
            if key.email.to_lowercase() == email_lower {
                return UserRole::Participant;
            }
        }
    }
    // Default to WSManager (platform admin)
    UserRole::WSManager
}

// Update routing logic
impl State {
    #[rustfmt::skip]
    pub fn update(&mut self, message: Msg) -> Task<Msg> {
        println!("{message:?}");
        match message {
            // Login/Auth
            Msg::LoginUpdateEmail(email) => self.views.login.on_update_email(email),
            Msg::LoginUpdateCode(code) => self.on_login_update_code(code),
            Msg::LoginSendToken => self.on_login_send_token(),
            Msg::LoginResendToken => self.on_login_resend_token(),
            Msg::LoginSendAuthCode => self.on_login_send_auth_code(),

            // Org management
            Msg::OrgSelected(id) => self.on_org_selected(id),
            Msg::OrgWalletSelected(id) => self.on_org_wallet_selected(id),
            Msg::OrgCreateNewWallet => self.on_org_create_new_wallet(),

            // Wallet selection
            Msg::WalletSelectToggleHideFinalized(checked) => {
                self.views.wallet_select.hide_finalized = checked;
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
            Msg::TemplateAddKeyToPrimary(id) => self.on_template_add_key_to_primary(id),
            Msg::TemplateDelKeyFromPrimary(id) => self.on_template_del_key_from_primary(id),
            Msg::TemplateAddKeyToSecondary(i, id) => self.on_template_add_key_to_secondary(i, id),
            Msg::TemplateDelKeyFromSecondary(i, id) => self.on_template_del_key_from_secondary(i, id),
            Msg::TemplateAddSecondaryPath => self.on_template_add_secondary_path(),
            Msg::TemplateDeleteSecondaryPath(i) => self.on_template_delete_secondary_path(i),
            Msg::TemplateEditPath(primary, i) => self.on_template_edit_path(primary, i),
            Msg::TemplateSavePath => self.on_template_save_path(),
            Msg::TemplateValidate => self.on_template_validate(),

            // Navigation
            Msg::NavigateToHome => self.on_navigate_to_home(),
            Msg::NavigateToPaths => self.on_navigate_to_paths(),
            Msg::NavigateToKeys => self.on_navigate_to_keys(),
            Msg::NavigateToOrgSelect => self.on_navigate_to_org_select(),
            Msg::NavigateToWalletSelect => self.on_navigate_to_wallet_select(),
            Msg::NavigateBack => return self.on_navigate_back(),

            // Backend
            Msg::BackendNotif(notif) => return self.on_backend_notif(notif),
            Msg::BackendDisconnected => self.on_backend_disconnected(),

            // Warnings
            Msg::WarningShowModal(title, message) => self.on_warning_show_modal(title, message),
            Msg::WarningCloseModal => self.on_warning_close_modal(),
        }
        Task::none()
    }

    #[rustfmt::skip]
    fn on_backend_notif(&mut self, response: Notification) -> Task<Msg> {
        match response {
            Notification::Connected => self.on_backend_connected(),
            Notification::Disconnected => self.on_backend_disconnected(),
            // Notification::Orgs(_) => self.on_backend_orgs(),
            Notification::AuthCodeSent => return self.on_backend_auth_code_sent(),
            Notification::InvalidEmail => self.on_backend_invalid_email(),
            Notification::AuthCodeFail => self.on_backend_auth_code_fail(),
            Notification::LoginSuccess => self.on_backend_login_success(),
            Notification::LoginFail => self.on_backend_login_fail(),
            Notification::Error(error) => self.on_backend_error(error),
            Notification::Org(_) => todo!(),
            Notification::Wallet(_) => todo!(),
            Notification::User(_) => todo!(),
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
        println!("State::on_login_send_token()");
        let email = self.views.login.email.form.value.clone();
        if self.views.login.email.form.valid && !email.is_empty() {
            self.views.login.email.processing = true;
            self.backend.auth_request(email);
        }
    }
    fn on_login_resend_token(&mut self) {
        // FIXME: should we "rate limit" here or only on server?
        self.on_login_send_token();
    }
    fn on_login_send_auth_code(&mut self) {
        // Trim the code value before sending to backend
        let code = self.views.login.code.form.value.trim().to_string();
        if !code.is_empty() {
            self.backend.auth_code(code);
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
    fn on_org_wallet_selected(&mut self, id: Uuid) {
        // Get wallet and check access before loading
        let (wallet_status, user_role) = {
            let current_email = &self.views.login.email.form.value;
            if let Some(org_id) = self.app.selected_org {
                if let Some(org) = self.backend.get_org(org_id) {
                    if let Some(wallet) = org.wallets.get(&id) {
                        let role = derive_user_role(wallet, current_email);
                        (Some(wallet.status.clone()), Some(role))
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        };

        // Check access based on role + status
        if let (Some(status), Some(role)) = (&wallet_status, &user_role) {
            match (status, role) {
                // Draft + Participant -> Access Denied
                (WalletStatus::Created | WalletStatus::Drafted, UserRole::Participant) => {
                    self.on_warning_show_modal(
                        "Access Denied",
                        "Participants cannot access Draft wallets. Please wait for the wallet to be validated.",
                    );
                    return;
                }
                // All other combinations proceed
                _ => {}
            }
        }

        // Load wallet template into AppState
        self.app.selected_wallet = Some(id);
        if let Some(org_id) = self.app.selected_org {
            if let Some(org) = self.backend.get_org(org_id) {
                if let Some(wallet) = org.wallets.get(&id) {
                    let wallet_template = wallet.template.clone().unwrap_or(PolicyTemplate::new());
                    self.app.current_wallet_template = Some(wallet_template.clone());
                    // Convert template to AppState
                    let app_state: AppState = wallet_template.clone().into();
                    self.app.keys = app_state.keys;
                    self.app.primary_path = app_state.primary_path;
                    self.app.secondary_paths = app_state.secondary_paths;
                    self.app.next_key_id = app_state.next_key_id;
                }
            }
        }

        // Route based on wallet status
        match wallet_status {
            Some(WalletStatus::Validated) => {
                // Validated -> Add Key Information (xpub entry)
                self.current_view = View::Xpub;
            }
            Some(WalletStatus::Finalized) => {
                // Final -> Signal exit to Liana Lite
                // exit_maybe() will return NextState::LoginLianaLite
                self.app.exit_to_liana_lite = true;
            }
            _ => {
                // Draft + (WSManager|Owner) -> Edit Template
                self.current_view = View::WalletEdit;
            }
        }
    }
    fn on_org_create_new_wallet(&mut self) {
        // Create a new blank wallet template
        let new_template = PolicyTemplate::new();
        self.app.current_wallet_template = Some(new_template.clone());
        // Convert to AppState
        let app_state: AppState = new_template.into();
        self.app.keys = app_state.keys;
        self.app.primary_path = app_state.primary_path;
        self.app.secondary_paths = app_state.secondary_paths;
        self.app.next_key_id = app_state.next_key_id;
        self.current_view = View::WalletEdit;
    }
}

// Key management
impl State {
    fn on_key_add(&mut self) {
        // Create an empty key with default values
        let key = Key {
            id: self.app.next_key_id,
            alias: String::new(),
            description: String::new(),
            email: String::new(),
            key_type: liana_connect::KeyType::Internal,
            xpub: None,
        };
        self.app.keys.insert(self.app.next_key_id, key);
        self.app.next_key_id = self.app.next_key_id.wrapping_add(1);
    }

    fn on_key_edit(&mut self, key_id: u8) {
        if let Some(key) = self.app.keys.get(&key_id) {
            self.views.keys.edit_key = Some(views::EditKeyModalState {
                key_id,
                alias: key.alias.clone(),
                description: key.description.clone(),
                email: key.email.clone(),
                key_type: key.key_type,
            });
        }
    }

    fn on_key_delete(&mut self, key_id: u8) {
        // Remove key from all paths first
        self.app.primary_path.key_ids.retain(|&id| id != key_id);
        for (path, _) in &mut self.app.secondary_paths {
            path.key_ids.retain(|&id| id != key_id);
        }
        // Then remove the key itself
        self.app.keys.remove(&key_id);
    }

    fn on_key_save(&mut self) {
        // Only handle editing now (adding is done directly in AddKey)
        if let Some(modal_state) = &self.views.keys.edit_key {
            if let Some(key) = self.app.keys.get_mut(&modal_state.key_id) {
                key.alias = modal_state.alias.clone();
                key.description = modal_state.description.clone();
                key.email = modal_state.email.clone();
                key.key_type = modal_state.key_type;
                self.views.keys.edit_key = None;
            }
        }
    }
}

// Template management
impl State {
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
        if let Some((path, _)) = self.app.secondary_paths.get_mut(path_index) {
            if !path.contains_key(key_id) {
                path.key_ids.push(key_id);
            }
        }
    }

    fn on_template_del_key_from_secondary(&mut self, path_index: usize, key_id: u8) {
        if let Some((path, _)) = self.app.secondary_paths.get_mut(path_index) {
            path.key_ids.retain(|&id| id != key_id);
            // Adjust threshold_n if needed
            let m = path.key_ids.len();
            if path.threshold_n as usize > m && m > 0 {
                path.threshold_n = m as u8;
            }
        }
    }

    fn on_template_add_secondary_path(&mut self) {
        // Create a new secondary path with default values
        // threshold_n defaults to 1, timelock defaults to 0 blocks (must be set later)
        let path = SpendingPath::new(false, 1, Vec::new());
        let timelock = Timelock::new(0);
        self.app.secondary_paths.push((path, timelock));
    }

    fn on_template_delete_secondary_path(&mut self, path_index: usize) {
        if path_index < self.app.secondary_paths.len() {
            self.app.secondary_paths.remove(path_index);
        }
    }

    fn on_template_edit_path(&mut self, is_primary: bool, path_index: Option<usize>) {
        if is_primary {
            self.views.paths.edit_path = Some(views::EditPathModalState {
                is_primary: true,
                path_index: None,
                threshold: self.app.primary_path.threshold_n.to_string(),
                timelock: None,
            });
        } else if let Some(index) = path_index {
            if let Some((path, timelock)) = self.app.secondary_paths.get(index) {
                self.views.paths.edit_path = Some(views::EditPathModalState {
                    is_primary: false,
                    path_index: Some(index),
                    threshold: path.threshold_n.to_string(),
                    timelock: Some(timelock.blocks.to_string()),
                });
            }
        }
    }

    fn on_template_save_path(&mut self) {
        if let Some(modal_state) = &self.views.paths.edit_path {
            // Handle threshold
            if let Ok(threshold_n) = modal_state.threshold.parse::<u8>() {
                if modal_state.is_primary {
                    let m = self.app.primary_path.key_ids.len();
                    if threshold_n > 0 && (threshold_n as usize) <= m && m > 0 {
                        self.app.primary_path.threshold_n = threshold_n;
                    }
                } else if let Some(path_index) = modal_state.path_index {
                    if let Some((path, _)) = self.app.secondary_paths.get_mut(path_index) {
                        let m = path.key_ids.len();
                        if threshold_n > 0 && (threshold_n as usize) <= m && m > 0 {
                            path.threshold_n = threshold_n;
                        }
                    }
                }
            }

            // Handle timelock (only for secondary paths)
            if let (false, Some(path_index), Some(blocks_str)) = (
                modal_state.is_primary,
                modal_state.path_index,
                &modal_state.timelock,
            ) {
                if let Ok(blocks) = blocks_str.parse::<u64>() {
                    if let Some((_, timelock)) = self.app.secondary_paths.get_mut(path_index) {
                        timelock.blocks = blocks;
                    }
                }
            }

            self.views.paths.edit_path = None;
        }
    }

    fn on_template_validate(&mut self) {
        if self.is_template_valid() {
            // TODO: send template to server
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

    fn on_navigate_to_paths(&mut self) {
        self.current_view = View::Paths;
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
            _ => Task::none(),
        }
    }
}

// Backend updates
impl State {
    fn on_backend_connected(&mut self) {
        // TODO: ?
    }

    fn on_backend_orgs(&mut self) {
        // TODO: ?
    }

    fn on_backend_org(&mut self) {
        // TODO: ?
    }

    fn on_backend_wallet(&mut self) {
        // TODO: ?
    }

    fn on_backend_user(&mut self) {
        // TODO: ?
    }

    fn on_backend_auth_code_sent(&mut self) -> Task<Msg> {
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

        // Set the token for the WS connection
        // TODO: In production, the token should come from the auth response
        self.backend.set_token("auth-token".to_string());

        // Mark that we're intentionally reconnecting (old channel will close)
        // NOTE: Reconnection is now handled by BusinessInstaller
        self.app.reconnecting = true;
    }

    fn on_backend_login_fail(&mut self) {
        self.views.login.code.form.valid = false;
        self.views.login.code.form.warning = Some("Login fail!");
        self.views.login.code.processing = false;
    }

    fn on_backend_error(&mut self, error: Error) {
        if error.show_warning() {
            self.on_warning_show_modal("Backend error", error.to_string());
        }
    }

    fn on_backend_disconnected(&mut self) {
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
}
