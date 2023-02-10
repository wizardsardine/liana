use iced::widget::{
    scrollable::Properties, Button, Checkbox, Column, Container, PickList, Row, Scrollable, Space,
};
use iced::{alignment, Alignment, Element, Length};

use std::collections::HashSet;

use liana::miniscript::bitcoin;

use crate::{
    hw::HardwareWallet,
    installer::{
        context::Context,
        message::{self, Message},
        prompt, Error,
    },
    ui::{
        color,
        component::{
            button, card, collapse, container, form, separation,
            text::{text, Text},
            tooltip,
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
                                    .width(Length::Units(250))
                                    .push(icon::wallet_icon().size(50).width(Length::Units(100)))
                                    .push(text("Create a new wallet"))
                                    .align_items(Alignment::Center),
                            )
                            .padding(20),
                        )
                        .style(button::Style::Border.into())
                        .on_press(Message::CreateWallet),
                    )
                    .push(
                        Button::new(
                            Container::new(
                                Column::new()
                                    .width(Length::Units(250))
                                    .push(icon::people_icon().size(50).width(Length::Units(100)))
                                    .push(text("Participate in a new wallet"))
                                    .align_items(Alignment::Center),
                            )
                            .padding(20),
                        )
                        .style(button::Style::Border.into())
                        .on_press(Message::ParticipateWallet),
                    )
                    .push(
                        Button::new(
                            Container::new(
                                Column::new()
                                    .width(Length::Units(250))
                                    .push(icon::import_icon().size(50).width(Length::Units(100)))
                                    .push(text("Import a wallet backup"))
                                    .align_items(Alignment::Center),
                            )
                            .padding(20),
                        )
                        .style(button::Style::Border.into())
                        .on_press(Message::ImportWallet),
                    ),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .spacing(50)
            .align_items(Alignment::Center),
    ))
    .center_y()
    .center_x()
    .height(Length::Fill)
    .width(Length::Fill)
    .into()
}

