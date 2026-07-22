use iced::{
    alignment::Horizontal,
    widget::{column, row, text::Style, Space},
    Alignment, Length,
};
use std::fmt::Display;

use bitcoin::{Amount, Denomination};

use crate::{
    color,
    component::{
        amount::{self, amount_with_fiat, AmountSize, Currency, DisplayAmount, FiatAmount},
        button, card,
        checkbox::{labelled_checkbox, labelled_radio},
        form,
        label::LABEL_LENGTH_WARNING,
        pill, scrollable, section,
        text::{caption, new, P1_SIZE},
        tooltip,
    },
    icon,
    theme::{self, Theme},
    widget::{Column, Container, Element, SpaceExt, Stack},
};

const COIN_LIST_MAX_HEIGHT: f32 = 300.0;
const FEERATE_INPUT_WIDTH: f32 = 150.0;

pub enum CoinLabel {
    /// Label set on this coin.
    Outpoint(String),
    /// Label inherited from the parent transaction.
    Transaction(String),
    None,
}

pub enum CoinStatus {
    Spent,
    Unconfirmed,
    Sequence(u32),
}

/// The optional fiat column shown beside a recipient's BTC amount. `to_fiat`
/// converts a BTC amount to fiat, `summary` is the price-source tooltip content
/// (built by the caller), and `on_edit` maps an edited fiat string to a message.
pub struct RecipientFiat<'a, M> {
    pub currency: Currency,
    pub to_fiat: Box<dyn Fn(Amount) -> FiatAmount + 'a>,
    pub form_value: Option<&'a form::Value<String>>,
    pub summary: Element<'a, M>,
    pub on_edit: Box<dyn Fn(String) -> M + 'static>,
}

