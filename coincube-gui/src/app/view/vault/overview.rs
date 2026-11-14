use chrono::{DateTime, Local, Utc};
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::locale::Text as CopyText;
use coincube_ui::localized_format;
use std::{collections::HashMap, time::Duration, vec};

use iced::{
    widget::{Container, Row, Space},
    Alignment::{self, Center},
    Length,
};

use coincube_core::miniscript::bitcoin;
use coincube_ui::component::tenshu::{asset_row, AmountVisibility, BalanceValue, Figure};
use coincube_ui::theme::tenshu::size;
use coincube_ui::{
    component::{amount::*, button, card, form, spinner, text::*},
    icon::{self, cross_icon},
    theme,
    widget::{Button, Column, ColumnExt, Element},
};

use crate::{
    app::{
        cache::Cache,
        menu::{self, Menu, VaultSubMenu},
        settings::display::DisplayMode,
        view::{
            dashboard_headed,
            message::Message,
            shared::landing::{landing_balance_section, landing_button, landing_control_id},
            vault::coins,
            vault::label,
            vault_page_head,
            wallet_header::{wallet_header, HeaderVariant, SyncState, WalletHeaderProps},
            FiatAmountConverter,
        },
        wallet::SyncStatus,
    },
    daemon::model::{HistoryTransaction, TransactionKind},
};

const RESCAN_DATE_PROMPT: &str = "This Vault was restored from a Recovery Kit that doesn't record when the wallet was created, so its past transactions can't be found without a start date. Pick one to scan the blockchain from.";

fn rescan_date_prompt<'a>(cube_id: &str) -> Element<'a, Message> {
    Container::new(
        Column::new()
            .spacing(10)
            .push(
                Row::new()
                    .spacing(5)
                    .push(icon::warning_icon().style(theme::text::warning))
                    .push(text(RESCAN_DATE_PROMPT).style(theme::text::warning))
                    .align_y(Center),
            )
            .push(
                Row::new()
                    .spacing(5)
                    .push(Space::new().width(Length::Fill))
                    .push(landing_button(
                        landing_control_id(cube_id, "Vault", "rescan-pick-date"),
                        localized_copy("Pick a date"),
                        None,
                        Message::FocusAction,
                        Some(Message::Menu(Menu::Vault(menu::VaultSubMenu::Settings(
                            Some(menu::SettingsOption::Node),
                        )))),
                        theme::button::secondary,
                    ))
                    .push(landing_button(
                        landing_control_id(cube_id, "Vault", "rescan-dismiss"),
                        localized_copy("Dismiss"),
                        Some(cross_icon()),
                        Message::FocusAction,
                        Some(Message::HideRescanPrompt),
                        theme::button::secondary,
                    )),
            ),
    )
    .padding(25)
    .style(theme::card::border)
    .into()
}

/// How the balance header should present the Vault's balance.
///
/// An empty balance is only shown as an amount when the Vault knows its history.
/// While a full scan is still loading it, or syncing keeps failing before it has
/// loaded, or a restore is waiting on a rescan date, the empty database says nothing
/// about the funds — and a confident "$0.00" is exactly what led a user with a
/// funded, restored Vault to believe the funds were gone.
pub(crate) fn balance_sync_state(
    sync_status: &SyncStatus,
    total_balance: bitcoin::Amount,
    awaiting_rescan_date: bool,
) -> SyncState {
    let empty = total_balance == bitcoin::Amount::ZERO;
    let awaiting_label = localized_copy(
        "Balance unknown until this Vault's history is scanned. Pick the date it was created to start.",
    );
    match sync_status {
        // However far the node or the wallet has synced, a restore awaiting its
        // rescan date has no history: its zero is not a balance.
        SyncStatus::Synced | SyncStatus::LatestWalletSync if empty && awaiting_rescan_date => {
            SyncState::Unknown {
                progress: None,
                label: awaiting_label.to_string(),
                failing: true,
            }
        }
        SyncStatus::BlockchainSync(progress) if empty && awaiting_rescan_date => {
            SyncState::Unknown {
                progress: Some(*progress),
                label: localized_copy(
                    "Syncing blockchain; the balance stays unknown until this Vault's history is scanned",
                ).to_owned(),
                failing: false,
            }
        }
        SyncStatus::Synced => SyncState::Synced,
        SyncStatus::BlockchainSync(progress) => SyncState::Syncing {
            progress: Some(*progress),
            label: localized_copy("Syncing blockchain").to_owned(),
        },
        SyncStatus::WalletFullScan { progress } if empty => SyncState::Unknown {
            progress: *progress,
            label: localized_copy("Loading Vault history").to_owned(),
            failing: false,
        },
        SyncStatus::WalletFullScan { progress } => SyncState::Syncing {
            progress: *progress,
            label: localized_copy("Syncing").to_owned(),
        },
        SyncStatus::LatestWalletSync => SyncState::Checking,
        SyncStatus::SyncFailing {
            message,
            since,
            retry_progress,
        } if empty => SyncState::Unknown {
            progress: None,
            label: localized_format!(
                "Balance unavailable: syncing has failed since {}{}. {}",
                "Saldo no disponible. La sincronización falla desde {}{}. {}",
                format_failure_time(*since),
                retry_note(*retry_progress),
                failure_detail(message),
            ),
            failing: true,
        },
        SyncStatus::SyncFailing {
            message,
            since,
            retry_progress,
        } => SyncState::Stale {
            message: localized_format!(
                "Last known balance: syncing has failed since {}{}. {}",
                "Último saldo conocido. La sincronización falla desde {}{}. {}",
                format_failure_time(*since),
                retry_note(*retry_progress),
                failure_detail(message),
            ),
        },
    }
}

