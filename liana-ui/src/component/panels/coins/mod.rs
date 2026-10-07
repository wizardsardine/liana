use bitcoin::{Address, Amount, OutPoint, Txid};
use iced::{
    widget::{column, row, Space},
    Alignment,
};
use liana_i18n::t;

use crate::{
    component::{
        address::address as address_view,
        amount::{amount_with_fiat_tooltip, AmountSize, FiatAmount},
        button, card, panels, pill,
        text::new,
    },
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{Element, Row, SpaceExt},
};

#[derive(Debug, Clone, Copy)]
pub struct CoinSpend {
    pub txid: Txid,
    pub height: Option<i32>,
}

#[allow(clippy::too_many_arguments)]
pub fn coin_entry<'a, M: Clone + 'static>(
    coin_amount: Amount,
    outpoint: OutPoint,
    address: &'a Address,
    address_label: Option<&'a str>,
    block_height: Option<i32>,
    spend: Option<CoinSpend>,
    seq: u32,
    label: Element<'a, M>,
    copy_address: M,
    copy_outpoint: M,
    reset_timelock: M,
    expanded: bool,
    on_toggle: M,
) -> Element<'a, M> {
    let outpoint = outpoint.to_string();
    let address = address.to_string();

    let status = if spend.is_some() {
        pill::spent()
    } else if block_height.is_none() {
        pill::unconfirmed()
    } else {
        pill::coin_sequence(seq)
    };
    let amount = amount_with_fiat_tooltip(
        &coin_amount,
        None::<fn(Amount) -> FiatAmount>,
        AmountSize::M,
        false,
        None,
    );
    let right = row![status, amount]
        .spacing(HSpacing::S)
        .align_y(Alignment::Center);
    let header = row![label, Space::fill_width(), right]
        .align_y(Alignment::Center)
        .spacing(HSpacing::L)
        .height(panels::ListEntryHeight::Standard);

    let recovery = match (spend, block_height) {
        (None, Some(_)) if seq == 0 => {
            Some(new::b2_medium(t!("coins-recovery-active")).style(theme::text::warning))
        }
        (None, Some(_)) => Some(new::b2_medium(t!(
            "coins-first-recovery-in-blocks",
            blocks = seq
        ))),
        _ => None,
    };

    let address_label = info_row(
        t!("coins-address-label"),
        new::b2(
            address_label
                .map(str::to_string)
                .unwrap_or_else(|| t!("common-no-label")),
        )
        .style(theme::text::secondary),
    );
    let copy_address = button::btn_copy(Some(copy_address));
    let address = info_row(
        t!("common-address-label"),
        row![address_view(address), copy_address].align_y(Alignment::Center),
    );
    let copy_outpoint = button::btn_copy(Some(copy_outpoint));
    let outpoint_row = info_row(
        t!("coins-outpoint"),
        row![
            new::b2(outpoint).style(theme::text::secondary),
            copy_outpoint
        ]
        .align_y(Alignment::Center),
    );
    let block_height = block_height.map(|b| {
        info_row(
            t!("coins-block-height"),
            new::b2(b.to_string()).style(theme::text::secondary),
        )
    });
    let coin_info =
        column![address_label, address, outpoint_row, block_height].spacing(VSpacing::XS);

    let spend: Element<'a, M> = match spend {
        Some(CoinSpend { txid, height }) => {
            let spend_txid = info_row(
                t!("coins-spend-txid"),
                new::b2(txid.to_string()).style(theme::text::secondary),
            );
            let spend_height = match height {
                Some(height) => info_row(
                    t!("coins-spend-block-height"),
                    new::b2(height.to_string()).style(theme::text::secondary),
                ),
                None => {
                    row![new::b2_medium(t!("coins-not-in-block"))]
                }
            };
            column![spend_txid, spend_height]
                .spacing(VSpacing::XS)
                .into()
        }
        None => {
            let reset = button::btn_reset_timelock(Some(reset_timelock), seq == 0);
            column![row![Space::fill_width(), reset]].into()
        }
    };

    let details = column![recovery, coin_info, spend]
        .spacing(VSpacing::S)
        .padding(panels::LIST_ENTRY_PADDING);

    card::foldable::FoldableCard::new(None, header, Some(details.into()))
        .expanded(expanded)
        .on_toggle(move || on_toggle.clone())
        .style(if expanded {
            theme::button::list_entry_unfolded
        } else {
            theme::button::list_entry
        })
        .padding(panels::LIST_ENTRY_PADDING)
        .into()
}

fn info_row<'a, M: 'a>(key: String, value: impl Into<Element<'a, M>>) -> Row<'a, M> {
    row![new::b2_medium(key), value.into()]
        .align_y(Alignment::Center)
        .spacing(HSpacing::S)
}
