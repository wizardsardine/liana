use std::{
    collections::{BTreeMap, HashMap, HashSet},
    convert::Infallible,
    fmt, fs, io,
    path::{Path, PathBuf},
    str::FromStr,
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};

use bwk::{
    account::Account,
    bwk_coin::{CoinSpendInfo, KeyChain},
    bwk_descriptor::descriptor::ScriptType,
    bwk_electrum::{
        coin_store::CoinEntry,
        label_store::LabelKey,
        notification::{Notification, TxListenerNotif},
        open, tx_listener,
        tx_store::TxEntry,
        url::{parse_electrum_url, ElectrumScheme},
    },
    config::Config,
    persist::PersistenceKind,
};
use liana::{
    label::Label,
    miniscript::{
        bitcoin::{
            address,
            bip32::{ChildNumber, DerivationPath, Fingerprint, Xpub},
            Network, NetworkKind, OutPoint,
        },
        descriptor::{DerivPaths, DescriptorMultiXKey, Wildcard},
        translate_hash_clone, Descriptor, DescriptorPublicKey, ForEachKey, TranslateErr,
        TranslatePk, Translator,
    },
};
use lianad::commands::{GraphItem, GraphLayoutEntry, LCSpendInfo, TransactionInfo};
use serde::{Deserialize, Serialize};

use crate::{
    daemon::{
        history_txs, label_items,
        model::{Coin, HistoryTransaction, LabelItem},
        set_labels,
    },
    dir::NetworkDirectory,
};

const EXTERNAL_DIR: &str = "external";
const WALLET_FILE: &str = "wallet.json";
const LAYOUT_FILE: &str = "layout.json";
/// The Electrum server last asked at import, in the external directory.
const ELECTRUM_FILE: &str = "electrum.json";
/// Directory of the bwk accounts inside the wallet directory, one account per descriptor.
const ACCOUNTS_DIR: &str = "accounts";

/// A scan is over once no scan event arrived for this long.
const SCAN_QUIET: Duration = Duration::from_secs(3);
const SCAN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// A wallet that is not a Liana wallet, scanned with bwk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalWallet {
    pub id: String,
    pub name: String,
    pub descriptors: Vec<Descriptor<DescriptorPublicKey>>,
    pub created: u32,
    pub last_scan: Option<u32>,
    /// Electrum server asked at import, `None` when the current wallet's one is used.
    pub electrum: Option<String>,
}

impl ExternalWallet {
    pub fn new(
        name: String,
        descriptors: Vec<Descriptor<DescriptorPublicKey>>,
        created: u32,
        electrum: Option<String>,
    ) -> Result<Self, ImportError> {
        let first = descriptors.first().ok_or(ImportError::Empty)?;
        Ok(Self {
            id: format!("{}-{created}", checksum(first)),
            name,
            descriptors,
            created,
            last_scan: None,
            electrum,
        })
    }

    /// The checksum part of the id.
    pub fn checksum(&self) -> &str {
        self.id
            .split_once('-')
            .map_or(&self.id, |(checksum, _)| checksum)
    }

    pub fn dir(&self, network_dir: &NetworkDirectory) -> PathBuf {
        external_dir(network_dir).join(&self.id)
    }

