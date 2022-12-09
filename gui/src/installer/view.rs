use iced::widget::{Button, Checkbox, Column, Container, PickList, Row, Scrollable};
use iced::{Alignment, Element, Length};

use liana::miniscript::bitcoin;

use crate::{
    hw::HardwareWallet,
    installer::{
        message::{self, Message},
        step::Context,
        Error,
    },
    ui::{
        color,
        component::{
            button, card, collapse, container, form,
            text::{text, Text},
        },
        icon,
        util::Collection,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Network {
    Mainnet,
    Testnet,
    Regtest,
    Signet,
}

impl From<bitcoin::Network> for Network {
    fn from(n: bitcoin::Network) -> Self {
        match n {
            bitcoin::Network::Bitcoin => Network::Mainnet,
            bitcoin::Network::Testnet => Network::Testnet,
            bitcoin::Network::Regtest => Network::Regtest,
            bitcoin::Network::Signet => Network::Signet,
        }
    }
}

impl From<Network> for bitcoin::Network {
    fn from(network: Network) -> bitcoin::Network {
        match network {
            Network::Mainnet => bitcoin::Network::Bitcoin,
            Network::Testnet => bitcoin::Network::Testnet,
            Network::Regtest => bitcoin::Network::Regtest,
            Network::Signet => bitcoin::Network::Signet,
        }
    }
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Mainnet => write!(f, "Bitcoin mainnet"),
            Self::Testnet => write!(f, "Bitcoin testnet"),
            Self::Regtest => write!(f, "Bitcoin regtest"),
            Self::Signet => write!(f, "Bitcoin signet"),
        }
    }
}

const NETWORKS: [Network; 4] = [
    Network::Mainnet,
    Network::Testnet,
    Network::Signet,
    Network::Regtest,
];

pub fn welcome<'a>() -> Element<'a, Message> {
    Container::new(Container::new(
        Column::new()
            .push(
                Row::new()
                    .spacing(20)
                    .push(
                        Button::new(
                            Container::new(
                                Column::new()
                                    .width(Length::Units(200))
                                    .push(icon::wallet_icon().size(50).width(Length::Units(100)))
                                    .push(text("Create new wallet"))
                                    .align_items(Alignment::Center),
                            )
                            .padding(50),
                        )
                        .style(button::Style::Border.into())
                        .on_press(Message::CreateWallet),
                    )
                    .push(
                        Button::new(
                            Container::new(
                                Column::new()
                                    .width(Length::Units(200))
                                    .push(icon::import_icon().size(50).width(Length::Units(100)))
                                    .push(text("Import wallet"))
                                    .align_items(Alignment::Center),
                            )
                            .padding(50),
                        )
                        .style(button::Style::Border.into())
                        .on_press(Message::ImportWallet),
                    ),
            )
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
    progress: (usize, usize),
    network: bitcoin::Network,
    network_valid: bool,
    user_xpub: &form::Value<String>,
    heir_xpub: &form::Value<String>,
    sequence: &form::Value<String>,
    error: Option<&String>,
) -> Element<'a, Message> {
    let row_network = Row::new()
        .spacing(10)
        .align_items(Alignment::Center)
        .push(text("Network:").bold())
        .push(Container::new(
            PickList::new(&NETWORKS[..], Some(Network::from(network)), |net| {
                Message::Network(net.into())
            })
            .padding(10),
        ))
        .push_maybe(if network_valid {
            None
        } else {
            Some(card::warning(
                "A data directory already exists for this network".to_string(),
            ))
        });

    let col_user_xpub = Column::new()
        .push(text("Your public key:").bold())
        .push(
            Row::new()
                .push(button::border(Some(icon::chip_icon()), "Import").on_press(
                    Message::DefineDescriptor(message::DefineDescriptor::ImportUserHWXpub),
                ))
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
                    .padding(12),
                )
                .push(Container::new(text("/<0;1>/*")))
                .spacing(5)
                .align_items(Alignment::Center),
        )
        .spacing(10);

    let col_heir_xpub = Column::new()
        .push(text("Public key of the recovery key:").bold())
        .push(
            Row::new()
                .push(button::border(Some(icon::chip_icon()), "Import").on_press(
                    Message::DefineDescriptor(message::DefineDescriptor::ImportHeirHWXpub),
                ))
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
                    .padding(12),
                )
                .push(Container::new(text("/<0;1>/*")))
                .spacing(5)
                .align_items(Alignment::Center),
        )
        .spacing(10);

    let col_sequence = Column::new()
        .push(text("Number of block before enabling recovery:").bold())
        .push(
            Container::new(
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
        progress,
        Column::new()
            .push(text("Create the wallet").bold().size(50))
            .push(
                Column::new()
                    .push(row_network)
                    .push(col_user_xpub)
                    .push(col_sequence)
                    .push(col_heir_xpub)
                    .spacing(25),
            )
            .push(
                if user_xpub.value.is_empty()
                    && heir_xpub.value.is_empty()
                    && sequence.value.is_empty()
                {
                    button::primary(None, "Next").width(Length::Units(200))
                } else {
                    button::primary(None, "Next")
                        .width(Length::Units(200))
                        .on_press(Message::Next)
                },
            )
            .push_maybe(error.map(|e| card::error("Failed to create descriptor", e.to_string())))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(100)
            .spacing(50)
            .align_items(Alignment::Center),
    )
}

