//! Plan & billing in Tenshu (billing v2, boards 1–3 of the billing canvas).
//!
//! - [`plan_billing_ux`] routes between Plan & billing, Change plan, the
//!   Review / checkout step and billing history.
//! - Plan & billing (board 1): the current plan summary, notices, the promo
//!   field, features on this account and the latest billing history.
//! - Change plan (board 2): billing period, plan rows and the comparison
//!   table; Continue sends [`ConnectAccountMessage::ReviewPlanChange`].
//! - Review (board 3): plan, amount, dates covered, how to pay; "Create
//!   invoice" sends [`ConnectAccountMessage::StartCheckout`].
//!
//! Plan facts come from Connect (`/connect/plan`, `/connect/features`); the
//! local tier defaults are only an offline fallback (master invariant I4).

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::{
    component::tenshu::{self, text as tenshu_text, FieldSize},
    icon::*,
    theme::{
        self,
        tenshu::{button::ActionKind, size as tenshu_size, Tone},
    },
    widget::{action::Action as FocusableAction, *},
};
use iced::{widget::container, Alignment, Length};

use super::{
    footnote, format_date, group_title, horizontal_rule, launch_feature_copy, lead,
    plan_provenance_display, row_tag, ruled_list, sheet_button, sheet_link, sheet_title,
    status_line,
};
use crate::{
    app::{
        settings::global::AccountTier,
        state::connect::{
            account::{PayWith, PaymentNetwork, PlanRefresh},
            CheckoutPhase, CheckoutState, ConnectAccountPanel, PlanLifecycle,
        },
        view::ConnectAccountMessage,
    },
    services::coincube::{
        BillingCycle, BillingHistoryEntry, ChargeState, ConnectPlan, PlanEntitlements,
        PlanFeatureInfo, PlanStatus, PlanTier,
    },
};

type Msg = ConnectAccountMessage;

const TIERS: [PlanTier; 3] = [PlanTier::Free, PlanTier::Pro, PlanTier::Estate];

// ── Shared pieces ────────────────────────────────────────────────────────────

fn rank(tier: &PlanTier) -> u8 {
    match tier {
        PlanTier::Free => 0,
        PlanTier::Pro => 1,
        PlanTier::Estate => 2,
    }
}

fn tier_key(tier: &PlanTier) -> &'static str {
    match tier {
        PlanTier::Free => "free",
        PlanTier::Pro => "pro",
        PlanTier::Estate => "estate",
    }
}

fn cycle_label(cycle: BillingCycle) -> &'static str {
    match cycle {
        BillingCycle::Monthly => "Monthly",
        BillingCycle::Annual => "Annual",
    }
}

/// The server's catalog entry for `tier` (`/connect/features`). The API
/// still calls Estate `legacy` on some older payloads.
fn plan_info<'a>(state: &'a ConnectAccountPanel, tier: &PlanTier) -> Option<&'a PlanFeatureInfo> {
    let key = tier_key(tier);
    state.features.as_ref()?.plans.iter().find(|info| {
        info.name.eq_ignore_ascii_case(key)
            || (*tier == PlanTier::Estate && info.name.eq_ignore_ascii_case("legacy"))
    })
}

/// USD price of `tier` for `cycle`; `None` for the unpriced Free tier or
/// when pricing is unavailable.
fn price_for(state: &ConnectAccountPanel, tier: &PlanTier, cycle: BillingCycle) -> Option<u32> {
    let price = plan_info(state, tier)?.price.as_ref()?;
    Some(match cycle {
        BillingCycle::Monthly => price.monthly,
        BillingCycle::Annual => price.annual,
    })
}

/// Offline fallback for a tier's entitlements (I4: the server value wins).
fn fallback_entitlements(tier: &PlanTier) -> PlanEntitlements {
    let account_tier = match tier {
        PlanTier::Free => AccountTier::Free,
        PlanTier::Pro => AccountTier::Pro,
        PlanTier::Estate => AccountTier::Estate,
    };
    PlanEntitlements {
        cube_limit: account_tier.cube_limit() as u32,
        recovery_alerts: true,
        inheritance_escrow: *tier == PlanTier::Estate,
        collaborative_invitations: *tier == PlanTier::Estate,
        attach_policies: *tier != PlanTier::Free,
        ..PlanEntitlements::default()
    }
}

/// Entitlements for `tier`: the catalog's, then (for the account's own tier)
/// `/connect/plan`'s, then the offline fallback.
fn entitlements_for(state: &ConnectAccountPanel, tier: &PlanTier) -> PlanEntitlements {
    if let Some(mut ent) = plan_info(state, tier).and_then(|info| info.entitlements.clone()) {
        // An omitted cubeLimit parses as 0: never show "0 per network";
        // use the plan's own value, else the offline default.
        if ent.cube_limit == 0 {
            ent.cube_limit = state
                .plan
                .as_ref()
                .filter(|plan| plan.tier() == tier && plan.entitlements.cube_limit > 0)
                .map(|plan| plan.entitlements.cube_limit)
                .unwrap_or_else(|| fallback_entitlements(tier).cube_limit);
        }
        return ent;
    }
    if let Some(plan) = state.plan.as_ref().filter(|plan| plan.tier() == tier) {
        if plan.entitlements.cube_limit > 0 {
            return plan.entitlements.clone();
        }
    }
    fallback_entitlements(tier)
}

/// The board's notice (`.notice`): selected fill, a glyph in `tone`, a bold
/// title, a muted body and optional actions.
pub(super) fn billing_notice<'a, M: 'a>(
    icon: iced::widget::Text<'a, theme::Theme>,
    tone: Tone,
    title: impl iced::widget::text::IntoFragment<'a>,
    body: impl iced::widget::text::IntoFragment<'a>,
    actions: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let mut column = Column::new()
        .spacing(4)
        .width(Length::Fill)
        .push(group_title(title))
        .push(lead(body));
    if let Some(actions) = actions {
        column = column
            .push(iced::widget::Space::new().height(Length::Fixed(6.0)))
            .push(actions);
    }
    container(
        Row::new()
            .spacing(12)
            .align_y(Alignment::Start)
            .push(
                icon.size(18)
                    .style(move |t| theme::tenshu::text::tone(t, tone)),
            )
            .push(column),
    )
    .padding([14, 16])
    .width(Length::Fill)
    .style(|t: &theme::Theme| container::Style {
        background: Some(iced::Background::Color(t.colors.shell.selected)),
        border: iced::Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

fn actions_row<'a>(items: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut row = Row::new().spacing(12).align_y(Alignment::Center);
    for item in items {
        row = row.push(item);
    }
    row.into()
}

fn space<'a>(height: f32) -> Element<'a, Msg> {
    iced::widget::Space::new()
        .height(Length::Fixed(height))
        .into()
}

/// A selectable row (`.radio`): a 16px radio, the name with an optional tag,
/// a muted description and, on the right, an optional price column.
#[allow(clippy::too_many_arguments)]
fn choice_row<'a>(
    id: &'static str,
    name: String,
    tag: Option<(&'static str, bool)>,
    description: String,
    price: Option<(String, &'static str)>,
    selected: bool,
    enabled: bool,
    on_press: Option<Msg>,
) -> Element<'a, Msg> {
    let mark = container(
        container(iced::widget::Space::new())
            .width(Length::Fixed(tenshu_size::RADIO_DOT))
            .height(Length::Fixed(tenshu_size::RADIO_DOT))
            .style(move |t| theme::tenshu::control::radio_dot(t, selected, enabled)),
    )
    .width(Length::Fixed(tenshu_size::RADIO))
    .height(Length::Fixed(tenshu_size::RADIO))
    .center_x(Length::Fixed(tenshu_size::RADIO))
    .center_y(Length::Fixed(tenshu_size::RADIO))
    .style(move |t| theme::tenshu::control::radio_ring(t, selected, enabled));

    let mut name_row = Row::new().spacing(8).align_y(Alignment::Center).push(
        tenshu_text::body(name)
            .font(coincube_ui::font::SEMIBOLD)
            .style(move |t: &theme::Theme| {
                theme::tenshu::text::tone(t, if enabled { Tone::Neutral } else { Tone::Muted })
            }),
    );
    if let Some((tag, current)) = tag {
        name_row = name_row.push(row_tag(tag, current));
    }
    let text = Column::new()
        .spacing(2)
        .width(Length::Fill)
        .push(name_row)
        .push(lead(description));
    let mut content = Row::new()
        .spacing(10)
        .align_y(Alignment::Start)
        .width(Length::Fill)
        .push(
            container(mark)
                .width(Length::Fixed(20.0))
                .padding(iced::Padding::default().top(3.0)),
        )
        .push(text);
    if let Some((amount, per)) = price {
        content = content.push(
            Column::new()
                .width(Length::Fixed(168.0))
                .align_x(Alignment::End)
                .push(
                    tenshu_text::body(amount)
                        .size(16)
                        .font(coincube_ui::font::SEMIBOLD),
                )
                .push(footnote(per)),
        );
    }
    FocusableAction::new(id, content, Msg::FocusAction)
        .style(theme::tenshu::control::radio_row)
        .width(Length::Fill)
        .padding([14.0, 0.0])
        .focus_radius(theme::tenshu::radius::ROW)
        .on_press_maybe(on_press)
        .into()
}

// ── Router ───────────────────────────────────────────────────────────────────

pub(super) fn plan_billing_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, Msg> {
    if let Some(checkout_state) = &state.checkout {
        return match checkout_state.phase {
            CheckoutPhase::Review => review_ux(state, checkout_state),
            CheckoutPhase::CardOpening
            | CheckoutPhase::CardInBrowser
            | CheckoutPhase::CardActive
            | CheckoutPhase::CardCancelled
            | CheckoutPhase::CardFailed { .. }
            | CheckoutPhase::ChangePreviewing
            | CheckoutPhase::ChangeReview
            | CheckoutPhase::ChangeApplying
            | CheckoutPhase::ChangeInBrowser
            | CheckoutPhase::ChangeDone { .. }
            | CheckoutPhase::ChangeFailed { .. }
            | CheckoutPhase::ChangeUnconfirmed { .. } => card_phase_ux(state, checkout_state),
            CheckoutPhase::PollFailed(_)
                if checkout_state.card_session.is_some() || checkout_state.action_url.is_some() =>
            {
                card_poll_failed_ux(checkout_state)
            }
            _ => checkout_phase_ux(state, checkout_state),
        };
    }
    if state.show_billing_history {
        return billing_history_ux(state);
    }
    if state.show_plan_comparison {
        return change_plan_ux(state);
    }
    billing_management_ux(state)
}

// ── Plan & billing (board 1) ─────────────────────────────────────────────────

/// Renewal reminder or expired prompt (board 1 notices). `None` while the
/// plan is comfortably active or free. The per-session dismissal applies
/// only to the reminder; the expired prompt is not dismissible.
pub(super) fn renewal_banner<'a>(state: &'a ConnectAccountPanel) -> Option<Element<'a, Msg>> {
    match state.plan_lifecycle() {
        PlanLifecycle::RenewalDue { .. } => {
            // Hidden while dismissed for this session, and while purchasing
            // is closed so "Renew" is never a dead end.
            if !state.show_renewal_banner() {
                return None;
            }
            let plan = state.plan.as_ref()?;
            let date = plan.renewal_at.as_deref().map(format_date);
            let title = match &date {
                Some(date) => format!("Renewal due {date}"),
                None => "Renewal due soon".to_string(),
            };
            let body = match &date {
                Some(date) => format!(
                    "Your {} plan is paid through {date}. Renew now to keep these features.",
                    plan.tier()
                ),
                None => format!(
                    "Your {} plan ends soon. Renew now to keep these features.",
                    plan.tier()
                ),
            };
            Some(billing_notice(
                exclamation_circle_icon(),
                Tone::Warn,
                title,
                body,
                Some(actions_row(vec![
                    sheet_button(format!("Renew {}", plan.tier()), ActionKind::Primary, true)
                        .on_press(Msg::RenewCurrentPlan)
                        .into(),
                    sheet_link(localized_copy("Remind me later"))
                        .on_press(Msg::DismissRenewalBanner)
                        .into(),
                ])),
            ))
        }
        PlanLifecycle::Expired => {
            let title = match state
                .plan
                .as_ref()
                .and_then(|p| p.renewal_at.as_deref())
                .map(format_date)
            {
                Some(date) => format!("Plan expired on {date}"),
                None => "Plan expired".to_string(),
            };
            Some(billing_notice(
                exclamation_circle_icon(),
                Tone::Bad,
                title,
                localized_copy("Renew to restore premium features."),
                // A lapsed plan reports as Free, so there is no tier to
                // pre-fill an invoice with: open Change plan instead.
                Some(
                    sheet_button(localized_copy("View plans"), ActionKind::Primary, true)
                        .on_press(Msg::ComparePlans(true))
                        .into(),
                ),
            ))
        }
        PlanLifecycle::Active | PlanLifecycle::Free | PlanLifecycle::PastDue { .. } => None,
    }
}

/// Soft note when Connect describes plans with a newer pricing schema.
fn schema_update_note<'a>() -> Element<'a, Msg> {
    billing_notice(
        exclamation_circle_icon(),
        Tone::Muted,
        localized_copy("A newer pricing update is available"),
        localized_copy(
            "Some plan details may be incomplete. Update COINCUBE Desktop to see the latest plans and prices.",
        ),
        None,
    )
}

/// Purchasing is closed server-side (a promotion is running).
fn purchasing_unavailable_note<'a>() -> Element<'a, Msg> {
    billing_notice(
        exclamation_circle_icon(),
        Tone::Muted,
        localized_copy("Purchasing is paused"),
        localized_copy(
            "Plans can't be bought right now. You can still compare what each plan includes.",
        ),
        None,
    )
}

/// The "Access" row for a campaign-granted plan, or `None` for purchased and
/// free plans. Label and badge are server-authored and launch-filtered.
pub(super) fn access_row<'a>(plan: &'a ConnectPlan) -> Option<Element<'a, Msg>> {
    let (label, badge) = plan_provenance_display(plan)?;
    let expires = plan
        .plan_provenance
        .as_ref()
        .and_then(|p| p.expires_at.as_deref())
        .filter(|e| !e.is_empty())
        .map(format_date);
    let mut title = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(tenshu_text::label(label));
    if let Some(badge) = badge {
        title = title.push(row_tag(badge, false));
    }
    let detail = match expires {
        Some(date) => format!("Granted by Coincube · expires {date}"),
        None => "Granted by Coincube".to_string(),
    };
    Some(summary_row(
        localized_copy("Access"),
        Column::new().push(title).push(lead(detail)).into(),
        Some(
            tenshu_text::body(localized_copy("No payment on file"))
                .size(13)
                .style(theme::tenshu::text::muted)
                .into(),
        ),
    ))
}

