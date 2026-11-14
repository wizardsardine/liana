pub mod assets;
mod client;
mod config;
pub mod swap_status;

pub use client::BreezClient;
pub use config::BreezConfig;

// Re-export Breez SDK response types
pub use breez_sdk_liquid::prelude::{GetInfoResponse, ReceivePaymentResponse, SendPaymentResponse};

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, Network};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::app::seed_source::SeedSource;

#[derive(Debug, Clone)]
pub enum BreezError {
    NetworkNotSupported(Network),
    Connection(String),
    Sdk(String),
    SignerNotFound(Fingerprint),
    SignerError(String),
}

impl std::fmt::Display for BreezError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BreezError::NetworkNotSupported(n) => {
                write!(f, "Liquid wallet is not supported on {} network", n)
            }
            BreezError::Connection(msg) => write!(f, "failed to connect Breez SDK: {}", msg),
            BreezError::Sdk(msg) => write!(f, "SDK request failed: {}", msg),
            BreezError::SignerNotFound(fp) => {
                write!(f, "Liquid wallet signer not found for fingerprint: {}", fp)
            }
            BreezError::SignerError(msg) => write!(f, "Signer error: {}", msg),
        }
    }
}

impl std::error::Error for BreezError {}

/// The directories under the Breez working dir that hold a real, initialized
/// Liquid wallet — one per signer the SDK has connected on this machine.
///
/// Layout, created by the SDK itself (we only hand it `working_dir`):
/// `<datadir>/breez/<network>/<signer-id>/storage.sql`. The `storage.sql` file
/// is the load-bearing part: the SDK creates the enclosing directories eagerly,
/// so a bare directory proves nothing, whereas the DB only exists once a
/// wallet has actually been initialized.
///
/// This is the on-disk half of [`crate::app::features::LiquidGate`] — the
/// signal that says "this machine already has a Liquid wallet, so the sunset
/// gate must not hide it".
///
/// **Machine-scoped, not cube-scoped**, and deliberately so. The `<signer-id>`
/// segment is the SDK's own derivation, not our fingerprint, so we can't map a
/// subdir back to a cube without guessing at SDK internals. A machine with one
/// grandfathered Liquid cube will therefore also show Liquid on a second, newer
/// cube. That's the correct way to be wrong here: the failure mode is an extra
/// empty wallet in the nav, versus stranding real funds if we guessed the other
/// way and guessed wrong.
pub fn liquid_state_dirs(datadir: &Path, network: Network) -> Vec<std::path::PathBuf> {
    let network_dir = datadir.join("breez").join(match network {
        Network::Bitcoin => "mainnet",
        // Every other network maps to the SDK's Testnet (see `BreezConfig`).
        _ => "testnet",
    });
    let Ok(entries) = std::fs::read_dir(&network_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("storage.sql").is_file())
        .collect()
}

/// Whether a Liquid wallet has been initialized on this machine. The on-disk
/// input to the sunset gate — see [`liquid_state_dirs`].
///
/// Cheap (one `read_dir` + a `stat` per entry), synchronous, and needs no PIN,
/// no decryption and no SDK — so the gate can be resolved before the wallet is
/// open, and *offline*: the SDK's `storage.sql` caches balance and history
/// locally, which is what lets a funded wallet still surface when Connect (or
/// the whole network) is unreachable.
pub fn local_state_exists(datadir: &Path, network: Network) -> bool {
    !liquid_state_dirs(datadir, network).is_empty()
}

