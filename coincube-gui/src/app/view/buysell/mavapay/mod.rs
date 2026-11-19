pub mod ui;

use std::sync::Arc;

use coincube_spark_protocol::PrepareSendOk;

use crate::app::{
    breez_spark::client::SparkClientError,
    state::spark::send::{
        bolt11_amount_sat, reconcile_pending_send, retry_guard_for, PendingSend, ReconcileOutcome,
        RetryGuard,
    },
    view::{self, buysell::panel},
    wallets::SparkBackend,
};

use crate::services::{
    coincube::*,
    mavapay::{api::*, MavapayClient, MavapayMessage},
};

#[derive(Debug)]
pub enum MavapayFlowStep {
    BuyInputFrom {
        ln_invoice: Option<String>,
        getting_invoice: bool,
        sending_quote: bool,
    },
    SellInputForm {
        /// Spark's sat balance, once loaded. `None` hides the Spark fulfilment
        /// option, which leaves the deposit QR as the only way to pay.
        spark_balance: Option<u64>,
        banks: Option<MavapayBanks>,
        beneficiary: Beneficiary,
        sending_quote: bool,
    },
    Checkout {
        quote: GetQuoteResponse,
        fulfilled_order: Option<GetOrderResponse>,
        invoice_qr_code_data: Option<iced::widget::qr_code::Data>,
        spark_balance: Option<u64>,
        /// Paying a sell order's invoice from Spark. Unused for a buy.
        sell: SellPayment,
        /// Order ID for SSE transaction status updates
        stream_order_id: Option<String>,
    },
    History {
        transactions: Option<Vec<OrderTransaction>>,
        loading: bool,
    },
    OrderDetail {
        transaction: OrderTransaction,
        order: Option<GetOrderResponse>,
        loading: bool,
    },
}

/// Where paying a sell order's Lightning invoice from Spark stands.
///
/// This is a money path, so it follows the Spark Send panel's retry rule
/// (liquid-removal invariant I5): once a send has been handed to the bridge,
/// nothing offers another attempt until the payment's state has been checked,
/// and an attempt is only ever repeated under the same idempotency key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SellFulfilment {
    /// Nothing in flight; the user may pay from Spark.
    #[default]
    Idle,
    /// `prepare_send` is running. No money has moved.
    Preparing,
    /// `send_payment` was dispatched and hasn't answered.
    Sending,
    /// The bridge confirmed the payment (or a check found it landed).
    Paid,
    /// The send was dispatched and nobody said what happened to it. No new
    /// attempt is offered until a check says it is safe.
    OutcomeUnknown {
        /// A state check is running.
        checking: bool,
        /// The last check's verdict. `None` until the first one finishes.
        outcome: Option<ReconcileOutcome>,
        /// What protects a repeat of this payment from paying twice.
        guard: RetryGuard,
    },
}

impl SellFulfilment {
    /// A request for this payment is with the bridge and its answer is due
    /// (a send's or a check's answer arrives within the send deadline, or
    /// as a made-up error when the bridge dies). Anything that would
    /// replace the checkout before then orphans that answer.
    pub fn awaiting_answer(&self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Sending | Self::OutcomeUnknown { checking: true, .. }
        )
    }

    /// The payment is unresolved, so leaving the checkout (or replacing the
    /// flow, or rebuilding the panel) would drop its idempotency key, its
    /// recorded intent and the "Check again" path.
    ///
    /// True while a request is with the bridge, and after a check that could
    /// not settle the payment: before the first verdict, or on an
    /// inconclusive one ("still in progress", history unavailable, or a
    /// repeat that can't be made safely). Only a verdict a user can act on
    /// releases it: `NoTrace`, which says there is no sign the payment left
    /// the wallet and is the state that offers a guarded retry. `Paid` and
    /// `Idle` are at rest. See [`MavapayState::sell_checkout_held`] for the
    /// one case that lets go of an unresolved payment.
    pub fn in_flight(&self) -> bool {
        matches!(
            self,
            Self::Preparing
                | Self::Sending
                | Self::OutcomeUnknown { checking: true, .. }
                | Self::OutcomeUnknown {
                    outcome: None | Some(ReconcileOutcome::Inconclusive(_)),
                    ..
                }
        )
    }

    /// Whether the check that followed an unknown outcome allows another
    /// attempt. Fail-closed: see [`ReconcileOutcome::may_resend`].
    pub fn may_resend(&self) -> bool {
        matches!(
            self,
            Self::OutcomeUnknown {
                checking: false,
                outcome: Some(outcome),
                guard,
            } if outcome.may_resend(*guard)
        )
    }
}

/// Paying one sell order's invoice from Spark: where it stands, plus what a
/// retry needs. Lives on the checkout step, so it dies with the order's
/// invoice — a new quote is a new payment and gets a fresh key.
#[derive(Debug, Clone, Default)]
pub struct SellPayment {
    pub fulfilment: SellFulfilment,
    /// Minted on the first send and reused by every repeat of *this* payment,
    /// so the SDK dedups a repeat against an attempt that did land. Retired
    /// only once the payment is settled either way.
    pub idempotency_key: Option<String>,
    /// What the last dispatched send looked like, for the state check.
    pub pending: Option<PendingSend>,
}

/// Create the Lightning invoice a Mavapay buy pays into: a Spark BOLT11
/// invoice, so the purchased sats land in the Spark wallet.
pub(crate) async fn create_buy_invoice(
    spark: &SparkBackend,
    sat_amount: u64,
) -> Result<String, String> {
    let description = format!("Coincube-Buysell Mavapay Purchase: {} SATS", sat_amount);
    spark
        .receive_bolt11(Some(sat_amount), description, None)
        .await
        .map(|ok| ok.payment_request)
        .map_err(|e| e.to_string())
}

/// Prepare paying a sell order's invoice from Spark, and refuse a preview
/// that isn't the order the user reviewed.
///
/// An invoice that carries its own amount is prepared as-is; an amountless
/// one is prepared for the quoted total. Either way the prepared payment must
/// be a plain Lightning payment of no more than the quoted total, or nothing
/// is sent.
pub(crate) async fn prepare_sell_payment(
    spark: &SparkBackend,
    invoice: String,
    quoted_total_sat: u64,
    source_currency: &MavapayUnitCurrency,
) -> Result<PrepareSendOk, String> {
    // The quoted total is only a sats figure when the order sells sats.
    if *source_currency != MavapayUnitCurrency::BitcoinSatoshi {
        return Err(format!(
            "the order is quoted in {:?}, not satoshis",
            source_currency
        ));
    }
    let amount_sat = match bolt11_amount_sat(&invoice) {
        Some(_) => None,
        None => Some(quoted_total_sat),
    };
    let prepare = spark
        .prepare_send(invoice, amount_sat)
        .await
        .map_err(|e| e.to_string())?;
    check_prepared_sell(&prepare, quoted_total_sat)?;
    Ok(prepare)
}

/// The most a sell payment's Lightning routing fee may be, in sats.
///
/// The checkout offers "Fulfil from Spark" without showing a fee, so the fee
/// the SDK prepares must stay within what a user would expect a Lightning
/// payment of this size to cost: 1% of the quoted total, and never less than
/// [`SELL_FEE_FLOOR_SAT`] so a small order can still route. Mavapay's own
/// `estimated_routing_fee` isn't used as the bound: its unit hasn't been
/// confirmed against a live quote.
fn max_sell_fee_sat(quoted_total_sat: u64) -> u64 {
    (quoted_total_sat / 100).max(SELL_FEE_FLOOR_SAT)
}

/// See [`max_sell_fee_sat`].
const SELL_FEE_FLOOR_SAT: u64 = 10;

fn check_prepared_sell(prepare: &PrepareSendOk, quoted_total_sat: u64) -> Result<(), String> {
    if prepare.method != "Bolt11Invoice" || prepare.cross_chain.is_some() {
        return Err(format!(
            "the order's invoice prepared as {}, not a Lightning invoice",
            prepare.method
        ));
    }
    if prepare.amount_sat == 0 || prepare.amount_sat > quoted_total_sat {
        return Err(format!(
            "the order's invoice asks for {} sats, not the quoted {}",
            prepare.amount_sat, quoted_total_sat
        ));
    }
    // Nothing else bounds the fee: `send_payment` spends whatever the
    // preview prepared.
    let max_fee = max_sell_fee_sat(quoted_total_sat);
    if prepare.fee_sat > max_fee {
        return Err(format!(
            "paying the order's invoice would cost a {} sat routing fee, more than the {} sats allowed",
            prepare.fee_sat, max_fee
        ));
    }
    Ok(())
}

/// Send a prepared sell payment under `idempotency_key`, and say what came of
/// it.
pub(crate) async fn send_sell_payment(
    spark: &SparkBackend,
    quote_id: String,
    prepare_handle: String,
    idempotency_key: String,
) -> MavapayMessage {
    sell_send_result(
        quote_id,
        spark
            .send_payment(prepare_handle, Some(idempotency_key))
            .await,
    )
}

/// What sending a prepared sell payment came to.
fn sell_send_result(
    quote_id: String,
    result: Result<coincube_spark_protocol::SendPaymentOk, SparkClientError>,
) -> MavapayMessage {
    match result {
        Ok(payment) => MavapayMessage::SellInvoiceFulfilled { quote_id, payment },
        // Nobody said the payment didn't happen: never report it as failed.
        Err(SparkClientError::OutcomeUnknown {
            request_id,
            message,
        }) => MavapayMessage::SellInvoiceOutcomeUnknown {
            quote_id,
            request_id,
            message,
        },
        Err(e) => MavapayMessage::SellSendFailed {
            quote_id,
            error: e.to_string(),
        },
    }
}

fn reconcile_sell_task(
    spark: Arc<SparkBackend>,
    quote_id: String,
    pending: Option<PendingSend>,
) -> iced::Task<view::Message> {
    iced::Task::perform(
        async move {
            match pending {
                Some(pending) => reconcile_pending_send(&spark, &pending).await,
                None => ReconcileOutcome::Inconclusive(
                    "the payment's details are no longer available".to_string(),
                ),
            }
        },
        move |outcome| MavapayMessage::SellPaymentChecked { quote_id, outcome }.into(),
    )
}

