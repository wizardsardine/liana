use crate::{
    backend::Backend,
    client::Client,
    state::app::AppState,
    views::{
        keys_view, login_view, modals, org_select_view, template_builder_view, wallet_select_view,
        xpub_view,
    },
};
use async_hwi::service::HwiService;
use crossbeam::channel;
use liana_ui::widget::{modal::Modal, Element};
pub use message::{Message, Msg};
use miniscript::bitcoin::Network;

pub mod app;
pub mod message;
pub mod update;
pub mod views;

/// Current view state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Login,
    OrgSelect,
    WalletSelect,
    WalletEdit,
    Xpub,
    Keys,
}

/// Main application state
pub struct State {
    pub app: AppState,
    pub views: views::ViewsState,
    pub backend: Client,
    pub current_view: View,
    pub notif_sender: channel::Sender<Message>,
    pub notif_receiver: channel::Receiver<Message>,
    pub hw: HwiService<Message>,
    /// Track if HW listener is running (to make stop_hw idempotent)
    hw_running: bool,
}

impl State {
    pub fn new(network: Network) -> Self {
        let (notif_sender, notif_receiver) = channel::unbounded();
        Self {
            app: AppState::new(),
            views: views::ViewsState::new(),
            backend: Client::new(notif_sender.clone()),
            current_view: View::Login,
            notif_sender,
            notif_receiver,
            hw: HwiService::new(network, None),
            hw_running: false,
        }
    }

    /// Start hardware wallet listening (call when modal opens)
    pub fn start_hw(&mut self) {
        if !self.hw_running {
            self.hw.start(self.notif_sender.clone());
            self.hw_running = true;
        }
    }

    /// Stop hardware wallet listening (call when modal closes)
    /// This is idempotent - safe to call multiple times
    pub fn stop_hw(&mut self) {
        if self.hw_running {
            self.hw.stop();
            self.hw_running = false;
        }
    }

    /// Initialize backend connection
    pub fn connect_backend(
        &mut self,
        url: String,
        version: u8,
        notif_sender: channel::Sender<Message>,
    ) {
        self.backend.connect_ws(url, version, notif_sender);
    }

    /// Close the backend connection
    pub fn close_backend(&mut self) {
        self.backend.close();
    }

    /// Determine which view should be displayed based on current state
    /// This handles routing logic like: if authenticated but still on login view, navigate to org select
    pub fn route(&self) -> View {
        // If authenticated but still on login view, navigate to org select
        if self.current_view == View::Login
            && self.views.login.current == views::LoginState::Authenticated
        {
            View::OrgSelect
        } else {
            self.current_view
        }
    }

    /// Render the current view with modals
    pub fn view(&self) -> Element<'_, Message> {
        let view_to_show = self.route();

        let content = match view_to_show {
            View::Login => login_view(self),
            View::OrgSelect => org_select_view(self),
            View::WalletSelect => wallet_select_view(self),
            View::WalletEdit => template_builder_view(self),
            View::Xpub => xpub_view(self),
            View::Keys => keys_view(self),
        };

        // Overlay modals if any are open
        // render_modals() already handles stacking (warning on top of other modals)
        if let Some(modal) = modals::render_modals(self) {
            // Determine which cancel message to use based on which modal is open
            // Warning modal has priority - if it's open, close it first
            let cancel_msg = if self.views.modals.warning.is_some() {
                Message::WarningCloseModal
            } else if self.views.keys.edit_key.is_some() {
                Message::KeyCancelModal
            } else if self.views.paths.edit_path.is_some() {
                Message::TemplateCancelPathModal
            } else if self.views.xpub.modal.is_some() {
                Message::XpubCancelModal
            } else {
                Message::WarningCloseModal
            };
            Modal::new(content, modal).on_blur(Some(cancel_msg)).into()
        } else {
            content
        }
    }

    /// Check if the template is valid and ready for validation
    /// Returns true if:
    /// - Primary path has at least one key and valid threshold
    /// - All secondary paths have non-zero timelocks
    /// - All secondary paths have unique timelocks (no duplicates)
    /// - All secondary paths have valid thresholds
    pub fn is_template_valid(&self) -> bool {
        // Check primary path
        if self.app.primary_path.key_ids.is_empty() {
            return false;
        }
        if !self.app.primary_path.is_valid() {
            return false;
        }

        // Check all secondary paths
        let mut seen_timelocks = std::collections::HashSet::new();
        for (path, timelock) in &self.app.secondary_paths {
            // Check timelock is set (non-zero)
            if timelock.is_zero() {
                return false;
            }
            // Check for duplicate timelocks
            if !seen_timelocks.insert(timelock.blocks) {
                return false;
            }
            // Check path has at least one key and valid threshold
            if !path.is_valid() {
                return false;
            }
        }

        true
    }
}

// NOTE: implementation of State::update() is in src/state/update.rs

// Default removed - State requires explicit initialization with datadir and network
