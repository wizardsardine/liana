//! Settings panel state for the local LAN signer ("Paired phones").

use std::collections::{HashMap, HashSet};
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

pub struct LocalSigningState {
    pub phones: PairingStoreFile,
    pub flow: PairingFlow,
    /// Vault id (`Wallet::id_fingerprint`) of the loaded wallet,
    /// captured during `reload`. Threaded into the pairing offer's
    /// `wallet_fingerprint` claim so the phone displays "pair with
    /// vault X" and the listener can reject an offer that was
    /// generated for a different vault.
    pub wallet_fingerprint: Option<Fingerprint>,
    /// Sorted `descriptor_keys()` of the loaded wallet (the real
    /// BIP-32 signer fingerprints). Persisted into
    /// `PairedPhone.wallet_fingerprints` so the steady-state hw
    /// refresh tick has a real signer fp to put on
    /// `HardwareWallet::Supported`. Separate from
    /// `wallet_fingerprint` (the vault id) because the vault id is
    /// intentionally NOT one of the descriptor keys — using it as
    /// the persisted signer fp would get the phone immediately
    /// downgraded to `Unsupported(NotPartOfWallet)`.
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
    /// `reload` doesn't get a `Cache` (it only sees daemon + wallet),
    /// so we defer the first store load to the first `update` tick
    /// where `cache` is available.
    initialised: bool,
}

