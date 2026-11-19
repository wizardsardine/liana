//! Restricted destination Vault admission for cross-Cube Transfer.
//! Neither a signer nor a PRF result enters an Iced message or the global
//! session. This module cannot sign, construct a source spend, or broadcast.

use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, Network};
use coincube_core::signer::MasterSigner;
use coincubed::config::Config;
use zeroize::Zeroizing;

use super::{
    cache::AppGeneration,
    settings::{CubeSettings, Settings, WalletId, WalletSettings},
    transfer_destination::{OwnedDestinationResource, Source, WalletIdentity},
    wallet::{descriptor_id_fingerprint, Wallet},
};
use crate::{
    chain::ChainId,
    daemon::{client, embedded::EmbeddedDaemon, Daemon},
    dir::CoincubeDirectory,
    services::unlock::{self, throttle::PinAttempt, PinOutcome},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    Stale,
    Unsupported,
    SameWallet,
    IdentityChanged,
    BackendUnavailable,
    SparkUnavailable,
    SameSparkWallet,
    /// The other Cube's daemon is still stopping after a lock.
    StillStopping,
    WrongCredential,
    Duress,
    Locked(Duration),
}

impl Refusal {
    /// What the Unlock page says about a refusal. Duress never reaches it.
    pub(crate) fn reason(self) -> String {
        match self {
            Self::Stale => {
                coincube_ui::locale::copy("This step expired. Start the unlock again.").into()
            }
            Self::Unsupported => {
                coincube_ui::locale::copy("This Cube can't receive a cross-Cube transfer.").into()
            }
            Self::SameWallet => {
                coincube_ui::locale::copy("That Cube holds the same Vault as this one.").into()
            }
            Self::IdentityChanged => {
                coincube_ui::locale::copy("The other Cube changed. Choose it again.").into()
            }
            Self::BackendUnavailable => {
                coincube_ui::locale::copy("The other Cube's receiving wallet isn't available.")
                    .into()
            }
            Self::SparkUnavailable => {
                coincube_ui::locale::copy("The other Cube's Spark wallet isn't available.").into()
            }
            Self::SameSparkWallet => {
                coincube_ui::locale::copy("That Cube holds the same Spark wallet as this one.")
                    .into()
            }
            Self::StillStopping => coincube_ui::locale::copy(
                "The other Cube is still finishing locking. Try again in a moment.",
            )
            .into(),
            Self::WrongCredential => {
                coincube_ui::locale::copy("That PIN or passkey didn't unlock it.").into()
            }
            Self::Duress => coincube_ui::locale::copy("The other Cube can't be used.").into(),
            Self::Locked(remaining) => coincube_ui::localized_format!(
                "Too many attempts. Try again in {} s.",
                "Demasiados intentos. Inténtalo de nuevo en {} s.",
                remaining.as_secs().max(1)
            ),
        }
    }
}

/// One exact local catalog snapshot. Re-read before and after every async
/// backend call; no caller-provided Cube label or address is trusted.
fn selected(
    root: &CoincubeDirectory,
    cube_id: &str,
    wallet_id: Option<&WalletId>,
) -> Result<(CubeSettings, Option<WalletSettings>), Refusal> {
    let settings = Settings::from_file(&root.network_directory(ChainId::Bitcoin))
        .map_err(|_| Refusal::BackendUnavailable)?;
    let cube = settings
        .cubes
        .into_iter()
        .find(|cube| cube.id == cube_id)
        .ok_or(Refusal::IdentityChanged)?;
    if cube.network != ChainId::Bitcoin || cube.master_signer_fingerprint.is_none() {
        return Err(Refusal::Unsupported);
    }
    let Some(wallet_id) = wallet_id else {
        return Ok((cube, None));
    };
    if receiving_wallet_id(&cube).as_ref() != Some(wallet_id) {
        return Err(Refusal::Unsupported);
    }
    let wallet = settings
        .wallets
        .into_iter()
        .find(|wallet| wallet.wallet_id() == *wallet_id)
        .ok_or(Refusal::IdentityChanged)?;
    Ok((cube, Some(wallet)))
}

/// The Vault that can receive a cross-Cube transfer in `cube`: a Vault with
/// its descriptor fingerprint recorded, or a fully scanned imported Vault.
fn receiving_wallet_id(cube: &CubeSettings) -> Option<WalletId> {
    match cube.bitcoin_import.as_deref() {
        None => cube.vault_wallet_id.clone().filter(|_| {
            cube.vault_fingerprint
                .as_deref()
                .is_some_and(|fp| !fp.is_empty())
        }),
        Some(import) if cube.vault_wallet_id.is_none() && import.is_ready() => import
            .active_attachment()
            .map(|wallet| wallet.wallet_id.clone()),
        Some(_) => None,
    }
}

/// One row of the Other Cube picker, read without unlocking anything.
/// Locked Cubes show wallet availability without exposing their balances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CubeChoice {
    pub cube_id: String,
    pub name: String,
    pub chain: ChainId,
    pub has_vault: bool,
    pub has_spark: bool,
    pub passkey: bool,
    pub vault_wallet_id: Option<WalletId>,
    pub refusal: Option<&'static str>,
}

impl CubeChoice {
    pub(crate) fn can_receive(&self, wallet: super::view::global_home::WalletKind) -> bool {
        use super::view::global_home::WalletKind;
        self.refusal.is_none()
            && match wallet {
                WalletKind::Vault => self.vault_wallet_id.is_some(),
                WalletKind::Spark => self.has_spark,
            }
    }
}

/// Every other local Cube on the source's chain; other networks can't receive.
pub(crate) fn other_cubes(root: &CoincubeDirectory, source: &Source) -> Vec<CubeChoice> {
    let mut choices: Vec<CubeChoice> = Settings::from_file(&root.network_directory(source.chain))
        .map(|settings| settings.cubes)
        .unwrap_or_default()
        .into_iter()
        .filter(|cube| cube.id != source.cube_id && cube.network == source.chain)
        .map(|cube| {
            // A Vault, or an imported Vault (once its scan has finished).
            let has_vault = cube.vault_wallet_id.is_some() || cube.has_imported_bitcoin();
            let receiving = receiving_wallet_id(&cube);
            let shares_source = receiving.as_ref().is_some_and(|id| {
                source
                    .wallet
                    .shares_vault_descriptor(&WalletIdentity::Vault(id.clone()))
            });
            let has_spark =
                cube.network == ChainId::Bitcoin && cube.master_signer_fingerprint.is_some();
            let vault_ready =
                receiving.is_some() && !shares_source && cube.master_signer_fingerprint.is_some();
            let refusal = if has_spark || vault_ready {
                None
            } else if !has_vault {
                Some(coincube_ui::locale::copy(
                    "No Vault to receive this transfer",
                ))
            } else if shares_source {
                Some(coincube_ui::locale::copy("Same Vault as the source"))
            } else if cube.master_signer_fingerprint.is_none() || receiving.is_none() {
                Some(coincube_ui::locale::copy(
                    "Not ready for a cross-Cube transfer",
                ))
            } else {
                None
            };
            CubeChoice {
                name: crate::cube_display::name(&cube.name),
                chain: cube.network,
                has_vault,
                has_spark,
                passkey: cube.is_passkey_cube(),
                vault_wallet_id: receiving.filter(|_| vault_ready),
                cube_id: cube.id,
                refusal,
            }
        })
        .collect();
    choices.sort_by_key(|choice| choice.refusal.is_some());
    choices
}

