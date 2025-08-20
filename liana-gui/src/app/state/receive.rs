use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use iced::{widget::qr_code, Subscription, Task};
use liana::miniscript::bitcoin::{
    bip32::{ChildNumber, Fingerprint},
    Address, Network,
};
use liana_ui::{component::modal, widget::*};
use payjoin::Url;

use crate::daemon::model::LabelsLoader;
use crate::dir::LianaDirectory;
use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        message::Message,
        state::{label::LabelsEdited, State},
        view,
        wallet::Wallet,
    },
    hw::{HardwareWallet, HardwareWallets},
};

use crate::daemon::{
    model::{LabelItem, Labelled},
    Daemon,
};

const PREV_ADDRESSES_PAGE_SIZE: usize = 20;

pub enum Modal {
    VerifyAddress(VerifyAddressModal),
    ShowQrCode(ShowQrCodeModal),
    ShowBip21QrCode(ShowBip21QrCodeModal),
    None,
}

#[derive(Debug, Default)]
pub struct Addresses {
    list: Vec<Address>,
    bip21s: HashMap<Address, Url>,
    derivation_indexes: Vec<ChildNumber>,
    labels: HashMap<String, String>,
}

impl Addresses {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.derivation_indexes.is_empty() && self.labels.is_empty()
    }
}

impl Labelled for Addresses {
    fn labelled(&self) -> Vec<LabelItem> {
        self.list
            .iter()
            .map(|a| LabelItem::Address(a.clone()))
            .collect()
    }
    fn labels(&mut self) -> &mut HashMap<String, String> {
        &mut self.labels
    }
}

pub struct ReceivePanel {
    data_dir: LianaDirectory,
    wallet: Arc<Wallet>,
    addresses: Addresses,
    prev_addresses: Addresses,
    prev_continue_from: Option<ChildNumber>,
    show_prev_addresses: bool,
    selected: HashSet<Address>,
    labels_edited: LabelsEdited,
    modal: Modal,
    warning: Option<Error>,
    processing: bool,
}

impl ReceivePanel {
    pub fn new(data_dir: LianaDirectory, wallet: Arc<Wallet>) -> Self {
        Self {
            data_dir,
            wallet,
            addresses: Addresses::default(),
            prev_addresses: Addresses::default(),
            prev_continue_from: None,
            show_prev_addresses: false,
            selected: HashSet::new(),
            labels_edited: LabelsEdited::default(),
            modal: Modal::None,
            warning: None,
            processing: false,
        }
    }

    pub fn address(&self, i: usize) -> Option<&Address> {
        if i < self.addresses.list.len() {
            self.addresses.list.get(i)
        } else {
            // i >= self.addresses.list.len()
            self.prev_addresses.list.get(i - self.addresses.list.len())
        }
    }

    pub fn derivation_index(&self, i: usize) -> Option<&ChildNumber> {
        if i < self.addresses.list.len() {
            self.addresses.derivation_indexes.get(i)
        } else {
            // i >= self.addresses.list.len()
            self.prev_addresses
                .derivation_indexes
                .get(i - self.addresses.list.len())
        }
    }
}

