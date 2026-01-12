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
use std::sync::{Arc, Mutex};
use std::task::Waker;

/// Shared waker for the notification stream.
/// When a message is sent to notif_sender, the sender should wake this
/// so the stream gets polled again by the async executor.
pub type SharedWaker = Arc<Mutex<Option<Waker>>>;

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
    /// Shared waker for the notification stream - wake this after sending to notif_sender
    pub notif_waker: SharedWaker,
    pub hw: HwiService<Message>,
    /// Track if HW listener is running (to make stop_hw idempotent)
    hw_running: bool,
    /// Bitcoin network (mainnet, testnet, signet, regtest)
    pub network: Network,
    /// Dedicated sender for HwiService - messages are bridged to notif_sender with waking
    hw_sender: channel::Sender<Message>,
    /// Handle to the bridge thread (for cleanup)
    _hw_bridge_handle: Option<std::thread::JoinHandle<()>>,
}

impl State {
    pub fn new(network: Network) -> Self {
        let (notif_sender, notif_receiver) = channel::unbounded();
        let notif_waker: SharedWaker = Arc::new(Mutex::new(None));

        // Create a dedicated channel for HwiService
        let (hw_sender, hw_receiver) = channel::unbounded::<Message>();

        // Spawn bridge thread: forwards HW messages to notif_sender with waking
        let bridge_notif_sender = notif_sender.clone();
        let bridge_waker = notif_waker.clone();
        let hw_bridge_handle = std::thread::spawn(move || {
            tracing::debug!("HW bridge thread started");
            while let Ok(msg) = hw_receiver.recv() {
                if bridge_notif_sender.send(msg).is_ok() {
                    if let Ok(guard) = bridge_waker.lock() {
                        if let Some(waker) = guard.as_ref() {
                            waker.wake_by_ref();
                        }
                    }
                }
            }
            tracing::debug!("HW bridge thread stopped (channel disconnected)");
        });

        Self {
            app: AppState::new(),
            views: views::ViewsState::new(),
            backend: Client::new(notif_sender.clone(), notif_waker.clone()),
            current_view: View::Login,
            notif_sender,
            notif_receiver,
            notif_waker,
            // Note: Passing None for runtime - HwiService will create its own if needed.
            // We can't pass iced's tokio runtime as we only have a Handle, not a &Runtime.
            hw: HwiService::new(network, None),
            hw_running: false,
            network,
            hw_sender,
            _hw_bridge_handle: Some(hw_bridge_handle),
        }
    }

    /// Start hardware wallet listening (call when modal opens)
    pub fn start_hw(&mut self) {
        if !self.hw_running {
            self.hw.start(self.hw_sender.clone());
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
        // modals_view() already handles stacking (warning on top of other modals)
        if let Some(modal) = modals::modals_view(self) {
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
