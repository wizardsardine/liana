use super::{app::AppState, message::Msg, views, State, View};
use crate::backend::{Backend, Error, Notification, UserRole, Wallet, WalletStatus, BACKEND_RECV};
use crate::client::{BACKEND_URL, PROTOCOL_VERSION};
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

            // Warnings
            Msg::WarningShowModal(title, message) => self.on_warning_show_modal(title, message),
            Msg::WarningCloseModal => self.on_warning_close_modal(),

            // Logout
            Msg::Logout => return self.on_logout(),
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

        // Store user role for the selected wallet
        self.app.current_user_role = user_role;

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
        // Open modal for creating a new key
        let key_id = self.app.next_key_id;
        self.views.keys.edit_key = Some(views::EditKeyModalState {
            key_id,
            alias: String::new(),
            description: String::new(),
            email: String::new(),
            key_type: liana_connect::KeyType::Internal,
            is_new: true,
        });
    }

    fn on_key_edit(&mut self, key_id: u8) {
        if let Some(key) = self.app.keys.get(&key_id) {
            self.views.keys.edit_key = Some(views::EditKeyModalState {
                key_id,
                alias: key.alias.clone(),
                description: key.description.clone(),
                email: key.email.clone(),
                key_type: key.key_type,
                is_new: false,
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
        // Close modal if it was open for this key
        if let Some(modal_state) = &self.views.keys.edit_key {
            if modal_state.key_id == key_id {
                self.views.keys.edit_key = None;
            }
        }

        // Auto-save for WSManager: push changes to server with status = Drafted
        if matches!(self.app.current_user_role, Some(UserRole::WSManager)) {
            if let Some(wallet) = self.build_wallet_from_app_state(WalletStatus::Drafted) {
                self.backend.edit_wallet(wallet);
            }
        }
    }

    fn on_key_save(&mut self) {
        if let Some(modal_state) = &self.views.keys.edit_key {
            if modal_state.is_new {
                // Creating a new key
                let key = Key {
                    id: modal_state.key_id,
                    alias: modal_state.alias.clone(),
                    description: modal_state.description.clone(),
                    email: modal_state.email.clone(),
                    key_type: modal_state.key_type,
                    xpub: None,
                };
                self.app.keys.insert(modal_state.key_id, key);
                self.app.next_key_id = self.app.next_key_id.wrapping_add(1);
            } else {
                // Editing existing key
                if let Some(key) = self.app.keys.get_mut(&modal_state.key_id) {
                    key.alias = modal_state.alias.clone();
                    key.description = modal_state.description.clone();
                    key.email = modal_state.email.clone();
                    key.key_type = modal_state.key_type;
                }
            }
            self.views.keys.edit_key = None;

            // Auto-save for WSManager: push changes to server with status = Drafted
            if matches!(self.app.current_user_role, Some(UserRole::WSManager)) {
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
            owner: wallet.owner.clone(),
            status,
            template: Some(template),
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
        self.app.sort_secondary_paths();
    }

    fn on_template_delete_secondary_path(&mut self, path_index: usize) {
        if path_index < self.app.secondary_paths.len() {
            self.app.secondary_paths.remove(path_index);
            
            // Auto-save for WSManager: push changes to server with status = Drafted
            if matches!(self.app.current_user_role, Some(UserRole::WSManager)) {
                if let Some(wallet) = self.build_wallet_from_app_state(WalletStatus::Drafted) {
                    self.backend.edit_wallet(wallet);
                }
            }
        }
    }

    fn on_template_edit_path(&mut self, is_primary: bool, path_index: Option<usize>) {
        use views::path::TimelockUnit;

        if is_primary {
            self.views.paths.edit_path = Some(views::EditPathModalState {
                is_primary: true,
                path_index: None,
                selected_key_ids: self.app.primary_path.key_ids.clone(),
                threshold: self.app.primary_path.threshold_n.to_string(),
                timelock_value: None,
                timelock_unit: TimelockUnit::default(),
            });
        } else if let Some(index) = path_index {
            if let Some((path, timelock)) = self.app.secondary_paths.get(index) {
                // Determine the best unit for display (largest unit that divides evenly)
                let blocks = timelock.blocks;
                let (unit, value) = if blocks >= TimelockUnit::Months.blocks_per_unit()
                    && blocks % TimelockUnit::Months.blocks_per_unit() == 0
                {
                    (TimelockUnit::Months, TimelockUnit::Months.from_blocks(blocks))
                } else if blocks >= TimelockUnit::Days.blocks_per_unit()
                    && blocks % TimelockUnit::Days.blocks_per_unit() == 0
                {
                    (TimelockUnit::Days, TimelockUnit::Days.from_blocks(blocks))
                } else {
                    (TimelockUnit::Hours, TimelockUnit::Hours.from_blocks(blocks))
                };

                self.views.paths.edit_path = Some(views::EditPathModalState {
                    is_primary: false,
                    path_index: Some(index),
                    selected_key_ids: path.key_ids.clone(),
                    threshold: path.threshold_n.to_string(),
                    timelock_value: Some(value.to_string()),
                    timelock_unit: unit,
                });
            }
        }
    }

    fn on_template_new_path_modal(&mut self) {
        use views::path::TimelockUnit;

        // Open modal for creating a new recovery path (all keys deselected)
        self.views.paths.edit_path = Some(views::EditPathModalState {
            is_primary: false,
            path_index: None, // None indicates a new path
            selected_key_ids: Vec::new(),
            threshold: String::new(),
            timelock_value: Some("1".to_string()),
            timelock_unit: TimelockUnit::Days,
        });
    }

    fn on_template_save_path(&mut self) {
        if let Some(modal_state) = &self.views.paths.edit_path {
            let selected_keys = modal_state.selected_key_ids.clone();
            let selected_count = selected_keys.len();

            if modal_state.is_primary {
                // Apply key changes to primary path
                self.app.primary_path.key_ids = selected_keys;

                // Handle threshold - parse and validate
                if let Ok(threshold_n) = modal_state.threshold.parse::<u8>() {
                    if threshold_n > 0 && (threshold_n as usize) <= selected_count && selected_count > 0 {
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
                if let Some((path, timelock)) = self.app.secondary_paths.get_mut(path_index) {
                    // Apply key changes to secondary path
                    path.key_ids = selected_keys;

                    // Handle threshold - parse and validate
                    if let Ok(threshold_n) = modal_state.threshold.parse::<u8>() {
                        if threshold_n > 0 && (threshold_n as usize) <= selected_count && selected_count > 0 {
                            path.threshold_n = threshold_n;
                        } else if selected_count > 0 {
                            // Default to all keys required if threshold invalid
                            path.threshold_n = selected_count as u8;
                        }
                    } else if selected_count > 0 {
                        // Default to all keys required if parse fails
                        path.threshold_n = selected_count as u8;
                    }

                    // Handle timelock (only for secondary paths)
                    if let Some(value_str) = &modal_state.timelock_value {
                        if let Ok(value) = value_str.parse::<u64>() {
                            timelock.blocks = modal_state.timelock_unit.to_blocks(value);
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
                        modal_state.timelock_unit.to_blocks(value)
                    } else {
                        144 // Default 1 day
                    }
                } else {
                    144 // Default 1 day
                };

                let new_path = liana_connect::SpendingPath::new(false, threshold_n, selected_keys);
                let new_timelock = liana_connect::Timelock::new(blocks);
                self.app.secondary_paths.push((new_path, new_timelock));
                self.app.sort_secondary_paths();
            }

            self.views.paths.edit_path = None;
            
            // Auto-save for WSManager: push changes to server with status = Drafted
            if matches!(self.app.current_user_role, Some(UserRole::WSManager)) {
                if let Some(wallet) = self.build_wallet_from_app_state(WalletStatus::Drafted) {
                    self.backend.edit_wallet(wallet);
                }
            }
        }
    }

    fn on_template_validate(&mut self) {
        // Only Owner can validate
        if !matches!(self.app.current_user_role, Some(UserRole::Owner)) {
            return;
        }
        
        if self.is_template_valid() {
            // Push template to server with status = Validated
            if let Some(wallet) = self.build_wallet_from_app_state(WalletStatus::Validated) {
                self.backend.edit_wallet(wallet);
            }
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
        self.app.reconnecting = true;

        // Connect WebSocket immediately after login success
        // This will establish the connection now that we have a token
        let recv = self.backend.connect_ws(BACKEND_URL.to_string(), PROTOCOL_VERSION);
        
        // Update the global receiver for the subscription
        if let Some(receiver) = recv {
            *BACKEND_RECV.lock().expect("poisoned") = Some(receiver);
        }
        // Note: If connection fails, an Error notification will be sent
        // which will be handled by on_backend_error()
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

    fn on_logout(&mut self) -> Task<Msg> {
        // Call backend logout to clear token, close connection, and remove cache
        self.backend.logout();

        // Reset login state to EmailEntry
        self.views.login.current = views::LoginState::EmailEntry;

        // Clear email and code form values
        self.views.login.email.form.value = String::new();
        self.views.login.email.form.valid = false;
        self.views.login.email.form.warning = None;
        self.views.login.email.processing = false;
        self.views.login.code.form.value = String::new();
        self.views.login.code.form.valid = false;
        self.views.login.code.form.warning = None;
        self.views.login.code.processing = false;

        // Reset application state
        self.app.selected_org = None;
        self.app.selected_wallet = None;
        self.app.current_wallet_template = None;
        self.app.reconnecting = false;
        self.app.exit_to_liana_lite = false;

        // Navigate to login view
        self.current_view = View::Login;

        // Focus email input
        text_input::focus("login_email")
    }
}
