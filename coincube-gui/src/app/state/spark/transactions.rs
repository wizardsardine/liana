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

/// Bump to a larger value (e.g. 50) once Prev/Next pagination is verified
/// end-to-end on real wallets. Kept low during rollout so QA can exercise
/// pagination without needing 50+ transactions in a single wallet.
pub const PAGE_SIZE: u32 = 10;

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
    /// Page currently displayed (0-indexed).
    current_page: u32,
    /// Target page of an in-flight Prev/Next fetch. Committed to
    /// `current_page` only on `DataLoaded`; dropped on `Error` so a failed
    /// fetch doesn't desync the page counter from the shown data.
    pending_page: Option<u32>,
    /// Monotonic fetch-generation counter. Every `fetch_page` (reload or
    /// Prev/Next) bumps it and tags its result; the handler discards any
    /// response whose token isn't the latest, so a stale response can't
    /// clobber `payments` with wrong-page data.
    fetch_token: u64,
    is_last_page: bool,
    processing: bool,
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
            current_page: 0,
            pending_page: None,
            fetch_token: 0,
            is_last_page: false,
            processing: false,
        }
    }

    /// Fetch `page` (0-indexed). `current_page` is *not* moved here — it is
    /// only committed once `DataLoaded` lands, so a failed fetch leaves the
    /// panel showing the page it was already on.
    ///
    /// Bumps `fetch_token` and tags the result with it; the handler drops
    /// any response that isn't the latest, so a stale page response can't
    /// overwrite data from a newer reload.
    ///
    /// Requests `PAGE_SIZE + 1` rows: the extra row is a probe for whether
    /// a further page exists. The handler trims it off before display and
    /// sets `is_last_page` from whether it was returned — without it, a
    /// total that is an exact multiple of `PAGE_SIZE` would leave Next
    /// enabled onto an empty page.
    fn fetch_page(&mut self, page: u32) -> Task<Message> {
        let Some(backend) = self.backend.clone() else {
            return Task::none();
        };
        self.fetch_token = self.fetch_token.wrapping_add(1);
        let token = self.fetch_token;
        let offset = page.saturating_mul(PAGE_SIZE);
        Task::perform(
            async move {
                backend
                    .list_payments(Some(PAGE_SIZE + 1), Some(offset))
                    .await
            },
            move |result| match result {
                Ok(list) => Message::View(crate::app::view::Message::SparkTransactions(
                    crate::app::view::SparkTransactionsMessage::DataLoaded(token, list.payments),
                )),
                Err(e) => Message::View(crate::app::view::Message::SparkTransactions(
                    crate::app::view::SparkTransactionsMessage::Error(token, e.to_string()),
                )),
            },
        )
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
                    fiat_converter,
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
                current_page: self.current_page,
                is_last_page: self.is_last_page,
                processing: self.processing,
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
        if self.backend.is_none() {
            return Task::none();
        }
        self.loading = true;
        self.error = None;
        self.current_page = 0;
        self.pending_page = None;
        self.is_last_page = false;
        self.processing = false;
        self.fetch_page(0)
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::SparkTransactions(msg)) => match msg {
                view::SparkTransactionsMessage::DataLoaded(token, mut payments) => {
                    // Discard a stale response: a newer fetch (reload or
                    // another Prev/Next) has since been dispatched, so
                    // applying this page would show wrong-page data.
                    if token != self.fetch_token {
                        return Task::none();
                    }
                    self.loading = false;
                    self.processing = false;
                    // Commit the page navigation now that the fetch
                    // succeeded. `pending_page` is `None` for a reload, where
                    // `current_page` was already set to 0 by `reload`.
                    if let Some(page) = self.pending_page.take() {
                        self.current_page = page;
                    }
                    // `fetch_page` over-fetches one probe row: receiving more
                    // than `PAGE_SIZE` means a further page exists. Trim the
                    // probe row off before display. Because Next is only
                    // offered when the probe row was returned, a committed
                    // non-zero page always has at least one real row.
                    self.is_last_page = (payments.len() as u32) <= PAGE_SIZE;
                    payments.truncate(PAGE_SIZE as usize);
                    self.payments = payments;
                    self.error = None;
                    self.rebuild_rows(cache);
                }
                view::SparkTransactionsMessage::Error(token, err) => {
                    // Stale error from a superseded fetch — ignore it so it
                    // can't clear `pending_page` for the newer in-flight
                    // fetch or surface a spurious error.
                    if token != self.fetch_token {
                        return Task::none();
                    }
                    self.loading = false;
                    self.processing = false;
                    // Discard the in-flight navigation: `current_page` stays
                    // on the page whose data is still displayed.
                    self.pending_page = None;
                    self.error = Some(err);
                }
                view::SparkTransactionsMessage::PrevPage => {
                    if self.current_page > 0 && !self.processing {
                        let target = self.current_page - 1;
                        self.pending_page = Some(target);
                        self.processing = true;
                        return self.fetch_page(target);
                    }
                }
                view::SparkTransactionsMessage::NextPage => {
                    if !self.is_last_page && !self.processing {
                        let target = self.current_page + 1;
                        self.pending_page = Some(target);
                        self.processing = true;
                        return self.fetch_page(target);
                    }
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
            },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::State;
    use crate::app::view::spark::SparkPaymentMethod;
    use crate::app::view::{Message as ViewMessage, SparkTransactionsMessage};
    use crate::app::wallets::DomainPaymentStatus;

    fn payment(id: &str, amount_sat: i64) -> PaymentSummary {
        PaymentSummary {
            id: id.to_string(),
            amount_sat,
            fees_sat: 7,
            token_amount: None,
            token_decimals: None,
            token_ticker: None,
            timestamp: 1_700_000_000,
            status: "Completed".to_string(),
            direction: if amount_sat >= 0 {
                "Receive".to_string()
            } else {
                "Send".to_string()
            },
            method: "lightning".to_string(),
            description: Some(format!("payment {id}")),
        }
    }

    fn payments(count: usize) -> Vec<PaymentSummary> {
        (0..count)
            .map(|i| payment(&format!("payment-{i}"), (i as i64 + 1) * 1_000))
            .collect()
    }

    fn update(panel: &mut SparkTransactions, msg: SparkTransactionsMessage) {
        let _ = State::update(
            panel,
            None,
            &Cache::default(),
            Message::View(ViewMessage::SparkTransactions(msg)),
        );
    }

    #[test]
    fn new_panel_starts_empty() {
        let panel = SparkTransactions::new(None);

        assert!(panel.backend.is_none());
        assert!(panel.payments.is_empty());
        assert!(panel.recent_transactions.is_empty());
        assert!(!panel.loading);
        assert_eq!(panel.current_page, 0);
        assert_eq!(panel.pending_page, None);
        assert_eq!(panel.fetch_token, 0);
        assert!(!panel.is_last_page);
        assert!(!panel.processing);
        assert!(matches!(panel.modal, SparkTransactionsModal::None));
        assert!(panel.selected_payment.is_none());
    }

    #[test]
    fn reload_without_backend_is_noop() {
        let mut panel = SparkTransactions::new(None);
        panel.loading = true;
        panel.error = Some("keep me".to_string());

        let _ = State::reload(&mut panel, None, None);

        assert!(panel.loading);
        assert_eq!(panel.error.as_deref(), Some("keep me"));
        assert_eq!(panel.fetch_token, 0);
    }

    #[test]
    fn stale_data_response_is_ignored() {
        let mut panel = SparkTransactions::new(None);
        panel.fetch_token = 2;
        panel.loading = true;
        panel.processing = true;
        panel.payments = vec![payment("current", 1_000)];

        update(
            &mut panel,
            SparkTransactionsMessage::DataLoaded(1, payments(3)),
        );

        assert!(panel.loading);
        assert!(panel.processing);
        assert_eq!(panel.payments.len(), 1);
        assert_eq!(panel.payments[0].id, "current");
        assert!(panel.recent_transactions.is_empty());
    }

    #[test]
    fn data_loaded_commits_pending_page_and_trims_probe_row() {
        let mut panel = SparkTransactions::new(None);
        panel.fetch_token = 9;
        panel.pending_page = Some(2);
        panel.loading = true;
        panel.processing = true;
        panel.error = Some("old error".to_string());

        update(
            &mut panel,
            SparkTransactionsMessage::DataLoaded(9, payments(PAGE_SIZE as usize + 1)),
        );

        assert!(!panel.loading);
        assert!(!panel.processing);
        assert_eq!(panel.current_page, 2);
        assert_eq!(panel.pending_page, None);
        assert!(!panel.is_last_page);
        assert_eq!(panel.payments.len(), PAGE_SIZE as usize);
        assert_eq!(panel.recent_transactions.len(), PAGE_SIZE as usize);
        assert_eq!(panel.error, None);

        let first = &panel.recent_transactions[0];
        assert_eq!(first.id, "payment-0");
        assert_eq!(first.description, "payment payment-0");
        assert_eq!(first.amount.to_sat(), 1_000);
        assert_eq!(first.fees_sat.to_sat(), 7);
        assert_eq!(first.status, DomainPaymentStatus::Complete);
        assert_eq!(first.method, SparkPaymentMethod::Lightning);
    }

    #[test]
    fn exact_page_marks_last_page() {
        let mut panel = SparkTransactions::new(None);
        panel.fetch_token = 3;
        panel.loading = true;

        update(
            &mut panel,
            SparkTransactionsMessage::DataLoaded(3, payments(PAGE_SIZE as usize)),
        );

        assert!(panel.is_last_page);
        assert_eq!(panel.payments.len(), PAGE_SIZE as usize);
    }

    #[test]
    fn current_error_response_rolls_back_pending_navigation() {
        let mut panel = SparkTransactions::new(None);
        panel.fetch_token = 5;
        panel.current_page = 1;
        panel.pending_page = Some(2);
        panel.loading = true;
        panel.processing = true;

        update(
            &mut panel,
            SparkTransactionsMessage::Error(5, "bridge timed out".to_string()),
        );

        assert!(!panel.loading);
        assert!(!panel.processing);
        assert_eq!(panel.current_page, 1);
        assert_eq!(panel.pending_page, None);
        assert_eq!(panel.error.as_deref(), Some("bridge timed out"));
    }

    #[test]
    fn stale_error_response_is_ignored() {
        let mut panel = SparkTransactions::new(None);
        panel.fetch_token = 8;
        panel.current_page = 1;
        panel.pending_page = Some(2);
        panel.loading = true;
        panel.processing = true;

        update(
            &mut panel,
            SparkTransactionsMessage::Error(7, "old timeout".to_string()),
        );

        assert!(panel.loading);
        assert!(panel.processing);
        assert_eq!(panel.pending_page, Some(2));
        assert_eq!(panel.error, None);
    }

    #[test]
    fn select_preselect_and_close_manage_detail_selection() {
        let mut panel = SparkTransactions::new(None);
        panel.fetch_token = 1;
        update(
            &mut panel,
            SparkTransactionsMessage::DataLoaded(1, payments(2)),
        );

        update(&mut panel, SparkTransactionsMessage::Select(1));
        assert_eq!(
            panel.selected_payment.as_ref().map(|p| p.id.as_str()),
            Some("payment-1")
        );

        let explicit = panel.recent_transactions[0].clone();
        update(
            &mut panel,
            SparkTransactionsMessage::Preselect(explicit.clone()),
        );
        assert_eq!(
            panel.selected_payment.as_ref().map(|p| p.id.as_str()),
            Some(explicit.id.as_str())
        );

        let _ = State::update(
            &mut panel,
            None,
            &Cache::default(),
            Message::View(ViewMessage::Close),
        );
        assert!(panel.selected_payment.is_none());
    }

    #[test]
    fn export_modal_progress_and_close_are_local_state_only() {
        let mut panel = SparkTransactions::new(None);
        panel.modal = SparkTransactionsModal::Export {
            state: ImportExportState::Started,
        };

        let _ = State::update(
            &mut panel,
            None,
            &Cache::default(),
            Message::View(ViewMessage::ImportExport(ImportExportMessage::Progress(
                crate::export::Progress::Ended,
            ))),
        );
        assert!(matches!(
            panel.modal,
            SparkTransactionsModal::Export {
                state: ImportExportState::Ended
            }
        ));

        let _ = State::update(
            &mut panel,
            None,
            &Cache::default(),
            Message::View(ViewMessage::ImportExport(ImportExportMessage::Close)),
        );
        assert!(matches!(panel.modal, SparkTransactionsModal::None));

        panel.modal = SparkTransactionsModal::Export {
            state: ImportExportState::Started,
        };
        let _ = State::update(
            &mut panel,
            None,
            &Cache::default(),
            Message::View(ViewMessage::ImportExport(ImportExportMessage::Path(None))),
        );
        assert!(matches!(panel.modal, SparkTransactionsModal::None));
    }
}
