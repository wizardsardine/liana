use super::*;
use async_hwi::{DeviceKind, Error, Version};
use coincube_core::miniscript::bitcoin::{
    bip32::{DerivationPath, Xpriv},
    secp256k1::Secp256k1,
    Network, Psbt,
};
use std::{
    str::FromStr,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn account(seed: u8, path: &str, network: Network) -> (String, ImportedAccount) {
    let secp = Secp256k1::new();
    let master = Xpriv::new_master(network, &[seed; 32]).unwrap();
    let origin = DerivationPath::from_str(path).unwrap();
    let xpub = Xpub::from_priv(&secp, &master.derive_priv(&secp, &origin).unwrap());
    let fingerprint = master.fingerprint(&secp);
    (
        format!(
            "[{}/{}]{xpub}",
            fingerprint,
            origin.to_string().trim_start_matches("m/")
        ),
        ImportedAccount {
            fingerprint,
            origin,
            xpub,
        },
    )
}

fn wallet(kind: &str, chain: ChainId) -> (ImportedWalletDescriptor, Vec<ImportedAccount>) {
    let network = chain.bitcoin_network();
    let coin = if network == Network::Bitcoin { 0 } else { 1 };
    let path = if kind.starts_with("wsh") {
        format!("m/48h/{coin}h/0h/2h")
    } else {
        let purpose = match kind {
            "pkh" => 44,
            "sh(wpkh)" => 49,
            _ => 84,
        };
        format!("m/{purpose}h/{coin}h/0h")
    };
    let count = if kind.starts_with("wsh") { 3 } else { 1 };
    let keys: Vec<_> = (1..=count)
        .map(|seed| account(seed, &path, network))
        .collect();
    let render = |branch| {
        let branch_keys = keys
            .iter()
            .map(|(key, _)| format!("{key}/{branch}/*"))
            .collect::<Vec<_>>()
            .join(",");
        match kind {
            "pkh" => format!("pkh({branch_keys})"),
            "wpkh" => format!("wpkh({branch_keys})"),
            "sh(wpkh)" => format!("sh(wpkh({branch_keys}))"),
            "wsh(multi)" => format!("wsh(multi(2,{branch_keys}))"),
            "wsh(sortedmulti)" => format!("wsh(sortedmulti(2,{branch_keys}))"),
            _ => unreachable!(),
        }
    };
    (
        ImportedWalletDescriptor::parse(&render(0), Some(&render(1)), chain).unwrap(),
        keys.into_iter().map(|(_, account)| account).collect(),
    )
}

#[derive(Debug)]
struct FakeDevice {
    fingerprint: Fingerprint,
    exported: Xpub,
    expected_path: DerivationPath,
    current: Arc<AtomicBool>,
    cancel_after_fingerprint: bool,
    cancel_after_export: bool,
    fingerprint_reads: Arc<AtomicUsize>,
    xpub_reads: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl HWI for FakeDevice {
    fn device_kind(&self) -> DeviceKind {
        DeviceKind::Specter
    }

    async fn get_version(&self) -> Result<Version, Error> {
        Err(Error::UnimplementedMethod)
    }

    async fn get_master_fingerprint(&self) -> Result<Fingerprint, Error> {
        self.fingerprint_reads.fetch_add(1, Ordering::SeqCst);
        if self.cancel_after_fingerprint {
            self.current.store(false, Ordering::SeqCst);
        }
        Ok(self.fingerprint)
    }

    async fn get_extended_pubkey(&self, path: &DerivationPath) -> Result<Xpub, Error> {
        assert_eq!(path, &self.expected_path);
        self.xpub_reads.fetch_add(1, Ordering::SeqCst);
        if self.cancel_after_export {
            self.current.store(false, Ordering::SeqCst);
        }
        Ok(self.exported)
    }

    async fn register_wallet(&self, _: &str, _: &str) -> Result<Option<[u8; 32]>, Error> {
        panic!("identity verification must not register a wallet")
    }

    async fn is_wallet_registered(&self, _: &str, _: &str) -> Result<bool, Error> {
        panic!("identity verification must not query registration")
    }

    async fn display_address(&self, _: &async_hwi::AddressScript) -> Result<(), Error> {
        panic!("identity verification must not display an address")
    }

    async fn sign_tx(&self, _: &mut Psbt) -> Result<(), Error> {
        panic!("identity verification must not sign")
    }
}

fn fake(
    account: &ImportedAccount,
    exported: Xpub,
    current: Arc<AtomicBool>,
    cancel_after_fingerprint: bool,
) -> (
    Arc<dyn HWI + Send + Sync>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
) {
    let fingerprint_reads = Arc::new(AtomicUsize::new(0));
    let xpub_reads = Arc::new(AtomicUsize::new(0));
    (
        Arc::new(FakeDevice {
            fingerprint: account.fingerprint,
            exported,
            expected_path: account.origin.clone(),
            current,
            cancel_after_fingerprint,
            cancel_after_export: false,
            fingerprint_reads: fingerprint_reads.clone(),
            xpub_reads: xpub_reads.clone(),
        }),
        fingerprint_reads,
        xpub_reads,
    )
}

#[tokio::test]
async fn exact_device_export_matches_all_supported_scripts_and_multisig_members() {
    for kind in ["pkh", "wpkh", "sh(wpkh)", "wsh(multi)", "wsh(sortedmulti)"] {
        for chain in [ChainId::Bitcoin, ChainId::Regtest] {
            let (wallet, accounts) = wallet(kind, chain);
            for selected in &accounts {
                let current = Arc::new(AtomicBool::new(true));
                let (device, _, _) = fake(selected, selected.xpub, current.clone(), false);
                let verified = verify_device_account(device, &wallet, selected, || {
                    current.load(Ordering::SeqCst)
                })
                .await
                .unwrap();
                assert_eq!(verified.account(), selected);
                assert!(verified.matches_wallet(&wallet));
                assert!(!format!("{verified:?}").contains(&selected.xpub.to_string()));
            }
        }
    }
}

#[tokio::test]
async fn refuses_wrong_key_path_network_and_script_identity() {
    let (wallet, accounts) = wallet("wsh(sortedmulti)", ChainId::Regtest);
    let selected = &accounts[0];
    let current = Arc::new(AtomicBool::new(true));
    let sibling = account(77, &selected.origin.to_string(), Network::Regtest).1;
    let (device, _, _) = fake(selected, sibling.xpub, current.clone(), false);
    assert!(matches!(
        verify_device_account(device, &wallet, selected, || true).await,
        Err(DeviceIdentityError::Xpub)
    ));

    let wrong_network = account(1, &selected.origin.to_string(), Network::Bitcoin).1;
    let (device, _, _) = fake(selected, wrong_network.xpub, current.clone(), false);
    assert!(matches!(
        verify_device_account(device, &wallet, selected, || true).await,
        Err(DeviceIdentityError::Network)
    ));

    let mut wrong_path = selected.clone();
    wrong_path.origin = "m/48h/1h/1h/2h".parse().unwrap();
    let (device, fingerprint_reads, _) = fake(&wrong_path, selected.xpub, current.clone(), false);
    assert!(matches!(
        verify_device_account(device, &wallet, &wrong_path, || true).await,
        Err(DeviceIdentityError::AccountNotInWallet)
    ));
    assert_eq!(fingerprint_reads.load(Ordering::SeqCst), 0);

    let (other_script, _) = self::wallet("wsh(multi)", ChainId::Regtest);
    assert!(
        !check_export(&wallet, selected, selected.fingerprint, selected.xpub)
            .unwrap()
            .matches_wallet(&other_script)
    );
    let threshold_one = |descriptor: &coincube_core::miniscript::Descriptor<
        coincube_core::miniscript::DescriptorPublicKey,
    >| {
        descriptor.to_string().split('#').next().unwrap().replacen(
            "sortedmulti(2,",
            "sortedmulti(1,",
            1,
        )
    };
    let different_threshold = ImportedWalletDescriptor::parse(
        &threshold_one(wallet.receive_descriptor()),
        Some(&threshold_one(wallet.change_descriptor())),
        ChainId::Regtest,
    )
    .unwrap();
    assert!(account_in_both_branches(&different_threshold, selected));
    assert!(
        !check_export(&wallet, selected, selected.fingerprint, selected.xpub)
            .unwrap()
            .matches_wallet(&different_threshold)
    );
    let other_chain = self::wallet("wsh(sortedmulti)", ChainId::Testnet4).0;
    assert!(
        !check_export(&wallet, selected, selected.fingerprint, selected.xpub)
            .unwrap()
            .matches_wallet(&other_chain)
    );
    let (mainnet, mainnet_accounts) = self::wallet("wpkh", ChainId::Bitcoin);
    let fork = mainnet.paired_blake2b().unwrap();
    // A Bitcoin Blake2b import checks its devices like any other chain
    // (Robert, 2026-11-17); the identity is that chain's own.
    assert_eq!(
        check_wallet(&fork, &mainnet_accounts[0]).unwrap(),
        fork.identity(fork.chain()).unwrap()
    );
    assert_ne!(
        fork.identity(fork.chain()).unwrap(),
        mainnet.identity(mainnet.chain()).unwrap()
    );
}

#[tokio::test]
async fn refuses_swapped_device_and_stale_async_reply() {
    let (wallet, accounts) = wallet("wpkh", ChainId::Bitcoin);
    let selected = &accounts[0];
    let current = Arc::new(AtomicBool::new(true));
    let (device, _, xpub_reads) = fake(selected, selected.xpub, current.clone(), true);
    assert!(matches!(
        verify_device_account(device, &wallet, selected, || current.load(Ordering::SeqCst)).await,
        Err(DeviceIdentityError::Cancelled)
    ));
    assert_eq!(xpub_reads.load(Ordering::SeqCst), 0);

    current.store(true, Ordering::SeqCst);
    let device: Arc<dyn HWI + Send + Sync> = Arc::new(FakeDevice {
        fingerprint: selected.fingerprint,
        exported: selected.xpub,
        expected_path: selected.origin.clone(),
        current: current.clone(),
        cancel_after_fingerprint: false,
        cancel_after_export: true,
        fingerprint_reads: Arc::new(AtomicUsize::new(0)),
        xpub_reads: Arc::new(AtomicUsize::new(0)),
    });
    assert!(matches!(
        verify_device_account(device, &wallet, selected, || current.load(Ordering::SeqCst)).await,
        Err(DeviceIdentityError::Cancelled)
    ));

    current.store(false, Ordering::SeqCst);
    let (device, fingerprint_reads, _) = fake(selected, selected.xpub, current.clone(), false);
    assert!(matches!(
        verify_device_account(device, &wallet, selected, || current.load(Ordering::SeqCst)).await,
        Err(DeviceIdentityError::Cancelled)
    ));
    assert_eq!(fingerprint_reads.load(Ordering::SeqCst), 0);

    current.store(true, Ordering::SeqCst);
    let mut wrong_fingerprint = selected.clone();
    wrong_fingerprint.fingerprint = Fingerprint::from_str("01020304").unwrap();
    let (device, _, xpub_reads) = fake(&wrong_fingerprint, selected.xpub, current, false);
    assert!(matches!(
        verify_device_account(device, &wallet, selected, || true).await,
        Err(DeviceIdentityError::Fingerprint)
    ));
    assert_eq!(xpub_reads.load(Ordering::SeqCst), 0);
}
