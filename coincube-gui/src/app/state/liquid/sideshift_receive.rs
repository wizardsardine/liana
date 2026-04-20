use std::sync::Arc;
use std::time::Duration;

use coincube_ui::widget::Element;
use iced::{clipboard, widget::qr_code, Subscription, Task};

use crate::app::breez_liquid::assets::usdt_asset_id;
use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::view;
use crate::app::wallets::LiquidBackend;
use crate::services::coincube::CoincubeClient;
use crate::services::sideshift::{
    ShiftQuote, ShiftResponse, ShiftStatusKind, SideshiftClient, SideshiftNetwork,
};

use view::{SideshiftReceiveMessage, SideshiftShiftType};

// ---------------------------------------------------------------------------
// State machine phases
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceivePhase {
    /// Initial screen: user picks a network.
    NetworkSelection,
    /// External network: entering optional amount + fixed/variable choice.
    ExternalSetup,
    /// Fetching affiliate ID from backend.
    FetchingAffiliate,
    /// Requesting a fixed-rate quote from SideShift.
    FetchingQuote,
    /// Creating the shift on SideShift.
    CreatingShift,
    /// Shift is active — deposit address is ready, polling status.
    Active,
    /// Terminal error state.
    Failed,
}

// ---------------------------------------------------------------------------
// SideshiftReceiveFlow
// ---------------------------------------------------------------------------

pub struct SideshiftReceiveFlow {
    breez_client: Arc<LiquidBackend>,
    coincube_client: CoincubeClient,
    sideshift_client: SideshiftClient,

    phase: ReceivePhase,
    selected_network: SideshiftNetwork,
    shift_type: SideshiftShiftType,
    amount_input: String,

    affiliate_id: Option<String>,
    quote: Option<ShiftQuote>,
    shift: Option<ShiftResponse>,
    shift_status: Option<ShiftStatusKind>,
    qr_data: Option<qr_code::Data>,

    loading: bool,
    error: Option<String>,
}

impl SideshiftReceiveFlow {
    pub fn new(breez_client: Arc<LiquidBackend>) -> Self {
        Self {
            breez_client,
            coincube_client: CoincubeClient::new(),
            sideshift_client: SideshiftClient::new(),
            phase: ReceivePhase::NetworkSelection,
            selected_network: SideshiftNetwork::Liquid,
            shift_type: SideshiftShiftType::Variable,
            amount_input: String::new(),
            affiliate_id: None,
            quote: None,
            shift: None,
            shift_status: None,
            qr_data: None,
            loading: false,
            error: None,
        }
    }

    pub fn phase(&self) -> &ReceivePhase {
        &self.phase
    }

    pub fn selected_network(&self) -> &SideshiftNetwork {
        &self.selected_network
    }

    pub fn shift_type(&self) -> &SideshiftShiftType {
        &self.shift_type
    }

    pub fn amount_input(&self) -> &str {
        &self.amount_input
    }

    pub fn shift(&self) -> Option<&ShiftResponse> {
        self.shift.as_ref()
    }

