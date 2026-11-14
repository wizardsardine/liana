//! Inline view for the Spark "receive from another network" (SideShift) flow.
//!
//! Rendered **below the Spark Receive two-card selector**, in place of the
//! Bitcoin rail form — the THEY SEND card shows which asset was picked, so this
//! body carries only what the swap adds: a refund-address field, a third-party
//! disclosure, and, once a shift is live, a progress tracker over a body that
//! follows SideShift's status — the deposit address while waiting, what came in
//! and what's on its way while converting, and where the money goes back to on
//! the refund path. Copy always says **bitcoin arrives**, never "receive USDT".
//! No standalone title / asset picker / Back button — navigation happens via
//! the cards + THEY SEND modal.

use std::cmp::Ordering;

use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use coincube_ui::{
    color,
    component::{
        amount::{BitcoinDisplayUnit, DisplayAmount},
        button,
        text::*,
    },
    icon::{check_circle_icon, check_icon, clipboard_icon, clock_icon, reload_icon, warning_icon},
    theme,
    widget::Element,
};
use iced::{
    widget::{container, qr_code, rule, text::Wrapping, Column, Container, Row, Space, TextInput},
    Alignment, Background, Length,
};

use crate::app::state::spark::esplora::DEPOSIT_MATURITY_CONFIRMATIONS;
use crate::app::state::spark::sideshift_receive::{SparkShiftPhase, SparkSideshiftReceiveFlow};
use crate::app::view::SparkSideshiftReceiveMessage as Msg;
use crate::services::sideshift::{DepositOption, ShiftResponse, ShiftStatusKind};

/// `confirmations` is how many blocks the swap's settle transaction has, when
/// the caller can match it to one of the wallet's pending deposits;
/// `reduce_motion` holds the in-progress pulse steady.
pub fn spark_sideshift_receive_view(
    flow: &SparkSideshiftReceiveFlow,
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
    confirmations: Option<u32>,
    reduce_motion: bool,
) -> Element<'_, Msg> {
    match flow.phase() {
        SparkShiftPhase::Setup => setup_body(flow),
        SparkShiftPhase::FetchingAffiliate | SparkShiftPhase::CreatingShift => {
            loading_body("Setting up your deposit address…")
        }
        SparkShiftPhase::Active => active_body(
            flow,
            bitcoin_unit,
            mask_amounts,
            confirmations,
            flow.pulse_dim() && !reduce_motion,
        ),
        SparkShiftPhase::Arrived => arrived_body(flow, bitcoin_unit, mask_amounts),
        SparkShiftPhase::Failed => error_body(flow.error()),
    }
}

// ── Setup: give a refund address, generate the deposit ──────────────────────

fn setup_body(flow: &SparkSideshiftReceiveFlow) -> Element<'_, Msg> {
    let selected = flow.selected();

    // The refund address. Collected *before* any deposit address is shown,
    // because a shift that fails or gets held has to have somewhere to send the
    // money back to — and asking for it afterwards is useless.
    let mut refund = Column::new()
        .spacing(10)
        .push(h4_bold(localized_copy("Refund address")))
        .push(
            text(localized_format!(
                "Your {} address. If the swap can't complete, your deposit is returned here — \
                 on {}, not as bitcoin.", "Su dirección {}. Si el swap no puede completar, su depósito es devuelto aquí - en {}, no como bitcoin.",
                selected.network.network_name(),
                selected.network.network_name(),
            ))
            .size(P2_SIZE)
            .style(theme::text::secondary),
        )
        .push(
            TextInput::new(
                &localized_format!("Your {} address", "Su dirección {}", selected.network.network_name()),
                flow.refund_address(),
            )
            .on_input(Msg::RefundAddressEdited)
            .padding([10, 14])
            .size(P1_SIZE),
        );
    if let Some(err) = flow.refund_error() {
        refund = refund.push(
            text(coincube_ui::locale::notice(err.to_string()))
                .size(P2_SIZE)
                .color(color::RED),
        );
    }
    let refund_card = Container::new(refund)
        .padding(16)
        .width(Length::Fill)
        .style(theme::card::simple);

    // Master invariant 9: name the third party and the jurisdictional caveat.
    // The user is handing funds to SideShift, not to us, and they should know
    // that before they do it — not after something goes wrong.
    let disclosure = Container::new(
        text(format!(
            "Conversion is performed by SideShift, a third-party service. It isn't available \
             in all jurisdictions, and swaps may be held for review. Bitcoin arrives on-chain \
             and is added to your wallet after about {DEPOSIT_MATURITY_CONFIRMATIONS} \
             confirmations.",
        ))
        .size(P2_SIZE)
        .style(theme::text::secondary),
    )
    .padding([8, 12])
    .width(Length::Fill)
    .style(theme::card::simple);

    let generate_btn = if flow.is_loading() {
        button::primary(None, localized_copy("Generating…")).width(Length::Fill)
    } else {
        // Disabled until the refund address validates against the origin
        // chain: a deposit address with nowhere valid to refund to is a trap.
        button::primary(None, localized_copy("Generate deposit address"))
            .on_press_maybe(flow.can_generate().then_some(Msg::Generate))
            .width(Length::Fill)
    };

    let mut col = Column::new().spacing(20).push(refund_card).push(disclosure);
    if let Some(err) = flow.error() {
        col = col.push(
            Container::new(
                text(coincube_ui::locale::notice(err.to_string()))
                    .size(P2_SIZE)
                    .color(color::RED),
            )
            .padding([8, 12])
            .width(Length::Fill)
            .style(theme::card::error),
        );
    }
    col.push(generate_btn).into()
}

// ── Active shift: progress tracker + the body for the current status ────────

fn active_body(
    flow: &SparkSideshiftReceiveFlow,
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
    confirmations: Option<u32>,
    pulse_dim: bool,
) -> Element<'_, Msg> {
    let Some(shift) = flow.shift() else {
        return error_body(Some("Shift data missing."));
    };
    active_shift_body(
        &ActiveShift {
            shift,
            selected: flow.selected(),
            status: flow.shift_status(),
            qr: flow.qr_data(),
            refund_address: flow.refund_address(),
            polled_deposit_amount: flow.deposit_amount(),
            settle_amount_sat: flow.settle_amount_sat(),
            confirmations,
            pulse_dim,
        },
        bitcoin_unit,
        mask_amounts,
    )
}

