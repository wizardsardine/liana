use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use iced::Subscription;

use iced::Task;
use liana::{
    descriptors::LianaPolicy,
    miniscript::bitcoin::{bip32::Fingerprint, psbt::Psbt, Network, Txid},
};
use lianad::commands::CoinStatus;

use liana_ui::component::toast;
use liana_ui::{component::modal, widget::Element};

use crate::daemon::model::LabelsLoader;
use crate::export::{ImportExportMessage, ImportExportType, Progress};
use crate::{
    app::{
        cache::Cache,
        error::Error,
        message::Message,
        state::label::{label_item_from_str, LabelsEdited},
        view,
        wallet::{Wallet, WalletError},
    },
    daemon::{
        model::{LabelItem, Labelled, SpendStatus, SpendTx},
        Daemon,
    },
    dir::LianaDirectory,
    hw::{HardwareWallet, HardwareWallets},
};

use super::export::ExportModal;

pub trait Modal {
    fn load(&self, _daemon: Arc<dyn Daemon + Sync + Send>) -> Task<Message> {
        Task::none()
    }
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _message: Message,
        _tx: &mut SpendTx,
    ) -> Task<Message> {
        Task::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message>;
}

pub enum PsbtModal {
    Save(SaveModal),
    Sign(SignModal),
    Broadcast(BroadcastModal),
    Delete(DeleteModal),
    Export(ExportModal),
    SendPayjoin(SendPayjoinModal),
}

impl<'a> AsRef<dyn Modal + 'a> for PsbtModal {
    fn as_ref(&self) -> &(dyn Modal + 'a) {
        match &self {
            Self::Save(a) => a,
            Self::Sign(a) => a,
            Self::Broadcast(a) => a,
            Self::Delete(a) => a,
            Self::Export(a) => a,
            Self::SendPayjoin(a) => a,
        }
    }
}

impl<'a> AsMut<dyn Modal + 'a> for PsbtModal {
    fn as_mut(&mut self) -> &mut (dyn Modal + 'a) {
        match self {
            Self::Save(a) => a,
            Self::Sign(a) => a,
            Self::Broadcast(a) => a,
            Self::Delete(a) => a,
            Self::Export(a) => a,
            Self::SendPayjoin(a) => a,
        }
    }
}

pub struct PsbtState {
    pub wallet: Arc<Wallet>,
    pub desc_policy: LianaPolicy,
    pub tx: SpendTx,
    pub bip21: Option<String>,
    pub saved: bool,
    pub warning: Option<Error>,
    pub labels_edited: LabelsEdited,
    pub modal: Option<PsbtModal>,
}

impl PsbtState {
    pub fn new(wallet: Arc<Wallet>, tx: SpendTx, saved: bool, bip21: Option<String>) -> Self {
        Self {
            desc_policy: wallet.main_descriptor.policy(),
            wallet,
            labels_edited: LabelsEdited::default(),
            warning: None,
            modal: None,
            tx,
            bip21,
            saved,
        }
    }

    pub fn interrupt(&mut self) {
        self.modal = None;
    }

    pub fn subscription(&self) -> Subscription<Message> {
        if let Some(modal) = &self.modal {
            modal.as_ref().subscription()
        } else {
            Subscription::none()
        }
    }

    pub fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Task<Message> {
        if let Some(modal) = &self.modal {
            modal.as_ref().load(daemon)
        } else {
            Task::none()
        }
    }

