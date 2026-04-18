use std::convert::TryInto;
use std::sync::Arc;
use std::time::Duration;

use coincube_core::miniscript::bitcoin::{Amount, Denomination};
use coincube_ui::component::form;
use coincube_ui::widget::Element;
use iced::{clipboard, widget::qr_code, Subscription, Task};

use super::sideshift_receive::SideshiftReceiveFlow;
use crate::app::breez_liquid::assets::{
    format_usdt_display, parse_asset_to_minor_units, usdt_asset_id, USDT_PRECISION,
};
use crate::app::menu::LiquidSubMenu;
use crate::app::settings::unit::BitcoinDisplayUnit;
use crate::app::state::liquid::send::SendAsset;
use crate::app::state::redirect;
use crate::app::view::{LiquidReceiveMessage, ReceiveMethod, SenderNetwork};
use crate::app::wallets::{
    DomainPayment, DomainPaymentDetails, DomainPaymentStatus, LiquidBackend,
};

/// Return the base-unit USDt amount carried by `details`, if the payment is a
/// Liquid payment for the given `usdt_id`.
fn usdt_amount_minor(details: &DomainPaymentDetails, usdt_id: &str) -> Option<u64> {
    match details {
        DomainPaymentDetails::LiquidAsset {
            asset_id,
            asset_info,
            ..
        } if !usdt_id.is_empty() && asset_id == usdt_id => {
            asset_info.as_ref().map(|i| i.amount_minor)
        }
        _ => None,
    }
}
use crate::app::{cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::utils::format_time_ago;

pub struct LiquidReceive {
    breez_client: Arc<LiquidBackend>,
    receive_method: ReceiveMethod,
    sideshift_flow: Option<SideshiftReceiveFlow>,
    /// Asset the user wants to receive into their wallet.
    receive_asset: SendAsset,
    /// Network the sender is sending from.
    sender_network: SenderNetwork,
    /// Whether the "You Receive" picker modal is open.
    receive_picker_open: bool,
    /// Whether the "They Send" picker modal is open.
    sender_picker_open: bool,
    lightning_address: Option<String>,
    lightning_qr_data: Option<qr_code::Data>,
    liquid_address: Option<String>,
    liquid_qr_data: Option<qr_code::Data>,
    onchain_address: Option<String>,
    onchain_qr_data: Option<qr_code::Data>,
    usdt_address: Option<String>,
    usdt_qr_data: Option<qr_code::Data>,
    usdt_amount_input: form::Value<String>,
    loading: bool,
    amount_input: form::Value<String>,
    description_input: String,
    lightning_receive_limits: Option<(u64, u64)>, // (min_sat, max_sat)
    onchain_receive_limits: Option<(u64, u64)>,   // (min_sat, max_sat)
    error: Option<String>,
    btc_balance: Amount,
    usdt_balance: u64,
    recent_transaction: Vec<view::liquid::RecentTransaction>,
    recent_payments: Vec<DomainPayment>,
    show_qr_modal: bool,
    /// Show the "Payment received!" celebration screen.
    show_received_celebration: bool,
    received_amount_display: String,
    received_quote: coincube_ui::component::quote_display::Quote,
    received_image_handle: iced::widget::image::Handle,
}

impl LiquidReceive {
    /// Returns a clone of the inner `Arc<LiquidBackend>`.
    pub fn breez_client_arc(&self) -> Arc<LiquidBackend> {
        self.breez_client.clone()
    }

    pub fn receive_asset(&self) -> SendAsset {
        self.receive_asset
    }

    pub fn sender_network(&self) -> SenderNetwork {
        self.sender_network
    }

    pub fn receive_picker_open(&self) -> bool {
        self.receive_picker_open
    }

    pub fn sender_picker_open(&self) -> bool {
        self.sender_picker_open
    }

    pub fn new(breez_client: Arc<LiquidBackend>) -> Self {
        Self {
            breez_client,
            receive_method: ReceiveMethod::Lightning,
            sideshift_flow: None,
            receive_asset: SendAsset::Lbtc,
            sender_network: SenderNetwork::Lightning,
            receive_picker_open: false,
            sender_picker_open: false,
            lightning_address: None,
            lightning_qr_data: None,
            liquid_address: None,
            liquid_qr_data: None,
            onchain_address: None,
            onchain_qr_data: None,
            usdt_address: None,
            usdt_qr_data: None,
            usdt_amount_input: form::Value::default(),
            loading: false,
            amount_input: form::Value::default(),
            description_input: String::new(),
            lightning_receive_limits: None,
            onchain_receive_limits: None,
            error: None,
            btc_balance: Amount::ZERO,
            usdt_balance: 0,
            recent_transaction: Vec::new(),
            recent_payments: Vec::new(),
            show_qr_modal: false,
            show_received_celebration: false,
            received_amount_display: String::new(),
            received_quote: coincube_ui::component::quote_display::random_quote(
                "transaction-received",
            ),
            received_image_handle: coincube_ui::component::quote_display::image_handle_for_context(
                "transaction-received",
            ),
        }
    }

    async fn generate_lightning_invoice(
        client: Arc<LiquidBackend>,
        amount: Amount,
        description: Option<String>,
    ) -> Result<String, String> {
        let response = client
            .receive_invoice(Some(amount), description)
            .await
            .map_err(|e| e.to_string())?;

        Ok(response.destination)
    }

    async fn generate_onchain_address(client: Arc<LiquidBackend>) -> Result<String, String> {
        let response = client
            .receive_onchain(None)
            .await
            .map_err(|e| e.to_string())?;

        Ok(response.destination)
    }

    async fn generate_liquid_address(client: Arc<LiquidBackend>) -> Result<String, String> {
        let response = client.receive_liquid().await.map_err(|e| e.to_string())?;

        Ok(response.destination)
    }
}

impl State for LiquidReceive {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        // Delegate to SideShift flow when active
        if let Some(sideshift) = &self.sideshift_flow {
            return sideshift.view(menu, cache);
        }

        // Show celebration screen when a new payment is received
        if self.show_received_celebration {
            let celebration = view::liquid::received_celebration_page(
                &self.received_amount_display,
                &self.received_quote,
                &self.received_image_handle,
            )
            .map(view::Message::LiquidReceive);
            return view::dashboard(menu, cache, celebration);
        }

        let receive_view = view::liquid::liquid_receive_view(
            &self.receive_method,
            self.current_address(),
            self.current_qr_data(),
            self.loading,
            &self.amount_input,
            &self.usdt_amount_input,
            &self.description_input,
            cache.bitcoin_unit,
            self.error.as_ref(),
            self.lightning_receive_limits,
            self.onchain_receive_limits,
            self.receive_asset,
            self.sender_network,
            &self.recent_transaction,
            self.btc_balance,
            self.usdt_balance,
            cache.show_direction_badges,
        )
        .map(view::Message::LiquidReceive);

        let content = view::dashboard(menu, cache, receive_view);

        // Show picker modals if open
        if self.receive_picker_open {
            let modal_content = view::liquid::receive_asset_picker_modal(self.receive_asset)
                .map(view::Message::LiquidReceive);
            return coincube_ui::widget::modal::Modal::new(content, modal_content)
                .on_blur(Some(view::Message::LiquidReceive(
                    LiquidReceiveMessage::ClosePicker,
                )))
                .into();
        }

        if self.sender_picker_open {
            let modal_content =
                view::liquid::sender_network_picker_modal(self.receive_asset, self.sender_network)
                    .map(view::Message::LiquidReceive);
            return coincube_ui::widget::modal::Modal::new(content, modal_content)
                .on_blur(Some(view::Message::LiquidReceive(
                    LiquidReceiveMessage::ClosePicker,
                )))
                .into();
        }

        if self.show_qr_modal {
            if let (Some(qr), Some(addr)) = (self.current_qr_data(), self.current_address()) {
                let modal_content = view::liquid::qr_modal(qr, addr, &self.receive_method)
                    .map(view::Message::LiquidReceive);
                return coincube_ui::widget::modal::Modal::new(content, modal_content)
                    .on_blur(Some(view::Message::LiquidReceive(
                        LiquidReceiveMessage::CloseQrCode,
                    )))
                    .into();
            }
        }

        content
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        // Handle SideShift receive messages when flow is active
        if let Message::View(view::Message::SideshiftReceive(ref msg)) = message {
            if let Some(sideshift) = &mut self.sideshift_flow {
                match sideshift.update(msg) {
                    Some(task) => return task,
                    None => {
                        // SelectNetwork(Liquid) — switch to native Liquid USDt receive
                        self.sideshift_flow = None;
                        self.receive_method = ReceiveMethod::Usdt;
                        self.sender_network = SenderNetwork::Liquid;
                        return self.fetch_limits();
                    }
                }
            }
            return Task::none();
        }

        // When SideShift flow is active, ignore other messages
        if self.sideshift_flow.is_some() {
            return Task::none();
        }

        if let Message::View(view::Message::LiquidReceive(msg)) = message {
            match msg {
                LiquidReceiveMessage::ToggleMethod(method) => {
                    if self.receive_method != method {
                        self.receive_method = method.clone();
                        self.error = None;
                        self.show_qr_modal = false;
                    }
                    return self.fetch_limits();
                }
                LiquidReceiveMessage::ShowQrCode => {
                    self.show_qr_modal = true;
                }
                LiquidReceiveMessage::CloseQrCode => {
                    self.show_qr_modal = false;
                }
                LiquidReceiveMessage::DismissCelebration => {
                    self.show_received_celebration = false;
                }
                LiquidReceiveMessage::Copy => {
                    if let Some(address) = self.current_address() {
                        // Clean up address for clipboard
                        let address_copy = if self.receive_method == ReceiveMethod::OnChain {
                            // Strip "bitcoin:" prefix and query parameters for on-chain addresses
                            let addr = address.strip_prefix("bitcoin:").unwrap_or(address);
                            addr.split('?').next().unwrap_or(addr).to_string()
                        } else {
                            address.to_string()
                        };

                        let message = match self.receive_method {
                            ReceiveMethod::Lightning => {
                                "Copied Lightning Address to clipboard".to_string()
                            }
                            ReceiveMethod::Liquid => {
                                "Copied Liquid Address to clipboard".to_string()
                            }
                            ReceiveMethod::OnChain => {
                                "Copied Bitcoin Address to clipboard".to_string()
                            }
                            ReceiveMethod::Usdt => "Copied USDt Address to clipboard".to_string(),
                        };

                        // Use global toast overlay
                        let toast_task = Task::done(Message::View(view::Message::ShowToast(
                            log::Level::Info,
                            message,
                        )));

                        return Task::batch([clipboard::write(address_copy), toast_task]);
                    }
                    return Task::none();
                }
                LiquidReceiveMessage::AmountInput(value) => {
                    self.amount_input.value = value;
                    if self.receive_method == ReceiveMethod::Lightning {
                        match self.parse_amount(cache.bitcoin_unit) {
                            Some(amount) => {
                                if let Some((min_sat, max_sat)) = self.lightning_receive_limits {
                                    let min_sat = Amount::from_sat(min_sat);
                                    let max_sat = Amount::from_sat(max_sat);
                                    if amount < min_sat {
                                        self.amount_input.valid = false;
                                        self.amount_input.warning =
                                            Some("Amount below minimum limits");
                                    } else if amount > max_sat {
                                        self.amount_input.valid = false;
                                        self.amount_input.warning =
                                            Some("Amount above maximum limits");
                                    } else {
                                        self.amount_input.valid = true;
                                        self.amount_input.warning = None;
                                    }
                                } else {
                                    self.amount_input.valid = true;
                                    self.amount_input.warning = None;
                                }
                            }
                            None => {
                                // Distinguish empty input from malformed input
                                if self.amount_input.value.trim().is_empty() {
                                    self.amount_input.valid = true;
                                    self.amount_input.warning = None;
                                } else {
                                    self.amount_input.valid = false;
                                    self.amount_input.warning = Some("Invalid amount format");
                                }
                            }
                        }
                        self.lightning_address = None;
                        self.lightning_qr_data = None;
                    }
                    return Task::none();
                }
                LiquidReceiveMessage::DescriptionInput(value) => {
                    self.description_input = value;
                    // Clear current Lightning address so user knows they need to regenerate
                    if self.receive_method == ReceiveMethod::Lightning {
                        self.lightning_address = None;
                        self.lightning_qr_data = None;
                    }
                    return Task::none();
                }
                LiquidReceiveMessage::GenerateAddress => {
                    return match self.receive_method {
                        ReceiveMethod::Lightning => self.generate_lightning(cache.bitcoin_unit),
                        ReceiveMethod::Liquid => self.generate_liquid(),
                        ReceiveMethod::OnChain => self.generate_onchain(),
                        ReceiveMethod::Usdt => self.generate_usdt(),
                    };
                }
                LiquidReceiveMessage::UsdtAmountInput(value) => {
                    self.usdt_amount_input.value = value;
                    self.usdt_amount_input.valid = self.parse_usdt_amount().is_some()
                        || self.usdt_amount_input.value.trim().is_empty();
                    self.usdt_address = None;
                    self.usdt_qr_data = None;
                    return Task::none();
                }
                LiquidReceiveMessage::AddressGenerated(method, result) => {
                    self.loading = false;
                    match result {
                        Ok(address) => {
                            // Always store the address first
                            match method {
                                ReceiveMethod::Lightning => {
                                    self.lightning_address = Some(address.clone());
                                }
                                ReceiveMethod::Liquid => {
                                    self.liquid_address = Some(address.clone());
                                }
                                ReceiveMethod::OnChain => {
                                    self.onchain_address = Some(address.clone());
                                }
                                ReceiveMethod::Usdt => {
                                    self.usdt_address = Some(address.clone());
                                }
                            }

                            // Attempt QR generation, but keep address even if QR fails
                            match qr_code::Data::new(&address) {
                                Ok(qr_data) => match method {
                                    ReceiveMethod::Lightning => {
                                        self.lightning_qr_data = Some(qr_data);
                                    }
                                    ReceiveMethod::Liquid => {
                                        self.liquid_qr_data = Some(qr_data);
                                    }
                                    ReceiveMethod::OnChain => {
                                        self.onchain_qr_data = Some(qr_data);
                                    }
                                    ReceiveMethod::Usdt => {
                                        self.usdt_qr_data = Some(qr_data);
                                    }
                                },
                                Err(_) => {
                                    // QR generation failed, but address is still stored
                                    match method {
                                        ReceiveMethod::Lightning => {
                                            self.lightning_qr_data = None;
                                        }
                                        ReceiveMethod::Liquid => {
                                            self.liquid_qr_data = None;
                                        }
                                        ReceiveMethod::OnChain => {
                                            self.onchain_qr_data = None;
                                        }
                                        ReceiveMethod::Usdt => {
                                            self.usdt_qr_data = None;
                                        }
                                    }
                                }
                            }

                            // Clear inputs after successful address generation
                            self.amount_input = form::Value::default();
                            self.usdt_amount_input = form::Value::default();
                            self.description_input.clear();
                        }
                        Err(e) => {
                            let err_msg = e.to_string();
                            self.error = Some(err_msg.clone());
                            match method {
                                ReceiveMethod::Lightning => {
                                    self.lightning_address = None;
                                    self.lightning_qr_data = None;
                                }
                                ReceiveMethod::Liquid => {
                                    self.liquid_address = None;
                                    self.liquid_qr_data = None;
                                }
                                ReceiveMethod::OnChain => {
                                    self.onchain_address = None;
                                    self.onchain_qr_data = None;
                                }
                                ReceiveMethod::Usdt => {
                                    self.usdt_address = None;
                                    self.usdt_qr_data = None;
                                }
                            }
                            return Task::done(Message::View(view::Message::ShowError(err_msg)));
                        }
                    }
                    return Task::none();
                }

                LiquidReceiveMessage::Error(err) => {
                    self.error = Some(err.to_string());
                    return Task::perform(
                        async {
                            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                        },
                        |_| {
                            Message::View(view::Message::LiquidReceive(
                                view::LiquidReceiveMessage::ClearError,
                            ))
                        },
                    );
                }

                LiquidReceiveMessage::ClearError => {
                    self.error = None;
                }

                LiquidReceiveMessage::LightningLimitsFetched { min_sat, max_sat } => {
                    self.lightning_receive_limits = Some((min_sat, max_sat));
                    // Re-validate current amount input against new limits
                    if self.receive_method == ReceiveMethod::Lightning
                        && !self.amount_input.value.is_empty()
                    {
                        if let Some(amount) = self.parse_amount(cache.bitcoin_unit) {
                            let min_amount = Amount::from_sat(min_sat);
                            let max_amount = Amount::from_sat(max_sat);
                            if amount < min_amount {
                                self.amount_input.valid = false;
                                self.amount_input.warning = Some("Amount below minimum limits");
                            } else if amount > max_amount {
                                self.amount_input.valid = false;
                                self.amount_input.warning = Some("Amount above maximum limits");
                            } else {
                                self.amount_input.valid = true;
                                self.amount_input.warning = None;
                            }
                        }
                    }
                }
                LiquidReceiveMessage::OnChainLimitsFetched { min_sat, max_sat } => {
                    self.onchain_receive_limits = Some((min_sat, max_sat));
                }
                LiquidReceiveMessage::OpenReceivePicker => {
                    self.receive_picker_open = true;
                    self.sender_picker_open = false;
                    return Task::none();
                }
                LiquidReceiveMessage::OpenSenderPicker => {
                    self.sender_picker_open = true;
                    self.receive_picker_open = false;
                    return Task::none();
                }
                LiquidReceiveMessage::ClosePicker => {
                    self.receive_picker_open = false;
                    self.sender_picker_open = false;
                    return Task::none();
                }
                LiquidReceiveMessage::SetReceiveAsset(asset) => {
                    self.receive_picker_open = false;
                    if self.receive_asset != asset {
                        self.receive_asset = asset;
                        self.sideshift_flow = None;
                        self.error = None;
                        // Reset to default sender network for the new asset
                        match asset {
                            SendAsset::Lbtc => {
                                self.sender_network = SenderNetwork::Lightning;
                                self.receive_method = ReceiveMethod::Lightning;
                            }
                            SendAsset::Usdt => {
                                self.sender_network = SenderNetwork::Liquid;
                                self.receive_method = ReceiveMethod::Usdt;
                            }
                        }
                        self.recent_transaction.clear();
                        self.recent_payments.clear();
                        return Task::batch(vec![
                            self.fetch_limits(),
                            self.load_recent_transactions(),
                        ]);
                    }
                    return Task::none();
                }
                LiquidReceiveMessage::SetSenderNetwork(network) => {
                    self.sender_picker_open = false;
                    self.sender_network = network;
                    self.sideshift_flow = None;
                    self.error = None;

                    // Map sender network to internal ReceiveMethod or SideShift flow
                    match network {
                        SenderNetwork::Lightning => {
                            self.receive_method = ReceiveMethod::Lightning;
                        }
                        SenderNetwork::Liquid => {
                            if self.receive_asset == SendAsset::Usdt {
                                self.receive_method = ReceiveMethod::Usdt;
                            } else {
                                self.receive_method = ReceiveMethod::Liquid;
                            }
                        }
                        SenderNetwork::Bitcoin => {
                            self.receive_method = ReceiveMethod::OnChain;
                        }
                        _ if network.is_sideshift() => {
                            // Activate SideShift flow with the selected network
                            let flow = SideshiftReceiveFlow::new(self.breez_client.clone());
                            if let Some(ss_net) = network.to_sideshift_network() {
                                self.sideshift_flow = Some(flow);
                                // Dispatch SelectNetwork to the SideShift flow
                                return Task::done(Message::View(view::Message::SideshiftReceive(
                                    view::SideshiftReceiveMessage::SelectNetwork(ss_net),
                                )));
                            }
                        }
                        _ => {}
                    }
                    return self.fetch_limits();
                }
                LiquidReceiveMessage::DataLoaded {
                    btc_balance,
                    usdt_balance,
                    recent_payment,
                } => {
                    self.btc_balance = btc_balance;
                    self.usdt_balance = usdt_balance;

                    let usdt_id = usdt_asset_id(self.breez_client.network()).unwrap_or("");
                    let receive_usdt = self.receive_asset == SendAsset::Usdt;

                    // Filter payments by receive asset, matching Send behavior
                    let filtered: Vec<DomainPayment> = recent_payment
                        .into_iter()
                        .filter(|p| {
                            let is_usdt = usdt_amount_minor(&p.details, usdt_id).is_some();
                            if receive_usdt {
                                is_usdt
                            } else {
                                !is_usdt
                            }
                        })
                        .take(5)
                        .collect();
                    // Detect new incoming payment by scanning the unseen prefix
                    // of filtered (all items newer than the previous head).
                    if !self.recent_payments.is_empty() {
                        let prev_head_tx_id = self.recent_payments.first().map(|p| &p.tx_id);
                        // Scan filtered until we hit the previous head (unseen prefix)
                        for payment in &filtered {
                            // Stop at the previous head — everything after is already known
                            if Some(&payment.tx_id) == prev_head_tx_id {
                                break;
                            }
                            let is_receive = payment.is_incoming();
                            if is_receive && matches!(payment.status, DomainPaymentStatus::Complete)
                            {
                                let usdt_amount = usdt_amount_minor(&payment.details, usdt_id);
                                self.received_amount_display = if let Some(minor) = usdt_amount {
                                    format!("{} USDt", format_usdt_display(minor))
                                } else {
                                    use coincube_ui::component::amount::DisplayAmount;
                                    Amount::from_sat(payment.amount_sat)
                                        .to_formatted_string_with_unit(cache.bitcoin_unit)
                                };
                                self.received_quote =
                                    coincube_ui::component::quote_display::random_quote(
                                        "transaction-received",
                                    );
                                self.received_image_handle =
                                    coincube_ui::component::quote_display::image_handle_for_context(
                                        "transaction-received",
                                    );
                                self.show_received_celebration = true;
                                break; // Only fire for the first unseen receive
                            }
                        }
                    }
                    self.recent_payments = filtered.clone();

                    let fiat_converter: Option<view::FiatAmountConverter> =
                        cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

                    self.recent_transaction = filtered
                        .iter()
                        .map(|payment| {
                            let status = payment.status;
                            let time_ago = format_time_ago(payment.timestamp.into());
                            let usdt_amount = usdt_amount_minor(&payment.details, usdt_id);
                            let is_usdt = usdt_amount.is_some();

                            let amount = match usdt_amount {
                                Some(minor) => Amount::from_sat(minor),
                                None => Amount::from_sat(payment.amount_sat),
                            };

                            // Only compute fiat for BTC rows; USDt has its own display.
                            let fiat_amount = if is_usdt {
                                None
                            } else {
                                fiat_converter
                                    .as_ref()
                                    .map(|c: &view::FiatAmountConverter| c.convert(amount))
                            };

                            let (desc, usdt_display) = if let Some(minor) = usdt_amount {
                                (
                                    "USDt Transfer".to_owned(),
                                    Some(format!("{} USDt", format_usdt_display(minor))),
                                )
                            } else {
                                (payment.details.description().to_owned(), None)
                            };

                            let is_incoming = payment.is_incoming();
                            let details = payment.details.clone();
                            let fees_sat = Amount::from_sat(payment.fees_sat);
                            view::liquid::RecentTransaction {
                                description: desc,
                                time_ago,
                                amount,
                                fiat_amount,
                                is_incoming,
                                status,
                                details,
                                fees_sat,
                                usdt_display,
                            }
                        })
                        .collect();
                }
                LiquidReceiveMessage::SelectTransaction(idx) => {
                    if let Some(payment) = self.recent_payments.get(idx).cloned() {
                        return Task::batch(vec![
                            redirect(Menu::Liquid(LiquidSubMenu::Transactions(None))),
                            Task::done(Message::View(view::Message::PreselectPayment(payment))),
                        ]);
                    }
                }
                LiquidReceiveMessage::History => {
                    return redirect(Menu::Liquid(LiquidSubMenu::Transactions(None)));
                }
                LiquidReceiveMessage::RefreshRequested => {
                    return self.load_recent_transactions();
                }
            }
        }
        Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.sideshift_flow = None;
        self.receive_picker_open = false;
        self.sender_picker_open = false;
        // Trigger an SDK sync in the background so on-chain state is fresh.
        // When the sync completes the SDK fires SdkEvent::Synced automatically.
        let breez = self.breez_client.clone();
        Task::batch(vec![
            Task::perform(
                async move {
                    let _ = breez.sync().await;
                },
                |_| Message::CacheUpdated,
            ),
            self.fetch_limits(),
            self.load_recent_transactions(),
        ])
    }

    fn subscription(&self) -> Subscription<Message> {
        if let Some(sideshift) = &self.sideshift_flow {
            return sideshift.subscription();
        }
        if self.loading {
            iced::time::every(Duration::from_millis(50)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        }
    }
}

impl LiquidReceive {
    pub fn view_usdt_only<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> Element<'a, view::Message> {
        let receive_view = view::liquid::usdt_only_receive_view(
            self.usdt_address.as_ref(),
            self.usdt_qr_data.as_ref(),
            self.loading,
            &self.usdt_amount_input,
            self.error.as_ref(),
        )
        .map(view::Message::LiquidReceive);

        let content = view::dashboard(menu, cache, receive_view);

        // Use global toast overlay instead of local toast
        content
    }

    pub fn current_usdt_address(&self) -> Option<&String> {
        self.usdt_address.as_ref()
    }

    pub fn current_usdt_qr(&self) -> Option<&qr_code::Data> {
        self.usdt_qr_data.as_ref()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn usdt_amount_input(&self) -> &form::Value<String> {
        &self.usdt_amount_input
    }

    pub fn current_error(&self) -> Option<&String> {
        self.error.as_ref()
    }

    fn current_address(&self) -> Option<&String> {
        match self.receive_method {
            ReceiveMethod::Lightning => self.lightning_address.as_ref(),
            ReceiveMethod::Liquid => self.liquid_address.as_ref(),
            ReceiveMethod::OnChain => self.onchain_address.as_ref(),
            ReceiveMethod::Usdt => self.usdt_address.as_ref(),
        }
    }

    fn current_qr_data(&self) -> Option<&qr_code::Data> {
        match self.receive_method {
            ReceiveMethod::Lightning => self.lightning_qr_data.as_ref(),
            ReceiveMethod::Liquid => self.liquid_qr_data.as_ref(),
            ReceiveMethod::OnChain => self.onchain_qr_data.as_ref(),
            ReceiveMethod::Usdt => self.usdt_qr_data.as_ref(),
        }
    }

    fn generate_lightning(&mut self, bitcoin_unit: BitcoinDisplayUnit) -> Task<Message> {
        self.loading = true;

        let client = self.breez_client.clone();

        // Check for empty input first
        if self.amount_input.value.is_empty() {
            self.loading = false;
            return Task::done(Message::View(view::Message::LiquidReceive(
                LiquidReceiveMessage::Error("Please enter an amount".to_string()),
            )));
        }

        match self.parse_amount(bitcoin_unit) {
            Some(amount) => {
                // Guard: re-check limits before proceeding
                if let Some((min_sat, max_sat)) = self.lightning_receive_limits {
                    let min_amount = Amount::from_sat(min_sat);
                    let max_amount = Amount::from_sat(max_sat);
                    if amount < min_amount {
                        self.loading = false;
                        return Task::done(Message::View(view::Message::LiquidReceive(
                            LiquidReceiveMessage::Error(format!(
                                "Amount below minimum limit of {}",
                                min_amount
                            )),
                        )));
                    } else if amount > max_amount {
                        self.loading = false;
                        return Task::done(Message::View(view::Message::LiquidReceive(
                            LiquidReceiveMessage::Error(format!(
                                "Amount above maximum limit of {}",
                                max_amount
                            )),
                        )));
                    }
                }

                let description = if self.description_input.is_empty() {
                    None
                } else {
                    Some(self.description_input.clone())
                };

                Task::perform(
                    Self::generate_lightning_invoice(client, amount, description),
                    |result| {
                        Message::View(view::Message::LiquidReceive(
                            LiquidReceiveMessage::AddressGenerated(
                                ReceiveMethod::Lightning,
                                result,
                            ),
                        ))
                    },
                )
            }
            None => {
                self.loading = false;
                Task::done(Message::View(view::Message::LiquidReceive(
                    LiquidReceiveMessage::Error("Invalid amount format".to_string()),
                )))
            }
        }
    }

    fn generate_onchain(&mut self) -> Task<Message> {
        self.loading = true;

        let client = self.breez_client.clone();

        Task::perform(Self::generate_onchain_address(client), |result| {
            Message::View(view::Message::LiquidReceive(
                LiquidReceiveMessage::AddressGenerated(ReceiveMethod::OnChain, result),
            ))
        })
    }

    fn generate_liquid(&mut self) -> Task<Message> {
        self.loading = true;

        let client = self.breez_client.clone();

        Task::perform(Self::generate_liquid_address(client), |result| {
            Message::View(view::Message::LiquidReceive(
                LiquidReceiveMessage::AddressGenerated(ReceiveMethod::Liquid, result),
            ))
        })
    }

    fn generate_usdt(&mut self) -> Task<Message> {
        if !self.usdt_amount_input.valid {
            return Task::done(Message::View(view::Message::LiquidReceive(
                LiquidReceiveMessage::Error("Invalid USDt amount".to_string()),
            )));
        }

        let network = self.breez_client.network();
        let asset_id = match usdt_asset_id(network) {
            Some(id) => id.to_string(),
            None => {
                return Task::done(Message::View(view::Message::LiquidReceive(
                    LiquidReceiveMessage::Error(
                        "USDt is not available on this network".to_string(),
                    ),
                )));
            }
        };

        let amount = self.parse_usdt_amount();
        self.loading = true;
        let client = self.breez_client.clone();

        Task::perform(
            async move {
                client
                    .receive_usdt(&asset_id, amount, USDT_PRECISION)
                    .await
                    .map(|r| r.destination)
                    .map_err(|e| e.to_string())
            },
            |result| {
                Message::View(view::Message::LiquidReceive(
                    LiquidReceiveMessage::AddressGenerated(ReceiveMethod::Usdt, result),
                ))
            },
        )
    }

    /// Parse the USDt amount input (decimal USDt) into u64 base units.
    /// Returns `None` for empty or malformed input.
    fn parse_usdt_amount(&self) -> Option<u64> {
        let trimmed = self.usdt_amount_input.value.trim();
        if trimmed.is_empty() {
            return None;
        }
        parse_asset_to_minor_units(trimmed, USDT_PRECISION)
    }

    fn parse_amount(&self, bitcoin_unit: BitcoinDisplayUnit) -> Option<Amount> {
        let denomination = match bitcoin_unit {
            BitcoinDisplayUnit::BTC => Denomination::Bitcoin,
            BitcoinDisplayUnit::Sats => Denomination::Satoshi,
        };
        Amount::from_str_in(&self.amount_input.value, denomination).ok()
    }

    fn load_recent_transactions(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();
        Task::perform(
            async move {
                let info = breez_client.info().await;
                let payments = breez_client.list_payments(Some(20)).await;

                let btc_balance = info
                    .as_ref()
                    .map(|info| {
                        Amount::from_sat(
                            info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat,
                        )
                    })
                    .unwrap_or(Amount::ZERO);

                let usdt_id = usdt_asset_id(breez_client.network()).unwrap_or("");
                let usdt_balance = info
                    .as_ref()
                    .ok()
                    .and_then(|info| {
                        info.wallet_info.asset_balances.iter().find_map(|ab| {
                            if ab.asset_id == usdt_id {
                                Some(ab.balance_sat)
                            } else {
                                None
                            }
                        })
                    })
                    .unwrap_or(0);

                let error = match (&info, &payments) {
                    (Err(_), Err(_)) => Some("Couldn't fetch balance or transactions".to_string()),
                    (Err(_), _) => Some("Couldn't fetch account balance".to_string()),
                    (_, Err(_)) => Some("Couldn't fetch recent transactions".to_string()),
                    _ => None,
                };

                let payments = payments.unwrap_or_default();

                (btc_balance, usdt_balance, payments, error)
            },
            |(btc_balance, usdt_balance, recent_payment, error)| {
                if let Some(err) = error {
                    Message::View(view::Message::LiquidReceive(LiquidReceiveMessage::Error(
                        err,
                    )))
                } else {
                    Message::View(view::Message::LiquidReceive(
                        LiquidReceiveMessage::DataLoaded {
                            btc_balance,
                            usdt_balance,
                            recent_payment,
                        },
                    ))
                }
            },
        )
    }

    /// Fetch lightning + onchain receive limits from the SDK.
    ///
    /// Onchain limits are fetched eagerly (regardless of which tab is
    /// currently open) so that when the user switches to the Bitcoin tab they
    /// immediately see the min/max and the swap-receive warning copy. Without
    /// this, the limits are fetched lazily on tab open and the warning block
    /// would flash empty for a moment, or worse, the user could generate an
    /// address before seeing the constraints.
    ///
    /// Lightning limits are cheap and fetched alongside.
    fn fetch_limits(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();

        if self.lightning_receive_limits.is_none() {
            let breez_client = self.breez_client.clone();
            tasks.push(Task::perform(
                async move { breez_client.fetch_lightning_limits().await },
                |response| match response {
                    Ok(limits) => Message::View(view::Message::LiquidReceive(
                        LiquidReceiveMessage::LightningLimitsFetched {
                            min_sat: limits.receive.min_sat,
                            max_sat: limits.receive.max_sat,
                        },
                    )),
                    Err(error) => Message::View(view::Message::LiquidReceive(
                        LiquidReceiveMessage::Error(error.to_string()),
                    )),
                },
            ));
        }

        if self.onchain_receive_limits.is_none() {
            let breez_client = self.breez_client.clone();
            tasks.push(Task::perform(
                async move { breez_client.fetch_onchain_limits().await },
                |response| match response {
                    Ok(limits) => Message::View(view::Message::LiquidReceive(
                        LiquidReceiveMessage::OnChainLimitsFetched {
                            min_sat: limits.receive.min_sat,
                            max_sat: limits.receive.max_sat,
                        },
                    )),
                    Err(error) => Message::View(view::Message::LiquidReceive(
                        LiquidReceiveMessage::Error(error.to_string()),
                    )),
                },
            ));
        }

        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }
}
