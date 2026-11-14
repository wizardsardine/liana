use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    app::settings::KeySetting,
    backup::Backup,
    chain::ChainIdExt,
    dir::CoincubeDirectory,
    installer::descriptor::PathKind,
    node::bitcoind::{Bitcoind, InternalBitcoindConfig, NodeFlavor},
    services::coincube::CoincubeClient,
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

/// How an install's descriptor reached this machine.
///
/// All three carry a descriptor with on-chain history, so all three owe a
/// rescan. Only [`Self::PasswordKit`] evidences a password Recovery Kit for
/// *this* Cube — the keychain seal has its own record
/// (`recovery_kit_last_backed_up_keychain_descriptor_fingerprint`), and an
/// heir restoring somebody else's Vault has no kit of their own at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreSource {
    /// The owner's password-protected Cube Recovery Kit.
    PasswordKit,
    /// The owner's Cube, recovered through a paired phone's keychain seal.
    KeychainSeal,
    /// A Vault released to an heir.
    Inheritance,
}

#[derive(Clone)]
pub struct Context {
    #[cfg(feature = "mcp-proposals")]
    pub(crate) mcp_proposal: Option<Arc<super::proposal::Session>>,
    #[cfg(feature = "mcp-proposals")]
    pub(crate) creation_lease: Arc<std::sync::Mutex<Option<super::proposal::Lease>>>,
    pub bitcoin_config: BitcoinConfig,
    pub bitcoin_backend: Option<BitcoinBackend>,
    pub descriptor_template: DescriptorTemplate,
    pub descriptor: Option<CoincubeDescriptor>,
    /// Public imported-wallet metadata recovered from an authenticated Kit.
    /// The Cube seed is retained separately and never becomes an imported signer.
    pub restored_imported_wallet: Option<crate::services::recovery::ImportedDescriptorBlob>,
    pub keys: HashMap<bitcoin::bip32::Fingerprint, KeySetting>,
    /// Whether [`Self::keys`] were built by the descriptor editor, which
    /// knows every key's source. Becomes
    /// [`crate::app::settings::WalletSettings::keychain_keys_recorded`].
    pub keychain_keys_recorded: bool,
    pub hws: Vec<(DeviceKind, bitcoin::bip32::Fingerprint, Option<[u8; 32]>)>,
    pub coincube_directory: CoincubeDirectory,
    pub network: bitcoin::Network,
    pub hw_is_used: bool,
    // In case a user entered a mnemonic,
    // we dont want to override the generated signer with it.
    pub recovered_signer: Option<Arc<Signer>>,
    /// A Vault for a Bitcoin Blake2b Cube that is already open: the Cube and
    /// its master seed exist, and its Vault may run on a local node without
    /// Connect.
    pub in_cube_fork_vault: bool,
    /// How the descriptor being installed was restored, if it was.
    ///
    /// Not derivable from the other fields. A `Full` restore leaves
    /// [`Self::recovered_signer`] set, but a descriptor-only one (a passkey
    /// Cube, or the descriptor half uploaded first) leaves nothing behind but
    /// `descriptor` — which a fresh install has too.
    ///
    /// Deliberately *which* restore, not merely whether one happened. Two
    /// different questions read this, and they do not have the same answer:
    /// every restore owes the new watchonly wallet a rescan, but only one of
    /// them says anything about the Cube owning a password Recovery Kit.
    /// Collapsing the two to a bool made an inherited Vault claim its heir had
    /// a password kit.
    pub restore_source: Option<RestoreSource>,
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
    /// The installer was launched with a signed-in Connect session, which
    /// `CoincubeConnectStep` adopts without showing its sign-in form.
    pub connect_signed_in_at_launch: bool,
    pub install_node_alongside_connect: bool,
    /// Managed-node flavour chosen on the node-management step. Connect-first
    /// installs persist it for background provisioning after the Vault opens;
    /// local-only installs pass it to `InternalBitcoindStep`.
    ///
    /// Authoritative. An existing on-disk `bitcoin.conf` does **not** override
    /// it: the managed-node directory is shared and survives a Cube delete, so a
    /// config left by an earlier install would otherwise silently install Core
    /// for a user who asked for Knots. `InternalBitcoindStep::DefineConfig`
    /// reuses that config's ports and rebuilds the rest on a flavour change.
    pub node_flavor: NodeFlavor,
    pub internal_bitcoind_config: Option<InternalBitcoindConfig>,
    pub internal_bitcoind: Option<Bitcoind>,
    /// Holds the Bitcoind config that will become the primary backend after
    /// sync. Connect-first setup leaves this empty until background install
    /// completes and the running app records the pending node.
    pub pending_bitcoind_config: Option<coincubed::config::BitcoindConfig>,
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
    /// Set for [`super::UserFlow::ClaimBlake2b`]: the Bitcoin Cube this claim
    /// target reuses the descriptor and master seed of.
    ///
    /// Not `cube_settings` — those describe the Cube being built, and a claim
    /// builds a fork Cube from inside a Bitcoin one. See
    /// [`super::claim::target_master_seed`] for the seed decision.
    pub claim_source: Option<super::claim::ClaimSource>,
}

