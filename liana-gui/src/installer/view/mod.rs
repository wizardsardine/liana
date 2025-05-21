pub mod editor;

use async_hwi::utils::extract_keys_and_template;
use iced::widget::{
    checkbox, radio, scrollable, scrollable::Scrollbar, tooltip, Button, Space, TextInput,
};
use iced::{
    alignment,
    widget::{progress_bar, tooltip as iced_tooltip},
    Alignment, Length,
};

use async_hwi::DeviceKind;
use liana::miniscript::bitcoin::bip32::ChildNumber;
use liana_ui::component::text::{self, p2_regular};
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::{collections::HashSet, str::FromStr};

use liana::{
    descriptors::{LianaDescriptor, LianaPolicy},
    miniscript::bitcoin::{self, bip32::Fingerprint},
};
use liana_ui::{
    component::{
        button, card, collapse, form, hw, separation,
        text::{h2, h3, h4_bold, p1_bold, p1_regular, text, Text},
    },
    icon, theme,
    widget::*,
};

use crate::{
    app::settings,
    hw::{is_compatible_with_tapminiscript, HardwareWallet, UnsupportedReason},
    installer::{
        descriptor::{Key, PathSequence, PathWarning},
        message::{self, DefineBitcoind, DefineNode, Message},
        prompt,
        step::{DownloadState, InstallState},
        view::editor::duration_from_sequence,
        Error,
    },
    node::{
        bitcoind::{ConfigField, RpcAuthType, RpcAuthValues, StartInternalBitcoindError},
        electrum, NodeType,
    },
};

pub fn import_wallet_or_descriptor<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    invitation: &'a form::Value<String>,
    invitation_wallet: Option<&'a str>,
    imported_descriptor: &'a form::Value<String>,
    error: Option<&'a String>,
    wallets: Vec<(&'a String, Option<&'a String>)>,
) -> Element<'a, Message> {
    let mut col_wallets = Column::new()
        .spacing(20)
        .push(h4_bold("Load a previously used wallet"));
    let no_wallets = wallets.is_empty();
    for (i, (name, alias)) in wallets.into_iter().enumerate() {
        col_wallets = col_wallets.push(
            Button::new(
                Column::new()
                    .push_maybe(alias.map(p1_bold))
                    .push(p1_regular(name))
                    .width(Length::Fill),
            )
            .style(theme::button::secondary)
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
                    .push(h4_bold("Load a shared wallet").style(theme::text::primary))
                    .push(
                        text("If you received an invitation to join a shared wallet")
                            .style(theme::text::secondary),
                    ),
            )
            .padding(15)
            .width(Length::Fill)
            .style(theme::button::transparent_border)
        },
        || {
            Button::new(
                Column::new()
                    .spacing(5)
                    .push(h4_bold("Load a shared wallet").style(theme::text::primary))
                    .push(
                        text("Type the invitation token you received by email")
                            .style(theme::text::secondary),
                    ),
            )
            .padding(15)
            .width(Length::Fill)
            .style(theme::button::transparent_border)
        },
        move || {
            if let Some(wallet) = invitation_wallet {
                Element::<'a, Message>::from(
                    Column::new()
                        .push(Space::with_height(0))
                        .push(
                            Row::new()
                                .spacing(5)
                                .push(Space::with_width(15))
                                .push(text("Accept invitation for wallet:"))
                                .push(text(wallet).bold()),
                        )
                        .push(
                            Row::new()
                                .push(Space::with_width(Length::Fill))
                                .push(
                                    button::secondary(None, "Accept")
                                        .width(Length::Fixed(200.0))
                                        .on_press(Message::ImportRemoteWallet(
                                            message::ImportRemoteWallet::AcceptInvitation,
                                        )),
                                )
                                .push(Space::with_width(Length::Fill)),
                        )
                        .push(Space::with_width(5))
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
                                    button::secondary(None, "Next")
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
                    .push(h4_bold("Load a wallet from descriptor").style(theme::text::primary))
                    .push(
                        text("Creates a new wallet from the descriptor")
                            .style(theme::text::secondary),
                    ),
            )
            .padding(15)
            .width(Length::Fill)
            .style(theme::button::transparent_border)
        },
        || {
            Button::new(
                Column::new()
                    .spacing(5)
                    .push(h4_bold("Load a wallet from descriptor").style(theme::text::primary))
                    .push(
                        text("Creates a new wallet from the descriptor")
                            .style(theme::text::secondary),
                    ),
            )
            .padding(15)
            .width(Length::Fill)
            .style(theme::button::transparent_border)
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
                                button::secondary(None, "Next")
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
    imported_backup: bool,
    wrong_network: bool,
    error: Option<&String>,
) -> Element<'a, Message> {
    let valid = !imported_descriptor.value.is_empty() && imported_descriptor.valid;

    let col_descriptor = Column::new()
        .push(text("Descriptor:").bold())
        .push(Space::with_height(10))
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
        );

    let descriptor = if imported_backup {
        None
    } else {
        Some(col_descriptor)
    };

    let or = if !valid && !imported_backup {
        Some(
            Row::new()
                .push(text("or").bold())
                .push(Space::with_width(Length::Fill)),
        )
    } else {
        None
    };

    let import_backup = if !valid && !imported_backup {
        Some(
            Row::new()
                .push(button::secondary(None, "Import backup").on_press(Message::ImportBackup))
                .push(Space::with_width(Length::Fill)),
        )
    } else {
        None
    };

    let backup_imported = if imported_backup {
        Some(
            Row::new()
                .push(text("Backup successfuly imported!").bold())
                .push(Space::with_width(Length::Fill)),
        )
    } else {
        None
    };

    layout(
        progress,
        email,
        "Import the wallet",
        Column::new()
            .push(
                Column::new()
                    .spacing(20)
                    .push_maybe(descriptor)
                    .push_maybe(or)
                    .push_maybe(import_backup)
                    .push_maybe(backup_imported)
                    .push(text(
                        "If you are using a Bitcoin Core node, \
                you will need to perform a rescan of \
                the blockchain after creating the wallet \
                in order to see your coins and past \
                transactions. This can be done in \
                Settings > Node.",
                    )),
            )
            .push(
                if imported_descriptor.value.is_empty() || !imported_descriptor.valid {
                    button::secondary(None, "Next").width(Length::Fixed(200.0))
                } else {
                    button::secondary(None, "Next")
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
                    Row::new().align_y(Alignment::Center).push(
                        Column::new()
                            .push(text("Generate a new mnemonic").bold())
                            .push(text(BACKUP_WARNING).small().style(theme::text::warning))
                            .spacing(5)
                            .width(Length::Fill),
                    ),
                )
                .on_press(Message::UseHotSigner)
                .padding(10)
                .style(theme::button::secondary)
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
                                    .align_y(Alignment::End)
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
                            .align_y(Alignment::Center)
                            .push(
                                Container::new(
                                    scrollable(Container::new(text(xpub).small()).padding(10))
                                        .direction(scrollable::Direction::Horizontal(
                                            Scrollbar::new().width(5).scroller_width(5),
                                        )),
                                )
                                .width(Length::Fill),
                            )
                            .push(
                                Container::new(
                                    button::primary(Some(icon::backup_icon()), "Export")
                                        .on_press(Message::ExportXpub(xpub.clone()))
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
    .style(theme::card::simple)
    .into()
}

pub fn hardware_wallet_xpubs<'a>(
    i: usize,
    hw: &'a HardwareWallet,
    xpubs: Option<&'a Vec<String>>,
    processing: bool,
    error: Option<&Error>,
    accounts: &HashMap<Fingerprint, ChildNumber>,
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
                hw::supported_hardware_wallet_with_account(
                    kind,
                    version.as_ref(),
                    *fingerprint,
                    alias.as_ref(),
                    accounts.get(fingerprint).cloned(),
                    true,
                )
            }
        }
        HardwareWallet::Unsupported {
            version,
            kind,
            reason,
            ..
        } => match reason {
            UnsupportedReason::NotPartOfWallet(fg) => {
                hw::unrelated_hardware_wallet(kind.to_string(), version.as_ref(), fg)
            }
            UnsupportedReason::WrongNetwork => {
                hw::wrong_network_hardware_wallet(kind.to_string(), version.as_ref())
            }
            UnsupportedReason::Version {
                minimal_supported_version,
            } => hw::unsupported_version_hardware_wallet(
                kind.to_string(),
                version.as_ref(),
                minimal_supported_version,
            ),
            _ => hw::unsupported_hardware_wallet(kind.to_string(), version.as_ref()),
        },
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => hw::locked_hardware_wallet(kind, pairing_code.as_ref()),
    })
    .style(theme::button::secondary)
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
                            .align_y(Alignment::Center)
                            .push(
                                Container::new(
                                    scrollable(Container::new(text(xpub).small()).padding(10))
                                        .direction(scrollable::Direction::Horizontal(
                                            Scrollbar::new().width(5).scroller_width(5),
                                        )),
                                )
                                .width(Length::Fill),
                            )
                            .push(
                                Container::new(
                                    button::primary(Some(icon::backup_icon()), "Export")
                                        .on_press(Message::ExportXpub(xpub.clone()))
                                        .width(Length::Shrink),
                                )
                                .padding(10),
                            ),
                    )
                })
            })),
    )
    .style(theme::card::simple)
    .into()
}