/// ", retrying (x%)" while a full scan is retrying after failures, so a retry that
/// is getting somewhere does not read as failed throughout.
fn retry_note(retry_progress: Option<f64>) -> String {
    retry_progress
        .map(|p| localized_format!(", retrying ({:.1}%)", ", reintentando ({:.1}%)", 100.0 * p))
        .unwrap_or_default()
}

/// When a run of failed polls began, in local time.
fn format_failure_time(since: u32) -> String {
    DateTime::<Utc>::from_timestamp(i64::from(since), 0)
        .map(|at| {
            let local = at.with_timezone(&Local);
            local
                .format(&coincube_ui::locale::date_pattern("%b %-d, %H:%M", &local))
                .to_string()
        })
        .unwrap_or_else(|| localized_copy("recently").to_owned())
}

/// The daemon's failure message, trimmed to fit under the balance. The daemon log
/// keeps the full text.
fn failure_detail(message: &str) -> String {
    const MAX_CHARS: usize = 160;
    let message = message.trim();
    if message.chars().count() <= MAX_CHARS {
        return message.to_string();
    }
    let truncated: String = message.chars().take(MAX_CHARS).collect();
    format!("{}…", truncated.trim_end())
}

/// One item of the Vault page's protection strip (W3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProtectionItem {
    /// Coins whose recovery path opens soon, with the copy that says so.
    Recovery(String),
    /// PSBTs still waiting for signatures, with the copy that says so.
    Psbts(String),
}

/// The strip's items: each only when its count is above zero.
///
/// - Recovery: the coins `coins_summary` flags as expiring, and the
///   soonest of their recovery paths, `remaining_sequence` blocks away. The
///   day count rounds up, so a path a few blocks away never reads "0 days".
/// - PSBTs: the proposals PSBT proposals shows as "Not signed" or "Partially
///   signed" ([`waiting_for_signatures`]).
pub(crate) fn protection_items(
    expiring_coins: usize,
    remaining_sequence: Option<u32>,
    psbts_waiting: usize,
) -> Vec<ProtectionItem> {
    let plural =
        |n: usize, one: &str, many: &str| format!("{} {}", n, if n == 1 { one } else { many });
    let recovery = (expiring_coins > 0).then(|| {
        let coins = plural(expiring_coins, "coin", "coins");
        ProtectionItem::Recovery(match remaining_sequence.map(coins::blocks_to_days_ceil) {
            Some(0) | None => format!("Recovery path is open for {}", coins),
            Some(days) => format!(
                "Recovery path opens in {} for {}",
                plural(days as usize, "day", "days"),
                coins
            ),
        })
    });
    let psbts = (psbts_waiting > 0).then(|| {
        ProtectionItem::Psbts(format!(
            "{} waiting for signatures",
            plural(psbts_waiting, "PSBT", "PSBTs")
        ))
    });
    recovery.into_iter().chain(psbts).collect()
}

/// PSBTs waiting for signatures: pending, with the primary path short of
/// its threshold. The same split PSBT proposals uses for "Not signed" and
/// "Partially signed" (`view::vault::psbts`), so the two pages agree.
pub(crate) fn waiting_for_signatures(txs: &[crate::daemon::model::SpendTx]) -> usize {
    txs.iter()
        .filter(|tx| {
            tx.status == crate::daemon::model::SpendStatus::Pending && tx.path_ready().is_none()
        })
        .count()
}