    pub fn save(&self, network_dir: &NetworkDirectory) -> Result<(), ExternalError> {
        let dir = self.dir(network_dir);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(WALLET_FILE), serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

fn external_dir(network_dir: &NetworkDirectory) -> PathBuf {
    network_dir.path().join(EXTERNAL_DIR)
}

fn read_wallet(dir: &Path) -> Result<ExternalWallet, ExternalError> {
    Ok(serde_json::from_slice(&fs::read(dir.join(WALLET_FILE))?)?)
}

/// The external wallets of the network, oldest first, unreadable ones left out.
pub fn external_wallets(network_dir: &NetworkDirectory) -> Vec<ExternalWallet> {
    let Ok(entries) = fs::read_dir(external_dir(network_dir)) else {
        return Vec::new();
    };
    let mut wallets: Vec<_> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| match read_wallet(&entry.path()) {
            Ok(wallet) => Some(wallet),
            Err(e) => {
                log::warn!(
                    "Skipping external wallet at '{}': {e}",
                    entry.path().display()
                );
                None
            }
        })
        .collect();
    wallets.sort_by_key(|wallet| wallet.created);
    wallets
}

pub fn remove(network_dir: &NetworkDirectory, id: &str) -> Result<(), ExternalError> {
    Ok(fs::remove_dir_all(external_dir(network_dir).join(id))?)
}

#[derive(Serialize, Deserialize)]
struct RememberedElectrum {
    addr: String,
}

/// The Electrum server last asked at import, `None` when none was asked yet.
pub fn remembered_electrum(
    network_dir: &NetworkDirectory,
) -> Result<Option<String>, ExternalError> {
    match fs::read(external_dir(network_dir).join(ELECTRUM_FILE)) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice::<RememberedElectrum>(&bytes)?.addr,
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn remember_electrum(network_dir: &NetworkDirectory, addr: &str) -> Result<(), ExternalError> {
    let dir = external_dir(network_dir);
    fs::create_dir_all(&dir)?;
    let remembered = RememberedElectrum {
        addr: addr.to_string(),
    };
    fs::write(
        dir.join(ELECTRUM_FILE),
        serde_json::to_vec_pretty(&remembered)?,
    )?;
    Ok(())
}

pub fn checksum(descriptor: &Descriptor<DescriptorPublicKey>) -> String {
    descriptor
        .to_string()
        .split_once('#')
        .map(|(_, checksum)| checksum.to_string())
        .expect("a displayed descriptor has a checksum")
}

#[derive(Debug)]
pub enum ImportError {
    Descriptor(liana::miniscript::Error),
    /// A key is not an extended key ending with `<0;1>/*` or `/0/*`.
    Keys,
    Network,
    Empty,
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Descriptor(e) => write!(f, "Invalid descriptor: {e}"),
            Self::Keys => write!(f, "Every key must be an xpub ending with <0;1>/* or /0/*"),
            Self::Network => write!(f, "A key is not for this network"),
            Self::Empty => write!(f, "No descriptor to import"),
        }
    }
}

impl From<liana::miniscript::Error> for ImportError {
    fn from(e: liana::miniscript::Error) -> Self {
        Self::Descriptor(e)
    }
}

/// A single key script type with its standard derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleSig {
    Pkh,
    ShWpkh,
    Wpkh,
    Tr,
}

impl SingleSig {
    /// The BIP44, BIP49, BIP84 and BIP86 purpose.
    pub fn purpose(self) -> u32 {
        match self {
            Self::Pkh => 44,
            Self::ShWpkh => 49,
            Self::Wpkh => 84,
            Self::Tr => 86,
        }
    }

    fn descriptor(
        self,
        key: DescriptorPublicKey,
    ) -> Result<Descriptor<DescriptorPublicKey>, ImportError> {
        Ok(match self {
            Self::Pkh => Descriptor::new_pkh(key)?,
            Self::ShWpkh => Descriptor::new_sh_wpkh(key)?,
            Self::Wpkh => Descriptor::new_wpkh(key)?,
            Self::Tr => Descriptor::new_tr(key, None)?,
        })
    }
}

/// The `m/purpose'/coin'/account'` path of each single key script type, coin being 0 on
/// mainnet and 1 otherwise.
pub fn standard_paths(network: Network, account: u32) -> [(SingleSig, DerivationPath); 4] {
    let coin = match NetworkKind::from(network) {
        NetworkKind::Main => 0,
        NetworkKind::Test => 1,
    };
    [
        SingleSig::Pkh,
        SingleSig::ShWpkh,
        SingleSig::Wpkh,
        SingleSig::Tr,
    ]
    .map(|single_sig| {
        let path = DerivationPath::from(vec![
            ChildNumber::Hardened {
                index: single_sig.purpose(),
            },
            ChildNumber::Hardened { index: coin },
            ChildNumber::Hardened { index: account },
        ]);
        (single_sig, path)
    })
}

/// The `<0;1>/*` descriptors of the xpubs a signing device returned for `fingerprint`.
pub fn device_descriptors(
    fingerprint: Fingerprint,
    xpubs: Vec<(SingleSig, DerivationPath, Xpub)>,
    network: Network,
) -> Result<Vec<Descriptor<DescriptorPublicKey>>, ImportError> {
    xpubs
        .into_iter()
        .map(|(single_sig, path, xpub)| {
            if xpub.network != NetworkKind::from(network) {
                return Err(ImportError::Network);
            }
            single_sig.descriptor(DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
                origin: Some((fingerprint, path)),
                xkey: xpub,
                derivation_paths: receive_change(),
                wildcard: Wildcard::Unhardened,
            }))
        })
        .collect()
}

/// Turns a `/0/*` key into `<0;1>/*`.
struct ReceiveChange;

impl Translator<DescriptorPublicKey, DescriptorPublicKey, Infallible> for ReceiveChange {
    fn pk(&mut self, pk: &DescriptorPublicKey) -> Result<DescriptorPublicKey, Infallible> {
        Ok(match pk {
            DescriptorPublicKey::XPub(key)
                if key.wildcard == Wildcard::Unhardened && key.derivation_path == path(0) =>
            {
                DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
                    origin: key.origin.clone(),
                    xkey: key.xkey,
                    derivation_paths: receive_change(),
                    wildcard: Wildcard::Unhardened,
                })
            }
            key => key.clone(),
        })
    }

    translate_hash_clone!(DescriptorPublicKey, DescriptorPublicKey, Infallible);
}

