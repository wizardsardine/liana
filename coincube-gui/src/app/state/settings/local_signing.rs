//! Settings panel state for the local LAN signer ("Paired phones").

use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
use coincube_ui::widget::Element;
use iced::Task;

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::State;
use crate::app::view;
// LocalSigningMessage is re-exported by `view::message` via the
// glob in `view/mod.rs`; reach it through the public re-export.
use crate::app::view::LocalSigningMessage;
use crate::app::wallet::Wallet;
use crate::daemon::Daemon;
use iced::widget::qr_code;

use crate::phone_signer::errors::PairingError;
use crate::phone_signer::pairing::{GeneratedOffer, PairingOffer};
use crate::phone_signer::pairing_store::{PairedPhone, PairingStoreFile};

/// How often the panel syncs pairing removals with its phones while open.
const SYNC_INTERVAL: Duration = Duration::from_secs(3);

/// What the pairing wizard is currently doing.
pub enum PairingFlow {
    /// No pairing in flight; render the paired-phones table.
    Idle,
    /// User clicked "Pair phone" and we're showing the list of
    /// discovered phones on the LAN so they can pick one. mDNS
    /// browse runs every tick to keep the list fresh.
    PhonePicker {
        discovered: Vec<crate::phone_signer::mdns::DiscoveredPhone>,
    },
    /// User picked a phone and we've generated an offer aimed at it.
    /// The view renders the QR + countdown until the offer expires
    /// or `run_pairing` returns.
    Waiting {
        phone: crate::phone_signer::mdns::DiscoveredPhone,
        offer: PairingOffer,
        /// Pre-rendered QR pixel grid. Held on the state so the view
        /// can hand a reference to iced's `QRCode` widget without
        /// reconstructing each frame and without lifetime-laundering
        /// a stack-local.
        qr: Option<qr_code::Data>,
    },
    /// `run_pairing` returned an error before completion. Typed so
    /// the view can render category-specific copy and decide whether
    /// to show a Try-Again button.
    Error(PairingError),
}

/// In-progress edits for one paired-phone row. Mirrors the row's
/// persisted name and fallback-addr until [`LocalSigningMessage::SaveRow`]
/// flushes them back to the store.
#[derive(Default, Clone)]
pub struct RowDraft {
    pub name: String,
    pub fallback: String,
}

/// One vault key the Pair panel offers, as the user should see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairableKey {
    /// Account xpub, the identity the pairing offer carries.
    pub xpub: String,
    /// The key exactly as written in the descriptor, origin included.
    pub descriptor_key: String,
    /// Origin (master) fingerprint, when the descriptor records one.
    pub fingerprint: Option<Fingerprint>,
    /// The name the user gave this key, if any.
    pub name: Option<String>,
    /// Whether this key is known to come from the COINCUBE Keychain.
    pub keychain: bool,
}

pub struct LocalSigningState {
    /// Keys a phone can be paired for. Only the Vault's Keychain keys when
    /// [`Self::keychain_keys_recorded`], every spendable key otherwise.
    pub vault_keys: Vec<PairableKey>,
    /// Whether the loaded Vault recorded which of its keys came from the
    /// Keychain. When set and [`Self::vault_keys`] is empty, the Vault has
    /// no Keychain key and there is nothing a phone could pair for.
    pub keychain_keys_recorded: bool,
    /// Whether each key's full descriptor string is shown under its name.
    pub show_key_details: bool,
    /// The loaded Vault, kept so a Keychain lookup's answer is applied to the
    /// latest version of it rather than to the one the lookup started from.
    wallet: Option<Arc<Wallet>>,
    /// Descriptor checksum of the Vault a Keychain lookup was started for
    /// in this panel, so a Vault is asked about at most once per visit.
    keychain_lookup_started: Option<String>,
    /// Origin (master) fingerprint of each entry in [`Self::vault_keys`],
    /// keyed by the same xpub string.
    ///
    /// A signer's origin fingerprint is *not* derivable from its account
    /// xpub — it belongs to the master key several levels up — so binding
    /// the two halves of a phone's reported identity needs this lookup
    /// from the local descriptor. See the `expected_signer_fp` argument of
    /// [`crate::phone_signer::pairing_listener::run_pairing`].
    pub vault_key_fingerprints: Vec<(String, Fingerprint)>,
    pub selected_key: Option<String>,
    pub descriptor_sha256: String,
    pub phones: PairingStoreFile,
    pub flow: PairingFlow,
    /// Vault id (`Wallet::id_fingerprint`) of the loaded wallet,
    /// captured during `reload`. Threaded into the pairing offer's
    /// `wallet_fingerprint` claim so the phone displays "pair with
    /// vault X" and the listener can reject an offer that was
    /// generated for a different vault.
    pub wallet_fingerprint: Option<Fingerprint>,
    /// Locally derived fingerprint set used only for pairing consistency.
    /// The selected xpub and phone-reported backend ID determine identity.
    pub wallet_signer_fingerprints: Vec<Fingerprint>,
    /// Per-row drafts keyed by the phone's 8-hex cert pin
    /// fingerprint. Seeded from the persisted row on load and
    /// after pairing completes; mutations are kept in memory until
    /// the user clicks Save.
    pub row_drafts: HashMap<String, RowDraft>,
    /// Monotonic id stamped onto every spawned pairing run. The
    /// `Task::perform` for the listener captures the id at spawn
    /// time and includes it in the `PairingCompleted` message;
    /// completions whose id doesn't match the current value are
    /// ignored. Bumped on `PickPhone` (start a new run) and
    /// `CancelPairing` (invalidate any in-flight run).
    pub pairing_id: u64,
    pub pairing_run: crate::phone_signer::pairing_run::PairingRun,
    /// Cert pins the user has explicitly removed during this app
    /// session. The spawned `Task::perform` for a pairing handshake
    /// captures a clone of this `Arc` and checks it before persisting
    /// the result — without the check, an in-flight pairing whose
    /// handshake completes AFTER the user clicks "Remove" on the same
    /// row would call `upsert_preserving_user_fields` and silently
    /// re-add the row the user just deleted (the existing `pairing_id`
    /// guard only gates the UI update, not the disk write).
    ///
    /// `Arc<Mutex<_>>` rather than a plain field so the spawned task
    /// can read it after the state's owning panel has been dropped on
    /// settings navigation. The set is purely in-memory and not
    /// persisted: tombstones only need to outlive any in-flight
    /// `run_pairing` future (at most the pairing-offer TTL, ~120s),
    /// not across app restarts.
    pub tombstones: Arc<Mutex<HashSet<[u8; 32]>>>,
    /// `reload` doesn't get a `Cache` (it only sees daemon + wallet),
    /// so we defer the first store load to the first `update` tick
    /// where `cache` is available.
    initialised: bool,
    wallet_chain: Option<crate::chain::ChainId>,
    /// A [`LocalSigningMessage::SyncTick`] pass is running; ticks are
    /// skipped until it reports back.
    sync_in_flight: bool,
    /// Says which phones removed this desktop since the panel opened, so a
    /// row that vanishes on its own is explained.
    pub unpaired_notice: Option<String>,
    /// Fingerprint of the phone whose local pairing is awaiting review.
    pub pending_remove_phone: Option<String>,
    /// Full identity approved by that review; the short row id alone can
    /// identify a different phone if the store changes before confirmation.
    pending_remove_cert_pin: Option<[u8; 32]>,
    /// Full Vault descriptor approved by the review. The four-byte Vault id
    /// can collide across distinct descriptors.
    pending_remove_descriptor: Option<[u8; 32]>,
}

impl Default for LocalSigningState {
    fn default() -> Self {
        Self {
            vault_keys: Vec::new(),
            keychain_keys_recorded: false,
            show_key_details: false,
            wallet: None,
            keychain_lookup_started: None,
            vault_key_fingerprints: Vec::new(),
            selected_key: None,
            descriptor_sha256: String::new(),
            phones: PairingStoreFile::default(),
            flow: PairingFlow::Idle,
            wallet_fingerprint: None,
            wallet_signer_fingerprints: Vec::new(),
            row_drafts: HashMap::new(),
            pairing_id: 0,
            pairing_run: Default::default(),
            tombstones: Arc::new(Mutex::new(HashSet::new())),
            initialised: false,
            wallet_chain: None,
            sync_in_flight: false,
            unpaired_notice: None,
            pending_remove_phone: None,
            pending_remove_cert_pin: None,
            pending_remove_descriptor: None,
        }
    }
}

impl LocalSigningState {
    fn refresh_phones(&mut self, cache: &Cache) {
        self.refresh_phones_from(&cache.datadir_path);
    }

    /// Datadir-only refresh path. Lets tests drive the store without
    /// constructing a full [`Cache`].
    pub(crate) fn refresh_phones_from(&mut self, dir: &crate::dir::CoincubeDirectory) {
        if let Ok(file) = crate::phone_signer::pairing_store::load(dir) {
            self.phones = file;
        }
        self.seed_row_drafts();
    }

    pub(crate) fn seed_row_drafts(&mut self) {
        for p in &self.phones.phones {
            let fp8 = crate::phone_signer::identity::pin_hex8(&p.cert_pin);
            self.row_drafts.entry(fp8).or_insert_with(|| RowDraft {
                name: p.name.clone(),
                fallback: p.fallback_addr.clone().unwrap_or_default(),
            });
        }
    }

    /// Bump `pairing_id` and return the new value. The caller stamps
    /// the returned id onto the spawned listener task so its
    /// completion can be distinguished from any prior in-flight run.
    pub(crate) fn start_pairing_run(&mut self) -> u64 {
        self.pairing_run.cancel();
        self.pairing_run = Default::default();
        self.pairing_id = self.pairing_id.wrapping_add(1);
        self.pairing_id
    }

    /// Capture the vault id and signer-fingerprint set for the
    /// supplied wallet. Used both from [`Self::reload`] (initial
    /// entry into the panel) and from the [`Message::WalletUpdated`]
    /// path in [`Self::update`] — without the latter, switching
    /// wallets while the user is sitting on this panel leaves these
    /// two fields pointing at the previous vault, and the next
    /// pairing offer would target the wrong vault id.
    pub(crate) fn apply_wallet(&mut self, wallet: &Wallet) {
        self.wallet_chain = Some(wallet.chain);
        let Some(descriptor) = wallet.inheritance_descriptor() else {
            // Imported public accounts have no Cube Keychain pairing policy.
            self.selected_key = None;
            self.descriptor_sha256.clear();
            self.vault_key_fingerprints.clear();
            self.vault_keys.clear();
            self.wallet_fingerprint = None;
            self.wallet_signer_fingerprints.clear();
            self.keychain_keys_recorded = false;
            return;
        };
        use coincube_core::miniscript::DescriptorPublicKey;
        use sha2::{Digest, Sha256};
        let hash = hex::encode(Sha256::digest(descriptor.to_string().as_bytes()));
        if hash != self.descriptor_sha256 {
            self.selected_key = None;
        }
        self.descriptor_sha256 = hash;
        let spendable: Vec<_> = descriptor
            .spendable_keys()
            .into_iter()
            .filter_map(|key| {
                let label = key.to_string();
                match key {
                    DescriptorPublicKey::XPub(k) => Some((k.xkey.to_string(), label, k.origin)),
                    _ => None,
                }
            })
            .collect();
        // Record each key's origin fingerprint alongside its xpub. This is
        // the same fingerprint `descriptor_keys()` collects below (both read
        // `DescriptorXKey::origin`), so a pairing bound against this map can
        // never disagree with the membership set. Keys without an origin are
        // skipped rather than guessed: `thresh_origins` requires one, so a
        // descriptor missing it would not have produced a usable vault.
        self.vault_key_fingerprints = spendable
            .iter()
            .filter_map(|(xpub, _, origin)| origin.as_ref().map(|(fp, _)| (xpub.clone(), *fp)))
            .collect();
        // A phone can only pair for a key it holds, and only Keychain keys
        // are held by a phone. Where the Vault never recorded which keys
        // those are, offer them all rather than guess.
        self.keychain_keys_recorded = wallet.keychain_keys_recorded;
        self.vault_keys = spendable
            .into_iter()
            .map(|(xpub, descriptor_key, origin)| {
                let fingerprint = origin.map(|(fp, _)| fp);
                PairableKey {
                    xpub,
                    descriptor_key,
                    fingerprint,
                    name: fingerprint
                        .and_then(|fp| wallet.keys_aliases.get(&fp))
                        .filter(|name| !name.trim().is_empty())
                        .cloned(),
                    keychain: fingerprint
                        .is_some_and(|fp| wallet.keychain_key_ids.contains_key(&fp)),
                }
            })
            .filter(|key| key.keychain || !wallet.keychain_keys_recorded)
            .collect();
        // The same key can sit on several paths; offer it once.
        let mut seen = HashSet::new();
        self.vault_keys.retain(|key| seen.insert(key.xpub.clone()));
        if !self
            .selected_key
            .as_ref()
            .is_some_and(|selected| self.vault_keys.iter().any(|k| k.xpub == *selected))
        {
            // With a single Keychain key there is nothing to choose. An
            // unrecorded Vault's one key may not be a Keychain key, so the
            // user still picks it.
            self.selected_key = match self.vault_keys.as_slice() {
                [only] if self.keychain_keys_recorded => Some(only.xpub.clone()),
                _ => None,
            };
        }
        // Identify the **vault as a whole**, not one of its signers
        // — `id_fingerprint` is a stable 4-byte digest of the
        // descriptor, unique per vault and distinct from any signer
        // key.
        self.wallet_fingerprint = Some(wallet.id_fingerprint());
        // Capture the descriptor's signer fingerprints, sorted for
        // determinism (`descriptor_keys()` returns a HashSet).
        let mut v: Vec<_> = wallet.descriptor_keys().into_iter().collect();
        v.sort();
        self.wallet_signer_fingerprints = v;
    }

