use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use std::collections::HashMap;
use std::time::Duration;

use iced::{
    advanced::widget::Id,
    widget::{
        button::{Status as ButtonStatus, Style as ButtonStyle},
        checkbox, scrollable, tooltip, Column, Container, Row, Space,
    },
    Alignment, Background, Border, Color, Length,
};

use coincube_core::{
    descriptors::CoincubePolicy,
    miniscript::bitcoin::{bip32::Fingerprint, Amount, Denomination, Network},
};

use coincube_ui::{
    color,
    component::{amount::*, badge, button, form, spinner, text::*},
    font,
    icon::{self},
    theme,
    widget::{action::Action, Button, ColumnExt, Element, RowExt},
};

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        view::{
            dashboard_headed,
            message::*,
            shared::payment_columns::{payment_columns, rail_card},
            vault::coins,
            vault::psbt,
            FiatAmountConverter,
        },
        wallet::SyncStatus,
    },
    daemon::model::{remaining_sequence, Coin, SpendStatus, SpendTx},
};

#[allow(clippy::too_many_arguments)]
pub fn spend_view<'a>(
    action_scope: u64,
    menu: &'a Menu,
    cache: &'a Cache,
    tx: &'a SpendTx,
    spend_warnings: &'a [String],
    saved: bool,
    desc_info: &'a CoincubePolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    network: Network,
    currently_signing: bool,
    bitcoin_unit: BitcoinDisplayUnit,
    identities: &'a [(usize, crate::services::branta::LookupResult)],
    replay: Option<psbt::ReplayPill<'a>>,
    broadcast_ready: bool,
    mcp_review: bool,
) -> Element<'a, Message> {
    let is_recovery = tx
        .psbt
        .unsigned_tx
        .input
        .iter()
        .any(|txin| txin.sequence.is_relative_lock_time());
    dashboard_headed(
        menu,
        cache,
        send_head(
            cache,
            if is_recovery { "Recovery" } else { "Send" },
            review_step(saved, &tx.status),
        ),
        Column::new()
            .spacing(20)
            .push(p1_regular(crate::chain::canonical_network_name(
                cache.chain(),
            )))
            .push_maybe(mcp_review.then(|| mcp_prepared_review(tx, network)))
            .push(psbt::spend_header(tx, labels_editing, bitcoin_unit))
            .push_maybe(if spend_warnings.is_empty() || saved {
                None
            } else {
                Some(spend_warnings.iter().fold(
                    Column::new().padding(15).spacing(5),
                    |col, warning| {
                        col.push(
                            Row::new()
                                .spacing(5)
                                .push(icon::warning_icon().style(theme::text::warning))
                                .push(text(warning).style(theme::text::warning)),
                        )
                    },
                ))
            })
            .push(psbt::spend_overview_view(
                action_scope,
                tx,
                desc_info,
                key_aliases,
                currently_signing,
                saved,
                replay,
                broadcast_ready,
                !mcp_review,
            ))
            .push(
                Column::new()
                    .spacing(20)
                    .push(psbt::inputs_view(
                        &tx.coins,
                        &tx.psbt.unsigned_tx,
                        &tx.labels,
                        labels_editing,
                        bitcoin_unit,
                    ))
                    .push(psbt::outputs_view_with_identities(
                        &tx.psbt.unsigned_tx,
                        network,
                        &tx.change_indexes,
                        &tx.labels,
                        labels_editing,
                        bitcoin_unit,
                        tx.is_single_payment().is_some(),
                        false,
                        identities,
                        cache.theme_mode,
                    )),
            )
            .push(if saved {
                Row::new()
                    .push(
                        button::secondary(None, localized_copy("Delete"))
                            .width(Length::Fixed(200.0))
                            .on_press_maybe(if currently_signing {
                                None
                            } else {
                                Some(Message::Spend(SpendTxMessage::Delete))
                            }),
                    )
                    .width(Length::Fill)
            } else {
                Row::new()
                    .push(
                        button::secondary(None, "< Back")
                            .width(Length::Fixed(150.0))
                            .on_press_maybe(if currently_signing {
                                None
                            } else {
                                Some(Message::Previous)
                            }),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::secondary(
                            None,
                            if mcp_review {
                                localized_copy("Sign in COINCUBE Desktop")
                            } else {
                                localized_copy("Save")
                            },
                        )
                        .width(Length::Fixed(if mcp_review { 180.0 } else { 150.0 }))
                        .on_press_maybe(if currently_signing {
                            None
                        } else if mcp_review {
                            #[cfg(feature = "mcp-transactions")]
                            {
                                Some(Message::Spend(SpendTxMessage::ContinueNativeSigning))
                            }
                            #[cfg(not(feature = "mcp-transactions"))]
                            {
                                None
                            }
                        } else {
                            Some(Message::Spend(SpendTxMessage::Save))
                        }),
                    )
                    .width(Length::Fill)
            }),
    )
}

/// Stable focus identities for the compose actions. They are fixed so focus
/// survives re-rendering; fee presets are keyed by block target instead.
const ADD_RECIPIENT_ID: &str = "tenshu.vault.send.add-recipient";
const BACK_ID: &str = "tenshu.vault.send.back";
const CLEAR_ID: &str = "tenshu.vault.send.clear";
const PREPARE_ID: &str = "tenshu.vault.send.prepare";
const CHOOSE_COINS_ID: &str = "tenshu.vault.send.choose-coins";

/// What the collapsed Coins section says. Automatic selection keeps the fee
/// and the change low (branch and bound over the confirmed coins); it does
/// not order coins by their recovery dates, so the copy doesn't claim to.
pub(crate) const AUTOMATIC_COINS: &str =
    "Automatic: COINCUBE Desktop picks the coins for this payment.";

/// Fee presets in display order, each with the block target whose estimate it
/// fetches. The target, never the label or position, is the preset's identity.
const FEERATE_PRESETS: [(&str, usize); 3] =
    [("Slow (~4h)", 24), ("Normal (~1h)", 6), ("Fast (~10m)", 1)];

/// A focused preset's ring extends 4px beyond its bounds: a 2px gap, then a
/// 2px stroke. A later preset paints an opaque fill when selected, so presets
/// keep at least that much space between them for the ring to stay visible.
const PRESET_SPACING: u32 = 4;

fn feerate_preset_id(block_target: usize) -> Id {
    Id::from(format!("tenshu.vault.send.feerate-preset.{block_target}"))
}

const FEERATE_CUSTOM_ID: &str = "tenshu.vault.send.feerate-custom";

/// The fourth segment after the presets (W4, master §3 decision 6): it
/// reveals the feerate field, which the presets fill and hide.
fn feerate_custom_button<'a>(selected: bool) -> Element<'a, Message> {
    let label = text(localized_copy("Custom"))
        .size(P2_SIZE)
        .font(if selected {
            font::SEMIBOLD
        } else {
            font::MEDIUM
        });
    Action::new(Id::new(FEERATE_CUSTOM_ID), label, Message::FocusAction)
        .padding([4, 12])
        .style(move |theme, status| preset_style(theme, status, selected))
        .on_press(Message::CreateSpend(CreateSpendMessage::CustomFeerate))
        .into()
}

/// The footer line for a policy that needs more than one key: "This Vault
/// needs 2 of 3 keys. You'll sign, then share the PSBT." `None` for one key.
pub(crate) fn threshold_footer(threshold: usize, keys: usize) -> Option<String> {
    (threshold > 1).then(|| {
        format!(
            "This Vault needs {} of {} keys. You'll sign, then share the PSBT.",
            threshold, keys
        )
    })
}

/// The flow on Vault Send's head rule (the H-Send-Vault board).
pub const SEND_STEPS: [&str; 4] = ["Details", "Review", "Sign", "Sent"];

/// Where a built transaction stands on [`SEND_STEPS`]: Review until it is
/// saved, Sign while it waits for signatures, Sent once it is broadcast.
pub(crate) fn review_step(saved: bool, status: &SpendStatus) -> usize {
    match status {
        SpendStatus::Broadcast | SpendStatus::Spent => 3,
        SpendStatus::Pending | SpendStatus::Deprecated if saved => 2,
        SpendStatus::Pending | SpendStatus::Deprecated => 1,
    }
}

/// A Vault Send head with its steps on the rule.
fn send_head<'a>(
    cache: &Cache,
    title: &'a str,
    step: usize,
) -> coincube_ui::component::tenshu::PageHead<'a, Message> {
    super::sub_page_head(cache, title)
        .rule(coincube_ui::component::tenshu::steps(&SEND_STEPS, step))
}

/// Always-visible review for an MCP-origin construction. Read the actual loaded
/// PSBT/SpendTx, never the assistant's summary. Ordinary spend screens do not use it.
/// Ordinary widgets intentionally avoid the lazy collapsible component here.
pub(crate) fn mcp_prepared_review<'a, R>(
    tx: &SpendTx,
    network: Network,
) -> iced::Element<'a, Message, coincube_ui::theme::Theme, R>
where
    R: iced::advanced::text::Renderer<Font = iced::Font> + 'a,
{
    let unsigned = tx.psbt.inputs.iter().all(|i| {
        i.partial_sigs.is_empty()
            && i.final_script_sig.is_none()
            && i.final_script_witness.is_none()
            && i.tap_key_sig.is_none()
            && i.tap_script_sigs.is_empty()
    });
    let mut content = Column::new()
        .spacing(10)
        .padding(20)
        .width(Length::Fill)
        .push(iced::widget::text(localized_copy("Prepared transaction review")).size(24))
        .push(
            iced::widget::text(if unsigned {
                localized_copy("Unsigned transaction. Your signature is required.")
            } else {
                localized_copy("Signatures present. Review signing status in COINCUBE Desktop.")
            })
            .size(20),
        )
        .push(iced::widget::text(
            localized_copy("This screen cannot sign or broadcast yet. Choose Sign in COINCUBE Desktop to open the ordinary signing controls. A successful preparation may already have reserved a change address."),
        ))
        .push(iced::widget::text(localized_format!(
            "Payment total: {} sats", "Importe total del pago: {} sats",
            tx.spend_amount.to_sat()
        )));
    let input_total = tx.psbt.inputs.iter().try_fold(0u64, |sum, i| {
        sum.checked_add(i.witness_utxo.as_ref()?.value.to_sat())
    });
    content = content
        .push(iced::widget::text(
            input_total
                .map(|v| localized_format!("Input total: {v} sats", "Total de entrada: {v} sats"))
                .unwrap_or_else(|| "Input total unavailable".into()),
        ))
        .push(iced::widget::text(
            tx.fee_amount
                .map(|v| format!("Miner fee: {} sats", v.to_sat()))
                .unwrap_or_else(|| "Miner fee unavailable".into()),
        ));
    let changes: Vec<_> = tx
        .psbt
        .unsigned_tx
        .output
        .iter()
        .enumerate()
        .filter(|(index, _)| tx.change_indexes.contains(index))
        .map(|(_, output)| output)
        .collect();
    let total: u64 = changes.iter().map(|o| o.value.to_sat()).sum();
    content = content.push(
        iced::widget::text(localized_format!(
            "Change returned to this Vault: {total} sats",
            "Cambio devuelto a este Vault: {total} sats"
        ))
        .size(20),
    );
    for output in changes {
        let address = coincube_core::miniscript::bitcoin::Address::from_script(
            &output.script_pubkey,
            network,
        )
        .map(|v| v.to_string())
        .unwrap_or_else(|_| "Address unavailable; do not sign".into());
        content = content
            .push(iced::widget::text(localized_format!(
                "Change output: {} sats",
                "Cambio de salida: sats {}",
                output.value.to_sat()
            )))
            .push(
                iced::widget::text(localized_format!(
                    "Change address: {address}",
                    "Dirección de cambio: {address}"
                ))
                .width(Length::Fill),
            );
    }
    Container::new(content)
        .width(Length::Fill)
        .style(theme::card::simple)
        .into()
}

/// A Fast/Normal/Slow feerate preset button. Renders with an orange outline
/// when its estimate currently fills the feerate field, and is disabled while
/// its own estimate is being fetched.
fn feerate_preset_button<'a>(
    label: &'static str,
    block_target: usize,
    loading_fee_estimate: Option<usize>,
    selected_feerate_target: Option<usize>,
) -> Element<'a, Message> {
    let id = feerate_preset_id(block_target);
    let loading = loading_fee_estimate == Some(block_target);
    let selected = selected_feerate_target == Some(block_target);
    let label = text(label).size(P2_SIZE).font(if selected {
        font::SEMIBOLD
    } else {
        font::MEDIUM
    });
    let content: Element<'a, Message> = if loading {
        Row::new()
            .align_y(Alignment::Center)
            .push(label)
            .push(spinner::typing_text_carousel(
                "...",
                true,
                Duration::from_millis(400),
                |dots| text(dots).size(P2_SIZE),
            ))
            .into()
    } else {
        label.into()
    };
    Action::new(id, content, Message::FocusAction)
        .padding([4, 12])
        .style(move |theme, status| preset_style(theme, status, selected))
        .on_press_maybe((!loading).then_some(Message::CreateSpend(
            CreateSpendMessage::FetchFeeEstimate(block_target),
        )))
        .into()
}

fn preset_style(theme: &theme::Theme, status: ButtonStatus, selected: bool) -> ButtonStyle {
    let shell = &theme.colors.shell;
    let background = if selected {
        shell.selected
    } else if matches!(status, ButtonStatus::Hovered | ButtonStatus::Pressed) {
        shell.hover
    } else {
        Color::TRANSPARENT
    };
    ButtonStyle {
        background: Some(Background::Color(background)),
        text_color: if selected { shell.ink } else { shell.muted },
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Bordered secondary control for Add recipient, Back and Clear.
fn quiet_action<'a>(
    id: &'static str,
    icon: Option<coincube_ui::widget::Text<'a>>,
    label: &'static str,
    press: Message,
) -> Element<'a, Message> {
    Action::new(
        id,
        Row::new()
            .spacing(6)
            .align_y(Alignment::Center)
            .push_maybe(icon.map(|icon| icon.size(P2_SIZE)))
            .push(text(label).size(P2_SIZE).font(font::MEDIUM)),
        Message::FocusAction,
    )
    .padding([6, 12])
    .style(quiet_button_style)
    .on_press(press)
    .into()
}

fn quiet_button_style(theme: &theme::Theme, status: ButtonStatus) -> ButtonStyle {
    let shell = &theme.colors.shell;
    ButtonStyle {
        background: Some(Background::Color(match status {
            ButtonStatus::Hovered | ButtonStatus::Pressed => shell.hover,
            _ => shell.panel,
        })),
        text_color: if matches!(status, ButtonStatus::Disabled) {
            shell.muted
        } else {
            shell.ink
        },
        border: Border {
            color: shell.line,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

/// The footer's primary action. It only dispatches `Generate` when the draft
/// can proceed and no generation is already in flight; otherwise it is
/// disabled, which also removes it from keyboard focus.
fn prepare_action<'a>(can_proceed: bool, generating: bool) -> Element<'a, Message> {
    let label = text(localized_copy("Prepare"))
        .size(P1_SIZE)
        .font(font::MEDIUM);
    let content: Element<'a, Message> = if generating {
        Row::new()
            .spacing(2)
            .align_y(Alignment::Center)
            .push(label)
            .push(spinner::typing_text_carousel(
                "...",
                true,
                Duration::from_millis(400),
                |dots| text(dots).size(P1_SIZE).font(font::MEDIUM),
            ))
            .into()
    } else {
        label.into()
    };
    let style: fn(&theme::Theme, ButtonStatus) -> ButtonStyle = if can_proceed || generating {
        theme::button::primary
    } else {
        theme::button::secondary
    };
    Action::new(PREPARE_ID, content, Message::FocusAction)
        .padding([10, 22])
        .style(style)
        .on_press_maybe(
            (can_proceed && !generating)
                .then_some(Message::CreateSpend(CreateSpendMessage::Generate)),
        )
        .into()
}

fn compose_card_style(theme: &theme::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Background::Color(theme.colors.shell.panel)),
        border: Border {
            color: theme.colors.shell.line,
            width: 1.0,
            radius: 16.0.into(),
        },
        ..Default::default()
    }
}

