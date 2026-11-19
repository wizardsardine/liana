use std::{
    fs,
    str::FromStr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use async_hwi::{DeviceKind, HWI};
use coincube_core::miniscript::{
    bitcoin::{
        absolute::LockTime,
        bip32::{DerivationPath, Fingerprint, Xpriv, Xpub},
        hashes::{sha256, Hash},
        psbt::PsbtSighashType,
        secp256k1::{self, Secp256k1},
        sighash::EcdsaSighashType,
        transaction::Version,
        Amount, Network, OutPoint, Psbt, ScriptBuf, Transaction, TxIn, TxOut,
    },
    psbt::PsbtExt,
    Descriptor, DescriptorPublicKey,
};

use super::*;
use crate::split_hardware::policy::imported_policy;
use crate::{dir::CoincubeDirectory, hw::HardwareWallets};
use coincube_core::{
    chain::ChainId,
    imported_spend::{finalize_imported_spend, verify_imported_signatures},
    wallet_descriptor::ImportedWalletDescriptor,
};

fn secp() -> Secp256k1<secp256k1::All> {
    Secp256k1::new()
}

fn master(seed: u8) -> Xpriv {
    Xpriv::new_master(Network::Bitcoin, &[seed; 32]).unwrap()
}

fn fp(seed: u8) -> Fingerprint {
    master(seed).fingerprint(&secp())
}

fn key_expr(seed: u8, path: &str) -> String {
    let path = DerivationPath::from_str(path).unwrap();
    let xpub = Xpub::from_priv(&secp(), &master(seed).derive_priv(&secp(), &path).unwrap());
    format!(
        "[{}/{}]{}/<0;1>/*",
        fp(seed),
        path.to_string().trim_start_matches("m/"),
        xpub
    )
}

fn desc(s: &str) -> Descriptor<DescriptorPublicKey> {
    Descriptor::from_str(s).unwrap()
}

fn singlesig(script: SinglesigScript, seed: u8) -> Descriptor<DescriptorPublicKey> {
    let (wrap, purpose) = match script {
        SinglesigScript::Pkh => ("pkh(KEY)", 44),
        SinglesigScript::ShWpkh => ("sh(wpkh(KEY))", 49),
        SinglesigScript::Wpkh => ("wpkh(KEY)", 84),
    };
    desc(&wrap.replace("KEY", &key_expr(seed, &format!("m/{purpose}'/0'/0'"))))
}

fn multisig(sorted: bool) -> Descriptor<DescriptorPublicKey> {
    let keys = [1u8, 2, 3]
        .iter()
        .map(|s| key_expr(*s, "m/48'/0'/0'/2'"))
        .collect::<Vec<_>>()
        .join(",");
    let f = if sorted { "sortedmulti" } else { "multi" };
    desc(&format!("wsh({f}(2,{keys}))"))
}

/// A PSBT spending one coin at receive index 0 of `descriptor`, filled the
/// way the Split constructions fill theirs (UTXOs, scripts, derivations).
fn spend(descriptor: &Descriptor<DescriptorPublicKey>) -> Psbt {
    let single = descriptor
        .clone()
        .into_single_descriptors()
        .unwrap()
        .remove(0)
        .at_derivation_index(0)
        .unwrap();
    let prev = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn::default()],
        output: vec![TxOut {
            value: Amount::from_sat(100_000),
            script_pubkey: single.script_pubkey(),
        }],
    };
    let tx = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(prev.compute_txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(90_000),
            script_pubkey: ScriptBuf::new_op_return([7u8; 20]),
        }],
    };
    let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
    psbt.inputs[0].non_witness_utxo = Some(prev.clone());
    if !matches!(descriptor, Descriptor::Pkh(_)) {
        psbt.inputs[0].witness_utxo = Some(prev.output[0].clone());
    }
    psbt.update_input_with_descriptor(0, &single).unwrap();
    psbt
}

/// How the fake keeps a multisig registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Registry {
    /// Jade/Coldcard/BitBox02: the device stores it and can report it.
    DeviceStored,
    /// Ledger: the device returns an HMAC, and only a handle opened with it
    /// can report or use the registration. Signs only through a bound handle.
    Token,
    /// Specter: registration cannot be queried.
    Unqueryable,
}

#[derive(Debug, Default)]
struct Shared {
    stored: Mutex<Vec<(String, String)>>,
    register_calls: AtomicUsize,
    sign_calls: AtomicUsize,
}

#[derive(Debug, Clone)]
struct Fake {
    kind: DeviceKind,
    seed: u8,
    registry: Registry,
    shared: Arc<Shared>,
    bound: Option<(String, String, Option<[u8; 32]>)>,
    sighash: EcdsaSighashType,
    /// Report the sighash only inside each signature, not the input field.
    hide_sighash_field: bool,
    refuse_registration: bool,
}