fn path(index: u32) -> DerivationPath {
    DerivationPath::from(vec![ChildNumber::Normal { index }])
}

fn receive_change() -> DerivPaths {
    DerivPaths::new(vec![path(0), path(1)]).expect("two paths")
}

/// Keys must end with `<0;1>/*`, the receive and change paths bwk scans.
pub fn parse_descriptor(
    s: &str,
    network: Network,
) -> Result<Descriptor<DescriptorPublicKey>, ImportError> {
    let descriptor = Descriptor::<DescriptorPublicKey>::from_str(s.trim())?
        .translate_pk(&mut ReceiveChange)
        .map_err(|e| match e {
            TranslateErr::TranslatorErr(never) => match never {},
            TranslateErr::OuterError(e) => ImportError::Descriptor(e),
        })?;
    let keys_ok = descriptor.for_each_key(|key| match key {
        DescriptorPublicKey::MultiXPub(key) => {
            key.wildcard == Wildcard::Unhardened && key.derivation_paths == receive_change()
        }
        _ => false,
    });
    if !keys_ok {
        return Err(ImportError::Keys);
    }
    let network_ok = descriptor.for_each_key(|key| match key {
        DescriptorPublicKey::MultiXPub(key) => key.xkey.network == NetworkKind::from(network),
        _ => false,
    });
    if !network_ok {
        return Err(ImportError::Network);
    }
    Ok(descriptor)
}

#[derive(Debug)]
pub enum ScanError {
    /// The Electrum address is not `[ssl://|tcp://]host:port`.
    Url(String),
    Open(open::Error),
    Listener(tx_listener::Error),
    InvalidElectrumConfig,
    Disconnected,
    Timeout,
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Url(url) => write!(f, "Invalid Electrum server '{url}'"),
            Self::Open(e) => write!(f, "Cannot open the wallet data: {e}"),
            Self::Listener(e) => write!(f, "Electrum error: {e}"),
            Self::InvalidElectrumConfig => write!(f, "Invalid Electrum server"),
            Self::Disconnected => write!(f, "Disconnected from the Electrum server"),
            Self::Timeout => write!(f, "The scan did not finish in time"),
        }
    }
}

impl From<open::Error> for ScanError {
    fn from(e: open::Error) -> Self {
        Self::Open(e)
    }
}

