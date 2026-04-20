use coincube_ui::{
    color,
    component::{button, text},
    icon::*,
    theme,
    widget::*,
};
use iced::{widget::container, Alignment, Length};

use crate::{
    app::{
        state::connect::{ConnectAccountPanel, ContactsStep},
        view::{ConnectAccountMessage, ContactsMessage},
    },
    services::coincube::{ContactRole, Invite},
};

use super::{card_style, format_date};

/// Top-level contacts dispatcher.
pub fn contacts_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    match &state.contacts_state.step {
        ContactsStep::List => contacts_list_ux(state),
        ContactsStep::InviteForm => invite_form_ux(state),
        ContactsStep::Detail(contact_id) => contact_detail_ux(state, *contact_id),
    }
}

// =============================================================================
// Contacts List
// =============================================================================

fn contacts_list_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let cs = &state.contacts_state;

    let header = Row::new()
        .push(
            Column::new()
                .push(text::h4_bold("Contacts").style(theme::text::primary))
                .push(text::p2_regular("Manage your trusted contacts.").color(color::GREY_3))
                .spacing(2),
        )
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(
            button::primary(None, "+")
                .on_press(ConnectAccountMessage::Contacts(
                    ContactsMessage::ShowInviteForm,
                ))
                .width(Length::Shrink),
        )
        .align_y(Alignment::Center);

    let mut col = Column::new()
        .push(header)
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .spacing(0)
        .width(Length::Fill);

    // Loading state
    if cs.loading && cs.contacts.is_none() && cs.invites.is_none() {
        col = col.push(
            text::p1_regular("Loading\u{2026}")
                .color(color::GREY_3)
                .width(Length::Fill),
        );
        return col.into();
    }

    // Show error on the list view only when not loading
    // (errors during initial load are silently swallowed — the empty state is shown instead)
    if !cs.loading {
        if let Some(err) = cs.error.as_deref() {
            if cs.contacts.is_some() || cs.invites.is_some() {
                col = col.push(
                    container(text::p2_regular(err).color(color::RED))
                        .padding(8)
                        .width(Length::Fill),
                );
                col = col.push(iced::widget::Space::new().height(Length::Fixed(10.0)));
            }
        }
    }

    // Pending invites section
    let pending: Vec<&Invite> = cs
        .invites
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|i| i.status == "pending")
        .collect();

    if !pending.is_empty() {
        col = col.push(text::p1_bold("Pending Invites").style(theme::text::primary));
        col = col.push(iced::widget::Space::new().height(Length::Fixed(8.0)));

        for invite in &pending {
            col = col.push(invite_card(invite));
            col = col.push(iced::widget::Space::new().height(Length::Fixed(8.0)));
        }
        col = col.push(iced::widget::Space::new().height(Length::Fixed(12.0)));
    }

    // Contacts list
    let contacts = cs.contacts.as_deref().unwrap_or_default();
    if contacts.is_empty() && pending.is_empty() {
        // Empty state
        col = col.push(
            container(
                Column::new()
                    .push(person_icon().size(40).color(color::GREY_3))
                    .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                    .push(text::p1_bold("No contacts yet").style(theme::text::primary))
                    .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
                    .push(
                        text::p2_regular("Invite your first trusted contact to get started.")
                            .color(color::GREY_3),
                    )
                    .push(iced::widget::Space::new().height(Length::Fixed(16.0)))
                    .push(button::primary(None, "Invite Contact").on_press(
                        ConnectAccountMessage::Contacts(ContactsMessage::ShowInviteForm),
                    ))
                    .align_x(Alignment::Center)
                    .padding(24)
                    .spacing(0),
            )
            .style(card_style)
            .width(Length::Fill),
        );
    } else if !contacts.is_empty() {
        col = col.push(text::p1_bold("Contacts").style(theme::text::primary));
        col = col.push(iced::widget::Space::new().height(Length::Fixed(8.0)));

        for contact in contacts {
            // Contact responses with no linked user are rare (backend
            // marks the field `omitempty`) — render a placeholder and
            // still list them so the user can revoke/clean up.
            let email = contact
                .contact_user
                .as_ref()
                .map(|u| u.email.as_str())
                .unwrap_or("unknown contact");
            let first_char = email
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            let role_label = contact.role.to_string();

            let row = Row::new()
                .push(
                    container(text::p1_bold(first_char).color(color::WHITE))
                        .width(Length::Fixed(36.0))
                        .height(Length::Fixed(36.0))
                        .center_x(Length::Fixed(36.0))
                        .center_y(Length::Fixed(36.0))
                        .style(|_t| container::Style {
                            background: Some(iced::Background::Color(color::ORANGE)),
                            border: iced::Border {
                                radius: 18.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                )
                .push(iced::widget::Space::new().width(Length::Fixed(12.0)))
                .push(
                    Column::new()
                        .push(text::p1_regular(email).style(theme::text::primary))
                        .push(text::p2_regular(role_label).color(role_color(&contact.role)))
                        .spacing(2),
                )
                .push(iced::widget::Space::new().width(Length::Fill))
                .align_y(Alignment::Center);

            let btn = iced::widget::button(container(row).padding(12).width(Length::Fill))
                .style(theme::button::transparent)
                .on_press(ConnectAccountMessage::Contacts(
                    ContactsMessage::ShowDetail(contact.id),
                ))
                .width(Length::Fill);

            col = col.push(container(btn).style(card_style).width(Length::Fill));
            col = col.push(iced::widget::Space::new().height(Length::Fixed(6.0)));
        }
    }

    col.into()
}

fn invite_card<'a>(invite: &'a Invite) -> Element<'a, ConnectAccountMessage> {
    let role_label = invite.role.to_string();
    let expiry_elem = expiry_element(&invite.expires_at);

    let info = Column::new()
        .push(text::p1_regular(invite.invitee_email.as_str()).style(theme::text::primary))
        .push(
            Row::new()
                .push(text::p2_regular(role_label).color(role_color(&invite.role)))
                .push(iced::widget::Space::new().width(Length::Fixed(12.0)))
                .push(expiry_elem)
                .align_y(Alignment::Center),
        )
        .spacing(2);

    let invite_id = invite.id;
    let actions = Row::new()
        .push(
            button::secondary(None, "Resend").on_press(ConnectAccountMessage::Contacts(
                ContactsMessage::ResendInvite(invite_id),
            )),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            button::secondary(None, "Revoke").on_press(ConnectAccountMessage::Contacts(
                ContactsMessage::RevokeInvite(invite_id),
            )),
        )
        .align_y(Alignment::Center);

    container(
        Row::new()
            .push(info)
            .push(iced::widget::Space::new().width(Length::Fill))
            .push(actions)
            .align_y(Alignment::Center)
            .padding(12),
    )
    .style(card_style)
    .width(Length::Fill)
    .into()
}