fn fake(kind: DeviceKind, seed: u8) -> Fake {
    Fake {
        kind,
        seed,
        registry: match kind {
            DeviceKind::Ledger | DeviceKind::LedgerSimulator => Registry::Token,
            DeviceKind::Specter | DeviceKind::SpecterSimulator => Registry::Unqueryable,
            _ => Registry::DeviceStored,
        },
        shared: Arc::default(),
        bound: None,
        sighash: EcdsaSighashType::All,
        hide_sighash_field: false,
        refuse_registration: false,
    }
}

fn token(name: &str, policy: &str) -> [u8; 32] {
    sha256::Hash::hash(format!("hmac|{name}|{policy}").as_bytes()).to_byte_array()
}

impl Fake {
    fn arc(&self) -> Arc<dyn HWI + Send + Sync> {
        Arc::new(self.clone())
    }

    fn registered(&self, name: &str, policy: &str) -> bool {
        match self.registry {
            Registry::Token => self.bound.as_ref().is_some_and(|(n, p, h)| {
                n == name && p == policy && *h == Some(token(name, policy))
            }),
            _ => self
                .shared
                .stored
                .lock()
                .unwrap()
                .contains(&(name.to_string(), policy.to_string())),
        }
    }
}

#[async_trait::async_trait]
impl HWI for Fake {
    fn device_kind(&self) -> DeviceKind {
        self.kind
    }
    async fn get_version(&self) -> Result<async_hwi::Version, async_hwi::Error> {
        Err(async_hwi::Error::UnimplementedMethod)
    }
    async fn get_master_fingerprint(&self) -> Result<Fingerprint, async_hwi::Error> {
        Ok(fp(self.seed))
    }
    async fn get_extended_pubkey(&self, _: &DerivationPath) -> Result<Xpub, async_hwi::Error> {
        Err(async_hwi::Error::UnimplementedMethod)
    }
    async fn register_wallet(
        &self,
        name: &str,
        policy: &str,
    ) -> Result<Option<[u8; 32]>, async_hwi::Error> {
        self.shared.register_calls.fetch_add(1, Ordering::SeqCst);
        if self.refuse_registration {
            return Err(async_hwi::Error::UserRefused);
        }
        match self.registry {
            Registry::Token => Ok(Some(token(name, policy))),
            _ => {
                self.shared
                    .stored
                    .lock()
                    .unwrap()
                    .push((name.to_string(), policy.to_string()));
                Ok(None)
            }
        }
    }
    async fn is_wallet_registered(
        &self,
        name: &str,
        policy: &str,
    ) -> Result<bool, async_hwi::Error> {
        match self.registry {
            Registry::Unqueryable => Err(async_hwi::Error::UnimplementedMethod),
            _ => Ok(self.registered(name, policy)),
        }
    }
    /// A device-stored registry is searched by policy, whatever the name,
    /// as Jade and Coldcard now do.
    async fn find_wallet_registration(
        &self,
        name: &str,
        policy: &str,
    ) -> Result<Option<async_hwi::WalletRegistration>, async_hwi::Error> {
        match self.registry {
            Registry::DeviceStored => Ok(self
                .shared
                .stored
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, p)| p == policy)
                .min_by_key(|(n, _)| n != name)
                .map(|(n, _)| async_hwi::WalletRegistration {
                    name: Some(n.clone()),
                })),
            _ => Ok(self.is_wallet_registered(name, policy).await?.then(|| {
                async_hwi::WalletRegistration {
                    name: Some(name.to_string()),
                }
            })),
        }
    }
    async fn display_address(&self, _: &async_hwi::AddressScript) -> Result<(), async_hwi::Error> {
        Err(async_hwi::Error::UnimplementedMethod)
    }
    async fn sign_tx(&self, psbt: &mut Psbt) -> Result<(), async_hwi::Error> {
        self.shared.sign_calls.fetch_add(1, Ordering::SeqCst);
        if self.registry == Registry::Token && self.bound.is_none() {
            // Ledger cannot sign without a policy.
            return Err(async_hwi::Error::UnimplementedMethod);
        }
        let multisig = psbt.inputs.iter().any(|i| i.witness_script.is_some());
        if multisig {
            let ok = match (&self.registry, &self.bound) {
                (Registry::Token, Some((n, p, _))) => self.registered(n, p),
                _ => !self.shared.stored.lock().unwrap().is_empty(),
            };
            if !ok {
                return Err(async_hwi::Error::Device("policy not registered".into()));
            }
        }
        for input in &mut psbt.inputs {
            input.sighash_type = Some(PsbtSighashType::from(self.sighash));
        }
        let _ = psbt.sign(&master(self.seed), &secp());
        if self.hide_sighash_field {
            for input in &mut psbt.inputs {
                input.sighash_type = None;
            }
        }
        Ok(())
    }
}

/// Opens a bound handle the way a production binder would: same device,
/// constructed with the policy and the session token.
#[derive(Default)]
struct FakeBinder {
    device: Option<Fake>,
    /// Bind a different physical device than the listed one.
    swap_seed: Option<u8>,
    seen: Mutex<Vec<(String, Option<[u8; 32]>)>>,
}

