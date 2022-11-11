use iced::pure::{column, container, pick_list, row, scrollable, widget, Element};
use iced::{Alignment, Length};

use minisafe::miniscript::bitcoin;

use crate::{
    hw::HardwareWallet,
    installer::{
        message::{self, Message},
        Error,
    },
    ui::{
        color,
        component::{
            button, card, form,
            text::{text, Text},
        },
        icon,
        util::Collection,
    },
};

const NETWORKS: [bitcoin::Network; 4] = [
    bitcoin::Network::Bitcoin,
    bitcoin::Network::Testnet,
    bitcoin::Network::Signet,
    bitcoin::Network::Regtest,
];

pub fn welcome(network: &bitcoin::Network, valid: bool) -> Element<Message> {
    container(container(
        column()
            .push(container(
                pick_list(&NETWORKS[..], Some(*network), message::Message::Network).padding(10),
            ))
            .push(if valid {
                container(
                    button::primary(None, "Start the install")
                        .on_press(Message::Next)
                        .width(Length::Units(200)),
                )
            } else {
                card::warning("A data directory already exists for this network")
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(100)
            .spacing(50)
            .align_items(Alignment::Center),
    ))
    .center_y()
    .center_x()
    .height(Length::Fill)
    .width(Length::Fill)
    .into()
}

pub fn define_descriptor<'a>(
    network: bitcoin::Network,
    imported_descriptor: &form::Value<String>,
    user_xpub: &form::Value<String>,
    heir_xpub: &form::Value<String>,
    sequence: &form::Value<String>,
    error: Option<&String>,
) -> Element<'a, Message> {
    let col_descriptor = column()
        .push(text("Descriptor:").bold())
        .push(
            form::Form::new("Descriptor", imported_descriptor, |msg| {
                Message::DefineDescriptor(message::DefineDescriptor::ImportDescriptor(msg))
            })
            .warning("Please enter correct descriptor")
            .size(20)
            .padding(10),
        )
        .spacing(10);

    let col_user_xpub = column()
        .push(text("Your xpub:").bold())
        .push(
            row()
                .push(
                    form::Form::new("Xpub", user_xpub, |msg| {
                        Message::DefineDescriptor(message::DefineDescriptor::UserXpubEdited(msg))
                    })
                    .warning(if network == bitcoin::Network::Bitcoin {
                        "Please enter correct xpub"
                    } else {
                        "Please enter correct tpub"
                    })
                    .size(20)
                    .padding(10),
                )
                .push(button::primary(Some(icon::chip_icon()), "Import").on_press(
                    Message::DefineDescriptor(message::DefineDescriptor::ImportUserHWXpub),
                ))
                .spacing(5)
                .align_items(Alignment::Center),
        )
        .spacing(10);

    let col_heir_xpub = column()
        .push(text("Heir xpub:").bold())
        .push(
            row()
                .push(
                    form::Form::new("Xpub", heir_xpub, |msg| {
                        Message::DefineDescriptor(message::DefineDescriptor::HeirXpubEdited(msg))
                    })
                    .warning(if network == bitcoin::Network::Bitcoin {
                        "Please enter correct xpub"
                    } else {
                        "Please enter correct tpub"
                    })
                    .size(20)
                    .padding(10),
                )
                .push(button::primary(Some(icon::chip_icon()), "Import").on_press(
                    Message::DefineDescriptor(message::DefineDescriptor::ImportHeirHWXpub),
                ))
                .spacing(5)
                .align_items(Alignment::Center),
        )
        .spacing(10);

    let col_sequence = column()
        .push(text("Number of block:").bold())
        .push(
            container(
                form::Form::new("Number of block", sequence, |msg| {
                    Message::DefineDescriptor(message::DefineDescriptor::SequenceEdited(msg))
                })
                .warning("Please enter correct block number")
                .size(20)
                .padding(10),
            )
            .width(Length::Units(150)),
        )
        .spacing(10);

    layout(
        column()
            .push(text("Create the descriptor").bold().size(50))
            .push(
                column()
                    .push(col_user_xpub)
                    .push(col_sequence)
                    .push(col_heir_xpub)
                    .spacing(20),
            )
            .push(text("or import it").bold().size(25))
            .push(col_descriptor)
            .push(
                if !imported_descriptor.value.is_empty()
                    && (!user_xpub.value.is_empty()
                        || !heir_xpub.value.is_empty()
                        || !sequence.value.is_empty())
                {
                    button::primary(None, "Next").width(Length::Units(200))
                } else {
                    button::primary(None, "Next")
                        .width(Length::Units(200))
                        .on_press(Message::Next)
                },
            )
            .push_maybe(error.map(|e| card::error("Failed to create descriptor", e)))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(100)
            .spacing(50)
            .align_items(Alignment::Center),
    )
}