fn compose_divider<'a>() -> Element<'a, Message> {
    Container::new(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(|theme: &theme::Theme| iced::widget::container::Style {
            background: Some(Background::Color(theme.colors.shell.line)),
            ..Default::default()
        })
        .into()
}

fn compose_section<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Container::new(content)
        .padding([20, 24])
        .width(Length::Fill)
        .into()
}

fn section_label<'a>(label: &'static str) -> coincube_ui::widget::Text<'a> {
    text(label).size(P2_SIZE).font(font::SEMIBOLD)
}

#[allow(clippy::too_many_arguments)]
pub fn create_spend_tx<'a>(
    balance: &'a Amount,
    unconfirmed_balance: &'a Amount,
    menu: &'a Menu,
    cache: &'a Cache,
    fiat_converter: Option<&FiatAmountConverter>,
    recipients: Vec<Element<'a, Message>>,
    is_valid: bool,
    duplicate: bool,
    timelock: Option<u16>,
    recovery_timelock: Option<u16>,
    coins: &[(Coin, bool)],
    coins_labels: &'a HashMap<String, String>,
    batch_label: &form::Value<String>,
    amount_left: Option<&Amount>,
    draft_warning: Option<String>,
    blocking_warning: Option<String>,
    feerate: &form::Value<String>,
    fee_amount: Option<&Amount>,
    sync_status: &SyncStatus,
    is_first_step: bool,
    loading_fee_estimate: Option<usize>,
    selected_feerate_target: Option<usize>,
    custom_feerate: bool,
    threshold: Option<(usize, usize)>,
    coins_shown: bool,
    generating: bool,
    bitcoin_unit: BitcoinDisplayUnit,
    max_under_dust: bool,
) -> Element<'a, Message> {
    let is_self_send = recipients.is_empty();
    let is_batch = recipients.len() > 1;
    let recipient_count = recipients.len();
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(*balance));
    let fiat_unconfirmed = fiat_converter.map(|c| c.convert(*unconfirmed_balance));
    // Keep actual balances for draft validation; mask only passive context.
    let mask_amounts = cache.balance_amounts_masked();
    let unconfirmed_display: Element<'a, Message> = if mask_amounts {
        text("••••••").size(H3_SIZE).into()
    } else {
        unconfirmed_amount_with_size_and_unit(unconfirmed_balance, H3_SIZE, bitcoin_unit).into()
    };

    let can_proceed = is_valid
        && blocking_warning.is_none()
        && !duplicate
        && (is_self_send
            || recovery_timelock.is_some()
            || Some(&Amount::from_sat(0)) == amount_left);

    // A Branta tamper result or a BIP21 amount mismatch blocks the send with
    // the draft fully selected (`amount_left == Some(0)`), so it must win over
    // every other hint.
    let disabled_hint: Option<String> = if blocking_warning.is_some() {
        blocking_warning
    } else if can_proceed {
        None
    } else if feerate.value.is_empty() {
        Some("Please set a feerate.".to_string())
    } else if !feerate.valid {
        Some("Invalid feerate.".to_string())
    } else if duplicate {
        Some("Duplicate recipient address.".to_string())
    } else if coins.iter().all(|(_, selected)| !selected) {
        Some("Please select at least one coin.".to_string())
    } else if !is_self_send && recovery_timelock.is_none() {
        match amount_left {
            // `amount_left` is only `None` once the recipient and amount are
            // filled in but the background draft could not produce a result.
            // If that draft failed with an error, show it rather than the
            // generic "enter recipient/amount" hint, which is misleading once
            // the fields are actually populated.
            None => Some(
                draft_warning.unwrap_or_else(|| "Enter recipient address and amount.".to_string()),
            ),
            Some(amount) if amount.to_sat() != 0 => Some("Insufficient funds.".to_string()),
            _ => None,
        }
    } else {
        None
    };

    let title = if recovery_timelock.is_some() {
        localized_copy("Recovery")
    } else if is_self_send {
        localized_copy("Self-transfer")
    } else {
        localized_copy("Send")
    };
    let balance_context = Column::new()
        .spacing(10)
        .push(
            Column::new()
                .spacing(8)
                .push(caption(localized_copy("Available in Vault")).style(theme::text::secondary))
                .push(
                    if mask_amounts {
                        Column::new().push(text("••••••").size(H2_SIZE))
                    } else if sync_status.is_synced() {
                        Column::new()
                            .spacing(5)
                            .push(amount_with_size_and_unit(balance, H2_SIZE, bitcoin_unit))
                            .push_maybe(fiat_balance.map(|fiat| {
                                fiat.to_text().size(P2_SIZE).style(theme::text::secondary)
                            }))
                    } else {
                        Column::new().push(Row::new().push(spinner::Carousel::new(
                            Duration::from_millis(1000),
                            vec![
                                amount_with_size_and_unit(balance, H2_SIZE, bitcoin_unit),
                                amount_with_size_colors_and_unit(
                                    balance,
                                    H2_SIZE,
                                    color::GREY_3,
                                    Some(color::GREY_3),
                                    bitcoin_unit,
                                ),
                            ],
                        )))
                    }
                    .wrap(),
                )
                .push_maybe(if !sync_status.is_synced() {
                    Some(
                        Row::new()
                            .push(
                                match sync_status {
                                    SyncStatus::BlockchainSync(progress) => {
                                        text(localized_format!(
                                            "Syncing blockchain ({:.2}%)",
                                            "Sincing blockchain ({:.2}%)",
                                            crate::utils::syncing_percent(*progress)
                                        ))
                                    }
                                    SyncStatus::WalletFullScan { .. } => {
                                        text(localized_copy("Syncing"))
                                    }
                                    SyncStatus::SyncFailing { .. } => text(localized_copy(
                                        "Wallet sync is failing; the balance may be out of date",
                                    )),
                                    _ => text(localized_copy("Checking for new transactions")),
                                }
                                .style(theme::text::secondary),
                            )
                            .push(spinner::typing_text_carousel(
                                "...",
                                true,
                                Duration::from_millis(2000),
                                |content| text(content).style(theme::text::secondary),
                            )),
                    )
                } else {
                    None
                }),
        )
        .push_maybe(
            if unconfirmed_balance.to_sat() != 0 && sync_status.is_synced() {
                Some(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(text("+").size(H3_SIZE).style(theme::text::secondary))
                        .push(unconfirmed_display)
                        .push(
                            text("unconfirmed")
                                .size(H3_SIZE)
                                .style(theme::text::secondary),
                        )
                        .push_maybe(fiat_unconfirmed.filter(|_| !mask_amounts).map(|fiat| {
                            Row::new()
                                .align_y(Alignment::Center)
                                .push(Space::new().width(10)) // total spacing = 20 including row spacing
                                .push(fiat.to_text().size(H4_SIZE).color(color::GREY_3))
                        }))
                        .wrap(),
                )
            } else {
                None
            },
        );

    // Every slot below is always present, so a warning or batch label
    // appearing does not shift the widget state (focus, text cursor) of the
    // controls that follow it.
    let recipient_groups = recipients.into_iter().enumerate().fold(
        Column::new().spacing(16),
        |column, (index, recipient)| {
            if index == 0 {
                column.push(recipient)
            } else {
                column.push(compose_divider()).push(recipient)
            }
        },
    );
    let recipients_block: Element<'a, Message> = if is_self_send {
        // An empty Column has zero rendered height but a non-void size hint,
        // so the fee section keeps its position in Iced's widget tree.
        Column::new().width(Length::Fill).into()
    } else {
        Column::new()
            .push(compose_divider())
            .push(compose_section(
                Column::new()
                    .spacing(8)
                    .push(section_label(if recovery_timelock.is_some() {
                        "Recipient"
                    } else {
                        "Recipients"
                    }))
                    // Keep this slot in the Column's tree when the label is absent.
                    // A zero-height Space is discarded by Column::push and shifts
                    // recipient input state when a second recipient is added.
                    .push(
                        Column::new()
                            .width(Length::Fill)
                            .push_maybe(is_batch.then(|| {
                                form::Form::new(
                                    coincube_ui::locale::copy("Batch label"),
                                    batch_label,
                                    |s| {
                                        Message::CreateSpend(CreateSpendMessage::BatchLabelEdited(
                                            s,
                                        ))
                                    },
                                )
                                .warning(localized_copy(
                                    "Invalid label length, cannot be superior to 100",
                                ))
                                .size(P1_SIZE)
                                .padding(10)
                            })),
                    )
                    .push(recipient_groups)
                    .push(
                        Row::new()
                            .spacing(12)
                            .align_y(Alignment::Center)
                            .push(if duplicate {
                                Element::from(
                                    text(localized_copy("Two payment addresses are the same"))
                                        .size(P2_SIZE)
                                        .style(theme::text::warning),
                                )
                            } else {
                                // Keep the warning slot in the Row even when
                                // it draws nothing. Fixed(0) is dropped by
                                // Row::push and would move Add recipient's state.
                                Row::new().into()
                            })
                            .push(Space::new().width(Length::Fill))
                            // Recipients cannot be added for recovery (exactly one recipient).
                            .push_maybe(recovery_timelock.is_none().then(|| {
                                quiet_action(
                                    ADD_RECIPIENT_ID,
                                    Some(icon::plus_icon()),
                                    coincube_ui::locale::copy("Add recipient"),
                                    Message::CreateSpend(CreateSpendMessage::AddRecipient),
                                )
                            })),
                    ),
            ))
            .into()
    };

    let fee_section = Column::new()
        .spacing(10)
        .push(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(
                    section_label(coincube_ui::locale::copy("Fee rate (sat/vB)"))
                        .width(Length::Fill),
                )
                .push(
                    Container::new(
                        Row::with_children(
                            FEERATE_PRESETS
                                .iter()
                                .map(|&(label, block_target)| {
                                    feerate_preset_button(
                                        label,
                                        block_target,
                                        loading_fee_estimate,
                                        selected_feerate_target.filter(|_| !custom_feerate),
                                    )
                                })
                                .chain(std::iter::once(feerate_custom_button(custom_feerate))),
                        )
                        .spacing(PRESET_SPACING),
                    )
                    .padding(2)
                    .style(|theme: &theme::Theme| {
                        iced::widget::container::Style {
                            background: Some(Background::Color(theme.colors.shell.background)),
                            border: Border {
                                color: theme.colors.shell.line,
                                width: 1.0,
                                radius: 10.0.into(),
                            },
                            ..Default::default()
                        }
                    }),
                ),
        )
        // Custom reveals the field; a preset fills it and shows its rate.
        .push(if custom_feerate {
            Element::from(
                Container::new(
                    form::Form::new_trimmed(
                        localized_copy("42 (in sats/vbyte)"),
                        feerate,
                        move |msg| Message::CreateSpend(CreateSpendMessage::FeerateEdited(msg)),
                    )
                    .warning(localized_copy(
                        "Feerate must be an integer less than or equal to 1000 sats/vbyte",
                    ))
                    .size(P1_SIZE)
                    .padding(10),
                )
                .width(Length::Fixed(240.0)),
            )
        } else {
            text(
                if loading_fee_estimate.is_some() || feerate.value.is_empty() {
                    "Fetching the fee estimate…".to_string()
                } else {
                    format!("{} sat/vB", feerate.value)
                },
            )
            .size(P2_SIZE)
            .style(theme::text::secondary)
            .into()
        })
        .push_maybe(fee_amount.map(|fee| {
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(
                    p2_regular(localized_copy("Fee for this transaction"))
                        .style(theme::text::secondary),
                )
                .push(amount_with_size(fee, P1_SIZE))
                .push(fiat_converter.map(|conv| {
                    conv.convert(*fee)
                        .to_text()
                        .size(P2_SIZE)
                        .style(theme::text::secondary)
                }))
                .wrap()
        }));

    // Until the user chooses coins, the draft picks them: the section says
    // so and offers Choose coins, which opens the list below.
    let automatic_coins = Column::new()
        .spacing(10)
        .push(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(section_label(coincube_ui::locale::copy("Coins")).width(Length::Fill))
                .push(quiet_action(
                    CHOOSE_COINS_ID,
                    None,
                    coincube_ui::locale::copy("Choose coins"),
                    Message::CreateSpend(CreateSpendMessage::ChooseCoins),
                )),
        )
        .push(
            text(AUTOMATIC_COINS)
                .size(P2_SIZE)
                .style(theme::text::secondary),
        );
    let coins_section = Column::new()
        .spacing(10)
        .push(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(
                    section_label(coincube_ui::locale::copy("Coin selection")).width(Length::Fill),
                )
                .push(if is_self_send || recovery_timelock.is_some() {
                    Row::new()
                        .spacing(5)
                        .push(amount_with_size(
                            &Amount::from_sat(
                                coins
                                    .iter()
                                    .filter_map(|(coin, selected)| {
                                        if *selected {
                                            Some(coin.amount.to_sat())
                                        } else {
                                            None
                                        }
                                    })
                                    .sum(),
                            ),
                            P2_SIZE,
                        ))
                        .push(p2_regular("selected").style(theme::text::secondary))
                } else if let Some(amount_left) = amount_left {
                    if amount_left.to_sat() == 0 && !is_valid {
                        // If amount left is set, the current configuration must be redraftable.
                        // If it's not valid, either no coins are selected or there's a recipient
                        // with max selected and invalid amount.
                        if coins.iter().all(|(_, selected)| !selected) {
                            // This can happen if we have a single recipient
                            // and it has the max selected.
                            Row::new().push(
                                text(localized_copy("Select at least one coin."))
                                    .style(theme::text::secondary),
                            )
                        } else {
                            // There must be a recipient with max selected and value 0.
                            Row::new().push(
                                text(localized_copy("Check max amount for recipient."))
                                    .style(theme::text::secondary),
                            )
                        }
                    } else {
                        Row::new()
                            .spacing(5)
                            .push(amount_with_size(amount_left, P2_SIZE))
                            .push(
                                p2_regular(localized_copy("left to select"))
                                    .style(theme::text::secondary),
                            )
                    }
                } else {
                    Row::new().push(
                        text(if feerate.value.is_empty() || !feerate.valid {
                            localized_copy("Feerate needs to be set.")
                        } else if !max_under_dust {
                            localized_copy("Add recipient details.")
                        } else {
                            localized_copy("Select or add more funds.")
                        })
                        .style(theme::text::secondary),
                    )
                }),
        )
        .push(
            Container::new(
                scrollable(coins.iter().enumerate().fold(
                    Column::new().spacing(0),
                    |col, (i, (coin, selected))| {
                        let col = if i == 0 {
                            col
                        } else {
                            col.push(compose_divider())
                        };
                        col.push(coin_list_view(
                            i,
                            coin,
                            coins_labels,
                            timelock,
                            cache.blockheight() as u32,
                            *selected,
                            bitcoin_unit,
                        ))
                    },
                ))
                // Iced's default 10px scrollbar floats over its content.
                // Embed it with a 2px gap so the coin unit remains legible.
                .spacing(2.0),
            )
            .max_height(300),
        );

    let hint: Element<'a, Message> = if let Some(hint) = disabled_hint {
        Row::new()
            .spacing(6)
            .align_y(Alignment::Center)
            .push(
                icon::warning_icon()
                    .size(P2_SIZE)
                    .style(theme::text::warning),
            )
            .push(
                text(coincube_ui::locale::notice(hint))
                    .size(P2_SIZE)
                    .style(theme::text::warning),
            )
            .into()
    } else if generating {
        text(localized_copy("Preparing the unsigned transaction."))
            .size(P2_SIZE)
            .style(theme::text::secondary)
            .into()
    } else {
        text(localized_copy("Builds an unsigned transaction to review."))
            .size(P2_SIZE)
            .style(theme::text::secondary)
            .into()
    };

    let threshold_line = threshold.and_then(|(threshold, keys)| threshold_footer(threshold, keys));
    let footer = Container::new(
        Column::new()
            .spacing(10)
            .push(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push_maybe((!is_first_step).then(|| {
                        quiet_action(
                            BACK_ID,
                            Some(icon::previous_icon()),
                            coincube_ui::locale::copy("Back"),
                            Message::Previous,
                        )
                    }))
                    .push(Container::new(hint).width(Length::Fill))
                    .push(quiet_action(
                        CLEAR_ID,
                        None,
                        coincube_ui::locale::copy("Clear"),
                        Message::CreateSpend(CreateSpendMessage::Clear),
                    ))
                    .push(prepare_action(can_proceed, generating)),
            )
            .push_maybe(
                threshold_line.map(|line| text(line).size(P2_SIZE).style(theme::text::secondary)),
            ),
    )
    .padding([16, 24])
    .width(Length::Fill);

    let compose_card = Container::new(
        Column::new()
            .push(
                Container::new(
                    text(if recovery_timelock.is_some() {
                        localized_copy("Recovery details")
                    } else if is_self_send {
                        localized_copy("Transfer details")
                    } else {
                        localized_copy("Payment details")
                    })
                    .size(H5_SIZE)
                    .font(font::SEMIBOLD),
                )
                .padding([18, 24])
                .width(Length::Fill),
            )
            .push(recipients_block)
            .push(compose_divider())
            .push(compose_section(fee_section))
            .push(compose_divider())
            .push(if coins_shown {
                compose_section(coins_section)
            } else {
                compose_section(automatic_coins)
            })
            .push(compose_divider())
            .push(footer),
    )
    .width(Length::Fill)
    .style(compose_card_style);

    dashboard_headed(
        menu,
        cache,
        send_head(cache, title, 0),
        // The balance sits in the rail beside the form (it was a card above
        // it), with what this spend has set so far.
        payment_columns(
            compose_card,
            rail_card(
                Column::new()
                    .spacing(20)
                    .push(balance_context)
                    .push(iced::widget::rule::horizontal(1))
                    .push(spend_summary(
                        recipient_count,
                        recovery_timelock.is_some(),
                        feerate,
                        custom_feerate,
                        coins_shown.then(|| coins.iter().filter(|(_, selected)| *selected).count()),
                        fee_amount,
                        mask_amounts,
                        bitcoin_unit,
                    )),
            ),
        ),
    )
}

