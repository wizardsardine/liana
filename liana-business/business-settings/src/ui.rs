//! Business-specific settings UI implementation.

use std::sync::Arc;

use iced::{Subscription, Task};
use liana::miniscript::bitcoin::Network;
use liana_gui::{
    app::{
        cache::Cache,
        menu::Menu,
        message::Message,
        settings::{fiat::PriceSetting, update_settings_file, SettingsError, SettingsUI},
        state::{settings::wallet::RegisterWalletModal, State},
        view,
        wallet::Wallet,
        Config,
    },
    daemon::{Daemon, DaemonBackend},
    dir::LianaDirectory,
};
use liana_ui::widget::{modal, Element};

use crate::message::{Msg, Section};
use crate::{views, BusinessSettings};

/// Business-specific settings UI.
pub struct BusinessSettingsUI {
    pub(crate) data_dir: LianaDirectory,
    pub(crate) wallet: Arc<Wallet>,
    pub(crate) current_section: Option<Section>,
    fiat_setting: PriceSetting,
    #[allow(dead_code)]
    processing: bool,
    register_modal: Option<RegisterWalletModal>,
}

fn wallet_fiat_setting_or_default(wallet: &Wallet) -> PriceSetting {
    wallet
        .fiat_price_setting
        .as_ref()
        .cloned()
        .unwrap_or_default()
}

async fn update_business_fiat_setting(
    data_dir: LianaDirectory,
    network: Network,
    wallet: Arc<Wallet>,
    new_setting: PriceSetting,
) -> Result<Arc<Wallet>, SettingsError> {
    let mut wallet = wallet.as_ref().clone();
    wallet = wallet.with_fiat_price_setting(Some(new_setting.clone()));
    let network_dir = data_dir.network_directory(network);
    let wallet_id = wallet.id();
    update_settings_file::<BusinessSettings, _>(&network_dir, |mut settings| {
        if let Some(ws) = settings
            .wallets
            .iter_mut()
            .find(|w| w.wallet_id() == wallet_id)
        {
            ws.fiat_price = Some(new_setting);
        }
        settings
    })
    .await?;
    Ok(Arc::new(wallet))
}

impl SettingsUI<Msg> for BusinessSettingsUI {
    fn new(
        data_dir: LianaDirectory,
        wallet: Arc<Wallet>,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _daemon_backend: DaemonBackend,
        _internal_bitcoind: bool,
        _config: Arc<Config>,
    ) -> (Self, Task<Msg>) {
        let fiat_setting = wallet_fiat_setting_or_default(&wallet);
        let ui = Self {
            data_dir,
            wallet,
            current_section: None,
            fiat_setting,
            processing: false,
            register_modal: None,
        };
        (ui, Task::none())
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Msg,
    ) -> Task<Msg> {
        match message {
            Msg::Home => {
                self.current_section = None;
                Task::none()
            }
            Msg::SelectSection(section) => self.on_select_section(section),
            Msg::RegisterWallet | Msg::FiatEnable(_) | Msg::FiatCurrencyEdited(_) => Task::none(), // Handled in State::update()
        }
    }

    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, Msg> {
        match self.current_section {
            None => views::list_view(),
            Some(Section::Wallet) => views::wallet_view(self),
            Some(Section::General) => {
                let bc = crate::BackendCurrency::try_from(self.fiat_setting.currency)
                    .unwrap_or_default();
                views::general_view(self.fiat_setting.is_enabled, bc)
            }
            Some(Section::About) => views::about_view(),
        }
    }

    fn subscription(&self) -> Subscription<Msg> {
        Subscription::none()
    }

