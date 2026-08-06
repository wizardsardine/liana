//! Unlock screen for a passkey Cube — the assertion counterpart of
//! [`crate::pin_entry`].
//!
//! A passkey Cube has **no encrypted seed file and no PIN**. Its master seed is
//! re-derived on every open from a WebAuthn PRF assertion:
//!
//! 1. read the credential id from
//!    [`crate::app::settings::CubeSettings::passkey_metadata`];
//! 2. run [`crate::services::passkey::macos::NativePasskeyCeremony::authenticate`]
//!    against it — one Touch ID prompt;
//! 3. HKDF-SHA256 the 32-byte PRF output under `coincube-tenshu/v1/master-seed`
//!    and feed the result to BIP39 ([`MasterSigner::from_prf_output`]);
//! 4. hand the signer to [`crate::app::session`], where the Liquid and Spark
//!    loaders pick it up as `SeedSource::InMemory`.
//!
//! # Key material never enters a message
//!
//! iced clones messages freely, so every clone of a message carrying a seed
//! would be another master seed on the heap. The PRF output is consumed inside
//! [`PasskeyUnlock::update`] the moment it arrives from the ceremony channel,
//! and the only thing that leaves is the derived **fingerprint** — a public
//! identifier. Same rule `pin_entry` follows with its `Verdict`.
//!
//! # Invariant I12
//!
//! A failed, cancelled, or unsupported assertion is never reported as a lost or
//! corrupt Cube. The three outcomes come back as distinct
//! [`crate::services::passkey::PasskeyError`] variants and are rendered by
//! `PasskeyError::user_message`, which is where that copy lives.

use iced::widget::{image, Space};
use iced::{Alignment, Length, Task};

use coincube_ui::{
    component::{
        button,
        quote_display::{self, Quote, QuoteDisplayProps},
        text::{h3, p1_regular, text},
    },
    icon, theme,
    widget::{Column, Container, Element, Row},
};

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
// Referenced only from `poll`, which is `#[cfg(target_os = "macos")]` — the
// non-macOS `begin` (line 200) short-circuits before any seed is derived, so on
// other platforms this would be an unused import (`-D warnings` in CI).
#[cfg(target_os = "macos")]
use coincube_core::signer::MasterSigner;

use crate::app::settings::CubeSettings;
use crate::pin_entry::PinEntrySuccess;
use crate::services::passkey::PasskeyError;

/// How often the ceremony's channel is polled. Matches the cadence
/// `home.rs` uses for the registration ceremony.
///
/// macOS-only for the same reason as the `MasterSigner` import above: the only
/// reader is `subscription`'s `#[cfg(target_os = "macos")]` arm, and there is no
/// ceremony to poll on a platform whose `begin` short-circuits (line 200).
#[cfg(target_os = "macos")]
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

pub struct PasskeyUnlock {
    cube: CubeSettings,
    /// Data root. Unused by the assertion itself — carried so this screen has
    /// the same shape as `PinEntry` and so the caller has it in hand on success.
    #[allow(dead_code)]
    datadir_root: std::path::PathBuf,
    phase: Phase,
    /// What to do after a successful unlock. Shared with `PinEntry` so both
    /// unlock paths converge on one `LoadApp` handler in `gui::tab`.
    pub on_success: PinEntrySuccess,
    /// The live ceremony, while one is running. Dropping it cancels it.
    #[cfg(target_os = "macos")]
    ceremony: Option<crate::services::passkey::macos::NativePasskeyCeremony>,
    loading_quote: Quote,
    loading_image_handle: image::Handle,
}