fn summary_row<'a>(
    key: &'a str,
    main: Element<'a, Msg>,
    trailing: Option<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let mut row = Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .push(lead(key).width(Length::Fixed(168.0)))
        .push(container(main).width(Length::Fill));
    if let Some(trailing) = trailing {
        row = row.push(trailing);
    }
    row.into()
}

/// What the "Current plan" row says: plan name, the date line and the status.
pub(super) fn current_plan_status(
    state: &ConnectAccountPanel,
) -> Option<(String, Option<String>, &'static str, Tone)> {
    let plan = state.plan.as_ref()?;
    let date = plan.renewal_at.as_deref().map(format_date);
    let cycle = plan.billing_cycle.map(cycle_label);
    let lifecycle = state.plan_lifecycle();
    let promo_until = plan
        .plan_provenance
        .as_ref()
        .and_then(|p| p.expires_at.as_deref())
        .filter(|e| !e.is_empty())
        .map(format_date)
        .or(date.clone());
    let pending = pending_change_line(plan);
    let card = plan.is_card();
    let next_charge = plan
        .next_charge_at
        .as_deref()
        .map(format_date)
        .or(date.clone());
    let (status, tone) = match (&lifecycle, &plan.status) {
        (PlanLifecycle::PastDue { .. }, _) => ("Past due", Tone::Warn),
        (PlanLifecycle::Expired, _) => ("Expired", Tone::Bad),
        (_, PlanStatus::Canceled) => ("Cancelled", Tone::Muted),
        (PlanLifecycle::RenewalDue { .. }, _) => ("Renewal due", Tone::Warn),
        _ => ("Active", Tone::Good),
    };
    let with_cycle = |text: String| match cycle {
        Some(c) => format!("{text} · {c}"),
        None => text,
    };
    let line = match (&lifecycle, &plan.status) {
        (PlanLifecycle::PastDue { .. }, _) => Some(with_cycle(match &date {
            Some(d) => format!("Renewal payment due {d}"),
            None => "Renewal payment due".to_string(),
        })),
        _ if card && plan.pending_plan.is_some() => {
            let pending = plan.pending_plan.as_ref().expect("checked");
            let at = plan
                .pending_plan_at
                .as_deref()
                .map(format_date)
                .or(date.clone());
            let then = match plan.pending_billing_cycle {
                Some(c) => format!("{pending} · {}", cycle_label(c)),
                None => pending.to_string(),
            };
            Some(match at {
                Some(d) => format!("{} until {d}, then {then}", plan.tier()),
                None => format!("Then {then}"),
            })
        }
        _ if card && plan.auto_renew == Some(false) => Some(match &date {
            Some(d) => format!("Active until {d} · auto-renewal off"),
            None => "Auto-renewal off".to_string(),
        }),
        _ if card => Some(with_cycle(match &next_charge {
            Some(d) => format!("Renews automatically on {d}"),
            None => "Renews automatically".to_string(),
        })),
        (PlanLifecycle::Expired, _) => Some(match &date {
            Some(d) => format!("Expired {d} · now on Free features"),
            None => "Now on Free features".to_string(),
        }),
        (_, PlanStatus::Canceled) => date.map(|d| format!("Access until {d}")),
        _ if plan.plan_provenance.is_some() => {
            promo_until.map(|d| format!("Promotional access until {d}"))
        }
        (PlanLifecycle::Free, _) => Some("Free features · no payment".to_string()),
        _ => match (date, cycle) {
            (Some(d), Some(c)) => Some(format!("Paid through {d} · {c}")),
            (Some(d), None) => Some(format!("Paid through {d}")),
            (None, Some(c)) => Some(c.to_string()),
            (None, None) => None,
        },
    };
    // A card's scheduled change is already in its line.
    let pending = pending.filter(|_| !card);
    let line = match (line, pending) {
        (Some(line), Some(pending)) => Some(format!("{line} · {pending}")),
        (line, pending) => line.or(pending),
    };
    Some((plan.tier().to_string(), line, status, tone))
}

fn status_icon<'a>(tone: Tone) -> iced::widget::Text<'a, theme::Theme> {
    match tone {
        Tone::Good => check_circle_icon(),
        Tone::Bad => x_circle_icon(),
        Tone::Warn => exclamation_circle_icon(),
        _ => dash_circle_icon(),
    }
}

/// The summary list: Current plan, then Access for a granted plan.
fn current_plan_summary<'a>(state: &'a ConnectAccountPanel) -> Element<'a, Msg> {
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    match current_plan_status(state) {
        Some((name, line, status, tone)) => {
            // The plan and its date sit on the row's true center (COIN-668,
            // COIN-669, master 7ba2a1d1e): equal Fill columns either side.
            let mut main = Column::new()
                .push(tenshu_text::label(name))
                .align_x(Alignment::Center);
            if let Some(line) = line {
                main = main.push(lead(line));
            }
            rows.push(
                Row::new()
                    .spacing(16)
                    .align_y(Alignment::Center)
                    .push(lead(localized_copy("Current plan")).width(Length::Fill))
                    .push(main)
                    .push(
                        container(status_line(status_icon(tone), status, tone))
                            .align_right(Length::Fill),
                    )
                    .into(),
            );
        }
        None => rows.push(summary_row(
            localized_copy("Current plan"),
            lead(localized_copy("Checking your current plan…")).into(),
            None,
        )),
    }
    if let Some(access) = state.plan.as_ref().and_then(access_row) {
        rows.push(access);
    }
    if let Some(payment) = payment_row(state) {
        rows.push(payment);
    }
    ruled_list(rows)
}

/// The card line under "Payment" (board 1, card states).
pub(super) fn card_line(state: &ConnectAccountPanel) -> Option<String> {
    let plan = state.plan.as_ref().filter(|p| p.is_card())?;
    let usd = plan.next_charge_amount.map(format_usd);
    let next = plan.next_charge_at.as_deref().map(format_date);
    Some(match state.plan_lifecycle() {
        PlanLifecycle::PastDue { grace_ends_at } => format!(
            "Renewal declined · Stripe retries until {}",
            short_date(grace_ends_at)
        ),
        _ if plan.auto_renew == Some(false) => "Won’t be charged again".to_string(),
        _ => match (usd, next, plan.pending_plan.as_ref()) {
            (Some(usd), Some(next), Some(pending)) => {
                format!("Next charge ${usd} for {pending} on {next}")
            }
            (Some(usd), Some(next), None) => format!("Next charge ${usd} on {next}"),
            (None, Some(next), _) => format!("Next charge on {next}"),
            _ => "Renews automatically".to_string(),
        },
    })
}

/// "Payment · Visa •••• 4242 [Stripe] · Update card" for a card plan.
fn payment_row<'a>(state: &'a ConnectAccountPanel) -> Option<Element<'a, Msg>> {
    let plan = state.plan.as_ref()?;
    let label = plan.card_label()?;
    let title = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(tenshu_text::label(label))
        .push(row_tag("Stripe", false));
    let mut main = Column::new().push(title);
    if let Some(line) = card_line(state) {
        main = main.push(lead(line));
    }
    Some(summary_row(
        localized_copy("Payment"),
        main.into(),
        Some(
            sheet_link(localized_copy("Update card"))
                .on_press_maybe(
                    (!state.card_request_busy && state.card_payments_enabled())
                        .then_some(Msg::OpenCardPortal),
                )
                .into(),
        ),
    ))
}

/// Card notices on Plan & billing: past due (C7), turn off auto-renewal
/// (C8) and a failed card request.
fn card_notices<'a>(state: &'a ConnectAccountPanel) -> Vec<Element<'a, Msg>> {
    let mut out = Vec::new();
    let Some(plan) = state.plan.as_ref() else {
        return out;
    };
    let busy = state.card_request_busy;
    if let Some(error) = &state.card_request_error {
        out.push(billing_notice(
            exclamation_circle_icon(),
            Tone::Bad,
            localized_copy("That didn’t go through"),
            format!("{error}. Nothing changed; try again."),
            None,
        ));
    }
    if let PlanLifecycle::PastDue { grace_ends_at } = state.plan_lifecycle() {
        let until = short_date(grace_ends_at);
        let free_limit = fallback_entitlements(&PlanTier::Free).cube_limit;
        out.push(billing_notice(
            exclamation_circle_icon(),
            Tone::Bad,
            localized_copy("Card renewal payment failed"),
            format!(
                "Your card was declined for the renewal. {tier} stays on until {until} while \
                 Stripe retries the card. Update your card or pay with bitcoin before then. \
                 After {until} the account moves to Free: all your Cubes stay, but only \
                 {free_limit} per network keeps Connect features.",
                tier = plan.tier()
            ),
            Some(actions_row(vec![
                sheet_button(localized_copy("Update card"), ActionKind::Primary, true)
                    .on_press_maybe(
                        (!busy && state.card_payments_enabled()).then_some(Msg::OpenCardPortal),
                    )
                    .into(),
                sheet_button(
                    localized_copy("Pay with bitcoin"),
                    ActionKind::Secondary,
                    true,
                )
                .on_press_maybe(
                    state
                        .purchasing_enabled()
                        .then(|| Msg::ReviewPlanChange(plan.tier().clone())),
                )
                .into(),
            ])),
        ));
    }
    if state.confirm_auto_renew_off && plan.card_auto_renews() {
        let until = plan
            .renewal_at
            .as_deref()
            .map(format_date)
            .unwrap_or_else(|| "the end of this period".to_string());
        out.push(billing_notice(
            dash_circle_icon(),
            Tone::Muted,
            localized_copy("Turn off auto-renewal?"),
            format!(
                "{} stays active until {until} and your card won't be charged again. Before \
                 then you can renew with bitcoin or turn auto-renewal back on.",
                plan.tier()
            ),
            Some(actions_row(vec![
                sheet_button(
                    localized_copy("Turn off auto-renewal"),
                    ActionKind::Secondary,
                    true,
                )
                .on_press_maybe(
                    (!busy && state.card_payments_enabled()).then_some(Msg::SetAutoRenew(false)),
                )
                .into(),
                sheet_button(localized_copy("Keep it on"), ActionKind::Primary, true)
                    .on_press(Msg::ShowAutoRenewOff(false))
                    .into(),
            ])),
        ));
    }
    out
}

/// Availability of one row in a feature list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FeatureValue {
    Included,
    /// A limit or other text, drawn as included ("Up to 3").
    Value(String),
    /// Not on this plan ("1 on Free" for an expired plan's Cube row).
    Muted(String),
    ComingSoon,
}

/// "Features on this account" (board 1): "Cubes per network" first, then
/// included features, then Coming soon rows (master decisions 9 and 10).
/// Built from the account's tier in `/connect/features` (bullets and
/// entitlements), with offline fallbacks.
pub(super) fn account_feature_rows(state: &ConnectAccountPanel) -> Vec<(String, FeatureValue)> {
    let Some(plan) = state.plan.as_ref() else {
        return Vec::new();
    };
    // A lapsed plan keeps its old tier name with Free entitlements (I4):
    // list what the account has now.
    let lapsed = matches!(state.plan_lifecycle(), PlanLifecycle::Expired);
    let tier = if lapsed { &PlanTier::Free } else { plan.tier() };
    let ent = if lapsed && plan.entitlements.cube_limit > 0 {
        plan.entitlements.clone()
    } else {
        entitlements_for(state, tier)
    };
    let pending_limit = plan
        .pending_plan
        .as_ref()
        .filter(|_| !lapsed)
        .and_then(|p| {
            let date = short_date(
                chrono::DateTime::parse_from_rfc3339(plan.pending_plan_at.as_deref()?)
                    .ok()?
                    .with_timezone(&chrono::Utc),
            );
            Some((entitlements_for(state, p).cube_limit, date))
        });
    let mut rows = vec![(
        "Cubes per network".to_string(),
        match (tier, ent.cube_limit, pending_limit) {
            (PlanTier::Free, n, _) => FeatureValue::Muted(format!("{n} on Free")),
            (_, n, Some((m, date))) if m != n => {
                FeatureValue::Value(format!("Up to {n} · {m} from {date}"))
            }
            (_, n, _) => FeatureValue::Value(format!("Up to {n}")),
        },
    )];
    let mut coming_soon = Vec::new();
    if let Some(info) = plan_info(state, tier) {
        for feature in &info.features {
            let Some((name, value)) = launch_feature_copy(feature) else {
                continue;
            };
            if rows
                .iter()
                .chain(coming_soon.iter())
                .any(|(n, _)| n == &name)
            {
                continue;
            }
            match value {
                FeatureValue::ComingSoon => coming_soon.push((name, value)),
                value => rows.push((name, value)),
            }
        }
    }
    // Recovery alerts are on every tier (decision 10); inheritance escrow is
    // Estate's. Both are entitlements, so list them even when the catalog's
    // bullets don't.
    if ent.recovery_alerts && !rows.iter().any(|(n, _)| n == "Recovery alerts") {
        rows.push(("Recovery alerts".to_string(), FeatureValue::Included));
    }
    if ent.inheritance_escrow && !rows.iter().any(|(n, _)| n == "Inheritance escrow") {
        rows.push(("Inheritance escrow".to_string(), FeatureValue::Included));
    }
    if *tier != PlanTier::Free && !coming_soon.iter().any(|(n, _)| n == SPENDING_POLICIES) {
        coming_soon.push((SPENDING_POLICIES.to_string(), FeatureValue::ComingSoon));
    }
    rows.extend(coming_soon);
    rows
}

pub(super) const SPENDING_POLICIES: &str = "Connect-enforced spending policies";

fn feature_value_line<'a>(value: &FeatureValue) -> Element<'a, Msg> {
    match value {
        FeatureValue::Included => {
            status_line(check_circle_icon(), localized_copy("Included"), Tone::Good)
        }
        FeatureValue::Value(text) => status_line(check_circle_icon(), text.clone(), Tone::Good),
        FeatureValue::Muted(text) => status_line(dash_circle_icon(), text.clone(), Tone::Muted),
        FeatureValue::ComingSoon => status_line(
            dash_circle_icon(),
            localized_copy("Coming soon"),
            Tone::Muted,
        ),
    }
}

