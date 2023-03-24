use std::path::PathBuf;

use iced::{Alignment, Command, Length, Subscription};

use liana::{config::ConfigError, miniscript::bitcoin::Network};
use liana_ui::{
    component::{badge, card, text::*},
    icon, theme,
    util::*,
    widget::*,
};

use crate::app;

pub struct Launcher {
    choices: Vec<Network>,
    datadir_path: PathBuf,
    error: Option<String>,
}

impl Launcher {
    pub fn new(datadir_path: PathBuf) -> Self {
        let mut choices = Vec::new();
        for network in [
            Network::Bitcoin,
            Network::Testnet,
            Network::Signet,
            Network::Regtest,
        ] {
            if datadir_path.join(network.to_string()).exists() {
                choices.push(network)
            }
        }
        Self {
            datadir_path,
            choices,
            error: None,
        }
    }

    pub fn stop(&mut self) {}

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::View(ViewMessage::StartInstall) => {
                let datadir_path = self.datadir_path.clone();
                Command::perform(async move { datadir_path }, Message::Install)
            }
            Message::View(ViewMessage::Check(network)) => Command::perform(
                check_network_datadir(self.datadir_path.clone(), network),
                Message::Checked,
            ),
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
            _ => Command::none(),
        }
    }

    pub fn view(&self) -> Element<Message> {
        Into::<Element<ViewMessage>>::into(
            Container::new(
                Column::new()
                    .spacing(30)
                    .push(text("Welcome back").size(50).bold())
                    .push_maybe(self.error.as_ref().map(|e| card::simple(text(e))))
                    .push(
                        self.choices
                            .iter()
                            .fold(
                                Column::new()
                                    .push(text("Select network:").small().bold())
                                    .spacing(10),
                                |col, choice| {
                                    col.push(
                                        Button::new(
                                            Row::new()
                                                .spacing(20)
                                                .align_items(Alignment::Center)
                                                .push(
                                                    badge::Badge::new(icon::bitcoin_icon()).style(
                                                        match choice {
                                                            Network::Bitcoin => {
                                                                theme::Badge::Bitcoin
                                                            }
                                                            _ => theme::Badge::Standard,
                                                        },
                                                    ),
                                                )
                                                .push(text(match choice {
                                                    Network::Bitcoin => "Bitcoin Mainnet",
                                                    Network::Testnet => "Bitcoin Testnet",
                                                    Network::Signet => "Bitcoin Signet",
                                                    Network::Regtest => "Bitcoin Regtest",
                                                })),
                                        )
                                        .on_press(ViewMessage::Check(*choice))
                                        .padding(10)
                                        .width(Length::Fill)
                                        .style(theme::Button::Secondary),
                                    )
                                },
                            )
                            .push(
                                Button::new(
                                    Row::new()
                                        .spacing(20)
                                        .align_items(Alignment::Center)
                                        .push(badge::Badge::new(icon::plus_icon()))
                                        .push(text("Install Liana on another network")),
                                )
                                .on_press(ViewMessage::StartInstall)
                                .padding(10)
                                .width(Length::Fill)
                                .style(theme::Button::TransparentBorder),
                            ),
                    )
                    .max_width(500)
                    .align_items(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .center_y(),
        )
        .map(Message::View)
    }
}

#[derive(Debug)]
pub enum Message {
    View(ViewMessage),
    Install(PathBuf),
    Checked(Result<Network, String>),
    Run(PathBuf, app::config::Config, Network),
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
    StartInstall,
    Check(Network),
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
