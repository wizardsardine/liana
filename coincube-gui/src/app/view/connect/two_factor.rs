//! Connect authenticator app: the sign-in step that asks for its code, and
//! Devices & security › Authenticator app and Backup codes.

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::{
    color,
    component::tenshu::text as tenshu_text,
    component::{button, text},
    icon::*,
    theme,
    theme::tenshu::{button::ActionKind, Tone},
    widget::*,
};
use iced::{widget::container, Alignment, Length};

use super::{
    group_title, horizontal_rule, lead, review_style, sheet_button, sheet_link, status_line,
    TWO_FACTOR_INPUT_ID, TWO_FACTOR_SETTINGS_INPUT_ID,
};
use crate::app::{
    state::connect::account::{
        second_factor_code_complete, ConnectAccountPanel, TwoFactorNote, TwoFactorPanel,
        TwoFactorSettings, TwoFactorStatusView, TwoFactorStepFailure,
    },
    view::{ConnectAccountMessage, SecretInput, TwoFactorSettingsMessage},
};

use crate::services::coincube::{BackupCodes, TotpSetup, TwoFactorStatus};

/// Where someone with neither the app nor a backup code can turn. The same
/// address the app gives for other account problems.
const SUPPORT_URL: &str = "https://coincube.io/support";

/// Fewer backup codes than this left reads as a warning.
const LOW_BACKUP_CODES: u32 = 3;

fn settings(msg: TwoFactorSettingsMessage) -> ConnectAccountMessage {
    ConnectAccountMessage::TwoFactorSettings(msg)
}

fn backup_left_text(left: u32) -> String {
    match left {
        0 => "You have no backup codes left.".to_string(),
        1 => "1 backup code left.".to_string(),
        n => format!("{n} backup codes left."),
    }
}

/// The sign-in step after the email code, for an account with an
/// authenticator app.
#[allow(clippy::too_many_arguments)]
pub(super) fn sign_in_ux<'a>(
    email: &'a str,
    code: &'a str,
    backup: bool,
    sending: bool,
    failure: Option<TwoFactorStepFailure>,
    lost_open: bool,
) -> Element<'a, ConnectAccountMessage> {
    let expired = failure == Some(TwoFactorStepFailure::Expired);
    let valid = second_factor_code_complete(code, backup);

    let (title, label, placeholder) = if backup {
        ("Enter a backup code", "Backup code", "xxxx-xxxx-xxxx")
    } else {
        (
            "Enter your authenticator code",
            "Authenticator code",
            "000000",
        )
    };
    let how = if backup {
        "Enter one of the backup codes you saved when you turned on the authenticator app. Each code works once.".to_string()
    } else {
        format!("Open the app and find COINCUBE ({email}). Enter the 6-digit code it shows now.")
    };

    let mut col = Column::new()
        .push(text::h3(localized_copy(title)).style(theme::text::primary))
        .push(
            text::p2_regular(format!(
                "Your email code checked out. {email} also has an authenticator app turned on."
            ))
            .style(theme::text::secondary),
        )
        .push(text::p2_regular(how).style(theme::text::secondary))
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)));

    if expired {
        col = col
            .push(status_line(
                warning_icon(),
                "This sign-in timed out. Start over to get a new email code.",
                Tone::Bad,
            ))
            .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
            .push(
                button::primary(None, localized_copy("Start over"))
                    .on_press(ConnectAccountMessage::BackToEmail)
                    .width(Length::Fill),
            );
    } else {
        let submit = sign_in_submit(sending, valid);

        col = col
            .push(text::p2_medium(localized_copy(label)).style(theme::text::primary))
            .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
            .push(
                TextInput::new(localized_copy(placeholder), code)
                    .id(TWO_FACTOR_INPUT_ID)
                    .on_input(|v| ConnectAccountMessage::TwoFactorCodeChanged(SecretInput(v)))
                    .on_submit_maybe(
                        (!sending && valid).then_some(ConnectAccountMessage::VerifyTwoFactor),
                    )
                    .size(16)
                    .padding(15),
            );
        col = append_sign_in_failure(col, failure, backup);
        col = col
            .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
            .push(submit)
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(
                iced::widget::button(
                    text::p2_regular(localized_copy(if backup {
                        "Use your authenticator app instead"
                    } else {
                        "Use a backup code instead"
                    }))
                    .style(theme::text::secondary),
                )
                .style(theme::button::link)
                .on_press_maybe((!sending).then_some(ConnectAccountMessage::TwoFactorToggleBackup)),
            )
            .push(
                iced::widget::button(
                    text::p2_regular(localized_copy("No app and no backup codes?"))
                        .style(theme::text::secondary),
                )
                .style(theme::button::link)
                .on_press(ConnectAccountMessage::TwoFactorToggleLost),
            );
        if lost_open {
            col = col.push(
                container(
                    Column::new()
                        .push(tenshu_text::body(localized_copy(
                            "Without one of them you can't finish signing in to Connect. Your Cubes on this computer aren't affected: they open with their own PIN or passkey.",
                        )).size(13))
                        .push(tenshu_text::body(format!(
                            "If you're signed in to Connect on another device, turn the authenticator app off there under Devices & security. Otherwise, contact COINCUBE support at {SUPPORT_URL} from {email}."
                        )).size(13))
                        .spacing(8),
                )
                .padding([12, 14])
                .width(Length::Fill)
                .style(review_style),
            );
        }
        col = col.push(
            iced::widget::button(
                text::p2_regular(localized_copy("Start over")).style(theme::text::secondary),
            )
            .style(theme::button::link)
            .on_press_maybe((!sending).then_some(ConnectAccountMessage::BackToEmail)),
        );
    }

    col.align_x(Alignment::Center)
        .spacing(2)
        .max_width(620)
        .width(Length::Fill)
        .into()
}