/// What a live shift's view needs, lifted off the flow so the view can be
/// rendered (and tested) without a Spark backend.
struct ActiveShift<'a> {
    shift: &'a ShiftResponse,
    selected: DepositOption,
    status: Option<&'a ShiftStatusKind>,
    qr: Option<&'a qr_code::Data>,
    refund_address: &'a str,
    polled_deposit_amount: Option<&'a str>,
    settle_amount_sat: Option<u64>,
    /// Blocks on the settle transaction, once it's among the pending deposits.
    confirmations: Option<u32>,
    /// The in-progress pulse's dim half (already off under reduce-motion).
    pulse_dim: bool,
}

impl<'a> ActiveShift<'a> {
    /// The deposit SideShift has seen: the latest poll's figure, else the one
    /// on the shift itself.
    fn deposit_amount(&self) -> Option<&'a str> {
        self.polled_deposit_amount
            .or(self.shift.deposit_amount.as_deref())
            .map(str::trim)
            .filter(|a| !a.is_empty())
    }
}

fn active_shift_body<'a>(
    v: &ActiveShift<'a>,
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'a, Msg> {
    let mut col = Column::new().spacing(18).width(Length::Fill);

    // Expired has no progress to show — nothing was ever deposited.
    if let Some(steps) = tracker_steps(false, v.status) {
        col = col.push(tracker(&steps, v.pulse_dim));
        // Refunded's own body says where the money went; the shared sentence
        // would only repeat it.
        if v.status != Some(&ShiftStatusKind::Refunded) {
            col = col.push(guidance_text(spark_shift_guidance(v.status, &v.selected)));
        }
        col = col.push(rule::horizontal(1));
    }

    let body: Element<Msg> = match v.status {
        None | Some(ShiftStatusKind::Waiting) | Some(ShiftStatusKind::Unknown(_)) => {
            deposit_section(v)
        }
        Some(
            st @ (ShiftStatusKind::Pending
            | ShiftStatusKind::Processing
            | ShiftStatusKind::Settling
            | ShiftStatusKind::Settled),
        ) => in_flight_section(v, st, bitcoin_unit, mask_amounts),
        Some(
            st @ (ShiftStatusKind::Review | ShiftStatusKind::Error | ShiftStatusKind::Refunding),
        ) => refund_section(v, st, mask_amounts),
        Some(ShiftStatusKind::Refunded) => refunded_section(v, mask_amounts),
        Some(ShiftStatusKind::Expired) => expired_section(),
    };

    col.push(body)
        .push(rule::horizontal(1))
        .push(details_list(v))
        .into()
}

// ── Progress tracker ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepMark {
    Done,
    /// Done, but not a success worth a green check — the "Swap stopped" step on
    /// the refund path once the refund has moved past it.
    DoneMuted,
    Current,
    /// Current and stuck: on hold for review, or failed. Not animated, not
    /// orange — the user has to act (review) or the refund is about to start.
    Halted,
    Todo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrackerStep {
    label: &'static str,
    mark: StepMark,
}

const CONVERT_STEPS: [&str; 4] = [
    "Waiting for deposit",
    "Deposit detected",
    "Converting",
    "Arriving in Spark",
];

/// The four tracker steps for where the swap is. The convert path runs
/// waiting → detected → converting → arriving; once SideShift stops a swap the
/// labels switch to the refund path (received → stopped → refunding →
/// refunded) so the user always sees where their money is going. `None` for an
/// expired shift: nothing was deposited, so there is no progress to show.
fn tracker_steps(arrived: bool, status: Option<&ShiftStatusKind>) -> Option<[TrackerStep; 4]> {
    let refund = |stop: &'static str| ["Deposit received", stop, "Refunding", "Refunded"];
    let (labels, current, halted) = if arrived {
        (CONVERT_STEPS, None, false)
    } else {
        match status {
            None | Some(ShiftStatusKind::Waiting) | Some(ShiftStatusKind::Unknown(_)) => {
                (CONVERT_STEPS, Some(0), false)
            }
            Some(ShiftStatusKind::Pending) => (CONVERT_STEPS, Some(1), false),
            Some(ShiftStatusKind::Processing) => (CONVERT_STEPS, Some(2), false),
            Some(ShiftStatusKind::Settling | ShiftStatusKind::Settled) => {
                (CONVERT_STEPS, Some(3), false)
            }
            Some(ShiftStatusKind::Review) => (refund("On hold for review"), Some(1), true),
            Some(ShiftStatusKind::Error) => (refund("Swap failed"), Some(1), true),
            Some(ShiftStatusKind::Refunding) => (refund("Swap stopped"), Some(2), false),
            Some(ShiftStatusKind::Refunded) => (refund("Swap stopped"), None, false),
            Some(ShiftStatusKind::Expired) => return None,
        }
    };
    let refund_path = labels[0] == "Deposit received";
    Some(std::array::from_fn(|i| {
        let mark = match current {
            Some(c) if i == c => {
                if halted {
                    StepMark::Halted
                } else {
                    StepMark::Current
                }
            }
            Some(c) if i > c => StepMark::Todo,
            _ if refund_path && i == 1 => StepMark::DoneMuted,
            _ => StepMark::Done,
        };
        TrackerStep {
            label: labels[i],
            mark,
        }
    }))
}

fn tracker<'a>(steps: &[TrackerStep; 4], pulse_dim: bool) -> Element<'a, Msg> {
    let mut row = Row::new().spacing(8).width(Length::Fill);
    for step in steps {
        let mark = step.mark;
        let bar = progress_bar(3.0, move |shell| match mark {
            StepMark::Done => shell.good,
            StepMark::DoneMuted => shell.muted,
            StepMark::Current => pulsed(shell.accent, pulse_dim),
            StepMark::Halted => shell.bad,
            StepMark::Todo => shell.line,
        });
        let mut label = text(localized_copy(step.label)).size(CAPTION_SIZE + 1);
        label = match mark {
            StepMark::Current => label.bold().style(theme::text::primary),
            StepMark::Halted => label
                .bold()
                .style(|t: &theme::Theme| iced::widget::text::Style {
                    color: Some(t.colors.shell.bad),
                }),
            _ => label.style(theme::text::secondary),
        };
        let mut caption = Row::new().spacing(6).align_y(Alignment::Center);
        if matches!(mark, StepMark::Done | StepMark::DoneMuted) {
            caption = caption.push(check_icon().size(12).style(move |t: &theme::Theme| {
                iced::widget::text::Style {
                    color: Some(if mark == StepMark::Done {
                        t.colors.shell.good
                    } else {
                        t.colors.shell.muted
                    }),
                }
            }));
        }
        row = row.push(
            Column::new()
                .spacing(8)
                .width(Length::FillPortion(1))
                .push(bar)
                .push(caption.push(label)),
        );
    }
    row.into()
}