    pub fn qr_data(&self) -> Option<&qr_code::Data> {
        self.qr_data.as_ref()
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

    /// Returns `true` when the user selected the native Liquid network.
    pub fn is_liquid_native(&self) -> bool {
        self.selected_network == SideshiftNetwork::Liquid
    }

    pub fn reset(&mut self) {
        self.phase = ReceivePhase::NetworkSelection;
        self.selected_network = SideshiftNetwork::Liquid;
        self.shift_type = SideshiftShiftType::Variable;
        self.amount_input.clear();
        self.affiliate_id = None;
        self.quote = None;
        self.shift = None;
        self.shift_status = None;
        self.qr_data = None;
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
                Message::View(view::Message::SideshiftReceive(
                    SideshiftReceiveMessage::AffiliateFetched(result),
                ))
            },
        )
    }

    fn fetch_quote(&self, affiliate_id: &str) -> Task<Message> {
        let client = self.sideshift_client.clone();
        let deposit_network = self.selected_network.network_slug().to_string();
        let affiliate_id = affiliate_id.to_string();
        let settle_amount = if self.amount_input.trim().is_empty() {
            None
        } else {
            Some(self.amount_input.trim().to_string())
        };

        Task::perform(
            async move {
                client
                    .get_quote(
                        &deposit_network,
                        "liquid",
                        settle_amount.as_deref(),
                        None,
                        &affiliate_id,
                    )
                    .await
            },
            |result| {
                Message::View(view::Message::SideshiftReceive(
                    SideshiftReceiveMessage::QuoteFetched(result),
                ))
            },
        )
    }

    fn create_shift(&self, affiliate_id: &str, quote: Option<&ShiftQuote>) -> Task<Message> {
        let client = self.sideshift_client.clone();
        let breez = self.breez_client.clone();
        let network = self.breez_client.network();
        let deposit_network = self.selected_network.network_slug().to_string();
        let affiliate_id = affiliate_id.to_string();
        let is_fixed = self.shift_type == SideshiftShiftType::Fixed && quote.is_some();
        let quote_id = quote.map(|q| q.id.clone());

        Task::perform(
            async move {
                let usdt_id = usdt_asset_id(network).ok_or("USDt not available on network")?;
                let destination = breez
                    .receive_usdt(usdt_id, None, 8)
                    .await
                    .map(|r| r.destination)
                    .map_err(|e| e.to_string())?;
                // Breez returns a BIP21 URI (e.g. "liquidnetwork:VJL…?assetid=…").
                // SideShift needs just the raw address.
                let liquid_address = destination
                    .split(':')
                    .next_back()
                    .unwrap_or(&destination)
                    .split('?')
                    .next()
                    .unwrap_or(&destination)
                    .to_string();

                if is_fixed {
                    let qid = quote_id.ok_or("Missing quote ID for fixed shift")?;
                    client
                        .create_fixed_shift(&qid, &liquid_address, &affiliate_id)
                        .await
                } else {
                    client
                        .create_variable_receive_shift(
                            &deposit_network,
                            &liquid_address,
                            &affiliate_id,
                        )
                        .await
                }
            },
            |result| {
                Message::View(view::Message::SideshiftReceive(
                    SideshiftReceiveMessage::ShiftCreated(result),
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
                    Message::View(view::Message::SideshiftReceive(
                        SideshiftReceiveMessage::StatusUpdated(result),
                    ))
                },
            )
        } else {
            Task::none()
        }
    }

    fn build_qr(&mut self, address: &str) {
        self.qr_data = qr_code::Data::new(address).ok();
    }

    // -----------------------------------------------------------------------
    // View
    // -----------------------------------------------------------------------

    pub fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let sideshift_view = view::liquid::sideshift_receive_view(
            &self.phase,
            &self.selected_network,
            &self.shift_type,
            &self.amount_input,
            self.shift.as_ref(),
            self.qr_data.as_ref(),
            self.shift_status.as_ref(),
            self.loading,
            self.error.as_deref(),
        );

        view::dashboard(
            menu,
            cache,
            sideshift_view.map(view::Message::SideshiftReceive),
        )
    }

    // -----------------------------------------------------------------------
    // Update
    // -----------------------------------------------------------------------

    /// Returns `Some(task)` if the message was handled, `None` if the message
    /// should be handled by the caller (e.g. `SelectNetwork(Liquid)` triggers
    /// native receive).
    pub fn update(&mut self, msg: &SideshiftReceiveMessage) -> Option<Task<Message>> {
        match msg {
            SideshiftReceiveMessage::SelectNetwork(network) => {
                self.selected_network = *network;
                self.error = None;
                if *network == SideshiftNetwork::Liquid {
                    // Signal to caller: switch to native Liquid receive
                    return None;
                }
                self.shift_type = SideshiftShiftType::Variable;
                self.phase = ReceivePhase::ExternalSetup;
                Some(Task::none())
            }

            SideshiftReceiveMessage::AmountInput(v) => {
                self.amount_input = v.clone();
                self.shift_type = if v.trim().is_empty() {
                    SideshiftShiftType::Variable
                } else {
                    SideshiftShiftType::Fixed
                };
                Some(Task::none())
            }

            SideshiftReceiveMessage::Generate => {
                if !self.amount_input.trim().is_empty() {
                    let amount = self
                        .amount_input
                        .trim()
                        .parse::<f64>()
                        .ok()
                        .filter(|a| a.is_finite());
                    match amount {
                        Some(a) if a >= 5.0 => {}
                        Some(_) => {
                            self.error = Some("Minimum amount is 5 USDt".to_string());
                            return Some(Task::none());
                        }
                        None => {
                            self.error = Some("Please enter a valid amount".to_string());
                            return Some(Task::none());
                        }
                    }
                }
                self.loading = true;
                self.error = None;
                self.phase = ReceivePhase::FetchingAffiliate;
                Some(self.fetch_affiliate_id())
            }

            SideshiftReceiveMessage::AffiliateFetched(result) => {
                if self.phase != ReceivePhase::FetchingAffiliate {
                    return Some(Task::none());
                }
                match result {
                    Ok(id) => {
                        self.affiliate_id = Some(id.clone());
                        if self.shift_type == SideshiftShiftType::Fixed
                            && !self.amount_input.trim().is_empty()
                        {
                            self.phase = ReceivePhase::FetchingQuote;
                            Some(self.fetch_quote(id))
                        } else {
                            self.phase = ReceivePhase::CreatingShift;
                            Some(self.create_shift(id, None))
                        }
                    }
                    Err(e) => {
                        self.loading = false;
                        self.phase = ReceivePhase::Failed;
                        self.error = Some(format!("Failed to fetch SideShift config: {}", e));
                        Some(Task::none())
                    }
                }
            }

            SideshiftReceiveMessage::QuoteFetched(result) => {
                if self.phase != ReceivePhase::FetchingQuote {
                    return Some(Task::none());
                }
                match result {
                    Ok(quote) => {
                        let affiliate_id = self.affiliate_id.clone().unwrap_or_default();
                        self.quote = Some(quote.clone());
                        self.phase = ReceivePhase::CreatingShift;
                        Some(self.create_shift(&affiliate_id, Some(quote)))
                    }
                    Err(e) => {
                        self.loading = false;
                        self.phase = ReceivePhase::Failed;
                        self.error = Some(format!("Quote failed: {}", e));
                        Some(Task::none())
                    }
                }
            }

            SideshiftReceiveMessage::ShiftCreated(result) => {
                if self.phase != ReceivePhase::CreatingShift {
                    return Some(Task::none());
                }
                self.loading = false;
                match result {
                    Ok(shift) => {
                        self.build_qr(&shift.deposit_address);
                        self.shift = Some(shift.clone());
                        self.shift_status = Some(ShiftStatusKind::Waiting);
                        self.phase = ReceivePhase::Active;
                    }
                    Err(e) => {
                        self.phase = ReceivePhase::Failed;
                        self.error = Some(format!("Failed to create shift: {}", e));
                    }
                }
                Some(Task::none())
            }

            SideshiftReceiveMessage::PollStatus => Some(self.poll_shift_status()),

            SideshiftReceiveMessage::StatusUpdated(result) => {
                if let Ok(status) = result {
                    let kind = ShiftStatusKind::from(status.status.as_str());
                    self.shift_status = Some(kind);
                }
                Some(Task::none())
            }

            SideshiftReceiveMessage::Copy => {
                if let Some(shift) = &self.shift {
                    let toast_task = Task::done(Message::View(view::Message::ShowToast(
                        log::Level::Info,
                        "Copied deposit address to clipboard".to_string(),
                    )));
                    Some(Task::batch([
                        clipboard::write(shift.deposit_address.clone()),
                        toast_task,
                    ]))
                } else {
                    Some(Task::none())
                }
            }

            SideshiftReceiveMessage::Back => {
                self.error = None;
                self.loading = false;
                match self.phase {
                    ReceivePhase::ExternalSetup => {
                        self.phase = ReceivePhase::NetworkSelection;
                    }
                    _ => {
                        self.reset();
                    }
                }
                Some(Task::none())
            }

            SideshiftReceiveMessage::Reset => {
                self.reset();
                Some(Task::none())
            }

            SideshiftReceiveMessage::Error(e) => {
                self.error = Some(e.clone());
                self.loading = false;
                Some(Task::none())
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
        if self.phase == ReceivePhase::Active && !is_terminal {
            iced::time::every(Duration::from_secs(10)).map(|_| {
                Message::View(view::Message::SideshiftReceive(
                    SideshiftReceiveMessage::PollStatus,
                ))
            })
        } else {
            Subscription::none()
        }
    }
}