#[allow(clippy::too_many_arguments)]
pub fn recipient_card<'a, M: Clone + 'static>(
    address: &'a form::Value<String>,
    label: &'a form::Value<String>,
    amount: &'a form::Value<String>,
    fiat: Option<RecipientFiat<'a, M>>,
    is_max_selected: bool,
    dust_warning: Option<&'a str>,
    max_estimated_amount: Option<Amount>,
    on_address_edit: impl Fn(String) -> M + 'static,
    on_label_edit: impl Fn(String) -> M + 'static,
    on_amount_edit: impl Fn(String) -> M + 'static + Clone,
    on_max: Option<M>,
    on_delete: Option<M>,
) -> Element<'a, M> {
    let btc_amt = if dust_warning.is_some() {
        max_estimated_amount
    } else {
        Amount::from_str_in(&amount.value, Denomination::Bitcoin).ok()
    };

    let address_form: Element<'a, M> = form::Form::new("Address", address, on_address_edit)
        .label("Address")
        .warning("Invalid address (maybe it is for another network?)")
        .size(P1_SIZE)
        .padding(10)
        .into();

    let description_form: Element<'a, M> = form::Form::new("Payment label", label, on_label_edit)
        .label("Description")
        .warning(LABEL_LENGTH_WARNING)
        .size(P1_SIZE)
        .padding(10)
        .into();

    let btc_input: Element<'a, M> = if is_max_selected {
        let amount_txt = btc_amt
            .map(|a| a.to_formatted_string())
            .unwrap_or_else(|| amount.value.clone());
        let value = Container::new(
            new::caption(amount_txt)
                .size(P1_SIZE)
                .style(theme::text::secondary),
        )
        .width(Length::Fill);
        column![new::b3("Amount (BTC)"), value]
            .spacing(5)
            .width(Length::Fill)
            .into()
    } else {
        form::Form::new_amount_btc("0.001 (in BTC)", amount, on_amount_edit)
            .label("Amount (BTC)")
            .warning("Invalid amount. (Note amounts lower than 0.000005 BTC are invalid.)")
            .size(P1_SIZE)
            .padding(10)
            .into()
    };

    let fiat_price = fiat.map(|fiat| {
        if is_max_selected {
            let RecipientFiat {
                currency,
                to_fiat,
                summary,
                ..
            } = fiat;
            let visible = btc_amt.is_some();
            let currency_label = visible.then_some(new::h3(format!("~{currency}")));
            let value = btc_amt.map(|a| {
                let a = to_fiat(a).to_formatted_string();
                Container::new(new::caption(a).size(P1_SIZE).style(theme::text::secondary))
                    .width(Length::Fill)
            });
            let summary = visible.then_some(iced::widget::tooltip::Tooltip::new(
                icon::tooltip_icon(),
                summary,
                iced::widget::tooltip::Position::Bottom,
            ));
            row![
                Space::with_width(20),
                currency_label,
                Space::with_width(5),
                value,
                summary,
                Space::with_width(10),
            ]
            .align_y(Alignment::Center)
            .spacing(5)
        } else {
            let RecipientFiat {
                currency,
                to_fiat,
                form_value,
                summary,
                on_edit,
            } = fiat;
            let label = row![
                new::h3(format!("~{currency}")),
                iced::widget::tooltip::Tooltip::new(
                    icon::tooltip_icon(),
                    summary,
                    iced::widget::tooltip::Position::Bottom,
                ),
            ]
            .align_y(Alignment::Center)
            .spacing(5);
            let fiat_form = if let Some(val) = form_value {
                val
            } else if let Some(btc_amt) = btc_amt {
                let fa = to_fiat(btc_amt);
                &form::Value {
                    value: fa.to_rounded_string(), // required decimal places for currency
                    warning: None,
                    valid: true,
                }
            } else {
                &form::Value::default()
            };
            let input = form::Form::new(format!("Enter amount in {currency}"), fiat_form, on_edit)
                .component_label(label)
                .size(P1_SIZE)
                .padding(10)
                .into_container();
            row![Space::with_width(20), input, Space::with_width(10)]
                .align_y(Alignment::Center)
                .spacing(5)
        }
    });

    // The MAX option cannot be edited for recovery recipients (on_max is None).
    let max = on_max.map(|msg| {
        iced::widget::tooltip::Tooltip::new(
            labelled_checkbox(new::caption("MAX"), is_max_selected, move |_| msg.clone()),
            // Add spaces at end so that text is padded at screen edge.
            "Total amount remaining after paying fee and any other recipients     ",
            iced::widget::tooltip::Position::Bottom,
        )
    });

    let amount_row = row![btc_input, fiat_price, max]
        .align_y(Alignment::End)
        .spacing(10)
        .width(Length::Fill);
    // Show dust warning, if any, or otherwise any amount warning.
    let warning = dust_warning
        .map(|w| new::caption(w).color(color::RED))
        .or_else(|| {
            amount
                .warning
                .as_ref()
                .map(|w| new::caption(w).color(color::ORANGE))
        });
    let amount_warning_row = row![Space::with_width(20), warning];
    let amount_section: Element<'a, M> = column![amount_row, amount_warning_row].into();

    let body = column![address_form, description_form, amount_section].spacing(10);
    let card = card::flat(body, [28, 42]);

    match on_delete {
        // Float the remove button over the top-right corner of the card.
        Some(msg) => {
            let remove = Container::new(button::btn_remove(Some(msg)))
                .padding(5)
                .width(Length::Fill)
                .align_x(Horizontal::Right);
            Stack::new().push(card).push(remove).into()
        }
        None => card.into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeLevel {
    Low,
    Medium,
    High,
}

pub enum SmartFee<M> {
    Manual {
        on_smart: M,
    },
    Smart {
        level: FeeLevel,
        on_manual: M,
        on_low: M,
        on_medium: Option<M>,
        on_high: M,
    },
}

pub fn fee_rate_row<'a, M: Clone + 'static, F: Fn(Amount) -> FiatAmount>(
    smart_fee: Option<SmartFee<M>>,
    feerate: &form::Value<String>,
    on_edit: impl Fn(String) -> M + 'static,
    fee: Option<&Amount>,
    to_fiat: Option<F>,
    available_width: f32,
    max_feerate: u64,
) -> Element<'a, M> {
    let h_spacer = 14;

    let label = new::b3("Feerate:");

    let input: Element<'a, M> = Container::new(
        form::Form::new_trimmed("e.g. 5 (in sats/vb)", feerate, on_edit)
            .compact()
            .fee(),
    )
    .width(FEERATE_INPUT_WIDTH)
    .into();

    let mode_selector = |manual_active: bool, switch: M| -> Element<'a, M> {
        let switch_smart = switch.clone();
        row![
            labelled_radio("Manual", manual_active, switch),
            Space::with_width(h_spacer),
            labelled_radio("Smart Select", !manual_active, switch_smart),
            tooltip("Pick a preset feerate instead of entering one manually."),
        ]
        .align_y(Alignment::Center)
        .into()
    };

    let level_buttons =
        |level: FeeLevel, on_low: M, on_medium: Option<M>, on_high: M| -> Element<'a, M> {
            let low = button::btn_low(level == FeeLevel::Low, Some(on_low));
            let high = button::btn_high(level == FeeLevel::High, Some(on_high));
            let mut buttons = row![low].spacing(5).align_y(Alignment::Center);
            if let Some(on_medium) = on_medium {
                buttons = buttons.push(button::btn_medium(
                    level == FeeLevel::Medium,
                    Some(on_medium),
                ));
            }
            buttons.push(high).into()
        };

    let is_smart = matches!(smart_fee, Some(SmartFee::Smart { .. }));
    let (selector, entry): (Option<Element<'a, M>>, Element<'a, M>) = match smart_fee {
        None => (None, input),
        Some(SmartFee::Manual { on_smart }) => (Some(mode_selector(true, on_smart)), input),
        Some(SmartFee::Smart {
            level,
            on_manual,
            on_low,
            on_medium,
            on_high,
        }) => (
            Some(mode_selector(false, on_manual)),
            level_buttons(level, on_low, on_medium, on_high),
        ),
    };

    let fee = fee.map(move |fee| {
        let label = if is_smart {
            format!("Fee ({} sats/vb):", feerate.value)
        } else {
            "Fee:".to_string()
        };
        let fee_label = new::caption(label).style(theme::text::secondary);
        let fee_amount = amount_with_fiat(fee, to_fiat, AmountSize::S);
        row![fee_label, fee_amount]
            .spacing(h_spacer)
            .align_y(Alignment::Center)
    });

    let split = available_width < 1750.0;
    let rows = if split && is_smart {
        column![
            row![label, selector]
                .spacing(h_spacer)
                .align_y(Alignment::Center),
            Space::with_width(10),
            row![entry, fee]
                .spacing(h_spacer)
                .align_y(Alignment::Center)
                .wrap(),
        ]
    } else {
        column![row![label, selector, entry, fee]
            .height(40.0)
            .align_y(Alignment::Center)
            .spacing(h_spacer)]
    };

    let warn_offset = if is_smart { 355 } else { 95 };
    let content = if !is_smart && !feerate.valid {
        rows.push(row![
            Space::with_width(warn_offset),
            caption(format!(
                "Feerate must be an integer less than or equal to {max_feerate} sats/vbyte"
            ))
            .color(color::RED)
        ])
    } else {
        rows
    };
    card::flat(content, [12, 42]).width(Length::Fill).into()
}