#[async_trait::async_trait]
impl PolicyBinder for FakeBinder {
    fn can_bind(&self, kind: DeviceKind) -> bool {
        self.device.as_ref().is_some_and(|d| d.kind == kind)
    }

    async fn bind(
        &self,
        request: BindRequest<'_>,
    ) -> Result<Arc<dyn HWI + Send + Sync>, async_hwi::Error> {
        let mut device = self
            .device
            .clone()
            .ok_or(async_hwi::Error::DeviceNotFound)?;
        assert_eq!(request.kind, device.kind);
        assert_eq!(request.fingerprint, fp(device.seed));
        if let Some(seed) = self.swap_seed {
            device.seed = seed;
        }
        self.seen
            .lock()
            .unwrap()
            .push((request.name.to_string(), request.hmac.copied()));
        device.bound = Some((
            request.name.to_string(),
            request.policy.to_string(),
            request.hmac.copied(),
        ));
        Ok(Arc::new(device))
    }
}

fn binder(device: &Fake) -> Arc<FakeBinder> {
    Arc::new(FakeBinder {
        device: Some(device.clone()),
        ..FakeBinder::default()
    })
}

fn no_binder() -> Arc<dyn PolicyBinder> {
    Arc::new(NoBinder)
}

fn sig_count(psbt: &Psbt) -> usize {
    psbt.inputs.iter().map(|i| i.partial_sigs.len()).sum()
}

fn imported(combined: &Descriptor<DescriptorPublicKey>) -> ImportedWalletDescriptor {
    let mut branches = combined.clone().into_single_descriptors().unwrap();
    ImportedWalletDescriptor::from_descriptors(
        branches.remove(0),
        branches.remove(0),
        ChainId::Bitcoin,
    )
    .unwrap()
}

#[tokio::test]
async fn imported_policy_uses_separate_identity_and_mock_devices_verify_signatures() {
    for combined in [
        singlesig(SinglesigScript::Pkh, 1),
        singlesig(SinglesigScript::ShWpkh, 1),
        singlesig(SinglesigScript::Wpkh, 1),
        multisig(false),
        multisig(true),
    ] {
        let wallet = imported(&combined);
        let policy = imported_policy(&wallet).unwrap();
        let split = DevicePolicy::new(&combined).unwrap();
        if policy.shape().is_multisig() {
            assert!(policy.name().starts_with("Import"));
            assert_eq!(policy.name().len(), 14);
            assert!(split.name().starts_with("Split"));
            assert_ne!(policy.name(), split.name());
        } else {
            assert_eq!(policy.name(), "");
        }
        let original = spend(&combined);
        let jade = fake(DeviceKind::Jade, 1);
        let mut session = SigningSession::open(jade.arc(), fp(1), policy.clone(), no_binder())
            .await
            .unwrap();
        session.register().await.unwrap();
        let first = session.sign(&original).await.unwrap().into_psbt();
        assert_eq!(
            verify_imported_signatures(&wallet, &original, &first, &secp()).unwrap(),
            vec![1]
        );
        if policy.shape().is_multisig() {
            let ledger = fake(DeviceKind::Ledger, 2);
            let mut session = SigningSession::open(ledger.arc(), fp(2), policy, binder(&ledger))
                .await
                .unwrap();
            session.register().await.unwrap();
            let second = session.sign(&first).await.unwrap().into_psbt();
            assert_eq!(
                verify_imported_signatures(&wallet, &original, &second, &secp()).unwrap(),
                vec![2]
            );
            assert_eq!(
                finalize_imported_spend(&wallet, &original, &second, &secp())
                    .unwrap()
                    .signatures_per_input(),
                &[2]
            );
        } else {
            assert_eq!(
                finalize_imported_spend(&wallet, &original, &first, &secp())
                    .unwrap()
                    .signatures_per_input(),
                &[1]
            );
        }
    }
}

#[tokio::test]
async fn split_hw_sign_singlesig_shapes_sign_and_finalize() {
    for script in [
        SinglesigScript::Pkh,
        SinglesigScript::ShWpkh,
        SinglesigScript::Wpkh,
    ] {
        let descriptor = singlesig(script, 1);
        for kind in [DeviceKind::Jade, DeviceKind::Ledger, DeviceKind::BitBox02] {
            let device = fake(kind, 1);
            let policy = DevicePolicy::new(&descriptor).unwrap();
            assert_eq!(policy.shape(), PolicyShape::Singlesig(script));
            assert_eq!(policy.name(), "", "default policy is unnamed");
            let binder = binder(&device);
            let opened = SigningSession::open(device.arc(), fp(1), policy, binder.clone()).await;
            if (kind, script) == (DeviceKind::BitBox02, SinglesigScript::Pkh) {
                // bitbox-api has no P2PKH: refused before the device is used.
                assert_eq!(
                    opened.unwrap_err(),
                    SignError::UnsupportedDevice(DeviceKind::BitBox02)
                );
                continue;
            }
            let mut session = opened.unwrap();
            assert_eq!(
                session.register().await.unwrap(),
                RegistrationOutcome::NotNeeded
            );
            assert_eq!(device.shared.register_calls.load(Ordering::SeqCst), 0);
            let psbt = spend(&descriptor);
            let mut signed = session.sign(&psbt).await.unwrap().into_psbt();
            assert_eq!(sig_count(&signed), 1, "{script:?} {kind}");
            assert_eq!(signed.unsigned_tx, psbt.unsigned_tx);
            // Ledger signs through a handle bound to the default policy
            // with no token; the others sign on the listed handle.
            let seen = binder.seen.lock().unwrap().clone();
            if kind == DeviceKind::Ledger {
                assert_eq!(seen, vec![(String::new(), None)]);
            } else {
                assert!(seen.is_empty());
            }
            // The signatures are real: miniscript verifies them.
            signed.finalize_mut(&secp()).unwrap();
        }
    }
}

