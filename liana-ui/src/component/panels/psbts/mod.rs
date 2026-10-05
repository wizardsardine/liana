use std::collections::HashMap;

use bitcoin::{bip32::Fingerprint, Amount};
use iced::{
    widget::{column, row, text::Style, Space},
    Alignment, Length,
};
use liana::{
    descriptors::{PathInfo, PathSpendInfo},
    label::Label,
    spend::SpendStatus,
    transaction::PaymentKind,
};
use liana_i18n::t;

use crate::{
    component::{
        address::address as address_view,
        amount::{amount, amount_with_fiat_tooltip, AmountSize},
        button, card,
        checkbox::{self, TogglerSize},
        label::display_label,
        panels::{
            self,
            home::payment::{kind_icon, FiatPrice, FiatSource},
        },
        pill::{self, PillWidth},
        scrollable,
        text::{self, new},
    },
    spacing::{HSpacing, VSpacing},
    theme::{self, Theme},
    widget::{Column, Container, Element, Row, SpaceExt},
};

#[derive(Debug, Clone, Copy)]
pub struct PsbtSigs {
    pub count: usize,
    pub threshold: usize,
}

pub fn status_pill<'a, M: 'a>(status: SpendStatus) -> Option<Container<'a, M>> {
    match status {
        SpendStatus::Unsigned => Some(pill::unsigned().width(PillWidth::M)),
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
    let toggler = checkbox::toggler_button(hidden, TogglerSize::Large, move |_| toggle.clone());

    row![label, toggler, Space::fill_width()]
        .spacing(HSpacing::M)
        .align_y(Alignment::Center)
        .into()
}

#[allow(clippy::too_many_arguments)]
pub fn list_entry<'a, M: Clone + 'static>(
    label: &Label,
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

    let sigs_text = |count: usize, threshold: usize| {
        if available_width >= 1460.0 {
            t!(
                "psbts-signatures-collected",
                count = count,
                threshold = threshold
            )
        } else {
            format!("{count}/{threshold}")
        }
    };
    let sig_style: fn(&Theme) -> Style = if !signed {
        theme::text::warning
    } else {
        theme::text::success
    };
    // Sized to the widest text, so the pills after it line up from one entry to another.
    let sigs_width = text::width(
        &sigs_text(9, 9),
        new::B4_MEDIUM_SPEC.font,
        new::B4_MEDIUM_SPEC.size.expect("size"),
    );
    let sigs = new::b4_medium(sigs_text(count, threshold))
        .style(sig_style)
        .width(sigs_width);

    let recovery_pill = is_recovery.then_some(pill::recovery().width(PillWidth::WalletStatus));
    let batch_pill = is_batch.then_some(pill::batch().width(PillWidth::WalletStatus));

    let status_pill = status_pill(status);

    let max_lbl_chars = (available_width - 500.0) as usize / 22;
    let (kind, label) = if is_send_to_self {
        let label = new::h2(t!("common-self-transfer")).style(theme::text::primary);
        (PaymentKind::SendToSelf, label.into())
    } else {
        let label = display_label(label, new::H2_SPEC, Some(max_lbl_chars));
        (PaymentKind::Outgoing, label)
    };

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
    let spent = row![kind_icon(kind), amount]
        .spacing(HSpacing::S)
        .align_y(Alignment::Center);

    let content = row![left, spent]
        .spacing(HSpacing::L)
        .height(panels::ListEntryHeight::Standard);

    card::list_entry_with_padding(content, msg, panels::LIST_ENTRY_PADDING)
}

pub fn collapsible_section<'a, M: Clone + 'static>(
    title: String,
    rows: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    let rows = Column::with_children(rows)
        .spacing(VSpacing::S)
        .padding(card::CardPadding::Soft);

    let header = row![new::h3_semi(title)]
        .height(panels::ListEntryHeight::Standard)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    card::foldable::FoldableCard::new(None, header, Some(rows.into()))
        .list_chevrons()
        .padding(panels::FOLDABLE_ENTRY_PADDING)
        .into()
}

pub enum Signatures<'a> {
    Ready(&'a PathSpendInfo),
    Missing(&'a PathInfo, &'a PathSpendInfo),
}

#[allow(clippy::too_many_arguments)]
pub fn signatures_card<'a, M: Clone + 'static>(
    signatures: Signatures<'a>,
    key_aliases: &'a HashMap<Fingerprint, String>,
    saved: bool,
    export: Option<M>,
    import: Option<M>,
    sign: Option<M>,
    broadcast: Option<M>,
) -> Element<'a, M> {
    let pills = |fingerprints: Vec<Fingerprint>| {
        fingerprints
            .into_iter()
            .fold(Row::new().spacing(HSpacing::S), |row, fg| {
                row.push(pill::fingerprint(
                    fg.to_string(),
                    key_aliases.get(&fg).map(String::as_str),
                ))
            })
    };
    let caption = |text: String| new::caption(text).style(theme::text::secondary);

    let sigs = match signatures {
        Signatures::Ready(sigs) | Signatures::Missing(_, sigs) => sigs,
    };
    let mut signed: Vec<Fingerprint> = sigs.signed_pubkeys.keys().copied().collect();
    signed.sort();
    let required = match signatures {
        Signatures::Ready(_) => row![caption(t!("psbt-fully-signed-by")), pills(signed)],
        Signatures::Missing(path, sigs) => {
            let mut unsigned: Vec<Fingerprint> = path
                .thresh_origins()
                .1
                .into_keys()
                .filter(|fg| !sigs.signed_pubkeys.contains_key(fg))
                .collect();
            unsigned.sort();
            let missing = sigs.threshold.saturating_sub(sigs.sigs_count);
            let already_signed = (!signed.is_empty()).then(|| {
                row![caption(t!("psbt-already-signed-by")), pills(signed)]
                    .spacing(HSpacing::S)
                    .align_y(Alignment::Center)
            });
            row![
                caption(t!("psbt-requires-signatures", count = missing)),
                pills(unsigned),
                already_signed
            ]
        }
    }
    .spacing(HSpacing::S)
    .align_y(Alignment::Center);

    let count = t!(
        "psbt-signatures-count",
        count = sigs.sigs_count.min(sigs.threshold),
        threshold = sigs.threshold
    );
    let header = row![
        new::b2_medium(t!("psbt-signatures")).style(theme::text::primary),
        new::b2(count).style(theme::text::secondary),
        Space::fill_width(),
        button::btn_export_psbt(saved, export),
        button::btn_import(import),
        sign.map(|msg| button::btn_sign(Some(msg))),
        broadcast.map(|msg| button::btn_broadcast(Some(msg))),
    ]
    .spacing(HSpacing::S)
    .align_y(Alignment::Center);

    let content = column![header, scrollable::horizontal_thin(required)].spacing(VSpacing::S);
    Container::new(content)
        .padding(card::CardPadding::Soft)
        .style(theme::card::button_simple)
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
    label: Option<Element<'a, M>>,
    value: Amount,
    address: String,
    copy: M,
) -> Element<'a, M> {
    let label = label.unwrap_or_else(|| Space::fill_width().into());
    let header = row![Container::new(label).width(Length::Fill), amount(&value)]
        .spacing(HSpacing::S)
        .align_y(Alignment::Center);

    column![header, address_row(address, copy)]
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
