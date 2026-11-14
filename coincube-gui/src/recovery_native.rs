//! Opt-in native product integration. This is never GUI or funded-spend evidence.
//! The signed process runner owns admission, process boundaries and private storage.
use crate::{
    app::{
        settings::{self, CubeSettings, VaultIdentity, WalletId},
        wallet::Wallet,
    },
    chain::ChainId,
    dir::CoincubeDirectory,
    installer::{self, integration::Context},
    services::{
        coincube::{CoincubeClient, RecoveryKit, RegisterCubeRequest},
        recovery::{self, SeedBlobCube},
        unlock::{self, device_secret},
    },
    signer::Signer,
};
use coincube_core::{
    descriptors::{CoincubeDescriptor, CoincubePolicy, PathInfo},
    miniscript::{
        bitcoin::{self, bip32::ChildNumber, secp256k1::Secp256k1},
        descriptor::DescriptorPublicKey,
    },
};
use std::{
    collections::BTreeMap,
    str::FromStr,
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, &'static str>;

pub fn allowed_chain(chain: ChainId) -> Result<()> {
    if matches!(chain, ChainId::Testnet4 | ChainId::BitcoinBlake2bTestnet4) {
        Ok(())
    } else {
        Err("native_chain_forbidden")
    }
}

fn check(ok: bool, code: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(code)
    }
}

pub(crate) fn initialize_preferences(root: &CoincubeDirectory, chain: ChainId) -> Result<()> {
    use crate::app::settings::global::GlobalSettings;
    allowed_chain(chain)?;
    let path = GlobalSettings::path(root);
    GlobalSettings::update_developer_mode(&path, true).map_err(|_| "native_preferences")?;
    if chain.is_blake2b() {
        GlobalSettings::update_bitcoin_blake2b_beta(&path, true)
            .map_err(|_| "native_preferences")?;
    }
    Ok(())
}

pub(crate) async fn require_backend_feature(
    chain: ChainId,
    client: &CoincubeClient,
    root: &CoincubeDirectory,
) -> Result<()> {
    allowed_chain(chain)?;
    if chain.is_blake2b() {
        crate::chain::require_connect_feature(chain, client, root)
            .await
            .map_err(|_| "native_chain_capability_blocked")?;
    }
    Ok(())
}

/// Public identity is retained privately; never print its descriptor/xpubs.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub cube: CubeSettings,
    pub descriptor: String,
    pub root_xpub: String,
    pub fingerprint: String,
    pub remote_cube_id: u64,
    pub owner_id: u32,
}

fn identity_matches(
    seed: &recovery::SeedBlob,
    desc: &recovery::DescriptorBlob,
    identity: &Identity,
) -> Result<()> {
    check(
        seed.cube.uuid == identity.cube.id
            && desc.cube.uuid == identity.cube.id
            && seed.cube.network == identity.cube.network.api_str()
            && desc.cube.network == seed.cube.network
            && desc.vault.descriptor == identity.descriptor,
        "native_kit_identity",
    )
}

/// Same native installer as the UI, with real backend admission. No mock daemon.
#[allow(clippy::too_many_arguments)]
async fn install(
    root: &CoincubeDirectory,
    cube: &mut CubeSettings,
    descriptor: CoincubeDescriptor,
    signer: Signer,
    jwt: &str,
    client: CoincubeClient,
    restored: Option<(&recovery::SeedBlob, &recovery::DescriptorBlob)>,
    pin: &str,
) -> Result<()> {
    let chain = cube.network;
    let mut ctx = Context::new_for_chain(chain, root.clone(), Some(cube), Some(client));
    ctx.descriptor = Some(descriptor.clone());
    ctx.wallet_alias = cube.name.clone();
    ctx.cube_pin = Some(Zeroizing::new(pin.into()));
    ctx.use_coincube_connect = true;
    ctx.bitcoin_backend = Some(coincubed::config::BitcoinBackend::Esplora(
        installer::integration::recovery_esplora_config(chain, jwt),
    ));
    if let Some((seed, desc)) = restored {
        ctx.restore_pin = Some(Zeroizing::new(pin.into()));
        installer::integration::stage_full_recovery(&mut ctx, seed, desc)
            .map_err(|_| "native_restore_staging")?;
    }
    let wallet_id = WalletId::generate(&descriptor);
    installer::install_local_wallet(ctx, wallet_id.clone(), Arc::new(Mutex::new(signer)))
        .await
        .map_err(|_| "native_installer_blocked")?;
    cube.set_vault(VaultIdentity::new(wallet_id, Some(&descriptor)));
    let saved = cube.clone();
    settings::update_settings_file(&root.network_directory(chain), move |mut settings| {
        settings.cubes.retain(|c| c.id != saved.id);
        settings.cubes.push(saved);
        Some(settings)
    })
    .await
    .map_err(|_| "native_settings_write")?;
    Ok(())
}

/// Read-only admission, before generating any fixture credential or canary.
/// Repeated immediately before product creation to close stale quota evidence.
pub async fn create_capacity_available(
    chain: ChainId,
    id: uuid::Uuid,
    owner_id: u32,
    client: &CoincubeClient,
) -> Result<bool> {
    allowed_chain(chain)?;
    check(
        client
            .get_user()
            .await
            .map_err(|_| "native_auth_blocked")?
            .id
            == owner_id,
        "native_account_changed",
    )?;
    if chain.is_blake2b() {
        check(
            client
                .get_connect_features()
                .await
                .map_err(|_| "native_chain_capability_blocked")?
                .bitcoin_blake2b_enabled
                == Some(true),
            "native_chain_capability_blocked",
        )?;
    }
    check(
        !client
            .list_cubes()
            .await
            .map_err(|_| "native_remote_inventory_blocked")?
            .iter()
            .any(|cube| cube.uuid == id.to_string()),
        "native_remote_uuid_already_exists",
    )?;
    let limits = client
        .get_cube_limits(chain.api_str())
        .await
        .map_err(|_| "native_limits_blocked")?;
    check(
        limits.network == chain.api_str() && limits.current_count >= 0,
        "native_limits_blocked",
    )?;
    Ok(limits.current_count < limits.max_allowed as i64)
}

