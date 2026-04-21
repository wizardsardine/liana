//! Spark Transactions panel.
//!
//! Mirrors [`crate::app::state::liquid::transactions::LiquidTransactions`]
//! minus the asset-filter tabs (Spark only holds BTC) and the
//! refundable-swap section (Spark has no boltz-style swap refunds).
//!
//! On reload the panel asks the bridge for up to 100 recent payments
//! via `list_payments`, maps each [`PaymentSummary`] into the shared
//! [`SparkRecentTransaction`] row shape that the overview already
//! uses, and hands the list to the view renderer.

use std::convert::TryInto;
use std::sync::Arc;

use coincube_spark_protocol::PaymentSummary;
use coincube_ui::{
    component::quote_display::{self, Quote},
    widget::Element,
};
use iced::widget::image;
use iced::Task;

use crate::app::cache::Cache;
use crate::app::menu::{Menu, SparkSubMenu};
use crate::app::message::Message;
use crate::app::state::{redirect, State};
use crate::app::view::spark::{
    SparkRecentTransaction, SparkTransactionsStatus, SparkTransactionsView,
};
use crate::app::view::{self, FiatAmountConverter};
use crate::app::wallets::SparkBackend;
use crate::export::{ImportExportMessage, ImportExportState};

#[derive(Debug)]
enum SparkTransactionsModal {
    None,
    Export { state: ImportExportState },
}

pub struct SparkTransactions {
    backend: Option<Arc<SparkBackend>>,
    payments: Vec<PaymentSummary>,
    recent_transactions: Vec<SparkRecentTransaction>,
    loading: bool,
    error: Option<String>,
    modal: SparkTransactionsModal,
    /// When `Some`, render the detail pane for this payment instead
    /// of the list. Cleared via `Message::Close` (the back button).
    selected_payment: Option<SparkRecentTransaction>,
    /// Empty-state Kage quote + image. Picked once when the panel is
    /// constructed so repeated reloads don't re-randomize the quote.
    empty_state_quote: Quote,
    empty_state_image_handle: image::Handle,
}

impl SparkTransactions {
    pub fn new(backend: Option<Arc<SparkBackend>>) -> Self {
        let empty_state_quote = quote_display::random_quote("empty-wallet");
        let empty_state_image_handle = quote_display::image_handle_for_context("empty-wallet");
        Self {
            backend,
            payments: Vec::new(),
            recent_transactions: Vec::new(),
            loading: false,
            error: None,
            modal: SparkTransactionsModal::None,
            selected_payment: None,
            empty_state_quote,
            empty_state_image_handle,
        }
    }

    fn rebuild_rows(&mut self, cache: &Cache) {
        let fiat_converter: Option<FiatAmountConverter> =
            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
        self.recent_transactions = self
            .payments
            .iter()
            .map(|p| {
                crate::app::state::spark::overview::payment_summary_to_recent_tx(
                    p,
                    fiat_converter.as_ref(),
                )
            })
            .collect();
    }
}

impl State for SparkTransactions {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> Element<'a, crate::app::view::Message> {
        let fiat_converter: Option<FiatAmountConverter> =
            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

        // When a payment has been selected (via tapping a row here, or
        // preselected from Overview/Send/Receive), take over the panel
        // body with the detail view; the back button clears the state
        // via `Message::Close` and we fall through to the list again.
        if let Some(payment) = &self.selected_payment {
            return crate::app::view::dashboard(
                menu,
                cache,
                crate::app::view::spark::transactions::transaction_detail_view(
                    payment,
                    cache.bitcoin_unit,
                ),
            );
        }

        let status = if self.backend.is_none() {
            SparkTransactionsStatus::Unavailable
        } else if self.loading && self.payments.is_empty() {
            SparkTransactionsStatus::Loading
        } else if let Some(err) = &self.error {
            SparkTransactionsStatus::Error(err.clone())
        } else {
            SparkTransactionsStatus::Loaded(self.payments.clone())
        };

        let content = crate::app::view::dashboard(
            menu,
            cache,
            SparkTransactionsView {
                status,
                recent_transactions: &self.recent_transactions,
                fiat_converter,
                bitcoin_unit: cache.bitcoin_unit,
                show_direction_badges: cache.show_direction_badges,
                empty_state_quote: &self.empty_state_quote,
                empty_state_image_handle: &self.empty_state_image_handle,
            }
            .render(),
        );