fn sign_in_submit<'a>(sending: bool, valid: bool) -> Element<'a, ConnectAccountMessage> {
    if sending {
        iced::widget::button(
            container(text::p1_regular(localized_copy("Verifying…")).color(color::GREY_3))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fixed(44.0))
        .style(theme::button::primary)
        .into()
    } else {
        button::primary(None, localized_copy("Verify"))
            .on_press_maybe(valid.then_some(ConnectAccountMessage::VerifyTwoFactor))
            .width(Length::Fill)
            .into()
    }
}

fn append_sign_in_failure<'a>(
    mut col: Column<'a, ConnectAccountMessage>,
    failure: Option<TwoFactorStepFailure>,
    backup: bool,
) -> Column<'a, ConnectAccountMessage> {
    match failure {
        Some(TwoFactorStepFailure::Wrong) => {
            let msg = if backup {
                "That backup code is wrong or was already used. Check for typos, or try another code."
            } else {
                "That code didn't work. Codes change every 30 seconds; enter the one showing now."
            };
            col = col
                .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
                .push(status_line(x_circle_icon(), msg, Tone::Bad));
        }
        Some(TwoFactorStepFailure::Limited) => {
            col = col
                .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
                .push(status_line(
                    warning_icon(),
                    "Too many tries. Wait 5 minutes, then enter the code showing at that time.",
                    Tone::Warn,
                ));
        }
        _ => {}
    }
    col
}

/// The reminder after a sign-in that spent a backup code.
pub(super) fn backup_codes_notice<'a>(left: u32) -> Element<'a, ConnectAccountMessage> {
    let low = left <= LOW_BACKUP_CODES;
    let column = Column::new()
        .push(group_title(format!(
            "Signed in with a backup code. {}",
            backup_left_text(left)
        )))
        .push(lead(localized_copy(
            "Make a new set now, while you're signed in: Connect account › Devices & security › Backup codes. If you still have the authenticator app, you'll need it for that.",
        )))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(
            sheet_button("Dismiss", ActionKind::Secondary, true)
                .on_press(ConnectAccountMessage::DismissBackupCodesNotice),
        )
        .spacing(4);
    container(column)
        .padding([14, 16])
        .width(Length::Fill)
        .style(move |t: &theme::Theme| iced::widget::container::Style {
            border: iced::Border {
                color: if low {
                    t.colors.shell.warn
                } else {
                    t.colors.shell.line
                },
                width: 1.0,
                radius: 10.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// One settings row: title, a muted line, a status and an optional action.
fn setting_row<'a>(
    title: &'a str,
    detail: String,
    status: Option<Element<'a, ConnectAccountMessage>>,
    action: Option<Element<'a, ConnectAccountMessage>>,
) -> Element<'a, ConnectAccountMessage> {
    let mut row = Row::new()
        .push(
            Column::new()
                .push(tenshu_text::label(title))
                .push(lead(detail))
                .spacing(2)
                .width(Length::Fill),
        )
        .spacing(16)
        .align_y(Alignment::Center);
    if let Some(status) = status {
        row = row.push(status);
    }
    if let Some(action) = action {
        row = row.push(action);
    }
    container(row).padding([10, 0]).width(Length::Fill).into()
}

fn review_block<'a>(
    content: Column<'a, ConnectAccountMessage>,
) -> Element<'a, ConnectAccountMessage> {
    container(content.spacing(4))
        .padding([14, 16])
        .width(Length::Fill)
        .style(review_style)
        .into()
}