/// "This spend": the rail's summary of what the form has set so far. Only
/// what the draft already knows; the fee shows once a draft has one, masked
/// with the balances.
#[allow(clippy::too_many_arguments)]
fn spend_summary<'a>(
    recipients: usize,
    recovery: bool,
    feerate: &form::Value<String>,
    custom_feerate: bool,
    selected_coins: Option<usize>,
    fee: Option<&Amount>,
    mask_amounts: bool,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let line = |name: &'static str, value: String| {
        Row::new()
            .spacing(12)
            .push(
                p2_regular(localized_copy(name))
                    .style(theme::text::secondary)
                    .width(Length::Fill),
            )
            .push(p2_regular(value))
    };
    let rate = if feerate.value.is_empty() || !feerate.valid {
        localized_copy("Not set").to_owned()
    } else if custom_feerate {
        format!("{} sat/vB · {}", feerate.value, localized_copy("Custom"))
    } else {
        format!("{} sat/vB", feerate.value)
    };
    let coins = match selected_coins {
        None => localized_copy("Automatic").to_owned(),
        Some(1) => localized_copy("1 selected").to_owned(),
        Some(n) => format!("{n} {}", localized_copy("selected")),
    };
    Column::new()
        .spacing(10)
        .push(p1_bold(localized_copy(if recovery {
            "This recovery"
        } else {
            "This spend"
        })))
        .push_maybe(
            (!recovery && recipients > 0).then(|| line("Recipients", recipients.to_string())),
        )
        .push(line("Fee rate", rate))
        .push(line("Coins", coins))
        // The background draft sets the fee as soon as the form is complete,
        // before Prepare; masked balances hide it without hiding that it exists.
        .push(line(
            "Fee",
            match fee {
                None => localized_copy("Not set").to_owned(),
                Some(_) if mask_amounts => "••••••".to_owned(),
                Some(fee) => format!(
                    "{} {}",
                    fee.to_formatted_string_with_unit(bitcoin_unit),
                    if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                        "BTC"
                    } else {
                        "sats"
                    }
                ),
            },
        ))
        .into()
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
    dust_warning: &'a Option<String>,
    max_estimated_amount: Option<Amount>,
    bitcoin_unit: BitcoinDisplayUnit,
    // Flag the amount input red when the draft can't be funded, mirroring the
    // "Insufficient funds." disabled-action hint.
    insufficient: bool,
) -> Element<'a, CreateSpendMessage> {
    // When a send-max output is under the dust limit the amount field is cleared
    // and an estimate is shown instead, so use that for any fiat conversion.
    let btc_amt = if dust_warning.is_some() {
        max_estimated_amount
    } else {
        match bitcoin_unit {
            BitcoinDisplayUnit::BTC => {
                Amount::from_str_in(&amount.value, Denomination::Bitcoin).ok()
            }
            BitcoinDisplayUnit::Sats => amount
                .value
                .replace(' ', "")
                .parse::<u64>()
                .ok()
                .map(Amount::from_sat),
        }
    };

    // Show the dust warning (red) if any, otherwise any amount warning (orange).
    let amount_warning_row = Row::new()
        .push(Space::new().width(Length::Fixed(20.0)))
        .push(
            dust_warning
                .as_ref()
                .map(|w| caption(w).color(color::RED))
                .or_else(|| amount.warning.map(|w| caption(w).color(color::ORANGE))),
        );

    Container::new(
        Column::new()
            .spacing(8)
            .push(
                Column::new()
                    .spacing(4)
                    .push(
                        Row::new()
                            .spacing(12)
                            .align_y(Alignment::Center)
                            .push(p1_bold(localized_copy("Address")))
                            .push(
                                text(localized_format!("Recipient {}", "Destinatario {}", index + 1))
                                    .size(P2_SIZE)
                                    .font(font::SEMIBOLD)
                                    .style(theme::text::secondary),
                            )
                            .push(Space::new().width(Length::Fill))
                            // Recipient for recovery cannot be deleted.
                            .push_maybe((!is_recovery).then(|| {
                                Button::new(text(localized_copy("Remove")).size(P2_SIZE))
                                    .style(theme::button::transparent)
                                    .on_press(CreateSpendMessage::DeleteRecipient(index))
                                    .width(Length::Shrink)
                            })),
                    )
                    .push(
                        form::Form::new_trimmed(localized_copy("Address"), address, move |msg| {
                            CreateSpendMessage::RecipientEdited(index, "address", msg)
                        })
                        .warning(localized_copy("Invalid address (maybe it is for another network?)"))
                        .size(P1_SIZE)
                        .padding(10),
                    ),
            )
            .push(
                Column::new()
                    .spacing(4)
                    .push(p1_bold(localized_copy("Description")))
                    .push(
                        form::Form::new(coincube_ui::locale::copy("Payment label"), label, move |msg| {
                            CreateSpendMessage::RecipientEdited(index, "label", msg)
                        })
                        .warning(localized_copy("Label length is too long (> 100 char)"))
                        .size(P1_SIZE)
                        .padding(10),
                    ),
            )
            .push(
                Column::new().spacing(4)
                    .push(p1_bold(match bitcoin_unit {
                        BitcoinDisplayUnit::BTC => localized_copy("Amount BTC"),
                        BitcoinDisplayUnit::Sats => localized_copy("Amount (sats)"),
                    }))
                    .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(5)
                            .push(if is_max_selected {
                                let amount_txt = btc_amt
                                    .map(|a| a.to_formatted_string_with_unit(bitcoin_unit))
                                    .unwrap_or(amount.value.clone());
                                Container::new(
                                    text(amount_txt).size(P1_SIZE).style(theme::text::secondary),
                                )
                                .width(Length::Fill)
                            } else {

                                let form = match bitcoin_unit {
                                    BitcoinDisplayUnit::BTC => {
                                        form::Form::new_amount_btc(coincube_ui::locale::copy("0.001 (in BTC)"), amount, move |msg| {
                                            CreateSpendMessage::RecipientEdited(index, "amount", msg)
                                        })
                                        .warning(localized_copy("Invalid amount. (Note amounts lower than 0.000005 BTC are invalid.)"))
                                    }
                                    BitcoinDisplayUnit::Sats => {
                                        form::Form::new_amount_sats(coincube_ui::locale::copy("100 000 (in sats)"), amount, move |msg| {
                                            CreateSpendMessage::RecipientEdited(index, "amount", msg)
                                        })
                                        .warning(localized_copy("Invalid amount. (Note amounts lower than 500 sats are invalid.)"))
                                    }
                                };

                                form
                                .invalid(insufficient)
                                .size(P1_SIZE)
                                .padding(10)
                                .into_container()
                            })
                            .push(fiat_converter.map(|conv| {
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .spacing(5)
                                    .push(Space::new().width(Length::Fixed(20.0))) // add some space between BTC and fiat amounts
                                    // Don't show an empty fiat label when max is selected but no amount is known yet.
                                    .push((!is_max_selected || btc_amt.is_some())
                                        .then_some(p1_bold(format!("{}", conv.currency()))))
                                    .push(Space::new().width(Length::Fixed(5.0)))
                                    .push(if is_max_selected {
                                        // fiat is derived from the btc amount; hide it entirely when unknown.
                                        btc_amt.map(|a| {
                                            let fiat_from_btc = conv.convert(a).to_formatted_string();
                                            Container::new(
                                                text(fiat_from_btc)
                                                    .size(P1_SIZE)
                                                    .style(theme::text::secondary),
                                            )
                                            .width(Length::Fill)
                                        })
                                    } else {
                                        let conv = *conv;
                                        // The particular form shown depends on whether the user has entered a fiat amount or
                                        // if we are instead converting the BTC amount.
                                        let fiat_form = if let Some(val) = fiat_form_value {
                                            val
                                        } else if let Some(btc_amt) = btc_amt {
                                            let fa = conv.convert(btc_amt);
                                            &form::Value {
                                                value: fa.to_rounded_string(), // required decimal places for currency
                                                warning: None,
                                                valid: true,
                                            }
                                        } else {
                                            &form::Value::default()
                                        };
                                        Some(
                                            form::Form::new_trimmed(
                                                &localized_format!("Enter amount in {}", "Introduce el importe en {}", conv.currency()),
                                                fiat_form,
                                                move |msg| {
                                                    CreateSpendMessage::RecipientFiatAmountEdited(
                                                        index, msg, conv,
                                                    )
                                                },
                                            )
                                            .size(P1_SIZE)
                                            .padding(10)
                                            .into_container(),
                                        )
                                    })
                                    .push((!is_max_selected || btc_amt.is_some()).then_some(
                                        tooltip::Tooltip::new(
                                            icon::tooltip_icon(),
                                            conv.to_container_summary(),
                                            tooltip::Position::Bottom,
                                        ),
                                    ))
                                    .push(Space::new().width(Length::Fixed(10.0)))
                            })),
                    )
                    .push(
                        // The MAX option cannot be edited for recovery recipients.
                        (!is_recovery).then_some(tooltip::Tooltip::new(
                            checkbox(is_max_selected).label("MAX")
                                .on_toggle(move |_| CreateSpendMessage::SendMaxToRecipient(index)),
                            // Add spaces at end so that text is padded at screen edge.
                            "Total amount remaining after paying fee and any other recipients     ",
                            tooltip::Position::Bottom,
                        )),
                    )
                    .width(Length::Fill),
                )
                .push(amount_warning_row),
            ),
    )
    .width(Length::Fill)
    .into()
}

