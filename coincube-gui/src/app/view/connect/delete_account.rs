//! Connect sheet › Account › Delete account: what a deletion request removes,
//! and the request itself behind a confirm step.

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::{
    component::tenshu::text as tenshu_text,
    icon::*,
    localized_format,
    theme::tenshu::{button::ActionKind, Tone},
    widget::*,
};
use iced::{widget::container, Alignment, Length};

use super::{
    format_date, group_title, lead, review_style, sheet_button, sheet_link, sheet_title,
    status_line,
};
use crate::app::{
    state::connect::{
        account::ConnectAccountPanel,
        delete_account::{DeletionStatus, DELETE_ACCOUNT_POLICY_URL},
    },
    view::{ConnectAccountMessage, DeleteAccountMessage},
};

fn msg(m: DeleteAccountMessage) -> ConnectAccountMessage {
    ConnectAccountMessage::DeleteAccount(m)
}

const DELETED_DATA: [&str; 6] = [
    "Account email, devices and push tokens",
    "Key records (public keys and fingerprints)",
    "Cubes you own, contacts and invitations",
    "Security history",
    "Encrypted Recovery Kit and inheritance backups",
    "Payment and verification records",
];

/// Mobile's `deleteAccountConfirmBody`.
const CONFIRM_BODY: &str = "We'll delete your COINCUBE account and its data within 30 days. Make sure your recovery phrase is backed up first. Once deletion is complete, it can't be undone.";
/// Mobile's `deleteAccountRequestFailed`.
const REQUEST_FAILED: &str = "We couldn't send your request. Check your connection and try again.";

const BEFORE_REQUEST: &str = "When you send the request, we'll email you to confirm we received it. Deletion completes within 30 days. To cancel before then, contact privacy@coincube.io.";
const AFTER_REQUEST: &str = "You should receive a confirmation email at your account address. Deletion completes within 30 days. To cancel before then, contact privacy@coincube.io.";

/// The Account section's entry: shown only on the signed-in Account overview.
pub(super) fn entry<'a>() -> Element<'a, ConnectAccountMessage> {
    Column::new()
        .push(group_title(localized_copy("Delete account")))
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        .push(lead(localized_copy(
            "Ask COINCUBE to delete this account and the data our servers hold for it.",
        )))
        .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
        .push(
            sheet_button(
                localized_copy("Delete account…"),
                ActionKind::Destructive,
                false,
            )
            .on_press(msg(DeleteAccountMessage::Open)),
        )
        .width(Length::Fill)
        .into()
}

fn bullet<'a>(line: &'a str) -> Element<'a, ConnectAccountMessage> {
    Row::new()
        .push(tenshu_text::body("•").size(13))
        .push(tenshu_text::body(localized_copy(line)).size(13))
        .spacing(8)
        .into()
}

fn paragraph<'a>(line: &'a str) -> Element<'a, ConnectAccountMessage> {
    tenshu_text::body(localized_copy(line)).size(13).into()
}

/// The page that replaces the Account overview while it is open.
pub(super) fn page<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let email = state
        .user
        .as_ref()
        .map(|user| user.email.as_str())
        .unwrap_or("—");
    let deletion = &state.delete_account;

    let mut what = Column::new().spacing(6);
    for line in DELETED_DATA {
        what = what.push(bullet(line));
    }

    let mut column = Column::new()
        .push(
            sheet_link(localized_copy("‹ Account"))
                .on_press_maybe((!deletion.submitting).then_some(msg(DeleteAccountMessage::Close))),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(sheet_title(localized_copy("Delete your COINCUBE account")))
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        .push(lead(email))
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(paragraph(
            "This deletes your COINCUBE account and the data our servers hold for it, across COINCUBE Desktop and COINCUBE Mobile:",
        ))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(what)
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(group_title(localized_copy("Cubes and heirs")))
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        .push(paragraph(
            "Deleting the Cubes you own also removes the backups your heirs depend on.",
        ))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(paragraph(
            "If you hold a key in someone else's Cube, your key and seat in that Cube, and any recovery or inheritance material addressed to your key, are removed. This can affect the owner's ability to coordinate spending through COINCUBE.",
        ))
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(group_title(localized_copy("Your bitcoin")))
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        .push(paragraph(
            "COINCUBE does not hold your bitcoin; it is not affected. Wallets and keys on this computer are not removed by this request — back up your recovery phrase first.",
        ))
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(paragraph(
            "Some records may be kept where the law requires.",
        ))
        .push(
            sheet_link(DELETE_ACCOUNT_POLICY_URL.trim_start_matches("https://"))
                .on_press(msg(DeleteAccountMessage::OpenPolicy)),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(24.0)));

    column = match &deletion.status {
        DeletionStatus::Unknown | DeletionStatus::Loading => {
            column.push(lead(localized_copy("Checking for an existing request…")))
        }
        DeletionStatus::Failed => column.push(lead(localized_copy(
            "Couldn't check for an existing request.",
        ))),
        DeletionStatus::Pending(request) => column
            .push(status_line(
                check_circle_icon(),
                localized_copy("Request received"),
                Tone::Good,
            ))
            .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
            .push(lead(localized_format!(
                "Requested on {date}",
                "Solicitada el {date}",
                date = format_date(&request.requested_at)
            )))
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(paragraph(AFTER_REQUEST)),
        DeletionStatus::NotRequested => {
            let column = column
                .push(paragraph(BEFORE_REQUEST))
                .push(iced::widget::Space::new().height(Length::Fixed(16.0)));
            if deletion.confirming {
                column.push(confirm_step(
                    email,
                    deletion.submitting,
                    deletion.request_failed,
                ))
            } else {
                column.push(
                    sheet_button(
                        localized_copy("Request account deletion…"),
                        ActionKind::Destructive,
                        false,
                    )
                    .on_press(msg(DeleteAccountMessage::Review(true))),
                )
            }
        }
    };

    column.width(Length::Fill).into()
}

fn confirm_step<'a>(
    email: &'a str,
    submitting: bool,
    failed: bool,
) -> Element<'a, ConnectAccountMessage> {
    let send_label = if submitting {
        localized_copy("Sending request…")
    } else {
        localized_copy("Delete my account")
    };
    container(
        Column::new()
            .push(group_title(localized_format!(
                "Delete the COINCUBE account {email}?",
                "¿Eliminar la cuenta de COINCUBE {email}?"
            )))
            .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
            .push(paragraph(CONFIRM_BODY))
            .push_maybe(failed.then(|| {
                Column::new()
                    .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                    .push(
                        tenshu_text::body(localized_copy(REQUEST_FAILED))
                            .size(13)
                            .style(|theme| {
                                coincube_ui::theme::tenshu::text::tone(theme, Tone::Bad)
                            }),
                    )
            }))
            .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
            .push(
                Row::new()
                    .push(
                        sheet_button(send_label, ActionKind::Destructive, false).on_press_maybe(
                            (!submitting).then_some(msg(DeleteAccountMessage::Confirm)),
                        ),
                    )
                    .push(
                        sheet_button(localized_copy("Cancel"), ActionKind::Secondary, false)
                            .on_press_maybe(
                                (!submitting).then_some(msg(DeleteAccountMessage::Review(false))),
                            ),
                    )
                    .spacing(12)
                    .align_y(Alignment::Center),
            ),
    )
    .padding([14, 16])
    .width(Length::Fill)
    .style(review_style)
    .into()
}
