use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
mod billing;
mod contacts;
use billing::plan_billing_ux;
#[cfg(test)]
use billing::renewal_banner;
pub mod cube_members;
pub mod duress_contacts;
pub mod duress_enroll;
pub mod sign_in_prompt;

use coincube_ui::{
    color,
    component::{button, notification, text},
    icon::*,
    image::coincube_wordmark,
    theme,
    widget::{action::Action as FocusableAction, *},
};
use coincube_ui::{
    component::tenshu::text as tenshu_text,
    theme::tenshu::{
        button::{ActionKind, ActionShape},
        Tone,
    },
};
use iced::widget::text::IntoFragment;
use iced::{
    advanced::widget::Id,
    widget::{container, scrollable},
    Alignment, Length,
};

use crate::{
    app::{
        menu::ConnectSubMenu,
        state::connect::{
            duress_gate_blocked, duress_tier1_gate_blocked, AvatarFlowStep, ConnectAccountPanel,
            ConnectCubePanel, ConnectFlowStep, ConnectPanel, CubeBackupCompleteness,
            DuressContactsStep, DuressCube, DuressGateStatus,
        },
        view::{AvatarMessage, ConnectAccountMessage, ConnectCubeMessage, DuressMessage},
    },
    services::coincube::{
        AvatarAccentMotif, AvatarAgeFeel, AvatarArchetype, AvatarArmorStyle, AvatarDemeanor,
        AvatarGender, ConnectPlan,
    },
};

use crate::app::state::connect::account::{
    PanelError, RecoveryAlertEligibility, RecoveryKitReport, RecoveryServicesState,
};

use crate::app::view::Message as ViewMessage;

pub fn connect_panel<'a>(state: &'a ConnectPanel) -> Element<'a, ViewMessage> {
    let acct = &state.account;
    if acct.requires_authenticated_reopen() {
        return text::p1_regular(localized_copy("Connect authorization ended. Close and reopen this Cube through authenticated Connect startup to reconnect.")).into();
    }

    let header = Row::new()
        .push(coincube_wordmark::<ViewMessage>(20.0))
        .push(text::h5_regular(localized_copy(" | CONNECT")).color(color::GREY_3))
        .align_y(Alignment::Center);

    let body: Element<ViewMessage> = match &acct.step {
        ConnectFlowStep::CheckingSession => Column::new()
            .push(text::p1_regular(localized_copy("Loading…")).color(color::GREY_3))
            .align_x(Alignment::Center)
            .into(),

        ConnectFlowStep::Login { email, loading } => {
            login_ux(email, *loading).map(ViewMessage::ConnectAccount)
        }

        ConnectFlowStep::Register { email, loading } => {
            register_ux(email, &acct.register_campaign_code, *loading)
                .map(ViewMessage::ConnectAccount)
        }

        ConnectFlowStep::OtpVerification {
            email,
            otp,
            sending,
            cooldown,
            ..
        } => otp_ux(email, otp, *sending, *cooldown).map(ViewMessage::ConnectAccount),

        ConnectFlowStep::CheckingDuress { status } => {
            checking_duress_ux(*status).map(ViewMessage::ConnectAccount)
        }

        ConnectFlowStep::DuressRecovery {
            unlock_at,
            passphrase,
            submitting,
            cleared,
        } => duress_enroll::recovery_ux(unlock_at.as_ref(), passphrase, *submitting, *cleared)
            .map(ViewMessage::ConnectAccount),

        ConnectFlowStep::Dashboard => match &acct.active_sub {
            ConnectSubMenu::Overview => overview_ux(acct).map(ViewMessage::ConnectAccount),
            ConnectSubMenu::PlanBilling => plan_billing_ux(acct).map(ViewMessage::ConnectAccount),
            ConnectSubMenu::Security => security_ux(acct).map(ViewMessage::ConnectAccount),
            ConnectSubMenu::Duress => duress_ux(acct).map(ViewMessage::ConnectAccount),
            ConnectSubMenu::Contacts => {
                contacts::contacts_ux(acct).map(ViewMessage::ConnectAccount)
            }
        },
    };

    let is_auth_step = !matches!(acct.step, ConnectFlowStep::Dashboard);
    let col_align = if is_auth_step {
        Alignment::Center
    } else {
        Alignment::Start
    };

    let mut col = Column::new()
        .push(header)
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .spacing(0)
        .align_x(col_align)
        .width(Length::Fill);

    if let Some(step) = auth_step(&acct.step) {
        col = col.push(crate::home::restore_frame::stepper(&AUTH_STEPS, step));
        col = col.push(iced::widget::Space::new().height(Length::Fixed(22.0)));
    }

    if let Some(e) = acct.error.as_ref() {
        col = col.push(account_error_card(e).map(ViewMessage::ConnectAccount));
    }

    col.push(body).into()
}

/// Renders account-level Connect views (used by the home).
/// Returns Element<ConnectAccountMessage> (not ViewMessage) so the caller can map it.
pub fn connect_account_panel<'a>(
    acct: &'a ConnectAccountPanel,
) -> Element<'a, ConnectAccountMessage> {
    if acct.requires_authenticated_reopen() {
        return text::p1_regular(localized_copy("Connect authorization ended. Close and reopen this Cube through authenticated Connect startup to reconnect.")).into();
    }
    let header = Row::new()
        .push(coincube_wordmark::<ConnectAccountMessage>(20.0))
        .push(text::h5_regular(localized_copy(" | CONNECT")).color(color::GREY_3))
        .align_y(Alignment::Center);

    let body = connect_account_body(acct);

    let is_auth_step = !matches!(
        acct.step,
        ConnectFlowStep::Dashboard | ConnectFlowStep::DuressRecovery { .. }
    );
    let col_align = if is_auth_step {
        Alignment::Center
    } else {
        Alignment::Start
    };

    let mut col = Column::new()
        .push(header)
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .spacing(0)
        .align_x(col_align)
        .width(Length::Fill);
    if let Some(step) = auth_step(&acct.step) {
        col = col.push(crate::home::restore_frame::stepper(&AUTH_STEPS, step));
        col = col.push(iced::widget::Space::new().height(Length::Fixed(22.0)));
    }
    col.push(body).into()
}

/// Intents owned by the shell that mounts the Connect sheet. The account panel
/// continues to own authentication, billing and security operations; the shell
/// owns the origin, section route and Close lifecycle.
#[derive(Debug, Clone)]
pub enum ConnectSheetMessage {
    Account(ConnectAccountMessage),
    FocusAction(Id),
    SelectSection(ConnectSheetSection),
    OpenAuthentication {
        create_account: bool,
    },
    /// "Not now" on the signed-out-by-Connect notice.
    DismissSignOut,
    Close,
}

pub const CLOSE_ID: &str = "tenshu.connect.close";
pub const CONTENT_SCROLL_ID: &str = "tenshu.connect.content-scroll";
pub const EMAIL_INPUT_ID: &str = "tenshu.connect.auth.email";
pub const OTP_INPUT_ID: &str = "tenshu.connect.auth.otp";

/// Account-wide sections shown in the Connect sheet. Recovery
/// services has its own section here without changing the older Home menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectSheetSection {
    Account,
    PlanBilling,
    Contacts,
    DevicesSecurity,
    Duress,
    RecoveryServices,
}

/// The sheet's account navigation and the separate email/token flow. Its
/// owner retains this page while the underlying account state changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectSheetPage {
    Sections(ConnectSheetSection),
    Authentication,
}

/// Connect's content inside an origin-preserving sheet. The caller draws the
/// retained Launcher or Cube behind it and routes Close to that exact origin.
/// This view never creates another account state or signs out on Close.
///
/// Geometry follows the Connect artboard (`design-review/canvas/P-Connect.dc.html`):
/// a 72px header over a rule, a 232px section list with its own rule, and
/// content inset 28/40px with a 680px reading width.
pub fn connect_sheet<'a>(
    acct: &'a ConnectAccountPanel,
    page: ConnectSheetPage,
) -> Element<'a, ConnectSheetMessage> {
    let blocked = acct.requires_authenticated_reopen();
    let signed_in = matches!(acct.step, ConnectFlowStep::Dashboard) && !blocked;
    // Once token entry or duress verification begins, keep the real auth flow
    // visible even if a stale caller page still names an account section.
    let auth_in_progress = matches!(
        acct.step,
        ConnectFlowStep::OtpVerification { .. }
            | ConnectFlowStep::CheckingDuress { .. }
            | ConnectFlowStep::DuressRecovery { .. }
    );
    let show_auth =
        !signed_in && (matches!(page, ConnectSheetPage::Authentication) || auth_in_progress);
    let requested_section = match page {
        ConnectSheetPage::Sections(section) => section,
        ConnectSheetPage::Authentication => ConnectSheetSection::Account,
    };
    // The sheet can outlive a feature refresh or account change. Never keep
    // painting a hidden Duress route after its entitlement or launch gate drops.
    let active_section =
        if requested_section == ConnectSheetSection::Duress && !(signed_in && acct.show_duress()) {
            ConnectSheetSection::Account
        } else {
            requested_section
        };
    // A stored session that couldn't be checked is still this account's:
    // the sheet says why instead of offering the sign-in form (L1).
    let unverified = (!signed_in && !blocked)
        .then_some(acct.unverified.as_ref())
        .flatten();
    let title = if signed_in || unverified.is_some() {
        "Connect account"
    } else {
        "Connect"
    };
    let identity = signed_in
        .then_some(acct.user.as_ref())
        .flatten()
        .map(|user| user.email.as_str())
        .or(unverified.map(|u| u.email.as_str()));
    let mut heading = Row::new()
        .push(
            iced::widget::text(localized_copy(title))
                .size(24)
                .font(coincube_ui::font::SEMIBOLD)
                .style(theme::tenshu::text::ink),
        )
        .spacing(16)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    if let Some(email) = identity {
        heading = heading.push(tenshu_text::body(email).style(theme::tenshu::text::muted));
    }
    heading = heading
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(
            FocusableAction::new(
                CLOSE_ID,
                centred(
                    Row::new()
                        .push(cross_icon().size(14).style(theme::tenshu::text::ink))
                        .push(tenshu_text::label(localized_copy("Close")))
                        .spacing(6)
                        .align_y(Alignment::Center),
                ),
                ConnectSheetMessage::FocusAction,
            )
            .height(Length::Fixed(36.0))
            .padding([0, 12])
            .style(|theme, status| sheet_button_style(theme, status, ActionKind::Secondary))
            .on_press(ConnectSheetMessage::Close),
        );

    let content: Element<ConnectSheetMessage> = if blocked {
        text::p1_regular("Connect authorization ended. Close and reopen this Cube through authenticated Connect startup to reconnect.").into()
    } else if let Some(unverified) = unverified.filter(|_| !show_auth) {
        unverified_account_ux(unverified)
    } else if show_auth {
        connect_account_body(acct).map(ConnectSheetMessage::Account)
    } else if acct.is_checking_session() {
        Column::new()
            .push(sheet_title(section_label(active_section)))
            .push(lead(localized_copy("Loading your Connect account…")))
            .spacing(4)
            .into()
    } else if signed_in {
        let section_content = match active_section {
            ConnectSheetSection::Account => sheet_account_ux(acct),
            ConnectSheetSection::PlanBilling => plan_billing_ux(acct),
            ConnectSheetSection::Contacts => contacts::contacts_ux(acct),
            ConnectSheetSection::DevicesSecurity => security_ux(acct),
            ConnectSheetSection::Duress => duress_ux(acct),
            ConnectSheetSection::RecoveryServices => recovery_services_ux(acct),
        };
        let mut column = Column::new().spacing(12);
        if let Some(error) = acct.error.as_ref() {
            column = column.push(account_error_card(error));
        }
        let content: Element<ConnectAccountMessage> = column.push(section_content).into();
        content.map(ConnectSheetMessage::Account)
    } else if active_section == ConnectSheetSection::Account {
        signed_out_account_ux(acct.signed_out_by)
    } else {
        signed_out_section_ux(active_section)
    };
    let body: Element<ConnectSheetMessage> = if !show_auth {
        let mut sections = vec![
            (
                "tenshu.connect.section.account",
                ConnectSheetSection::Account,
            ),
            (
                "tenshu.connect.section.plan",
                ConnectSheetSection::PlanBilling,
            ),
            (
                "tenshu.connect.section.contacts",
                ConnectSheetSection::Contacts,
            ),
            (
                "tenshu.connect.section.security",
                ConnectSheetSection::DevicesSecurity,
            ),
        ];
        if signed_in && acct.show_duress() {
            sections.push(("tenshu.connect.section.duress", ConnectSheetSection::Duress));
        }
        sections.push((
            "tenshu.connect.section.recovery",
            ConnectSheetSection::RecoveryServices,
        ));
        let mut navigation = Column::new().spacing(6).height(Length::Fill);
        for (id, section) in sections {
            let selected = section == active_section;
            let icon = match section {
                ConnectSheetSection::Account => person_outline_icon(),
                ConnectSheetSection::PlanBilling => credit_card_icon(),
                ConnectSheetSection::Contacts => people_icon(),
                ConnectSheetSection::DevicesSecurity => laptop_icon(),
                ConnectSheetSection::Duress => shield_check_icon(),
                ConnectSheetSection::RecoveryServices => shield_check_icon(),
            };
            let label = tenshu_text::body(section_label(section));
            navigation = navigation.push(
                FocusableAction::new(
                    id,
                    centred(
                        Row::new()
                            .push(icon.size(18).style(if selected {
                                theme::tenshu::text::accent
                            } else {
                                theme::tenshu::text::muted
                            }))
                            .push(if selected {
                                label.font(coincube_ui::font::SEMIBOLD)
                            } else {
                                label
                            })
                            .spacing(10)
                            .align_y(Alignment::Center),
                    ),
                    ConnectSheetMessage::FocusAction,
                )
                .width(Length::Fill)
                .height(Length::Fixed(theme::tenshu::size::NAV_ITEM_HEIGHT))
                .padding([0, 12])
                .style(move |theme, status| {
                    theme::tenshu::control::nav_item(theme, status, selected)
                })
                .on_press(ConnectSheetMessage::SelectSection(section)),
            );
        }
        navigation = navigation
            .push(iced::widget::Space::new().height(Length::Fill))
            .push(
                container(tenshu_text::caption(
                    localized_copy("Account-wide. Each Cube's own service connection stays in that Cube's Settings › Connections."),
                ))
                .padding([0, 12]),
            );
        let reading_width = if signed_in && active_section == ConnectSheetSection::RecoveryServices
        {
            f32::INFINITY
        } else {
            680.0
        };
        Row::new()
            .push(
                container(navigation)
                    .padding([16, 12])
                    .width(Length::Fixed(232.0))
                    .height(Length::Fill),
            )
            .push(vertical_rule())
            .push(
                scrollable(
                    container(container(content).max_width(reading_width)).padding(iced::Padding {
                        top: 28.0,
                        right: 40.0,
                        bottom: 40.0,
                        left: 40.0,
                    }),
                )
                .id(CONTENT_SCROLL_ID)
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .height(Length::Fill)
            .into()
    } else {
        container(
            scrollable(container(content).padding(iced::Padding {
                right: 30.0,
                ..iced::Padding::ZERO
            }))
            .id(CONTENT_SCROLL_ID)
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .max_width(600)
        .center_x(Length::Fill)
        .height(Length::Fill)
        .padding(28)
        .into()
    };

    // The slot stays when there is no rail (a failed account check, duress
    // recovery) so the body keeps its place in the tree.
    let rail: Element<ConnectSheetMessage> = match auth_step(&acct.step) {
        Some(step) if show_auth && !blocked => auth_rail(step),
        _ => iced::widget::Space::new().width(Length::Fill).into(),
    };

    container(
        Column::new()
            .push(
                container(heading)
                    .height(Length::Fixed(72.0))
                    .center_y(Length::Fixed(72.0))
                    .padding(iced::Padding {
                        left: 28.0,
                        right: 24.0,
                        ..iced::Padding::ZERO
                    }),
            )
            .push(horizontal_rule())
            .push(rail)
            .push(body)
            .spacing(0)
            .height(Length::Fill),
    )
    .max_width(if show_auth { 600 } else { 1040 })
    .height(Length::Fill)
    .style(sheet_style)
    .into()
}

fn section_label(section: ConnectSheetSection) -> &'static str {
    match section {
        ConnectSheetSection::Account => localized_copy("Account"),
        ConnectSheetSection::PlanBilling => localized_copy("Plan & billing"),
        ConnectSheetSection::Contacts => localized_copy("Contacts"),
        ConnectSheetSection::DevicesSecurity => localized_copy("Devices & security"),
        ConnectSheetSection::Duress => localized_copy("Duress mode"),
        ConnectSheetSection::RecoveryServices => localized_copy("Recovery services"),
    }
}

/// The sheet surface (`.sheet`): panel fill, 1px line, 12px radius.
fn sheet_style(theme: &theme::Theme) -> container::Style {
    let shell = &theme.colors.shell;
    container::Style {
        background: Some(iced::Background::Color(shell.panel)),
        border: iced::Border {
            color: shell.line,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    }
}

/// `.btn` / `.btn.primary`: 8px-radius buttons in the sheet's own scale.
fn sheet_button_style(
    theme: &theme::Theme,
    status: iced::widget::button::Status,
    kind: ActionKind,
) -> iced::widget::button::Style {
    theme::tenshu::button::action(theme, status, kind, ActionShape::Compact)
}

/// Label ink for a sheet button; the theme's default text class would
/// otherwise override the button's own ink.
fn action_ink(kind: ActionKind) -> impl Fn(&theme::Theme) -> iced::widget::text::Style {
    move |theme| iced::widget::text::Style {
        color: Some(match kind {
            ActionKind::Primary => theme::tenshu::ON_ACCENT,
            ActionKind::Quiet => theme.colors.shell.accent_ink,
            ActionKind::Destructive => theme.colors.shell.bad,
            ActionKind::Secondary => theme.colors.shell.ink,
        }),
    }
}

/// Centres a control's content on its fixed height.
fn centred<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    container(content).center_y(Length::Fill).into()
}

/// A labelled `.btn` (36px) or `.btn.sm` (30px) for account-panel messages.
fn sheet_button<'a, M: Clone + 'a>(
    label: impl IntoFragment<'a>,
    kind: ActionKind,
    small: bool,
) -> iced::widget::Button<'a, M, theme::Theme> {
    let (height, size, pad) = if small {
        (30.0, 13.0, 12)
    } else {
        (36.0, 14.0, 16)
    };
    iced::widget::button(centred(
        iced::widget::text(label)
            .size(size)
            .font(coincube_ui::font::MEDIUM)
            .style(action_ink(kind)),
    ))
    .height(Length::Fixed(height))
    .padding([0, pad])
    .style(move |theme, status| sheet_button_style(theme, status, kind))
}

/// `.linkbtn`: underlined-looking quiet text action.
fn sheet_link<'a, M: Clone + 'a>(
    label: impl IntoFragment<'a>,
) -> iced::widget::Button<'a, M, theme::Theme> {
    iced::widget::button(
        iced::widget::text(label)
            .size(13)
            .style(action_ink(ActionKind::Quiet)),
    )
    .padding([2, 0])
    .style(|theme, status| sheet_button_style(theme, status, ActionKind::Quiet))
}

/// Section heading (`.sec h3`), 20px semibold.
fn sheet_title<'a>(label: impl IntoFragment<'a>) -> iced::widget::Text<'a, theme::Theme> {
    tenshu_text::heading(label)
}

/// Group heading (`.grp h4`), 14px semibold.
fn group_title<'a>(label: impl IntoFragment<'a>) -> iced::widget::Text<'a, theme::Theme> {
    tenshu_text::body(label).font(coincube_ui::font::SEMIBOLD)
}

/// Muted 13px line under a heading (`.lead`, `.s`, `.conseq`).
fn lead<'a>(label: impl IntoFragment<'a>) -> iced::widget::Text<'a, theme::Theme> {
    tenshu_text::body(label)
        .size(13)
        .style(theme::tenshu::text::muted)
}

/// 12px muted footnote (`.fnote`).
fn footnote<'a>(label: impl IntoFragment<'a>) -> iced::widget::Text<'a, theme::Theme> {
    tenshu_text::caption(label)
}

fn horizontal_rule<'a, M: 'a>() -> Element<'a, M> {
    container(iced::widget::Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(theme::tenshu::control::divider)
        .into()
}

fn vertical_rule<'a, M: 'a>() -> Element<'a, M> {
    container(iced::widget::Space::new())
        .width(Length::Fixed(1.0))
        .height(Length::Fill)
        .style(theme::tenshu::control::divider)
        .into()
}

/// A ruled list (`.list` + `.li`): a rule above the first row and under every row.
fn ruled_list<'a, M: 'a>(rows: impl IntoIterator<Item = Element<'a, M>>) -> Element<'a, M> {
    let mut list = Column::new().push(horizontal_rule());
    for row in rows {
        list = list
            .push(container(row).padding([10, 0]).width(Length::Fill))
            .push(horizontal_rule());
    }
    list.width(Length::Fill).into()
}

/// A label/value list row (`.li .k` + `.main`) with the 168px key column.
fn key_value_row<'a, M: 'a>(key: &'a str, value: Element<'a, M>) -> Element<'a, M> {
    Row::new()
        .push(lead(key).width(Length::Fixed(168.0)))
        .push(container(value).width(Length::Fill))
        .spacing(16)
        .height(Length::Fixed(32.0))
        .align_y(Alignment::Center)
        .into()
}

/// A status with its glyph (`.stt` + tone).
fn status_line<'a, M: 'a>(
    icon: iced::widget::Text<'a, theme::Theme>,
    label: impl IntoFragment<'a>,
    tone: Tone,
) -> Element<'a, M> {
    Row::new()
        .push(
            icon.size(16)
                .style(move |t| theme::tenshu::text::tone(t, tone)),
        )
        .push(
            tenshu_text::body(label)
                .size(13)
                .style(move |t| theme::tenshu::text::tone(t, tone)),
        )
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
}

