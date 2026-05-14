use std::collections::{HashMap, HashSet};
use std::convert::{TryFrom, TryInto};
use std::sync::Arc;

use coincube_ui::{
    component::{button, card, form, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{column, combo_box, container, qr_code, row, slider, Space},
    Alignment, Length, Subscription, Task,
};

use crate::app::{
    cache::Cache,
    menu::P2PSubMenu,
    menu::{MarketplaceSubMenu, Menu},
    message::Message,
    view::{self, message::P2PMessage},
    wallet::Wallet,
    wallets::SparkBackend,
    State,
};
use coincube_spark_protocol::PrepareSendOk;

use super::components::order_card::TakeOrderState;
use super::components::trade_card::{TradeRole, TradeStatus};
use super::components::{
    order_card, order_detail, order_filter_sidebar, payment_methods_for, trade_card,
    trade_status_filter, BuySellFilter, OrderFilterState, OrderType, P2POrder, P2PTrade,
    PricingMode, TradeFilter, FIAT_CURRENCIES,
};
use super::config::{load_mostro_config, save_mostro_config, MostroConfig, MostroNode};

/// Per-field validation warnings for the order creation form.
/// Which chat view is currently active (mutually exclusive).
#[derive(Default, PartialEq)]
enum ActiveChat {
    #[default]
    None,
    Peer,
    Dispute,
}

#[derive(Default)]
struct FormValidation {
    amount: Option<&'static str>,
    max_amount: Option<&'static str>,
    sats: Option<&'static str>,
    /// Owned string for dynamic range messages (includes node limits).
    sats_range: Option<String>,
    premium: Option<&'static str>,
    payment: Option<&'static str>,
    lightning_address: Option<&'static str>,
    /// True when node limits haven't been fetched yet (market price only).
    node_limits_missing: bool,
}

impl FormValidation {
    fn has_errors(&self) -> bool {
        self.amount.is_some()
            || self.max_amount.is_some()
            || self.sats.is_some()
            || self.sats_range.is_some()
            || self.premium.is_some()
            || self.payment.is_some()
            || self.lightning_address.is_some()
    }
}

/// Returns true if this DM action starts a countdown timer.
fn is_countdown_action(action: &str) -> bool {
    matches!(
        action,
        "PayInvoice" | "WaitingSellerToPay" | "AddInvoice" | "WaitingBuyerInvoice"
    )
}

/// A label–value row used in order confirmation and trade detail views.
fn detail_row(label: impl ToString, value: impl ToString) -> Element<'static, view::Message> {
    row![
        p2_regular(label.to_string()).width(Length::FillPortion(2)),
        p1_bold(value.to_string()).width(Length::FillPortion(3)),
    ]
    .spacing(8)
    .into()
}

/// Chat bubble style for own messages (accent tint, rounded).
fn chat_bubble_own(_theme: &coincube_ui::theme::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(iced::color!(0x2A1A00))),
        border: iced::Border {
            radius: 14.0.into(),
            width: 1.0,
            color: iced::color!(0x5A3A10),
        },
        ..Default::default()
    }
}

/// Chat bubble style for counterparty messages (dark foreground, rounded).
fn chat_bubble_peer(theme: &coincube_ui::theme::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(theme.colors.general.foreground)),
        border: iced::Border {
            radius: 14.0.into(),
            width: 1.0,
            color: iced::color!(0x3F3F3F),
        },
        ..Default::default()
    }
}

/// Info row style (card-like background for the trade info section in chat).
fn chat_info_card(theme: &coincube_ui::theme::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(theme.colors.general.foreground)),
        border: iced::Border {
            radius: 8.0.into(),
            width: 1.0,
            color: iced::color!(0x2A2A2A),
        },
        ..Default::default()
    }
}

/// Extract the text content from a SendDm payload JSON.
fn extract_chat_text(payload_json: &str) -> String {
    // Payload is serialized as e.g. {"TextMessage":"hello"}
    if let Ok(Some(mostro_core::message::Payload::TextMessage(text))) =
        serde_json::from_str::<Option<mostro_core::message::Payload>>(payload_json)
    {
        return text;
    }
    // Fallback: try to parse as a simple JSON string value
    if let Ok(text) = serde_json::from_str::<String>(payload_json) {
        return text;
    }
    payload_json.to_string()
}

/// Which tab is active in the Chat list view.
#[derive(Default, PartialEq)]
enum ChatListTab {
    #[default]
    Messages,
    Disputes,
}

/// Cached state for a downloaded chat image.
enum ImageCacheEntry {
    Loading,
    Ready(iced::widget::image::Handle),
    Failed(String),
}

/// Data for a chat message whose send is in-flight.
struct PendingChatMessage {
    order_id: String,
    cube_name: String,
    payload: String,
    timestamp: u64,
    /// Original input text, restored on send failure.
    original_text: String,
}

/// Phase of the "Pay from Spark" flow inside the payment-required modal.
///
/// Mirrors `crate::app::state::spark::send::SparkSendPhase` but lives on
/// `P2PPanel` so the seller can pay the Mostro hold invoice directly
/// without leaving the modal.
#[derive(Debug, Clone, Default)]
enum SparkPayPhase {
    /// No Spark-pay action in flight (balance not yet known, or the
    /// user is reviewing the legacy QR fallback). The modal renders
    /// the "Pay from Spark" entry button when `spark_balance_sat`
    /// covers the hold amount.
    #[default]
    Idle,
    /// `prepare_send` is in flight.
    Preparing,
    /// `prepare_send` returned — preview the amount + fee before send.
    Prepared(PrepareSendOk),
    /// `send_payment` is in flight.
    Sending,
    /// A prepare/send step failed — error stays visible until the user
    /// retries or flips to the QR fallback.
    Error(String),
}

/// Returns the fee headroom (in sats) to add on top of a hold amount
/// when deciding whether the cube's Spark balance can cover the trade.
///
/// Lightning routing fees scale with the payment (typically 0.1–1%, up
/// to a couple percent on bad routes), so a flat 10-sat buffer is
/// useless for anything but micro-payments: a 100k-sat trade can
/// easily incur 1–2k sats of routing fees, and a flat buffer would
/// green-light "Pay from Spark" only for `prepare_send` to reject.
/// We pad by 2% of the amount with a 10-sat floor so tiny invoices
/// still gate sensibly. Erring high here only hides the button when
/// Spark *could* barely cover — the opposite mistake (showing it and
/// failing the user mid-flow) is worse.
fn spark_pay_fee_buffer_sats(amount_sat: u64) -> u64 {
    (amount_sat / 50).max(10)
}

/// Heuristic for "should we silently retry this Spark error?".
/// Targets the common LSP-side blips (TLS close_notify with no
/// graceful shutdown, transport EOFs, generic timeouts) that resolve
/// on a second try. Anything that looks like a logic error (insufficient
/// funds, route not found) we surface immediately.
fn is_transient_spark_error(raw: &str) -> bool {
    let lower = raw.to_lowercase();
    lower.contains("unexpectedeof")
        || lower.contains("close_notify")
        || lower.contains("connect error")
        || lower.contains("network error")
        || lower.contains("timeout")
        || lower.contains("timed out")
}

/// Map a raw `SparkClientError` string into a user-facing message
/// for the Spark-pay Error phase. The SDK errors are long, nested,
/// and intimidating ("SparkSdkError: Service error: service provider
/// error: network error: Connect error: IoError(Custom { … })") —
/// users can't act on that. We map known failure shapes to one-line
/// guidance and fall back to a truncated first line for everything
/// else.
fn friendly_spark_pay_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("insufficient") {
        return "Spark wallet has insufficient funds for this trade.".to_string();
    }
    if lower.contains("routenotfound") || lower.contains("no route") {
        return "Couldn't find a Lightning route. Try again or pay from another wallet."
            .to_string();
    }
    if is_transient_spark_error(raw) {
        return "Spark connection issue. Please try again.".to_string();
    }
    let trimmed = raw.lines().next().unwrap_or(raw).trim();
    if trimmed.chars().count() > 200 {
        let cut: String = trimmed.chars().take(200).collect();
        format!("{}…", cut)
    } else {
        trimmed.to_string()
    }
}

pub struct P2PPanel {
    wallet: Option<Arc<Wallet>>,
    /// Spark backend, when the cube has it. Used by the payment-required
    /// modal to pay Mostro hold invoices directly instead of forcing
    /// the user to scan/copy into an external wallet.
    spark_backend: Option<Arc<SparkBackend>>,
    /// Last-known Spark balance in sats, refreshed each time the
    /// payment-required modal appears. `None` until the lookup
    /// completes; treated as "insufficient" for the spark-pay gate.
    spark_balance_sat: Option<u64>,
    /// Current phase of the "Pay from Spark" sub-flow inside the modal.
    spark_pay_phase: SparkPayPhase,
    /// When `true`, the payment-required modal renders the legacy QR /
    /// Copy Invoice / Cancel UI even if Spark could cover the trade.
    /// Toggled by the "Pay from another wallet" link.
    show_qr_fallback: bool,
    /// Order id of the in-flight Spark-pay session. Set when we kick
    /// off any Spark-related async task (balance lookup, prepare_send,
    /// send_payment) and cleared when the session ends (terminal
    /// success, modal dismiss, trade detail close). Async result
    /// messages carry their originating `order_id` and are dropped
    /// when it doesn't match this field — stale responses from a
    /// previous session can't mutate state or pay the wrong invoice.
    spark_pay_session_id: Option<String>,
    /// Hold-invoice amount in sats, pre-parsed via
    /// `spark.parse_input` so the trade-detail Spark-pay summary can
    /// show "Lock amount: X sats" before the user commits. Mostro
    /// leaves `trade.sats_amount = None` for market-priced sell
    /// orders, so we can't rely on that alone. `None` until parse
    /// completes (or if the invoice carries no amount).
    spark_pay_amount_sat: Option<u64>,
    /// State of the current `prepare_send` attempt for auto-retry:
    /// `(session_id, invoice, retry_used)`. We retry once on
    /// transient errors (TLS close_notify, network EOF, timeout) —
    /// these are common LSP-side blips that resolve immediately on a
    /// second try. The user only sees the error if both attempts
    /// fail. Cleared on `SparkPaySent` and on session change.
    spark_pay_attempt: Option<(String, String, bool)>,
    /// Order ids whose hold invoice we paid via Spark. Used to keep
    /// the trade visible in My Trades even when Mostro's session
    /// timer expires and the order goes Canceled/Expired (which the
    /// default filter would otherwise hide). The seller's Spark
    /// HTLC stays in-flight until Mostro cancels the hold invoice
    /// and Lightning returns the sats — the trade needs to remain
    /// visible until then so the user can see what's happening.
    /// Populated on `SparkPaySent`; never cleared in-process (the
    /// memory cost is one UUID per Spark-paid trade, negligible).
    spark_funded_order_ids: HashSet<String>,
    mnemonic: String,
    // Node info (fetched from info event)
    node_currencies: Vec<String>,
    node_min_order_sats: Option<u64>,
    node_max_order_sats: Option<u64>,
    // Order book state
    orders: Vec<P2POrder>,
    /// Order IDs we created locally — used to ensure is_mine stays true even
    /// if the subscription delivers the event before the session is persisted.
    my_created_order_ids: HashSet<String>,
    buy_sell_filter: BuySellFilter,
    // Order book filters
    filter_currency: String,
    filter_currency_combo_state: combo_box::State<String>,
    filter_deselected_payment_methods: HashSet<String>,
    filter_min_rating: f32,
    filter_min_days_active: u32,
    /// Cached unique payment methods from orders matching buy/sell + currency.
    filter_available_payment_methods: Vec<String>,
    selected_order: Option<String>,
    // Trades state
    trades: Vec<P2PTrade>,
    trade_filters: Vec<TradeFilter>,
    // Create Order form state
    create_order_type: OrderType,
    create_pricing_mode: PricingMode,
    create_fiat_currency: String,
    currency_combo_state: combo_box::State<String>,
    create_sats_amount: form::Value<String>,
    create_premium: form::Value<String>,
    create_payment_methods: Vec<String>,
    create_custom_payment_method: form::Value<String>,
    payment_method_combo_state: combo_box::State<String>,
    create_min_amount: form::Value<String>,
    create_max_amount: form::Value<String>,
    // Explicit toggle for range vs single-amount orders. When false, only
    // create_min_amount is used (as the fiat amount).
    range_order_mode: bool,
    create_lightning_address: form::Value<String>,
    // Tracks whether the user has interacted with the lightning address field;
    // gates auto-prefill from the cube's registered address.
    lightning_address_user_edited: bool,
    // When true, show the editable lightning address input; otherwise show the
    // prefilled address as static text with an "Edit" affordance.
    editing_lightning_address: bool,
    // Set once the user has clicked Submit. Required-field errors are only
    // surfaced after this; before that, an untouched form looks clean.
    submit_attempted: bool,
    // Order submission state
    confirming_order: bool,
    order_submitting: bool,
    order_submit_error: Option<String>,
    // Take order state
    taking_order: bool,
    take_order_amount: form::Value<String>,
    take_order_invoice: form::Value<String>,
    take_order_submitting: bool,
    // Trade detail state
    selected_trade: Option<String>,
    trade_invoice_input: form::Value<String>,
    trade_action_loading: bool,
    trade_rating: u8, // 1-5 star rating for counterparty
    // Cached QR code for the hold invoice in the selected trade's detail view
    hold_invoice_qr: Option<qr_code::Data>,
    // Hold invoice to display (seller must pay after taking a buy order)
    pending_payment_invoice: Option<(String, String, Option<i64>, qr_code::Data)>, // (order_id, invoice, amount_sats, qr_data)
    invoice_copied: bool,
    // Chat
    /// Trade selected from the Chat tab's conversation list.
    chat_selected_trade: Option<String>,
    /// Active tab in the Chat list (Messages vs Disputes).
    chat_list_tab: ChatListTab,
    active_chat: ActiveChat,
    chat_input: form::Value<String>,
    /// Holds the data for a chat message that is currently being sent.
    /// On success the message is appended to the transcript; on error the
    /// input text is restored and no phantom entry is created.
    pending_chat_message: Option<PendingChatMessage>,
    chat_show_trade_info: bool,
    chat_show_user_info: bool,
    // Dispute chat
    dispute_chat_input: form::Value<String>,
    pending_dispute_chat_message: Option<PendingChatMessage>,
    // Mostro settings
    mostro_config: MostroConfig,
    new_relay_input: form::Value<String>,
    new_node_name_input: form::Value<String>,
    new_node_pubkey_input: form::Value<String>,
    mostro_config_error: Option<&'static str>,
    /// Error surfaced from the subscription stream (relay failures, restore errors, etc.)
    pub stream_error: Option<String>,
    /// Cached trade messages for the selected trade (avoids disk I/O per frame).
    cached_trade_messages: Vec<super::mostro::TradeMessage>,
    /// Cached chat identity info for the selected trade (avoids key-derivation per frame).
    cached_chat_identity: Option<super::mostro::ChatIdentityInfo>,
    /// Decrypted image cache, keyed by blossom URL.
    image_cache: HashMap<String, ImageCacheEntry>,
    /// Blossom URLs currently being downloaded (prevents duplicate fetches).
    image_downloads_in_flight: HashSet<String>,
    /// Whether an image attachment is currently being sent.
    attachment_sending: bool,
}

