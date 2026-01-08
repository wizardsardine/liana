use std::collections::HashMap;
use std::convert::TryInto;
use std::str::FromStr;
use std::sync::Arc;

use breez_sdk_liquid::model::{
    PayOnchainRequest, PreparePayOnchainRequest, PreparePayOnchainResponse,
};
use coincube_core::miniscript::bitcoin::{bip32::ChildNumber, Address, Amount};
use coincube_ui::component::form;
use coincube_ui::widget::*;
use iced::Task;

use super::{Cache, Menu, State};
use crate::app::breez::BreezClient;
use crate::app::error::Error;
use crate::app::state::vault::label::LabelsEdited;
use crate::app::state::vault::receive::ShowQrCodeModal;
use crate::app::view::global_home::{GlobalViewConfig, HomeView, TransferDirection};
use crate::app::view::HomeMessage;
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::model::{CreateSpendResult, LabelItem, Labelled};
use crate::daemon::Daemon;
use crate::services::feeestimation::fee_estimation::FeeEstimator;

#[derive(Debug, Default)]
pub enum Modal {
    ShowQrCode(ShowQrCodeModal),
    #[default]
    None,
}

#[derive(Debug, Clone)]
pub struct ReceiveAddressInfo {
    pub address: Address,
    pub index: ChildNumber,
    pub labels: HashMap<String, String>,
}

impl Labelled for ReceiveAddressInfo {
    fn labelled(&self) -> Vec<LabelItem> {
        vec![LabelItem::Address(self.address.clone())]
    }

    fn labels(&mut self) -> &mut HashMap<String, String> {
        &mut self.labels
    }
}

#[derive(Debug)]
pub struct GlobalHome {
    breez_client: Arc<BreezClient>,
    active_balance: Amount,
    wallet: Option<Arc<Wallet>>,
    balance_masked: bool,
    transfer_direction: Option<TransferDirection>,
    current_view: HomeView,
    entered_amount: form::Value<String>,
    receive_address_info: Option<ReceiveAddressInfo>,
    labels_edited: LabelsEdited,
    address_expanded: bool,
    modal: Modal,
    empty_labels: HashMap<String, String>,
    warning: Option<Error>,
    onchain_send_limit: Option<(u64, u64)>,
    onchain_receive_limit: Option<(u64, u64)>,
    prepare_onchain_send_response: Option<PreparePayOnchainResponse>,
    is_sending: bool,
}

impl GlobalHome {
    pub fn new(wallet: Arc<Wallet>, breez_client: Arc<BreezClient>) -> Self {
        Self {
            wallet: Some(wallet),
            active_balance: Amount::ZERO,
            breez_client,
            balance_masked: false,
            transfer_direction: None,
            current_view: HomeView::default(),
            entered_amount: form::Value::default(),
            receive_address_info: None,
            labels_edited: LabelsEdited::default(),
            address_expanded: false,
            modal: Modal::default(),
            empty_labels: HashMap::default(),
            warning: None,
            onchain_send_limit: None,
            onchain_receive_limit: None,
            prepare_onchain_send_response: None,
            is_sending: false,
        }
    }

    pub fn new_without_wallet(breez_client: Arc<BreezClient>) -> Self {
        Self {
            wallet: None,
            active_balance: Amount::from_sat(90099),
            breez_client,
            balance_masked: false,
            transfer_direction: None,
            current_view: HomeView::default(),
            entered_amount: form::Value::default(),
            receive_address_info: None,
            labels_edited: LabelsEdited::default(),
            address_expanded: false,
            modal: Modal::default(),
            empty_labels: HashMap::default(),
            warning: None,
            onchain_send_limit: None,
            onchain_receive_limit: None,
            prepare_onchain_send_response: None,
            is_sending: false,
        }
    }
}

impl State for GlobalHome {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let vault_balance = cache
            .coins()
            .iter()
            .filter(|coin| coin.spend_info.is_none())
            .fold(Amount::from_sat(0), |acc, coin| acc + coin.amount);

