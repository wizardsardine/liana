use std::sync::Arc;
use std::time::Duration;

use coincube_ui::widget::Element;
use iced::{clipboard, Subscription, Task};

use crate::app::breez_liquid::assets::{parse_asset_to_minor_units, usdt_asset_id, USDT_PRECISION};
use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::view;
use crate::app::wallets::LiquidBackend;
use crate::services::coincube::CoincubeClient;
use crate::services::sideshift::{
    ShiftQuote, ShiftResponse, ShiftStatusKind, SideshiftClient, SideshiftNetwork,
};

use view::{SideshiftSendMessage, SideshiftShiftType};

/// The SideShift client no longer creates shifts that settle to Liquid: the
/// Liquid wallet is being removed and is hidden for everyone
/// (`LiquidGate::show()` is false), so this flow is unreachable. It keeps
/// compiling, failing at the quote/shift step with this message, until the
/// Liquid modules are deleted.
const LIQUID_SHIFTS_RETIRED: &str =
    "SideShift swaps for the Liquid wallet are no longer available. Use Receive in the Spark wallet.";

// ---------------------------------------------------------------------------
// Send phase
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendPhase {
    /// Enter recipient address (balance shown, network auto-detected).
    AddressInput,
    /// Address matched multiple networks (0x) — pick one.
    NetworkDisambiguation,
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
// SideshiftSendFlow
// ---------------------------------------------------------------------------

pub struct SideshiftSendFlow {
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
    /// Final amount reported by SideShift when the shift settles.
    confirmed_settle_amount: Option<String>,

    loading: bool,
    error: Option<String>,

    coincube_client: CoincubeClient,
    sideshift_client: SideshiftClient,
}

impl Default for SideshiftSendFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl SideshiftSendFlow {
    pub fn new() -> Self {
        Self {
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
            confirmed_settle_amount: None,
            loading: false,
            error: None,
            coincube_client: CoincubeClient::new(),
            sideshift_client: SideshiftClient::new(),
        }
    }

    pub fn phase(&self) -> &SendPhase {
        &self.phase
    }

    pub fn selected_network(&self) -> Option<&SideshiftNetwork> {
        self.selected_network.as_ref()
    }

    pub fn detected_networks(&self) -> &[SideshiftNetwork] {
        &self.detected_networks
    }

    pub fn shift_type(&self) -> &SideshiftShiftType {
        &self.shift_type
    }

    pub fn recipient_address(&self) -> &str {
        &self.recipient_address
    }

    pub fn amount_input(&self) -> &str {
        &self.amount_input
    }

    pub fn shift(&self) -> Option<&ShiftResponse> {
        self.shift.as_ref()
    }

    pub fn shift_status(&self) -> Option<&ShiftStatusKind> {
        self.shift_status.as_ref()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Returns `true` if the detected address is a native Liquid address.
    pub fn is_liquid_native(&self) -> bool {
        self.selected_network == Some(SideshiftNetwork::Liquid)
    }

    pub fn reset(&mut self) {
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
        self.confirmed_settle_amount = None;
        self.loading = false;
        self.error = None;
    }

    // -----------------------------------------------------------------------
    // Async helpers
    // -----------------------------------------------------------------------

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

    fn fetch_quote(&self, _affiliate_id: &str) -> Task<Message> {
        Task::done(Message::View(view::Message::SideshiftSend(
            SideshiftSendMessage::QuoteFetched(Err(LIQUID_SHIFTS_RETIRED.to_string())),
        )))
    }

    fn create_shift(&self, _affiliate_id: &str, _quote: Option<&ShiftQuote>) -> Task<Message> {
        Task::done(Message::View(view::Message::SideshiftSend(
            SideshiftSendMessage::ShiftCreated(Err(LIQUID_SHIFTS_RETIRED.to_string())),
        )))
    }

    fn poll_shift_status(&self) -> Task<Message> {
        if let Some(shift) = &self.shift {
            let client = self.sideshift_client.clone();
            let shift_id = shift.id.clone();
            Task::perform(
                async move {
                    let result = client.get_shift_status(&shift_id).await;
                    (shift_id, result)
                },
                |(shift_id, result)| {
                    Message::View(view::Message::SideshiftSend(
                        SideshiftSendMessage::StatusUpdated(shift_id, result),
                    ))
                },
            )
        } else {
            Task::none()
        }
    }

    // -----------------------------------------------------------------------
    // View
    // -----------------------------------------------------------------------

    pub fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
        usdt_balance: u64,
        recent_transactions: &'a [view::liquid::RecentTransaction],
        usdt_asset_id_str: &str,
    ) -> Element<'a, view::Message> {
        let sideshift_view = view::liquid::sideshift_send_view(
            &self.phase,
            self.selected_network.as_ref(),
            &self.detected_networks,
            &self.shift_type,
            &self.recipient_address,
            &self.amount_input,
            usdt_balance,
            recent_transactions,
            self.shift.as_ref(),
            self.shift_status.as_ref(),
            self.confirmed_settle_amount.as_deref(),
            self.loading,
            self.error.as_deref(),
            usdt_asset_id_str,
            cache.balance_amounts_masked(),
        );

        view::dashboard(
            menu,
            cache,
            sideshift_view.map(view::Message::SideshiftSend),
        )
    }

    // -----------------------------------------------------------------------
    // Update
    // -----------------------------------------------------------------------

    pub fn update(
        &mut self,
        msg: &SideshiftSendMessage,
        breez_client: &Arc<LiquidBackend>,
        usdt_balance: u64,
    ) -> Task<Message> {
        match msg {
            SideshiftSendMessage::RecipientAddressEdited(v) => {
                self.recipient_address = v.clone();
                self.error = None;
                self.detected_networks =
                    SideshiftNetwork::detect_from_address(&self.recipient_address);
                if self.detected_networks.len() == 1 {
                    self.selected_network = Some(self.detected_networks[0]);
                } else {
                    self.selected_network = None;
                }
                Task::none()
            }

            SideshiftSendMessage::DisambiguateNetwork(network) => {
                self.selected_network = Some(*network);
                Task::none()
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

                // Ambiguous? Need disambiguation first
                if self.detected_networks.len() > 1 && self.selected_network.is_none() {
                    self.phase = SendPhase::NetworkDisambiguation;
                    return Task::none();
                }

                if self.selected_network.is_none() {
                    self.error = Some("Please select a network".to_string());
                    return Task::none();
                }

                // Liquid native is handled by the caller — should not reach here
                // if is_liquid_native() was checked.
                self.phase = SendPhase::AmountInput;
                Task::none()
            }

            SideshiftSendMessage::AmountInput(v) => {
                self.amount_input = v.clone();
                self.shift_type = if v.trim().is_empty() {
                    SideshiftShiftType::Variable
                } else {
                    SideshiftShiftType::Fixed
                };
                Task::none()
            }

            SideshiftSendMessage::Generate => {
                let trimmed = self.amount_input.trim();
                if trimmed.is_empty() {
                    self.error = Some("Please enter an amount to send".to_string());
                    return Task::none();
                }
                let base_units = match parse_asset_to_minor_units(trimmed, USDT_PRECISION) {
                    Some(v) => v,
                    None => {
                        self.error = Some("Please enter a valid amount".to_string());
                        return Task::none();
                    }
                };
                let min_base = 5 * 10_u64.pow(USDT_PRECISION as u32);
                if base_units < min_base {
                    self.error = Some("Minimum amount is 5 USDt".to_string());
                    return Task::none();
                }
                if base_units > usdt_balance {
                    self.error = Some("Insufficient USDt balance".to_string());
                    return Task::none();
                }
                self.loading = true;
                self.error = None;
                self.phase = SendPhase::FetchingAffiliate;
                self.fetch_affiliate_id()
            }

            SideshiftSendMessage::AffiliateFetched(result) => {
                if self.phase != SendPhase::FetchingAffiliate {
                    return Task::none();
                }
                match result {
                    Ok(id) => {
                        self.affiliate_id = Some(id.clone());
                        if self.shift_type == SideshiftShiftType::Fixed
                            && !self.amount_input.trim().is_empty()
                        {
                            self.phase = SendPhase::FetchingQuote;
                            self.fetch_quote(id)
                        } else {
                            self.phase = SendPhase::CreatingShift;
                            self.create_shift(id, None)
                        }
                    }
                    Err(e) => {
                        self.loading = false;
                        self.phase = SendPhase::Failed;
                        self.error = Some(coincube_ui::localized_format!(
                            "Failed to fetch SideShift config: {}",
                            "No se pudo obtener la configuración de SideShift: {}",
                            e
                        ));
                        Task::none()
                    }
                }
            }

            SideshiftSendMessage::QuoteFetched(result) => {
                if self.phase != SendPhase::FetchingQuote {
                    return Task::none();
                }
                match result {
                    Ok(quote) => {
                        let affiliate_id = self.affiliate_id.clone().unwrap_or_default();
                        self.quote = Some(quote.clone());
                        self.phase = SendPhase::CreatingShift;
                        self.create_shift(&affiliate_id, Some(quote))
                    }
                    Err(e) => {
                        self.loading = false;
                        self.phase = SendPhase::Failed;
                        self.error = Some(coincube_ui::localized_format!(
                            "Quote failed: {}",
                            "Error en la cotización: {}",
                            e
                        ));
                        Task::none()
                    }
                }
            }

            SideshiftSendMessage::ShiftCreated(result) => {
                if self.phase != SendPhase::CreatingShift {
                    return Task::none();
                }
                self.loading = false;
                match result {
                    Ok(shift) => {
                        self.shift = Some(shift.clone());
                        self.phase = SendPhase::Review;
                    }
                    Err(e) => {
                        self.phase = SendPhase::Failed;
                        self.error = Some(coincube_ui::localized_format!(
                            "Failed to create shift: {}",
                            "No se pudo crear el intercambio: {}",
                            e
                        ));
                    }
                }
                Task::none()
            }

            SideshiftSendMessage::ConfirmSend => {
                if self.phase != SendPhase::Review {
                    return Task::none();
                }
                let Some(shift) = &self.shift else {
                    return Task::none();
                };
                let deposit_address = shift.deposit_address.clone();
                let breez = breez_client.clone();
                let network = breez.network();
                let amount_input = shift
                    .deposit_amount
                    .clone()
                    .unwrap_or_else(|| self.amount_input.clone());

                self.phase = SendPhase::Sending;
                self.error = None;

                Task::perform(
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
                )
            }

            SideshiftSendMessage::PaymentPrepared(prepare_response) => {
                if self.phase != SendPhase::Sending {
                    return Task::none();
                }
                let breez = breez_client.clone();
                let prepare = prepare_response.clone();
                Task::perform(
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
                )
            }

            SideshiftSendMessage::PaymentSent => {
                self.phase = SendPhase::Sent;
                // The Liquid payment succeeded locally; only a SideShift poll
                // can confirm that its deposit has been detected.
                self.shift_status = None;
                Task::none()
            }

            SideshiftSendMessage::PaymentFailed(e) => {
                self.phase = SendPhase::Failed;
                self.error = Some(coincube_ui::localized_format!(
                    "Payment failed: {}",
                    "El pago falló: {}",
                    e
                ));
                Task::none()
            }

            SideshiftSendMessage::PollStatus => self.poll_shift_status(),

            SideshiftSendMessage::StatusUpdated(shift_id, result) => {
                if self
                    .shift
                    .as_ref()
                    .is_none_or(|shift| &shift.id != shift_id)
                {
                    return Task::none();
                }
                if let Ok(status) = result {
                    let kind = ShiftStatusKind::from(status.status.as_str());
                    self.confirmed_settle_amount = if kind == ShiftStatusKind::Settled {
                        status.settle_amount.clone()
                    } else {
                        None
                    };
                    self.shift_status = Some(kind);
                }
                Task::none()
            }

            SideshiftSendMessage::Back => {
                self.error = None;
                self.loading = false;
                match self.phase {
                    SendPhase::NetworkDisambiguation => {
                        self.phase = SendPhase::AddressInput;
                    }
                    SendPhase::AmountInput => {
                        self.phase = SendPhase::AddressInput;
                    }
                    SendPhase::Review => {
                        self.shift = None;
                        self.quote = None;
                        self.affiliate_id = None;
                        self.phase = SendPhase::AmountInput;
                    }
                    _ => {
                        self.reset();
                    }
                }
                Task::none()
            }

            SideshiftSendMessage::Reset => {
                self.reset();
                Task::none()
            }

            SideshiftSendMessage::Error(e) => {
                self.error = Some(e.clone());
                self.loading = false;
                Task::none()
            }

            SideshiftSendMessage::Copy => {
                if let Some(shift) = &self.shift {
                    clipboard::write(shift.id.clone())
                } else {
                    Task::none()
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Subscription
    // -----------------------------------------------------------------------

    pub fn subscription(&self) -> Subscription<Message> {
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
}