    pub fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::ExportPsbt) => {
                if self.modal.is_none() {
                    let psbt_str = self.tx.psbt.to_string();
                    let modal = ExportModal::new(None, ImportExportType::ExportPsbt(psbt_str));
                    let launch = modal.launch(true);
                    self.modal = Some(PsbtModal::Export(modal));
                    return launch;
                }
            }
            Message::View(view::Message::ImportPsbt) => {
                if self.modal.is_none() {
                    let modal = ExportModal::new(
                        Some(daemon.clone()),
                        ImportExportType::ImportPsbt(Some(self.tx.psbt.unsigned_tx.compute_txid())),
                    );
                    let launch = modal.launch(false);
                    self.modal = Some(PsbtModal::Export(modal));
                    return launch;
                }
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Close)) => {
                if matches!(self.modal, Some(PsbtModal::Export(_))) {
                    self.modal = None;
                }
            }
            Message::View(view::Message::ImportExport(m)) => {
                if let Some(PsbtModal::Export(modal)) = self.modal.as_mut() {
                    return modal.update(m);
                }
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Cancel)) => {
                if let Some(PsbtModal::Sign(SignModal {
                    display_modal,
                    signing,
                    ..
                })) = &mut self.modal
                {
                    if !signing.is_empty() {
                        *display_modal = false;
                        return Task::none();
                    }
                }

                self.modal = None;
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Delete)) => {
                self.modal = Some(PsbtModal::Delete(DeleteModal::default()));
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::SendPayjoin)) => {
                let modal = SendPayjoinModal;
                let cmd = modal.load(daemon);
                self.modal = Some(PsbtModal::SendPayjoin(modal));
                return cmd;
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::PayjoinInitiated)) => {
                self.tx.status = SpendStatus::PayjoinInitiated;
                self.modal = None;
                if let Some(_payjoin_info) = self.tx.payjoin_status {
                    let psbt = self.tx.psbt.clone();
                    // TODO: should this be an error?
                    let bip21 = self.bip21.clone().expect("bip21 should be set");
                    return Task::perform(
                        async move {
                            daemon
                                .send_payjoin(bip21, &psbt)
                                .await
                                .map_err(|e| e.into())
                        },
                        Message::SendPayjoin,
                    );
                }
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Sign)) => {
                if let Some(PsbtModal::Sign(SignModal { display_modal, .. })) = &mut self.modal {
                    *display_modal = true;
                    return Task::none();
                }

                let modal = SignModal::new(
                    self.tx.signers(),
                    self.wallet.clone(),
                    cache.datadir_path.clone(),
                    cache.network,
                    self.saved,
                    self.tx.recovery_timelock(),
                );
                let cmd = modal.load(daemon);
                self.modal = Some(PsbtModal::Sign(modal));
                return cmd;
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Broadcast)) => {
                let outpoints: Vec<_> = self.tx.coins.keys().cloned().collect();
                return Task::perform(
                    async move {
                        daemon
                            .list_coins(&[CoinStatus::Spending], &outpoints)
                            .await
                            .map(|res| {
                                res.coins
                                    .iter()
                                    .filter_map(|c| c.spend_info.map(|info| info.txid))
                                    .collect()
                            })
                            .map_err(|e| e.into())
                    },
                    Message::BroadcastModal,
                );
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Save)) => {
                self.modal = Some(PsbtModal::Save(SaveModal::default()));
            }
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    std::iter::once(&mut self.tx).map(|tx| tx as &mut dyn LabelsLoader),
                ) {
                    Ok(cmd) => {
                        return cmd;
                    }
                    Err(e) => {
                        self.warning = Some(e);
                    }
                };
            }
            Message::Updated(Ok(_)) => {
                self.saved = true;
                if let Some(modal) = self.modal.as_mut() {
                    let cmd = modal.as_mut().update(daemon.clone(), message, &mut self.tx);
                    // if modal is only the pending notif then we remove it once the psbt was
                    // updated.
                    if let PsbtModal::Sign(SignModal { display_modal, .. }) = modal {
                        if !*display_modal {
                            self.modal = None;
                        }
                    }
                    return cmd;
                }
            }
            Message::BroadcastModal(res) => match res {
                Ok(conflicting_txids) => {
                    self.modal = Some(PsbtModal::Broadcast(BroadcastModal {
                        conflicting_txids,
                        ..Default::default()
                    }));
                }
                Err(e) => {
                    self.warning = Some(e);
                }
            },
            Message::Export(ImportExportMessage::Progress(Progress::Psbt(psbt))) => {
                merge_signatures(&mut self.tx.psbt, &psbt);
                self.tx.sigs = self
                    .wallet
                    .main_descriptor
                    .partial_spend_info(&self.tx.psbt)
                    .expect("already check in psbt import logic");
            }
            _ => {
                if let Some(modal) = self.modal.as_mut() {
                    return modal.as_mut().update(daemon.clone(), message, &mut self.tx);
                }
            }
        };
        Task::none()
    }

    pub fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = view::psbt::psbt_view(
            cache,
            &self.tx,
            self.saved,
            &self.desc_policy,
            &self.wallet.keys_aliases,
            self.labels_edited.cache(),
            cache.network,
            if let Some(PsbtModal::Sign(m)) = &self.modal {
                m.is_signing()
            } else {
                false
            },
            self.warning.as_ref(),
        );
        if let Some(modal) = &self.modal {
            modal.as_ref().view(content)
        } else {
            content
        }
    }
}