        let active_balance = self.active_balance;

        // Fiat price is cube-level, not wallet-level, so get it directly from cache
        let fiat_converter: Option<view::FiatAmountConverter> =
            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

        let content = view::dashboard(
            menu,
            cache,
            None,
            view::global_home::global_home_view(GlobalViewConfig {
                active_balance,
                vault_balance,
                fiat_converter,
                balance_masked: self.balance_masked,
                has_vault: cache.has_vault,
                current_view: self.current_view,
                transfer_direction: self.transfer_direction,
                entered_amount: &self.entered_amount,
                receive_address: self.receive_address_info.as_ref().map(|info| &info.address),
                receive_index: self.receive_address_info.as_ref().map(|info| &info.index),
                labels: self
                    .receive_address_info
                    .as_ref()
                    .map_or(&self.empty_labels, |info| &info.labels),
                labels_editing: self.labels_edited.cache(),
                address_expanded: self.address_expanded,
                warning: self.warning.as_ref(),
                bitcoin_unit: cache.bitcoin_unit,
                onchain_send_limit: self.onchain_send_limit,
                onchain_receive_limit: self.onchain_receive_limit,
                is_sending: self.is_sending,
            }),
        );

        let overlay = match &self.modal {
            Modal::ShowQrCode(m) => m.view(),
            Modal::None => return content,
        };

