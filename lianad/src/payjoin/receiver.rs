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
    persist::{OptionalTransitionOutcome, SessionPersister},
    receive::{
        v2::{
            replay_event_log, Initialized, MaybeInputsOwned, MaybeInputsSeen, OutputsUnknown,
            PayjoinProposal, ProvisionalProposal, ReceiveSession, Receiver,
            UncheckedOriginalPayload, WantsFeeRange, WantsInputs, WantsOutputs,
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

use super::db::{ReceiverPersister, SessionId};

fn read_from_directory(
    receiver: Receiver<Initialized>,
    persister: &ReceiverPersister,
    db_conn: &mut Box<dyn DatabaseConnection>,
    bit: &mut sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), Box<dyn Error>> {
    let (req, context) = receiver
        .create_poll_request(OHTTP_RELAY)
        .map_err(|e| format!("Failed to extract request: {:?}", e))?;

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
    proposal: Receiver<UncheckedOriginalPayload>,
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
    let psbt = proposal.psbt_to_sign();

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
    let psbt = proposal.psbt_to_sign();

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
            proposal
                .finalize_proposal(|_| {
                    let mut psbt = psbt.clone();
                    finalize_psbt(&mut psbt, secp);
                    Ok(psbt)
                })
                .save(persister)?;
        }
    }
    Ok(())
}

fn send_payjoin_proposal(
    proposal: Receiver<PayjoinProposal>,
    persister: &ReceiverPersister,
) -> Result<(), Box<dyn Error>> {
    let (req, ctx) = proposal
        .create_post_request(OHTTP_RELAY)
        .map_err(|e| format!("Failed to extract request: {:?}", e))?;

    log::info!("[Payjoin] Receiver responding to sender...");
    let resp = post_request(req)?;
    proposal
        .process_response(resp.bytes()?.as_ref(), ctx)
        .save(persister)?;
    Ok(())
}

/// Extract the payjoin PSBT's txid from a single serialized `SessionEvent`, if present.
/// Looks at both `AppliedFeeRange` (ProvisionalProposal-era) and `FinalizedProposal`
/// events so we can match sessions that have expired after the PSBT was produced.
fn payjoin_txid_from_event(event_json: &[u8]) -> Option<bitcoin::Txid> {
    let val: serde_json::Value = serde_json::from_slice(event_json).ok()?;
    let obj = val.as_object()?;
    for (key, inner) in obj {
        match key.as_str() {
            "FinalizedProposal" => {
                let psbt: bitcoin::psbt::Psbt = serde_json::from_value(inner.clone()).ok()?;
                return Some(psbt.unsigned_tx.compute_txid());
            }
            "AppliedFeeRange" => {
                let payjoin = inner.get("payjoin_psbt")?.clone();
                let psbt: bitcoin::psbt::Psbt = serde_json::from_value(payjoin).ok()?;
                return Some(psbt.unsigned_tx.compute_txid());
            }
            _ => {}
        }
    }
    None
}

/// Find the receiver session whose payjoin PSBT has the given txid.
/// First attempts replay and inspects the current state; if replay fails
/// (e.g. expired) or yields a state without a PSBT, falls back to scanning
/// raw event JSON for the PSBT so closed and expired sessions still match.
pub(crate) fn find_session_id_by_txid(
    db: &sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    txid: &bitcoin::Txid,
) -> Option<SessionId> {
    let session_ids = {
        let mut db_conn = db.connection();
        db_conn.get_all_receiver_session_ids()
    };
    for session_id in session_ids {
        let persister = ReceiverPersister::from_id(Arc::new(db.clone()), session_id.clone());
        if let Ok((state, _)) = replay_event_log(&persister) {
            let psbt_txid = match &state {
                ReceiveSession::ProvisionalProposal(r) => {
                    Some(r.psbt_to_sign().unsigned_tx.compute_txid())
                }
                ReceiveSession::PayjoinProposal(r) => Some(r.psbt().unsigned_tx.compute_txid()),
                _ => None,
            };
            if psbt_txid.as_ref() == Some(txid) {
                return Some(session_id);
            }
        }
        let events = db.connection().load_receiver_session_events(&session_id);
        if events
            .iter()
            .filter_map(|ev| payjoin_txid_from_event(ev))
            .any(|t| t == *txid)
        {
            return Some(session_id);
        }
    }
    None
}