fn prepare_sell_task(
    spark: Arc<SparkBackend>,
    quote: &GetQuoteResponse,
) -> iced::Task<view::Message> {
    let quote_id = quote.id.clone();
    let invoice = quote.invoice.clone();
    let total = quote.total_amount_in_source_currency;
    let source_currency = quote.source_currency.clone();
    iced::Task::perform(
        async move { prepare_sell_payment(&spark, invoice, total, &source_currency).await },
        move |result| match result {
            Ok(prepare) => MavapayMessage::SellInvoicePrepared { quote_id, prepare }.into(),
            Err(error) => MavapayMessage::SellPrepareFailed { quote_id, error }.into(),
        },
    )
}

/// The quote a sell answer names, if the message is one.
fn sell_answer_quote(msg: &MavapayMessage) -> Option<&str> {
    match msg {
        MavapayMessage::SellInvoicePrepared { quote_id, .. }
        | MavapayMessage::SellPrepareFailed { quote_id, .. }
        | MavapayMessage::SellInvoiceFulfilled { quote_id, .. }
        | MavapayMessage::SellSendFailed { quote_id, .. }
        | MavapayMessage::SellInvoiceOutcomeUnknown { quote_id, .. }
        | MavapayMessage::SellPaymentChecked { quote_id, .. } => Some(quote_id),
        _ => None,
    }
}

impl MavapayState {
    fn sell_fulfilment(&self) -> Option<&SellFulfilment> {
        match self.steps.last() {
            Some(MavapayFlowStep::Checkout { sell, .. }) => Some(&sell.fulfilment),
            _ => None,
        }
    }

    /// The checkout must stay: its payment is unresolved
    /// ([`SellFulfilment::in_flight`]).
    ///
    /// Except when the Spark bridge is gone and no answer is due. A closed
    /// client is never respawned, so every later check would come back
    /// inconclusive and hold the checkout until the app restarts. Then the
    /// checkout can be left; the payment, if it landed, is in Activity, and
    /// Mavapay's own order status still reports the order.
    pub fn sell_checkout_held(&self) -> bool {
        self.sell_fulfilment().is_some_and(|fulfilment| {
            fulfilment.awaiting_answer()
                || (fulfilment.in_flight() && !self.spark.client().is_closed())
        })
    }

    /// A request for the checkout's payment is with the bridge.
    pub fn sell_awaiting_answer(&self) -> bool {
        self.sell_fulfilment()
            .is_some_and(SellFulfilment::awaiting_answer)
    }
}

pub struct MavapayState {
    pub buy_or_sell: panel::BuyOrSell,
    pub steps: Vec<MavapayFlowStep>,
    pub country: &'static Country,

    pub spark: Arc<SparkBackend>,

    pub(self) sat_amount: u64,        // Unit Amount in BTCSAT
    pub(self) btc_price: Option<f64>, // btc_price_in_unit_currency
}

impl MavapayState {
    pub fn new(
        buy_or_sell: panel::BuyOrSell,
        base: MavapayFlowStep,
        country: &'static Country,
        spark: Arc<SparkBackend>,
    ) -> MavapayState {
        MavapayState {
            buy_or_sell,
            steps: vec![base],
            country,
            sat_amount: 6000,
            btc_price: None,
            spark,
        }
    }

