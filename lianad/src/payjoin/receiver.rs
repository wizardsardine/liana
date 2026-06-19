use std::{
    collections::{HashMap, HashSet},
    error::Error,
    sync::{self, Arc},
};

use liana::descriptors;

use payjoin::{
    bitcoin::{
        self, consensus::encode::serialize_hex, psbt::Input, secp256k1, FeeRate, OutPoint,
        Sequence, TxIn, Weight,
    },
    persist::{OptionalTransitionOutcome, SessionPersister},
    receive::{
        v2::{
            replay_event_log, Initialized, MaybeInputsOwned, MaybeInputsSeen, OutputsUnknown,
            PayjoinProposal, ProvisionalProposal, ReceiveSession, Receiver, SessionEvent,
            UncheckedOriginalPayload, WantsFeeRange, WantsInputs, WantsOutputs,
        },
        InputPair,
    },
};

use crate::{
    bitcoin::BitcoinInterface,
    database::{Coin, CoinStatus, DatabaseConnection, DatabaseInterface},
    payjoin::helpers::{finalize_psbt, post_request, with_relay_fallback, RelayAttempt},
};

use super::db::{ReceiverPersister, SessionId};

/// Maximum derivation index to check when verifying whether a script belongs to the wallet.
/// Addresses derived beyond this index will not be recognized as owned, so this must be
/// large enough to cover any address the sender could reasonably use.  1000 matches
/// bitcoind's default gap limit and is cheap to derive (~2000 EC multiplications).
const MAX_OWNED_INDEX: u32 = 1000;