        coincube_ui::widget::modal::Modal::new(content, overlay)
            .on_blur(Some(view::Message::Close))
            .into()
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Home(msg)) => {
                match msg {
                    HomeMessage::ToggleBalanceMask => {
                        self.balance_masked = !self.balance_masked;
                        Task::none()
                    }
                    HomeMessage::NextStep => {
                        if let Some(daemon) = daemon {
                            if self.current_view.step == 1 {
                                self.current_view.next();
                                let breez_client = self.breez_client.clone();
                                return Task::perform(
                                    async move { breez_client.fetch_onchain_limits().await },
                                    |limit| match limit {
                                        Ok(limits) => Message::View(view::Message::Home(
                                            HomeMessage::OnChainLimitsFetched {
                                                send: (limits.send.min_sat, limits.send.max_sat),
                                                receive: (
                                                    limits.receive.min_sat,
                                                    limits.receive.max_sat,
                                                ),
                                            },
                                        )),
                                        Err(error) => Message::View(view::Message::Home(
                                            HomeMessage::Error(error.to_string()),
                                        )),
                                    },
                                );
                            }
                            if self.current_view.step == 2 {
                                let mut tasks = Vec::new();
                                if matches!(
                                    self.transfer_direction,
                                    Some(TransferDirection::ActiveToVault)
                                ) {
                                    self.current_view.next();
                                    tasks.push(Task::perform(
                                        async move {
                                            match daemon.get_new_address().await {
                                                Ok(res) => Ok((res.address, res.derivation_index)),
                                                Err(e) => Err(e.into()),
                                            }
                                        },
                                        Message::ReceiveAddress,
                                    ));
                                    if let Ok(amount) = Amount::from_str_in(
                                        &self.entered_amount.value,
                                        breez_sdk_liquid::bitcoin::Denomination::Bitcoin,
                                    ) {
                                        let breez_client = self.breez_client.clone();
                                        tasks.push(Task::perform(
                                            async move {
                                                breez_client
                                                    .prepare_pay_onchain(&PreparePayOnchainRequest {
                                                        fee_rate_sat_per_vbyte: None,
                                                        amount: breez_sdk_liquid::model::PayAmount::Bitcoin { receiver_amount_sat: amount.to_sat() },
                                                    })
                                                    .await
                                            },
                                            |result| match result {
                                                Ok(response) => Message::View(view::Message::Home(HomeMessage::PrepareOnChainResponseReceived(response))),
                                                Err(error) => Message::View(view::Message::Home(
                                                    HomeMessage::Error(error.to_string()),
                                                )),
                                            },
                                        ))
                                    }
                                } else if matches!(
                                    self.transfer_direction,
                                    Some(TransferDirection::VaultToActive)
                                ) {
                                    self.current_view.next();
                                    let breez_client = self.breez_client.clone();
                                    tasks.push(Task::perform(
                                        async move {
                                            let result = breez_client.receive_onchain(None).await;
                                            result
                                        },
                                        |result| match result {
                                            Ok(response) => Message::View(view::Message::Home(
                                                HomeMessage::BreezOnchainAddress(
                                                    response.destination,
                                                ),
                                            )),
                                            Err(error) => Message::View(view::Message::Home(
                                                HomeMessage::Error(error.to_string()),
                                            )),
                                        },
                                    ));
                                }
                                return Task::batch(tasks);
                            }
                            self.current_view.next();
                        }
                        Task::none()
                    }
                    HomeMessage::PreviousStep => {
                        self.current_view.previous();
                        Task::none()
                    }
                    HomeMessage::SelectTransferDirection(direction) => {
                        self.transfer_direction = Some(direction);
                        Task::none()
                    }
                    HomeMessage::AmountEdited(amount) => {
                        self.entered_amount.value = amount.clone();

                        // Parse the entered amount
                        if amount.is_empty() {
                            self.entered_amount.valid = true;
                            self.entered_amount.warning = None;
                        } else if let Ok(entered_amt) = Amount::from_str_in(
                            &amount,
                            coincube_core::miniscript::bitcoin::Denomination::Bitcoin,
                        ) {
                            let entered_sat = entered_amt.to_sat();
                            let mut valid = true;
                            let mut warning = None;

                            let vault_balance = cache
                                .coins()
                                .iter()
                                .filter(|coin| coin.spend_info.is_none())
                                .fold(Amount::from_sat(0), |acc, coin| acc + coin.amount);

                            if let Some(direction) = self.transfer_direction {
                                match direction {
                                    TransferDirection::ActiveToVault => {
                                        if entered_amt > self.active_balance {
                                            valid = false;
                                            warning = Some("Amount exceeds Active balance");
                                        } else if let Some((min_sat, max_sat)) = self.onchain_send_limit {
                                            if entered_sat < min_sat || entered_sat > max_sat {
                                                valid = false;
                                                warning =
                                                    Some("Amount outside onchain send limits");
                                            }
                                        }
                                    }
                                    TransferDirection::VaultToActive => {
                                        if entered_amt > vault_balance {
                                            valid = false;
                                            warning = Some("Amount exceeds Vault balance");
                                        } else if let Some((min_sat, max_sat)) = self.onchain_receive_limit
                                        {
                                            if entered_sat < min_sat || entered_sat > max_sat {
                                                valid = false;
                                                warning =
                                                    Some("Amount outside onchain receive limits");
                                            }
                                        }
                                    }
                                }
                            }

                            self.entered_amount.valid = valid;
                            self.entered_amount.warning = warning;
                        } else {
                            self.entered_amount.valid = false;
                            self.entered_amount.warning = Some("Invalid amount format");
                        }

                        Task::none()
                    }
                    HomeMessage::ConfirmTransfer => {
                        if let Some(transfer_direction) = self.transfer_direction {
                            if matches!(transfer_direction, TransferDirection::ActiveToVault) {
                                if let Some(address_info) = self.receive_address_info.clone() {
                                    if let Some(prepare_onchain_send_response) =
                                        self.prepare_onchain_send_response.clone()
                                    {
                                        let breez_client = self.breez_client.clone();
                                        self.is_sending = true;
                                        return Task::perform(
                                            async move {
                                                breez_client
                                                    .pay_onchain(&PayOnchainRequest {
                                                        address: address_info.address.to_string(),
                                                        prepare_response:
                                                            prepare_onchain_send_response,
                                                    })
                                                    .await
                                            },
                                            |result| match result {
                                                Ok(_) => Message::View(view::Message::Home(
                                                    HomeMessage::TransferSuccessful,
                                                )),
                                                Err(error) => Message::View(view::Message::Home(
                                                    HomeMessage::Error(error.to_string()),
                                                )),
                                            },
                                        );
                                    }
                                }
                            } else if matches!(transfer_direction, TransferDirection::VaultToActive)
                            {
                                if let Some(address_info) = self.receive_address_info.clone() {
                                    if let Some(daemon) = daemon {
                                        // Parse the amount to send
                                        if let Ok(amount) = Amount::from_str_in(
                                        &self.entered_amount.value,
                                        coincube_core::miniscript::bitcoin::Denomination::Bitcoin,
                                    ) {
                                        let amount_sat = amount.to_sat();

                                        // Create destinations map (Breez address)
                                        let mut destinations = std::collections::HashMap::new();
                                        destinations.insert(
                                            address_info.address.as_unchecked().clone(),
                                            amount_sat,
                                        );

                                        // Get wallet for signing
                                        let wallet = self.wallet.clone();

                                        // Create, sign and broadcast the spend transaction
                                        let daemon_clone = daemon.clone();
                                        self.is_sending = true;
                                        return Task::perform(
                                            async move {
                                                let feerate_vb = FeeEstimator::new()
                                                    .get_high_priority_rate()
                                                    .await
                                                    .map_err(|e| format!("Failed to get fee rate: {}", e))?;

                                                // Create the spend transaction
                                                let mut psbt = match daemon_clone
                                                    .create_spend_tx(
                                                        &[], // Empty means auto-select coins
                                                        &destinations,
                                                        feerate_vb as u64,
                                                        None, // No specific change address
                                                    )
                                                    .await
                                                {
                                                    Ok(CreateSpendResult::Success { psbt, .. }) => {
                                                        psbt
                                                    }
                                                    Ok(CreateSpendResult::InsufficientFunds { missing }) => {
                                                        return Err(format!("Insufficient funds: {} sats missing", missing));
                                                    }
                                                    Err(e) => {
                                                        return Err(format!("Failed to create transaction: {}", e));
                                                    }
                                                };

                                                // Sign the PSBT with hot signer if available
                                                if let Some(wallet) = wallet {
                                                    if let Some(signer) = &wallet.signer {
                                                        psbt = signer.sign_psbt(psbt).map_err(|e| {
                                                            format!("Failed to sign PSBT: {}", e)
                                                        })?;
                                                    } else {
                                                        log::error!("No hot signer available in wallet");
                                                        return Err("No hot signer available. Please set up a hot signer for automatic signing.".to_string());
                                                    }
                                                } else {
                                                    log::error!("Wallet not available");
                                                    return Err("Wallet not available".to_string());
                                                }

                                                // Update the spend in daemon
                                                if let Err(e) = daemon_clone.update_spend_tx(&psbt).await {
                                                    return Err(format!("Failed to update spend: {}", e));
                                                }

                                                // Broadcast the transaction
                                                let txid = psbt.unsigned_tx.compute_txid();
                                                if let Err(e) = daemon_clone.broadcast_spend_tx(&txid).await {
                                                    return Err(format!("Failed to broadcast: {}", e));
                                                }

                                                Ok(())
                                            },
                                            |result| {
                                                match result {
                                                Ok(_) => Message::View(view::Message::Home(
                                                    HomeMessage::TransferSuccessful,
                                                )),
                                                Err(error) => {
                                                        log::error!("ERROR IN SENDING THE TX: {}", error);
                                                        Message::View(view::Message::Home(
                                                    HomeMessage::Error(error),
                                                ))},
                                            }},
                                        );
                                    }
                                    }
                                }
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::Error(err) => {
                        self.is_sending = false;
                        self.warning = Some(Error::Unexpected(err));
                        Task::none()
                    }
                    HomeMessage::ActiveBalanceUpdated(active_balance) => {
                        self.active_balance = active_balance;
                        Task::none()
                    }
                    HomeMessage::OnChainLimitsFetched { send, receive } => {
                        self.onchain_send_limit = Some(send);
                        self.onchain_receive_limit = Some(receive);
                        Task::none()
                    }
                    HomeMessage::PrepareOnChainResponseReceived(response) => {
                        self.prepare_onchain_send_response = Some(response);
                        Task::none()
                    }
                    HomeMessage::TransferSuccessful => {
                        self.current_view.next();
                        self.is_sending = false;
                        Task::none()
                    }
                    HomeMessage::BackToHome => {
                        self.current_view.reset();
                        self.transfer_direction = None;
                        self.entered_amount = form::Value::default();
                        self.receive_address_info = None;
                        self.warning = None;
                        self.onchain_send_limit = None;
                        self.onchain_receive_limit = None;
                        self.prepare_onchain_send_response = None;
                        self.is_sending = false;
                        Task::none()
                    }
                    HomeMessage::BreezOnchainAddress(address) => {
                        // Parse BIP-21 URI format: bitcoin:address?params or plain address
                        let addr_str = address
                            .strip_prefix("bitcoin:")
                            .unwrap_or(&address)
                            .split('?')
                            .next()
                            .unwrap_or(&address);

                        if let Ok(parsed) = Address::from_str(addr_str) {
                            self.receive_address_info = Some(ReceiveAddressInfo {
                                address: parsed.assume_checked(),
                                index: ChildNumber::Normal { index: 1 },
                                labels: HashMap::new(),
                            });
                        } else {
                            log::error!("Failed to parse Breez on-chain address: {}", addr_str);
                        }
                        Task::none()
                    }
                    HomeMessage::RefreshActiveBalance => self.load_active_balance(),
                }
            }
            Message::ReceiveAddress(res) => match res {
                Ok((address, index)) => {
                    self.warning = None;
                    self.receive_address_info = Some(ReceiveAddressInfo {
                        address,
                        index,
                        labels: HashMap::new(),
                    });
                    Task::none()
                }
                Err(e) => {
                    self.warning = Some(e);
                    self.receive_address_info = None;
                    Task::none()
                }
            },
            Message::View(view::Message::SelectAddress(_addr)) => {
                self.address_expanded = !self.address_expanded;
                Task::none()
            }
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                if let Some(daemon) = daemon {
                    match self.labels_edited.update(
                        daemon,
                        message,
                        self.receive_address_info
                            .iter_mut()
                            .map(|info| info as &mut dyn crate::daemon::model::LabelsLoader),
                    ) {
                        Ok(cmd) => {
                            self.warning = None;
                            cmd
                        }
                        Err(e) => {
                            self.warning = Some(e);
                            Task::none()
                        }
                    }
                } else {
                    Task::none()
                }
            }
            Message::View(view::Message::ShowQrCode(_)) => {
                if let Some(info) = &self.receive_address_info {
                    if let Some(modal) = ShowQrCodeModal::new(&info.address, info.index) {
                        self.modal = Modal::ShowQrCode(modal);
                    }
                }
                Task::none()
            }
            Message::View(view::Message::Close) => {
                self.modal = Modal::None;
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.wallet = wallet;
        self.load_active_balance()
    }
}

impl GlobalHome {
    fn load_active_balance(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();
        Task::perform(async move { breez_client.info().await }, |info| {
            if let Ok(info) = info {
                let balance = Amount::from_sat(
                    info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat,
                );
                Message::View(view::Message::Home(HomeMessage::ActiveBalanceUpdated(
                    balance,
                )))
            } else {
                Message::View(view::Message::Home(HomeMessage::Error(
                    "Couldn't fetch Active Wallet Balance".to_string(),
                )))
            }
        })
    }
}
