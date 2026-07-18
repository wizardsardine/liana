pub mod editor;

use async_hwi::utils::extract_keys_and_template;
use iced::{
    alignment,
    widget::{checkbox, column, progress_bar, radio, row, tooltip, Button, Space, TextInput},
    Alignment, Length,
};

use liana::{
    descriptors::{LianaDescriptor, LianaPolicy},
    miniscript::bitcoin::{
        self,
        bip32::{ChildNumber, Fingerprint},
        Network,
    },
};
use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, Ipv6Addr},
    path::PathBuf,
    str::FromStr,
};

use liana_ui::{
    component::{
        badge::Tile,
        button::{
            self, btn_accept, btn_backend_options_help, btn_backup_descriptor,
            btn_check_connection, btn_next, btn_select, EntryWidth,
        },
        card, form, installer as installer_layout,
        list::{self, DeviceStatus, EntryAccent},
        modal, scrollable, separation,
        text::{self, new, text, Text as _},
    },
    icon, theme,
    widget::*,
    Variant,
};

use crate::node::electrum::validate_domain_checkbox;
use crate::{
    app::settings,
    help,
    hw::HardwareWallet,
    installer::{
        descriptor::{PathSequence, PathWarning},
        message::{self, DefineBitcoind, DefineNode, Message},
        prompt,
        step::{DownloadState, InstallState},
        view::editor::format_sequence_duration,
        Error,
    },
    node::{
        bitcoind::{ConfigField, RpcAuthType, RpcAuthValues, StartInternalBitcoindError},
        electrum, NodeType,
    },
};

#[allow(clippy::too_many_arguments)]
pub fn import_wallet_or_descriptor<'a>(
    progress: (usize, usize),
    network: Network,
    email: Option<&'a str>,
    invitation: &'a form::Value<String>,
    invitation_wallet: Option<&'a str>,
    imported_descriptor: &'a form::Value<String>,
    error: Option<&'a String>,
    options_expanded: bool,
    active_option: Option<message::ImportWalletOption>,
    wallets: Vec<(&'a String, Option<&'a String>)>,
) -> Element<'a, Message> {
    // Error banner
    let error = error.map(|e| card::error("Something wrong happened", e.to_string()));

    // Wallet list
    let title = row![
        new::h3_semi("Load a previously used wallet"),
        Space::fill_width()
    ];
    let no_wallets = wallets.is_empty();
    let wallet_accent = Some(match network {
        Network::Bitcoin => EntryAccent::Bitcoin,
        _ => EntryAccent::Testnet,
    });
    let wallets: Element<'a, Message> = if no_wallets {
        Container::new(new::caption("You have no current wallets"))
            .center_x(Length::Fill)
            .into()
    } else {
        wallets
            .into_iter()
            .enumerate()
            .fold(column![].spacing(20), |wallets, (i, (name, alias))| {
                let title = alias.map_or(name.as_str(), |alias| alias.as_str());
                let subtitle =
                    alias.map(|_| new::caption(name).style(theme::text::secondary).into());
                wallets.push(list::entry_wallet(
                    wallet_accent,
                    title,
                    subtitle,
                    None,
                    None,
                    Some(Message::Select(i)),
                ))
            })
            .into()
    };
    let previous_wallets = column![title, wallets].spacing(20);

    // Invitation entry
    let fetch_invitation = (!invitation.value.is_empty()).then_some(Message::ImportRemoteWallet(
        message::ImportRemoteWallet::FetchInvitation,
    ));
    let invitation_token_msg =
        |msg| Message::ImportRemoteWallet(message::ImportRemoteWallet::ImportInvitationToken(msg));
    let invitation_form = row![
        form::Form::new_trimmed("Invitation token", invitation, invitation_token_msg)
            .warning("Invitation token is invalid or expired"),
        btn_next(fetch_invitation),
    ]
    .align_y(Alignment::Start)
    .spacing(10);
    let button_accept = btn_accept(Some(Message::ImportRemoteWallet(
        message::ImportRemoteWallet::AcceptInvitation,
    )));
    let accept_invitation = |wallet: &'a str| {
        row![
            Space::with_width(15),
            new::caption("Accept invitation for wallet:"),
            new::b5_bold(wallet),
            Space::fill_width(),
            button_accept
        ]
        .align_y(Alignment::Center)
        .spacing(5)
        .into()
    };
    let invitation_content: Element<'a, Message> = if let Some(wallet) = invitation_wallet {
        accept_invitation(wallet)
    } else {
        invitation_form.into()
    };
    let invitation = list::entry_collapsible(list::CollapsibleEntry {
        accent: wallet_accent,
        tile: Tile::Import,
        title: "Load a shared wallet",
        collapsed_subtitle: Some("If you received an invitation to join a shared wallet"),
        expanded_subtitle: Some("Type the invitation token you received by email"),
        content: invitation_content,
        expanded: active_option == Some(message::ImportWalletOption::Invitation),
        on_toggle: Message::ImportRemoteWallet(message::ImportRemoteWallet::ToggleOption(
            message::ImportWalletOption::Invitation,
        )),
    });

    // Import a descriptor entry
    let import_descriptor = list::entry_action_accent(
        wallet_accent,
        Tile::Import,
        "Import a descriptor",
        None::<String>,
        None,
        button::EntryWidth::Standard,
        Some(Message::ImportRemoteWallet(
            message::ImportRemoteWallet::ImportDescriptorFromFile,
        )),
    );

    // Paste a descriptor entry
    let confirm_descriptor = (!imported_descriptor.value.is_empty() && imported_descriptor.valid)
        .then_some(Message::ImportRemoteWallet(
            message::ImportRemoteWallet::ConfirmDescriptor,
        ));
    let descriptor_form = row![
        form::Form::new_trimmed("Descriptor", imported_descriptor, |msg| {
            Message::ImportRemoteWallet(message::ImportRemoteWallet::ImportDescriptor(msg))
        })
        .warning("Either descriptor is invalid or incompatible with network"),
        btn_next(confirm_descriptor),
    ]
    .align_y(Alignment::Start)
    .spacing(10);
    let descriptor_content = column![Space::with_height(0), descriptor_form,].spacing(10);
    let paste_descriptor = list::entry_collapsible(list::CollapsibleEntry {
        accent: wallet_accent,
        tile: Tile::Paste,
        title: "Paste a descriptor",
        collapsed_subtitle: Some("Creates a new wallet from the pasted descriptor"),
        expanded_subtitle: Some("Creates a new wallet from the pasted descriptor"),
        content: descriptor_content.into(),
        expanded: active_option == Some(message::ImportWalletOption::PasteDescriptor),
        on_toggle: Message::ImportRemoteWallet(message::ImportRemoteWallet::ToggleOption(
            message::ImportWalletOption::PasteDescriptor,
        )),
    });

    // Other options block
    let other_options_header = row![
        modal::optional_section(
            options_expanded,
            "Other options".to_string(),
            || Message::ImportRemoteWallet(message::ImportRemoteWallet::ToggleOptions(true)),
            || Message::ImportRemoteWallet(message::ImportRemoteWallet::ToggleOptions(false)),
        ),
        Space::fill_width(),
    ];
    let other_options_content: Option<Element<'a, Message>> = options_expanded.then_some(
        column![invitation, import_descriptor, paste_descriptor,]
            .spacing(20)
            .into(),
    );
    let other_options = column![other_options_header, other_options_content].spacing(20);

    let content = column![
        error,
        previous_wallets,
        other_options,
        Space::with_height(10),
    ]
    .width(EntryWidth::Standard)
    .align_x(Alignment::Center)
    .spacing(20);

    let content = Container::new(content).center_x(Length::Fill);

    layout(
        progress,
        network,
        email,
        "Add wallet",
        content,
        Some(Message::Previous),
    )
}