/// Delete Liquid working directories that turned out to hold nothing.
///
/// The sunset gate connects Liquid for everyone — including a fresh install
/// that hasn't been granted it — because that's the only way to answer "does
/// this seed have recoverable Liquid funds?" (the restore-from-seed case: no
/// local state yet, but real L-BTC on chain). Connecting creates a working dir,
/// which would then make [`local_state_exists`] true for *every* new user and
/// quietly defeat the gate. So a wallet that scans clean has its scratch dir
/// discarded, restoring the fresh-install state.
///
/// Only ever called with directories that did **not** exist before this
/// connect (the caller diffs [`liquid_state_dirs`] across the connect), so a
/// grandfathered wallet's data is never a candidate — and only for wallets
/// observed to have zero balance and zero history, so nothing recoverable is
/// destroyed. A failed delete is logged and ignored: the cost is a spurious
/// Liquid entry in the nav, which is the safe direction to fail.
pub fn discard_empty_liquid_state(dirs: &[std::path::PathBuf]) {
    for dir in dirs {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => log::info!("Discarded empty Liquid scratch state at {}", dir.display()),
            Err(e) => log::warn!(
                "Could not discard empty Liquid scratch state at {}: {e}",
                dir.display()
            ),
        }
    }
}

/// Load BreezClient for a Cube, taking its master seed from `seed`.
///
/// `seed` is an explicit [`SeedSource`] rather than a `(fingerprint, password)`
/// pair because a passkey Cube has no seed file at all — its master seed is
/// re-derived from a WebAuthn PRF assertion at unlock and only ever exists in
/// memory. [`SeedSource::EncryptedFile`] is the unchanged PIN-Cube path,
/// session-cache fast path included.
///
/// The master signer is always loaded — its mnemonic backs seed-derived
/// features (Spark, and P2P/Mostro identity) that work on networks where the
/// Liquid SDK doesn't. The Liquid SDK itself is only *connected* where it has
/// a usable backend (`crate::app::features::liquid` — mainnet only; the SDK
/// rejects `LiquidNetwork::Testnet` and regtest needs a localhost Esplora).
/// On every other network it returns a disconnected client that still carries
/// the signer, so the Liquid wallet UI stays gated without taking down P2P.
/// `liquid_granted` is the account's last-seen `liquidEnabled` grant, read from
/// [`crate::app::settings::CubeSettings::liquid_granted`] — a *persisted* copy,
/// because the live one isn't known this early (Connect signs in after the cube
/// opens, and may not exist at all). It only ever forces the wallet to be kept;
/// it can never hide one.
pub async fn load_breez_client(
    datadir: &Path,
    network: Network,
    seed: SeedSource<'_>,
    cube_id: &str,
    liquid_granted: bool,
) -> Result<Arc<BreezClient>, BreezError> {
    let liquid_supported = crate::app::features::liquid(network).is_available();
    let master_signer_fingerprint = seed.fingerprint();

    // `SeedSource::resolve` prefers the signer the unlock already decrypted.
    // Verifying the PIN is a full Argon2id pass over the seed file, so
    // re-reading it here would pay ~831 ms for a result we are already
    // holding. It falls back to disk whenever the session has nothing for this
    // Cube+fingerprint — every other entry point (restore, login, tests) takes
    // that path unchanged.
    let liquid_signer = match seed.resolve(datadir, network, cube_id) {
        Ok(signer) => Arc::new(Mutex::new(signer)),
        Err(e) => {
            let mapped = match e {
                coincube_core::signer::SignerError::MnemonicStorage(io_err)
                    if io_err.kind() == std::io::ErrorKind::NotFound =>
                {
                    BreezError::SignerNotFound(master_signer_fingerprint)
                }
                coincube_core::signer::SignerError::SignerNotFound(fingerprint) => {
                    BreezError::SignerNotFound(fingerprint)
                }
                _ => BreezError::SignerError(e.to_string()),
            };
            // A genuinely *absent* signer (seed-less / watch-only cube) is not
            // fatal on a network where Liquid isn't connected — degrade to a
            // signer-less disconnected client so the cube still loads. Every
            // other failure (wrong PIN, decryption, IO errors) propagates so
            // it surfaces as a real error rather than being silently masked.
            if !liquid_supported && matches!(mapped, BreezError::SignerNotFound(_)) {
                log::info!(
                    "No master signer for disconnected cube on {network}: {mapped}; \
                     using a signer-less disconnected client"
                );
                return Ok(Arc::new(BreezClient::disconnected(network)));
            }
            return Err(mapped);
        }
    };

    // Liquid is only enabled where a real Liquid Esplora backend exists (see
    // `features::liquid`, the single source of truth for the gate). On
    // unsupported networks return a disconnected client that still carries
    // the signer — the Liquid UI stays gated (rail greyed, panels show
    // disconnected) while the mnemonic remains available to P2P/Spark.
    if !liquid_supported {
        return Ok(Arc::new(BreezClient::disconnected_with_signer(
            network,
            liquid_signer,
        )));
    }

    // Create Breez config
    let breez_config = BreezConfig::from_env(network, datadir)?;

    // Snapshot the wallets already on disk *before* connecting, so we can tell
    // a pre-existing (grandfathered) wallet's working dir apart from one this
    // connect is about to create. Only the latter is ever a discard candidate.
    let pre_existing = liquid_state_dirs(datadir, network);
    let already_had_liquid = !pre_existing.is_empty();

    // Connect to Breez SDK with the signer
    let breez_client =
        BreezClient::connect_with_signer(breez_config, liquid_signer.clone()).await?;

    // Liquid sunset: a wallet that already exists on this machine, or an
    // account the server has grandfathered, keeps Liquid unconditionally.
    if already_had_liquid || liquid_granted {
        return Ok(Arc::new(breez_client));
    }

    // Neither — so this is a fresh, ungranted wallet, and Liquid should be
    // hidden. But we can't just skip connecting: a restore-from-seed onto a
    // fresh machine has no local state either, and refusing to connect would
    // strand recoverable L-BTC/L-USDt (the more so because the manual server
    // grant can't help someone who never made a Connect account). So we connect
    // anyway and *ask the chain* whether this seed has any Liquid history.
    if seed_has_liquid_activity(&breez_client).await {
        log::info!("Liquid activity found for this seed — surfacing the wallet despite the gate");
        return Ok(Arc::new(breez_client));
    }

    // Nothing to recover. Discard the working dir this connect just created,
    // which restores the fresh-install state — otherwise every new user would
    // end up with a `storage.sql` on disk and `local_state_exists` would be
    // true for everyone, silently defeating the gate.
    log::info!("No Liquid activity for this seed and no grant — hiding Liquid");
    breez_client.disconnect().await;
    let scratch: Vec<_> = liquid_state_dirs(datadir, network)
        .into_iter()
        .filter(|dir| !pre_existing.contains(dir))
        .collect();
    discard_empty_liquid_state(&scratch);

    Ok(Arc::new(BreezClient::disconnected_with_signer(
        network,
        liquid_signer,
    )))
}