fn protection_strip<'a>(
    cube_id: &str,
    expiring_coins: &[bitcoin::OutPoint],
    remaining_sequence: Option<u32>,
    psbts_waiting: usize,
) -> Option<Element<'a, Message>> {
    let items = protection_items(expiring_coins.len(), remaining_sequence, psbts_waiting);
    if items.is_empty() {
        return None;
    }
    let column = items
        .into_iter()
        .fold(Column::new().spacing(12), |column, item| {
            let (copy, control, label, route, icon) = match item {
                ProtectionItem::Recovery(copy) => (
                    copy,
                    "refresh-coins",
                    "Refresh coins",
                    Menu::Vault(VaultSubMenu::Coins(Some(expiring_coins.to_owned()))),
                    Some(icon::arrow_repeat()),
                ),
                ProtectionItem::Psbts(copy) => (
                    copy,
                    "review-psbts",
                    CopyText::Review.get(),
                    Menu::Vault(VaultSubMenu::PSBTs(None)),
                    None,
                ),
            };
            column.push(
                Row::new()
                    .spacing(15)
                    .align_y(Alignment::Center)
                    .push(text(copy).size(P1_SIZE).width(Length::Fill))
                    .push(landing_button(
                        landing_control_id(cube_id, "Vault", control),
                        label,
                        icon,
                        Message::FocusAction,
                        Some(Message::Menu(route)),
                        theme::button::secondary,
                    )),
            )
        });
    Some(
        Container::new(column)
            .padding(20)
            .width(Length::Fill)
            .style(theme::card::border)
            .into(),
    )
}

/// An empty database, hidden amount, or test network must never earn a
/// balance-based voice line.
fn empty_vault_voice(
    chain: crate::chain::ChainId,
    balance: bitcoin::Amount,
    sync: &SyncState,
    masked: bool,
) -> Option<&'static str> {
    use crate::chain::ChainId;
    use coincube_ui::worldview_copy::VoiceLine;

    if masked || balance != bitcoin::Amount::ZERO || !matches!(sync, SyncState::Synced) {
        return None;
    }
    match chain {
        ChainId::Bitcoin => VoiceLine::EmptySha256Vault.get(),
        ChainId::BitcoinBlake2b => VoiceLine::EmptyBlake2bVault.get(),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn vault_overview_view<'a>(
    balance: &'a bitcoin::Amount,
    unconfirmed_balance: &'a bitcoin::Amount,
    remaining_sequence: &Option<u32>,
    fiat_converter: Option<FiatAmountConverter>,
    expiring_coins: &[bitcoin::OutPoint],
    recent_activity: Option<Element<'a, Message>>,
    cube_id: &str,
    chain: crate::chain::ChainId,
    sync_status: &SyncStatus,
    show_rescan_prompt: bool,
    awaiting_rescan_date: bool,
    bitcoin_unit: BitcoinDisplayUnit,
    node_bitcoind_sync_progress: Option<f64>,
    node_bitcoind_ibd: Option<bool>,
    _show_direction_badges: bool,
    display_mode: DisplayMode,
    mask_amounts: bool,
    psbts_waiting: usize,
    alert: Option<&crate::app::VaultAlert>,
) -> Element<'a, Message> {
    // Show external-unconfirmed coins as part of the headline balance
    // (matching the Cube → Overview Vault card). The per-transaction
    // "Unconfirmed" pill in the recent-payments list below still
    // surfaces which inputs are pending — we deliberately omit the
    // `wallet_header` "+ X SATS unconfirmed" breakout here because
    // its leading `+` reads as additive next to an already-inclusive
    // headline.
    let total_balance = *balance + *unconfirmed_balance;
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(total_balance));
    let mut sync = balance_sync_state(sync_status, total_balance, awaiting_rescan_date);
    // The Vault's alert names why it stopped and when its figures are from,
    // in the same words as the Connect panel and Vault › Connections.
    if let (Some(alert), SyncState::Stale { message }) = (alert, &mut sync) {
        *message = format!("{} · {}. {}", alert.word(), alert.title(), alert.detail());
    }
    let history_unknown = matches!(sync, SyncState::Unknown { .. });
    let voice = empty_vault_voice(chain, total_balance, &sync, mask_amounts || alert.is_some());
    let primary = Figure::new(total_balance.to_formatted_string_with_unit(bitcoin_unit))
        .unit(bitcoin_unit.label());
    let secondary = fiat_balance
        .as_ref()
        .map(|f| Figure::new(f.to_formatted_string()).unit(f.currency().to_string()));
    let vault_btc_row = asset_row(
        coincube_ui::image::asset_network_logo::<Message>("btc", "bitcoin", size::COIN),
        "Bitcoin",
        if history_unknown {
            BalanceValue::Pending {
                label: localized_copy("Vault history has not loaded").into(),
            }
        } else if total_balance == bitcoin::Amount::ZERO {
            BalanceValue::Zero { primary, secondary }
        } else {
            BalanceValue::Known { primary, secondary }
        },
        if mask_amounts {
            AmountVisibility::Hidden
        } else {
            AmountVisibility::Shown
        },
        [
            landing_button(
                landing_control_id(cube_id, "Vault", "send-btc"),
                localized_copy("Send"),
                None,
                Message::FocusAction,
                Some(Message::Menu(Menu::Vault(VaultSubMenu::Send))),
                theme::button::primary,
            )
            .width(Length::Fixed(90.0))
            .into(),
            landing_button(
                landing_control_id(cube_id, "Vault", "receive-btc"),
                localized_copy("Receive"),
                None,
                Message::FocusAction,
                Some(Message::Menu(Menu::Vault(VaultSubMenu::Receive))),
                theme::button::orange_outline,
            )
            .width(Length::Fixed(90.0))
            .into(),
        ],
    );
    Container::new(
        Column::new()
            .push(landing_balance_section(
                Column::new()
                    .spacing(16)
                    .push(
                        Column::new()
                            .spacing(8)
                            .push(h4_bold(localized_copy("Total balance")))
                            .push(wallet_header::<Message>(WalletHeaderProps {
                                sats: total_balance,
                                fiat: fiat_balance,
                                balance_masked: mask_amounts,
                                bitcoin_unit,
                                variant: HeaderVariant::Landing,
                                sync,
                                unconfirmed: None,
                                pending_send_sats: 0,
                                pending_receive_sats: 0,
                                display_mode,
                                on_swap: Some(Message::FlipDisplayMode),
                                on_swap_focus: Some((
                                    landing_control_id(cube_id, "Vault", "flip-balance"),
                                    Message::FocusAction,
                                )),
                            })),
                    )
                    .push(iced::widget::rule::horizontal(1))
                    .push(vault_btc_row)
                    .push_maybe(voice.map(|line| p2_regular(line).style(theme::text::secondary))),
            ))
            .push(show_rescan_prompt.then_some(rescan_date_prompt(cube_id)))
            .push(match (node_bitcoind_ibd, node_bitcoind_sync_progress) {
                (Some(true), Some(progress)) => Some(
                    Container::new(
                        Row::new()
                            .spacing(10)
                            .align_y(Alignment::Center)
                            .push(
                                text(localized_format!(
                                    "Your local node is syncing — {:.1}% complete",
                                    "Tu nodo local se está sincronizando. Progreso: {:.1}%",
                                    crate::utils::syncing_percent(progress)
                                ))
                                .style(theme::text::secondary)
                                .width(Length::Fill),
                            )
                            .push(spinner::typing_text_carousel(
                                "...",
                                true,
                                Duration::from_millis(2000),
                                |content| text(content).style(theme::text::secondary),
                            ))
                            .width(Length::Fill),
                    )
                    .padding(15)
                    .style(theme::card::border),
                ),
                _ => None,
            })
            .push_maybe(protection_strip(
                cube_id,
                expiring_coins,
                *remaining_sequence,
                psbts_waiting,
            ))
            // "Recent activity": App mounts Activity's rows for the Vault
            // (`view::activity::recent_activity`).
            .push_maybe(recent_activity)
            .push(Space::new().height(Length::Fixed(40.0)))
            .spacing(20),
    )
    .into()
}