/// The promo / referral code field (Tenshu text field + Redeem). Forwards
/// the code to `POST /connect/campaigns/redeem` and shows the server's reply.
fn campaign_redeem_field<'a>(state: &'a ConnectAccountPanel) -> Element<'a, Msg> {
    let rs = &state.campaign_redeem;
    let valid = !rs.code.trim().is_empty();
    let mut input = tenshu::text_field(
        "tenshu.connect.billing.promo-code",
        localized_copy("Promo or referral code"),
        &rs.code,
        Msg::FocusAction,
    )
    .placeholder(localized_copy("Promo or referral code"))
    .hide_label()
    .size(FieldSize::Compact)
    // Locked while a redeem is in flight so the code can't drift from the
    // result that comes back.
    .on_input_maybe((!rs.submitting).then_some(Msg::CampaignCodeChanged));
    if valid && !rs.submitting {
        input = input.on_submit(Msg::RedeemCampaignCode);
    }
    let submit = sheet_button(
        if rs.submitting {
            localized_copy("Redeeming…")
        } else {
            localized_copy("Redeem")
        },
        ActionKind::Secondary,
        false,
    )
    .on_press_maybe((valid && !rs.submitting).then_some(Msg::RedeemCampaignCode));
    let mut col = Column::new().spacing(8).push(
        container(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(container(input).width(Length::Fill))
                .push(submit),
        )
        // Flush with the sheet's edges, like the ruled rows (COIN-669).
        .width(Length::Fill),
    );
    if let Some(result) = &rs.result {
        let (msg, tone) = match result {
            Ok(m) => (coincube_ui::locale::notice(m.clone()), Tone::Good),
            Err(e) => (coincube_ui::locale::notice(e.clone()), Tone::Bad),
        };
        col = col.push(
            tenshu_text::body(msg)
                .size(13)
                .style(move |t| theme::tenshu::text::tone(t, tone)),
        );
    }
    col.into()
}

/// History status text and tone for a charge state.
pub(super) fn history_status(state: ChargeState) -> (&'static str, Tone) {
    match state {
        ChargeState::Paid => ("Paid", Tone::Good),
        ChargeState::Processing => ("Confirming", Tone::Warn),
        ChargeState::PartiallyPaid => ("Partly paid", Tone::Warn),
        ChargeState::NeedsSupport => ("Needs support", Tone::Bad),
        ChargeState::Unpaid => ("Unpaid", Tone::Muted),
        ChargeState::Expired => ("Expired", Tone::Muted),
        ChargeState::Unknown => ("Pending", Tone::Muted),
    }
}

/// Date, item, amount and status of one history entry (board 1 / D4).
pub(super) fn history_row_parts(
    entry: &BillingHistoryEntry,
) -> (String, String, String, &'static str, Tone) {
    let date = entry
        .paid_at
        .as_deref()
        .unwrap_or(entry.created_at.as_str());
    let item = format!(
        "{} · {} · bitcoin",
        entry.plan,
        cycle_label(entry.billing_cycle)
    );
    let amount = format!(
        "${} · {} sats",
        format_usd(entry.amount_fiat),
        group_sats(entry.amount_sats)
    );
    let (status, tone) = history_status(entry.charge_state());
    (format_date(date), item, amount, status, tone)
}

pub(super) fn history_row<'a>(entry: &BillingHistoryEntry) -> Element<'a, Msg> {
    let (date, item, amount, status, tone) = history_row_parts(entry);
    let mut main = Column::new().push(tenshu_text::label(item));
    if let Some(detail) = history_detail(entry) {
        main = main.push(
            tenshu_text::mono(detail)
                .size(12)
                .style(theme::tenshu::text::muted),
        );
    }
    Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .push(lead(date).width(Length::Fixed(120.0)))
        .push(main.width(Length::Fill))
        .push(tenshu_text::body(amount).size(13))
        .push(
            container(status_line(status_icon(tone), status, tone))
                .width(Length::Fixed(132.0))
                .align_right(Length::Fixed(132.0)),
        )
        .into()
}

/// The invoice line under a history row that needs it: support needs the
/// invoice id for Needs support, and a partly paid or overpaid row says how
/// much arrived. Keyed by the API's `chargeId` (billing v2 A1), with
/// `invoiceId` preferred when sent.
pub(super) fn history_detail(entry: &BillingHistoryEntry) -> Option<String> {
    let id = entry
        .invoice_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .unwrap_or(entry.charge_id.as_str());
    match entry.charge_state() {
        ChargeState::NeedsSupport | ChargeState::PartiallyPaid if !id.is_empty() => {
            Some(if entry.amount_received_sats > 0 {
                format!(
                    "Invoice {id} · {} sats received",
                    group_sats(entry.amount_received_sats)
                )
            } else {
                format!("Invoice {id}")
            })
        }
        ChargeState::Paid if entry.overpaid_sats > 0 && !id.is_empty() => Some(format!(
            "Invoice {id} · overpaid by {} sats",
            group_sats(entry.overpaid_sats)
        )),
        _ => None,
    }
}

/// The billing history page (D4): every charge as a ruled list.
pub(super) fn billing_history_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, Msg> {
    let mut col = Column::new()
        .width(Length::Fill)
        .push(back_link(
            localized_copy("Plan & billing"),
            Msg::ToggleBillingHistory,
        ))
        .push(sheet_title(localized_copy("Billing history")))
        .push(lead(localized_copy(
            "Every bitcoin invoice on this account. Contact support with the invoice ID for any \
             payment marked Needs support.",
        )))
        .push(space(20.0));
    col = match &state.billing_history {
        None => col.push(lead(localized_copy("Loading billing history…"))),
        Some(entries) if entries.is_empty() => col.push(lead(localized_copy(
            "No payments yet. Invoices you create appear here.",
        ))),
        Some(entries) => col.push(ruled_list(entries.iter().map(history_row))),
    };
    col.push(space(12.0))
        .push(footnote(localized_copy(
            "Payments use Lightning or on-chain bitcoin through BTCPay. Amounts in sats are fixed \
             when the invoice is created.",
        )))
        .into()
}

/// USD with cents only when there are some ("100", "33.15").
pub(super) fn format_usd(amount: f64) -> String {
    if (amount.fract()).abs() < 0.005 {
        format!("{amount:.0}")
    } else {
        format!("{amount:.2}")
    }
}

/// Sats with thin thousands separators ("41,000").
pub(super) fn group_sats(sats: u64) -> String {
    let digits = sats.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn billing_management_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, Msg> {
    let cancelled = state
        .plan
        .as_ref()
        .is_some_and(|plan| matches!(plan.status, PlanStatus::Canceled));
    let card_plan = state.plan.as_ref().filter(|p| p.is_card());
    // Card copy appears once cards are offered or the plan is on one.
    let card_ui = state.card_payments_enabled() || card_plan.is_some();
    let mut col = Column::new()
        .width(Length::Fill)
        .push(sheet_title(localized_copy("Plan & billing")))
        .push(lead(if card_ui {
            localized_copy("Pay upfront in bitcoin, or by card with automatic renewal.")
        } else {
            localized_copy("Plans are paid upfront in bitcoin. There is no recurring subscription.")
        }))
        .push(space(20.0))
        .push(current_plan_summary(state));
    if cancelled {
        col = col.push(space(12.0)).push(footnote(localized_copy(
            "Your plan is cancelled. Connect services stay on until the access date.",
        )));
    }
    let mut actions: Vec<Element<'a, Msg>> = vec![sheet_button(
        if cancelled {
            localized_copy("Resubscribe…")
        } else {
            localized_copy("Change plan…")
        },
        ActionKind::Secondary,
        true,
    )
    .on_press(Msg::ComparePlans(true))
    .into()];
    if let Some(plan) = card_plan.filter(|_| state.card_payments_enabled()) {
        let busy = state.card_request_busy;
        if plan.auto_renew == Some(false) {
            actions.push(
                sheet_link(localized_copy("Turn auto-renewal back on"))
                    .on_press_maybe((!busy).then_some(Msg::SetAutoRenew(true)))
                    .into(),
            );
        } else if !state.confirm_auto_renew_off {
            actions.push(
                sheet_link(localized_copy("Turn off auto-renewal"))
                    .on_press_maybe((!busy).then_some(Msg::ShowAutoRenewOff(true)))
                    .into(),
            );
        }
    }
    col = col.push(space(12.0)).push(actions_row({
        let mut items = actions;
        items.push(
            sheet_link(if state.show_campaign_redeem {
                localized_copy("Hide code")
            } else {
                localized_copy("Have a promo or referral code?")
            })
            .on_press(Msg::ShowCampaignRedeem(!state.show_campaign_redeem))
            .into(),
        );
        items
    }));
    if state.show_campaign_redeem {
        col = col.push(space(12.0)).push(campaign_redeem_field(state));
    }
    for notice in card_notices(state) {
        col = col.push(space(20.0)).push(notice);
    }
    if let Some(notice) = pending_change_notice(state) {
        col = col.push(space(20.0)).push(notice);
    }
    if let Some(banner) = renewal_banner(state) {
        col = col.push(space(20.0)).push(banner);
    }
    if state.pricing_schema_outdated() {
        col = col.push(space(20.0)).push(schema_update_note());
    }
    if state.features.is_some() && !state.purchasing_enabled() {
        col = col.push(space(20.0)).push(purchasing_unavailable_note());
    }

    col = col
        .push(space(32.0))
        .push(group_title(localized_copy("Features on this account")))
        .push(space(8.0));
    let rows = account_feature_rows(state);
    if state.plan.is_none() {
        col = col.push(lead(localized_copy("Checking your current plan…")));
    } else if state.features.is_none() && rows.len() <= 1 {
        col = col.push(lead(localized_copy(
            "Current plan features are unavailable. Try again when Connect is online.",
        )));
    } else {
        col = col.push(ruled_list(rows.into_iter().map(|(name, value)| {
            Row::new()
                .spacing(16)
                .align_y(Alignment::Center)
                .push(tenshu_text::label(name).width(Length::Fill))
                .push(feature_value_line(&value))
                .into()
        })));
    }

    col = col
        .push(space(32.0))
        .push(group_title(localized_copy("Billing history")))
        .push(space(8.0));
    match &state.billing_history {
        Some(entries) if !entries.is_empty() => {
            col = col.push(ruled_list(entries.iter().take(3).map(history_row)));
        }
        Some(_) => {
            col = col.push(lead(localized_copy("No payments yet.")));
        }
        None => {}
    }
    col.push(space(12.0))
        .push(
            sheet_link(localized_copy("View all billing history"))
                .on_press(Msg::ToggleBillingHistory),
        )
        .push(space(12.0))
        .push(footnote(if card_ui {
            localized_copy(
                "Card payments are processed by Stripe; card details are never stored in \
                 Coincube. Bitcoin payments use BTCPay.",
            )
        } else {
            localized_copy(
                "Payments use Lightning or on-chain bitcoin. Renewal reminders are sent by email.",
            )
        }))
        .into()
}

/// "then Pro · Annual from Mar 14, 2027" for a scheduled lower tier.
fn pending_change_line(plan: &ConnectPlan) -> Option<String> {
    let pending = plan.pending_plan.as_ref()?;
    let date = format_date(plan.pending_plan_at.as_deref()?);
    Some(match plan.pending_billing_cycle {
        Some(c) => format!("then {pending} · {} from {date}", cycle_label(c)),
        None => format!("then {pending} from {date}"),
    })
}

/// A scheduled lower-tier plan (billing v2 A4, `pendingPlan`).
fn pending_change_notice<'a>(state: &'a ConnectAccountPanel) -> Option<Element<'a, Msg>> {
    let plan = state.plan.as_ref()?;
    let pending = plan.pending_plan.as_ref()?;
    let date = plan.pending_plan_at.as_deref().map(format_date)?;
    let cycle = plan
        .pending_billing_cycle
        .map(|c| format!(" · {}", cycle_label(c)))
        .unwrap_or_default();
    if plan.is_card() {
        // A scheduled card downgrade (C13): undo with "Keep Estate".
        let limit = entitlements_for(state, pending).cube_limit;
        let charge = plan
            .next_charge_amount
            .map(|usd| {
                format!(
                    " From {date} your card is charged ${} for {pending}.",
                    format_usd(usd)
                )
            })
            .unwrap_or_default();
        return Some(billing_notice(
            clock_icon(),
            Tone::Muted,
            format!("Changes to {pending}{cycle} on {date}"),
            format!(
                "You keep {} until then.{charge} If you have more than {limit} Cubes on a \
                 network then, every Cube keeps working, but only {limit} per network keep \
                 Connect features.",
                plan.tier()
            ),
            Some(
                sheet_button(format!("Keep {}", plan.tier()), ActionKind::Secondary, true)
                    .on_press_maybe(
                        (!state.card_request_busy && state.card_payments_enabled())
                            .then_some(Msg::KeepCurrentPlan),
                    )
                    .into(),
            ),
        ));
    }
    Some(billing_notice(
        clock_icon(),
        Tone::Muted,
        format!("Changes to {pending}{cycle} on {date}"),
        format!(
            "You keep {} until then. The {pending} period you paid for starts on {date}.",
            plan.tier()
        ),
        None,
    ))
}

// ── Change plan (board 2) ────────────────────────────────────────────────────

/// The plan a Change plan row describes, for the selected cycle.
struct PlanRow {
    tier: PlanTier,
    description: &'static str,
    price: Option<u32>,
}

fn plan_description(tier: &PlanTier) -> &'static str {
    match tier {
        PlanTier::Free => {
            "The essentials to create and recover your Cubes, with Esplora access, a \
             Lightning address and recovery alerts."
        }
        PlanTier::Pro => {
            "More Cubes per network, with Connect-enforced spending policies coming soon."
        }
        PlanTier::Estate => {
            "Maximum Cube capacity, inheritance escrow and collaborative multi-sig keyholder \
             invitations."
        }
    }
}

/// The tier `plan` effectively gives today: a lapsed plan (`past_due`, the
/// API's "already demoted") counts as Free, as does no plan at all.
pub(super) fn effective_tier(plan: Option<&ConnectPlan>) -> PlanTier {
    match plan {
        Some(plan)
            if !matches!(plan.status, PlanStatus::PastDue)
                || plan.grace_until(chrono::Utc::now()).is_some() =>
        {
            plan.tier().clone()
        }
        _ => PlanTier::Free,
    }
}

fn current_tier(state: &ConnectAccountPanel) -> PlanTier {
    effective_tier(state.plan.as_ref())
}

/// Whether a Change plan row is the account's current plan, for the
/// "Current" tag. `/connect/plan` does not send the billing cycle today, so
/// an unknown cycle tags the tier on both periods.
fn is_current(state: &ConnectAccountPanel, tier: &PlanTier, cycle: BillingCycle) -> bool {
    *tier == current_tier(state)
        && (*tier == PlanTier::Free
            || state
                .plan
                .as_ref()
                .and_then(|p| p.billing_cycle)
                .is_none_or(|c| c == cycle))
}