    pub fn update(
        &mut self,
        msg: MavapayMessage,
        coincube_client: &CoincubeClient,
    ) -> Option<iced::Task<view::Message>> {
        match (self.steps.last_mut()?, msg) {
            (_, MavapayMessage::NavigateBack) => {
                // Leaving a checkout while its payment is with the bridge
                // would drop the answer, and with it the only record of
                // whether the order was paid.
                if self.sell_checkout_held() {
                    return None;
                }
                match self.steps.len() {
                    1 => {
                        // `MavapayState` must always have at least one FlowStep
                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::ResetWidget,
                        )));
                    }
                    _ => self.steps.pop(),
                };
            }

            (_, MavapayMessage::GetPrice) => {
                let client = coincube_client.clone();
                let currency = match self.country.code {
                    "KE" => MavapayCurrency::KenyanShilling,
                    "ZA" => MavapayCurrency::SouthAfricanRand,
                    "NG" => MavapayCurrency::NigerianNaira,
                    c => unreachable!("Country {:?} is not supported by Mavapay", c),
                };

                let task = iced::Task::perform(
                    async move { MavapayClient(&client).get_price(currency).await },
                    |result| match result {
                        MavapayApiResult::Success { data } => {
                            MavapayMessage::PriceReceived(data).into()
                        }
                        MavapayApiResult::Error { message } => {
                            view::Message::BuySell(view::BuySellMessage::SessionError(
                                "Unable to get latest Bitcoin price",
                                message,
                            ))
                        }
                    },
                );

                return Some(task);
            }
            (_, MavapayMessage::PriceReceived(res)) => {
                self.btc_price = Some(res.btc_price_in_unit_currency)
            }

            (_, MavapayMessage::NormalizeAmounts) => {
                self.sat_amount = self.sat_amount.clamp(6000, 2_100_000_000_000_000)
            }
            (_, MavapayMessage::SatAmountChanged(sats)) => self.sat_amount = sats.round() as _,
            (_, MavapayMessage::FiatAmountChanged(fiat)) => match self.btc_price {
                Some(price) => {
                    let sat_price = price / 100_000_000.0;
                    self.sat_amount = (fiat / sat_price).round() as _
                }
                None => log::warn!("Unable to update BTC amount, BTC price is unknown"),
            },
            (_, MavapayMessage::SendQuote(request)) => {
                let coincube_client = coincube_client.clone();

                let task = iced::Task::perform(
                    async move {
                        // Step 1: Create quote with Mavapay
                        let quote =
                            match MavapayClient(&coincube_client).create_quote(request).await {
                                MavapayApiResult::Success { data } => data,
                                MavapayApiResult::Error { message } => return Err(message),
                            };

                        // Step 2: Save quote to coincube-api (fallible)
                        match coincube_client.save_quote(&quote.id, &quote).await {
                            Ok(_) => {
                                log::trace!("[COINCUBE] Successfully saved quote: {}", quote.id)
                            }
                            Err(err) => {
                                log::error!("[COINCUBE] Unable to save quote: {:?}", err)
                            }
                        }

                        Ok(quote)
                    },
                    move |result| match result {
                        Ok(quote) => MavapayMessage::QuoteCreated(quote).into(),
                        Err(e) => view::Message::BuySell(view::BuySellMessage::SessionError(
                            "Unable to create quote",
                            e,
                        )),
                    },
                );

                return Some(task);
            }

            // state specific updates
            (
                MavapayFlowStep::BuyInputFrom {
                    getting_invoice,
                    ln_invoice,
                    sending_quote,
                },
                msg,
            ) => {
                match msg {
                    MavapayMessage::GenerateLightningInvoice => {
                        *getting_invoice = true;

                        let spark = self.spark.clone();
                        let sat_amount = self.sat_amount;

                        let task = iced::Task::perform(
                            async move { create_buy_invoice(&spark, sat_amount).await },
                            |res| match res {
                                Ok(ln_invoice) => {
                                    MavapayMessage::LightningInvoiceReceived(ln_invoice).into()
                                }
                                Err(e) => {
                                    view::Message::BuySell(view::BuySellMessage::SessionError(
                                        "Unable to acquire new invoice",
                                        e.to_string(),
                                    ))
                                }
                            },
                        )
                        .chain(iced::Task::done(MavapayMessage::CreateQuote.into()));

                        return Some(task);
                    }
                    MavapayMessage::LightningInvoiceReceived(invoice) => {
                        *getting_invoice = false;
                        *ln_invoice = Some(invoice);
                    }
                    MavapayMessage::WriteInvoiceToClipboard => {
                        return ln_invoice.as_ref().map(|invoice| {
                            iced::Task::batch([
                                iced::Task::done(view::Message::Clipboard(invoice.clone())),
                                iced::Task::done(view::Message::ShowError(
                                    "Invoice Copied to Clipboard".to_string(),
                                )),
                            ])
                        })
                    }
                    MavapayMessage::CreateQuote => {
                        let Some(invoice) = ln_invoice else {
                            unreachable!()
                        };

                        let beneficiary = Beneficiary::Lightning {
                            ln_invoice: invoice.clone(),
                        };
                        let local_currency = match self.country.code {
                            "KE" => MavapayUnitCurrency::KenyanShillingCent,
                            "NG" => MavapayUnitCurrency::NigerianNairaKobo,
                            "ZA" => MavapayUnitCurrency::SouthAfricanRandCent,
                            iso => unreachable!("Country ({}) is unsupported by Mavapay", iso),
                        };

                        let request = GetQuoteRequest {
                            amount: self.sat_amount,
                            source_currency: local_currency,
                            target_currency: MavapayUnitCurrency::BitcoinSatoshi,
                            // the currency denomination of the `amount` field
                            payment_currency: MavapayUnitCurrency::BitcoinSatoshi,
                            speed: OnchainTransferSpeed::Fast,
                            autopayout: true,
                            customer_internal_fee: None,
                            payment_method: MavapayPaymentMethod::BankTransfer,
                            beneficiary_format: beneficiary.format(),
                            beneficiary,
                        };

                        *sending_quote = true;
                        return Some(iced::Task::done(MavapayMessage::SendQuote(request).into()));
                    }
                    MavapayMessage::QuoteCreated(quote) => {
                        // Set up SSE stream for transaction status updates
                        if let Some(order_id) = quote.order_id.clone() {
                            *sending_quote = false;

                            // proceed to checkout
                            self.steps.push(MavapayFlowStep::Checkout {
                                quote,
                                invoice_qr_code_data: None,
                                fulfilled_order: None,
                                stream_order_id: Some(order_id),
                                spark_balance: None,
                                sell: SellPayment::default(),
                            });
                        } else {
                            return Some(iced::Task::done(view::Message::BuySell(
                                view::BuySellMessage::SessionError(
                                    "Unable to process payment",
                                    "Mavapay Quote created without `order-id`".to_string(),
                                ),
                            )));
                        }
                    }

                    msg => log::warn!("[MAVAPAY] Message Ignored {:?}", msg),
                }
            }

            (
                MavapayFlowStep::SellInputForm {
                    sending_quote,
                    beneficiary,
                    banks,
                    spark_balance,
                },
                msg,
            ) => match msg {
                MavapayMessage::GetBanks => {
                    let code = self.country.code;

                    if code == "KE" {
                        // Kenyan customers use mobile money, not bank transfers
                        return None;
                    }

                    let client = coincube_client.clone();
                    let task = iced::Task::perform(
                        async move { MavapayClient(&client).get_banks(code).await },
                        |result| match result {
                            MavapayApiResult::Success { data } => {
                                MavapayMessage::BanksReceived(data).into()
                            }
                            MavapayApiResult::Error { message } => {
                                view::Message::BuySell(view::BuySellMessage::SessionError(
                                    "Unable to fetch supported banks for your country",
                                    message,
                                ))
                            }
                        },
                    );

                    return Some(task);
                }
                MavapayMessage::BanksReceived(mut b) => {
                    if cfg!(debug_assertions) {
                        match b {
                            MavapayBanks::Nigerian(ref mut banks) => {
                                banks.sort_by(|a, b| a.nip_bank_code.cmp(&b.nip_bank_code))
                            }
                            MavapayBanks::SouthAfrican(ref mut banks) => banks.sort(),
                        }
                    }

                    let b = banks.insert(b);

                    if match b {
                        MavapayBanks::Nigerian(banks) => banks.is_empty(),
                        MavapayBanks::SouthAfrican(banks) => banks.is_empty(),
                    } {
                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::SessionError(
                                "Can't proceed with flow",
                                "Unable to fetch banks from API".to_string(),
                            ),
                        )));
                    }
                }
                MavapayMessage::GetSparkBalance => {
                    let spark = self.spark.clone();
                    let task = iced::Task::perform(
                        async move { spark.get_info().await.map(|info| info.balance_sats) },
                        |result| result,
                    )
                    .then(|result| match result {
                        Ok(balance) => {
                            iced::Task::done(MavapayMessage::ReceivedSparkBalance(balance).into())
                        }
                        // Without a balance the checkout simply offers the
                        // deposit QR, which works from any wallet, so this
                        // isn't worth interrupting the flow over.
                        Err(e) => {
                            log::warn!("[MAVAPAY] Unable to fetch Spark balance: {}", e);
                            iced::Task::none()
                        }
                    });

                    return Some(task);
                }
                MavapayMessage::ReceivedSparkBalance(sats) => *spark_balance = Some(sats),

                MavapayMessage::BeneficiaryFieldUpdate(field, data) => match (field, beneficiary) {
                    // NG
                    (
                        "NGN.bank_account_number",
                        Beneficiary::NGN {
                            bank_account_number,
                            bank_account_name,
                            ..
                        },
                    ) => {
                        *bank_account_number = data;
                        *bank_account_name = None;
                    }
                    (
                        "NGN.bank_code",
                        Beneficiary::NGN {
                            bank_code,
                            bank_name,
                            bank_account_name,
                            ..
                        },
                    ) => {
                        if let Some(MavapayBanks::Nigerian(ngn_banks)) = banks {
                            if let Some(bank) = ngn_banks.iter().find(|b| b.nip_bank_code == data) {
                                *bank_code = bank.nip_bank_code.clone();
                                *bank_name = bank.bank_name.clone();

                                *bank_account_name = None;
                            }
                        }
                    }
                    // ZA
                    (
                        "ZAR.bank_account_number",
                        Beneficiary::ZAR {
                            bank_account_number,
                            ..
                        },
                    ) => *bank_account_number = data,
                    ("ZAR.name", Beneficiary::ZAR { name, .. }) => *name = data,
                    ("ZAR.bank_name", Beneficiary::ZAR { bank_name, .. }) => *bank_name = data,
                    // KE
                    (
                        "KES.account_name",
                        Beneficiary::KES(KenyanBeneficiary::PayToPhone { account_name, .. }),
                    ) => *account_name = data,
                    (
                        "KES.phone_number",
                        Beneficiary::KES(KenyanBeneficiary::PayToPhone { phone_number, .. }),
                    ) => *phone_number = data,

                    (field, ..) => {
                        log::warn!("Field Edit: `{}` ignored for Beneficiary", field)
                    }
                },
                MavapayMessage::VerifyNgnBankDetails => {
                    if let Beneficiary::NGN {
                        bank_account_number,
                        bank_code,
                        ..
                    } = beneficiary
                    {
                        let client = coincube_client.clone();
                        let ban = bank_account_number.clone();
                        let bac = bank_code.clone();

                        let task = iced::Task::perform(
                            async move {
                                MavapayClient(&client)
                                    .ngn_customer_inquiry(&ban, &bac)
                                    .await
                            },
                            |res| match res {
                                MavapayApiResult::Success { data } => {
                                    MavapayMessage::VerifiedNgnBankDetails(data).into()
                                }
                                MavapayApiResult::Error { message } => {
                                    view::Message::BuySell(view::BuySellMessage::SessionError(
                                        "Unable to verify Bank Details",
                                        message,
                                    ))
                                }
                            },
                        );

                        return Some(task);
                    }
                }
                MavapayMessage::VerifiedNgnBankDetails(details) => {
                    if let Beneficiary::NGN {
                        bank_account_name, ..
                    } = beneficiary
                    {
                        *bank_account_name = Some(details.account_name);
                    }
                }

                MavapayMessage::CreateQuote => {
                    *sending_quote = true;

                    let local_currency = match self.country.code {
                        "KE" => MavapayUnitCurrency::KenyanShillingCent,
                        "NG" => MavapayUnitCurrency::NigerianNairaKobo,
                        "ZA" => MavapayUnitCurrency::SouthAfricanRandCent,
                        iso => unreachable!("Country ({}) is unsupported by Mavapay", iso),
                    };

                    let request = GetQuoteRequest {
                        amount: self.sat_amount,
                        source_currency: MavapayUnitCurrency::BitcoinSatoshi,
                        target_currency: local_currency,
                        // the currency denomination of the `amount` field
                        payment_currency: MavapayUnitCurrency::BitcoinSatoshi,
                        speed: OnchainTransferSpeed::Fast,
                        autopayout: true,
                        customer_internal_fee: None,
                        payment_method: MavapayPaymentMethod::Lightning,
                        beneficiary_format: beneficiary.format(),
                        beneficiary: beneficiary.clone(),
                    };

                    return Some(iced::Task::done(MavapayMessage::SendQuote(request).into()));
                }
                MavapayMessage::QuoteCreated(quote) => {
                    if let Some(order_id) = quote.order_id.clone() {
                        *sending_quote = false;

                        let spark_balance = *spark_balance;
                        let invoice_qr_code_data =
                            iced::widget::qr_code::Data::new(quote.invoice.as_bytes()).ok();

                        // proceed to checkout
                        self.steps.push(MavapayFlowStep::Checkout {
                            quote,
                            invoice_qr_code_data,
                            fulfilled_order: None,
                            stream_order_id: Some(order_id),
                            spark_balance,
                            sell: SellPayment::default(),
                        });
                    } else {
                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::SessionError(
                                "Unable to process payment",
                                "Mavapay Quote created without `order-id`".to_string(),
                            ),
                        )));
                    }
                }
                _ => {}
            },

            (
                MavapayFlowStep::Checkout {
                    quote,
                    fulfilled_order,
                    stream_order_id,
                    sell,
                    ..
                },
                msg,
            ) => match msg {
                // An answer for another order's payment (one this checkout
                // replaced, or a panel rebuilt mid-send) is not this order's.
                msg if sell_answer_quote(&msg).is_some_and(|id| id != quote.id) => {
                    log::warn!(
                        "[MAVAPAY] Ignoring a sell payment answer for another order: {:?}",
                        msg
                    );
                }
                MavapayMessage::FulfillSellInvoice => {
                    // Only from rest: a click while a payment is with the
                    // bridge, or after one whose outcome is unknown, must not
                    // start another.
                    if matches!(self.buy_or_sell, panel::BuyOrSell::Sell)
                        && sell.fulfilment == SellFulfilment::Idle
                    {
                        sell.fulfilment = SellFulfilment::Preparing;
                        return Some(prepare_sell_task(self.spark.clone(), quote));
                    }
                }
                MavapayMessage::SellInvoicePrepared { prepare, .. } => {
                    if sell.fulfilment != SellFulfilment::Preparing {
                        log::warn!(
                            "[MAVAPAY] Ignoring a sell payment preview nobody is waiting for"
                        );
                        return None;
                    }
                    let guard = retry_guard_for(&prepare);
                    // A key is only ever held between a dispatch whose outcome
                    // is unknown and its settlement, so holding one means this
                    // repeats that attempt. A repeat is only safe while the key
                    // guards it; if this preview would have the bridge drop the
                    // key (a token leg), stop and leave the user to check.
                    if sell.idempotency_key.is_some() && guard != RetryGuard::IdempotencyKey {
                        sell.fulfilment = SellFulfilment::OutcomeUnknown {
                            checking: false,
                            outcome: Some(ReconcileOutcome::Inconclusive(
                                "this payment can no longer be repeated safely".to_string(),
                            )),
                            guard: RetryGuard::None,
                        };
                        return None;
                    }
                    // Reused, not re-minted, when this repeats an attempt
                    // whose outcome was unknown: the key is what stops the
                    // SDK paying the invoice twice.
                    let key = sell
                        .idempotency_key
                        .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
                        .clone();
                    // Recorded before dispatch: if no answer comes, this is
                    // all the state check has to go on.
                    sell.pending = Some(PendingSend {
                        request_id: None,
                        dispatched_at: chrono::Utc::now().timestamp(),
                        amount_sat: prepare.amount_sat,
                        token_amount: None,
                        // A state check may only settle the order on a
                        // payment of *this* invoice.
                        invoice: Some(quote.invoice.clone()),
                        guard,
                    });
                    sell.fulfilment = SellFulfilment::Sending;

                    let spark = self.spark.clone();
                    let handle = prepare.handle;
                    let quote_id = quote.id.clone();
                    return Some(iced::Task::perform(
                        async move { send_sell_payment(&spark, quote_id, handle, key).await },
                        view::Message::from,
                    ));
                }
                MavapayMessage::SellPrepareFailed { error: err, .. } => {
                    if sell.fulfilment != SellFulfilment::Preparing {
                        return None;
                    }
                    // A preview failing says nothing about an earlier attempt
                    // it may be repeating. If one was dispatched, its outcome
                    // is still unknown: keep the key and the intent, and
                    // offer the check again rather than the manual invoice.
                    if let Some(pending) = sell.pending.as_ref() {
                        sell.fulfilment = SellFulfilment::OutcomeUnknown {
                            checking: false,
                            outcome: Some(ReconcileOutcome::Inconclusive(format!(
                                "the repeat couldn't be prepared: {err}"
                            ))),
                            guard: pending.guard,
                        };
                        return None;
                    }
                    // Nothing was ever sent, so paying is offered again.
                    sell.fulfilment = SellFulfilment::Idle;
                    return Some(iced::Task::done(view::Message::BuySell(
                        view::BuySellMessage::SessionError("Unable to fulfil invoice", err),
                    )));
                }
                MavapayMessage::SellInvoiceFulfilled { payment, .. } => {
                    // Only the send this checkout is waiting for can settle it.
                    if sell.fulfilment != SellFulfilment::Sending {
                        log::warn!(
                            "[MAVAPAY] Ignoring a sell payment result nobody is waiting for"
                        );
                        return None;
                    }
                    // Accepted is not paid: a pending payment may still fail.
                    // Keep the key and the intent and check its state until
                    // it settles one way or the other.
                    if !payment.is_completed() {
                        log::info!(
                            "[MAVAPAY] Sell payment accepted but not settled ({:?})",
                            payment.status
                        );
                        let guard = sell
                            .pending
                            .as_ref()
                            .map(|p| p.guard)
                            .unwrap_or(RetryGuard::None);
                        sell.fulfilment = SellFulfilment::OutcomeUnknown {
                            checking: true,
                            outcome: None,
                            guard,
                        };
                        return Some(reconcile_sell_task(
                            self.spark.clone(),
                            quote.id.clone(),
                            sell.pending.clone(),
                        ));
                    }
                    sell.fulfilment = SellFulfilment::Paid;
                    sell.idempotency_key = None;
                    sell.pending = None;
                    // Generic success toasts stay mounted when privacy changes.
                    // Keep this asynchronous notification amount-free.
                    return Some(iced::Task::done(view::Message::ShowSuccess(
                        "Invoice paid from your Spark wallet".to_owned(),
                    )));
                }
                MavapayMessage::SellSendFailed { error: err, .. } => {
                    // The bridge answered "no": nothing was paid, so the
                    // ordinary retry applies, under a fresh key as the Send
                    // panel does after a definite rejection.
                    if sell.fulfilment == SellFulfilment::Sending {
                        sell.fulfilment = SellFulfilment::Idle;
                        sell.idempotency_key = None;
                        sell.pending = None;
                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::SessionError("Unable to fulfil invoice", err),
                        )));
                    }
                }
                MavapayMessage::SellInvoiceOutcomeUnknown {
                    request_id,
                    message,
                    ..
                } => {
                    if sell.fulfilment != SellFulfilment::Sending {
                        log::warn!(
                            "[MAVAPAY] Ignoring a sell payment result nobody is waiting for"
                        );
                        return None;
                    }
                    log::warn!("[MAVAPAY] Sell payment outcome unknown: {}", message);
                    // Keep the key and the intent; add the request id so the
                    // bridge's late answer can still be claimed.
                    let pending = sell.pending.get_or_insert_with(|| PendingSend {
                        request_id: None,
                        dispatched_at: chrono::Utc::now().timestamp(),
                        amount_sat: 0,
                        token_amount: None,
                        invoice: None,
                        guard: RetryGuard::None,
                    });
                    pending.request_id = Some(request_id);
                    let guard = pending.guard;
                    sell.fulfilment = SellFulfilment::OutcomeUnknown {
                        checking: true,
                        outcome: None,
                        guard,
                    };
                    return Some(reconcile_sell_task(
                        self.spark.clone(),
                        quote.id.clone(),
                        sell.pending.clone(),
                    ));
                }
                MavapayMessage::CheckSellPayment => {
                    if let SellFulfilment::OutcomeUnknown {
                        checking: false,
                        guard,
                        ..
                    } = sell.fulfilment
                    {
                        sell.fulfilment = SellFulfilment::OutcomeUnknown {
                            checking: true,
                            outcome: None,
                            guard,
                        };
                        return Some(reconcile_sell_task(
                            self.spark.clone(),
                            quote.id.clone(),
                            sell.pending.clone(),
                        ));
                    }
                }
                MavapayMessage::SellPaymentChecked { outcome, .. } => {
                    // Only a check this checkout started and is waiting for.
                    let SellFulfilment::OutcomeUnknown {
                        checking: true,
                        guard,
                        ..
                    } = sell.fulfilment
                    else {
                        return None;
                    };
                    match outcome {
                        ReconcileOutcome::Landed => {
                            sell.fulfilment = SellFulfilment::Paid;
                            sell.idempotency_key = None;
                            sell.pending = None;
                            return Some(iced::Task::done(view::Message::ShowSuccess(
                                "Invoice paid from your Spark wallet".to_owned(),
                            )));
                        }
                        // The bridge's own late answer was "rejected": a
                        // definite failure, so the ordinary retry applies.
                        ReconcileOutcome::DefinitelyFailed(reason) => {
                            sell.fulfilment = SellFulfilment::Idle;
                            sell.idempotency_key = None;
                            sell.pending = None;
                            return Some(iced::Task::done(view::Message::BuySell(
                                view::BuySellMessage::SessionError(
                                    "Unable to fulfil invoice",
                                    reason,
                                ),
                            )));
                        }
                        outcome => {
                            sell.fulfilment = SellFulfilment::OutcomeUnknown {
                                checking: false,
                                outcome: Some(outcome),
                                guard,
                            };
                        }
                    }
                }
                MavapayMessage::RetrySellPayment => {
                    // Only after a check found no trace, on a rail where the
                    // key guards a repeat. Re-prepares the same invoice and
                    // sends it under the same key.
                    if sell.fulfilment.may_resend() {
                        debug_assert!(
                            sell.idempotency_key.is_some(),
                            "a repeat must reuse the original key"
                        );
                        sell.fulfilment = SellFulfilment::Preparing;
                        return Some(prepare_sell_task(self.spark.clone(), quote));
                    }
                }

                MavapayMessage::WriteInvoiceToClipboard => {
                    return Some(iced::Task::batch([
                        iced::Task::done(view::Message::Clipboard(quote.invoice.clone())),
                        iced::Task::done(view::Message::ShowError(
                            "Invoice Copied to Clipboard".to_string(),
                        )),
                    ]));
                }

                MavapayMessage::StreamConnected => {
                    log::info!("[MAVAPAY SSE] Connected to transaction stream");
                }
                MavapayMessage::EventSourceDisconnected(msg) => {
                    log::info!("[MAVAPAY SSE] Stream disconnected: {}", msg);
                }
                MavapayMessage::StreamError(err) => {
                    log::error!("[MAVAPAY SSE] Stream error: {}", err);
                }

                MavapayMessage::TransactionUpdated(update) => {
                    log::info!(
                        "[MAVAPAY-SSE] Order({}): event={} status={:?}",
                        update.order_id,
                        update.event_type,
                        update.status
                    );

                    // A stream left over from an earlier checkout can still
                    // deliver: only this checkout's own order may move it.
                    if quote.order_id.as_deref() != Some(update.order_id.as_str()) {
                        log::warn!(
                            "[MAVAPAY-SSE] Ignoring an update for Order({}): this checkout is for {:?}",
                            update.order_id,
                            quote.order_id
                        );
                        return None;
                    }

                    if matches!(
                        update.status,
                        TransactionStatus::Paid | TransactionStatus::Success
                    ) {
                        let client = coincube_client.clone();
                        let order_id = update.order_id.clone();

                        let task = iced::Task::perform(
                            async move {
                                let mut res = MavapayClient(&client).get_order(&order_id).await;

                                for attempt in 0..10 {
                                    match res.as_ref() {
                                        Ok(..) => break,
                                        Err(err) => {
                                            log::error!(
                                                "[MAVAPAY] Unable to get order information: {:?}, Attempts: {}",
                                                err, 5 - attempt
                                            );

                                            tokio::time::sleep(std::time::Duration::from_secs(5))
                                                .await
                                        }
                                    }

                                    res = MavapayClient(&client).get_order(&order_id).await;
                                }

                                res
                            },
                            |result| match result {
                                Ok(order) => MavapayMessage::QuoteFulfilled(order).into(),
                                Err(err) => {
                                    view::Message::BuySell(view::BuySellMessage::SessionError(
                                        "Failed to fetch order details",
                                        err.to_string(),
                                    ))
                                }
                            },
                        );

                        return Some(task);
                    }
                }

                MavapayMessage::QuoteFulfilled(order) => {
                    // Only this checkout's own order may change it: a fetch
                    // started for an earlier order can outlive that checkout
                    // and land here. Showing it would put another order's
                    // receipt on this screen and stop this order's stream.
                    let this_order = quote.order_id.as_deref() == Some(order.order_id.as_str())
                        || order.quote_id.as_deref() == Some(quote.id.as_str());
                    if !this_order {
                        log::warn!(
                            "[MAVAPAY] Ignoring Order({}) fulfilment: this checkout is Quote({})",
                            order.order_id,
                            quote.id
                        );
                        return None;
                    }
                    log::info!(
                        "[MAVAPAY] Quote({}) has been fulfilled: Order = {:?}",
                        quote.id,
                        order
                    );
                    *fulfilled_order = Some(order);
                    *stream_order_id = None;
                    // Mavapay says this order is paid, so its invoice is: a
                    // sell payment whose outcome was unknown landed, and there
                    // is nothing left to repeat. One still awaiting an answer
                    // settles when that answer comes.
                    if matches!(sell.fulfilment, SellFulfilment::OutcomeUnknown { .. }) {
                        log::info!("[MAVAPAY] Mavapay confirmed the order: sell payment settled");
                        sell.fulfilment = SellFulfilment::Paid;
                        sell.idempotency_key = None;
                        sell.pending = None;
                    }
                }

                #[cfg(debug_assertions)]
                MavapayMessage::SimulatePayIn => {
                    let Some(order_id) = quote.order_id.clone() else {
                        log::error!("[MAVAPAY] Cannot simulate Pay-In: Quote has no order_id");
                        return None;
                    };

                    log::info!(
                        "[MAVAPAY] Simulating Pay-In for Quote({}), Order ID({})",
                        quote.id,
                        order_id
                    );

                    let client = coincube_client.clone();
                    let request = SimulatePayInRequest {
                        order_id,
                        amount: matches!(self.buy_or_sell, panel::BuyOrSell::Buy)
                            .then_some(quote.amount_in_source_currency),
                        currency: (&quote.source_currency).into(),
                    };

                    let task = iced::Task::perform(
                        async move { MavapayClient(&client).simulate_pay_in(&request).await },
                        |s| match s {
                            MavapayApiResult::Success { data } => {
                                log::info!("[MAVAPAY] {}", data)
                            }
                            MavapayApiResult::Error { message } => {
                                log::error!("[MAVAPAY] Unable to simulate Pay-In: {}", message)
                            }
                        },
                    );

                    return Some(task.then(|_| iced::Task::none()));
                }

                #[cfg(not(debug_assertions))]
                MavapayMessage::SimulatePayIn => {
                    log::warn!(
                                "[MAVAPAY] Unable to simulate pay-in for Quote({}), app built in release mode",
                                quote.id,
                            );
                }

                msg => log::warn!("[MAVAPAY] Message Ignored {:?}", msg),
            },

            (
                MavapayFlowStep::History {
                    transactions,
                    loading,
                },
                msg,
            ) => match msg {
                MavapayMessage::FetchTransactions => {
                    *loading = true;

                    let client = coincube_client.clone();
                    let task = iced::Task::perform(
                        async move { MavapayClient(&client).get_transactions().await },
                        |result| match result {
                            Ok(data) => {
                                MavapayMessage::TransactionsReceived(data.transactions).into()
                            }
                            Err(err) => view::Message::BuySell(view::BuySellMessage::SessionError(
                                "Failed to fetch transactions",
                                err.to_string(),
                            )),
                        },
                    );

                    return Some(task);
                }
                MavapayMessage::TransactionsReceived(received_transactions) => {
                    log::info!(
                        "[MAVAPAY] Received {} transactions",
                        received_transactions.len()
                    );
                    *transactions = Some(received_transactions);
                    *loading = false;
                }
                MavapayMessage::SelectTransaction(idx) => {
                    let Some(transaction) = transactions.as_mut().and_then(|t| t.get(idx)) else {
                        log::warn!("[MAVAPAY] Selected transaction index is out of bounds");
                        return None;
                    };

                    let order_id = transaction.order_id.clone();
                    let client = coincube_client.clone();

                    let task = iced::Task::perform(
                        async move { MavapayClient(&client).get_order(&order_id).await },
                        |result| match result {
                            Ok(data) => MavapayMessage::OrderReceived(data).into(),
                            Err(err) => view::Message::BuySell(view::BuySellMessage::SessionError(
                                "Failed to fetch order details",
                                err.to_string(),
                            )),
                        },
                    );

                    let transaction = transaction.clone();
                    self.steps.push(MavapayFlowStep::OrderDetail {
                        transaction,
                        order: None,
                        loading: true,
                    });

                    return Some(task);
                }
                msg => log::warn!("[MAVAPAY] Message Ignored {:?}", msg),
            },

            (MavapayFlowStep::OrderDetail { order, loading, .. }, msg) => match msg {
                MavapayMessage::OrderReceived(received_order) => {
                    *order = Some(received_order);
                    *loading = false;
                }
                msg => log::warn!("[MAVAPAY] Message Ignored {:?}", msg),
            },
        }

        None
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::breez_spark::SparkClient;
    use coincube_spark_protocol::{
        ListPaymentsOk, Method, OkPayload, PaymentSummary, ReceivePaymentOk, Request, SendPaymentOk,
    };
    use tokio::sync::mpsc;

    /// A real, signed BOLT11 invoice: the spec's 0.025 BTC test vector.
    const SPEC_VECTOR_25M: &str = "lnbc25m1pvjluezpp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rq\
                                   wzqfqypqdq5vdhkven9v5sxyetpdeessp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3\
                                   zyg3zyg3zyg3zyg3zyg3zygs9q5sqqqqqqqqqqqqqqqpqsq67gye39hfg3zd8r\
                                   gc80k32tvy9xk2xunwm5lzexnvpx6fd77en8qaq424dxgt56cag2dpt359k3ss\
                                   yhetktkpqh24jqnjyw6uqd08sgptq44qu";
    const INVOICE_SATS: u64 = 2_500_000;

    fn fake_spark() -> (Arc<SparkBackend>, mpsc::UnboundedReceiver<Request>) {
        let (client, requests) = SparkClient::mock_transport();
        (Arc::new(SparkBackend::new(Arc::new(client))), requests)
    }

    /// Play the bridge: take the next request and answer it (`None` drops
    /// the waiter, which is how a bridge that dies mid-request looks).
    async fn bridge_answers(
        spark: &SparkBackend,
        requests: &mut mpsc::UnboundedReceiver<Request>,
        answer: Option<OkPayload>,
    ) -> Method {
        let request = requests.recv().await.expect("a request was sent");
        spark.client().mock_reply(request.id, answer).await;
        request.method
    }

    fn country() -> &'static Country {
        crate::services::coincube::get_countries()
            .iter()
            .find(|country| country.code == "NG")
            .expect("fixture country")
    }

    fn quote() -> GetQuoteResponse {
        sell_quote("quote-sell")
    }

    /// A sats-sourced sell quote paying [`SPEC_VECTOR_25M`].
    pub(crate) fn sell_quote(id: &str) -> GetQuoteResponse {
        GetQuoteResponse {
            id: id.to_string(),
            order_id: Some(format!("order-{id}")),
            exchange_rate: 1.0,
            usd_to_target_currency_rate: 1.0,
            source_currency: MavapayUnitCurrency::BitcoinSatoshi,
            target_currency: MavapayUnitCurrency::NigerianNairaKobo,
            transaction_fees_in_source_currency: 0,
            transaction_fees_in_target_currency: 0,
            amount_in_source_currency: INVOICE_SATS,
            amount_in_target_currency: 6_000,
            payment_method: MavapayPaymentMethod::Lightning,
            expiry: "2026-01-01T00:00:00Z".to_string(),
            is_valid: true,
            invoice: SPEC_VECTOR_25M.to_string(),
            hash: "hash".to_string(),
            total_amount_in_source_currency: INVOICE_SATS,
            total_amount_in_target_currency: Some(6_000),
            customer_internal_fee: 0,
            created_at: None,
            updated_at: None,
            estimated_routing_fee: None,
            bank_name: None,
            ngn_bank_account_number: None,
            ngn_account_name: None,
            ngn_bank_code: None,
        }
    }

    fn sell_checkout(spark: Arc<SparkBackend>) -> MavapayState {
        MavapayState::new(
            panel::BuyOrSell::Sell,
            MavapayFlowStep::Checkout {
                quote: quote(),
                fulfilled_order: None,
                invoice_qr_code_data: None,
                spark_balance: Some(INVOICE_SATS * 2),
                sell: SellPayment::default(),
                stream_order_id: None,
            },
            country(),
            spark,
        )
    }

    fn sell(state: &MavapayState) -> &SellPayment {
        match state.steps.last() {
            Some(MavapayFlowStep::Checkout { sell, .. }) => sell,
            other => panic!("expected a checkout, got {:?}", other),
        }
    }

    fn prepared(has_token_leg: Option<bool>) -> PrepareSendOk {
        PrepareSendOk {
            handle: "prepare-1".to_string(),
            amount_sat: INVOICE_SATS,
            fee_sat: 3,
            method: "Bolt11Invoice".to_string(),
            cross_chain: None,
            has_token_leg,
        }
    }

    fn sell_prepared(prepare: PrepareSendOk) -> MavapayMessage {
        MavapayMessage::SellInvoicePrepared {
            quote_id: "quote-sell".to_string(),
            prepare,
        }
    }

    fn checked_msg(outcome: ReconcileOutcome) -> MavapayMessage {
        MavapayMessage::SellPaymentChecked {
            quote_id: "quote-sell".to_string(),
            outcome,
        }
    }

    fn outgoing(amount_sat: u64) -> PaymentSummary {
        PaymentSummary {
            id: "pay-1".to_string(),
            amount_sat: -(amount_sat as i64),
            fees_sat: 3,
            token_amount: None,
            token_decimals: None,
            token_ticker: None,
            timestamp: chrono::Utc::now().timestamp() as u64,
            // The bridge's wire values: the SDK's Debug names.
            status: "Completed".to_string(),
            direction: "Send".to_string(),
            method: "lightning".to_string(),
            description: None,
            // This order's invoice: a check settles the order only on a
            // payment of it.
            details: coincube_spark_protocol::PaymentDetailSummary {
                invoice: Some(SPEC_VECTOR_25M.to_string()),
                ..Default::default()
            },
        }
    }

    /// Drive a sell checkout from "Fulfil from Spark" to a dispatched send,
    /// playing the bridge's prepare. Returns the send's own request.
    async fn dispatch(
        state: &mut MavapayState,
        spark: &Arc<SparkBackend>,
        requests: &mut mpsc::UnboundedReceiver<Request>,
        prepare: PrepareSendOk,
    ) {
        let client = crate::services::coincube::CoincubeClient::new();
        assert!(state
            .update(MavapayMessage::FulfillSellInvoice, &client)
            .is_some());
        assert_eq!(sell(state).fulfilment, SellFulfilment::Preparing);

        let (result, method) = tokio::join!(
            prepare_sell_payment(
                spark,
                SPEC_VECTOR_25M.to_string(),
                INVOICE_SATS,
                &MavapayUnitCurrency::BitcoinSatoshi
            ),
            bridge_answers(spark, requests, Some(OkPayload::PrepareSend(prepare)))
        );
        // The invoice carries its amount, so none is passed alongside it.
        assert!(matches!(
            method,
            Method::PrepareSend(ref p) if p.input == SPEC_VECTOR_25M && p.amount_sat.is_none()
        ));
        let prepare = result.expect("the prepare matches the quote");
        assert!(state.update(sell_prepared(prepare), &client).is_some());
        assert_eq!(sell(state).fulfilment, SellFulfilment::Sending);
    }

    /// Buy: the Lightning invoice Mavapay pays is a Spark BOLT11 invoice for
    /// the bought amount, so the sats land in Spark.
    #[tokio::test]
    async fn buy_invoice_comes_from_spark_receive() {
        let (spark, mut requests) = fake_spark();
        let (result, method) = tokio::join!(
            create_buy_invoice(&spark, 6_000),
            bridge_answers(
                &spark,
                &mut requests,
                Some(OkPayload::ReceivePayment(ReceivePaymentOk {
                    payment_request: "lnbc60u1sparkinvoice".to_string(),
                    fee_sat: 0,
                }))
            )
        );
        let Method::ReceiveBolt11(params) = method else {
            panic!("expected a Spark BOLT11 receive, got {:?}", method);
        };
        assert_eq!(params.amount_sat, Some(6_000));
        assert_eq!(result.as_deref(), Ok("lnbc60u1sparkinvoice"));
    }

    /// Sell happy path: Spark funded → invoice prepared and paid under a key →
    /// success, with the key retired.
    #[tokio::test]
    async fn sell_pays_the_invoice_from_spark_once() {
        let (spark, mut requests) = fake_spark();
        let mut state = sell_checkout(spark.clone());
        dispatch(&mut state, &spark, &mut requests, prepared(Some(false))).await;

        let key = sell(&state)
            .idempotency_key
            .clone()
            .expect("a key was minted");
        assert_eq!(
            sell(&state).pending.as_ref().map(|p| p.guard),
            Some(RetryGuard::IdempotencyKey)
        );
        // A second click while the send is out starts nothing.
        let client = crate::services::coincube::CoincubeClient::new();
        assert!(state
            .update(MavapayMessage::FulfillSellInvoice, &client)
            .is_none());
        // Nor can the user leave the checkout and lose the answer.
        assert!(state
            .update(MavapayMessage::NavigateBack, &client)
            .is_none());
        assert_eq!(state.steps.len(), 1);

        let (outcome, method) = tokio::join!(
            send_sell_payment(
                &spark,
                "quote-sell".to_string(),
                "prepare-1".to_string(),
                key.clone()
            ),
            bridge_answers(
                &spark,
                &mut requests,
                Some(OkPayload::SendPayment(SendPaymentOk {
                    payment_id: "pay-1".to_string(),
                    amount_sat: INVOICE_SATS,
                    fee_sat: 3,
                    status: Some("Completed".to_string()),
                }))
            )
        );
        assert!(matches!(
            method,
            Method::SendPayment(ref p)
                if p.prepare_handle == "prepare-1" && p.idempotency_key.as_deref() == Some(key.as_str())
        ));
        assert!(matches!(
            outcome,
            MavapayMessage::SellInvoiceFulfilled { .. }
        ));
        assert!(state.update(outcome, &client).is_some());
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Paid);
        assert!(sell(&state).idempotency_key.is_none());
        // Paid: nothing offers to pay again.
        assert!(state
            .update(MavapayMessage::FulfillSellInvoice, &client)
            .is_none());
        assert!(requests.try_recv().is_err(), "no further request was sent");
    }

    /// A preview that isn't the reviewed order is refused before anything is
    /// sent: more than the quoted total, or not a Lightning payment.
    #[test]
    fn a_prepared_sell_must_match_the_quote() {
        assert!(check_prepared_sell(&prepared(Some(false)), INVOICE_SATS).is_ok());
        assert!(check_prepared_sell(&prepared(Some(false)), INVOICE_SATS - 1).is_err());
        let mut onchain = prepared(Some(false));
        onchain.method = "BitcoinAddress".to_string();
        assert!(check_prepared_sell(&onchain, INVOICE_SATS).is_err());
        let mut zero = prepared(Some(false));
        zero.amount_sat = 0;
        assert!(check_prepared_sell(&zero, INVOICE_SATS).is_err());
    }

    /// A definite rejection from the bridge returns to rest with a fresh key
    /// for the next attempt, as the Send panel does.
    /// The prepared routing fee is bounded: `send_payment` spends whatever
    /// the preview prepared, and the checkout shows no fee to approve.
    #[test]
    fn a_prepared_sell_fee_is_bounded() {
        let cap = max_sell_fee_sat(INVOICE_SATS);
        assert_eq!(cap, INVOICE_SATS / 100);
        let with_fee = |fee_sat| PrepareSendOk {
            fee_sat,
            ..prepared(Some(false))
        };
        assert!(check_prepared_sell(&with_fee(cap), INVOICE_SATS).is_ok());
        let err = check_prepared_sell(&with_fee(cap + 1), INVOICE_SATS).unwrap_err();
        assert!(err.contains("routing fee"), "{}", err);
        // A small order can still route: the cap never drops below the floor.
        assert_eq!(max_sell_fee_sat(500), SELL_FEE_FLOOR_SAT);
    }

    /// `SendPaymentOk` for a payment the SDK still has pending is not "paid":
    /// the checkout keeps the key and the intent and checks its state.
    #[test]
    fn a_pending_sell_send_is_checked_not_paid() {
        let (spark, _requests) = fake_spark();
        let client = crate::services::coincube::CoincubeClient::new();
        for status in [Some("Pending"), None] {
            let mut state = sell_checkout(spark.clone());
            assert!(state
                .update(MavapayMessage::FulfillSellInvoice, &client)
                .is_some());
            assert!(state
                .update(sell_prepared(prepared(Some(false))), &client)
                .is_some());
            let key = sell(&state).idempotency_key.clone();
            assert!(key.is_some());
            assert_eq!(
                sell(&state).pending.as_ref().unwrap().invoice.as_deref(),
                Some(SPEC_VECTOR_25M),
                "the check may only settle on this order's invoice"
            );

            let accepted = MavapayMessage::SellInvoiceFulfilled {
                quote_id: "quote-sell".to_string(),
                payment: coincube_spark_protocol::SendPaymentOk {
                    payment_id: "pay-1".to_string(),
                    amount_sat: INVOICE_SATS,
                    fee_sat: 3,
                    status: status.map(str::to_string),
                },
            };
            // The state check starts straight away.
            assert!(state.update(accepted, &client).is_some(), "{:?}", status);
            assert!(
                matches!(
                    sell(&state).fulfilment,
                    SellFulfilment::OutcomeUnknown { checking: true, .. }
                ),
                "{status:?}: {:?}",
                sell(&state).fulfilment
            );
            assert_eq!(sell(&state).idempotency_key, key);
            assert!(sell(&state).pending.is_some());
            assert!(sell(&state).fulfilment.in_flight());
        }
    }

    /// A payment whose state couldn't be settled keeps its checkout: until a
    /// verdict the user can act on, leaving would drop the key and the
    /// "Check again" path. `NoTrace`, `Paid` and `Idle` release it.
    #[test]
    fn an_unresolved_sell_payment_stays_in_flight() {
        let unknown = |checking, outcome| SellFulfilment::OutcomeUnknown {
            checking,
            outcome,
            guard: RetryGuard::IdempotencyKey,
        };
        for held in [
            SellFulfilment::Preparing,
            SellFulfilment::Sending,
            unknown(true, None),
            unknown(false, None),
            unknown(
                false,
                Some(ReconcileOutcome::Inconclusive("still in progress".into())),
            ),
        ] {
            assert!(held.in_flight(), "{:?}", held);
        }
        for released in [
            SellFulfilment::Idle,
            SellFulfilment::Paid,
            unknown(false, Some(ReconcileOutcome::NoTrace)),
        ] {
            assert!(!released.in_flight(), "{:?}", released);
        }

        // An inconclusive check no longer lets Back drop the checkout.
        let (spark, _requests) = fake_spark();
        let client = crate::services::coincube::CoincubeClient::new();
        let mut state = sell_checkout(spark);
        set_sell(&mut state, |sell| {
            sell.idempotency_key = Some("key-1".to_string());
            sell.fulfilment = unknown(true, None);
        });
        let _ = state.update(
            checked_msg(ReconcileOutcome::Inconclusive("history unavailable".into())),
            &client,
        );
        let steps = state.steps.len();
        assert!(state
            .update(MavapayMessage::NavigateBack, &client)
            .is_none());
        assert_eq!(state.steps.len(), steps);
        assert_eq!(sell(&state).idempotency_key.as_deref(), Some("key-1"));
    }

    #[test]
    fn a_rejected_sell_send_can_be_tried_again() {
        let rejected = sell_send_result(
            "quote-sell".to_string(),
            Err(SparkClientError::BridgeError {
                kind: coincube_spark_protocol::ErrorKind::Sdk,
                message: "insufficient funds".to_string(),
            }),
        );
        assert!(matches!(rejected, MavapayMessage::SellSendFailed { .. }));

        let (spark, _requests) = fake_spark();
        let mut state = sell_checkout(spark);
        if let Some(MavapayFlowStep::Checkout { sell, .. }) = state.steps.last_mut() {
            sell.fulfilment = SellFulfilment::Sending;
            sell.idempotency_key = Some("k".to_string());
        }
        let client = crate::services::coincube::CoincubeClient::new();
        assert!(state.update(rejected, &client).is_some());
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Idle);
        assert!(sell(&state).idempotency_key.is_none());
    }

    /// Ambiguous failure: the bridge dies after the send was handed over. The
    /// checkout checks the payment's state before anything else, offers no
    /// second payment while it does, and a payment found in the history is
    /// reported as paid — with exactly one send on the wire.
    #[tokio::test]
    async fn an_ambiguous_sell_failure_checks_state_and_never_pays_twice() {
        let (spark, mut requests) = fake_spark();
        let mut state = sell_checkout(spark.clone());
        dispatch(&mut state, &spark, &mut requests, prepared(Some(false))).await;
        let key = sell(&state).idempotency_key.clone().unwrap();

        let (outcome, method) = tokio::join!(
            send_sell_payment(
                &spark,
                "quote-sell".to_string(),
                "prepare-1".to_string(),
                key.clone()
            ),
            bridge_answers(&spark, &mut requests, None)
        );
        assert!(matches!(method, Method::SendPayment(_)));
        assert!(
            matches!(outcome, MavapayMessage::SellInvoiceOutcomeUnknown { .. }),
            "{:?}",
            outcome
        );

        let client = crate::services::coincube::CoincubeClient::new();
        // The unknown outcome starts a state check straight away.
        assert!(state.update(outcome, &client).is_some());
        assert!(matches!(
            sell(&state).fulfilment,
            SellFulfilment::OutcomeUnknown { checking: true, .. }
        ));
        assert_eq!(sell(&state).idempotency_key.as_deref(), Some(key.as_str()));
        assert!(sell(&state).pending.as_ref().unwrap().request_id.is_some());
        for blocked in [
            MavapayMessage::FulfillSellInvoice,
            MavapayMessage::RetrySellPayment,
            MavapayMessage::NavigateBack,
        ] {
            assert!(state.update(blocked, &client).is_none());
        }

        // The check: no late answer, so the history; the payment is there.
        let pending = sell(&state).pending.clone().unwrap();
        let (checked, method) = tokio::join!(
            reconcile_pending_send(&spark, &pending),
            bridge_answers(
                &spark,
                &mut requests,
                Some(OkPayload::ListPayments(ListPaymentsOk {
                    payments: vec![outgoing(INVOICE_SATS)],
                }))
            )
        );
        assert!(matches!(method, Method::ListPayments(_)));
        assert_eq!(checked, ReconcileOutcome::Landed);
        assert!(state.update(checked_msg(checked), &client).is_some());
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Paid);
        assert!(state
            .update(MavapayMessage::FulfillSellInvoice, &client)
            .is_none());
        assert!(
            requests.try_recv().is_err(),
            "exactly one send was dispatched"
        );
    }

    /// No trace after an unknown outcome allows a repeat only under the same
    /// key; an inconclusive check allows none.
    #[test]
    fn a_repeat_after_an_unknown_outcome_reuses_the_key_or_is_refused() {
        let (spark, _requests) = fake_spark();
        let client = crate::services::coincube::CoincubeClient::new();
        let unknown = |state: &mut MavapayState| {
            if let Some(MavapayFlowStep::Checkout { sell, .. }) = state.steps.last_mut() {
                sell.idempotency_key = Some("key-1".to_string());
                sell.fulfilment = SellFulfilment::OutcomeUnknown {
                    checking: true,
                    outcome: None,
                    guard: RetryGuard::IdempotencyKey,
                };
            }
        };

        // Inconclusive: no repeat.
        let mut state = sell_checkout(spark.clone());
        unknown(&mut state);
        let _ = state.update(
            checked_msg(ReconcileOutcome::Inconclusive("x".into())),
            &client,
        );
        assert!(!sell(&state).fulfilment.may_resend());
        assert!(state
            .update(MavapayMessage::RetrySellPayment, &client)
            .is_none());

        // No trace on a key-guarded rail: a repeat, under the same key.
        let mut state = sell_checkout(spark.clone());
        unknown(&mut state);
        let _ = state.update(checked_msg(ReconcileOutcome::NoTrace), &client);
        assert!(state
            .update(MavapayMessage::RetrySellPayment, &client)
            .is_some());
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Preparing);
        assert!(state
            .update(sell_prepared(prepared(Some(false))), &client)
            .is_some());
        assert_eq!(sell(&state).idempotency_key.as_deref(), Some("key-1"));
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Sending);

        // …unless the new preview would have the bridge drop the key.
        let mut state = sell_checkout(spark);
        unknown(&mut state);
        let _ = state.update(checked_msg(ReconcileOutcome::NoTrace), &client);
        let _ = state.update(MavapayMessage::RetrySellPayment, &client);
        assert!(state
            .update(sell_prepared(prepared(Some(true))), &client)
            .is_none());
        assert!(matches!(
            sell(&state).fulfilment,
            SellFulfilment::OutcomeUnknown {
                guard: RetryGuard::None,
                ..
            }
        ));
        assert!(!sell(&state).fulfilment.may_resend());
    }

    /// A repeat whose preview fails says nothing about the attempt it was
    /// repeating: the checkout goes back to "outcome unknown" with the key
    /// and the intent, not to the manual invoice.
    #[test]
    fn a_failed_repeat_preview_keeps_the_unknown_outcome() {
        let (spark, _requests) = fake_spark();
        let client = crate::services::coincube::CoincubeClient::new();
        let mut state = sell_checkout(spark.clone());
        set_sell(&mut state, |sell| {
            sell.idempotency_key = Some("key-1".to_string());
            sell.pending = Some(PendingSend {
                request_id: Some(7),
                dispatched_at: chrono::Utc::now().timestamp(),
                amount_sat: INVOICE_SATS,
                token_amount: None,
                invoice: Some(SPEC_VECTOR_25M.to_string()),
                guard: RetryGuard::IdempotencyKey,
            });
            sell.fulfilment = SellFulfilment::OutcomeUnknown {
                checking: false,
                outcome: Some(ReconcileOutcome::NoTrace),
                guard: RetryGuard::IdempotencyKey,
            };
        });
        assert!(state
            .update(MavapayMessage::RetrySellPayment, &client)
            .is_some());
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Preparing);
        let failed = MavapayMessage::SellPrepareFailed {
            quote_id: "quote-sell".to_string(),
            error: "no route".to_string(),
        };
        let _ = state.update(failed, &client);
        assert!(
            matches!(
                sell(&state).fulfilment,
                SellFulfilment::OutcomeUnknown {
                    checking: false,
                    outcome: Some(ReconcileOutcome::Inconclusive(_)),
                    guard: RetryGuard::IdempotencyKey,
                }
            ),
            "{:?}",
            sell(&state).fulfilment
        );
        assert_eq!(sell(&state).idempotency_key.as_deref(), Some("key-1"));
        assert!(sell(&state).pending.is_some());
        // The check is still on offer.
        assert!(state
            .update(MavapayMessage::CheckSellPayment, &client)
            .is_some());

        // A first preview failing, with nothing ever sent, is a plain retry.
        let mut state = sell_checkout(spark);
        let _ = state.update(MavapayMessage::FulfillSellInvoice, &client);
        let _ = state.update(
            MavapayMessage::SellPrepareFailed {
                quote_id: "quote-sell".to_string(),
                error: "no route".to_string(),
            },
            &client,
        );
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Idle);
    }

    /// Mavapay confirming the order settles a sell payment whose outcome was
    /// unknown: the invoice is paid, so nothing is left to repeat.
    #[test]
    fn mavapay_confirming_the_order_settles_an_unknown_payment() {
        let (spark, _requests) = fake_spark();
        let client = crate::services::coincube::CoincubeClient::new();
        let mut state = sell_checkout(spark);
        set_sell(&mut state, |sell| {
            sell.idempotency_key = Some("key-1".to_string());
            sell.fulfilment = SellFulfilment::OutcomeUnknown {
                checking: false,
                outcome: Some(ReconcileOutcome::Inconclusive("still in progress".into())),
                guard: RetryGuard::IdempotencyKey,
            };
        });
        assert!(state.sell_checkout_held());
        let order = GetOrderResponse {
            id: 1,
            order_id: "order-sell".to_string(),
            quote_id: Some("quote-sell".to_string()),
            amount: INVOICE_SATS,
            status: TransactionStatus::Paid,
            currency: MavapayCurrency::Bitcoin,
            payment_method: MavapayPaymentMethod::Lightning,
            is_valid: Some(true),
            payment_btc_detail: Some(SPEC_VECTOR_25M.to_string()),
            created_at: "2026-11-17T00:00:00Z".to_string(),
            updated_at: "2026-11-17T00:00:00Z".to_string(),
            order_data: None,
        };
        // Another order's confirmation settles nothing here.
        let other = GetOrderResponse {
            order_id: "order-other".to_string(),
            quote_id: Some("quote-other".to_string()),
            ..order.clone()
        };
        if let Some(MavapayFlowStep::Checkout {
            stream_order_id, ..
        }) = state.steps.last_mut()
        {
            *stream_order_id = Some("order-quote-sell".to_string());
        }
        let _ = state.update(MavapayMessage::QuoteFulfilled(other), &client);
        assert!(state.sell_checkout_held());
        assert!(sell(&state).idempotency_key.is_some());
        // …and changes nothing else either: no foreign receipt on this
        // screen, and this order's own stream keeps running.
        match state.steps.last() {
            Some(MavapayFlowStep::Checkout {
                fulfilled_order,
                stream_order_id,
                ..
            }) => {
                assert!(fulfilled_order.is_none());
                assert_eq!(stream_order_id.as_deref(), Some("order-quote-sell"));
            }
            other => panic!("expected a checkout, got {:?}", other),
        }

        let _ = state.update(MavapayMessage::QuoteFulfilled(order), &client);
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Paid);
        assert!(sell(&state).idempotency_key.is_none());
        assert!(!state.sell_checkout_held());
    }

    /// A paid update on the stream for some other order starts no order
    /// fetch here; this checkout's own order does.
    #[test]
    fn stream_updates_for_another_order_are_ignored() {
        let (spark, _requests) = fake_spark();
        let client = crate::services::coincube::CoincubeClient::new();
        let mut state = sell_checkout(spark);
        let update = |order_id: &str| crate::services::mavapay::stream::TransactionUpdate {
            amount: INVOICE_SATS,
            created_at: "2026-11-17T00:00:00Z".to_string(),
            currency: MavapayCurrency::Bitcoin,
            event_type: "transaction.updated".to_string(),
            order_hash: "hash".to_string(),
            order_id: order_id.to_string(),
            status: TransactionStatus::Paid,
            transaction_id: 1,
        };
        assert!(state
            .update(
                MavapayMessage::TransactionUpdated(update("order-other")),
                &client
            )
            .is_none());
        assert!(state
            .update(
                MavapayMessage::TransactionUpdated(update("order-quote-sell")),
                &client
            )
            .is_some());
    }

    /// A dead bridge is never respawned, so its checks can't settle
    /// anything: an unresolved checkout with no answer due lets go rather
    /// than holding until the app restarts. One awaiting an answer still
    /// holds; the dying bridge answers it.
    #[test]
    fn a_dead_bridge_lets_go_of_an_unresolved_checkout() {
        let (spark, _requests) = fake_spark();
        let mut state = sell_checkout(spark.clone());
        set_sell(&mut state, |sell| {
            sell.fulfilment = SellFulfilment::OutcomeUnknown {
                checking: false,
                outcome: Some(ReconcileOutcome::Inconclusive("bridge gone".into())),
                guard: RetryGuard::IdempotencyKey,
            };
        });
        assert!(state.sell_checkout_held());

        spark.client().mock_close();
        assert!(spark.client().is_closed());
        assert!(!state.sell_checkout_held());

        set_sell(&mut state, |sell| sell.fulfilment = SellFulfilment::Sending);
        assert!(state.sell_checkout_held(), "an answer is still due");
    }

    fn set_sell(state: &mut MavapayState, f: impl FnOnce(&mut SellPayment)) {
        if let Some(MavapayFlowStep::Checkout { sell, .. }) = state.steps.last_mut() {
            f(sell);
        }
    }

    /// A late answer for an earlier order (order A) reaching the current
    /// checkout (order B) changes nothing on B: it can't mark B paid, can't
    /// fabricate a pending record, and doesn't stop B's own answer landing.
    #[test]
    fn answers_for_another_order_do_not_touch_this_checkout() {
        let (spark, _requests) = fake_spark();
        let client = crate::services::coincube::CoincubeClient::new();
        let mut state = sell_checkout(spark);
        set_sell(&mut state, |sell| {
            sell.fulfilment = SellFulfilment::Sending;
            sell.idempotency_key = Some("key-b".to_string());
        });
        let order_a = || "quote-a".to_string();
        for stale in [
            MavapayMessage::SellInvoiceFulfilled {
                quote_id: order_a(),
                payment: SendPaymentOk {
                    payment_id: "pay-a".to_string(),
                    amount_sat: INVOICE_SATS,
                    fee_sat: 3,
                    status: Some("Completed".to_string()),
                },
            },
            MavapayMessage::SellInvoiceOutcomeUnknown {
                quote_id: order_a(),
                request_id: 9,
                message: "lost".to_string(),
            },
            MavapayMessage::SellSendFailed {
                quote_id: order_a(),
                error: "no".to_string(),
            },
            MavapayMessage::SellPaymentChecked {
                quote_id: order_a(),
                outcome: ReconcileOutcome::Landed,
            },
            MavapayMessage::SellInvoicePrepared {
                quote_id: order_a(),
                prepare: prepared(Some(false)),
            },
        ] {
            assert!(state.update(stale, &client).is_none());
            assert_eq!(sell(&state).fulfilment, SellFulfilment::Sending);
            assert_eq!(sell(&state).idempotency_key.as_deref(), Some("key-b"));
            assert!(sell(&state).pending.is_none());
        }

        // B's own rejection still applies.
        assert!(state
            .update(
                MavapayMessage::SellSendFailed {
                    quote_id: "quote-sell".to_string(),
                    error: "no route".to_string(),
                },
                &client
            )
            .is_some());
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Idle);
    }

    /// Even for this order, a send result only settles a checkout that is
    /// waiting for one, and a check result only one that is checking.
    #[test]
    fn send_and_check_results_need_the_matching_state() {
        let (spark, _requests) = fake_spark();
        let client = crate::services::coincube::CoincubeClient::new();
        let mut state = sell_checkout(spark);
        let paid = MavapayMessage::SellInvoiceFulfilled {
            quote_id: "quote-sell".to_string(),
            payment: SendPaymentOk {
                payment_id: "pay-1".to_string(),
                amount_sat: INVOICE_SATS,
                fee_sat: 3,
                status: Some("Completed".to_string()),
            },
        };
        assert!(state.update(paid.clone(), &client).is_none());
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Idle);
        assert!(state
            .update(
                MavapayMessage::SellInvoiceOutcomeUnknown {
                    quote_id: "quote-sell".to_string(),
                    request_id: 1,
                    message: "lost".to_string(),
                },
                &client
            )
            .is_none());
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Idle);

        // A finished check isn't re-settled by a stray result.
        set_sell(&mut state, |sell| {
            sell.fulfilment = SellFulfilment::OutcomeUnknown {
                checking: false,
                outcome: Some(ReconcileOutcome::Inconclusive("x".into())),
                guard: RetryGuard::IdempotencyKey,
            };
        });
        assert!(state
            .update(checked_msg(ReconcileOutcome::Landed), &client)
            .is_none());
        assert!(matches!(
            sell(&state).fulfilment,
            SellFulfilment::OutcomeUnknown {
                checking: false,
                ..
            }
        ));
    }

    /// The quoted total is only a sats amount for a sats-sourced order;
    /// anything else is refused before Spark is asked to prepare.
    #[tokio::test]
    async fn a_sell_quoted_in_another_currency_is_refused_before_preparing() {
        let (spark, mut requests) = fake_spark();
        let result = prepare_sell_payment(
            &spark,
            SPEC_VECTOR_25M.to_string(),
            INVOICE_SATS,
            &MavapayUnitCurrency::NigerianNairaKobo,
        )
        .await;
        assert!(result.is_err());
        assert!(requests.try_recv().is_err(), "nothing was prepared");
    }

    /// The sell caller of the shared check: a failed same-size attempt isn't
    /// "paid", and a pending one keeps the payment unconfirmed with no repeat.
    #[tokio::test]
    async fn a_failed_or_pending_history_entry_is_not_a_paid_sell() {
        let (spark, mut requests) = fake_spark();
        let client = crate::services::coincube::CoincubeClient::new();
        let pending = PendingSend {
            request_id: None,
            dispatched_at: chrono::Utc::now().timestamp(),
            amount_sat: INVOICE_SATS,
            token_amount: None,
            invoice: None,
            guard: RetryGuard::IdempotencyKey,
        };
        for (status, may_resend) in [("Failed", true), ("Pending", false)] {
            let mut state = sell_checkout(spark.clone());
            set_sell(&mut state, |sell| {
                sell.idempotency_key = Some("key-1".to_string());
                sell.pending = Some(pending.clone());
                sell.fulfilment = SellFulfilment::OutcomeUnknown {
                    checking: true,
                    outcome: None,
                    guard: RetryGuard::IdempotencyKey,
                };
            });
            let entry = PaymentSummary {
                status: status.to_string(),
                ..outgoing(INVOICE_SATS)
            };
            let (outcome, _) = tokio::join!(
                reconcile_pending_send(&spark, &pending),
                bridge_answers(
                    &spark,
                    &mut requests,
                    Some(OkPayload::ListPayments(ListPaymentsOk {
                        payments: vec![entry],
                    }))
                )
            );
            assert_ne!(outcome, ReconcileOutcome::Landed, "{}", status);
            let _ = state.update(checked_msg(outcome), &client);
            assert!(
                matches!(
                    sell(&state).fulfilment,
                    SellFulfilment::OutcomeUnknown {
                        checking: false,
                        ..
                    }
                ),
                "{}: still unconfirmed, not paid",
                status
            );
            // Failed leaves no trace (a key-guarded repeat is safe); pending
            // may still land, so no repeat.
            assert_eq!(
                sell(&state).fulfilment.may_resend(),
                may_resend,
                "{}",
                status
            );
        }
    }

    /// A same-key repeat of a sell payment that ended `Failed`: the bridge
    /// answers the SDK's stored failed payment with an error, so the checkout
    /// goes back to rest with the key retired — never to Paid.
    #[tokio::test]
    async fn a_same_key_repeat_answered_with_a_failed_payment_is_a_definite_failure() {
        let (spark, mut requests) = fake_spark();
        let client = crate::services::coincube::CoincubeClient::new();
        let mut state = sell_checkout(spark.clone());
        set_sell(&mut state, |sell| {
            sell.fulfilment = SellFulfilment::Sending;
            sell.idempotency_key = Some("key-1".to_string());
        });

        let (outcome, _) = tokio::join!(
            send_sell_payment(
                &spark,
                "quote-sell".to_string(),
                "prepare-2".to_string(),
                "key-1".to_string()
            ),
            async {
                let request = requests.recv().await.expect("the send");
                spark
                    .client()
                    .mock_refuse(
                        request.id,
                        coincube_spark_protocol::ErrorKind::Sdk,
                        "send_payment failed: the payment pay-1 failed",
                    )
                    .await;
            }
        );
        assert!(
            matches!(outcome, MavapayMessage::SellSendFailed { .. }),
            "{:?}",
            outcome
        );
        assert!(state.update(outcome, &client).is_some());
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Idle);
        assert!(sell(&state).idempotency_key.is_none());
        assert!(sell(&state).pending.is_none());
    }
}