#[allow(clippy::too_many_arguments)]
pub fn define_descriptor<'a>(
    progress: (usize, usize),
    network: bitcoin::Network,
    network_valid: bool,
    spending_keys: Vec<Element<'a, Message>>,
    recovery_keys: Vec<Element<'a, Message>>,
    sequence: &form::Value<String>,
    spending_threshold: usize,
    recovery_threshold: usize,
    valid: bool,
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
        })
        .padding(50);

    let col_spending_keys = Column::new()
        .push(
            Row::new()
                .spacing(10)
                .push(Space::with_width(Length::Units(40)))
                .push(text("Primary path:").bold())
                .push(tooltip(prompt::DEFINE_DESCRIPTOR_PRIMATRY_PATH_TOOLTIP)),
        )
        .push(separation().width(Length::Fill))
        .push(
            Container::new(
                Row::new()
                    .align_items(Alignment::Center)
                    .push_maybe(if spending_keys.len() > 1 {
                        Some(threshsold_input::threshsold_input(
                            spending_threshold,
                            spending_keys.len(),
                            |value| {
                                Message::DefineDescriptor(
                                    message::DefineDescriptor::ThresholdEdited(false, value),
                                )
                            },
                        ))
                    } else {
                        None
                    })
                    .push(
                        Scrollable::new(
                            Row::new()
                                .spacing(5)
                                .align_items(Alignment::Center)
                                .push(Row::with_children(spending_keys).spacing(5))
                                .push(
                                    Button::new(
                                        Container::new(icon::plus_icon().size(50))
                                            .width(Length::Units(200))
                                            .height(Length::Units(200))
                                            .align_y(alignment::Vertical::Center)
                                            .align_x(alignment::Horizontal::Center),
                                    )
                                    .width(Length::Units(200))
                                    .height(Length::Units(200))
                                    .style(button::Style::TransparentBorder.into())
                                    .on_press(
                                        Message::DefineDescriptor(
                                            message::DefineDescriptor::AddKey(false),
                                        ),
                                    ),
                                )
                                .padding(5),
                        )
                        .horizontal_scroll(Properties::new().width(3).scroller_width(3)),
                    ),
            )
            .width(Length::Fill)
            .align_x(alignment::Horizontal::Center),
        )
        .spacing(10);

    let col_recovery_keys = Column::new()
        .push(
            Row::new()
                .push(Space::with_width(Length::Units(50)))
                .push(text("Recovery path:").bold()),
        )
        .push(separation().width(Length::Fill))
        .push(
            Container::new(
                Row::new()
                    .align_items(Alignment::Center)
                    .push_maybe(if recovery_keys.len() > 1 {
                        Some(threshsold_input::threshsold_input(
                            recovery_threshold,
                            recovery_keys.len(),
                            |value| {
                                Message::DefineDescriptor(
                                    message::DefineDescriptor::ThresholdEdited(true, value),
                                )
                            },
                        ))
                    } else {
                        None
                    })
                    .push(
                        Scrollable::new(
                            Row::new()
                                .spacing(5)
                                .align_items(Alignment::Center)
                                .push(Row::with_children(recovery_keys).spacing(5))
                                .push(
                                    Button::new(
                                        Container::new(icon::plus_icon().size(50))
                                            .width(Length::Units(200))
                                            .height(Length::Units(200))
                                            .align_y(alignment::Vertical::Center)
                                            .align_x(alignment::Horizontal::Center),
                                    )
                                    .width(Length::Units(200))
                                    .height(Length::Units(200))
                                    .style(button::Style::TransparentBorder.into())
                                    .on_press(
                                        Message::DefineDescriptor(
                                            message::DefineDescriptor::AddKey(true),
                                        ),
                                    ),
                                )
                                .padding(5),
                        )
                        .horizontal_scroll(Properties::new().width(3).scroller_width(3)),
                    ),
            )
            .width(Length::Fill)
            .align_x(alignment::Horizontal::Center),
        )
        .spacing(10);

    let col_sequence = Container::new(
        Row::new()
            .spacing(50)
            .align_items(Alignment::Center)
            .push(Container::new(icon::arrow_down().size(50)).align_x(alignment::Horizontal::Right))
            .push(
                Column::new()
                    .push(
                        Row::new()
                            .spacing(10)
                            .push(text("Blocks before recovery:").bold())
                            .push(tooltip(prompt::DEFINE_DESCRIPTOR_SEQUENCE_TOOLTIP)),
                    )
                    .push(
                        Container::new(
                            form::Form::new("Number of blocks", sequence, |msg| {
                                Message::DefineDescriptor(
                                    message::DefineDescriptor::SequenceEdited(msg),
                                )
                            })
                            .warning("Please enter correct block number")
                            .size(20)
                            .padding(10),
                        )
                        .width(Length::Units(150)),
                    )
                    .spacing(10),
            )
            .padding(20),
    )
    .width(Length::Fill)
    .align_x(alignment::Horizontal::Center);

    layout(
        progress,
        Column::new()
            .push(Space::with_height(Length::Units(30)))
            .push(text("Create the wallet").bold().size(50))
            .push(
                Column::new()
                    .push(row_network)
                    .push(col_spending_keys)
                    .push(col_sequence)
                    .push(col_recovery_keys)
                    .spacing(25),
            )
            .push(if !valid {
                button::primary(None, "Next").width(Length::Units(200))
            } else {
                button::primary(None, "Next")
                    .width(Length::Units(200))
                    .on_press(Message::Next)
            })
            .push_maybe(error.map(|e| card::error("Failed to create descriptor", e.to_string())))
            .push(Space::with_height(Length::Units(20)))
            .width(Length::Fill)
            .height(Length::Fill)
            .spacing(50)
            .align_items(Alignment::Center),
    )
}

pub fn import_descriptor<'a>(
    progress: (usize, usize),
    change_network: bool,
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
            .warning("Incompatible descriptor. Note that starting from v0.2 Liana requires extended keys in a descriptor to have an origin.")
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
                    .push_maybe(if change_network {
                        Some(row_network)
                    } else {
                        None
                    })
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

