use std::sync::Arc;
use std::time::Duration;

use coincube_ui::widget::*;
use iced::{clipboard, widget::qr_code, Subscription, Task};

use crate::app::breez::assets::usdt_asset_id;
use crate::app::breez::BreezClient;
use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::liquid::receive::LiquidReceive;
use crate::app::state::State;
use crate::app::view;
use crate::app::view::ReceiveMethod;
use crate::app::wallet::Wallet;
use crate::daemon::Daemon;
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
    /// Native Liquid USDt — handled by the inner `LiquidReceive`.
    LiquidNative,
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
// UsdtReceive
// ---------------------------------------------------------------------------

pub struct UsdtReceive {
    breez_client: Arc<BreezClient>,
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

    /// Inner LiquidReceive used for the native Liquid USDt path.
    liquid_inner: LiquidReceive,
}

impl UsdtReceive {
    pub fn new(inner: LiquidReceive) -> Self {
        let breez_client = inner.breez_client_arc();
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
            liquid_inner: inner,
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn reset_shift(&mut self) {
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
}

// ---------------------------------------------------------------------------
// State impl
// ---------------------------------------------------------------------------

impl State for UsdtReceive {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        if self.phase == ReceivePhase::LiquidNative {
            use coincube_ui::{component::text::*, icon::previous_icon, theme};
            use iced::{
                widget::{Column, Row},
                Alignment, Length,
            };

            let back_btn: Element<'a, view::Message> = iced::widget::button(
                Row::new()
                    .spacing(5)
                    .align_y(Alignment::Center)
                    .push(previous_icon().style(theme::text::secondary))
                    .push(
                        iced::widget::text("Previous")
                            .size(P1_SIZE)
                            .style(theme::text::secondary),
                    ),
            )
            .on_press(view::Message::SideshiftReceive(
                view::SideshiftReceiveMessage::Reset,
            ))
            .style(theme::button::transparent)
            .into();

            let liquid_view = view::liquid::usdt_only_receive_view(
                self.liquid_inner.current_usdt_address(),
                self.liquid_inner.current_usdt_qr(),
                self.liquid_inner.is_loading(),
                self.liquid_inner.usdt_amount_input(),
                self.liquid_inner.current_error(),
            )
            .map(view::Message::LiquidReceive);

            let content = Column::new()
                .spacing(20)
                .push(back_btn)
                .push(liquid_view)
                .width(Length::Fill);

            return view::dashboard(menu, cache, content);
        }

        let sideshift_view = view::usdt::usdt_receive_view(
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

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        // Intercept Reset even when in LiquidNative to go back to network picker.
        if let Message::View(view::Message::SideshiftReceive(
            view::SideshiftReceiveMessage::Reset,
        )) = &message
        {
            self.reset_shift();
            return Task::none();
        }

        // Delegate to LiquidReceive when in native Liquid path.
        if self.phase == ReceivePhase::LiquidNative {
            return self.liquid_inner.update(daemon, cache, message);
        }

        if let Message::View(view::Message::SideshiftReceive(ref msg)) = message {
            match msg {
                SideshiftReceiveMessage::SelectNetwork(network) => {
                    self.selected_network = *network;
                    self.error = None;
                    if *network == SideshiftNetwork::Liquid {
                        self.phase = ReceivePhase::LiquidNative;
                        let reload_task = self.liquid_inner.reload(daemon, None);
                        let preset_task = Task::done(Message::View(view::Message::LiquidReceive(
                            view::LiquidReceiveMessage::ToggleMethod(ReceiveMethod::Usdt),
                        )));
                        return Task::batch(vec![reload_task, preset_task]);
                    }
                    // Determine initial shift type: default variable, but if amount
                    // is already filled it will switch to fixed on Generate.
                    self.shift_type = SideshiftShiftType::Variable;
                    self.phase = ReceivePhase::ExternalSetup;
                    return Task::none();
                }

                SideshiftReceiveMessage::ToggleShiftType(st) => {
                    self.shift_type = st.clone();
                    return Task::none();
                }

                SideshiftReceiveMessage::AmountInput(v) => {
                    self.amount_input = v.clone();
                    // Auto-switch shift type based on whether an amount is provided.
                    self.shift_type = if v.trim().is_empty() {
                        SideshiftShiftType::Variable
                    } else {
                        SideshiftShiftType::Fixed
                    };
                    return Task::none();
                }

                SideshiftReceiveMessage::Generate => {
                    // Validate minimum amount for fixed-rate shifts
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
                                return Task::none();
                            }
                            None => {
                                self.error = Some("Please enter a valid amount".to_string());
                                return Task::none();
                            }
                        }
                    }
                    self.loading = true;
                    self.error = None;
                    self.phase = ReceivePhase::FetchingAffiliate;
                    return self.fetch_affiliate_id();
                }

                SideshiftReceiveMessage::AffiliateFetched(result) => {
                    match result {
                        Ok(id) => {
                            self.affiliate_id = Some(id.clone());
                            if self.shift_type == SideshiftShiftType::Fixed
                                && !self.amount_input.trim().is_empty()
                            {
                                self.phase = ReceivePhase::FetchingQuote;
                                return self.fetch_quote(id);
                            } else {
                                self.phase = ReceivePhase::CreatingShift;
                                return self.create_shift(id, None);
                            }
                        }
                        Err(e) => {
                            self.loading = false;
                            self.phase = ReceivePhase::Failed;
                            self.error = Some(format!("Failed to fetch SideShift config: {}", e));
                        }
                    }
                    return Task::none();
                }

                SideshiftReceiveMessage::QuoteFetched(result) => {
                    match result {
                        Ok(quote) => {
                            let affiliate_id = self.affiliate_id.clone().unwrap_or_default();
                            self.quote = Some(quote.clone());
                            self.phase = ReceivePhase::CreatingShift;
                            return self.create_shift(&affiliate_id, Some(quote));
                        }
                        Err(e) => {
                            self.loading = false;
                            self.phase = ReceivePhase::Failed;
                            self.error = Some(format!("Quote failed: {}", e));
                        }
                    }
                    return Task::none();
                }

                SideshiftReceiveMessage::ShiftCreated(result) => {
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
                    return Task::none();
                }

                SideshiftReceiveMessage::PollStatus => {
                    return self.poll_shift_status();
                }

                SideshiftReceiveMessage::StatusUpdated(result) => {
                    if let Ok(status) = result {
                        let kind = ShiftStatusKind::from(status.status.as_str());
                        self.shift_status = Some(kind);
                    }
                    return Task::none();
                }

                SideshiftReceiveMessage::Copy => {
                    if let Some(shift) = &self.shift {
                        let toast_task = Task::done(Message::View(view::Message::ShowToast(
                            log::Level::Info,
                            "Copied deposit address to clipboard".to_string(),
                        )));
                        return Task::batch([
                            clipboard::write(shift.deposit_address.clone()),
                            toast_task,
                        ]);
                    }
                    return Task::none();
                }

                SideshiftReceiveMessage::Back => {
                    self.error = None;
                    self.loading = false;
                    match self.phase {
                        ReceivePhase::ExternalSetup => {
                            // Back to network picker, keep amount/shift_type
                            self.phase = ReceivePhase::NetworkSelection;
                        }
                        _ => {
                            // From Active or Failed, full reset is safest
                            self.reset_shift();
                        }
                    }
                    return Task::none();
                }

                SideshiftReceiveMessage::Reset => {
                    self.reset_shift();
                    return Task::none();
                }

                SideshiftReceiveMessage::Error(e) => {
                    self.error = Some(e.clone());
                    self.loading = false;
                    return Task::none();
                }
            }
        }

        // Always forward LiquidReceive messages to inner state (e.g. DataLoaded for balance).
        if matches!(message, Message::View(view::Message::LiquidReceive(_))) {
            return self.liquid_inner.update(daemon, cache, message);
        }

        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.phase == ReceivePhase::LiquidNative {
            return self.liquid_inner.subscription();
        }
        let is_terminal = self
            .shift_status
            .as_ref()
            .is_some_and(ShiftStatusKind::is_terminal);
        if self.phase == ReceivePhase::Active && !is_terminal {
            // Poll shift status every 10 seconds.
            iced::time::every(Duration::from_secs(10)).map(|_| {
                Message::View(view::Message::SideshiftReceive(
                    SideshiftReceiveMessage::PollStatus,
                ))
            })
        } else {
            Subscription::none()
        }
    }

    fn close(&mut self) -> Task<Message> {
        self.liquid_inner.close()
    }

    fn interrupt(&mut self) {
        self.liquid_inner.interrupt();
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        // Always return to the network selection screen on navigation.
        self.reset_shift();
        self.liquid_inner.reload(daemon, wallet)
    }
}
