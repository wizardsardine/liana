use crate::state::{SharedWaker, State};
use crossbeam::channel::{self};
use iced::Task;
use liana::miniscript::bitcoin::{self};
use liana_gui::{
    dir::LianaDirectory,
    installer::{Installer, NextState, UserFlow},
    services::connect::client::backend::BackendClient,
};
use liana_ui::widget::Element;
use std::pin::Pin;
use std::task::{Context, Poll};
use tracing::debug;

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
    fn new(datadir: LianaDirectory, network: bitcoin::Network) -> (Self, Task<Message>) {
        use crate::state::views::login::{Login, LoginState};

        let mut state = State::new(network);

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
        let (installer, task) = BusinessInstaller::new(destination_path, network);
        let listener = Task::stream(NotifListener {
            receiver: installer.state.notif_receiver.clone(),
            waker: installer.state.notif_waker.clone(),
        });
        (Box::new(installer), Task::batch([listener, task]))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        self.state.update(message)
    }

    fn view(&self) -> Element<Message> {
        self.state.view()
    }

    fn stop(&mut self) {
        self.state.stop_hw();
        self.state.close_backend();
    }

    fn datadir(&self) -> &LianaDirectory {
        &self.datadir
    }

    fn network(&self) -> bitcoin::Network {
        self.network
    }

    fn skip_launcher() -> bool {
        true
    }

    fn exit_maybe(&mut self, _msg: &Message) -> Option<NextState> {
        // Check if we should exit to App (user selected a Final wallet)
        if self.state.app.exit {
            // Reset the flag
            self.state.app.exit = false;

            // Get wallet ID from selected wallet
            let wallet_id = self
                .state
                .app
                .selected_wallet
                .map(|id| id.to_string())
                .unwrap_or_default();

            // Get user email from login state
            let email = self.state.views.login.email.form.value.clone();

            // Use RunLianaBusiness for direct transition to App
            // User is already authenticated, tokens are cached in connect.json
            return Some(NextState::RunLianaBusiness {
                datadir: self.datadir.clone(),
                network: self.network,
                wallet_id,
                email,
            });
        }
        None
    }
}

struct NotifListener {
    receiver: channel::Receiver<Message>,
    waker: SharedWaker,
}

impl iced::futures::Stream for NotifListener {
    type Item = Message;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Use non-blocking try_recv to avoid blocking the async executor
        match self.receiver.try_recv() {
            Ok(msg) => Poll::Ready(Some(msg)),
            Err(channel::TryRecvError::Empty) => {
                // Store the waker so senders can wake us when new messages arrive
                if let Ok(mut guard) = self.waker.lock() {
                    *guard = Some(cx.waker().clone());
                }
                Poll::Pending
            }
            Err(channel::TryRecvError::Disconnected) => Poll::Ready(None),
        }
    }
}

impl Drop for NotifListener {
    fn drop(&mut self) {
        debug!("NotifListener dropped");
    }
}

impl Drop for BusinessInstaller {
    fn drop(&mut self) {
        self.state.stop_hw();
        self.state.close_backend();
    }
}
