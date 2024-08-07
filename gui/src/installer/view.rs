use async_hwi::utils::extract_keys_and_template;
use iced::widget::{
    checkbox, container, pick_list, radio, scrollable, scrollable::Properties, slider, Button,
    Space, TextInput,
};
use iced::{alignment, widget::progress_bar, Alignment, Length};

use async_hwi::DeviceKind;
use liana_ui::component::text;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::{collections::HashSet, str::FromStr};

use liana::miniscript::bitcoin::{self, bip32::Fingerprint};
use liana_ui::{
    color,
    component::{
        button, card, collapse, form, hw, separation,
        text::{h3, h4_bold, h5_regular, p1_regular, text, Text},
        tooltip,
    },
    icon, image, theme,
    widget::*,
};

use crate::{
    bitcoind::{ConfigField, RpcAuthType, RpcAuthValues, StartInternalBitcoindError},
    hw::{is_compatible_with_tapminiscript, HardwareWallet, UnsupportedReason},
    installer::{
        message::{self, Message},
        prompt,
        step::{DownloadState, InstallState},
        Error,
    },
};

pub fn welcome<'a>() -> Element<'a, Message> {
    Container::new(
        Column::new()
            .push(
                Container::new(
                    Row::new()
                        .align_items(Alignment::Center)
                        .push(
                            Container::new(image::liana_brand_grey().width(Length::Fixed(200.0)))
                                .width(Length::Fill),
                        )
                        .push(
                            Row::new()
                                .push(
                                    button::secondary(
                                        Some(icon::previous_icon()),
                                        "Change network",
                                    )
                                    .width(Length::Fixed(200.0))
                                    .on_press(Message::BackToLauncher),
                                )
                                .push(
                                    button::secondary(None, "Share Xpubs")
                                        .width(Length::Fixed(200.0))
                                        .on_press(Message::ShareXpubs),
                                )
                                .spacing(20),
                        ),
                )
                .padding(100),
            )
            .push(
                Container::new(
                    Column::new()
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
                                                    .on_press(Message::CreateWallet),
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
                                                    .on_press(Message::ImportWallet),
                                            )
                                            .align_items(Alignment::Center),
                                    )
                                    .padding(20),
                                ),
                        )
                        .push(Space::with_height(Length::Fixed(100.0)))
                        .spacing(50)
                        .align_items(Alignment::Center),
                )
                .center_y()
                .center_x()
                .width(Length::Fill)
                .height(Length::Fill),
            ),
    )
    .into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorKind {
    P2WSH,
    Taproot,
}

const DESCRIPTOR_KINDS: [DescriptorKind; 2] = [DescriptorKind::P2WSH, DescriptorKind::Taproot];

impl std::fmt::Display for DescriptorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::P2WSH => write!(f, "P2WSH"),
            Self::Taproot => write!(f, "Taproot"),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn define_descriptor_advanced_settings<'a>(use_taproot: bool) -> Element<'a, Message> {
    let col_wallet = Column::new()
        .spacing(10)
        .push(text("Descriptor type").bold())
        .push(container(
            pick_list(
                &DESCRIPTOR_KINDS[..],
                Some(if use_taproot {
                    DescriptorKind::Taproot
                } else {
                    DescriptorKind::P2WSH
                }),
                |kind| Message::CreateTaprootDescriptor(kind == DescriptorKind::Taproot),
            )
            .style(theme::PickList::Secondary)
            .padding(10),
        ));

    container(
        Column::new()
            .spacing(20)
            .push(Space::with_height(0))
            .push(separation().width(500))
            .push(Row::new().push(col_wallet))
            .push_maybe(if use_taproot {
                Some(
                    p1_regular("Taproot is only supported by Liana version 5.0 and above")
                        .style(color::GREY_2),
                )
            } else {
                None
            }),
    )
    .into()
}

#[allow(clippy::too_many_arguments)]
pub fn define_descriptor<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    use_taproot: bool,
    spending_keys: Vec<Element<'a, Message>>,
    spending_threshold: usize,
    recovery_paths: Vec<Element<'a, Message>>,
    valid: bool,
    error: Option<&String>,
) -> Element<'a, Message> {
    let col_spending_keys = Column::new()
        .push(
            Row::new()
                .spacing(10)
                .push(text("Primary path:").bold())
                .push(tooltip(prompt::DEFINE_DESCRIPTOR_PRIMARY_PATH_TOOLTIP)),
        )
        .push(Container::new(
            Row::new()
                .align_items(Alignment::Center)
                .push_maybe(if spending_keys.len() > 1 {
                    Some(threshsold_input::threshsold_input(
                        spending_threshold,
                        spending_keys.len(),
                        |value| {
                            Message::DefineDescriptor(message::DefineDescriptor::PrimaryPath(
                                message::DefinePath::ThresholdEdited(value),
                            ))
                        },
                    ))
                } else {
                    None
                })
                .push(
                    scrollable(
                        Row::new()
                            .spacing(5)
                            .align_items(Alignment::Center)
                            .push(Row::with_children(spending_keys).spacing(5))
                            .push(
                                Button::new(
                                    Container::new(icon::plus_icon().size(50))
                                        .width(Length::Fixed(150.0))
                                        .height(Length::Fixed(150.0))
                                        .align_y(alignment::Vertical::Center)
                                        .align_x(alignment::Horizontal::Center),
                                )
                                .width(Length::Fixed(150.0))
                                .height(Length::Fixed(150.0))
                                .style(theme::Button::TransparentBorder)
                                .on_press(
                                    Message::DefineDescriptor(
                                        message::DefineDescriptor::PrimaryPath(
                                            message::DefinePath::AddKey,
                                        ),
                                    ),
                                ),
                            )
                            .padding(5),
                    )
                    .direction(scrollable::Direction::Horizontal(
                        Properties::new().width(3).scroller_width(3),
                    )),
                ),
        ))
        .spacing(10);

    layout(
        progress,
        email,
        "Create the wallet",
        Column::new()
            .push(collapse::Collapse::new(
                || {
                    Button::new(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text("Advanced settings").small().bold())
                            .push(icon::collapse_icon()),
                    )
                    .style(theme::Button::Transparent)
                },
                || {
                    Button::new(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text("Advanced settings").small().bold())
                            .push(icon::collapsed_icon()),
                    )
                    .style(theme::Button::Transparent)
                },
                move || define_descriptor_advanced_settings(use_taproot),
            ))
            .push(
                Column::new()
                    .width(Length::Fill)
                    .push(
                        Column::new()
                            .spacing(25)
                            .push(col_spending_keys)
                            .push(
                                Row::new()
                                    .spacing(10)
                                    .push(text("Recovery paths:").bold())
                                    .push(tooltip(prompt::DEFINE_DESCRIPTOR_RECOVERY_PATH_TOOLTIP)),
                            )
                            .push(Column::with_children(recovery_paths).spacing(10)),
                    )
                    .spacing(25),
            )
            .push(
                Row::new()
                    .spacing(10)
                    .push(
                        button::secondary(Some(icon::plus_icon()), "Add a recovery path")
                            .on_press(Message::DefineDescriptor(
                                message::DefineDescriptor::AddRecoveryPath,
                            ))
                            .width(Length::Fixed(200.0)),
                    )
                    .push(if !valid {
                        button::primary(None, "Next").width(Length::Fixed(200.0))
                    } else {
                        button::primary(None, "Next")
                            .width(Length::Fixed(200.0))
                            .on_press(Message::Next)
                    }),
            )
            .push_maybe(error.map(|e| card::error("Failed to create descriptor", e.to_string())))
            .push(Space::with_height(Length::Fixed(20.0)))
            .spacing(50),
        false,
        Some(Message::Previous),
    )
}