/// One sunset-gate probe. Failure is not "empty": the gate keeps the wallet
/// unless it has positively seen no funds and no history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Probe<T> {
    Known(T),
    Failed,
}

/// The balance fields the sunset gate reads. `balance_sat` is L-BTC;
/// `any_asset_balance` is L-USDt or any other issued asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BalanceSnapshot {
    balance_sat: u64,
    pending_receive_sat: u64,
    pending_send_sat: u64,
    any_asset_balance: bool,
}

fn any_positive_sat(balances: impl IntoIterator<Item = u64>) -> bool {
    balances.into_iter().any(|sat| sat > 0)
}

fn balance_snapshot_shows_activity(snapshot: &BalanceSnapshot) -> bool {
    snapshot.balance_sat > 0
        || snapshot.pending_receive_sat > 0
        || snapshot.pending_send_sat > 0
        || snapshot.any_asset_balance
}

/// Whether the two probes are enough to keep Liquid visible.
///
/// `history_nonempty` is `None` when the balance probe already decided, and a
/// history result only when that balance was a known zero. A missing or failed
/// history answer keeps the wallet: discarding is allowed only for an explicit
/// empty history.
fn seed_activity_worth_keeping(
    balance: Probe<BalanceSnapshot>,
    history_nonempty: Option<Probe<bool>>,
) -> bool {
    let balance_is_clear = match balance {
        Probe::Failed => return true,
        Probe::Known(snapshot) => !balance_snapshot_shows_activity(&snapshot),
    };
    if !balance_is_clear {
        return true;
    }
    match history_nonempty {
        Some(Probe::Known(nonempty)) => nonempty,
        Some(Probe::Failed) | None => true,
    }
}

