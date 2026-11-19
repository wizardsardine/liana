use coincube_ui::locale::{copy as localized_copy, notice as localized_notice};
use iced::widget::Space;
use iced::{Length, Task};

use coincube_ui::{
    component::{
        quote_display::{self, Quote},
        text::p1_regular,
    },
    theme,
    widget::{Column, Element},
};

use crate::app::settings::CubeSettings;
use crate::pin_input;
use crate::services::unlock::{self, PinOutcome};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub struct PinEntry {
    cube: CubeSettings,
    fork_signer: std::sync::Arc<
        std::sync::Mutex<Option<std::sync::Arc<coincube_core::signer::MasterSigner>>>,
    >,
    /// Data root, needed to reach this Cube's seed file — the PIN is verified
    /// by decrypting it, not by checking a stored hash.
    datadir_root: std::path::PathBuf,
    pin_input: pin_input::PinInput,
    error: Option<String>,
    loading: bool,
    fork_generation: u64,
    fork_task: Option<iced::task::Handle>,
    connect_provider: String,
    connect_expected_user: Option<u32>,
    // Store what to do after successful PIN entry
    pub on_success: PinEntrySuccess,
    /// This device's enrolled Connect duress account id, captured at
    /// construction so it can be carried explicitly through `DuressDetected`
    /// (Task A.1) rather than re-derived deep inside activation. `None` for a
    /// sovereign (no-Connect) enrollment.
    duress_account_id: Option<String>,
    loading_quote: Quote,
}

pub enum PinEntrySuccess {
    LoadApp {
        datadir: crate::dir::CoincubeDirectory,
        config: crate::app::Config,
        network: coincube_core::miniscript::bitcoin::Network,
        // Optional Vault wallet loading fields
        internal_bitcoind: Option<crate::node::bitcoind::Bitcoind>,
        backup: Option<crate::backup::Backup>,
        wallet_settings: Option<crate::app::settings::WalletSettings>,
        connect_client: Option<crate::services::coincube::CoincubeClient>,
    },
}

#[derive(Debug, Clone)]
pub enum Message {
    PinInput(pin_input::Message),
    Submit,
    Back,
    PinVerified,
    /// The submitted PIN matched this Cube's **duress** PIN. Bubbles up to the
    /// tab state machine, which delegates to the duress orchestrator (wipe Cube
    /// data + server POST) and locks into the cryptic "Duress Mode Activated"
    /// screen. The parent intercepts this; it is never handled inside
    /// `PinEntry::update`.
    ///
    /// Carries this device's enrolled Connect duress `account_id` (`None` for
    /// sovereign) so the orchestrator receives it explicitly — see Task A.1.
    DuressDetected {
        account_id: Option<String>,
    },
    /// The (blocking, ~831 ms) trial decryption finished off the UI thread.
    ///
    /// Carries only a classification, never the decrypted seed. Key material
    /// must not enter the message queue — iced clones messages, and every clone
    /// would be another copy of the mnemonic on the heap.
    Classified(u64, Result<Verdict, String>),
    ForkClassified(u64, Result<Verdict, String>),
}

/// What [`Message::Classified`] reports back to the UI thread.
#[derive(Debug, Clone)]
pub enum Verdict {
    Unlock,
    Duress(VerifiedUnlockDuress),
    Wrong,
}

/// One-use emergency evidence minted only by a completed PIN trial. The
/// captured origin survives Back, tab close, or a new PinEntry under the same
/// tab id; it never borrows the currently visible Cube's identity.
#[derive(Clone)]
pub struct VerifiedUnlockDuress {
    root: std::path::PathBuf,
    cube_id: String,
    chain: crate::chain::ChainId,
    account_id: Option<String>,
    claimed: Arc<AtomicBool>,
}

impl std::fmt::Debug for VerifiedUnlockDuress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VerifiedUnlockDuress(<redacted>)")
    }
}

