//! Spark overview panel.
//!
//! Mirrors [`crate::app::state::liquid::overview::LiquidOverview`] — the
//! layout is the same Liquid-style unified portfolio card + recent
//! transactions list, minus the USDt asset row (Spark doesn't hold
//! Liquid assets) and with BTC branding instead of L-BTC. A
//! [`SparkOverviewView::stable_balance_active`] flag drives a small
//! "Stable" badge on the balance header when the SDK reports an
//! active Stable Balance token (Phase 6).

use std::convert::TryInto;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::Amount;
use coincube_spark_protocol::PaymentSummary;
use coincube_ui::component::tenshu::PageHead;
use coincube_ui::widget::Element;
use iced::{widget::qr_code, Task};

use crate::app::cache::Cache;
use crate::app::menu::{Menu, SparkSubMenu};
use crate::app::message::Message;
use crate::app::state::{redirect, State};
use crate::app::view::global_home::{WalletKind, WalletTransferEntry};
use crate::app::view::spark::overview::{SparkLandingAvailability, SparkLightningAddress};
use crate::app::view::spark::{
    SparkOverviewView, SparkPaymentMethod, SparkRecentTransaction, SparkStatus,
};
use crate::app::view::{self, FiatAmountConverter};
use crate::app::wallets::{DomainPaymentStatus, SparkBackend};
use crate::daemon::Daemon;
use crate::utils::format_time_ago;

/// Loaded info snapshot. `None` while the first `reload()` is in
/// flight or if the bridge returned an error.
#[derive(Debug, Clone)]
pub struct SparkBalanceSnapshot {
    pub balance_sats: u64,
    /// The bridge's validated USDB holding (`usdb_holding`), `None` when
    /// there is none. The view shows it as the US Dollar row and adds it to
    /// the total at the BTC/USD quote.
    pub usdb_balance: Option<coincube_spark_protocol::UsdbBalanceSnapshot>,
}

pub struct SparkOverview {
    backend: Option<Arc<SparkBackend>>,
    snapshot: Option<SparkBalanceSnapshot>,
    recent_transactions: Vec<SparkRecentTransaction>,
    error: Option<String>,
    loading: bool,
    /// Phase 6: cached Stable Balance flag. `None` until the
    /// `get_user_settings` RPC returns; `Some(true)` renders a
    /// "Stable" badge next to the balance in the overview.
    stable_balance_active: Option<bool>,
    stable_balance_saving: bool,
    /// The open share panel: the address it shows and that address's QR.
    lightning_share: Option<LightningShare>,
}

/// The share panel's content, fixed when it opens.
struct LightningShare {
    address: String,
    qr: Option<qr_code::Data>,
}

impl SparkOverview {
    pub fn new(backend: Option<Arc<SparkBackend>>) -> Self {
        Self {
            backend,
            snapshot: None,
            recent_transactions: Vec::new(),
            error: None,
            loading: false,
            stable_balance_active: None,
            stable_balance_saving: false,
            lightning_share: None,
        }
    }

    /// The backend handle can outlive a failed bridge refresh. Match the
    /// Settings toggle's reachability gate while keeping the last successful
    /// snapshot visible as stale portfolio context.
    fn bridge_reachable(&self) -> bool {
        self.landing_availability() == SparkLandingAvailability::Ready
    }

    fn landing_availability(&self) -> SparkLandingAvailability {
        landing_availability(
            self.backend.is_some(),
            self.loading,
            self.snapshot.is_some(),
            self.error.is_some(),
        )
    }