/// Whether this seed has anything worth surfacing on Liquid — any balance, or
/// any payment in its history. Used by the sunset gate to decide whether a
/// fresh wallet is a restore with real funds (keep it) or just an empty
/// wallet a new user never asked for (discard it).
///
/// Errors count as "yes". A failed probe means we don't *know* the wallet is
/// empty, and the cost of the two mistakes is wildly asymmetric: wrongly
/// keeping an empty wallet shows a spurious nav entry, while wrongly
/// discarding a funded one hides the user's money.
async fn seed_has_liquid_activity(client: &BreezClient) -> bool {
    let balance = match client.info().await {
        // `balance_sat` covers L-BTC; `asset_balances` covers L-USDt and any
        // other issued asset, which a pure L-BTC check would miss.
        Ok(info) => {
            let wallet = &info.wallet_info;
            Probe::Known(BalanceSnapshot {
                balance_sat: wallet.balance_sat,
                pending_receive_sat: wallet.pending_receive_sat,
                pending_send_sat: wallet.pending_send_sat,
                any_asset_balance: any_positive_sat(
                    wallet.asset_balances.iter().map(|a| a.balance_sat),
                ),
            })
        }
        Err(e) => {
            log::warn!("Liquid balance probe failed ({e}) — keeping the wallet to be safe");
            Probe::Failed
        }
    };
    // A zero balance isn't proof of an unused wallet: a user who swept their
    // L-BTC out still has history worth reaching. One payment is enough.
    // History is skipped when the balance probe already decided.
    let history = if matches!(
        &balance,
        Probe::Known(snapshot) if !balance_snapshot_shows_activity(snapshot)
    ) {
        Some(match client.list_payments(Some(1), None, None).await {
            Ok(payments) => Probe::Known(!payments.is_empty()),
            Err(e) => {
                log::warn!("Liquid history probe failed ({e}) — keeping the wallet to be safe");
                Probe::Failed
            }
        })
    } else {
        None
    };
    seed_activity_worth_keeping(balance, history)
}