/// Manually send the payjoin proposal for a given session. If the session is still in the
/// `ProvisionalProposal` state and the stored PSBT is signed, it is finalized first.
pub(crate) fn send_payjoin_for_session(
    db: &sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    session_id: SessionId,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<(), Box<dyn Error>> {
    let persister = ReceiverPersister::from_id(Arc::new(db.clone()), session_id.clone());
    let (state, _) = match replay_event_log(&persister) {
        Ok(v) => v,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("expired") {
                log::info!(
                    "Payjoin session {:?} expired during manual send, marking closed",
                    session_id
                );
                let _ = persister.close();
                return Err("Payjoin session expired.".into());
            }
            return Err(format!("Failed to replay receiver event log: {:?}", e).into());
        }
    };
    let proposal = match state {
        ReceiveSession::PayjoinProposal(proposal) => proposal,
        ReceiveSession::ProvisionalProposal(proposal) => {
            let mut db_conn = db.connection();
            finalize_proposal(proposal, &persister, &mut db_conn, secp)?;
            let (state, _) = replay_event_log(&persister)
                .map_err(|e| format!("Failed to replay receiver event log: {:?}", e))?;
            match state {
                ReceiveSession::PayjoinProposal(proposal) => proposal,
                _ => return Err("PSBT must be signed before sending the payjoin proposal.".into()),
            }
        }
        _ => return Err("Payjoin session is not ready to send.".into()),
    };
    send_payjoin_proposal(proposal, &persister)
}

fn process_receiver_session(
    db_conn: &mut Box<dyn DatabaseConnection>,
    bit: &mut sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    persister: ReceiverPersister,
) -> Result<(), Box<dyn Error>> {
    let (state, _) = replay_event_log(&persister)
        .map_err(|e| format!("Failed to replay receiver event log: {:?}", e))?;

    match state {
        ReceiveSession::Initialized(context) => {
            read_from_directory(context, &persister, db_conn, bit, desc, secp)?;
        }
        ReceiveSession::UncheckedOriginalPayload(proposal) => {
            check_proposal(proposal, &persister, db_conn, bit, desc, secp)?;
        }
        ReceiveSession::MaybeInputsOwned(proposal) => {
            check_inputs_not_owned(proposal, &persister, db_conn, desc, secp)?;
        }
        ReceiveSession::MaybeInputsSeen(proposal) => {
            check_no_inputs_seen_before(proposal, &persister, db_conn, desc, secp)?;
        }
        ReceiveSession::OutputsUnknown(proposal) => {
            identify_receiver_outputs(proposal, &persister, db_conn, desc, secp)?;
        }
        ReceiveSession::WantsOutputs(proposal) => {
            commit_outputs(proposal, &persister, db_conn, desc, secp)?;
        }
        ReceiveSession::WantsInputs(proposal) => {
            contribute_inputs(proposal, &persister, db_conn, desc, secp)?
        }
        ReceiveSession::WantsFeeRange(proposal) => {
            apply_fee_range(proposal, &persister, db_conn, secp)?;
        }
        ReceiveSession::ProvisionalProposal(proposal) => {
            finalize_proposal(proposal, &persister, db_conn, secp)?
        }
        ReceiveSession::PayjoinProposal(_) => {
            log::debug!("[Payjoin] Payjoin proposal ready; awaiting manual send");
        }
        ReceiveSession::Closed(_) | ReceiveSession::HasReplyableError(_) => {
            log::info!("Payjoin session completed or expired, marking as closed");
            persister.close()?;
        }
        ReceiveSession::Monitor(_) => {
            log::debug!("Payjoin session in monitoring state");
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
    let mut db_conn = db.connection();

    for session_id in db_conn.get_all_active_receiver_session_ids() {
        let persister = ReceiverPersister::from_id(Arc::new(db.clone()), session_id.clone());

        match replay_event_log(&persister) {
            Ok(_) => match process_receiver_session(&mut db_conn, bit, desc, secp, persister) {
                Ok(_) => (),
                Err(e) => {
                    log::warn!("process_receiver_session(): {}", e);
                }
            },
            Err(e) => {
                let error_str = e.to_string();
                if error_str.contains("expired") {
                    log::info!(
                        "Payjoin session {:?} expired, marking as closed",
                        session_id
                    );
                    if let Err(close_err) = persister.close() {
                        log::warn!("Failed to close expired payjoin session: {}", close_err);
                    }
                    continue;
                }
                log::warn!("Failed to replay payjoin session {:?}: {}", session_id, e);
            }
        }
    }
}
