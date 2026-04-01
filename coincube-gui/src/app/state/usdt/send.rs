use std::sync::Arc;
use std::time::Duration;

use coincube_ui::widget::*;
use iced::{clipboard, Subscription, Task};

use crate::app::breez::assets::{parse_asset_to_minor_units, usdt_asset_id, USDT_PRECISION};
use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::liquid::send::{LiquidSend, SendAsset};
use crate::app::state::State;
use crate::app::view;
use crate::app::wallet::Wallet;
use crate::daemon::Daemon;
use crate::services::coincube::CoincubeClient;
use crate::services::sideshift::{
    ShiftQuote, ShiftResponse, ShiftStatusKind, SideshiftClient, SideshiftNetwork,
};

use view::{SideshiftSendMessage, SideshiftShiftType};

// ---------------------------------------------------------------------------
// Send phase
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendPhase {
    /// Enter recipient address (balance shown, network auto-detected).
    AddressInput,
    /// Address matched multiple networks (0x) — pick one.
    NetworkDisambiguation,
    /// Sending natively via Liquid — delegated to LiquidSend.
    LiquidNative,
    /// Enter amount + fixed/variable choice.
    AmountInput,
    /// Fetching affiliate ID.
    FetchingAffiliate,
    /// Fetching a fixed-rate quote.
    FetchingQuote,
    /// Creating the SideShift shift.
    CreatingShift,
    /// Shift created — reviewing fees before confirming payment.
    Review,
    /// Preparing / sending the Liquid USDt payment to SideShift deposit address.
    Sending,
    /// Liquid USDt payment sent; polling shift status.
    Sent,
    /// Terminal error.
    Failed,
}

// ---------------------------------------------------------------------------
// UsdtSend
// ---------------------------------------------------------------------------

pub struct UsdtSend {
    inner: LiquidSend,

    phase: SendPhase,
    selected_network: Option<SideshiftNetwork>,
    /// Candidate networks detected from address format.
    detected_networks: Vec<SideshiftNetwork>,
    shift_type: SideshiftShiftType,
    recipient_address: String,
    amount_input: String,

    affiliate_id: Option<String>,
    quote: Option<ShiftQuote>,
    shift: Option<ShiftResponse>,
    shift_status: Option<ShiftStatusKind>,

    loading: bool,
    error: Option<String>,

    coincube_client: CoincubeClient,
    sideshift_client: SideshiftClient,
}

impl UsdtSend {
    pub fn new(inner: LiquidSend) -> Self {
        Self {
            inner,
            phase: SendPhase::AddressInput,
            selected_network: None,
            detected_networks: vec![],
            shift_type: SideshiftShiftType::Variable,
            recipient_address: String::new(),
            amount_input: String::new(),
            affiliate_id: None,
            quote: None,
            shift: None,
            shift_status: None,
            loading: false,
            error: None,
            coincube_client: CoincubeClient::new(),
            sideshift_client: SideshiftClient::new(),
        }
    }

    fn reset(&mut self) {
        self.phase = SendPhase::AddressInput;
        self.selected_network = None;
        self.detected_networks.clear();
        self.shift_type = SideshiftShiftType::Variable;
        self.recipient_address.clear();
        self.amount_input.clear();
        self.affiliate_id = None;
        self.quote = None;
        self.shift = None;
        self.shift_status = None;
        self.loading = false;
        self.error = None;
    }

    fn fetch_affiliate_id(&self) -> Task<Message> {
        let client = self.coincube_client.clone();
        Task::perform(
            async move { client.get_sideshift_affiliate_id().await },
            |result| {
                Message::View(view::Message::SideshiftSend(
                    SideshiftSendMessage::AffiliateFetched(result),
                ))
            },
        )
    }

    fn fetch_quote(&self, affiliate_id: &str) -> Task<Message> {
        let client = self.sideshift_client.clone();
        let settle_network = self
            .selected_network
            .as_ref()
            .map(|n| n.network_slug().to_string())
            .unwrap_or_default();
        let affiliate_id = affiliate_id.to_string();
        let deposit_amount = if self.amount_input.trim().is_empty() {
            None
        } else {
            Some(self.amount_input.trim().to_string())
        };

        Task::perform(
            async move {
                client
                    .get_quote(
                        "liquid",
                        &settle_network,
                        None,
                        deposit_amount.as_deref(),
                        &affiliate_id,
                    )
                    .await
            },
            |result| {
                Message::View(view::Message::SideshiftSend(
                    SideshiftSendMessage::QuoteFetched(result),
                ))
            },
        )
    }