    /// The page without App's mounts (Activity, the Cube's address).
    fn view_alone<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard_headed(
            menu,
            cache,
            Self::page_head(cache, None),
            self.page_body(cache, SparkLightningAddress::Unknown, None),
        )
    }

    #[cfg(test)]
    pub fn page_head_for_tests<'a>(
        cache: &Cache,
        transfer: Option<WalletTransferEntry>,
    ) -> PageHead<'a, view::Message> {
        Self::page_head(cache, transfer)
    }

    /// The head: Transfer (when App supplies its state, as the Overview
    /// offers it), then the page's own actions.
    fn page_head<'a>(
        cache: &Cache,
        transfer: Option<WalletTransferEntry>,
    ) -> PageHead<'a, view::Message> {
        let head = view::wallet_page_head(cache, "Spark", cache.wallet_names.spark.as_deref());
        let head = match transfer {
            Some(entry) => head.action(view::global_home::wallet_transfer_action(
                &cache.cube_id,
                WalletKind::Spark,
                entry,
            )),
            None => head,
        };
        head_actions(&cache.cube_id, super::swap::swap_available(cache.network))
            .into_iter()
            .fold(head, |head, (_, action)| head.action(action))
    }

    /// The balance with the Lightning address beside it, then "Recent
    /// activity" (from Activity, mounted by App).
    pub fn page_body<'a>(
        &'a self,
        cache: &'a Cache,
        lightning_address: SparkLightningAddress,
        recent_activity: Option<Element<'a, view::Message>>,
    ) -> Element<'a, view::Message> {
        let fiat_converter: Option<FiatAmountConverter> =
            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

        let status = if self.backend.is_none() {
            SparkStatus::Unavailable
        } else if self.loading && self.snapshot.is_none() {
            SparkStatus::Loading
        } else if let Some(snapshot) = &self.snapshot {
            SparkStatus::Connected(snapshot.clone())
        } else if let Some(err) = &self.error {
            SparkStatus::Error(err.clone())
        } else {
            SparkStatus::Loading
        };

        let status_connected = matches!(status, SparkStatus::Connected(_));
        // The share popover shows only while the address it opened with is
        // still the Cube's.
        let lightning_share = self
            .lightning_share
            .as_ref()
            .filter(|share| {
                self.backend.is_some()
                    && matches!(&lightning_address,
                        SparkLightningAddress::Ready(address) if *address == share.address)
            })
            .map(|share| (share.address.as_str(), share.qr.as_ref()));
        let overview = SparkOverviewView {
            status,
            recent_transactions: &self.recent_transactions,
            cube_id: &cache.cube_id,
            last_error: self.error.as_deref(),
            fiat_converter,
            bitcoin_unit: cache.bitcoin_unit,
            show_direction_badges: cache.show_direction_badges,
            btc_usd_price: cache.btc_usd_price,
            display_mode: cache.display_mode,
            mask_amounts: cache.balance_amounts_masked(),
            lightning_address,
            lightning_share,
        }
        .render()
        .map(view::Message::SparkOverview);

        let mut content = iced::widget::Column::new().push(overview);
        // Only beside a balance: not under "Connecting" or a bridge error.
        // The shared page gutter aligns activity with the balance and head.
        if let Some(recent) = recent_activity.filter(|_| status_connected) {
            content = content.push(recent);
        }
        content.into()
    }

    /// The Spark page as App mounts it, with the share popover under the
    /// address button while it is open.
    pub fn view_with_lightning_share<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
        lightning_address: SparkLightningAddress,
        transfer: WalletTransferEntry,
        recent_activity: impl Fn() -> Element<'a, view::Message> + 'a,
        detail: impl Fn() -> Option<Element<'a, view::Message>> + 'a,
    ) -> Element<'a, view::Message> {
        view::dashboard_with_detail(
            menu,
            cache,
            move || Self::page_head(cache, Some(transfer)),
            move || self.page_body(cache, lightning_address.clone(), Some(recent_activity())),
            detail,
        )
    }

    #[cfg(test)]
    pub fn lightning_share_for_tests(&self) -> Option<&str> {
        self.lightning_share
            .as_ref()
            .map(|share| share.address.as_str())
    }

    #[cfg(test)]
    pub fn has_snapshot_for_tests(&self) -> bool {
        self.snapshot.is_some()
    }

    /// A save result is routed through App even if the user left this panel.
    pub fn finish_stable_balance_save(&mut self) {
        self.stable_balance_saving = false;
    }

    /// One app-wide in-flight guard covers either Spark entry point. A fast
    /// Settings → Overview route change cannot queue a second SDK mutation.
    pub fn try_start_stable_balance_save(&mut self) -> bool {
        if self.stable_balance_saving {
            return false;
        }
        self.stable_balance_saving = true;
        true
    }
}

/// Resolve a preview action against the current snapshot, not its prior row
/// position. A missing, empty, or duplicate bridge ID cannot open a payment.
fn find_unique_recent<'a>(
    recent: &'a [SparkRecentTransaction],
    id: &str,
) -> Option<&'a SparkRecentTransaction> {
    if id.trim().is_empty() {
        return None;
    }
    let mut matches = recent.iter().filter(|payment| payment.id == id);
    let selected = matches.next()?;
    matches.next().is_none().then_some(selected)
}

