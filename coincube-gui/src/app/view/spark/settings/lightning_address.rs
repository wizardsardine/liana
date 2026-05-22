//! Lightning Address claim / manage UX.
//!
//! Renders the per-Cube Lightning Address sub-page: empty-state claim
//! form when no address is bound, full-page display + "Change" button
//! when bound, and an in-place edit form when the user is renaming.
//! The "Change your Lightning Address?" confirmation lives here as a
//! modal stacked on the page.
//!
//! Owns `LN_ADDRESS_DOMAIN`, the only string in the codebase that must
//! match the Connect backend's `lightningAddressDomain`.
//!
//! State and message types are unchanged from the per-Cube Connect
//! location this UX was lifted from — the form still reads
//! [`ConnectCubePanel`] and emits [`ConnectCubeMessage`] so the
//! existing dispatch path through `ConnectPanel::update` keeps
//! working.

use coincube_ui::{
    color,
    component::{button, text},
    icon::clipboard_icon,
    theme,
    widget::{modal::Modal, Column, ColumnExt, Element, Row, TextInput},
};
use iced::{
    widget::container,
    Alignment, Length,
};

use crate::app::{state::connect::ConnectCubePanel, view::ConnectCubeMessage};

/// Domain suffix displayed in the Lightning Address claim form.
/// Must match the backend's `lightningAddressDomain`.
pub(crate) const LN_ADDRESS_DOMAIN: &str = "@coincube.io";

fn card_style(t: &theme::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(t.colors.cards.simple.background)),
        border: iced::Border {
            color: t.colors.cards.simple.border.unwrap_or(color::GREY_5),
            width: 0.2,
            radius: 16.0.into(),
        },
        ..Default::default()
    }
}