pub fn import_descriptor<'a>(
    progress: (usize, usize),
    network: Network,
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
                .push(button::primary(None, "Import backup").on_press(Message::ImportBackup))
                .push(Space::with_width(Length::Fill)),
        )
    } else {
        None
    };

    let backup_imported = if imported_backup {
        Some(
            Row::new()
                .push(text("Backup successfully imported!").bold())
                .push(Space::with_width(Length::Fill)),
        )
    } else {
        None
    };

    layout(
        progress,
        network,
        email,
        "Import the wallet",
        Column::new()
            .push(
                Column::new()
                    .spacing(20)
                    .push_maybe(import_backup)
                    .push_maybe(backup_imported)
                    .push_maybe(or)
                    .push_maybe(descriptor)
                    .push(text(
                        "If you are using a Bitcoin Core node, \
                you will need to perform a rescan of \
                the blockchain after creating the wallet \
                in order to see your coins and past \
                transactions. This can be done in \
                Settings > Node.",
                    )),
            )
            .push(btn_next(valid.then_some(Message::Next)))
            .push_maybe(error.map(|e| card::error("Invalid descriptor", e.to_string())))
            .spacing(50),
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
                        checkbox(did_backup)
                            .label("I have backed up the mnemonic, show the extended public key")
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
                                Container::new(scrollable::horizontal_thin(
                                    Container::new(text(xpub).small()).padding(10),
                                ))
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
    let select_msg = (!processing && hw.is_supported()).then_some(Message::Select(i));
    let bttn: Element<'a, Message> = match hw {
        HardwareWallet::Supported {
            kind,
            fingerprint,
            alias,
            ..
        } => {
            if processing {
                modal::device_entry(
                    Some(format!("#{fingerprint}")),
                    Some(kind),
                    alias.as_ref(),
                    DeviceStatus::Processing,
                    None,
                )
            } else {
                modal::account_device_entry(
                    *fingerprint,
                    Some(kind),
                    alias.as_ref(),
                    accounts.get(fingerprint).cloned(),
                    select_msg,
                )
            }
        }
        _ => crate::view::hw::unusable_device_entry(hw),
    };
    Container::new(
        Column::new()
            .push_maybe(error.map(|e| card::legacy_warning(e.to_string()).width(Length::Fill)))
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
                                Container::new(scrollable::horizontal_thin(
                                    Container::new(text(xpub).small()).padding(10),
                                ))
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
    network: Network,
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
        .push(new::b5_bold(
            "Import an extended public key by selecting a signing device:",
        ))
        .push(Space::with_width(10))
        .push(info)
        .push(Space::with_width(Length::Fill));
    layout(
        (0, 0),
        network,
        email,
        "Share your public keys (Xpubs)",
        column![
            title,
            if hws.is_empty() {
                modal::modal_no_devices_placeholder()
            } else {
                Column::with_children(hws).spacing(10).into()
            },
            Container::new(new::b5_bold("Or create a new random key:")).width(Length::Fill),
            signer,
            Space::with_height(10),
        ]
        .spacing(10)
        .width(Length::Fill),
        Some(Message::Previous),
    )
}

