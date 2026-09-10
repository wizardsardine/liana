use bitcoin::Amount;
use iced::{
    widget::{column, row, text::Style, Space},
    Alignment, Length,
};
use liana::spend::SpendStatus;
use liana_i18n::t;

use crate::{
    component::{
        address::address as address_view,
        amount::{amount, amount_with_fiat_tooltip, AmountSize},
        button, card,
        panels::{
            home::payment::{FiatPrice, FiatSource, PaymentKind},
            LIST_ENTRY_PADDING,
        },
        pill,
        text::{new, truncate},
    },
    spacing::HSpacing,
    theme::{self, Theme},
    widget::{Container, Element, SpaceExt, Toggler},
};

const PSBT_HEIGHT: u32 = 90;

const STATUS_PILL_WIDTH_SMALL: f32 = 100.0;
const STATUS_PILL_WIDTH: f32 = 120.0;
const STATUS_PILL_WIDTH_LARGE: f32 = 150.0;

/// How far along the signing of a PSBT is on the path it spends through.
#[derive(Debug, Clone, Copy)]
pub struct PsbtSigs {
    pub count: usize,
    pub threshold: usize,
}

/// The pill telling where a saved PSBT is at in its lifecycle.
pub fn status_pill<'a, M: 'a>(status: SpendStatus, signed: bool) -> Option<Container<'a, M>> {
    match status {
        SpendStatus::Broadcastable => {
            signed.then_some(pill::signed().width(STATUS_PILL_WIDTH_LARGE))
        }
        SpendStatus::Broadcast => Some(pill::unconfirmed().width(STATUS_PILL_WIDTH)),
        SpendStatus::Spent => Some(pill::spent().width(STATUS_PILL_WIDTH)),
        SpendStatus::Deprecated => Some(pill::deprecated().width(STATUS_PILL_WIDTH)),
    }
}

/// Row toggling whether the confirmed psbts are listed.
pub fn hide_confirmed_row<'a, M: Clone + 'static>(hidden: bool, toggle: M) -> Element<'a, M> {
    let label = new::b4_medium(t!("psbts-hide-confirmed"));
    let toggler = Toggler::new(hidden)
        .on_toggle(move |_| toggle.clone())
        .size(28)
        .style(theme::toggler::primary);

    row![label, toggler]
        .spacing(HSpacing::M)
        .align_y(Alignment::Center)
        .into()
}

#[allow(clippy::too_many_arguments)]
pub fn list_entry<'a, M: Clone + 'static>(
    label: Option<&'a str>,
    is_send_to_self: bool,
    is_batch: bool,
    is_recovery: bool,
    status: SpendStatus,
    sigs: PsbtSigs,
    amount: Amount,
    fiat_price: Option<FiatPrice>,
    available_width: f32,
    msg: Option<M>,
) -> Element<'a, M> {
    let PsbtSigs { count, threshold } = sigs;
    let signed = count >= threshold;

    let thresh_descr = if available_width >= 1460.0 {
        t!("psbts-signatures-collected")
    } else {
        String::new()
    };
    let sig_style: fn(&Theme) -> Style = if !signed {
        theme::text::warning
    } else {
        theme::text::success
    };
    let counter = format!("{}/{threshold}", count.min(threshold));
    let sigs = new::b4_medium(format!("{counter} {thresh_descr}")).style(sig_style);

    let recovery_pill = is_recovery.then_some(pill::recovery().width(STATUS_PILL_WIDTH_SMALL));
    let batch_pill = is_batch.then_some(pill::batch().width(STATUS_PILL_WIDTH_SMALL));

    let status_pill = status_pill(status, signed);

    let max_lbl_chars = (available_width - 500.0) as usize / 22;
    let mut label = label.map(|l| truncate(l, max_lbl_chars));

    let kind = if is_send_to_self {
        label = Some(t!("common-self-transfer"));
        PaymentKind::SendToSelf
    } else {
        PaymentKind::Outgoing
    };

    let label = label.map(|l| new::h2(l).style(theme::text::primary));

    let sigs = row![
        sigs,
        status_pill,
        Space::fill_width(),
        recovery_pill,
        batch_pill,
    ]
    .align_y(Alignment::Center)
    .spacing(22);
    let left = column![label, sigs].spacing(12);

    let to_fiat = fiat_price.map(|fp| move |_: Amount| fp.amount);
    let approximate = fiat_price.is_none_or(|fp| fp.source == FiatSource::Timestamp);
    let tooltip = fiat_price.map(|fp| fp.source.infotip());
    let amount = amount_with_fiat_tooltip(&amount, to_fiat, AmountSize::M, approximate, tooltip);
    let spent = row![kind.icon(), amount]
        .spacing(HSpacing::S)
        .align_y(Alignment::Center);

    let content = row![left, spent].spacing(HSpacing::L).height(PSBT_HEIGHT);

    card::list_entry_with_padding(content, msg, LIST_ENTRY_PADDING)
}

/// Row of a change output: what it holds and where it goes.
pub fn change_row<'a, M: Clone + 'static>(
    value: Amount,
    address: String,
    copy: M,
) -> Element<'a, M> {
    let value = row![Space::fill_width(), amount(&value)];

    let label = new::b5_bold(t!("common-address-label")).style(theme::text::secondary);
    let copy = button::btn_copy(Some(copy));
    let address = row![label, address_view(address), copy]
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .spacing(5);

    column![value, address]
        .width(Length::Fill)
        .spacing(5)
        .into()
}