#[tokio::test]
async fn split_hw_sign_multisig_registers_then_signs_2_of_3() {
    for sorted in [true, false] {
        let descriptor = multisig(sorted);
        let psbt = spend(&descriptor);
        let policy = DevicePolicy::new(&descriptor).unwrap();
        assert_eq!(policy.shape(), PolicyShape::Multisig { k: 2, n: 3, sorted });
        assert!(policy.name().starts_with("Split") && policy.name().len() == 13);

        // Key 1 on a Jade: the device stores the registration.
        let jade = fake(DeviceKind::Jade, 1);
        let mut session = SigningSession::open(jade.arc(), fp(1), policy.clone(), no_binder())
            .await
            .unwrap();
        assert_eq!(session.handle(), SigningHandle::Listed);
        assert_eq!(
            session.register().await.unwrap(),
            RegistrationOutcome::Registered
        );
        let by_jade = session.sign(&psbt).await.unwrap().into_psbt();
        assert_eq!(sig_count(&by_jade), 1);
        // A second session finds it already on the device.
        let mut again = SigningSession::open(jade.arc(), fp(1), policy.clone(), no_binder())
            .await
            .unwrap();
        assert_eq!(
            again.register().await.unwrap(),
            RegistrationOutcome::AlreadyRegistered
        );
        assert_eq!(jade.shared.register_calls.load(Ordering::SeqCst), 1);

        // Key 2 on a Ledger: registration returns a token that only this
        // session holds and that the bound handle signs with.
        let ledger = fake(DeviceKind::Ledger, 2);
        let ledger_binder = binder(&ledger);
        let mut session =
            SigningSession::open(ledger.arc(), fp(2), policy.clone(), ledger_binder.clone())
                .await
                .unwrap();
        assert_eq!(session.handle(), SigningHandle::Bound);
        assert_eq!(
            session.register().await.unwrap(),
            RegistrationOutcome::Registered
        );
        let by_ledger = session.sign(&by_jade).await.unwrap().into_psbt();
        assert_eq!(
            ledger_binder.seen.lock().unwrap().clone(),
            vec![(
                policy.name().to_string(),
                Some(token(policy.name(), &descriptor.to_string()))
            )]
        );
        assert_eq!(sig_count(&by_ledger), 2);

        let mut combined = by_ledger;
        combined.finalize_mut(&secp()).unwrap();
    }
}

#[tokio::test]
async fn split_hw_sign_uses_a_registration_another_app_made() {
    for kind in [DeviceKind::Jade, DeviceKind::Coldcard] {
        let descriptor = multisig(true);
        let psbt = spend(&descriptor);
        let policy = DevicePolicy::new(&descriptor).unwrap();

        // Specter registered the same wallet under its own name.
        let device = fake(kind, 1);
        device
            .shared
            .stored
            .lock()
            .unwrap()
            .push(("Specter 2of3".to_string(), descriptor.to_string()));
        let session_binder: Arc<dyn PolicyBinder> = if kind == DeviceKind::Coldcard {
            binder(&device)
        } else {
            no_binder()
        };
        let mut session = SigningSession::open(device.arc(), fp(1), policy, session_binder)
            .await
            .unwrap();
        assert_eq!(
            session.register().await.unwrap(),
            RegistrationOutcome::AlreadyRegistered,
            "{kind}"
        );
        assert_eq!(
            device.shared.register_calls.load(Ordering::SeqCst),
            0,
            "{kind}"
        );
        let signed = session.sign(&psbt).await.unwrap().into_psbt();
        assert_eq!(sig_count(&signed), 1, "{kind}");
    }
}