impl State for SparkOverview {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> Element<'a, crate::app::view::Message> {
        self.view_alone(menu, cache)
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> Task<Message> {
        let Some(backend) = self.backend.clone() else {
            return Task::none();
        };
        self.loading = true;
        // Keep the last refresh error alongside a retained snapshot until a
        // new success supersedes it; retrying alone is not fresh data.
        let data_task = Task::perform(
            {
                let backend = backend.clone();
                async move {
                    let info = backend.get_info().await;
                    let payments = backend.list_payments(Some(20), None).await;
                    (info, payments)
                }
            },
            |(info, payments)| match (info, payments) {
                (Ok(info), Ok(list)) => Message::View(view::Message::SparkOverview(
                    view::SparkOverviewMessage::DataLoaded {
                        balance: Amount::from_sat(info.balance_sats),
                        usdb_balance: crate::app::breez_spark::assets::usdb_holding(&info),
                        recent_payments: list.payments,
                    },
                )),
                (Err(e), _) | (_, Err(e)) => Message::View(view::Message::SparkOverview(
                    view::SparkOverviewMessage::Error(crate::user_error::report_spark(&e)),
                )),
            },
        );
        let settings_task = Task::perform(
            async move { backend.get_user_settings().await },
            |result| match result {
                Ok(settings) => Message::View(view::Message::SparkOverview(
                    view::SparkOverviewMessage::StableBalanceLoaded(settings.stable_balance_active),
                )),
                Err(e) => {
                    tracing::warn!("spark overview get_user_settings failed: {}", e);
                    // Preserve previous stable_balance_active state on
                    // transient failures — do not emit StableBalanceLoaded.
                    Message::Tick
                }
            },
        );
        Task::batch(vec![data_task, settings_task])
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::SparkOverview(msg)) = message {
            match msg {
                view::SparkOverviewMessage::DataLoaded {
                    balance,
                    usdb_balance,
                    recent_payments,
                } => {
                    self.loading = false;
                    self.error = None;
                    self.snapshot = Some(SparkBalanceSnapshot {
                        balance_sats: balance.to_sat(),
                        usdb_balance,
                    });

                    let fiat_converter: Option<FiatAmountConverter> =
                        cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
                    self.recent_transactions = recent_payments
                        .iter()
                        .take(5)
                        .map(|p| payment_summary_to_recent_tx(p, fiat_converter.as_ref()))
                        .collect();
                }
                view::SparkOverviewMessage::Error(err) => {
                    self.loading = false;
                    self.error = Some(err.clone());
                    return Task::done(Message::View(view::Message::ShowError(err)));
                }
                view::SparkOverviewMessage::StableBalanceLoaded(active) => {
                    self.stable_balance_active = Some(active);
                }
                view::SparkOverviewMessage::ToggleStableBalance(enabled) => {
                    if self.bridge_reachable()
                        && !self.stable_balance_saving
                        && self.stable_balance_active == Some(!enabled)
                    {
                        return Task::done(Message::View(view::Message::SparkSettings(
                            view::SparkSettingsMessage::StableBalanceToggled(enabled),
                        )));
                    }
                }
                view::SparkOverviewMessage::SendBtc => {
                    return send_from(coincube_spark_protocol::WalletAsset::Bitcoin);
                }
                view::SparkOverviewMessage::SendUsdb => {
                    return send_from(coincube_spark_protocol::WalletAsset::Usdb);
                }
                view::SparkOverviewMessage::ReceiveBtc => {
                    return receive_into(coincube_spark_protocol::WalletAsset::Bitcoin);
                }
                view::SparkOverviewMessage::ReceiveUsdb => {
                    return receive_into(coincube_spark_protocol::WalletAsset::Usdb);
                }
                view::SparkOverviewMessage::ViewConversion => {
                    return Task::batch(vec![
                        redirect(Menu::Spark(SparkSubMenu::Receive)),
                        Task::done(Message::View(view::Message::SparkReceive(
                            view::SparkReceiveMessage::ShowConversion,
                        ))),
                    ]);
                }
                view::SparkOverviewMessage::History => {
                    return redirect(Menu::Spark(SparkSubMenu::Transactions(None)));
                }
                view::SparkOverviewMessage::SelectTransaction(id) => {
                    if let Some(payment) =
                        find_unique_recent(&self.recent_transactions, &id).cloned()
                    {
                        return Task::batch(vec![
                            redirect(Menu::Spark(SparkSubMenu::Transactions(None))),
                            Task::done(Message::View(view::Message::SparkTransactions(
                                view::SparkTransactionsMessage::Preselect(payment),
                            ))),
                        ]);
                    }
                }
                view::SparkOverviewMessage::FocusAction(id) => {
                    return iced::widget::operation::focus(id);
                }
                view::SparkOverviewMessage::FlipDisplayMode => {
                    return Task::done(Message::View(view::Message::FlipDisplayMode));
                }
                view::SparkOverviewMessage::ShareLightningAddress(address) => {
                    // Encoded as Spark › Receive encodes it: the bare address.
                    let qr = qr_code::Data::new(&address).ok();
                    self.lightning_share = Some(LightningShare { address, qr });
                }
                view::SparkOverviewMessage::CloseLightningShare => {
                    self.lightning_share = None;
                }
                view::SparkOverviewMessage::CopyLightningAddress(address) => {
                    return Task::done(Message::View(view::Message::Clipboard(address)));
                }
                view::SparkOverviewMessage::SaveLightningQr(address) => {
                    return save_lightning_qr(address);
                }
                view::SparkOverviewMessage::ChangeLightningAddress => {
                    // The change form opens on the address's own Settings
                    // tab, after the route has switched to it.
                    self.lightning_share = None;
                    return redirect(lightning_address_settings()).chain(Task::done(
                        Message::View(view::Message::ConnectCube(
                            view::ConnectCubeMessage::BeginEditLightningAddress,
                        )),
                    ));
                }
                view::SparkOverviewMessage::ClaimLightningAddress => {
                    self.lightning_share = None;
                    return redirect(lightning_address_settings());
                }
            }
        }
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        iced::Subscription::none()
    }

    /// Leaving the page closes the share panel; it never reopens on return.
    fn close(&mut self) -> Task<Message> {
        self.lightning_share = None;
        Task::none()
    }
}

/// Spark's Send, paying from `asset` — each Overview balance row's Send.
/// Spark's Receive, set for what the balance row holds (USDB → USDB).
fn receive_into(asset: coincube_spark_protocol::WalletAsset) -> Task<Message> {
    Task::batch(vec![
        redirect(Menu::Spark(SparkSubMenu::Receive)),
        Task::done(Message::View(view::Message::SparkReceive(
            view::SparkReceiveMessage::StartFrom(asset),
        ))),
    ])
}

fn send_from(asset: coincube_spark_protocol::WalletAsset) -> Task<Message> {
    Task::batch(vec![
        redirect(Menu::Spark(SparkSubMenu::Send)),
        Task::done(Message::View(view::Message::SparkSend(
            view::SparkSendMessage::StartFrom(asset),
        ))),
    ])
}

