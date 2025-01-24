use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use iced::Subscription;

use iced::Command;
use liana::{
    descriptors::LianaPolicy,
    miniscript::bitcoin::{bip32::Fingerprint, psbt::Psbt, Network, Txid},
};
use lianad::commands::CoinStatus;

use liana_ui::component::toast;
use liana_ui::{
    component::{form, modal},
    widget::Element,
};

use crate::daemon::model::LabelsLoader;
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
    hw::{HardwareWallet, HardwareWallets},
};

pub trait Action {
    fn load(&self, _daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        Command::none()
    }
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _message: Message,
        _tx: &mut SpendTx,
    ) -> Command<Message> {
        Command::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message>;
}

pub enum PsbtAction {
    Save(SaveAction),
    Sign(SignAction),
    Update(UpdateAction),
    Broadcast(BroadcastAction),
    Delete(DeleteAction),
}

impl<'a> AsRef<dyn Action + 'a> for PsbtAction {
    fn as_ref(&self) -> &(dyn Action + 'a) {
        match &self {
            Self::Save(a) => a,
            Self::Sign(a) => a,
            Self::Update(a) => a,
            Self::Broadcast(a) => a,
            Self::Delete(a) => a,
        }
    }
}

impl<'a> AsMut<dyn Action + 'a> for PsbtAction {
    fn as_mut(&mut self) -> &mut (dyn Action + 'a) {
        match self {
            Self::Save(a) => a,
            Self::Sign(a) => a,
            Self::Update(a) => a,
            Self::Broadcast(a) => a,
            Self::Delete(a) => a,
        }
    }
}

pub struct PsbtState {
    pub wallet: Arc<Wallet>,
    pub desc_policy: LianaPolicy,
    pub tx: SpendTx,
    pub saved: bool,
    pub warning: Option<Error>,
    pub labels_edited: LabelsEdited,
    pub action: Option<PsbtAction>,
}

impl PsbtState {
    pub fn new(wallet: Arc<Wallet>, tx: SpendTx, saved: bool) -> Self {
        Self {
            desc_policy: wallet.main_descriptor.policy(),
            wallet,
            labels_edited: LabelsEdited::default(),
            warning: None,
            action: None,
            tx,
            saved,
        }
    }

    pub fn interrupt(&mut self) {
        self.action = None;
    }

    pub fn subscription(&self) -> Subscription<Message> {
        if let Some(action) = &self.action {
            action.as_ref().subscription()
        } else {
            Subscription::none()
        }
    }

    pub fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        if let Some(action) = &self.action {
            action.as_ref().load(daemon)
        } else {
            Command::none()
        }
    }

    pub fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::Cancel)) => {
                if let Some(PsbtAction::Sign(SignAction { display_modal, .. })) = &mut self.action {
                    *display_modal = false;
                    return Command::none();
                }

                self.action = None;
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Delete)) => {
                self.action = Some(PsbtAction::Delete(DeleteAction::default()));
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Sign)) => {
                if let Some(PsbtAction::Sign(SignAction { display_modal, .. })) = &mut self.action {
                    *display_modal = true;
                    return Command::none();
                }

                let action = SignAction::new(
                    self.tx.signers(),
                    self.wallet.clone(),
                    cache.datadir_path.clone(),
                    cache.network,
                    self.saved,
                );
                let cmd = action.load(daemon);
                self.action = Some(PsbtAction::Sign(action));
                return cmd;
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::EditPsbt)) => {
                let action = UpdateAction::new(self.wallet.clone(), self.tx.psbt.to_string());
                let cmd = action.load(daemon);
                self.action = Some(PsbtAction::Update(action));
                return cmd;
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Broadcast)) => {
                let outpoints: Vec<_> = self.tx.coins.keys().cloned().collect();
                return Command::perform(
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
                self.action = Some(PsbtAction::Save(SaveAction::default()));
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
                if let Some(action) = self.action.as_mut() {
                    return action
                        .as_mut()
                        .update(daemon.clone(), message, &mut self.tx);
                }
            }
            Message::BroadcastModal(res) => match res {
                Ok(conflicting_txids) => {
                    self.action = Some(PsbtAction::Broadcast(BroadcastAction {
                        conflicting_txids,
                        ..Default::default()
                    }));
                }
                Err(e) => {
                    self.warning = Some(e);
                }
            },
            _ => {
                if let Some(action) = self.action.as_mut() {
                    return action
                        .as_mut()
                        .update(daemon.clone(), message, &mut self.tx);
                }
            }
        };
        Command::none()
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
            self.warning.as_ref(),
        );
        if let Some(action) = &self.action {
            action.as_ref().view(content)
        } else {
            content
        }
    }
}

#[derive(Default)]
pub struct SaveAction {
    saved: bool,
    error: Option<Error>,
}