/// The future end of the current paid period, if any.
fn paid_until(plan: Option<&ConnectPlan>) -> Option<chrono::DateTime<chrono::Utc>> {
    plan.and_then(|p| p.renewal_at.as_deref())
        .and_then(|r| chrono::DateTime::parse_from_rfc3339(r).ok())
        .map(|r| r.with_timezone(&chrono::Utc))
        .filter(|r| *r > chrono::Utc::now())
}

/// The Change plan CTA label, the line under it and whether it is enabled
/// (board 2 logic plus the API's rules for buying a lower tier ahead).
pub(super) fn change_plan_cta(
    state: &ConnectAccountPanel,
    selected: &PlanTier,
    cycle: BillingCycle,
) -> (String, String, bool) {
    let current = current_tier(state);
    let plan = state.plan.as_ref();
    let blocked = |label: String, why: &str| (label, why.to_string(), false);
    if *selected == current && current == PlanTier::Free {
        return blocked(
            "Continue to review".to_string(),
            "This is your current plan. Pick another plan to continue.",
        );
    }
    // Switching to Free costs nothing and needs no purchase.
    if *selected == PlanTier::Free {
        return (
            "Review switch to Free".to_string(),
            "You’ll see when the switch takes effect before you confirm it.".to_string(),
            true,
        );
    }
    if !state.purchasing_enabled() {
        return blocked(
            "Continue to review".to_string(),
            "Purchasing is paused, so plan changes can’t be started right now.",
        );
    }
    if let Some(card) = plan.filter(|p| p.is_card()) {
        let renews = card.auto_renew == Some(true);
        let higher = rank(selected) > rank(&current);
        // A card that starts after a paid period (trialing; the API sends no
        // cycle then) can't change until it starts.
        if card.billing_cycle.is_none() && (renews || higher) {
            return blocked(
                "Continue to review".to_string(),
                "Your card subscription starts when your paid period ends; the plan can change \
                 after that.",
            );
        }
        if matches!(state.plan_lifecycle(), PlanLifecycle::PastDue { .. }) && *selected != current {
            return blocked(
                "Continue to review".to_string(),
                "Update your card first: the plan can change once the renewal is paid. You can \
                 renew this plan with bitcoin.",
            );
        }
        if !state.card_payments_enabled() && (renews || higher) {
            return blocked(
                "Continue to review".to_string(),
                "Card plan changes are unavailable right now.",
            );
        }
    }
    if let Some(card) = plan.filter(|_| state.card_change_route(selected, cycle)) {
        // Through the card subscription (B3): a higher tier or monthly →
        // annual now, prorated; anything else at the period end.
        let current_cycle = card.billing_cycle;
        if *selected == current && current_cycle.is_none_or(|c| c == cycle) {
            return blocked(
                "Continue to review".to_string(),
                "This is your current plan; it renews automatically by card.",
            );
        }
        let upgrade = rank(selected) > rank(&current)
            || (*selected == current
                && cycle == BillingCycle::Annual
                && current_cycle == Some(BillingCycle::Monthly));
        if !upgrade {
            if let Some(pending) = card.pending_plan.as_ref() {
                return blocked(
                    format!("Review change to {selected} · {}", cycle_label(cycle)),
                    &format!("A change to {pending} is already scheduled. Keep your plan first to pick another."),
                );
            }
        }
        return (
            if upgrade {
                format!("Review upgrade to {selected}")
            } else {
                format!("Review change to {selected} · {}", cycle_label(cycle))
            },
            if upgrade {
                "You’ll see what’s due today, with credit for unused time, before your card is charged."
                    .to_string()
            } else {
                "The change starts when this period ends. Nothing is charged or refunded today."
                    .to_string()
            },
            true,
        );
    }
    let sats_note =
        "You’ll see the amount in sats and choose Lightning or on-chain before anything is charged.";
    if *selected == current {
        // Buying the current tier again adds a period after this one (the
        // API stacks same-tier purchases), whatever the cycle.
        let until = paid_until(plan).map(short_date);
        let in_grace = plan.is_some_and(|p| matches!(p.status, PlanStatus::PastDue));
        if in_grace {
            return (
                format!("Review renewal · {selected} · {}", cycle_label(cycle)),
                format!(
                    "Pays for a new {} period starting today. {sats_note}",
                    cycle_label(cycle).to_lowercase()
                ),
                true,
            );
        }
        return (
            format!("Review renewal · {selected} · {}", cycle_label(cycle)),
            match until {
                Some(date) => format!(
                    "Adds a {} period after your current one ends on {date}. {sats_note}",
                    cycle_label(cycle).to_lowercase()
                ),
                None => sats_note.to_string(),
            },
            true,
        );
    }
    if rank(selected) < rank(&current) {
        if let Some(pending) = plan.and_then(|p| p.pending_plan.as_ref()) {
            let date = plan
                .and_then(|p| p.pending_plan_at.as_deref())
                .map(format_date)
                .unwrap_or_else(|| "the end of this period".to_string());
            return (
                format!("Review switch to {selected} · {}", cycle_label(cycle)),
                format!("A change to {pending} is already scheduled for {date}."),
                false,
            );
        }
        if paid_until(plan).is_none() {
            return blocked(
                format!("Review switch to {selected} · {}", cycle_label(cycle)),
                "A lower plan starts when your current paid period ends, and this plan has no \
                 end date to follow.",
            );
        }
        return (
            format!("Review switch to {selected} · {}", cycle_label(cycle)),
            format!("{selected} starts when your current period ends. {sats_note}"),
            true,
        );
    }
    (
        format!("Review upgrade to {selected}"),
        sats_note.to_string(),
        true,
    )
}

/// Comparison table rows (board 2): name and one cell per tier.
pub(super) fn comparison_rows(
    state: &ConnectAccountPanel,
) -> Vec<(&'static str, [FeatureValue; 3])> {
    let ents = TIERS.map(|tier| entitlements_for(state, &tier));
    let each = |f: &dyn Fn(&PlanEntitlements) -> FeatureValue| -> [FeatureValue; 3] {
        [f(&ents[0]), f(&ents[1]), f(&ents[2])]
    };
    let yes_no = |on: bool| {
        if on {
            FeatureValue::Included
        } else {
            FeatureValue::Muted("—".to_string())
        }
    };
    vec![
        (
            "Create and recover Cubes",
            each(&|_| FeatureValue::Included),
        ),
        (
            "Cubes per network",
            each(&|e| FeatureValue::Value(e.cube_limit.to_string())),
        ),
        ("Hosted Esplora server", each(&|_| FeatureValue::Included)),
        (
            "Lightning Address for Spark",
            each(&|_| FeatureValue::Included),
        ),
        ("Recovery alerts", each(&|e| yes_no(e.recovery_alerts))),
        (
            "Inheritance escrow",
            each(&|e| yes_no(e.inheritance_escrow)),
        ),
        (
            "Keyholder invitations",
            each(&|e| yes_no(e.collaborative_invitations)),
        ),
        (
            SPENDING_POLICIES,
            each(&|e| {
                if e.attach_policies {
                    FeatureValue::ComingSoon
                } else {
                    FeatureValue::Muted("—".to_string())
                }
            }),
        ),
    ]
}

fn comparison_table<'a>(state: &'a ConnectAccountPanel, selected: &PlanTier) -> Element<'a, Msg> {
    let sel = TIERS.iter().position(|t| t == selected);
    let cell_width = Length::Fixed(96.0);
    let cell = move |i: usize, content: Element<'a, Msg>| -> Element<'a, Msg> {
        let picked = sel == Some(i);
        container(content)
            .width(cell_width)
            .center_x(cell_width)
            .padding([10, 0])
            .style(move |t: &theme::Theme| container::Style {
                background: picked.then_some(iced::Background::Color(t.colors.shell.selected)),
                ..Default::default()
            })
            .into()
    };
    let mut header = Row::new()
        .align_y(Alignment::Center)
        .push(footnote(localized_copy("Feature")).width(Length::Fill));
    for (i, tier) in TIERS.iter().enumerate() {
        let picked = sel == Some(i);
        let label = tenshu_text::caption(tier.to_string());
        header = header.push(
            container(if picked {
                label
                    .font(coincube_ui::font::SEMIBOLD)
                    .style(theme::tenshu::text::ink)
            } else {
                label
            })
            .width(cell_width)
            .center_x(cell_width)
            .padding(iced::Padding::default().bottom(8.0)),
        );
    }
    let mut table = Column::new().push(header).push(horizontal_rule());
    for (name, cells) in comparison_rows(state) {
        let mut row = Row::new()
            .align_y(Alignment::Center)
            .push(tenshu_text::label(name).width(Length::Fill));
        for (i, value) in cells.iter().enumerate() {
            let content: Element<'a, Msg> = match value {
                FeatureValue::Included => check_circle_icon()
                    .size(16)
                    .style(|t| theme::tenshu::text::tone(t, Tone::Good))
                    .into(),
                FeatureValue::Value(text) | FeatureValue::Muted(text) => {
                    footnote(text.clone()).into()
                }
                FeatureValue::ComingSoon => footnote(localized_copy("Coming soon")).into(),
            };
            row = row.push(cell(i, content));
        }
        table = table.push(row).push(horizontal_rule());
    }
    table.width(Length::Fill).into()
}

fn back_link<'a>(label: &'a str, on_press: Msg) -> Element<'a, Msg> {
    iced::widget::button(
        Row::new()
            .spacing(6)
            .align_y(Alignment::Center)
            .push(previous_icon().size(14).style(theme::tenshu::text::muted))
            .push(
                tenshu_text::body(label)
                    .size(13)
                    .style(theme::tenshu::text::muted),
            ),
    )
    .padding([6, 0])
    .style(|t, s| {
        theme::tenshu::button::action(
            t,
            s,
            ActionKind::Quiet,
            theme::tenshu::button::ActionShape::Compact,
        )
    })
    .on_press(on_press)
    .into()
}

fn change_plan_ux<'a>(state: &'a ConnectAccountPanel) -> Element<'a, Msg> {
    let cycle = state.selected_billing_cycle;
    let current = current_tier(state);
    let selected = state
        .selected_plan_tier
        .clone()
        .unwrap_or_else(|| current.clone());
    let lead_text = match (state.plan.as_ref(), state.plan_lifecycle()) {
        (Some(plan), PlanLifecycle::Active | PlanLifecycle::RenewalDue { .. }) => {
            match (
                plan.billing_cycle,
                plan.renewal_at.as_deref().map(format_date),
            ) {
                (Some(c), Some(d)) => format!(
                    "You're on {} · {}, paid through {d}. Nothing is charged until you confirm \
                     the amount on the next screen.",
                    plan.tier(),
                    cycle_label(c)
                ),
                _ => format!(
                    "You're on {}. Nothing is charged until you confirm the amount on the next \
                     screen.",
                    plan.tier()
                ),
            }
        }
        _ => "You're on Free. Nothing is charged until you confirm the amount on the next screen."
            .to_string(),
    };
    let mut col = Column::new()
        .width(Length::Fill)
        .push(back_link(
            localized_copy("Plan & billing"),
            Msg::ComparePlans(false),
        ))
        .push(sheet_title(localized_copy("Change plan")))
        .push(lead(lead_text));

    if state.pricing_schema_outdated() {
        col = col.push(space(20.0)).push(schema_update_note());
    }
    if state.features.is_some() && !state.purchasing_enabled() {
        col = col.push(space(20.0)).push(purchasing_unavailable_note());
    }
    if let Some(banner) = renewal_banner(state) {
        col = col.push(space(20.0)).push(banner);
    }

    let priced = TIERS.iter().any(|t| plan_info(state, t).is_some());
    if priced {
        let cycle_note = match cycle {
            BillingCycle::Annual => "Paid once a year, upfront.",
            BillingCycle::Monthly => "Paid each month, upfront. No auto-renewal.",
        };
        col = col.push(space(20.0)).push(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(
                    tenshu::segmented(
                        [
                            tenshu::segment(
                                "tenshu.connect.billing.cycle.monthly",
                                BillingCycle::Monthly,
                                localized_copy("Monthly"),
                            ),
                            tenshu::segment(
                                "tenshu.connect.billing.cycle.annual",
                                BillingCycle::Annual,
                                localized_copy("Annual"),
                            ),
                        ],
                        Some(cycle),
                        Msg::FocusAction,
                    )
                    .size(tenshu::SegmentSize::Compact)
                    .plain()
                    .on_select(Msg::BillingCycleSelected),
                )
                .push(lead(localized_copy(cycle_note))),
        );
        let rows: Vec<PlanRow> = TIERS
            .iter()
            .filter(|tier| plan_info(state, tier).is_some())
            .map(|tier| PlanRow {
                tier: tier.clone(),
                description: plan_description(tier),
                price: price_for(state, tier, cycle),
            })
            .collect();
        let per = match cycle {
            BillingCycle::Monthly => "per month",
            BillingCycle::Annual => "per year",
        };
        let mut list = Column::new().push(space(16.0)).push(horizontal_rule());
        for row in rows {
            let id = match row.tier {
                PlanTier::Free => "tenshu.connect.billing.plan.free",
                PlanTier::Pro => "tenshu.connect.billing.plan.pro",
                PlanTier::Estate => "tenshu.connect.billing.plan.estate",
            };
            let price = match (row.price, &row.tier) {
                (Some(amount), _) => (format!("${amount}"), per),
                (None, PlanTier::Free) => ("Free".to_string(), "No payment"),
                // A paid tier without a price in the catalog is never "Free".
                (None, _) => ("—".to_string(), "Price unavailable"),
            };
            list = list
                .push(choice_row(
                    id,
                    row.tier.to_string(),
                    is_current(state, &row.tier, cycle).then_some(("Current", true)),
                    row.description.to_string(),
                    Some(price),
                    row.tier == selected,
                    true,
                    Some(Msg::SelectPlanTier(row.tier.clone())),
                ))
                .push(horizontal_rule());
        }
        col = col.push(list);
        let (label, conseq, can_continue) = change_plan_cta(state, &selected, cycle);
        col = col
            .push(space(20.0))
            .push(actions_row(vec![
                sheet_button(label, ActionKind::Primary, false)
                    .on_press_maybe(can_continue.then(|| Msg::ReviewPlanChange(selected.clone())))
                    .into(),
                sheet_button(localized_copy("Cancel"), ActionKind::Secondary, false)
                    .on_press(Msg::ComparePlans(false))
                    .into(),
            ]))
            .push(space(8.0))
            .push(lead(conseq));
    } else {
        col = col.push(space(20.0)).push(billing_notice(
            exclamation_circle_icon(),
            Tone::Warn,
            localized_copy("Plans are unavailable offline"),
            localized_copy("Reconnect to the internet to see current plans and prices."),
            None,
        ));
    }

    col.push(space(32.0))
        .push(group_title(localized_copy("What each plan includes")))
        .push(space(8.0))
        .push(comparison_table(state, &selected))
        .push(space(12.0))
        .push(footnote(localized_copy(
            "Connect-enforced spending policies are coming soon and are not sold yet. Recovery \
             alerts email your keyholders when a Vault’s recovery path opens; inheritance escrow \
             (Estate) also lets them recover the Vault or full Cube without the password.",
        )))
        .into()
}