fn same_selection(expected: &CubeSettings, current: &CubeSettings) -> bool {
    expected.id == current.id
        && expected.network == current.network
        && expected.vault_wallet_id == current.vault_wallet_id
        && expected.vault_fingerprint == current.vault_fingerprint
        && expected.master_signer_fingerprint == current.master_signer_fingerprint
        && expected.duress_slot_file == current.duress_slot_file
        && expected
            .passkey_metadata
            .as_ref()
            .map(|meta| (&meta.rp_id, &meta.credential_id))
            == current
                .passkey_metadata
                .as_ref()
                .map(|meta| (&meta.rp_id, &meta.credential_id))
}

fn operation_live(
    expected_source: &Source,
    current_source: &Source,
    expected_generation: AppGeneration,
    result_generation: AppGeneration,
    cancelled: &AtomicBool,
) -> bool {
    !cancelled.load(Ordering::Acquire)
        && expected_source == current_source
        && expected_generation == result_generation
}

pub(crate) struct ScopedDestination {
    root: CoincubeDirectory,
    source: Source,
    cube: CubeSettings,
    /// `None` selects the receiving Cube's seed-backed Spark wallet.
    wallet_id: Option<WalletId>,
    generation: AppGeneration,
    pin_attempt: Option<AppGeneration>,
    cancelled: Arc<AtomicBool>,
    pin_busy: Arc<AtomicBool>,
    pin_signer: Arc<Mutex<Option<MasterSigner>>>,
    verified: bool,
    #[cfg(target_os = "macos")]
    ceremony: Option<crate::services::passkey::macos::NativePasskeyCeremony>,
    backend: Option<BoundBackend>,
    spark_cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ScopedDestination {
    pub(crate) fn new(
        root: CoincubeDirectory,
        source: Source,
        destination_cube_id: &str,
        wallet_id: impl Into<Option<WalletId>>,
    ) -> Result<Self, Refusal> {
        if source.cube_id == destination_cube_id
            || source.chain != ChainId::Bitcoin
            || !matches!(
                &source.wallet,
                WalletIdentity::Vault(_) | WalletIdentity::Spark(_)
            )
        {
            return Err(Refusal::Unsupported);
        }
        let wallet_id = wallet_id.into();
        if let Some(id) = &wallet_id {
            let destination_wallet = WalletIdentity::Vault(id.clone());
            if source.wallet == destination_wallet
                || source.wallet.shares_vault_descriptor(&destination_wallet)
            {
                return Err(Refusal::SameWallet);
            }
        }
        let (cube, _) = selected(&root, destination_cube_id, wallet_id.as_ref())?;
        Ok(Self {
            root,
            source,
            cube,
            wallet_id,
            generation: AppGeneration::next(),
            pin_attempt: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            pin_busy: Arc::new(AtomicBool::new(false)),
            pin_signer: Arc::new(Mutex::new(None)),
            verified: false,
            #[cfg(target_os = "macos")]
            ceremony: None,
            backend: None,
            spark_cancel: None,
        })
    }

    pub(crate) fn current(&self, source: &Source) -> Result<(), Refusal> {
        if !operation_live(
            &self.source,
            source,
            self.generation,
            self.generation,
            &self.cancelled,
        ) {
            return Err(Refusal::Stale);
        }
        let (cube, _) = selected(&self.root, &self.cube.id, self.wallet_id.as_ref())?;
        if !same_selection(&self.cube, &cube) {
            return Err(Refusal::IdentityChanged);
        }
        Ok(())
    }

    /// The returned job is `Send` and runs on the blocking pool. Its result
    /// carries a verdict only; the signer stays in this flow's private slot.
    pub(crate) fn begin_pin(
        &mut self,
        source: &Source,
        pin: Zeroizing<String>,
    ) -> Result<PinRequest, Refusal> {
        self.current(source)?;
        if self.cube.is_passkey_cube() || self.verified {
            return Err(Refusal::Unsupported);
        }
        if self
            .pin_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Refusal::Stale);
        }
        let generation = AppGeneration::next();
        self.pin_attempt = Some(generation);
        if let Ok(mut slot) = self.pin_signer.lock() {
            *slot = None;
        }
        Ok(PinRequest {
            root: self.root.clone(),
            cube: self.cube.clone(),
            wallet_id: self.wallet_id.clone(),
            pin,
            generation,
            origin: source.clone(),
            cancelled: Arc::clone(&self.cancelled),
            busy: Arc::clone(&self.pin_busy),
            signer_slot: Arc::clone(&self.pin_signer),
        })
    }

    pub(crate) fn accept_pin(&mut self, source: &Source, result: PinCheck) -> Result<(), Refusal> {
        // Duress remains an emergency signal if the source locked or switched
        // during Argon2. The caller must activate the existing orchestrator.
        if matches!(&result.verdict, Ok(PinVerdict::Duress)) {
            self.revoke();
            return Err(Refusal::Duress);
        }
        if self.pin_attempt != Some(result.generation) {
            return Err(Refusal::Stale);
        }
        if let Err(error) = self.current(source) {
            self.revoke();
            return Err(error);
        }
        self.pin_attempt = None;
        match result.verdict? {
            PinVerdict::Locked(remaining) => Err(Refusal::Locked(remaining)),
            PinVerdict::Wrong => Err(Refusal::WrongCredential),
            PinVerdict::Duress => {
                self.revoke();
                Err(Refusal::Duress)
            }
            PinVerdict::Unlock => {
                let signer = self
                    .pin_signer
                    .lock()
                    .map_err(|_| Refusal::BackendUnavailable)?
                    .take()
                    .ok_or(Refusal::Stale)?;
                self.accept_signer(source, signer)
            }
        }
    }

    fn accept_signer(&mut self, source: &Source, signer: MasterSigner) -> Result<(), Refusal> {
        self.current(source)?;
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
        let fingerprint: Fingerprint = signer.fingerprint(&secp);
        if self.cube.master_signer_fingerprint != Some(fingerprint) {
            return Err(Refusal::IdentityChanged);
        }
        // Vault receive needs only the verified bit. Spark needs the signer
        // once to open its restricted address service.
        if self.wallet_id.is_none() {
            *self
                .pin_signer
                .lock()
                .map_err(|_| Refusal::SparkUnavailable)? = Some(signer);
        }
        self.verified = true;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn begin_passkey(&mut self, source: &Source) -> Result<(), Refusal> {
        use base64::Engine;
        self.current(source)?;
        if !self.cube.is_passkey_cube()
            || self.ceremony.is_some()
            || self.pin_busy.load(Ordering::Acquire)
            || self.verified
        {
            return Err(Refusal::Unsupported);
        }
        let meta = self
            .cube
            .passkey_metadata
            .as_ref()
            .ok_or(Refusal::Unsupported)?;
        let credential_id = base64::engine::general_purpose::STANDARD
            .decode(&meta.credential_id)
            .map_err(|_| Refusal::Unsupported)?;
        self.ceremony = Some(
            crate::services::passkey::macos::NativePasskeyCeremony::authenticate(
                &meta.rp_id,
                &credential_id,
            )
            .map_err(|_| Refusal::BackendUnavailable)?,
        );
        Ok(())
    }

    /// Poll on the UI thread; consume the PRF inline, never in a cloned event.
    #[cfg(target_os = "macos")]
    pub(crate) fn poll_passkey(&mut self, source: &Source) -> Option<Result<(), Refusal>> {
        use crate::services::passkey::macos::NativeOutcome;
        if let Err(error) = self.current(source) {
            self.revoke();
            return Some(Err(error));
        }
        let outcome = self.ceremony.as_ref()?.try_recv()?;
        self.ceremony = None;
        Some(match outcome {
            NativeOutcome::Authenticated { prf_output } => {
                MasterSigner::from_prf_output(Network::Bitcoin, &prf_output)
                    .map_err(|_| Refusal::WrongCredential)
                    .and_then(|signer| self.accept_signer(source, signer))
            }
            NativeOutcome::Registered { .. } | NativeOutcome::Failed(_) => {
                Err(Refusal::WrongCredential)
            }
        })
    }

    /// Creates a separately runnable, cancellation-aware read-only attachment.
    /// A future UI may dispatch it but must call `accept_backend` on return.
    pub(crate) fn request_backend(&self, source: &Source) -> Result<BackendRequest, Refusal> {
        self.current(source)?;
        if !self.verified {
            return Err(Refusal::WrongCredential);
        }
        Ok(BackendRequest {
            root: self.root.clone(),
            cube: self.cube.clone(),
            wallet_id: self.wallet_id.clone().ok_or(Refusal::Unsupported)?,
            generation: self.generation,
            cancelled: Arc::clone(&self.cancelled),
        })
    }

    pub(crate) fn accept_backend(
        &mut self,
        source: &Source,
        generation: AppGeneration,
        backend: BoundBackend,
    ) -> Result<(), Refusal> {
        self.current(source)?;
        if !self.verified
            || !operation_live(
                &self.source,
                source,
                self.generation,
                generation,
                &self.cancelled,
            )
            || self.wallet_id.as_ref() != Some(&backend.wallet_id)
        {
            return Err(Refusal::Stale);
        }
        self.backend = Some(backend);
        Ok(())
    }

    /// Asks the bound destination Vault for a fresh receive address. The
    /// job carries no credential; it only reaches B's own daemon.
    pub(crate) fn request_address(&self, source: &Source) -> Result<AddressRequest, Refusal> {
        self.current(source)?;
        let backend = self.backend.as_ref().ok_or(Refusal::BackendUnavailable)?;
        if !self.verified {
            return Err(Refusal::WrongCredential);
        }
        Ok(AddressRequest {
            root: self.root.clone(),
            descriptor: backend.descriptor.clone(),
            daemon: Arc::clone(&backend.daemon),
            generation: self.generation,
            cancelled: Arc::clone(&self.cancelled),
        })
    }

    /// Admits a daemon reply only if this operation is still live and the
    /// address re-derives from B's verified descriptor at the reported
    /// index. A daemon string alone is never trusted as B's address.
    pub(crate) fn accept_address(
        &self,
        source: &Source,
        reply: AddressReply,
    ) -> Result<DestinationAddress, Refusal> {
        self.current(source)?;
        let backend = self.backend.as_ref().ok_or(Refusal::BackendUnavailable)?;
        if !self.verified || reply.generation != self.generation {
            return Err(Refusal::Stale);
        }
        let (address, index) = reply.result?;
        derived_receive_address(&backend.descriptor, address, index)
    }
}

