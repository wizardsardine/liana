//! Duress "Emergency contacts" management (Estate Notifications — PR 1).
//!
//! Rendered as a section appended to the duress settings panel
//! (`duress_ux`), and — while adding/editing — as a focused form takeover.
//! The whole surface is reachable ONLY from normal-mode settings; it is
//! never shown on the duress activation/cryptic screen (that flow lives in
//! a different `ConnectFlowStep`, so `duress_ux` isn't even rendered), so a
//! coercer can't learn who would be alerted.
//!
//! Gating: Estate-only via the `duress_alerts` entitlement. Non-Estate
//! accounts see the standard locked-feature affordance instead of the list.

use coincube_ui::{
    color,
    component::{button, text},
    theme,
    widget::*,
};
use iced::{widget::container, Alignment, Length};

use crate::{
    app::{
        state::connect::ConnectAccountPanel,
        view::{ConnectAccountMessage, DuressContactsMessage},
    },
    services::coincube::{
        is_valid_e164, DuressAlertContact, DURESS_CHANNEL_EMAIL, DURESS_CHANNEL_SMS,
        DURESS_CHANNEL_WHATSAPP, MAX_DURESS_ALERT_CONTACTS,
    },
};

use super::{card_style, format_datetime};

/// Wrap a [`DuressContactsMessage`] for `on_press`.
fn msg(m: DuressContactsMessage) -> ConnectAccountMessage {
    ConnectAccountMessage::DuressContacts(m)
}

/// Exact copy of the alert a contact receives if duress activates, rendered
/// so the owner sees precisely what's sent — no balances, addresses, or
/// mechanics (master plan §4). `{name}` is substituted with the account's
/// own identity where known. The real template lives server-side; this is a
/// faithful preview, kept in sync with the coincube-api template at review.
pub fn alert_template_preview(owner: &str) -> String {
    format!(
        "COINCUBE alert: {owner} asked us to notify you in an emergency. They may need your \
         help — please reach out to them directly, by phone or in person. This is an automated \
         message; reply STOP to opt out."
    )
}

/// Copy of the one-time intro message sent when a contact is added.
pub fn intro_template_preview(owner: &str) -> String {
    format!(
        "{owner} added you as an emergency contact on COINCUBE. You won't hear from us again \
         unless they trigger an emergency alert. Reply STOP to opt out."
    )
}

/// The owner label used in template previews — the account email when known,
/// else a neutral stand-in.
fn owner_label(state: &ConnectAccountPanel) -> String {
    state
        .user
        .as_ref()
        .map(|u| u.email.clone())
        .unwrap_or_else(|| "Your COINCUBE contact".to_string())
}

// =============================================================================
// Section (appended to the duress settings panel)
// =============================================================================

/// The "Emergency contacts" section. Either the locked-feature affordance
/// (non-Estate) or the contacts list + explainer cards (Estate).
pub fn section<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let mut col = Column::new()
        .push(text::h4_bold("Emergency contacts").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(
            text::p2_regular(
                "People we'll notify only if you activate duress mode. They get one intro \
                 message now, and nothing else unless an alert fires.",
            )
            .color(color::GREY_3),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(14.0)))
        .width(Length::Fill);

    if !state.is_duress_alerts_entitled() {
        return col.push(locked_card()).width(Length::Fill).into();
    }

    col = col.push(explainer_cards(state));
    col = col.push(iced::widget::Space::new().height(Length::Fixed(16.0)));
    col = col.push(list_body(state));
    col.width(Length::Fill).into()
}

/// Estate-gated locked affordance, mirroring the duress panel's own
/// upgrade-prompt pattern.
fn locked_card<'a>() -> Element<'a, ConnectAccountMessage> {
    container(
        Column::new()
            .push(text::p1_bold("Available on the Estate plan").style(theme::text::primary))
            .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
            .push(
                text::p2_regular(
                    "Emergency contacts are part of the Estate plan. Upgrade your Connect plan \
                     to let trusted people be alerted if you ever activate duress mode.",
                )
                .color(color::GREY_3),
            )
            .padding(20)
            .spacing(0),
    )
    .style(card_style)
    .width(Length::Fill)
    .into()
}