/// One segment of a progress bar, coloured from the shell palette.
fn progress_bar<'a>(
    height: f32,
    color: impl Fn(&coincube_ui::theme::palette::ShellPalette) -> iced::Color + 'a,
) -> Element<'a, Msg> {
    Container::new(Space::new().height(Length::Fixed(height)))
        .width(Length::Fill)
        .style(move |t: &theme::Theme| container::Style {
            background: Some(Background::Color(color(&t.colors.shell))),
            border: iced::Border {
                radius: (height / 2.0).into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// The in-progress segment's pulse: full colour, then its dim half.
fn pulsed(color: iced::Color, dim: bool) -> iced::Color {
    if dim {
        iced::Color { a: 0.4, ..color }
    } else {
        color
    }
}

fn guidance_text<'a>(copy: String) -> Element<'a, Msg> {
    text(copy).size(P2_SIZE).style(theme::text::primary).into()
}

/// Spark-specific status guidance. Diverges from the shared
/// [`ShiftStatusKind::guidance`] where the Spark flow knows more:
///
/// - `Waiting` names the asset and chain, since the deposit address sits just
///   below.
/// - `Pending` / `Processing` are separate tracker steps here, so each says
///   which half of the swap is running.
/// - `Settled`: a Spark settle lands as on-chain BTC in the wallet's static
///   deposit address, so it *hasn't* arrived in the spendable balance yet —
///   it's confirming, and gets added automatically once it matures. The shared
///   copy says "has arrived", which sent users looking for a transaction that
///   wasn't there yet.
fn spark_shift_guidance(status: Option<&ShiftStatusKind>, selected: &DepositOption) -> String {
    match status {
        None | Some(ShiftStatusKind::Waiting) => localized_format!(
            "Send {} on {} to the address below. This page updates when the deposit is seen.",
            "Envía {} en {} a la dirección de abajo. Esta página se actualiza cuando se detecte el depósito.",
            selected.coin.to_uppercase(),
            selected.network.network_name(),
        ),
        Some(ShiftStatusKind::Pending) => localized_copy(
            "SideShift has seen your deposit and is confirming it before converting.",
        )
        .to_owned(),
        Some(ShiftStatusKind::Processing) => localized_copy(
            "Converting your deposit to bitcoin. The final amount is set when the conversion \
             finishes.",
        )
        .to_owned(),
        Some(ShiftStatusKind::Settled) => localized_copy(
            "Converted. Your bitcoin is confirming on-chain and is added to your Spark wallet \
             automatically after about 3 confirmations.",
        )
        .to_owned(),
        Some(other) => localized_copy(other.guidance()).to_owned(),
    }
}

// ── Waiting: where to send ──────────────────────────────────────────────────

fn deposit_section<'a>(v: &ActiveShift<'a>) -> Element<'a, Msg> {
    let qr: Element<Msg> = match v.qr {
        Some(data) => Container::new(qr_code(data).cell_size(4))
            .padding(12)
            .style(|_: &theme::Theme| container::Style {
                background: Some(Background::Color(color::WHITE)),
                border: iced::Border {
                    radius: 10.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into(),
        None => Space::new().width(Length::Fixed(0.0)).into(),
    };

    let address = Column::new()
        .spacing(8)
        .push(
            text(localized_copy("Deposit address"))
                .size(CAPTION_SIZE + 1)
                .style(theme::text::secondary),
        )
        .push(
            text(v.shift.deposit_address.as_str())
                .size(P2_SIZE)
                .font(iced::Font::MONOSPACE)
                .width(Length::Fill)
                .wrapping(Wrapping::WordOrGlyph),
        )
        .push(
            button::secondary_compact(Some(clipboard_icon()), localized_copy("Copy address"))
                .on_press(Msg::Copy),
        );

    // Sending the wrong asset or chain to this address loses it, and sending an
    // amount outside SideShift's min/max gets it refunded rather than converted.
    // Neither can be caught at input — the deposit is an external send — so both
    // constraints sit right next to the address, once, in a calm note rather
    // than an alarm.
    let chain = if v.selected.network.standard_label() == v.selected.network.network_name() {
        v.selected.network.network_name().to_owned()
    } else {
        format!(
            "{} ({})",
            v.selected.network.network_name(),
            v.selected.network.standard_label()
        )
    };
    let mut rules = Column::new().spacing(4).push(
        text(localized_format!(
            "Only send {} on {}.",
            "Sólo envía {} en {}.",
            v.selected.coin.to_uppercase(),
            chain,
        ))
        .size(CAPTION_SIZE + 1)
        .bold()
        .style(theme::text::primary),
    );
    if let (Some(min), Some(max)) = (&v.shift.deposit_min, &v.shift.deposit_max) {
        rules = rules.push(
            text(localized_format!(
                "Send between {min} and {max} {}. Amounts outside this range are refunded.",
                "Enviar entre {min} y {max} {}. Cantidades fuera de este rango se reembolsan.",
                v.selected.coin.to_uppercase(),
            ))
            .size(CAPTION_SIZE + 1)
            .style(theme::text::primary),
        );
    }
    let note = tinted_note(Tone::Warn, warning_icon(), rules.into());

    Row::new()
        .spacing(24)
        .align_y(Alignment::Start)
        .width(Length::Fill)
        .push(qr)
        .push(
            Column::new()
                .spacing(16)
                .width(Length::Fill)
                .push(address)
                .push(note),
        )
        .into()
}

// ── Detected → arriving: what came in and what's on its way ────────────────

fn in_flight_section<'a>(
    v: &ActiveShift<'a>,
    status: &ShiftStatusKind,
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'a, Msg> {
    let settled = matches!(status, ShiftStatusKind::Settling | ShiftStatusKind::Settled);
    let deposit = deposit_tile(v, mask_amounts);
    let (label, sub) = if settled {
        ("Bitcoin on the way", "Confirming on-chain")
    } else {
        ("You will receive about", "Estimate from SideShift")
    };
    let amount: Element<Msg> = match v.settle_amount_sat {
        Some(sats) => observed_settlement_amount(sats, status, bitcoin_unit, mask_amounts),
        None => text(localized_copy("Set after conversion"))
            .size(P1_SIZE)
            .style(theme::text::secondary)
            .into(),
    };
    let receive = tile(
        settled,
        Column::new()
            .spacing(4)
            .push(
                text(localized_copy(label))
                    .size(CAPTION_SIZE + 1)
                    .style(theme::text::secondary),
            )
            .push(amount)
            .push(
                text(localized_copy(sub))
                    .size(CAPTION_SIZE + 1)
                    .style(theme::text::secondary),
            )
            .into(),
    );
    let pair = tile_pair(deposit, text("→"), receive);
    if !settled {
        return pair;
    }
    Column::new()
        .spacing(18)
        .push(pair)
        .push(confirmations_block(v.confirmations, v.pulse_dim))
        .into()
}

/// The settle transaction's progress toward the wallet's claim threshold:
/// "1 of 3", a segment per block, and roughly how long is left. Until the
/// transaction shows up among the wallet's pending deposits it reads 0.
fn confirmations_block<'a>(confirmations: Option<u32>, pulse_dim: bool) -> Element<'a, Msg> {
    let required = DEPOSIT_MATURITY_CONFIRMATIONS;
    let have = confirmations.unwrap_or(0).min(required);
    let mut bars = Row::new().spacing(6).width(Length::Fill);
    for i in 0..required {
        bars = bars.push(progress_bar(6.0, move |shell| {
            if i < have {
                shell.good
            } else if i == have {
                pulsed(shell.accent, pulse_dim)
            } else {
                shell.line
            }
        }));
    }
    let note = if have >= required {
        localized_copy("Confirmed. Adding it to your Spark wallet.").to_owned()
    } else {
        localized_format!(
            "About {} minutes left. You can leave this page; the bitcoin is added automatically. Come back to it from Spark → Receive.",
            "Faltan unos {} minutos. Puedes salir de esta página; el bitcoin se añade automáticamente. Vuelve a ella desde Spark → Recibir.",
            (required - have) * 10,
        )
    };
    Column::new()
        .spacing(8)
        .push(
            Row::new()
                .push(
                    text(localized_copy("Confirmations"))
                        .size(P2_SIZE)
                        .style(theme::text::secondary),
                )
                .push(Space::new().width(Length::Fill))
                .push(
                    text(localized_format!("{} of {}", "{} de {}", have, required)).size(P2_SIZE),
                ),
        )
        .push(bars)
        .push(
            text(note)
                .size(CAPTION_SIZE + 1)
                .style(theme::text::secondary),
        )
        .into()
}

// ── Refund path: on hold, failed, refunding ─────────────────────────────────

fn refund_section<'a>(
    v: &ActiveShift<'a>,
    status: &ShiftStatusKind,
    mask_amounts: bool,
) -> Element<'a, Msg> {
    let deposit = deposit_tile(v, mask_amounts);
    let back_to = tile(
        false,
        Column::new()
            .spacing(4)
            .push(
                text(localized_copy("Refund goes back to"))
                    .size(CAPTION_SIZE + 1)
                    .style(theme::text::secondary),
            )
            .push(
                text(short_address(v.refund_address))
                    .size(P1_SIZE)
                    .font(iced::Font::MONOSPACE),
            )
            .push(
                text(localized_format!(
                    "your {} address, as {}",
                    "su dirección de {}, como {}",
                    v.selected.network.network_name(),
                    v.selected.coin.to_uppercase(),
                ))
                .size(CAPTION_SIZE + 1)
                .style(theme::text::secondary),
            )
            .into(),
    );

    let mut col = Column::new()
        .spacing(16)
        .push(tile_pair(deposit, reload_icon(), back_to));

    if *status == ShiftStatusKind::Refunding {
        if let Some(reason) = out_of_range_reason(v, mask_amounts) {
            col = col.push(tinted_note(
                Tone::Warn,
                clock_icon(),
                text(reason)
                    .size(CAPTION_SIZE + 1)
                    .style(theme::text::primary)
                    .into(),
            ));
        }
    }
    // A held shift only moves when the user contacts SideShift, and the shift
    // ID is what they'll be asked for — put it one click away.
    if *status == ShiftStatusKind::Review {
        col = col.push(
            button::secondary_compact(Some(clipboard_icon()), localized_copy("Copy shift ID"))
                .on_press(Msg::CopyShiftId),
        );
    }
    col.into()
}

fn refunded_section<'a>(v: &ActiveShift<'a>, mask_amounts: bool) -> Element<'a, Msg> {
    let mut col = Column::new()
        .spacing(10)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .push(reload_icon().size(28).style(theme::text::secondary))
        .push(
            text(localized_copy("Refunded"))
                .size(P1_SIZE)
                .bold()
                .style(theme::text::primary),
        );
    if let Some(amount) = v.deposit_amount() {
        col = col.push(
            text(passive_shift_amount(
                format!("{} {}", amount, v.selected.coin.to_uppercase()),
                mask_amounts,
            ))
            .size(H3_SIZE)
            .bold(),
        );
    }
    let mut returned = localized_format!(
        "Returned to {} on {}.",
        "Devuelto a {} en {}.",
        short_address(v.refund_address),
        v.selected.network.network_name(),
    );
    if let Some(reason) = out_of_range_reason(v, mask_amounts) {
        returned = format!("{returned} {reason}");
    }
    col.push(
        text(returned)
            .size(P2_SIZE)
            .align_x(iced::alignment::Horizontal::Center)
            .style(theme::text::secondary),
    )
    .push(Space::new().height(Length::Fixed(6.0)))
    .push(
        button::primary(None, localized_copy("Done"))
            .on_press(Msg::Reset)
            .width(Length::Fixed(160.0)),
    )
    .into()
}

