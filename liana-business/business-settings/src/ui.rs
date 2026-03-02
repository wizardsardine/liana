//! Business-specific settings UI implementation.

use std::sync::Arc;

use iced::{clipboard, Subscription, Task};
use liana_gui::{
    app::{
        cache::Cache,
        menu::Menu,
        message::Message,
        settings::SettingsUI,
        state::{export::ExportModal, settings::wallet::RegisterWalletModal, State},
        view,
        wallet::Wallet,
        Config,
    },
    daemon::{Daemon, DaemonBackend},
    dir::LianaDirectory,
    export::{ImportExportMessage, ImportExportType},
};
use liana_ui::widget::{modal, Element};

use crate::message::{Msg, Section};
use crate::views;

/// Business-specific settings UI.
pub struct BusinessSettingsUI {
    pub(crate) data_dir: LianaDirectory,
    pub(crate) wallet: Arc<Wallet>,
    pub(crate) current_section: Option<Section>,
    #[allow(dead_code)]
    processing: bool,
    register_modal: Option<RegisterWalletModal>,
    export_modal: Option<ExportModal>,
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
        let ui = Self {
            data_dir,
            wallet,
            current_section: None,
            processing: false,
            register_modal: None,
            export_modal: None,
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
            Msg::Home => {
                self.current_section = None;
                Task::none()
            }
            Msg::SelectSection(section) => self.on_select_section(section),
            Msg::RegisterWallet => Task::none(), // Handled in State::update()
            Msg::CopyDescriptor => {
                let descriptor = self.wallet.main_descriptor.to_string();
                clipboard::write(descriptor)
            }
            Msg::ExportEncryptedDescriptor => {
                self.export_modal = Some(ExportModal::new(
                    None,
                    ImportExportType::ExportEncryptedDescriptor(Box::new(
                        self.wallet.main_descriptor.clone(),
                    )),
                ));
                self.export_modal
                    .as_ref()
                    .map(|m| m.launch(true))
                    .unwrap_or(Task::none())
            }
            Msg::Export(msg) => {
                if matches!(msg, ImportExportMessage::Close) {
                    self.export_modal = None;
                    return Task::none();
                }
                if let Some(modal) = &mut self.export_modal {
                    modal.update(msg)
                } else {
                    Task::none()
                }
            }
        }
    }

    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, Msg> {
        let content = match self.current_section {
            None => views::list_view(),
            Some(Section::Wallet) => views::wallet_view(self),
            Some(Section::About) => views::about_view(),
        };

        if let Some(export_modal) = &self.export_modal {
            export_modal.view(content)
        } else {
            content
        }
    }

    fn subscription(&self) -> Subscription<Msg> {
        if let Some(modal) = &self.export_modal {
            modal
                .subscription()
                .map(|s| s.map(|m| Msg::Export(ImportExportMessage::Progress(m))))
                .unwrap_or(Subscription::none())
        } else {
            Subscription::none()
        }
    }

    fn stop(&mut self) {
        self.current_section = None;
        self.register_modal = None;
        self.export_modal = None;
    }

    fn reload(&mut self, _daemon: Arc<dyn Daemon + Sync + Send>, wallet: Arc<Wallet>) -> Task<Msg> {
        self.current_section = None;
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
}

/// State trait implementation for integration with liana-gui's App panel system.
impl State for BusinessSettingsUI {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = SettingsUI::view(self, cache).map(|msg| match msg {
            Msg::Home => view::Message::Menu(Menu::Settings),
            Msg::SelectSection(Section::Wallet) => {
                view::Message::Settings(view::SettingsMessage::EditWalletSettings)
            }
            Msg::SelectSection(Section::About) => {
                view::Message::Settings(view::SettingsMessage::AboutSection)
            }
            Msg::RegisterWallet => view::Message::Settings(view::SettingsMessage::RegisterWallet),
            Msg::CopyDescriptor => {
                view::Message::Clipboard(self.wallet.main_descriptor.to_string())
            }
            Msg::ExportEncryptedDescriptor => {
                view::Message::Settings(view::SettingsMessage::ExportEncryptedDescriptor)
            }
            Msg::Export(msg) => view::Message::ImportExport(msg),
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
            Message::View(view::Message::Settings(
                view::SettingsMessage::ExportEncryptedDescriptor,
            )) => {
                SettingsUI::update(self, daemon, cache, Msg::ExportEncryptedDescriptor).map(|m| {
                    if let Msg::Export(export_msg) = m {
                        Message::View(view::Message::ImportExport(export_msg))
                    } else {
                        Message::View(view::Message::Close)
                    }
                })
            }
            Message::View(view::Message::ImportExport(msg)) => {
                SettingsUI::update(self, daemon, cache, Msg::Export(msg)).map(|m| {
                    if let Msg::Export(export_msg) = m {
                        Message::View(view::Message::ImportExport(export_msg))
                    } else {
                        Message::View(view::Message::Close)
                    }
                })
            }
            Message::View(view::Message::Clipboard(text)) => clipboard::write(text),
            Message::View(view::Message::Settings(ref settings_msg)) => {
                let msg = match settings_msg {
                    view::SettingsMessage::EditWalletSettings => {
                        Some(Msg::SelectSection(Section::Wallet))
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
        let register_sub = if let Some(modal) = &self.register_modal {
            modal.subscription()
        } else {
            Subscription::none()
        };
        let export_sub = SettingsUI::subscription(self).map(|msg| match msg {
            Msg::Export(m) => Message::View(view::Message::ImportExport(m)),
            _ => Message::View(view::Message::Close),
        });
        Subscription::batch([register_sub, export_sub])
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