    /// Ask Connect which of an unrecorded Vault's keys are Keychain keys.
    ///
    /// Only when signed in, and at most once per Vault per visit; a Vault
    /// that already records its Keychain keys is never asked about. See
    /// [`super::keychain_backfill`].
    fn keychain_lookup(
        &mut self,
        daemon: Option<&Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
    ) -> Task<Message> {
        let Some(wallet) = self.wallet.clone() else {
            return Task::none();
        };
        if wallet.keychain_keys_recorded
            || self.keychain_lookup_started.as_ref() == Some(&wallet.descriptor_checksum)
        {
            return Task::none();
        }
        let Some(tokens) = cache.connect_tokens.clone() else {
            return Task::none();
        };
        if cache.cube_id.is_empty() {
            return Task::none();
        }
        let checksum = wallet.descriptor_checksum.clone();
        self.keychain_lookup_started = Some(checksum.clone());
        let persist = daemon.is_some();
        Task::perform(
            super::keychain_backfill::lookup(
                cache.datadir_path.clone(),
                wallet,
                tokens,
                cache.cube_id.clone(),
                persist,
            ),
            move |res| {
                Message::View(view::Message::Settings(
                    view::SettingsMessage::LocalSigning(LocalSigningMessage::KeychainKeysFound(
                        checksum.clone(),
                        res,
                    )),
                ))
            },
        )
    }

    /// Handle a `WalletUpdated` arriving while this panel is active.
    /// Re-derives the vault id + signer fps via [`Self::apply_wallet`]
    /// and, when the vault **actually changes**, resets any in-progress
    /// pairing wizard back to `Idle`.
    ///
    /// Why reset: every non-`Idle` flow is scoped to the previous
    /// vault, so leaving it up after a switch shows stale UI for a
    /// vault the panel has navigated away from:
    /// - `Waiting`: the offer (and the in-flight `run_pairing` task
    ///   spawned in [`LocalSigningMessage::PickPhone`]) carries the old
    ///   vault's `wallet_fingerprint`, `expected_vault_id`, and
    ///   `signer_fps`. The QR on screen is for the old vault, and a
    ///   phone that scans it would be persisted against it.
    /// - `Error`: the failure copy / Try-Again action reflect the old
    ///   vault (e.g. a wallet-fingerprint-mismatch message naming the
    ///   previous vault's keys).
    /// - `PhonePicker`: harmless on its own (the list is
    ///   vault-independent), but reset too so the wizard is a clean
    ///   slate after the switch rather than a half-started flow.
    ///
    /// Revoke the asynchronous run as well as its UI completion. A durable
    /// decision that already won the cancellation lock completes its protocol.
    ///
    /// Returns `true` if the wizard was reset.
    pub(crate) fn apply_wallet_update(&mut self, wallet: &Wallet) -> bool {
        let previous_vault = self.wallet_fingerprint;
        let previous_chain = self.wallet_chain;
        let previous_descriptor = self.descriptor_sha256.clone();
        self.apply_wallet(wallet);
        let vault_changed = previous_vault != self.wallet_fingerprint
            || previous_chain != self.wallet_chain
            || previous_descriptor != self.descriptor_sha256;
        if vault_changed {
            // A removal review belongs to the vault that displayed the row.
            // WalletUpdated can switch this active panel without `reload`.
            self.pending_remove_phone = None;
            self.pending_remove_cert_pin = None;
            self.pending_remove_descriptor = None;
        }
        if vault_changed && !matches!(self.flow, PairingFlow::Idle) {
            self.start_pairing_run();
            self.flow = PairingFlow::Idle;
            true
        } else {
            false
        }
    }

    /// React to the result of a pairing run. Persistence already
    /// happened inside the spawned task (see [`PickPhone`]'s
    /// `Task::perform`), so this handler is UI-only: it decides what
    /// the pairing card shows next.
    ///
    /// The flow transition is gated on `id == self.pairing_id` **and**
    /// `Waiting`, so a completion from a cancelled or superseded run
    /// doesn't stomp the card the user is now looking at. Returns
    /// `true` when the flow was transitioned, `false` when the
    /// completion was gated out.
    ///
    /// A successful `Ok` always refreshes the paired-phones table from
    /// disk regardless of the gate: the row was just written by the
    /// task, and even a gated-out (e.g. cancelled-then-completed) run
    /// should surface its phone in the table behind the card rather
    /// than vanish until the next panel reload.
    ///
    /// `[PickPhone]: LocalSigningMessage::PickPhone`
    pub(crate) fn apply_pairing_completed(
        &mut self,
        id: u64,
        res: Result<PairedPhone, PairingError>,
        dir: &crate::dir::CoincubeDirectory,
    ) -> bool {
        let gated = id == self.pairing_id && matches!(self.flow, PairingFlow::Waiting { .. });
        match res {
            Ok(_) => {
                // The task already persisted the row; reflect disk.
                self.refresh_phones_from(dir);
                if gated {
                    // Transition back to Idle on success. The success
                    // signal is the new row appearing in the "Paired
                    // phones" list below the pairing card — a separate
                    // "Paired with X successfully" block would just
                    // duplicate that.
                    self.flow = PairingFlow::Idle;
                }
            }
            Err(e) => {
                if gated {
                    self.flow = PairingFlow::Error(e);
                }
            }
        }
        gated
    }

    /// One-second tick from the panel's subscription. While the
    /// phone-picker is open we refresh the mDNS browse so phones
    /// appear/disappear in roughly real time. While a pairing offer
    /// is on-screen we also promote the offer-expired transition
    /// here, rather than waiting for the background `run_pairing`
    /// task to notice — its `try_pair_once` can hold inside a single
    /// `recv` for up to the full offer TTL, so without this Tick
    /// check the view would show "Pairing offer expired" while the
    /// state was still `Waiting` and the background task was still
    /// running in the background. Bumping `pairing_id` also gates
    /// out the eventual `PairingCompleted` from the dispatcher so a
    /// late-arriving Err doesn't stomp the just-set `Error` state.
    pub(crate) fn apply_tick(&mut self) {
        let mut waiting_expired = false;
        match &mut self.flow {
            PairingFlow::PhonePicker { discovered } => {
                *discovered = crate::phone_signer::mdns::browse();
            }
            PairingFlow::Waiting { offer, .. }
                if crate::phone_signer::pairing::is_expired(offer) =>
            {
                waiting_expired = true;
            }
            _ => {}
        }
        if waiting_expired {
            self.start_pairing_run();
            self.flow = PairingFlow::Error(PairingError::OfferExpired);
        }
    }

    /// Apply a `DraftName` mutation. Doesn't touch disk.
    pub(crate) fn apply_draft_name(&mut self, fp8: String, text: String) {
        self.row_drafts.entry(fp8).or_default().name = text;
    }

    /// Apply a `DraftFallback` mutation. Doesn't touch disk.
    pub(crate) fn apply_draft_fallback(&mut self, fp8: String, text: String) {
        self.row_drafts.entry(fp8).or_default().fallback = text;
    }

    /// Persist the draft for `fp8` into the matching `PairedPhone`
    /// and write the store back to disk. Empty name keeps the prior
    /// name; empty fallback clears it.
    pub(crate) fn apply_save_row(&mut self, dir: &crate::dir::CoincubeDirectory, fp8: &str) {
        let Some(draft) = self.row_drafts.get(fp8).cloned() else {
            return;
        };
        // Scoped so the `&mut self.phones.phones` borrow is released
        // before we touch `self.row_drafts` below.
        let save_outcome: Option<std::io::Result<()>> = {
            if let Some(p) = self
                .phones
                .phones
                .iter_mut()
                .find(|p| crate::phone_signer::identity::pin_hex8(&p.cert_pin) == fp8)
            {
                if !draft.name.trim().is_empty() {
                    p.name = draft.name.trim().to_string();
                }
                let f = draft.fallback.trim();
                p.fallback_addr = if f.is_empty() {
                    None
                } else {
                    Some(f.to_string())
                };
                Some(crate::phone_signer::pairing_store::update_user_fields(
                    dir,
                    &p.cert_pin,
                    p.name.clone(),
                    p.fallback_addr.clone(),
                ))
            } else {
                None
            }
        };

        match save_outcome {
            None => {
                // No matching phone in `self.phones.phones`; nothing
                // was written, nothing to revert or re-sync.
            }
            Some(Err(e)) => {
                // Persist failed — drop the in-memory edit so the
                // table reflects what's actually on disk. The
                // user's typed values stay in `row_drafts` (seed
                // uses `or_insert_with`) so they can retry.
                tracing::warn!(
                    "pairing_store::save failed for {}: {}; reverting in-memory edit",
                    fp8,
                    e
                );
                self.refresh_phones_from(dir);
            }
            Some(Ok(())) => {
                // Drop the stale draft so the row's text inputs stop
                // showing the user's pre-trim whitespace. The view
                // prefers the draft entry whenever one is present,
                // so without this the inputs would keep rendering
                // `"  Pixel 7  "` (or whatever the user typed) even
                // though disk now has `"Pixel 7"`. The subsequent
                // `refresh_phones_from` re-seeds the draft from the
                // just-persisted (trimmed) values via
                // `seed_row_drafts`'s `or_insert_with`.
                self.row_drafts.remove(fp8);
                self.refresh_phones_from(dir);
            }
        }
    }

    fn current_descriptor_digest(&self) -> Option<[u8; 32]> {
        hex::decode(&self.descriptor_sha256).ok()?.try_into().ok()
    }

    /// Resolve the short row identifier only within the active Vault and its
    /// full descriptor. Refuse legacy rows and ambiguous prefixes.
    fn unique_current_vault_phone(&self, fp8: &str) -> Option<[u8; 32]> {
        let vault = self.wallet_fingerprint?;
        let descriptor = self.current_descriptor_digest()?;
        let mut matches = self.phones.phones.iter().filter(|phone| {
            phone.vault_fingerprint == vault
                && crate::phone_signer::identity::pin_hex8(&phone.cert_pin) == fp8
        });
        let phone = matches.next()?;
        if matches.next().is_some() {
            tracing::warn!("ambiguous paired-phone fingerprint {} in active Vault", fp8);
            return None;
        }
        phone
            .signer_binding
            .as_ref()
            .filter(|binding| binding.descriptor_sha256.as_slice() == descriptor)
            .map(|_| phone.cert_pin)
    }

    /// Remove a uniquely identified phone from the active Vault. No-op if the
    /// short fingerprint is absent or ambiguous.
    pub(crate) fn apply_remove_phone(
        &mut self,
        dir: &crate::dir::CoincubeDirectory,
        fp8: &str,
    ) -> bool {
        if let (Some(pk), Some(vault), Some(descriptor)) = (
            self.unique_current_vault_phone(fp8),
            self.wallet_fingerprint,
            self.current_descriptor_digest(),
        ) {
            // Tombstone BEFORE the store delete so an in-flight
            // `Task::perform` whose `run_pairing` resolves between
            // here and the disk write sees the tombstone and skips
            // its own `upsert_preserving_user_fields`. Without this,
            // a pairing handshake that completes immediately after
            // the user clicks Remove would re-add the row the user
            // just deleted — the existing `pairing_id` mechanism
            // only gates the UI; it doesn't reach the spawned task's
            // persistence step.
            self.pairing_run.cancel();
            let inserted = match self.tombstones.lock() {
                Ok(mut g) => g.insert(pk),
                Err(_) => return false,
            };
            // Gone here at once; the phone is told by the next sync pass
            // that can reach it.
            match crate::phone_signer::pairing_store::unpair_if_vault(dir, &pk, vault, &descriptor)
            {
                Ok(true) => {}
                outcome => {
                    if inserted {
                        self.tombstones.lock().unwrap().remove(&pk);
                    }
                    if let Err(error) = outcome {
                        tracing::warn!(%error, "paired-phone removal failed");
                    }
                    self.refresh_phones_from(dir);
                    return false;
                }
            }
            self.row_drafts.remove(fp8);
            self.unpaired_notice = None;
            self.refresh_phones_from(dir);
            return true;
        }
        false
    }