/// The address is B's only if B's verified descriptor derives it, as a
/// receive (not change) address, at the reported normal index.
fn derived_receive_address(
    descriptor: &coincube_core::wallet_descriptor::WalletDescriptor,
    address: coincube_core::miniscript::bitcoin::Address,
    index: coincube_core::miniscript::bitcoin::bip32::ChildNumber,
) -> Result<DestinationAddress, Refusal> {
    if !index.is_normal() {
        return Err(Refusal::IdentityChanged);
    }
    let expected = descriptor
        .receive_descriptor()
        .script_pubkey(u32::from(index))
        .map_err(|_| Refusal::IdentityChanged)?;
    if expected != address.script_pubkey()
        || !address
            .as_unchecked()
            .is_valid_for_network(Network::Bitcoin)
    {
        return Err(Refusal::IdentityChanged);
    }
    Ok(DestinationAddress { address, index })
}

impl ScopedDestination {
    pub(crate) fn revoke(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.pin_attempt = None;
        self.verified = false;
        if let Ok(mut slot) = self.pin_signer.lock() {
            *slot = None;
        }
        #[cfg(target_os = "macos")]
        {
            self.ceremony = None;
        }
        self.backend = None;
        self.spark_cancel = None;
    }
}

/// The only result permitted through an Iced message is a classification.
/// There is no signer, PIN, or PRF output here.
#[derive(Debug, Clone)]
pub struct PinCheck {
    generation: AppGeneration,
    verdict: Result<PinVerdict, Refusal>,
    /// Present only for a duress match: the one-use activation proof.
    duress: Option<crate::services::unlock::throttle::VerifiedSeedExportDuress>,
}

impl PinCheck {
    /// A duress PIN for the destination Cube. The caller must hand this to
    /// the tab (`DuressActivateLocal`) whatever else has changed.
    pub(crate) fn duress_proof(
        &self,
    ) -> Option<crate::services::unlock::throttle::VerifiedSeedExportDuress> {
        self.duress.clone()
    }
}

#[derive(Debug, Clone)]
enum PinVerdict {
    Unlock,
    Duress,
    Wrong,
    Locked(Duration),
}

pub(crate) struct PinRequest {
    root: CoincubeDirectory,
    cube: CubeSettings,
    wallet_id: Option<WalletId>,
    pin: Zeroizing<String>,
    generation: AppGeneration,
    /// The open (source) Cube: the tab accepts a duress proof only from
    /// the App it hosts, and duress then arms every Cube of the root.
    origin: Source,
    cancelled: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    signer_slot: Arc<Mutex<Option<MasterSigner>>>,
}

impl PinRequest {
    /// `run` on the blocking pool (Argon2 must not stall the UI thread).
    pub(crate) async fn run_blocking(self) -> PinCheck {
        let generation = self.generation;
        tokio::task::spawn_blocking(move || self.run())
            .await
            .unwrap_or(PinCheck {
                generation,
                verdict: Err(Refusal::BackendUnavailable),
                duress: None,
            })
    }

    pub(crate) fn run(self) -> PinCheck {
        let mut duress = None;
        let verdict = if self.cancelled.load(Ordering::Acquire) {
            Err(Refusal::Stale)
        } else {
            let loc = unlock::CubeLocation::new(self.root.path(), &self.cube);
            let metadata_changed = std::cell::Cell::new(false);
            let validate = || {
                let (current, _) = selected(&self.root, &self.cube.id, self.wallet_id.as_ref())
                    .map_err(|_| {
                        metadata_changed.set(true);
                        unlock::UnlockError::Io("Destination Cube changed".into())
                    })?;
                if same_selection(&self.cube, &current) && !self.cancelled.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    metadata_changed.set(true);
                    Err(unlock::UnlockError::Io("Destination Cube changed".into()))
                }
            };
            match unlock::throttle::classify_pin_scoped(self.root.path(), &loc, &self.pin, validate)
            {
                Ok(PinAttempt::Locked(remaining)) => Ok(PinVerdict::Locked(remaining)),
                Ok(PinAttempt::Classified(PinOutcome::Wrong)) => Ok(PinVerdict::Wrong),
                Ok(PinAttempt::Classified(PinOutcome::Duress)) => {
                    duress = Some(unlock::throttle::destination_unlock_duress(
                        self.root.path(),
                        &self.origin.cube_id,
                        self.origin.chain,
                        self.origin.app,
                    ));
                    Ok(PinVerdict::Duress)
                }
                Ok(PinAttempt::Classified(PinOutcome::Unlock(signer))) => {
                    match self.signer_slot.lock() {
                        Ok(mut slot) if !self.cancelled.load(Ordering::Acquire) => {
                            *slot = Some(*signer);
                            Ok(PinVerdict::Unlock)
                        }
                        _ => Err(Refusal::Stale),
                    }
                }
                Err(_) if metadata_changed.get() => Err(Refusal::IdentityChanged),
                Err(_) => Err(Refusal::BackendUnavailable),
            }
        };
        PinCheck {
            generation: self.generation,
            verdict,
            duress,
        }
    }
}