fn expiry_element<'a>(expires_at: &str) -> Element<'a, ConnectAccountMessage> {
    let days = compute_days_until(expires_at);
    match days {
        Some(1) => text::p2_regular("Expires in 1 day")
            .color(color::GREY_3)
            .into(),
        Some(d) if d > 0 => text::p2_regular(format!("Expires in {} days", d))
            .color(color::GREY_3)
            .into(),
        Some(0) => text::p2_regular("Expires today")
            .color(color::ORANGE)
            .into(),
        Some(d) if d < 0 => text::p2_regular("Expired").color(color::RED).into(),
        _ => text::p2_regular("").into(),
    }
}

fn compute_days_until(expires_at: &str) -> Option<i64> {
    let expiry = chrono::DateTime::parse_from_rfc3339(expires_at).ok()?;
    let now = chrono::Utc::now();
    Some((expiry.date_naive() - now.date_naive()).num_days())
}

// =============================================================================
// Invite Form
// =============================================================================

fn invite_form_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let cs = &state.contacts_state;

    let back_button = iced::widget::button(
        Row::new()
            .push(previous_icon().color(color::GREY_2))
            .push(iced::widget::Space::new().width(Length::Fixed(5.0)))
            .push(text::p1_medium("Back").style(theme::text::secondary))
            .spacing(5)
            .align_y(Alignment::Center),
    )
    .style(theme::button::transparent)
    .on_press(ConnectAccountMessage::Contacts(ContactsMessage::BackToList));

    let email = &cs.invite_email;
    let email_trimmed = email.trim();
    let email_valid = email_address::EmailAddress::parse_with_options(
        email_trimmed,
        email_address::Options::default().with_required_tld(),
    )
    .is_ok();

    let submit: Element<ConnectAccountMessage> = if cs.invite_sending {
        iced::widget::button(
            container(text::p1_regular("Sending\u{2026}").color(color::GREY_3))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fixed(44.0))
        .style(theme::button::primary)
        .into()
    } else {
        button::primary(None, "Send Invite")
            .on_press_maybe((email_valid && !email_trimmed.is_empty()).then_some(
                ConnectAccountMessage::Contacts(ContactsMessage::SubmitInvite),
            ))
            .width(Length::Fill)
            .into()
    };

    let mut form = Column::new()
        .push(back_button)
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(text::h4_bold("Invite Contact").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(text::p2_regular("Email Address").color(color::GREY_3))
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        .push(
            TextInput::new("email@example.com", email)
                .on_input(|s| {
                    ConnectAccountMessage::Contacts(ContactsMessage::InviteEmailChanged(s))
                })
                .on_submit_maybe((email_valid && !cs.invite_sending).then_some(
                    ConnectAccountMessage::Contacts(ContactsMessage::SubmitInvite),
                ))
                .size(16)
                .padding(15),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(16.0)));
    // Role selector removed 2026-04-20 per PR 5 §2.7 tweak #2 —
    // Keyholder is the only usable role today; invite_role stays in
    // ContactsState pre-initialised so reintroduction is a small diff.

    // W12: optional cube multi-select. Only rendered when the backend
    // returned a non-empty cube list.
    if let Some(cubes) = cs.invite_available_cubes.as_deref() {
        if !cubes.is_empty() {
            form = form
                .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
                .push(text::p2_regular("Also add to Cube(s) (optional)").color(color::GREY_3))
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(invite_cubes_section(cubes, &cs.invite_cube_selections));
        }
    }

    form = form
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(submit)
        .spacing(0)
        .max_width(500)
        .width(Length::Fill);

    if let Some(err) = cs.error.as_deref() {
        form = form
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(text::p2_regular(err).color(color::RED));
    }

    if let Some(msg) = cs.invite_cube_error.as_deref() {
        form = form
            .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
            .push(invite_cube_conflict_card(msg));
    }

    form.into()
}

