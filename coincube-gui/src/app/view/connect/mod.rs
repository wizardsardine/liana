mod contacts;
pub mod cube_members;
pub mod duress_enroll;
pub mod sign_in_prompt;

use coincube_ui::{
    color,
    component::{button, text},
    icon::*,
    image::coincube_wordmark,
    theme,
    widget::*,
};
use iced::{
    widget::{container, QRCode},
    Alignment, Length,
};

use crate::{
    app::{
        menu::ConnectSubMenu,
        settings::global::AccountTier,
        state::connect::{
            AvatarFlowStep, CheckoutPhase, ConnectAccountPanel, ConnectCubePanel, ConnectFlowStep,
            ConnectPanel, PlanLifecycle,
        },
        view::{AvatarMessage, ConnectAccountMessage, ConnectCubeMessage, DuressMessage},
    },
    services::coincube::{
        AvatarAccentMotif, AvatarAgeFeel, AvatarArchetype, AvatarArmorStyle, AvatarDemeanor,
        AvatarGender, BillingCycle, ConnectPlan, PlanTier,
    },
};

use crate::app::view::Message as ViewMessage;

pub fn connect_panel<'a>(state: &'a ConnectPanel) -> Element<'a, ViewMessage> {
    let acct = &state.account;

    let header = Row::new()
        .push(coincube_wordmark::<ViewMessage>(20.0))
        .push(text::h5_regular(" | CONNECT").color(color::GREY_3))
        .align_y(Alignment::Center);

    let body: Element<ViewMessage> = match &acct.step {
        ConnectFlowStep::CheckingSession => Column::new()
            .push(text::p1_regular("Loading…").color(color::GREY_3))
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

        ConnectFlowStep::CheckingDuress { failed } => {
            checking_duress_ux(*failed).map(ViewMessage::ConnectAccount)
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

    if let Some(e) = acct.error.as_deref() {
        col = col.push(
            container(text::p2_regular(e).color(color::RED))
                .padding(8)
                .style(|t| container::Style {
                    background: Some(iced::Background::Color(t.colors.cards.simple.background)),
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

/// Renders account-level Connect views (used by the home).
/// Returns Element<ConnectAccountMessage> (not ViewMessage) so the caller can map it.
pub fn connect_account_panel<'a>(
    acct: &'a ConnectAccountPanel,
) -> Element<'a, ConnectAccountMessage> {
    let header = Row::new()
        .push(coincube_wordmark::<ConnectAccountMessage>(20.0))
        .push(text::h5_regular(" | CONNECT").color(color::GREY_3))
        .align_y(Alignment::Center);

    let body: Element<ConnectAccountMessage> = match &acct.step {
        ConnectFlowStep::CheckingSession => Column::new()
            .push(text::p1_regular("Loading…").color(color::GREY_3))
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
        ConnectFlowStep::CheckingDuress { failed } => checking_duress_ux(*failed),
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
        .push(header)
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .spacing(0)
        .align_x(col_align)
        .width(Length::Fill);

    if let Some(e) = acct.error.as_deref() {
        col = col.push(
            container(text::p2_regular(e).color(color::RED))
                .padding(8)
                .style(|t| container::Style {
                    background: Some(iced::Background::Color(t.colors.cards.simple.background)),
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

/// Parse an RFC 3339 timestamp and render it as `"Mon DD, YYYY"`
/// (e.g. `"Apr 20, 2026"`). Falls back to the raw input on parse
/// failure. Shared helper used by both the Contacts and Cube Members
/// views.
pub(super) fn format_date(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|dt| dt.format("%b %d, %Y").to_string())
        .unwrap_or_else(|_| iso.to_string())
}

/// Parse an RFC 3339 timestamp and render it as `"Mon DD, YYYY HH:MM"`
/// (e.g. `"Apr 20, 2026 14:31"`). Falls back to the raw input on parse failure.
pub(super) fn format_datetime(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%b %d, %Y %H:%M %Z")
                .to_string()
        })
        .unwrap_or_else(|_| iso.to_string())
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

fn login_ux<'a>(email: &'a str, loading: bool) -> Element<'a, ConnectAccountMessage> {
    let valid = email.contains('.') && email.contains('@') && email.len() >= 5;

    let submit: Element<ConnectAccountMessage> = if loading {
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
            .on_press_maybe(valid.then_some(ConnectAccountMessage::SubmitLogin))
            .width(Length::Fill)
            .into()
    };

    Column::new()
        .push(text::h3("Sign in to COINCUBE").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(30.0)))
        .push(
            TextInput::new("Email", email)
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
                container(text::p2_regular("Don't have an account? Sign up").color(color::ORANGE))
                    .padding(5),
            )
            .style(theme::button::link)
            .on_press(ConnectAccountMessage::CreateAccount),
        )
        .align_x(Alignment::Center)
        .spacing(2)
        .max_width(500)
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
            .on_press_maybe(valid.then_some(ConnectAccountMessage::SubmitRegistration))
            .width(Length::Fill)
            .into()
    };

    Column::new()
        .push(
            button::secondary(Some(previous_icon()), "Back")
                .width(Length::Fixed(150.0))
                .on_press_maybe((!loading).then_some(ConnectAccountMessage::LogOut)),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(text::h3("Create an Account").style(theme::text::primary))
        .push(
            text::p2_regular("Create a COINCUBE account to access Connect and Buy/Sell")
                .color(color::GREY_3),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(
            TextInput::new("Email", email)
                .on_input(ConnectAccountMessage::EmailChanged)
                .on_submit_maybe(
                    (!loading && valid).then_some(ConnectAccountMessage::SubmitRegistration),
                )
                .size(16)
                .padding(15),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
        // Optional promo/referral code (server-driven campaign engine, v2).
        // Redeemed automatically once the new account authenticates.
        .push(
            TextInput::new("Promo or referral code (optional)", code)
                .on_input(ConnectAccountMessage::RegisterCampaignCodeChanged)
                .on_submit_maybe(
                    (!loading && valid).then_some(ConnectAccountMessage::SubmitRegistration),
                )
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
) -> Element<'a, ConnectAccountMessage> {
    let valid = otp.len() == 6;

    let submit: Element<ConnectAccountMessage> = if sending {
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
            .on_press_maybe(valid.then_some(ConnectAccountMessage::VerifyOtp))
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
        .push(text::h3("Enter your OTP").style(theme::text::primary))
        .push(text::p2_regular(sent_to).color(color::GREY_3))
        .push(iced::widget::Space::new().height(Length::Fixed(25.0)))
        .push(
            TextInput::new("6-digit code", otp)
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
        .align_x(Alignment::Center)
        .spacing(2)
        .max_width(500)
        .width(Length::Fill)
        .into()
}

fn overview_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let email = state.user.as_ref().map(|u| u.email.as_str()).unwrap_or("—");
    let verified = state
        .user
        .as_ref()
        .and_then(|u| u.email_verified)
        .unwrap_or(false);
    let plan_label = state
        .plan
        .as_ref()
        .map(|p| p.tier().to_string())
        .unwrap_or_else(|| "Free".to_string());

    let verification_badge: Element<ConnectAccountMessage> = if verified {
        text::p2_regular("✓ Verified").color(color::ORANGE).into()
    } else {
        text::p2_regular("✗ Unverified").color(color::GREY_3).into()
    };

    let mut col = Column::new()
        .push(text::h4_bold("Account Overview").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)));

    // Pre-expiry renewal reminder (D1) / expired prompt (D3).
    if let Some(banner) = renewal_banner(state) {
        col = col
            .push(banner)
            .push(iced::widget::Space::new().height(Length::Fixed(12.0)));
    }

    col.push(
        container(
            Column::new()
                .push(
                    Row::new()
                        .push(text::p1_medium("Email").color(color::GREY_3))
                        .push(iced::widget::Space::new().width(Length::Fill))
                        .push(text::p1_regular(email).style(theme::text::primary))
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
                .push(button::secondary(None, "Sign Out").on_press(ConnectAccountMessage::LogOut))
                .padding(20)
                .spacing(2),
        )
        .style(|t| container::Style {
            background: Some(iced::Background::Color(t.colors.cards.simple.background)),
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
        PlanTier::Estate => color::LIGHT_BLUE,
    }
}

fn cube_limit_for(tier: &PlanTier) -> usize {
    match tier {
        PlanTier::Free => AccountTier::Free.cube_limit(),
        PlanTier::Pro => AccountTier::Pro.cube_limit(),
        PlanTier::Estate => AccountTier::Estate.cube_limit(),
    }
}

// ── Renewal reminder / expired prompt banner (D1 / D3) ──────────────────────

/// Pre-expiry renewal reminder (D1) or, for a lapsed plan, an expired
/// prompt (D3). Returns `None` when the plan is comfortably active or free.
/// Shared by the account overview and the plan view; the renewal CTA opens
/// checkout pre-selected to the current tier + cycle, while the expired CTA
/// opens the picker.
///
/// The per-session dismissal applies *only* to the pre-expiry reminder —
/// the expired prompt is not dismissible, so dismissing the reminder and
/// then lapsing in the same session still surfaces the expired state.
fn renewal_banner<'a>(
    state: &'a ConnectAccountPanel,
) -> Option<Element<'a, ConnectAccountMessage>> {
    // `dismissible` gates the Dismiss control: only the pre-expiry reminder
    // honours `renewal_banner_dismissed`, so the expired prompt omits the
    // button entirely rather than rendering a no-op.
    let (title, accent, copy, cta_label, cta_msg, dismissible): (
        &str,
        iced::Color,
        String,
        &str,
        ConnectAccountMessage,
        bool,
    ) = match state.plan_lifecycle() {
        PlanLifecycle::RenewalDue { .. } => {
            // Folds in per-session dismissal plus the purchasing-disabled
            // suppression (PLAN-estate-promo PR2) — when there's no purchase
            // path, the pre-expiry "Renew" reminder is hidden so it never
            // becomes a dead-end CTA. The Expired arm below is navigation-only
            // ("View plans"), so it stays unsuppressed.
            if !state.show_renewal_banner() {
                return None;
            }
            let plan = state.plan.as_ref()?;
            let date = plan
                .renewal_at
                .as_deref()
                .map(format_date)
                .unwrap_or_else(|| "soon".to_string());
            (
                "Renewal reminder",
                color::ORANGE,
                format!(
                    "Your {} plan renews on {}. Renew now to keep your features.",
                    plan.tier(),
                    date
                ),
                "Renew",
                ConnectAccountMessage::RenewCurrentPlan,
                true,
            )
        }
        PlanLifecycle::Expired => {
            let copy = match state
                .plan
                .as_ref()
                .and_then(|p| p.renewal_at.as_deref())
                .map(format_date)
            {
                Some(d) => format!(
                    "Your plan expired on {} — renew to restore premium features.",
                    d
                ),
                None => "Your plan expired — renew to restore premium features.".to_string(),
            };
            (
                "Plan expired",
                color::RED,
                copy,
                // Routes to the picker, not a direct checkout — a lapsed
                // plan reports as Free, so the prior tier is gone and there's
                // nothing to pre-fill an invoice with. Label reflects that.
                "View plans",
                ConnectAccountMessage::OpenPlanBilling,
                false,
            )
        }
        PlanLifecycle::Active | PlanLifecycle::Free => return None,
    };

    let mut actions = Column::new().push(button::primary(None, cta_label).on_press(cta_msg));
    if dismissible {
        actions = actions
            .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
            .push(
                button::secondary(None, "Dismiss")
                    .on_press(ConnectAccountMessage::DismissRenewalBanner),
            );
    }

    let body = Row::new()
        .push(
            Column::new()
                .push(text::p2_bold(title).color(accent))
                .push(iced::widget::Space::new().height(Length::Fixed(2.0)))
                .push(text::p2_regular(copy).style(theme::text::primary))
                .width(Length::Fill),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(10.0)))
        .push(actions.align_x(Alignment::End))
        .align_y(Alignment::Center);

    Some(
        container(body)
            .padding(14)
            .width(Length::Fill)
            .style(move |t| container::Style {
                background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                border: iced::Border {
                    color: accent,
                    width: 1.0,
                    radius: 14.0.into(),
                },
                ..Default::default()
            })
            .into(),
    )
}

/// Soft "update available" note shown in the plan picker when the
/// backend's pricing schema version exceeds what this build supports
/// (D4). Non-blocking — the picker still renders the plans it understands.
fn schema_update_note<'a>() -> Element<'a, ConnectAccountMessage> {
    container(
        text::p2_regular(
            "A newer pricing update is available. Some plan details may be \
             incomplete — update Coincube to see the latest plans and prices.",
        )
        .color(color::GREY_3),
    )
    .padding(12)
    .width(Length::Fill)
    .style(|t| container::Style {
        background: Some(iced::Background::Color(t.colors.cards.simple.background)),
        border: iced::Border {
            color: color::ORANGE,
            width: 0.5,
            radius: 10.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Informational note shown atop the plan picker when self-service
/// purchasing is closed (`purchasing_enabled = false` — PLAN-estate-promo
/// PR2). The plans/prices still render so the user can see what each tier
/// offers; this just explains why there's no purchase button behind them.
/// Copy is campaign-agnostic.
fn purchasing_unavailable_note<'a>() -> Element<'a, ConnectAccountMessage> {
    container(
        text::p2_regular(
            "Purchasing is currently unavailable, so plans can't be bought \
             right now. You can still review what each tier includes below.",
        )
        .color(color::GREY_3),
    )
    .padding(12)
    .width(Length::Fill)
    .style(|t| container::Style {
        background: Some(iced::Background::Color(t.colors.cards.simple.background)),
        border: iced::Border {
            color: color::ORANGE,
            width: 0.5,
            radius: 10.0.into(),
        },
        ..Default::default()
    })
    .into()
}

// ── Plan & Billing — top-level router ───────────────────────────────────────

fn plan_billing_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    if let Some(checkout_state) = &state.checkout {
        return checkout_ux(checkout_state);
    }
    if state.show_billing_history {
        return billing_history_ux(state);
    }
    plan_selection_ux(state)
}

// ── Plan provenance card (server-driven, v2) ────────────────────────────────

/// Renders the current plan's `plan_provenance` (`{label, expires_at,
/// badge}`) verbatim — the desktop knows nothing about which campaign
/// granted it. Returns `None` for purchased/free plans (no provenance),
/// which keeps the existing paid/free UX. Display strings are entirely
/// server-authored so a campaign's copy never needs an app release.
fn plan_provenance_card<'a>(plan: &'a ConnectPlan) -> Option<Element<'a, ConnectAccountMessage>> {
    let prov = plan.plan_provenance.as_ref()?;
    let badge_color = plan_tier_color(plan.tier());

    let mut header = Row::new()
        .push(text::p1_bold(plan.tier().to_string()).color(badge_color))
        .push(iced::widget::Space::new().width(Length::Fill));
    if let Some(badge) = prov.badge.as_deref().filter(|b| !b.is_empty()) {
        header = header.push(text::p2_bold(badge).color(color::ORANGE));
    }

    let mut card_col = Column::new()
        .push(header.align_y(Alignment::Center))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(text::p1_medium(prov.label.clone()).style(theme::text::primary));
    if let Some(exp) = prov.expires_at.as_deref().filter(|e| !e.is_empty()) {
        card_col = card_col
            .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
            .push(text::p2_regular(format!("Expires {}", format_date(exp))).color(color::GREY_3));
    }

    Some(
        container(card_col.padding(16).spacing(2))
            .style(move |t| container::Style {
                background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                border: iced::Border {
                    color: badge_color,
                    width: 1.0,
                    radius: 16.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fill)
            .into(),
    )
}

// ── Campaign code redemption field (server-driven, v2) ──────────────────────

/// The generic "promo or referral code" field for Settings → Plan. Campaign-
/// agnostic: forwards the typed code to `POST /connect/campaigns/redeem` and
/// renders the server's success/error message verbatim.
fn campaign_redeem_field<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let rs = &state.campaign_redeem;
    let valid = !rs.code.trim().is_empty();

    let submit: Element<ConnectAccountMessage> = if rs.submitting {
        button::secondary(None, "Redeeming…")
            .width(Length::Fixed(130.0))
            .into()
    } else {
        button::primary(None, "Redeem")
            .on_press_maybe(valid.then_some(ConnectAccountMessage::RedeemCampaignCode))
            .width(Length::Fixed(130.0))
            .into()
    };

    let input = TextInput::new("Promo or referral code", &rs.code)
        // Locked while a redeem is in flight so the in-flight code can't drift
        // out of sync with the result that comes back.
        .on_input_maybe((!rs.submitting).then_some(ConnectAccountMessage::CampaignCodeChanged))
        .on_submit_maybe(
            (valid && !rs.submitting).then_some(ConnectAccountMessage::RedeemCampaignCode),
        )
        .size(15)
        .padding(12);

    let mut col = Column::new()
        .push(text::p2_bold("Have a promo or referral code?").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(
            Row::new()
                .push(input)
                .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
                .push(submit)
                .align_y(Alignment::Center),
        );

    if let Some(result) = &rs.result {
        let (msg, accent) = match result {
            Ok(m) => (m.clone(), color::GREEN),
            Err(e) => (e.clone(), color::RED),
        };
        col = col
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(text::p2_regular(msg).color(accent));
    }

    container(col.padding(16).spacing(2))
        .style(card_style)
        .width(Length::Fill)
        .into()
}

// ── Plan selection view ─────────────────────────────────────────────────────

fn plan_selection_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let current_tier = state
        .plan
        .as_ref()
        .map(|p| p.tier())
        .unwrap_or(&PlanTier::Free);
    let current_cycle = state.plan.as_ref().and_then(|p| p.billing_cycle);

    let cycle = state.selected_billing_cycle;

    // Whether to render checkout CTAs at all. Disabled while the promo
    // window has purchasing closed server-side (PLAN-estate-promo PR2); the
    // picker still shows plans/prices, just with no purchase path behind
    // them so we never route to a checkout the API would reject.
    let purchasing_enabled = state.purchasing_enabled();

    // Billing cycle toggle
    let monthly_btn = if cycle == BillingCycle::Monthly {
        button::primary(None, "Monthly").width(Length::Fill)
    } else {
        button::secondary(None, "Monthly")
            .on_press(ConnectAccountMessage::BillingCycleSelected(
                BillingCycle::Monthly,
            ))
            .width(Length::Fill)
    };
    let annual_btn = if cycle == BillingCycle::Annual {
        button::primary(None, "Annual").width(Length::Fill)
    } else {
        button::secondary(None, "Annual")
            .on_press(ConnectAccountMessage::BillingCycleSelected(
                BillingCycle::Annual,
            ))
            .width(Length::Fill)
    };
    let cycle_toggle = Row::new()
        .push(monthly_btn)
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(annual_btn)
        .width(Length::Fill);

    // Determine upgrade order: Free < Pro < Estate
    let tier_rank = |t: &PlanTier| -> u8 {
        match t {
            PlanTier::Free => 0,
            PlanTier::Pro => 1,
            PlanTier::Estate => 2,
        }
    };

    struct PlanCardData {
        name: String,
        tier: PlanTier,
        features: Vec<String>,
        price_label: String,
    }

    let cards: Vec<PlanCardData> = state
        .features
        .as_ref()
        .map(|features| {
            features
                .plans
                .iter()
                .filter_map(|info| {
                    let tier = match info.name.as_str() {
                        "free" => PlanTier::Free,
                        "pro" => PlanTier::Pro,
                        "estate" | "legacy" => PlanTier::Estate,
                        _ => return None,
                    };
                    let price_label = match &info.price {
                        Some(p) => match cycle {
                            BillingCycle::Monthly => format!("${}/mo", p.monthly),
                            BillingCycle::Annual => format!("${}/yr", p.annual),
                        },
                        None => "Free".to_string(),
                    };
                    let mut features = vec![format!("{} cubes per network", cube_limit_for(&tier))];
                    features.extend(info.features.iter().cloned());
                    Some(PlanCardData {
                        name: tier.to_string(),
                        tier,
                        features,
                        price_label,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if cards.is_empty() {
        // Features failed to load, but the renewal/expired prompt is driven
        // by `/connect/plan` (not features), so still surface it here —
        // otherwise an expired user routed to Plan & Billing only sees the
        // generic "unavailable" copy and loses the renew messaging.
        let mut col = Column::new()
            .push(text::h4_bold("Plan & Billing").style(theme::text::primary))
            .push(iced::widget::Space::new().height(Length::Fixed(20.0)));
        if let Some(banner) = renewal_banner(state) {
            col = col
                .push(banner)
                .push(iced::widget::Space::new().height(Length::Fixed(15.0)));
        }
        // Plan provenance is driven by `/connect/plan`, not features, so
        // render the grant card here too (it shows even while pricing is
        // unavailable, e.g. offline).
        if let Some(prov) = state.plan.as_ref().and_then(plan_provenance_card) {
            col = col
                .push(prov)
                .push(iced::widget::Space::new().height(Length::Fixed(15.0)));
        }
        // A newer schema can rename tiers so every card is filtered out,
        // landing here — exactly when the "update available" note is most
        // relevant, so surface it on this path too (D4).
        if state.pricing_schema_outdated() {
            col = col
                .push(schema_update_note())
                .push(iced::widget::Space::new().height(Length::Fixed(12.0)));
        }
        col = col
            .push(
                text::p1_regular(
                    "Pricing temporarily unavailable.\n\
                     Reconnect to the internet to view current plans and features.",
                )
                .color(color::GREY_3),
            )
            // The redeem field doesn't depend on the pricing fetch.
            .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
            .push(campaign_redeem_field(state));
        return container(col).padding(16).into();
    }

    let mut col = Column::new()
        .push(text::h4_bold("Plan & Billing").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)));

    // Renewal reminder (D1) / expired prompt (D3). On the plan view the
    // expired prompt sits above the picker so the user can renew in place.
    if let Some(banner) = renewal_banner(state) {
        col = col
            .push(banner)
            .push(iced::widget::Space::new().height(Length::Fixed(15.0)));
    }

    // Server-driven provenance card for a campaign-granted plan (v2) — sits
    // above the picker so the grant reads as the headline state.
    if let Some(prov) = state.plan.as_ref().and_then(plan_provenance_card) {
        col = col
            .push(prov)
            .push(iced::widget::Space::new().height(Length::Fixed(15.0)));
    }

    // Soft "update available" note when the backend advertises a pricing
    // schema newer than this build understands (D4).
    if state.pricing_schema_outdated() {
        col = col
            .push(schema_update_note())
            .push(iced::widget::Space::new().height(Length::Fixed(12.0)));
    }

    // "Purchasing closed" note while the promo window has checkout disabled
    // (PLAN-estate-promo PR2). The upgrade CTAs below render non-pressable in
    // that state; this explains why.
    if !purchasing_enabled {
        col = col
            .push(purchasing_unavailable_note())
            .push(iced::widget::Space::new().height(Length::Fixed(12.0)));
    }

    col = col
        .push(cycle_toggle)
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)));

    for card in cards {
        // Paid tiers are "current" only when both tier *and* cycle match the
        // user's actual plan. Free tier has no cycle, so tier alone suffices.
        let is_current = card.tier == *current_tier
            && match current_cycle {
                Some(cc) => cc == cycle,
                None => true,
            };
        let is_upgrade = tier_rank(&card.tier) > tier_rank(current_tier);
        let badge_color = plan_tier_color(&card.tier);

        let mut card_col = Column::new()
            .push(
                Row::new()
                    .push(text::p1_bold(card.name).color(badge_color))
                    .push(iced::widget::Space::new().width(Length::Fill))
                    .push(text::p1_regular(card.price_label).color(color::GREY_3)),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(6.0)));

        for feature in card.features {
            card_col =
                card_col.push(text::p2_regular(format!("• {}", feature)).color(color::GREY_3));
        }

        // Expiry line on the user's current paid plan card.
        if is_current && card.tier != PlanTier::Free {
            if let Some(renewal) = state.plan.as_ref().and_then(|p| p.renewal_at.as_deref()) {
                let date_short = if renewal.len() >= 10 {
                    &renewal[..10]
                } else {
                    renewal
                };
                card_col = card_col
                    .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
                    .push(
                        text::p2_regular(format!("Expires on {}", date_short)).color(color::GREY_3),
                    );
            }
        }

        card_col = card_col
            .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
            .push(if is_current {
                button::secondary(None, "Current Plan").width(Length::Fill)
            } else if is_upgrade && purchasing_enabled {
                let label = match &card.tier {
                    PlanTier::Pro => "Upgrade to Pro",
                    PlanTier::Estate => "Upgrade to Estate",
                    _ => "Upgrade",
                };
                button::primary(None, label)
                    .on_press(ConnectAccountMessage::StartCheckout(card.tier))
                    .width(Length::Fill)
            } else if is_upgrade {
                // Purchasing closed (promo window) — show the tier without a
                // checkout CTA (PLAN-estate-promo PR2). Non-pressable, like
                // the other informational buttons here.
                button::secondary(None, "Unavailable").width(Length::Fill)
            } else {
                // Downgrade or Free — no action
                button::secondary(None, "—").width(Length::Fill)
            })
            .padding(16)
            .spacing(2);

        col = col.push(
            container(card_col)
                .style(move |t| container::Style {
                    background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                    border: iced::Border {
                        color: if is_current {
                            badge_color
                        } else {
                            t.colors.cards.simple.border.unwrap_or(color::GREY_5)
                        },
                        width: if is_current { 1.0 } else { 0.2 },
                        radius: 16.0.into(),
                    },
                    ..Default::default()
                })
                .width(Length::Fill),
        );
        col = col.push(iced::widget::Space::new().height(Length::Fixed(10.0)));
    }

    // Promo / referral code field (server-driven redemption, v2).
    col = col
        .push(iced::widget::Space::new().height(Length::Fixed(5.0)))
        .push(campaign_redeem_field(state))
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)));

    // Billing history link
    col = col
        .push(iced::widget::Space::new().height(Length::Fixed(5.0)))
        .push(
            button::secondary(None, "View Billing History")
                .on_press(ConnectAccountMessage::ToggleBillingHistory)
                .width(Length::Fill),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(
            container(
                text::p2_regular(
                    "Payments via Bitcoin (Lightning or on-chain). \
                     No recurring subscriptions — pay upfront, renewal reminders sent by email.",
                )
                .color(color::GREY_3),
            )
            .padding(12)
            .style(|t| container::Style {
                background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                border: iced::Border {
                    radius: 10.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .width(Length::Fill),
        );

    col.spacing(0).width(Length::Fill).into()
}

// ── Checkout / payment view ─────────────────────────────────────────────────

fn checkout_ux<'a>(
    checkout_state: &'a crate::app::state::connect::CheckoutState,
) -> Element<'a, ConnectAccountMessage> {
    let mut col = Column::new()
        .push(text::h4_bold("Checkout").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)));

    match &checkout_state.phase {
        CheckoutPhase::Creating => {
            col = col.push(text::p1_regular("Creating invoice…").color(color::GREY_3));
        }

        CheckoutPhase::AwaitingPayment | CheckoutPhase::Processing => {
            if let Some(ref resp) = checkout_state.checkout {
                let amount_line = format!(
                    "{} {} ({} sats)",
                    resp.amount_fiat, resp.fiat_currency, resp.amount_sats
                );
                let plan_line = format!("Upgrade to {} ({})", resp.plan, resp.billing_cycle);

                col = col
                    .push(text::p1_bold(plan_line).style(theme::text::primary))
                    .push(iced::widget::Space::new().height(Length::Fixed(4.0)))
                    .push(text::p1_regular(amount_line).color(color::ORANGE))
                    .push(iced::widget::Space::new().height(Length::Fixed(15.0)));

                // Lightning QR
                if let Some(ref qr) = checkout_state.lightning_qr {
                    col = col.push(
                        container(QRCode::new(qr).cell_size(6))
                            .width(Length::Fill)
                            .center_x(Length::Fill),
                    );
                    col = col.push(iced::widget::Space::new().height(Length::Fixed(12.0)));
                }

                // Lightning invoice (truncated) + copy
                let invoice_display = if resp.lightning_invoice.len() > 40 {
                    format!("{}…", &resp.lightning_invoice[..40])
                } else {
                    resp.lightning_invoice.clone()
                };
                col = col.push(
                    Row::new()
                        .push(
                            text::p2_regular(invoice_display)
                                .color(color::GREY_3)
                                .width(Length::Fill),
                        )
                        .push(
                            button::secondary(None, "Copy")
                                .on_press(ConnectAccountMessage::CopyToClipboard(
                                    resp.lightning_invoice.clone(),
                                ))
                                .width(Length::Shrink),
                        )
                        .align_y(Alignment::Center)
                        .spacing(8),
                );

                col = col.push(iced::widget::Space::new().height(Length::Fixed(10.0)));

                // On-chain address + copy
                col = col.push(
                    Row::new()
                        .push(
                            text::p2_regular(format!("On-chain: {}", resp.on_chain_address))
                                .color(color::GREY_3)
                                .width(Length::Fill),
                        )
                        .push(
                            button::secondary(None, "Copy")
                                .on_press(ConnectAccountMessage::CopyToClipboard(
                                    resp.on_chain_address.clone(),
                                ))
                                .width(Length::Shrink),
                        )
                        .align_y(Alignment::Center)
                        .spacing(8),
                );

                col = col.push(iced::widget::Space::new().height(Length::Fixed(12.0)));

                // Open in browser
                col = col.push(
                    button::secondary(None, "Open in Browser")
                        .on_press(ConnectAccountMessage::OpenCheckoutUrl(
                            resp.checkout_url.clone(),
                        ))
                        .width(Length::Fill),
                );

                col = col.push(iced::widget::Space::new().height(Length::Fixed(10.0)));

                // Expires
                col = col.push(
                    text::p2_regular(format!("Expires: {}", resp.expires_at)).color(color::GREY_3),
                );

                if matches!(checkout_state.phase, CheckoutPhase::Processing) {
                    col = col
                        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                        .push(
                            text::p2_regular("Payment detected, confirming…").color(color::ORANGE),
                        );
                }
            }

            col = col
                .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
                .push(
                    button::secondary(None, "Cancel")
                        .on_press(ConnectAccountMessage::DismissCheckout)
                        .width(Length::Fill),
                );
        }

        CheckoutPhase::Paid => {
            let plan_name = checkout_state
                .checkout
                .as_ref()
                .map(|c| c.plan.to_string())
                .unwrap_or_else(|| "your new plan".to_string());
            col = col
                .push(
                    text::p1_bold(format!("Payment confirmed! Upgraded to {}.", plan_name))
                        .color(color::ORANGE),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
                .push(
                    button::primary(None, "Done")
                        .on_press(ConnectAccountMessage::DismissCheckout)
                        .width(Length::Fill),
                );
        }

        CheckoutPhase::Expired => {
            col = col
                .push(text::p1_regular("Invoice expired.").color(color::RED))
                .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
                .push(
                    button::primary(None, "Try Again")
                        .on_press(ConnectAccountMessage::DismissCheckout)
                        .width(Length::Fill),
                );
        }

        CheckoutPhase::Failed(msg) => {
            col = col
                .push(text::p1_regular(format!("Error: {}", msg)).color(color::RED))
                .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
                .push(
                    button::primary(None, "Try Again")
                        .on_press(ConnectAccountMessage::DismissCheckout)
                        .width(Length::Fill),
                );
        }
    }

    col.spacing(0).width(Length::Fill).into()
}

// ── Billing history view ────────────────────────────────────────────────────

fn billing_history_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, ConnectAccountMessage> {
    let back_button = iced::widget::button(
        Row::new()
            .push(previous_icon().color(color::GREY_2))
            .push(iced::widget::Space::new().width(Length::Fixed(5.0)))
            .push(text::p1_medium("Back").style(theme::text::secondary))
            .spacing(5)
            .align_y(Alignment::Center),
    )
    .style(theme::button::transparent)
    .on_press(ConnectAccountMessage::ToggleBillingHistory);

    let mut col = Column::new()
        .push(back_button)
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(text::h4_bold("Billing History").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)));

    match &state.billing_history {
        None => {
            col = col.push(text::p1_regular("Loading…").color(color::GREY_3));
        }
        Some(entries) if entries.is_empty() => {
            col = col.push(text::p1_regular("No billing history yet.").color(color::GREY_3));
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
                let time_label = if let Some(ref last_used) = d.last_used_at {
                    format!("Last active: {}", format_datetime(last_used))
                } else {
                    format!("Added: {}", format_datetime(&d.created_at))
                };
                col = col.push(
                    Row::new()
                        .push(text::p2_regular(label).style(theme::text::primary))
                        .push(iced::widget::Space::new().width(Length::Fill))
                        .push(text::p2_regular(time_label).color(color::GREY_3)),
                );
            }
            col.into()
        }
    };

    let activity_section: Element<ConnectAccountMessage> = match &state.login_activity {
        None => text::p2_regular("Loading activity…")
            .color(color::GREY_3)
            .into(),
        Some(activity) if activity.is_empty() => text::p2_regular("No login activity on record.")
            .color(color::GREY_3)
            .into(),
        Some(activity) => {
            let mut col = Column::new().spacing(6);
            for a in activity.iter().take(50) {
                let ok = a.success.unwrap_or(false);
                let status = if ok { "✓" } else { "✗" };
                let status_color = if ok { color::ORANGE } else { color::RED };

                let ip = a.ip_address.as_deref();
                let ua = a.user_agent.as_deref();
                let ip_and_ua = match (ip, ua) {
                    (Some(i), Some(u)) => {
                        let short_u = if u.chars().count() > 60 {
                            format!("{}…", u.chars().take(59).collect::<String>())
                        } else {
                            u.to_string()
                        };
                        format!("{} - {}", i, short_u)
                    }
                    (Some(i), None) => i.to_string(),
                    (None, Some(u)) => {
                        if u.chars().count() > 60 {
                            format!("{}…", u.chars().take(59).collect::<String>())
                        } else {
                            u.to_string()
                        }
                    }
                    (None, None) => "Unknown device".to_string(),
                };

                let message_text = if ok {
                    format!("Successful login from {}", ip_and_ua)
                } else {
                    format!("Failed login attempt from {}", ip_and_ua)
                };

                col = col.push(
                    Row::new()
                        .push(text::p2_regular(status).color(status_color))
                        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
                        .push(text::p2_regular(message_text).style(theme::text::primary))
                        .push(iced::widget::Space::new().width(Length::Fill))
                        .push(
                            text::p2_regular(format_datetime(&a.created_at)).color(color::GREY_3),
                        ),
                );
            }
            col.into()
        }
    };

    Column::new()
        .push(text::h4_bold("Security").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
        .push(
            container(
                Column::new()
                    .push(text::p1_bold("Verified Devices").style(theme::text::primary))
                    .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
                    .push(devices_section)
                    .padding(16)
                    .spacing(2),
            )
            .style(card_style)
            .width(Length::Fill),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
        .push(
            container(
                Column::new()
                    .push(text::p1_bold("Login Activity").style(theme::text::primary))
                    .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
                    .push(activity_section)
                    .padding(16)
                    .spacing(2),
            )
            .style(card_style)
            .width(Length::Fill),
        )
        .spacing(0)
        .width(Length::Fill)
        .into()
}

/// Post-login duress verification gate (Phase 6). Shown after auth while
/// `get_duress_state` is in flight, so the dashboard isn't revealed to a
/// possibly-in-duress account. On terminal failure (`failed`) it offers a Retry
/// rather than falling through to the dashboard.
fn checking_duress_ux<'a>(failed: bool) -> Element<'a, ConnectAccountMessage> {
    let mut col = Column::new().spacing(16).align_x(Alignment::Center);
    if failed {
        col = col
            .push(text::p1_regular("Couldn't verify your account status.").color(color::GREY_3))
            .push(
                button::primary(None, "Retry")
                    .width(Length::Fixed(160.0))
                    .on_press(ConnectAccountMessage::RetryDuressCheck),
            );
    } else {
        col = col.push(text::p1_regular("Checking your account…").color(color::GREY_3));
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

    // Wizard takes over the panel while enrollment is in progress.
    if let Some(enroll) = &state.duress_enroll {
        return duress_enroll::enroll_ux(enroll);
    }

    let entitled = state
        .plan
        .as_ref()
        .map(|p| p.entitlements.duress_remote_lock)
        .unwrap_or(false);

    let mut col = Column::new()
        .push(text::h4_bold("Duress Mode").style(theme::text::primary))
        .push(iced::widget::Space::new().height(Length::Fixed(15.0)));

    if !entitled {
        return col
            .push(
                container(
                    Column::new()
                        .push(text::p1_regular(
                            "Duress mode is available on Pro and Estate plans. Upgrade your \
                             Connect plan to enable it.",
                        ))
                        .padding(20),
                )
                .style(card_style)
                .width(Length::Fill),
            )
            .width(Length::Fill)
            .into();
    }

    let delays: String = DuressDelay::ALL
        .iter()
        .map(|d| d.label())
        .collect::<Vec<_>>()
        .join(" · ");

    // What duress activation does — stated plainly, no fine print.
    col = col.push(
        container(
            Column::new()
                .push(text::p1_bold("What duress mode does").style(theme::text::primary))
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    text::p2_regular(
                        "When you unlock a Cube with your duress PIN, this device erases every \
                     Cube on it and signals Connect to lock your account. The device then \
                     shows a dead-end screen until you clear duress from another trusted device.",
                    )
                    .color(color::GREY_3),
                )
                .padding(20)
                .spacing(2),
        )
        .style(card_style)
        .width(Length::Fill),
    );

    col = col.push(iced::widget::Space::new().height(Length::Fixed(12.0)));

    // Credentials the user will set during enrollment.
    col = col.push(
        container(
            Column::new()
                .push(text::p1_bold("You'll set").style(theme::text::primary))
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    text::p2_regular(
                        "• A duress PIN — at least 2 character changes from your regular PIN.",
                    )
                    .color(color::GREY_3),
                )
                .push(
                    text::p2_regular(format!(
                        "• An all-clear passphrase — at least {} characters, used to unlock your \
                     account from a trusted device.",
                        MIN_ALL_CLEAR_LEN
                    ))
                    .color(color::GREY_3),
                )
                .push(
                    text::p2_regular(
                        "• A duress recovery-kit password (Tier 1) — covers all your Cubes.",
                    )
                    .color(color::GREY_3),
                )
                .push(
                    text::p2_regular(format!("• An unlock delay: {}.", delays))
                        .color(color::GREY_3),
                )
                .padding(20)
                .spacing(4),
        )
        .style(card_style)
        .width(Length::Fill),
    );

    col = col.push(iced::widget::Space::new().height(Length::Fixed(12.0)));

    // Tier 2 BIG warning — shown whenever the account has no recovery kit yet.
    // We can't see per-Cube CRK state from this panel, so we surface the
    // warning unconditionally; the wizard refines it per tier.
    col = col.push(
        container(
            Column::new()
                .push(
                    text::p1_bold("Set up a Cube Recovery Kit first")
                        .style(theme::text::warning),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(text::p2_regular(
                    "Without a Cube Recovery Kit, a duress wipe is irreversible from Connect — \
                     you would need your seed-phrase backup to restore. We strongly recommend \
                     setting up a Cube Recovery Kit before enabling duress mode.",
                ).color(color::GREY_3))
                .padding(20)
                .spacing(2),
        )
        .style(card_style)
        .width(Length::Fill),
    );

    col = col.push(iced::widget::Space::new().height(Length::Fixed(16.0)));
    col = col.push(
        button::primary(None, "Set up Duress Mode")
            .width(Length::Fixed(240.0))
            .on_press(ConnectAccountMessage::Duress(
                DuressMessage::StartEnrollment,
            )),
    );
    // Tier 2 (Connect, no recovery kit) — the plan's Task 2.1 secondary path.
    // The panel can't see per-Cube CRK state, so the user picks: this skips the
    // duress recovery-kit password step.
    col = col.push(
        button::transparent(None, "Continue without a recovery kit (advanced)").on_press(
            ConnectAccountMessage::Duress(DuressMessage::StartEnrollmentWithoutCrk),
        ),
    );

    col.width(Length::Fill).into()
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
        .style(card_style)
        .width(Length::Fill)
        .into(),

        AvatarFlowStep::Reveal | AvatarFlowStep::Settings => avatar_settings_ux(state),
    };

    if let Some(err) = state.avatar_error.as_deref() {
        let retry_button: Element<ConnectCubeMessage> = if state.registration_error.is_some() {
            button::primary(None, "Retry Connection")
                .on_press(ConnectCubeMessage::RetryRegistration)
                .into()
        } else {
            button::primary(None, "Try Again")
                .on_press(ConnectCubeMessage::Avatar(AvatarMessage::Retry))
                .into()
        };
        return Column::new()
            .push(title)
            .push(iced::widget::Space::new().height(Length::Fixed(15.0)))
            .push(
                container(
                    Column::new()
                        .push(text::p1_bold("Error").color(color::RED))
                        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                        .push(text::p2_regular(err).color(color::GREY_3))
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

fn avatar_questionnaire_ux<'a>(state: &'a ConnectCubePanel) -> Element<'a, ConnectCubeMessage> {
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
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::GenderChanged(
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
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::GenderChanged(
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
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::ArchetypeChanged(
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
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::ArchetypeChanged(
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
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::ArchetypeChanged(
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
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::AgeFeelChanged(
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
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::AgeFeelChanged(
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
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::AgeFeelChanged(
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
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::DemeanorChanged(
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
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::DemeanorChanged(
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
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::DemeanorChanged(
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
            .on_press(ConnectCubeMessage::Avatar(
                AvatarMessage::ArmorStyleChanged(AvatarArmorStyle::Light),
            )),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.armor_style == AvatarArmorStyle::Standard {
                button::primary(None, "Standard")
            } else {
                button::secondary(None, "Standard")
            }
            .on_press(ConnectCubeMessage::Avatar(
                AvatarMessage::ArmorStyleChanged(AvatarArmorStyle::Standard),
            )),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.armor_style == AvatarArmorStyle::Heavy {
                button::primary(None, "Heavy")
            } else {
                button::secondary(None, "Heavy")
            }
            .on_press(ConnectCubeMessage::Avatar(
                AvatarMessage::ArmorStyleChanged(AvatarArmorStyle::Heavy),
            )),
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
            .on_press(ConnectCubeMessage::Avatar(
                AvatarMessage::AccentMotifChanged(AvatarAccentMotif::OrangeSun),
            )),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.accent_motif == AvatarAccentMotif::Splatter {
                button::primary(None, "Splatter")
            } else {
                button::secondary(None, "Splatter")
            }
            .on_press(ConnectCubeMessage::Avatar(
                AvatarMessage::AccentMotifChanged(AvatarAccentMotif::Splatter),
            )),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.accent_motif == AvatarAccentMotif::Seal {
                button::primary(None, "Seal")
            } else {
                button::secondary(None, "Seal")
            }
            .on_press(ConnectCubeMessage::Avatar(
                AvatarMessage::AccentMotifChanged(AvatarAccentMotif::Seal),
            )),
        )
        .push(iced::widget::Space::new().width(Length::Fixed(8.0)))
        .push(
            if draft.accent_motif == AvatarAccentMotif::Calligraphy {
                button::primary(None, "Calligraphy")
            } else {
                button::secondary(None, "Calligraphy")
            }
            .on_press(ConnectCubeMessage::Avatar(
                AvatarMessage::AccentMotifChanged(AvatarAccentMotif::Calligraphy),
            )),
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
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::LaserEyesToggled(
                !draft.laser_eyes,
            ))),
        )
        .align_y(Alignment::Center);

    let has_ln = state.lightning_address.is_some();
    let generate_btn = if has_ln {
        button::primary(None, "Generate Avatar")
            .on_press(ConnectCubeMessage::Avatar(AvatarMessage::Generate))
    } else {
        button::primary(None, "Set Lightning Address First")
    };

    container(
        Column::new()
            .push(text::p1_bold("Choose Your Traits").style(theme::text::primary))
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
    .style(card_style)
    .width(Length::Fill)
    .into()
}

fn avatar_settings_ux<'a>(state: &'a ConnectCubePanel) -> Element<'a, ConnectCubeMessage> {
    let Some(ref data) = state.avatar_data else {
        return container(text::p2_regular("Loading avatar data…").color(color::GREY_3))
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
        button::primary(None, "No Regenerations Remaining")
    } else {
        button::primary(None, "Regenerate Avatar").on_press(ConnectCubeMessage::Avatar(
            AvatarMessage::SetStep(AvatarFlowStep::Questionnaire),
        ))
    };

    let download_btn = button::secondary(None, "Download PNG")
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
    use crate::services::coincube::{ConnectPlan, PlanEntitlements, PlanProvenance, PlanStatus};

    fn plan(tier: PlanTier, status: PlanStatus, renewal_at: Option<&str>) -> ConnectPlan {
        ConnectPlan {
            plan: tier,
            status,
            renewal_at: renewal_at.map(|s| s.to_string()),
            entitlements: PlanEntitlements {
                free_signing_key_count: 0,
                policy_editing: false,
                legacy_invites: false,
                linked_keychains: false,
                duress_remote_lock: false,
                business_orgs: false,
            },
            billing_cycle: Some(BillingCycle::Monthly),
            plan_provenance: None,
        }
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
            }],
            pricing_schema_version: None,
            purchasing_enabled: enabled,
        }
    }

    /// The provenance card renders only when the API sends `plan_provenance`
    /// — a purchased/free plan (no provenance) shows nothing extra.
    #[test]
    fn provenance_card_present_only_with_provenance() {
        let granted = granted_estate(Some("2027-07-04T00:00:00Z"), Some("Founding member"));
        assert!(plan_provenance_card(&granted).is_some());

        let paid = plan(
            PlanTier::Pro,
            PlanStatus::Active,
            Some("2027-01-01T00:00:00Z"),
        );
        assert!(plan_provenance_card(&paid).is_none());
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
}
