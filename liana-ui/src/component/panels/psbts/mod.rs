use std::collections::HashMap;

use bitcoin::{bip32::Fingerprint, Amount};
use iced::{
    widget::{column, row, text::Style, Space},
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
            self,
            home::payment::{FiatPrice, FiatSource, PaymentKind},
        },
        pill::{self, PillWidth},
        scrollable,
        text::{new, truncate},
    },
    icon,
    spacing::{HSpacing, VSpacing},
    theme::{self, Theme},
    widget::{Column, Container, Element, Row, SpaceExt, Toggler},
};

const PSBT_HEIGHT: u32 = 90;

#[derive(Debug, Clone, Copy)]
pub struct PsbtSigs {
    pub count: usize,
    pub threshold: usize,
}

pub fn status_pill<'a, M: 'a>(status: SpendStatus) -> Option<Container<'a, M>> {
    match status {
        SpendStatus::Unsigned => None,
        SpendStatus::Timelocked => Some(pill::timelocked().width(PillWidth::SM)),
        SpendStatus::Broadcastable => Some(pill::signed().width(PillWidth::M)),
        SpendStatus::Broadcast => Some(pill::unconfirmed().width(PillWidth::SM)),
        SpendStatus::Confirmed => Some(pill::confirmed().width(PillWidth::SM)),
        SpendStatus::Deprecated => Some(pill::deprecated().width(PillWidth::SM)),
        SpendStatus::Unknown => None,
    }
}

pub fn hide_confirmed_row<'a, M: Clone + 'static>(hidden: bool, toggle: M) -> Element<'a, M> {
    let label = new::b4_medium(t!("psbts-hide-confirmed"));
    let toggler = Toggler::new(hidden)
        .on_toggle(move |_| toggle.clone())
        .size(28)
        .style(theme::toggler::primary);

    row![label, toggler, Space::fill_width()]
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
    let count = count.min(threshold);

    let sigs_text = if available_width >= 1460.0 {
        t!(
            "psbts-signatures-collected",
            count = count,
            threshold = threshold
        )
    } else {
        format!("{count}/{threshold}")
    };
    let sig_style: fn(&Theme) -> Style = if !signed {
        theme::text::warning
    } else {
        theme::text::success
    };
    let sigs = new::b4_medium(sigs_text).style(sig_style);

    let recovery_pill = is_recovery.then_some(pill::recovery().width(PillWidth::WalletStatus));
    let batch_pill = is_batch.then_some(pill::batch().width(PillWidth::WalletStatus));

    let status_pill = status_pill(status);

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
    .spacing(HSpacing::XL);
    let left = column![label, sigs].spacing(VSpacing::SM);

    let to_fiat = fiat_price.map(|fp| move |_: Amount| fp.amount);
    let approximate = fiat_price.is_none_or(|fp| fp.source == FiatSource::Timestamp);
    let tooltip = fiat_price.map(|fp| fp.source.infotip());
    let amount = amount_with_fiat_tooltip(&amount, to_fiat, AmountSize::M, approximate, tooltip);
    let spent = row![kind.icon(), amount]
        .spacing(HSpacing::S)
        .align_y(Alignment::Center);

    let content = row![left, spent].spacing(HSpacing::L).height(PSBT_HEIGHT);

    card::list_entry_with_padding(content, msg, panels::LIST_ENTRY_PADDING)
}

pub fn collapsible_section<'a, M: Clone + 'static>(
    title: String,
    rows: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    let rows = Column::with_children(rows)
        .spacing(VSpacing::S)
        .padding(card::CardPadding::Soft);

    let header = new::h3_semi(title).width(Length::Fill);

    card::foldable::FoldableCard::new(None, header, Some(rows.into()))
        .padding(card::CardPadding::Soft)
        .into()
}

fn address_row<'a, M: Clone + 'static>(address: String, copy: M) -> Row<'a, M> {
    let title = new::b5_bold(t!("common-address-label")).style(theme::text::secondary);
    let copy = button::btn_copy(Some(copy));
    row![title, address_view(address), copy]
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .spacing(HSpacing::S)
}

fn address_label_row<'a, M: 'a>(label: &'a str) -> Row<'a, M> {
    let title = new::b5_bold(t!("coins-address-label")).style(theme::text::secondary);
    row![
        title,
        new::small_caption(label).style(theme::text::secondary)
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .spacing(HSpacing::S)
}

pub fn change_row<'a, M: Clone + 'static>(
    value: Amount,
    address: String,
    copy: M,
) -> Element<'a, M> {
    let value = row![Space::fill_width(), amount(&value)];

    column![value, address_row(address, copy)]
        .width(Length::Fill)
        .spacing(VSpacing::XS)
        .into()
}

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
    .spacing(HSpacing::S)
    .align_y(Alignment::Center);

    let title = new::b5_bold(t!("coins-outpoint")).style(theme::text::secondary);
    let outpoint = row![
        title,
        new::small_caption(outpoint).style(theme::text::secondary),
        button::btn_copy(Some(copy_outpoint))
    ]
    .align_y(Alignment::Center)
    .spacing(HSpacing::S);

    let address = address
        .zip(copy_address)
        .map(|(address, copy)| address_row(address, copy));
    let details = column![outpoint, address, address_label.map(address_label_row)];

    column![header, details]
        .width(Length::Fill)
        .spacing(VSpacing::XS)
        .into()
}