fn read_from_directory(
    receiver: Receiver<Initialized>,
    persister: &ReceiverPersister,
    db_conn: &mut Box<dyn DatabaseConnection>,
    bit: &mut sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ohttp_relays: &[String],
) -> Result<(), Box<dyn Error>> {
    let outcome =
        with_relay_fallback(ohttp_relays, "no ohttp relays configured".into(), |relay| {
            let (req, context) = match receiver.create_poll_request(relay) {
                Ok(v) => v,
                Err(e) => return RelayAttempt::Retry(format!("create_poll_request: {e:?}").into()),
            };
            match post_request(req) {
                Ok(ohttp_response) => {
                    let response_bytes = match ohttp_response.bytes() {
                        Ok(b) => b,
                        Err(e) => return RelayAttempt::Retry(Box::new(e) as Box<dyn Error>),
                    };
                    let state_transition = receiver
                        .clone()
                        .process_response(response_bytes.as_ref(), context)
                        .save(persister);
                    match state_transition {
                        Ok(outcome) => RelayAttempt::Ok(outcome),
                        Err(e) => RelayAttempt::Fatal(e.into()),
                    }
                }
                Err(e) => RelayAttempt::Retry(Box::new(e) as Box<dyn Error>),
            }
        })?;
    let proposal = match outcome {
        OptionalTransitionOutcome::Progress(next_state) => next_state,
        OptionalTransitionOutcome::Stasis(_current_state) => return Err("NoResults".into()),
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
    // Receive Check 1: Can Broadcast, enforce wallet min fee rate (1 sat/vb)
    let proposal = proposal
        .check_broadcast_suitability(FeeRate::from_sat_per_vb(1), |tx| {
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
    let mut owned_scripts = HashSet::new();
    for i in 0..MAX_OWNED_INDEX {
        owned_scripts.insert(
            desc.receive_descriptor()
                .derive(i.into(), secp)
                .script_pubkey(),
        );
        owned_scripts.insert(
            desc.change_descriptor()
                .derive(i.into(), secp)
                .script_pubkey(),
        );
    }
    let proposal = proposal
        .check_inputs_not_owned(&mut |script| Ok(owned_scripts.contains(script)))
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
            let seen = db_conn.insert_input_seen_before(outpoint);
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
    let mut owned_scripts = HashSet::new();
    for i in 0..MAX_OWNED_INDEX {
        owned_scripts.insert(
            desc.receive_descriptor()
                .derive(i.into(), secp)
                .script_pubkey(),
        );
        owned_scripts.insert(
            desc.change_descriptor()
                .derive(i.into(), secp)
                .script_pubkey(),
        );
    }
    let proposal = proposal
        .identify_receiver_outputs(&mut |script| Ok(owned_scripts.contains(script)))
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
    ohttp_relays: &[String],
) -> Result<(), Box<dyn Error>> {
    with_relay_fallback(ohttp_relays, "no ohttp relays configured".into(), |relay| {
        let (req, ctx) = match proposal.create_post_request(relay) {
            Ok(v) => v,
            Err(e) => return RelayAttempt::Retry(format!("create_post_request: {e:?}").into()),
        };
        log::info!("[Payjoin] Receiver responding to sender via {}...", relay);
        let resp = match post_request(req) {
            Ok(r) => r,
            Err(e) => return RelayAttempt::Retry(Box::new(e) as Box<dyn Error>),
        };
        let bytes = match resp.bytes() {
            Ok(b) => b,
            Err(e) => return RelayAttempt::Retry(Box::new(e) as Box<dyn Error>),
        };
        match proposal
            .clone()
            .process_response(bytes.as_ref(), ctx)
            .save(persister)
        {
            Ok(_) => RelayAttempt::Ok(()),
            Err(e) => RelayAttempt::Fatal(e.into()),
        }
    })
}

/// Extract the payjoin PSBT's txid from a `SessionEvent`, if the event carries one.
/// Covers `AppliedFeeRange` (ProvisionalProposal-era) and `FinalizedProposal`
/// (PayjoinProposal-era / Monitor-era) — these together span every state in
/// which a payjoin PSBT exists, so closed, expired, and monitoring sessions
/// all match by txid.
fn payjoin_txid_from_event(event: &SessionEvent) -> Option<bitcoin::Txid> {
    match event {
        SessionEvent::FinalizedProposal(psbt) => Some(psbt.unsigned_tx.compute_txid()),
        SessionEvent::AppliedFeeRange(ctx) => {
            // PsbtContext::payjoin_psbt is private; round-trip via the
            // crate's own serde to extract the field by name.
            let v = serde_json::to_value(ctx).ok()?;
            let psbt: bitcoin::psbt::Psbt =
                serde_json::from_value(v.get("payjoin_psbt")?.clone()).ok()?;
            Some(psbt.unsigned_tx.compute_txid())
        }
        _ => None,
    }
}

/// Find the receiver session whose payjoin PSBT has the given txid, returning
/// the session id along with its receive-address derivation index.
/// First attempts replay and inspects the current state; if replay fails
/// (e.g. expired) or yields a state whose PSBT isn't directly accessible
/// (Monitor's `psbt_context` is private upstream), falls back to scanning
/// the typed event log so monitoring, closed, and expired sessions still
/// match.
pub(crate) fn find_receiver_session_by_txid(
    db: &sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    txid: &bitcoin::Txid,
) -> Option<(SessionId, u32)> {
    let sessions = {
        let mut db_conn = db.connection();
        db_conn.get_all_receiver_sessions()
    };
    for (session_id, derivation_index) in sessions {
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
                return Some((session_id, derivation_index));
            }
        }
        if let Ok(events) = persister.load() {
            if events
                .filter_map(|ev| payjoin_txid_from_event(&ev))
                .any(|t| t == *txid)
            {
                return Some((session_id, derivation_index));
            }
        }
    }
    None
}

/// Cancel the payjoin receiver session associated with the given txid and return the
/// sender's original (non-payjoin) transaction, if one was ever received. Closes the
/// session via the persister as a terminal transition.
pub(crate) fn cancel_payjoin_for_session(
    db: &sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    session_id: SessionId,
) -> Result<Option<bitcoin::Transaction>, Box<dyn Error>> {
    let persister = ReceiverPersister::from_id(Arc::new(db.clone()), session_id.clone());
    let (state, _) = match replay_event_log(&persister) {
        Ok(v) => v,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("expired") {
                log::info!(
                    "Payjoin session {:?} expired during cancel, marking closed",
                    session_id
                );
                let _ = persister.close();
                return Err("Payjoin session expired.".into());
            }
            return Err(format!("Failed to replay receiver event log: {e:?}").into());
        }
    };
    let fallback = match state {
        ReceiveSession::Initialized(r) => {
            r.cancel().save(&persister)?;
            None
        }
        ReceiveSession::UncheckedOriginalPayload(r) => {
            r.cancel().save(&persister)?;
            None
        }
        ReceiveSession::MaybeInputsOwned(r) => {
            Some(r.cancel().save(&persister)?.fallback_tx().clone())
        }
        ReceiveSession::MaybeInputsSeen(r) => {
            Some(r.cancel().save(&persister)?.fallback_tx().clone())
        }
        ReceiveSession::OutputsUnknown(r) => {
            Some(r.cancel().save(&persister)?.fallback_tx().clone())
        }
        ReceiveSession::WantsOutputs(r) => Some(r.cancel().save(&persister)?.fallback_tx().clone()),
        ReceiveSession::WantsInputs(r) => Some(r.cancel().save(&persister)?.fallback_tx().clone()),
        ReceiveSession::WantsFeeRange(r) => {
            Some(r.cancel().save(&persister)?.fallback_tx().clone())
        }
        ReceiveSession::ProvisionalProposal(r) => {
            Some(r.cancel().save(&persister)?.fallback_tx().clone())
        }
        ReceiveSession::PayjoinProposal(r) => {
            Some(r.cancel().save(&persister)?.fallback_tx().clone())
        }
        ReceiveSession::HasReplyableError(r) => r
            .cancel()
            .save(&persister)?
            .map(|pf| pf.fallback_tx().clone()),
        ReceiveSession::Monitor(r) => Some(r.cancel().save(&persister)?.fallback_tx().clone()),
        ReceiveSession::PendingFallback(r) => {
            let tx = r.fallback_tx().clone();
            r.close().save(&persister)?;
            Some(tx)
        }
        ReceiveSession::Closed(_) => return Err("Payjoin session already closed.".into()),
    };
    Ok(fallback)
}