pub async fn admit_create(
    chain: ChainId,
    id: uuid::Uuid,
    owner_id: u32,
    client: &CoincubeClient,
) -> Result<()> {
    check(
        create_capacity_available(chain, id, owner_id, client).await?,
        "native_capacity_blocked",
    )
}

/// Drive Home's real creation/backup ceremony, then real in-Cube Vault installation
/// and the exact full-kit encrypt/upload function shared by Settings and creation.
#[allow(clippy::too_many_arguments)]
pub async fn create_export(
    chain: ChainId,
    root: &CoincubeDirectory,
    id: uuid::Uuid,
    pin: &str,
    words: &[String],
    password: &Zeroizing<String>,
    jwt: &str,
    client: CoincubeClient,
) -> Result<(Identity, RecoveryKit)> {
    allowed_chain(chain)?;
    let user = client.get_user().await.map_err(|_| "native_auth_blocked")?;
    if chain.is_blake2b() {
        check(
            client
                .get_connect_features()
                .await
                .map_err(|_| "native_chain_capability_blocked")?
                .bitcoin_blake2b_enabled
                == Some(true),
            "native_chain_capability_blocked",
        )?;
    }
    check(
        !client
            .list_cubes()
            .await
            .map_err(|_| "native_remote_inventory_blocked")?
            .iter()
            .any(|cube| cube.uuid == id.to_string()),
        "native_remote_uuid_already_exists",
    )?;
    let limits = client
        .get_cube_limits(chain.api_str())
        .await
        .map_err(|_| "native_limits_blocked")?;
    check(
        limits.network == chain.api_str() && limits.current_count < limits.max_allowed as i64,
        "native_capacity_blocked",
    )?;
    let mut cube = crate::home::integration::create_recovery_fixture_cube(
        chain,
        root.clone(),
        id,
        pin,
        words,
        limits.max_allowed,
        client.clone(),
    )
    .await?;
    let location = unlock::CubeLocation::new(root.path(), &cube);
    let master =
        match unlock_admitted(&location, chain, pin).map_err(|_| "native_original_unlock")? {
            unlock::PinOutcome::Unlock(signer) => signer,
            _ => return Err("native_original_unlock"),
        };
    let secp = Secp256k1::new();
    let fingerprint = master.fingerprint(&secp).to_string();
    let root_xpub = master
        .xpub_at(&bitcoin::bip32::DerivationPath::master(), &secp)
        .to_string();
    let account =
        bitcoin::bip32::DerivationPath::from_str("m/48'/1'/0'/2'").map_err(|_| "native_path")?;
    let xpub = master.xpub_at(&account, &secp);
    let primary =
        DescriptorPublicKey::from_str(&format!("[{fingerprint}/48h/1h/0h/2h]{xpub}/<0;1>/*"))
            .map_err(|_| "native_descriptor")?;
    // A real single-hot-primary Vault; recovery branch is the same owned root at
    // disjoint paths. No other user's key and no phone signer is involved.
    let recovery_key =
        DescriptorPublicKey::from_str(&format!("[{fingerprint}/48h/1h/0h/2h]{xpub}/<2;3>/*"))
            .map_err(|_| "native_descriptor")?;
    let descriptor = CoincubeDescriptor::new(
        CoincubePolicy::new_legacy(
            PathInfo::Single(primary),
            BTreeMap::from([(144, PathInfo::Single(recovery_key))]),
        )
        .map_err(|_| "native_policy")?,
    );
    let remote = client
        .register_cube(RegisterCubeRequest {
            uuid: cube.id.clone(),
            name: cube.name.clone(),
            network: chain.api_str().into(),
            has_vault: None,
            spark_stable_balance: None,
        })
        .await
        .map_err(|_| "native_registration_uncertain")?;
    check(
        remote.uuid == cube.id && remote.network == chain.api_str(),
        "native_remote_identity",
    )?;
    install(
        root,
        &mut cube,
        descriptor.clone(),
        Signer::new(*master),
        jwt,
        client.clone(),
        None,
        pin,
    )
    .await?;
    let settings = settings::Settings::from_file(&root.network_directory(chain))
        .map_err(|_| "native_settings_read")?;
    let installed = settings
        .wallets
        .iter()
        .find(|w| Some(w.wallet_id()) == cube.vault_wallet_id)
        .ok_or("native_installed_wallet_missing")?;
    let wallet = Wallet::new(descriptor.clone())
        .with_chain(chain)
        .load_from_settings(installed.clone())
        .map_err(|_| "native_installed_wallet_invalid")?;
    let blob = crate::app::state::settings::recovery_kit::descriptor_blob_from_wallet(
        &wallet,
        &cube.id,
        chain.api_str(),
    )
    .ok_or("native_descriptor_blob")?;
    crate::app::state::settings::recovery_kit::encrypt_and_upload(
        client.clone(),
        remote.id,
        Some(Zeroizing::new(words.to_vec())),
        Some(blob),
        SeedBlobCube {
            uuid: cube.id.clone(),
            name: cube.name.clone(),
            network: chain.api_str().into(),
            created_at: chrono::DateTime::<chrono::Utc>::from_timestamp(cube.created_at, 0)
                .ok_or("native_cube_timestamp")?
                .to_rfc3339(),
            lightning_address: None,
        },
        password.clone(),
    )
    .await
    .map_err(|_| "native_upload_uncertain")?;
    let identity = Identity {
        cube,
        descriptor: descriptor.to_string(),
        root_xpub,
        fingerprint,
        remote_cube_id: remote.id,
        owner_id: user.id,
    };
    let kit = client
        .download_recovery_kit(remote.id, password)
        .await
        .map_err(|_| "native_kit_readback_blocked")?;
    validate_kit(&kit, password, &identity)?;
    check(
        client
            .get_user()
            .await
            .map_err(|_| "native_auth_blocked")?
            .id
            == identity.owner_id,
        "native_account_changed",
    )?;
    Ok((identity, kit))
}

