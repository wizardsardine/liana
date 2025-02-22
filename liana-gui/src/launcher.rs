use std::path::PathBuf;

use iced::{
    alignment::Horizontal,
    widget::{pick_list, scrollable, Button, Space},
    Alignment, Length, Subscription, Task,
};

use liana::miniscript::bitcoin::Network;
use liana_ui::{
    component::{button, card, modal::Modal, network_banner, notification, text::*},
    icon, image, theme,
    widget::*,
};
use lianad::config::ConfigError;

use crate::{app, installer::UserFlow};

const NETWORKS: [Network; 4] = [
    Network::Bitcoin,
    Network::Testnet,
    Network::Signet,
    Network::Regtest,
];

// ===================
// ~~~~~~ STATE ~~~~~~
// ===================
pub struct Launcher {
    state: WalletState,
    network: Network,
    datadir_path: PathBuf,
    error: Option<String>,
    delete_wallet_modal: Option<DeleteWalletModal>,
}

// ===================
// ~~~~ SUB STATE ~~~~
// ===================
#[derive(Debug, Clone)]
pub enum WalletState {
    Unchecked,
    Wallet {
        name: Option<String>,
        email: Option<String>,
        checksum: Option<String>,
    },
    NoWallet,
}

// ===================
// ~~~ STATE CTOR ~~~~
// ===================
impl Launcher {
    pub fn new(datadir_path: PathBuf, network: Option<Network>) -> (Self, Task<Message>) {
        let network = network.unwrap_or(
            NETWORKS
                .iter()
                .find(|net| datadir_path.join(net.to_string()).exists())
                .cloned()
                .unwrap_or(Network::Bitcoin),
        );
        (
            Self {
                state: WalletState::Unchecked,
                network,
                datadir_path: datadir_path.clone(),
                error: None,
                delete_wallet_modal: None,
            },
            Task::perform(
                check_network_datadir(datadir_path.clone(), network),
                Message::Checked,
            ),
        )
    }
}

// ===================
// ~~~~~ UPDATE ~~~~~~
// ===================
impl Launcher {
    pub fn stop(&mut self) {}

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    fn do_import_wallet(&mut self) -> Task<Message> {
        let datadir_path = self.datadir_path.clone();
        let network = self.network;
        Task::perform(async move { (datadir_path, network) }, |(d, n)| {
            Message::Install(d, n, UserFlow::AddWallet)
        })
    }

    fn do_create_wallet(&mut self) -> Task<Message> {
        let datadir_path = self.datadir_path.clone();
        let network = self.network;
        Task::perform(async move { (datadir_path, network) }, |(d, n)| {
            Message::Install(d, n, UserFlow::CreateWallet)
        })
    }

    fn do_share_xpubs(&mut self) -> Task<Message> {
        let datadir_path = self.datadir_path.clone();
        let network = self.network;
        Task::perform(async move { (datadir_path, network) }, |(d, n)| {
            Message::Install(d, n, UserFlow::ShareXpubs)
        })
    }

    fn do_delete_wallet(&mut self) -> Task<Message> {
        let wallet_datadir = self.datadir_path.join(self.network.to_string());
        let config_path = wallet_datadir.join(app::config::DEFAULT_FILE_NAME);
        let internal_bitcoind = if let Ok(cfg) = app::Config::from_file(&config_path) {
            Some(cfg.start_internal_bitcoind)
        } else {
            None
        };
        self.delete_wallet_modal = Some(DeleteWalletModal::new(
            self.network,
            wallet_datadir,
            internal_bitcoind,
        ));
        Task::none()
    }

    fn do_select_network(&mut self, network: Network) -> Task<Message> {
        self.network = network;
        Task::perform(
            check_network_datadir(self.datadir_path.clone(), self.network),
            Message::Checked,
        )
    }

    fn do_run(&mut self) -> Task<Message> {
        if matches!(self.state, WalletState::Wallet { .. }) {
            let datadir_path = self.datadir_path.clone();
            let mut path = self.datadir_path.clone();
            path.push(self.network.to_string());
            path.push(app::config::DEFAULT_FILE_NAME);
            let cfg = app::Config::from_file(&path).expect("Already checked");
            let network = self.network;
            Task::perform(async move { (datadir_path.clone(), cfg, network) }, |m| {
                Message::Run(m.0, m.1, m.2)
            })
        } else {
            Task::none()
        }
    }