/// Manually send the payjoin proposal for a given session. If the session is still in the
/// `ProvisionalProposal` state and the stored PSBT is signed, it is finalized first.
pub(crate) fn send_payjoin_for_session(
    db: &sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    session_id: SessionId,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ohttp_relays: &[String],
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
            return Err(format!("Failed to replay receiver event log: {e:?}").into());
        }
    };
    let proposal = match state {
        ReceiveSession::PayjoinProposal(proposal) => proposal,
        ReceiveSession::ProvisionalProposal(proposal) => {
            let mut db_conn = db.connection();
            finalize_proposal(proposal, &persister, &mut db_conn, secp)?;
            let (state, _) = replay_event_log(&persister)
                .map_err(|e| format!("Failed to replay receiver event log: {e:?}"))?;
            match state {
                ReceiveSession::PayjoinProposal(proposal) => proposal,
                _ => return Err("PSBT must be signed before sending the payjoin proposal.".into()),
            }
        }
        _ => return Err("Payjoin session is not ready to send.".into()),
    };
    send_payjoin_proposal(proposal, &persister, ohttp_relays)
}

fn process_receiver_session(
    state: ReceiveSession,
    db_conn: &mut Box<dyn DatabaseConnection>,
    bit: &mut sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    persister: ReceiverPersister,
    ohttp_relays: &[String],
) -> Result<(), Box<dyn Error>> {
    match state {
        ReceiveSession::Initialized(context) => {
            read_from_directory(context, &persister, db_conn, bit, desc, secp, ohttp_relays)?;
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
        ReceiveSession::Closed(_)
        | ReceiveSession::HasReplyableError(_)
        | ReceiveSession::PendingFallback(_) => {
            log::info!("Payjoin session completed or expired, marking as closed");
            persister.close()?;
        }
        ReceiveSession::Monitor(monitor) => {
            let bit = bit.clone();
            monitor
                .check_payment(|txid| {
                    let tx_opt = bit
                        .lock()
                        .expect("BitcoinInterface mutex poisoned")
                        .wallet_transaction(&txid)
                        .map(|(tx, _)| tx);
                    Ok(tx_opt)
                })
                .save(&persister)?;
        }
    }
    Ok(())
}

pub(crate) fn payjoin_receiver_check(
    db: &sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    bit: &mut sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    desc: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ohttp_relays: &[String],
) {
    let mut db_conn = db.connection();

    for session_id in db_conn.get_all_active_receiver_session_ids() {
        let persister = ReceiverPersister::from_id(Arc::new(db.clone()), session_id.clone());

        let (state, _) = match replay_event_log(&persister) {
            Ok(v) => v,
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("expired") {
                    log::info!(
                        "Payjoin session {:?} expired, marking as closed",
                        session_id
                    );
                    let _ = persister.close();
                } else {
                    log::error!("Failed to replay payjoin session {:?}: {}", session_id, e);
                }
                continue;
            }
        };

        if let Err(e) = process_receiver_session(
            state,
            &mut db_conn,
            bit,
            desc,
            secp,
            persister,
            ohttp_relays,
        ) {
            log::error!("process_receiver_session(): {}", e);
        }
    }
}
