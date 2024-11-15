#![no_main]

use libfuzzer_sys::fuzz_target;

use liana::{descriptors::LianaDescriptor, miniscript::bitcoin::Network};
use secp256k1::global::SECP256K1;

use std::str::{self, FromStr};

// Hacky way to detect too deeply nested wrappers, which would make rust-miniscript stack overflow
// when parsing them.
fn too_deep_wrappers(desc_str: &str) -> bool {
    let mut wrapper_count = 0;
    for c in desc_str.chars() {
        if c == ':' || c == '(' || c == ')' || c == ',' {
            if c == ':' && wrapper_count > 10 {
                return true;
            }
            wrapper_count = 0;
        } else {
            wrapper_count += 1;
        }
    }
    false
}

fuzz_target!(|data: &[u8]| {
    let desc_str = match str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => return,
    };
    if data.len() > 10_000 {
        return;
    }

    // Rust-miniscript uses a recursive parsing algorithm which could make us crash when trying to
    // parse nested fragments. Until they fix it, rule out the most trivial recursion overflow the
    // fuzzer can generate: too deep wrappers.
    // FIXME: this shouldn't be necessary when upgrading to the next rust-miniscript version.
    if too_deep_wrappers(desc_str) {
        return;
    }

    let desc = match LianaDescriptor::from_str(desc_str) {
        Ok(d) => d,
        Err(_) => return,
    };

    // The descriptor must roundtrip.
    assert_eq!(desc, LianaDescriptor::from_str(&desc.to_string()).unwrap());

    // We can get the policy out of this desc and a desc out of this policy, but it's not
    // guaranteed to roundtrip: policy->descriptor involves a compilation.
    LianaDescriptor::new(desc.policy());

    // Exercise the various methods on the descriptor. None should crash.
    desc.receive_descriptor();
    desc.change_descriptor();
    desc.first_timelock_value();
    desc.max_sat_weight(true);
    desc.max_sat_vbytes(true);
    desc.spender_input_size(true);
    desc.max_sat_weight(false);
    desc.max_sat_vbytes(false);
    desc.spender_input_size(false);

    // Exercise the various methods of derived descriptors. None should crash.
    let der_index = 42.into();
    let der_descs = [
        desc.receive_descriptor().derive(der_index, SECP256K1),
        desc.change_descriptor().derive(der_index, SECP256K1),
    ];
    let mut psbt_in = Default::default();
    let mut psbt_out = Default::default();
    for desc in der_descs {
        desc.address(Network::Bitcoin);
        desc.script_pubkey();
        desc.update_psbt_in(&mut psbt_in);
        desc.update_change_psbt_out(&mut psbt_out);
    }
});
