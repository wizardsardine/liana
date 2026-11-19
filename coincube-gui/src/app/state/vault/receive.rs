use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{
    bip32::{ChildNumber, Fingerprint},
    Address, Amount, Network,
};
use coincube_ui::{
    component::amount::{BitcoinDisplayUnit, DisplayAmount},
    widget::{modal, Element},
};
use iced::{
    advanced::widget::Id,
    clipboard,
    widget::{operation, qr_code},
    Subscription, Task,
};

use crate::daemon::model::LabelsLoader;
use crate::dir::CoincubeDirectory;
use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        message::Message,
        settings,
        state::{coins_summary, vault::label::LabelsEdited, State},
        view,
        wallet::{sync_status, SyncStatus, Wallet},
    },
    hw::{HardwareWallet, HardwareWallets},
};

use crate::daemon::{
    model::{LabelItem, Labelled},
    Daemon, DaemonBackend,
};

const PREV_ADDRESSES_PAGE_SIZE: usize = 20;

fn vault_display_name(wallet: &Wallet) -> &str {
    wallet
        .alias
        .as_deref()
        .filter(|alias| !alias.trim().is_empty())
        .unwrap_or_else(|| coincube_ui::locale::copy("Unnamed Vault"))
}

fn zero_balance_known(status: &SyncStatus, rescan_pending: bool) -> bool {
    status.is_synced() && !rescan_pending
}

fn zero_balance_eligible(
    balance: Amount,
    masked: bool,
    backend: Option<&DaemonBackend>,
    cache: &Cache,
) -> bool {
    balance == Amount::ZERO
        && !masked
        && backend.is_some_and(|backend| {
            let status = sync_status(
                backend.clone(),
                cache.blockheight(),
                cache.sync_progress(),
                cache.last_poll_timestamp(),
                cache.last_poll_at_startup,
                cache.history_sync(),
            );
            zero_balance_known(&status, cache.rescan_progress().is_some())
        })
}

/// One bounded settings read. A missing, changing, or unreadable record leaves
/// an empty Vault balance unknown; the normal settings reader retries with
/// sleeps and must not run from Iced's render path.
/// A new receive address. An imported wallet allocates under its source's
/// reservation lock, shared with a paired Bitcoin Blake2b import, so the two
/// Cubes never hand out the same address.
pub(crate) async fn next_receive_address(
    data_dir: CoincubeDirectory,
    wallet: &Wallet,
    daemon: Arc<dyn Daemon + Sync + Send>,
) -> Result<(Address, ChildNumber), Error> {
    vault_receive_address(data_dir, wallet, daemon)
        .await
        .map(|res| (res.address, res.derivation_index))
}

/// A new Vault receive address for any screen that hands one out (Receive,
/// transfers into the Vault, refunds, Buy/Sell). See [`next_receive_address`].
pub(crate) async fn vault_receive_address(
    data_dir: CoincubeDirectory,
    wallet: &Wallet,
    daemon: Arc<dyn Daemon + Sync + Send>,
) -> Result<crate::daemon::model::GetAddressResult, Error> {
    if wallet.is_imported() {
        return crate::app::state::imported_bitcoin::reserve_receive_address(
            data_dir,
            wallet.main_descriptor.clone(),
            daemon,
        )
        .await
        .map_err(Error::Unexpected);
    }
    daemon.get_new_address().await.map_err(|e| e.into())
}

