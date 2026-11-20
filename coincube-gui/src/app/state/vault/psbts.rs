use std::sync::Arc;

use iced::{Subscription, Task};

use coincube_ui::widget::Element;

use super::{super::State, export::VaultExportModal, psbt};
use crate::{
    app::{cache::Cache, error::Error, menu::Menu, message::Message, view, wallet::Wallet},
    daemon::{model::SpendTx, Daemon},
    export::{ImportExportMessage, ImportExportType},
};

pub struct PsbtsPanel {
    wallet: Arc<Wallet>,
    selected_tx: Option<psbt::PsbtState>,
    spend_txs: Vec<SpendTx>,
    warning: Option<Error>,
    modal: Option<VaultExportModal>,
    /// The selected PSBT was opened from Cube › Activity, so its Back
    /// returns there rather than to this panel's list.
    opened_from_activity: bool,
}

impl PsbtsPanel {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self {
            wallet,
            spend_txs: Vec::new(),
            warning: None,
            selected_tx: None,
            modal: None,
            opened_from_activity: false,
        }
    }

    pub fn preselect(&mut self, spend_tx: SpendTx) {
        let psbt_state = psbt::PsbtState::new(self.wallet.clone(), spend_tx, true);
        self.selected_tx = Some(psbt_state);
        self.warning = None;
        self.modal = None;
        self.opened_from_activity = false;
    }

    /// Open `spend_tx` as Cube › Activity's detail page: Back returns there.
    pub fn preselect_from_activity(&mut self, spend_tx: SpendTx) {
        self.preselect(spend_tx);
        self.opened_from_activity = true;
        if let Some(selected) = self.selected_tx.as_mut() {
            selected.back_to_activity = true;
        }
    }

    /// The detail page is showing and Activity opened it.
    pub fn returns_to_activity(&self) -> bool {
        self.opened_from_activity && self.selected_tx.is_some()
    }

    /// A PSBT's detail page is open, from either route.
    pub fn has_selection(&self) -> bool {
        self.selected_tx.is_some()
    }

    /// Leave the detail page without re-reading the PSBT list.
    pub fn clear_selection(&mut self) {
        self.selected_tx = None;
        self.opened_from_activity = false;
    }

    fn apply_spend_tx_refresh(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        res: Result<Vec<SpendTx>, Error>,
    ) -> Task<Message> {
        match res {
            Err(e) => {
                let err_msg = crate::user_error::report(&e);
                self.warning = Some(e);
                return Task::done(Message::View(view::Message::ShowError(err_msg)));
            }
            Ok(mut txs) => {
                self.warning = None;
                // Optimistic Pending->Broadcast override for txs
                // the user just broadcast locally, until the
                // daemon's mempool poller reflects the spend in
                // its derived `SpendStatus`.
                self.wallet.apply_spend_tx_overrides(&mut txs);
                self.spend_txs = txs;
                if let Some(tx) = &self.selected_tx {
                    // Never rebuild the open PSBT panel while a modal is up:
                    // a `PsbtState::new` here resets `modal` to `None` and
                    // replaces `tx.psbt` with the daemon's copy, dropping an
                    // in-progress Sign session (and its in-flight Keychain
                    // sessions) and re-exposing only the signatures persisted
                    // so far. The signing flow keeps the merged psbt in
                    // memory and persists it itself, so a refresh has nothing
                    // to add here.
                    if tx.modal.is_some() {
                        return Task::none();
                    }
                    if let Some(tx) = self.spend_txs.iter().find(|spend_tx| {
                        spend_tx.psbt.unsigned_tx.compute_txid()
                            == tx.tx.psbt.unsigned_tx.compute_txid()
                    }) {
                        let mut tx = psbt::PsbtState::new(self.wallet.clone(), tx.clone(), true);
                        // The rebuilt detail keeps naming where its Back goes.
                        tx.back_to_activity = self.opened_from_activity;
                        let cmd = tx.load(daemon);
                        self.selected_tx = Some(tx);
                        return cmd;
                    }
                }
            }
        }
        Task::none()
    }

    fn start_psbt_import(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Some(tx) = &mut self.selected_tx {
            return tx.update(daemon, cache, message);
        } else if self.modal.is_none() {
            let modal =
                VaultExportModal::new(Some(daemon.clone()), ImportExportType::ImportPsbt(None));
            let launch = modal.launch(false);
            self.modal = Some(modal);
            return launch;
        }
        Task::none()
    }
}