/// Where the screen is. Deliberately not "loading: bool" — the user needs to
/// be able to tell "we're waiting on the system Touch ID sheet" from "we have
/// your key and are opening your wallets".
enum Phase {
    /// Nothing running; the user taps Unlock to raise the system prompt.
    Idle { error: Option<String> },
    /// The ceremony is up; the system sheet is in front of the user.
    ///
    /// Constructed only by `begin`'s `#[cfg(target_os = "macos")]` arm — the
    /// non-macOS `begin` (line 200) short-circuits straight to `Idle` with an
    /// explanatory error — so elsewhere this is matched but never built, which
    /// `dead_code` reports. Annotated per-variant, not on the enum: an
    /// enum-level allow would also silence a variant added later that really is
    /// unreachable.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Authenticating,
    /// The assertion succeeded and the wallets are loading. Stays up until
    /// `gui::tab` swaps this screen out, so the window never flashes back to
    /// the Unlock button in between.
    ///
    /// Constructed only by `poll`'s `#[cfg(target_os = "macos")]` arm; see
    /// [`Phase::Authenticating`] for why the allow is per-variant.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Opening,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Raise the system passkey prompt.
    Begin,
    /// Poll the ceremony channel. Driven by [`PasskeyUnlock::subscription`].
    Tick,
    /// Abandon the unlock and return to Home.
    Back,
    /// The assertion produced a seed and the signer is in the session.
    ///
    /// Carries the derived **fingerprint** only — never key material. The
    /// parent (`gui::tab`) intercepts this; it is a no-op inside
    /// [`PasskeyUnlock::update`].
    Unlocked { fingerprint: Fingerprint },
}

impl PasskeyUnlock {
    pub fn new(
        cube: CubeSettings,
        datadir_root: std::path::PathBuf,
        on_success: PinEntrySuccess,
    ) -> Self {
        Self {
            cube,
            datadir_root,
            phase: Phase::Idle { error: None },
            on_success,
            #[cfg(target_os = "macos")]
            ceremony: None,
            loading_quote: quote_display::random_quote("loading"),
            loading_image_handle: quote_display::image_handle_for_context("loading"),
        }
    }

    pub fn cube(&self) -> &CubeSettings {
        &self.cube
    }

