use bitcoin::Amount;
use iced::{
    widget::{column, row},
    Alignment, Length,
};
use liana::spend::SpendStatus;
use liana_i18n::t;

use crate::{
    component::{
        amount::{amount, amount_with_font},
        badge, pill,
        text::new,
    },
    icon, theme,
    widget::{Button, Container, Element},
};

const STATUS_PILL_WIDTH: f32 = 120.0;

/// How far along the signing of a PSBT is: either it spends through a recovery
/// path, or it has some of the signatures its primary path requires.
#[derive(Debug, Clone, Copy)]
pub enum PsbtSigs {
    Recovery,
    Primary { count: usize, threshold: usize },
}

/// The pill telling where a saved PSBT is at in its lifecycle.
pub fn status_pill<'a, M: 'a>(status: SpendStatus) -> Container<'a, M> {
    match status {
        SpendStatus::Broadcastable => pill::signed().width(STATUS_PILL_WIDTH),
        SpendStatus::Broadcast => pill::unconfirmed().width(STATUS_PILL_WIDTH),
        SpendStatus::Spent => pill::spent().width(STATUS_PILL_WIDTH),
        SpendStatus::Deprecated => pill::deprecated().width(STATUS_PILL_WIDTH),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn list_entry<'a, M: Clone + 'static>(
    label: Option<&'a str>,
    is_send_to_self: bool,
    is_batch: bool,
    status: SpendStatus,
    sigs: PsbtSigs,
    spend_amount: Amount,
    fee_amount: Option<Amount>,
    msg: Option<M>,
) -> Element<'a, M> {
    let badge = if is_send_to_self {
        badge::cycle()
    } else {
        badge::spend()
    };

    let sigs = match sigs {
        PsbtSigs::Recovery => pill::recovery(),
        PsbtSigs::Primary { count, threshold } => {
            let counter = new::caption(format!("{}/{threshold}", count.min(threshold)))
                .style(theme::text::secondary);
            let key = icon::key_icon().style(theme::text::secondary);
            Container::new(row![counter, key].spacing(5).align_y(Alignment::Center))
        }
    };

    let label = label.map(new::b5_medium);

    let left = row![badge, sigs, label]
        .spacing(10)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    let batch = is_batch.then_some(pill::batch());

    let status = status_pill(status);

    let spent = if is_send_to_self {
        Container::new(new::b5_medium(t!("common-self-transfer")))
    } else {
        Container::new(amount(&spend_amount))
    };
    let fee = fee_amount.map(|fee| amount_with_font(&fee, new::CAPTION_SPEC));
    let amounts = column![spent, fee].align_x(Alignment::End).width(140);

    let content = row![left, batch, status, amounts]
        .align_y(Alignment::Center)
        .spacing(20);

    let entry = Button::new(content)
        .padding(10)
        .on_press_maybe(msg)
        .style(theme::button::transparent_border);

    Container::new(entry)
        .style(theme::card::button_simple)
        .into()
}
