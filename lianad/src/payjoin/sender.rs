use crate::database::{DatabaseConnection, DatabaseInterface};

use crate::payjoin::db::SessionId;
use crate::payjoin::helpers::post_request;

use std::error::Error;
use std::sync::{self, Arc};

use payjoin::bitcoin::Psbt;
use payjoin::persist::OptionalTransitionOutcome;
use payjoin::send::v2::{replay_event_log, SendSession, SessionHistory, V2GetContext};
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

fn update_db_with_psbt(
    db_conn: &mut Box<dyn DatabaseConnection>,
    session_history: &SessionHistory,
    session_id: &SessionId,
    psbt: Psbt,
) {
    let original_txid = session_history
        .fallback_tx()
        .map(|tx| tx.compute_txid())
        .expect("fallback tx should be present");

    log::info!("[Payjoin] Deleting original Payjoin psbt (txid={original_txid})");
    db_conn.delete_spend(&original_txid);

    let new_txid = psbt.unsigned_tx.compute_txid();
    if db_conn.spend_tx(&new_txid).is_some() {
        log::info!("[Payjoin] Proposal already exists in the db");
        return;
    }

    log::info!(
        "[Payjoin] Updating Payjoin psbt: {} -> {}",
        original_txid,
        new_txid
    );
    db_conn.store_spend(&psbt);
    db_conn.save_proposed_payjoin_txid(session_id, &new_txid);
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
    db_conn: &mut Box<dyn DatabaseConnection>,
    session_id: SessionId,
    persister: &SenderPersister,
) -> Result<(), Box<dyn Error>> {
    let (state, session_history) = replay_event_log(persister)
        .map_err(|e| format!("Failed to replay sender event log: {:?}", e))?;

    match state {
        SendSession::WithReplyKey(sender) => {
            log::info!("[Payjoin] SenderState::WithReplyKey");
            if let Err(err) = post_orginal_proposal(sender, persister) {
                log::warn!("post_orginal_proposal(): {}", err);
            }
            Ok(())
        }
        SendSession::V2GetContext(context) => {
            log::info!("[Payjoin] SenderState::V2GetContext");
            if let Ok(Some(psbt)) = get_proposed_payjoin_psbt(context, persister) {
                update_db_with_psbt(db_conn, &session_history, &session_id, psbt);
            }
            Ok(())
        }
        SendSession::ProposalReceived(psbt) => {
            log::info!(
                "[Payjoin] SenderState::ProposalReceived: {}",
                psbt.to_string()
            );
            update_db_with_psbt(db_conn, &session_history, &session_id, psbt.clone());
            Ok(())
        }
        _ => Err("Unexpected sender state".into()),
    }
}

pub(crate) fn payjoin_sender_check(db: &sync::Arc<sync::Mutex<dyn DatabaseInterface>>) {
    let mut db_conn = db.connection();
    for session_id in db_conn.get_all_active_sender_session_ids() {
        let persister = SenderPersister::from_id(Arc::new(db.clone()), session_id.clone());
        match process_sender_session(&mut db_conn, session_id, &persister) {
            Ok(_) => (),
            Err(e) => {
                log::warn!("payjoin_sender_check(): {}", e);
                continue;
            }
        }
    }
}