pub fn payment_view<'a>(
    menu: &'a Menu,
    cache: &'a Cache,
    tx: &'a HistoryTransaction,
    output_index: usize,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let txid = tx.tx.compute_txid().to_string();
    let outpoint = bitcoin::OutPoint {
        txid: tx.tx.compute_txid(),
        vout: output_index as u32,
    }
    .to_string();
    let title =
        match tx.kind {
            TransactionKind::OutgoingSinglePayment(_)
            | TransactionKind::OutgoingPaymentBatch(_) => localized_copy("Outgoing payment"),
            TransactionKind::IncomingSinglePayment(_)
            | TransactionKind::IncomingPaymentBatch(_) => localized_copy("Incoming payment"),
            _ => localized_copy("Payment"),
        };
    dashboard_headed(
        menu,
        cache,
        vault_page_head(cache, title),
        Column::new()
            .push(if tx.is_single_payment().is_some() {
                // if the payment is a payment of a single payment transaction then
                // the label of the transaction is attached to the label of the payment outpoint
                if let Some(label) = labels_editing.get(&outpoint) {
                    label::label_editing(vec![outpoint.clone(), txid.clone()], label, H3_SIZE)
                } else {
                    label::label_editable(
                        vec![outpoint.clone(), txid.clone()],
                        tx.labels.get(&outpoint),
                        H3_SIZE,
                    )
                }
            } else if let Some(label) = labels_editing.get(&outpoint) {
                label::label_editing(vec![outpoint.clone()], label, H3_SIZE)
            } else {
                label::label_editable(vec![outpoint.clone()], tx.labels.get(&outpoint), H3_SIZE)
            })
            .push(Container::new(if cache.balance_amounts_masked() {
                Row::new().push(text("••••••").size(H3_SIZE))
            } else {
                amount_with_size(&tx.tx.output[output_index].value, H3_SIZE)
            }))
            .push(Space::new().height(H3_SIZE))
            .push(Container::new(h3(localized_copy("Transaction"))).width(Length::Fill))
            .push(if tx.is_batch() {
                if let Some(label) = labels_editing.get(&txid) {
                    Some(label::label_editing(vec![txid.clone()], label, H3_SIZE))
                } else {
                    Some(label::label_editable(
                        vec![txid.clone()],
                        tx.labels.get(&txid),
                        H3_SIZE,
                    ))
                }
            } else {
                None
            })
            .push(tx.fee_amount.map(|fee_amount| {
                Row::new()
                    .align_y(Alignment::Center)
                    .push(h3(localized_copy("Miner fee: ")).style(theme::text::secondary))
                    .push(if cache.balance_amounts_masked() {
                        Row::new().push(text("••••••").size(H3_SIZE))
                    } else {
                        amount_with_size(&fee_amount, H3_SIZE)
                    })
                    .push(text(" ").size(H3_SIZE))
                    .push(
                        text(if cache.balance_amounts_masked() {
                            "••••••".to_string()
                        } else {
                            format!(
                                "({} sats/vbyte)",
                                fee_amount.to_sat() / tx.tx.vsize() as u64
                            )
                        })
                        .size(H4_SIZE)
                        .style(theme::text::secondary),
                    )
            }))
            .push(card::simple(
                Column::new()
                    .push(tx.time.map(|t| {
                        let local = DateTime::<Utc>::from_timestamp(t as i64, 0)
                            .unwrap()
                            .with_timezone(&Local);
                        let date = local
                            .format(&coincube_ui::locale::date_pattern(
                                "%b. %d, %Y - %T",
                                &local,
                            ))
                            .to_string();
                        Row::new()
                            .width(Length::Fill)
                            .push(
                                Container::new(text(localized_copy("Date:")).bold())
                                    .width(Length::Fill),
                            )
                            .push(Container::new(text(date)).width(Length::Shrink))
                    }))
                    .push(
                        Row::new()
                            .width(Length::Fill)
                            .align_y(Alignment::Center)
                            .push(Container::new(text("Txid:").bold()).width(Length::Fill))
                            .push(
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .push(Container::new(
                                        text(format!("{}", tx.tx.compute_txid())).small(),
                                    ))
                                    .push(
                                        Button::new(icon::clipboard_icon())
                                            .on_press(Message::Clipboard(
                                                tx.tx.compute_txid().to_string(),
                                            ))
                                            .style(theme::button::transparent_border),
                                    )
                                    .width(Length::Shrink),
                            ),
                    )
                    .spacing(5),
            ))
            .push(
                button::secondary(None, localized_copy("See transaction details")).on_press(
                    Message::Menu(Menu::Vault(VaultSubMenu::Transactions(Some(
                        tx.tx.compute_txid(),
                    )))),
                ),
            )
            .spacing(20),
    )
}