pub fn share_xpubs<'a>(
    email: Option<&'a str>,
    hws: Vec<Element<'a, Message>>,
    signer: Element<'a, Message>,
) -> Element<'a, Message> {
    let info = Column::new()
        .push(Space::with_height(5))
        .push(tooltip::Tooltip::new(
            icon::tooltip_icon(),
            "Switch account if you already use the same hardware in other configurations",
            tooltip::Position::Bottom,
        ));
    let title = Row::new()
        .push(text("Import an extended public key by selecting a signing device:").bold())
        .push(Space::with_width(10))
        .push(info)
        .push(Space::with_width(Length::Fill));
    layout(
        (0, 0),
        email,
        "Share your public keys (Xpubs)",
        Column::new()
            .push(title)
            .push_maybe(if hws.is_empty() {
                Some(p1_regular("No signing device connected").style(theme::text::secondary))
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
    descriptor: &'a LianaDescriptor,
    hws: &'a [HardwareWallet],
    registered: &HashSet<bitcoin::bip32::Fingerprint>,
    error: Option<&Error>,
    processing: bool,
    chosen_hw: Option<usize>,
    done: bool,
    created_desc: bool,
) -> Element<'a, Message> {
    let descriptor_str = descriptor.to_string();
    let displayed_descriptor =
        if let Ok((template, keys)) = extract_keys_and_template::<String>(&descriptor_str) {
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
                                        scrollable::Scrollbar::new().width(5).scroller_width(5),
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
                                            scrollable::Scrollbar::new().width(5).scroller_width(5),
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
                                .push(text(descriptor_str.to_owned()).small())
                                .push(Space::with_height(Length::Fixed(5.0))),
                        )
                        .direction(scrollable::Direction::Horizontal(
                            scrollable::Scrollbar::new().width(5).scroller_width(5),
                        )),
                    )
                    .push(
                        Row::new().push(Column::new().width(Length::Fill)).push(
                            button::secondary(Some(icon::clipboard_icon()), "Copy")
                                .on_press(Message::Clibpboard(descriptor_str)),
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
                                    Some(descriptor),
                                    false,
                                    None,
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
                button::secondary(None, "Next")
                    .on_press(Message::Next)
                    .width(Length::Fixed(200.0))
            } else {
                button::secondary(None, "Next").width(Length::Fixed(200.0))
            })
            .push(Space::with_height(5))
            .spacing(50),
        true,
        if !processing {
        Some(Message::Previous)
        } else {
            None
        }
    )
}

