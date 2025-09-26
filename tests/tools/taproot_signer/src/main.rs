//! A quick and dirty program which reads a PSBT and an xpriv from stdin and outputs the signed
//! PSBT to stdout. Uses function copied from Liana's hot signer and adapted.

use std::{
    env,
    io::{self, Write},
    str::FromStr,
};

use bitcoin::{
    self,
    bip32::{self, Xpriv},
    hashes::Hash,
    key::TapTweak,
    psbt::{Input as PsbtIn, Psbt},
    secp256k1, sighash,
};

fn sign_taproot(
    secp: &secp256k1::Secp256k1<secp256k1::All>,
    sighash_cache: &mut sighash::SighashCache<&bitcoin::Transaction>,
    master_xpriv: Xpriv,
    master_fingerprint: bip32::Fingerprint,
    prevouts: &[bitcoin::TxOut],
    psbt_in: &mut PsbtIn,
    input_index: usize,
) {
    let sig_type = sighash::TapSighashType::Default;
    let prevouts = sighash::Prevouts::All(prevouts);

    // If the details of the internal key are filled, provide a keypath signature.
    if let Some(ref int_key) = psbt_in.tap_internal_key {
        // NB: we don't check for empty leaf hashes on purpose, in case the internal key also
        // appears in a leaf.
        if let Some((_, (fg, der_path))) = psbt_in.tap_key_origins.get(int_key) {
            if *fg == master_fingerprint {
                let privkey = master_xpriv.derive_priv(secp, der_path).unwrap().to_priv();
                let keypair = secp256k1::Keypair::from_secret_key(secp, &privkey.inner);
                assert_eq!(keypair.x_only_public_key().0, *int_key);
                let keypair = keypair.tap_tweak(secp, psbt_in.tap_merkle_root).to_inner();
                let sighash = sighash_cache
                    .taproot_key_spend_signature_hash(input_index, &prevouts, sig_type)
                    .unwrap();
                let sighash = secp256k1::Message::from_digest_slice(sighash.as_byte_array())
                    .expect("Sighash is always 32 bytes.");
                let sig = secp.sign_schnorr_no_aux_rand(&sighash, &keypair);
                let sig = bitcoin::taproot::Signature {
                    sig,
                    hash_ty: sig_type,
                };
                psbt_in.tap_key_sig = Some(sig);
            }
        }
    }

    // Now sign for all the public keys derived from our master secret, in all the leaves where
    // they are present.
    for (pubkey, (leaf_hashes, (fg, der_path))) in &psbt_in.tap_key_origins {
        if *fg != master_fingerprint {
            continue;
        }

        for leaf_hash in leaf_hashes {
            let privkey = master_xpriv.derive_priv(secp, der_path).unwrap().to_priv();
            let keypair = secp256k1::Keypair::from_secret_key(secp, &privkey.inner);
            let sighash = sighash_cache
                .taproot_script_spend_signature_hash(input_index, &prevouts, *leaf_hash, sig_type)
                .unwrap();
            let sighash = secp256k1::Message::from_digest_slice(sighash.as_byte_array())
                .expect("Sighash is always 32 bytes.");
            let sig = secp.sign_schnorr_no_aux_rand(&sighash, &keypair);
            let sig = bitcoin::taproot::Signature {
                sig,
                hash_ty: sig_type,
            };
            psbt_in.tap_script_sigs.insert((*pubkey, *leaf_hash), sig);
        }
    }
}

fn sign_psbt(psbt: &mut Psbt, master_xpriv: Xpriv, secp: &secp256k1::Secp256k1<secp256k1::All>) {
    let mut sighash_cache = sighash::SighashCache::new(&psbt.unsigned_tx);
    let master_fingerprint = master_xpriv.fingerprint(secp);

    let prevouts: Vec<_> = psbt
        .inputs
        .iter()
        .filter_map(|psbt_in| psbt_in.witness_utxo.clone())
        .collect();
    assert_eq!(prevouts.len(), psbt.inputs.len());

    // Sign each input in the PSBT.
    for i in 0..psbt.inputs.len() {
        sign_taproot(
            secp,
            &mut sighash_cache,
            master_xpriv,
            master_fingerprint,
            &prevouts,
            &mut psbt.inputs[i],
            i,
        );
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    assert_eq!(args.len(), 3);

    let mut psbt = Psbt::from_str(&args[1]).unwrap();
    let xprv = Xpriv::from_str(&args[2]).unwrap();

    let secp = secp256k1::Secp256k1::new();
    sign_psbt(&mut psbt, xprv, &secp);

    let psbt_str = psbt.to_string();
    print!("{}", psbt_str);
    io::stdout().flush().unwrap();
}