/// The code field shared by turning on, making new codes and turning off.
fn code_field<'a>(
    acct: &'a ConnectAccountPanel,
    label: &'a str,
    backup: bool,
    wrong_msg: &'a str,
    on_submit: TwoFactorSettingsMessage,
) -> Column<'a, ConnectAccountMessage> {
    let tf = &acct.two_factor;
    let valid = second_factor_code_complete(&tf.code.0, backup);
    let mut col = Column::new()
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(tenshu_text::label(label))
        .push(
            TextInput::new(if backup { "xxxx-xxxx-xxxx" } else { "000000" }, &tf.code.0)
                .id(TWO_FACTOR_SETTINGS_INPUT_ID)
                .on_input(|v| settings(TwoFactorSettingsMessage::CodeChanged(SecretInput(v))))
                .on_submit_maybe((valid && !tf.busy).then(|| settings(on_submit)))
                .size(15)
                .padding(10)
                .width(Length::Fixed(220.0)),
        );
    if tf.wrong {
        col = col.push(status_line(x_circle_icon(), wrong_msg, Tone::Bad));
    }
    col
}

fn panel_actions<'a>(
    acct: &'a ConnectAccountPanel,
    confirm: &'a str,
    confirm_kind: ActionKind,
    on_confirm: TwoFactorSettingsMessage,
    backup: bool,
) -> Element<'a, ConnectAccountMessage> {
    let tf = &acct.two_factor;
    let valid = second_factor_code_complete(&tf.code.0, backup);
    let mut confirm_btn = sheet_button(
        if tf.busy { "Checking…" } else { confirm },
        confirm_kind,
        false,
    );
    if valid && !tf.busy {
        confirm_btn = confirm_btn.on_press(settings(on_confirm));
    }
    let mut cancel = sheet_button("Cancel", ActionKind::Secondary, false);
    if !tf.busy {
        cancel = cancel.on_press(settings(TwoFactorSettingsMessage::Cancel));
    }
    Row::new().push(confirm_btn).push(cancel).spacing(12).into()
}

/// Devices & security › Authenticator app and Backup codes.
pub(super) fn settings_ux<'a>(acct: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let tf = &acct.two_factor;
    let email = acct.user.as_ref().map(|u| u.email.as_str()).unwrap_or("");

    let mut col = Column::new().push(horizontal_rule());

    let status = match &tf.status {
        TwoFactorStatusView::Loading => {
            return col
                .push(
                    container(lead(localized_copy("Checking the authenticator app…")))
                        .padding([10, 0]),
                )
                .push(horizontal_rule())
                .into();
        }
        TwoFactorStatusView::Unsupported => {
            return col
                .push(setting_row(
                    "Authenticator app",
                    "COINCUBE | Connect can't manage the authenticator app from this app yet."
                        .to_string(),
                    None,
                    None,
                ))
                .push(horizontal_rule())
                .into();
        }
        TwoFactorStatusView::Failed => {
            return col
                .push(setting_row(
                    "Authenticator app",
                    "Couldn't check whether the authenticator app is on. Reopen Devices & security to try again.".to_string(),
                    None,
                    None,
                ))
                .push(horizontal_rule())
                .into();
        }
        TwoFactorStatusView::Loaded(status) => status,
    };

    let on = status.enabled;
    let closed = matches!(tf.panel, TwoFactorPanel::Closed);
    let tfa_status = if on {
        status_line(check_circle_icon(), "On", Tone::Good)
    } else {
        status_line(x_circle_icon(), "Off", Tone::Muted)
    };
    let tfa_action: Option<Element<ConnectAccountMessage>> = closed.then(|| {
        let (label, msg) = if on {
            ("Turn off…", TwoFactorSettingsMessage::AskDisable)
        } else {
            ("Turn on…", TwoFactorSettingsMessage::StartSetup)
        };
        sheet_button(label, ActionKind::Secondary, true)
            .on_press(settings(msg))
            .into()
    });
    let tfa_detail = if on {
        "Asked for after the email code, each time you sign in.".to_string()
    } else {
        "Adds a second step after the email code. Stops someone who can read your email from signing in as you.".to_string()
    };
    col = col
        .push(setting_row(
            "Authenticator app",
            tfa_detail,
            Some(tfa_status),
            tfa_action,
        ))
        .push(horizontal_rule());

    match tf.note {
        Some(TwoFactorNote::TurnedOn) if on && closed => {
            col = col.push(container(status_line(
                check_circle_icon(),
                "Authenticator app turned on. Your next sign-in on any device will ask for a code.",
                Tone::Good,
            )).padding([8, 0]));
        }
        Some(TwoFactorNote::TurnedOff) if !on && closed => {
            col = col.push(
                container(status_line(
                    check_circle_icon(),
                    format!("Authenticator app turned off. We emailed {email} to confirm."),
                    Tone::Good,
                ))
                .padding([8, 0]),
            );
        }
        _ => {}
    }

    match &tf.panel {
        TwoFactorPanel::Closed => {}
        TwoFactorPanel::LoadingSetup => {
            col = col.push(
                container(lead(localized_copy("Preparing your setup key…"))).padding([10, 0]),
            );
        }
        TwoFactorPanel::Setup { setup, qr } => {
            col = col.push(authenticator_setup_panel(acct, email, setup, qr));
        }
        TwoFactorPanel::Codes { codes, regenerated } => {
            col = col.push(issued_backup_codes_panel(tf, codes, *regenerated));
        }
        TwoFactorPanel::Disable { backup } => {
            col = col.push(disable_authenticator_panel(acct, *backup));
        }
        TwoFactorPanel::Regenerate => {}
    }

    if on {
        col = append_backup_code_settings(col, acct, status, closed);
    }

    col.width(Length::Fill).into()
}