    fn view_update(&mut self, view_message: ViewMessage) -> Option<Task<Message>> {
        match view_message {
            ViewMessage::ImportWallet => Some(self.do_import_wallet()),
            ViewMessage::CreateWallet => Some(self.do_create_wallet()),
            ViewMessage::ShareXpubs => Some(self.do_share_xpubs()),
            ViewMessage::DeleteWallet(DeleteWalletMessage::ShowModal) => {
                Some(self.do_delete_wallet())
            }
            ViewMessage::SelectNetwork(network) => Some(self.do_select_network(network)),
            ViewMessage::DeleteWallet(DeleteWalletMessage::Deleted) => {
                self.state = WalletState::NoWallet;
                Some(Task::none())
            }
            ViewMessage::DeleteWallet(DeleteWalletMessage::CloseModal) => {
                self.delete_wallet_modal = None;
                Some(Task::none())
            }
            ViewMessage::Run => Some(self.do_run()),
            ViewMessage::Check | ViewMessage::StartInstall(_) | ViewMessage::DeleteWallet(_) => {
                None
            }
        }
    }

    fn default_update(&mut self, message: &Message) -> Task<Message> {
        if let Some(modal) = &mut self.delete_wallet_modal {
            return modal.update(message.clone());
        }
        Task::none()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message.clone() {
            Message::View(view_message) => self
                .view_update(view_message)
                .unwrap_or(self.default_update(&message)),
            Message::Checked(res) => match res {
                Err(e) => {
                    self.error = Some(e);
                    Task::none()
                }
                Ok(state) => {
                    self.state = state;
                    Task::none()
                }
            },
            Message::Install(..) | Message::Run(..) => self.default_update(&message),
        }
    }

    fn view_wallet(
        &self,
        email: &Option<String>,
        checksum: &Option<String>,
    ) -> Column<ViewMessage> {
        Column::new().push(
            Row::new()
                .align_y(Alignment::Center)
                .spacing(20)
                .push(
                    Container::new(
                        Button::new(
                            Column::new()
                                .push(p1_bold(format!(
                                    "My Liana {} wallet",
                                    match self.network {
                                        Network::Bitcoin => "Bitcoin",
                                        Network::Signet => "Signet",
                                        Network::Testnet => "Testnet",
                                        Network::Regtest => "Regtest",
                                        _ => "",
                                    }
                                )))
                                .push_maybe(checksum.as_ref().map(|checksum| {
                                    p1_regular(format!("Liana-{}", checksum))
                                        .style(theme::text::secondary)
                                }))
                                .push_maybe(email.as_ref().map(|email| {
                                    Row::new()
                                        .push(Space::with_width(Length::Fill))
                                        .push(p1_regular(email).style(theme::text::secondary))
                                })),
                        )
                        .on_press(ViewMessage::Run)
                        .padding(15)
                        .style(theme::button::container_border)
                        .width(Length::Fill),
                    )
                    .style(theme::card::simple),
                )
                .push(
                    Button::new(icon::trash_icon())
                        .style(theme::button::secondary)
                        .padding(10)
                        .on_press(ViewMessage::DeleteWallet(DeleteWalletMessage::ShowModal)),
                ),
        )
    }

    fn view_no_wallet(&self) -> Column<ViewMessage> {
        Column::new()
            .push(
                Row::new()
                    .align_y(Alignment::End)
                    .spacing(20)
                    .push(
                        Container::new(
                            Column::new()
                                .spacing(20)
                                .align_x(Alignment::Center)
                                .push(image::create_new_wallet_icon().width(Length::Fixed(100.0)))
                                .push(
                                    p1_regular("Create a new Liana wallet")
                                        .style(theme::text::secondary),
                                )
                                .push(
                                    button::secondary(None, "Select")
                                        .width(Length::Fixed(200.0))
                                        .on_press(ViewMessage::CreateWallet),
                                )
                                .align_x(Alignment::Center),
                        )
                        .padding(20),
                    )
                    .push(
                        Container::new(
                            Column::new()
                                .spacing(20)
                                .align_x(Alignment::Center)
                                .push(image::restore_wallet_icon().width(Length::Fixed(100.0)))
                                .push(
                                    p1_regular("Add an existing Liana wallet")
                                        .style(theme::text::secondary),
                                )
                                .push(
                                    button::secondary(None, "Select")
                                        .width(Length::Fixed(200.0))
                                        .on_press(ViewMessage::ImportWallet),
                                )
                                .align_x(Alignment::Center),
                        )
                        .padding(20),
                    ),
            )
            .align_x(Alignment::Center)
    }

