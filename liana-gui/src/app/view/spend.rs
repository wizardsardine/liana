use std::collections::HashMap;

use iced::{
    widget::{column, row, Column, Space},
    Alignment, Length,
};

use liana::{
    descriptors::LianaPolicy,
    miniscript::bitcoin::{bip32::Fingerprint, Amount},
};

use lianad::commands::CreateRecoveryWarning;

use liana_ui::{
    component::{
        amount::*,
        button, form,
        panels::spend::{self, DustWarning},
        text::new,
    },
    icon,
    spacing::VSpacing,
    theme,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        state::{FeeMode, Recipient},
        view::{
            dashboard,
            message::*,
            transaction::{tx_view, TxDetail},
            FiatAmountConverter,
        },
    },
    daemon::model::{remaining_sequence, Coin, SpendTx},
    t,
};

#[allow(clippy::too_many_arguments)]
pub fn spend_view<'a>(
    cache: &'a Cache,
    tx: &'a SpendTx,
    spend_warnings: &'a [CreateRecoveryWarning],
    saved: bool,
    desc_info: &'a LianaPolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    currently_signing: bool,
    warning: Option<&'a Error>,
    fiat_converter: Option<FiatAmountConverter>,
) -> Element<'a, Message> {
    let is_recovery = tx
        .psbt
        .unsigned_tx
        .input
        .iter()
        .any(|txin| txin.sequence.is_relative_lock_time());

    let warnings = (!(spend_warnings.is_empty() || saved)).then_some({
        let rows = spend_warnings.iter().map(|warning| {
            let text = match warning {
                CreateRecoveryWarning::ToOwnAddress => t!("spend-warning-recovery-own-address"),
                // Worded by the daemon or the Connect API, so it stays as it comes.
                CreateRecoveryWarning::String(warning) => warning.clone(),
            };
            let warn_icon = icon::warning_icon().style(theme::text::warning);
            let warn_text = new::caption(text).style(theme::text::warning);
            row![warn_icon, warn_text].spacing(5).into()
        });
        Column::with_children(rows).padding(15).spacing(5)
    });

    let detail = TxDetail::Psbt {
        tx,
        desc_info,
        key_aliases,
        saved,
        currently_signing,
        previous: true,
    };
    let detail = tx_view(cache, detail, labels_editing, fiat_converter);
    let content = column![warnings, detail].spacing(80);

    dashboard(
        if is_recovery {
            &Menu::Recovery
        } else {
            &Menu::CreateSpendTx
        },
        cache,
        warning,
        content,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_spend_tx<'a>(
    cache: &'a Cache,
    fiat_converter: Option<&FiatAmountConverter>,
    recipients: &'a [Recipient],
    send_max_to_recipient: Option<usize>,
    duplicate: bool,
    timelock: u16,
    recovery_timelock: Option<u16>,
    coins: &[(Coin, bool)],
    coins_labels: &'a HashMap<String, String>,
    tx_label: Option<&form::Value<String>>,
    amount_left: Option<&Amount>,
    feerate: &form::Value<String>,
    fee_mode: FeeMode,
    fee_amount: Option<&Amount>,
    error: Option<&'a Error>,
    is_first_step: bool,
    max_under_dust: bool,
) -> Element<'a, Message> {
    let is_self_send = recipients.is_empty();

    let title_text = new::d2(if recovery_timelock.is_some() {
        Menu::Recovery.title()
    } else if is_self_send {
        t!("common-self-transfer")
    } else {
        Menu::CreateSpendTx.title()
    });
    let self_transfer_btn =
        (!is_self_send && recovery_timelock.is_none()).then_some(button::btn_tertiary(
            None,
            t!("common-self-transfer"),
            button::BtnWidth::Auto,
            Some(Message::CreateSpend(CreateSpendMessage::SelfTransfer)),
        ));
    let title_row =
        row![title_text, Space::fill_width(), self_transfer_btn].align_y(Alignment::Center);
    let subtitle = is_self_send
        .then_some(new::b2(t!("spend-self-transfer-info")).style(theme::text::secondary));
    let title = column![title_row, subtitle].spacing(VSpacing::L);

    let tx_label_input = tx_label.map(|tx_label| {
        form::Form::new(t!("spend-tx-label"), tx_label, |s| {
            Message::CreateSpend(CreateSpendMessage::TxLabelEdited(s))
        })
        .label(t!("spend-description"))
        .warning(t!("label-invalid-length"))
    });

    let recipient_views = recipients.iter().enumerate().map(|(i, recipient)| {
        recipient
            .view(
                i,
                send_max_to_recipient == Some(i),
                fiat_converter,
                recipients.len() > 1,
            )
            .map(Message::CreateSpend)
    });
    let recipients_cards = Column::with_children(recipient_views).spacing(10);

    let duplicates_warning = duplicate.then_some(
        Container::new(new::caption(t!("spend-duplicate-addresses")).style(theme::text::warning))
            .padding(10),
    );
    let add_payment_btn = (!(is_self_send || recovery_timelock.is_some())).then_some(
        button::btn_add_payment(Some(Message::CreateSpend(CreateSpendMessage::AddRecipient))),
    );
    let add_payment_row = row![duplicates_warning, Space::fill_width(), add_payment_btn];

    let smart_fee = cache.feerate_estimate.map(|est| match fee_mode {
        FeeMode::Manual => spend::SmartFee::Manual {
            on_smart: Message::CreateSpend(CreateSpendMessage::FeeModeSmart),
        },
        FeeMode::Smart(level) => spend::SmartFee::Smart {
            level,
            on_manual: Message::CreateSpend(CreateSpendMessage::FeeModeManual),
            on_low: Message::CreateSpend(CreateSpendMessage::SelectFeeLevel(spend::FeeLevel::Low)),
            on_medium: est.medium.map(|_| {
                Message::CreateSpend(CreateSpendMessage::SelectFeeLevel(spend::FeeLevel::Medium))
            }),
            on_high: Message::CreateSpend(CreateSpendMessage::SelectFeeLevel(
                spend::FeeLevel::High,
            )),
        },
    });
    let to_fiat = fiat_converter.map(|conv| move |a: Amount| conv.convert(a));
    let fee_rate_row = spend::fee_rate_row(
        smart_fee,
        feerate,
        |msg| Message::CreateSpend(CreateSpendMessage::FeerateEdited(msg)),
        fee_amount,
        to_fiat,
        cache.pane_size.get().width,
        liana::spend::MAX_FEERATE_VB,
    );

    let coin_rows = coins
        .iter()
        .enumerate()
        .map(|(i, (coin, selected))| {
            coin_list_view(
                i,
                coin,
                coins_labels,
                timelock,
                cache.blockheight() as u32,
                *selected,
                cache.pane_size.get().width,
            )
        })
        .collect();
    let coin_selection = spend::coin_selection(coin_rows, is_self_send);

    let previous = (!is_first_step).then_some(button::btn_previous(Some(Message::Previous)));
    let clear = button::btn_clear(Some(Message::CreateSpend(CreateSpendMessage::Clear)));
    // Single source of truth for whether the spend can proceed: the same blocker
    // that drives the displayed reason also gates the Next button. `duplicate`
    // and `error` have their own UI feedback, so they only gate the button.
    let next_blocker = next_disabled_reason(
        recipients,
        tx_label,
        feerate,
        amount_left,
        coins.iter().any(|(_, selected)| *selected),
        max_under_dust,
        is_self_send,
        recovery_timelock,
    );
    let next_enabled = next_blocker.is_none() && !duplicate && error.is_none();
    let next = button::btn_next(
        next_enabled.then_some(Message::CreateSpend(CreateSpendMessage::Generate)),
    );
    let bottom_row = row![previous, Space::fill_width(), clear, next]
        .spacing(20)
        .align_y(Alignment::Center);

    let next_reason = next_blocker.map(|blocker| {
        let reason = |text: String| -> Element<Message> {
            new::caption(text).style(theme::text::card_secondary).into()
        };
        let content = match blocker {
            NextBlocker::RecipientAddress => reason(t!("spend-recipient-address-invalid")),
            NextBlocker::PaymentDescription => reason(t!("spend-payment-description-invalid")),
            NextBlocker::Funds => reason(t!("spend-select-or-add-funds")),
            NextBlocker::RecipientAmount => reason(t!("spend-recipient-amount-invalid")),
            NextBlocker::Feerate => reason(t!("spend-feerate-missing-invalid")),
            NextBlocker::Coin => reason(t!("spend-select-one-coin")),
            NextBlocker::CoinsLeft => match amount_left {
                Some(left) if left.to_sat() > 0 => row![
                    amount_with_font(left, new::CAPTION_SPEC),
                    reason(t!("spend-left-to-select")),
                ]
                .spacing(5)
                .into(),
                _ => reason(t!("spend-select-coins-to-cover-amount")),
            },
        };
        Container::new(content)
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Right)
    });

    let content = column![
        title,
        tx_label_input,
        recipients_cards,
        add_payment_row,
        fee_rate_row,
        coin_selection,
        bottom_row,
        next_reason,
        Space::with_height(20),
    ]
    .spacing(20);

    dashboard(
        if recovery_timelock.is_some() {
            &Menu::Recovery
        } else {
            &Menu::CreateSpendTx
        },
        cache,
        error,
        content,
    )
}

