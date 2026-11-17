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
    /// A request is with the bridge, so leaving the checkout would orphan its
    /// answer.
    pub fn in_flight(&self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Sending | Self::OutcomeUnknown { checking: true, .. }
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
) -> Result<PrepareSendOk, String> {
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
    Ok(())
}

/// Send a prepared sell payment under `idempotency_key`, and say what came of
/// it.
pub(crate) async fn send_sell_payment(
    spark: &SparkBackend,
    prepare_handle: String,
    idempotency_key: String,
) -> MavapayMessage {
    sell_send_result(
        spark
            .send_payment(prepare_handle, Some(idempotency_key))
            .await,
    )
}

/// What sending a prepared sell payment came to.
fn sell_send_result(
    result: Result<coincube_spark_protocol::SendPaymentOk, SparkClientError>,
) -> MavapayMessage {
    match result {
        Ok(ok) => MavapayMessage::SellInvoiceFulfilled(ok),
        // Nobody said the payment didn't happen: never report it as failed.
        Err(SparkClientError::OutcomeUnknown {
            request_id,
            message,
        }) => MavapayMessage::SellInvoiceOutcomeUnknown {
            request_id,
            message,
        },
        Err(e) => MavapayMessage::SellSendFailed(e.to_string()),
    }
}

fn reconcile_sell_task(
    spark: Arc<SparkBackend>,
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
        |outcome| MavapayMessage::SellPaymentChecked(outcome).into(),
    )
}