#[tokio::test]
async fn split_hw_sign_refuses_unregistered_policy() {
    let descriptor = multisig(true);
    let psbt = spend(&descriptor);
    let policy = DevicePolicy::new(&descriptor).unwrap();

    // Signing before registering in this session is refused without
    // touching the device.
    let jade = fake(DeviceKind::Jade, 1);
    let session = SigningSession::open(jade.arc(), fp(1), policy.clone(), no_binder())
        .await
        .unwrap();
    assert_eq!(
        session.sign(&psbt).await.unwrap_err(),
        SignError::PolicyNotRegistered
    );
    assert_eq!(jade.shared.sign_calls.load(Ordering::SeqCst), 0);

    // The registration disappears from the device (wiped or replaced)
    // between registering and signing.
    let mut session = SigningSession::open(jade.arc(), fp(1), policy.clone(), no_binder())
        .await
        .unwrap();
    session.register().await.unwrap();
    jade.shared.stored.lock().unwrap().clear();
    assert_eq!(
        session.sign(&psbt).await.unwrap_err(),
        SignError::PolicyNotRegistered
    );
    assert_eq!(jade.shared.sign_calls.load(Ordering::SeqCst), 0);

    // A Ledger handle bound with the wrong token does not report the
    // policy, so nothing is signed.
    let ledger = fake(DeviceKind::Ledger, 2);
    struct WrongToken(Fake);
    #[async_trait::async_trait]
    impl PolicyBinder for WrongToken {
        fn can_bind(&self, _: DeviceKind) -> bool {
            true
        }
        async fn bind(
            &self,
            r: BindRequest<'_>,
        ) -> Result<Arc<dyn HWI + Send + Sync>, async_hwi::Error> {
            let mut device = self.0.clone();
            device.bound = Some((r.name.into(), r.policy.into(), Some([0; 32])));
            Ok(Arc::new(device))
        }
    }
    let mut session = SigningSession::open(
        ledger.arc(),
        fp(2),
        policy.clone(),
        Arc::new(WrongToken(ledger.clone())),
    )
    .await
    .unwrap();
    session.register().await.unwrap();
    assert_eq!(
        session.sign(&psbt).await.unwrap_err(),
        SignError::PolicyNotRegistered
    );
    assert_eq!(ledger.shared.sign_calls.load(Ordering::SeqCst), 0);

    // The user declines the registration.
    let mut refusing = fake(DeviceKind::Jade, 1);
    refusing.refuse_registration = true;
    let mut session = SigningSession::open(refusing.arc(), fp(1), policy.clone(), no_binder())
        .await
        .unwrap();
    assert_eq!(
        session.register().await.unwrap_err(),
        SignError::RegistrationRefused
    );
    assert_eq!(
        session.sign(&psbt).await.unwrap_err(),
        SignError::PolicyNotRegistered
    );

    // Specter cannot report registration: multisig is refused at open,
    // singlesig still works.
    let specter = fake(DeviceKind::Specter, 1);
    assert_eq!(
        SigningSession::open(specter.arc(), fp(1), policy.clone(), no_binder())
            .await
            .unwrap_err(),
        SignError::UnsupportedDevice(DeviceKind::Specter)
    );
    let single = singlesig(SinglesigScript::Wpkh, 1);
    let session = SigningSession::open(
        specter.arc(),
        fp(1),
        DevicePolicy::new(&single).unwrap(),
        no_binder(),
    )
    .await
    .unwrap();
    session.sign(&spend(&single)).await.unwrap();
}

/// F1: a device that signs only through a bound handle is refused at open
/// when the caller has no binder, so the user is never asked to register
/// (and Coldcard/BitBox02 never store) a policy that cannot be used.
#[tokio::test]
async fn split_hw_sign_no_binder_refuses_before_any_device_prompt() {
    let multi = DevicePolicy::new(&multisig(true)).unwrap();
    let single = DevicePolicy::new(&singlesig(SinglesigScript::Wpkh, 1)).unwrap();
    for (kind, policy) in [
        (DeviceKind::Coldcard, &multi),
        (DeviceKind::BitBox02, &multi),
        (DeviceKind::Ledger, &multi),
        (DeviceKind::Ledger, &single),
    ] {
        let device = fake(kind, 1);
        assert_eq!(
            SigningSession::open(device.arc(), fp(1), policy.clone(), no_binder())
                .await
                .unwrap_err(),
            SignError::NeedsPolicyBinding(kind),
            "{kind}"
        );
        // A binder for another device class does not count either.
        let other = binder(&fake(DeviceKind::Jade, 1));
        assert_eq!(
            SigningSession::open(device.arc(), fp(1), policy.clone(), other)
                .await
                .unwrap_err(),
            SignError::NeedsPolicyBinding(kind),
            "{kind}"
        );
        assert_eq!(device.shared.register_calls.load(Ordering::SeqCst), 0);
        assert_eq!(device.shared.sign_calls.load(Ordering::SeqCst), 0);
        assert!(device.shared.stored.lock().unwrap().is_empty());
    }
    // With a binder for the class, the same device opens and registers.
    let coldcard = fake(DeviceKind::Coldcard, 1);
    let mut session = SigningSession::open(coldcard.arc(), fp(1), multi, binder(&coldcard))
        .await
        .unwrap();
    assert_eq!(
        session.register().await.unwrap(),
        RegistrationOutcome::Registered
    );
}

