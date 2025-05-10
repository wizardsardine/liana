use crate::database::DatabaseInterface;

use crate::payjoin::helpers::post_request;

use std::error::Error;
use std::sync::{self, Arc};

use payjoin::bitcoin::Psbt;
use payjoin::persist::OptionalTransitionOutcome;
use payjoin::send::v2::{replay_event_log, SendSession, V2GetContext};
use payjoin::send::v2::{Sender, WithReplyKey};

use super::db::SenderPersister;
use super::helpers::OHTTP_RELAY;

fn get_proposed_payjoin_psbt(
    context: Sender<V2GetContext>,
    persister: &SenderPersister,
    // TODO: replace with specific error
) -> Result<Option<Psbt>, Box<dyn Error>> {
    let (req, ctx) = context.create_poll_request(OHTTP_RELAY)?;
    match post_request(req) {
        Ok(resp) => {
            let res = context
                .process_response(resp.bytes().expect("Failed to read response").as_ref(), ctx)
                .save(persister);
            match res {
                Ok(OptionalTransitionOutcome::Progress(psbt)) => {
                    log::info!("[Payjoin] ProposalReceived!");
                    Ok(Some(psbt))
                }
                Ok(OptionalTransitionOutcome::Stasis(_current_state)) => {
                    log::info!("[Payjoin] No response yet.");
                    Ok(None)
                }
                Err(e) => {
                    log::error!("{:?}", e);
                    Err(format!("Response error: {}", e).into())
                }
            }
        }
        Err(e) => Err(Box::new(e)),
    }
}

fn post_orginal_proposal(
    sender: Sender<WithReplyKey>,
    persister: &SenderPersister,
) -> Result<(), Box<dyn Error>> {
    let (req, ctx) = sender.create_v2_post_request(OHTTP_RELAY)?;
    match post_request(req) {
        Ok(resp) => {
            log::info!("[Payjoin] Posted original proposal...");
            sender
                .process_response(resp.bytes().expect("Failed to read response").as_ref(), ctx)
                .save(persister)?;
            Ok(())
        }
        Err(e) => Err(Box::new(e)),
    }
}

fn process_sender_session(
    state: SendSession,
    persister: &SenderPersister,
) -> Result<Option<Psbt>, Box<dyn Error>> {
    match state {
        SendSession::WithReplyKey(sender) => {
            log::info!("[Payjoin] SenderState::WithReplyKey");
            match post_orginal_proposal(sender, persister) {
                Ok(_) => {}
                Err(err) => log::warn!("post_orginal_proposal(): {}", err),
            }
            Ok(None)
        }
        SendSession::V2GetContext(context) => {
            log::info!("[Payjoin] SenderState::V2GetContext");
            get_proposed_payjoin_psbt(context, persister)
        }
        SendSession::ProposalReceived(psbt) => {
            log::info!(
                "[Payjoin] SenderState::ProposalReceived: {}",
                psbt.to_string()
            );
            Ok(Some(psbt.clone()))
        }
        _ => Err("Unexpected sender state".into()),
    }
}

pub(crate) fn payjoin_sender_check(db: &sync::Arc<sync::Mutex<dyn DatabaseInterface>>) {
    let mut db_conn = db.connection();
    for session_id in db_conn.get_all_active_sender_session_ids() {
        let persister = SenderPersister::from_id(Arc::new(db.clone()), session_id.clone());

        let (state, session_history) = replay_event_log(&persister)
            .map_err(|e| format!("Failed to replay sender event log: {:?}", e))
            // TODO: handle error
            .unwrap();
        let original_psbt = match session_history.fallback_tx().map(|tx| tx.compute_txid()) {
            Some(txid) => {
                // Get the original psbt so we can restore the input fields
                let original_psbt = db_conn.spend_tx(&txid);
                if original_psbt.is_none() {
                    log::error!("[Payjoin] expecting fallback txid for session={session_id:?}, but none found");
                    return;
                }
                original_psbt.expect("checked above")
            }
            None => {
                log::info!("[Payjoin] No fallback txid found for session={session_id:?}");
                return;
            }
        };

        match process_sender_session(state, &persister) {
            Ok(Some(proposal_psbt)) => {
                let original_txid = original_psbt.unsigned_tx.compute_txid();
                // TODO: should we be deleting the original psbt?  can we fallback without it?
                log::info!("[Payjoin] Deleting original Payjoin psbt (txid={original_txid})");
                db_conn.delete_spend(&original_txid);
                let new_txid = proposal_psbt.unsigned_tx.compute_txid();
                if db_conn.spend_tx(&new_txid).is_some() {
                    log::info!("[Payjoin] Proposal already exists in the db");
                    return;
                }
                log::info!(
                    "[Payjoin] Updating Payjoin psbt: {} -> {}",
                    original_txid,
                    new_txid
                );
                db_conn.store_spend(&proposal_psbt);
                db_conn.save_proposed_payjoin_txid(&session_id, &new_txid);
            }
            Ok(None) => {
                log::info!("[Payjoin] Proposal not received yet...");
            }
            Err(e) => log::warn!("payjoin_sender_check(): {}", e),
        }
    }
}
