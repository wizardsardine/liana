use crate::app::view::global_home::{WalletKind, WalletTransferEntry};
use std::convert::TryInto;
use std::sync::Arc;
use std::time::{Duration, Instant};

use coincube_core::miniscript::bitcoin::{Amount, OutPoint};
use coincube_ui::component::tenshu::PageHead;
use coincube_ui::widget::Element;
use coincubed::commands::CoinStatus;
use iced::Task;

use crate::{
    app::cache::Cache,
    app::error::Error,
    app::menu::Menu,
    app::message::Message,
    app::state::coins_summary,
    app::state::State,
    app::view,
    app::wallet::{sync_status, SyncStatus, Wallet},
};

pub const HISTORY_EVENT_PAGE_SIZE: u64 = 20;
const HOME_RELOAD_MAX_TTL: Duration = Duration::from_secs(10);
const HOME_RELOAD_MIN_TTL: Duration = Duration::from_secs(3);

use crate::app::state::vault::label::LabelsEdited;
use crate::daemon::model::LabelsLoader;
use crate::daemon::{
    model::{Coin, HistoryTransaction, Payment},
    Daemon,
};
use crate::utils::now;

#[derive(Default)]
pub struct Payments {
    list: Vec<Payment>,
    is_last_page: bool,
    loaded_page_count: usize,
    history_loaded: bool,
    refresh_in_flight: bool,
    last_error: Option<String>,
}

pub struct VaultOverview {
    sync_status: SyncStatus,
    last_reload: Instant,

    wallet: Arc<Wallet>,
    balance: Amount,
    unconfirmed_balance: Amount,
    remaining_sequence: Option<u32>,
    expiring_coins: Vec<OutPoint>,
    /// PSBT proposals waiting for signatures, for the protection strip.
    psbts_waiting: usize,
    payments: Payments,
    processing: bool,
    selected_event: Option<(HistoryTransaction, usize)>,
    labels_edited: LabelsEdited,

    warning: Option<Error>,
    /// Set when the restore recorded a rescan it has no date for; the only
    /// case the user has to act on (see `App::needs_rescan_date`).
    show_rescan_prompt: bool,
    /// Whether that restore is still waiting for its history to be scanned. The
    /// balance header reads this, not the prompt: dismissing the prompt hides a
    /// reminder, it does not load any history, so the empty balance stays unknown.
    awaiting_rescan_date: bool,
    /// A rescan has been seen running since the panel was built. Once it is no
    /// longer running, the history it was owed has been scanned.
    rescan_seen_running: bool,
    show_received_celebration: bool,
    received_amount_display: String,
    received_quote: coincube_ui::component::quote_display::Quote,
    received_image_handle: iced::widget::image::Handle,
}

impl VaultOverview {
    pub fn new(
        wallet: Arc<Wallet>,
        coins: &[Coin],
        sync_status: SyncStatus,
        tip_height: i32,
        show_rescan_prompt: bool,
    ) -> Self {
        let (balance, unconfirmed_balance, expiring_coins, remaining_seq) =
            coins_summary(coins, tip_height as u32, wallet.recovery_timelock());

        Self {
            wallet,
            sync_status,
            balance,
            unconfirmed_balance,
            remaining_sequence: remaining_seq,
            expiring_coins,
            psbts_waiting: 0,
            selected_event: None,
            payments: Payments::default(),
            labels_edited: LabelsEdited::default(),
            warning: None,
            processing: false,
            show_rescan_prompt,
            awaiting_rescan_date: show_rescan_prompt,
            rescan_seen_running: false,
            last_reload: Instant::now(),
            show_received_celebration: false,
            received_amount_display: String::new(),
            received_quote: coincube_ui::component::quote_display::random_quote("bitcoin-receive"),
            received_image_handle: coincube_ui::component::quote_display::image_handle_for_context(
                "bitcoin-receive",
            ),
        }
    }
}