    fn create_shift(&self, affiliate_id: &str, quote: Option<&ShiftQuote>) -> Task<Message> {
        let client = self.sideshift_client.clone();
        let settle_network = self
            .selected_network
            .as_ref()
            .map(|n| n.network_slug().to_string())
            .unwrap_or_default();
        let settle_address = self.recipient_address.clone();
        let affiliate_id = affiliate_id.to_string();
        let is_fixed = self.shift_type == SideshiftShiftType::Fixed && quote.is_some();
        let quote_id = quote.map(|q| q.id.clone());

        Task::perform(
            async move {
                if is_fixed {
                    let qid = quote_id.ok_or("Missing quote ID for fixed shift")?;
                    client
                        .create_fixed_shift(&qid, &settle_address, &affiliate_id)
                        .await
                } else {
                    client
                        .create_variable_send_shift(&settle_network, &settle_address, &affiliate_id)
                        .await
                }
            },
            |result| {
                Message::View(view::Message::SideshiftSend(
                    SideshiftSendMessage::ShiftCreated(result),
                ))
            },
        )
    }

    fn poll_shift_status(&self) -> Task<Message> {
        if let Some(shift) = &self.shift {
            let client = self.sideshift_client.clone();
            let shift_id = shift.id.clone();
            Task::perform(
                async move { client.get_shift_status(&shift_id).await },
                |result| {
                    Message::View(view::Message::SideshiftSend(
                        SideshiftSendMessage::StatusUpdated(result),
                    ))
                },
            )
        } else {
            Task::none()
        }
    }
}

// ---------------------------------------------------------------------------
// State impl
// ---------------------------------------------------------------------------

impl State for UsdtSend {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        if self.phase == SendPhase::LiquidNative {
            return self.inner.view(menu, cache);
        }

        let asset_id = usdt_asset_id(self.inner.breez_client().network()).unwrap_or("");
        let sideshift_view = view::usdt::usdt_send_view(
            &self.phase,
            self.selected_network.as_ref(),
            &self.detected_networks,
            &self.shift_type,
            &self.recipient_address,
            &self.amount_input,
            self.inner.usdt_balance(),
            self.inner.recent_transactions(),
            self.shift.as_ref(),
            self.shift_status.as_ref(),
            self.loading,
            self.error.as_deref(),
            asset_id,
        );

        view::dashboard(
            menu,
            cache,
            sideshift_view.map(view::Message::SideshiftSend),
        )
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if self.phase == SendPhase::LiquidNative {
            return self.inner.update(daemon, cache, message);
        }

        // Always forward LiquidSend messages to inner state (e.g. DataLoaded for balance).
        if matches!(message, Message::View(view::Message::LiquidSend(_))) {
            return self.inner.update(daemon, cache, message);
        }