/// F4: a bound handle that answers for another device is refused before it
/// is asked to sign.
#[tokio::test]
async fn split_hw_sign_bound_handle_fingerprint_is_rechecked() {
    let descriptor = singlesig(SinglesigScript::Wpkh, 1);
    let ledger = fake(DeviceKind::Ledger, 1);
    let swapped = Arc::new(FakeBinder {
        device: Some(ledger.clone()),
        swap_seed: Some(9),
        ..FakeBinder::default()
    });
    let session = SigningSession::open(
        ledger.arc(),
        fp(1),
        DevicePolicy::new(&descriptor).unwrap(),
        swapped,
    )
    .await
    .unwrap();
    assert_eq!(
        session.sign(&spend(&descriptor)).await.unwrap_err(),
        SignError::Fingerprint
    );
    assert_eq!(ledger.shared.sign_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn split_hw_sign_handle_table() {
    let single = PolicyShape::Singlesig(SinglesigScript::Wpkh);
    let multi = PolicyShape::Multisig {
        k: 2,
        n: 3,
        sorted: true,
    };
    use SigningHandle::*;
    for (kind, s, m) in [
        (DeviceKind::Jade, Ok(Listed), Ok(Listed)),
        (DeviceKind::Coldcard, Ok(Listed), Ok(Bound)),
        // Multisig must stay Bound: bitbox-api reaches `todo!()` for P2WSH
        // without a forced policy.
        (DeviceKind::BitBox02, Ok(Listed), Ok(Bound)),
        (DeviceKind::Ledger, Ok(Bound), Ok(Bound)),
        (DeviceKind::LedgerSimulator, Ok(Bound), Ok(Bound)),
        (
            DeviceKind::Specter,
            Ok(Listed),
            Err(SignError::UnsupportedDevice(DeviceKind::Specter)),
        ),
    ] {
        assert_eq!(signing_handle(kind, single), s, "{kind}");
        assert_eq!(signing_handle(kind, multi), m, "{kind}");
    }
    // BitBox02 cannot infer P2PKH; the other singlesig scripts are listed.
    let pkh = PolicyShape::Singlesig(SinglesigScript::Pkh);
    assert_eq!(
        signing_handle(DeviceKind::BitBox02, pkh),
        Err(SignError::UnsupportedDevice(DeviceKind::BitBox02))
    );
    assert_eq!(
        signing_handle(
            DeviceKind::BitBox02,
            PolicyShape::Singlesig(SinglesigScript::ShWpkh)
        ),
        Ok(Listed)
    );
    for kind in [DeviceKind::Jade, DeviceKind::Coldcard, DeviceKind::Ledger] {
        assert_eq!(signing_handle(kind, pkh), signing_handle(kind, single));
    }
}

/// F2: only Ledger's registration is session-only; the others keep it.
#[test]
fn split_hw_sign_registration_notice_matches_device() {
    for kind in [DeviceKind::Ledger, DeviceKind::LedgerSimulator] {
        assert_eq!(registration_notice(kind), REGISTRATION_NOTICE_SESSION);
    }
    for kind in [DeviceKind::Jade, DeviceKind::Coldcard, DeviceKind::BitBox02] {
        assert_eq!(registration_notice(kind), REGISTRATION_NOTICE_ON_DEVICE);
    }
    assert!(REGISTRATION_NOTICE_SESSION.contains("asked again next time"));
    assert!(REGISTRATION_NOTICE_ON_DEVICE.contains("will not ask again"));
}

#[tokio::test]
async fn split_hw_sign_refuses_non_all_device_output() {
    let descriptor = singlesig(SinglesigScript::Wpkh, 1);
    let psbt = spend(&descriptor);
    let policy = DevicePolicy::new(&descriptor).unwrap();
    for sighash in [
        EcdsaSighashType::None,
        EcdsaSighashType::Single,
        EcdsaSighashType::AllPlusAnyoneCanPay,
        EcdsaSighashType::NonePlusAnyoneCanPay,
        EcdsaSighashType::SinglePlusAnyoneCanPay,
    ] {
        for hide in [false, true] {
            let mut device = fake(DeviceKind::Jade, 1);
            device.sighash = sighash;
            device.hide_sighash_field = hide;
            let session = SigningSession::open(device.arc(), fp(1), policy.clone(), no_binder())
                .await
                .unwrap();
            assert_eq!(
                session.sign(&psbt).await.unwrap_err(),
                SignError::NonAllSighash { input: 0 },
                "{sighash} hidden={hide}"
            );
            assert_eq!(device.shared.sign_calls.load(Ordering::SeqCst), 1);
        }
    }

    // A PSBT that itself asks for a non-ALL sighash is refused before the
    // device sees it; an explicit ALL (0x01) is accepted like the default.
    let device = fake(DeviceKind::Jade, 1);
    let session = SigningSession::open(device.arc(), fp(1), policy.clone(), no_binder())
        .await
        .unwrap();
    let mut asks_single = psbt.clone();
    asks_single.inputs[0].sighash_type = Some(PsbtSighashType::from(EcdsaSighashType::Single));
    assert_eq!(
        session.sign(&asks_single).await.unwrap_err(),
        SignError::NonAllSighash { input: 0 }
    );
    assert_eq!(device.shared.sign_calls.load(Ordering::SeqCst), 0);
    let mut explicit_all = psbt.clone();
    explicit_all.inputs[0].sighash_type = Some(PsbtSighashType::from(EcdsaSighashType::All));
    let mut signed = session.sign(&explicit_all).await.unwrap().into_psbt();
    signed.finalize_mut(&secp()).unwrap();
}

#[test]
fn split_hw_sign_screens_untrusted_device_output() {
    let descriptor = multisig(true);
    let original = spend(&descriptor);
    let mine = fp(1);
    let sign_as = |seed: u8, base: &Psbt| {
        let mut psbt = base.clone();
        let _ = psbt.sign(&master(seed), &secp());
        psbt
    };

    let good = sign_as(1, &original);
    let accepted = accept_device_signatures(&original, &good, mine).unwrap();
    assert_eq!(sig_count(&accepted), 1);

    // Extra fields the device may set are dropped, not trusted.
    let mut noisy = good.clone();
    noisy.inputs[0].final_script_witness = Some(Default::default());
    noisy.inputs[0].redeem_script = Some(ScriptBuf::new());
    let accepted = accept_device_signatures(&original, &noisy, mine).unwrap();
    assert!(accepted.inputs[0].final_script_witness.is_none());
    assert_eq!(
        accepted.inputs[0].redeem_script,
        original.inputs[0].redeem_script
    );

    // F4: the device sets a non-ALL sighash field on the input while every
    // signature byte is ALL. Only the field check catches this.
    for sighash in [EcdsaSighashType::Single, EcdsaSighashType::None] {
        let mut field_only = good.clone();
        field_only.inputs[0].sighash_type = Some(PsbtSighashType::from(sighash));
        assert!(field_only.inputs[0]
            .partial_sigs
            .values()
            .all(|sig| sig.sighash_type == EcdsaSighashType::All));
        assert_eq!(
            accept_device_signatures(&original, &field_only, mine).unwrap_err(),
            SignError::NonAllSighash { input: 0 }
        );
    }
    // An explicit ALL field is fine.
    let mut explicit = good.clone();
    explicit.inputs[0].sighash_type = Some(PsbtSighashType::from(EcdsaSighashType::All));
    accept_device_signatures(&original, &explicit, mine).unwrap();

    // A changed transaction.
    let mut changed = good.clone();
    changed.unsigned_tx.output[0].value = Amount::from_sat(1);
    assert_eq!(
        accept_device_signatures(&original, &changed, mine).unwrap_err(),
        SignError::TransactionChanged
    );
    // A signature for a key the device does not own in this PSBT.
    let other = sign_as(2, &original);
    assert_eq!(
        accept_device_signatures(&original, &other, mine).unwrap_err(),
        SignError::UnexpectedSignature { input: 0 }
    );
    // A replaced existing signature.
    let (key, sig) = good.inputs[0].partial_sigs.iter().next().unwrap();
    let mut prior = original.clone();
    prior.inputs[0].partial_sigs.insert(*key, *sig);
    let mut replaced = good.clone();
    let mut forged = *sig;
    forged.signature = other.inputs[0]
        .partial_sigs
        .values()
        .next()
        .unwrap()
        .signature;
    replaced.inputs[0].partial_sigs.insert(*key, forged);
    assert_eq!(
        accept_device_signatures(&prior, &replaced, mine).unwrap_err(),
        SignError::UnexpectedSignature { input: 0 }
    );
    // A Taproot signature on a non-Taproot shape.
    let mut taproot = good.clone();
    taproot.inputs[0].tap_key_sig =
        Some(coincube_core::miniscript::bitcoin::taproot::Signature::from_slice(&[1; 64]).unwrap());
    assert_eq!(
        accept_device_signatures(&original, &taproot, mine).unwrap_err(),
        SignError::UnexpectedSignature { input: 0 }
    );
    // Nothing new.
    assert_eq!(
        accept_device_signatures(&original, &original, mine).unwrap_err(),
        SignError::DeviceDidNotSign
    );
    assert_eq!(
        accept_device_signatures(&prior, &good, mine).unwrap_err(),
        SignError::DeviceDidNotSign
    );
}

#[tokio::test]
async fn split_hw_sign_policy_validation() {
    let k1 = key_expr(1, "m/48'/0'/0'/2'");
    let k2 = key_expr(2, "m/48'/0'/0'/2'");
    let refuse = |s: &str| DevicePolicy::new(&desc(s)).unwrap_err();

    assert_eq!(
        refuse(&format!("sh(multi(1,{k1},{k2}))")),
        SignError::UnsupportedShape
    );
    assert_eq!(
        refuse(&format!("sh(wsh(multi(1,{k1},{k2})))")),
        SignError::UnsupportedShape
    );
    assert_eq!(
        refuse(&format!("wsh(and_v(v:pk({k1}),pk({k2})))")),
        SignError::UnsupportedShape
    );
    assert_eq!(
        refuse(&format!("wsh(pk({k1}))")),
        SignError::UnsupportedShape
    );
    assert_eq!(
        refuse(&format!("tr({})", key_expr(1, "m/86'/0'/0'"))),
        SignError::UnsupportedShape
    );
    // Key forms: no origin, a single branch, a hardened wildcard.
    let bare = k1.split(']').nth(1).unwrap().to_string();
    assert_eq!(
        refuse(&format!("wsh(multi(1,{bare},{k2}))")),
        SignError::KeyForm
    );
    assert_eq!(
        refuse(&format!(
            "wsh(multi(1,{},{k2}))",
            k1.replace("/<0;1>/*", "/0/*")
        )),
        SignError::KeyForm
    );
    assert_eq!(
        refuse(&format!(
            "wsh(multi(1,{},{k2}))",
            k1.replace("/<0;1>/*", "/<0;1>/*'")
        )),
        SignError::KeyForm
    );
    assert_eq!(
        refuse(&format!("wsh(multi(1,{k1},{k1}))")),
        SignError::DuplicateKey
    );
    // Singlesig keys must be on their script's standard account path.
    assert_eq!(
        refuse(&format!("wpkh({})", key_expr(1, "m/44'/0'/0'"))),
        SignError::NonStandardSinglesigPath
    );
    assert_eq!(
        refuse(&format!("pkh({})", key_expr(1, "m/44'/0'/0'/0'"))),
        SignError::NonStandardSinglesigPath
    );
    assert_eq!(
        refuse(&format!("sh(wpkh({}))", key_expr(1, "m/49'/1'/0'"))),
        SignError::NonStandardSinglesigPath
    );

    // The device must hold one of the keys, and must be the listed one.
    let policy = DevicePolicy::new(&multisig(true)).unwrap();
    assert_eq!(
        SigningSession::open(
            fake(DeviceKind::Jade, 9).arc(),
            fp(9),
            policy.clone(),
            no_binder()
        )
        .await
        .unwrap_err(),
        SignError::DeviceNotInPolicy
    );
    assert_eq!(
        SigningSession::open(fake(DeviceKind::Jade, 3).arc(), fp(1), policy, no_binder())
            .await
            .unwrap_err(),
        SignError::Fingerprint
    );
}

fn temp_root() -> std::path::PathBuf {
    crate::split_hardware::tests::unique_temp_root("coincube-split-hw-sign")
}

/// P6 (Ledger): the token stays in memory, nothing reaches the datadir, and a
/// new session registers again. Devices that store the registration find it
/// next time instead (see `split_hw_sign_multisig_registers_then_signs_2_of_3`).
#[tokio::test]
async fn split_hw_sign_session_persists_nothing_and_reregisters() {
    let root = temp_root();
    let mut devices =
        HardwareWallets::new(CoincubeDirectory::new(root.clone()), Network::Bitcoin).ephemeral();
    let ledger = fake(DeviceKind::Ledger, 2);
    devices.list.push(HardwareWallet::Supported {
        id: "ledger-1".into(),
        device: ledger.arc(),
        kind: DeviceKind::Ledger,
        fingerprint: fp(2),
        version: None,
        registered: None,
        alias: None,
    });
    let descriptor = multisig(false);
    let psbt = spend(&descriptor);
    let policy = DevicePolicy::new(&descriptor).unwrap();
    let hmac_hex = hex::encode(token(policy.name(), &descriptor.to_string()));

    for session_number in 1..=2 {
        let mut session =
            SigningSession::from_listed(&devices.list[0], policy.clone(), binder(&ledger))
                .await
                .unwrap();
        // A fresh session holds no registration, whatever an earlier one did.
        assert_eq!(
            session.sign(&psbt).await.unwrap_err(),
            SignError::PolicyNotRegistered
        );
        assert_eq!(
            session.register().await.unwrap(),
            RegistrationOutcome::Registered
        );
        assert_eq!(
            ledger.shared.register_calls.load(Ordering::SeqCst),
            session_number
        );
        let debug = format!("{session:?}");
        assert!(
            !debug.contains(&hmac_hex) && !debug.contains("xpub") && !debug.contains("wsh"),
            "{}",
            debug
        );
        session.sign(&psbt).await.unwrap();
        drop(session);
    }
    assert!(!devices.persists_pairing());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    fs::remove_dir(root).unwrap();

    // A locked or unsupported listing cannot open a session.
    let locked = HardwareWallet::Unsupported {
        id: "x".into(),
        kind: DeviceKind::Ledger,
        version: None,
        reason: crate::hw::UnsupportedReason::AppIsNotOpen,
    };
    assert_eq!(
        SigningSession::from_listed(&locked, policy, no_binder())
            .await
            .unwrap_err(),
        SignError::DeviceUnavailable
    );
}