pub fn coin_selection<'a, M: 'a>(rows: Vec<Element<'a, M>>) -> Element<'a, M> {
    let header = section("Coins selection");

    let coin_cards: Vec<Element<'a, M>> = rows
        .into_iter()
        .map(|r| card::flat(r, [12, 42]).width(Length::Fill).into())
        .collect();
    let list = Container::new(
        scrollable::vertical(Column::with_children(coin_cards).spacing(10)).spacing(5),
    )
    .max_height(COIN_LIST_MAX_HEIGHT)
    .width(Length::Fill);

    column![header, list].spacing(10).into()
}

const COMPACT_PILL_WIDTH: f32 = 1400.0;
const DASHBOARD_PADDING: f32 = 380.0;
const MIN_LABEL_LEN: usize = 20;
const MAX_LABEL_LEN: usize = 75;
const LABEL_SCALE_MIN_WIDTH: f32 = 1000.0;
const LABEL_SCALE_MAX_WIDTH: f32 = 1600.0;

fn label_len(available_width: f32) -> usize {
    let width = available_width - DASHBOARD_PADDING;
    let t = ((width - LABEL_SCALE_MIN_WIDTH) / (LABEL_SCALE_MAX_WIDTH - LABEL_SCALE_MIN_WIDTH))
        .clamp(0.0, 1.0);
    MIN_LABEL_LEN + (t * (MAX_LABEL_LEN - MIN_LABEL_LEN) as f32).round() as usize
}

pub fn coin_row<'a, M: Clone + 'static>(
    label: CoinLabel,
    amount: &Amount,
    status: CoinStatus,
    selected: bool,
    toggle: M,
    available_width: f32,
) -> Element<'a, M> {
    fn font<'a>(txt: impl Display) -> iced::widget::Text<'a, Theme> {
        new::b3_medium(txt)
    }
    fn label_style(theme: &Theme) -> Style {
        theme::amount::zeroes(theme, false)
    }
    let max_len = label_len(available_width);
    let short = |s: String| -> String {
        if s.chars().count() > max_len {
            format!("{}…", s.chars().take(max_len).collect::<String>())
        } else {
            s
        }
    };
    let coin_label: Element<M> = match label {
        CoinLabel::Outpoint(label) => font(short(label)).style(label_style).into(),
        CoinLabel::Transaction(label) => {
            let from = font("From").style(theme::text::secondary);
            row![from, font(short(label)).style(label_style)]
                .spacing(5)
                .into()
        }
        CoinLabel::None => font("").style(label_style).into(),
    };

    let timelock_pill: Container<M> = match status {
        CoinStatus::Spent => pill::spent(),
        CoinStatus::Unconfirmed => pill::unconfirmed(),
        CoinStatus::Sequence(seq) if available_width < COMPACT_PILL_WIDTH => {
            pill::coin_sequence_compact(seq)
        }
        CoinStatus::Sequence(seq) => pill::coin_sequence(seq),
    };

    let select = labelled_checkbox(coin_label, selected, move |_| toggle.clone());

    row![
        select,
        Space::fill_width(),
        timelock_pill,
        amount::amount_with_font(amount, new::B3_MEDIUM_SPEC)
    ]
    .spacing(14)
    .align_y(Alignment::Center)
    .into()
}
