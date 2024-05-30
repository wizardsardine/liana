use std::path::PathBuf;

use iced::{
    alignment::Horizontal,
    widget::{scrollable, tooltip},
    Alignment, Command, Length, Subscription,
};

use liana::{config::ConfigError, miniscript::bitcoin::Network};
use liana_ui::{
    color,
    component::{badge, button, card, modal::Modal, notification, text::*},
    icon, image, theme,
    widget::*,
};

use crate::app;

fn wallet_name(network: &Network) -> String {
    match network {
        Network::Bitcoin => "Bitcoin Mainnet",
        Network::Testnet => "Bitcoin Testnet",
        Network::Signet => "Bitcoin Signet",
        Network::Regtest => "Bitcoin Regtest",
        _ => "Bitcoin unknown",
    }
    .to_string()
}

pub struct Launcher {
    // true if installed
    choices: Vec<(Network, bool)>,
    datadir_path: PathBuf,
    error: Option<String>,
    delete_wallet_modal: Option<DeleteWalletModal>,
    collapsed: bool,
}

impl Launcher {
    pub fn new(datadir_path: PathBuf) -> Self {
        Self {
            choices: [
                Network::Bitcoin,
                Network::Testnet,
                Network::Signet,
                Network::Regtest,
            ]
            .iter()
            .map(|net| (*net, datadir_path.join(net.to_string()).exists()))
            .collect(),
            datadir_path,
            error: None,
            delete_wallet_modal: None,
            collapsed: false,
        }
    }

    fn is_fresh_install(&self) -> bool {
        !self.choices.iter().any(|(_, installed)| *installed)
    }

