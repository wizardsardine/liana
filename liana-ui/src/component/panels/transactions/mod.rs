use bitcoin::Amount;
use iced::{
    widget::{column, row},
    Alignment, Length,
};
use liana_i18n::t;

use crate::{
    component::{amount, badge, list, pill, text::legacy},
    theme,
    widget::Element,
};

#[allow(clippy::too_many_arguments)]
pub fn list_entry<'a, M: Clone + 'static>(
    label: Option<&'a str>,
    date: Option<String>,
    is_external: bool,
    is_send_to_self: bool,
    is_batch: bool,
    amount: Amount,
    msg: M,
) -> Element<'a, M> {
    let is_unconfirmed = date.is_none();
    let label = label.map(legacy::p1_regular);
    let date = date.map(|date| legacy::text(date).style(theme::text::secondary));
    let badge = if is_external {
        badge::receive()
    } else if is_send_to_self {
        badge::cycle()
    } else {
        badge::spend()
    };
    let unconfirmed = is_unconfirmed.then_some(pill::unconfirmed());
    let batch = is_batch.then_some(pill::batch());
    let description = row![badge, column![label, date]]
        .spacing(10)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let amount_row = if is_send_to_self {
        row![legacy::text(t!("common-self-transfer"))]
    } else {
        let sign = if is_external { "+" } else { "-" };
        row![legacy::text(sign), amount::amount(&amount)]
            .spacing(5)
            .align_y(Alignment::Center)
    };

    let content = row![description, unconfirmed, batch, amount_row]
        .align_y(Alignment::Center)
        .spacing(20);

    list::entry_history(content, msg)
}