fn prepare_sell_task(
    spark: Arc<SparkBackend>,
    invoice: String,
    quoted_total_sat: u64,
) -> iced::Task<view::Message> {
    iced::Task::perform(
        async move { prepare_sell_payment(&spark, invoice, quoted_total_sat).await },
        |result| match result {
            Ok(prepare) => MavapayMessage::SellInvoicePrepared(prepare).into(),
            Err(e) => MavapayMessage::SellPrepareFailed(e).into(),
        },
    )
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
                if let Some(MavapayFlowStep::Checkout { sell, .. }) = self.steps.last() {
                    if sell.fulfilment.in_flight() {
                        return None;
                    }
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
                MavapayMessage::FulfillSellInvoice => {
                    // Only from rest: a click while a payment is with the
                    // bridge, or after one whose outcome is unknown, must not
                    // start another.
                    if matches!(self.buy_or_sell, panel::BuyOrSell::Sell)
                        && sell.fulfilment == SellFulfilment::Idle
                    {
                        sell.fulfilment = SellFulfilment::Preparing;
                        return Some(prepare_sell_task(
                            self.spark.clone(),
                            quote.invoice.clone(),
                            quote.total_amount_in_source_currency,
                        ));
                    }
                }
                MavapayMessage::SellInvoicePrepared(prepare) => {
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
                        guard,
                    });
                    sell.fulfilment = SellFulfilment::Sending;

                    let spark = self.spark.clone();
                    let handle = prepare.handle;
                    return Some(iced::Task::perform(
                        async move { send_sell_payment(&spark, handle, key).await },
                        view::Message::from,
                    ));
                }
                MavapayMessage::SellPrepareFailed(err) => {
                    // Nothing was sent, so paying is offered again. The key, if
                    // any, stays: a preview failing says nothing about an
                    // earlier attempt it may be repeating.
                    if sell.fulfilment == SellFulfilment::Preparing {
                        sell.fulfilment = SellFulfilment::Idle;
                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::SessionError("Unable to fulfil invoice", err),
                        )));
                    }
                }
                MavapayMessage::SellInvoiceFulfilled(_payment) => {
                    sell.fulfilment = SellFulfilment::Paid;
                    sell.idempotency_key = None;
                    sell.pending = None;
                    // Generic success toasts stay mounted when privacy changes.
                    // Keep this asynchronous notification amount-free.
                    return Some(iced::Task::done(view::Message::ShowSuccess(
                        "Invoice paid from your Spark wallet".to_owned(),
                    )));
                }
                MavapayMessage::SellSendFailed(err) => {
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
                } => {
                    log::warn!("[MAVAPAY] Sell payment outcome unknown: {}", message);
                    // Keep the key and the intent; add the request id so the
                    // bridge's late answer can still be claimed.
                    let pending = sell.pending.get_or_insert_with(|| PendingSend {
                        request_id: None,
                        dispatched_at: chrono::Utc::now().timestamp(),
                        amount_sat: 0,
                        token_amount: None,
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
                            sell.pending.clone(),
                        ));
                    }
                }
                MavapayMessage::SellPaymentChecked(outcome) => {
                    let SellFulfilment::OutcomeUnknown { guard, .. } = sell.fulfilment else {
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
                        return Some(prepare_sell_task(
                            self.spark.clone(),
                            quote.invoice.clone(),
                            quote.total_amount_in_source_currency,
                        ));
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
                    log::info!(
                        "[MAVAPAY] Quote({}) has been fulfilled: Order = {:?}",
                        quote.id,
                        order
                    );

                    *fulfilled_order = Some(order);
                    *stream_order_id = None;
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
mod tests {
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
        GetQuoteResponse {
            id: "quote-sell".to_string(),
            order_id: Some("order-sell".to_string()),
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

    fn outgoing(amount_sat: u64) -> PaymentSummary {
        PaymentSummary {
            id: "pay-1".to_string(),
            amount_sat: -(amount_sat as i64),
            fees_sat: 3,
            token_amount: None,
            token_decimals: None,
            token_ticker: None,
            timestamp: chrono::Utc::now().timestamp() as u64,
            status: "completed".to_string(),
            direction: "outgoing".to_string(),
            method: "lightning".to_string(),
            description: None,
            details: Default::default(),
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
            prepare_sell_payment(spark, SPEC_VECTOR_25M.to_string(), INVOICE_SATS),
            bridge_answers(spark, requests, Some(OkPayload::PrepareSend(prepare)))
        );
        // The invoice carries its amount, so none is passed alongside it.
        assert!(matches!(
            method,
            Method::PrepareSend(ref p) if p.input == SPEC_VECTOR_25M && p.amount_sat.is_none()
        ));
        let prepare = result.expect("the prepare matches the quote");
        assert!(state
            .update(MavapayMessage::SellInvoicePrepared(prepare), &client)
            .is_some());
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
            send_sell_payment(&spark, "prepare-1".to_string(), key.clone()),
            bridge_answers(
                &spark,
                &mut requests,
                Some(OkPayload::SendPayment(SendPaymentOk {
                    payment_id: "pay-1".to_string(),
                    amount_sat: INVOICE_SATS,
                    fee_sat: 3,
                }))
            )
        );
        assert!(matches!(
            method,
            Method::SendPayment(ref p)
                if p.prepare_handle == "prepare-1" && p.idempotency_key.as_deref() == Some(key.as_str())
        ));
        assert!(matches!(outcome, MavapayMessage::SellInvoiceFulfilled(_)));
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
    #[test]
    fn a_rejected_sell_send_can_be_tried_again() {
        let rejected = sell_send_result(Err(SparkClientError::BridgeError {
            kind: coincube_spark_protocol::ErrorKind::Sdk,
            message: "insufficient funds".to_string(),
        }));
        assert!(matches!(rejected, MavapayMessage::SellSendFailed(_)));

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
            send_sell_payment(&spark, "prepare-1".to_string(), key.clone()),
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
        assert!(state
            .update(MavapayMessage::SellPaymentChecked(checked), &client)
            .is_some());
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
            MavapayMessage::SellPaymentChecked(ReconcileOutcome::Inconclusive("x".into())),
            &client,
        );
        assert!(!sell(&state).fulfilment.may_resend());
        assert!(state
            .update(MavapayMessage::RetrySellPayment, &client)
            .is_none());

        // No trace on a key-guarded rail: a repeat, under the same key.
        let mut state = sell_checkout(spark.clone());
        unknown(&mut state);
        let _ = state.update(
            MavapayMessage::SellPaymentChecked(ReconcileOutcome::NoTrace),
            &client,
        );
        assert!(state
            .update(MavapayMessage::RetrySellPayment, &client)
            .is_some());
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Preparing);
        assert!(state
            .update(
                MavapayMessage::SellInvoicePrepared(prepared(Some(false))),
                &client
            )
            .is_some());
        assert_eq!(sell(&state).idempotency_key.as_deref(), Some("key-1"));
        assert_eq!(sell(&state).fulfilment, SellFulfilment::Sending);

        // …unless the new preview would have the bridge drop the key.
        let mut state = sell_checkout(spark);
        unknown(&mut state);
        let _ = state.update(
            MavapayMessage::SellPaymentChecked(ReconcileOutcome::NoTrace),
            &client,
        );
        let _ = state.update(MavapayMessage::RetrySellPayment, &client);
        assert!(state
            .update(
                MavapayMessage::SellInvoicePrepared(prepared(Some(true))),
                &client
            )
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
}