pub fn policy_entry_card(title: String, content: String) -> Container<'static, Message> {
    let title = new::b5_bold(title);
    let scroll = scrollable::horizontal_thin(column![new::caption(content)]);
    card::simple(column![title, scroll].spacing(10)).width(Length::Fill)
}

pub fn policy_view(template: String, keys: Vec<String>) -> Element<'static, Message> {
    let template = policy_entry_card("Descriptor template".into(), template);
    let mut col = column![template].spacing(5);

    for (index, key) in keys.into_iter().enumerate() {
        let title = format!("Key @{index}:");
        col = col.push(policy_entry_card(title, key));
    }
    col.into()
}

pub fn descriptor_view(descriptor_str: String) -> Element<'static, Message> {
    policy_entry_card("The descriptor".into(), descriptor_str).into()
}

#[allow(clippy::too_many_arguments)]
pub fn register_descriptor<'a>(
    progress: (usize, usize),
    network: Network,
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
            policy_view(template, keys)
        } else {
            descriptor_view(descriptor_str)
        };

    let warning = (!created_desc).then_some(new::b5_bold(
        "This step is only necessary if you are using a signing device.",
    ));
    let error_card = error.map(|e| card::error("Failed to register descriptor", e.to_string()));

    let devices_title = Container::new(if created_desc {
        new::b5_bold("Select hardware wallet to register descriptor on:")
    } else {
        new::b5_bold("If necessary, please select the signing device to register descriptor on:")
    })
    .width(Length::Fill);
    let devices: Element<'a, Message> = if hws.is_empty() {
        modal::modal_no_devices_placeholder()
    } else {
        Column::with_children(hws.iter().enumerate().map(|(i, hw)| {
            let entry = crate::view::hw::device_list_entry(
                hw,
                crate::view::hw::HwRowMode::Registration {
                    chosen: Some(i) == chosen_hw,
                    processing,
                    complete: hw
                        .fingerprint()
                        .map(|fg| registered.contains(&fg))
                        .unwrap_or(false),
                    descriptor: Some(descriptor),
                    device_must_support_taproot: false,
                },
                move || Message::Select(i),
            );
            Container::new(entry).width(EntryWidth::Standard).into()
        }))
        .spacing(10)
        .into()
    };
    let signing_devices = column![devices_title, devices]
        .align_x(Alignment::Center)
        .spacing(10)
        .width(EntryWidth::Standard);
    let signing_devices = Container::new(signing_devices).center_x(Length::Fill);

    let registered_checkbox = created_desc.then_some(
        checkbox(done)
            .label("I have registered the descriptor on my device(s)")
            .on_toggle(Message::UserActionDone),
    );

    let next = (!created_desc || (done && !processing)).then_some(Message::Next);
    let next_button = row![Space::fill_width(), btn_next(next)];

    let help = new::caption(prompt::REGISTER_DESCRIPTOR_HELP);

    let content = column![
        warning,
        displayed_descriptor,
        help,
        error_card,
        signing_devices,
        registered_checkbox,
        next_button,
        Space::with_height(5),
    ]
    .spacing(20);

    let previous = (!processing).then_some(Message::Previous);

    layout(
        progress,
        network,
        email,
        "Register descriptor",
        content,
        previous,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn backup_descriptor<'a>(
    progress: (usize, usize),
    network: Network,
    email: Option<&'a str>,
    descriptor: &'a LianaDescriptor,
    keys: &'a HashMap<Fingerprint, settings::KeySetting>,
    error: Option<&Error>,
    done: bool,
    help_open: bool,
) -> Element<'a, Message> {
    let help_button = modal::optional_section(
        help_open,
        "Learn more".to_string(),
        || Message::ShowBackupDescriptorHelp(true),
        || Message::ShowBackupDescriptorHelp(false),
    );
    let help = help_open.then_some(text::new::caption(prompt::BACKUP_DESCRIPTOR_HELP));
    let intro = column![
        text::new::caption(prompt::BACKUP_DESCRIPTOR_MESSAGE),
        help_button,
        help,
    ];

    let error_card = error.map(|e| card::error("Failed to export backup", e.to_string()));

    let descriptor_str = descriptor.to_string();

    let backup_button = btn_backup_descriptor(Some(Message::BackupDescriptor), !done);
    let copy_button = column![
        button::btn_copy(Some(Message::Clipboard(descriptor_str.clone()))),
        Space::with_height(10)
    ];
    let descriptor_scroll =
        scrollable::horizontal_thin(text::new::caption(descriptor_str)).width(Length::Fill);
    let descriptor_actions = row![Space::fill_width(), backup_button];
    let descriptor_header = row![descriptor_scroll, copy_button]
        .align_y(Alignment::Center)
        .spacing(10);
    let descriptor_card = card::simple(
        column![
            text::new::b5_bold("The descriptor:"),
            descriptor_header,
            descriptor_actions,
        ]
        .spacing(10),
    );

    let policy_card = card::simple(display_policy(descriptor.policy(), keys)).width(Length::Fill);

    let backup_checkbox = checkbox(done)
        .label("I have backed up my descriptor")
        .on_toggle(Message::UserActionDone);

    let button_next = btn_next(done.then_some(Message::Next));
    let row_next = row![Space::fill_width(), button_next];

    let content = column![
        intro,
        error_card,
        descriptor_card,
        policy_card,
        backup_checkbox,
        row_next,
        Space::with_height(20),
    ]
    .spacing(50);

    layout(
        progress,
        network,
        email,
        "Back Up your wallet configuration (Descriptor)",
        content,
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

    let primary_signature = new::b5_bold(format!(
        "{} signature{}",
        primary_threshold,
        if primary_threshold > 1 { "s" } else { "" }
    ));
    let primary_key_count = if primary_keys.len() > 1 {
        new::caption(format!("out of {} by", primary_keys.len()))
    } else {
        new::caption("by")
    };
    let primary_key_list =
        primary_keys
            .iter()
            .enumerate()
            .fold(Row::new().spacing(5), |row, (i, k)| {
                let content = if let Some(key) = keys.get(k) {
                    Container::new(
                        tooltip::Tooltip::new(
                            new::b5_bold(key.name.clone()),
                            new::caption(k.to_string()),
                            tooltip::Position::Bottom,
                        )
                        .style(theme::card::simple),
                    )
                } else {
                    Container::new(new::b5_bold(format!("[{k}]")))
                };
                if primary_keys.len() == 1 || i == primary_keys.len() - 1 {
                    row.push(content)
                } else if i <= primary_keys.len() - 2 {
                    row.push(content).push(new::caption("and"))
                } else {
                    row.push(content).push(new::caption(","))
                }
            });
    let primary_row = row![
        primary_signature,
        primary_key_count,
        primary_key_list,
        new::caption("can always spend this wallet's funds (Primary path)"),
    ]
    .spacing(5);

    let mut col = column![primary_row];
    for (i, (sequence, recovery_path)) in recovery_paths.iter().enumerate() {
        let (threshold, recovery_keys) = recovery_path.thresh_origins();
        // The iteration over an HashMap keys can have a different order at each refresh
        let mut recovery_keys: Vec<Fingerprint> = recovery_keys.into_keys().collect();
        recovery_keys.sort();

        let recovery_signature = new::b5_bold(format!(
            "{} signature{}",
            threshold,
            if threshold > 1 { "s" } else { "" }
        ));
        let recovery_key_count = if recovery_keys.len() > 1 {
            new::caption(format!("out of {} by", recovery_keys.len()))
        } else {
            new::caption("by")
        };
        let recovery_key_list =
            recovery_keys
                .iter()
                .enumerate()
                .fold(Row::new().spacing(5), |row, (i, k)| {
                    let content = if let Some(key) = keys.get(k) {
                        Container::new(
                            tooltip::Tooltip::new(
                                new::b5_bold(key.name.clone()),
                                new::caption(k.to_string()),
                                tooltip::Position::Bottom,
                            )
                            .style(theme::card::simple),
                        )
                    } else {
                        Container::new(new::b5_bold(format!("[{k}]")))
                    };
                    if recovery_keys.len() == 1 || i == recovery_keys.len() - 1 {
                        row.push(content)
                    } else if i <= recovery_keys.len() - 2 {
                        row.push(content).push(new::caption("and"))
                    } else {
                        row.push(content).push(new::caption(","))
                    }
                });
        let recovery_duration = new::b5_bold(format!(
            "{} blocks (~{})",
            sequence,
            expire_message_units(*sequence as u32).join(",")
        ));
        let recovery_kind = new::caption(
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
        );
        let recovery_row = row![
            recovery_signature,
            recovery_key_count,
            recovery_key_list,
            new::caption("can spend coins inactive for"),
            recovery_duration,
            recovery_kind,
        ]
        .spacing(5);

        col = col.push(recovery_row);
    }

    column![
        new::b5_bold("The wallet policy:"),
        scrollable::horizontal_thin(col)
    ]
    .spacing(10)
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
                    Some(format!("{n}{u}"))
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
                    Some(format!("{n}{u}"))
                } else {
                    None
                }
            })
            .collect()
    }
}

