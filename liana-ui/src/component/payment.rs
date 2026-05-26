use iced::{
    alignment::Horizontal,
    widget::{column, row, tooltip::Position, Space},
    Alignment, Length,
};

use crate::{
    component::{
        self, pill,
        text::{
            new::{caption, h2, h3, H2_SEMI_SPEC},
            text, truncate,
        },
        tooltip::tooltip_with_style,
        tooltip_custom,
    },
    icon,
    theme::{self, amount},
    widget::{Button, Container, Element, SpaceExt},
};

const ICON_SIZE: u32 = 16;
const PAYMENT_HEIGHT: u32 = 90;
const MAX_LABEL_LENGTH: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaymentKind {
    Outgoing,
    Incoming,
    /// A payment to self, which could be either from a self-transfer
    /// or a change output from an outgoing transaction.
    SendToSelf,
}

impl PaymentKind {
    pub fn icon<'a, M: 'a>(&self) -> Element<'a, M> {
        match self {
            PaymentKind::Outgoing => minus(),
            PaymentKind::Incoming => plus(),
            PaymentKind::SendToSelf => refresh(),
        }
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
            FiatSource::User => "Price you have filled yourself",
            FiatSource::Wizardsardine => {
                "Price automaticaly processed by WS when you crafted the transaction"
            }
            FiatSource::Timestamp => "Default price at the time the transaction has been confirmed",
        };
        tooltip_with_style(txt, |t| theme::amount::zeroes(t, false)).into()
    }
}

#[derive(Debug, Clone)]
pub struct FiatPrice {
    pub amount: String,
    pub currency: String,
    pub source: FiatSource,
}

#[derive(Debug, Clone)]
pub struct UIPayment<'a> {
    pub label: Option<&'a str>,
    pub kind: PaymentKind,
    pub time: Option<chrono::DateTime<chrono::Utc>>,
    pub amount: bitcoin::Amount,
    pub fiat_price: Option<FiatPrice>,
}

/// Format a date as "Mar 12, 2026".
pub fn format_date(time: chrono::DateTime<chrono::Utc>) -> String {
    time.format("%b %-d, %Y").to_string()
}

/// "See more" button paginating the payment history. Shows "Fetching ..." and
/// is disabled while `processing`.
pub fn see_more<'a, M: Clone + 'a>(processing: bool, next: M) -> Element<'a, M> {
    Container::new(
        Button::new(
            text(if processing {
                "Fetching ..."
            } else {
                "See more"
            })
            .width(Length::Fill)
            .align_x(Horizontal::Center),
        )
        .width(Length::Fill)
        .padding(15)
        .style(theme::button::transparent_border)
        .on_press_maybe((!processing).then_some(next)),
    )
    .width(Length::Fill)
    .style(theme::card::simple)
    .into()
}

pub fn payment_card<'a, M: 'a + Clone>(payment: UIPayment<'a>, msg: Option<M>) -> Element<'a, M> {
    let UIPayment {
        label,
        kind,
        time,
        amount,
        fiat_price,
    } = payment;
    let label: Element<'a, M> = match label {
        None => h2("(No label)").style(theme::text::primary).into(),
        Some(label) if label.chars().count() > MAX_LABEL_LENGTH => {
            let short = truncate(label, MAX_LABEL_LENGTH);
            let short = h2(short).style(theme::text::primary);
            tooltip_custom(label, short, Position::Top).into()
        }
        Some(label) => h2(label).style(theme::text::primary).into(),
    };

    let time: Element<'a, M> = if let Some(time) = time {
        caption(format_date(time))
            .style(theme::text::card_secondary)
            .into()
    } else {
        pill::unconfirmed_compact().into()
    };

    let icon = kind.icon();
    let btc_amount =
        component::amount::amount_with_font(&amount, H2_SEMI_SPEC).align_y(Alignment::Center);
    let btc_row = row![icon, btc_amount].spacing(5).align_y(Alignment::Center);
    let fiat_price = fiat_price.map(
        |FiatPrice {
             amount,
             currency,
             source,
         }| {
            let tilde = if source == FiatSource::Timestamp {
                "~"
            } else {
                ""
            };
            let fp_string = format!("{tilde}{amount} {currency}");
            let fp = h3(fp_string).style(amount::fiat_price);
            let infotip = source.infotip();
            row![fp, infotip].spacing(5).align_y(Alignment::Center)
        },
    );

    let left = column![label, time].spacing(2);
    let right = column![btc_row, fiat_price].align_x(Alignment::End);
    let content = row![left, Space::fill_width(), right].height(PAYMENT_HEIGHT);
    component::card::clickable_card_with_padding(content, msg, [5, 10])
}