        match &self.modal {
            SparkTransactionsModal::None => content,
            SparkTransactionsModal::Export { state } => {
                use coincube_ui::component::text::*;
                use coincube_ui::widget::modal::Modal;
                use iced::widget::Column;

                let modal_content = match state {
                    ImportExportState::Ended => Column::new()
                        .spacing(20)
                        .push(text("Export successful!").size(20).bold())
                        .push(
                            coincube_ui::component::button::primary(None, "Close")
                                .width(150)
                                .on_press(view::Message::ImportExport(ImportExportMessage::Close)),
                        ),
                    _ => Column::new()
                        .spacing(20)
                        .push(text("Exporting payments…").size(20).bold()),
                };

                Modal::new(content, modal_content)
                    .on_blur(Some(view::Message::ImportExport(
                        ImportExportMessage::Close,
                    )))
                    .into()
            }
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> Task<Message> {
        let Some(backend) = self.backend.clone() else {
            return Task::none();
        };
        self.loading = true;
        self.error = None;
        Task::perform(
            async move { backend.list_payments(Some(100)).await },
            |result| match result {
                Ok(list) => Message::View(crate::app::view::Message::SparkTransactions(
                    crate::app::view::SparkTransactionsMessage::DataLoaded(list.payments),
                )),
                Err(e) => Message::View(crate::app::view::Message::SparkTransactions(
                    crate::app::view::SparkTransactionsMessage::Error(e.to_string()),
                )),
            },
        )
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::SparkTransactions(msg)) => {
                match msg {
                    view::SparkTransactionsMessage::DataLoaded(payments) => {
                        self.loading = false;
                        self.payments = payments;
                        self.error = None;
                        self.rebuild_rows(cache);
                    }
                    view::SparkTransactionsMessage::Error(err) => {
                        self.loading = false;
                        self.error = Some(err);
                    }
                    view::SparkTransactionsMessage::Select(idx) => {
                        self.selected_payment = self.recent_transactions.get(idx).cloned();
                    }
                    view::SparkTransactionsMessage::Preselect(payment) => {
                        self.selected_payment = Some(payment);
                    }
                    view::SparkTransactionsMessage::SendBtc => {
                        return redirect(Menu::Spark(SparkSubMenu::Send));
                    }
                    view::SparkTransactionsMessage::ReceiveBtc => {
                        return redirect(Menu::Spark(SparkSubMenu::Receive));
                    }
                }
            }
            // Detail pane's back button emits `Message::Close`. Clear
            // the selection so the next render falls back to the list.
            Message::View(view::Message::Close) => {
                self.selected_payment = None;
            }
            // Export flow. Mirrors the Liquid transactions handler:
            // Open → prompt for path → run export → show modal →
            // user closes.
            Message::View(view::Message::ImportExport(ImportExportMessage::Open)) => {
                if matches!(self.modal, SparkTransactionsModal::None) {
                    return Task::perform(
                        crate::export::get_path(
                            format!(
                                "coincube-spark-txs-{}.csv",
                                chrono::Local::now().format("%Y-%m-%dT%H-%M-%S")
                            ),
                            true,
                        ),
                        |path| {
                            Message::View(view::Message::ImportExport(ImportExportMessage::Path(
                                path,
                            )))
                        },
                    );
                }
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Path(Some(path)))) => {
                // Only run the export if the user actually opened it
                // from the Spark panel. If the Liquid panel opened a
                // modal concurrently it'll have its own handler — but
                // panels only receive messages while active so this is
                // safe in practice.
                let Some(backend) = self.backend.clone() else {
                    return Task::none();
                };
                self.modal = SparkTransactionsModal::Export {
                    state: ImportExportState::Started,
                };
                return Task::perform(
                    async move {
                        crate::export::export_spark_payments(
                            &tokio::sync::mpsc::unbounded_channel().0,
                            backend,
                            path,
                        )
                        .await
                    },
                    |result| {
                        Message::View(view::Message::ImportExport(ImportExportMessage::Progress(
                            match result {
                                Ok(_) => crate::export::Progress::Ended,
                                Err(e) => crate::export::Progress::Error(e),
                            },
                        )))
                    },
                );
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Path(None))) => {
                self.modal = SparkTransactionsModal::None;
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Progress(
                crate::export::Progress::Ended,
            ))) => {
                if matches!(self.modal, SparkTransactionsModal::Export { .. }) {
                    self.modal = SparkTransactionsModal::Export {
                        state: ImportExportState::Ended,
                    };
                }
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Progress(
                crate::export::Progress::Error(e),
            ))) => {
                if matches!(self.modal, SparkTransactionsModal::Export { .. }) {
                    self.modal = SparkTransactionsModal::None;
                    return Task::done(Message::View(view::Message::ShowError(format!(
                        "Export failed: {:?}",
                        e
                    ))));
                }
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Close)) => {
                self.modal = SparkTransactionsModal::None;
            }
            _ => {}
        }
        Task::none()
    }
}
