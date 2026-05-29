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
    /// Pairing completed with this phone.
    Done(PairedPhone),
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
    /// Master fingerprint of the loaded wallet, captured during
    /// `reload`. Threaded into the pairing offer so the phone can
    /// verify it can sign for this wallet.
    pub wallet_fingerprint: Option<Fingerprint>,
    /// Per-row drafts keyed by the phone's 8-hex cert pin
    /// fingerprint. Seeded from the persisted row on load and
    /// after pairing completes; mutations are kept in memory until
    /// the user clicks Save.
    pub row_drafts: HashMap<String, RowDraft>,
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
            row_drafts: HashMap::new(),
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
            let fp8 = crate::phone_signer::identity::pin_hex8(&p.identity_pubkey);
            self.row_drafts.entry(fp8).or_insert_with(|| RowDraft {
                name: p.name.clone(),
                fallback: p.fallback_addr.clone().unwrap_or_default(),
            });
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
        if let Some(p) =
            self.phones.phones.iter_mut().find(|p| {
                crate::phone_signer::identity::pin_hex8(&p.identity_pubkey) == fp8
            })
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
            if let Err(e) = crate::phone_signer::pairing_store::save(dir, &self.phones) {
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
        }
    }

    /// Remove a row by 8-hex fingerprint. No-op if not present.
    pub(crate) fn apply_remove_phone(&mut self, dir: &crate::dir::CoincubeDirectory, fp8: &str) {
        let to_remove = self.phones.phones.iter().find_map(|p| {
            if crate::phone_signer::identity::pin_hex8(&p.identity_pubkey) == fp8 {
                Some(p.identity_pubkey)
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
                    PairingFlow::PhonePicker { discovered } => discovered
                        .iter()
                        .find(|d| d.cert_fp8 == fp8)
                        .cloned(),
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
                let dir = cache.datadir_path.clone();
                let fps = vec![fingerprint];
                Task::perform(
                    async move {
                        crate::phone_signer::pairing_listener::run_pairing(
                            identity, offer, phone, fps, dir,
                        )
                        .await
                    },
                    |res| {
                        Message::View(view::Message::Settings(
                            view::SettingsMessage::LocalSigning(
                                LocalSigningMessage::PairingCompleted(res),
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
                self.flow = PairingFlow::Idle;
                Task::none()
            }
            LocalSigningMessage::PairingCompleted(res) => {
                match res {
                    Ok(p) => {
                        self.flow = PairingFlow::Done(p);
                        self.refresh_phones(cache);
                    }
                    Err(e) => {
                        self.flow = PairingFlow::Error(e);
                    }
                }
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
                    Message::View(view::Message::Settings(view::SettingsMessage::LocalSigning(
                        LocalSigningMessage::Tick,
                    )))
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
        self.wallet_fingerprint = wallet
            .as_ref()
            .and_then(|w| w.descriptor_keys().iter().next().copied());
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
            identity_pubkey: [seed; 32],
            name: name.into(),
            paired_at_unix: 1_700_000_000,
            wallet_fingerprints: Vec::new(),
            fallback_addr: fallback.map(|s| s.to_string()),
        }
    }

    fn fp8_of(p: &PairedPhone) -> String {
        crate::phone_signer::identity::pin_hex8(&p.identity_pubkey)
    }

    fn seed_store(dir: &CoincubeDirectory, phones: Vec<PairedPhone>) {
        let mut file = pairing_store::PairingStoreFile::default();
        file.phones = phones;
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
        assert_eq!(on_disk.phones[0].identity_pubkey, [7u8; 32]);
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
}