impl Default for LocalSigningState {
    fn default() -> Self {
        Self {
            phones: PairingStoreFile::default(),
            flow: PairingFlow::Idle,
            wallet_fingerprint: None,
            wallet_signer_fingerprints: Vec::new(),
            row_drafts: HashMap::new(),
            pairing_id: 0,
            initialised: false,
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

    /// Handle a `WalletUpdated` arriving while this panel is active.
    /// Re-derives the vault id + signer fps via [`Self::apply_wallet`]
    /// and, when the vault **actually changes** mid-pairing, tears the
    /// wizard down.
    ///
    /// Why tear down: a `Waiting` offer (and the in-flight
    /// `run_pairing` task spawned for it in [`LocalSigningMessage::PickPhone`])
    /// is bound to the *previous* vault — its `wallet_fingerprint`
    /// claim, its `expected_vault_id`, and the `signer_fps` it will
    /// persist all belong to the old vault. Leaving the wizard in
    /// `Waiting` would show the user a QR for a vault the panel has
    /// navigated away from, and a phone that scans it would be
    /// persisted against the old vault while the panel claims to be on
    /// the new one. Bumping the run id gates the stale task's eventual
    /// completion out of the UI; dropping to `Idle` makes the user
    /// re-initiate pairing for the new vault.
    ///
    /// The task itself isn't cancellable, so a phone that *already*
    /// scanned the old QR stays paired to the old vault — which is the
    /// consistent outcome: that's the offer it cryptographically
    /// consumed. Returns `true` if an in-flight pairing was
    /// invalidated.
    pub(crate) fn apply_wallet_update(&mut self, wallet: &Wallet) -> bool {
        let vault_changed = self.wallet_fingerprint != Some(wallet.id_fingerprint());
        self.apply_wallet(wallet);
        if vault_changed && matches!(self.flow, PairingFlow::Waiting { .. }) {
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
            PairingFlow::Waiting { offer, .. } => {
                if crate::phone_signer::pairing::is_expired(offer) {
                    waiting_expired = true;
                }
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
                Some(crate::phone_signer::pairing_store::save(dir, &self.phones))
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

    /// Remove a row by 8-hex fingerprint. No-op if not present.
    pub(crate) fn apply_remove_phone(&mut self, dir: &crate::dir::CoincubeDirectory, fp8: &str) {
        let to_remove = self.phones.phones.iter().find_map(|p| {
            if crate::phone_signer::identity::pin_hex8(&p.cert_pin) == fp8 {
                Some(p.cert_pin)
            } else {
                None
            }
        });
        if let Some(pk) = to_remove {
            let _ = crate::phone_signer::pairing_store::remove(dir, &pk);
            self.row_drafts.remove(fp8);
            self.refresh_phones_from(dir);
        }
    }
}

impl State for LocalSigningState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        crate::app::view::settings::local_signing::section(menu, cache, self)
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
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
            self.apply_wallet_update(wallet);
            return Task::none();
        }
        let msg = match message {
            Message::View(view::Message::Settings(view::SettingsMessage::LocalSigning(m))) => m,
            _ => return Task::none(),
        };
        match msg {
            LocalSigningMessage::StartPairing => {
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
            LocalSigningMessage::PickPhone(fp8) => {
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
                    self.flow = PairingFlow::Error(PairingError::InternalError(format!(
                        "Phone {} no longer discoverable on the LAN.",
                        fp8
                    )));
                    return Task::none();
                };
                let identity =
                    match crate::phone_signer::identity::load_or_create(&cache.datadir_path) {
                        Ok(id) => id,
                        Err(e) => {
                            self.flow = PairingFlow::Error(PairingError::InternalError(format!(
                                "identity: {}",
                                e
                            )));
                            return Task::none();
                        }
                    };
                let GeneratedOffer { offer } = crate::phone_signer::pairing::generate_offer(
                    fingerprint,
                    &identity,
                    phone.instance_name.clone(),
                );
                let qr = crate::phone_signer::pairing::encode_offer(&offer)
                    .ok()
                    .and_then(|s| qr_code::Data::new(&s).ok());
                self.flow = PairingFlow::Waiting {
                    phone: phone.clone(),
                    offer: offer.clone(),
                    qr,
                };
                let expected_vault_id = fingerprint;
                let signer_fps = self.wallet_signer_fingerprints.clone();
                let dir = cache.datadir_path.clone();
                let run_id = self.start_pairing_run();
                Task::perform(
                    async move {
                        // Persist here, inside the spawned task, rather
                        // than in the `PairingCompleted` UI handler. The
                        // panel's `LocalSigningState` is recreated on any
                        // settings navigation, so a completion delivered
                        // after the user left the panel would otherwise be
                        // dropped on the floor and never written — leaving
                        // the phone paired but the desktop unaware. Once
                        // `run_pairing` returns `Ok` the phone is
                        // committed, so we always record it.
                        match crate::phone_signer::pairing_listener::run_pairing(
                            identity,
                            offer,
                            phone,
                            expected_vault_id,
                            signer_fps,
                        )
                        .await
                        {
                            Ok(p) => {
                                crate::phone_signer::pairing_store::upsert_preserving_user_fields(
                                    &dir, p,
                                )
                                .map_err(|e| PairingError::InternalError(format!("persist: {}", e)))
                            }
                            Err(e) => Err(e),
                        }
                    },
                    move |res| {
                        Message::View(view::Message::Settings(
                            view::SettingsMessage::LocalSigning(
                                LocalSigningMessage::PairingCompleted(run_id, res),
                            ),
                        ))
                    },
                )
            }
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
            LocalSigningMessage::RemovePhone(fp8) => {
                self.apply_remove_phone(&cache.datadir_path, &fp8);
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
        // picker is open.
        match self.flow {
            PairingFlow::Waiting { .. } | PairingFlow::PhonePicker { .. } => {
                iced::time::every(Duration::from_secs(1)).map(|_| {
                    Message::View(view::Message::Settings(
                        view::SettingsMessage::LocalSigning(LocalSigningMessage::Tick),
                    ))
                })
            }
            _ => iced::Subscription::none(),
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        if let Some(w) = wallet.as_ref() {
            self.apply_wallet(w);
            tracing::debug!("local-signer reload with wallet {}", w.name);
        } else {
            self.wallet_fingerprint = None;
            self.wallet_signer_fingerprints = Vec::new();
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
            cert_pin: [seed; 32],
            name: name.into(),
            paired_at_unix: 1_700_000_000,
            wallet_fingerprints: Vec::new(),
            fallback_addr: fallback.map(|s| s.to_string()),
        }
    }

    fn fp8_of(p: &PairedPhone) -> String {
        crate::phone_signer::identity::pin_hex8(&p.cert_pin)
    }

    fn seed_store(dir: &CoincubeDirectory, phones: Vec<PairedPhone>) {
        let file = pairing_store::PairingStoreFile { phones };
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

        let mut state = LocalSigningState::default();
        state.refresh_phones_from(&dir);
        let fp2 = fp8_of(&p2);
        state.apply_remove_phone(&dir, &fp2);

        let on_disk = pairing_store::load(&dir).unwrap();
        assert_eq!(on_disk.phones.len(), 1);
        assert_eq!(on_disk.phones[0].cert_pin, [7u8; 32]);
        assert!(!state.row_drafts.contains_key(&fp2));
    }

    #[test]
    fn apply_remove_phone_noop_when_fp_not_present() {
        let dir = fresh_dir();
        seed_store(&dir, vec![paired(9, "Keep", None)]);

        let mut state = LocalSigningState::default();
        state.refresh_phones_from(&dir);
        state.apply_remove_phone(&dir, "deadbeef");

        let on_disk = pairing_store::load(&dir).unwrap();
        assert_eq!(on_disk.phones.len(), 1);
    }

    /// Helper: build a `PairedPhone` for the run-id tests. The
    /// exact fields don't matter — these tests only assert on the
    /// state's gating, not on what gets persisted.
    fn dummy_paired() -> PairedPhone {
        paired(42, "Test", None)
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
        // task — which is uncancellable — does persist a completed
        // pairing even after a cancel, since the phone is committed
        // once the handshake returns `Ok`.
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

        let invalidated = state.apply_wallet_update(&wallet_b);

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
}
