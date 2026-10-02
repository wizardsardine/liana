pub mod home;
pub mod psbts;
pub mod receive;
pub mod setting;
pub mod spend;
pub mod transactions;

use bitcoin::Amount;
use iced::{
    widget::{row, Space},
    Alignment, Length,
};
use liana_i18n::t;

use crate::{
    component::{amount::amount_with_font, button, text::new},
    spacing::HSpacing,
    theme,
    widget::{Element, SpaceExt},
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

/// Miner fee row, a `None` fee shows the missing inputs caption instead.
pub fn fees_row<'a, M: 'a>(fee: Option<Amount>, feerate: Option<String>) -> Element<'a, M> {
    let missing_inputs = fee
        .is_none()
        .then_some(new::caption(t!("psbt-missing-inputs")));
    let fee = fee.map(|fee| amount_with_font(&fee, new::H1_SPEC));
    let feerate = feerate.map(|feerate| new::h3(feerate).style(theme::text::secondary));
    row![
        new::h1(t!("transactions-miner-fee")).style(theme::text::secondary),
        missing_inputs,
        fee,
        feerate
    ]
    .spacing(HSpacing::S)
    .align_y(Alignment::Center)
    .into()
}

pub fn txid_row<'a, M: Clone + 'a>(txid: String, copy_txid: M) -> Element<'a, M> {
    row![
        new::b5_bold(t!("transactions-txid")),
        Space::fill_width(),
        new::small_caption(txid).style(theme::text::secondary),
        button::btn_copy(Some(copy_txid))
    ]
    .align_y(Alignment::Center)
    .into()
}
