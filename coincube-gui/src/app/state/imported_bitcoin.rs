//! Public-descriptor Bitcoin wallet setup and runtime for one Cube.
//!
//! The Cube's own seed and recovery kit remain separate from the imported
//! wallet's signers. This panel never constructs a Vault wallet or a Vault
//! `SpendTx` from the imported descriptors.

use std::{
    collections::{BTreeSet, HashMap},
    convert::TryFrom,
    str::FromStr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use coincube_core::{
    imported_spend::{finalize_imported_spend, verify_imported_signatures, ImportedFinalizeError},
    miniscript::{
        bitcoin::{address, psbt::Psbt, secp256k1, Address, Network, Txid},
        DescriptorPublicKey, ForEachKey,
    },
    public_wallet_finalize::FinalizeError,
    wallet_descriptor::{ImportedWalletDescriptor, WalletDescriptor},
};
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::{
    component::{button, date_picker, spinner, text::*},
    theme,
    widget::{Column, ColumnExt, Element, Row, RowExt},
};
use coincubed::{
    commands::{CoinStatus, CreateSpendResult, ListSpendEntry},
    config::{BitcoinBackend, BitcoinConfig, Config, ElectrumConfig, EsploraConfig},
};
use iced::{
    widget::{progress_bar, text_input},
    Subscription, Task,
};

use crate::{
    app::{
        cache::Cache,
        menu::{Menu, SettingsOption, VaultSubMenu},
        message::Message,
        settings::{self, BitcoinImport, BitcoinImportStage, ImportedWalletAttachment},
        state::State,
        view,
    },
    chain::ChainId,
    daemon::{embedded::EmbeddedDaemon, model, Daemon},
    dir::{CoincubeDirectory, NetworkDirectory},
    hw::{HardwareWallet, HardwareWalletMessage, HardwareWallets},
    services::{bitcoin_import, coincube::CoincubeClient, psbt_file},
    split_hardware::{bind::ProductionBinder, policy::imported_policy, sign::SigningSession},
    wallet_source_form::{self, Source, SourceFormCopy, WalletSourceForm},
};

mod keys;

use keys::{KeySource, KeyStatus, KeysState};

/// Serialize imported receive allocation per public source across sessions.
/// Source and paired target descriptors have the same source digest. The guard
/// is released before callers request a poll; it never owns daemon shutdown.
pub(crate) async fn reserve_receive_address(
    root: CoincubeDirectory,
    descriptor: WalletDescriptor,
    daemon: Arc<dyn Daemon + Send + Sync>,
) -> Result<model::GetAddressResult, String> {
    let source = descriptor.as_imported().and_then(|imported| {
        coincube_core::public_wallet_source::SplitSource::new(
            imported.receive_descriptor().clone(),
            Some(imported.change_descriptor().clone()),
        )
        .ok()
    });
    // Scan-only policies have no change branch and take no lock.
    let _guard = match source {
        Some(source) => Some(
            tokio::task::spawn_blocking(move || {
                bitcoin_import::pairing::source_lock::source_reservation_lock(&root, &source)
            })
            .await
            .map_err(|_| "Reserving the receive address was interrupted.".to_string())??,
        ),
        None => None,
    };
    daemon
        .get_new_address()
        .await
        .map_err(|error| error.to_string())
}

fn birthday_timestamp(date: chrono::NaiveDate) -> Result<u32, String> {
    if date < chrono::NaiveDate::from_ymd_opt(2009, 1, 3).unwrap()
        || date > chrono::Utc::now().date_naive()
    {
        return Err(
            "Choose a date from January 3, 2009 through today, or scan from the beginning.".into(),
        );
    }
    let timestamp = date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
    u32::try_from(timestamp).map_err(|_| "The wallet creation date is out of range.".into())
}

/// Show an English error or notice from this panel or its services in the
/// window's language. State keeps the English; the panel's own templates are
/// matched here and everything else goes to `locale::notice`, which copies
/// unknown diagnostics exactly.
fn shown_notice(english: &str) -> String {
    if let Some(error) = english.strip_prefix("Hardware wallet: ") {
        return coincube_ui::localized_format!(
            "Hardware wallet: {}",
            "Cartera de hardware: {}",
            shown_notice(error)
        );
    }
    if let Some(error) = english.strip_prefix("Could not expand the wallet history search: ") {
        return coincube_ui::localized_format!(
            "Could not expand the wallet history search: {}",
            "No se pudo ampliar la búsqueda del historial de la cartera: {}",
            shown_notice(error)
        );
    }
    if let Some(error) = english.strip_prefix("Could not read signed PSBT: ") {
        return coincube_ui::localized_format!(
            "Could not read signed PSBT: {}",
            "No se pudo leer la PSBT firmada: {}",
            error
        );
    }
    if let Some(missing) = english
        .strip_prefix("Insufficient funds: ")
        .and_then(|rest| rest.strip_suffix(" more satoshis are needed."))
    {
        return coincube_ui::localized_format!(
            "Insufficient funds: {missing} more satoshis are needed.",
            "Fondos insuficientes: faltan {missing} satoshis."
        );
    }
    coincube_ui::locale::notice(english.to_owned())
}

static NEXT_PANEL_GENERATION: AtomicU64 = AtomicU64::new(1);

fn fresh_generation() -> u64 {
    NEXT_PANEL_GENERATION.fetch_add(1, Ordering::SeqCst)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Connect,
    Public,
    Esplora,
    Electrum,
}

impl Provider {
    fn label(self) -> &'static str {
        match self {
            Self::Connect => "COINCUBE Connect",
            Self::Public => "Public Esplora (mempool.space)",
            Self::Esplora => "My Esplora server",
            Self::Electrum => "My Electrum server",
        }
    }
}

fn public_server_label(endpoint: &str) -> String {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return localized_copy("Configured server").into();
    };
    let Some(host) = url.host_str() else {
        return localized_copy("Configured server").into();
    };
    let port = url
        .port()
        .map_or_else(String::new, |port| format!(":{port}"));
    format!("{}://{host}{port}", url.scheme())
}

/// Recovery words (and passphrase) derive exactly `expected`: its
/// fingerprint, origin and xpub, found in both descriptor branches.
fn check_words(
    descriptor: &ImportedWalletDescriptor,
    expected: &coincube_core::imported_keys::ImportedAccount,
    words: zeroize::Zeroizing<String>,
    passphrase: zeroize::Zeroizing<String>,
) -> Result<(), String> {
    let key = coincube_core::imported_keys::verify_imported_account(
        descriptor,
        &expected.origin,
        words,
        passphrase,
    )
    .map_err(|error| error.to_string())?;
    if key.account() != expected {
        return Err(
            coincube_core::imported_keys::ImportedKeyError::KeyDoesNotMatchWallet.to_string(),
        );
    }
    Ok(())
}

fn wallet_summary(descriptor: &ImportedWalletDescriptor) -> (&'static str, String, String) {
    let receive = descriptor.receive_descriptor().to_string();
    let script = if receive.starts_with("sh(wpkh(") {
        localized_copy("Nested SegWit")
    } else if receive.starts_with("wpkh(") {
        localized_copy("Native SegWit")
    } else if receive.starts_with("pkh(") {
        localized_copy("Legacy")
    } else {
        localized_copy("Multisig SegWit")
    };
    let mut fingerprints = BTreeSet::new();
    let mut signer_keys = 0usize;
    descriptor.receive_descriptor().for_each_key(|key| {
        signer_keys += 1;
        if let DescriptorPublicKey::XPub(xpub) = key {
            if let Some((fingerprint, _)) = &xpub.origin {
                fingerprints.insert(fingerprint.to_string());
            }
        }
        true
    });
    let threshold = receive
        .split_once("multi(")
        .and_then(|(_, rest)| rest.split_once(','))
        .map_or_else(
            || localized_copy("1 signer").to_string(),
            |(required, _)| {
                coincube_ui::localized_format!(
                    "{required} of {signer_keys} signers",
                    "{required} de {signer_keys} firmantes"
                )
            },
        );
    let fingerprints = fingerprints.into_iter().collect::<Vec<_>>().join(", ");
    (script, threshold, fingerprints)
}

fn sample_receive_address(descriptor: &ImportedWalletDescriptor) -> Result<String, String> {
    let script = descriptor
        .receive_descriptor()
        .at_derivation_index(0)
        .map_err(|error| error.to_string())?
        .script_pubkey();
    Address::from_script(&script, descriptor.chain().bitcoin_network())
        .map(|address| address.to_string())
        .map_err(|error| error.to_string())
}

fn unsigned_construction(psbt: &Psbt) -> Psbt {
    let mut original = psbt.clone();
    for input in &mut original.inputs {
        input.partial_sigs.clear();
        input.tap_key_sig = None;
        input.tap_script_sigs.clear();
        input.final_script_sig = None;
        input.final_script_witness = None;
    }
    original
}

fn checked_draft(
    wallet: &ImportedWalletDescriptor,
    psbt: Psbt,
    warnings: Vec<String>,
) -> Result<ImportedDraft, String> {
    let original = unsigned_construction(&psbt);
    let secp = secp256k1::Secp256k1::verification_only();
    let signatures_per_input =
        verify_imported_signatures(wallet, &original, &psbt, &secp).map_err(|e| e.to_string())?;
    let final_txid = match finalize_imported_spend(wallet, &original, &psbt, &secp) {
        Ok(verified) => Some(verified.transaction().compute_txid()),
        Err(ImportedFinalizeError::Signatures(FinalizeError::Unsatisfied)) => None,
        Err(error) => return Err(error.to_string()),
    };
    Ok(ImportedDraft {
        psbt,
        signatures_per_input,
        final_txid,
        warnings,
    })
}

fn import_signed_psbt(
    wallet: &ImportedWalletDescriptor,
    current: &Psbt,
    text: &str,
) -> Result<(Psbt, Vec<usize>, Option<Txid>), String> {
    if text.len() > crate::services::psbt_file::MAX_FILE_BYTES as usize * 2 {
        return Err("The signed PSBT is too large.".into());
    }
    let signed = Psbt::from_str(text.trim()).map_err(|_| "Paste a base64 Bitcoin PSBT.")?;
    import_signed_psbt_value(wallet, current, &signed)
}

fn import_signed_psbt_value(
    wallet: &ImportedWalletDescriptor,
    current: &Psbt,
    signed: &Psbt,
) -> Result<(Psbt, Vec<usize>, Option<Txid>), String> {
    if signed.inputs.iter().any(|input| {
        input.final_script_sig.is_some()
            || input.final_script_witness.is_some()
            || input.tap_key_sig.is_some()
            || !input.tap_script_sigs.is_empty()
    }) {
        return Err("Import an unfinalized Bitcoin PSBT containing only signer signatures.".into());
    }
    let original = unsigned_construction(current);
    let secp = secp256k1::Secp256k1::verification_only();
    let incoming = verify_imported_signatures(wallet, &original, signed, &secp)
        .map_err(|error| error.to_string())?;
    if incoming.iter().all(|count| *count == 0) {
        return Err("The PSBT has no valid signer signatures.".into());
    }
    let mut combined = current.clone();
    for (target, source) in combined.inputs.iter_mut().zip(&signed.inputs) {
        for (key, signature) in &source.partial_sigs {
            if target
                .partial_sigs
                .get(key)
                .is_some_and(|previous| previous != signature)
            {
                return Err("A signer returned a conflicting signature for this wallet.".into());
            }
            target.partial_sigs.insert(*key, *signature);
        }
    }
    let checked = checked_draft(wallet, combined, Vec::new())?;
    Ok((
        checked.psbt,
        checked.signatures_per_input,
        checked.final_txid,
    ))
}

/// Typed recovery words or passphrase on their way from the input to the
/// Keys step. Debug never prints them.
#[derive(Clone)]
pub struct SecretInput(zeroize::Zeroizing<String>);

impl std::fmt::Debug for SecretInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretInput([REDACTED])")
    }
}

impl SecretInput {
    pub fn new(value: String) -> Self {
        Self(zeroize::Zeroizing::new(value))
    }
}

#[derive(Debug, Clone)]
pub enum ImportMessage {
    Source(wallet_source_form::Message),
    BirthdayEdited(String),
    BirthdayCalendarOpen,
    BirthdayCalendarCancel,
    BirthdaySelected(chrono::NaiveDate),
    ScanFromBeginning(bool),
    LookaheadEdited(String),
    ProviderSelected(Provider),
    EndpointEdited(String),
    Review,
    Edit,
    Confirm,
    Retry,
    Refresh,
    RetrySynchronization,
    PollProgress,
    ExpandDiscovery,
    Receive,
    CopyAddress,
    ToggleDetails,
    Abandon,
    /// Scan → Done while the scan carries on.
    ContinueFromScan,
    /// Close, or Done's Open Cube: back to the Cube.
    LeaveJourney,
    /// Keys: where signer `n`'s key is.
    KeySourcePicked(usize, KeySource),
    KeyWordsEdited(usize, SecretInput),
    KeyPassphraseEdited(usize, SecretInput),
    VerifyWords(usize),
    CheckDevice(usize),
    /// Keys → Review.
    KeysDone,
    /// Review → Keys.
    BackToKeys,
    /// Store the verified words again after a failed attempt.
    RetryStoreKeys,
    /// Vault settings: add recovery words to the attached wallet.
    AddKeys,
    /// The Vault page's Continue import: back to Import or restore.
    ResumeJourney,
    ShowSend,
    CloseSend,
    RecipientEdited(String),
    AmountEdited(String),
    FeerateEdited(String),
    CreateDraft,
    SelectDraft(Txid),
    CopyPsbt,
    SignedPsbtEdited(String),
    ImportSignedPsbt,
    /// Carries the language of the view that offered it, for the file
    /// dialog's title.
    SavePsbt(coincube_ui::locale::Language),
    LoadSignedPsbt(coincube_ui::locale::Language),
    Broadcast,
    Hardware(HardwareWalletMessage),
    SignWithDevice(String),
}

type RefreshSnapshot = (
    model::GetInfoResult,
    model::ListCoinsResult,
    Vec<model::HistoryTransaction>,
    Vec<ListSpendEntry>,
    BitcoinImportStage,
);

type SignedSpend = (Psbt, Vec<usize>, Option<Txid>);

#[derive(Debug)]
pub enum ImportEvent {
    Start,
    /// (generation, signer, check id, result)
    WordsVerified(u64, usize, u64, Result<(), String>),
    DeviceVerified(u64, usize, u64, Result<(), String>),
    /// (storage epoch, result)
    KeysStored(u64, Result<usize, String>),
    /// (storage epoch, local key records for this wallet)
    KeysInventory(u64, Result<usize, String>),
    Installed(u64, Result<(), String>),
    Resumed(u64, Result<Option<settings::WalletId>, String>),
    Opened(u64, Result<Arc<dyn Daemon + Send + Sync>, String>),
    Refreshed(u64, Result<RefreshSnapshot, String>),
    SynchronizationRequested(u64, Result<(), String>),
    ProgressChecked(u64, Result<model::GetInfoResult, String>),
    Address(u64, Result<model::GetAddressResult, String>),
    Abandoned(u64, Result<(), String>),
    DraftCreated(u64, Result<(Psbt, Vec<String>), String>),
    SignedSaved(u64, Txid, Result<SignedSpend, String>),
    Broadcasted(u64, Result<(Txid, bool), String>),
    HardwareSigned(u64, Txid, Result<SignedSpend, String>),
    Expanded(u64, Result<u32, String>),
    FileSaved(u64, Txid, Result<bool, String>),
    FileLoaded(u64, Txid, Result<Option<SignedSpend>, String>),
}

struct ImportedDraft {
    psbt: Psbt,
    signatures_per_input: Vec<usize>,
    final_txid: Option<Txid>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ScanDisplay {
    Starting,
    Scanning {
        checked: u64,
        expected: u64,
        progress: f64,
    },
    Saving,
    Retrying,
    NeedsAttention,
}

impl ScanDisplay {
    fn from_info(info: Option<&model::GetInfoResult>, sync_failed: bool) -> Self {
        if sync_failed {
            return Self::Retrying;
        }
        let Some(info) = info else {
            return Self::Starting;
        };
        if info.chain_divergence || info.refused_reorg_depth.is_some() {
            return Self::NeedsAttention;
        }
        if info.history_sync.full_scan_in_progress {
            let history = &info.history_sync;
            if history.addresses_expected > 0 {
                return Self::Scanning {
                    checked: history.addresses_checked,
                    expected: history.addresses_expected,
                    progress: history.full_scan_progress().unwrap_or(0.0),
                };
            }
            return Self::Starting;
        }
        if info.history_sync.last_failure.is_some() {
            return Self::Retrying;
        }
        if info.history_sync.full_scan_completed_at.is_some()
            || info
                .imported_wallet
                .as_ref()
                .is_some_and(|status| status.history_complete)
        {
            Self::Saving
        } else {
            Self::Starting
        }
    }
}

/// The import's steps after the Cube, on Import or restore's stepper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JourneyStep {
    Source,
    Keys,
    Review,
    Scan,
    Done,
}

impl JourneyStep {
    fn label(self) -> &'static str {
        match self {
            Self::Source => "Source",
            Self::Keys => "Keys",
            Self::Review => "Review",
            Self::Scan => "Scan",
            Self::Done => "Done",
        }
    }
}

pub struct ImportedBitcoinPanel {
    directory: NetworkDirectory,
    cube_id: String,
    import: BitcoinImport,
    source: WalletSourceForm,
    birthday: String,
    birthday_calendar_open: bool,
    scan_from_beginning: bool,
    lookahead: String,
    provider: Provider,
    provider_known: bool,
    endpoint: String,
    review: Option<ImportedWalletAttachment>,
    review_connect_epoch: Option<u64>,
    daemon: Option<Arc<dyn Daemon + Send + Sync>>,
    /// True only before the App adopts a newly installed daemon. Once bound,
    /// the App owns shutdown and cache polling for the shared Vault runtime.
    owns_daemon: bool,
    generation: u64,
    generation_gate: Arc<AtomicU64>,
    busy: bool,
    address_reserving: bool,
    sync_requesting: bool,
    progress_polling: bool,
    snapshot_failed: bool,
    error: Option<String>,
    sync_failed: bool,
    info: Option<model::GetInfoResult>,
    coins: Option<model::ListCoinsResult>,
    history: Vec<model::HistoryTransaction>,
    address: Option<model::GetAddressResult>,
    show_details: bool,
    abandoned: bool,
    send_recipient: String,
    send_amount: String,
    send_feerate: String,
    signed_psbt_text: String,
    spend_drafts: Vec<ListSpendEntry>,
    draft: Option<ImportedDraft>,
    last_broadcast_txid: Option<Txid>,
    broadcast_notice: Option<String>,
    file_notice: Option<String>,
    connect_token: Option<String>,
    hardware_root: Option<CoincubeDirectory>,
    hardware_wallets: Option<HardwareWallets>,
    /// Import or restore (P-Restore) holds the window from Source until the
    /// user leaves Done. It opens for a Cube with no wallet attached yet,
    /// new or resumed, and stays through this session's scan.
    journey: bool,
    /// The user moved on from Scan to Done before the scan finished.
    scan_continued: bool,
    /// The Keys step's signers, from the reviewed descriptor.
    keys: Option<KeysState>,
    /// Keys → Review happened for the current review.
    keys_confirmed: bool,
    /// Storing the verified recovery words in this Cube.
    keys_stored: KeysStored,
    /// Import or restore reopened at Keys to add words to the attached
    /// wallet.
    adding_keys: bool,
    /// Local key records for the attached wallet, once read.
    stored_keys: Option<usize>,
    /// Key storage and inventory reads end only with the panel. A refresh
    /// or a wider history search must not drop their results.
    keys_gate: Arc<AtomicU64>,
}

/// Where storing the Keys step's recovery words stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum KeysStored {
    /// Nothing to store yet, or no recovery words were given.
    None,
    Storing,
    Stored(usize),
    Failed(String),
}

impl ImportedBitcoinPanel {
    fn chain(&self) -> ChainId {
        self.import
            .attachment()
            .map(|w| w.descriptor.chain())
            .unwrap_or(ChainId::Bitcoin)
    }
    pub fn new(directory: NetworkDirectory, cube_id: String, import: BitcoinImport) -> Self {
        let review = import.attachment().cloned();
        let generation = fresh_generation();
        Self {
            directory,
            cube_id,
            source: WalletSourceForm::default(),
            birthday: String::new(),
            birthday_calendar_open: false,
            scan_from_beginning: true,
            lookahead: "200".into(),
            provider: Provider::Public,
            provider_known: true,
            endpoint: String::new(),
            review,
            review_connect_epoch: None,
            daemon: None,
            owns_daemon: false,
            generation,
            generation_gate: Arc::new(AtomicU64::new(generation)),
            busy: false,
            address_reserving: false,
            sync_requesting: false,
            progress_polling: false,
            snapshot_failed: false,
            error: None,
            sync_failed: false,
            info: None,
            coins: None,
            history: Vec::new(),
            address: None,
            show_details: false,
            abandoned: false,
            send_recipient: String::new(),
            send_amount: String::new(),
            send_feerate: "5".into(),
            signed_psbt_text: String::new(),
            spend_drafts: Vec::new(),
            draft: None,
            last_broadcast_txid: None,
            broadcast_notice: None,
            file_notice: None,
            connect_token: None,
            hardware_root: None,
            hardware_wallets: None,
            journey: import.active_attachment().is_none(),
            scan_continued: false,
            // A resumed install has lost the words it was given; its keys are
            // asked for again before Retry.
            keys: import
                .attachment()
                .filter(|_| import.active_attachment().is_none())
                // No device access yet, so they start on recovery words.
                .map(|wallet| KeysState::new(&wallet.descriptor).starting_on(KeySource::Words)),
            keys_confirmed: false,
            keys_stored: KeysStored::None,
            adding_keys: false,
            stored_keys: None,
            keys_gate: Arc::new(AtomicU64::new(fresh_generation())),
            import,
        }
    }

    pub fn set_hardware_root(&mut self, root: crate::dir::CoincubeDirectory) {
        self.source.set_hardware_root(root.clone());
        self.hardware_root = Some(root);
    }

    pub fn set_connect_client(&mut self, client: Option<&CoincubeClient>) {
        self.connect_token = client.and_then(CoincubeClient::token).map(str::to_owned);
    }

    pub fn is_ready(&self) -> bool {
        self.import.is_ready()
            && self.coins.is_some()
            && self.info.as_ref().is_some_and(|info| {
                info.imported_wallet.as_ref().is_some_and(|status| {
                    status.history_complete
                        && info.chain_id == Some(self.chain())
                        && info.network == self.chain().bitcoin_network()
                        && status.chain_id == self.chain()
                        && status.discovery_target >= status.initial_lookahead
                        && status.discovery_target <= 10_000
                        // The daemon attests to the initial/restored/wider full
                        // scan. A newly reserved address changes allocation,
                        // not the completed historical discovery target.
                        && status.receive_scanned >= status.discovery_target
                        && status.change_scanned >= status.discovery_target
                        && info.sync.is_finite()
                        && info.sync >= 1.0
                        && info.rescan_progress.is_none()
                        && !info.history_sync.full_scan_in_progress
                        && info.history_sync.last_failure.is_none()
                        && !info.chain_divergence
                        && info.refused_reorg_depth.is_none()
                        && self.import.active_attachment().is_some_and(|wallet| {
                            wallet.wallet_id.descriptor_checksum
                                == format!("imported-v1-{}", status.wallet_identity)
                                && wallet.birthday == status.birthday
                                && status.initial_lookahead >= wallet.initial_lookahead
                        })
                })
            })
    }

    pub fn authoritative_balance(&self) -> Option<coincube_core::miniscript::bitcoin::Amount> {
        self.is_ready()
            .then(|| {
                self.coins.as_ref().map(|coins| {
                    // Match the ordinary Vault balance: pending external
                    // deposits remain pending, while self-change is owned.
                    super::coins_summary(&coins.coins, 0, None).0
                })
            })
            .flatten()
    }

    /// Last durably allocated addresses, only after the daemon proves it is
    /// reporting for this exact Cube attachment. Historical scan bounds are
    /// deliberately not used as address-allocation state.
    pub fn verified_address_state(&self) -> Option<(u32, u32)> {
        let wallet = self.import.active_attachment()?;
        let info = self.info.as_ref()?;
        let status = info.imported_wallet.as_ref()?;
        (info.chain_id == Some(wallet.descriptor.chain())
            && status.chain_id == wallet.descriptor.chain()
            && wallet.wallet_id.descriptor_checksum
                == format!("imported-v1-{}", status.wallet_identity)
            && wallet.birthday == status.birthday
            && status.initial_lookahead >= wallet.initial_lookahead)
            .then_some((info.receive_index, info.change_index))
    }

    fn info_matches_attachment(&self, info: &model::GetInfoResult) -> bool {
        let (Some(wallet), Some(status)) = (
            self.import.active_attachment(),
            info.imported_wallet.as_ref(),
        ) else {
            return false;
        };
        matches!(&info.descriptors.main, WalletDescriptor::Imported(descriptor) if descriptor == &wallet.descriptor)
            && info.network == wallet.descriptor.chain().bitcoin_network()
            && info.chain_id == Some(wallet.descriptor.chain())
            && status.chain_id == wallet.descriptor.chain()
            && wallet.wallet_id.descriptor_checksum
                == format!("imported-v1-{}", status.wallet_identity)
            && status.birthday == wallet.birthday
            && status.initial_lookahead >= wallet.initial_lookahead
            && status.discovery_target >= status.initial_lookahead
            && status.discovery_target <= 10_000
    }

    pub fn backup_daemon(&self) -> Option<Arc<dyn Daemon + Send + Sync>> {
        self.import.active_attachment()?;
        self.daemon.clone()
    }

    pub fn runtime_daemon(&self) -> Option<Arc<dyn Daemon + Send + Sync>> {
        self.import.active_attachment()?;
        self.daemon.clone()
    }

    /// Bind the daemon opened by the ordinary Vault loader. The panel shares
    /// that handle for import progress and PSBT operations; App owns shutdown.
    pub fn bind_runtime(&mut self, daemon: Arc<dyn Daemon + Send + Sync>) -> Task<Message> {
        if self
            .daemon
            .as_ref()
            .is_some_and(|current| !Arc::ptr_eq(current, &daemon))
        {
            self.error = Some("A different Bitcoin wallet is already open for this Cube.".into());
            return Task::none();
        }
        // An external daemon does not expose Config. Its installed attachment
        // still has the exact persisted settings used by the loader.
        let persisted_config = if daemon.config().is_none() {
            self.import.active_attachment().and_then(|wallet| {
                let path = self
                    .directory
                    .coincubed_data_directory(&wallet.wallet_id)
                    .path()
                    .join("daemon.toml");
                Config::from_file(Some(path)).ok()
            })
        } else {
            None
        };
        if let Some(config) = daemon.config().or(persisted_config.as_ref()) {
            if let Err(error) = self.hydrate_provider_from_config(config) {
                log::warn!("Imported Bitcoin provider metadata unavailable: {error}");
                self.provider_known = false;
                self.endpoint.clear();
            }
        } else {
            self.provider_known = false;
            self.endpoint.clear();
        }
        self.generation = fresh_generation();
        self.generation_gate
            .store(self.generation, Ordering::SeqCst);
        self.busy = false;
        self.address_reserving = false;
        self.sync_requesting = false;
        self.progress_polling = false;
        self.snapshot_failed = false;
        self.daemon = Some(daemon);
        self.owns_daemon = false;
        if self.import.active_attachment().is_some() {
            self.refresh()
        } else {
            Task::none()
        }
    }

    /// Hand the panel-started daemon to App after an exact-Cube attachment is
    /// validated. Both sides retain an Arc, but only App will stop it.
    pub fn handoff_runtime(&mut self) -> Option<Arc<dyn Daemon + Send + Sync>> {
        let daemon = self.runtime_daemon()?;
        self.owns_daemon = false;
        Some(daemon)
    }

    /// Seed the normal Vault cache only from a complete, identity-bound scan.
    /// A partial scan never becomes a zero balance in the shared dashboard.
    pub fn sync_cache(&self, cache: &mut Cache) -> bool {
        if !self.is_ready() {
            return false;
        }
        let (Some(info), Some(coins)) = (&self.info, &self.coins) else {
            return false;
        };
        cache.daemon_cache = crate::app::cache::DaemonCache {
            blockheight: info.block_height,
            coins: coins.coins.clone(),
            rescan_progress: info.rescan_progress,
            sync_progress: info.sync,
            last_poll_timestamp: info.last_poll_timestamp,
            history_sync: info.history_sync.clone(),
            last_tick: std::time::Instant::now(),
        };
        true
    }