const RADIO_TITLE_WIDTH: u32 = 160;

#[allow(clippy::too_many_arguments)]
pub fn define_bitcoin_node<'a>(
    progress: (usize, usize),
    network: Network,
    available_node_types: impl Iterator<Item = NodeType>,
    selected_node_type: NodeType,
    node_view: Element<'a, Message>,
    is_running: Option<&Result<(), Error>>,
    can_try_ping: bool,
    waiting_for_ping_result: bool,
) -> Element<'a, Message> {
    let node_type = available_node_types.fold(
        row![text::new::b5_bold("Node type:").width(RADIO_TITLE_WIDTH)].spacing(10),
        |row, node_type| {
            row.push(radio(
                match node_type {
                    NodeType::Bitcoind => "Bitcoin Core",
                    NodeType::Electrum => "Electrum",
                },
                node_type,
                Some(selected_node_type),
                |new_selection| {
                    Message::DefineNode(message::DefineNode::NodeTypeSelected(new_selection))
                },
            ))
            .spacing(30)
            .align_y(Alignment::Center)
        },
    );

    let connection_status: Element<'a, Message> = if waiting_for_ping_result {
        text::new::caption("Checking connection...").into()
    } else if let Some(res) = is_running {
        if res.is_ok() {
            row![
                icon::circle_check_icon().style(theme::text::success),
                text::new::caption("Connection checked").style(theme::text::success),
            ]
        } else {
            row![
                icon::circle_cross_icon().style(theme::text::error),
                text::new::caption("Connection failed").style(theme::text::error),
            ]
        }
        .align_y(Alignment::Center)
        .into()
    } else {
        Container::new(Space::with_height(21)).into()
    };
    let node_view = column![node_view, connection_status].spacing(5);

    let msg_next = is_running.and_then(|r| r.is_ok().then_some(Message::Next));

    let msg_check_connection =
        (can_try_ping && !waiting_for_ping_result).then_some(Message::DefineNode(DefineNode::Ping));
    let button_check_connection = btn_check_connection(msg_check_connection, msg_next.is_none());
    let button_next = btn_next(msg_next);
    let actions = row![Space::fill_width(), button_check_connection, button_next].spacing(10);

    let content = column![node_type, node_view, actions].spacing(50);

    layout(
        progress,
        network,
        None,
        "Set up connection to the Bitcoin node",
        content,
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

    let address_msg = |msg| {
        Message::DefineNode(DefineNode::DefineBitcoind(
            DefineBitcoind::ConfigFieldEdited(ConfigField::Address, msg),
        ))
    };
    let address_input = form::Form::new_trimmed("Address", address, address_msg)
        .warning("Please enter correct address")
        .label("Address:")
        .padding(10);
    let loopback_warning = (!is_loopback && address.valid).then_some(
        text::new::caption(
            "Connection to a remote Bitcoin node is not supported. Insert an IP address bound to the same machine running Liana (ignore this warning if that's already the case)",
        )
        .style(theme::text::warning),
    );
    let address = column![address_input, loopback_warning].spacing(10);

    let auth_type = [RpcAuthType::CookieFile, RpcAuthType::UserPass]
        .iter()
        .fold(
            row![text::new::b5_bold("RPC authentication:").width(RADIO_TITLE_WIDTH)].spacing(10),
            |row, auth_type| {
                row.push(radio(
                    format!("{auth_type}"),
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
        );
    let auth_fields = match selected_auth_type {
        RpcAuthType::CookieFile => {
            row![
                form::Form::new_trimmed("Cookie path", &rpc_auth_vals.cookie_path, |msg| {
                    Message::DefineNode(DefineNode::DefineBitcoind(
                        DefineBitcoind::ConfigFieldEdited(ConfigField::CookieFilePath, msg),
                    ))
                })
                .warning("Please enter correct path")
            ]
        }
        RpcAuthType::UserPass => row![
            form::Form::new_trimmed("User", &rpc_auth_vals.user, |msg| {
                Message::DefineNode(DefineNode::DefineBitcoind(
                    DefineBitcoind::ConfigFieldEdited(ConfigField::User, msg),
                ))
            })
            .warning("Please enter correct user"),
            form::Form::new_trimmed("Password", &rpc_auth_vals.password, |msg| {
                Message::DefineNode(DefineNode::DefineBitcoind(
                    DefineBitcoind::ConfigFieldEdited(ConfigField::Password, msg),
                ))
            })
            .warning("Please enter correct password")
        ]
        .spacing(10),
    };
    let auth = column![auth_type, auth_fields].spacing(10);

    column![address, auth].spacing(50).into()
}

pub fn define_electrum<'a>(
    address: &form::Value<String>,
    validate_domain: bool,
) -> Element<'a, Message> {
    let validate_certificate_msg = |b| {
        Message::DefineNode(DefineNode::DefineElectrum(
            message::DefineElectrum::ValidDomainChanged(b),
        ))
    };
    let checkbox = validate_domain_checkbox(address, validate_domain, validate_certificate_msg);

    let address_msg = |msg| {
        Message::DefineNode(DefineNode::DefineElectrum(
            message::DefineElectrum::ConfigFieldEdited(electrum::ConfigField::Address, msg),
        ))
    };
    let address_input = form::Form::new_trimmed("127.0.0.1:50001", address, address_msg)
        .warning(
            "Please enter correct address (including port), \
        optionally prefixed with tcp:// or ssl://",
        )
        .label("Address")
        .padding(10);
    let address = column![
        address_input,
        checkbox,
        text::new::caption(electrum::ADDRESS_NOTES),
    ]
    .spacing(10);

    column![address].spacing(50).into()
}

pub fn select_bitcoind_type<'a>(
    progress: (usize, usize),
    network: Network,
) -> Element<'a, Message> {
    let existing_node_title = Container::new(text::new::b5_bold("I already have a node"))
        .padding(20)
        .width(Length::FillPortion(1));
    let managed_node_title = Container::new(text::new::b5_bold(
        "I want Liana to automatically install a Bitcoin node on my device",
    ))
    .padding(20)
    .width(Length::FillPortion(1));
    let titles = row![existing_node_title, managed_node_title].spacing(20);

    let existing_node_description = Container::new(
        text::new::caption(
            "Select this option if you already have a Bitcoin node running locally or remotely. Liana will connect to it.",
        )
        .style(theme::text::secondary),
    )
    .padding(20)
    .width(Length::FillPortion(1));
    let managed_node_description = Container::new(
        text::new::caption(
            "Liana will install a pruned node on your computer. You won't need to do anything except have some disk space available (~30GB required on mainnet) and wait for the initial synchronization with the network (it can take some days depending on your internet connection speed).",
        )
        .style(theme::text::secondary),
    )
    .padding(20)
    .width(Length::FillPortion(1));
    let descriptions = row![existing_node_description, managed_node_description].spacing(20);

    let existing_node_action = Container::new(btn_select(Some(Message::SelectBitcoindType(
        message::SelectBitcoindTypeMsg::UseExternal(true),
    ))))
    .padding(20)
    .center_x(Length::FillPortion(1));
    let managed_node_action = Container::new(btn_select(Some(Message::SelectBitcoindType(
        message::SelectBitcoindTypeMsg::UseExternal(false),
    ))))
    .padding(20)
    .center_x(Length::FillPortion(1));
    let actions = row![existing_node_action, managed_node_action].spacing(20);

    let content = column![titles, descriptions, actions];

    layout(
        progress,
        network,
        None,
        "Bitcoin node management",
        content,
        Some(Message::Previous),
    )
}