fn expired_section<'a>() -> Element<'a, Msg> {
    Column::new()
        .spacing(10)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .push(clock_icon().size(28).style(theme::text::secondary))
        .push(
            text(localized_copy("Deposit address expired"))
                .size(P1_SIZE)
                .bold()
                .style(theme::text::primary),
        )
        .push(
            text(localized_copy(ShiftStatusKind::Expired.guidance()))
                .size(P2_SIZE)
                .align_x(iced::alignment::Horizontal::Center)
                .style(theme::text::secondary),
        )
        .push(Space::new().height(Length::Fixed(6.0)))
        .push(button::primary(None, localized_copy("Get a new address")).on_press(Msg::Reset))
        .into()
}

// ── Shift details ────────────────────────────────────────────────────────────

fn details_list<'a>(v: &ActiveShift<'a>) -> Element<'a, Msg> {
    let mut rows = Column::new().spacing(8).push(detail_row(
        localized_copy("Shift ID"),
        text(v.shift.id.as_str())
            .size(P2_SIZE)
            .font(iced::Font::MONOSPACE)
            .into(),
        Some(Msg::CopyShiftId),
    ));
    if !v.refund_address.trim().is_empty() {
        rows = rows.push(detail_row(
            localized_copy("Refunds go to"),
            text(short_address(v.refund_address))
                .size(P2_SIZE)
                .font(iced::Font::MONOSPACE)
                .into(),
            Some(Msg::CopyRefundAddress),
        ));
    }
    rows.push(detail_row(
        localized_copy("Converted by"),
        text(localized_copy("SideShift (third party)"))
            .size(P2_SIZE)
            .into(),
        None,
    ))
    .into()
}