#[derive(Default)]
pub struct SendPayjoinModal;

impl Modal for SendPayjoinModal {
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(content, view::psbt::payjoin_send_success_view())
            // On blur, show the psbts view
            .on_blur(Some(view::Message::Spend(
                view::SpendTxMessage::PayjoinInitiated,
            )))
            .into()
    }
}

#[derive(Default)]
pub struct SaveModal {
    saved: bool,
    error: Option<Error>,
}

impl Modal for SaveModal {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::Confirm)) => {
                let daemon = daemon.clone();
                let psbt = tx.psbt.clone();
                let mut labels = HashMap::<LabelItem, Option<String>>::new();
                for (item, label) in tx.labels() {
                    if !label.is_empty() {
                        labels.insert(label_item_from_str(item), Some(label.clone()));
                    }
                }
                return Task::perform(
                    async move {
                        daemon.update_spend_tx(&psbt).await?;
                        daemon.update_labels(&labels).await.map_err(|e| e.into())
                    },
                    Message::Updated,
                );
            }
            Message::Updated(res) => match res {
                Ok(()) => self.saved = true,
                Err(e) => self.error = Some(e),
            },
            _ => {}
        }
        Task::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(
            content,
            view::psbt::save_action(self.error.as_ref(), self.saved),
        )
        .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
        .into()
    }
}

#[derive(Default)]
pub struct BroadcastModal {
    broadcast: bool,
    error: Option<Error>,
    /// IDs of any directly conflicting transactions.
    conflicting_txids: HashSet<Txid>,
}

impl Modal for BroadcastModal {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::Confirm)) => {
                let daemon = daemon.clone();
                let psbt = tx.psbt.clone();
                self.error = None;
                return Task::perform(
                    async move {
                        daemon
                            .broadcast_spend_tx(&psbt.unsigned_tx.compute_txid())
                            .await
                            .map_err(|e| e.into())
                    },
                    Message::Updated,
                );
            }
            Message::Updated(res) => match res {
                Ok(()) => {
                    tx.status = SpendStatus::Broadcast;
                    self.broadcast = true;
                }
                Err(e) => self.error = Some(e),
            },
            _ => {}
        }
        Task::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(
            content,
            view::psbt::broadcast_action(
                &self.conflicting_txids,
                self.error.as_ref(),
                self.broadcast,
            ),
        )
        .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
        .into()
    }
}

#[derive(Default)]
pub struct DeleteModal {
    deleted: bool,
    error: Option<Error>,
}

impl Modal for DeleteModal {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::Confirm)) => {
                let daemon = daemon.clone();
                let psbt = tx.psbt.clone();
                self.error = None;
                return Task::perform(
                    async move {
                        daemon
                            .delete_spend_tx(&psbt.unsigned_tx.compute_txid())
                            .await
                            .map_err(|e| e.into())
                    },
                    Message::Updated,
                );
            }
            Message::Updated(res) => match res {
                Ok(()) => self.deleted = true,
                Err(e) => self.error = Some(e),
            },
            _ => {}
        }
        Task::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(
            content,
            view::psbt::delete_action(self.error.as_ref(), self.deleted),
        )
        .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
        .into()
    }
}

pub struct SignModal {
    wallet: Arc<Wallet>,
    hws: HardwareWallets,
    error: Option<Error>,
    signing: HashSet<Fingerprint>,
    signed: HashSet<Fingerprint>,
    is_saved: bool,
    display_modal: bool,
    recovery_timelock: Option<u16>,
}

