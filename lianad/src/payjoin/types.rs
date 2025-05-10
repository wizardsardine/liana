use payjoin::{receive::v2::ReceiveSession, send::v2::SendSession};
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
            ReceiveSession::Uninitialized(_)
            | ReceiveSession::Initialized(_)
            | ReceiveSession::UncheckedProposal(_)
            | ReceiveSession::MaybeInputsOwned(_)
            | ReceiveSession::MaybeInputsSeen(_)
            | ReceiveSession::OutputsUnknown(_)
            | ReceiveSession::WantsOutputs(_)
            | ReceiveSession::WantsInputs(_)
            | ReceiveSession::WantsFeeRange(_) => PayjoinStatus::Pending,
            ReceiveSession::ProvisionalProposal(_) => PayjoinStatus::WaitingToSign,
            ReceiveSession::PayjoinProposal(_) => PayjoinStatus::Success,
            ReceiveSession::TerminalFailure => PayjoinStatus::Failed,
        }
    }
}

// TODO: None of the current states lead to a successful status
impl From<SendSession> for PayjoinStatus {
    fn from(session: SendSession) -> Self {
        match session {
            SendSession::Uninitialized
            | SendSession::WithReplyKey(_)
            | SendSession::V2GetContext(_) => PayjoinStatus::Pending,
            SendSession::ProposalReceived(_) => PayjoinStatus::WaitingToSign,
            SendSession::TerminalFailure => PayjoinStatus::Failed,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PayjoinInfo {
    pub status: PayjoinStatus,
    pub bip21: String,
}