enum NextBlocker {
    RecipientAddress,
    PaymentDescription,
    Funds,
    RecipientAmount,
    Feerate,
    Coin,
    CoinsLeft,
}

#[allow(clippy::too_many_arguments)]
fn next_disabled_reason(
    recipients: &[Recipient],
    tx_label: Option<&form::Value<String>>,
    feerate: &form::Value<String>,
    amount_left: Option<&Amount>,
    any_coin_selected: bool,
    max_under_dust: bool,
    is_self_send: bool,
    recovery_timelock: Option<u16>,
) -> Option<NextBlocker> {
    let empty_or_invalid = |v: &form::Value<String>| v.value.is_empty() || !v.valid;
    if recipients.iter().any(|r| empty_or_invalid(&r.address)) {
        Some(NextBlocker::RecipientAddress)
    } else if recipients.iter().any(|r| empty_or_invalid(&r.label))
        || tx_label.is_some_and(|tx_label| !tx_label.valid)
    {
        Some(NextBlocker::PaymentDescription)
    } else if recipients.iter().any(|r| empty_or_invalid(&r.amount)) {
        Some(if max_under_dust {
            NextBlocker::Funds
        } else {
            NextBlocker::RecipientAmount
        })
    } else if empty_or_invalid(feerate) {
        Some(NextBlocker::Feerate)
    } else if !any_coin_selected {
        Some(NextBlocker::Coin)
    } else if !is_self_send
        && recovery_timelock.is_none()
        && amount_left != Some(&Amount::from_sat(0))
    {
        Some(NextBlocker::CoinsLeft)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
pub fn recipient_view<'a>(
    index: usize,
    address: &'a form::Value<String>,
    amount: &'a form::Value<String>,
    fiat_form_value: Option<&'a form::Value<String>>,
    fiat_converter: Option<&FiatAmountConverter>,
    label: &'a form::Value<String>,
    is_max_selected: bool,
    is_recovery: bool,
    can_delete: bool,
    dust_warning: Option<DustWarning>,
    max_estimated_amount: Option<Amount>,
) -> Element<'a, CreateSpendMessage> {
    let fiat = fiat_converter.map(|conv| {
        let conv = *conv;
        spend::RecipientFiat {
            currency: conv.currency(),
            to_fiat: Box::new(move |a| conv.convert(a)),
            form_value: fiat_form_value,
            summary: conv.to_container_summary().into(),
            on_edit: Box::new(move |msg| {
                CreateSpendMessage::RecipientFiatAmountEdited(index, msg, conv)
            }),
        }
    });

    let on_max = (!is_recovery).then_some(CreateSpendMessage::SendMaxToRecipient(index));
    let on_delete =
        (can_delete && !is_recovery).then_some(CreateSpendMessage::DeleteRecipient(index));

    spend::recipient_card(
        address,
        label,
        amount,
        fiat,
        is_max_selected,
        dust_warning,
        max_estimated_amount,
        move |msg| CreateSpendMessage::RecipientEdited(index, "address", msg.trim().to_string()),
        move |msg| CreateSpendMessage::RecipientEdited(index, "label", msg),
        move |msg| CreateSpendMessage::RecipientEdited(index, "amount", msg),
        on_max,
        on_delete,
    )
}

fn coin_list_view<'a>(
    i: usize,
    coin: &Coin,
    coins_labels: &'a HashMap<String, String>,
    timelock: u16,
    blockheight: u32,
    selected: bool,
    available_width: f32,
) -> Element<'a, Message> {
    let status = if coin.spend_info.is_some() {
        spend::CoinStatus::Spent
    } else if coin.block_height.is_none() {
        spend::CoinStatus::Unconfirmed
    } else {
        spend::CoinStatus::Sequence(remaining_sequence(coin, blockheight, timelock))
    };

    let own_label = coins_labels
        .get(&coin.outpoint.to_string())
        .map(String::as_str);
    let label = liana::label::resolve(own_label, &coin.default_label);
    spend::coin_row(
        &label,
        &coin.amount,
        status,
        selected,
        Message::CreateSpend(CreateSpendMessage::SelectCoin(i)),
        available_width,
    )
}