    fn stop(&mut self) {
        self.current_section = None;
        self.register_modal = None;
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Task<Msg> {
        self.current_section = None;
        self.fiat_setting = wallet_fiat_setting_or_default(&wallet);
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

    fn save_fiat_setting(&self, cache: &Cache) -> Task<Message> {
        let wallet = self.wallet.clone();
        let setting = self.fiat_setting.clone();
        let network = cache.network;
        let datadir = self.data_dir.clone();
        Task::perform(
            async move { update_business_fiat_setting(datadir, network, wallet, setting).await },
            |res| Message::WalletUpdated(res.map_err(Into::into)),
        )
    }
}

/// State trait implementation for integration with liana-gui's App panel system.
impl State for BusinessSettingsUI {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = SettingsUI::view(self, cache).map(|msg| match msg {
            Msg::Home => view::Message::Menu(Menu::Settings),
            Msg::SelectSection(Section::Wallet) => {
                view::Message::Settings(view::SettingsMessage::EditWalletSettings)
            }
            Msg::SelectSection(Section::General) => {
                view::Message::Settings(view::SettingsMessage::GeneralSection)
            }
            Msg::SelectSection(Section::About) => {
                view::Message::Settings(view::SettingsMessage::AboutSection)
            }
            Msg::RegisterWallet => view::Message::Settings(view::SettingsMessage::RegisterWallet),
            Msg::FiatEnable(b) => {
                view::Message::Settings(view::SettingsMessage::Fiat(view::FiatMessage::Enable(b)))
            }
            Msg::FiatCurrencyEdited(c) => view::Message::Settings(view::SettingsMessage::Fiat(
                view::FiatMessage::CurrencyEdited(c.into()),
            )),
        });
        let dashboard = view::dashboard(&Menu::Settings, cache, None, content);

        if let Some(m) = &self.register_modal {
            modal::Modal::new(dashboard, m.view())
                .on_blur(Some(view::Message::Close))
                .into()
        } else {
            dashboard
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Settings(view::SettingsMessage::RegisterWallet)) => {
                self.register_modal = Some(RegisterWalletModal::new(
                    self.data_dir.clone(),
                    self.wallet.clone(),
                    cache.network,
                ));
                Task::none()
            }
            Message::View(view::Message::Close) => {
                self.register_modal = None;
                Task::none()
            }
            Message::WalletUpdated(ref res) => {
                if let Ok(wallet) = res {
                    self.fiat_setting = wallet_fiat_setting_or_default(wallet);
                    self.wallet = wallet.clone();
                }
                if let Some(modal) = &mut self.register_modal {
                    modal.update(daemon, cache, message)
                } else {
                    Task::none()
                }
            }
            Message::HardwareWallets(_)
            | Message::View(view::Message::SelectHardwareWallet(_))
            | Message::View(view::Message::Reload) => {
                if let Some(modal) = &mut self.register_modal {
                    modal.update(daemon, cache, message)
                } else {
                    Task::none()
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::Fiat(ref fiat_msg))) => {
                match fiat_msg {
                    view::FiatMessage::Enable(enabled) => {
                        self.fiat_setting.is_enabled = *enabled;
                        self.save_fiat_setting(cache)
                    }
                    view::FiatMessage::CurrencyEdited(currency) => {
                        self.fiat_setting.currency = *currency;
                        self.save_fiat_setting(cache)
                    }
                    _ => Task::none(),
                }
            }
            Message::View(view::Message::Settings(ref settings_msg)) => {
                let msg = match settings_msg {
                    view::SettingsMessage::EditWalletSettings => {
                        Some(Msg::SelectSection(Section::Wallet))
                    }
                    view::SettingsMessage::GeneralSection => {
                        Some(Msg::SelectSection(Section::General))
                    }
                    view::SettingsMessage::AboutSection => Some(Msg::SelectSection(Section::About)),
                    _ => None,
                };
                if let Some(m) = msg {
                    let _ = SettingsUI::update(self, daemon, cache, m);
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        if let Some(modal) = &self.register_modal {
            modal.subscription()
        } else {
            Subscription::none()
        }
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.register_modal = None;
        let _ = SettingsUI::reload(self, daemon, wallet);
        Task::none()
    }
}
