//! Installer step: owner self-recovery via Keychain
//! (PLAN-owner-keychain-recovery PR 3).
//!
//! The owner picks "Recover a Cube I own → with my phone"; that launches the
//! installer with [`UserFlow::RecoverOwnCubeWithPhone`](crate::installer::UserFlow).
//! This step is the **decrypt source** — it fetches the owner's *own* ciphertext
//! envelope set from the gated `recovery-kit/envelope` endpoint, brokers the
//! per-envelope key through the owner's Keychain over the Connect gRPC relay
//! (exactly the heir relay, [`decrypt_envelopes`]), AES-256-GCM-opens locally,
//! and stages the seed/descriptor into the installer `Context`. From there the
//! **existing** restore machinery runs unchanged (the same [`stage_restore`] seam
//! the owner Cube Recovery Kit restore + heir inheritance restore use).
//!
//! It is a near-clone of [`super::inheritance_restore`]: the **only** differences
//! are the fetch endpoint (owner's own `recovery-kit/envelope` instead of the
//! heir's `vault/recovery-envelope`) and the gate matrix ([`OwnerKeychainRecoveryError`]
//! — `403` reads as "not the owner", `423` stays neutral, invariant I3). The
//! decrypt relay, AEAD open, seed-derive, and restore are identical.

use crate::installer::context::RestoreSource;
use coincube_ui::widget::Element;
use iced::Task;
use zeroize::Zeroizing;

