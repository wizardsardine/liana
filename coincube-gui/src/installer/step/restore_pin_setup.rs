//! PIN-setup step for the full Recovery Kit restore flow
//! (`UserFlow::RestoreFromRecoveryKit`).
//!
//! # Why this step exists
//!
//! Fresh-install Cubes always persist their master mnemonic in the
//! AES-encrypted, password-protected layout (see `MasterSigner::
//! store_encrypted`). The Liquid / Spark BreezClient then decrypts the
//! blob via the user's PIN at Cube-open time. Before this step
//! existed, the restore flow wrote the restored mnemonic **unencrypted**,
//! then tried to boot the app without constructing a BreezClient at
//! all — the app hung on "Starting daemon…" because the loader's
//! `Synced` branch expects a pre-loaded BreezClient.
//!
//! Inserting this step between `RecoveryKitRestoreStep` and the
//! node-setup steps gives the user an explicit PIN-creation pad and
//! lets `install_local_wallet` store the mnemonic in the same shape a
//! fresh-install Cube uses. Downstream, the tab-level `CubeSaved`
//! handler can load a BreezClient against that blob before handing
//! off to the Loader.
//!
//! # Scope note
//!
//! This step only runs in `RestoreFromRecoveryKit`. The
//! `RestoreVaultFromRecoveryKit` flow (W15) restores a descriptor
//! into an existing Cube whose mnemonic + PIN are already on disk, so
//! re-prompting there would be wrong. The `skip()` hook checks for
//! `ctx.recovered_signer.is_some()` — if there's no new seed to
//! persist, we skip ahead.

use iced::Task;
use zeroize::Zeroizing;

use coincube_ui::widget::*;

use crate::{
    hw::HardwareWallets,
    installer::{
        context::Context,
        message::{Message, PinField, RestorePinSetupMsg},
        step::Step,
        view,
    },
    pin_input::PinInput,
};

pub struct RestorePinSetupStep {
    entry: PinInput,
    confirm: PinInput,
    /// Validation error displayed under the confirm pad — currently
    /// only "PINs do not match", but typed so it's easy to surface
    /// future validations (e.g. "PIN too weak").
    error: Option<&'static str>,
}

impl Default for RestorePinSetupStep {
    fn default() -> Self {
        Self::new()
    }
}

impl RestorePinSetupStep {
    pub fn new() -> Self {
        Self {
            entry: PinInput::new(),
            confirm: PinInput::new(),
            error: None,
        }
    }

    /// Both pads full *and* matching — the only shape that unlocks the
    /// Next button and a viable `apply()`.
    fn is_ready(&self) -> bool {
        self.entry.is_complete()
            && self.confirm.is_complete()
            && self.entry.value() == self.confirm.value()
    }
}

impl Step for RestorePinSetupStep {
    fn skip(&self, ctx: &Context) -> bool {
        // Only run when we actually have a freshly-restored mnemonic
        // to persist. If `recovered_signer` is absent, this is either
        // W15 (descriptor-only restore into an existing Cube) or a
        // misconfigured flow — either way, a PIN prompt doesn't belong.
        ctx.recovered_signer.is_none()
    }

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        let Message::RestorePinSetup(msg) = message else {
            return Task::none();
        };
        match msg {
            RestorePinSetupMsg::Pin(PinField::Entry, pin_msg) => {
                // Reset the mismatch error as soon as the user edits
                // either pad — it'll be recomputed on Submit /
                // is_ready, and leaving it visible while the user is
                // still typing is misleading.
                self.error = None;
                self.entry.update(pin_msg).map(move |m| {
                    Message::RestorePinSetup(RestorePinSetupMsg::Pin(PinField::Entry, m))
                })
            }
            RestorePinSetupMsg::Pin(PinField::Confirm, pin_msg) => {
                self.error = None;
                self.confirm.update(pin_msg).map(move |m| {
                    Message::RestorePinSetup(RestorePinSetupMsg::Pin(PinField::Confirm, m))
                })
            }
            RestorePinSetupMsg::Submit => {
                // The view gates Next on `is_ready`, but handle
                // Submit defensively in case the user hit Enter on a
                // `pin_input` without the pads matching.
                if self.is_ready() {
                    Task::done(Message::Next)
                } else {
                    self.error = Some("PINs do not match.");
                    Task::none()
                }
            }
        }
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        email: Option<&'a str>,
    ) -> Element<'a, Message> {
        view::restore_pin_setup(
            progress,
            email,
            &self.entry,
            &self.confirm,
            self.error,
            self.is_ready(),
        )
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        if !self.is_ready() {
            self.error = Some("PINs do not match.");
            return false;
        }
        // Store the PIN in the Context so `install_local_wallet` can
        // use it as the encryption password for the restored mnemonic
        // and the tab-level `CubeSaved` handler can mint
        // `CubeSettings.security_pin_hash`.
        ctx.restore_pin = Some(Zeroizing::new(self.entry.value()));
        true
    }

    fn revert(&self, ctx: &mut Context) {
        // Keep Context in a consistent state if the user navigates
        // back past this step — otherwise a later step might still
        // see a stale PIN from a prior pass of the wizard.
        ctx.restore_pin = None;
    }
}

impl From<RestorePinSetupStep> for Box<dyn Step> {
    fn from(s: RestorePinSetupStep) -> Box<dyn Step> {
        Box::new(s)
    }
}