/// Full-screen celebration view when a vault payment is received.
pub fn received_celebration_page<'a>(
    context: &str,
    amount_display: &'a str,
    quote: &'a coincube_ui::component::quote_display::Quote,
    image_handle: &'a iced::widget::image::Handle,
) -> Element<'a, Message> {
    use coincube_ui::component::payment_result::{payment_result, Outcome};
    payment_result(
        Outcome::Received,
        amount_display,
        context,
        quote,
        image_handle,
        Message::DismissReceivedCelebration,
    )
    .detail(localized_copy("Into Vault"))
    .voice(coincube_ui::worldview_copy::VoiceLine::PaymentReceived.get())
    .on_activity(Message::ReceivedCelebrationActivity)
    .into()
}

#[cfg(test)]
mod balance_state_tests {
    use super::{
        balance_sync_state, coins, empty_vault_voice, failure_detail, protection_items,
        ProtectionItem,
    };
    use crate::app::{view::wallet_header::SyncState, wallet::SyncStatus};
    use coincube_core::miniscript::bitcoin::Amount;

    #[test]
    fn worldview_empty_vault_copy_requires_a_visible_synced_zero_on_the_exact_mainnet() {
        use crate::chain::ChainId;
        use coincube_ui::{theme::WorldviewMode, worldview_copy};

        let zero = Amount::ZERO;
        let visible = false;
        let synced = SyncState::Synced;
        let waiting = balance_sync_state(&SyncStatus::Synced, zero, true);
        worldview_copy::render_in(WorldviewMode::Luke, || {
            assert_eq!(
                empty_vault_voice(ChainId::Bitcoin, zero, &synced, visible),
                Some("No Spamcoin detected. Excellent.")
            );
            assert_eq!(
                empty_vault_voice(ChainId::Bitcoin, zero, &synced, true),
                None
            );
            assert_eq!(
                empty_vault_voice(ChainId::Bitcoin, zero, &waiting, visible),
                None
            );
            assert_eq!(
                empty_vault_voice(ChainId::Bitcoin, zero, &SyncState::Checking, visible),
                None
            );
            assert_eq!(
                empty_vault_voice(ChainId::Bitcoin, Amount::from_sat(1), &synced, visible),
                None
            );
            assert_eq!(
                empty_vault_voice(ChainId::Testnet4, zero, &synced, visible),
                None
            );
        });
        worldview_copy::render_in(WorldviewMode::Suitcoiner, || {
            assert_eq!(
                empty_vault_voice(ChainId::BitcoinBlake2b, zero, &synced, visible),
                Some("Your portfolio remains institutionally compliant.")
            );
            assert_eq!(
                empty_vault_voice(ChainId::BitcoinBlake2bTestnet4, zero, &synced, visible),
                None
            );
        });
        assert_eq!(
            empty_vault_voice(ChainId::Bitcoin, zero, &synced, visible),
            None
        );
    }