const COPY_BUTTON_WIDTH: f32 = 28.0;

fn detail_row<'a>(label: &'a str, value: Element<'a, Msg>, copy: Option<Msg>) -> Element<'a, Msg> {
    let mut row = Row::new()
        .spacing(8)
        .height(Length::Fixed(COPY_BUTTON_WIDTH))
        .align_y(Alignment::Center)
        .push(text(label).size(P2_SIZE).style(theme::text::secondary))
        .push(Space::new().width(Length::Fill))
        .push(value);
    // Rows without a copy button keep the same right edge as rows with one.
    row = match copy {
        Some(msg) => row.push(
            iced::widget::button(clipboard_icon().size(14))
                .padding(6)
                .width(Length::Fixed(COPY_BUTTON_WIDTH))
                .on_press(msg)
                .style(theme::button::transparent_border),
        ),
        None => row.push(Space::new().width(Length::Fixed(COPY_BUTTON_WIDTH))),
    };
    row.into()
}

// ── Shared pieces ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum Tone {
    Warn,
}

/// A soft, tinted note (8% fill, 35% border) — for rules the user must follow
/// or a reason something happened. Never a solid colour block: that reads as
/// an error and shouts over the address it's annotating.
fn tinted_note<'a>(
    tone: Tone,
    icon: coincube_ui::widget::Text<'a>,
    body: Element<'a, Msg>,
) -> Element<'a, Msg> {
    let tint = move |t: &theme::Theme| match tone {
        Tone::Warn => t.colors.shell.warn,
    };
    Container::new(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Start)
            .push(
                icon.size(16)
                    .style(move |t: &theme::Theme| iced::widget::text::Style {
                        color: Some(tint(t)),
                    }),
            )
            .push(body),
    )
    .padding([12, 14])
    .width(Length::Fill)
    .style(move |t: &theme::Theme| {
        let c = tint(t);
        container::Style {
            background: Some(Background::Color(iced::Color { a: 0.08, ..c })),
            border: iced::Border {
                color: iced::Color { a: 0.35, ..c },
                width: 1.0,
                radius: 10.0.into(),
            },
            ..Default::default()
        }
    })
    .into()
}

fn tile<'a>(highlight: bool, body: Element<'a, Msg>) -> Element<'a, Msg> {
    Container::new(body)
        .padding(16)
        .width(Length::Fill)
        .style(move |t: &theme::Theme| {
            let shell = &t.colors.shell;
            container::Style {
                background: Some(Background::Color(shell.background)),
                border: iced::Border {
                    color: if highlight { shell.accent } else { shell.line },
                    width: 1.0,
                    radius: 10.0.into(),
                },
                ..Default::default()
            }
        })
        .into()
}

fn tile_pair<'a>(
    left: Element<'a, Msg>,
    icon: coincube_ui::widget::Text<'a>,
    right: Element<'a, Msg>,
) -> Element<'a, Msg> {
    Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(Container::new(left).width(Length::FillPortion(1)))
        .push(icon.size(18).style(theme::text::secondary))
        .push(Container::new(right).width(Length::FillPortion(1)))
        .into()
}

fn deposit_tile<'a>(v: &ActiveShift<'a>, mask_amounts: bool) -> Element<'a, Msg> {
    let coin = v.selected.coin.to_uppercase();
    let headline = match v.deposit_amount() {
        Some(amount) => passive_shift_amount(format!("{amount} {coin}"), mask_amounts),
        None => coin,
    };
    tile(
        false,
        Column::new()
            .spacing(4)
            .push(
                Row::new()
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .push(check_icon().size(12).style(|t: &theme::Theme| {
                        iced::widget::text::Style {
                            color: Some(t.colors.shell.good),
                        }
                    }))
                    .push(
                        text(localized_copy("Deposit received"))
                            .size(CAPTION_SIZE + 1)
                            .style(theme::text::secondary),
                    ),
            )
            .push(text(headline).size(P1_SIZE).bold())
            .push(
                text(localized_format!(
                    "on {}",
                    "en {}",
                    v.selected.network.network_name()
                ))
                .size(CAPTION_SIZE + 1)
                .style(theme::text::secondary),
            )
            .into(),
    )
}

/// Why a deposit is being refunded, when the amounts say so. SideShift's status
/// carries no reason, but a deposit outside the shift's own min/max is refunded
/// by rule — so when that's what happened we can say it plainly.
///
/// With amounts masked the sentence still gives the reason, but names "the
/// deposit" instead of its value; the swap's limits are SideShift's, not the
/// user's, so they stay.
fn out_of_range_reason(v: &ActiveShift<'_>, mask_amounts: bool) -> Option<String> {
    let amount = v.deposit_amount()?;
    let coin = v.selected.coin.to_uppercase();
    let what = if mask_amounts {
        localized_copy("The deposit").to_owned()
    } else {
        format!("{amount} {coin}")
    };
    if let Some(min) = v.shift.deposit_min.as_deref() {
        if cmp_decimal(amount, min)? == Ordering::Less {
            return Some(localized_format!(
                "{what} is below the {min} {coin} minimum, so nothing was converted.",
                "{what} está por debajo del mínimo de {min} {coin}, así que no se convirtió nada.",
            ));
        }
    }
    if let Some(max) = v.shift.deposit_max.as_deref() {
        if cmp_decimal(amount, max)? == Ordering::Greater {
            return Some(localized_format!(
                "{what} is above the {max} {coin} maximum, so nothing was converted.",
                "{what} está por encima del máximo de {max} {coin}, así que no se convirtió nada.",
            ));
        }
    }
    None
}

/// Compare two non-negative decimal strings exactly ("25.08" vs "12"), without
/// a float round-trip. `None` if either isn't a plain decimal.
fn cmp_decimal(a: &str, b: &str) -> Option<Ordering> {
    fn split(s: &str) -> Option<(&str, &str)> {
        let s = s.trim();
        let (int, frac) = s.split_once('.').unwrap_or((s, ""));
        let digits = |p: &str| p.bytes().all(|c| c.is_ascii_digit());
        if (int.is_empty() && frac.is_empty()) || !digits(int) || !digits(frac) {
            return None;
        }
        Some((int.trim_start_matches('0'), frac.trim_end_matches('0')))
    }
    let (ai, af) = split(a)?;
    let (bi, bf) = split(b)?;
    Some(
        ai.len()
            .cmp(&bi.len())
            .then_with(|| ai.cmp(bi))
            .then_with(|| af.cmp(bf)),
    )
}

