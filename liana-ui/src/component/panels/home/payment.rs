use iced::{
    widget::{column, row, Space},
    Alignment,
};
use liana::transaction::PaymentKind;
use liana_i18n::t;

use crate::{
    component::{
        self,
        amount::{amount_with_fiat_tooltip, AmountSize, FiatAmount},
        pill,
        text::{
            format_date, label_truncated,
            new::{self, caption},
        },
        tooltip::tooltip_with_style,
    },
    icon,
    theme::{self, amount},
    widget::{Container, Element, SpaceExt},
};

const ICON_SIZE: u32 = 16;
const MAX_LABEL_LENGTH: usize = 30;

pub fn kind_icon<'a, M: 'a>(kind: PaymentKind) -> Element<'a, M> {
    match kind {
        PaymentKind::Outgoing => minus(),
        PaymentKind::Incoming => plus(),
        PaymentKind::SendToSelf => refresh(),
    }
}

fn plus<'a, M: 'a>() -> Element<'a, M> {
    Container::new(icon::plus_icon().style(amount::receive).size(ICON_SIZE))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

fn minus<'a, M: 'a>() -> Element<'a, M> {
    Container::new(icon::minus_icon().style(amount::spend).size(ICON_SIZE))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

fn refresh<'a, M: 'a>() -> Element<'a, M> {
    Container::new(icon::reload_icon().style(amount::refresh).size(ICON_SIZE))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiatSource {
    User,
    Wizardsardine,
    Timestamp,
}

impl FiatSource {
    pub fn infotip<'a, M: 'a>(&self) -> Element<'a, M> {
        let txt = match self {
            FiatSource::User => t!("fiat-source-user"),
            FiatSource::Wizardsardine => t!("fiat-source-wizardsardine"),
            FiatSource::Timestamp => t!("fiat-source-timestamp"),
        };
        tooltip_with_style(txt, |t| theme::amount::zeroes(t, false)).into()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FiatPrice {
    pub amount: FiatAmount,
    pub source: FiatSource,
}

/// Payment or transaction list entry.
#[allow(clippy::too_many_arguments)]
pub fn list_entry<'a, M: 'a + Clone>(
    label: Option<String>,
    time: Option<chrono::DateTime<chrono::Utc>>,
    kind: PaymentKind,
    is_batch: bool,
    is_payjoin: bool,
    amount: bitcoin::Amount,
    fiat_price: Option<FiatPrice>,
    msg: Option<M>,
) -> Element<'a, M> {
    let label = label.unwrap_or_else(|| t!("common-no-label-parenthesized"));
    let label = label_truncated(&label, MAX_LABEL_LENGTH);

    let time: Element<'a, M> = if let Some(time) = time {
        caption(format_date(time))
            .style(theme::text::card_secondary)
            .into()
    } else {
        pill::unconfirmed_compact().into()
    };

    let amount: Element<'a, M> = if kind == PaymentKind::SendToSelf {
        new::h2(t!("common-self-transfer"))
            .style(theme::text::primary)
            .into()
    } else {
        let to_fiat = fiat_price.map(|fp| move |_: bitcoin::Amount| fp.amount);
        let approximate = fiat_price.is_none_or(|fp| fp.source == FiatSource::Timestamp);
        let tooltip = fiat_price.map(|fp| fp.source.infotip());
        amount_with_fiat_tooltip(&amount, to_fiat, AmountSize::M, approximate, tooltip)
    };
    let batch = is_batch.then_some(pill::batch());
    let payjoin = is_payjoin.then_some(pill::payjoin());

    let left = column![label, time].spacing(2);
    let right = row![payjoin, batch, kind_icon(kind), amount]
        .spacing(5)
        .align_y(Alignment::Center);
    let content =
        row![left, Space::fill_width(), right].height(component::panels::ListEntryHeight::Standard);
    component::card::list_entry_with_padding(content, msg, component::panels::LIST_ENTRY_PADDING)
}
