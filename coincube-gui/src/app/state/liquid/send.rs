use std::convert::TryInto;
use std::sync::Arc;
use std::time::Duration;

use breez_sdk_liquid::model::PaymentDetails;
use breez_sdk_liquid::prelude::Payment;
use breez_sdk_liquid::InputType;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{component::form, widget::*};
use iced::Task;

use super::sideshift_send::SideshiftSendFlow;

/// Map SDK prepare errors to user-friendly messages.
fn friendly_prepare_error(e: &impl std::fmt::Display) -> String {
    let msg = e.to_string();
    if msg.contains("not enough funds") || msg.contains("InsufficientFunds") {
        "Minimum spendable amount not met. Try adding more funds.".to_string()
    } else {
        format!("Failed to prepare payment: {}", msg)
    }
}
use crate::app::breez::assets::{
    asset_kind_for_id, format_usdt_display, lbtc_asset_id, parse_asset_to_minor_units,
    usdt_asset_id, AssetKind, USDT_PRECISION,
};
use crate::app::menu::{LiquidSubMenu, Menu};
use crate::app::settings::unit::BitcoinDisplayUnit;
use crate::app::state::{redirect, State};
use crate::app::view::SendPopupMessage;
use crate::app::{breez::BreezClient, cache::Cache};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::utils::format_time_ago;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAsset {
    Lbtc,
    Usdt,
}

/// Network/rail for the receiving side of a send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveNetwork {
    /// BTC via Lightning Network
    Lightning,
    /// L-BTC or USDt on Liquid
    Liquid,
    /// BTC on-chain
    Bitcoin,
    /// USDt on Ethereum (SideShift)
    Ethereum,
    /// USDt on Tron (SideShift)
    Tron,
    /// USDt on Binance Smart Chain (SideShift)
    Binance,
    /// USDt on Solana (SideShift)
    Solana,
}

impl ReceiveNetwork {
    /// Display name for the network badge.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Lightning => "Lightning",
            Self::Liquid => "Liquid",
            Self::Bitcoin => "Bitcoin",
            Self::Ethereum => "Ethereum",
            Self::Tron => "Tron",
            Self::Binance => "Binance",
            Self::Solana => "Solana",
        }
    }

    /// Whether this network requires SideShift.
    pub fn is_sideshift(&self) -> bool {
        matches!(
            self,
            Self::Ethereum | Self::Tron | Self::Binance | Self::Solana
        )
    }

    /// Convert to SideshiftNetwork for the SideShift API.
    pub fn to_sideshift_network(&self) -> Option<crate::services::sideshift::SideshiftNetwork> {
        match self {
            Self::Ethereum => Some(crate::services::sideshift::SideshiftNetwork::Ethereum),
            Self::Tron => Some(crate::services::sideshift::SideshiftNetwork::Tron),
            Self::Binance => Some(crate::services::sideshift::SideshiftNetwork::Binance),
            Self::Solana => Some(crate::services::sideshift::SideshiftNetwork::Solana),
            _ => None,
        }
    }

    /// Valid "They Receive" networks for a given "You Send" asset.
    pub fn options_for_send_asset(
        send_asset: SendAsset,
        cross_asset_supported: bool,
    ) -> Vec<(SendAsset, ReceiveNetwork)> {
        match send_asset {
            SendAsset::Lbtc => {
                let mut opts = vec![
                    (SendAsset::Lbtc, ReceiveNetwork::Lightning),
                    (SendAsset::Lbtc, ReceiveNetwork::Liquid),
                    (SendAsset::Lbtc, ReceiveNetwork::Bitcoin),
                ];
                if cross_asset_supported {
                    opts.push((SendAsset::Usdt, ReceiveNetwork::Liquid));
                }
                opts
            }
            SendAsset::Usdt => {
                let mut opts = vec![(SendAsset::Usdt, ReceiveNetwork::Liquid)];
                if cross_asset_supported {
                    opts.push((SendAsset::Lbtc, ReceiveNetwork::Lightning));
                    opts.push((SendAsset::Lbtc, ReceiveNetwork::Liquid));
                    opts.push((SendAsset::Lbtc, ReceiveNetwork::Bitcoin));
                }
                opts.extend([
                    (SendAsset::Usdt, ReceiveNetwork::Ethereum),
                    (SendAsset::Usdt, ReceiveNetwork::Tron),
                    (SendAsset::Usdt, ReceiveNetwork::Binance),
                    (SendAsset::Usdt, ReceiveNetwork::Solana),
                ]);
                opts
            }
        }
    }
}

#[derive(Debug)]
pub enum Modal {
    AmountInput,
    FiatInput {
        fiat_input: form::Value<String>,
        currencies: [crate::services::fiat::Currency; 4],
        selected_currency: crate::services::fiat::Currency,
        converters:
            std::collections::HashMap<crate::services::fiat::Currency, view::FiatAmountConverter>,
    },
    None,
}

#[derive(Debug)]
pub enum LiquidSendFlowState {
    Main { modal: Modal },
    FinalCheck,
    Sent,
}

/// LiquidSend manages the send interface for all Liquid wallet assets.
pub struct LiquidSend {
    breez_client: Arc<BreezClient>,
    sideshift_flow: Option<SideshiftSendFlow>,
    btc_balance: Amount,
    usdt_balance: u64,
    amount: Amount,
    amount_input: form::Value<String>,
    usdt_amount_input: form::Value<String>,
    /// The asset the recipient will receive.
    to_asset: SendAsset,
    /// The asset the user is paying with. Equals `to_asset` for same-asset sends;
    /// differs for cross-asset swaps (via SideSwap).
    from_asset: SendAsset,
    /// The wallet screen the user entered from. Set once when the send screen is
    /// opened and never mutated by cross-asset toggles. Used for guards and resets
    /// that need to know the user's original intent (replaces the old `usdt_only`
    /// invariant).
    home_asset: SendAsset,
    /// Network the recipient receives on (Lightning, Liquid, Bitcoin, Ethereum, etc.)
    receive_network: ReceiveNetwork,
    /// Whether the "You Send" picker modal is open.
    send_picker_open: bool,
    /// Whether the "They Receive" picker modal is open.
    receive_picker_open: bool,
    recent_transaction: Vec<view::liquid::RecentTransaction>,
    recent_payments: Vec<Payment>,
    selected_payment: Option<Payment>,
    input: form::Value<String>,
    input_type: Option<InputType>,
    lightning_limits: Option<(u64, u64)>, // (min_sats, max_sats)
    onchain_limits: Option<(u64, u64)>,   // (min_sats, max_sats)
    /// The asset requested by the URI (locked once detected from BIP21 asset_id).
    uri_asset: Option<AssetKind>,
    flow_state: LiquidSendFlowState,
    description: Option<String>,
    comment: Option<String>,
    error: Option<String>,
    prepare_response: Option<breez_sdk_liquid::prelude::PrepareSendResponse>,
    prepare_onchain_response: Option<breez_sdk_liquid::prelude::PreparePayOnchainResponse>,
    is_sending: bool,
    /// User preference for paying fees in the asset (USDt) vs L-BTC.
    /// `true` = pay fees in USDt, `false` = pay fees in L-BTC.
    /// Only relevant for same-asset USDt sends.
    pay_fees_with_asset: bool,
    /// Whether a SendMax prepare call is in flight.
    max_loading: bool,
    /// Quote and image handle for the "Transaction complete" screen.
    sent_quote: coincube_ui::component::quote_display::Quote,
    sent_image_handle: iced::widget::image::Handle,
}