pub fn register_descriptor<'a>(
    descriptor: &str,
    hws: &[(HardwareWallet, Option<[u8; 32]>)],
    error: Option<&Error>,
    processing: bool,
    chosen_hw: Option<usize>,
) -> Element<'a, Message> {
    layout(
        column()
            .push(text("Register descriptor").bold().size(50))
            .push(
                column()
                    .push(text(descriptor).small())
                    .push(
                        button::transparent_border(Some(icon::clipboard_icon()), "Copy")
                            .on_press(Message::Clibpboard(descriptor.to_string())),
                    )
                    .spacing(10)
                    .align_items(Alignment::Center),
            )
            .push_maybe(error.map(|e| card::error("Failed to import xpub", &e.to_string())))
            .push(if !hws.is_empty() {
                column()
                    .push(text(&format!("{} hardware wallets connected", hws.len())).bold())
                    .spacing(10)
                    .push(
                        hws.iter()
                            .enumerate()
                            .fold(column().spacing(10), |col, (i, hw)| {
                                col.push(hw_list_view(
                                    i,
                                    &hw.0,
                                    Some(i) == chosen_hw,
                                    processing,
                                    hw.1.is_some(),
                                ))
                            }),
                    )
                    .width(Length::Fill)
            } else {
                column().push(card::simple(
                    column()
                        .spacing(20)
                        .push("No hardware wallet connected")
                        .push(button::primary(None, "Refresh").on_press(Message::Reload))
                        .align_items(Alignment::Center)
                        .width(Length::Fill),
                ))
            })
            .push(
                button::primary(None, "Next")
                    .on_press(Message::Next)
                    .width(Length::Units(200)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(100)
            .spacing(50)
            .align_items(Alignment::Center),
    )
}

pub fn define_bitcoin<'a>(
    address: &form::Value<String>,
    cookie_path: &form::Value<String>,
) -> Element<'a, Message> {
    let col_address = column()
        .push(text("Address:").bold())
        .push(
            form::Form::new("Address", address, |msg| {
                Message::DefineBitcoind(message::DefineBitcoind::AddressEdited(msg))
            })
            .warning("Please enter correct address")
            .size(20)
            .padding(10),
        )
        .spacing(10);

    let col_cookie = column()
        .push(text("Cookie path:").bold())
        .push(
            form::Form::new("Cookie path", cookie_path, |msg| {
                Message::DefineBitcoind(message::DefineBitcoind::CookiePathEdited(msg))
            })
            .warning("Please enter correct path")
            .size(20)
            .padding(10),
        )
        .spacing(10);

    layout(
        column()
            .push(
                text("Set up connection to the Bitcoin full node")
                    .bold()
                    .size(50),
            )
            .push(col_address)
            .push(col_cookie)
            .push(
                button::primary(None, "Next")
                    .on_press(Message::Next)
                    .width(Length::Units(200)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(100)
            .spacing(50)
            .align_items(Alignment::Center),
    )
}

pub fn install<'a>(
    generating: bool,
    config_path: Option<&std::path::PathBuf>,
    warning: Option<&String>,
) -> Element<'a, Message> {
    let mut col = column()
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(100)
        .spacing(50)
        .align_items(Alignment::Center);

    if let Some(error) = warning {
        col = col.push(text(error));
    }

    if generating {
        col = col.push(button::primary(None, "Installing ...").width(Length::Units(200)))
    } else if let Some(path) = config_path {
        col = col.push(
            container(
                column()
                    .push(container(text("Installed !")))
                    .push(container(
                        button::primary(None, "Start")
                            .on_press(Message::Exit(path.clone()))
                            .width(Length::Units(200)),
                    ))
                    .align_items(Alignment::Center)
                    .spacing(20),
            )
            .padding(50)
            .width(Length::Fill)
            .center_x(),
        );
    } else {
        col = col.push(
            button::primary(None, "Finalize installation")
                .on_press(Message::Install)
                .width(Length::Units(200)),
        );
    }

    layout(col)
}