// ── Review (board 3) ─────────────────────────────────────────────────────────

/// What the Review list says for a checkout: plan line, what it replaces,
/// amount, sats note and the dates it covers.
pub(super) struct ReviewFacts {
    pub plan: String,
    pub replaces: Option<String>,
    pub amount: Option<String>,
    pub covers: Option<String>,
    /// For a lower tier or a renewal: the date the paid period starts
    /// (decision 12; Connect stacks it onto the current period).
    pub starts: Option<String>,
    /// A lower tier already scheduled (A4 `pendingPlan`).
    pub pending: Option<PlanTier>,
    /// Facts the buyer should know before paying (an upgrade forfeits the
    /// rest of the current period; a scheduled change stays).
    pub notes: Vec<String>,
}

fn add_cycle(
    start: chrono::DateTime<chrono::Utc>,
    cycle: BillingCycle,
) -> chrono::DateTime<chrono::Utc> {
    let months = match cycle {
        BillingCycle::Monthly => 1,
        BillingCycle::Annual => 12,
    };
    start
        .checked_add_months(chrono::Months::new(months))
        .unwrap_or(start)
}

fn short_date(at: chrono::DateTime<chrono::Utc>) -> String {
    at.format("%b %-d, %Y").to_string()
}

/// Review facts for `checkout`, against `now`. Pure so it is testable.
///
/// Reads the plan as it was when Review opened (`CheckoutState::plan`), so
/// the Paid screen still describes the purchase after `/connect/plan`
/// reloads with the new tier.
pub(super) fn review_facts(
    state: &ConnectAccountPanel,
    checkout: &CheckoutState,
    now: chrono::DateTime<chrono::Utc>,
) -> ReviewFacts {
    let tier = &checkout.tier;
    let cycle = checkout.cycle;
    let plan = checkout.plan.as_ref();
    let current = effective_tier(plan);
    let renewal = plan
        .and_then(|p| p.renewal_at.as_deref())
        .and_then(|r| chrono::DateTime::parse_from_rfc3339(r).ok())
        .map(|r| r.with_timezone(&chrono::Utc))
        .filter(|r| *r > now);
    let replaces = plan.filter(|_| current != PlanTier::Free).map(|p| {
        if p.tier() == tier {
            format!("Adds to your current {tier} plan")
        } else {
            match p.billing_cycle {
                Some(c) => format!("Replaces {} · {}", p.tier(), cycle_label(c)),
                None => format!("Replaces {}", p.tier()),
            }
        }
    });
    // Connect stacks a same-tier renewal and a lower tier onto the current
    // period; an upgrade or a lapsed plan starts now (API A4 activatePlan).
    // Card grace time (past_due) isn't paid time: a purchase starts now.
    let in_grace = plan.is_some_and(|p| matches!(p.status, PlanStatus::PastDue));
    let stacks = current != PlanTier::Free && rank(tier) <= rank(&current) && !in_grace;
    let start = if stacks { renewal.unwrap_or(now) } else { now };
    let starts = if stacks {
        renewal.map(short_date)
    } else {
        None
    };
    let pending = plan.and_then(|p| p.pending_plan.clone());
    let mut notes = Vec::new();
    if current != PlanTier::Free && rank(tier) > rank(&current) {
        if let Some(until) = renewal {
            notes.push(format!(
                "{tier} starts today. Your remaining {current} time (until {}) isn’t credited.",
                short_date(until)
            ));
        }
        if let Some(pending) = &pending {
            notes.push(format!(
                "Your scheduled change to {pending} stays scheduled and starts when this new \
                 period ends."
            ));
        }
    }
    if *tier == PlanTier::Free {
        return ReviewFacts {
            plan: "Free".to_string(),
            replaces,
            amount: Some("Nothing to pay".to_string()),
            // With a paid lower tier scheduled, Free follows that period,
            // whose end isn't known here.
            covers: renewal
                .filter(|_| pending.is_none())
                .map(|r| format!("Free from {}", short_date(r))),
            starts: renewal.map(short_date),
            pending,
            notes,
        };
    }
    ReviewFacts {
        plan: format!("{tier} · {}", cycle_label(cycle)),
        replaces,
        amount: price_for(state, tier, cycle).map(|usd| format!("${usd} USD")),
        covers: Some(format!(
            "{} – {}",
            short_date(start),
            short_date(add_cycle(start, cycle))
        )),
        starts,
        pending,
        notes,
    }
}

fn review_ux<'a>(state: &'a ConnectAccountPanel, checkout: &'a CheckoutState) -> Element<'a, Msg> {
    let facts = review_facts(state, checkout, checkout.opened_at);
    let to_free = checkout.tier == PlanTier::Free;
    let paying_card = checkout.pay_with == PayWith::Card && card_offered(state, checkout);
    let mut plan_main = Column::new().push(tenshu_text::label(facts.plan.clone()));
    if let Some(replaces) = facts.replaces.clone() {
        plan_main = plan_main.push(lead(replaces));
    }
    let mut amount_main = Column::new().push(tenshu_text::label(
        facts
            .amount
            .clone()
            .unwrap_or_else(|| "Shown on the invoice".to_string()),
    ));
    if paying_card {
        amount_main = amount_main.push(lead(match checkout.cycle {
            BillingCycle::Annual => localized_copy("Charged to your card, then every year"),
            BillingCycle::Monthly => localized_copy("Charged to your card, then every month"),
        }));
    } else if !to_free {
        amount_main = amount_main.push(lead(localized_copy(
            "In sats at today’s rate, fixed for this invoice",
        )));
    }
    // Card dates follow the card subscription: it starts when a paid
    // period ends (or today) and then renews.
    let covers_text = if paying_card {
        let start = card_start_at(checkout).unwrap_or(checkout.opened_at);
        format!(
            "{} – {}, then renews",
            short_date(start),
            short_date(add_cycle(start, checkout.cycle))
        )
    } else {
        facts.covers.clone().unwrap_or_else(|| "—".to_string())
    };
    let mut covers_main = Column::new().push(tenshu_text::label(covers_text));
    let starts_line = if paying_card {
        card_start(checkout)
    } else {
        facts.starts.clone()
    };
    if let (Some(date), false) = (&starts_line, to_free) {
        covers_main = covers_main.push(lead(format!("Starts {date}")));
    }
    let list = ruled_list([
        summary_row(localized_copy("Plan"), plan_main.into(), None),
        summary_row(localized_copy("Amount"), amount_main.into(), None),
        summary_row(localized_copy("Covers"), covers_main.into(), None),
        summary_row(
            localized_copy("Status"),
            iced::widget::Space::new().into(),
            Some(status_line(
                dash_circle_icon(),
                if to_free {
                    localized_copy("Nothing to pay")
                } else if paying_card {
                    localized_copy("Not charged yet")
                } else {
                    localized_copy("Not invoiced yet")
                },
                Tone::Muted,
            )),
        ),
    ]);
    let col = Column::new()
        .width(Length::Fill)
        .push(back_link(
            localized_copy("Change plan"),
            Msg::ComparePlans(true),
        ))
        .push(sheet_title(localized_copy("Review plan change")))
        .push(lead(localized_copy(
            "Check the plan, amount and how you pay. Nothing is charged yet.",
        )))
        .push(space(20.0))
        .push(list);
    let mut col = col;
    // The notes describe how Connect applies a bitcoin purchase.
    if !paying_card {
        for note in &facts.notes {
            col = col.push(space(12.0)).push(lead(note.clone()));
        }
    }

    if to_free && checkout.plan.as_ref().is_some_and(|p| p.card_auto_renews()) {
        // On a card, switching to Free is turning auto-renewal off (C8).
        let until = facts
            .starts
            .clone()
            .unwrap_or_else(|| "the end of this period".to_string());
        return col
            .push(space(20.0))
            .push(billing_notice(
                clock_icon(),
                Tone::Muted,
                localized_copy("Turn off auto-renewal to switch to Free"),
                format!(
                    "Your card stops renewing and your plan stays on until {until}, then the \
                     account moves to Free. All your Cubes stay; Cubes over Free's limit lose \
                     Connect features."
                ),
                Some(actions_row(vec![
                    sheet_button(
                        localized_copy("Turn off auto-renewal…"),
                        ActionKind::Primary,
                        true,
                    )
                    .on_press_maybe(
                        state
                            .card_payments_enabled()
                            .then_some(Msg::ShowAutoRenewOff(true)),
                    )
                    .into(),
                    sheet_button(localized_copy("Back"), ActionKind::Secondary, true)
                        .on_press(Msg::ComparePlans(true))
                        .into(),
                ])),
            ))
            .into();
    }

    if to_free {
        let body = match (&facts.starts, &facts.pending) {
            (Some(date), Some(pending)) => format!(
                "Bitcoin plans don't renew by themselves, so there is nothing to cancel. Your \
                 current plan stays on until {date}, then the {pending} period you already paid \
                 for starts. The account moves to Free when that ends."
            ),
            (Some(date), None) => format!(
                "Bitcoin plans don't renew by themselves, so there is nothing to cancel. Your \
                 current plan stays on until {date}, then the account moves to Free. All your \
                 Cubes stay; Cubes over Free's limit lose Connect features."
            ),
            _ => "Bitcoin plans don't renew by themselves, so there is nothing to cancel."
                .to_string(),
        };
        return col
            .push(space(20.0))
            .push(billing_notice(
                clock_icon(),
                Tone::Muted,
                localized_copy("No payment needed"),
                body,
                Some(
                    sheet_button(
                        localized_copy("Back to Plan & billing"),
                        ActionKind::Secondary,
                        true,
                    )
                    .on_press(Msg::DismissCheckout)
                    .into(),
                ),
            ))
            .into();
    }

    let methods = Column::new()
        .push(horizontal_rule())
        .push(choice_row(
            "tenshu.connect.billing.pay.bitcoin",
            "Bitcoin".to_string(),
            None,
            match checkout.cycle {
                BillingCycle::Annual => "Lightning or on-chain, through BTCPay. One payment covers the year, and we email you before it ends. No auto-renewal.",
                BillingCycle::Monthly => "Lightning or on-chain, through BTCPay. One payment covers the month, and we email you before it ends. No auto-renewal.",
            }
            .to_string(),
            None,
            checkout.pay_with == PayWith::Bitcoin,
            true,
            Some(Msg::SelectPayWith(PayWith::Bitcoin)),
        ))
        .push(horizontal_rule())
        .push(card_choice(state, checkout, &facts))
        .push(horizontal_rule());

    let purchasing = state.purchasing_enabled();
    let card = checkout.pay_with == PayWith::Card && card_offered(state, checkout);
    col.push(space(24.0))
        .push(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(group_title(localized_copy("Billing period")).width(Length::Fill))
                .push(
                    tenshu::segmented(
                        [
                            tenshu::segment(
                                "tenshu.connect.billing.review.monthly",
                                BillingCycle::Monthly,
                                localized_copy("Monthly"),
                            ),
                            tenshu::segment(
                                "tenshu.connect.billing.review.annual",
                                BillingCycle::Annual,
                                localized_copy("Annual"),
                            ),
                        ],
                        Some(checkout.cycle),
                        Msg::FocusAction,
                    )
                    .size(tenshu::SegmentSize::Compact)
                    .plain()
                    .on_select(Msg::SetReviewCycle),
                ),
        )
        .push(space(24.0))
        .push(group_title(localized_copy("Pay with")))
        .push(space(8.0))
        .push(methods)
        .push(space(20.0))
        .push(actions_row(vec![
            if card {
                sheet_button(
                    localized_copy("Continue to secure checkout"),
                    ActionKind::Primary,
                    false,
                )
                .on_press_maybe(purchasing.then_some(Msg::StartCardCheckout))
                .into()
            } else {
                sheet_button(localized_copy("Create invoice"), ActionKind::Primary, false)
                    .on_press_maybe(purchasing.then(|| Msg::StartCheckout(checkout.tier.clone())))
                    .into()
            },
            sheet_button(localized_copy("Back"), ActionKind::Secondary, false)
                .on_press(Msg::ComparePlans(true))
                .into(),
        ]))
        .push(space(8.0))
        .push(lead(if purchasing && card {
            localized_copy(
                "Opens Stripe Checkout in your browser. You enter card details there, never in \
                 Coincube.",
            )
        } else if purchasing {
            localized_copy(
                "Creates a BTCPay invoice you can pay with Lightning or on-chain. It shows when \
                 it expires.",
            )
        } else {
            localized_copy("Purchasing is paused, so an invoice can’t be created right now.")
        }))
        .into()
}

/// Card is selectable on Review when Connect offers it and the plan isn't
/// already on a card (that changes through the card instead).
fn card_offered(state: &ConnectAccountPanel, checkout: &CheckoutState) -> bool {
    state.card_payments_enabled()
        && !checkout.plan.as_ref().is_some_and(|p| p.is_card())
        && checkout.tier != PlanTier::Free
}

/// When a new card subscription is first charged: Stripe's `startsAt` once
/// the session exists, else the end of a paid period or grant still running
/// (the API trials the card until then, for any tier). `None` = today.
pub(super) fn card_start(checkout: &CheckoutState) -> Option<String> {
    card_start_at(checkout).map(short_date)
}