    /// The incident: a restored Vault's history was still loading and the header said
    /// "$0.00" as though that were its balance. An empty balance during a full scan is
    /// now shown as unknown.
    #[test]
    fn an_empty_balance_is_unknown_while_the_history_loads() {
        let state = balance_sync_state(
            &SyncStatus::WalletFullScan {
                progress: Some(0.25),
            },
            Amount::ZERO,
            false,
        );
        assert!(matches!(
            state,
            SyncState::Unknown {
                progress: Some(p),
                failing: false,
                ..
            } if p == 0.25
        ));
    }

    /// A Vault that already holds coins keeps showing them, pulsing, while it rescans.
    #[test]
    fn a_known_balance_stays_visible_while_rescanning() {
        let state = balance_sync_state(
            &SyncStatus::WalletFullScan { progress: None },
            Amount::from_sat(1),
            false,
        );
        assert!(matches!(state, SyncState::Syncing { .. }));
    }

    #[test]
    fn failing_sync_hides_an_empty_balance_and_flags_a_known_one() {
        let failing = SyncStatus::SyncFailing {
            message: "Esplora client error".into(),
            since: 0,
            retry_progress: None,
        };
        match balance_sync_state(&failing, Amount::ZERO, false) {
            SyncState::Unknown {
                failing: true,
                label,
                ..
            } => assert!(label.contains("Esplora client error"), "{}", label),
            _ => panic!("an empty balance must not be shown while syncing fails"),
        }
        match balance_sync_state(&failing, Amount::from_sat(5), false) {
            SyncState::Stale { message } => {
                assert!(message.starts_with("Last known balance"), "{}", message)
            }
            _ => panic!("a known balance must be flagged as possibly stale"),
        }
    }

    /// A restore still waiting for a rescan date has no history at all: its "0" is
    /// not a balance either.
    #[test]
    fn a_vault_awaiting_its_rescan_date_has_no_balance_to_show() {
        assert!(matches!(
            balance_sync_state(&SyncStatus::Synced, Amount::ZERO, true),
            SyncState::Unknown { failing: true, .. }
        ));
        assert!(matches!(
            balance_sync_state(&SyncStatus::Synced, Amount::from_sat(5), true),
            SyncState::Synced
        ));
        assert!(matches!(
            balance_sync_state(&SyncStatus::Synced, Amount::ZERO, false),
            SyncState::Synced
        ));
        // Nor while the node or the wallet is still syncing: the blockchain's
        // progress is kept, but no zero is shown.
        assert!(matches!(
            balance_sync_state(&SyncStatus::BlockchainSync(0.4), Amount::ZERO, true),
            SyncState::Unknown {
                progress: Some(p),
                failing: false,
                ..
            } if p == 0.4
        ));
        assert!(matches!(
            balance_sync_state(&SyncStatus::LatestWalletSync, Amount::ZERO, true),
            SyncState::Unknown { failing: true, .. }
        ));
        // A funded Vault keeps showing its coins while syncing.
        assert!(matches!(
            balance_sync_state(&SyncStatus::BlockchainSync(0.4), Amount::from_sat(5), true),
            SyncState::Syncing { .. }
        ));
        assert!(matches!(
            balance_sync_state(&SyncStatus::LatestWalletSync, Amount::from_sat(5), true),
            SyncState::Checking
        ));
    }

