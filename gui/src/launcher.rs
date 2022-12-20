use std::path::PathBuf;

use iced::{
    widget::{Button, Column, Container, Row},
    Alignment, Element, Length, Subscription,
};

use liana::miniscript::bitcoin::Network;

use crate::ui::{
    component::{badge, button, text::*},
    icon,
};

pub struct Launcher {
    should_exit: bool,
    choices: Vec<Network>,
    pub datadir_path: PathBuf,
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
            should_exit: false,
        }
    }

    pub fn stop(&mut self) {
        self.should_exit = true;
    }

    pub fn should_exit(&self) -> bool {
        self.should_exit
    }

    pub fn subscription(&self) -> Subscription<Message> {
        iced_native::subscription::events().map(Message::Event)
    }

    pub fn view(&self) -> Element<Message> {
        Container::new(
            Column::new()
                .spacing(30)
                .push(text("Welcome back").size(50).bold())
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
                                            .push(badge::Badge::new(icon::bitcoin_icon()).style(
                                                match choice {
                                                    Network::Bitcoin => badge::Style::Bitcoin,
                                                    _ => badge::Style::Standard,
                                                },
                                            ))
                                            .push(text(match choice {
                                                Network::Bitcoin => "Bitcoin Mainnet",
                                                Network::Testnet => "Bitcoin Testnet",
                                                Network::Signet => "Bitcoin Signet",
                                                Network::Regtest => "Bitcoin Regtest",
                                            })),
                                    )
                                    .on_press(Message::Run(*choice))
                                    .padding(10)
                                    .width(Length::Fill)
                                    .style(button::Style::Border.into()),
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
                            .on_press(Message::Install)
                            .padding(10)
                            .width(Length::Fill)
                            .style(button::Style::TransparentBorder.into()),
                        ),
                )
                .max_width(500)
                .align_items(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x()
        .center_y()
        .into()
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Event(iced_native::Event),
    Install,
    Run(Network),
}