pub fn payment_row<'a, M: Clone + 'static>(
    label: Element<'a, M>,
    value: Amount,
    address: Option<String>,
    address_label: Option<&'a str>,
    copy_address: Option<M>,
) -> Element<'a, M> {
    let header = row![Container::new(label).width(Length::Fill), amount(&value)]
        .spacing(HSpacing::S)
        .align_y(Alignment::Center);

    let address = address.zip(copy_address).map(|(address, copy)| {
        column![
            address_row(address, copy),
            address_label.map(address_label_row)
        ]
    });

    column![header, address]
        .width(Length::Fill)
        .spacing(VSpacing::XS)
        .into()
}

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
    let row_unsigned =
        non_signed_fgs
            .into_iter()
            .fold(Row::new().spacing(HSpacing::S), |row, fg| {
                row.push(pill::fingerprint(
                    fg.to_string(),
                    key_aliases.get(&fg).map(String::as_str),
                ))
            });
    let row_signed = signed_fgs
        .into_iter()
        .fold(Row::new().spacing(HSpacing::S), |row, fg| {
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
    let status = row![status, Space::with_width(HSpacing::XL)];

    let missing = new::caption(t!("psbt-more-signatures", count = missing_signatures))
        .style(theme::text::secondary);
    let already_signed = (!sigs.signed_pubkeys.is_empty())
        .then_some(new::caption(t!("psbt-already-signed-by")).style(theme::text::secondary));

    let content =
        row![status, missing, row_unsigned, already_signed, row_signed].align_y(Alignment::Center);

    scrollable::horizontal_thin(content).into()
}

pub fn signatures_ready<'a, M: 'static>(
    sigs: &'a PathSpendInfo,
    key_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, M> {
    let signers = sigs
        .signed_pubkeys
        .keys()
        .fold(Row::new().spacing(HSpacing::S), |row, fg| {
            row.push(pill::fingerprint(
                fg.to_string(),
                key_aliases.get(fg).map(String::as_str),
            ))
        });

    let ready = row![
        new::b5_bold(t!("psbt-status")),
        icon::circle_check_icon().style(theme::text::success),
        new::b5_bold(t!("common-ready")).style(theme::text::success),
        new::caption(t!("psbt-signed-by")),
        signers
    ]
    .align_y(Alignment::Center)
    .spacing(HSpacing::M);

    scrollable::horizontal_thin(ready).into()
}

pub fn signatures_missing<'a, M: 'static>() -> Element<'a, M> {
    let status = row![
        icon::circle_cross_icon().style(theme::text::error),
        new::caption(t!("psbt-not-ready")).style(theme::text::error)
    ]
    .spacing(HSpacing::S)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    row![new::b5_bold(t!("psbt-status")), status]
        .align_y(Alignment::Center)
        .spacing(HSpacing::XL)
        .into()
}

pub fn signatures_requirement<'a, M: 'static>(
    requirement: Option<Element<'a, M>>,
) -> Element<'a, M> {
    column![new::caption(t!("psbt-finalizing-requires")), requirement]
        .padding(15)
        .spacing(VSpacing::S)
        .into()
}

pub fn spend_header<'a, M: 'static>(
    label: Element<'a, M>,
    is_send_to_self: bool,
    spent: Amount,
    fee: Option<Amount>,
    feerate: Option<u64>,
) -> Element<'a, M> {
    let spent: Element<'a, M> = if is_send_to_self {
        new::d2(t!("common-self-transfer")).into()
    } else {
        amount_with_font(&spent, new::D2_SPEC).into()
    };
    let spent = Container::new(spent);

    let missing_inputs = fee
        .is_none()
        .then_some(new::caption(t!("psbt-missing-inputs")));
    let fee = fee.map(|fee| amount_with_font(&fee, new::H1_SPEC));
    let feerate = feerate.map(|rate| {
        new::h3(t!("common-approx-feerate-value", rate = rate)).style(theme::text::secondary)
    });
    let fees = row![
        new::h1(t!("transactions-miner-fee")).style(theme::text::secondary),
        missing_inputs,
        fee,
        new::h1(" "),
        feerate
    ]
    .align_y(Alignment::Center);

    column![label, column![spent, fees]]
        .spacing(VSpacing::L)
        .into()
}

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
    let export_button = button::btn_export_psbt(saved, export);
    let buttons = row![export_button, button::btn_import(import)].spacing(HSpacing::S);
    let header = row![new::b5_bold(t!("psbt-title")).width(Length::Fill), buttons]
        .align_y(Alignment::Center);

    let txid = row![
        new::b5_bold(t!("transactions-txid")).width(Length::Fill),
        new::small_caption(txid).style(theme::text::secondary),
        button::btn_copy(Some(copy_txid))
    ]
    .align_y(Alignment::Center);

    let psbt = column![header, txid].spacing(VSpacing::S);
    let card = card::foldable::FoldableCard::new(Some(psbt.into()), status, details)
        .padding(card::CardPadding::Soft);

    let action = action.map(|action| {
        row![Space::fill_width(), action]
            .align_y(Alignment::Center)
            .spacing(HSpacing::XL)
    });

    column![card, action].spacing(VSpacing::L).into()
}