pub fn validate_kit(
    kit: &RecoveryKit,
    password: &Zeroizing<String>,
    identity: &Identity,
) -> Result<()> {
    check(
        !kit.encrypted_cube_seed.is_empty() && !kit.encrypted_wallet_descriptor.is_empty(),
        "native_partial_kit",
    )?;
    let aad = identity.remote_cube_id.to_string();
    let seed = recovery::decrypt_seed_blob(&kit.encrypted_cube_seed, password, &aad)
        .map_err(|_| "native_kit_seed")?;
    let desc = recovery::decrypt_descriptor_blob(&kit.encrypted_wallet_descriptor, password, &aad)
        .map_err(|_| "native_kit_descriptor")?;
    identity_matches(&seed, &desc, identity)?;
    let signer = coincube_core::signer::MasterSigner::from_str(
        identity.cube.network.bitcoin_network(),
        &seed.mnemonic.phrase,
    )
    .map_err(|_| "native_kit_seed")?;
    check(
        signer
            .xpub_at(&bitcoin::bip32::DerivationPath::master(), &Secp256k1::new())
            .to_string()
            == identity.root_xpub,
        "native_kit_root",
    )
}

/// Restore receives only authenticated ciphertext and its recovery password,
/// not the original phrase/factor/PIN. Product staging derives the restored key.
pub async fn restore(
    root: &CoincubeDirectory,
    identity: &Identity,
    kit: &RecoveryKit,
    password: &Zeroizing<String>,
    new_pin: &str,
    jwt: &str,
    client: CoincubeClient,
) -> Result<()> {
    allowed_chain(identity.cube.network)?;
    check(
        !root.path().exists()
            && device_secret::load_optional(&identity.cube.id)
                .map_err(|_| "native_keystore_blocked")?
                .is_none(),
        "native_restore_not_fresh",
    )?;
    check(
        client
            .get_user()
            .await
            .map_err(|_| "native_auth_blocked")?
            .id
            == identity.owner_id,
        "native_account_changed",
    )?;
    validate_kit(kit, password, identity)?;
    let aad = identity.remote_cube_id.to_string();
    let seed = recovery::decrypt_seed_blob(&kit.encrypted_cube_seed, password, &aad)
        .map_err(|_| "native_kit_seed")?;
    let desc = recovery::decrypt_descriptor_blob(&kit.encrypted_wallet_descriptor, password, &aad)
        .map_err(|_| "native_kit_descriptor")?;
    let mut cube = identity.cube.clone();
    cube.vault_wallet_id = None;
    cube.vault_fingerprint = None;
    // Match the installer: the builder signer is unrelated; only the product
    // full-kit staging supplies ctx.recovered_signer for restored persistence.
    let signer =
        Signer::generate(cube.network.bitcoin_network()).map_err(|_| "native_builder_signer")?;
    let descriptor = desc
        .vault
        .descriptor
        .parse::<CoincubeDescriptor>()
        .map_err(|_| "native_descriptor")?;
    tokio::fs::create_dir_all(root.path())
        .await
        .map_err(|_| "native_profile_create")?;
    initialize_preferences(root, cube.network)?;
    require_backend_feature(cube.network, &client, root).await?;
    device_secret::get_or_create(root.path(), &cube.id).map_err(|_| "native_keystore_blocked")?;
    install(
        root,
        &mut cube,
        descriptor,
        signer,
        jwt,
        client,
        Some((&seed, &desc)),
        new_pin,
    )
    .await?;
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct UnfundedProof {
    pub version: u8,
    pub chain: ChainId,
    pub descriptor_sha256: String,
    pub observed_at: u32,
    pub last_poll_timestamp: u32,
    pub full_scan_completed_at: u32,
    pub full_scan_requested_at: Option<u32>,
    pub full_scan_completed_generation: u64,
    pub daemon_started_at: u32,
    pub addresses_checked: u64,
    pub known_coin_count: u64,
}
#[cfg(unix)]
fn oracle_diagnostic(
    path: &std::path::Path,
    info: &coincubed::commands::GetInfoResult,
    requested: Option<u32>,
) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::{fs::OpenOptions, io::Write};
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "native_oracle_diagnostic")?;
    let metadata = file.metadata().map_err(|_| "native_oracle_diagnostic")?;
    check(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.permissions().mode() & 0o077 == 0
            && metadata.len() < 1_000_000,
        "native_oracle_diagnostic",
    )?;
    let row = serde_json::json!({"version":1,"observed_at":chrono::Utc::now().timestamp(),
        "chain":info.chain_id,"block_height":info.block_height,"sync":info.sync,
        "full_scan_requested_at":requested,"full_scan_in_progress":info.history_sync.full_scan_in_progress,
        "running_addresses_checked":info.history_sync.addresses_checked,
        "running_addresses_expected":info.history_sync.addresses_expected,
        "full_scan_completed_at":info.history_sync.full_scan_completed_at,
        "full_scan_generation":info.history_sync.full_scan_generation,
        "full_scan_completed_generation":info.history_sync.full_scan_completed_generation,
        "last_poll_timestamp":info.last_poll_timestamp,"failure_present":info.history_sync.last_failure.is_some(),
        "rescan_progress":info.rescan_progress,"chain_divergence":info.chain_divergence,
        "refused_reorg_depth":info.refused_reorg_depth});
    writeln!(file, "{}", row).map_err(|_| "native_oracle_diagnostic")?;
    file.sync_all().map_err(|_| "native_oracle_diagnostic")
}

#[cfg(not(unix))]
fn oracle_diagnostic(
    _path: &std::path::Path,
    _info: &coincubed::commands::GetInfoResult,
    _requested: Option<u32>,
) -> Result<()> {
    // No equivalent secure diagnostic writer is implemented on this platform.
    Err("native_oracle_diagnostic")
}