pub fn recovery_path_view(
    sequence: u16,
    duplicate_sequence: bool,
    recovery_threshold: usize,
    recovery_keys: Vec<Element<message::DefinePath>>,
) -> Element<message::DefinePath> {
    Container::new(
        Column::new()
            .push(defined_sequence(sequence, duplicate_sequence))
            .push(
                Row::new()
                    .align_items(Alignment::Center)
                    .push_maybe(if recovery_keys.len() > 1 {
                        Some(threshsold_input::threshsold_input(
                            recovery_threshold,
                            recovery_keys.len(),
                            message::DefinePath::ThresholdEdited,
                        ))
                    } else {
                        None
                    })
                    .push(
                        scrollable(
                            Row::new()
                                .spacing(5)
                                .align_items(Alignment::Center)
                                .push(Row::with_children(recovery_keys).spacing(5))
                                .push(
                                    Button::new(
                                        Container::new(icon::plus_icon().size(50))
                                            .width(Length::Fixed(150.0))
                                            .height(Length::Fixed(150.0))
                                            .align_y(alignment::Vertical::Center)
                                            .align_x(alignment::Horizontal::Center),
                                    )
                                    .width(Length::Fixed(150.0))
                                    .height(Length::Fixed(150.0))
                                    .style(theme::Button::TransparentBorder)
                                    .on_press(message::DefinePath::AddKey),
                                )
                                .padding(5),
                        )
                        .direction(scrollable::Direction::Horizontal(
                            Properties::new().width(3).scroller_width(3),
                        )),
                    ),
            ),
    )
    .padding(5)
    .style(theme::Container::Card(theme::Card::Border))
    .into()
}

pub fn import_wallet_or_descriptor<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    invitation: &'a form::Value<String>,
    invitation_wallet: Option<&'a str>,
    imported_descriptor: &'a form::Value<String>,
    error: Option<&'a String>,
    wallets: Vec<&'a String>,
) -> Element<'a, Message> {
    let mut col_wallets = Column::new()
        .spacing(20)
        .push(h4_bold("Choose the wallet to import"));
    let no_wallets = wallets.is_empty();
    for (i, wallet) in wallets.into_iter().enumerate() {
        col_wallets = col_wallets.push(
            Button::new(h5_regular(wallet).width(Length::Fill))
                .padding(10)
                .on_press(Message::Select(i)),
        );
    }
    let card_wallets: Element<'a, Message> = if no_wallets {
        h4_bold("You have no current wallets").into()
    } else {
        card::simple(col_wallets).into()
    };

    let col_invitation_token = collapse::Collapse::new(
        || {
            Button::new(
                Column::new()
                    .spacing(5)
                    .push(h4_bold("Join a shared wallet").style(color::WHITE))
                    .push(
                        text("If you received an invitation to join a shared wallet")
                            .style(color::GREY_3),
                    ),
            )
            .padding(15)
            .width(Length::Fill)
            .style(theme::Button::TransparentBorder)
        },
        || {
            Button::new(
                Column::new()
                    .spacing(5)
                    .push(h4_bold("Join a shared wallet").style(color::WHITE))
                    .push(
                        text("If you received an invitation to join a shared wallet")
                            .style(color::GREY_3),
                    ),
            )
            .padding(15)
            .width(Length::Fill)
            .style(theme::Button::TransparentBorder)
        },
        move || {
            if let Some(wallet) = invitation_wallet {
                Element::<'a, Message>::from(
                    Column::new()
                        .push(Space::with_height(0))
                        .push(
                            Row::new()
                                .spacing(5)
                                .push(text("Accept invitation for wallet:"))
                                .push(text(wallet).bold()),
                        )
                        .push(
                            Row::new().push(Space::with_width(Length::Fill)).push(
                                button::primary(None, "Accept")
                                    .width(Length::Fixed(200.0))
                                    .on_press(Message::ImportRemoteWallet(
                                        message::ImportRemoteWallet::AcceptInvitation,
                                    )),
                            ),
                        )
                        .spacing(20),
                )
            } else {
                Element::<'a, Message>::from(
                    Container::new(
                        Column::new()
                            .push(Space::with_height(0))
                            .push(
                                Column::new()
                                    .push(text("Paste invitation:").bold())
                                    .push(
                                        form::Form::new_trimmed("Invitation", invitation, |msg| {
                                            Message::ImportRemoteWallet(
                                                message::ImportRemoteWallet::ImportInvitationToken(
                                                    msg,
                                                ),
                                            )
                                        })
                                        .warning("Invitation token is invalid or expired")
                                        .size(text::P1_SIZE)
                                        .padding(10),
                                    )
                                    .spacing(10),
                            )
                            .push(
                                Row::new().push(Space::with_width(Length::Fill)).push(
                                    button::primary(None, "Next")
                                        .width(Length::Fixed(200.0))
                                        .on_press_maybe(if !invitation.value.is_empty() {
                                            Some(Message::ImportRemoteWallet(
                                                message::ImportRemoteWallet::FetchInvitation,
                                            ))
                                        } else {
                                            None
                                        }),
                                ),
                            )
                            .spacing(20),
                    )
                    .padding(15),
                )
            }
        },
    );

    let col_descriptor = collapse::Collapse::new(
        || {
            Button::new(
                Column::new()
                    .spacing(5)
                    .push(h4_bold("Import a wallet from descriptor").style(color::WHITE))
                    .push(
                        text("The remote backend will rescan the blockchain to find your coins")
                            .style(color::GREY_3),
                    ),
            )
            .padding(15)
            .width(Length::Fill)
            .style(theme::Button::TransparentBorder)
        },
        || {
            Button::new(
                Column::new()
                    .spacing(5)
                    .push(h4_bold("Import a wallet from descriptor").style(color::WHITE))
                    .push(
                        text("The remote backend will rescan the blockchain to find your coins")
                            .style(color::GREY_3),
                    ),
            )
            .padding(15)
            .width(Length::Fill)
            .style(theme::Button::TransparentBorder)
        },
        move || {
            Element::<'a, Message>::from(
                Container::new(
                    Column::new()
                        .push(Space::with_height(0))
                        .push(
                            Column::new()
                                .push(text("Descriptor:").bold())
                                .push(
                                    form::Form::new_trimmed(
                                        "Descriptor",
                                        imported_descriptor,
                                        |msg| {
                                            Message::ImportRemoteWallet(
                                                message::ImportRemoteWallet::ImportDescriptor(msg),
                                            )
                                        },
                                    )
                                    .warning(
                                        "Either descriptor is invalid or incompatible with network",
                                    )
                                    .size(text::P1_SIZE)
                                    .padding(10),
                                )
                                .spacing(10),
                        )
                        .push(
                            Row::new().push(Space::with_width(Length::Fill)).push(
                                button::primary(None, "Next")
                                    .width(Length::Fixed(200.0))
                                    .on_press_maybe(
                                        if imported_descriptor.value.is_empty()
                                            || !imported_descriptor.valid
                                        {
                                            None
                                        } else {
                                            Some(Message::ImportRemoteWallet(
                                                message::ImportRemoteWallet::ConfirmDescriptor,
                                            ))
                                        },
                                    ),
                            ),
                        )
                        .spacing(20),
                )
                .padding(15),
            )
        },
    );

    layout(
        progress,
        email,
        "Add wallet",
        Column::new()
            .spacing(50)
            .push_maybe(error.map(|e| card::error("Something wrong happened", e.to_string())))
            .push(card_wallets)
            .push(card::simple(col_invitation_token).padding(0))
            .push(card::simple(col_descriptor).padding(0)),
        true,
        Some(Message::Previous),
    )
}

pub fn import_descriptor<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    imported_descriptor: &form::Value<String>,
    wrong_network: bool,
    error: Option<&String>,
) -> Element<'a, Message> {
    let col_descriptor = Column::new()
        .push(text("Descriptor:").bold())
        .push(
            form::Form::new_trimmed("Descriptor", imported_descriptor, |msg| {
                Message::DefineDescriptor(message::DefineDescriptor::ImportDescriptor(msg))
            })
            .warning(if wrong_network {
                "The descriptor is for another network"
            } else {
                "Failed to read the descriptor"
            })
            .size(text::P1_SIZE)
            .padding(10),
        )
        .spacing(10);
    layout(
        progress,
        email,
        "Import the wallet",
        Column::new()
            .push(Column::new().spacing(20).push(col_descriptor).push(text(
                "After creating the wallet, \
                            you will need to perform a rescan of \
                            the blockchain in order to see your \
                            coins and past transactions. This can \
                            be done in Settings > Bitcoin Core.",
            )))
            .push(
                if imported_descriptor.value.is_empty() || !imported_descriptor.valid {
                    button::primary(None, "Next").width(Length::Fixed(200.0))
                } else {
                    button::primary(None, "Next")
                        .width(Length::Fixed(200.0))
                        .on_press(Message::Next)
                },
            )
            .push_maybe(error.map(|e| card::error("Invalid descriptor", e.to_string())))
            .spacing(50),
        true,
        Some(Message::Previous),
    )
}