pub fn start_internal_bitcoind<'a>(
    progress: (usize, usize),
    network: Network,
    exe_path: Option<&PathBuf>,
    started: Option<&Result<(), StartInternalBitcoindError>>,
    error: Option<&'a String>,
    download_state: Option<&DownloadState>,
    install_state: Option<&InstallState>,
) -> Element<'a, Message> {
    let version = crate::node::bitcoind::VERSION;
    let msg_next = if let Some(Ok(_)) = started {
        Some(Message::Next)
    } else {
        None
    };
    layout(
        progress,
        network,
        None,
        "Start Bitcoin full node",
        Column::new()
            .push_maybe(download_state.map(|s| {
                match s {
                    DownloadState::Finished(_) => Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(icon::circle_check_icon().style(theme::text::success))
                        .push(new::caption("Download complete").style(theme::text::success)),
                    DownloadState::Downloading { progress } => Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(new::caption(format!(
                            "Downloading Bitcoin Core {version}... {progress:.2}%"
                        ))),
                    DownloadState::Errored(e) => Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(icon::circle_cross_icon().style(theme::text::error))
                        .push(
                            new::caption(format!("Download failed: '{e}'."))
                                .style(theme::text::error),
                        ),
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
                        .push(new::caption("Installation complete").style(theme::text::success)),
                    InstallState::Errored(e) => Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(icon::circle_cross_icon().style(theme::text::error))
                        .push(
                            new::caption(format!("Installation failed: '{e}'."))
                                .style(theme::text::error),
                        ),
                }
            } else if exe_path.is_some() {
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(icon::circle_check_icon().style(theme::text::success))
                    .push(
                        new::caption("Liana-managed bitcoind already installed")
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
                                .push(new::caption("Started").style(theme::text::success)),
                        )
                    } else {
                        Container::new(
                            Row::new()
                                .spacing(10)
                                .align_y(Alignment::Center)
                                .push(icon::circle_cross_icon().style(theme::text::error))
                                .push(
                                    new::caption(res.as_ref().err().unwrap().to_string())
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
                            .push(new::caption("Starting...")),
                    )),
                    _ => Some(Container::new(Space::with_height(Length::Fixed(25.0)))),
                }
            })
            .spacing(50)
            .push(btn_next(msg_next))
            .push_maybe(error.map(|e| card::invalid(new::caption(e)))),
        Some(message::Message::InternalBitcoind(
            message::InternalBitcoindMsg::Previous,
        )),
    )
}