impl State for PsbtsPanel {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(tx) = &self.selected_tx {
            tx.view(cache)
        } else {
            let list_view = view::dashboard(
                menu,
                cache,
                view::vault::psbts::psbts_view(
                    &self.spend_txs,
                    cache.bitcoin_unit,
                    cache.balance_amounts_masked(),
                ),
            );
            if let Some(modal) = &self.modal {
                modal.view(list_view)
            } else {
                list_view
            }
        }
    }

    fn interrupt(&mut self) {
        self.selected_tx = None;
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let daemon = daemon.expect("Daemon required for vault psbts panel");
        match message {
            Message::View(view::Message::Reload) | Message::View(view::Message::Close) => {
                return self.reload(Some(daemon), Some(self.wallet.clone()));
            }
            Message::SpendTxs(res) => return self.apply_spend_tx_refresh(daemon, res),
            Message::View(view::Message::ImportPsbt) => {
                return self.start_psbt_import(daemon, cache, message);
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Close)) => {
                if let Some(tx) = &mut self.selected_tx {
                    return tx.update(daemon, cache, message);
                } else if self.modal.is_some() {
                    self.modal = None;
                    return Task::perform(async {}, |_| Message::View(view::Message::Reload));
                }
            }
            Message::View(view::Message::ImportExport(m)) => {
                let m = m.clone();
                if let Some(tx) = &mut self.selected_tx {
                    let message = Message::View(view::Message::ImportExport(m));
                    return tx.update(daemon, cache, message);
                } else if let Some(modal) = self.modal.as_mut() {
                    return modal.update(m.clone());
                }
            }
            Message::View(view::Message::Select(i)) => {
                if let Some(tx) = self.spend_txs.get(i) {
                    let tx = psbt::PsbtState::new(self.wallet.clone(), tx.clone(), true);
                    let cmd = tx.load(daemon);
                    self.selected_tx = Some(tx);
                    self.opened_from_activity = false;
                    return cmd;
                }
            }
            _ => {
                if let Some(tx) = &mut self.selected_tx {
                    return tx.update(daemon, cache, message);
                }
            }
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        if let Some(psbt) = &self.selected_tx {
            psbt.subscription()
        } else if let Some(modal) = &self.modal {
            modal
                .subscription()
                .map(|s| {
                    s.map(|m| {
                        Message::View(view::Message::ImportExport(ImportExportMessage::Progress(
                            m,
                        )))
                    })
                })
                .unwrap_or(Subscription::none())
        } else {
            Subscription::none()
        }
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        let daemon = daemon.expect("Vault panels require daemon");
        let wallet = wallet.expect("Vault panels require wallet");
        self.wallet = wallet;
        self.selected_tx = None;
        self.opened_from_activity = false;
        self.modal = None;
        let daemon = daemon.clone();
        Task::perform(
            async move {
                daemon
                    .list_spend_transactions(None)
                    .await
                    .map_err(|e| e.into())
            },
            Message::SpendTxs,
        )
    }
}