/// The two explainer cards: what the alert says, and the STOP/opt-out note.
fn explainer_cards<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let owner = owner_label(state);

    let alert_card = container(
        Column::new()
            .push(text::p1_bold("What the alert says").style(theme::text::primary))
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(
                container(text::p2_regular(alert_template_preview(&owner)).color(color::GREY_2))
                    .padding(12)
                    .width(Length::Fill)
                    .style(|t| container::Style {
                        background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                        border: iced::Border {
                            color: color::GREY_5,
                            width: 0.5,
                            radius: 10.0.into(),
                        },
                        ..Default::default()
                    }),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(
                text::p2_regular(
                    "It never includes your balances, addresses, or how COINCUBE works — only \
                     that you may need help.",
                )
                .color(color::GREY_3),
            )
            .padding(20)
            .spacing(0),
    )
    .style(card_style)
    .width(Length::Fill);

    let intro_card = container(
        Column::new()
            .push(text::p1_bold("The one-time intro message").style(theme::text::primary))
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(text::p2_regular(intro_template_preview(&owner)).color(color::GREY_2))
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(
                text::p2_regular(
                    "Anyone can reply STOP at any time to opt out — we'll never message them \
                     again.",
                )
                .color(color::GREY_3),
            )
            .padding(20)
            .spacing(0),
    )
    .style(card_style)
    .width(Length::Fill);

    Column::new()
        .push(alert_card)
        .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
        .push(intro_card)
        .width(Length::Fill)
        .into()
}

/// The list of configured contacts + add affordance, or an empty state.
fn list_body<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let dc = &state.duress_contacts;

    let mut col = Column::new().width(Length::Fill);

    if let Some(err) = dc.error.as_deref() {
        col = col
            .push(text::p2_regular(err).color(color::RED))
            .push(iced::widget::Space::new().height(Length::Fixed(10.0)));
    }

    // Header row with count + Add button (hidden at cap).
    let count = dc.count();
    let header = Row::new()
        .push(
            text::p1_bold(format!(
                "Contacts ({}/{})",
                count, MAX_DURESS_ALERT_CONTACTS
            ))
            .style(theme::text::primary),
        )
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(if dc.at_cap() {
            Element::from(text::p2_regular("Maximum reached").color(color::GREY_3))
        } else {
            button::primary(None, "+ Add contact")
                .on_press(msg(DuressContactsMessage::ShowAddForm))
                .width(Length::Shrink)
                .into()
        })
        .align_y(Alignment::Center);
    col = col
        .push(header)
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)));

    match &dc.contacts {
        None => {
            col = col.push(text::p2_regular("Loading\u{2026}").color(color::GREY_3));
        }
        Some(list) if list.is_empty() => {
            col = col.push(
                container(
                    Column::new()
                        .push(
                            text::p1_bold("No emergency contacts yet").style(theme::text::primary),
                        )
                        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
                        .push(
                            text::p2_regular(
                                "Add up to 5 people who should be alerted if you activate \
                                 duress mode.",
                            )
                            .color(color::GREY_3),
                        )
                        .push(iced::widget::Space::new().height(Length::Fixed(14.0)))
                        .push(
                            button::primary(None, "Add a contact")
                                .on_press(msg(DuressContactsMessage::ShowAddForm)),
                        )
                        .align_x(Alignment::Center)
                        .padding(24)
                        .spacing(0),
                )
                .style(card_style)
                .width(Length::Fill),
            );
        }
        Some(list) => {
            for c in list {
                col = col.push(contact_card(dc, c));
                col = col.push(iced::widget::Space::new().height(Length::Fixed(8.0)));
            }
        }
    }

    col.into()
}