impl SignModal {
    pub fn new(
        signed: HashSet<Fingerprint>,
        wallet: Arc<Wallet>,
        datadir_path: LianaDirectory,
        network: Network,
        is_saved: bool,
        recovery_timelock: Option<u16>,
    ) -> Self {
        Self {
            signing: HashSet::new(),
            hws: HardwareWallets::new(datadir_path, network).with_wallet(wallet.clone()),
            wallet,
            error: None,
            signed,
            is_saved,
            display_modal: true,
            recovery_timelock,
        }
    }

    pub fn is_signing(&self) -> bool {
        !self.signing.is_empty()
    }
}

impl Modal for SignModal {
    fn subscription(&self) -> Subscription<Message> {
        self.hws.refresh().map(Message::HardwareWallets)
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::SelectHardwareWallet(i)) => {
                if let Some(HardwareWallet::Supported {
                    fingerprint,
                    device,
                    ..
                }) = self.hws.list.get(i)
                {
                    self.display_modal = false;
                    self.signing.insert(*fingerprint);
                    let psbt = tx.psbt.clone();
                    let fingerprint = *fingerprint;
                    return Task::perform(
                        sign_psbt(self.wallet.clone(), device.clone(), psbt),
                        move |res| Message::Signed(fingerprint, res),
                    );
                }
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::SelectHotSigner)) => {
                return Task::perform(
                    sign_psbt_with_hot_signer(self.wallet.clone(), tx.psbt.clone()),
                    |(fg, res)| Message::Signed(fg, res),
                );
            }
            Message::Signed(fingerprint, res) => {
                self.signing.remove(&fingerprint);
                match res {
                    Err(e) => {
                        self.display_modal = true;
                        if !matches!(e, Error::HardwareWallet(async_hwi::Error::UserRefused)) {
                            self.error = Some(e)
                        }
                    }
                    Ok(psbt) => {
                        self.error = None;
                        self.signed.insert(fingerprint);
                        let daemon = daemon.clone();
                        merge_signatures(&mut tx.psbt, &psbt);

                        if self.is_saved {
                            return Task::perform(
                                async move { daemon.update_spend_tx(&psbt).await.map_err(|e| e.into()) },
                                Message::Updated,
                            );
                        // If the spend transaction was never saved before, then both the psbt and
                        // labels attached to it must be updated.
                        } else {
                            let mut labels = HashMap::<LabelItem, Option<String>>::new();
                            for (item, label) in tx.labels() {
                                if !label.is_empty() {
                                    labels.insert(label_item_from_str(item), Some(label.clone()));
                                }
                            }
                            return Task::perform(
                                async move {
                                    daemon.update_spend_tx(&psbt).await?;
                                    daemon.update_labels(&labels).await.map_err(|e| e.into())
                                },
                                Message::Updated,
                            );
                        }
                    }
                }
            }
            Message::Updated(res) => match res {
                Ok(()) => match self.wallet.main_descriptor.partial_spend_info(&tx.psbt) {
                    Ok(sigs) => tx.sigs = sigs,
                    Err(e) => self.error = Some(Error::Unexpected(e.to_string())),
                },
                Err(e) => self.error = Some(e),
            },

            Message::HardwareWallets(msg) => match self.hws.update(msg) {
                Ok(cmd) => {
                    return cmd.map(Message::HardwareWallets);
                }
                Err(e) => {
                    self.error = Some(e.into());
                }
            },
            _ => {}
        };
        Task::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        let content = toast::Manager::new(
            content,
            view::psbt::sign_action_toasts(self.error.as_ref(), &self.hws.list, &self.signing),
        )
        .into();
        if self.display_modal {
            modal::Modal::new(
                content,
                view::psbt::sign_action(
                    self.error.as_ref(),
                    &self.hws.list,
                    &self.wallet.main_descriptor,
                    self.wallet.signer.as_ref().map(|s| s.fingerprint()),
                    self.wallet
                        .signer
                        .as_ref()
                        .and_then(|signer| self.wallet.keys_aliases.get(&signer.fingerprint)),
                    &self.signed,
                    &self.signing,
                    self.recovery_timelock,
                ),
            )
            .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
            .into()
        } else {
            content
        }
    }
}