impl From<PsbtsPanel> for Box<dyn State> {
    fn from(s: PsbtsPanel) -> Box<dyn State> {
        Box::new(s)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use crate::{
        daemon::{client::Coincubed, model::TransactionKind},
        utils::mock::Daemon as MockDaemon,
    };
    use coincube_core::{
        descriptors::CoincubeDescriptor,
        miniscript::bitcoin::{secp256k1::Secp256k1, Amount, Network, Psbt},
    };
    use coincube_ui::component::amount::{BitcoinDisplayUnit, DisplayAmount};
    use coincube_ui::theme;
    use std::str::FromStr;

    const DESC: &str = "wsh(or_d(multi(2,[f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<0;1>/*,[2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<0;1>/*),and_v(v:thresh(1,pkh([f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<2;3>/*),a:pkh([2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<2;3>/*)),older(65535))))#9s8ekrce";

    // Unsigned PSBT spending a Coincube coin under `DESC` (mirrors the fixture
    // used by the psbt panel tests).
    const UNSIGNED_PSBT: &str = "cHNidP8BAIkCAAAAAc0x/jtWvFugrl8zc34KVIlWCugXT6JNtgir6UqX+Vv6AQAAAAD9////AkBCDwAAAAAAIgAgtQu/fA/8rQhJ0I6wUoBDO0vNa3lgsEpEIj7rTOMnBcXuIEkBAAAAACIAIOdCiXh7yL2V/f6S6KMTOzgqKkqyIXgmFuwDnmXbIiosAAAAAAABAP04AQIAAAAAAQKYYriMs/PtSqm6LPNWWFYskTL6nWZegJdwxYcVCRn8vwEAAAAA/f///87D7dkdgMd1Laj/v6xspNRtrQXGP+8BPFMLqkeBb6MRAQAAAAD9////AuGQDgAAAAAAIlEg7DgdNxI7WybaPUZXcMCh+uN1E4X8E5DzJIlj83S+tIMQZFgBAAAAACIAIJZAn7j5iOen7xo2sKzjMc24llTZIuS+RpdwcLHtE6ufAUCksqYUJBbHB9x8eHdoRvRqiGzG4wQXpmY96vh14zAJEM2CS/oZaNVC4Wj8rY2cdjAvZj9dlVZFPbOxx9g5tFxUAUA24s2KJ7sjSHUAcUSd4yqRK/G3CZM8qhkhyHhGDSS0zZvZaIcgoqOPe23gH32wAI9Aax1gJUDv4kKOqOx64ltg9BADAAEBKxBkWAEAAAAAIgAglkCfuPmI56fvGjawrOMxzbiWVNki5L5Gl3Bwse0Tq58BBYZSIQIeYxzruE4/cvi6zbRmB1asJO0bMfUutoH0bpubw1zAZSEDLZSmORZKW/k5A+4QxJR2/H+vcV8U0WPX9SvS+MRMffNSrnNkdqkUmNf1mL657o/oxxnHkIrtdNkbge+IrGt2qRSIigBO15eaB9dj93ihNpAX9HHDuoisbJNRiAP//wCyaCIGAh5jHOu4Tj9y+LrNtGYHVqwk7Rsx9S62gfRum5vDXMBlHPcUwigwAACAAQAAgAAAAIACAACAAAAAAAAAAAAiBgIr7HqsyKEvERWQsmsv6FleMuXThpI77+TVkQ3TSOOLURz3FMIoMAAAgAEAAIAAAACAAgAAgAIAAAAAAAAAIgYDLZSmORZKW/k5A+4QxJR2/H+vcV8U0WPX9SvS+MRMffMcJSLyPDAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA/h0pUXGHq1+kSuTYVTO8RHKfQLJlhfNtm+qdcIIr09jHCUi8jwwAACAAQAAgAAAAIACAACAAgAAAAAAAAAAIgICGAO/4xFiX/S5DXTV6uARFTcMwP1hto8BtPkdn3gIjf0c9xTCKDAAAIABAACAAAAAgAIAAIACAAAAAgAAACICAuNOSbsNRv31XkF2ygwCOuCnsJNRLhV0isJ/VRdj1k7IHPcUwigwAACAAQAAgAAAAIACAACAAAAAAAIAAAAiAgOpBJHEchNOeXuQwuLHlwOfkAyfoGvrYfb4pCFLKEPw2hwlIvI8MAAAgAEAAIAAAACAAgAAgAIAAAACAAAAIgIDyLkJiZTjLCysDOQotYs9us5CEYev4kyTYW2uL2r5H1McJSLyPDAAAIABAACAAAAAgAIAAIAAAAAAAgAAAAAiAgIlvGBvHRPmmVP6sn9g/akW2VJAvbJagMnZ/24gLdITsxz3FMIoMAAAgAEAAIAAAACAAgAAgAMAAAADAAAAIgIDNmVQOMMezQgABjk1zjfc3I2eKFJ4xLqT55jG4BP4p0Ec9xTCKDAAAIABAACAAAAAgAIAAIABAAAAAwAAACICA4Subm7T6yYCMWLgDtMy92hOgjanJefukbCOSVEHlX0IHCUi8jwwAACAAQAAgAAAAIACAACAAQAAAAMAAAAiAgPpsETw12nxLEM6OSOPfxp4YYj8NtRcLdqBpi3S4/BTuRwlIvI8MAAAgAEAAIAAAACAAgAAgAMAAAADAAAAAA==";

    pub(crate) fn spend_tx() -> SpendTx {
        let desc = CoincubeDescriptor::from_str(DESC).unwrap();
        let psbt = Psbt::from_str(UNSIGNED_PSBT).unwrap();
        let secp = Secp256k1::verification_only();
        SpendTx::new(None, psbt, vec![], &desc, &secp, Network::Signet)
    }

    #[test]
    fn hidden_psbt_list_masks_spend_and_fee_without_removing_entry() {
        let mut tx = spend_tx();
        // The shared PSBT is a self-transfer; make this rendering fixture an
        // outbound row with nonzero values so both amount fields are visible.
        tx.kind = TransactionKind::OutgoingPaymentBatch(vec![]);
        // Ten BTC in the default Sats unit exercises the narrow Amount cell.
        tx.spend_amount = Amount::from_sat(1_000_000_000);
        tx.fee_amount = Some(Amount::from_sat(12_345));
        assert!(tx.spend_amount.to_sat() > 0);
        let raw_spend = tx
            .spend_amount
            .to_formatted_string_with_unit(BitcoinDisplayUnit::Sats);
        let fee = tx
            .fee_amount
            .map(|amount| amount.to_formatted_string_with_unit(BitcoinDisplayUnit::Sats));
        let rows = [tx];
        for size in [(1024, 768), (1440, 900)] {
            for (appearance, theme) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                for masked in [true, false] {
                    assert_psbt_privacy_view(
                        &rows,
                        &raw_spend,
                        fee.as_deref(),
                        size,
                        appearance,
                        theme,
                        masked,
                    );
                }
            }
        }
    }

