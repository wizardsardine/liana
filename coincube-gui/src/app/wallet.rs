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

/// The default *display name* for a wallet the user never named:
/// `Coincube-<descriptor checksum>`.
///
/// **Not a vault identity.** The checksum half is the descriptor's BIP-380
/// checksum, whose real job is keying local storage (wallet directories,
/// [`crate::app::settings::WalletId`], the `mnemonic-…` files in
/// `crate::signer`). It was once rendered on the home cube list as if it
/// identified the vault, which is what
/// `plans/PLAN-vault-identity-unification.md` set out to fix: a vault's human
/// identity is its Cube's name, and its technical identity is
/// [`descriptor_id_fingerprint`]. Do not present this string — or the bare
/// checksum — as a vault id again.
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

/// The vault's technical identity: `sha256(descriptor)[..4]`, rendered as 8
/// lowercase hex. Same descriptor → same id; distinct from any individual
/// signer's BIP-32 master fingerprint.
///
/// This is the value shown in Vault settings, sent in the pairing QR as `wfp`,
/// displayed by Keychain, and persisted into
/// [`crate::app::settings::CubeSettings::vault_fingerprint`]. It is a free
/// function rather than only a [`Wallet`] method because the sites that must
/// assert it — the installer's Connect vault registration, and the
/// `CubeSettings` write the home cube list reads back — hold a descriptor but
/// no `Wallet` (`plans/PLAN-vault-identity-unification.md` D1/D3).
pub fn descriptor_id_fingerprint(main_descriptor: &CoincubeDescriptor) -> Fingerprint {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(main_descriptor.to_string().as_bytes());
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&digest[..4]);
    Fingerprint::from(bytes)
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

    /// Stable 4-byte identifier derived from the wallet's descriptor.
    /// Unique per vault (same descriptor → same id), distinct from
    /// any individual signer's BIP-32 master fingerprint in
    /// [`Self::descriptor_keys`].
    ///
    /// Used by the local-signer pairing flow to identify the **vault
    /// as a whole** in the QR offer and on the persisted
    /// `PairedPhone`. A multisig wallet has no canonical BIP-32
    /// "master fingerprint" — picking one of its key fingerprints
    /// would conflate the wallet with one of its signers — so the
    /// identifier is hashed from the descriptor string itself.
    pub fn id_fingerprint(&self) -> Fingerprint {
        descriptor_id_fingerprint(&self.main_descriptor)
    }

    pub fn descriptor_keys(&self) -> HashSet<Fingerprint> {
        let info = self.main_descriptor.policy();
        let mut descriptor_keys = HashSet::new();
        for fingerprint in info.primary_path().thresh_origins().1.keys() {
            descriptor_keys.insert(*fingerprint);
        }
        for path_info in info.recovery_paths().values() {
            for fingerprint in path_info.thresh_origins().1.keys() {
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

    /// Load the Vault's **hot signer** — the seed behind a "this computer" key
    /// — so the wallet can sign PSBTs with it.
    ///
    /// # Why this takes credentials
    ///
    /// It used to call `MasterSigner::from_datadir_vault_only`, which passes
    /// `password: None`, and `from_datadir_with_password_filtered` *skips every
    /// encrypted file* when it has no password. Since seed hardening (I5) made
    /// `store_encrypted` the only way to write a mnemonic, every hot-signer
    /// seed on disk is encrypted — so the loop matched nothing, `self.signer`
    /// stayed `None`, and a Vault whose descriptor contains a hot key could not
    /// sign with it. The installer wrote the seed and nothing ever read it
    /// back. That was true for PIN Cubes and passkey Cubes alike.
    ///
    /// `password` therefore comes from [`crate::app::session::seed_file_password`]
    /// — the same definition the installer encrypts under, which is what makes
    /// the two halves meet.
    ///
    /// # Why not `from_datadir_by_fingerprint`
    ///
    /// Because `coincube-core` has no keystore access and answers
    /// `DeviceSecretRequired` for every `ENCRYPTED_V3` file — and
    /// `migrate_seed_files` promotes a PIN Cube's *hot-signer* files to v3
    /// along with its master seed. [`crate::services::unlock::open_seed_by_fingerprint`]
    /// supplies the device secret, and handles v2 (a passkey Cube, which has
    /// none) and pre-hardening plaintext on the same path.
    ///
    /// # Cost
    ///
    /// One Argon2id pass (~831 ms) per *matching* file, and a filename must
    /// carry the fingerprint to match at all — so a Vault with no hot key pays
    /// only a directory scan, and one with a hot key pays once per load. The
    /// session cache is tried first, which covers the developer-mode case where
    /// the Vault key *is* the Cube master seed already in hand.
    ///
    /// A `None` password is not an error: a watch-only Vault, or a load with no
    /// session (tests, some restore paths), keeps today's behaviour of reading
    /// only unencrypted files. It never fails the load — a Vault without its
    /// hot signer still watches, receives, and builds PSBTs for other keys.
    pub fn load_hotsigners(
        self,
        datadir_path: &CoincubeDirectory,
        network: bitcoin::Network,
        cube_id: &str,
        password: Option<&str>,
    ) -> Result<Self, WalletError> {
        let keys = self.descriptor_keys();

        let Some(password) = password else {
            return self.load_unencrypted_hotsigners(datadir_path, network, &keys);
        };

        for fingerprint in &keys {
            // Free when it hits: the signer the unlock already decrypted, with
            // no Argon2 pass. Only answers for this Cube and this exact key.
            if let Some(signer) = crate::app::session::unlocked_signer(cube_id, *fingerprint) {
                return Ok(self.with_signer(Signer::new(signer)));
            }

            match crate::services::unlock::open_seed_by_fingerprint(
                datadir_path.path(),
                network,
                *fingerprint,
                password,
                cube_id,
            ) {
                Ok(signer) => return Ok(self.with_signer(Signer::new(signer))),
                // Expected for every descriptor key that is not a hot key —
                // hardware wallets, Keychain cosigners, a Contact's xpub. There
                // is no seed on this machine for those and never will be.
                Err(coincube_core::signer::SignerError::SignerNotFound(_)) => continue,
                Err(e) => {
                    // A seed file for this key exists and would not open. Not
                    // fatal — the rest of the Vault works — but it means this
                    // Cube cannot sign with a key it believes it holds, so it
                    // must not pass silently.
                    tracing::warn!(
                        %fingerprint,
                        error = %e,
                        "a Vault hot-signer seed exists for this key but would not open"
                    );
                    continue;
                }
            }
        }

        Ok(self)
    }

    /// The no-credentials arm of [`Self::load_hotsigners`]: pre-hardening
    /// plaintext seed files only, exactly as before. Kept as its own function so
    /// the encrypted path never silently degrades into it.
    fn load_unencrypted_hotsigners(
        self,
        datadir_path: &CoincubeDirectory,
        network: bitcoin::Network,
        keys: &HashSet<Fingerprint>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const DESC: &str = "wsh(or_d(multi(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[de6eb005/48'/1'/0'/2']tpubDFGuYfS2JwiUSEXiQuNGdT3R7WTDhbaE6jbUhgYSSdhmfQcSx7ZntMPPv7nrkvAqjpj3jX9wbhSGMeKVao4qAzhbNyBi7iQmv5xxQk6H6jz/<0;1>/*),and_v(v:pkh([ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*),older(3))))#p9ax3xxp";

    /// `id_fingerprint` must be deterministic: the same descriptor
    /// produces the same 4-byte identifier across constructions.
    #[test]
    fn id_fingerprint_is_stable_per_descriptor() {
        let a = Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap());
        let b = Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap());
        assert_eq!(a.id_fingerprint(), b.id_fingerprint());
    }

    /// A Vault whose descriptor names a **hot key** must be able to sign with
    /// it, and the seed for it is encrypted on disk like every other (I5).
    ///
    /// The regression this pins: `load_hotsigners` used to call
    /// `from_datadir_vault_only`, which passes no password and therefore skips
    /// every encrypted file. The installer wrote the seed, nothing read it
    /// back, and `wallet.signer` was `None` on a Vault that owned a key.
    #[test]
    fn an_encrypted_hot_signer_is_loaded_with_the_cubes_credential() {
        use coincube_core::miniscript::bitcoin::bip32::DerivationPath;

        let secp = bitcoin::secp256k1::Secp256k1::signing_only();
        let net = Network::Testnet;
        let pin = "1234";
        let cube_id = format!("cube-hotsigner-{}", std::process::id());
        let root = std::env::temp_dir().join(format!(
            "coincube-hotsigner-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let signer = MasterSigner::generate(net).unwrap();
        let fp = signer.fingerprint(&secp);
        let path = DerivationPath::from_str("48'/1'/0'/2'").unwrap();
        let xpub = signer.xpub_at(&path, &secp);
        let descriptor = CoincubeDescriptor::from_str(&format!(
            "wsh(or_d(pk([{fp}/48'/1'/0'/2']{xpub}/<0;1>/*),\
             and_v(v:pkh([{fp}/48'/1'/0'/2']{xpub}/<2;3>/*),older(3))))"
        ))
        .expect("a descriptor built from the hot signer must parse");

        signer
            .store_encrypted(
                &root,
                net,
                &secp,
                Some(("hotsigner1000".to_string(), 1000)),
                pin,
                &cube_id,
                None,
            )
            .unwrap();

        let dir = CoincubeDirectory::new(root.clone());

        let loaded = Wallet::new(descriptor.clone())
            .load_hotsigners(&dir, net, &cube_id, Some(pin))
            .unwrap();
        assert_eq!(
            loaded.signer.as_ref().map(|s| s.fingerprint()),
            Some(fp),
            "the Vault's own hot signer must load with the Cube's credential"
        );

        // No credential (watch-only load, no session): the encrypted file stays
        // shut. Not an error — the Vault still loads, it just cannot sign.
        let none = Wallet::new(descriptor.clone())
            .load_hotsigners(&dir, net, &cube_id, None)
            .unwrap();
        assert!(none.signer.is_none());

        // A wrong credential must not hand back *some other* signer, and must
        // not fail the load either.
        let wrong = Wallet::new(descriptor)
            .load_hotsigners(&dir, net, &cube_id, Some("9999"))
            .unwrap();
        assert!(wrong.signer.is_none());

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Regression for the bug where the local-signer panel surfaced
    /// one of the *signer* fingerprints as the wallet identifier on
    /// a multisig vault. The vault id MUST NOT collide with any
    /// individual signer's master fingerprint from `descriptor_keys`.
    #[test]
    fn id_fingerprint_is_distinct_from_descriptor_keys() {
        let wallet = Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap());
        let id = wallet.id_fingerprint();
        let keys = wallet.descriptor_keys();
        assert!(!keys.is_empty(), "test fixture should have signer keys");
        assert!(
            !keys.contains(&id),
            "vault id {} collided with a signer key fingerprint in {:?}",
            id,
            keys,
        );
    }
}