fn coin_list_view<'a>(
    i: usize,
    coin: &Coin,
    coins_labels: &'a HashMap<String, String>,
    timelock: Option<u16>,
    blockheight: u32,
    selected: bool,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let outpoint = coin.outpoint.to_string();
    let identity: Element<'a, Message> = if let Some(label) = coins_labels.get(&outpoint) {
        p1_regular(label)
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
            .into()
    } else if let Some(label) = coins_labels.get(&coin.outpoint.txid.to_string()) {
        Row::new()
            .spacing(5)
            .push(
                // It is not possible to know if a coin is a
                // change coin or not so for now, From is
                // enough
                p1_regular(localized_copy("From")).style(theme::text::secondary),
            )
            .push(p1_regular(label).wrapping(iced::widget::text::Wrapping::WordOrGlyph))
            .into()
    } else {
        let txid = coin.outpoint.txid.to_string();
        p1_regular(format!(
            "{}…{}:{}",
            &txid[..8],
            &txid[txid.len() - 8..],
            coin.outpoint.vout
        ))
        .style(theme::text::secondary)
        .into()
    };
    let row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(
            checkbox(selected)
                .on_toggle(move |_| Message::CreateSpend(CreateSpendMessage::SelectCoin(i))),
        )
        .push(Container::new(identity).width(Length::Fill))
        .push(if coin.spend_info.is_some() {
            Some(badge::spent())
        } else if coin.block_height.is_none() {
            Some(badge::unconfirmed())
        } else {
            // An imported wallet has no recovery clock to count down.
            timelock.map(|timelock| {
                let seq = remaining_sequence(coin, blockheight, timelock);
                coins::coin_sequence_label(seq, timelock as u32)
            })
        })
        .push(
            Container::new(amount_with_unit(&coin.amount, bitcoin_unit))
                // BTC uses grouped decimal text plus a separate unit. Let the
                // amount take its measured width before the identity fills the
                // remainder, so the coin scroll does not clip the unit.
                .width(Length::Shrink)
                .align_x(iced::alignment::Horizontal::Right),
        );
    Container::new(row)
        .padding([10, 8])
        .width(Length::Fill)
        .style(move |theme: &theme::Theme| iced::widget::container::Style {
            background: selected.then_some(Background::Color(theme.colors.shell.hover)),
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::menu::VaultSubMenu;
    use crate::{
        app::cache::FiatPriceRequest,
        services::fiat::{Currency, PriceSource},
    };
    use coincube_core::miniscript::bitcoin::{bip32::ChildNumber, Address, OutPoint, Txid};
    use coincube_ui::theme::palette::ThemeMode;
    use iced::advanced::{
        layout,
        renderer::Headless,
        text::paragraph::{Paragraph, Plain},
        widget::{
            operation::{self, focusable, Focusable, Scrollable, TextInput},
            Operation, Tree,
        },
        Layout,
    };
    use iced::{keyboard, mouse, Event, Rectangle, Size, Vector};
    use std::str::FromStr;

    // BIP-173 test vectors: synthetic, not anyone's wallet.
    const ADDRESS_A: &str = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
    const ADDRESS_B: &str = "bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3";
    const WIDE: (u32, u32) = (1440, 900);
    const COMPACT: (u32, u32) = (1024, 768);
    /// Tall enough that every control is inside the shell's content viewport,
    /// so a pointer press lands on the control rather than the scroll edge.
    const TALL: (u32, u32) = (1440, 2400);

    fn value(value: &str, valid: bool) -> form::Value<String> {
        form::Value {
            value: value.to_string(),
            warning: None,
            valid,
        }
    }

    fn coin(index: u32, sats: u64) -> Coin {
        Coin {
            amount: Amount::from_sat(sats),
            outpoint: OutPoint::new(Txid::from_str(&format!("{:064x}", index + 1)).unwrap(), 0),
            address: Address::from_str(ADDRESS_A).unwrap().assume_checked(),
            block_height: Some(100),
            derivation_index: ChildNumber::from_normal_idx(index).unwrap(),
            spend_info: None,
            is_immature: false,
            is_change: false,
            is_from_self: false,
        }
    }

    struct Fixture {
        balance: Amount,
        unconfirmed: Amount,
        menu: Menu,
        cache: Cache,
        coins: Vec<Coin>,
        coins_labels: HashMap<String, String>,
        batch_label: form::Value<String>,
        addresses: Vec<form::Value<String>>,
        amounts: Vec<form::Value<String>>,
        labels: Vec<form::Value<String>>,
        fiat_amounts: Vec<form::Value<String>>,
        converter: FiatAmountConverter,
        dust_warning: Option<String>,
        fee: Amount,
        sync_status: SyncStatus,
    }

    impl Fixture {
        fn new(theme_mode: ThemeMode, coin_count: u32) -> Self {
            Self {
                balance: Amount::from_sat(250_000),
                unconfirmed: Amount::from_sat(0),
                menu: Menu::Vault(VaultSubMenu::Send),
                cache: Cache {
                    cube_name: "Synthetic Cube".into(),
                    has_vault: true,
                    theme_mode,
                    ..Default::default()
                },
                coins: (0..coin_count)
                    .map(|i| coin(i, 50_000 + i as u64))
                    .collect(),
                coins_labels: HashMap::new(),
                batch_label: value("", true),
                addresses: vec![value(ADDRESS_A, true), value(ADDRESS_B, true)],
                amounts: vec![value("40000", true), value("20000", true)],
                labels: vec![value("", true), value("", true)],
                fiat_amounts: vec![value("20.00", true), value("10.00", true)],
                converter: FiatAmountConverter::new(
                    50_000.0,
                    None,
                    FiatPriceRequest::new(PriceSource::Coincube, Currency::USD),
                )
                .unwrap(),
                dust_warning: None,
                fee: Amount::from_sat(1_234),
                sync_status: SyncStatus::Synced,
            }
        }
    }

    /// Inputs that vary between compose states. Everything else comes from
    /// the synthetic fixture.
    #[derive(Clone)]
    struct Scenario {
        recipients: usize,
        is_valid: bool,
        duplicate: bool,
        recovery: bool,
        amount_left: Option<u64>,
        draft_warning: Option<&'static str>,
        blocking_warning: Option<&'static str>,
        feerate: (&'static str, bool),
        fee: bool,
        first_step: bool,
        loading: Option<usize>,
        selected: Option<usize>,
        custom: bool,
        threshold: Option<(usize, usize)>,
        /// The coin list is open (Choose coins, or a selection already made).
        coins_shown: bool,
        generating: bool,
        max_under_dust: bool,
        coins_selected: bool,
        unit: BitcoinDisplayUnit,
        fiat: bool,
        fiat_editing: bool,
        max_selected: bool,
        max_estimated: Option<u64>,
        insufficient: bool,
    }

    impl Scenario {
        fn valid_send() -> Self {
            Self {
                recipients: 1,
                is_valid: true,
                duplicate: false,
                recovery: false,
                amount_left: Some(0),
                draft_warning: None,
                blocking_warning: None,
                feerate: ("5", true),
                fee: false,
                first_step: true,
                loading: None,
                selected: None,
                custom: false,
                threshold: None,
                coins_shown: true,
                generating: false,
                max_under_dust: false,
                coins_selected: true,
                unit: BitcoinDisplayUnit::Sats,
                fiat: false,
                fiat_editing: false,
                max_selected: false,
                max_estimated: None,
                insufficient: false,
            }
        }
    }

    /// The production presenter, fed the same way `step.rs` feeds it: real
    /// `recipient_view` rows mapped into `Message::CreateSpend`.
    fn compose<'a>(fixture: &'a Fixture, s: &Scenario) -> Element<'a, Message> {
        let coins: Vec<(Coin, bool)> = fixture
            .coins
            .iter()
            .map(|coin| (coin.clone(), s.coins_selected))
            .collect();
        let feerate = value(s.feerate.0, s.feerate.1);
        let amount_left = s.amount_left.map(Amount::from_sat);
        let recipients = (0..s.recipients)
            .map(|i| {
                // A duplicate pair reuses the first address.
                let address = if s.duplicate { 0 } else { i };
                recipient_view(
                    i,
                    &fixture.addresses[address],
                    &fixture.amounts[i],
                    s.fiat_editing.then_some(&fixture.fiat_amounts[i]),
                    s.fiat.then_some(&fixture.converter),
                    &fixture.labels[i],
                    s.max_selected,
                    s.recovery,
                    &fixture.dust_warning,
                    s.max_estimated.map(Amount::from_sat),
                    s.unit,
                    s.insufficient,
                )
                .map(Message::CreateSpend)
            })
            .collect();
        create_spend_tx(
            &fixture.balance,
            &fixture.unconfirmed,
            &fixture.menu,
            &fixture.cache,
            s.fiat.then_some(&fixture.converter),
            recipients,
            s.is_valid,
            s.duplicate,
            Some(144),
            s.recovery.then_some(52_560),
            &coins,
            &fixture.coins_labels,
            &fixture.batch_label,
            amount_left.as_ref(),
            s.draft_warning.map(String::from),
            s.blocking_warning.map(String::from),
            &feerate,
            s.fee.then_some(&fixture.fee),
            &fixture.sync_status,
            s.first_step,
            s.loading,
            s.selected,
            fee_is_custom(s),
            s.threshold,
            s.coins_shown,
            s.generating,
            s.unit,
            s.max_under_dust,
        )
    }

    /// As `DefineSpend::custom_feerate_shown`: Custom chosen, or no preset
    /// filling the field.
    fn fee_is_custom(s: &Scenario) -> bool {
        s.custom || (s.selected.is_none() && s.loading.is_none())
    }

    #[derive(Debug, Clone)]
    struct FocusEntry {
        id: Option<Id>,
        bounds: Rectangle,
        focused: bool,
        /// Visible text (value or placeholder) when the target is a text input.
        input: Option<String>,
    }

    #[derive(Default)]
    struct Probe {
        text: Vec<(String, Rectangle)>,
        focus: Vec<FocusEntry>,
        /// Text inputs as reported, joined to `focus` by ID and bounds after
        /// traversal: Iced's own TextInput reports `text_input` before
        /// `focusable`, the coincube-ui input used by `form::Form` the reverse.
        text_inputs: Vec<(Option<Id>, Rectangle, String)>,
        /// (viewport height, content height) of every scrollable.
        scrollables: Vec<(f32, f32)>,
        scrollable_bounds: Vec<Rectangle>,
    }

    impl Operation for Probe {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn text(&mut self, _id: Option<&Id>, bounds: Rectangle, value: &str) {
            self.text.push((value.to_owned(), bounds));
        }

        fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
            self.focus.push(FocusEntry {
                id: id.cloned(),
                bounds,
                focused: state.is_focused(),
                input: None,
            });
        }

        fn text_input(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn TextInput) {
            self.text_inputs
                .push((id.cloned(), bounds, state.text().to_owned()));
        }

        fn scrollable(
            &mut self,
            _id: Option<&Id>,
            bounds: Rectangle,
            content_bounds: Rectangle,
            _translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            self.scrollables
                .push((bounds.height, content_bounds.height));
            self.scrollable_bounds.push(bounds);
        }
    }

    impl Probe {
        /// Attach every reported text input to its focus target, whichever
        /// callback the widget made first.
        fn join_inputs(mut self) -> Self {
            for (id, bounds, text) in std::mem::take(&mut self.text_inputs) {
                let entry = self
                    .focus
                    .iter_mut()
                    .find(|entry| entry.input.is_none() && entry.id == id && entry.bounds == bounds)
                    .unwrap_or_else(|| panic!("text input at {:?} has no focus target", bounds));
                entry.input = Some(text);
            }
            self
        }

        fn contains(&self, needle: &str) -> bool {
            self.text.iter().any(|(value, _)| value.contains(needle))
        }

        fn has_focus(&self, id: &Id) -> bool {
            self.focus.iter().any(|entry| entry.id.as_ref() == Some(id))
        }

        fn focused(&self) -> Option<&FocusEntry> {
            self.focus.iter().find(|entry| entry.focused)
        }

        fn inputs(&self) -> Vec<&str> {
            self.focus
                .iter()
                .filter_map(|entry| entry.input.as_deref())
                .collect()
        }
    }

    fn load_fonts() {
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
    }

    /// The returned production element at a full window size, inside the
    /// real dashboard shell. No App, wallet, daemon or OS window exists.
    struct Harness<'a> {
        element: Element<'a, Message>,
        tree: Tree,
        node: layout::Node,
        renderer: iced::Renderer,
        size: Size,
    }

    impl<'a> Harness<'a> {
        fn new(mut element: Element<'a, Message>, (width, height): (u32, u32)) -> Self {
            load_fonts();
            let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
                font::REGULAR,
                iced::Pixels(14.0),
                Some("tiny-skia"),
            ))
            .unwrap();
            let size = Size::new(width as f32, height as f32);
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, size),
            );
            Self {
                element,
                tree,
                node,
                renderer,
                size,
            }
        }

        /// Re-render with new inputs, keeping widget state as the runtime does.
        fn rebuild(&mut self, element: Element<'a, Message>) {
            self.element = element;
            self.tree.diff(self.element.as_widget());
            self.node = self.element.as_widget_mut().layout(
                &mut self.tree,
                &self.renderer,
                &layout::Limits::new(Size::ZERO, self.size),
            );
        }

        fn run<T: 'static>(&mut self, operation: impl Operation<T> + 'static) {
            let mut operation: Box<dyn Operation<T>> = Box::new(operation);
            loop {
                self.element.as_widget_mut().operate(
                    &mut self.tree,
                    Layout::new(&self.node),
                    &self.renderer,
                    &mut operation::black_box(operation.as_mut()),
                );
                match operation.finish() {
                    operation::Outcome::Chain(next) => operation = next,
                    _ => break,
                }
            }
        }

        fn probe(&mut self) -> Probe {
            let mut probe = Probe::default();
            self.element.as_widget_mut().operate(
                &mut self.tree,
                Layout::new(&self.node),
                &self.renderer,
                &mut probe,
            );
            probe.join_inputs()
        }

        fn focus(&mut self, id: Id) {
            self.run(focusable::focus::<()>(id));
        }

        /// Tab and Shift-Tab, as `gui/mod.rs` maps them.
        fn tab(&mut self) -> FocusEntry {
            self.run(focusable::focus_next::<()>());
            self.probe()
                .focused()
                .cloned()
                .expect("Tab focuses a target")
        }

        /// Tab without assuming a target remains: Iced's `focus_next` leaves
        /// nothing focused after the last enabled target instead of wrapping.
        fn tab_raw(&mut self) -> Option<FocusEntry> {
            self.run(focusable::focus_next::<()>());
            self.probe().focused().cloned()
        }

        fn shift_tab(&mut self) -> FocusEntry {
            self.run(focusable::focus_previous::<()>());
            self.probe()
                .focused()
                .cloned()
                .expect("Shift-Tab focuses a target")
        }

        fn focused_id(&mut self) -> Option<Id> {
            self.probe().focused().and_then(|entry| entry.id.clone())
        }

        fn send(&mut self, event: Event, cursor: mouse::Cursor) -> Vec<Message> {
            let mut messages = Vec::new();
            self.element.as_widget_mut().update(
                &mut self.tree,
                &event,
                Layout::new(&self.node),
                cursor,
                &self.renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut messages),
                &Rectangle::with_size(self.size),
            );
            messages
        }

        fn key(
            &mut self,
            named: keyboard::key::Named,
            repeat: bool,
            pressed: bool,
        ) -> Vec<Message> {
            let key = keyboard::Key::Named(named);
            let physical_key =
                keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
            let event = if pressed {
                keyboard::Event::KeyPressed {
                    key: key.clone(),
                    modified_key: key,
                    physical_key,
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::empty(),
                    text: None,
                    repeat,
                }
            } else {
                keyboard::Event::KeyReleased {
                    key: key.clone(),
                    modified_key: key,
                    physical_key,
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::empty(),
                }
            };
            self.send(Event::Keyboard(event), mouse::Cursor::Unavailable)
        }

        fn type_character(&mut self, character: char) -> Vec<Message> {
            let key = keyboard::Key::Character(character.to_string().into());
            self.send(
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: key.clone(),
                    modified_key: key,
                    physical_key: keyboard::key::Physical::Unidentified(
                        keyboard::key::NativeCode::Unidentified,
                    ),
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::empty(),
                    text: Some(character.to_string().into()),
                    repeat: false,
                }),
                mouse::Cursor::Unavailable,
            )
        }

        fn press_at(&mut self, point: iced::Point) -> Vec<Message> {
            self.send(
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                mouse::Cursor::Available(point),
            )
        }

        fn release_at(&mut self, point: iced::Point) -> Vec<Message> {
            self.send(
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                mouse::Cursor::Available(point),
            )
        }

        fn click_at(&mut self, bounds: Rectangle) -> Vec<Message> {
            let mut messages = self.press_at(bounds.center());
            messages.extend(self.release_at(bounds.center()));
            messages
        }

        /// Bounds of the rendered text exactly equal to `label`.
        fn label_bounds(&mut self, label: &str) -> Rectangle {
            self.probe()
                .text
                .iter()
                .find(|(text, _)| text == label)
                .map(|(_, bounds)| *bounds)
                .unwrap_or_else(|| panic!("{:?} is not rendered", label))
        }

        /// Find the native unlabeled checkbox beside a coin identity in the
        /// actual dashboard layout. Iced's checkbox has a 16px mark child and
        /// a zero-size label child, but does not report a focus/text operation.
        fn coin_checkbox_bounds(&self, identity: Rectangle) -> Rectangle {
            fn find(layout: Layout<'_>, identity: Rectangle) -> Option<Rectangle> {
                let bounds = layout.bounds();
                let children: Vec<_> = layout.children().collect();
                let is_coin_checkbox = children.len() == 2
                    && (bounds.width - 16.0).abs() < 1.0
                    && (children[0].bounds().width - 16.0).abs() < 1.0
                    && children[1].bounds().width == 0.0
                    && bounds.x < identity.x
                    && identity.x - bounds.x < 40.0
                    && bounds.y < identity.y + identity.height
                    && bounds.y + bounds.height > identity.y;
                if is_coin_checkbox {
                    Some(bounds)
                } else {
                    children.into_iter().find_map(|child| find(child, identity))
                }
            }
            find(Layout::new(&self.node), identity).expect("native coin checkbox beside identity")
        }

        fn click(&mut self, id: &Id) -> Vec<Message> {
            let bounds = self
                .probe()
                .focus
                .iter()
                .find(|entry| entry.id.as_ref() == Some(id))
                .map(|entry| entry.bounds)
                .unwrap_or_else(|| panic!("{:?} is not a focus target", id));
            self.click_at(bounds)
        }

        /// Redraw, then draw the whole window and return its RGBA pixels.
        fn render(&mut self, theme: &theme::Theme) -> Vec<u8> {
            let redraw = self.send(
                Event::Window(iced::window::Event::RedrawRequested(
                    std::time::Instant::now(),
                )),
                mouse::Cursor::Unavailable,
            );
            // Only the dashboard's initial viewport notice may appear; see
            // `rendering_emits_only_the_dashboard_viewport_notice_...`.
            assert!(
                redraw.is_empty()
                    || matches!(redraw.as_slice(), [Message::Scroll(offset)] if *offset == 0.0),
                "{:?}",
                redraw
            );
            self.element.as_widget().draw(
                &self.tree,
                &mut self.renderer,
                theme,
                &iced::advanced::renderer::Style {
                    text_color: theme.colors.text.primary,
                },
                Layout::new(&self.node),
                mouse::Cursor::Unavailable,
                &Rectangle::with_size(self.size),
            );
            self.renderer.screenshot(
                Size::new(self.size.width as u32, self.size.height as u32),
                1.0,
                theme.colors.general.background,
            )
        }

        /// Optional synthetic capture for root's visual review; off by default.
        fn capture(&mut self, scenario: &str, appearance: &str, theme: &theme::Theme) {
            let Ok(directory) = std::env::var("R07A_CAPTURE_DIR") else {
                return;
            };
            let pixels = self.render(theme);
            let (width, height) = (self.size.width as u32, self.size.height as u32);
            std::fs::create_dir_all(&directory).expect("create disposable capture directory");
            image::save_buffer(
                std::path::Path::new(&directory).join(format!(
                    "r07a-{scenario}-{appearance}-{width}x{height}-synthetic.png"
                )),
                &pixels,
                width,
                height,
                image::ColorType::Rgba8,
            )
            .expect("save synthetic R07a capture");
        }

        fn capture_r07c(&mut self, scenario: &str, appearance: &str, theme: &theme::Theme) {
            let Ok(directory) = std::env::var("R07C_CAPTURE_DIR") else {
                return;
            };
            let pixels = self.render(theme);
            let (width, height) = (self.size.width as u32, self.size.height as u32);
            std::fs::create_dir_all(&directory).expect("create R07c capture directory");
            image::save_buffer(
                std::path::Path::new(&directory).join(format!(
                    "r07c-vault-{scenario}-{appearance}-{width}x{height}-dashboard.png"
                )),
                &pixels,
                width,
                height,
                image::ColorType::Rgba8,
            )
            .expect("save R07c dashboard capture");
        }
    }

    /// Optionally save `region` cropped from a full-window render.
    fn save_crop(name: &str, pixels: &[u8], window_width: u32, region: Rectangle) {
        let Ok(directory) = std::env::var("R07A_CAPTURE_DIR") else {
            return;
        };
        let (x0, y0) = (region.x as u32, region.y as u32);
        let (x1, y1) = (
            (region.x + region.width) as u32,
            (region.y + region.height) as u32,
        );
        let mut crop = Vec::with_capacity(((x1 - x0) * (y1 - y0) * 4) as usize);
        for y in y0..y1 {
            let start = ((y * window_width + x0) * 4) as usize;
            crop.extend_from_slice(&pixels[start..start + ((x1 - x0) * 4) as usize]);
        }
        std::fs::create_dir_all(&directory).expect("create disposable capture directory");
        image::save_buffer(
            std::path::Path::new(&directory).join(name),
            &crop,
            x1 - x0,
            y1 - y0,
            image::ColorType::Rgba8,
        )
        .expect("save synthetic R07a crop");
    }

    fn preset(block_target: usize) -> Id {
        feerate_preset_id(block_target)
    }

    fn themes() -> [(&'static str, ThemeMode, theme::Theme); 2] {
        [
            ("dark", ThemeMode::Dark, theme::Theme::dark()),
            ("light", ThemeMode::Light, theme::Theme::light()),
        ]
    }

    struct Case {
        name: &'static str,
        scenario: Scenario,
        /// Text that must be present in the rendered tree.
        present: &'static [&'static str],
        absent: &'static [&'static str],
        prepare_enabled: bool,
        add_shown: bool,
    }

    #[test]
    fn compose_states_keep_production_rules_hints_and_actions() {
        let add = Id::new(ADD_RECIPIENT_ID);
        let back = Id::new(BACK_ID);
        let clear = Id::new(CLEAR_ID);
        let prepare = Id::new(PREPARE_ID);
        let base = Scenario::valid_send();
        let capture_tall = std::env::var_os("R07A_CAPTURE_DIR").is_some();
        let cases = [
            Case {
                name: "valid",
                scenario: base.clone(),
                present: &[
                    "Send",
                    // The pinned head's wallet line.
                    "Vault · Synthetic Cube · Bitcoin mainnet",
                    "Payment details",
                    "Recipients",
                    "Fee rate (sat/vB)",
                    "Coin selection",
                    "Builds an unsigned transaction to review.",
                ],
                absent: &["Self-transfer", "Transfer details", "Recovery details"],
                prepare_enabled: true,
                add_shown: true,
            },
            Case {
                name: "missing-fee",
                scenario: Scenario {
                    is_valid: false,
                    amount_left: None,
                    feerate: ("", true),
                    ..base.clone()
                },
                present: &["Please set a feerate.", "Feerate needs to be set."],
                absent: &["Builds an unsigned transaction"],
                prepare_enabled: false,
                add_shown: true,
            },
            Case {
                name: "invalid-fee",
                scenario: Scenario {
                    is_valid: false,
                    amount_left: None,
                    feerate: ("0", false),
                    ..base.clone()
                },
                present: &["Invalid feerate.", "Feerate needs to be set."],
                absent: &[],
                prepare_enabled: false,
                add_shown: true,
            },
            Case {
                name: "duplicate",
                scenario: Scenario {
                    recipients: 2,
                    duplicate: true,
                    amount_left: None,
                    ..base.clone()
                },
                present: &[
                    "Duplicate recipient address.",
                    "Two payment addresses are the same",
                ],
                absent: &[],
                prepare_enabled: false,
                add_shown: true,
            },
            Case {
                name: "no-coins",
                scenario: Scenario {
                    is_valid: false,
                    coins_selected: false,
                    ..base.clone()
                },
                present: &[
                    "Please select at least one coin.",
                    "Select at least one coin.",
                ],
                absent: &[],
                prepare_enabled: false,
                add_shown: true,
            },
            Case {
                name: "max-under-dust",
                scenario: Scenario {
                    is_valid: false,
                    amount_left: None,
                    max_under_dust: true,
                    ..base.clone()
                },
                present: &[
                    "Enter recipient address and amount.",
                    "Select or add more funds.",
                ],
                absent: &[],
                prepare_enabled: false,
                add_shown: true,
            },
            Case {
                name: "draft-warning",
                scenario: Scenario {
                    amount_left: None,
                    draft_warning: Some("Synthetic draft failure"),
                    ..base.clone()
                },
                present: &["Synthetic draft failure", "Add recipient details."],
                absent: &["Enter recipient address and amount."],
                prepare_enabled: false,
                add_shown: true,
            },
            Case {
                name: "insufficient",
                scenario: Scenario {
                    amount_left: Some(1_000),
                    ..base.clone()
                },
                present: &["Insufficient funds.", "left to select"],
                absent: &[],
                prepare_enabled: false,
                add_shown: true,
            },
            Case {
                name: "generating",
                scenario: Scenario {
                    generating: true,
                    ..base.clone()
                },
                present: &["Prepare", "Preparing the unsigned transaction."],
                absent: &[],
                prepare_enabled: false,
                add_shown: true,
            },
            Case {
                name: "self-transfer",
                scenario: Scenario {
                    recipients: 0,
                    amount_left: None,
                    ..base.clone()
                },
                present: &["Self-transfer", "Transfer details", "selected"],
                absent: &["Recipients", "Payment details"],
                prepare_enabled: true,
                add_shown: false,
            },
            Case {
                name: "recovery",
                scenario: Scenario {
                    recovery: true,
                    first_step: false,
                    amount_left: None,
                    ..base.clone()
                },
                present: &["Recovery", "Recovery details", "Recipient", "selected"],
                absent: &["Recipients", "Payment details"],
                prepare_enabled: true,
                add_shown: false,
            },
            Case {
                name: "batch",
                scenario: Scenario {
                    recipients: 2,
                    fee: true,
                    ..base.clone()
                },
                present: &["Recipients", "Fee for this transaction"],
                absent: &["Two payment addresses are the same"],
                prepare_enabled: true,
                add_shown: true,
            },
        ];
        for (appearance, mode, theme) in themes() {
            let fixture = Fixture::new(mode, 3);
            for outer in [COMPACT, WIDE] {
                for Case {
                    name,
                    scenario,
                    present,
                    absent,
                    prepare_enabled,
                    add_shown,
                } in &cases
                {
                    let mut harness = Harness::new(compose(&fixture, scenario), outer);
                    let probe = harness.probe();
                    let context = format!("{name} {appearance} {outer:?}");
                    for text in present.iter() {
                        assert!(probe.contains(text), "{}: missing {:?}", context, text);
                    }
                    for text in absent.iter() {
                        assert!(!probe.contains(text), "{}: unexpected {:?}", context, text);
                    }
                    for (text, bounds) in &probe.text {
                        assert!(
                            bounds.x >= -1.0 && bounds.x + bounds.width <= outer.0 as f32 + 1.0,
                            "{}: {:?} at {:?} leaves the window",
                            context,
                            text,
                            bounds
                        );
                    }
                    assert_eq!(probe.has_focus(&prepare), *prepare_enabled, "{context}");
                    assert_eq!(probe.has_focus(&add), *add_shown, "{context}");
                    assert_eq!(probe.has_focus(&back), !scenario.first_step, "{context}");
                    assert!(probe.has_focus(&clear), "{}", context);
                    for (_, block_target) in FEERATE_PRESETS {
                        assert!(probe.has_focus(&preset(block_target)), "{}", context);
                    }
                    // Native editing stays native: the fee form, every passed
                    // recipient form, and the batch label for 2+ recipients.
                    let inputs = probe.inputs();
                    let custom = fee_is_custom(scenario);
                    assert!(
                        !custom
                            || inputs.contains(&scenario.feerate.0)
                            || (scenario.feerate.0.is_empty()
                                && inputs.contains(&"42 (in sats/vbyte)")),
                        "{}: fee input {:?}",
                        context,
                        inputs
                    );
                    assert!(probe.has_focus(&Id::new(FEERATE_CUSTOM_ID)), "{}", context);
                    assert_eq!(
                        inputs.contains(&"Batch label"),
                        scenario.recipients > 1,
                        "{context}: {inputs:?}"
                    );
                    assert_eq!(
                        inputs.len(),
                        usize::from(custom)
                            + 3 * scenario.recipients
                            + usize::from(scenario.recipients > 1),
                        "{context}: {inputs:?}"
                    );
                    harness.capture(name, appearance, &theme);
                    if capture_tall
                        && outer == WIDE
                        && matches!(*name, "valid" | "batch" | "invalid-fee" | "generating")
                    {
                        // The ordinary viewport cuts off the real dashboard
                        // footer. TALL is already used above to put every
                        // Send control inside the rendered content viewport.
                        Harness::new(compose(&fixture, scenario), TALL)
                            .capture(name, appearance, &theme);
                    }
                }
            }
        }
    }

    /// Whether an emitted message is the intent an Action is expected to send.
    type IntentCheck = fn(&Message) -> bool;

    #[test]
    fn compose_actions_follow_visual_order_and_activate_existing_intents_once() {
        use keyboard::key::Named;
        let fixture = Fixture::new(ThemeMode::Dark, 3);
        let scenario = Scenario {
            first_step: false,
            ..Scenario::valid_send()
        };
        let cases: [(Id, IntentCheck); 8] = [
            (Id::new(ADD_RECIPIENT_ID), |m| {
                matches!(m, Message::CreateSpend(CreateSpendMessage::AddRecipient))
            }),
            (preset(24), |m| {
                matches!(
                    m,
                    Message::CreateSpend(CreateSpendMessage::FetchFeeEstimate(24))
                )
            }),
            (preset(6), |m| {
                matches!(
                    m,
                    Message::CreateSpend(CreateSpendMessage::FetchFeeEstimate(6))
                )
            }),
            (preset(1), |m| {
                matches!(
                    m,
                    Message::CreateSpend(CreateSpendMessage::FetchFeeEstimate(1))
                )
            }),
            (Id::new(FEERATE_CUSTOM_ID), |m| {
                matches!(m, Message::CreateSpend(CreateSpendMessage::CustomFeerate))
            }),
            (Id::new(BACK_ID), |m| matches!(m, Message::Previous)),
            (Id::new(CLEAR_ID), |m| {
                matches!(m, Message::CreateSpend(CreateSpendMessage::Clear))
            }),
            (Id::new(PREPARE_ID), |m| {
                matches!(m, Message::CreateSpend(CreateSpendMessage::Generate))
            }),
        ];
        for outer in [COMPACT, WIDE] {
            let mut harness = Harness::new(compose(&fixture, &scenario), outer);
            let order = [
                Id::new(ADD_RECIPIENT_ID),
                preset(24),
                preset(6),
                preset(1),
                Id::new(FEERATE_CUSTOM_ID),
            ];
            harness.focus(order[0].clone());
            assert_eq!(harness.focused_id(), Some(order[0].clone()));
            for expected in &order[1..] {
                assert_eq!(harness.tab().id.as_ref(), Some(expected), "{outer:?}");
            }
            let fee_input = harness.tab();
            assert_eq!(fee_input.id, None);
            assert_eq!(
                fee_input.input.as_deref(),
                Some("5"),
                "Tab reaches the fee form"
            );
            for expected in [BACK_ID, CLEAR_ID, PREPARE_ID] {
                assert_eq!(harness.tab().id, Some(Id::new(expected)), "{outer:?}");
            }
            for expected in [CLEAR_ID, BACK_ID] {
                assert_eq!(harness.shift_tab().id, Some(Id::new(expected)), "{outer:?}");
            }
            assert_eq!(harness.shift_tab().input.as_deref(), Some("5"));
            for expected in order.iter().rev() {
                assert_eq!(harness.shift_tab().id.as_ref(), Some(expected), "{outer:?}");
            }
            let recipient_field = harness.shift_tab();
            assert_eq!(recipient_field.id, None);
            assert!(
                recipient_field.input.is_some(),
                "Shift-Tab from Add recipient reaches the passed recipient form"
            );

            for (id, expected) in cases.iter().cloned() {
                harness.focus(id.clone());
                assert_eq!(harness.focused_id(), Some(id.clone()));
                for key in [Named::Enter, Named::Space] {
                    let pressed = harness.key(key, false, true);
                    assert!(
                        matches!(pressed.as_slice(), [m] if expected(m)),
                        "{:?} {:?}: {:?}",
                        id,
                        key,
                        pressed
                    );
                    assert!(harness.key(key, true, true).is_empty(), "{:?} repeat", id);
                    assert!(
                        harness.key(key, false, false).is_empty(),
                        "{:?} release",
                        id
                    );
                }
            }
        }
        let mut harness = Harness::new(compose(&fixture, &scenario), TALL);
        for (id, expected) in cases {
            let clicked = harness.click(&id);
            assert!(
                matches!(
                    clicked.as_slice(),
                    [Message::FocusAction(focus), m] if focus == &id && expected(m)
                ),
                "{:?} click: {:?}",
                id,
                clicked
            );
        }
    }

    #[test]
    fn unavailable_prepare_is_skipped_by_focus_and_inert() {
        use keyboard::key::Named;
        let fixture = Fixture::new(ThemeMode::Light, 3);
        let prepare = Id::new(PREPARE_ID);
        for scenario in [
            Scenario {
                is_valid: false,
                amount_left: None,
                feerate: ("", true),
                ..Scenario::valid_send()
            },
            Scenario {
                generating: true,
                ..Scenario::valid_send()
            },
        ] {
            for outer in [COMPACT, WIDE, TALL] {
                let mut harness = Harness::new(compose(&fixture, &scenario), outer);
                assert!(!harness.probe().has_focus(&prepare));
                harness.focus(Id::new(CLEAR_ID));
                assert_eq!(harness.focused_id(), Some(Id::new(CLEAR_ID)));
                // Clear is the last enabled target in the dashboard. Tab past
                // it leaves nothing focused; it never lands on Prepare.
                let after_clear = harness.tab_raw();
                assert!(
                    after_clear.is_none(),
                    "Tab after Clear focused {:?}",
                    after_clear
                );
                harness.focus(prepare.clone());
                assert_eq!(harness.focused_id(), None);
                for key in [Named::Enter, Named::Space] {
                    let messages = harness.key(key, false, true);
                    assert!(messages.is_empty(), "{:?}: {:?}", key, messages);
                    harness.key(key, false, false);
                }
                // Only the tall window guarantees Prepare is inside the
                // shell's content viewport, so the press really lands on it.
                if outer == TALL {
                    let label = harness.label_bounds("Prepare");
                    assert!(label.y + label.height <= TALL.1 as f32);
                    let clicked = harness.click_at(label);
                    assert!(clicked.is_empty(), "{:?}", clicked);
                }
            }
        }
    }

    /// One key: no footer; 2 of 3 and 3 of 5 say how many keys sign.
    #[test]
    fn the_threshold_footer_names_the_keys_a_send_needs() {
        assert_eq!(threshold_footer(1, 1), None);
        assert_eq!(
            threshold_footer(2, 3).as_deref(),
            Some("This Vault needs 2 of 3 keys. You'll sign, then share the PSBT.")
        );
        assert_eq!(
            threshold_footer(3, 5).as_deref(),
            Some("This Vault needs 3 of 5 keys. You'll sign, then share the PSBT.")
        );
        let fixture = Fixture::new(ThemeMode::Dark, 3);
        for (threshold, shown) in [(None, false), (Some((1, 1)), false), (Some((2, 3)), true)] {
            let scenario = Scenario {
                threshold,
                ..Scenario::valid_send()
            };
            let probe = Harness::new(compose(&fixture, &scenario), TALL).probe();
            assert_eq!(
                probe.contains("This Vault needs 2 of 3 keys."),
                shown,
                "{threshold:?}"
            );
        }
    }

    /// A Branta tamper result leaves the draft fully selected
    /// (`amount_left == Some(0)`) but blocks the send. Its warning must be
    /// the hint shown, in a normal send and in recovery alike.
    #[test]
    fn a_tampered_recipient_shows_its_warning_and_blocks_prepare() {
        const TAMPER: &str = "Synthetic tamper warning. Do not send.";
        let fixture = Fixture::new(ThemeMode::Dark, 3);
        for recovery in [false, true] {
            let scenario = Scenario {
                is_valid: false,
                recovery,
                amount_left: Some(0),
                blocking_warning: Some(TAMPER),
                ..Scenario::valid_send()
            };
            let probe = Harness::new(compose(&fixture, &scenario), TALL).probe();
            assert!(probe.contains(TAMPER), "recovery {}", recovery);
            assert!(
                !probe.has_focus(&Id::new(PREPARE_ID)),
                "recovery {}",
                recovery
            );
        }
    }

    /// Until coins are chosen the section says the draft picks them, and
    /// Choose coins opens the existing list.
    #[test]
    fn choose_coins_opens_the_inline_coin_list() {
        let fixture = Fixture::new(ThemeMode::Dark, 3);
        let automatic = Scenario {
            coins_shown: false,
            ..Scenario::valid_send()
        };
        let mut harness = Harness::new(compose(&fixture, &automatic), TALL);
        let probe = harness.probe();
        assert!(probe.contains(AUTOMATIC_COINS));
        assert!(!probe.contains("Coin selection"));
        let messages = harness.click(&Id::new(CHOOSE_COINS_ID));
        assert!(
            matches!(
                messages.as_slice(),
                [
                    Message::FocusAction(_),
                    Message::CreateSpend(CreateSpendMessage::ChooseCoins)
                ]
            ),
            "{:?}",
            messages
        );

        let probe = Harness::new(compose(&fixture, &Scenario::valid_send()), TALL).probe();
        assert!(probe.contains("Coin selection"));
        assert!(!probe.contains(AUTOMATIC_COINS));
        assert!(!probe
            .focus
            .iter()
            .any(|entry| entry.id == Some(Id::new(CHOOSE_COINS_ID))));
    }

    #[test]
    fn vault_send_steps_follow_the_transaction() {
        assert_eq!(review_step(false, &SpendStatus::Pending), 1);
        assert_eq!(review_step(true, &SpendStatus::Pending), 2);
        assert_eq!(review_step(true, &SpendStatus::Broadcast), 3);
        assert_eq!(review_step(true, &SpendStatus::Spent), 3);
        let fixture = Fixture::new(ThemeMode::Dark, 3);
        let probe = Harness::new(compose(&fixture, &Scenario::valid_send()), TALL).probe();
        for label in SEND_STEPS {
            assert!(probe.contains(label), "{}", label);
        }
    }

    /// The rail's fee: unset before a draft, masked with the balances, and in
    /// the user's unit otherwise (never "After Prepare": the draft sets it).
    #[test]
    fn the_rail_fee_is_masked_with_balances_and_uses_the_display_unit() {
        use crate::app::view::shared::landing::capture;
        let feerate = form::Value {
            value: "5".to_string(),
            warning: None,
            valid: true,
        };
        let fee = Amount::from_sat(1_234);
        let render = |fee: Option<&Amount>, masked: bool, unit: BitcoinDisplayUnit| {
            capture::inspect(
                "vault-spend-summary",
                spend_summary(1, false, &feerate, false, None, fee, masked, unit),
                (1440, 900),
                "dark",
                theme::Theme::dark(),
            )
        };
        let unset = render(None, false, BitcoinDisplayUnit::Sats);
        assert!(unset.contains("Not set"));
        let masked = render(Some(&fee), true, BitcoinDisplayUnit::Sats);
        assert!(masked.contains("••••••"));
        assert!(!masked.contains("1,234") && !masked.contains("1 234"));
        assert!(!masked.contains("Not set"));
        let btc = render(Some(&fee), false, BitcoinDisplayUnit::BTC);
        assert!(btc.contains(" BTC"), "fee in BTC");
        assert!(!btc.contains("sats"));
        let sats = render(Some(&fee), false, BitcoinDisplayUnit::Sats);
        assert!(sats.contains(" sats"));
    }

    /// A preset hides the field and shows its rate; Custom shows the field.
    #[test]
    fn a_preset_shows_its_rate_and_custom_shows_the_field() {
        let fixture = Fixture::new(ThemeMode::Dark, 3);
        let preset_scenario = Scenario {
            selected: Some(6),
            ..Scenario::valid_send()
        };
        let probe = Harness::new(compose(&fixture, &preset_scenario), TALL).probe();
        assert!(probe.contains("5 sat/vB"));
        assert!(!probe.inputs().contains(&"5"));
        let custom = Scenario {
            custom: true,
            ..preset_scenario
        };
        let probe = Harness::new(compose(&fixture, &custom), TALL).probe();
        assert!(probe.inputs().contains(&"5"));
        // The form's rate line is gone; the rail's summary still names it.
        assert!(!probe.text.iter().any(|(value, _)| value == "5 sat/vB"));
        assert!(probe.contains("5 sat/vB · Custom"));
    }

    #[test]
    fn preset_identity_is_the_block_target_and_its_own_fetch_disables_it() {
        use keyboard::key::Named::Enter;
        let fixture = Fixture::new(ThemeMode::Dark, 3);
        let base = Scenario::valid_send();
        let state = |loading: Option<usize>, selected: Option<usize>| Scenario {
            loading,
            selected,
            ..base.clone()
        };
        let normal = preset(6);
        let label = "Normal (~1h)";
        let mut harness = Harness::new(compose(&fixture, &base), TALL);

        // Focus on a preset survives other presets loading and selection
        // changes, because its ID is its block target.
        harness.focus(normal.clone());
        for (loading, selected) in [
            (Some(1), None),
            (None, Some(1)),
            (Some(24), Some(1)),
            (None, Some(6)),
        ] {
            harness.rebuild(compose(&fixture, &state(loading, selected)));
            assert_eq!(
                harness.focused_id(),
                Some(normal.clone()),
                "loading {:?} selected {:?}",
                loading,
                selected
            );
            let pressed = harness.key(Enter, false, true);
            harness.key(Enter, false, false);
            assert!(
                matches!(
                    pressed.as_slice(),
                    [Message::CreateSpend(CreateSpendMessage::FetchFeeEstimate(
                        6
                    ))]
                ),
                "{:?}",
                pressed
            );
        }

        // Its own fetch disables it. It stays visible with the same ID but
        // leaves the focus order and keyboard activation. Keeping focus
        // through its own loading would need a shared Action capability; that
        // is an integration dependency, not something this presenter fakes.
        harness.rebuild(compose(&fixture, &state(Some(6), None)));
        let probe = harness.probe();
        assert!(!probe.has_focus(&normal));
        assert!(probe.has_focus(&preset(24)) && probe.has_focus(&preset(1)));
        assert!(probe.contains(label));
        assert_eq!(harness.focused_id(), None);
        assert!(harness.key(Enter, false, true).is_empty());
        harness.key(Enter, false, false);
        harness.focus(normal.clone());
        assert_eq!(harness.focused_id(), None);
        harness.focus(preset(24));
        assert_eq!(
            harness.tab().id,
            Some(preset(1)),
            "Tab skips the loading preset"
        );
        let busy = harness.label_bounds(label);
        assert!(harness.click_at(busy).is_empty());

        // Pressed while loading, estimate arrives, released: no fetch.
        let point = busy.center();
        let down = harness.press_at(point);
        harness.rebuild(compose(&fixture, &state(None, Some(6))));
        assert!(harness.label_bounds(label).contains(point));
        let up = harness.release_at(point);
        assert!(down.is_empty() && up.is_empty(), "{:?} {:?}", down, up);

        // Pressed while ready, its fetch starts, released: the stale press
        // must not dispatch a second fetch.
        let point = harness.label_bounds(label).center();
        let down = harness.press_at(point);
        assert!(
            matches!(down.as_slice(), [Message::FocusAction(id)] if id == &normal),
            "{:?}",
            down
        );
        harness.rebuild(compose(&fixture, &state(Some(6), Some(6))));
        assert!(harness.label_bounds(label).contains(point));
        let up = harness.release_at(point);
        assert!(up.is_empty(), "{:?}", up);
    }

    /// A focused preset's ring stays visible when its right neighbour is
    /// selected. The neighbour paints after the ring with an opaque fill, so
    /// the presets keep `PRESET_SPACING` (the ring's 4px outset) between them.
    /// Samples 3px outside each edge of the focused control in the actual
    /// dashboard render, like the shell's
    /// `selected_successor_does_not_paint_over_focused_nav_ring`.
    #[test]
    fn selected_right_neighbour_does_not_paint_over_focused_preset_ring() {
        for (appearance, mode, theme) in themes() {
            let fixture = Fixture::new(mode, 3);
            // The compact window's width, but tall enough that the presets
            // sit inside the window under the pinned page head: the ring's
            // spacing depends on the width, not on how far down they are.
            for outer in [(COMPACT.0, TALL.1), WIDE] {
                for (focused, selected) in [(24, 6), (6, 1)] {
                    let context = format!(
                        "{} {:?} focus {} selected {}",
                        appearance, outer, focused, selected
                    );
                    let scenario = Scenario {
                        selected: Some(selected),
                        ..Scenario::valid_send()
                    };
                    let mut harness = Harness::new(compose(&fixture, &scenario), outer);
                    // Arrive as a keyboard user does: only keyboard
                    // navigation paints the ring.
                    harness.key(keyboard::key::Named::Tab, false, true);
                    harness.focus(preset(focused));
                    assert_eq!(harness.focused_id(), Some(preset(focused)), "{}", context);
                    let probe = harness.probe();
                    let bounds = |target: usize| {
                        probe
                            .focus
                            .iter()
                            .find(|entry| entry.id == Some(preset(target)))
                            .map(|entry| entry.bounds)
                            .unwrap_or_else(|| panic!("preset {} is not rendered", target))
                    };
                    let control = bounds(focused);
                    let neighbour = bounds(selected);
                    let segment = Rectangle {
                        x: bounds(24).x - 12.0,
                        y: control.y - 12.0,
                        width: bounds(1).x + bounds(1).width - bounds(24).x + 24.0,
                        height: control.height + 24.0,
                    };
                    // The selected preset is painted after the focused one.
                    assert!(
                        neighbour.x > control.x + control.width,
                        "{}: {:?} {:?}",
                        context,
                        control,
                        neighbour
                    );
                    // Every sample must land inside the rendered window.
                    assert!(
                        segment.x >= 0.0
                            && segment.y >= 0.0
                            && segment.x + segment.width <= outer.0 as f32
                            && segment.y + segment.height <= outer.1 as f32,
                        "{}: segment {:?} outside the window",
                        context,
                        segment
                    );

                    let pixels = harness.render(&theme);
                    let pixel = |x: u32, y: u32| {
                        let i = ((y * outer.0 + x) * 4) as usize;
                        [pixels[i], pixels[i + 1], pixels[i + 2]]
                    };
                    let has_color = |actual: [u8; 3], color: iced::Color| {
                        let expected =
                            [color.r, color.g, color.b].map(|component| (component * 255.0) as u8);
                        actual
                            .iter()
                            .zip(expected)
                            .all(|(actual, expected)| actual.abs_diff(expected) <= 2)
                    };
                    // The neighbour really has the opaque selected fill.
                    let fill = pixel(
                        (neighbour.x + 2.0) as u32,
                        (neighbour.y + neighbour.height / 2.0) as u32,
                    );
                    assert!(
                        has_color(fill, theme.colors.shell.selected),
                        "{}: neighbour fill {:?}",
                        context,
                        fill
                    );
                    let center_x = (control.x + control.width / 2.0) as u32;
                    let center_y = (control.y + control.height / 2.0) as u32;
                    for (edge, x, y) in [
                        ("top", center_x, (control.y - 3.0) as u32),
                        (
                            "bottom",
                            center_x,
                            (control.y + control.height + 3.0) as u32,
                        ),
                        ("left", (control.x - 3.0) as u32, center_y),
                        ("right", (control.x + control.width + 3.0) as u32, center_y),
                    ] {
                        let actual = pixel(x, y);
                        assert!(
                            has_color(actual, theme.colors.shell.focus),
                            "{}: {} ring edge covered at ({}, {}): {:?}",
                            context,
                            edge,
                            x,
                            y,
                            actual
                        );
                    }
                    save_crop(
                        &format!(
                            "r07a-preset-ring-focus{}-selected{}-{}-{}x{}-synthetic-crop.png",
                            focused, selected, appearance, outer.0, outer.1
                        ),
                        &pixels,
                        outer.0,
                        segment,
                    );
                }
            }
        }
    }

    /// Iced's TextInput reports `text_input` before `focusable`; the
    /// coincube-ui input inside `form::Form` reports them the other way round.
    /// The probe must see both as native inputs.
    #[test]
    fn input_probe_joins_either_operation_order() {
        let form_value = value("", true);
        let element: Element<'_, Message> = Column::new()
            .push(
                iced::widget::text_input("Iced placeholder", "iced value")
                    .on_input(|_| Message::Reload),
            )
            .push(form::Form::new("Form placeholder", &form_value, |_| {
                Message::Reload
            }))
            .into();
        let mut harness = Harness::new(element, WIDE);
        assert_eq!(
            harness.probe().inputs(),
            vec!["iced value", "Form placeholder"]
        );
    }

    fn r07c_case(mode: ThemeMode, name: &str) -> (Fixture, Scenario) {
        let mut fixture = Fixture::new(
            mode,
            if name == "coin-mix" || name == "batch-btc" {
                18
            } else {
                3
            },
        );
        let mut scenario = Scenario::valid_send();
        match name {
            "single-sats" => {}
            "batch-btc" => {
                scenario.recipients = 2;
                scenario.unit = BitcoinDisplayUnit::BTC;
                fixture.amounts = vec![value("0.0004", true), value("0.0002", true)];
                fixture.labels = vec![value("Rent", true), value("Savings", true)];
                fixture.coins_labels.insert(
                    fixture.coins[0].outpoint.to_string(),
                    "Long-term savings for the extended household and future school expenses"
                        .into(),
                );
                fixture.coins[1].block_height = None;
                fixture.coins[2].spend_info = Some(coincubed::commands::LCSpendInfo {
                    txid: Txid::from_str(&format!("{:064x}", 99)).unwrap(),
                    height: None,
                });
            }
            "fiat-editing" => {
                scenario.fiat = true;
                scenario.fiat_editing = true;
                fixture.amounts[0] = value("5000", true);
                fixture.fiat_amounts[0] = value("2.50", true);
            }
            "max-under-dust" => {
                scenario.max_selected = true;
                scenario.max_under_dust = true;
                scenario.max_estimated = Some(400);
                scenario.is_valid = false;
                scenario.amount_left = None;
                fixture.dust_warning = Some("Amount is below the dust limit".into());
                fixture.amounts[0] = value("", true);
            }
            "coin-mix" => {
                fixture.coins_labels.insert(
                    fixture.coins[0].outpoint.to_string(),
                    "Long-term savings for the extended household and future school expenses"
                        .into(),
                );
                fixture.coins_labels.insert(
                    fixture.coins[1].outpoint.txid.to_string(),
                    "Transfer from another wallet".into(),
                );
                fixture.coins[1].block_height = None;
                fixture.coins[2].spend_info = Some(coincubed::commands::LCSpendInfo {
                    txid: Txid::from_str(&format!("{:064x}", 99)).unwrap(),
                    height: None,
                });
                fixture.coins[3].block_height = Some(95);
            }
            "recovery" => {
                scenario.recovery = true;
            }
            "generating" => {
                scenario.generating = true;
            }
            _ => panic!("unknown R07c scenario: {}", name),
        }
        (fixture, scenario)
    }

    #[test]
    fn r07c_actual_dashboard_capture_matrix_keeps_visible_compose_truth() {
        let core = [
            "single-sats",
            "batch-btc",
            "fiat-editing",
            "max-under-dust",
            "coin-mix",
            "recovery",
        ];
        let tall = ["single-sats", "batch-btc", "max-under-dust", "generating"];
        for (appearance, mode, theme) in themes() {
            for name in core {
                let (fixture, scenario) = r07c_case(mode, name);
                for outer in [COMPACT, WIDE] {
                    let mut harness = Harness::new(compose(&fixture, &scenario), outer);
                    let probe = harness.probe();
                    assert!(probe.contains("Recipient 1"), "{} {}", name, appearance);
                    assert!(probe.contains("Address") && probe.contains("Description"));
                    assert_eq!(probe.contains("Remove"), name != "recovery");
                    assert_eq!(probe.contains("Recipient 2"), name == "batch-btc");
                    assert_eq!(probe.contains("Amount BTC"), name == "batch-btc");
                    assert_eq!(probe.contains("Amount (sats)"), name != "batch-btc");
                    assert_eq!(probe.contains("USD"), name == "fiat-editing");
                    if name == "fiat-editing" {
                        assert!(
                            probe
                                .inputs()
                                .windows(2)
                                .any(|pair| pair == ["5000", "2.50"]),
                            "fiat preview must pair 5,000 sats with USD 2.50"
                        );
                    }
                    if name == "recovery" {
                        assert!(!probe.contains("MAX"));
                    }
                    if outer == COMPACT && (name == "coin-mix" || name == "batch-btc") {
                        let expected_amount = match scenario.unit {
                            BitcoinDisplayUnit::Sats => "50,000",
                            BitcoinDisplayUnit::BTC => "50 000",
                        };
                        let label = probe.text.iter().find(|(text, _)| text ==
                            "Long-term savings for the extended household and future school expenses")
                            .map(|(_, bounds)| *bounds).expect("long coin label");
                        let amount = probe
                            .text
                            .iter()
                            .find(|(text, bounds)| {
                                text == expected_amount
                                    && bounds.y < label.y + label.height
                                    && bounds.y + bounds.height > label.y
                            })
                            .map(|(_, bounds)| *bounds)
                            .expect("same-row amount");
                        let checkbox = harness.coin_checkbox_bounds(label);
                        assert!(
                            checkbox.x >= 0.0
                                && checkbox.x + checkbox.width < label.x
                                && checkbox.y < label.y + label.height
                                && checkbox.y + checkbox.height > label.y,
                            "actual compact coin checkbox {:?} left of {:?}",
                            checkbox,
                            label
                        );
                        assert!(
                            label.width > 0.0
                                && label.height > 0.0
                                && label.x + label.width <= amount.x
                                && amount.x + amount.width <= outer.0 as f32,
                            "{} compact {:?} label {:?}, amount {:?}",
                            name,
                            scenario.unit,
                            label,
                            amount
                        );
                    }
                    assert!(
                        probe.scrollables.iter().any(|(height, content)| {
                            *height <= 300.5 && (name != "coin-mix" || content > height)
                        }),
                        "{}: expected the native coin scroll",
                        name
                    );
                    harness.capture_r07c(name, appearance, &theme);
                }
            }
            for name in tall {
                let (fixture, scenario) = r07c_case(mode, name);
                let mut harness = Harness::new(compose(&fixture, &scenario), TALL);
                assert!(harness.probe().contains("Coin selection"));
                harness.capture_r07c(name, appearance, &theme);
            }
        }
    }

    /// Coin text must fit the native coin scroll's viewport. The dashboard
    /// window is wider than this inner clip, so window bounds alone miss a
    /// truncated BTC unit at the right edge of the list.
    #[test]
    fn r07c_coin_amount_and_unit_fit_inner_scroll_viewport() {
        // Operation::text reports the allocated layout rectangle, which can
        // be narrower than the glyphs in a constrained amount row. Measure
        // each actual Iced paragraph again with ample width, preserving its
        // loaded font, size, line height, and shaping from the native Tree.
        fn full_text_width(tree: &Tree, content: &str) -> f32 {
            type NativeParagraph = <iced::Renderer as iced::advanced::text::Renderer>::Paragraph;

            fn find(tree: &Tree, content: &str) -> Option<f32> {
                if let iced::advanced::widget::tree::State::Some(state) = &tree.state {
                    if let Some(paragraph) = state.downcast_ref::<Plain<NativeParagraph>>() {
                        if paragraph.content() == content
                            && (paragraph.raw().size().0 - P1_SIZE as f32).abs() < 0.5
                        {
                            let mut text = paragraph.as_text();
                            text.bounds = Size::new(10_000.0, 100.0);
                            return Some(NativeParagraph::with_text(text).min_bounds().width);
                        }
                    }
                }
                tree.children.iter().find_map(|child| find(child, content))
            }

            find(tree, content)
                .unwrap_or_else(|| panic!("native P1 paragraph absent for {:?}", content))
        }

        for (appearance, mode, _) in themes() {
            for name in ["single-sats", "batch-btc", "coin-mix"] {
                let (fixture, scenario) = r07c_case(mode, name);
                let unit = match scenario.unit {
                    BitcoinDisplayUnit::BTC => "BTC",
                    BitcoinDisplayUnit::Sats => "SATS",
                };
                for outer in [COMPACT, WIDE, TALL] {
                    let mut harness = Harness::new(compose(&fixture, &scenario), outer);
                    let probe = harness.probe();
                    assert_eq!(probe.scrollable_bounds.len(), probe.scrollables.len());
                    let (viewport, (reported_height, content_height)) = probe
                        .scrollable_bounds
                        .iter()
                        .zip(&probe.scrollables)
                        .find(|(bounds, _)| bounds.height <= 300.5)
                        .expect("native coin-list viewport");
                    assert!((viewport.height - *reported_height).abs() < 0.01);
                    // Iced embeds its 10px rail plus this scroll's 2px gap
                    // only while the native content exceeds the viewport.
                    let right_reserve = if *content_height > viewport.height {
                        12.0
                    } else {
                        2.0
                    };
                    let viewport = *viewport;
                    let visible_coin_text: Vec<_> = probe
                        .text
                        .iter()
                        .filter(|(_, bounds)| {
                            bounds.x >= viewport.x
                                && bounds.y + bounds.height / 2.0 >= viewport.y
                                && bounds.y + bounds.height / 2.0 <= viewport.y + viewport.height
                        })
                        .collect();
                    // The second coin has a short identity even in the batch
                    // fixture, which makes it the useful clipping regression.
                    // Match its complete split numeric formatting and unit on
                    // one line, then check those three actual text bounds.
                    let formatted = fixture.coins[1]
                        .amount
                        .to_formatted_string_with_unit(scenario.unit);
                    let split = formatted
                        .char_indices()
                        .find(|(_, character)| character.is_ascii_digit() && *character != '0')
                        .map(|(index, _)| index)
                        .expect("nonzero second coin");
                    let (leading, significant) = formatted.split_at(split);
                    let amount = visible_coin_text
                        .iter()
                        .find_map(|entry| {
                            let (text, bounds) = *entry;
                            (text == significant).then_some(*bounds)
                        })
                        .unwrap_or_else(|| {
                            panic!(
                                "{} {} {:?}: missing formatted second coin {:?} in {:?}",
                                appearance, name, outer, formatted, viewport
                            )
                        });
                    let same_line =
                        |a: Rectangle, b: Rectangle| a.y < b.y + b.height && b.y < a.y + a.height;
                    let leading_bounds = (!leading.is_empty()).then(|| {
                        visible_coin_text
                            .iter()
                            .find_map(|entry| {
                                let (text, bounds) = *entry;
                                (text == leading
                                    && same_line(*bounds, amount)
                                    && bounds.x + bounds.width <= amount.x + 1.0)
                                    .then_some(*bounds)
                            })
                            .unwrap_or_else(|| {
                                panic!(
                                    "{} {} {:?}: missing leading amount {:?} on coin row",
                                    appearance, name, outer, leading
                                )
                            })
                    });
                    let unit_bounds = visible_coin_text
                        .iter()
                        .find_map(|entry| {
                            let (text, bounds) = *entry;
                            (text == unit
                                && same_line(*bounds, amount)
                                && bounds.x >= amount.x + amount.width)
                                .then_some(*bounds)
                        })
                        .unwrap_or_else(|| {
                            panic!(
                                "{} {} {:?}: missing {} beside second coin {:?}",
                                appearance, name, outer, unit, formatted
                            )
                        });
                    let significant_width = full_text_width(&harness.tree, significant);
                    let unit_width = full_text_width(&harness.tree, unit);
                    assert!(
                        amount.x + significant_width + 4.0 <= unit_bounds.x,
                        "{} {} {:?}: full second-coin amount {:?} at {:?} overlaps unit at {:?}",
                        appearance,
                        name,
                        outer,
                        formatted,
                        amount,
                        unit_bounds
                    );
                    let mut parts = vec![
                        ("amount", amount, significant_width),
                        ("unit", unit_bounds, unit_width),
                    ];
                    if let Some(bounds) = leading_bounds {
                        let leading_width = full_text_width(&harness.tree, leading);
                        assert!(
                            bounds.x + leading_width <= amount.x + 1.0,
                            "{} {} {:?}: full leading amount {:?} at {:?} overlaps significant digits at {:?}",
                            appearance, name, outer, leading, bounds, amount
                        );
                        parts.push(("leading amount", bounds, leading_width));
                    }
                    for (part, bounds, full_width) in parts {
                        assert!(
                            bounds.x >= viewport.x
                                && bounds.x + full_width <= viewport.x + viewport.width - right_reserve,
                            "{} {} {:?}: {} of {:?} starts at {:?}, needs {:.1}px of actual text, exceeds inner {:?}",
                            appearance, name, outer, part, formatted, bounds, full_width, viewport
                        );
                    }
                    assert!(
                        visible_coin_text.iter().any(|(text, _)| text == unit),
                        "{} {} {:?}: no visible coin unit in {:?}",
                        appearance,
                        name,
                        outer,
                        viewport
                    );
                    for (text, bounds) in visible_coin_text {
                        assert!(
                            bounds.x + bounds.width <= viewport.x + viewport.width - right_reserve,
                            "{} {} {:?}: coin text {:?} at {:?} exceeds inner {:?}",
                            appearance,
                            name,
                            outer,
                            text,
                            bounds,
                            viewport
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn r07c_native_second_recipient_fields_keep_existing_edit_payloads() {
        use keyboard::key::Named;
        let mut fixture = Fixture::new(ThemeMode::Dark, 3);
        fixture.amounts = vec![value("0.0004", true), value("0.00005", true)];
        fixture.fiat_amounts[1] = value("2.50", true);
        let base = Scenario {
            recipients: 2,
            unit: BitcoinDisplayUnit::BTC,
            ..Scenario::valid_send()
        };
        let mut harness = Harness::new(compose(&fixture, &base), TALL);
        let fields = [
            (ADDRESS_B, ' ', "address", ADDRESS_B.to_string()),
            ("Payment label", 'x', "label", "x".to_string()),
            ("0.00005", '5', "amount", "0.000055".to_string()),
        ];
        for (existing, typed, field, expected) in fields {
            let bounds = harness
                .probe()
                .focus
                .iter()
                .rfind(|entry| entry.input.as_deref() == Some(existing))
                .map(|entry| entry.bounds)
                .expect("second recipient input exists");
            harness.click_at(bounds);
            harness.key(Named::End, false, true);
            let messages = harness.type_character(typed);
            assert!(
                matches!(messages.as_slice(), [Message::CreateSpend(CreateSpendMessage::RecipientEdited(1, key, value))]
                if *key == field && value == &expected),
                "{}: {:?}",
                field,
                messages
            );
        }
        let sats_fixture = Fixture::new(ThemeMode::Dark, 3);
        let sats = Scenario {
            unit: BitcoinDisplayUnit::Sats,
            ..base.clone()
        };
        let mut sats_harness = Harness::new(compose(&sats_fixture, &sats), TALL);
        let bounds = sats_harness
            .probe()
            .focus
            .iter()
            .find(|entry| entry.input.as_deref() == Some("20000"))
            .map(|entry| entry.bounds)
            .unwrap();
        sats_harness.click_at(bounds);
        sats_harness.key(Named::End, false, true);
        let messages = sats_harness.type_character('x');
        assert!(
            matches!(messages.as_slice(), [Message::CreateSpend(CreateSpendMessage::RecipientEdited(1, "amount", value))]
            if value == "20000"),
            "{:?}",
            messages
        );
        let fiat = Scenario {
            fiat: true,
            fiat_editing: true,
            ..base
        };
        let mut fiat_harness = Harness::new(compose(&fixture, &fiat), TALL);
        let bounds = fiat_harness
            .probe()
            .focus
            .iter()
            .rfind(|entry| entry.input.as_deref() == Some("2.50"))
            .map(|entry| entry.bounds)
            .unwrap();
        fiat_harness.click_at(bounds);
        fiat_harness.key(Named::End, false, true);
        let messages = fiat_harness.type_character(' ');
        assert!(
            matches!(messages.as_slice(), [Message::CreateSpend(CreateSpendMessage::RecipientFiatAmountEdited(1, value, _))]
            if value == "2.50"),
            "{:?}",
            messages
        );
    }

    #[test]
    fn r07c_native_field_focus_survives_warning_max_fiat_and_batch_rebuilds() {
        let fixture = Fixture::new(ThemeMode::Light, 3);
        let base = Scenario::valid_send();
        let mut harness = Harness::new(compose(&fixture, &base), TALL);
        let address = harness
            .probe()
            .focus
            .iter()
            .find(|entry| entry.input.as_deref() == Some(ADDRESS_A))
            .map(|entry| entry.bounds)
            .unwrap();
        harness.click_at(address);
        assert_eq!(
            harness
                .probe()
                .focused()
                .and_then(|entry| entry.input.as_deref()),
            Some(ADDRESS_A)
        );
        assert_eq!(harness.tab().input.as_deref(), Some("Payment label"));
        assert_eq!(harness.tab().input.as_deref(), Some("40000"));
        assert_eq!(harness.shift_tab().input.as_deref(), Some("Payment label"));
        assert_eq!(harness.shift_tab().input.as_deref(), Some(ADDRESS_A));
        for (transition, changed) in [
            (
                "insufficient",
                Scenario {
                    insufficient: true,
                    ..base.clone()
                },
            ),
            ("insufficient-to-normal", base.clone()),
            (
                "max",
                Scenario {
                    max_selected: true,
                    ..base.clone()
                },
            ),
            ("max-to-normal", base.clone()),
            (
                "fiat",
                Scenario {
                    fiat: true,
                    fiat_editing: true,
                    ..base.clone()
                },
            ),
            ("fiat-to-normal", base.clone()),
            (
                "single-to-batch",
                Scenario {
                    recipients: 2,
                    ..base.clone()
                },
            ),
        ] {
            harness.rebuild(compose(&fixture, &changed));
            assert_eq!(
                harness
                    .probe()
                    .focused()
                    .and_then(|entry| entry.input.as_deref()),
                Some(ADDRESS_A),
                "focus moved during {transition} compose rebuild"
            );
        }
        // No focus command after adding recipient 1: a real edit must still
        // belong to recipient 0, rather than a newly inserted batch label.
        harness.key(keyboard::key::Named::End, false, true);
        let edited = harness.type_character('x');
        assert!(
            matches!(edited.as_slice(), [Message::CreateSpend(CreateSpendMessage::RecipientEdited(0, "address", value))]
                if value == &format!("{ADDRESS_A}x")),
            "batch rebuild lost recipient 0 input ownership: {:?}",
            edited
        );
        harness.rebuild(compose(&fixture, &base));
        assert_eq!(
            harness
                .probe()
                .focused()
                .and_then(|entry| entry.input.as_deref()),
            Some(ADDRESS_A),
            "focus moved during batch-to-single compose rebuild"
        );
    }

    #[test]
    fn warning_and_self_send_rebuilds_keep_mounted_action_focus() {
        let fixture = Fixture::new(ThemeMode::Light, 3);
        for size in [COMPACT, WIDE] {
            let batch = Scenario {
                recipients: 2,
                ..Scenario::valid_send()
            };
            let duplicate = Scenario {
                duplicate: true,
                ..batch.clone()
            };
            let add = Id::new(ADD_RECIPIENT_ID);
            let mut harness = Harness::new(compose(&fixture, &batch), size);
            harness.focus(add.clone());
            let add_bounds = harness
                .probe()
                .focus
                .iter()
                .find(|entry| entry.id.as_ref() == Some(&add))
                .expect("Add recipient is mounted")
                .bounds;
            for scenario in [&duplicate, &batch] {
                harness.rebuild(compose(&fixture, scenario));
                let probe = harness.probe();
                assert_eq!(
                    probe.focused().and_then(|entry| entry.id.as_ref()),
                    Some(&add),
                    "{:?}",
                    size
                );
                assert_eq!(
                    probe
                        .focus
                        .iter()
                        .find(|entry| entry.id.as_ref() == Some(&add))
                        .unwrap()
                        .bounds,
                    add_bounds,
                    "Add recipient moved when duplicate warning changed at {:?}",
                    size
                );
                assert_eq!(
                    probe.contains("Two payment addresses are the same"),
                    scenario.duplicate,
                    "{:?}",
                    size
                );
            }

            // The empty self-send recipient block occupies the same tree slot
            // as the recipient form, preserving the following fee controls.
            let self_send = Scenario {
                recipients: 0,
                ..Scenario::valid_send()
            };
            let fee_preset = preset(24);
            let mut harness = Harness::new(compose(&fixture, &self_send), size);
            harness.focus(fee_preset.clone());
            assert_eq!(harness.focused_id(), Some(fee_preset.clone()), "{:?}", size);
            harness.rebuild(compose(&fixture, &Scenario::valid_send()));
            assert_eq!(
                harness.focused_id(),
                Some(fee_preset.clone()),
                "self-send to send at {:?}",
                size
            );
            harness.rebuild(compose(&fixture, &self_send));
            assert_eq!(
                harness.focused_id(),
                Some(fee_preset),
                "send to self-send at {:?}",
                size
            );
        }
    }

    #[test]
    fn r07c_coin_labels_and_positional_pointer_intents_follow_current_rows() {
        let mut fixture = Fixture::new(ThemeMode::Dark, 3);
        let first = fixture.coins[0].outpoint.to_string();
        fixture.coins_labels.insert(first, "Outpoint label".into());
        fixture.coins_labels.insert(
            fixture.coins[0].outpoint.txid.to_string(),
            "Ignored txid label".into(),
        );
        fixture.coins_labels.insert(
            fixture.coins[1].outpoint.txid.to_string(),
            "Earlier wallet".into(),
        );
        fixture.coins[1].block_height = None;
        fixture.coins[2].spend_info = Some(coincubed::commands::LCSpendInfo {
            txid: Txid::from_str(&format!("{:064x}", 99)).unwrap(),
            height: None,
        });
        fixture.coins.swap(0, 2);
        let fallback_txid = fixture.coins[0].outpoint.txid.to_string();
        let fallback = format!("{}…{}:0", &fallback_txid[..8], &fallback_txid[56..]);
        let scenario = Scenario {
            recipients: 2,
            ..Scenario::valid_send()
        };
        let mut harness = Harness::new(compose(&fixture, &scenario), TALL);
        let probe = harness.probe();
        assert!(probe.contains("Outpoint label") && !probe.contains("Ignored txid label"));
        assert!(probe.contains("From") && probe.contains("Earlier wallet"));
        assert!(probe.contains(&fallback));
        assert!(probe.contains("  Spent  ") && probe.contains("  Unconfirmed  "));
        let fallback_bounds = probe
            .text
            .iter()
            .find(|(text, _)| text == &fallback)
            .map(|(_, bounds)| *bounds)
            .unwrap();
        let checkbox = harness.coin_checkbox_bounds(fallback_bounds);
        let mut messages = harness.press_at(checkbox.center());
        messages.extend(harness.release_at(checkbox.center()));
        assert!(
            matches!(
                messages.as_slice(),
                [Message::CreateSpend(CreateSpendMessage::SelectCoin(0))]
            ),
            "first reordered row checkbox: {:?}",
            messages
        );
        let remove = harness
            .probe()
            .text
            .iter()
            .rfind(|(text, _)| text == "Remove")
            .map(|(_, bounds)| *bounds)
            .unwrap();
        let removed = harness.click_at(remove);
        assert!(
            matches!(
                removed.as_slice(),
                [Message::CreateSpend(CreateSpendMessage::DeleteRecipient(1))]
            ),
            "second recipient Remove: {:?}",
            removed
        );
    }

    #[test]
    fn r07c_max_recovery_and_generating_keep_native_control_truth() {
        use keyboard::key::Named;
        let mut fixture = Fixture::new(ThemeMode::Light, 3);
        fixture.dust_warning = Some("Amount is below the dust limit".into());
        fixture.amounts[0] = value("", true);
        fixture.fiat_amounts[0] = value("2.50", true);
        let max = Scenario {
            max_selected: true,
            max_estimated: Some(400),
            ..Scenario::valid_send()
        };
        let mut harness = Harness::new(compose(&fixture, &max), TALL);
        let probe = harness.probe();
        assert!(probe.contains("MAX") && probe.contains("400"));
        assert_eq!(probe.inputs().len(), 3, "MAX amount is read-only");
        let max_bounds = harness.label_bounds("MAX");
        let clicked = harness.click_at(max_bounds);
        assert!(
            matches!(
                clicked.as_slice(),
                [Message::CreateSpend(
                    CreateSpendMessage::SendMaxToRecipient(0)
                )]
            ),
            "MAX pointer: {:?}",
            clicked
        );

        let unknown = Scenario {
            fiat: true,
            max_estimated: None,
            ..max.clone()
        };
        let mut harness = Harness::new(compose(&fixture, &unknown), TALL);
        let probe = harness.probe();
        assert!(probe.contains("MAX"));
        assert_eq!(
            probe.inputs().len(),
            3,
            "unknown MAX has no editable amount or fiat input"
        );
        assert!(
            !probe.contains("2.50"),
            "unknown MAX has no derived fiat value"
        );

        let recovery = Scenario {
            recovery: true,
            ..Scenario::valid_send()
        };
        let mut harness = Harness::new(compose(&fixture, &recovery), TALL);
        let probe = harness.probe();
        assert!(!probe.contains("MAX") && !probe.contains("Remove"));
        assert_eq!(
            probe.inputs().len(),
            4,
            "recovery retains native amount input"
        );

        let generating = Scenario {
            generating: true,
            ..Scenario::valid_send()
        };
        let mut harness = Harness::new(compose(&fixture, &generating), TALL);
        assert!(!harness.probe().has_focus(&Id::new(PREPARE_ID)));
        let address = harness
            .probe()
            .focus
            .iter()
            .find(|entry| entry.input.as_deref() == Some(ADDRESS_A))
            .map(|entry| entry.bounds)
            .unwrap();
        harness.click_at(address);
        harness.key(Named::End, false, true);
        let edited = harness.type_character(' ');
        assert!(
            matches!(edited.as_slice(), [Message::CreateSpend(CreateSpendMessage::RecipientEdited(0, "address", value))]
            if value == ADDRESS_A),
            "generating address edit: {:?}",
            edited
        );
        let max_bounds = harness.label_bounds("MAX");
        let max_clicked = harness.click_at(max_bounds);
        assert!(
            matches!(
                max_clicked.as_slice(),
                [Message::CreateSpend(
                    CreateSpendMessage::SendMaxToRecipient(0)
                )]
            ),
            "generating MAX: {:?}",
            max_clicked
        );
        let outpoint = fixture.coins[0].outpoint.txid.to_string();
        let fallback = format!("{}…{}:0", &outpoint[..8], &outpoint[56..]);
        let bounds = harness.label_bounds(&fallback);
        let checkbox = harness.coin_checkbox_bounds(bounds);
        let mut selected = harness.press_at(checkbox.center());
        selected.extend(harness.release_at(checkbox.center()));
        assert!(
            matches!(
                selected.as_slice(),
                [Message::CreateSpend(CreateSpendMessage::SelectCoin(0))]
            ),
            "generating coin checkbox: {:?}",
            selected
        );
    }

    #[test]
    fn actions_stay_reachable_around_the_nested_coin_scroll() {
        let fixture = Fixture::new(ThemeMode::Light, 40);
        let scenario = Scenario {
            first_step: false,
            ..Scenario::valid_send()
        };
        let mut harness = Harness::new(compose(&fixture, &scenario), COMPACT);
        let probe = harness.probe();
        assert!(
            probe
                .scrollables
                .iter()
                .any(|(viewport, content)| *viewport <= 300.5 && content > viewport),
            "40 coins overflow the coin list's own scroll: {:?}",
            probe.scrollables
        );
        harness.focus(Id::new(FEERATE_CUSTOM_ID));
        assert_eq!(harness.tab().input.as_deref(), Some("5"));
        for expected in [BACK_ID, CLEAR_ID, PREPARE_ID] {
            assert_eq!(harness.tab().id, Some(Id::new(expected)));
        }
        for _ in 0..2 {
            harness.shift_tab();
        }
        assert_eq!(harness.shift_tab().input.as_deref(), Some("5"));
        assert_eq!(harness.shift_tab().id, Some(Id::new(FEERATE_CUSTOM_ID)));
        assert_eq!(harness.shift_tab().id, Some(preset(1)));
    }

    /// Rendering and pointer motion emit no compose intent. The only message a
    /// redraw may produce is the shared dashboard's content scroll reporting its
    /// initial viewport (`view::Message::Scroll`, from iced
    /// `Scrollable::notify_viewport`): once, and only when the page overflows.
    #[test]
    fn rendering_emits_only_the_dashboard_viewport_notice_and_motion_emits_nothing() {
        let fixture = Fixture::new(ThemeMode::Dark, 3);
        for scenario in [
            Scenario::valid_send(),
            Scenario {
                generating: true,
                ..Scenario::valid_send()
            },
            Scenario {
                loading: Some(6),
                ..Scenario::valid_send()
            },
        ] {
            for outer in [COMPACT, WIDE, TALL] {
                let mut harness = Harness::new(compose(&fixture, &scenario), outer);
                let overflows = harness
                    .probe()
                    .scrollables
                    .iter()
                    .any(|(viewport, content)| content > viewport);
                // Compact and wide pages scroll; the tall window shows it all.
                assert_eq!(overflows, outer != TALL, "{:?}", outer);
                let redraw = || {
                    Event::Window(iced::window::Event::RedrawRequested(
                        std::time::Instant::now(),
                    ))
                };
                let first = harness.send(redraw(), mouse::Cursor::Unavailable);
                if overflows {
                    assert!(
                        matches!(first.as_slice(), [Message::Scroll(offset)] if *offset == 0.0),
                        "{:?}: {:?}",
                        outer,
                        first
                    );
                } else {
                    assert!(first.is_empty(), "{:?}: {:?}", outer, first);
                }
                let second = harness.send(redraw(), mouse::Cursor::Unavailable);
                assert!(second.is_empty(), "{:?}: {:?}", outer, second);
                let bounds = harness.probe().focus[0].bounds;
                let moved = harness.send(
                    Event::Mouse(mouse::Event::CursorMoved {
                        position: bounds.center(),
                    }),
                    mouse::Cursor::Available(bounds.center()),
                );
                assert!(moved.is_empty(), "{:?}: {:?}", outer, moved);
            }
        }
    }
}