    /// Start a sync pass unless one is running or a pairing is in progress
    /// (a pass must not dial a phone mid-handshake). Also re-reads the store,
    /// so a pairing another screen removed disappears here too.
    pub(crate) fn start_sync(&mut self, dir: &crate::dir::CoincubeDirectory) -> Task<Message> {
        if !matches!(self.flow, PairingFlow::Idle | PairingFlow::Error(_)) {
            return Task::none();
        }
        self.refresh_phones_from(dir);
        if self.sync_in_flight
            || self
                .wallet_chain
                .is_some_and(|chain| !crate::phone_signer::lan_signing_allowed(chain))
        {
            return Task::none();
        }
        self.sync_in_flight = true;
        Task::perform(
            crate::phone_signer::unpair_sync::sync(dir.clone(), self.wallet_fingerprint),
            |res| {
                Message::View(view::Message::Settings(
                    view::SettingsMessage::LocalSigning(LocalSigningMessage::PairingsSynced(res)),
                ))
            },
        )
    }

    pub(crate) fn apply_pairings_synced(
        &mut self,
        dir: &crate::dir::CoincubeDirectory,
        res: Result<crate::phone_signer::unpair_sync::SyncReport, String>,
    ) {
        self.sync_in_flight = false;
        match res {
            Ok(report) if !report.removed_by_phone.is_empty() => {
                let phones = report.removed_by_phone.join(", ");
                self.unpaired_notice = Some(if report.removed_by_phone.len() == 1 {
                    coincube_ui::localized_format!(
                        "{} removed this pairing on the phone.",
                        "{} eliminó esta vinculación en el teléfono.",
                        phones
                    )
                } else {
                    coincube_ui::localized_format!(
                        "{} removed this pairing on the phone.",
                        "{} eliminaron esta vinculación en sus teléfonos.",
                        phones
                    )
                });
            }
            Ok(_) => {}
            Err(e) => tracing::debug!("paired-phone sync: {}", e),
        }
        self.refresh_phones_from(dir);
    }

    fn pick_phone(&mut self, fp8: String, cache: &Cache) -> Task<Message> {
        let Some(selected_key) = self
            .selected_key
            .clone()
            .filter(|key| self.vault_keys.iter().any(|k| k.xpub == *key))
        else {
            self.flow = PairingFlow::Error(PairingError::InternalError(
                "Select the exact vault key held by this phone before pairing.".into(),
            ));
            return Task::none();
        };
        let Some(fingerprint) = self.wallet_fingerprint else {
            self.flow = PairingFlow::Error(PairingError::InternalError(
                "No wallet loaded — pairing needs a wallet fingerprint.".into(),
            ));
            return Task::none();
        };
        // Look up the picked phone in whatever we last
        // discovered. If the user picked from a stale list
        // and the phone has since dropped, error out cleanly.
        let Some(phone) = (match &self.flow {
            PairingFlow::PhonePicker { discovered } => {
                discovered.iter().find(|d| d.cert_fp8 == fp8).cloned()
            }
            _ => None,
        }) else {
            self.flow =
                PairingFlow::Error(PairingError::InternalError(coincube_ui::localized_format!(
                    "Phone {} no longer discoverable on the LAN.",
                    "El teléfono {} ya no se puede encontrar en la red local.",
                    fp8
                )));
            return Task::none();
        };
        let identity = match crate::phone_signer::identity::load_or_create(&cache.datadir_path) {
            Ok(id) => id,
            Err(e) => {
                self.flow =
                    PairingFlow::Error(PairingError::InternalError(format!("identity: {}", e)));
                return Task::none();
            }
        };
        let GeneratedOffer { offer } = crate::phone_signer::pairing::generate_offer(
            fingerprint,
            &identity,
            phone.instance_name.clone(),
            crate::phone_signer::pairing::OfferedKey {
                signer_xpub: selected_key,
                descriptor_sha256: self.descriptor_sha256.clone(),
            },
        );
        // Build the QR up front and fail closed if it can't be
        // rendered. Entering `Waiting` with `qr: None` would
        // show the user a "scan this QR" prompt with no code
        // and spawn the dial loop anyway — stranding them with
        // no way to complete pairing. A typed error keeps the
        // Try-Again path available instead.
        let qr = match crate::phone_signer::pairing::encode_offer(&offer)
            .ok()
            .and_then(|s| qr_code::Data::new(&s).ok())
        {
            Some(qr) => qr,
            None => {
                self.flow = PairingFlow::Error(PairingError::InternalError(
                    "Couldn't render the pairing QR code. Try again.".into(),
                ));
                return Task::none();
            }
        };
        self.flow = PairingFlow::Waiting {
            phone: phone.clone(),
            offer: offer.clone(),
            qr: Some(qr),
        };
        let expected_vault_id = fingerprint;
        let signer_fps = self.wallet_signer_fingerprints.clone();
        // Resolve the selected key's origin fingerprint from the
        // local descriptor so the listener can require the phone to
        // report that exact key, not merely some key in this vault.
        // Fail closed if the descriptor has no origin for it: pairing
        // without the binding is what we're trying to prevent.
        let Some(expected_signer_fp) = self
            .vault_key_fingerprints
            .iter()
            .find(|(xpub, _)| *xpub == offer.signer_xpub)
            .map(|(_, fp)| *fp)
        else {
            self.flow = PairingFlow::Error(PairingError::InternalError(
                "Couldn't identify the selected vault key; reload the wallet and try again.".into(),
            ));
            return Task::none();
        };
        let dir = cache.datadir_path.clone();
        let run_id = self.start_pairing_run();
        let run = self.pairing_run.clone();
        Task::perform(
            async move {
                crate::phone_signer::pairing_listener::run_pairing(
                    identity,
                    offer,
                    phone,
                    expected_vault_id,
                    signer_fps,
                    expected_signer_fp,
                    &dir,
                    &run,
                )
                .await
            },
            move |res| {
                Message::View(view::Message::Settings(
                    view::SettingsMessage::LocalSigning(LocalSigningMessage::PairingCompleted(
                        run_id, res,
                    )),
                ))
            },
        )
    }

    fn remove_reviewed_phone(&mut self, fp8: String, cache: &Cache) -> Task<Message> {
        if self.pending_remove_phone.as_deref() != Some(fp8.as_str()) {
            return Task::none();
        }
        self.pending_remove_phone = None;
        let Some(reviewed_pin) = self.pending_remove_cert_pin.take() else {
            return Task::none();
        };
        let Some(reviewed_descriptor) = self.pending_remove_descriptor.take() else {
            return Task::none();
        };
        if self.current_descriptor_digest() != Some(reviewed_descriptor) {
            return Task::none();
        }
        if self.unique_current_vault_phone(&fp8) != Some(reviewed_pin) {
            return Task::none();
        }
        if !self.apply_remove_phone(&cache.datadir_path, &fp8) {
            return Task::none();
        }
        // Tell the phone now if it's reachable.
        self.start_sync(&cache.datadir_path)
    }
}

impl State for LocalSigningState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        crate::app::view::settings::local_signing::section(menu, cache, self)
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if !self.initialised {
            self.refresh_phones(cache);
            self.initialised = true;
        }
        // Wallet switches arrive here, not via `reload`. The parent
        // `SettingsState` forwards `WalletUpdated` to the active
        // sub-panel as an `update` message; without re-deriving the
        // vault id and signer-fp set now, the next pairing offer
        // would carry the previous vault's fingerprint and the
        // resulting `PairedPhone` would be persisted with the
        // previous vault's signer fps — i.e. paired to the wrong
        // vault. A switch mid-pairing also invalidates the on-screen
        // offer (see `apply_wallet_update`).
        if let Message::WalletUpdated(Ok(wallet)) = &message {
            self.wallet = Some(wallet.clone());
            self.apply_wallet_update(wallet);
            return self.keychain_lookup(daemon.as_ref(), cache);
        }
        let msg = match message {
            Message::View(view::Message::Settings(view::SettingsMessage::LocalSigning(m))) => m,
            _ => return Task::none(),
        };
        if self
            .wallet_chain
            .is_some_and(|chain| !crate::phone_signer::lan_signing_allowed(chain))
            && matches!(
                msg,
                LocalSigningMessage::StartPairing | LocalSigningMessage::PickPhone(_)
            )
        {
            self.start_pairing_run();
            self.flow = PairingFlow::Error(PairingError::InternalError(
                crate::phone_signer::BTCB2_LAN_UNAVAILABLE.to_string(),
            ));
            return Task::none();
        }
        match msg {
            LocalSigningMessage::SelectKey(key) => {
                if matches!(self.flow, PairingFlow::Idle)
                    && self.vault_keys.iter().any(|k| k.xpub == key)
                {
                    self.selected_key = Some(key);
                }
                Task::none()
            }
            LocalSigningMessage::LookupKeychainKeys => self.keychain_lookup(daemon.as_ref(), cache),
            LocalSigningMessage::KeychainKeysFound(checksum, res) => {
                let Some(wallet) = self
                    .wallet
                    .as_ref()
                    .filter(|w| w.descriptor_checksum == checksum && !w.keychain_keys_recorded)
                else {
                    return Task::none();
                };
                match res {
                    // Through `WalletUpdated`, so every panel holding the
                    // Vault sees the record, this one included.
                    Ok(found) => Task::done(Message::WalletUpdated(Ok(Arc::new(
                        super::keychain_backfill::apply_to_wallet(wallet, &found),
                    )))),
                    Err(e) => {
                        tracing::warn!("Mobile key lookup for the Pair panel failed: {}", e);
                        Task::none()
                    }
                }
            }
            LocalSigningMessage::ToggleKeyDetails => {
                self.show_key_details = !self.show_key_details;
                Task::none()
            }
            LocalSigningMessage::StartPairing => {
                self.unpaired_notice = None;
                // We need a wallet fingerprint before we can build
                // an offer. Bail with a typed error if there's no
                // wallet loaded yet.
                if self.wallet_fingerprint.is_none() {
                    self.flow = PairingFlow::Error(PairingError::InternalError(
                        "No wallet loaded — pairing needs a wallet fingerprint.".into(),
                    ));
                    return Task::none();
                }
                // Enter the picker; the Tick subscription will keep
                // refreshing the discovered list every second.
                self.flow = PairingFlow::PhonePicker {
                    discovered: crate::phone_signer::mdns::browse(),
                };
                Task::none()
            }
            LocalSigningMessage::PickPhone(fp8) => self.pick_phone(fp8, cache),
            LocalSigningMessage::Tick => {
                self.apply_tick();
                Task::none()
            }
            LocalSigningMessage::CancelPairing => {
                // Bump the run id so any in-flight listener task's
                // completion is gated out of the UI when it eventually
                // arrives. (`Task::perform` futures aren't cancellable
                // from here; this is the next-best thing.) Note this
                // does NOT prevent the disk write: persistence now
                // lives in the spawned task, so a phone that already
                // completed the handshake before the user hit Cancel
                // stays paired — it's committed once `run_pairing`
                // returns `Ok`. Cancel only abandons the wizard UI.
                self.start_pairing_run();
                self.flow = PairingFlow::Idle;
                Task::none()
            }
            LocalSigningMessage::PairingCompleted(id, res) => {
                self.apply_pairing_completed(id, res, &cache.datadir_path);
                Task::none()
            }
            LocalSigningMessage::ReviewRemovePhone(fp8) => {
                let cert_pin = self.unique_current_vault_phone(&fp8);
                self.pending_remove_phone = cert_pin.map(|_| fp8);
                self.pending_remove_cert_pin = cert_pin;
                self.pending_remove_descriptor =
                    cert_pin.and_then(|_| self.current_descriptor_digest());
                Task::none()
            }
            LocalSigningMessage::CancelRemovePhone => {
                self.pending_remove_phone = None;
                self.pending_remove_cert_pin = None;
                self.pending_remove_descriptor = None;
                Task::none()
            }
            LocalSigningMessage::RemovePhone(fp8) => self.remove_reviewed_phone(fp8, cache),
            LocalSigningMessage::SyncTick => self.start_sync(&cache.datadir_path),
            LocalSigningMessage::PairingsSynced(res) => {
                self.apply_pairings_synced(&cache.datadir_path, res);
                Task::none()
            }
            LocalSigningMessage::DraftName(fp8, text) => {
                self.apply_draft_name(fp8, text);
                Task::none()
            }
            LocalSigningMessage::DraftFallback(fp8, text) => {
                self.apply_draft_fallback(fp8, text);
                Task::none()
            }
            LocalSigningMessage::SaveRow(fp8) => {
                self.apply_save_row(&cache.datadir_path, &fp8);
                Task::none()
            }
        }
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        // Drive the countdown while a pairing offer is on-screen,
        // and refresh the phone-picker list every second while the
        // picker is open. Otherwise keep the paired list in step with
        // the phones: removals made on either side show up within a tick.
        match self.flow {
            PairingFlow::Waiting { .. } | PairingFlow::PhonePicker { .. } => {
                iced::time::every(Duration::from_secs(1)).map(|_| {
                    Message::View(view::Message::Settings(
                        view::SettingsMessage::LocalSigning(LocalSigningMessage::Tick),
                    ))
                })
            }
            PairingFlow::Idle | PairingFlow::Error(_) => {
                iced::time::every(SYNC_INTERVAL).map(|_| {
                    Message::View(view::Message::Settings(
                        view::SettingsMessage::LocalSigning(LocalSigningMessage::SyncTick),
                    ))
                })
            }
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.pending_remove_phone = None;
        self.pending_remove_cert_pin = None;
        self.pending_remove_descriptor = None;
        if let Some(w) = wallet.as_ref() {
            self.wallet = Some(w.clone());
            self.apply_wallet_update(w);
            tracing::debug!("local-signer reload with wallet {}", w.name);
            if !w.keychain_keys_recorded {
                // `reload` has no `Cache`, so the sign-in check and the
                // lookup itself happen in `update`.
                return Task::done(Message::View(view::Message::Settings(
                    view::SettingsMessage::LocalSigning(LocalSigningMessage::LookupKeychainKeys),
                )));
            }
        } else {
            self.wallet = None;
            self.start_pairing_run();
            self.flow = PairingFlow::Idle;
            self.wallet_fingerprint = None;
            self.wallet_chain = None;
            self.wallet_signer_fingerprints = Vec::new();
            self.vault_key_fingerprints = Vec::new();
            self.vault_keys = Vec::new();
            self.keychain_keys_recorded = false;
            self.selected_key = None;
        }
        // Cache isn't passed to reload; the panel reads it on the
        // first update tick instead.
        Task::none()
    }
}

