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
        menu::ConnectSubMenu,
        state::connect::{ConnectFlowStep, ConnectPanel},
        view::ConnectMessage,
    },
    services::coincube::PlanTier,
};

pub fn connect_panel<'a>(state: &'a ConnectPanel) -> Element<'a, ConnectMessage> {
    let header = Row::new()
        .push(text::h4_bold("COIN").color(color::ORANGE))
        .push(text::h4_bold("CUBE").color(color::WHITE))
        .push(text::h5_regular(" | CONNECT").color(color::GREY_3))
        .align_y(Alignment::Center);

    let body: Element<ConnectMessage> = match &state.step {
        ConnectFlowStep::CheckingSession => Column::new()
            .push(text::p1_regular("Loading…").color(color::GREY_3))
            .align_x(Alignment::Center)
            .into(),

        ConnectFlowStep::Login { email, loading } => login_ux(email, *loading),

        ConnectFlowStep::Register { email, loading } => register_ux(email, *loading),

        ConnectFlowStep::OtpVerification {
            email,
            otp,
            sending,
            cooldown,
            ..
        } => otp_ux(email, otp, *sending, *cooldown),

        ConnectFlowStep::Dashboard => match &state.active_sub {
            ConnectSubMenu::Overview => overview_ux(state),
            ConnectSubMenu::PlanBilling => plan_billing_ux(state),
            ConnectSubMenu::Security => security_ux(state),
            ConnectSubMenu::Duress => duress_ux(),
            ConnectSubMenu::Invites => invites_ux(state),
        },
    };

    let mut col = Column::new()
        .push(header)
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .spacing(0)
        .align_x(Alignment::Start)
        .width(Length::Fill);

    if let Some(e) = state.error.as_deref() {
        col = col.push(
            container(text::p2_regular(e).color(color::RED))
                .padding(8)
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(color::GREY_6)),
                    border: iced::Border {
                        color: color::RED,
                        width: 0.5,
                        radius: 8.0.into(),
                    },
                    ..Default::default()
                }),
        );
    }

    col.push(body).into()
}

fn card_style() -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(color::GREY_6)),
        border: iced::Border {
            color: color::GREY_5,
            width: 0.2,
            radius: 16.0.into(),
        },
        ..Default::default()
    }
}

fn login_ux<'a>(email: &'a str, loading: bool) -> Element<'a, ConnectMessage> {
    let valid = email.contains('.') && email.contains('@') && email.len() >= 5;

    let submit: Element<ConnectMessage> = if loading {
        iced::widget::button(
            container(text::p1_regular("Signing in…").color(color::GREY_3))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fixed(44.0))
        .style(theme::button::primary)
        .into()
    } else {
        button::primary(None, "Continue")
            .on_press_maybe(valid.then_some(ConnectMessage::SubmitLogin))
            .width(Length::Fill)
            .into()
    };

    Column::new()
        .push(text::h3("Sign in to COINCUBE").color(color::WHITE))
        .push(iced::widget::Space::new().height(Length::Fixed(30.0)))
        .push(
            TextInput::new("Email", email)
                .on_input(ConnectMessage::EmailChanged)
                .on_submit_maybe((!loading && valid).then_some(ConnectMessage::SubmitLogin))
                .size(16)
                .padding(15),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(submit)
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(
            iced::widget::button(
                container(text::p2_regular("Don't have an account? Sign up").color(color::BLUE))
                    .padding(5),
            )
            .style(theme::button::link)
            .on_press(ConnectMessage::CreateAccount),
        )
        .align_x(Alignment::Center)
        .spacing(2)
        .max_width(500)
        .width(Length::Fill)
        .into()
}