pub fn backup_descriptor<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    descriptor: &'a LianaDescriptor,
    keys: &'a HashMap<Fingerprint, settings::KeySetting>,
    error: Option<&Error>,
    done: bool,
) -> Element<'a, Message> {
    let backup_button = if done {
        button::secondary(Some(icon::backup_icon()), "Back Up Wallet")
            .on_press(Message::BackupWallet)
    } else {
        button::primary(Some(icon::backup_icon()), "Back Up Wallet").on_press(Message::BackupWallet)
    };

    layout(
        progress,
        email,
        "Back Up your wallet",
        Column::new()
            .push(
                Column::new()
                    .push(text(prompt::BACKUP_DESCRIPTOR_MESSAGE))
                    .push(collapse::Collapse::new(
                        || {
                            Button::new(
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .spacing(10)
                                    .push(text("Learn more").small().bold())
                                    .push(icon::collapse_icon()),
                            )
                            .style(theme::button::transparent)
                        },
                        || {
                            Button::new(
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .spacing(10)
                                    .push(text("Learn more").small().bold())
                                    .push(icon::collapsed_icon()),
                            )
                            .style(theme::button::transparent)
                        },
                        help_backup,
                    ))
                    .max_width(1000),
            )
            .push_maybe(error.map(|e| card::error("Failed to export backup", e.to_string())))
            .push(
                card::simple(
                    Column::new()
                        .push(text("The descriptor:").small().bold())
                        .push(
                            scrollable(
                                Column::new()
                                    .push(text(descriptor.to_string()).small())
                                    .push(Space::with_height(Length::Fixed(5.0))),
                            )
                            .direction(
                                scrollable::Direction::Horizontal(
                                    scrollable::Scrollbar::new().width(5).scroller_width(5),
                                ),
                            ),
                        )
                        .push(
                            Row::new()
                                .push(Space::with_width(Length::Fill))
                                .push(backup_button)
                                .push(Space::with_width(10))
                                .push(
                                    button::secondary(Some(icon::clipboard_icon()), "Copy")
                                        .on_press(Message::Clibpboard(descriptor.to_string())),
                                ),
                        )
                        .spacing(10),
                )
                .max_width(1500),
            )
            .push(
                card::simple(display_policy(descriptor.policy(), keys))
                    .width(Length::Fill)
                    .max_width(1500),
            )
            .push(
                checkbox("I have backed up my wallet/descriptor", done)
                    .on_toggle(Message::UserActionDone),
            )
            .push(if done {
                button::primary(None, "Next")
                    .on_press(Message::Next)
                    .width(Length::Fixed(200.0))
            } else {
                button::secondary(None, "Next").width(Length::Fixed(200.0))
            })
            .push(Space::with_height(20.0))
            .spacing(50),
        true,
        Some(Message::Previous),
    )
}

fn display_policy(
    policy: LianaPolicy,
    keys: &HashMap<Fingerprint, settings::KeySetting>,
) -> Element<'_, Message> {
    let (primary_threshold, primary_keys) = policy.primary_path().thresh_origins();
    // The iteration over an HashMap keys can have a different order at each refresh
    let mut primary_keys: Vec<Fingerprint> = primary_keys.into_keys().collect();
    primary_keys.sort();
    let recovery_paths = policy.recovery_paths();
    let mut col = Column::new().push(
        Row::new()
            .spacing(5)
            .push(
                text(format!(
                    "{} signature{}",
                    primary_threshold,
                    if primary_threshold > 1 { "s" } else { "" }
                ))
                .bold(),
            )
            .push(if primary_keys.len() > 1 {
                text(format!("out of {} by", primary_keys.len()))
            } else {
                text("by")
            })
            .push(
                primary_keys
                    .iter()
                    .enumerate()
                    .fold(Row::new().spacing(5), |row, (i, k)| {
                        let content = if let Some(key) = keys.get(k) {
                            Container::new(
                                iced_tooltip::Tooltip::new(
                                    text(key.name.clone()).bold(),
                                    text(k.to_string()),
                                    iced_tooltip::Position::Bottom,
                                )
                                .style(theme::card::simple),
                            )
                        } else {
                            Container::new(text(format!("[{}]", k)).bold())
                        };
                        if primary_keys.len() == 1 || i == primary_keys.len() - 1 {
                            row.push(content)
                        } else if i <= primary_keys.len() - 2 {
                            row.push(content).push(text("and"))
                        } else {
                            row.push(content).push(text(","))
                        }
                    }),
            )
            .push(text("can always spend this wallet's funds (Primary path)")),
    );
    for (i, (sequence, recovery_path)) in recovery_paths.iter().enumerate() {
        let (threshold, recovery_keys) = recovery_path.thresh_origins();
        // The iteration over an HashMap keys can have a different order at each refresh
        let mut recovery_keys: Vec<Fingerprint> = recovery_keys.into_keys().collect();
        recovery_keys.sort();
        col = col.push(
            Row::new()
                .spacing(5)
                .push(
                    text(format!(
                        "{} signature{}",
                        threshold,
                        if threshold > 1 { "s" } else { "" }
                    ))
                    .bold(),
                )
                .push(if recovery_keys.len() > 1 {
                    text(format!("out of {} by", recovery_keys.len()))
                } else {
                    text("by")
                })
                .push(recovery_keys.iter().enumerate().fold(
                    Row::new().spacing(5),
                    |row, (i, k)| {
                        let content = if let Some(key) = keys.get(k) {
                            Container::new(
                                iced_tooltip::Tooltip::new(
                                    text(key.name.clone()).bold(),
                                    text(k.to_string()),
                                    iced_tooltip::Position::Bottom,
                                )
                                .style(theme::card::simple),
                            )
                        } else {
                            Container::new(text(format!("[{}]", k)).bold())
                        };
                        if recovery_keys.len() == 1 || i == recovery_keys.len() - 1 {
                            row.push(content)
                        } else if i <= recovery_keys.len() - 2 {
                            row.push(content).push(text("and"))
                        } else {
                            row.push(content).push(text(","))
                        }
                    },
                ))
                .push(text("can spend coins inactive for"))
                .push(
                    text(format!(
                        "{} blocks (~{})",
                        sequence,
                        expire_message_units(*sequence as u32).join(",")
                    ))
                    .bold(),
                )
                .push(text(
                    // If max timelock and all keys are from provider, then it's a safety net path.
                    if *sequence == u16::MAX
                        && recovery_keys
                            .iter()
                            .all(|fg| keys.get(fg).is_some_and(|k| k.provider_key.is_some()))
                    {
                        "(Safety Net path)".to_string()
                    } else {
                        format!("(Recovery path #{})", i + 1)
                    },
                )),
        );
    }
    Column::new()
        .spacing(10)
        .push(text("The wallet policy:").bold())
        .push(scrollable(col).direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new().width(5).scroller_width(5),
        )))
        .into()
}