impl From<LocalSigningState> for Box<dyn State> {
    fn from(s: LocalSigningState) -> Box<dyn State> {
        Box::new(s)
    }
}

impl Drop for LocalSigningState {
    fn drop(&mut self) {
        self.pairing_run.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir::CoincubeDirectory;
    use crate::phone_signer::pairing_store;
    use coincube_core::descriptors::CoincubeDescriptor;
    use std::str::FromStr;

    /// Two distinct multisig descriptors (different keys, different
    /// timelock branch) used to exercise [`apply_wallet`] from two
    /// different vaults.
    const DESC_A: &str = "wsh(or_d(multi(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[de6eb005/48'/1'/0'/2']tpubDFGuYfS2JwiUSEXiQuNGdT3R7WTDhbaE6jbUhgYSSdhmfQcSx7ZntMPPv7nrkvAqjpj3jX9wbhSGMeKVao4qAzhbNyBi7iQmv5xxQk6H6jz/<0;1>/*),and_v(v:pkh([ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*),older(3))))#p9ax3xxp";
    const DESC_B: &str = "wsh(or_d(multi(2,[f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<0;1>/*,[2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<0;1>/*),and_v(v:thresh(1,pkh([f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<2;3>/*),a:pkh([2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<2;3>/*)),older(65535))))#9s8ekrce";

    fn fresh_dir() -> CoincubeDirectory {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "coincube-localsigning-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).expect("mkdir tempdir");
        CoincubeDirectory::new(path)
    }

    fn paired(seed: u8, name: &str, fallback: Option<&str>) -> PairedPhone {
        PairedPhone {
            signer_binding: Some(crate::phone_signer::pairing_store::SignerBinding {
                key_id: "1".into(),
                xpub: "test-xpub".into(),
                fingerprint: Fingerprint::default(),
                descriptor_sha256: vec![0; 32],
                imported_scope: None,
            }),
            cert_pin: [seed; 32],
            name: name.into(),
            paired_at_unix: 1_700_000_000,
            wallet_fingerprints: Vec::new(),
            vault_fingerprint: Fingerprint::default(),
            transport_pubkey: Vec::new(),
            fallback_addr: fallback.map(|s| s.to_string()),
        }
    }

    fn removal_state() -> LocalSigningState {
        let mut state = LocalSigningState::default();
        state.descriptor_sha256 = hex::encode([0; 32]);
        state
    }

    fn bind_descriptor(phone: &mut PairedPhone, digest: [u8; 32]) {
        phone.signer_binding.as_mut().unwrap().descriptor_sha256 = digest.to_vec();
    }

    fn fp8_of(p: &PairedPhone) -> String {
        crate::phone_signer::identity::pin_hex8(&p.cert_pin)
    }

    #[test]
    fn removing_phone_requires_matching_vault_review_and_explicit_confirmation() {
        let dir = fresh_dir();
        let phone = paired(17, "Mobile", None);
        let mut other_vault_phone = paired(18, "Other Vault", None);
        other_vault_phone.vault_fingerprint = Fingerprint::from([1, 2, 3, 4]);
        seed_store(&dir, vec![phone.clone(), other_vault_phone.clone()]);
        let cache = Cache {
            datadir_path: dir.clone(),
            ..Cache::default()
        };
        let mut state = removal_state();
        state.wallet_fingerprint = Some(Fingerprint::default());
        state.refresh_phones_from(&dir);
        state.initialised = true;
        let fp = fp8_of(&phone);

        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::RemovePhone(fp.clone()),
        ));
        assert_eq!(pairing_store::load(&dir).unwrap().phones.len(), 2);
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp8_of(&other_vault_phone)),
        ));
        assert!(state.pending_remove_phone.is_none());

        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp.clone()),
        ));
        assert_eq!(state.pending_remove_phone.as_deref(), Some(fp.as_str()));
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::CancelRemovePhone,
        ));
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::RemovePhone(fp.clone()),
        ));
        assert_eq!(pairing_store::load(&dir).unwrap().phones.len(), 2);

        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp.clone()),
        ));
        let mut reentered = removal_state();
        reentered.wallet_fingerprint = Some(Fingerprint::default());
        reentered.refresh_phones_from(&dir);
        reentered.initialised = true;
        assert!(reentered.pending_remove_phone.is_none());
        drop(send(
            &mut reentered,
            &cache,
            LocalSigningMessage::RemovePhone(fp.clone()),
        ));
        assert_eq!(pairing_store::load(&dir).unwrap().phones.len(), 2);

        drop(send(
            &mut reentered,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp.clone()),
        ));
        drop(send(
            &mut reentered,
            &cache,
            LocalSigningMessage::RemovePhone(fp.clone()),
        ));
        let stored = pairing_store::load(&dir).unwrap();
        assert_eq!(stored.phones.len(), 1);
        assert_eq!(stored.phones[0].cert_pin, other_vault_phone.cert_pin);
        assert_eq!(stored.pending_unpairs.len(), 1);
        assert!(reentered.pending_remove_phone.is_none());
        drop(send(
            &mut reentered,
            &cache,
            LocalSigningMessage::RemovePhone(fp),
        ));
        assert_eq!(pairing_store::load(&dir).unwrap().pending_unpairs.len(), 1);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn removal_uses_active_vault_when_cert_pins_share_fp8() {
        let dir = fresh_dir();
        let mut other_vault_phone = paired(41, "Other Vault", None);
        other_vault_phone.cert_pin[4] = 1;
        other_vault_phone.vault_fingerprint = Fingerprint::from([1, 2, 3, 4]);
        let mut current_vault_phone = paired(41, "Current Vault", None);
        current_vault_phone.cert_pin[4] = 2;
        assert_ne!(other_vault_phone.cert_pin, current_vault_phone.cert_pin);
        assert_eq!(fp8_of(&other_vault_phone), fp8_of(&current_vault_phone));
        // The other Vault's colliding row comes first in the store.
        seed_store(
            &dir,
            vec![other_vault_phone.clone(), current_vault_phone.clone()],
        );
        let cache = Cache {
            datadir_path: dir.clone(),
            ..Cache::default()
        };
        let mut state = removal_state();
        state.wallet_fingerprint = Some(current_vault_phone.vault_fingerprint);
        state.refresh_phones_from(&dir);
        state.initialised = true;
        let fp8 = fp8_of(&current_vault_phone);

        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp8.clone()),
        ));
        assert_eq!(state.pending_remove_phone.as_deref(), Some(fp8.as_str()));
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::RemovePhone(fp8),
        ));

        let stored = pairing_store::load(&dir).unwrap();
        assert_eq!(stored.phones.len(), 1);
        assert_eq!(stored.phones[0].cert_pin, other_vault_phone.cert_pin);
        assert_eq!(stored.pending_unpairs.len(), 1);
        assert_eq!(
            stored.pending_unpairs[0].cert_pin,
            current_vault_phone.cert_pin
        );
        assert!(state
            .tombstones
            .lock()
            .unwrap()
            .contains(&current_vault_phone.cert_pin));
        assert!(!state
            .tombstones
            .lock()
            .unwrap()
            .contains(&other_vault_phone.cert_pin));
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn removal_refuses_persisted_vault_rebinding_after_review() {
        let dir = fresh_dir();
        let original = paired(43, "Vault A", None);
        seed_store(&dir, vec![original.clone()]);
        let cache = Cache {
            datadir_path: dir.clone(),
            ..Cache::default()
        };
        let mut state = removal_state();
        state.wallet_fingerprint = Some(original.vault_fingerprint);
        state.refresh_phones_from(&dir);
        state.initialised = true;
        let fp8 = fp8_of(&original);

        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp8.clone()),
        ));
        assert_eq!(state.pending_remove_phone.as_deref(), Some(fp8.as_str()));

        // A second pairing moves the same full cert pin to Vault B on disk.
        // The UI still displays and confirms its cached Vault A row.
        let mut rebound = original.clone();
        rebound.vault_fingerprint = Fingerprint::from([1, 2, 3, 4]);
        pairing_store::upsert(&dir, rebound.clone()).unwrap();
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::RemovePhone(fp8),
        ));

        let stored = pairing_store::load(&dir).unwrap();
        assert_eq!(stored.phones.len(), 1);
        assert_eq!(stored.phones[0].cert_pin, rebound.cert_pin);
        assert_eq!(
            stored.phones[0].vault_fingerprint,
            rebound.vault_fingerprint
        );
        assert!(stored.pending_unpairs.is_empty());
        assert!(state.tombstones.lock().unwrap().is_empty());
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn removal_refuses_same_short_vault_id_with_changed_full_descriptor() {
        let dir = fresh_dir();
        let original = paired(46, "Vault A", None);
        let mut rebound = original.clone();
        rebound.name = "Vault B".into();
        let mut digest_b = [0u8; 32];
        digest_b[4] = 1; // Same four-byte Vault id, distinct full descriptor.
        bind_descriptor(&mut rebound, digest_b);
        seed_store(&dir, vec![original.clone()]);
        let cache = Cache {
            datadir_path: dir.clone(),
            ..Cache::default()
        };
        let mut state = removal_state();
        state.wallet_fingerprint = Some(Fingerprint::default());
        state.refresh_phones_from(&dir);
        state.initialised = true;
        let fp8 = fp8_of(&original);

        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp8.clone()),
        ));
        assert_eq!(state.pending_remove_phone.as_deref(), Some(fp8.as_str()));
        pairing_store::upsert(&dir, rebound.clone()).unwrap();
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::RemovePhone(fp8),
        ));

        let stored = pairing_store::load(&dir).unwrap();
        assert_eq!(stored.phones.len(), 1);
        assert_eq!(stored.phones[0].cert_pin, rebound.cert_pin);
        assert_eq!(stored.phones[0].signer_binding, rebound.signer_binding);
        assert!(stored.pending_unpairs.is_empty());
        assert!(state.tombstones.lock().unwrap().is_empty());
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn legacy_phone_without_full_descriptor_must_be_repaired_before_removal() {
        let dir = fresh_dir();
        let mut legacy = paired(47, "Legacy phone", None);
        legacy.signer_binding = None;
        seed_store(&dir, vec![legacy.clone()]);
        let cache = Cache {
            datadir_path: dir.clone(),
            ..Cache::default()
        };
        let mut state = removal_state();
        state.wallet_fingerprint = Some(Fingerprint::default());
        state.refresh_phones_from(&dir);
        state.initialised = true;
        let fp8 = fp8_of(&legacy);

        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp8.clone()),
        ));
        assert!(state.pending_remove_phone.is_none());
        assert!(!state.apply_remove_phone(&dir, &fp8));
        let stored = pairing_store::load(&dir).unwrap();
        assert_eq!(stored.phones.len(), 1);
        assert!(stored.pending_unpairs.is_empty());
        assert!(state.tombstones.lock().unwrap().is_empty());
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn removal_refuses_duplicate_full_pin_from_other_vault() {
        let dir = fresh_dir();
        let vault_a = paired(45, "Vault A", None);
        let mut vault_b = vault_a.clone();
        vault_b.name = "Vault B".into();
        vault_b.vault_fingerprint = Fingerprint::from([1, 2, 3, 4]);
        // The other Vault's duplicate pin comes first on disk.
        seed_store(&dir, vec![vault_b.clone(), vault_a.clone()]);
        let cache = Cache {
            datadir_path: dir.clone(),
            ..Cache::default()
        };
        let mut state = removal_state();
        state.wallet_fingerprint = Some(vault_a.vault_fingerprint);
        state.refresh_phones_from(&dir);
        state.initialised = true;
        let fp8 = fp8_of(&vault_a);

        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp8.clone()),
        ));
        assert_eq!(state.pending_remove_phone.as_deref(), Some(fp8.as_str()));
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::RemovePhone(fp8),
        ));

        let stored = pairing_store::load(&dir).unwrap();
        assert_eq!(stored.phones.len(), 2);
        assert_eq!(
            stored.phones[0].vault_fingerprint,
            vault_b.vault_fingerprint
        );
        assert_eq!(
            stored.phones[1].vault_fingerprint,
            vault_a.vault_fingerprint
        );
        assert!(stored.pending_unpairs.is_empty());
        assert!(state.tombstones.lock().unwrap().is_empty());
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn removal_refuses_ambiguous_fp8_within_active_vault() {
        let dir = fresh_dir();
        let mut first = paired(42, "First", None);
        first.cert_pin[4] = 1;
        let mut second = paired(42, "Second", None);
        second.cert_pin[4] = 2;
        assert_eq!(fp8_of(&first), fp8_of(&second));
        seed_store(&dir, vec![first.clone()]);
        let cache = Cache {
            datadir_path: dir.clone(),
            ..Cache::default()
        };
        let mut state = removal_state();
        state.wallet_fingerprint = Some(Fingerprint::default());
        state.refresh_phones_from(&dir);
        state.initialised = true;
        let fp8 = fp8_of(&first);

        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp8.clone()),
        ));
        assert_eq!(state.pending_remove_phone.as_deref(), Some(fp8.as_str()));
        // A second row appears before confirmation; the old review must
        // not authorize an arbitrary removal under the same short id.
        seed_store(&dir, vec![first.clone(), second.clone()]);
        state.refresh_phones_from(&dir);
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::RemovePhone(fp8.clone()),
        ));
        assert!(state.pending_remove_phone.is_none());
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp8.clone()),
        ));
        assert!(state.pending_remove_phone.is_none());
        state.apply_remove_phone(&dir, &fp8);
        let stored = pairing_store::load(&dir).unwrap();
        assert_eq!(stored.phones.len(), 2);
        assert!(stored.pending_unpairs.is_empty());
        assert!(state.tombstones.lock().unwrap().is_empty());

        // Even if one colliding row replaces the reviewed row, the old
        // confirmation cannot remove that different full cert pin.
        seed_store(&dir, vec![first]);
        state.refresh_phones_from(&dir);
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp8.clone()),
        ));
        seed_store(&dir, vec![second.clone()]);
        state.refresh_phones_from(&dir);
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::RemovePhone(fp8),
        ));
        let stored = pairing_store::load(&dir).unwrap();
        assert_eq!(stored.phones.len(), 1);
        assert_eq!(stored.phones[0].cert_pin, second.cert_pin);
        assert!(stored.pending_unpairs.is_empty());
        assert!(state.tombstones.lock().unwrap().is_empty());
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn wallet_switch_revokes_prior_vault_phone_removal_review() {
        let dir = fresh_dir();
        let wallet_a = Arc::new(Wallet::new(CoincubeDescriptor::from_str(DESC_A).unwrap()));
        let wallet_b = Arc::new(Wallet::new(CoincubeDescriptor::from_str(DESC_B).unwrap()));
        assert_ne!(wallet_a.id_fingerprint(), wallet_b.id_fingerprint());
        let mut phone_a = paired(31, "Vault A phone", None);
        phone_a.vault_fingerprint = wallet_a.id_fingerprint();
        let mut phone_b = paired(32, "Vault B phone", None);
        phone_b.vault_fingerprint = wallet_b.id_fingerprint();
        let mut state = LocalSigningState::default();
        state.apply_wallet(&wallet_a);
        bind_descriptor(&mut phone_a, state.current_descriptor_digest().unwrap());
        state.apply_wallet(&wallet_b);
        bind_descriptor(&mut phone_b, state.current_descriptor_digest().unwrap());
        state.apply_wallet(&wallet_a);
        seed_store(&dir, vec![phone_a.clone(), phone_b.clone()]);
        let cache = Cache {
            datadir_path: dir.clone(),
            ..Cache::default()
        };
        state.refresh_phones_from(&dir);
        state.initialised = true;
        let fp_a = fp8_of(&phone_a);
        let fp_b = fp8_of(&phone_b);

        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp_a.clone()),
        ));
        assert_eq!(state.pending_remove_phone.as_deref(), Some(fp_a.as_str()));

        // This is the active-panel route, which bypasses `reload`.
        drop(state.update(None, &cache, Message::WalletUpdated(Ok(wallet_b))));
        assert!(state.pending_remove_phone.is_none());
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::RemovePhone(fp_a.clone()),
        ));
        let stored = pairing_store::load(&dir).unwrap();
        assert_eq!(stored.phones.len(), 2);
        assert!(stored.pending_unpairs.is_empty());

        // The old row cannot be reviewed in B. B's own row can be
        // reviewed and removed exactly once with the existing payload.
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp_a),
        ));
        assert!(state.pending_remove_phone.is_none());
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::ReviewRemovePhone(fp_b.clone()),
        ));
        assert_eq!(state.pending_remove_phone.as_deref(), Some(fp_b.as_str()));
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::RemovePhone(fp_b.clone()),
        ));
        let stored = pairing_store::load(&dir).unwrap();
        assert_eq!(stored.phones.len(), 1);
        assert_eq!(stored.phones[0].cert_pin, phone_a.cert_pin);
        assert_eq!(stored.pending_unpairs.len(), 1);
        assert_eq!(stored.pending_unpairs[0].cert_pin, phone_b.cert_pin);
        drop(send(
            &mut state,
            &cache,
            LocalSigningMessage::RemovePhone(fp_b),
        ));
        assert_eq!(pairing_store::load(&dir).unwrap().pending_unpairs.len(), 1);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn full_descriptor_change_revokes_review_with_same_short_vault_id() {
        let wallet = Wallet::new(CoincubeDescriptor::from_str(DESC_B).unwrap());
        let mut state = LocalSigningState::default();
        state.apply_wallet(&wallet);
        let current_digest = hex::decode(&state.descriptor_sha256).unwrap();
        let mut old_digest = current_digest.clone();
        old_digest[4] ^= 1;
        assert_eq!(&old_digest[..4], &current_digest[..4]);
        state.descriptor_sha256 = hex::encode(&old_digest);
        state.pending_remove_phone = Some("abababab".into());
        state.pending_remove_cert_pin = Some([0xab; 32]);
        state.pending_remove_descriptor = Some(old_digest.try_into().unwrap());
        state.flow = PairingFlow::PhonePicker {
            discovered: Vec::new(),
        };

        assert!(state.apply_wallet_update(&wallet));
        assert_eq!(state.wallet_fingerprint, Some(wallet.id_fingerprint()));
        assert!(state.pending_remove_phone.is_none());
        assert!(state.pending_remove_cert_pin.is_none());
        assert!(state.pending_remove_descriptor.is_none());
        assert!(matches!(state.flow, PairingFlow::Idle));
    }

    fn seed_store(dir: &CoincubeDirectory, phones: Vec<PairedPhone>) {
        let file = pairing_store::PairingStoreFile {
            phones,
            ..Default::default()
        };
        pairing_store::save(dir, &file).expect("seed store");
    }

    #[test]
    fn seed_row_drafts_fills_in_one_draft_per_phone() {
        let dir = fresh_dir();
        let p1 = paired(1, "Pixel", Some("10.0.0.1:8443"));
        let p2 = paired(2, "iPhone", None);
        seed_store(&dir, vec![p1.clone(), p2.clone()]);

        let mut state = LocalSigningState::default();
        state.refresh_phones_from(&dir);

        assert_eq!(state.row_drafts.len(), 2);
        let d1 = state.row_drafts.get(&fp8_of(&p1)).expect("p1 draft");
        assert_eq!(d1.name, "Pixel");
        assert_eq!(d1.fallback, "10.0.0.1:8443");
        let d2 = state.row_drafts.get(&fp8_of(&p2)).expect("p2 draft");
        assert_eq!(d2.name, "iPhone");
        assert_eq!(d2.fallback, "");
    }

    #[test]
    fn apply_draft_name_mutates_only_the_buffer_not_the_store() {
        let dir = fresh_dir();
        let p = paired(3, "Original", None);
        seed_store(&dir, vec![p.clone()]);

        let mut state = LocalSigningState::default();
        state.refresh_phones_from(&dir);
        let fp = fp8_of(&p);
        state.apply_draft_name(fp.clone(), "Renamed".into());

        // Draft updated.
        assert_eq!(state.row_drafts.get(&fp).unwrap().name, "Renamed");
        // Store on disk unchanged.
        let on_disk = pairing_store::load(&dir).unwrap();
        assert_eq!(on_disk.phones[0].name, "Original");
    }

    #[test]
    fn apply_save_row_persists_draft_back_to_store() {
        let dir = fresh_dir();
        let p = paired(4, "Original", None);
        seed_store(&dir, vec![p.clone()]);

        let mut state = LocalSigningState::default();
        state.refresh_phones_from(&dir);
        let fp = fp8_of(&p);
        state.apply_draft_name(fp.clone(), "Renamed via Save".into());
        state.apply_draft_fallback(fp.clone(), "192.168.5.20:9000".into());
        state.apply_save_row(&dir, &fp);

        let on_disk = pairing_store::load(&dir).unwrap();
        assert_eq!(on_disk.phones[0].name, "Renamed via Save");
        assert_eq!(
            on_disk.phones[0].fallback_addr.as_deref(),
            Some("192.168.5.20:9000")
        );
    }

    #[test]
    fn apply_save_row_with_empty_fallback_clears_persisted_field() {
        let dir = fresh_dir();
        let p = paired(5, "P", Some("10.0.0.1:8443"));
        seed_store(&dir, vec![p.clone()]);

        let mut state = LocalSigningState::default();
        state.refresh_phones_from(&dir);
        let fp = fp8_of(&p);
        state.apply_draft_fallback(fp.clone(), "   ".into()); // whitespace
        state.apply_save_row(&dir, &fp);

        let on_disk = pairing_store::load(&dir).unwrap();
        assert!(on_disk.phones[0].fallback_addr.is_none());
    }

    #[test]
    fn apply_save_row_with_empty_name_keeps_prior_name() {
        let dir = fresh_dir();
        let p = paired(6, "Prior", None);
        seed_store(&dir, vec![p.clone()]);

        let mut state = LocalSigningState::default();
        state.refresh_phones_from(&dir);
        let fp = fp8_of(&p);
        state.apply_draft_name(fp.clone(), "".into());
        state.apply_save_row(&dir, &fp);

        let on_disk = pairing_store::load(&dir).unwrap();
        assert_eq!(on_disk.phones[0].name, "Prior");
    }

    /// Regression: `apply_save_row` writes trimmed values to disk
    /// but used to leave `row_drafts` holding the pre-trim
    /// whitespace, so the view's text inputs (which prefer the
    /// draft entry whenever present) would keep rendering the
    /// user's leading/trailing whitespace after a successful save.
    /// The fix drops the stale draft and re-seeds it from the
    /// just-persisted values.
    #[test]
    fn apply_save_row_resyncs_draft_with_trimmed_values() {
        let dir = fresh_dir();
        let p = paired(11, "Original", None);
        seed_store(&dir, vec![p.clone()]);

        let mut state = LocalSigningState::default();
        state.refresh_phones_from(&dir);
        let fp = fp8_of(&p);
        state.apply_draft_name(fp.clone(), "  Pixel 7  ".into());
        state.apply_draft_fallback(fp.clone(), "  10.0.0.1:443  ".into());
        state.apply_save_row(&dir, &fp);

        // Disk has the trimmed values.
        let on_disk = pairing_store::load(&dir).unwrap();
        assert_eq!(on_disk.phones[0].name, "Pixel 7");
        assert_eq!(
            on_disk.phones[0].fallback_addr.as_deref(),
            Some("10.0.0.1:443"),
        );

        // Draft must be re-synced so the row's text inputs reflect
        // the trimmed values now on disk, not the user's
        // pre-trim whitespace.
        let d = state
            .row_drafts
            .get(&fp)
            .expect("draft should be re-seeded after a successful save");
        assert_eq!(d.name, "Pixel 7");
        assert_eq!(d.fallback, "10.0.0.1:443");
    }

    #[test]
    fn apply_save_row_reverts_in_memory_when_persist_fails() {
        // Force `pairing_store::save` to fail by parking a directory
        // at the temp path it writes to — `std::fs::write(&tmp, ..)`
        // can't overwrite a directory.
        let dir = fresh_dir();
        let p = paired(10, "Original", Some("10.0.0.1:8443"));
        seed_store(&dir, vec![p.clone()]);

        let blocker = dir.path().join("paired-phones.json.tmp");
        std::fs::create_dir(&blocker).expect("create blocker dir");

        let mut state = LocalSigningState::default();
        state.refresh_phones_from(&dir);
        let fp = fp8_of(&p);
        state.apply_draft_name(fp.clone(), "Renamed".into());
        state.apply_draft_fallback(fp.clone(), "192.168.5.20:9000".into());
        state.apply_save_row(&dir, &fp);

        // In-memory phones reverted to what's actually on disk.
        assert_eq!(state.phones.phones[0].name, "Original");
        assert_eq!(
            state.phones.phones[0].fallback_addr.as_deref(),
            Some("10.0.0.1:8443")
        );
        let on_disk = pairing_store::load(&dir).unwrap();
        assert_eq!(on_disk.phones[0].name, "Original");

        // Draft preserved so the user can retry once the disk issue
        // clears.
        let draft = state.row_drafts.get(&fp).expect("draft still there");
        assert_eq!(draft.name, "Renamed");
        assert_eq!(draft.fallback, "192.168.5.20:9000");
    }

    #[test]
    fn apply_remove_phone_deletes_from_store_and_drafts() {
        let dir = fresh_dir();
        let p1 = paired(7, "Keep", None);
        let p2 = paired(8, "Drop", None);
        seed_store(&dir, vec![p1.clone(), p2.clone()]);

        let mut state = removal_state();
        state.wallet_fingerprint = Some(Fingerprint::default());
        state.refresh_phones_from(&dir);
        let fp2 = fp8_of(&p2);
        state.apply_remove_phone(&dir, &fp2);

        let on_disk = pairing_store::load(&dir).unwrap();
        assert_eq!(on_disk.phones.len(), 1);
        assert_eq!(on_disk.phones[0].cert_pin, [7u8; 32]);
        assert!(!state.row_drafts.contains_key(&fp2));
    }

    /// Removing a phone here queues a notice so the phone drops this
    /// desktop too, and a later sync explains rows a phone removed.
    #[test]
    fn apply_remove_phone_queues_a_notice_for_the_phone() {
        let dir = fresh_dir();
        let p = paired(9, "iPhone", Some("10.0.0.9:8443"));
        seed_store(&dir, vec![p.clone()]);
        let mut state = removal_state();
        state.wallet_fingerprint = Some(Fingerprint::default());
        state.refresh_phones_from(&dir);
        state.apply_remove_phone(&dir, &fp8_of(&p));

        let on_disk = pairing_store::load(&dir).unwrap();
        assert!(on_disk.phones.is_empty());
        assert_eq!(on_disk.pending_unpairs.len(), 1);
        let pending = &on_disk.pending_unpairs[0];
        assert_eq!(pending.cert_pin, p.cert_pin);
        assert_eq!(pending.fallback_addr.as_deref(), Some("10.0.0.9:8443"));
    }

    #[test]
    fn sync_waits_for_pairing_and_for_the_previous_pass() {
        let dir = fresh_dir();
        seed_store(&dir, vec![paired(1, "iPhone", None)]);
        let mut state = removal_state();
        state.flow = PairingFlow::PhonePicker {
            discovered: Vec::new(),
        };
        let _ = state.start_sync(&dir);
        assert!(!state.sync_in_flight, "no pass mid-pairing");
        assert!(state.phones.phones.is_empty(), "nor a store re-read");

        state.flow = PairingFlow::Idle;
        let _ = state.start_sync(&dir);
        assert!(state.sync_in_flight);
        assert_eq!(state.phones.phones.len(), 1, "idle ticks re-read the store");
        // A tick arriving before the pass reports back starts nothing new.
        let _ = state.start_sync(&dir);
        assert!(state.sync_in_flight);

        state.apply_pairings_synced(&dir, Ok(Default::default()));
        assert!(!state.sync_in_flight);
        assert!(state.unpaired_notice.is_none());
    }

    #[test]
    fn sync_names_phones_that_removed_this_desktop() {
        let dir = fresh_dir();
        seed_store(&dir, vec![paired(1, "Kept", None)]);
        let mut state = removal_state();
        let _ = state.start_sync(&dir);
        state.apply_pairings_synced(
            &dir,
            Ok(crate::phone_signer::unpair_sync::SyncReport {
                removed_by_phone: vec!["iPhone".into()],
            }),
        );
        assert_eq!(
            state.unpaired_notice.as_deref(),
            Some("iPhone removed this pairing on the phone.")
        );
        assert_eq!(state.phones.phones.len(), 1);
    }

    #[test]
    fn apply_remove_phone_noop_when_fp_not_present() {
        let dir = fresh_dir();
        seed_store(&dir, vec![paired(9, "Keep", None)]);

        let mut state = removal_state();
        state.refresh_phones_from(&dir);
        state.apply_remove_phone(&dir, "deadbeef");

        let on_disk = pairing_store::load(&dir).unwrap();
        assert_eq!(on_disk.phones.len(), 1);
    }

    /// Regression for the Cursor finding "Remove undone by pairing task":
    /// the spawned pairing `Task::perform` would happily call
    /// `upsert_preserving_user_fields` after its handshake completed,
    /// even if the user had clicked Remove on that row in the meantime.
    /// The fix has `apply_remove_phone` add the cert_pin to a shared
    /// tombstone set that the spawned task checks before persisting.
    /// This test pins the WRITE half of that contract: after Remove,
    /// the cert_pin is in the tombstones (so a captured clone of the
    /// Arc would see it). The READ half is exercised by the existing
    /// loopback integration tests in tests/local_signer_pairing.rs.
    #[test]
    fn apply_remove_phone_tombstones_cert_pin_for_spawned_task() {
        let dir = fresh_dir();
        let p = paired(11, "Removed", None);
        seed_store(&dir, vec![p.clone()]);

        let mut state = removal_state();
        state.wallet_fingerprint = Some(Fingerprint::default());
        state.refresh_phones_from(&dir);

        // Snapshot the Arc the way the spawned `Task::perform` would —
        // captured at spawn time, dereferenced later. Insert by Remove
        // must be visible through this captured clone.
        let captured_for_task = state.tombstones.clone();
        assert!(
            captured_for_task.lock().unwrap().is_empty(),
            "tombstones start empty"
        );

        state.apply_remove_phone(&dir, &fp8_of(&p));

        // Captured clone now sees the tombstone — proves the spawned
        // task's late completion would observe it and skip the upsert.
        let g = captured_for_task.lock().unwrap();
        assert!(
            g.contains(&p.cert_pin),
            "Remove must tombstone the cert_pin so an in-flight pairing \
             task's late completion does NOT re-add the row the user \
             just deleted. Pre-fix the tombstone was never written and \
             the task happily persisted the phone back."
        );
        assert_eq!(g.len(), 1, "only the removed cert_pin should be tombstoned");
    }

    /// A no-op Remove (fp not present in the store) must NOT pollute
    /// the tombstone set — otherwise a pairing for an unrelated cert
    /// could be silently skipped because the user once "removed" a
    /// stale fp that didn't exist.
    #[test]
    fn apply_remove_phone_does_not_tombstone_when_fp_not_present() {
        let dir = fresh_dir();
        seed_store(&dir, vec![paired(9, "Keep", None)]);

        let mut state = removal_state();
        state.refresh_phones_from(&dir);

        state.apply_remove_phone(&dir, "deadbeef");

        assert!(
            state.tombstones.lock().unwrap().is_empty(),
            "Remove against an absent fp must not write a tombstone"
        );
    }

    /// Helper: build a `PairedPhone` for the run-id tests. The
    /// exact fields don't matter — these tests only assert on the
    /// state's gating, not on what gets persisted.
    fn dummy_paired() -> PairedPhone {
        paired(42, "Test", None)
    }

    #[tokio::test]
    async fn cancelled_run_must_not_write_a_late_pairing() {
        let dir = fresh_dir();
        let mut state = removal_state();
        state.start_pairing_run();
        let run = state.pairing_run.clone();
        let task_dir = dir.clone();
        let (release, wait) = tokio::sync::oneshot::channel();
        // Reproduce the spawned task's current authorization/persistence path.
        let task = tokio::spawn(async move {
            wait.await.unwrap();
            let p = dummy_paired();
            let _ = run.decide(|| {
                pairing_store::upsert_preserving_user_fields(&task_dir, p)
                    .map_err(|e| PairingError::InternalError(e.to_string()))
            });
        });
        state.start_pairing_run(); // CancelPairing's current revocation action.
        state.flow = PairingFlow::Idle;
        release.send(()).unwrap();
        task.await.unwrap();
        assert!(
            pairing_store::load(&dir).unwrap().phones.is_empty(),
            "a cancelled task persisted a trusted row"
        );
    }

    #[test]
    fn saving_user_fields_revokes_a_provisional_pairing_writer() {
        use crate::phone_signer::pairing_transaction::PairingTransaction;
        let dir = fresh_dir();
        let phone = dummy_paired();
        let pin = phone.cert_pin;
        seed_store(&dir, vec![phone.clone()]);
        let transaction = PairingTransaction::prepare(&dir, "pending".into(), phone).unwrap();
        let mut state = LocalSigningState::default();
        state.refresh_phones_from(&dir);
        let fp8 = crate::phone_signer::identity::pin_hex8(&pin);
        state.apply_draft_name(fp8.clone(), "User edit during pairing".into());
        state.apply_save_row(&dir, &fp8);
        assert!(
            transaction.finish().is_err(),
            "stale writer retained authority after user edit"
        );
        transaction.rollback().unwrap();
        assert_eq!(
            pairing_store::load(&dir).unwrap().phones[0].name,
            "User edit during pairing"
        );
    }

    #[test]
    fn navigation_drop_revokes_pending_persistence() {
        let mut state = LocalSigningState::default();
        state.start_pairing_run();
        let authority = state.pairing_run.clone();
        drop(state);
        assert!(authority
            .decide::<()>(|| panic!("navigation allowed late persistence"))
            .is_err());
    }

    #[test]
    fn pairing_completed_with_stale_id_is_ignored() {
        let dir = fresh_dir();
        let mut state = LocalSigningState::default();
        let id = state.start_pairing_run();
        // Simulate the dispatcher's PickPhone branch: transition to
        // Waiting so the completion guard's state check would pass
        // if the id matched.
        state.flow = PairingFlow::Waiting {
            phone: crate::phone_signer::mdns::DiscoveredPhone {
                cert_fp8: "01010101".into(),
                addr: "127.0.0.1:0".parse().unwrap(),
                instance_name: "x".into(),
            },
            offer: crate::phone_signer::pairing::PairingOffer {
                signer_xpub: String::new(),
                descriptor_sha256: String::new(),
                version: 1,
                cert_der_b64: String::new(),
                cert_fp: String::new(),
                service_name: String::new(),
                wallet_fingerprint: Fingerprint::default(),
                expires_at_unix: 0,
                psk_b64: String::new(),
            },
            qr: None,
        };
        // User cancels: bumps the id, drops to Idle.
        state.start_pairing_run();
        state.flow = PairingFlow::Idle;
        // Stale Ok arrives carrying the *original* run id.
        let applied = state.apply_pairing_completed(id, Ok(dummy_paired()), &dir);
        assert!(!applied, "stale completion must not transition the flow");
        // The cancelled run must not flip the card the user is now
        // looking at — it stays on whatever the cancel left it on.
        assert!(matches!(state.flow, PairingFlow::Idle));
        // The handler itself performs no disk write (persistence is the
        // spawned task's job), so this unit test never persists. The
        // task separately observes revocation before its durable decision.
        let on_disk = pairing_store::load(&dir).expect("load store");
        assert!(on_disk.phones.is_empty());
    }

    #[test]
    fn pairing_completed_with_matching_id_transitions_to_idle_and_refreshes() {
        let dir = fresh_dir();
        // Persistence happens in the spawned pairing task; by the time
        // the completion message reaches the handler the row is already
        // on disk. Seed it to mirror that ordering.
        seed_store(&dir, vec![dummy_paired()]);

        let mut state = LocalSigningState::default();
        let id = state.start_pairing_run();
        state.flow = PairingFlow::Waiting {
            phone: crate::phone_signer::mdns::DiscoveredPhone {
                cert_fp8: "01010101".into(),
                addr: "127.0.0.1:0".parse().unwrap(),
                instance_name: "x".into(),
            },
            offer: crate::phone_signer::pairing::PairingOffer {
                signer_xpub: String::new(),
                descriptor_sha256: String::new(),
                version: 1,
                cert_der_b64: String::new(),
                cert_fp: String::new(),
                service_name: String::new(),
                wallet_fingerprint: Fingerprint::default(),
                expires_at_unix: 0,
                psk_b64: String::new(),
            },
            qr: None,
        };
        let applied = state.apply_pairing_completed(id, Ok(dummy_paired()), &dir);
        assert!(
            applied,
            "matching id while Waiting must transition the flow"
        );
        // Success drops back to Idle — the new row in the
        // paired-phones list is the user-visible success signal,
        // not a separate "Pairing complete" view state.
        assert!(matches!(state.flow, PairingFlow::Idle));
        // The handler refreshes the in-memory table from disk so the
        // just-paired phone shows in the "Paired phones" card.
        assert_eq!(state.phones.phones.len(), 1);
        assert_eq!(state.phones.phones[0].cert_pin, dummy_paired().cert_pin);
    }

    #[test]
    fn pairing_completed_when_not_waiting_is_ignored_even_with_matching_id() {
        let dir = fresh_dir();
        let mut state = LocalSigningState::default();
        let id = state.start_pairing_run();
        // No transition to Waiting — state is still Idle.
        let applied = state.apply_pairing_completed(id, Ok(dummy_paired()), &dir);
        assert!(
            !applied,
            "completion while non-Waiting must be ignored (defence in depth)",
        );
        assert!(matches!(state.flow, PairingFlow::Idle));
    }

    /// Regression: a completed pairing must surface in the in-memory
    /// "Paired phones" table even when its run was superseded/cancelled
    /// (gated out of the flow transition). The spawned task persists
    /// the row regardless; the handler must still refresh from disk so
    /// the phone doesn't vanish until the next panel reload.
    #[test]
    fn gated_out_ok_still_refreshes_table_from_disk() {
        let dir = fresh_dir();
        let mut state = LocalSigningState::default();
        let id = state.start_pairing_run();
        // User cancelled: id bumped, flow back to Idle. The original
        // run's completion is now stale.
        state.start_pairing_run();
        state.flow = PairingFlow::Idle;
        // The (uncancellable) task already wrote the row to disk.
        seed_store(&dir, vec![dummy_paired()]);

        let applied = state.apply_pairing_completed(id, Ok(dummy_paired()), &dir);
        assert!(!applied, "stale completion must not transition the flow");
        // Even gated out, the just-persisted phone is reflected.
        assert_eq!(state.phones.phones.len(), 1);
        assert_eq!(state.phones.phones[0].cert_pin, dummy_paired().cert_pin);
    }

    /// Regression: while a pairing offer was on-screen, the Tick
    /// handler only refreshed the picker list and never checked
    /// offer expiry. The view's countdown would tick down to
    /// "Pairing offer expired" but the flow stayed `Waiting`, the
    /// background `run_pairing` task kept retrying until its own
    /// expiry check finally fired (which, in the recv-stuck case,
    /// could be up to a full TTL away). Tick now promotes the
    /// transition synchronously and bumps `pairing_id` so the
    /// stale completion is gated out.
    #[test]
    fn tick_promotes_expired_waiting_to_error() {
        let mut state = LocalSigningState::default();
        let prior_id = state.start_pairing_run();
        state.flow = PairingFlow::Waiting {
            phone: crate::phone_signer::mdns::DiscoveredPhone {
                cert_fp8: "01010101".into(),
                addr: "127.0.0.1:0".parse().unwrap(),
                instance_name: "x".into(),
            },
            offer: crate::phone_signer::pairing::PairingOffer {
                signer_xpub: String::new(),
                descriptor_sha256: String::new(),
                version: 1,
                cert_der_b64: String::new(),
                cert_fp: String::new(),
                service_name: String::new(),
                wallet_fingerprint: Fingerprint::default(),
                expires_at_unix: 1, // far in the past
                psk_b64: String::new(),
            },
            qr: None,
        };

        state.apply_tick();

        assert!(matches!(
            state.flow,
            PairingFlow::Error(PairingError::OfferExpired)
        ));
        assert_ne!(
            state.pairing_id, prior_id,
            "tick must bump the run id so the in-flight task's eventual completion is ignored",
        );
    }

    /// A still-valid offer must NOT be tipped into Error by Tick —
    /// only genuine expiry transitions the state.
    #[test]
    fn tick_leaves_waiting_alone_when_offer_still_valid() {
        let mut state = LocalSigningState::default();
        let prior_id = state.start_pairing_run();
        state.flow = PairingFlow::Waiting {
            phone: crate::phone_signer::mdns::DiscoveredPhone {
                cert_fp8: "01010101".into(),
                addr: "127.0.0.1:0".parse().unwrap(),
                instance_name: "x".into(),
            },
            offer: crate::phone_signer::pairing::PairingOffer {
                signer_xpub: String::new(),
                descriptor_sha256: String::new(),
                version: 1,
                cert_der_b64: String::new(),
                cert_fp: String::new(),
                service_name: String::new(),
                wallet_fingerprint: Fingerprint::default(),
                // Far future — `is_expired` will return false.
                expires_at_unix: u64::MAX,
                psk_b64: String::new(),
            },
            qr: None,
        };

        state.apply_tick();

        assert!(matches!(state.flow, PairingFlow::Waiting { .. }));
        assert_eq!(state.pairing_id, prior_id, "no bump while offer is alive");
    }

    /// Regression: switching wallets while the user is sitting on
    /// the Paired phones page used to leave `wallet_fingerprint`
    /// and `wallet_signer_fingerprints` pointing at the previous
    /// vault, because `WalletUpdated` was forwarded to the sub-panel
    /// as an `update` message and the `update` body dropped any
    /// non-`LocalSigningMessage` variant on the floor. A pairing
    /// offer generated afterwards then carried the wrong vault id.
    /// `apply_wallet` is the helper both paths now go through.
    #[test]
    fn bitcoin_twin_switch_revokes_pairing_and_btcb2_cannot_start_or_pick() {
        use crate::chain::ChainId;
        let bitcoin =
            Wallet::new(CoincubeDescriptor::from_str(DESC_A).unwrap()).with_chain(ChainId::Bitcoin);
        let btcb2 = Wallet::new(CoincubeDescriptor::from_str(DESC_A).unwrap())
            .with_chain(ChainId::BitcoinBlake2b);
        assert_eq!(bitcoin.id_fingerprint(), btcb2.id_fingerprint());
        let mut state = LocalSigningState::default();
        state.apply_wallet(&bitcoin);
        state.flow = PairingFlow::PhonePicker { discovered: vec![] };
        let old_id = state.pairing_id;
        assert!(state.apply_wallet_update(&btcb2));
        assert_ne!(state.pairing_id, old_id);
        assert!(matches!(state.flow, PairingFlow::Idle));
        state.initialised = true; // no fixture needs a real pairing-store read
        for message in [
            LocalSigningMessage::StartPairing,
            LocalSigningMessage::PickPhone("fixture".into()),
        ] {
            let task = state.update(
                None,
                &Cache::default(),
                Message::View(view::Message::Settings(
                    view::SettingsMessage::LocalSigning(message),
                )),
            );
            assert!(iced_runtime::task::into_stream(task).is_none());
            match &state.flow {
                PairingFlow::Error(PairingError::InternalError(error)) => {
                    assert!(error.contains("chain-bound pairing"))
                }
                _ => panic!("BTCB2 pairing should be unavailable"),
            }
        }
    }

    const PHONE_FP: &str = "f714c228";
    const LEDGER_FP: &str = "2522f23c";

    fn fp(hex: &str) -> Fingerprint {
        Fingerprint::from_str(hex).unwrap()
    }

    /// `DESC_B` with both keys named, and the Keychain record as given.
    fn named_wallet(keychain: &[(&str, u64)], recorded: bool) -> Wallet {
        Wallet::new(CoincubeDescriptor::from_str(DESC_B).unwrap())
            .with_key_aliases(HashMap::from([
                (fp(PHONE_FP), "My iPhone".to_string()),
                (fp(LEDGER_FP), "Ledger".to_string()),
            ]))
            .with_keychain_keys(
                keychain.iter().map(|(hex, id)| (fp(hex), *id)).collect(),
                recorded,
            )
    }

    fn xpub_of(state: &LocalSigningState, hex: &str) -> String {
        state
            .vault_keys
            .iter()
            .find(|k| k.fingerprint == Some(fp(hex)))
            .unwrap_or_else(|| panic!("{} not offered", hex))
            .xpub
            .clone()
    }

    #[test]
    fn a_recorded_vault_offers_only_its_keychain_keys_by_name() {
        let mut state = LocalSigningState::default();
        state.apply_wallet(&named_wallet(&[(PHONE_FP, 42)], true));

        assert!(state.keychain_keys_recorded);
        assert_eq!(state.vault_keys.len(), 1, "{:?}", state.vault_keys);
        let key = &state.vault_keys[0];
        assert_eq!(key.fingerprint, Some(fp(PHONE_FP)));
        assert_eq!(key.name.as_deref(), Some("My iPhone"));
        assert!(key.keychain);
        // The one candidate is chosen for the user.
        assert_eq!(state.selected_key.as_ref(), Some(&key.xpub));
    }

    #[test]
    fn a_recorded_vault_without_keychain_keys_offers_nothing() {
        let mut state = LocalSigningState::default();
        state.apply_wallet(&named_wallet(&[], true));

        assert!(state.keychain_keys_recorded);
        assert!(state.vault_keys.is_empty());
        assert_eq!(state.selected_key, None);
        // Vault identity is still loaded; only the key list is empty.
        assert!(state.wallet_fingerprint.is_some());
    }

    #[test]
    fn an_unrecorded_vault_offers_every_key_by_name_and_selects_none() {
        let mut state = LocalSigningState::default();
        state.apply_wallet(&named_wallet(&[], false));

        assert!(!state.keychain_keys_recorded);
        let mut names: Vec<_> = state
            .vault_keys
            .iter()
            .map(|k| k.name.clone().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, vec!["Ledger", "My iPhone"]);
        assert!(state.vault_keys.iter().all(|k| !k.keychain));
        assert_eq!(state.selected_key, None, "two candidates: the user chooses");
    }

    #[test]
    fn an_unrecorded_single_key_vault_selects_nothing() {
        let key = "[f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j";
        let desc = format!("wsh(or_d(pk({key}/<0;1>/*),and_v(v:pkh({key}/<2;3>/*),older(3))))");
        let wallet = |recorded| {
            Wallet::new(CoincubeDescriptor::from_str(&desc).unwrap()).with_keychain_keys(
                if recorded {
                    HashMap::from([(fp(PHONE_FP), 42)])
                } else {
                    HashMap::new()
                },
                recorded,
            )
        };

        let mut state = LocalSigningState::default();
        state.apply_wallet(&wallet(false));
        assert_eq!(state.vault_keys.len(), 1, "{:?}", state.vault_keys);
        assert_eq!(state.selected_key, None, "may not be a Mobile key");

        // Once recorded as a Keychain key, it is chosen for the user.
        state.apply_wallet(&wallet(true));
        assert_eq!(state.selected_key, Some(xpub_of(&state, PHONE_FP)));
    }

    #[test]
    fn a_non_keychain_key_cannot_be_selected_once_recorded() {
        let mut unrecorded = LocalSigningState::default();
        unrecorded.apply_wallet(&named_wallet(&[], false));
        let ledger_xpub = xpub_of(&unrecorded, LEDGER_FP);

        let mut state = LocalSigningState::default();
        state.initialised = true;
        state.apply_wallet(&named_wallet(&[(PHONE_FP, 42)], true));
        let phone_xpub = xpub_of(&state, PHONE_FP);
        let _ = state.update(
            None,
            &Cache::default(),
            Message::View(view::Message::Settings(
                view::SettingsMessage::LocalSigning(LocalSigningMessage::SelectKey(ledger_xpub)),
            )),
        );
        assert_eq!(state.selected_key, Some(phone_xpub));
    }

    #[test]
    fn a_selection_survives_reload_while_the_key_is_still_offered() {
        let mut state = LocalSigningState::default();
        state.apply_wallet(&named_wallet(&[], false));
        let ledger_xpub = xpub_of(&state, LEDGER_FP);
        state.selected_key = Some(ledger_xpub.clone());
        state.apply_wallet(&named_wallet(&[], false));
        assert_eq!(state.selected_key, Some(ledger_xpub));

        // Once the Vault records that the Ledger is not a Keychain key, the
        // stale choice gives way to the only real candidate.
        state.apply_wallet(&named_wallet(&[(PHONE_FP, 42)], true));
        assert_eq!(state.selected_key, Some(xpub_of(&state, PHONE_FP)));
    }

    #[test]
    fn back_returns_to_key_selection_from_every_pairing_step() {
        let mut state = LocalSigningState::default();
        state.initialised = true;
        state.apply_wallet(&named_wallet(&[(PHONE_FP, 42)], true));
        let chosen = state.selected_key.clone();
        assert!(chosen.is_some());

        let steps = [
            PairingFlow::PhonePicker { discovered: vec![] },
            PairingFlow::Waiting {
                phone: crate::phone_signer::mdns::DiscoveredPhone {
                    cert_fp8: "01010101".into(),
                    addr: "127.0.0.1:0".parse().unwrap(),
                    instance_name: "x".into(),
                },
                offer: crate::phone_signer::pairing::PairingOffer {
                    signer_xpub: String::new(),
                    descriptor_sha256: String::new(),
                    version: 1,
                    cert_der_b64: String::new(),
                    cert_fp: String::new(),
                    service_name: String::new(),
                    wallet_fingerprint: Fingerprint::default(),
                    expires_at_unix: 0,
                    psk_b64: String::new(),
                },
                qr: None,
            },
            // Not retriable, so Back is the only way out of it.
            PairingFlow::Error(PairingError::ReplayRefused),
        ];
        for step in steps {
            state.flow = step;
            let _ = send(
                &mut state,
                &Cache::default(),
                LocalSigningMessage::CancelPairing,
            );
            assert!(matches!(state.flow, PairingFlow::Idle));
            assert_eq!(state.selected_key, chosen, "Back keeps the chosen key");
        }
    }

    fn send(
        state: &mut LocalSigningState,
        cache: &Cache,
        msg: LocalSigningMessage,
    ) -> Task<Message> {
        state.update(
            None,
            cache,
            Message::View(view::Message::Settings(
                view::SettingsMessage::LocalSigning(msg),
            )),
        )
    }

    fn found_phone() -> HashMap<Fingerprint, super::super::keychain_backfill::FoundKeychainKey> {
        HashMap::from([(
            fp(PHONE_FP),
            super::super::keychain_backfill::FoundKeychainKey {
                key_id: 42,
                name: "My iPhone".to_string(),
            },
        )])
    }

    /// Loads an unrecorded `DESC_B` Vault the way the app does.
    fn unrecorded_state() -> (LocalSigningState, Arc<Wallet>) {
        let wallet = Arc::new(named_wallet(&[], false));
        let mut state = LocalSigningState::default();
        state.initialised = true;
        let task = state.reload(None, Some(wallet.clone()));
        // `reload` hands the lookup to `update`, which has the `Cache`.
        assert!(iced_runtime::task::into_stream(task).is_some());
        (state, wallet)
    }

    #[test]
    fn no_lookup_while_signed_out() {
        let (mut state, _) = unrecorded_state();
        let task = send(
            &mut state,
            &Cache::default(),
            LocalSigningMessage::LookupKeychainKeys,
        );
        assert!(iced_runtime::task::into_stream(task).is_none());
        assert_eq!(state.keychain_lookup_started, None);
    }

    #[test]
    fn a_signed_in_lookup_starts_once_per_vault() {
        let (mut state, wallet) = unrecorded_state();
        let cache = Cache {
            connect_tokens: Some(crate::app::state::vault::test_support::tokens()),
            cube_id: "cube-local".to_string(),
            ..Cache::default()
        };
        // The returned task is never polled, so no request is made.
        let first = send(&mut state, &cache, LocalSigningMessage::LookupKeychainKeys);
        assert!(iced_runtime::task::into_stream(first).is_some());
        assert_eq!(
            state.keychain_lookup_started.as_ref(),
            Some(&wallet.descriptor_checksum)
        );
        let again = send(&mut state, &cache, LocalSigningMessage::LookupKeychainKeys);
        assert!(iced_runtime::task::into_stream(again).is_none());
    }

    #[test]
    fn a_recorded_vault_is_never_looked_up() {
        let wallet = Arc::new(named_wallet(&[], true));
        let mut state = LocalSigningState::default();
        state.initialised = true;
        let task = state.reload(None, Some(wallet));
        assert!(iced_runtime::task::into_stream(task).is_none());
    }

    #[tokio::test]
    async fn a_found_answer_updates_the_vault_and_filters_the_list() {
        use iced::futures::StreamExt;
        let (mut state, wallet) = unrecorded_state();
        assert_eq!(state.vault_keys.len(), 2, "unrecorded: every key offered");

        let task = send(
            &mut state,
            &Cache::default(),
            LocalSigningMessage::KeychainKeysFound(
                wallet.descriptor_checksum.clone(),
                Ok(found_phone()),
            ),
        );
        let mut stream = iced_runtime::task::into_stream(task).expect("WalletUpdated");
        let updated = loop {
            match stream.next().await {
                Some(iced_runtime::Action::Output(Message::WalletUpdated(Ok(w)))) => break w,
                Some(_) => continue,
                None => panic!("no WalletUpdated emitted"),
            }
        };
        assert!(updated.keychain_keys_recorded);
        assert_eq!(
            updated.keychain_key_ids,
            HashMap::from([(fp(PHONE_FP), 42)])
        );

        // The app hands it back to the panel like any other Vault update.
        let _ = state.update(None, &Cache::default(), Message::WalletUpdated(Ok(updated)));
        assert_eq!(state.vault_keys.len(), 1);
        assert_eq!(state.vault_keys[0].name.as_deref(), Some("My iPhone"));
    }

    #[test]
    fn a_failed_or_stale_answer_changes_nothing() {
        let (mut state, wallet) = unrecorded_state();
        let before = state.vault_keys.clone();

        let failed = send(
            &mut state,
            &Cache::default(),
            LocalSigningMessage::KeychainKeysFound(
                wallet.descriptor_checksum.clone(),
                Err("offline".to_string()),
            ),
        );
        assert!(iced_runtime::task::into_stream(failed).is_none());

        let other_vault = send(
            &mut state,
            &Cache::default(),
            LocalSigningMessage::KeychainKeysFound("othervault".to_string(), Ok(found_phone())),
        );
        assert!(iced_runtime::task::into_stream(other_vault).is_none());

        assert!(!state.keychain_keys_recorded);
        assert_eq!(state.vault_keys, before);
    }

    #[test]
    fn apply_wallet_overwrites_prior_vault_fingerprints() {
        let wallet_a = Wallet::new(CoincubeDescriptor::from_str(DESC_A).unwrap());
        let wallet_b = Wallet::new(CoincubeDescriptor::from_str(DESC_B).unwrap());
        // Sanity: the two test fixtures must be distinct, otherwise
        // the "overwrites" assertion below proves nothing.
        assert_ne!(wallet_a.id_fingerprint(), wallet_b.id_fingerprint());

        let mut state = LocalSigningState::default();
        state.apply_wallet(&wallet_a);
        assert_eq!(state.wallet_fingerprint, Some(wallet_a.id_fingerprint()));
        let signers_a = state.wallet_signer_fingerprints.clone();
        assert!(!signers_a.is_empty(), "fixture A should have signer keys");

        // Switch wallets — the previous values must NOT survive.
        state.apply_wallet(&wallet_b);
        assert_eq!(state.wallet_fingerprint, Some(wallet_b.id_fingerprint()));
        assert_ne!(
            state.wallet_signer_fingerprints, signers_a,
            "signer-fp set must be replaced, not augmented or stuck on the prior vault's",
        );
        // Defence in depth: the new set must match `wallet_b`'s
        // `descriptor_keys()` exactly (sorted).
        let mut expected_b: Vec<_> = wallet_b.descriptor_keys().into_iter().collect();
        expected_b.sort();
        assert_eq!(state.wallet_signer_fingerprints, expected_b);
    }

    #[test]
    fn second_pick_phone_invalidates_first_runs_completion() {
        let dir = fresh_dir();
        let mut state = LocalSigningState::default();
        let first_id = state.start_pairing_run();
        state.flow = PairingFlow::Waiting {
            phone: crate::phone_signer::mdns::DiscoveredPhone {
                cert_fp8: "01010101".into(),
                addr: "127.0.0.1:0".parse().unwrap(),
                instance_name: "x".into(),
            },
            offer: crate::phone_signer::pairing::PairingOffer {
                signer_xpub: String::new(),
                descriptor_sha256: String::new(),
                version: 1,
                cert_der_b64: String::new(),
                cert_fp: String::new(),
                service_name: String::new(),
                wallet_fingerprint: Fingerprint::default(),
                expires_at_unix: 0,
                psk_b64: String::new(),
            },
            qr: None,
        };
        // User picks a second phone before the first run returns.
        let _second_id = state.start_pairing_run();
        // First run's completion arrives carrying the old id.
        let applied = state.apply_pairing_completed(first_id, Ok(dummy_paired()), &dir);
        assert!(
            !applied,
            "first run's completion must not stomp the second run"
        );
    }

    /// A `Waiting` flow stamped with an arbitrary offer, for the
    /// wallet-switch tests below.
    fn waiting_flow() -> PairingFlow {
        PairingFlow::Waiting {
            phone: crate::phone_signer::mdns::DiscoveredPhone {
                cert_fp8: "01010101".into(),
                addr: "127.0.0.1:0".parse().unwrap(),
                instance_name: "x".into(),
            },
            offer: crate::phone_signer::pairing::PairingOffer {
                signer_xpub: String::new(),
                descriptor_sha256: String::new(),
                version: 2,
                cert_der_b64: String::new(),
                cert_fp: String::new(),
                service_name: String::new(),
                wallet_fingerprint: Fingerprint::default(),
                expires_at_unix: u64::MAX,
                psk_b64: String::new(),
            },
            qr: None,
        }
    }

    /// Regression for the "wallet switch mid-pairing stale" finding:
    /// switching to a *different* vault while a pairing offer is on
    /// screen must invalidate the in-flight run (bump the id so the
    /// stale task's completion is gated out) and drop the wizard back
    /// to Idle — the offer was bound to the previous vault.
    #[test]
    fn wallet_switch_during_waiting_invalidates_pairing() {
        let wallet_a = Wallet::new(CoincubeDescriptor::from_str(DESC_A).unwrap());
        let wallet_b = Wallet::new(CoincubeDescriptor::from_str(DESC_B).unwrap());
        assert_ne!(wallet_a.id_fingerprint(), wallet_b.id_fingerprint());

        let mut state = LocalSigningState::default();
        state.apply_wallet(&wallet_a);
        let run_id = state.start_pairing_run();
        state.flow = waiting_flow();

        let authority = state.pairing_run.clone();
        let invalidated = state.apply_wallet_update(&wallet_b);
        assert!(authority
            .decide::<()>(|| panic!("wallet switch must revoke persistence"))
            .is_err());

        assert!(
            invalidated,
            "a real vault switch while Waiting must invalidate"
        );
        assert!(matches!(state.flow, PairingFlow::Idle));
        assert_ne!(
            state.pairing_id, run_id,
            "run id must bump so the stale task's completion is gated out",
        );
        // Panel now reflects the new vault for the next pairing.
        assert_eq!(state.wallet_fingerprint, Some(wallet_b.id_fingerprint()));
    }

    /// A `WalletUpdated` for the *same* vault (a routine refresh) must
    /// NOT disturb an in-flight pairing.
    #[test]
    fn wallet_refresh_same_vault_leaves_pairing_untouched() {
        let wallet_a = Wallet::new(CoincubeDescriptor::from_str(DESC_A).unwrap());

        let mut state = LocalSigningState::default();
        state.apply_wallet(&wallet_a);
        let run_id = state.start_pairing_run();
        state.flow = waiting_flow();

        let invalidated = state.apply_wallet_update(&wallet_a);

        assert!(!invalidated, "same-vault refresh must not invalidate");
        assert!(matches!(state.flow, PairingFlow::Waiting { .. }));
        assert_eq!(state.pairing_id, run_id, "no id bump on same-vault refresh");
    }

    /// A vault switch while NOT mid-pairing (Idle) is a no-op beyond
    /// re-deriving the vault fields — nothing to invalidate.
    #[test]
    fn wallet_switch_while_idle_only_updates_fingerprints() {
        let wallet_a = Wallet::new(CoincubeDescriptor::from_str(DESC_A).unwrap());
        let wallet_b = Wallet::new(CoincubeDescriptor::from_str(DESC_B).unwrap());

        let mut state = LocalSigningState::default();
        state.apply_wallet(&wallet_a);
        let run_id = state.pairing_id;
        // flow is Idle by default.

        let invalidated = state.apply_wallet_update(&wallet_b);

        assert!(!invalidated);
        assert!(matches!(state.flow, PairingFlow::Idle));
        assert_eq!(state.pairing_id, run_id, "no in-flight run, no id bump");
        assert_eq!(state.wallet_fingerprint, Some(wallet_b.id_fingerprint()));
    }

    /// A vault switch while the phone picker is open resets it — the
    /// picker is a half-started flow scoped to the prior vault.
    #[test]
    fn wallet_switch_during_phone_picker_resets_to_idle() {
        let wallet_a = Wallet::new(CoincubeDescriptor::from_str(DESC_A).unwrap());
        let wallet_b = Wallet::new(CoincubeDescriptor::from_str(DESC_B).unwrap());

        let mut state = LocalSigningState::default();
        state.apply_wallet(&wallet_a);
        state.flow = PairingFlow::PhonePicker {
            discovered: Vec::new(),
        };

        let invalidated = state.apply_wallet_update(&wallet_b);

        assert!(
            invalidated,
            "an open picker must reset on a real vault switch"
        );
        assert!(matches!(state.flow, PairingFlow::Idle));
        assert_eq!(state.wallet_fingerprint, Some(wallet_b.id_fingerprint()));
    }

    /// A vault switch while an error card is showing resets it — the
    /// failure copy / Try-Again action are scoped to the prior vault.
    #[test]
    fn wallet_switch_during_error_resets_to_idle() {
        let wallet_a = Wallet::new(CoincubeDescriptor::from_str(DESC_A).unwrap());
        let wallet_b = Wallet::new(CoincubeDescriptor::from_str(DESC_B).unwrap());

        let mut state = LocalSigningState::default();
        state.apply_wallet(&wallet_a);
        state.flow = PairingFlow::Error(PairingError::WalletFingerprintMismatch {
            expected: vec![wallet_a.id_fingerprint()],
            claimed: wallet_a.id_fingerprint(),
        });

        let invalidated = state.apply_wallet_update(&wallet_b);

        assert!(
            invalidated,
            "a stale error card must reset on a real vault switch"
        );
        assert!(matches!(state.flow, PairingFlow::Idle));
        assert_eq!(state.wallet_fingerprint, Some(wallet_b.id_fingerprint()));
    }
}