fn register_ux<'a>(email: &'a str, loading: bool) -> Element<'a, ConnectMessage> {
    let valid = email.contains('.') && email.contains('@') && email.len() >= 5;

    let submit: Element<ConnectMessage> = if loading {
        iced::widget::button(
            container(text::p1_regular("Signing up…").color(color::GREY_3))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fixed(44.0))
        .style(theme::button::primary)
        .into()
    } else {
        button::primary(None, "Continue")
            .on_press_maybe(valid.then_some(ConnectMessage::SubmitRegistration))
            .width(Length::Fill)
            .into()
    };

    Column::new()
        .push(
            iced::widget::button(
                Row::new()
                    .push(previous_icon().color(color::GREY_2))
                    .push(iced::widget::Space::new().width(Length::Fixed(5.0)))
                    .push(text::p1_medium("Previous").color(color::GREY_2))
                    .spacing(5)
                    .align_y(Alignment::Center),
            )
            .style(theme::button::transparent)
            .on_press_maybe((!loading).then_some(ConnectMessage::LogOut)),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(text::h3("Create an Account").color(color::WHITE))
        .push(
            text::p2_regular("Create a COINCUBE account to access Connect and Buy/Sell")
                .color(color::GREY_3),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(
            TextInput::new("Email", email)
                .on_input(ConnectMessage::EmailChanged)
                .on_submit_maybe((!loading && valid).then_some(ConnectMessage::SubmitRegistration))
                .size(16)
                .padding(15),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(submit)
        .align_x(Alignment::Center)
        .spacing(2)
        .max_width(500)
        .width(Length::Fill)
        .into()
}

fn otp_ux<'a>(
    email: &'a str,
    otp: &'a str,
    sending: bool,
    cooldown: u8,
) -> Element<'a, ConnectMessage> {
    let valid = otp.len() == 6;

    let submit: Element<ConnectMessage> = if sending {
        iced::widget::button(
            container(text::p1_regular("Verifying…").color(color::GREY_3))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fixed(44.0))
        .style(theme::button::primary)
        .into()
    } else {
        button::primary(None, "Verify OTP")
            .on_press_maybe(valid.then_some(ConnectMessage::VerifyOtp))
            .width(Length::Fill)
            .into()
    };

    let sent_to = format!("We sent a code to {email}");
    let resend_label = if cooldown > 0 {
        format!("Resend OTP ({cooldown}s)")
    } else {
        "Resend OTP".to_string()
    };
    let can_resend = cooldown == 0 && !sending;

    Column::new()
        .push(text::h3("Enter your OTP").color(color::WHITE))
        .push(text::p2_regular(sent_to).color(color::GREY_3))
        .push(iced::widget::Space::new().height(Length::Fixed(25.0)))
        .push(
            TextInput::new("6-digit code", otp)
                .on_input(ConnectMessage::OtpChanged)
                .on_submit_maybe((!sending && valid).then_some(ConnectMessage::VerifyOtp))
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
            .on_press_maybe(can_resend.then_some(ConnectMessage::SubmitLogin)),
        )
        .align_x(Alignment::Center)
        .spacing(2)
        .max_width(500)
        .width(Length::Fill)
        .into()
}