/// What the Spark page shows for the Cube's Lightning address. Signed out, it
/// offers the claim (Settings asks to sign in first). Signed in, an address
/// is unknown until the Cube's registration says whether it has one.
pub fn lightning_address_for(
    authenticated: bool,
    cube: &crate::app::state::connect::ConnectCubePanel,
) -> SparkLightningAddress {
    let address = cube
        .lightning_address
        .as_ref()
        .and_then(|value| value.lightning_address.as_deref())
        .filter(|address| !address.trim().is_empty());
    match address {
        _ if !authenticated => SparkLightningAddress::Unclaimed,
        Some(address) => {
            SparkLightningAddress::Ready(view::spark::overview::full_lightning_address(address))
        }
        None if cube.server_cube_id.is_some() => SparkLightningAddress::Unclaimed,
        None => SparkLightningAddress::Unknown,
    }
}

/// Settings › Spark › Lightning address: where the address is claimed and
/// changed.
fn lightning_address_settings() -> Menu {
    Menu::Spark(SparkSubMenu::Settings(Some(
        crate::app::menu::SparkSettingsOption::LightningAddress,
    )))
}

/// The Spark page's head actions, labelled: **Swap BTC ⇄ USDB** once
/// `swap_enabled` (S3), then **Wallet settings**, which opens Settings ›
/// Spark › Balances & conversion.
/// A Lightning address's QR code as PNG bytes: black modules on white, 16px
/// each, inside the four-module quiet zone scanners expect. Encoded as the
/// share popover encodes it (the bare address, medium error correction).
pub fn lightning_qr_png(address: &str) -> Result<Vec<u8>, String> {
    const MODULE_PX: u32 = 16;
    const QUIET_MODULES: u32 = 4;
    let code = qrcode::QrCode::with_error_correction_level(address, qrcode::EcLevel::M)
        .map_err(|e| e.to_string())?;
    let modules = code.width() as u32;
    let colors = code.to_colors();
    let side = (modules + 2 * QUIET_MODULES) * MODULE_PX;
    let image = image::GrayImage::from_fn(side, side, |x, y| {
        let (mx, my) = (x / MODULE_PX, y / MODULE_PX);
        let dark = (QUIET_MODULES..QUIET_MODULES + modules).contains(&mx)
            && (QUIET_MODULES..QUIET_MODULES + modules).contains(&my)
            && colors[((my - QUIET_MODULES) * modules + (mx - QUIET_MODULES)) as usize]
                == qrcode::Color::Dark;
        image::Luma([if dark { 0 } else { 255 }])
    });
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(png.into_inner())
}

/// Ask where to save the address's QR code, then write it there. Cancelling
/// the dialog does nothing; a failure says so in a toast.
fn save_lightning_qr(address: String) -> Task<Message> {
    let name = address.split('@').next().unwrap_or("lightning");
    let file_name = format!("{name}-lightning-address.png");
    // Dialog copy is read here, on the window's thread, so it follows the
    // window language; the file name stays English.
    let title = coincube_ui::locale::copy("Save QR image").to_string();
    let filter = coincube_ui::locale::copy("PNG Image").to_string();
    Task::perform(
        async move {
            let png = lightning_qr_png(&address)?;
            let Some(handle) = rfd::AsyncFileDialog::new()
                .set_title(title)
                .set_file_name(file_name)
                .add_filter(filter, &["png"])
                .save_file()
                .await
            else {
                return Ok(false);
            };
            let path = handle.path().to_path_buf();
            tokio::task::spawn_blocking(move || std::fs::write(path, png))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            Ok(true)
        },
        |result: Result<bool, String>| result,
    )
    .then(|result| match result {
        Ok(true) => Task::done(Message::View(view::Message::ShowToast(
            log::Level::Info,
            "QR image saved".to_owned(),
        ))),
        Ok(false) => Task::none(),
        Err(e) => {
            tracing::error!("Saving the Lightning address QR image failed: {e}");
            Task::done(Message::View(view::Message::ShowToast(
                log::Level::Error,
                "Couldn't save the QR image".to_owned(),
            )))
        }
    })
}

fn head_actions<'a>(
    cube_id: &str,
    swap_enabled: bool,
) -> Vec<(&'static str, Element<'a, view::Message>)> {
    use coincube_ui::component::tenshu::{ActionButton, ActionKind};
    let action = |label: &'static str, control: &str, on_press: Option<view::Message>| {
        let button: Element<'a, view::Message> = ActionButton::new(
            view::shared::landing::landing_control_id(cube_id, "Spark", control),
            coincube_ui::locale::copy(label),
            ActionKind::Secondary,
            view::Message::FocusAction,
        )
        .on_press_maybe(on_press)
        .compact()
        .into();
        (label, button)
    };
    let swap = swap_enabled.then(|| {
        action(
            "Swap BTC ⇄ USDB",
            "swap",
            Some(view::Message::Menu(Menu::Spark(SparkSubMenu::Swap))),
        )
    });
    let settings = action(
        "Wallet settings",
        "wallet-settings",
        Some(view::Message::Menu(Menu::Spark(SparkSubMenu::Settings(
            Some(crate::app::menu::SparkSettingsOption::General),
        )))),
    );
    swap.into_iter().chain(std::iter::once(settings)).collect()
}

