use std::{
    collections::HashMap,
    error::Error,
    sync::{self, Arc},
};

use liana::{descriptors, spend::AddrInfo};

use payjoin::{
    bitcoin::{
        self, consensus::encode::serialize_hex, psbt::Input, secp256k1, OutPoint, Sequence, TxIn,
        Weight,
    },
    persist::OptionalTransitionOutcome,
    receive::{
        v2::{
            replay_event_log, Initialized, MaybeInputsOwned, MaybeInputsSeen, OutputsUnknown,
            PayjoinProposal, ProvisionalProposal, ReceiveSession, Receiver, UncheckedProposal,
            WantsFeeRange, WantsInputs, WantsOutputs,
        },
        InputPair,
    },
    ImplementationError,
};

use crate::{
    bitcoin::BitcoinInterface,
    database::{Coin, CoinStatus, DatabaseConnection, DatabaseInterface},
    payjoin::helpers::{finalize_psbt, post_request, OHTTP_RELAY},
};

use super::db::ReceiverPersister;

fn read_from_directory(
    receiver: Receiver<Initialized>,
    persister: &ReceiverPersister,
    db_conn: &mut Box<dyn DatabaseConnection>,
    bit: &mut sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), Box<dyn Error>> {
    let mut receiver = receiver;
    let (req, context) = receiver
        .create_poll_request(OHTTP_RELAY)
        .expect("Failed to extract request");
    let proposal = match post_request(req.clone()) {
        Ok(ohttp_response) => {
            let response_bytes = ohttp_response.bytes()?;
            let state_transition = receiver
                .process_response(response_bytes.as_ref(), context)
                .save(persister);
            match state_transition {
                Ok(OptionalTransitionOutcome::Progress(next_state)) => next_state,
                Ok(OptionalTransitionOutcome::Stasis(_current_state)) => {
                    return Err("NoResults".into())
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(e) => return Err(Box::new(e)),
    };
    check_proposal(proposal, persister, db_conn, bit, desc, secp)
}

fn check_proposal(
    proposal: Receiver<UncheckedProposal>,
    persister: &ReceiverPersister,
    db_conn: &mut Box<dyn DatabaseConnection>,
    bit: &mut sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), Box<dyn Error>> {
    // Receive Check 1: Can Broadcast
    let proposal = proposal
        .check_broadcast_suitability(None, |tx| {
            let result = bit.test_mempool_accept(vec![serialize_hex(tx)]);
            match result.first().cloned() {
                Some(can_broadcast) => Ok(can_broadcast),
                None => Ok(false),
            }
        })
        .save(persister)?;
    check_inputs_not_owned(proposal, persister, db_conn, desc, secp)
}

fn check_inputs_not_owned(
    proposal: Receiver<MaybeInputsOwned>,
    persister: &ReceiverPersister,
    db_conn: &mut Box<dyn DatabaseConnection>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), Box<dyn Error>> {
    let proposal = proposal
        .check_inputs_not_owned(&mut |script| {
            let address =
                bitcoin::Address::from_script(script, db_conn.network()).map_err(|e| {
                    ImplementationError::from(Box::new(e) as Box<dyn Error + Send + Sync>)
                })?;
            Ok(db_conn
                .derivation_index_by_address(&address)
                .map(|(index, is_change)| AddrInfo { index, is_change })
                .is_some())
        })
        .save(persister)?;
    check_no_inputs_seen_before(proposal, persister, db_conn, desc, secp)
}

fn check_no_inputs_seen_before(
    proposal: Receiver<MaybeInputsSeen>,
    persister: &ReceiverPersister,
    db_conn: &mut Box<dyn DatabaseConnection>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), Box<dyn Error>> {
    let proposal = proposal
        .check_no_inputs_seen_before(&mut |outpoint| {
            let seen = db_conn.insert_input_seen_before(&[*outpoint]);
            Ok(seen)
        })
        .save(persister)?;
    identify_receiver_outputs(proposal, persister, db_conn, desc, secp)
}

fn identify_receiver_outputs(
    proposal: Receiver<OutputsUnknown>,
    persister: &ReceiverPersister,
    db_conn: &mut Box<dyn DatabaseConnection>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), Box<dyn Error>> {
    log::debug!("[Payjoin] receiver outputs");
    let proposal = proposal
        .identify_receiver_outputs(&mut |script| {
            let address =
                bitcoin::Address::from_script(script, db_conn.network()).map_err(|e| {
                    ImplementationError::from(Box::new(e) as Box<dyn Error + Send + Sync>)
                })?;
            Ok(db_conn
                .derivation_index_by_address(&address)
                .map(|(index, is_change)| AddrInfo { index, is_change })
                .is_some())
        })
        .save(persister)?;
    commit_outputs(proposal, persister, db_conn, desc, secp)
}

fn commit_outputs(
    proposal: Receiver<WantsOutputs>,
    persister: &ReceiverPersister,
    db_conn: &mut Box<dyn DatabaseConnection>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), Box<dyn Error>> {
    let proposal = proposal.commit_outputs().save(persister)?;
    contribute_inputs(proposal, persister, db_conn, desc, secp)
}

fn contribute_inputs(
    proposal: Receiver<WantsInputs>,
    persister: &ReceiverPersister,
    db_conn: &mut Box<dyn DatabaseConnection>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), Box<dyn Error>> {
    let coins = db_conn.coins(&[CoinStatus::Confirmed], &[]);

    let mut candidate_inputs_map = HashMap::<OutPoint, (Coin, TxIn, Input, Weight)>::new();
    for (outpoint, coin) in coins.iter() {
        let txs = db_conn.list_wallet_transactions(&[outpoint.txid]);
        let (db_tx, _, _) = txs
            .first()
            .expect("There should be at least tx in the wallet");

        let tx = db_tx.clone();

        let txout = tx.tx_out(outpoint.vout as usize)?.clone();

        let derived_desc = if coin.is_change {
            desc.change_descriptor().derive(coin.derivation_index, secp)
        } else {
            desc.receive_descriptor()
                .derive(coin.derivation_index, secp)
        };

        let txin = TxIn {
            previous_output: *outpoint,
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            ..Default::default()
        };

        let mut psbtin = Input {
            non_witness_utxo: Some(tx.clone()),
            witness_utxo: Some(txout.clone()),
            ..Default::default()
        };

        derived_desc.update_psbt_in(&mut psbtin);
        // TODO: revisit using primary path boolean. Perphaps we should use both paths and take the max.
        let worse_case_weight = Weight::from_wu_usize(desc.max_sat_weight(true))
            // Segwit marker
            + Weight::from_wu(2)
            // Non-witness data size
            + Weight::from_non_witness_data_size(txin.base_size() as u64);

        candidate_inputs_map.insert(*outpoint, (*coin, txin, psbtin, worse_case_weight));
    }

    let mut candidate_inputs = candidate_inputs_map
        .values()
        .map(|(_, txin, psbtin, weight)| {
            InputPair::new(txin.clone(), psbtin.clone(), Some(*weight)).unwrap()
        });
    log::info!("[Payjoin] Candidate inputs: {:?}", candidate_inputs);

    if candidate_inputs.len() == 0 {
        return Err("No candidate inputs".into());
    }

    let selected_input = proposal
        .try_preserving_privacy(candidate_inputs.clone())
        .unwrap_or(
            candidate_inputs
                .next()
                .expect("Should have at least one input")
                .clone(),
        );

    let proposal = proposal
        .contribute_inputs(vec![selected_input])?
        .commit_inputs()
        .save(persister)?;

    apply_fee_range(proposal, persister, db_conn, secp)?;
    Ok(())
}

fn apply_fee_range(
    proposal: Receiver<WantsFeeRange>,
    persister: &ReceiverPersister,
    db_conn: &mut Box<dyn DatabaseConnection>,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), Box<dyn Error>> {
    let proposal = proposal.apply_fee_range(None, None).save(persister)?;
    let (_, session_history) = replay_event_log(persister)?;
    let psbt = session_history
        .psbt_ready_for_signing()
        .expect("Just added fee applied psbt");

    let txid = psbt.unsigned_tx.compute_txid();
    db_conn.save_receiver_session_original_txid(&persister.session_id, &txid);
    db_conn.store_spend(&psbt);
    log::info!("[Payjoin] PSBT in the DB...");

    finalize_proposal(proposal, persister, db_conn, secp)?;
    Ok(())
}

fn finalize_proposal(
    proposal: Receiver<ProvisionalProposal>,
    persister: &ReceiverPersister,
    db_conn: &mut Box<dyn DatabaseConnection>,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), Box<dyn Error>> {
    let (_, session_history) = replay_event_log(persister)?;
    let psbt = session_history
        .psbt_ready_for_signing()
        .expect("Just added fee applied psbt");

    let txid = psbt.unsigned_tx.compute_txid();
    if let Some(psbt) = db_conn.spend_tx(&txid) {
        let mut is_signed = false;
        for psbtin in &psbt.inputs {
            if !psbtin.partial_sigs.is_empty() {
                log::debug!("[Payjoin] PSBT is signed!");
                is_signed = true;
                break;
            }
        }

        if is_signed {
            let proposal = proposal
                .finalize_proposal(|_| {
                    let mut psbt = psbt.clone();
                    finalize_psbt(&mut psbt, secp);
                    Ok(psbt)
                })
                .save(persister)?;

            let proposed_txid = proposal.psbt().unsigned_tx.compute_txid();
            db_conn.save_receiver_session_proposed_txid(&persister.session_id, &proposed_txid);
            send_payjoin_proposal(proposal, persister)?;
        }
    }
    Ok(())
}

fn send_payjoin_proposal(
    mut proposal: Receiver<PayjoinProposal>,
    persister: &ReceiverPersister,
) -> Result<(), Box<dyn Error>> {
    let (req, ctx) = proposal
        .create_post_request(OHTTP_RELAY)
        .expect("Failed to extract request");

    // Respond to sender
    log::info!("[Payjoin] Receiver responding to sender...");
    match post_request(req) {
        Ok(resp) => {
            proposal
                .process_response(resp.bytes().expect("Failed to read response").as_ref(), ctx)
                .save(persister)?;
        }
        Err(err) => log::error!("[Payjoin] send_payjoin_proposal(): {}", err),
    }
    Ok(())
}

fn process_receiver_session(
    db: &sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    bit: &mut sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), Box<dyn Error>> {
    let mut db_conn = db.connection();
    for session_id in db_conn.get_all_active_receiver_session_ids() {
        let persister = ReceiverPersister::from_id(Arc::new(db.clone()), session_id.clone());

        let (state, _) = replay_event_log(&persister)
            .map_err(|e| format!("Failed to replay receiver event log: {:?}", e))?;

        match state {
            ReceiveSession::Initialized(context) => {
                read_from_directory(context, &persister, &mut db_conn, bit, desc, secp)?;
            }
            ReceiveSession::UncheckedProposal(proposal) => {
                check_proposal(proposal, &persister, &mut db_conn, bit, desc, secp)?;
            }
            ReceiveSession::MaybeInputsOwned(proposal) => {
                check_inputs_not_owned(proposal, &persister, &mut db_conn, desc, secp)?;
            }
            ReceiveSession::MaybeInputsSeen(proposal) => {
                check_no_inputs_seen_before(proposal, &persister, &mut db_conn, desc, secp)?;
            }
            ReceiveSession::OutputsUnknown(proposal) => {
                identify_receiver_outputs(proposal, &persister, &mut db_conn, desc, secp)?;
            }
            ReceiveSession::WantsOutputs(proposal) => {
                commit_outputs(proposal, &persister, &mut db_conn, desc, secp)?;
            }
            ReceiveSession::WantsInputs(proposal) => {
                contribute_inputs(proposal, &persister, &mut db_conn, desc, secp)?
            }
            ReceiveSession::ProvisionalProposal(proposal) => {
                finalize_proposal(proposal, &persister, &mut db_conn, secp)?
            }
            ReceiveSession::PayjoinProposal(proposal) => {
                send_payjoin_proposal(proposal, &persister)?
            }
            _ => return Err(format!("Unexpected receiver state: {:?}", state).into()),
        }
    }
    Ok(())
}

pub(crate) fn payjoin_receiver_check(
    db: &sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    bit: &mut sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) {
    match process_receiver_session(db, bit, desc, secp) {
        Ok(_) => (),
        Err(e) => log::warn!("process_receiver_session(): {}", e),
    }
}