    #[test]
    fn a_retrying_scan_shows_its_progress_alongside_the_failure() {
        let retrying = SyncStatus::SyncFailing {
            message: "Esplora client error".into(),
            since: 0,
            retry_progress: Some(0.5),
        };
        match balance_sync_state(&retrying, Amount::ZERO, false) {
            SyncState::Unknown { label, .. } => {
                assert!(label.contains("retrying (50.0%)"), "{}", label)
            }
            _ => panic!("an empty balance must not be shown while syncing fails"),
        }
    }

    /// Each item shows only when its count is above zero.
    #[test]
    fn the_protection_strip_shows_each_item_only_when_its_count_is_above_zero() {
        let kinds = |expiring, psbts| {
            protection_items(expiring, Some(1_440), psbts)
                .into_iter()
                .map(|item| match item {
                    ProtectionItem::Recovery(_) => "recovery",
                    ProtectionItem::Psbts(_) => "psbts",
                })
                .collect::<Vec<_>>()
        };
        assert!(kinds(0, 0).is_empty());
        assert_eq!(kinds(2, 0), ["recovery"]);
        assert_eq!(kinds(0, 1), ["psbts"]);
        assert_eq!(kinds(2, 1), ["recovery", "psbts"]);
    }

    #[test]
    fn protection_copy_pluralises_coins_days_and_psbts() {
        // 144 blocks is one day; 1,440 is ten.
        assert_eq!(
            protection_items(1, Some(144), 1),
            [
                ProtectionItem::Recovery("Recovery path opens in 1 day for 1 coin".into()),
                ProtectionItem::Psbts("1 PSBT waiting for signatures".into()),
            ]
        );
        assert_eq!(
            protection_items(3, Some(1_440), 2),
            [
                ProtectionItem::Recovery("Recovery path opens in 10 days for 3 coins".into()),
                ProtectionItem::Psbts("2 PSBTs waiting for signatures".into()),
            ]
        );
        // A path that has opened says so.
        assert_eq!(
            protection_items(2, Some(0), 0),
            [ProtectionItem::Recovery(
                "Recovery path is open for 2 coins".into()
            )]
        );
    }

    /// Rounds up, sharing the block length `expire_message_units` uses.
    #[test]
    fn recovery_days_round_up() {
        assert_eq!(coins::blocks_to_days_ceil(0), 0);
        assert_eq!(
            coins::blocks_to_days_ceil(1),
            1,
            "a block away is a day away"
        );
        assert_eq!(coins::blocks_to_days_ceil(144), 1);
        assert_eq!(coins::blocks_to_days_ceil(145), 2);
        assert_eq!(coins::blocks_to_days_ceil(4_320), 30);
        assert_eq!(coins::expire_message_units(4_320), ["30 days"]);
    }