/// A small bordered tag (`.rtag`), accent-bordered when `current`.
fn row_tag<'a, M: 'a>(label: impl IntoFragment<'a>, current: bool) -> Element<'a, M> {
    container(
        iced::widget::text(label)
            .size(12)
            .font(coincube_ui::font::MEDIUM)
            .style(theme::tenshu::text::ink),
    )
    .padding([1, 6])
    .style(move |theme: &theme::Theme| {
        let shell = &theme.colors.shell;
        container::Style {
            border: iced::Border {
                color: if current { shell.accent } else { shell.line },
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        }
    })
    .into()
}

/// The selected-fill review block (`.review`).
fn review_style(theme: &theme::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.selected)),
        border: iced::Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A bordered notice at the top of the sheet: a title, a line and actions.
fn sheet_notice<'a>(
    bad: bool,
    title: &'static str,
    detail: String,
    actions: Option<Element<'a, ConnectSheetMessage>>,
) -> Element<'a, ConnectSheetMessage> {
    let mut column = Column::new()
        .spacing(4)
        .push(group_title(title))
        .push(lead(detail));
    if let Some(actions) = actions {
        column = column
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(actions);
    }
    container(column)
        .padding([14, 16])
        .width(Length::Fill)
        .style(move |t: &theme::Theme| iced::widget::container::Style {
            border: iced::Border {
                color: if bad {
                    t.colors.shell.bad
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

/// The sheet for a stored session that couldn't be checked: still signed
/// in, waiting for the network or Connect (L1).
fn unverified_account_ux<'a>(
    unverified: &crate::app::state::connect::account::UnverifiedSession,
) -> Element<'a, ConnectSheetMessage> {
    use crate::services::connect::grpc::trouble::Trouble;
    let (title, detail) = match unverified.trouble {
        Trouble::Offline => (
            "No internet connection",
            "You’re still signed in. Tenshu checks your Connect session again when you’re back \
             online. Connect-only Cubes can be restored then.",
        ),
        Trouble::Unreachable => (
            "Can’t reach COINCUBE | Connect",
            "You’re still signed in. Tenshu keeps trying. Connect-only Cubes can be restored once \
             it’s back.",
        ),
    };
    Column::new()
        .push(sheet_title(localized_copy("Account")))
        .push(iced::widget::Space::new().height(Length::Fixed(16.0)))
        .push(sheet_notice(false, title, detail.to_owned(), None))
        .width(Length::Fill)
        .into()
}

/// The signed-out sheet, led by the reason when Connect ended the session.
fn signed_out_account_ux<'a>(
    signed_out_by: Option<crate::app::SignedOutBy>,
) -> Element<'a, ConnectSheetMessage> {
    let mut column = Column::new();
    if let Some(reason) = signed_out_by {
        column = column
            .push(sheet_notice(
                true,
                reason.title(),
                "Sign in to see your Connect-only Cubes and to back up Recovery Kits to Connect \
                 again."
                    .to_owned(),
                Some(
                    Row::new()
                        .push(sheet_action(
                            "tenshu.connect.notice.sign-in",
                            "Sign in",
                            ActionKind::Primary,
                            ConnectSheetMessage::OpenAuthentication {
                                create_account: false,
                            },
                        ))
                        .push(sheet_action(
                            "tenshu.connect.notice.not-now",
                            "Not now",
                            ActionKind::Secondary,
                            ConnectSheetMessage::DismissSignOut,
                        ))
                        .spacing(12)
                        .into(),
                ),
            ))
            .push(iced::widget::Space::new().height(Length::Fixed(24.0)));
    }
    column
        .push(sheet_title("Account"))
        .push(iced::widget::Space::new().height(Length::Fixed(16.0)))
        .push(connect_benefits(true).map(ConnectSheetMessage::Account))
        .push(iced::widget::Space::new().height(Length::Fixed(24.0)))
        .push(
            Row::new()
                .push(sheet_action(
                    "tenshu.connect.sign-in",
                    "Sign in",
                    ActionKind::Primary,
                    ConnectSheetMessage::OpenAuthentication {
                        create_account: false,
                    },
                ))
                .push(sheet_action(
                    "tenshu.connect.create-account",
                    "Create account",
                    ActionKind::Secondary,
                    ConnectSheetMessage::OpenAuthentication {
                        create_account: true,
                    },
                ))
                .spacing(12),
        )
        .width(Length::Fill)
        .into()
}

/// A focusable `.btn` owned by the sheet shell.
fn sheet_action<'a>(
    id: &'static str,
    label: &'static str,
    kind: ActionKind,
    message: ConnectSheetMessage,
) -> Element<'a, ConnectSheetMessage> {
    FocusableAction::new(
        id,
        centred(
            iced::widget::text(label)
                .size(14)
                .font(coincube_ui::font::MEDIUM)
                .style(action_ink(kind)),
        ),
        ConnectSheetMessage::FocusAction,
    )
    .height(Length::Fixed(36.0))
    .padding([0, 16])
    .style(move |theme, status| sheet_button_style(theme, status, kind))
    .on_press(message)
    .into()
}

/// Account-wide presentation shared by the sheet and the older Home panel.
/// Plan, counts, and Recovery services stay in their own destinations.
fn sheet_account_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let email = state
        .user
        .as_ref()
        .map(|user| user.email.as_str())
        .unwrap_or("—");
    let avatar = container(
        person_outline_icon()
            .size(24)
            .style(theme::tenshu::text::accent),
    )
    .width(48)
    .height(48)
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(|theme: &theme::Theme| container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.selected)),
        border: iced::Border {
            radius: 24.0.into(),
            ..Default::default()
        },
        ..Default::default()
    });
    let column = Column::new()
        .push(sheet_title(localized_copy("Account")))
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(
            Row::new()
                .push(avatar)
                .push(
                    Column::new()
                        .push(tenshu_text::section(email).width(Length::Fill))
                        .push(lead(localized_copy("Signed in on this device")))
                        .width(Length::Fill),
                )
                .spacing(16)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(24.0)))
        .push(ruled_list([key_value_row(
            "Sign-in method",
            tenshu_text::body(localized_copy("Email · one-time code")).into(),
        )]));
    let mut column = column
        .push(iced::widget::Space::new().height(Length::Fixed(32.0)))
        .push(group_title(localized_copy("What Connect provides")))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(connect_benefits(false))
        .push(iced::widget::Space::new().height(Length::Fixed(32.0)));
    let consequence = "Cubes stay on this device; Connect services pause until you sign in again.";
    if state.review_sign_out {
        column = column.push(
            container(
                Column::new()
                    .push(group_title(localized_format!(
                        "Sign out {email} on this device?",
                        "¿Señalar {email} en este dispositivo?"
                    )))
                    .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
                    .push(tenshu_text::body(consequence).size(13))
                    .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                    .push(
                        Row::new()
                            .push(
                                sheet_button("Sign out", ActionKind::Primary, false)
                                    .on_press(ConnectAccountMessage::LogOut),
                            )
                            .push(
                                sheet_button("Cancel", ActionKind::Secondary, false)
                                    .on_press(ConnectAccountMessage::ReviewSignOut(false)),
                            )
                            .spacing(12),
                    ),
            )
            .padding([14, 16])
            .width(Length::Fill)
            .style(review_style),
        );
    } else {
        column = column
            .push(
                sheet_button("Sign out…", ActionKind::Secondary, false)
                    .on_press(ConnectAccountMessage::ReviewSignOut(true)),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(lead(consequence));
    }
    column.width(Length::Fill).into()
}

fn signed_out_section_ux<'a>(section: ConnectSheetSection) -> Element<'a, ConnectSheetMessage> {
    Column::new()
        .push(sheet_title(section_label(section)))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(
            tenshu_text::body(localized_copy("Sign in to see this"))
                .style(theme::tenshu::text::muted),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(16.0)))
        .push(sheet_action(
            "tenshu.connect.gate.sign-in",
            "Sign in",
            ActionKind::Primary,
            ConnectSheetMessage::OpenAuthentication {
                create_account: false,
            },
        ))
        .width(Length::Fill)
        .into()
}

fn connect_account_body<'a>(acct: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let body: Element<ConnectAccountMessage> = match &acct.step {
        ConnectFlowStep::CheckingSession => Column::new()
            .push(text::p1_regular(localized_copy("Loading…")).color(color::GREY_3))
            .align_x(Alignment::Center)
            .into(),
        ConnectFlowStep::Login { email, loading } => login_ux(email, *loading),
        ConnectFlowStep::Register { email, loading } => {
            register_ux(email, &acct.register_campaign_code, *loading)
        }
        ConnectFlowStep::OtpVerification {
            email,
            otp,
            sending,
            cooldown,
            ..
        } => otp_ux(email, otp, *sending, *cooldown),
        ConnectFlowStep::CheckingDuress { status } => checking_duress_ux(*status),
        ConnectFlowStep::DuressRecovery {
            unlock_at,
            passphrase,
            submitting,
            cleared,
        } => duress_enroll::recovery_ux(unlock_at.as_ref(), passphrase, *submitting, *cleared),
        ConnectFlowStep::Dashboard => match &acct.active_sub {
            ConnectSubMenu::Overview => overview_ux(acct),
            ConnectSubMenu::PlanBilling => plan_billing_ux(acct),
            ConnectSubMenu::Security => security_ux(acct),
            ConnectSubMenu::Duress => duress_ux(acct),
            ConnectSubMenu::Contacts => contacts::contacts_ux(acct),
        },
    };

    let is_auth_step = !matches!(
        acct.step,
        ConnectFlowStep::Dashboard | ConnectFlowStep::DuressRecovery { .. }
    );
    let col_align = if is_auth_step {
        Alignment::Center
    } else {
        Alignment::Start
    };

    let mut col = Column::new()
        .spacing(0)
        .align_x(col_align)
        .width(Length::Fill);

    if let Some(e) = acct.error.as_ref() {
        col = col.push(account_error_card(e));
    }

    col.push(body).into()
}

/// Parse an RFC 3339 timestamp and render it as `"Mon DD, YYYY"`
/// (e.g. `"Apr 20, 2026"`). Falls back to the raw input on parse
/// failure. Shared helper used by both the Contacts and Cube Members
/// views.
pub(super) fn format_date(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|dt| {
            dt.format(&coincube_ui::locale::date_pattern("%b %d, %Y", &dt))
                .to_string()
        })
        .unwrap_or_else(|_| iso.to_string())
}

/// Parse an RFC 3339 timestamp and render it as `"Mon DD, YYYY HH:MM"`
/// (e.g. `"Apr 20, 2026 14:31"`). Falls back to the raw input on parse failure.
pub(super) fn format_datetime(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|dt| {
            let local = dt.with_timezone(&chrono::Local);
            local
                .format(&coincube_ui::locale::date_pattern(
                    "%b %d, %Y %H:%M %Z",
                    &local,
                ))
                .to_string()
        })
        .unwrap_or_else(|_| iso.to_string())
}

/// Treat a blank device name as absent.
///
/// The API can return an empty string for device rows written before device
/// naming was fixed. `Option::unwrap_or` alone does not catch that — `Some("")`
/// is "present" as far as the option is concerned, so the fallback never fires
/// and the row renders nameless (or, in login activity, as a dangling
/// `"1.2.3.4 - "`).
pub(super) fn non_blank(name: Option<&str>) -> Option<&str> {
    name.map(str::trim).filter(|s| !s.is_empty())
}

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

/// The account panel's error card.
///
/// Replaces two byte-identical copy-pasted banners that rendered a raw error
/// string as red text — including, on a token-refresh timeout, the API host and
/// endpoint path. It also makes the retry real: the old banner's copy ended in
/// "Tap to retry" while the widget had no button and no `on_press` at all.
///
/// The button, when there is one, re-runs the operation that actually failed —
/// see [`crate::app::view::RetryAction`]. It is deliberately *not* one fixed
/// message: wiring every card to `Init` gave a button that did nothing on the
/// dashboard (`Init` returns early once signed in) and one that navigated away
/// from the code-entry screen (`Init` falls through to the login form).
///
/// Two independent things have to be true for a button to appear: the failure
/// must be worth retrying (`UserError::retryable` — an expired session or a
/// rejected 4xx is not), and this screen must have something to re-run that the
/// user cannot already press for themselves.
fn account_error_card<'a>(err: &PanelError) -> Element<'a, ConnectAccountMessage> {
    notification::error_card(
        err.user.title.clone(),
        err.user.guidance.clone(),
        err.user.reference.clone(),
        err.retry_button(),
    )
    .max_width(500)
    .into()
}

/// Product benefits from the approved Connect artboards, each with its own
/// glyph. These describe the service; the plan picker below remains the
/// source for current eligibility. The last field marks a benefit that is not
/// enabled yet. Buy/Sell, P2P and Duress are not enabled either, so they are
/// not listed (Robert, 2026-10-27; Duress 2026-11-03).
fn connect_benefit_rows<'a>() -> [(iced::widget::Text<'a, theme::Theme>, &'static str, bool); 7] {
    [
        (shield_check_icon(), "Encrypted Recovery Kit backup for each Cube — COINCUBE never sees your keys", false),
        (people_icon(), "Inheritance: invite family or friends to hold keys in the Keychain app, with time-locked recovery keys to recover funds", false),
        (bell_icon(), "Recovery alerts and emergency contacts for people you trust", false),
        (lightning_outline_icon(), "Hosted Esplora server — use your Vault instantly while your node syncs", false),
        (laptop_icon(), "Sign Vault transactions from your phone with the companion Keychain app", false),
        (lightning_outline_icon(), "Lightning Address and avatar for Spark wallets", false),
        (exclamation_circle_icon(), "Connect-enforced spending policies (Pro and Estate)", true),
    ]
}

/// "What Connect provides" (`.provides`); `compact` is the signed-out
/// variant with 13px text and tighter rows.
fn connect_benefits<'a>(compact: bool) -> Element<'a, ConnectAccountMessage> {
    let (size, gap) = if compact { (13.0, 6) } else { (14.0, 8) };
    let mut list = Column::new().spacing(gap);
    for (icon, benefit, coming_soon) in connect_benefit_rows() {
        let mut copy = Row::new()
            .push(tenshu_text::body(benefit).size(size))
            .spacing(8)
            .align_y(Alignment::Center);
        if coming_soon {
            copy = copy.push(coincube_ui::component::tenshu::status_badge(
                "Coming soon",
                Tone::Muted,
            ));
        }
        list = list.push(
            Row::new()
                .push(icon.size(18).style(theme::tenshu::text::muted))
                .push(copy.width(Length::Fill).wrap())
                .spacing(10)
                .align_y(Alignment::Start),
        );
    }
    list.width(Length::Fill).into()
}

const AUTH_STEPS: [&str; 3] = ["Email", "Token", "Done"];

/// The sign-in step rail, in the same 52px band under the sheet header that
/// Create Cube and Import or restore use, ruled off from the body.
fn auth_rail<'a, M: 'a>(current: usize) -> Element<'a, M> {
    Column::new()
        .push(
            container(crate::home::restore_frame::stepper(&AUTH_STEPS, current))
                .width(Length::Fill)
                .center_y(Length::Fixed(52.0))
                .padding([0, 28]),
        )
        .push(horizontal_rule())
        .into()
}

/// The rail position for an auth step. Done is the post-token account check
/// while it runs: the session is signed in and the sheet moves on to the
/// account once it passes, so there is no separate confirmation screen. A
/// failed check offers Retry or Sign Out, so it drops the rail rather than
/// claim the user is done.
fn auth_step(step: &ConnectFlowStep) -> Option<usize> {
    match step {
        ConnectFlowStep::Login { .. } | ConnectFlowStep::Register { .. } => Some(0),
        ConnectFlowStep::OtpVerification { .. } => Some(1),
        ConnectFlowStep::CheckingDuress {
            status: DuressGateStatus::Checking,
        } => Some(2),
        _ => None,
    }
}

