use std::collections::HashMap;

use bitcoin::{bip32::Fingerprint, Amount};
use iced::{
    widget::{column, row, text::Style, tooltip, Space},
    Alignment, Length,
};
use liana::{
    descriptors::{PathInfo, PathSpendInfo},
    spend::SpendStatus,
};
use liana_i18n::t;

use crate::{
    component::{
        address::address as address_view,
        amount::{amount, amount_with_fiat_tooltip, amount_with_font, AmountSize},
        button, card,
        panels::{
            home::payment::{FiatPrice, FiatSource, PaymentKind},
            LIST_ENTRY_PADDING,
        },
        pill, scrollable,
        text::{legacy, new, truncate, Text as _},
    },
    icon,
    spacing::HSpacing,
    theme::{self, Theme},
    widget::{Column, Container, Element, Row, SpaceExt, Toggler},
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

/// Section of the psbt page folding the given rows under a title.
pub fn collapsible_section<'a, M: Clone + 'static>(
    title: String,
    rows: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    let rows = Column::with_children(rows).spacing(10).padding(20);

    let header = legacy::h4_bold(title).width(Length::Fill);

    card::foldable::FoldableCard::new(None, header, Some(rows.into()))
        .padding(20)
        .into()
}

/// The address line of a change, payment or input row.
fn address_row<'a, M: Clone + 'static>(address: String, copy: M) -> Row<'a, M> {
    let title = new::b5_bold(t!("common-address-label")).style(theme::text::secondary);
    let copy = button::btn_copy(Some(copy));
    row![title, address_view(address), copy]
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .spacing(5)
}

/// The address label line of a payment or input row.
fn address_label_row<'a, M: 'a>(label: &'a str) -> Row<'a, M> {
    let title = new::b5_bold(t!("coins-address-label")).style(theme::text::secondary);
    row![
        title,
        legacy::p2_regular(label).style(theme::text::secondary)
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .spacing(5)
}

/// Row of a change output: what it holds and where it goes.
pub fn change_row<'a, M: Clone + 'static>(
    value: Amount,
    address: String,
    copy: M,
) -> Element<'a, M> {
    let value = row![Space::fill_width(), amount(&value)];

    column![value, address_row(address, copy)]
        .width(Length::Fill)
        .spacing(5)
        .into()
}

/// Row of a coin being spent: its label and amount, then where it comes from.
#[allow(clippy::too_many_arguments)]
pub fn input_row<'a, M: Clone + 'static>(
    label: Element<'a, M>,
    value: Option<Amount>,
    outpoint: String,
    copy_outpoint: M,
    address: Option<String>,
    address_label: Option<&'a str>,
    copy_address: Option<M>,
) -> Element<'a, M> {
    let header = row![
        Container::new(label).width(Length::Fill),
        value.map(|value| amount(&value))
    ]
    .spacing(5)
    .align_y(Alignment::Center);

    let title = new::b5_bold(t!("coins-outpoint")).style(theme::text::secondary);
    let outpoint = row![
        title,
        legacy::p2_regular(outpoint).style(theme::text::secondary),
        button::btn_copy(Some(copy_outpoint))
    ]
    .align_y(Alignment::Center)
    .spacing(5);

    let address = address
        .zip(copy_address)
        .map(|(address, copy)| address_row(address, copy));
    let details = column![outpoint, address, address_label.map(address_label_row)];

    column![header, details]
        .width(Length::Fill)
        .spacing(5)
        .into()
}

/// Row of a payment: its label and amount, then where it goes.
pub fn payment_row<'a, M: Clone + 'static>(
    label: Element<'a, M>,
    value: Amount,
    address: Option<String>,
    address_label: Option<&'a str>,
    copy_address: Option<M>,
) -> Element<'a, M> {
    let header = row![Container::new(label).width(Length::Fill), amount(&value)]
        .spacing(5)
        .align_y(Alignment::Center);

    let address = address.zip(copy_address).map(|(address, copy)| {
        column![
            address_row(address, copy),
            address_label.map(address_label_row)
        ]
    });

    column![header, address]
        .width(Length::Fill)
        .spacing(5)
        .into()
}