pub fn import_descriptor<'a>(
    progress: (usize, usize),
    network: bitcoin::Network,
    network_valid: bool,
    imported_descriptor: &form::Value<String>,
    error: Option<&String>,
) -> Element<'a, Message> {
    let row_network = Row::new()
        .spacing(10)
        .align_items(Alignment::Center)
        .push(text("Network:").bold())
        .push(Container::new(
            PickList::new(&NETWORKS[..], Some(Network::from(network)), |net| {
                Message::Network(net.into())
            })
            .padding(10),
        ))
        .push_maybe(if network_valid {
            None
        } else {
            Some(card::warning(
                "A data directory already exists for this network".to_string(),
            ))
        });
    let col_descriptor = Column::new()
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
    layout(
        progress,
        Column::new()
            .push(text("Import the wallet").bold().size(50))
            .push(
                Column::new()
                    .spacing(20)
                    .push(row_network)
                    .push(col_descriptor),
            )
            .push(if imported_descriptor.value.is_empty() {
                button::primary(None, "Next").width(Length::Units(200))
            } else {
                button::primary(None, "Next")
                    .width(Length::Units(200))
                    .on_press(Message::Next)
            })
            .push_maybe(error.map(|e| card::error("Invalid descriptor", e.to_string())))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(100)
            .spacing(50)
            .align_items(Alignment::Center),
    )
}

