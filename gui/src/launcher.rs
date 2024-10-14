use std::path::PathBuf;

use iced::{
    alignment::Horizontal,
    widget::{pick_list, scrollable, Button, Space},
    Alignment, Command, Length, Subscription,
};

use liana::{config::ConfigError, miniscript::bitcoin::Network};
use liana_ui::{
    color,
    component::{button, card, modal::Modal, network_banner, notification, text::*},
    icon, image, theme,
    widget::*,
};

use crate::{app, installer::UserFlow};

const NETWORKS: [Network; 4] = [
    Network::Bitcoin,
    Network::Testnet,
    Network::Signet,
    Network::Regtest,
];

#[derive(Debug, Clone)]
pub enum State {
    Unchecked,
    Wallet {
        name: Option<String>,
        email: Option<String>,
        checksum: Option<String>,
    },
    NoWallet,
}

pub struct Launcher {
    state: State,
    network: Network,
    datadir_path: PathBuf,
    error: Option<String>,
    delete_wallet_modal: Option<DeleteWalletModal>,
}

impl Launcher {
    pub fn new(datadir_path: PathBuf, network: Option<Network>) -> (Self, Command<Message>) {
        let network = network.unwrap_or(
            NETWORKS
                .iter()
                .find(|net| datadir_path.join(net.to_string()).exists())
                .cloned()
                .unwrap_or(Network::Bitcoin),
        );
        (
            Self {
                state: State::Unchecked,
                network,
                datadir_path: datadir_path.clone(),
                error: None,
                delete_wallet_modal: None,
            },
            Command::perform(
                check_network_datadir(datadir_path.clone(), network),
                Message::Checked,
            ),
        )
    }

    pub fn stop(&mut self) {}

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::View(ViewMessage::ImportWallet) => {
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                Command::perform(async move { (datadir_path, network) }, |(d, n)| {
                    Message::Install(d, n, UserFlow::AddWallet)
                })
            }
            Message::View(ViewMessage::CreateWallet) => {
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                Command::perform(async move { (datadir_path, network) }, |(d, n)| {
                    Message::Install(d, n, UserFlow::CreateWallet)
                })
            }
            Message::View(ViewMessage::ShareXpubs) => {
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                Command::perform(async move { (datadir_path, network) }, |(d, n)| {
                    Message::Install(d, n, UserFlow::ShareXpubs)
                })
            }
            Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::ShowModal)) => {
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
                Command::none()
            }
            Message::View(ViewMessage::SelectNetwork(network)) => {
                self.network = network;
                Command::perform(
                    check_network_datadir(self.datadir_path.clone(), self.network),
                    Message::Checked,
                )
            }
            Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::Deleted)) => {
                self.state = State::NoWallet;
                Command::none()
            }
            Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::CloseModal)) => {
                self.delete_wallet_modal = None;
                Command::none()
            }
            Message::Checked(res) => match res {
                Err(e) => {
                    self.error = Some(e);
                    Command::none()
                }
                Ok(state) => {
                    self.state = state;
                    Command::none()
                }
            },
            Message::View(ViewMessage::Run) => {
                if matches!(self.state, State::Wallet { .. }) {
                    let datadir_path = self.datadir_path.clone();
                    let mut path = self.datadir_path.clone();
                    path.push(self.network.to_string());
                    path.push(app::config::DEFAULT_FILE_NAME);
                    let cfg = app::Config::from_file(&path).expect("Already checked");
                    let network = self.network;
                    Command::perform(async move { (datadir_path.clone(), cfg, network) }, |m| {
                        Message::Run(m.0, m.1, m.2)
                    })
                } else {
                    Command::none()
                }
            }
            _ => {
                if let Some(modal) = &mut self.delete_wallet_modal {
                    return modal.update(message);
                }
                Command::none()
            }
        }
    }

    pub fn view(&self) -> Element<Message> {
        let content = Into::<Element<ViewMessage>>::into(scrollable(
            Column::new()
                .push(
                    Row::new()
                        .spacing(20)
                        .push(
                            Container::new(image::liana_brand_grey().width(Length::Fixed(200.0)))
                                .width(Length::Fill),
                        )
                        .push(
                            button::secondary(None, "Share Xpubs")
                                .on_press(ViewMessage::ShareXpubs),
                        )
                        .push(
                            pick_list(
                                &NETWORKS[..],
                                Some(self.network),
                                ViewMessage::SelectNetwork,
                            )
                            .style(theme::PickList::Simple)
                            .padding(10),
                        )
                        .align_items(Alignment::Center)
                        .padding(100),
                )
                .push(
                    Container::new(
                        Column::new()
                            .align_items(Alignment::Center)
                            .spacing(30)
                            .push(if matches!(self.state, State::Wallet { .. }) {
                                text("Welcome back").size(50).bold()
                            } else {
                                text("Welcome").size(50).bold()
                            })
                            .push_maybe(self.error.as_ref().map(|e| card::simple(text(e))))
                            .push(match &self.state {
                                State::Unchecked => Column::new(),
                                State::Wallet {
                                    email, checksum, ..
                                } => Column::new().push(
                                    Row::new()
                                        .align_items(Alignment::Center)
                                        .spacing(20)
                                        .push(
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
                                                            .style(color::GREY_3)
                                                    }))
                                                    .push_maybe(email.as_ref().map(|email| {
                                                        Row::new()
                                                            .push(Space::with_width(Length::Fill))
                                                            .push(
                                                                p1_regular(email)
                                                                    .style(color::GREEN),
                                                            )
                                                    })),
                                            )
                                            .on_press(ViewMessage::Run)
                                            .style(theme::Button::Border)
                                            .padding(10)
                                            .width(Length::Fill),
                                        )
                                        .push(
                                            Button::new(icon::trash_icon())
                                                .style(theme::Button::Secondary)
                                                .padding(10)
                                                .on_press(ViewMessage::DeleteWallet(
                                                    DeleteWalletMessage::ShowModal,
                                                )),
                                        ),
                                ),
                                State::NoWallet => Column::new()
                                    .push(
                                        Row::new()
                                            .align_items(Alignment::End)
                                            .spacing(20)
                                            .push(
                                                Container::new(
                                                    Column::new()
                                                        .spacing(20)
                                                        .align_items(Alignment::Center)
                                                        .push(
                                                            image::create_new_wallet_icon()
                                                                .width(Length::Fixed(100.0)),
                                                        )
                                                        .push(
                                                            p1_regular("Create a new wallet")
                                                                .style(color::GREY_3),
                                                        )
                                                        .push(
                                                            button::secondary(None, "Select")
                                                                .width(Length::Fixed(200.0))
                                                                .on_press(
                                                                    ViewMessage::CreateWallet,
                                                                ),
                                                        )
                                                        .align_items(Alignment::Center),
                                                )
                                                .padding(20),
                                            )
                                            .push(
                                                Container::new(
                                                    Column::new()
                                                        .spacing(20)
                                                        .align_items(Alignment::Center)
                                                        .push(
                                                            image::restore_wallet_icon()
                                                                .width(Length::Fixed(100.0)),
                                                        )
                                                        .push(
                                                            p1_regular("Add an existing wallet")
                                                                .style(color::GREY_3),
                                                        )
                                                        .push(
                                                            button::secondary(None, "Select")
                                                                .width(Length::Fixed(200.0))
                                                                .on_press(
                                                                    ViewMessage::ImportWallet,
                                                                ),
                                                        )
                                                        .align_items(Alignment::Center),
                                                )
                                                .padding(20),
                                            ),
                                    )
                                    .align_items(Alignment::Center),
                            })
                            .max_width(500),
                    )
                    .width(Length::Fill)
                    .center_x(),
                )
                .push(Space::with_height(Length::Fixed(100.0))),
        ))
        .map(Message::View);
        let content = if self.network != Network::Bitcoin {
            Column::with_children(vec![network_banner(self.network).into(), content]).into()
        } else {
            content
        };
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
}

