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
    bip21: HashMap<ChildNumber, String>,
}

impl Addresses {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.derivation_indexes.is_empty() && self.labels.is_empty()
    }

    pub fn push(&mut self, address: Address, derivation_index: ChildNumber, bip21: Option<String>) {
        self.list.push(address);
        self.derivation_indexes.push(derivation_index);
        if let Some(b) = bip21 {
            self.bip21.insert(derivation_index, b);
        }
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
    active_payjoin_sessions: HashSet<ChildNumber>,
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
            active_payjoin_sessions: HashSet::new(),
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
        // A payjoin receiver must contribute at least one confirmed, mature coin as
        // an input. The shared cache already holds all coins, so derive availability
        // from it (refreshed every few seconds by the daemon cache update).
        #[cfg(feature = "payjoin")]
        let payjoin_enabled = cache
            .coins()
            .iter()
            .any(|c| c.block_height.is_some() && !c.is_immature);
        #[cfg(not(feature = "payjoin"))]
        let payjoin_enabled = false;

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
                payjoin_enabled,
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
            Modal::NewAddress(m) => {
                // No blur-to-close while the label is saving and the address is revealing.
                let on_blur = (!m.is_processing())
                    .then_some(view::Message::NewAddress(view::NewAddressMessage::Close));
                modal::Modal::new(content, m.view()).on_blur(on_blur).into()
            }
            Modal::None => content,
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        match &self.modal {
            Modal::VerifyAddress(modal) => modal.subscription(),
            Modal::NewAddress(m) => m
                .verify_sub()
                .map(VerifyAddressModal::subscription)
                .unwrap_or_else(Subscription::none),
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
                // Reveal-and-label completed: record the revealed address and switch the
                // modal to its show step.
                match res {
                    Ok((address, index)) => {
                        self.warning = None;
                        if let Modal::NewAddress(m) = &mut self.modal {
                            if let Some(label) = m.revealed(address.clone(), index, None) {
                                let key = LabelItem::Address(address.clone()).to_string();
                                self.prev_addresses.list.insert(0, address);
                                self.prev_addresses.derivation_indexes.insert(0, index);
                                self.prev_addresses.labels.insert(key, label);
                            }
                        }
                    }
                    Err(e) => {
                        self.warning = Some(e);
                        self.modal = Modal::None;
                    }
                }
                Task::none()
            }
            Message::ReceivePayjoin(res) => {
                match res {
                    Ok((address, derivation_index, bip21)) => {
                        self.warning = None;
                        // The bip21 is kept on the modal for display/QR but is not stored
                        // in prev_addresses: the previously-generated list treats payjoin
                        // addresses like any other address.
                        if let Modal::NewAddress(m) = &mut self.modal {
                            if let Some(label) =
                                m.revealed(address.clone(), derivation_index, bip21)
                            {
                                let key = LabelItem::Address(address.clone()).to_string();
                                self.prev_addresses.list.insert(0, address);
                                self.prev_addresses
                                    .derivation_indexes
                                    .insert(0, derivation_index);
                                self.prev_addresses.labels.insert(key, label);
                            }
                        }
                    }
                    Err(e) => {
                        self.warning = Some(e);
                        self.modal = Modal::None;
                    }
                }
                Task::none()
            }
            Message::View(view::Message::NewAddress(msg)) => match msg {
                view::NewAddressMessage::LabelEdited(s) => {
                    if let Modal::NewAddress(m) = &mut self.modal {
                        m.edit_label(s);
                    }
                    Task::none()
                }
                view::NewAddressMessage::Confirm => {
                    // Reveal first, then store the label on the address that was actually
                    // revealed. Labelling the revealed address (not a guessed one) makes a
                    // mislabel impossible; the worst case is a missing label.
                    let label = if let Modal::NewAddress(m) = &mut self.modal {
                        m.start_reveal()
                    } else {
                        None
                    };
                    let Some(label) = label else {
                        return Task::none();
                    };
                    let is_payjoin = matches!(&self.modal, Modal::NewAddress(m) if m.is_payjoin());
                    let daemon = daemon.clone();
                    if is_payjoin {
                        #[cfg(feature = "payjoin")]
                        {
                            Task::perform(
                                async move {
                                    let res = daemon.receive_payjoin().await?;
                                    let updates = HashMap::from([(
                                        LabelItem::Address(res.address.clone()),
                                        Some(label),
                                    )]);
                                    if let Err(e) = daemon.update_labels(&updates).await {
                                        tracing::warn!(
                                            "failed to store label for {}: {e}",
                                            res.address
                                        );
                                    }
                                    Ok((res.address, res.derivation_index, res.bip21))
                                },
                                Message::ReceivePayjoin,
                            )
                        }
                        #[cfg(not(feature = "payjoin"))]
                        {
                            Task::none()
                        }
                    } else {
                        Task::perform(
                            async move {
                                let res = daemon.get_new_address().await?;
                                let updates = HashMap::from([(
                                    LabelItem::Address(res.address.clone()),
                                    Some(label),
                                )]);
                                if let Err(e) = daemon.update_labels(&updates).await {
                                    // FIXME: should we add a retry or error mechanism here?
                                    tracing::warn!(
                                        "failed to store label for {}: {e}",
                                        res.address
                                    );
                                }
                                Ok((res.address, res.derivation_index))
                            },
                            Message::ReceiveAddress,
                        )
                    }
                }
                view::NewAddressMessage::Verify => {
                    let verify = if let Modal::NewAddress(m) = &self.modal {
                        m.shown().map(|(address, index, _)| {
                            VerifyAddressModal::new(
                                self.data_dir.clone(),
                                self.wallet.clone(),
                                cache.network,
                                address.clone(),
                                index,
                            )
                        })
                    } else {
                        None
                    };
                    if let (Modal::NewAddress(m), Some(verify)) = (&mut self.modal, verify) {
                        m.set_sub(NewAddressSubModal::Verify(verify));
                    }
                    Task::none()
                }
                view::NewAddressMessage::ShowQr => {
                    let qr = if let Modal::NewAddress(m) = &self.modal {
                        m.shown().and_then(|(address, _, bip21)| {
                            if let Some(bip21) = bip21 {
                                ShowQrCodeModal::from_bip21(bip21)
                            } else {
                                ShowQrCodeModal::new(address, None)
                            }
                        })
                    } else {
                        None
                    };
                    if let (Modal::NewAddress(m), Some(qr)) = (&mut self.modal, qr) {
                        m.set_sub(NewAddressSubModal::Qr(qr));
                    }
                    Task::none()
                }
                view::NewAddressMessage::Close => {
                    self.modal = Modal::None;
                    Task::none()
                }
            },
            Message::View(view::Message::Close) => {
                // Closing a stacked sub-modal returns to the show-address modal.
                if let Modal::NewAddress(m) = &mut self.modal {
                    if m.close_sub() {
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
                self.modal = Modal::NewAddress(NewAddressModal::new());
                Task::none()
            }
            #[cfg(feature = "payjoin")]
            Message::View(view::Message::ReceivePayjoin) => {
                self.modal = Modal::NewAddress(NewAddressModal::new_payjoin());
                Task::none()
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
                                self.prev_addresses
                                    .push(entry.address.clone(), entry.index, None);
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
            Message::ActivePayjoinSessions(res) => {
                match res {
                    Ok(indexes) => {
                        self.active_payjoin_sessions = indexes
                            .into_iter()
                            .map(|i| ChildNumber::from_normal_idx(i).unwrap())
                            .collect();
                    }
                    Err(e) => {
                        log::error!("Failed to load active payjoin sessions: {:?}", e);
                    }
                }
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
            Message::View(view::Message::ShowAddressQrCode(view::AddressQrSource::Row(i))) => {
                if let Some(address) = self.address(i) {
                    if let Some(index) = self.derivation_index(i) {
                        if let Some(bip21) = self.prev_addresses.bip21.get(index) {
                            if let Some(modal) = ShowQrCodeModal::from_bip21(bip21) {
                                self.modal = Modal::ShowQrCode(modal);
                            }
                        } else if let Some(modal) = ShowQrCodeModal::new(address, None) {
                            self.modal = Modal::ShowQrCode(modal);
                        }
                    }
                }
                Task::none()
            }
            Message::View(view::Message::ShowAddressQrCode(view::AddressQrSource::WithIndex(
                address,
                i,
            ))) => {
                // Specter DIY devices need the derivation index in the QR code.
                if let Some(qr) = ShowQrCodeModal::new(&address, Some(i)) {
                    // From the generate flow's verify sub-modal, stack the QR so closing it
                    // returns to the show-address step. From a standalone verify modal, replace.
                    if let Modal::NewAddress(m) = &mut self.modal {
                        m.set_sub(NewAddressSubModal::Qr(qr));
                    } else {
                        self.modal = Modal::ShowQrCode(qr);
                    }
                }
                Task::none()
            }
            Message::View(view::Message::ShowQrOptSection(open)) => {
                // The verify modal can be standalone or stacked on the new-address modal.
                let modal = match &mut self.modal {
                    Modal::VerifyAddress(m) => Some(m),
                    Modal::NewAddress(m) => m.verify_sub_mut(),
                    _ => None,
                };
                if let Some(modal) = modal {
                    modal.qr_section_open = open;
                }
                Task::none()
            }
            _ => match &mut self.modal {
                Modal::VerifyAddress(m) => m.update(daemon, cache, message),
                Modal::NewAddress(m) => m
                    .verify_sub_mut()
                    .map(|v| v.update(daemon, cache, message))
                    .unwrap_or_else(Task::none),
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
        let daemon1 = daemon.clone();
        let tasks = vec![Task::perform(
            async move {
                daemon1
                    .list_revealed_addresses(false, true, PREV_ADDRESSES_PAGE_SIZE, None)
                    .await
                    .map_err(|e| e.into())
            },
            |res| Message::RevealedAddresses(res, None),
        )];
        #[cfg(feature = "payjoin")]
        let tasks = {
            let mut tasks = tasks;
            let daemon2 = daemon.clone();
            tasks.push(Task::perform(
                async move {
                    daemon2
                        .get_active_payjoin_receiver_sessions()
                        .await
                        .map_err(|e| e.into())
                },
                Message::ActivePayjoinSessions,
            ));
            tasks
        };
        Task::batch(tasks)
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
    /// Whether the "Other options" (specter DIY QR code) section is open.
    qr_section_open: bool,
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
            qr_section_open: false,
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
            self.derivation_index,
            self.qr_section_open,
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
    pub fn new(address: &Address, index: Option<ChildNumber>) -> Option<Self> {
        let index = index.map(|i| format!("?index={i}")).unwrap_or_default();
        qr_code::Data::new(format!("bitcoin:{address}{index}"))
            .ok()
            .map(|qr_code| Self {
                qr_code,
                address: address.to_string(),
            })
    }

    pub fn from_bip21(bip21: &str) -> Option<Self> {
        qr_code::Data::new(bip21).ok().map(|qr_code| Self {
            qr_code,
            address: bip21.to_string(),
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

/// Modal for a freshly generated address. The user enters a label, then the address
/// is revealed and its label stored. The address is never shown before it is revealed.
pub struct NewAddressModal {
    step: Step,
    is_payjoin: bool,
}

#[allow(clippy::large_enum_variant)]
enum Step {
    /// Entering the label. No address is revealed or displayed at this step.
    Label { label: form::Value<String> },
    /// Revealing the address and storing its label.
    Processing { label: String },
    /// Address revealed and displayed, with an optional stacked verify/QR sub-modal.
    Show {
        address: Address,
        index: ChildNumber,
        /// Bip21 URI (with payjoin params) for a payjoin address; None for a plain address.
        bip21: Option<String>,
        sub: Option<NewAddressSubModal>,
    },
}

impl NewAddressModal {
    fn new() -> Self {
        Self::with_kind(false)
    }

    #[cfg(feature = "payjoin")]
    fn new_payjoin() -> Self {
        Self::with_kind(true)
    }

    fn with_kind(is_payjoin: bool) -> Self {
        Self {
            step: Step::Label {
                label: form::Value {
                    value: String::new(),
                    warning: None,
                    valid: true,
                },
            },
            is_payjoin,
        }
    }

    fn is_payjoin(&self) -> bool {
        self.is_payjoin
    }

    fn edit_label(&mut self, value: String) {
        if let Step::Label { label, .. } = &mut self.step {
            // Empty is valid (no warning); the Generate button is gated on non-empty
            // by the modal itself.
            label.valid = value.len() <= 100;
            label.value = value;
        }
    }

    /// Move to the reveal step, returning the label to persist. None if not at the
    /// label step or the label is empty.
    fn start_reveal(&mut self) -> Option<String> {
        let Step::Label { label } = &self.step else {
            return None;
        };
        if label.value.is_empty() {
            return None;
        }
        let label = label.value.clone();
        self.step = Step::Processing {
            label: label.clone(),
        };
        Some(label)
    }

    /// Complete the reveal: switch to the show step, returning the stored label. None
    /// if not awaiting a reveal.
    fn revealed(
        &mut self,
        address: Address,
        index: ChildNumber,
        bip21: Option<String>,
    ) -> Option<String> {
        if let Step::Processing { label } = &mut self.step {
            let label = std::mem::take(label);
            self.step = Step::Show {
                address,
                index,
                bip21,
                sub: None,
            };
            Some(label)
        } else {
            None
        }
    }

    /// The revealed address, its index, and its bip21 (if any), if at the show step.
    fn shown(&self) -> Option<(&Address, ChildNumber, &Option<String>)> {
        if let Step::Show {
            address,
            index,
            bip21,
            ..
        } = &self.step
        {
            Some((address, *index, bip21))
        } else {
            None
        }
    }

    fn set_sub(&mut self, new_sub: NewAddressSubModal) {
        if let Step::Show { sub, .. } = &mut self.step {
            *sub = Some(new_sub);
        }
    }

    /// Pop a stacked sub-modal, returning to the show step. True if one was open.
    fn close_sub(&mut self) -> bool {
        if let Step::Show { sub, .. } = &mut self.step {
            if sub.is_some() {
                *sub = None;
                return true;
            }
        }
        false
    }

    fn verify_sub(&self) -> Option<&VerifyAddressModal> {
        if let Step::Show {
            sub: Some(NewAddressSubModal::Verify(m)),
            ..
        } = &self.step
        {
            Some(m)
        } else {
            None
        }
    }

    fn verify_sub_mut(&mut self) -> Option<&mut VerifyAddressModal> {
        if let Step::Show {
            sub: Some(NewAddressSubModal::Verify(m)),
            ..
        } = &mut self.step
        {
            Some(m)
        } else {
            None
        }
    }

    fn is_processing(&self) -> bool {
        matches!(self.step, Step::Processing { .. })
    }

    fn view(&self) -> Element<'_, view::Message> {
        match &self.step {
            Step::Label { label, .. } => view::receive::new_address_label_modal(label),
            Step::Processing { .. } => view::receive::new_address_processing_modal(),
            Step::Show {
                address,
                bip21,
                sub,
                ..
            } => {
                let base = view::receive::new_address_show_modal(address, bip21.as_deref());
                // A nested sub-modal stacks on top of the show-address modal: it becomes
                // an overlay over the (full-screen) base, so the base stays rendered behind
                // and reappears once the sub-modal is closed.
                if let Some(sub) = sub {
                    modal::Modal::new(Container::new(base).center(Length::Fill), sub.view())
                        .on_blur(Some(view::Message::Close))
                        .into()
                } else {
                    base
                }
            }
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

    use liana::{descriptors::LianaDescriptor, miniscript::bitcoin::secp256k1};
    use serde_json::json;
    use std::{path::PathBuf, str::FromStr};

    const DESC: &str = "wsh(or_d(multi(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[de6eb005/48'/1'/0'/2']tpubDFGuYfS2JwiUSEXiQuNGdT3R7WTDhbaE6jbUhgYSSdhmfQcSx7ZntMPPv7nrkvAqjpj3jX9wbhSGMeKVao4qAzhbNyBi7iQmv5xxQk6H6jz/<0;1>/*),and_v(v:pkh([ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*),older(3))))#p9ax3xxp";

    #[tokio::test]
    async fn test_receive_panel() {
        let wallet = Arc::new(Wallet::new(LianaDescriptor::from_str(DESC).unwrap()));
        // The reveal returns the next receive address; derive the same one here to build
        // the getnewaddress mock response.
        let secp = secp256k1::Secp256k1::verification_only();
        let index = ChildNumber::from_normal_idx(1).unwrap();
        let addr = wallet
            .main_descriptor
            .receive_descriptor()
            .derive(index, &secp)
            .address(Network::Bitcoin);
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
            #[cfg(feature = "payjoin")]
            (
                Some(
                    json!({"method": "getactivepayjoinreceiversessions", "params": Option::<Request>::None}),
                ),
                Ok(json!(Vec::<u32>::new())),
            ),
            (
                // getnewaddress: reveal first.
                Some(json!({"method": "getnewaddress", "params": Option::<Request>::None})),
                Ok(json!(GetAddressResult::new(addr.clone(), index, None))),
            ),
            // updatelabels: store the label on the revealed address.
            (None, Ok(json!(null))),
        ]);
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

        // Generating opens the modal at the mandatory-label step, without revealing or
        // displaying any address.
        assert!(matches!(
            &sandbox.state().modal,
            Modal::NewAddress(m) if matches!(&m.step, Step::Label { .. })
        ));

        // Enter a label, then confirm: the address is revealed, then its label stored.
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
                client,
                &cache,
                Message::View(viewMessage::NewAddress(view::NewAddressMessage::Confirm)),
            )
            .await;

        // After the reveal, the address is recorded with its label and shown.
        let panel = sandbox.state();
        assert_eq!(panel.prev_addresses.list, vec![addr.clone()]);
        assert_eq!(
            panel.prev_addresses.labels.get(&addr.to_string()),
            Some(&"test".to_string())
        );
        assert!(matches!(
            &panel.modal,
            Modal::NewAddress(m) if matches!(m.step, Step::Show { .. })
        ));
    }
}
