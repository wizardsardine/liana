use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::daemon::model::{Coin, HistoryTransaction, SpendStatus, SpendTx};
use crate::dir::CoincubeDirectory;
use crate::{
    app::settings, daemon::DaemonBackend, hw::HardwareWalletConfig, node::NodeType, signer::Signer,
};
use coincubed::commands::LCSpendInfo;

use coincube_core::{miniscript::bitcoin, signer::MasterSigner};

use coincube_core::descriptors::CoincubeDescriptor;
use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
use coincube_core::miniscript::bitcoin::{Network, OutPoint, Transaction, Txid};

use super::settings::{WalletId, WalletSettings};

const DEFAULT_WALLET_NAME: &str = "Coincube";

pub fn wallet_name(main_descriptor: &CoincubeDescriptor) -> String {
    let desc = main_descriptor.to_string();
    let checksum = desc
        .split_once('#')
        .map(|(_, checksum)| checksum)
        .unwrap_or("");
    format!(
        "{}{}{}",
        DEFAULT_WALLET_NAME,
        if checksum.is_empty() { "" } else { "-" },
        checksum
    )
}

/// In-memory record of a transaction the user has just broadcast from
/// this wallet. The daemon derives `SpendStatus::Broadcast` and
/// `coin.spend_info` from its mempool poller; until that poller observes
/// the tx, the GUI would otherwise show the spend as if it had never
/// happened — stale Pending PSBTs, an un-debited balance, no entry in
/// the Transactions list. Holding the broadcast data here lets the
/// panels apply optimistic overrides until the daemon catches up.
///
/// Captures only what the panels need to synthesize their views:
/// the broadcast `Transaction`, the input `Coin`s being spent, the
/// PSBT's change indices, and the wallet's network. Entries are
/// cleared by `reconcile_with_coins` once daemon-side state reflects
/// the spend.
#[derive(Debug, Clone)]
pub struct RecentBroadcast {
    pub tx: Transaction,
    pub input_coins: Vec<Coin>,
    pub change_indexes: Vec<usize>,
    pub network: Network,
}

#[derive(Debug, Clone)]
pub struct Wallet {
    pub name: String,
    pub alias: Option<String>,
    pub main_descriptor: CoincubeDescriptor,
    pub descriptor_checksum: String,
    pub pinned_at: Option<i64>,
    // TODO: We could replace these two fields with `keys: HashMap<Fingerprint, settings::KeySetting>`.
    pub keys_aliases: HashMap<Fingerprint, String>,
    pub provider_keys: HashMap<Fingerprint, settings::ProviderKey>,
    pub border_wallet_fingerprints: HashSet<Fingerprint>,
    pub hardware_wallets: Vec<HardwareWalletConfig>,
    pub signer: Option<Arc<Signer>>,
    /// Txids the user has just broadcast locally, mapped to the data
    /// needed to synthesize coin/tx/PSBT overrides until the daemon
    /// catches up. `Arc<Mutex<...>>` so the map is shared across every
    /// `Arc<Wallet>` clone held by the panels and the BroadcastModal —
    /// recording happens in one place, every reader sees it.
    pub recently_broadcast: Arc<Mutex<HashMap<Txid, RecentBroadcast>>>,
}

