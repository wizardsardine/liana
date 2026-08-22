use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    app::settings::KeySetting,
    backup::Backup,
    dir::CoincubeDirectory,
    installer::descriptor::PathKind,
    node::bitcoind::{Bitcoind, InternalBitcoindConfig, NodeFlavor},
    services::{
        coincube::CoincubeClient,
        connect::client::backend::{BackendClient, BackendWalletClient},
    },
    signer::Signer,
};
use async_hwi::DeviceKind;
use coincube_core::{
    descriptors::CoincubeDescriptor,
    miniscript::bitcoin::{self, bip32::Fingerprint},
};
use coincubed::config::{BitcoinBackend, BitcoinConfig};

/// One backend `ConnectVaultMember` row to create after the descriptor
/// is installed. Only keychain-sourced descriptor keys (W8 / W3) produce
/// a payload — hardware-wallet, xpub-entered, master-signer and
/// token-sourced keys are intentionally skipped (per
/// `plans/PLAN-cube-membership-desktop.md` design decision,
/// 2026-04-18: "only keychain-sourced keys become VaultMember rows").
#[derive(Debug, Clone)]
pub struct ConnectVaultMemberPayload {
    pub fingerprint: Fingerprint,
    /// Backend `keys.id` captured when the user selected this key in the
    /// Vault Builder picker.
    pub key_id: u64,
    /// Populated when the key belongs to a contact-Keyholder, `None` when
    /// the key belongs to the vault owner themselves.
    pub contact_id: Option<u64>,
    /// Path the key participates in. Carried through for future role
    /// inference; all members currently default to `Keyholder` per the
    /// 2026-04-18 plan direction.
    pub path_kind: PathKind,
}

#[derive(Debug, Clone)]
pub enum RemoteBackend {
    Undefined,
    None,
    // The installer will have to create a wallet from the created descriptor.
    WithoutWallet(BackendClient),
    // The installer will have to fetch the wallet and only install the missing configuration files.
    WithWallet(BackendWalletClient),
}