pub fn lightning_address_ux<'a>(state: &'a ConnectCubePanel) -> Element<'a, ConnectCubeMessage> {
    let current_address = state
        .lightning_address
        .as_ref()
        .and_then(|la| la.lightning_address.clone())
        .map(|mut a| {
            if !a.contains('@') {
                a.push_str(LN_ADDRESS_DOMAIN);
            }
            a
        });
    let has_address = current_address.is_some();
    let current_username = current_address
        .as_deref()
        .and_then(|a| a.split('@').next())
        .map(|u| u.to_string());

    let card_content: Element<ConnectCubeMessage> = if has_address && state.ln_editing {
        // Edit mode: in-place rename form on the claimed-address card.
        let address = current_address.clone().unwrap_or_default();
        let username = &state.ln_username_input;
        let format_ok = state.ln_username_error.is_none() && !username.is_empty();
        let available = state.ln_username_available == Some(true);
        let differs = current_username
            .as_deref()
            .map(|u| u != username.as_str())
            .unwrap_or(true);
        let can_change = format_ok && available && differs && !state.ln_changing;

        let status: Element<ConnectCubeMessage> = if state.ln_checking {
            text::p2_regular("Checking…").color(color::GREY_3).into()
        } else if let Some(err) = &state.ln_username_error {
            text::p2_regular(err.as_str()).color(color::RED).into()
        } else if !differs && !username.is_empty() {
            text::p2_regular("This is your current username")
                .color(color::GREY_3)
                .into()
        } else if state.ln_username_available == Some(true) {
            text::p2_regular("✓ Available").color(color::GREEN).into()
        } else if username.is_empty() {
            text::p2_regular("Choose a new username")
                .color(color::GREY_3)
                .into()
        } else {
            text::p2_regular(" ").into()
        };

        let cancel_btn = button::secondary(None, "Cancel").on_press_maybe(
            (!state.ln_changing).then_some(ConnectCubeMessage::CancelEditLightningAddress),
        );
        let change_btn: Element<ConnectCubeMessage> = if state.ln_changing {
            iced::widget::button(
                container(text::p1_regular("Changing…").color(color::GREY_3))
                    .center_x(Length::Fixed(140.0))
                    .center_y(Length::Fill),
            )
            .height(Length::Fixed(44.0))
            .style(theme::button::primary)
            .into()
        } else {
            button::primary(None, "Change")
                .on_press_maybe(
                    can_change.then_some(ConnectCubeMessage::RequestChangeLightningAddress),
                )
                .into()
        };

        container(
            Column::new()
                .push(text::p1_bold("Your Lightning Address").style(theme::text::primary))
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(text::p2_regular(format!("Current: {}", address)).color(color::GREY_3))
                .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                .push(
                    Row::new()
                        .push(
                            TextInput::new("new username", username)
                                .on_input_maybe(
                                    (!state.ln_changing)
                                        .then_some(ConnectCubeMessage::LnUsernameChanged),
                                )
                                .on_submit_maybe(
                                    can_change.then_some(
                                        ConnectCubeMessage::RequestChangeLightningAddress,
                                    ),
                                )
                                .size(16)
                                .padding(15),
                        )
                        .push(
                            container(text::p1_regular(LN_ADDRESS_DOMAIN).color(color::GREY_3))
                                .padding(15)
                                .center_y(Length::Fixed(50.0)),
                        )
                        .align_y(Alignment::Center),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
                .push(status)
                .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                .push(
                    Row::new()
                        .push(cancel_btn)
                        .push(iced::widget::Space::new().width(Length::Fill))
                        .push(change_btn)
                        .align_y(Alignment::Center),
                )
                .push_maybe(state.ln_claim_error.as_deref().map(|err| {
                    Column::new()
                        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                        .push(text::p2_regular(err).color(color::RED))
                }))
                .padding(20)
                .spacing(2),
        )
        .style(|t| container::Style {
            background: Some(iced::Background::Color(t.colors.cards.simple.background)),
            border: iced::Border {
                color: color::ORANGE,
                width: 0.5,
                radius: 16.0.into(),
            },
            ..Default::default()
        })
        .width(Length::Fill)
        .into()
    } else if has_address {
        // Display the claimed address
        let address = current_address.clone().unwrap_or_default();

        container(
            Column::new()
                .push(text::p1_bold("Your Lightning Address").style(theme::text::primary))
                .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                .push(
                    container(
                        Row::new()
                            .push(text::h3(address.clone()).color(color::ORANGE))
                            .push(iced::widget::Space::new().width(Length::Fill))
                            .push(
                                button::secondary(Some(clipboard_icon()), "Copy")
                                    .on_press(ConnectCubeMessage::CopyToClipboard(address)),
                            )
                            .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
                            .push(
                                button::secondary(None, "Change")
                                    .on_press(ConnectCubeMessage::BeginEditLightningAddress),
                            )
                            .align_y(Alignment::Center),
                    )
                    .padding(16)
                    .width(Length::Fill),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    text::p2_regular(
                        "Anyone can send you bitcoin using this address. \
                         This app must be open and active to receive payments.",
                    )
                    .color(color::GREY_3),
                )
                .push_maybe(state.ln_reconcile_needs_reregister.as_deref().map(|err| {
                    // API↔SDK divergence — the DB confirms the
                    // address but the Spark SDK isn't bound to it on
                    // this device. The retry button fires
                    // `RetryLightningAddressReregister`, which does
                    // an idempotent SDK delete followed by a fresh
                    // register against the DB-confirmed username.
                    let retry_label = if state.ln_reregistering {
                        "Retrying…"
                    } else {
                        "Retry"
                    };
                    Column::new()
                        .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                        .push(
                            text::p2_bold("Lightning address needs re-registration")
                                .color(color::RED),
                        )
                        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
                        .push(text::p2_regular(err).color(color::GREY_3))
                        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                        .push(
                            button::secondary(None, retry_label)
                                .on_press_maybe((!state.ln_reregistering).then_some(
                                    ConnectCubeMessage::RetryLightningAddressReregister,
                                )),
                        )
                }))
                .padding(20)
                .spacing(2),
        )
        .style(|t| container::Style {
            background: Some(iced::Background::Color(t.colors.cards.simple.background)),
            border: iced::Border {
                color: color::ORANGE,
                width: 0.5,
                radius: 16.0.into(),
            },
            ..Default::default()
        })
        .width(Length::Fill)
        .into()
    } else {
        // Claim form
        let username = &state.ln_username_input;
        let is_valid = state.ln_username_error.is_none() && !username.is_empty();
        let is_available = state.ln_username_available == Some(true);
        let can_claim = is_valid && is_available && !state.ln_claiming;

        // Status indicator
        let status: Element<ConnectCubeMessage> = if state.ln_checking {
            text::p2_regular("Checking…").color(color::GREY_3).into()
        } else if let Some(err) = &state.ln_username_error {
            text::p2_regular(err.as_str()).color(color::RED).into()
        } else if state.ln_username_available == Some(true) {
            text::p2_regular("✓ Available").color(color::GREEN).into()
        } else if username.is_empty() {
            text::p2_regular("Choose a username for your Lightning Address")
                .color(color::GREY_3)
                .into()
        } else {
            // Waiting for debounce
            text::p2_regular(" ").into()
        };

        let claim_button: Element<ConnectCubeMessage> = if state.ln_claiming {
            iced::widget::button(
                container(text::p1_regular("Claiming…").color(color::GREY_3))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fixed(44.0))
            .style(theme::button::primary)
            .into()
        } else {
            button::primary(None, "Claim Lightning Address")
                .on_press_maybe(can_claim.then_some(ConnectCubeMessage::ClaimLightningAddress))
                .width(Length::Fill)
                .into()
        };

        container(
            Column::new()
                .push(text::p1_bold("Claim Your Lightning Address").color(color::ORANGE))
                .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
                .push(
                    text::p2_regular(
                        "Get a free Lightning Address to receive bitcoin from anyone.",
                    )
                    .color(color::GREY_3),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(16.0)))
                .push(
                    Row::new()
                        .push(
                            TextInput::new("satoshi", username)
                                .on_input(ConnectCubeMessage::LnUsernameChanged)
                                .on_submit_maybe(
                                    can_claim.then_some(ConnectCubeMessage::ClaimLightningAddress),
                                )
                                .size(16)
                                .padding(15),
                        )
                        .push(
                            container(text::p1_regular(LN_ADDRESS_DOMAIN).color(color::GREY_3))
                                .padding(15)
                                .center_y(Length::Fixed(50.0)),
                        )
                        .align_y(Alignment::Center),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
                .push(status)
                .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                .push(claim_button)
                .push_maybe(state.ln_claim_error.as_deref().map(|err| {
                    let mut col = Column::new()
                        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                        .push(text::p2_regular(err).color(color::RED));
                    if state.registration_error.is_some() {
                        col = col
                            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                            .push(
                                button::primary(None, "Retry Connection")
                                    .on_press(ConnectCubeMessage::RetryRegistration)
                                    .width(Length::Fill),
                            );
                    }
                    col
                }))
                .padding(20)
                .spacing(2),
        )
        .style(card_style)
        .width(Length::Fill)
        .into()
    };

    let body: Element<ConnectCubeMessage> = Column::new()
        .push(text::h4_bold("Lightning Address").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(card_content)
        .spacing(0)
        .width(Length::Fill)
        .into();

    if let Some(proposed) = state.ln_change_confirm_pending.clone() {
        let current_addr = current_address.clone().unwrap_or_default();
        let new_addr = format!("{}{}", proposed, LN_ADDRESS_DOMAIN);
        let confirm_card = container(
            Column::new()
                .push(text::h4_bold("Change your Lightning Address?").color(color::ORANGE))
                .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                .push(text::p1_regular(format!(
                    "Your current address {} will stop working immediately.",
                    current_addr
                )))
                .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
                .push(text::p1_regular(
                    "Anyone using the old address won't be able to send you bitcoin.",
                ))
                .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
                .push(text::p1_regular(format!(
                    "You can claim a different name later, but {} may be taken \
                     by someone else once you release it.",
                    current_addr
                )))
                .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                .push(text::p1_bold(format!("New address: {}", new_addr)).color(color::ORANGE))
                .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
                .push(
                    Row::new()
                        .push(
                            button::secondary(None, "Cancel")
                                .on_press(ConnectCubeMessage::DismissChangeConfirmation),
                        )
                        .push(iced::widget::Space::new().width(Length::Fill))
                        .push(
                            button::primary(None, "Yes, change it")
                                .on_press(ConnectCubeMessage::ConfirmChangeLightningAddress),
                        )
                        .align_y(Alignment::Center),
                )
                .padding(24)
                .spacing(0),
        )
        .style(card_style)
        .width(Length::Fixed(560.0));

        Modal::new(body, confirm_card)
            .on_blur(Some(ConnectCubeMessage::DismissChangeConfirmation))
            .into()
    } else {
        body
    }
}
