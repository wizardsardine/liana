//! View renderer for [`crate::app::state::spark::receive::SparkReceive`].

use std::collections::HashMap;

use coincube_core::miniscript::bitcoin::{Amount, Network};
use coincube_ui::{
    color,
    component::{
        amount::{BitcoinDisplayUnit, DisplayAmount},
        button,
        text::*,
    },
    icon,
    image::{asset_network_logo, network_logo, usdt_network_logo},
    theme,
    widget::{Column, Container, Element, Row},
};
use iced::{
    widget::{button as iced_button, container, qr_code, scrollable, text_input, QRCode, Space},
    Alignment, Background, Length,
};

use coincube_spark_protocol::DepositInfo;

use crate::app::state::spark::cross_chain::supported_on;
use crate::app::state::spark::esplora::DEPOSIT_MATURITY_CONFIRMATIONS;
use crate::app::state::spark::receive::{SparkReceiveMethod, SparkReceivePhase};
use crate::app::view::shared::picker::picker_modal_card;
use crate::app::view::spark::{last_tx::last_transactions_section, SparkRecentTransaction};
use crate::app::view::{Message, SparkReceiveMessage};
use crate::services::sideshift::{deposit_options, DepositOption};

pub struct SparkReceiveView<'a> {
    pub backend_available: bool,
    pub method: SparkReceiveMethod,
    pub amount_input: &'a str,
    pub description_input: &'a str,
    pub phase: &'a SparkReceivePhase,
    pub qr_data: Option<&'a qr_code::Data>,
    /// Phase 4f: list of pending on-chain deposits surfaced from the
    /// SDK's `list_unclaimed_deposits` RPC. Rendered as a dedicated
    /// card below the main phase body.
    pub pending_deposits: &'a [DepositInfo],
    /// Phase 4f: which deposit is currently being claimed (in-flight
    /// RPC). Used to disable the row's button while waiting.
    pub claiming: Option<&'a (String, u32)>,
    /// Phase 4f: transient error from the most recent claim attempt.
    pub claim_error: Option<&'a str>,
    /// Live on-chain confirmation count per pending deposit, keyed by
    /// `(txid, vout)`. Sourced from a public Esplora and refreshed on a
    /// 30s tick; missing keys render the SDK's plain fallback text.
    pub pending_deposit_confirmations: &'a HashMap<(String, u32), u32>,
    /// Bitcoin network the cube is running on. Drives the Pending
    /// Deposits explanatory copy (live counts only available on mainnet).
    pub network: Network,
    pub received_amount_display: &'a str,
    pub received_celebration_context: &'a str,
    pub received_quote: &'a coincube_ui::component::quote_display::Quote,
    pub received_image_handle: &'a iced::widget::image::Handle,
    pub recent_transactions: &'a [SparkRecentTransaction],
    /// Unified balance (sats: BTC + Stable Balance), shown on the YOU RECEIVE card.
    pub balance_sats: u64,
    pub bitcoin_unit: BitcoinDisplayUnit,
    pub show_direction_badges: bool,
    /// When the cross-network (SideShift) flow is active, its inline body —
    /// rendered below the two cards in place of the Bitcoin rail form.
    pub sideshift_body: Option<Element<'a, Message>>,
    /// The cross-network asset the SideShift flow is set to (for the THEY SEND
    /// card). `None` when a Bitcoin rail is selected.
    pub cross_network_selected: Option<DepositOption>,
}

impl<'a> SparkReceiveView<'a> {
    pub fn render(self) -> Element<'a, Message> {
        if !self.backend_available {
            return Column::new()
                .spacing(20)
                .push(p1_regular(
                    "Spark is not available for this cube. Set up a Spark \
                     signer to receive payments.",
                ))
                .into();
        }