fn landing_availability(
    backend_available: bool,
    loading: bool,
    snapshot_present: bool,
    refresh_failed: bool,
) -> SparkLandingAvailability {
    if !backend_available || refresh_failed {
        SparkLandingAvailability::Unavailable
    } else if loading || !snapshot_present {
        SparkLandingAvailability::Refreshing
    } else {
        SparkLandingAvailability::Ready
    }
}

/// Build the view-level `SparkRecentTransaction` row from a bridge
/// `PaymentSummary`. The bridge already strips SDK-specific types to
/// strings + scalars, so the mapping is mostly decoration:
/// direction/status normalization, method lookup for the row icon,
/// and fiat conversion for the secondary amount line.
///
/// Shared with [`crate::app::state::spark::transactions::SparkTransactions`]
/// so both the overview list preview and the full Transactions page
/// produce identical row shapes.
pub(crate) fn payment_summary_to_recent_tx(
    p: &PaymentSummary,
    fiat_converter: Option<&FiatAmountConverter>,
) -> SparkRecentTransaction {
    let is_incoming = p.direction.eq_ignore_ascii_case("Receive");
    let status = parse_status(&p.status);

    if let Some(token_amount) = p.token_amount {
        return token_payment_to_recent_tx(p, token_amount, is_incoming, status);
    }

    let method = parse_method(&p.method);
    // PaymentSummary carries the unsigned sat amount; the view displays
    // it and composes the direction from the `direction` field.
    let amount = Amount::from_sat(p.amount_sat.unsigned_abs());
    let fees_sat = Amount::from_sat(p.fees_sat);
    let fiat_amount = fiat_converter.map(|c| c.convert(amount));

    let description = p
        .description
        .clone()
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| match method {
            SparkPaymentMethod::Lightning => {
                coincube_ui::locale::copy("Lightning payment").to_string()
            }
            SparkPaymentMethod::OnChainBitcoin => {
                if is_incoming {
                    coincube_ui::locale::copy("On-chain deposit").to_string()
                } else {
                    coincube_ui::locale::copy("On-chain withdrawal").to_string()
                }
            }
            SparkPaymentMethod::Spark => coincube_ui::locale::copy("Spark transfer").to_string(),
            SparkPaymentMethod::StableBalance => {
                coincube_ui::locale::copy("Stable Balance").to_string()
            }
        });

    SparkRecentTransaction {
        id: p.id.clone(),
        description,
        time_ago: format_time_ago(p.timestamp as i64),
        timestamp: p.timestamp,
        amount,
        fees_sat,
        fiat_amount,
        is_incoming,
        status,
        method,
        token_display: None,
    }
}

/// Build a recent-tx row for a `method=token` payment. The bridge
/// strips the sat fields to zero in this case and ships the token
/// amount + decimals + ticker; the view renders that as the headline
/// figure ("1.58 USDB") instead of pretending it's sats. USDB pegs
/// 1:1 to USD, so the secondary fiat line is the token amount itself,
/// not a BTC-derived conversion.
fn token_payment_to_recent_tx(
    p: &PaymentSummary,
    token_amount: u64,
    is_incoming: bool,
    status: DomainPaymentStatus,
) -> SparkRecentTransaction {
    use crate::app::breez_spark::assets::{format_token_display, MAX_TOKEN_DECIMALS_U64};
    use crate::app::view::vault::fiat::FiatAmount;
    use crate::services::fiat::Currency;

    // Clamp once so the formatter and the fiat math see the same
    // decimals value. `format_token_display` clamps internally, but
    // the `10_f64.powi(decimals as i32)` below would wrap for
    // pathological inputs (`decimals > i32::MAX` casts to negative,
    // making `powi` near-zero and the dollar figure blow up).
    let decimals = p.token_decimals.unwrap_or(0).min(MAX_TOKEN_DECIMALS_U64);
    let ticker = p.token_ticker.clone().unwrap_or_else(|| "USDB".to_string());
    let token_str = format_token_display(token_amount, decimals);
    let token_display = format!("{} {}", token_str, ticker);

    // USDB ≈ $1, so the dollar value is `amount / 10^decimals`. We
    // surface that as the row's secondary text via `FiatAmount` so it
    // matches the existing fiat label style.
    let dollar_value = if decimals == 0 {
        token_amount as f64
    } else {
        token_amount as f64 / 10_f64.powi(decimals as i32)
    };
    let fiat_amount = FiatAmount::new(dollar_value, Currency::USD).ok();

    let description = p
        .description
        .clone()
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| coincube_ui::locale::copy("Stable Balance").to_string());

    SparkRecentTransaction {
        id: p.id.clone(),
        description,
        time_ago: format_time_ago(p.timestamp as i64),
        timestamp: p.timestamp,
        // Zero out the sat amount so the row's pending-sat sums and
        // any callers that read `amount` for BTC totals don't pick up
        // a token figure as if it were satoshis.
        amount: Amount::ZERO,
        fees_sat: Amount::ZERO,
        fiat_amount,
        is_incoming,
        status,
        method: SparkPaymentMethod::StableBalance,
        token_display: Some(token_display),
    }
}