const BACKUP_WARNING: &str =
    "Beware to back up the mnemonic as it will NOT be stored on the computer.";

pub fn signer_xpubs<'a>(
    xpubs: &'a [String],
    words: &'a [&'static str; 12],
    did_backup: bool,
) -> Element<'a, Message> {
    Container::new(
        Column::new()
            .push(
                Button::new(
                    Row::new().align_items(Alignment::Center).push(
                        Column::new()
                            .push(text("Generate a new mnemonic").bold())
                            .push(text(BACKUP_WARNING).small().style(color::ORANGE))
                            .spacing(5)
                            .width(Length::Fill),
                    ),
                )
                .on_press(Message::UseHotSigner)
                .padding(10)
                .style(theme::Button::TransparentBorder)
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
                Some(
                    Container::new(words.iter().enumerate().fold(
                        Column::new().spacing(5),
                        |acc, (i, w)| {
                            acc.push(
                                Row::new()
                                    .align_items(Alignment::End)
                                    .push(
                                        Container::new(text(format!("#{}", i + 1)).small())
                                            .width(Length::Fixed(50.0)),
                                    )
                                    .push(text(*w).bold()),
                            )
                        },
                    ))
                    .padding(15),
                )
            })
            .push_maybe(if !xpubs.is_empty() {
                Some(
                    Container::new(
                        checkbox(
                            "I have backed up the mnemonic, show the extended public key",
                            did_backup,
                        )
                        .on_toggle(Message::UserActionDone),
                    )
                    .padding(10),
                )
            } else {
                None
            })
            .push_maybe(if !xpubs.is_empty() && did_backup {
                Some(xpubs.iter().fold(Column::new().padding(15), |col, xpub| {
                    col.push(
                        Row::new()
                            .spacing(5)
                            .align_items(Alignment::Center)
                            .push(
                                Container::new(
                                    scrollable(Container::new(text(xpub).small()).padding(10))
                                        .direction(scrollable::Direction::Horizontal(
                                            Properties::new().width(5).scroller_width(5),
                                        )),
                                )
                                .width(Length::Fill),
                            )
                            .push(
                                Container::new(
                                    button::secondary(Some(icon::clipboard_icon()), "Copy")
                                        .on_press(Message::Clibpboard(xpub.clone()))
                                        .width(Length::Shrink),
                                )
                                .padding(10),
                            ),
                    )
                }))
            } else {
                None
            }),
    )
    .style(theme::Container::Card(theme::Card::Simple))
    .into()
}

pub fn hardware_wallet_xpubs<'a>(
    i: usize,
    hw: &'a HardwareWallet,
    xpubs: Option<&'a Vec<String>>,
    processing: bool,
    error: Option<&Error>,
) -> Element<'a, Message> {
    let mut bttn = Button::new(match hw {
        HardwareWallet::Supported {
            kind,
            version,
            fingerprint,
            alias,
            ..
        } => {
            if processing {
                hw::processing_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            } else {
                hw::supported_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            }
        }
        HardwareWallet::Unsupported {
            version,
            kind,
            reason,
            ..
        } => match reason {
            UnsupportedReason::NotPartOfWallet(fg) => {
                hw::unrelated_hardware_wallet(&kind.to_string(), version.as_ref(), fg)
            }
            UnsupportedReason::WrongNetwork => {
                hw::wrong_network_hardware_wallet(&kind.to_string(), version.as_ref())
            }
            _ => hw::unsupported_hardware_wallet(&kind.to_string(), version.as_ref()),
        },
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => hw::locked_hardware_wallet(kind, pairing_code.as_ref()),
    })
    .style(theme::Button::Secondary)
    .width(Length::Fill);
    if !processing && hw.is_supported() {
        bttn = bttn.on_press(Message::Select(i));
    }
    Container::new(
        Column::new()
            .push_maybe(error.map(|e| card::warning(e.to_string()).width(Length::Fill)))
            .push(bttn)
            .push_maybe(if xpubs.is_none() {
                None
            } else {
                Some(separation().width(Length::Fill))
            })
            .push_maybe(xpubs.map(|xpubs| {
                xpubs.iter().fold(Column::new().padding(15), |col, xpub| {
                    col.push(
                        Row::new()
                            .spacing(5)
                            .align_items(Alignment::Center)
                            .push(
                                Container::new(
                                    scrollable(Container::new(text(xpub).small()).padding(10))
                                        .direction(scrollable::Direction::Horizontal(
                                            Properties::new().width(5).scroller_width(5),
                                        )),
                                )
                                .width(Length::Fill),
                            )
                            .push(
                                Container::new(
                                    button::secondary(Some(icon::clipboard_icon()), "Copy")
                                        .on_press(Message::Clibpboard(xpub.clone()))
                                        .width(Length::Shrink),
                                )
                                .padding(10),
                            ),
                    )
                })
            })),
    )
    .style(theme::Container::Card(theme::Card::Simple))
    .into()
}

pub fn share_xpubs<'a>(
    email: Option<&'a str>,
    hws: Vec<Element<'a, Message>>,
    signer: Element<'a, Message>,
) -> Element<'a, Message> {
    layout(
        (0, 0),
        email,
        "Share your public keys (Xpubs)",
        Column::new()
            .push(
                Container::new(
                    text("Import an extended public key by selecting a signing device:").bold(),
                )
                .width(Length::Fill),
            )
            .push_maybe(if hws.is_empty() {
                Some(p1_regular("No signing device connected").style(color::GREY_3))
            } else {
                None
            })
            .spacing(10)
            .push(Column::with_children(hws).spacing(10))
            .push(Container::new(text("Or create a new random key:").bold()).width(Length::Fill))
            .push(signer)
            .push(Space::with_height(10))
            .width(Length::Fill),
        true,
        Some(Message::Previous),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn register_descriptor<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    descriptor: String,
    hws: &'a [HardwareWallet],
    registered: &HashSet<bitcoin::bip32::Fingerprint>,
    error: Option<&Error>,
    processing: bool,
    chosen_hw: Option<usize>,
    done: bool,
    created_desc: bool,
) -> Element<'a, Message> {
    let displayed_descriptor = if let Ok((template, keys)) =
        extract_keys_and_template::<String>(&descriptor)
    {
        let mut col = Column::new()
            .push(
                card::simple(
                    Column::new()
                        .push(text("Descriptor template:").small().bold())
                        .push(
                            scrollable(
                                Column::new()
                                    .push(text(template).small())
                                    .push(Space::with_height(Length::Fixed(5.0))),
                            )
                            .direction(
                                scrollable::Direction::Horizontal(
                                    scrollable::Properties::new().width(5).scroller_width(5),
                                ),
                            ),
                        )
                        .spacing(10),
                )
                .width(Length::Fill),
            )
            .push(Space::with_height(5));

        for (index, key) in keys.into_iter().enumerate() {
            col = col
                .push(
                    card::simple(
                        Column::new()
                            .push(text(format!("Key @{}:", index)).small().bold())
                            .push(
                                scrollable(
                                    Column::new()
                                        .push(text(key.to_owned()).small())
                                        .push(Space::with_height(Length::Fixed(5.0))),
                                )
                                .direction(
                                    scrollable::Direction::Horizontal(
                                        scrollable::Properties::new().width(5).scroller_width(5),
                                    ),
                                ),
                            )
                            .spacing(10),
                    )
                    .width(Length::Fill),
                )
                .push(Space::with_height(5));
        }

        col
    } else {
        Column::new().push(card::simple(
            Column::new()
                .push(text("The descriptor:").small().bold())
                .push(
                    scrollable(
                        Column::new()
                            .push(text(descriptor.to_owned()).small())
                            .push(Space::with_height(Length::Fixed(5.0))),
                    )
                    .direction(scrollable::Direction::Horizontal(
                        scrollable::Properties::new().width(5).scroller_width(5),
                    )),
                )
                .push(
                    Row::new().push(Column::new().width(Length::Fill)).push(
                        button::secondary(Some(icon::clipboard_icon()), "Copy")
                            .on_press(Message::Clibpboard(descriptor)),
                    ),
                )
                .spacing(10),
        ))
    };
    layout(
        progress,
        email,
        "Register descriptor",
        Column::new()
            .push_maybe((!created_desc).then_some(
                text("This step is only necessary if you are using a signing device.").bold(),
            ))
            .push(displayed_descriptor)
            .push(text(prompt::REGISTER_DESCRIPTOR_HELP))
            .push_maybe(error.map(|e| card::error("Failed to register descriptor", e.to_string())))
            .push(
                Column::new()
                    .push(
                        Container::new(
                            if created_desc {
                                text("Select hardware wallet to register descriptor on:")
                                    .bold()
                            } else {
                                text("If necessary, please select the signing device to register descriptor on:")
                                    .bold()
                            },
                        )
                        .width(Length::Fill),
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
                                    false,
                                ))
                            }),
                    )
                    .width(Length::Fill),
            )
            .push_maybe(created_desc.then_some(checkbox(
                "I have registered the descriptor on my device(s)",
                done,
            ).on_toggle(Message::UserActionDone)))
            .push(if !created_desc || (done && !processing) {
                button::primary(None, "Next")
                    .on_press(Message::Next)
                    .width(Length::Fixed(200.0))
            } else {
                button::primary(None, "Next").width(Length::Fixed(200.0))
            })
            .spacing(50),
        true,
        if !processing {
        Some(Message::Previous)
        } else {
            None
        }
    )
}

