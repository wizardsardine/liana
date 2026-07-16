use std::collections::HashMap;

use iced::{
    widget::{column, row, Column, Space},
    Alignment, Length,
};

use liana::{
    descriptors::LianaPolicy,
    miniscript::bitcoin::{bip32::Fingerprint, Amount, Network},
};

use liana_ui::{
    component::{amount::*, button, form, label::LABEL_LENGTH_WARNING, panels::spend, text::new},
    icon, theme,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        state::{FeeMode, Recipient},
        view::{dashboard, message::*, psbt, FiatAmountConverter},
    },
    daemon::model::{remaining_sequence, Coin, SpendTx},
};

#[allow(clippy::too_many_arguments)]
pub fn spend_view<'a>(
    cache: &'a Cache,
    tx: &'a SpendTx,
    spend_warnings: &'a [String],
    saved: bool,
    desc_info: &'a LianaPolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    network: Network,
    currently_signing: bool,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let is_recovery = tx
        .psbt
        .unsigned_tx
        .input
        .iter()
        .any(|txin| txin.sequence.is_relative_lock_time());

    let title = Container::new(new::d2(if is_recovery {
        Menu::Recovery.title()
    } else {
        Menu::CreateSpendTx.title()
    }))
    .width(Length::Fill);

    let warnings = (!(spend_warnings.is_empty() || saved)).then_some({
        let rows = spend_warnings.iter().map(|warning| {
            let warn_icon = icon::warning_icon().style(theme::text::warning);
            let warn_text = new::caption(warning).style(theme::text::warning);
            row![warn_icon, warn_text].spacing(5).into()
        });
        Column::with_children(rows).padding(15).spacing(5)
    });

    let spend_overview =
        psbt::spend_overview_view(tx, desc_info, key_aliases, currently_signing, saved);

    let inputs = psbt::inputs_view(&tx.coins, &tx.psbt.unsigned_tx, &tx.labels, labels_editing);
    let outputs = psbt::outputs_view(
        &tx.psbt.unsigned_tx,
        network,
        &tx.change_indexes,
        &tx.labels,
        labels_editing,
        tx.is_single_payment().is_some(),
        false,
    );
    let inputs_outputs = column![inputs, outputs].spacing(20);

    let bottom_row = if saved {
        let delete = button::btn_delete(
            (!currently_signing).then_some(Message::Spend(SpendTxMessage::Delete)),
        );
        row![delete].width(Length::Fill)
    } else {
        let previous = button::btn_previous((!currently_signing).then_some(Message::Previous));
        let save_msg = (!currently_signing).then_some(Message::Spend(SpendTxMessage::Save));
        let save = button::btn_save(save_msg, false);
        row![previous, Space::fill_width(), save].width(Length::Fill)
    };

    let header = psbt::spend_header(tx, labels_editing);
    let content = column![
        title,
        header,
        warnings,
        spend_overview,
        inputs_outputs,
        bottom_row,
    ]
    .spacing(20);

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
    batch_label: &form::Value<String>,
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
        "Self-transfer"
    } else {
        Menu::CreateSpendTx.title()
    });
    let self_transfer_btn =
        (!is_self_send && recovery_timelock.is_none()).then_some(button::btn_tertiary(
            None,
            "Self-transfer",
            button::BtnWidth::Auto,
            Some(Message::CreateSpend(CreateSpendMessage::SelfTransfer)),
        ));
    let title = row![title_text, Space::fill_width(), self_transfer_btn].align_y(Alignment::Center);

    let batch_label_input = (recipients.len() > 1).then_some(
        form::Form::new("Batch label", batch_label, |s| {
            Message::CreateSpend(CreateSpendMessage::BatchLabelEdited(s))
        })
        .warning(LABEL_LENGTH_WARNING)
        .size(30)
        .padding(10),
    );

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
        Container::new(
            new::caption("Two payment addresses are the same").style(theme::text::warning),
        )
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
        liana::spend::MAX_FEERATE,
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
    let coin_selection = spend::coin_selection(coin_rows);

    let previous = (!is_first_step).then_some(button::btn_previous(Some(Message::Previous)));
    let clear = button::btn_clear(Some(Message::CreateSpend(CreateSpendMessage::Clear)));
    // Single source of truth for whether the spend can proceed: the same blocker
    // that drives the displayed reason also gates the Next button. `duplicate`
    // and `error` have their own UI feedback, so they only gate the button.
    let next_blocker = next_disabled_reason(
        recipients,
        batch_label,
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
        let content: Element<Message> = match blocker {
            NextBlocker::Reason(msg) => new::caption(msg).style(theme::text::card_secondary).into(),
            NextBlocker::CoinsLeft => match amount_left {
                Some(left) if left.to_sat() > 0 => row![
                    amount_with_font(left, new::CAPTION_SPEC),
                    new::caption("left to select").style(theme::text::card_secondary),
                ]
                .spacing(5)
                .into(),
                _ => new::caption("Select coins to cover the amount")
                    .style(theme::text::card_secondary)
                    .into(),
            },
        };
        Container::new(content)
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Right)
    });

    let content = column![
        title,
        batch_label_input,
        recipients_cards,
        add_payment_row,
        fee_rate_row,
        coin_selection,
        bottom_row,
        next_reason,
        Space::with_height(Length::Fixed(20.0)),
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
    Reason(&'static str),
    CoinsLeft,
}

#[allow(clippy::too_many_arguments)]
fn next_disabled_reason(
    recipients: &[Recipient],
    batch_label: &form::Value<String>,
    feerate: &form::Value<String>,
    amount_left: Option<&Amount>,
    any_coin_selected: bool,
    max_under_dust: bool,
    is_self_send: bool,
    recovery_timelock: Option<u16>,
) -> Option<NextBlocker> {
    let empty_or_invalid = |v: &form::Value<String>| v.value.is_empty() || !v.valid;
    if recipients.iter().any(|r| empty_or_invalid(&r.address)) {
        Some(NextBlocker::Reason(
            "A recipient address is missing or invalid",
        ))
    } else if recipients.iter().any(|r| empty_or_invalid(&r.label))
        || (recipients.len() >= 2 && !batch_label.valid)
    {
        Some(NextBlocker::Reason("A label is missing or invalid"))
    } else if recipients.iter().any(|r| empty_or_invalid(&r.amount)) {
        Some(NextBlocker::Reason(if max_under_dust {
            "Select or add more funds"
        } else {
            "A recipient amount is missing or invalid"
        }))
    } else if empty_or_invalid(feerate) {
        Some(NextBlocker::Reason("The feerate is missing or invalid"))
    } else if !any_coin_selected {
        Some(NextBlocker::Reason("Select at least one coin"))
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
    dust_warning: &'a Option<String>,
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
        dust_warning.as_deref(),
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
    let label = if let Some(label) = coins_labels.get(&coin.outpoint.to_string()) {
        spend::CoinLabel::Outpoint(label.clone())
    } else if let Some(label) = coins_labels.get(&coin.outpoint.txid.to_string()) {
        spend::CoinLabel::Transaction(label.clone())
    } else {
        spend::CoinLabel::None
    };

    let status = if coin.spend_info.is_some() {
        spend::CoinStatus::Spent
    } else if coin.block_height.is_none() {
        spend::CoinStatus::Unconfirmed
    } else {
        spend::CoinStatus::Sequence(remaining_sequence(coin, blockheight, timelock))
    };

    spend::coin_row(
        label,
        &coin.amount,
        status,
        selected,
        Message::CreateSpend(CreateSpendMessage::SelectCoin(i)),
        available_width,
    )
}