/// `lq1qqgp7…x7d2`: enough to recognise an address the user typed, short
/// enough for a details row. The copy button next to it copies the whole thing.
fn short_address(address: &str) -> String {
    let address = address.trim();
    let chars: Vec<char> = address.chars().collect();
    if chars.len() <= 14 {
        return address.to_owned();
    }
    let head: String = chars[..8].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

// ── Arrived: the swap's bitcoin has landed and been claimed ──────────────────

fn arrived_body(
    flow: &SparkSideshiftReceiveFlow,
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'_, Msg> {
    let mut col = Column::new().spacing(18).width(Length::Fill);
    if let Some(steps) = tracker_steps(true, None) {
        col = col.push(tracker(&steps, false)).push(rule::horizontal(1));
    }

    let mut done =
        Column::new()
            .spacing(10)
            .align_x(Alignment::Center)
            .width(Length::Fill)
            .push(check_circle_icon().size(32).style(|t: &theme::Theme| {
                iced::widget::text::Style {
                    color: Some(t.colors.shell.good),
                }
            }))
            .push(
                text(localized_copy("Bitcoin arrived"))
                    .size(P1_SIZE)
                    .bold()
                    .style(|t: &theme::Theme| iced::widget::text::Style {
                        color: Some(t.colors.shell.good),
                    }),
            );
    if let Some(sats) = flow.arrived_amount_sat() {
        done = done.push(observed_arrival_amount(sats, bitcoin_unit, mask_amounts));
    }
    col.push(
        done.push(
            text(localized_copy(
                "Your bitcoin has been added to your Spark wallet.",
            ))
            .size(P2_SIZE)
            .style(theme::text::secondary),
        )
        .push(Space::new().height(Length::Fixed(6.0)))
        .push(
            button::primary(None, localized_copy("Done"))
                .on_press(Msg::Reset)
                .width(Length::Fixed(160.0)),
        ),
    )
    .into()
}

fn settle_amount_display(
    sats: u64,
    status: &ShiftStatusKind,
    bitcoin_unit: BitcoinDisplayUnit,
) -> String {
    // Once SideShift is settling it has fixed the bitcoin output, so the
    // figure is firm — the same point the tile switches to "Bitcoin on the way".
    let firm = matches!(status, ShiftStatusKind::Settling | ShiftStatusKind::Settled);
    format!(
        "{}{} {}",
        if firm { "" } else { "≈ " },
        Amount::from_sat(sats).to_formatted_string_with_unit(bitcoin_unit),
        bitcoin_unit_label(bitcoin_unit),
    )
}

fn arrived_amount_display(sats: u64, bitcoin_unit: BitcoinDisplayUnit) -> String {
    format!(
        "{} {}",
        Amount::from_sat(sats).to_formatted_string_with_unit(bitcoin_unit),
        bitcoin_unit_label(bitcoin_unit),
    )
}

fn passive_shift_amount(amount: String, mask_amounts: bool) -> String {
    if mask_amounts {
        "••••••".to_string()
    } else {
        amount
    }
}

fn observed_settlement_amount(
    sats: u64,
    status: &ShiftStatusKind,
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'static, Msg> {
    text(passive_shift_amount(
        settle_amount_display(sats, status, bitcoin_unit),
        mask_amounts,
    ))
    .size(P1_SIZE)
    .bold()
    .into()
}

fn observed_arrival_amount(
    sats: u64,
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'static, Msg> {
    text(passive_shift_amount(
        arrived_amount_display(sats, bitcoin_unit),
        mask_amounts,
    ))
    .size(H3_SIZE)
    .bold()
    .into()
}

fn bitcoin_unit_label(bitcoin_unit: BitcoinDisplayUnit) -> &'static str {
    if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
        "BTC"
    } else {
        "SATS"
    }
}

// ── Loading / error ─────────────────────────────────────────────────────────

fn loading_body<'a>(label: &'a str) -> Element<'a, Msg> {
    Container::new(text(label).size(P1_SIZE).style(theme::text::secondary))
        .padding(24)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