pub fn register_descriptor<'a>(
    progress: (usize, usize),
    descriptor: String,
    hws: &[(HardwareWallet, Option<[u8; 32]>, bool)],
    error: Option<&Error>,
    processing: bool,
    chosen_hw: Option<usize>,
) -> Element<'a, Message> {
    layout(
        progress,
        Column::new()
            .push(text("Register descriptor").bold().size(50))
            .push(card::simple(
                Column::new()
                    .push(text("The descriptor:").small().bold())
                    .push(text(descriptor.clone()).small())
                    .push(
                        Row::new().push(Column::new().width(Length::Fill)).push(
                            button::transparent_border(Some(icon::clipboard_icon()), "Copy")
                                .on_press(Message::Clibpboard(descriptor)),
                        ),
                    )
                    .spacing(10)
                    .max_width(1000),
            ))
            .push_maybe(error.map(|e| card::error("Failed to register descriptor", e.to_string())))
            .push(
                Column::new()
                    .push(
                        Row::new()
                            .spacing(10)
                            .align_items(Alignment::Center)
                            .push(
                                Container::new(
                                    text(format!("{} hardware wallets connected", hws.len()))
                                        .bold(),
                                )
                                .width(Length::Fill),
                            )
                            .push(
                                button::border(Some(icon::reload_icon()), "Refresh")
                                    .on_press(Message::Reload),
                            ),
                    )
                    .spacing(10)
                    .push(
                        hws.iter()
                            .enumerate()
                            .fold(Column::new().spacing(10), |col, (i, hw)| {
                                col.push(hw_list_view(
                                    i,
                                    &hw.0,
                                    Some(i) == chosen_hw,
                                    processing,
                                    hw.2,
                                ))
                            }),
                    )
                    .width(Length::Fill),
            )
            .push(if processing {
                button::primary(None, "Next").width(Length::Units(200))
            } else {
                button::primary(None, "Next")
                    .on_press(Message::Next)
                    .width(Length::Units(200))
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(100)
            .spacing(50)
            .align_items(Alignment::Center),
    )
}

pub fn backup_descriptor<'a>(
    progress: (usize, usize),
    descriptor: String,
    done: bool,
) -> Element<'a, Message> {
    layout(
        progress,
        Column::new()
            .push(
                text("Did you backup your wallet descriptor ?")
                    .bold()
                    .size(50),
            )
            .push(
                Column::new()
                    .push(text(super::prompt::BACKUP_DESCRIPTOR_MESSAGE))
                    .push(collapse::Collapse::new(
                        || {
                            Button::new(
                                Row::new()
                                    .align_items(Alignment::Center)
                                    .spacing(10)
                                    .push(text("Learn more").small().bold())
                                    .push(icon::collapse_icon()),
                            )
                            .style(button::Style::Transparent.into())
                        },
                        || {
                            Button::new(
                                Row::new()
                                    .align_items(Alignment::Center)
                                    .spacing(10)
                                    .push(text("Learn more").small().bold())
                                    .push(icon::collapsed_icon()),
                            )
                            .style(button::Style::Transparent.into())
                        },
                        help_backup,
                    ))
                    .max_width(1000),
            )
            .push(card::simple(
                Column::new()
                    .push(text("The descriptor:").small().bold())
                    .push(text(descriptor.clone()).small())
                    .push(
                        Row::new().push(Column::new().width(Length::Fill)).push(
                            button::transparent_border(Some(icon::clipboard_icon()), "Copy")
                                .on_press(Message::Clibpboard(descriptor)),
                        ),
                    )
                    .spacing(10)
                    .max_width(1000),
            ))
            .push(Checkbox::new(
                done,
                "I have backed up my descriptor",
                Message::BackupDone,
            ))
            .push(if done {
                button::primary(None, "Next")
                    .on_press(Message::Next)
                    .width(Length::Units(200))
            } else {
                button::primary(None, "Next").width(Length::Units(200))
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(100)
            .spacing(50)
            .align_items(Alignment::Center),
    )
}

pub fn help_backup<'a>() -> Element<'a, Message> {
    text(super::prompt::BACKUP_DESCRIPTOR_HELP).small().into()
}

pub fn define_bitcoin<'a>(
    progress: (usize, usize),
    address: &form::Value<String>,
    cookie_path: &form::Value<String>,
) -> Element<'a, Message> {
    let col_address = Column::new()
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

    let col_cookie = Column::new()
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
        progress,
        Column::new()
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
    progress: (usize, usize),
    context: &Context,
    descriptor: String,
    generating: bool,
    config_path: Option<&std::path::PathBuf>,
    warning: Option<&'a String>,
) -> Element<'a, Message> {
    let mut col = Column::new()
        .push(
            Container::new(
                Column::new()
                    .spacing(10)
                    .push(
                        card::simple(
                            Column::new()
                                .spacing(5)
                                .push(text("Descriptor:").small().bold())
                                .push(text(descriptor).small()),
                        )
                        .width(Length::Fill),
                    )
                    .push(
                        card::simple(
                            Column::new()
                                .spacing(5)
                                .push(text("Hardware devices:").small().bold())
                                .push(context.hws.iter().fold(Column::new(), |acc, hw| {
                                    acc.push(
                                        Row::new()
                                            .spacing(5)
                                            .push(text(hw.0.to_string()).small())
                                            .push(text(format!("(fingerprint: {})", hw.1)).small()),
                                    )
                                })),
                        )
                        .width(Length::Fill),
                    )
                    .push(
                        card::simple(
                            Column::new()
                                .push(text("Bitcoind:").small().bold())
                                .push(
                                    Row::new()
                                        .spacing(5)
                                        .align_items(Alignment::Center)
                                        .push(text("Cookie path:").small())
                                        .push(
                                            text(format!(
                                                "{}",
                                                context
                                                    .bitcoind_config
                                                    .as_ref()
                                                    .unwrap()
                                                    .cookie_path
                                                    .to_string_lossy()
                                            ))
                                            .small(),
                                        ),
                                )
                                .push(
                                    Row::new()
                                        .spacing(5)
                                        .align_items(Alignment::Center)
                                        .push(text("Address:").small())
                                        .push(
                                            text(format!(
                                                "{}",
                                                context.bitcoind_config.as_ref().unwrap().addr
                                            ))
                                            .small(),
                                        ),
                                ),
                        )
                        .width(Length::Fill),
                    ),
            )
            .padding(50)
            .max_width(1000),
        )
        .spacing(50)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_items(Alignment::Center);

    if let Some(error) = warning {
        col = col.push(text(error));
    }

    if generating {
        col = col.push(button::primary(None, "Installing ...").width(Length::Units(200)))
    } else if let Some(path) = config_path {
        col = col.push(
            Container::new(
                Column::new()
                    .push(Container::new(text("Installed !")))
                    .push(Container::new(
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

    layout(progress, col)
}

pub fn hardware_wallet_xpubs_modal<'a>(
    is_heir: bool,
    hws: &[HardwareWallet],
    error: Option<&Error>,
    processing: bool,
    chosen_hw: Option<usize>,
) -> Element<'a, Message> {
    modal(
        Column::new()
            .push(
                text(if is_heir {
                    "Import the recovery public key"
                } else {
                    "Import the user public key"
                })
                .bold()
                .size(50),
            )
            .push_maybe(error.map(|e| card::error("Failed to import xpub", e.to_string())))
            .push(
                Column::new()
                    .push(
                        Row::new()
                            .spacing(10)
                            .align_items(Alignment::Center)
                            .push(
                                Container::new(
                                    text(format!("{} hardware wallets connected", hws.len()))
                                        .bold(),
                                )
                                .width(Length::Fill),
                            )
                            .push(
                                button::border(Some(icon::reload_icon()), "Refresh")
                                    .on_press(Message::Reload),
                            ),
                    )
                    .spacing(10)
                    .push(
                        hws.iter()
                            .enumerate()
                            .fold(Column::new().spacing(10), |col, (i, hw)| {
                                col.push(hw_list_view(
                                    i,
                                    hw,
                                    Some(i) == chosen_hw,
                                    processing,
                                    false,
                                ))
                            }),
                    )
                    .width(Length::Fill),
            )
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
    let mut bttn = Button::new(
        Row::new()
            .push(
                Column::new()
                    .push(text(format!("{}", hw.kind)).bold())
                    .push(text(format!("fingerprint: {}", hw.fingerprint)).small())
                    .spacing(5)
                    .width(Length::Fill),
            )
            .push_maybe(if chosen && processing {
                Some(
                    Column::new()
                        .push(text("Processing..."))
                        .push(text("Please check your device").small()),
                )
            } else {
                None
            })
            .push_maybe(if registered {
                Some(Column::new().push(icon::circle_check_icon().style(color::SUCCESS)))
            } else {
                None
            })
            .align_items(Alignment::Center)
            .width(Length::Fill),
    )
    .padding(10)
    .style(button::Style::TransparentBorder.into())
    .width(Length::Fill);
    if !processing {
        bttn = bttn.on_press(Message::Select(i));
    }
    Container::new(bttn)
        .width(Length::Fill)
        .style(card::SimpleCardStyle)
        .into()
}

fn layout<'a>(
    progress: (usize, usize),
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    Container::new(Scrollable::new(
        Column::new()
            .push(
                Container::new(button::transparent(None, "< Previous").on_press(Message::Previous))
                    .padding(5),
            )
            .push(
                Container::new(text(format!("{}/{}", progress.0, progress.1)))
                    .width(Length::Fill)
                    .center_x(),
            )
            .push(Container::new(content).width(Length::Fill).center_x()),
    ))
    .center_x()
    .height(Length::Fill)
    .width(Length::Fill)
    .style(container::Style::Background)
    .into()
}

fn modal<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Container::new(Scrollable::new(
        Column::new()
            .push(
                Row::new().push(Column::new().width(Length::Fill)).push(
                    Container::new(
                        button::primary(Some(icon::cross_icon()), "Close").on_press(Message::Close),
                    )
                    .padding(10),
                ),
            )
            .push(Container::new(content).width(Length::Fill).center_x()),
    ))
    .center_x()
    .height(Length::Fill)
    .width(Length::Fill)
    .style(container::Style::Background)
    .into()
}