        // ── Full-screen celebration for received payments ─────────────
        if let SparkReceivePhase::Received { count, .. } = self.phase {
            let verb_phrase = if *count > 1 {
                "have arrived."
            } else {
                "has arrived."
            };
            return coincube_ui::component::received_celebration_page(
                self.received_celebration_context,
                self.received_amount_display,
                self.received_quote,
                self.received_image_handle,
                verb_phrase,
                Message::SparkReceive(SparkReceiveMessage::Reset),
            );
        }

        let mut content = Column::new().spacing(20);

        // ── Two-card selector: YOU RECEIVE (Bitcoin) ← THEY SEND ───────
        // Replaces the old Method chips + "Receive from another network" card.
        // Tapping the THEY SEND card opens the unified picker modal (Bitcoin
        // rails + cross-network assets), wired at the state's `view()` since the
        // picker-open flag lives there.
        content = content.push(spark_receive_cards(
            self.method,
            self.cross_network_selected,
            self.balance_sats,
            self.bitcoin_unit,
        ));

        if let Some(body) = self.sideshift_body {
            // Cross-network (SideShift) selected: its refund/deposit flow renders
            // inline here — the same slot the Bitcoin rail form would occupy.
            content = content.push(body);
        } else {
            // ── Bitcoin rail: method-specific inputs ──────────────────
            let method_form: Element<'a, Message> = if self.method == SparkReceiveMethod::Bolt11 {
                let amount = text_input("Amount in sats (optional)", self.amount_input)
                    .on_input(|v| {
                        Message::SparkReceive(
                            crate::app::view::SparkReceiveMessage::AmountInputChanged(v),
                        )
                    })
                    .padding(10);

                let description = text_input(
                    "Description shown to the payer (optional)",
                    self.description_input,
                )
                .on_input(|v| {
                    Message::SparkReceive(
                        crate::app::view::SparkReceiveMessage::DescriptionInputChanged(v),
                    )
                })
                .padding(10);

                Container::new(
                    Column::new()
                        .spacing(10)
                        .push(h4_bold("Invoice details"))
                        .push(amount)
                        .push(Space::new().height(Length::Fixed(6.0)))
                        .push(description),
                )
                .padding(16)
                .style(theme::card::simple)
                .into()
            } else if self.method == SparkReceiveMethod::OnchainBitcoin {
                // On-chain: nothing to configure. Explain the deposit
                // model so the user knows what to expect.
                Container::new(
                    Column::new()
                        .spacing(8)
                        .push(h4_bold("Spark on-chain receive"))
                        .push(p2_regular(format!(
                            "Spark uses a deposit-address model — send BTC to \
                             the address below and Spark will notice the \
                             incoming transaction automatically. The deposit \
                             becomes spendable once it has {} on-chain \
                             confirmations, at which point the SDK claims it \
                             into your wallet.",
                            DEPOSIT_MATURITY_CONFIRMATIONS,
                        ))),
                )
                .padding(16)
                .style(theme::card::simple)
                .into()
            } else {
                // Spark: static, reusable, identity-bound address. No
                // amount/invoice form — mirrors the on-chain UX.
                Container::new(
                    Column::new()
                        .spacing(8)
                        .push(h4_bold("Spark address"))
                        .push(p2_regular(
                            "This is a static, reusable Spark address bound to \
                             this wallet's identity. Share it freely — anyone on \
                             Spark can pay it directly. Spark-to-Spark transfers \
                             settle instantly with a lower fee than Lightning or \
                             on-chain, and the address is token-agnostic (it can \
                             receive bitcoin or any Spark token).",
                        )),
                )
                .padding(16)
                .style(theme::card::simple)
                .into()
            };
            content = content.push(method_form);