pub fn install<'a>(
    progress: (usize, usize),
    network: Network,
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
        network,
        email,
        "Finalize installation",
        Column::new()
            .push_maybe(warning.map(|e| card::invalid(new::caption(e))))
            .push(if generating {
                Container::new(new::caption("Installing..."))
            } else if installed {
                Container::new(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(icon::circle_check_icon().style(theme::text::success))
                        .push(new::caption("Installed").style(theme::text::success)),
                )
            } else {
                Container::new(Space::with_height(Length::Fixed(25.0)))
            })
            .spacing(10)
            .width(Length::Fill),
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
                .push(new::caption(format!(
                    "{} out of {} key{}",
                    threshold.0,
                    threshold.1,
                    if threshold.1 > 1 { "s" } else { "" },
                )))
                .push(icon::edit_icon()),
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
                .push(new::caption(format!(
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
    let duration_row = Row::new()
        .padding(5)
        .spacing(5)
        .align_y(Alignment::Center)
        .push(new::caption(
            format_sequence_duration(sequence.as_u16(), true)
                .iter()
                .filter_map(|(n, unit)| {
                    if *n > 0 {
                        Some(format!("{n}{unit}"))
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
                                new::caption("Available after inactivity of ~")
                                    .style(theme::text::secondary),
                            )
                            .push(
                                Button::new(duration_row.push(icon::edit_icon()))
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
                        new::caption("Able to move the funds at any time.")
                            .style(theme::text::secondary),
                    )
                    .padding(5),
                PathSequence::SafetyNet => Row::new().align_y(Alignment::Center).push(
                    Container::new(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(5)
                            .push(
                                new::caption("Available after inactivity of ~")
                                    .style(theme::text::secondary),
                            )
                            .push(duration_row),
                    )
                    .width(Length::Fill)
                    .padding(5)
                    .align_y(alignment::Vertical::Center),
                ),
            })
            .push_maybe(warning.map(|w| new::small_caption(w.message()).style(theme::text::error)))
            .spacing(15),
    )
    .padding(5)
    .into()
}

pub fn backup_mnemonic<'a>(
    progress: (usize, usize),
    network: Network,
    email: Option<&'a str>,
    words: &'a [&'static str; 12],
    done: bool,
) -> Element<'a, Message> {
    let msg_next = done.then_some(Message::Next);
    layout(
        progress,
        network,
        email,
        "Back Up your mnemonic",
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
            .push(
                checkbox(done)
                    .label("I have backed up my mnemonic")
                    .on_toggle(Message::UserActionDone),
            )
            .push(btn_next(msg_next))
            .push(Space::with_height(20.0))
            .spacing(50),
        Some(Message::Previous),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn recover_mnemonic<'a>(
    progress: (usize, usize),
    network: Network,
    email: Option<&'a str>,
    words: &'a [(String, bool); 12],
    current: usize,
    suggestions: &'a [String],
    recover: bool,
    error: Option<&'a String>,
) -> Element<'a, Message> {
    let msg_next =
        (!words.iter().any(|(_, valid)| !valid) && error.is_none()).then_some(Message::Next);
    let button_next = btn_next(msg_next);

    layout(
        progress,
        network,
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
                    .push(button_next)
            })
            .spacing(50),
        Some(Message::Previous),
    )
}