pub fn hardware_wallet_xpubs_modal<'a>(
    is_heir: bool,
    hws: &[HardwareWallet],
    error: Option<&Error>,
    processing: bool,
    chosen_hw: Option<usize>,
) -> Element<'a, Message> {
    modal(
        column()
            .push(
                text(if is_heir {
                    "Import the Heir xpub"
                } else {
                    "Import the user xpub"
                })
                .bold()
                .size(50),
            )
            .push_maybe(error.map(|e| card::error("Failed to import xpub", &e.to_string())))
            .push(if !hws.is_empty() {
                column()
                    .push(text(&format!("{} hardware wallets connected", hws.len())).bold())
                    .spacing(10)
                    .push(
                        hws.iter()
                            .enumerate()
                            .fold(column().spacing(10), |col, (i, hw)| {
                                col.push(hw_list_view(
                                    i,
                                    hw,
                                    Some(i) == chosen_hw,
                                    processing,
                                    false,
                                ))
                            }),
                    )
                    .width(Length::Fill)
            } else {
                column()
                    .push(
                        card::simple(
                            column()
                                .spacing(20)
                                .width(Length::Fill)
                                .push("Please connect a hardware wallet")
                                .push(button::primary(None, "Refresh").on_press(Message::Reload))
                                .align_items(Alignment::Center),
                        )
                        .width(Length::Fill),
                    )
                    .width(Length::Fill)
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(100)
            .spacing(50)
            .align_items(Alignment::Center),
    )
}

fn hw_list_view<'a>(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
    processing: bool,
    registered: bool,
) -> Element<'a, Message> {
    let mut bttn = iced::pure::button(
        row()
            .push(
                column()
                    .push(text(&format!("{}", hw.kind)).bold())
                    .push(text(&format!("fingerprint: {}", hw.fingerprint)).small())
                    .spacing(5)
                    .width(Length::Fill),
            )
            .push_maybe(if chosen && processing {
                Some(
                    column()
                        .push(text("Processing..."))
                        .push(text("Please check your device").small()),
                )
            } else {
                None
            })
            .push_maybe(if registered {
                Some(column().push(icon::circle_check_icon().color(color::SUCCESS)))
            } else {
                None
            })
            .align_items(Alignment::Center)
            .width(Length::Fill),
    )
    .padding(10)
    .style(button::Style::TransparentBorder)
    .width(Length::Fill);
    if !processing {
        bttn = bttn.on_press(Message::Select(i));
    }
    container(bttn)
        .width(Length::Fill)
        .style(card::SimpleCardStyle)
        .into()
}

fn layout<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(scrollable(
        column()
            .push(
                container(button::transparent(None, "< Previous").on_press(Message::Previous))
                    .padding(5),
            )
            .push(container(content).width(Length::Fill).center_x()),
    ))
    .center_x()
    .height(Length::Fill)
    .width(Length::Fill)
    .style(BackgroundStyle)
    .into()
}

fn modal<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(scrollable(
        column()
            .push(
                row().push(column().width(Length::Fill)).push(
                    container(
                        button::primary(Some(icon::cross_icon()), "Close").on_press(Message::Close),
                    )
                    .padding(10),
                ),
            )
            .push(container(content).width(Length::Fill).center_x()),
    ))
    .center_x()
    .height(Length::Fill)
    .width(Length::Fill)
    .style(BackgroundStyle)
    .into()
}

pub struct BackgroundStyle;
impl widget::container::StyleSheet for BackgroundStyle {
    fn style(&self) -> widget::container::Style {
        widget::container::Style {
            background: color::BACKGROUND.into(),
            ..widget::container::Style::default()
        }
    }
}