pub fn backup_descriptor(
    progress: (usize, usize),
    email: Option<&str>,
    descriptor: String,
    done: bool,
) -> Element<'_, Message> {
    layout(
        progress,
        email,
        "Backup your wallet descriptor",
        Column::new()
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
                            .style(theme::Button::Transparent)
                        },
                        || {
                            Button::new(
                                Row::new()
                                    .align_items(Alignment::Center)
                                    .spacing(10)
                                    .push(text("Learn more").small().bold())
                                    .push(icon::collapsed_icon()),
                            )
                            .style(theme::Button::Transparent)
                        },
                        help_backup,
                    ))
                    .max_width(1000),
            )
            .push(card::simple(
                Column::new()
                    .push(text("The descriptor:").small().bold())
                    .push(
                        scrollable(
                            Column::new()
                                .push(text(descriptor.to_owned()).small())
                                .push(Space::with_height(Length::Fixed(5.0))),
                        )
                        .direction(scrollable::Direction::Horizontal(
                            scrollable::Properties::new().width(5).scroller_width(5),
                        )),
                    )
                    .push(
                        Row::new().push(Column::new().width(Length::Fill)).push(
                            button::secondary(Some(icon::clipboard_icon()), "Copy")
                                .on_press(Message::Clibpboard(descriptor)),
                        ),
                    )
                    .spacing(10)
                    .max_width(1000),
            ))
            .push(
                checkbox("I have backed up my descriptor", done).on_toggle(Message::UserActionDone),
            )
            .push(if done {
                button::primary(None, "Next")
                    .on_press(Message::Next)
                    .width(Length::Fixed(200.0))
            } else {
                button::primary(None, "Next").width(Length::Fixed(200.0))
            })
            .spacing(50),
        true,
        Some(Message::Previous),
    )
}

pub fn help_backup<'a>() -> Element<'a, Message> {
    text(prompt::BACKUP_DESCRIPTOR_HELP).small().into()
}

pub fn define_bitcoin<'a>(
    progress: (usize, usize),
    address: &form::Value<String>,
    rpc_auth_vals: &RpcAuthValues,
    selected_auth_type: &RpcAuthType,
    is_running: Option<&Result<(), Error>>,
) -> Element<'a, Message> {
    let is_loopback = if let Some((ip, _port)) = address.value.clone().rsplit_once(':') {
        let (ipv4, ipv6) = (Ipv4Addr::from_str(ip), Ipv6Addr::from_str(ip));
        match (ipv4, ipv6) {
            (_, Ok(ip)) => ip.is_loopback(),
            (Ok(ip), _) => ip.is_loopback(),
            _ => false,
        }
    } else {
        false
    };

    let col_address = Column::new()
        .push(text("Address:").bold())
        .push(
            form::Form::new_trimmed("Address", address, |msg| {
                Message::DefineBitcoind(message::DefineBitcoind::ConfigFieldEdited(
                    ConfigField::Address,
                    msg,
                ))
            })
            .warning("Please enter correct address")
            .size(text::P1_SIZE)
            .padding(10),
        )
        .push_maybe(if !is_loopback && address.valid {
            Some(
                iced::widget::Text::new(
                    "Connection to a remote Bitcoin node \
                    is not supported. Insert an IP address bound to the same machine \
                    running Liana (ignore this warning if that's already the case)",
                )
                .style(color::ORANGE)
                .size(text::CAPTION_SIZE),
            )
        } else {
            None
        })
        .spacing(10);

    let col_auth = Column::new()
        .push(
            [RpcAuthType::CookieFile, RpcAuthType::UserPass]
                .iter()
                .fold(
                    Row::new()
                        .push(text("RPC authentication:").small().bold())
                        .spacing(10),
                    |row, auth_type| {
                        row.push(radio(
                            format!("{}", auth_type),
                            *auth_type,
                            Some(*selected_auth_type),
                            |new_selection| {
                                Message::DefineBitcoind(
                                    message::DefineBitcoind::RpcAuthTypeSelected(new_selection),
                                )
                            },
                        ))
                        .spacing(30)
                        .align_items(Alignment::Center)
                    },
                ),
        )
        .push(match selected_auth_type {
            RpcAuthType::CookieFile => Row::new().push(
                form::Form::new_trimmed("Cookie path", &rpc_auth_vals.cookie_path, |msg| {
                    Message::DefineBitcoind(message::DefineBitcoind::ConfigFieldEdited(
                        ConfigField::CookieFilePath,
                        msg,
                    ))
                })
                .warning("Please enter correct path")
                .size(text::P1_SIZE)
                .padding(10),
            ),
            RpcAuthType::UserPass => Row::new()
                .push(
                    form::Form::new_trimmed("User", &rpc_auth_vals.user, |msg| {
                        Message::DefineBitcoind(message::DefineBitcoind::ConfigFieldEdited(
                            ConfigField::User,
                            msg,
                        ))
                    })
                    .warning("Please enter correct user")
                    .size(text::P1_SIZE)
                    .padding(10),
                )
                .push(
                    form::Form::new_trimmed("Password", &rpc_auth_vals.password, |msg| {
                        Message::DefineBitcoind(message::DefineBitcoind::ConfigFieldEdited(
                            ConfigField::Password,
                            msg,
                        ))
                    })
                    .warning("Please enter correct password")
                    .size(text::P1_SIZE)
                    .padding(10),
                )
                .spacing(10),
        })
        .spacing(10);

    let check_connect_enable = if let RpcAuthType::UserPass = selected_auth_type {
        address.valid
            && !rpc_auth_vals.password.value.is_empty()
            && !rpc_auth_vals.user.value.is_empty()
    } else {
        address.valid && !rpc_auth_vals.cookie_path.value.is_empty()
    };
    layout(
        progress,
        None,
        "Set up connection to the Bitcoin full node",
        Column::new()
            .push(col_address)
            .push(col_auth)
            .push_maybe(if is_running.is_some() {
                is_running.map(|res| {
                    if res.is_ok() {
                        Container::new(
                            Row::new()
                                .spacing(10)
                                .align_items(Alignment::Center)
                                .push(icon::circle_check_icon().style(color::GREEN))
                                .push(text("Connection checked").style(color::GREEN)),
                        )
                    } else {
                        Container::new(
                            Row::new()
                                .spacing(10)
                                .align_items(Alignment::Center)
                                .push(icon::circle_cross_icon().style(color::RED))
                                .push(text("Connection failed").style(color::RED)),
                        )
                    }
                })
            } else {
                Some(Container::new(Space::with_height(Length::Fixed(25.0))))
            })
            .push(
                Row::new()
                    .spacing(10)
                    .push(Container::new(
                        button::secondary(None, "Check connection")
                            .on_press_maybe(if check_connect_enable {
                                Some(Message::DefineBitcoind(
                                    message::DefineBitcoind::PingBitcoind,
                                ))
                            } else {
                                None
                            })
                            .width(Length::Fixed(200.0)),
                    ))
                    .push(if is_running.map(|res| res.is_ok()).unwrap_or(false) {
                        button::primary(None, "Next")
                            .on_press(Message::Next)
                            .width(Length::Fixed(200.0))
                    } else {
                        button::primary(None, "Next").width(Length::Fixed(200.0))
                    }),
            )
            .spacing(50),
        true,
        Some(Message::Previous),
    )
}

