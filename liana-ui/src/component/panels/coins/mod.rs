use bitcoin::{Address, Amount, OutPoint, Txid};
use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};
use liana::label::Label;
use liana_i18n::t;

use crate::{
    component::{
        address::address as address_view, amount::amount, badge, button, card,
        label::display_label, pill, text::new,
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
    tx_label: Option<&'a str>,
    block_height: Option<i32>,
    spend: Option<CoinSpend>,
    blockheight: u32,
    timelock: u16,
    seq: u32,
    coin_label: Label,
    label_editor: Element<'a, M>,
    copy_address: M,
    copy_outpoint: M,
    refresh: M,
    expanded: bool,
    on_toggle: M,
) -> Element<'a, M> {
    let outpoint = outpoint.to_string();
    let address = address.to_string();

    // The label is edited in the details, so the header only shows it while folded.
    let label = (!expanded).then(|| display_label(&coin_label, new::CAPTION_SPEC, None));

    let status = if spend.is_some() {
        pill::spent()
    } else if block_height.is_none() {
        pill::unconfirmed()
    } else {
        pill::coin_sequence(seq)
    };
    let summary = row![badge::coin(), label, Space::fill_width(), status]
        .spacing(HSpacing::M)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let header = row![summary, amount(&coin_amount)]
        .align_y(Alignment::Center)
        .spacing(HSpacing::XL);

    let recovery = match (spend, block_height) {
        (None, Some(b)) if blockheight > b as u32 + timelock as u32 => {
            Some(new::b5_bold(t!("coins-recovery-available")).style(theme::text::error))
        }
        (None, Some(b)) => Some(new::b5_bold(t!(
            "coins-first-recovery-in-blocks",
            blocks = b as u32 + timelock as u32 - blockheight
        ))),
        _ => None,
    };

    let address_label = info_row(
        t!("coins-address-label"),
        new::small_caption(
            address_label
                .map(str::to_string)
                .unwrap_or_else(|| t!("common-no-label")),
        )
        .style(theme::text::secondary),
    )
    .align_y(Alignment::Center);
    let copy_address = button::btn_copy(Some(copy_address));
    let address = info_row(
        t!("common-address-label"),
        row![address_view(address), copy_address].align_y(Alignment::Center),
    )
    .align_y(Alignment::Center);
    let deposit = info_row(
        t!("coins-deposit-transaction-label"),
        new::small_caption(
            tx_label
                .map(str::to_string)
                .unwrap_or_else(|| t!("common-no-label")),
        )
        .style(theme::text::secondary),
    )
    .align_y(Alignment::Center);
    let copy_outpoint = button::btn_copy(Some(copy_outpoint));
    let outpoint_row = info_row(
        t!("coins-outpoint"),
        row![
            new::small_caption(outpoint).style(theme::text::secondary),
            copy_outpoint
        ]
        .align_y(Alignment::Center),
    )
    .align_y(Alignment::Center);
    let block_height = block_height.map(|b| {
        info_row(
            t!("coins-block-height"),
            new::small_caption(b.to_string()).style(theme::text::secondary),
        )
    });
    let coin_info = column![address_label, address, deposit, outpoint_row, block_height];

    let spend: Element<'a, M> = match spend {
        Some(CoinSpend { txid, height }) => {
            let spend_txid = info_row(t!("coins-spend-txid"), new::small_caption(txid.to_string()));
            let spend_height = match height {
                Some(height) => info_row(
                    t!("coins-spend-block-height"),
                    new::small_caption(height.to_string()),
                ),
                None => {
                    row![new::b5_bold(t!("coins-not-in-block")).style(theme::text::secondary)]
                }
            };
            column![spend_txid, spend_height]
                .spacing(VSpacing::XS)
                .into()
        }
        None => {
            let refresh = button::btn_refresh_coin(Some(refresh), seq == 0);
            column![row![Space::fill_width(), refresh]].into()
        }
    };

    let details = column![label_editor, recovery, coin_info, spend]
        .padding(10)
        .spacing(VSpacing::XS);

    card::foldable::FoldableCard::new(None, header, Some(details.into()))
        .expanded(expanded)
        .on_toggle(move || on_toggle.clone())
        .padding(card::CardPadding::Soft)
        .into()
}

fn info_row<'a, M: 'a>(key: String, value: impl Into<Element<'a, M>>) -> Row<'a, M> {
    row![
        new::b5_bold(key).style(theme::text::secondary),
        value.into()
    ]
    .spacing(HSpacing::S)
}