impl VaultOverview {
    /// The Vault page with its "Recent activity" and the selected row's
    /// detail pane, both read from Activity by App (`view::activity`).
    pub fn view_with_recent_activity<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
        transfer: WalletTransferEntry,
        recent_activity: impl Fn(Option<&'static str>) -> Element<'a, view::Message> + 'a,
        detail: impl Fn() -> Option<Element<'a, view::Message>> + 'a,
    ) -> Element<'a, view::Message> {
        if let Some(page) = self.full_page_view(menu, cache) {
            return page;
        }
        let unknown_history = self.unknown_history_note();
        view::dashboard_with_detail(
            menu,
            cache,
            move || Self::page_head(cache, Some(transfer)),
            move || self.page_body(cache, Some(recent_activity(unknown_history))),
            detail,
        )
    }

    /// What a "Recent activity" table that lists the Vault's rows says in
    /// place of "No transactions yet" while the Vault's history is not known:
    /// an empty answer from an unfinished scan says nothing about the
    /// payments. The Vault page and the Overview share it.
    pub fn unknown_history_note(&self) -> Option<&'static str> {
        self.history_unknown().then(|| {
            coincube_ui::locale::copy("Transactions appear once the Vault's history has loaded")
        })
    }

    /// The balance row's own test: the Vault's history is not known yet.
    fn history_unknown(&self) -> bool {
        matches!(
            view::vault::overview::balance_sync_state(
                &self.sync_status,
                self.balance + self.unconfirmed_balance,
                self.awaiting_rescan_date,
            ),
            view::wallet_header::SyncState::Unknown { .. }
        )
    }

    /// The received celebration or a payment's own screen replace the page.
    fn full_page_view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> Option<Element<'a, view::Message>> {
        if self.show_received_celebration {
            let celebration = view::vault::overview::received_celebration_page(
                "bitcoin-receive",
                passive_received_amount(&self.received_amount_display, cache),
                &self.received_quote,
                &self.received_image_handle,
            );
            return Some(view::dashboard(menu, cache, celebration));
        }
        self.selected_event.as_ref().map(|(tx, output_index)| {
            view::vault::overview::payment_view(
                menu,
                cache,
                tx,
                *output_index,
                self.labels_edited.cache(),
            )
        })
    }

    /// The head: Transfer (when App supplies its state, as the Overview
    /// offers it), then the page's own actions.
    fn page_head<'a>(
        cache: &Cache,
        transfer: Option<WalletTransferEntry>,
    ) -> PageHead<'a, view::Message> {
        let tools = view::nav::vault::tools(&view::nav::NavContext::from_cache(cache));
        let head = view::wallet_page_head(cache, "Vault", cache.wallet_names.vault.as_deref());
        let head = match transfer {
            Some(entry) => head.action(view::global_home::wallet_transfer_action(
                &cache.cube_id,
                WalletKind::Vault,
                entry,
            )),
            None => head,
        };
        head_actions(
            &cache.cube_id,
            tools.first().map(|(_, route)| route.clone()),
        )
        .into_iter()
        .fold(head, |head, (_, action)| head.action(action))
    }

    #[cfg(test)]
    pub fn page_head_for_tests<'a>(
        cache: &Cache,
        transfer: Option<WalletTransferEntry>,
    ) -> PageHead<'a, view::Message> {
        Self::page_head(cache, transfer)
    }

    fn page_body<'a>(
        &'a self,
        cache: &'a Cache,
        recent_activity: Option<Element<'a, view::Message>>,
    ) -> Element<'a, view::Message> {
        let converter: Option<view::FiatAmountConverter> =
            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
        view::vault::overview::vault_overview_view(
            &self.balance,
            &self.unconfirmed_balance,
            &self.remaining_sequence,
            converter,
            &self.expiring_coins,
            recent_activity,
            &cache.cube_id,
            cache.chain(),
            &self.sync_status,
            self.show_rescan_prompt,
            self.awaiting_rescan_date,
            cache.bitcoin_unit,
            cache.node_bitcoind_sync_progress,
            cache.node_bitcoind_ibd,
            cache.show_direction_badges,
            cache.display_mode,
            cache.balance_amounts_masked(),
            self.psbts_waiting,
            cache.vault_alert.as_ref(),
        )
    }
}