impl P2PPanel {
    pub fn new(
        wallet: Option<Arc<Wallet>>,
        spark_backend: Option<Arc<SparkBackend>>,
        mnemonic: String,
        default_currency: Option<String>,
    ) -> Self {
        let default_currency = default_currency.unwrap_or_else(|| "USD".to_string());
        Self {
            wallet,
            spark_backend,
            spark_balance_sat: None,
            spark_pay_phase: SparkPayPhase::Idle,
            show_qr_fallback: false,
            spark_pay_session_id: None,
            spark_pay_amount_sat: None,
            spark_pay_attempt: None,
            spark_funded_order_ids: HashSet::new(),
            mnemonic,
            node_currencies: Vec::new(),
            node_min_order_sats: None,
            node_max_order_sats: None,
            orders: Vec::new(),
            my_created_order_ids: HashSet::new(),
            buy_sell_filter: BuySellFilter::Sell,
            filter_currency: default_currency.clone(),
            filter_currency_combo_state: combo_box::State::new(
                std::iter::once("All".to_string())
                    .chain(FIAT_CURRENCIES.iter().map(|s| s.to_string()))
                    .collect(),
            ),
            filter_deselected_payment_methods: HashSet::new(),
            filter_min_rating: 0.0,
            filter_min_days_active: 0,
            filter_available_payment_methods: Vec::new(),
            selected_order: None,
            trades: Vec::new(),
            trade_filters: vec![TradeFilter::All],
            create_order_type: OrderType::Buy,
            create_pricing_mode: PricingMode::Market,
            create_fiat_currency: "USD".to_string(),
            currency_combo_state: combo_box::State::new(
                FIAT_CURRENCIES.iter().map(|s| s.to_string()).collect(),
            ),
            create_sats_amount: Default::default(),
            create_premium: Default::default(),
            create_payment_methods: Vec::new(),
            create_custom_payment_method: Default::default(),
            payment_method_combo_state: combo_box::State::new(
                payment_methods_for("USD")
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
            create_min_amount: Default::default(),
            create_max_amount: Default::default(),
            range_order_mode: false,
            create_lightning_address: Default::default(),
            lightning_address_user_edited: false,
            editing_lightning_address: false,
            submit_attempted: false,
            confirming_order: false,
            order_submitting: false,
            order_submit_error: None,
            taking_order: false,
            take_order_amount: Default::default(),
            take_order_invoice: Default::default(),
            take_order_submitting: false,
            selected_trade: None,
            trade_invoice_input: Default::default(),
            trade_action_loading: false,
            trade_rating: 0,
            hold_invoice_qr: None,
            pending_payment_invoice: None,
            invoice_copied: false,
            chat_selected_trade: None,
            chat_list_tab: ChatListTab::Messages,
            active_chat: ActiveChat::None,
            chat_input: Default::default(),
            pending_chat_message: None,
            chat_show_trade_info: false,
            chat_show_user_info: false,
            dispute_chat_input: Default::default(),
            pending_dispute_chat_message: None,
            mostro_config: load_mostro_config().unwrap_or_else(|e| {
                tracing::error!("Failed to load mostro config, using defaults: {e}");
                MostroConfig::default()
            }),
            new_relay_input: Default::default(),
            new_node_name_input: Default::default(),
            new_node_pubkey_input: Default::default(),
            mostro_config_error: None,
            stream_error: None,
            cached_trade_messages: Vec::new(),
            cached_chat_identity: None,
            image_cache: HashMap::new(),
            image_downloads_in_flight: HashSet::new(),
            attachment_sending: false,
        }
    }

    fn cube_name(&self) -> String {
        self.wallet
            .as_ref()
            .map(|w| w.name.clone())
            .unwrap_or_else(|| "default".to_string())
    }

    /// Refresh the in-memory trade message and identity caches for the
    /// currently selected trade.  Call this whenever the selected trade
    /// changes or new messages arrive so that view methods can read from
    /// `self.cached_trade_messages` / `self.cached_chat_identity` instead
    /// of hitting the disk on every frame.
    fn refresh_trade_cache(&mut self) {
        let order_id = self.active_order_id();
        if let Some(order_id) = order_id {
            let cube_name = self.cube_name();
            self.cached_trade_messages = super::mostro::get_trade_messages(&cube_name, &order_id);
            self.cached_chat_identity = Some(super::mostro::get_chat_identity_info(
                &cube_name,
                &self.mnemonic,
                &order_id,
            ));
        } else {
            self.cached_trade_messages = Vec::new();
            self.cached_chat_identity = None;
        }
    }

    /// Separate cache refresh for the Chat tab's selected trade.
    fn refresh_chat_trade_cache(&mut self) {
        if let Some(ref order_id) = self.chat_selected_trade.clone() {
            let cube_name = self.cube_name();
            self.cached_trade_messages = super::mostro::get_trade_messages(&cube_name, order_id);
            self.cached_chat_identity = Some(super::mostro::get_chat_identity_info(
                &cube_name,
                &self.mnemonic,
                order_id,
            ));
        } else {
            self.cached_trade_messages = Vec::new();
            self.cached_chat_identity = None;
        }
    }

    /// The currently active order ID — prefers `chat_selected_trade` if set,
    /// otherwise falls back to `selected_trade` (MyTrades).
    fn active_order_id(&self) -> Option<String> {
        self.chat_selected_trade
            .clone()
            .or_else(|| self.selected_trade.clone())
    }

    /// Scan cached messages for image attachments and trigger downloads for any
    /// that aren't already cached or in-flight.
    fn trigger_image_downloads(&mut self) -> Task<Message> {
        let Some(order_id) = self.active_order_id() else {
            return Task::none();
        };
        let mut tasks: Vec<Task<Message>> = Vec::new();
        let chat_msgs: Vec<_> = self
            .cached_trade_messages
            .iter()
            .filter(|m| m.action == "SendDm" || m.action == "AdminDm")
            .collect();

        for msg in chat_msgs {
            // Only auto-download images; files just show metadata
            if let Some(super::mostro::AttachmentMeta::Image(meta)) =
                super::mostro::parse_attachment_metadata(&msg.payload_json)
            {
                let url = meta.blossom_url.clone();
                if self.image_cache.contains_key(&url)
                    || self.image_downloads_in_flight.contains(&url)
                {
                    continue;
                }
                self.image_cache
                    .insert(url.clone(), ImageCacheEntry::Loading);
                self.image_downloads_in_flight.insert(url.clone());

                let oid = order_id.clone();
                let err_oid = order_id.clone();
                let err_url = url.clone();
                let cname = self.cube_name();
                let mnemonic = self.mnemonic.clone();
                tasks.push(Task::perform(
                    super::mostro::download_and_decrypt_image(url, oid, cname, mnemonic),
                    move |result| match result {
                        Ok((order_id, blossom_url, bytes)) => {
                            Message::View(view::Message::P2P(P2PMessage::AttachmentDownloaded {
                                order_id,
                                blossom_url,
                                data: Ok(bytes),
                            }))
                        }
                        Err(e) => {
                            Message::View(view::Message::P2P(P2PMessage::AttachmentDownloaded {
                                order_id: err_oid,
                                blossom_url: err_url,
                                data: Err(e),
                            }))
                        }
                    },
                ));
            }
        }
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    /// Orders matching the current buy/sell tab + all active filters.
    fn filtered_orders(&self) -> Vec<&P2POrder> {
        self.orders
            .iter()
            .filter(|order| {
                let type_match = match self.buy_sell_filter {
                    BuySellFilter::Buy => order.order_type == OrderType::Sell,
                    BuySellFilter::Sell => order.order_type == OrderType::Buy,
                };
                type_match && self.order_passes_filters(order)
            })
            .collect()
    }

    /// Whether an order passes the currency, payment method, and reputation filters
    /// (everything except the buy/sell tab).
    fn order_passes_filters(&self, order: &P2POrder) -> bool {
        // Currency
        if self.filter_currency != "All" && order.fiat_currency != self.filter_currency {
            return false;
        }
        // Payment methods
        if !order.payment_methods.is_empty()
            && !order
                .payment_methods
                .iter()
                .any(|m| !self.filter_deselected_payment_methods.contains(m))
        {
            return false;
        }
        // Min rating
        if self.filter_min_rating > 0.0 {
            match order.seller_rating {
                Some(rating) if rating >= self.filter_min_rating => {}
                _ => return false,
            }
        }
        // Min days active
        if self.filter_min_days_active > 0 {
            match order.seller_days_old {
                Some(days) if days >= self.filter_min_days_active => {}
                _ => return false,
            }
        }
        true
    }

    /// Recompute the cached payment methods from orders matching buy/sell + currency filters.
    fn recompute_available_payment_methods(&mut self) {
        let mut methods: Vec<String> = self
            .orders
            .iter()
            .filter(|order| {
                let type_match = match self.buy_sell_filter {
                    BuySellFilter::Buy => order.order_type == OrderType::Sell,
                    BuySellFilter::Sell => order.order_type == OrderType::Buy,
                };
                type_match
                    && (self.filter_currency == "All"
                        || order.fiat_currency == self.filter_currency)
            })
            .flat_map(|order| order.payment_methods.iter().cloned())
            .collect::<HashSet<String>>()
            .into_iter()
            .collect();
        methods.sort();
        self.filter_available_payment_methods = methods;
    }

    fn filtered_trades(&self) -> Vec<&P2PTrade> {
        self.trades
            .iter()
            .filter(|trade| {
                // Hide own orders that were canceled/expired before
                // anyone took them — EXCEPT trades we paid via Spark.
                // Mostro may cancel a hold-invoice trade after our
                // HTLC is in flight (timeout, dispute, etc.); hiding
                // it then leaves the user staring at a pending Spark
                // payment with no context.
                if trade.role == TradeRole::Creator
                    && matches!(trade.status, TradeStatus::Canceled | TradeStatus::Expired)
                    && !self.spark_funded_order_ids.contains(&trade.id)
                {
                    return false;
                }
                self.trade_filters.iter().any(|f| match f {
                    TradeFilter::All => true,
                    TradeFilter::Pending => trade.status == TradeStatus::Pending,
                    TradeFilter::Active => {
                        matches!(
                            trade.status,
                            TradeStatus::Active | TradeStatus::SettledHoldInvoice
                        )
                    }
                    TradeFilter::WaitingPayment => trade.status == TradeStatus::WaitingPayment,
                    TradeFilter::WaitingInvoice => trade.status == TradeStatus::WaitingBuyerInvoice,
                    TradeFilter::FiatSent => trade.status == TradeStatus::FiatSent,
                    TradeFilter::Success => {
                        matches!(trade.status, TradeStatus::Success | TradeStatus::Expired)
                    }
                    TradeFilter::Canceled => matches!(
                        trade.status,
                        TradeStatus::Canceled | TradeStatus::CooperativelyCanceled
                    ),
                    TradeFilter::PayingSats => {
                        trade.last_dm_action.as_deref() == Some("HoldInvoicePaymentSettled")
                    }
                    TradeFilter::Dispute => trade.status == TradeStatus::Dispute,
                })
            })
            .collect()
    }

    pub fn sync_lightning_address_from_cache(&mut self, cache: &Cache) {
        if self.lightning_address_user_edited || !self.create_lightning_address.value.is_empty() {
            return;
        }
        if let Some(addr) = cache.lightning_address.as_ref() {
            self.create_lightning_address.value = addr.clone();
        }
    }

    fn clear_create_form(&mut self) {
        self.create_order_type = OrderType::Buy;
        self.create_pricing_mode = PricingMode::Market;
        self.create_fiat_currency = "USD".to_string();
        self.create_sats_amount = Default::default();
        self.create_premium = Default::default();
        self.create_payment_methods = Vec::new();
        self.create_custom_payment_method = Default::default();
        self.rebuild_payment_method_combo();
        self.create_min_amount = Default::default();
        self.create_max_amount = Default::default();
        self.range_order_mode = false;
        self.create_lightning_address = Default::default();
        self.lightning_address_user_edited = false;
        self.editing_lightning_address = false;
        self.submit_attempted = false;
    }

    fn rebuild_currency_combo(&mut self) {
        let options: Vec<String> = if self.node_currencies.is_empty() {
            FIAT_CURRENCIES.iter().map(|s| s.to_string()).collect()
        } else {
            self.node_currencies.clone()
        };
        self.currency_combo_state = combo_box::State::new(options.clone());
        // Keep filter combo in sync (with "All" prepended)
        self.filter_currency_combo_state =
            combo_box::State::new(std::iter::once("All".to_string()).chain(options).collect());
    }

    fn rebuild_payment_method_combo(&mut self) {
        let options: Vec<String> = payment_methods_for(&self.create_fiat_currency)
            .iter()
            .filter(|&&m| !self.create_payment_methods.contains(&m.to_string()))
            .map(|s| s.to_string())
            .collect();
        self.payment_method_combo_state = combo_box::State::new(options);
    }

    fn build_order_form(&self) -> super::mostro::OrderFormData {
        let mut payment_methods = self.create_payment_methods.clone();
        let custom = self.create_custom_payment_method.value.trim().to_string();
        if !custom.is_empty() && !payment_methods.contains(&custom) {
            payment_methods.push(custom);
        }

        let is_range = self.is_range_order();

        super::mostro::OrderFormData {
            kind: match self.create_order_type {
                OrderType::Buy => mostro_core::order::Kind::Buy,
                OrderType::Sell => mostro_core::order::Kind::Sell,
            },
            fiat_code: self.create_fiat_currency.clone(),
            fiat_amount: if is_range {
                0
            } else {
                self.create_min_amount.value.parse().unwrap_or(0)
            },
            min_amount: if is_range {
                self.create_min_amount.value.parse().ok()
            } else {
                None
            },
            max_amount: if is_range {
                self.create_max_amount.value.parse().ok()
            } else {
                None
            },
            amount: if self.create_pricing_mode == PricingMode::Fixed {
                self.create_sats_amount.value.parse().unwrap_or(0)
            } else {
                0
            },
            premium: if self.create_pricing_mode == PricingMode::Market {
                self.create_premium.value.parse().unwrap_or(0)
            } else {
                0
            },
            payment_method: payment_methods.join(","),
            cube_name: self.cube_name(),
            mnemonic: self.mnemonic.clone(),
            buyer_invoice: if self.create_order_type == OrderType::Buy {
                let addr = self.create_lightning_address.value.trim().to_string();
                if addr.is_empty() {
                    None
                } else {
                    Some(addr)
                }
            } else {
                None
            },
            expiry_days: 1,
            mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
            relay_urls: self.mostro_config.relays.clone(),
        }
    }

    fn is_range_order(&self) -> bool {
        self.range_order_mode
    }

    /// Returns BTC price (units of `create_fiat_currency` per 1 BTC) when the
    /// cache has a usable rate for the currently selected currency.
    fn btc_price_for_selected(&self, cache: &Cache) -> Option<f64> {
        use crate::services::fiat::Currency;
        let target: Currency = self.create_fiat_currency.parse().ok()?;
        if let Some(fp) = cache.fiat_price.as_ref() {
            if fp.currency() == target {
                if let Ok(p) = fp.res.as_ref() {
                    if p.value > 0.0 {
                        return Some(p.value);
                    }
                }
            }
        }
        if target == Currency::USD {
            return cache.btc_usd_price.filter(|p| *p > 0.0);
        }
        None
    }

    fn fiat_to_sats_estimate(&self, fiat_amount: i64, cache: &Cache) -> Option<u64> {
        if fiat_amount <= 0 {
            return None;
        }
        let price = self.btc_price_for_selected(cache)?;
        let sats = (fiat_amount as f64 / price * 1e8).round();
        if sats.is_finite() && sats > 0.0 {
            Some(sats as u64)
        } else {
            None
        }
    }

    fn sats_preview_caption<'a>(&self, sats: u64) -> Element<'a, view::Message> {
        let formatted = super::components::format_with_separators(sats);
        let below_min = self.node_min_order_sats.is_some_and(|m| sats < m);
        let above_max = self.node_max_order_sats.is_some_and(|m| sats > m);
        if below_min {
            caption(format!("≈ {formatted} sats — below trade minimum"))
                .style(theme::text::warning)
                .into()
        } else if above_max {
            caption(format!("≈ {formatted} sats — exceeds trade maximum"))
                .style(theme::text::warning)
                .into()
        } else {
            caption(format!("≈ {formatted} sats"))
                .style(theme::text::secondary)
                .into()
        }
    }

    fn sats_to_fiat_estimate(&self, sats: u64, cache: &Cache) -> Option<f64> {
        if sats == 0 {
            return None;
        }
        let price = self.btc_price_for_selected(cache)?;
        let fiat = sats as f64 / 1e8 * price;
        if fiat.is_finite() && fiat > 0.0 {
            Some(fiat)
        } else {
            None
        }
    }

    /// Validate the create-order form and return per-field warnings.
    fn validate_order_form(&self) -> FormValidation {
        let is_range = self.is_range_order();
        let effective_pricing_fixed = !is_range && self.create_pricing_mode == PricingMode::Fixed;

        let mut v = FormValidation::default();

        // Maximum reasonable fiat amount (no real P2P trade exceeds this in any currency)
        const MAX_FIAT: i64 = 999_999_999;

        // --- Amount (fiat — must be a positive integer, matching Mostro's i64 field) ---
        let min_amt: Option<i64> = self.create_min_amount.value.parse::<i64>().ok();
        if self.create_min_amount.value.is_empty() {
            v.amount = Some("Amount is required");
        } else if min_amt.is_none() {
            v.amount = Some("Enter a whole number");
        } else if min_amt.unwrap_or(0) <= 0 {
            v.amount = Some("Amount must be greater than 0");
        } else if min_amt.unwrap_or(0) > MAX_FIAT {
            v.amount = Some("Amount is too large");
        }

        // --- Max amount (range — must be a positive integer greater than min) ---
        if is_range {
            let max_amt: Option<i64> = self.create_max_amount.value.parse::<i64>().ok();
            if max_amt.is_none() {
                v.max_amount = Some("Enter a whole number");
            } else if max_amt.unwrap_or(0) <= 0 {
                v.max_amount = Some("Max must be greater than 0");
            } else if max_amt.unwrap_or(0) > MAX_FIAT {
                v.max_amount = Some("Max amount is too large");
            } else if let (Some(min), Some(max)) = (min_amt, max_amt) {
                if max <= min {
                    v.max_amount = Some("Max must be greater than min");
                }
            }
        }

        // --- Sats amount (fixed price — validated against node limits) ---
        if effective_pricing_fixed {
            let node_min = self.node_min_order_sats;
            let node_max = self.node_max_order_sats;

            if self.create_sats_amount.value.is_empty() {
                v.sats = Some("Sats amount is required for fixed price");
            } else if let Ok(sats) = self.create_sats_amount.value.parse::<u64>() {
                if sats == 0 {
                    v.sats = Some("Sats must be greater than 0");
                } else if node_min.is_none() || node_max.is_none() {
                    // Node limits not loaded — block until we know the range
                    v.sats = Some("Waiting for node limits...");
                } else {
                    if let Some(min) = node_min {
                        if sats < min {
                            v.sats_range = Some(format!(
                                "Below minimum ({} sats)",
                                super::components::format_with_separators(min),
                            ));
                        }
                    }
                    if v.sats_range.is_none() {
                        if let Some(max) = node_max {
                            if sats > max {
                                v.sats_range = Some(format!(
                                    "Above maximum ({} sats)",
                                    super::components::format_with_separators(max),
                                ));
                            }
                        }
                    }
                }
            } else {
                v.sats = Some("Enter a valid number");
            }
        }

        // --- Market price: warn if node limits not loaded ---
        if !effective_pricing_fixed
            && (self.node_min_order_sats.is_none() || self.node_max_order_sats.is_none())
            && !self.create_min_amount.value.is_empty()
        {
            v.node_limits_missing = true;
        }

        // --- Premium ---
        if !effective_pricing_fixed && !self.create_premium.value.is_empty() {
            if let Ok(p) = self.create_premium.value.parse::<i64>() {
                if !(-100..=100).contains(&p) {
                    v.premium = Some("Premium must be between -100 and 100");
                }
            } else {
                v.premium = Some("Enter a valid number");
            }
        }

        // --- Payment methods ---
        let has_payment = !self.create_payment_methods.is_empty()
            || !self.create_custom_payment_method.value.trim().is_empty();
        if !has_payment {
            v.payment = Some("Select at least one payment method");
        }

        // --- Lightning address / invoice (Buy orders only) ---
        if self.create_order_type == OrderType::Buy
            && self.create_lightning_address.value.trim().is_empty()
        {
            v.lightning_address = Some("Lightning Address or invoice is required");
        }

        v
    }

    /// Build a `TradeActionData` for the selected trade, dispatch the given async
    /// action, and route the result to `TradeActionResult`.
    fn perform_trade_action<F, Fut>(&mut self, invoice: Option<String>, action: F) -> Task<Message>
    where
        F: FnOnce(super::mostro::TradeActionData) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<super::mostro::TradeActionResponse, String>>
            + Send
            + 'static,
    {
        self.trade_action_loading = true;
        if let Some(ref order_id) = self.selected_trade {
            let data = super::mostro::TradeActionData {
                order_id: order_id.clone(),
                cube_name: self.cube_name(),
                mnemonic: self.mnemonic.clone(),
                invoice,
                mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                relay_urls: self.mostro_config.relays.clone(),
            };
            return Task::perform(action(data), |result| {
                Message::View(view::Message::P2P(P2PMessage::TradeActionResult(result)))
            });
        } else {
            tracing::warn!("perform_trade_action called with no selected trade");
        }
        self.trade_action_loading = false;
        Task::none()
    }

    fn order_confirmation_view(&self) -> Element<'_, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);
        let is_range = self.is_range_order();

        let kind_label = match self.create_order_type {
            OrderType::Buy => "Buy",
            OrderType::Sell => "Sell",
        };

        let amount_label = if self.create_pricing_mode == PricingMode::Fixed
            && !self.create_sats_amount.value.is_empty()
        {
            format!("{} sats", self.create_sats_amount.value)
        } else {
            "Market price".to_string()
        };

        let fiat_label = if is_range {
            format!(
                "{}-{} {}",
                self.create_min_amount.value,
                self.create_max_amount.value,
                self.create_fiat_currency
            )
        } else {
            format!(
                "{} {}",
                self.create_min_amount.value, self.create_fiat_currency
            )
        };

        let mut payment_methods = self.create_payment_methods.clone();
        let custom = self.create_custom_payment_method.value.trim().to_string();
        if !custom.is_empty() && !payment_methods.contains(&custom) {
            payment_methods.push(custom);
        }
        let payment_label = payment_methods.join(", ");

        let premium_label = if self.create_pricing_mode == PricingMode::Market {
            format!("{}%", self.create_premium.value.parse::<i64>().unwrap_or(0))
        } else {
            "N/A".to_string()
        };

        let mut details = column![
            detail_row("Order Type", kind_label),
            detail_row("Currency", &self.create_fiat_currency),
            detail_row("Amount", &amount_label),
            detail_row("Fiat Amount", &fiat_label),
            detail_row("Payment", &payment_label),
            detail_row("Premium", &premium_label),
        ]
        .spacing(8);

        if self.create_order_type == OrderType::Buy
            && !self.create_lightning_address.value.trim().is_empty()
        {
            details = details.push(detail_row(
                "Invoice",
                self.create_lightning_address.value.trim(),
            ));
        }

        details = details.push(detail_row("Expiry", "24 hours"));

        card::simple(
            column![
                p1_bold("Please review your order:"),
                details,
                row![
                    button::secondary(None, "Cancel")
                        .on_press(p2p(P2PMessage::CancelConfirmation))
                        .width(Length::Fill),
                    button::primary(None, "Confirm")
                        .on_press(p2p(P2PMessage::ConfirmOrder))
                        .width(Length::Fill),
                ]
                .spacing(8),
            ]
            .spacing(16),
        )
        .width(Length::Fixed(500.0))
        .into()
    }

    fn create_order_view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);
        let is_range = self.is_range_order();
        // Fixed price not available for range orders
        let effective_pricing_mode = if is_range {
            &PricingMode::Market
        } else {
            &self.create_pricing_mode
        };

        // Form validation (computed each render — only show after user has typed something)
        let v = self.validate_order_form();

        // Order type toggle
        let buy_btn = if self.create_order_type == OrderType::Buy {
            button::primary(None, "Buy")
        } else {
            button::secondary(None, "Buy")
        }
        .on_press(p2p(P2PMessage::OrderTypeSelected(OrderType::Buy)))
        .width(Length::Fill);

        let sell_btn = if self.create_order_type == OrderType::Sell {
            button::primary(None, "Sell")
        } else {
            button::secondary(None, "Sell")
        }
        .on_press(p2p(P2PMessage::OrderTypeSelected(OrderType::Sell)))
        .width(Length::Fill);

        // Order type card
        let order_type_card = container(row![buy_btn, sell_btn].spacing(8).width(Length::Fill))
            .padding(4)
            .width(Length::Fill)
            .style(theme::container::foreground_rounded);

        // Currency card
        let currency_combo = combo_box(
            &self.currency_combo_state,
            "Search currency...",
            Some(&self.create_fiat_currency),
            |selected: String| view::Message::P2P(P2PMessage::FiatCurrencyEdited(selected)),
        )
        .padding(10)
        .width(Length::Fill);

        let currency_label = match self.create_order_type {
            OrderType::Buy => "Select the fiat currency you will pay with",
            OrderType::Sell => "Select the fiat currency you want to receive",
        };

        let currency_card = card::simple(
            column![
                p2_regular(currency_label).style(theme::text::secondary),
                row![
                    icon::dollar_icon().style(theme::text::warning),
                    currency_combo,
                ]
                .spacing(12)
                .align_y(iced::alignment::Vertical::Center),
            ]
            .spacing(12),
        )
        .width(Length::Fill);

        // Amount card with Single / Range toggle
        let amount_label = match (&self.create_order_type, is_range) {
            (OrderType::Buy, false) => "Enter amount you want to send",
            (OrderType::Buy, true) => "Enter amount range you want to send",
            (OrderType::Sell, false) => "Enter amount you want to receive",
            (OrderType::Sell, true) => "Enter amount range you want to receive",
        };

        let single_btn = if !is_range {
            button::primary(None, "Single")
        } else {
            button::secondary(None, "Single")
        }
        .on_press(p2p(P2PMessage::RangeOrderToggled(false)))
        .width(Length::Fill);

        let range_btn = if is_range {
            button::primary(None, "Range")
        } else {
            button::secondary(None, "Range")
        }
        .on_press(p2p(P2PMessage::RangeOrderToggled(true)))
        .width(Length::Fill);

        // Inline per-field validation. Only surface errors after the user has
        // typed in the field, so an empty form doesn't render warnings.
        let min_field_col = {
            let placeholder = if is_range { "Min" } else { "Amount" };
            let mut col =
                column![
                    form::Form::new_amount_sats(placeholder, &self.create_min_amount, |v| {
                        view::Message::P2P(P2PMessage::MinAmountEdited(v))
                    })
                    .padding(10),
                ]
                .spacing(4)
                .width(Length::Fill);
            if !self.create_min_amount.value.is_empty() || self.submit_attempted {
                if let Some(warn) = v.amount {
                    col = col.push(caption(warn).style(theme::text::warning));
                }
            }
            if let Ok(amt) = self.create_min_amount.value.parse::<i64>() {
                if let Some(sats) = self.fiat_to_sats_estimate(amt, cache) {
                    col = col.push(self.sats_preview_caption(sats));
                }
            }
            col
        };

