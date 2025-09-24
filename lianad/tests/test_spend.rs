#![cfg(not(target_os = "windows"))]

mod common;

use lianad::commands::CreateSpendResult;
use miniscript::bitcoin::address::NetworkUnchecked;
use miniscript::bitcoin::{secp256k1, Address, Amount};
use std::collections::HashMap;

use crate::common::bitcoind::{generate_blocks, new_address_unchecked, setup_bitcoind};
use crate::common::env::{node_kind, use_taproot};
use crate::common::lianad::LianaD;
use crate::common::utils::wait_for;

#[ignore]
#[test]
fn test_spend_change() {
    // We can spend a coin that was received on a change address.

    let bitcoind = setup_bitcoind().unwrap();
    let lianad = LianaD::new_single_sig(&bitcoind, node_kind(), use_taproot());
    let control = lianad.control();

    assert!(wait_for(
        || control.get_info().block_height as u64 == bitcoind.client.get_block_count().unwrap().0
    ));

    // Receive a coin on a fresh receive address.
    let addr = control.get_new_address().address;
    let txid = bitcoind
        .client
        .send_to_address(&addr, Amount::from_sat(1_000_000))
        .unwrap()
        .txid()
        .unwrap();

    generate_blocks(&bitcoind.client, 1, &[txid]);

    assert!(wait_for(|| control.list_coins(&[], &[]).coins.len() == 1));

    // Create a transaction that will spend this coin to:
    // 1. one of our receive addresses
    // 2. an external address
    // 3. one of our change addresses
    let outpoints: Vec<_> = control
        .list_coins(&[], &[])
        .coins
        .iter()
        .map(|c| c.outpoint)
        .collect();
    assert_eq!(outpoints.len(), 1);

    let destinations = HashMap::<Address<NetworkUnchecked>, u64>::from([
        (new_address_unchecked(&bitcoind.client), 100_000),
        (control.get_new_address().address.into_unchecked(), 100_000),
    ]);

    let res = control
        .create_spend(&destinations, &outpoints, 2, None)
        .unwrap();
    let CreateSpendResult::Success {
        psbt: spend_psbt,
        warnings,
    } = res
    else {
        panic!("expected successful spend creation, got {:?}", res);
    };

    // The transaction must contain a change output.
    assert_eq!(spend_psbt.outputs.len(), 3);
    assert_eq!(spend_psbt.unsigned_tx.output.len(), 3);
    assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);

    // Sign and broadcast this first spend transaction.
    let secp = secp256k1::Secp256k1::new();
    let signed_psbt = lianad.desc.sign_psbt(spend_psbt, &secp, 0, None).unwrap();
    control.update_spend(signed_psbt.clone()).unwrap();
    let spend_txid = signed_psbt.unsigned_tx.compute_txid();
    control.broadcast_spend(&spend_txid).unwrap();
    generate_blocks(&bitcoind.client, 1, &[spend_txid]);
    assert!(wait_for(|| control.list_coins(&[], &[]).coins.len() == 3));

    // Now create a new transaction that spends the change output as well as
    // the output sent to the receive address.
    let outpoints: Vec<_> = control
        .list_coins(&[], &[])
        .coins
        .iter()
        .filter_map(|c| c.spend_info.as_ref().is_none().then_some(c.outpoint))
        .collect();
    assert_eq!(outpoints.len(), 2);
    let destinations = HashMap::<Address<NetworkUnchecked>, u64>::from([(
        new_address_unchecked(&bitcoind.client),
        100_000,
    )]);

    let res = control
        .create_spend(&destinations, &outpoints, 2, None)
        .unwrap();
    let CreateSpendResult::Success {
        psbt: spend_psbt,
        warnings,
    } = res
    else {
        panic!("expected successful spend creation, got {:?}", res);
    };
    assert_eq!(spend_psbt.outputs.len(), 2);
    assert_eq!(spend_psbt.unsigned_tx.output.len(), 2);
    assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);

    // We can sign and broadcast it.
    let signed_psbt = lianad.desc.sign_psbt(spend_psbt, &secp, 0, None).unwrap();
    control.update_spend(signed_psbt.clone()).unwrap();
    let spend_txid = signed_psbt.unsigned_tx.compute_txid();
    control.broadcast_spend(&spend_txid).unwrap();
    generate_blocks(&bitcoind.client, 1, &[spend_txid]);
}
