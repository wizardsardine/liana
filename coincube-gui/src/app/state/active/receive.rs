use std::sync::Arc;

use coincube_ui::widget::*;
use iced::{clipboard, Task, widget::qr_code};

use crate::app::{breez::BreezClient, cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::app::view::{ActiveReceiveMessage, ReceiveMethod};
use crate::daemon::Daemon;

pub struct ActiveReceive {
    breez_client: Arc<BreezClient>,
    receive_method: ReceiveMethod,
    lightning_address: Option<String>,
    lightning_qr_data: Option<qr_code::Data>,
    onchain_address: Option<String>,
    onchain_qr_data: Option<qr_code::Data>,
    loading: bool,
    toast: Option<String>,
    amount_input: String,
    description_input: String,
}

impl ActiveReceive {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
            receive_method: ReceiveMethod::Lightning,
            lightning_address: None,
            lightning_qr_data: None,
            onchain_address: None,
            onchain_qr_data: None,
            loading: false,
            toast: None,
            amount_input: String::new(),
            description_input: String::new(),
        }
    }

    async fn generate_lightning_address(
        client: Arc<BreezClient>,
        amount_sat: Option<u64>,
        description: Option<String>,
    ) -> Result<String, String> {
        let response = client
            .receive_invoice(amount_sat, description)
            .await
            .map_err(|e| format!("Failed to generate Lightning invoice: {}", e))?;
        
        Ok(response.destination)
    }

    async fn generate_onchain_address(
        client: Arc<BreezClient>,
    ) -> Result<String, String> {
        let response = client
            .receive_onchain(None)
            .await
            .map_err(|e| format!("Failed to generate Bitcoin address: {}", e))?;
        
        Ok(response.destination)
    }
}

impl State for ActiveReceive {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let receive_view = view::active::active_receive_view(
            &self.receive_method,
            self.current_address(),
            self.current_qr_data(),
            self.loading,
            &self.amount_input,
            &self.description_input,
        )
        .map(view::Message::ActiveReceive);

        view::dashboard(menu, cache, None, receive_view)
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::ActiveReceive(msg)) = message {
            match msg {
                ActiveReceiveMessage::ToggleMethod(method) => {
                    if self.receive_method != method {
                        self.receive_method = method.clone();
                        self.toast = None;
                    }
                    return Task::none();
                }
                ActiveReceiveMessage::Copy => {
                    if let Some(address) = self.current_address() {
                        let address_copy = address.to_string();
                        self.toast = Some(match self.receive_method {
                            ReceiveMethod::Lightning => "Copied Lightning Address to clipboard".to_string(),
                            ReceiveMethod::OnChain => "Copied Bitcoin Address to clipboard".to_string(),
                        });
                        return clipboard::write(address_copy);
                    }
                    return Task::none();
                }
                ActiveReceiveMessage::AmountInput(value) => {
                    self.amount_input = value;
                    // Clear current Lightning address so user knows they need to regenerate
                    if self.receive_method == ReceiveMethod::Lightning {
                        self.lightning_address = None;
                        self.lightning_qr_data = None;
                    }
                    return Task::none();
                }
                ActiveReceiveMessage::DescriptionInput(value) => {
                    self.description_input = value;
                    // Clear current Lightning address so user knows they need to regenerate
                    if self.receive_method == ReceiveMethod::Lightning {
                        self.lightning_address = None;
                        self.lightning_qr_data = None;
                    }
                    return Task::none();
                }
                ActiveReceiveMessage::GenerateAddress => {
                    return match self.receive_method {
                        ReceiveMethod::Lightning => self.generate_lightning(),
                        ReceiveMethod::OnChain => self.generate_onchain(),
                    };
                }
                ActiveReceiveMessage::AddressGenerated(method, result) => {
                    self.loading = false;
                    match result {
                        Ok(address) => {
                            if let Ok(qr_data) = qr_code::Data::new(&address) {
                                match method {
                                    ReceiveMethod::Lightning => {
                                        self.lightning_address = Some(address);
                                        self.lightning_qr_data = Some(qr_data);
                                    }
                                    ReceiveMethod::OnChain => {
                                        self.onchain_address = Some(address);
                                        self.onchain_qr_data = Some(qr_data);
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            match method {
                                ReceiveMethod::Lightning => {
                                    self.lightning_address = None;
                                    self.lightning_qr_data = None;
                                }
                                ReceiveMethod::OnChain => {
                                    self.onchain_address = None;
                                    self.onchain_qr_data = None;
                                }
                            }
                        }
                    }
                    return Task::none();
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
        // Don't auto-generate on reload - let user click Generate button
        Task::none()
    }
}

impl ActiveReceive {
    fn current_address(&self) -> Option<&String> {
        match self.receive_method {
            ReceiveMethod::Lightning => self.lightning_address.as_ref(),
            ReceiveMethod::OnChain => self.onchain_address.as_ref(),
        }
    }

    fn current_qr_data(&self) -> Option<&qr_code::Data> {
        match self.receive_method {
            ReceiveMethod::Lightning => self.lightning_qr_data.as_ref(),
            ReceiveMethod::OnChain => self.onchain_qr_data.as_ref(),
        }
    }


    fn generate_lightning(&mut self) -> Task<Message> {
        self.loading = true;
        let client = self.breez_client.clone();
        
        let amount_sat = self.parse_amount();
        let description = if self.description_input.is_empty() {
            None
        } else {
            Some(self.description_input.clone())
        };
        
        Task::perform(
            Self::generate_lightning_address(client, amount_sat, description),
            |result| {
                Message::View(view::Message::ActiveReceive(
                    ActiveReceiveMessage::AddressGenerated(ReceiveMethod::Lightning, result)
                ))
            },
        )
    }

    fn generate_onchain(&mut self) -> Task<Message> {
        self.loading = true;
        let client = self.breez_client.clone();
        
        Task::perform(
            Self::generate_onchain_address(client),
            |result| {
                Message::View(view::Message::ActiveReceive(
                    ActiveReceiveMessage::AddressGenerated(ReceiveMethod::OnChain, result)
                ))
            },
        )
    }
    
    fn parse_amount(&self) -> Option<u64> {
        if self.amount_input.is_empty() {
            None
        } else {
            self.amount_input.parse::<u64>().ok()
        }
    }
}
