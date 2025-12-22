use crate::{
    backend::{Notification, BACKEND_RECV},
    state::State,
};
use crossbeam::channel;
use iced::{Subscription, Task};
use liana::miniscript::bitcoin::{self};
use liana_gui::{
    app::settings::{AuthConfig, WalletId},
    dir::LianaDirectory,
    installer::{Installer, NextState, UserFlow},
    services::connect::client::backend::BackendClient,
};
use liana_ui::widget::Element;
use std::{pin::Pin, thread, time::Duration};

pub use crate::state::Msg as Message;

/// BusinessInstaller implements the Installer trait from liana-gui.
///
/// This struct encapsulates all the business logic for creating wallet policies
/// through the Liana Business service.
pub struct BusinessInstaller {
    datadir: LianaDirectory,
    network: bitcoin::Network,
    state: State,
}

impl BusinessInstaller {
    /// Create a new BusinessInstaller with internal initialization
    fn new_internal(datadir: LianaDirectory, network: bitcoin::Network) -> (Self, Task<Message>) {
        use crate::state::views::login::{Login, LoginState};

        let mut state = State::new(datadir.clone(), network);

        // Set network directory for token caching (same location as liana-gui)
        let network_dir = datadir.network_directory(network);
        state.backend.set_network_dir(network_dir);
        state.backend.set_network(network);

        // Validate cached tokens before showing UI
        let (valid_accounts, to_remove) = state.backend.validate_all_cached_tokens();

        // Clear invalid tokens from cache
        if !to_remove.is_empty() {
            state.backend.clear_invalid_tokens(&to_remove);
        }

        // Set initial login state based on cached accounts
        if !valid_accounts.is_empty() {
            state.views.login = Login::with_cached_accounts(valid_accounts);
        }

        // Initialize notification channel for auth flow (needed before any auth_request)
        let recv = state.backend.init_notif_channel();
        *BACKEND_RECV.lock().expect("poisoned") = Some(recv);

        // Determine initial focus based on login state
        let focus_task = if state.views.login.current == LoginState::AccountSelect {
            // No input to focus on account select view
            Task::none()
        } else {
            // Focus the email input on initial load
            liana_ui::widget::text_input::focus("login_email")
        };

        let installer = Self {
            datadir,
            network,
            state,
        };

        (installer, focus_task)
    }
}

impl<'a> Installer<'a, Message> for BusinessInstaller {
    fn new(
        destination_path: LianaDirectory,
        network: bitcoin::Network,
        _remote_backend: Option<BackendClient>,
        _user_flow: UserFlow,
    ) -> (Box<Self>, Task<Message>) {
        let (installer, task) = BusinessInstaller::new_internal(destination_path, network);
        (Box::new(installer), task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        self.state.update(message)
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![Subscription::run(BackendSubscription::new)];

        // Only refresh hardware wallets when xpub modal is open
        if self.state.views.xpub.modal.is_some() {
            subs.push(self.state.hw.refresh().map(hw_message_to_app_message));
        }

        Subscription::batch(subs)
    }

    fn view(&self) -> Element<Message> {
        self.state.view()
    }

    fn stop(&mut self) {
        self.state.close_backend();
    }

    fn datadir(&self) -> &LianaDirectory {
        &self.datadir
    }

    fn network(&self) -> bitcoin::Network {
        self.network
    }

    fn exit_maybe(&mut self, _msg: &Message) -> Option<NextState> {
        // Check if we should exit to Liana Lite (user selected a Final wallet)
        if self.state.app.exit_to_liana_lite {
            // Reset the flag
            self.state.app.exit_to_liana_lite = false;

            // Get wallet ID from selected wallet
            let wallet_id_str = self
                .state
                .app
                .selected_wallet
                .map(|id| id.to_string())
                .unwrap_or_default();

            // Get user email from login state
            let email = self.state.views.login.email.form.value.clone();

            // Create WalletId (using wallet UUID as descriptor checksum placeholder)
            let directory_wallet_id = WalletId::new(wallet_id_str.clone(), None);

            // Create AuthConfig with user's email and wallet ID
            let auth_cfg = AuthConfig {
                email,
                wallet_id: wallet_id_str,
                refresh_token: None,
            };

            return Some(NextState::LoginLianaLite {
                datadir: self.datadir.clone(),
                network: self.network,
                directory_wallet_id,
                auth_cfg,
            });
        }
        None
    }
}

// Subscription for backend stream
struct BackendSubscription {
    receiver: Option<channel::Receiver<Notification>>,
}

impl BackendSubscription {
    fn new() -> Self {
        if let Ok(mut channel_guard) = BACKEND_RECV.lock() {
            if let Some(receiver) = channel_guard.take() {
                return Self {
                    receiver: Some(receiver),
                };
            }
        }
        Self { receiver: None }
    }
}

impl iced::futures::Stream for BackendSubscription {
    type Item = Message;

    fn poll_next(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;

        let this = Pin::get_mut(self);
        loop {
            // NOTE: If there is a new connection we replace this one
            if let Some(recv) = BACKEND_RECV.lock().expect("poisoned").take() {
                this.receiver = Some(recv);
            }

            if let Some(receiver) = this.receiver.as_mut() {
                // NOTE: if we send a Poll::Ready(None), iced will drop subscription so
                // we call (blocking) .recv().
                if let Ok(m) = receiver.recv() {
                    return Poll::Ready(Some(Message::BackendNotif(m)));
                } else {
                    this.receiver = None;
                };
            }
            // NOTE: if there is no receiver we just block until there is one
            // with a delay to avoid spinloop
            thread::sleep(Duration::from_millis(500));
        }
    }
}

impl Drop for BackendSubscription {
    fn drop(&mut self) {
        // Backend subscription dropped
    }
}

impl Drop for BusinessInstaller {
    fn drop(&mut self) {
        self.state.close_backend();
    }
}

/// Map hardware wallet messages to application messages
fn hw_message_to_app_message(msg: liana_gui::hw::HardwareWalletMessage) -> Message {
    Message::HardwareWallets(msg)
}