    pub fn stop(&mut self) {}

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::View(ViewMessage::ShowUninstalledNetworks) => {
                self.collapsed = true;
                Command::none()
            }
            Message::View(ViewMessage::StartInstall(net)) => {
                let datadir_path = self.datadir_path.clone();
                Command::perform(async move { (datadir_path, net) }, |(d, n)| {
                    Message::Install(d, n)
                })
            }
            Message::View(ViewMessage::Check(network)) => Command::perform(
                check_network_datadir(self.datadir_path.clone(), network),
                Message::Checked,
            ),
            Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::ShowModal(network))) => {
                let wallet_datadir = self.datadir_path.join(network.to_string());
                let config_path = wallet_datadir.join(app::config::DEFAULT_FILE_NAME);
                let internal_bitcoind = if let Ok(cfg) = app::Config::from_file(&config_path) {
                    Some(cfg.start_internal_bitcoind)
                } else {
                    None
                };
                self.delete_wallet_modal = Some(DeleteWalletModal::new(
                    network,
                    wallet_datadir,
                    internal_bitcoind,
                ));
                Command::none()
            }
            Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::Deleted)) => {
                if let Some(modal) = &self.delete_wallet_modal {
                    if let Some(choice) = self.choices.iter_mut().find(|c| c.0 == modal.network) {
                        choice.1 = false;
                    }
                }
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
                Ok(network) => {
                    let datadir_path = self.datadir_path.clone();
                    let mut path = self.datadir_path.clone();
                    path.push(network.to_string());
                    path.push(app::config::DEFAULT_FILE_NAME);
                    let cfg = app::Config::from_file(&path).expect("Already checked");
                    Command::perform(async move { (datadir_path.clone(), cfg, network) }, |m| {
                        Message::Run(m.0, m.1, m.2)
                    })
                }
            },
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
                    Container::new(image::liana_brand_grey().width(Length::Fixed(200.0)))
                        .padding(100),
                )
                .push(
                    Container::new(
                        Column::new()
                            .spacing(30)
                            .push(if !self.is_fresh_install() {
                                text("Welcome back").size(50).bold()
                            } else {
                                text("Welcome").size(50).bold()
                            })
                            .push_maybe(self.error.as_ref().map(|e| card::simple(text(e))))
                            .push(if self.is_fresh_install() {
                                Column::new()
                                    .spacing(10)
                                    .push(
                                        Button::new(
                                            Row::new()
                                                .spacing(20)
                                                .align_items(Alignment::Center)
                                                .push(
                                                    badge::Badge::new(icon::bitcoin_icon())
                                                        .style(theme::Badge::Bitcoin),
                                                )
                                                .push(text(format!(
                                                    "Create wallet on {}",
                                                    wallet_name(&Network::Bitcoin)
                                                ))),
                                        )
                                        .on_press(ViewMessage::StartInstall(Network::Bitcoin))
                                        .padding(10)
                                        .width(Length::Fixed(400.0))
                                        .style(theme::Button::Border),
                                    )
                                    .push(if !self.collapsed {
                                        Column::new().push(
                                            Button::new(
                                                Row::new()
                                                    .spacing(20)
                                                    .align_items(Alignment::Center)
                                                    .push(badge::Badge::new(icon::plus_icon()))
                                                    .push(text("Create wallet on another network")),
                                            )
                                            .on_press(ViewMessage::ShowUninstalledNetworks)
                                            .padding(10)
                                            .width(Length::Fixed(400.0))
                                            .style(theme::Button::TransparentBorder),
                                        )
                                    } else {
                                        self.choices
                                            .iter()
                                            .filter_map(|(net, installed)| {
                                                if *installed || *net == Network::Bitcoin {
                                                    None
                                                } else {
                                                    Some(net)
                                                }
                                            })
                                            .fold(Column::new().spacing(10), |col, choice| {
                                                col.push(
                                                    Button::new(
                                                        Row::new()
                                                            .spacing(20)
                                                            .align_items(Alignment::Center)
                                                            .push(
                                                                badge::Badge::new(
                                                                    icon::bitcoin_icon(),
                                                                )
                                                                .style(theme::Badge::Standard),
                                                            )
                                                            .push(text(format!(
                                                                "Create wallet on {}",
                                                                wallet_name(choice)
                                                            ))),
                                                    )
                                                    .on_press(ViewMessage::StartInstall(*choice))
                                                    .padding(10)
                                                    .width(Length::Fixed(400.0))
                                                    .style(theme::Button::Border),
                                                )
                                            })
                                    })
                            } else {
                                Column::new()
                                    .spacing(10)
                                    .push(
                                        self.choices
                                            .iter()
                                            .filter_map(
                                                |(net, installed)| {
                                                    if *installed {
                                                        Some(net)
                                                    } else {
                                                        None
                                                    }
                                                },
                                            )
                                            .fold(
                                                Column::new()
                                                    .spacing(10),
                                                |col, choice| {
                                                    col.push(
                                                Row::new()
                                                    .spacing(10)
                                                    .push(
                                                        Button::new(
                                                            Row::new()
                                                                .spacing(20)
                                                                .align_items(Alignment::Center)
                                                                .push(
                                                                    badge::Badge::new(
                                                                        icon::bitcoin_icon(),
                                                                    )
                                                                    .style(match choice {
                                                                        Network::Bitcoin => {
                                                                            theme::Badge::Bitcoin
                                                                        }
                                                                        _ => theme::Badge::Standard,
                                                                    }),
                                                                )
                                                                .push(text(format!("Open wallet on {}", choice))),
                                                        )
                                                        .on_press(ViewMessage::Check(*choice))
                                                        .padding(10)
                                                        .width(Length::Fixed(400.0))
                                                        .style(theme::Button::Border),
                                                    )
                                                    .push(tooltip::Tooltip::new(
                                                        Button::new(icon::trash_icon())
                                                            .on_press(ViewMessage::DeleteWallet(
                                                                DeleteWalletMessage::ShowModal(
                                                                    *choice,
                                                                ),
                                                            ))
                                                            .style(
                                                                theme::Button::SecondaryDestructive,
                                                            ),
                                                        "Delete wallet",
                                                        tooltip::Position::Right,
                                                    ))
                                                    .align_items(Alignment::Center),
                                            )
                                                },
                                            ),
                                    )
                                    .push(
                                        if !self.collapsed
                                            && self.choices.iter().any(|(_, installed)| !installed)
                                        {
                                            Column::new().push(
                                                Button::new(
                                                    Row::new()
                                                        .spacing(20)
                                                        .align_items(Alignment::Center)
                                                        .push(badge::Badge::new(icon::plus_icon()))
                                                        .push(text("Create a new wallet")),
                                                )
                                                .on_press(ViewMessage::ShowUninstalledNetworks)
                                                .padding(10)
                                                .width(Length::Fixed(400.0))
                                                .style(theme::Button::TransparentBorder),
                                            )
                                        } else if self.collapsed {
                                            self.choices
                                                .iter()
                                                .filter_map(|(net, installed)| {
                                                    if *installed {
                                                        None
                                                    } else {
                                                        Some(net)
                                                    }
                                                })
                                                .fold(
                                                    Column::new()
                                                        .spacing(10),
                                                    |col, choice| {
                                                        col.push(
                                                    Button::new(
                                                        Row::new()
                                                            .spacing(20)
                                                            .align_items(Alignment::Center)
                                                            .push(
                                                                badge::Badge::new(
                                                                    icon::bitcoin_icon(),
                                                                )
                                                                .style(match choice {
                                                                    Network::Bitcoin => {
                                                                        theme::Badge::Bitcoin
                                                                    }
                                                                    _ => theme::Badge::Standard,
                                                                }),
                                                            )
                                                            .push(text(format!("Create wallet on {}", wallet_name(choice)))),
                                                    )
                                                    .on_press(ViewMessage::StartInstall(*choice))
                                                    .padding(10)
                                                    .width(Length::Fixed(400.0))
                                                    .style(theme::Button::Border),
                                                )
                                                    },
                                                )
                                        } else {
                                            Column::new()
                                        },
                                    )
                            })
                            .align_items(if self.is_fresh_install() {
                                Alignment::Center
                            } else {
                                Alignment::Start
                            })
                            .max_width(500),
                    )
                    .width(Length::Fill)
                    .center_x(),
                ),
        ))
        .map(Message::View);
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
    Install(PathBuf, Network),
    Checked(Result<Network, String>),
    Run(PathBuf, app::config::Config, Network),
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
    StartInstall(Network),
    ShowUninstalledNetworks,
    Check(Network),
    DeleteWallet(DeleteWalletMessage),
}

#[derive(Debug, Clone)]
pub enum DeleteWalletMessage {
    ShowModal(Network),
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
        let mut confirm_button = button::primary(None, "Delete wallet")
            .width(Length::Fixed(200.0))
            .style(theme::Button::Destructive);
        if self.warning.is_none() {
            confirm_button =
                confirm_button.on_press(ViewMessage::DeleteWallet(DeleteWalletMessage::Confirm));
        }
        // Use separate `Row`s for help text in order to have better spacing.
        let help_text_1 = format!(
            "Are you sure you want to delete the wallet and all associated data for {}?",
            wallet_name(&self.network)
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
                        h4_bold(format!("Delete wallet for {}", wallet_name(&self.network)))
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

async fn check_network_datadir(mut path: PathBuf, network: Network) -> Result<Network, String> {
    path.push(network.to_string());
    path.push(app::config::DEFAULT_FILE_NAME);

    let cfg = app::Config::from_file(&path).map_err(|_| {
        format!(
            "Failed to read GUI configuration file in the directory: {}",
            path.to_string_lossy()
        )
    })?;

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

    Ok(network)
}