fn parse_status(raw: &str) -> DomainPaymentStatus {
    // Spark SDK statuses: Completed, Pending, Failed. The bridge ships
    // them via `{:?}` so the casing matches the variant names exactly.
    if raw.eq_ignore_ascii_case("Completed") || raw.eq_ignore_ascii_case("Complete") {
        DomainPaymentStatus::Complete
    } else if raw.eq_ignore_ascii_case("Pending") {
        DomainPaymentStatus::Pending
    } else if raw.eq_ignore_ascii_case("Failed") {
        DomainPaymentStatus::Failed
    } else {
        DomainPaymentStatus::Pending
    }
}

fn parse_method(raw: &str) -> SparkPaymentMethod {
    match raw.to_lowercase().as_str() {
        "lightning" => SparkPaymentMethod::Lightning,
        "deposit" | "withdraw" => SparkPaymentMethod::OnChainBitcoin,
        "token" => SparkPaymentMethod::StableBalance,
        _ => SparkPaymentMethod::Spark,
    }
}

#[cfg(test)]
mod tests {
    /// The saved QR image is a square PNG: white quiet zone, dark finder
    /// pattern in the top-left module just inside it.
    #[test]
    fn lightning_qr_png_draws_the_address_inside_a_quiet_zone() {
        let png = super::lightning_qr_png("sats@coincube.io").expect("address encodes");
        let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("a PNG")
            .to_luma8();
        let modules =
            qrcode::QrCode::with_error_correction_level("sats@coincube.io", qrcode::EcLevel::M)
                .expect("address encodes")
                .width() as u32;
        assert_eq!(image.width(), image.height());
        assert_eq!(image.width(), (modules + 8) * 16);
        assert_eq!(image.get_pixel(0, 0).0, [255]);
        assert_eq!(image.get_pixel(4 * 16 - 1, 4 * 16 - 1).0, [255]);
        assert_eq!(image.get_pixel(4 * 16, 4 * 16).0, [0]);
    }

    /// Swap shows on mainnet only and opens the Swap screen; Wallet settings
    /// opens Settings › Spark › Balances & conversion.
    #[test]
    fn spark_head_actions_offer_swap_on_mainnet_and_open_wallet_settings() {
        use crate::app::view::shared::landing::{capture, landing_control_id};
        use coincube_core::miniscript::bitcoin::Network;
        let labels = |swap| {
            head_actions("cube", swap)
                .into_iter()
                .map(|(label, _)| label)
                .collect::<Vec<_>>()
        };
        assert_eq!(labels(false), ["Wallet settings"]);
        assert_eq!(labels(true), ["Swap BTC ⇄ USDB", "Wallet settings"]);
        // The page head asks for the Cube's network.
        for (network, swap) in [(Network::Bitcoin, true), (Network::Regtest, false)] {
            let cache = Cache {
                network,
                ..Cache::default()
            };
            let probe = capture::inspect(
                "spark-head-swap",
                SparkOverview::page_head_for_tests(&cache, None).view(view::page_gutter),
                (1440, 900),
                "dark",
                coincube_ui::theme::Theme::dark(),
            );
            assert_eq!(probe.contains("Swap BTC ⇄ USDB"), swap, "{:?}", network);
        }
        for pointer in [false, true] {
            let (_, swap) = head_actions("cube", true).remove(0);
            let emitted = capture::activate(
                swap,
                landing_control_id("cube", "Spark", "swap"),
                (1024, 768),
                pointer,
            );
            assert!(emitted.iter().any(|message| matches!(
                message,
                view::Message::Menu(Menu::Spark(SparkSubMenu::Swap))
            )));
        }

        for pointer in [false, true] {
            let (_, settings) = head_actions("cube", false).pop().expect("Wallet settings");
            let emitted = capture::activate(
                settings,
                landing_control_id("cube", "Spark", "wallet-settings"),
                (1024, 768),
                pointer,
            );
            assert!(emitted.iter().any(|message| matches!(
                message,
                view::Message::Menu(Menu::Spark(SparkSubMenu::Settings(Some(
                    crate::app::menu::SparkSettingsOption::General
                ))))
            )));
        }
    }

    use super::*;

    #[test]
    fn stable_balance_save_guard_survives_panel_navigation_until_settlement() {
        let mut overview = SparkOverview::new(None);
        assert!(overview.try_start_stable_balance_save());
        // A second action can arrive from the other Spark page before the
        // first bridge result. Only the first is admitted.
        assert!(!overview.try_start_stable_balance_save());
        overview.finish_stable_balance_save();
        assert!(overview.try_start_stable_balance_save());
    }

    fn payment(direction: &str, status: &str, method: &str, amount_sat: i64) -> PaymentSummary {
        PaymentSummary {
            id: "payment-1".to_string(),
            amount_sat,
            fees_sat: 12,
            token_amount: None,
            token_decimals: None,
            token_ticker: None,
            timestamp: 1_700_000_000,
            status: status.to_string(),
            direction: direction.to_string(),
            method: method.to_string(),
            description: None,
            details: Default::default(),
        }
    }

