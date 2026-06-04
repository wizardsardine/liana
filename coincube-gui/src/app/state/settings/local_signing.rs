//! Settings panel state for the local LAN signer ("Paired phones").

use std::collections::HashMap;
use std::sync::Arc;
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

    /// Apply the result of a pairing run **only if** `id` matches
    /// the current `pairing_id` and the wizard is still in
    /// `Waiting`. Returns `true` when the result was applied;
    /// `false` when ignored (cancelled, superseded, or already past
    /// the wizard). Datadir-typed so tests don't need a [`Cache`].
    ///
    /// Persistence happens here — not inside `run_pairing` — so a
    /// `CancelPairing` between the dial and the phone's reply can't
    /// leak a paired-phone row to disk. The in-flight `Task::perform`
    /// future isn't cancellable from the dispatcher; gating
    /// `pairing_store::upsert` on the same id-+-Waiting check that
    /// drops the UI message is the kill-the-bug-at-the-right-layer
    /// fix.
    pub(crate) fn apply_pairing_completed(
        &mut self,
        id: u64,
        res: Result<PairedPhone, PairingError>,
        dir: &crate::dir::CoincubeDirectory,
    ) -> bool {
        if id != self.pairing_id || !matches!(self.flow, PairingFlow::Waiting { .. }) {
            return false;
        }
        match res {
            Ok(p) => {
                // Re-pair must preserve user-customised fields from
                // the prior row keyed by `cert_pin`. Without this,
                // re-pair clobbers a manual fallback host:port (set
                // in the settings panel for mDNS-blocked networks)
                // and any user-applied rename — the fresh pairing
                // run only knows phone-reported defaults, so it
                // can't be the source of truth for fields the
                // desktop user has since edited. A load error here
                // is harmless: `upsert` below would surface the
                // same I/O failure, and on success this branch had
                // no prior row to preserve from anyway.
                let merged =
                    match crate::phone_signer::pairing_store::load(dir)
                        .ok()
                        .and_then(|file| {
                            file.phones
                                .into_iter()
                                .find(|existing| existing.cert_pin == p.cert_pin)
                        }) {
                        Some(existing) => PairedPhone {
                            fallback_addr: existing.fallback_addr,
                            name: existing.name,
                            ..p
                        },
                        None => p,
                    };
                match crate::phone_signer::pairing_store::upsert(dir, merged) {
                    Ok(_) => {
                        // Transition back to Idle on success. The
                        // success signal is the new row appearing in
                        // the "Paired phones" list below the pairing
                        // card — a separate "Paired with X
                        // successfully" block in the card would just
                        // duplicate that.
                        self.flow = PairingFlow::Idle;
                        self.refresh_phones_from(dir);
                    }
                    Err(e) => {
                        self.flow = PairingFlow::Error(PairingError::InternalError(format!(
                            "persist: {}",
                            e
                        )));
                    }
                }
            }
            Err(e) => {
                self.flow = PairingFlow::Error(e);
            }
        }
        true
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
                let run_id = self.start_pairing_run();
                Task::perform(
                    async move {
                        crate::phone_signer::pairing_listener::run_pairing(
                            identity,
                            offer,
                            phone,
                            expected_vault_id,
                            signer_fps,
                        )
                        .await
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
                // Refresh the discovered list while the picker is
                // open so the user sees phones appear/disappear in
                // ~real time.
                if let PairingFlow::PhonePicker { discovered } = &mut self.flow {
                    *discovered = crate::phone_signer::mdns::browse();
                }
                Task::none()
            }
            LocalSigningMessage::CancelPairing => {
                // Bump the run id so any in-flight listener task's
                // completion is ignored when it eventually arrives.
                // (`Task::perform` futures aren't cancellable from
                // here; this is the next-best thing.) The id bump
                // also short-circuits the disk write in
                // `apply_pairing_completed`, so a cancel between
                // dial and the phone's reply can't leak a paired
                // row to `paired-phones.json`.
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
        // Identify the **vault as a whole**, not one of its signers.
        // Earlier rounds picked the lexicographic min of
        // `descriptor_keys()` (the per-signer master fingerprints),
        // which surfaced one of the user's hardware-wallet keys as
        // the "wallet fingerprint" in the QR + UI — confusing on a
        // multisig vault that contains multiple keys. The vault
        // doesn't have a canonical BIP-32 fingerprint; instead derive
        // a stable 4-byte identifier from the descriptor itself
        // (`Wallet::id_fingerprint`), which is unique per vault and
        // distinct from any signer key.
        self.wallet_fingerprint = wallet.as_ref().map(|w| w.id_fingerprint());
        // Capture the descriptor's signer fingerprints (sorted for
        // determinism — `descriptor_keys()` returns a HashSet) so
        // pairing can persist them on the resulting `PairedPhone`.
        self.wallet_signer_fingerprints = wallet
            .as_ref()
            .map(|w| {
                let mut v: Vec<_> = w.descriptor_keys().into_iter().collect();
                v.sort();
                v
            })
            .unwrap_or_default();
        // Lazily load whatever's persisted so the table renders even
        // before the first pairing.
        if let Some(w) = wallet.as_ref() {
            tracing::debug!("local-signer reload with wallet {}", w.name);
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
            },
            qr: None,
        };
        // User cancels: bumps the id, drops to Idle.
        state.start_pairing_run();
        state.flow = PairingFlow::Idle;
        // Stale Ok arrives carrying the *original* run id.
        let applied = state.apply_pairing_completed(id, Ok(dummy_paired()), &dir);
        assert!(!applied, "stale completion must not be applied");
        assert!(matches!(state.flow, PairingFlow::Idle));
        // Regression: a cancel-then-completion sequence must not
        // leak a paired-phone row to disk. Before the fix the
        // listener's `pairing_store::upsert` ran regardless of UI
        // state, so the cancelled run still persisted.
        let on_disk = pairing_store::load(&dir).expect("load store");
        assert!(
            on_disk.phones.is_empty(),
            "cancelled pairing leaked to disk: {:?}",
            on_disk.phones,
        );
    }

    #[test]
    fn pairing_completed_with_matching_id_transitions_to_idle_and_persists() {
        let dir = fresh_dir();
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
            },
            qr: None,
        };
        let applied = state.apply_pairing_completed(id, Ok(dummy_paired()), &dir);
        assert!(applied, "matching id while Waiting must be applied");
        // Success drops back to Idle — the new row in the
        // paired-phones list is the user-visible success signal,
        // not a separate "Pairing complete" view state.
        assert!(matches!(state.flow, PairingFlow::Idle));
        // Persistence is now the dispatcher's job — the matching-id
        // path must write the row to disk so the steady-state hw
        // refresh tick can pick it up.
        let on_disk = pairing_store::load(&dir).expect("load store");
        assert_eq!(on_disk.phones.len(), 1);
        assert_eq!(on_disk.phones[0].cert_pin, dummy_paired().cert_pin);
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

    /// Regression: re-pairing the same phone (matched by cert pin)
    /// used to wipe `fallback_addr` and any user-applied rename
    /// because `run_pairing` rebuilds the row with phone-reported
    /// defaults and `upsert` is a full replace. The merge in
    /// `apply_pairing_completed` preserves both fields from the
    /// prior row.
    #[test]
    fn repair_preserves_existing_fallback_and_name() {
        let dir = fresh_dir();
        // Seed an already-paired row with a user-set fallback and a
        // user-applied rename.
        let prior = PairedPhone {
            cert_pin: [42u8; 32],
            name: "My Phone".into(),
            paired_at_unix: 1_700_000_000,
            wallet_fingerprints: Vec::new(),
            fallback_addr: Some("10.0.0.5:8443".into()),
        };
        pairing_store::save(
            &dir,
            &pairing_store::PairingStoreFile {
                phones: vec![prior],
            },
        )
        .expect("seed store");

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
            },
            qr: None,
        };
        // `run_pairing` would deliver a fresh row with the
        // phone-reported defaults and no fallback. Same cert_pin so
        // it joins to the prior row.
        let fresh = PairedPhone {
            cert_pin: [42u8; 32],
            name: "Pixel 8".into(),
            paired_at_unix: 1_700_999_999,
            wallet_fingerprints: vec![Fingerprint::from([1, 2, 3, 4])],
            fallback_addr: None,
        };
        let applied = state.apply_pairing_completed(id, Ok(fresh), &dir);
        assert!(applied);
        // Successful re-pair returns the wizard to Idle; the merged
        // row surfaces in `state.phones` via the refresh below.
        assert!(matches!(state.flow, PairingFlow::Idle));

        let on_disk = pairing_store::load(&dir).expect("load");
        assert_eq!(on_disk.phones.len(), 1);
        // Name and fallback preserved from the prior row.
        assert_eq!(on_disk.phones[0].name, "My Phone");
        assert_eq!(
            on_disk.phones[0].fallback_addr.as_deref(),
            Some("10.0.0.5:8443"),
        );
        // Fields that legitimately come from the fresh run survive.
        assert_eq!(on_disk.phones[0].paired_at_unix, 1_700_999_999);
        assert_eq!(
            on_disk.phones[0].wallet_fingerprints,
            vec![Fingerprint::from([1, 2, 3, 4])],
        );

        // And the in-memory snapshot mirrors disk (rendered by the
        // "Paired phones" card).
        assert_eq!(state.phones.phones.len(), 1);
        assert_eq!(state.phones.phones[0].name, "My Phone");
        assert_eq!(
            state.phones.phones[0].fallback_addr.as_deref(),
            Some("10.0.0.5:8443"),
        );
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
}