fn login_ux<'a>(email: &'a str, loading: bool) -> Element<'a, ConnectAccountMessage> {
    let valid = email.contains('.') && email.contains('@') && email.len() >= 5;

    let submit: Element<ConnectAccountMessage> = if loading {
        iced::widget::button(
            container(text::p1_regular(localized_copy("Signing in…")).color(color::GREY_3))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fixed(44.0))
        .style(theme::button::primary)
        .into()
    } else {
        button::primary(None, localized_copy("Continue"))
            .on_press_maybe(valid.then_some(ConnectAccountMessage::SubmitLogin))
            .width(Length::Fill)
            .into()
    };

    Column::new()
        .push(text::h3(localized_copy("Sign in to COINCUBE")).style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(
            text::p1_regular(localized_copy(
                "Sign in to access your COINCUBE | Connect features:",
            ))
            .style(theme::text::secondary),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
        .push(connect_benefits(true))
        .push(iced::widget::Space::new().height(Length::Fixed(18.0)))
        .push(
            text::p2_regular(localized_copy(
                "Enter your email address to receive a verification code.",
            ))
            .style(theme::text::secondary),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
        .push(
            text::p2_medium(localized_copy("Email"))
                .style(theme::text::primary)
                .width(Length::Fill)
                .align_x(Alignment::Start),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        .push(
            TextInput::new("you@example.test", email)
                .id(EMAIL_INPUT_ID)
                .on_input(ConnectAccountMessage::EmailChanged)
                .on_submit_maybe((!loading && valid).then_some(ConnectAccountMessage::SubmitLogin))
                .size(16)
                .padding(15),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(submit)
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(
            iced::widget::button(
                container(
                    text::p2_regular(localized_copy("Don't have an account? Sign up"))
                        .color(color::ORANGE),
                )
                .padding(5),
            )
            .style(theme::button::link)
            .on_press(ConnectAccountMessage::CreateAccount),
        )
        .align_x(Alignment::Center)
        .spacing(2)
        .max_width(620)
        .width(Length::Fill)
        .into()
}

fn register_ux<'a>(
    email: &'a str,
    code: &'a str,
    loading: bool,
) -> Element<'a, ConnectAccountMessage> {
    let valid = email.contains('.') && email.contains('@') && email.len() >= 5;

    let submit: Element<ConnectAccountMessage> = if loading {
        iced::widget::button(
            container(text::p1_regular(localized_copy("Signing up…")).color(color::GREY_3))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fixed(44.0))
        .style(theme::button::primary)
        .into()
    } else {
        button::primary(None, localized_copy("Continue"))
            .on_press_maybe(valid.then_some(ConnectAccountMessage::SubmitRegistration))
            .width(Length::Fill)
            .into()
    };

    Column::new()
        .push(
            button::secondary(Some(previous_icon()), localized_copy("Back"))
                .width(Length::Fixed(150.0))
                .on_press_maybe((!loading).then_some(ConnectAccountMessage::LogOut)),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(text::h3(localized_copy("Create an Account")).style(theme::text::primary))
        .push(
            text::p1_regular(localized_copy(
                "Create a COINCUBE | Connect account to access these features:",
            ))
            .style(theme::text::secondary),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
        .push(connect_benefits(true))
        .push(iced::widget::Space::new().height(Length::Fixed(18.0)))
        .push(
            text::p2_medium(localized_copy("Email"))
                .style(theme::text::primary)
                .width(Length::Fill)
                .align_x(Alignment::Start),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        .push(
            TextInput::new("you@example.test", email)
                .id(EMAIL_INPUT_ID)
                .on_input(ConnectAccountMessage::EmailChanged)
                .on_submit_maybe(
                    (!loading && valid).then_some(ConnectAccountMessage::SubmitRegistration),
                )
                .size(16)
                .padding(15),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
        .push(
            text::p2_medium(localized_copy("Promo or referral code (optional)"))
                .style(theme::text::primary)
                .width(Length::Fill)
                .align_x(Alignment::Start),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        // Optional promo/referral code (server-driven campaign engine, v2).
        // Redeemed automatically once the new account authenticates. Locked
        // while signup is in flight — the code is already stashed at submit,
        // so editing the visible field would diverge from what's redeemed.
        .push(
            TextInput::new("", code)
                .on_input_maybe(
                    (!loading).then_some(ConnectAccountMessage::RegisterCampaignCodeChanged),
                )
                .on_submit_maybe(
                    (!loading && valid).then_some(ConnectAccountMessage::SubmitRegistration),
                )
                .size(16)
                .padding(15),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(submit)
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(
            iced::widget::button(
                text::p2_regular(localized_copy("Already have an account? Sign in"))
                    .color(color::ORANGE),
            )
            .style(theme::button::link)
            .on_press_maybe((!loading).then_some(ConnectAccountMessage::LogOut)),
        )
        .align_x(Alignment::Center)
        .spacing(2)
        .max_width(620)
        .width(Length::Fill)
        .into()
}

fn otp_ux<'a>(
    email: &'a str,
    otp: &'a str,
    sending: bool,
    cooldown: u8,
) -> Element<'a, ConnectAccountMessage> {
    let valid = otp.len() == 6 && otp.bytes().all(|c| c.is_ascii_digit());

    let submit: Element<ConnectAccountMessage> = if sending {
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
            .on_press_maybe(valid.then_some(ConnectAccountMessage::VerifyOtp))
            .width(Length::Fill)
            .into()
    };

    let sent_to = format!("An authentication code was sent to your email: {email}");
    let resend_label = if cooldown > 0 {
        format!("Resend token ({cooldown}s)")
    } else {
        "Resend token".to_string()
    };
    let can_resend = cooldown == 0 && !sending;

    Column::new()
        .push(text::h3(localized_copy("Enter your token")).style(theme::text::primary))
        .push(text::p2_regular(sent_to).style(theme::text::secondary))
        .push(iced::widget::Space::new().height(Length::Fixed(25.0)))
        .push(text::p2_medium("Token").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        .push(
            TextInput::new(localized_copy("6-digit code"), otp)
                .id(OTP_INPUT_ID)
                .on_input(ConnectAccountMessage::OtpChanged)
                .on_submit_maybe((!sending && valid).then_some(ConnectAccountMessage::VerifyOtp))
                .size(16)
                .padding(15),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(submit)
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(
            iced::widget::button(
                container(text::p1_regular(resend_label).color(color::GREY_2))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .padding(5),
            )
            .width(Length::Fill)
            .height(Length::Fixed(44.0))
            .style(theme::button::secondary)
            .on_press_maybe(can_resend.then_some(ConnectAccountMessage::ResendOtp)),
        )
        .push(
            iced::widget::button(
                text::p2_regular(localized_copy("Go back")).style(theme::text::secondary),
            )
            .style(theme::button::link)
            .on_press_maybe((!sending).then_some(ConnectAccountMessage::BackToEmail)),
        )
        .align_x(Alignment::Center)
        .spacing(2)
        .max_width(620)
        .width(Length::Fill)
        .into()
}

fn overview_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    sheet_account_ux(state)
}

/// Source-backed Recovery services table. R03 can call this from its own
/// Connect section after it owns launcher navigation; each row's typed
/// `RecoveryServiceTarget` carries only the server Cube ID and network. R10
/// supplies the authenticated Backup & protection destination.
pub fn recovery_services_ux<'a>(
    state: &'a ConnectAccountPanel,
) -> Element<'a, ConnectAccountMessage> {
    // Plan-wide: Connect does not report per-Cube alert setup yet.
    let eligibility = state.recovery_alert_eligibility();
    let alerts_status = move || match eligibility {
        RecoveryAlertEligibility::Included => status_line(
            check_circle_icon(),
            localized_copy("Included in plan"),
            Tone::Good,
        ),
        RecoveryAlertEligibility::NotIncluded => status_line(
            dash_circle_icon(),
            localized_copy("Not in this plan"),
            Tone::Muted,
        ),
        RecoveryAlertEligibility::Unknown => status_line(
            question_circle_icon(),
            localized_copy("Unknown"),
            Tone::Muted,
        ),
    };
    let mut col = Column::new()
        .push(sheet_title(localized_copy("Recovery services")))
        .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
        .push(lead(localized_copy("Recovery Kit and recovery alerts per Cube, as reported by Connect. Open a Cube from the launcher to manage Backup & protection.")))
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .width(Length::Fill);

    match &state.recovery_services {
        RecoveryServicesState::NotLoaded => {
            col = col
                .push(lead(localized_copy("Cube status has not been loaded.")))
                .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                .push(
                    sheet_button("Load Cube status", ActionKind::Secondary, false)
                        .on_press(ConnectAccountMessage::RefreshRecoveryServices),
                );
        }
        RecoveryServicesState::Loading => {
            col = col.push(lead(localized_copy("Loading Cube status…")));
        }
        RecoveryServicesState::Failed(error) => {
            col = col.push(notification::error_card(
                error.title.clone(),
                error.guidance.clone(),
                error.reference.clone(),
                error.retry_action(ConnectAccountMessage::RefreshRecoveryServices),
            ));
        }
        RecoveryServicesState::Ready(rows) if rows.is_empty() => {
            col = col.push(lead(localized_copy(
                "No Cubes are registered with Connect yet.",
            )));
        }
        RecoveryServicesState::Ready(rows) => {
            let cell = |content: Element<'a, ConnectAccountMessage>| {
                container(content).width(Length::FillPortion(1))
            };
            let header = Row::new()
                .push(cell(tenshu_text::caption("Cube").into()))
                .push(cell(tenshu_text::caption(localized_copy("Network")).into()))
                .push(cell(
                    tenshu_text::caption(localized_copy("Recovery Kit")).into(),
                ))
                .push(cell(
                    tenshu_text::caption(localized_copy("Recovery alerts")).into(),
                ))
                .spacing(12);
            col = col
                .push(container(header).padding(iced::Padding {
                    bottom: 8.0,
                    ..iced::Padding::ZERO
                }))
                .push(horizontal_rule());
            for row in rows {
                let (kit_icon, kit, kit_tone) = match row.kit {
                    RecoveryKitReport::Reported => (check_circle_icon(), "Reported", Tone::Good),
                    RecoveryKitReport::NotReported => {
                        (question_circle_icon(), "Not reported", Tone::Muted)
                    }
                };
                col = col
                    .push(
                        container(
                            Row::new()
                                .push(cell(
                                    tenshu_text::label(crate::cube_display::name(&row.name)).into(),
                                ))
                                .push(cell(
                                    tenshu_text::body(network_label(&row.target.network)).into(),
                                ))
                                .push(cell(status_line(kit_icon, kit, kit_tone)))
                                .push(cell(alerts_status()))
                                .spacing(12)
                                .align_y(Alignment::Center),
                        )
                        .padding([12, 0]),
                    )
                    .push(horizontal_rule());
            }
            col = col
                .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                .push(footnote("A reported Kit record does not confirm this device is verified or that every secret is backed up."));
        }
    }

    col.into()
}

/// Connect's network id as the artboard names it.
fn network_label(network: &str) -> String {
    match network.trim() {
        "" => "Unknown network".to_string(),
        "bitcoin" | "mainnet" => "Bitcoin mainnet".to_string(),
        "testnet" => "Bitcoin testnet".to_string(),
        "testnet4" => "Bitcoin Testnet4".to_string(),
        "signet" => "Bitcoin signet".to_string(),
        "regtest" => "Bitcoin regtest".to_string(),
        other => other.to_string(),
    }
}

// Plan & billing views live in `billing.rs`.

// ── Plan provenance card (server-driven, v2) ────────────────────────────────

/// Label shown in place of a server provenance label that carries stale
/// campaign copy. Deliberately tier- and duration-neutral: the grant's real
/// expiry is rendered separately from `expires_at`.
const NEUTRAL_PROVENANCE_LABEL: &str = "Promotional access";

/// Pre-launch campaign wording that must not render at launch (#511): no
/// "Beta", no "free forever", no "Founding member", no year-one/year-two
/// promises. Matched case-insensitively, with hyphens treated as spaces.
const STALE_CAMPAIGN_PHRASES: &[&str] = &[
    "founding member",
    "free forever",
    "first year",
    "year one",
    "second year",
    "year two",
];

fn contains_stale_campaign_copy(copy: &str) -> bool {
    let normalized = copy.to_lowercase().replace('-', " ");
    STALE_CAMPAIGN_PHRASES
        .iter()
        .any(|phrase| normalized.contains(phrase))
        || normalized
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| word == "beta")
}

/// Launch-safe `(label, badge)` for the current plan's `plan_provenance`.
///
/// The label and badge stay server-authored (the campaign engine owns grant
/// semantics, so the desktop cannot know whether a grant is the 60-day launch
/// offer, a year-one grant or an admin grant). Only stale campaign wording is
/// filtered: a stale label becomes [`NEUTRAL_PROVENANCE_LABEL`] and a stale
/// or empty badge is dropped. Returns `None` for purchased/free plans (no
/// provenance), which keeps the existing paid/free UX.
fn plan_provenance_display(plan: &ConnectPlan) -> Option<(&str, Option<&str>)> {
    let prov = plan.plan_provenance.as_ref()?;
    let label = prov.label.trim();
    let label = if label.is_empty() || contains_stale_campaign_copy(label) {
        NEUTRAL_PROVENANCE_LABEL
    } else {
        label
    };
    let badge = prov
        .badge
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty() && !contains_stale_campaign_copy(b));
    Some((label, badge))
}

/// Launch-safe `(label, availability)` for one server-authored plan
/// bullet, or `None` to hide it.
///
/// - Duress is unannounced: any bullet naming it is dropped.
/// - Recovery alerts are shipped on every tier (billing v2 decision 10), so
///   recovery- and duress-alert bullets read as included "Recovery alerts".
/// - Connect-enforced spending policies are not sold yet (decision 9, I9):
///   any policy bullet becomes the Coming soon row.
/// - A bullet the server marks "(coming soon)" is Coming soon.
/// - The per-network Cube count is its own "Cubes per network" row, built
///   from the entitlement, so a "N cubes per network" bullet is dropped.
fn launch_feature_copy(feature: &str) -> Option<(String, billing::FeatureValue)> {
    use billing::FeatureValue;
    let trimmed = feature.trim();
    let key = trimmed.to_ascii_lowercase();
    match key.as_str() {
        "recovery alert" | "recovery alerts" | "duress alert" | "duress alerts" => {
            return Some(("Recovery alerts".to_string(), FeatureValue::Included));
        }
        _ => {}
    }
    if key.contains("duress") || (key.contains("cube") && key.contains("per network")) {
        return None;
    }
    if key.contains("polic") {
        return Some((
            billing::SPENDING_POLICIES.to_string(),
            FeatureValue::ComingSoon,
        ));
    }
    for marker in ["(coming soon)", "— coming soon", "- coming soon"] {
        if let Some(stripped) = key.strip_suffix(marker) {
            let name = trimmed[..stripped.trim_end().len()].to_string();
            return Some((name, FeatureValue::ComingSoon));
        }
    }
    Some((trimmed.to_string(), FeatureValue::Included))
}

// ── Billing history view ────────────────────────────────────────────────────

fn billing_history_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let back_button = iced::widget::button(
        Row::new()
            .push(previous_icon().style(theme::text::primary))
            .push(iced::widget::Space::new().width(Length::Fixed(5.0)))
            .push(text::p1_medium(localized_copy("Back")).style(theme::text::secondary))
            .spacing(5)
            .align_y(Alignment::Center),
    )
    .style(theme::button::transparent)
    // No horizontal padding, so the arrow lines up with the heading below.
    .padding([5, 0])
    .on_press(ConnectAccountMessage::ToggleBillingHistory);

    let mut col = Column::new()
        .push(back_button)
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(sheet_title(localized_copy("Billing history")))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)));

    match &state.billing_history {
        None => {
            col = col.push(text::p1_regular(localized_copy("Loading…")).color(color::GREY_3));
        }
        Some(entries) if entries.is_empty() => {
            col = col.push(
                text::p1_regular(localized_copy("No billing history yet.")).color(color::GREY_3),
            );
        }
        Some(entries) => {
            for entry in entries {
                let status_color = match entry.status {
                    crate::services::coincube::ChargeStatus::Paid => color::ORANGE,
                    crate::services::coincube::ChargeStatus::Expired => color::RED,
                    _ => color::GREY_3,
                };
                let status_label = match entry.status {
                    crate::services::coincube::ChargeStatus::Unpaid => "Unpaid",
                    crate::services::coincube::ChargeStatus::Processing => "Processing",
                    crate::services::coincube::ChargeStatus::Paid => "Paid",
                    crate::services::coincube::ChargeStatus::Expired => "Expired",
                    crate::services::coincube::ChargeStatus::Unknown => "Pending",
                };
                let amount_label = format!(
                    "{} {} ({} sats)",
                    entry.amount_fiat, entry.fiat_currency, entry.amount_sats
                );
                let date = entry
                    .paid_at
                    .as_deref()
                    .unwrap_or(entry.created_at.as_str());
                // Truncate ISO date to just the date portion
                let date_short = if date.len() >= 10 { &date[..10] } else { date };

                col = col.push(
                    container(
                        Row::new()
                            .push(
                                Column::new()
                                    .push(
                                        text::p2_bold(format!(
                                            "{} ({})",
                                            entry.plan, entry.billing_cycle
                                        ))
                                        .style(theme::text::primary),
                                    )
                                    .push(text::p2_regular(amount_label).color(color::GREY_3))
                                    .width(Length::Fill),
                            )
                            .push(
                                Column::new()
                                    .push(text::p2_regular(date_short).color(color::GREY_3))
                                    .push(text::p2_bold(status_label).color(status_color))
                                    .align_x(Alignment::End),
                            )
                            .align_y(Alignment::Center)
                            .padding(12),
                    )
                    .style(|t| container::Style {
                        background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                        border: iced::Border {
                            color: t.colors.cards.simple.border.unwrap_or(color::GREY_5),
                            width: 0.2,
                            radius: 12.0.into(),
                        },
                        ..Default::default()
                    })
                    .width(Length::Fill),
                );
                col = col.push(iced::widget::Space::new().height(Length::Fixed(6.0)));
            }
        }
    }

    col.spacing(0).width(Length::Fill).into()
}

fn security_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let devices_section: Element<ConnectAccountMessage> = match &state.verified_devices {
        None => lead(localized_copy("Loading devices…")).into(),
        Some(devices) if devices.is_empty() => {
            lead(localized_copy("No verified devices on record.")).into()
        }
        Some(devices) => {
            let mut col = Column::new().push(horizontal_rule());

            for d in devices {
                let name = non_blank(d.device_name.as_deref()).unwrap_or("Unknown Device");
                let time_label = if let Some(ref last_used) = d.last_used_at {
                    format!("Last activity {}", format_datetime(last_used))
                } else {
                    format!("Added {}", format_datetime(&d.created_at))
                };

                let mut title = Row::new()
                    .push(tenshu_text::label(name))
                    .spacing(8)
                    .align_y(Alignment::Center);
                if d.is_current {
                    title = title.push(row_tag("Current", true));
                }
                let mut row = Row::new()
                    .push(laptop_icon().size(20).style(theme::tenshu::text::muted))
                    .push(
                        Column::new()
                            .push(title)
                            .push(lead(time_label))
                            .width(Length::Fill),
                    )
                    .spacing(16)
                    .align_y(Alignment::Center);

                if !d.is_current {
                    let is_deleting = state.verified_devices_state.deleting_ids.contains(&d.id);
                    let mut remove = sheet_button(
                        if is_deleting { "Removing…" } else { "Remove" },
                        ActionKind::Secondary,
                        true,
                    );
                    if !is_deleting {
                        remove = remove.on_press(
                            ConnectAccountMessage::ReviewVerifiedDeviceRemoval(Some(d.id)),
                        );
                    }
                    row = row.push(remove);
                }

                col = col
                    .push(container(row).padding([10, 0]).width(Length::Fill))
                    .push(horizontal_rule());
                if state.review_device_removal == Some(d.id) {
                    col = col.push(
                        container(
                            Column::new()
                                .push(group_title(localized_format!(
                                    "Remove {name}?",
                                    "¿Retirar {name}?"
                                )))
                                .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
                                .push(
                                    tenshu_text::body(localized_copy(
                                        "This removes the device from your verified devices.",
                                    ))
                                    .size(13),
                                )
                                .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                                .push(
                                    Row::new()
                                        .push(
                                            sheet_button(
                                                localized_format!(
                                                    "Remove {name}",
                                                    "Eliminar a {name}"
                                                ),
                                                ActionKind::Primary,
                                                false,
                                            )
                                            .on_press(
                                                ConnectAccountMessage::DeleteVerifiedDevice(d.id),
                                            ),
                                        )
                                        .push(
                                            sheet_button("Cancel", ActionKind::Secondary, false)
                                                .on_press(
                                                ConnectAccountMessage::ReviewVerifiedDeviceRemoval(
                                                    None,
                                                ),
                                            ),
                                        )
                                        .spacing(12),
                                ),
                        )
                        .padding([14, 16])
                        .width(Length::Fill)
                        .style(review_style),
                    );
                }
            }

            col.into()
        }
    };

    let mut activity_summary = None;
    let activity_section: Element<ConnectAccountMessage> = match &state.login_activity {
        None => lead(localized_copy("Loading activity…")).into(),
        Some(activity) if activity.is_empty() => {
            lead(localized_copy("No login activity on record.")).into()
        }
        Some(activity) => {
            let shown = &activity[..activity.len().min(50)];
            let succeeded = shown.iter().filter(|a| a.success == Some(true)).count();
            let failed = shown.iter().filter(|a| a.success == Some(false)).count();
            activity_summary = Some(format!("{succeeded} successful · {failed} failed"));
            ruled_list(shown.iter().map(|a| {
                let (icon, status, tone) = match a.success {
                    Some(true) => (check_circle_icon(), "Successful", Tone::Good),
                    Some(false) => (x_circle_icon(), "Failed", Tone::Bad),
                    None => (question_circle_icon(), "Unknown", Tone::Muted),
                };

                let ip = non_blank(a.ip_address.as_deref());
                let ua = non_blank(a.device_name.as_deref());
                let shorten = |u: &str| {
                    if u.chars().count() > 60 {
                        format!("{}…", u.chars().take(59).collect::<String>())
                    } else {
                        u.to_string()
                    }
                };
                let ip_and_ua = match (ip, ua) {
                    (Some(i), Some(u)) => format!("{} - {}", i, shorten(u)),
                    (Some(i), None) => i.to_string(),
                    (None, Some(u)) => shorten(u),
                    (None, None) => "Unknown device".to_string(),
                };

                Row::new()
                    .push(container(status_line(icon, status, tone)).width(Length::Fixed(120.0)))
                    .push(tenshu_text::body(ip_and_ua).width(Length::Fill))
                    .push(lead(format_datetime(&a.created_at)))
                    .spacing(16)
                    .align_y(Alignment::Center)
                    .into()
            }))
        }
    };

    let mut activity_heading = Row::new()
        .push(group_title(localized_copy("Login Activity")).width(Length::Fill))
        .align_y(Alignment::Center);
    if let Some(summary) = activity_summary {
        activity_heading = activity_heading.push(lead(summary));
    }

    Column::new()
        .push(sheet_title(localized_copy("Devices & security")))
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(group_title(localized_copy("Verified Devices")))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(devices_section)
        .push(iced::widget::Space::new().height(Length::Fixed(32.0)))
        .push(activity_heading)
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(activity_section)
        .width(Length::Fill)
        .into()
}

/// Post-login duress verification gate (Phase 6). Shown after auth while
/// `get_duress_state` is in flight, so the dashboard isn't revealed to a
/// possibly-in-duress account. Both terminal failure modes fail CLOSED (no
/// dashboard) but show distinct copy: `Unreachable` reads as a transient
/// connectivity problem, `Incompatible` (a body we couldn't decode) hints at an
/// out-of-date app. Both offer a Sign Out escape hatch so the user is never
/// fully trapped.
fn checking_duress_ux<'a>(status: DuressGateStatus) -> Element<'a, ConnectAccountMessage> {
    let mut col = Column::new().spacing(16).align_x(Alignment::Center);
    match status {
        DuressGateStatus::Checking => {
            col = col.push(
                text::p1_regular(localized_copy("Checking your account…")).color(color::GREY_3),
            );
        }
        DuressGateStatus::Unreachable => {
            col = col
                .push(
                    text::p1_regular(localized_copy("Couldn't verify your account status."))
                        .color(color::GREY_3),
                )
                .push(
                    button::primary(None, localized_copy("Retry"))
                        .width(Length::Fixed(160.0))
                        .on_press(ConnectAccountMessage::RetryDuressCheck),
                )
                .push(
                    button::secondary(None, localized_copy("Sign Out"))
                        .width(Length::Fixed(160.0))
                        .on_press(ConnectAccountMessage::LogOut),
                );
        }
        DuressGateStatus::Incompatible => {
            col = col
                .push(
                    text::p1_regular(localized_copy("We couldn't read your account status."))
                        .color(color::GREY_3),
                )
                .push(
                    text::caption(localized_copy(
                        "Make sure Tenshu is up to date, then try again.",
                    ))
                    .color(color::GREY_3),
                )
                .push(
                    button::primary(None, localized_copy("Retry"))
                        .width(Length::Fixed(160.0))
                        .on_press(ConnectAccountMessage::RetryDuressCheck),
                )
                .push(
                    button::secondary(None, localized_copy("Sign Out"))
                        .width(Length::Fixed(160.0))
                        .on_press(ConnectAccountMessage::LogOut),
                );
        }
    }
    col.width(Length::Fill).into()
}