fn oracle_ready(
    info: &coincubed::commands::GetInfoResult,
    identity: &Identity,
    started: u32,
) -> Result<bool> {
    check(
        info.chain_id == Some(identity.cube.network)
            && info.network == identity.cube.network.bitcoin_network()
            && info.descriptors.main.to_string() == identity.descriptor,
        "native_oracle_chain_descriptor",
    )?;
    Ok(info.sync.is_finite()
        && info.sync >= 1.0
        && info.refused_reorg_depth.is_none()
        && !info.chain_divergence
        && info.rescan_progress.is_none()
        && info.history_sync.last_failure.is_none()
        && !info.history_sync.full_scan_in_progress
        && info.history_sync.full_scan_completed_generation > 0
        && info.history_sync.full_scan_completed_generation
            == info.history_sync.full_scan_generation
        && info.last_poll_timestamp.is_some_and(|t| t >= started)
        && info
            .history_sync
            .full_scan_completed_at
            .is_some_and(|t| t >= started))
}

fn saved_vault_config(
    root: &CoincubeDirectory,
    identity: &Identity,
) -> Result<(CubeSettings, coincubed::config::Config)> {
    let chain = identity.cube.network;
    let disk = settings::Settings::from_file(&root.network_directory(chain))
        .map_err(|_| "native_settings_read")?;
    let cube = disk
        .cubes
        .iter()
        .find(|c| c.id == identity.cube.id && c.network == chain)
        .ok_or("native_cube_missing")?;
    let id = cube
        .vault_wallet_id
        .as_ref()
        .ok_or("native_persisted_vault_missing")?;
    let directory = root
        .network_directory(chain)
        .coincubed_data_directory(id)
        .path()
        .to_owned();
    let cfg = coincubed::config::Config::from_file(Some(directory.join("daemon.toml")))
        .map_err(|_| "native_persisted_config")?;
    check(
        cfg.bitcoin_config.chain == chain
            && cfg.main_descriptor.to_string() == identity.descriptor
            && cfg
                .data_directory()
                .is_some_and(|d| d.path() == directory.as_path()),
        "native_persisted_descriptor_changed",
    )?;
    Ok((cube.clone(), cfg))
}

/// Positive exact-chain product wallet baseline before any factor/profile wipe.
/// Unknown backend/sync/coin state blocks; ordinary Bitcoin is never a B2 oracle.
pub async fn require_unfunded(
    root: &CoincubeDirectory,
    identity: &Identity,
    client: CoincubeClient,
    diagnostic: &std::path::Path,
) -> Result<UnfundedProof> {
    let chain = identity.cube.network;
    allowed_chain(chain)?;
    check(
        client
            .get_user()
            .await
            .map_err(|_| "native_auth_blocked")?
            .id
            == identity.owner_id,
        "native_account_changed",
    )?;
    let (_, mut cfg) = saved_vault_config(root, identity)?;
    cfg.bitcoin_config.poll_interval_secs = std::time::Duration::from_secs(1); // in-memory integration retry only
    require_unfunded_config(cfg, identity, client, diagnostic).await
}

/// New watch-only oracle: only the validated full kit's public descriptor enters
/// this initially absent data directory. No old database, seed or factor is used.
pub async fn descriptor_only_unfunded(
    directory: &std::path::Path,
    identity: &Identity,
    kit: &RecoveryKit,
    password: &Zeroizing<String>,
    client: CoincubeClient,
    jwt: &str,
    diagnostic: &std::path::Path,
) -> Result<UnfundedProof> {
    check(
        identity.cube.network == ChainId::Testnet4
            && tokio::fs::symlink_metadata(directory)
                .await
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "native_orphan_oracle_scope",
    )?;
    validate_kit(kit, password, identity)?;
    check(
        client
            .get_user()
            .await
            .map_err(|_| "native_auth_blocked")?
            .id
            == identity.owner_id,
        "native_account_changed",
    )?;
    let desc = recovery::decrypt_descriptor_blob(
        &kit.encrypted_wallet_descriptor,
        password,
        &identity.remote_cube_id.to_string(),
    )
    .map_err(|_| "native_kit_descriptor")?;
    let descriptor =
        CoincubeDescriptor::from_str(&desc.vault.descriptor).map_err(|_| "native_descriptor")?;
    check(
        descriptor.to_string() == identity.descriptor,
        "native_orphan_oracle_scope",
    )?;
    tokio::fs::create_dir(directory)
        .await
        .map_err(|_| "native_orphan_oracle_scope")?;
    check(
        tokio::fs::canonicalize(directory)
            .await
            .is_ok_and(|canonical| canonical == directory),
        "native_orphan_oracle_scope",
    )?;
    let cfg = coincubed::config::Config::new(
        coincubed::config::BitcoinConfig::new(
            identity.cube.network,
            std::time::Duration::from_secs(1),
        ),
        Some(coincubed::config::BitcoinBackend::Esplora(
            installer::integration::recovery_esplora_config(identity.cube.network, jwt),
        )),
        log::LevelFilter::Off,
        descriptor,
        coincubed::datadir::DataDirectory::new(directory.to_owned()),
    );
    require_unfunded_config(cfg, identity, client, diagnostic).await
}