/// The url and port bwk connects to, from an Electrum address like `ssl://host:port`.
pub fn electrum_endpoint(addr: &str) -> Result<(String, u16), ScanError> {
    let invalid = || ScanError::Url(addr.to_string());
    let (Some(host), Some(port), scheme) = parse_electrum_url(addr).map_err(|_| invalid())? else {
        return Err(invalid());
    };
    let url = match scheme {
        ElectrumScheme::Ssl => format!("ssl://{host}"),
        ElectrumScheme::Tcp => host,
    };
    Ok((url, port))
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScanResult {
    pub descriptor: Descriptor<DescriptorPublicKey>,
    pub has_history: bool,
}

fn account_config(
    descriptor: &Descriptor<DescriptorPublicKey>,
    dir: &Path,
    network: Network,
) -> Config {
    let mut config = Config::new(
        None,
        checksum(descriptor),
        network,
        ScriptType::Descriptor(Box::new(descriptor.clone())),
        dir.to_path_buf(),
        ACCOUNTS_DIR.to_string(),
        Some(PersistenceKind::Json),
    )
    .expect("a descriptor account needs no mnemonic");
    // A mainnet header store needs a checkpoint: take the confirmations the server reports.
    config.scanner.header_scanner = false;
    config.scanner.stay_offline = true;
    config
}

fn open_account(
    descriptor: &Descriptor<DescriptorPublicKey>,
    dir: &Path,
    network: Network,
) -> Result<Account, open::Error> {
    Account::try_new(account_config(descriptor, dir, network))
}

/// Scans each descriptor from the Electrum server into the bwk accounts under `dir`, blocking.
pub fn scan(
    descriptors: &[Descriptor<DescriptorPublicKey>],
    electrum: &str,
    dir: &Path,
    network: Network,
) -> Result<Vec<ScanResult>, ScanError> {
    let (url, port) = electrum_endpoint(electrum)?;
    let (sender, receiver) = mpsc::channel();
    let mut accounts = descriptors
        .iter()
        .map(|descriptor| {
            let mut config = account_config(descriptor, dir, network);
            config.scanner.set_electrum(Some(url.clone()), Some(port));
            config.scanner.stay_offline = false;
            Account::try_new_with_sender(config, sender.clone())
        })
        .collect::<Result<Vec<Account>, _>>()?;
    let waited = wait_scan(&receiver, accounts.len());
    for account in &mut accounts {
        account.stop_electrum();
    }
    waited?;
    Ok(descriptors
        .iter()
        .zip(&accounts)
        .map(|(descriptor, account)| ScanResult {
            descriptor: descriptor.clone(),
            has_history: !account.scanner().tx_history().is_empty(),
        })
        .collect())
}

/// Waits until the `accounts` are connected and no scan event arrived for `SCAN_QUIET`.
fn wait_scan(receiver: &mpsc::Receiver<Notification>, accounts: usize) -> Result<(), ScanError> {
    let deadline = Instant::now() + SCAN_TIMEOUT;
    let mut connected = 0;
    let mut last_event = Instant::now();
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(ScanError::Timeout);
        }
        let wait = if connected < accounts {
            deadline - now
        } else {
            let quiet_end = last_event + SCAN_QUIET;
            if now >= quiet_end {
                return Ok(());
            }
            quiet_end.min(deadline) - now
        };
        match receiver.recv_timeout(wait) {
            Ok(Notification::Electrum(TxListenerNotif::Connected(_))) => {
                connected += 1;
                last_event = Instant::now();
            }
            Ok(
                Notification::CoinUpdate
                | Notification::AddressTipChanged
                | Notification::CoinReceived { .. }
                | Notification::PaymentHistoryUpdated,
            ) => last_event = Instant::now(),
            Ok(Notification::Electrum(TxListenerNotif::Disconnected))
            | Err(RecvTimeoutError::Disconnected) => return Err(ScanError::Disconnected),
            Ok(Notification::Electrum(TxListenerNotif::Error(e))) => {
                return Err(ScanError::Listener(e))
            }
            Ok(Notification::InvalidElectrumConfig) => {
                return Err(ScanError::InvalidElectrumConfig)
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

#[derive(Debug)]
pub enum ExternalError {
    Io(io::Error),
    Json(serde_json::Error),
    Open(open::Error),
    Address(address::ParseError),
    /// The coin is not on the receive or change path of a descriptor.
    Coin(OutPoint),
    Empty,
}

impl fmt::Display for ExternalError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "External wallet file error: {e}"),
            Self::Json(e) => write!(f, "Invalid external wallet file: {e}"),
            Self::Open(e) => write!(f, "Cannot open the wallet data: {e}"),
            Self::Address(e) => write!(f, "Invalid coin address: {e}"),
            Self::Coin(outpoint) => write!(f, "Unexpected coin {outpoint}"),
            Self::Empty => write!(f, "The external wallet has no descriptor"),
        }
    }
}

impl From<io::Error> for ExternalError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for ExternalError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<open::Error> for ExternalError {
    fn from(e: open::Error) -> Self {
        Self::Open(e)
    }
}

impl From<address::ParseError> for ExternalError {
    fn from(e: address::ParseError) -> Self {
        Self::Address(e)
    }
}

/// The scanned history of an external wallet, with its labels.
#[derive(Debug)]
pub struct ExternalHistory {
    pub txs: Vec<HistoryTransaction>,
    pub coins: Vec<Coin>,
}

fn open_accounts(
    wallet: &ExternalWallet,
    network_dir: &NetworkDirectory,
    network: Network,
) -> Result<Vec<Account>, ExternalError> {
    if wallet.descriptors.is_empty() {
        return Err(ExternalError::Empty);
    }
    let dir = wallet.dir(network_dir);
    Ok(wallet
        .descriptors
        .iter()
        .map(|descriptor| open_account(descriptor, &dir, network))
        .collect::<Result<_, _>>()?)
}

/// Reads the last scan of `wallet` without network access, blocking.
pub fn load_external(
    wallet: &ExternalWallet,
    network_dir: &NetworkDirectory,
    network: Network,
) -> Result<ExternalHistory, ExternalError> {
    let accounts = open_accounts(wallet, network_dir, network)?;
    let (txs, coins) = convert(
        accounts
            .iter()
            .flat_map(|account| account.scanner().tx_history()),
        accounts
            .iter()
            .flat_map(|account| account.scanner().coins().into_values()),
        network,
    )?;
    let mut txs = history_txs(txs, &coins, network);
    let labels = {
        // The labels of an external wallet live in the store of its first account.
        let store = accounts[0]
            .scanner()
            .label_store()
            .lock()
            .expect("poisoned");
        label_items(&txs)
            .into_iter()
            .filter_map(|item| {
                store
                    .get(&label_key(&item))
                    .map(|label| (item.to_string(), label))
            })
            .collect()
    };
    set_labels(&mut txs, labels);
    Ok(ExternalHistory { txs, coins })
}