        if let Message::View(view::Message::SideshiftSend(ref msg)) = message {
            match msg {
                SideshiftSendMessage::RecipientAddressEdited(v) => {
                    self.recipient_address = v.clone();
                    self.error = None;
                    // Auto-detect network from address
                    self.detected_networks =
                        SideshiftNetwork::detect_from_address(&self.recipient_address);
                    // Auto-select if unambiguous
                    if self.detected_networks.len() == 1 {
                        self.selected_network = Some(self.detected_networks[0]);
                    } else {
                        self.selected_network = None;
                    }
                    return Task::none();
                }

                SideshiftSendMessage::DisambiguateNetwork(network) => {
                    self.selected_network = Some(*network);
                    return Task::none();
                }

                SideshiftSendMessage::Next => {
                    let addr = self.recipient_address.trim();
                    if addr.is_empty() {
                        self.error = Some("Please enter a recipient address".to_string());
                        return Task::none();
                    }
                    if self.detected_networks.is_empty() {
                        self.error = Some("Unrecognised address format".to_string());
                        return Task::none();
                    }

                    // If Liquid detected, go to native send
                    if self.selected_network == Some(SideshiftNetwork::Liquid) {
                        self.phase = SendPhase::LiquidNative;
                        let reload_task = self.inner.reload(daemon, None);
                        let preset_task = Task::done(Message::View(view::Message::LiquidSend(
                            view::LiquidSendMessage::PresetAsset(SendAsset::Usdt),
                        )));
                        return Task::batch(vec![reload_task, preset_task]);
                    }

                    // Ambiguous? Need disambiguation first
                    if self.detected_networks.len() > 1 && self.selected_network.is_none() {
                        self.phase = SendPhase::NetworkDisambiguation;
                        return Task::none();
                    }

                    if self.selected_network.is_none() {
                        self.error = Some("Please select a network".to_string());
                        return Task::none();
                    }

                    self.phase = SendPhase::AmountInput;
                    return Task::none();
                }

                SideshiftSendMessage::ToggleShiftType(st) => {
                    self.shift_type = st.clone();
                    return Task::none();
                }

                SideshiftSendMessage::AmountInput(v) => {
                    self.amount_input = v.clone();
                    self.shift_type = if v.trim().is_empty() {
                        SideshiftShiftType::Variable
                    } else {
                        SideshiftShiftType::Fixed
                    };
                    return Task::none();
                }

                SideshiftSendMessage::Generate => {
                    // Amount is always required for sends — we need to know
                    // how much USDt to pay into SideShift's deposit address.
                    let trimmed = self.amount_input.trim();
                    if trimmed.is_empty() {
                        self.error = Some("Please enter an amount to send".to_string());
                        return Task::none();
                    }
                    // Use parse_asset_to_minor_units as the single validator —
                    // this is the same parser used at payment time.
                    let base_units = match parse_asset_to_minor_units(trimmed, USDT_PRECISION) {
                        Some(v) => v,
                        None => {
                            self.error = Some("Please enter a valid amount".to_string());
                            return Task::none();
                        }
                    };
                    // 5 USDt minimum in base units (10^8 per USDt).
                    let min_base = 5 * 10_u64.pow(USDT_PRECISION as u32);
                    if base_units < min_base {
                        self.error = Some("Minimum amount is 5 USDt".to_string());
                        return Task::none();
                    }
                    if base_units > self.inner.usdt_balance() {
                        self.error = Some("Insufficient USDt balance".to_string());
                        return Task::none();
                    }
                    self.loading = true;
                    self.error = None;
                    self.phase = SendPhase::FetchingAffiliate;
                    return self.fetch_affiliate_id();
                }

                SideshiftSendMessage::AffiliateFetched(result) => {
                    match result {
                        Ok(id) => {
                            self.affiliate_id = Some(id.clone());
                            if self.shift_type == SideshiftShiftType::Fixed
                                && !self.amount_input.trim().is_empty()
                            {
                                self.phase = SendPhase::FetchingQuote;
                                return self.fetch_quote(id);
                            } else {
                                self.phase = SendPhase::CreatingShift;
                                return self.create_shift(id, None);
                            }
                        }
                        Err(e) => {
                            self.loading = false;
                            self.phase = SendPhase::Failed;
                            self.error = Some(format!("Failed to fetch SideShift config: {}", e));
                        }
                    }
                    return Task::none();
                }

                SideshiftSendMessage::QuoteFetched(result) => {
                    match result {
                        Ok(quote) => {
                            let affiliate_id = self.affiliate_id.clone().unwrap_or_default();
                            self.quote = Some(quote.clone());
                            self.phase = SendPhase::CreatingShift;
                            return self.create_shift(&affiliate_id, Some(quote));
                        }
                        Err(e) => {
                            self.loading = false;
                            self.phase = SendPhase::Failed;
                            self.error = Some(format!("Quote failed: {}", e));
                        }
                    }
                    return Task::none();
                }

                SideshiftSendMessage::ShiftCreated(result) => {
                    self.loading = false;
                    match result {
                        Ok(shift) => {
                            self.shift = Some(shift.clone());
                            self.phase = SendPhase::Review;
                        }
                        Err(e) => {
                            self.phase = SendPhase::Failed;
                            self.error = Some(format!("Failed to create shift: {}", e));
                        }
                    }
                    return Task::none();
                }

                SideshiftSendMessage::ConfirmSend => {
                    let Some(shift) = &self.shift else {
                        return Task::none();
                    };
                    let deposit_address = shift.deposit_address.clone();
                    let breez = self.inner.breez_client().clone();
                    let network = breez.network();
                    // Prefer API-confirmed deposit amount over raw user input
                    let amount_input = shift
                        .deposit_amount
                        .clone()
                        .unwrap_or_else(|| self.amount_input.clone());

                    self.phase = SendPhase::Sending;
                    self.error = None;

                    // Parse amount and prepare the Liquid USDt payment
                    return Task::perform(
                        async move {
                            let asset_id = usdt_asset_id(network)
                                .ok_or_else(|| "USDt not available on network".to_string())?;
                            let amount_str = amount_input.trim();
                            let base_units = if amount_str.is_empty() {
                                return Err("Amount is required for send".to_string());
                            } else {
                                parse_asset_to_minor_units(amount_str, USDT_PRECISION)
                                    .ok_or_else(|| "Invalid amount".to_string())?
                            };
                            breez
                                .prepare_send_asset(
                                    deposit_address,
                                    asset_id,
                                    base_units,
                                    USDT_PRECISION,
                                    None,
                                )
                                .await
                                .map_err(|e| e.to_string())
                        },
                        |result| match result {
                            Ok(prepare_response) => Message::View(view::Message::SideshiftSend(
                                SideshiftSendMessage::PaymentPrepared(prepare_response),
                            )),
                            Err(e) => Message::View(view::Message::SideshiftSend(
                                SideshiftSendMessage::PaymentFailed(e),
                            )),
                        },
                    );
                }

                SideshiftSendMessage::PaymentPrepared(prepare_response) => {
                    let breez = self.inner.breez_client().clone();
                    let prepare = prepare_response.clone();
                    return Task::perform(
                        async move {
                            breez
                                .send_payment(&breez_sdk_liquid::prelude::SendPaymentRequest {
                                    prepare_response: prepare,
                                    payer_note: None,
                                    use_asset_fees: Some(true),
                                })
                                .await
                                .map_err(|e| e.to_string())
                        },
                        |result| match result {
                            Ok(_) => Message::View(view::Message::SideshiftSend(
                                SideshiftSendMessage::PaymentSent,
                            )),
                            Err(e) => Message::View(view::Message::SideshiftSend(
                                SideshiftSendMessage::PaymentFailed(e),
                            )),
                        },
                    );
                }

                SideshiftSendMessage::PaymentSent => {
                    self.phase = SendPhase::Sent;
                    self.shift_status = Some(ShiftStatusKind::Pending);
                    return Task::none();
                }

                SideshiftSendMessage::PaymentFailed(e) => {
                    self.phase = SendPhase::Failed;
                    self.error = Some(format!("Payment failed: {}", e));
                    return Task::none();
                }

                SideshiftSendMessage::SendComplete => {
                    self.phase = SendPhase::Sent;
                    self.shift_status = Some(ShiftStatusKind::Pending);
                    return Task::none();
                }

                SideshiftSendMessage::PollStatus => {
                    return self.poll_shift_status();
                }

                SideshiftSendMessage::StatusUpdated(result) => {
                    if let Ok(status) = result {
                        let kind = ShiftStatusKind::from(status.status.as_str());
                        self.shift_status = Some(kind);
                    }
                    return Task::none();
                }

                SideshiftSendMessage::Back => {
                    self.error = None;
                    self.loading = false;
                    match self.phase {
                        SendPhase::NetworkDisambiguation => {
                            self.phase = SendPhase::AddressInput;
                        }
                        SendPhase::AmountInput => {
                            // Back to address input, keep address + network
                            self.phase = SendPhase::AddressInput;
                        }
                        SendPhase::Review => {
                            // Back to amount input, keep address/network/amount;
                            // clear shift data since it was a pending quote
                            self.shift = None;
                            self.quote = None;
                            self.affiliate_id = None;
                            self.phase = SendPhase::AmountInput;
                        }
                        _ => {
                            // From Sent/Failed, full reset
                            self.reset();
                            let reload_task = self.inner.reload(daemon, None);
                            let preset_task = Task::done(Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::PresetAsset(SendAsset::Usdt),
                            )));
                            return Task::batch(vec![reload_task, preset_task]);
                        }
                    }
                    return Task::none();
                }