impl LiquidSend {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
            sideshift_flow: None,
            btc_balance: Amount::from_sat(0),
            usdt_balance: 0,
            amount: Amount::from_sat(0),
            amount_input: form::Value::default(),
            usdt_amount_input: form::Value::default(),
            to_asset: SendAsset::Lbtc,
            from_asset: SendAsset::Lbtc,
            home_asset: SendAsset::Lbtc,
            receive_network: ReceiveNetwork::Lightning,
            send_picker_open: false,
            receive_picker_open: false,
            recent_transaction: Vec::new(),
            recent_payments: Vec::new(),
            selected_payment: None,
            input: form::Value::default(),
            uri_asset: None,
            error: None,
            flow_state: LiquidSendFlowState::Main { modal: Modal::None },
            input_type: None,
            lightning_limits: None,
            onchain_limits: None,
            comment: None,
            description: None,
            prepare_response: None,
            prepare_onchain_response: None,
            is_sending: false,
            pay_fees_with_asset: true,
            max_loading: false,
            sent_quote: coincube_ui::component::quote_display::random_quote("transaction-sent"),
            sent_image_handle: coincube_ui::component::quote_display::image_handle_for_context(
                "transaction-sent",
            ),
        }
    }

    pub fn usdt_balance(&self) -> u64 {
        self.usdt_balance
    }

    pub fn btc_balance(&self) -> Amount {
        self.btc_balance
    }

    pub fn pay_fees_with_asset(&self) -> bool {
        self.pay_fees_with_asset
    }

    pub fn max_loading(&self) -> bool {
        self.max_loading
    }

    pub fn send_asset(&self) -> SendAsset {
        self.from_asset
    }

    pub fn receive_asset(&self) -> SendAsset {
        self.to_asset
    }

    pub fn receive_network(&self) -> ReceiveNetwork {
        self.receive_network
    }

    pub fn send_picker_open(&self) -> bool {
        self.send_picker_open
    }

    pub fn receive_picker_open(&self) -> bool {
        self.receive_picker_open
    }

    pub fn cross_asset_supported(&self) -> bool {
        matches!(
            self.breez_client.network(),
            breez_sdk_liquid::bitcoin::Network::Bitcoin
        )
    }

    pub fn breez_client(&self) -> &Arc<BreezClient> {
        &self.breez_client
    }

    pub fn recent_transactions(&self) -> &[view::liquid::RecentTransaction] {
        &self.recent_transaction
    }

    fn load_balance(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();
        let usdt_only = self.home_asset == SendAsset::Usdt;

        Task::perform(
            async move {
                let info = breez_client.info().await;
                let payments = breez_client.list_payments(Some(20)).await;

                let balance = info
                    .as_ref()
                    .map(|info| {
                        let balance =
                            info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat;
                        Amount::from_sat(balance)
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

                let all_payments = payments.unwrap_or_default();
                let payments: Vec<_> = all_payments
                    .into_iter()
                    .filter(|p| {
                        let is_usdt = matches!(
                            &p.details,
                            PaymentDetails::Liquid { asset_id, .. } if asset_id == usdt_id
                        );
                        if usdt_only {
                            is_usdt
                        } else {
                            !is_usdt
                        }
                    })
                    .take(5)
                    .collect();

                (balance, usdt_balance, payments, error)
            },
            |(balance, usdt_balance, recent_payment, error)| {
                if let Some(err) = error {
                    Message::View(view::Message::LiquidSend(view::LiquidSendMessage::Error(
                        err,
                    )))
                } else {
                    Message::View(view::Message::LiquidSend(
                        view::LiquidSendMessage::DataLoaded {
                            balance,
                            usdt_balance,
                            recent_payment,
                        },
                    ))
                }
            },
        )
    }
}

impl State for LiquidSend {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        // Delegate to SideShift flow when active
        if let Some(sideshift) = &self.sideshift_flow {
            let asset_id = usdt_asset_id(self.breez_client.network()).unwrap_or("");
            return sideshift.view(
                menu,
                cache,
                self.usdt_balance,
                &self.recent_transaction,
                asset_id,
            );
        }

        let fiat_converter = cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

        if let Some(payment) = &self.selected_payment {
            view::dashboard(
                menu,
                cache,
                view::liquid::transaction_detail_view(
                    payment,
                    fiat_converter,
                    cache.bitcoin_unit,
                    usdt_asset_id(self.breez_client.network()).unwrap_or(""),
                ),
            )
        } else {
            let comment = self.comment.clone().unwrap_or("".to_string());

            view::liquid_send_with_flow(view::LiquidSendFlowConfig {
                flow_state: &self.flow_state,
                btc_balance: self.btc_balance,
                usdt_balance: self.usdt_balance,
                fiat_converter,
                recent_transaction: &self.recent_transaction,
                input: &self.input,
                amount_input: &self.amount_input,
                usdt_amount_input: &self.usdt_amount_input,
                to_asset: self.to_asset,
                from_asset: self.from_asset,
                receive_network: self.receive_network,
                send_picker_open: self.send_picker_open,
                receive_picker_open: self.receive_picker_open,
                uri_asset: self.uri_asset,
                usdt_asset_id: usdt_asset_id(self.breez_client.network()).unwrap_or(""),
                comment,
                description: self.description.as_deref(),
                lightning_limits: self.lightning_limits,
                amount: self.amount,
                prepare_response: self.prepare_response.as_ref(),
                is_sending: self.is_sending,
                menu,
                cache,
                input_type: &self.input_type,
                onchain_limits: self.onchain_limits,
                bitcoin_unit: cache.bitcoin_unit,
                prepare_onchain_response: self.prepare_onchain_response.as_ref(),
                error: self.error.as_deref(),
                // Cross-asset swaps require SideSwap (mainnet only)
                cross_asset_supported: matches!(
                    self.breez_client.network(),
                    breez_sdk_liquid::bitcoin::Network::Bitcoin
                ),
                pay_fees_with_asset: self.pay_fees_with_asset,
                max_loading: self.max_loading,
                sent_quote: &self.sent_quote,
                sent_image_handle: &self.sent_image_handle,
            })
        }
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        // Handle SideShift send messages when flow is active
        if let Message::View(view::Message::SideshiftSend(ref msg)) = message {
            if let Some(sideshift) = &mut self.sideshift_flow {
                // Intercept Reset/Back to return to native send
                if matches!(
                    msg,
                    view::SideshiftSendMessage::Reset | view::SideshiftSendMessage::Back
                ) && matches!(
                    sideshift.phase(),
                    super::sideshift_send::SendPhase::Sent
                        | super::sideshift_send::SendPhase::Failed
                        | super::sideshift_send::SendPhase::AddressInput
                ) {
                    self.sideshift_flow = None;
                    return self.load_balance();
                }
                return sideshift.update(msg, &self.breez_client, self.usdt_balance);
            }
            return Task::none();
        }

        // When SideShift flow is active, only forward DataLoaded for balance updates
        if self.sideshift_flow.is_some() {
            if let Message::View(view::Message::LiquidSend(view::LiquidSendMessage::DataLoaded {
                ..
            })) = &message
            {
                // Fall through to handle DataLoaded below
            } else {
                return Task::none();
            }
        }

        if let Message::View(view::Message::LiquidSend(ref msg)) = message {
            match msg {
                view::LiquidSendMessage::PresetAsset(asset) => {
                    // Set both "You Send" and "They Receive" to the same asset
                    self.from_asset = *asset;
                    self.to_asset = *asset;
                    self.home_asset = *asset;
                    self.receive_network = match asset {
                        SendAsset::Lbtc => ReceiveNetwork::Lightning,
                        SendAsset::Usdt => ReceiveNetwork::Liquid,
                    };
                    self.amount = Amount::ZERO;
                    self.input = form::Value::default();
                    self.input_type = None;
                    self.uri_asset = None;
                    self.error = None;
                    self.sideshift_flow = None;
                    return self.load_balance();
                }
                view::LiquidSendMessage::InputEdited(value) => {
                    self.input.value = value.clone();
                    self.error = None;
                    let breez = self.breez_client.clone();
                    let breez_clone = self.breez_client.clone();
                    let breez_client = self.breez_client.clone();
                    let value_owned = value.clone();
                    // TODO: Add some kind of debouncing mechanism here, so that we don't call breez
                    // API again and again
                    let value_for_callback = value.clone();
                    let validate_input = Task::perform(
                        async move { breez.validate_input(value_owned).await },
                        move |input| {
                            Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::InputValidated(
                                    value_for_callback.clone(),
                                    input,
                                ),
                            ))
                        },
                    );

                    // Fetch limits only if not already available
                    if self.lightning_limits.is_none() || self.onchain_limits.is_none() {
                        let fetch_lightning_limits = Task::perform(
                            async move { breez_clone.fetch_lightning_limits().await },
                            |limits| match limits {
                                Ok(limits) => Message::View(view::Message::LiquidSend(
                                    view::LiquidSendMessage::LightningLimitsFetched {
                                        min_sat: limits.send.min_sat,
                                        max_sat: limits.send.max_sat,
                                    },
                                )),
                                Err(e) => Message::View(view::Message::LiquidSend(
                                    view::LiquidSendMessage::Error(format!(
                                        "Couldn't fetch lightning limits: {}",
                                        e
                                    )),
                                )),
                            },
                        );

                        let fetch_onchain_limits = Task::perform(
                            async move { breez_client.fetch_onchain_limits().await },
                            |limits| match limits {
                                Ok(limits) => Message::View(view::Message::LiquidSend(
                                    view::LiquidSendMessage::OnChainLimitsFetched {
                                        min_sat: limits.send.min_sat,
                                        max_sat: limits.send.max_sat,
                                    },
                                )),
                                Err(e) => Message::View(view::Message::LiquidSend(
                                    view::LiquidSendMessage::Error(format!(
                                        "Couldn't fetch onchain limits: {}",
                                        e
                                    )),
                                )),
                            },
                        );
                        return Task::batch(vec![
                            validate_input,
                            fetch_lightning_limits,
                            fetch_onchain_limits,
                        ]);
                    }
                    return validate_input;
                }
                view::LiquidSendMessage::Send => {
                    // Route to SideShift for cross-chain sends
                    if self.receive_network.is_sideshift() {
                        let flow = SideshiftSendFlow::new();
                        // Pre-fill the address from the input and auto-select the network
                        let addr = self.input.value.trim().to_string();
                        if !addr.is_empty() {
                            // Dispatch address edit + network selection + Next
                            self.sideshift_flow = Some(flow);
                            let addr_msg = Message::View(view::Message::SideshiftSend(
                                view::SideshiftSendMessage::RecipientAddressEdited(addr),
                            ));
                            let network = self.receive_network.to_sideshift_network();
                            let mut tasks = vec![Task::done(addr_msg)];
                            if let Some(net) = network {
                                tasks.push(Task::done(Message::View(
                                    view::Message::SideshiftSend(
                                        view::SideshiftSendMessage::DisambiguateNetwork(net),
                                    ),
                                )));
                            }
                            tasks.push(Task::done(Message::View(view::Message::SideshiftSend(
                                view::SideshiftSendMessage::Next,
                            ))));
                            return Task::batch(tasks);
                        } else {
                            // No address yet — just open SideShift flow
                            self.sideshift_flow = Some(flow);
                            return Task::none();
                        }
                    }

                    let description = if let Some(input_type) = &self.input_type {
                        match input_type {
                            InputType::BitcoinAddress { address } => {
                                format!(
                                    "Sending money to {}",
                                    display_abbreviated(address.address.clone())
                                )
                            }
                            InputType::Bolt11 { invoice } => {
                                if let Some(amt) = invoice.amount_msat {
                                    if let Ok(amount) = Amount::from_str_in(
                                        &amt.to_string(),
                                        breez_sdk_liquid::bitcoin::Denomination::MilliSatoshi,
                                    ) {
                                        self.amount = amount;
                                        self.amount_input.valid = true;
                                        self.amount_input.value = if matches!(
                                            cache.bitcoin_unit,
                                            BitcoinDisplayUnit::BTC
                                        ) {
                                            amount.to_btc().to_string()
                                        } else {
                                            amount.to_sat().to_string()
                                        };
                                    }
                                }
                                if let Some(description) =
                                    invoice.description.as_deref().filter(|d| !d.is_empty())
                                {
                                    description.to_string()
                                } else {
                                    format!(
                                        "Sending money to {}",
                                        display_abbreviated(invoice.bolt11.clone())
                                    )
                                }
                            }
                            InputType::Bolt12Offer {
                                offer,
                                bip353_address,
                            } => {
                                let min_amount = offer.min_amount.clone().unwrap_or(
                                    breez_sdk_liquid::Amount::Bitcoin { amount_msat: 0 },
                                );

                                if let Some((min_limits, max_limits)) = self.lightning_limits {
                                    if let breez_sdk_liquid::Amount::Bitcoin { amount_msat } =
                                        min_amount
                                    {
                                        // convert from millisat to sat
                                        let amount_sat = amount_msat / 1000;
                                        self.lightning_limits = Some((
                                            std::cmp::max(min_limits, amount_sat),
                                            max_limits,
                                        ));
                                    }
                                }

                                if let Some(bip353_address) = bip353_address {
                                    format!("Sending money to {}", bip353_address.clone())
                                } else if let Some(description) = offer.description.clone() {
                                    description
                                } else {
                                    format!(
                                        "Sending money to {}",
                                        display_abbreviated(offer.offer.clone())
                                    )
                                }
                            }

                            InputType::LiquidAddress { address } => {
                                if self.to_asset == SendAsset::Usdt {
                                    if let Some(amount) = address.amount {
                                        let amount_str = format!("{}", amount);
                                        let base_units_opt = parse_asset_to_minor_units(
                                            amount_str.trim(),
                                            USDT_PRECISION,
                                        );
                                        match base_units_opt {
                                            Some(base_units) => {
                                                self.usdt_amount_input.value = amount_str;
                                                if base_units == 0 {
                                                    self.usdt_amount_input.valid = false;
                                                    self.usdt_amount_input.warning =
                                                        Some("Amount must be greater than zero");
                                                } else if base_units > self.usdt_balance {
                                                    self.usdt_amount_input.valid = false;
                                                    self.usdt_amount_input.warning =
                                                        Some("Insufficient USDt balance");
                                                } else {
                                                    self.usdt_amount_input.valid = true;
                                                    self.usdt_amount_input.warning = None;
                                                }
                                            }
                                            None => {
                                                self.usdt_amount_input.value = String::new();
                                                self.usdt_amount_input.valid = false;
                                                self.usdt_amount_input.warning =
                                                    Some("Invalid amount");
                                            }
                                        }
                                    }
                                } else if let Some(amount_sat) = address.amount_sat {
                                    let amount_str =
                                        if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                            Amount::from_sat(amount_sat).to_btc().to_string()
                                        } else {
                                            amount_sat.to_string()
                                        };
                                    let amount = Amount::from_sat(amount_sat);
                                    self.amount = amount;
                                    self.amount_input.value = amount_str;
                                    if amount > self.btc_balance {
                                        self.amount_input.valid = false;
                                        self.amount_input.warning = Some("Insufficient balance");
                                    } else if let Some((min_sat, max_sat)) = self.lightning_limits {
                                        if amount_sat < min_sat {
                                            self.amount_input.valid = false;
                                            self.amount_input.warning = Some("Below minimum limit");
                                        } else if amount_sat > max_sat {
                                            self.amount_input.valid = false;
                                            self.amount_input.warning =
                                                Some("Exceeds maximum limit");
                                        } else {
                                            self.amount_input.valid = true;
                                            self.amount_input.warning = None;
                                        }
                                    } else {
                                        self.amount_input.valid = true;
                                        self.amount_input.warning = None;
                                    }
                                }
                                format!(
                                    "Sending money to {}",
                                    display_abbreviated(address.address.clone())
                                )
                            }
                            _ => String::from("Send Payment"),
                        }
                    } else {
                        String::from("")
                    };

                    self.description = if description.is_empty() {
                        None
                    } else {
                        Some(description)
                    };
                    self.flow_state = LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    };
                }
                view::LiquidSendMessage::History => {
                    return redirect(Menu::Liquid(LiquidSubMenu::Transactions(None)));
                }
                view::LiquidSendMessage::SelectTransaction(idx) => {
                    if let Some(payment) = self.recent_payments.get(*idx).cloned() {
                        self.selected_payment = Some(payment.clone());
                        return Task::batch(vec![
                            redirect(Menu::Liquid(LiquidSubMenu::Transactions(None))),
                            Task::done(Message::View(view::Message::PreselectPayment(payment))),
                        ]);
                    }
                }
                view::LiquidSendMessage::DataLoaded {
                    balance,
                    usdt_balance,
                    recent_payment,
                } => {
                    self.btc_balance = *balance;
                    self.usdt_balance = *usdt_balance;
                    self.recent_payments = recent_payment.clone();

                    if !recent_payment.is_empty() {
                        let fiat_converter: Option<view::FiatAmountConverter> =
                            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
                        let txns = recent_payment
                            .iter()
                            .map(|payment| {
                                let status = payment.status;
                                let time_ago = format_time_ago(payment.timestamp.into());
                                let is_usdt_payment = matches!(
                                    &payment.details,
                                    PaymentDetails::Liquid { asset_id, .. }
                                        if asset_id == usdt_asset_id(self.breez_client.network()).unwrap_or("")
                                );
                                let amount = if is_usdt_payment {
                                    if let PaymentDetails::Liquid { asset_info: Some(ref ai), .. } = &payment.details {
                                        Amount::from_sat((ai.amount * 10_f64.powi(USDT_PRECISION as i32)).round() as u64)
                                    } else {
                                        Amount::from_sat(payment.amount_sat)
                                    }
                                } else {
                                    Amount::from_sat(payment.amount_sat)
                                };
                                let fiat_amount = if is_usdt_payment {
                                    None
                                } else {
                                    fiat_converter
                                        .as_ref()
                                        .map(|c: &view::FiatAmountConverter| c.convert(amount))
                                };

                                let desc: &str = match &payment.details {
                                    PaymentDetails::Lightning { payer_note, description, .. } => payer_note
                                        .as_ref()
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or(description),
                                    PaymentDetails::Liquid { payer_note, description, .. } => {
                                        let fallback = if is_usdt_payment && description.is_empty() {
                                            "USDt Transfer"
                                        } else {
                                            description.as_str()
                                        };
                                        payer_note
                                            .as_ref()
                                            .filter(|s| !s.is_empty())
                                            .map(|s| s.as_str())
                                            .unwrap_or(fallback)
                                    }
                                    PaymentDetails::Bitcoin { description, .. } => description,
                                };

                                let is_incoming = matches!(
                                    payment.payment_type,
                                    breez_sdk_liquid::prelude::PaymentType::Receive
                                );

                                let fees_sat = Amount::from_sat(payment.fees_sat);

                                let details = payment.details.clone();
                                let usdt_display = if is_usdt_payment {
                                    Some(format!(
                                        "{} USDt",
                                        format_usdt_display(amount.to_sat())
                                    ))
                                } else {
                                    None
                                };

                                view::liquid::RecentTransaction {
                                    description: desc.to_owned(),
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
                        self.recent_transaction = txns;
                    } else {
                        self.recent_transaction = Vec::new();
                    }
                }
                view::LiquidSendMessage::Error(err) => {
                    self.error = Some(err.to_string());
                    self.is_sending = false; // Reset sending flag on error
                                             // When a modal is open, the error toast renders inside the modal
                                             // overlay (above the backdrop). Otherwise use the global toast.
                    let modal_open = matches!(
                        self.flow_state,
                        LiquidSendFlowState::Main {
                            modal: Modal::AmountInput | Modal::FiatInput { .. }
                        }
                    );
                    if !modal_open {
                        return Task::done(Message::View(view::Message::ShowError(
                            err.to_string(),
                        )));
                    }
                }
                view::LiquidSendMessage::ClearError => {
                    self.error = None;
                }
                view::LiquidSendMessage::InputValidated(original_input, input_type) => {
                    // Discard stale async validation results — the user may have
                    // edited the input while validation was in-flight.
                    if *original_input != self.input.value {
                        return Task::none();
                    }
                    self.input.valid = input_type.is_some();
                    self.input_type = input_type.clone();

                    // Auto-detect asset from Liquid URI's asset_id
                    if let Some(InputType::LiquidAddress { address }) = &input_type {
                        let network = self.breez_client.network();
                        if let Some(ref uri_asset_id) = address.asset_id {
                            match asset_kind_for_id(uri_asset_id, network) {
                                Some(kind) => {
                                    self.uri_asset = Some(kind);
                                    let target_asset = match kind {
                                        AssetKind::Usdt => SendAsset::Usdt,
                                        AssetKind::Lbtc => SendAsset::Lbtc,
                                    };
                                    // On usdt_only screen with L-BTC URI: auto-enable
                                    // cross-asset (pay from USDt, receiver gets L-BTC).
                                    // Only on mainnet where SideSwap is available.
                                    let cross_asset_supported = matches!(
                                        network,
                                        breez_sdk_liquid::bitcoin::Network::Bitcoin
                                    );
                                    if self.home_asset == SendAsset::Usdt
                                        && target_asset == SendAsset::Lbtc
                                        && cross_asset_supported
                                    {
                                        self.to_asset = SendAsset::Lbtc;
                                        self.from_asset = SendAsset::Usdt;
                                    } else if self.home_asset == SendAsset::Usdt
                                        && target_asset != SendAsset::Usdt
                                    {
                                        // Non-mainnet: cross-asset not available, keep USDt
                                        self.to_asset = SendAsset::Usdt;
                                        self.from_asset = self.to_asset;
                                    } else {
                                        self.to_asset = target_asset;
                                        self.from_asset = self.to_asset;
                                    }
                                }
                                None => {
                                    // Unknown asset_id — only reset to_asset if we're
                                    // clearing a previously set URI lock. Otherwise preserve
                                    // the user's current asset selection.
                                    if self.uri_asset.is_some() {
                                        self.to_asset = if self.home_asset == SendAsset::Usdt {
                                            SendAsset::Usdt
                                        } else {
                                            SendAsset::Lbtc
                                        };
                                    }
                                    self.uri_asset = None;
                                    self.from_asset = self.to_asset;
                                }
                            }
                        } else {
                            // No asset_id in URI — only reset to_asset if we're
                            // clearing a previously set URI lock.
                            if self.uri_asset.is_some() {
                                self.to_asset = if self.home_asset == SendAsset::Usdt {
                                    SendAsset::Usdt
                                } else {
                                    SendAsset::Lbtc
                                };
                            }
                            self.uri_asset = None;
                            self.from_asset = self.to_asset;
                        }

                        // Pre-fill amount from URI if present, or clear stale values
                        if self.to_asset == SendAsset::Usdt {
                            if let Some(amount) = address.amount {
                                self.usdt_amount_input.value = amount.to_string();
                                self.usdt_amount_input.valid = amount > 0.0;
                            } else {
                                self.usdt_amount_input = form::Value::default();
                            }
                        }
                        if self.to_asset == SendAsset::Lbtc {
                            if let Some(amount_sat) = address.amount_sat {
                                self.amount = Amount::from_sat(amount_sat);
                                self.amount_input.value =
                                    if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                        Amount::from_sat(amount_sat).to_btc().to_string()
                                    } else {
                                        amount_sat.to_string()
                                    };
                                self.amount_input.valid = true;
                            } else {
                                self.amount = Amount::ZERO;
                                self.amount_input = form::Value::default();
                            }
                        }
                    } else {
                        // Not a LiquidAddress — clear URI asset state and restore default
                        self.uri_asset = None;
                        self.to_asset = if self.home_asset == SendAsset::Usdt {
                            SendAsset::Usdt
                        } else {
                            SendAsset::Lbtc
                        };
                        self.from_asset = self.to_asset;
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::AmountEdited(v)) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &mut self.flow_state
                    {
                        self.amount_input.value = v.clone();

                        if v.is_empty() {
                            self.amount_input.valid = true;
                            self.amount_input.warning = None;
                            self.amount = Amount::from_sat(0);
                        } else if let Ok(amount) = Amount::from_str_in(
                            v,
                            if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                            } else {
                                coincube_core::miniscript::bitcoin::Denomination::Satoshi
                            },
                        ) {
                            self.amount = amount;
                            let amount_sats = amount.to_sat();
                            let is_cross_asset = self.from_asset != self.to_asset;

                            // Skip balance check in cross-asset mode — the receiver amount
                            // is in a different denomination than the paying asset; the SDK
                            // validates actual balance during prepare.
                            if !is_cross_asset && amount > self.btc_balance {
                                self.amount_input.valid = false;
                                self.amount_input.warning = Some("Insufficient balance");
                            }
                            // Check limits if available
                            else if let Some((min_sat, max_sat)) = self.lightning_limits {
                                if amount_sats < min_sat {
                                    self.amount_input.valid = false;
                                    self.amount_input.warning = Some("Below minimum limit");
                                } else if amount_sats > max_sat {
                                    self.amount_input.valid = false;
                                    self.amount_input.warning = Some("Exceeds maximum limit");
                                } else {
                                    self.amount_input.valid = true;
                                    self.amount_input.warning = None;
                                }
                            } else {
                                self.amount_input.valid = true;
                                self.amount_input.warning = None;
                            }
                        } else {
                            self.amount_input.valid = false;
                            self.amount_input.warning = Some("Invalid amount format");
                        }
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::CommentEdited(comment)) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &mut self.flow_state
                    {
                        self.comment = Some(comment.clone());
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::FiatConvert) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &self.flow_state
                    {
                        // Determine default currencies
                        use crate::services::fiat::Currency;
                        let fiat_currency = cache
                            .fiat_price
                            .as_ref()
                            .and_then(|p| TryInto::<view::FiatAmountConverter>::try_into(p).ok())
                            .map(|c| c.currency())
                            .unwrap_or(Currency::USD);

                        let currencies = if fiat_currency == Currency::USD
                            || fiat_currency == Currency::EUR
                            || fiat_currency == Currency::GBP
                            || fiat_currency == Currency::JPY
                        {
                            [Currency::USD, Currency::EUR, Currency::GBP, Currency::JPY]
                        } else {
                            [fiat_currency, Currency::USD, Currency::EUR, Currency::GBP]
                        };

                        // Transition to Fiat Input with empty converters initially
                        self.flow_state = LiquidSendFlowState::Main {
                            modal: Modal::FiatInput {
                                fiat_input: form::Value::default(),
                                currencies,
                                selected_currency: fiat_currency,
                                converters: std::collections::HashMap::new(),
                            },
                        };

                        let price_source = cache
                            .fiat_price
                            .as_ref()
                            .map(|p| p.source())
                            .unwrap_or(crate::services::fiat::PriceSource::CoinGecko);

                        return Task::perform(
                            async move {
                                use crate::app::cache::FiatPriceRequest;

                                let mut tasks = vec![];
                                for currency in currencies.iter() {
                                    let request = FiatPriceRequest::new(price_source, *currency);
                                    tasks.push(async move {
                                        let price = request.send_default().await;
                                        (*currency, price)
                                    });
                                }

                                let mut converters = std::collections::HashMap::new();

                                for task in tasks {
                                    let (currency, price) = task.await;
                                    if let Ok(converter) =
                                        TryInto::<view::FiatAmountConverter>::try_into(&price)
                                    {
                                        converters.insert(currency, converter);
                                    }
                                }

                                converters
                            },
                            |converters| {
                                Message::View(view::Message::LiquidSend(
                                    view::LiquidSendMessage::PopupMessage(
                                        SendPopupMessage::FiatPricesLoaded(converters),
                                    ),
                                ))
                            },
                        );
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::FiatInputEdited(
                    fiat_input,
                )) => {
                    let is_cross_asset = self.from_asset != self.to_asset;
                    if let LiquidSendFlowState::Main {
                        modal:
                            Modal::FiatInput {
                                fiat_input: current_input,
                                selected_currency,
                                converters,
                                ..
                            },
                    } = &mut self.flow_state
                    {
                        current_input.value = fiat_input.clone();
                        current_input.warning = None;

                        // Validate numeric format
                        if fiat_input.is_empty() {
                            current_input.valid = true;
                        } else if fiat_input.parse::<f64>().is_ok() {
                            // Check if converted BTC amount exceeds limits
                            if let Some(converter) = converters.get(selected_currency) {
                                if let Ok(fiat_amount) = view::vault::fiat::FiatAmount::from_str_in(
                                    fiat_input,
                                    *selected_currency,
                                ) {
                                    if let Ok(btc_amount) = converter.convert_to_btc(&fiat_amount) {
                                        let amount_sats = btc_amount.to_sat();

                                        // Skip balance check in cross-asset mode — receiver
                                        // amount denomination differs from paying asset.
                                        if !is_cross_asset && btc_amount > self.btc_balance {
                                            current_input.valid = false;
                                            current_input.warning = Some("Insufficient balance");
                                        } else if let Some((min_sat, max_sat)) =
                                            self.lightning_limits
                                        {
                                            if amount_sats < min_sat {
                                                current_input.valid = false;
                                                current_input.warning = Some("Below minimum limit");
                                            } else if amount_sats > max_sat {
                                                current_input.valid = false;
                                                current_input.warning =
                                                    Some("Exceeds maximum limit");
                                            } else {
                                                current_input.valid = true;
                                            }
                                        } else {
                                            current_input.valid = true;
                                        }
                                    } else {
                                        // Conversion to BTC failed
                                        current_input.valid = false;
                                        current_input.warning = Some("Unable to convert to BTC");
                                    }
                                } else {
                                    // Invalid fiat amount format
                                    current_input.valid = false;
                                    current_input.warning = Some("Invalid fiat amount");
                                }
                            } else {
                                // Converter not available
                                current_input.valid = false;
                                current_input.warning = Some("Exchange rate unavailable");
                            }
                        } else {
                            current_input.valid = false;
                            current_input.warning = Some("Invalid number format");
                        }
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::FiatCurrencySelected(
                    currency,
                )) => {
                    if let LiquidSendFlowState::Main {
                        modal:
                            Modal::FiatInput {
                                selected_currency, ..
                            },
                    } = &mut self.flow_state
                    {
                        *selected_currency = *currency;
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::FiatPricesLoaded(
                    converters,
                )) => {
                    if let LiquidSendFlowState::Main {
                        modal:
                            Modal::FiatInput {
                                converters: modal_converters,
                                ..
                            },
                    } = &mut self.flow_state
                    {
                        *modal_converters = converters.clone();
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::FiatDone) => {
                    let is_cross_asset = self.from_asset != self.to_asset;
                    if let LiquidSendFlowState::Main {
                        modal:
                            Modal::FiatInput {
                                fiat_input,
                                selected_currency,
                                converters,
                                ..
                            },
                    } = &mut self.flow_state
                    {
                        if let Ok(_fiat_val) = fiat_input.value.parse::<f64>() {
                            // Check if converter is available
                            if let Some(converter) = converters.get(selected_currency) {
                                // Convert fiat to BTC using the converter for selected currency
                                if let Ok(fiat_amount) = view::vault::fiat::FiatAmount::from_str_in(
                                    &fiat_input.value,
                                    *selected_currency,
                                ) {
                                    if let Ok(btc_amount) = converter.convert_to_btc(&fiat_amount) {
                                        self.amount = btc_amount;
                                        let btc_str = if matches!(
                                            cache.bitcoin_unit,
                                            BitcoinDisplayUnit::BTC
                                        ) {
                                            btc_amount.to_btc().to_string()
                                        } else {
                                            btc_amount.to_sat().to_string()
                                        };
                                        let amount_sats = btc_amount.to_sat();

                                        // Validate the converted BTC amount.
                                        // Skip balance check in cross-asset mode — receiver
                                        // amount denomination differs from paying asset.
                                        let (valid, warning) = if !is_cross_asset
                                            && btc_amount > self.btc_balance
                                        {
                                            (false, Some("Amount exceeds available balance"))
                                        } else {
                                            let limits = if matches!(
                                                self.input_type,
                                                Some(InputType::BitcoinAddress { .. })
                                            ) {
                                                self.onchain_limits
                                            } else {
                                                self.lightning_limits
                                            };

                                            if let Some((min_sat, max_sat)) = limits {
                                                if amount_sats < min_sat {
                                                    (false, Some("Amount is below minimum limit"))
                                                } else if amount_sats > max_sat {
                                                    (false, Some("Amount exceeds maximum limit"))
                                                } else {
                                                    (true, None)
                                                }
                                            } else {
                                                (true, None)
                                            }
                                        };

                                        self.amount_input = form::Value {
                                            value: btc_str,
                                            valid,
                                            warning,
                                        };

                                        // Only close modal on successful conversion
                                        self.flow_state = LiquidSendFlowState::Main {
                                            modal: Modal::AmountInput,
                                        };
                                    } else {
                                        // Conversion to BTC failed - stay in fiat modal with error
                                        fiat_input.valid = false;
                                        fiat_input.warning = Some("Unable to convert to BTC");
                                    }
                                } else {
                                    // Invalid fiat amount - stay in fiat modal with error
                                    fiat_input.valid = false;
                                    fiat_input.warning = Some("Invalid fiat amount");
                                }
                            } else {
                                // Converter not available - stay in fiat modal with error
                                fiat_input.valid = false;
                                fiat_input.warning = Some("Exchange rate unavailable");
                            }
                        }
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::Done) => {
                    self.error = None;
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &self.flow_state
                    {
                        if let Some(input_type) = &self.input_type {
                            // USDt send path: Liquid address + USDt asset selected
                            if matches!(input_type, InputType::LiquidAddress { .. })
                                && self.to_asset == SendAsset::Usdt
                            {
                                let usdt_val_str = self.usdt_amount_input.value.trim().to_string();
                                let usdt_base =
                                    match parse_asset_to_minor_units(&usdt_val_str, USDT_PRECISION)
                                        .filter(|&v| v > 0)
                                    {
                                        Some(v) => v,
                                        None => {
                                            self.error = Some("Invalid USDt amount".to_string());
                                            return Task::none();
                                        }
                                    };
                                let network = self.breez_client.network();
                                let to_asset_id = match usdt_asset_id(network) {
                                    Some(id) => id.to_string(),
                                    None => {
                                        self.error =
                                            Some("USDt not available on this network".to_string());
                                        return Task::none();
                                    }
                                };
                                // Resolve from_asset for cross-asset swap
                                let from_asset_id: Option<String> =
                                    if self.from_asset != self.to_asset {
                                        let kind = match self.from_asset {
                                            SendAsset::Lbtc => AssetKind::Lbtc,
                                            SendAsset::Usdt => AssetKind::Usdt,
                                        };
                                        match kind.asset_id(network) {
                                            Some(id) => Some(id.to_string()),
                                            None => {
                                                self.error = Some(format!(
                                                    "{} not available on this network",
                                                    kind.ticker()
                                                ));
                                                return Task::none();
                                            }
                                        }
                                    } else {
                                        None
                                    };
                                let destination = match input_type {
                                    InputType::LiquidAddress { address } => address.address.clone(),
                                    _ => unreachable!(),
                                };
                                let breez_client = self.breez_client.clone();
                                return Task::perform(
                                    async move {
                                        breez_client
                                            .prepare_send_asset(
                                                destination,
                                                &to_asset_id,
                                                usdt_base,
                                                USDT_PRECISION,
                                                from_asset_id.as_deref(),
                                            )
                                            .await
                                    },
                                    |result| match result {
                                        Ok(prepare_response) => {
                                            Message::View(view::Message::LiquidSend(
                                                view::LiquidSendMessage::PrepareResponseReceived(
                                                    prepare_response,
                                                ),
                                            ))
                                        }
                                        Err(e) => Message::View(view::Message::LiquidSend(
                                            view::LiquidSendMessage::Error(friendly_prepare_error(
                                                &e,
                                            )),
                                        )),
                                    },
                                );
                            }

                            let destination = match input_type {
                                InputType::Bolt11 { invoice } => invoice.bolt11.clone(),
                                InputType::Bolt12Offer { offer, .. } => offer.offer.clone(),
                                InputType::BitcoinAddress { address } => address.address.clone(),
                                InputType::LiquidAddress { address } => address.address.clone(),
                                _ => {
                                    self.error = Some("Unsupported payment type".to_string());
                                    return Task::none();
                                }
                            };

                            // Cross-asset swap: from_asset differs from to_asset
                            // Use PayAmount::Asset with the appropriate asset IDs
                            if self.from_asset != self.to_asset {
                                let network = self.breez_client.network();
                                let to_asset_id = match lbtc_asset_id(network) {
                                    Some(id) => id.to_string(),
                                    None => {
                                        self.error =
                                            Some("L-BTC not available on this network".to_string());
                                        return Task::none();
                                    }
                                };
                                let from_kind = match self.from_asset {
                                    SendAsset::Lbtc => AssetKind::Lbtc,
                                    SendAsset::Usdt => AssetKind::Usdt,
                                };
                                let from_asset_id = match from_kind.asset_id(network) {
                                    Some(id) => id.to_string(),
                                    None => {
                                        self.error = Some(format!(
                                            "{} not available on this network",
                                            from_kind.ticker()
                                        ));
                                        return Task::none();
                                    }
                                };
                                let amount_sat = self.amount.to_sat();
                                let breez_client = self.breez_client.clone();
                                return Task::perform(
                                    async move {
                                        breez_client
                                            .prepare_send_asset(
                                                destination,
                                                &to_asset_id,
                                                amount_sat,
                                                crate::app::breez::assets::LBTC_PRECISION,
                                                Some(&from_asset_id),
                                            )
                                            .await
                                    },
                                    |result| match result {
                                        Ok(prepare_response) => {
                                            Message::View(view::Message::LiquidSend(
                                                view::LiquidSendMessage::PrepareResponseReceived(
                                                    prepare_response,
                                                ),
                                            ))
                                        }
                                        Err(e) => Message::View(view::Message::LiquidSend(
                                            view::LiquidSendMessage::Error(format!(
                                                "Failed to prepare cross-asset payment: {}",
                                                e
                                            )),
                                        )),
                                    },
                                );
                            }

                            let breez_client = self.breez_client.clone();
                            let breez_clone = self.breez_client.clone();
                            let amount_sat = self.amount.to_sat();

                            let lightning_send = Task::perform(
                                async move {
                                    breez_client
                                        .prepare_send_payment(
                                            &breez_sdk_liquid::prelude::PrepareSendRequest {
                                                destination,
                                                amount: Some(
                                                    breez_sdk_liquid::prelude::PayAmount::Bitcoin {
                                                        receiver_amount_sat: amount_sat,
                                                    },
                                                ),
                                                disable_mrh: None,
                                                payment_timeout_sec: None,
                                            },
                                        )
                                        .await
                                },
                                |result| match result {
                                    Ok(prepare_response) => {
                                        Message::View(view::Message::LiquidSend(
                                            view::LiquidSendMessage::PrepareResponseReceived(
                                                prepare_response,
                                            ),
                                        ))
                                    }
                                    Err(e) => Message::View(view::Message::LiquidSend(
                                        view::LiquidSendMessage::Error(format!(
                                            "Failed to prepare payment: {}",
                                            e
                                        )),
                                    )),
                                },
                            );

                            let onchain_send = Task::perform(
                                async move {
                                    breez_clone
                                        .prepare_pay_onchain(
                                            &breez_sdk_liquid::prelude::PreparePayOnchainRequest {
                                                amount:
                                                    breez_sdk_liquid::prelude::PayAmount::Bitcoin {
                                                        receiver_amount_sat: amount_sat,
                                                    },
                                                fee_rate_sat_per_vbyte: None,
                                            },
                                        )
                                        .await
                                },
                                |result| match result {
                                    Ok(prepare_response) => {
                                        Message::View(view::Message::LiquidSend(
                                            view::LiquidSendMessage::PrepareOnChainResponseReceived(
                                                prepare_response,
                                            ),
                                        ))
                                    }
                                    Err(e) => Message::View(view::Message::LiquidSend(
                                        view::LiquidSendMessage::Error(format!(
                                            "Failed to prepare payment: {}",
                                            e
                                        )),
                                    )),
                                },
                            );

                            if let InputType::BitcoinAddress { .. } = input_type {
                                return onchain_send;
                            } else {
                                return lightning_send;
                            }
                        }
                    }
                }
                view::LiquidSendMessage::PrepareResponseReceived(prepare_response) => {
                    // If the user wanted L-BTC fees but the SDK couldn't estimate
                    // them (fees_sat is None), fall back to asset fees automatically.
                    if !self.pay_fees_with_asset
                        && prepare_response.fees_sat.is_none()
                        && prepare_response.estimated_asset_fees.is_some()
                    {
                        self.pay_fees_with_asset = true;
                    }
                    self.prepare_response = Some(prepare_response.clone());
                    self.flow_state = LiquidSendFlowState::FinalCheck;
                }
                view::LiquidSendMessage::PrepareOnChainResponseReceived(prepare_response) => {
                    self.prepare_onchain_response = Some(prepare_response.clone());
                    self.flow_state = LiquidSendFlowState::FinalCheck;
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::ToggleSendAsset) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &self.flow_state
                    {
                        // Cross-asset swaps require SideSwap (mainnet only)
                        let cross_asset_supported = matches!(
                            self.breez_client.network(),
                            breez_sdk_liquid::bitcoin::Network::Bitcoin
                        );
                        if self.uri_asset.is_some() && cross_asset_supported {
                            // URI locked the to_asset — toggle changes from_asset (cross-asset swap)
                            let opposite = match self.to_asset {
                                SendAsset::Lbtc => SendAsset::Usdt,
                                SendAsset::Usdt => SendAsset::Lbtc,
                            };
                            if self.from_asset != self.to_asset {
                                // Already in cross-asset mode — toggle back to same-asset.
                                // On usdt_only screen: if to_asset was forced to Lbtc by URI,
                                // we can't go back to same-asset Lbtc send — block the toggle.
                                if self.home_asset == SendAsset::Usdt
                                    && self.to_asset != SendAsset::Usdt
                                {
                                    // Can't disable cross-asset on usdt_only screen when URI
                                    // requires a non-USDt asset — ignore toggle
                                } else {
                                    self.from_asset = self.to_asset;
                                }
                            } else {
                                // Enable cross-asset: pay with the opposite asset
                                self.from_asset = opposite;
                            }

                            // Re-validate amount inputs after cross-asset mode change.
                            // Balance checks depend on is_cross_asset, which just changed.
                            let is_cross_asset = self.from_asset != self.to_asset;
                            match self.to_asset {
                                SendAsset::Lbtc => {
                                    if !self.amount_input.value.trim().is_empty() {
                                        if !is_cross_asset && self.amount > self.btc_balance {
                                            self.amount_input.valid = false;
                                            self.amount_input.warning =
                                                Some("Insufficient balance");
                                        } else if self.amount_input.warning
                                            == Some("Insufficient balance")
                                        {
                                            // Clear stale balance warning
                                            self.amount_input.valid = true;
                                            self.amount_input.warning = None;
                                        }
                                    }
                                }
                                SendAsset::Usdt => {
                                    let trimmed = self.usdt_amount_input.value.trim();
                                    if !trimmed.is_empty() {
                                        if let Some(base_units) =
                                            parse_asset_to_minor_units(trimmed, USDT_PRECISION)
                                        {
                                            if !is_cross_asset && base_units > self.usdt_balance {
                                                self.usdt_amount_input.valid = false;
                                                self.usdt_amount_input.warning =
                                                    Some("Insufficient USDt balance");
                                            } else if self.usdt_amount_input.warning
                                                == Some("Insufficient USDt balance")
                                            {
                                                // Clear stale balance warning
                                                self.usdt_amount_input.valid = true;
                                                self.usdt_amount_input.warning = None;
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            // No URI lock — legacy behavior: toggle to_asset directly
                            let next = match self.to_asset {
                                SendAsset::Lbtc => SendAsset::Usdt,
                                SendAsset::Usdt => SendAsset::Lbtc,
                            };
                            self.to_asset = next;
                            self.from_asset = self.to_asset;
                            self.amount = Amount::ZERO;
                            self.usdt_amount_input = form::Value::default();
                            self.amount_input = form::Value::default();
                        }
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::ToggleFeeAsset) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &self.flow_state
                    {
                        self.pay_fees_with_asset = !self.pay_fees_with_asset;
                        self.error = None;
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::SendMax) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &self.flow_state
                    {
                        self.error = None;
                        if self.to_asset == SendAsset::Usdt {
                            if !self.pay_fees_with_asset || self.from_asset != self.to_asset {
                                // Fees paid in L-BTC or cross-asset — full USDt balance can be sent
                                let display = format_usdt_display(self.usdt_balance);
                                self.usdt_amount_input.value = display;
                                self.usdt_amount_input.valid = true;
                                self.usdt_amount_input.warning = None;
                            } else if let Some(InputType::LiquidAddress { address }) =
                                &self.input_type
                            {
                                // Fees paid in USDt — prepare with a small probe amount
                                // to learn the asset fee, then subtract it from balance.
                                let destination = address.address.clone();
                                let network = self.breez_client.network();
                                if let Some(to_asset_id) = usdt_asset_id(network) {
                                    let breez_client = self.breez_client.clone();
                                    let to_asset_id = to_asset_id.to_string();
                                    // Use a small probe amount (0.01 USDt = 1_000_000 base units)
                                    // just to discover the fee.
                                    let probe_amount = 1_000_000_u64;
                                    self.max_loading = true;
                                    return Task::perform(
                                        async move {
                                            breez_client
                                                .prepare_send_asset(
                                                    destination,
                                                    &to_asset_id,
                                                    probe_amount,
                                                    USDT_PRECISION,
                                                    None,
                                                )
                                                .await
                                                .map_err(|e| e.to_string())
                                        },
                                        |result| {
                                            Message::View(view::Message::LiquidSend(
                                                view::LiquidSendMessage::SendMaxPrepared(result),
                                            ))
                                        },
                                    );
                                }
                            }
                        } else if let Some(input_type) = &self.input_type {
                            // L-BTC send: use Drain to let SDK calculate max minus fees
                            let destination = match input_type {
                                InputType::Bolt11 { invoice } => invoice.bolt11.clone(),
                                InputType::Bolt12Offer { offer, .. } => offer.offer.clone(),
                                InputType::BitcoinAddress { address } => address.address.clone(),
                                InputType::LiquidAddress { address } => address.address.clone(),
                                _ => return Task::none(),
                            };
                            let breez_client = self.breez_client.clone();
                            self.max_loading = true;
                            return Task::perform(
                                async move {
                                    breez_client
                                        .prepare_send_payment(
                                            &breez_sdk_liquid::prelude::PrepareSendRequest {
                                                destination,
                                                amount: Some(
                                                    breez_sdk_liquid::prelude::PayAmount::Drain,
                                                ),
                                                disable_mrh: None,
                                                payment_timeout_sec: None,
                                            },
                                        )
                                        .await
                                        .map_err(|e| e.to_string())
                                },
                                |result| {
                                    Message::View(view::Message::LiquidSend(
                                        view::LiquidSendMessage::SendMaxPrepared(result),
                                    ))
                                },
                            );
                        }
                    }
                }
                view::LiquidSendMessage::SendMaxPrepared(result) => {
                    self.max_loading = false;
                    match result {
                        Ok(prepare_response) => {
                            if self.to_asset == SendAsset::Usdt {
                                // USDt with asset fees: subtract fee from balance
                                if let Some(asset_fee) = prepare_response.estimated_asset_fees {
                                    let fee_base =
                                        (asset_fee * 10_u64.pow(USDT_PRECISION as u32) as f64)
                                            .ceil() as u64;
                                    let max_amount = self.usdt_balance.saturating_sub(fee_base);
                                    if max_amount == 0 {
                                        self.error =
                                            Some("Balance too low to cover fees".to_string());
                                    } else {
                                        let display = format_usdt_display(max_amount);
                                        self.usdt_amount_input.value = display;
                                        self.usdt_amount_input.valid = true;
                                        self.usdt_amount_input.warning = None;
                                    }
                                } else {
                                    // No asset fee — use full balance
                                    let display = format_usdt_display(self.usdt_balance);
                                    self.usdt_amount_input.value = display;
                                    self.usdt_amount_input.valid = true;
                                    self.usdt_amount_input.warning = None;
                                }
                            } else {
                                // L-BTC drain: SDK returns the max sendable amount
                                // via fees_sat; calculate balance - fees
                                let fees = prepare_response.fees_sat.unwrap_or(0);
                                let max_sat = self.btc_balance.to_sat().saturating_sub(fees);
                                if max_sat == 0 {
                                    self.error = Some("Balance too low to cover fees".to_string());
                                } else {
                                    self.amount = Amount::from_sat(max_sat);
                                    self.amount_input.value = max_sat.to_string();
                                    self.amount_input.valid = true;
                                    self.amount_input.warning = None;
                                }
                            }
                        }
                        Err(e) => {
                            self.error = Some(format!("Failed to estimate max: {}", e));
                        }
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::UsdtAmountEdited(v)) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &mut self.flow_state
                    {
                        self.usdt_amount_input.value = v.clone();
                        let trimmed = v.trim();
                        if trimmed.is_empty() {
                            self.usdt_amount_input.valid = true;
                            self.usdt_amount_input.warning = None;
                        } else if let Some(base_units) =
                            parse_asset_to_minor_units(trimmed, USDT_PRECISION)
                        {
                            let is_cross_asset = self.from_asset != self.to_asset;

                            if base_units == 0 {
                                self.usdt_amount_input.valid = false;
                                self.usdt_amount_input.warning =
                                    Some("Amount must be greater than zero");
                            } else if !is_cross_asset && base_units > self.usdt_balance {
                                // Skip balance check in cross-asset mode — receiver amount
                                // denomination differs from paying asset; SDK validates during prepare.
                                self.usdt_amount_input.valid = false;
                                self.usdt_amount_input.warning = Some("Insufficient USDt balance");
                            } else {
                                self.usdt_amount_input.valid = true;
                                self.usdt_amount_input.warning = None;
                            }
                        } else {
                            self.usdt_amount_input.valid = false;
                            self.usdt_amount_input.warning = Some("Invalid amount");
                        }
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::Close) => {
                    self.flow_state = LiquidSendFlowState::Main { modal: Modal::None };
                    self.error = None;
                    self.amount = Amount::ZERO;
                    self.lightning_limits = None;
                    self.description = None;
                    self.comment = None;
                    self.amount_input = form::Value::default();
                    self.usdt_amount_input = form::Value::default();
                    self.to_asset = if self.home_asset == SendAsset::Usdt {
                        SendAsset::Usdt
                    } else {
                        SendAsset::Lbtc
                    };
                    self.input = form::Value::default();
                    self.input_type = None;
                    self.uri_asset = None;
                    self.from_asset = self.to_asset;
                }
                view::LiquidSendMessage::ConfirmSend => {
                    if let LiquidSendFlowState::FinalCheck = &self.flow_state {
                        if self.is_sending {
                            return Task::none();
                        }

                        self.is_sending = true;

                        if let Some(prepare_response) = self.prepare_response.clone() {
                            let breez_client = self.breez_client.clone();
                            let comment = self.comment.clone();
                            // Cross-asset swaps cannot use asset fees per SDK constraint.
                            // For same-asset USDt sends, respect the user's fee preference.
                            let is_cross_asset = self.from_asset != self.to_asset;
                            let use_asset_fees = if is_cross_asset {
                                false
                            } else if matches!(self.to_asset, SendAsset::Usdt) {
                                self.pay_fees_with_asset
                            } else {
                                false
                            };

                            return Task::perform(
                                async move {
                                    breez_client
                                        .send_payment(
                                            &breez_sdk_liquid::prelude::SendPaymentRequest {
                                                prepare_response,
                                                payer_note: comment,
                                                use_asset_fees: Some(use_asset_fees),
                                            },
                                        )
                                        .await
                                },
                                |result| match result {
                                    Ok(_send_response) => Message::View(view::Message::LiquidSend(
                                        view::LiquidSendMessage::SendComplete,
                                    )),
                                    Err(e) => Message::View(view::Message::LiquidSend(
                                        view::LiquidSendMessage::Error(format!(
                                            "Failed to send payment: {}",
                                            e
                                        )),
                                    )),
                                },
                            );
                        } else if let Some(prepare_onchain_response) =
                            self.prepare_onchain_response.clone()
                        {
                            if let Some(InputType::BitcoinAddress { address }) =
                                self.input_type.clone()
                            {
                                let breez_client = self.breez_client.clone();

                                return Task::perform(
                                    async move {
                                        breez_client
                                            .pay_onchain(
                                                &breez_sdk_liquid::prelude::PayOnchainRequest {
                                                    address: address.address.clone(),
                                                    prepare_response: prepare_onchain_response,
                                                },
                                            )
                                            .await
                                    },
                                    |result| match result {
                                        Ok(_send_response) => {
                                            Message::View(view::Message::LiquidSend(
                                                view::LiquidSendMessage::SendComplete,
                                            ))
                                        }
                                        Err(e) => Message::View(view::Message::LiquidSend(
                                            view::LiquidSendMessage::Error(format!(
                                                "Failed to send payment: {}",
                                                e
                                            )),
                                        )),
                                    },
                                );
                            }
                        } else {
                            self.error = Some("No prepare response available".to_string());
                            self.is_sending = false;
                        }
                    }
                }
                view::LiquidSendMessage::SendComplete => {
                    self.flow_state = LiquidSendFlowState::Sent;
                    self.prepare_response = None;
                    self.is_sending = false;
                    // Fresh quote for the success screen
                    self.sent_quote =
                        coincube_ui::component::quote_display::random_quote("transaction-sent");
                    self.sent_image_handle =
                        coincube_ui::component::quote_display::image_handle_for_context(
                            "transaction-sent",
                        );
                    let breez_client = self.breez_client.clone();
                    return Task::perform(async move { breez_client.sync().await }, |result| {
                        match result {
                            Ok(()) => Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::RefreshRequested,
                            )),
                            Err(err) => Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::Error(format!(
                                    "Failed to sync wallet: {}",
                                    err
                                )),
                            )),
                        }
                    });
                }
                view::LiquidSendMessage::BackToHome => {
                    self.input = form::Value::default();
                    self.amount = Amount::ZERO;
                    self.amount_input = form::Value::default();
                    self.usdt_amount_input = form::Value::default();
                    self.to_asset = if self.home_asset == SendAsset::Usdt {
                        SendAsset::Usdt
                    } else {
                        SendAsset::Lbtc
                    };
                    self.input_type = None;
                    self.uri_asset = None;
                    self.from_asset = self.to_asset;
                    self.description = None;
                    self.comment = None;
                    self.lightning_limits = None;
                    self.prepare_response = None;
                    self.is_sending = false;
                    self.flow_state = LiquidSendFlowState::Main { modal: Modal::None };
                }
                view::LiquidSendMessage::LightningLimitsFetched { min_sat, max_sat } => {
                    self.lightning_limits = Some((*min_sat, *max_sat));
                }
                view::LiquidSendMessage::OnChainLimitsFetched { min_sat, max_sat } => {
                    self.onchain_limits = Some((*min_sat, *max_sat));
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::FiatClose) => {
                    self.error = None;
                    self.flow_state = LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    }
                }
                view::LiquidSendMessage::RefreshRequested => {
                    return self.load_balance();
                }
                view::LiquidSendMessage::OpenSendPicker => {
                    self.send_picker_open = true;
                    self.receive_picker_open = false;
                    return Task::none();
                }
                view::LiquidSendMessage::OpenReceivePicker => {
                    self.receive_picker_open = true;
                    self.send_picker_open = false;
                    return Task::none();
                }
                view::LiquidSendMessage::ClosePicker => {
                    self.send_picker_open = false;
                    self.receive_picker_open = false;
                    return Task::none();
                }
                view::LiquidSendMessage::SetSendAsset(asset) => {
                    self.send_picker_open = false;
                    if self.from_asset != *asset {
                        self.from_asset = *asset;
                        self.to_asset = *asset;
                        self.home_asset = *asset;
                        // Reset receive network to default for the new asset
                        self.receive_network = match asset {
                            SendAsset::Lbtc => ReceiveNetwork::Lightning,
                            SendAsset::Usdt => ReceiveNetwork::Liquid,
                        };
                        // Reset input state
                        self.input = form::Value::default();
                        self.input_type = None;
                        self.uri_asset = None;
                        self.error = None;
                        self.sideshift_flow = None;
                        return self.load_balance();
                    }
                    return Task::none();
                }
                view::LiquidSendMessage::SetReceiveTarget(asset, network) => {
                    self.receive_picker_open = false;
                    self.to_asset = *asset;
                    self.receive_network = *network;
                    // If cross-asset, set from_asset differently
                    if *asset != self.from_asset {
                        // Cross-asset swap: from_asset stays, to_asset changes
                    } else {
                        self.from_asset = *asset;
                    }
                    // Reset input state for new target
                    self.input = form::Value::default();
                    self.input_type = None;
                    self.uri_asset = None;
                    self.error = None;
                    self.sideshift_flow = None;
                    return self.load_balance();
                }
            }
        }
        if let Message::View(view::Message::Close) | Message::View(view::Message::Reload) = message
        {
            self.selected_payment = None;
        }
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        if let Some(sideshift) = &self.sideshift_flow {
            return sideshift.subscription();
        }
        if self.is_sending {
            iced::time::every(Duration::from_millis(50)).map(|_| Message::Tick)
        } else {
            iced::Subscription::none()
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.selected_payment = None;
        self.sideshift_flow = None;
        self.load_balance()
    }
}

fn display_abbreviated(s: String) -> String {
    let formatted = if s.chars().count() > 30 {
        let first: String = s.chars().take(7).collect();
        let last: String = s
            .chars()
            .rev()
            .take(7)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        format!("{first}.....{last}")
    } else {
        s.to_string()
    };
    formatted
}