fn authenticator_setup_panel<'a>(
    acct: &'a ConnectAccountPanel,
    email: &str,
    setup: &TotpSetup,
    qr: &'a Option<iced::widget::qr_code::Data>,
) -> Element<'a, ConnectAccountMessage> {
    let mut scan = Row::new().spacing(20).align_y(Alignment::Start);
    if let Some(data) = qr {
        scan = scan.push(
            container(iced::widget::qr_code(data).cell_size(4).style(|_| {
                iced::widget::qr_code::Style {
                    background: color::WHITE,
                    cell: color::BLACK,
                }
            }))
            .padding(8)
            .style(|_: &theme::Theme| container::Style {
                background: Some(iced::Background::Color(color::WHITE)),
                border: iced::Border {
                    radius: 6.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        );
    }
    scan = scan.push(
        Column::new()
            .push(group_title(localized_copy("Scan this with your authenticator app")))
            .push(lead(format!(
                "It adds COINCUBE ({email}). Any app that shows 6-digit codes works: 1Password, Google Authenticator, Authy and others."
            )))
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(tenshu_text::label(localized_copy("Can't scan? Enter this setup key instead")))
            .push(
                Row::new()
                    .push(
                        tenshu_text::body(setup.grouped_secret())
                            .font(coincube_ui::font::MONO_REGULAR),
                    )
                    .push(
                        sheet_link("Copy key")
                            .on_press(settings(TwoFactorSettingsMessage::CopySetupKey)),
                    )
                    .spacing(12)
                    .align_y(Alignment::Center),
            )
            .push(lead(localized_copy(
                "Treat the key like a password. Anyone who has it can make your codes.",
            )))
            .spacing(4)
            .width(Length::Fill),
    );
    let content = Column::new()
        .push(scan)
        .push(code_field(
            acct,
            "Enter the 6-digit code the app shows",
            false,
            "That code didn't match. Enter the one showing now; codes change every 30 seconds. If it keeps failing, check that this computer sets its clock automatically.",
            TwoFactorSettingsMessage::ConfirmSetup,
        ))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(panel_actions(
            acct,
            "Turn on",
            ActionKind::Primary,
            TwoFactorSettingsMessage::ConfirmSetup,
            false,
        ))
        .push(lead(localized_copy(
            "Next you'll get 10 backup codes. Keep them somewhere safe: they're how you get back into Connect if you lose this app.",
        )));
    review_block(content)
}

fn issued_backup_codes_panel<'a>(
    tf: &TwoFactorSettings,
    codes: &'a BackupCodes,
    regenerated: bool,
) -> Element<'a, ConnectAccountMessage> {
    let mut grid = Column::new().spacing(6);
    for pair in codes.codes.chunks(2) {
        let mut row = Row::new().spacing(24);
        for code in pair {
            row = row.push(
                tenshu_text::body(code.as_str())
                    .font(coincube_ui::font::MONO_REGULAR)
                    .width(Length::Fixed(150.0)),
            );
        }
        grid = grid.push(row);
    }
    let mut content = Column::new()
        .push(group_title(localized_copy(if regenerated {
            "Save your new backup codes"
        } else {
            "Save your backup codes"
        })))
        .push(lead(localized_copy(
            "If you lose your authenticator app, each code signs you in once in its place. This is the only time COINCUBE shows them.",
        )))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(grid)
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(
            Row::new()
                .push(
                    sheet_button("Copy all", ActionKind::Secondary, false)
                        .on_press(settings(TwoFactorSettingsMessage::CopyBackupCodes)),
                )
                .push(
                    sheet_button("Save as text file…", ActionKind::Secondary, false)
                        .on_press(settings(TwoFactorSettingsMessage::SaveBackupCodes)),
                )
                .push(
                    sheet_button("I've saved them", ActionKind::Primary, false)
                        .on_press(settings(TwoFactorSettingsMessage::FinishBackupCodes)),
                )
                .spacing(12),
        );
    match &tf.saved_to_file {
        Some(Ok(())) => {
            content = content.push(status_line(
                check_circle_icon(),
                "Saved to a text file.",
                Tone::Good,
            ));
        }
        Some(Err(_)) => {
            content = content.push(status_line(
                x_circle_icon(),
                "Couldn't save the file. Copy the codes instead, or choose another folder.",
                Tone::Bad,
            ));
        }
        None => {}
    }
    content = content.push(lead(localized_copy(
        "Store them apart from this computer, for example in a password manager or on paper. Anyone with a code and access to your email can sign in as you.",
    )));
    review_block(content)
}