pub fn select_bitcoind_type<'a>(progress: (usize, usize)) -> Element<'a, Message> {
    layout(
        progress,
        None,
        "Bitcoin node management",
        Column::new().push(
            Row::new()
                .align_items(Alignment::Start)
                .spacing(20)
                .push(
                    Container::new(
                        Column::new()
                            .spacing(20)
                            .width(Length::Fixed(300.0))
                            .push(text("Manage your own Bitcoin node").bold())
                    )
                    .padding(20),
                )
                .push(
                    Container::new(
                        Column::new()
                            .spacing(20)
                            .width(Length::Fixed(300.0))
                            .push(text("Have Liana manage and run a dedicated Bitcoin node").bold())
                    )
                    .padding(20),
                ),
        )
        .push(
            Row::new()
                .align_items(Alignment::Start)
                .spacing(20)
                .push(
                    Container::new(
                        Column::new()
                            .spacing(20)
                            .width(Length::Fixed(300.0))
                            .align_items(Alignment::Start)
                            .push(text("Liana will connect to your existing instance of Bitcoin Core. You will have to make sure Bitcoin Core is running when you use Liana.\n\n(Use this if you already have a full node on your machine, and don't need a new instance)"))
                    )
                    .padding(20),
                )
                .push(
                    Container::new(
                        Column::new()
                            .spacing(20)
                            .width(Length::Fixed(300.0))
                            .align_items(Alignment::Start)
                            .push(text("Liana will run its own instance of Bitcoin Core. This will use a pruned node, and perform the synchronization in the Liana folder.\n\nIf you select this option, Bitcoin Core will be downloaded, installed and started on the next step.\n\n(Use this if you don't want to deal with Bitcoin Core yourself, or need a new, dedicated instance for Liana)"))
                    )
                    .padding(20),
                ),
        )
        .push(
            Row::new()
                .align_items(Alignment::End)
                .spacing(20)
                .push(
                    Container::new(
                        Column::new()
                            .spacing(20)
                            .width(Length::Fixed(300.0))
                            .align_items(Alignment::Center)
                            .push(
                                button::primary(None, "Select")
                                .width(Length::Fixed(300.0))
                                    .on_press(Message::SelectBitcoindType(
                                        message::SelectBitcoindTypeMsg::UseExternal(true),
                                    )),
                            )
                    )
                    .padding(20),
                )
                .push(
                    Container::new(
                        Column::new()
                            .spacing(20)
                            .width(Length::Fixed(300.0))
                            .align_items(Alignment::Center)
                            .push(
                                button::primary(None, "Select")
                                    .width(Length::Fixed(300.0))
                                    .on_press(Message::SelectBitcoindType(
                                        message::SelectBitcoindTypeMsg::UseExternal(false),
                                    )),
                            )
                    )
                    .padding(20),
                ),
        ),
        true,
        Some(Message::Previous),
    )
}

pub fn start_internal_bitcoind<'a>(
    progress: (usize, usize),
    exe_path: Option<&PathBuf>,
    started: Option<&Result<(), StartInternalBitcoindError>>,
    error: Option<&'a String>,
    download_state: Option<&DownloadState>,
    install_state: Option<&InstallState>,
) -> Element<'a, Message> {
    let version = crate::bitcoind::VERSION;
    let mut next_button = button::primary(None, "Next").width(Length::Fixed(200.0));
    if let Some(Ok(_)) = started {
        next_button = next_button.on_press(Message::Next);
    };
    layout(
        progress,
        None,
        "Start Bitcoin full node",
        Column::new()
            .push_maybe(download_state.map(|s| {
                match s {
                    DownloadState::Finished(_) => Row::new()
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .push(icon::circle_check_icon().style(color::GREEN))
                        .push(text("Download complete").style(color::GREEN)),
                    DownloadState::Downloading { progress } => Row::new()
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .push(text(format!(
                            "Downloading Bitcoin Core {version}... {progress:.2}%"
                        ))),
                    DownloadState::Errored(e) => Row::new()
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .push(icon::circle_cross_icon().style(color::RED))
                        .push(text(format!("Download failed: '{}'.", e)).style(color::RED)),
                    _ => Row::new().spacing(10).align_items(Alignment::Center),
                }
            }))
            .push(Container::new(if let Some(state) = install_state {
                match state {
                    InstallState::InProgress => Row::new()
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .push("Installing bitcoind..."),
                    InstallState::Finished => Row::new()
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .push(icon::circle_check_icon().style(color::GREEN))
                        .push(text("Installation complete").style(color::GREEN)),
                    InstallState::Errored(e) => Row::new()
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .push(icon::circle_cross_icon().style(color::RED))
                        .push(text(format!("Installation failed: '{}'.", e)).style(color::RED)),
                }
            } else if exe_path.is_some() {
                Row::new()
                    .spacing(10)
                    .align_items(Alignment::Center)
                    .push(icon::circle_check_icon().style(color::GREEN))
                    .push(text("Liana-managed bitcoind already installed").style(color::GREEN))
            } else if let Some(DownloadState::Downloading { progress }) = download_state {
                Row::new()
                    .spacing(10)
                    .align_items(Alignment::Center)
                    .push(progress_bar(0.0..=100.0, *progress))
            } else {
                Row::new().spacing(10).align_items(Alignment::Center)
            }))
            .push_maybe(if started.is_some() {
                started.map(|res| {
                    if res.is_ok() {
                        Container::new(
                            Row::new()
                                .spacing(10)
                                .align_items(Alignment::Center)
                                .push(icon::circle_check_icon().style(color::GREEN))
                                .push(text("Started").style(color::GREEN)),
                        )
                    } else {
                        Container::new(
                            Row::new()
                                .spacing(10)
                                .align_items(Alignment::Center)
                                .push(icon::circle_cross_icon().style(color::RED))
                                .push(
                                    text(res.as_ref().err().unwrap().to_string()).style(color::RED),
                                ),
                        )
                    }
                })
            } else {
                match (install_state, exe_path) {
                    // We have either just installed bitcoind or it was already installed.
                    (Some(InstallState::Finished), _) | (None, Some(_)) => Some(Container::new(
                        Row::new()
                            .spacing(10)
                            .align_items(Alignment::Center)
                            .push(text("Starting...")),
                    )),
                    _ => Some(Container::new(Space::with_height(Length::Fixed(25.0)))),
                }
            })
            .spacing(50)
            .push(
                Row::new()
                    .spacing(10)
                    .push(Row::new().spacing(10).push(next_button)),
            )
            .push_maybe(error.map(|e| card::invalid(text(e)))),
        true,
        Some(message::Message::InternalBitcoind(
            message::InternalBitcoindMsg::Previous,
        )),
    )
}

pub fn install<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    generating: bool,
    config_path: Option<&std::path::PathBuf>,
    warning: Option<&'a String>,
) -> Element<'a, Message> {
    let prev_msg = if !generating && warning.is_some() {
        Some(Message::Previous)
    } else {
        None
    };
    layout(
        progress,
        email,
        "Finalize installation",
        Column::new()
            .push_maybe(warning.map(|e| card::invalid(text(e))))
            .push(if generating {
                Container::new(text("Installing..."))
            } else if config_path.is_some() {
                Container::new(
                    Row::new()
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .push(icon::circle_check_icon().style(color::GREEN))
                        .push(text("Installed").style(color::GREEN)),
                )
            } else {
                Container::new(Space::with_height(Length::Fixed(25.0)))
            })
            .spacing(10)
            .width(Length::Fill),
        true,
        prev_msg,
    )
}