async fn require_unfunded_config(
    cfg: coincubed::config::Config,
    identity: &Identity,
    client: CoincubeClient,
    diagnostic: &std::path::Path,
) -> Result<UnfundedProof> {
    use crate::daemon::{embedded::EmbeddedDaemon, Daemon};
    let chain = identity.cube.network;
    let started = chrono::Utc::now().timestamp() as u32;
    let daemon = if chain.is_blake2b() {
        EmbeddedDaemon::start_authenticated(cfg, client)
            .await
            .map_err(|_| "native_unfunded_backend_blocked")?
    } else {
        tokio::task::spawn_blocking(move || EmbeddedDaemon::start(cfg))
            .await
            .map_err(|_| "native_unfunded_backend_blocked")?
            .map_err(|_| "native_unfunded_backend_blocked")?
    };
    let result = async {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(540);
        let mut requested = None;
        let mut after_generation = None;
        loop {
            let info = daemon
                .get_info()
                .await
                .map_err(|_| "native_unfunded_oracle_blocked")?;
            let ready = oracle_ready(&info, identity, requested.unwrap_or(started))?;
            oracle_diagnostic(diagnostic, &info, requested)?;
            if after_generation.is_none() && info.history_sync.full_scan_generation > 0 {
                // This cache was created with the new daemon: a startup full
                // scan is genuine fresh work, not persisted historical state.
                after_generation = Some(0);
            }
            if after_generation.is_none()
                && info.sync.is_finite()
                && info.sync >= 1.0
                && info.last_poll_timestamp.is_some_and(|t| t >= started)
                && info.rescan_progress.is_none()
                && !info.history_sync.full_scan_in_progress
                && info.history_sync.last_failure.is_none()
                && info.refused_reorg_depth.is_none()
                && !info.chain_divergence
            {
                let at = chrono::Utc::now().timestamp() as u32;
                let bounds = daemon
                    .integration_rescan_time_bounds()
                    .await
                    .map_err(|_| "native_unfunded_rescan_bounds")?;
                check(
                    bounds.chain == chain && bounds.tip_timestamp > bounds.genesis_timestamp,
                    "native_unfunded_rescan_bounds",
                )?;
                // Full history from the ACTUAL admitted chain, never projected
                // Bitcoin genesis or a fabricated wallet birthday.
                daemon
                    .start_rescan(bounds.genesis_timestamp)
                    .await
                    .map_err(|_| "native_unfunded_rescan_blocked")?;
                daemon
                    .request_sync()
                    .await
                    .map_err(|_| "native_unfunded_rescan_blocked")?;
                requested = Some(at);
                after_generation = Some(info.history_sync.full_scan_generation);
                continue;
            }
            if ready
                && after_generation.is_some_and(|generation| {
                    info.history_sync.full_scan_completed_generation > generation
                })
            {
                let coins = daemon
                    .list_coins(&[], &[])
                    .await
                    .map_err(|_| "native_unfunded_oracle_blocked")?;
                check(coins.coins.is_empty(), "native_fixture_funded")?;
                let fresh = daemon
                    .get_info()
                    .await
                    .map_err(|_| "native_unfunded_oracle_blocked")?;
                check(
                    oracle_ready(&fresh, identity, requested.unwrap_or(started))?
                        && after_generation.is_some_and(|generation| {
                            fresh.history_sync.full_scan_completed_generation > generation
                        }),
                    "native_unfunded_history_changed",
                )?;
                use bitcoin::hashes::{sha256, Hash};
                return Ok(UnfundedProof {
                    version: 2,
                    chain,
                    descriptor_sha256: sha256::Hash::hash(identity.descriptor.as_bytes())
                        .to_string(),
                    observed_at: chrono::Utc::now().timestamp() as u32,
                    last_poll_timestamp: fresh
                        .last_poll_timestamp
                        .ok_or("native_oracle_poll_unknown")?,
                    full_scan_completed_at: fresh
                        .history_sync
                        .full_scan_completed_at
                        .ok_or("native_oracle_history_unknown")?,
                    full_scan_requested_at: requested,
                    full_scan_completed_generation: fresh
                        .history_sync
                        .full_scan_completed_generation,
                    daemon_started_at: started,
                    addresses_checked: fresh.history_sync.addresses_checked,
                    known_coin_count: 0,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("native_unfunded_history_unknown");
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    .await;
    let stopped = daemon
        .stop()
        .await
        .map_err(|_| "native_oracle_shutdown_blocked");
    let proof = result?;
    stopped?;
    Ok(proof)
}

fn unlock_admitted(
    location: &unlock::CubeLocation<'_>,
    chain: ChainId,
    pin: &str,
) -> std::result::Result<unlock::PinOutcome, unlock::UnlockError> {
    if chain == ChainId::BitcoinBlake2bTestnet4 {
        unlock::unlock_connect_fork(location, pin)
    } else {
        unlock::unlock_blocking(location, pin)
    }
}

/// Cold native product loaders. Wrong PIN is tested before any correct unlock.
/// The runner must invoke this in a different process from both create/restore.
pub async fn cold_load(
    root: &CoincubeDirectory,
    identity: &Identity,
    new_pin: &str,
    wrong_pin: &str,
    client: CoincubeClient,
) -> Result<Wallet> {
    check(new_pin != wrong_pin, "native_wrong_pin_equal")?;
    let chain = identity.cube.network;
    allowed_chain(chain)?;
    check(!crate::app::session::is_open(), "native_session_not_cold")?;
    let (cube, cfg) = saved_vault_config(root, identity)?;
    check(
        client
            .get_user()
            .await
            .map_err(|_| "native_auth_blocked")?
            .id
            == identity.owner_id,
        "native_account_changed",
    )?;
    require_backend_feature(chain, &client, root).await?;
    let location = unlock::CubeLocation::new(root.path(), &cube);
    check(
        matches!(
            unlock_admitted(&location, chain, wrong_pin),
            Ok(unlock::PinOutcome::Wrong)
        ),
        "native_wrong_pin_accepted",
    )?;
    let fingerprint = bitcoin::bip32::Fingerprint::from_str(&identity.fingerprint)
        .map_err(|_| "native_fingerprint")?;
    if chain == ChainId::BitcoinBlake2bTestnet4 {
        let signer = match unlock_admitted(&location, chain, new_pin)
            .map_err(|_| "native_cold_fork_unlock")?
        {
            unlock::PinOutcome::Unlock(signer) => signer,
            _ => return Err("native_cold_fork_unlock"),
        };
        check(
            signer
                .xpub_at(&bitcoin::bip32::DerivationPath::master(), &Secp256k1::new())
                .to_string()
                == identity.root_xpub,
            "native_cold_root",
        )?;
    }
    if chain == ChainId::Testnet4 {
        let signer = crate::app::seed_source::SeedSource::encrypted_file(fingerprint, new_pin)
            .resolve(root.path(), chain.bitcoin_network(), &cube.id)
            .map_err(|_| "native_cold_seed_source_blocked")?;
        check(
            signer
                .xpub_at(&bitcoin::bip32::DerivationPath::master(), &Secp256k1::new())
                .to_string()
                == identity.root_xpub,
            "native_cold_root",
        )?;
    }
    let mut disk_wallet = Wallet::new(cfg.main_descriptor);
    disk_wallet.chain = chain;
    let wallet = disk_wallet
        .load_hotsigners(root, chain, &cube.id, Some(new_pin))
        .map_err(|_| "native_hot_loader")?;
    check(
        wallet.signer.as_ref().is_some_and(|s| {
            s.get_extended_pubkey(&bitcoin::bip32::DerivationPath::master())
                .to_string()
                == identity.root_xpub
        }),
        "native_hot_root",
    )?;
    Ok(wallet)
}

/// Explicitly unfunded key-control PSBT. Never admitted as a real wallet coin,
/// never broadcast. Uses the actual product chain-specific hot-signing function.
pub fn sign_unfunded(wallet: &Wallet) -> Result<bitcoin::psbt::Psbt> {
    use bitcoin::{
        absolute, transaction, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut,
        Witness,
    };
    let descriptor = wallet
        .main_descriptor
        .as_vault()
        .ok_or("native_not_vault")?;
    let secp = Secp256k1::new();
    let index = ChildNumber::from_normal_idx(0).map_err(|_| "native_path")?;
    let receive = descriptor.receive_descriptor().derive(index, &secp);
    let change = descriptor.change_descriptor().derive(index, &secp);
    let parent = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(100000),
            script_pubkey: receive.script_pubkey(),
        }],
    };
    let tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(parent.compute_txid(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(99738),
            script_pubkey: change.script_pubkey(),
        }],
    };
    let mut psbt = bitcoin::psbt::Psbt::from_unsigned_tx(tx).map_err(|_| "native_psbt")?;
    receive.update_psbt_in(&mut psbt.inputs[0]);
    psbt.inputs[0].witness_utxo = Some(parent.output[0].clone());
    psbt.inputs[0].non_witness_utxo = Some(parent);
    crate::app::state::vault::psbt::sign_recovery_fixture_for_chain(
        wallet.chain,
        wallet.signer.as_ref().ok_or("native_no_hot_signer")?,
        psbt,
    )
    .map_err(|_| "native_signing")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Existing public test descriptor; no signer, daemon, account or key store.
    fn oracle_diagnostic_info() -> coincubed::commands::GetInfoResult {
        let descriptor = CoincubeDescriptor::from_str("wsh(or_d(pk([8a550171/48'/1'/0'/2']tpubDFnCs5ZaCqopaNhgLCiXAwbkaBdcnuMt1VFoPsRpUrpidyvzG67MYjkfxw6HnTBhHqeU3xw2ioNBVcWY3jXwGhSyppEQvtn38GsL7RH1eef/<0;1>/*),and_v(v:pkh([8a550171/48'/1'/0'/2']tpubDFnCs5ZaCqopaNhgLCiXAwbkaBdcnuMt1VFoPsRpUrpidyvzG67MYjkfxw6HnTBhHqeU3xw2ioNBVcWY3jXwGhSyppEQvtn38GsL7RH1eef/<2;3>/*),older(52596))))#jz5sm0xn").unwrap();
        coincubed::commands::GetInfoResult {
            version: "diagnostic-fixture-version".into(),
            network: ChainId::Testnet4.bitcoin_network(),
            chain_id: Some(ChainId::Testnet4),
            block_height: 100,
            sync: 1.0,
            descriptors: coincubed::commands::GetInfoDescriptors {
                main: descriptor.into(),
            },
            imported_wallet: None,
            rescan_progress: None,
            refused_reorg_depth: None,
            chain_divergence: false,
            timestamp: 100,
            last_poll_timestamp: Some(100),
            history_sync: Default::default(),
            receive_index: 0,
            change_index: 0,
        }
    }

    #[cfg(unix)]
    #[test]
    fn oracle_diagnostic_private_file_appends_private_rows() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("diagnostic.jsonl");
        let info = oracle_diagnostic_info();
        oracle_diagnostic(&path, &info, Some(123)).unwrap();
        oracle_diagnostic(&path, &info, None).unwrap();
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert!(metadata.is_file());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        let text = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["version"], 1);
        assert_eq!(rows[0]["block_height"], 100);
        assert_eq!(
            rows[0]["chain"],
            serde_json::to_value(info.chain_id).unwrap()
        );
        assert_eq!(rows[0]["full_scan_requested_at"], 123);
        assert!(rows[1]["full_scan_requested_at"].is_null());
        assert!(!text.contains("diagnostic-fixture-version"));
        assert!(!text.contains("tpub"));
    }

    #[cfg(unix)]
    #[test]
    fn oracle_diagnostic_public_permissions_refused_without_write() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("public.jsonl");
        let sentinel = b"retain diagnostic sentinel\n";
        std::fs::write(&path, sentinel).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            oracle_diagnostic(&path, &oracle_diagnostic_info(), None),
            Err("native_oracle_diagnostic")
        );
        assert_eq!(std::fs::read(&path).unwrap(), sentinel);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[cfg(unix)]
    #[test]
    fn oracle_diagnostic_symlink_refused_without_write() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.jsonl");
        let link = directory.path().join("alias.jsonl");
        let sentinel = b"retain symlink target sentinel\n";
        std::fs::write(&target, sentinel).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &link).unwrap();
        assert_eq!(
            oracle_diagnostic(&link, &oracle_diagnostic_info(), None),
            Err("native_oracle_diagnostic")
        );
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        assert_eq!(std::fs::read(&target).unwrap(), sentinel);
    }

    #[cfg(not(unix))]
    #[test]
    fn oracle_diagnostic_unsupported_platform_refuses_without_output() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("diagnostic.jsonl");
        let info = oracle_diagnostic_info();
        assert_eq!(
            oracle_diagnostic(&path, &info, Some(123)),
            Err("native_oracle_diagnostic")
        );
        assert_eq!(
            std::fs::symlink_metadata(&path).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        let sentinel = b"retain unsupported-platform sentinel\n";
        std::fs::write(&path, sentinel).unwrap();
        assert_eq!(
            oracle_diagnostic(&path, &info, None),
            Err("native_oracle_diagnostic")
        );
        assert_eq!(std::fs::read(&path).unwrap(), sentinel);
    }

    fn fixture(chain: ChainId) -> (Wallet, Identity) {
        let signer = Signer::generate(chain.bitcoin_network()).unwrap();
        let fp = signer.fingerprint();
        let root_xpub = signer
            .get_extended_pubkey(&bitcoin::bip32::DerivationPath::master())
            .to_string();
        let account = bitcoin::bip32::DerivationPath::from_str("m/48'/1'/0'/2'").unwrap();
        let xpub = signer.get_extended_pubkey(&account);
        let key = |branches: &str| {
            DescriptorPublicKey::from_str(&format!("[{fp}/48h/1h/0h/2h]{xpub}/<{branches}>/*"))
                .unwrap()
        };
        let descriptor = CoincubeDescriptor::new(
            CoincubePolicy::new_legacy(
                PathInfo::Single(key("0;1")),
                BTreeMap::from([(144, PathInfo::Single(key("2;3")))]),
            )
            .unwrap(),
        );
        let id = uuid::Uuid::new_v4();
        let cube =
            CubeSettings::new_with_raw_id(id.to_string(), "Native offline selftest".into(), chain);
        let identity = Identity {
            cube,
            descriptor: descriptor.to_string(),
            root_xpub,
            fingerprint: fp.to_string(),
            remote_cube_id: 1,
            owner_id: 1,
        };
        (
            Wallet::new(descriptor)
                .with_chain(chain)
                .with_signer(signer),
            identity,
        )
    }
    #[test]
    fn unfunded_product_signing_uses_real_chain_domains() {
        for chain in [ChainId::Testnet4, ChainId::BitcoinBlake2bTestnet4] {
            let (wallet, identity) = fixture(chain);
            let psbt = sign_unfunded(&wallet).unwrap();
            if chain.is_blake2b() {
                assert!(psbt.inputs[0].partial_sigs.is_empty());
                assert_eq!(psbt.inputs[0].proprietary.len(), 2);
                assert_eq!(
                    psbt.inputs[0].proprietary.values().next().unwrap().last(),
                    Some(&0x21)
                );
            } else {
                assert_eq!(psbt.inputs[0].partial_sigs.len(), 2);
                assert!(psbt.inputs[0].proprietary.is_empty());
            }
            // Optional PUBLIC-only export lets the independent Python verifier
            // consume actual product signing bytes without any private key.
            if let Ok(path) = std::env::var("COINCUBE_NATIVE_PUBLIC_SELFTEST_OUTPUT") {
                let path = std::path::PathBuf::from(path);
                std::fs::create_dir_all(&path).unwrap();
                std::fs::write(
                    path.join(format!("{}.psbt", chain.api_str())),
                    psbt.serialize(),
                )
                .unwrap();
                std::fs::write(
                    path.join(format!("{}.json", chain.api_str())),
                    serde_json::to_vec(&identity).unwrap(),
                )
                .unwrap();
            }
        }
    }
    #[test]
    fn admitted_fork_unlock_tests_wrong_and_correct_pin_without_changing_generic_gate() {
        let temp = tempfile::tempdir().unwrap();
        let chain = ChainId::BitcoinBlake2bTestnet4;
        let secp = Secp256k1::new();
        let signer =
            coincube_core::signer::MasterSigner::generate(chain.bitcoin_network()).unwrap();
        let fp = signer.fingerprint(&secp);
        let id = uuid::Uuid::new_v4().to_string();
        std::fs::create_dir_all(temp.path().join(chain.dir_name())).unwrap();
        signer
            .store_encrypted_for_chain(temp.path(), chain, &secp, None, "1234", &id, None)
            .unwrap();
        let mut cube = CubeSettings::new_with_raw_id(id, "Offline fork routing".into(), chain);
        cube.master_signer_fingerprint = Some(fp);
        let location = unlock::CubeLocation::new(temp.path(), &cube);
        assert!(matches!(
            unlock::unlock_blocking(&location, "1234"),
            Err(unlock::UnlockError::ChainUnavailable(_))
        ));
        assert!(matches!(
            unlock_admitted(&location, chain, "9999"),
            Ok(unlock::PinOutcome::Wrong)
        ));
        let opened = match unlock_admitted(&location, chain, "1234").unwrap() {
            unlock::PinOutcome::Unlock(value) => value,
            _ => panic!("correct PIN refused"),
        };
        assert_eq!(
            opened.xpub_at(&bitcoin::bip32::DerivationPath::master(), &secp),
            signer.xpub_at(&bitcoin::bip32::DerivationPath::master(), &secp)
        );
    }

    #[test]
    fn production_networks_are_refused() {
        for chain in ChainId::ALL {
            assert_eq!(
                allowed_chain(chain).is_ok(),
                matches!(chain, ChainId::Testnet4 | ChainId::BitcoinBlake2bTestnet4)
            );
        }
    }
    #[tokio::test]
    async fn cold_loader_refuses_missing_or_changed_persisted_vault_before_keystore() {
        let temp = tempfile::tempdir().unwrap();
        let root = CoincubeDirectory::new(temp.path().to_owned());
        let (_, identity) = fixture(ChainId::Testnet4);
        assert!(matches!(
            cold_load(&root, &identity, "1234", "0000", CoincubeClient::new()).await,
            Err("native_settings_read")
        ));
        // No fabricated descriptor supplied by the expected-identity record can
        // substitute for an absent on-disk product installation.
    }
    #[test]
    fn chain_sync_without_current_complete_wallet_history_never_proves_zero() {
        use coincubed::commands::{GetInfoDescriptors, GetInfoResult};
        let (wallet, identity) = fixture(ChainId::BitcoinBlake2bTestnet4);
        let mut info = GetInfoResult {
            version: "native test".into(),
            network: identity.cube.network.bitcoin_network(),
            chain_id: Some(identity.cube.network),
            block_height: 100,
            sync: 1.0,
            descriptors: GetInfoDescriptors {
                main: wallet.main_descriptor.clone(),
            },
            imported_wallet: None,
            rescan_progress: None,
            refused_reorg_depth: None,
            chain_divergence: false,
            timestamp: 100,
            last_poll_timestamp: Some(100),
            history_sync: Default::default(),
            receive_index: 0,
            change_index: 0,
        };
        let cache = coincubed::commands::HistorySyncCache::default();
        cache.begin_full_scan(40);
        cache.branch_address_checked(false);
        cache.branch_address_checked(true);
        assert!(cache.snapshot().full_scan_in_progress);
        cache.full_scan_fetched();
        assert!(cache.snapshot().full_scan_completed_at.is_none());
        cache.poll_succeeded(100);
        info.history_sync = cache.snapshot();
        assert_eq!(info.history_sync.addresses_checked, 0);
        assert!(oracle_ready(&info, &identity, 100).unwrap());
        for case in 0..8 {
            let mut bad = info.clone();
            match case {
                0 => bad.history_sync.full_scan_completed_at = None,
                1 => bad.history_sync.full_scan_in_progress = true,
                2 => {
                    bad.history_sync.last_failure=serde_json::from_value(serde_json::json!({"message":"offline","since":100,"last_at":100,"consecutive":1})).unwrap();
                }
                3 => bad.last_poll_timestamp = None,
                4 => bad.history_sync.full_scan_completed_at = Some(99),
                5 => bad.rescan_progress = Some(1.0),
                6 => bad.sync = f64::NAN,
                _ => bad.chain_divergence = true,
            };
            assert!(!oracle_ready(&bad, &identity, 100).unwrap());
        }
        let mut wrong = info.clone();
        wrong.chain_id = Some(ChainId::Testnet4);
        assert!(oracle_ready(&wrong, &identity, 100).is_err());
    }
    #[tokio::test]
    async fn backend_gate_skips_bitcoin_but_requires_saved_beta_before_blake2b_api() {
        use httpmock::{Method::GET, MockServer};
        let server = MockServer::start();
        let flag=server.mock(|when,then|{when.method(GET).path("/api/v1/connect/features");then.status(200).json_body(serde_json::json!({"success":true,"data":{"plans":[],"bitcoinBlake2bEnabled":true}}));});
        let temp = tempfile::tempdir().unwrap();
        let root = CoincubeDirectory::new(temp.path().to_owned());
        let mut client = CoincubeClient::for_test(server.base_url());
        client.set_token("offline-test-only");
        require_backend_feature(ChainId::Testnet4, &client, &root)
            .await
            .unwrap();
        assert_eq!(flag.hits(), 0);
        assert!(
            require_backend_feature(ChainId::BitcoinBlake2bTestnet4, &client, &root)
                .await
                .is_err()
        );
        assert_eq!(flag.hits(), 0);
        initialize_preferences(&root, ChainId::BitcoinBlake2bTestnet4).unwrap();
        require_backend_feature(ChainId::BitcoinBlake2bTestnet4, &client, &root)
            .await
            .unwrap();
        assert_eq!(flag.hits(), 1);
    }
    #[test]
    fn saved_wallet_config_uses_actual_restored_id_and_refuses_corruption() {
        let temp = tempfile::tempdir().unwrap();
        let root = CoincubeDirectory::new(temp.path().to_owned());
        let (wallet, mut identity) = fixture(ChainId::Testnet4);
        let descriptor = wallet.main_descriptor.as_vault().unwrap().clone();
        let mut original = WalletId::generate(&descriptor);
        original.timestamp = Some(1);
        let mut restored = original.clone();
        restored.timestamp = Some(2);
        identity
            .cube
            .set_vault(VaultIdentity::new(original.clone(), Some(&descriptor)));
        let mut actual = identity.cube.clone();
        actual.set_vault(VaultIdentity::new(restored.clone(), Some(&descriptor)));
        let network = root.network_directory(ChainId::Testnet4);
        network.init().unwrap();
        let settings = settings::Settings {
            cubes: vec![actual],
            ..Default::default()
        };
        std::fs::write(
            network.path().join("settings.json"),
            serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();
        let directory = network
            .coincubed_data_directory(&restored)
            .path()
            .to_owned();
        std::fs::create_dir_all(&directory).unwrap();
        let cfg = coincubed::config::Config::new(
            coincubed::config::BitcoinConfig::new(
                ChainId::Testnet4,
                std::time::Duration::from_secs(10),
            ),
            Some(coincubed::config::BitcoinBackend::Esplora(
                installer::integration::recovery_esplora_config(
                    ChainId::Testnet4,
                    "never_persist_this_token",
                ),
            )),
            log::LevelFilter::Info,
            descriptor,
            coincubed::datadir::DataDirectory::new(directory.clone()),
        );
        let path = directory.join("daemon.toml");
        let serialized = toml::to_string_pretty(&cfg.for_persistence()).unwrap();
        assert!(!serialized.contains("never_persist_this_token"));
        std::fs::write(&path, serialized).unwrap();
        assert!(saved_vault_config(&root, &identity).is_ok());
        assert!(!network.coincubed_data_directory(&original).path().exists());
        std::fs::write(path, "corrupted").unwrap();
        assert!(matches!(
            saved_vault_config(&root, &identity),
            Err("native_persisted_config")
        ));
    }
}