/// [`card_start`] as an instant. Before the session exists it mirrors the
/// API's paid-through date: the end of a scheduled bitcoin lower tier when
/// there is one (pendingPlanAt + its cycle), else the current period end.
pub(super) fn card_start_at(checkout: &CheckoutState) -> Option<chrono::DateTime<chrono::Utc>> {
    let parse = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|t| t.with_timezone(&chrono::Utc))
    };
    if let Some(starts) = checkout
        .card_session
        .as_ref()
        .and_then(|s| s.starts_at.as_deref())
    {
        return parse(starts);
    }
    if effective_tier(checkout.plan.as_ref()) == PlanTier::Free {
        return None;
    }
    let plan = checkout.plan.as_ref()?;
    if matches!(plan.status, PlanStatus::PastDue) {
        return None;
    }
    let pending_end = match (
        plan.pending_plan.as_ref(),
        plan.pending_plan_at.as_deref().and_then(parse),
        plan.pending_billing_cycle,
    ) {
        (Some(_), Some(at), Some(cycle)) => Some(add_cycle(at, cycle)),
        _ => None,
    };
    pending_end
        .or_else(|| plan.renewal_at.as_deref().and_then(parse))
        .filter(|t| *t > chrono::Utc::now())
}

/// The Card row on Review: selectable with the charge schedule (C1), or a
/// disabled "Coming soon" row while the flag is off (board 3).
fn card_choice<'a>(
    state: &'a ConnectAccountPanel,
    checkout: &'a CheckoutState,
    facts: &ReviewFacts,
) -> Element<'a, Msg> {
    if !card_offered(state, checkout) {
        return choice_row(
            "tenshu.connect.billing.pay.card",
            "Card".to_string(),
            Some(("Coming soon", false)),
            "Pay by card with automatic renewal.".to_string(),
            None,
            false,
            false,
            None,
        );
    }
    let every = match checkout.cycle {
        BillingCycle::Annual => "every year",
        BillingCycle::Monthly => "every month",
    };
    let amount = facts.amount.clone().unwrap_or_default().replace(" USD", "");
    let description = match &card_start(checkout) {
        Some(date) => format!(
            "Through Stripe. First charged {amount} on {date}, when your paid period ends, then \
             {every} until you turn auto-renewal off."
        ),
        None => format!(
            "Through Stripe. Charged {amount} today, then {every} until you turn auto-renewal off."
        ),
    };
    choice_row(
        "tenshu.connect.billing.pay.card",
        "Card".to_string(),
        None,
        description,
        None,
        checkout.pay_with == PayWith::Card,
        true,
        Some(Msg::SelectPayWith(PayWith::Card)),
    )
}

// ── Card (boards C1–C13) ─────────────────────────────────────────────────────

/// Whether Plan & billing has caught up after an unconfirmed change.
pub(super) fn unconfirmed_refresh_line(refresh: &PlanRefresh) -> &'static str {
    match refresh {
        PlanRefresh::Pending(_) => "Refreshing Plan & billing…",
        PlanRefresh::Updated => "Plan & billing now shows the latest plan from Connect.",
        PlanRefresh::Failed => {
            "Plan & billing couldn’t be refreshed, so it may be out of date. Refresh it again."
        }
    }
}

/// A card checkout or 3-D Secure confirmation whose plan polling stopped.
fn card_poll_failed_ux<'a>(checkout: &'a CheckoutState) -> Element<'a, Msg> {
    Column::new()
        .width(Length::Fill)
        .push(sheet_title(format!("Pay for {}", checkout.tier)))
        .push(space(20.0))
        .push(billing_notice(
            exclamation_circle_icon(),
            Tone::Warn,
            localized_copy("Couldn’t check the checkout"),
            localized_copy(
                "Connect didn’t answer. If you finished on Stripe’s page, your plan updates once \
                 Connect sees it. Check again, or come back later.",
            ),
            Some(actions_row(vec![
                sheet_button(localized_copy("Check again"), ActionKind::Primary, true)
                    .on_press(Msg::ResumeChargePolling)
                    .into(),
                back_to_billing(),
            ])),
        ))
        .into()
}

/// Card checkout and card plan-change screens.
fn card_phase_ux<'a>(
    state: &'a ConnectAccountPanel,
    checkout: &'a CheckoutState,
) -> Element<'a, Msg> {
    let tier = &checkout.tier;
    let cycle = cycle_label(checkout.cycle);
    let price = price_for(state, tier, checkout.cycle);
    let starts = card_start(checkout);
    let preview = checkout.preview.as_ref();
    let upgrade = preview.is_some_and(|p| p.is_upgrade());
    let current = effective_tier(checkout.plan.as_ref());
    let renewal = checkout
        .plan
        .as_ref()
        .and_then(|p| p.renewal_at.as_deref())
        .map(format_date);

    let (heading, lead_text) = match &checkout.phase {
        CheckoutPhase::CardOpening | CheckoutPhase::CardInBrowser => (
            format!("Pay for {tier}"),
            "Finish checkout on Stripe’s page.".to_string(),
        ),
        CheckoutPhase::CardActive => match &starts {
            Some(date) => (
                "Card subscription set up".to_string(),
                format!("{tier} starts on {date}, when your current period ends."),
            ),
            None => (
                "Plan changed".to_string(),
                "Your new plan is active and renews automatically.".to_string(),
            ),
        },
        CheckoutPhase::CardCancelled => (
            "Review plan change".to_string(),
            "Checkout didn’t finish.".to_string(),
        ),
        CheckoutPhase::CardFailed { .. } => (
            "Review plan change".to_string(),
            "Stripe checkout couldn’t be opened.".to_string(),
        ),
        CheckoutPhase::ChangeDone { scheduled: true } => (
            format!("Change to {tier} scheduled"),
            "You keep your current plan until this period ends.".to_string(),
        ),
        CheckoutPhase::ChangeDone { scheduled: false } => (
            "Plan changed".to_string(),
            match preview
                .and_then(|p| p.renewal_at.as_deref())
                .map(format_date)
            {
                Some(d) => format!("{tier} is active now and renews on {d}."),
                None => format!("{tier} is active now."),
            },
        ),
        CheckoutPhase::ChangeUnconfirmed { .. } => (
            if upgrade {
                format!("Upgrade to {tier}")
            } else {
                format!("Change to {tier}")
            },
            format!(
                "COINCUBE Desktop couldn’t confirm whether the change to {tier} went through. Check Plan \
                 & billing before you try again."
            ),
        ),
        _ if upgrade => (
            format!("Upgrade to {tier}"),
            format!(
                "{tier} starts as soon as you confirm. You pay only the difference for the rest \
                 of this period."
            ),
        ),
        _ => (
            format!("Change to {tier}"),
            format!(
                "You keep {current} until this period ends. Nothing is charged or refunded today."
            ),
        ),
    };

    // Summary list.
    let card_label = checkout
        .plan
        .as_ref()
        .and_then(|p| p.card_label())
        .unwrap_or_else(|| "Card".to_string());
    let (amount_main, amount_sub, covers) = match (&checkout.phase, preview) {
        // A change that got no clear answer: say what was asked for, never
        // that it was (or wasn't) charged.
        (CheckoutPhase::ChangeUnconfirmed { .. }, _) => (
            "Not confirmed".to_string(),
            match preview {
                Some(p) if p.is_upgrade() => format!(
                    "${} was due today if the change went through",
                    format_usd(p.amount_due_today)
                ),
                Some(_) => "Nothing was due today for this change".to_string(),
                None => "Check Plan & billing for what was charged".to_string(),
            },
            "Unknown until Plan & billing shows the result".to_string(),
        ),
        (
            CheckoutPhase::ChangePreviewing
            | CheckoutPhase::ChangeReview
            | CheckoutPhase::ChangeApplying
            | CheckoutPhase::ChangeInBrowser
            | CheckoutPhase::ChangeDone { .. }
            | CheckoutPhase::ChangeFailed { .. },
            Some(p),
        ) => {
            let next = format_usd(p.next_charge_amount);
            let per = match checkout.cycle {
                BillingCycle::Annual => "a year",
                BillingCycle::Monthly => "a month",
            };
            if p.is_upgrade() {
                let renew = p.renewal_at.as_deref().map(format_date).or(renewal.clone());
                (
                    format!("${} USD today", format_usd(p.amount_due_today)),
                    match &renew {
                        Some(d) => format!("Prorated, then ${next} {per} from {d}"),
                        None => format!("Prorated, then ${next} {per}"),
                    },
                    match &renew {
                        Some(d) => format!("Today – {d}, then renews"),
                        None => "From today, then renews".to_string(),
                    },
                )
            } else {
                let from = p
                    .effective_at
                    .as_deref()
                    .map(format_date)
                    .or(renewal.clone());
                (
                    "$0.00 today".to_string(),
                    match &from {
                        Some(d) => format!("${next} {per} from {d}"),
                        None => format!("${next} {per}"),
                    },
                    match &from {
                        Some(d) => format!("{tier} from {d}, then renews"),
                        None => format!("{tier} at the end of this period"),
                    },
                )
            }
        }
        _ => (
            price
                .map(|usd| match checkout.cycle {
                    BillingCycle::Annual => format!("${usd} USD per year"),
                    BillingCycle::Monthly => format!("${usd} USD per month"),
                })
                .unwrap_or_default(),
            match &starts {
                Some(d) => format!("First charged on {d}, then each period"),
                None => "Charged to your card today and each period".to_string(),
            },
            match &starts {
                Some(d) => format!("From {d}, then renews"),
                None => "From today, then renews".to_string(),
            },
        ),
    };
    let (status_text, tone) = match (&checkout.phase, &starts) {
        (CheckoutPhase::CardActive, Some(date)) => {
            (format!("Starts {date} · auto-renews"), Tone::Good)
        }
        _ => checkout_status(checkout),
    };
    let mut plan_main = Column::new().push(tenshu_text::label(format!("{tier} · {cycle}")));
    if current != PlanTier::Free {
        if let Some(p) = checkout.plan.as_ref() {
            let paid_by = if p.is_card() { ", paid by card" } else { "" };
            plan_main = plan_main.push(lead(match p.billing_cycle {
                Some(c) => format!("Replaces {} · {}{paid_by}", p.tier(), cycle_label(c)),
                None => format!("Replaces {}{paid_by}", p.tier()),
            }));
        }
    }
    let list = ruled_list([
        summary_row(localized_copy("Plan"), plan_main.into(), None),
        summary_row(
            localized_copy("Amount"),
            Column::new()
                .push(tenshu_text::label(amount_main))
                .push(lead(amount_sub))
                .into(),
            None,
        ),
        summary_row(
            localized_copy("Covers"),
            tenshu_text::label(covers).into(),
            None,
        ),
        summary_row(
            localized_copy("Status"),
            iced::widget::Space::new().into(),
            Some(status_line(status_icon(tone), status_text, tone)),
        ),
    ]);

    let body: Element<'a, Msg> = match &checkout.phase {
        CheckoutPhase::CardOpening => lead(localized_copy("Opening Stripe Checkout…")).into(),
        CheckoutPhase::ChangePreviewing => Column::new()
            .spacing(12)
            .push(lead(localized_copy(
                "Asking Stripe what this change costs…",
            )))
            .push(back_to_billing())
            .into(),
        CheckoutPhase::CardInBrowser => billing_notice(
            clock_icon(),
            Tone::Warn,
            localized_copy("Finish checkout in your browser"),
            localized_copy(
                "Stripe’s secure checkout page is open in your browser. Enter your card there; \
                 this screen updates when checkout finishes.",
            ),
            Some(actions_row(vec![
                sheet_button(
                    localized_copy("Reopen checkout page"),
                    ActionKind::Secondary,
                    true,
                )
                .on_press(Msg::ReopenCardPage)
                .into(),
                sheet_link(localized_copy("Cancel"))
                    .on_press(Msg::BackToReview {
                        pay_with: PayWith::Card,
                    })
                    .into(),
            ])),
        ),
        CheckoutPhase::CardActive => {
            let charge = price.map(|usd| format!("${usd}")).unwrap_or_default();
            let body = match (&starts, state.plan.as_ref().and_then(|p| p.card_label())) {
                (Some(date), Some(card)) => format!(
                    "{card} is charged {charge} on {date}, when your paid period ends, then each \
                     period. You can turn auto-renewal off in Plan & billing."
                ),
                (None, Some(card)) => format!(
                    "Charged {charge} to {card}. Renews automatically; you can turn this off in \
                     Plan & billing."
                ),
                _ => "Your card subscription is set up. You can turn auto-renewal off in Plan & \
                      billing."
                    .to_string(),
            };
            billing_notice(
                check_circle_icon(),
                Tone::Good,
                if starts.is_some() {
                    format!("{tier} starts on your card later")
                } else {
                    format!("You're on {tier}")
                },
                body,
                Some(
                    sheet_button(localized_copy("Done"), ActionKind::Primary, true)
                        .on_press(Msg::DismissCheckout)
                        .into(),
                ),
            )
        }
        CheckoutPhase::CardCancelled => billing_notice(
            dash_circle_icon(),
            Tone::Muted,
            localized_copy("Checkout was cancelled"),
            localized_copy("Nothing was charged and your plan hasn't changed."),
            Some(actions_row(vec![
                sheet_button(localized_copy("Try card again"), ActionKind::Primary, true)
                    .on_press(Msg::StartCardCheckout)
                    .into(),
                sheet_button(
                    localized_copy("Pay with bitcoin instead"),
                    ActionKind::Secondary,
                    true,
                )
                .on_press(Msg::BackToReview {
                    pay_with: PayWith::Bitcoin,
                })
                .into(),
            ])),
        ),
        CheckoutPhase::CardFailed { message, refused } => billing_notice(
            exclamation_circle_icon(),
            Tone::Bad,
            if *refused {
                localized_copy("Connect didn’t accept this card checkout")
            } else {
                localized_copy("Couldn’t open checkout")
            },
            if *refused {
                format!("{message}. Nothing was charged.")
            } else {
                format!(
                    "Connect didn’t respond. Nothing was charged. Check your connection and try \
                     again. ({message})"
                )
            },
            Some(actions_row(vec![
                if *refused {
                    sheet_button(localized_copy("Change plan"), ActionKind::Primary, true)
                        .on_press(Msg::ComparePlans(true))
                        .into()
                } else {
                    sheet_button(localized_copy("Try again"), ActionKind::Primary, true)
                        .on_press(Msg::StartCardCheckout)
                        .into()
                },
                sheet_button(
                    localized_copy("Pay with bitcoin instead"),
                    ActionKind::Secondary,
                    true,
                )
                .on_press(Msg::BackToReview {
                    pay_with: PayWith::Bitcoin,
                })
                .into(),
            ])),
        ),
        CheckoutPhase::ChangeReview | CheckoutPhase::ChangeApplying => {
            let applying = checkout.phase == CheckoutPhase::ChangeApplying;
            let p = preview.expect("ChangeReview carries a preview");
            let line = |label: String, amount: String, strong: bool| -> Element<'a, Msg> {
                let text = |t: String| {
                    if strong {
                        tenshu_text::label(t)
                    } else {
                        lead(t)
                    }
                };
                Row::new()
                    .spacing(16)
                    .align_y(Alignment::Center)
                    .push(text(label).width(Length::Fill))
                    .push(text(amount))
                    .into()
            };
            let renew = p.renewal_at.as_deref().map(format_date).or(renewal.clone());
            let lines: Vec<Element<'a, Msg>> = if p.is_upgrade() {
                vec![
                    line(
                        format!(
                            "{tier} for the rest of this period{}",
                            renew
                                .as_ref()
                                .map(|d| format!(" (until {d})"))
                                .unwrap_or_default()
                        ),
                        format!("${}", format_usd(p.amount_due_today + p.credit)),
                        false,
                    ),
                    line(
                        format!("Credit for unused {current} time"),
                        format!("−${}", format_usd(p.credit)),
                        false,
                    ),
                    line(
                        "Due today".to_string(),
                        format!("${}", format_usd(p.amount_due_today)),
                        true,
                    ),
                ]
            } else {
                let per = match checkout.cycle {
                    BillingCycle::Annual => "a year",
                    BillingCycle::Monthly => "a month",
                };
                let from = p
                    .effective_at
                    .as_deref()
                    .map(format_date)
                    .or(renewal.clone());
                vec![
                    line(
                        format!("{current} stays on until"),
                        from.clone()
                            .unwrap_or_else(|| "the end of this period".to_string()),
                        false,
                    ),
                    line(
                        format!("Then {tier} renews at"),
                        format!("${} {per}", format_usd(p.next_charge_amount)),
                        false,
                    ),
                    line("Due today".to_string(), "$0.00".to_string(), true),
                ]
            };
            let limit = entitlements_for(state, tier).cube_limit;
            let note = if p.is_upgrade() {
                format!(
                    "{}If your bank asks you to confirm the payment, Stripe opens in your browser.",
                    renew
                        .as_ref()
                        .map(|d| format!("{tier} renews on {d}. "))
                        .unwrap_or_default()
                )
            } else {
                format!(
                    "{tier} allows {limit} Cubes per network. If you have more then, every Cube \
                     keeps working, but only {limit} per network keep Connect features. You can \
                     undo this in Plan & billing before then."
                )
            };
            let cta = if p.is_upgrade() {
                format!("Upgrade and pay ${}", format_usd(p.amount_due_today))
            } else {
                format!("Schedule change to {tier}")
            };
            Column::new()
                .push(group_title(if p.is_upgrade() {
                    localized_copy("Due today")
                } else {
                    localized_copy("What changes")
                }))
                .push(space(8.0))
                .push(ruled_list(lines))
                .push(space(20.0))
                .push(ruled_list([summary_row(
                    localized_copy("Pay with"),
                    Column::new()
                        .push(
                            Row::new()
                                .spacing(8)
                                .align_y(Alignment::Center)
                                .push(tenshu_text::label(card_label.clone()))
                                .push(row_tag("Stripe", false)),
                        )
                        .push(lead(if p.is_upgrade() {
                            "Card on file · charged when you confirm".to_string()
                        } else {
                            "Card on file · charged when the new plan starts".to_string()
                        }))
                        .into(),
                    None,
                )]))
                .push(space(20.0))
                .push(actions_row(vec![
                    sheet_button(cta, ActionKind::Primary, false)
                        .on_press_maybe((!applying).then_some(Msg::ConfirmPlanChange))
                        .into(),
                    sheet_button(localized_copy("Back"), ActionKind::Secondary, false)
                        .on_press_maybe((!applying).then_some(Msg::ComparePlans(true)))
                        .into(),
                    sheet_link(localized_copy("Back to Plan & billing"))
                        .on_press_maybe((!applying).then_some(Msg::DismissCheckout))
                        .into(),
                ]))
                .push(space(8.0))
                .push(lead(note))
                .into()
        }
        CheckoutPhase::ChangeInBrowser => billing_notice(
            clock_icon(),
            Tone::Warn,
            localized_copy("Confirm the payment in your browser"),
            localized_copy(
                "Your bank asked to confirm this payment. Stripe’s page is open in your browser; \
                 this screen updates when it’s confirmed.",
            ),
            Some(actions_row(vec![
                sheet_button(
                    localized_copy("Reopen the page"),
                    ActionKind::Secondary,
                    true,
                )
                .on_press(Msg::ReopenCardPage)
                .into(),
                // Leaving stops this screen's polling; if the payment is
                // confirmed later, Plan & billing shows it when it loads.
                sheet_link(localized_copy("Back to Plan & billing"))
                    .on_press(Msg::DismissCheckout)
                    .into(),
            ])),
        ),
        CheckoutPhase::ChangeDone { scheduled } => billing_notice(
            check_circle_icon(),
            Tone::Good,
            if *scheduled {
                format!("{tier} starts when this period ends")
            } else {
                format!("You're on {tier}")
            },
            if *scheduled {
                "Nothing was charged today. Plan & billing shows the change, and you can keep \
                 your current plan before then."
                    .to_string()
            } else {
                format!(
                    "Charged to {card_label}. The receipt is in your billing history{}.",
                    preview
                        .and_then(|p| p.renewal_at.as_deref())
                        .map(format_date)
                        .or(renewal.clone())
                        .map(|d| format!("; {tier} renews on {d}"))
                        .unwrap_or_default()
                )
            },
            Some(
                sheet_button(localized_copy("Done"), ActionKind::Primary, true)
                    .on_press(Msg::DismissCheckout)
                    .into(),
            ),
        ),
        CheckoutPhase::ChangeUnconfirmed { message, refresh } => billing_notice(
            exclamation_circle_icon(),
            Tone::Warn,
            localized_copy("We couldn’t confirm the plan change"),
            format!(
                "{message}. The change may or may not have gone through, and your card may have \
                 been charged. {} Check Plan & billing before trying again.",
                unconfirmed_refresh_line(refresh)
            ),
            Some(match refresh {
                PlanRefresh::Failed => actions_row(vec![
                    sheet_button(localized_copy("Refresh plan"), ActionKind::Primary, true)
                        .on_press(Msg::RetryPlanRefresh)
                        .into(),
                    back_to_billing(),
                ]),
                _ => back_to_billing(),
            }),
        ),
        CheckoutPhase::ChangeFailed { message, refused } => billing_notice(
            exclamation_circle_icon(),
            Tone::Bad,
            if *refused {
                localized_copy("Connect didn’t accept this plan change")
            } else {
                localized_copy("Couldn’t change the plan")
            },
            if *refused {
                format!("{message}. Nothing was charged.")
            } else {
                format!("{message}. Nothing was charged; try again in a moment.")
            },
            Some(actions_row(vec![
                sheet_button(localized_copy("Change plan"), ActionKind::Primary, true)
                    .on_press(Msg::ComparePlans(true))
                    .into(),
                back_to_billing(),
            ])),
        ),
        _ => iced::widget::Space::new().into(),
    };

    Column::new()
        .width(Length::Fill)
        .push(sheet_title(heading))
        .push(lead(lead_text))
        .push(space(20.0))
        .push(list)
        .push(space(24.0))
        .push(body)
        .into()
}