impl Context {
    /// A Bitcoin Blake2b Vault being built here may run on a local node
    /// without Connect: only a Vault for an open Cube.
    pub fn fork_local_vault_allowed(&self) -> bool {
        self.in_cube_fork_vault
    }

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

    /// Re-key the daemon configuration on `network`, keeping the chain identity and the
    /// encoding together. A `BitcoinConfig` must never carry a `network` that is not
    /// `chain.bitcoin_network()`: the daemon refuses such a pair before it touches anything.
    /// So a step that learns the network from a descriptor or a picker must not write the
    /// field on its own — it goes through here, which derives the identity the new encoding
    /// selects (see [`chain_for_network`]) and refuses when there is none.
    pub fn set_bitcoin_network(&mut self, network: bitcoin::Network) -> Result<(), String> {
        let current = self.bitcoin_config.chain;
        let chain = chain_for_network(current, network).ok_or_else(|| {
            format!(
                "{} has no variant that uses the {} network; keep the chain or pick a matching \
                 network.",
                current.label(),
                network
            )
        })?;
        self.bitcoin_config = BitcoinConfig::new(chain, self.bitcoin_config.poll_interval_secs);
        self.network = network;
        Ok(())
    }
}

/// The chain identity that `network` selects for a configuration currently on `current`.
///
/// The encoding alone cannot name a chain — Bitcoin Blake2b encodes exactly like its Bitcoin
/// twin — so the answer depends on where the configuration already is:
/// - the encoding is unchanged: the identity stays what it is (a fork stays a fork);
/// - a Bitcoin-family identity follows the network to the Bitcoin-family identity of that
///   network, which is what the installer's network picker means today;
/// - a fork identity moves only within the fork's own variants (mainnet ↔ testnet4) and has
///   no answer for a network the fork does not exist on. It is never mapped back to Bitcoin.
pub(crate) fn chain_for_network(
    current: crate::chain::ChainId,
    network: bitcoin::Network,
) -> Option<crate::chain::ChainId> {
    use crate::chain::ChainId;
    if current.bitcoin_network() == network {
        return Some(current);
    }
    if !current.is_blake2b() {
        return Some(ChainId::from(network));
    }
    ChainId::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.is_blake2b() && candidate.bitcoin_network() == network)
}

impl Context {
    pub fn new(
        network: bitcoin::Network,
        coincube_directory: CoincubeDirectory,
        cube_settings: Option<&crate::app::settings::CubeSettings>,
        coincube_client: Option<CoincubeClient>,
    ) -> Self {
        Self::new_for_chain(
            network.into(),
            coincube_directory,
            cube_settings,
            coincube_client,
        )
    }