pub fn defined_sequence<'a>(
    sequence: u16,
    duplicate_sequence: bool,
) -> Element<'a, message::DefinePath> {
    let (n_years, n_months, n_days, n_hours, n_minutes) = duration_from_sequence(sequence);
    Container::new(
        Column::new()
            .spacing(5)
            .push_maybe(if duplicate_sequence {
                Some(
                    text("No two recovery paths may become available at the very same date.")
                        .small()
                        .style(color::RED),
                )
            } else {
                None
            })
            .push(
                Row::new()
                    .align_items(Alignment::Center)
                    .push(
                        Container::new(
                            Column::new()
                                .spacing(5)
                                .push(text(format!("Available after {} blocks", sequence)).bold())
                                .push(
                                    [
                                        (n_years, "y"),
                                        (n_months, "m"),
                                        (n_days, "d"),
                                        (n_hours, "h"),
                                        (n_minutes, "mn"),
                                    ]
                                    .iter()
                                    .fold(
                                        Row::new().spacing(5),
                                        |row, (n, unit)| {
                                            row.push_maybe(if *n > 0 {
                                                Some(text(format!("{}{}", n, unit,)))
                                            } else {
                                                None
                                            })
                                        },
                                    ),
                                ),
                        )
                        .padding(5)
                        .align_y(alignment::Vertical::Center),
                    )
                    .push(
                        button::secondary(Some(icon::pencil_icon()), "Edit")
                            .on_press(message::DefinePath::EditSequence),
                    )
                    .spacing(15),
            ),
    )
    .padding(5)
    .into()
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
                            .style(theme::Button::Transparent)
                            .on_press(message::DefineKey::Delete),
                    ),
            )
            .push(
                Container::new(
                    Column::new()
                        .spacing(15)
                        .align_items(Alignment::Center)
                        .push(image::key_mark_icon().width(Length::Fixed(30.0))),
                )
                .height(Length::Fill)
                .align_y(alignment::Vertical::Center),
            )
            .push(
                button::secondary(Some(icon::pencil_icon()), "Set")
                    .on_press(message::DefineKey::Edit),
            )
            .push(Space::with_height(Length::Fixed(5.0))),
    )
    .padding(5)
    .height(Length::Fixed(150.0))
    .width(Length::Fixed(150.0))
    .into()
}

pub fn defined_descriptor_key<'a>(
    name: String,
    duplicate_name: bool,
    incompatible_with_tapminiscript: bool,
) -> Element<'a, message::DefineKey> {
    let col = Column::new()
        .width(Length::Fill)
        .align_items(Alignment::Center)
        .push(
            Row::new()
                .align_items(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    Button::new(icon::cross_icon())
                        .style(theme::Button::Transparent)
                        .on_press(message::DefineKey::Delete),
                ),
        )
        .push(
            Container::new(
                Column::new()
                    .spacing(10)
                    .align_items(Alignment::Center)
                    .push(
                        scrollable(
                            Column::new()
                                .push(text(name).bold())
                                .push(Space::with_height(Length::Fixed(5.0))),
                        )
                        .direction(scrollable::Direction::Horizontal(
                            Properties::new().width(5).scroller_width(5),
                        )),
                    )
                    .push(image::success_mark_icon().width(Length::Fixed(50.0)))
                    .push(Space::with_width(Length::Fixed(1.0))),
            )
            .height(Length::Fill)
            .align_y(alignment::Vertical::Center),
        )
        .push(
            button::secondary(Some(icon::pencil_icon()), "Edit").on_press(message::DefineKey::Edit),
        )
        .push(Space::with_height(Length::Fixed(5.0)));

    if duplicate_name {
        Column::new()
            .align_items(Alignment::Center)
            .push(
                card::invalid(col)
                    .padding(5)
                    .height(Length::Fixed(150.0))
                    .width(Length::Fixed(150.0)),
            )
            .push(text("Duplicate name").small().style(color::RED))
            .into()
    } else if incompatible_with_tapminiscript {
        Column::new()
            .align_items(Alignment::Center)
            .push(
                card::invalid(col)
                    .padding(5)
                    .height(Length::Fixed(150.0))
                    .width(Length::Fixed(150.0)),
            )
            .push(
                text("Taproot is not supported\nby this key device")
                    .small()
                    .style(color::RED),
            )
            .into()
    } else {
        card::simple(col)
            .padding(5)
            .height(Length::Fixed(150.0))
            .width(Length::Fixed(150.0))
            .into()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn edit_key_modal<'a>(
    network: bitcoin::Network,
    hws: Vec<Element<'a, Message>>,
    keys: Vec<Element<'a, Message>>,
    error: Option<&Error>,
    chosen_signer: Option<Fingerprint>,
    hot_signer_fingerprint: &Fingerprint,
    signer_alias: Option<&'a String>,
    form_xpub: &form::Value<String>,
    form_name: &'a form::Value<String>,
    edit_name: bool,
    duplicate_master_fg: bool,
) -> Element<'a, Message> {
    Column::new()
        .push_maybe(error.map(|e| card::error("Failed to import xpub", e.to_string())))
        .push(card::simple(
            Column::new()
                .spacing(25)
                .push(
                    Column::new()
                        .push(
                            Container::new(text("Select a signing device:").bold())
                                .width(Length::Fill),
                        )
                        .spacing(10)
                        .push(
                            Column::with_children(hws).spacing(10)
                        )
                        .push(
                            Column::with_children(keys).spacing(10)
                        )
                        .push(
                            Button::new(if Some(*hot_signer_fingerprint) == chosen_signer {
                                hw::selected_hot_signer(hot_signer_fingerprint, signer_alias)
                            } else {
                                hw::unselected_hot_signer(hot_signer_fingerprint, signer_alias)
                            })
                            .width(Length::Fill)
                            .on_press(Message::UseHotSigner)
                            .style(theme::Button::Border),
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
                                    form::Form::new_trimmed(
                                        &format!(
                                            "[aabbccdd/42'/0']{}pub6DAkq8LWw91WGgUGnkR5Sbzjev5JCsXaTVZQ9MwsPV4BkNFKygtJ8GHodfDVx1udR723nT7JASqGPpKvz7zQ25pUTW6zVEBdiWoaC4aUqik",
                                            if network == bitcoin::Network::Bitcoin {
                                                "x"
                                             } else {
                                                 "t"
                                             }
                                        ),
                                         form_xpub, |msg| {
                                             Message::DefineDescriptor(
                                                 message::DefineDescriptor::KeyModal(
                                                     message::ImportKeyModal::XPubEdited(msg),),)
                                         })
                                    .warning(if network == bitcoin::Network::Bitcoin {
                                        "Please enter correct xpub with origin and without appended derivation path"
                                    } else {
                                        "Please enter correct tpub with origin and without appended derivation path"
                                    })
                    .size(text::P1_SIZE)
                                    .padding(10),
                                )
                                .spacing(10)
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
                                .push(
                                    button::secondary(Some(icon::pencil_icon()), "Edit").on_press(
                                        Message::DefineDescriptor(
                                            message::DefineDescriptor::KeyModal(
                                                message::ImportKeyModal::EditName,
                                            ),
                                        ),
                                    ),
                                ),
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
                                    Message::DefineDescriptor(message::DefineDescriptor::KeyModal(
                                        message::ImportKeyModal::NameEdited(msg),
                                    ))
                                })
                                .warning("Please enter correct alias")
                                .size(text::P1_SIZE)
                                .padding(10),
                            )
                    } else {
                        Column::new()
                    },
                )
                .push_maybe(
                    if duplicate_master_fg {
                        Some(text("A single signing device may not be used more than once per path. (It can still be used in other paths.)").style(color::RED))
                    } else {
                        None
                    }
                )
                .push(
                    if form_xpub.valid && !form_xpub.value.is_empty() && !form_name.value.is_empty() && !duplicate_master_fg
                    {
                        button::primary(None, "Apply")
                            .on_press(Message::DefineDescriptor(
                                message::DefineDescriptor::KeyModal(
                                    message::ImportKeyModal::ConfirmXpub,
                                ),
                            ))
                            .width(Length::Fixed(200.0))
                    } else {
                        button::primary(None, "Apply").width(Length::Fixed(100.0))
                    },
                )
                .align_items(Alignment::Center),
        ))
        .width(Length::Fixed(600.0))
        .into()
}

/// returns y,m,d,h,m
fn duration_from_sequence(sequence: u16) -> (u32, u32, u32, u32, u32) {
    let mut n_minutes = sequence as u32 * 10;
    let n_years = n_minutes / 525960;
    n_minutes -= n_years * 525960;
    let n_months = n_minutes / 43830;
    n_minutes -= n_months * 43830;
    let n_days = n_minutes / 1440;
    n_minutes -= n_days * 1440;
    let n_hours = n_minutes / 60;
    n_minutes -= n_hours * 60;

    (n_years, n_months, n_days, n_hours, n_minutes)
}

