use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use iced::{widget::qr_code, Length, Subscription, Task};
use liana::miniscript::bitcoin::{
    bip32::{ChildNumber, Fingerprint},
    Address, Network,
};
use liana_ui::{component::form, widget::modal, widget::*};

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

#[allow(clippy::large_enum_variant)]
pub enum Modal {
    VerifyAddress(VerifyAddressModal),
    ShowQrCode(ShowQrCodeModal),
    EditLabel(String),
    NewAddress(NewAddressModal),
    None,
}

#[derive(Debug, Default)]
pub struct Addresses {
    list: Vec<Address>,
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
    prev_addresses: Addresses,
    prev_continue_from: Option<ChildNumber>,
    show_prev_addresses: bool,
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
            prev_addresses: Addresses::default(),
            prev_continue_from: None,
            show_prev_addresses: false,
            labels_edited: LabelsEdited::default(),
            modal: Modal::None,
            warning: None,
            processing: false,
        }
    }

    pub fn address(&self, i: usize) -> Option<&Address> {
        self.prev_addresses.list.get(i)
    }

    pub fn derivation_index(&self, i: usize) -> Option<&ChildNumber> {
        self.prev_addresses.derivation_indexes.get(i)
    }
}

impl State for ReceivePanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = view::dashboard(
            &Menu::Receive,
            cache,
            self.warning.as_ref(),
            view::receive::receive(
                &self.prev_addresses.list,
                &self.prev_addresses.labels,
                self.show_prev_addresses,
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
            Modal::EditLabel(addr) => {
                let value = self
                    .labels_edited
                    .cache()
                    .get(addr)
                    .expect("seeded when EditLabel modal opened");
                modal::Modal::new(content, view::receive::edit_label_modal(addr, value))
                    .on_blur(Some(view::Message::Label(
                        vec![addr.clone()],
                        view::LabelMessage::Cancel,
                    )))
                    .into()
            }
            Modal::NewAddress(m) => modal::Modal::new(content, m.view())
                .on_blur(Some(view::Message::NewAddress(
                    view::NewAddressMessage::Close,
                )))
                .into(),
            Modal::None => content,
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        match &self.modal {
            Modal::VerifyAddress(modal) => modal.subscription(),
            Modal::NewAddress(NewAddressModal {
                sub: Some(NewAddressSubModal::Verify(modal)),
                ..
            }) => modal.subscription(),
            _ => Subscription::none(),
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Label(items, view::LabelMessage::Edit)) => {
                let addr = items.into_iter().next().unwrap_or_default();
                let current = self
                    .prev_addresses
                    .labels
                    .get(&addr)
                    .cloned()
                    .unwrap_or_default();
                self.labels_edited.edit(addr.clone(), current);
                self.modal = Modal::EditLabel(addr);
                Task::none()
            }
            Message::View(view::Message::Label(_, view::LabelMessage::Confirm))
            | Message::View(view::Message::Label(_, view::LabelMessage::Cancel)) => {
                self.modal = Modal::None;
                match self.labels_edited.update(
                    daemon,
                    message,
                    std::iter::once(&mut self.prev_addresses as &mut dyn LabelsLoader),
                ) {
                    Ok(cmd) => cmd,
                    Err(e) => {
                        self.warning = Some(e);
                        Task::none()
                    }
                }
            }
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    std::iter::once(&mut self.prev_addresses as &mut dyn LabelsLoader),
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
                    Ok((address, derivation_index)) => {
                        self.warning = None;
                        self.modal =
                            Modal::NewAddress(NewAddressModal::new(address, derivation_index));
                    }
                    Err(e) => self.warning = Some(e),
                }
                Task::none()
            }
            Message::View(view::Message::NewAddress(msg)) => match msg {
                view::NewAddressMessage::LabelEdited(s) => {
                    if let Modal::NewAddress(m) = &mut self.modal {
                        // Empty is valid (no warning); the Generate button is gated
                        // on non-empty by the modal itself.
                        m.label.valid = s.len() <= 100;
                        m.label.value = s;
                    }
                    Task::none()
                }
                view::NewAddressMessage::Confirm => {
                    if let Modal::NewAddress(m) = &mut self.modal {
                        m.show_address = true;
                    }
                    Task::none()
                }
                view::NewAddressMessage::Verify => {
                    let verify = if let Modal::NewAddress(m) = &self.modal {
                        Some(VerifyAddressModal::new(
                            self.data_dir.clone(),
                            self.wallet.clone(),
                            cache.network,
                            m.address.clone(),
                            m.derivation_index,
                        ))
                    } else {
                        None
                    };
                    if let (Modal::NewAddress(m), Some(verify)) = (&mut self.modal, verify) {
                        m.sub = Some(NewAddressSubModal::Verify(verify));
                    }
                    Task::none()
                }
                view::NewAddressMessage::ShowQr => {
                    let qr = if let Modal::NewAddress(m) = &self.modal {
                        ShowQrCodeModal::new(&m.address, m.derivation_index)
                    } else {
                        None
                    };
                    if let (Modal::NewAddress(m), Some(qr)) = (&mut self.modal, qr) {
                        m.sub = Some(NewAddressSubModal::Qr(qr));
                    }
                    Task::none()
                }
                view::NewAddressMessage::Close => {
                    let finish = if let Modal::NewAddress(m) = &self.modal {
                        m.show_address
                            .then(|| (m.address.clone(), m.derivation_index, m.label.value.clone()))
                    } else {
                        None
                    };
                    self.modal = Modal::None;
                    if let Some((address, index, label)) = finish {
                        let key = LabelItem::Address(address.clone()).to_string();
                        self.prev_addresses.list.insert(0, address.clone());
                        self.prev_addresses.derivation_indexes.insert(0, index);
                        self.prev_addresses.labels.insert(key, label.clone());
                        let updated = HashMap::from([(LabelItem::Address(address), Some(label))]);
                        return Task::perform(
                            async move {
                                daemon
                                    .update_labels(&updated)
                                    .await
                                    .map(|_| HashMap::new())
                                    .map_err(|e| e.into())
                            },
                            Message::LabelsUpdated,
                        );
                    }
                    Task::none()
                }
            },
            Message::View(view::Message::Close) => {
                // Closing a stacked sub-modal returns to the show-address modal.
                if let Modal::NewAddress(m) = &mut self.modal {
                    if m.sub.is_some() {
                        m.sub = None;
                        return Task::none();
                    }
                }
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
                            .map(|res| (res.address, res.derivation_index))
                            .map_err(|e| e.into())
                    },
                    Message::ReceiveAddress,
                )
            }
            Message::View(view::Message::ToggleShowPreviousAddresses) => {
                self.show_prev_addresses = !self.show_prev_addresses;
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
            _ => match &mut self.modal {
                Modal::VerifyAddress(m) => m.update(daemon, cache, message),
                Modal::NewAddress(NewAddressModal {
                    sub: Some(NewAddressSubModal::Verify(m)),
                    ..
                }) => m.update(daemon, cache, message),
                _ => Task::none(),
            },
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
    fn view(&self) -> Element<'_, view::Message> {
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
        qr_code::Data::new(format!("bitcoin:{address}?index={index}"))
            .ok()
            .map(|qr_code| Self {
                qr_code,
                address: address.to_string(),
            })
    }

    fn view(&self) -> Element<'_, view::Message> {
        view::receive::qr_modal(&self.qr_code, &self.address)
    }
}

