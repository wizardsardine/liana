pub mod account;
pub mod cube;

pub(crate) const CONNECT_KEYRING_SERVICE: &str = if cfg!(debug_assertions) {
    "dev.coincube.Connect"
} else {
    "io.coincube.Connect"
};

pub(crate) const CONNECT_KEYRING_USER: &str = "global_session";

pub use account::{CheckoutPhase, CheckoutState, ConnectAccountPanel, ConnectFlowStep};
pub use cube::ConnectCubePanel;

use std::sync::Arc;

use crate::{
    app::{
        breez::BreezClient,
        cache::Cache,
        menu::Menu,
        message::Message,
        state::State,
        view::{self, ConnectAccountMessage},
    },
    daemon::Daemon,
};

/// Sub-steps within the Avatar sub-menu (does not replace ConnectFlowStep).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvatarFlowStep {
    /// No avatar exists and the user hasn't started creation.
    Idle,
    /// Trait questionnaire is open.
    Questionnaire,
    /// Waiting for OpenAI response (~10–30s).
    Generating,
    /// Showing a freshly generated avatar.
    Reveal,
    /// Viewing / managing an existing avatar.
    Settings,
}

/// Wrapper that holds both the account-level and cube-level Connect panels.
/// Implements `State` so the existing routing in App works unchanged.
pub struct ConnectPanel {
    pub account: ConnectAccountPanel,
    pub cube: ConnectCubePanel,
}

impl ConnectPanel {
    pub fn new(
        breez_client: Arc<BreezClient>,
        cube_uuid: String,
        cube_name: String,
        cube_network: String,
    ) -> Self {
        ConnectPanel {
            account: ConnectAccountPanel::new(),
            cube: ConnectCubePanel::new(breez_client, cube_uuid, cube_name, cube_network),
        }
    }

    /// Sync the authenticated client from account panel to cube panel.
    fn sync_client(&mut self) {
        if self.account.is_authenticated() {
            if let Some(client) = self.account.authenticated_client() {
                self.cube.set_client(client);
            }
        } else {
            self.cube.clear_client();
        }
    }
}

impl State for ConnectPanel {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> coincube_ui::widget::Element<'a, view::Message> {
        // Get the active avatar image handle for the sidebar
        let avatar_handle = self.cube.avatar_data.as_ref().and_then(|d| {
            let url = d.active_avatar_url.as_deref().unwrap_or("");
            d.variants
                .iter()
                .find(|v| url.ends_with(&v.id.to_string()))
                .and_then(|v| self.cube.avatar_image_cache.get(&v.id))
                .map(|(_, handle)| handle)
        });
        let ln_addr = self
            .cube
            .lightning_address
            .as_ref()
            .and_then(|la| la.lightning_address.as_deref());

        view::dashboard_with_info(
            menu,
            cache,
            view::connect::connect_panel(self),
            &cache.cube_name,
            avatar_handle,
            ln_addr,
        )
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> iced::Task<Message> {
        if matches!(self.account.step, ConnectFlowStep::CheckingSession) {
            iced::Task::done(Message::View(view::Message::ConnectAccount(
                ConnectAccountMessage::Init,
            )))
        } else {
            iced::Task::none()
        }
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> iced::Task<Message> {
        match message {
            Message::View(view::Message::ConnectAccount(msg)) => {
                let was_authenticated = self.account.is_authenticated();
                let task = self.account.update_message(msg);
                self.sync_client();
                // After first login, register the cube with the backend
                // (idempotent — returns existing if already registered).
                // The response includes lightning address if already claimed.
                let now_authenticated = self.account.is_authenticated();
                if !was_authenticated && now_authenticated {
                    let register_task = self.cube.register_cube();
                    return iced::Task::batch([task, register_task]);
                }
                task
            }
            Message::View(view::Message::ConnectCube(msg)) => self.cube.update_message(msg),
            _ => iced::Task::none(),
        }
    }
}