impl Action for SaveAction {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Command<Message> {
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
                return Command::perform(
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
        Command::none()
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
pub struct BroadcastAction {
    broadcast: bool,
    error: Option<Error>,
    /// IDs of any directly conflicting transactions.
    conflicting_txids: HashSet<Txid>,
}

impl Action for BroadcastAction {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::Confirm)) => {
                let daemon = daemon.clone();
                let psbt = tx.psbt.clone();
                self.error = None;
                return Command::perform(
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
        Command::none()
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
pub struct DeleteAction {
    deleted: bool,
    error: Option<Error>,
}

impl Action for DeleteAction {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::Confirm)) => {
                let daemon = daemon.clone();
                let psbt = tx.psbt.clone();
                self.error = None;
                return Command::perform(
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
        Command::none()
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

pub struct SignAction {
    wallet: Arc<Wallet>,
    hws: HardwareWallets,
    error: Option<Error>,
    signing: HashSet<Fingerprint>,
    signed: HashSet<Fingerprint>,
    is_saved: bool,
    display_modal: bool,
}

impl SignAction {
    pub fn new(
        signed: HashSet<Fingerprint>,
        wallet: Arc<Wallet>,
        datadir_path: PathBuf,
        network: Network,
        is_saved: bool,
    ) -> Self {
        Self {
            signing: HashSet::new(),
            hws: HardwareWallets::new(datadir_path, network).with_wallet(wallet.clone()),
            wallet,
            error: None,
            signed,
            is_saved,
            display_modal: true,
        }
    }
}

impl Action for SignAction {
    fn subscription(&self) -> Subscription<Message> {
        self.hws.refresh().map(Message::HardwareWallets)
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Command<Message> {
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
                    return Command::perform(
                        sign_psbt(self.wallet.clone(), device.clone(), psbt),
                        move |res| Message::Signed(fingerprint, res),
                    );
                }
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::SelectHotSigner)) => {
                return Command::perform(
                    sign_psbt_with_hot_signer(self.wallet.clone(), tx.psbt.clone()),
                    |(fg, res)| Message::Signed(fg, res),
                );
            }
            Message::Signed(fingerprint, res) => {
                self.signing.remove(&fingerprint);
                match res {
                    Err(e) => {
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
                            return Command::perform(
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
                            return Command::perform(
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
        Command::none()
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
                    self.wallet.signer.as_ref().map(|s| s.fingerprint()),
                    self.wallet
                        .signer
                        .as_ref()
                        .and_then(|signer| self.wallet.keys_aliases.get(&signer.fingerprint)),
                    &self.signed,
                    &self.signing,
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

pub struct UpdateAction {
    wallet: Arc<Wallet>,
    psbt: String,
    updated: form::Value<String>,
    processing: bool,
    error: Option<Error>,
    success: bool,
}

impl UpdateAction {
    pub fn new(wallet: Arc<Wallet>, psbt: String) -> Self {
        Self {
            wallet,
            psbt,
            updated: form::Value::default(),
            processing: false,
            error: None,
            success: false,
        }
    }
}

impl Action for UpdateAction {
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(
            content,
            if self.success {
                view::psbt::update_spend_success_view()
            } else {
                view::psbt::update_spend_view(
                    self.psbt.clone(),
                    &self.updated,
                    self.error.as_ref(),
                    self.processing,
                )
            },
        )
        .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
        .into()
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Command<Message> {
        match message {
            Message::Updated(res) => {
                self.processing = false;
                match res {
                    Ok(()) => {
                        self.success = true;
                        self.error = None;
                        let psbt = Psbt::from_str(&self.updated.value).expect("Already checked");
                        merge_signatures(&mut tx.psbt, &psbt);
                        tx.sigs = self
                            .wallet
                            .main_descriptor
                            .partial_spend_info(&tx.psbt)
                            .unwrap();
                    }
                    Err(e) => self.error = e.into(),
                }
            }
            Message::View(view::Message::ImportSpend(view::ImportSpendMessage::PsbtEdited(s))) => {
                self.updated.value = s;
                if let Ok(psbt) = Psbt::from_str(&self.updated.value) {
                    self.updated.valid =
                        tx.psbt.unsigned_tx.compute_txid() == psbt.unsigned_tx.compute_txid();
                } else {
                    self.updated.valid = false;
                }
            }
            Message::View(view::Message::ImportSpend(view::ImportSpendMessage::Confirm)) => {
                self.processing = true;
                self.error = None;
                if let Ok(updated) = Psbt::from_str(&self.updated.value) {
                    return Command::perform(
                        async move { daemon.update_spend_tx(&updated).await.map_err(|e| e.into()) },
                        Message::Updated,
                    );
                }
            }
            _ => {}
        }

        Command::none()
    }
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