#[cfg(test)]
mod seed_source_tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::bip32::DerivationPath;
    use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;
    use coincube_core::signer::MasterSigner;
    use std::str::FromStr;

    /// A fresh datadir under the system temp dir. `tempfile` isn't a dependency
    /// of this crate; the same hand-rolled pattern is used in
    /// `phone_signer::pairing_store`.
    fn scratch_datadir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "coincube-seed-source-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The equivalence PR A exists to prove: a signer handed in as
    /// `SeedSource::InMemory` and the *same* seed read back out of its
    /// encrypted file produce the same wallet.
    ///
    /// Compares **xpubs**, not mnemonics — the same rule the two-machine
    /// acceptance gate runs under. Two different encodings of one key would
    /// compare unequal as words and equal as xpubs, and it is the xpub that
    /// decides whether the user's coins are there.
    ///
    /// Runs on testnet so `features::liquid` is unavailable and
    /// `load_breez_client` returns a disconnected client carrying the signer,
    /// with no Breez SDK, network or API key involved.
    #[tokio::test]
    async fn in_memory_and_encrypted_file_derive_the_same_xpubs() {
        let datadir = scratch_datadir("equivalence");
        let network = Network::Testnet;
        let cube_id = "cube-equivalence";
        let pin = "1234";
        let secp = Secp256k1::signing_only();

        let signer = MasterSigner::generate(network).unwrap();
        let fingerprint = signer.fingerprint(&secp);
        // v2 file (no device secret), so the read below needs only the PIN.
        signer
            .store_encrypted(&datadir, network, &secp, None, pin, cube_id, None)
            .unwrap();

        let from_file = load_breez_client(
            &datadir,
            network,
            SeedSource::encrypted_file(fingerprint, pin),
            cube_id,
            false,
        )
        .await
        .expect("the encrypted-file arm loads");

        let from_memory = load_breez_client(
            &datadir,
            network,
            SeedSource::in_memory(Arc::new(signer.try_clone().unwrap())),
            cube_id,
            false,
        )
        .await
        .expect("the in-memory arm loads");

        // BIP-84 account 0 — an arbitrary but real derivation, so this compares
        // derived key material rather than just the master fingerprint.
        let path = DerivationPath::from_str("m/84'/0'/0'").unwrap();
        let xpub_of = |client: &Arc<BreezClient>| {
            let arc = client
                .liquid_signer()
                .expect("a disconnected client still carries its signer");
            let guard = arc.lock().unwrap();
            guard.xpub_at(&path, &secp)
        };

        assert_eq!(
            xpub_of(&from_file),
            xpub_of(&from_memory),
            "the two SeedSource arms must produce the same wallet"
        );
        assert_eq!(
            xpub_of(&from_file),
            signer.xpub_at(&path, &secp),
            "and both must match the seed they were built from"
        );

        let _ = std::fs::remove_dir_all(&datadir);
    }

    /// Restore completion opens a PIN-only session before loading this client.
    /// Exercise that actual loader with a v3 seed and no cached master, rather
    /// than testing only the lower-level unlock helper.
    // The guard deliberately spans the client load: the global session must
    // stay PIN-only until this async restore-completion path finishes.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "needs a code-signed binary (data-protection keychain returns -34018 unsigned)"
    )]
    async fn restored_v3_pin_only_session_loads_without_cached_signer() {
        use crate::app::session;
        use crate::services::unlock::device_secret;
        use coincube_core::signer::SignerError;

        struct Fixture {
            root: std::path::PathBuf,
            cube_id: String,
            other_id: String,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                session::close();
                device_secret::delete(&self.cube_id).expect("remove disposable device secret");
                device_secret::delete(&self.other_id).expect("remove other disposable secret");
                std::fs::remove_dir_all(&self.root).expect("remove disposable seed files");
            }
        }

        let _guard = session::test_guard();
        for network in [Network::Bitcoin, Network::Testnet4] {
            let fixture = Fixture {
                root: scratch_datadir("restored-v3"),
                cube_id: format!("seed-source-restore-{}", uuid::Uuid::new_v4()),
                other_id: format!("seed-source-other-{}", uuid::Uuid::new_v4()),
            };
            session::close();
            let pin = "9753";
            let secp = Secp256k1::signing_only();
            let master = MasterSigner::generate(network).unwrap();
            let fp = master.fingerprint(&secp);
            let secret = device_secret::get_or_create(&fixture.root, &fixture.cube_id).unwrap();
            master
                .store_encrypted(
                    &fixture.root,
                    network,
                    &secp,
                    None,
                    pin,
                    &fixture.cube_id,
                    Some(&secret),
                )
                .unwrap();
            // Core cannot obtain the device secret; routing here recreates the
            // pre-fix GUI failure, even though the Keychain item exists.
            assert!(matches!(
                MasterSigner::from_datadir_by_fingerprint(
                    &fixture.root,
                    network,
                    fp,
                    Some(pin),
                    &fixture.cube_id,
                ),
                Err(SignerError::DeviceSecretRequired)
            ));
            session::open(&fixture.cube_id, zeroize::Zeroizing::new(pin.into()));
            assert!(session::unlocked_signer(&fixture.cube_id, fp).is_none());
            let source = SeedSource::encrypted_file(fp, pin);
            let path = DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();
            assert_eq!(
                source
                    .resolve(&fixture.root, network, &fixture.cube_id)
                    .expect("restore must load the v3 root without a restart")
                    .xpub_at(&path, &secp),
                master.xpub_at(&path, &secp),
            );
            // Testnet4 follows the same restore-completion loader without a
            // live mainnet Liquid SDK or API credential dependency.
            if network == Network::Testnet4 {
                let client = load_breez_client(
                    &fixture.root,
                    network,
                    source.clone(),
                    &fixture.cube_id,
                    false,
                )
                .await
                .expect("restore completion must carry the restored v3 signer");
                assert_eq!(
                    client
                        .liquid_signer()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .xpub_at(&path, &secp),
                    master.xpub_at(&path, &secp),
                );
            }
            assert!(session::unlocked_signer(&fixture.cube_id, fp).is_none());
            assert!(matches!(
                SeedSource::encrypted_file(fp, "0000").resolve(
                    &fixture.root,
                    network,
                    &fixture.cube_id
                ),
                Err(SignerError::InvalidPassword)
            ));
            device_secret::get_or_create(&fixture.root, &fixture.other_id).unwrap();
            assert!(matches!(
                source.resolve(&fixture.root, network, &fixture.other_id),
                Err(SignerError::InvalidPassword)
            ));
            assert!(matches!(
                SeedSource::encrypted_file(Fingerprint::default(), pin).resolve(
                    &fixture.root,
                    network,
                    &fixture.cube_id
                ),
                Err(SignerError::SignerNotFound(_))
            ));
            assert!(
                device_secret::load(&fixture.cube_id).unwrap() == secret,
                "loading must not replace the device secret",
            );
            device_secret::delete(&fixture.cube_id).unwrap();
            assert!(matches!(
                source.resolve(&fixture.root, network, &fixture.cube_id),
                Err(SignerError::DeviceSecretRequired)
            ));
            assert!(device_secret::load_optional(&fixture.cube_id)
                .unwrap()
                .is_none());
        }
    }

    /// The property that makes passkey unlock possible at all: the in-memory
    /// arm never touches the datadir, so a Cube with no seed file on disk still
    /// loads its wallets.
    #[tokio::test]
    async fn the_in_memory_arm_loads_with_no_seed_file_on_disk() {
        let datadir = scratch_datadir("no-seed-file");
        let network = Network::Testnet;
        let signer = MasterSigner::generate(network).unwrap();

        let client = load_breez_client(
            &datadir,
            network,
            SeedSource::in_memory(Arc::new(signer)),
            "cube-passkey",
            false,
        )
        .await
        .expect("a passkey Cube has no seed file and must still load");
        assert!(client.liquid_signer().is_some());

        let _ = std::fs::remove_dir_all(&datadir);
    }
}