fn merge_signatures(psbt: &mut Psbt, signed_psbt: &Psbt) {
    for i in 0..signed_psbt.inputs.len() {
        let psbtin = match psbt.inputs.get_mut(i) {
            Some(psbtin) => psbtin,
            None => continue,
        };
        let signed_psbtin = match signed_psbt.inputs.get(i) {
            Some(signed_psbtin) => signed_psbtin,
            None => continue,
        };
        psbtin
            .partial_sigs
            .extend(&mut signed_psbtin.partial_sigs.iter());
        psbtin
            .tap_script_sigs
            .extend(&mut signed_psbtin.tap_script_sigs.iter());
        if let Some(sig) = signed_psbtin.tap_key_sig {
            psbtin.tap_key_sig = Some(sig);
        }
    }
}

async fn sign_psbt_with_hot_signer(
    wallet: Arc<Wallet>,
    psbt: Psbt,
) -> (Fingerprint, Result<Psbt, Error>) {
    if let Some(signer) = &wallet.signer {
        let res = signer
            .sign_psbt(psbt)
            .map_err(|e| WalletError::HotSigner(format!("Hot signer failed to sign psbt: {}", e)))
            .map_err(|e| e.into());
        (signer.fingerprint(), res)
    } else {
        (
            Fingerprint::default(),
            Err(WalletError::HotSigner("Hot signer not loaded".to_string()).into()),
        )
    }
}