/// A modal stacked on top of the show-address step (verify on hardware, or QR code).
#[allow(clippy::large_enum_variant)]
enum NewAddressSubModal {
    Verify(VerifyAddressModal),
    Qr(ShowQrCodeModal),
}

impl NewAddressSubModal {
    fn view(&self) -> Element<'_, view::Message> {
        match self {
            NewAddressSubModal::Verify(m) => m.view(),
            NewAddressSubModal::Qr(m) => m.view(),
        }
    }
}

/// Two-step modal for a freshly generated address: enter a mandatory label, then
/// display the address. The address is added to the list when the modal is closed.
pub struct NewAddressModal {
    address: Address,
    derivation_index: ChildNumber,
    label: form::Value<String>,
    show_address: bool,
    /// Verify/QR modal stacked on top of the show-address step, if open.
    sub: Option<NewAddressSubModal>,
}

impl NewAddressModal {
    fn new(address: Address, derivation_index: ChildNumber) -> Self {
        Self {
            address,
            derivation_index,
            label: form::Value {
                value: String::new(),
                warning: None,
                valid: true,
            },
            show_address: false,
            sub: None,
        }
    }

    fn view(&self) -> Element<'_, view::Message> {
        let base = if self.show_address {
            view::receive::new_address_show_modal(&self.address)
        } else {
            view::receive::new_address_label_modal(&self.label)
        };
        // A nested sub-modal stacks on top of the show-address modal: it becomes
        // an overlay over the (full-screen) base, so the base stays rendered behind
        // and reappears once the sub-modal is closed.
        if let Some(sub) = &self.sub {
            modal::Modal::new(Container::new(base).center(Length::Fill), sub.view())
                .on_blur(Some(view::Message::Close))
                .into()
        } else {
            base
        }
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
                    ChildNumber::from_normal_idx(0).unwrap()
                ))),
            ),
            // updatelabels, triggered when the new-address modal is closed.
            (None, Ok(json!(null))),
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
                client.clone(),
                &cache,
                Message::View(viewMessage::NextReceiveAddress),
            )
            .await;

        // Generating opens the new-address modal at the mandatory-label step.
        assert!(matches!(
            &sandbox.state().modal,
            Modal::NewAddress(m) if m.address == addr && !m.show_address
        ));

        // Enter a label, confirm to reach the show-address step, then close.
        let sandbox = sandbox
            .update(
                client.clone(),
                &cache,
                Message::View(viewMessage::NewAddress(
                    view::NewAddressMessage::LabelEdited("test".to_string()),
                )),
            )
            .await;
        let sandbox = sandbox
            .update(
                client.clone(),
                &cache,
                Message::View(viewMessage::NewAddress(view::NewAddressMessage::Confirm)),
            )
            .await;
        let sandbox = sandbox
            .update(
                client,
                &cache,
                Message::View(viewMessage::NewAddress(view::NewAddressMessage::Close)),
            )
            .await;

        let panel = sandbox.state();
        assert_eq!(panel.prev_addresses.list, vec![addr]);
        assert!(matches!(panel.modal, Modal::None));
    }
}