impl Drop for PinRequest {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::Release);
    }
}

impl std::fmt::Debug for ScopedDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScopedDestination(<destination Cube>)")
    }
}

impl Drop for ScopedDestination {
    fn drop(&mut self) {
        self.revoke();
    }
}

pub(crate) struct BackendRequest {
    root: CoincubeDirectory,
    cube: CubeSettings,
    wallet_id: WalletId,
    generation: AppGeneration,
    cancelled: Arc<AtomicBool>,
}

impl BackendRequest {
    pub(crate) async fn run(self) -> Result<BoundBackend, Refusal> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(Refusal::Stale);
        }
        let (current, wallet) = selected(&self.root, &self.cube.id, Some(&self.wallet_id))?;
        if !same_selection(&self.cube, &current) {
            return Err(Refusal::IdentityChanged);
        }
        // This restricted path uses B's own wallet-scoped config. An external
        // B daemon is only attached; an ordinary embedded B daemon is owned
        // here and stopped when this operation ends. Never use A's socket.
        let data_dir = self
            .root
            .network_directory(ChainId::Bitcoin)
            .coincubed_data_directory(&self.wallet_id);
        let config = Config::from_file(Some(data_dir.path().join("daemon.toml")))
            .map_err(|_| Refusal::BackendUnavailable)?;
        let expected_dir =
            std::path::absolute(data_dir.path()).map_err(|_| Refusal::BackendUnavailable)?;
        if config.bitcoin_config.chain != ChainId::Bitcoin
            || config
                .data_directory()
                .is_none_or(|dir| !crate::chain::same_path(dir.path(), &expected_dir))
        {
            return Err(Refusal::IdentityChanged);
        }
        let socket = data_dir.coincubed_rpc_socket_path();
        let (daemon, lease) = destination_daemon(&socket, &config).await?;
        let info = daemon
            .get_info()
            .await
            .map_err(|_| Refusal::BackendUnavailable)?;
        if self.cancelled.load(Ordering::Acquire) {
            return Err(Refusal::Stale);
        }
        let (latest, latest_wallet) = selected(&self.root, &self.cube.id, Some(&self.wallet_id))?;
        let wallet = wallet.ok_or(Refusal::Unsupported)?;
        let latest_wallet = latest_wallet.ok_or(Refusal::Unsupported)?;
        if !same_selection(&current, &latest) || wallet.wallet_id() != latest_wallet.wallet_id() {
            return Err(Refusal::IdentityChanged);
        }
        if info.chain_id != Some(ChainId::Bitcoin) || info.network != Network::Bitcoin {
            return Err(Refusal::IdentityChanged);
        }
        let descriptor = &info.descriptors.main;
        if config.main_descriptor != *descriptor {
            return Err(Refusal::IdentityChanged);
        }
        self.verify_destination_descriptor(descriptor, &latest)?;
        Wallet::new(descriptor.clone())
            .load_from_settings(latest_wallet)
            .map_err(|_| Refusal::IdentityChanged)?;
        Ok(BoundBackend {
            wallet_id: self.wallet_id,
            descriptor: descriptor.clone(),
            daemon,
            _lease: lease,
        })
    }

    fn verify_destination_descriptor(
        &self,
        descriptor: &coincube_core::wallet_descriptor::WalletDescriptor,
        latest: &CubeSettings,
    ) -> Result<(), Refusal> {
        match descriptor {
            // A Vault is identified by its recorded descriptor fingerprint.
            coincube_core::wallet_descriptor::WalletDescriptor::Vault(vault) => {
                let fingerprint = descriptor_id_fingerprint(vault).to_string();
                if latest.vault_fingerprint.as_deref() != Some(fingerprint.as_str()) {
                    return Err(Refusal::IdentityChanged);
                }
            }
            // An imported Vault's wallet id is its descriptor's identity.
            coincube_core::wallet_descriptor::WalletDescriptor::Imported(imported) => {
                if WalletId::for_imported(imported, ChainId::Bitcoin)
                    .ok()
                    .as_ref()
                    != Some(&self.wallet_id)
                    || latest
                        .bitcoin_import
                        .as_deref()
                        .and_then(crate::app::settings::BitcoinImport::active_attachment)
                        .is_none_or(|wallet| wallet.descriptor != *imported)
                {
                    return Err(Refusal::IdentityChanged);
                }
            }
        }
        Ok(())
    }
}

/// Carries one `BackendRequest` result through an Iced message. Cloning
/// shares the slot; the first `ScopedDestination::accept_backend_reply` takes
/// it, and dropping the last clone stops an owned daemon.
#[derive(Clone)]
pub struct BackendReply {
    generation: AppGeneration,
    slot: Arc<Mutex<Option<Result<BoundBackend, Refusal>>>>,
}

impl std::fmt::Debug for BackendReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BackendReply(<destination backend>)")
    }
}

impl BackendRequest {
    pub(crate) async fn run_reply(self) -> BackendReply {
        let generation = self.generation;
        BackendReply {
            generation,
            slot: Arc::new(Mutex::new(Some(self.run().await))),
        }
    }
}

impl ScopedDestination {
    pub(crate) fn accept_backend_reply(
        &mut self,
        source: &Source,
        reply: BackendReply,
    ) -> Result<(), Refusal> {
        let result = reply
            .slot
            .lock()
            .map_err(|_| Refusal::BackendUnavailable)?
            .take()
            .ok_or(Refusal::Stale)?;
        self.accept_backend(source, reply.generation, result?)
    }

    pub(crate) fn is_passkey(&self) -> bool {
        self.cube.is_passkey_cube()
    }

    pub(crate) fn receives_spark(&self) -> bool {
        self.wallet_id.is_none()
    }
}

/// One `get_new_address` call on B's bound daemon.
pub(crate) struct AddressRequest {
    root: CoincubeDirectory,
    descriptor: coincube_core::wallet_descriptor::WalletDescriptor,
    daemon: Arc<dyn Daemon + Send + Sync>,
    generation: AppGeneration,
    cancelled: Arc<AtomicBool>,
}

/// The reply passed back through an Iced message: an unverified address and
/// index, checked by `ScopedDestination::accept_address`.
#[derive(Debug, Clone)]
pub struct AddressReply {
    generation: AppGeneration,
    result: Result<
        (
            coincube_core::miniscript::bitcoin::Address,
            coincube_core::miniscript::bitcoin::bip32::ChildNumber,
        ),
        Refusal,
    >,
}

impl AddressRequest {
    pub(crate) async fn run(self) -> AddressReply {
        let result = if self.cancelled.load(Ordering::Acquire) {
            Err(Refusal::Stale)
        } else {
            // An imported Vault allocates under its source reservation lock,
            // like its own Receive screen.
            crate::app::state::imported_bitcoin::reserve_receive_address(
                self.root,
                self.descriptor,
                self.daemon,
            )
            .await
            .map(|res| (res.address, res.derivation_index))
            .map_err(|_| Refusal::BackendUnavailable)
        };
        AddressReply {
            generation: self.generation,
            result,
        }
    }
}