#[derive(Debug, Clone)]
pub enum Message {
    View(ViewMessage),
    Install(PathBuf, Network, UserFlow),
    Checked(Result<State, String>),
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

    fn update(&mut self, message: Message) -> Command<Message> {
        if let Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::Confirm)) = message {
            self.warning = None;
            if let Err(e) = std::fs::remove_dir_all(&self.wallet_datadir) {
                self.warning = Some(e);
            } else {
                self.deleted = true;
                return Command::perform(async {}, |_| {
                    Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::Deleted))
                });
            };
        }
        Command::none()
    }
    fn view(&self) -> Element<Message> {
        let mut confirm_button = button::secondary(None, "Delete wallet")
            .width(Length::Fixed(200.0))
            .style(theme::Button::Destructive);
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
                            .style(color::RED)
                            .width(Length::Fill),
                    ))
                    .push(Row::new().push(text(help_text_1)))
                    .push_maybe(
                        help_text_2.map(|t| Row::new().push(p1_regular(t).style(color::GREY_3))),
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
                                .push(icon::circle_check_icon().style(color::GREEN))
                                .push(text("Wallet successfully deleted").style(color::GREEN))
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

async fn check_network_datadir(path: PathBuf, network: Network) -> Result<State, String> {
    let mut config_path = path.clone();
    config_path.push(network.to_string());
    config_path.push(app::config::DEFAULT_FILE_NAME);

    let cfg = match app::Config::from_file(&config_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            if e == app::config::ConfigError::NotFound {
                return Ok(State::NoWallet);
            } else {
                return Err(format!(
                    "Failed to read GUI configuration file in the directory: {}",
                    path.to_string_lossy()
                ));
            }
        }
    };

    if let Some(daemon_config_path) = cfg.daemon_config_path {
        liana::config::Config::from_file(Some(daemon_config_path.clone())).map_err(|e| match e {
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
            return Ok(State::Wallet {
                name: Some(wallet.name),
                checksum: Some(wallet.descriptor_checksum),
                email: wallet.remote_backend_auth.map(|auth| auth.email),
            });
        }
    }
    Ok(State::Wallet {
        name: None,
        checksum: None,
        email: None,
    })
}