            // ── Phase-specific body ───────────────────────────────────
            content = content.push(phase_body(self.phase, self.qr_data));
        }

        // ── Phase 4f: pending on-chain deposits card ──────────────────
        // Renders only when there's something to show. The list
        // refreshes automatically on `DepositsChanged` events from
        // the bridge, so the card appears the moment the SDK
        // observes an incoming deposit (no manual refresh needed).
        if !self.pending_deposits.is_empty() || self.claim_error.is_some() {
            content = content.push(pending_deposits_card(
                self.pending_deposits,
                self.claiming,
                self.claim_error,
                self.pending_deposit_confirmations,
                self.network,
            ));
        }

        // ── Last transactions ─────────────────────────────────────────
        content = content.push(last_transactions_section(
            self.recent_transactions,
            self.bitcoin_unit,
            self.show_direction_badges,
            |idx| Message::SparkReceive(SparkReceiveMessage::SelectTransaction(idx)),
            Message::SparkReceive(SparkReceiveMessage::History),
        ));

        content.into()
    }
}

fn pending_deposits_card<'a>(
    deposits: &'a [DepositInfo],
    claiming: Option<&'a (String, u32)>,
    claim_error: Option<&'a str>,
    confirmations: &'a HashMap<(String, u32), u32>,
    network: Network,
) -> Element<'a, Message> {
    let mut card = Column::new()
        .spacing(12)
        .push(h4_bold("Pending deposits"))
        .push(p2_regular(format!(
            "Spark notices incoming on-chain transactions automatically. \
             Each deposit becomes claimable after {} on-chain confirmations, \
             at which point the SDK claims it into your wallet.",
            DEPOSIT_MATURITY_CONFIRMATIONS,
        )));

    if let Some(err) = claim_error {
        card = card.push(p2_regular(format!("Claim failed: {}", err)));
    }

    for deposit in deposits {
        let confs = confirmations
            .get(&(deposit.txid.clone(), deposit.vout))
            .copied();
        card = card.push(deposit_row(deposit, claiming, confs, network));
    }

    Container::new(card)
        .padding(16)
        .style(theme::card::simple)
        .into()
}

fn deposit_row<'a>(
    deposit: &'a DepositInfo,
    claiming: Option<&'a (String, u32)>,
    confirmations: Option<u32>,
    network: Network,
) -> Element<'a, Message> {
    let is_being_claimed = claiming
        .map(|(txid, vout)| txid == &deposit.txid && *vout == deposit.vout)
        .unwrap_or(false);

    let amount_label = format!("{} sats", deposit.amount_sat);
    let txid_short = if deposit.txid.len() > 16 {
        format!(
            "{}…{}",
            &deposit.txid[..8],
            &deposit.txid[deposit.txid.len() - 8..]
        )
    } else {
        deposit.txid.clone()
    };
    let txid_label = format!("{}:{}", txid_short, deposit.vout);

    // Right-side action: either a Claim button (mature, idle), a
    // disabled "Claiming…" button (in-flight), or a status text
    // (immature / errored).
    let action: Element<'_, Message> = if is_being_claimed {
        // Disabled button — `on_press_maybe(None)` keeps the visual
        // weight without firing on click.
        button::primary(None, "Claiming…")
            .on_press_maybe(None)
            .width(Length::Fixed(140.0))
            .into()
    } else if !deposit.is_mature {
        // Immature — show "X / N confirmations" if Esplora has
        // answered, otherwise the SDK-driven fallback wording. The
        // fallback also applies on networks where no public Esplora
        // is available (regtest).
        let label = match confirmations {
            Some(c) if network == Network::Bitcoin => {
                format!("{} / {} confirmations", c, DEPOSIT_MATURITY_CONFIRMATIONS)
            }
            _ => format!(
                "Waiting for confirmations ({} required)",
                DEPOSIT_MATURITY_CONFIRMATIONS
            ),
        };
        p2_regular(label).into()
    } else if let Some(err) = &deposit.claim_error {
        // Previous claim attempt failed for a reason the SDK
        // surfaces. Show a short hint + Retry button.
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(p2_regular(format!("Error: {}", short_error(err))))
            .push(
                button::secondary(None, "Retry")
                    .on_press(Message::SparkReceive(
                        crate::app::view::SparkReceiveMessage::ClaimDepositRequested {
                            txid: deposit.txid.clone(),
                            vout: deposit.vout,
                        },
                    ))
                    .width(Length::Fixed(100.0)),
            )
            .into()
    } else {
        button::primary(None, "Claim")
            .on_press(Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::ClaimDepositRequested {
                    txid: deposit.txid.clone(),
                    vout: deposit.vout,
                },
            ))
            .width(Length::Fixed(140.0))
            .into()
    };

    Container::new(
        Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(
                Column::new()
                    .spacing(4)
                    .width(Length::Fill)
                    .push(p1_regular(amount_label))
                    .push(p2_regular(txid_label)),
            )
            .push(action),
    )
    .padding(10)
    .style(theme::card::simple)
    .into()
}

