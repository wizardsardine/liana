//! Activity result, navigation and wallet-panel hand-off routing.
use super::*;

impl App {
    pub(super) fn route_activity_result(&mut self, msg: Message) -> Task<Message> {
        let task = self
            .panels
            .activity
            .update(self.daemon.clone(), &self.cache, msg);
        self.settle_wallet_selection();
        task
    }

    pub(super) fn route_activity_view_all(
        &mut self,
        wallet: view::activity::WalletFilter,
    ) -> Task<Message> {
        let filter = self.panels.activity.update(
            self.daemon.clone(),
            &self.cache,
            Message::View(view::Message::Activity(
                view::activity::ActivityMessage::ViewAll(wallet),
            )),
        );
        Task::batch([
            filter,
            self.set_current_panel(Menu::Cube(menu::CubeSubMenu::Activity)),
        ])
    }

    pub(super) fn route_activity_open(&mut self, id: String) -> Task<Message> {
        // An Overview row: Activity opens unfiltered on it. Selected
        // after the switch, so leaving a page can't clear it. The row
        // as the Overview listed it is read first: the switch's reload
        // clears Activity's rows, and its pane shows that copy until
        // (or unless) the read brings a fresh one.
        let opened = self
            .panels
            .activity
            .recent_entries(view::activity::RecentScope::Cube)
            .into_iter()
            .find(|entry| entry.id == id);
        let load = self.set_current_panel(Menu::Cube(menu::CubeSubMenu::Activity));
        let select = self.panels.activity.update(
            self.daemon.clone(),
            &self.cache,
            Message::View(view::Message::Activity(
                view::activity::ActivityMessage::Open(id.clone()),
            )),
        );
        if self.panels.activity.selected_id.as_deref() == Some(id.as_str()) {
            self.panels.activity.opened = opened;
        }
        Task::batch([load, select])
    }

    pub(super) fn route_activity_view(&mut self, msg: Message) -> Task<Message> {
        self.panels
            .activity
            .update(self.daemon.clone(), &self.cache, msg)
    }

    pub(super) fn route_activity_spark_overview(&mut self, msg: Message) -> Task<Message> {
        // Every Spark page reload (a payment event, a `Synced` tick,
        // opening the page) re-reads payments for its balance; its
        // "Recent activity" follows from Activity's copy.
        let refresh =
            self.panels
                .activity
                .refresh_wallet(view::activity::WalletFilter::Spark, None, None);
        let forwarded = self.panels.spark_overview.update(None, &self.cache, msg);
        Task::batch([forwarded, refresh])
    }

    pub(super) fn route_activity_vault_overview(&mut self, msg: Message) -> Task<Message> {
        // The Vault page re-read its payments; its "Recent activity"
        // follows from Activity's copy of the history.
        let refresh = self.panels.activity.refresh_wallet(
            view::activity::WalletFilter::Vault,
            self.daemon.clone(),
            self.wallet.clone(),
        );
        let daemon = self.daemon.clone();
        let forwarded = match self.panels.current_mut() {
            Some(panel) => panel.update(daemon, &self.cache, msg),
            None => Task::none(),
        };
        Task::batch([forwarded, refresh])
    }

    /// The transaction detail page showing on the current route whose Back
    /// goes to Cube › Activity, the one history list: a Vault or Spark
    /// transaction always, a PSBT when Activity opened it. `None` leaves
    /// Close to the page. While an imported Vault's own panel holds a Vault
    /// route, the Vault panels behind it are not showing, whatever they hold.
    fn activity_detail_showing(&self) -> Option<ActivityDetail> {
        if self
            .panels
            .imported_bitcoin
            .as_ref()
            .is_some_and(|imported| imported.handles_route(&self.panels.current))
        {
            return None;
        }
        match &self.panels.current {
            Menu::Vault(menu::VaultSubMenu::Transactions(_)) => self
                .panels
                .transactions
                .as_ref()
                .is_some_and(|panel| panel.has_selection())
                .then_some(ActivityDetail::VaultTransaction),
            Menu::Vault(menu::VaultSubMenu::PSBTs(_)) => self
                .panels
                .psbts
                .as_ref()
                .is_some_and(|panel| panel.returns_to_activity())
                .then_some(ActivityDetail::Psbt),
            Menu::Spark(menu::SparkSubMenu::Transactions(_)) => self
                .panels
                .spark_transactions
                .has_selection()
                .then_some(ActivityDetail::SparkPayment),
            _ => None,
        }
    }

    /// Back from a transaction detail page goes to Cube › Activity; see
    /// [`Self::activity_detail_showing`].
    pub(super) fn activity_detail_back_applies(&self) -> bool {
        self.activity_detail_showing().is_some()
    }

    /// Close the detail page that is showing and open Cube › Activity. Only
    /// that page's selection is cleared: the other wallet panels stay alive
    /// across routes and keep what they hold (a PSBT open from its own list,
    /// with any signing in progress, survives Back from a Spark payment).
    pub(super) fn route_activity_detail_back(&mut self) -> Task<Message> {
        match self.activity_detail_showing() {
            Some(ActivityDetail::VaultTransaction) => {
                if let Some(panel) = self.panels.transactions.as_mut() {
                    panel.clear_selection();
                }
            }
            Some(ActivityDetail::Psbt) => {
                if let Some(panel) = self.panels.psbts.as_mut() {
                    panel.clear_selection();
                }
            }
            Some(ActivityDetail::SparkPayment) => self.panels.spark_transactions.clear_selection(),
            None => return Task::none(),
        }
        self.set_current_panel(Menu::Cube(menu::CubeSubMenu::Activity))
    }

    pub(super) fn route_activity_detail(
        &mut self,
        target: state::activity::ActivityTarget,
    ) -> Task<Message> {
        // The hand-off leaves the wallet page: its selection ends.
        if wallet_page(&self.panels.current).is_some() {
            self.panels.activity.clear_selection();
        }
        // Hand off the source record to its owning wallet panel. Those
        // panels retain signing, refund and replacement safeguards.
        match target {
            state::activity::ActivityTarget::Vault(tx) => {
                if self.panels.transactions.is_some() {
                    let load =
                        self.set_current_panel(Menu::Vault(menu::VaultSubMenu::Transactions(None)));
                    let panel = self.panels.transactions.as_mut().expect("checked above");
                    panel.preselect(tx);
                    return load;
                }
            }
            state::activity::ActivityTarget::Psbt(tx) => {
                if self.panels.psbts.is_some() {
                    let load = self.set_current_panel(Menu::Vault(menu::VaultSubMenu::PSBTs(None)));
                    let panel = self.panels.psbts.as_mut().expect("checked above");
                    panel.preselect_from_activity(tx);
                    return load;
                }
            }
            state::activity::ActivityTarget::Spark(payment) => {
                let row = state::spark::overview::payment_summary_to_recent_tx(&payment, None);
                self.panels.current = Menu::Spark(menu::SparkSubMenu::Transactions(None));
                let load = self
                    .panels
                    .spark_transactions
                    .reload(self.daemon.clone(), self.wallet.clone());
                let select = self.panels.spark_transactions.update(
                    self.daemon.clone(),
                    &self.cache,
                    Message::View(view::Message::SparkTransactions(
                        view::SparkTransactionsMessage::Preselect(row),
                    )),
                );
                return Task::batch([load, select]);
            }
        }
        Task::none()
    }
}

/// A transaction detail page whose Back goes to Cube › Activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivityDetail {
    VaultTransaction,
    Psbt,
    SparkPayment,
}