/// The transactions, deduplicated, and the coins of several bwk accounts.
fn convert(
    entries: impl IntoIterator<Item = TxEntry>,
    coins: impl IntoIterator<Item = CoinEntry>,
    network: Network,
) -> Result<(Vec<TransactionInfo>, Vec<Coin>), ExternalError> {
    let txs: Vec<TransactionInfo> = entries
        .into_iter()
        .map(|entry| (entry.txid(), entry))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .map(|entry| TransactionInfo {
            height: entry.height().map(|height| height as i32),
            time: entry.timestamp().map(|time| time as u32),
            tx: entry.tx().clone(),
            default_label: Label::None,
        })
        .collect();
    let coins = coins
        .into_iter()
        .map(|entry| coin(entry, &txs, network))
        .collect::<Result<_, _>>()?;
    Ok((txs, coins))
}

/// `entry` as a wallet coin, spent by the transaction of `txs` spending it.
fn coin(
    entry: CoinEntry,
    txs: &[TransactionInfo],
    network: Network,
) -> Result<Coin, ExternalError> {
    let outpoint = entry.coin.outpoint;
    let CoinSpendInfo::Bip32 {
        coin_path: (keychain, index),
        ..
    } = entry.coin.spend_info
    else {
        return Err(ExternalError::Coin(outpoint));
    };
    let is_change = match keychain {
        KeyChain::Receive => false,
        KeyChain::Change => true,
        KeyChain::Custom(_) => return Err(ExternalError::Coin(outpoint)),
    };
    let derivation_index =
        ChildNumber::from_normal_idx(index).map_err(|_| ExternalError::Coin(outpoint))?;
    let spend_info = txs
        .iter()
        .find(|tx| {
            tx.tx
                .input
                .iter()
                .any(|input| input.previous_output == outpoint)
        })
        .map(|tx| LCSpendInfo {
            txid: tx.tx.compute_txid(),
            height: tx.height,
        });
    Ok(Coin {
        amount: entry.coin.txout.value,
        outpoint,
        address: entry.address.require_network(network)?,
        block_height: entry.coin.height.map(|height| height as i32),
        derivation_index,
        spend_info,
        is_immature: false,
        is_change,
        is_from_self: false,
        default_label: Label::None,
    })
}

fn label_key(item: &LabelItem) -> LabelKey {
    match item {
        LabelItem::Address(address) => LabelKey::Address(address.as_unchecked().clone()),
        LabelItem::Txid(txid) => LabelKey::Transaction(*txid),
        LabelItem::OutPoint(outpoint) => LabelKey::OutPoint(*outpoint),
    }
}

/// Writes labels of `wallet` to its first bwk account, blocking.
pub fn save_external_labels(
    wallet: &ExternalWallet,
    network_dir: &NetworkDirectory,
    network: Network,
    items: &HashMap<LabelItem, Option<String>>,
) -> Result<(), ExternalError> {
    let accounts = open_accounts(wallet, network_dir, network)?;
    let mut store = accounts[0]
        .scanner()
        .label_store()
        .lock()
        .expect("poisoned");
    for (item, label) in items {
        store.edit(label_key(item), label.clone());
    }
    store.persist();
    Ok(())
}