fn overview_ux<'a>(state: &'a ConnectPanel) -> Element<'a, ConnectMessage> {
    let email = state.user.as_ref().map(|u| u.email.as_str()).unwrap_or("—");
    let verified = state
        .user
        .as_ref()
        .and_then(|u| u.email_verified)
        .unwrap_or(false);
    let plan_label = state
        .plan
        .as_ref()
        .map(|p| p.tier.to_string())
        .unwrap_or_else(|| "Free".to_string());

    let verification_badge: Element<ConnectMessage> = if verified {
        text::p2_regular("✓ Verified").color(color::ORANGE).into()
    } else {
        text::p2_regular("✗ Unverified").color(color::GREY_3).into()
    };

    Column::new()
        .push(text::h4_bold("Account Overview").color(color::WHITE))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(
            container(
                Column::new()
                    .push(
                        Row::new()
                            .push(text::p1_medium("Email").color(color::GREY_3))
                            .push(iced::widget::Space::new().width(Length::Fill))
                            .push(text::p1_regular(email).color(color::WHITE))
                            .align_y(Alignment::Center),
                    )
                    .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                    .push(
                        Row::new()
                            .push(text::p1_medium("Status").color(color::GREY_3))
                            .push(iced::widget::Space::new().width(Length::Fill))
                            .push(verification_badge)
                            .align_y(Alignment::Center),
                    )
                    .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                    .push(
                        Row::new()
                            .push(text::p1_medium("Plan").color(color::GREY_3))
                            .push(iced::widget::Space::new().width(Length::Fill))
                            .push(text::p1_bold(plan_label).color(color::ORANGE))
                            .align_y(Alignment::Center),
                    )
                    .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
                    .push(
                        button::secondary(Some(cross_icon()), "Sign Out")
                            .on_press(ConnectMessage::LogOut),
                    )
                    .padding(20)
                    .spacing(2),
            )
            .style(|_| container::Style {
                background: Some(iced::Background::Color(color::GREY_6)),
                border: iced::Border {
                    color: color::ORANGE,
                    width: 0.2,
                    radius: 20.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fill),
        )
        .spacing(0)
        .width(Length::Fill)
        .into()
}

fn plan_tier_color(tier: &PlanTier) -> iced::Color {
    match tier {
        PlanTier::Free => color::GREY_3,
        PlanTier::Pro => color::ORANGE,
        PlanTier::Legacy => color::BLUE,
    }
}

fn plan_billing_ux<'a>(state: &'a ConnectPanel) -> Element<'a, ConnectMessage> {
    let current_tier = state
        .plan
        .as_ref()
        .map(|p| &p.tier)
        .unwrap_or(&PlanTier::Free);

    let plan_card = |name: &'static str,
                     tier: &PlanTier,
                     desc: &'static str,
                     price: &'static str|
     -> Element<'a, ConnectMessage> {
        let is_current = tier == current_tier;
        let badge_color = plan_tier_color(tier);

        container(
            Column::new()
                .push(
                    Row::new()
                        .push(text::p1_bold(name).color(badge_color))
                        .push(iced::widget::Space::new().width(Length::Fill))
                        .push(text::p1_regular(price).color(color::GREY_3)),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
                .push(text::p2_regular(desc).color(color::GREY_3))
                .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                .push(if is_current {
                    button::secondary(None, "Current Plan").width(Length::Fill)
                } else {
                    button::primary(None, "Upgrade (Coming Soon)").width(Length::Fill)
                })
                .padding(16)
                .spacing(2),
        )
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(color::GREY_6)),
            border: iced::Border {
                color: if is_current {
                    badge_color
                } else {
                    color::GREY_5
                },
                width: if is_current { 1.0 } else { 0.2 },
                radius: 16.0.into(),
            },
            ..Default::default()
        })
        .width(Length::Fill)
        .into()
    };

    Column::new()
        .push(text::h4_bold("Plan & Billing").color(color::WHITE))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(plan_card(
            "Free",
            &PlanTier::Free,
            "Core features: Liquid wallet, Buy/Sell",
            "Free",
        ))
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(plan_card(
            "Pro",
            &PlanTier::Pro,
            "Advanced policy templates, priority support",
            "Coming Soon",
        ))
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(plan_card(
            "Legacy",
            &PlanTier::Legacy,
            "Full feature access including Invites and Duress",
            "Coming Soon",
        ))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(
            container(
                text::p2_regular(
                    "Paid plans will be available via Bitcoin / Lightning (OpenNode). \
                     No subscriptions — pay upfront, auto-renew reminders sent by email.",
                )
                .color(color::GREY_3),
            )
            .padding(12)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(color::GREY_6)),
                border: iced::Border {
                    radius: 10.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .width(Length::Fill),
        )
        .spacing(0)
        .width(Length::Fill)
        .into()
}

