//! Cube-scoped members panel view (W8).
//!
//! Rendered when the user navigates to `ConnectSubMenu::CubeMembers` inside a
//! loaded cube. Consumes [`ConnectCubePanel`] state and emits
//! [`ConnectCubeMessage::Members`] messages.

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
        state::connect::ConnectCubePanel,
        view::{ConnectCubeMembersMessage, ConnectCubeMessage},
    },
    services::coincube::{CubeInviteSummary, CubeMember},
};

use super::{card_style, format_date};

/// Top-level Members view. Composes the invite form, member list, and
/// pending-invite list. Dialogs (error banner, stranded-vault conflict) are
/// rendered at the bottom.
pub fn cube_members_ux<'a>(state: &'a ConnectCubePanel) -> Element<'a, ConnectCubeMessage> {
    let panel = &state.members;

    let header = Row::new()
        .push(
            Column::new()
                .push(text::h4_bold("Members").style(theme::text::primary))
                .push(
                    text::p2_regular("People who can view this Cube and contribute signing keys.")
                        .color(color::GREY_3),
                )
                .spacing(2),
        )
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(
            button::secondary(None, "Refresh")
                .on_press(ConnectCubeMessage::Members(
                    ConnectCubeMembersMessage::Reload,
                ))
                .width(Length::Shrink),
        )
        .align_y(Alignment::Center);

    let mut col = Column::new()
        .push(header)
        .push(iced::widget::Space::new().height(Length::Fixed(16.0)))
        .push(invite_form(panel))
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .spacing(0)
        .width(Length::Fill);

    // Pending invites section
    if !panel.pending_invites.is_empty() {
        col = col.push(text::p1_bold("Pending Invites").style(theme::text::primary));
        col = col.push(iced::widget::Space::new().height(Length::Fixed(8.0)));
        for invite in &panel.pending_invites {
            col = col.push(pending_invite_card(invite));
            col = col.push(iced::widget::Space::new().height(Length::Fixed(6.0)));
        }
        col = col.push(iced::widget::Space::new().height(Length::Fixed(16.0)));
    }

    // Members list. Skip the "No members yet" placeholder when there's
    // an error — we don't actually know the cube is empty, the load just
    // failed. The `error_banner` below communicates that state instead.
    if panel.members.is_empty()
        && panel.pending_invites.is_empty()
        && !panel.loading
        && panel.error.is_none()
    {
        col = col.push(empty_state());
    } else if !panel.members.is_empty() {
        col = col.push(text::p1_bold("Members").style(theme::text::primary));
        col = col.push(iced::widget::Space::new().height(Length::Fixed(8.0)));
        for member in &panel.members {
            col = col.push(member_card(member));
            col = col.push(iced::widget::Space::new().height(Length::Fixed(6.0)));
        }
    } else if panel.loading {
        col = col.push(text::p1_regular("Loading\u{2026}").color(color::GREY_3));
    }

    if let Some(err) = panel.error.as_deref() {
        col = col.push(iced::widget::Space::new().height(Length::Fixed(12.0)));
        col = col.push(error_banner(err));
    }

    if let Some(conflict) = panel.remove_conflict.as_deref() {
        col = col.push(iced::widget::Space::new().height(Length::Fixed(12.0)));
        col = col.push(remove_conflict_banner(conflict));
    }

    col.into()
}

// =============================================================================
// Invite Form
// =============================================================================

fn invite_form<'a>(
    panel: &'a crate::app::state::connect::ConnectCubeMembersState,
) -> Element<'a, ConnectCubeMessage> {
    let email = panel.invite_email.trim();
    let email_valid = email_address::EmailAddress::parse_with_options(
        email,
        email_address::Options::default().with_required_tld(),
    )
    .is_ok();
    let can_submit = email_valid && !email.is_empty() && !panel.invite_sending;

    let submit: Element<ConnectCubeMessage> = if panel.invite_sending {
        iced::widget::button(
            container(text::p1_regular("Sending\u{2026}").color(color::GREY_3))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Shrink)
        .height(Length::Fixed(44.0))
        .style(theme::button::primary)
        .into()
    } else {
        button::primary(None, "Add Member")
            .on_press_maybe(can_submit.then_some(ConnectCubeMessage::Members(
                ConnectCubeMembersMessage::SubmitInvite,
            )))
            .width(Length::Shrink)
            .into()
    };

    let input = TextInput::new("email@example.com", &panel.invite_email)
        .on_input(|s| ConnectCubeMessage::Members(ConnectCubeMembersMessage::InviteEmailChanged(s)))
        .on_submit_maybe(can_submit.then_some(ConnectCubeMessage::Members(
            ConnectCubeMembersMessage::SubmitInvite,
        )))
        .size(16)
        .padding(15)
        .width(Length::Fill);

    let row = Row::new()
        .push(input)
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(submit)
        .align_y(Alignment::Center);

    container(
        Column::new()
            .push(
                text::p2_regular(
                    "Invite by email. Existing contacts are added immediately; new emails \
                     get an invite and join on accept.",
                )
                .color(color::GREY_3),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
            .push(row)
            .padding(16)
            .spacing(0),
    )
    .style(card_style)
    .width(Length::Fill)
    .into()
}

// =============================================================================
// Member & Invite cards
// =============================================================================