fn disable_authenticator_panel<'a>(
    acct: &'a ConnectAccountPanel,
    backup: bool,
) -> Element<'a, ConnectAccountMessage> {
    let tf = &acct.two_factor;
    let content = Column::new()
        .push(group_title(localized_copy(
            "Turn off the authenticator app?",
        )))
        .push(lead(localized_copy(
            "Signing in to Connect will only need the code we email you.",
        )))
        .push(code_field(
            acct,
            if backup {
                "Backup code"
            } else {
                "Code from your authenticator app"
            },
            backup,
            if backup {
                "That backup code is wrong or was already used."
            } else {
                "That code didn't match. Enter the one showing now."
            },
            TwoFactorSettingsMessage::ConfirmDisable,
        ))
        .push(
            Row::new()
                .push(lead(localized_copy(if backup {
                    "Have the app?"
                } else {
                    "No longer have the app?"
                })))
                .push(
                    sheet_link(if backup {
                        "Use a code from the app instead"
                    } else {
                        "Use a backup code instead"
                    })
                    .on_press_maybe(
                        (!tf.busy).then(|| settings(TwoFactorSettingsMessage::ToggleDisableBackup)),
                    ),
                )
                .spacing(6)
                .align_y(Alignment::Center),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(panel_actions(
            acct,
            "Turn off",
            ActionKind::Destructive,
            TwoFactorSettingsMessage::ConfirmDisable,
            backup,
        ));
    review_block(content)
}

fn append_backup_code_settings<'a>(
    mut col: Column<'a, ConnectAccountMessage>,
    acct: &'a ConnectAccountPanel,
    status: &TwoFactorStatus,
    closed: bool,
) -> Column<'a, ConnectAccountMessage> {
    let tf = &acct.two_factor;
    let left = status.remaining;
    let low = left <= LOW_BACKUP_CODES;
    let detail = if left == 0 {
        "None left. Make a new set so you can still get in without the app.".to_string()
    } else if low {
        "Make a new set before you run out. Each code works once.".to_string()
    } else {
        "Each code works once in place of the app.".to_string()
    };
    let total = if status.total == 0 { 10 } else { status.total };
    let bc_status = if low {
        status_line(
            warning_icon(),
            format!("{left} of {total} left"),
            Tone::Warn,
        )
    } else {
        status_line(
            check_circle_icon(),
            format!("{left} of {total} left"),
            Tone::Good,
        )
    };
    let bc_action: Option<Element<ConnectAccountMessage>> = closed.then(|| {
        sheet_button("Make new codes…", ActionKind::Secondary, true)
            .on_press(settings(TwoFactorSettingsMessage::AskRegenerate))
            .into()
    });
    col = col
        .push(setting_row(
            "Backup codes",
            detail,
            Some(bc_status),
            bc_action,
        ))
        .push(horizontal_rule());
    if matches!(tf.panel, TwoFactorPanel::Regenerate) {
        let content = Column::new()
            .push(group_title(localized_copy("Make new backup codes?")))
            .push(lead(localized_copy(
                "Your current codes stop working, including any you haven't used.",
            )))
            .push(code_field(
                acct,
                "Code from your authenticator app",
                false,
                "That code didn't match. Enter the one showing now.",
                TwoFactorSettingsMessage::ConfirmRegenerate,
            ))
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(panel_actions(
                acct,
                "Make new codes",
                ActionKind::Primary,
                TwoFactorSettingsMessage::ConfirmRegenerate,
                false,
            ));
        col = col.push(review_block(content));
    }
    col
}