impl VerifiedUnlockDuress {
    fn from_classification(
        root: std::path::PathBuf,
        cube_id: String,
        chain: crate::chain::ChainId,
        account_id: Option<String>,
    ) -> Self {
        Self {
            root,
            cube_id,
            chain,
            account_id,
            claimed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn origin(&self) -> (&std::path::Path, &str, crate::chain::ChainId) {
        (&self.root, &self.cube_id, self.chain)
    }

    pub(crate) fn claim(
        self,
    ) -> Option<(
        std::path::PathBuf,
        String,
        crate::chain::ChainId,
        Option<String>,
    )> {
        (!self.claimed.swap(true, Ordering::AcqRel)).then_some((
            self.root,
            self.cube_id,
            self.chain,
            self.account_id,
        ))
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        root: std::path::PathBuf,
        cube_id: String,
        chain: crate::chain::ChainId,
    ) -> Self {
        Self::from_classification(root, cube_id, chain, None)
    }
}

fn next_fork_generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

type ForkSignerSlot =
    std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<coincube_core::signer::MasterSigner>>>>;

fn retain_unlocked_signer(
    _chain: crate::chain::ChainId,
    _cube_id: &str,
    slot: &ForkSignerSlot,
    signer: coincube_core::signer::MasterSigner,
) -> Result<Verdict, String> {
    *slot
        .lock()
        .map_err(|_| "Unlock state unavailable".to_string())? = Some(std::sync::Arc::new(signer));
    Ok(Verdict::Unlock)
}

async fn check_pin_trial_admission(
    chain: crate::chain::ChainId,
    root: &std::path::Path,
    connect_client: Option<&crate::services::coincube::CoincubeClient>,
    requires_connect: bool,
    expected_user: Option<u32>,
) -> Result<(), String> {
    if chain.is_blake2b() && !requires_connect {
        crate::chain::require_local_feature(
            chain,
            connect_client.cloned(),
            &crate::dir::CoincubeDirectory::new(root.to_path_buf()),
        )
        .await?;
    } else if chain.is_blake2b() {
        let client = connect_client.ok_or_else(|| {
            "Sign in to Connect before unlocking this Bitcoin Blake2b Cube".to_string()
        })?;
        if let Some(expected_user) = expected_user {
            let user = client.get_user().await.map_err(|_| "Connect session could not be verified. Return Home and retry the account check.".to_string())?;
            if user.id != expected_user {
                return Err("Connect account changed. Return Home and sign in again.".to_string());
            }
        }
        crate::chain::require_connect_feature(
            chain,
            client,
            &crate::dir::CoincubeDirectory::new(root.to_path_buf()),
        )
        .await?;
    }
    Ok(())
}

impl PinEntry {
    pub fn new(
        cube: CubeSettings,
        datadir_root: std::path::PathBuf,
        on_success: PinEntrySuccess,
        duress_account_id: Option<String>,
    ) -> Self {
        let PinEntrySuccess::LoadApp { connect_client, .. } = &on_success;
        let connect_provider = connect_client
            .as_ref()
            .map(|client| client.base_url.clone())
            .unwrap_or_else(crate::services::coincube_api_base_url);
        let loading_quote = quote_display::random_quote("loading");
        Self {
            cube,
            fork_signer: Default::default(),
            datadir_root,
            pin_input: pin_input::PinInput::new(),
            error: None,
            loading: false,
            fork_generation: next_fork_generation(),
            fork_task: None,
            connect_provider,
            connect_expected_user: None,
            on_success,
            duress_account_id,
            loading_quote,
        }
    }

    pub fn cube(&self) -> &CubeSettings {
        &self.cube
    }

    /// The loading view a submitted PIN puts up, without classifying one.
    #[cfg(test)]
    pub(crate) fn start_loading_for_test(&mut self) {
        self.loading = true;
    }

    #[cfg(test)]
    pub(crate) fn reviewer_trial_generation(&self) -> u64 {
        self.fork_generation
    }

    pub fn pin(&self) -> zeroize::Zeroizing<String> {
        self.pin_input.value()
    }

    pub(crate) fn requires_connect(&self) -> bool {
        let PinEntrySuccess::LoadApp {
            wallet_settings,
            connect_client,
            ..
        } = &self.on_success;
        // A fork Cube without a Vault has no backend to admit yet. Signed
        // out, it opens under the local feature check; signed in, its
        // account and feature are checked as for a Connect-backed Vault.
        self.connect_capable()
            && (wallet_settings.is_some()
                || connect_client
                    .as_ref()
                    .is_some_and(|client| client.token().is_some()))
    }

    /// A fork Cube that a Connect session can serve: anything but a Vault
    /// on the managed local node.
    fn connect_capable(&self) -> bool {
        let PinEntrySuccess::LoadApp {
            datadir,
            wallet_settings,
            ..
        } = &self.on_success;
        self.cube.network.is_blake2b()
            && !crate::chain::has_managed_local_fork(
                datadir,
                self.cube.network,
                wallet_settings.as_ref(),
            )
    }

    /// Capture the opening Home's provider even while its account check is pending.
    pub(crate) fn set_connect_provider(&mut self, provider: &str) {
        self.connect_provider = provider.to_owned();
    }

    /// A candidate is not admission: cancel any old unlock and require a new
    /// PIN submission to verify this exact account and the fork feature again.
    pub(crate) fn adopt_connect_candidate(
        &mut self,
        root: &crate::dir::CoincubeDirectory,
        client: crate::services::coincube::CoincubeClient,
        expected_user: u32,
    ) -> bool {
        if !self.connect_capable()
            || !crate::chain::same_path(&self.datadir_root, root.path())
            || self.connect_provider.trim_end_matches('/') != client.base_url.trim_end_matches('/')
            || client.token().is_none_or(|token| token.trim().is_empty())
        {
            return false;
        }
        let PinEntrySuccess::LoadApp { connect_client, .. } = &self.on_success;
        if self.connect_expected_user == Some(expected_user)
            && connect_client
                .as_ref()
                .is_some_and(|current| current.token() == client.token())
        {
            return true;
        }
        self.fail_open(String::new());
        let PinEntrySuccess::LoadApp { connect_client, .. } = &mut self.on_success;
        *connect_client = Some(client);
        self.connect_expected_user = Some(expected_user);
        self.error = None;
        true
    }

    pub fn take_fork_signer(&self) -> Option<std::sync::Arc<coincube_core::signer::MasterSigner>> {
        self.fork_signer.lock().ok()?.take()
    }

    /// The blocking worker holds only this private slot. Move the signer into
    /// the process session after the matching PIN trial is accepted by its
    /// still-live tab; a cancelled worker may never write the global session.
    pub(crate) fn take_bitcoin_signer(&self) -> Option<coincube_core::signer::MasterSigner> {
        Arc::try_unwrap(self.take_fork_signer()?).ok()
    }

    pub fn fail_open(&mut self, reason: String) {
        self.loading = false;
        self.error = Some(reason);
        self.pin_input.clear();
        self.fork_generation = next_fork_generation();
        self.fork_task.take();
        // A blocking decrypt can still finish after cancellation. Detach its
        // slot so it cannot repopulate this screen's current handoff.
        self.fork_signer = Default::default();
        self.connect_expected_user = None;
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let message = match message {
            Message::ForkClassified(generation, verdict) => {
                if generation != self.fork_generation || !self.loading {
                    return Task::none();
                }
                self.fork_task.take();
                self.fork_generation = next_fork_generation();
                Message::Classified(generation, verdict)
            }
            Message::Classified(generation, verdict) => {
                if generation != self.fork_generation || !self.loading {
                    return Task::none();
                }
                // A repeated callback cannot affect a later attempt, even
                // when this same PinEntry remains on screen after Wrong.
                self.fork_generation = next_fork_generation();
                Message::Classified(generation, verdict)
            }
            msg => msg,
        };
        match message {
            Message::PinInput(pin_input::Message::Submit) => {
                // Enter key pressed in a PIN field — trigger submit
                self.update(Message::Submit)
            }
            // The face stays up while the PIN is checked; its digits are
            // already submitted.
            Message::PinInput(_) if self.loading => Task::none(),
            Message::PinInput(msg) => {
                self.error = None;
                self.pin_input.update(msg).map(Message::PinInput)
            }
            Message::Submit => self.submit_pin(),
            Message::Classified(_, Ok(Verdict::Unlock)) => {
                // Stay in `loading` — the Breez/Spark load runs next and the
                // screen must not flash back to the keypad in between.
                Task::done(Message::PinVerified)
            }
            Message::Classified(_, Ok(Verdict::Duress(_proof))) => {
                // Clear the buffer and bubble up the enrolled account id so the
                // parent can drive the orchestrator. The neutral loading screen
                // stays up during the brief async activation gap: it is
                // identical to a normal unlock, so it reveals nothing to an
                // onlooker, and it blocks further input until we lock into the
                // cryptic screen.
                self.pin_input.clear();
                let account_id = self.duress_account_id.clone();
                Task::done(Message::DuressDetected { account_id })
            }
            Message::Classified(_, Ok(Verdict::Wrong)) => {
                self.loading = false;
                let penalty = unlock::throttle::ThrottleState::load(&self.datadir_root)
                    .current_penalty(&self.cube.id);
                self.error = Some(if penalty.is_zero() {
                    "Incorrect PIN. Please try again.".to_string()
                } else {
                    unlock::throttle::lockout_message(penalty)
                });
                self.pin_input.clear();
                Task::none()
            }
            Message::Classified(_, Err(e)) => {
                // Deliberately NOT "Incorrect PIN". This is a locked keychain, a
                // missing keyring entry, or unreadable files — telling a user
                // their PIN is wrong in that situation is how they conclude
                // their wallet is gone (invariant I7).
                self.loading = false;
                self.error = Some(e);
                self.pin_input.clear();
                Task::none()
            }
            // `DuressDetected` is intercepted by the parent (tab state machine);
            // if it ever reaches here it's a no-op.
            Message::Back
            | Message::PinVerified
            | Message::DuressDetected { .. }
            | Message::ForkClassified(_, _) => Task::none(),
        }
    }

    fn submit_pin(&mut self) -> Task<Message> {
        if self.loading {
            return Task::none();
        }

        if !self.pin_input.is_complete() {
            self.error = Some("Please enter all 4 digits".to_string());
            return Task::none();
        }

        // Escalating lockout on repeated wrong PINs. Not load-bearing
        // — an offline attacker skips the UI entirely — but a laptop
        // thief shouldn't get unlimited free guesses through the front
        // door. See `services::unlock::throttle`.
        let throttle = unlock::throttle::ThrottleState::load(&self.datadir_root);
        let remaining = throttle.remaining_lockout(&self.cube.id);
        if !remaining.is_zero() {
            self.error = Some(unlock::throttle::lockout_message(remaining));
            self.pin_input.clear();
            return Task::none();
        }

        let pin = self.pin_input.value();

        self.launch_pin_trial(pin)
    }

    fn launch_pin_trial(&mut self, pin: zeroize::Zeroizing<String>) -> Task<Message> {
        // Classification costs ~831 ms on the happy path and ~1.7 s on
        // a wrong PIN with duress enrolled: the PIN is now checked by
        // decrypting the seed file, not against a 27 ms hash. That MUST
        // NOT run inline in `update()` — it would freeze the window for
        // the whole derivation. Put up the loading screen (which
        // already exists for the Breez load that follows) and do the
        // work on the blocking pool.
        self.loading = true;
        self.error = None;

        let cube = self.cube.clone();
        let root = self.datadir_root.clone();
        let fork_signer = self.fork_signer.clone();
        let PinEntrySuccess::LoadApp { connect_client, .. } = &self.on_success;
        let connect_client = connect_client.clone();
        let requires_connect = self.requires_connect();
        let expected_user = self.connect_expected_user;
        let duress_account_id = self.duress_account_id.clone();
        let generation = self.fork_generation;
        let is_fork = cube.network.is_blake2b();
        let task = Task::perform(
            async move {
                check_pin_trial_admission(
                    cube.network,
                    &root,
                    connect_client.as_ref(),
                    requires_connect,
                    expected_user,
                )
                .await?;
                tokio::task::spawn_blocking(move || {
                    let loc = unlock::CubeLocation::new(&root, &cube);
                    let attempt = if cube.network.is_blake2b() {
                        unlock::throttle::classify_connect_fork(&root, &loc, &pin)
                    } else {
                        unlock::throttle::classify_pin(&root, &loc, &pin)
                    };
                    let result = match attempt {
                        Ok(unlock::throttle::PinAttempt::Classified(outcome)) => Ok(outcome),
                        Ok(unlock::throttle::PinAttempt::Locked(remaining)) => {
                            return Err(unlock::throttle::lockout_message(remaining));
                        }
                        Err(error) => Err(error),
                    };
                    match result {
                        Ok(PinOutcome::Unlock(signer)) => {
                            retain_unlocked_signer(cube.network, &cube.id, &fork_signer, *signer)
                        }
                        Ok(PinOutcome::Duress) => {
                            Ok(Verdict::Duress(VerifiedUnlockDuress::from_classification(
                                root,
                                cube.id,
                                cube.network,
                                duress_account_id,
                            )))
                        }
                        Ok(PinOutcome::Wrong) => Ok(Verdict::Wrong),
                        // Everything else — a missing seed, a missing
                        // keyring entry, an unreachable keychain — is an
                        // operational failure, not a wrong PIN (I7).
                        //
                        // `NoPinConfigured` in particular used to be
                        // reported as `Wrong`, which told the user their
                        // (correct) PIN was incorrect *and* charged them
                        // a throttle penalty for a Cube whose seed
                        // simply is not on this device. The input is
                        // still rejected — it is never treated as a
                        // successful unlock (PR 4 / I1) — but it is not
                        // a PIN attempt and must not be counted as one.
                        Err(e) => Err(e.to_string()),
                    }
                })
                .await
                .unwrap_or_else(|e| Err(format!("PIN check failed to run: {e}")))
            },
            move |result| {
                if is_fork {
                    Message::ForkClassified(generation, result)
                } else {
                    Message::Classified(generation, result)
                }
            },
        );
        if is_fork {
            let (task, handle) = task.abortable();
            self.fork_task = Some(handle.abort_on_drop());
            task
        } else {
            task
        }
    }

    pub fn view(&self) -> Element<Message> {
        self.view_with_theme(crate::home::launcher_presenter::saved_unlock_theme(
            &self.datadir_root,
        ))
    }

    pub(crate) fn is_loading(&self) -> bool {
        self.loading
    }

    /// The quote this screen's loading view shows.
    pub(crate) fn loading_quote(&self) -> &Quote {
        &self.loading_quote
    }

    pub fn view_with_theme(
        &self,
        mode: coincube_ui::theme::palette::ThemeMode,
    ) -> Element<Message> {
        self.view_with_theme_and_floor(mode, None)
    }

    pub(crate) fn view_with_theme_and_floor<'a>(
        &'a self,
        mode: coincube_ui::theme::palette::ThemeMode,
        floor: Option<&'a crate::home::LauncherSceneContext>,
    ) -> Element<'a, Message> {
        if let Some(floor) = floor {
            crate::home::launcher_presenter::unlock_face_on_floor(
                &self.cube,
                mode,
                floor,
                move || self.unlock_controls(),
            )
        } else {
            crate::home::launcher_presenter::unlock_face(&self.cube, mode, move || {
                self.unlock_controls()
            })
        }
    }

