#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

use liana::{
    descriptors::{LianaDescriptor, LianaPolicy, PathInfo},
    miniscript::{
        bitcoin::{bip32, Network, Psbt},
        descriptor,
    },
};
use secp256k1::global::SECP256K1;

use std::{collections::BTreeMap, str::FromStr};

#[derive(Arbitrary, Debug)]
struct PathConfig {
    pub thresh: u8,
    pub count: u8,
}

impl PathConfig {
    /// Generate the data for this path. We reuse the same xpub across the board as it doesn't
    /// matter. However we change the fingerprint, as it matters for the spend info analysis.
    pub fn info(&self, path_index: u8) -> Option<PathInfo> {
        if self.thresh > self.count || self.count > 100 {
            // Not worth wasting time generating pathological or huge policies.
            return None;
        }

        let public_key = secp256k1::PublicKey::from_str(
            "0250929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0",
        )
        .expect("Valid pubkey: NUMS from BIP341");
        let dummy_fg = [0, 0, path_index, 0].into();
        let xpub = bip32::Xpub {
            network: Network::Bitcoin,
            depth: 0,
            parent_fingerprint: dummy_fg,
            child_number: 0.into(),
            public_key,
            chain_code: [path_index; 32].into(),
        };

        if self.count == 1 {
            Some(PathInfo::Single(
                descriptor::DescriptorPublicKey::MultiXPub(descriptor::DescriptorMultiXKey {
                    origin: Some((dummy_fg, vec![0.into()].into())),
                    xkey: xpub,
                    derivation_paths: descriptor::DerivPaths::new(vec![
                        vec![0.into()].into(),
                        vec![1.into()].into(),
                    ])
                    .unwrap(),
                    wildcard: descriptor::Wildcard::Unhardened,
                }),
            ))
        } else {
            let keys: Vec<_> = (0..self.count)
                .map(|i| {
                    descriptor::DescriptorPublicKey::MultiXPub(descriptor::DescriptorMultiXKey {
                        origin: Some(([0, 0, path_index, i].into(), vec![0.into()].into())),
                        xkey: xpub,
                        derivation_paths: descriptor::DerivPaths::new(vec![
                            vec![(i as u32).into()].into(),
                            vec![(i as u32 + 1).into()].into(),
                        ])
                        .unwrap(),
                        wildcard: descriptor::Wildcard::Unhardened,
                    })
                })
                .collect();
            Some(PathInfo::Multi(self.thresh.into(), keys))
        }
    }
}

#[derive(Arbitrary, Debug)]
struct RecPathConfig {
    pub timelock: u8,
    pub path: PathConfig,
}

#[derive(Debug)]
struct DummyPsbt(pub Option<Psbt>);

impl<'a> Arbitrary<'a> for DummyPsbt {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let psbt_bytes = <&[u8]>::arbitrary(u)?;
        Ok(Self(Psbt::deserialize(&psbt_bytes).ok()))
    }
}

// TODO: be smarter instead of parsing a PSBT, have a config which generates an interesting PSBT.
/// The configuration for this target: the descriptor policy to be used, and various things we use
/// in exercising the various methods of the descriptor.
#[derive(Arbitrary, Debug)]
struct Config {
    pub prim_path: PathConfig,
    pub recovery_paths: Vec<RecPathConfig>,
    pub der_index: u16,
    pub dummy_psbt: DummyPsbt,
}

fuzz_target!(|config: Config| {
    // Not worth wasting time compiling huge policies.
    if config.recovery_paths.len() > 100 {
        return;
    }

    // Generate the descriptor according to the policy in the config.
    let prim_path_info = if let Some(info) = config.prim_path.info(0) {
        info
    } else {
        return;
    };
    let rec_paths_info: Option<BTreeMap<_, _>> = config
        .recovery_paths
        .into_iter()
        .enumerate()
        .map(|(idx, rec_conf)| Some((rec_conf.timelock.into(), rec_conf.path.info(idx as u8 + 1)?)))
        .collect();
    let rec_paths_info = if let Some(info) = rec_paths_info {
        info
    } else {
        return;
    };
    let policy = if let Ok(policy) = LianaPolicy::new(prim_path_info, rec_paths_info) {
        policy
    } else {
        return;
    };
    let desc = LianaDescriptor::new(policy);

    // The descriptor must roundtrip.
    assert_eq!(desc, LianaDescriptor::from_str(&desc.to_string()).unwrap());

    // We can get the policy out of this desc and a desc out of this policy, but it's not
    // guaranteed to roundtrip: policy->descriptor involves a compilation.
    LianaDescriptor::new(desc.policy());

    // Exercise the various methods on the descriptor. None should crash.
    desc.receive_descriptor();
    desc.change_descriptor();
    desc.first_timelock_value();
    desc.max_sat_weight();
    desc.max_sat_vbytes();
    desc.spender_input_size();

    // Exercise the various methods of derived descriptors. None should crash.
    let der_index = (config.der_index as u32).into();
    let der_descs = [
        desc.receive_descriptor().derive(der_index, &SECP256K1),
        desc.change_descriptor().derive(der_index, &SECP256K1),
    ];
    for desc in &der_descs {
        desc.address(Network::Bitcoin);
        desc.script_pubkey();
        desc.witness_script();
        desc.bip32_derivations();
    }

    // Exercise the methods gathering information from a PSBT. TODO: get more useful PSBTs.
    if let Some(mut psbt) = config.dummy_psbt.0 {
        desc.change_indexes(&psbt, &SECP256K1);

        // Get the spend info without populating the PSBT at all.
        let _ = desc.partial_spend_info(&psbt);

        // Populate the PSBT. We arbitrarily use the receive desc for inputs and the change desc
        // for outputs.
        let rec_desc = &der_descs[0];
        for psbt_in in psbt.inputs.iter_mut() {
            psbt_in.witness_script = Some(rec_desc.witness_script());
            psbt_in.bip32_derivation = rec_desc.bip32_derivations();
        }
        let change_desc = &der_descs[1];
        for psbt_out in psbt.outputs.iter_mut() {
            psbt_out.bip32_derivation = change_desc.bip32_derivations();
        }

        // Now get the spend info again with these info.
        let _ = desc.partial_spend_info(&psbt);

        // Prune all the info but those for the latest available path, and get the spend info
        // again.
        if let Ok(psbt) = desc.prune_bip32_derivs_last_avail(psbt) {
            let _ = desc.partial_spend_info(&psbt);
        }
    }
});