fn member_card<'a>(member: &'a CubeMember) -> Element<'a, ConnectCubeMessage> {
    let email = &member.user.email;
    let initial = email
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();

    let avatar = container(text::p1_bold(initial).color(color::WHITE))
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
        });

    let joined = format_date(&member.joined_at);
    let info = Column::new()
        .push(text::p1_regular(email.as_str()).style(theme::text::primary))
        .push(text::p2_regular(format!("Joined {}", joined)).color(color::GREY_3))
        .spacing(2);

    let member_id = member.id;
    let remove_btn = button::secondary(None, "Remove").on_press(ConnectCubeMessage::Members(
        ConnectCubeMembersMessage::RemoveMember(member_id),
    ));

    container(
        Row::new()
            .push(avatar)
            .push(iced::widget::Space::new().width(Length::Fixed(12.0)))
            .push(info)
            .push(iced::widget::Space::new().width(Length::Fill))
            .push(remove_btn)
            .align_y(Alignment::Center)
            .padding(12),
    )
    .style(card_style)
    .width(Length::Fill)
    .into()
}

fn pending_invite_card<'a>(invite: &'a CubeInviteSummary) -> Element<'a, ConnectCubeMessage> {
    let expiry_elem = expiry_element(&invite.expires_at);

    let info = Column::new()
        .push(text::p1_regular(invite.email.as_str()).style(theme::text::primary))
        .push(
            Row::new()
                .push(text::p2_regular(invite.status.as_str()).color(status_color(&invite.status)))
                .push(iced::widget::Space::new().width(Length::Fixed(12.0)))
                .push(expiry_elem)
                .align_y(Alignment::Center),
        )
        .spacing(2);

    let invite_id = invite.id;
    let actions = Row::new()
        .push(
            button::secondary(None, "Revoke").on_press(ConnectCubeMessage::Members(
                ConnectCubeMembersMessage::RevokeInvite(invite_id),
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

// =============================================================================
// Empty state, error banners
// =============================================================================

fn empty_state<'a>() -> Element<'a, ConnectCubeMessage> {
    container(
        Column::new()
            .push(person_icon().size(40).color(color::GREY_3))
            .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
            .push(text::p1_bold("No members yet").style(theme::text::primary))
            .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
            .push(
                text::p2_regular(
                    "Add a contact by email to let them contribute a signing key to this Cube.",
                )
                .color(color::GREY_3),
            )
            .align_x(Alignment::Center)
            .padding(24)
            .spacing(0),
    )
    .style(card_style)
    .width(Length::Fill)
    .into()
}

fn error_banner<'a>(err: &str) -> Element<'a, ConnectCubeMessage> {
    container(
        Row::new()
            .push(text::p2_regular(err.to_string()).color(color::RED))
            .push(iced::widget::Space::new().width(Length::Fill))
            .push(
                iced::widget::button(cross_icon().color(color::GREY_3))
                    .padding([6, 8])
                    .style(theme::button::transparent)
                    .on_press(ConnectCubeMessage::Members(
                        ConnectCubeMembersMessage::DismissError,
                    )),
            )
            .align_y(Alignment::Center)
            .padding(12),
    )
    .style(|t| container::Style {
        background: Some(iced::Background::Color(t.colors.cards.simple.background)),
        border: iced::Border {
            color: color::RED,
            width: 0.5,
            radius: 8.0.into(),
        },
        ..Default::default()
    })
    .width(Length::Fill)
    .into()
}

fn remove_conflict_banner<'a>(msg: &str) -> Element<'a, ConnectCubeMessage> {
    container(
        Column::new()
            .push(text::p1_bold("Can't remove this member").color(color::RED))
            .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
            .push(text::p2_regular(msg.to_string()).style(theme::text::primary))
            .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
            .push(
                text::p2_regular("Wind down the Vault(s) first, then try again.")
                    .color(color::GREY_3),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
            .push(
                Row::new()
                    .push(iced::widget::Space::new().width(Length::Fill))
                    .push(button::secondary(None, "Dismiss").on_press(
                        ConnectCubeMessage::Members(
                            ConnectCubeMembersMessage::DismissRemoveConflict,
                        ),
                    )),
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
// Helpers
// =============================================================================

fn status_color(status: &str) -> iced::Color {
    match status {
        "pending" => color::ORANGE,
        "accepted" => color::GREEN,
        "revoked" | "expired" => color::GREY_3,
        _ => color::GREY_3,
    }
}

fn expiry_element<'a>(expires_at: &str) -> Element<'a, ConnectCubeMessage> {
    let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(expires_at) else {
        return text::p2_regular("").into();
    };
    let now = chrono::Utc::now();
    // Check the full timestamp first. If we only compared
    // `expiry.date_naive() - now.date_naive()` we'd bucket an invite
    // that expired at 08:00 today as "Expires today" at 18:00 even
    // though it's already past.
    let expiry_utc = expiry.with_timezone(&chrono::Utc);
    if expiry_utc <= now {
        return text::p2_regular("Expired").color(color::RED).into();
    }
    let days = (expiry_utc.date_naive() - now.date_naive()).num_days();
    match days {
        0 => text::p2_regular("Expires today")
            .color(color::ORANGE)
            .into(),
        1 => text::p2_regular("Expires in 1 day")
            .color(color::GREY_3)
            .into(),
        d => text::p2_regular(format!("Expires in {} days", d))
            .color(color::GREY_3)
            .into(),
    }
}