    /// The prototype's unlock `.fc` below its heading: the PIN fieldset,
    /// Unlock, the network-checks note and Cancel.
    fn unlock_controls(&self) -> Element<Message> {
        use crate::home::launcher_presenter as face;
        let mut fields = Column::new()
            .push(Space::new().height(Length::Fixed(24.0)))
            .push(face::face_words(
                "PIN",
                13.0,
                coincube_ui::font::MEDIUM,
                false,
            ))
            .push(Space::new().height(Length::Fixed(6.0)))
            .push(self.pin_input.face_view().map(Message::PinInput))
            .push(Space::new().height(Length::Fixed(6.0)))
            .push(face::face_words(
                if self.loading {
                    localized_copy("Checking your PIN…")
                } else {
                    localized_copy("Enter this Cube's PIN.")
                },
                12.0,
                coincube_ui::font::REGULAR,
                true,
            ));
        if let Some(error) = &self.error {
            fields = fields.push(Space::new().height(Length::Fixed(6.0))).push(
                p1_regular(localized_notice(error.clone()))
                    .size(12)
                    .style(theme::text::error),
            );
        }
        let unlock = iced::widget::button(face::face_button_label(if self.loading {
            localized_copy("Unlocking…")
        } else {
            localized_copy("Unlock")
        }))
        .height(Length::Fixed(36.0))
        .padding([0.0, 16.0])
        .style(face::face_primary_style)
        .on_press_maybe((!self.loading && self.pin_input.is_complete()).then_some(Message::Submit));
        let fields = fields
            .push(Space::new().height(Length::Fixed(20.0)))
            .push(unlock);
        let cancel = iced::widget::button(face::face_link_label(localized_copy("Cancel")))
            .padding([2.0, 4.0])
            .style(face::face_link_style)
            .on_press_maybe((!self.loading).then_some(Message::Back));
        face::unlock_face_body(fields.into(), cancel.into())
    }
}