impl Wallet {
    pub fn new(main_descriptor: CoincubeDescriptor) -> Self {
        Self {
            name: wallet_name(&main_descriptor),
            alias: None,
            descriptor_checksum: main_descriptor
                .to_string()
                .split_once('#')
                .map(|(_, checksum)| checksum)
                .unwrap()
                .to_string(),
            pinned_at: None,
            main_descriptor,
            keys_aliases: HashMap::new(),
            provider_keys: HashMap::new(),
            border_wallet_fingerprints: HashSet::new(),
            hardware_wallets: Vec::new(),
            signer: None,
            recently_broadcast: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Acquires the `recently_broadcast` lock, recovering from
    /// poisoning rather than dropping the operation silently.
    ///
    /// A poisoned mutex means some earlier thread panicked while
    /// holding the lock. The data inside is a `HashMap` whose
    /// mutations (insert/remove/retain) are atomic — none of our
    /// access patterns leaves the map in a half-modified state — so
    /// recovery via `into_inner()` is safe. We log a warning so
    /// poisoning shows up in diagnostics; silently returning `Err`
    /// here would make the optimistic-broadcast UI fail with no
    /// explanation.
    fn lock_recently_broadcast(&self) -> std::sync::MutexGuard<'_, HashMap<Txid, RecentBroadcast>> {
        self.recently_broadcast.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(
                target: "coincube_gui::wallet",
                "recently_broadcast mutex was poisoned; recovering. \
                 Map state is consistent (HashMap mutations are atomic)."
            );
            poisoned.into_inner()
        })
    }

    /// Records a freshly-broadcast transaction so the panels can apply
    /// optimistic overrides until the daemon's mempool poller catches
    /// up. Only call after `broadcast_spend_tx` has returned `Ok`.
    pub fn record_broadcast(
        &self,
        tx: Transaction,
        input_coins: Vec<Coin>,
        change_indexes: Vec<usize>,
        network: Network,
    ) {
        let txid = tx.compute_txid();
        let mut map = self.lock_recently_broadcast();
        map.insert(
            txid,
            RecentBroadcast {
                tx,
                input_coins,
                change_indexes,
                network,
            },
        );
    }

    /// Adds synthetic `spend_info` to coins whose outpoints match the
    /// inputs of a recently-broadcast tx. Daemon-provided `spend_info`
    /// is preserved if already present (daemon view is the source of
    /// truth once available).
    pub fn apply_coin_overrides(&self, coins: &mut [Coin]) {
        let map = self.lock_recently_broadcast();
        if map.is_empty() {
            return;
        }
        let mut outpoint_to_txid: HashMap<OutPoint, Txid> = HashMap::new();
        for (txid, rb) in map.iter() {
            for coin in &rb.input_coins {
                outpoint_to_txid.insert(coin.outpoint, *txid);
            }
        }
        for coin in coins.iter_mut() {
            if coin.spend_info.is_none() {
                if let Some(txid) = outpoint_to_txid.get(&coin.outpoint) {
                    coin.spend_info = Some(LCSpendInfo {
                        txid: *txid,
                        height: None,
                    });
                }
            }
        }
    }

    /// Promotes `Pending` PSBTs to `Broadcast` when the user has
    /// already broadcast them locally but the daemon hasn't yet
    /// reflected the spend in its coin state.
    ///
    /// Pure: never mutates `recently_broadcast`. Entries are dropped
    /// only by `reconcile_with_coins`, which runs on the cache update
    /// path. Doing reconciliation here would race with an in-flight
    /// cache update task — if this method ran first and removed an
    /// entry, the cache update could then fetch stale (pre-catchup)
    /// coins, find an empty map, and store coins without the synthetic
    /// `spend_info` — yielding a temporarily-wrong balance.
    pub fn apply_spend_tx_overrides(&self, txs: &mut [SpendTx]) {
        let map = self.lock_recently_broadcast();
        if map.is_empty() {
            return;
        }
        for tx in txs.iter_mut() {
            if matches!(tx.status, SpendStatus::Pending) {
                let txid = tx.psbt.unsigned_tx.compute_txid();
                if map.contains_key(&txid) {
                    tx.status = SpendStatus::Broadcast;
                }
            }
        }
    }

    /// Returns synthesized pending `HistoryTransaction`s for any
    /// recently-broadcast tx not already present in `existing_txids`.
    /// The Transactions panel merges these with daemon-supplied
    /// pending txs so the broadcast shows up immediately.
    ///
    /// Pure: never mutates `recently_broadcast` (see
    /// `apply_spend_tx_overrides` for the race this avoids).
    /// Read-time filtering against `existing_txids` is enough to
    /// prevent a duplicate row once the daemon lists the tx itself —
    /// the orphan entry stays in the map until `reconcile_with_coins`
    /// observes a matching output coin, but that doesn't affect this
    /// panel's display.
    pub fn synthesized_pending_history_txs(
        &self,
        existing_txids: &HashSet<Txid>,
    ) -> Vec<HistoryTransaction> {
        let map = self.lock_recently_broadcast();
        if map.is_empty() {
            return Vec::new();
        }
        map.iter()
            .filter(|(txid, _)| !existing_txids.contains(*txid))
            .map(|(_, rb)| {
                HistoryTransaction::new(
                    rb.tx.clone(),
                    None,
                    None,
                    rb.input_coins.clone(),
                    rb.change_indexes.clone(),
                    rb.network,
                )
            })
            .collect()
    }

    /// Drops entries the daemon has caught up on. The cache path
    /// calls this with the result of
    /// `list_coins(&[Unconfirmed, Confirmed], &[])`, so `coins` never
    /// contains spend_info-bearing coins — the daemon's filter
    /// already excluded them. Two complementary signals catch the
    /// catch-up:
    ///
    /// 1. **Any broadcast input is no longer in the returned coin
    ///    set.** The inputs were Unconfirmed/Confirmed at
    ///    `record_broadcast` time, so their absence means the daemon
    ///    moved them to `Spending`/`Spent` — that catches our own
    ///    broadcast, an RBF replacement, or a conflicting tx
    ///    consuming the input. All three are reasons our optimistic
    ///    override is no longer authoritative.
    /// 2. **A wallet-tracked output of the broadcast tx has
    ///    appeared.** Faster signal when the tx has change; the
    ///    daemon may surface the new output coin before its poller
    ///    finishes flagging the inputs.
    ///
    /// An empty `coins` result is treated as legitimate: it can
    /// genuinely happen after the user spends every UTXO with no
    /// change output, and skipping reconciliation in that case left a
    /// stale entry that produced a duplicate pending row in the
    /// Transactions panel once the tx confirmed. The input-
    /// disappearance check already handles the empty case correctly
    /// (all inputs absent ⇒ entry cleared). Mid-sync transients
    /// briefly clearing entries is acceptable: they self-correct
    /// once the daemon catches up, since by then the daemon shows
    /// authoritative state on its own.
    pub fn reconcile_with_coins(&self, coins: &[Coin]) {
        let mut map = self.lock_recently_broadcast();
        if map.is_empty() {
            return;
        }
        let present_outpoints: HashSet<OutPoint> = coins.iter().map(|c| c.outpoint).collect();
        let output_txids: HashSet<Txid> = coins.iter().map(|c| c.outpoint.txid).collect();
        map.retain(|txid, rb| {
            let all_inputs_still_present = rb
                .input_coins
                .iter()
                .all(|c| present_outpoints.contains(&c.outpoint));
            let no_output_observed = !output_txids.contains(txid);
            all_inputs_still_present && no_output_observed
        });
    }

    pub fn with_name(mut self, name: String) -> Self {
        self.name = name;
        self
    }

    pub fn with_alias(mut self, alias: Option<String>) -> Self {
        self.alias = alias;
        self
    }

    // To match with WalletSettings.wallet_id
    pub fn id(&self) -> WalletId {
        WalletId::new(self.descriptor_checksum.clone(), self.pinned_at)
    }

    pub fn with_pinned_at(mut self, pinned_at: Option<i64>) -> Self {
        self.pinned_at = pinned_at;
        self
    }

    pub fn with_key_aliases(mut self, aliases: HashMap<Fingerprint, String>) -> Self {
        self.keys_aliases = aliases;
        self
    }

    pub fn with_provider_keys(
        mut self,
        provider_keys: HashMap<Fingerprint, settings::ProviderKey>,
    ) -> Self {
        self.provider_keys = provider_keys;
        self
    }

    pub fn with_border_wallet_fingerprints(
        mut self,
        border_wallet_fingerprints: HashSet<Fingerprint>,
    ) -> Self {
        self.border_wallet_fingerprints = border_wallet_fingerprints;
        self
    }

    pub fn with_hardware_wallets(mut self, hardware_wallets: Vec<HardwareWalletConfig>) -> Self {
        self.hardware_wallets = hardware_wallets;
        self
    }

    pub fn with_signer(mut self, signer: Signer) -> Self {
        self.signer = Some(Arc::new(signer));
        self
    }

    pub fn descriptor_keys(&self) -> HashSet<Fingerprint> {
        let info = self.main_descriptor.policy();
        let mut descriptor_keys = HashSet::new();
        for (fingerprint, _) in info.primary_path().thresh_origins().1.iter() {
            descriptor_keys.insert(*fingerprint);
        }
        for (_, path_info) in info.recovery_paths().iter() {
            for (fingerprint, _) in path_info.thresh_origins().1.iter() {
                descriptor_keys.insert(*fingerprint);
            }
        }
        descriptor_keys
    }

    pub fn load_from_settings(self, wallet_settings: WalletSettings) -> Result<Self, WalletError> {
        if wallet_settings.descriptor_checksum != self.descriptor_checksum {
            Err(WalletError::WrongWalletLoaded)
        } else {
            Ok(self
                .with_key_aliases(wallet_settings.keys_aliases())
                .with_provider_keys(wallet_settings.provider_keys())
                .with_border_wallet_fingerprints(wallet_settings.border_wallet_fingerprints())
                .with_alias(wallet_settings.alias)
                .with_name(wallet_settings.name)
                .with_pinned_at(wallet_settings.pinned_at)
                .with_hardware_wallets(wallet_settings.hardware_wallets))
        }
    }

    pub fn load_hotsigners(
        self,
        datadir_path: &CoincubeDirectory,
        network: bitcoin::Network,
    ) -> Result<Self, WalletError> {
        // Load only Vault mnemonics, skip Liquid wallet mnemonics (managed by Breez SDK)
        let master_signers =
            match MasterSigner::from_datadir_vault_only(datadir_path.path(), network) {
                Ok(signers) => signers,
                Err(e) => match e {
                    coincube_core::signer::SignerError::MnemonicStorage(e) => {
                        if e.kind() == std::io::ErrorKind::NotFound {
                            Vec::new()
                        } else {
                            return Err(WalletError::MasterSigner(e.to_string()));
                        }
                    }
                    _ => return Err(WalletError::MasterSigner(e.to_string())),
                },
            };

        let curve = bitcoin::secp256k1::Secp256k1::signing_only();
        let keys = self.descriptor_keys();
        if let Some(master_signer) = master_signers
            .into_iter()
            .find(|s| keys.contains(&s.fingerprint(&curve)))
        {
            Ok(self.with_signer(Signer::new(master_signer)))
        } else {
            Ok(self)
        }
    }

    pub fn keys(&self) -> HashMap<Fingerprint, settings::KeySetting> {
        let mut map = HashMap::new();
        self.keys_aliases.iter().for_each(|(fg, alias)| {
            map.insert(
                *fg,
                settings::KeySetting {
                    name: alias.clone(),
                    master_fingerprint: *fg,
                    provider_key: None,
                    is_border_wallet: self.border_wallet_fingerprints.contains(fg),
                },
            );
        });

        self.provider_keys.iter().for_each(|(fg, key)| {
            if let Some(entry) = map.get_mut(fg) {
                entry.provider_key = Some(key.clone())
            }
        });

        map
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum WalletError {
    WrongWalletLoaded,
    Settings(settings::SettingsError),
    MasterSigner(String),
    BorderWallet(String),
}

impl std::fmt::Display for WalletError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::WrongWalletLoaded => write!(f, "Wrong wallet was loaded"),
            Self::Settings(e) => write!(f, "Failed to load settings: {}", e),
            Self::MasterSigner(e) => write!(f, "Failed to load master signer: {}", e),
            Self::BorderWallet(e) => write!(f, "Border wallet signing failed: {}", e),
        }
    }
}