impl State for ReceivePanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = view::dashboard(
            &Menu::Receive,
            cache,
            self.warning.as_ref(),
            view::receive::receive(
                &self.addresses.list,
                &self.addresses.bip21s,
                &self.addresses.labels,
                &self.prev_addresses.list,
                &self.prev_addresses.labels,
                self.show_prev_addresses,
                &self.selected,
                self.labels_edited.cache(),
                self.prev_continue_from.is_none(),
                self.processing,
            ),
        );

        match &self.modal {
            Modal::VerifyAddress(m) => modal::Modal::new(content, m.view())
                .on_blur(Some(view::Message::Close))
                .into(),
            Modal::ShowQrCode(m) => modal::Modal::new(content, m.view())
                .on_blur(Some(view::Message::Close))
                .into(),
            Modal::ShowBip21QrCode(m) => modal::Modal::new(content, m.view())
                .on_blur(Some(view::Message::Close))
                .into(),
            Modal::None => content,
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        if let Modal::VerifyAddress(modal) = &self.modal {
            modal.subscription()
        } else {
            Subscription::none()
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    std::iter::once(&mut self.addresses)
                        .chain(std::iter::once(&mut self.prev_addresses))
                        .map(|a| a as &mut dyn LabelsLoader),
                ) {
                    Ok(cmd) => cmd,
                    Err(e) => {
                        self.warning = Some(e);
                        Task::none()
                    }
                }
            }
            Message::ReceiveAddress(res) => {
                match res {
                    Ok((address, derivation_index, bip21)) => {
                        self.warning = None;
                        self.addresses.list.push(address.clone());
                        self.addresses.derivation_indexes.push(derivation_index);
                        if let Some(bip21) = bip21 {
                            self.addresses.bip21s.insert(address, bip21);
                        }
                    }
                    Err(e) => self.warning = Some(e),
                }
                Task::none()
            }
            Message::View(view::Message::Close) => {
                self.modal = Modal::None;
                Task::none()
            }
            Message::View(view::Message::Select(i)) => {
                let (address, index) = (
                    self.address(i).expect("Must be present"),
                    self.derivation_index(i).expect("Must be present"),
                );
                self.modal = Modal::VerifyAddress(VerifyAddressModal::new(
                    self.data_dir.clone(),
                    self.wallet.clone(),
                    cache.network,
                    address.clone(),
                    *index,
                ));
                Task::none()
            }
            Message::View(view::Message::NextReceiveAddress) => {
                let daemon = daemon.clone();
                Task::perform(
                    async move {
                        daemon
                            .get_new_address()
                            .await
                            .map(|res| (res.address, res.derivation_index, res.bip21))
                            .map_err(|e| e.into())
                    },
                    Message::ReceiveAddress,
                )
            }
            Message::View(view::Message::ToggleShowPreviousAddresses) => {
                self.show_prev_addresses = !self.show_prev_addresses;
                Task::none()
            }
            Message::View(view::Message::SelectAddress(addr)) => {
                if self.selected.contains(&addr) {
                    self.selected.remove(&addr);
                } else {
                    self.selected.insert(addr);
                }
                Task::none()
            }
            Message::RevealedAddresses(res, start_index) => {
                self.processing = false;
                match res {
                    Ok(revealed) => {
                        self.warning = None;
                        // Make sure these results are for the expected continuation.
                        // The start index can only be None for the first request when there are no prev addresses saved.
                        if self.prev_continue_from == start_index
                            && (start_index.is_some() || self.prev_addresses.is_empty())
                        {
                            for entry in revealed.addresses.iter() {
                                // A new wallet always has index 0 "revealed", but we ignore it as
                                // it was not generated by the user.
                                if entry.index == 0.into() {
                                    continue;
                                }
                                self.prev_addresses.list.push(entry.address.clone());
                                self.prev_addresses.derivation_indexes.push(entry.index);
                                if let Some(label) = &entry.label {
                                    self.prev_addresses.labels.insert(
                                        LabelItem::from(entry.address.clone()).to_string(),
                                        label.clone(),
                                    );
                                }
                            }
                            self.prev_continue_from = revealed.continue_from;
                        }
                    }
                    Err(e) => {
                        self.warning = Some(e);
                    }
                };
                Task::none()
            }
            Message::View(view::Message::Next) => {
                if self.prev_continue_from.is_some() {
                    self.processing = true;
                    let start_index = self.prev_continue_from;
                    Task::perform(
                        async move {
                            (
                                daemon
                                    .list_revealed_addresses(
                                        false,
                                        true,
                                        PREV_ADDRESSES_PAGE_SIZE,
                                        start_index,
                                    )
                                    .await
                                    .map_err(|e| e.into()),
                                start_index,
                            )
                        },
                        |(res, start_index)| Message::RevealedAddresses(res, start_index),
                    )
                } else {
                    Task::none()
                }
            }
            Message::View(view::Message::ShowQrCode(i)) => {
                if let (Some(address), Some(index)) = (self.address(i), self.derivation_index(i)) {
                    if let Some(modal) = ShowQrCodeModal::new(address, *index) {
                        self.modal = Modal::ShowQrCode(modal);
                    }
                }
                Task::none()
            }
            Message::View(view::Message::ShowBip21QrCode(i)) => {
                if let (Some(bip21), Some(index)) = (
                    &self
                        .addresses
                        .bip21s
                        .get(self.address(i).expect("Address should be in bip21")),
                    self.derivation_index(i),
                ) {
                    if let Some(modal) = ShowBip21QrCodeModal::new(bip21, *index) {
                        self.modal = Modal::ShowBip21QrCode(modal);
                    }
                }
                Task::none()
            }
            Message::View(view::Message::PayjoinInitiate) => {
                let daemon = daemon.clone();
                Task::perform(
                    async move {
                        daemon
                            .receive_payjoin()
                            .await
                            .map(|res| (res.address, res.derivation_index, res.bip21))
                            .map_err(|e| e.into())
                    },
                    Message::ReceiveAddress,
                )
            }
            _ => {
                if let Modal::VerifyAddress(ref mut m) = self.modal {
                    m.update(daemon, cache, message)
                } else {
                    Task::none()
                }
            }
        }
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
        let data_dir = self.data_dir.clone();
        *self = Self::new(data_dir, wallet);
        Task::perform(
            async move {
                daemon
                    .list_revealed_addresses(false, true, PREV_ADDRESSES_PAGE_SIZE, None)
                    .await
                    .map_err(|e| e.into())
            },
            |res| Message::RevealedAddresses(res, None),
        )
    }
}