    fn assert_psbt_privacy_view(
        rows: &[SpendTx],
        raw_spend: &str,
        fee: Option<&str>,
        size: (u32, u32),
        appearance: &str,
        theme: theme::Theme,
        masked: bool,
    ) {
        let mounted = capture::inspect(
            if masked {
                "vault-psbts-private"
            } else {
                "vault-psbts-shown"
            },
            view::vault::psbts::psbts_view(rows, BitcoinDisplayUnit::Sats, masked),
            size,
            appearance,
            theme,
        );
        assert!(mounted.contains("PSBT proposals"));
        assert_eq!(mounted.contains(raw_spend), !masked);
        if let Some(fee) = fee {
            assert_eq!(mounted.contains(fee), !masked);
        }
        assert_eq!(mounted.contains("••••••"), masked);
        if size == (1024, 768) && !masked {
            let partial = capture::inspect(
                "vault-psbts-partially-signed",
                coincube_ui::widget::Container::new(
                    view::vault::psbts::spend_tx_list_view_with_status(
                        0,
                        &rows[0],
                        BitcoinDisplayUnit::Sats,
                        false,
                        ("Partially signed", theme::pill::warning),
                    ),
                )
                .width(iced::Length::Fixed(652.0))
                .into(),
                size,
                appearance,
                theme,
            );
            assert!(partial.contains("Partially signed"));
            let status_height = partial
                .text
                .iter()
                .find(|(value, _)| value == "Partially signed")
                .map(|(_, bounds)| bounds.height)
                .expect("captured status label");
            assert!(status_height <= 24.0, "status pill text wrapped");
        }
    }