/// A checked Bitcoin receive address. Vault addresses include their verified
/// derivation index. Spark deposit addresses have no Vault derivation index.
#[derive(Debug, Clone)]
pub(crate) struct DestinationAddress {
    pub address: coincube_core::miniscript::bitcoin::Address,
    pub index: coincube_core::miniscript::bitcoin::bip32::ChildNumber,
}

#[derive(Debug, Clone)]
pub(crate) enum OtherReceiveAddress {
    Bitcoin(DestinationAddress),
    Spark(String),
}

pub(crate) struct SparkAddressRequest {
    signer: MasterSigner,
    source_identity: Option<String>,
    generation: AppGeneration,
    cancelled: tokio::sync::oneshot::Receiver<()>,
}

#[derive(Debug, Clone)]
pub struct SparkAddressReply {
    generation: AppGeneration,
    result: Result<OtherReceiveAddress, Refusal>,
}

impl ScopedDestination {
    pub(crate) fn request_spark_address(
        &mut self,
        source: &Source,
    ) -> Result<SparkAddressRequest, Refusal> {
        self.current(source)?;
        if !self.verified || self.wallet_id.is_some() {
            return Err(Refusal::Unsupported);
        }
        let signer = self
            .pin_signer
            .lock()
            .map_err(|_| Refusal::SparkUnavailable)?
            .take()
            .ok_or(Refusal::Stale)?;
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        self.spark_cancel = Some(cancel);
        Ok(SparkAddressRequest {
            signer,
            source_identity: match &source.wallet {
                WalletIdentity::Spark(identity) => Some(identity.clone()),
                _ => None,
            },
            generation: self.generation,
            cancelled,
        })
    }

    pub(crate) fn accept_spark_address(
        &self,
        source: &Source,
        reply: SparkAddressReply,
    ) -> Result<OtherReceiveAddress, Refusal> {
        self.current(source)?;
        if !self.verified || self.wallet_id.is_some() || reply.generation != self.generation {
            return Err(Refusal::Stale);
        }
        reply.result
    }
}

impl SparkAddressRequest {
    pub(crate) async fn run(self) -> SparkAddressReply {
        let Self {
            signer,
            source_identity,
            generation,
            cancelled,
        } = self;
        let result = tokio::select! {
            biased;
            _ = cancelled => Err(Refusal::Stale),
            result = Self::receive(signer, source_identity) => result,
        };
        SparkAddressReply { generation, result }
    }

    async fn receive(
        signer: MasterSigner,
        source_identity: Option<String>,
    ) -> Result<OtherReceiveAddress, Refusal> {
        use crate::app::breez_spark::{SparkClient, SparkConfig};
        // The normal wallet may already be open. The SDK's reusable Bitcoin
        // deposit address is registered with the operator and is recovered by
        // the regular wallet on sync. Neither address needs this scratch DB.
        let scratch = tempfile::tempdir().map_err(|_| Refusal::SparkUnavailable)?;
        let config = SparkConfig::for_network(Network::Bitcoin, scratch.path().to_owned())
            .map_err(|_| Refusal::SparkUnavailable)?;
        let mnemonic = signer.mnemonic_str();
        drop(signer);
        let client = SparkClient::connect_receive_only(config, mnemonic.as_str())
            .await
            .map_err(|_| Refusal::SparkUnavailable)?;
        drop(mnemonic);
        let result = Self::read_address(&client, source_identity.as_deref()).await;
        let _ = client.shutdown().await;
        result
    }

    async fn read_address(
        client: &crate::app::breez_spark::SparkClient,
        source_identity: Option<&str>,
    ) -> Result<OtherReceiveAddress, Refusal> {
        if let Some(source_identity) = source_identity {
            let identity = client
                .receive_identity()
                .await
                .map_err(|_| Refusal::SparkUnavailable)?;
            if identity == source_identity {
                return Err(Refusal::SameSparkWallet);
            }
            let address = client
                .receive_spark()
                .await
                .map_err(|_| Refusal::SparkUnavailable)?;
            Ok(OtherReceiveAddress::Spark(address.payment_request))
        } else {
            let response = client
                .receive_onchain(None)
                .await
                .map_err(|_| Refusal::SparkUnavailable)?;
            let address = spark_deposit_address(&response.payment_request)
                .ok_or(Refusal::SparkUnavailable)?;
            Ok(OtherReceiveAddress::Bitcoin(DestinationAddress {
                address,
                // Review's on-chain address model carries an index. Other
                // Cube transfers never expose Vault address details.
                index: 0.into(),
            }))
        }
    }
}

/// Spark's on-chain `payment_request` may be a bare address or a BIP21 URI.
/// Keep only the mainnet address; any other scheme is refused.
fn spark_deposit_address(
    payment_request: &str,
) -> Option<coincube_core::miniscript::bitcoin::Address> {
    let address = match payment_request.split_once(':') {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("bitcoin") => rest,
        Some(_) => return None,
        None => payment_request,
    };
    let address = address.split('?').next().unwrap_or(address);
    address
        .parse::<coincube_core::miniscript::bitcoin::Address<_>>()
        .ok()?
        .require_network(Network::Bitcoin)
        .ok()
}

async fn destination_daemon(
    socket: &Path,
    config: &Config,
) -> Result<(Arc<dyn Daemon + Send + Sync>, BackendLease), Refusal> {
    // A present socket is treated as externally owned. If it is stale or
    // broken, refuse rather than launching a second daemon against that DB.
    if socket.exists() {
        return Ok((
            Arc::new(client::Coincubed::new(client::jsonrpc::JsonRPCClient::new(
                socket,
            ))),
            BackendLease(None),
        ));
    }
    let config = config.clone();
    // The owned guard is constructed inside the blocking task. If the async
    // caller cancels before receiving its result, dropping the result still
    // stops the daemon instead of leaking its poller.
    let owned = tokio::task::spawn_blocking(move || {
        EmbeddedDaemon::start(config)
            .map(|daemon| OwnedEmbeddedDestination {
                daemon: Arc::new(daemon),
            })
            .map_err(|error| match error {
                crate::daemon::DaemonError::Unexpected(message)
                    if message == crate::daemon::stopping::STILL_STOPPING =>
                {
                    Refusal::StillStopping
                }
                _ => Refusal::BackendUnavailable,
            })
    })
    .await
    .map_err(|_| Refusal::BackendUnavailable)??;
    let daemon: Arc<dyn Daemon + Send + Sync> = owned.daemon.clone();
    Ok((daemon, BackendLease(Some(Box::new(owned)))))
}

struct OwnedEmbeddedDestination {
    daemon: Arc<EmbeddedDaemon>,
}

impl OwnedDestinationResource for OwnedEmbeddedDestination {
    fn close(self: Box<Self>) {
        drop(self);
    }
}

impl Drop for OwnedEmbeddedDestination {
    fn drop(&mut self) {
        let daemon = Arc::clone(&self.daemon);
        // Use an independent runtime: an abandoned async request can be
        // dropped while the caller's runtime is shutting down.
        let result = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?;
            runtime
                .block_on(async { daemon.stop().await })
                .map_err(|error| error.to_string())
        })
        .join();
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::error!("Destination daemon stop failed: {error}"),
            Err(_) => tracing::error!("Destination daemon stop panicked"),
        }
    }
}

/// Attached external services are dropped without `stop`; locally started B
/// daemons carry a guard that stops them at the operation boundary.
pub(crate) struct BoundBackend {
    wallet_id: WalletId,
    /// B's descriptor as checked against its settings: a Vault's
    /// fingerprint, or an imported Vault's wallet identity.
    descriptor: coincube_core::wallet_descriptor::WalletDescriptor,
    daemon: Arc<dyn Daemon + Send + Sync>,
    _lease: BackendLease,
}