/// The layout entries stored in `dir`, none when the file is missing.
pub fn load_layout(dir: &Path) -> Result<Vec<GraphLayoutEntry>, ExternalError> {
    match fs::read(dir.join(LAYOUT_FILE)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

/// Upserts `set` and deletes `remove` in the layout stored in `dir`.
pub fn save_layout(
    dir: &Path,
    set: &[GraphLayoutEntry],
    remove: &[GraphItem],
) -> Result<(), ExternalError> {
    let mut entries = load_layout(dir)?;
    for new in set {
        match entries.iter_mut().find(|entry| entry.item == new.item) {
            Some(entry) => *entry = new.clone(),
            None => entries.push(new.clone()),
        }
    }
    let remove: HashSet<_> = remove.iter().collect();
    entries.retain(|entry| !remove.contains(&entry.item));
    fs::write(dir.join(LAYOUT_FILE), serde_json::to_vec_pretty(&entries)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{env, fs, process, str::FromStr};

    use bwk::{
        bwk_coin::{Coin as BwkCoin, CoinSpendInfo, CoinStatus, KeyChain},
        bwk_electrum::{coin_store::CoinEntry, tx_store::TxEntry},
    };
    use liana::miniscript::bitcoin::{
        absolute::LockTime,
        bip32::{ChildNumber, DerivationPath, Fingerprint, Xpub},
        transaction::Version,
        Address, Amount, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
        Witness,
    };
    use lianad::commands::{GraphItem, GraphLayoutEntry};

    use crate::{
        app::state::map::external::{
            convert, device_descriptors, electrum_endpoint, external_wallets, load_layout,
            parse_descriptor, remember_electrum, remembered_electrum, remove, save_layout,
            standard_paths, ExternalError, ExternalWallet, ImportError, ScanError, SingleSig,
        },
        dir::NetworkDirectory,
    };

    const TPUB: &str = "tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N";
    const XPUB: &str = "xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW";

    fn network_dir(test: &str) -> NetworkDirectory {
        let path = env::temp_dir().join(format!("liana-gui-map-external-{}-{test}", process::id()));
        fs::create_dir_all(&path).unwrap();
        NetworkDirectory::new(path)
    }

    fn paths(paths: [&str; 4]) -> Vec<(SingleSig, DerivationPath)> {
        [
            SingleSig::Pkh,
            SingleSig::ShWpkh,
            SingleSig::Wpkh,
            SingleSig::Tr,
        ]
        .iter()
        .zip(paths)
        .map(|(single_sig, path)| (*single_sig, DerivationPath::from_str(path).unwrap()))
        .collect()
    }

    #[test]
    fn standard_paths_per_network() {
        assert_eq!(
            standard_paths(Network::Bitcoin, 0).to_vec(),
            paths(["m/44'/0'/0'", "m/49'/0'/0'", "m/84'/0'/0'", "m/86'/0'/0'"])
        );
        for network in [Network::Testnet, Network::Signet, Network::Regtest] {
            assert_eq!(
                standard_paths(network, 0).to_vec(),
                paths(["m/44'/1'/0'", "m/49'/1'/0'", "m/84'/1'/0'", "m/86'/1'/0'"])
            );
        }
        assert_eq!(
            standard_paths(Network::Testnet, 1).to_vec(),
            paths(["m/44'/1'/1'", "m/49'/1'/1'", "m/84'/1'/1'", "m/86'/1'/1'"])
        );
    }

    fn device_xpubs(network: Network, xpub: &str) -> Vec<(SingleSig, DerivationPath, Xpub)> {
        standard_paths(network, 0)
            .iter()
            .map(|(single_sig, path)| (*single_sig, path.clone(), Xpub::from_str(xpub).unwrap()))
            .collect()
    }

    #[test]
    fn device_gives_four_descriptors() {
        let fingerprint = Fingerprint::from_str("f5acc2fd").unwrap();
        assert_eq!(
            device_descriptors(
                fingerprint,
                device_xpubs(Network::Testnet, TPUB),
                Network::Testnet
            )
            .unwrap()
            .iter()
            .map(|descriptor| descriptor.to_string())
            .collect::<Vec<_>>(),
            vec![
                format!("pkh([f5acc2fd/44'/1'/0']{TPUB}/<0;1>/*)#xvsglrz0"),
                format!("sh(wpkh([f5acc2fd/49'/1'/0']{TPUB}/<0;1>/*))#rvt425cr"),
                format!("wpkh([f5acc2fd/84'/1'/0']{TPUB}/<0;1>/*)#xlrwytju"),
                format!("tr([f5acc2fd/86'/1'/0']{TPUB}/<0;1>/*)#war8l84f"),
            ]
        );
    }

    #[test]
    fn device_xpub_of_another_network_is_rejected() {
        let fingerprint = Fingerprint::from_str("f5acc2fd").unwrap();
        assert!(matches!(
            device_descriptors(
                fingerprint,
                device_xpubs(Network::Bitcoin, TPUB),
                Network::Bitcoin
            ),
            Err(ImportError::Network)
        ));
        assert!(matches!(
            device_descriptors(
                fingerprint,
                device_xpubs(Network::Testnet, XPUB),
                Network::Testnet
            ),
            Err(ImportError::Network)
        ));
        assert!(device_descriptors(
            fingerprint,
            device_xpubs(Network::Bitcoin, XPUB),
            Network::Bitcoin
        )
        .is_ok());
    }

    #[test]
    fn descriptor_keys() {
        assert_eq!(
            parse_descriptor(&format!("wpkh({TPUB}/<0;1>/*)"), Network::Signet)
                .unwrap()
                .to_string(),
            format!("wpkh({TPUB}/<0;1>/*)#vkwlmr4k")
        );
        assert_eq!(
            parse_descriptor(&format!("wpkh({TPUB}/0/*)"), Network::Regtest)
                .unwrap()
                .to_string(),
            format!("wpkh({TPUB}/<0;1>/*)#vkwlmr4k")
        );
        for rejected in [
            format!("wpkh({TPUB}/*)"),
            format!("wpkh({TPUB}/1/*)"),
            format!("wpkh({TPUB}/0/0/*)"),
            format!("wpkh({TPUB}/<0;1>/0/*)"),
            format!("wpkh({TPUB}/<2;3>/*)"),
            format!("wpkh({TPUB}/<0;1;2>/*)"),
            format!("wpkh({TPUB}/0/*')"),
            format!("wpkh({TPUB}/0)"),
            "wpkh(02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5)".to_string(),
        ] {
            assert!(
                matches!(
                    parse_descriptor(&rejected, Network::Testnet),
                    Err(ImportError::Keys)
                ),
                "{}",
                rejected
            );
        }
    }

    #[test]
    fn descriptor_of_another_network_is_rejected() {
        assert!(matches!(
            parse_descriptor(&format!("wpkh({TPUB}/<0;1>/*)"), Network::Bitcoin),
            Err(ImportError::Network)
        ));
        assert!(matches!(
            parse_descriptor(
                &format!("wsh(multi(1,{TPUB}/<0;1>/*,{XPUB}/<0;1>/*))"),
                Network::Testnet
            ),
            Err(ImportError::Network)
        ));
        assert!(matches!(
            parse_descriptor("wpkh(notakey)", Network::Testnet),
            Err(ImportError::Descriptor(_))
        ));
    }

    #[test]
    fn electrum_addresses() {
        assert_eq!(
            electrum_endpoint("ssl://electrum.example.com:50002").unwrap(),
            ("ssl://electrum.example.com".to_string(), 50002)
        );
        assert_eq!(
            electrum_endpoint("tcp://127.0.0.1:60401").unwrap(),
            ("127.0.0.1".to_string(), 60401)
        );
        assert_eq!(
            electrum_endpoint("127.0.0.1:60401").unwrap(),
            ("127.0.0.1".to_string(), 60401)
        );
        for invalid in ["ssl://electrum.example.com", "", "wss://host:443"] {
            assert!(
                matches!(electrum_endpoint(invalid), Err(ScanError::Url(url)) if url == invalid),
                "{}",
                invalid
            );
        }
    }

    #[test]
    fn wallet_files() {
        let dir = network_dir("wallets");
        assert_eq!(external_wallets(&dir), Vec::new());

        let descriptor = |s: String| parse_descriptor(&s, Network::Testnet).unwrap();
        let mut savings = ExternalWallet::new(
            "Savings".to_string(),
            vec![
                descriptor(format!("wpkh({TPUB}/<0;1>/*)")),
                descriptor(format!("tr({TPUB}/<0;1>/*)")),
            ],
            1700000001,
            Some("ssl://electrum.example.com:50002".to_string()),
        )
        .unwrap();
        savings.last_scan = Some(1700000100);
        let legacy = ExternalWallet::new(
            "Legacy".to_string(),
            vec![descriptor(format!("pkh({TPUB}/<0;1>/*)"))],
            1700000000,
            None,
        )
        .unwrap();
        assert_eq!(savings.id, "vkwlmr4k-1700000001");
        assert_eq!(savings.checksum(), "vkwlmr4k");
        assert_eq!(legacy.id, "fldssyp5-1700000000");
        assert!(matches!(
            ExternalWallet::new("Empty".to_string(), Vec::new(), 1700000002, None),
            Err(ImportError::Empty)
        ));

        savings.save(&dir).unwrap();
        legacy.save(&dir).unwrap();
        fs::create_dir_all(dir.path().join("external").join("broken")).unwrap();
        fs::write(
            dir.path()
                .join("external")
                .join("broken")
                .join("wallet.json"),
            "{",
        )
        .unwrap();
        remember_electrum(&dir, "ssl://electrum.example.com:50002").unwrap();
        assert_eq!(
            external_wallets(&dir),
            vec![legacy.clone(), savings.clone()]
        );

        remove(&dir, &legacy.id).unwrap();
        assert!(!legacy.dir(&dir).exists());
        assert_eq!(external_wallets(&dir), vec![savings]);
        assert!(matches!(
            remove(&dir, &legacy.id),
            Err(ExternalError::Io(_))
        ));

        fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn electrum_file() {
        let dir = network_dir("electrum");
        assert_eq!(remembered_electrum(&dir).unwrap(), None);
        remember_electrum(&dir, "ssl://electrum.example.com:50002").unwrap();
        remember_electrum(&dir, "tcp://127.0.0.1:60401").unwrap();
        assert_eq!(
            remembered_electrum(&dir).unwrap(),
            Some("tcp://127.0.0.1:60401".to_string())
        );
        fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn layout_file() {
        let dir = network_dir("layout");
        assert_eq!(load_layout(dir.path()).unwrap(), Vec::new());

        let tx = GraphItem::Tx(Txid::from_str(&"01".repeat(32)).unwrap());
        let leaf =
            GraphItem::OutputLeaf(OutPoint::from_str(&format!("{}:1", "02".repeat(32))).unwrap());
        let entry = |item, position| GraphLayoutEntry {
            item,
            position,
            input_order: None,
            output_order: Some(vec![1, 0]),
            lane_position: None,
        };
        save_layout(
            dir.path(),
            &[entry(tx, Some((1.0, 2.0))), entry(leaf, None)],
            &[],
        )
        .unwrap();
        assert_eq!(
            load_layout(dir.path()).unwrap(),
            vec![entry(tx, Some((1.0, 2.0))), entry(leaf, None)]
        );

        save_layout(dir.path(), &[entry(tx, Some((3.0, 4.0)))], &[leaf]).unwrap();
        assert_eq!(
            load_layout(dir.path()).unwrap(),
            vec![entry(tx, Some((3.0, 4.0)))]
        );

        fs::remove_dir_all(dir.path()).unwrap();
    }

    fn tx(inputs: &[OutPoint], outputs: usize) -> Transaction {
        Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: inputs
                .iter()
                .map(|outpoint| TxIn {
                    previous_output: *outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Witness::new(),
                })
                .collect(),
            output: (0..outputs)
                .map(|_| TxOut {
                    value: Amount::from_sat(1000),
                    script_pubkey: ScriptBuf::new(),
                })
                .collect(),
        }
    }

    fn coin_entry(outpoint: OutPoint, coin_path: (KeyChain, u32)) -> CoinEntry {
        CoinEntry {
            coin: BwkCoin {
                txout: TxOut {
                    value: Amount::from_sat(1000),
                    script_pubkey: ScriptBuf::new(),
                },
                outpoint,
                height: Some(100),
                sequence: Sequence::MAX,
                status: CoinStatus::ConfirmedUnverified,
                label: None,
                satisfaction_size: 0,
                spend_info: CoinSpendInfo::Bip32 {
                    coin_path,
                    descriptor: parse_descriptor(
                        &format!("wpkh({TPUB}/<0;1>/*)"),
                        Network::Testnet,
                    )
                    .unwrap(),
                    secret_key: None,
                },
            },
            address: Address::from_str("tb1qfufcrdyarcg5eph608c6l8vktrc9re6agu4se2").unwrap(),
        }
    }

    #[test]
    fn coins_from_bwk() {
        let funding = tx(
            &[OutPoint::from_str(&format!("{}:0", "03".repeat(32))).unwrap()],
            2,
        );
        let receive = OutPoint::new(funding.compute_txid(), 0);
        let change = OutPoint::new(funding.compute_txid(), 1);
        let spending = tx(&[receive], 1);

        // The funding tx is reported by both accounts.
        let (txs, coins) = convert(
            [
                TxEntry::unconfirmed(funding.clone()),
                TxEntry::unconfirmed(spending.clone()),
                TxEntry::unconfirmed(funding.clone()),
            ],
            [
                coin_entry(receive, (KeyChain::Receive, 3)),
                coin_entry(change, (KeyChain::Change, 5)),
            ],
            Network::Testnet,
        )
        .unwrap();
        let mut txids: Vec<_> = txs.iter().map(|tx| tx.tx.compute_txid()).collect();
        txids.sort();
        let mut expected = vec![funding.compute_txid(), spending.compute_txid()];
        expected.sort();
        assert_eq!(txids, expected);
        assert!(txs
            .iter()
            .all(|tx| tx.height.is_none() && tx.time.is_none()));

        assert_eq!(
            coins
                .iter()
                .map(|coin| (
                    coin.outpoint,
                    coin.derivation_index,
                    coin.is_change,
                    coin.block_height,
                    coin.spend_info.map(|spend| (spend.txid, spend.height))
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    receive,
                    ChildNumber::from_normal_idx(3).unwrap(),
                    false,
                    Some(100),
                    Some((spending.compute_txid(), None))
                ),
                (
                    change,
                    ChildNumber::from_normal_idx(5).unwrap(),
                    true,
                    Some(100),
                    None
                ),
            ]
        );

        assert!(matches!(
            convert(
                [],
                [coin_entry(receive, (KeyChain::Custom(2), 0))],
                Network::Testnet
            ),
            Err(ExternalError::Coin(outpoint)) if outpoint == receive
        ));
        assert!(matches!(
            convert(
                [],
                [coin_entry(receive, (KeyChain::Receive, 0))],
                Network::Bitcoin
            ),
            Err(ExternalError::Address(_))
        ));
    }
}