    /// Only a PSBT that Cube › Activity opened sends its Back there; one
    /// opened from this panel's own list (or a direct route) stays here.
    #[test]
    fn only_an_activity_opened_psbt_returns_to_activity() {
        let wallet = Arc::new(Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap()));
        let mut panel = PsbtsPanel::new(wallet);
        assert!(!panel.returns_to_activity());

        panel.preselect_from_activity(spend_tx());
        assert!(panel.returns_to_activity());

        panel.preselect(spend_tx());
        assert!(!panel.returns_to_activity(), "direct route keeps list Back");

        panel.preselect_from_activity(spend_tx());
        assert!(panel.selected_tx.as_ref().unwrap().back_to_activity);
        panel.clear_selection();
        assert!(!panel.returns_to_activity());
        assert!(panel.selected_tx.is_none());

        panel.preselect(spend_tx());
        assert!(
            !panel.selected_tx.as_ref().unwrap().back_to_activity,
            "a list-opened PSBT keeps the PSBT labels"
        );
    }

    /// Opening a PSBT from Activity reloads this panel, and the reload's
    /// `SpendTxs` rebuilds the open detail. The rebuilt detail must keep
    /// "‹ Activity", matching where its Back goes.
    #[test]
    fn a_refresh_rebuild_keeps_the_activity_back_label() {
        let wallet = Arc::new(Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap()));
        let mut panel = PsbtsPanel::new(wallet);
        panel.preselect_from_activity(spend_tx());
        // Marks the pre-refresh state: a rebuild resets it to saved.
        panel.selected_tx.as_mut().unwrap().saved = false;

        let daemon: Arc<dyn Daemon + Sync + Send> =
            Arc::new(Coincubed::new(MockDaemon::new(vec![]).run()));
        drop(panel.update(
            Some(daemon),
            &Cache::default(),
            Message::SpendTxs(Ok(vec![spend_tx()])),
        ));

        let rebuilt = panel.selected_tx.as_ref().expect("detail stays open");
        assert!(rebuilt.saved, "the refresh rebuilt the detail");
        assert!(rebuilt.back_to_activity, "label follows Back");
        assert!(panel.returns_to_activity());
    }

    // Regression: a background `SpendTxs` refresh must NOT rebuild the open
    // PSBT panel while a modal is up — doing so drops the in-progress Sign
    // session (and its in-flight Keychain sessions) and re-exposes only the
    // signatures persisted so far.
    #[test]
    fn spendtxs_refresh_preserves_an_open_modal() {
        let wallet = Arc::new(Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap()));
        let tx = spend_tx();

        let mut panel = PsbtsPanel::new(wallet.clone());
        panel.preselect(tx.clone());
        // Simulate an open Sign session by attaching a modal to the panel.
        panel.selected_tx.as_mut().unwrap().modal =
            Some(psbt::PsbtModal::Save(psbt::SaveModal::default()));

        // The daemon must never be hit on this path (we return before using it);
        // an empty mock enforces that.
        let daemon: Arc<dyn Daemon + Sync + Send> =
            Arc::new(Coincubed::new(MockDaemon::new(vec![]).run()));

        let _ = panel.update(
            Some(daemon),
            &Cache::default(),
            Message::SpendTxs(Ok(vec![tx])),
        );

        assert!(
            panel
                .selected_tx
                .as_ref()
                .expect("selected tx retained")
                .modal
                .is_some(),
            "open modal must survive a background SpendTxs refresh"
        );
    }
}