/// returns y,m,d
fn expire_message_units(sequence: u32) -> Vec<String> {
    let mut n_minutes = sequence * 10;
    let n_years = n_minutes / 525960;
    n_minutes -= n_years * 525960;
    let n_months = n_minutes / 43830;
    n_minutes -= n_months * 43830;
    let n_days = n_minutes / 1440;

    #[allow(clippy::nonminimal_bool)]
    if n_years != 0 || n_months != 0 || n_days != 0 {
        [(n_years, "y"), (n_months, "m"), (n_days, "d")]
            .iter()
            .filter_map(|(n, u)| {
                if *n != 0 {
                    Some(format!("{}{}", n, u))
                } else {
                    None
                }
            })
            .collect()
    } else {
        n_minutes -= n_days * 1440;
        let n_hours = n_minutes / 60;
        n_minutes -= n_hours * 60;
        [(n_hours, "h"), (n_minutes, "m")]
            .iter()
            .filter_map(|(n, u)| {
                if *n != 0 {
                    Some(format!("{}{}", n, u))
                } else {
                    None
                }
            })
            .collect()
    }
}

pub fn help_backup<'a>() -> Element<'a, Message> {
    text(prompt::BACKUP_DESCRIPTOR_HELP).small().into()
}

pub fn define_bitcoin_node<'a>(
    progress: (usize, usize),
    available_node_types: impl Iterator<Item = NodeType>,
    selected_node_type: NodeType,
    node_view: Element<'a, Message>,
    is_running: Option<&Result<(), Error>>,
    can_try_ping: bool,
    waiting_for_ping_result: bool,
) -> Element<'a, Message> {
    let col = Column::new()
        .push(
            available_node_types.fold(
                Row::new()
                    .push(text("Node type:").small().bold())
                    .spacing(10),
                |row, node_type| {
                    row.push(radio(
                        match node_type {
                            NodeType::Bitcoind => "Bitcoin Core",
                            NodeType::Electrum => "Electrum",
                        },
                        node_type,
                        Some(selected_node_type),
                        |new_selection| {
                            Message::DefineNode(message::DefineNode::NodeTypeSelected(
                                new_selection,
                            ))
                        },
                    ))
                    .spacing(30)
                    .align_y(Alignment::Center)
                },
            ),
        )
        .push(node_view)
        .push_maybe(if waiting_for_ping_result {
            Some(Container::new(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(text("Checking connection...")),
            ))
        } else if is_running.is_some() {
            is_running.map(|res| {
                if res.is_ok() {
                    Container::new(
                        Row::new()
                            .spacing(10)
                            .align_y(Alignment::Center)
                            .push(icon::circle_check_icon().style(theme::text::success))
                            .push(text("Connection checked").style(theme::text::success)),
                    )
                } else {
                    Container::new(
                        Row::new()
                            .spacing(10)
                            .align_y(Alignment::Center)
                            .push(icon::circle_cross_icon().style(theme::text::error))
                            .push(text("Connection failed").style(theme::text::error)),
                    )
                }
            })
        } else {
            Some(Container::new(Space::with_height(Length::Fixed(21.0))))
        })
        .push(
            Row::new()
                .spacing(10)
                .push(Container::new(
                    button::secondary(None, "Check connection")
                        .on_press_maybe(if can_try_ping && !waiting_for_ping_result {
                            Some(Message::DefineNode(DefineNode::Ping))
                        } else {
                            None
                        })
                        .width(Length::Fixed(200.0)),
                ))
                .push(if is_running.map(|res| res.is_ok()).unwrap_or(false) {
                    button::secondary(None, "Next")
                        .on_press(Message::Next)
                        .width(Length::Fixed(200.0))
                } else {
                    button::secondary(None, "Next").width(Length::Fixed(200.0))
                }),
        )
        .spacing(50);

    layout(
        progress,
        None,
        "Set up connection to the Bitcoin node",
        col,
        true,
        Some(Message::Previous),
    )
}

pub fn define_bitcoind<'a>(
    address: &form::Value<String>,
    rpc_auth_vals: &RpcAuthValues,
    selected_auth_type: &RpcAuthType,
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
                Message::DefineNode(DefineNode::DefineBitcoind(
                    DefineBitcoind::ConfigFieldEdited(ConfigField::Address, msg),
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
                .style(theme::text::warning)
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
                                Message::DefineNode(DefineNode::DefineBitcoind(
                                    DefineBitcoind::RpcAuthTypeSelected(new_selection),
                                ))
                            },
                        ))
                        .spacing(30)
                        .align_y(Alignment::Center)
                    },
                ),
        )
        .push(match selected_auth_type {
            RpcAuthType::CookieFile => Row::new().push(
                form::Form::new_trimmed("Cookie path", &rpc_auth_vals.cookie_path, |msg| {
                    Message::DefineNode(DefineNode::DefineBitcoind(
                        DefineBitcoind::ConfigFieldEdited(ConfigField::CookieFilePath, msg),
                    ))
                })
                .warning("Please enter correct path")
                .size(text::P1_SIZE)
                .padding(10),
            ),
            RpcAuthType::UserPass => Row::new()
                .push(
                    form::Form::new_trimmed("User", &rpc_auth_vals.user, |msg| {
                        Message::DefineNode(DefineNode::DefineBitcoind(
                            DefineBitcoind::ConfigFieldEdited(ConfigField::User, msg),
                        ))
                    })
                    .warning("Please enter correct user")
                    .size(text::P1_SIZE)
                    .padding(10),
                )
                .push(
                    form::Form::new_trimmed("Password", &rpc_auth_vals.password, |msg| {
                        Message::DefineNode(DefineNode::DefineBitcoind(
                            DefineBitcoind::ConfigFieldEdited(ConfigField::Password, msg),
                        ))
                    })
                    .warning("Please enter correct password")
                    .size(text::P1_SIZE)
                    .padding(10),
                )
                .spacing(10),
        })
        .spacing(10);

    Column::new()
        .push(col_address)
        .push(col_auth)
        .spacing(50)
        .into()
}