pub fn choose_backend(progress: (usize, usize), network: Network) -> Element<'static, Message> {
    const PADDING: [u16; 2] = [0, 10];
    let local_title = Container::new(text::new::b1_bold("Use your own node"))
        .padding(PADDING)
        .width(Length::FillPortion(1));
    let remote_title = Container::new(text::new::b1_bold("Use Liana Connect"))
        .padding(PADDING)
        .width(Length::FillPortion(1));
    let titles = row![local_title, remote_title].spacing(20);

    let local_description =
        Container::new(text::new::caption(LOCAL_WALLET_DESC).style(theme::text::secondary))
            .padding(PADDING)
            .width(Length::FillPortion(1));
    let remote_description =
        Container::new(text::new::caption(REMOTE_BACKEND_DESC).style(theme::text::secondary))
            .padding(PADDING)
            .width(Length::FillPortion(1));
    let descriptions = row![local_description, remote_description].spacing(20);

    let local_action = Container::new(btn_select(Some(Message::SelectBackend(
        message::SelectBackend::ContinueWithLocalWallet(true),
    ))))
    .padding(PADDING)
    .center_x(Length::FillPortion(1));
    let remote_action = Container::new(btn_select(Some(Message::SelectBackend(
        message::SelectBackend::ContinueWithLocalWallet(false),
    ))))
    .padding(PADDING)
    .center_x(Length::FillPortion(1));
    let actions = row![local_action, remote_action].spacing(20);

    let help_link = tooltip::Tooltip::new(
        btn_backend_options_help(Message::OpenUrl(
            help::CHANGE_BACKEND_OR_NODE_URL.to_string(),
        )),
        Container::new(new::caption(help::CHANGE_BACKEND_OR_NODE_URL))
            .style(theme::card::simple)
            .padding(10),
        tooltip::Position::Bottom,
    );

    let content = column![
        titles,
        descriptions,
        actions,
        Space::with_height(20),
        help_link,
    ]
    .spacing(20);

    layout(
        progress,
        network,
        None,
        "Choose backend",
        content,
        Some(Message::Previous),
    )
}

pub fn login(
    progress: (usize, usize),
    network: Network,
    connection_step: Element<Message>,
) -> Element<Message> {
    let content = Container::new(
        column![
            installer_layout::screen_intro("Liana Connect", None, true),
            connection_step,
        ]
        .spacing(50)
        .align_x(Alignment::Center)
        .width(Length::FillPortion(1)),
    )
    .center_x(Length::Fill);

    layout(
        progress,
        network,
        None,
        "Login",
        content,
        Some(Message::Previous),
    )
}

pub fn connection_step_enter_email<'a>(
    email: &'a form::Value<String>,
    processing: bool,
    connection_error: Option<&'a Error>,
    accounts: &'a [String],
    auth_error: Option<&'a str>,
) -> Element<'a, Message> {
    let accounts_row = accounts.iter().fold(row![].spacing(10), |row, account| {
        row.push(
            Button::new(Container::new(new::caption(account)).padding(5))
                .style(theme::button::secondary)
                .on_press(Message::SelectBackend(
                    message::SelectBackend::SelectConnectAccount(account.clone()),
                )),
        )
    });
    let email_prompt: Element<'_, Message> = if accounts.is_empty() {
        new::caption("Enter an email you want to associate with the wallet:").into()
    } else {
        new::caption("Or enter a new email you want to associate with the wallet:").into()
    };
    let send_token_msg = (!(processing || !email.valid || email.value.trim().is_empty()))
        .then_some(Message::SelectBackend(message::SelectBackend::RequestOTP));
    let accounts_prompt: Option<Element<'_, Message>> = (!accounts.is_empty())
        .then_some(new::caption("Choose an account you are already using:").into());
    column![
        accounts_prompt,
        accounts_row.wrap(),
        connection_error.map(|error| -> Element<'_, Message> {
            new::caption(error.to_string())
                .style(theme::text::warning)
                .into()
        }),
        auth_error.map(|error| -> Element<'_, Message> {
            new::caption(error.to_string())
                .style(theme::text::warning)
                .into()
        }),
        email_prompt,
        form::Form::new_trimmed("email", email, |msg| {
            Message::SelectBackend(message::SelectBackend::EmailEdited(msg))
        })
        .padding(10)
        .warning("Email is not valid"),
        row![
            Space::fill_width(),
            button::secondary(None, "Send token")
                .on_press_maybe(send_token_msg)
                .width(200),
        ],
    ]
    .spacing(20)
    .into()
}

