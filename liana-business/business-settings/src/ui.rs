//! Business-specific settings UI implementation.

use std::sync::Arc;

use iced::{Subscription, Task};
use liana_gui::{
    app::{
        cache::Cache, message::Message, settings::SettingsUI, state::State, view, wallet::Wallet,
        Config,
    },
    daemon::{Daemon, DaemonBackend},
    dir::LianaDirectory,
};
use liana_ui::widget::Element;

use crate::message::{Msg, Section};
use crate::views;

/// Business-specific settings UI.
pub struct BusinessSettingsUI {
    pub(crate) wallet: Arc<Wallet>,
    pub(crate) current_section: Option<Section>,
    pub(crate) fiat_enabled: bool,
    #[allow(dead_code)]
    processing: bool,
}

impl SettingsUI<Msg> for BusinessSettingsUI {
    fn new(
        _data_dir: LianaDirectory,
        wallet: Arc<Wallet>,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _daemon_backend: DaemonBackend,
        _internal_bitcoind: bool,
        _config: Arc<Config>,
    ) -> (Self, Task<Msg>) {
        let ui = Self {
            wallet,
            current_section: None,
            fiat_enabled: false,
            processing: false,
        };
        // TODO: Fetch fiat setting from backend on load
        (ui, Task::none())
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Msg,
    ) -> Task<Msg> {
        match message {
            Msg::SelectSection(section) => self.on_select_section(section),
            Msg::EnableFiat(enabled) => self.on_enable_fiat(enabled),
            Msg::SelectDevice(fingerprint) => self.on_select_device(fingerprint),
            Msg::RegisterWallet => self.on_register_wallet(),
        }
    }

    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, Msg> {
        let content = match self.current_section {
            None => views::list_view(),
            Some(Section::General) => views::general_view(self),
            Some(Section::Wallet) => views::wallet_view(self),
            Some(Section::About) => views::about_view(),
        };
        views::layout(content)
    }

    fn subscription(&self) -> Subscription<Msg> {
        // TODO: Add async-hwi service subscription when on Wallet section
        Subscription::none()
    }

    fn stop(&mut self) {
        self.current_section = None;
    }

    fn reload(&mut self, _daemon: Arc<dyn Daemon + Sync + Send>, wallet: Arc<Wallet>) -> Task<Msg> {
        self.wallet = wallet;
        Task::none()
    }
}

// Update handlers
impl BusinessSettingsUI {
    fn on_select_section(&mut self, section: Section) -> Task<Msg> {
        self.current_section = Some(section);
        Task::none()
    }

    fn on_enable_fiat(&mut self, enabled: bool) -> Task<Msg> {
        self.fiat_enabled = enabled;
        // TODO: Save to backend
        Task::none()
    }

    fn on_select_device(
        &mut self,
        _fingerprint: liana::miniscript::bitcoin::bip32::Fingerprint,
    ) -> Task<Msg> {
        // TODO: Implement when async-hwi service is available
        Task::none()
    }

    fn on_register_wallet(&mut self) -> Task<Msg> {
        // TODO: Implement when async-hwi service is available
        Task::none()
    }
}

/// State trait implementation for integration with liana-gui's App panel system.
impl State for BusinessSettingsUI {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        SettingsUI::view(self, cache).map(|msg| match msg {
            Msg::SelectSection(Section::General) => {
                view::Message::Settings(view::SettingsMessage::GeneralSection)
            }
            Msg::SelectSection(Section::Wallet) => {
                view::Message::Settings(view::SettingsMessage::EditWalletSettings)
            }
            Msg::SelectSection(Section::About) => {
                view::Message::Settings(view::SettingsMessage::AboutSection)
            }
            // Internal messages that don't need to propagate
            _ => view::Message::Reload,
        })
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::Settings(settings_msg)) = &message {
            let msg = match settings_msg {
                view::SettingsMessage::GeneralSection => Some(Msg::SelectSection(Section::General)),
                view::SettingsMessage::EditWalletSettings => {
                    Some(Msg::SelectSection(Section::Wallet))
                }
                view::SettingsMessage::AboutSection => Some(Msg::SelectSection(Section::About)),
                _ => None,
            };
            if let Some(m) = msg {
                let _ = SettingsUI::update(self, daemon, cache, m);
            }
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
        let _ = SettingsUI::reload(self, daemon, wallet);
        Task::none()
    }
}