struct BackendLease(Option<Box<dyn OwnedDestinationResource>>);

impl Drop for BackendLease {
    fn drop(&mut self) {
        if let Some(resource) = self.0.take() {
            resource.close();
        }
        // A JSON-RPC client has no resource guard; dropping its Arc never
        // invokes Daemon::stop on the external process.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn source() -> Source {
        Source {
            cube_id: "source".into(),
            chain: ChainId::Bitcoin,
            wallet: WalletIdentity::Vault(WalletId::new("source-id".into(), Some(1))),
            app: AppGeneration::next(),
        }
    }

    #[test]
    fn spark_deposit_address_accepts_a_bip21_uri() {
        let bare = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";
        for request in [
            bare.to_string(),
            format!("bitcoin:{bare}"),
            format!("BITCOIN:{bare}?amount=0.1&label=Cube"),
        ] {
            assert_eq!(
                spark_deposit_address(&request).map(|address| address.to_string()),
                Some(bare.to_string()),
                "{}",
                request
            );
        }
        for request in [
            format!("liquidnetwork:{bare}"),
            "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx".to_string(),
            "not an address".to_string(),
        ] {
            assert!(spark_deposit_address(&request).is_none(), "{}", request);
        }
    }

    #[test]
    fn other_cubes_lists_only_the_source_network() {
        let directory = tempfile::tempdir().unwrap();
        let root = CoincubeDirectory::new(directory.path().to_owned());
        for (chain, names) in [
            (ChainId::Bitcoin, ["source", "Archive"]),
            (ChainId::Testnet4, ["Lab", "Passkey"]),
        ] {
            let dir = root.network_directory(chain);
            std::fs::create_dir_all(dir.path()).unwrap();
            let cubes = names
                .iter()
                .map(|name| CubeSettings::new_with_raw_id((*name).into(), (*name).into(), chain))
                .collect();
            std::fs::write(
                dir.path().join(super::super::settings::SETTINGS_FILE_NAME),
                serde_json::to_vec(&Settings {
                    cubes,
                    ..Settings::default()
                })
                .unwrap(),
            )
            .unwrap();
        }
        let choices = other_cubes(&root, &source());
        assert_eq!(
            choices
                .iter()
                .map(|choice| (choice.name.as_str(), choice.chain))
                .collect::<Vec<_>>(),
            [("Archive", ChainId::Bitcoin)]
        );
    }

    #[tokio::test]
    async fn both_spark_destination_routes_share_a_cancellable_unlock() {
        let directory = tempfile::tempdir().unwrap();
        let root = CoincubeDirectory::new(directory.path().to_owned());
        let bitcoin = root.network_directory(ChainId::Bitcoin);
        std::fs::create_dir_all(bitcoin.path()).unwrap();
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
        let mut cube =
            CubeSettings::new_with_raw_id("destination".into(), "Archive".into(), ChainId::Bitcoin);
        let vault_source = source();
        let mut spark_source = vault_source.clone();
        spark_source.wallet = WalletIdentity::Spark("sending-identity".into());
        for (source, identity) in [
            (&vault_source, None),
            (&spark_source, Some("sending-identity")),
        ] {
            let signer = MasterSigner::generate(Network::Bitcoin).unwrap();
            cube.master_signer_fingerprint = Some(signer.fingerprint(&secp));
            std::fs::write(
                bitcoin
                    .path()
                    .join(super::super::settings::SETTINGS_FILE_NAME),
                serde_json::to_vec(&Settings {
                    cubes: vec![cube.clone()],
                    ..Settings::default()
                })
                .unwrap(),
            )
            .unwrap();
            let mut scope =
                ScopedDestination::new(root.clone(), source.clone(), &cube.id, None).unwrap();
            assert!(matches!(
                scope.request_spark_address(source),
                Err(Refusal::Unsupported)
            ));
            scope.accept_signer(source, signer).unwrap();
            let job = scope.request_spark_address(source).unwrap();
            assert_eq!(job.source_identity.as_deref(), identity);
            assert!(matches!(
                scope.request_spark_address(source),
                Err(Refusal::Stale)
            ));
            scope.revoke();
            let reply = job.run().await;
            assert!(matches!(reply.result, Err(Refusal::Stale)));
            assert!(matches!(
                scope.accept_spark_address(source, reply),
                Err(Refusal::Stale)
            ));
        }
    }

