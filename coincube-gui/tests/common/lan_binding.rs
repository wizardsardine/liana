use coincube_core::{
    descriptors::CoincubeDescriptor,
    miniscript::{bitcoin::bip32::Fingerprint, DescriptorPublicKey},
};
use coincube_gui::phone_signer::{pairing_store::SignerBinding, protocol::local_v1};
use sha2::{Digest, Sha256};
use std::str::FromStr;

pub fn binding(descriptor: &str) -> SignerBinding {
    let keys = CoincubeDescriptor::from_str(descriptor)
        .unwrap()
        .spendable_keys();
    assert_eq!(keys.len(), 1, "transport fixture has one distinct key");
    let DescriptorPublicKey::XPub(k) = &keys[0] else {
        panic!("xpub")
    };
    SignerBinding {
        key_id: "10".into(),
        xpub: k.xkey.to_string(),
        fingerprint: k.origin.as_ref().unwrap().0,
        descriptor_sha256: Sha256::digest(descriptor.as_bytes()).to_vec(),
    }
}
pub fn vault(descriptor: &str) -> Fingerprint {
    hex::encode(&Sha256::digest(descriptor.as_bytes())[..4])
        .parse()
        .unwrap()
}
#[allow(dead_code)] // Shared test helper; the loopback-only suite needs no pairing response.
pub fn proto(descriptor: &str) -> local_v1::SignerBinding {
    let b = binding(descriptor);
    local_v1::SignerBinding {
        key_id: b.key_id,
        xpub: b.xpub,
        fingerprint: b.fingerprint.to_string(),
        descriptor_sha256: b.descriptor_sha256,
    }
}
