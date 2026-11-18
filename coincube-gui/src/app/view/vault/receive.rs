use coincube_ui::locale::copy as localized_copy;
use std::collections::{HashMap, HashSet};

use coincube_ui::locale::Text as CopyText;
use iced::{
    advanced::widget::Id,
    widget::{
        button as iced_button, container,
        qr_code::{self, QRCode},
        scrollable,
        text::Wrapping,
        Button, Space,
    },
    Alignment, Length, Padding,
};

use coincube_core::miniscript::bitcoin::{
    self,
    bip32::{ChildNumber, Fingerprint},
    Address,
};

use coincube_ui::{
    component::{
        button, card, form,
        text::{self, *},
    },
    font, icon, theme,
    widget::{action::Action, Column, Container, Element, Row},
};

use crate::{
    app::{
        menu::{CubeSubMenu, Menu},
        view::{
            loading_placeholder,
            message::VaultReceiveMessage,
            placeholder,
            shared::landing::{landing_card_style, landing_subtle_button_style},
            vault::{hw, label},
        },
    },
    hw::HardwareWallet,
};

use super::super::message::Message;

/// Compact address card with label, copy, verify and QR controls.
///
/// The Receive route renders its own redesigned presenter below. This card
/// keeps its layout for the Transfer confirmation in `global_home`, its only
/// other caller.
pub fn address_card<'a>(
    row_index: usize,
    address: &'a bitcoin::Address,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Container<'a, Message> {
    let addr = address.to_string();
    card::simple(
        Column::new()
            .push(if let Some(label) = labels_editing.get(&addr) {
                label::label_editing(vec![addr.clone()], label, text::P1_SIZE)
            } else {
                label::label_editable(vec![addr.clone()], labels.get(&addr), text::P1_SIZE)
            })
            .push(
                Row::new()
                    .push(
                        Container::new(
                            scrollable(
                                Column::new()
                                    .push(Space::new().height(Length::Fixed(10.0)))
                                    .push(
                                        p2_regular(address.to_string())
                                            .small()
                                            .style(theme::text::secondary),
                                    )
                                    // Space between the address and the scrollbar
                                    .push(Space::new().height(Length::Fixed(10.0))),
                            )
                            .direction(
                                scrollable::Direction::Horizontal(
                                    scrollable::Scrollbar::new().width(2).scroller_width(2),
                                ),
                            ),
                        )
                        .width(Length::Fill),
                    )
                    .push(
                        Button::new(icon::clipboard_icon().style(theme::text::secondary))
                            .on_press(Message::VaultReceive(VaultReceiveMessage::Copy(
                                addr.clone(),
                            )))
                            .style(theme::button::transparent_border),
                    )
                    .align_y(Alignment::Center),
            )
            .push(
                Row::new()
                    .push(
                        button::secondary(None, localized_copy("Verify on hardware device"))
                            .on_press(Message::Select(row_index)),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::secondary(None, localized_copy("Show QR Code"))
                            .on_press(Message::ShowQrCode(row_index)),
                    ),
            )
            .spacing(10),
    )
}

const ID_PREFIX: &str = "tenshu.vault.receive";
/// Characters kept at each end of a collapsed previous address.
const NUM_ADDR_CHARS: usize = 16;
const SECTION_GAP: f32 = 24.0;

/// Where an address control is rendered. Addresses generated in this session
/// keep their scope when a newer one takes the current slot, so their action
/// identity does not depend on display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Generated,
    Previous,
    VerifyModal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    Copy,
    Verify,
    Qr,
    Toggle,
}

fn control_id(control: &str) -> Id {
    Id::from(format!("{ID_PREFIX}:{control}"))
}

fn address_control_id(scope: Scope, control: Control, address: &str) -> Id {
    let scope = match scope {
        Scope::Generated => "generated",
        Scope::Previous => "previous",
        Scope::VerifyModal => "verify-modal",
    };
    let control = match control {
        Control::Copy => "copy",
        Control::Verify => "verify",
        Control::Qr => "qr",
        Control::Toggle => "toggle",
    };
    Id::from(format!("{ID_PREFIX}:{scope}:{control}:{address}"))
}

fn generate_id() -> Id {
    control_id("generate")
}

fn previous_disclosure_id() -> Id {
    control_id("previous")
}

fn see_more_id() -> Id {
    control_id("more")
}

pub(crate) fn verify_close_id() -> Id {
    control_id("verify-close")
}

pub(crate) fn qr_close_id() -> Id {
    control_id("qr-close")
}

pub(crate) fn modal_origin_id(
    index: usize,
    generated_count: usize,
    address: &Address,
    verify: bool,
) -> Id {
    let scope = if index < generated_count {
        Scope::Generated
    } else {
        Scope::Previous
    };
    address_control_id(
        scope,
        if verify { Control::Verify } else { Control::Qr },
        &address.to_string(),
    )
}

/// A keyboard-reachable control. A disabled action has no press message and
/// therefore no focus target, so Tab skips it.
fn action<'a>(
    id: Id,
    label: &'static str,
    icon: Option<coincube_ui::widget::Text<'a>>,
    press: Option<Message>,
    style: fn(&theme::Theme, iced_button::Status) -> iced_button::Style,
) -> Action<'a, Message> {
    let label = p2_medium(label);
    let content: Element<'a, Message> = match icon {
        Some(icon) => Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(icon)
            .push(label)
            .into(),
        None => label.into(),
    };
    Action::new(
        id,
        Container::new(content)
            .padding([6, 12])
            .align_y(iced::alignment::Vertical::Center),
        Message::FocusAction,
    )
    .style(style)
    .on_press_maybe(press)
}

fn tile_style(theme: &theme::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.background)),
        border: iced::Border {
            color: theme.colors.shell.line,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    }
}

fn field_style(theme: &theme::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.background)),
        border: iced::Border {
            color: theme.colors.shell.line,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

fn tag_style(theme: &theme::Theme) -> container::Style {
    container::Style {
        border: iced::Border {
            color: theme.colors.shell.accent_ink,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    }
}

fn accent_text(theme: &theme::Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(theme.colors.shell.accent_ink),
    }
}

/// The shared orange outline with a readable resting label. The shared label
/// orange is 2.6:1 on the light card, below 4.5:1 for 14px text, so the active
/// label uses the shell's accent ink. Hovered, pressed and disabled are the
/// shared style unchanged.
fn new_address_style(theme: &theme::Theme, status: iced_button::Status) -> iced_button::Style {
    let style = theme::button::orange_outline(theme, status);
    if matches!(status, iced_button::Status::Active) {
        iced_button::Style {
            text_color: theme.colors.shell.accent_ink,
            ..style
        }
    } else {
        style
    }
}

/// The card's generate action is primary until the first address exists,
/// then the readable outline.
fn generate_style(
    has_address: bool,
) -> fn(&theme::Theme, iced_button::Status) -> iced_button::Style {
    if has_address {
        new_address_style
    } else {
        theme::button::primary
    }
}

fn activity_link_style(theme: &theme::Theme, status: iced_button::Status) -> iced_button::Style {
    let mut style = theme::button::link(theme, status);
    // The shared link's idle grey misses P-Receive's orange text-link cue.
    // accent_ink keeps that cue readable in both dark and light themes.
    style.text_color = theme.colors.shell.accent_ink;
    style
}

fn truncate_middle(address: &str) -> String {
    let len = address.chars().count();
    if len > 2 * NUM_ADDR_CHARS {
        format!(
            "{}...{}",
            address.chars().take(NUM_ADDR_CHARS).collect::<String>(),
            address
                .chars()
                .skip(len - NUM_ADDR_CHARS)
                .collect::<String>(),
        )
    } else {
        address.to_string()
    }
}

/// The full address in a monospace field that wraps instead of scrolling, so
/// every character stays visible for comparison, followed by Copy.
fn address_field<'a>(copy_id: Id, address: String) -> Element<'a, Message> {
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            Container::new(
                p2_regular(address.clone())
                    .font(font::MONO_REGULAR)
                    .wrapping(Wrapping::Glyph),
            )
            .padding([10, 12])
            .width(Length::Fill)
            .style(field_style),
        )
        .push(action(
            copy_id,
            coincube_ui::locale::copy("Copy"),
            Some(icon::clipboard_icon()),
            Some(Message::VaultReceive(VaultReceiveMessage::Copy(address))),
            landing_subtle_button_style,
        ))
        .into()
}