impl RemoteBackend {
    pub fn user_email(&self) -> Option<&str> {
        match self {
            Self::WithWallet(b) => Some(b.user_email()),
            Self::WithoutWallet(b) => Some(b.user_email()),
            _ => None,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, RemoteBackend::None)
    }
    pub fn is_some(&self) -> bool {
        matches!(
            self,
            RemoteBackend::WithoutWallet { .. } | RemoteBackend::WithWallet { .. }
        )
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorTemplate {
    #[default]
    SimpleInheritance,
    Custom,
    MultisigSecurity,
    /// 2-of-3 multisig primary spend + a simple timelocked inheritance key.
    TwoOfThreeInheritance,
    /// 2-of-3 multisig primary spend + an inheritance path + a second
    /// (later-timelocked) recovery path.
    MultisigInheritanceRecovery,
    /// Like [`Self::MultisigInheritanceRecovery`], but the primary keys are
    /// reused in the inheritance path (2-of-6 over primary + inheritance
    /// keys), plus a second (later-timelocked) recovery path.
    ExpandingMultisigInheritanceRecovery,
}

#[derive(Clone)]
pub struct Context {
    pub bitcoin_config: BitcoinConfig,
    pub bitcoin_backend: Option<BitcoinBackend>,
    pub descriptor_template: DescriptorTemplate,
    pub descriptor: Option<CoincubeDescriptor>,
    pub keys: HashMap<bitcoin::bip32::Fingerprint, KeySetting>,
    pub hws: Vec<(DeviceKind, bitcoin::bip32::Fingerprint, Option<[u8; 32]>)>,
    pub coincube_directory: CoincubeDirectory,
    pub network: bitcoin::Network,
    pub hw_is_used: bool,
    // In case a user entered a mnemonic,
    // we dont want to override the generated signer with it.
    pub recovered_signer: Option<Arc<Signer>>,
    /// Whether the descriptor being installed came out of a **Recovery Kit**.
    ///
    /// Not derivable from the other fields. A `Full` kit leaves
    /// [`Self::recovered_signer`] set, but a descriptor-only kit (a passkey
    /// Cube, or the descriptor half uploaded first) leaves nothing behind but
    /// `descriptor` — which a fresh install has too. The distinction decides
    /// whether the new watchonly wallet owes a rescan, so it is recorded
    /// rather than inferred.
    pub restored_from_kit: bool,
    /// The restored Vault's creation time, when the Recovery Kit recorded one
    /// ([`crate::services::recovery::plaintext::DescriptorBlobVault::birthday`]).
    ///
    /// Turns the rescan a restore owes from a question into something the app
    /// starts on its own. `None` for kits written before the field existed.
    pub restored_wallet_birthday: Option<u32>,
    pub bitcoind_is_external: bool,
    pub use_coincube_connect: bool,
    /// Connect JWT threaded across installer steps. Wrapped in
    /// `Zeroizing<String>` so the heap allocation is scrubbed when the
    /// `Context` (and any `Task::perform` clone of it) drops — keeps
    /// the token off the residual heap between the step that writes it
    /// (`CoincubeConnectStep` or `RecoveryKitRestoreStep`) and the
    /// downstream step that copies it into `EsploraConfig.token`.
    pub connect_jwt: Option<zeroize::Zeroizing<String>>,
    pub install_node_alongside_connect: bool,
    /// Managed-node flavour chosen on the node-management step, carried to the
    /// `InternalBitcoindStep` that actually downloads/configures it. Defaults to
    /// Knots; the user can switch to Core on that step.
    ///
    /// Authoritative. An existing on-disk `bitcoin.conf` does **not** override
    /// it: the managed-node directory is shared and survives a Cube delete, so a
    /// config left by an earlier install would otherwise silently install Core
    /// for a user who asked for Knots. `InternalBitcoindStep::DefineConfig`
    /// reuses that config's ports and rebuilds the rest on a flavour change.
    pub node_flavor: NodeFlavor,
    pub internal_bitcoind_config: Option<InternalBitcoindConfig>,
    pub internal_bitcoind: Option<Bitcoind>,
    /// Set when `install_node_alongside_connect` is true; holds the Bitcoind
    /// config that will become the primary backend once IBD completes.
    pub pending_bitcoind_config: Option<coincubed::config::BitcoindConfig>,
    pub remote_backend: RemoteBackend,
    pub backup: Option<Backup>,
    pub wallet_alias: String,
    /// Cube UUID (from CubeSettings.id) — present when the Vault installer
    /// is launched from inside a Cube.  Used by the key picker to fetch
    /// Cube-scoped Keychain keys from the API.
    pub cube_id: Option<String>,
    /// Authenticated coincube-api client, used by the key picker to
    /// fetch Cube-scoped Keychain keys.  `None` when launched from
    /// the Loader (user hasn't done coincube-api auth yet).
    pub coincube_client: Option<CoincubeClient>,
    /// The Cube's Connect-blinding encryption key (`SPEC-cube-xpub-envelope-v1`
    /// §3), derived in `Installer::new` from the master signer the Cube was
    /// unlocked with. The Vault builder needs it to open the xpub envelopes
    /// Connect now serves in place of plaintext keys (PR D3).
    ///
    /// `None` when the installer is running without an unlocked Cube seed —
    /// a fresh install, or a watch-only restore. Blinded keys then surface as
    /// "needs re-sharing"/locked in the picker rather than being selectable
    /// with a key we can't read.
    pub cube_encryption_key:
        Option<std::sync::Arc<crate::services::connect::crypto::CubeEncryptionKey>>,
    /// Cube display name used when idempotently registering the cube
    /// with the backend during Final. `None` when no cube settings
    /// were passed in.
    pub cube_name: Option<String>,
    /// Vault members to fan out to the backend after the local install
    /// completes. Populated by `DefineDescriptor::apply()` for every
    /// keychain-sourced descriptor key. Empty when no such keys exist
    /// in the descriptor.
    pub connect_vault_members: Vec<ConnectVaultMemberPayload>,
    /// Approximate timelock (in days) used for the backend vault's
    /// `timelockDays` field. Derived from the longest Recovery path's
    /// `PathSequence::Recovery(blocks)` via `max(blocks / 144, 1)` —
    /// inherently approximate because block cadence varies. Surfaced
    /// with an "approximate" caveat in the Final step's success caption.
    /// `None` when the descriptor has no recovery paths.
    pub connect_vault_timelock_days: Option<i32>,
    /// PIN chosen by the user during a Recovery Kit restore. Populated
    /// by `RestorePinSetupStep` (between `RecoveryKitRestoreStep` and
    /// the node-setup step in `UserFlow::RestoreFromRecoveryKit`).
    ///
    /// Downstream consumers:
    /// - `install_local_wallet` branches on this to call
    ///   `Signer::store_encrypted(..., &pin)` rather than the
    ///   unencrypted `store(...)` so the Liquid/Spark BreezClient can
    ///   decrypt the mnemonic on subsequent Cube opens.
    /// - `gui::tab::find_or_create_cube` / the `CubeSaved` handler use
    ///   the value to populate `CubeSettings.security_pin_hash` and
    ///   `CubeSettings.master_signer_fingerprint`, matching what a
    ///   fresh-install Cube stores.
    ///
    /// Wrapped in `Zeroizing<String>` so the heap allocation is zeroed
    /// when the `Context` clone held by `Task::perform` drops after
    /// the install completes. `None` for non-restore flows.
    pub restore_pin: Option<zeroize::Zeroizing<String>>,
    /// The PIN under which this Cube's seed material is encrypted on disk.
    ///
    /// Every mnemonic the installer writes goes through
    /// `Signer::store_encrypted`, which requires a PIN — there is no
    /// plaintext branch any more (I5). So the installer has to be told the
    /// Cube's PIN, and where it comes from depends on the flow:
    ///
    /// - launched from inside an open Cube (`SetupVault`, loader vault setup):
    ///   the live session PIN, captured at unlock
    ///   ([`crate::app::session`]);
    /// - Recovery-Kit restore: [`Self::restore_pin`], chosen on
    ///   `RestorePinSetupStep` — a Cube that does not exist yet has no session.
    ///
    /// `None` is not by itself a failure any more: a **passkey** Cube has no
    /// PIN and supplies [`Self::passkey_seed_password`] instead. It is a
    /// failure when that is `None` too, and the install then fails loudly at
    /// the point it would have had to write a seed. That is deliberate: the
    /// alternative this replaces was writing the seed in the clear.
    pub cube_pin: Option<zeroize::Zeroizing<String>>,
    /// The seed-file password for a **passkey** Cube, derived from the master
    /// seed the unlock assertion produced
    /// ([`crate::services::passkey::seed_password`]).
    ///
    /// A passkey Cube has no PIN, so [`Self::cube_pin`] is `None` for it by
    /// design — and before this existed, "Set up a Vault" inside one died at
    /// the seed write with "the Cube's PIN isn't available in this session".
    /// The Cube's root secret was in hand the whole time; it just was not a
    /// PIN.
    ///
    /// Populated in [`super::Installer::new`] from
    /// [`crate::app::session::seed_file_password`] — the same resolver the read
    /// side ([`crate::app::wallet::Wallet::load_hotsigners`]) uses, so what the
    /// installer encrypts under is by construction what a later load decrypts
    /// with. `None` for every PIN Cube, and for a passkey Cube whose session
    /// holds no master signer — which fails the same loud way rather than
    /// inventing a password no later unlock could rederive.
    pub passkey_seed_password: Option<zeroize::Zeroizing<String>>,
}

impl Context {
    /// The password to encrypt seed material under, preferring the restore PIN
    /// the user just chose over a session PIN inherited from another Cube, and
    /// falling back to the passkey Cube's seed-derived password.
    ///
    /// The order matters. A restore always writes files the *restored* Cube
    /// must open, so its freshly-chosen PIN wins outright. The passkey password
    /// comes last because it only ever exists when the other two cannot: it is
    /// set only for a passkey Cube, which has no PIN of either kind.
    pub fn seed_password(&self) -> Option<&zeroize::Zeroizing<String>> {
        self.restore_pin
            .as_ref()
            .or(self.cube_pin.as_ref())
            .or(self.passkey_seed_password.as_ref())
    }

    /// The Cube id to bind seed files to. `""` when the Cube has not been
    /// minted yet — such files stay readable once it is (see
    /// [`coincube_core::seed_crypt`]).
    pub fn seed_cube_id(&self) -> &str {
        self.cube_id.as_deref().unwrap_or("")
    }

    /// Whether this install ends with a Vault — a native-Bitcoin wallet that
    /// needs a chain backend to watch.
    ///
    /// The descriptor *is* the Vault: [`Message::Install`](super::Message)
    /// branches on exactly this to choose between creating a wallet and
    /// `persist_seed_only_install`. So when it is `None`, everything the
    /// node/backend steps configure is written and then never read.
    ///
    /// Safe to ask at any step that runs after the descriptor is settled, which
    /// is every flow's node and alias steps: the fresh-install flows fix it in
    /// `DefineDescriptor`/`ImportDescriptor`, and the restore flows in their
    /// respective restore step — all of which precede them. Only the "Full"
    /// restores can reach those steps with `None`, which is precisely the
    /// seed-only (Cube, no Vault) kit this exists to detect.
    pub fn installs_vault(&self) -> bool {
        self.descriptor.is_some()
    }
}

impl Context {
    pub fn new(
        network: bitcoin::Network,
        coincube_directory: CoincubeDirectory,
        remote_backend: RemoteBackend,
        cube_settings: Option<&crate::app::settings::CubeSettings>,
        coincube_client: Option<CoincubeClient>,
    ) -> Self {
        Self {
            descriptor_template: DescriptorTemplate::default(),
            bitcoin_config: BitcoinConfig {
                network,
                poll_interval_secs: Duration::from_secs(10),
            },
            hws: Vec::new(),
            keys: HashMap::new(),
            bitcoin_backend: None,
            descriptor: None,
            coincube_directory,
            network,
            hw_is_used: false,
            recovered_signer: None,
            restored_from_kit: false,
            restored_wallet_birthday: None,
            bitcoind_is_external: true,
            use_coincube_connect: false,
            connect_jwt: None,
            install_node_alongside_connect: false,
            node_flavor: NodeFlavor::Knots,
            internal_bitcoind_config: None,
            internal_bitcoind: None,
            pending_bitcoind_config: None,
            remote_backend,
            wallet_alias: String::new(),
            backup: None,
            cube_id: cube_settings.map(|cs| cs.id.clone()),
            coincube_client,
            cube_encryption_key: None,
            cube_name: cube_settings.map(|cs| cs.name.clone()),
            connect_vault_members: Vec::new(),
            connect_vault_timelock_days: None,
            restore_pin: None,
            // Filled in by `Installer::new` from the live session when the
            // installer is launched from inside an open Cube.
            cube_pin: None,
            passkey_seed_password: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Context, RemoteBackend};
    use crate::dir::CoincubeDirectory;
    use crate::installer::step::Step;
    use coincube_core::miniscript::bitcoin::Network;
    use std::path::PathBuf;
    use std::str::FromStr;

    /// Compile-time pin: if anyone reverts `Context.connect_jwt` to a
    /// plain `Option<String>`, this type-level assertion stops
    /// compiling. Keeps the JWT's scrub-on-drop guarantee intact
    /// across the installer → Esplora handoff.
    #[test]
    fn connect_jwt_is_zeroizing_wrapped() {
        #[allow(dead_code)]
        const _: fn(&Context) -> Option<&zeroize::Zeroizing<String>> =
            |ctx| ctx.connect_jwt.as_ref();
    }

    fn ctx() -> Context {
        Context::new(
            Network::Bitcoin,
            CoincubeDirectory::new(PathBuf::new()),
            RemoteBackend::None,
            None,
            None,
        )
    }

    fn with_vault() -> Context {
        let mut c = ctx();
        c.descriptor = Some(
            coincube_core::descriptors::CoincubeDescriptor::from_str(
                "wsh(or_d(pk([8a550171/48'/1'/0'/2']tpubDFnCs5ZaCqopaNhgLCiXAwbkaBdcnuMt1VFoPs\
                 RpUrpidyvzG67MYjkfxw6HnTBhHqeU3xw2ioNBVcWY3jXwGhSyppEQvtn38GsL7RH1eef/<0;1>/*),\
                 and_v(v:pkh([8a550171/48'/1'/0'/2']tpubDFnCs5ZaCqopaNhgLCiXAwbkaBdcnuMt1VFoPsRp\
                 UrpidyvzG67MYjkfxw6HnTBhHqeU3xw2ioNBVcWY3jXwGhSyppEQvtn38GsL7RH1eef/<2;3>/*),\
                 older(52596))))#jz5sm0xn",
            )
            .expect("fixture descriptor must parse"),
        );
        c
    }

    /// A passkey Cube reaches the seed write with no PIN of either kind. Before
    /// `passkey_seed_password` existed, `seed_password()` answered `None` there
    /// and "Set up a Vault" died on the installer's last step.
    #[test]
    fn a_passkey_cube_encrypts_seeds_under_its_derived_password() {
        let mut c = ctx();
        assert!(
            c.seed_password().is_none(),
            "no credentials of any kind is still a hard failure"
        );

        c.passkey_seed_password = Some(zeroize::Zeroizing::new("derived".into()));
        assert_eq!(c.seed_password().map(|p| p.as_str()), Some("derived"));
    }

    /// Ordering, not just presence. The passkey password is last because it can
    /// only exist where neither PIN does — but if a flow ever set both, a
    /// restore's freshly chosen PIN is the one the restored Cube will unlock
    /// with, so it has to win.
    #[test]
    fn the_pins_outrank_the_passkey_password() {
        let mut c = ctx();
        c.passkey_seed_password = Some(zeroize::Zeroizing::new("derived".into()));
        c.cube_pin = Some(zeroize::Zeroizing::new("session".into()));
        assert_eq!(c.seed_password().map(|p| p.as_str()), Some("session"));

        c.restore_pin = Some(zeroize::Zeroizing::new("restore".into()));
        assert_eq!(c.seed_password().map(|p| p.as_str()), Some("restore"));
    }

    #[test]
    fn installs_vault_follows_the_descriptor() {
        assert!(
            !ctx().installs_vault(),
            "no descriptor means no Vault — the same rule Message::Install uses \
             to pick persist_seed_only_install"
        );
        assert!(with_vault().installs_vault());
    }

    /// The reported bug: restoring a seed-only (Cube, no Vault) Recovery Kit
    /// walked the user through choosing a Bitcoin node. The node steps
    /// configure a chain backend for a Vault that this install will never
    /// create — `Message::Install` takes the seed-only branch and reads none of
    /// it. Every one of them must drop out.
    #[test]
    fn a_seed_only_restore_skips_every_vault_only_step() {
        use crate::installer::step::{
            DefineNode, InternalBitcoindStep, SelectBitcoindTypeStep, WalletAlias,
        };

        let seed_only = ctx();
        let dir = CoincubeDirectory::new(PathBuf::new());

        let steps: Vec<(&str, Box<dyn Step>)> = vec![
            ("SelectBitcoindType", SelectBitcoindTypeStep::new().into()),
            ("InternalBitcoind", InternalBitcoindStep::new(&dir).into()),
            (
                "DefineNode",
                DefineNode::new(crate::node::NodeType::Esplora).into(),
            ),
            ("WalletAlias", WalletAlias::default().into()),
        ];
        for (name, step) in &steps {
            assert!(
                step.skip(&seed_only),
                "{} must be skipped when the restore produced no descriptor",
                name
            );
        }
    }

    /// ...and the same steps must still run for a Vault, or this would quietly
    /// strand every real wallet without a chain backend.
    ///
    /// `InternalBitcoindStep` is asserted separately: a default context has
    /// `bitcoind_is_external = true`, so it skips for its own long-standing
    /// reason. What matters is that the new Vault condition is not what's
    /// deciding — flipping the flag brings it back.
    #[test]
    fn a_vault_install_still_gets_its_node_steps() {
        use crate::installer::step::{
            DefineNode, InternalBitcoindStep, SelectBitcoindTypeStep, WalletAlias,
        };

        let vault = with_vault();
        let dir = CoincubeDirectory::new(PathBuf::new());

        let select: Box<dyn Step> = SelectBitcoindTypeStep::new().into();
        assert!(!select.skip(&vault), "SelectBitcoindType must still run");
        let define: Box<dyn Step> = DefineNode::new(crate::node::NodeType::Esplora).into();
        assert!(!define.skip(&vault), "DefineNode must still run");
        let alias: Box<dyn Step> = WalletAlias::default().into();
        assert!(!alias.skip(&vault), "WalletAlias must still run");

        let mut managed_node = with_vault();
        managed_node.bitcoind_is_external = false;
        let internal: Box<dyn Step> = InternalBitcoindStep::new(&dir).into();
        assert!(
            !internal.skip(&managed_node),
            "InternalBitcoind must still run for a Vault on a managed node"
        );
    }
}