    #[test]
    fn long_failure_messages_are_trimmed() {
        let long = "x".repeat(500);
        let detail = failure_detail(&long);
        assert_eq!(detail.chars().count(), 161);
        assert!(detail.ends_with('…'));
        assert_eq!(failure_detail("  short  "), "short");
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;
    use crate::app::view::shared::landing::capture;

    /// The Vault page under its head, with both protection items (W3).
    /// Opt-in: `TOP_BAR_CAPTURE_DIR=<dir> cargo test -p coincube-gui --lib
    /// page_head_capture_vault -- --ignored`.
    #[test]
    #[ignore = "writes PNG frames; needs a GPU adapter"]
    fn page_head_capture_vault() {
        use crate::app::state::preferences::BalancePrivacy;
        use crate::app::view::nav::shell::capture_tests;
        let Some(directory) = capture_tests::directory() else {
            return;
        };
        let mut renderer = capture_tests::renderer();
        let menu = Menu::Vault(VaultSubMenu::Overview);
        let balance = bitcoin::Amount::from_sat(1_250_000);
        let zero = bitcoin::Amount::ZERO;
        let expiring = [bitcoin::OutPoint::null()];
        for (appearance, palette) in capture_tests::themes() {
            let mut cache = capture_tests::cache(appearance == "light", BalancePrivacy::Shown);
            cache.has_vault = true;
            let cache = &cache;
            let (balance, zero, expiring) = (&balance, &zero, &expiring);
            capture_tests::save_page_head_frames(
                &mut renderer,
                &directory,
                "vault",
                appearance,
                &palette,
                || {
                    crate::app::view::dashboard_with_detail(
                        &menu,
                        cache,
                        move || {
                            crate::app::state::vault::overview::VaultOverview::page_head_for_tests(
                                cache,
                                Some(crate::app::view::global_home::WalletTransferEntry::Ready),
                            )
                        },
                        move || {
                            vault_overview_view(
                                balance,
                                zero,
                                &Some(1_440),
                                None,
                                expiring,
                                None,
                                "synthetic-cube",
                                crate::chain::ChainId::Bitcoin,
                                &SyncStatus::Synced,
                                false,
                                false,
                                BitcoinDisplayUnit::Sats,
                                None,
                                None,
                                true,
                                DisplayMode::BitcoinNative,
                                false,
                                1,
                                None,
                            )
                        },
                        || None,
                    )
                },
            );
        }
    }

    #[test]
    fn hidden_overview_redacts_balance() {
        let balance = bitcoin::Amount::from_sat(12_345_678);
        let zero = bitcoin::Amount::ZERO;
        let mounted = capture::inspect(
            "vault-hidden-amounts",
            vault_overview_view(
                &balance,
                &zero,
                &None,
                None,
                &[],
                None,
                "synthetic-cube",
                crate::chain::ChainId::Bitcoin,
                &SyncStatus::Synced,
                false,
                false,
                BitcoinDisplayUnit::Sats,
                None,
                None,
                true,
                DisplayMode::BitcoinNative,
                true,
                0,
                None,
            ),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(mounted.contains("••••••"));
        assert!(!mounted.contains("12,345,678"));
    }

    #[test]
    fn r06a_vault_headless_content_matrix() {
        let balance = bitcoin::Amount::from_sat(250_000);
        let zero = bitcoin::Amount::ZERO;
        let none_sequence = None;
        let failing = SyncStatus::SyncFailing {
            message: "Esplora refresh failed".into(),
            since: 1_722_000_000,
            retry_progress: Some(0.5),
        };
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                macro_rules! render {
                    ($balance:expr, $sync:expr, $rescan:expr $(,)?) => {
                        vault_overview_view(
                            $balance,
                            &zero,
                            &none_sequence,
                            None,
                            &[],
                            None,
                            "synthetic-cube",
                            crate::chain::ChainId::Bitcoin,
                            $sync,
                            $rescan,
                            $rescan,
                            BitcoinDisplayUnit::Sats,
                            None,
                            None,
                            true,
                            DisplayMode::BitcoinNative,
                            false,
                            0,
                            None,
                        )
                    };
                }
                let normal = capture::inspect(
                    "vault-normal",
                    render!(&balance, &SyncStatus::Synced, false),
                    outer,
                    appearance,
                    theme,
                );
                for control in ["flip-balance", "send-btc", "receive-btc"] {
                    assert!(normal.has_focus(landing_control_id(
                        "synthetic-cube",
                        "Vault",
                        control
                    )));
                }
                assert!(normal.contains("Total balance"));
                assert!(normal.contains("Bitcoin"));
                // No counts, no strip.
                assert!(!normal.contains("Recovery path"));
                assert!(!normal.contains("waiting for signatures"));

                let unfinished = capture::inspect(
                    "vault-rescan-date-unknown",
                    render!(
                        &zero,
                        &SyncStatus::WalletFullScan {
                            progress: Some(0.25),
                        },
                        true,
                    ),
                    outer,
                    appearance,
                    theme,
                );
                assert!(unfinished.contains("Pick a date"));
                assert!(unfinished.contains("Vault history has not loaded"));

                let stale = capture::inspect(
                    "vault-retained-success-error",
                    render!(&balance, &failing, false),
                    outer,
                    appearance,
                    theme,
                );
                assert!(stale.contains("Last known balance"));
            }
        }
    }

    /// The strip renders both items with their actions, which open Coin
    /// control on the expiring coins and PSBT proposals.
    #[test]
    fn the_protection_strip_opens_coins_and_psbts() {
        let balance = bitcoin::Amount::from_sat(250_000);
        let zero = bitcoin::Amount::ZERO;
        let expiring = [bitcoin::OutPoint::null()];
        let view = || {
            vault_overview_view(
                &balance,
                &zero,
                &Some(1_440),
                None,
                &expiring,
                None,
                "synthetic-cube",
                crate::chain::ChainId::Bitcoin,
                &SyncStatus::Synced,
                false,
                false,
                BitcoinDisplayUnit::Sats,
                None,
                None,
                true,
                DisplayMode::BitcoinNative,
                false,
                2,
                None,
            )
        };
        let mounted = capture::inspect(
            "vault-strip",
            view(),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(mounted.contains("Recovery path opens in 10 days for 1 coin"));
        assert!(mounted.contains("2 PSBTs waiting for signatures"));
        for (control, route) in [
            (
                "refresh-coins",
                Menu::Vault(VaultSubMenu::Coins(Some(expiring.to_vec()))),
            ),
            ("review-psbts", Menu::Vault(VaultSubMenu::PSBTs(None))),
        ] {
            let emitted = capture::activate(
                view(),
                landing_control_id("synthetic-cube", "Vault", control),
                (1440, 900),
                true,
            );
            assert!(
                emitted
                    .iter()
                    .any(|message| matches!(message, Message::Menu(m) if *m == route)),
                "{}",
                control
            );
        }
    }
}