/// Duress enrollment eligibility gate (Phase 2, Task 2.1).
///
/// Inside the Connect dashboard the user is already signed in, so the tiers
/// reduce to Tier 1 (Connect + CRK) and Tier 2 (Connect, no CRK). The sovereign
/// Tier 3 flow lives behind a separate entry (Phase 8). The interactive
/// multi-step wizard (`duress_enroll.rs`) builds on the credential rules in
/// `services::duress::enroll`; this surface explains eligibility and the
/// credentials the user will set, and surfaces the Tier 2 BIG warning.
fn duress_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    use crate::services::duress::enroll::{DuressDelay, MIN_ALL_CLEAR_LEN};

    // A submission abandoned on another sheet route may still succeed. Keep
    // its payload for local arming without resurfacing the entered secrets.
    if state.duress_enroll_abandoned {
        return Column::new()
            .push(text::h4_bold(localized_copy("Duress Mode")).style(theme::text::primary))
            .push(text::p1_regular(localized_copy("Finishing enrollment…")))
            .into();
    }

    // Wizard takes over the panel while enrollment is in progress.
    if let Some(enroll) = &state.duress_enroll {
        return duress_enroll::enroll_ux(enroll);
    }

    // The step-up re-auth dialog takes over the panel while a disable is in
    // progress (Issue 2).
    if let Some(disable) = &state.duress_disable {
        return duress_enroll::disable_ux(disable);
    }

    // The Emergency-contacts add/edit form takes over the panel too (only
    // reachable from the Estate-entitled list, so the guard is belt-and-
    // suspenders against a mid-session downgrade).
    if matches!(state.duress_contacts.step, DuressContactsStep::Form)
        && state.is_duress_alerts_entitled()
    {
        return duress_contacts::form_ux(state);
    }

    let mut col = Column::new()
        .push(text::h4_bold(localized_copy("Duress Mode")).style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)));

    if !state.is_duress_entitled() {
        return col
            .push(
                container(
                    Column::new()
                        .spacing(12)
                        .push(text::p1_regular(localized_copy(
                            "Duress mode is available on Pro and Estate plans. Upgrade your \
                             Connect plan to enable it.",
                        )))
                        .push(
                            button::primary(None, localized_copy("View plans"))
                                .width(Length::Fixed(160.0))
                                .on_press(ConnectAccountMessage::OpenPlanBilling),
                        )
                        .padding(20),
                )
                .style(card_style)
                .width(Length::Fill),
            )
            .width(Length::Fill)
            .into();
    }

    // Entitled but the launch gate is off (server flag off and this account
    // hasn't enrolled): duress is a *hidden* kill-switch, not a greyed one. The
    // nav hides the entry and both the navigation guard and the features-poll
    // backstop redirect away, so this branch only guards a stale route for the
    // instant before the redirect lands — render nothing rather than the
    // upgrade CTA above, which would read wrong to an already-paid account. See
    // `ConnectAccountPanel::show_duress`.
    if !state.duress_gate().on() {
        return Column::new().width(Length::Fill).into();
    }

    // The approved Connect layout uses one continuous panel with ruled
    // sections. Keep the security explanation ahead of every account state.
    col = col.push(
        Column::new()
            .push(
                text::p1_bold(localized_copy("What duress mode does")).style(theme::text::primary),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
            .push(
                text::p2_regular(localized_copy(
                    "Unlocking any one Cube with your duress PIN wipes every Cube on this \
                     device and signals Connect to lock your entire account — this locks all \
                     of your Cubes, not just the one you opened. The device then shows a \
                     dead-end screen until you clear duress from another trusted device.",
                ))
                .width(Length::Fill)
                .style(theme::text::secondary),
            )
            .width(Length::Fill),
    );

    col = col
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(divider_line())
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)));

    // Once duress is enrolled (server-authoritative), show the enabled state
    // instead of the setup flow. `None` (state not yet loaded, or the account
    // genuinely hasn't enrolled) falls through to the setup flow.
    // Account-level state from the server vs. whether THIS device actually
    // armed the duress PIN on its Cubes. They differ: the account can be
    // enrolled (on another device, or after a local persist that failed) while
    // this device wrote no Cube hashes — in which case entering the duress PIN
    // here does nothing. Only `locally_armed` (from `DuressLocalState`) can
    // honestly claim "armed on this device".
    let server_enrolled = state
        .duress_state
        .as_ref()
        .map(|s| s.enrolled)
        .unwrap_or(false);
    // Active duress hides the disable control (the app is on the cryptic /
    // recovery screen then anyway, but guard defensively).
    let duress_active = state
        .duress_state
        .as_ref()
        .map(|s| s.active)
        .unwrap_or(false);
    let locally_armed = state.duress_locally_armed;

    if locally_armed {
        col = col.push(
            Column::new()
                .push(
                    text::p1_bold(localized_copy("Duress mode is enabled"))
                        .style(theme::text::success),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    text::p2_regular(localized_copy(
                        "Your duress PIN is armed on every Cube on this device. Entering it \
                         at any Cube's unlock screen triggers a duress wipe and locks your \
                         account.",
                    ))
                    .width(Length::Fill)
                    .style(theme::text::secondary),
                )
                .spacing(2)
                .width(Length::Fill),
        );

        // Enrolled-state Recovery-Kit warning (master decision 6): if a Vault
        // Cube's kit is incomplete — e.g. a Vault created *after* enrollment,
        // whose Wallet Descriptor was never added to the kit — surface the
        // same shape-aware checklist as a warning. Duress stays on; this is a
        // nudge, not a gate (no disable pressure).
        if let Some(warning) = duress_enrolled_kit_warning(state) {
            col = col.push(iced::widget::Space::new().height(Length::Fixed(12.0)));
            col = col.push(warning);
        }

        // Disable control — shown only when enrolled & inactive (this whole
        // branch is unreachable during active duress, which routes to the
        // recovery/cryptic screen, but guard anyway). Styled as a
        // destructive-secondary action with a one-line explainer.
        if !duress_active {
            col = col.push(iced::widget::Space::new().height(Length::Fixed(16.0)));
            col = col.push(
                button::secondary(None, localized_copy("Disable Duress Mode"))
                    .width(Length::Fixed(240.0))
                    .on_press(ConnectAccountMessage::Duress(DuressMessage::DisableStart)),
            );
            col = col.push(iced::widget::Space::new().height(Length::Fixed(6.0)));
            col = col.push(
                text::p2_regular(localized_copy(
                    "Turns off duress on all your devices. You can re-enable it anytime.",
                ))
                .width(Length::Fill)
                .style(theme::text::secondary),
            );
        }
    } else if server_enrolled {
        // Enrolled on the account but not armed on THIS device (set up on
        // another device, or a local persist that didn't complete). Be honest:
        // the duress PIN does nothing here. No setup CTA — re-enrolling an
        // already-enrolled account is rejected server-side, and this device
        // can't arm without the duress PIN it never had.
        col = col.push(
            Column::new()
                .push(
                    text::p1_bold(localized_copy("Duress mode is enabled on your account"))
                        .style(theme::text::primary),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    text::p2_regular(localized_copy(
                        "It was set up on another device, so it is not armed on this one — \
                         entering your duress PIN here will not trigger a wipe. Manage duress \
                         mode from the device where you enabled it.",
                    ))
                    .width(Length::Fill)
                    .style(theme::text::secondary),
                )
                .spacing(2)
                .width(Length::Fill),
        );

        // Duress can still be turned off account-wide from here (the server
        // disable propagates to every device); this one just has no local PIN
        // hashes to clear. Shown only while inactive.
        if !duress_active {
            col = col.push(iced::widget::Space::new().height(Length::Fixed(16.0)));
            col = col.push(
                button::secondary(None, localized_copy("Disable Duress Mode"))
                    .width(Length::Fixed(240.0))
                    .on_press(ConnectAccountMessage::Duress(DuressMessage::DisableStart)),
            );
            col = col.push(iced::widget::Space::new().height(Length::Fixed(6.0)));
            col = col.push(
                text::p2_regular(localized_copy(
                    "Turns off duress on all your devices. You can re-enable it anytime.",
                ))
                .width(Length::Fill)
                .style(theme::text::secondary),
            );
        }
    } else {
        let delays: String = DuressDelay::ALL
            .iter()
            .map(|d| d.label())
            .collect::<Vec<_>>()
            .join(" · ");

        // Credentials the user will set during enrollment.
        col = col.push(
            Column::new()
                .push(text::p1_bold(localized_copy("You'll set")).style(theme::text::primary))
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    text::p2_regular(
                        localized_copy("• A duress PIN — one you don't use to unlock any of your Cubes."),
                    )
                    .width(Length::Fill)
                    .style(theme::text::secondary),
                )
                .push(
                    text::p2_regular(localized_format!(
                        "• An all-clear passphrase — at least {} characters, used to unlock \
                         your account from a trusted device.", "• Una contraseña completa —al menos caracteres de {}— usada para desbloquear su cuenta desde un dispositivo de confianza.",
                        MIN_ALL_CLEAR_LEN
                    ))
                    .width(Length::Fill)
                    .style(theme::text::secondary),
                )
                .push(
                    text::p2_regular(localized_copy("• A duress recovery-kit password — covers all your Cubes."))
                        .width(Length::Fill)
                        .style(theme::text::secondary),
                )
                .push(
                    text::p2_regular(localized_format!("• An unlock delay: {}.", "• Un retraso de desbloqueo: {}.", delays))
                        .width(Length::Fill)
                        .style(theme::text::secondary),
                )
                .spacing(6)
                .width(Length::Fill),
        );

        col = col
            .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
            .push(divider_line())
            .push(iced::widget::Space::new().height(Length::Fixed(20.0)));

        // Per-Cube recovery-kit checklist. A duress wipe is only reversible
        // from Connect for Cubes that have a Recovery Kit, so we list the
        // user's mainnet Cubes with their backup state and nudge them to back
        // up each one first. The list is additive — when it hasn't loaded (or
        // there are no mainnet Cubes) the card still shows its guidance copy.
        col = col.push({
            let mut card_col = Column::new()
                .push(
                    text::p1_bold(localized_copy("Set up a Cube Recovery Kit for each Cube"))
                        .style(theme::text::warning),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    text::p2_regular(localized_copy(
                        "Without a Cube Recovery Kit, a duress wipe is irreversible from Connect. \
                         Seed-only Cubes need their Master Seed Phrase backed up; Cubes with a \
                         Vault need both the Master Seed Phrase and the Vault Wallet Descriptor. \
                         Finish the Recovery Kit for each of your Vault Cubes before enabling \
                         duress mode.",
                    ))
                    .width(Length::Fill)
                    .style(theme::text::secondary),
                );
            if let Some(list) = duress_recovery_kit_list(state) {
                card_col = card_col
                    .push(iced::widget::Space::new().height(Length::Fixed(14.0)))
                    .push(list);
            }
            // Transparency footnote for the has_vault gate's accepted
            // limitation (PLAN-duress-vault-gate master §9): other-device
            // Cubes are shown as *reported to Connect*, and a Vault created on
            // another device that hasn't synced yet can still read as
            // vaultless here. Only surfaced when the checklist actually
            // includes a Cube this device doesn't hold, so single-device users
            // aren't shown an irrelevant caveat.
            let has_other_device_cube = state
                .duress_cubes
                .as_deref()
                .is_some_and(|cubes| cubes.iter().any(|c| !c.local));
            if has_other_device_cube {
                card_col = card_col
                    .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
                    .push(
                        text::caption(localized_copy(
                            "Cubes on your other devices are shown as reported to Connect. A \
                             Vault created on another device that hasn't synced yet may not \
                             appear here — open that device to confirm its Recovery Kit.",
                        ))
                        .width(Length::Fill)
                        .style(theme::text::secondary),
                    );
            }
            card_col.spacing(2).width(Length::Fill)
        });

        col = col.push(iced::widget::Space::new().height(Length::Fixed(16.0)));
        // The hard vault gate (PLAN-duress-vault-gate): enrollment is blocked
        // while any Vault Cube in the checklist has an incomplete (or
        // unverifiable) Recovery Kit — a duress wipe of such a Cube would be
        // irreversible. This is now the single hard requirement: there is no
        // "advanced" bypass and no sovereign path.
        //
        // Fail closed on an unloaded checklist (`None`): until the cube list +
        // kit statuses land we can't confirm every Vault Cube is safe, so the
        // CTA stays disabled rather than opening a fail-open window before the
        // fetch completes (master I7 / Resolved decision 4). A failed fetch
        // also leaves `None`, so it stays blocked. An empty (loaded) list is
        // `Some([])` → not blocked.
        let cubes_loaded = state.duress_cubes.is_some();
        let gate_blocked = duress_tier1_gate_blocked(state.duress_cubes.as_deref());
        if gate_blocked {
            // Disabled CTA — `button::primary` with no `.on_press` renders
            // inert. The state handler re-checks the gate too, so a stale view
            // that somehow fires `StartEnrollment` no-ops (belt-and-suspenders).
            col = col.push(
                button::primary(None, localized_copy("Set up Duress Mode"))
                    .width(Length::Fixed(240.0)),
            );
            col = col.push(iced::widget::Space::new().height(Length::Fixed(6.0)));
            // Distinguish "still verifying" from "known incomplete": the former
            // shouldn't accuse the user of an unfinished kit before we've even
            // checked.
            let gate_copy = if cubes_loaded {
                "Finish the Recovery Kit for your Vault Cubes above to enable Duress Mode."
            } else {
                "Checking your Cubes' Recovery Kits…"
            };
            col = col.push(
                text::p2_regular(gate_copy)
                    .width(Length::Fill)
                    .style(theme::text::secondary),
            );
        } else {
            col = col.push(
                button::primary(None, localized_copy("Set up Duress Mode"))
                    .width(Length::Fixed(240.0))
                    .on_press(ConnectAccountMessage::Duress(
                        DuressMessage::StartEnrollment,
                    )),
            );
        }
    }

    // Emergency contacts (Estate Notifications — PR 1). Rendered below the
    // enrollment surface as its own section; Estate-gated inside `section`.
    col = col.push(iced::widget::Space::new().height(Length::Fixed(28.0)));
    col = col.push(divider_line());
    col = col.push(iced::widget::Space::new().height(Length::Fixed(20.0)));
    col = col.push(duress_contacts::section(state));

    col.width(Length::Fill).into()
}

/// Badge label, colour, and (for incomplete Cubes) a one-line "where to
/// fix it" pointer, derived from a Cube's [`CubeBackupCompleteness`]
/// (PLAN-duress-vault-gate PR 2). The checklist is no longer purely
/// informational — a red/amber row tells the user exactly which half is
/// missing and where to add it.
fn checklist_badge(cube: &DuressCube) -> (&'static str, iced::Color, Option<String>) {
    use CubeBackupCompleteness::*;
    let fix = |cta: &str| {
        Some(localized_format!(
            "Open {} → Settings → Recovery Kit → {}.",
            "Abrir {} → Ajustes → Kit de recuperación → {}.",
            cube.name,
            cta
        ))
    };
    match cube.completeness() {
        Complete => ("Backed up", color::GREEN, None),
        MissingDescriptor => (
            "Kit incomplete — Vault Wallet Descriptor missing",
            color::RED,
            fix("Add Wallet Descriptor"),
        ),
        MissingSeed => (
            "Kit incomplete — Master Seed Phrase missing",
            color::RED,
            fix("Add Master Seed Phrase"),
        ),
        NoKit => ("No recovery kit", color::RED, fix("Create Recovery Kit")),
        // Amber, with a retry affordance rendered by the caller. Vault
        // presence unknown (other-device) reads the same to the user: we
        // couldn't confirm this Cube is safe to wipe.
        Unknown => (
            "Couldn't verify — retry",
            color::WARN_ORANGE,
            Some(if cube.local {
                "We couldn't reach Connect to check this Cube's Recovery Kit.".to_string()
            } else {
                "This Cube is on another device — we couldn't confirm its Recovery Kit here."
                    .to_string()
            }),
        ),
    }
}

/// The per-Cube recovery-kit checklist rendered inside the duress intro's
/// "Set up a Cube Recovery Kit for each Cube" card. `None` until the
/// mainnet cube list has loaded, and when there are no mainnet Cubes — in
/// either case the card shows only its guidance copy. Each row names the
/// Cube, shows a shape-aware backup badge (driven by the completeness
/// function shared with the enrollment gate), and — for incomplete Cubes —
/// a one-line pointer to where the missing half is added. `Unknown` rows
/// carry a retry affordance so a transient status-probe failure can be
/// re-fetched without leaving the tab.
fn duress_recovery_kit_list<'a>(
    state: &'a ConnectAccountPanel,
) -> Option<Element<'a, ConnectAccountMessage>> {
    let cubes = state.duress_cubes.as_deref()?;
    if cubes.is_empty() {
        return None;
    }

    let mut col = Column::new().spacing(10);
    for cube in cubes {
        let (label, badge_color, fix_copy) = checklist_badge(cube);
        let badge = container(text::caption(label).color(badge_color))
            .padding(iced::Padding {
                top: 2.0,
                bottom: 2.0,
                left: 8.0,
                right: 8.0,
            })
            .style(move |_t| container::Style {
                border: iced::Border {
                    color: badge_color,
                    width: 0.5,
                    radius: 6.0.into(),
                },
                ..Default::default()
            });

        let mut row_col = Column::new().spacing(4).push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(text::p2_regular(cube.name.as_str()).style(theme::text::primary))
                .push(iced::widget::Space::new().width(Length::Fill))
                .push(badge),
        );

        // Where-to-fix copy (and a retry link for the amber Unknown state).
        if let Some(copy) = fix_copy {
            let is_unknown = cube.completeness() == CubeBackupCompleteness::Unknown;
            row_col = row_col.push(text::caption(copy).color(color::GREY_3));
            if is_unknown {
                row_col = row_col.push(
                    button::transparent(None, localized_copy("Retry"))
                        .on_press(ConnectAccountMessage::Duress(DuressMessage::ReloadCubes)),
                );
            }
        }

        col = col.push(row_col);
    }
    Some(col.into())
}

/// The enrolled-state Recovery-Kit warning card (master decision 6). When
/// any Vault Cube's kit is incomplete (or unverifiable) while duress is
/// already enabled, this surfaces the shape-aware checklist as a warning —
/// covering the Vault-created-after-enrollment case. `None` when every Cube
/// is complete (nothing to warn about) or the list hasn't loaded. Warning
/// only: it applies no disable pressure, and duress stays on.
fn duress_enrolled_kit_warning<'a>(
    state: &'a ConnectAccountPanel,
) -> Option<Element<'a, ConnectAccountMessage>> {
    let cubes = state.duress_cubes.as_deref()?;
    if !duress_gate_blocked(cubes) {
        return None;
    }
    let list = duress_recovery_kit_list(state)?;
    Some(
        container(
            Column::new()
                .push(
                    text::p1_bold(localized_copy("Finish your Recovery Kits"))
                        .style(theme::text::warning),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    text::p2_regular(localized_copy(
                        "Duress is on, but one of your Vault Cubes isn't fully backed up — a \
                         duress wipe of it would be irreversible from Connect. Add the missing \
                         Recovery-Kit material below. Duress stays enabled.",
                    ))
                    .color(color::GREY_3),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(14.0)))
                .push(list)
                .padding(20)
                .spacing(2),
        )
        .style(card_style)
        .width(Length::Fill)
        .into(),
    )
}

/// A thin full-width horizontal rule used to separate panel sections.
fn divider_line<'a>() -> Element<'a, ConnectAccountMessage> {
    container(iced::widget::Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(theme::tenshu::control::divider)
        .into()
}

pub fn avatar_ux<'a>(state: &'a ConnectCubePanel) -> Element<'a, ConnectCubeMessage> {
    let title = Row::new()
        .push(text::h4_bold("Avatar").style(theme::text::primary))
        .push(iced::widget::Space::new().width(Length::Fill))
        .align_y(Alignment::Center);

    let body: Element<ConnectCubeMessage> = match &state.avatar_step {
        AvatarFlowStep::Idle | AvatarFlowStep::Questionnaire => avatar_questionnaire_ux(state),
        AvatarFlowStep::Generating => container(
            Column::new()
                .push(text::p1_bold(localized_copy("Forging your identity…")).color(color::ORANGE))
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    text::p2_regular(localized_copy(
                        "Generating your sumi-e avatar. This may take up to 30 seconds.",
                    ))
                    .color(color::GREY_3),
                )
                .padding(24)
                .spacing(4),
        )
        .style(card_style)
        .width(Length::Fill)
        .into(),

        AvatarFlowStep::Reveal | AvatarFlowStep::Settings => avatar_settings_ux(state),
    };

    if let Some(err) = state.avatar_error.as_deref() {
        let retry_button: Element<ConnectCubeMessage> = if state.registration_error.is_some() {
            button::primary(None, localized_copy("Retry Connection"))
                .on_press(ConnectCubeMessage::RetryRegistration)
                .into()
        } else {
            button::primary(None, localized_copy("Try Again"))
                .on_press(ConnectCubeMessage::Avatar(AvatarMessage::Retry))
                .into()
        };
        return Column::new()
            .push(title)
            .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
            .push(
                container(
                    Column::new()
                        .push(text::p1_bold(localized_copy("Error")).color(color::RED))
                        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                        .push(
                            text::p2_regular(coincube_ui::locale::notice(err.to_owned()))
                                .color(color::GREY_3),
                        )
                        .push(iced::widget::Space::new().height(Length::Fixed(16.0)))
                        .push(retry_button)
                        .padding(16)
                        .spacing(4),
                )
                .style(card_style)
                .width(Length::Fill),
            )
            .spacing(0)
            .width(Length::Fill)
            .into();
    }

    Column::new()
        .push(title)
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(body)
        .spacing(0)
        .width(Length::Fill)
        .into()
}

fn avatar_trait_label<'a>(label: &'static str) -> iced::widget::Text<'a, theme::Theme> {
    text::p2_regular(label)
        .color(color::GREY_3)
        .width(Length::Fixed(110.0))
}

/// One selectable trait option. Selected options use the orange
/// outline style so they stay visually distinct from the solid-orange
/// "Generate Avatar" CTA below the picker.
fn avatar_trait_option<'a>(
    label: &'static str,
    selected: bool,
) -> iced::widget::Button<'a, ConnectCubeMessage, theme::Theme> {
    if selected {
        button::orange_outline(None, label)
    } else {
        button::secondary(None, label)
    }
    .width(Length::Fill)
}

/// A labelled row of mutually exclusive trait options.
fn avatar_trait_row<'a, V: PartialEq + Copy + 'a>(
    label: &'static str,
    current: V,
    options: &[(&'static str, V)],
    to_msg: fn(V) -> AvatarMessage,
) -> Row<'a, ConnectCubeMessage> {
    options
        .iter()
        .enumerate()
        .fold(
            Row::new().push(avatar_trait_label(label)),
            |row, (i, (name, value))| {
                let row = if i > 0 {
                    row.push(iced::widget::Space::new().width(Length::Fixed(8.0)))
                } else {
                    row
                };
                row.push(
                    avatar_trait_option(name, *value == current)
                        .on_press(ConnectCubeMessage::Avatar(to_msg(*value))),
                )
            },
        )
        .align_y(Alignment::Center)
}

fn avatar_questionnaire_ux<'a>(state: &'a ConnectCubePanel) -> Element<'a, ConnectCubeMessage> {
    let draft = &state.avatar_draft;

    let gender_row = avatar_trait_row(
        "Gender",
        draft.gender,
        &[("Man", AvatarGender::Man), ("Woman", AvatarGender::Woman)],
        AvatarMessage::GenderChanged,
    );

    let archetype_row = avatar_trait_row(
        "Archetype",
        draft.archetype,
        &[
            ("Ronin", AvatarArchetype::Ronin),
            ("Samurai", AvatarArchetype::Samurai),
            ("Shogun", AvatarArchetype::Shogun),
        ],
        AvatarMessage::ArchetypeChanged,
    );

    let age_row = avatar_trait_row(
        "Age Feel",
        draft.age_feel,
        &[
            ("Young", AvatarAgeFeel::Young),
            ("Mature", AvatarAgeFeel::Mature),
            ("Elder", AvatarAgeFeel::Elder),
        ],
        AvatarMessage::AgeFeelChanged,
    );

    let demeanor_row = avatar_trait_row(
        "Demeanor",
        draft.demeanor,
        &[
            ("Calm", AvatarDemeanor::Calm),
            ("Fierce", AvatarDemeanor::Fierce),
            ("Mysterious", AvatarDemeanor::Mysterious),
        ],
        AvatarMessage::DemeanorChanged,
    );

    let armor_row = avatar_trait_row(
        "Armor",
        draft.armor_style,
        &[
            ("Light", AvatarArmorStyle::Light),
            ("Standard", AvatarArmorStyle::Standard),
            ("Heavy", AvatarArmorStyle::Heavy),
        ],
        AvatarMessage::ArmorStyleChanged,
    );

    let motif_row = avatar_trait_row(
        "Accent",
        draft.accent_motif,
        &[
            ("Sun", AvatarAccentMotif::OrangeSun),
            ("Splatter", AvatarAccentMotif::Splatter),
            ("Seal", AvatarAccentMotif::Seal),
            ("Calligraphy", AvatarAccentMotif::Calligraphy),
        ],
        AvatarMessage::AccentMotifChanged,
    );

    // Laser eyes is a single toggle, so it reads as "selected" when on.
    let laser_row = Row::new()
        .push(avatar_trait_label("Laser Eyes"))
        .push(
            avatar_trait_option(
                if draft.laser_eyes { "On" } else { "Off" },
                draft.laser_eyes,
            )
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::LaserEyesToggled(
                !draft.laser_eyes,
            ))),
        )
        .align_y(Alignment::Center);

    let has_ln = state.lightning_address.is_some();
    let generate_btn = if has_ln {
        button::primary(None, localized_copy("Generate Avatar"))
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::Generate))
    } else {
        button::primary(None, localized_copy("Set Lightning Address First"))
    };

    container(
        Column::new()
            .push(text::p1_bold(localized_copy("Choose Your Traits")).style(theme::text::primary))
            .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
            .push(text::p2_regular(localized_copy("Your selections, combined with a hash of your Lightning address, create your unique avatar.")).color(color::GREY_3))
            .push(iced::widget::Space::new().height(Length::Fixed(16.0)))
            .push(gender_row)
            .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
            .push(archetype_row)
            .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
            .push(age_row)
            .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
            .push(demeanor_row)
            .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
            .push(armor_row)
            .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
            .push(motif_row)
            .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
            .push(laser_row)
            .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
            .push(generate_btn)
            .padding(16)
            .spacing(0),
    )
    .style(card_style)
    .width(Length::Fill)
    .into()
}