#[cfg(test)]
mod sunset_gate_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn zero_balance() -> BalanceSnapshot {
        BalanceSnapshot {
            balance_sat: 0,
            pending_receive_sat: 0,
            pending_send_sat: 0,
            any_asset_balance: false,
        }
    }

    fn with_activity(mutate: impl FnOnce(&mut BalanceSnapshot)) -> Probe<BalanceSnapshot> {
        let mut snapshot = zero_balance();
        mutate(&mut snapshot);
        Probe::Known(snapshot)
    }

    /// Each funded field, on its own, keeps the wallet. History is not an input.
    #[test]
    fn funded_balances_keep_the_wallet_without_a_history_probe() {
        assert!(!any_positive_sat([0, 0]));
        assert!(any_positive_sat([0, 1]));

        let funded = [
            with_activity(|s| s.balance_sat = 1),
            with_activity(|s| s.pending_receive_sat = 1),
            with_activity(|s| s.pending_send_sat = 1),
            with_activity(|s| s.any_asset_balance = true),
        ];
        for balance in funded {
            assert!(
                seed_activity_worth_keeping(balance, None),
                "a funded probe must keep Liquid even with no history answer"
            );
            assert!(
                seed_activity_worth_keeping(balance, Some(Probe::Known(false))),
                "a funded probe must not be overturned by an empty history"
            );
        }
    }

    #[test]
    fn a_swept_wallet_stays_when_one_payment_remains() {
        assert!(seed_activity_worth_keeping(
            Probe::Known(zero_balance()),
            Some(Probe::Known(true))
        ));
    }

    /// Discarding is the exception. Every "we don't know" answer keeps the wallet,
    /// including a failed balance probe that also carries an empty history.
    #[test]
    fn only_a_known_empty_history_discards_the_wallet() {
        let clear = Probe::Known(zero_balance());
        assert!(!seed_activity_worth_keeping(
            clear,
            Some(Probe::Known(false))
        ));
        assert!(seed_activity_worth_keeping(clear, Some(Probe::Failed)));
        assert!(seed_activity_worth_keeping(clear, None));
        assert!(seed_activity_worth_keeping(
            Probe::Failed,
            Some(Probe::Known(false))
        ));
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "coincube-liquid-gate-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_wallet(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("storage.sql"), b"wallet").unwrap();
    }

    fn sorted(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
        dirs.sort();
        dirs
    }

    #[test]
    fn only_a_storage_sql_file_marks_liquid_state_and_networks_stay_apart() {
        let scratch = Scratch::new("state");
        let root = scratch.path();
        assert!(liquid_state_dirs(root, Network::Bitcoin).is_empty());
        assert!(!local_state_exists(root, Network::Bitcoin));
        assert!(!local_state_exists(&root.join("missing"), Network::Testnet));

        let bare = root.join("breez/mainnet/bare");
        std::fs::create_dir_all(&bare).unwrap();
        let sql_dir = root.join("breez/mainnet/sql-is-a-directory");
        std::fs::create_dir_all(sql_dir.join("storage.sql")).unwrap();
        let nested = root.join("breez/mainnet/nested/extra");
        write_wallet(&nested);
        assert!(
            liquid_state_dirs(root, Network::Bitcoin).is_empty(),
            "a bare directory, a storage.sql directory, or a nested file is not a wallet"
        );

        let mainnet = root.join("breez/mainnet/signer-a");
        write_wallet(&mainnet);
        assert_eq!(
            sorted(liquid_state_dirs(root, Network::Bitcoin)),
            vec![mainnet]
        );
        assert!(local_state_exists(root, Network::Bitcoin));
        assert!(
            liquid_state_dirs(root, Network::Testnet).is_empty(),
            "a mainnet wallet must not satisfy the testnet gate"
        );
        assert!(liquid_state_dirs(root, Network::Regtest).is_empty());

        let testnet = root.join("breez/testnet/signer-b");
        write_wallet(&testnet);
        assert_eq!(
            sorted(liquid_state_dirs(root, Network::Testnet)),
            vec![testnet.clone()]
        );
        assert_eq!(
            sorted(liquid_state_dirs(root, Network::Signet)),
            vec![testnet],
            "every non-mainnet network shares the SDK testnet directory"
        );
        assert!(local_state_exists(root, Network::Bitcoin));
    }

    #[test]
    fn discarding_scratch_state_leaves_every_other_wallet() {
        let scratch = Scratch::new("discard");
        let root = scratch.path();
        let grandfathered = root.join("breez/mainnet/grandfathered");
        let fresh = root.join("breez/mainnet/fresh");
        let testnet = root.join("breez/testnet/other");
        write_wallet(&grandfathered);
        write_wallet(&fresh);
        write_wallet(&testnet);

        discard_empty_liquid_state(std::slice::from_ref(&fresh));
        assert!(!fresh.exists());
        assert_eq!(
            std::fs::read(grandfathered.join("storage.sql")).unwrap(),
            b"wallet"
        );
        assert_eq!(
            std::fs::read(testnet.join("storage.sql")).unwrap(),
            b"wallet"
        );
        assert_eq!(
            sorted(liquid_state_dirs(root, Network::Bitcoin)),
            vec![grandfathered.clone()]
        );
        assert_eq!(
            sorted(liquid_state_dirs(root, Network::Testnet)),
            vec![testnet]
        );

        let stray_file = root.join("breez/mainnet/not-a-dir");
        std::fs::write(&stray_file, b"keep").unwrap();
        discard_empty_liquid_state(&[stray_file.clone(), root.join("breez/mainnet/missing")]);
        assert_eq!(std::fs::read(&stray_file).unwrap(), b"keep");
        assert!(grandfathered.join("storage.sql").is_file());
    }
}
