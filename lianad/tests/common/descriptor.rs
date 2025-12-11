use std::{collections::BTreeMap, str::FromStr};

use liana::{
    bip39::Mnemonic,
    descriptors::{LianaDescriptor, LianaPolicy, PathInfo},
    signer::HotSigner,
};
use miniscript::{
    bitcoin::{bip32, secp256k1, Network, Psbt},
    descriptor::{DerivPaths, DescriptorMultiXKey, Wildcard},
    DescriptorPublicKey,
};

// Create a hot signer. The index and timelock (if any) are used to vary the entropy
// so that different signers are created for different indices and timelocks.
fn create_hot_signer(index: u8, timelock: Option<u16>) -> HotSigner {
    let timelock_bytes = timelock.unwrap_or_default().to_le_bytes();

    let mut entropy = [0u8; 32];
    entropy[0] = timelock_bytes[0];
    entropy[1] = timelock_bytes[1];
    entropy[2] = index;

    let mnemonic = Mnemonic::from_entropy(&entropy).expect("valid entropy for mnemonic");

    HotSigner::from_str(Network::Regtest, mnemonic.to_string().as_str())
        .expect("valid mnemonic for signer")
}

fn hot_signer_multi_xkey(signer: &HotSigner) -> DescriptorPublicKey {
    let secp = secp256k1::Secp256k1::signing_only();
    let fg = signer.fingerprint(&secp);
    let xkey = signer.xpub_at(&bip32::DerivationPath::master(), &secp);
    DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
        origin: Some((fg, bip32::DerivationPath::master())),
        xkey,
        derivation_paths: DerivPaths::new(vec![
            bip32::DerivationPath::from_str("m/0").unwrap(),
            bip32::DerivationPath::from_str("m/1").unwrap(),
        ])
        .expect("valid deriv paths"),
        wildcard: Wildcard::Unhardened,
    })
}

/// Kinds of test descriptors that can be created.
pub enum DescriptorKind {
    /// A primary path and one recovery path.
    ///
    /// Each spending path consists of one signer only.
    SingleSig,
}

pub struct TestDescriptor {
    pub descriptor: LianaDescriptor,
    pub primary_signers: Vec<HotSigner>,
    pub recovery_signers: BTreeMap<u16, Vec<HotSigner>>,
}

impl TestDescriptor {
    pub fn new(kind: DescriptorKind, use_taproot: bool) -> Self {
        match kind {
            DescriptorKind::SingleSig => {
                const TIMELOCK: u16 = 10;
                // One signer per path.
                let prim_signer = create_hot_signer(0, None);
                let recov_signer = create_hot_signer(0, Some(TIMELOCK));

                let prim_xpub = hot_signer_multi_xkey(&prim_signer);
                let recov_xpub = hot_signer_multi_xkey(&recov_signer);
                let recov_paths = BTreeMap::from([(TIMELOCK, PathInfo::Single(recov_xpub))]);

                let policy = if use_taproot {
                    LianaPolicy::new(PathInfo::Single(prim_xpub), recov_paths)
                        .expect("taproot policy")
                } else {
                    LianaPolicy::new_legacy(PathInfo::Single(prim_xpub), recov_paths)
                        .expect("legacy policy")
                };
                let descriptor = LianaDescriptor::new(policy);
                Self {
                    descriptor,
                    primary_signers: vec![prim_signer],
                    recovery_signers: BTreeMap::from([(TIMELOCK, vec![recov_signer])]),
                }
            }
        }
    }

    /// Sign the given PSBT using the signer at the given index in the path with the given
    /// timelock, if any, or otherwise the primary path.
    pub fn sign_psbt(
        &self,
        psbt: Psbt,
        secp: &secp256k1::Secp256k1<secp256k1::All>,
        signer_index: usize,
        timelock: Option<u16>,
    ) -> Result<Psbt, anyhow::Error> {
        let signers = if let Some(tl) = timelock {
            self.recovery_signers
                .get(&tl)
                .expect("signers for given timelock")
        } else {
            &self.primary_signers
        };
        signers[signer_index].sign_psbt(psbt, secp).map_err(|e| {
            anyhow::anyhow!(
                "signing psbt with signer at timelock {:?} and index {} failed: {}",
                timelock,
                signer_index,
                e
            )
        })
    }
}
