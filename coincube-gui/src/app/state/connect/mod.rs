pub mod account;
pub mod cube;
pub mod cube_members;

pub(crate) const CONNECT_KEYRING_SERVICE: &str = if cfg!(debug_assertions) {
    "dev.coincube.Connect"
} else {
    "io.coincube.Connect"
};

pub(crate) const CONNECT_KEYRING_USER: &str = "global_session";

pub use account::{
    AddToCubeDialog, CheckoutPhase, CheckoutState, ConnectAccountPanel, ConnectFlowStep,
    ContactsState, ContactsStep, InviteCubeOption,
};
pub use cube::ConnectCubePanel;
pub use cube_members::ConnectCubeMembersState;

use std::sync::Arc;

use crate::{
    app::{
        breez_liquid::BreezClient,
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
        let mut account = ConnectAccountPanel::new();
        // W12 §2.7 tweak #1 / W14: propagate the active cube's network
        // into ContactsState so the invite-form + add-to-cube dialogs
        // can filter their candidate-cube lists.
        account.set_active_network(Some(cube_network.clone()));
        ConnectPanel {
            account,
            cube: ConnectCubePanel::new(breez_client, cube_uuid, cube_name, cube_network),
        }
    }

    /// Mirror the active Cube's server-side numeric id onto
    /// `ContactsState` so the W14 "Add to Current Cube" action can
    /// target the exact loaded cube (works even when the user has
    /// multiple cubes on the same network).
    fn sync_active_cube_server_id(&mut self) {
        self.account
            .set_active_cube_server_id(self.cube.server_cube_id);
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

    /// Check if avatar should be loaded and return task if so.
    pub fn check_and_load_avatar(&self) -> iced::Task<Message> {
        if let Some(task) = self.cube.load_avatar_if_needed() {
            return task;
        }
        iced::Task::none()
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
                self.sync_active_cube_server_id();
                // After first login, register the cube with the backend
                // (idempotent — returns existing if already registered).
                // The response includes lightning address if already claimed.
                let now_authenticated = self.account.is_authenticated();
                if !was_authenticated && now_authenticated {
                    // First login - register cube, avatar will load after CubeRegistered
                    let register_task = self.cube.register_cube();
                    return iced::Task::batch([task, register_task]);
                }
                // Already authenticated - ensure cube is registered, then load avatar
                if now_authenticated {
                    if self.cube.server_cube_id.is_none() {
                        // Need to register cube first, avatar will load after CubeRegistered
                        let register_task = self.cube.register_cube();
                        return iced::Task::batch([task, register_task]);
                    } else {
                        // Cube already registered, load avatar now
                        let avatar_task = self.check_and_load_avatar();
                        return iced::Task::batch([task, avatar_task]);
                    }
                }
                task
            }
            Message::View(view::Message::ConnectCube(msg)) => {
                let task = self.cube.update_message(msg);
                // `CubeRegistered(Ok)` populates `server_cube_id`; mirror
                // it into the account panel so the "Add to Current
                // Cube" button becomes enabled as soon as the cube is
                // known to the backend.
                self.sync_active_cube_server_id();
                task
            }
            _ => iced::Task::none(),
        }
    }
}