    #[test]
    fn recent_identity_survives_reorder_and_fails_closed_on_removal_or_duplicate() {
        let first =
            payment_summary_to_recent_tx(&payment("Receive", "Complete", "spark", 11_000), None);
        let mut second_summary = payment("Send", "Pending", "spark", 22_000);
        second_summary.id = "payment-2".to_string();
        let second = payment_summary_to_recent_tx(&second_summary, None);
        let mut recent = vec![first.clone(), second];
        assert_eq!(
            find_unique_recent(&recent, "payment-1").map(|p| p.amount.to_sat()),
            Some(11_000)
        );
        recent.swap(0, 1);
        assert_eq!(
            find_unique_recent(&recent, "payment-1").map(|p| p.amount.to_sat()),
            Some(11_000)
        );
        recent.remove(1);
        assert!(find_unique_recent(&recent, "payment-1").is_none());
        recent.extend([first.clone(), first]);
        assert!(find_unique_recent(&recent, "payment-1").is_none());
        assert!(find_unique_recent(&recent, " ").is_none());
    }

    #[test]
    fn lightning_receive_maps_direction_status_amount_and_default_description() {
        let row = payment_summary_to_recent_tx(
            &payment("receive", "completed", "lightning", 42_000),
            None,
        );

        assert_eq!(row.id, "payment-1");
        assert!(row.is_incoming);
        assert_eq!(row.status, DomainPaymentStatus::Complete);
        assert_eq!(row.method, SparkPaymentMethod::Lightning);
        assert_eq!(row.amount.to_sat(), 42_000);
        assert_eq!(row.fees_sat.to_sat(), 12);
        assert_eq!(row.description, "Lightning payment");
        assert_eq!(row.token_display, None);
    }

    #[test]
    fn onchain_withdraw_uses_outgoing_description_and_failed_status() {
        let row =
            payment_summary_to_recent_tx(&payment("Send", "Failed", "withdraw", -25_000), None);

        assert!(!row.is_incoming);
        assert_eq!(row.status, DomainPaymentStatus::Failed);
        assert_eq!(row.method, SparkPaymentMethod::OnChainBitcoin);
        assert_eq!(row.amount.to_sat(), 25_000);
        assert_eq!(row.description, "On-chain withdrawal");
    }

    #[test]
    fn unknown_wire_values_degrade_to_pending_spark_transfer() {
        let row =
            payment_summary_to_recent_tx(&payment("outbound", "future", "future", 1_000), None);

        assert!(!row.is_incoming);
        assert_eq!(row.status, DomainPaymentStatus::Pending);
        assert_eq!(row.method, SparkPaymentMethod::Spark);
        assert_eq!(row.description, "Spark transfer");
    }

    #[test]
    fn token_payment_uses_token_units_instead_of_satoshis() {
        let mut summary = payment("Receive", "Complete", "token", 999_999);
        summary.fees_sat = 500;
        summary.token_amount = Some(1_580_000);
        summary.token_decimals = Some(6);
        summary.token_ticker = Some("USDB".to_string());

        let row = payment_summary_to_recent_tx(&summary, None);

        assert!(row.is_incoming);
        assert_eq!(row.status, DomainPaymentStatus::Complete);
        assert_eq!(row.method, SparkPaymentMethod::StableBalance);
        assert_eq!(row.amount, Amount::ZERO);
        assert_eq!(row.fees_sat, Amount::ZERO);
        assert_eq!(row.description, "Stable Balance");
        assert_eq!(row.token_display.as_deref(), Some("1.58 USDB"));
        assert!(row.fiat_amount.is_some());
    }

    #[test]
    fn explicit_payment_description_is_preserved() {
        let mut summary = payment("Receive", "Completed", "deposit", 21_000);
        summary.description = Some("Savings refill".to_string());

        let row = payment_summary_to_recent_tx(&summary, None);

        assert_eq!(row.description, "Savings refill");
        assert_eq!(row.method, SparkPaymentMethod::OnChainBitcoin);
    }

    #[test]
    fn data_loaded_sets_snapshot_and_keeps_only_five_recent_rows() {
        let mut panel = SparkOverview::new(None);
        panel.loading = true;
        panel.error = Some("old error".to_string());

        let payments: Vec<_> = (0..7)
            .map(|i| {
                let mut p = payment("Receive", "Completed", "spark", 1_000 + i);
                p.id = format!("payment-{i}");
                p
            })
            .collect();

        let _ = panel.update(
            None,
            &Cache::default(),
            Message::View(view::Message::SparkOverview(
                view::SparkOverviewMessage::DataLoaded {
                    balance: Amount::from_sat(123_456),
                    usdb_balance: Some(coincube_spark_protocol::UsdbBalanceSnapshot {
                        token_identifier: "btkn1synthetic".into(),
                        balance: 2_500_000,
                        decimals: 6,
                        ticker: "USDB".to_string(),
                    }),
                    recent_payments: payments,
                },
            )),
        );

        assert!(!panel.loading);
        assert_eq!(panel.error, None);
        let snapshot = panel.snapshot.as_ref().expect("snapshot loaded");
        assert_eq!(snapshot.balance_sats, 123_456);
        assert_eq!(
            snapshot.usdb_balance.as_ref().map(|s| s.balance),
            Some(2_500_000)
        );
        assert_eq!(panel.recent_transactions.len(), 5);
        assert_eq!(panel.recent_transactions[0].id, "payment-0");
        assert_eq!(panel.recent_transactions[4].id, "payment-4");
    }

