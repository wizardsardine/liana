pub mod coins;
pub mod home;
pub mod map;
pub mod psbts;
pub mod receive;
pub mod recovery;
pub mod setting;
pub mod spend;
pub mod transactions;

use bitcoin::Amount;
use iced::{widget::row, Alignment, Length};
use liana_i18n::t;

use crate::{
    component::{
        amount::{amount_with_fiat_tooltip, AmountSize, FiatAmount},
        button,
        card::{self, CardPadding},
        text::new,
    },
    spacing::HSpacing,
    theme,
    widget::Element,
};

#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum ListEntryHeight {
    Standard = 90,
}

impl From<ListEntryHeight> for Length {
    fn from(value: ListEntryHeight) -> Self {
        (value as u32).into()
    }
}

pub const LIST_ENTRY_PADDING: [u16; 2] = [5 /* Top/Bottom */, 10 /* Left/Right */];

pub const FOLDABLE_ENTRY_PADDING: [u16; 2] = [5 /* Top/Bottom */, 20 /* Left/Right */];

pub fn fees_row<'a, M: 'a, F: Fn(Amount) -> FiatAmount>(
    fee: Option<Amount>,
    feerate: Option<String>,
    to_fiat: Option<F>,
) -> Option<Element<'a, M>> {
    let fee = amount_with_fiat_tooltip(&fee?, to_fiat, AmountSize::M, true, None);
    let feerate = feerate.map(|feerate| new::b2(feerate).style(theme::text::secondary));
    let row = row![
        new::b2_medium(t!("transactions-miner-fee")).style(theme::text::primary),
        fee,
        feerate
    ]
    .spacing(HSpacing::S)
    .align_y(Alignment::Center);
    let card = card::section(row)
        .padding(CardPadding::Soft)
        .width(Length::Fill);
    Some(card.into())
}

pub fn txid_row<'a, M: Clone + 'a>(txid: String, copy_txid: M) -> Element<'a, M> {
    row![
        new::b2_medium(t!("transactions-txid")).style(theme::text::primary),
        new::b2(txid).style(theme::text::secondary),
        button::btn_copy(Some(copy_txid))
    ]
    .spacing(HSpacing::S)
    .align_y(Alignment::Center)
    .into()
}