                SideshiftSendMessage::Reset => {
                    self.reset();
                    let reload_task = self.inner.reload(daemon, None);
                    let preset_task = Task::done(Message::View(view::Message::LiquidSend(
                        view::LiquidSendMessage::PresetAsset(SendAsset::Usdt),
                    )));
                    return Task::batch(vec![reload_task, preset_task]);
                }

                SideshiftSendMessage::Error(e) => {
                    self.error = Some(e.clone());
                    self.loading = false;
                    return Task::none();
                }

                SideshiftSendMessage::Copy => {
                    if let Some(shift) = &self.shift {
                        return clipboard::write(shift.id.clone());
                    }
                    return Task::none();
                }
            }
        }

        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.phase == SendPhase::LiquidNative {
            return self.inner.subscription();
        }
        let is_terminal = self
            .shift_status
            .as_ref()
            .is_some_and(ShiftStatusKind::is_terminal);
        if self.phase == SendPhase::Sent && !is_terminal {
            iced::time::every(Duration::from_secs(10)).map(|_| {
                Message::View(view::Message::SideshiftSend(
                    SideshiftSendMessage::PollStatus,
                ))
            })
        } else {
            Subscription::none()
        }
    }

    fn close(&mut self) -> Task<Message> {
        self.inner.close()
    }

    fn interrupt(&mut self) {
        self.inner.interrupt()
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.reset();
        let reload_task = self.inner.reload(daemon, wallet);
        let preset_task = Task::done(Message::View(view::Message::LiquidSend(
            view::LiquidSendMessage::PresetAsset(SendAsset::Usdt),
        )));
        Task::batch(vec![reload_task, preset_task])
    }
}
