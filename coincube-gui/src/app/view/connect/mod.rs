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
        state::connect::{AvatarFlowStep, ConnectFlowStep, ConnectPanel},
        view::{AvatarMessage, ConnectMessage},
    },
    services::coincube::{
        AvatarAccentMotif, AvatarAgeFeel, AvatarArchetype, AvatarArmorStyle, AvatarDemeanor,
        AvatarGender, PlanTier,
    },
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
            ConnectSubMenu::LightningAddress => lightning_address_ux(state),
            ConnectSubMenu::Avatar => avatar_ux(state),
            ConnectSubMenu::PlanBilling => plan_billing_ux(state),
            ConnectSubMenu::Security => security_ux(state),
            ConnectSubMenu::Duress => duress_ux(),
            ConnectSubMenu::Invites => invites_ux(state),
        },
    };

    let is_auth_step = !matches!(state.step, ConnectFlowStep::Dashboard);
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
                        button::secondary( None,"Sign Out")
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
                let ok = a.success.unwrap_or(false);
                let status = if ok { "✓" } else { "✗" };
                let status_color = if ok { color::ORANGE } else { color::RED };
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

fn lightning_address_ux<'a>(state: &'a ConnectPanel) -> Element<'a, ConnectMessage> {
    let has_address = state
        .lightning_address
        .as_ref()
        .and_then(|la| la.lightning_address.as_deref())
        .is_some();

    let card_content: Element<ConnectMessage> = if has_address {
        // Display the claimed address
        let address = state
            .lightning_address
            .as_ref()
            .and_then(|la| la.lightning_address.clone())
            .unwrap_or_default();

        container(
            Column::new()
                .push(text::p1_bold("Your Lightning Address").color(color::WHITE))
                .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                .push(
                    container(
                        Row::new()
                            .push(text::h3(address.clone()).color(color::ORANGE))
                            .push(iced::widget::Space::new().width(Length::Fill))
                            .push(
                                button::secondary(Some(clipboard_icon()), "Copy")
                                    .on_press(ConnectMessage::CopyToClipboard(address)),
                            )
                            .align_y(Alignment::Center),
                    )
                    .padding(16)
                    .style(|_| container::Style {
                        background: Some(iced::Background::Color(color::GREY_7)),
                        border: iced::Border {
                            color: color::ORANGE,
                            width: 0.5,
                            radius: 12.0.into(),
                        },
                        ..Default::default()
                    })
                    .width(Length::Fill),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    text::p2_regular(
                        "Anyone can send you bitcoin using this address. \
                         It works with any wallet that supports BOLT12 / BIP353.",
                    )
                    .color(color::GREY_3),
                )
                .padding(20)
                .spacing(2),
        )
        .style(|_| card_style())
        .width(Length::Fill)
        .into()
    } else {
        // Claim form
        let username = &state.ln_username_input;
        let is_valid = state.ln_username_error.is_none() && !username.is_empty();
        let is_available = state.ln_username_available == Some(true);
        let can_claim = is_valid && is_available && !state.ln_claiming;

        // Status indicator
        let status: Element<ConnectMessage> = if state.ln_checking {
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

        let claim_button: Element<ConnectMessage> = if state.ln_claiming {
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
                .on_press_maybe(can_claim.then_some(ConnectMessage::ClaimLightningAddress))
                .width(Length::Fill)
                .into()
        };

        container(
            Column::new()
                .push(text::p1_bold("Claim Your Lightning Address").color(color::WHITE))
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
                                .on_input(ConnectMessage::LnUsernameChanged)
                                .on_submit_maybe(
                                    can_claim.then_some(ConnectMessage::ClaimLightningAddress),
                                )
                                .size(16)
                                .padding(15),
                        )
                        .push(
                            container(text::p1_regular("@coincube.io").color(color::GREY_3))
                                .padding(15)
                                .center_y(Length::Fixed(50.0)),
                        )
                        .align_y(Alignment::Center),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
                .push(status)
                .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
                .push(claim_button)
                .padding(20)
                .spacing(2),
        )
        .style(|_| card_style())
        .width(Length::Fill)
        .into()
    };

    Column::new()
        .push(text::h4_bold("Lightning Address").color(color::WHITE))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(card_content)
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

