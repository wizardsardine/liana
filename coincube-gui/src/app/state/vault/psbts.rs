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
}

impl PsbtsPanel {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self {
            wallet,
            spend_txs: Vec::new(),
            warning: None,
            selected_tx: None,
            modal: None,
        }
    }

    pub fn preselect(&mut self, spend_tx: SpendTx) {
        let psbt_state = psbt::PsbtState::new(self.wallet.clone(), spend_tx, true);
        self.selected_tx = Some(psbt_state);
        self.warning = None;
        self.modal = None;
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
                view::vault::psbts::psbts_view(&self.spend_txs, cache.bitcoin_unit),
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
            Message::SpendTxs(res) => match res {
                Err(e) => {
                    let err_msg = e.to_string();
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
                            let tx = psbt::PsbtState::new(self.wallet.clone(), tx.clone(), true);
                            let cmd = tx.load(daemon);
                            self.selected_tx = Some(tx);
                            return cmd;
                        }
                    }
                }
            },
            Message::View(view::Message::ImportPsbt) => {
                if let Some(tx) = &mut self.selected_tx {
                    return tx.update(daemon, cache, message);
                } else if self.modal.is_none() {
                    let modal = VaultExportModal::new(
                        Some(daemon.clone()),
                        ImportExportType::ImportPsbt(None),
                    );
                    let launch = modal.launch(false);
                    self.modal = Some(modal);
                    return launch;
                }
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
mod tests {
    use super::*;
    use crate::{daemon::client::Coincubed, utils::mock::Daemon as MockDaemon};
    use coincube_core::{
        descriptors::CoincubeDescriptor,
        miniscript::bitcoin::{secp256k1::Secp256k1, Network, Psbt},
    };
    use std::str::FromStr;

    const DESC: &str = "wsh(or_d(multi(2,[f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<0;1>/*,[2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<0;1>/*),and_v(v:thresh(1,pkh([f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<2;3>/*),a:pkh([2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<2;3>/*)),older(65535))))#9s8ekrce";

    // Unsigned PSBT spending a Coincube coin under `DESC` (mirrors the fixture
    // used by the psbt panel tests).
    const UNSIGNED_PSBT: &str = "cHNidP8BAIkCAAAAAc0x/jtWvFugrl8zc34KVIlWCugXT6JNtgir6UqX+Vv6AQAAAAD9////AkBCDwAAAAAAIgAgtQu/fA/8rQhJ0I6wUoBDO0vNa3lgsEpEIj7rTOMnBcXuIEkBAAAAACIAIOdCiXh7yL2V/f6S6KMTOzgqKkqyIXgmFuwDnmXbIiosAAAAAAABAP04AQIAAAAAAQKYYriMs/PtSqm6LPNWWFYskTL6nWZegJdwxYcVCRn8vwEAAAAA/f///87D7dkdgMd1Laj/v6xspNRtrQXGP+8BPFMLqkeBb6MRAQAAAAD9////AuGQDgAAAAAAIlEg7DgdNxI7WybaPUZXcMCh+uN1E4X8E5DzJIlj83S+tIMQZFgBAAAAACIAIJZAn7j5iOen7xo2sKzjMc24llTZIuS+RpdwcLHtE6ufAUCksqYUJBbHB9x8eHdoRvRqiGzG4wQXpmY96vh14zAJEM2CS/oZaNVC4Wj8rY2cdjAvZj9dlVZFPbOxx9g5tFxUAUA24s2KJ7sjSHUAcUSd4yqRK/G3CZM8qhkhyHhGDSS0zZvZaIcgoqOPe23gH32wAI9Aax1gJUDv4kKOqOx64ltg9BADAAEBKxBkWAEAAAAAIgAglkCfuPmI56fvGjawrOMxzbiWVNki5L5Gl3Bwse0Tq58BBYZSIQIeYxzruE4/cvi6zbRmB1asJO0bMfUutoH0bpubw1zAZSEDLZSmORZKW/k5A+4QxJR2/H+vcV8U0WPX9SvS+MRMffNSrnNkdqkUmNf1mL657o/oxxnHkIrtdNkbge+IrGt2qRSIigBO15eaB9dj93ihNpAX9HHDuoisbJNRiAP//wCyaCIGAh5jHOu4Tj9y+LrNtGYHVqwk7Rsx9S62gfRum5vDXMBlHPcUwigwAACAAQAAgAAAAIACAACAAAAAAAAAAAAiBgIr7HqsyKEvERWQsmsv6FleMuXThpI77+TVkQ3TSOOLURz3FMIoMAAAgAEAAIAAAACAAgAAgAIAAAAAAAAAIgYDLZSmORZKW/k5A+4QxJR2/H+vcV8U0WPX9SvS+MRMffMcJSLyPDAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA/h0pUXGHq1+kSuTYVTO8RHKfQLJlhfNtm+qdcIIr09jHCUi8jwwAACAAQAAgAAAAIACAACAAgAAAAAAAAAAIgICGAO/4xFiX/S5DXTV6uARFTcMwP1hto8BtPkdn3gIjf0c9xTCKDAAAIABAACAAAAAgAIAAIACAAAAAgAAACICAuNOSbsNRv31XkF2ygwCOuCnsJNRLhV0isJ/VRdj1k7IHPcUwigwAACAAQAAgAAAAIACAACAAAAAAAIAAAAiAgOpBJHEchNOeXuQwuLHlwOfkAyfoGvrYfb4pCFLKEPw2hwlIvI8MAAAgAEAAIAAAACAAgAAgAIAAAACAAAAIgIDyLkJiZTjLCysDOQotYs9us5CEYev4kyTYW2uL2r5H1McJSLyPDAAAIABAACAAAAAgAIAAIAAAAAAAgAAAAAiAgIlvGBvHRPmmVP6sn9g/akW2VJAvbJagMnZ/24gLdITsxz3FMIoMAAAgAEAAIAAAACAAgAAgAMAAAADAAAAIgIDNmVQOMMezQgABjk1zjfc3I2eKFJ4xLqT55jG4BP4p0Ec9xTCKDAAAIABAACAAAAAgAIAAIABAAAAAwAAACICA4Subm7T6yYCMWLgDtMy92hOgjanJefukbCOSVEHlX0IHCUi8jwwAACAAQAAgAAAAIACAACAAQAAAAMAAAAiAgPpsETw12nxLEM6OSOPfxp4YYj8NtRcLdqBpi3S4/BTuRwlIvI8MAAAgAEAAIAAAACAAgAAgAMAAAADAAAAAA==";

    fn spend_tx() -> SpendTx {
        let desc = CoincubeDescriptor::from_str(DESC).unwrap();
        let psbt = Psbt::from_str(UNSIGNED_PSBT).unwrap();
        let secp = Secp256k1::verification_only();
        SpendTx::new(None, psbt, vec![], &desc, &secp, Network::Signet)
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