fn security_ux<'a>(state: &'a ConnectPanel) -> Element<'a, ConnectMessage> {
    let devices_section: Element<ConnectMessage> = match &state.verified_devices {
        None => text::p2_regular("Loading devices…")
            .color(color::GREY_3)
            .into(),
        Some(devices) if devices.is_empty() => text::p2_regular("No verified devices on record.")
            .color(color::GREY_3)
            .into(),
        Some(devices) => {
            let mut col = Column::new().spacing(6);
            for d in devices {
                let name = d.device_name.as_deref().unwrap_or("Unknown Device");
                let suffix = if d.is_current { " (this device)" } else { "" };
                let label = format!("{name}{suffix}");
                col = col.push(
                    Row::new()
                        .push(text::p2_regular(label).color(color::WHITE))
                        .push(iced::widget::Space::new().width(Length::Fill))
                        .push(text::p2_regular(d.created_at.as_str()).color(color::GREY_3)),
                );
            }
            col.into()
        }
    };

    let activity_section: Element<ConnectMessage> = match &state.login_activity {
        None => text::p2_regular("Loading activity…")
            .color(color::GREY_3)
            .into(),
        Some(activity) if activity.is_empty() => text::p2_regular("No login activity on record.")
            .color(color::GREY_3)
            .into(),
        Some(activity) => {
            let mut col = Column::new().spacing(6);
            for a in activity.iter().take(10) {
                let status = if a.success { "✓" } else { "✗" };
                let status_color = if a.success { color::ORANGE } else { color::RED };
                let ip = a.ip_address.as_deref().unwrap_or("unknown").to_string();
                col = col.push(
                    Row::new()
                        .push(text::p2_regular(status).color(status_color))
                        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
                        .push(text::p2_regular(ip).color(color::WHITE))
                        .push(iced::widget::Space::new().width(Length::Fill))
                        .push(text::p2_regular(a.created_at.as_str()).color(color::GREY_3)),
                );
            }
            col.into()
        }
    };

    Column::new()
        .push(text::h4_bold("Security").color(color::WHITE))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(
            container(
                Column::new()
                    .push(text::p1_bold("Verified Devices").color(color::WHITE))
                    .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
                    .push(devices_section)
                    .padding(16)
                    .spacing(2),
            )
            .style(|_| card_style())
            .width(Length::Fill),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
        .push(
            container(
                Column::new()
                    .push(text::p1_bold("Login Activity").color(color::WHITE))
                    .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
                    .push(activity_section)
                    .padding(16)
                    .spacing(2),
            )
            .style(|_| card_style())
            .width(Length::Fill),
        )
        .spacing(0)
        .width(Length::Fill)
        .into()
}

fn duress_ux<'a>() -> Element<'a, ConnectMessage> {
    Column::new()
        .push(text::h4_bold("Duress Settings").color(color::WHITE))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(
            container(
                Column::new()
                    .push(
                        text::p1_regular(
                            "Duress protection allows you to lock signing and optionally wipe \
                             local data under coercion. Configure trusted contacts and escalation \
                             rules.",
                        )
                        .color(color::GREY_3),
                    )
                    .push(iced::widget::Space::new().height(Length::Fixed(16.0)))
                    .push(
                        text::p2_regular("Coming Soon — requires backend endpoint /connect/duress")
                            .color(color::GREY_3),
                    )
                    .padding(20)
                    .spacing(2),
            )
            .style(|_| card_style())
            .width(Length::Fill),
        )
        .spacing(0)
        .width(Length::Fill)
        .into()
}

fn invites_ux<'a>(state: &'a ConnectPanel) -> Element<'a, ConnectMessage> {
    let is_legacy = state
        .plan
        .as_ref()
        .map(|p| p.tier == PlanTier::Legacy)
        .unwrap_or(false);

    let card_content: Element<ConnectMessage> = if !is_legacy {
        container(
            Column::new()
                .push(text::p1_bold("Legacy Plan Required").color(color::WHITE))
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    text::p2_regular(
                        "Invites are available on the Legacy plan. Upgrade to share \
                         COINCUBE access with trusted contacts.",
                    )
                    .color(color::GREY_3),
                )
                .padding(16)
                .spacing(2),
        )
        .style(|_| card_style())
        .width(Length::Fill)
        .into()
    } else {
        container(
            Column::new()
                .push(
                    text::p2_regular("Coming Soon — requires backend endpoint /connect/invites")
                        .color(color::GREY_3),
                )
                .padding(16)
                .spacing(2),
        )
        .style(|_| card_style())
        .width(Length::Fill)
        .into()
    };

    Column::new()
        .push(text::h4_bold("Invites").color(color::WHITE))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(card_content)
        .spacing(0)
        .width(Length::Fill)
        .into()
}