    fn view_navigation_bar(&self) -> Row<ViewMessage> {
        Row::new()
            .spacing(20)
            .push(
                Container::new(image::liana_brand_grey().width(Length::Fixed(200.0)))
                    .width(Length::Fill),
            )
            .push(button::secondary(None, "Share Xpubs").on_press(ViewMessage::ShareXpubs))
            .push(
                pick_list(
                    &NETWORKS[..],
                    Some(self.network),
                    ViewMessage::SelectNetwork,
                )
                .style(theme::pick_list::primary)
                .padding(10),
            )
            .align_y(Alignment::Center)
            .padding(100)
    }

    fn view_body(&self) -> Container<ViewMessage> {
        Container::new(
            Column::new()
                .align_x(Alignment::Center)
                .spacing(30)
                .push(if matches!(self.state, WalletState::Wallet { .. }) {
                    text("Welcome back").size(50).bold()
                } else {
                    text("Welcome").size(50).bold()
                })
                .push_maybe(self.error.as_ref().map(|e| card::simple(text(e))))
                .push(match &self.state {
                    WalletState::Unchecked => Column::new(),
                    WalletState::Wallet {
                        email, checksum, ..
                    } => self.view_wallet(email, checksum),
                    WalletState::NoWallet => self.view_no_wallet(),
                })
                .max_width(500),
        )
    }

    fn with_children<'a>(&'a self, content: Element<'a, Message>) -> Element<'a, Message> {
        if self.network != Network::Bitcoin {
            Column::with_children(vec![network_banner(self.network).into(), content]).into()
        } else {
            content
        }
    }

    fn with_modal<'a>(&'a self, content: Element<'a, Message>) -> Element<'a, Message> {
        if let Some(modal) = &self.delete_wallet_modal {
            Modal::new(Container::new(content).height(Length::Fill), modal.view())
                .on_blur(Some(Message::View(ViewMessage::DeleteWallet(
                    DeleteWalletMessage::CloseModal,
                ))))
                .into()
        } else {
            content
        }
    }