// ── Pay (boards B1–B10) ──────────────────────────────────────────────────────

/// "16:42" in local time for an RFC 3339 instant.
fn local_time(iso: &str) -> Option<String> {
    let at = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    Some(at.with_timezone(&chrono::Local).format("%H:%M").to_string())
}

/// The Status row for a checkout: text and tone (board 3 `st` table).
pub(super) fn checkout_status(checkout: &CheckoutState) -> (String, Tone) {
    let expires = checkout
        .last_status
        .as_ref()
        .and_then(|s| s.expires_at.as_deref())
        .or_else(|| checkout.checkout.as_ref()?.expires_at.as_deref())
        .and_then(local_time);
    let waiting = |label: &str| match &expires {
        Some(time) => format!("{label} · expires {time}"),
        None => label.to_string(),
    };
    match &checkout.phase {
        CheckoutPhase::Review => ("Not invoiced yet".to_string(), Tone::Muted),
        CheckoutPhase::Creating => ("Creating invoice…".to_string(), Tone::Muted),
        CheckoutPhase::AwaitingPayment | CheckoutPhase::PayingInBrowser => {
            (waiting("Waiting for payment"), Tone::Warn)
        }
        CheckoutPhase::PartlyPaid { .. } => (waiting("Partly paid"), Tone::Warn),
        CheckoutPhase::Processing => ("Payment seen · confirming".to_string(), Tone::Warn),
        CheckoutPhase::Paid { .. } => ("Paid".to_string(), Tone::Good),
        CheckoutPhase::Expired => ("Expired · nothing received".to_string(), Tone::Muted),
        CheckoutPhase::NeedsSupport { .. } => ("Needs support".to_string(), Tone::Bad),
        CheckoutPhase::Failed { .. } => ("Not invoiced".to_string(), Tone::Bad),
        CheckoutPhase::PollFailed(_) => ("Couldn’t check the payment".to_string(), Tone::Warn),
        CheckoutPhase::CardOpening | CheckoutPhase::ChangePreviewing => {
            ("Not charged yet".to_string(), Tone::Muted)
        }
        CheckoutPhase::CardInBrowser => ("Waiting for checkout".to_string(), Tone::Warn),
        CheckoutPhase::CardActive => ("Paid · auto-renews".to_string(), Tone::Good),
        CheckoutPhase::CardCancelled => ("Not charged".to_string(), Tone::Muted),
        CheckoutPhase::CardFailed { .. } => ("Not charged".to_string(), Tone::Bad),
        CheckoutPhase::ChangeReview | CheckoutPhase::ChangeApplying => {
            match checkout.preview.as_ref() {
                Some(p) if !p.is_upgrade() => ("Nothing due today".to_string(), Tone::Muted),
                _ => ("Not charged yet".to_string(), Tone::Muted),
            }
        }
        CheckoutPhase::ChangeInBrowser => ("Waiting for your bank".to_string(), Tone::Warn),
        CheckoutPhase::ChangeDone { scheduled: true } => ("Scheduled".to_string(), Tone::Good),
        CheckoutPhase::ChangeDone { scheduled: false } => {
            ("Paid · auto-renews".to_string(), Tone::Good)
        }
        CheckoutPhase::ChangeFailed { .. } => ("Not charged".to_string(), Tone::Bad),
        CheckoutPhase::ChangeUnconfirmed { .. } => ("Not confirmed".to_string(), Tone::Warn),
    }
}

fn qr_box<'a>(qr: Option<&'a iced::widget::qr_code::Data>) -> Element<'a, Msg> {
    let frame = |t: &theme::Theme| container::Style {
        background: Some(iced::Background::Color(t.colors.shell.background)),
        border: iced::Border {
            color: t.colors.shell.line,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    };
    match qr {
        Some(qr) => container(iced::widget::QRCode::<theme::Theme>::new(qr).total_size(184.0))
            .padding(8)
            .style(frame)
            .into(),
        None => container(iced::widget::Space::new())
            .width(Length::Fixed(200.0))
            .height(Length::Fixed(200.0))
            .style(frame)
            .into(),
    }
}

/// "62% received" meter (B5).
fn meter<'a>(received: u64, total: u64) -> Element<'a, Msg> {
    let pct = received
        .saturating_mul(100)
        .checked_div(total)
        .map_or(0, |p| p.min(100) as u16);
    let bar = |portion: u16, filled: bool| {
        container(iced::widget::Space::new())
            .width(Length::FillPortion(portion.max(1)))
            .height(Length::Fixed(6.0))
            .style(move |t: &theme::Theme| container::Style {
                background: Some(iced::Background::Color(if filled {
                    t.colors.shell.accent
                } else {
                    t.colors.shell.line
                })),
                border: iced::Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
    };
    let mut row = Row::new().spacing(2).width(Length::Fill);
    if pct > 0 {
        row = row.push(bar(pct, true));
    }
    if pct < 100 {
        row = row.push(bar(100 - pct, false));
    }
    row.into()
}

fn back_to_billing<'a>() -> Element<'a, Msg> {
    sheet_button(
        localized_copy("Back to Plan & billing"),
        ActionKind::Secondary,
        true,
    )
    .on_press(Msg::DismissCheckout)
    .into()
}

/// A labelled value that wraps across lines (a BOLT11 invoice, an address)
/// beside a Copy button, with a hint under it.
fn wrapped_copy_field<'a>(label: String, value: &'a str, hint: String) -> Element<'a, Msg> {
    Column::new()
        .spacing(6)
        .push(
            tenshu_text::body(label)
                .size(13)
                .font(coincube_ui::font::SEMIBOLD),
        )
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Start)
                .push(
                    container(
                        tenshu_text::mono(value)
                            .size(13)
                            .wrapping(iced::widget::text::Wrapping::Glyph),
                    )
                    .padding([8, 12])
                    .width(Length::Fill)
                    .style(|t: &theme::Theme| container::Style {
                        background: Some(iced::Background::Color(t.colors.shell.background)),
                        border: iced::Border {
                            color: t.colors.shell.line,
                            width: 1.0,
                            radius: 8.0.into(),
                        },
                        ..Default::default()
                    }),
                )
                .push(
                    sheet_button(localized_copy("Copy"), ActionKind::Secondary, false)
                        .on_press(Msg::CopyToClipboard(value.to_string())),
                ),
        )
        .push(lead(hint))
        .into()
}