impl From<ReceivePanel> for Box<dyn State> {
    fn from(s: ReceivePanel) -> Box<dyn State> {
        Box::new(s)
    }
}

pub struct VerifyAddressModal {
    warning: Option<Error>,
    chosen_hws: HashSet<Fingerprint>,
    hws: HardwareWallets,
    address: Address,
    derivation_index: ChildNumber,
}

impl VerifyAddressModal {
    pub fn new(
        data_dir: LianaDirectory,
        wallet: Arc<Wallet>,
        network: Network,
        address: Address,
        derivation_index: ChildNumber,
    ) -> Self {
        Self {
            warning: None,
            chosen_hws: HashSet::new(),
            hws: HardwareWallets::new(data_dir, network).with_wallet(wallet),
            address,
            derivation_index,
        }
    }
}

impl VerifyAddressModal {
    fn view(&self) -> Element<view::Message> {
        view::receive::verify_address_modal(
            self.warning.as_ref(),
            &self.hws.list,
            &self.chosen_hws,
            &self.address,
            &self.derivation_index,
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        self.hws.refresh().map(Message::HardwareWallets)
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::HardwareWallets(msg) => match self.hws.update(msg) {
                Ok(cmd) => cmd.map(Message::HardwareWallets),
                Err(e) => {
                    self.warning = Some(e.into());
                    Task::none()
                }
            },
            Message::Verified(fg, res) => {
                self.chosen_hws.remove(&fg);
                if let Err(e) = res {
                    self.warning = Some(e);
                }
                Task::none()
            }
            Message::View(view::Message::SelectHardwareWallet(i)) => {
                if let Some(HardwareWallet::Supported {
                    device,
                    fingerprint,
                    ..
                }) = self.hws.list.get(i)
                {
                    self.warning = None;
                    self.chosen_hws.insert(*fingerprint);
                    let fg = *fingerprint;
                    Task::perform(
                        verify_address(device.clone(), self.derivation_index),
                        move |res| Message::Verified(fg, res),
                    )
                } else {
                    Task::none()
                }
            }
            _ => Task::none(),
        }
    }
}

