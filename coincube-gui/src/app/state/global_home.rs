use std::collections::HashMap;
use std::convert::TryInto;
use std::str::FromStr;
use std::sync::Arc;

use breez_sdk_liquid::model::{
    PayOnchainRequest, PaymentDetails, PaymentType, PreparePayOnchainRequest,
    PreparePayOnchainResponse,
};
use coincube_core::miniscript::bitcoin::{bip32::ChildNumber, Address, Amount};

use crate::app::wallets::{DomainPaymentDetails, DomainPaymentStatus};
use coincube_ui::component::amount::BitcoinDisplayUnit;
use coincube_ui::component::form;
use coincube_ui::widget::*;
use iced::{Subscription, Task};
use std::time::Duration;

use super::vault::psbt::SignModal;
use super::{Cache, Menu, State};
use crate::app::state::vault::label::LabelsEdited;
use crate::app::state::vault::receive::ShowQrCodeModal;
use crate::app::view::global_home::{
    GlobalViewConfig, HomeView, IncomingTransferStage, PendingIncomingTransfer, TransferDirection,
};
use crate::app::view::HomeMessage;
use crate::app::wallets::{LiquidBackend, SparkBackend};
use crate::app::{message::Message, settings, view, wallet::Wallet};
use crate::daemon::model::{CreateSpendResult, LabelItem, Labelled, SpendTx};
use crate::daemon::Daemon;
use crate::dir::CoincubeDirectory;
use crate::services::feeestimation::fee_estimation::FeeEstimator;

#[derive(Default)]
pub enum Modal {
    ShowQrCode(ShowQrCodeModal),
    Sign(Box<SignModal>),
    #[default]
    None,
}

impl std::fmt::Debug for Modal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShowQrCode(m) => f.debug_tuple("ShowQrCode").field(m).finish(),
            Self::Sign(_) => f.debug_tuple("Sign").field(&"<SignModal>").finish(),
            Self::None => write!(f, "None"),
        }
    }
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
    breez_client: Arc<LiquidBackend>,
    /// Optional Spark backend handle. `None` when the cube has no
    /// Spark signer or the bridge subprocess failed to spawn — the
    /// Home page simply hides the Spark card in that case.
    spark_backend: Option<Arc<SparkBackend>>,
    liquid_balance: Amount,
    /// Spark wallet balance in sats, refreshed by
    /// [`load_balance`]. `Amount::ZERO` while the first
    /// `get_info` RPC is in flight, or forever when no Spark
    /// backend is wired up for this cube.
    spark_balance: Amount,
    usdt_balance: u64,
    usdt_balance_error: bool,
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
    onchain_send_limit: Option<(u64, u64)>,
    onchain_receive_limit: Option<(u64, u64)>,
    prepare_onchain_send_response: Option<PreparePayOnchainResponse>,
    is_sending: bool,
    transfer_spend_tx: Option<SpendTx>,
    transfer_signed: bool,
    spend_tx_fees: Option<Amount>,
    pending_vault_incoming: Option<PendingIncomingTransfer>,
    pending_vault_incoming_swap_id: Option<String>,
    pending_transfer_animation_phase: f32,
    pending_liquid_send_sats: u64,
    pending_usdt_send_sats: u64,
    pending_liquid_receive_sats: u64,
    pending_usdt_receive_sats: u64,
    datadir_path: CoincubeDirectory,
    network: coincube_core::miniscript::bitcoin::Network,
    cube_id: String,
}

impl GlobalHome {
    pub fn new(
        wallet: Arc<Wallet>,
        breez_client: Arc<LiquidBackend>,
        spark_backend: Option<Arc<SparkBackend>>,
        datadir_path: CoincubeDirectory,
        network: coincube_core::miniscript::bitcoin::Network,
        cube_id: String,
    ) -> Self {
        Self {
            wallet: Some(wallet),
            liquid_balance: Amount::ZERO,
            spark_balance: Amount::ZERO,
            usdt_balance: 0,
            usdt_balance_error: false,
            breez_client,
            spark_backend,
            balance_masked: false,
            transfer_direction: None,
            current_view: HomeView::default(),
            entered_amount: form::Value::default(),
            receive_address_info: None,
            labels_edited: LabelsEdited::default(),
            address_expanded: false,
            modal: Modal::default(),
            empty_labels: HashMap::default(),
            onchain_send_limit: None,
            onchain_receive_limit: None,
            prepare_onchain_send_response: None,
            is_sending: false,
            transfer_spend_tx: None,
            transfer_signed: false,
            spend_tx_fees: None,
            pending_vault_incoming: None,
            pending_vault_incoming_swap_id: None,
            pending_transfer_animation_phase: 0.0,
            pending_liquid_send_sats: 0,
            pending_usdt_send_sats: 0,
            pending_liquid_receive_sats: 0,
            pending_usdt_receive_sats: 0,
            datadir_path,
            network,
            cube_id,
        }
    }