/// Truncate a long SDK error string for inline display. The full
/// error stays in the panel state; this just keeps the row from
/// blowing up vertically.
fn short_error(err: &str) -> String {
    const MAX_CHARS: usize = 60;
    if err.chars().count() <= MAX_CHARS {
        err.to_string()
    } else {
        format!("{}…", err.chars().take(MAX_CHARS).collect::<String>())
    }
}

// ── Two-card "YOU RECEIVE ← THEY SEND" selector + picker modal ──────────────

/// Badge slug (for the composited logo) + display label for a Bitcoin rail.
fn rail_display(method: SparkReceiveMethod) -> (&'static str, &'static str) {
    match method {
        SparkReceiveMethod::Bolt11 => ("lightning", "Lightning"),
        SparkReceiveMethod::OnchainBitcoin => ("bitcoin", "On-chain"),
        SparkReceiveMethod::Spark => ("spark", "Spark"),
    }
}

/// Small orange-outlined pill (asset network / rail label).
fn orange_badge<'a>(label: &str) -> Element<'a, Message> {
    Container::new(text(label.to_uppercase()).size(11).color(color::ORANGE))
        .padding([2, 8])
        .style(|_: &theme::Theme| container::Style {
            border: iced::Border {
                color: color::ORANGE,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Hover-highlight style shared by the tappable selector cards.
fn card_button_style(
    _: &theme::Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    iced::widget::button::Style {
        background: Some(Background::Color(color::TRANSPARENT)),
        border: iced::Border {
            color: if matches!(status, iced::widget::button::Status::Hovered) {
                color::ORANGE
            } else {
                color::TRANSPARENT
            },
            width: 1.0,
            radius: 16.0.into(),
        },
        ..Default::default()
    }
}

/// The "YOU RECEIVE (Bitcoin) ← THEY SEND (rail)" pair. YOU RECEIVE is fixed —
/// Spark only ever receives bitcoin — so only THEY SEND is tappable.
fn spark_receive_cards<'a>(
    method: SparkReceiveMethod,
    cross: Option<DepositOption>,
    balance_sats: u64,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let you_receive = Container::new(
        Column::new()
            .spacing(6)
            .push(
                text("YOU RECEIVE")
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
            )
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(asset_network_logo("btc", "spark", 40.0))
                    .push(
                        text("Bitcoin")
                            .size(H3_SIZE)
                            .bold()
                            .style(theme::text::primary),
                    ),
            )
            .push(
                text(format!(
                    "{} {}",
                    Amount::from_sat(balance_sats).to_formatted_string_with_unit(bitcoin_unit),
                    if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                        "BTC"
                    } else {
                        "SATS"
                    }
                ))
                .size(P2_SIZE)
                .style(theme::text::secondary),
            )
            .push(orange_badge("SPARK")),
    )
    .padding(16)
    .width(Length::Fill)
    .height(Length::Fixed(160.0))
    .style(theme::card::simple);

    // THEY SEND reflects the current pick: a Bitcoin rail, or a cross-network
    // asset while the SideShift flow is active.
    let (they_icon, they_label, they_badge): (Element<'a, Message>, String, String) = match cross {
        Some(option) => (
            cross_network_logo(&option, 40.0),
            asset_display_name(option.coin).to_string(),
            option.network.network_name().to_string(),
        ),
        None => {
            let (net_slug, badge) = rail_display(method);
            (
                asset_network_logo("btc", net_slug, 40.0),
                "Bitcoin".to_string(),
                badge.to_string(),
            )
        }
    };
    let they_send = iced_button(
        Container::new(
            Column::new()
                .spacing(6)
                .push(
                    text("THEY SEND")
                        .size(P2_SIZE)
                        .style(theme::text::secondary),
                )
                .push(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(they_icon)
                        .push(
                            text(they_label)
                                .size(H3_SIZE)
                                .bold()
                                .style(theme::text::primary),
                        ),
                )
                .push(Space::new().height(Length::Fixed(18.0)))
                .push(orange_badge(&they_badge)),
        )
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fixed(160.0))
        .style(theme::card::simple),
    )
    .padding(0)
    .on_press(Message::SparkReceive(SparkReceiveMessage::OpenSenderPicker))
    .style(card_button_style);

    Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(Container::new(you_receive).width(Length::FillPortion(1)))
        .push(text("←").size(H3_SIZE).style(theme::text::secondary))
        .push(Container::new(they_send).width(Length::FillPortion(1)))
        .into()
}