    #[test]
    fn error_and_stable_balance_messages_update_local_state() {
        let mut panel = SparkOverview::new(None);
        panel.loading = true;

        let _ = panel.update(
            None,
            &Cache::default(),
            Message::View(view::Message::SparkOverview(
                view::SparkOverviewMessage::Error("bridge offline".to_string()),
            )),
        );

        assert!(!panel.loading);
        assert_eq!(panel.error.as_deref(), Some("bridge offline"));

        let _ = panel.update(
            None,
            &Cache::default(),
            Message::View(view::Message::SparkOverview(
                view::SparkOverviewMessage::StableBalanceLoaded(true),
            )),
        );

        assert_eq!(panel.stable_balance_active, Some(true));
    }

    #[test]
    fn refresh_error_retains_spark_snapshot_and_recent_activity() {
        let mut panel = SparkOverview::new(None);
        let _ = panel.update(
            None,
            &Cache::default(),
            Message::View(view::Message::SparkOverview(
                view::SparkOverviewMessage::DataLoaded {
                    balance: Amount::from_sat(42_000),
                    usdb_balance: None,
                    recent_payments: vec![payment("Receive", "Complete", "spark", 1_000)],
                },
            )),
        );
        let _ = panel.update(
            None,
            &Cache::default(),
            Message::View(view::Message::SparkOverview(
                view::SparkOverviewMessage::Error("bridge offline".to_string()),
            )),
        );
        assert_eq!(panel.error.as_deref(), Some("bridge offline"));
        assert_eq!(
            panel.snapshot.as_ref().map(|s| s.balance_sats),
            Some(42_000)
        );
        assert_eq!(panel.recent_transactions.len(), 1);
        assert_eq!(panel.recent_transactions[0].id, "payment-1");
    }

    #[test]
    fn retained_snapshot_after_bridge_error_does_not_enable_stable_balance() {
        let mut panel = SparkOverview::new(None);
        let availability_with_backend = |panel: &SparkOverview| {
            landing_availability(
                true,
                panel.loading,
                panel.snapshot.is_some(),
                panel.error.is_some(),
            )
        };

        let _ = panel.update(
            None,
            &Cache::default(),
            Message::View(view::Message::SparkOverview(
                view::SparkOverviewMessage::DataLoaded {
                    balance: Amount::from_sat(42_000),
                    usdb_balance: None,
                    recent_payments: vec![],
                },
            )),
        );
        let _ = panel.update(
            None,
            &Cache::default(),
            Message::View(view::Message::SparkOverview(
                view::SparkOverviewMessage::StableBalanceLoaded(false),
            )),
        );
        assert_eq!(
            availability_with_backend(&panel),
            SparkLandingAvailability::Ready
        );

        panel.loading = true;
        let refreshing = availability_with_backend(&panel);
        assert_eq!(refreshing, SparkLandingAvailability::Refreshing);
        let _ = panel.update(
            None,
            &Cache::default(),
            Message::View(view::Message::SparkOverview(
                view::SparkOverviewMessage::Error("bridge offline".to_string()),
            )),
        );
        assert!(panel.snapshot.is_some());
        assert_eq!(panel.stable_balance_active, Some(false));
        let unavailable = availability_with_backend(&panel);
        assert_eq!(unavailable, SparkLandingAvailability::Unavailable);

        // A settings reply can arrive after the failed info refresh; it
        // cannot make the stale snapshot actionable again.
        let _ = panel.update(
            None,
            &Cache::default(),
            Message::View(view::Message::SparkOverview(
                view::SparkOverviewMessage::StableBalanceLoaded(true),
            )),
        );
        assert_eq!(
            availability_with_backend(&panel),
            SparkLandingAvailability::Unavailable
        );
    }

    /// Signed out, the page offers the claim; signed in, the Cube's address,
    /// the claim once registration shows there is none, and nothing before.
    #[test]
    fn spark_page_address_follows_the_connect_cube() {
        use crate::app::state::connect::ConnectCubePanel;
        use crate::services::coincube::LightningAddress;
        let mut cube = ConnectCubePanel::new(
            None,
            "synthetic-cube".into(),
            "Test Cube".into(),
            "bitcoin".into(),
            false,
        );
        assert_eq!(
            lightning_address_for(false, &cube),
            SparkLightningAddress::Unclaimed
        );
        assert_eq!(
            lightning_address_for(true, &cube),
            SparkLightningAddress::Unknown
        );
        cube.server_cube_id = Some(7);
        assert_eq!(
            lightning_address_for(true, &cube),
            SparkLightningAddress::Unclaimed
        );
        cube.lightning_address = Some(LightningAddress {
            lightning_address: Some("sats".into()),
        });
        assert_eq!(
            lightning_address_for(true, &cube),
            SparkLightningAddress::Ready("sats@coincube.io".into())
        );
        cube.lightning_address = Some(LightningAddress {
            lightning_address: Some("sats@coincube.io".into()),
        });
        assert_eq!(
            lightning_address_for(true, &cube),
            SparkLightningAddress::Ready("sats@coincube.io".into())
        );
        assert_eq!(
            lightning_address_for(false, &cube),
            SparkLightningAddress::Unclaimed
        );
    }
}
