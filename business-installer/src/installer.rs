//! Business Installer implementation
//!
//! This module exposes `BusinessInstaller` which implements the `Installer` trait
//! from liana-gui, allowing it to be used as an installer component in liana-gui
//! in the future.

use std::pin::Pin;
use std::thread;
use std::time::Duration;

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

use crate::backend::{Notification, BACKEND_RECV};
use crate::client::{BACKEND_URL, PROTOCOL_VERSION};
use crate::state::State;

// Re-export Message type for external use
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
        let mut state = State::new();
        let recv = state.connect_backend(BACKEND_URL.to_string(), PROTOCOL_VERSION);

        // Store the receiver in the global static for the subscription to use
        *BACKEND_RECV.lock().expect("poisoned") = recv;

        let installer = Self {
            datadir,
            network,
            state,
        };

        (installer, Task::none())
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
        Subscription::run(BackendSubscription::new)
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
        println!("BackendSubscription dropped");
    }
}

impl Drop for BusinessInstaller {
    fn drop(&mut self) {
        self.state.close_backend();
    }
}