fn no_pending_restore_rescan(data_dir: &CoincubeDirectory, wallet: &Wallet) -> bool {
    const MAX_SETTINGS_BYTES: u64 = 8 * 1024 * 1024;
    let path = data_dir
        .network_directory(wallet.chain)
        .path()
        .join(settings::SETTINGS_FILE_NAME);
    let Some(settings) = (|| {
        let file = std::fs::File::open(path).ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(MAX_SETTINGS_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_SETTINGS_BYTES {
            return None;
        }
        serde_json::from_slice::<settings::Settings>(&bytes).ok()
    })() else {
        return false;
    };
    settings
        .wallets
        .iter()
        .find(|record| record.descriptor_checksum == wallet.descriptor_checksum)
        .is_some_and(|record| record.pending_rescan.is_none())
}

fn balance_label(
    balance: Amount,
    unit: BitcoinDisplayUnit,
    masked: bool,
    zero_known: bool,
) -> String {
    if masked {
        "••••••".to_string()
    } else if balance.to_sat() > 0 || zero_known {
        format!(
            "{} {}",
            balance.to_formatted_string_with_unit(unit),
            unit.label()
        )
    } else {
        coincube_ui::locale::copy("Check balance on Vault Overview").to_string()
    }
}

pub enum Modal {
    VerifyAddress(VerifyAddressModal),
    ShowQrCode(ShowQrCodeModal),
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

pub struct VaultReceivePanel {
    data_dir: CoincubeDirectory,
    wallet: Arc<Wallet>,
    daemon_backend: Option<DaemonBackend>,
    addresses: Addresses,
    current_qr: Option<qr_code::Data>,
    prev_addresses: Addresses,
    prev_continue_from: Option<ChildNumber>,
    show_prev_addresses: bool,
    selected: HashSet<Address>,
    labels_edited: LabelsEdited,
    modal: Modal,
    modal_origin: Option<Id>,
    warning: Option<Error>,
    processing: bool,
    generating: bool,
    /// Refreshed on panel updates and reload, never by `view`.
    restore_rescan_clear: bool,
}

impl VaultReceivePanel {
    pub fn new(data_dir: CoincubeDirectory, wallet: Arc<Wallet>) -> Self {
        Self {
            data_dir,
            wallet,
            daemon_backend: None,
            addresses: Addresses::default(),
            current_qr: None,
            prev_addresses: Addresses::default(),
            prev_continue_from: None,
            show_prev_addresses: false,
            selected: HashSet::new(),
            labels_edited: LabelsEdited::default(),
            modal: Modal::None,
            modal_origin: None,
            warning: None,
            // Starts true: the panel always fires a `reload` fetch the
            // moment it's shown, so the first render should display the
            // loading state rather than briefly flashing "No addresses yet".
            // Mirrors `VaultTransactionsPanel::new`.
            processing: true,
            generating: false,
            restore_rescan_clear: false,
        }
    }

    fn refresh_restore_rescan(&mut self, cache: &Cache) {
        self.refresh_restore_rescan_with(cache, no_pending_restore_rescan);
    }

    fn refresh_restore_rescan_with(
        &mut self,
        cache: &Cache,
        read: impl FnOnce(&CoincubeDirectory, &Wallet) -> bool,
    ) {
        let (confirmed, unconfirmed, _, _) = coins_summary(
            cache.coins(),
            cache.blockheight().max(0) as u32,
            self.wallet.recovery_timelock(),
        );
        self.restore_rescan_clear = zero_balance_eligible(
            confirmed + unconfirmed,
            cache.balance_amounts_masked(),
            self.daemon_backend.as_ref(),
            cache,
        ) && read(&self.data_dir, &self.wallet);
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

impl State for VaultReceivePanel {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        // Reuse Overview's coin accounting and sync classifier. A zero is
        // authoritative only after sync and any restore rescan are complete.
        let (confirmed, unconfirmed, _, _) = coins_summary(
            cache.coins(),
            cache.blockheight().max(0) as u32,
            self.wallet.recovery_timelock(),
        );
        let balance = confirmed + unconfirmed;
        let masked = cache.balance_amounts_masked();
        let zero_known =
            zero_balance_eligible(balance, masked, self.daemon_backend.as_ref(), cache)
                && self.restore_rescan_clear;
        let balance_display = balance_label(balance, cache.bitcoin_unit, masked, zero_known);
        let content = view::dashboard_headed(
            menu,
            cache,
            view::vault::sub_page_head(cache, "Receive"),
            view::vault::receive::receive(
                &self.addresses.list,
                self.current_qr.as_ref(),
                vault_display_name(&self.wallet),
                balance_display,
                &self.addresses.labels,
                &self.prev_addresses.list,
                &self.prev_addresses.labels,
                self.show_prev_addresses,
                &self.selected,
                self.labels_edited.cache(),
                self.prev_continue_from.is_none(),
                self.processing,
                self.generating,
            ),
        );

        // Use global toast overlay instead of local toast
        match &self.modal {
            Modal::VerifyAddress(m) => modal::Modal::new(content, m.view())
                .on_blur(Some(view::Message::Close))
                .into(),
            Modal::ShowQrCode(m) => modal::Modal::new(content, m.view())
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
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let daemon = daemon.expect("Daemon required for vault receive panel");
        // App forwards each Tick to the current panel. Keep disk I/O off
        // redraws and interactive actions. A new restore obligation written
        // while Receive is open revokes a known zero on the next tick.
        if matches!(message, Message::Tick | Message::UpdatePanelCache(_)) {
            self.refresh_restore_rescan(cache);
        }
        match message {
            Message::View(view::Message::VaultReceive(msg)) => match msg {
                view::VaultReceiveMessage::Copy(address) => {
                    // Use global toast overlay
                    let toast_task = Task::done(Message::View(view::Message::ShowToast(
                        log::Level::Info,
                        "Copied Vault address to clipboard".to_string(),
                    )));

                    Task::batch([clipboard::write(address), toast_task])
                }
            },
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon.clone(),
                    message,
                    std::iter::once(&mut self.addresses)
                        .chain(std::iter::once(&mut self.prev_addresses))
                        .map(|a| a as &mut dyn LabelsLoader),
                ) {
                    Ok(cmd) => cmd,
                    Err(e) => {
                        let err_msg = crate::user_error::report(&e);
                        self.warning = Some(e);
                        Task::done(Message::View(view::Message::ShowError(err_msg)))
                    }
                }
            }
            Message::ReceiveAddress(res) => {
                self.generating = false;
                match res {
                    Ok((address, derivation_index)) => {
                        self.warning = None;
                        self.current_qr = ShowQrCodeModal::qr_data(&address, derivation_index);
                        self.addresses.list.push(address);
                        self.addresses.derivation_indexes.push(derivation_index);
                        Task::none()
                    }
                    Err(e) => {
                        let err_msg = crate::user_error::report(&e);
                        self.warning = Some(e);
                        Task::done(Message::View(view::Message::ShowError(err_msg)))
                    }
                }
            }
            Message::View(view::Message::Close) => {
                self.modal = Modal::None;
                self.modal_origin
                    .take()
                    .map(operation::focus)
                    .unwrap_or_else(Task::none)
            }
            Message::View(view::Message::Select(i)) => {
                let (address, index) = (
                    self.address(i).expect("Must be present"),
                    self.derivation_index(i).expect("Must be present"),
                );
                let origin = view::vault::receive::modal_origin_id(
                    i,
                    self.addresses.list.len(),
                    address,
                    true,
                );
                self.modal = Modal::VerifyAddress(VerifyAddressModal::new(
                    self.data_dir.clone(),
                    self.wallet.clone(),
                    cache.network,
                    address.clone(),
                    *index,
                ));
                self.modal_origin = Some(origin);
                operation::focus(view::vault::receive::verify_close_id())
            }
            Message::View(view::Message::NextReceiveAddress) => {
                // flight so we don't burn through receive addresses.
                if self.generating {
                    return Task::none();
                }
                self.generating = true;
                let daemon = daemon.clone();
                let (data_dir, wallet) = (self.data_dir.clone(), self.wallet.clone());
                Task::perform(
                    async move {
                        let res = next_receive_address(data_dir, &wallet, daemon.clone()).await;
                        // User just generated a fresh receive
                        // address — they're staring at the QR
                        // waiting for an incoming payment, so kick
                        // the poller for a per-SPK rescan on the
                        // next tick instead of waiting up to 10
                        // min for the smart-poll cadence.
                        //
                        // Detached rather than awaited: `request_sync`
                        // is a second blocking RPC, and awaiting it here
                        // would hold `Message::ReceiveAddress` — and so
                        // the button's spinner — for its whole round
                        // trip. `Task::perform` futures run on iced's
                        // tokio executor, so a runtime is in context.
                        let sync_daemon = daemon.clone();
                        tokio::spawn(async move {
                            let _ = sync_daemon.request_sync().await;
                        });
                        res
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
                        let err_msg = crate::user_error::report(&e);
                        self.warning = Some(e);
                        return Task::done(Message::View(view::Message::ShowError(err_msg)));
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
            Message::View(view::Message::ShowQrCode(i)) => {
                if let (Some(address), Some(index)) = (self.address(i), self.derivation_index(i)) {
                    if let Some(modal) = ShowQrCodeModal::new(address, *index) {
                        self.modal_origin = Some(view::vault::receive::modal_origin_id(
                            i,
                            self.addresses.list.len(),
                            address,
                            false,
                        ));
                        self.modal = Modal::ShowQrCode(modal);
                        return operation::focus(view::vault::receive::qr_close_id());
                    }
                }
                Task::none()
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
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        let daemon = daemon.expect("Vault panels require daemon");
        let wallet = wallet.expect("Vault panels require wallet");
        let data_dir = self.data_dir.clone();
        // `Self::new` already starts in the loading state, so the fetch
        // below is covered from the first frame after navigation.
        *self = Self::new(data_dir, wallet);
        self.daemon_backend = Some(daemon.backend());
        Task::perform(
            async move {
                let res = daemon
                    .list_revealed_addresses(false, true, PREV_ADDRESSES_PAGE_SIZE, None)
                    .await
                    .map_err(|e| e.into());
                // The Receive panel just opened — user is here to
                // either generate a new address or check on an
                // existing one. Kick the poller for fresh mempool
                // state so an incoming unconfirmed tx shows up
                // promptly instead of waiting for the smart-poll
                // safety-net rescan.
                //
                // Detached rather than awaited: `request_sync` is a
                // second blocking RPC, and awaiting it here would hold
                // `Message::RevealedAddresses` — and so the loading
                // placeholder — for its whole round trip even though the
                // addresses are already in hand. `Task::perform` futures
                // run on iced's tokio executor, so a runtime is in
                // context.
                let sync_daemon = daemon.clone();
                tokio::spawn(async move {
                    let _ = sync_daemon.request_sync().await;
                });
                res
            },
            |res| Message::RevealedAddresses(res, None),
        )
    }
}

impl From<VaultReceivePanel> for Box<dyn State> {
    fn from(s: VaultReceivePanel) -> Box<dyn State> {
        Box::new(s)
    }
}

pub struct VerifyAddressModal {
    warning: Option<Error>,
    chosen_hws: HashSet<Fingerprint>,
    hws: HardwareWallets,
    chain: crate::chain::ChainId,
    address: Address,
    derivation_index: ChildNumber,
}

impl VerifyAddressModal {
    pub fn new(
        data_dir: CoincubeDirectory,
        wallet: Arc<Wallet>,
        network: Network,
        address: Address,
        derivation_index: ChildNumber,
    ) -> Self {
        Self {
            warning: None,
            chosen_hws: HashSet::new(),
            chain: wallet.chain,
            hws: HardwareWallets::new(data_dir, network).with_wallet(wallet),
            address,
            derivation_index,
        }
    }
}

impl VerifyAddressModal {
    pub fn view(&self) -> Element<'_, view::Message> {
        view::vault::receive::verify_address_modal(
            self.chain,
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
                    let err: Error = e.into();
                    let err_msg = crate::user_error::report(&err);
                    self.warning = Some(err);
                    Task::done(Message::View(view::Message::ShowError(err_msg)))
                }
            },
            Message::Verified(fg, res) => {
                self.chosen_hws.remove(&fg);
                if let Err(e) = res {
                    let err_msg = crate::user_error::report(&e);
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
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

#[derive(Debug)]
pub struct ShowQrCodeModal {
    qr_code: qr_code::Data,
    address: String,
}

impl ShowQrCodeModal {
    fn qr_data(address: &Address, index: ChildNumber) -> Option<qr_code::Data> {
        qr_code::Data::new(format!("bitcoin:{}?index={}", address, index)).ok()
    }

    pub fn new(address: &Address, index: ChildNumber) -> Option<Self> {
        Self::qr_data(address, index).map(|qr_code| Self {
            qr_code,
            address: address.to_string(),
        })
    }

    pub fn view(&self) -> Element<'_, view::Message> {
        view::vault::receive::qr_modal(&self.qr_code, &self.address)
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
    use crate::app::settings::{self, PendingRescan, WalletSettings};
    use crate::app::state::preferences::{BalancePrivacy, BalancePrivacyChrome};
    use crate::{
        app::{cache::Cache, view::Message as viewMessage, Message},
        daemon::{
            client::{Coincubed, Request},
            model::*,
        },
        utils::{mock::Daemon, sandbox::Sandbox},
    };

    use coincube_core::{descriptors::CoincubeDescriptor, miniscript::bitcoin::Address};
    use serde_json::json;
    use std::{path::PathBuf, str::FromStr};

    const DESC: &str = "wsh(or_d(multi(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[de6eb005/48'/1'/0'/2']tpubDFGuYfS2JwiUSEXiQuNGdT3R7WTDhbaE6jbUhgYSSdhmfQcSx7ZntMPPv7nrkvAqjpj3jX9wbhSGMeKVao4qAzhbNyBi7iQmv5xxQk6H6jz/<0;1>/*),and_v(v:pkh([ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*),older(3))))#p9ax3xxp";

    #[test]
    fn receive_aside_masks_a_known_balance_and_avoids_an_unverified_zero() {
        let balance = Amount::from_sat(250_000);
        assert_eq!(
            balance_label(balance, BitcoinDisplayUnit::Sats, false, false),
            "250,000 SATS"
        );
        assert_eq!(
            balance_label(balance, BitcoinDisplayUnit::Sats, true, true),
            "••••••"
        );
        assert_eq!(
            balance_label(Amount::ZERO, BitcoinDisplayUnit::Sats, false, false),
            "Check balance on Vault Overview"
        );
        assert_eq!(
            balance_label(Amount::ZERO, BitcoinDisplayUnit::Sats, false, true),
            "0 SATS"
        );
    }

    #[test]
    fn receive_zero_is_known_only_after_sync_and_restore_rescan() {
        assert!(zero_balance_known(&SyncStatus::Synced, false));
        assert!(!zero_balance_known(&SyncStatus::Synced, true));
        assert!(!zero_balance_known(
            &SyncStatus::WalletFullScan {
                progress: Some(0.5)
            },
            false
        ));
        assert!(!zero_balance_known(&SyncStatus::LatestWalletSync, false));
    }

    #[test]
    fn receive_aside_uses_vault_alias_not_cube_name() {
        let wallet = Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap())
            .with_name("Family Cube".to_string())
            .with_alias(Some("Family Vault".to_string()));
        assert_eq!(vault_display_name(&wallet), "Family Vault");
        assert_ne!(vault_display_name(&wallet), wallet.name);

        let unnamed = wallet.with_alias(Some("  ".to_string()));
        assert_eq!(vault_display_name(&unnamed), "Unnamed Vault");
    }

    #[test]
    fn receive_rechecks_the_restore_marker_before_calling_zero_known() {
        let root = std::env::temp_dir().join(format!("receive-rescan-{}", uuid::Uuid::new_v4()));
        let data_dir = CoincubeDirectory::new(root.clone());
        let wallet = Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap());
        let network_dir = data_dir.network_directory(wallet.chain);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        let mut settings = settings::Settings {
            wallets: vec![WalletSettings {
                name: wallet.name.clone(),
                alias: None,
                descriptor_checksum: wallet.descriptor_checksum.clone(),
                pinned_at: None,
                keys: Vec::new(),
                hardware_wallets: Vec::new(),
                remote_backend_auth: None,
                start_internal_bitcoind: None,
                pending_rescan: Some(PendingRescan::DateUnknown),
                keychain_keys_recorded: false,
            }],
            ..Default::default()
        };
        let path = network_dir.path().join(settings::SETTINGS_FILE_NAME);
        let write = |settings: &settings::Settings| {
            std::fs::write(&path, serde_json::to_vec(settings).unwrap()).unwrap();
        };
        let known = || {
            zero_balance_known(
                &SyncStatus::Synced,
                !no_pending_restore_rescan(&data_dir, &wallet),
            )
        };
        assert!(!known(), "missing settings cannot prove a zero");
        write(&settings);
        assert!(!known(), "pending restore is not a known zero");
        settings.wallets[0].pending_rescan = None;
        write(&settings);
        assert!(known(), "retired marker permits a synced zero");
        let record = settings.wallets.pop().unwrap();
        write(&settings);
        assert!(!known(), "missing wallet record cannot prove a zero");
        settings.wallets.push(record);
        settings.wallets[0].pending_rescan = Some(PendingRescan::DateUnknown);
        write(&settings);
        assert!(!known(), "a new marker must hide the zero immediately");
        std::fs::write(&path, b"{broken").unwrap();
        assert!(!known(), "corrupt settings cannot prove a zero");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(!known(), "unreadable endpoint cannot prove a zero");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn receive_refreshes_restore_status_on_reload_and_tick() {
        let root = std::env::temp_dir().join(format!("receive-update-{}", uuid::Uuid::new_v4()));
        let data_dir = CoincubeDirectory::new(root.clone());
        let wallet = Arc::new(Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap()));
        let network_dir = data_dir.network_directory(wallet.chain);
        std::fs::create_dir_all(network_dir.path()).unwrap();
        let path = network_dir.path().join(settings::SETTINGS_FILE_NAME);
        let mut settings = settings::Settings {
            wallets: vec![WalletSettings {
                name: wallet.name.clone(),
                alias: None,
                descriptor_checksum: wallet.descriptor_checksum.clone(),
                pinned_at: None,
                keys: Vec::new(),
                hardware_wallets: Vec::new(),
                remote_backend_auth: None,
                start_internal_bitcoind: None,
                pending_rescan: Some(PendingRescan::DateUnknown),
                keychain_keys_recorded: false,
            }],
            ..Default::default()
        };
        let write = |settings: &settings::Settings| {
            std::fs::write(&path, serde_json::to_vec(settings).unwrap()).unwrap();
        };
        let daemon = Arc::new(Coincubed::new(Daemon::new(vec![]).run()));
        let mut cache = Cache::default();
        cache.daemon_cache.blockheight = 1;
        cache.balance_privacy = Some(BalancePrivacyChrome {
            privacy: BalancePrivacy::Shown,
            busy: false,
            interactive: false,
        });
        let mut panel = VaultReceivePanel::new(data_dir, wallet.clone());
        assert!(!panel.restore_rescan_clear, "first render must fail closed");

        write(&settings);
        drop(panel.reload(Some(daemon.clone()), Some(wallet)));
        assert!(!panel.restore_rescan_clear, "reload must start unknown");
        panel.daemon_backend = Some(DaemonBackend::ExternalCoincubed);
        drop(panel.update(Some(daemon.clone()), &cache, Message::Tick));
        assert!(!panel.restore_rescan_clear, "tick sees pending restore");

        settings.wallets[0].pending_rescan = None;
        write(&settings);
        drop(panel.update(Some(daemon.clone()), &cache, Message::Tick));
        assert!(
            panel.restore_rescan_clear,
            "retired marker permits known zero"
        );

        cache.balance_privacy = Some(BalancePrivacyChrome {
            privacy: BalancePrivacy::Hidden,
            busy: false,
            interactive: false,
        });
        panel.refresh_restore_rescan_with(&cache, |_, _| panic!("hidden balance read settings"));
        assert!(!panel.restore_rescan_clear, "Hide revokes known zero");
        cache.balance_privacy.as_mut().unwrap().privacy = BalancePrivacy::Shown;
        let txid = coincube_core::miniscript::bitcoin::Txid::from_str(&"00".repeat(32)).unwrap();
        let address =
            Address::from_str("tb1qkldgvljmjpxrjq2ev5qxe8dvhn0dph9q85pwtfkjeanmwdue2akqj4twxj")
                .unwrap()
                .assume_checked();
        cache.daemon_cache.coins.push(Coin {
            outpoint: coincube_core::miniscript::bitcoin::OutPoint::new(txid, 0),
            amount: Amount::from_sat(1),
            address,
            derivation_index: ChildNumber::from_normal_idx(0).unwrap(),
            block_height: None,
            is_immature: false,
            is_change: false,
            is_from_self: false,
            spend_info: None,
        });
        panel.refresh_restore_rescan_with(&cache, |_, _| panic!("nonzero balance read settings"));
        assert!(!panel.restore_rescan_clear, "nonzero needs no marker read");
        cache.daemon_cache.coins.clear();
        cache.daemon_cache.sync_progress = 0.5;
        panel.refresh_restore_rescan_with(&cache, |_, _| panic!("unsynced balance read settings"));
        assert!(!panel.restore_rescan_clear, "unsynced zero stays unknown");
        cache.daemon_cache.sync_progress = 1.0;
        cache.daemon_cache.rescan_progress = Some(0.5);
        panel.refresh_restore_rescan_with(&cache, |_, _| panic!("active rescan read settings"));
        assert!(!panel.restore_rescan_clear, "active rescan stays unknown");
        cache.daemon_cache.rescan_progress = None;
        drop(panel.update(Some(daemon.clone()), &cache, Message::Tick));
        assert!(
            panel.restore_rescan_clear,
            "re-entry must refresh the marker"
        );

        settings.wallets[0].pending_rescan = Some(PendingRescan::DateUnknown);
        write(&settings);
        drop(panel.update(Some(daemon.clone()), &cache, Message::Tick));
        assert!(
            !panel.restore_rescan_clear,
            "new pending marker revokes known zero"
        );

        settings.wallets[0].pending_rescan = None;
        write(&settings);
        drop(panel.update(Some(daemon.clone()), &cache, Message::Tick));
        assert!(panel.restore_rescan_clear);
        std::fs::write(&path, b"{broken").unwrap();
        drop(panel.update(Some(daemon), &cache, Message::Tick));
        assert!(!panel.restore_rescan_clear, "corrupt settings fail closed");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn test_receive_panel() {
        let addr =
            Address::from_str("tb1qkldgvljmjpxrjq2ev5qxe8dvhn0dph9q85pwtfkjeanmwdue2akqj4twxj")
                .unwrap()
                .assume_checked();
        // The mock daemon's `requests` queue is strictly positional (see
        // `utils::mock::Daemon`): each entry consumes the next outgoing
        // RPC. Only the two calls the panel awaits inline go in it, in the
        // order the panel makes them:
        //   1. listrevealedaddresses    (reload's primary)
        //   2. getnewaddress            (NextReceiveAddress primary)
        //
        // Both paths also kick an eager `requestsync`, but each is
        // detached with `tokio::spawn` and never awaited, so it lands
        // whenever the scheduler gets to it — possibly after the next
        // ordered call, possibly after this test ends. Ordering it in the
        // queue would make the test depend on runtime scheduling, so it
        // gets a standing response matched by method name instead and
        // can't shift the queue. That does mean the kicks themselves
        // aren't asserted: a fire-and-forget side effect has no
        // race-free moment to assert on from here. `reload`'s and
        // `NextReceiveAddress`'s call to it is covered by reading the
        // source, not by this test.
        //
        // The daemon answers `requestsync` with the empty JSON object
        // `{}`; the client deserialises into a discarded
        // `serde_json::Value`.
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
        ])
        .with_standing_response("requestsync", json!({}));
        let wallet = Arc::new(Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap()));
        let sandbox: Sandbox<VaultReceivePanel> = Sandbox::new(VaultReceivePanel::new(
            CoincubeDirectory::new(PathBuf::new()),
            wallet.clone(),
        ));
        let client = Arc::new(Coincubed::new(daemon.run()));
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
        assert!(panel.current_qr.is_some());
    }
}