/// The existing native label editor, unchanged, under a "Label" heading.
fn label_block<'a>(
    address: &str,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    Column::new()
        .spacing(8)
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(p2_bold(localized_copy("Label")).width(Length::Fill))
                .push(p2_regular(localized_copy("Optional")).style(theme::text::secondary)),
        )
        .push(if let Some(label) = labels_editing.get(address) {
            label::label_editing(vec![address.to_string()], label, text::P1_SIZE)
        } else {
            label::label_editable(
                vec![address.to_string()],
                labels.get(address),
                text::P1_SIZE,
            )
        })
        .into()
}

/// Show QR and Verify for the address at `row_index`, the state's original
/// index (generated rows display newest first; previous rows are offset).
fn address_actions<'a>(scope: Scope, row_index: usize, address: &str) -> Element<'a, Message> {
    Row::new()
        .spacing(8)
        .push(action(
            address_control_id(scope, Control::Qr, address),
            coincube_ui::locale::copy("Show QR code"),
            Some(icon::qr_code_icon()),
            Some(Message::ShowQrCode(row_index)),
            landing_subtle_button_style,
        ))
        .push(action(
            address_control_id(scope, Control::Verify, address),
            coincube_ui::locale::copy("Verify on hardware device"),
            Some(icon::usb_icon()),
            Some(Message::Select(row_index)),
            landing_subtle_button_style,
        ))
        .into()
}

/// Address details for a listed address: label, full address with Copy, then
/// QR and Verify.
fn address_details<'a>(
    scope: Scope,
    row_index: usize,
    address: &'a bitcoin::Address,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let addr = address.to_string();
    Container::new(
        Column::new()
            .spacing(12)
            .push(label_block(&addr, labels, labels_editing))
            .push(address_field(
                address_control_id(scope, Control::Copy, &addr),
                addr.clone(),
            ))
            .push(address_actions(scope, row_index, &addr)),
    )
    .padding(16)
    .width(Length::Fill)
    .style(tile_style)
    .into()
}

/// The newest generated address, presented as the card's receive output.
fn current_address<'a>(
    row_index: usize,
    address: &'a bitcoin::Address,
    qr: Option<&'a qr_code::Data>,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let addr = address.to_string();
    let details = Column::new()
        .spacing(16)
        .push(
            Column::new()
                .spacing(8)
                .push(p2_bold(localized_copy("Bitcoin address")))
                .push(address_field(
                    address_control_id(Scope::Generated, Control::Copy, &addr),
                    addr.clone(),
                ))
                .push(
                    p2_regular(localized_copy(
                        "Addresses are single-use. Generate a new one for each deposit.",
                    ))
                    .style(theme::text::secondary),
                ),
        )
        .push(action(
            address_control_id(Scope::Generated, Control::Verify, &addr),
            coincube_ui::locale::copy("Verify on hardware device"),
            Some(icon::usb_icon()),
            Some(Message::Select(row_index)),
            landing_subtle_button_style,
        ))
        .push(label_block(&addr, labels, labels_editing));
    if let Some(qr) = qr {
        Row::new()
            .spacing(24)
            .align_y(Alignment::Start)
            .width(Length::Fill)
            .push(
                Container::new(QRCode::<coincube_ui::theme::Theme>::new(qr).total_size(160))
                    .padding(8)
                    .style(field_style),
            )
            .push(Container::new(details).width(Length::Fill))
            .into()
    } else {
        Column::new()
            .spacing(12)
            .push(
                p2_regular(localized_copy("QR code unavailable for this address."))
                    .style(theme::text::secondary),
            )
            .push(details)
            .into()
    }
}

fn route_tile<'a>(caption: &'static str, rail: &'static str) -> Element<'a, Message> {
    Container::new(
        Column::new()
            .spacing(6)
            .push(p2_regular(caption).style(theme::text::secondary))
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(icon::bitcoin_icon().style(accent_text))
                    .push(p1_bold("Bitcoin")),
            )
            .push(p2_regular(localized_copy("Any amount")).style(theme::text::secondary))
            .push(
                Container::new(
                    text(rail)
                        .size(CAPTION_SIZE)
                        .font(font::MEDIUM)
                        .style(accent_text),
                )
                .padding([1, 8])
                .style(tag_style),
            ),
    )
    .padding(16)
    .width(Length::Fill)
    .style(tile_style)
    .into()
}

fn card_section<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Container::new(content)
        .padding([20, 24])
        .width(Length::Fill)
        .into()
}

/// One previously generated address: a selection row that discloses its
/// details below it. The details are a sibling of the row, never its child, so
/// Copy, QR and Verify do not also toggle the selection.
fn previous_row<'a>(
    row_index: usize,
    address: &'a bitcoin::Address,
    prev_labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    expanded: bool,
) -> Element<'a, Message> {
    let addr = address.to_string();
    let toggle = Action::new(
        address_control_id(Scope::Previous, Control::Toggle, &addr),
        Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(
                p2_regular(truncate_middle(&addr))
                    .font(font::MONO_REGULAR)
                    .style(theme::text::secondary)
                    .width(Length::FillPortion(3)),
            )
            .push(
                p2_regular(prev_labels.get(&addr).cloned().unwrap_or_default())
                    .style(theme::text::secondary)
                    .wrapping(Wrapping::WordOrGlyph)
                    .width(Length::FillPortion(2)),
            )
            .push(if expanded {
                icon::up_icon()
            } else {
                icon::down_icon()
            }),
        Message::FocusAction,
    )
    .padding([10, 16])
    .width(Length::Fill)
    .style(landing_subtle_button_style)
    .on_press(Message::SelectAddress(address.clone()));
    Column::new()
        .push(toggle)
        .push(expanded.then(|| {
            gap(
                8.0,
                address_details(
                    Scope::Previous,
                    row_index,
                    address,
                    prev_labels,
                    labels_editing,
                ),
            )
        }))
        .into()
}

/// Space above an optional section. Optional sections occupy a fixed slot
/// (empty when absent), so the gap lives inside the slot rather than in the
/// parent's spacing, which would also separate empty slots.
fn gap<'a>(top: f32, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Container::new(content)
        .padding(Padding::ZERO.top(top))
        .width(Length::Fill)
        .into()
}

fn gap_above<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    gap(SECTION_GAP, content)
}