/// One contact row: identity, channels, delivery state, edit/remove.
fn contact_card<'a>(
    dc: &'a crate::app::state::connect::DuressContactsState,
    c: &'a DuressAlertContact,
) -> Element<'a, ConnectAccountMessage> {
    let mut info = Column::new()
        .push(text::p1_regular(c.display_name.as_str()).style(theme::text::primary))
        .spacing(2);

    // Contact methods line.
    let mut methods: Vec<String> = Vec::new();
    if let Some(p) = c.phone.as_deref() {
        if !p.is_empty() {
            methods.push(p.to_string());
        }
    }
    if let Some(e) = c.email.as_deref() {
        if !e.is_empty() {
            methods.push(e.to_string());
        }
    }
    if !methods.is_empty() {
        info = info.push(text::p2_regular(methods.join("  ·  ")).color(color::GREY_3));
    }

    // Channel badges.
    let channels_line = channel_badges(c);
    info = info.push(channels_line);

    // Delivery state: opted out > intro sent > pending.
    let status: Element<ConnectAccountMessage> = if c.is_opted_out() {
        text::p2_regular("Opted out (replied STOP)")
            .color(color::ORANGE)
            .into()
    } else if let Some(at) = c.intro_sent_at.as_deref() {
        text::p2_regular(format!("Intro sent {}", format_datetime(at)))
            .color(color::GREY_3)
            .into()
    } else {
        text::p2_regular("Intro message queued")
            .color(color::GREY_3)
            .into()
    };
    info = info.push(status);

    let id = c.id;
    let deleting = dc.deleting_ids.contains(&id);
    let remove_btn: Element<ConnectAccountMessage> = if deleting {
        button::secondary(None, "Removing\u{2026}")
            .on_press_maybe(None)
            .into()
    } else {
        button::secondary(None, "Remove")
            .on_press(msg(DuressContactsMessage::Delete(id)))
            .into()
    };

    let actions = Row::new()
        .push(button::secondary(None, "Edit").on_press(msg(DuressContactsMessage::EditContact(id))))
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(remove_btn)
        .align_y(Alignment::Center);

    container(
        Row::new()
            .push(info)
            .push(iced::widget::Space::new().width(Length::Fill))
            .push(actions)
            .align_y(Alignment::Center)
            .padding(14),
    )
    .style(card_style)
    .width(Length::Fill)
    .into()
}

/// Small inline channel badges (SMS / WhatsApp / Email) reflecting which
/// channels are enabled for the contact.
fn channel_badges<'a>(c: &'a DuressAlertContact) -> Element<'a, ConnectAccountMessage> {
    let mut row = Row::new().spacing(6).align_y(Alignment::Center);
    let mut any = false;
    for (bit, label) in [
        (DURESS_CHANNEL_SMS, "SMS"),
        (DURESS_CHANNEL_WHATSAPP, "WhatsApp"),
        (DURESS_CHANNEL_EMAIL, "Email"),
    ] {
        if c.has_channel(bit) {
            any = true;
            row = row.push(
                container(text::caption(label).color(color::ORANGE))
                    .padding(iced::Padding {
                        top: 2.0,
                        bottom: 2.0,
                        left: 6.0,
                        right: 6.0,
                    })
                    .style(|_t| container::Style {
                        border: iced::Border {
                            color: color::ORANGE,
                            width: 0.5,
                            radius: 6.0.into(),
                        },
                        ..Default::default()
                    }),
            );
        }
    }
    if !any {
        return text::p2_regular("No channels selected")
            .color(color::RED)
            .into();
    }
    row.into()
}

// =============================================================================
// Add / Edit form (panel takeover)
// =============================================================================