impl State for VaultOverview {
    /// App renders this page through [`Self::view_with_recent_activity`];
    /// on its own it has no Activity rows to show.
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        self.full_page_view(menu, cache).unwrap_or_else(|| {
            view::dashboard_headed(
                menu,
                cache,
                Self::page_head(cache, None),
                self.page_body(cache, None),
            )
        })
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let daemon = daemon.expect("Daemon required for vault overview panel");
        match message {
            Message::View(view::Message::DismissReceivedCelebration) => {
                self.show_received_celebration = false;
                return Task::none();
            }
            Message::Tick => {
                // we reload the page only when the user is not exploring the history
                // looking at a selected event
                // or when the last reload was less than the defined TTL.
                if (self.payments.loaded_page_count == 1 || self.payments.last_error.is_some())
                    && !self.payments.refresh_in_flight
                    && self.selected_event.is_none()
                    && Instant::now() > self.last_reload + HOME_RELOAD_MAX_TTL
                {
                    return self.reload(Some(daemon.clone()), Some(self.wallet.clone()));
                }
            }
            Message::Coins(res) => match res {
                Err(e) => {
                    let err_msg = crate::user_error::report(&e);
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
                Ok(coins) => {
                    self.warning = None;
                    (
                        self.balance,
                        self.unconfirmed_balance,
                        self.expiring_coins,
                        self.remaining_sequence,
                    ) = coins_summary(
                        &coins,
                        cache.blockheight() as u32,
                        self.wallet.recovery_timelock(),
                    );
                }
            },
            // The protection strip's PSBT count. A failed read keeps the
            // last count rather than raising an error on the Vault page;
            // PSBT proposals reports it.
            Message::SpendTxs(Ok(mut txs)) => {
                self.wallet.apply_spend_tx_overrides(&mut txs);
                self.psbts_waiting = view::vault::overview::waiting_for_signatures(&txs);
            }
            Message::SpendTxs(Err(_)) => {}
            Message::Payments(res) => match res {
                Err(e) => {
                    self.payments.refresh_in_flight = false;
                    let err_msg = crate::user_error::report(&e);
                    self.payments.last_error = Some(err_msg.clone());
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
                Ok(events) => {
                    self.payments.refresh_in_flight = false;
                    self.payments.history_loaded = true;
                    self.payments.last_error = None;
                    self.warning = None;
                    // Detect new incoming payment by comparing newest tx
                    if let Some(newest) = events.first() {
                        use crate::daemon::model::PaymentKind;
                        let is_incoming = matches!(newest.kind, PaymentKind::Incoming);
                        let is_new = self
                            .payments
                            .list
                            .first()
                            .is_none_or(|prev| prev.outpoint != newest.outpoint);
                        if is_incoming && is_new && !self.payments.list.is_empty() {
                            use coincube_ui::component::amount::DisplayAmount;
                            self.received_amount_display = format!(
                                "{} {}",
                                newest
                                    .amount
                                    .to_formatted_string_with_unit(cache.bitcoin_unit),
                                cache.bitcoin_unit.label()
                            );
                            self.received_quote =
                                coincube_ui::component::quote_display::random_quote(
                                    "bitcoin-receive",
                                );
                            self.received_image_handle =
                                coincube_ui::component::quote_display::image_handle_for_context(
                                    "bitcoin-receive",
                                );
                            self.show_received_celebration = true;
                        }
                    }
                    self.payments.list = events;
                    self.payments.loaded_page_count = 1;
                    self.payments.is_last_page =
                        (self.payments.list.len() as u64) < HISTORY_EVENT_PAGE_SIZE;
                }
            },
            Message::PaymentsExtension(res) => match res {
                Err(e) => {
                    self.processing = false;
                    let err_msg = crate::user_error::report(&e);
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
                Ok((events, actual_limit)) => {
                    self.processing = false;
                    self.warning = None;
                    self.payments.loaded_page_count += 1;
                    self.payments.is_last_page = (events.len() as u64) < actual_limit;
                    if let Some(event) = events.first() {
                        if let Some(position) = self
                            .payments
                            .list
                            .iter()
                            .position(|event2| event2.outpoint == event.outpoint)
                        {
                            let len = self.payments.list.len();
                            for event in events {
                                if !self.payments.list[position..len]
                                    .iter()
                                    .any(|event2| event2.outpoint == event.outpoint)
                                {
                                    self.payments.list.push(event);
                                }
                            }
                        } else {
                            self.payments.list.extend(events);
                        }
                    }
                }
            },
            Message::UpdatePanelCache(is_current) => {
                // `rescan_progress` is the daemon's definitive signal: it is
                // reported for exactly as long as the persisted rescan
                // timestamp is set, and only `complete_rescan` clears that,
                // after the post-rescan update succeeds. An interrupted rescan
                // keeps it and goes on reporting progress (0% if the backend
                // is unreachable); a failed refresh leaves this cache as it was.
                if cache.rescan_progress().is_some() {
                    self.rescan_seen_running = true;
                } else if self.rescan_seen_running {
                    // The rescan the restore was owed has run to completion.
                    self.rescan_seen_running = false;
                    self.awaiting_rescan_date = false;
                }
                let wallet_was_syncing = !self.sync_status.is_synced();
                self.sync_status = sync_status(
                    daemon.backend(),
                    cache.blockheight(),
                    cache.sync_progress(),
                    cache.last_poll_timestamp(),
                    cache.last_poll_at_startup,
                    cache.history_sync(),
                );
                // If this is the current panel, reload it if wallet is no longer syncing.
                if is_current && wallet_was_syncing && self.sync_status.is_synced() {
                    return self.reload(Some(daemon.clone()), Some(self.wallet.clone()));
                }
            }
            Message::Payment(res) => match res {
                Ok(event) => {
                    self.selected_event = Some(event);
                }
                Err(e) => {
                    let err_msg = crate::user_error::report(&e);
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },
            Message::View(view::Message::HideRescanPrompt) => {
                self.show_rescan_prompt = false;
            }
            Message::View(view::Message::SelectPayment(outpoint)) => {
                return Task::perform(
                    async move {
                        let txs = daemon.get_history_txs(&[outpoint.txid]).await?;
                        let tx = txs.into_iter().next().ok_or_else(|| {
                            Error::Unexpected(format!(
                                "Transaction {} not found in history",
                                outpoint.txid
                            ))
                        })?;
                        Ok((tx, outpoint.vout as usize))
                    },
                    Message::Payment,
                );
            }
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    self.payments
                        .list
                        .iter_mut()
                        .map(|tx| tx as &mut dyn LabelsLoader)
                        .chain(
                            self.selected_event
                                .iter_mut()
                                .map(|(tx, _)| tx as &mut dyn LabelsLoader),
                        ),
                ) {
                    Ok(cmd) => {
                        return cmd;
                    }
                    Err(e) => {
                        let err_msg = crate::user_error::report(&e);
                        self.warning = Some(e);
                        return Task::done(Message::View(view::Message::ShowError(err_msg)));
                    }
                };
            }
            Message::View(view::Message::Reload) => {
                return self.reload(Some(daemon), Some(self.wallet.clone()));
            }
            Message::View(view::Message::Close) => {
                self.selected_event = None;
            }
            Message::View(view::Message::Scroll(offset)) => {
                if offset == 0.0
                    && (self.payments.loaded_page_count > 1
                        || Instant::now() > self.last_reload + HOME_RELOAD_MIN_TTL)
                {
                    return self.reload(Some(daemon), Some(self.wallet.clone()));
                }
            }
            Message::View(view::Message::Next) => {
                if let Some(last) = self.payments.list.last() {
                    let daemon = daemon.clone();
                    let last_event_date = last.time.unwrap();
                    self.processing = true;
                    return Task::perform(
                        async move {
                            let last_event_date = last_event_date.timestamp() as u32;
                            let mut limit = HISTORY_EVENT_PAGE_SIZE;
                            let mut events = daemon
                                .list_confirmed_payments(0_u32, last_event_date, limit)
                                .await?;

                            // because gethistory cursor is inclusive and use blocktime
                            // multiple events can occur in the same block.
                            // If there is more event in the same block that the
                            // HISTORY_EVENT_PAGE_SIZE they can not be retrieved by changing
                            // the cursor value (blocktime) but by increasing the limit.
                            //
                            // 1. Check if the events retrieved have all the same blocktime
                            let blocktime = if let Some(event) = events.first() {
                                event.time
                            } else {
                                return Ok((events, limit));
                            };

                            // 2. Retrieve a larger batch of event with the same cursor but
                            //    a larger limit.
                            while !events.iter().any(|evt| evt.time != blocktime)
                                && events.len() as u64 == limit
                            {
                                // increments of the equivalent of one page more.
                                limit += HISTORY_EVENT_PAGE_SIZE;
                                events = daemon
                                    .list_confirmed_payments(0, last_event_date, limit)
                                    .await?;
                            }
                            events.sort_by(|a, b| a.compare(b));
                            Ok((events, limit))
                        },
                        Message::PaymentsExtension,
                    );
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        let daemon = daemon.expect("Vault panels require daemon");
        let wallet = wallet.expect("Vault panels require wallet");

        // If the wallet is syncing, we expect it to finish soon and so better to wait for
        // updated data before reloading. Besides, if the wallet is syncing, the DB may be
        // locked if the poller is running and we wouldn't be able to reload data until
        // syncing completes anyway.
        if self.sync_status.wallet_is_syncing() {
            return Task::none();
        }
        self.selected_event = None;
        self.wallet = wallet;
        self.payments.loaded_page_count = 0;
        self.payments.refresh_in_flight = true;
        let daemon2 = daemon.clone();
        let now: u32 = now().as_secs().try_into().unwrap();
        self.last_reload = Instant::now();
        // Apply the optimistic-broadcast override so the Vault balance
        // summary on this panel reduces immediately after the user
        // broadcasts, instead of waiting for the daemon's mempool
        // poller to flag the spend. Mirrors the override applied to
        // the central `Cache.coins()`.
        let wallet_for_coins = self.wallet.clone();
        let daemon3 = daemon.clone();
        Task::batch(vec![
            Task::perform(
                async move {
                    daemon3
                        .list_spend_transactions(None)
                        .await
                        .map_err(|e| e.into())
                },
                Message::SpendTxs,
            ),
            Task::perform(
                async move {
                    let mut payments = daemon
                        .list_confirmed_payments(0, now, HISTORY_EVENT_PAGE_SIZE)
                        .await?;
                    payments.sort_by(|a, b| a.compare(b));

                    let mut pending_payments = daemon.list_pending_payments().await?;
                    pending_payments.extend(payments);
                    Ok(pending_payments)
                },
                Message::Payments,
            ),
            Task::perform(
                async move {
                    daemon2
                        .list_coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
                        .await
                        .map(|res| {
                            let mut coins = res.coins;
                            wallet_for_coins.apply_coin_overrides(&mut coins);
                            coins
                        })
                        .map_err(|e| e.into())
                },
                Message::Coins,
            ),
        ])
    }
}

fn passive_received_amount<'a>(amount: &'a str, cache: &Cache) -> &'a str {
    if cache.balance_amounts_masked() {
        "••••••"
    } else {
        amount
    }
}

impl From<VaultOverview> for Box<dyn State> {
    fn from(s: VaultOverview) -> Box<dyn State> {
        Box::new(s)
    }
}

/// The Vault page's head actions, labelled: **Tools**, opening the first
/// tool the Vault offers (`tools`; none, none shown), and **Wallet
/// settings**, opening Settings › Vault › Wallet & signers.
fn head_actions<'a>(
    cube_id: &str,
    tools: Option<Menu>,
) -> Vec<(&'static str, Element<'a, view::Message>)> {
    use coincube_ui::component::tenshu::{ActionButton, ActionKind};
    let action = |label: &'static str, control: &str, route: Menu| {
        let button: Element<'a, view::Message> = ActionButton::new(
            view::shared::landing::landing_control_id(cube_id, "Vault", control),
            coincube_ui::locale::copy(label),
            ActionKind::Secondary,
            view::Message::FocusAction,
        )
        .on_press(view::Message::Menu(route))
        .compact()
        .into();
        (label, button)
    };
    let tools = tools.map(|route| action("Tools", "tools", route));
    let settings = action(
        "Wallet settings",
        "wallet-settings",
        Menu::Vault(crate::app::menu::VaultSubMenu::Settings(Some(
            crate::app::menu::SettingsOption::Wallet,
        ))),
    );
    tools.into_iter().chain(std::iter::once(settings)).collect()
}
#[cfg(test)]
mod tests {
    /// Tools opens the first tool and is left out without one; Wallet
    /// settings opens Settings › Vault › Wallet & signers.
    #[test]
    fn vault_head_actions_open_tools_and_wallet_settings() {
        use crate::app::menu::{SettingsOption, VaultSubMenu};
        use crate::app::view::shared::landing::{capture, landing_control_id};
        let labels = |tools| {
            head_actions("cube", tools)
                .into_iter()
                .map(|(label, _)| label)
                .collect::<Vec<_>>()
        };
        let coins = Menu::Vault(VaultSubMenu::Coins(None));
        assert_eq!(labels(Some(coins.clone())), ["Tools", "Wallet settings"]);
        assert_eq!(labels(None), ["Wallet settings"]);
        for (index, control, route) in [
            (0, "tools", coins.clone()),
            (
                1,
                "wallet-settings",
                Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Wallet))),
            ),
        ] {
            let (_, action) = head_actions("cube", Some(coins.clone())).remove(index);
            let emitted = capture::activate(
                action,
                landing_control_id("cube", "Vault", control),
                (1024, 768),
                false,
            );
            assert!(
                emitted
                    .iter()
                    .any(|message| matches!(message, view::Message::Menu(m) if *m == route)),
                "{}",
                control
            );
        }
    }

    use super::*;
    use crate::daemon::model::Coin;
    use coincube_core::miniscript::bitcoin;
    use coincubed::commands::LCSpendInfo;
    use std::str::FromStr;

    fn test_overview() -> (VaultOverview, Arc<dyn Daemon + Sync + Send>, Cache) {
        const DESC: &str = "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr";
        let root = std::env::temp_dir().join(format!("overview-test-{}", uuid::Uuid::new_v4()));
        let descriptor = coincube_core::descriptors::CoincubeDescriptor::from_str(DESC).unwrap();
        let cfg: coincubed::config::Config = toml::from_str(&format!(
            "main_descriptor = '{}'\ndata_directory = '{}'\n[bitcoin_config]\nnetwork = 'bitcoin'\n[bitcoind_config]\naddr = '127.0.0.1:8332'\ncookie_path = '{}'\n",
            descriptor,
            root.display(),
            root.join(".cookie").display()
        ))
        .unwrap();
        let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
            crate::daemon::embedded::EmbeddedDaemon::unstarted_for_test(cfg, None),
        );
        let wallet = Arc::new(Wallet::new(descriptor));
        (
            VaultOverview::new(wallet, &[], SyncStatus::Synced, 970_247, true),
            daemon,
            Cache::default(),
        )
    }

    /// A Vault restored without a date, on a local node, is empty until it is
    /// rescanned. Dismissing the prompt asking for that date must not turn its
    /// unknown balance into a confident zero; only a rescan that runs to the end does.
    #[test]
    fn dismissing_the_rescan_prompt_keeps_an_unscanned_balance_unknown() {
        use crate::app::{
            view::vault::overview::balance_sync_state, view::wallet_header::SyncState,
        };
        let (mut overview, daemon, mut cache) = test_overview();
        let shown = |overview: &VaultOverview| {
            balance_sync_state(
                &SyncStatus::Synced,
                Amount::ZERO,
                overview.awaiting_rescan_date,
            )
        };
        assert!(matches!(shown(&overview), SyncState::Unknown { .. }));

        let _ = overview.update(
            Some(daemon.clone()),
            &cache,
            Message::View(view::Message::HideRescanPrompt),
        );
        assert!(!overview.show_rescan_prompt, "the prompt is dismissed");
        assert!(
            matches!(shown(&overview), SyncState::Unknown { .. }),
            "but the history is still unscanned"
        );

        // The user picks a date elsewhere; the rescan runs, then finishes.
        cache.daemon_cache.rescan_progress = Some(0.5);
        let _ = overview.update(
            Some(daemon.clone()),
            &cache,
            Message::UpdatePanelCache(false),
        );
        assert!(matches!(shown(&overview), SyncState::Unknown { .. }));
        cache.daemon_cache.rescan_progress = None;
        let _ = overview.update(Some(daemon), &cache, Message::UpdatePanelCache(false));
        assert!(matches!(shown(&overview), SyncState::Synced));
    }

    #[test]
    fn refresh_failure_keeps_history_visible_and_tick_retries_once() {
        use crate::daemon::model::PaymentKind;

        let (mut overview, daemon, cache) = test_overview();
        let payment = Payment {
            label: None,
            address: None,
            address_label: None,
            amount: Amount::from_sat(1),
            outpoint: OutPoint::new(
                bitcoin::Txid::from_str(
                    "f7bd1b2a995b689d326e51eb742eb1088c4a8f110d9cb56128fd553acc9f88e5",
                )
                .unwrap(),
                0,
            ),
            time: None,
            kind: PaymentKind::Outgoing,
        };
        let _ = overview.update(
            Some(daemon.clone()),
            &cache,
            Message::Payments(Ok(vec![payment])),
        );
        assert!(overview.payments.history_loaded);
        assert_eq!(overview.payments.loaded_page_count, 1);

        let wallet = overview.wallet.clone();
        let _ = overview.reload(Some(daemon.clone()), Some(wallet));
        assert!(overview.payments.refresh_in_flight);
        assert!(overview.payments.history_loaded);
        assert_eq!(overview.payments.list.len(), 1);
        assert_eq!(overview.payments.loaded_page_count, 0);

        overview.last_reload = Instant::now() - HOME_RELOAD_MAX_TTL - Duration::from_secs(1);
        let old_reload = overview.last_reload;
        let _ = overview.update(Some(daemon.clone()), &cache, Message::Tick);
        assert_eq!(
            overview.last_reload, old_reload,
            "Tick must not duplicate an in-flight fetch"
        );

        let _ = overview.update(
            Some(daemon.clone()),
            &cache,
            Message::Payments(Err(Error::Unexpected("refresh failed".into()))),
        );
        assert!(!overview.payments.refresh_in_flight);
        assert!(overview.payments.history_loaded);
        assert!(overview.payments.last_error.is_some());
        assert_eq!(overview.payments.list.len(), 1);

        let _ = overview.update(Some(daemon.clone()), &cache, Message::Tick);
        assert!(overview.payments.refresh_in_flight);
        assert!(overview.last_reload > old_reload);
        overview.last_reload = Instant::now() - HOME_RELOAD_MAX_TTL - Duration::from_secs(1);
        let retry_reload = overview.last_reload;
        let _ = overview.update(Some(daemon), &cache, Message::Tick);
        assert_eq!(
            overview.last_reload, retry_reload,
            "in-flight retry must remain throttled"
        );
    }

    #[test]
    fn loaded_empty_history_stays_known_during_refresh_and_error() {
        let (mut overview, daemon, cache) = test_overview();
        let _ = overview.update(Some(daemon.clone()), &cache, Message::Payments(Ok(vec![])));
        assert!(overview.payments.history_loaded);
        assert!(overview.payments.list.is_empty());

        let wallet = overview.wallet.clone();
        let _ = overview.reload(Some(daemon.clone()), Some(wallet));
        assert!(overview.payments.history_loaded);
        assert!(overview.payments.refresh_in_flight);
        let _ = overview.update(
            Some(daemon),
            &cache,
            Message::Payments(Err(Error::Unexpected("refresh failed".into()))),
        );
        assert!(overview.payments.history_loaded);
        assert!(overview.payments.list.is_empty());
        assert!(overview.payments.last_error.is_some());
    }
    #[tokio::test]
    async fn test_coins_summary() {
        // Will use the same address for all coins.
        let dummy_address =
            bitcoin::Address::from_str("bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg")
                .unwrap()
                .assume_checked();
        // Will use the same txid for all outpoints and spend info.
        let dummy_txid = bitcoin::Txid::from_str(
            "f7bd1b2a995b689d326e51eb742eb1088c4a8f110d9cb56128fd553acc9f88e5",
        )
        .unwrap();

        let tip_height = 800_000;
        let timelock = 10_000;
        let mut coins = Vec::new();
        // Without coins, all values are 0 / empty / None:
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (Amount::from_sat(0), Amount::from_sat(0), Vec::new(), None)
        );
        // Add a spending coin.
        coins.push(Coin {
            outpoint: OutPoint::new(dummy_txid, 0),
            amount: Amount::from_sat(100),
            address: dummy_address.clone(),
            derivation_index: bitcoin::bip32::ChildNumber::Normal { index: 0 },
            block_height: Some(1),
            is_immature: false,
            is_change: false,
            is_from_self: false,
            spend_info: Some(LCSpendInfo {
                txid: dummy_txid,
                height: None,
            }),
        });
        // Spending coin is ignored.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (Amount::from_sat(0), Amount::from_sat(0), Vec::new(), None)
        );
        // Add unconfirmed change coin not from self.
        coins.push(Coin {
            outpoint: OutPoint::new(dummy_txid, 1),
            amount: Amount::from_sat(109),
            address: dummy_address.clone(),
            derivation_index: bitcoin::bip32::ChildNumber::Normal { index: 1 },
            block_height: None,
            is_immature: false,
            is_change: true,
            is_from_self: false,
            spend_info: None,
        });
        // Included in unconfirmed balance. Other values remain the same.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (Amount::from_sat(0), Amount::from_sat(109), Vec::new(), None)
        );
        // Add unconfirmed coin from self.
        coins.push(Coin {
            outpoint: OutPoint::new(dummy_txid, 2),
            amount: Amount::from_sat(111),
            address: dummy_address.clone(),
            derivation_index: bitcoin::bip32::ChildNumber::Normal { index: 2 },
            block_height: None,
            is_immature: false,
            is_change: false,
            is_from_self: true,
            spend_info: None,
        });
        // Included in confirmed balance. Other values remain the same.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (
                Amount::from_sat(111),
                Amount::from_sat(109),
                Vec::new(),
                None
            )
        );
        // Add a confirmed coin 1 more than 10% from expiry:
        coins.push(Coin {
            outpoint: OutPoint::new(dummy_txid, 3),
            amount: Amount::from_sat(101),
            address: dummy_address.clone(),
            derivation_index: bitcoin::bip32::ChildNumber::Normal { index: 3 },
            block_height: Some(791_001), // 791_001 + timelock - tip_height = 1_001 > 1_000 = (timelock / 10)
            is_immature: false,
            is_change: false,
            is_from_self: false,
            spend_info: None,
        });
        // Coin is added to confirmed balance. Not expiring, but remaining seq is set.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (
                Amount::from_sat(212),
                Amount::from_sat(109),
                Vec::new(),
                Some(1_001)
            )
        );
        // Now decrease the last coin's confirmation height by 1 so that
        // it is within 10% of expiry:
        coins.last_mut().unwrap().block_height = Some(791_000);
        // Its outpoint has been added to expiring coins and remaining seq is lower.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (
                Amount::from_sat(212),
                Amount::from_sat(109),
                vec![OutPoint::new(dummy_txid, 3)],
                Some(1_000)
            )
        );
        // Now add a confirmed coin that is not yet expiring.
        coins.push(Coin {
            outpoint: OutPoint::new(dummy_txid, 4),
            amount: Amount::from_sat(105),
            address: dummy_address.clone(),
            derivation_index: bitcoin::bip32::ChildNumber::Normal { index: 4 },
            block_height: Some(792_000),
            is_immature: false,
            is_change: false,
            is_from_self: false,
            spend_info: None,
        });
        // Only confirmed balance has changed.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (
                Amount::from_sat(317),
                Amount::from_sat(109),
                vec![OutPoint::new(dummy_txid, 3)],
                Some(1_000)
            )
        );
        // Now add another confirmed coin that is expiring.
        coins.push(Coin {
            outpoint: OutPoint::new(dummy_txid, 5),
            amount: Amount::from_sat(108),
            address: dummy_address.clone(),
            derivation_index: bitcoin::bip32::ChildNumber::Normal { index: 5 },
            block_height: Some(790_500),
            is_immature: false,
            is_change: false,
            is_from_self: false,
            spend_info: None,
        });
        // Confirmed balance updated, as well as expiring coins and the remaining seq.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (
                Amount::from_sat(425),
                Amount::from_sat(109),
                vec![OutPoint::new(dummy_txid, 3), OutPoint::new(dummy_txid, 5)],
                Some(500)
            )
        );
    }
}