use crate::{
    hw::HardwareWallets,
    installer::{
        context::Context,
        message::{Message, OwnerKeychainRestoreMsg},
        step::{
            recovery_kit_restore::{stage_restore, RestoreScope, StagedRestore},
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
        inheritance::{heir::decrypt_envelopes_raw, ArtifactKind},
        recovery::{
            fetch_owner_recovery_envelope, parse_wallet_descriptor_blob,
            OwnerKeychainRecoveryError, RecoveredWalletBlob, SeedBlob, BLOB_VERSION,
        },
    },
};

#[derive(Debug)]
enum Phase {
    /// Fetching the owner's envelope set + brokering the Keychain decrypt relay.
    Decrypting,
    /// Decrypted cleanly; `apply` stages these into `Context` and the flow
    /// auto-advances. Holds the seed only transiently (ZeroizeOnDrop blobs).
    Ready {
        seed: Option<SeedBlob>,
        descriptor: Option<RecoveredWalletBlob>,
    },
    /// Terminal, display-safe copy (the duress 423 never explains why — I3).
    Error { message: String },
}

fn parse_owner_artifacts(
    raw: Vec<(ArtifactKind, Zeroizing<Vec<u8>>)>,
) -> Result<(Option<SeedBlob>, Option<RecoveredWalletBlob>), String> {
    let mut seed = None;
    let mut descriptor = None;
    for (kind, plaintext) in raw {
        match kind {
            ArtifactKind::Seed => {
                if seed.is_some() {
                    return Err("The phone backup contains duplicate seed material.".into());
                }
                let blob: SeedBlob = serde_json::from_slice(&plaintext)
                    .map_err(|e| format!("Seed backup is invalid: {e}"))?;
                if blob.version != BLOB_VERSION {
                    return Err(
                        "This seed backup needs a newer version of COINCUBE Desktop.".into(),
                    );
                }
                seed = Some(blob);
            }
            ArtifactKind::Descriptor => {
                if descriptor.is_some() {
                    return Err("The phone backup contains duplicate wallet material.".into());
                }
                descriptor =
                    Some(parse_wallet_descriptor_blob(&plaintext).map_err(|e| e.to_string())?);
            }
            ArtifactKind::Xpub => {
                return Err("The phone backup contains an unexpected key artifact.".into());
            }
        }
    }
    Ok((seed, descriptor))
}

/// Owner self-recovery decrypt step. Self-contained: it reuses the owner's
/// already-authenticated Connect client threaded into `Context` and builds a
/// short-lived [`GrpcSessionClient`] for the decrypt relay.
pub struct OwnerKeychainRestoreStep {
    scope: RestoreScope,
    /// Connect (server) cube id of the Cube being recovered.
    cube_id: u64,
    /// Authenticated Connect REST client, pulled from `Context` on first load.
    client: Option<CoincubeClient>,
    phase: Phase,
}

impl OwnerKeychainRestoreStep {
    pub fn new(scope: RestoreScope, cube_id: u64) -> Self {
        Self {
            scope,
            cube_id,
            client: None,
            phase: Phase::Decrypting,
        }
    }

    /// The async decrypt: fetch the owner's gated ciphertext, relay-decrypt each
    /// envelope via the owner's Keychain, return the opened blobs. All errors are
    /// mapped to display-safe strings (gate failures via
    /// [`OwnerKeychainRecoveryError`], which neutralises the duress 423).
    fn decrypt_task(client: CoincubeClient, cube_id: u64) -> Task<Message> {
        Task::perform(
            async move {
                let wires = fetch_owner_recovery_envelope(&client, cube_id)
                    .await
                    .map_err(|e| e.to_string())?;
                if wires.is_empty() {
                    return Err(OwnerKeychainRecoveryError::NoEnvelope.to_string());
                }
                let token = client
                    .token()
                    .ok_or_else(|| "Sign in to your account to recover this Cube.".to_string())?
                    .to_string();
                let grpc_url = resolve_connect_grpc_url().await.ok_or_else(|| {
                    "Recovery is unavailable right now. Try again later.".to_string()
                })?;
                let channel = create_channel(&grpc_url)
                    .await
                    .map_err(|e| format!("Couldn't reach Connect: {}", e))?;
                let mut grpc = GrpcSessionClient::new(channel, AuthInterceptor::new(&token));
                let raw = decrypt_envelopes_raw(&mut grpc, cube_id, &wires)
                    .await
                    .map_err(|e| e.to_string())?;
                parse_owner_artifacts(raw)
            },
            |res| Message::OwnerKeychainRestore(OwnerKeychainRestoreMsg::DecryptResult(res)),
        )
    }

    fn apply_recovery_identity_and_session(&self, ctx: &mut Context, seed: Option<&SeedBlob>) {
        // Carry the original Cube identity (UUID + name) out of the decrypted
        // envelope so the post-install `find_or_create_cube` re-mints the same
        // Cube. Reusing the UUID makes the Connect `register_cube` call
        // (idempotent on UUID) reactivate the original Cube instead of creating
        // a duplicate — the same duplicate-Cube bug the owner CRK restore had.
        // Full scope only (a real Cube); Vault-only restores into an existing
        // Cube with its own identity.
        if matches!(self.scope, RestoreScope::Full) {
            if let Some(seed) = seed {
                ctx.cube_id = Some(seed.cube.uuid.clone());
                ctx.cube_name = Some(seed.cube.name.clone());
            }
        }
        // Thread the owner's Connect session into the context so the downstream
        // `CoincubeConnectStep` skips a redundant re-auth and `Final` re-registers
        // the recovered Cube under the owner's account — mirrors the owner CRK
        // restore + heir restore JWT hand-off.
        if let Some(token) = self.client.as_ref().and_then(|c| c.token()) {
            ctx.connect_jwt = Some(Zeroizing::new(token.to_string()));
            ctx.use_coincube_connect = true;
        }
    }
}

impl From<OwnerKeychainRestoreStep> for Box<dyn Step> {
    fn from(s: OwnerKeychainRestoreStep) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl Step for OwnerKeychainRestoreStep {
    fn restore_stage(&self) -> Option<crate::installer::RestoreStage> {
        Some(crate::installer::RestoreStage::Approve)
    }

    /// Pick up the owner's already-authenticated Connect client from `Context`.
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
            None => Task::done(Message::OwnerKeychainRestore(
                OwnerKeychainRestoreMsg::DecryptResult(Err(
                    "Sign in to your account to recover this Cube.".to_string(),
                )),
            )),
        }
    }

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        let Message::OwnerKeychainRestore(OwnerKeychainRestoreMsg::DecryptResult(res)) = message
        else {
            return Task::none();
        };
        match res {
            Ok((seed, descriptor)) => {
                // Verify we got the half this scope needs before advancing (Full
                // needs the seed; DescriptorOnly the descriptor).
                let missing_half = match self.scope {
                    RestoreScope::Full => seed.is_none(),
                    RestoreScope::DescriptorOnly => descriptor.is_none(),
                };
                if missing_half {
                    self.phase = Phase::Error {
                        message: "This Cube isn't set up for the kind of recovery you started."
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
        // A Bitcoin Blake2b Cube and its Bitcoin twin share a seed, so the
        // seed's own Cube must be on the chain being restored.
        if seed
            .as_ref()
            .is_some_and(|seed| seed.cube.network != ctx.bitcoin_config.chain.api_str())
        {
            self.phase = Phase::Error {
                message: "This phone backup belongs to a Cube on another network.".into(),
            };
            return false;
        }
        // Reuse the shared seam: parse descriptor + derive the seed signer (Full
        // only), all-or-nothing.
        let (vault_blob, imported_blob) = match descriptor.as_ref() {
            Some(RecoveredWalletBlob::Vault(vault)) => (Some(vault), None),
            Some(RecoveredWalletBlob::Imported(imported)) => (None, Some(imported.clone())),
            None => (None, None),
        };
        if let Some(imported) = &imported_blob {
            if let Err(message) = imported.validate(ctx.bitcoin_config.chain) {
                self.phase = Phase::Error { message };
                return false;
            }
            if seed.as_ref().is_some_and(|seed| {
                seed.cube.uuid != imported.cube.uuid || seed.cube.network != imported.cube.network
            }) || (matches!(self.scope, RestoreScope::DescriptorOnly)
                && ctx.cube_id.as_deref() != Some(imported.cube.uuid.as_str()))
            {
                self.phase = Phase::Error {
                    message: "This phone backup belongs to a different Cube or chain.".into(),
                };
                return false;
            }
        }
        let staged = match stage_restore(
            ctx.bitcoin_config.network,
            self.scope,
            seed.as_ref(),
            vault_blob,
        ) {
            Ok(staged) => staged,
            Err(message) => {
                self.phase = Phase::Error { message };
                return false;
            }
        };
        staged.commit(ctx, RestoreSource::KeychainSeal);
        ctx.restored_imported_wallet = imported_blob;
        if let Some(imported) = &ctx.restored_imported_wallet {
            ctx.descriptor = None;
            ctx.restored_wallet_birthday = Some(imported.wallet.birthday);
            ctx.wallet_alias = imported.wallet.name.clone();
        }
        self.apply_recovery_identity_and_session(ctx, seed.as_ref());
        true
    }

    fn revert(&self, ctx: &mut Context) {
        if matches!(self.scope, RestoreScope::Full) {
            ctx.recovered_signer = None;
            // Drop the original Cube identity stamped in `apply` (Full scope
            // only) so a later forward pass doesn't re-mint from stale values.
            ctx.cube_id = None;
            ctx.cube_name = None;
        }
        StagedRestore::revert_commit(ctx);
        ctx.restored_imported_wallet = None;
        ctx.connect_jwt = None;
        ctx.use_coincube_connect = false;
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
    ) -> Element<'a, Message> {
        match &self.phase {
            Phase::Decrypting | Phase::Ready { .. } => view::install_titled(
                "Approve on your phone",
                progress,
                true,
                false,
                None,
                Some(
                    "Recovering this Cube — approve the decryption on your phone when prompted…"
                        .to_string(),
                ),
            ),
            Phase::Error { message } => view::install_titled(
                "Approve on your phone",
                progress,
                false,
                false,
                Some(message),
                None,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        chain::ChainId,
        dir::CoincubeDirectory,
        services::recovery::{
            ImportedDescriptorBlob, ImportedDescriptorBlobWallet, SeedBlobCube, SeedBlobMnemonic,
        },
    };
    use coincube_core::{
        miniscript::bitcoin::{
            bip32::{DerivationPath, Xpriv, Xpub},
            secp256k1::Secp256k1,
            Network,
        },
        wallet_descriptor::ImportedWalletDescriptor,
    };
    use std::str::FromStr;

    fn imported_blob() -> ImportedDescriptorBlob {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, &[31; 32]).unwrap();
        let account = master
            .derive_priv(&secp, &DerivationPath::from_str("m/84h/0h/0h").unwrap())
            .unwrap();
        let xpub = Xpub::from_priv(&secp, &account);
        let key = format!("[{}/84h/0h/0h]{xpub}", master.fingerprint(&secp));
        let descriptor = ImportedWalletDescriptor::parse(
            &format!("wpkh({key}/0/*)"),
            Some(&format!("wpkh({key}/1/*)")),
            ChainId::Bitcoin,
        )
        .unwrap();
        ImportedDescriptorBlob::new(
            "restored-cube".into(),
            ChainId::Bitcoin,
            ImportedDescriptorBlobWallet {
                name: "Existing wallet".into(),
                descriptor,
                birthday: 0,
                initial_lookahead: 200,
                receive_index: 45,
                change_index: 32,
            },
        )
        .unwrap()
    }

    fn seed_blob() -> SeedBlob {
        SeedBlob {
            version: BLOB_VERSION,
            cube: SeedBlobCube {
                uuid: "restored-cube".into(),
                name: "Restored Cube".into(),
                network: "mainnet".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                lightning_address: None,
            },
            mnemonic: SeedBlobMnemonic {
                phrase: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".into(),
                language: "en".into(),
            },
        }
    }

    fn context() -> Context {
        Context::new(
            Network::Bitcoin,
            CoincubeDirectory::new(
                std::env::temp_dir().join("owner-imported-restore-context-only"),
            ),
            None,
            None,
        )
    }

    #[test]
    fn owner_phone_parses_imported_v2_without_changing_heir_vault_parser() {
        let imported = imported_blob();
        let seed = seed_blob();
        let raw = vec![
            (
                ArtifactKind::Seed,
                Zeroizing::new(serde_json::to_vec(&seed).unwrap()),
            ),
            (
                ArtifactKind::Descriptor,
                Zeroizing::new(serde_json::to_vec(&imported).unwrap()),
            ),
        ];
        let (parsed_seed, parsed_wallet) = parse_owner_artifacts(raw).unwrap();
        assert_eq!(parsed_seed.unwrap().cube.uuid, "restored-cube");
        let RecoveredWalletBlob::Imported(parsed_wallet) = parsed_wallet.unwrap() else {
            panic!("owner phone restore must retain imported wallet type");
        };
        assert_eq!(parsed_wallet.wallet.receive_index, 45);
        assert_eq!(parsed_wallet.wallet.change_index, 32);
        assert!(
            parse_owner_artifacts(vec![(ArtifactKind::Xpub, Zeroizing::new(vec![1, 2, 3]),)])
                .is_err()
        );
    }

    #[test]
    fn owner_phone_full_and_descriptor_only_restore_stage_exact_imported_wallet() {
        let imported = imported_blob();
        let mut full = OwnerKeychainRestoreStep::new(RestoreScope::Full, 7);
        full.phase = Phase::Ready {
            seed: Some(seed_blob()),
            descriptor: Some(RecoveredWalletBlob::Imported(imported.clone())),
        };
        let mut ctx = context();
        assert!(full.apply(&mut ctx));
        assert_eq!(ctx.cube_id.as_deref(), Some("restored-cube"));
        assert!(ctx.recovered_signer.is_some());
        assert!(ctx.descriptor.is_none());
        assert_eq!(ctx.restore_source, Some(RestoreSource::KeychainSeal));
        assert_eq!(
            ctx.restored_imported_wallet
                .as_ref()
                .unwrap()
                .wallet
                .change_index,
            32
        );
        full.revert(&mut ctx);
        assert!(ctx.restored_imported_wallet.is_none());

        let mut descriptor_only = OwnerKeychainRestoreStep::new(RestoreScope::DescriptorOnly, 7);
        descriptor_only.phase = Phase::Ready {
            seed: None,
            descriptor: Some(RecoveredWalletBlob::Imported(imported.clone())),
        };
        let mut ctx = context();
        ctx.cube_id = Some("wrong-cube".into());
        assert!(!descriptor_only.apply(&mut ctx));
        assert!(ctx.restored_imported_wallet.is_none());
        assert!(matches!(descriptor_only.phase, Phase::Error { .. }));

        // A Cube mismatch ends this restore attempt. A retry enters with a
        // fresh decrypted step after the user selects the matching Cube.
        let mut descriptor_only = OwnerKeychainRestoreStep::new(RestoreScope::DescriptorOnly, 8);
        descriptor_only.phase = Phase::Ready {
            seed: None,
            descriptor: Some(RecoveredWalletBlob::Imported(imported)),
        };
        ctx.cube_id = Some("restored-cube".into());
        assert!(descriptor_only.apply(&mut ctx));
        assert!(ctx.recovered_signer.is_none());
        assert_eq!(
            ctx.restored_imported_wallet
                .as_ref()
                .unwrap()
                .wallet
                .receive_index,
            45
        );
    }
}
