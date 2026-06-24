//! Installer step: heir inheritance recovery (ECIES pivot, COIN-377 PR 3).
//!
//! The heir picks a recoverable Vault on the pre-Cube "Recover a Vault" surface
//! ([`crate::recover_vault`]); that launches the installer with
//! [`UserFlow::RecoverInheritedVault`](crate::installer::UserFlow). This step is
//! the **decrypt source** — it fetches the caller's ciphertext envelope(s) from
//! the gated release endpoint, brokers the per-envelope key through the heir's
//! Keychain over the Connect gRPC relay, AES-256-GCM-opens the envelopes
//! locally, and stages the seed/descriptor into the installer `Context`. From
//! there the **existing** restore machinery runs unchanged (the same
//! [`stage_restore`] seam the owner Cube Recovery Kit restore uses → PIN setup →
//! node → `install_local_wallet`).
//!
//! Crypto: `SPEC-ecies-v1.md` §4 (open) + §4b (key unwrap). The seed exists only
//! transiently here, on the heir's desktop, for the restore — Keychain never
//! returns it, and it never rides a top-level message (the decrypt result stays
//! on this step's redacting [`InheritanceRestoreMsg`]).

use std::sync::Arc;

use coincube_ui::widget::Element;
use iced::Task;
use zeroize::Zeroizing;

use crate::{
    hw::HardwareWallets,
    installer::{
        context::Context,
        message::{InheritanceRestoreMsg, Message},
        step::{
            recovery_kit_restore::{stage_restore, RestoreScope},
            Step,
        },
        view,
    },
    services::{
        coincube::CoincubeClient,
        connect::{
            client::resolve_connect_grpc_url,
            grpc::{create_channel, interceptor::AuthInterceptor, session::GrpcSessionClient},
        },
        inheritance::heir::decrypt_envelopes,
        recovery::{DescriptorBlob, KeyholderRecoveryError, SeedBlob},
    },
};

#[derive(Debug)]
enum Phase {
    /// Fetching the envelope set + brokering the Keychain decrypt relay.
    Decrypting,
    /// Decrypted cleanly; `apply` stages these into `Context` and the flow
    /// auto-advances. Holds the seed only transiently (ZeroizeOnDrop blobs).
    Ready {
        seed: Option<SeedBlob>,
        descriptor: Option<DescriptorBlob>,
    },
    /// Terminal, display-safe copy (gate failures already neutralised; the
    /// duress 423 never explains why — invariant I3).
    Error { message: String },
}

/// Heir inheritance-recovery decrypt step. Self-contained: it reuses the
/// authenticated Connect client threaded into `Context` (the heir is already
/// signed in on the discovery surface) and builds a short-lived
/// [`GrpcSessionClient`] for the decrypt relay.
pub struct InheritanceRestoreStep {
    scope: RestoreScope,
    /// Connect (server) cube id of the Vault being recovered.
    cube_id: u64,
    /// Authenticated Connect REST client, pulled from `Context` on first load.
    client: Option<CoincubeClient>,
    phase: Phase,
}

impl InheritanceRestoreStep {
    pub fn new(scope: RestoreScope, cube_id: u64) -> Self {
        Self {
            scope,
            cube_id,
            client: None,
            phase: Phase::Decrypting,
        }
    }

    /// The async decrypt: fetch the gated ciphertext, relay-decrypt each
    /// envelope via the heir's Keychain, return the opened blobs. All errors
    /// are mapped to display-safe strings (gate failures via
    /// [`KeyholderRecoveryError`], which neutralises the duress 423).
    fn decrypt_task(client: CoincubeClient, cube_id: u64) -> Task<Message> {
        Task::perform(
            async move {
                let wires = client
                    .get_recovery_envelope(cube_id)
                    .await
                    .map_err(|e| KeyholderRecoveryError::from(e).to_string())?;
                if wires.is_empty() {
                    return Err("There's nothing escrowed for you to recover here.".to_string());
                }
                let token = client
                    .token()
                    .ok_or_else(|| "Sign in to your account to recover this Vault.".to_string())?
                    .to_string();
                let grpc_url = resolve_connect_grpc_url().await.ok_or_else(|| {
                    "Recovery is unavailable right now. Try again later.".to_string()
                })?;
                let channel = create_channel(&grpc_url)
                    .await
                    .map_err(|e| format!("Couldn't reach Connect: {}", e))?;
                let mut grpc = GrpcSessionClient::new(channel, AuthInterceptor::new(&token));
                let kit = decrypt_envelopes(&mut grpc, cube_id, &wires)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>((kit.seed, kit.descriptor))
            },
            |res| Message::InheritanceRestore(InheritanceRestoreMsg::DecryptResult(res)),
        )
    }
}