pub fn edit_sequence_modal<'a>(sequence: &form::Value<String>) -> Element<'a, Message> {
    let mut col = Column::new()
        .width(Length::Fill)
        .spacing(20)
        .align_items(Alignment::Center)
        .push(text("Activate recovery path after:"))
        .push(
            Row::new()
                .push(
                    Container::new(
                        form::Form::new_trimmed("ex: 1000", sequence, |v| {
                            Message::DefineDescriptor(message::DefineDescriptor::SequenceModal(
                                message::SequenceModal::SequenceEdited(v),
                            ))
                        })
                        .warning("Sequence must be superior to 0 and inferior to 65535"),
                    )
                    .width(Length::Fixed(200.0)),
                )
                .spacing(10)
                .push(text("blocks").bold()),
        );

    if sequence.valid {
        if let Ok(sequence) = u16::from_str(&sequence.value) {
            let (n_years, n_months, n_days, n_hours, n_minutes) = duration_from_sequence(sequence);
            col = col
                .push(
                    [
                        (n_years, "year"),
                        (n_months, "month"),
                        (n_days, "day"),
                        (n_hours, "hour"),
                        (n_minutes, "minute"),
                    ]
                    .iter()
                    .fold(Row::new().spacing(5), |row, (n, unit)| {
                        row.push_maybe(if *n > 0 {
                            Some(
                                text(format!("{} {}{}", n, unit, if *n > 1 { "s" } else { "" }))
                                    .bold(),
                            )
                        } else {
                            None
                        })
                    }),
                )
                .push(
                    Container::new(
                        slider(1..=u16::MAX, sequence, |v| {
                            Message::DefineDescriptor(message::DefineDescriptor::SequenceModal(
                                message::SequenceModal::SequenceEdited(v.to_string()),
                            ))
                        })
                        .step(144_u16), // 144 blocks per day
                    )
                    .width(Length::Fixed(500.0)),
                );
        }
    }

    card::simple(col.push(if sequence.valid {
        button::primary(None, "Apply")
            .on_press(Message::DefineDescriptor(
                message::DefineDescriptor::SequenceModal(message::SequenceModal::ConfirmSequence),
            ))
            .width(Length::Fixed(200.0))
    } else {
        button::primary(None, "Apply").width(Length::Fixed(200.0))
    }))
    .width(Length::Fixed(800.0))
    .into()
}

pub fn hw_list_view(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
    processing: bool,
    selected: bool,
    device_must_support_taproot: bool,
) -> Element<Message> {
    let mut bttn = Button::new(match hw {
        HardwareWallet::Supported {
            kind,
            version,
            fingerprint,
            alias,
            ..
        } => {
            if chosen && processing {
                hw::processing_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            } else if selected {
                hw::selected_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            } else if device_must_support_taproot
                && !is_compatible_with_tapminiscript(kind, version.as_ref())
            {
                hw::warning_hardware_wallet(
                    kind,
                    version.as_ref(),
                    fingerprint,
                    alias.as_ref(),
                    "Device firmware version does not support taproot miniscript",
                )
            } else {
                hw::supported_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            }
        }
        HardwareWallet::Unsupported {
            version,
            kind,
            reason,
            ..
        } => match reason {
            UnsupportedReason::NotPartOfWallet(fg) => {
                hw::unrelated_hardware_wallet(&kind.to_string(), version.as_ref(), fg)
            }
            UnsupportedReason::WrongNetwork => {
                hw::wrong_network_hardware_wallet(&kind.to_string(), version.as_ref())
            }
            _ => hw::unsupported_hardware_wallet(&kind.to_string(), version.as_ref()),
        },
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => hw::locked_hardware_wallet(kind, pairing_code.as_ref()),
    })
    .style(theme::Button::Border)
    .width(Length::Fill);
    if !processing && hw.is_supported() {
        bttn = bttn.on_press(Message::Select(i));
    }
    Container::new(bttn)
        .width(Length::Fill)
        .style(theme::Container::Card(theme::Card::Simple))
        .into()
}

pub fn key_list_view<'a>(
    i: usize,
    name: &'a str,
    fingerprint: &'a Fingerprint,
    kind: Option<&'a DeviceKind>,
    version: Option<&'a async_hwi::Version>,
    chosen: bool,
    device_must_support_taproot: bool,
) -> Element<'a, Message> {
    let bttn = Button::new(if chosen {
        hw::selected_hardware_wallet(
            kind.map(|k| k.to_string()).unwrap_or_default(),
            version,
            fingerprint,
            Some(name),
        )
    } else if device_must_support_taproot
        && kind.map(|kind| is_compatible_with_tapminiscript(kind, version)) == Some(false)
    {
        hw::warning_hardware_wallet(
            kind.map(|k| k.to_string()).unwrap_or_default(),
            version,
            fingerprint,
            Some(name),
            "Device firmware version does not support taproot miniscript",
        )
    } else {
        hw::supported_hardware_wallet(
            kind.map(|k| k.to_string()).unwrap_or_default(),
            version,
            fingerprint,
            Some(name),
        )
    })
    .style(theme::Button::Border)
    .width(Length::Fill)
    .on_press(Message::DefineDescriptor(
        message::DefineDescriptor::KeyModal(message::ImportKeyModal::SelectKey(i)),
    ));
    Container::new(bttn)
        .width(Length::Fill)
        .style(theme::Container::Card(theme::Card::Simple))
        .into()
}

pub fn backup_mnemonic<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    words: &'a [&'static str; 12],
    done: bool,
) -> Element<'a, Message> {
    layout(
        progress,
        email,
        "Backup your mnemonic",
        Column::new()
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
                                        .width(Length::Fixed(50.0)),
                                )
                                .push(text(*w).bold()),
                        )
                    }),
            )
            .push(checkbox("I have backed up my mnemonic", done).on_toggle(Message::UserActionDone))
            .push(if done {
                button::primary(None, "Next")
                    .on_press(Message::Next)
                    .width(Length::Fixed(200.0))
            } else {
                button::primary(None, "Next").width(Length::Fixed(200.0))
            })
            .spacing(50),
        true,
        Some(Message::Previous),
    )
}

pub fn recover_mnemonic<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    words: &'a [(String, bool); 12],
    current: usize,
    suggestions: &'a [String],
    recover: bool,
    error: Option<&'a String>,
) -> Element<'a, Message> {
    layout(
        progress,
        email,
        "Import Mnemonic",
        Column::new()
            .push(text(prompt::RECOVER_MNEMONIC_HELP))
            .push_maybe(if recover {
                Some(
                    Column::new()
                        .align_items(Alignment::Center)
                        .push(
                            Container::new(if !suggestions.is_empty() {
                                suggestions.iter().fold(Row::new().spacing(5), |row, sugg| {
                                    row.push(
                                        Button::new(text(sugg))
                                            .style(theme::Button::Secondary)
                                            .on_press(Message::MnemonicWord(
                                                current,
                                                sugg.to_string(),
                                            )),
                                    )
                                })
                            } else {
                                Row::new()
                            })
                            // Fixed height in order to not move words list
                            .height(Length::Fixed(50.0)),
                        )
                        .push(words.iter().enumerate().fold(
                            Column::new().spacing(5),
                            |acc, (i, (word, valid))| {
                                acc.push(
                                    Row::new()
                                        .spacing(10)
                                        .align_items(Alignment::Center)
                                        .push(
                                            Container::new(text(format!("#{}", i + 1)).small())
                                                .width(Length::Fixed(50.0)),
                                        )
                                        .push(
                                            Container::new(TextInput::new("", word).on_input(
                                                move |msg| Message::MnemonicWord(i, msg),
                                            ))
                                            .width(Length::Fixed(100.0)),
                                        )
                                        .push_maybe(if *valid {
                                            Some(icon::circle_check_icon().style(color::GREEN))
                                        } else {
                                            None
                                        }),
                                )
                            },
                        ))
                        .push(Space::with_height(Length::Fixed(50.0)))
                        .push_maybe(error.map(|e| card::invalid(text(e).style(color::RED)))),
                )
            } else {
                None
            })
            .push(if !recover {
                Row::new()
                    .spacing(10)
                    .push(
                        button::secondary(None, "Import mnemonic")
                            .on_press(Message::ImportMnemonic(true))
                            .width(Length::Fixed(200.0)),
                    )
                    .push(
                        button::primary(None, "Skip")
                            .on_press(Message::Skip)
                            .width(Length::Fixed(200.0)),
                    )
            } else {
                Row::new()
                    .spacing(10)
                    .push(
                        button::secondary(None, "Cancel")
                            .on_press(Message::ImportMnemonic(false))
                            .width(Length::Fixed(200.0)),
                    )
                    .push(
                        if words.iter().any(|(_, valid)| !valid) || error.is_some() {
                            button::primary(None, "Next").width(Length::Fixed(200.0))
                        } else {
                            button::primary(None, "Next")
                                .on_press(Message::Next)
                                .width(Length::Fixed(200.0))
                        },
                    )
            })
            .spacing(50),
        true,
        Some(Message::Previous),
    )
}