/// The add/edit form. Takes over the duress panel (returned early by
/// `duress_ux`) so the user focuses on one contact at a time.
pub fn form_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let dc = &state.duress_contacts;
    let editing = dc.editing_id.is_some();

    let back_button = iced::widget::button(
        Row::new()
            .push(text::p1_medium("‹ Back").style(theme::text::secondary))
            .align_y(Alignment::Center),
    )
    .style(theme::button::transparent)
    .on_press(msg(DuressContactsMessage::BackToList));

    let phone = dc.form_phone.trim();
    let email = dc.form_email.trim();
    let phone_present = !phone.is_empty();
    let email_present = !email.is_empty();
    let phone_ok = is_valid_e164(phone);
    let email_ok = email.is_empty()
        || email_address::EmailAddress::parse_with_options(
            email,
            email_address::Options::default().with_required_tld(),
        )
        .is_ok();

    // Name field.
    let name_field = Column::new()
        .push(text::p2_regular("Name").color(color::GREY_3))
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        .push(
            TextInput::new("e.g. Jane Doe", &dc.form_name)
                .on_input(|s| msg(DuressContactsMessage::NameChanged(s)))
                .size(16)
                .padding(15),
        )
        .spacing(0);

    // Phone field + validity hint.
    let mut phone_col = Column::new()
        .push(text::p2_regular("Phone (international format)").color(color::GREY_3))
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        .push(
            TextInput::new("+15551234567", &dc.form_phone)
                .on_input(|s| msg(DuressContactsMessage::PhoneChanged(s)))
                .size(16)
                .padding(15),
        )
        .spacing(0);
    if phone_present && !phone_ok {
        phone_col = phone_col
            .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
            .push(
                text::caption("Use international format, starting with + and country code.")
                    .color(color::RED),
            );
    }

    // Email field + validity hint.
    let mut email_col = Column::new()
        .push(text::p2_regular("Email").color(color::GREY_3))
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        .push(
            TextInput::new("jane@example.com", &dc.form_email)
                .on_input(|s| msg(DuressContactsMessage::EmailChanged(s)))
                .size(16)
                .padding(15),
        )
        .spacing(0);
    if email_present && !email_ok {
        email_col = email_col
            .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
            .push(text::caption("That email doesn't look right.").color(color::RED));
    }

    // Channel checkboxes. SMS/WhatsApp require a phone; Email requires an
    // email — disabled (no `on_toggle`) until the matching field is filled.
    let sms_box = channel_checkbox(
        "SMS",
        dc.form_ch_sms && phone_present,
        phone_present,
        DuressContactsMessage::ToggleSms,
    );
    let wa_box = channel_checkbox(
        "WhatsApp",
        dc.form_ch_whatsapp && phone_present,
        phone_present,
        DuressContactsMessage::ToggleWhatsapp,
    );
    let email_box = channel_checkbox(
        "Email",
        dc.form_ch_email && email_present,
        email_present,
        DuressContactsMessage::ToggleEmailChannel,
    );

    let channels_section = Column::new()
        .push(text::p2_regular("How should we reach them?").color(color::GREY_3))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(sms_box)
        .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
        .push(wa_box)
        .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
        .push(email_box)
        .spacing(0);

    // Submit button. Enabled only when the form is minimally valid.
    let name_ok = !dc.form_name.trim().is_empty();
    let reachable = phone_present || email_present;
    let any_channel = ((dc.form_ch_sms || dc.form_ch_whatsapp) && phone_present)
        || (dc.form_ch_email && email_present);
    let can_submit = !dc.submitting && name_ok && reachable && phone_ok && email_ok && any_channel;

    let submit_label = if dc.submitting {
        "Saving\u{2026}"
    } else if editing {
        "Save contact"
    } else {
        "Add contact"
    };
    let submit = button::primary(None, submit_label)
        .on_press_maybe(can_submit.then_some(msg(DuressContactsMessage::Submit)))
        .width(Length::Fill);

    let title = if editing {
        "Edit emergency contact"
    } else {
        "Add emergency contact"
    };

    let mut form = Column::new()
        .push(back_button)
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(text::h4_bold(title).style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
        .push(
            text::p2_regular(
                "Add a phone number, an email, or both — then pick at least one way to reach \
                 them.",
            )
            .color(color::GREY_3),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(18.0)))
        .push(name_field)
        .push(iced::widget::Space::new().height(Length::Fixed(14.0)))
        .push(phone_col)
        .push(iced::widget::Space::new().height(Length::Fixed(14.0)))
        .push(email_col)
        .push(iced::widget::Space::new().height(Length::Fixed(18.0)))
        .push(channels_section)
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(submit)
        .spacing(0)
        .max_width(520)
        .width(Length::Fill);

    if let Some(err) = dc.error.as_deref() {
        form = form
            .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
            .push(text::p2_regular(err).color(color::RED));
    }

    form.into()
}

/// A single channel checkbox. Disabled (no `on_toggle`) when `enabled` is
/// false, so a channel can't be selected without its contact method.
fn channel_checkbox<'a>(
    label: &'a str,
    checked: bool,
    enabled: bool,
    on_toggle: fn(bool) -> DuressContactsMessage,
) -> Element<'a, ConnectAccountMessage> {
    let base = CheckBox::new(checked)
        .label(label)
        .style(theme::checkbox::primary)
        .size(18);
    if enabled {
        base.on_toggle(move |b| msg(on_toggle(b))).into()
    } else {
        // No `on_toggle` → rendered inert. Pair with a muted hint so the
        // user understands why.
        Row::new()
            .push(base)
            .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
            .push(
                text::caption(if label == "Email" {
                    "add an email first"
                } else {
                    "add a phone first"
                })
                .color(color::GREY_3),
            )
            .align_y(Alignment::Center)
            .into()
    }
}
