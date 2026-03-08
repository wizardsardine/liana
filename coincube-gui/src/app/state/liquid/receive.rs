use std::sync::Arc;
use std::time::Duration;

use coincube_core::miniscript::bitcoin::{Amount, Denomination};
use coincube_ui::component::{form, toast};
use coincube_ui::widget::*;
use iced::{clipboard, widget::qr_code, Subscription, Task};

use crate::app::settings::unit::BitcoinDisplayUnit;
use crate::app::breez::assets::{usdt_asset_id, USDT_PRECISION};
use crate::app::view::{LiquidReceiveMessage, ReceiveMethod};
use crate::app::{breez::BreezClient, cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

pub struct LiquidReceive {
    breez_client: Arc<BreezClient>,
    receive_method: ReceiveMethod,
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
    toast: Option<String>,
    amount_input: form::Value<String>,
    description_input: String,
    lightning_receive_limits: Option<(u64, u64)>, // (min_sat, max_sat)
    onchain_receive_limits: Option<(u64, u64)>,   // (min_sat, max_sat)
    error: Option<String>,
}

impl LiquidReceive {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
            receive_method: ReceiveMethod::Lightning,
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
            toast: None,
            amount_input: form::Value::default(),
            description_input: String::new(),
            lightning_receive_limits: None,
            onchain_receive_limits: None,
            error: None,
        }
    }

    async fn generate_lightning_invoice(
        client: Arc<BreezClient>,
        amount: Amount,
        description: Option<String>,
    ) -> Result<String, String> {
        let response = client
            .receive_invoice(Some(amount), description)
            .await
            .map_err(|e| e.to_string())?;

        Ok(response.destination)
    }

    async fn generate_onchain_address(client: Arc<BreezClient>) -> Result<String, String> {
        let response = client
            .receive_onchain(None)
            .await
            .map_err(|e| e.to_string())?;

        Ok(response.destination)
    }

    async fn generate_liquid_address(client: Arc<BreezClient>) -> Result<String, String> {
        let response = client.receive_liquid().await.map_err(|e| e.to_string())?;

        Ok(response.destination)
    }
}

impl State for LiquidReceive {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
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
        )
        .map(view::Message::LiquidReceive);

        let content = view::dashboard(menu, cache, receive_view);

        // Add toast notification for clipboard copy
        let toasts = if let Some(message) = &self.toast {
            vec![view::simple_toast(message).into()]
        } else {
            vec![]
        };

        toast::Manager::new(content, toasts).into()
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::LiquidReceive(msg)) = message {
            match msg {
                LiquidReceiveMessage::ToggleMethod(method) => {
                    if self.receive_method != method {
                        self.receive_method = method.clone();
                        self.toast = None;
                        self.error = None;
                    }
                    return self.fetch_limits();
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

                        self.toast = Some(match self.receive_method {
                            ReceiveMethod::Lightning => {
                                "Copied Lightning Address to clipboard".to_string()
                            }
                            ReceiveMethod::Liquid => {
                                "Copied Liquid Address to clipboard".to_string()
                            }
                            ReceiveMethod::OnChain => {
                                "Copied Bitcoin Address to clipboard".to_string()
                            }
                            ReceiveMethod::Usdt => {
                                "Copied USDt Address to clipboard".to_string()
                            }
                        });

                        // Auto-dismiss toast after 3 seconds
                        let clear_toast_task = Task::future(async {
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            Message::View(view::Message::LiquidReceive(
                                LiquidReceiveMessage::ClearToast,
                            ))
                        });

                        return Task::batch([clipboard::write(address_copy), clear_toast_task]);
                    }
                    return Task::none();
                }
                LiquidReceiveMessage::ClearToast => {
                    self.toast = None;
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
            }
        }
        Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.fetch_limits()
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.loading {
            iced::time::every(Duration::from_millis(50)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        }
    }
}

impl LiquidReceive {
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
        let display: f64 = trimmed.parse().ok()?;
        if display < 0.0 {
            return None;
        }
        let base_units = (display * 10_u64.pow(USDT_PRECISION as u32) as f64).round() as u64;
        Some(base_units)
    }

    fn parse_amount(&self, bitcoin_unit: BitcoinDisplayUnit) -> Option<Amount> {
        let denomination = match bitcoin_unit {
            BitcoinDisplayUnit::BTC => Denomination::Bitcoin,
            BitcoinDisplayUnit::Sats => Denomination::Satoshi,
        };
        Amount::from_str_in(&self.amount_input.value, denomination).ok()
    }

    fn fetch_limits(&mut self) -> Task<Message> {
        if self.lightning_receive_limits.is_none()
            && matches!(self.receive_method, ReceiveMethod::Lightning)
        {
            let breez_client = self.breez_client.clone();
            Task::perform(
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
            )
        } else if self.onchain_receive_limits.is_none()
            && matches!(self.receive_method, ReceiveMethod::OnChain)
        {
            let breez_client = self.breez_client.clone();
            Task::perform(
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
            )
        } else {
            Task::none()
        }
    }
}