fn error_body(error: Option<&str>) -> Element<'_, Msg> {
    Column::new()
        .spacing(16)
        .push(
            Container::new(
                Column::new()
                    .spacing(8)
                    .push(h4_bold(localized_copy("Something went wrong")))
                    .push(
                        text(coincube_ui::locale::notice(
                            error.unwrap_or("The swap couldn't be set up.").to_string(),
                        ))
                        .size(P2_SIZE)
                        .style(theme::text::secondary),
                    ),
            )
            .padding(16)
            .width(Length::Fill)
            .style(theme::card::error),
        )
        .push(
            button::primary(None, localized_copy("Start over"))
                .on_press(Msg::Reset)
                .width(Length::Fixed(160.0)),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::cache::Cache;
    use crate::app::state::preferences::{BalancePrivacy, BalancePrivacyChrome};
    use crate::app::view::shared::landing::capture;

    fn marks(steps: [TrackerStep; 4]) -> [StepMark; 4] {
        steps.map(|s| s.mark)
    }
    fn labels(steps: [TrackerStep; 4]) -> [&'static str; 4] {
        steps.map(|s| s.label)
    }

    #[test]
    fn tracker_walks_the_convert_path_one_step_per_status() {
        use StepMark::*;
        for (status, expected) in [
            (None, [Current, Todo, Todo, Todo]),
            (Some(ShiftStatusKind::Waiting), [Current, Todo, Todo, Todo]),
            (Some(ShiftStatusKind::Pending), [Done, Current, Todo, Todo]),
            (
                Some(ShiftStatusKind::Processing),
                [Done, Done, Current, Todo],
            ),
            (Some(ShiftStatusKind::Settling), [Done, Done, Done, Current]),
            (Some(ShiftStatusKind::Settled), [Done, Done, Done, Current]),
        ] {
            let steps = tracker_steps(false, status.as_ref()).unwrap();
            assert_eq!(labels(steps), CONVERT_STEPS);
            assert_eq!(marks(steps), expected, "{:?}", status);
        }
        let arrived = tracker_steps(true, None).unwrap();
        assert_eq!(marks(arrived), [Done; 4]);
    }

    #[test]
    fn tracker_switches_to_the_refund_path_once_a_swap_stops() {
        use StepMark::*;
        let review = tracker_steps(false, Some(&ShiftStatusKind::Review)).unwrap();
        assert_eq!(
            labels(review),
            [
                "Deposit received",
                "On hold for review",
                "Refunding",
                "Refunded"
            ]
        );
        assert_eq!(marks(review), [Done, Halted, Todo, Todo]);

        let failed = tracker_steps(false, Some(&ShiftStatusKind::Error)).unwrap();
        assert_eq!(failed[1].label, "Swap failed");
        assert_eq!(marks(failed), [Done, Halted, Todo, Todo]);

        let refunding = tracker_steps(false, Some(&ShiftStatusKind::Refunding)).unwrap();
        assert_eq!(refunding[1].label, "Swap stopped");
        assert_eq!(marks(refunding), [Done, DoneMuted, Current, Todo]);

        let refunded = tracker_steps(false, Some(&ShiftStatusKind::Refunded)).unwrap();
        assert_eq!(marks(refunded), [Done, DoneMuted, Done, Done]);

        assert!(tracker_steps(false, Some(&ShiftStatusKind::Expired)).is_none());
    }

    fn liquid_usdt() -> DepositOption {
        crate::services::sideshift::deposit_options()
            .iter()
            .copied()
            .find(|o| o.coin == "usdt" && o.network.network_name() == "Liquid")
            .expect("USDt on Liquid is a deposit option")
    }

    fn sample_shift() -> ShiftResponse {
        ShiftResponse {
            id: "057b58372a64a80defb8".into(),
            deposit_address: "lq1qqgp7u5wmv7na5rrr2fqensxelf93yj3cm3dvleyhksk3vqhh4ke7x6ngc5mwz0k8h2tm3l4a0y6c9xqvwdu5x2hqnf7yjr5gdpkaz8e3".into(),
            deposit_coin: Some("USDT".into()),
            deposit_network: Some("liquid".into()),
            settle_address: None,
            settle_coin: Some("BTC".into()),
            settle_network: Some("bitcoin".into()),
            deposit_min: Some("25.08".into()),
            deposit_max: Some("30000".into()),
            deposit_amount: None,
            settle_amount: None,
            rate: None,
            expires_at: None,
            created_at: None,
            status: None,
            affiliate_fee_percent: None,
            network_fee_usd: None,
        }
    }

    /// Every status mounts the body it should — and only that body — with the
    /// shift details always underneath. `R06A_CAPTURE_DIR` saves each one.
    #[test]
    fn every_shift_status_renders_its_own_body_with_details() {
        let shift = sample_shift();
        let qr = iced::widget::qr_code::Data::new(&shift.deposit_address).unwrap();
        let refund = "lq1qqvq9d3fkk0x2l5ffhaw5hm7r3w0yq0t5p4yq9ax7d2";
        for (name, status, deposit, settle, present, absent) in [
            (
                "waiting",
                Some(ShiftStatusKind::Waiting),
                None,
                None,
                vec![
                    "Waiting for deposit",
                    "Deposit address",
                    "Copy address",
                    "Only send USDT on Liquid.",
                    "Send between 25.08 and 30000 USDT. Amounts outside this range are refunded.",
                ],
                vec!["Deposit received", "Refund goes back to"],
            ),
            (
                "detected",
                Some(ShiftStatusKind::Pending),
                Some("50.00"),
                Some(31_250),
                vec![
                    "Deposit received",
                    "50.00 USDT",
                    "You will receive about",
                    "≈ 31,250 SATS",
                ],
                vec!["Copy address", "Refund goes back to"],
            ),
            (
                "converting",
                Some(ShiftStatusKind::Processing),
                Some("50.00"),
                None,
                vec![
                    "Converting",
                    "Set after conversion",
                    "Estimate from SideShift",
                ],
                vec!["Copy address"],
            ),
            (
                "arriving",
                Some(ShiftStatusKind::Settled),
                Some("50.00"),
                Some(31_250),
                vec![
                    "Bitcoin on the way",
                    "31,250 SATS",
                    "Confirming on-chain",
                    "Confirmations",
                    "1 of 3",
                    "About 20 minutes left. You can leave this page; the bitcoin is added automatically. Come back to it from Spark → Receive.",
                ],
                vec!["≈ 31,250 SATS", "Copy address"],
            ),
            (
                "review",
                Some(ShiftStatusKind::Review),
                Some("50.00"),
                None,
                vec!["On hold for review", "Refund goes back to", "Copy shift ID"],
                vec!["Copy address", "minimum"],
            ),
            (
                "failed",
                Some(ShiftStatusKind::Error),
                Some("50.00"),
                None,
                vec!["Swap failed", "Refund goes back to"],
                vec!["Copy shift ID"],
            ),
            (
                "refunding",
                Some(ShiftStatusKind::Refunding),
                Some("12.00"),
                None,
                vec![
                    "Swap stopped",
                    "Refund goes back to",
                    "12.00 USDT is below the 25.08 USDT minimum, so nothing was converted.",
                ],
                vec!["Copy address"],
            ),
            (
                "refunded",
                Some(ShiftStatusKind::Refunded),
                Some("12.00"),
                None,
                vec!["Refunded", "12.00 USDT", "Done"],
                vec!["Copy address", "Refund goes back to"],
            ),
            (
                "expired",
                Some(ShiftStatusKind::Expired),
                None,
                None,
                vec!["Deposit address expired", "Get a new address"],
                vec!["Waiting for deposit", "Copy address"],
            ),
        ] {
            let view = ActiveShift {
                shift: &shift,
                selected: liquid_usdt(),
                status: status.as_ref(),
                qr: Some(&qr),
                refund_address: refund,
                polled_deposit_amount: deposit,
                settle_amount_sat: settle,
                confirmations: if name == "arriving" { Some(1) } else { None },
                pulse_dim: false,
            };
            for (appearance, theme) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                let mounted = capture::inspect_wide_content(
                    &format!("spark-shift-{name}"),
                    active_shift_body(&view, BitcoinDisplayUnit::Sats, false),
                    (1440, 900),
                    appearance,
                    theme,
                    632,
                );
                for label in present.iter().chain(
                    [
                        "Shift ID",
                        "057b58372a64a80defb8",
                        "Refunds go to",
                        "lq1qqvq9…x7d2",
                    ]
                    .iter(),
                ) {
                    assert!(
                        mounted.contains(label),
                        "{}/{}: missing {}",
                        name,
                        appearance,
                        label
                    );
                }
                for label in &absent {
                    assert!(
                        !mounted.contains(label),
                        "{}/{}: unexpected {}",
                        name,
                        appearance,
                        label
                    );
                }
            }
        }
    }

    #[test]
    fn refund_reason_hides_the_deposit_amount_when_amounts_are_masked() {
        let mut shift = sample_shift();
        shift.deposit_amount = Some("12.00".into());
        let view = ActiveShift {
            shift: &shift,
            selected: liquid_usdt(),
            status: Some(&ShiftStatusKind::Refunding),
            qr: None,
            refund_address: "lq1qqvq9d3fkk0x2l5ffhaw5hm7r3w0yq0t5p4yq9ax7d2",
            polled_deposit_amount: None,
            settle_amount_sat: None,
            confirmations: None,
            pulse_dim: false,
        };
        assert_eq!(
            out_of_range_reason(&view, false).as_deref(),
            Some("12.00 USDT is below the 25.08 USDT minimum, so nothing was converted.")
        );
        let masked = out_of_range_reason(&view, true).unwrap();
        assert_eq!(
            masked,
            "The deposit is below the 25.08 USDT minimum, so nothing was converted."
        );
        assert!(!masked.contains("12.00"));

        for status in [ShiftStatusKind::Refunding, ShiftStatusKind::Refunded] {
            let view = ActiveShift {
                status: Some(&status),
                ..view
            };
            let mounted = capture::inspect_wide_content(
                "spark-shift-refund-masked",
                active_shift_body(&view, BitcoinDisplayUnit::Sats, true),
                (1440, 900),
                "dark",
                theme::Theme::dark(),
                632,
            );
            assert!(!mounted.contains("12.00"), "{:?} leaked the amount", status);
            assert!(mounted.contains("The deposit is below the 25.08 USDT minimum"));
        }
    }

    #[test]
    fn decimal_comparison_is_exact_and_rejects_non_decimals() {
        assert_eq!(cmp_decimal("12.00", "25.08"), Some(Ordering::Less));
        assert_eq!(cmp_decimal("25.08", "25.080"), Some(Ordering::Equal));
        assert_eq!(cmp_decimal("30000.01", "30000"), Some(Ordering::Greater));
        assert_eq!(cmp_decimal("100", "99.999"), Some(Ordering::Greater));
        assert_eq!(cmp_decimal("0.5", ".5"), Some(Ordering::Equal));
        assert_eq!(cmp_decimal("007", "7"), Some(Ordering::Equal));
        assert_eq!(cmp_decimal("1e3", "2"), None);
        assert_eq!(cmp_decimal("", "2"), None);
        assert_eq!(cmp_decimal("-1", "2"), None);
    }

    #[test]
    fn short_address_keeps_head_and_tail() {
        assert_eq!(
            short_address("lq1qqgp7u5wmv7na5rrr2fqensxelf93yj3cm3dx7d2"),
            "lq1qqgp7…x7d2"
        );
        assert_eq!(short_address(" bc1qshort "), "bc1qshort");
    }

    #[test]
    fn observed_settle_and_arrival_amounts_follow_passive_privacy_authority() {
        let mut cache = Cache::default();
        for privacy in [
            None,
            Some(BalancePrivacy::Unresolved),
            Some(BalancePrivacy::Hidden),
            Some(BalancePrivacy::Shown),
        ] {
            cache.balance_privacy = privacy.map(|privacy| BalancePrivacyChrome {
                privacy,
                busy: false,
                interactive: false,
            });
            for (name, amount, content) in [
                (
                    "spark-shift-settle-privacy",
                    settle_amount_display(
                        76_543_219,
                        &ShiftStatusKind::Settled,
                        BitcoinDisplayUnit::Sats,
                    ),
                    observed_settlement_amount(
                        76_543_219,
                        &ShiftStatusKind::Settled,
                        BitcoinDisplayUnit::Sats,
                        cache.balance_amounts_masked(),
                    ),
                ),
                (
                    "spark-shift-arrival-privacy",
                    arrived_amount_display(87_654_321, BitcoinDisplayUnit::Sats),
                    observed_arrival_amount(
                        87_654_321,
                        BitcoinDisplayUnit::Sats,
                        cache.balance_amounts_masked(),
                    ),
                ),
            ] {
                let mounted =
                    capture::inspect(name, content, (1440, 900), "dark", theme::Theme::dark());
                if privacy == Some(BalancePrivacy::Shown) {
                    assert!(mounted.contains(&amount));
                } else {
                    assert!(mounted.contains("••••••"));
                    assert!(!mounted.contains(&amount));
                }
            }
        }
    }

    #[test]
    fn spark_guidance_overrides_the_statuses_it_knows_more_about() {
        let usdt = crate::services::sideshift::deposit_options()
            .iter()
            .copied()
            .find(|o| o.coin == "usdt")
            .unwrap();

        let waiting = spark_shift_guidance(Some(&ShiftStatusKind::Waiting), &usdt);
        assert!(waiting.contains("USDT on "));
        assert!(waiting.contains("address below"));

        let settled = spark_shift_guidance(Some(&ShiftStatusKind::Settled), &usdt);
        assert_ne!(settled, ShiftStatusKind::Settled.guidance());
        assert!(settled.contains("confirming on-chain"));
        assert!(settled.contains("automatically"));
        assert!(settled.contains("3 confirmations"));
        assert!(!settled.contains("has arrived"));

        for status in [ShiftStatusKind::Pending, ShiftStatusKind::Processing] {
            assert_ne!(
                spark_shift_guidance(Some(&status), &usdt),
                status.guidance()
            );
        }
    }

    #[test]
    fn refund_and_stopped_guidance_delegates_to_shared_status_copy() {
        let usdt = crate::services::sideshift::deposit_options()[0];
        for status in [
            ShiftStatusKind::Settling,
            ShiftStatusKind::Expired,
            ShiftStatusKind::Review,
            ShiftStatusKind::Refunding,
            ShiftStatusKind::Refunded,
            ShiftStatusKind::Error,
        ] {
            assert_eq!(
                spark_shift_guidance(Some(&status), &usdt),
                status.guidance()
            );
        }
    }

    #[test]
    fn settle_amount_display_is_approx_until_settling_and_uses_selected_unit() {
        assert_eq!(
            settle_amount_display(
                123_456_789,
                &ShiftStatusKind::Settling,
                BitcoinDisplayUnit::Sats
            ),
            "123,456,789 SATS"
        );
        assert_eq!(
            settle_amount_display(
                123_456_789,
                &ShiftStatusKind::Processing,
                BitcoinDisplayUnit::BTC
            ),
            "≈ 1.23 456 789 BTC"
        );
        assert_eq!(
            settle_amount_display(
                123_456_789,
                &ShiftStatusKind::Settled,
                BitcoinDisplayUnit::BTC
            ),
            "1.23 456 789 BTC"
        );
        assert_eq!(
            settle_amount_display(
                123_456_789,
                &ShiftStatusKind::Pending,
                BitcoinDisplayUnit::Sats
            ),
            "≈ 123,456,789 SATS"
        );
    }

    #[test]
    fn arrived_amount_display_is_never_approximate() {
        assert_eq!(
            arrived_amount_display(42_000, BitcoinDisplayUnit::Sats),
            "42,000 SATS"
        );
        assert_eq!(
            arrived_amount_display(42_000, BitcoinDisplayUnit::BTC),
            "0.00 042 000 BTC"
        );
    }
}