impl From<settings::SettingsError> for WalletError {
    fn from(error: settings::SettingsError) -> Self {
        WalletError::Settings(error)
    }
}

/// The sync status of a wallet with respect to the blockchain.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncStatus {
    /// Wallet and blockchain are fully synced.
    Synced,
    /// Wallet is performing a full scan of the blockchain.
    WalletFullScan,
    /// Wallet is syncing with latest transactions.
    LatestWalletSync,
    /// Blockchain is syncing with given progress between 0.0 and 1.0.
    BlockchainSync(f64),
}

impl SyncStatus {
    pub fn is_synced(&self) -> bool {
        self == &SyncStatus::Synced
    }

    /// Whether the wallet itself, and not the blockchain, is syncing.
    pub fn wallet_is_syncing(&self) -> bool {
        self == &SyncStatus::WalletFullScan || self == &SyncStatus::LatestWalletSync
    }
}

/// Get the [`SyncStatus`].
///
/// The `last_poll_at_startup` is the timestamp of the last poll
/// of the blockchain when the application was first loaded, while
/// `last_poll` refers to the most recent poll.
///
/// `sync_progress` is the blockchain synchronization progress as
/// a number between `0.0` and `1.0`.
pub fn sync_status(
    daemon_backend: DaemonBackend,
    blockheight: i32,
    sync_progress: f64,
    last_poll: Option<u32>,
    last_poll_at_startup: Option<u32>,
) -> SyncStatus {
    if sync_progress < 1.0 {
        return SyncStatus::BlockchainSync(sync_progress);
    } else if blockheight <= 0 {
        // If blockheight <= 0, then this is a newly created wallet.
        // If user imported descriptor and is using a local bitcoind, a rescan
        // will need to be performed in order to see past transactions and so the
        // syncing status could be misleading as it could suggest the rescan is
        // being performed.
        // For external daemon or if we otherwise don't know the node type,
        // treat it the same as bitcoind to be sure we don't mislead the user.
        if daemon_backend == DaemonBackend::RemoteBackend
            || daemon_backend == DaemonBackend::EmbeddedCoincubed(Some(NodeType::Electrum))
        {
            return SyncStatus::WalletFullScan;
        }
    }
    // For an existing wallet with any local node type, if the first poll has
    // not completed, then the wallet has not yet caught up with the tip.
    // An existing wallet with remote backend remains synced so we can ignore it.
    // If external daemon, we cannot be sure it will return last poll as it
    // depends on the version, so assume it won't unless the last poll at
    // startup is set.
    // TODO: should we check the daemon version at GUI startup?
    else if last_poll <= last_poll_at_startup
        && (daemon_backend.is_embedded()
            || (daemon_backend == DaemonBackend::ExternalCoincubed
                && last_poll_at_startup.is_some()))
    {
        return SyncStatus::LatestWalletSync;
    }
    SyncStatus::Synced
}