pub fn signer_xpubs(xpubs: &Vec<String>) -> Element<Message> {
    Container::new(
        Column::new()
            .push(
                Button::new(
                    Row::new().align_items(Alignment::Center).push(
                        Column::new()
                            .push(text("This computer").bold())
                            .push(
                                text("Derive a key from a mnemonic stored on this computer")
                                    .small(),
                            )
                            .spacing(5)
                            .width(Length::Fill),
                    ),
                )
                .on_press(Message::UseHotSigner)
                .padding(10)
                .style(button::Style::TransparentBorder.into())
                .width(Length::Fill),
            )
            .push_maybe(if xpubs.is_empty() {
                None
            } else {
                Some(separation().width(Length::Fill))
            })
            .push_maybe(if xpubs.is_empty() {
                None
            } else {
                Some(xpubs.iter().fold(Column::new().padding(15), |col, xpub| {
                    col.push(
                        Row::new()
                            .spacing(5)
                            .align_items(Alignment::Center)
                            .push(
                                Container::new(
                                    Scrollable::new(Container::new(text(xpub).small()).padding(10))
                                        .horizontal_scroll(
                                            Properties::new().width(2).scroller_width(2),
                                        ),
                                )
                                .width(Length::Fill),
                            )
                            .push(
                                Container::new(
                                    button::border(Some(icon::clipboard_icon()), "Copy")
                                        .on_press(Message::Clibpboard(xpub.clone()))
                                        .width(Length::Shrink),
                                )
                                .padding(10),
                            ),
                    )
                }))
            })
            .push_maybe(if !xpubs.is_empty() {
                Some(
                    Container::new(
                        button::border(Some(icon::plus_icon()), "New public key")
                            .on_press(Message::UseHotSigner),
                    )
                    .padding(10),
                )
            } else {
                None
            }),
    )
    .style(card::SimpleCardStyle)
    .into()
}

pub fn hardware_wallet_xpubs<'a>(
    i: usize,
    xpubs: &'a Vec<String>,
    hw: &'a HardwareWallet,
    processing: bool,
    error: Option<&Error>,
) -> Element<'a, Message> {
    let mut bttn = Button::new(
        Row::new()
            .align_items(Alignment::Center)
            .push(
                Column::new()
                    .push(text(format!("{}", hw.kind())).bold())
                    .push(match hw {
                        HardwareWallet::Supported {
                            fingerprint,
                            version,
                            ..
                        } => Row::new()
                            .spacing(5)
                            .push(text(format!("fingerprint: {}", fingerprint)).small())
                            .push_maybe(
                                version
                                    .as_ref()
                                    .map(|v| text(format!("version: {}", v)).small()),
                            ),
                        HardwareWallet::Unsupported {
                            version, message, ..
                        } => Row::new()
                            .spacing(5)
                            .push_maybe(
                                version
                                    .as_ref()
                                    .map(|v| text(format!("version: {}", v)).small()),
                            )
                            .push(
                                iced::widget::tooltip::Tooltip::new(
                                    icon::warning_icon(),
                                    message,
                                    iced::widget::tooltip::Position::Bottom,
                                )
                                .style(card::SimpleCardStyle),
                            ),
                    })
                    .spacing(5)
                    .width(Length::Fill),
            )
            .push_maybe(error.map(|e| {
                iced::widget::tooltip(
                    Row::new()
                        .spacing(5)
                        .align_items(Alignment::Center)
                        .push(icon::warning_icon().style(color::ALERT))
                        .push(text("An error occured").style(color::ALERT)),
                    e,
                    iced::widget::tooltip::Position::Bottom,
                )
                .style(card::ErrorCardStyle)
            })),
    )
    .padding(10)
    .style(button::Style::TransparentBorder.into())
    .width(Length::Fill);
    if !processing && hw.is_supported() {
        bttn = bttn.on_press(Message::Select(i));
    }
    Container::new(
        Column::new()
            .push(bttn)
            .push_maybe(if xpubs.is_empty() {
                None
            } else {
                Some(separation().width(Length::Fill))
            })
            .push_maybe(if xpubs.is_empty() {
                None
            } else {
                Some(xpubs.iter().fold(Column::new().padding(15), |col, xpub| {
                    col.push(
                        Row::new()
                            .spacing(5)
                            .align_items(Alignment::Center)
                            .push(
                                Container::new(
                                    Scrollable::new(Container::new(text(xpub).small()).padding(10))
                                        .horizontal_scroll(
                                            Properties::new().width(2).scroller_width(2),
                                        ),
                                )
                                .width(Length::Fill),
                            )
                            .push(
                                Container::new(
                                    button::border(Some(icon::clipboard_icon()), "Copy")
                                        .on_press(Message::Clibpboard(xpub.clone()))
                                        .width(Length::Shrink),
                                )
                                .padding(10),
                            ),
                    )
                }))
            })
            .push_maybe(if !xpubs.is_empty() {
                Some(
                    Container::new(if !processing {
                        button::border(Some(icon::plus_icon()), "New public key")
                            .on_press(Message::Select(i))
                    } else {
                        button::border(Some(icon::plus_icon()), "New public key")
                    })
                    .padding(10),
                )
            } else {
                None
            }),
    )
    .style(card::SimpleCardStyle)
    .into()
}