    /// Poll the ceremony channel only while one is actually running. The
    /// delegate answers on the main thread and there is no `Waker` to hook, so
    /// polling is the mechanism — same as the registration ceremony in
    /// `home.rs`.
    pub fn subscription(&self) -> iced::Subscription<Message> {
        #[cfg(target_os = "macos")]
        if self.ceremony.is_some() {
            return iced::time::every(POLL_INTERVAL).map(|_| Message::Tick);
        }
        iced::Subscription::none()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Begin => self.begin(),
            Message::Tick => self.poll(),
            // Both are terminal for this screen and handled by the parent.
            // `Back` still needs the in-flight ceremony torn down here, because
            // the parent replaces the whole state and `Drop` is what cancels.
            Message::Back | Message::Unlocked { .. } => Task::none(),
        }
    }

    #[cfg(target_os = "macos")]
    fn begin(&mut self) -> Task<Message> {
        use crate::services::passkey::macos::NativePasskeyCeremony;

        if self.ceremony.is_some() {
            // Already waiting on a system sheet — a second `performRequests`
            // would stack two prompts.
            return Task::none();
        }

        let (rp_id, credential_id) = match self.credential() {
            Ok(pair) => pair,
            Err(e) => {
                self.fail(e);
                return Task::none();
            }
        };

        match NativePasskeyCeremony::authenticate(&rp_id, &credential_id) {
            Ok(ceremony) => {
                self.ceremony = Some(ceremony);
                self.phase = Phase::Authenticating;
                Task::none()
            }
            Err(e) => {
                self.fail(e);
                Task::none()
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn begin(&mut self) -> Task<Message> {
        // Passkey Cubes can only be *created* on macOS
        // (`company-brain/decisions/2026-08-03-passkey-macos-only-for-now.md`),
        // so the only way to reach this is a datadir carried across from a Mac.
        // Say what is actually true — the Cube is fine, this OS just can't open
        // it — rather than anything that reads as a lost wallet (I12).
        self.phase = Phase::Idle {
            error: Some(
                "This Cube is unlocked with a passkey, which COINCUBE currently supports on \
                 macOS only. Open it on your Mac, or restore it here from your Recovery Kit \
                 and its recovery password."
                    .to_string(),
            ),
        };
        Task::none()
    }

    /// This Cube's relying-party id and raw credential id.
    #[cfg(target_os = "macos")]
    fn credential(&self) -> Result<(String, Vec<u8>), PasskeyError> {
        use base64::Engine;

        let Some(meta) = self.cube.passkey_metadata.as_ref() else {
            // `gui::tab` only routes here for `is_passkey_cube()`, so this is
            // unreachable rather than a user-facing case — but it must not
            // panic on a hand-edited settings.json.
            return Err(PasskeyError::CredentialNotFound(
                "this Cube has no passkey metadata".to_string(),
            ));
        };

        // The RP id is read from the Cube, not from `passkey_svc::RP_ID`: a
        // credential registered under one RP cannot be asserted under another,
        // so a build whose `COINCUBE_PASSKEY_RP_ID` changed after this Cube was
        // created must still be able to open it.
        let rp_id = meta.rp_id.clone();

        let credential_id = base64::engine::general_purpose::STANDARD
            .decode(&meta.credential_id)
            .map_err(|e| {
                PasskeyError::CredentialNotFound(format!(
                    "this Cube's stored credential id is not valid base64: {e}"
                ))
            })?;

        Ok((rp_id, credential_id))
    }

    /// Drain the ceremony channel. Returns `Message::Unlocked` once the seed is
    /// derived and parked in the session.
    #[cfg(target_os = "macos")]
    fn poll(&mut self) -> Task<Message> {
        use crate::services::passkey::macos::NativeOutcome;

        let Some(outcome) = self.ceremony.as_ref().and_then(|c| c.try_recv()) else {
            return Task::none();
        };
        // One ceremony, one result. Drop it now so the subscription stops and a
        // late duplicate can't be read as a second unlock.
        self.ceremony = None;

        match outcome {
            NativeOutcome::Authenticated { prf_output } => {
                // Derived here, inline, and handed straight to the session.
                // ~1 ms (HKDF + BIP39 + one BIP-32 master derivation), so it
                // does not need the blocking pool the way an Argon2id pass
                // does — and keeping it inline is what keeps the seed out of
                // the message queue.
                let signer = match MasterSigner::from_prf_output(self.cube.network, &prf_output) {
                    Ok(signer) => signer,
                    Err(e) => {
                        tracing::error!(
                            cube_id = %self.cube.id,
                            error = %e,
                            "passkey assertion succeeded but the seed would not derive"
                        );
                        self.fail(PasskeyError::InvalidPrfOutput);
                        return Task::none();
                    }
                };

                let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
                let fingerprint = signer.fingerprint(&secp);

                // Guard against opening the wrong wallet. The Cube records the
                // fingerprint its passkey derived at creation; if the assertion
                // produces a different one, some *other* credential answered
                // and its seed is not this Cube's. Opening anyway would show a
                // valid-looking, empty wallet and invite the user to conclude
                // their coins are gone.
                if let Some(expected) = self.cube.master_signer_fingerprint {
                    if expected != fingerprint {
                        tracing::error!(
                            cube_id = %self.cube.id,
                            %expected,
                            derived = %fingerprint,
                            "passkey assertion derived a seed that is not this Cube's"
                        );
                        self.fail(PasskeyError::CredentialNotFound(
                            "the passkey that answered belongs to a different Cube".to_string(),
                        ));
                        return Task::none();
                    }
                }

                crate::app::session::store_unlocked_signer(&self.cube.id, fingerprint, signer);
                crate::app::session::open_without_pin(self.cube.id.clone());

                self.phase = Phase::Opening;
                Task::done(Message::Unlocked { fingerprint })
            }
            NativeOutcome::Registered { .. } => {
                // An assertion request cannot produce a registration; if it
                // somehow does, the response is not one we can derive from.
                self.fail(PasskeyError::InvalidResponse(
                    "the authenticator answered an unlock with a registration".to_string(),
                ));
                Task::none()
            }
            NativeOutcome::Failed(e) => {
                self.fail(e);
                Task::none()
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn poll(&mut self) -> Task<Message> {
        Task::none()
    }

    /// Land a classified failure on the screen. **I12**: the copy comes from
    /// [`PasskeyError::user_message`], and none of it says the Cube is lost.
    fn fail(&mut self, error: PasskeyError) {
        tracing::warn!(cube_id = %self.cube.id, error = %error, "passkey unlock did not complete");
        self.phase = Phase::Idle {
            error: Some(error.user_message()),
        };
    }

    pub fn view(&self) -> Element<Message> {
        if matches!(self.phase, Phase::Opening) {
            // Same full-screen loading treatment the PIN path uses once its
            // seed is in hand, so the two unlock paths look identical from the
            // moment they succeed.
            return Container::new(
                Column::new()
                    .width(Length::Fill)
                    .spacing(20)
                    .align_x(Alignment::Center)
                    .push(Space::new().height(Length::Fill))
                    .push(quote_display::display(&QuoteDisplayProps::new(
                        "loading",
                        &self.loading_quote,
                        &self.loading_image_handle,
                    )))
                    .push(crate::loading::loading_indicator(None))
                    .push(text("Loading your Cube...").style(theme::text::secondary))
                    .push(Space::new().height(Length::Fill)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(50)
            .into();
        }

        let back_button = button::secondary(Some(icon::previous_icon()), "Back")
            .width(Length::Fixed(150.0))
            .on_press(Message::Back);

        let header = Row::new()
            .align_y(Alignment::Center)
            .push(Container::new(back_button).center_x(Length::FillPortion(2)))
            .push(Space::new().width(Length::FillPortion(8)))
            .push(Space::new().width(Length::FillPortion(2)));

        let mut content = Column::new()
            .spacing(30)
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .push(h3(format!("Unlock {}", self.cube.name)));

        match &self.phase {
            Phase::Authenticating => {
                content = content
                    .push(
                        p1_regular("Confirm with Touch ID to unlock this Cube.")
                            .style(theme::text::secondary),
                    )
                    .push(crate::loading::loading_indicator(None));
            }
            Phase::Idle { error } => {
                content = content.push(
                    p1_regular("This Cube is unlocked with a passkey — there's no PIN to enter.")
                        .style(theme::text::secondary),
                );
                if let Some(error) = error {
                    content = content.push(p1_regular(error.as_str()).style(theme::text::error));
                }
                content = content.push(
                    button::primary(None, "Unlock with passkey")
                        .width(Length::Fixed(280.0))
                        .on_press(Message::Begin),
                );
            }
            Phase::Opening => unreachable!("handled above"),
        }

        Container::new(
            Column::new()
                .width(Length::Fill)
                .push(Space::new().height(Length::Fixed(100.0)))
                .push(header)
                .push(Space::new().height(Length::Fixed(100.0)))
                .push(Container::new(content).center_x(Length::Fill)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(20)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use crate::services::passkey::PasskeyError;

    /// **I12**, stated as a test. Three outcomes, three different messages, and
    /// not one of them tells the user their Cube is gone.
    #[test]
    fn no_passkey_failure_reads_as_a_lost_cube() {
        let cancelled = PasskeyError::Cancelled.user_message();
        let prf = PasskeyError::PrfNotSupported.user_message();
        let missing = PasskeyError::CredentialNotFound("code 1004".to_string()).user_message();

        for msg in [&cancelled, &prf, &missing] {
            let lower = msg.to_lowercase();
            for forbidden in ["lost", "gone", "corrupt", "deleted", "destroyed"] {
                assert!(
                    !lower.contains(forbidden),
                    "passkey failure copy says {:?}, which is how a user \
                     concludes their wallet is gone (I12): {}",
                    forbidden,
                    msg
                );
            }
        }

        assert_ne!(
            cancelled, prf,
            "cancel must be distinguishable from failure"
        );
        assert_ne!(cancelled, missing);
        assert_ne!(prf, missing);

        // Cancel is the common case and must not send anyone to their Recovery
        // Kit — nothing is wrong.
        assert!(
            !cancelled.to_lowercase().contains("recovery kit"),
            "a cancelled Touch ID prompt must not read as a recovery event: {}",
            cancelled
        );
        // The other two are genuinely stuck, so they must point somewhere.
        assert!(prf.to_lowercase().contains("recovery kit"));
        assert!(missing.to_lowercase().contains("recovery kit"));
    }
}