    /// Construct dormant installer state without losing the authenticated chain.
    /// This does not grant permission to start a wallet; runtime boundaries still
    /// require the chain's runtime support and the account's feature flag.
    pub fn new_for_chain(
        chain: crate::chain::ChainId,
        coincube_directory: CoincubeDirectory,
        cube_settings: Option<&crate::app::settings::CubeSettings>,
        coincube_client: Option<CoincubeClient>,
    ) -> Self {
        let network = chain.bitcoin_network();
        Self {
            #[cfg(feature = "mcp-proposals")]
            mcp_proposal: None,
            #[cfg(feature = "mcp-proposals")]
            creation_lease: Arc::new(std::sync::Mutex::new(None)),
            descriptor_template: DescriptorTemplate::default(),
            bitcoin_config: BitcoinConfig::new(chain, Duration::from_secs(10)),
            hws: Vec::new(),
            keys: HashMap::new(),
            keychain_keys_recorded: false,
            bitcoin_backend: None,
            descriptor: None,
            restored_imported_wallet: None,
            coincube_directory,
            network,
            hw_is_used: false,
            recovered_signer: None,
            in_cube_fork_vault: false,
            restore_source: None,
            restored_wallet_birthday: None,
            bitcoind_is_external: true,
            use_coincube_connect: false,
            connect_jwt: None,
            connect_signed_in_at_launch: false,
            install_node_alongside_connect: false,
            node_flavor: if chain.is_blake2b() {
                NodeFlavor::KnotsBlake2b
            } else {
                NodeFlavor::Knots
            },
            internal_bitcoind_config: None,
            internal_bitcoind: None,
            pending_bitcoind_config: None,
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
            claim_source: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Context;
    use crate::dir::CoincubeDirectory;
    use crate::installer::step::Step;
    use coincube_core::miniscript::bitcoin::Network;
    use std::path::PathBuf;
    use std::str::FromStr;

    #[test]
    fn explicit_chain_context_keeps_encoding_and_identity_separate() {
        use crate::chain::ChainId;
        for chain in ChainId::ALL {
            let context =
                Context::new_for_chain(chain, CoincubeDirectory::new(PathBuf::new()), None, None);
            assert_eq!(context.bitcoin_config.chain, chain);
            assert_eq!(context.network, chain.bitcoin_network());
            assert_eq!(context.bitcoin_config.network, chain.bitcoin_network());
            assert_eq!(
                context.node_flavor == crate::node::bitcoind::NodeFlavor::KnotsBlake2b,
                chain.is_blake2b()
            );
            assert_eq!(
                context
                    .coincube_directory
                    .network_directory(context.bitcoin_config.chain)
                    .path(),
                std::path::Path::new(chain.dir_name())
            );
        }
    }

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
            None,
            None,
        )
    }

    /// `set_bitcoin_network` is the only way a step changes the daemon's network, and it
    /// keeps the identity/encoding pair the daemon insists on.
    #[test]
    fn changing_the_network_keeps_chain_and_encoding_together() {
        use super::chain_for_network;
        use crate::chain::ChainId;

        let mut c = ctx();
        assert_eq!(c.bitcoin_config.chain, ChainId::Bitcoin);
        let poll = c.bitcoin_config.poll_interval_secs;
        for (network, chain) in [
            (Network::Signet, ChainId::Signet),
            (Network::Testnet4, ChainId::Testnet4),
            (Network::Regtest, ChainId::Regtest),
            (Network::Testnet, ChainId::Testnet),
            (Network::Bitcoin, ChainId::Bitcoin),
        ] {
            c.set_bitcoin_network(network).unwrap();
            assert_eq!(c.bitcoin_config.chain, chain, "{}", network);
            assert_eq!(c.bitcoin_config.network, network);
            assert_eq!(c.network, network);
            assert_eq!(c.bitcoin_config.poll_interval_secs, poll);
            c.bitcoin_config.check_chain_encoding().unwrap();
        }

        // A fork identity is never mapped back to Bitcoin: unchanged encoding keeps it, the
        // fork's other variant is reachable, and a network the fork does not exist on is a
        // refusal rather than a silent downgrade.
        assert_eq!(
            chain_for_network(ChainId::BitcoinBlake2b, Network::Bitcoin),
            Some(ChainId::BitcoinBlake2b)
        );
        assert_eq!(
            chain_for_network(ChainId::BitcoinBlake2b, Network::Testnet4),
            Some(ChainId::BitcoinBlake2bTestnet4)
        );
        assert_eq!(
            chain_for_network(ChainId::BitcoinBlake2bTestnet4, Network::Bitcoin),
            Some(ChainId::BitcoinBlake2b)
        );
        for network in [Network::Signet, Network::Testnet, Network::Regtest] {
            assert_eq!(
                chain_for_network(ChainId::BitcoinBlake2b, network),
                None,
                "{}",
                network
            );
        }
        let mut fork = ctx();
        fork.bitcoin_config = coincubed::config::BitcoinConfig::new(ChainId::BitcoinBlake2b, poll);
        let err = fork.set_bitcoin_network(Network::Signet).unwrap_err();
        assert!(err.contains("Bitcoin Blake2b"), "{}", err);
        assert_eq!(
            fork.bitcoin_config.chain,
            ChainId::BitcoinBlake2b,
            "left untouched"
        );
        assert_eq!(fork.bitcoin_config.network, Network::Bitcoin);
        fork.set_bitcoin_network(Network::Testnet4).unwrap();
        assert_eq!(fork.bitcoin_config.chain, ChainId::BitcoinBlake2bTestnet4);
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
