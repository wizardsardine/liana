use payjoin::{receive::v2::ReceiveSession, receive::v2::SessionOutcome as ReceiveSessionOutcome};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PayjoinStatus {
    Pending,
    WaitingToSign,
    Success,
    Failed,
    Unknown,
}

impl From<ReceiveSession> for PayjoinStatus {
    fn from(session: ReceiveSession) -> Self {
        match session {
            ReceiveSession::Initialized(_)
            | ReceiveSession::UncheckedOriginalPayload(_)
            | ReceiveSession::MaybeInputsOwned(_)
            | ReceiveSession::MaybeInputsSeen(_)
            | ReceiveSession::OutputsUnknown(_)
            | ReceiveSession::WantsOutputs(_)
            | ReceiveSession::WantsInputs(_)
            | ReceiveSession::WantsFeeRange(_) => PayjoinStatus::Pending,
            ReceiveSession::ProvisionalProposal(_) => PayjoinStatus::WaitingToSign,
            ReceiveSession::PayjoinProposal(_) => PayjoinStatus::Success,
            ReceiveSession::HasReplyableError(_) => PayjoinStatus::Failed,
            ReceiveSession::Closed(outcome) => match outcome {
                ReceiveSessionOutcome::Success(_) => PayjoinStatus::Success,
                _ => PayjoinStatus::Failed,
            },
            ReceiveSession::Monitor(_) => PayjoinStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PayjoinInfo {
    pub status: PayjoinStatus,
    pub bip21: String,
}