/// Logo for a cross-network deposit option. Only USDt has a correct coin SVG;
/// every other asset falls back to the plain network logo so we never show a
/// *wrong* coin (the row label carries the asset name). Mirrors
/// `sideshift_receive::deposit_option_logo`.
fn cross_network_logo<'a>(option: &DepositOption, size: f32) -> Element<'a, Message> {
    match option.coin {
        "usdt" => usdt_network_logo(option.network.network_slug(), size),
        "usdc" => asset_network_logo("usdc", option.network.network_slug(), size),
        _ => network_logo(option.network.network_slug())
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .into(),
    }
}

/// Human asset name from a SideShift coin slug.
fn asset_display_name(coin: &str) -> &str {
    match coin {
        "usdt" => "USDt",
        "usdc" => "USDC",
        "eth" => "Ether",
        "sol" => "Solana",
        "trx" => "Tron",
        other => other,
    }
}

/// One selectable row in the THEY SEND modal. Adapted from Liquid's
/// `receive_picker_row`.
fn spark_picker_row<'a>(
    ico: impl Into<Element<'a, Message>>,
    label: &str,
    network: &str,
    is_selected: bool,
    on_press: Message,
) -> Element<'a, Message> {
    let mut row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(ico)
        .push(
            text(label.to_string())
                .size(14)
                .bold()
                .style(theme::text::primary),
        )
        .push(
            Container::new(text(network.to_uppercase()).size(10).color(color::ORANGE))
                .padding([2, 6])
                .style(|_: &theme::Theme| container::Style {
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..Default::default()
                }),
        )
        .push(Space::new().width(Length::Fill));

    if is_selected {
        row = row.push(icon::check2_icon().size(18).color(color::ORANGE));
    }

    iced_button(
        Container::new(row)
            .padding([12, 16])
            .width(Length::Fill)
            .style(if is_selected {
                picker_row_selected
            } else {
                theme::card::simple
            }),
    )
    .on_press(on_press)
    .style(|_: &theme::Theme, _| iced::widget::button::Style {
        background: Some(Background::Color(color::TRANSPARENT)),
        border: iced::Border {
            radius: 12.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .width(Length::Fill)
    .into()
}

/// Selected-row style — orange border + subtle tint.
fn picker_row_selected(theme: &theme::Theme) -> container::Style {
    let bg = match theme.mode {
        coincube_ui::theme::palette::ThemeMode::Dark => iced::color!(0x1a1a10),
        coincube_ui::theme::palette::ThemeMode::Light => iced::color!(0xFFF5E6),
    };
    container::Style {
        background: Some(Background::Color(bg)),
        border: iced::Border {
            color: color::ORANGE,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    }
}

/// The "THEY SEND" picker modal: Bitcoin rails, then (mainnet only) the
/// cross-network deposit assets. Overlaid by the state's `view()`.
///
/// `cross` is the cross-network asset a running SideShift flow is set to. While
/// one is up it — not the Bitcoin rail underneath — is what the panel is
/// actually set to receive, so it carries the checkmark.
pub fn sender_picker_modal<'a>(
    current: SparkReceiveMethod,
    cross: Option<DepositOption>,
    network: Network,
) -> Element<'a, Message> {
    let is_mainnet = supported_on(network);

    let mut list = Column::new()
        .spacing(8)
        .push(text("Bitcoin").size(P2_SIZE).style(theme::text::secondary));

    for method in [
        SparkReceiveMethod::Bolt11,
        SparkReceiveMethod::OnchainBitcoin,
        SparkReceiveMethod::Spark,
    ] {
        let (net_slug, badge) = rail_display(method);
        list = list.push(spark_picker_row(
            asset_network_logo("btc", net_slug, 36.0),
            "Bitcoin",
            badge,
            cross.is_none() && current == method,
            Message::SparkReceive(SparkReceiveMessage::SelectSenderRail(method)),
        ));
    }

    if is_mainnet {
        list = list.push(Space::new().height(Length::Fixed(6.0))).push(
            text("From another network")
                .size(P2_SIZE)
                .style(theme::text::secondary),
        );
        for option in deposit_options() {
            list = list.push(spark_picker_row(
                cross_network_logo(option, 36.0),
                asset_display_name(option.coin),
                option.network.network_name(),
                cross == Some(*option),
                Message::SparkReceive(SparkReceiveMessage::SelectSenderCrossNetwork(option.key())),
            ));
        }
    }

    // Off mainnet there are only the three rails (no scroll needed); on mainnet
    // the cross-network assets push the list past a screenful, so cap + scroll.
    // The right padding is the scrollbar's gutter — without it the scroller
    // draws on top of the rows' right edge.
    let body: Element<'a, Message> = if is_mainnet {
        scrollable(list.padding(iced::Padding {
            right: 10.0,
            ..iced::Padding::ZERO
        }))
        .height(Length::Fixed(420.0))
        .into()
    } else {
        list.into()
    };

    picker_modal_card("THEY SEND", body)
}