    pub fn new_without_wallet(
        breez_client: Arc<LiquidBackend>,
        spark_backend: Option<Arc<SparkBackend>>,
        datadir_path: CoincubeDirectory,
        network: coincube_core::miniscript::bitcoin::Network,
        cube_id: String,
    ) -> Self {
        Self {
            wallet: None,
            liquid_balance: Amount::from_sat(0),
            spark_balance: Amount::ZERO,
            usdt_balance: 0,
            usdt_balance_error: false,
            breez_client,
            spark_backend,
            balance_masked: false,
            transfer_direction: None,
            current_view: HomeView::default(),
            entered_amount: form::Value::default(),
            receive_address_info: None,
            labels_edited: LabelsEdited::default(),
            address_expanded: false,
            modal: Modal::default(),
            empty_labels: HashMap::default(),
            onchain_send_limit: None,
            onchain_receive_limit: None,
            prepare_onchain_send_response: None,
            is_sending: false,
            transfer_spend_tx: None,
            transfer_signed: false,
            spend_tx_fees: None,
            pending_vault_incoming: None,
            pending_vault_incoming_swap_id: None,
            pending_transfer_animation_phase: 0.0,
            pending_liquid_send_sats: 0,
            pending_usdt_send_sats: 0,
            pending_liquid_receive_sats: 0,
            pending_usdt_receive_sats: 0,
            datadir_path,
            network,
            cube_id,
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

        let vault_pending_receive_sats = cache
            .coins()
            .iter()
            .filter(|coin| coin.spend_info.is_none() && !crate::daemon::model::coin_is_owned(coin))
            .fold(Amount::ZERO, |acc, coin| acc + coin.amount)
            .to_sat();

        let vault_pending_send_sats = cache
            .coins()
            .iter()
            .filter(|coin| {
                coin.spend_info
                    .as_ref()
                    .map(|si| si.height.is_none())
                    .unwrap_or(false)
            })
            .fold(Amount::ZERO, |acc, coin| acc + coin.amount)
            .to_sat();

        let liquid_balance = self.liquid_balance;
        let usdt_balance = self.usdt_balance;
        let usdt_balance_error = self.usdt_balance_error;

        // Fiat price is cube-level, not wallet-level, so get it directly from cache
        let fiat_converter: Option<view::FiatAmountConverter> =
            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

        let content = view::dashboard(
            menu,
            cache,
            view::global_home::global_home_view(GlobalViewConfig {
                liquid_balance,
                spark_balance: self.spark_balance,
                usdt_balance,
                usdt_balance_error,
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
                bitcoin_unit: cache.bitcoin_unit,
                onchain_send_limit: self.onchain_send_limit,
                onchain_receive_limit: self.onchain_receive_limit,
                is_sending: self.is_sending,
                is_tx_signed: self.transfer_signed,
                prepare_onchain_send_response: self.prepare_onchain_send_response.as_ref(),
                spend_tx_fees: self.spend_tx_fees,
                pending_liquid_send_sats: self.pending_liquid_send_sats,
                pending_usdt_send_sats: self.pending_usdt_send_sats,
                pending_liquid_receive_sats: self.pending_liquid_receive_sats,
                pending_usdt_receive_sats: self.pending_usdt_receive_sats,
                vault_pending_send_sats,
                vault_pending_receive_sats,
                pending_vault_incoming: self.pending_vault_incoming,
                pending_animation_phase: self.pending_transfer_animation_phase,
                btc_usd_price: cache.btc_usd_price,
            }),
        );

        let overlay = match &self.modal {
            Modal::ShowQrCode(m) => m.view(),
            Modal::Sign(sign_modal) => {
                // Delegate to SignModal's view this will render the signing UI
                use crate::app::state::vault::psbt::Modal as PsbtModalTrait;
                if self.transfer_spend_tx.is_some() {
                    return sign_modal.view(content);
                } else {
                    return content;
                }
            }
            Modal::None => return content,
        };

        coincube_ui::widget::modal::Modal::new(content, overlay)
            .on_blur(Some(view::Message::Close))
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = Vec::new();

        if let Modal::Sign(sign_modal) = &self.modal {
            // To fetch hardware wallets
            use crate::app::state::vault::psbt::Modal as PsbtModalTrait;
            subscriptions.push(sign_modal.subscription());
        }

        if self
            .pending_vault_incoming
            .map(|pending| pending.stage != IncomingTransferStage::Completed)
            .unwrap_or(false)
        {
            subscriptions.push(iced::time::every(Duration::from_millis(120)).map(|_| {
                Message::View(view::Message::Home(
                    HomeMessage::PendingTransferAnimationTick,
                ))
            }));
        }

        Subscription::batch(subscriptions)
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
                    HomeMessage::SendAsset(asset) => {
                        use crate::app::menu::LiquidSubMenu;
                        Task::batch(vec![
                            crate::app::state::redirect(Menu::Liquid(LiquidSubMenu::Send)),
                            Task::done(Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::PresetAsset(asset),
                            ))),
                        ])
                    }
                    HomeMessage::ReceiveAsset(asset) => {
                        use crate::app::menu::LiquidSubMenu;
                        Task::batch(vec![
                            crate::app::state::redirect(Menu::Liquid(LiquidSubMenu::Receive)),
                            Task::done(Message::View(view::Message::LiquidReceive(
                                view::LiquidReceiveMessage::SetReceiveAsset(asset),
                            ))),
                        ])
                    }
                    HomeMessage::SendSparkBtc => {
                        use crate::app::menu::SparkSubMenu;
                        crate::app::state::redirect(Menu::Spark(SparkSubMenu::Send))
                    }
                    HomeMessage::ReceiveSparkBtc => {
                        use crate::app::menu::SparkSubMenu;
                        crate::app::state::redirect(Menu::Spark(SparkSubMenu::Receive))
                    }
                    HomeMessage::SparkBalanceUpdated(balance) => {
                        self.spark_balance = balance;
                        Task::none()
                    }
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
                                    Some(TransferDirection::LiquidToVault)
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
                                        if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                            breez_sdk_liquid::bitcoin::Denomination::Bitcoin
                                        } else {
                                            breez_sdk_liquid::bitcoin::Denomination::Satoshi
                                        },
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
                                            move |result| match result {
                                                Ok(response) => Message::View(view::Message::Home(HomeMessage::PrepareOnChainResponseReceived(response))),
                                                Err(error) => Message::View(view::Message::Home(
                                                    HomeMessage::Error(error.to_string()),
                                                )),
                                            },
                                        ))
                                    }
                                } else if matches!(
                                    self.transfer_direction,
                                    Some(TransferDirection::VaultToLiquid)
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
                            if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                            } else {
                                coincube_core::miniscript::bitcoin::Denomination::Satoshi
                            },
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
                                    TransferDirection::LiquidToVault => {
                                        if entered_amt > self.liquid_balance {
                                            valid = false;
                                            warning = Some("Amount exceeds Liquid balance");
                                        } else if let Some((min_sat, max_sat)) =
                                            self.onchain_send_limit
                                        {
                                            if entered_sat < min_sat {
                                                valid = false;
                                                warning = Some("Amount below minimum limits");
                                            } else if entered_sat > max_sat {
                                                valid = false;
                                                warning = Some("Amount above maximum limits");
                                            }
                                        }
                                    }
                                    TransferDirection::VaultToLiquid => {
                                        if entered_amt > vault_balance {
                                            valid = false;
                                            warning = Some("Amount exceeds Vault balance");
                                        } else if let Some((min_sat, max_sat)) =
                                            self.onchain_receive_limit
                                        {
                                            if entered_sat < min_sat {
                                                valid = false;
                                                warning = Some("Amount below minimum limits");
                                            } else if entered_sat > max_sat {
                                                valid = false;
                                                warning = Some("Amount above maximum limits");
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
                    HomeMessage::SignVaultToLiquidTx => {
                        if let Some(transfer_direction) = self.transfer_direction {
                            if matches!(transfer_direction, TransferDirection::VaultToLiquid) {
                                if let Some(address_info) = self.receive_address_info.clone() {
                                    if let Some(daemon) = daemon {
                                        let denomination = if matches!(
                                            cache.bitcoin_unit,
                                            crate::app::settings::unit::BitcoinDisplayUnit::BTC
                                        ) {
                                            coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                                        } else {
                                            coincube_core::miniscript::bitcoin::Denomination::Satoshi
                                        };
                                        if let Ok(amount) = Amount::from_str_in(
                                            &self.entered_amount.value,
                                            denomination,
                                        ) {
                                            let amount_sat = amount.to_sat();
                                            let mut destinations = std::collections::HashMap::new();
                                            destinations.insert(
                                                address_info.address.as_unchecked().clone(),
                                                amount_sat,
                                            );

                                            let daemon_clone = daemon.clone();
                                            let wallet = self.wallet.clone();
                                            let cache_clone =
                                                (cache.datadir_path.clone(), cache.network);
                                            self.is_sending = true;
                                            return Task::perform(
                                                async move {
                                                    let feerate_vb = FeeEstimator::new()
                                                        .get_high_priority_rate()
                                                        .await
                                                        .map_err(|e| {
                                                            format!("Failed to get fee rate: {}", e)
                                                        })?;

                                                    let psbt = match daemon_clone
                                                        .create_spend_tx(
                                                            &[],
                                                            &destinations,
                                                            feerate_vb as u64,
                                                            None,
                                                        )
                                                        .await
                                                    {
                                                        Ok(CreateSpendResult::Success {
                                                            psbt,
                                                            ..
                                                        }) => psbt,
                                                        Ok(
                                                            CreateSpendResult::InsufficientFunds {
                                                                missing,
                                                            },
                                                        ) => {
                                                            return Err(format!("Insufficient funds: {} sats missing", missing));
                                                        }
                                                        Err(e) => {
                                                            return Err(format!(
                                                                "Failed to create transaction: {}",
                                                                e
                                                            ));
                                                        }
                                                    };

                                                    daemon_clone
                                                        .update_spend_tx(&psbt)
                                                        .await
                                                        .map_err(|e| {
                                                            format!("Failed to save PSBT: {}", e)
                                                        })?;

                                                    Ok((psbt, wallet, cache_clone))
                                                },
                                                |result| {
                                                    Message::View(view::Message::Home(
                                                        HomeMessage::TransferPsbtReady(result),
                                                    ))
                                                },
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::ConfirmTransfer => {
                        if let Some(transfer_direction) = self.transfer_direction {
                            if matches!(transfer_direction, TransferDirection::LiquidToVault) {
                                // LiquidToVault: Direct broadcast (no signing needed)
                                if let Some(address_info) = self.receive_address_info.clone() {
                                    if let Some(prepare_onchain_send_response) =
                                        self.prepare_onchain_send_response.clone()
                                    {
                                        let Ok(transfer_amount) = Amount::from_str_in(
                                            &self.entered_amount.value,
                                            if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC)
                                            {
                                                breez_sdk_liquid::bitcoin::Denomination::Bitcoin
                                            } else {
                                                breez_sdk_liquid::bitcoin::Denomination::Satoshi
                                            },
                                        ) else {
                                            self.entered_amount.valid = false;
                                            return Task::none();
                                        };
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
                                            move |result| match result {
                                                Ok(response) => {
                                                    let swap_id = if matches!(
                                                        response.payment.payment_type,
                                                        PaymentType::Send
                                                    ) {
                                                        match response.payment.details {
                                                            PaymentDetails::Bitcoin {
                                                                swap_id,
                                                                ..
                                                            } => Some(swap_id),
                                                            _ => None,
                                                        }
                                                    } else {
                                                        None
                                                    };

                                                    Message::View(view::Message::Home(
                                                        HomeMessage::LiquidToVaultSubmitted {
                                                            amount: transfer_amount,
                                                            swap_id,
                                                        },
                                                    ))
                                                }
                                                Err(error) => Message::View(view::Message::Home(
                                                    HomeMessage::Error(error.to_string()),
                                                )),
                                            },
                                        );
                                    }
                                }
                            } else if matches!(transfer_direction, TransferDirection::VaultToLiquid)
                            {
                                if self.transfer_signed {
                                    if let Some(spend_tx) = &self.transfer_spend_tx {
                                        if let Some(daemon) = daemon {
                                            let txid = spend_tx.psbt.unsigned_tx.compute_txid();
                                            let daemon_clone = daemon.clone();
                                            self.is_sending = true;

                                            return Task::perform(
                                                async move {
                                                    daemon_clone
                                                        .broadcast_spend_tx(&txid)
                                                        .await
                                                        .map_err(|e| {
                                                            format!("Failed to broadcast: {}", e)
                                                        })
                                                },
                                                |result| match result {
                                                    Ok(()) => Message::View(view::Message::Home(
                                                        HomeMessage::TransferSuccessful,
                                                    )),
                                                    Err(e) => {
                                                        log::error!(
                                                            "Failed to broadcast transfer: {}",
                                                            e
                                                        );
                                                        Message::View(view::Message::Home(
                                                            HomeMessage::Error(e),
                                                        ))
                                                    }
                                                },
                                            );
                                        }
                                    }
                                } else {
                                    return Task::done(Message::View(view::Message::ShowError(
                                        "Please sign the transaction first".to_string(),
                                    )));
                                }
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::Error(err) => {
                        self.is_sending = false;
                        Task::done(Message::View(view::Message::ShowError(err)))
                    }
                    HomeMessage::PendingAmountsUpdated {
                        liquid_send_sats,
                        usdt_send_sats,
                        liquid_receive_sats,
                        usdt_receive_sats,
                    } => {
                        self.pending_liquid_send_sats = liquid_send_sats;
                        self.pending_usdt_send_sats = usdt_send_sats;
                        self.pending_liquid_receive_sats = liquid_receive_sats;
                        self.pending_usdt_receive_sats = usdt_receive_sats;
                        Task::none()
                    }
                    HomeMessage::LiquidBalanceUpdated(liquid_balance) => {
                        self.liquid_balance = liquid_balance;
                        Task::none()
                    }
                    HomeMessage::UsdtBalanceUpdated(usdt_balance) => {
                        self.usdt_balance = usdt_balance;
                        self.usdt_balance_error = false;
                        Task::none()
                    }
                    HomeMessage::UsdtBalanceFetchFailed => {
                        self.usdt_balance_error = true;
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
                    HomeMessage::LiquidToVaultSubmitted { amount, swap_id } => {
                        self.current_view.next();
                        self.is_sending = false;
                        self.pending_vault_incoming = Some(PendingIncomingTransfer {
                            amount,
                            stage: IncomingTransferStage::TransferInitiated,
                        });
                        self.pending_vault_incoming_swap_id = swap_id.clone();
                        if let Some(swap_id) = swap_id {
                            return self.persist_pending_liquid_to_vault_transfer(
                                swap_id,
                                amount.to_sat(),
                            );
                        }
                        Task::none()
                    }
                    HomeMessage::LiquidToVaultPending(swap_id) => {
                        if self.is_matching_pending_swap(swap_id.as_deref()) {
                            if let Some(mut pending) = self.pending_vault_incoming {
                                pending.stage = IncomingTransferStage::SwappingLbtcToBtc;
                                self.pending_vault_incoming = Some(pending);
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::LiquidToVaultWaitingConfirmation(swap_id) => {
                        if self.is_matching_pending_swap(swap_id.as_deref()) {
                            if let Some(mut pending) = self.pending_vault_incoming {
                                pending.stage = IncomingTransferStage::SendingToVault;
                                self.pending_vault_incoming = Some(pending);
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::LiquidToVaultSucceeded(swap_id) => {
                        if self.is_matching_pending_swap(swap_id.as_deref()) {
                            if let Some(mut pending) = self.pending_vault_incoming {
                                pending.stage = IncomingTransferStage::Completed;
                                self.pending_vault_incoming = Some(pending);
                            }
                            self.pending_vault_incoming_swap_id = None;
                            return self.clear_pending_liquid_to_vault_transfer();
                        }
                        Task::none()
                    }
                    HomeMessage::LiquidToVaultFailed(swap_id) => {
                        if self.is_matching_pending_swap(swap_id.as_deref()) {
                            self.pending_vault_incoming = None;
                            self.pending_vault_incoming_swap_id = None;
                            return Task::batch(vec![
                                self.clear_pending_liquid_to_vault_transfer(),
                                Task::done(Message::View(view::Message::ShowError(
                                    "Liquid to Vault transfer failed. Please retry.".to_string(),
                                ))),
                            ]);
                        }
                        Task::none()
                    }
                    HomeMessage::PendingTransferRestored {
                        amount_sat,
                        stage,
                        swap_id,
                    } => {
                        self.pending_vault_incoming = Some(PendingIncomingTransfer {
                            amount: Amount::from_sat(amount_sat),
                            stage,
                        });
                        self.pending_vault_incoming_swap_id = Some(swap_id);
                        Task::none()
                    }
                    HomeMessage::PendingTransferAnimationTick => {
                        if self
                            .pending_vault_incoming
                            .map(|pending| pending.stage != IncomingTransferStage::Completed)
                            .unwrap_or(false)
                        {
                            self.pending_transfer_animation_phase =
                                (self.pending_transfer_animation_phase + 0.08) % 1.0;
                        } else {
                            self.pending_transfer_animation_phase = 0.0;
                        }
                        Task::none()
                    }
                    HomeMessage::BackToHome => {
                        self.current_view.reset();
                        self.transfer_direction = None;
                        self.entered_amount = form::Value::default();
                        self.receive_address_info = None;
                        self.onchain_send_limit = None;
                        self.onchain_receive_limit = None;
                        self.prepare_onchain_send_response = None;
                        self.is_sending = false;
                        self.transfer_spend_tx = None;
                        self.transfer_signed = false;
                        if self
                            .pending_vault_incoming
                            .map(|pending| pending.stage == IncomingTransferStage::Completed)
                            .unwrap_or(false)
                        {
                            self.pending_vault_incoming = None;
                            self.pending_vault_incoming_swap_id = None;
                            return self.clear_pending_liquid_to_vault_transfer();
                        }
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
                            let network = cache.network;
                            match parsed.require_network(network) {
                                Ok(checked_address) => {
                                    self.receive_address_info = Some(ReceiveAddressInfo {
                                        address: checked_address,
                                        index: ChildNumber::Normal { index: 1 },
                                        labels: HashMap::new(),
                                    });
                                }
                                Err(_) => {
                                    log::error!(
                                        "Address {} is not valid for network {:?}",
                                        addr_str,
                                        network
                                    );
                                }
                            }
                        } else {
                            log::error!("Failed to parse Breez on-chain address: {}", addr_str);
                        }
                        Task::none()
                    }
                    HomeMessage::RefreshLiquidBalance => self.load_liquid_balance(),
                    HomeMessage::TransferPsbtReady(result) => {
                        self.is_sending = false;
                        match result {
                            Ok((psbt, wallet, (datadir_path, network))) => {
                                if let Some(wallet) = wallet {
                                    let sigs =
                                        match wallet.main_descriptor.partial_spend_info(&psbt) {
                                            Ok(info) => info,
                                            Err(e) => {
                                                let err_msg =
                                                    format!("Failed to get signature info: {}", e);
                                                return Task::done(Message::View(
                                                    view::Message::ShowError(err_msg),
                                                ));
                                            }
                                        };

                                    let spend_amount =
                                        psbt.unsigned_tx.output.iter().map(|out| out.value).sum();

                                    // Use primary path if no inputs are using a relative locktime
                                    let use_primary_path = !psbt
                                        .unsigned_tx
                                        .input
                                        .iter()
                                        .map(|txin| txin.sequence)
                                        .any(|seq| seq.is_relative_lock_time());
                                    let max_vbytes = wallet.main_descriptor.unsigned_tx_max_vbytes(
                                        &psbt.unsigned_tx,
                                        use_primary_path,
                                    );
                                    let fees = psbt.fee().expect("Fees should be present");
                                    self.spend_tx_fees = Some(fees);

                                    // Create minimal SpendTx
                                    let spend_tx = SpendTx {
                                        network,
                                        psbt: psbt.clone(),
                                        coins: std::collections::HashMap::new(), // Empty for transfer
                                        labels: HashMap::new(),
                                        sigs,
                                        change_indexes: vec![],
                                        spend_amount,
                                        fee_amount: None,
                                        max_vbytes,
                                        status: crate::daemon::model::SpendStatus::Pending,
                                        updated_at: Some(
                                            std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap()
                                                .as_secs()
                                                as u32,
                                        ),
                                        kind: crate::daemon::model::TransactionKind::SendToSelf,
                                    };

                                    self.transfer_spend_tx = Some(spend_tx);

                                    // Create the SignModal
                                    let sign_modal = SignModal::new(
                                        std::collections::HashSet::new(),
                                        wallet,
                                        datadir_path,
                                        network,
                                        true,
                                        None,
                                    );

                                    self.modal = Modal::Sign(Box::new(sign_modal));
                                } else {
                                    return Task::done(Message::View(view::Message::ShowError(
                                        "Wallet not available".to_string(),
                                    )));
                                }
                            }
                            Err(e) => {
                                return Task::done(Message::View(view::Message::ShowError(e)));
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::TransferSigningComplete => {
                        self.transfer_signed = true;
                        self.modal = Modal::None;
                        self.is_sending = false;
                        Task::none()
                    }
                }
            }
            Message::ReceiveAddress(res) => match res {
                Ok((address, index)) => {
                    self.receive_address_info = Some(ReceiveAddressInfo {
                        address,
                        index,
                        labels: HashMap::new(),
                    });
                    Task::none()
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    self.receive_address_info = None;
                    Task::done(Message::View(view::Message::ShowError(err_msg)))
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
                        Ok(cmd) => cmd,
                        Err(e) => {
                            let err_msg = e.to_string();
                            Task::done(Message::View(view::Message::ShowError(err_msg)))
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
                self.transfer_spend_tx = None;
                Task::none()
            }
            Message::Updated(_) => {
                if let (Modal::Sign(ref mut sign_modal), Some(daemon)) = (&mut self.modal, daemon) {
                    if let Some(ref mut spend_tx) = self.transfer_spend_tx {
                        use crate::app::state::vault::psbt::Modal as PsbtModalTrait;
                        let task = sign_modal.update(daemon, message, spend_tx);

                        return Task::batch(vec![
                            task,
                            Task::perform(async {}, |_| {
                                Message::View(view::Message::Home(
                                    HomeMessage::TransferSigningComplete,
                                ))
                            }),
                        ]);
                    }
                }
                Task::none()
            }
            Message::Signed(_, _)
            | Message::HardwareWallets(_)
            | Message::View(view::Message::SelectHardwareWallet(_))
            | Message::View(view::Message::Spend(_)) => {
                if let (Modal::Sign(ref mut sign_modal), Some(daemon)) = (&mut self.modal, daemon) {
                    if let Some(ref mut spend_tx) = self.transfer_spend_tx {
                        use crate::app::state::vault::psbt::Modal as PsbtModalTrait;
                        return sign_modal.update(daemon, message, spend_tx);
                    }
                }
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
        let mut tasks = vec![
            self.load_liquid_balance(),
            self.load_usdt_balance(),
            self.load_pending_sends(),
            self.restore_pending_liquid_to_vault_transfer(),
        ];
        if let Some(spark) = self.load_spark_balance() {
            tasks.push(spark);
        }
        Task::batch(tasks)
    }
}

impl GlobalHome {
    fn is_matching_pending_swap(&self, incoming_swap_id: Option<&str>) -> bool {
        match (
            &self.pending_vault_incoming,
            &self.pending_vault_incoming_swap_id,
        ) {
            (Some(_), Some(expected_swap_id)) => incoming_swap_id
                .map(|swap_id| swap_id == expected_swap_id)
                .unwrap_or(false),
            (Some(_), None) => false,
            _ => false,
        }
    }

    /// Fetch the Spark wallet balance via `get_info` on the bridge.
    /// Returns `None` when no Spark backend is wired up for the cube
    /// so the caller can skip scheduling the task entirely.
    fn load_spark_balance(&self) -> Option<Task<Message>> {
        let backend = self.spark_backend.clone()?;
        Some(Task::perform(
            async move { backend.get_info().await },
            |result| match result {
                Ok(info) => Message::View(view::Message::Home(HomeMessage::SparkBalanceUpdated(
                    Amount::from_sat(info.balance_sats),
                ))),
                Err(e) => {
                    tracing::warn!("Home: spark get_info failed: {}", e);
                    // Soft-fail: leave the card showing whatever the
                    // last successful fetch returned. No hard error
                    // because the Spark card is non-essential to the
                    // rest of the home page.
                    Message::View(view::Message::Home(HomeMessage::SparkBalanceUpdated(
                        Amount::ZERO,
                    )))
                }
            },
        ))
    }

    fn load_liquid_balance(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();
        Task::perform(async move { breez_client.info().await }, |info| {
            if let Ok(info) = info {
                let balance = Amount::from_sat(
                    info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat,
                );
                Message::View(view::Message::Home(HomeMessage::LiquidBalanceUpdated(
                    balance,
                )))
            } else {
                Message::View(view::Message::Home(HomeMessage::Error(
                    "Couldn't fetch Liquid Wallet Balance".to_string(),
                )))
            }
        })
    }

    fn load_pending_sends(&self) -> Task<Message> {
        use crate::app::breez_liquid::assets::{asset_kind_for_id, AssetKind};
        let breez_client = self.breez_client.clone();
        let network = self.network;
        Task::perform(
            async move {
                match breez_client.list_payments(Some(20)).await {
                    Ok(payments) => {
                        let mut liquid_send_sats: u64 = 0;
                        let mut usdt_send_sats: u64 = 0;
                        let mut liquid_receive_sats: u64 = 0;
                        let mut usdt_receive_sats: u64 = 0;
                        for payment in &payments {
                            if !matches!(payment.status, DomainPaymentStatus::Pending) {
                                continue;
                            }
                            let is_send = !payment.is_incoming();
                            match &payment.details {
                                DomainPaymentDetails::LiquidAsset {
                                    asset_id,
                                    asset_info,
                                    ..
                                } => {
                                    if asset_kind_for_id(asset_id, network) == Some(AssetKind::Usdt)
                                    {
                                        let minor = asset_info
                                            .as_ref()
                                            .map(|ai| ai.amount_minor)
                                            .unwrap_or(payment.amount_sat);
                                        if is_send {
                                            usdt_send_sats = usdt_send_sats.saturating_add(minor);
                                        } else {
                                            usdt_receive_sats =
                                                usdt_receive_sats.saturating_add(minor);
                                        }
                                    } else if is_send {
                                        liquid_send_sats = liquid_send_sats
                                            .saturating_add(payment.amount_sat + payment.fees_sat);
                                    } else {
                                        liquid_receive_sats =
                                            liquid_receive_sats.saturating_add(payment.amount_sat);
                                    }
                                }
                                _ => {
                                    if is_send {
                                        liquid_send_sats = liquid_send_sats
                                            .saturating_add(payment.amount_sat + payment.fees_sat);
                                    } else {
                                        liquid_receive_sats =
                                            liquid_receive_sats.saturating_add(payment.amount_sat);
                                    }
                                }
                            }
                        }
                        (
                            liquid_send_sats,
                            usdt_send_sats,
                            liquid_receive_sats,
                            usdt_receive_sats,
                        )
                    }
                    Err(_) => (0, 0, 0, 0),
                }
            },
            |(liquid_send_sats, usdt_send_sats, liquid_receive_sats, usdt_receive_sats)| {
                Message::View(view::Message::Home(HomeMessage::PendingAmountsUpdated {
                    liquid_send_sats,
                    usdt_send_sats,
                    liquid_receive_sats,
                    usdt_receive_sats,
                }))
            },
        )
    }

    fn load_usdt_balance(&self) -> Task<Message> {
        use crate::app::breez_liquid::assets::{asset_kind_for_id, AssetKind};
        let breez_client = self.breez_client.clone();
        let network = self.network;
        Task::perform(
            async move {
                breez_client.info().await.map(|info| {
                    info.wallet_info
                        .asset_balances
                        .iter()
                        .find_map(|ab| {
                            if asset_kind_for_id(&ab.asset_id, network) == Some(AssetKind::Usdt) {
                                Some(ab.balance_sat)
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0)
                })
            },
            |result| match result {
                Ok(usdt_balance) => Message::View(view::Message::Home(
                    HomeMessage::UsdtBalanceUpdated(usdt_balance),
                )),
                Err(e) => {
                    tracing::error!("USDt balance fetch failed: {:?}", e);
                    Message::View(view::Message::Home(HomeMessage::UsdtBalanceFetchFailed))
                }
            },
        )
    }

    fn persist_pending_liquid_to_vault_transfer(
        &self,
        swap_id: String,
        amount_sat: u64,
    ) -> Task<Message> {
        let network_dir = self.datadir_path.network_directory(self.network);
        let cube_id = self.cube_id.clone();
        Task::perform(
            async move {
                settings::update_settings_file(&network_dir, move |mut current| {
                    if let Some(cube) = current.cubes.iter_mut().find(|c| c.id == cube_id) {
                        cube.pending_liquid_to_vault_transfer =
                            Some(settings::PendingLiquidToVaultTransfer {
                                swap_id,
                                amount_sat,
                            });
                    }
                    Some(current)
                })
                .await
            },
            |res| {
                if let Err(e) = res {
                    log::warn!("Failed to persist pending liquid->vault transfer: {}", e);
                }
                Message::Tick
            },
        )
    }

    fn clear_pending_liquid_to_vault_transfer(&self) -> Task<Message> {
        let network_dir = self.datadir_path.network_directory(self.network);
        let cube_id = self.cube_id.clone();
        Task::perform(
            async move {
                settings::update_settings_file(&network_dir, move |mut current| {
                    if let Some(cube) = current.cubes.iter_mut().find(|c| c.id == cube_id) {
                        cube.pending_liquid_to_vault_transfer = None;
                    }
                    Some(current)
                })
                .await
            },
            |res| {
                if let Err(e) = res {
                    log::warn!("Failed to clear pending liquid->vault transfer: {}", e);
                }
                Message::Tick
            },
        )
    }

    fn restore_pending_liquid_to_vault_transfer(&self) -> Task<Message> {
        let network_dir = self.datadir_path.network_directory(self.network);
        let cube_id = self.cube_id.clone();
        let breez_client = self.breez_client.clone();
        Task::perform(
            async move {
                let settings = settings::Settings::from_file(&network_dir).ok();
                let pending = settings
                    .as_ref()
                    .and_then(|s| s.cubes.iter().find(|c| c.id == cube_id))
                    .and_then(|cube| cube.pending_liquid_to_vault_transfer.clone())?;

                let mut stage = IncomingTransferStage::TransferInitiated;
                let payments = breez_client.list_payments(None).await.ok();

                if let Some(payment) = payments.and_then(|ps| {
                    ps.into_iter().find(|payment| {
                        if payment.is_incoming() {
                            return false;
                        }
                        match &payment.details {
                            DomainPaymentDetails::OnChainBitcoin {
                                swap_id: Some(id), ..
                            } => id == &pending.swap_id,
                            _ => false,
                        }
                    })
                }) {
                    stage = match payment.status {
                        DomainPaymentStatus::Complete => {
                            let cube_id_for_clear = cube_id.clone();
                            let _ =
                                settings::update_settings_file(&network_dir, move |mut current| {
                                    if let Some(cube) =
                                        current.cubes.iter_mut().find(|c| c.id == cube_id_for_clear)
                                    {
                                        cube.pending_liquid_to_vault_transfer = None;
                                    }
                                    Some(current)
                                })
                                .await;
                            return None;
                        }
                        DomainPaymentStatus::Pending
                        | DomainPaymentStatus::WaitingFeeAcceptance => match payment.details {
                            DomainPaymentDetails::OnChainBitcoin {
                                claim_tx_id: Some(_),
                                ..
                            } => IncomingTransferStage::SendingToVault,
                            _ => IncomingTransferStage::SwappingLbtcToBtc,
                        },
                        DomainPaymentStatus::Created => IncomingTransferStage::TransferInitiated,
                        DomainPaymentStatus::Failed
                        | DomainPaymentStatus::TimedOut
                        | DomainPaymentStatus::Refundable
                        | DomainPaymentStatus::RefundPending => {
                            let cube_id_for_clear = cube_id.clone();
                            let _ =
                                settings::update_settings_file(&network_dir, move |mut current| {
                                    if let Some(cube) =
                                        current.cubes.iter_mut().find(|c| c.id == cube_id_for_clear)
                                    {
                                        cube.pending_liquid_to_vault_transfer = None;
                                    }
                                    Some(current)
                                })
                                .await;
                            return None;
                        }
                    };
                }

                Some((pending.amount_sat, pending.swap_id, stage))
            },
            |restored| {
                if let Some((amount_sat, swap_id, stage)) = restored {
                    Message::View(view::Message::Home(HomeMessage::PendingTransferRestored {
                        amount_sat,
                        stage,
                        swap_id,
                    }))
                } else {
                    Message::Tick
                }
            },
        )
    }
}