pub fn define_electrum<'a>(address: &form::Value<String>) -> Element<'a, Message> {
    let col_address = Column::new()
        .push(text("Address:").bold())
        .push(
            form::Form::new_trimmed("127.0.0.1:50001", address, |msg| {
                Message::DefineNode(DefineNode::DefineElectrum(
                    message::DefineElectrum::ConfigFieldEdited(electrum::ConfigField::Address, msg),
                ))
            })
            .warning(
                "Please enter correct address (including port), \
                optionally prefixed with tcp:// or ssl://",
            )
            .size(text::P1_SIZE)
            .padding(10),
        )
        .push(text(electrum::ADDRESS_NOTES).size(text::P2_SIZE))
        .spacing(10);

    Column::new().push(col_address).spacing(50).into()
}

pub fn select_bitcoind_type<'a>(progress: (usize, usize)) -> Element<'a, Message> {
    layout(
        progress,
        None,
        "Bitcoin node management",
        Column::new()
            .push(
                Row::new()
                    .align_y(Alignment::Start)
                    .spacing(20)
                    .push(
                        Container::new(
                            Column::new()
                                .spacing(20)
                                .width(Length::Fixed(300.0))
                                .push(text("I already have a node").bold()),
                        )
                        .padding(20),
                    )
                    .push(
                        Container::new(
                            Column::new().spacing(20).width(Length::Fixed(300.0)).push(
                                text(
                                    "I want Liana to automatically install \
                                    a Bitcoin node on my device",
                                )
                                .bold(),
                            ),
                        )
                        .padding(20),
                    ),
            )
            .push(
                Row::new()
                    .align_y(Alignment::Start)
                    .spacing(20)
                    .push(
                        Container::new(
                            Column::new()
                                .spacing(20)
                                .width(Length::Fixed(300.0))
                                .align_x(Alignment::Start)
                                .push(text(
                                    "Select this option if you already have \
                                    a Bitcoin node running locally or remotely. \
                                    Liana will connect to it.",
                                )),
                        )
                        .padding(20),
                    )
                    .push(
                        Container::new(
                            Column::new()
                                .spacing(20)
                                .width(Length::Fixed(300.0))
                                .align_x(Alignment::Start)
                                .push(text(
                                    "Liana will install a pruned node \
                                    on your computer. You won't need to do anything \
                                    except have some disk space available \
                                    (~30GB required on mainnet) and \
                                    wait for the initial synchronization with the \
                                    network (it can take some days depending on \
                                    your internet connection speed).",
                                )),
                        )
                        .padding(20),
                    ),
            )
            .push(
                Row::new()
                    .align_y(Alignment::End)
                    .spacing(20)
                    .push(
                        Container::new(
                            Column::new()
                                .spacing(20)
                                .width(Length::Fixed(300.0))
                                .align_x(Alignment::Center)
                                .push(
                                    button::secondary(None, "Select")
                                        .width(Length::Fixed(300.0))
                                        .on_press(Message::SelectBitcoindType(
                                            message::SelectBitcoindTypeMsg::UseExternal(true),
                                        )),
                                ),
                        )
                        .padding(20),
                    )
                    .push(
                        Container::new(
                            Column::new()
                                .spacing(20)
                                .width(Length::Fixed(300.0))
                                .align_x(Alignment::Center)
                                .push(
                                    button::secondary(None, "Select")
                                        .width(Length::Fixed(300.0))
                                        .on_press(Message::SelectBitcoindType(
                                            message::SelectBitcoindTypeMsg::UseExternal(false),
                                        )),
                                ),
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
    let version = crate::node::bitcoind::VERSION;
    layout(
        progress,
        None,
        "Start Bitcoin full node",
        Column::new()
            .push_maybe(download_state.map(|s| {
                match s {
                    DownloadState::Finished(_) => Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(icon::circle_check_icon().style(theme::text::success))
                        .push(text("Download complete").style(theme::text::success)),
                    DownloadState::Downloading { progress } => Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(text(format!(
                            "Downloading Bitcoin Core {version}... {progress:.2}%"
                        ))),
                    DownloadState::Errored(e) => Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(icon::circle_cross_icon().style(theme::text::error))
                        .push(text(format!("Download failed: '{}'.", e)).style(theme::text::error)),
                    _ => Row::new().spacing(10).align_y(Alignment::Center),
                }
            }))
            .push(Container::new(if let Some(state) = install_state {
                match state {
                    InstallState::InProgress => Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push("Installing bitcoind..."),
                    InstallState::Finished => Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(icon::circle_check_icon().style(theme::text::success))
                        .push(text("Installation complete").style(theme::text::success)),
                    InstallState::Errored(e) => Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(icon::circle_cross_icon().style(theme::text::error))
                        .push(
                            text(format!("Installation failed: '{}'.", e))
                                .style(theme::text::error),
                        ),
                }
            } else if exe_path.is_some() {
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(icon::circle_check_icon().style(theme::text::success))
                    .push(
                        text("Liana-managed bitcoind already installed")
                            .style(theme::text::success),
                    )
            } else if let Some(DownloadState::Downloading { progress }) = download_state {
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(progress_bar(0.0..=100.0, *progress))
            } else {
                Row::new().spacing(10).align_y(Alignment::Center)
            }))
            .push_maybe(if started.is_some() {
                started.map(|res| {
                    if res.is_ok() {
                        Container::new(
                            Row::new()
                                .spacing(10)
                                .align_y(Alignment::Center)
                                .push(icon::circle_check_icon().style(theme::text::success))
                                .push(text("Started").style(theme::text::success)),
                        )
                    } else {
                        Container::new(
                            Row::new()
                                .spacing(10)
                                .align_y(Alignment::Center)
                                .push(icon::circle_cross_icon().style(theme::text::error))
                                .push(
                                    text(res.as_ref().err().unwrap().to_string())
                                        .style(theme::text::error),
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
                            .align_y(Alignment::Center)
                            .push(text("Starting...")),
                    )),
                    _ => Some(Container::new(Space::with_height(Length::Fixed(25.0)))),
                }
            })
            .spacing(50)
            .push(
                Row::new().push(
                    button::secondary(None, "Next")
                        .width(Length::Fixed(200.0))
                        .on_press_maybe(if let Some(Ok(_)) = started {
                            Some(Message::Next)
                        } else {
                            None
                        }),
                ),
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
    installed: bool,
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
            } else if installed {
                Container::new(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(icon::circle_check_icon().style(theme::text::success))
                        .push(text("Installed").style(theme::text::success)),
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

pub fn defined_threshold<'a>(
    color: iced::Color,
    fixed: bool,
    threshold: (usize, usize),
) -> Element<'a, message::DefinePath> {
    if !fixed && threshold.1 > 1 {
        Button::new(
            Row::new()
                .spacing(10)
                .push((0..threshold.1).fold(Row::new(), |row, i| {
                    if i < threshold.0 {
                        row.push(icon::round_key_icon().color(color))
                    } else {
                        row.push(icon::round_key_icon())
                    }
                }))
                .push(text(format!(
                    "{} out of {} key{}",
                    threshold.0,
                    threshold.1,
                    if threshold.1 > 1 { "s" } else { "" },
                )))
                .push(icon::pencil_icon()),
        )
        .padding(10)
        .on_press(message::DefinePath::EditThreshold)
        .style(theme::button::secondary)
        .into()
    } else {
        card::simple(
            Row::new()
                .spacing(10)
                .push((0..threshold.1).fold(Row::new(), |row, i| {
                    if i < threshold.0 {
                        row.push(icon::round_key_icon().color(color))
                    } else {
                        row.push(icon::round_key_icon())
                    }
                }))
                .push(text(format!(
                    "{} out of {} key{}",
                    threshold.0,
                    threshold.1,
                    if threshold.1 > 1 { "s" } else { "" },
                ))),
        )
        .padding(10)
        .into()
    }
}

pub fn defined_sequence<'a>(
    sequence: PathSequence,
    warning: Option<PathWarning>,
) -> Element<'a, message::DefinePath> {
    let (n_years, n_months, n_days, n_hours, n_minutes) = duration_from_sequence(sequence.as_u16());
    let duration_row = Row::new()
        .padding(5)
        .spacing(5)
        .align_y(Alignment::Center)
        .push(text(
            [
                (n_years, "y"),
                (n_months, "m"),
                (n_days, "d"),
                (n_hours, "h"),
                (n_minutes, "mn"),
            ]
            .iter()
            .filter_map(|(n, unit)| {
                if *n > 0 {
                    Some(format!("{}{}", n, unit))
                } else {
                    None
                }
            })
            .collect::<Vec<String>>()
            .join(" "),
        ));
    Container::new(
        Column::new()
            .spacing(5)
            .push(match sequence {
                PathSequence::Recovery(_) => Row::new().align_y(Alignment::Center).push(
                    Container::new(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(5)
                            .push(
                                text::p1_regular("Available after inactivity of ~")
                                    .style(theme::text::secondary),
                            )
                            .push(
                                Button::new(duration_row.push(icon::pencil_icon()))
                                    .style(theme::button::secondary)
                                    .on_press(message::DefinePath::EditSequence),
                            ),
                    )
                    .width(Length::Fill)
                    .padding(5)
                    .align_y(alignment::Vertical::Center),
                ),
                PathSequence::Primary => Row::new()
                    .push(
                        p1_regular("Able to move the funds at any time.")
                            .style(theme::text::secondary),
                    )
                    .padding(5),
                PathSequence::SafetyNet => Row::new().align_y(Alignment::Center).push(
                    Container::new(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(5)
                            .push(
                                text::p1_regular("Available after inactivity of ~")
                                    .style(theme::text::secondary),
                            )
                            .push(duration_row),
                    )
                    .width(Length::Fill)
                    .padding(5)
                    .align_y(alignment::Vertical::Center),
                ),
            })
            .push_maybe(warning.map(|w| text(w.message()).small().style(theme::text::error)))
            .spacing(15),
    )
    .padding(5)
    .into()
}

#[allow(clippy::too_many_arguments)]
pub fn hw_list_view<'a>(
    i: usize,
    hw: &'a HardwareWallet,
    chosen: bool,
    processing: bool,
    selected: bool,
    descriptor: Option<&'a LianaDescriptor>,
    device_must_support_taproot: bool,
    accounts: Option<&HashMap<Fingerprint, ChildNumber>>,
    display_account: bool,
) -> Element<'a, Message> {
    let mut unrelated = false;
    let mut bttn = Button::new(match hw {
        HardwareWallet::Supported {
            kind,
            version,
            fingerprint,
            alias,
            ..
        } => {
            let device_in_descriptor = descriptor
                .map(|d| d.contains_fingerprint(*fingerprint))
                .unwrap_or(true);
            let not_tapminiscript = device_must_support_taproot
                && !is_compatible_with_tapminiscript(kind, version.as_ref());
            if !device_in_descriptor {
                unrelated = true;
                hw::unrelated_hardware_wallet(kind.to_string(), version.as_ref(), fingerprint)
            } else if chosen && processing {
                hw::processing_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            } else if selected {
                let acc = accounts
                    .as_ref()
                    .and_then(|map| map.get(fingerprint).cloned());
                hw::selected_hardware_wallet(
                    kind,
                    version.as_ref(),
                    fingerprint,
                    alias.as_ref(),
                    {
                        if not_tapminiscript {
                            Some("Device firmware version does not support taproot miniscript")
                        } else {
                            None
                        }
                    },
                    acc,
                    display_account,
                )
            } else if not_tapminiscript {
                hw::warning_hardware_wallet(
                    kind,
                    version.as_ref(),
                    fingerprint,
                    alias.as_ref(),
                    "Device firmware version does not support taproot miniscript",
                )
            } else if let Some(accounts) = accounts {
                hw::supported_hardware_wallet_with_account(
                    kind,
                    version.as_ref(),
                    *fingerprint,
                    alias.as_ref(),
                    accounts.get(fingerprint).cloned(),
                    true,
                )
            } else {
                hw::supported_hardware_wallet(kind, version.as_ref(), *fingerprint, alias.as_ref())
            }
        }
        HardwareWallet::Unsupported {
            version,
            kind,
            reason,
            ..
        } => match reason {
            UnsupportedReason::NotPartOfWallet(fg) => {
                hw::unrelated_hardware_wallet(kind.to_string(), version.as_ref(), fg)
            }
            UnsupportedReason::WrongNetwork => {
                hw::wrong_network_hardware_wallet(kind.to_string(), version.as_ref())
            }
            UnsupportedReason::Version {
                minimal_supported_version,
            } => hw::unsupported_version_hardware_wallet(
                kind.to_string(),
                version.as_ref(),
                minimal_supported_version,
            ),
            _ => hw::unsupported_hardware_wallet(kind.to_string(), version.as_ref()),
        },
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => hw::locked_hardware_wallet(kind, pairing_code.as_ref()),
    })
    .style(theme::button::secondary)
    .width(Length::Fill);
    if !processing && hw.is_supported() && !unrelated {
        bttn = bttn.on_press(Message::Select(i));
    }
    bttn.into()
}

#[allow(clippy::too_many_arguments)]
pub fn key_list_view<'a>(
    i: usize,
    name: &'a str,
    fingerprint: &'a Fingerprint,
    kind: Option<&'a DeviceKind>,
    version: Option<&'a async_hwi::Version>,
    chosen: bool,
    device_must_support_taproot: bool,
    accounts: &HashMap<Fingerprint, ChildNumber>,
) -> Element<'a, Message> {
    let account = accounts.get(fingerprint).copied();
    Button::new(if chosen {
        hw::selected_hardware_wallet(
            kind.map(|k| k.to_string()).unwrap_or_default(),
            version,
            fingerprint,
            Some(name),
            if device_must_support_taproot
                && kind.map(|kind| is_compatible_with_tapminiscript(kind, version)) == Some(false)
            {
                Some("Device firmware version does not support taproot miniscript")
            } else {
                None
            },
            account,
            true,
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
        hw::supported_hardware_wallet_with_account(
            kind.map(|k| k.to_string()).unwrap_or_default(),
            version,
            *fingerprint,
            Some(name),
            account,
            false,
        )
    })
    .style(theme::button::secondary)
    .width(Length::Fill)
    .on_press(Message::DefineDescriptor(
        message::DefineDescriptor::KeyModal(message::ImportKeyModal::SelectKey(i)),
    ))
    .into()
}

pub fn provider_key_list_view(i: Option<usize>, key: &Key, chosen: bool) -> Element<'_, Message> {
    // If `i.is_some()`, it means this key is in our list of (saved) keys and can be selected.
    let key_kind = key
        .source
        .provider_key_kind()
        .expect("has kind")
        .to_string();
    let token = key.source.token().expect("has token");
    Button::new(if i.is_some() {
        if chosen {
            hw::selected_provider_key(key.fingerprint, key.name.clone(), key_kind, token)
        } else {
            hw::unselected_provider_key(key.fingerprint, key.name.clone(), key_kind, token)
        }
    } else {
        hw::unsaved_provider_key(key.fingerprint, key_kind, token)
    })
    .style(theme::button::secondary)
    .width(Length::Fill)
    .on_press_maybe(i.map(|i| {
        Message::DefineDescriptor(message::DefineDescriptor::KeyModal(
            message::ImportKeyModal::SelectKey(i),
        ))
    }))
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
                                .align_y(Alignment::End)
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
                button::secondary(None, "Next")
                    .on_press(Message::Next)
                    .width(Length::Fixed(200.0))
            } else {
                button::secondary(None, "Next").width(Length::Fixed(200.0))
            })
            .push(Space::with_height(20.0))
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
                        .align_x(Alignment::Center)
                        .push(
                            Container::new(if !suggestions.is_empty() {
                                suggestions.iter().fold(Row::new().spacing(5), |row, sugg| {
                                    row.push(
                                        Button::new(text(sugg))
                                            .style(theme::button::secondary)
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
                                        .align_y(Alignment::Center)
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
                                            Some(
                                                icon::circle_check_icon()
                                                    .style(theme::text::success),
                                            )
                                        } else {
                                            None
                                        }),
                                )
                            },
                        ))
                        .push(Space::with_height(Length::Fixed(50.0)))
                        .push_maybe(
                            error.map(|e| card::invalid(text(e).style(theme::text::error))),
                        ),
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
                        button::secondary(None, "Skip")
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
                            button::secondary(None, "Next").width(Length::Fixed(200.0))
                        } else {
                            button::secondary(None, "Next")
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

pub fn choose_backend(progress: (usize, usize)) -> Element<'static, Message> {
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
                            .width(Length::FillPortion(1))
                            .push(h3("Use your own node"))
                            .push(text::p2_medium(LOCAL_WALLET_DESC).style(theme::text::secondary)),
                    )
                    .push(
                        Column::new()
                            .spacing(20)
                            .width(Length::FillPortion(1))
                            .push(h3("Use Liana Connect"))
                            .push(
                                text::p2_medium(REMOTE_BACKEND_DESC).style(theme::text::secondary),
                            ),
                    ),
            )
            .push(
                Row::new()
                    .spacing(20)
                    .push(
                        Container::new(
                            button::secondary(None, "Select")
                                .on_press(Message::SelectBackend(
                                    message::SelectBackend::ContinueWithLocalWallet(true),
                                ))
                                .width(Length::Fixed(200.0)),
                        )
                        .width(Length::FillPortion(1)),
                    )
                    .push(
                        Container::new(
                            button::secondary(None, "Select")
                                .on_press(Message::SelectBackend(
                                    message::SelectBackend::ContinueWithLocalWallet(false),
                                ))
                                .width(Length::Fixed(200.0)),
                        )
                        .width(Length::FillPortion(1)),
                    ),
            )
            .spacing(20),
        true,
        Some(Message::Previous),
    )
}