fn phase_body<'a>(
    phase: &'a SparkReceivePhase,
    qr_data: Option<&'a qr_code::Data>,
) -> Element<'a, Message> {
    use crate::app::view::SparkReceiveMessage;

    match phase {
        SparkReceivePhase::Idle => Container::new(
            Column::new().spacing(10).push(
                button::primary(None, "Generate")
                    .on_press(Message::SparkReceive(
                        SparkReceiveMessage::GenerateRequested,
                    ))
                    .width(Length::Fixed(160.0)),
            ),
        )
        .padding(16)
        .style(theme::card::simple)
        .into(),

        SparkReceivePhase::Generating => Container::new(Column::new().spacing(10).push(
            p1_regular("Generating… asking the Spark bridge for a payment request."),
        ))
        .padding(16)
        .style(theme::card::simple)
        .into(),

        SparkReceivePhase::Generated(ok) => {
            // QR code block — rendered only if we successfully
            // encoded the payment request in state. Falls back to a
            // plain "QR unavailable" note if encoding failed; for
            // typical BOLT11 / BTC address payloads this branch
            // should never fire in practice.
            let qr_block: Element<'_, Message> = if let Some(qr) = qr_data {
                Container::new(QRCode::<coincube_ui::theme::Theme>::new(qr).cell_size(6))
                    .align_x(iced::alignment::Horizontal::Center)
                    .width(Length::Fill)
                    .into()
            } else {
                p2_regular("QR code unavailable for this payload.").into()
            };

            let payment_request = ok.payment_request.clone();
            Container::new(
                Column::new()
                    .spacing(14)
                    .align_x(Alignment::Center)
                    .push(h4_bold("Payment request"))
                    .push(qr_block)
                    .push(Space::new().height(Length::Fixed(6.0)))
                    // Click-to-select text_input as a manual-copy
                    // fallback when the button isn't enough.
                    .push(text_input("", &ok.payment_request).padding(10))
                    .push(
                        Row::new()
                            .spacing(10)
                            .push(
                                button::primary(None, "Copy")
                                    .on_press(Message::SparkReceive(
                                        SparkReceiveMessage::CopyPaymentRequest(payment_request),
                                    ))
                                    .width(Length::Fixed(120.0)),
                            )
                            .push(
                                button::transparent_border(None, "Generate another")
                                    .on_press(Message::SparkReceive(SparkReceiveMessage::Reset))
                                    .width(Length::Fixed(180.0)),
                            ),
                    )
                    .push(kv_row("Fee", format!("{} sats", ok.fee_sat))),
            )
            .padding(16)
            .style(theme::card::simple)
            .into()
        }

        SparkReceivePhase::Received { .. } => {
            // Handled by the full-screen celebration in render()
            Container::new(Column::new()).into()
        }

        SparkReceivePhase::Error(err) => Container::new(
            Column::new()
                .spacing(10)
                .push(h4_bold("Error"))
                .push(p1_regular(err.clone()))
                .push(Space::new().height(Length::Fixed(8.0)))
                .push(
                    button::primary(None, "Try again")
                        .on_press(Message::SparkReceive(SparkReceiveMessage::Reset))
                        .width(Length::Fixed(140.0)),
                ),
        )
        .padding(16)
        .style(theme::card::simple)
        .into(),
    }
}