pub fn choose_backend(
    progress: (usize, usize),
    connection_step: Element<Message>,
) -> Element<Message> {
    layout(
        progress,
        None,
        "Choose backend",
        Column::new()
            .push(
                Row::new()
                    .spacing(20)
                    .push(
                        Column::new()
                            .spacing(20)
                            .align_items(Alignment::Center)
                            .width(Length::FillPortion(1))
                            .push(image::liana_brand_grey().height(Length::Fixed(100.0)))
                            .push(text::p2_medium(LIANA_DESC).style(color::GREY_3))
                            .push(button::primary(None, "Install local wallet").on_press(
                                Message::SelectBackend(
                                    message::SelectBackend::ContinueWithLocalWallet,
                                ),
                            )),
                    )
                    .push(
                        Column::new()
                            .spacing(20)
                            .align_items(Alignment::Center)
                            .width(Length::FillPortion(1))
                            .push(image::wizardsardine().height(Length::Fixed(100.0)))
                            .push(text::p2_medium(LIANALITE_DESC).style(color::GREY_3))
                            .push(connection_step),
                    ),
            )
            .spacing(50),
        true,
        Some(Message::Previous),
    )
}

pub fn connection_step_enter_email<'a>(
    email: &form::Value<String>,
    processing: bool,
    connection_error: Option<&Error>,
    auth_error: Option<&'static str>,
) -> Element<'a, Message> {
    Column::new()
        .spacing(20)
        .push_maybe(connection_error.map(|e| text(e.to_string()).style(color::ORANGE)))
        .push_maybe(auth_error.map(|e| text(e.to_string()).style(color::ORANGE)))
        .push(
            form::Form::new_trimmed("email", email, |msg| {
                Message::SelectBackend(message::SelectBackend::EmailEdited(msg))
            })
            .size(text::P1_SIZE)
            .padding(10)
            .warning("Email is not valid"),
        )
        .push(
            button::primary(None, "Next").on_press_maybe(if processing || !email.valid {
                None
            } else {
                Some(Message::SelectBackend(message::SelectBackend::RequestOTP))
            }),
        )
        .into()
}

pub fn connection_step_enter_otp<'a>(
    email: &'a str,
    otp: &form::Value<String>,
    processing: bool,
    connection_error: Option<&Error>,
    auth_error: Option<&'static str>,
) -> Element<'a, Message> {
    Column::new()
        .spacing(20)
        .push(text(email).style(color::GREEN))
        .push(text("An authentication token has been emailed to you"))
        .push_maybe(connection_error.map(|e| text(e.to_string()).style(color::ORANGE)))
        .push_maybe(auth_error.map(|e| text(e.to_string()).style(color::ORANGE)))
        .push(
            form::Form::new_trimmed("Token", otp, |msg| {
                Message::SelectBackend(message::SelectBackend::OTPEdited(msg))
            })
            .size(text::P1_SIZE)
            .padding(10)
            .warning("Token is not valid"),
        )
        .push(
            Row::new()
                .spacing(10)
                .push(
                    button::primary(Some(icon::previous_icon()), "Change Email")
                        .on_press(Message::SelectBackend(message::SelectBackend::EditEmail)),
                )
                .push(
                    button::primary(None, "Resend token").on_press_maybe(if processing {
                        None
                    } else {
                        Some(Message::SelectBackend(message::SelectBackend::RequestOTP))
                    }),
                ),
        )
        .into()
}

pub fn connection_step_connected<'a>(
    email: &'a str,
    processing: bool,
    connection_error: Option<&Error>,
    auth_error: Option<&'static str>,
) -> Element<'a, Message> {
    Column::new()
        .spacing(20)
        .push(text(email).style(color::GREEN))
        .push_maybe(connection_error.map(|e| text(e.to_string()).style(color::ORANGE)))
        .push_maybe(auth_error.map(|e| text(e.to_string()).style(color::ORANGE)))
        .push(Container::new(
            Row::new()
                .spacing(10)
                .push(
                    button::primary(Some(icon::previous_icon()), "Change Email")
                        .on_press(Message::SelectBackend(message::SelectBackend::EditEmail)),
                )
                .push(
                    button::primary(None, "Continue").on_press_maybe(if processing {
                        None
                    } else {
                        Some(Message::SelectBackend(
                            message::SelectBackend::ContinueWithRemoteBackend,
                        ))
                    }),
                ),
        ))
        .into()
}

pub const LIANALITE_DESC: &str = "Use the connection to the Bitcoin network provided by Wizardsardine. This removes the need for running a Bitcoin full node on your machine. It also provides synchronisation of your wallet data (labels, transactions, etc..) across your machines and participants in your wallet. We will also keep a backup of your wallet descriptor for you. This is the most convenient option but has privacy implications: your data would be stored on our servers (but never shared with a third party).";

pub const LIANA_DESC: &str = "This option creates a wallet on your machine. The wallet will never access any of our servers, we would not even be able to know you use our wallet. This option requires a local Bitcoin full node. A full node is necessary to use Bitcoin in a sovereign way, but it is more accessible than it sounds. The Liana wallet can download and run one for you so you don't have to manage it yourself. It will never use more than a couple GB of disk space. The initial synchronisation of the node takes time and is computationally intensive, but past this point running a Bitcoin full node on your machine is seamless.";

fn layout<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    title: &'static str,
    content: impl Into<Element<'a, Message>>,
    padding_left: bool,
    previous_message: Option<Message>,
) -> Element<'a, Message> {
    let mut prev_button = button::transparent(Some(icon::previous_icon()), "Previous");
    if let Some(msg) = previous_message {
        prev_button = prev_button.on_press(msg);
    }
    Container::new(scrollable(
        Column::new()
            .width(Length::Fill)
            .push(Row::new().push(Space::with_width(Length::Fill)).push_maybe(
                email.map(|e| Container::new(p1_regular(e).style(color::GREEN)).padding(20)),
            ))
            .push(Space::with_height(Length::Fixed(100.0)))
            .push(
                Row::new()
                    .align_items(Alignment::Center)
                    .push(
                        Container::new(prev_button)
                            .width(Length::FillPortion(2))
                            .center_x(),
                    )
                    .push(Container::new(h3(title)).width(Length::FillPortion(8)))
                    .push_maybe(if progress.1 > 0 {
                        Some(
                            Container::new(text(format!("{} | {}", progress.0, progress.1)))
                                .width(Length::FillPortion(2))
                                .center_x(),
                        )
                    } else {
                        None
                    }),
            )
            .push(
                Row::new()
                    .push(Space::with_width(Length::FillPortion(2)))
                    .push(
                        Container::new(
                            Column::new()
                                .push(Space::with_height(Length::Fixed(100.0)))
                                .push(content),
                        )
                        .width(Length::FillPortion(if padding_left {
                            8
                        } else {
                            10
                        })),
                    )
                    .push_maybe(if padding_left {
                        Some(Space::with_width(Length::FillPortion(2)))
                    } else {
                        None
                    }),
            ),
    ))
    .center_x()
    .height(Length::Fill)
    .width(Length::Fill)
    .style(theme::Container::Background)
    .into()
}

mod threshsold_input {
    use iced::alignment::{self, Alignment};
    use iced::widget::{component, Component};
    use iced::Length;
    use liana_ui::{component::text::*, icon, theme, widget::*};

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

    impl<Message> Component<Message, theme::Theme> for ThresholdInput<Message> {
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
                    .style(theme::Button::Transparent)
                    .width(Length::Fixed(50.0))
                    .on_press(on_press)
            };

            Column::new()
                .width(Length::Fixed(150.0))
                .push(button(icon::up_icon().size(30), Event::IncrementPressed))
                .push(text("Threshold:").small().bold())
                .push(
                    Container::new(text(format!("{}/{}", self.value, self.max)).size(30))
                        .align_y(alignment::Vertical::Center),
                )
                .push(button(icon::down_icon().size(30), Event::DecrementPressed))
                .align_items(Alignment::Center)
                .into()
        }
    }

    impl<'a, Message> From<ThresholdInput<Message>> for Element<'a, Message>
    where
        Message: 'a,
    {
        fn from(numeric_input: ThresholdInput<Message>) -> Self {
            component(numeric_input)
        }
    }
}
