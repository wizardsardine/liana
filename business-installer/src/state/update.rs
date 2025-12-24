use super::{app::AppState, message::Msg, views, State, View};
use crate::{
    backend::{Backend, Error, Notification, UserRole, Wallet, WalletStatus},
    client::{PROTOCOL_VERSION, WS_URL},
    state::views::modals::{ConflictModalState, ConflictType},
};
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

            // Account selection (cached token login)
            Msg::AccountSelectConnect(email) => return self.on_account_select_connect(email),
            Msg::AccountSelectNewEmail => return self.on_account_select_new_email(),

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

            // Hardware Wallets
            Msg::HardwareWallets(msg) => return self.on_hw_message(msg),

            // Xpub management
            Msg::XpubSelectKey(key_id) => self.on_xpub_select_key(key_id),
            Msg::XpubUpdateInput(input) => self.on_xpub_update_input(input),
            Msg::XpubSelectSource(source) => self.on_xpub_select_source(source),
            Msg::XpubSelectDevice(fingerprint) => self.on_xpub_select_device(fingerprint),
            Msg::XpubFetchFromDevice(fingerprint, account) => return self.on_xpub_fetch_from_device(fingerprint, account),
            Msg::XpubLoadFromFile => return self.on_xpub_load_from_file(),
            Msg::XpubFileLoaded(result) => self.on_xpub_file_loaded(result),
            Msg::XpubPaste => return self.on_xpub_paste(),
            Msg::XpubUpdateAccount(account) => self.on_xpub_update_account(account),
            Msg::XpubSave => return self.on_xpub_save(),
            Msg::XpubClear => return self.on_xpub_clear(),
            Msg::XpubCancelModal => self.on_xpub_cancel_modal(),
            Msg::XpubToggleOptions => self.on_xpub_toggle_options(),

            // Warnings
            Msg::WarningShowModal(title, message) => self.on_warning_show_modal(title, message),
            Msg::WarningCloseModal => self.on_warning_close_modal(),

            // Conflict resolution
            Msg::ConflictReload => return self.on_conflict_reload(),
            Msg::ConflictKeepLocal => self.on_conflict_keep_local(),
            Msg::ConflictDismiss => self.on_conflict_dismiss(),

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
            Notification::AuthCodeSent => return self.on_backend_auth_code_sent(),
            Notification::InvalidEmail => self.on_backend_invalid_email(),
            Notification::AuthCodeFail => self.on_backend_auth_code_fail(),
            Notification::LoginSuccess => self.on_backend_login_success(),
            Notification::LoginFail => self.on_backend_login_fail(),
            Notification::Error(error) => self.on_backend_error(error),
            Notification::Org(_) => { /* Cache already updated, no action needed */ }
            Notification::Wallet(wallet_id) => return self.on_backend_wallet(wallet_id),
            Notification::User(_) => { /* Cache already updated, no action needed */ }
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
                WS_URL.to_string(),
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
            if let (WalletStatus::Created | WalletStatus::Drafted, UserRole::Participant) =
                (status, role)
            {
                self.on_warning_show_modal(
                    "Access Denied",
                    "Participants cannot access Draft wallets. Please wait for the wallet to be validated.",
                );
                return;
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

        // Auto-save for WSManager/Owner: push changes to server with status = Drafted
        if matches!(
            self.app.current_user_role,
            Some(UserRole::WSManager) | Some(UserRole::Owner)
        ) {
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

            // Auto-save for WSManager/Owner: push changes to server with status = Drafted
            if matches!(
                self.app.current_user_role,
                Some(UserRole::WSManager) | Some(UserRole::Owner)
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

            // Auto-save for WSManager/Owner: push changes to server with status = Drafted
            if matches!(
                self.app.current_user_role,
                Some(UserRole::WSManager) | Some(UserRole::Owner)
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
                    (
                        TimelockUnit::Months,
                        TimelockUnit::Months.from_blocks(blocks),
                    )
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
                if let Some((path, timelock)) = self.app.secondary_paths.get_mut(path_index) {
                    // Apply key changes to secondary path
                    path.key_ids = selected_keys;

                    // Handle threshold - parse and validate
                    if let Ok(threshold_n) = modal_state.threshold.parse::<u8>() {
                        if threshold_n > 0
                            && (threshold_n as usize) <= selected_count
                            && selected_count > 0
                        {
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

            // Auto-save for WSManager/Owner: push changes to server with status = Drafted
            if matches!(
                self.app.current_user_role,
                Some(UserRole::WSManager) | Some(UserRole::Owner)
            ) {
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
        // Check if this connection came from cached token login (AccountSelect flow)
        if self.views.login.account_select.processing {
            // Success! Transition to authenticated state
            self.views.login.current = views::LoginState::Authenticated;
            self.views.login.account_select.processing = false;
            self.views.login.account_select.selected_email = None;
            self.current_view = View::OrgSelect;
        }
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

        // Token was already set by auth_code() after successful verify

        // Mark that we're intentionally reconnecting (old channel will close)
        self.app.reconnecting = true;

        // Connect WebSocket immediately after login success
        // This will establish the connection now that we have a token
        self.backend.connect_ws(
            WS_URL.to_string(),
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
        // Check if error occurred during cached token connection
        if self.views.login.account_select.processing {
            self.handle_cached_token_connection_failure();
            return;
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
        // Get the email of the logged-in account to clear from cache
        let logged_in_email = self.views.login.email.form.value.clone();

        // Call backend logout to clear token, close connection, and remove cache
        self.backend.logout();

        // Clear this account from the cache
        if !logged_in_email.is_empty() {
            self.backend
                .clear_invalid_tokens(&[logged_in_email.clone()]);

            // Also remove from account_select list
            self.views
                .login
                .account_select
                .accounts
                .retain(|a| a.email != logged_in_email);
        }

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
        self.app.reconnecting = false;
        self.app.exit_to_liana_lite = false;

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
        // Only relevant if this is the currently selected wallet
        if self.app.selected_wallet != Some(wallet_id) {
            return Task::none();
        }

        // Get the updated wallet from cache
        let Some(wallet) = self.backend.get_wallet(wallet_id) else {
            return Task::none();
        };

        // Check for conflicts with open modals before updating state
        self.check_modal_conflicts(&wallet, wallet_id);

        // If no conflict modal was shown, refresh local state
        if self.views.modals.conflict.is_none() {
            self.load_wallet_into_app_state(&wallet);
        }

        // Close xpub modal if open (edit was successful and wallet updated)
        // The modal should be closed after successful save/clear operations
        if self.views.xpub.modal.is_some() {
            self.views.xpub.close_modal();
        }

        Task::none()
    }

    /// Check for conflicts between open modals and the new wallet state
    fn check_modal_conflicts(&mut self, wallet: &Wallet, wallet_id: Uuid) {
        let new_template = wallet.template.as_ref();

        // Check if key modal is open
        if let Some(modal) = &self.views.keys.edit_key {
            if !modal.is_new {
                let key_id = modal.key_id;
                // Check if the key still exists in the new wallet
                let key_exists = new_template
                    .map(|t| t.keys.contains_key(&key_id))
                    .unwrap_or(false);

                if !key_exists {
                    // Key was deleted
                    self.views.keys.edit_key = None;
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
        if let Some(modal) = &mut self.views.paths.edit_path {
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
                        self.views.paths.edit_path = None;
                        self.views.modals.conflict = Some(ConflictModalState {
                            conflict_type: ConflictType::PathDeleted,
                            title: "Path Deleted".to_string(),
                            message: "The path you were editing was deleted by another user."
                                .to_string(),
                        });
                        return;
                    }

                    // Check if secondary path was modified
                    let (server_path, server_timelock) = &template.secondary_paths[path_index];
                    let modal_keys: std::collections::HashSet<u8> =
                        modal.selected_key_ids.iter().copied().collect();
                    let server_keys: std::collections::HashSet<u8> =
                        server_path.key_ids.iter().copied().collect();
                    let modal_threshold = modal.threshold.parse::<u8>().unwrap_or(0);
                    // Convert modal timelock from display units to blocks for comparison
                    let modal_timelock_blocks = modal
                        .timelock_value
                        .as_ref()
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|v| modal.timelock_unit.to_blocks(v))
                        .unwrap_or(0);

                    if modal_keys != server_keys
                        || modal_threshold != server_path.threshold_n
                        || modal_timelock_blocks != server_timelock.blocks
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
                                // Update the modal with server data
                                if let Some(modal) = &mut self.views.keys.edit_key {
                                    modal.alias = key.alias.clone();
                                    modal.description = key.description.clone();
                                    modal.email = key.email.clone();
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
                                if let Some(modal) = &mut self.views.paths.edit_path {
                                    modal.selected_key_ids = template.primary_path.key_ids.clone();
                                    modal.threshold = template.primary_path.threshold_n.to_string();
                                }
                                self.app.primary_path = template.primary_path.clone();
                            } else if let Some(idx) = path_index {
                                if let Some((path, timelock)) = template.secondary_paths.get(idx) {
                                    if let Some(modal) = &mut self.views.paths.edit_path {
                                        modal.selected_key_ids = path.key_ids.clone();
                                        modal.threshold = path.threshold_n.to_string();
                                        modal.timelock_value = Some(timelock.blocks.to_string());
                                    }
                                    if idx < self.app.secondary_paths.len() {
                                        self.app.secondary_paths[idx] =
                                            (path.clone(), timelock.clone());
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
    /// Handle hardware wallet messages
    fn on_hw_message(&mut self, msg: liana_gui::hw::HardwareWalletMessage) -> Task<Msg> {
        let Some(hw) = self.hw.as_mut() else {
            return Task::none();
        };

        // Update the hardware wallet state
        match hw.update(msg) {
            Ok(task) => {
                // Update modal state after list updates
                if let Some(modal) = self.views.xpub.modal_mut() {
                    // Build list of (fingerprint, device_name) for supported devices
                    modal.hw_devices = hw
                        .list
                        .iter()
                        .filter_map(|hw| match hw {
                            liana_gui::hw::HardwareWallet::Supported {
                                fingerprint,
                                kind,
                                version,
                                ..
                            } => {
                                let name = if let Some(v) = version {
                                    format!("{} {}", kind, v)
                                } else {
                                    kind.to_string()
                                };
                                Some((*fingerprint, name))
                            }
                            _ => None,
                        })
                        .collect();
                }

                // Map resulting task back to our message type
                task.map(Msg::HardwareWallets)
            }
            Err(e) => {
                // Show error in warning modal
                Task::done(Msg::WarningShowModal(
                    "Hardware Wallet Error".to_string(),
                    e.to_string(),
                ))
            }
        }
    }
}

// Xpub management handlers
impl State {
    /// Open xpub entry modal for a key
    fn on_xpub_select_key(&mut self, key_id: u8) {
        if let Some(key) = self.app.keys.get(&key_id) {
            self.views
                .xpub
                .open_modal(key_id, key.alias.clone(), key.xpub.clone());
        }
    }

    /// Update xpub input text
    fn on_xpub_update_input(&mut self, input: String) {
        if let Some(modal) = self.views.xpub.modal_mut() {
            modal.update_input(input);
        }
    }

    /// Switch xpub source tab
    fn on_xpub_select_source(&mut self, source: views::XpubSource) {
        if let Some(modal) = self.views.xpub.modal_mut() {
            modal.select_source(source);
        }
    }

    /// Select hardware wallet device
    fn on_xpub_select_device(&mut self, fingerprint: miniscript::bitcoin::bip32::Fingerprint) {
        if let Some(modal) = self.views.xpub.modal_mut() {
            modal.select_device(fingerprint);
        }
    }

    /// Fetch xpub from hardware wallet device
    fn on_xpub_fetch_from_device(
        &mut self,
        fingerprint: miniscript::bitcoin::bip32::Fingerprint,
        account: miniscript::bitcoin::bip32::ChildNumber,
    ) -> Task<Msg> {
        // Set processing state
        if let Some(modal) = self.views.xpub.modal_mut() {
            modal.set_processing(true);
            modal.clear_error();
        }

        // Find the device with matching fingerprint in the hardware wallet list
        let device = self.hw.as_ref().and_then(|hw| {
            hw.list
                .iter()
                .find(|hw| hw.fingerprint() == Some(fingerprint))
                .cloned()
        });

        match device {
            Some(liana_gui::hw::HardwareWallet::Supported { device, .. }) => {
                // Build derivation path: m/48'/network'/account'/2'
                use miniscript::bitcoin::bip32::{ChildNumber, DerivationPath};
                use miniscript::descriptor::{DescriptorPublicKey, DescriptorXKey, Wildcard};

                // Get network from HW subscription (it's already set correctly)
                // TODO: Get actual network from wallet or backend config
                let network = liana::miniscript::bitcoin::Network::Bitcoin;
                let network_idx = if network == liana::miniscript::bitcoin::Network::Bitcoin {
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

                let device_clone = device.clone();
                let fp = fingerprint;

                // Fetch xpub by spawning a thread with Tokio runtime
                // This avoids "no reactor running" panic with Iced's ThreadPool executor
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    let result = rt.block_on(async {
                        // Get xpub from device
                        match device_clone.get_extended_pubkey(&derivation_path).await {
                            Ok(xkey) => {
                                // Convert to DescriptorPublicKey
                                Ok(DescriptorPublicKey::XPub(DescriptorXKey {
                                    origin: Some((fp, derivation_path.clone())),
                                    derivation_path: DerivationPath::master(),
                                    wildcard: Wildcard::None,
                                    xkey,
                                }))
                            }
                            Err(e) => Err(e),
                        }
                    });
                    let _ = tx.send(result);
                });

                // Poll the channel for the result
                Task::perform(
                    async move {
                        // Block until result is available
                        rx.recv().unwrap_or_else(|_| {
                            Err(async_hwi::Error::Device(
                                "Failed to receive result from hardware wallet thread".to_string(),
                            ))
                        })
                    },
                    |result| {
                        match result {
                            Ok(xpub) => {
                                // Success - populate input with xpub string
                                Msg::XpubFileLoaded(Ok(xpub.to_string()))
                            }
                            Err(e) => {
                                // Error - show error message
                                Msg::XpubFileLoaded(Err(format!(
                                    "Failed to fetch from device: {}",
                                    e
                                )))
                            }
                        }
                    },
                )
            }
            Some(liana_gui::hw::HardwareWallet::Locked { .. }) => {
                if let Some(modal) = self.views.xpub.modal_mut() {
                    modal.set_error("Device is locked. Please unlock it first.".to_string());
                }
                Task::none()
            }
            Some(liana_gui::hw::HardwareWallet::Unsupported { .. }) => {
                if let Some(modal) = self.views.xpub.modal_mut() {
                    modal.set_error("Device is not supported".to_string());
                }
                Task::none()
            }
            None => {
                // Device not found
                if let Some(modal) = self.views.xpub.modal_mut() {
                    modal.set_error("Hardware wallet not found".to_string());
                }
                Task::none()
            }
        }
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
                            Ok(xpub)
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
                    Ok(xpub) => Msg::XpubFileLoaded(Ok(xpub)),
                    Err(e) if !e.is_empty() => Msg::XpubFileLoaded(Err(e)),
                    Err(_) => Msg::XpubFileLoaded(Err(String::new())), // User cancelled
                }
            },
        )
    }

    /// Handle file loaded result
    fn on_xpub_file_loaded(&mut self, result: Result<String, String>) {
        if let Some(modal) = self.views.xpub.modal_mut() {
            match result {
                Ok(content) => {
                    modal.update_input(content);
                }
                Err(error) if !error.is_empty() => {
                    // Only show error if it's not empty (empty means user cancelled)
                    modal.set_error(format!("Failed to load file: {}", error));
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
                Msg::XpubUpdateInput(xpub)
            } else {
                Msg::XpubFileLoaded(Err("Clipboard is empty".to_string()))
            }
        })
    }

    /// Update derivation account for HW wallet
    fn on_xpub_update_account(&mut self, account: miniscript::bitcoin::bip32::ChildNumber) {
        if let Some(modal) = self.views.xpub.modal_mut() {
            modal.update_account(account);
        }
    }

    /// Save xpub to backend
    fn on_xpub_save(&mut self) -> Task<Msg> {
        // Validate and save xpub
        if let Some(modal) = &mut self.views.xpub.modal {
            match modal.validate() {
                Ok(xpub) => {
                    let key_id = modal.key_id;

                    // Update local state
                    if let Some(key) = self.app.keys.get_mut(&key_id) {
                        key.xpub = Some(xpub.clone());
                    }

                    // Send to backend
                    if let Some(wallet_id) = self.app.selected_wallet {
                        self.backend.edit_xpub(wallet_id, Some(xpub), key_id);
                    }

                    // Close modal on success (will be closed when backend notification arrives)
                    self.views.xpub.close_modal();
                }
                Err(error) => {
                    modal.set_error(error);
                }
            }
        }
        Task::none()
    }

    /// Clear xpub (set to null)
    fn on_xpub_clear(&mut self) -> Task<Msg> {
        if let Some(modal) = &self.views.xpub.modal {
            let key_id = modal.key_id;

            // Update local state
            if let Some(key) = self.app.keys.get_mut(&key_id) {
                key.xpub = None;
            }

            // Send to backend (None means clear/delete the xpub)
            if let Some(wallet_id) = self.app.selected_wallet {
                self.backend.edit_xpub(wallet_id, None, key_id);
            }

            // Close modal (will be closed when backend notification arrives)
            self.views.xpub.close_modal();
        }
        Task::none()
    }

    /// Close xpub modal
    fn on_xpub_cancel_modal(&mut self) {
        self.views.xpub.close_modal();
    }

    fn on_xpub_toggle_options(&mut self) {
        if let Some(modal) = self.views.xpub.modal_mut() {
            modal.options_collapsed = !modal.options_collapsed;
        }
    }
}