    /// Tests: the attached policy's full scan finished at `block_height`
    /// with `coins`, as a refresh after the daemon attests it reports.
    #[cfg(test)]
    pub(crate) fn complete_scan_for_tests(&mut self, block_height: i32, coins: Vec<model::Coin>) {
        let wallet = self
            .import
            .active_attachment()
            .expect("an attached policy")
            .clone();
        let descriptor = wallet.descriptor.clone();
        self.import.stage = BitcoinImportStage::Ready {
            wallet: wallet.clone(),
        };
        self.info = Some(model::GetInfoResult {
            version: "test".into(),
            network: self.chain().bitcoin_network(),
            chain_id: Some(self.chain()),
            block_height,
            sync: 1.0,
            descriptors: coincubed::commands::GetInfoDescriptors {
                main: WalletDescriptor::Imported(descriptor.clone()),
            },
            imported_wallet: Some(coincubed::commands::ImportedWalletSync {
                wallet_identity: descriptor.identity(self.chain()).unwrap().to_string(),
                chain_id: self.chain(),
                birthday: wallet.birthday,
                initial_lookahead: wallet.initial_lookahead,
                discovery_target: wallet.initial_lookahead,
                receive_scanned: wallet.initial_lookahead,
                change_scanned: wallet.initial_lookahead,
                history_complete: true,
            }),
            rescan_progress: None,
            refused_reorg_depth: None,
            chain_divergence: false,
            timestamp: 0,
            last_poll_timestamp: Some(1),
            history_sync: Default::default(),
            receive_index: 0,
            change_index: 0,
        });
        self.coins = Some(coincubed::commands::ListCoinsResult { coins });
    }