/// Renders the cube multi-select list. Each row is a labelled
/// [`CheckBox`] emitting `ToggleInviteCube(id)` on toggle.
fn invite_cubes_section<'a>(
    cubes: &'a [crate::app::state::connect::InviteCubeOption],
    selections: &'a [u64],
) -> Element<'a, ConnectAccountMessage> {
    let mut col = Column::new().spacing(6);
    for cube in cubes {
        let checked = selections.contains(&cube.id);
        let id = cube.id;
        let label = format!("{} ({})", cube.name, cube.network);
        col = col.push(
            CheckBox::new(checked)
                .label(label)
                .on_toggle(move |_| {
                    ConnectAccountMessage::Contacts(ContactsMessage::ToggleInviteCube(id))
                })
                .style(theme::checkbox::primary)
                .size(18),
        );
    }
    container(col.padding(4)).width(Length::Fill).into()
}

/// Banner shown when `POST /connect/invites` 403'd on a cube id. The
/// message suggests the user re-pick — the cube list has already been
/// reloaded at this point so the new checkboxes reflect current
/// membership.
fn invite_cube_conflict_card<'a>(msg: &str) -> Element<'a, ConnectAccountMessage> {
    container(
        Column::new()
            .push(
                text::p1_bold("One or more selected cubes is no longer available")
                    .color(color::RED),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
            .push(text::p2_regular(msg.to_string()).style(theme::text::primary))
            .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
            .push(
                text::p2_regular("Review your cube selection above and try again.")
                    .color(color::GREY_3),
            )
            .padding(16)
            .spacing(0),
    )
    .style(|t| container::Style {
        background: Some(iced::Background::Color(t.colors.cards.simple.background)),
        border: iced::Border {
            color: color::RED,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    })
    .width(Length::Fill)
    .into()
}

// =============================================================================
// Contact Detail
// =============================================================================

fn contact_detail_ux<'a>(
    state: &'a ConnectAccountPanel,
    contact_id: u64,
) -> Element<'a, ConnectAccountMessage> {
    let cs = &state.contacts_state;

    let back_button = iced::widget::button(
        Row::new()
            .push(previous_icon().color(color::GREY_2))
            .push(iced::widget::Space::new().width(Length::Fixed(5.0)))
            .push(text::p1_medium("Back").style(theme::text::secondary))
            .spacing(5)
            .align_y(Alignment::Center),
    )
    .style(theme::button::transparent)
    .on_press(ConnectAccountMessage::Contacts(ContactsMessage::BackToList));

    let contact = cs
        .contacts
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|c| c.id == contact_id);

    let Some(contact) = contact else {
        return Column::new()
            .push(back_button)
            .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
            .push(text::p1_regular("Contact not found").color(color::GREY_3))
            .width(Length::Fill)
            .into();
    };

    let email = contact
        .contact_user
        .as_ref()
        .map(|u| u.email.as_str())
        .unwrap_or("unknown contact");
    let first_char = email
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let rc = role_color(&contact.role);
    let role_label = contact.role.to_string();
    let connected_date = format_date(&contact.created_at);

    let avatar = container(text::h3(first_char).color(color::WHITE))
        .width(Length::Fixed(64.0))
        .height(Length::Fixed(64.0))
        .center_x(Length::Fixed(64.0))
        .center_y(Length::Fixed(64.0))
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(color::ORANGE)),
            border: iced::Border {
                radius: 32.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });

    let role_badge = container(text::p2_regular(role_label).color(rc))
        .padding(iced::Padding {
            top: 2.0,
            bottom: 2.0,
            left: 8.0,
            right: 8.0,
        })
        .style(move |_t| container::Style {
            background: None,
            border: iced::Border {
                color: rc,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        });

    let contact_header = container(
        Column::new()
            .push(avatar)
            .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
            .push(text::h4_bold(email).style(theme::text::primary))
            .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
            .push(role_badge)
            .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
            .push(text::p2_regular(format!("Connected {}", connected_date)).color(color::GREY_3))
            .align_x(Alignment::Center)
            .padding(20)
            .spacing(0),
    )
    .style(card_style)
    .width(Length::Fill);

    // Associated Cubes section
    let cubes_section: Element<ConnectAccountMessage> = match &cs.detail_cubes {
        None if cs.detail_cubes_error.is_some() => Column::new()
            .push(
                text::p2_regular(
                    cs.detail_cubes_error
                        .as_deref()
                        .unwrap_or("Failed to load cubes"),
                )
                .color(color::RED),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(
                button::secondary(None, "Retry").on_press(ConnectAccountMessage::Contacts(
                    ContactsMessage::ShowDetail(contact_id),
                )),
            )
            .into(),
        None => text::p2_regular("Loading cubes\u{2026}")
            .color(color::GREY_3)
            .into(),
        Some(cubes) if cubes.is_empty() => text::p2_regular("No cubes found")
            .color(color::GREY_3)
            .into(),
        Some(cubes) => {
            let mut cube_col = Column::new().spacing(6);
            for cube in cubes {
                let mut row = Row::new()
                    .push(text::p1_regular(cube.name.as_str()).style(theme::text::primary))
                    .push(iced::widget::Space::new().width(Length::Fixed(12.0)))
                    .push(text::p2_regular(cube.network.as_str()).color(color::GREY_3));
                if cube.has_recovery_kit {
                    row = row
                        .push(iced::widget::Space::new().width(Length::Fixed(12.0)))
                        .push(
                            container(text::caption("Recovery Kit").color(color::ORANGE))
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
                row = row.align_y(Alignment::Center);
                cube_col = cube_col.push(row);
            }
            cube_col.into()
        }
    };

    // W14: entry points for "add this contact to a cube". Only rendered
    // when the contact has a linked user — an orphaned contact can't be
    // added to a cube anyway.
    let can_add = contact.contact_user.is_some();
    let current_cube_err = cs.add_to_current_cube_errors.get(&contact_id);
    let add_actions: Option<Element<ConnectAccountMessage>> = can_add.then(|| {
        let mut row = Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(
                // One-click "Add to Current Cube" — primary action,
                // gated on having the active cube's server-side id
                // resolved (populated post `register_cube`). Works
                // regardless of how many other cubes the user owns on
                // the same network — the action targets the specific
                // loaded cube, not a network-matching guess.
                button::primary(None, "Add to Current Cube").on_press_maybe(
                    cs.active_cube_server_id.is_some().then_some(
                        ConnectAccountMessage::Contacts(
                            ContactsMessage::AddContactToCurrentCube(contact_id),
                        ),
                    ),
                ),
            )
            .push(
                // Multi-select — secondary action, always available.
                button::secondary(None, "Add to Cube(s)…").on_press(
                    ConnectAccountMessage::Contacts(
                        ContactsMessage::OpenAddToCubeDialog(contact_id),
                    ),
                ),
            );
        if let Some(err) = current_cube_err {
            row = row
                .push(iced::widget::Space::new().width(Length::Fixed(12.0)))
                .push(text::p2_regular(err.clone()).color(color::RED));
        }
        row.into()
    });

    let add_to_cube_dialog = cs
        .add_to_cube_target
        .as_ref()
        .filter(|d| d.contact_id == contact_id)
        .map(add_to_cube_dialog_card);

    let mut col = Column::new()
        .push(back_button)
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(contact_header)
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(text::p1_bold("Associated Cubes").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(
            container(Column::new().push(cubes_section).padding(16).spacing(2))
                .style(card_style)
                .width(Length::Fill),
        );

    if let Some(actions) = add_actions {
        col = col
            .push(iced::widget::Space::new().height(Length::Fixed(14.0)))
            .push(actions);
    }

    if let Some(dialog) = add_to_cube_dialog {
        col = col
            .push(iced::widget::Space::new().height(Length::Fixed(14.0)))
            .push(dialog);
    }

    col.spacing(0).width(Length::Fill).into()
}

/// W14 multi-select dialog body. Rendered inline beneath the Associated
/// Cubes card in the contact detail view when
/// `contacts_state.add_to_cube_target` is set for this contact.
fn add_to_cube_dialog_card<'a>(
    dialog: &'a crate::app::state::connect::AddToCubeDialog,
) -> Element<'a, ConnectAccountMessage> {
    let header = Row::new()
        .push(text::p1_bold("Add to Cube(s)").style(theme::text::primary))
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(
            iced::widget::button(cross_icon().color(color::GREY_3))
                .padding([4, 6])
                .style(theme::button::transparent)
                .on_press(ConnectAccountMessage::Contacts(
                    ContactsMessage::CloseAddToCubeDialog,
                )),
        )
        .align_y(Alignment::Center);

    let body: Element<ConnectAccountMessage> = match dialog.candidate_cubes.as_deref() {
        None => text::p2_regular("Loading eligible cubes\u{2026}")
            .color(color::GREY_3)
            .into(),
        Some([]) => text::p2_regular(
            "No eligible cubes — the contact is already in all of your \
             active-network cubes, or you have none.",
        )
        .color(color::GREY_3)
        .into(),
        Some(cubes) => {
            let mut col = Column::new().spacing(6);
            for cube in cubes {
                let checked = dialog.selections.contains(&cube.id);
                let id = cube.id;
                let mut row = Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(
                        CheckBox::new(checked)
                            .label(format!("{} ({})", cube.name, cube.network))
                            .on_toggle(move |_| {
                                ConnectAccountMessage::Contacts(
                                    ContactsMessage::ToggleAddToCubeSelection(id),
                                )
                            })
                            .style(theme::checkbox::primary)
                            .size(18),
                    );
                if let Some(err) = dialog.failures.get(&id) {
                    row = row
                        .push(iced::widget::Space::new().width(Length::Fill))
                        .push(text::p2_regular(err.clone()).color(color::RED));
                }
                col = col.push(row);
            }
            col.into()
        }
    };

    let can_confirm =
        !dialog.submitting && !dialog.selections.is_empty() && dialog.candidate_cubes.is_some();
    let confirm_label = if dialog.submitting {
        "Adding\u{2026}"
    } else {
        "Add"
    };
    let confirm_btn = button::primary(None, confirm_label).on_press_maybe(can_confirm.then_some(
        ConnectAccountMessage::Contacts(ContactsMessage::ConfirmAddToCube),
    ));

    let footer = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(
            button::secondary(None, "Cancel").on_press(ConnectAccountMessage::Contacts(
                ContactsMessage::CloseAddToCubeDialog,
            )),
        )
        .push(confirm_btn);

    container(
        Column::new()
            .spacing(12)
            .push(header)
            .push(
                text::p2_regular(format!(
                    "Add {} to the selected cubes.",
                    dialog.contact_email
                ))
                .color(color::GREY_3),
            )
            .push(body)
            .push(footer)
            .padding(16),
    )
    .style(card_style)
    .width(Length::Fill)
    .into()
}

// =============================================================================
// Helpers
// =============================================================================

fn role_color(role: &ContactRole) -> iced::Color {
    match role {
        ContactRole::Keyholder => color::BLUE,
        ContactRole::Beneficiary => color::GREEN,
        ContactRole::Observer => color::GREY_3,
    }
}