fn kv_row<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    Row::new()
        .spacing(20)
        .push(
            Column::new()
                .width(Length::FillPortion(1))
                .push(h4_bold(label)),
        )
        .push(
            Column::new()
                .width(Length::FillPortion(3))
                .push(p1_regular(value)),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_error_leaves_short_and_boundary_length_errors_unchanged() {
        let short = "bridge unavailable";
        assert_eq!(short_error(short), short);

        let boundary = "x".repeat(60);
        assert_eq!(short_error(&boundary), boundary);
    }

    #[test]
    fn short_error_truncates_long_ascii_errors() {
        let err = format!("{}tail", "x".repeat(60));

        assert_eq!(short_error(&err), format!("{}…", "x".repeat(60)));
    }

    #[test]
    fn short_error_truncates_on_char_boundaries() {
        let err = format!("{}⚡tail", "x".repeat(59));

        assert_eq!(short_error(&err), format!("{}⚡…", "x".repeat(59)));
    }

    #[test]
    fn bitcoin_rail_display_labels_are_stable() {
        assert_eq!(
            rail_display(SparkReceiveMethod::Bolt11),
            ("lightning", "Lightning")
        );
        assert_eq!(
            rail_display(SparkReceiveMethod::OnchainBitcoin),
            ("bitcoin", "On-chain")
        );
        assert_eq!(rail_display(SparkReceiveMethod::Spark), ("spark", "Spark"));
    }

    #[test]
    fn cross_network_asset_display_names_are_stable() {
        assert_eq!(asset_display_name("usdt"), "USDt");
        assert_eq!(asset_display_name("usdc"), "USDC");
        assert_eq!(asset_display_name("eth"), "Ether");
        assert_eq!(asset_display_name("sol"), "Solana");
        assert_eq!(asset_display_name("trx"), "Tron");
        assert_eq!(asset_display_name("doge"), "doge");
    }

    #[test]
    fn selector_card_border_only_highlights_on_hover() {
        let theme = theme::Theme::default();
        let active = card_button_style(&theme, iced::widget::button::Status::Active);
        let hovered = card_button_style(&theme, iced::widget::button::Status::Hovered);

        assert_eq!(active.border.color, color::TRANSPARENT);
        assert_eq!(hovered.border.color, color::ORANGE);
    }

    #[test]
    fn selected_picker_row_uses_orange_border_in_both_themes() {
        for theme in [theme::Theme::dark(), theme::Theme::light()] {
            let style = picker_row_selected(&theme);
            assert_eq!(style.border.color, color::ORANGE);
            assert_eq!(style.border.width, 1.0);
        }
    }
}