    pub fn handles_route(&self, menu: &Menu) -> bool {
        match menu {
            Menu::Vault(VaultSubMenu::Recovery) => false,
            Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::LocalSigning))) => false,
            Menu::Vault(_) if !self.is_ready() => true,
            Menu::Vault(
                VaultSubMenu::Send | VaultSubMenu::PSBTs(_) | VaultSubMenu::Settings(_),
            ) => true,
            _ => false,
        }
    }

    pub fn enter_route(&mut self, menu: &Menu) -> Task<Message> {
        if !self.handles_route(menu) {
            return Task::none();
        }
        if !self.is_ready() {
            return if self.import.active_attachment().is_some() && self.daemon.is_none() {
                self.open_daemon()
            } else if self.daemon.is_some() {
                self.refresh()
            } else {
                Task::none()
            };
        }
        match menu {
            Menu::Vault(VaultSubMenu::Send | VaultSubMenu::PSBTs(_)) => {
                if self.chain().is_blake2b() {
                    self.error = Some(
                        "Sending from paired imported Bitcoin Blake2b wallets is not available."
                            .into(),
                    );
                    return Task::none();
                }
                self.error = self.ensure_hardware_wallets().err();
                if let Menu::Vault(VaultSubMenu::PSBTs(Some(id))) = menu {
                    self.select_saved_draft(*id);
                }
                self.refresh()
            }
            _ => self.refresh(),
        }
    }

    pub fn abandoned(&self) -> bool {
        self.abandoned
    }

    fn synchronization_action_label(&self) -> &'static str {
        if matches!(
            ScanDisplay::from_info(self.info.as_ref(), self.sync_failed),
            ScanDisplay::Retrying | ScanDisplay::NeedsAttention
        ) {
            "Retry synchronization"
        } else {
            "Refresh status"
        }
    }

    fn import_descriptor(&self) -> Result<ImportedWalletDescriptor, String> {
        self.import
            .active_attachment()
            .map(|wallet| wallet.descriptor.clone())
            .ok_or_else(|| "The imported wallet is not installed yet.".into())
    }

    fn send_request(&self) -> Result<(Address<address::NetworkUnchecked>, u64, u64), String> {
        let unchecked = Address::from_str(self.send_recipient.trim())
            .map_err(|_| "Enter a valid Bitcoin address.".to_string())?;
        unchecked
            .clone()
            .require_network(self.chain().bitcoin_network())
            .map_err(|_| "The recipient address belongs to another Bitcoin network.".to_string())?;
        let amount = self
            .send_amount
            .trim()
            .parse::<u64>()
            .map_err(|_| "Enter the amount in satoshis.".to_string())?;
        if amount == 0 {
            return Err("The amount must be greater than zero.".into());
        }
        let feerate = self
            .send_feerate
            .trim()
            .parse::<u64>()
            .map_err(|_| "Enter a fee rate in sat/vB.".to_string())?;
        if feerate == 0 {
            return Err("The fee rate must be at least 1 sat/vB.".into());
        }
        Ok((unchecked, amount, feerate))
    }

    fn select_saved_draft(&mut self, id: Txid) {
        self.draft = None;
        self.file_notice = None;
        self.last_broadcast_txid = None;
        self.broadcast_notice = None;
        self.signed_psbt_text.clear();
        let Some(entry) = self
            .spend_drafts
            .iter()
            .find(|entry| entry.psbt.unsigned_tx.compute_txid() == id)
        else {
            self.error = Some("This PSBT is no longer available.".into());
            return;
        };
        match self
            .import_descriptor()
            .and_then(|wallet| checked_draft(&wallet, entry.psbt.clone(), Vec::new()))
        {
            Ok(draft) => {
                self.last_broadcast_txid = entry.broadcast_txid;
                self.draft = Some(draft);
                self.signed_psbt_text.clear();
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn ensure_hardware_wallets(&mut self) -> Result<(), String> {
        if self.hardware_wallets.is_some() {
            return Ok(());
        }
        let root = self
            .hardware_root
            .clone()
            .ok_or("Hardware wallet access is unavailable.")?;
        let wallet = self.import_descriptor()?;
        let policy = imported_policy(&wallet).map_err(|error| error.to_string())?;
        self.hardware_wallets = Some(
            HardwareWallets::new(root, self.chain().bitcoin_network())
                .with_public_policy(policy.name().to_owned(), policy.descriptor().to_owned()),
        );
        Ok(())
    }

    fn invalidate(&mut self) {
        self.birthday_calendar_open = false;
        self.generation = fresh_generation();
        self.generation_gate
            .store(self.generation, Ordering::SeqCst);
        self.progress_polling = false;
        self.sync_requesting = false;
        self.snapshot_failed = false;
        self.address_reserving = false;
        self.review = None;
        self.review_connect_epoch = None;
        self.keys = None;
        self.keys_confirmed = false;
        if self.import.active_attachment().is_none() {
            // Bound to the reviewed descriptor's policy.
            self.hardware_wallets = None;
        }
        self.error = None;
    }

    fn parse_review(&self) -> Result<ImportedWalletAttachment, String> {
        let (receive, change) = if self.source.source() == Source::Hardware {
            let account = self
                .source
                .hardware_account()
                .ok_or("Connect and read a Bitcoin account from your hardware wallet first.")?;
            (
                account.external.canonical(),
                Some(account.internal.canonical()),
            )
        } else {
            let change = self.source.internal().trim();
            (
                self.source.external().trim().to_owned(),
                (!change.is_empty()).then(|| change.to_owned()),
            )
        };
        let descriptor =
            ImportedWalletDescriptor::parse(&receive, change.as_deref(), ChainId::Bitcoin)
                .map_err(|e| e.to_string())?;
        let birthday = if self.scan_from_beginning {
            0
        } else {
            let date = chrono::NaiveDate::parse_from_str(self.birthday.trim(), "%Y-%m-%d")
                .map_err(|_| "Enter the wallet creation date in YYYY-MM-DD format.".to_string())?;
            birthday_timestamp(date)?
        };
        let lookahead = self.lookahead.trim().parse::<u32>().map_err(|_| {
            "Enter an initial discovery window between 1 and 10,000 addresses per branch."
                .to_string()
        })?;
        ImportedWalletAttachment::new(descriptor, ChainId::Bitcoin, birthday, lookahead)
    }

    fn backend(&self, client: Option<&CoincubeClient>) -> Result<BitcoinBackend, String> {
        let backend = match self.provider {
            Provider::Connect => {
                let token = client
                    .and_then(CoincubeClient::token)
                    .ok_or("Sign in to Connect or choose another Bitcoin server.")?;
                BitcoinBackend::Esplora(crate::installer::connect_esplora_config(
                    ChainId::Bitcoin,
                    token,
                ))
            }
            Provider::Public => BitcoinBackend::Esplora(EsploraConfig {
                addr: crate::installer::public_esplora_url(
                    coincube_core::miniscript::bitcoin::Network::Bitcoin,
                ),
                token: None,
                fallback_addr: crate::installer::public_esplora_fallback_url(
                    coincube_core::miniscript::bitcoin::Network::Bitcoin,
                ),
                fallback_token: None,
                secondary_fallback_addr: None,
                secondary_fallback_token: None,
            }),
            Provider::Esplora => {
                let endpoint = self.endpoint.trim();
                if !endpoint.starts_with("https://") && !endpoint.starts_with("http://localhost") {
                    return Err("Enter an HTTPS Esplora URL, or a localhost HTTP URL.".into());
                }
                BitcoinBackend::Esplora(EsploraConfig {
                    addr: endpoint.into(),
                    token: None,
                    fallback_addr: None,
                    fallback_token: None,
                    secondary_fallback_addr: None,
                    secondary_fallback_token: None,
                })
            }
            Provider::Electrum => {
                let endpoint = self.endpoint.trim();
                if !endpoint.starts_with("ssl://") && !endpoint.starts_with("tcp://127.0.0.1") {
                    return Err("Enter an SSL Electrum URL, or a local TCP URL.".into());
                }
                BitcoinBackend::Electrum(ElectrumConfig {
                    addr: endpoint.into(),
                    validate_domain: true,
                })
            }
        };
        Ok(backend)
    }

    fn config(
        &self,
        wallet: &ImportedWalletAttachment,
        client: Option<&CoincubeClient>,
    ) -> Result<Config, String> {
        let backend = self.backend(client)?;
        let interval = if self.provider == Provider::Public {
            coincubed::config::ESPLORA_POLL_INTERVAL_SECS
        } else {
            30
        };
        Ok(Config::new(
            BitcoinConfig::new(ChainId::Bitcoin, Duration::from_secs(interval)),
            Some(backend),
            log::LevelFilter::Info,
            WalletDescriptor::Imported(wallet.descriptor.clone()),
            self.directory.coincubed_data_directory(&wallet.wallet_id),
        ))
    }

    pub fn update_view(
        &mut self,
        message: ImportMessage,
        client: Option<CoincubeClient>,
        connect_epoch: u64,
    ) -> Task<Message> {
        if self.chain().is_blake2b() && matches!(message, ImportMessage::ExpandDiscovery) {
            self.error = Some("Search more addresses in the original Bitcoin Cube.".into());
            return Task::none();
        }
        self.set_connect_client(client.as_ref());
        if self.chain().is_blake2b()
            && matches!(
                message,
                ImportMessage::ShowSend
                    | ImportMessage::CreateDraft
                    | ImportMessage::SelectDraft(_)
                    | ImportMessage::CopyPsbt
                    | ImportMessage::SignedPsbtEdited(_)
                    | ImportMessage::ImportSignedPsbt
                    | ImportMessage::SavePsbt(_)
                    | ImportMessage::LoadSignedPsbt(_)
                    | ImportMessage::Broadcast
                    | ImportMessage::Hardware(_)
                    | ImportMessage::SignWithDevice(_)
            )
        {
            self.error = Some(
                "Sending from paired imported Bitcoin Blake2b wallets is not available.".into(),
            );
            return Task::none();
        }
        // Moving between Import or restore and the Cube changes only what is
        // shown, so it stays open while a refresh keeps the panel busy.
        let presentation = matches!(
            &message,
            ImportMessage::LeaveJourney
                | ImportMessage::ResumeJourney
                | ImportMessage::ContinueFromScan
        );
        if self.busy
            && !presentation
            && (!matches!(&message, ImportMessage::PollProgress) || self.address_reserving)
        {
            return Task::none();
        }
        if matches!(&self.import.stage, BitcoinImportStage::Installing { .. })
            && matches!(
                &message,
                ImportMessage::Source(_)
                    | ImportMessage::BirthdayEdited(_)
                    | ImportMessage::BirthdayCalendarOpen
                    | ImportMessage::BirthdayCalendarCancel
                    | ImportMessage::BirthdaySelected(_)
                    | ImportMessage::ScanFromBeginning(_)
                    | ImportMessage::LookaheadEdited(_)
                    | ImportMessage::Edit
                    | ImportMessage::Review
            )
        {
            self.error = Some("This interrupted import is bound to the reviewed wallet. Retry it or return to your Cube.".into());
            return Task::none();
        }
        match message {
            ImportMessage::Source(message) => {
                let task = match message {
                    wallet_source_form::Message::Focus(id) => iced::widget::operation::focus(id),
                    wallet_source_form::Message::SourceSelected(source) => {
                        if self.source.select_source(source) {
                            self.invalidate();
                        }
                        Task::none()
                    }
                    wallet_source_form::Message::ExternalEdited(value) => {
                        self.source.edit_external(value);
                        self.invalidate();
                        Task::none()
                    }
                    wallet_source_form::Message::InternalEdited(value) => {
                        self.source.edit_internal(value);
                        self.invalidate();
                        Task::none()
                    }
                    wallet_source_form::Message::Hardware(message) => {
                        let (task, invalidated) = self.source.update_hardware(message);
                        if invalidated {
                            self.invalidate();
                        }
                        task
                    }
                };
                task.map(|m| {
                    Message::View(view::Message::ImportedBitcoin(ImportMessage::Source(m)))
                })
            }
            ImportMessage::BirthdayEdited(value) => {
                self.birthday = value;
                self.invalidate();
                Task::none()
            }
            ImportMessage::BirthdayCalendarOpen => {
                self.birthday_calendar_open = !self.scan_from_beginning
                    && self.review.is_none()
                    && self.import.active_attachment().is_none();
                Task::none()
            }
            ImportMessage::BirthdayCalendarCancel => {
                self.birthday_calendar_open = false;
                Task::none()
            }
            ImportMessage::BirthdaySelected(date) => {
                if !self.birthday_calendar_open {
                    return Task::none();
                }
                self.birthday_calendar_open = false;
                match birthday_timestamp(date) {
                    Ok(_) => {
                        self.birthday = date.format("%Y-%m-%d").to_string();
                        self.invalidate();
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            ImportMessage::ScanFromBeginning(value) => {
                self.scan_from_beginning = value;
                self.invalidate();
                Task::none()
            }
            ImportMessage::LookaheadEdited(value) => {
                self.lookahead = value;
                self.invalidate();
                Task::none()
            }
            ImportMessage::ProviderSelected(value) => {
                self.provider = value;
                self.provider_known = true;
                if matches!(&self.import.stage, BitcoinImportStage::Installing { .. }) {
                    self.review_connect_epoch =
                        (value == Provider::Connect).then_some(connect_epoch);
                    self.error = None;
                } else {
                    self.invalidate();
                }
                Task::none()
            }
            ImportMessage::EndpointEdited(value) => {
                self.endpoint = value;
                if matches!(&self.import.stage, BitcoinImportStage::Installing { .. }) {
                    self.error = None;
                } else {
                    self.invalidate();
                }
                Task::none()
            }
            ImportMessage::Review => {
                self.birthday_calendar_open = false;
                match self.parse_review().and_then(|wallet| {
                    self.backend(client.as_ref())?;
                    Ok(wallet)
                }) {
                    Ok(wallet) => {
                        let mut keys = KeysState::new(&wallet.descriptor);
                        // A descriptor without signer origins names no key to
                        // check; it goes straight to Review.
                        self.keys_confirmed = keys.slots.is_empty();
                        self.review = Some(wallet);
                        // Keys start on a hardware wallet, so its device list
                        // starts now; without device access they start on
                        // recovery words.
                        if !keys.slots.is_empty() && self.ensure_review_hardware_wallets().is_err()
                        {
                            keys = keys.starting_on(KeySource::Words);
                        }
                        self.keys = Some(keys);
                        self.review_connect_epoch =
                            (self.provider == Provider::Connect).then_some(connect_epoch);
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            ImportMessage::Edit => {
                self.birthday_calendar_open = false;
                self.review = None;
                self.keys = None;
                self.keys_confirmed = false;
                if self.import.active_attachment().is_none() {
                    // Bound to the reviewed descriptor's policy.
                    self.hardware_wallets = None;
                }
                self.error = None;
                Task::none()
            }
            ImportMessage::KeySourcePicked(index, source) => {
                if let Some(slot) = self.key_slot(index) {
                    slot.source = source;
                    slot.check = 0;
                    slot.auto_checked = None;
                    slot.words = zeroize::Zeroizing::new(String::new());
                    slot.passphrase = zeroize::Zeroizing::new(String::new());
                    slot.status = if source == KeySource::Elsewhere {
                        KeyStatus::Verified
                    } else {
                        KeyStatus::Missing
                    };
                }
                if source == KeySource::Hardware {
                    if let Err(error) = self.ensure_review_hardware_wallets() {
                        self.error = Some(error);
                    }
                    // Its device may already be connected.
                    return self.auto_check_devices();
                }
                Task::none()
            }
            ImportMessage::KeyWordsEdited(index, SecretInput(value)) => {
                if let Some(slot) = self.key_slot(index) {
                    slot.words = value;
                    slot.check = 0;
                    slot.status = KeyStatus::Missing;
                }
                Task::none()
            }
            ImportMessage::KeyPassphraseEdited(index, SecretInput(value)) => {
                if let Some(slot) = self.key_slot(index) {
                    slot.passphrase = value;
                    slot.check = 0;
                    slot.status = KeyStatus::Missing;
                }
                Task::none()
            }
            ImportMessage::VerifyWords(index) => self.verify_words(index),
            ImportMessage::CheckDevice(index) => self.check_device(index),
            ImportMessage::KeysDone if self.adding_keys => {
                if self.keys_stored == KeysStored::Storing
                    || !self.keys.as_ref().is_some_and(KeysState::ready_to_add)
                {
                    return Task::none();
                }
                if let Err(error) = self.storage_preflight() {
                    self.error = Some(error);
                    return Task::none();
                }
                self.keys_confirmed = true;
                self.error = None;
                self.store_keys()
            }
            ImportMessage::KeysDone => {
                if self.keys.as_ref().is_some_and(KeysState::ready) {
                    self.keys_confirmed = true;
                    self.error = None;
                }
                Task::none()
            }
            ImportMessage::AddKeys => {
                let Some(wallet) = self.import.active_attachment() else {
                    return Task::none();
                };
                if self.stored_keys != Some(0) || self.adding_keys {
                    return Task::none();
                }
                // Only words are stored, so a device is not offered.
                self.keys = Some(KeysState::new(&wallet.descriptor).starting_on(KeySource::Words));
                self.keys_confirmed = false;
                self.keys_stored = KeysStored::None;
                self.adding_keys = true;
                self.journey = true;
                self.error = None;
                Task::none()
            }
            ImportMessage::BackToKeys => {
                if self.keys.is_some() && !self.writing_setup() {
                    self.keys_confirmed = false;
                }
                Task::none()
            }
            ImportMessage::RetryStoreKeys => {
                if matches!(self.keys_stored, KeysStored::Failed(_)) {
                    self.store_keys()
                } else {
                    Task::none()
                }
            }
            ImportMessage::Confirm | ImportMessage::Retry => {
                if self.provider == Provider::Connect
                    && self
                        .review_connect_epoch
                        .is_some_and(|epoch| epoch != connect_epoch)
                {
                    self.invalidate();
                    self.error = Some("Your Connect session changed. Review the Bitcoin server again before importing.".into());
                    return Task::none();
                }
                if self.busy || self.import.is_ready() {
                    return Task::none();
                }
                let Some(wallet) = self.review.clone() else {
                    self.error = Some("Review a public Bitcoin wallet before importing it.".into());
                    return Task::none();
                };
                if self.keys.is_some() && !self.keys_confirmed {
                    self.error = Some("Add the wallet's keys before importing it.".into());
                    return Task::none();
                }
                // Installing cannot be undone, so the words must be storable
                // before the wallet is attached.
                if let Err(error) = self.storage_preflight() {
                    self.error = Some(error);
                    return Task::none();
                }
                let config = match self.config(&wallet, client.as_ref()) {
                    Ok(config) => config,
                    Err(error) => {
                        self.error = Some(error);
                        return Task::none();
                    }
                };
                self.busy = true;
                self.error = None;
                let generation = self.generation;
                let directory = self.directory.clone();
                let cube_id = self.cube_id.clone();
                let operation_id = self.import.operation_id;
                Task::perform(
                    async move {
                        bitcoin_import::install(&directory, &cube_id, operation_id, wallet, config)
                            .await
                            .map(|_| ())
                    },
                    move |result| {
                        Message::ImportedBitcoin(Box::new(ImportEvent::Installed(
                            generation, result,
                        )))
                    },
                )
            }
            ImportMessage::Refresh => {
                if self.busy {
                    Task::none()
                } else {
                    // This is the deliberate/slow refresh path. A failed full
                    // snapshot must not be retried by each 2-second status tick.
                    self.snapshot_failed = false;
                    if self.daemon.is_some() {
                        self.refresh()
                    } else {
                        self.open_daemon()
                    }
                }
            }
            ImportMessage::RetrySynchronization => self.retry_synchronization(),
            ImportMessage::PollProgress => self.poll_progress(),
            ImportMessage::ExpandDiscovery => {
                let Some(wallet) = self.import.active_attachment().cloned() else {
                    return Task::none();
                };
                if self.verified_address_state().is_none() {
                    self.error =
                        Some("Refresh this wallet before expanding its history search.".into());
                    return Task::none();
                }
                let Some(status) = self
                    .info
                    .as_ref()
                    .and_then(|info| info.imported_wallet.as_ref())
                else {
                    return Task::none();
                };
                let target = status
                    .discovery_target
                    .max(wallet.initial_lookahead)
                    .saturating_add(200)
                    .min(10_000);
                if target <= status.discovery_target {
                    self.error = Some("The maximum discovery range has been reached.".into());
                    return Task::none();
                }
                // Expansion requires the daemon to be stopped even when App
                // owns the shared runtime. Revoke late callbacks first, then
                // stop it in the task before changing its durable scan target.
                self.generation = fresh_generation();
                self.generation_gate
                    .store(self.generation, Ordering::SeqCst);
                self.progress_polling = false;
                self.sync_requesting = false;
                self.address_reserving = false;
                let daemon = self.daemon.take();
                self.owns_daemon = false;
                self.info = None;
                self.coins = None;
                self.history.clear();
                self.address = None;
                self.spend_drafts.clear();
                self.draft = None;
                self.import.stage = BitcoinImportStage::Syncing {
                    wallet: wallet.clone(),
                };
                self.busy = true;
                self.error = None;
                let generation = self.generation;
                let data_dir = self.directory.coincubed_data_directory(&wallet.wallet_id);
                Task::perform(
                    async move {
                        if let Some(daemon) = daemon {
                            daemon.stop().await.map_err(|error| error.to_string())?;
                        }
                        tokio::task::spawn_blocking(move || {
                            let config =
                                Config::from_file(Some(data_dir.path().join("daemon.toml")))
                                    .map_err(|error| error.to_string())?;
                            coincubed::expand_imported_discovery(&config, &data_dir, target)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(|error| error.to_string())?
                    },
                    move |result| {
                        Message::ImportedBitcoin(Box::new(ImportEvent::Expanded(
                            generation, result,
                        )))
                    },
                )
            }
            ImportMessage::Receive => {
                if !self.is_ready() {
                    self.error =
                        Some("Wait for the complete Bitcoin history scan before receiving.".into());
                    return Task::none();
                }
                let Some(daemon) = self.daemon.clone() else {
                    return Task::none();
                };
                let Some(root) = self.hardware_root.clone() else {
                    self.error =
                        Some("Reopen this Cube before reserving a receive address.".into());
                    return Task::none();
                };
                let Some(wallet) = self.import.active_attachment() else {
                    return Task::none();
                };
                let descriptor = WalletDescriptor::Imported(wallet.descriptor.clone());
                // A successful reservation changes the Kit fingerprint. Hide
                // the previous daemon snapshot until its new durable index is
                // read back, even if the address request fails midway.
                self.generation = fresh_generation();
                self.generation_gate
                    .store(self.generation, Ordering::SeqCst);
                self.progress_polling = false;
                self.sync_requesting = false;
                self.info = None;
                self.address_reserving = true;
                self.busy = true;
                let generation = self.generation;
                Task::perform(
                    async move { reserve_receive_address(root, descriptor, daemon).await },
                    move |result| {
                        Message::ImportedBitcoin(Box::new(ImportEvent::Address(generation, result)))
                    },
                )
            }
            ImportMessage::CopyAddress => {
                self.address.as_ref().map_or_else(Task::none, |address| {
                    iced::clipboard::write(address.address.to_string())
                })
            }
            ImportMessage::ToggleDetails => {
                self.show_details = !self.show_details;
                Task::none()
            }
            ImportMessage::ShowSend => {
                if !self.is_ready() {
                    self.error =
                        Some("Wait for the complete Bitcoin history scan before sending.".into());
                    Task::none()
                } else {
                    self.error = self.ensure_hardware_wallets().err();
                    Task::done(Message::View(view::Message::Menu(Menu::Vault(
                        VaultSubMenu::Send,
                    ))))
                }
            }
            ImportMessage::CloseSend => {
                self.error = None;
                Task::done(Message::View(view::Message::Menu(Menu::Vault(
                    VaultSubMenu::Overview,
                ))))
            }
            ImportMessage::RecipientEdited(value) => {
                self.send_recipient = value;
                self.error = None;
                Task::none()
            }
            ImportMessage::AmountEdited(value) => {
                self.send_amount = value;
                self.error = None;
                Task::none()
            }
            ImportMessage::FeerateEdited(value) => {
                self.send_feerate = value;
                self.error = None;
                Task::none()
            }
            ImportMessage::CreateDraft => {
                if !self.is_ready() {
                    self.error = Some(
                        "The complete history scan must finish before a spend can be drafted."
                            .into(),
                    );
                    return Task::none();
                }
                let (address, amount, feerate) = match self.send_request() {
                    Ok(request) => request,
                    Err(error) => {
                        self.error = Some(error);
                        return Task::none();
                    }
                };
                let Some(daemon) = self.daemon.clone() else {
                    return Task::none();
                };
                self.busy = true;
                self.error = None;
                let generation = self.generation;
                Task::perform(
                    async move {
                        let destinations = HashMap::from([(address, amount)]);
                        match daemon
                            .create_spend_tx(&[], &destinations, feerate, None)
                            .await
                            .map_err(|error| error.to_string())?
                        {
                            CreateSpendResult::Success { psbt, warnings } => {
                                daemon
                                    .update_spend_tx(&psbt)
                                    .await
                                    .map_err(|error| error.to_string())?;
                                Ok((psbt, warnings))
                            }
                            CreateSpendResult::InsufficientFunds { missing } => Err(format!(
                                "Insufficient funds: {missing} more satoshis are needed."
                            )),
                        }
                    },
                    move |result| {
                        Message::ImportedBitcoin(Box::new(ImportEvent::DraftCreated(
                            generation, result,
                        )))
                    },
                )
            }
            ImportMessage::SelectDraft(id) => {
                self.select_saved_draft(id);
                Task::none()
            }
            ImportMessage::CopyPsbt => self.draft.as_ref().map_or_else(Task::none, |draft| {
                iced::clipboard::write(draft.psbt.to_string())
            }),
            ImportMessage::SignedPsbtEdited(value) => {
                self.signed_psbt_text = value;
                self.error = None;
                Task::none()
            }
            ImportMessage::ImportSignedPsbt => {
                let Some(draft) = &self.draft else {
                    self.error =
                        Some("Create or select a spend before importing signatures.".into());
                    return Task::none();
                };
                let wallet = match self.import_descriptor() {
                    Ok(wallet) => wallet,
                    Err(error) => {
                        self.error = Some(error);
                        return Task::none();
                    }
                };
                let Some(daemon) = self.daemon.clone() else {
                    return Task::none();
                };
                let current = draft.psbt.clone();
                let draft_id = current.unsigned_tx.compute_txid();
                let text = self.signed_psbt_text.clone();
                self.busy = true;
                let generation = self.generation;
                let gate = self.generation_gate.clone();
                Task::perform(
                    async move {
                        let (psbt, counts, final_txid) =
                            import_signed_psbt(&wallet, &current, &text)?;
                        if gate.load(Ordering::SeqCst) != generation {
                            return Err(
                                "This Cube was closed before the signatures were saved.".into()
                            );
                        }
                        daemon
                            .update_spend_tx(&psbt)
                            .await
                            .map_err(|error| error.to_string())?;
                        Ok((psbt, counts, final_txid))
                    },
                    move |result| {
                        Message::ImportedBitcoin(Box::new(ImportEvent::SignedSaved(
                            generation, draft_id, result,
                        )))
                    },
                )
            }
            ImportMessage::SavePsbt(language) => {
                let Some(draft) = &self.draft else {
                    return Task::none();
                };
                let psbt = draft.psbt.clone();
                let id = psbt.unsigned_tx.compute_txid();
                let generation = self.generation;
                let gate = self.generation_gate.clone();
                self.busy = true;
                self.error = None;
                let title = coincube_ui::locale::render_in(language, || {
                    localized_copy("Save unsigned Bitcoin PSBT").to_owned()
                });
                Task::perform(
                    async move {
                        let name = format!("bitcoin-spend-{}.psbt", &id.to_string()[..12]);
                        let Some(handle) = rfd::AsyncFileDialog::new()
                            .set_title(title)
                            .set_file_name(&name)
                            .save_file()
                            .await
                        else {
                            return Ok(false);
                        };
                        if gate.load(Ordering::SeqCst) != generation {
                            return Err("This Cube was closed before the PSBT was saved.".into());
                        }
                        let bytes = psbt_file::encode(&psbt, psbt_file::Encoding::Binary);
                        if bytes.len() as u64 > psbt_file::MAX_FILE_BYTES {
                            return Err("The PSBT is too large to save.".into());
                        }
                        tokio::fs::write(handle.path(), bytes)
                            .await
                            .map_err(|error| error.to_string())?;
                        Ok(true)
                    },
                    move |result| {
                        Message::ImportedBitcoin(Box::new(ImportEvent::FileSaved(
                            generation, id, result,
                        )))
                    },
                )
            }
            ImportMessage::LoadSignedPsbt(language) => {
                let Some(draft) = &self.draft else {
                    return Task::none();
                };
                let wallet = match self.import_descriptor() {
                    Ok(wallet) => wallet,
                    Err(error) => {
                        self.error = Some(error);
                        return Task::none();
                    }
                };
                let Some(daemon) = self.daemon.clone() else {
                    return Task::none();
                };
                let current = draft.psbt.clone();
                let draft_id = current.unsigned_tx.compute_txid();
                let generation = self.generation;
                let gate = self.generation_gate.clone();
                self.busy = true;
                self.error = None;
                let title = coincube_ui::locale::render_in(language, || {
                    localized_copy("Choose a signed Bitcoin PSBT").to_owned()
                });
                Task::perform(
                    async move {
                        let Some(handle) = rfd::AsyncFileDialog::new()
                            .set_title(title)
                            .add_filter("Bitcoin PSBT", &["psbt", "txt"])
                            .pick_file()
                            .await
                        else {
                            return Ok(None);
                        };
                        if gate.load(Ordering::SeqCst) != generation {
                            return Err(
                                "This Cube was closed before the signed PSBT was loaded.".into()
                            );
                        }
                        let path = handle.path().to_path_buf();
                        let signed = tokio::task::spawn_blocking(move || psbt_file::load(&path))
                            .await
                            .map_err(|error| error.to_string())?
                            .map_err(|error| format!("Could not read signed PSBT: {error:?}"))?;
                        let (psbt, counts, final_txid) =
                            import_signed_psbt_value(&wallet, &current, &signed)?;
                        if gate.load(Ordering::SeqCst) != generation {
                            return Err(
                                "This Cube was closed before the signatures were saved.".into()
                            );
                        }
                        daemon
                            .update_spend_tx(&psbt)
                            .await
                            .map_err(|error| error.to_string())?;
                        Ok(Some((psbt, counts, final_txid)))
                    },
                    move |result| {
                        Message::ImportedBitcoin(Box::new(ImportEvent::FileLoaded(
                            generation, draft_id, result,
                        )))
                    },
                )
            }
            ImportMessage::Broadcast => {
                if !self.is_ready() {
                    self.error =
                        Some("Refresh the complete wallet history before broadcasting.".into());
                    return Task::none();
                }
                let Some(draft) = &self.draft else {
                    return Task::none();
                };
                if self.spend_drafts.iter().any(|entry| {
                    entry.psbt.unsigned_tx.compute_txid() == draft.psbt.unsigned_tx.compute_txid()
                        && entry.broadcast_txid.is_some()
                }) {
                    self.error = Some("This transaction was already broadcast.".into());
                    return Task::none();
                }
                let Some(final_txid) = draft.final_txid else {
                    self.error = Some("More signer signatures are needed before broadcast.".into());
                    return Task::none();
                };
                let wallet = match self.import_descriptor() {
                    Ok(wallet) => wallet,
                    Err(error) => {
                        self.error = Some(error);
                        return Task::none();
                    }
                };
                let Some(daemon) = self.daemon.clone() else {
                    return Task::none();
                };
                let signed = draft.psbt.clone();
                let draft_id = signed.unsigned_tx.compute_txid();
                self.busy = true;
                let generation = self.generation;
                Task::perform(
                    async move {
                        let original = unsigned_construction(&signed);
                        let secp = secp256k1::Secp256k1::verification_only();
                        let verified = finalize_imported_spend(&wallet, &original, &signed, &secp)
                            .map_err(|error| error.to_string())?;
                        if verified.transaction().compute_txid() != final_txid {
                            return Err(
                                "The finalized transaction changed. Review the spend again.".into(),
                            );
                        }
                        let before = daemon
                            .list_spend_txs()
                            .await
                            .map_err(|error| error.to_string())?
                            .spend_txs
                            .into_iter()
                            .find(|entry| entry.psbt.unsigned_tx.compute_txid() == draft_id)
                            .ok_or("The saved transaction no longer exists.")?;
                        if before.broadcast_txid.is_some() {
                            return Err("This transaction was already broadcast.".into());
                        }
                        if before.psbt != signed {
                            return Err("The saved signatures changed. Reopen and review this PSBT before broadcasting.".into());
                        }
                        daemon
                            .broadcast_spend_tx(&draft_id)
                            .await
                            .map_err(|error| error.to_string())?;
                        let actual = daemon.list_spend_txs().await.map_err(|error| error.to_string())?
                            .spend_txs.into_iter().find(|entry| entry.psbt.unsigned_tx.compute_txid() == draft_id)
                            .and_then(|entry| entry.broadcast_txid)
                            .ok_or("Broadcast was sent, but its final transaction ID could not be confirmed. Refresh before trying again.")?;
                        Ok((actual, actual != final_txid))
                    },
                    move |result| {
                        Message::ImportedBitcoin(Box::new(ImportEvent::Broadcasted(
                            generation, result,
                        )))
                    },
                )
            }
            ImportMessage::Hardware(message) => {
                let Some(listing) = &mut self.hardware_wallets else {
                    return Task::none();
                };
                let task = match listing.update(message) {
                    Ok(task) => task.map(|message| {
                        Message::View(view::Message::ImportedBitcoin(ImportMessage::Hardware(
                            message,
                        )))
                    }),
                    Err(error) => {
                        self.error = Some(format!("Hardware wallet: {error}"));
                        Task::none()
                    }
                };
                // The list may now hold the device a key is waiting for.
                Task::batch([task, self.auto_check_devices()])
            }
            ImportMessage::SignWithDevice(id) => {
                if !self.is_ready() {
                    self.error = Some("Refresh the complete wallet history before signing.".into());
                    return Task::none();
                }
                let Some(draft) = &self.draft else {
                    self.error = Some("Create or select a PSBT before signing.".into());
                    return Task::none();
                };
                let wallet = match self.import_descriptor() {
                    Ok(wallet) => wallet,
                    Err(error) => {
                        self.error = Some(error);
                        return Task::none();
                    }
                };
                let policy = match imported_policy(&wallet) {
                    Ok(policy) => policy,
                    Err(error) => {
                        self.error = Some(error.to_string());
                        return Task::none();
                    }
                };
                let Some(listing) = &self.hardware_wallets else {
                    self.error = Some("Connect a hardware wallet first.".into());
                    return Task::none();
                };
                let Some(device) = listing
                    .list
                    .iter()
                    .find(|device| device.id() == &id)
                    .cloned()
                else {
                    self.error =
                        Some("The selected hardware wallet is no longer connected.".into());
                    return Task::none();
                };
                let binder = ProductionBinder::from_listing(listing);
                let Some(daemon) = self.daemon.clone() else {
                    return Task::none();
                };
                let current = draft.psbt.clone();
                let draft_id = current.unsigned_tx.compute_txid();
                let generation = self.generation;
                let gate = self.generation_gate.clone();
                self.busy = true;
                self.error = None;
                Task::perform(
                    async move {
                        if gate.load(Ordering::SeqCst) != generation {
                            return Err("The Cube was closed before hardware signing began.".into());
                        }
                        let mut session =
                            SigningSession::from_listed(&device, policy, Arc::new(binder))
                                .await
                                .map_err(|error| error.to_string())?;
                        session
                            .register()
                            .await
                            .map_err(|error| error.to_string())?;
                        if gate.load(Ordering::SeqCst) != generation {
                            return Err("The Cube was closed before hardware signing began.".into());
                        }
                        let signed = session
                            .sign(&current)
                            .await
                            .map_err(|error| error.to_string())?
                            .into_psbt();
                        if gate.load(Ordering::SeqCst) != generation {
                            return Err(
                                "The Cube was closed before hardware signing completed.".into()
                            );
                        }
                        let (psbt, counts, final_txid) =
                            import_signed_psbt(&wallet, &current, &signed.to_string())?;
                        if gate.load(Ordering::SeqCst) != generation {
                            return Err(
                                "The Cube was closed before the signatures were saved.".into()
                            );
                        }
                        daemon
                            .update_spend_tx(&psbt)
                            .await
                            .map_err(|error| error.to_string())?;
                        Ok((psbt, counts, final_txid))
                    },
                    move |result| {
                        Message::ImportedBitcoin(Box::new(ImportEvent::HardwareSigned(
                            generation, draft_id, result,
                        )))
                    },
                )
            }
            ImportMessage::ContinueFromScan => {
                if self.import.active_attachment().is_some() {
                    self.scan_continued = true;
                }
                Task::none()
            }
            ImportMessage::LeaveJourney => {
                if !self.writing_setup() && self.keys_stored != KeysStored::Storing {
                    // Closing an import that never installed its wallet
                    // abandons it: the Cube made for it goes too, through
                    // the Launcher's delete confirmation, where the user can
                    // still keep it. A finished import only closes.
                    let abandoned = !self.adding_keys && self.can_delete_cube();
                    self.journey = false;
                    // Typed words do not outlive the screen they were typed on.
                    if let Some(keys) = &mut self.keys {
                        keys.forget_words();
                    }
                    if self.import.active_attachment().is_none() {
                        self.keys_confirmed = false;
                    }
                    if self.adding_keys {
                        self.adding_keys = false;
                        self.keys = None;
                    }
                    if abandoned {
                        return Task::done(Message::View(view::Message::DeleteUnfinishedCube));
                    }
                }
                Task::none()
            }
            ImportMessage::ResumeJourney => {
                if self.import.active_attachment().is_none() {
                    self.journey = true;
                }
                Task::none()
            }
            ImportMessage::Abandon => {
                if self.import.active_attachment().is_some() {
                    self.error =
                        Some("An installed Bitcoin wallet cannot be replaced here.".into());
                    return Task::none();
                }
                self.busy = true;
                let generation = self.generation;
                let directory = self.directory.clone();
                let cube_id = self.cube_id.clone();
                let operation_id = self.import.operation_id;
                Task::perform(
                    async move {
                        bitcoin_import::abandon_pending(&directory, &cube_id, operation_id).await
                    },
                    move |result| {
                        Message::ImportedBitcoin(Box::new(ImportEvent::Abandoned(
                            generation, result,
                        )))
                    },
                )
            }
        }
    }

    pub fn update_event(&mut self, event: ImportEvent) -> Task<Message> {
        match event {
            ImportEvent::Start => match &self.import.stage {
                BitcoinImportStage::PendingSource => Task::none(),
                BitcoinImportStage::Installing { .. } => {
                    let generation = self.generation;
                    let directory = self.directory.clone();
                    let cube_id = self.cube_id.clone();
                    let operation_id = self.import.operation_id;
                    self.busy = true;
                    Task::perform(
                        async move { bitcoin_import::resume(&directory, &cube_id, operation_id).await },
                        move |result| {
                            Message::ImportedBitcoin(Box::new(ImportEvent::Resumed(
                                generation, result,
                            )))
                        },
                    )
                }
                BitcoinImportStage::Syncing { .. } | BitcoinImportStage::Ready { .. } => {
                    Task::batch([self.open_daemon(), self.load_key_inventory()])
                }
            },
            ImportEvent::Installed(generation, result) if generation == self.generation => {
                self.busy = false;
                match result {
                    Ok(()) => {
                        if let Some(wallet) = self.review.clone() {
                            self.import.stage = BitcoinImportStage::Syncing { wallet };
                        }
                        let store = self.store_keys();
                        let inventory = if self.keys_stored == KeysStored::Storing {
                            Task::none()
                        } else {
                            // A device-only import: say what this device holds.
                            self.load_key_inventory()
                        };
                        Task::batch([self.open_daemon(), store, inventory])
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Task::none()
                    }
                }
            }
            ImportEvent::Resumed(generation, result) if generation == self.generation => {
                self.busy = false;
                match result {
                    Ok(Some(_)) => {
                        if let Some(wallet) = self.review.clone() {
                            self.import.stage = BitcoinImportStage::Syncing { wallet };
                        }
                        // The words given before the interruption were not
                        // kept; the inventory says whether any were stored.
                        Task::batch([self.open_daemon(), self.load_key_inventory()])
                    }
                    Ok(None) => {
                        self.error = Some(
                            "Review the Bitcoin server and confirm this interrupted import again."
                                .into(),
                        );
                        Task::none()
                    }
                    Err(error) => {
                        self.error = Some(error);
                        Task::none()
                    }
                }
            }
            ImportEvent::Opened(generation, result) if generation == self.generation => {
                self.busy = false;
                match result {
                    Ok(daemon) => {
                        self.daemon = Some(daemon);
                        self.owns_daemon = true;
                        self.sync_failed = false;
                        self.refresh()
                    }
                    Err(_) => {
                        // Backend errors can contain an entire HTML error page, endpoint,
                        // or authentication details. Keep those out of the wallet screen.
                        log::warn!(
                            "Imported Bitcoin daemon could not connect to its configured server"
                        );
                        self.sync_failed = true;
                        self.error = Some("Could not connect to this wallet's Bitcoin server. Check the server and your connection, then retry synchronization.".into());
                        Task::none()
                    }
                }
            }
            ImportEvent::Refreshed(generation, result) if generation == self.generation => {
                self.busy = false;
                match result {
                    Ok((info, coins, history, spend_drafts, stage)) => {
                        self.snapshot_failed = false;
                        self.info = Some(info);
                        self.coins = Some(coins);
                        self.history = history;
                        self.spend_drafts = spend_drafts;
                        self.import.stage = stage;
                        self.error = None;
                        self.sync_failed = false;
                        if let Some(selected) = self.draft.take() {
                            let id = selected.psbt.unsigned_tx.compute_txid();
                            let current = self
                                .spend_drafts
                                .iter()
                                .find(|entry| entry.psbt.unsigned_tx.compute_txid() == id)
                                .map(|entry| entry.psbt.clone());
                            match current {
                                Some(psbt) => match self.import_descriptor().and_then(|wallet| {
                                    checked_draft(&wallet, psbt, selected.warnings)
                                }) {
                                    Ok(draft) => self.draft = Some(draft),
                                    Err(error) => self.error = Some(error),
                                },
                                None => {
                                    self.error = Some(
                                        "The saved transaction is no longer available.".into(),
                                    );
                                }
                            }
                        }
                    }
                    Err(_) => {
                        log::warn!("Imported Bitcoin synchronization failed");
                        self.snapshot_failed = true;
                        self.info = None;
                        self.coins = None;
                        self.history.clear();
                        self.address = None;
                        self.sync_failed = true;
                        self.error = Some("Could not synchronize this Bitcoin wallet. Check the Bitcoin server and your connection, then retry synchronization.".into());
                    }
                }
                Task::none()
            }
            ImportEvent::SynchronizationRequested(generation, result)
                if generation == self.generation =>
            {
                self.sync_requesting = false;
                match result {
                    Ok(()) => self.poll_progress(),
                    Err(_) => {
                        self.snapshot_failed = true;
                        self.sync_failed = true;
                        self.error = Some("Could not retry this wallet's Bitcoin server. Check your connection and try again.".into());
                        Task::none()
                    }
                }
            }
            ImportEvent::ProgressChecked(generation, result) if generation == self.generation => {
                self.progress_polling = false;
                // A receive reservation deliberately clears the old address
                // snapshot until its new high-water index is read back.
                if self.is_ready() || self.address_reserving {
                    return Task::none();
                }
                // A status-only response cannot reauthorize coins from an
                // earlier scan. The full refresh must pair completed history
                // with a fresh authoritative coin snapshot.
                self.coins = None;
                match result {
                    Ok(info) => {
                        if !self.info_matches_attachment(&info) {
                            self.info = None;
                            self.sync_failed = true;
                            self.error = Some("This Bitcoin server reported a different wallet or chain. Check the wallet setup and retry synchronization.".into());
                            return Task::none();
                        }
                        let history_complete = info
                            .imported_wallet
                            .as_ref()
                            .is_some_and(|status| status.history_complete);
                        self.info = Some(info);
                        if self.snapshot_failed {
                            // Keep the full-snapshot error actionable until a
                            // user retry or the normal 20-second refresh.
                            return Task::none();
                        }
                        self.sync_failed = false;
                        if self.error.as_deref().is_some_and(|error| {
                            error.starts_with("Could not synchronize this Bitcoin wallet")
                                || error
                                    .starts_with("This Bitcoin server reported a different wallet")
                        }) {
                            self.error = None;
                        }
                        // Only the full refresh may mark the Cube ready: it also
                        // obtains the authoritative coin and transaction snapshot.
                        if history_complete && !self.busy && self.verified_address_state().is_some()
                        {
                            return self.refresh();
                        }
                    }
                    Err(_) => {
                        log::warn!("Imported Bitcoin scan status could not be refreshed");
                        self.sync_failed = true;
                        self.error = Some("Could not synchronize this Bitcoin wallet. Check the Bitcoin server and your connection, then retry synchronization.".into());
                    }
                }
                Task::none()
            }
            ImportEvent::Address(generation, result) if generation == self.generation => {
                self.busy = false;
                self.address_reserving = false;
                match result {
                    Ok(address) => {
                        self.address = Some(address);
                        return self.refresh();
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            ImportEvent::Abandoned(generation, result) if generation == self.generation => {
                self.busy = false;
                match result {
                    Ok(()) => {
                        self.abandoned = true;
                        self.keys = None;
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            ImportEvent::DraftCreated(generation, result) if generation == self.generation => {
                self.busy = false;
                match result {
                    Ok((psbt, warnings)) => match self
                        .import_descriptor()
                        .and_then(|wallet| checked_draft(&wallet, psbt, warnings))
                    {
                        Ok(draft) => {
                            self.draft = Some(draft);
                            self.signed_psbt_text.clear();
                            self.error = None;
                            return self.refresh();
                        }
                        Err(error) => self.error = Some(error),
                    },
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            ImportEvent::SignedSaved(generation, draft_id, result)
            | ImportEvent::HardwareSigned(generation, draft_id, result)
                if generation == self.generation =>
            {
                self.busy = false;
                if self
                    .draft
                    .as_ref()
                    .is_none_or(|draft| draft.psbt.unsigned_tx.compute_txid() != draft_id)
                {
                    return Task::none();
                }
                match result {
                    Ok((psbt, counts, final_txid)) => {
                        let warnings = self
                            .draft
                            .as_ref()
                            .map_or_else(Vec::new, |draft| draft.warnings.clone());
                        self.draft = Some(ImportedDraft {
                            psbt,
                            signatures_per_input: counts,
                            final_txid,
                            warnings,
                        });
                        self.signed_psbt_text.clear();
                        self.error = None;
                        return self.refresh();
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            ImportEvent::Broadcasted(generation, result) if generation == self.generation => {
                self.busy = false;
                match result {
                    Ok((txid, changed)) => {
                        self.last_broadcast_txid = Some(txid);
                        self.broadcast_notice = changed.then(|| "Another valid signer signature changed the final transaction ID. The ID shown here comes from the broadcast wallet record.".into());
                        self.error = None;
                        return self.refresh();
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            ImportEvent::Expanded(generation, result) if generation == self.generation => {
                self.busy = false;
                match result {
                    Ok(_) => self.open_daemon(),
                    Err(error) => {
                        self.error = Some(format!(
                            "Could not expand the wallet history search: {error}"
                        ));
                        Task::none()
                    }
                }
            }
            ImportEvent::FileSaved(generation, id, result) if generation == self.generation => {
                self.busy = false;
                match result {
                    Ok(true)
                        if self
                            .draft
                            .as_ref()
                            .is_some_and(|draft| draft.psbt.unsigned_tx.compute_txid() == id) =>
                    {
                        self.file_notice = Some("PSBT saved. Open it in the original signing wallet, then load the signed file here.".into());
                    }
                    Ok(_) => {}
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            ImportEvent::FileLoaded(generation, draft_id, result)
                if generation == self.generation =>
            {
                self.busy = false;
                if self
                    .draft
                    .as_ref()
                    .is_none_or(|draft| draft.psbt.unsigned_tx.compute_txid() != draft_id)
                {
                    return Task::none();
                }
                match result {
                    Ok(Some((psbt, counts, final_txid))) => {
                        let warnings = self
                            .draft
                            .as_ref()
                            .map_or_else(Vec::new, |draft| draft.warnings.clone());
                        self.draft = Some(ImportedDraft {
                            psbt,
                            signatures_per_input: counts,
                            final_txid,
                            warnings,
                        });
                        self.file_notice = Some("Signed PSBT loaded and verified.".into());
                        self.error = None;
                        return self.refresh();
                    }
                    Ok(None) => {}
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            ImportEvent::WordsVerified(generation, index, check, result)
            | ImportEvent::DeviceVerified(generation, index, check, result)
                if generation == self.generation =>
            {
                if let Some(slot) = self.key_slot(index) {
                    // Only the check this slot is waiting for may land: an
                    // edit, a new source or a newer check makes it stale.
                    if slot.status == KeyStatus::Checking && slot.check == check && check != 0 {
                        slot.check = 0;
                        slot.status = match result {
                            Ok(()) => KeyStatus::Verified,
                            Err(error) => KeyStatus::Failed(error),
                        };
                    }
                }
                // Another key's device may be waiting its turn.
                self.auto_check_devices()
            }
            ImportEvent::KeysInventory(epoch, result)
                if epoch == self.keys_gate.load(Ordering::SeqCst) =>
            {
                self.stored_keys = result.ok();
                Task::none()
            }
            ImportEvent::KeysStored(epoch, result)
                if epoch == self.keys_gate.load(Ordering::SeqCst) =>
            {
                match result {
                    Ok(count) => {
                        self.keys_stored = KeysStored::Stored(count);
                        self.stored_keys = Some(count);
                        // The words now live only in the Cube's encrypted
                        // key records.
                        if let Some(keys) = &mut self.keys {
                            for slot in &mut keys.slots {
                                slot.words = zeroize::Zeroizing::new(String::new());
                                slot.passphrase = zeroize::Zeroizing::new(String::new());
                            }
                        }
                    }
                    Err(error) => {
                        self.keys_stored = KeysStored::Failed(error);
                        // Once the words are gone, Add recovery words is the
                        // way back; it needs the current count.
                        return self.load_key_inventory();
                    }
                }
                Task::none()
            }
            ImportEvent::Opened(_, Ok(daemon)) => Task::perform(
                async move {
                    if let Err(error) = daemon.stop().await {
                        log::error!("Could not stop stale imported Bitcoin daemon: {error}");
                    }
                },
                |_| Message::Tick,
            ),
            _ => Task::none(),
        }
    }

    fn open_daemon(&mut self) -> Task<Message> {
        if self.daemon.is_some() {
            return self.refresh();
        }
        if self.chain().is_blake2b() {
            self.sync_failed = true;
            self.error = Some(
                "Reopen this paired Cube to restore its authenticated Bitcoin Blake2b session."
                    .into(),
            );
            return Task::none();
        }
        let Some(wallet) = self.import.active_attachment() else {
            return Task::none();
        };
        let path = self
            .directory
            .coincubed_data_directory(&wallet.wallet_id)
            .path()
            .join("daemon.toml");
        let mut config = match Config::from_file(Some(path)) {
            Ok(config) => config,
            Err(_) => {
                log::warn!("Imported Bitcoin daemon settings could not be loaded");
                self.sync_failed = true;
                self.error = Some("Could not read this wallet's Bitcoin server settings. Retry synchronization or check the wallet setup.".into());
                return Task::none();
            }
        };
        if let Err(error) = crate::loader::authenticate_imported_connect_provider(
            &mut config,
            self.chain(),
            self.connect_token.as_deref(),
        ) {
            self.sync_failed = true;
            self.error = Some(error.to_string());
            return Task::none();
        }
        if let Err(error) = self.hydrate_provider_from_config(&config) {
            self.sync_failed = true;
            self.error = Some(error.into());
            return Task::none();
        }
        let generation = self.generation;
        let gate = self.generation_gate.clone();
        self.busy = true;
        self.error = None;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let daemon = EmbeddedDaemon::start(config)
                        .map(|d| Arc::new(d) as Arc<dyn Daemon + Send + Sync>)
                        .map_err(|e| e.to_string())?;
                    if gate.load(Ordering::SeqCst) != generation {
                        tokio::runtime::Handle::current()
                            .block_on(async { daemon.stop().await })
                            .map_err(|e| e.to_string())?;
                        return Err("The Cube was closed before the wallet opened.".into());
                    }
                    Ok(daemon)
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |result| {
                Message::ImportedBitcoin(Box::new(ImportEvent::Opened(generation, result)))
            },
        )
    }

    fn hydrate_provider_from_config(&mut self, config: &Config) -> Result<(), &'static str> {
        match config.bitcoin_backend.as_ref() {
            Some(BitcoinBackend::Esplora(esplora))
                if esplora.addr == crate::installer::connect_url(self.chain()) =>
            {
                self.provider = Provider::Connect;
                self.endpoint = esplora.addr.clone();
            }
            Some(BitcoinBackend::Esplora(esplora))
                if esplora.addr == crate::installer::public_esplora_url(Network::Bitcoin) =>
            {
                self.provider = Provider::Public;
                self.endpoint = esplora.addr.clone();
            }
            Some(BitcoinBackend::Esplora(esplora)) => {
                self.provider = Provider::Esplora;
                self.endpoint = esplora.addr.clone();
            }
            Some(BitcoinBackend::Electrum(electrum)) => {
                self.provider = Provider::Electrum;
                self.endpoint = electrum.addr.clone();
            }
            _ => return Err("This imported wallet's Bitcoin server is unsupported."),
        }
        self.provider_known = true;
        Ok(())
    }

    fn refresh(&mut self) -> Task<Message> {
        if self.busy {
            return Task::none();
        }
        let Some(daemon) = self.daemon.clone() else {
            return Task::none();
        };
        let generation = self.generation;
        let directory = self.directory.clone();
        let cube_id = self.cube_id.clone();
        let operation_id = self.import.operation_id;
        self.busy = true;
        Task::perform(
            async move {
                let info = daemon.get_info().await.map_err(|e| e.to_string())?;
                bitcoin_import::reconcile_sync(&directory, &cube_id, operation_id, &info).await?;
                let stage = settings::Settings::from_file(&directory)
                    .map_err(|e| e.to_string())?
                    .cubes
                    .into_iter()
                    .find(|cube| cube.id == cube_id)
                    .and_then(|cube| cube.bitcoin_import)
                    .filter(|import| import.operation_id == operation_id)
                    .ok_or("The imported wallet is no longer attached to this Cube.")?
                    .stage;
                let coins = daemon
                    .list_coins(&[CoinStatus::Confirmed, CoinStatus::Unconfirmed], &[])
                    .await
                    .map_err(|e| e.to_string())?;
                let history = daemon
                    .list_history_txs(0, u32::MAX, 20)
                    .await
                    .map_err(|e| e.to_string())?;
                let spend_drafts = daemon
                    .list_spend_txs()
                    .await
                    .map_err(|e| e.to_string())?
                    .spend_txs;
                Ok((info, coins, history, spend_drafts, stage))
            },
            move |result| {
                Message::ImportedBitcoin(Box::new(ImportEvent::Refreshed(generation, result)))
            },
        )
    }

    fn poll_progress(&mut self) -> Task<Message> {
        if self.progress_polling || self.is_ready() {
            return Task::none();
        }
        let Some(daemon) = self.daemon.clone() else {
            return Task::none();
        };
        self.progress_polling = true;
        let generation = self.generation;
        Task::perform(
            async move { daemon.get_info().await.map_err(|error| error.to_string()) },
            move |result| {
                Message::ImportedBitcoin(Box::new(ImportEvent::ProgressChecked(generation, result)))
            },
        )
    }

    fn retry_synchronization(&mut self) -> Task<Message> {
        if self.sync_requesting || self.import.active_attachment().is_none() {
            return Task::none();
        }
        self.snapshot_failed = false;
        let Some(daemon) = self.daemon.clone() else {
            return self.open_daemon();
        };
        self.sync_requesting = true;
        let generation = self.generation;
        Task::perform(
            async move {
                daemon
                    .request_sync()
                    .await
                    .map_err(|error| error.to_string())
            },
            move |result| {
                Message::ImportedBitcoin(Box::new(ImportEvent::SynchronizationRequested(
                    generation, result,
                )))
            },
        )
    }

    /// Like [`Self::stop`], but the owned daemon stops on its own thread so
    /// locking the Cube does not wait for its in-flight network request.
    pub fn stop_in_background(&mut self) {
        if let Some(daemon) = self.release_daemon() {
            crate::daemon::stopping::stop_in_background(daemon, || {});
        }
    }

    fn release_daemon(&mut self) -> Option<Arc<dyn Daemon + Send + Sync>> {
        self.keys_gate.fetch_add(1, Ordering::SeqCst);
        if let Some(keys) = &mut self.keys {
            keys.forget_words();
        }
        self.birthday_calendar_open = false;
        self.generation = fresh_generation();
        self.generation_gate
            .store(self.generation, Ordering::SeqCst);
        self.busy = false;
        self.address_reserving = false;
        self.sync_requesting = false;
        self.progress_polling = false;
        self.snapshot_failed = false;
        self.source.clear_transient();
        let owns_daemon = self.owns_daemon;
        self.owns_daemon = false;
        self.daemon.take().filter(|_| owns_daemon)
    }

    pub fn stop(&mut self) {
        if let Some(daemon) = self.release_daemon() {
            let runtime = tokio::runtime::Handle::try_current().ok();
            let result = std::thread::spawn(move || {
                if let Some(handle) = runtime {
                    handle
                        .block_on(async { daemon.stop().await })
                        .map_err(|error| error.to_string())
                } else {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|error| error.to_string())?
                        .block_on(async { daemon.stop().await })
                        .map_err(|error| error.to_string())
                }
            })
            .join();
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log::error!("Could not stop imported Bitcoin daemon: {error}"),
                Err(_) => log::error!("Imported Bitcoin daemon shutdown panicked"),
            }
        }
    }

    /// The setup step as one column: its form over its action row.
    #[cfg(test)]
    fn setup_view(&self) -> Element<'_, view::Message> {
        let (form, actions) = self.setup_step();
        Column::new()
            .spacing(16)
            .max_width(800)
            .push(form)
            .push(actions)
            .into()
    }

    /// Done's account of where the wallet's keys are.
    fn keys_line(&self) -> Option<Element<'_, view::Message>> {
        if self.keys_stored == KeysStored::None && !self.adding_keys {
            // A resumed install or a device-only import: say what this
            // device holds and offer to add words.
            if let Some(view) = self.stored_keys_view() {
                return Some(view);
            }
        }
        let keys = self.keys.as_ref().filter(|keys| !keys.slots.is_empty())?;
        let devices = keys
            .slots
            .iter()
            .filter(|slot| slot.source == KeySource::Hardware && slot.provided())
            .count();
        let elsewhere = keys.signs_elsewhere();
        let mut parts = Vec::new();
        match &self.keys_stored {
            KeysStored::Stored(count) => parts.push(coincube_ui::localized_format!(
                "{count} from recovery words, stored encrypted on this device for this Cube. \
                 They are not in the Recovery Kit yet, so keep your original backup",
                "{count} con palabras de recuperación, guardadas cifradas en este dispositivo \
                 para este Cube. Todavía no están en el Kit de recuperación, así que conserva \
                 tu copia original"
            )),
            KeysStored::Storing => {
                parts.push(localized_copy("storing the recovery words…").to_string())
            }
            KeysStored::Failed(error) => {
                return Some(
                    p1_regular(coincube_ui::localized_format!(
                        "Keys: the recovery words were not stored. {}",
                        "Claves: no se guardaron las palabras de recuperación. {}",
                        shown_notice(error)
                    ))
                    .style(theme::text::error)
                    .into(),
                )
            }
            KeysStored::None => (),
        }
        if devices > 0 {
            parts.push(coincube_ui::localized_format!(
                "{devices} on a hardware wallet",
                "{devices} en una cartera de hardware"
            ));
        }
        if elsewhere > 0 {
            parts.push(coincube_ui::localized_format!(
                "{elsewhere} signs elsewhere",
                "{elsewhere} con firma externa"
            ));
        }
        (!parts.is_empty()).then(|| {
            p1_regular(coincube_ui::localized_format!(
                "Keys: {}",
                "Claves: {}",
                parts.join(" · ")
            ))
            .into()
        })
    }

    fn key_slot(&mut self, index: usize) -> Option<&mut keys::KeySlot> {
        self.keys
            .as_mut()
            .and_then(|keys| keys.slots.get_mut(index))
    }

    /// A connected hardware wallet with this signer's fingerprint. A paired
    /// phone joins the same list but is not a hardware wallet.
    fn connected_device(
        &self,
        account: &coincube_core::imported_keys::ImportedAccount,
    ) -> Option<Arc<dyn async_hwi::HWI + Send + Sync>> {
        self.hardware_wallets
            .as_ref()?
            .list
            .iter()
            .filter(|listed| !listed.is_lan_phone())
            .find_map(|listed| match listed {
                HardwareWallet::Supported {
                    device,
                    fingerprint,
                    ..
                } if *fingerprint == account.fingerprint => Some(device.clone()),
                _ => None,
            })
    }

    /// The Keys step's list of connected hardware wallets. A paired phone is
    /// not one.
    fn key_devices(&self) -> Vec<keys::ListedDevice> {
        use crate::hw::UnsupportedReason;
        let Some(listing) = &self.hardware_wallets else {
            return Vec::new();
        };
        listing
            .list
            .iter()
            .filter(|listed| !listed.is_lan_phone())
            .map(|listed| {
                let kind = keys::device_name(listed.kind());
                match listed {
                    HardwareWallet::Supported {
                        fingerprint, alias, ..
                    } => keys::ListedDevice {
                        name: alias
                            .as_ref()
                            .map_or_else(|| kind.to_owned(), |alias| format!("{alias} ({kind})")),
                        fingerprint: Some(*fingerprint),
                        problem: None,
                        pairing_code: None,
                    },
                    // A BitBox02 waiting to be paired shows a code to confirm.
                    HardwareWallet::Locked { pairing_code, .. } => keys::ListedDevice {
                        name: kind.to_owned(),
                        fingerprint: None,
                        problem: Some(
                            localized_copy(if pairing_code.is_some() {
                                "Confirm this pairing code on the device:"
                            } else {
                                "Locked. Unlock it to see which key it holds."
                            })
                            .to_owned(),
                        ),
                        pairing_code: pairing_code.clone(),
                    },
                    HardwareWallet::Unsupported { reason, .. } => {
                        let fixed = |problem: &str| Some(localized_copy(problem).to_owned());
                        let (fingerprint, problem) = match reason {
                            UnsupportedReason::NotPartOfWallet(fingerprint) => {
                                (Some(*fingerprint), None)
                            }
                            UnsupportedReason::AppIsNotOpen => {
                                (None, fixed("Open the Bitcoin app on it."))
                            }
                            UnsupportedReason::WrongNetwork => {
                                (None, fixed("Set to a different network than this wallet."))
                            }
                            UnsupportedReason::Version {
                                minimal_supported_version,
                                ..
                            } => (
                                None,
                                Some(coincube_ui::localized_format!(
                                    "Firmware too old to read its key. Update it to {minimal_supported_version} or later.",
                                    "Firmware demasiado antiguo para leer su clave. Actualízalo a {minimal_supported_version} o posterior."
                                )),
                            ),
                            UnsupportedReason::Method(_) => {
                                (None, fixed("Can't read its key in COINCUBE Desktop."))
                            }
                        };
                        keys::ListedDevice {
                            name: kind.to_owned(),
                            fingerprint,
                            problem,
                            pairing_code: None,
                        }
                    }
                }
            })
            .collect()
    }

    /// Check each hardware key whose device has just appeared, without a
    /// click: reading the key is read-only, and the row tells the user to
    /// approve on the device if it asks (Robert, 2026-11-16). Each key is
    /// checked once per connection of its device; a failed check waits for
    /// Check again or for the device to be reconnected.
    fn auto_check_devices(&mut self) -> Task<Message> {
        if self.adding_keys || self.keys_confirmed || self.review.is_none() {
            return Task::none();
        }
        let connected: Vec<(
            String,
            coincube_core::miniscript::bitcoin::bip32::Fingerprint,
        )> = self
            .hardware_wallets
            .as_ref()
            .map(|listing| {
                listing
                    .list
                    .iter()
                    .filter(|listed| !listed.is_lan_phone())
                    .filter_map(|listed| match listed {
                        HardwareWallet::Supported {
                            id, fingerprint, ..
                        } => Some((id.clone(), *fingerprint)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        // One check at a time, so a device never has two requests waiting
        // on it; the next starts when this one lands.
        let busy = self.keys.as_ref().is_some_and(KeysState::checking);
        let mut due = None;
        if let Some(keys) = &mut self.keys {
            for (index, slot) in keys.slots.iter_mut().enumerate() {
                let device = connected
                    .iter()
                    .find(|(_, fingerprint)| *fingerprint == slot.account.fingerprint)
                    .map(|(id, _)| id);
                // Unplugged: the next connection is checked afresh.
                if device.is_none() {
                    slot.auto_checked = None;
                }
                // A failed check is tried again only on a fresh connection.
                let waiting = slot.status == KeyStatus::Missing
                    || (matches!(slot.status, KeyStatus::Failed(_)) && slot.auto_checked.is_none());
                if slot.source != KeySource::Hardware || !waiting {
                    continue;
                }
                if let Some(id) = device {
                    if !busy && due.is_none() && slot.auto_checked.as_ref() != Some(id) {
                        slot.auto_checked = Some(id.clone());
                        due = Some(index);
                    }
                }
            }
        }
        due.map_or_else(Task::none, |index| self.check_device(index))
    }

    /// The device list for the Keys step, before the wallet is installed.
    fn ensure_review_hardware_wallets(&mut self) -> Result<(), String> {
        if self.hardware_wallets.is_some() {
            return Ok(());
        }
        let root = self
            .hardware_root
            .clone()
            .ok_or("Hardware wallet access is unavailable.")?;
        let wallet = self
            .review
            .as_ref()
            .ok_or("Review the wallet before checking a device.")?;
        let policy = imported_policy(&wallet.descriptor).map_err(|error| error.to_string())?;
        self.hardware_wallets = Some(
            HardwareWallets::new(root, self.chain().bitcoin_network())
                .with_public_policy(policy.name().to_owned(), policy.descriptor().to_owned()),
        );
        Ok(())
    }

    /// Check typed recovery words against signer `index`: the derived account
    /// must be that signer's fingerprint, origin and xpub in both branches.
    fn verify_words(&mut self, index: usize) -> Task<Message> {
        let Some(wallet) = self.review.clone() else {
            return Task::none();
        };
        let generation = self.generation;
        let gate = self.generation_gate.clone();
        let Some(slot) = self.key_slot(index) else {
            return Task::none();
        };
        if slot.source != KeySource::Words || slot.status == KeyStatus::Checking {
            return Task::none();
        }
        // BIP39 words are lowercase and single-spaced. Built in place so no
        // unwiped copy of the phrase is left behind.
        let mut words = zeroize::Zeroizing::new(String::with_capacity(slot.words.len()));
        for word in slot.words.split_whitespace() {
            if !words.is_empty() {
                words.push(' ');
            }
            for c in word.chars() {
                for lower in c.to_lowercase() {
                    words.push(lower);
                }
            }
        }
        slot.words = words.clone();
        let passphrase = slot.passphrase.clone();
        let expected = slot.account.clone();
        let check = keys::next_check();
        slot.check = check;
        slot.status = KeyStatus::Checking;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    if gate.load(Ordering::SeqCst) != generation {
                        return Err("The key check was cancelled.".to_string());
                    }
                    check_words(&wallet.descriptor, &expected, words, passphrase)
                })
                .await
                .map_err(|_| "The key check stopped unexpectedly.".to_string())?
            },
            move |result| {
                Message::ImportedBitcoin(Box::new(ImportEvent::WordsVerified(
                    generation, index, check, result,
                )))
            },
        )
    }

    /// Check the connected device for signer `index`. It keeps its seed.
    fn check_device(&mut self, index: usize) -> Task<Message> {
        let Some(wallet) = self.review.clone() else {
            return Task::none();
        };
        let Some(account) = self
            .keys
            .as_ref()
            .and_then(|keys| keys.slots.get(index))
            .filter(|slot| slot.source == KeySource::Hardware && slot.status != KeyStatus::Checking)
            .map(|slot| slot.account.clone())
        else {
            return Task::none();
        };
        let Some(device) = self.connected_device(&account) else {
            self.error = Some("Connect and unlock the device with this fingerprint.".into());
            return Task::none();
        };
        let generation = self.generation;
        let gate = self.generation_gate.clone();
        let check = keys::next_check();
        if let Some(slot) = self.key_slot(index) {
            slot.check = check;
            slot.status = KeyStatus::Checking;
        }
        Task::perform(
            async move {
                crate::services::imported_device_identity::verify_device_account(
                    device,
                    &wallet.descriptor,
                    &account,
                    || gate.load(Ordering::SeqCst) == generation,
                )
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
            },
            move |result| {
                Message::ImportedBitcoin(Box::new(ImportEvent::DeviceVerified(
                    generation, index, check, result,
                )))
            },
        )
    }

    /// Store the verified recovery words in this Cube's encrypted key
    /// records, once the wallet they belong to is attached.
    fn store_keys(&mut self) -> Task<Message> {
        let entries = match &self.keys {
            None => return Task::none(),
            Some(keys) => keys
                .slots
                .iter()
                .filter(|slot| slot.source == KeySource::Words && slot.provided())
                .map(|slot| {
                    coincube_core::imported_key_crypt::ImportedKeySecret::new(
                        slot.words.clone(),
                        slot.passphrase.clone(),
                    )
                    .map(|secret| (slot.account.clone(), secret))
                    .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, String>>(),
        };
        let entries = match entries {
            Ok(entries) if entries.is_empty() => {
                self.keys_stored = KeysStored::None;
                return Task::none();
            }
            Ok(entries) => entries,
            Err(error) => {
                self.keys_stored = KeysStored::Failed(error);
                return Task::none();
            }
        };
        let Some(attachment) = self.import.active_attachment().cloned() else {
            return Task::none();
        };
        let directory = self.directory.clone();
        let cube_id = self.cube_id.clone();
        let gate = self.keys_gate.clone();
        let epoch = gate.load(Ordering::SeqCst);
        self.keys_stored = KeysStored::Storing;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let settings = settings::Settings::from_file(&directory)
                        .map_err(|_| "Cannot read this Cube's settings.".to_string())?;
                    let cube = settings
                        .cubes
                        .iter()
                        .find(|cube| cube.id == cube_id)
                        .ok_or("This Cube no longer exists.")?;
                    let repository = crate::services::imported_keys::LocalKeyRepository::new(
                        &directory,
                        cube,
                        &attachment,
                    )?;
                    repository
                        .admit_new_batch(entries, || gate.load(Ordering::SeqCst) == epoch)
                        .map(|records| records.len())
                })
                .await
                .map_err(|_| "Storing the keys stopped unexpectedly.".to_string())?
            },
            move |result| {
                Message::ImportedBitcoin(Box::new(ImportEvent::KeysStored(epoch, result)))
            },
        )
    }

    /// Words can be stored only for the unlocked Cube and never while Duress
    /// Mode is set up. Checked before installing, which cannot be undone.
    fn storage_preflight(&self) -> Result<(), String> {
        let words = self.keys.as_ref().is_some_and(|keys| {
            keys.slots
                .iter()
                .any(|slot| slot.source == KeySource::Words && slot.provided())
        });
        if !words {
            return Ok(());
        }
        if crate::app::session::current_cube_id().as_deref() != Some(self.cube_id.as_str()) {
            return Err("Unlock this Cube again before storing recovery words.".into());
        }
        let root = self
            .directory
            .path()
            .parent()
            .ok_or("Missing Cube data root.")?;
        let duress = crate::services::duress::DuressLocalState::load(root)
            .map_err(|_| "Cannot check Duress Mode.".to_string())?;
        if duress.enrolled || duress.arming || duress.active {
            return Err(
                "Recovery words cannot be stored while Duress Mode is set up. Use a \
                        hardware wallet for this key, or turn off Duress Mode first."
                    .into(),
            );
        }
        Ok(())
    }

    /// Read how many local key records the attached wallet has.
    pub fn load_key_inventory(&self) -> Task<Message> {
        let Some(attachment) = self.import.active_attachment().cloned() else {
            return Task::none();
        };
        let directory = self.directory.clone();
        let cube_id = self.cube_id.clone();
        let epoch = self.keys_gate.load(Ordering::SeqCst);
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let settings = settings::Settings::from_file(&directory)
                        .map_err(|_| "Cannot read this Cube's settings.".to_string())?;
                    let cube = settings
                        .cubes
                        .iter()
                        .find(|cube| cube.id == cube_id)
                        .ok_or("This Cube no longer exists.")?;
                    crate::services::imported_keys::LocalKeyRepository::new(
                        &directory,
                        cube,
                        &attachment,
                    )?
                    .records()
                    .map(|records| records.len())
                })
                .await
                .map_err(|_| "Reading the stored keys stopped unexpectedly.".to_string())?
            },
            move |result| {
                Message::ImportedBitcoin(Box::new(ImportEvent::KeysInventory(epoch, result)))
            },
        )
    }

    /// Where this wallet's recovery words stand on this device, with Add
    /// recovery words when none are stored.
    fn stored_keys_view(&self) -> Option<Element<'_, view::Message>> {
        let count = self.stored_keys?;
        let mut content = Column::new()
            .spacing(8)
            .push(p1_bold(localized_copy("Keys on this device")));
        if count == 0 {
            content = content
                .push(
                    p1_regular(localized_copy("No recovery words for this wallet are stored on this device. If you hold any of its keys as recovery words, add them."))
                    .style(theme::text::secondary),
                )
                .push(
                    button::secondary(None, localized_copy("Add recovery words"))
                        .width(iced::Length::Shrink)
                        .on_press_maybe(
                            (!self.adding_keys)
                                .then_some(view::Message::ImportedBitcoin(ImportMessage::AddKeys)),
                        ),
                );
        } else {
            content = content.push(
                p1_regular(coincube_ui::localized_format!(
                    "{count} from recovery words, stored encrypted on this device for this \
                     Cube. They are not in the Recovery Kit yet, so keep your original backup.",
                    "{count} con palabras de recuperación, guardadas cifradas en este \
                     dispositivo para este Cube. Todavía no están en el Kit de recuperación, \
                     así que conserva tu copia original."
                ))
                .style(theme::text::secondary),
            );
        }
        Some(content.into())
    }

    /// The Vault page while the import is unfinished and Import or restore
    /// is closed.
    fn resume_view(&self) -> Element<'_, view::Message> {
        Column::new()
            .spacing(16)
            .max_width(800)
            .push(h3(localized_copy("Finish importing your Bitcoin wallet")))
            .push(
                p1_regular(localized_copy("This Cube is waiting for your existing Bitcoin wallet. Continue in Import or restore, or use a new Vault instead."))
                    .style(theme::text::secondary),
            )
            .push(
                Row::new()
                    .spacing(10)
                    .push(button::primary(None, localized_copy("Continue import")).on_press(
                        view::Message::ImportedBitcoin(ImportMessage::ResumeJourney),
                    ))
                    .push(
                        button::secondary(None, localized_copy("Use a new Vault instead")).on_press_maybe(
                            (!self.busy).then_some(view::Message::ImportedBitcoin(
                                ImportMessage::Abandon,
                            )),
                        ),
                    )
                    .push(self.delete_cube_button()),
            )
            .push_maybe(
                self.error
                    .as_ref()
                    .map(|error| p1_regular(shown_notice(error)).style(theme::text::error)),
            )
            .into()
    }

    /// True while Import or restore holds the window instead of the Cube.
    /// An install or abandon is being written. Refreshes during the scan
    /// also set `busy`, but leaving then interrupts nothing.
    fn writing_setup(&self) -> bool {
        self.busy && self.import.active_attachment().is_none()
    }

    #[cfg(test)]
    pub(crate) fn open_journey_for_tests(&mut self) {
        self.journey = true;
    }

    pub fn journey_open(&self) -> bool {
        self.journey && !self.abandoned
    }

    /// Where the import stands on Import or restore's stepper.
    fn journey_step(&self) -> JourneyStep {
        if self.adding_keys {
            return if self.keys_confirmed {
                JourneyStep::Done
            } else {
                JourneyStep::Keys
            };
        }
        if self.import.active_attachment().is_none() {
            if self.review.is_none() {
                JourneyStep::Source
            } else if self.keys.is_some() && !self.keys_confirmed {
                JourneyStep::Keys
            } else {
                JourneyStep::Review
            }
        } else if self.is_ready() || self.scan_continued {
            JourneyStep::Done
        } else {
            JourneyStep::Scan
        }
    }

    /// Import or restore around the import's steps after the Cube exists:
    /// the Launcher's header, path list and stepper, with no Cube menu.
    pub fn journey_view<'a>(
        &'a self,
        cache: &'a Cache,
        passkey: bool,
    ) -> Element<'a, view::Message> {
        use crate::home::restore_frame;
        use iced::{widget::Space, Alignment, Length};

        let light = matches!(
            cache.theme_mode,
            coincube_ui::theme::palette::ThemeMode::Light
        );
        let step = self.journey_step();
        let _ = passkey;
        let steps = restore_frame::import_steps();
        let active = steps
            .iter()
            .position(|label| *label == step.label())
            .unwrap_or(steps.len() - 1);
        // The stepper's labels are compared in English above and shown here.
        let shown_steps: Vec<&'static str> =
            steps.iter().map(|label| localized_copy(label)).collect();
        let (form, actions): (Element<'a, view::Message>, Element<'a, view::Message>) = match step {
            JourneyStep::Source | JourneyStep::Review => self.setup_step(),
            JourneyStep::Keys => match &self.keys {
                Some(keys) => keys::step(
                    keys,
                    self.adding_keys,
                    &self.key_devices(),
                    self.error.as_ref(),
                ),
                None => self.setup_step(),
            },
            JourneyStep::Scan => (
                self.scan_content(Column::new().spacing(16))
                    .push(
                        Row::new()
                            .spacing(10)
                            .align_y(Alignment::Center)
                            .push(
                                button::secondary(None, localized_copy("Receive"))
                                    .width(Length::Shrink),
                            )
                            .push(
                                caption(localized_copy(
                                    "Wait for the complete Bitcoin history scan before receiving.",
                                ))
                                .style(theme::text::secondary),
                            ),
                    )
                    .push_maybe(
                        self.error
                            .as_ref()
                            .map(|error| p1_regular(shown_notice(error)).style(theme::text::error)),
                    )
                    .into(),
                Row::new()
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::primary(None, localized_copy("Continue"))
                            .width(Length::Shrink)
                            .on_press(view::Message::ImportedBitcoin(
                                ImportMessage::ContinueFromScan,
                            )),
                    )
                    .into(),
            ),
            JourneyStep::Done => (
                self.done_content(cache),
                Row::new()
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::primary(None, localized_copy("Open Cube"))
                            .width(Length::Shrink)
                            .on_press_maybe((self.keys_stored != KeysStored::Storing).then_some(
                                view::Message::ImportedBitcoin(ImportMessage::LeaveJourney),
                            )),
                    )
                    .into(),
            ),
        };
        // The Cube now exists, so the import is the only path left open.
        let nav = restore_frame::nav()
            .push(restore_frame::nav_label(localized_copy("Overview"), false))
            .push(restore_frame::nav_label(
                localized_copy("Import Bitcoin wallet"),
                true,
            ));
        let body = iced::widget::scrollable(
            Column::new()
                .spacing(20)
                .padding([24, 28])
                .push_maybe(
                    (step != JourneyStep::Done)
                        .then(|| h2(localized_copy("Import Bitcoin wallet"))),
                )
                .push(form),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
        let region = restore_frame::region(
            Some(restore_frame::step_rail(&shown_steps, active)),
            body,
            Some(actions),
        );
        // Close before the wallet is installed asks the Launcher to delete
        // the Cube made for the import; a kept Cube's Vault page reopens it.
        // Done has its own Open Cube.
        let close: Element<'a, view::Message> = if step != JourneyStep::Done {
            restore_frame::close_plain(
                (!self.writing_setup() && self.keys_stored != KeysStored::Storing)
                    .then_some(view::Message::ImportedBitcoin(ImportMessage::LeaveJourney)),
            )
        } else {
            Space::new().into()
        };
        let header = restore_frame::sheet_header(
            localized_copy("Import or restore"),
            restore_frame::network_chip(self.chain(), light),
            close,
        );
        restore_frame::standalone(restore_frame::sheet(
            header,
            restore_frame::with_nav(nav.into(), region),
            None,
        ))
    }

    /// Done: the Sumi-e art, "[Cube] is ready" and one short line. Only a
    /// stalled scan or a failed key store adds its own retry.
    fn done_content<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let light = matches!(
            cache.theme_mode,
            coincube_ui::theme::palette::ThemeMode::Light
        );
        let ready = self.is_ready();
        let stalled = matches!(
            ScanDisplay::from_info(self.info.as_ref(), self.sync_failed),
            ScanDisplay::Retrying | ScanDisplay::NeedsAttention
        );
        let line = if ready {
            localized_copy(
                "Your wallet is now this Cube's Vault. Open it to receive or review its history.",
            )
        } else if stalled {
            localized_copy(
                "The history scan stopped before it finished. Retry it here or from the Vault page.",
            )
        } else {
            localized_copy(
                "Your wallet is now this Cube's Vault. Its history appears when the scan finishes.",
            )
        };
        Column::new()
            .spacing(16)
            .width(iced::Length::Fill)
            .align_x(iced::Alignment::Center)
            .push(crate::home::restore_frame::done_art(light))
            .push(h3(coincube_ui::localized_format!(
                "{} is ready",
                "{} está listo",
                cache.cube_name
            )))
            .push(p1_regular(line).style(theme::text::secondary))
            .push_maybe((!ready && stalled).then(|| {
                button::secondary(
                    None,
                    if self.sync_requesting {
                        localized_copy("Retrying…")
                    } else {
                        localized_copy(self.synchronization_action_label())
                    },
                )
                .width(iced::Length::Shrink)
                .on_press_maybe((!self.busy && !self.sync_requesting).then_some(
                    view::Message::ImportedBitcoin(ImportMessage::RetrySynchronization),
                ))
            }))
            .push_maybe(matches!(self.keys_stored, KeysStored::Failed(_)).then(|| {
                Column::new()
                    .spacing(8)
                    .align_x(iced::Alignment::Center)
                    .push_maybe(self.keys_line())
                    .push(
                        button::secondary(None, localized_copy("Store keys again"))
                            .width(iced::Length::Shrink)
                            .on_press(view::Message::ImportedBitcoin(
                                ImportMessage::RetryStoreKeys,
                            )),
                    )
            }))
            .into()
    }

    /// This Cube was created for the import. Until a wallet is installed
    /// in it, the user can delete it instead of keeping an empty Cube; the
    /// Launcher's delete confirmation does the deleting.
    fn delete_cube_button(&self) -> Element<'_, view::Message> {
        button::secondary(None, localized_copy("Delete this Cube"))
            .width(iced::Length::Shrink)
            .on_press_maybe(
                self.can_delete_cube()
                    .then_some(view::Message::DeleteUnfinishedCube),
            )
            .into()
    }

    fn can_delete_cube(&self) -> bool {
        !self.busy
            && self.import.active_attachment().is_none()
            && matches!(self.keys_stored, KeysStored::None | KeysStored::Failed(_))
    }

    /// Source, or Review once the form parses: the step's form and its
    /// action row.
    fn setup_step(&self) -> (Element<'_, view::Message>, Element<'_, view::Message>) {
        let mut content = Column::new().spacing(16);
        let mut footer = Row::new()
            .spacing(10)
            .align_y(iced::Alignment::Center)
            .push(
                button::secondary(None, localized_copy("Use a new Vault instead"))
                    .width(iced::Length::Shrink)
                    .on_press_maybe(
                        (!self.busy)
                            .then_some(view::Message::ImportedBitcoin(ImportMessage::Abandon)),
                    ),
            )
            .push(self.delete_cube_button())
            .push(iced::widget::Space::new().width(iced::Length::Fill));
        if self.review.is_none() {
            content = content.push(p1_regular(localized_copy("Use your existing Bitcoin wallet in this Cube. The descriptor describes the wallet; you add its keys on the next step.")).style(theme::text::secondary));
            let beginning = if self.scan_from_beginning {
                button::primary(None, localized_copy("From the beginning"))
            } else {
                button::secondary(None, localized_copy("From the beginning"))
            };
            let dated = if self.scan_from_beginning {
                button::secondary(None, localized_copy("From a creation date"))
            } else {
                button::primary(None, localized_copy("From a creation date"))
            };
            content = content
                .push(wallet_source_form::view(&self.source, &SourceFormCopy::import()).map(|m| {
                    view::Message::ImportedBitcoin(ImportMessage::Source(m))
                }))
                .push(iced::widget::Space::new().height(8))
                .push(p1_bold(localized_copy("Where should the history scan start?")))
                .push(Row::new().spacing(10)
                    .push(beginning
                        .width(iced::Length::Fill)
                        .on_press(view::Message::ImportedBitcoin(ImportMessage::ScanFromBeginning(true))))
                    .push(dated
                        .width(iced::Length::Fill)
                        .on_press(view::Message::ImportedBitcoin(ImportMessage::ScanFromBeginning(false)))))
                .push_maybe((!self.scan_from_beginning).then(|| {
                    Row::new()
                        .spacing(10)
                        .align_y(iced::Alignment::Center)
                        .push(text_input(localized_copy("YYYY-MM-DD"), &self.birthday)
                            .on_input(|s| view::Message::ImportedBitcoin(ImportMessage::BirthdayEdited(s)))
                            .padding(10))
                        .push(date_picker::calendar_button(
                            self.birthday_calendar_open,
                            chrono::NaiveDate::parse_from_str(self.birthday.trim(), "%Y-%m-%d")
                                .unwrap_or_else(|_| chrono::Utc::now().date_naive()),
                            ImportMessage::BirthdayCalendarOpen,
                            ImportMessage::BirthdayCalendarCancel,
                            ImportMessage::BirthdaySelected,
                        ).map(view::Message::ImportedBitcoin))
                }))
                .push(caption(if self.scan_from_beginning {
                    localized_copy("Starts at Bitcoin's first block. Choose this if you do not know when the wallet was created; it may take longer.")
                } else {
                    localized_copy("Choose the original wallet's creation date or an earlier date so older transactions are included.")
                }).style(theme::text::secondary))
                .push(iced::widget::Space::new().height(8))
                .push(p1_bold(localized_copy("Addresses to search per branch")))
                .push(text_input("200", &self.lookahead)
                    .on_input(|s| view::Message::ImportedBitcoin(ImportMessage::LookaheadEdited(s)))
                    .padding(10))
                .push(caption(localized_copy("Start with 200 receive and 200 change addresses. You can search further after the scan finishes.")).style(theme::text::secondary))
                .push(iced::widget::Space::new().height(8))
                .push(p1_bold(localized_copy("Bitcoin server")));
            let mut providers = Column::new().spacing(8);
            let mut provider_row = Row::new().spacing(8);
            for provider in [
                Provider::Connect,
                Provider::Public,
                Provider::Esplora,
                Provider::Electrum,
            ] {
                let button = if self.provider == provider {
                    button::primary(None, localized_copy(provider.label()))
                } else {
                    button::secondary(None, localized_copy(provider.label()))
                };
                provider_row = provider_row.push(button.width(iced::Length::Fill).on_press(
                    view::Message::ImportedBitcoin(ImportMessage::ProviderSelected(provider)),
                ));
                if matches!(provider, Provider::Public | Provider::Electrum) {
                    providers = providers.push(provider_row);
                    provider_row = Row::new().spacing(8);
                }
            }
            content = content.push(providers);
            if matches!(self.provider, Provider::Esplora | Provider::Electrum) {
                content = content.push(
                    text_input(localized_copy("Server URL"), &self.endpoint)
                        .on_input(|s| {
                            view::Message::ImportedBitcoin(ImportMessage::EndpointEdited(s))
                        })
                        .padding(10),
                );
            }
            footer = footer.push(
                button::primary(None, localized_copy("Review wallet"))
                    .width(iced::Length::Shrink)
                    .on_press(view::Message::ImportedBitcoin(ImportMessage::Review)),
            );
        } else if let Some(wallet) = &self.review {
            let retry = matches!(&self.import.stage, BitcoinImportStage::Installing { .. });
            let (script, threshold, fingerprints) = wallet_summary(&wallet.descriptor);
            let sample_address = sample_receive_address(&wallet.descriptor)
                .unwrap_or_else(|_| localized_copy("Unable to derive a sample address").into());
            content = content
                .push(h3(localized_copy("Review Bitcoin wallet")))
                .push(p1_bold(localized_copy("Wallet details")))
                .push(p1_regular(format!("{} · {script} · {threshold}", crate::chain::canonical_network_name(wallet.descriptor.chain()))))
                .push(p1_regular(coincube_ui::localized_format!("Signer fingerprints: {}", "Huellas de los firmantes: {}", if fingerprints.is_empty() { localized_copy("Not provided") } else { &fingerprints })))
                .push_maybe(self.keys.as_ref().filter(|keys| !keys.slots.is_empty()).map(|keys| p1_regular(keys.summary())))
                .push(p1_bold(localized_copy("First receive address")))
                .push(p1_regular(sample_address.clone()).wrapping(iced::widget::text::Wrapping::WordOrGlyph))
                .push(button::secondary_compact(None, localized_copy("Copy address"))
                    .on_press(view::Message::CopyWithToast(sample_address, "Address copied".into())))
                .push(caption(localized_copy("Compare this address with your original wallet before importing.")).style(theme::text::secondary))
                .push(p1_regular(coincube_ui::localized_format!("History starts: {}", "El historial empieza: {}",
                    if wallet.birthday == 0 { localized_copy("Beginning").to_string() } else {
                        chrono::DateTime::from_timestamp(i64::from(wallet.birthday), 0)
                            .map(|date| date.format("%Y-%m-%d").to_string())
                            .unwrap_or_else(|| localized_copy("Invalid date").into())
                    })))
                .push(p1_regular(coincube_ui::localized_format!("Address search: {} receive and {} change addresses", "Búsqueda de direcciones: {} de recepción y {} de cambio", wallet.initial_lookahead, wallet.initial_lookahead)))
                .push(caption(localized_copy("Your balance and transactions will appear after the scan finishes. Then update the Cube Recovery Kit to include this wallet.")).style(theme::text::secondary))
                .push(button::secondary_compact(None, if self.show_details { localized_copy("Hide descriptor details") } else { localized_copy("Show descriptor details") })
                    .on_press(view::Message::ImportedBitcoin(ImportMessage::ToggleDetails)));
            if self.show_details {
                content = content
                    .push(caption(coincube_ui::localized_format!(
                        "Wallet identity: {}",
                        "Identidad de la cartera: {}",
                        wallet.wallet_id.descriptor_checksum
                    )))
                    .push(
                        caption(coincube_ui::localized_format!(
                            "Receive: {}",
                            "Recepción: {}",
                            wallet.descriptor.receive_descriptor()
                        ))
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                        .style(theme::text::secondary),
                    )
                    .push(
                        caption(coincube_ui::localized_format!(
                            "Change: {}",
                            "Cambio: {}",
                            wallet.descriptor.change_descriptor()
                        ))
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                        .style(theme::text::secondary),
                    );
            }
            content = content
                .push(p1_bold(localized_copy("Bitcoin server")))
                .push(p1_regular(localized_copy(self.provider.label())));
            if retry {
                content = content.push(caption(localized_copy("The wallet identity is locked to this Cube. Choose the Bitcoin server to finish an interrupted import.")));
                let mut providers = Column::new().spacing(8);
                let mut provider_row = Row::new().spacing(8);
                for provider in [
                    Provider::Connect,
                    Provider::Public,
                    Provider::Esplora,
                    Provider::Electrum,
                ] {
                    let option = if self.provider == provider {
                        button::primary(None, localized_copy(provider.label()))
                    } else {
                        button::secondary(None, localized_copy(provider.label()))
                    };
                    provider_row =
                        provider_row.push(option.width(iced::Length::Fill).on_press_maybe(
                            (!self.busy).then_some(view::Message::ImportedBitcoin(
                                ImportMessage::ProviderSelected(provider),
                            )),
                        ));
                    if matches!(provider, Provider::Public | Provider::Electrum) {
                        providers = providers.push(provider_row);
                        provider_row = Row::new().spacing(8);
                    }
                }
                content = content.push(providers);
                if matches!(self.provider, Provider::Esplora | Provider::Electrum) {
                    content = content.push(
                        text_input(localized_copy("Server URL"), &self.endpoint)
                            .on_input(|value| {
                                view::Message::ImportedBitcoin(ImportMessage::EndpointEdited(value))
                            })
                            .padding(10),
                    );
                }
            }
            if matches!(self.provider, Provider::Esplora | Provider::Electrum) {
                content = content.push(caption(public_server_label(&self.endpoint)));
            }
            if self.provider == Provider::Public {
                content = content.push(caption(localized_copy("Public server: mempool.space, with blockstream.info fallback. This reveals wallet addresses to those providers.")));
            }
            if self.provider == Provider::Connect {
                content = content.push(caption(localized_copy("Connect server: COINCUBE's authenticated Bitcoin proxy, with public fallback servers.")));
            }
            footer = footer.push(
                Row::new()
                    .spacing(10)
                    .push(
                        button::secondary(
                            None,
                            if self.keys.is_some() {
                                localized_copy("Back to keys")
                            } else {
                                localized_copy("Edit wallet")
                            },
                        )
                        .width(iced::Length::Shrink)
                        .on_press_maybe(if self.keys.is_some() {
                            (!self.busy).then_some(view::Message::ImportedBitcoin(
                                ImportMessage::BackToKeys,
                            ))
                        } else {
                            (!retry).then_some(view::Message::ImportedBitcoin(ImportMessage::Edit))
                        }),
                    )
                    .push(
                        button::primary(
                            None,
                            if self.busy {
                                localized_copy("Importing…")
                            } else if retry {
                                localized_copy("Retry import")
                            } else {
                                localized_copy("Import and scan wallet")
                            },
                        )
                        .width(iced::Length::Shrink)
                        .on_press_maybe(
                            (!self.busy)
                                .then_some(view::Message::ImportedBitcoin(ImportMessage::Confirm)),
                        ),
                    ),
            );
        }
        if let Some(error) = &self.error {
            content = content.push(p1_regular(shown_notice(error)).style(theme::text::error));
        }
        (content.into(), footer.into())
    }

    fn wallet_view(&self, mask_amounts: bool) -> Element<'_, view::Message> {
        let ready = self.is_ready();
        let mut content = Column::new()
            .spacing(16)
            .max_width(if ready { 1500 } else { 800 })
            .push(h2("Vault"));
        if ready {
            if let Some(balance) = self.authoritative_balance() {
                content = content.push(h3(coincube_ui::localized_format!(
                    "{} available",
                    "{} disponible",
                    passive_wallet_amount(balance.to_sat(), mask_amounts)
                )));
            }
            content = content
                .push(p1_regular(localized_copy("The signing keys remain in your original wallet or hardware devices. Your Cube recovery seed cannot sign for this Bitcoin wallet.")))
                .push(p1_regular(localized_copy("Update the Cube Recovery Kit now that this wallet is attached.")))
                .push(Row::new().spacing(10)
                    .push_maybe((!self.chain().is_blake2b()).then(||button::primary(None, localized_copy("Send")).on_press(view::Message::ImportedBitcoin(ImportMessage::ShowSend))))
                    .push(button::primary(None, localized_copy("Receive")).on_press(view::Message::ImportedBitcoin(ImportMessage::Receive)))
                    .push(button::secondary(None, localized_copy("Refresh")).on_press(view::Message::ImportedBitcoin(ImportMessage::Refresh))));
            if let Some(address) = &self.address {
                content = content.push(p1_regular(address.address.to_string())).push(
                    button::secondary(None, localized_copy("Copy address"))
                        .on_press(view::Message::ImportedBitcoin(ImportMessage::CopyAddress)),
                );
            }
            content = content.push(p1_bold(localized_copy("Recent transactions")));
            if self.history.is_empty() {
                content = content.push(caption(localized_copy(
                    "No transactions found in the completed scan.",
                )));
            } else {
                for tx in self.history.iter().take(20) {
                    let (direction, amount) = match tx.kind {
                        model::TransactionKind::IncomingSinglePayment(_)
                        | model::TransactionKind::IncomingPaymentBatch(_) => {
                            (localized_copy("Received"), tx.incoming_amount.to_sat())
                        }
                        model::TransactionKind::SendToSelf => (
                            localized_copy("Moved within wallet"),
                            tx.incoming_amount.to_sat(),
                        ),
                        model::TransactionKind::OutgoingSinglePayment(_)
                        | model::TransactionKind::OutgoingPaymentBatch(_) => {
                            (localized_copy("Sent"), tx.outgoing_amount.to_sat())
                        }
                    };
                    let date = tx
                        .time
                        .and_then(|time| {
                            chrono::DateTime::from_timestamp(i64::from(time), 0)
                                .map(|date| date.format("%Y-%m-%d").to_string())
                        })
                        .unwrap_or_else(|| localized_copy("Date unknown").into());
                    let confirmation = match (tx.height, self.info.as_ref()) {
                        (Some(height), Some(info)) if height > 0 && info.block_height >= height => {
                            coincube_ui::localized_format!(
                                "{} confirmations",
                                "{} confirmaciones",
                                info.block_height - height + 1
                            )
                        }
                        _ => localized_copy("Unconfirmed").into(),
                    };
                    content = content
                        .push(p1_regular(format!(
                            "{direction} · {} · {date} · {confirmation}",
                            passive_wallet_amount(amount, mask_amounts)
                        )))
                        .push(caption(tx.txid.to_string()));
                }
            }
            if let Some(status) = self
                .info
                .as_ref()
                .and_then(|info| info.imported_wallet.as_ref())
            {
                if !self.chain().is_blake2b() && status.discovery_target < 10_000 {
                    content = content
                        .push(iced::widget::Space::new().height(8))
                        .push(p1_bold(localized_copy("Missing an older transaction?")))
                        .push(caption(localized_copy("Search 200 more receive and change addresses. This starts another history scan.")).style(theme::text::secondary))
                        .push(button::secondary_compact(None, localized_copy("Search more addresses"))
                            .on_press_maybe((!self.busy).then_some(
                                view::Message::ImportedBitcoin(ImportMessage::ExpandDiscovery)
                            )));
                }
            }
        } else {
            content = self.scan_content(content);
        }
        if let Some(error) = &self.error {
            content = content.push(p1_regular(shown_notice(error)).style(theme::text::error));
        }
        content.into()
    }

    /// The history scan's heading, progress and retry, under `content`.
    fn scan_content<'a>(
        &'a self,
        mut content: Column<'a, view::Message>,
    ) -> Column<'a, view::Message> {
        let scan = ScanDisplay::from_info(self.info.as_ref(), self.sync_failed);
        let label = match scan {
            ScanDisplay::Starting => localized_copy("Finding your wallet's Bitcoin history"),
            ScanDisplay::Scanning { .. } => localized_copy("Scanning your Bitcoin wallet history"),
            ScanDisplay::Saving => localized_copy("Finishing your Bitcoin history scan"),
            ScanDisplay::Retrying => localized_copy("Bitcoin history scan was interrupted"),
            ScanDisplay::NeedsAttention => localized_copy("Bitcoin history needs attention"),
        };
        content = content.push(h3(label));
        if matches!(
            scan,
            ScanDisplay::Starting | ScanDisplay::Scanning { .. } | ScanDisplay::Saving
        ) {
            content = content.push(
                p1_regular(localized_copy(
                    "Your balance and transactions will appear when the scan finishes.",
                ))
                .style(theme::text::secondary),
            );
        }
        match scan {
            ScanDisplay::Scanning {
                checked,
                expected,
                progress,
            } => {
                content = content
                    .push(
                        progress_bar(0.0..=1.0, progress as f32)
                            .style(theme::progress_bar::primary),
                    )
                    .push(
                        caption(coincube_ui::localized_format!(
                            "{:.0}% · {checked} addresses checked (about {expected} expected)",
                            "{:.0}% · {checked} direcciones revisadas (se esperan unas {expected})",
                            progress * 100.0
                        ))
                        .style(theme::text::secondary),
                    );
            }
            ScanDisplay::Starting | ScanDisplay::Saving => {
                content = content.push(spinner::typing_text_carousel(
                    "...",
                    true,
                    Duration::from_millis(2000),
                    |value| caption(value).style(theme::text::secondary),
                ));
            }
            ScanDisplay::Retrying | ScanDisplay::NeedsAttention => {
                if self.error.is_none() {
                    content = content.push(
                        p1_regular(localized_copy(
                            "Check your connection and Bitcoin server, then retry the scan.",
                        ))
                        .style(theme::text::secondary),
                    );
                }
                content = content.push(
                    button::secondary_compact(
                        None,
                        if self.sync_requesting {
                            localized_copy("Retrying…")
                        } else {
                            localized_copy(self.synchronization_action_label())
                        },
                    )
                    .on_press_maybe(
                        (!self.busy && !self.sync_requesting).then_some(
                            view::Message::ImportedBitcoin(ImportMessage::RetrySynchronization),
                        ),
                    ),
                );
            }
        }
        if matches!(
            scan,
            ScanDisplay::Starting | ScanDisplay::Scanning { .. } | ScanDisplay::Saving
        ) {
            content = content.push(
                caption(localized_copy(
                    "Progress updates automatically. Keep COINCUBE Desktop open while it scans.",
                ))
                .style(theme::text::secondary),
            );
        }
        content
    }

    fn send_view(&self, psbts_only: bool) -> Element<'_, view::Message> {
        if self.chain().is_blake2b() {
            return Column::new()
                .spacing(16)
                .push(h2(localized_copy("Send")))
                .push(p1_regular(localized_copy(
                    "Sending from paired imported Bitcoin Blake2b wallets is not available.",
                )))
                .into();
        }
        let mut content = Column::new()
            .spacing(14)
            .push(h2(if psbts_only { "PSBTs" } else { localized_copy("Send Bitcoin") }))
            .push(p1_regular(localized_copy("Spend from the imported wallet using its original signer or hardware device. Your Cube seed cannot sign this transaction.")))
            .push(button::secondary(None, localized_copy("Back to overview"))
                .on_press(view::Message::Menu(Menu::Vault(VaultSubMenu::Overview))));
        if let Some(txid) = self.last_broadcast_txid {
            content = content.push(p1_bold(coincube_ui::localized_format!(
                "Broadcast transaction: {txid}",
                "Transacción publicada: {txid}"
            )));
        }
        if let Some(notice) = &self.broadcast_notice {
            content = content.push(caption(shown_notice(notice)));
        }
        if let Some(notice) = &self.file_notice {
            content = content.push(caption(shown_notice(notice)));
        }
        if let Some(draft) = &self.draft {
            content = content
                .push(p1_bold(localized_copy("Review transaction")))
                .push(p1_regular(crate::chain::canonical_network_name(
                    self.chain(),
                )))
                .push(caption(coincube_ui::localized_format!(
                    "Draft ID: {}",
                    "ID del borrador: {}",
                    draft.psbt.unsigned_tx.compute_txid()
                )));
            for output in &draft.psbt.unsigned_tx.output {
                let address = Address::from_script(&output.script_pubkey, Network::Bitcoin)
                    .map(|address| address.to_string())
                    .unwrap_or_else(|_| localized_copy("Unsupported output script").into());
                content = content.push(p1_regular(format!(
                    "{} sats → {address}",
                    output.value.to_sat()
                )));
            }
            if let Ok(fee) = draft.psbt.fee() {
                content = content.push(p1_regular(coincube_ui::localized_format!(
                    "Network fee: {} sats",
                    "Comisión de red: {} sats",
                    fee.to_sat()
                )));
            }
            for warning in &draft.warnings {
                content = content.push(caption(shown_notice(warning)));
            }
            content = content
                .push(caption(coincube_ui::localized_format!("Valid signer signatures per input: {}", "Firmas válidas por entrada: {}",
                    draft.signatures_per_input.iter().map(|count| count.to_string()).collect::<Vec<_>>().join(", "))))
                .push(p1_regular(localized_copy("Save or copy this PSBT for the original signing wallet. Load the signed file or paste its base64 below. For multisig, repeat with each signer.")))
                .push(Row::new().spacing(10)
                    .push(button::secondary(None, localized_copy("Save PSBT file"))
                        .on_press_maybe((!self.busy).then_some(view::Message::ImportedBitcoin(ImportMessage::SavePsbt(coincube_ui::locale::language())))))
                    .push(button::secondary(None, localized_copy("Load signed PSBT"))
                        .on_press_maybe((!self.busy).then_some(view::Message::ImportedBitcoin(ImportMessage::LoadSignedPsbt(coincube_ui::locale::language())))))
                    .push(button::secondary(None, localized_copy("Copy PSBT"))
                        .on_press(view::Message::ImportedBitcoin(ImportMessage::CopyPsbt))))
                .push(p1_bold(localized_copy("Sign with a connected device")));
            if let Some(listing) = &self.hardware_wallets {
                if listing.list.is_empty() {
                    content = content.push(caption(localized_copy(
                        "Connect and unlock a hardware wallet to sign here.",
                    )));
                }
                for device in &listing.list {
                    let label = match device {
                        HardwareWallet::Supported {
                            kind, fingerprint, ..
                        } => {
                            format!("{kind} · {fingerprint}")
                        }
                        HardwareWallet::Locked { kind, .. } => coincube_ui::localized_format!(
                            "{kind} · unlock device",
                            "{kind} · desbloquea el dispositivo"
                        ),
                        HardwareWallet::Unsupported { kind, .. } => {
                            coincube_ui::localized_format!(
                                "{kind} · unsupported",
                                "{kind} · no compatible"
                            )
                        }
                    };
                    content = content.push(caption(label));
                    if device.is_supported() {
                        content = content.push(
                            button::secondary(None, localized_copy("Sign PSBT")).on_press_maybe(
                                (!self.busy).then_some(view::Message::ImportedBitcoin(
                                    ImportMessage::SignWithDevice(device.id().clone()),
                                )),
                            ),
                        );
                    }
                }
            }
            content = content
                .push(
                    text_input(
                        localized_copy("Paste signed PSBT (base64)"),
                        &self.signed_psbt_text,
                    )
                    .on_input(|value| {
                        view::Message::ImportedBitcoin(ImportMessage::SignedPsbtEdited(value))
                    })
                    .padding(10),
                )
                .push(
                    button::secondary(None, localized_copy("Import signatures")).on_press_maybe(
                        (!self.busy && !self.signed_psbt_text.trim().is_empty()).then_some(
                            view::Message::ImportedBitcoin(ImportMessage::ImportSignedPsbt),
                        ),
                    ),
                );
            let draft_id = draft.psbt.unsigned_tx.compute_txid();
            let already_broadcast = self.spend_drafts.iter().any(|entry| {
                entry.psbt.unsigned_tx.compute_txid() == draft_id && entry.broadcast_txid.is_some()
            });
            if let Some(final_txid) = draft.final_txid {
                content = content
                    .push(p1_bold(coincube_ui::localized_format!(
                        "Ready to broadcast · Final transaction ID: {final_txid}",
                        "Lista para publicar · ID de la transacción final: {final_txid}"
                    )))
                    .push(
                        button::primary(
                            None,
                            if self.busy {
                                localized_copy("Broadcasting…")
                            } else {
                                localized_copy("Broadcast transaction")
                            },
                        )
                        .on_press_maybe(
                            (!self.busy && self.is_ready() && !already_broadcast).then_some(
                                view::Message::ImportedBitcoin(ImportMessage::Broadcast),
                            ),
                        ),
                    );
                if already_broadcast {
                    content = content.push(caption(localized_copy(
                        "This saved transaction was already broadcast.",
                    )));
                }
            } else {
                content = content.push(caption(localized_copy(
                    "Waiting for enough valid signatures on every input.",
                )));
            }
        } else if !psbts_only {
            content = content
                .push(p1_bold(localized_copy("New transaction")))
                .push(
                    text_input(
                        localized_copy("Bitcoin recipient address"),
                        &self.send_recipient,
                    )
                    .on_input(|value| {
                        view::Message::ImportedBitcoin(ImportMessage::RecipientEdited(value))
                    })
                    .padding(10),
                )
                .push(
                    text_input(localized_copy("Amount in satoshis"), &self.send_amount)
                        .on_input(|value| {
                            view::Message::ImportedBitcoin(ImportMessage::AmountEdited(value))
                        })
                        .padding(10),
                )
                .push(
                    text_input(localized_copy("Fee rate in sat/vB"), &self.send_feerate)
                        .on_input(|value| {
                            view::Message::ImportedBitcoin(ImportMessage::FeerateEdited(value))
                        })
                        .padding(10),
                )
                .push(
                    button::primary(
                        None,
                        if self.busy {
                            localized_copy("Preparing…")
                        } else {
                            localized_copy("Review PSBT")
                        },
                    )
                    .on_press_maybe(
                        (!self.busy && self.is_ready())
                            .then_some(view::Message::ImportedBitcoin(ImportMessage::CreateDraft)),
                    ),
                );
        }
        if psbts_only && self.draft.is_none() {
            content = content.push(
                button::primary(None, localized_copy("New transaction"))
                    .on_press(view::Message::Menu(Menu::Vault(VaultSubMenu::Send))),
            );
        }
        if !self.spend_drafts.is_empty() {
            content = content.push(p1_bold(localized_copy("Saved transactions")));
            for entry in self.spend_drafts.iter().take(10) {
                let id = entry.psbt.unsigned_tx.compute_txid();
                content = content
                    .push(caption(coincube_ui::localized_format!(
                        "Draft {id}",
                        "Borrador {id}"
                    )))
                    .push(
                        button::secondary(None, localized_copy("Open saved PSBT")).on_press_maybe(
                            (!self.busy).then_some(view::Message::ImportedBitcoin(
                                ImportMessage::SelectDraft(id),
                            )),
                        ),
                    );
            }
        }
        if let Some(error) = &self.error {
            content = content.push(p1_regular(shown_notice(error)).style(theme::text::error));
        }
        content.into()
    }

    fn settings_view(&self, option: Option<SettingsOption>) -> Element<'_, view::Message> {
        let mut content = Column::new()
            .spacing(16)
            .push(h2(localized_copy("Vault settings")));
        match option.unwrap_or(SettingsOption::Node) {
            SettingsOption::Node | SettingsOption::Connections => {
                content =
                    content
                        .push(p1_bold(localized_copy("Bitcoin server")))
                        .push(p1_regular(if self.provider_known {
                            localized_copy(self.provider.label())
                        } else {
                            localized_copy("Server settings unavailable")
                        }))
                        .push(caption(public_server_label(&self.endpoint)))
                        .push(
                            button::secondary(None, localized_copy("Retry synchronization"))
                                .on_press_maybe((!self.busy).then_some(
                                    view::Message::ImportedBitcoin(ImportMessage::Refresh),
                                )),
                        );
                if let Some(status) = self
                    .info
                    .as_ref()
                    .and_then(|info| info.imported_wallet.as_ref())
                {
                    content = content.push(caption(coincube_ui::localized_format!(
                        "History checked: receive {} · change {} · target {} per branch",
                        "Historial revisado: recepción {} · cambio {} · objetivo {} por rama",
                        status.receive_scanned,
                        status.change_scanned,
                        status.discovery_target
                    )));
                    if !self.chain().is_blake2b() && status.discovery_target < 10_000 {
                        content = content.push(
                            button::secondary(None, localized_copy("Search 200 more addresses"))
                                .on_press_maybe((self.is_ready() && !self.busy).then_some(
                                    view::Message::ImportedBitcoin(ImportMessage::ExpandDiscovery),
                                )),
                        );
                    }
                }
            }
            SettingsOption::Wallet | SettingsOption::ImportExport => {
                content = content
                    .push(p1_regular(localized_copy("This Vault uses the imported wallet's original signer keys. The Cube seed unlocks the Cube but cannot sign its Bitcoin transactions.")))
                    .push(p1_regular(localized_copy("Keep the Cube Recovery Kit current after importing this wallet or reserving a new receive address.")))
                    .push_maybe(self.stored_keys_view());
                if let Some(wallet) = self.import.active_attachment() {
                    content = content.push(caption(coincube_ui::localized_format!(
                        "Wallet ID: {}",
                        "ID de la cartera: {}",
                        wallet.wallet_id.descriptor_checksum
                    )));
                    if self.show_details {
                        content = content
                            .push(caption(coincube_ui::localized_format!(
                                "Receive: {}",
                                "Recepción: {}",
                                wallet.descriptor.receive_descriptor()
                            )))
                            .push(caption(coincube_ui::localized_format!(
                                "Change: {}",
                                "Cambio: {}",
                                wallet.descriptor.change_descriptor()
                            )));
                    }
                    content = content.push(
                        button::secondary(
                            None,
                            if self.show_details {
                                localized_copy("Hide public descriptors")
                            } else {
                                localized_copy("Show public descriptors")
                            },
                        )
                        .on_press(view::Message::ImportedBitcoin(ImportMessage::ToggleDetails)),
                    );
                }
            }
            SettingsOption::LocalSigning => {}
        }
        if let Some(error) = &self.error {
            content = content.push(p1_regular(shown_notice(error)).style(theme::text::error));
        }
        content.into()
    }
}

fn passive_wallet_amount(sats: u64, mask_amounts: bool) -> String {
    if mask_amounts {
        "••••".to_owned()
    } else {
        format!("{sats} sats")
    }
}

impl State for ImportedBitcoinPanel {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let body = if self.import.active_attachment().is_none() {
            self.resume_view()
        } else if !self.is_ready() {
            self.wallet_view(cache.balance_amounts_masked())
        } else {
            match menu {
                Menu::Vault(VaultSubMenu::Send) => self.send_view(false),
                Menu::Vault(VaultSubMenu::PSBTs(_)) => self.send_view(true),
                Menu::Vault(VaultSubMenu::Settings(option)) => self.settings_view(*option),
                // Unreachable once ready: `App` routes only Send, PSBTs and
                // Settings here, and the Vault page (with its Tools links)
                // serves the rest.
                _ => self.wallet_view(cache.balance_amounts_masked()),
            }
        };
        view::dashboard(menu, cache, body)
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = Vec::new();
        if self.review.is_none() && !self.busy && self.import.active_attachment().is_none() {
            subscriptions.push(
                self.source.subscription().map(|m| {
                    Message::View(view::Message::ImportedBitcoin(ImportMessage::Source(m)))
                }),
            );
        }
        if self.import.active_attachment().is_some() {
            let interval = if self.is_ready() { 120 } else { 20 };
            subscriptions
                .push(iced::time::every(Duration::from_secs(interval)).map(|_| {
                    Message::View(view::Message::ImportedBitcoin(ImportMessage::Refresh))
                }));
            if !self.is_ready() && self.daemon.is_some() {
                subscriptions.push(iced::time::every(Duration::from_secs(2)).map(|_| {
                    Message::View(view::Message::ImportedBitcoin(ImportMessage::PollProgress))
                }));
            }
        }
        if let Some(hardware_wallets) = &self.hardware_wallets {
            subscriptions.push(hardware_wallets.refresh().map(|m| {
                Message::View(view::Message::ImportedBitcoin(ImportMessage::Hardware(m)))
            }));
        }
        Subscription::batch(subscriptions)
    }

    fn interrupt(&mut self) {
        // Navigation does not cancel a durable install or daemon. Source edits
        // do revoke review evidence through `invalidate()` above.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_source_keyboard_actions_keep_import_setup_operable() {
        use crate::app::view::shared::landing::capture;
        use iced::{advanced::widget::Id, keyboard::key::Named};

        let directory = NetworkDirectory::new(
            std::env::temp_dir().join(format!("import-source-keyboard-{}", uuid::Uuid::new_v4())),
        );
        let mut panel =
            ImportedBitcoinPanel::new(directory, "keyboard".into(), BitcoinImport::pending());
        let size = (1024, 768);
        let mounted = capture::inspect_full_window(
            "import-source-keyboard",
            panel.setup_view(),
            size,
            "light",
            theme::Theme::light(),
        );
        for id in [
            "tenshu.source.descriptor",
            "tenshu.source.hardware",
            "tenshu.source.receive",
            "tenshu.source.change",
        ] {
            assert!(
                mounted.has_focus(Id::new(id)),
                "import source focus: {}",
                id
            );
        }
        for key in [Named::Enter, Named::Space] {
            assert!(matches!(
                capture::activate_with_key(
                    panel.setup_view(),
                    Id::new("tenshu.source.hardware"),
                    size,
                    false,
                    key
                )
                .as_slice(),
                [view::Message::ImportedBitcoin(ImportMessage::Source(
                    wallet_source_form::Message::SourceSelected(Source::Hardware)
                ))]
            ));
        }
        let focus = panel.update_view(
            ImportMessage::Source(wallet_source_form::Message::Focus(Id::new(
                "tenshu.source.hardware",
            ))),
            None,
            0,
        );
        assert!(focus.units() > 0);
        assert_eq!(
            panel.source.source(),
            Source::Descriptor,
            "focus never changes import source"
        );
        let _ = panel.update_view(
            ImportMessage::Source(wallet_source_form::Message::SourceSelected(
                Source::Hardware,
            )),
            None,
            0,
        );
        let hardware = capture::inspect_full_window(
            "import-source-keyboard-hardware",
            panel.setup_view(),
            size,
            "dark",
            theme::Theme::dark(),
        );
        assert!(hardware.has_focus(
            panel
                .source
                .hardware_action_id("tenshu.hardware.purpose.bip84")
        ));
        assert!(hardware.has_focus(panel.source.hardware_action_id("tenshu.hardware.account")));
    }

    #[test]
    fn imported_setup_hardware_read_restores_focus_after_loaded_result() {
        use crate::services::public_hardware_source::tests::{
            add_fake_device, assert_focus_task, loaded_for_test, unique_temp_root,
        };
        use crate::split_hardware::HardwareMessage;

        let root = unique_temp_root("import-keyboard-read-return");
        for succeeds in [true, false] {
            let directory = NetworkDirectory::new(root.join(format!("import-{succeeds}")));
            let mut panel =
                ImportedBitcoinPanel::new(directory, "keyboard".into(), BitcoinImport::pending());
            panel.source.select_source(Source::Hardware);
            add_fake_device(panel.source.hardware_mut(), &root);
            let reading = panel.update_view(
                ImportMessage::Source(wallet_source_form::Message::Hardware(
                    HardwareMessage::Read("dev-1".into()),
                )),
                None,
                0,
            );
            assert_focus_task(
                reading,
                panel.setup_view().map(Message::View),
                panel
                    .source
                    .hardware_action_id("tenshu.hardware.purpose.bip84"),
            );
            let generation = panel.source.hardware_mut().generation_for_test();
            let action_scope = panel.source.hardware_mut().action_scope_for_test();
            let loaded = panel.update_view(
                ImportMessage::Source(wallet_source_form::Message::Hardware(loaded_for_test(
                    generation,
                    succeeds,
                    action_scope,
                ))),
                None,
                0,
            );
            assert_focus_task(
                loaded,
                panel.setup_view().map(Message::View),
                panel
                    .source
                    .hardware_action_id("tenshu.hardware.read:5:dev-1"),
            );
            assert_eq!(panel.source.hardware_account().is_some(), succeeds);
            if succeeds {
                let _ = panel.update_view(
                    ImportMessage::Source(wallet_source_form::Message::Hardware(
                        HardwareMessage::PurposeSelected(crate::split_hardware::Purpose::Bip84),
                    )),
                    None,
                    0,
                );
                assert!(panel.source.hardware_account().is_some());
                let _ = panel.update_view(
                    ImportMessage::Source(wallet_source_form::Message::Hardware(
                        HardwareMessage::PurposeSelected(crate::split_hardware::Purpose::Bip49),
                    )),
                    None,
                    0,
                );
                assert!(panel.source.hardware_account().is_none());
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn hardware_result_from_one_import_tab_cannot_focus_another() {
        use crate::services::public_hardware_source::tests::{
            add_fake_device, assert_focus_task, assert_focus_task_outcome, loaded_for_test,
            unique_temp_root,
        };
        use crate::split_hardware::HardwareMessage;

        let root = unique_temp_root("import-hardware-focus-ownership");
        let mut first = ImportedBitcoinPanel::new(
            NetworkDirectory::new(root.join("first")),
            "same-cube".into(),
            BitcoinImport::pending(),
        );
        let mut second = ImportedBitcoinPanel::new(
            NetworkDirectory::new(root.join("second")),
            "same-cube".into(),
            BitcoinImport::pending(),
        );
        for panel in [&mut first, &mut second] {
            panel.source.select_source(Source::Hardware);
            add_fake_device(panel.source.hardware_mut(), &root);
        }
        let first_read = first
            .source
            .hardware_action_id("tenshu.hardware.read:5:dev-1");
        let second_read = second
            .source
            .hardware_action_id("tenshu.hardware.read:5:dev-1");
        assert_ne!(first_read, second_read);
        let _ = first.update_view(
            ImportMessage::Source(wallet_source_form::Message::Hardware(
                HardwareMessage::Read("dev-1".into()),
            )),
            None,
            0,
        );
        let generation = first.source.hardware_mut().generation_for_test();
        let action_scope = first.source.hardware_mut().action_scope_for_test();
        let first_generation = generation;
        let completion = || {
            ImportMessage::Source(wallet_source_form::Message::Hardware(loaded_for_test(
                generation,
                true,
                action_scope,
            )))
        };
        assert_focus_task_outcome(
            first.update_view(completion(), None, 0),
            second.setup_view().map(Message::View),
            None,
            None,
        );
        // Re-enter Reading to get another matching completion for the focused case.
        let _ = first.update_view(
            ImportMessage::Source(wallet_source_form::Message::Hardware(
                HardwareMessage::Read("dev-1".into()),
            )),
            None,
            0,
        );
        let generation = first.source.hardware_mut().generation_for_test();
        let action_scope = first.source.hardware_mut().action_scope_for_test();
        let loaded = first.update_view(
            ImportMessage::Source(wallet_source_form::Message::Hardware(loaded_for_test(
                generation,
                true,
                action_scope,
            ))),
            None,
            0,
        );
        assert_focus_task_outcome(
            loaded,
            second.setup_view().map(Message::View),
            Some(second_read.clone()),
            Some(second_read),
        );
        let _ = first.update_view(
            ImportMessage::Source(wallet_source_form::Message::Hardware(
                HardwareMessage::Read("dev-1".into()),
            )),
            None,
            0,
        );
        let generation = first.source.hardware_mut().generation_for_test();
        let action_scope = first.source.hardware_mut().action_scope_for_test();
        let loaded = first.update_view(
            ImportMessage::Source(wallet_source_form::Message::Hardware(loaded_for_test(
                generation,
                true,
                action_scope,
            ))),
            None,
            0,
        );
        assert_focus_task(loaded, first.setup_view().map(Message::View), first_read);
        let mut recreated = ImportedBitcoinPanel::new(
            NetworkDirectory::new(root.join("recreated")),
            "same-cube".into(),
            BitcoinImport::pending(),
        );
        assert_ne!(
            recreated
                .source
                .hardware_action_id("tenshu.hardware.read:5:dev-1"),
            first
                .source
                .hardware_action_id("tenshu.hardware.read:5:dev-1")
        );
        recreated.source.select_source(Source::Hardware);
        add_fake_device(recreated.source.hardware_mut(), &root);
        let _ = recreated.update_view(
            ImportMessage::Source(wallet_source_form::Message::Hardware(
                HardwareMessage::Read("dev-1".into()),
            )),
            None,
            0,
        );
        assert_eq!(
            recreated.source.hardware_mut().generation_for_test(),
            first_generation
        );
        let old_owner = recreated.update_view(
            ImportMessage::Source(wallet_source_form::Message::Hardware(loaded_for_test(
                first_generation,
                true,
                action_scope,
            ))),
            None,
            0,
        );
        assert_focus_task_outcome(
            old_owner,
            recreated.setup_view().map(Message::View),
            None,
            None,
        );
        assert!(matches!(
            recreated.source.hardware_mut().status(),
            crate::split_hardware::HardwareStatus::Reading(_)
        ));
        assert!(recreated.source.hardware_account().is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    use coincube_core::miniscript::bitcoin::{
        absolute::LockTime,
        bip32::{ChildNumber, DerivationPath, Xpriv, Xpub},
        secp256k1::Secp256k1,
        transaction, Amount, OutPoint, Transaction, Txid,
    };
    use coincubed::commands::{
        GetInfoDescriptors, ImportedWalletSync, LCSpendInfo, ListCoinsEntry, ListCoinsResult,
    };
    use serde::{de::DeserializeOwned, Serialize};

    #[derive(Debug)]
    struct NoRpcClient;

    #[async_trait::async_trait]

    impl crate::daemon::client::Client for NoRpcClient {
        type Error = crate::daemon::DaemonError;

        async fn request<
            S: Serialize + std::fmt::Debug + Send,
            D: DeserializeOwned + std::fmt::Debug + Send,
        >(
            &self,
            _method: &str,
            _params: Option<S>,
        ) -> Result<D, Self::Error> {
            unreachable!("binding a daemon must not make an RPC request")
        }
    }

    #[test]
    fn import_calendar_preserves_text_until_selection_and_validates_dates() {
        let directory = NetworkDirectory::new(std::env::temp_dir().join("import-calendar-test"));
        let mut panel =
            ImportedBitcoinPanel::new(directory, "calendar".into(), BitcoinImport::pending());
        macro_rules! send {
            ($message:expr) => {
                let _ = panel.update_view($message, None, 0);
            };
        }
        send!(ImportMessage::ScanFromBeginning(false));
        send!(ImportMessage::BirthdayEdited("2024-02-".into()));
        send!(ImportMessage::BirthdayCalendarOpen);
        assert!(panel.birthday_calendar_open);
        send!(ImportMessage::BirthdayCalendarCancel);
        assert_eq!(panel.birthday, "2024-02-");
        assert!(!panel.birthday_calendar_open);
        send!(ImportMessage::BirthdayCalendarOpen);
        let leap_day = chrono::NaiveDate::from_ymd_opt(2024, 2, 29).unwrap();
        send!(ImportMessage::BirthdaySelected(leap_day));
        assert_eq!(panel.birthday, "2024-02-29");
        assert!(!panel.birthday_calendar_open);
        let timestamp = birthday_timestamp(leap_day).unwrap();
        assert_eq!(
            chrono::DateTime::from_timestamp(i64::from(timestamp), 0)
                .unwrap()
                .date_naive(),
            leap_day
        );
        assert!(birthday_timestamp(chrono::NaiveDate::from_ymd_opt(2009, 1, 3).unwrap()).is_ok());
        send!(ImportMessage::BirthdayCalendarOpen);
        send!(ImportMessage::BirthdaySelected(
            chrono::Utc::now().date_naive().succ_opt().unwrap()
        ));
        assert_eq!(panel.birthday, "2024-02-29");
        assert!(panel.error.is_some());
        assert!(birthday_timestamp(chrono::NaiveDate::from_ymd_opt(2008, 1, 1).unwrap()).is_err());
        send!(ImportMessage::BirthdayCalendarOpen);
        send!(ImportMessage::ScanFromBeginning(true));
        assert!(!panel.birthday_calendar_open);
        send!(ImportMessage::BirthdaySelected(
            chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap()
        ));
        assert_eq!(panel.birthday, "2024-02-29");
        send!(ImportMessage::BirthdayCalendarOpen);
        assert!(!panel.birthday_calendar_open);
    }

    #[test]
    fn shared_runtime_provider_metadata_labels_custom_esplora() {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, &[44; 32]).unwrap();
        let account = master
            .derive_priv(&secp, &DerivationPath::from_str("m/84'/0'/0'").unwrap())
            .unwrap();
        let key = format!(
            "[{}/84'/0'/0']{}",
            master.fingerprint(&secp),
            Xpub::from_priv(&secp, &account)
        );
        let descriptor = ImportedWalletDescriptor::parse(
            &format!("wpkh({key}/0/*)"),
            Some(&format!("wpkh({key}/1/*)")),
            ChainId::Bitcoin,
        )
        .unwrap();
        let directory = NetworkDirectory::new(
            std::env::temp_dir().join(format!("import-provider-test-{}", uuid::Uuid::new_v4())),
        );
        let wallet =
            ImportedWalletAttachment::new(descriptor.clone(), ChainId::Bitcoin, 0, 200).unwrap();
        let mut import = BitcoinImport::pending();
        import.stage = BitcoinImportStage::Ready {
            wallet: wallet.clone(),
        };
        let mut panel =
            ImportedBitcoinPanel::new(directory.clone(), "cube-1".into(), import.clone());
        let mut config = Config::new(
            BitcoinConfig::new(ChainId::Bitcoin, Duration::from_secs(30)),
            Some(BitcoinBackend::Esplora(EsploraConfig {
                addr: "http://localhost:18184".into(),
                token: None,
                fallback_addr: None,
                fallback_token: None,
                secondary_fallback_addr: None,
                secondary_fallback_token: None,
            })),
            log::LevelFilter::Info,
            WalletDescriptor::Imported(descriptor.clone()),
            directory.coincubed_data_directory(&wallet.wallet_id),
        );
        config.import_birthday = Some(0);
        config.import_initial_lookahead = Some(200);

        // A loader-owned daemon shares its handle without starting a second
        // daemon, while the Vault settings still reflect its actual server.
        let daemon: Arc<dyn Daemon + Send + Sync> =
            Arc::new(EmbeddedDaemon::unstarted_for_test(config.clone(), None));
        let _refresh = panel.bind_runtime(daemon.clone());
        assert!(Arc::ptr_eq(panel.daemon.as_ref().unwrap(), &daemon));
        assert!(!panel.owns_daemon);
        assert_eq!(panel.provider, Provider::Esplora);
        assert_eq!(
            public_server_label(&panel.endpoint),
            "http://localhost:18184"
        );
        assert_eq!(panel.provider.label(), "My Esplora server");

        // The external daemon exposes no Config. Binding reads the same
        // installed metadata without starting another daemon or making RPCs.
        let path = directory
            .coincubed_data_directory(&wallet.wallet_id)
            .path()
            .join("daemon.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, toml::to_string(&config.for_persistence()).unwrap()).unwrap();
        let mut external_panel =
            ImportedBitcoinPanel::new(directory.clone(), "cube-1".into(), import);
        let external: Arc<dyn Daemon + Send + Sync> =
            Arc::new(crate::daemon::client::Coincubed::new(NoRpcClient));
        let _refresh = external_panel.bind_runtime(external.clone());
        assert!(Arc::ptr_eq(
            external_panel.daemon.as_ref().unwrap(),
            &external
        ));
        assert_eq!(external_panel.provider, Provider::Esplora);
        assert_eq!(
            public_server_label(&external_panel.endpoint),
            "http://localhost:18184"
        );
        std::fs::remove_dir_all(directory.path()).unwrap();
    }

    #[test]
    fn server_caption_omits_credentials_path_and_query() {
        assert_eq!(
            public_server_label("https://name:secret@example.com:8443/token-path?api_key=secret"),
            "https://example.com:8443"
        );
    }

    fn ready_import_panel() -> ImportedBitcoinPanel {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, &[43; 32]).unwrap();
        let account = master
            .derive_priv(&secp, &DerivationPath::from_str("m/84'/0'/0'").unwrap())
            .unwrap();
        let key = format!(
            "[{}/84'/0'/0']{}",
            master.fingerprint(&secp),
            Xpub::from_priv(&secp, &account)
        );
        let descriptor = ImportedWalletDescriptor::parse(
            &format!("wpkh({key}/0/*)"),
            Some(&format!("wpkh({key}/1/*)")),
            ChainId::Bitcoin,
        )
        .unwrap();
        let wallet =
            ImportedWalletAttachment::new(descriptor.clone(), ChainId::Bitcoin, 0, 200).unwrap();
        let mut import = BitcoinImport::pending();
        import.stage = BitcoinImportStage::Ready { wallet };
        let directory = NetworkDirectory::new(std::env::temp_dir().join("import-ready-test"));
        let mut panel = ImportedBitcoinPanel::new(directory, "cube-1".into(), import);
        panel.set_hardware_root(CoincubeDirectory::new(
            std::env::temp_dir().join("import-ready-root-test"),
        ));
        panel.info = Some(model::GetInfoResult {
            version: "test".into(),
            network: Network::Bitcoin,
            chain_id: Some(ChainId::Bitcoin),
            block_height: 1,
            sync: 1.0,
            descriptors: GetInfoDescriptors {
                main: WalletDescriptor::Imported(descriptor.clone()),
            },
            imported_wallet: Some(ImportedWalletSync {
                wallet_identity: descriptor.identity(ChainId::Bitcoin).unwrap().to_string(),
                chain_id: ChainId::Bitcoin,
                birthday: 0,
                initial_lookahead: 200,
                discovery_target: 200,
                receive_scanned: 200,
                change_scanned: 200,
                history_complete: true,
            }),
            rescan_progress: None,
            refused_reorg_depth: None,
            chain_divergence: false,
            timestamp: 0,
            last_poll_timestamp: Some(1),
            history_sync: Default::default(),
            receive_index: 0,
            change_index: 0,
        });
        panel.coins = Some(ListCoinsResult { coins: Vec::new() });
        panel
    }

    #[tokio::test]
    async fn imported_receive_shares_source_claim_lock_and_releases_after_daemon_error() {
        let panel = ready_import_panel();
        let imported = panel.import.active_attachment().unwrap().descriptor.clone();
        let source = coincube_core::public_wallet_source::SplitSource::new(
            imported.receive_descriptor().clone(),
            Some(imported.change_descriptor().clone()),
        )
        .unwrap();
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("receive-claim-lock-{}", uuid::Uuid::new_v4())),
        );
        let config = Config::new(
            BitcoinConfig::new(ChainId::Bitcoin, Duration::from_secs(30)),
            None,
            log::LevelFilter::Off,
            WalletDescriptor::Imported(imported.clone()),
            root.network_directory(ChainId::Bitcoin)
                .coincubed_data_directory(&panel.import.active_attachment().unwrap().wallet_id),
        );
        let daemon: Arc<dyn Daemon + Send + Sync> =
            Arc::new(EmbeddedDaemon::unstarted_for_test(config, None));
        let guard =
            bitcoin_import::pairing::source_lock::source_reservation_lock(&root, &source).unwrap();
        for descriptor in [imported.clone(), imported.paired_blake2b().unwrap()] {
            let error = reserve_receive_address(root.clone(), descriptor.into(), daemon.clone())
                .await
                .unwrap_err();
            assert!(error.contains("reserving addresses"), "{}", error);
        }
        drop(guard);
        // A failed daemon request must drop the allocation guard too. The
        // source and paired target can each retry rather than keep a stale lock.
        for descriptor in [imported.clone(), imported.paired_blake2b().unwrap()] {
            let error = reserve_receive_address(root.clone(), descriptor.into(), daemon.clone())
                .await
                .unwrap_err();
            assert!(!error.contains("reserving addresses"), "{}", error);
            assert!(
                bitcoin_import::pairing::source_lock::source_reservation_lock(&root, &source)
                    .is_ok()
            );
        }
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn paired_imported_discovery_refusal_preserves_shared_runtime_and_snapshot() {
        let mut panel = ready_import_panel();
        let original = panel.import.active_attachment().unwrap();
        let descriptor = original.descriptor.paired_blake2b().unwrap();
        let wallet = ImportedWalletAttachment::new(
            descriptor.clone(),
            ChainId::BitcoinBlake2b,
            original.birthday,
            original.initial_lookahead,
        )
        .unwrap();
        panel.import.stage = BitcoinImportStage::Ready { wallet };
        let info = panel.info.as_mut().unwrap();
        info.chain_id = Some(ChainId::BitcoinBlake2b);
        info.network = ChainId::BitcoinBlake2b.bitcoin_network();
        info.descriptors.main = WalletDescriptor::Imported(descriptor.clone());
        let status = info.imported_wallet.as_mut().unwrap();
        status.chain_id = ChainId::BitcoinBlake2b;
        status.wallet_identity = descriptor
            .identity(ChainId::BitcoinBlake2b)
            .unwrap()
            .to_string();
        let daemon: Arc<dyn Daemon + Send + Sync> =
            Arc::new(crate::daemon::client::Coincubed::new(NoRpcClient));
        panel.daemon = Some(daemon.clone());
        panel.owns_daemon = false;
        assert!(panel.is_ready());
        assert!(panel.verified_address_state().is_some());
        let generation = panel.generation;
        let before = serde_json::to_value(panel.info.as_ref().unwrap()).unwrap();
        let _ = panel.update_view(ImportMessage::ExpandDiscovery, None, 0);
        assert!(Arc::ptr_eq(panel.daemon.as_ref().unwrap(), &daemon));
        assert!(!panel.owns_daemon);
        assert_eq!(panel.generation, generation);
        assert_eq!(panel.generation_gate.load(Ordering::SeqCst), generation);
        assert_eq!(
            serde_json::to_value(panel.info.as_ref().unwrap()).unwrap(),
            before
        );
        assert!(panel.coins.is_some());
        assert!(panel.is_ready());
        assert!(!panel.busy);
        assert!(panel
            .error
            .as_deref()
            .unwrap()
            .contains("original Bitcoin Cube"));

        // The original Bitcoin wallet retains the supported expansion route.
        let mut bitcoin = ready_import_panel();
        bitcoin.daemon = Some(daemon);
        let generation = bitcoin.generation;
        let _ = bitcoin.update_view(ImportMessage::ExpandDiscovery, None, 0);
        assert!(bitcoin.daemon.is_none());
        assert_ne!(bitcoin.generation, generation);
        assert!(bitcoin.info.is_none());
        assert!(bitcoin.busy);
    }

    #[test]
    fn scan_display_uses_only_reported_progress_and_never_claims_completion_early() {
        let mut info = ready_import_panel().info.unwrap();
        info.history_sync = coincubed::commands::HistorySync {
            full_scan_in_progress: true,
            addresses_checked: 100,
            addresses_expected: 400,
            ..Default::default()
        };
        assert_eq!(
            ScanDisplay::from_info(Some(&info), false),
            ScanDisplay::Scanning {
                checked: 100,
                expected: 400,
                progress: 0.25
            }
        );
        info.history_sync.addresses_checked = 600;
        assert_eq!(
            ScanDisplay::from_info(Some(&info), false),
            ScanDisplay::Scanning {
                checked: 600,
                expected: 400,
                progress: 0.99
            }
        );
        info.history_sync.addresses_expected = 0;
        assert_eq!(
            ScanDisplay::from_info(Some(&info), false),
            ScanDisplay::Starting
        );
        info.history_sync.full_scan_in_progress = false;
        info.history_sync.full_scan_completed_at = Some(1);
        assert_eq!(
            ScanDisplay::from_info(Some(&info), false),
            ScanDisplay::Saving
        );
        assert_eq!(
            ScanDisplay::from_info(Some(&info), true),
            ScanDisplay::Retrying
        );
    }

    #[test]
    fn progress_poll_keeps_partial_balance_unknown_and_recovers_after_error() {
        let mut panel = ready_import_panel();
        panel.coins = None;
        let mut scanning = panel.info.take().unwrap();
        scanning.imported_wallet.as_mut().unwrap().history_complete = false;
        scanning.history_sync = coincubed::commands::HistorySync {
            full_scan_in_progress: true,
            addresses_checked: 20,
            addresses_expected: 400,
            ..Default::default()
        };
        panel.progress_polling = true;
        let generation = panel.generation;
        let _ = panel.update_event(ImportEvent::ProgressChecked(
            generation,
            Ok(scanning.clone()),
        ));
        assert!(!panel.progress_polling);
        assert!(!panel.is_ready());
        assert!(panel.authoritative_balance().is_none());
        assert!(matches!(
            ScanDisplay::from_info(panel.info.as_ref(), panel.sync_failed),
            ScanDisplay::Scanning { progress, .. } if (progress - 0.05).abs() < f64::EPSILON
        ));

        panel.progress_polling = true;
        let _ = panel.update_event(ImportEvent::ProgressChecked(
            generation,
            Err("server error".into()),
        ));
        assert!(panel.sync_failed);
        assert!(!panel.progress_polling);
        assert_eq!(
            panel.synchronization_action_label(),
            "Retry synchronization"
        );
        assert!(panel
            .error
            .as_deref()
            .unwrap()
            .contains("retry synchronization"));

        panel.progress_polling = true;
        let _ = panel.update_event(ImportEvent::ProgressChecked(generation, Ok(scanning)));
        assert!(!panel.sync_failed);
        assert!(panel.error.is_none());
        assert!(!panel.is_ready());
    }

    #[test]
    fn progress_status_from_another_wallet_keeps_retry_actionable() {
        let mut panel = ready_import_panel();
        panel.info = None;
        panel.coins = None;
        let mut wrong = ready_import_panel().info.unwrap();
        wrong.imported_wallet.as_mut().unwrap().wallet_identity = "another-wallet".into();
        let generation = panel.generation;
        let _ = panel.update_event(ImportEvent::ProgressChecked(generation, Ok(wrong)));
        assert!(panel.sync_failed);
        assert!(panel.info.is_none());
        assert!(panel.error.as_deref().unwrap().contains("different wallet"));
        assert_eq!(
            panel.synchronization_action_label(),
            "Retry synchronization"
        );

        let mut correct = ready_import_panel().info.unwrap();
        correct.imported_wallet.as_mut().unwrap().history_complete = false;
        let _ = panel.update_event(ImportEvent::ProgressChecked(generation, Ok(correct)));
        assert!(!panel.sync_failed);
        assert!(panel.error.is_none());
        assert!(!panel.is_ready());
        panel.progress_polling = true;
        panel.invalidate();
        assert!(!panel.progress_polling);
    }

    #[test]
    fn completed_status_alone_cannot_reuse_an_old_coin_snapshot() {
        let mut panel = ready_import_panel();
        let complete = panel.info.take().unwrap();
        assert!(panel.coins.is_some());
        let generation = panel.generation;
        let _ = panel.update_event(ImportEvent::ProgressChecked(generation, Ok(complete)));
        assert!(panel.coins.is_none());
        assert!(!panel.is_ready());
        assert!(panel.authoritative_balance().is_none());
    }

    #[test]
    fn explicit_retry_wakes_sync_without_blocking_live_status_polling() {
        let mut panel = ready_import_panel();
        panel.info.as_mut().unwrap().history_sync.last_failure =
            Some(coincubed::commands::PollFailure {
                message: "server unavailable".into(),
                since: 1,
                last_at: 1,
                consecutive: 1,
            });
        let wallet = panel.import.active_attachment().unwrap();
        let config = Config::new(
            BitcoinConfig::new(ChainId::Bitcoin, Duration::from_secs(600)),
            None,
            log::LevelFilter::Info,
            WalletDescriptor::Imported(wallet.descriptor.clone()),
            panel.directory.coincubed_data_directory(&wallet.wallet_id),
        );
        panel.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(config, None)));

        let _ = panel.update_view(ImportMessage::RetrySynchronization, None, 0);
        assert!(panel.sync_requesting);
        assert!(!panel.busy);
        let generation = panel.generation;
        let _ = panel.update_event(ImportEvent::SynchronizationRequested(generation, Ok(())));
        assert!(!panel.sync_requesting);
        assert!(panel.progress_polling);
    }

    #[test]
    fn failed_full_snapshot_waits_for_retry_or_normal_refresh() {
        let mut panel = ready_import_panel();
        let complete = panel.info.as_ref().unwrap().clone();
        let wallet = panel.import.active_attachment().unwrap();
        let config = Config::new(
            BitcoinConfig::new(ChainId::Bitcoin, Duration::from_secs(600)),
            None,
            log::LevelFilter::Info,
            WalletDescriptor::Imported(wallet.descriptor.clone()),
            panel.directory.coincubed_data_directory(&wallet.wallet_id),
        );
        panel.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(config, None)));
        let generation = panel.generation;
        let _ = panel.update_event(ImportEvent::Refreshed(generation, Err("disk error".into())));
        assert!(panel.snapshot_failed);
        let failure = panel.error.clone();

        for _ in 0..2 {
            let _ = panel.update_event(ImportEvent::ProgressChecked(
                generation,
                Ok(complete.clone()),
            ));
            assert!(
                !panel.busy,
                "status polling must not requeue a full snapshot"
            );
            assert!(panel.snapshot_failed);
            assert_eq!(panel.error, failure);
            assert!(panel.coins.is_none());
        }

        let _ = panel.update_view(ImportMessage::RetrySynchronization, None, 0);
        assert!(!panel.snapshot_failed);
        let _ = panel.update_event(ImportEvent::SynchronizationRequested(generation, Ok(())));
        let _ = panel.update_event(ImportEvent::ProgressChecked(generation, Ok(complete)));
        assert!(
            panel.busy,
            "explicit retry may start the full snapshot again"
        );

        let _ = panel.update_event(ImportEvent::Refreshed(generation, Err("disk error".into())));
        assert!(panel.snapshot_failed);
        let _ = panel.update_view(ImportMessage::Refresh, None, 0);
        assert!(!panel.snapshot_failed);
        assert!(
            panel.busy,
            "the normal slower refresh may retry the snapshot"
        );
    }

    #[test]
    fn busy_ready_startup_tracks_scan_but_old_status_cannot_undo_receive_reservation() {
        let mut panel = ready_import_panel();
        let mut scanning = panel.info.take().unwrap();
        scanning.imported_wallet.as_mut().unwrap().history_complete = false;
        scanning.history_sync = coincubed::commands::HistorySync {
            full_scan_in_progress: true,
            addresses_checked: 50,
            addresses_expected: 400,
            ..Default::default()
        };
        panel.busy = true;
        let generation = panel.generation;
        let _ = panel.update_event(ImportEvent::ProgressChecked(generation, Ok(scanning)));
        assert_eq!(
            panel.info.as_ref().unwrap().history_sync.addresses_checked,
            50
        );

        panel.busy = false;
        let wallet = panel.import.active_attachment().unwrap();
        let config = Config::new(
            BitcoinConfig::new(ChainId::Bitcoin, Duration::from_secs(600)),
            None,
            log::LevelFilter::Info,
            WalletDescriptor::Imported(wallet.descriptor.clone()),
            panel.directory.coincubed_data_directory(&wallet.wallet_id),
        );
        panel.daemon = Some(Arc::new(EmbeddedDaemon::unstarted_for_test(config, None)));
        // The real Receive action is available only after history is complete.
        let completed = ready_import_panel().info.unwrap();
        panel.info = Some(completed.clone());
        panel.coins = Some(ListCoinsResult { coins: Vec::new() });
        let old_generation = panel.generation;
        panel.progress_polling = true;
        panel.sync_requesting = true;
        let _ = panel.update_view(ImportMessage::Receive, None, 0);
        assert!(panel.address_reserving);
        assert!(!panel.sync_requesting);
        assert_ne!(panel.generation, old_generation);
        assert!(panel.info.is_none());
        let new_generation = panel.generation;
        let _ = panel.update_event(ImportEvent::Address(new_generation, Err("retry".into())));
        assert!(!panel.address_reserving);
        let _ = panel.update_event(ImportEvent::ProgressChecked(old_generation, Ok(completed)));
        assert!(panel.info.is_none());
        assert!(panel.verified_address_state().is_none());
    }

    #[test]
    fn completed_import_stays_ready_after_receive_or_change_allocation() {
        let mut panel = ready_import_panel();
        assert!(panel.is_ready());

        // Ordinary address allocation advances durable high-water indices but
        // does not invalidate the attested, completed historical scan.
        panel.info.as_mut().unwrap().receive_index = 1;
        panel.info.as_mut().unwrap().change_index = 1;
        assert!(panel.is_ready());

        // A restored or wider scan clears the durable completion marker first.
        panel
            .info
            .as_mut()
            .unwrap()
            .imported_wallet
            .as_mut()
            .unwrap()
            .history_complete = false;
        assert!(!panel.is_ready());
        let status = panel
            .info
            .as_mut()
            .unwrap()
            .imported_wallet
            .as_mut()
            .unwrap();
        status.history_complete = true;
        status.discovery_target = 300;
        assert!(!panel.is_ready());
    }

    #[test]
    fn imported_home_available_balance_matches_vault_coin_ownership() {
        let mut panel = ready_import_panel();
        let txid =
            Txid::from_str("f7bd1b2a995b689d326e51eb742eb1088c4a8f110d9cb56128fd553acc9f88e5")
                .unwrap();
        let address = coincube_core::miniscript::bitcoin::Address::from_str(
            "bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg",
        )
        .unwrap()
        .assume_checked();
        let coin = |vout, sats, block_height, is_from_self, is_change, spend_info| ListCoinsEntry {
            outpoint: OutPoint::new(txid, vout),
            amount: Amount::from_sat(sats),
            address: address.clone(),
            derivation_index: ChildNumber::Normal { index: vout },
            block_height,
            is_immature: false,
            is_change,
            is_from_self,
            spend_info,
        };
        panel.coins = Some(ListCoinsResult {
            coins: vec![
                coin(0, 1_000, Some(1), false, false, None), // confirmed incoming
                coin(1, 2_000, None, false, false, None),    // pending external incoming
                coin(2, 3_000, None, true, true, None),      // pending self-change
                coin(
                    3,
                    4_000,
                    Some(1),
                    false,
                    false,
                    Some(LCSpendInfo {
                        txid,
                        height: Some(2),
                    }),
                ), // spent coin
            ],
        });

        let mut cache = Cache::default();
        assert!(panel.sync_cache(&mut cache));
        assert_eq!(cache.coins().len(), 4);
        let (available, pending, _, _) = crate::app::state::coins_summary(cache.coins(), 1, None);
        assert_eq!(available, Amount::from_sat(4_000));
        assert_eq!(pending, Amount::from_sat(2_000));
        assert_eq!(panel.authoritative_balance(), Some(available));
    }

    #[test]
    fn late_event_from_closed_panel_cannot_mutate_reopened_cube() {
        let directory =
            NetworkDirectory::new(std::env::temp_dir().join("import-panel-generation-test"));
        let import = BitcoinImport::pending();
        let mut old = ImportedBitcoinPanel::new(directory.clone(), "cube-1".into(), import.clone());
        let old_generation = old.generation;
        old.stop();

        let mut reopened = ImportedBitcoinPanel::new(directory, "cube-1".into(), import);
        assert_ne!(old_generation, reopened.generation);
        let _ = reopened.update_event(ImportEvent::Abandoned(old_generation, Ok(())));
        assert!(!reopened.abandoned());
    }

    #[test]
    fn backend_error_pages_are_not_shown_and_offer_synchronization_retry() {
        let directory = NetworkDirectory::new(std::env::temp_dir().join("import-panel-error-test"));
        let mut panel =
            ImportedBitcoinPanel::new(directory, "cube-1".into(), BitcoinImport::pending());
        let generation = panel.generation;
        let raw = "Error setting up Esplora: <!DOCTYPE HTML><html>404</html>".to_string();

        let _ = panel.update_event(ImportEvent::Opened(generation, Err(raw.clone())));
        assert_eq!(
            panel.synchronization_action_label(),
            "Retry synchronization"
        );
        assert!(!panel.error.as_deref().unwrap().contains("<!DOCTYPE"));
        assert!(panel.error.as_deref().unwrap().contains("Bitcoin server"));

        let _ = panel.update_event(ImportEvent::Refreshed(generation, Err(raw)));
        assert_eq!(
            panel.synchronization_action_label(),
            "Retry synchronization"
        );
        assert!(!panel.error.as_deref().unwrap().contains("<!DOCTYPE"));
    }

    use crate::home::restore_frame::rendered_labels;

    fn assert_labels(shown: &[String], present: &[&str], absent: &[&str]) {
        for label in present {
            assert!(
                shown.iter().any(|text| text == label),
                "missing {:?} in {:?}",
                label,
                shown
            );
        }
        for label in absent {
            assert!(
                !shown.iter().any(|text| text == label),
                "unexpected {:?} in {:?}",
                label,
                shown
            );
        }
    }

    #[tokio::test]
    async fn a_pending_import_opens_import_or_restore_at_source() {
        let directory = NetworkDirectory::new(std::env::temp_dir().join("import-journey-source"));
        let mut panel =
            ImportedBitcoinPanel::new(directory, "cube-1".into(), BitcoinImport::pending());
        let cache = Cache {
            cube_name: "Family savings".into(),
            ..Cache::default()
        };
        assert!(panel.journey_open());
        assert_eq!(panel.journey_step(), JourneyStep::Source);
        let shown = rendered_labels(panel.journey_view(&cache, false)).await;
        assert_labels(
            &shown,
            &[
                "Import or restore",
                "Bitcoin SHA-256 · Mainnet",
                "Overview",
                "Import Bitcoin wallet",
                "Cube",
                "Backup",
                "Source",
                "Scan",
                "Done",
                "Review wallet",
                "Use a new Vault instead",
                "Delete this Cube",
                "Close",
            ],
            // The Cube's own menu and the pre-redesign title stay out.
            &["Passkey", "Import a Bitcoin wallet", "Vault", "Open Cube"],
        );
        // Import sets a PIN only, so even a passkey Cube's rail has no
        // Passkey step.
        let passkey = rendered_labels(panel.journey_view(&cache, true)).await;
        assert_labels(&passkey, &["Cube"], &["Passkey"]);

        // Close abandons an import that never installed its wallet: it asks
        // the Launcher to delete the Cube. Kept there, the Cube's Vault page
        // reopens the import.
        let task = panel.update_view(ImportMessage::LeaveJourney, None, 0);
        assert!(asks_to_delete_the_cube(task).await);
        assert!(!panel.journey_open());
        let resume = rendered_labels(panel.resume_view()).await;
        assert_labels(
            &resume,
            &[
                "Finish importing your Bitcoin wallet",
                "Continue import",
                "Delete this Cube",
            ],
            &["Review wallet"],
        );
        let _ = panel.update_view(ImportMessage::ResumeJourney, None, 0);
        assert!(panel.journey_open());

        // Close cannot leave while an import is being written.
        panel.busy = true;
        let _ = panel.update_view(ImportMessage::LeaveJourney, None, 0);
        assert!(panel.journey_open());
    }

    #[test]
    fn the_cube_can_be_deleted_only_until_a_wallet_is_installed() {
        let directory = NetworkDirectory::new(std::env::temp_dir().join("import-journey-delete"));
        let mut panel =
            ImportedBitcoinPanel::new(directory, "cube-1".into(), BitcoinImport::pending());
        assert!(panel.can_delete_cube());
        panel.busy = true;
        assert!(!panel.can_delete_cube());
        panel.busy = false;
        panel.keys_stored = KeysStored::Storing;
        assert!(!panel.can_delete_cube());
        panel.keys_stored = KeysStored::Stored(1);
        assert!(!panel.can_delete_cube());
        panel.keys_stored = KeysStored::Failed("synthetic".into());
        assert!(panel.can_delete_cube());

        // An installed wallet makes it a Cube with a wallet, deleted only
        // from the Launcher.
        assert!(!ready_import_panel().can_delete_cube());
    }

    #[tokio::test]
    async fn the_scan_continues_to_done_and_open_cube_returns_to_the_cube() {
        let mut panel = ready_import_panel();
        let cache = Cache {
            cube_name: "Family savings".into(),
            ..Cache::default()
        };
        // A Cube reopened with its wallet attached starts in the Cube.
        assert!(!panel.journey_open());
        // This session's import: the scan is still running.
        panel.journey = true;
        let info = panel.info.take();
        assert_eq!(panel.journey_step(), JourneyStep::Scan);
        let scan = rendered_labels(panel.journey_view(&cache, false)).await;
        assert_labels(
            &scan,
            &[
                "Finding your wallet's Bitcoin history",
                "Receive",
                "Wait for the complete Bitcoin history scan before receiving.",
                "Continue",
                "Close",
            ],
            &["Open Cube"],
        );
        // ResumeJourney only reopens an unfinished import.
        panel.journey = false;
        let _ = panel.update_view(ImportMessage::ResumeJourney, None, 0);
        assert!(!panel.journey_open());
        panel.journey = true;

        // A refresh during the scan keeps the panel busy; moving on still works.
        panel.busy = true;
        let _ = panel.update_view(ImportMessage::ContinueFromScan, None, 0);
        assert_eq!(panel.journey_step(), JourneyStep::Done);
        panel.busy = false;
        let early = rendered_labels(panel.journey_view(&cache, false)).await;
        assert_labels(
            &early,
            &[
                "Family savings is ready",
                "Your wallet is now this Cube's Vault. Its history appears when the scan finishes.",
                "Open Cube",
            ],
            // Done has no Back, tag chips or summary list.
            &[
                "Continue",
                "Close",
                "Back",
                "Recovery Kit: update it from Cube settings to include this wallet",
            ],
        );

        // A stopped scan says so on Done and offers its retry there.
        panel.sync_failed = true;
        let stalled = rendered_labels(panel.journey_view(&cache, false)).await;
        assert_labels(
            &stalled,
            &[
                "The history scan stopped before it finished. Retry it here or from the Vault page.",
                panel.synchronization_action_label(),
            ],
            &["Your wallet is now this Cube's Vault. Its history appears when the scan finishes."],
        );
        panel.sync_failed = false;

        // The finished scan reaches Done on its own.
        panel.scan_continued = false;
        panel.info = info;
        assert!(panel.is_ready());
        assert_eq!(panel.journey_step(), JourneyStep::Done);
        let done = rendered_labels(panel.journey_view(&cache, false)).await;
        assert_labels(
            &done,
            &["Your wallet is now this Cube's Vault. Open it to receive or review its history."],
            &[],
        );
        panel.busy = true;
        let task = panel.update_view(ImportMessage::LeaveJourney, None, 0);
        assert!(!panel.journey_open());
        // A finished import only closes; its Cube is never offered for
        // deletion.
        assert!(!asks_to_delete_the_cube(task).await);
        panel.busy = false;
        panel.journey = true;
        let task = panel.update_view(ImportMessage::LeaveJourney, None, 0);
        assert!(!asks_to_delete_the_cube(task).await);
        assert!(!panel.can_delete_cube());
    }

    async fn asks_to_delete_the_cube(task: Task<Message>) -> bool {
        use iced_runtime::futures::futures::StreamExt;
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            return false;
        };
        stream.collect::<Vec<_>>().await.iter().any(|action| {
            matches!(
                action,
                iced_runtime::Action::Output(Message::View(view::Message::DeleteUnfinishedCube))
            )
        })
    }

    /// Import or restore's Source, Review, Scan and Done through the wgpu
    /// renderer. Opt-in: `LAUNCHER_CAPTURE_DIR=<dir> cargo test -p
    /// coincube-gui --lib import_journey_capture -- --ignored`.
    #[test]
    #[ignore]
    fn import_journey_capture() {
        use crate::home::launcher_capture::{renderer, save};
        let Ok(directory) = std::env::var("LAUNCHER_CAPTURE_DIR") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let mut renderer = renderer().expect("wgpu headless renderer");
        let size = iced::Size::new(1440.0, 900.0);
        let ready = ready_import_panel();
        let wallet = ready.import.active_attachment().cloned().unwrap();
        for light in [false, true] {
            let appearance = if light { "light" } else { "dark" };
            let palette = if light {
                coincube_ui::theme::Theme::light()
            } else {
                coincube_ui::theme::Theme::dark()
            };
            let cache = Cache {
                cube_name: "Family savings".into(),
                theme_mode: if light {
                    coincube_ui::theme::palette::ThemeMode::Light
                } else {
                    coincube_ui::theme::palette::ThemeMode::Dark
                },
                ..Cache::default()
            };
            let directory_for = |name: &str| {
                NetworkDirectory::new(std::env::temp_dir().join(format!("import-capture-{name}")))
            };
            let source = ImportedBitcoinPanel::new(
                directory_for("source"),
                "cube-1".into(),
                BitcoinImport::pending(),
            );
            let mut review = ImportedBitcoinPanel::new(
                directory_for("review"),
                "cube-1".into(),
                BitcoinImport::pending(),
            );
            review.review = Some(wallet.clone());
            let mut scan = ready_import_panel();
            scan.journey = true;
            scan.info = None;
            let mut done = ready_import_panel();
            done.journey = true;
            let mut keyed = KeysState::new(&wallet.descriptor);
            keyed.slots[0].status = keys::KeyStatus::Verified;
            done.keys = Some(keyed);
            done.keys_stored = KeysStored::Stored(1);
            let multisig =
                ImportedWalletAttachment::new(two_of_three(), ChainId::Bitcoin, 0, 200).unwrap();
            let mut keys_step = ImportedBitcoinPanel::new(
                directory_for("keys"),
                "cube-1".into(),
                BitcoinImport::pending(),
            );
            let mut slots = KeysState::new(&multisig.descriptor);
            slots.slots[0].status = keys::KeyStatus::Verified;
            slots.slots[1].source = KeySource::Hardware;
            slots.slots[2].source = KeySource::Elsewhere;
            slots.slots[2].status = keys::KeyStatus::Verified;
            keys_step.keys = Some(slots);
            keys_step.review = Some(multisig);
            for (name, panel) in [
                ("source", &source),
                ("keys", &keys_step),
                ("review", &review),
                ("scan", &scan),
                ("done", &done),
            ] {
                save(
                    &mut renderer,
                    panel.journey_view(&cache, false),
                    size,
                    &palette,
                    directory.join(format!("import-{name}-{appearance}.png")),
                );
            }
        }
    }

    /// A Bitcoin account from a known phrase: its descriptor key text and
    /// its public account.
    fn phrase_account(entropy: u8) -> (String, coincube_core::imported_keys::ImportedAccount) {
        let mnemonic = bip39::Mnemonic::from_entropy(&[entropy; 16]).unwrap();
        let seed = mnemonic.to_seed("");
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, &seed).unwrap();
        let origin = DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();
        let xpub = Xpub::from_priv(&secp, &master.derive_priv(&secp, &origin).unwrap());
        let fingerprint = master.fingerprint(&secp);
        (
            format!("[{fingerprint}/48'/0'/0'/2']{xpub}"),
            coincube_core::imported_keys::ImportedAccount {
                fingerprint,
                origin,
                xpub,
            },
        )
    }

    fn phrase(entropy: u8) -> zeroize::Zeroizing<String> {
        zeroize::Zeroizing::new(
            bip39::Mnemonic::from_entropy(&[entropy; 16])
                .unwrap()
                .to_string(),
        )
    }

    /// A 2-of-3 wallet whose keys come from phrases 1, 2 and 3.
    fn two_of_three() -> ImportedWalletDescriptor {
        let keys: Vec<String> = (1..=3).map(|entropy| phrase_account(entropy).0).collect();
        let branch = |n: u8| {
            format!(
                "wsh(sortedmulti(2,{}/{n}/*,{}/{n}/*,{}/{n}/*))",
                keys[0], keys[1], keys[2]
            )
        };
        ImportedWalletDescriptor::parse(&branch(0), Some(&branch(1)), ChainId::Bitcoin).unwrap()
    }

    #[test]
    fn keys_lists_each_signer_and_the_threshold() {
        let wallet = two_of_three();
        let keys = KeysState::new(&wallet);
        assert_eq!(keys.slots.len(), 3);
        assert_eq!(keys.threshold, 2);
        assert!(keys.multisig());
        for entropy in 1..=3 {
            let (_, account) = phrase_account(entropy);
            assert!(keys.slots.iter().any(|slot| slot.account == account));
        }
        let single = ready_import_panel()
            .import
            .active_attachment()
            .cloned()
            .unwrap();
        let single = KeysState::new(&single.descriptor);
        assert_eq!((single.slots.len(), single.threshold), (1, 1));
        assert!(!single.multisig());
    }

    #[test]
    fn words_must_derive_the_exact_signer() {
        let wallet = two_of_three();
        let (_, first) = phrase_account(1);
        let (_, second) = phrase_account(2);
        let none = || zeroize::Zeroizing::new(String::new());
        assert_eq!(check_words(&wallet, &first, phrase(1), none()), Ok(()));
        // Another signer's words, a passphrase the wallet never used, or a
        // phrase outside the wallet: all refused.
        assert!(check_words(&wallet, &first, phrase(2), none()).is_err());
        assert!(check_words(&wallet, &second, phrase(1), none()).is_err());
        assert!(check_words(
            &wallet,
            &first,
            phrase(1),
            zeroize::Zeroizing::new("extra".into())
        )
        .is_err());
        assert!(check_words(&wallet, &first, phrase(9), none()).is_err());
        assert!(check_words(
            &wallet,
            &first,
            zeroize::Zeroizing::new("not words".into()),
            none()
        )
        .is_err());
    }

    #[test]
    fn enough_keys_to_sign_lets_the_rest_sign_elsewhere() {
        let wallet = two_of_three();
        let mut keys = KeysState::new(&wallet);
        assert!(!keys.ready());
        keys.slots[0].status = keys::KeyStatus::Verified;
        keys.slots[2].source = KeySource::Elsewhere;
        keys.slots[2].status = keys::KeyStatus::Verified;
        // One key of the two needed: not yet.
        assert!(!keys.ready());
        keys.slots[1].source = KeySource::Hardware;
        keys.slots[1].status = keys::KeyStatus::Verified;
        assert!(keys.ready());
        assert_eq!(keys.provided(), 2);
        let summary = keys.summary();
        assert!(summary.contains("2 of 3 provided"), "{}", summary);
        assert!(summary.contains("1 signs elsewhere"), "{}", summary);
        // A signer left unanswered signs elsewhere too: the threshold is met.
        keys.slots[2].source = KeySource::Hardware;
        keys.slots[2].status = keys::KeyStatus::Missing;
        assert!(keys.ready());
        assert!(keys.summary().contains("1 signs elsewhere"));
        // Not while a check is in flight.
        keys.slots[2].status = keys::KeyStatus::Checking;
        assert!(!keys.ready());
    }

    use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

    /// A readable device is shown in the row of the key it holds, once;
    /// a locked device or a non-signer is listed apart, with what to do.
    #[tokio::test]
    async fn a_matched_device_sits_in_its_keys_row() {
        let wallet = two_of_three();
        let keys = KeysState::new(&wallet);
        let third = keys.slots[2].account.fingerprint;
        let stranger = Fingerprint::from([0x9d, 0x0e, 0x11, 0xaa]);
        let devices = vec![
            keys::ListedDevice {
                name: "Jade".into(),
                fingerprint: None,
                problem: Some("Locked. Unlock it to see which key it holds.".into()),
                pairing_code: None,
            },
            keys::ListedDevice {
                name: "BitBox02".into(),
                fingerprint: None,
                problem: Some("Confirm this pairing code on the device:".into()),
                pairing_code: Some("7NQ3B Z4WKD".into()),
            },
            keys::ListedDevice {
                name: "Coldcard".into(),
                fingerprint: Some(third),
                problem: None,
                pairing_code: None,
            },
            keys::ListedDevice {
                name: "Ledger".into(),
                fingerprint: Some(stranger),
                problem: None,
                pairing_code: None,
            },
        ];
        let (form, actions) = keys::step(&keys, false, &devices, None);
        let shown = rendered_labels(form).await;
        assert_labels(
            &shown,
            &[
                "Key 1",
                "Key 3",
                "Waiting for its device",
                "Coldcard",
                "Connected, and its fingerprint matches this key.",
                "Check device",
                "Connected, not matched to a key",
                "Jade",
                "Locked. Unlock it to see which key it holds.",
                "BitBox02",
                "7NQ3B Z4WKD",
                "Ledger",
                &stranger.to_string(),
                "Not a signer of this wallet.",
            ],
            &["Connected hardware wallets", "Where is this key?"],
        );
        // The Coldcard is named once: in Key 3's row, not again below.
        assert_eq!(shown.iter().filter(|text| *text == "Coldcard").count(), 1);
        let footer = rendered_labels(actions).await;
        assert_labels(&footer, &["0 of 2 needed keys verified"], &[]);
    }

    /// With the threshold met, a key nobody answered is not needed, and the
    /// footer says it signs elsewhere.
    #[tokio::test]
    async fn a_spare_key_signs_elsewhere_once_the_threshold_is_met() {
        let wallet = two_of_three();
        let mut keys = KeysState::new(&wallet);
        keys.slots[0].status = KeyStatus::Verified;
        keys.slots[1].status = KeyStatus::Verified;
        assert!(keys.ready());
        let (form, actions) = keys::step(&keys, false, &[], None);
        let shown = rendered_labels(form).await;
        assert_labels(
            &shown,
            &[
                "Not needed to spend",
                "2 keys are verified, so this one signs elsewhere. Connect its device to add it anyway.",
            ],
            &[
                "Waiting for its device",
                "No hardware wallet connected. Plug one in and unlock it; it appears beside the key it holds.",
            ],
        );
        let footer = rendered_labels(actions).await;
        assert_labels(
            &footer,
            &["2 of 2 needed keys verified · Key 3 signs elsewhere"],
            &[],
        );
    }

    /// A device that never answers: enough for the Keys step to list it and
    /// start a check, which the tests never run.
    #[derive(Debug)]
    struct SilentDevice;

    #[async_trait::async_trait]
    impl async_hwi::HWI for SilentDevice {
        fn device_kind(&self) -> async_hwi::DeviceKind {
            async_hwi::DeviceKind::Coldcard
        }
        async fn get_version(&self) -> Result<async_hwi::Version, async_hwi::Error> {
            unreachable!("the check never runs here")
        }
        async fn get_master_fingerprint(&self) -> Result<Fingerprint, async_hwi::Error> {
            unreachable!("the check never runs here")
        }
        async fn get_extended_pubkey(
            &self,
            _: &coincube_core::miniscript::bitcoin::bip32::DerivationPath,
        ) -> Result<coincube_core::miniscript::bitcoin::bip32::Xpub, async_hwi::Error> {
            unreachable!("the check never runs here")
        }
        async fn register_wallet(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<[u8; 32]>, async_hwi::Error> {
            unreachable!("the check never runs here")
        }
        async fn is_wallet_registered(&self, _: &str, _: &str) -> Result<bool, async_hwi::Error> {
            unreachable!("the check never runs here")
        }
        async fn display_address(
            &self,
            _: &async_hwi::AddressScript,
        ) -> Result<(), async_hwi::Error> {
            unreachable!("the check never runs here")
        }
        async fn sign_tx(
            &self,
            _: &mut coincube_core::miniscript::bitcoin::Psbt,
        ) -> Result<(), async_hwi::Error> {
            unreachable!("the check never runs here")
        }
    }

    fn connected(id: &str, fingerprint: Fingerprint) -> HardwareWallet {
        HardwareWallet::Supported {
            id: id.into(),
            device: Arc::new(SilentDevice),
            kind: async_hwi::DeviceKind::Coldcard,
            fingerprint,
            version: None,
            registered: None,
            alias: None,
        }
    }

    /// The matching device starts its key's check without a click, once per
    /// connection: a failed check is not repeated until the device is
    /// reconnected, and only one check runs at a time.
    #[test]
    fn a_matching_device_is_checked_once_per_connection() {
        let directory = NetworkDirectory::new(std::env::temp_dir().join("import-auto-check"));
        let mut panel =
            ImportedBitcoinPanel::new(directory, "cube-1".into(), BitcoinImport::pending());
        let wallet =
            ImportedWalletAttachment::new(two_of_three(), ChainId::Bitcoin, 0, 200).unwrap();
        panel.keys = Some(KeysState::new(&wallet.descriptor));
        panel.review = Some(wallet);
        let first = panel.keys.as_ref().unwrap().slots[0].account.fingerprint;
        let third = panel.keys.as_ref().unwrap().slots[2].account.fingerprint;
        let mut listing = HardwareWallets::new(
            crate::dir::CoincubeDirectory::new(std::env::temp_dir().join("import-auto-check")),
            Network::Bitcoin,
        );
        listing.list = vec![connected("coldcard-1", third), connected("jade-1", first)];
        panel.hardware_wallets = Some(listing);
        let status = |panel: &ImportedBitcoinPanel, index: usize| {
            panel.keys.as_ref().unwrap().slots[index].status.clone()
        };

        let _ = panel.auto_check_devices();
        // One check at a time: the first matching key starts, the other waits.
        assert_eq!(status(&panel, 0), KeyStatus::Checking);
        assert_eq!(status(&panel, 2), KeyStatus::Missing);

        // It fails; the waiting key's check starts, the failed one stays put.
        let check = panel.keys.as_ref().unwrap().slots[0].check;
        let generation = panel.generation;
        let _ = panel.update_event(ImportEvent::DeviceVerified(
            generation,
            0,
            check,
            Err("Not this key.".into()),
        ));
        assert_eq!(status(&panel, 0), KeyStatus::Failed("Not this key.".into()));
        assert_eq!(status(&panel, 2), KeyStatus::Checking);
        let check = panel.keys.as_ref().unwrap().slots[2].check;
        let _ = panel.update_event(ImportEvent::DeviceVerified(generation, 2, check, Ok(())));
        assert_eq!(status(&panel, 2), KeyStatus::Verified);
        let _ = panel.auto_check_devices();
        assert_eq!(status(&panel, 0), KeyStatus::Failed("Not this key.".into()));

        // Unplugged and plugged back in: checked afresh.
        panel
            .hardware_wallets
            .as_mut()
            .unwrap()
            .list
            .retain(|hw| hw.id() != "jade-1");
        let _ = panel.auto_check_devices();
        panel
            .hardware_wallets
            .as_mut()
            .unwrap()
            .list
            .push(connected("jade-1", first));
        let _ = panel.auto_check_devices();
        assert_eq!(status(&panel, 0), KeyStatus::Checking);
    }

    #[tokio::test]
    async fn the_keys_step_sits_between_source_and_review() {
        let directory = NetworkDirectory::new(std::env::temp_dir().join("import-journey-keys"));
        let mut panel =
            ImportedBitcoinPanel::new(directory, "cube-1".into(), BitcoinImport::pending());
        let wallet =
            ImportedWalletAttachment::new(two_of_three(), ChainId::Bitcoin, 0, 200).unwrap();
        // What a successful Review of the Source form leaves behind.
        panel.keys = Some(KeysState::new(&wallet.descriptor));
        panel.review = Some(wallet);
        assert_eq!(panel.journey_step(), JourneyStep::Keys);
        let cache = Cache::default();
        let shown = rendered_labels(panel.journey_view(&cache, false)).await;
        assert_labels(
            &shown,
            &[
                "Keys",
                "Add the wallet's keys",
                "Waiting for its device",
                "No hardware wallet connected. Plug one in and unlock it; it appears beside the key it holds.",
                "Review wallet",
            ],
            // No device is connected, so no check is offered yet.
            &["Import and scan wallet", "Check device", "Verify key"],
        );
        // Review stays shut, and so does importing.
        let _ = panel.update_view(ImportMessage::KeysDone, None, 0);
        assert_eq!(panel.journey_step(), JourneyStep::Keys);
        let _ = panel.update_view(ImportMessage::Confirm, None, 0);
        assert_eq!(
            panel.error.as_deref(),
            Some("Add the wallet's keys before importing it.")
        );

        let _ = panel.update_view(ImportMessage::KeySourcePicked(0, KeySource::Words), None, 0);
        // Each check carries an id; only the check a slot waits for lands.
        let check_of = |panel: &ImportedBitcoinPanel, index: usize| {
            panel.keys.as_ref().unwrap().slots[index].check
        };
        let status_of = |panel: &ImportedBitcoinPanel, index: usize| {
            panel.keys.as_ref().unwrap().slots[index].status.clone()
        };
        let _ = panel.update_view(
            ImportMessage::KeyWordsEdited(0, SecretInput::new(phrase(1).to_string())),
            None,
            0,
        );
        let _ = panel.update_view(ImportMessage::VerifyWords(0), None, 0);
        let first = check_of(&panel, 0);
        assert_eq!(status_of(&panel, 0), keys::KeyStatus::Checking);
        // Edit while checking, then check the new words: the first result is
        // stale and cannot mark the second words verified.
        let _ = panel.update_view(
            ImportMessage::KeyWordsEdited(0, SecretInput::new(phrase(4).to_string())),
            None,
            0,
        );
        assert_eq!(status_of(&panel, 0), keys::KeyStatus::Missing);
        let _ = panel.update_view(ImportMessage::VerifyWords(0), None, 0);
        let second = check_of(&panel, 0);
        assert_ne!(first, second);
        let generation = panel.generation;
        let _ = panel.update_event(ImportEvent::WordsVerified(generation, 0, first, Ok(())));
        assert_eq!(status_of(&panel, 0), keys::KeyStatus::Checking);
        // A result from another generation never lands either.
        let _ = panel.update_event(ImportEvent::WordsVerified(
            generation + 1,
            0,
            second,
            Ok(()),
        ));
        assert_eq!(status_of(&panel, 0), keys::KeyStatus::Checking);
        let _ = panel.update_event(ImportEvent::WordsVerified(
            generation,
            0,
            second,
            Err("This key does not match the selected wallet account.".into()),
        ));
        assert!(matches!(status_of(&panel, 0), keys::KeyStatus::Failed(_)));
        let _ = panel.update_view(
            ImportMessage::KeyWordsEdited(0, SecretInput::new(phrase(1).to_string())),
            None,
            0,
        );
        let _ = panel.update_view(ImportMessage::VerifyWords(0), None, 0);
        let third = check_of(&panel, 0);
        let _ = panel.update_event(ImportEvent::WordsVerified(generation, 0, third, Ok(())));
        assert_eq!(status_of(&panel, 0), keys::KeyStatus::Verified);
        // A words result cannot land on a slot moved to another source.
        let _ = panel.update_view(
            ImportMessage::KeyWordsEdited(2, SecretInput::new(phrase(3).to_string())),
            None,
            0,
        );
        let _ = panel.update_view(ImportMessage::VerifyWords(2), None, 0);
        let moved = check_of(&panel, 2);
        let _ = panel.update_view(
            ImportMessage::KeySourcePicked(2, KeySource::Hardware),
            None,
            0,
        );
        let _ = panel.update_event(ImportEvent::WordsVerified(generation, 2, moved, Ok(())));
        assert_eq!(status_of(&panel, 2), keys::KeyStatus::Missing);
        // A review-bound device list goes with an edited source.
        let _ = panel.update_view(ImportMessage::KeySourcePicked(2, KeySource::Words), None, 0);

        let _ = panel.update_view(
            ImportMessage::KeySourcePicked(1, KeySource::Elsewhere),
            None,
            0,
        );
        let _ = panel.update_view(
            ImportMessage::KeySourcePicked(2, KeySource::Elsewhere),
            None,
            0,
        );
        let _ = panel.update_view(ImportMessage::KeysDone, None, 0);
        assert_eq!(
            panel.journey_step(),
            JourneyStep::Keys,
            "one key cannot sign a 2-of-3"
        );
        let _ = panel.update_view(ImportMessage::KeySourcePicked(1, KeySource::Words), None, 0);
        let _ = panel.update_view(
            ImportMessage::KeyWordsEdited(1, SecretInput::new(phrase(2).to_string())),
            None,
            0,
        );
        let _ = panel.update_view(ImportMessage::VerifyWords(1), None, 0);
        let check = check_of(&panel, 1);
        let _ = panel.update_event(ImportEvent::WordsVerified(generation, 1, check, Ok(())));
        let _ = panel.update_view(ImportMessage::KeysDone, None, 0);
        assert_eq!(panel.journey_step(), JourneyStep::Review);
        let review = rendered_labels(panel.journey_view(&cache, false)).await;
        assert!(
            review
                .iter()
                .any(|text| text.starts_with("Keys: 2 of 3 provided")),
            "{:?}",
            review
        );
        assert_labels(&review, &["Back to keys"], &[]);
        let _ = panel.update_view(ImportMessage::BackToKeys, None, 0);
        assert_eq!(panel.journey_step(), JourneyStep::Keys);
        // Editing the source starts the keys over.
        let _ = panel.update_view(ImportMessage::Edit, None, 0);
        assert!(panel.keys.is_none() && panel.review.is_none());
    }

    #[test]
    fn storing_keys_reports_its_outcome_and_clears_the_words() {
        let mut panel = ready_import_panel();
        let wallet = panel.import.active_attachment().cloned().unwrap();
        let mut keys = KeysState::new(&wallet.descriptor).starting_on(KeySource::Words);
        keys.slots[0].words = phrase(1);
        keys.slots[0].status = keys::KeyStatus::Verified;
        panel.keys = Some(keys);
        let epoch = panel.keys_gate.load(Ordering::SeqCst);
        // The store is under way when its result arrives.
        panel.keys_stored = KeysStored::Storing;
        let _ = panel.update_event(ImportEvent::KeysStored(epoch, Err("synthetic".into())));
        assert_eq!(panel.keys_stored, KeysStored::Failed("synthetic".into()));
        // The words stay for a retry.
        assert!(!panel.keys.as_ref().unwrap().slots[0].words.is_empty());
        let _ = panel.update_event(ImportEvent::KeysStored(epoch, Ok(1)));
        assert_eq!(panel.keys_stored, KeysStored::Stored(1));
        assert!(panel.keys.as_ref().unwrap().slots[0].words.is_empty());
        // A hardware or elsewhere signer has no words to store.
        let mut device = KeysState::new(&wallet.descriptor);
        device.slots[0].source = KeySource::Hardware;
        device.slots[0].status = keys::KeyStatus::Verified;
        panel.keys = Some(device);
        let _ = panel.store_keys();
        assert_eq!(panel.keys_stored, KeysStored::None);
    }

    /// A panel whose 1-of-1 key is verified recovery words, rooted in a
    /// fresh data directory so Duress Mode state can be written beside it.
    fn verified_words_panel(root: &std::path::Path) -> ImportedBitcoinPanel {
        let ready = ready_import_panel();
        let wallet = ready.import.active_attachment().cloned().unwrap();
        let directory = NetworkDirectory::new(root.join("bitcoin"));
        let mut panel =
            ImportedBitcoinPanel::new(directory, "cube-1".into(), BitcoinImport::pending());
        let mut keys = KeysState::new(&wallet.descriptor).starting_on(KeySource::Words);
        keys.slots[0].words = phrase(1);
        keys.slots[0].status = keys::KeyStatus::Verified;
        panel.keys = Some(keys);
        panel.keys_confirmed = true;
        panel.review = Some(wallet);
        panel
    }

    /// Installing cannot be undone, so words that could not be stored stop
    /// the import before it attaches the wallet.
    #[test]
    fn words_that_cannot_be_stored_stop_the_import_first() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("import-preflight-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("bitcoin")).unwrap();
        let mut panel = verified_words_panel(&root);
        crate::app::session::close();
        let _ = panel.update_view(ImportMessage::Confirm, None, 0);
        assert_eq!(
            panel.error.as_deref(),
            Some("Unlock this Cube again before storing recovery words.")
        );
        assert!(!panel.busy, "nothing was installed");

        crate::app::session::open("cube-1", zeroize::Zeroizing::new("synthetic PIN".into()));
        crate::services::duress::DuressLocalState {
            enrolled: true,
            ..Default::default()
        }
        .save(&root)
        .unwrap();
        panel.error = None;
        let _ = panel.update_view(ImportMessage::Confirm, None, 0);
        assert!(
            panel
                .error
                .as_deref()
                .is_some_and(|error| error.contains("Duress Mode")),
            "{:?}",
            panel.error
        );
        assert!(!panel.busy);

        // A device-only import stores nothing, so the preflight passes.
        panel.keys.as_mut().unwrap().slots[0].source = KeySource::Hardware;
        assert_eq!(panel.storage_preflight(), Ok(()));
        crate::app::session::close();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Recovery words can be added to an attached wallet that has none, from
    /// Vault settings or Done; Storing survives a refresh or a wider search.
    #[test]
    fn recovery_words_can_be_added_after_the_wallet_attaches() {
        let _guard = crate::app::session::test_guard();
        let mut panel = ready_import_panel();
        let epoch = panel.keys_gate.load(Ordering::SeqCst);
        // Unknown inventory: nothing offered.
        let _ = panel.update_view(ImportMessage::AddKeys, None, 0);
        assert!(!panel.adding_keys);
        // An older read never lands.
        let _ = panel.update_event(ImportEvent::KeysInventory(epoch + 7, Ok(0)));
        assert_eq!(panel.stored_keys, None);
        let _ = panel.update_event(ImportEvent::KeysInventory(epoch, Ok(0)));
        assert_eq!(panel.stored_keys, Some(0));

        let _ = panel.update_view(ImportMessage::AddKeys, None, 0);
        assert!(panel.adding_keys && panel.journey_open());
        assert_eq!(panel.journey_step(), JourneyStep::Keys);
        // Nothing verified yet: Store words stays shut.
        let _ = panel.update_view(ImportMessage::KeysDone, None, 0);
        assert_eq!(panel.journey_step(), JourneyStep::Keys);
        let _ = panel.update_view(
            ImportMessage::KeyWordsEdited(0, SecretInput::new(phrase(1).to_string())),
            None,
            0,
        );
        let _ = panel.update_view(ImportMessage::VerifyWords(0), None, 0);
        let check = panel.keys.as_ref().unwrap().slots[0].check;
        let generation = panel.generation;
        let _ = panel.update_event(ImportEvent::WordsVerified(generation, 0, check, Ok(())));
        // Locked: refused before storing.
        crate::app::session::close();
        let _ = panel.update_view(ImportMessage::KeysDone, None, 0);
        assert_eq!(panel.keys_stored, KeysStored::None);
        assert_eq!(panel.journey_step(), JourneyStep::Keys);
        crate::app::session::open("cube-1", zeroize::Zeroizing::new("synthetic PIN".into()));
        let _ = panel.update_view(ImportMessage::KeysDone, None, 0);
        assert_eq!(panel.keys_stored, KeysStored::Storing);
        assert_eq!(panel.journey_step(), JourneyStep::Done);
        // Close waits for the store.
        let _ = panel.update_view(ImportMessage::LeaveJourney, None, 0);
        assert!(panel.journey_open());

        // A wider history search bumps the panel generation, not the key
        // storage epoch, so the result still lands.
        panel.generation = fresh_generation();
        let _ = panel.update_event(ImportEvent::KeysStored(epoch, Ok(1)));
        assert_eq!(panel.keys_stored, KeysStored::Stored(1));
        assert_eq!(panel.stored_keys, Some(1));
        assert!(panel.keys.as_ref().unwrap().slots[0].words.is_empty());
        // A second store is not offered once one exists.
        let _ = panel.update_view(ImportMessage::RetryStoreKeys, None, 0);
        assert_eq!(panel.keys_stored, KeysStored::Stored(1));
        let _ = panel.update_view(ImportMessage::LeaveJourney, None, 0);
        assert!(!panel.journey_open() && !panel.adding_keys && panel.keys.is_none());
        let _ = panel.update_view(ImportMessage::AddKeys, None, 0);
        assert!(!panel.adding_keys, "words are already stored");

        // Closing the Cube ends the epoch: a late result is dropped.
        panel.stop();
        let _ = panel.update_event(ImportEvent::KeysStored(epoch, Ok(2)));
        assert_eq!(panel.stored_keys, Some(1));
        crate::app::session::close();
    }

    #[test]
    fn leaving_or_abandoning_forgets_typed_words() {
        let root = std::env::temp_dir().join(format!("import-forget-{}", uuid::Uuid::new_v4()));
        let mut panel = verified_words_panel(&root);
        let _ = panel.update_view(ImportMessage::LeaveJourney, None, 0);
        let slot = &panel.keys.as_ref().unwrap().slots[0];
        assert!(slot.words.is_empty() && slot.status == keys::KeyStatus::Missing);
        assert!(!panel.keys_confirmed, "the words must be typed again");

        let mut panel = verified_words_panel(&root);
        let generation = panel.generation;
        let _ = panel.update_event(ImportEvent::Abandoned(generation, Ok(())));
        assert!(panel.keys.is_none());
    }

    /// Leaving while a device check runs must not leave the slot Checking:
    /// its result can no longer land, and Checking disables every control.
    #[test]
    fn leaving_during_a_device_check_frees_the_keys_step() {
        let root =
            std::env::temp_dir().join(format!("import-leave-check-{}", uuid::Uuid::new_v4()));
        let mut panel = verified_words_panel(&root);
        panel.keys_confirmed = false;
        {
            let slot = &mut panel.keys.as_mut().unwrap().slots[0];
            slot.source = KeySource::Hardware;
            slot.status = keys::KeyStatus::Checking;
            slot.check = keys::next_check();
        }
        let _ = panel.update_view(ImportMessage::LeaveJourney, None, 0);
        let _ = panel.update_view(ImportMessage::ResumeJourney, None, 0);
        let keys = panel.keys.as_ref().unwrap();
        assert!(!keys.checking(), "the Keys step must be usable again");
        assert_eq!(keys.slots[0].status, keys::KeyStatus::Missing);
    }

    /// A failed store asks for the inventory, so Add recovery words is
    /// offered in the same session once the typed words are gone.
    #[test]
    fn a_failed_store_reads_the_inventory_for_add_recovery_words() {
        let mut panel = ready_import_panel();
        let wallet = panel.import.active_attachment().cloned().unwrap();
        let mut keys = KeysState::new(&wallet.descriptor);
        keys.slots[0].words = phrase(1);
        keys.slots[0].status = keys::KeyStatus::Verified;
        panel.keys = Some(keys);
        panel.journey = true;
        panel.keys_stored = KeysStored::Storing;
        let epoch = panel.keys_gate.load(Ordering::SeqCst);
        let task = panel.update_event(ImportEvent::KeysStored(epoch, Err("synthetic".into())));
        assert!(task.units() > 0, "the inventory read is scheduled");
        let _ = panel.update_view(ImportMessage::LeaveJourney, None, 0);
        let _ = panel.update_event(ImportEvent::KeysInventory(epoch, Ok(0)));
        let _ = panel.update_view(ImportMessage::AddKeys, None, 0);
        assert!(panel.adding_keys && panel.journey_open());
    }

    #[test]
    fn editing_the_source_drops_its_device_list() {
        let root = std::env::temp_dir().join(format!("import-hw-{}", uuid::Uuid::new_v4()));
        let mut panel = verified_words_panel(&root);
        panel.set_hardware_root(CoincubeDirectory::new(root.clone()));
        panel.keys_confirmed = false;
        let _ = panel.update_view(
            ImportMessage::KeySourcePicked(0, KeySource::Hardware),
            None,
            0,
        );
        assert!(
            panel.hardware_wallets.is_some(),
            "listing bound to the reviewed policy"
        );
        let _ = panel.update_view(ImportMessage::Edit, None, 0);
        assert!(panel.hardware_wallets.is_none());
    }

    #[test]
    fn a_plain_multi_threshold_is_read_too() {
        let keys: Vec<String> = (1..=3).map(|entropy| phrase_account(entropy).0).collect();
        let branch = |n: u8| {
            format!(
                "wsh(multi(2,{}/{n}/*,{}/{n}/*,{}/{n}/*))",
                keys[0], keys[1], keys[2]
            )
        };
        let wallet =
            ImportedWalletDescriptor::parse(&branch(0), Some(&branch(1)), ChainId::Bitcoin)
                .unwrap();
        let state = KeysState::new(&wallet);
        assert_eq!((state.slots.len(), state.threshold), (3, 2));
    }

    #[test]
    fn typed_words_never_reach_debug_output() {
        let secret =
            "abandon ability able about above absent absorb abstract absurd abuse access accident";
        let message = ImportMessage::KeyWordsEdited(0, SecretInput::new(secret.into()));
        let debug = format!("{:?}", message);
        assert!(!debug.contains("abandon"), "{}", debug);
        let passphrase = ImportMessage::KeyPassphraseEdited(0, SecretInput::new("hunter2".into()));
        assert!(!format!("{:?}", passphrase).contains("hunter2"));
    }

    #[test]
    fn pending_import_uses_vault_routes_but_never_inheritance_actions() {
        let directory = NetworkDirectory::new(std::env::temp_dir().join("import-panel-route-test"));
        let panel = ImportedBitcoinPanel::new(directory, "cube-1".into(), BitcoinImport::pending());
        assert!(panel.handles_route(&Menu::Vault(VaultSubMenu::Overview)));
        assert!(panel.handles_route(&Menu::Vault(VaultSubMenu::Receive)));
        assert!(panel.handles_route(&Menu::Vault(VaultSubMenu::Send)));
        assert!(!panel.handles_route(&Menu::Vault(VaultSubMenu::Recovery)));
        assert!(
            !panel.handles_route(&Menu::Vault(VaultSubMenu::Settings(Some(
                SettingsOption::LocalSigning,
            ))))
        );
    }

    #[test]
    fn selecting_missing_saved_transaction_clears_previous_draft() {
        let directory = NetworkDirectory::new(std::env::temp_dir().join("import-panel-draft-test"));
        let mut panel =
            ImportedBitcoinPanel::new(directory, "cube-1".into(), BitcoinImport::pending());
        let psbt = Psbt::from_unsigned_tx(Transaction {
            version: transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: Vec::new(),
            output: Vec::new(),
        })
        .unwrap();
        let id = psbt.unsigned_tx.compute_txid();
        panel.draft = Some(ImportedDraft {
            psbt,
            signatures_per_input: Vec::new(),
            final_txid: Some(id),
            warnings: Vec::new(),
        });
        panel.select_saved_draft(id);
        assert!(panel.draft.is_none());
        assert!(panel
            .error
            .as_deref()
            .unwrap()
            .contains("no longer available"));
    }

    #[test]
    fn late_signer_results_cannot_replace_a_different_selected_psbt() {
        let directory =
            NetworkDirectory::new(std::env::temp_dir().join("import-panel-signing-test"));
        let mut panel =
            ImportedBitcoinPanel::new(directory, "cube-1".into(), BitcoinImport::pending());
        let make_psbt = |version| {
            Psbt::from_unsigned_tx(Transaction {
                version,
                lock_time: LockTime::ZERO,
                input: Vec::new(),
                output: Vec::new(),
            })
            .unwrap()
        };
        let prior = make_psbt(transaction::Version::ONE);
        let selected = make_psbt(transaction::Version::TWO);
        let prior_id = prior.unsigned_tx.compute_txid();
        let selected_id = selected.unsigned_tx.compute_txid();
        panel.draft = Some(ImportedDraft {
            psbt: selected,
            signatures_per_input: Vec::new(),
            final_txid: None,
            warnings: Vec::new(),
        });
        let generation = panel.generation;
        let _ = panel.update_event(ImportEvent::SignedSaved(
            generation,
            prior_id,
            Ok((prior.clone(), vec![], None)),
        ));
        assert_eq!(
            panel
                .draft
                .as_ref()
                .unwrap()
                .psbt
                .unsigned_tx
                .compute_txid(),
            selected_id
        );
        let _ = panel.update_event(ImportEvent::HardwareSigned(
            generation,
            prior_id,
            Ok((prior.clone(), vec![], None)),
        ));
        assert_eq!(
            panel
                .draft
                .as_ref()
                .unwrap()
                .psbt
                .unsigned_tx
                .compute_txid(),
            selected_id
        );
        let _ = panel.update_event(ImportEvent::FileLoaded(
            generation,
            prior_id,
            Ok(Some((prior, vec![], None))),
        ));
        assert_eq!(
            panel
                .draft
                .as_ref()
                .unwrap()
                .psbt
                .unsigned_tx
                .compute_txid(),
            selected_id
        );
    }
}