pub fn participate_xpub<'a>(
    progress: (usize, usize),
    network: bitcoin::Network,
    network_valid: bool,
    hws: Vec<Element<'a, Message>>,
    signer: Element<'a, Message>,
    shared: bool,
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

    layout(
        progress,
        Column::new()
            .push(text("Share your public keys").bold().size(50))
            .push(
                Column::new()
                    .spacing(20)
                    .width(Length::Fill)
                    .push(row_network),
            )
            .push(
                Column::new()
                    .push(
                        Row::new()
                            .spacing(10)
                            .align_items(Alignment::Center)
                            .push(
                                Container::new(text("Generate an extended public key by selecting a signing device:").bold())
                                    .width(Length::Fill),
                            )
                            .push(
                                button::border(Some(icon::reload_icon()), "Refresh")
                                    .on_press(Message::Reload),
                            ),
                    )
                    .spacing(10)
                    .push(Column::with_children(hws).spacing(10))
                    .push(signer)
                    .width(Length::Fill),
            )
            .push(Checkbox::new(
                "I have shared my public keys",
                shared,
                Message::UserActionDone,
            ))
            .push(if shared {
                button::primary(None, "Next")
                    .width(Length::Units(200))
                    .on_press(Message::Next)
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

#[allow(clippy::too_many_arguments)]
pub fn register_descriptor<'a>(
    progress: (usize, usize),
    descriptor: String,
    hws: &'a [HardwareWallet],
    registered: &HashSet<bitcoin::util::bip32::Fingerprint>,
    error: Option<&Error>,
    processing: bool,
    chosen_hw: Option<usize>,
    done: bool,
) -> Element<'a, Message> {
    layout(
        progress,
        Column::new()
            .max_width(1000)
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
                    .spacing(10),
            ))
            .push(text(prompt::REGISTER_DESCRIPTOR_HELP))
            .push_maybe(error.map(|e| card::error("Failed to register descriptor", e.to_string())))
            .push(
                Column::new()
                    .push(
                        Row::new()
                            .spacing(10)
                            .align_items(Alignment::Center)
                            .push(
                                Container::new(
                                    text("Select hardware wallet to register descriptor on:")
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
                                    hw.fingerprint()
                                        .map(|fg| registered.contains(&fg))
                                        .unwrap_or(false),
                                ))
                            }),
                    )
                    .width(Length::Fill),
            )
            .push(Checkbox::new(
                "I have registered the descriptor on my device(s)",
                done,
                Message::UserActionDone,
            ))
            .push(if done && !processing {
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
                    .push(text(prompt::BACKUP_DESCRIPTOR_MESSAGE))
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
                "I have backed up my descriptor",
                done,
                Message::UserActionDone,
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
    text(prompt::BACKUP_DESCRIPTOR_HELP).small().into()
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

pub fn undefined_descriptor_key<'a>() -> Element<'a, message::DefineKey> {
    card::simple(
        Column::new()
            .width(Length::Fill)
            .align_items(Alignment::Center)
            .push(
                Row::new()
                    .align_items(Alignment::Center)
                    .push(Space::with_width(Length::Fill))
                    .push(
                        Button::new(icon::cross_icon())
                            .style(button::Style::Transparent.into())
                            .on_press(message::DefineKey::Delete),
                    ),
            )
            .push(
                Container::new(
                    Column::new()
                        .spacing(15)
                        .align_items(Alignment::Center)
                        .push(
                            Scrollable::new(
                                icon::key_icon()
                                    .style(color::DARK_GREY)
                                    .size(50)
                                    .width(Length::Units(50)),
                            )
                            .horizontal_scroll(Properties::new().width(2).scroller_width(2)),
                        )
                        .push(icon::circle_check_icon().style(color::FOREGROUND).size(50)),
                )
                .height(Length::Fill)
                .align_y(alignment::Vertical::Center),
            )
            .push(
                button::border(Some(icon::pencil_icon()), "Set").on_press(message::DefineKey::Edit),
            )
            .push(Space::with_height(Length::Units(5))),
    )
    .padding(5)
    .height(Length::Units(200))
    .width(Length::Units(200))
    .into()
}

pub fn defined_descriptor_key(
    name: &str,
    valid: bool,
    duplicate_key: bool,
    duplicate_name: bool,
) -> Element<message::DefineKey> {
    let col = Column::new()
        .width(Length::Fill)
        .align_items(Alignment::Center)
        .push(
            Row::new()
                .align_items(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    Button::new(icon::cross_icon())
                        .style(button::Style::Transparent.into())
                        .on_press(message::DefineKey::Delete),
                ),
        )
        .push(
            Column::new()
                .align_items(Alignment::Center)
                .spacing(5)
                .push(
                    Container::new(
                        Column::new()
                            .spacing(15)
                            .align_items(Alignment::Center)
                            .push(
                                Scrollable::new(text(name).bold()).horizontal_scroll(
                                    Properties::new().width(2).scroller_width(2),
                                ),
                            )
                            .push(
                                icon::circle_check_icon()
                                    .style(color::SUCCESS)
                                    .size(40)
                                    .width(Length::Units(50)),
                            ),
                    )
                    .height(Length::Fill)
                    .align_y(alignment::Vertical::Center),
                )
                .height(Length::Fill),
        )
        .push(button::border(Some(icon::pencil_icon()), "Edit").on_press(message::DefineKey::Edit))
        .push(Space::with_height(Length::Units(5)));

    if !valid {
        Column::new()
            .align_items(Alignment::Center)
            .push(
                card::invalid(col)
                    .padding(5)
                    .height(Length::Units(200))
                    .width(Length::Units(200)),
            )
            .push(
                text("Key is for a different network")
                    .small()
                    .style(color::ALERT),
            )
            .into()
    } else if duplicate_key {
        Column::new()
            .align_items(Alignment::Center)
            .push(
                card::invalid(col)
                    .padding(5)
                    .height(Length::Units(200))
                    .width(Length::Units(200)),
            )
            .push(text("Duplicate key").small().style(color::ALERT))
            .into()
    } else if duplicate_name {
        Column::new()
            .align_items(Alignment::Center)
            .push(
                card::invalid(col)
                    .padding(5)
                    .height(Length::Units(200))
                    .width(Length::Units(200)),
            )
            .push(text("Duplicate name").small().style(color::ALERT))
            .into()
    } else {
        card::simple(col)
            .padding(5)
            .height(Length::Units(200))
            .width(Length::Units(200))
            .into()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn edit_key_modal<'a>(
    network: bitcoin::Network,
    hws: &'a [HardwareWallet],
    error: Option<&Error>,
    processing: bool,
    chosen_hw: Option<usize>,
    chosen_signer: bool,
    form_xpub: &form::Value<String>,
    form_name: &'a form::Value<String>,
    edit_name: bool,
) -> Element<'a, Message> {
    Column::new()
        .push_maybe(error.map(|e| card::error("Failed to import xpub", e.to_string())))
        .push(card::simple(
            Column::new()
                .spacing(25)
                .push(
                    Column::new()
                        .push(
                            Row::new()
                                .spacing(10)
                                .align_items(Alignment::Center)
                                .push(
                                    Container::new(text("Select a signing device:").bold())
                                        .width(Length::Fill),
                                )
                                .push(
                                    button::border(Some(icon::reload_icon()), "Refresh")
                                        .on_press(Message::Reload),
                                ),
                        )
                        .spacing(10)
                        .push(hws.iter().enumerate().fold(
                            Column::new().spacing(10),
                            |col, (i, hw)| {
                                col.push(hw_list_view(
                                    i,
                                    hw,
                                    Some(i) == chosen_hw,
                                    processing,
                                    !processing
                                        && Some(i) == chosen_hw
                                        && form_xpub.valid
                                        && !form_xpub.value.is_empty(),
                                ))
                            },
                        ))
                        .push(
                            Button::new(
                                Row::new()
                                    .padding(5)
                                    .width(Length::Fill)
                                    .align_items(Alignment::Center)
                                    .push(
                                        Column::new()
                                            .spacing(5)
                                            .push(text("This computer").bold())
                                            .push(
                                                text("Derive a key from a mnemonic stored on this computer").small(),
                                            )
                                            .width(Length::Fill),
                                    )
                                    .push_maybe(if chosen_signer {
                                        Some(icon::circle_check_icon().style(color::SUCCESS))
                                    } else {
                                        None
                                    })
                                    .spacing(10),
                            )
                            .width(Length::Fill)
                            .on_press(Message::UseHotSigner)
                            .style(button::Style::Border.into()),
                        )
                        .width(Length::Fill),
                )
                .push(
                    Column::new()
                        .spacing(5)
                        .push(text("Or enter an extended public key:").bold())
                        .push(
                            Row::new()
                                .push(
                                    form::Form::new("Extended public key", form_xpub, |msg| {
                                        Message::DefineDescriptor(
                                            message::DefineDescriptor::XPubEdited(msg),
                                        )
                                    })
                                    .warning(if network == bitcoin::Network::Bitcoin {
                                        "Please enter correct xpub with origin"
                                    } else {
                                        "Please enter correct tpub with origin"
                                    })
                                    .size(20)
                                    .padding(10),
                                )
                                .spacing(10)
                                .push(Container::new(text("/<0;1>/*")).padding(5)),
                        ),
                )
                .push(
                    if !edit_name && !form_xpub.value.is_empty() && form_xpub.valid {
                        Column::new().push(
                            Row::new()
                                .push(
                                    Column::new()
                                        .spacing(5)
                                        .width(Length::Fill)
                                        .push(
                                            Row::new()
                                                .spacing(5)
                                                .push(text("Fingerprint alias:").bold())
                                                .push(tooltip(
                                                    prompt::DEFINE_DESCRIPTOR_FINGERPRINT_TOOLTIP,
                                                )),
                                        )
                                        .push(text(&form_name.value)),
                                )
                                .push(button::border(Some(icon::pencil_icon()), "Edit").on_press(
                                    Message::DefineDescriptor(message::DefineDescriptor::EditName),
                                )),
                        )
                    } else if !form_xpub.value.is_empty() && form_xpub.valid {
                        Column::new()
                            .spacing(5)
                            .push(
                                Row::new()
                                    .spacing(5)
                                    .push(text("Fingerprint alias:").bold())
                                    .push(tooltip(prompt::DEFINE_DESCRIPTOR_FINGERPRINT_TOOLTIP)),
                            )
                            .push(
                                form::Form::new("Alias", form_name, |msg| {
                                    Message::DefineDescriptor(
                                        message::DefineDescriptor::NameEdited(msg),
                                    )
                                })
                                .warning("Please enter correct alias")
                                .size(20)
                                .padding(10),
                            )
                    } else {
                        Column::new()
                    },
                )
                .push(
                    if form_xpub.valid && !form_xpub.value.is_empty() && !form_name.value.is_empty()
                    {
                        button::primary(None, "Apply")
                            .on_press(Message::DefineDescriptor(
                                message::DefineDescriptor::ConfirmXpub,
                            ))
                            .width(Length::Units(200))
                    } else {
                        button::primary(None, "Apply").width(Length::Units(100))
                    },
                )
                .align_items(Alignment::Center),
        ))
        .width(Length::Units(600))
        .into()
}

fn hw_list_view(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
    processing: bool,
    registered: bool,
) -> Element<Message> {
    let mut bttn = Button::new(
        Row::new()
            .push(
                Column::new()
                    .push(text(format!("{}", hw.kind())).bold())
                    .push(match hw {
                        HardwareWallet::Supported {
                            fingerprint,
                            version,
                            ..
                        } => Row::new()
                            .spacing(5)
                            .push(text(format!("fingerprint: {}", fingerprint)).small())
                            .push_maybe(
                                version
                                    .as_ref()
                                    .map(|v| text(format!("version: {}", v)).small()),
                            ),
                        HardwareWallet::Unsupported {
                            version, message, ..
                        } => Row::new()
                            .spacing(5)
                            .push_maybe(
                                version
                                    .as_ref()
                                    .map(|v| text(format!("version: {}", v)).small()),
                            )
                            .push(
                                iced::widget::tooltip::Tooltip::new(
                                    icon::warning_icon(),
                                    message,
                                    iced::widget::tooltip::Position::Bottom,
                                )
                                .style(card::SimpleCardStyle),
                            ),
                    })
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
    if !processing && hw.is_supported() {
        bttn = bttn.on_press(Message::Select(i));
    }
    Container::new(bttn)
        .width(Length::Fill)
        .style(card::SimpleCardStyle)
        .into()
}

pub fn backup_mnemonic<'a>(
    progress: (usize, usize),
    words: &'a [&'static str; 12],
    done: bool,
) -> Element<'a, Message> {
    layout(
        progress,
        Column::new()
            .push(text("Backup your mnemonic").bold().size(50))
            .push(text(prompt::MNEMONIC_HELP))
            .push(
                words
                    .iter()
                    .enumerate()
                    .fold(Column::new().spacing(5), |acc, (i, w)| {
                        acc.push(
                            Row::new()
                                .align_items(Alignment::End)
                                .push(
                                    Container::new(text(format!("#{}", i + 1)).small())
                                        .width(Length::Units(50)),
                                )
                                .push(text(*w).bold()),
                        )
                    }),
            )
            .push(Checkbox::new(
                "I have backed up my mnemonic",
                done,
                Message::UserActionDone,
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

mod threshsold_input {
    use crate::ui::{
        component::{button, text::*},
        icon,
    };
    use iced::alignment::{self, Alignment};
    use iced::widget::{Button, Column, Container};
    use iced::{Element, Length};
    use iced_lazy::{self, Component};

    pub struct ThresholdInput<Message> {
        value: usize,
        max: usize,
        on_change: Box<dyn Fn(usize) -> Message>,
    }

    pub fn threshsold_input<Message>(
        value: usize,
        max: usize,
        on_change: impl Fn(usize) -> Message + 'static,
    ) -> ThresholdInput<Message> {
        ThresholdInput::new(value, max, on_change)
    }

    #[derive(Debug, Clone)]
    pub enum Event {
        IncrementPressed,
        DecrementPressed,
    }

    impl<Message> ThresholdInput<Message> {
        pub fn new(
            value: usize,
            max: usize,
            on_change: impl Fn(usize) -> Message + 'static,
        ) -> Self {
            Self {
                value,
                max,
                on_change: Box::new(on_change),
            }
        }
    }

    impl<Message> Component<Message, iced::Renderer> for ThresholdInput<Message> {
        type State = ();
        type Event = Event;

        fn update(&mut self, _state: &mut Self::State, event: Event) -> Option<Message> {
            match event {
                Event::IncrementPressed => {
                    if self.value < self.max {
                        Some((self.on_change)(self.value.saturating_add(1)))
                    } else {
                        None
                    }
                }
                Event::DecrementPressed => {
                    if self.value > 1 {
                        Some((self.on_change)(self.value.saturating_sub(1)))
                    } else {
                        None
                    }
                }
            }
        }

        fn view(&self, _state: &Self::State) -> Element<Self::Event> {
            let button = |label, on_press| {
                Button::new(label)
                    .style(button::Style::Transparent.into())
                    .width(Length::Units(50))
                    .on_press(on_press)
            };

            Column::new()
                .height(Length::Units(200))
                .width(Length::Units(100))
                .push(button(icon::up_icon().size(40), Event::IncrementPressed))
                .push(text("Threshold:").small().bold())
                .push(
                    Container::new(text(format!("{}/{}", self.value, self.max)).size(50))
                        .height(Length::Fill)
                        .align_y(alignment::Vertical::Center),
                )
                .push(button(icon::down_icon().size(40), Event::DecrementPressed))
                .align_items(Alignment::Center)
                .into()
        }
    }

    impl<'a, Message> From<ThresholdInput<Message>> for Element<'a, Message>
    where
        Message: 'a,
    {
        fn from(numeric_input: ThresholdInput<Message>) -> Self {
            iced_lazy::component(numeric_input)
        }
    }
}