    pub fn view(&self) -> Element<Message> {
        let content = Into::<Element<ViewMessage>>::into(scrollable(
            Column::new()
                .push(self.view_navigation_bar())
                .push(self.view_body().center_x(Length::Fill))
                .push(Space::with_height(Length::Fixed(100.0))),
        ))
        .map(Message::View);
        self.with_modal(self.with_children(content))
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    View(ViewMessage),
    Install(PathBuf, Network, UserFlow),
    Checked(Result<WalletState, String>),
    Run(PathBuf, app::config::Config, Network),
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
    ImportWallet,
    CreateWallet,
    ShareXpubs,
    SelectNetwork(Network),
    StartInstall(Network),
    Check,
    Run,
    DeleteWallet(DeleteWalletMessage),
}

#[derive(Debug, Clone)]
pub enum DeleteWalletMessage {
    ShowModal,
    CloseModal,
    Confirm,
    Deleted,
}

struct DeleteWalletModal {
    network: Network,
    wallet_datadir: PathBuf,
    warning: Option<std::io::Error>,
    deleted: bool,
    // `None` means we were not able to determine whether wallet uses internal bitcoind.
    internal_bitcoind: Option<bool>,
}

impl DeleteWalletModal {
    fn new(network: Network, wallet_datadir: PathBuf, internal_bitcoind: Option<bool>) -> Self {
        Self {
            network,
            wallet_datadir,
            warning: None,
            deleted: false,
            internal_bitcoind,
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        if let Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::Confirm)) = message {
            self.warning = None;
            if let Err(e) = std::fs::remove_dir_all(&self.wallet_datadir) {
                self.warning = Some(e);
            } else {
                self.deleted = true;
                return Task::perform(async {}, |_| {
                    Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::Deleted))
                });
            };
        }
        Task::none()
    }
    fn view(&self) -> Element<Message> {
        let mut confirm_button = button::secondary(None, "Delete wallet")
            .width(Length::Fixed(200.0))
            .style(theme::button::destructive);
        if self.warning.is_none() {
            confirm_button =
                confirm_button.on_press(ViewMessage::DeleteWallet(DeleteWalletMessage::Confirm));
        }
        // Use separate `Row`s for help text in order to have better spacing.
        let help_text_1 = format!(
            "Are you sure you want to delete the configuration and all associated data for the network {}?",
            &self.network
        );
        let help_text_2 = match self.internal_bitcoind {
            Some(true) => Some("(The Liana-managed Bitcoin node for this network will not be affected by this action.)"),
            Some(false) => None,
            None => Some("(If you are using a Liana-managed Bitcoin node, it will not be affected by this action.)"),
        };
        let help_text_3 = "WARNING: This cannot be undone.";

        Into::<Element<ViewMessage>>::into(
            card::simple(
                Column::new()
                    .spacing(10)
                    .push(Container::new(
                        h4_bold(format!("Delete configuration for {}", &self.network))
                            .style(theme::text::destructive)
                            .width(Length::Fill),
                    ))
                    .push(Row::new().push(text(help_text_1)))
                    .push_maybe(
                        help_text_2
                            .map(|t| Row::new().push(p1_regular(t).style(theme::text::secondary))),
                    )
                    .push(Row::new())
                    .push(Row::new().push(text(help_text_3)))
                    .push_maybe(self.warning.as_ref().map(|w| {
                        notification::warning(w.to_string(), w.to_string()).width(Length::Fill)
                    }))
                    .push(
                        Container::new(if !self.deleted {
                            Row::new().push(confirm_button)
                        } else {
                            Row::new()
                                .spacing(10)
                                .push(icon::circle_check_icon().style(theme::text::success))
                                .push(
                                    text("Wallet successfully deleted").style(theme::text::success),
                                )
                        })
                        .align_x(Horizontal::Center)
                        .width(Length::Fill),
                    ),
            )
            .width(Length::Fixed(700.0)),
        )
        .map(Message::View)
    }
}

async fn check_network_datadir(path: PathBuf, network: Network) -> Result<WalletState, String> {
    let mut config_path = path.clone();
    config_path.push(network.to_string());
    config_path.push(app::config::DEFAULT_FILE_NAME);

    let cfg = match app::Config::from_file(&config_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            if e == app::config::ConfigError::NotFound {
                return Ok(WalletState::NoWallet);
            } else {
                return Err(format!(
                    "Failed to read GUI configuration file in the directory: {}",
                    path.to_string_lossy()
                ));
            }
        }
    };

    if let Some(daemon_config_path) = cfg.daemon_config_path {
        lianad::config::Config::from_file(Some(daemon_config_path.clone())).map_err(|e| match e {
        ConfigError::FileNotFound
        | ConfigError::DatadirNotFound => {
            format!(
                "Failed to read daemon configuration file in the directory: {}",
                daemon_config_path.to_string_lossy()
            )
        }
        ConfigError::ReadingFile(e) => {
            if e.starts_with("Parsing configuration file: Error parsing descriptor") {
                "There is an issue with the configuration for this network. You most likely use a descriptor containing one or more public key(s) without origin. Liana v0.2 and later only support public keys with origins. Please migrate your funds using Liana v0.1.".to_string()
            } else {
                format!(
                    "Failed to read daemon configuration file in the directory: {}",
                    daemon_config_path.to_string_lossy()
                )
            }
        }
        ConfigError::UnexpectedDescriptor(_) => {
            "There is an issue with the configuration for this network. You most likely use a descriptor containing one or more public key(s) without origin. Liana v0.2 and later only support public keys with origins. Please migrate your funds using Liana v0.1.".to_string()
        }
        ConfigError::Unexpected(e) => {
            format!(
                "Unexpected {}",
                e,
            )
        }
    })?;
    }

    if let Ok(settings) = app::settings::Settings::from_file(path, network) {
        if let Some(wallet) = settings.wallets.first().cloned() {
            return Ok(WalletState::Wallet {
                name: Some(wallet.name),
                checksum: Some(wallet.descriptor_checksum),
                email: wallet.remote_backend_auth.map(|auth| auth.email),
            });
        }
    }
    Ok(WalletState::Wallet {
        name: None,
        checksum: None,
        email: None,
    })
}