fn avatar_ux<'a>(state: &'a ConnectPanel) -> Element<'a, ConnectMessage> {
    let title = Row::new()
        .push(text::h4_bold("Avatar").color(color::WHITE))
        .push(iced::widget::Space::new().width(Length::Fill))
        .align_y(Alignment::Center);

    let body: Element<ConnectMessage> = match &state.avatar_step {
        AvatarFlowStep::Idle | AvatarFlowStep::Questionnaire => avatar_questionnaire_ux(state),
        AvatarFlowStep::Generating => container(
            Column::new()
                .push(text::p1_bold("Forging your identity…").color(color::ORANGE))
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    text::p2_regular(
                        "Generating your sumi-e avatar. This may take up to 30 seconds.",
                    )
                    .color(color::GREY_3),
                )
                .padding(24)
                .spacing(4),
        )
        .style(|_| card_style())
        .width(Length::Fill)
        .into(),

        AvatarFlowStep::Reveal | AvatarFlowStep::Settings => avatar_settings_ux(state),
    };

    if let Some(err) = state.avatar_error.as_deref() {
        return Column::new()
            .push(title)
            .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
            .push(
                container(
                    Column::new()
                        .push(text::p1_bold("Generation Failed").color(color::RED))
                        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                        .push(text::p2_regular(err).color(color::GREY_3))
                        .push(iced::widget::Space::new().height(Length::Fixed(16.0)))
                        .push(
                            button::primary(None, "Try Again")
                                .on_press(ConnectMessage::Avatar(AvatarMessage::Retry)),
                        )
                        .padding(16)
                        .spacing(4),
                )
                .style(|_| card_style())
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

fn avatar_questionnaire_ux<'a>(state: &'a ConnectPanel) -> Element<'a, ConnectMessage> {
    let draft = &state.avatar_draft;

    let gender_row = Row::new()
        .push(
            text::p2_regular("Gender")
                .color(color::GREY_3)
                .width(Length::Fixed(110.0)),
        )
        .push(
            if draft.gender == AvatarGender::Man {
                button::primary(None, "Man")
            } else {
                button::secondary(None, "Man")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::GenderChanged(
                AvatarGender::Man,
            ))),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.gender == AvatarGender::Woman {
                button::primary(None, "Woman")
            } else {
                button::secondary(None, "Woman")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::GenderChanged(
                AvatarGender::Woman,
            ))),
        )
        .align_y(Alignment::Center);

    let archetype_row = Row::new()
        .push(
            text::p2_regular("Archetype")
                .color(color::GREY_3)
                .width(Length::Fixed(110.0)),
        )
        .push(
            if draft.archetype == AvatarArchetype::Ronin {
                button::primary(None, "Ronin")
            } else {
                button::secondary(None, "Ronin")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::ArchetypeChanged(
                AvatarArchetype::Ronin,
            ))),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.archetype == AvatarArchetype::Samurai {
                button::primary(None, "Samurai")
            } else {
                button::secondary(None, "Samurai")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::ArchetypeChanged(
                AvatarArchetype::Samurai,
            ))),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.archetype == AvatarArchetype::Shogun {
                button::primary(None, "Shogun")
            } else {
                button::secondary(None, "Shogun")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::ArchetypeChanged(
                AvatarArchetype::Shogun,
            ))),
        )
        .align_y(Alignment::Center);

    let age_row = Row::new()
        .push(
            text::p2_regular("Age Feel")
                .color(color::GREY_3)
                .width(Length::Fixed(110.0)),
        )
        .push(
            if draft.age_feel == AvatarAgeFeel::Young {
                button::primary(None, "Young")
            } else {
                button::secondary(None, "Young")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::AgeFeelChanged(
                AvatarAgeFeel::Young,
            ))),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.age_feel == AvatarAgeFeel::Mature {
                button::primary(None, "Mature")
            } else {
                button::secondary(None, "Mature")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::AgeFeelChanged(
                AvatarAgeFeel::Mature,
            ))),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.age_feel == AvatarAgeFeel::Elder {
                button::primary(None, "Elder")
            } else {
                button::secondary(None, "Elder")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::AgeFeelChanged(
                AvatarAgeFeel::Elder,
            ))),
        )
        .align_y(Alignment::Center);

    let demeanor_row = Row::new()
        .push(
            text::p2_regular("Demeanor")
                .color(color::GREY_3)
                .width(Length::Fixed(110.0)),
        )
        .push(
            if draft.demeanor == AvatarDemeanor::Calm {
                button::primary(None, "Calm")
            } else {
                button::secondary(None, "Calm")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::DemeanorChanged(
                AvatarDemeanor::Calm,
            ))),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.demeanor == AvatarDemeanor::Fierce {
                button::primary(None, "Fierce")
            } else {
                button::secondary(None, "Fierce")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::DemeanorChanged(
                AvatarDemeanor::Fierce,
            ))),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.demeanor == AvatarDemeanor::Mysterious {
                button::primary(None, "Mysterious")
            } else {
                button::secondary(None, "Mysterious")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::DemeanorChanged(
                AvatarDemeanor::Mysterious,
            ))),
        )
        .align_y(Alignment::Center);

    let armor_row = Row::new()
        .push(
            text::p2_regular("Armor")
                .color(color::GREY_3)
                .width(Length::Fixed(110.0)),
        )
        .push(
            if draft.armor_style == AvatarArmorStyle::Light {
                button::primary(None, "Light")
            } else {
                button::secondary(None, "Light")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::ArmorStyleChanged(
                AvatarArmorStyle::Light,
            ))),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.armor_style == AvatarArmorStyle::Standard {
                button::primary(None, "Standard")
            } else {
                button::secondary(None, "Standard")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::ArmorStyleChanged(
                AvatarArmorStyle::Standard,
            ))),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.armor_style == AvatarArmorStyle::Heavy {
                button::primary(None, "Heavy")
            } else {
                button::secondary(None, "Heavy")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::ArmorStyleChanged(
                AvatarArmorStyle::Heavy,
            ))),
        )
        .align_y(Alignment::Center);

    let motif_row = Row::new()
        .push(
            text::p2_regular("Accent")
                .color(color::GREY_3)
                .width(Length::Fixed(110.0)),
        )
        .push(
            if draft.accent_motif == AvatarAccentMotif::OrangeSun {
                button::primary(None, "Sun")
            } else {
                button::secondary(None, "Sun")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::AccentMotifChanged(
                AvatarAccentMotif::OrangeSun,
            ))),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.accent_motif == AvatarAccentMotif::Splatter {
                button::primary(None, "Splatter")
            } else {
                button::secondary(None, "Splatter")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::AccentMotifChanged(
                AvatarAccentMotif::Splatter,
            ))),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.accent_motif == AvatarAccentMotif::Seal {
                button::primary(None, "Seal")
            } else {
                button::secondary(None, "Seal")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::AccentMotifChanged(
                AvatarAccentMotif::Seal,
            ))),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.accent_motif == AvatarAccentMotif::Calligraphy {
                button::primary(None, "Calligraphy")
            } else {
                button::secondary(None, "Calligraphy")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::AccentMotifChanged(
                AvatarAccentMotif::Calligraphy,
            ))),
        )
        .align_y(Alignment::Center);

    let laser_row = Row::new()
        .push(
            text::p2_regular("Laser Eyes")
                .color(color::GREY_3)
                .width(Length::Fixed(110.0)),
        )
        .push(
            if draft.laser_eyes {
                button::primary(None, "On")
            } else {
                button::secondary(None, "Off")
            }
            .on_press(ConnectMessage::Avatar(AvatarMessage::LaserEyesToggled(
                !draft.laser_eyes,
            ))),
        )
        .align_y(Alignment::Center);

    let has_ln = state.lightning_address.is_some();
    let generate_btn = if has_ln {
        button::primary(None, "Generate Avatar")
            .on_press(ConnectMessage::Avatar(AvatarMessage::Generate))
    } else {
        button::primary(None, "Set Lightning Address First")
    };

    container(
        Column::new()
            .push(text::p1_bold("Choose Your Traits").color(color::WHITE))
            .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
            .push(text::p2_regular("Your selections, combined with a hash of your Lightning address, create your unique avatar.").color(color::GREY_3))
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
    .style(|_| card_style())
    .width(Length::Fill)
    .into()
}

