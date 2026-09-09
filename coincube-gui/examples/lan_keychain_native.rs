//! Test driver for the real Rust desktop -> Dart TLS host -> native BDK flow.
//! Public test fixtures only. No fake phone and no echoed signature.
use base64::{engine::general_purpose::STANDARD, Engine};
use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, psbt::Psbt};
use coincube_gui::{
    dir::{CoincubeDirectory, NetworkDirectory},
    phone_signer::{
        identity, mdns::DiscoveredPhone, pairing, pairing_listener, transport::PairedTransport,
        PhoneSigner,
    },
    services::connect::crypto::DeviceTransportKey,
};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&dir).unwrap();
    let fixture: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&args[3]).unwrap()).unwrap();
    let index: usize = args[4].parse().unwrap();
    let kind = &args[5];
    let descriptor = fixture["fixtures"][kind]["descriptor"].as_str().unwrap();
    let hash = Sha256::digest(descriptor.as_bytes());
    let vault: Fingerprint = hex::encode(&hash[..4]).parse().unwrap();
    let identity = identity::load_or_create(&CoincubeDirectory::new(dir.clone())).unwrap();
    if args[1] == "offer" {
        let offer = pairing::generate_offer(
            vault,
            &identity,
            "native-interop".into(),
            pairing::OfferedKey {
                signer_xpub: fixture["keys"][index]["xpub"].as_str().unwrap().into(),
                descriptor_sha256: hex::encode(hash),
            },
        )
        .offer;
        let encoded = pairing::encode_offer(&offer).unwrap();
        iced::widget::qr_code::Data::new(&encoded).expect("production QR must render");
        std::fs::write(dir.join("offer.json"), serde_json::to_vec(&offer).unwrap()).unwrap();
        println!("{}", pairing::encode_offer(&offer).unwrap());
        return;
    }
    let offer: pairing::PairingOffer =
        serde_json::from_slice(&std::fs::read(dir.join("offer.json")).unwrap()).unwrap();
    let port: u16 = args[6].parse().unwrap();
    let fingerprint: Fingerprint = fixture["keys"][index]["fingerprint"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let desktop_dir = CoincubeDirectory::new(dir.clone());
    if args[1] == "pair-desktop-storage-failure" {
        std::fs::create_dir(dir.join("pairing-transactions.json.tmp")).unwrap();
    }
    let result = pairing_listener::run_pairing(
        identity::DesktopIdentity {
            cert_der: identity.cert_der.clone(),
            key_der: identity.clone_key(),
        },
        offer,
        DiscoveredPhone {
            cert_fp8: "test".into(),
            addr: ([127, 0, 0, 1], port).into(),
            instance_name: "native-interop".into(),
        },
        vault,
        vec![fingerprint],
        fingerprint,
        &CoincubeDirectory::new(dir.clone()),
        &Default::default(),
    )
    .await;
    if args[1].starts_with("pair-") {
        assert!(result.is_err(), "injected failure was accepted");
        assert!(
            coincube_gui::phone_signer::pairing_store::load(&desktop_dir)
                .unwrap()
                .phones
                .is_empty()
        );
        println!("PAIRING_FAILED_STORES_UNTRUSTED");
        return;
    }
    let paired = result.unwrap();
    let stored = coincube_gui::phone_signer::pairing_store::load(&desktop_dir).unwrap();
    assert_eq!(stored.phones.len(), 1);
    assert_eq!(
        serde_json::to_value(&stored.phones[0]).unwrap(),
        serde_json::to_value(&paired).unwrap()
    );
    let binding = paired.exact_signer(descriptor).unwrap();
    assert_eq!(binding.key_id, format!("{}", 10 + index));
    assert_eq!(
        binding.xpub,
        fixture["keys"][index]["xpub"].as_str().unwrap()
    );
    let transport =
        PairedTransport::connect(([127, 0, 0, 1], port).into(), &identity, paired.cert_pin)
            .await
            .unwrap();
    let key = DeviceTransportKey::load_or_create(&NetworkDirectory::new(dir)).unwrap();
    let signer = PhoneSigner::new(
        transport,
        fingerprint,
        None,
        paired,
        descriptor.into(),
        Some(Arc::new(key)),
    );
    let mut psbt = Psbt::deserialize(
        &STANDARD
            .decode(fixture["fixtures"][kind]["psbt"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(psbt.inputs[0].tap_key_sig.is_none());
    assert!(psbt.inputs[0].tap_script_sigs.is_empty());
    async_hwi::HWI::sign_tx(&signer, &mut psbt).await.unwrap();
    if index == 0 && kind == "taproot" {
        assert!(psbt.inputs[0].tap_key_sig.is_some());
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());
    } else {
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert_eq!(psbt.inputs[0].tap_script_sigs.len(), 1);
    }
    println!("SIGNED:{}", STANDARD.encode(psbt.serialize()));
}