        let amount_inputs: Element<'a, view::Message> = if is_range {
            let max_field_col = {
                let mut col =
                    column![
                        form::Form::new_amount_sats("Max", &self.create_max_amount, |v| {
                            view::Message::P2P(P2PMessage::MaxAmountEdited(v))
                        })
                        .padding(10),
                    ]
                    .spacing(4)
                    .width(Length::Fill);
                if !self.create_max_amount.value.is_empty() || self.submit_attempted {
                    if let Some(warn) = v.max_amount {
                        col = col.push(caption(warn).style(theme::text::warning));
                    }
                }
                if let Ok(amt) = self.create_max_amount.value.parse::<i64>() {
                    if let Some(sats) = self.fiat_to_sats_estimate(amt, cache) {
                        col = col.push(self.sats_preview_caption(sats));
                    }
                }
                col
            };
            row![
                icon::coins_icon().style(theme::text::warning),
                min_field_col,
                max_field_col,
            ]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Top)
            .into()
        } else {
            row![
                icon::coins_icon().style(theme::text::warning),
                min_field_col,
            ]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Top)
            .into()
        };

        let mut amount_col = column![
            p2_regular(amount_label).style(theme::text::secondary),
            row![single_btn, range_btn].spacing(8).width(Length::Fill),
            amount_inputs,
        ]
        .spacing(12);
        // Show node order limits as a hint, or a warning if not loaded
        if let (Some(min), Some(max)) = (self.node_min_order_sats, self.node_max_order_sats) {
            amount_col = amount_col.push(
                caption(format!(
                    "Trade size must be between {} and {} sats",
                    super::components::format_with_separators(min),
                    super::components::format_with_separators(max),
                ))
                .style(theme::text::secondary),
            );
        } else if v.node_limits_missing {
            amount_col = amount_col.push(
                caption("Node limits not loaded — order may be rejected")
                    .style(theme::text::warning),
            );
        }
        let amount_card = card::simple(amount_col).width(Length::Fill);

        // Payment method card
        let pm_combo = combo_box(
            &self.payment_method_combo_state,
            "Select payment methods",
            None::<&String>,
            |selected: String| view::Message::P2P(P2PMessage::PaymentMethodSelected(selected)),
        )
        .padding(10)
        .width(Length::Fill);

        let pm_tags: Element<'a, view::Message> = if self.create_payment_methods.is_empty() {
            column![].into()
        } else {
            row(self.create_payment_methods.iter().map(|method| {
                let method_clone = method.clone();
                container(
                    row![
                        p2_regular(method.as_str()),
                        button::secondary_compact(None, "x").on_press(view::Message::P2P(
                            P2PMessage::PaymentMethodRemoved(method_clone,)
                        )),
                    ]
                    .spacing(4)
                    .align_y(iced::alignment::Vertical::Center),
                )
                .padding([4, 8])
                .style(theme::pill::simple)
                .into()
            }))
            .spacing(6)
            .wrap()
            .into()
        };

        let mut payment_col = column![
            p2_regular("Payment methods for").style(theme::text::secondary),
            row![icon::card_icon().style(theme::text::warning), pm_combo,]
                .spacing(12)
                .align_y(iced::alignment::Vertical::Center),
            pm_tags,
            form::Form::new_trimmed(
                "Enter custom payment method",
                &self.create_custom_payment_method,
                |v| { view::Message::P2P(P2PMessage::CustomPaymentMethodEdited(v)) }
            )
            .padding(10),
            if self.create_custom_payment_method.value.is_empty() {
                button::secondary(None, "Add Custom")
            } else {
                button::secondary(None, "Add Custom")
                    .on_press(view::Message::P2P(P2PMessage::AddCustomPaymentMethod))
            },
        ]
        .spacing(12);
        if self.submit_attempted {
            if let Some(warn) = v.payment {
                payment_col = payment_col.push(caption(warn).style(theme::text::warning));
            }
        }
        let payment_card = card::simple(payment_col).width(Length::Fill);

        // Combined price type + pricing input card
        let market_btn = if *effective_pricing_mode == PricingMode::Market {
            button::primary(None, "Market Rate")
        } else {
            button::secondary(None, "Market Rate")
        }
        .on_press(p2p(P2PMessage::PricingModeSelected(PricingMode::Market)))
        .width(Length::Fill);

        let fixed_btn = if *effective_pricing_mode == PricingMode::Fixed {
            button::primary(None, "Fixed Price")
        } else {
            button::secondary(None, "Fixed Price")
        }
        .width(Length::Fill);
        let fixed_btn = if is_range {
            fixed_btn
        } else {
            fixed_btn.on_press(p2p(P2PMessage::PricingModeSelected(PricingMode::Fixed)))
        };

        let pricing_input: Element<'a, view::Message> = if *effective_pricing_mode
            == PricingMode::Fixed
        {
            let mut sats_col = column![
                p2_regular("Sats Amount").style(theme::text::secondary),
                row![
                    icon::bitcoin_icon().style(theme::text::warning),
                    form::Form::new_amount_sats(
                        "Enter sats amount",
                        &self.create_sats_amount,
                        |v| { view::Message::P2P(P2PMessage::SatsAmountEdited(v)) }
                    )
                    .padding(10),
                ]
                .spacing(12)
                .align_y(iced::alignment::Vertical::Center),
            ]
            .spacing(8);
            if let Some(warn) = v.sats {
                if !self.create_sats_amount.value.is_empty() || self.submit_attempted {
                    sats_col = sats_col.push(caption(warn).style(theme::text::warning));
                }
            } else if let Some(warn) = v.sats_range.clone() {
                sats_col = sats_col.push(caption(warn).style(theme::text::warning));
            }
            if let Ok(sats) = self.create_sats_amount.value.parse::<u64>() {
                if let Some(fiat) = self.sats_to_fiat_estimate(sats, cache) {
                    sats_col = sats_col.push(
                        caption(format!("≈ {:.2} {}", fiat, self.create_fiat_currency))
                            .style(theme::text::secondary),
                    );
                }
            }
            if let (Some(min), Some(max)) = (self.node_min_order_sats, self.node_max_order_sats) {
                sats_col = sats_col.push(
                    caption(format!(
                        "Allowed: {} - {} sats",
                        super::components::format_with_separators(min),
                        super::components::format_with_separators(max),
                    ))
                    .style(theme::text::secondary),
                );
            }
            sats_col.into()
        } else {
            let premium_val: f32 = self.create_premium.value.parse::<f32>().unwrap_or(0.0);
            let slider_min: f32 = (-10.0f32).min(premium_val).max(-100.0);
            let slider_max: f32 = (10.0f32).max(premium_val).min(100.0);
            let premium_int = premium_val as i64;
            let premium_label = if premium_int > 0 {
                format!("+{}%", premium_int)
            } else {
                format!("{}%", premium_int)
            };

            let mut premium_col = column![
                p2_regular("Premium").style(theme::text::secondary),
                row![
                    icon::coins_icon().style(theme::text::warning),
                    form::Form::new_trimmed("e.g. 5 or -3", &self.create_premium, |v| {
                        view::Message::P2P(P2PMessage::PremiumEdited(v))
                    })
                    .padding(10),
                    p1_bold(premium_label),
                ]
                .spacing(12)
                .align_y(iced::alignment::Vertical::Center),
                slider(slider_min..=slider_max, premium_val, |v: f32| {
                    view::Message::P2P(P2PMessage::PremiumEdited((v as i64).to_string()))
                })
                .step(1.0),
            ]
            .spacing(8);
            if !self.create_premium.value.is_empty() {
                if let Some(warn) = v.premium {
                    premium_col = premium_col.push(caption(warn).style(theme::text::warning));
                }
            }
            premium_col.into()
        };

        let pricing_card = card::simple(
            column![
                p2_regular("Price type").style(theme::text::secondary),
                row![market_btn, fixed_btn].spacing(8).width(Length::Fill),
                pricing_input,
            ]
            .spacing(12),
        )
        .width(Length::Fill);

        // Lightning address card (Buy orders only). When the cube has a
        // registered address and the user hasn't edited the field, show it as
        // static text with an "Edit" affordance; otherwise show the editable
        // form. If the user is editing but a registered address is available,
        // offer a way to revert.
        let lightning_address_card: Element<'a, view::Message> = if self.create_order_type
            == OrderType::Buy
        {
            let registered = cache.lightning_address.as_deref();
            let value_matches_registered =
                registered.is_some_and(|r| r == self.create_lightning_address.value);
            let show_collapsed = !self.editing_lightning_address
                && !self.lightning_address_user_edited
                && value_matches_registered;

            let body: Element<'a, view::Message> = if show_collapsed {
                let addr = self.create_lightning_address.value.as_str();
                row![
                    icon::lightning_icon().style(theme::text::warning),
                    p2_regular(addr),
                    Space::new().width(Length::Fill),
                    button::secondary_compact(None, "Use different Lightning Address or invoice")
                        .on_press(p2p(P2PMessage::EditLightningAddress)),
                ]
                .spacing(12)
                .align_y(iced::alignment::Vertical::Center)
                .into()
            } else {
                let mut col = column![row![
                    icon::lightning_icon().style(theme::text::warning),
                    form::Form::new_trimmed(
                        "Enter lightning address or invoice",
                        &self.create_lightning_address,
                        |v| { view::Message::P2P(P2PMessage::LightningAddressEdited(v)) }
                    )
                    .padding(10),
                ]
                .spacing(12)
                .align_y(iced::alignment::Vertical::Center),]
                .spacing(8);
                if registered.is_some() {
                    col = col.push(
                        row![
                            Space::new().width(Length::Fill),
                            button::secondary_compact(None, "Use registered address")
                                .on_press(p2p(P2PMessage::UseRegisteredLightningAddress)),
                        ]
                        .align_y(iced::alignment::Vertical::Center),
                    );
                }
                col.into()
            };

            let mut card_col = column![
                p2_regular("Lightning Address or Invoice").style(theme::text::secondary),
                body,
            ]
            .spacing(12);
            if self.submit_attempted {
                if let Some(warn) = v.lightning_address {
                    card_col = card_col.push(caption(warn).style(theme::text::warning));
                }
            }
            card::simple(card_col).width(Length::Fill).into()
        } else {
            column![].into()
        };

        // Expiry days
        // Error message
        let error_msg: Element<'a, view::Message> = if let Some(ref err) = self.order_submit_error {
            p2_regular(err.as_str()).style(theme::text::warning).into()
        } else {
            column![].into()
        };

        // Submit is always clickable (unless a request is already in flight) —
        // clicking with errors flips submit_attempted so warnings appear inline.
        let submit_btn = if self.order_submitting {
            button::primary(None, "Submit").width(Length::Fill)
        } else {
            button::primary(None, "Submit")
                .on_press(p2p(P2PMessage::SubmitOrder))
                .width(Length::Fill)
        };

        let form_dirty = self.create_fiat_currency != "USD"
            || !self.create_min_amount.value.is_empty()
            || !self.create_max_amount.value.is_empty()
            || !self.create_sats_amount.value.is_empty()
            || !self.create_premium.value.is_empty()
            || !self.create_payment_methods.is_empty()
            || !self.create_custom_payment_method.value.is_empty()
            || !self.create_lightning_address.value.is_empty();

        let buttons: Element<'a, view::Message> = if form_dirty && !self.order_submitting {
            row![
                button::secondary(None, "Clear")
                    .on_press(p2p(P2PMessage::ClearForm))
                    .width(Length::Fill),
                submit_btn,
            ]
            .spacing(8)
            .into()
        } else {
            row![submit_btn].into()
        };

        column![
            order_type_card,
            currency_card,
            amount_card,
            payment_card,
            pricing_card,
            lightning_address_card,
            error_msg,
            buttons,
        ]
        .spacing(16)
        .width(Length::Fill)
        .into()
    }
    fn trade_detail_view<'a>(&'a self, trade: &'a P2PTrade) -> Element<'a, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);

        // Own pending order — show order-book-style view with Cancel
        if trade.role == TradeRole::Creator && trade.status == TradeStatus::Pending {
            let badge_style = match trade.order_type {
                OrderType::Buy => theme::pill::success as fn(&_) -> _,
                OrderType::Sell => theme::pill::warning as fn(&_) -> _,
            };
            let heading = match trade.order_type {
                OrderType::Buy => "You are buying",
                OrderType::Sell => "You are selling",
            };

            let info_card = card::simple(
                column![
                    container(p2_regular(match trade.order_type {
                        OrderType::Buy => "BUYING",
                        OrderType::Sell => "SELLING",
                    }))
                    .padding([4, 12])
                    .style(badge_style),
                    p2_regular(heading).style(theme::text::secondary),
                    if trade.is_range_order() {
                        row!(
                            h2(format!(
                                "{:.0} - {:.0}",
                                trade.min_amount.unwrap_or(0.0),
                                trade.max_amount.unwrap_or(0.0)
                            )),
                            p1_bold(format!(" {}", trade.fiat_currency))
                                .style(theme::text::secondary)
                        )
                        .spacing(8)
                        .align_y(iced::alignment::Vertical::Center)
                    } else {
                        row!(
                            h2(format!("{:.2}", trade.fiat_amount)),
                            p1_bold(format!(" {}", trade.fiat_currency))
                                .style(theme::text::secondary)
                        )
                        .spacing(8)
                        .align_y(iced::alignment::Vertical::Center)
                    },
                    if trade.is_fixed_price() {
                        row![
                            p2_regular("for").style(theme::text::secondary),
                            p2_bold(format!("{} sats", trade.sats_amount.unwrap_or(0)))
                        ]
                        .spacing(8)
                    } else {
                        row![
                            p2_regular("Market Price").style(theme::text::secondary),
                            p2_bold(trade.premium_text())
                        ]
                        .spacing(8)
                    },
                    if !trade.payment_method.is_empty() {
                        container(p2_regular(&trade.payment_method))
                            .padding(12)
                            .width(Length::Fill)
                            .style(theme::container::background)
                    } else {
                        container(column![]).width(Length::Fill)
                    },
                ]
                .spacing(8),
            )
            .width(Length::Fill);

            let id_card = card::simple(
                row![
                    icon::clipboard_icon().style(theme::text::secondary),
                    column![
                        p2_regular("Order ID").style(theme::text::secondary),
                        row![
                            p2_regular(&trade.id).style(theme::text::secondary),
                            Space::new().width(Length::Fill),
                            button::secondary_compact(None, "Copy")
                                .on_press(view::Message::Clipboard(trade.id.clone())),
                        ]
                        .spacing(8)
                        .align_y(iced::alignment::Vertical::Center),
                    ]
                    .spacing(4)
                    .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(iced::alignment::Vertical::Center),
            )
            .width(Length::Fill);

            let note = column![
                p2_regular("This order will be published for 24 hours.")
                    .style(theme::text::secondary),
                p2_regular("You can cancel it anytime before someone takes it.")
                    .style(theme::text::secondary),
            ]
            .spacing(4);

            let close_btn = button::secondary(None, "Close")
                .on_press(p2p(P2PMessage::CloseTradeDetail))
                .width(Length::Fill);
            let cancel_btn = button::alert(None, "Cancel Order")
                .on_press(p2p(P2PMessage::CancelOrder(trade.id.clone())))
                .width(Length::Fill);

            return column![
                info_card,
                id_card,
                note,
                row![close_btn, cancel_btn].spacing(8),
            ]
            .spacing(12)
            .width(Length::Fill)
            .into();
        }

        let type_badge_style = match trade.order_type {
            OrderType::Buy => theme::pill::success as fn(&_) -> _,
            OrderType::Sell => theme::pill::warning as fn(&_) -> _,
        };

        let status_badge_style = match trade.status {
            TradeStatus::Active | TradeStatus::FiatSent | TradeStatus::SettledHoldInvoice => {
                theme::pill::success as fn(&_) -> _
            }
            TradeStatus::Success => theme::pill::info as fn(&_) -> _,
            TradeStatus::Pending
            | TradeStatus::WaitingPayment
            | TradeStatus::WaitingBuyerInvoice => theme::pill::simple as fn(&_) -> _,
            TradeStatus::Dispute => theme::pill::warning as fn(&_) -> _,
            TradeStatus::CooperativelyCanceled => theme::pill::warning as fn(&_) -> _,
            TradeStatus::PaymentFailed | TradeStatus::Canceled | TradeStatus::Expired => {
                theme::pill::error as fn(&_) -> _
            }
        };

        // Is user the buyer? order_type always stores OUR perspective (what we're doing).
        let is_buyer = trade.order_type == OrderType::Buy;

        // Trade info card
        let info_card = card::simple(
            column![
                // Header badges
                row![
                    container(p2_regular(trade.order_type_label()))
                        .padding([4, 12])
                        .style(type_badge_style),
                    container(p2_regular(trade.status.label()))
                        .padding([4, 12])
                        .style(status_badge_style),
                    container(p2_regular(trade.role.label()))
                        .padding([4, 12])
                        .style(theme::pill::simple as fn(&_) -> _),
                ]
                .spacing(8),
                detail_row(
                    "Amount",
                    format!("{:.2} {}", trade.fiat_amount, trade.fiat_currency)
                ),
                if trade.is_fixed_price() {
                    detail_row("Sats", format!("{}", trade.sats_amount.unwrap_or(0)))
                } else {
                    detail_row("Price", format!("Market {}", trade.premium_text()))
                },
                if !trade.payment_method.is_empty() {
                    detail_row("Payment", &trade.payment_method)
                } else {
                    column![].into()
                },
                detail_row("Created", &trade.time_ago),
            ]
            .spacing(8),
        )
        .width(Length::Fill);

        // Order ID card
        let id_card = card::simple(
            row![
                icon::clipboard_icon().style(theme::text::secondary),
                column![
                    p2_regular("Order ID").style(theme::text::secondary),
                    row![
                        p2_regular(&trade.id).style(theme::text::secondary),
                        Space::new().width(Length::Fill),
                        button::secondary_compact(None, "Copy")
                            .on_press(view::Message::Clipboard(trade.id.clone())),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                ]
                .spacing(4)
                .width(Length::Fill),
            ]
            .spacing(12)
            .align_y(iced::alignment::Vertical::Center),
        )
        .width(Length::Fill);

        // Action buttons driven by last_dm_action (DM-driven), falling back to status (event-driven)
        let mut actions = column![].spacing(8);
        let loading = self.trade_action_loading;
        let dm_action = trade.last_dm_action.as_deref();

        // Banner for trades we paid out of Spark that Mostro then
        // canceled/expired. The Lightning HTLC is still in flight at
        // this point — once Mostro cancels the hold invoice (or it
        // times out), Lightning will return the sats automatically.
        // Without this banner the user just sees "Canceled" and a
        // Pending Spark transaction with no connection between them.
        if self.spark_funded_order_ids.contains(&trade.id)
            && matches!(
                trade.status,
                TradeStatus::Canceled | TradeStatus::Expired | TradeStatus::CooperativelyCanceled
            )
        {
            actions = actions.push(
                card::simple(
                    column![
                        p1_bold("Trade canceled after Spark payment"),
                        p2_regular(
                            "Mostro canceled this trade after you paid the hold \
                             invoice from Spark. Your Spark payment will resolve \
                             automatically: Lightning returns the sats when the \
                             hold invoice times out. Check Spark → Transactions \
                             for the latest status.",
                        )
                        .style(theme::text::secondary),
                    ]
                    .spacing(8),
                )
                .width(Length::Fill),
            );
        }

        // States where rating is available
        let can_rate = matches!(
            dm_action,
            Some("HoldInvoicePaymentSettled" | "PurchaseCompleted" | "Rate")
        );

        // Terminal states — no action buttons at all
        let is_terminal = matches!(
            dm_action,
            Some(
                "RateReceived"
                    | "CooperativeCancelAccepted"
                    | "AdminSettled"
                    | "AdminCanceled"
                    | "HoldInvoicePaymentCanceled"
            )
        ) || (!can_rate
            && matches!(
                trade.status,
                TradeStatus::Success | TradeStatus::Canceled | TradeStatus::Expired
            ));

        // Cooperative cancel tracking
        let cancel_initiated_by_you = matches!(dm_action, Some("CooperativeCancelInitiatedByYou"));
        let cancel_initiated_by_peer =
            matches!(dm_action, Some("CooperativeCancelInitiatedByPeer"));
        let in_cooperative_cancel = cancel_initiated_by_you || cancel_initiated_by_peer;

        // Dispute tracking
        let dispute_initiated = matches!(
            dm_action,
            Some("DisputeInitiatedByYou" | "DisputeInitiatedByPeer" | "AdminTookDispute")
        ) || matches!(trade.status, TradeStatus::Dispute);

        // --- Countdown timer for time-limited states ---
        // Only show for PayInvoice/WaitingSellerToPay and AddInvoice/WaitingBuyerInvoice
        if let Some(start_ts) = trade.countdown_start_ts {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let expiration_secs: u64 = 900; // 15 minutes
            let expires_at = start_ts + expiration_secs;
            if expires_at > now {
                let remaining = expires_at - now;
                let mins = remaining / 60;
                let secs = remaining % 60;
                actions = actions.push(
                    container(
                        column![
                            icon::clock_icon().size(24).style(theme::text::secondary),
                            p1_bold(format!("{:02}:{:02}", mins, secs)),
                            caption("Time remaining").style(theme::text::secondary),
                        ]
                        .spacing(4)
                        .align_x(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .center_x(Length::Fill)
                    .padding([12, 0]),
                );
            } else {
                actions = actions.push(
                    container(p2_regular("Time expired").style(theme::text::warning))
                        .width(Length::Fill)
                        .center_x(Length::Fill)
                        .padding([8, 0]),
                );
            }
        }

        if !is_terminal {
            // --- Status message for cooperative cancel ---
            if cancel_initiated_by_you {
                actions = actions.push(
                    card::simple(
                        column![
                            p1_bold("Cancellation requested"),
                            p2_regular(
                                "You have initiated a cooperative cancel. \
                                Waiting for your counterparty to accept. \
                                If they don't respond, you can open a dispute.",
                            )
                            .style(theme::text::secondary),
                        ]
                        .spacing(4),
                    )
                    .width(Length::Fill),
                );
            } else if cancel_initiated_by_peer {
                actions = actions.push(
                    card::simple(
                        column![
                            p1_bold("Cancel requested by counterparty"),
                            p2_regular(
                                "Your counterparty wants to cancel this order. \
                                If you agree, press Cancel. Otherwise, you can \
                                open a dispute.",
                            )
                            .style(theme::text::secondary),
                        ]
                        .spacing(4),
                    )
                    .width(Length::Fill),
                );
            }

            // --- Status message for dispute ---
            if matches!(dm_action, Some("DisputeInitiatedByYou")) {
                actions = actions.push(
                    card::simple(
                        column![
                            p1_bold("Dispute opened"),
                            p2_regular(
                                "You have initiated a dispute. A solver will be \
                                assigned soon to help resolve this.",
                            )
                            .style(theme::text::secondary),
                        ]
                        .spacing(4),
                    )
                    .width(Length::Fill),
                );
            } else if matches!(dm_action, Some("DisputeInitiatedByPeer")) {
                actions = actions.push(
                    card::simple(
                        column![
                            p1_bold("Dispute opened by counterparty"),
                            p2_regular(
                                "Your counterparty has opened a dispute. A solver will be \
                                assigned soon to help resolve this.",
                            )
                            .style(theme::text::secondary),
                        ]
                        .spacing(4),
                    )
                    .width(Length::Fill),
                );
            } else if matches!(dm_action, Some("AdminTookDispute")) {
                actions = actions.push(
                    card::simple(
                        column![
                            p1_bold("Admin reviewing dispute"),
                            p2_regular(
                                "A dispute resolver has been assigned to your case. \
                                Use the chat below to communicate with the admin.",
                            )
                            .style(theme::text::secondary),
                        ]
                        .spacing(4),
                    )
                    .width(Length::Fill),
                );
                // Chat with Admin button
                actions = actions.push(
                    button::primary(Some(icon::chat_icon()), "Chat with Admin")
                        .on_press(p2p(P2PMessage::OpenDisputeChat))
                        .width(Length::Fill),
                );
            }

            // --- Primary trade action buttons (only if not in cooperative cancel or dispute) ---
            // Button logic follows the Mostro protocol: Mostro sends different DM
            // actions to buyer and seller, so we match on (dm_action, role) pairs.
            if !in_cooperative_cancel && !dispute_initiated {
                match dm_action {
                    // ── Seller must pay hold invoice ──
                    Some("PayInvoice") | Some("WaitingSellerToPay") => {
                        if is_buyer {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Waiting for seller payment"),
                                        p2_regular(
                                            "A payment request has been sent to the seller. \
                                            If the seller doesn't complete the payment in time, \
                                            the trade will be canceled.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        } else {
                            let mut invoice_col = column![
                                p1_bold("Pay the hold invoice"),
                                p2_regular(
                                    "Please pay the hold invoice to lock your sats \
                                    and start the trade. If you don't pay in time, \
                                    the trade will be canceled.",
                                )
                                .style(theme::text::secondary),
                            ]
                            .spacing(8);

                            invoice_col = self.push_hold_invoice_elements(
                                invoice_col,
                                &trade.id,
                                trade.hold_invoice.as_ref(),
                                trade.sats_amount,
                            );

                            actions = actions.push(card::simple(invoice_col).width(Length::Fill));
                        }
                    }

                    // ── Hold invoice paid / Trade active ──
                    Some("HoldInvoicePaymentAccepted") | Some("BuyerInvoiceAccepted") => {
                        if is_buyer {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Send fiat payment"),
                                        p2_regular(
                                            "Contact the seller to arrange payment. \
                                            Once you've sent the fiat money, \
                                            press the button below to notify the seller.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                            actions = actions.push(if !loading {
                                button::primary(None, "Confirm Fiat Sent")
                                    .on_press(p2p(P2PMessage::ConfirmFiatSent))
                                    .width(Length::Fill)
                            } else {
                                button::primary(None, "Confirm Fiat Sent").width(Length::Fill)
                            });
                        } else {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Waiting for fiat payment"),
                                        p2_regular(
                                            "Contact the buyer to inform them how to send \
                                            the fiat payment. You'll be notified when \
                                            the buyer confirms the payment.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        }
                    }
                    // BuyerTookOrder: seller is notified a buyer took their order.
                    // If the payload included a hold invoice, show it immediately.
                    Some("BuyerTookOrder") => {
                        let mut took_col = column![p1_bold("Buyer took your order"),].spacing(8);

                        if let Some(ref invoice) = trade.hold_invoice {
                            took_col = took_col.push(
                                p2_regular(
                                    "Pay the hold invoice to lock your sats \
                                    and start the trade. If you don't pay in time, \
                                    the trade will be canceled.",
                                )
                                .style(theme::text::secondary),
                            );
                            took_col = self.push_hold_invoice_elements(
                                took_col,
                                &trade.id,
                                Some(invoice),
                                trade.sats_amount,
                            );
                        } else {
                            took_col = took_col.push(
                                p2_regular(
                                    "A buyer has taken your order. Please wait while \
                                    they decide to proceed. If they accept, you'll \
                                    be notified to complete your part.",
                                )
                                .style(theme::text::secondary),
                            );
                        }

                        actions = actions.push(card::simple(took_col).width(Length::Fill));
                    }

                    // ── Buyer must submit invoice ──
                    Some("AddInvoice") | Some("WaitingBuyerInvoice") => {
                        if is_buyer {
                            let mut invoice_col = column![
                                p1_bold("Submit your invoice"),
                                p2_regular(
                                    "Please send a Lightning invoice or address \
                                    where you'll receive the sats. If you don't \
                                    provide one in time, the trade will be canceled.",
                                )
                                .style(theme::text::secondary),
                            ]
                            .spacing(8);

                            if let Some(sats) = trade.sats_amount.filter(|&s| s > 0) {
                                invoice_col = invoice_col.push(
                                    caption(format!(
                                        "Expected amount: {} sats. Use a zero-amount invoice \
                                        or one matching this amount exactly. Invoice must not \
                                        expire within 1 hour.",
                                        super::components::format_with_separators(sats),
                                    ))
                                    .style(theme::text::warning),
                                );
                            } else {
                                invoice_col = invoice_col.push(
                                    caption(
                                        "Use a zero-amount invoice or a Lightning address. \
                                        Invoice must not expire within 1 hour.",
                                    )
                                    .style(theme::text::warning),
                                );
                            }

                            invoice_col = invoice_col.push(
                                form::Form::new_trimmed(
                                    "Enter lightning invoice or address",
                                    &self.trade_invoice_input,
                                    |v| view::Message::P2P(P2PMessage::TradeInvoiceEdited(v)),
                                )
                                .padding(10),
                            );

                            invoice_col = invoice_col.push(
                                if !self.trade_invoice_input.value.is_empty() && !loading {
                                    button::primary(None, "Submit Invoice")
                                        .on_press(p2p(P2PMessage::SubmitInvoice))
                                        .width(Length::Fill)
                                } else {
                                    button::primary(None, "Submit Invoice").width(Length::Fill)
                                },
                            );

                            actions = actions.push(card::simple(invoice_col).width(Length::Fill));
                        } else {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Waiting for buyer invoice"),
                                        p2_regular(
                                            "Payment received! Your sats are now held. \
                                            The buyer has been asked to provide an invoice. \
                                            If they don't do so in time, your sats will be \
                                            returned and the trade will be canceled.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        }
                    }

                    // ── Waiting for fiat payment (status-based fallback) ──
                    Some("WaitingPayment") => {
                        if is_buyer {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Send fiat payment"),
                                        p2_regular(
                                            "Contact the seller to arrange payment. \
                                            Once you've sent the fiat money, \
                                            press the button below to notify the seller.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                            actions = actions.push(if !loading {
                                button::primary(None, "Confirm Fiat Sent")
                                    .on_press(p2p(P2PMessage::ConfirmFiatSent))
                                    .width(Length::Fill)
                            } else {
                                button::primary(None, "Confirm Fiat Sent").width(Length::Fill)
                            });
                        } else {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Waiting for fiat payment"),
                                        p2_regular(
                                            "The buyer has been notified to send fiat. \
                                            You'll be notified when they confirm.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        }
                    }

                    // ── Fiat sent / waiting for release ──
                    Some("FiatSentOk") | Some("FiatSent") => {
                        if !is_buyer {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Buyer sent fiat"),
                                        p2_regular(
                                            "The buyer has confirmed sending the fiat payment. \
                                            Once you've verified receipt, release the sats. \
                                            This action cannot be undone.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                            actions = actions.push(if !loading {
                                button::primary(None, "Release Sats")
                                    .on_press(p2p(P2PMessage::ConfirmFiatReceived))
                                    .width(Length::Fill)
                            } else {
                                button::primary(None, "Release Sats").width(Length::Fill)
                            });
                        } else {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Fiat sent"),
                                        p2_regular(
                                            "The seller has been notified that you sent the fiat. \
                                            Once the seller confirms receipt, they will release \
                                            the sats. If they don't, you can open a dispute.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        }
                    }

                    // ── Released / settling ──
                    Some("Released") | Some("Release") => {
                        actions = actions.push(
                            card::simple(
                                column![
                                    p1_bold("Sats released"),
                                    p2_regular(if is_buyer {
                                        "The seller has released the sats! Expect your \
                                        invoice to be paid shortly. Make sure your wallet \
                                        is online to receive via Lightning Network."
                                    } else {
                                        "You have released the sats. The payment is being \
                                        processed to the buyer's invoice."
                                    })
                                    .style(theme::text::secondary),
                                ]
                                .spacing(4),
                            )
                            .width(Length::Fill),
                        );
                    }

                    // ── Payment failed ──
                    Some("PaymentFailed") => {
                        if is_buyer {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Payment failed"),
                                        p2_regular(
                                            "The payment to your invoice could not be completed. \
                                            Please submit a new invoice to receive the sats.",
                                        )
                                        .style(theme::text::warning),
                                        form::Form::new_trimmed(
                                            "Enter lightning invoice or address",
                                            &self.trade_invoice_input,
                                            |v| view::Message::P2P(P2PMessage::TradeInvoiceEdited(
                                                v
                                            )),
                                        )
                                        .padding(10),
                                        if !self.trade_invoice_input.value.is_empty() && !loading {
                                            button::primary(None, "Submit Invoice")
                                                .on_press(p2p(P2PMessage::SubmitInvoice))
                                                .width(Length::Fill)
                                        } else {
                                            button::primary(None, "Submit Invoice")
                                                .width(Length::Fill)
                                        },
                                    ]
                                    .spacing(8),
                                )
                                .width(Length::Fill),
                            );
                        } else {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Payment failed"),
                                        p2_regular(
                                            "Payment to the buyer's invoice failed. \
                                            Waiting for the buyer to submit a new invoice.",
                                        )
                                        .style(theme::text::warning),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        }
                    }

                    // ── Trade completed — rate counterparty ──
                    Some("HoldInvoicePaymentSettled")
                    | Some("PurchaseCompleted")
                    | Some("Rate") => {
                        let selected = self.trade_rating;
                        let star_buttons: Vec<Element<'_, view::Message>> = (1..=5u8)
                            .map(|i| {
                                let label = if i <= selected {
                                    "\u{2605}"
                                } else {
                                    "\u{2606}"
                                };
                                iced::widget::button(iced::widget::text(label).size(28).style(
                                    if i <= selected {
                                        theme::text::primary
                                    } else {
                                        theme::text::secondary
                                    },
                                ))
                                .on_press(p2p(P2PMessage::RatingSelected(i)))
                                .style(theme::button::container)
                                .into()
                            })
                            .collect();
                        let stars = iced::widget::Row::with_children(star_buttons).spacing(4);

                        actions = actions.push(
                            card::simple(
                                column![
                                    p1_bold("Trade Complete!"),
                                    p2_regular("Rate your counterparty")
                                        .style(theme::text::secondary),
                                    container(
                                        column![
                                            stars,
                                            p2_regular(if selected > 0 {
                                                format!("{} / 5", selected)
                                            } else {
                                                "Select a rating".to_string()
                                            })
                                            .style(theme::text::secondary),
                                        ]
                                        .spacing(4)
                                        .align_x(Alignment::Center),
                                    )
                                    .width(Length::Fill)
                                    .center_x(Length::Fill),
                                    if selected > 0 && !loading {
                                        button::primary(None, "Submit Rating")
                                            .on_press(p2p(P2PMessage::SubmitRating))
                                            .width(Length::Fill)
                                    } else {
                                        button::primary(None, "Submit Rating").width(Length::Fill)
                                    },
                                ]
                                .spacing(12),
                            )
                            .width(Length::Fill),
                        );
                    }

                    _ => {
                        // No DM action or unrecognized — fall back to status-based buttons
                        match trade.status {
                            TradeStatus::WaitingBuyerInvoice if is_buyer => {
                                actions = actions.push(
                                    card::simple(
                                        column![
                                            p2_regular(
                                                "Submit your Lightning invoice to receive sats"
                                            )
                                            .style(theme::text::secondary),
                                            form::Form::new_trimmed(
                                                "Enter lightning invoice or address",
                                                &self.trade_invoice_input,
                                                |v| view::Message::P2P(
                                                    P2PMessage::TradeInvoiceEdited(v)
                                                ),
                                            )
                                            .padding(10),
                                            if !self.trade_invoice_input.value.is_empty()
                                                && !loading
                                            {
                                                button::primary(None, "Submit Invoice")
                                                    .on_press(p2p(P2PMessage::SubmitInvoice))
                                                    .width(Length::Fill)
                                            } else {
                                                button::primary(None, "Submit Invoice")
                                                    .width(Length::Fill)
                                            },
                                        ]
                                        .spacing(8),
                                    )
                                    .width(Length::Fill),
                                );
                            }
                            TradeStatus::Active if is_buyer => {
                                actions = actions.push(if !loading {
                                    button::primary(None, "Confirm Fiat Sent")
                                        .on_press(p2p(P2PMessage::ConfirmFiatSent))
                                        .width(Length::Fill)
                                } else {
                                    button::primary(None, "Confirm Fiat Sent").width(Length::Fill)
                                });
                            }
                            TradeStatus::FiatSent if !is_buyer => {
                                actions = actions.push(if !loading {
                                    button::primary(None, "Release (Confirm Fiat Received)")
                                        .on_press(p2p(P2PMessage::ConfirmFiatReceived))
                                        .width(Length::Fill)
                                } else {
                                    button::primary(None, "Release (Confirm Fiat Received)")
                                        .width(Length::Fill)
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }

            // --- Cancel / Dispute footer buttons ---
            // Dispute only available once the trade is active (hold invoice paid)
            let can_dispute = matches!(
                trade.status,
                TradeStatus::Active | TradeStatus::FiatSent | TradeStatus::CooperativelyCanceled
            );
            // Hide once the trade is complete or past release
            let trade_complete =
                matches!(
                    dm_action,
                    Some(
                        "Released"
                            | "Release"
                            | "HoldInvoicePaymentSettled"
                            | "PurchaseCompleted"
                            | "Rate"
                            | "RateReceived"
                            | "AdminSettled"
                            | "AdminCanceled"
                            | "CooperativeCancelAccepted"
                    )
                ) || matches!(trade.status, TradeStatus::Success | TradeStatus::Canceled);
            if !loading && !trade_complete {
                if cancel_initiated_by_peer {
                    let mut btn_row = row![
                        button::secondary(None, "Close")
                            .on_press(p2p(P2PMessage::CloseTradeDetail))
                            .width(Length::Fill),
                        button::alert(None, "Accept Cancel")
                            .on_press(p2p(P2PMessage::CancelTrade))
                            .width(Length::Fill),
                    ]
                    .spacing(8);
                    if can_dispute {
                        btn_row = btn_row.push(
                            button::alert(None, "Dispute")
                                .on_press(p2p(P2PMessage::OpenDispute))
                                .width(Length::Fill),
                        );
                    }
                    actions = actions.push(btn_row);
                } else if cancel_initiated_by_you {
                    let mut btn_row = row![button::secondary(None, "Close")
                        .on_press(p2p(P2PMessage::CloseTradeDetail))
                        .width(Length::Fill),]
                    .spacing(8);
                    if can_dispute {
                        btn_row = btn_row.push(
                            button::alert(None, "Dispute")
                                .on_press(p2p(P2PMessage::OpenDispute))
                                .width(Length::Fill),
                        );
                    }
                    actions = actions.push(btn_row);
                } else if !dispute_initiated {
                    let mut btn_row = row![
                        button::secondary(None, "Close")
                            .on_press(p2p(P2PMessage::CloseTradeDetail))
                            .width(Length::Fill),
                        button::secondary(None, "Cancel")
                            .on_press(p2p(P2PMessage::CancelTrade))
                            .width(Length::Fill),
                    ]
                    .spacing(8);
                    if can_dispute {
                        btn_row = btn_row.push(
                            button::alert(None, "Dispute")
                                .on_press(p2p(P2PMessage::OpenDispute))
                                .width(Length::Fill),
                        );
                    }
                    actions = actions.push(btn_row);
                } else {
                    // In dispute — only Close
                    actions = actions.push(
                        button::secondary(None, "Close")
                            .on_press(p2p(P2PMessage::CloseTradeDetail))
                            .width(Length::Fill),
                    );
                }
            } else {
                // Terminal / loading / past release — just Close
                actions = actions.push(
                    button::secondary(None, "Close")
                        .on_press(p2p(P2PMessage::CloseTradeDetail))
                        .width(Length::Fill),
                );
            }
        } else {
            // Terminal state — only Close
            actions = actions.push(
                button::secondary(None, "Close")
                    .on_press(p2p(P2PMessage::CloseTradeDetail))
                    .width(Length::Fill),
            );
        }

        // ── Contact button ──
        // Show contact/chat button for active trades only (not completed/canceled)
        let chat_available = matches!(
            trade.status,
            TradeStatus::Active
                | TradeStatus::FiatSent
                | TradeStatus::SettledHoldInvoice
                | TradeStatus::CooperativelyCanceled
                | TradeStatus::Dispute
                | TradeStatus::PaymentFailed
        );

        let mut content = column![info_card, id_card, actions]
            .spacing(12)
            .width(Length::Fill);

        if chat_available {
            content = content.push(
                button::secondary(Some(icon::chat_icon()), "Contact")
                    .on_press(p2p(P2PMessage::OpenChat))
                    .width(Length::Fill),
            );
        }

        content.into()
    }

    fn chat_view<'a>(&'a self, trade: &'a P2PTrade) -> Element<'a, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);

        let chat_enabled = trade.counterparty_pubkey.is_some();

        let chat_messages: Vec<&super::mostro::TradeMessage> = self
            .cached_trade_messages
            .iter()
            .filter(|m| m.action == "SendDm")
            .collect();

        // Nicknames for chat bubbles
        let peer_nick = trade
            .counterparty_pubkey
            .as_deref()
            .map(super::mostro::nickname_from_pubkey)
            .unwrap_or_else(|| "Peer".to_string());

        // ── Accordion tab buttons (Trade Information / User Information) ──
        let trade_info_btn = if self.chat_show_trade_info {
            button::primary(Some(icon::receipt_icon()), "Trade Information")
                .on_press(p2p(P2PMessage::ToggleChatTradeInfo))
                .width(Length::Fill)
        } else {
            button::secondary(Some(icon::receipt_icon()), "Trade Information")
                .on_press(p2p(P2PMessage::ToggleChatTradeInfo))
                .width(Length::Fill)
        };
        let user_info_btn = if self.chat_show_user_info {
            button::primary(Some(icon::person_icon()), "User Information")
                .on_press(p2p(P2PMessage::ToggleChatUserInfo))
                .width(Length::Fill)
        } else {
            button::secondary(Some(icon::person_icon()), "User Information")
                .on_press(p2p(P2PMessage::ToggleChatUserInfo))
                .width(Length::Fill)
        };
        let tab_row = container(row![trade_info_btn, user_info_btn].spacing(8))
            .padding([12, 16])
            .width(Length::Fill);

        // ── Trade Information panel (expandable) ──
        let order_short = &trade.id[..8.min(trade.id.len())];
        let trade_type_label = match trade.order_type {
            OrderType::Buy => "Buying",
            OrderType::Sell => "Selling",
        };
        let sats_text = trade
            .sats_amount
            .map(|s| format!("{} sats", super::components::format_with_separators(s)))
            .unwrap_or_else(|| "Market price".to_string());
        let created_date = {
            let dt = chrono::DateTime::from_timestamp(trade.created_at_ts, 0)
                .unwrap_or_default()
                .with_timezone(&chrono::Local);
            dt.format("%B %d, %Y").to_string()
        };

        let trade_info_panel: Element<'_, view::Message> = if self.chat_show_trade_info {
            container(
                column![
                    // Order ID
                    row![
                        p2_regular("Order ID:").style(theme::text::secondary),
                        Space::new().width(Length::Fill),
                        p2_regular(order_short),
                    ]
                    .spacing(8),
                    // Trade summary
                    container(
                        column![
                            p1_bold(format!(
                                "{} {} {}",
                                trade_type_label, sats_text, trade.fiat_currency
                            )),
                            p2_regular(format!(
                                "for {:.2} {}",
                                trade.fiat_amount, trade.fiat_currency
                            ))
                            .style(theme::text::secondary),
                        ]
                        .spacing(4),
                    )
                    .padding(12)
                    .width(Length::Fill)
                    .style(chat_info_card as fn(&_) -> _),
                    // Payment method
                    row![
                        icon::cash_icon().style(theme::text::secondary),
                        column![
                            p2_regular("Payment Method").style(theme::text::secondary),
                            p2_bold(&trade.payment_method),
                        ]
                        .spacing(2),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                    // Created date
                    row![
                        icon::calendar_icon().style(theme::text::secondary),
                        column![
                            p2_regular("Created on").style(theme::text::secondary),
                            p2_bold(created_date),
                        ]
                        .spacing(2),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                ]
                .spacing(12),
            )
            .padding(iced::Padding {
                top: 0.0,
                right: 16.0,
                bottom: 12.0,
                left: 16.0,
            })
            .width(Length::Fill)
            .into()
        } else {
            Space::new().height(0).into()
        };

        // ── User Information panel (expandable) ──
        let user_info_panel: Element<'_, view::Message> = if self.chat_show_user_info {
            let default_identity = super::mostro::ChatIdentityInfo {
                counterparty_pubkey: None,
                counterparty_nickname: None,
                our_trade_pubkey: None,
                our_nickname: None,
                shared_key: None,
            };
            let identity = self
                .cached_chat_identity
                .as_ref()
                .unwrap_or(&default_identity);
            let cp_nickname = identity
                .counterparty_nickname
                .clone()
                .unwrap_or_else(|| "Unknown".to_string());
            let cp_pubkey_full = identity
                .counterparty_pubkey
                .as_deref()
                .unwrap_or("Unknown")
                .to_string();
            let our_nickname = identity
                .our_nickname
                .clone()
                .unwrap_or_else(|| "Unknown".to_string());
            let our_pubkey_full = identity
                .our_trade_pubkey
                .as_deref()
                .unwrap_or("Unknown")
                .to_string();
            let shared_key_full = identity
                .shared_key
                .as_deref()
                .unwrap_or("Not available")
                .to_string();

            container(
                column![
                    // Counterparty
                    row![
                        icon::person_icon().style(theme::text::secondary),
                        column![
                            p2_regular("Peer").style(theme::text::secondary),
                            p1_bold(cp_nickname),
                            caption(cp_pubkey_full).style(theme::text::secondary),
                        ]
                        .spacing(2),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                    // Our trade key
                    row![
                        icon::key_icon().style(theme::text::secondary),
                        column![
                            p2_regular("You").style(theme::text::secondary),
                            p1_bold(our_nickname),
                            caption(our_pubkey_full).style(theme::text::secondary),
                        ]
                        .spacing(2),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                    // Shared key
                    row![
                        icon::lock_icon().style(theme::text::secondary),
                        column![
                            p2_regular("Shared Key").style(theme::text::secondary),
                            p2_bold(shared_key_full),
                        ]
                        .spacing(2),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                ]
                .spacing(12),
            )
            .padding(iced::Padding {
                top: 0.0,
                right: 16.0,
                bottom: 12.0,
                left: 16.0,
            })
            .width(Length::Fill)
            .into()
        } else {
            Space::new().height(0).into()
        };

        // Thin divider
        let divider: Element<'_, view::Message> =
            container(Space::new().height(1).width(Length::Fill))
                .style(
                    |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                        background: Some(iced::Background::Color(iced::color!(0x333333))),
                        ..Default::default()
                    },
                )
                .into();

        // ── Message list ──
        let mut msg_col = column![].spacing(10).padding([12, 16]);

        if chat_messages.is_empty() {
            msg_col = msg_col.push(
                container(
                    column![
                        icon::chat_icon().size(40.0).style(theme::text::secondary),
                        p1_regular("No messages yet").style(theme::text::secondary),
                        p2_regular("Send a message to start the conversation")
                            .style(theme::text::secondary),
                    ]
                    .spacing(8)
                    .align_x(iced::alignment::Horizontal::Center),
                )
                .padding(60)
                .width(Length::Fill)
                .center_x(Length::Fill),
            );
        } else {
            for msg in &chat_messages {
                let ts = chrono::DateTime::from_timestamp(msg.timestamp as i64, 0)
                    .unwrap_or_default()
                    .with_timezone(&chrono::Local);
                let time_str = ts.format("%H:%M").to_string();

                // Check if this is an attachment (image or file)
                let content: Element<'_, view::Message> = if let Some(meta) =
                    super::mostro::parse_attachment_metadata(&msg.payload_json)
                {
                    let filename_label = meta.filename().to_string();
                    let blossom_url = meta.blossom_url().to_string();
                    match &meta {
                        super::mostro::AttachmentMeta::Image(img_meta) => {
                            match self.image_cache.get(&blossom_url) {
                                Some(ImageCacheEntry::Ready(handle)) => {
                                    let display_width = (img_meta.width).min(360);
                                    // Image with filename overlaid at bottom
                                    let img_widget = iced::widget::image(handle.clone())
                                        .width(display_width as f32);
                                    let overlay_label = container(
                                        caption(filename_label).style(theme::text::secondary),
                                    )
                                    .padding([4, 8]);
                                    column![img_widget, overlay_label].spacing(0).into()
                                }
                                Some(ImageCacheEntry::Loading) => container(
                                    column![
                                        p2_regular("Loading image...")
                                            .style(theme::text::secondary),
                                        caption(filename_label).style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .padding([16, 20])
                                .width(Length::Fixed(200.0))
                                .into(),
                                Some(ImageCacheEntry::Failed(e)) => container(
                                    column![
                                        p2_regular(format!("Failed: {e}"))
                                            .style(theme::text::warning),
                                        caption(filename_label).style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .padding([16, 20])
                                .into(),
                                None => container(
                                    column![
                                        p2_regular("Loading image...")
                                            .style(theme::text::secondary),
                                        caption(filename_label).style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .padding([16, 20])
                                .width(Length::Fixed(200.0))
                                .into(),
                            }
                        }
                        super::mostro::AttachmentMeta::File(file_meta) => {
                            let size_label = if file_meta.original_size < 1024 {
                                format!("{} B", file_meta.original_size)
                            } else if file_meta.original_size < 1024 * 1024 {
                                format!("{:.1} KB", file_meta.original_size as f64 / 1024.0)
                            } else {
                                format!(
                                    "{:.1} MB",
                                    file_meta.original_size as f64 / (1024.0 * 1024.0)
                                )
                            };
                            let save_url = file_meta.blossom_url.clone();
                            let save_name = file_meta.filename.clone();
                            // File card with info + download button (like mobile)
                            let info_row = row![
                                icon::tooltip_icon().size(32).style(theme::text::secondary),
                                column![
                                    p1_bold(filename_label),
                                    caption(format!("{} · Encrypted", size_label))
                                        .style(theme::text::secondary),
                                ]
                                .spacing(2),
                            ]
                            .spacing(10)
                            .align_y(iced::alignment::Vertical::Center);

                            let download_btn = button::secondary(None, "Download")
                                .on_press(p2p(P2PMessage::SaveFile {
                                    blossom_url: save_url,
                                    filename: save_name,
                                }))
                                .width(Length::Fill);

                            column![info_row, download_btn]
                                .spacing(8)
                                .width(Length::Fixed(260.0))
                                .into()
                        }
                    }
                } else {
                    let text = extract_chat_text(&msg.payload_json);
                    p1_regular(text).into()
                };

                if msg.is_own {
                    msg_col = msg_col.push(
                        column![
                            container(caption("You").style(theme::text::secondary))
                                .width(Length::Fill)
                                .align_right(Length::Fill),
                            row![
                                Space::new().width(Length::FillPortion(3)),
                                container(content)
                                    .padding([10, 16])
                                    .style(chat_bubble_own as fn(&_) -> _)
                                    .max_width(480),
                            ],
                            container(caption(time_str).style(theme::text::secondary))
                                .width(Length::Fill)
                                .align_right(Length::Fill),
                        ]
                        .spacing(2),
                    );
                } else {
                    msg_col = msg_col.push(
                        column![
                            caption(peer_nick.clone()).style(theme::text::primary),
                            row![
                                container(content)
                                    .padding([10, 16])
                                    .style(chat_bubble_peer as fn(&_) -> _)
                                    .max_width(480),
                                Space::new().width(Length::FillPortion(3)),
                            ],
                            caption(time_str).style(theme::text::secondary),
                        ]
                        .spacing(2),
                    );
                }
            }
        }

        // Show "Sending image..." indicator while upload is in progress
        if self.attachment_sending {
            msg_col = msg_col.push(
                column![
                    container(caption("You").style(theme::text::secondary))
                        .width(Length::Fill)
                        .align_right(Length::Fill),
                    row![
                        Space::new().width(Length::FillPortion(3)),
                        container(
                            p2_regular("Sending attachment...").style(theme::text::secondary),
                        )
                        .padding([10, 16])
                        .style(chat_bubble_own as fn(&_) -> _)
                        .max_width(480),
                    ],
                ]
                .spacing(2),
            );
        }

        let chat_scroll = iced::widget::scrollable(msg_col)
            .height(Length::Fill)
            .anchor_bottom();

        // ── Input area ──
        let input_area: Element<'_, view::Message> = if chat_enabled {
            let can_send =
                !self.chat_input.value.trim().is_empty() && self.pending_chat_message.is_none();
            let mut send_btn = iced::widget::button(icon::send_icon().size(18))
                .padding([8, 10])
                .style(theme::button::primary);
            if can_send {
                send_btn = send_btn.on_press(p2p(P2PMessage::SendChatMessage));
            }
            let can_attach = self.pending_chat_message.is_none() && !self.attachment_sending;
            let mut attach_btn = iced::widget::button(icon::plus_icon().size(18))
                .padding([8, 10])
                .style(theme::button::secondary);
            if can_attach {
                attach_btn = attach_btn.on_press(p2p(P2PMessage::AttachFile));
            }
            container(
                row![
                    attach_btn,
                    form::Form::new("Type a message...", &self.chat_input, |v| {
                        view::Message::P2P(P2PMessage::ChatInputEdited(v))
                    })
                    .padding(10),
                    send_btn,
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center),
            )
            .padding([12, 16])
            .width(Length::Fill)
            .into()
        } else if !chat_messages.is_empty() {
            container(p2_regular("Chat is read-only").style(theme::text::secondary))
                .padding([12, 16])
                .width(Length::Fill)
                .center_x(Length::Fill)
                .into()
        } else {
            Space::new().height(0).into()
        };

        column![
            tab_row,
            trade_info_panel,
            user_info_panel,
            divider,
            chat_scroll,
            input_area,
        ]
        .spacing(0)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    /// Build the `Task` that fetches the Spark wallet's spendable balance
    /// for a given session id. Mirrors GlobalHome's
    /// `SparkBalanceUpdated` calculation: raw BTC sats plus the
    /// USDB-as-sats equivalent, because Stable Balance users keep
    /// their funds in USDB and the SDK auto-converts to BTC at send
    /// time. Without this, the `info.balance_sats` field would report
    /// 0 for any user with Stable Balance enabled even though the
    /// wallet can pay a Lightning invoice — the original cause of
    /// the "Pay from Spark" button never appearing.
    ///
    /// Returns `None` when the Spark backend isn't wired up for this
    /// cube.
    fn spark_balance_fetch_task(&self, cache: &Cache, session_id: String) -> Option<Task<Message>> {
        let spark = self.spark_backend.clone()?;
        let reference_price = cache.btc_usd_price.or_else(|| {
            let converter: Option<view::FiatAmountConverter> =
                cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
            converter.map(|c| c.price_per_btc())
        });
        Some(Task::perform(
            async move { spark.get_info().await },
            move |result| match result {
                Ok(info) => {
                    let usdb_sats = info
                        .stable_balance
                        .map(|sb| {
                            crate::app::breez_spark::assets::stable_token_as_sats(
                                sb.balance,
                                sb.decimals,
                                reference_price,
                            )
                        })
                        .unwrap_or(0);
                    Message::View(view::Message::P2P(P2PMessage::SparkBalanceLoaded {
                        order_id: session_id.clone(),
                        balance_sat: info.balance_sats.saturating_add(usdb_sats),
                    }))
                }
                Err(e) => Message::View(view::Message::P2P(P2PMessage::SparkBalanceFailed {
                    order_id: session_id.clone(),
                    err: e.to_string(),
                })),
            },
        ))
    }

    /// Pre-parse the hold invoice via `spark.parse_input` so we can
    /// show "Lock amount: X sats" in the Spark-pay summary before the
    /// user commits. Fires alongside the balance fetch. Failure (or
    /// a missing amount in the invoice) emits the message with
    /// `amount_sat: None` and is otherwise a no-op — this is a
    /// cosmetic enhancement, not a gate.
    fn spark_parse_invoice_task(
        &self,
        session_id: String,
        invoice: String,
    ) -> Option<Task<Message>> {
        let spark = self.spark_backend.clone()?;
        Some(Task::perform(
            async move { spark.parse_input(invoice).await },
            move |result| match result {
                Ok(parsed) => {
                    Message::View(view::Message::P2P(P2PMessage::SparkInvoiceAmountParsed {
                        order_id: session_id.clone(),
                        amount_sat: parsed.amount_sat,
                    }))
                }
                Err(e) => {
                    tracing::debug!(
                        target: "p2p::spark_pay",
                        "parse_input failed for {}: {} — Spark-pay summary will say TBD",
                        session_id,
                        e,
                    );
                    Message::View(view::Message::P2P(P2PMessage::SparkInvoiceAmountParsed {
                        order_id: session_id.clone(),
                        amount_sat: None,
                    }))
                }
            },
        ))
    }

    /// Whether the cube's Spark balance is sufficient to cover a hold
    /// invoice of `hold_amount_sat`. Market-priced sell orders leave
    /// Mostro's `sats_amount = None` until settlement, in which case we
    /// answer optimistically when the balance is positive — `prepare_send`
    /// will pull the real amount from the BOLT11 invoice and the preview
    /// step gives the user a chance to back out.
    fn spark_can_cover(&self, hold_amount_sat: Option<u64>) -> bool {
        match (self.spark_balance_sat, hold_amount_sat) {
            (Some(bal), Some(amt)) => bal >= amt.saturating_add(spark_pay_fee_buffer_sats(amt)),
            (Some(bal), None) => bal > 0,
            _ => false,
        }
    }

    /// Append hold invoice elements to a column: either Spark-pay UX
    /// (when the cube can cover the hold amount and the user hasn't
    /// toggled to QR), or QR + Copy Invoice for paying from another
    /// wallet. Falls back to a warning when no invoice is available.
    ///
    /// `sats_amount` is the trade's locked-amount in sats; when known
    /// it gates the Spark-pay path. None defaults to QR-only so we
    /// never offer "Pay from Spark" without knowing whether the
    /// balance covers it.
    fn push_hold_invoice_elements<'a>(
        &'a self,
        col: Column<'a, view::Message>,
        order_id: &str,
        invoice: Option<&String>,
        sats_amount: Option<u64>,
    ) -> Column<'a, view::Message> {
        let mut col = col;
        let Some(invoice) = invoice else {
            return col.push(
                caption(
                    "Hold invoice not available. \
                    Please check your external wallet for a pending invoice.",
                )
                .style(theme::text::warning),
            );
        };

        // Spark-pay gate: backend exists and `spark_can_cover` says yes.
        // See `spark_can_cover` for the market-priced-order rationale
        // behind accepting a `None` amount when the balance is positive.
        // If the balance turns out to be too low at `prepare_send` time
        // we surface the failure in the Error phase with a "Pay from
        // another wallet" escape hatch.
        let spark_can_cover = self.spark_can_cover(sats_amount);
        let spark_mode = self.spark_backend.is_some() && spark_can_cover && !self.show_qr_fallback;
        tracing::debug!(
            target: "p2p::spark_pay",
            "push_hold_invoice_elements order_id={} spark_backend={} \
             spark_balance_sat={:?} sats_amount={:?} show_qr_fallback={} \
             session_id={:?} spark_can_cover={} spark_mode={}",
            order_id,
            self.spark_backend.is_some(),
            self.spark_balance_sat,
            sats_amount,
            self.show_qr_fallback,
            self.spark_pay_session_id,
            spark_can_cover,
            spark_mode,
        );

        if spark_mode {
            // Prefer the parsed BOLT11 amount (populated alongside the
            // balance fetch via `spark_parse_invoice_task`) so market-priced
            // sell orders show a real number instead of "TBD". Fall back
            // to `trade.sats_amount` when Mostro already settled it.
            let hold_amount = self.spark_pay_amount_sat.or(sats_amount);
            col = col.push(self.spark_pay_action_body(order_id, invoice, hold_amount, None));
        } else {
            if let Some(ref qr_data) = self.hold_invoice_qr {
                col = col.push(
                    container(
                        container(
                            iced::widget::QRCode::<coincube_ui::theme::Theme>::new(qr_data)
                                .cell_size(2),
                        )
                        .padding(10)
                        .style(|_| {
                            iced::widget::container::Style::default().background(iced::Color::WHITE)
                        })
                        .max_width(280)
                        .max_height(280),
                    )
                    .width(Length::Fill)
                    .center_x(Length::Fill),
                );
            }
            col = col.push(
                button::primary(None, "Copy Invoice")
                    .on_press(view::Message::Clipboard(invoice.clone()))
                    .width(Length::Fill),
            );
            // Surface a way back to Spark when the cube *could* pay
            // from Spark but the user is currently in the QR fallback.
            if self.spark_backend.is_some() && spark_can_cover && self.show_qr_fallback {
                col = col.push(
                    button::transparent(None, "← Pay with Spark")
                        .on_press(view::Message::P2P(P2PMessage::ToggleQrFallback(false))),
                );
            }
        }
        col
    }

    /// Phase-aware Spark-pay body for the trade-detail view. Mirrors
    /// the modal's `spark_pay_body` but without a "Cancel Order"
    /// button — the trade-detail page has its own page-level Cancel
    /// affordance, so adding another here would be confusing and
    /// double the cancel-race surface.
    /// Render the Spark-pay action body shared by the inline (trade detail)
    /// and modal variants. The modal passes `Some(cancel_button)` so the
    /// "Cancel Order" affordance appears in Idle/Prepared (alongside the
    /// "Pay from another wallet" toggle) and standalone in Error. The
    /// inline variant passes `None`. Preparing/Sending never expose cancel
    /// or the QR toggle — those phases own the order state until the
    /// Spark RPC resolves; surfacing a cancel here invites a double-spend
    /// race with Mostro.
    fn spark_pay_action_body<'a>(
        &'a self,
        order_id: &str,
        invoice: &str,
        hold_amount_sat: Option<u64>,
        cancel_button: Option<Button<'a, view::Message>>,
    ) -> Element<'a, view::Message> {
        let balance_text = self
            .spark_balance_sat
            .map(|b| format!("Spark balance: {} sats", b))
            .unwrap_or_default();
        let hold_text = hold_amount_sat
            .map(|a| format!("Lock amount: {} sats", a))
            .unwrap_or_else(|| "Lock amount: TBD".to_string());

        let summary = container(
            column![
                p2_regular(hold_text).style(theme::text::primary),
                p2_regular(balance_text).style(theme::text::secondary),
            ]
            .spacing(4)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .center_x(Length::Fill);

        let action: Element<'a, view::Message> = match &self.spark_pay_phase {
            SparkPayPhase::Idle => {
                let switch_to_qr = button::transparent(None, "Pay from another wallet")
                    .on_press(view::Message::P2P(P2PMessage::ToggleQrFallback(true)));
                let bottom: Element<'a, view::Message> = match cancel_button {
                    Some(c) => row![switch_to_qr, c].spacing(8).into(),
                    None => switch_to_qr.into(),
                };
                column![
                    button::primary(None, "Pay from Spark")
                        .on_press(view::Message::P2P(P2PMessage::SparkPayPrepare {
                            order_id: order_id.to_string(),
                            invoice: invoice.to_string(),
                        }))
                        .width(Length::Fill),
                    bottom,
                ]
                .spacing(8)
                .into()
            }
            SparkPayPhase::Preparing => {
                column![p2_regular("Preparing payment…").style(theme::text::secondary),]
                    .spacing(8)
                    .align_x(Alignment::Center)
                    .into()
            }
            SparkPayPhase::Prepared(preview) => {
                let switch_to_qr = button::transparent(None, "Pay from another wallet")
                    .on_press(view::Message::P2P(P2PMessage::ToggleQrFallback(true)));
                let bottom: Element<'a, view::Message> = match cancel_button {
                    Some(c) => row![switch_to_qr, c].spacing(8).into(),
                    None => switch_to_qr.into(),
                };
                let total = preview.amount_sat.saturating_add(preview.fee_sat);
                column![
                    container(
                        column![
                            detail_row("Amount", format!("{} sats", preview.amount_sat)),
                            detail_row("Network fee", format!("{} sats", preview.fee_sat)),
                            detail_row("Total", format!("{} sats", total)),
                        ]
                        .spacing(4),
                    )
                    .width(Length::Fill),
                    row![
                        button::primary(None, "Confirm and pay")
                            .on_press(view::Message::P2P(P2PMessage::SparkPayConfirm))
                            .width(Length::Fill),
                        button::transparent(None, "Back")
                            .on_press(view::Message::P2P(P2PMessage::SparkPayCancel))
                            .width(Length::Fill),
                    ]
                    .spacing(8),
                    bottom,
                ]
                .spacing(12)
                .into()
            }
            SparkPayPhase::Sending => {
                column![p2_regular("Sending payment…").style(theme::text::secondary),]
                    .spacing(8)
                    .align_x(Alignment::Center)
                    .into()
            }
            SparkPayPhase::Error(msg) => {
                let mut col = column![
                    p2_regular(format!("Spark payment failed: {}", msg))
                        .style(theme::text::warning),
                    row![
                        button::primary(None, "Try again")
                            .on_press(view::Message::P2P(P2PMessage::SparkPayPrepare {
                                order_id: order_id.to_string(),
                                invoice: invoice.to_string(),
                            }))
                            .width(Length::Fill),
                        button::transparent(None, "Pay from another wallet")
                            .on_press(view::Message::P2P(P2PMessage::ToggleQrFallback(true)))
                            .width(Length::Fill),
                    ]
                    .spacing(8),
                ]
                .spacing(12);
                if let Some(c) = cancel_button {
                    col = col.push(c);
                }
                col.into()
            }
        };

        column![summary, action]
            .spacing(16)
            .align_x(Alignment::Center)
            .into()
    }

    fn payment_invoice_modal_view<'a>(
        &'a self,
        order_id: &str,
        invoice: &str,
        amount_sats: Option<i64>,
        qr_data: &'a qr_code::Data,
    ) -> Element<'a, view::Message> {
        let amount_text = match amount_sats {
            Some(amt) => format!("{} sats", amt),
            None => "amount TBD".to_string(),
        };

        let truncated_id = if order_id.len() > 8 {
            &order_id[..8]
        } else {
            order_id
        };

        // Decide whether the cube's Spark balance covers this trade.
        // A small fee buffer prevents promising "Pay from Spark" only
        // for `prepare_send` to fail on routing fees.
        let hold_amount_sat: Option<u64> = amount_sats.and_then(|a| u64::try_from(a).ok());
        let spark_can_cover = self.spark_can_cover(hold_amount_sat);
        let spark_mode = self.spark_backend.is_some() && spark_can_cover && !self.show_qr_fallback;

        let header = container(
            column![
                p1_bold("Payment Required"),
                p2_regular(format!(
                    "Order {} taken. Pay this hold invoice to lock {} for the trade.",
                    truncated_id, amount_text
                ))
                .style(theme::text::secondary),
            ]
            .spacing(8)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .center_x(Length::Fill);

        let cancel_button = button::alert(None, "Cancel Order")
            .on_press(view::Message::P2P(P2PMessage::CancelPaymentInvoice(
                order_id.to_string(),
            )))
            .width(Length::Fill);

        let body: Element<'a, view::Message> = if spark_mode {
            self.spark_pay_action_body(order_id, invoice, hold_amount_sat, Some(cancel_button))
        } else {
            self.qr_pay_body(invoice, order_id, qr_data, cancel_button)
        };

        card::simple(column![header, body].spacing(16).align_x(Alignment::Center))
            .width(Length::Fixed(450.0))
            .into()
    }

    /// QR / Copy Invoice / Cancel Order — the legacy "pay from external
    /// wallet" body. Used when Spark is unavailable, balance is too
    /// low, or the user clicked "Pay from another wallet".
    fn qr_pay_body<'a>(
        &'a self,
        invoice: &str,
        _order_id: &str,
        qr_data: &'a qr_code::Data,
        cancel_button: Button<'a, view::Message>,
    ) -> Element<'a, view::Message> {
        let mut col = column![
            container(
                container(
                    iced::widget::QRCode::<coincube_ui::theme::Theme>::new(qr_data).cell_size(2),
                )
                .padding(10)
                .style(|_| {
                    iced::widget::container::Style::default().background(iced::Color::WHITE)
                })
                .max_width(280)
                .max_height(280),
            )
            .width(Length::Fill)
            .center_x(Length::Fill),
            row![
                if self.invoice_copied {
                    button::secondary(None, "Copied!").width(Length::Fill)
                } else {
                    button::primary(None, "Copy Invoice")
                        .on_press(view::Message::P2P(P2PMessage::CopyPaymentInvoice(
                            invoice.to_string(),
                        )))
                        .width(Length::Fill)
                },
                cancel_button,
            ]
            .spacing(8),
        ]
        .spacing(16)
        .align_x(Alignment::Center);

        // Offer a way back to Spark-pay if the cube has a sufficient
        // balance — used when the user toggled into the fallback by
        // mistake. Hidden when Spark can't cover the hold amount
        // (rendering the toggle would be a dead end).
        let hold_amount_sat = self
            .pending_payment_invoice
            .as_ref()
            .and_then(|(_, _, a, _)| a.and_then(|n| u64::try_from(n).ok()));
        let spark_can_cover = self.spark_can_cover(hold_amount_sat);
        if self.spark_backend.is_some() && spark_can_cover && self.show_qr_fallback {
            col = col.push(
                button::transparent(None, "← Back to Spark")
                    .on_press(view::Message::P2P(P2PMessage::ToggleQrFallback(false))),
            );
        }
        col.into()
    }

    fn dispute_chat_view<'a>(&'a self, trade: &'a P2PTrade) -> Element<'a, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);

        let chat_enabled =
            trade.admin_pubkey.is_some() && matches!(trade.status, TradeStatus::Dispute);

        let admin_messages: Vec<&super::mostro::TradeMessage> = self
            .cached_trade_messages
            .iter()
            .filter(|m| m.action == "AdminDm")
            .collect();

        // Message list
        let mut msg_col = column![].spacing(10).padding([12, 16]);

        if !chat_enabled && admin_messages.is_empty() {
            msg_col = msg_col.push(
                container(
                    column![
                        icon::lock_icon().size(40.0).style(theme::text::secondary),
                        p1_regular("Waiting for admin assignment").style(theme::text::secondary),
                        p2_regular("An admin will be assigned to review your dispute")
                            .style(theme::text::secondary),
                    ]
                    .spacing(8)
                    .align_x(iced::alignment::Horizontal::Center),
                )
                .padding(60)
                .width(Length::Fill)
                .center_x(Length::Fill),
            );
        } else if admin_messages.is_empty() {
            msg_col = msg_col.push(
                container(
                    column![
                        icon::chat_icon().size(40.0).style(theme::text::secondary),
                        p1_regular("Admin assigned").style(theme::text::secondary),
                        p2_regular("Send a message to describe your issue")
                            .style(theme::text::secondary),
                    ]
                    .spacing(8)
                    .align_x(iced::alignment::Horizontal::Center),
                )
                .padding(60)
                .width(Length::Fill)
                .center_x(Length::Fill),
            );
        } else {
            for msg in &admin_messages {
                let text = extract_chat_text(&msg.payload_json);
                let ts = chrono::DateTime::from_timestamp(msg.timestamp as i64, 0)
                    .unwrap_or_default()
                    .with_timezone(&chrono::Local);
                let time_str = ts.format("%H:%M").to_string();

                if msg.is_own {
                    msg_col = msg_col.push(
                        column![
                            container(caption("You").style(theme::text::secondary))
                                .width(Length::Fill)
                                .align_right(Length::Fill),
                            row![
                                Space::new().width(Length::FillPortion(3)),
                                container(p1_regular(text))
                                    .padding([10, 16])
                                    .style(chat_bubble_own as fn(&_) -> _)
                                    .max_width(480),
                            ],
                            container(caption(time_str).style(theme::text::secondary))
                                .width(Length::Fill)
                                .align_right(Length::Fill),
                        ]
                        .spacing(2),
                    );
                } else {
                    msg_col = msg_col.push(
                        column![
                            caption("Admin").style(theme::text::primary),
                            row![
                                container(p1_regular(text))
                                    .padding([10, 16])
                                    .style(chat_bubble_peer as fn(&_) -> _)
                                    .max_width(480),
                                Space::new().width(Length::FillPortion(3)),
                            ],
                            caption(time_str).style(theme::text::secondary),
                        ]
                        .spacing(2),
                    );
                }
            }
        }

        let chat_scroll = iced::widget::scrollable(msg_col)
            .height(Length::Fill)
            .anchor_bottom();

        // Input area
        let input_area: Element<'_, view::Message> = if chat_enabled {
            let can_send = !self.dispute_chat_input.value.trim().is_empty()
                && self.pending_dispute_chat_message.is_none();
            let send_btn = if can_send {
                button::primary(Some(icon::send_icon()), "Send")
                    .on_press(p2p(P2PMessage::SendDisputeChatMessage))
            } else {
                button::primary(Some(icon::send_icon()), "Send")
            };
            container(
                row![
                    form::Form::new("Type a message...", &self.dispute_chat_input, |v| {
                        view::Message::P2P(P2PMessage::DisputeChatInputEdited(v))
                    })
                    .padding(10),
                    send_btn,
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center),
            )
            .padding(12)
            .width(Length::Fill)
            .into()
        } else {
            container(
                p2_regular("Chat will be available when an admin is assigned")
                    .style(theme::text::secondary),
            )
            .padding(12)
            .width(Length::Fill)
            .center_x(Length::Fill)
            .into()
        };

        container(
            column![chat_scroll, input_area,]
                .spacing(0)
                .width(Length::Fill),
        )
        .width(Length::Fixed(450.0))
        .into()
    }

    fn mostro_settings_view<'a>(&'a self) -> Element<'a, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);

        // ── Nodes card ──
        let node_rows: Vec<Element<'a, view::Message>> = self
            .mostro_config
            .nodes
            .iter()
            .map(|node| {
                let is_active = node.pubkey_hex == self.mostro_config.active_node_pubkey;
                let truncated_pubkey = if node.pubkey_hex.len() > 16 {
                    format!(
                        "{}...{}",
                        &node.pubkey_hex[..8],
                        &node.pubkey_hex[node.pubkey_hex.len() - 8..]
                    )
                } else {
                    node.pubkey_hex.clone()
                };

                let select_btn = if is_active {
                    button::primary(None, "Active").width(Length::Shrink)
                } else {
                    button::secondary(None, "Select")
                        .on_press(p2p(P2PMessage::MostroSelectActiveNode(
                            node.pubkey_hex.clone(),
                        )))
                        .width(Length::Shrink)
                };

                let remove_btn = button::secondary_compact(Some(icon::trash_icon()), "")
                    .on_press(p2p(P2PMessage::MostroRemoveNode(node.pubkey_hex.clone())));

                row![
                    column![
                        p2_bold(node.name.as_str()),
                        p2_regular(truncated_pubkey).style(theme::text::secondary),
                    ]
                    .spacing(4)
                    .width(Length::Fill),
                    select_btn,
                    remove_btn,
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .into()
            })
            .collect();

        let nodes_card = card::simple(
            column![
                p1_bold("Mostro Nodes"),
                p2_regular("Select which Mostro node to connect to").style(theme::text::secondary),
            ]
            .spacing(4)
            .push(column(node_rows).spacing(12))
            .push(
                column![
                    p2_regular("Add a new node").style(theme::text::secondary),
                    form::Form::new("Node name", &self.new_node_name_input, |v| {
                        view::Message::P2P(P2PMessage::MostroNodeNameInputEdited(v))
                    })
                    .padding(10),
                    form::Form::new_trimmed(
                        "Node pubkey (hex)",
                        &self.new_node_pubkey_input,
                        |v| { view::Message::P2P(P2PMessage::MostroNodePubkeyInputEdited(v)) }
                    )
                    .padding(10),
                    if self.new_node_name_input.value.trim().is_empty()
                        || self.new_node_pubkey_input.value.trim().is_empty()
                    {
                        button::secondary(Some(icon::plus_icon()), "Add Node")
                    } else {
                        button::secondary(Some(icon::plus_icon()), "Add Node")
                            .on_press(p2p(P2PMessage::MostroAddNode))
                    },
                ]
                .spacing(8),
            )
            .spacing(16),
        )
        .width(Length::Fill);

        // ── Relays card ──
        let relay_rows: Vec<Element<'a, view::Message>> = self
            .mostro_config
            .relays
            .iter()
            .map(|relay| {
                let remove_btn = button::secondary_compact(Some(icon::trash_icon()), "")
                    .on_press(p2p(P2PMessage::MostroRemoveRelay(relay.clone())));

                row![
                    icon::globe_icon().style(theme::text::secondary),
                    p2_regular(relay.as_str()).width(Length::Fill),
                    remove_btn,
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .into()
            })
            .collect();

        let relays_card = card::simple(
            column![
                p1_bold("Relays"),
                p2_regular("Relays used to connect to Mostro nodes").style(theme::text::secondary),
            ]
            .spacing(4)
            .push(column(relay_rows).spacing(12))
            .push(
                row![
                    form::Form::new_trimmed(
                        "wss://relay.example.com",
                        &self.new_relay_input,
                        |v| { view::Message::P2P(P2PMessage::MostroRelayInputEdited(v)) }
                    )
                    .padding(10),
                    if self.new_relay_input.value.trim().is_empty() {
                        button::secondary(Some(icon::plus_icon()), "Add")
                    } else {
                        button::secondary(Some(icon::plus_icon()), "Add")
                            .on_press(p2p(P2PMessage::MostroAddRelay))
                    },
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center),
            )
            .spacing(16),
        )
        .width(Length::Fill);

        let mut settings_col = column![nodes_card, relays_card]
            .spacing(16)
            .width(Length::Fill);
        if let Some(err) = self.mostro_config_error {
            settings_col = settings_col.push(p2_regular(err).style(theme::text::error));
        }
        settings_col.into()
    }

    /// Build the chat list view showing all trades with active conversations.
    fn chat_list_view(&self) -> Element<'_, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);
        let is_disputes = self.chat_list_tab == ChatListTab::Disputes;

        // Tab buttons (same style as buy/sell tabs)
        let messages_tab = iced::widget::button(
            container(p1_bold("Messages"))
                .padding([12, 0])
                .align_x(iced::alignment::Horizontal::Center)
                .width(Length::Fill),
        )
        .style(if !is_disputes {
            theme::button::primary as fn(&_, _) -> _
        } else {
            theme::button::transparent as fn(&_, _) -> _
        })
        .on_press(p2p(P2PMessage::ChatListTabMessages))
        .width(Length::Fill);

        let disputes_tab = iced::widget::button(
            container(p1_bold("Disputes"))
                .padding([12, 0])
                .align_x(iced::alignment::Horizontal::Center)
                .width(Length::Fill),
        )
        .style(if is_disputes {
            theme::button::primary as fn(&_, _) -> _
        } else {
            theme::button::transparent as fn(&_, _) -> _
        })
        .on_press(p2p(P2PMessage::ChatListTabDisputes))
        .width(Length::Fill);

        let tabs = container(
            row![messages_tab, disputes_tab]
                .spacing(4)
                .width(Length::Fill),
        )
        .padding(4)
        .width(Length::Fill)
        .style(theme::container::foreground_rounded);

        // Filter action based on active tab
        let action_filter: &[&str] = if is_disputes {
            &["AdminDm"]
        } else {
            &["SendDm"]
        };

        // Collect trades that have matching chat messages
        let cube_name = self.cube_name();
        let mut chat_entries: Vec<(&P2PTrade, String, u64)> = Vec::new();
        for trade in &self.trades {
            let messages = super::mostro::get_trade_messages(&cube_name, &trade.id);
            let chat_msgs: Vec<_> = messages
                .iter()
                .filter(|m| action_filter.contains(&m.action.as_str()))
                .collect();
            if !chat_msgs.is_empty() {
                let last_msg = chat_msgs.last().unwrap();
                let preview = if let Some(meta) =
                    super::mostro::parse_attachment_metadata(&last_msg.payload_json)
                {
                    match meta {
                        super::mostro::AttachmentMeta::Image(_) => "Image".to_string(),
                        super::mostro::AttachmentMeta::File(f) => f.filename.clone(),
                    }
                } else {
                    let text = extract_chat_text(&last_msg.payload_json);
                    if text.chars().count() > 50 {
                        let truncated: String = text.chars().take(50).collect();
                        format!("{truncated}...")
                    } else {
                        text
                    }
                };
                let prefix = if last_msg.is_own {
                    format!("You: {preview}")
                } else {
                    preview
                };
                chat_entries.push((trade, prefix, last_msg.timestamp));
            }
        }

        // Sort by last message timestamp, newest first
        chat_entries.sort_by(|a, b| b.2.cmp(&a.2));

        let list_content: Element<'_, view::Message> = if chat_entries.is_empty() {
            let (empty_label, empty_hint) = if is_disputes {
                (
                    "No dispute conversations",
                    "Dispute chats will appear here if a trade is disputed",
                )
            } else {
                (
                    "No conversations yet",
                    "Chat messages will appear here when you trade",
                )
            };
            let empty_icon = if is_disputes {
                icon::warning_icon().size(48).style(theme::text::secondary)
            } else {
                icon::chat_icon().size(48).style(theme::text::secondary)
            };
            container(
                column![
                    empty_icon,
                    p1_bold(empty_label),
                    p2_regular(empty_hint).style(theme::text::secondary),
                ]
                .spacing(12)
                .align_x(iced::alignment::Horizontal::Center),
            )
            .padding(60)
            .width(Length::Fill)
            .center_x(Length::Fill)
            .into()
        } else {
            // Pre-build owned data for each entry to avoid lifetime issues
            let entries: Vec<_> = chat_entries
                .iter()
                .map(|(trade, preview, last_ts)| {
                    let nick = if is_disputes {
                        "Admin / Solver".to_string()
                    } else {
                        trade
                            .counterparty_pubkey
                            .as_deref()
                            .map(super::mostro::nickname_from_pubkey)
                            .unwrap_or_else(|| "Peer".to_string())
                    };
                    let order_short = trade.id[..8.min(trade.id.len())].to_string();
                    let action_label = match trade.order_type {
                        OrderType::Buy => format!("You are buying from {}", nick),
                        OrderType::Sell => format!("You are selling to {}", nick),
                    };
                    let time_str = {
                        let dt = chrono::DateTime::from_timestamp(*last_ts as i64, 0)
                            .unwrap_or_default()
                            .with_timezone(&chrono::Local);
                        let today = chrono::Local::now().date_naive();
                        if dt.date_naive() == today {
                            dt.format("%H:%M").to_string()
                        } else {
                            dt.format("%b %d, %H:%M").to_string()
                        }
                    };
                    let subtitle = format!("{} · {}", action_label, order_short);
                    (trade.id.clone(), nick, subtitle, preview.clone(), time_str)
                })
                .collect();

            let mut list_col = column![].spacing(8).width(Length::Fill);
            for (trade_id, nick, subtitle, preview, time_str) in entries {
                let entry_content = card::simple(
                    row![
                        column![
                            p1_bold(nick),
                            p2_regular(subtitle).style(theme::text::secondary),
                            p2_regular(preview).style(theme::text::secondary),
                        ]
                        .spacing(4)
                        .width(Length::Fill),
                        caption(time_str).style(theme::text::secondary),
                    ]
                    .spacing(12)
                    .align_y(iced::alignment::Vertical::Center),
                )
                .width(Length::Fill);

                let on_press = if is_disputes {
                    p2p(P2PMessage::OpenDisputeChatForTrade(trade_id))
                } else {
                    p2p(P2PMessage::OpenChatForTrade(trade_id))
                };

                let entry = iced::widget::button(entry_content)
                    .on_press(on_press)
                    .width(Length::Fill)
                    .style(theme::button::transparent);

                list_col = list_col.push(entry);
            }
            list_col.into()
        };

        column![tabs, list_content].spacing(16).into()
    }
}

impl State for P2PPanel {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Menu::Marketplace(MarketplaceSubMenu::P2P(submenu)) = menu {
            match submenu {
                P2PSubMenu::Overview => {
                    // If an order is selected, show its detail view
                    // Built separately (not inside dashboard's scrollable) to avoid
                    // first-click issues when switching from the order list.
                    if let Some(ref selected_id) = self.selected_order {
                        if let Some(order) = self.orders.iter().find(|o| o.id == *selected_id) {
                            let take_state = if self.taking_order {
                                Some(TakeOrderState {
                                    amount: &self.take_order_amount,
                                    invoice: &self.take_order_invoice,
                                    submitting: self.take_order_submitting,
                                })
                            } else {
                                None
                            };
                            let has_vault = cache.has_vault;
                            let detail_content = iced::widget::scrollable(row![
                                Space::new().width(Length::FillPortion(1)),
                                column![
                                    Space::new().height(Length::Fixed(30.0)),
                                    column![
                                        h3("Order Details").bold(),
                                        p2_regular("View order information")
                                            .style(theme::text::secondary),
                                    ]
                                    .spacing(8)
                                    .width(Length::Fill),
                                    container(order_detail(order, take_state)).width(Length::Fill),
                                    Space::new().height(Length::Fixed(40.0)),
                                ]
                                .spacing(16)
                                .width(Length::FillPortion(8))
                                .max_width(1500),
                                Space::new().width(Length::FillPortion(1)),
                            ]);

                            return row![]
                                .push(view::nav::sidebar(
                                    menu,
                                    &view::nav::NavContext {
                                        has_vault,
                                        has_p2p: cache.has_p2p,
                                        cube_name: &cache.cube_name,
                                        lightning_address: None,
                                        avatar: None,
                                        theme_mode: cache.theme_mode,
                                        connect_authenticated: cache.connect_authenticated,
                                    },
                                ))
                                .push(
                                    iced::widget::Column::new()
                                        .push(view::warn(None))
                                        .push(
                                            container(detail_content)
                                                .center_x(Length::Fill)
                                                .style(theme::container::background)
                                                .height(Length::Fill),
                                        )
                                        .width(Length::Fill),
                                )
                                .width(Length::Fill)
                                .height(Length::Fill)
                                .into();
                        }
                    }

                    let filtered_orders = self.filtered_orders();

                    // Count offers per tab (applying all filters except buy/sell)
                    let buy_count = self
                        .orders
                        .iter()
                        .filter(|o| o.order_type == OrderType::Sell && self.order_passes_filters(o))
                        .count();
                    let sell_count = self
                        .orders
                        .iter()
                        .filter(|o| o.order_type == OrderType::Buy && self.order_passes_filters(o))
                        .count();

                    let filter_state = OrderFilterState {
                        buy_sell: &self.buy_sell_filter,
                        buy_count,
                        sell_count,
                        filter_currency: &self.filter_currency,
                        currency_combo_state: &self.filter_currency_combo_state,
                        available_payment_methods: &self.filter_available_payment_methods,
                        deselected_payment_methods: &self.filter_deselected_payment_methods,
                        min_rating: self.filter_min_rating,
                        min_days_active: self.filter_min_days_active,
                        filtered_count: filtered_orders.len(),
                    };

                    // --- Header ---
                    let mut overview_col = column![column![
                        h3("P2P Order Book").bold(),
                        p2_regular("Browse and take P2P trading orders from the Mostro network")
                            .style(theme::text::secondary),
                    ]
                    .spacing(8)
                    .width(Length::Fill),]
                    .spacing(16);

                    // Stream error banner
                    if let Some(ref err) = self.stream_error {
                        overview_col = overview_col.push(
                            container(p2_regular(err.as_str()).style(theme::text::warning))
                                .width(Length::Fill),
                        );
                    }

                    // --- Sidebar + order list side by side ---
                    let sidebar = order_filter_sidebar(filter_state);

                    let order_list: Element<'_, view::Message> = if filtered_orders.is_empty() {
                        container(p1_bold("No orders found"))
                            .padding(40)
                            .align_x(Alignment::Center)
                            .width(Length::Fill)
                            .into()
                    } else {
                        column(filtered_orders.iter().map(|order| order_card(order).into()))
                            .spacing(16)
                            .width(Length::Fill)
                            .into()
                    };

                    let content_row = row![sidebar, container(order_list).width(Length::Fill),]
                        .spacing(20)
                        .align_y(iced::alignment::Vertical::Top)
                        .width(Length::Fill);

                    overview_col = overview_col.push(content_row);
                    overview_col = overview_col.push(Space::new().height(Length::Fixed(40.0)));

                    let overview_content: Element<'_, view::Message> =
                        view::dashboard(menu, cache, overview_col);

                    // Show payment invoice modal if seller took a buy order
                    if let Some((ref oid, ref inv, amt, ref qr_data)) = self.pending_payment_invoice
                    {
                        coincube_ui::widget::modal::Modal::new(
                            overview_content,
                            self.payment_invoice_modal_view(oid, inv, amt, qr_data),
                        )
                        .on_blur(Some(view::Message::P2P(P2PMessage::DismissPaymentInvoice)))
                        .into()
                    } else {
                        overview_content
                    }
                }
                P2PSubMenu::MyTrades => {
                    // If a trade is selected, show its detail or chat view
                    if let Some(ref selected_id) = self.selected_trade {
                        if let Some(trade) = self.trades.iter().find(|t| t.id == *selected_id) {
                            if self.active_chat == ActiveChat::Peer {
                                // Separate chat view — built without dashboard()
                                // to avoid nested scrollables.
                                let has_vault = cache.has_vault;
                                let order_short = &trade.id[..8.min(trade.id.len())];
                                let trade_type_label = match trade.order_type {
                                    OrderType::Buy => "BUY",
                                    OrderType::Sell => "SELL",
                                };
                                let peer_nick = trade
                                    .counterparty_pubkey
                                    .as_deref()
                                    .map(super::mostro::nickname_from_pubkey)
                                    .unwrap_or_else(|| "Peer".to_string());

                                // Header bar
                                let header = container(
                                    row![
                                        button::secondary(Some(icon::previous_icon()), "Back",)
                                            .on_press(view::Message::P2P(P2PMessage::CloseChat)),
                                        Space::new().width(Length::Fill),
                                        column![
                                            p1_bold(peer_nick),
                                            p2_regular(format!(
                                                "{} Order {}",
                                                trade_type_label, order_short
                                            ))
                                            .style(theme::text::secondary),
                                        ]
                                        .spacing(2)
                                        .align_x(iced::alignment::Horizontal::Center),
                                        Space::new().width(Length::Fill),
                                        // Spacer to balance the Back button
                                        Space::new().width(Length::Fixed(80.0)),
                                    ]
                                    .align_y(iced::alignment::Vertical::Center),
                                )
                                .padding([16, 20])
                                .width(Length::Fill)
                                .style(theme::container::foreground);

                                return row![]
                                    .push(view::nav::sidebar(
                                        menu,
                                        &view::nav::NavContext {
                                            has_vault,
                                            has_p2p: cache.has_p2p,
                                            cube_name: &cache.cube_name,
                                            lightning_address: None,
                                            avatar: None,
                                            theme_mode: cache.theme_mode,
                                            connect_authenticated: cache.connect_authenticated,
                                        },
                                    ))
                                    .push(
                                        iced::widget::Column::new()
                                            .push(view::warn(None))
                                            .push(header)
                                            .push(
                                                container(self.chat_view(trade))
                                                    .padding(20)
                                                    .width(Length::Fill)
                                                    .height(Length::Fill)
                                                    .style(theme::container::background),
                                            )
                                            .width(Length::Fill),
                                    )
                                    .width(Length::Fill)
                                    .height(Length::Fill)
                                    .into();
                            } else if self.active_chat == ActiveChat::Dispute {
                                // Dispute chat view
                                let has_vault = cache.has_vault;
                                let order_short = &trade.id[..8.min(trade.id.len())];
                                let header = container(
                                    row![
                                        button::secondary(Some(icon::previous_icon()), "Back",)
                                            .on_press(view::Message::P2P(
                                                P2PMessage::CloseDisputeChat,
                                            )),
                                        Space::new().width(Length::Fill),
                                        column![
                                            p1_bold("Dispute Chat"),
                                            p2_regular(format!("Order {}", order_short))
                                                .style(theme::text::secondary),
                                        ]
                                        .spacing(2)
                                        .align_x(iced::alignment::Horizontal::Center),
                                        Space::new().width(Length::Fill),
                                        Space::new().width(Length::Fixed(80.0)),
                                    ]
                                    .align_y(iced::alignment::Vertical::Center),
                                )
                                .padding(12)
                                .width(Length::Fill)
                                .style(theme::container::foreground);

                                return row![]
                                    .push(view::nav::sidebar(
                                        menu,
                                        &view::nav::NavContext {
                                            has_vault,
                                            has_p2p: cache.has_p2p,
                                            cube_name: &cache.cube_name,
                                            lightning_address: None,
                                            avatar: None,
                                            theme_mode: cache.theme_mode,
                                            connect_authenticated: cache.connect_authenticated,
                                        },
                                    ))
                                    .push(
                                        iced::widget::Column::new()
                                            .push(view::warn(None))
                                            .push(header)
                                            .push(
                                                container(self.dispute_chat_view(trade))
                                                    .padding(20)
                                                    .width(Length::Fill)
                                                    .height(Length::Fill)
                                                    .style(theme::container::background),
                                            )
                                            .width(Length::Fill),
                                    )
                                    .width(Length::Fill)
                                    .height(Length::Fill)
                                    .into();
                            }
                            return view::dashboard(
                                menu,
                                cache,
                                column![
                                    column![
                                        h3("Trade Details").bold(),
                                        p2_regular("View trade information and take actions")
                                            .style(theme::text::secondary),
                                    ]
                                    .spacing(8)
                                    .width(Length::Fill),
                                    container(self.trade_detail_view(trade)).width(Length::Fill),
                                    Space::new().height(Length::Fixed(40.0)),
                                ]
                                .spacing(16),
                            );
                        }
                    }

                    let filtered = self.filtered_trades();
                    let shown_count = filtered.len();

                    view::dashboard(
                        menu,
                        cache,
                        column![
                            // Title
                            column![
                                h3("My Trades").bold(),
                                p2_regular("Your active and completed P2P trades")
                                    .style(theme::text::secondary),
                            ]
                            .spacing(8)
                            .width(Length::Fill),
                            // Trade status filter
                            container(trade_status_filter(&self.trade_filters, shown_count,))
                                .width(Length::Fill),
                            // Trade list
                            if filtered.is_empty() {
                                Element::from(
                                    container(p1_bold("No trades yet"))
                                        .padding(40)
                                        .align_x(Alignment::Center)
                                        .width(Length::Fill),
                                )
                            } else {
                                Element::from(
                                    column(filtered.iter().map(|trade| trade_card(trade).into()))
                                        .spacing(16)
                                        .width(Length::Fill),
                                )
                            },
                            Space::new().height(Length::Fixed(40.0)),
                        ]
                        .spacing(16),
                    )
                }
                P2PSubMenu::Chat => {
                    // If a trade is selected from the chat list, show the chat view
                    if let Some(ref selected_id) = self.chat_selected_trade {
                        if let Some(trade) = self.trades.iter().find(|t| t.id == *selected_id) {
                            if self.active_chat == ActiveChat::Peer {
                                let has_vault = cache.has_vault;
                                let order_short = &trade.id[..8.min(trade.id.len())];
                                let trade_type_label = match trade.order_type {
                                    OrderType::Buy => "BUY",
                                    OrderType::Sell => "SELL",
                                };
                                let peer_nick = trade
                                    .counterparty_pubkey
                                    .as_deref()
                                    .map(super::mostro::nickname_from_pubkey)
                                    .unwrap_or_else(|| "Peer".to_string());

                                let header = container(
                                    row![
                                        button::secondary(Some(icon::previous_icon()), "Back")
                                            .on_press(view::Message::P2P(P2PMessage::CloseChat)),
                                        Space::new().width(Length::Fill),
                                        column![
                                            p1_bold(peer_nick),
                                            p2_regular(format!(
                                                "{} Order {}",
                                                trade_type_label, order_short
                                            ))
                                            .style(theme::text::secondary),
                                        ]
                                        .spacing(2)
                                        .align_x(iced::alignment::Horizontal::Center),
                                        Space::new().width(Length::Fill),
                                        Space::new().width(Length::Fixed(80.0)),
                                    ]
                                    .align_y(iced::alignment::Vertical::Center),
                                )
                                .padding([16, 20])
                                .width(Length::Fill)
                                .style(theme::container::foreground);

                                return row![]
                                    .push(view::nav::sidebar(
                                        menu,
                                        &view::nav::NavContext {
                                            has_vault,
                                            has_p2p: cache.has_p2p,
                                            cube_name: &cache.cube_name,
                                            lightning_address: None,
                                            avatar: None,
                                            theme_mode: cache.theme_mode,
                                            connect_authenticated: cache.connect_authenticated,
                                        },
                                    ))
                                    .push(
                                        iced::widget::Column::new()
                                            .push(view::warn(None))
                                            .push(header)
                                            .push(
                                                container(self.chat_view(trade))
                                                    .padding(20)
                                                    .width(Length::Fill)
                                                    .height(Length::Fill)
                                                    .style(theme::container::background),
                                            )
                                            .width(Length::Fill),
                                    )
                                    .width(Length::Fill)
                                    .height(Length::Fill)
                                    .into();
                            } else if self.active_chat == ActiveChat::Dispute {
                                let has_vault = cache.has_vault;
                                let order_short = &trade.id[..8.min(trade.id.len())];
                                let header = container(
                                    row![
                                        button::secondary(Some(icon::previous_icon()), "Back")
                                            .on_press(view::Message::P2P(P2PMessage::CloseChat)),
                                        Space::new().width(Length::Fill),
                                        column![
                                            p1_bold("Dispute Chat"),
                                            p2_regular(format!("Order {}", order_short))
                                                .style(theme::text::secondary),
                                        ]
                                        .spacing(2)
                                        .align_x(iced::alignment::Horizontal::Center),
                                        Space::new().width(Length::Fill),
                                        Space::new().width(Length::Fixed(80.0)),
                                    ]
                                    .align_y(iced::alignment::Vertical::Center),
                                )
                                .padding([16, 20])
                                .width(Length::Fill)
                                .style(theme::container::foreground);

                                return row![]
                                    .push(view::nav::sidebar(
                                        menu,
                                        &view::nav::NavContext {
                                            has_vault,
                                            has_p2p: cache.has_p2p,
                                            cube_name: &cache.cube_name,
                                            lightning_address: None,
                                            avatar: None,
                                            theme_mode: cache.theme_mode,
                                            connect_authenticated: cache.connect_authenticated,
                                        },
                                    ))
                                    .push(
                                        iced::widget::Column::new()
                                            .push(view::warn(None))
                                            .push(header)
                                            .push(
                                                container(self.dispute_chat_view(trade))
                                                    .padding(20)
                                                    .width(Length::Fill)
                                                    .height(Length::Fill)
                                                    .style(theme::container::background),
                                            )
                                            .width(Length::Fill),
                                    )
                                    .width(Length::Fill)
                                    .height(Length::Fill)
                                    .into();
                            }
                        }
                    }

                    // Default: show chat list
                    view::dashboard(
                        menu,
                        cache,
                        column![
                            column![
                                h3("Chat").bold(),
                                p2_regular("Your P2P trade conversations")
                                    .style(theme::text::secondary),
                            ]
                            .spacing(8)
                            .width(Length::Fill),
                            container(self.chat_list_view()).width(Length::Fill),
                            Space::new().height(Length::Fixed(40.0)),
                        ]
                        .spacing(16),
                    )
                }
                P2PSubMenu::CreateOrder => {
                    let content: Element<'_, view::Message> = view::dashboard(
                        menu,
                        cache,
                        column![
                            column![
                                h3("Create P2P Order").bold(),
                                p2_regular(
                                    "Create a new buy or sell order for the P2P marketplace"
                                )
                                .style(theme::text::secondary),
                            ]
                            .spacing(8)
                            .width(Length::Fill),
                            container(self.create_order_view(cache)).width(Length::Fill),
                            Space::new().height(Length::Fixed(40.0)),
                        ]
                        .spacing(16),
                    );

                    if self.confirming_order {
                        coincube_ui::widget::modal::Modal::new(
                            content,
                            self.order_confirmation_view(),
                        )
                        .on_blur(Some(view::Message::P2P(P2PMessage::CancelConfirmation)))
                        .into()
                    } else {
                        content
                    }
                }
                P2PSubMenu::Settings => view::dashboard(
                    menu,
                    cache,
                    column![
                        column![
                            h3("P2P Settings").bold(),
                            p2_regular("Configure Mostro nodes and relays")
                                .style(theme::text::secondary),
                        ]
                        .spacing(8)
                        .width(Length::Fill),
                        container(self.mostro_settings_view()).width(Length::Fill),
                        Space::new().height(Length::Fixed(40.0)),
                    ]
                    .spacing(16),
                ),
            }
        } else {
            view::dashboard(
                menu,
                cache,
                view::placeholder(
                    icon::person_icon().size(100),
                    "P2P Trading",
                    "Mostro P2P trading integration",
                ),
            )
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let cube_name = self.cube_name();
        let mnemonic = self.mnemonic.clone();
        let active_pubkey = self.mostro_config.active_pubkey_hex().to_string();
        let relays = self.mostro_config.relays.clone();
        let mostro_sub =
            super::mostro::mostro_subscription(cube_name, mnemonic, active_pubkey, relays);

        // Tick every second when viewing a trade detail (for action countdown timer)
        let selected_is_active = self.selected_trade.as_ref().is_some_and(|id| {
            self.trades
                .iter()
                .any(|t| t.id == *id && !t.status.is_terminal())
        });
        if selected_is_active {
            let timer = iced::time::every(std::time::Duration::from_secs(1))
                .map(|_| Message::View(view::Message::P2P(P2PMessage::TradeTimerTick)));
            Subscription::batch([mostro_sub, timer])
        } else {
            mostro_sub
        }
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let msg = match message {
            Message::View(view::Message::P2P(msg)) => msg,
            _ => return Task::none(),
        };
        if !self.lightning_address_user_edited && self.create_lightning_address.value.is_empty() {
            if let Some(addr) = cache.lightning_address.as_ref() {
                self.create_lightning_address.value = addr.clone();
            }
        }
        match msg {
            P2PMessage::OrderTypeSelected(t) => self.create_order_type = t,
            P2PMessage::PricingModeSelected(m) => self.create_pricing_mode = m,
            P2PMessage::FiatCurrencyEdited(v) => {
                self.create_fiat_currency = v;
                self.create_payment_methods.clear();
                self.rebuild_payment_method_combo();
            }
            P2PMessage::SatsAmountEdited(v) => self.create_sats_amount.value = v,
            P2PMessage::PremiumEdited(v) => {
                // Allow empty, minus sign, or valid integers in -100..100 range
                let trimmed = v.trim();
                if trimmed.is_empty() || trimmed == "-" || trimmed.parse::<i64>().is_ok() {
                    self.create_premium.value = v;
                }
            }
            P2PMessage::PaymentMethodSelected(v) => {
                if !self.create_payment_methods.contains(&v) {
                    self.create_payment_methods.push(v);
                    self.rebuild_payment_method_combo();
                }
            }
            P2PMessage::PaymentMethodRemoved(v) => {
                self.create_payment_methods.retain(|m| m != &v);
                self.rebuild_payment_method_combo();
            }
            P2PMessage::CustomPaymentMethodEdited(v) => {
                self.create_custom_payment_method.value = v;
            }
            P2PMessage::AddCustomPaymentMethod => {
                let custom = self.create_custom_payment_method.value.trim().to_string();
                if !custom.is_empty() && !self.create_payment_methods.contains(&custom) {
                    self.create_payment_methods.push(custom);
                    self.create_custom_payment_method = Default::default();
                    self.rebuild_payment_method_combo();
                }
            }
            P2PMessage::MinAmountEdited(v) => {
                self.create_min_amount.value = v;
            }
            P2PMessage::MaxAmountEdited(v) => {
                self.create_max_amount.value = v;
            }
            P2PMessage::RangeOrderToggled(on) => {
                self.range_order_mode = on;
                if on {
                    // Range orders only support market pricing.
                    self.create_pricing_mode = PricingMode::Market;
                } else {
                    self.create_max_amount = Default::default();
                }
            }
            P2PMessage::LightningAddressEdited(v) => {
                self.create_lightning_address.value = v;
                self.lightning_address_user_edited = true;
            }
            P2PMessage::EditLightningAddress => {
                self.editing_lightning_address = true;
            }
            P2PMessage::UseRegisteredLightningAddress => {
                self.editing_lightning_address = false;
                self.lightning_address_user_edited = false;
                self.create_lightning_address = Default::default();
                if let Some(addr) = cache.lightning_address.as_ref() {
                    self.create_lightning_address.value = addr.clone();
                }
            }
            P2PMessage::SubmitOrder => {
                self.submit_attempted = true;
                // Double-check validation before showing confirmation
                if self.validate_order_form().has_errors() {
                    return Task::none();
                }
                self.order_submit_error = None;
                self.confirming_order = true;
            }
            P2PMessage::CancelConfirmation => {
                self.confirming_order = false;
            }
            P2PMessage::ConfirmOrder => {
                // Final validation gate before sending to network
                if self.validate_order_form().has_errors() {
                    self.confirming_order = false;
                    return Task::none();
                }
                self.confirming_order = false;
                self.order_submitting = true;
                self.order_submit_error = None;

                let form = self.build_order_form();

                return Task::perform(super::mostro::submit_order(form), |result| {
                    Message::View(view::Message::P2P(P2PMessage::OrderSubmitResult(result)))
                });
            }
            P2PMessage::ClearForm => self.clear_create_form(),
            P2PMessage::MostroNodeInfoReceived {
                currencies,
                min_order_sats,
                max_order_sats,
            } => {
                if !currencies.is_empty() {
                    self.node_currencies = currencies;
                    self.rebuild_currency_combo();
                    // Reset selected currency if no longer supported by this node
                    if !self.node_currencies.contains(&self.create_fiat_currency) {
                        self.create_fiat_currency = self
                            .node_currencies
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "USD".to_string());
                        self.create_payment_methods.clear();
                        self.rebuild_payment_method_combo();
                    }
                    // Reset filter currency if no longer supported (unless "All")
                    if self.filter_currency != "All"
                        && !self.node_currencies.contains(&self.filter_currency)
                    {
                        self.filter_currency = "All".to_string();
                        self.filter_deselected_payment_methods.clear();
                        self.recompute_available_payment_methods();
                    }
                }
                self.node_min_order_sats = min_order_sats;
                self.node_max_order_sats = max_order_sats;
            }
            P2PMessage::MostroOrdersReceived(mut orders) => {
                // Patch is_mine for orders we created locally (handles race where
                // the subscription delivers the event before the session is persisted)
                for order in &mut orders {
                    if self.my_created_order_ids.contains(&order.id) {
                        order.is_mine = true;
                    }
                }
                self.orders = orders;
                self.recompute_available_payment_methods();
                // Clear transient stream/relay errors — data is flowing successfully.
                // Preserve restore failure messages (they require user attention).
                if self
                    .stream_error
                    .as_ref()
                    .is_none_or(|e| !e.contains("restore"))
                {
                    self.stream_error = None;
                }
            }
            P2PMessage::MostroTradesReceived(trades) => {
                // Mostro can drop a session entirely (e.g. after the
                // hold-invoice timer expires it cancels the order and
                // eventually stops surfacing it). For trades we just
                // paid out of Spark, that would leave the user with a
                // Pending Spark HTLC and no trade record to explain
                // it. Carry forward any Spark-funded trade that the
                // new payload doesn't include — preserves whatever
                // status it last had (typically Canceled/Expired) so
                // the user can still find it in My Trades.
                let mut retained: Vec<P2PTrade> = self
                    .trades
                    .iter()
                    .filter(|t| {
                        self.spark_funded_order_ids.contains(&t.id)
                            && !trades.iter().any(|n| n.id == t.id)
                    })
                    .cloned()
                    .collect();
                self.trades = trades;
                self.trades.append(&mut retained);
                // Recompute QR cache if the selected trade now has a hold invoice
                if let Some(ref sel_id) = self.selected_trade {
                    if self.hold_invoice_qr.is_none() {
                        self.hold_invoice_qr = self
                            .trades
                            .iter()
                            .find(|t| t.id == *sel_id)
                            .and_then(|t| t.hold_invoice.as_ref())
                            .and_then(|inv| qr_code::Data::new(inv).ok());
                    }
                }
            }
            P2PMessage::BuySellFilterChanged(filter) => {
                self.buy_sell_filter = filter;
                self.filter_deselected_payment_methods.clear();
                self.recompute_available_payment_methods();
            }
            P2PMessage::FilterCurrencySelected(currency) => {
                self.filter_currency = currency;
                self.filter_deselected_payment_methods.clear();
                self.recompute_available_payment_methods();
            }
            P2PMessage::FilterPaymentMethodToggled(method) => {
                if !self.filter_deselected_payment_methods.remove(&method) {
                    self.filter_deselected_payment_methods.insert(method);
                }
            }
            P2PMessage::FilterMinRatingChanged(v) => {
                self.filter_min_rating = (v * 2.0).round() / 2.0;
            }
            P2PMessage::FilterMinDaysActiveChanged(v) => {
                self.filter_min_days_active = v;
            }
            P2PMessage::TradeFilterChanged(filter) => {
                if filter == TradeFilter::All {
                    // "All" is exclusive — toggle it alone
                    if self.trade_filters.contains(&TradeFilter::All) {
                        // Already showing all, do nothing
                    } else {
                        self.trade_filters = vec![TradeFilter::All];
                    }
                } else if self.trade_filters.contains(&filter) {
                    // Deselect this filter
                    self.trade_filters.retain(|f| f != &filter);
                    if self.trade_filters.is_empty() {
                        self.trade_filters = vec![TradeFilter::All];
                    }
                } else {
                    // Select this filter, remove "All" if present
                    self.trade_filters.retain(|f| f != &TradeFilter::All);
                    self.trade_filters.push(filter);
                }
            }
            P2PMessage::SelectOrder(id) => {
                self.selected_order = Some(id);
                self.taking_order = false;
                self.take_order_amount = Default::default();
                self.take_order_invoice = Default::default();
                self.take_order_submitting = false;
            }
            P2PMessage::CloseOrderDetail => {
                self.selected_order = None;
                self.taking_order = false;
            }
            P2PMessage::CopyOrderId(id) => {
                return Task::done(Message::View(view::Message::Clipboard(id)));
            }
            P2PMessage::CancelOrder(order_id) => {
                let data = super::mostro::TradeActionData {
                    order_id,
                    cube_name: self.cube_name(),
                    mnemonic: self.mnemonic.clone(),
                    invoice: None,
                    mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                    relay_urls: self.mostro_config.relays.clone(),
                };
                return Task::perform(super::mostro::cancel_trade(data), |result| {
                    Message::View(view::Message::P2P(P2PMessage::CancelOrderResult(
                        result.map(|_| ()),
                    )))
                });
            }
            P2PMessage::CancelOrderResult(result) => match result {
                Ok(()) => {
                    self.selected_order = None;
                    // Mirror the modal-dismissal reset used by
                    // `DismissPaymentInvoice` and `SparkPaySent` so a
                    // subsequent SelectTrade/TakeOrderResult doesn't
                    // inherit stale Spark-pay state.
                    self.pending_payment_invoice = None;
                    self.invoice_copied = false;
                    self.spark_balance_sat = None;
                    self.spark_pay_amount_sat = None;
                    self.spark_pay_attempt = None;
                    self.spark_pay_phase = SparkPayPhase::Idle;
                    self.show_qr_fallback = false;
                    self.spark_pay_session_id = None;
                    return Task::done(Message::View(view::Message::ShowSuccess(
                        "Order canceled".to_string(),
                    )));
                }
                Err(e) => {
                    return Task::done(Message::View(view::Message::ShowError(format!(
                        "Cancel failed: {}",
                        e
                    ))));
                }
            },
            P2PMessage::OrderSubmitResult(result) => {
                self.order_submitting = false;
                match result {
                    Ok(resp) => {
                        let super::mostro::OrderSubmitResponse::Success { order_id } = resp;
                        tracing::info!("Order created: {}", order_id);
                        // Track this order ID so we always mark it as ours, even if
                        // the subscription delivered the event before the session was saved.
                        self.my_created_order_ids.insert(order_id.clone());
                        if let Some(order) = self.orders.iter_mut().find(|o| o.id == order_id) {
                            order.is_mine = true;
                        }
                        self.clear_create_form();
                        return Task::done(Message::View(view::Message::ShowSuccess(format!(
                            "Order created successfully ({})",
                            order_id
                        ))));
                    }
                    Err(e) => {
                        self.order_submit_error = Some(e);
                    }
                }
            }
            // Mostro settings
            P2PMessage::MostroRelayInputEdited(v) => {
                self.new_relay_input.value = v;
                self.new_relay_input.warning = None;
                self.new_relay_input.valid = true;
            }
            P2PMessage::MostroAddRelay => {
                let url = self.new_relay_input.value.trim().to_string();
                let parsed = nostr_sdk::Url::parse(&url);
                let scheme_ok = url.starts_with("wss://") || url.starts_with("ws://");
                let has_host = parsed.as_ref().ok().and_then(|u| u.host()).is_some();
                if !scheme_ok {
                    self.new_relay_input.valid = false;
                    self.new_relay_input.warning = Some("URL must start with wss:// or ws://");
                } else if !has_host {
                    self.new_relay_input.valid = false;
                    self.new_relay_input.warning = Some("Invalid relay URL — missing host");
                } else if self.mostro_config.relays.contains(&url) {
                    self.new_relay_input.valid = false;
                    self.new_relay_input.warning = Some("Relay already exists");
                } else {
                    let mut trial = self.mostro_config.clone();
                    trial.relays.push(url);
                    match save_mostro_config(&trial) {
                        Ok(()) => {
                            self.mostro_config = trial;
                            self.new_relay_input = Default::default();
                            self.mostro_config_error = None;
                        }
                        Err(_) => {
                            self.new_relay_input.valid = false;
                            self.new_relay_input.warning = Some("Failed to save config");
                        }
                    }
                }
            }
            P2PMessage::MostroRemoveRelay(url) => {
                let mut trial = self.mostro_config.clone();
                trial.relays.retain(|r| r != &url);
                trial.ensure_defaults();
                match save_mostro_config(&trial) {
                    Ok(()) => {
                        self.mostro_config = trial;
                        self.mostro_config_error = None;
                    }
                    Err(_) => {
                        self.mostro_config_error = Some("Failed to save config");
                    }
                }
            }
            P2PMessage::MostroNodeNameInputEdited(v) => {
                self.new_node_name_input.value = v;
                self.new_node_name_input.warning = None;
                self.new_node_name_input.valid = true;
            }
            P2PMessage::MostroNodePubkeyInputEdited(v) => {
                self.new_node_pubkey_input.value = v;
                self.new_node_pubkey_input.warning = None;
                self.new_node_pubkey_input.valid = true;
            }
            P2PMessage::MostroAddNode => {
                let name = self.new_node_name_input.value.trim().to_string();
                let pubkey = self.new_node_pubkey_input.value.trim().to_string();
                if name.is_empty() {
                    self.new_node_name_input.valid = false;
                    self.new_node_name_input.warning = Some("Node name is required");
                } else if nostr_sdk::PublicKey::from_hex(&pubkey).is_err() {
                    self.new_node_pubkey_input.valid = false;
                    self.new_node_pubkey_input.warning = Some("Invalid hex pubkey");
                } else if self
                    .mostro_config
                    .nodes
                    .iter()
                    .any(|n| n.pubkey_hex == pubkey)
                {
                    self.new_node_pubkey_input.valid = false;
                    self.new_node_pubkey_input.warning =
                        Some("Node with this pubkey already exists");
                } else {
                    let mut trial = self.mostro_config.clone();
                    trial.nodes.push(MostroNode {
                        name,
                        pubkey_hex: pubkey,
                    });
                    match save_mostro_config(&trial) {
                        Ok(()) => {
                            self.mostro_config = trial;
                            self.new_node_name_input = Default::default();
                            self.new_node_pubkey_input = Default::default();
                            self.mostro_config_error = None;
                        }
                        Err(_) => {
                            self.new_node_pubkey_input.valid = false;
                            self.new_node_pubkey_input.warning = Some("Failed to save config");
                        }
                    }
                }
            }
            P2PMessage::MostroRemoveNode(pubkey) => {
                let mut trial = self.mostro_config.clone();
                trial.nodes.retain(|n| n.pubkey_hex != pubkey);
                trial.ensure_defaults();
                match save_mostro_config(&trial) {
                    Ok(()) => {
                        self.mostro_config = trial;
                        self.mostro_config_error = None;
                    }
                    Err(_) => {
                        self.mostro_config_error = Some("Failed to save config");
                    }
                }
            }
            P2PMessage::MostroSelectActiveNode(pubkey) => {
                let mut trial = self.mostro_config.clone();
                trial.active_node_pubkey = pubkey;
                match save_mostro_config(&trial) {
                    Ok(()) => {
                        self.mostro_config = trial;
                        self.mostro_config_error = None;
                    }
                    Err(_) => {
                        self.mostro_config_error = Some("Failed to save config");
                    }
                }
            }
            // Take order flow
            P2PMessage::TakeOrder => {
                // Check if this order needs user input before confirming.
                // Non-range orders where user is selling don't need any input,
                // so skip straight to confirmation to avoid an invisible intermediate state.
                let needs_input = self
                    .selected_order
                    .as_ref()
                    .and_then(|id| self.orders.iter().find(|o| o.id == *id))
                    .map(|order| {
                        let is_buying = order.order_type == OrderType::Sell;
                        order.is_range_order() || is_buying
                    })
                    .unwrap_or(true);

                if needs_input {
                    self.taking_order = true;
                    self.take_order_amount = Default::default();
                    self.take_order_invoice = Default::default();
                    self.take_order_submitting = false;
                } else {
                    // No input needed — proceed directly
                    self.taking_order = true;
                    return self.update(
                        _daemon,
                        cache,
                        Message::View(view::Message::P2P(P2PMessage::ConfirmTakeOrder)),
                    );
                }
            }
            P2PMessage::TakeOrderAmountEdited(v) => {
                self.take_order_amount.value = v;
            }
            P2PMessage::TakeOrderInvoiceEdited(v) => {
                self.take_order_invoice.value = v;
            }
            P2PMessage::CancelTakeOrder => {
                self.taking_order = false;
                self.take_order_amount = Default::default();
                self.take_order_invoice = Default::default();
                self.take_order_submitting = false;
            }
            P2PMessage::ConfirmTakeOrder => {
                if let Some(ref selected_id) = self.selected_order {
                    if let Some(order) = self.orders.iter().find(|o| o.id == *selected_id) {
                        let amount = if order.is_range_order() {
                            match self.take_order_amount.value.parse::<i64>() {
                                Ok(v) => {
                                    let min = order.min_amount.unwrap_or(0.0) as i64;
                                    let max = order.max_amount.unwrap_or(i64::MAX as f64) as i64;
                                    if v < min || v > max {
                                        return Task::none();
                                    }
                                    Some(v)
                                }
                                Err(_) => return Task::none(),
                            }
                        } else {
                            None
                        };

                        self.take_order_submitting = true;
                        let invoice = {
                            let v = self.take_order_invoice.value.trim().to_string();
                            if v.is_empty() {
                                None
                            } else {
                                Some(v)
                            }
                        };

                        let data = super::mostro::TakeOrderData {
                            order_id: order.id.clone(),
                            order_type: order.order_type.clone(),
                            cube_name: self.cube_name(),
                            mnemonic: self.mnemonic.clone(),
                            amount,
                            lightning_invoice: invoice,
                            mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                            relay_urls: self.mostro_config.relays.clone(),
                            fiat_code: Some(order.fiat_currency.clone()),
                            fiat_amount: Some(amount.unwrap_or(order.fiat_amount as i64)),
                            payment_method: Some(order.payment_methods.join(",")),
                            premium: order.premium_percent.map(|p| p.trunc() as i64),
                            sats_amount: order.sats_amount.and_then(|s| i64::try_from(s).ok()),
                            min_amount: order.min_amount.map(|m| m.trunc() as i64),
                            max_amount: order.max_amount.map(|m| m.trunc() as i64),
                        };

                        return Task::perform(super::mostro::take_order(data), |result| {
                            Message::View(view::Message::P2P(P2PMessage::TakeOrderResult(result)))
                        });
                    }
                }
                self.take_order_submitting = false;
            }
            P2PMessage::TakeOrderResult(result) => {
                self.take_order_submitting = false;
                self.taking_order = false;
                match result {
                    Ok(super::mostro::TakeOrderResponse::Success { order_id, .. }) => {
                        tracing::info!("Order taken: {}", order_id);

                        self.selected_order = None;
                        return Task::done(Message::View(view::Message::ShowSuccess(format!(
                            "Order taken successfully ({})",
                            order_id
                        ))));
                    }
                    Ok(super::mostro::TakeOrderResponse::PaymentRequired {
                        order_id,
                        invoice,
                        amount_sats,
                        ..
                    }) => {
                        tracing::info!("Order taken, payment required: {}", order_id);

                        self.selected_order = None;
                        // Reset Spark-pay state for the new modal session.
                        self.spark_balance_sat = None;
                        self.spark_pay_amount_sat = None;
                        self.spark_pay_attempt = None;
                        self.spark_pay_phase = SparkPayPhase::Idle;
                        self.show_qr_fallback = false;
                        self.spark_pay_session_id = Some(order_id.clone());
                        match qr_code::Data::new(&invoice) {
                            Ok(qr_data) => {
                                self.pending_payment_invoice =
                                    Some((order_id.clone(), invoice.clone(), amount_sats, qr_data));
                                // Kick off a balance lookup and an
                                // invoice parse in parallel. The
                                // modal renders QR-only until the
                                // balance lands; the parsed amount
                                // populates the Spark-pay summary.
                                let balance =
                                    self.spark_balance_fetch_task(cache, order_id.clone());
                                let parse = self
                                    .spark_parse_invoice_task(order_id.clone(), invoice.clone());
                                return match (balance, parse) {
                                    (Some(b), Some(p)) => Task::batch([b, p]),
                                    (Some(b), None) => b,
                                    (None, Some(p)) => p,
                                    (None, None) => Task::none(),
                                };
                            }
                            Err(e) => {
                                tracing::warn!("Failed to generate QR code: {e}");
                                return Task::done(Message::View(view::Message::ShowError(
                                    "Invoice too long for QR code display".to_string(),
                                )));
                            }
                        }
                    }
                    Err(e) => {
                        return Task::done(Message::View(view::Message::ShowError(e)));
                    }
                }
            }
            P2PMessage::DismissPaymentInvoice => {
                // Same race-protection as `CancelPaymentInvoice` / `CancelTrade`:
                // an on_blur during an in-flight prepare/send would drop the
                // session id, causing the eventual SparkPaySent to be discarded
                // as stale and leaving the user unaware the payment succeeded.
                if matches!(
                    self.spark_pay_phase,
                    SparkPayPhase::Preparing | SparkPayPhase::Sending
                ) {
                    return Task::none();
                }
                self.pending_payment_invoice = None;
                self.invoice_copied = false;
                self.spark_balance_sat = None;
                self.spark_pay_amount_sat = None;
                self.spark_pay_attempt = None;
                self.spark_pay_phase = SparkPayPhase::Idle;
                self.show_qr_fallback = false;
                self.spark_pay_session_id = None;
            }
            P2PMessage::SparkBalanceLoaded {
                order_id,
                balance_sat,
            } => {
                if self.spark_pay_session_id.as_deref() != Some(&order_id) {
                    tracing::info!(
                        target: "p2p::spark_pay",
                        "Ignoring stale SparkBalanceLoaded(balance={}) for order {} \
                         (current session={:?})",
                        balance_sat,
                        order_id,
                        self.spark_pay_session_id,
                    );
                    return Task::none();
                }
                tracing::info!(
                    target: "p2p::spark_pay",
                    "SparkBalanceLoaded for order {}: balance={} sats",
                    order_id,
                    balance_sat,
                );
                self.spark_balance_sat = Some(balance_sat);
            }
            P2PMessage::SparkBalanceFailed { order_id, err } => {
                if self.spark_pay_session_id.as_deref() != Some(&order_id) {
                    tracing::info!(
                        target: "p2p::spark_pay",
                        "Ignoring stale SparkBalanceFailed for order {} (err={}, current session={:?})",
                        order_id,
                        err,
                        self.spark_pay_session_id,
                    );
                    return Task::none();
                }
                tracing::warn!(
                    target: "p2p::spark_pay",
                    "Spark balance lookup failed for order {}: {}",
                    order_id,
                    err,
                );
                self.spark_balance_sat = None;
            }
            P2PMessage::SparkInvoiceAmountParsed {
                order_id,
                amount_sat,
            } => {
                if self.spark_pay_session_id.as_deref() != Some(&order_id) {
                    tracing::debug!(
                        target: "p2p::spark_pay",
                        "Ignoring stale SparkInvoiceAmountParsed for order {} (current session={:?})",
                        order_id,
                        self.spark_pay_session_id,
                    );
                    return Task::none();
                }
                tracing::info!(
                    target: "p2p::spark_pay",
                    "SparkInvoiceAmountParsed for order {}: amount_sat={:?}",
                    order_id,
                    amount_sat,
                );
                self.spark_pay_amount_sat = amount_sat;
            }
            P2PMessage::SparkPayPrepare { order_id, invoice } => {
                // Only honor "Pay from Spark" for the currently active
                // session. A button press that races with a session
                // change (modal dismiss + new take) would otherwise
                // kick off `prepare_send` against a freshly-opened
                // modal's invoice — using stale view state.
                if self.spark_pay_session_id.as_deref() != Some(&order_id) {
                    tracing::debug!("Ignoring stale SparkPayPrepare for order {}", order_id);
                    return Task::none();
                }
                let Some(spark) = self.spark_backend.clone() else {
                    self.spark_pay_phase =
                        SparkPayPhase::Error("Spark wallet is not available.".to_string());
                    return Task::none();
                };
                self.spark_pay_phase = SparkPayPhase::Preparing;
                // Track this prepare attempt so a transient failure
                // can be auto-retried once without bothering the user.
                self.spark_pay_attempt = Some((order_id.clone(), invoice.clone(), false));
                let session_id = order_id;
                return Task::perform(
                    async move { spark.prepare_send(invoice, None).await },
                    move |result| match result {
                        Ok(ok) => Message::View(view::Message::P2P(P2PMessage::SparkPayPrepared {
                            order_id: session_id.clone(),
                            ok,
                        })),
                        Err(e) => Message::View(view::Message::P2P(P2PMessage::SparkPayFailed {
                            order_id: session_id.clone(),
                            err: e.to_string(),
                        })),
                    },
                );
            }
            P2PMessage::SparkPayPrepared { order_id, ok } => {
                // Reject stale prepares so a `SparkPayConfirm` in a
                // fresh session can't reuse a previous session's
                // single-use `handle`.
                if self.spark_pay_session_id.as_deref() != Some(&order_id) {
                    tracing::debug!("Ignoring stale SparkPayPrepared for order {}", order_id);
                    return Task::none();
                }
                self.spark_pay_phase = SparkPayPhase::Prepared(ok);
            }
            P2PMessage::SparkPayConfirm => {
                let Some(spark) = self.spark_backend.clone() else {
                    self.spark_pay_phase =
                        SparkPayPhase::Error("Spark wallet is not available.".to_string());
                    return Task::none();
                };
                let SparkPayPhase::Prepared(ref prepared) = self.spark_pay_phase else {
                    return Task::none();
                };
                let Some(session_id) = self.spark_pay_session_id.clone() else {
                    // No active session — the view shouldn't expose
                    // this button here, but bail rather than fire an
                    // unattributed send.
                    return Task::none();
                };
                let handle = prepared.handle.clone();
                self.spark_pay_phase = SparkPayPhase::Sending;
                return Task::perform(
                    async move { spark.send_payment(handle).await },
                    move |result| match result {
                        Ok(ok) => Message::View(view::Message::P2P(P2PMessage::SparkPaySent {
                            order_id: session_id.clone(),
                            ok,
                        })),
                        Err(e) => Message::View(view::Message::P2P(P2PMessage::SparkPayFailed {
                            order_id: session_id.clone(),
                            err: e.to_string(),
                        })),
                    },
                );
            }
            P2PMessage::SparkPaySent { order_id, ok } => {
                if self.spark_pay_session_id.as_deref() != Some(&order_id) {
                    tracing::warn!(
                        "Ignoring SparkPaySent for old session {} (payment id={}, \
                         amount={}, fee={}) — the active session has moved on. \
                         Mostro will receive its hold-invoice settlement via the \
                         normal DM channel regardless.",
                        order_id,
                        ok.payment_id,
                        ok.amount_sat,
                        ok.fee_sat
                    );
                    return Task::none();
                }
                tracing::info!(
                    "Spark payment for hold invoice succeeded: id={}, amount={}, fee={}",
                    ok.payment_id,
                    ok.amount_sat,
                    ok.fee_sat
                );
                // Remember that this order's hold invoice was paid via
                // Spark. If Mostro later cancels/expires it (e.g. our
                // payment was too late), the trade would otherwise
                // disappear from My Trades while the Lightning HTLC
                // is still settling. Keeping the id in this set
                // overrides the default "hide canceled creator
                // trades" filter so the user can see the trade
                // resolve to Success or refund.
                self.spark_funded_order_ids.insert(order_id);
                self.pending_payment_invoice = None;
                self.invoice_copied = false;
                self.spark_balance_sat = None;
                self.spark_pay_amount_sat = None;
                self.spark_pay_attempt = None;
                self.spark_pay_phase = SparkPayPhase::Idle;
                self.show_qr_fallback = false;
                self.spark_pay_session_id = None;
            }
            P2PMessage::SparkPayFailed { order_id, err } => {
                if self.spark_pay_session_id.as_deref() != Some(&order_id) {
                    tracing::debug!(
                        "Ignoring stale SparkPayFailed for order {}: {}",
                        order_id,
                        err
                    );
                    return Task::none();
                }
                // Auto-retry once when a `prepare_send` blew up on a
                // transient error (LSP TLS close_notify, network EOF,
                // timeout). These resolve immediately on a second try
                // and were surfacing as a scary "Try again" prompt
                // even though the underlying state was recoverable.
                // We only retry the prepare path — send_payment
                // failures consume the prepared handle and can't be
                // retried with the same data.
                let retry_in_prepare_phase = matches!(
                    self.spark_pay_attempt.as_ref(),
                    Some((sid, _, false)) if sid == &order_id,
                );
                if retry_in_prepare_phase && is_transient_spark_error(&err) {
                    if let Some(spark) = self.spark_backend.clone() {
                        let invoice = self
                            .spark_pay_attempt
                            .as_ref()
                            .map(|(_, inv, _)| inv.clone())
                            .expect("retry_in_prepare_phase implies Some attempt");
                        if let Some(attempt) = self.spark_pay_attempt.as_mut() {
                            attempt.2 = true;
                        }
                        tracing::info!(
                            target: "p2p::spark_pay",
                            "Auto-retrying Spark prepare for {} after transient error: {}",
                            order_id,
                            err,
                        );
                        // Stay in Preparing so the spinner stays up —
                        // the user doesn't see the retry happen.
                        let session_id = order_id;
                        return Task::perform(
                            async move {
                                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                                spark.prepare_send(invoice, None).await
                            },
                            move |result| match result {
                                Ok(ok) => Message::View(view::Message::P2P(
                                    P2PMessage::SparkPayPrepared {
                                        order_id: session_id.clone(),
                                        ok,
                                    },
                                )),
                                Err(e) => {
                                    Message::View(view::Message::P2P(P2PMessage::SparkPayFailed {
                                        order_id: session_id.clone(),
                                        err: e.to_string(),
                                    }))
                                }
                            },
                        );
                    }
                }
                tracing::warn!(
                    target: "p2p::spark_pay",
                    "Spark pay failed for {}: {}",
                    order_id,
                    err,
                );
                self.spark_pay_attempt = None;
                self.spark_pay_phase = SparkPayPhase::Error(friendly_spark_pay_error(&err));
            }
            P2PMessage::SparkPayCancel => {
                self.spark_pay_phase = SparkPayPhase::Idle;
            }
            P2PMessage::ToggleQrFallback(show) => {
                // Ignore while a Spark RPC is in flight — the view freezes
                // these controls during Preparing/Sending, but stale events
                // (keyboard, mouse, hooks) could still arrive.
                if !matches!(
                    self.spark_pay_phase,
                    SparkPayPhase::Preparing | SparkPayPhase::Sending
                ) {
                    self.show_qr_fallback = show;
                }
            }
            P2PMessage::CopyPaymentInvoice(invoice) => {
                self.invoice_copied = true;
                return Task::batch([
                    Task::done(Message::View(view::Message::Clipboard(invoice))),
                    Task::perform(
                        async { tokio::time::sleep(std::time::Duration::from_secs(2)).await },
                        |_| Message::View(view::Message::P2P(P2PMessage::ResetInvoiceCopied)),
                    ),
                ]);
            }
            P2PMessage::ResetInvoiceCopied => {
                self.invoice_copied = false;
            }
            P2PMessage::CancelPaymentInvoice(order_id) => {
                // Block cancel while a Spark RPC is in flight. If the
                // prepare/send completes after we fire `cancel_trade`,
                // Mostro and Spark disagree on the order state and the
                // seller's sats can end up locked against a cancelled
                // trade. The view hides the button during these phases,
                // but guard the dispatch too for safety.
                if matches!(
                    self.spark_pay_phase,
                    SparkPayPhase::Preparing | SparkPayPhase::Sending
                ) {
                    return Task::none();
                }
                let data = super::mostro::TradeActionData {
                    order_id,
                    cube_name: self.cube_name(),
                    mnemonic: self.mnemonic.clone(),
                    invoice: None,
                    mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                    relay_urls: self.mostro_config.relays.clone(),
                };
                return Task::perform(super::mostro::cancel_trade(data), |result| {
                    Message::View(view::Message::P2P(P2PMessage::CancelOrderResult(
                        result.map(|_| ()),
                    )))
                });
            }
            // Trade detail
            P2PMessage::SelectTrade(id) => {
                // Cache QR code for the hold invoice if this trade has one
                let trade_has_hold_invoice = self
                    .trades
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.hold_invoice.as_ref())
                    .is_some();
                self.hold_invoice_qr = self
                    .trades
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.hold_invoice.as_ref())
                    .and_then(|inv| qr_code::Data::new(inv).ok());
                self.selected_trade = Some(id.clone());
                self.chat_selected_trade = None; // clear Chat tab context
                self.trade_invoice_input = Default::default();
                self.trade_action_loading = false;
                self.trade_rating = 0;
                self.active_chat = ActiveChat::None;
                self.chat_input = Default::default();
                // Reset Spark-pay state so a stale Error/Prepared from a
                // previous trade doesn't bleed into this view. Re-fetch
                // the balance only if this trade has a hold invoice the
                // user might pay from Spark; skip the RPC otherwise.
                self.spark_pay_phase = SparkPayPhase::Idle;
                self.show_qr_fallback = false;
                self.spark_pay_amount_sat = None;
                self.spark_pay_attempt = None;
                let balance_task = if trade_has_hold_invoice {
                    self.spark_pay_session_id = Some(id.clone());
                    self.spark_balance_sat = None;
                    let invoice = self
                        .trades
                        .iter()
                        .find(|t| t.id == id)
                        .and_then(|t| t.hold_invoice.clone());
                    let session_id = id.clone();
                    let balance = self.spark_balance_fetch_task(cache, session_id.clone());
                    let parse = invoice
                        .and_then(|inv| self.spark_parse_invoice_task(session_id.clone(), inv));
                    match (balance, parse) {
                        (Some(b), Some(p)) => {
                            tracing::info!(
                                target: "p2p::spark_pay",
                                "SelectTrade: kicking off balance fetch + invoice parse for order {}",
                                session_id,
                            );
                            Some(Task::batch([b, p]))
                        }
                        (Some(b), None) => {
                            tracing::info!(
                                target: "p2p::spark_pay",
                                "SelectTrade: kicking off balance fetch for order {} (no invoice to parse)",
                                session_id,
                            );
                            Some(b)
                        }
                        (None, _) => {
                            tracing::info!(
                                target: "p2p::spark_pay",
                                "SelectTrade: trade {} has hold invoice but Spark backend is None",
                                id,
                            );
                            None
                        }
                    }
                } else {
                    tracing::info!(
                        target: "p2p::spark_pay",
                        "SelectTrade: trade {} has no hold invoice yet, skipping balance fetch",
                        id,
                    );
                    self.spark_pay_session_id = None;
                    None
                };
                self.refresh_trade_cache();
                // Clear image cache from previous trade and trigger downloads for new one
                self.image_cache.clear();
                self.image_downloads_in_flight.clear();
                let image_task = self.trigger_image_downloads();
                return match balance_task {
                    Some(bt) => Task::batch([bt, image_task]),
                    None => image_task,
                };
            }
            P2PMessage::CloseTradeDetail => {
                self.selected_trade = None;
                self.trade_invoice_input = Default::default();
                self.trade_rating = 0;
                self.trade_action_loading = false;
                self.hold_invoice_qr = None;
                self.active_chat = ActiveChat::None;
                self.chat_input = Default::default();
                self.spark_pay_phase = SparkPayPhase::Idle;
                self.show_qr_fallback = false;
                self.spark_pay_session_id = None;
                self.spark_pay_amount_sat = None;
                self.spark_pay_attempt = None;
                self.refresh_trade_cache();
                self.image_cache.clear();
                self.image_downloads_in_flight.clear();
            }
            // Trade actions
            P2PMessage::TradeInvoiceEdited(v) => {
                self.trade_invoice_input.value = v;
            }
            P2PMessage::SubmitInvoice => {
                let invoice = Some(self.trade_invoice_input.value.trim().to_string());
                return self.perform_trade_action(invoice, super::mostro::submit_invoice);
            }
            P2PMessage::ConfirmFiatSent => {
                return self.perform_trade_action(None, super::mostro::confirm_fiat_sent);
            }
            P2PMessage::ConfirmFiatReceived => {
                return self.perform_trade_action(None, super::mostro::confirm_fiat_received);
            }
            P2PMessage::RatingSelected(rating) => {
                self.trade_rating = rating;
            }
            P2PMessage::SubmitRating => {
                let rating = self.trade_rating;
                if rating == 0 {
                    return Task::none();
                }
                let Some(ref order_id) = self.selected_trade else {
                    return Task::none();
                };
                self.trade_action_loading = true;
                let data = super::mostro::TradeActionData {
                    order_id: order_id.clone(),
                    cube_name: self.cube_name(),
                    mnemonic: self.mnemonic.clone(),
                    invoice: None,
                    mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                    relay_urls: self.mostro_config.relays.clone(),
                };
                return Task::perform(super::mostro::rate_counterparty(data, rating), |result| {
                    Message::View(view::Message::P2P(P2PMessage::TradeActionResult(result)))
                });
            }
            P2PMessage::CancelTrade => {
                // Same race-protection as `CancelPaymentInvoice`: don't
                // dispatch `cancel_trade` while a Spark RPC is in flight,
                // or Mostro and Spark end up disagreeing on whether sats
                // are locked.
                if matches!(
                    self.spark_pay_phase,
                    SparkPayPhase::Preparing | SparkPayPhase::Sending
                ) {
                    return Task::none();
                }
                return self.perform_trade_action(None, super::mostro::cancel_trade);
            }
            P2PMessage::OpenDispute => {
                return self.perform_trade_action(None, super::mostro::open_dispute);
            }
            P2PMessage::TradeActionResult(result) => {
                self.trade_action_loading = false;
                self.trade_rating = 0;
                match result {
                    Ok(resp) => {
                        let super::mostro::TradeActionResponse::Success { new_status } = resp;
                        tracing::info!("Trade action succeeded: {}", new_status);

                        // Update last_dm_action, timestamp, and status on the matching in-memory trade
                        if let Some(ref order_id) = self.selected_trade {
                            if let Some(trade) = self.trades.iter_mut().find(|t| t.id == *order_id)
                            {
                                let ts = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0);
                                trade.last_dm_action = Some(new_status.clone());
                                if is_countdown_action(&new_status) {
                                    trade.countdown_start_ts = Some(ts);
                                } else {
                                    trade.countdown_start_ts = None;
                                }
                                if let Some(s) = super::mostro::dm_action_to_status(&new_status) {
                                    trade.status = s;
                                }
                            }
                            let cube_name = self.cube_name();
                            super::mostro::append_trade_message(
                                &cube_name,
                                order_id,
                                super::mostro::TradeMessage {
                                    timestamp: chrono::Utc::now().timestamp() as u64,
                                    action: new_status.clone(),
                                    payload_json: String::new(),
                                    is_own: false,
                                },
                            );
                            self.refresh_trade_cache();
                        }

                        return Task::done(Message::View(view::Message::ShowSuccess(format!(
                            "Action completed: {}",
                            new_status
                        ))));
                    }
                    Err(e) => {
                        return Task::done(Message::View(view::Message::ShowError(e)));
                    }
                }
            }
            // Chat
            P2PMessage::OpenChat => {
                self.active_chat = ActiveChat::Peer;
                self.chat_input = Default::default();
            }
            P2PMessage::OpenChatForTrade(trade_id) => {
                self.chat_selected_trade = Some(trade_id);
                self.selected_trade = None; // clear MyTrades context
                self.active_chat = ActiveChat::Peer;
                self.chat_input = Default::default();
                self.refresh_chat_trade_cache();
                self.image_cache.clear();
                self.image_downloads_in_flight.clear();
                return self.trigger_image_downloads();
            }
            P2PMessage::CloseChat => {
                self.active_chat = ActiveChat::None;
                self.chat_input = Default::default();
                self.chat_selected_trade = None;
            }
            P2PMessage::ChatListTabMessages => {
                self.chat_list_tab = ChatListTab::Messages;
            }
            P2PMessage::ChatListTabDisputes => {
                self.chat_list_tab = ChatListTab::Disputes;
            }
            P2PMessage::OpenDisputeChatForTrade(trade_id) => {
                self.chat_selected_trade = Some(trade_id);
                self.selected_trade = None; // clear MyTrades context
                self.active_chat = ActiveChat::Dispute;
                self.dispute_chat_input = Default::default();
                self.refresh_chat_trade_cache();
                self.image_cache.clear();
                self.image_downloads_in_flight.clear();
            }
            P2PMessage::ChatInputEdited(v) => {
                self.chat_input.value = v;
            }
            P2PMessage::ToggleChatTradeInfo => {
                self.chat_show_trade_info = !self.chat_show_trade_info;
                if self.chat_show_trade_info {
                    self.chat_show_user_info = false;
                }
            }
            P2PMessage::ToggleChatUserInfo => {
                self.chat_show_user_info = !self.chat_show_user_info;
                if self.chat_show_user_info {
                    self.chat_show_trade_info = false;
                }
            }
            // Dispute chat
            P2PMessage::OpenDisputeChat => {
                self.active_chat = ActiveChat::Dispute;
                self.dispute_chat_input = Default::default();
            }
            P2PMessage::CloseDisputeChat => {
                self.active_chat = ActiveChat::None;
            }
            P2PMessage::DisputeChatInputEdited(v) => {
                self.dispute_chat_input.value = v;
            }
            P2PMessage::SendDisputeChatMessage => {
                let text = self.dispute_chat_input.value.trim().to_string();
                if text.is_empty() || self.pending_dispute_chat_message.is_some() {
                    return Task::none();
                }
                if let Some(ref order_id) = self.active_order_id() {
                    let cube_name = self.cube_name();
                    let payload = serde_json::to_string(&Some(
                        mostro_core::message::Payload::TextMessage(text.clone()),
                    ))
                    .unwrap_or_default();
                    self.pending_dispute_chat_message = Some(PendingChatMessage {
                        order_id: order_id.clone(),
                        cube_name: cube_name.clone(),
                        payload,
                        timestamp: chrono::Utc::now().timestamp() as u64,
                        original_text: text.clone(),
                    });
                    self.dispute_chat_input = Default::default();
                    let data = super::mostro::TradeActionData {
                        order_id: order_id.clone(),
                        cube_name,
                        mnemonic: self.mnemonic.clone(),
                        invoice: Some(text),
                        mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                        relay_urls: self.mostro_config.relays.clone(),
                    };
                    return Task::perform(super::mostro::send_admin_chat_message(data), |result| {
                        Message::View(view::Message::P2P(P2PMessage::DisputeChatMessageSent(
                            result,
                        )))
                    });
                }
            }
            P2PMessage::DisputeChatMessageSent(result) => {
                if let Some(pending) = self.pending_dispute_chat_message.take() {
                    match result {
                        Ok(()) => {
                            super::mostro::append_trade_message(
                                &pending.cube_name,
                                &pending.order_id,
                                super::mostro::TradeMessage {
                                    timestamp: pending.timestamp,
                                    action: "AdminDm".to_string(),
                                    payload_json: pending.payload,
                                    is_own: true,
                                },
                            );
                            self.refresh_trade_cache();
                        }
                        Err(e) => {
                            // Only restore input if the user is still viewing the same trade
                            if self.active_order_id().as_deref() == Some(&pending.order_id) {
                                self.dispute_chat_input.value = pending.original_text;
                            }
                            return Task::done(Message::View(view::Message::ShowError(format!(
                                "Dispute chat send failed: {}",
                                e
                            ))));
                        }
                    }
                }
            }
            P2PMessage::SendChatMessage => {
                let text = self.chat_input.value.trim().to_string();
                if text.is_empty() || self.pending_chat_message.is_some() {
                    return Task::none();
                }
                if let Some(ref order_id) = self.active_order_id() {
                    let cube_name = self.cube_name();

                    // Build payload but defer persisting until send succeeds
                    let payload = serde_json::to_string(&Some(
                        mostro_core::message::Payload::TextMessage(text.clone()),
                    ))
                    .unwrap_or_default();

                    self.pending_chat_message = Some(PendingChatMessage {
                        order_id: order_id.clone(),
                        cube_name: cube_name.clone(),
                        payload,
                        timestamp: chrono::Utc::now().timestamp() as u64,
                        original_text: text.clone(),
                    });

                    self.chat_input = Default::default();

                    // Fire async send
                    let data = super::mostro::TradeActionData {
                        order_id: order_id.clone(),
                        cube_name,
                        mnemonic: self.mnemonic.clone(),
                        invoice: Some(text),
                        mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                        relay_urls: self.mostro_config.relays.clone(),
                    };
                    return Task::perform(super::mostro::send_chat_message(data), |result| {
                        Message::View(view::Message::P2P(P2PMessage::ChatMessageSent(result)))
                    });
                }
            }
            P2PMessage::ChatMessageSent(result) => {
                if let Some(pending) = self.pending_chat_message.take() {
                    match result {
                        Ok(()) => {
                            super::mostro::append_trade_message(
                                &pending.cube_name,
                                &pending.order_id,
                                super::mostro::TradeMessage {
                                    timestamp: pending.timestamp,
                                    action: "SendDm".to_string(),
                                    payload_json: pending.payload,
                                    is_own: true,
                                },
                            );
                            self.refresh_trade_cache();
                        }
                        Err(e) => {
                            // Only restore input if the user is still viewing the same trade
                            if self.active_order_id().as_deref() == Some(&pending.order_id) {
                                self.chat_input.value = pending.original_text;
                            }
                            return Task::done(Message::View(view::Message::ShowError(format!(
                                "Chat send failed: {}",
                                e
                            ))));
                        }
                    }
                }
            }
            // Timer tick — no-op; re-render happens automatically
            P2PMessage::TradeTimerTick => {}
            // Real-time DM updates
            P2PMessage::TradeUpdate {
                order_id,
                action,
                payload_json,
            } => {
                tracing::info!("Trade update for {}: {}", order_id, action);

                let is_chat = action == "SendDm" || action == "AdminDm";

                // Update last_dm_action and status on the matching in-memory trade
                // (skip for chat messages — they don't change trade status)
                if !is_chat {
                    let now_ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    if let Some(trade) = self.trades.iter_mut().find(|t| t.id == order_id) {
                        trade.last_dm_action = Some(action.clone());
                        if is_countdown_action(&action) {
                            trade.countdown_start_ts = Some(now_ts);
                        } else {
                            trade.countdown_start_ts = None;
                        }
                        if let Some(new_status) = super::mostro::dm_action_to_status(&action) {
                            trade.status = new_status;
                        }
                        // Extract hold invoice from PayInvoice or BuyerTookOrder DM payload
                        let mut active_invoice_just_arrived: Option<String> = None;
                        if (action == "PayInvoice"
                            || action == "WaitingSellerToPay"
                            || action == "BuyerTookOrder")
                            && trade.hold_invoice.is_none()
                        {
                            if let Ok(Some(mostro_core::message::Payload::PaymentRequest(
                                _,
                                ref invoice,
                                _,
                            ))) = serde_json::from_str::<Option<mostro_core::message::Payload>>(
                                &payload_json,
                            ) {
                                trade.hold_invoice = Some(invoice.clone());
                                let is_active =
                                    self.active_order_id().as_deref() == Some(&order_id);
                                if is_active {
                                    self.hold_invoice_qr = qr_code::Data::new(invoice).ok();
                                    active_invoice_just_arrived = Some(invoice.clone());
                                }
                            }
                        }
                        // If the user is sitting on the trade detail when
                        // the hold-invoice DM lands, `SelectTrade`'s
                        // earlier balance fetch was skipped (no invoice
                        // at that point). Kick one off now so the
                        // Spark-pay gate can flip without requiring the
                        // user to navigate away and back.
                        if let Some(invoice) = active_invoice_just_arrived {
                            self.spark_pay_phase = SparkPayPhase::Idle;
                            self.show_qr_fallback = false;
                            self.spark_pay_session_id = Some(order_id.clone());
                            self.spark_balance_sat = None;
                            self.spark_pay_amount_sat = None;
                            self.spark_pay_attempt = None;
                            let balance = self.spark_balance_fetch_task(cache, order_id.clone());
                            let parse = self.spark_parse_invoice_task(order_id.clone(), invoice);
                            match (balance, parse) {
                                (Some(b), Some(p)) => {
                                    tracing::info!(
                                        target: "p2p::spark_pay",
                                        "TradeUpdate({}): hold invoice landed on active trade — \
                                         kicking off balance fetch + invoice parse",
                                        order_id,
                                    );
                                    return Task::batch([b, p]);
                                }
                                (Some(b), None) => {
                                    tracing::info!(
                                        target: "p2p::spark_pay",
                                        "TradeUpdate({}): hold invoice landed on active trade — \
                                         kicking off balance fetch",
                                        order_id,
                                    );
                                    return b;
                                }
                                (None, _) => {
                                    tracing::info!(
                                        target: "p2p::spark_pay",
                                        "TradeUpdate({}): hold invoice landed but Spark backend is None",
                                        order_id,
                                    );
                                }
                            }
                        } else if action == "PayInvoice"
                            || action == "WaitingSellerToPay"
                            || action == "BuyerTookOrder"
                        {
                            tracing::info!(
                                target: "p2p::spark_pay",
                                "TradeUpdate({}): DM action={} did not produce an \
                                 active-trade invoice arrival (active_order_id={:?}, \
                                 trade.hold_invoice already set?={})",
                                order_id,
                                action,
                                self.active_order_id(),
                                self.trades.iter().find(|t| t.id == order_id)
                                    .map(|t| t.hold_invoice.is_some())
                                    .unwrap_or(false),
                            );
                        }
                    }
                }

                // Chat messages are already persisted by process_dm_notifications.
                // Still refresh the cache so the view picks up the new message.
                if is_chat {
                    if self.active_order_id().as_deref() == Some(&order_id) {
                        self.refresh_trade_cache();
                        return self.trigger_image_downloads();
                    }
                    return Task::none();
                }

                // Persist non-chat protocol messages to disk
                let cube_name = self.cube_name();
                super::mostro::append_trade_message(
                    &cube_name,
                    &order_id,
                    super::mostro::TradeMessage {
                        timestamp: chrono::Utc::now().timestamp() as u64,
                        action: action.clone(),
                        payload_json,
                        is_own: false,
                    },
                );

                // Refresh cache when this update belongs to the active trade
                if self.active_order_id().as_deref() == Some(&order_id) {
                    self.refresh_trade_cache();
                }

                return Task::done(Message::View(view::Message::ShowSuccess(format!(
                    "Trade update: {} ({})",
                    action,
                    &order_id[..8.min(order_id.len())]
                ))));
            }
            P2PMessage::StreamError(msg) => {
                self.stream_error = Some(msg);
            }
            // ── Image attachment messages ──
            P2PMessage::AttachFile => {
                if self.attachment_sending || self.pending_chat_message.is_some() {
                    return Task::none();
                }
                // Open file picker — attachment_sending is NOT set yet
                return Task::perform(
                    async move {
                        let dialog = rfd::AsyncFileDialog::new()
                            .set_title("Send File")
                            .add_filter(
                                "All supported",
                                &[
                                    "png", "jpg", "jpeg", "gif", "webp", "mp4", "mov", "avi",
                                    "pdf", "doc", "docx",
                                ],
                            )
                            .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
                            .add_filter("Videos", &["mp4", "mov", "avi"])
                            .add_filter("Documents", &["pdf", "doc", "docx"]);
                        dialog.pick_file().await
                    },
                    |file| {
                        if let Some(file) = file {
                            Message::View(view::Message::P2P(P2PMessage::FileSelected(
                                file.path().to_path_buf(),
                            )))
                        } else {
                            // User cancelled — no-op
                            Message::View(view::Message::P2P(P2PMessage::AttachmentSent(Err(
                                "cancelled".to_string(),
                            ))))
                        }
                    },
                );
            }
            P2PMessage::FileSelected(path) => {
                if let Some(ref order_id) = self.active_order_id() {
                    let order_id = order_id.clone();
                    let oid_for_result = order_id.clone();
                    let cube_name = self.cube_name();
                    let mnemonic = self.mnemonic.clone();
                    let mostro_pubkey_hex = self.mostro_config.active_pubkey_hex().to_string();
                    let relay_urls = self.mostro_config.relays.clone();

                    self.attachment_sending = true;
                    return Task::perform(
                        super::mostro::send_attachment(super::mostro::AttachmentData {
                            file_path: path,
                            order_id,
                            cube_name,
                            mnemonic,
                            mostro_pubkey_hex,
                            relay_urls,
                        }),
                        move |result| {
                            Message::View(view::Message::P2P(P2PMessage::AttachmentSent(
                                result.map(|meta| (oid_for_result, meta)),
                            )))
                        },
                    );
                }
            }
            P2PMessage::AttachmentSent(result) => {
                self.attachment_sending = false;
                match result {
                    Ok((order_id, metadata_json)) => {
                        let payload = serde_json::to_string(&Some(
                            mostro_core::message::Payload::TextMessage(metadata_json),
                        ))
                        .unwrap_or_default();
                        super::mostro::append_trade_message(
                            &self.cube_name(),
                            &order_id,
                            super::mostro::TradeMessage {
                                timestamp: chrono::Utc::now().timestamp() as u64,
                                action: "SendDm".to_string(),
                                payload_json: payload,
                                is_own: true,
                            },
                        );
                        self.refresh_trade_cache();
                        let dl = self.trigger_image_downloads();
                        return dl;
                    }
                    Err(e) => {
                        if e != "cancelled" {
                            return Task::done(Message::View(view::Message::ShowError(format!(
                                "Image send failed: {e}"
                            ))));
                        }
                    }
                }
            }
            P2PMessage::AttachmentDownloaded {
                order_id,
                blossom_url,
                data,
            } => {
                self.image_downloads_in_flight.remove(&blossom_url);
                match data {
                    Ok(bytes) => {
                        let handle = iced::widget::image::Handle::from_bytes(bytes);
                        self.image_cache
                            .insert(blossom_url, ImageCacheEntry::Ready(handle));
                    }
                    Err(e) => {
                        if !blossom_url.is_empty() {
                            self.image_cache
                                .insert(blossom_url, ImageCacheEntry::Failed(e));
                        } else {
                            tracing::warn!("Image download failed: {e}");
                        }
                    }
                }
                let _ = order_id; // used for future scoping if needed
            }
            P2PMessage::SaveFile {
                blossom_url,
                filename,
            } => {
                if let Some(order_id) = self.active_order_id() {
                    let cube_name = self.cube_name();
                    let mnemonic = self.mnemonic.clone();
                    return Task::perform(
                        super::mostro::download_and_save_file(
                            blossom_url,
                            filename,
                            order_id,
                            cube_name,
                            mnemonic,
                        ),
                        |result| Message::View(view::Message::P2P(P2PMessage::FileSaved(result))),
                    );
                }
            }
            P2PMessage::FileSaved(result) => match result {
                Ok(()) => {
                    return Task::done(Message::View(view::Message::ShowSuccess(
                        "File saved".to_string(),
                    )));
                }
                Err(e) => {
                    if e != "cancelled" {
                        return Task::done(Message::View(view::Message::ShowError(format!(
                            "File save failed: {e}",
                        ))));
                    }
                }
            },
        }
        Task::none()
    }
}