impl From<InheritanceRestoreStep> for Box<dyn Step> {
    fn from(s: InheritanceRestoreStep) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl Step for InheritanceRestoreStep {
    /// Pick up the heir's already-authenticated Connect client from `Context`
    /// (forwarded by the launcher from the discovery surface).
    fn load_context(&mut self, ctx: &Context) {
        if self.client.is_none() {
            self.client.clone_from(&ctx.coincube_client);
        }
    }

    /// Kick off the decrypt as soon as the step becomes active.
    fn load(&self) -> Task<Message> {
        if !matches!(self.phase, Phase::Decrypting) {
            return Task::none();
        }
        match &self.client {
            Some(client) => Self::decrypt_task(client.clone(), self.cube_id),
            None => Task::done(Message::InheritanceRestore(
                InheritanceRestoreMsg::DecryptResult(Err(
                    "Sign in to your account to recover this Vault.".to_string(),
                )),
            )),
        }
    }

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        let Message::InheritanceRestore(InheritanceRestoreMsg::DecryptResult(res)) = message else {
            return Task::none();
        };
        match res {
            Ok((seed, descriptor)) => {
                // Verify we actually got the half this scope needs before we
                // advance (Full needs the seed; DescriptorOnly the descriptor).
                let missing_half = match self.scope {
                    RestoreScope::Full => seed.is_none(),
                    RestoreScope::DescriptorOnly => descriptor.is_none(),
                };
                if missing_half {
                    self.phase = Phase::Error {
                        message: "This Vault isn't set up for the kind of recovery you started."
                            .to_string(),
                    };
                    return Task::none();
                }
                self.phase = Phase::Ready { seed, descriptor };
                // Auto-advance — the decrypt succeeded; no extra click needed.
                Task::done(Message::Next)
            }
            Err(message) => {
                self.phase = Phase::Error { message };
                Task::none()
            }
        }
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        let Phase::Ready { seed, descriptor } = &self.phase else {
            return false;
        };
        // Reuse the shared seam: parse descriptor + derive the seed signer
        // (Full only), all-or-nothing. Borrows of `seed`/`descriptor` end with
        // this call, so the error arm can re-borrow `self` mutably.
        let staged = match stage_restore(
            ctx.bitcoin_config.network,
            self.scope,
            seed.as_ref(),
            descriptor.as_ref(),
        ) {
            Ok(staged) => staged,
            Err(message) => {
                self.phase = Phase::Error { message };
                return false;
            }
        };
        if let Some(d) = staged.descriptor {
            ctx.descriptor = Some(d);
        }
        if let Some(s) = staged.signer {
            ctx.recovered_signer = Some(Arc::new(s));
        }
        // Thread the heir's Connect session into the context so the downstream
        // `CoincubeConnectStep` skips a redundant re-auth and `Final` registers
        // the recovered Cube under the heir's account — mirrors the owner CRK
        // restore's JWT hand-off.
        if let Some(token) = self.client.as_ref().and_then(|c| c.token()) {
            ctx.connect_jwt = Some(Zeroizing::new(token.to_string()));
            ctx.use_coincube_connect = true;
        }
        true
    }

    fn revert(&self, ctx: &mut Context) {
        if matches!(self.scope, RestoreScope::Full) {
            ctx.recovered_signer = None;
        }
        ctx.descriptor = None;
        ctx.connect_jwt = None;
        ctx.use_coincube_connect = false;
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        email: Option<&'a str>,
    ) -> Element<'a, Message> {
        match &self.phase {
            Phase::Decrypting | Phase::Ready { .. } => view::install(
                progress,
                email,
                true,
                false,
                None,
                Some(
                    "Recovering this Vault — approve the decryption on your Keychain when prompted…"
                        .to_string(),
                ),
            ),
            Phase::Error { message } => {
                view::install(progress, email, false, false, Some(message), None)
            }
        }
    }
}