/// What a spending path still requires to be satisfied, and who signed for it already.
pub fn path_row<'a, M: 'static>(
    path: &'a PathInfo,
    sigs: &'a PathSpendInfo,
    key_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, M> {
    // We get a sorted list of all the fingerprints (which correspond to a signer) from this
    // spending path, and from it get an iterator on those of these fingerprints for which a
    // signature was provided in the PSBT, and those for which there isn't any.
    let mut all_fgs: Vec<Fingerprint> = path.thresh_origins().1.into_keys().collect();
    all_fgs.sort();
    let signed_fgs = sigs.signed_pubkeys.keys();
    let non_signed_fgs = all_fgs
        .into_iter()
        .filter(|fg| !sigs.signed_pubkeys.contains_key(fg));
    let missing_signatures = sigs.threshold.saturating_sub(sigs.sigs_count);

    // From these iterators, create the appropriate rows to be displayed.
    let row_unsigned = non_signed_fgs.into_iter().fold(None, |row, fg| {
        Some(
            row.unwrap_or_else(|| Row::new().spacing(5))
                .push(pill::fingerprint(
                    fg.to_string(),
                    key_aliases.get(&fg).map(String::as_str),
                )),
        )
    });
    let row_signed = signed_fgs
        .into_iter()
        .fold(Row::new().spacing(5), |row, fg| {
            row.push(pill::fingerprint(
                fg.to_string(),
                key_aliases.get(fg).map(String::as_str),
            ))
        });

    let status = if missing_signatures == 0 {
        icon::circle_check_icon().style(theme::text::success)
    } else {
        icon::circle_cross_icon().style(theme::text::secondary)
    };
    let status = row![status, Space::with_width(20)];

    let missing = new::caption(t!("psbt-more-signatures", count = missing_signatures))
        .style(theme::text::secondary);
    let already_signed = (!sigs.signed_pubkeys.is_empty())
        .then_some(new::caption(t!("psbt-already-signed-by")).style(theme::text::secondary));

    let content =
        row![status, missing, row_unsigned, already_signed, row_signed].align_y(Alignment::Center);

    scrollable::horizontal_thin(content).into()
}

/// Signature status row of a psbt that can be broadcast, with the keys that signed it.
pub fn signatures_ready<'a, M: 'static>(
    sigs: &'a PathSpendInfo,
    key_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, M> {
    let signers = sigs
        .signed_pubkeys
        .keys()
        .fold(Row::new().spacing(5), |row, fg| {
            row.push(pill::fingerprint(
                fg.to_string(),
                key_aliases.get(fg).map(String::as_str),
            ))
        });

    let ready = row![
        new::b5_bold(t!("psbt-status")),
        icon::circle_check_icon().style(theme::text::success),
        new::b5_bold(t!("common-ready")).style(theme::text::success),
        legacy::text(t!("psbt-signed-by")),
        signers
    ]
    .align_y(Alignment::Center)
    .spacing(10);

    scrollable::horizontal_thin(ready).into()
}

/// Signature status row of a psbt that still misses signatures.
pub fn signatures_missing<'a, M: 'static>() -> Element<'a, M> {
    let status = row![
        icon::circle_cross_icon().style(theme::text::error),
        legacy::text(t!("psbt-not-ready")).style(theme::text::error)
    ]
    .spacing(5)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    row![new::b5_bold(t!("psbt-status")), status]
        .align_y(Alignment::Center)
        .spacing(20)
        .into()
}

/// What a psbt still misses signatures for before it can be finalized.
pub fn signatures_requirement<'a, M: 'static>(
    requirement: Option<Element<'a, M>>,
) -> Element<'a, M> {
    column![legacy::text(t!("psbt-finalizing-requires")), requirement]
        .padding(15)
        .spacing(10)
        .into()
}

/// Header of a psbt: its label, then what it sends out and what it costs.
pub fn spend_header<'a, M: 'static>(
    label: Element<'a, M>,
    is_send_to_self: bool,
    spent: Amount,
    fee: Option<Amount>,
    feerate: Option<u64>,
) -> Element<'a, M> {
    let spent = if is_send_to_self {
        Container::new(legacy::h1(t!("common-self-transfer")))
    } else {
        Container::new(amount_with_font(&spent, legacy::H1_SPEC))
    };

    let missing_inputs = fee
        .is_none()
        .then_some(legacy::text(t!("psbt-missing-inputs")));
    let fee = fee.map(|fee| amount_with_font(&fee, legacy::H3_SPEC));
    let feerate = feerate.map(|rate| {
        legacy::text(t!("common-approx-feerate-value", rate = rate))
            .size(legacy::H4_SIZE)
            .style(theme::text::secondary)
    });
    let fees = row![
        legacy::h3(t!("transactions-miner-fee")).style(theme::text::secondary),
        missing_inputs,
        fee,
        legacy::text(" ").size(legacy::H3_SIZE),
        feerate
    ]
    .align_y(Alignment::Center);

    column![label, column![spent, fees]].spacing(20).into()
}

/// The psbt card: what can be done with the psbt itself, its txid, and how far its signing is.
#[allow(clippy::too_many_arguments)]
pub fn spend_overview<'a, M: Clone + 'static>(
    saved: bool,
    export: Option<M>,
    import: Option<M>,
    txid: String,
    copy_txid: M,
    status: Element<'a, M>,
    details: Option<Element<'a, M>>,
    action: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let export_button = if saved {
        Container::new(button::btn_export(export))
    } else {
        Container::new(tooltip::Tooltip::new(
            button::btn_export(export),
            Container::new(new::caption(t!("psbt-sign-save-before-export")))
                .style(theme::card::simple)
                .padding(10),
            tooltip::Position::Top,
        ))
    };
    let buttons = row![export_button, button::btn_import(import)].spacing(5);
    let header =
        row![legacy::text("PSBT").bold().width(Length::Fill), buttons].align_y(Alignment::Center);

    let txid = row![
        new::b5_bold(t!("transactions-txid")).width(Length::Fill),
        legacy::p2_regular(txid).style(theme::text::secondary),
        button::btn_copy(Some(copy_txid))
    ]
    .align_y(Alignment::Center);

    let psbt = column![header, txid].spacing(10);
    let card = card::foldable::FoldableCard::new(Some(psbt.into()), status, details).padding(15);

    let action = action.map(|action| {
        row![Space::fill_width(), action]
            .align_y(Alignment::Center)
            .spacing(20)
    });

    column![card, action].spacing(20).into()
}