#[cfg(test)]
mod received_privacy_tests {
    use super::*;
    use crate::app::state::preferences::{BalancePrivacy, BalancePrivacyChrome};
    use crate::app::view::shared::landing::capture;
    use coincube_ui::theme;

    #[test]
    fn mounted_celebration_follows_passive_privacy_authority() {
        let mut cache = Cache::default();
        let amount = "87,654,321 sats";
        let quote = coincube_ui::component::quote_display::random_quote("bitcoin-receive");
        let image =
            coincube_ui::component::quote_display::image_handle_for_context("bitcoin-receive");
        for privacy in [
            None,
            Some(BalancePrivacy::Unresolved),
            Some(BalancePrivacy::Hidden),
            Some(BalancePrivacy::Shown),
        ] {
            cache.balance_privacy = privacy.map(|privacy| BalancePrivacyChrome {
                privacy,
                busy: false,
                interactive: false,
            });
            let mounted = capture::inspect(
                "vault-received-celebration-privacy",
                view::vault::overview::received_celebration_page(
                    "bitcoin-receive",
                    passive_received_amount(amount, &cache),
                    &quote,
                    &image,
                ),
                (1440, 900),
                "dark",
                theme::Theme::dark(),
            );
            if privacy == Some(BalancePrivacy::Shown) {
                // The receipt sets the unit beside the figure as its own text.
                let (figure, unit) = amount.rsplit_once(' ').unwrap_or((amount, ""));
                assert!(mounted.contains(figure));
                assert!(mounted.contains(unit));
            } else {
                assert!(mounted.contains("••••••"));
                assert!(!mounted.contains(amount));
            }
        }
    }
}