pub struct ShowQrCodeModal {
    qr_code: qr_code::Data,
    address: String,
}

impl ShowQrCodeModal {
    pub fn new(address: &Address, index: ChildNumber) -> Option<Self> {
        qr_code::Data::new(format!("bitcoin:{}?index={}", address, index))
            .ok()
            .map(|qr_code| Self {
                qr_code,
                address: address.to_string(),
            })
    }

    fn view(&self) -> Element<view::Message> {
        view::receive::qr_modal(&self.qr_code, &self.address)
    }
}

pub struct ShowBip21QrCodeModal {
    qr_code: qr_code::Data,
    bip21: String,
}

impl ShowBip21QrCodeModal {
    pub fn new(bip21: &payjoin::Url, _index: ChildNumber) -> Option<Self> {
        qr_code::Data::new(format!("{}", bip21))
            .ok()
            .map(|qr_code| Self {
                qr_code,
                bip21: bip21.to_string(),
            })
    }

    fn view(&self) -> Element<view::Message> {
        view::receive::qr_modal(&self.qr_code, &self.bip21)
    }
}

async fn verify_address(
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    index: ChildNumber,
) -> Result<(), Error> {
    hw.display_address(&async_hwi::AddressScript::Miniscript {
        change: false,
        index: index.into(),
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{cache::Cache, view::Message as viewMessage, Message},
        daemon::{
            client::{Lianad, Request},
            model::*,
        },
        utils::{mock::Daemon, sandbox::Sandbox},
    };

    use liana::{descriptors::LianaDescriptor, miniscript::bitcoin::Address};
    use serde_json::json;
    use std::{path::PathBuf, str::FromStr};

    const DESC: &str = "wsh(or_d(multi(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[de6eb005/48'/1'/0'/2']tpubDFGuYfS2JwiUSEXiQuNGdT3R7WTDhbaE6jbUhgYSSdhmfQcSx7ZntMPPv7nrkvAqjpj3jX9wbhSGMeKVao4qAzhbNyBi7iQmv5xxQk6H6jz/<0;1>/*),and_v(v:pkh([ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*),older(3))))#p9ax3xxp";

    #[tokio::test]
    async fn test_receive_panel() {
        let addr =
            Address::from_str("tb1qkldgvljmjpxrjq2ev5qxe8dvhn0dph9q85pwtfkjeanmwdue2akqj4twxj")
                .unwrap()
                .assume_checked();
        let daemon = Daemon::new(vec![
            (
                Some(
                    json!({"method": "listrevealedaddresses", "params": [false, true, 20, Option::<ChildNumber>::None]}),
                ),
                Ok(json!(ListRevealedAddressesResult {
                    addresses: vec![],
                    continue_from: None,
                })),
            ),
            (
                Some(json!({"method": "getnewaddress", "params": Option::<Request>::None})),
                Ok(json!(GetAddressResult::new(
                    addr.clone(),
                    ChildNumber::from_normal_idx(0).unwrap(),
                    None,
                ))),
            ),
        ]);
        let wallet = Arc::new(Wallet::new(LianaDescriptor::from_str(DESC).unwrap()));
        let sandbox: Sandbox<ReceivePanel> = Sandbox::new(ReceivePanel::new(
            LianaDirectory::new(PathBuf::new()),
            wallet.clone(),
        ));
        let client = Arc::new(Lianad::new(daemon.run()));
        let cache = Cache::default();
        let sandbox = sandbox.load(client.clone(), &cache, wallet).await;
        let sandbox = sandbox
            .update(
                client,
                &cache,
                Message::View(viewMessage::NextReceiveAddress),
            )
            .await;

        let panel = sandbox.state();
        assert_eq!(panel.addresses.list, vec![addr]);
    }
}