fn avatar_settings_ux<'a>(state: &'a ConnectCubePanel) -> Element<'a, ConnectCubeMessage> {
    let Some(ref data) = state.avatar_data else {
        return container(
            text::p2_regular(localized_copy("Loading avatar data…")).color(color::GREY_3),
        )
        .padding(16)
        .style(card_style)
        .width(Length::Fill)
        .into();
    };

    let active_url = data.active_avatar_url.as_deref().unwrap_or("");

    // Active avatar image
    let active_id = data
        .variants
        .iter()
        .find(|v| active_url.ends_with(&v.id.to_string()))
        .map(|v| v.id);

    let image_widget: Element<ConnectCubeMessage> = if let Some(id) = active_id {
        if let Some((_, handle)) = state.avatar_image_cache.get(&id) {
            iced::widget::image(handle.clone())
                .width(Length::Fixed(200.0))
                .height(Length::Fixed(200.0))
                .into()
        } else {
            text::p2_regular(localized_copy("Loading image…"))
                .color(color::GREY_3)
                .into()
        }
    } else {
        text::p2_regular(localized_copy("No active variant"))
            .color(color::GREY_3)
            .into()
    };

    let archetype_upper: String = data
        .identity
        .as_ref()
        .map(|i| i.archetype.as_str().to_uppercase())
        .unwrap_or_default();

    let regen_remaining = data.regenerations_remaining;
    let regen_text: String = if regen_remaining == -1 {
        "Regenerations: Unlimited".to_string()
    } else {
        format!("Regenerations remaining: {}", regen_remaining)
    };

    // Variant thumbnails row
    let variant_row: Element<ConnectCubeMessage> = if data.variants.len() > 1 {
        let mut row = Row::new().spacing(8);
        for v in &data.variants {
            let is_active = active_url.ends_with(&v.id.to_string());
            let vid = v.id;
            let thumb: Element<ConnectCubeMessage> =
                if let Some((_, handle)) = state.avatar_image_cache.get(&vid) {
                    let img = iced::widget::image(handle.clone())
                        .width(Length::Fixed(60.0))
                        .height(Length::Fixed(60.0));
                    if is_active {
                        container(img)
                            .style(|_| container::Style {
                                border: iced::Border {
                                    color: color::ORANGE,
                                    width: 2.0,
                                    radius: 4.0.into(),
                                },
                                ..Default::default()
                            })
                            .into()
                    } else {
                        iced::widget::button(img)
                            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::SelectVariant(
                                vid,
                            )))
                            .style(|_, _| iced::widget::button::Style::default())
                            .into()
                    }
                } else {
                    iced::widget::button(
                        container(text::p2_regular("…").color(color::GREY_3))
                            .width(Length::Fixed(60.0))
                            .height(Length::Fixed(60.0))
                            .align_x(Alignment::Center)
                            .align_y(Alignment::Center)
                            .style(card_style),
                    )
                    .on_press(ConnectCubeMessage::Avatar(AvatarMessage::SelectVariant(
                        vid,
                    )))
                    .style(|_, _| iced::widget::button::Style::default())
                    .into()
                };
            row = row.push(thumb);
        }
        row.into()
    } else {
        iced::widget::Space::new().height(Length::Fixed(0.0)).into()
    };

    let regen_btn = if regen_remaining == 0 {
        button::primary(None, localized_copy("No Regenerations Remaining"))
    } else {
        button::primary(None, localized_copy("Regenerate Avatar")).on_press(
            ConnectCubeMessage::Avatar(AvatarMessage::SetStep(AvatarFlowStep::Questionnaire)),
        )
    };

    let download_btn = button::secondary(None, localized_copy("Download PNG"))
        .on_press(ConnectCubeMessage::Avatar(AvatarMessage::DownloadAvatar));

    container(
        Column::new()
            .push(
                Row::new()
                    .push(image_widget)
                    .push(iced::widget::Space::new().width(Length::Fixed(16.0)))
                    .push(
                        Column::new()
                            .push(text::p1_bold(archetype_upper).color(color::ORANGE))
                            .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
                            .push(text::p2_regular(regen_text).color(color::GREY_3))
                            .push(iced::widget::Space::new().height(Length::Fixed(16.0)))
                            .push(regen_btn)
                            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                            .push(download_btn)
                            .spacing(0),
                    )
                    .align_y(Alignment::Start),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(16.0)))
            .push(variant_row)
            .padding(16)
            .spacing(0),
    )
    .style(card_style)
    .width(Length::Fill)
    .into()
}

#[cfg(test)]
mod renewal_banner_tests {
    //! Visibility tests for `renewal_banner`. The function returns an
    //! `Option<Element>` and its `style`/`on_press` closures are deferred,
    //! so we can construct it headless and assert only Some/None.
    use super::*;
    use crate::app::state::connect::{CheckoutPhase, PlanLifecycle};
    use crate::services::coincube::{BillingCycle, PlanTier};
    use crate::user_error::UserError;
    use crate::{
        app::state::connect::{
            account::SUPPORTED_PRICING_SCHEMA_VERSION, CheckoutState, DuressDisableState,
        },
        services::coincube::{
            AvatarVariant, BillingHistoryEntry, ChargeStatus, CheckoutResponse, ConnectPlan,
            FeaturesResponse, GetAvatarData, LightningAddress, LoginActivity, PlanEntitlements,
            PlanFeatureInfo, PlanPrice, PlanProvenance, PlanStatus, User, VerifiedDevice,
        },
    };