fn otp_intro<'a>(email: &'a str) -> Column<'a, Message> {
    column![
        new::caption(email).style(theme::text::success),
        new::caption("An authentication token has been emailed to you"),
    ]
    .spacing(20)
}

fn otp_form<'a>(otp: &'a form::Value<String>) -> form::Form<'a, Message> {
    form::Form::new_trimmed("Token", otp, |msg| {
        Message::SelectBackend(message::SelectBackend::OTPEdited(msg))
    })
    .padding(10)
    .warning("Token is not valid")
}

pub fn connection_step_enter_otp<'a>(
    email: &'a str,
    otp: &'a form::Value<String>,
    processing: bool,
    connection_error: Option<&'a Error>,
    auth_error: Option<&'a str>,
) -> Element<'a, Message> {
    let resend_token =
        (!processing).then_some(Message::SelectBackend(message::SelectBackend::RequestOTP));
    column![
        otp_intro(email),
        connection_error.map(|error| -> Element<'_, Message> {
            new::caption(error.to_string())
                .style(theme::text::warning)
                .into()
        }),
        auth_error.map(|error| -> Element<'_, Message> {
            new::caption(error.to_string())
                .style(theme::text::warning)
                .into()
        }),
        otp_form(otp),
        row![
            button::secondary(Some(icon::previous_icon()), "Change Email")
                .on_press(Message::SelectBackend(message::SelectBackend::EditEmail)),
            button::secondary(None, "Resend token").on_press_maybe(resend_token),
        ]
        .spacing(10),
    ]
    .spacing(20)
    .into()
}

pub fn connection_step_connected<'a>(
    email: &'a str,
    processing: bool,
    connection_error: Option<&'a Error>,
    auth_error: Option<&'a str>,
) -> Element<'a, Message> {
    let msg_next = (!processing).then_some(Message::Next);
    column![
        new::caption(email).style(theme::text::success),
        connection_error.map(|error| -> Element<'_, Message> {
            new::caption(error.to_string())
                .style(theme::text::warning)
                .into()
        }),
        auth_error.map(|error| -> Element<'_, Message> {
            new::caption(error.to_string())
                .style(theme::text::warning)
                .into()
        }),
        Container::new(
            row![
                button::secondary(Some(icon::previous_icon()), "Change Email").on_press_maybe(
                    (!processing)
                        .then_some(Message::SelectBackend(message::SelectBackend::EditEmail))
                ),
                Space::fill_width(),
                button::secondary(None, "Continue").on_press_maybe(msg_next),
            ]
            .spacing(10),
        ),
    ]
    .spacing(20)
    .into()
}

pub const REMOTE_BACKEND_DESC: &str = "Use our service to instantly be ready to transact. Wizardsardine runs the infrastructure, allowing multiple computers or participants to connect and synchronize.\n\nThis is a simpler and safer option for people who want Wizardsardine to keep a backup of their descriptor. You are still in control of your keys, and Wizardsardine does not have any control over your funds, but it will be able to see your wallet's information, associated to an email address. Privacy focused users should run their own infrastructure instead.";

pub const LOCAL_WALLET_DESC: &str = "Use your already existing Bitcoin node or automatically install one. The Liana wallet will not connect to any external server.\n\nThis is the most private option, but the data is locally stored on this computer, only. You must perform your own backups, and share the descriptor with other people you want to be able to access the wallet.";

pub fn wallet_alias<'a>(
    progress: (usize, usize),
    network: Network,
    email: Option<&'a str>,
    wallet_alias: &form::Value<String>,
) -> Element<'a, Message> {
    let msg_next = wallet_alias.valid.then_some(Message::Next);
    let label = new::b5_bold("Wallet alias:");
    let form = form::Form::new("Wallet alias", wallet_alias, Message::WalletAliasEdited)
        .warning("Wallet alias is too long.");
    let note = new::caption("You will be able to change it later in Settings > Wallet");
    let form_section = column![label, form, note].spacing(20);
    let next = row![Space::fill_width(), btn_next(msg_next)];
    let content = column![form_section, next].spacing(50);

    layout(
        progress,
        network,
        email,
        "Give your wallet an alias",
        content,
        Some(Message::Previous),
    )
}

fn layout<'a>(
    progress: (usize, usize),
    network: Network,
    email: Option<&'a str>,
    title: &'static str,
    content: impl Into<Element<'a, Message>>,
    previous_message: Option<Message>,
) -> Element<'a, Message> {
    installer_layout::layout(
        installer_layout::LayoutConfig {
            variant: Variant::Liana,
            network,
            email,
            is_ws_admin: false,
            nav_bar: installer_layout::NavBar::StepTitle {
                progress,
                title,
                previous_message,
            },
            content_width: 800.0,
        },
        content,
    )
}