fn generate_action<'a>(has_address: bool, generating: bool) -> Action<'a, Message> {
    let style = generate_style(has_address);
    let label = if !has_address {
        localized_copy("Generate address")
    } else {
        localized_copy("New address")
    };
    if generating {
        action(
            generate_id(),
            coincube_ui::locale::copy("Generating address…"),
            None,
            None,
            style,
        )
    } else {
        action(
            generate_id(),
            label,
            Some(icon::plus_icon()),
            Some(Message::NextReceiveAddress),
            style,
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub fn receive<'a>(
    addresses: &'a [bitcoin::Address],
    current_qr: Option<&'a qr_code::Data>,
    vault_name: &'a str,
    balance_display: String,
    labels: &'a HashMap<String, String>,
    prev_addresses: &'a [bitcoin::Address],
    prev_labels: &'a HashMap<String, String>,
    show_prev_addresses: bool,
    selected: &'a HashSet<bitcoin::Address>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    is_last_page: bool,
    processing: bool,
    generating: bool,
) -> Element<'a, Message> {
    // The newest generated address is the current one; `addresses` is in
    // generation order, so it is the last entry.
    let body: Element<'a, Message> = if let Some((i, address)) =
        addresses.iter().enumerate().next_back()
    {
        current_address(i, address, current_qr, labels, labels_editing)
    } else if !prev_addresses.is_empty() {
        p2_regular(
            localized_copy("Generate a new address for this deposit. Earlier addresses still awaiting a deposit are listed below."),
        )
        .style(theme::text::secondary)
        .into()
    } else if processing {
        loading_placeholder(icon::receive_icon().size(56), "Loading addresses")
    } else {
        placeholder(
            icon::receive_icon().size(56),
            coincube_ui::locale::copy("No addresses yet"),
            coincube_ui::locale::copy("Generate a new address to receive bitcoin. Always generate a new address for each deposit."),
        )
    };

    let generate = generate_action(!addresses.is_empty(), generating);

    // Fixed sections keep every control at a stable tree position, so focus
    // survives a state change that only alters a section's contents.
    let payment_card = Container::new(
        Column::new()
            .push(
                Container::new(h5_medium("Payment details"))
                    .padding([16, 24])
                    .width(Length::Fill),
            )
            .push(iced::widget::rule::horizontal(1))
            .push(card_section(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push(route_tile(
                        coincube_ui::locale::copy("They send"),
                        coincube_ui::locale::copy("ON-CHAIN"),
                    ))
                    .push(text("→").style(theme::text::secondary))
                    .push(route_tile(
                        coincube_ui::locale::copy("You receive"),
                        coincube_ui::locale::copy("ON-CHAIN"),
                    )),
            ))
            .push(iced::widget::rule::horizontal(1))
            .push(card_section(body))
            .push(iced::widget::rule::horizontal(1))
            .push(
                Container::new(
                    Row::new()
                        .align_y(Alignment::Center)
                        .push(Space::new().width(Length::Fill))
                        .push(generate),
                )
                .padding([16, 24])
                .width(Length::Fill),
            ),
    )
    .width(Length::Fill)
    .style(landing_card_style);

    let earlier = (addresses.len() > 1).then(|| -> Element<'a, Message> {
        Column::new()
            .spacing(12)
            .push(p1_bold(localized_copy("Generated earlier in this session")))
            .push(addresses.iter().enumerate().rev().skip(1).fold(
                Column::new().spacing(12),
                |col, (i, address)| {
                    col.push(address_details(
                        Scope::Generated,
                        i,
                        address,
                        labels,
                        labels_editing,
                    ))
                },
            ))
            .into()
    });

    let previous = (!prev_addresses.is_empty()).then(|| -> Element<'a, Message> {
        Column::new()
            .push(
                Action::new(
                    previous_disclosure_id(),
                    Row::new()
                        .spacing(12)
                        .align_y(Alignment::Center)
                        .push(
                            p1_bold(localized_copy(
                                "Previously generated addresses still awaiting deposit",
                            ))
                            .width(Length::Fill),
                        )
                        .push(if show_prev_addresses {
                            icon::collapsed_icon()
                        } else {
                            icon::collapse_icon()
                        }),
                    Message::FocusAction,
                )
                .padding([16, 20])
                .width(Length::Fill)
                .style(landing_subtle_button_style)
                .on_press(Message::ToggleShowPreviousAddresses),
            )
            .push(show_prev_addresses.then(|| {
                // Previous rows are already in descending order. Their state
                // index continues after the generated addresses.
                gap(
                    12.0,
                    prev_addresses.iter().enumerate().fold(
                        Column::new().spacing(8).width(Length::Fill),
                        |col, (i, address)| {
                            col.push(previous_row(
                                addresses.len() + i,
                                address,
                                prev_labels,
                                labels_editing,
                                selected.contains(address),
                            ))
                        },
                    ),
                )
            }))
            .push((show_prev_addresses && !is_last_page).then(|| {
                gap(
                    12.0,
                    Action::new(
                        see_more_id(),
                        Container::new(p2_medium(if processing {
                            localized_copy("Fetching…")
                        } else {
                            localized_copy("See more")
                        }))
                        .width(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Center),
                        Message::FocusAction,
                    )
                    .padding([12, 16])
                    .width(Length::Fill)
                    .style(landing_subtle_button_style)
                    .on_press_maybe((!processing).then_some(Message::Next)),
                )
            }))
            .into()
    });

    let aside = Column::new()
        .spacing(8)
        .push(p2_regular(localized_copy("Receiving into")).style(theme::text::secondary))
        .push(p1_bold(format!("Vault · {vault_name}")))
        .push(p2_bold(balance_display))
        .push(iced::widget::rule::horizontal(1))
        .push(
            p2_regular(localized_copy(
                "On-chain deposits appear after the Vault syncs.",
            ))
            .style(theme::text::secondary),
        )
        .push(
            Action::new(
                control_id("activity"),
                p2_medium(localized_copy("See all in Activity")),
                Message::FocusAction,
            )
            .padding([8, 0])
            .style(activity_link_style)
            .on_press(Message::Menu(Menu::Cube(CubeSubMenu::Activity))),
        );

    // The title and the Cube line are in the page head (`state::vault::receive`).
    Column::new()
        .width(Length::Fill)
        .push(
            Row::new()
                .spacing(40)
                .align_y(Alignment::Start)
                .push(
                    Container::new(payment_card)
                        .width(Length::Fill)
                        .max_width(680.0),
                )
                .push(Container::new(aside).width(Length::Fixed(240.0)))
                .wrap()
                .vertical_spacing(20),
        )
        .push(earlier.map(gap_above))
        .push(previous.map(gap_above))
        .into()
}

pub fn verify_address_modal<'a>(
    chain: crate::chain::ChainId,
    hws: &'a [HardwareWallet],
    chosen_hws: &HashSet<Fingerprint>,
    address: &Address,
    derivation_index: &ChildNumber,
) -> Element<'a, Message> {
    let addr = address.to_string();
    let devices: Element<'a, Message> = if hws.is_empty() {
        p2_regular(
            localized_copy("No hardware device detected. Connect and unlock a device that holds a key to this Vault."),
        )
        .style(theme::text::secondary)
        .into()
    } else {
        hws.iter()
            .enumerate()
            .fold(Column::new().spacing(10), |col, (i, hw)| {
                col.push(hw::hw_list_view_verify_address(
                    i,
                    hw,
                    if let HardwareWallet::Supported { fingerprint, .. } = hw {
                        chosen_hws.contains(fingerprint)
                    } else {
                        false
                    },
                ))
            })
            .into()
    };
    Container::new(
        Column::new()
            .spacing(20)
            .push(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push(h4_bold(localized_copy("Verify address on a device")).width(Length::Fill))
                    .push(action(
                        verify_close_id(),
                        CopyText::Close.get(),
                        Some(icon::cross_icon()),
                        Some(Message::Close),
                        landing_subtle_button_style,
                    )),
            )
            .push(
                p2_regular(
                    localized_copy("Select a device, then check that the address it shows matches this one exactly."),
                )
                .style(theme::text::secondary),
            )
            .push(
                Column::new()
                    .spacing(8)
                    .push(p2_bold(CopyText::Address.get()))
                    .push(p2_regular(crate::chain::canonical_network_name(chain)))
                    .push(address_field(
                        address_control_id(Scope::VerifyModal, Control::Copy, &addr),
                        addr.clone(),
                    )),
            )
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(p2_bold(localized_copy("Derivation index")).width(Length::Fill))
                    .push(p2_regular(derivation_index.to_string()).font(font::MONO_REGULAR)),
            )
            .push(
                Column::new()
                    .spacing(10)
                    .push(p2_bold(localized_copy("Devices")))
                    .push(devices),
            ),
    )
    .padding(24)
    .width(Length::Fill)
    .max_width(750)
    .style(landing_card_style)
    .into()
}