    /// Collect text from laid-out production widgets, including button labels.
    fn visible_labels<M>(mut element: Element<'_, M>, renderer: &iced::Renderer) -> Vec<String> {
        use iced::advanced::{
            layout,
            widget::{Id, Operation, Tree},
            Layout,
        };

        #[derive(Default)]
        struct Labels(Vec<String>);
        impl Operation for Labels {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }

            fn text(&mut self, _: Option<&Id>, _: iced::Rectangle, value: &str) {
                self.0.push(value.to_owned());
            }
        }

        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            renderer,
            &layout::Limits::new(iced::Size::ZERO, iced::Size::new(760.0, 1600.0)),
        );
        let mut labels = Labels::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), renderer, &mut labels);
        labels.0
    }

    #[tokio::test]
    async fn connect_sheet_fits_compact_window_in_auth_and_account_sections() {
        use iced::advanced::{layout, renderer::Headless, widget::Tree};

        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer for Connect sheet");
        let size = iced::Size::new(1000.0, 712.0);
        let mut account = ConnectAccountPanel::new();
        let mut entitled = plan(PlanTier::Pro, PlanStatus::Active, None);
        entitled.entitlements.duress = true;
        account.plan = Some(entitled);
        let mut features = features_purchasing(Some(true));
        features.duress_enabled = Some(true);
        account.features = Some(features);

        for (name, step, page) in [
            (
                "sign-in",
                ConnectFlowStep::Login {
                    email: String::new(),
                    loading: false,
                },
                ConnectSheetPage::Authentication,
            ),
            (
                "sign-up",
                ConnectFlowStep::Register {
                    email: String::new(),
                    loading: false,
                },
                ConnectSheetPage::Authentication,
            ),
            (
                "token",
                ConnectFlowStep::OtpVerification {
                    email: "founder@example.com".to_string(),
                    otp: String::new(),
                    sending: false,
                    is_signup: false,
                    cooldown: 0,
                },
                ConnectSheetPage::Authentication,
            ),
            (
                "done",
                ConnectFlowStep::CheckingDuress {
                    status: DuressGateStatus::Checking,
                },
                ConnectSheetPage::Authentication,
            ),
            (
                "account-check-failed",
                ConnectFlowStep::CheckingDuress {
                    status: DuressGateStatus::Unreachable,
                },
                ConnectSheetPage::Authentication,
            ),
            (
                "signed-out-account",
                ConnectFlowStep::Login {
                    email: String::new(),
                    loading: false,
                },
                ConnectSheetPage::Sections(ConnectSheetSection::Account),
            ),
            (
                "signed-out-plan-gate",
                ConnectFlowStep::Login {
                    email: String::new(),
                    loading: false,
                },
                ConnectSheetPage::Sections(ConnectSheetSection::PlanBilling),
            ),
            (
                "plan",
                ConnectFlowStep::Dashboard,
                ConnectSheetPage::Sections(ConnectSheetSection::PlanBilling),
            ),
            (
                "security",
                ConnectFlowStep::Dashboard,
                ConnectSheetPage::Sections(ConnectSheetSection::DevicesSecurity),
            ),
            (
                "duress",
                ConnectFlowStep::Dashboard,
                ConnectSheetPage::Sections(ConnectSheetSection::Duress),
            ),
            (
                "recovery",
                ConnectFlowStep::Dashboard,
                ConnectSheetPage::Sections(ConnectSheetSection::RecoveryServices),
            ),
        ] {
            account.step = step;
            if matches!(account.step, ConnectFlowStep::Dashboard) {
                account.user = Some(user());
            }
            let mut element = connect_sheet(&account, page);
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(iced::Size::ZERO, size),
            );
            assert!(node.size().width <= size.width, "{} overflows width", name);
            assert!(
                node.size().height <= size.height,
                "{} overflows height",
                name
            );
            let labels = visible_labels(connect_sheet(&account, page), &renderer);
            let rail: Vec<&str> = labels
                .iter()
                .map(String::as_str)
                .filter(|label| AUTH_STEPS.contains(label))
                .collect();
            if matches!(name, "sign-in" | "sign-up" | "token" | "done") {
                // The rail sits in its band right under the header, ahead of
                // the step's own copy, with its three steps in order.
                let close = labels
                    .iter()
                    .position(|label| label == "Close")
                    .expect("sheet header Close");
                // A completed step shows the rail's check glyph in place of
                // its number.
                const CHECK: &str = "\u{f633}";
                let expected = match name {
                    "token" => [CHECK, "Email", "2", "Token", "3", "Done"],
                    "done" => [CHECK, "Email", CHECK, "Token", "3", "Done"],
                    _ => ["1", "Email", "2", "Token", "3", "Done"],
                };
                assert_eq!(
                    labels.get(close + 1..close + 7).map(|rail| rail.to_vec()),
                    Some(expected.map(String::from).to_vec()),
                    "{} rail: {:?}",
                    name,
                    labels
                );
                assert_eq!(rail[..3], AUTH_STEPS, "{} rail order", name);
            } else {
                assert!(rail.is_empty(), "{} shows the sign-in rail", name);
            }
            if name == "signed-out-account" {
                assert!(labels.iter().any(|label| label == "Create account"));
                assert!(labels.iter().any(|label| label == "Recovery services"));
            }
            if name == "signed-out-plan-gate" {
                assert!(labels.iter().any(|label| label == "Sign in to see this"));
                assert!(labels.iter().any(|label| label == "Account"));
            }
        }
    }

    #[tokio::test]
    async fn connect_sheet_duress_entry_obeys_account_gate() {
        use iced::advanced::renderer::Headless;

        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let mut account = dashboard_panel();
        account.plan.as_mut().unwrap().entitlements.duress = true;
        account.features.as_mut().unwrap().duress_enabled = Some(true);
        let labels = visible_labels(
            connect_sheet(
                &account,
                ConnectSheetPage::Sections(ConnectSheetSection::Duress),
            ),
            &renderer,
        );
        assert!(labels.iter().any(|label| label == "Duress mode"));
        assert!(labels.iter().any(|label| label == "What duress mode does"));

        // A server kill switch or lost entitlement hides both navigation and
        // stale route contents, including an in-flight enrollment screen.
        for (entitled, enabled) in [(true, false), (false, true)] {
            account.plan.as_mut().unwrap().entitlements.duress = entitled;
            account.features.as_mut().unwrap().duress_enabled = Some(enabled);
            let labels = visible_labels(
                connect_sheet(
                    &account,
                    ConnectSheetPage::Sections(ConnectSheetSection::Duress),
                ),
                &renderer,
            );
            assert!(!labels.iter().any(|label| label == "Duress mode"));
            assert!(!labels.iter().any(|label| label == "What duress mode does"));
            assert!(labels
                .iter()
                .any(|label| label == "Signed in on this device"));
        }

        account.step = ConnectFlowStep::CheckingDuress {
            status: DuressGateStatus::Checking,
        };
        let labels = visible_labels(
            connect_sheet(
                &account,
                ConnectSheetPage::Sections(ConnectSheetSection::Duress),
            ),
            &renderer,
        );
        assert!(!labels.iter().any(|label| label == "Duress mode"));
        assert!(!labels
            .iter()
            .any(|label| label == "Signed in on this device"));
    }

    fn plan(tier: PlanTier, status: PlanStatus, renewal_at: Option<&str>) -> ConnectPlan {
        ConnectPlan {
            plan: tier,
            status,
            renewal_at: renewal_at.map(|s| s.to_string()),
            entitlements: PlanEntitlements::default(),
            billing_cycle: Some(BillingCycle::Monthly),
            plan_provenance: None,
            pending_plan: None,
            pending_plan_at: None,
            pending_billing_cycle: None,
        }
    }

    fn provenance_plan(tier: PlanTier, label: &str, badge: Option<&str>) -> ConnectPlan {
        let mut p = plan(tier, PlanStatus::Active, Some("2027-07-04T00:00:00Z"));
        p.plan_provenance = Some(PlanProvenance {
            label: label.to_string(),
            expires_at: Some("2027-07-04T00:00:00Z".to_string()),
            badge: badge.map(|b| b.to_string()),
        });
        p
    }

    /// Regression (#574 P2-1): an Estate grant that is not the 60-day launch
    /// offer (year-one campaign, retro-granted beta cohort, admin grant) must
    /// never be labelled with the launch offer's duration.
    #[test]
    fn non_launch_estate_grant_never_renders_sixty_days() {
        for (label, badge) in [
            ("Complimentary Estate access", Some("Grant")),
            ("Estate granted by support", None),
            ("Free for your first year", Some("Founding member")),
        ] {
            let p = provenance_plan(PlanTier::Estate, label, badge);
            let (shown, shown_badge) = plan_provenance_display(&p).expect("provenance");
            assert!(
                !shown.contains("60 days"),
                "label {:?} for {:?}",
                shown,
                label
            );
            assert!(
                !shown_badge.unwrap_or_default().contains("60 days"),
                "badge {:?} for {:?}",
                shown_badge,
                badge
            );
        }
        // The fixture above an "Expires Jul 4, 2027" line keeps a neutral label.
        let granted = granted_estate(Some("2027-07-04T00:00:00Z"), Some("Founding member"));
        assert_eq!(
            plan_provenance_display(&granted),
            Some((NEUTRAL_PROVENANCE_LABEL, None))
        );
    }

    /// Server labels/badges carrying banned pre-launch campaign copy are
    /// replaced by the neutral fallback label and the badge is dropped.
    #[test]
    fn stale_campaign_provenance_copy_falls_back_to_neutral() {
        for stale in [
            "Founding member",
            "FOUNDING MEMBER perks",
            "Estate free forever",
            "Free for your first year",
            "Free for year-one",
            "Year two pricing locked",
            "Beta cohort",
            "",
            "   ",
        ] {
            let p = provenance_plan(PlanTier::Estate, stale, Some(stale));
            assert_eq!(
                plan_provenance_display(&p),
                Some((NEUTRAL_PROVENANCE_LABEL, None)),
                "stale copy {:?}",
                stale
            );
        }
        // A clean label keeps rendering while only the stale badge is dropped.
        let p = provenance_plan(PlanTier::Pro, "Partner grant", Some("Founding member"));
        assert_eq!(plan_provenance_display(&p), Some(("Partner grant", None)));
        // Word matching: "Alphabetic" is not "Beta".
        assert!(!contains_stale_campaign_copy("Alphabetic betatron"));
    }

    /// Normal server-authored provenance renders unchanged: label and badge.
    #[test]
    fn launch_offer_provenance_renders_server_label_and_badge() {
        let p = provenance_plan(
            PlanTier::Estate,
            "Launch offer — Estate free for 60 days",
            Some("Launch offer"),
        );
        assert_eq!(
            plan_provenance_display(&p),
            Some((
                "Launch offer — Estate free for 60 days",
                Some("Launch offer")
            ))
        );
        let p = provenance_plan(PlanTier::Pro, "Promotional Pro access", None);
        assert_eq!(
            plan_provenance_display(&p),
            Some(("Promotional Pro access", None))
        );
        let paid = plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2027-01-01T00:00:00Z"),
        );
        assert_eq!(plan_provenance_display(&paid), None);
    }

    #[test]
    fn sign_in_benefits_do_not_mention_duress() {
        for (_, benefit, _) in connect_benefit_rows() {
            assert!(
                !benefit.to_ascii_lowercase().contains("duress"),
                "{}",
                benefit
            );
        }
    }

    #[test]
    fn unreleased_plan_features_are_hidden_or_marked_coming_soon() {
        use billing::FeatureValue::{ComingSoon, Included};
        let copy = |f: &str| launch_feature_copy(f);
        assert_eq!(copy("Duress mode"), None);
        assert_eq!(copy("Duress protection"), None);
        assert_eq!(copy("Duress activation alerts"), None);
        assert_eq!(copy("Duress (coming soon)"), None);
        // Recovery alerts ship on every tier (billing v2 decision 10).
        assert_eq!(
            copy("Duress alerts"),
            Some(("Recovery alerts".to_string(), Included))
        );
        assert_eq!(
            copy("Recovery alerts"),
            Some(("Recovery alerts".to_string(), Included))
        );
        // Spending policies are not sold yet (decision 9, I9).
        assert_eq!(
            copy("Attach Connect-enforced policies"),
            Some((billing::SPENDING_POLICIES.to_string(), ComingSoon))
        );
        assert_eq!(
            copy("Price alerts (coming soon)"),
            Some(("Price alerts".to_string(), ComingSoon))
        );
        // The Cube count is its own row, from the entitlement.
        assert_eq!(copy("4 cubes per network"), None);
        assert_eq!(copy("1 cube per network"), None);
        assert_eq!(
            copy("More Cubes"),
            Some(("More Cubes".to_string(), Included))
        );
    }

    /// Regression: dismissing the pre-expiry reminder must NOT suppress the
    /// expired prompt once the plan lapses in the same session.
    #[test]
    fn expired_banner_shows_even_after_reminder_dismissed() {
        let mut panel = ConnectAccountPanel::new();
        panel.renewal_banner_dismissed = true;
        // Free + past_due is the backend's demoted/expired shape.
        panel.plan = Some(plan(
            PlanTier::Free,
            PlanStatus::PastDue,
            Some("2026-06-01T00:00:00Z"),
        ));
        assert!(matches!(panel.plan_lifecycle(), PlanLifecycle::Expired));
        assert!(
            renewal_banner(&panel).is_some(),
            "expired prompt must render regardless of the reminder dismissal"
        );
    }

    /// A free, never-paid account shows no banner.
    #[test]
    fn no_banner_for_plain_free_account() {
        let mut panel = ConnectAccountPanel::new();
        panel.plan = Some(plan(PlanTier::Free, PlanStatus::Active, None));
        assert!(renewal_banner(&panel).is_none());
    }

    // ── Estate promo: server-driven provenance + purchasing (v2) ──────
    fn granted_estate(renewal_at: Option<&str>, badge: Option<&str>) -> ConnectPlan {
        let mut p = plan(PlanTier::Estate, PlanStatus::Active, renewal_at);
        p.plan_provenance = Some(PlanProvenance {
            label: "Free for your first year".to_string(),
            expires_at: renewal_at.map(|s| s.to_string()),
            badge: badge.map(|s| s.to_string()),
        });
        p
    }

    fn features_purchasing(enabled: Option<bool>) -> crate::services::coincube::FeaturesResponse {
        crate::services::coincube::FeaturesResponse {
            plans: vec![crate::services::coincube::PlanFeatureInfo {
                name: "pro".to_string(),
                price: None,
                features: Vec::new(),
                included_linked_participants: None,
                entitlements: None,
            }],
            pricing_schema_version: None,
            purchasing_enabled: enabled,
            marketplace_enabled: None,
            liquid_enabled: None,
            buy_sell_enabled: None,
            p2p_enabled: None,
            duress_enabled: None,
            bitcoin_blake2b_enabled: None,
        }
    }

    fn user() -> User {
        User {
            id: 7,
            email: "founder@example.com".to_string(),
            email_verified: Some(true),
        }
    }

    fn priced_features(
        schema_version: Option<u32>,
        purchasing_enabled: Option<bool>,
    ) -> FeaturesResponse {
        FeaturesResponse {
            plans: vec![
                PlanFeatureInfo {
                    name: "free".to_string(),
                    price: None,
                    features: vec!["One Cube".to_string()],
                    included_linked_participants: None,
                    entitlements: None,
                },
                PlanFeatureInfo {
                    name: "pro".to_string(),
                    price: Some(PlanPrice {
                        monthly: 12,
                        annual: 120,
                    }),
                    features: vec!["More Cubes".to_string(), "Duress mode".to_string()],
                    included_linked_participants: None,
                    entitlements: None,
                },
                PlanFeatureInfo {
                    name: "estate".to_string(),
                    price: Some(PlanPrice {
                        monthly: 24,
                        annual: 240,
                    }),
                    features: vec!["Recovery alerts".to_string()],
                    included_linked_participants: None,
                    entitlements: None,
                },
            ],
            pricing_schema_version: schema_version,
            purchasing_enabled,
            marketplace_enabled: Some(true),
            liquid_enabled: Some(true),
            buy_sell_enabled: Some(true),
            p2p_enabled: Some(false),
            duress_enabled: Some(true),
            bitcoin_blake2b_enabled: None,
        }
    }

    fn checkout_response() -> CheckoutResponse {
        CheckoutResponse {
            charge_id: "charge-1".to_string(),
            lightning_invoice: "lnbc1abcdefghijklmnopqrstuvwxyz0123456789extra".to_string(),
            on_chain_address: "bc1qtestaddress000000000000000000000000000000".to_string(),
            amount_sats: 50_000,
            amount_fiat: 25.0,
            fiat_currency: "USD".to_string(),
            plan: PlanTier::Pro,
            billing_cycle: BillingCycle::Annual,
            checkout_url: "https://checkout.example/charge-1".to_string(),
            expires_at: Some("2026-08-01T00:00:00Z".to_string()),
            starts_at: None,
        }
    }

    fn billing_entry(status: ChargeStatus) -> BillingHistoryEntry {
        BillingHistoryEntry {
            charge_id: format!("charge-{status:?}"),
            plan: PlanTier::Pro,
            billing_cycle: BillingCycle::Monthly,
            amount_sats: 42_000,
            amount_fiat: 21.0,
            fiat_currency: "USD".to_string(),
            status,
            created_at: "2026-07-01T12:00:00Z".to_string(),
            paid_at: Some("2026-07-02T12:00:00Z".to_string()),
            state: None,
            invoice_id: None,
            amount_received_sats: 0,
            overpaid_sats: 0,
        }
    }

    fn dashboard_panel() -> ConnectAccountPanel {
        let mut panel = ConnectAccountPanel::new();
        panel.step = ConnectFlowStep::Dashboard;
        panel.user = Some(user());
        panel.plan = Some(plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2027-01-01T00:00:00Z"),
        ));
        panel.features = Some(priced_features(
            Some(SUPPORTED_PRICING_SCHEMA_VERSION),
            Some(true),
        ));
        panel.overview_contact_count = Some(3);
        panel.overview_cube_count = Some(2);
        panel
    }

    /// The provenance card renders only when the API sends `plan_provenance`
    /// — a purchased/free plan (no provenance) shows nothing extra.
    #[test]
    fn provenance_card_present_only_with_provenance() {
        let granted = granted_estate(Some("2027-07-04T00:00:00Z"), Some("Founding member"));
        assert!(billing::access_row(&granted).is_some());

        let paid = plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2027-01-01T00:00:00Z"),
        );
        assert!(billing::access_row(&paid).is_none());
    }

    /// A granted account still gets a renewal banner gated only by the
    /// generic purchasing signal — promo provenance no longer suppresses it
    /// by itself (v2 removes campaign conditionals). With purchasing closed
    /// (the campaign window), it's suppressed.
    #[test]
    fn granted_account_banner_follows_purchasing_signal() {
        let mut panel = ConnectAccountPanel::new();
        // Renewal in the past so `plan_lifecycle()` is RenewalDue.
        panel.plan = Some(granted_estate(
            Some("2020-01-01T00:00:00Z"),
            Some("Founding member"),
        ));
        assert!(matches!(
            panel.plan_lifecycle(),
            PlanLifecycle::RenewalDue { .. }
        ));
        // Purchasing closed → suppressed (no dead-end CTA).
        panel.features = Some(features_purchasing(Some(false)));
        assert!(renewal_banner(&panel).is_none());
        // Purchasing open → the generic reminder shows.
        panel.features = Some(features_purchasing(Some(true)));
        assert!(renewal_banner(&panel).is_some());
    }

    /// PR2: with purchasing disabled, the pre-expiry "Renew" banner is hidden
    /// for an ordinary paid account too — no dead-end CTA.
    #[test]
    fn no_renewal_banner_when_purchasing_disabled() {
        let mut panel = ConnectAccountPanel::new();
        panel.plan = Some(plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2020-01-01T00:00:00Z"),
        ));
        panel.features = Some(features_purchasing(Some(false)));
        assert!(matches!(
            panel.plan_lifecycle(),
            PlanLifecycle::RenewalDue { .. }
        ));
        assert!(renewal_banner(&panel).is_none());
    }

    /// Smoke: the manage-plan view (with a provenance card + redeem field),
    /// the purchasing-disabled picker, and the account-creation screen all
    /// construct without panicking. Element internals are opaque, so this
    /// just exercises the render paths.
    #[test]
    fn provenance_picker_and_register_render() {
        let mut granted = ConnectAccountPanel::new();
        granted.plan = Some(granted_estate(
            Some("2027-07-04T00:00:00Z"),
            Some("Founding member"),
        ));
        granted.features = Some(features_purchasing(Some(false)));
        let _ = plan_billing_ux(&granted);

        let mut paid = ConnectAccountPanel::new();
        paid.plan = Some(plan(PlanTier::Free, PlanStatus::Active, None));
        paid.campaign_redeem.result = Some(Err("This code has expired.".into()));
        let _ = plan_billing_ux(&paid);

        let _ = register_ux("founder@example.com", "FOUNDER", false);
    }

    #[test]
    fn formatting_and_color_helpers_are_stable() {
        assert_eq!(format_date("2026-04-20T14:31:00Z"), "Apr 20, 2026");
        assert_eq!(format_date("not-a-date"), "not-a-date");
        assert_eq!(format_datetime("not-a-date"), "not-a-date");
        assert_eq!(billing::format_usd(100.0), "100");
        assert_eq!(billing::format_usd(33.15), "33.15");
        assert_eq!(billing::group_sats(41_000), "41,000");
        assert_eq!(billing::group_sats(999), "999");
        assert_eq!(billing::group_sats(1_234_567), "1,234,567");
    }

    #[test]
    fn auth_gate_and_error_views_build_for_each_step() {
        let mut account = ConnectAccountPanel::new();
        account.error = Some(PanelError::retryable(
            UserError::new(
                "Can't reach COINCUBE | Connect",
                crate::user_error::RETRY_GUIDANCE,
                crate::user_error::CC_NET_TIMEOUT,
                true,
            ),
            crate::app::view::RetryAction::Session,
        ));

        let unlock_at = chrono::DateTime::parse_from_rfc3339("2026-07-20T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        // Each step with the rail position it shows: Email, Token, Done
        // while the account check runs, and no rail once it fails.
        for (step, expected) in [
            (ConnectFlowStep::CheckingSession, None),
            (
                ConnectFlowStep::Login {
                    email: "bad".to_string(),
                    loading: false,
                },
                Some(0),
            ),
            (
                ConnectFlowStep::Login {
                    email: "founder@example.com".to_string(),
                    loading: true,
                },
                Some(0),
            ),
            (
                ConnectFlowStep::Register {
                    email: "new@example.com".to_string(),
                    loading: false,
                },
                Some(0),
            ),
            (
                ConnectFlowStep::OtpVerification {
                    email: "founder@example.com".to_string(),
                    otp: "123456".to_string(),
                    sending: false,
                    is_signup: false,
                    cooldown: 5,
                },
                Some(1),
            ),
            (
                ConnectFlowStep::CheckingDuress {
                    status: DuressGateStatus::Checking,
                },
                Some(2),
            ),
            (
                ConnectFlowStep::CheckingDuress {
                    status: DuressGateStatus::Unreachable,
                },
                None,
            ),
            (
                ConnectFlowStep::CheckingDuress {
                    status: DuressGateStatus::Incompatible,
                },
                None,
            ),
            (
                ConnectFlowStep::DuressRecovery {
                    unlock_at: Some(unlock_at),
                    passphrase: "all-clear".to_string(),
                    submitting: false,
                    cleared: false,
                },
                None,
            ),
            (
                ConnectFlowStep::DuressRecovery {
                    unlock_at: None,
                    passphrase: String::new(),
                    submitting: true,
                    cleared: true,
                },
                None,
            ),
            (ConnectFlowStep::Dashboard, None),
        ] {
            assert_eq!(auth_step(&step), expected, "{:?}", step);
            account.step = step;
            let _ = connect_account_panel(&account);
        }
    }

    #[test]
    fn dashboard_account_tabs_build_loaded_empty_and_processing_states() {
        let mut account = dashboard_panel();

        for sub in [
            ConnectSubMenu::Overview,
            ConnectSubMenu::PlanBilling,
            ConnectSubMenu::Security,
            ConnectSubMenu::Contacts,
            ConnectSubMenu::Duress,
        ] {
            account.active_sub = sub;
            let _ = connect_account_panel(&account);
        }

        account.active_sub = ConnectSubMenu::Security;
        account.verified_devices = Some(vec![
            VerifiedDevice {
                id: 1,
                device_name: Some("Laptop".to_string()),
                created_at: "2026-07-01T00:00:00Z".to_string(),
                last_used_at: Some("2026-07-02T00:00:00Z".to_string()),
                is_current: true,
            },
            VerifiedDevice {
                id: 2,
                device_name: None,
                created_at: "2026-06-01T00:00:00Z".to_string(),
                last_used_at: None,
                is_current: false,
            },
            // Rows written before device naming was fixed come back with an
            // empty (not absent) name.
            VerifiedDevice {
                id: 3,
                device_name: Some(String::new()),
                created_at: "2026-05-01T00:00:00Z".to_string(),
                last_used_at: Some("2026-05-02T00:00:00Z".to_string()),
                is_current: false,
            },
        ]);
        account.verified_devices_state.deleting_ids.insert(2);
        account.login_activity = Some(vec![
            LoginActivity {
                id: 1,
                ip_address: Some("127.0.0.1".to_string()),
                device_name: Some(
                    "Browser with a very long user agent string that should truncate in the view"
                        .to_string(),
                ),
                created_at: "2026-07-03T00:00:00Z".to_string(),
                success: Some(true),
            },
            LoginActivity {
                id: 2,
                ip_address: None,
                device_name: None,
                created_at: "2026-07-04T00:00:00Z".to_string(),
                success: Some(false),
            },
            LoginActivity {
                id: 3,
                ip_address: Some("104.28.154.80".to_string()),
                device_name: Some(String::new()),
                created_at: "2026-05-02T00:00:00Z".to_string(),
                success: Some(false),
            },
        ]);
        let _ = connect_account_panel(&account);

        account.verified_devices = Some(Vec::new());
        account.login_activity = Some(Vec::new());
        let _ = connect_account_panel(&account);
    }

    /// The API returns an empty `deviceName` for device rows written before
    /// device naming was fixed. `Some("")` must be treated as "no name" so the
    /// caller's fallback fires — otherwise the Security page renders a nameless
    /// device row, and a login-activity line that trails off as `"1.2.3.4 - "`.
    #[test]
    fn blank_device_names_are_treated_as_absent() {
        assert_eq!(super::non_blank(Some("MacBook Pro")), Some("MacBook Pro"));
        assert_eq!(super::non_blank(Some("  Dart  ")), Some("Dart"));

        assert_eq!(super::non_blank(None), None);
        assert_eq!(super::non_blank(Some("")), None);
        assert_eq!(super::non_blank(Some("   ")), None);
        assert_eq!(super::non_blank(Some("\t\n")), None);

        // The fallbacks the view applies on top of it.
        assert_eq!(
            super::non_blank(Some("")).unwrap_or("Unknown Device"),
            "Unknown Device"
        );
    }

    #[test]
    fn recovery_services_build_unknown_loading_error_empty_and_reported_states() {
        let mut account = dashboard_panel();
        account.plan = None;
        let _ = recovery_services_ux(&account);
        account.recovery_services = RecoveryServicesState::Loading;
        let _ = recovery_services_ux(&account);
        account.recovery_services =
            RecoveryServicesState::Failed(crate::user_error::UserError::new(
                "Could not load Cube status",
                "Check your connection and try again.",
                "CC-RECOVERY-TEST",
                true,
            ));
        let _ = recovery_services_ux(&account);
        account.recovery_services = RecoveryServicesState::Ready(Vec::new());
        let _ = recovery_services_ux(&account);
        let cube: crate::services::coincube::CubeResponse = serde_json::from_str(
            r#"{"id":7,"uuid":"cube-7","name":"Family","network":"bitcoin","status":"active","hasRecoveryKit":true}"#,
        )
        .unwrap();
        account.recovery_services = RecoveryServicesState::Ready(vec![cube.into()]);
        let _ = recovery_services_ux(&account);
    }

    #[tokio::test]
    async fn in_cube_recovery_rows_guide_to_launcher_without_inert_open_action() {
        use iced::advanced::renderer::Headless;

        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let mut account = dashboard_panel();
        let cube: crate::services::coincube::CubeResponse = serde_json::from_str(
            r#"{"id":7,"uuid":"cube-7","name":"Family","network":"bitcoin","status":"active","hasRecoveryKit":true}"#,
        )
        .unwrap();
        account.recovery_services = RecoveryServicesState::Ready(vec![cube.into()]);
        let labels = visible_labels(recovery_services_ux(&account), &renderer);
        assert!(labels
            .iter()
            .any(|label| label.contains("Open a Cube from the launcher")));
        assert!(!labels.iter().any(|label| label == "Open Cube to manage"));
    }

    #[tokio::test]
    async fn recovery_error_card_shows_reference_and_only_actionable_retry() {
        use iced::advanced::renderer::Headless;

        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let mut account = dashboard_panel();
        for (retryable, reference, guidance) in [
            (
                true,
                "CC-RECOVERY-TEMP",
                "Check your connection and try again.",
            ),
            (
                false,
                "CC-RECOVERY-AUTH",
                "Sign in again or contact support.",
            ),
        ] {
            account.recovery_services = RecoveryServicesState::Failed(UserError::new(
                "Could not load Cube status",
                guidance,
                reference,
                retryable,
            ));
            let labels = visible_labels(recovery_services_ux(&account), &renderer);
            assert!(labels
                .iter()
                .any(|text| text == "Could not load Cube status"));
            assert!(labels.iter().any(|text| text == guidance));
            assert!(labels
                .iter()
                .any(|text| text == &format!("Ref: {reference}")));
            assert_eq!(
                labels.iter().any(|text| text == "Try again"),
                retryable,
                "wrong recovery retry affordance: {:?}",
                labels
            );
        }
    }

    #[tokio::test]
    async fn current_signup_redemption_result_appears_in_default_plan_view() {
        use iced::advanced::renderer::Headless;

        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        for result in [
            Ok("Estate unlocked".to_string()),
            Err("Code expired".to_string()),
        ] {
            let mut account = dashboard_panel();
            account.campaign_redeem.code = "FOUNDER".into();
            assert!(!account.show_campaign_redeem);
            let generation = account.session_generation();
            let _ = account.update_message(ConnectAccountMessage::CampaignRedeemed(
                Ok("old account result".into()),
                generation + 1,
            ));
            let hidden = visible_labels(plan_billing_ux(&account), &renderer);
            assert!(!hidden.iter().any(|text| text == "old account result"));
            assert!(!account.show_campaign_redeem);

            let expected = match &result {
                Ok(message) | Err(message) => message.clone(),
            };
            let _ =
                account.update_message(ConnectAccountMessage::CampaignRedeemed(result, generation));
            let _ = account.update_message(ConnectAccountMessage::OpenPlanBilling);
            let visible = visible_labels(plan_billing_ux(&account), &renderer);
            assert!(
                visible.iter().any(|text| text == &expected),
                "visible labels: {:?}",
                visible
            );
            assert!(!visible.iter().any(|text| text == "old account result"));
        }
    }

    /// The current API's `/connect/features` payload (entitlements per tier,
    /// v2 cube limits), for the billing v2 views.
    fn billing_v2_features() -> FeaturesResponse {
        let ent = |cube_limit, estate: bool, paid: bool| PlanEntitlements {
            cube_limit,
            recovery_alerts: true,
            inheritance_escrow: estate,
            collaborative_invitations: estate,
            attach_policies: paid,
            ..PlanEntitlements::default()
        };
        let info = |name: &str, price: Option<(u32, u32)>, features: &[&str], e| PlanFeatureInfo {
            name: name.to_string(),
            price: price.map(|(monthly, annual)| PlanPrice { monthly, annual }),
            features: features.iter().map(|f| f.to_string()).collect(),
            included_linked_participants: None,
            entitlements: Some(e),
        };
        let mut f = priced_features(Some(SUPPORTED_PRICING_SCHEMA_VERSION), Some(true));
        f.plans = vec![
            info(
                "free",
                None,
                &[
                    "1 cube per network",
                    "Esplora access",
                    "Lightning address & avatar",
                ],
                ent(1, false, false),
            ),
            info(
                "pro",
                Some((10, 100)),
                &[
                    "3 cubes per network",
                    "Attach Connect-enforced policies",
                    "Duress (coming soon)",
                    "Esplora access",
                ],
                ent(3, false, true),
            ),
            info(
                "estate",
                Some((20, 200)),
                &[
                    "10 cubes per network",
                    "Collaborative Multi-sig Keyholder Invitations",
                ],
                ent(10, true, true),
            ),
        ];
        f
    }

    #[test]
    fn account_features_list_cubes_first_and_coming_soon_last() {
        use billing::FeatureValue::{ComingSoon, Included, Muted, Value};
        let mut account = dashboard_panel();
        account.features = Some(billing_v2_features());
        let rows = billing::account_feature_rows(&account);
        let names: Vec<&str> = rows.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "Cubes per network",
                "Esplora access",
                "Recovery alerts",
                billing::SPENDING_POLICIES,
            ]
        );
        assert_eq!(rows[0].1, Value("Up to 3".to_string()));
        assert_eq!(rows.last().unwrap().1, ComingSoon);
        assert!(!names.iter().any(|n| n.to_lowercase().contains("duress")));

        account.plan = Some(plan(PlanTier::Estate, PlanStatus::Active, None));
        let rows = billing::account_feature_rows(&account);
        assert_eq!(rows[0].1, Value("Up to 10".to_string()));
        assert!(rows.contains(&("Inheritance escrow".to_string(), Included)));
        assert!(rows.contains(&("Recovery alerts".to_string(), Included)));
        assert_eq!(rows.last().unwrap().0, billing::SPENDING_POLICIES);

        // Free: one Cube, recovery alerts included, no Coming soon policies.
        account.plan = Some(plan(PlanTier::Free, PlanStatus::Active, None));
        let rows = billing::account_feature_rows(&account);
        assert_eq!(rows[0].1, Muted("1 on Free".to_string()));
        assert!(rows.contains(&("Recovery alerts".to_string(), Included)));
        assert!(!rows.iter().any(|(_, v)| *v == ComingSoon));
    }

    #[test]
    fn comparison_rows_follow_board_order_and_server_limits() {
        use billing::FeatureValue::{ComingSoon, Included, Muted, Value};
        let mut account = dashboard_panel();
        account.features = Some(billing_v2_features());
        let rows = billing::comparison_rows(&account);
        let names: Vec<&str> = rows.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "Create and recover Cubes",
                "Cubes per network",
                "Hosted Esplora server",
                "Lightning Address for Spark",
                "Recovery alerts",
                "Inheritance escrow",
                "Keyholder invitations",
                billing::SPENDING_POLICIES,
            ]
        );
        assert_eq!(
            rows[1].1,
            [
                Value("1".to_string()),
                Value("3".to_string()),
                Value("10".to_string())
            ]
        );
        assert_eq!(rows[4].1, [Included, Included, Included]);
        assert_eq!(rows[7].1, [Muted("—".to_string()), ComingSoon, ComingSoon]);
    }

    #[test]
    fn change_plan_cta_names_the_change_and_follows_the_api_rules() {
        let mut account = dashboard_panel();
        account.features = Some(billing_v2_features());
        // /connect/plan sends no billing cycle in production.
        let mut pro = plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2099-01-01T00:00:00Z"),
        );
        pro.billing_cycle = None;
        account.plan = Some(pro.clone());
        let (label, _, ok) =
            billing::change_plan_cta(&account, &PlanTier::Estate, BillingCycle::Annual);
        assert_eq!((label.as_str(), ok), ("Review upgrade to Estate", true));
        // The current tier is renewable on either period (Connect stacks it).
        for cycle in [BillingCycle::Monthly, BillingCycle::Annual] {
            let (label, note, ok) = billing::change_plan_cta(&account, &PlanTier::Pro, cycle);
            assert!(ok, "renewal enabled for {:?}", cycle);
            assert!(label.starts_with("Review renewal · Pro"), "{}", label);
            assert!(note.contains("after your current one ends"), "{}", note);
        }
        let (label, _, ok) =
            billing::change_plan_cta(&account, &PlanTier::Free, BillingCycle::Annual);
        assert_eq!((label.as_str(), ok), ("Review switch to Free", true));

        // A lower tier needs a future period end and no change already scheduled.
        let mut estate = plan(
            PlanTier::Estate,
            PlanStatus::Active,
            Some("2099-01-01T00:00:00Z"),
        );
        estate.billing_cycle = None;
        account.plan = Some(estate.clone());
        let (_, _, ok) = billing::change_plan_cta(&account, &PlanTier::Pro, BillingCycle::Annual);
        assert!(ok);
        estate.pending_plan = Some(PlanTier::Pro);
        estate.pending_plan_at = Some("2099-01-01T00:00:00Z".into());
        account.plan = Some(estate.clone());
        let (_, note, ok) =
            billing::change_plan_cta(&account, &PlanTier::Pro, BillingCycle::Annual);
        assert!(!ok);
        assert!(note.contains("already scheduled"), "{}", note);
        estate.pending_plan = None;
        estate.renewal_at = None;
        account.plan = Some(estate);
        let (_, _, ok) = billing::change_plan_cta(&account, &PlanTier::Pro, BillingCycle::Annual);
        assert!(!ok, "no period end to start after");

        // Free is never the current plan to renew; paused purchasing blocks
        // purchases but not the switch to Free.
        account.plan = Some(plan(PlanTier::Free, PlanStatus::Active, None));
        let (_, _, ok) = billing::change_plan_cta(&account, &PlanTier::Free, BillingCycle::Annual);
        assert!(!ok);
        account.plan = Some(pro);
        account.features.as_mut().unwrap().purchasing_enabled = Some(false);
        let (_, note, ok) =
            billing::change_plan_cta(&account, &PlanTier::Estate, BillingCycle::Annual);
        assert!(!ok);
        assert!(note.contains("paused"));
        let (_, _, ok) = billing::change_plan_cta(&account, &PlanTier::Free, BillingCycle::Annual);
        assert!(ok);
    }

    #[test]
    fn paid_screen_keeps_the_reviewed_facts_after_the_plan_reloads() {
        let now = chrono::Utc::now();
        let renewal = (now + chrono::Duration::days(120)).to_rfc3339();
        let mut account = dashboard_panel();
        account.features = Some(billing_v2_features());
        account.plan = Some(plan(PlanTier::Pro, PlanStatus::Active, Some(&renewal)));
        let mut cs = CheckoutState::new(
            1,
            PlanTier::Estate,
            BillingCycle::Annual,
            account.plan.clone(),
        );
        let before = billing::review_facts(&account, &cs, now);
        assert!(before
            .replaces
            .as_deref()
            .unwrap()
            .starts_with("Replaces Pro"));
        assert!(
            before.notes.iter().any(|n| n.contains("isn’t credited")),
            "{:?}",
            before.notes
        );
        // Paid: /connect/plan now says Estate for a year.
        cs.phase = CheckoutPhase::Paid { overpaid_sats: 0 };
        let next_year = (now + chrono::Duration::days(365)).to_rfc3339();
        account.plan = Some(plan(PlanTier::Estate, PlanStatus::Active, Some(&next_year)));
        let after = billing::review_facts(&account, &cs, now);
        assert_eq!(after.covers, before.covers);
        assert_eq!(after.replaces, before.replaces);
    }

    #[test]
    fn lapsed_account_lists_free_features() {
        let mut account = dashboard_panel();
        account.features = Some(billing_v2_features());
        let mut lapsed = plan(
            PlanTier::Pro,
            PlanStatus::PastDue,
            Some("2026-01-01T00:00:00Z"),
        );
        lapsed.entitlements.cube_limit = 1;
        account.plan = Some(lapsed);
        let rows = billing::account_feature_rows(&account);
        assert_eq!(rows[0].1, billing::FeatureValue::Muted("1 on Free".into()));
        assert!(!rows.iter().any(|(n, _)| n == billing::SPENDING_POLICIES));
    }

    #[test]
    fn review_facts_cover_upgrade_renewal_and_downgrade() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut account = dashboard_panel();
        account.features = Some(billing_v2_features());
        let mut p = plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2027-03-14T00:00:00Z"),
        );
        p.billing_cycle = Some(BillingCycle::Annual);
        account.plan = Some(p);
        let checkout = |plan: &Option<ConnectPlan>, tier, cycle| {
            CheckoutState::new(1, tier, cycle, plan.clone())
        };

        // Upgrade starts now.
        let up = billing::review_facts(
            &account,
            &checkout(&account.plan, PlanTier::Estate, BillingCycle::Annual),
            now,
        );
        assert_eq!(up.plan, "Estate · Annual");
        assert_eq!(up.replaces.as_deref(), Some("Replaces Pro · Annual"));
        assert_eq!(up.amount.as_deref(), Some("$200 USD"));
        assert_eq!(up.covers.as_deref(), Some("Nov 13, 2026 – Nov 13, 2027"));
        assert_eq!(up.starts, None);

        // Same-tier renewal stacks onto the paid period.
        let renew = billing::review_facts(
            &account,
            &checkout(&account.plan, PlanTier::Pro, BillingCycle::Annual),
            now,
        );
        assert_eq!(renew.covers.as_deref(), Some("Mar 14, 2027 – Mar 14, 2028"));
        assert_eq!(renew.starts.as_deref(), Some("Mar 14, 2027"));
        assert_eq!(
            renew.replaces.as_deref(),
            Some("Adds to your current Pro plan")
        );

        // A lower tier starts when the current period ends (decision 12).
        let mut estate = plan(
            PlanTier::Estate,
            PlanStatus::Active,
            Some("2027-03-14T00:00:00Z"),
        );
        estate.billing_cycle = Some(BillingCycle::Annual);
        account.plan = Some(estate);
        let down = billing::review_facts(
            &account,
            &checkout(&account.plan, PlanTier::Pro, BillingCycle::Monthly),
            now,
        );
        assert_eq!(down.amount.as_deref(), Some("$10 USD"));
        assert_eq!(down.starts.as_deref(), Some("Mar 14, 2027"));
        assert_eq!(down.covers.as_deref(), Some("Mar 14, 2027 – Apr 14, 2027"));
    }

    #[tokio::test]
    async fn plan_billing_board_states_render_tenshu_copy() {
        use iced::advanced::renderer::Headless;
        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let mut account = dashboard_panel();
        account.features = Some(billing_v2_features());
        account.billing_history = Some(vec![
            billing_entry(ChargeStatus::Paid),
            billing_entry(ChargeStatus::Paid),
            billing_entry(ChargeStatus::Expired),
            billing_entry(ChargeStatus::Paid),
        ]);

        // Board 1, Active.
        let labels = visible_labels(plan_billing_ux(&account), &renderer);
        for expected in [
            "Plan & billing",
            "Current plan",
            "Change plan…",
            "Have a promo or referral code?",
            "Features on this account",
            "Cubes per network",
            "Up to 3",
            "Coming soon",
            "View all billing history",
        ] {
            assert!(
                labels.iter().any(|l| l == expected),
                "{}: {:?}",
                expected,
                labels
            );
        }
        // Only the latest three history rows are inline.
        assert_eq!(
            labels.iter().filter(|l| l.ends_with("· bitcoin")).count(),
            3
        );
        let cubes = labels
            .iter()
            .position(|l| l == "Cubes per network")
            .unwrap();
        let soon = labels
            .iter()
            .position(|l| l == billing::SPENDING_POLICIES)
            .unwrap();
        assert!(cubes < soon);

        // Board 1, Promotional: the Access row.
        account.plan = Some(granted_estate(
            Some("2026-12-19T00:00:00Z"),
            Some("Launch offer"),
        ));
        let labels = visible_labels(plan_billing_ux(&account), &renderer);
        assert!(labels.iter().any(|l| l == "Access"), "{:?}", labels);
        assert!(labels.iter().any(|l| l == "No payment on file"));

        // Board 2: Change plan with the wired CTA.
        account.plan = Some(plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2027-01-01T00:00:00Z"),
        ));
        account.show_plan_comparison = true;
        account.selected_billing_cycle = BillingCycle::Annual;
        account.selected_plan_tier = Some(PlanTier::Estate);
        let labels = visible_labels(plan_billing_ux(&account), &renderer);
        for expected in [
            "Change plan",
            "Monthly",
            "Annual",
            "$200",
            "per year",
            "Review upgrade to Estate",
            "What each plan includes",
            "Keyholder invitations",
        ] {
            assert!(
                labels.iter().any(|l| l == expected),
                "{}: {:?}",
                expected,
                labels
            );
        }
        assert!(!labels.iter().any(|l| l == "BEST VALUE"));

        // Board 3: Review.
        let _ = account.update_message(ConnectAccountMessage::ReviewPlanChange(PlanTier::Estate));
        let labels = visible_labels(plan_billing_ux(&account), &renderer);
        for expected in [
            "Review plan change",
            "Estate · Annual",
            "$200 USD",
            "Not invoiced yet",
            "Pay with",
            "Bitcoin",
            "Card",
            "Create invoice",
        ] {
            assert!(
                labels.iter().any(|l| l == expected),
                "{}: {:?}",
                expected,
                labels
            );
        }
    }

    /// Draw `account`'s Plan & billing sheet at `size` into
    /// `$BILLING_PREVIEW_DIR/billing-{name}-{appearance}.png` for PR
    /// evidence. A no-op unless the variable is set.
    pub(super) async fn capture_billing_sheet(
        account: &ConnectAccountPanel,
        name: &str,
        size: (u32, u32),
    ) {
        use iced::advanced::{layout, renderer::Headless, widget::Tree, Layout};
        let Ok(directory) = std::env::var("BILLING_PREVIEW_DIR") else {
            return;
        };
        for bytes in [
            coincube_ui::font::REGULAR_BYTES,
            coincube_ui::font::MEDIUM_BYTES,
            coincube_ui::font::SEMIBOLD_BYTES,
            coincube_ui::font::BOLD_BYTES,
            coincube_ui::font::MONO_REGULAR_BYTES,
            coincube_ui::font::BOOTSTRAP_ICONS_BYTE,
            coincube_ui::font::COINCUBE_ICONS_BYTES,
        ] {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(std::borrow::Cow::Borrowed(bytes));
        }
        let (width, height) = size;
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            let mut renderer = <iced::Renderer as Headless>::new(
                coincube_ui::font::REGULAR,
                iced::Pixels(16.0),
                Some("tiny-skia"),
            )
            .await
            .expect("software renderer");
            let mut element = connect_sheet(
                account,
                ConnectSheetPage::Sections(ConnectSheetSection::PlanBilling),
            );
            let mut tree = Tree::new(element.as_widget());
            let viewport_size = iced::Size::new(width as f32, height as f32);
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(iced::Size::ZERO, viewport_size),
            );
            let viewport = iced::Rectangle::with_size(viewport_size);
            element.as_widget_mut().update(
                &mut tree,
                &iced::Event::Window(iced::window::Event::RedrawRequested(
                    std::time::Instant::now(),
                )),
                Layout::new(&node),
                iced::mouse::Cursor::Unavailable,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut Vec::new()),
                &viewport,
            );
            element.as_widget().draw(
                &tree,
                &mut renderer,
                &theme,
                &iced::advanced::renderer::Style {
                    text_color: theme.colors.text.primary,
                },
                Layout::new(&node),
                iced::mouse::Cursor::Unavailable,
                &viewport,
            );
            let pixels = renderer.screenshot(
                iced::Size::new(width, height),
                1.0,
                theme.colors.general.background,
            );
            image::save_buffer(
                std::path::Path::new(&directory).join(format!("billing-{name}-{appearance}.png")),
                &pixels,
                width,
                height,
                image::ColorType::Rgba8,
            )
            .expect("save billing preview");
        }
    }

    /// Board 1 and 2 states as PNGs (`BILLING_PREVIEW_DIR`). Also checks
    /// every state lays out inside the sheet.
    #[tokio::test(flavor = "multi_thread")]
    async fn plan_billing_board_captures() {
        let mut account = dashboard_panel();
        account.features = Some(billing_v2_features());
        let history = vec![
            billing_entry(ChargeStatus::Paid),
            billing_entry(ChargeStatus::Paid),
            billing_entry(ChargeStatus::Expired),
        ];
        account.billing_history = Some(history);
        let mut annual = plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2027-11-13T00:00:00Z"),
        );
        annual.billing_cycle = Some(BillingCycle::Annual);
        account.plan = Some(annual.clone());
        capture_billing_sheet(&account, "1-active", (1040, 1180)).await;

        let soon = (chrono::Utc::now() + chrono::Duration::days(5)).to_rfc3339();
        let mut due = annual.clone();
        due.renewal_at = Some(soon);
        account.plan = Some(due);
        capture_billing_sheet(&account, "1-renewal-due", (1040, 1240)).await;

        account.plan = Some(plan(
            PlanTier::Free,
            PlanStatus::PastDue,
            Some("2026-10-01T00:00:00Z"),
        ));
        capture_billing_sheet(&account, "1-expired", (1040, 1240)).await;

        let mut cancelled = annual.clone();
        cancelled.status = PlanStatus::Canceled;
        account.plan = Some(cancelled);
        capture_billing_sheet(&account, "1-cancelled", (1040, 1240)).await;

        account.plan = Some(granted_estate(
            Some("2026-12-19T00:00:00Z"),
            Some("Launch offer"),
        ));
        account.billing_history = Some(Vec::new());
        account.show_campaign_redeem = true;
        capture_billing_sheet(&account, "1-promotional", (1040, 1240)).await;
        account.show_campaign_redeem = false;

        account.plan = Some(annual);
        account.show_plan_comparison = true;
        account.selected_billing_cycle = BillingCycle::Annual;
        account.selected_plan_tier = Some(PlanTier::Estate);
        capture_billing_sheet(&account, "2-change-plan", (1040, 1300)).await;
        account.features.as_mut().unwrap().purchasing_enabled = Some(false);
        capture_billing_sheet(&account, "2-purchasing-paused", (1040, 1300)).await;
        account.features = None;
        capture_billing_sheet(&account, "2-pricing-unavailable", (1040, 1000)).await;

        account.features = Some(billing_v2_features());
        let _ = account.update_message(ConnectAccountMessage::ReviewPlanChange(PlanTier::Estate));
        capture_billing_sheet(&account, "3-review", (1040, 960)).await;
        account.checkout = None;
        account.show_plan_comparison = true;
        let _ = account.update_message(ConnectAccountMessage::ReviewPlanChange(PlanTier::Free));
        capture_billing_sheet(&account, "3-review-free", (1040, 860)).await;
    }

    /// Every BTCPay board (B1–B10) renders its copy, and the invoice is
    /// shown in full (no 40-character truncation).
    #[tokio::test(flavor = "multi_thread")]
    async fn btcpay_phase_boards_render_and_capture() {
        use crate::app::state::connect::account::PaymentNetwork;
        use iced::advanced::renderer::Headless;
        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let mut account = dashboard_panel();
        account.features = Some(billing_v2_features());
        let mut p = plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2027-03-14T00:00:00Z"),
        );
        p.billing_cycle = Some(BillingCycle::Annual);
        account.plan = Some(p);
        let mut resp = checkout_response();
        resp.plan = PlanTier::Estate;
        resp.amount_fiat = 200.0;
        resp.amount_sats = 210_000;
        resp.lightning_invoice = format!("lnbc2100u1p{}", "q".repeat(180));
        resp.expires_at = Some((chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339());
        let status = |json: &str| -> crate::services::coincube::ChargeStatusResponse {
            serde_json::from_str(json).unwrap()
        };
        type Case<'s> = (
            &'s str,
            CheckoutPhase,
            PaymentNetwork,
            Option<&'s str>,
            &'s [&'s str],
        );
        let cases: Vec<Case> = vec![
            (
                "b01-creating",
                CheckoutPhase::Creating,
                PaymentNetwork::Lightning,
                None,
                &["Pay for Estate", "Creating invoice…"],
            ),
            (
                "b02-lightning",
                CheckoutPhase::AwaitingPayment,
                PaymentNetwork::Lightning,
                None,
                &[
                    "Lightning invoice",
                    "Pay on the BTCPay page instead",
                    "Lightning",
                    "On-chain",
                ],
            ),
            (
                "b03-onchain",
                CheckoutPhase::AwaitingPayment,
                PaymentNetwork::OnChain,
                None,
                &[
                    "Bitcoin address",
                    "Send exactly 210,000 sats. Estate turns on after the payment confirms.",
                ],
            ),
            (
                "b04-browser",
                CheckoutPhase::PayingInBrowser,
                PaymentNetwork::Lightning,
                None,
                &[
                    "Finish paying on the BTCPay page",
                    "Reopen checkout page",
                    "Show the QR code here instead",
                ],
            ),
            (
                "b05-partial",
                CheckoutPhase::PartlyPaid {
                    received_sats: 130_200,
                    due_sats: 79_800,
                },
                PaymentNetwork::OnChain,
                None,
                &[
                    "Send the rest on-chain",
                    "130,200 of 210,000 sats received · 79,800 sats left to pay",
                    "Same Bitcoin address · send 79,800 sats more",
                ],
            ),
            (
                "b06-confirming",
                CheckoutPhase::Processing,
                PaymentNetwork::OnChain,
                None,
                &[
                    "Payment seen, waiting for confirmation",
                    "Payment seen · confirming",
                ],
            ),
            (
                "b07-paid",
                CheckoutPhase::Paid { overpaid_sats: 0 },
                PaymentNetwork::Lightning,
                None,
                &["Plan changed", "You're on Estate", "Done", "Paid"],
            ),
            (
                "b07-paid-overpaid",
                CheckoutPhase::Paid {
                    overpaid_sats: 1_500,
                },
                PaymentNetwork::Lightning,
                None,
                &["You're on Estate"],
            ),
            (
                "b08-expired",
                CheckoutPhase::Expired,
                PaymentNetwork::Lightning,
                None,
                &[
                    "This invoice expired",
                    "Create new invoice",
                    "Expired · nothing received",
                ],
            ),
            (
                "b09-needs-support",
                CheckoutPhase::NeedsSupport {
                    invoice_id: "Q7xWqv3kP1".into(),
                    received_sats: 210_000,
                },
                PaymentNetwork::Lightning,
                Some(
                    r#"{"chargeId":"charge-1","invoiceId":"Q7xWqv3kP1","status":"expired","state":"needs_support","plan":"estate","additionalStatus":"PaidLate","amountReceivedSats":210000}"#,
                ),
                &[
                    "Payment arrived after the invoice expired",
                    "Invoice ID",
                    "Q7xWqv3kP1",
                    "Needs support",
                ],
            ),
            (
                "b10-failed",
                CheckoutPhase::Failed {
                    message: "Connect returned 502".into(),
                    refused: false,
                },
                PaymentNetwork::Lightning,
                None,
                &["Couldn’t create the invoice", "Try again", "Not invoiced"],
            ),
            (
                "b10-poll-failed",
                CheckoutPhase::PollFailed("timeout".into()),
                PaymentNetwork::Lightning,
                None,
                &["Couldn’t check the payment", "Check again"],
            ),
        ];
        for (name, phase, network, last, expected) in cases {
            let mut cs = CheckoutState::new(
                7,
                PlanTier::Estate,
                BillingCycle::Annual,
                account.plan.clone(),
            );
            if phase != CheckoutPhase::Creating && !matches!(phase, CheckoutPhase::Failed { .. }) {
                cs.checkout = Some(resp.clone());
                cs.lightning_qr = iced::widget::qr_code::Data::new(&resp.lightning_invoice).ok();
                cs.onchain_qr = iced::widget::qr_code::Data::new(
                    crate::app::state::connect::account::bip21_uri(&resp.on_chain_address, 79_800),
                )
                .ok();
            }
            cs.phase = phase;
            cs.network = network;
            cs.last_status = last.map(status);
            account.checkout = Some(cs);
            let labels = visible_labels(plan_billing_ux(&account), &renderer);
            for text in expected {
                assert!(
                    labels.iter().any(|l| l == text),
                    "{}: missing {:?} in {:?}",
                    name,
                    text,
                    labels
                );
            }
            if network == PaymentNetwork::Lightning && name == "b02-lightning" {
                assert!(
                    labels.iter().any(|l| l == &resp.lightning_invoice),
                    "full invoice shown"
                );
            }
            capture_billing_sheet(&account, name, (1040, 1000)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_billing_bot_findings_hold() {
        use iced::advanced::renderer::Headless;
        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let mut account = dashboard_panel();
        let mut f = billing_v2_features();
        f.purchasing_enabled = Some(false);
        f.pricing_schema_version = Some(SUPPORTED_PRICING_SCHEMA_VERSION + 1);
        account.features = Some(f);
        // Plan & billing itself shows the pause and schema notices.
        let labels = visible_labels(plan_billing_ux(&account), &renderer);
        assert!(
            labels.iter().any(|l| l == "Purchasing is paused"),
            "{:?}",
            labels
        );
        assert!(labels
            .iter()
            .any(|l| l == "A newer pricing update is available"));

        // A paid tier without a catalog price is never labelled Free.
        let mut f = billing_v2_features();
        f.plans[1].price = None;
        account.features = Some(f);
        account.show_plan_comparison = true;
        let labels = visible_labels(plan_billing_ux(&account), &renderer);
        assert!(
            labels.iter().any(|l| l == "Price unavailable"),
            "{:?}",
            labels
        );
        assert_eq!(labels.iter().filter(|l| *l == "No payment").count(), 1);
        account.show_plan_comparison = false;

        // Switching to Free with a paid lower tier scheduled shows no
        // "Free from" date.
        let mut estate = plan(
            PlanTier::Estate,
            PlanStatus::Active,
            Some("2099-03-14T00:00:00Z"),
        );
        estate.pending_plan = Some(PlanTier::Pro);
        estate.pending_plan_at = Some("2099-03-14T00:00:00Z".into());
        let cs = CheckoutState::new(1, PlanTier::Free, BillingCycle::Annual, Some(estate));
        let facts = billing::review_facts(&account, &cs, chrono::Utc::now());
        assert_eq!(facts.covers, None);

        // Covers follows Connect's startsAt for a lower tier bought ahead.
        account.features = Some(billing_v2_features());
        let mut cs =
            CheckoutState::new(2, PlanTier::Pro, BillingCycle::Annual, account.plan.clone());
        let mut resp = checkout_response();
        resp.starts_at = Some("2027-04-14T00:00:00Z".into());
        cs.checkout = Some(resp);
        cs.phase = CheckoutPhase::Paid { overpaid_sats: 0 };
        account.checkout = Some(cs);
        let labels = visible_labels(plan_billing_ux(&account), &renderer);
        assert!(
            labels.iter().any(|l| l == "Apr 14, 2027 – Apr 14, 2028"),
            "{:?}",
            labels
        );

        // Covers for a purchase that starts now uses when Review opened.
        let mut cs = CheckoutState::new(
            3,
            PlanTier::Estate,
            BillingCycle::Annual,
            account.plan.clone(),
        );
        cs.opened_at = chrono::DateTime::parse_from_rfc3339("2026-11-13T23:59:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let facts = billing::review_facts(&account, &cs, cs.opened_at);
        assert_eq!(facts.covers.as_deref(), Some("Nov 13, 2026 – Nov 13, 2027"));
    }

    #[test]
    fn plan_billing_checkout_and_history_views_build_for_branch_states() {
        let mut account = dashboard_panel();
        account.active_sub = ConnectSubMenu::PlanBilling;

        account.features = None;
        account.plan = Some(granted_estate(Some("2027-07-04T00:00:00Z"), Some("Grant")));
        account.campaign_redeem.code = "FOUNDERS".to_string();
        account.campaign_redeem.result = Some(Ok("Code applied.".to_string()));
        let _ = plan_billing_ux(&account);
        account.show_plan_comparison = true;
        let _ = plan_billing_ux(&account);

        account.features = Some(priced_features(
            Some(SUPPORTED_PRICING_SCHEMA_VERSION + 1),
            Some(false),
        ));
        account.selected_billing_cycle = BillingCycle::Annual;
        account.campaign_redeem.submitting = true;
        let _ = plan_billing_ux(&account);
        account.show_plan_comparison = false;
        account.show_campaign_redeem = true;
        let _ = plan_billing_ux(&account);

        for status in [PlanStatus::Canceled, PlanStatus::PastDue] {
            account.plan = Some(plan(PlanTier::Pro, status, Some("2026-01-01T00:00:00Z")));
            let _ = plan_billing_ux(&account);
        }

        account.show_billing_history = true;
        account.billing_history = None;
        let _ = plan_billing_ux(&account);
        account.billing_history = Some(Vec::new());
        let _ = plan_billing_ux(&account);
        account.billing_history = Some(vec![
            billing_entry(ChargeStatus::Unpaid),
            billing_entry(ChargeStatus::Processing),
            billing_entry(ChargeStatus::Paid),
            billing_entry(ChargeStatus::Expired),
        ]);
        let _ = plan_billing_ux(&account);

        account.show_billing_history = false;
        for phase in [
            CheckoutPhase::Creating,
            CheckoutPhase::AwaitingPayment,
            CheckoutPhase::Processing,
            CheckoutPhase::Paid { overpaid_sats: 0 },
            CheckoutPhase::Expired,
            CheckoutPhase::Failed {
                message: "invoice failed".to_string(),
                refused: false,
            },
        ] {
            let mut cs =
                CheckoutState::new(1, PlanTier::Pro, BillingCycle::Annual, account.plan.clone());
            cs.phase = phase;
            cs.checkout = Some(checkout_response());
            account.checkout = Some(cs);
            let _ = plan_billing_ux(&account);
        }
    }

    #[test]
    fn duress_views_build_locked_setup_enabled_and_disable_states() {
        let mut account = dashboard_panel();
        account.active_sub = ConnectSubMenu::Duress;

        account.plan = Some(plan(PlanTier::Free, PlanStatus::Active, None));
        let _ = duress_ux(&account);

        let mut entitled = plan(PlanTier::Pro, PlanStatus::Active, None);
        entitled.entitlements.duress = true;
        account.plan = Some(entitled);
        account.duress_cubes = Some(vec![DuressCube {
            server_id: 1,
            uuid: Some("cube-uuid".to_string()),
            name: "Family Vault".to_string(),
            has_recovery_kit: true,
            halves: Some((true, false)),
            has_vault: Some(true),
            is_passkey: Some(false),
            local: true,
        }]);

        // Entitled but the launch gate is off (features cleared → flag off, not
        // enrolled): the hidden-placeholder branch. Normally unreachable (nav
        // hides it + route backstop redirects); exercised here belt-and-suspenders.
        account.features = None;
        assert!(!account.show_duress());
        let _ = duress_ux(&account);

        // Launch flag on → gate opens, so the not-yet-enrolled setup flow (not
        // the hidden placeholder) renders. `priced_features` carries
        // `duressEnabled: true`.
        account.features = Some(priced_features(None, None));
        assert!(account.show_duress());
        let _ = duress_ux(&account);

        account.duress_locally_armed = true;
        account.duress_state = Some(crate::services::coincube::DuressState {
            active: false,
            unlock_at: None,
            enrolled: true,
            this_device_registered: true,
        });
        let _ = duress_ux(&account);

        account.duress_locally_armed = false;
        account.duress_state = Some(crate::services::coincube::DuressState {
            active: true,
            unlock_at: None,
            enrolled: true,
            this_device_registered: false,
        });
        let _ = duress_ux(&account);

        // Every shape of the step-up dialog: still probing, PIN, passkey, and
        // the device that can offer neither. Each renders a different body and
        // a different confirm affordance, so each is built here.
        for method in [
            None,
            Some(crate::app::DuressStepUpMethod::Pin),
            Some(crate::app::DuressStepUpMethod::Passkey(Box::new(
                crate::app::settings::CubeSettings::new_with_id(
                    uuid::Uuid::nil(),
                    "Passkey Cube".to_string(),
                    coincube_core::miniscript::bitcoin::Network::Bitcoin,
                ),
            ))),
            Some(crate::app::DuressStepUpMethod::Unavailable),
        ] {
            account.duress_disable = Some(DuressDisableState {
                pin: "1234".to_string(),
                method,
                submitting: false,
                error: Some("bad pin".to_string()),
            });
            let _ = duress_ux(&account);
        }

        // Mid-flight: both confirm buttons swap to their waiting label and the
        // Cancel button goes inert.
        account.duress_disable = Some(DuressDisableState {
            pin: "1234".to_string(),
            method: Some(crate::app::DuressStepUpMethod::Passkey(Box::new(
                crate::app::settings::CubeSettings::new_with_id(
                    uuid::Uuid::nil(),
                    "Passkey Cube".to_string(),
                    coincube_core::miniscript::bitcoin::Network::Bitcoin,
                ),
            ))),
            submitting: true,
            error: None,
        });
        let _ = duress_ux(&account);
    }

    #[test]
    fn duress_status_fits_compact_connect_in_both_themes() {
        use crate::app::view::shared::landing::capture;

        let mut account = dashboard_panel();
        account.active_sub = ConnectSubMenu::Duress;
        account.plan.as_mut().unwrap().entitlements.duress = true;
        account.plan.as_mut().unwrap().entitlements.duress_alerts = true;
        account.features.as_mut().unwrap().duress_enabled = Some(true);
        account.duress_cubes = Some(Vec::new());
        account.duress_contacts.contacts =
            Some(vec![crate::services::coincube::DuressAlertContact {
                id: 1,
                display_name: "Synthetic emergency contact".to_string(),
                phone: None,
                email: Some("synthetic-contact@example.test".to_string()),
                channels: vec!["email".to_string()],
                intro_sent_at: None,
                opted_out_at: None,
                created_at: None,
                updated_at: None,
            }]);
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            // Exercise the live sheet route with synthetic account/contact
            // data at the 1024px minimum, in both supported appearances.
            let sheet_padding = iced::Padding {
                top: 28.0,
                right: 40.0,
                bottom: 40.0,
                left: 40.0,
            };
            let inspection = capture::inspect_full_window(
                "duress-status-sheet",
                connect_sheet(
                    &account,
                    ConnectSheetPage::Sections(ConnectSheetSection::Duress),
                ),
                (1024, 768),
                appearance,
                theme,
            );
            assert!(inspection.contains("What duress mode does"));
            assert!(inspection.contains("Set up a Cube Recovery Kit for each Cube"));
            assert!(inspection.contains("Set up Duress Mode"));
            assert!(inspection.contains("Synthetic emergency contact"));
            assert!(inspection.contains("Remove"));

            let contacts = capture::inspect_wide_content(
                "duress-contacts-content-mount",
                container(duress_contacts::section(&account))
                    .padding(sheet_padding)
                    .into(),
                (1024, 768),
                appearance,
                theme,
                711,
            );
            assert!(contacts.contains("Synthetic emergency contact"));
            assert!(contacts.contains("Intro message queued"));
        }
    }

    #[test]
    fn connect_panel_and_avatar_views_build_active_paths() {
        let mut panel = ConnectPanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "bitcoin".to_string(),
            true,
        );
        panel.account.error = Some(PanelError::retryable(
            UserError::new(
                "Can't reach COINCUBE | Connect",
                crate::user_error::RETRY_GUIDANCE,
                crate::user_error::CC_NET_TIMEOUT,
                true,
            ),
            crate::app::view::RetryAction::Session,
        ));
        for step in [
            ConnectFlowStep::CheckingSession,
            ConnectFlowStep::Login {
                email: String::new(),
                loading: false,
            },
            ConnectFlowStep::Register {
                email: "new@example.com".to_string(),
                loading: true,
            },
            ConnectFlowStep::Dashboard,
        ] {
            panel.account.step = step;
            let _ = connect_panel(&panel);
        }

        let mut cube = ConnectCubePanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "bitcoin".to_string(),
            true,
        );
        let _ = avatar_ux(&cube);

        cube.lightning_address = Some(LightningAddress {
            lightning_address: Some("founder@getcoincube.com".to_string()),
        });
        cube.avatar_step = AvatarFlowStep::Questionnaire;
        cube.avatar_draft.gender = AvatarGender::Woman;
        cube.avatar_draft.archetype = AvatarArchetype::Shogun;
        cube.avatar_draft.age_feel = AvatarAgeFeel::Elder;
        cube.avatar_draft.demeanor = AvatarDemeanor::Fierce;
        cube.avatar_draft.armor_style = AvatarArmorStyle::Heavy;
        cube.avatar_draft.accent_motif = AvatarAccentMotif::OrangeSun;
        cube.avatar_draft.laser_eyes = true;
        let _ = avatar_ux(&cube);

        cube.avatar_step = AvatarFlowStep::Generating;
        let _ = avatar_ux(&cube);

        cube.avatar_error = Some("avatar unavailable".to_string());
        let _ = avatar_ux(&cube);
        cube.registration_error = Some("not registered".to_string());
        let _ = avatar_ux(&cube);
        cube.avatar_error = None;
        cube.registration_error = None;

        cube.avatar_step = AvatarFlowStep::Settings;
        let _ = avatar_ux(&cube);
        cube.avatar_data = Some(GetAvatarData {
            has_avatar: true,
            active_avatar_url: Some("https://cdn.example/avatar/7".to_string()),
            identity: None,
            variants: vec![
                AvatarVariant {
                    id: 7,
                    index: 0,
                    image_url: "https://cdn.example/avatar/7".to_string(),
                },
                AvatarVariant {
                    id: 8,
                    index: 1,
                    image_url: "https://cdn.example/avatar/8".to_string(),
                },
            ],
            regenerations_remaining: -1,
            created_at: None,
            updated_at: None,
        });
        let _ = avatar_ux(&cube);

        cube.avatar_step = AvatarFlowStep::Reveal;
        if let Some(data) = cube.avatar_data.as_mut() {
            data.active_avatar_url = Some("https://cdn.example/avatar/none".to_string());
            data.regenerations_remaining = 0;
            data.variants.truncate(1);
        }
        let _ = avatar_ux(&cube);
    }

    /// Layout uses the actual Connect widgets in a compact scroll viewport.
    /// Root bounds describe that viewport, not the height of its scrollable
    /// content. Optional PNG output supports review at the 1024×768 minimum.
    #[tokio::test(flavor = "multi_thread")]
    async fn connect_views_layout_in_compact_scroll_viewport_and_can_be_captured() {
        use iced::advanced::{layout, renderer::Headless, widget::Tree, Layout};

        for bytes in [
            coincube_ui::font::REGULAR_BYTES,
            coincube_ui::font::MEDIUM_BYTES,
            coincube_ui::font::BOLD_BYTES,
            coincube_ui::font::BOOTSTRAP_ICONS_BYTE,
        ] {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(std::borrow::Cow::Borrowed(bytes));
        }

        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for scenario in [
                "sign-in",
                "sign-up",
                "token",
                "account",
                "account-long-email",
                "account-review",
                "plan",
                "plan-promo-success",
                "plan-promo-failure",
                "compare",
                "recovery",
                "recovery-error-retryable",
                "recovery-error-terminal",
                "checkout",
                "security",
                "device-review",
                "contacts",
            ] {
                let mut panel = ConnectAccountPanel::new();
                match scenario {
                    "sign-in" => {
                        panel.step = ConnectFlowStep::Login {
                            email: String::new(),
                            loading: false,
                        };
                    }
                    "sign-up" => {
                        panel.step = ConnectFlowStep::Register {
                            email: String::new(),
                            loading: false,
                        };
                    }
                    "token" => {
                        panel.step = ConnectFlowStep::OtpVerification {
                            email: "founder@example.test".into(),
                            otp: String::new(),
                            sending: false,
                            is_signup: false,
                            cooldown: 30,
                        };
                    }
                    section => {
                        panel.step = ConnectFlowStep::Dashboard;
                        panel.user = Some(user());
                        if section == "account-long-email" {
                            panel.user.as_mut().unwrap().email = "morgan.with-a-long-household-recovery-address@family-office.example.test".into();
                        }
                        panel.plan = Some(plan(
                            PlanTier::Pro,
                            PlanStatus::Active,
                            Some("2027-07-04T00:00:00Z"),
                        ));
                        panel.features = Some(priced_features(None, Some(false)));
                        panel.active_sub = match section {
                            "plan" | "plan-promo-success" | "plan-promo-failure" | "compare"
                            | "checkout" => ConnectSubMenu::PlanBilling,
                            "security" | "device-review" => ConnectSubMenu::Security,
                            "contacts" => ConnectSubMenu::Contacts,
                            _ => ConnectSubMenu::Overview,
                        };
                        panel.verified_devices = Some(vec![
                            VerifiedDevice {
                                id: 7,
                                device_name: Some("This Mac".into()),
                                created_at: "2026-10-01T00:00:00Z".into(),
                                last_used_at: Some("2026-10-23T12:00:00Z".into()),
                                is_current: true,
                            },
                            VerifiedDevice {
                                id: 9,
                                device_name: Some("Travel laptop".into()),
                                created_at: "2026-09-01T00:00:00Z".into(),
                                last_used_at: Some("2026-10-19T12:00:00Z".into()),
                                is_current: false,
                            },
                        ]);
                        panel.login_activity = Some(vec![LoginActivity {
                            id: 8,
                            ip_address: None,
                            device_name: Some("This Mac".into()),
                            created_at: "2026-10-23T12:00:00Z".into(),
                            success: Some(true),
                        }]);
                        panel.contacts_state.contacts = Some(Vec::new());
                        panel.contacts_state.invites = Some(Vec::new());
                        panel.contacts_state.received_invites = Some(Vec::new());
                        if section == "compare" {
                            panel.show_plan_comparison = true;
                        }
                        if section == "recovery" {
                            let cube: crate::services::coincube::CubeResponse = serde_json::from_str(
                                r#"{"id":7,"uuid":"cube-7","name":"Family Vault","network":"bitcoin","status":"active","hasRecoveryKit":true}"#,
                            ).unwrap();
                            panel.recovery_services =
                                RecoveryServicesState::Ready(vec![cube.into()]);
                        }
                        if section == "recovery-error-retryable"
                            || section == "recovery-error-terminal"
                        {
                            let retryable = section == "recovery-error-retryable";
                            panel.recovery_services =
                                RecoveryServicesState::Failed(crate::user_error::UserError::new(
                                    "Could not load Cube status",
                                    if retryable {
                                        "Check your connection and try again."
                                    } else {
                                        "Sign in again or contact support."
                                    },
                                    if retryable {
                                        "CC-RECOVERY-TEMP"
                                    } else {
                                        "CC-RECOVERY-AUTH"
                                    },
                                    retryable,
                                ));
                        }
                        if section == "plan-promo-success" || section == "plan-promo-failure" {
                            panel.show_campaign_redeem = true;
                            panel.campaign_redeem.result =
                                Some(if section == "plan-promo-success" {
                                    Ok("Estate unlocked".into())
                                } else {
                                    Err("Code expired".into())
                                });
                        }
                        if section == "account-review" {
                            panel.review_sign_out = true;
                        }
                        if section == "device-review" {
                            panel.review_device_removal = Some(9);
                        }
                        if section == "checkout" {
                            let mut cs = CheckoutState::new(
                                1,
                                PlanTier::Pro,
                                BillingCycle::Monthly,
                                panel.plan.clone(),
                            );
                            cs.phase = CheckoutPhase::AwaitingPayment;
                            cs.checkout = Some(checkout_response());
                            panel.checkout = Some(cs);
                        }
                    }
                }

                let mut renderer = <iced::Renderer as Headless>::new(
                    coincube_ui::font::REGULAR,
                    iced::Pixels(16.0),
                    Some("tiny-skia"),
                )
                .await
                .expect("software renderer for Connect");
                let content = if scenario == "recovery"
                    || scenario == "recovery-error-retryable"
                    || scenario == "recovery-error-terminal"
                {
                    recovery_services_ux(&panel)
                } else {
                    connect_account_panel(&panel)
                };
                let mut element: Element<'_, ConnectAccountMessage> =
                    iced::widget::scrollable(content)
                        .height(Length::Fill)
                        .into();
                let mut tree = Tree::new(element.as_widget());
                let size = iced::Size::new(760.0, 768.0);
                let node = element.as_widget_mut().layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(iced::Size::ZERO, size),
                );
                assert!(
                    node.size().width <= size.width,
                    "{} scroll viewport overflows width",
                    scenario
                );
                assert!(
                    node.size().height <= size.height,
                    "{} scroll viewport overflows height",
                    scenario
                );

                if let Ok(directory) = std::env::var("CONNECT_PREVIEW_DIR") {
                    let viewport = iced::Rectangle::with_size(size);
                    element.as_widget_mut().update(
                        &mut tree,
                        &iced::Event::Window(iced::window::Event::RedrawRequested(
                            std::time::Instant::now(),
                        )),
                        Layout::new(&node),
                        iced::mouse::Cursor::Unavailable,
                        &renderer,
                        &mut iced::advanced::clipboard::Null,
                        &mut iced::advanced::Shell::new(&mut Vec::new()),
                        &viewport,
                    );
                    element.as_widget().draw(
                        &tree,
                        &mut renderer,
                        &theme,
                        &iced::advanced::renderer::Style {
                            text_color: theme.colors.text.primary,
                        },
                        Layout::new(&node),
                        iced::mouse::Cursor::Unavailable,
                        &viewport,
                    );
                    let pixels = renderer.screenshot(
                        iced::Size::new(760, 768),
                        1.0,
                        theme.colors.general.background,
                    );
                    image::save_buffer(
                        std::path::Path::new(&directory)
                            .join(format!("connect-{scenario}-{appearance}-compact.png")),
                        &pixels,
                        760,
                        768,
                        image::ColorType::Rgba8,
                    )
                    .expect("save Connect preview");
                }
            }
        }
    }

    #[tokio::test]
    async fn signed_in_overview_shows_identity_and_benefits_without_other_sections() {
        use iced::advanced::renderer::Headless;

        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let mut account = dashboard_panel();
        account.overview_contact_count = Some(42);
        account.overview_cube_count = Some(84);
        account.recovery_services = RecoveryServicesState::Ready(Vec::new());
        let labels = visible_labels(overview_ux(&account), &renderer);

        for expected in [
            "Account",
            "Signed in on this device",
            "Sign-in method",
            "What Connect provides",
            "Sign out…",
            "Coming soon",
        ] {
            assert!(
                labels.iter().any(|label| label == expected),
                "{}: {:?}",
                expected,
                labels
            );
        }
        for excluded in [
            "Plan",
            "Plan status",
            "Contacts",
            "Cubes",
            "Recovery services",
            "42",
            "84",
        ] {
            assert!(
                !labels.iter().any(|label| label == excluded),
                "{}: {:?}",
                excluded,
                labels
            );
        }
        assert_eq!(connect_benefit_rows().len(), 7);

        account.review_sign_out = true;
        let review = visible_labels(overview_ux(&account), &renderer);
        assert!(review.iter().any(|label| label == "Sign out"));
        assert!(review.iter().any(|label| label == "Cancel"));
    }

    /// Lay out the real sheet at the minimum and reference window sizes.
    /// Child bounds catch broad geometry regressions; emitted PNGs must be
    /// visually checked for glyph clipping, scroll visibility, and overlap.
    #[tokio::test(flavor = "multi_thread")]
    async fn signed_in_account_sheet_layout_and_captures() {
        use iced::advanced::{
            layout,
            renderer::Headless,
            widget::{Operation, Tree},
            Layout,
        };

        #[derive(Default)]
        struct TextBounds(Vec<(String, iced::Rectangle)>);
        impl Operation for TextBounds {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }

            fn text(&mut self, _: Option<&Id>, bounds: iced::Rectangle, value: &str) {
                self.0.push((value.to_owned(), bounds));
            }
        }

        for bytes in [
            coincube_ui::font::REGULAR_BYTES,
            coincube_ui::font::MEDIUM_BYTES,
            coincube_ui::font::BOLD_BYTES,
            coincube_ui::font::BOOTSTRAP_ICONS_BYTE,
        ] {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(std::borrow::Cow::Borrowed(bytes));
        }

        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for (width, height) in [(1024_u32, 768_u32), (1440, 900)] {
                for (long_email, review_sign_out) in
                    [(false, false), (true, false), (false, true), (true, true)]
                {
                    let mut account = dashboard_panel();
                    account.review_sign_out = review_sign_out;
                    if long_email {
                        account.user.as_mut().unwrap().email = "morgan.with-a-long-household-recovery-address@family-office.example.test".into();
                    }
                    let mut renderer = <iced::Renderer as Headless>::new(
                        coincube_ui::font::REGULAR,
                        iced::Pixels(16.0),
                        Some("tiny-skia"),
                    )
                    .await
                    .expect("software renderer");
                    let mut element = connect_sheet(
                        &account,
                        ConnectSheetPage::Sections(ConnectSheetSection::Account),
                    );
                    let mut tree = Tree::new(element.as_widget());
                    let size = iced::Size::new(width as f32, height as f32);
                    let node = element.as_widget_mut().layout(
                        &mut tree,
                        &renderer,
                        &layout::Limits::new(iced::Size::ZERO, size),
                    );
                    assert!(node.size().width <= size.width);
                    assert!(node.size().height <= size.height);
                    let mut bounds = TextBounds::default();
                    element.as_widget_mut().operate(
                        &mut tree,
                        Layout::new(&node),
                        &renderer,
                        &mut bounds,
                    );
                    let action_label = if review_sign_out {
                        "Sign out"
                    } else {
                        "Sign out…"
                    };
                    for label in ["Close", action_label, "Signed in on this device"] {
                        let rect = bounds
                            .0
                            .iter()
                            .find(|(value, _)| value == label)
                            .map(|(_, rect)| rect)
                            .unwrap_or_else(|| panic!("missing {} at {}x{}", label, width, height));
                        assert!(
                            rect.x >= 0.0 && rect.x + rect.width <= size.width,
                            "{} clipped horizontally at {}x{}: {:?}",
                            label,
                            width,
                            height,
                            rect
                        );
                        if label == "Close" {
                            assert!(
                                rect.y >= 0.0 && rect.y + rect.height <= size.height,
                                "Close below viewport at {}x{}: {:?}",
                                width,
                                height,
                                rect
                            );
                        }
                    }
                    if review_sign_out {
                        assert!(bounds.0.iter().any(|(value, _)| value == "Cancel"));
                    }
                    if long_email {
                        let email = account.user.as_ref().unwrap().email.as_str();
                        let email_bounds: Vec<_> = bounds
                            .0
                            .iter()
                            .filter(|(value, _)| value == email)
                            .map(|(_, rect)| rect)
                            .collect();
                        assert!(
                            email_bounds.len() >= 2,
                            "email missing from header or identity"
                        );
                        assert!(
                            email_bounds
                                .iter()
                                .all(|rect| rect.x + rect.width <= size.width),
                            "long email clipped horizontally at {}x{}: {:?}",
                            width,
                            height,
                            email_bounds
                        );
                    }

                    if let Ok(directory) = std::env::var("CONNECT_PREVIEW_DIR") {
                        let viewport = iced::Rectangle::with_size(size);
                        element.as_widget_mut().update(
                            &mut tree,
                            &iced::Event::Window(iced::window::Event::RedrawRequested(
                                std::time::Instant::now(),
                            )),
                            Layout::new(&node),
                            iced::mouse::Cursor::Unavailable,
                            &renderer,
                            &mut iced::advanced::clipboard::Null,
                            &mut iced::advanced::Shell::new(&mut Vec::new()),
                            &viewport,
                        );
                        element.as_widget().draw(
                            &tree,
                            &mut renderer,
                            &theme,
                            &iced::advanced::renderer::Style {
                                text_color: theme.colors.text.primary,
                            },
                            Layout::new(&node),
                            iced::mouse::Cursor::Unavailable,
                            &viewport,
                        );
                        let pixels = renderer.screenshot(
                            iced::Size::new(width, height),
                            1.0,
                            theme.colors.general.background,
                        );
                        let suffix = match (long_email, review_sign_out) {
                            (false, false) => "",
                            (true, false) => "-long-email",
                            (false, true) => "-review",
                            (true, true) => "-long-email-review",
                        };
                        image::save_buffer(
                            std::path::Path::new(&directory).join(format!(
                                "connect-account-{appearance}-{width}x{height}{suffix}.png"
                            )),
                            &pixels,
                            width,
                            height,
                            image::ColorType::Rgba8,
                        )
                        .expect("save Connect account preview");
                    }
                }
            }
        }
    }

    /// The sheet follows the Connect artboard's copy: sentence-case section
    /// A stored session waiting for the network keeps its account in the
    /// sheet, and Connect ending the session leads with the reason (L1, L3).
    #[tokio::test]
    async fn connect_sheet_says_why_the_session_is_waiting_or_ended() {
        use crate::services::connect::grpc::trouble::Trouble;
        use iced::advanced::renderer::Headless;

        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let page = ConnectSheetPage::Sections(ConnectSheetSection::Account);

        let mut waiting = ConnectAccountPanel::new();
        waiting.step = ConnectFlowStep::Login {
            email: String::new(),
            loading: true,
        };
        waiting.unverified = Some(crate::app::state::connect::account::UnverifiedSession {
            email: "alex@example.test".into(),
            trouble: Trouble::Offline,
            attempts: 2,
        });
        let labels = visible_labels(connect_sheet(&waiting, page), &renderer);
        for expected in [
            "Connect account",
            "alex@example.test",
            "No internet connection",
        ] {
            assert!(
                labels.iter().any(|l| l == expected),
                "{}: {:?}",
                expected,
                labels
            );
        }
        assert!(!labels
            .iter()
            .any(|l| l == "Sign in" || l.starts_with("Loading")));

        let mut ended = ConnectAccountPanel::new();
        ended.step = ConnectFlowStep::Login {
            email: String::new(),
            loading: false,
        };
        ended.signed_out_by = Some(crate::app::SignedOutBy::SessionEnded);
        let labels = visible_labels(connect_sheet(&ended, page), &renderer);
        for expected in [
            "Your Connect session on this computer ended",
            "Not now",
            "Sign in",
        ] {
            assert!(
                labels.iter().any(|l| l == expected),
                "{}: {:?}",
                expected,
                labels
            );
        }
        let plain = ConnectAccountPanel::new();
        let labels = visible_labels(connect_sheet(&plain, page), &renderer);
        assert!(!labels.iter().any(|l| l == "Not now"));
    }

    /// titles, the artboard's benefit wording and its list headings.
    #[tokio::test]
    async fn connect_sheet_sections_use_artboard_copy() {
        use iced::advanced::renderer::Headless;

        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let account = dashboard_panel();
        let labels = |section| {
            visible_labels(
                connect_sheet(&account, ConnectSheetPage::Sections(section)),
                &renderer,
            )
        };

        let overview = labels(ConnectSheetSection::Account);
        for expected in [
            "Connect account",
            "Signed in on this device",
            "Sign-in method",
            "What Connect provides",
            "Hosted Esplora server — use your Vault instantly while your node syncs",
            "Sign Vault transactions from your phone with the companion Keychain app",
            "Sign out…",
        ] {
            assert!(
                overview.iter().any(|l| l == expected),
                "{}: {:?}",
                expected,
                overview
            );
        }
        // Duress is listed but not enabled yet; Buy/Sell and P2P are not
        // enabled and not listed (Robert, 2026-10-27).
        assert_eq!(
            overview.iter().filter(|l| *l == "Coming soon").count(),
            1,
            "{:?}",
            overview
        );
        assert!(
            !overview
                .iter()
                .any(|l| l.contains("marketplace") || l.contains("Buy/Sell")),
            "{:?}",
            overview
        );
        // The section list already routes to Plan & billing.
        assert_eq!(
            overview.iter().filter(|l| *l == "Plan & billing").count(),
            1,
            "{:?}",
            overview
        );

        let plan = labels(ConnectSheetSection::PlanBilling);
        assert!(plan.iter().any(|l| l == "Features on this account"));
        assert!(plan.iter().any(|l| l == "Billing history"));
        assert!(!plan.iter().any(|l| l == "Plan & Billing"));

        let security = labels(ConnectSheetSection::DevicesSecurity);
        assert!(security.iter().any(|l| l == "Verified Devices"));
        assert!(security.iter().any(|l| l == "Login Activity"));
    }
}