/// The pay block for Awaiting payment and Partly paid (B2, B3, B5).
fn pay_block<'a>(checkout: &'a CheckoutState) -> Element<'a, Msg> {
    let Some(resp) = checkout.checkout.as_ref() else {
        return lead(localized_copy("Waiting for the invoice…")).into();
    };
    let partial = match checkout.phase {
        CheckoutPhase::PartlyPaid {
            received_sats,
            due_sats,
        } => Some((received_sats, due_sats)),
        _ => None,
    };
    let expires = resp.expires_at.as_deref().and_then(local_time);
    let lightning = partial.is_none() && checkout.network == PaymentNetwork::Lightning;
    let mut head = Row::new().align_y(Alignment::Center).push(
        group_title(if partial.is_some() {
            localized_copy("Send the rest on-chain")
        } else {
            localized_copy("Pay with")
        })
        .width(Length::Fill),
    );
    if partial.is_none() {
        head = head.push(
            tenshu::segmented(
                [
                    tenshu::segment(
                        "tenshu.connect.billing.network.lightning",
                        PaymentNetwork::Lightning,
                        localized_copy("Lightning"),
                    ),
                    tenshu::segment(
                        "tenshu.connect.billing.network.onchain",
                        PaymentNetwork::OnChain,
                        localized_copy("On-chain"),
                    ),
                ],
                Some(checkout.network),
                Msg::FocusAction,
            )
            .size(tenshu::SegmentSize::Compact)
            .plain()
            .on_select(Msg::SelectPaymentNetwork),
        );
    }
    let mut col = Column::new().spacing(8).push(head);
    if let Some((received, due)) = partial {
        let total = received + due;
        col = col.push(meter(received, total)).push(lead(format!(
            "{} of {} sats received · {} sats left to pay",
            group_sats(received),
            group_sats(total),
            group_sats(due)
        )));
    }
    let (label, value, hint) = if lightning {
        (
            "Lightning invoice".to_string(),
            resp.lightning_invoice.as_str(),
            "The amount is already in the invoice. Lightning settles in seconds.".to_string(),
        )
    } else if let Some((_, due)) = partial {
        (
            format!("Same Bitcoin address · send {} sats more", group_sats(due)),
            resp.on_chain_address.as_str(),
            match &expires {
                Some(time) => format!(
                    "Send the remaining amount before {time}. If the invoice expires first, \
                     contact support with the invoice ID."
                ),
                None => "Send the remaining amount before the invoice expires. If it expires \
                         first, contact support with the invoice ID."
                    .to_string(),
            },
        )
    } else {
        (
            "Bitcoin address".to_string(),
            resp.on_chain_address.as_str(),
            format!(
                "Send exactly {} sats. {} turns on after the payment confirms.",
                group_sats(resp.amount_sats),
                checkout.tier
            ),
        )
    };
    let qr = if lightning {
        checkout.lightning_qr.as_ref()
    } else {
        checkout.onchain_qr.as_ref()
    };
    // The full invoice or address, wrapped and never truncated; Copy copies
    // the whole value.
    let field = wrapped_copy_field(label, value, hint);
    let links = actions_row(vec![
        sheet_link(localized_copy("Pay on the BTCPay page instead"))
            .on_press(Msg::OpenCheckoutUrl(resp.checkout_url.clone()))
            .into(),
        sheet_link(localized_copy("Cancel"))
            .on_press(Msg::DismissCheckout)
            .into(),
    ]);
    col.push(space(8.0))
        .push(
            Row::new()
                .spacing(24)
                .align_y(Alignment::Start)
                .push(qr_box(qr))
                .push(
                    Column::new()
                        .spacing(16)
                        .width(Length::Fill)
                        .push(field)
                        .push(links),
                ),
        )
        .into()
}

/// A checkout past Review: header, the summary list and the phase body.
fn checkout_phase_ux<'a>(
    state: &'a ConnectAccountPanel,
    checkout: &'a CheckoutState,
) -> Element<'a, Msg> {
    let facts = review_facts(state, checkout, checkout.opened_at);
    let tier = &checkout.tier;
    // A4: the poll's startsAt wins (a renewal may have moved the period end
    // since the invoice was created), then the create response's.
    let starts = checkout
        .last_status
        .as_ref()
        .and_then(|s| s.starts_at.as_deref())
        .or_else(|| checkout.checkout.as_ref()?.starts_at.as_deref())
        .map(format_date);
    // When the paid period starts later: Connect's startsAt, else the
    // stacked start review_facts computed (a same-tier renewal, for which
    // Connect sends no startsAt).
    let paid_start = starts.clone().or_else(|| facts.starts.clone());
    let renewal = effective_tier(checkout.plan.as_ref()) == *tier;
    let (heading, lead_text) = match &checkout.phase {
        CheckoutPhase::Creating => (
            format!("Pay for {tier}"),
            "Preparing a Bitcoin invoice for the amount above.".to_string(),
        ),
        CheckoutPhase::Paid { .. } => match &paid_start {
            Some(date) if renewal => (
                "Plan renewed".to_string(),
                format!("Your {tier} plan continues. The new period starts on {date}."),
            ),
            Some(date) => (
                "Plan change scheduled".to_string(),
                format!("{tier} starts on {date}, when your current period ends."),
            ),
            None => (
                "Plan changed".to_string(),
                "Your new plan is active on this account.".to_string(),
            ),
        },
        CheckoutPhase::NeedsSupport { .. } => (
            format!("Pay for {tier}"),
            "There is a problem with this payment.".to_string(),
        ),
        CheckoutPhase::Failed { .. } => (
            "Review plan change".to_string(),
            "The invoice couldn’t be created.".to_string(),
        ),
        _ => (
            format!("Pay for {tier}"),
            "Pay from any Bitcoin wallet. This screen updates by itself when the payment arrives."
                .to_string(),
        ),
    };

    let resp = checkout.checkout.as_ref();
    let amount_main = match resp {
        Some(r) => format!("${} {}", format_usd(r.amount_fiat), r.fiat_currency),
        None => facts.amount.clone().unwrap_or_default(),
    };
    let amount_sub = match resp {
        Some(r) => format!(
            "{} sats at today’s rate, fixed for this invoice",
            group_sats(r.amount_sats)
        ),
        None => "In sats at today’s rate, fixed for this invoice".to_string(),
    };
    let mut plan_main = Column::new().push(tenshu_text::label(facts.plan.clone()));
    if let Some(replaces) = facts.replaces.clone() {
        plan_main = plan_main.push(lead(replaces));
    }
    // Connect's startsAt is authoritative for a lower tier bought ahead.
    let starts_raw = checkout
        .last_status
        .as_ref()
        .and_then(|s| s.starts_at.as_deref())
        .or_else(|| checkout.checkout.as_ref()?.starts_at.as_deref())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|s| s.with_timezone(&chrono::Utc));
    let covers_text = match starts_raw {
        Some(start) => format!(
            "{} – {}",
            short_date(start),
            short_date(add_cycle(start, checkout.cycle))
        ),
        None => facts.covers.clone().unwrap_or_else(|| "—".to_string()),
    };
    let mut covers = Column::new().push(tenshu_text::label(covers_text));
    if let Some(date) = starts.clone().or(facts.starts.clone()) {
        covers = covers.push(lead(format!("Starts {date}")));
    }
    let invoice_id = match &checkout.phase {
        CheckoutPhase::Creating | CheckoutPhase::Failed { .. } => None,
        _ => checkout
            .last_status
            .as_ref()
            .and_then(|s| s.invoice_id.clone())
            .filter(|id| !id.is_empty())
            .or_else(|| resp.map(|r| r.charge_id.clone())),
    };
    let (status_text, tone) = checkout_status(checkout);
    let status_main: Element<'a, Msg> = match &invoice_id {
        Some(id) => tenshu_text::mono(format!("Invoice {id}"))
            .size(12)
            .style(theme::tenshu::text::muted)
            .into(),
        None => iced::widget::Space::new().into(),
    };
    let list = ruled_list([
        summary_row(localized_copy("Plan"), plan_main.into(), None),
        summary_row(
            localized_copy("Amount"),
            Column::new()
                .push(tenshu_text::label(amount_main))
                .push(lead(amount_sub))
                .into(),
            None,
        ),
        summary_row(localized_copy("Covers"), covers.into(), None),
        summary_row(
            localized_copy("Status"),
            status_main,
            Some(status_line(status_icon(tone), status_text, tone)),
        ),
    ]);

    let body: Element<'a, Msg> = match &checkout.phase {
        CheckoutPhase::Review => iced::widget::Space::new().into(),
        CheckoutPhase::Creating => Row::new()
            .spacing(24)
            .align_y(Alignment::Start)
            .push(qr_box(None))
            .push(lead(localized_copy(
                "Asking BTCPay for an invoice. This usually takes a few seconds.",
            )))
            .into(),
        CheckoutPhase::AwaitingPayment | CheckoutPhase::PartlyPaid { .. } => pay_block(checkout),
        CheckoutPhase::PayingInBrowser => billing_notice(
            clock_icon(),
            Tone::Warn,
            localized_copy("Finish paying on the BTCPay page"),
            localized_copy(
                "The BTCPay checkout page is open in your browser. Keep this screen open or come \
                 back later; it updates when the payment arrives.",
            ),
            Some(actions_row({
                let mut items: Vec<Element<'a, Msg>> = Vec::new();
                if let Some(r) = resp {
                    items.push(
                        sheet_button(
                            localized_copy("Reopen checkout page"),
                            ActionKind::Secondary,
                            true,
                        )
                        .on_press(Msg::OpenCheckoutUrl(r.checkout_url.clone()))
                        .into(),
                    );
                }
                items.push(
                    sheet_link(localized_copy("Show the QR code here instead"))
                        .on_press(Msg::ShowInvoiceHere)
                        .into(),
                );
                items.push(
                    sheet_link(localized_copy("Cancel"))
                        .on_press(Msg::DismissCheckout)
                        .into(),
                );
                items
            })),
        ),
        CheckoutPhase::Processing => billing_notice(
            clock_icon(),
            Tone::Warn,
            localized_copy("Payment seen, waiting for confirmation"),
            format!(
                "BTCPay has seen your payment. {tier} turns on once it confirms. You can leave \
                 this screen: Connect applies the payment when it confirms, and Plan & billing \
                 shows it the next time it loads."
            ),
            Some(back_to_billing()),
        ),
        CheckoutPhase::Paid { overpaid_sats } => {
            let mut body = match (&paid_start, facts.covers.as_deref()) {
                (Some(date), _) if renewal => format!(
                    "The new {tier} period starts on {date}, when the current one ends. The \
                     receipt is in your billing history."
                ),
                (Some(date), _) => format!(
                    "{tier} starts on {date}. Until then you keep your current plan. The receipt \
                     is in your billing history."
                ),
                (None, Some(covers)) => {
                    format!("Covers {covers}. The receipt is in your billing history.")
                }
                (None, None) => "The receipt is in your billing history.".to_string(),
            };
            if *overpaid_sats > 0 {
                body.push_str(&format!(
                    " You overpaid by {} sats; contact support for the difference.",
                    group_sats(*overpaid_sats)
                ));
            }
            billing_notice(
                check_circle_icon(),
                Tone::Good,
                match &paid_start {
                    Some(_) if renewal => format!("{tier} renewed"),
                    Some(_) => format!("{tier} is paid for"),
                    None => format!("You're on {tier}"),
                },
                body,
                Some(
                    sheet_button(localized_copy("Done"), ActionKind::Primary, true)
                        .on_press(Msg::DismissCheckout)
                        .into(),
                ),
            )
        }
        CheckoutPhase::Expired => billing_notice(
            dash_circle_icon(),
            Tone::Muted,
            localized_copy("This invoice expired"),
            localized_copy(
                "No payment was received, so nothing was charged. A new invoice uses the current \
                 bitcoin rate.",
            ),
            Some(actions_row(vec![
                // Re-issues the same tier and cycle (a new checkout
                // generation), not a dismiss.
                sheet_button(
                    localized_copy("Create new invoice"),
                    ActionKind::Primary,
                    true,
                )
                .on_press(Msg::StartCheckout(tier.clone()))
                .into(),
                back_to_billing(),
            ])),
        ),
        CheckoutPhase::NeedsSupport {
            invoice_id,
            received_sats,
        } => {
            let late = checkout
                .last_status
                .as_ref()
                .and_then(|s| s.additional_status.as_deref())
                .is_some_and(|a| a.eq_ignore_ascii_case("PaidLate"));
            let received = if *received_sats > 0 {
                format!("BTCPay received {} sats", group_sats(*received_sats))
            } else {
                "BTCPay received a payment".to_string()
            };
            billing_notice(
                exclamation_circle_icon(),
                Tone::Bad,
                if late {
                    localized_copy("Payment arrived after the invoice expired")
                } else {
                    localized_copy("This payment needs support")
                },
                format!(
                    "{received}, but it can’t be applied to your plan automatically. Your plan \
                     hasn't changed. Contact support with the invoice ID below."
                ),
                Some(
                    Column::new()
                        .spacing(12)
                        .push(
                            tenshu::copy_field(
                                "tenshu.connect.billing.copy.invoice-id",
                                localized_copy("Invoice ID"),
                                invoice_id.as_str(),
                                Msg::FocusAction,
                            )
                            .on_copy(Msg::CopyToClipboard(invoice_id.clone())),
                        )
                        .push(back_to_billing())
                        .into(),
                ),
            )
        }
        CheckoutPhase::Failed { message, refused } if *refused => billing_notice(
            exclamation_circle_icon(),
            Tone::Bad,
            localized_copy("Connect didn’t accept this plan change"),
            format!("{message}. Nothing was charged."),
            Some(actions_row(vec![
                sheet_button(localized_copy("Change plan"), ActionKind::Primary, true)
                    .on_press(Msg::ComparePlans(true))
                    .into(),
                back_to_billing(),
            ])),
        ),
        CheckoutPhase::Failed { message, .. } => billing_notice(
            exclamation_circle_icon(),
            Tone::Bad,
            localized_copy("Couldn’t create the invoice"),
            format!(
                "Connect or BTCPay didn’t respond. Nothing was charged. Check your connection \
                 and try again. ({message})"
            ),
            Some(actions_row(vec![
                sheet_button(localized_copy("Try again"), ActionKind::Primary, true)
                    .on_press(Msg::StartCheckout(tier.clone()))
                    .into(),
                back_to_billing(),
            ])),
        ),
        CheckoutPhase::CardOpening
        | CheckoutPhase::CardInBrowser
        | CheckoutPhase::CardActive
        | CheckoutPhase::CardCancelled
        | CheckoutPhase::CardFailed { .. }
        | CheckoutPhase::ChangePreviewing
        | CheckoutPhase::ChangeReview
        | CheckoutPhase::ChangeApplying
        | CheckoutPhase::ChangeInBrowser
        | CheckoutPhase::ChangeDone { .. }
        | CheckoutPhase::ChangeFailed { .. }
        | CheckoutPhase::ChangeUnconfirmed { .. } => iced::widget::Space::new().into(),
        CheckoutPhase::PollFailed(_) => billing_notice(
            exclamation_circle_icon(),
            Tone::Warn,
            localized_copy("Couldn’t check the payment"),
            localized_copy(
                "Connect didn’t answer. If you already paid, your plan updates once Connect sees \
                 the payment. Check again, or come back later.",
            ),
            Some(actions_row(vec![
                sheet_button(localized_copy("Check again"), ActionKind::Primary, true)
                    .on_press(Msg::ResumeChargePolling)
                    .into(),
                back_to_billing(),
            ])),
        ),
    };

    Column::new()
        .width(Length::Fill)
        .push(sheet_title(heading))
        .push(lead(lead_text))
        .push(space(20.0))
        .push(list)
        .push(space(24.0))
        .push(body)
        .into()
}