async fn sign_psbt(
    wallet: Arc<Wallet>,
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    mut psbt: Psbt,
) -> Result<Psbt, Error> {
    // The BitBox02 is only going to produce a signature for a single key in the Script. In order
    // to make sure it doesn't sign for a public key from another spending path we remove the BIP32
    // derivation for the other paths.
    if matches!(hw.device_kind(), async_hwi::DeviceKind::BitBox02) {
        // We need to make sure we don't prune the BIP32 derivations from the original PSBT (which
        // would end up being updated in the daemon's database and erase the previously unpruned
        // one). To this end we create a new, pruned, psbt we use for signing and then merge its
        // signatures back into the original PSBT.
        let mut pruned_psbt = wallet
            .main_descriptor
            .prune_bip32_derivs_last_avail(psbt.clone())
            .map_err(Error::Desc)?;
        hw.sign_tx(&mut pruned_psbt).await.map_err(Error::from)?;
        for (i, psbt_in) in psbt.inputs.iter_mut().enumerate() {
            if let Some(pruned_psbt_in) = pruned_psbt.inputs.get_mut(i) {
                psbt_in
                    .partial_sigs
                    .append(&mut pruned_psbt_in.partial_sigs);
                if let Some(tap_key_sig) = pruned_psbt_in.tap_key_sig {
                    psbt_in.tap_key_sig = Some(tap_key_sig);
                }
                psbt_in
                    .tap_script_sigs
                    .append(&mut pruned_psbt_in.tap_script_sigs);
            } else {
                log::error!(
                    "Not all PSBT inputs are present in the pruned psbt. Pruned psbt: '{}'.",
                    &pruned_psbt
                );
            }
        }
    } else {
        hw.sign_tx(&mut psbt).await.map_err(Error::from)?;
    }
    Ok(psbt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{cache::Cache, state::PsbtsPanel},
        daemon::client::{Lianad, Request},
        utils::{mock::Daemon, sandbox::Sandbox},
    };

    use liana::descriptors::LianaDescriptor;
    use serde_json::json;
    use std::str::FromStr;

    const DESC: &str = "wsh(or_d(multi(2,[f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<0;1>/*,[2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<0;1>/*),and_v(v:thresh(1,pkh([f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<2;3>/*),a:pkh([2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<2;3>/*)),older(65535))))#9s8ekrce";

    #[tokio::test]
    async fn test_update_psbt() {
        let daemon = Daemon::new(vec![
            (
                Some(json!({"method": "getinfo", "params": Option::<Request>::None})),
                Ok(json!({
                    "version": "",
                    "network": "signet",
                    "block_height": 1000,
                    "sync": 1.0,
                    "descriptors": { "main": LianaDescriptor::from_str(DESC).unwrap() },
                    "receive_index": 4,
                    "change_index": 3,
                    "timestamp": 1000,
                })),
            ),
            (
                Some(json!({"method": "listspendtxs", "params": Option::<Request>::None})),
                Ok(json!({ "spend_txs": [{
                    "psbt": "cHNidP8BAIkCAAAAAc0x/jtWvFugrl8zc34KVIlWCugXT6JNtgir6UqX+Vv6AQAAAAD9////AkBCDwAAAAAAIgAgtQu/fA/8rQhJ0I6wUoBDO0vNa3lgsEpEIj7rTOMnBcXuIEkBAAAAACIAIOdCiXh7yL2V/f6S6KMTOzgqKkqyIXgmFuwDnmXbIiosAAAAAAABAP04AQIAAAAAAQKYYriMs/PtSqm6LPNWWFYskTL6nWZegJdwxYcVCRn8vwEAAAAA/f///87D7dkdgMd1Laj/v6xspNRtrQXGP+8BPFMLqkeBb6MRAQAAAAD9////AuGQDgAAAAAAIlEg7DgdNxI7WybaPUZXcMCh+uN1E4X8E5DzJIlj83S+tIMQZFgBAAAAACIAIJZAn7j5iOen7xo2sKzjMc24llTZIuS+RpdwcLHtE6ufAUCksqYUJBbHB9x8eHdoRvRqiGzG4wQXpmY96vh14zAJEM2CS/oZaNVC4Wj8rY2cdjAvZj9dlVZFPbOxx9g5tFxUAUA24s2KJ7sjSHUAcUSd4yqRK/G3CZM8qhkhyHhGDSS0zZvZaIcgoqOPe23gH32wAI9Aax1gJUDv4kKOqOx64ltg9BADAAEBKxBkWAEAAAAAIgAglkCfuPmI56fvGjawrOMxzbiWVNki5L5Gl3Bwse0Tq58BBYZSIQIeYxzruE4/cvi6zbRmB1asJO0bMfUutoH0bpubw1zAZSEDLZSmORZKW/k5A+4QxJR2/H+vcV8U0WPX9SvS+MRMffNSrnNkdqkUmNf1mL657o/oxxnHkIrtdNkbge+IrGt2qRSIigBO15eaB9dj93ihNpAX9HHDuoisbJNRiAP//wCyaCIGAh5jHOu4Tj9y+LrNtGYHVqwk7Rsx9S62gfRum5vDXMBlHPcUwigwAACAAQAAgAAAAIACAACAAAAAAAAAAAAiBgIr7HqsyKEvERWQsmsv6FleMuXThpI77+TVkQ3TSOOLURz3FMIoMAAAgAEAAIAAAACAAgAAgAIAAAAAAAAAIgYDLZSmORZKW/k5A+4QxJR2/H+vcV8U0WPX9SvS+MRMffMcJSLyPDAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA/h0pUXGHq1+kSuTYVTO8RHKfQLJlhfNtm+qdcIIr09jHCUi8jwwAACAAQAAgAAAAIACAACAAgAAAAAAAAAAIgICGAO/4xFiX/S5DXTV6uARFTcMwP1hto8BtPkdn3gIjf0c9xTCKDAAAIABAACAAAAAgAIAAIACAAAAAgAAACICAuNOSbsNRv31XkF2ygwCOuCnsJNRLhV0isJ/VRdj1k7IHPcUwigwAACAAQAAgAAAAIACAACAAAAAAAIAAAAiAgOpBJHEchNOeXuQwuLHlwOfkAyfoGvrYfb4pCFLKEPw2hwlIvI8MAAAgAEAAIAAAACAAgAAgAIAAAACAAAAIgIDyLkJiZTjLCysDOQotYs9us5CEYev4kyTYW2uL2r5H1McJSLyPDAAAIABAACAAAAAgAIAAIAAAAAAAgAAAAAiAgIlvGBvHRPmmVP6sn9g/akW2VJAvbJagMnZ/24gLdITsxz3FMIoMAAAgAEAAIAAAACAAgAAgAMAAAADAAAAIgIDNmVQOMMezQgABjk1zjfc3I2eKFJ4xLqT55jG4BP4p0Ec9xTCKDAAAIABAACAAAAAgAIAAIABAAAAAwAAACICA4Subm7T6yYCMWLgDtMy92hOgjanJefukbCOSVEHlX0IHCUi8jwwAACAAQAAgAAAAIACAACAAQAAAAMAAAAiAgPpsETw12nxLEM6OSOPfxp4YYj8NtRcLdqBpi3S4/BTuRwlIvI8MAAAgAEAAIAAAACAAgAAgAMAAAADAAAAAA==",
                }]})),
            ),
            (
                Some(
                    json!({"method": "listcoins", "params": vec![Vec::new(), vec!["fa5bf9974ae9ab08b64da24f17e80a5689540a7e73335faea05bbc563bfe31cd:1"]]}),
                ),
                Ok(json!({ "coins": [{
                    "amount": 10000,
                    "outpoint": "fa5bf9974ae9ab08b64da24f17e80a5689540a7e73335faea05bbc563bfe31cd:1",
                    "address": "TB1QJEQFLW8E3RN60MC6X6C2ECE3EKUFV4XEYTJTU35HWPCTRMGN4W0S3DCXH5",
                    "block_height": 200949,
                    "derivation_index": 0,
                    "is_immature": false,
                    "is_change": false,
                    "is_from_self": false,

                }]})),
            ),
            (
                Some(json!({"method": "getlabels", "params": vec![vec![
                    "4bc07e8fe753f7314b69da02a7cfbedc3e4e0d5fbee316a048240ae87b8aaa58",
                    "4bc07e8fe753f7314b69da02a7cfbedc3e4e0d5fbee316a048240ae87b8aaa58:0",
                    "4bc07e8fe753f7314b69da02a7cfbedc3e4e0d5fbee316a048240ae87b8aaa58:1",
                    "fa5bf9974ae9ab08b64da24f17e80a5689540a7e73335faea05bbc563bfe31cd:1",
                    "tb1qjeqflw8e3rn60mc6x6c2ece3ekufv4xeytjtu35hwpctrmgn4w0s3dcxh5",
                    "tb1qk59m7lq0ljkssjws36c99qzr8d9u66mevzcy53pz8m45ece8qhzs6alndx",
                    "tb1quapgj7rmez7etl07jt52xyem8q4z5j4jy9uzv9hvqw0xtkez9gkqaw7rgr",
                ]]})),
                Ok(json!({ "labels": {}})),
            ),
            (
                Some(json!({"method": "updatespend", "params": vec![vec![json!({})]]})),
                Ok(json!({})),
            ),
        ]);
        let wallet = Arc::new(Wallet::new(LianaDescriptor::from_str(DESC).unwrap()));
        let sandbox: Sandbox<PsbtsPanel> = Sandbox::new(PsbtsPanel::new(wallet.clone()));
        let client = Arc::new(Lianad::new(daemon.run()));
        let cache = Cache::default();
        let sandbox = sandbox
            .load(client.clone(), &Cache::default(), wallet)
            .await;
        let _sandbox = sandbox
            .update(
                client.clone(),
                &cache,
                Message::View(view::Message::Select(0)),
            )
            .await
            .update(
                client.clone(),
                &cache,
                Message::View(view::Message::Spend(view::SpendTxMessage::EditPsbt)),
            )
            .await
            .update(
                client.clone(),
                &cache,
                Message::View(view::Message::ImportSpend(
                    view::ImportSpendMessage::PsbtEdited("panic".to_string()),
                )),
            )
            .await
            .update(
                client.clone(),
                &cache,
                Message::View(view::Message::ImportSpend(
                    view::ImportSpendMessage::Confirm,
                )),
            )
            .await;
    }
}