    #[tokio::test]
    async fn receiving_spark_refuses_the_sending_wallet_identity() {
        use crate::app::breez_spark::SparkClient;
        use coincube_spark_protocol::{GetInfoOk, GetInfoParams, Method, OkPayload};
        let (client, mut requests) = SparkClient::mock_transport();
        let (result, ()) = tokio::join!(
            SparkAddressRequest::read_address(&client, Some("same-identity")),
            async {
                let request = requests.recv().await.unwrap();
                assert!(matches!(
                    request.method,
                    Method::GetInfo(GetInfoParams {
                        ensure_synced: Some(false)
                    })
                ));
                client
                    .mock_reply(
                        request.id,
                        Some(OkPayload::GetInfo(GetInfoOk {
                            balance_sats: 0,
                            identity_pubkey: "same-identity".into(),
                            stable_balance: None,
                            usdb_balance: None,
                            recoverable_funds_sats: 0,
                        })),
                    )
                    .await;
            },
        );
        assert!(matches!(result, Err(Refusal::SameSparkWallet)));
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn stale_source_generation_and_cancellation_refuse_a_result() {
        let original = source();
        let generation = AppGeneration::next();
        let cancelled = AtomicBool::new(false);
        assert!(operation_live(
            &original, &original, generation, generation, &cancelled
        ));
        let mut switched = original.clone();
        switched.app = AppGeneration::next();
        assert!(!operation_live(
            &original, &switched, generation, generation, &cancelled
        ));
        assert!(!operation_live(
            &original,
            &original,
            generation,
            AppGeneration::next(),
            &cancelled
        ));
        cancelled.store(true, Ordering::Release);
        assert!(!operation_live(
            &original, &original, generation, generation, &cancelled
        ));
    }

    #[test]
    fn selected_cube_mutation_invalidates_its_verification() {
        let mut selected = CubeSettings::new_with_raw_id(
            "destination".into(),
            "Destination".into(),
            ChainId::Bitcoin,
        );
        selected.vault_wallet_id = Some(WalletId::new("selected".into(), Some(2)));
        let mut changed = selected.clone();
        assert!(same_selection(&selected, &changed));
        changed.vault_wallet_id = Some(WalletId::new("rotated".into(), Some(2)));
        assert!(!same_selection(&selected, &changed));
        changed = selected.clone();
        changed.network = ChainId::Testnet;
        assert!(!same_selection(&selected, &changed));
        changed = selected.clone();
        changed.duress_slot_file = Some("rotated-marker".into());
        assert!(!same_selection(&selected, &changed));
    }

    #[test]
    fn another_cube_with_the_source_wallet_is_not_a_destination() {
        let source = source();
        let WalletIdentity::Vault(wallet_id) = source.wallet.clone() else {
            unreachable!();
        };
        let root = CoincubeDirectory::new(std::env::temp_dir().join("unused-r16-alias"));
        assert!(matches!(
            ScopedDestination::new(root, source, "different-cube-id", wallet_id),
            Err(Refusal::SameWallet)
        ));
    }

    #[test]
    fn a_restored_vault_with_a_new_timestamp_is_not_a_destination() {
        let source = source();
        let WalletIdentity::Vault(source_id) = &source.wallet else {
            unreachable!();
        };
        let restored_id = WalletId::new(source_id.descriptor_checksum.clone(), Some(99));
        assert_ne!(source.wallet, WalletIdentity::Vault(restored_id.clone()));
        let root = CoincubeDirectory::new(std::env::temp_dir().join("unused-r16-restored-alias"));
        assert!(matches!(
            ScopedDestination::new(root, source, "different-cube-id", restored_id),
            Err(Refusal::SameWallet)
        ));
    }

    #[test]
    fn synthetic_destination_pin_outcomes_leave_the_source_session_intact() {
        use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;
        use coincube_core::signer::MASTER_SEED_LABEL;

        let _session_guard = super::super::session::test_guard();
        super::super::session::close();
        let root_path = std::env::temp_dir().join(format!(
            "coincube-r16-b-pin-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let root = CoincubeDirectory::new(root_path.clone());
        let bitcoin_dir = root.network_directory(ChainId::Bitcoin);
        std::fs::create_dir_all(bitcoin_dir.path()).unwrap();

        let secp = Secp256k1::signing_only();
        let source_signer = MasterSigner::generate(Network::Bitcoin).unwrap();
        let source_fp = source_signer.fingerprint(&secp);
        super::super::session::open("source", Zeroizing::new("source-pin".into()));
        super::super::session::store_unlocked_signer("source", source_fp, source_signer);

        let destination_signer = MasterSigner::generate(Network::Bitcoin).unwrap();
        let destination_fp = destination_signer.fingerprint(&secp);
        let mut cube = CubeSettings::new_with_raw_id(
            "destination".into(),
            "Synthetic destination".into(),
            ChainId::Bitcoin,
        );
        let wallet_id = WalletId::new("destination".into(), Some(1));
        cube.vault_wallet_id = Some(wallet_id.clone());
        cube.vault_fingerprint = Some("synthetic-vault".into());
        cube.master_signer_fingerprint = Some(destination_fp);
        destination_signer
            .store_encrypted(
                &root_path,
                Network::Bitcoin,
                &secp,
                Some((
                    format!("{}{}", MASTER_SEED_LABEL, cube.created_at),
                    cube.created_at,
                )),
                "1234",
                &cube.id,
                None,
            )
            .unwrap();
        let marker_name = unlock::marker::new_file_name(cube.created_at);
        unlock::marker::write(
            &root_path,
            ChainId::Bitcoin,
            &cube.id,
            &marker_name,
            "9999",
            None,
        )
        .unwrap();
        cube.duress_slot_file = Some(marker_name);
        let wallet = WalletSettings {
            name: "Synthetic destination".into(),
            alias: None,
            descriptor_checksum: wallet_id.descriptor_checksum.clone(),
            pinned_at: wallet_id.timestamp,
            keys: Vec::new(),
            hardware_wallets: Vec::new(),
            remote_backend_auth: None,
            start_internal_bitcoind: None,
            pending_rescan: None,
            keychain_keys_recorded: false,
        };
        let settings = Settings {
            cubes: vec![cube],
            wallets: vec![wallet],
            ..Settings::default()
        };
        std::fs::write(
            bitcoin_dir
                .path()
                .join(super::super::settings::SETTINGS_FILE_NAME),
            serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();

        let source = source();
        let mut destination = ScopedDestination::new(
            root.clone(),
            source.clone(),
            "destination",
            wallet_id.clone(),
        )
        .unwrap();
        let wrong = destination
            .begin_pin(&source, Zeroizing::new("4321".into()))
            .unwrap()
            .run();
        assert_eq!(
            destination.accept_pin(&source, wrong),
            Err(Refusal::WrongCredential)
        );
        let counters =
            serde_json::to_value(unlock::throttle::ThrottleState::load(&root_path)).unwrap();
        assert_eq!(
            counters["failures"]["destination"]["consecutive_failures"],
            1
        );
        assert!(counters["failures"].get("source").is_none());
        let correct = destination
            .begin_pin(&source, Zeroizing::new("1234".into()))
            .unwrap()
            .run();
        assert_eq!(destination.accept_pin(&source, correct), Ok(()));
        assert!(destination.verified);
        assert!(destination.pin_signer.lock().unwrap().is_none());
        let counters =
            serde_json::to_value(unlock::throttle::ThrottleState::load(&root_path)).unwrap();
        assert!(counters["failures"].get("destination").is_none());
        assert_eq!(
            super::super::session::current_cube_id().as_deref(),
            Some("source")
        );
        assert!(super::super::session::unlocked_signer("source", source_fp).is_some());
        assert!(super::super::session::unlocked_signer("destination", destination_fp).is_none());

        let mut throttle = unlock::throttle::ThrottleState::load(&root_path);
        for _ in 0..6 {
            throttle.record_failure(&root_path, "destination");
        }
        let mut locked_destination = ScopedDestination::new(
            root.clone(),
            source.clone(),
            "destination",
            wallet_id.clone(),
        )
        .unwrap();
        let locked = locked_destination
            .begin_pin(&source, Zeroizing::new("1234".into()))
            .unwrap()
            .run();
        let remaining = match &locked.verdict {
            Ok(PinVerdict::Locked(remaining)) => *remaining,
            other => panic!("expected lockout, got {:?}", other),
        };
        assert!(!remaining.is_zero());
        assert_eq!(
            locked_destination.accept_pin(&source, locked),
            Err(Refusal::Locked(remaining))
        );
        unlock::throttle::ThrottleState::load(&root_path).record_success(&root_path, "destination");

        let mut duress_destination =
            ScopedDestination::new(root.clone(), source.clone(), "destination", wallet_id).unwrap();
        let duress = duress_destination
            .begin_pin(&source, Zeroizing::new("9999".into()))
            .unwrap()
            .run();
        assert_eq!(
            duress_destination.accept_pin(&source, duress),
            Err(Refusal::Duress)
        );
        assert!(!duress_destination.verified);
        assert!(duress_destination.pin_signer.lock().unwrap().is_none());
        assert_eq!(
            super::super::session::current_cube_id().as_deref(),
            Some("source")
        );
        assert!(super::super::session::unlocked_signer("source", source_fp).is_some());
        super::super::session::close();
        std::fs::remove_dir_all(root_path).unwrap();
    }

    struct CountClose(Arc<AtomicUsize>);

    impl OwnedDestinationResource for CountClose {
        fn close(self: Box<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn teardown_closes_owned_resource_once_and_never_stops_external() {
        let closes = Arc::new(AtomicUsize::new(0));
        drop(BackendLease(Some(Box::new(CountClose(Arc::clone(
            &closes,
        ))))));
        assert_eq!(closes.load(Ordering::SeqCst), 1);
        drop(BackendLease(None));
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancelled_pin_job_never_opens_a_seed_or_repopulates_its_slot() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("coincube-r16-cancelled-pin-{}", std::process::id())),
        );
        let cancelled = Arc::new(AtomicBool::new(true));
        let busy = Arc::new(AtomicBool::new(true));
        let signer_slot = Arc::new(Mutex::new(None));
        let job = PinRequest {
            root,
            cube: CubeSettings::new_with_raw_id(
                "destination".into(),
                "Destination".into(),
                ChainId::Bitcoin,
            ),
            wallet_id: Some(WalletId::new("destination".into(), Some(1))),
            pin: Zeroizing::new("1234".into()),
            generation: AppGeneration::next(),
            origin: source(),
            cancelled,
            busy: Arc::clone(&busy),
            signer_slot: Arc::clone(&signer_slot),
        };
        let result = job.run();
        assert!(matches!(result.verdict, Err(Refusal::Stale)));
        assert!(signer_slot.lock().unwrap().is_none());
        assert!(!busy.load(Ordering::Acquire));
    }

    #[test]
    fn duress_outlives_a_stale_or_locked_source_and_tears_down_the_owner() {
        let original = source();
        let mut changed = original.clone();
        changed.app = AppGeneration::next();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut owner = ScopedDestination {
            root: CoincubeDirectory::new(std::env::temp_dir().join("unused-r16-duress")),
            source: original,
            cube: CubeSettings::new_with_raw_id(
                "destination".into(),
                "Destination".into(),
                ChainId::Bitcoin,
            ),
            wallet_id: Some(WalletId::new("destination".into(), Some(1))),
            generation: AppGeneration::next(),
            pin_attempt: None,
            cancelled: Arc::clone(&cancelled),
            pin_busy: Arc::new(AtomicBool::new(false)),
            pin_signer: Arc::new(Mutex::new(None)),
            verified: false,
            #[cfg(target_os = "macos")]
            ceremony: None,
            backend: None,
            spark_cancel: None,
        };
        let result = PinCheck {
            generation: AppGeneration::next(),
            verdict: Ok(PinVerdict::Duress),
            duress: None,
        };
        assert_eq!(owner.accept_pin(&changed, result), Err(Refusal::Duress));
        assert!(cancelled.load(Ordering::Acquire));
        assert!(!owner.verified);
    }

    #[test]
    fn destination_address_must_derive_from_the_verified_descriptor() {
        use coincube_core::descriptors::CoincubeDescriptor;
        use coincube_core::miniscript::bitcoin::{bip32::ChildNumber, secp256k1::Secp256k1};
        use std::str::FromStr;
        let descriptor = CoincubeDescriptor::from_str("wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr").unwrap();
        let wallet = coincube_core::wallet_descriptor::WalletDescriptor::Vault(descriptor.clone());
        let secp = Secp256k1::verification_only();
        let index = ChildNumber::from(3);
        let receive = descriptor
            .receive_descriptor()
            .derive(index, &secp)
            .address(Network::Bitcoin);
        let accepted = derived_receive_address(&wallet, receive.clone(), index).unwrap();
        assert_eq!(accepted.address, receive);

        // Same descriptor, other index: not the reported address.
        let other_index = descriptor
            .receive_descriptor()
            .derive(ChildNumber::from(4), &secp)
            .address(Network::Bitcoin);
        assert_eq!(
            derived_receive_address(&wallet, other_index, index).unwrap_err(),
            Refusal::IdentityChanged
        );
        // A change address at the same index is not a receive address.
        let change = descriptor
            .change_descriptor()
            .derive(index, &secp)
            .address(Network::Bitcoin);
        assert_eq!(
            derived_receive_address(&wallet, change, index).unwrap_err(),
            Refusal::IdentityChanged
        );
        // Hardened indexes are never receive indexes.
        assert_eq!(
            derived_receive_address(&wallet, receive, ChildNumber::from_hardened_idx(3).unwrap())
                .unwrap_err(),
            Refusal::IdentityChanged
        );
    }

    /// An imported Vault's address is checked the same way: it must derive
    /// from the verified imported descriptor's receive branch at the index.
    #[test]
    fn an_imported_destination_address_must_derive_from_its_descriptor() {
        use crate::services::split_test_wallets::{imported_vault, SHAPES};
        use coincube_core::miniscript::bitcoin::{bip32::ChildNumber, Address};
        for shape in SHAPES {
            let wallet = imported_vault(shape).main_descriptor;
            let imported = wallet.as_imported().unwrap();
            let at = |branch: coincube_core::wallet_descriptor::PublicBranchRef<'_>, index| {
                Address::from_script(&branch.script_pubkey(index).unwrap(), Network::Bitcoin)
                    .unwrap()
            };
            let index = ChildNumber::from(3);
            let receive = at(wallet.receive_descriptor(), 3);
            assert_eq!(
                derived_receive_address(&wallet, receive.clone(), index)
                    .unwrap()
                    .address,
                receive,
                "{:?}",
                shape
            );
            assert_eq!(
                derived_receive_address(&wallet, at(wallet.receive_descriptor(), 4), index)
                    .unwrap_err(),
                Refusal::IdentityChanged
            );
            assert_eq!(
                derived_receive_address(&wallet, at(wallet.change_descriptor(), 3), index)
                    .unwrap_err(),
                Refusal::IdentityChanged
            );
            assert!(imported.chain() == ChainId::Bitcoin);
        }
    }

    /// Which Cubes can receive into their Vault: a Vault with its fingerprint
    /// recorded, or an imported Vault once its scan has finished.
    #[test]
    fn a_ready_imported_vault_can_receive_a_cross_cube_transfer() {
        use crate::app::settings::{BitcoinImport, BitcoinImportStage, ImportedWalletAttachment};
        let mut vault = CubeSettings::new("Vault".into(), ChainId::Bitcoin);
        vault.vault_wallet_id = Some(WalletId::new("abcd1234".into(), Some(1)));
        assert_eq!(receiving_wallet_id(&vault), None, "no fingerprint yet");
        vault.vault_fingerprint = Some("abcd1234".into());
        assert_eq!(receiving_wallet_id(&vault), vault.vault_wallet_id);

        let descriptor = crate::services::split_test_wallets::imported_vault(
            crate::services::split_test_wallets::SHAPES[0],
        )
        .main_descriptor
        .as_imported()
        .unwrap()
        .clone();
        let wallet = ImportedWalletAttachment::new(descriptor, ChainId::Bitcoin, 0, 20).unwrap();
        let mut imported = CubeSettings::new("Imported".into(), ChainId::Bitcoin);
        imported.bitcoin_import = Some(Box::new(BitcoinImport {
            operation_id: uuid::Uuid::new_v4(),
            stage: BitcoinImportStage::Syncing {
                wallet: wallet.clone(),
            },
            last_error: None,
        }));
        assert_eq!(receiving_wallet_id(&imported), None, "still scanning");
        imported.bitcoin_import.as_mut().unwrap().stage = BitcoinImportStage::Ready {
            wallet: wallet.clone(),
        };
        assert_eq!(
            receiving_wallet_id(&imported),
            Some(wallet.wallet_id.clone())
        );
    }

    #[test]
    fn older_pin_trial_cannot_admit_a_later_result_in_the_same_operation() {
        let original = source();
        let newer = AppGeneration::next();
        let mut owner = ScopedDestination {
            root: CoincubeDirectory::new(std::env::temp_dir().join("unused-r16-pin-nonce")),
            source: original.clone(),
            cube: CubeSettings::new_with_raw_id(
                "destination".into(),
                "Destination".into(),
                ChainId::Bitcoin,
            ),
            wallet_id: Some(WalletId::new("destination".into(), Some(1))),
            generation: AppGeneration::next(),
            pin_attempt: Some(newer),
            cancelled: Arc::new(AtomicBool::new(false)),
            pin_busy: Arc::new(AtomicBool::new(false)),
            pin_signer: Arc::new(Mutex::new(None)),
            verified: false,
            #[cfg(target_os = "macos")]
            ceremony: None,
            backend: None,
            spark_cancel: None,
        };
        let old = PinCheck {
            generation: AppGeneration::next(),
            verdict: Ok(PinVerdict::Unlock),
            duress: None,
        };
        assert_eq!(owner.accept_pin(&original, old), Err(Refusal::Stale));
        assert_eq!(owner.pin_attempt, Some(newer));
        assert!(!owner.verified);
    }
}