pub fn login(progress: (usize, usize), connection_step: Element<Message>) -> Element<Message> {
    layout(
        progress,
        None,
        "Login",
        Container::new(
            Column::new()
                .spacing(50)
                .max_width(700)
                .align_x(Alignment::Center)
                .width(Length::FillPortion(1))
                .push(h2("Liana Connect"))
                .push(connection_step),
        )
        .center_x(Length::Fill),
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
        .push_maybe(connection_error.map(|e| text(e.to_string()).style(theme::text::warning)))
        .push_maybe(auth_error.map(|e| text(e.to_string()).style(theme::text::warning)))
        .push(text(
            "Enter the email you want to associate with the wallet:",
        ))
        .push(
            form::Form::new_trimmed("email", email, |msg| {
                Message::SelectBackend(message::SelectBackend::EmailEdited(msg))
            })
            .size(text::P1_SIZE)
            .padding(10)
            .warning("Email is not valid"),
        )
        .push(
            button::secondary(None, "Next")
                .on_press_maybe(if processing || !email.valid {
                    None
                } else {
                    Some(Message::SelectBackend(message::SelectBackend::RequestOTP))
                })
                .width(Length::Fixed(200.0)),
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
        .push(text(email).style(theme::text::success))
        .push(text("An authentication token has been emailed to you"))
        .push_maybe(connection_error.map(|e| text(e.to_string()).style(theme::text::warning)))
        .push_maybe(auth_error.map(|e| text(e.to_string()).style(theme::text::warning)))
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
                    button::secondary(Some(icon::previous_icon()), "Change Email")
                        .on_press(Message::SelectBackend(message::SelectBackend::EditEmail)),
                )
                .push(
                    button::secondary(None, "Resend token").on_press_maybe(if processing {
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
        .push(text(email).style(theme::text::success))
        .push_maybe(connection_error.map(|e| text(e.to_string()).style(theme::text::warning)))
        .push_maybe(auth_error.map(|e| text(e.to_string()).style(theme::text::warning)))
        .push(Container::new(
            Row::new()
                .spacing(10)
                .push(
                    button::secondary(Some(icon::previous_icon()), "Change Email")
                        .on_press(Message::SelectBackend(message::SelectBackend::EditEmail)),
                )
                .push(
                    button::secondary(None, "Continue").on_press_maybe(if processing {
                        None
                    } else {
                        Some(Message::Next)
                    }),
                ),
        ))
        .into()
}

pub const REMOTE_BACKEND_DESC: &str = "Use our service to instantly be ready to transact. Wizardsardine runs the infrastructure, allowing multiple computers or participants to connect and synchronize.\n\nThis is a simpler and safer option for people who want Wizardsardine to keep a backup of their descriptor. You are still in control of your keys, and Wizardsardine does not have any control over your funds, but it will be able to see your wallet's information, associated to an email address. Privacy focused users should run their own infrastructure instead.";

pub const LOCAL_WALLET_DESC: &str = "Use your already existing Bitcoin node or automatically install one. The Liana wallet will not connect to any external server.\n\nThis is the most private option, but the data is locally stored on this computer, only. You must perform your own backups, and share the descriptor with other people you want to be able to access the wallet";

pub fn wallet_alias<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    wallet_alias: &form::Value<String>,
) -> Element<'a, Message> {
    layout(
        progress,
        email,
        "Give your wallet an alias",
        Column::new()
            .push(
                Column::new()
                    .spacing(20)
                    .push(p1_bold("Wallet alias:"))
                    .push(
                        form::Form::new("Wallet alias", wallet_alias, Message::WalletAliasEdited)
                            .warning("Wallet alias is too long.")
                            .size(text::P1_SIZE)
                            .padding(10),
                    )
                    .push(p2_regular(
                        "You will be able to change it later in Settings > Wallet",
                    )),
            )
            .push(
                button::secondary(None, "Next")
                    .width(Length::Fixed(200.0))
                    .on_press_maybe(if wallet_alias.valid {
                        Some(Message::Next)
                    } else {
                        None
                    }),
            )
            .spacing(50),
        true,
        Some(Message::Previous),
    )
}

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
            .push(
                Row::new()
                    .push(Space::with_width(Length::Fill))
                    .push_maybe(email.map(|e| {
                        Container::new(p1_regular(e).style(theme::text::success)).padding(20)
                    })),
            )
            .push(Space::with_height(Length::Fixed(100.0)))
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(Container::new(prev_button).center_x(Length::FillPortion(2)))
                    .push(Container::new(h3(title)).width(Length::FillPortion(8)))
                    .push_maybe(if progress.1 > 0 {
                        Some(
                            Container::new(text(format!("{} | {}", progress.0, progress.1)))
                                .center_x(Length::FillPortion(2)),
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
    .center_x(Length::Fill)
    .height(Length::Fill)
    .width(Length::Fill)
    .style(theme::container::background)
    .into()
}