#[cfg(test)]
mod fork_handoff_tests {
    use super::*;
    fn entry(chain: crate::chain::ChainId) -> PinEntry {
        let cube = CubeSettings::new_with_raw_id("same-id".into(), "Synthetic".into(), chain);
        PinEntry::new(
            cube,
            Default::default(),
            PinEntrySuccess::LoadApp {
                datadir: crate::dir::CoincubeDirectory::new(Default::default()),
                config: crate::app::Config::new(false),
                network: chain.bitcoin_network(),
                internal_bitcoind: None,
                backup: None,
                wallet_settings: None,
                connect_client: None,
            },
            None,
        )
    }
    #[test]
    fn spanish_pin_unlock_renders_wrong_pin_and_lockout() {
        use crate::app::view::shared::landing::capture;
        use coincube_ui::locale::{render_in, Language};

        let mut pin = entry(crate::chain::ChainId::Bitcoin);
        for (error, expected) in [
            (
                "Incorrect PIN. Please try again.".to_owned(),
                "PIN incorrecto. Inténtalo de nuevo.",
            ),
            (
                unlock::throttle::lockout_message(std::time::Duration::from_secs(30)),
                "Demasiados intentos con un PIN incorrecto. Inténtalo de nuevo en 30 s.",
            ),
        ] {
            pin.error = Some(error);
            let view = render_in(Language::Spanish, || pin.unlock_controls());
            let shown = capture::inspect(
                "pin-error-es",
                view,
                (1024, 768),
                "dark",
                theme::Theme::dark(),
            );
            assert!(shown.contains("Ingresa el PIN de este Cube."));
            assert!(shown.contains("Desbloquear"));
            assert!(shown.contains(expected));
        }
    }