pub fn qr_modal<'a>(qr: &'a qr_code::Data, _address: &'a String) -> Element<'a, Message> {
    Container::new(
        Column::new()
            .spacing(16)
            .align_x(Alignment::Center)
            .push(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push(p1_bold(localized_copy("Scan to pay this address")).width(Length::Fill))
                    .push(action(
                        qr_close_id(),
                        CopyText::Close.get(),
                        Some(icon::cross_icon()),
                        Some(Message::Close),
                        landing_subtle_button_style,
                    )),
            )
            .push(
                Container::new(QRCode::<coincube_ui::theme::Theme>::new(qr).cell_size(8))
                    .padding(10),
            ),
    )
    .padding(20)
    .width(Length::Fill)
    .max_width(400)
    .style(landing_card_style)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use coincube_core::miniscript::bitcoin::{Network, ScriptBuf};
    use iced::advanced::{
        layout,
        renderer::Headless,
        widget::{operation, operation::focusable, Operation, Tree},
        Layout,
    };
    use iced::{keyboard, mouse, Event, Rectangle, Size};

    const COMPACT: (u32, u32) = (1024, 768);
    const WIDE: (u32, u32) = (1440, 900);
    /// Tall enough that a pointer press lands on every control unscrolled.
    const TALL: (u32, u32) = (1440, 3000);

    /// Synthetic testnet P2WSH address; distinct `n` gives a distinct address.
    fn addr(n: u8) -> Address {
        Address::p2wsh(&ScriptBuf::from_bytes(vec![0x51, n]), Network::Testnet)
    }

    struct Fixture {
        addresses: Vec<Address>,
        current_qr: Option<qr_code::Data>,
        labels: HashMap<String, String>,
        prev: Vec<Address>,
        prev_labels: HashMap<String, String>,
        show_prev: bool,
        selected: HashSet<Address>,
        editing: HashMap<String, form::Value<String>>,
        is_last_page: bool,
        processing: bool,
        generating: bool,
    }

    impl Fixture {
        fn empty() -> Self {
            Self {
                addresses: Vec::new(),
                current_qr: None,
                labels: HashMap::new(),
                prev: Vec::new(),
                prev_labels: HashMap::new(),
                show_prev: false,
                selected: HashSet::new(),
                editing: HashMap::new(),
                is_last_page: true,
                processing: false,
                generating: false,
            }
        }

        /// Three generated addresses (newest last), two previous addresses
        /// with the second expanded, and a further page available.
        fn populated() -> Self {
            let mut f = Self::empty();
            f.addresses = vec![addr(1), addr(2), addr(3)];
            f.current_qr = Some(
                qr_code::Data::new(format!("bitcoin:{}?index=3", addr(3)))
                    .expect("synthetic address fits a QR code"),
            );
            f.labels
                .insert(addr(3).to_string(), "Salary · synthetic".into());
            f.prev = vec![addr(10), addr(11)];
            f.prev_labels.insert(
                addr(11).to_string(),
                "Rent deposit from the family trust · synthetic".into(),
            );
            f.show_prev = true;
            f.selected.insert(addr(11));
            f.is_last_page = false;
            f
        }

        fn view(&self) -> Element<'_, Message> {
            receive(
                &self.addresses,
                self.current_qr.as_ref(),
                "Family Vault",
                "250,000 SATS".to_string(),
                &self.labels,
                &self.prev,
                &self.prev_labels,
                self.show_prev,
                &self.selected,
                &self.editing,
                self.is_last_page,
                self.processing,
                self.generating,
            )
        }
    }

    fn generated(control: Control, n: u8) -> Id {
        address_control_id(Scope::Generated, control, &addr(n).to_string())
    }

    fn previous(control: Control, n: u8) -> Id {
        address_control_id(Scope::Previous, control, &addr(n).to_string())
    }

    fn themes() -> [(&'static str, theme::Theme); 2] {
        [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ]
    }

    #[test]
    fn activity_link_uses_the_readable_accent_without_a_button_box() {
        for (_, theme) in themes() {
            let style = activity_link_style(&theme, iced_button::Status::Active);
            assert_eq!(style.text_color, theme.colors.shell.accent_ink);
            assert_eq!(style.border.color.a, 0.0);
            assert!(matches!(
                style.background,
                Some(iced::Background::Color(color)) if color.a == 0.0
            ));
        }
    }

    /// The single non-focus message from activating `target` on the real
    /// tree. A pointer press must also publish exactly one focus intent for
    /// the same target; Enter publishes none.
    fn activate_once(content: Element<'_, Message>, target: Id, pointer: bool) -> Message {
        let messages = capture::activate(content, target.clone(), TALL, pointer);
        let (focus, rest): (Vec<_>, Vec<_>) = messages
            .into_iter()
            .partition(|m| matches!(m, Message::FocusAction(_)));
        if pointer {
            assert!(
                matches!(focus.as_slice(), [Message::FocusAction(id)] if id == &target),
                "{:?}",
                focus
            );
        } else {
            assert!(focus.is_empty(), "{:?}", focus);
        }
        assert_eq!(rest.len(), 1, "{:?}", rest);
        rest.into_iter().next().unwrap()
    }

    fn assert_expanded_address_actions(ready: &capture::Inspection) {
        assert!(ready.contains("Bitcoin address"));
        assert!(ready.contains(&addr(3).to_string()));
        assert!(ready.contains("Salary · synthetic"));
        assert!(ready.contains("New address"));
        assert!(ready.contains("Generated earlier in this session"));
        for n in [1, 2, 3] {
            assert!(ready.contains(&addr(n).to_string()));
            for control in [Control::Copy, Control::Verify] {
                assert!(
                    ready.has_focus(generated(control, n)),
                    "{} {:?}",
                    n,
                    control
                );
            }
        }
        assert!(!ready.has_focus(generated(Control::Qr, 3)));
        for n in [1, 2] {
            assert!(ready.has_focus(generated(Control::Qr, n)));
        }
        assert!(ready.has_focus(previous_disclosure_id()));
        assert!(ready.has_focus(previous(Control::Toggle, 10)));
        assert!(ready.has_focus(previous(Control::Toggle, 11)));
        // Only the selected previous row discloses its actions.
        for control in [Control::Copy, Control::Qr, Control::Verify] {
            assert!(ready.has_focus(previous(control, 11)));
            assert!(!ready.has_focus(previous(control, 10)));
        }
        assert!(ready.contains(&truncate_middle(&addr(10).to_string())));
        assert!(ready.contains(&addr(11).to_string()));
        assert!(ready.contains("See more"));
        assert!(ready.has_focus(see_more_id()));
    }

    #[test]
    fn receive_states_render_in_both_themes_at_compact_and_wide_sizes() {
        for (appearance, theme) in themes() {
            for outer in [COMPACT, WIDE] {
                let inspect = |scenario: &str, fixture: &Fixture| {
                    capture::inspect(scenario, fixture.view(), outer, appearance, theme)
                };

                let mut f = Fixture::empty();
                f.processing = true;
                let loading = inspect("vault-receive-initial-loading", &f);
                assert!(loading.contains("Loading addresses"));
                assert!(!loading.contains("No addresses yet"));
                assert!(loading.contains("Generate address"));
                assert!(loading.has_focus(generate_id()));

                f.processing = false;
                let empty = inspect("vault-receive-empty", &f);
                assert!(empty.contains("No addresses yet"));
                assert!(!empty.contains("Loading addresses"));
                assert!(empty.has_focus(generate_id()));
                assert!(!empty.contains("Previously generated addresses"));

                let f = Fixture::populated();
                let ready = inspect("vault-receive-generated-previous-expanded", &f);
                assert!(ready.contains("Payment details"));
                assert!(ready.contains("Receiving into"));
                assert!(ready.contains("Vault · Family Vault"));
                assert!(!ready.contains("Vault · Family Cube"));
                assert!(ready.contains("250,000 SATS"));
                assert!(ready.contains("On-chain deposits appear after the Vault syncs."));
                assert!(ready.has_focus(control_id("activity")));
                if outer == COMPACT {
                    let generate = ready
                        .focus
                        .iter()
                        .find(|(id, _)| *id == generate_id())
                        .expect("new address action");
                    let compact_width = ((COMPACT.0 as f32 - 208.0) * 0.8).floor();
                    assert!(
                        generate.1.x + generate.1.width <= compact_width,
                        "new address action exceeds compact content width"
                    );
                    let centered = capture::inspect_wide_content(
                        "vault-receive-centered-960",
                        f.view(),
                        WIDE,
                        appearance,
                        theme,
                        960,
                    );
                    let aside = centered
                        .text
                        .iter()
                        .find(|(value, _)| value == "Receiving into")
                        .expect("receive aside");
                    assert!(aside.1.x >= 720.0, "aside should sit beside the 680px card");
                    let link = centered
                        .text
                        .iter()
                        .find(|(value, _)| value == "See all in Activity")
                        .expect("Activity link");
                    assert_eq!(link.1.x, aside.1.x, "Activity link should align with aside");
                }
                assert_expanded_address_actions(&ready);

                let mut f = Fixture::populated();
                f.show_prev = false;
                let collapsed = inspect("vault-receive-previous-collapsed", &f);
                assert!(collapsed.contains("Previously generated addresses still awaiting deposit"));
                assert!(collapsed.has_focus(previous_disclosure_id()));
                assert!(!collapsed.has_focus(previous(Control::Toggle, 10)));
                assert!(!collapsed.has_focus(see_more_id()));

                let mut f = Fixture::populated();
                f.selected.clear();
                let unselected = inspect("vault-receive-previous-unselected", &f);
                assert!(unselected.has_focus(previous(Control::Toggle, 11)));
                assert!(!unselected.has_focus(previous(Control::Copy, 11)));
                assert!(!unselected.contains(&addr(11).to_string()));

                let mut f = Fixture::populated();
                f.processing = true;
                let fetching = inspect("vault-receive-pagination-processing", &f);
                assert!(fetching.contains("Fetching…"));
                assert!(!fetching.has_focus(see_more_id()));
                // Existing rows stay usable while the next page loads.
                assert!(fetching.has_focus(previous(Control::Copy, 11)));

                let mut f = Fixture::populated();
                f.is_last_page = true;
                let last = inspect("vault-receive-last-page", &f);
                assert!(!last.contains("See more"));
                assert!(!last.has_focus(see_more_id()));

                let mut f = Fixture::populated();
                f.generating = true;
                let generating = inspect("vault-receive-generating", &f);
                assert!(generating.contains("Generating address…"));
                assert!(!generating.has_focus(generate_id()));

                let mut f = Fixture::empty();
                f.prev = vec![addr(10)];
                let only_previous = inspect("vault-receive-previous-only", &f);
                assert!(only_previous.contains("Generate a new address for this deposit."));
                assert!(only_previous.contains("Generate address"));
                assert!(!only_previous.contains("No addresses yet"));
            }
        }
    }

    #[test]
    fn address_actions_send_exact_payloads_and_original_indexes() {
        let f = Fixture::populated();
        for pointer in [false, true] {
            // Generated rows display newest first but keep their state index.
            for (n, index) in [(3u8, 2usize), (2, 1), (1, 0)] {
                assert!(matches!(
                    activate_once(f.view(), generated(Control::Copy, n), pointer),
                    Message::VaultReceive(VaultReceiveMessage::Copy(a)) if a == addr(n).to_string()
                ));
                assert!(matches!(
                    activate_once(f.view(), generated(Control::Verify, n), pointer),
                    Message::Select(i) if i == index
                ));
                if n != 3 {
                    assert!(matches!(
                        activate_once(f.view(), generated(Control::Qr, n), pointer),
                        Message::ShowQrCode(i) if i == index
                    ));
                }
            }
            // Previous rows continue after the generated addresses: the
            // second previous address is row 3 + 1. Its nested actions
            // dispatch once and never toggle the parent selection.
            assert!(matches!(
                activate_once(f.view(), previous(Control::Copy, 11), pointer),
                Message::VaultReceive(VaultReceiveMessage::Copy(a)) if a == addr(11).to_string()
            ));
            assert!(matches!(
                activate_once(f.view(), previous(Control::Verify, 11), pointer),
                Message::Select(4)
            ));
            assert!(matches!(
                activate_once(f.view(), previous(Control::Qr, 11), pointer),
                Message::ShowQrCode(4)
            ));
            assert!(matches!(
                activate_once(f.view(), previous(Control::Toggle, 10), pointer),
                Message::SelectAddress(a) if a == addr(10)
            ));
            assert!(matches!(
                activate_once(f.view(), previous_disclosure_id(), pointer),
                Message::ToggleShowPreviousAddresses
            ));
            assert!(matches!(
                activate_once(f.view(), see_more_id(), pointer),
                Message::Next
            ));
            assert!(matches!(
                activate_once(f.view(), generate_id(), pointer),
                Message::NextReceiveAddress
            ));
            assert!(matches!(
                activate_once(f.view(), control_id("activity"), pointer),
                Message::Menu(Menu::Cube(CubeSubMenu::Activity))
            ));

            // The first previous address is row 3 + 0.
            let mut first = Fixture::populated();
            first.selected = HashSet::from([addr(10)]);
            assert!(matches!(
                activate_once(first.view(), previous(Control::Verify, 10), pointer),
                Message::Select(3)
            ));
            assert!(matches!(
                activate_once(first.view(), previous(Control::Qr, 10), pointer),
                Message::ShowQrCode(3)
            ));
        }
    }

    fn load_fonts() {
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
    }

    fn renderer() -> iced::Renderer {
        load_fonts();
        iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .unwrap()
    }

    /// The dashboard's measured content box at the compact window size,
    /// inside the vertical scrollable the dashboard wraps every route in.
    fn viewport() -> Size {
        let (w, h) = COMPACT;
        Size::new(((w as f32 - 208.0) * 0.8).floor(), (h - 100) as f32)
    }

    fn scrolled(content: Element<'_, Message>) -> Element<'_, Message> {
        iced::widget::scrollable(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn relayout(
        element: &mut Element<'_, Message>,
        tree: &mut Tree,
        renderer: &iced::Renderer,
    ) -> layout::Node {
        element
            .as_widget_mut()
            .layout(tree, renderer, &layout::Limits::new(Size::ZERO, viewport()))
    }

    fn operate<T: 'static>(
        element: &mut Element<'_, Message>,
        tree: &mut Tree,
        node: &layout::Node,
        renderer: &iced::Renderer,
        operation: impl Operation<T> + 'static,
    ) -> Option<T> {
        let mut operation: Box<dyn Operation<T>> = Box::new(operation);
        loop {
            element.as_widget_mut().operate(
                tree,
                Layout::new(node),
                renderer,
                &mut operation::black_box(operation.as_mut()),
            );
            match operation.finish() {
                operation::Outcome::None => return None,
                operation::Outcome::Some(value) => return Some(value),
                operation::Outcome::Chain(next) => operation = next,
            }
        }
    }

    fn focused(
        element: &mut Element<'_, Message>,
        tree: &mut Tree,
        node: &layout::Node,
        renderer: &iced::Renderer,
    ) -> Option<Id> {
        operate(element, tree, node, renderer, focusable::find_focused())
    }

    fn key(named: keyboard::key::Named, pressed: bool) -> Event {
        let key = keyboard::Key::Named(named);
        let physical_key =
            keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
        if pressed {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key,
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: None,
                repeat: false,
            })
        } else {
            Event::Keyboard(keyboard::Event::KeyReleased {
                key: key.clone(),
                modified_key: key,
                physical_key,
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
            })
        }
    }

    fn send(
        element: &mut Element<'_, Message>,
        tree: &mut Tree,
        node: &layout::Node,
        renderer: &iced::Renderer,
        event: Event,
    ) -> Vec<Message> {
        let mut messages = Vec::new();
        element.as_widget_mut().update(
            tree,
            &event,
            Layout::new(node),
            mouse::Cursor::Unavailable,
            renderer,
            &mut iced::advanced::clipboard::Null,
            &mut iced::advanced::Shell::new(&mut messages),
            &Rectangle::with_size(viewport()),
        );
        messages
    }

    /// Tab order through the populated view, in visual order.
    fn populated_tab_order() -> Vec<Id> {
        let mut order = Vec::new();
        // Current address card, then the card footer.
        order.extend([
            generated(Control::Copy, 3),
            generated(Control::Verify, 3),
            generate_id(),
            control_id("activity"),
        ]);
        // Earlier generated addresses, newest first.
        for n in [2, 1] {
            order.extend([
                generated(Control::Copy, n),
                generated(Control::Qr, n),
                generated(Control::Verify, n),
            ]);
        }
        order.extend([
            previous_disclosure_id(),
            previous(Control::Toggle, 10),
            previous(Control::Toggle, 11),
            previous(Control::Copy, 11),
            previous(Control::Qr, 11),
            previous(Control::Verify, 11),
            see_more_id(),
        ]);
        order
    }

    #[test]
    fn tab_and_shift_tab_reach_every_enabled_control_through_the_scrollable() {
        let renderer = renderer();
        let f = Fixture::populated();
        let mut element = scrolled(f.view());
        let mut tree = Tree::new(element.as_widget());
        let node = relayout(&mut element, &mut tree, &renderer);
        let expected = populated_tab_order();

        // The content overflows the compact viewport, so later controls are
        // only reachable through the scrollable's traversal.
        assert!(node.children()[0].size().height > viewport().height);
        let mut forward = Vec::new();
        for _ in 0..expected.len() {
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus_next::<()>(),
            );
            forward.push(focused(&mut element, &mut tree, &node, &renderer).unwrap());
        }
        assert_eq!(forward, expected);

        let mut backward = vec![focused(&mut element, &mut tree, &node, &renderer).unwrap()];
        for _ in 1..expected.len() {
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus_previous::<()>(),
            );
            backward.push(focused(&mut element, &mut tree, &node, &renderer).unwrap());
        }
        backward.reverse();
        assert_eq!(backward, expected);
    }

    #[test]
    fn disabled_generate_and_see_more_are_not_in_the_focus_order() {
        let renderer = renderer();
        let mut f = Fixture::populated();
        f.generating = true;
        f.processing = true;
        let mut element = scrolled(f.view());
        let mut tree = Tree::new(element.as_widget());
        let node = relayout(&mut element, &mut tree, &renderer);
        let expected: Vec<Id> = populated_tab_order()
            .into_iter()
            .filter(|id| id != &generate_id() && id != &see_more_id())
            .collect();
        let mut forward = Vec::new();
        for _ in 0..expected.len() {
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus_next::<()>(),
            );
            forward.push(focused(&mut element, &mut tree, &node, &renderer).unwrap());
        }
        assert_eq!(forward, expected);
        // Past the last enabled control nothing else takes focus.
        operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::focus_next::<()>(),
        );
        assert_eq!(focused(&mut element, &mut tree, &node, &renderer), None);
    }

    #[test]
    fn enter_and_space_activate_the_focused_control_once() {
        let renderer = renderer();
        let f = Fixture::populated();
        let mut element = scrolled(f.view());
        let mut tree = Tree::new(element.as_widget());
        let node = relayout(&mut element, &mut tree, &renderer);

        for (named, target) in [
            (keyboard::key::Named::Space, generate_id()),
            (keyboard::key::Named::Enter, previous(Control::Copy, 11)),
            (keyboard::key::Named::Space, control_id("activity")),
        ] {
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus::<()>(target.clone()),
            );
            assert_eq!(
                focused(&mut element, &mut tree, &node, &renderer),
                Some(target)
            );
            let pressed = send(&mut element, &mut tree, &node, &renderer, key(named, true));
            assert_eq!(pressed.len(), 1, "{:?}", pressed);
            // Holding the key does not repeat the action.
            assert!(send(&mut element, &mut tree, &node, &renderer, key(named, true)).is_empty());
            send(&mut element, &mut tree, &node, &renderer, key(named, false));
            match (named, &pressed[0]) {
                (keyboard::key::Named::Space, Message::NextReceiveAddress) => {}
                (
                    keyboard::key::Named::Enter,
                    Message::VaultReceive(VaultReceiveMessage::Copy(a)),
                ) if a == &addr(11).to_string() => {}
                (keyboard::key::Named::Space, Message::Menu(Menu::Cube(CubeSubMenu::Activity))) => {
                }
                other => panic!("unexpected activation {:?}", other),
            }
        }
    }

    /// Re-render after a state change and reconcile the same widget tree, as
    /// the runtime does between frames.
    fn rerender<'a>(
        tree: &mut Tree,
        content: Element<'a, Message>,
        renderer: &iced::Renderer,
    ) -> (Element<'a, Message>, layout::Node) {
        let mut element = scrolled(content);
        tree.diff(element.as_widget());
        let node = relayout(&mut element, tree, renderer);
        (element, node)
    }

    #[test]
    fn focus_identity_survives_pagination_and_disclosure() {
        let renderer = renderer();

        // A new page appends rows; the focused row keeps its identity.
        let before = Fixture::populated();
        let mut after = Fixture::populated();
        after.prev.extend([addr(12), addr(13)]);
        let target = previous(Control::Copy, 11);
        let mut tree = {
            let mut element = scrolled(before.view());
            let mut tree = Tree::new(element.as_widget());
            let node = relayout(&mut element, &mut tree, &renderer);
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus::<()>(target.clone()),
            );
            tree
        };
        let (mut element, node) = rerender(&mut tree, after.view(), &renderer);
        assert_eq!(
            focused(&mut element, &mut tree, &node, &renderer),
            Some(target)
        );
        let pressed = send(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            key(keyboard::key::Named::Enter, true),
        );
        assert!(matches!(
            pressed.as_slice(),
            [Message::VaultReceive(VaultReceiveMessage::Copy(a))] if a == &addr(11).to_string()
        ));

        // Expanding the previous list keeps focus on its disclosure.
        let mut closed = Fixture::populated();
        closed.show_prev = false;
        let open = Fixture::populated();
        let mut tree = {
            let mut element = scrolled(closed.view());
            let mut tree = Tree::new(element.as_widget());
            let node = relayout(&mut element, &mut tree, &renderer);
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus::<()>(previous_disclosure_id()),
            );
            tree
        };
        let (mut element, node) = rerender(&mut tree, open.view(), &renderer);
        assert_eq!(
            focused(&mut element, &mut tree, &node, &renderer),
            Some(previous_disclosure_id())
        );

        // Selecting a previous row keeps focus on that row's toggle.
        let mut unselected = Fixture::populated();
        unselected.selected.clear();
        let mut selected = Fixture::populated();
        selected.selected = HashSet::from([addr(10)]);
        let toggle = previous(Control::Toggle, 10);
        let mut tree = {
            let mut element = scrolled(unselected.view());
            let mut tree = Tree::new(element.as_widget());
            let node = relayout(&mut element, &mut tree, &renderer);
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus::<()>(toggle.clone()),
            );
            tree
        };
        let (mut element, node) = rerender(&mut tree, selected.view(), &renderer);
        assert_eq!(
            focused(&mut element, &mut tree, &node, &renderer),
            Some(toggle)
        );
    }

    #[test]
    fn verify_modal_shows_device_states_truthfully_and_its_controls_work() {
        let address = addr(3);
        let index = ChildNumber::from_normal_idx(7).unwrap();
        let chosen = HashSet::new();
        let copy = address_control_id(Scope::VerifyModal, Control::Copy, &address.to_string());
        let unsupported = [HardwareWallet::Unsupported {
            id: "synthetic-device".into(),
            kind: async_hwi::DeviceKind::Ledger,
            version: None,
            reason: crate::hw::UnsupportedReason::WrongNetwork,
        }];
        for (appearance, theme) in themes() {
            for outer in [COMPACT, WIDE] {
                let none = capture::inspect(
                    "vault-receive-verify-no-device",
                    verify_address_modal(
                        crate::chain::ChainId::Bitcoin,
                        &[],
                        &chosen,
                        &address,
                        &index,
                    ),
                    outer,
                    appearance,
                    theme,
                );
                assert!(none.contains("No hardware device detected"));
                assert!(none.contains(&address.to_string()));
                assert!(none.contains("Derivation index"));
                assert!(none.text.iter().any(|(value, _)| value == "7"));
                assert!(none.has_focus(verify_close_id()));
                assert!(none.has_focus(copy.clone()));
                // Nothing claims the address was verified.
                assert!(!none.contains("Matches"));
                assert!(!none.contains("Verified"));

                let device = capture::inspect(
                    "vault-receive-verify-unavailable-device",
                    verify_address_modal(
                        crate::chain::ChainId::Bitcoin,
                        &unsupported,
                        &chosen,
                        &address,
                        &index,
                    ),
                    outer,
                    appearance,
                    theme,
                );
                assert!(!device.contains("No hardware device detected"));
                assert!(device.has_focus(verify_close_id()));
            }
        }
        for pointer in [false, true] {
            assert!(matches!(
                activate_once(
                    verify_address_modal(crate::chain::ChainId::Bitcoin, &[], &chosen, &address, &index),
                    copy.clone(),
                    pointer
                ),
                Message::VaultReceive(VaultReceiveMessage::Copy(a)) if a == address.to_string()
            ));
            assert!(matches!(
                activate_once(
                    verify_address_modal(
                        crate::chain::ChainId::Bitcoin,
                        &[],
                        &chosen,
                        &address,
                        &index
                    ),
                    verify_close_id(),
                    pointer
                ),
                Message::Close
            ));
        }
    }

    #[test]
    fn spanish_vault_address_review_preserves_address_network_and_index() {
        use coincube_ui::locale::{render_in, Language};

        let address = addr(3);
        let index = ChildNumber::from_normal_idx(7).unwrap();
        let chosen = HashSet::new();
        for (appearance, theme) in themes() {
            let view = render_in(Language::Spanish, || {
                verify_address_modal(
                    crate::chain::ChainId::Bitcoin,
                    &[],
                    &chosen,
                    &address,
                    &index,
                )
            });
            let rendered =
                capture::inspect("vault-address-review-es", view, COMPACT, appearance, theme);
            assert!(rendered.contains("Bitcoin SHA-256 · Red principal"));
            assert!(rendered.contains("Dirección"));
            assert!(rendered.contains("No se detectó ningún dispositivo de firma"));
            assert!(rendered.contains(&address.to_string()));
            assert!(rendered.text.iter().any(|(value, _)| value == "7"));
        }
    }

    #[test]
    fn qr_modal_closes_through_its_owner() {
        let address = addr(3);
        let text = address.to_string();
        let data = qr_code::Data::new(format!("bitcoin:{}?index={}", address, 7)).unwrap();
        for (appearance, theme) in themes() {
            for outer in [COMPACT, WIDE] {
                let qr = capture::inspect(
                    "vault-receive-qr",
                    qr_modal(&data, &text),
                    outer,
                    appearance,
                    theme,
                );
                assert!(qr.contains("Scan to pay this address"));
                assert!(qr.has_focus(qr_close_id()));
            }
        }
        for pointer in [false, true] {
            assert!(matches!(
                activate_once(qr_modal(&data, &text), qr_close_id(), pointer),
                Message::Close
            ));
        }
    }

    /// Capture the production QRCode widget without a modal or scrollable so
    /// translations of its own geometry can be compared on the same backend.
    /// The original PNGs and manifest are inputs to an external exact-payload
    /// decoder; rendering alone does not establish that a QR is scannable.
    #[test]
    #[ignore = "requires R08A_QR_CAPTURE_DIR and external exact-payload decoding"]
    fn qr_widget_tiny_skia_translation_captures() {
        let directory = std::path::PathBuf::from(
            std::env::var_os("R08A_QR_CAPTURE_DIR")
                .expect("set R08A_QR_CAPTURE_DIR to a disposable capture directory"),
        );
        std::fs::create_dir_all(&directory).expect("create QR capture directory");

        let payload = format!("bitcoin:{}?index={}", addr(3), 7);
        let data = qr_code::Data::new(payload.as_bytes()).expect("encode synthetic receive URI");
        let theme = theme::Theme::light();
        let framebuffer = (640_u32, 640_u32);
        let cell_size = 8_u32;
        let positions = [(0_u32, 0_u32), (36, 0), (0, 89), (36, 89)];
        let mut captures = Vec::new();

        for (x, y) in positions {
            let mut renderer = renderer();
            // Use the same production widget and Data cache as qr_modal.
            let mut element: Element<'_, Message> = QRCode::<coincube_ui::theme::Theme>::new(&data)
                .cell_size(cell_size)
                .into();
            let mut tree = Tree::new(element.as_widget());
            let mut node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(
                    Size::ZERO,
                    Size::new(framebuffer.0 as f32, framebuffer.1 as f32),
                ),
            );
            node.move_to_mut(iced::Point::new(x as f32, y as f32));
            let bounds = Layout::new(&node).bounds();
            assert_eq!(bounds.x, x as f32, "mounted QR x at ({}, {})", x, y);
            assert_eq!(bounds.y, y as f32, "mounted QR y at ({}, {})", x, y);
            assert_eq!(bounds.width, bounds.height, "square QR at ({}, {})", x, y);
            assert!(bounds.width > 0.0, "nonempty QR at ({}, {})", x, y);
            assert_eq!(bounds.width % cell_size as f32, 0.0);
            assert!(bounds.x + bounds.width < framebuffer.0 as f32);
            assert!(bounds.y + bounds.height < framebuffer.1 as f32);

            let viewport =
                Rectangle::with_size(Size::new(framebuffer.0 as f32, framebuffer.1 as f32));
            element.as_widget().draw(
                &tree,
                &mut renderer,
                &theme,
                &iced::advanced::renderer::Style {
                    text_color: theme.colors.text.primary,
                },
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &viewport,
            );
            let pixels = renderer.screenshot(
                Size::new(framebuffer.0, framebuffer.1),
                1.0,
                iced::Color::WHITE,
            );
            let filename = format!("qr-widget-tiny-skia-x{}-y{}.png", x, y);
            image::save_buffer(
                directory.join(&filename),
                &pixels,
                framebuffer.0,
                framebuffer.1,
                image::ColorType::Rgba8,
            )
            .expect("save unmodified QR renderer pixels");
            captures.push(serde_json::json!({
                "position": {"x": x, "y": y},
                "mounted_widget_bounds": {
                    "x": bounds.x, "y": bounds.y,
                    "width": bounds.width, "height": bounds.height
                },
                "data_cells_per_side": bounds.width as u32 / cell_size - 4,
                "png": filename
            }));
        }

        let manifest = serde_json::json!({
            "payload": payload,
            "widget": "iced_widget::qr_code::QRCode via iced::widget::qr_code",
            "backend": "tiny-skia",
            "cell_size_px": cell_size,
            "framebuffer_px": {"width": framebuffer.0, "height": framebuffer.1},
            "background": "#ffffff",
            "captures": captures
        });
        std::fs::write(
            directory.join("qr-widget-tiny-skia-manifest.json"),
            serde_json::to_vec_pretty(&manifest).expect("serialize QR capture manifest"),
        )
        .expect("save QR capture manifest");
    }

    fn qr_translation_mismatches(
        physical_size: Size<u32>,
        pixels: &[u8],
        original: &[u8],
        shift_x: u32,
        shift_y: u32,
    ) -> (u64, Option<(u32, u32)>) {
        let mut mismatches = 0_u64;
        let mut first_mismatch = None;
        for py in 0..physical_size.height {
            for px in 0..physical_size.width {
                let offset = ((py * physical_size.width + px) * 4) as usize;
                let actual = &pixels[offset..offset + 4];
                let expected = if px >= shift_x && py >= shift_y {
                    let original_offset =
                        (((py - shift_y) * physical_size.width + (px - shift_x)) * 4) as usize;
                    &original[original_offset..original_offset + 4]
                } else {
                    &[255, 255, 255, 255]
                };
                if actual != expected {
                    mismatches += 1;
                    first_mismatch.get_or_insert((px, py));
                }
            }
        }
        (mismatches, first_mismatch)
    }

    /// A QR's cached geometry must move with its widget without losing any
    /// modules. Keep the same Data and widget tree through every move, then
    /// compare the actual backend pixels with the origin frame translated by
    /// the same physical offset. The optional captures permit independent
    /// exact-payload decoding without changing the original diagnostic.
    #[test]
    fn qr_widget_tiny_skia_cached_translation_preserves_pixels() {
        use iced::advanced::Renderer as _;

        let payload = format!("bitcoin:{}?index={}", addr(3), 7);
        let data = qr_code::Data::new(payload.as_bytes()).expect("encode synthetic receive URI");
        let theme = theme::Theme::light();
        let logical_size = Size::new(640_u32, 640_u32);
        let logical_bounds = Rectangle::with_size(Size::new(640.0, 640.0));
        let positions = [(0_u32, 0_u32), (36, 0), (0, 89), (36, 89), (0, 0)];
        let capture_dir =
            std::env::var_os("R08A_QR_REGRESSION_CAPTURE_DIR").map(std::path::PathBuf::from);
        if let Some(directory) = &capture_dir {
            std::fs::create_dir_all(directory).expect("create regression capture directory");
        }

        // One widget tree and one QR Data/cache are used for every position
        // and scale. Reusing the renderer exercises its reset/redraw path;
        // Headless::screenshot still paints full damage, not incremental damage.
        let mut element: Element<'_, Message> = QRCode::<coincube_ui::theme::Theme>::new(&data)
            .cell_size(8)
            .into();
        let mut tree = Tree::new(element.as_widget());
        let mut renderer = renderer();
        assert_eq!(renderer.name(), "tiny-skia");
        let mut manifest = Vec::new();
        let mut comparisons = Vec::new();

        for scale in [1_u32, 2] {
            let physical_size = Size::new(logical_size.width * scale, logical_size.height * scale);
            let mut origin = None;

            for (step, (x, y)) in positions.iter().copied().enumerate() {
                renderer.reset(logical_bounds);
                let mut node = element.as_widget_mut().layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(Size::ZERO, logical_bounds.size()),
                );
                node.move_to_mut(iced::Point::new(x as f32, y as f32));
                let bounds = Layout::new(&node).bounds();
                assert_eq!(bounds.position(), iced::Point::new(x as f32, y as f32));
                assert_eq!(bounds.size(), Size::new(328.0, 328.0));
                assert!(bounds.x + bounds.width < logical_bounds.width);
                assert!(bounds.y + bounds.height < logical_bounds.height);

                element.as_widget().draw(
                    &tree,
                    &mut renderer,
                    &theme,
                    &iced::advanced::renderer::Style {
                        text_color: theme.colors.text.primary,
                    },
                    Layout::new(&node),
                    mouse::Cursor::Unavailable,
                    &logical_bounds,
                );
                let pixels = renderer.screenshot(physical_size, scale as f32, iced::Color::WHITE);
                assert_eq!(
                    pixels.len(),
                    (physical_size.width * physical_size.height * 4) as usize
                );

                if let Some(directory) = &capture_dir {
                    let filename = format!(
                        "qr-widget-cache-scale{}-step{}-x{}-y{}.png",
                        scale, step, x, y
                    );
                    image::save_buffer(
                        directory.join(&filename),
                        &pixels,
                        physical_size.width,
                        physical_size.height,
                        image::ColorType::Rgba8,
                    )
                    .expect("save unmodified regression renderer pixels");
                    manifest.push(serde_json::json!({
                        "scale_factor": scale,
                        "step": step,
                        "position": {"x": x, "y": y},
                        "mounted_widget_bounds": {
                            "x": bounds.x, "y": bounds.y,
                            "width": bounds.width, "height": bounds.height
                        },
                        "png": filename
                    }));
                }

                if step == 0 {
                    let dark_pixels = pixels
                        .chunks_exact(4)
                        .filter(|pixel| pixel[0] < 32 && pixel[1] < 32 && pixel[2] < 32)
                        .count();
                    assert!(dark_pixels > 1000, "origin QR has visible dark modules");
                    origin = Some(pixels);
                    continue;
                }
                let original = origin.as_ref().expect("origin screenshot");
                let shift_x = x * scale;
                let shift_y = y * scale;
                let (mismatches, first_mismatch) =
                    qr_translation_mismatches(physical_size, &pixels, original, shift_x, shift_y);
                comparisons.push((scale, step, x, y, mismatches, first_mismatch));
            }
        }

        if let Some(directory) = &capture_dir {
            let details = serde_json::json!({
                "payload": payload,
                "widget": "iced_widget::qr_code::QRCode via iced::widget::qr_code",
                "backend": "tiny-skia",
                "cell_size_logical_px": 8,
                "logical_framebuffer_px": {"width": 640, "height": 640},
                "background": "#ffffff",
                "captures": manifest
            });
            std::fs::write(
                directory.join("qr-widget-cache-regression-manifest.json"),
                serde_json::to_vec_pretty(&details).expect("serialize regression manifest"),
            )
            .expect("save regression manifest");
        }

        for (scale, step, x, y, mismatches, first_mismatch) in comparisons {
            assert_eq!(
                mismatches, 0,
                "QR pixels differ from origin shifted to ({x},{y}) at {scale}x, step {step}; first mismatch: {first_mismatch:?}"
            );
        }
    }

    /// sRGB relative luminance, as WCAG 2.x defines it.
    fn luminance(color: iced::Color) -> f32 {
        let linear = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }

    fn contrast(a: iced::Color, b: iced::Color) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// `top` composited over an opaque `bottom`.
    fn over(top: iced::Color, bottom: iced::Color) -> iced::Color {
        let mix = |t: f32, b: f32| t * top.a + b * (1.0 - top.a);
        iced::Color::from_rgb(
            mix(top.r, bottom.r),
            mix(top.g, bottom.g),
            mix(top.b, bottom.b),
        )
    }

    /// The label against what is actually behind it: the button background
    /// composited over the card's `shell.panel`.
    fn label_contrast(style: iced_button::Style, panel: iced::Color) -> f32 {
        let background = match style.background {
            Some(iced::Background::Color(color)) => over(color, panel),
            None => panel,
            Some(other) => panic!("unexpected button background {:?}", other),
        };
        contrast(over(style.text_color, background), background)
    }

    #[test]
    fn new_address_label_is_readable_on_the_card_in_both_themes() {
        use iced_button::Status;
        // The production selector for the card footer once an address exists.
        let style = generate_style(true);
        for (appearance, th) in themes() {
            let panel = th.colors.shell.panel;
            for status in [Status::Active, Status::Hovered, Status::Pressed] {
                let ratio = label_contrast(style(&th, status), panel);
                assert!(ratio >= 4.5, "{} {:?}: {:.3}:1", appearance, status, ratio);
            }
            // Only the resting label changes; disabled keeps its palette and
            // alpha.
            assert_eq!(
                style(&th, Status::Active),
                iced_button::Style {
                    text_color: th.colors.shell.accent_ink,
                    ..theme::button::orange_outline(&th, Status::Active)
                },
                "{}",
                appearance
            );
            for status in [Status::Hovered, Status::Pressed, Status::Disabled] {
                assert_eq!(
                    style(&th, status),
                    theme::button::orange_outline(&th, status),
                    "{} {:?}",
                    appearance,
                    status
                );
            }
            // Before any address exists the footer keeps the primary style.
            for status in [
                Status::Active,
                Status::Hovered,
                Status::Pressed,
                Status::Disabled,
            ] {
                assert_eq!(
                    generate_style(false)(&th, status),
                    theme::button::primary(&th, status),
                    "{} {:?}",
                    appearance,
                    status
                );
            }
        }
        // The shared resting style this replaces fails on the light card.
        let light = theme::Theme::light();
        let old = label_contrast(
            theme::button::orange_outline(&light, Status::Active),
            light.colors.shell.panel,
        );
        assert!(old < 4.5, "{:.3}:1", old);
    }
}