fn avatar_settings_ux<'a>(state: &'a ConnectPanel) -> Element<'a, ConnectMessage> {
    let Some(ref data) = state.avatar_data else {
        return container(text::p2_regular("Loading avatar data…").color(color::GREY_3))
            .padding(16)
            .style(|_| card_style())
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

    let image_widget: Element<ConnectMessage> = if let Some(id) = active_id {
        if let Some((_, handle)) = state.avatar_image_cache.get(&id) {
            iced::widget::image(handle.clone())
                .width(Length::Fixed(200.0))
                .height(Length::Fixed(200.0))
                .into()
        } else {
            text::p2_regular("Loading image…")
                .color(color::GREY_3)
                .into()
        }
    } else {
        text::p2_regular("No active variant")
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
    let variant_row: Element<ConnectMessage> = if data.variants.len() > 1 {
        let mut row = Row::new().spacing(8);
        for v in &data.variants {
            let is_active = active_url.ends_with(&v.id.to_string());
            let vid = v.id;
            let thumb: Element<ConnectMessage> =
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
                            .on_press(ConnectMessage::Avatar(AvatarMessage::SelectVariant(vid)))
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
                            .style(|_| card_style()),
                    )
                    .on_press(ConnectMessage::Avatar(AvatarMessage::SelectVariant(vid)))
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
        button::primary(None, "No Regenerations Remaining")
    } else {
        button::primary(None, "Regenerate Avatar").on_press(ConnectMessage::Avatar(
            AvatarMessage::SetStep(AvatarFlowStep::Questionnaire),
        ))
    };

    let download_btn = button::secondary(None, "Download PNG")
        .on_press(ConnectMessage::Avatar(AvatarMessage::DownloadAvatar));

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
    .style(|_| card_style())
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