    #[test]
    fn the_unlock_face_stays_up_while_the_pin_is_checked() {
        let mut pin = entry(crate::chain::ChainId::Bitcoin);
        for (index, digit) in "1234".chars().enumerate() {
            drop(
                pin.update(Message::PinInput(pin_input::Message::DigitChanged(
                    index,
                    digit.to_string(),
                ))),
            );
        }
        pin.start_loading_for_test();
        // The submitted digits cannot be edited while they are checked.
        drop(
            pin.update(Message::PinInput(pin_input::Message::DigitChanged(
                0,
                "9".to_string(),
            ))),
        );
        assert_eq!(pin.pin().as_str(), "1234");
        // The face, not a separate loading screen, is what shows.
        drop(pin.view_with_theme(coincube_ui::theme::palette::ThemeMode::Dark));
        assert!(pin.is_loading());
    }

    #[tokio::test]
    async fn local_pin_reopen_uses_anonymous_capability_and_keeps_chain_bound_signer() {
        use httpmock::prelude::*;
        use iced::futures::StreamExt;
        let chain = crate::chain::ChainId::BitcoinBlake2bTestnet4;
        let path = std::env::temp_dir().join(format!("local-pin-{}", uuid::Uuid::new_v4()));
        let root = crate::dir::CoincubeDirectory::new(path.clone());
        let (_, wallet) = crate::chain::managed_local_fixture(&root, chain);
        let signer =
            coincube_core::signer::MasterSigner::generate(chain.bitcoin_network()).unwrap();
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
        let mut cube =
            CubeSettings::new_with_raw_id("local-pin-fixture".into(), "Local".into(), chain);
        cube.master_signer_fingerprint = Some(signer.fingerprint(&secp));
        signer
            .store_encrypted_for_chain(&path, chain, &secp, None, "1234", &cube.id, None)
            .unwrap();
        let server = MockServer::start_async().await;
        let feature = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api/v1/connect/features")
                    .matches(|request| {
                        request.headers.as_ref().is_none_or(|headers| {
                            headers
                                .iter()
                                .all(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
                        })
                    });
                then.status(200).json_body(
                    serde_json::json!({"data":{"plans":[],"bitcoin_blake2b_enabled":true}}),
                );
            })
            .await;
        let pref = crate::app::settings::global::GlobalSettings::path(&root);
        crate::app::settings::global::GlobalSettings::update_bitcoin_blake2b_beta(&pref, true)
            .unwrap();
        let mut pin = PinEntry::new(
            cube,
            path.clone(),
            PinEntrySuccess::LoadApp {
                datadir: root.clone(),
                config: crate::app::Config::new(false),
                network: chain.bitcoin_network(),
                internal_bitcoind: None,
                backup: None,
                wallet_settings: Some(wallet),
                connect_client: Some(crate::services::coincube::CoincubeClient::for_test(
                    server.base_url(),
                )),
            },
            None,
        );
        assert!(!pin.requires_connect());
        for (index, digit) in "1234".chars().enumerate() {
            drop(
                pin.update(Message::PinInput(pin_input::Message::DigitChanged(
                    index,
                    digit.to_string(),
                ))),
            );
        }
        let task = pin.update(Message::Submit);
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        while let Some(action) = stream.next().await {
            if let iced_runtime::Action::Output(message) = action {
                drop(pin.update(message));
            }
        }
        assert!(pin.error.is_none(), "{:?}", pin.error);
        let unlocked = pin
            .take_fork_signer()
            .expect("signed-out local PIN opens the fork signer");
        assert_eq!(unlocked.fingerprint(&secp), signer.fingerprint(&secp));
        // Continue the real successful local PIN through the tab's production
        // handoff. Do not poll the returned startup task: admission is under
        // test, and no managed node or wallet daemon should run here.
        *pin.fork_signer.lock().unwrap() = Some(unlocked);
        {
            let _session = crate::app::session::test_guard();
            let mut tab = crate::gui::tab::Tab::new(1, crate::gui::tab::State::PinEntry(pin));
            let task = tab.update(crate::gui::tab::Message::PinEntry(Message::PinVerified));
            assert!(
                task.units() > 0,
                "local handoff must schedule its dedicated startup"
            );
            let crate::gui::tab::State::Loader(loader) = &tab.state else {
                panic!("successful local PIN must reach its loader");
            };
            assert_eq!(loader.cube_settings.network, chain);
            assert!(!loader.requires_connect());
            drop(task);
            drop(tab);
            crate::app::session::close();
        }
        feature.assert_async().await;
        std::fs::remove_dir_all(path).unwrap();
    }

    async fn candidate_outputs(task: Task<Message>) -> Vec<Message> {
        use iced::futures::StreamExt;
        match iced_runtime::task::into_stream(task) {
            Some(stream) => {
                stream
                    .filter_map(|action| async move {
                        match action {
                            iced_runtime::Action::Output(message) => Some(message),
                            _ => None,
                        }
                    })
                    .collect()
                    .await
            }
            None => Vec::new(),
        }
    }

    #[tokio::test]
    async fn scoped_connect_candidate_checks_account_and_feature_before_decrypting() {
        for case in ["native", "imported", "wrong-user", "feature-off", "revoked"] {
            check_scoped_connect_candidate(case).await;
        }
    }

    async fn check_scoped_connect_candidate(case: &str) {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        let user = server.mock_async(|when, then| {
            when.method(GET).path("/api/v1/user").header("authorization", "Bearer fresh-session");
            then.status(200).delay(std::time::Duration::from_millis(50)).json_body(serde_json::json!({
                "id": if case == "wrong-user" { 8 } else { 7 }, "email":"fixture@example.invalid", "email_verified":true,
            }));
        }).await;
        let feature = server.mock_async(|when, then| {
            when.method(GET).path("/api/v1/connect/features").header("authorization", "Bearer fresh-session");
            then.status(200).json_body(serde_json::json!({"data":{"plans":[],"bitcoin_blake2b_enabled":case != "feature-off"}}));
        }).await;
        let chain = crate::chain::ChainId::BitcoinBlake2b;
        let path = std::env::temp_dir().join(format!("scoped-fork-pin-{}", uuid::Uuid::new_v4()));
        let root = crate::dir::CoincubeDirectory::new(path.clone());
        std::fs::create_dir_all(&path).unwrap();
        let signer =
            coincube_core::signer::MasterSigner::generate(chain.bitcoin_network()).unwrap();
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
        let mut cube = CubeSettings::new("Pending fork".into(), chain);
        if case == "imported" {
            let source = crate::services::bitcoin_import::pairing::tests::imported_cube();
            let import = source.bitcoin_import.unwrap();
            let source_wallet = import.active_attachment().unwrap().clone();
            let wallet = crate::app::settings::ImportedWalletAttachment::new(
                source_wallet.descriptor.paired_blake2b().unwrap(),
                chain,
                source_wallet.birthday,
                source_wallet.initial_lookahead,
            )
            .unwrap();
            cube.id = crate::services::bitcoin_import::pairing::target_cube_id(&source.id);
            cube.bitcoin_import = Some(Box::new(crate::app::settings::BitcoinImport {
                operation_id: uuid::Uuid::new_v4(),
                stage: crate::app::settings::BitcoinImportStage::Ready { wallet },
                last_error: None,
            }));
            cube.imported_claim_source = Some(Box::new(
                crate::app::settings::bitcoin_import::ImportedClaimPair {
                    source_cube_id: source.id,
                    source_operation_id: import.operation_id,
                    source_wallet,
                },
            ));
        }
        cube.master_signer_fingerprint = Some(signer.fingerprint(&secp));
        signer
            .store_encrypted_for_chain(&path, chain, &secp, None, "1234", &cube.id, None)
            .unwrap();
        let pref = crate::app::settings::global::GlobalSettings::path(&root);
        crate::app::settings::global::GlobalSettings::update_bitcoin_blake2b_beta(&pref, true)
            .unwrap();
        let mut client = crate::services::coincube::CoincubeClient::for_test(server.base_url());
        client.set_token("fresh-session");
        let mut pin = PinEntry::new(
            cube,
            path.clone(),
            PinEntrySuccess::LoadApp {
                datadir: root.clone(),
                config: crate::app::Config::new(false),
                network: chain.bitcoin_network(),
                internal_bitcoind: None,
                backup: None,
                wallet_settings: None,
                connect_client: None,
            },
            None,
        );
        pin.set_connect_provider(&server.base_url());
        let generation = pin.fork_generation;
        assert!(!pin.adopt_connect_candidate(
            &crate::dir::CoincubeDirectory::new(path.join("other-root")),
            client.clone(),
            7
        ));
        let mut other_provider = client.clone();
        other_provider.base_url = "http://127.0.0.1:1".into();
        assert!(!pin.adopt_connect_candidate(&root, other_provider, 7));
        assert_eq!(pin.fork_generation, generation);
        assert_eq!(user.hits_async().await, 0);
        assert!(pin.adopt_connect_candidate(&root, client, 7));
        for (index, digit) in "1234".chars().enumerate() {
            drop(
                pin.update(Message::PinInput(pin_input::Message::DigitChanged(
                    index,
                    digit.to_string(),
                ))),
            );
        }
        let generation = pin.fork_generation;
        let task = pin.update(Message::Submit);
        let mut messages = if case == "revoked" {
            let worker = tokio::spawn(candidate_outputs(task));
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while user.hits_async().await == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let PinEntrySuccess::LoadApp { connect_client, .. } = &pin.on_success;
            let duplicate = connect_client.clone().unwrap();
            assert!(pin.adopt_connect_candidate(&root, duplicate, 7));
            assert_eq!(
                pin.fork_generation, generation,
                "duplicate readiness must not abort verification"
            );
            assert!(pin.loading);
            pin.fail_open("Connect session changed".into());
            let PinEntrySuccess::LoadApp { connect_client, .. } = &mut pin.on_success;
            *connect_client = None;
            let _ = pin.update(Message::ForkClassified(generation, Ok(Verdict::Unlock)));
            worker.await.unwrap()
        } else {
            candidate_outputs(task).await
        };
        for message in messages.drain(..) {
            drop(pin.update(message));
        }
        if case == "native" || case == "imported" {
            assert!(pin.error.is_none(), "{}: {:?}", case, pin.error);
            assert!(
                pin.take_fork_signer().is_some(),
                "{} must unlock the chain-bound seed",
                case
            );
            feature.assert_hits_async(1).await;
        } else {
            assert!(
                pin.take_fork_signer().is_none(),
                "{} must not decrypt or retain a signer",
                case
            );
            assert!(pin.error.is_some());
            feature
                .assert_hits_async(usize::from(case == "feature-off"))
                .await;
            assert!(
                !matches!(pin.error.as_deref(), Some("Incorrect PIN")),
                "service failures are not wrong PINs"
            );
        }
        user.assert_hits_async(1).await;
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn blocking_handoff_keeps_both_signers_private_until_accepted() {
        let _guard = crate::app::session::test_guard();
        crate::app::session::close();
        crate::app::session::open("same-id", zeroize::Zeroizing::new("1234".to_string()));
        let fork = entry(crate::chain::ChainId::BitcoinBlake2b);
        let signer = coincube_core::signer::MasterSigner::generate(
            coincube_core::miniscript::bitcoin::Network::Bitcoin,
        )
        .unwrap();
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
        let fp = signer.fingerprint(&secp);
        let twin = signer.try_clone().unwrap();
        let bitcoin = entry(crate::chain::ChainId::Bitcoin);
        retain_unlocked_signer(
            crate::chain::ChainId::BitcoinBlake2b,
            "same-id",
            &fork.fork_signer,
            signer,
        )
        .unwrap();
        assert!(crate::app::session::unlocked_signer("same-id", fp).is_none());
        retain_unlocked_signer(
            crate::chain::ChainId::Bitcoin,
            "same-id",
            &bitcoin.fork_signer,
            twin,
        )
        .unwrap();
        assert!(crate::app::session::unlocked_signer("same-id", fp).is_none());
        assert!(fork.take_fork_signer().is_some());
        assert!(fork.take_fork_signer().is_none());
        let accepted = bitcoin.take_bitcoin_signer().unwrap();
        crate::app::session::store_unlocked_signer("same-id", fp, accepted);
        assert!(crate::app::session::unlocked_signer("same-id", fp).is_some());
        crate::app::session::close();
    }

    #[tokio::test]
    async fn late_blocking_handoff_and_classification_cannot_revive_invalidated_entry() {
        use iced_runtime::futures::futures::StreamExt;
        let mut fork = entry(crate::chain::ChainId::BitcoinBlake2b);
        let slot = fork.fork_signer.clone();
        let generation = fork.fork_generation;
        let (send, receive) = tokio::sync::oneshot::channel();
        let completion = tokio::spawn(async move {
            receive.await.unwrap();
            let signer = coincube_core::signer::MasterSigner::generate(
                coincube_core::miniscript::bitcoin::Network::Bitcoin,
            )
            .unwrap();
            let verdict = retain_unlocked_signer(
                crate::chain::ChainId::BitcoinBlake2b,
                "same-id",
                &slot,
                signer,
            );
            Message::ForkClassified(generation, verdict)
        });
        fork.fail_open("Signed out".into());
        send.send(()).unwrap();
        let late = completion.await.unwrap();
        let task = fork.update(late);
        if let Some(stream) = iced_runtime::task::into_stream(task) {
            assert_eq!(stream.count().await, 0);
        }
        assert!(fork.take_fork_signer().is_none());
        assert_eq!(fork.error.as_deref(), Some("Signed out"));
        assert!(!fork.loading);
    }

    #[tokio::test]
    async fn actual_pending_fork_pin_task_is_aborted_on_invalidation() {
        use iced_runtime::futures::futures::StreamExt;
        let mut fork = entry(crate::chain::ChainId::BitcoinBlake2b);
        fork.datadir_root =
            std::env::temp_dir().join(format!("fork-pin-cancel-{}", uuid::Uuid::new_v4()));
        for (i, digit) in "2468".chars().enumerate() {
            let _ = fork.update(Message::PinInput(pin_input::Message::DigitChanged(
                i,
                digit.to_string(),
            )));
        }
        let pending = fork.update(Message::Submit);
        assert!(fork.fork_task.is_some());
        fork.fail_open("Signed out".into());
        if let Some(stream) = iced_runtime::task::into_stream(pending) {
            assert_eq!(stream.count().await, 0);
        }
        assert!(fork.take_fork_signer().is_none());
    }

    #[test]
    fn failed_or_invalidated_open_discards_pending_signer() {
        let mut fork = entry(crate::chain::ChainId::BitcoinBlake2b);
        let signer = coincube_core::signer::MasterSigner::generate(
            coincube_core::miniscript::bitcoin::Network::Bitcoin,
        )
        .unwrap();
        *fork.fork_signer.lock().unwrap() = Some(std::sync::Arc::new(signer));
        fork.fail_open("Synthetic invalidation".into());
        assert!(fork.take_fork_signer().is_none());
    }
}
