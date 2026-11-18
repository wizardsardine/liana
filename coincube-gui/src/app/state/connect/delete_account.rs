//! Connect sheet › Account › Delete account.
//!
//! The signed-in user asks COINCUBE to delete their account
//! (`POST /api/v1/user/deletion-request`). An operator completes the deletion
//! later, so the app only records that the request is pending; it never
//! touches the Cubes, wallets or keys on this computer.

use crate::{
    app::view::DeletionFailure,
    services::coincube::{AccountDeletionRequest, CoincubeError},
};

/// Where the server says the user can read what is deleted and what is kept.
pub const DELETE_ACCOUNT_POLICY_URL: &str = "https://coincube.tech/legal/delete-account";

/// What the server reported about this account's deletion request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DeletionStatus {
    /// Not asked yet, or the page is closed.
    #[default]
    Unknown,
    Loading,
    /// No request on record (404): the request button is offered.
    NotRequested,
    /// A request is on record; it replaces the button.
    Pending(AccountDeletionRequest),
    /// The status couldn't be read; the panel error card offers a retry.
    Failed,
}

#[derive(Debug, Default)]
pub struct DeleteAccountState {
    /// The Delete account page is showing instead of the Account overview.
    pub open: bool,
    pub status: DeletionStatus,
    /// The confirm step is showing under the request button.
    pub confirming: bool,
    /// `POST` in flight.
    pub submitting: bool,
    /// The last `POST` failed; the confirm step says so beside its button.
    pub request_failed: bool,
    /// Bumped for each status load and request. A result carrying an older
    /// value belongs to a superseded load, page or session and is dropped.
    pub seq: u64,
}

impl DeleteAccountState {
    /// Opens the page and starts a status load. Returns the load's token.
    pub fn open(&mut self) -> u64 {
        self.open = true;
        self.confirming = false;
        self.submitting = false;
        self.request_failed = false;
        self.begin_status_load()
    }

    pub fn begin_status_load(&mut self) -> u64 {
        self.status = DeletionStatus::Loading;
        self.next()
    }

    /// Leaves the page. A status load still in flight is abandoned so its
    /// result can't put a card on the Account overview; a request already
    /// sent is kept so a success is still recorded.
    pub fn close(&mut self) {
        self.open = false;
        self.confirming = false;
        self.request_failed = false;
        if !self.submitting {
            self.next();
        }
    }

    /// Shows or hides the confirm step. Only offered while no request is on
    /// record and none is being sent.
    pub fn review(&mut self, show: bool) {
        if self.submitting {
            return;
        }
        self.request_failed = false;
        self.confirming = show && self.status == DeletionStatus::NotRequested;
    }

    /// Starts the request when the confirm step is showing. Returns its token,
    /// or `None` when nothing should be sent.
    pub fn begin_request(&mut self) -> Option<u64> {
        if !self.open
            || !self.confirming
            || self.submitting
            || self.status != DeletionStatus::NotRequested
        {
            return None;
        }
        self.submitting = true;
        self.request_failed = false;
        Some(self.next())
    }

    /// Applies a status load. Returns the failure for the caller to present,
    /// or `None` when the result was applied or was stale.
    pub fn status_loaded(
        &mut self,
        result: Result<Option<AccountDeletionRequest>, DeletionFailure>,
        seq: u64,
    ) -> Option<DeletionFailure> {
        if seq != self.seq {
            return None;
        }
        match result {
            Ok(None) => self.status = DeletionStatus::NotRequested,
            Ok(Some(request)) => {
                self.status = DeletionStatus::Pending(request);
                self.confirming = false;
            }
            Err(failure) => {
                self.status = DeletionStatus::Failed;
                return Some(failure);
            }
        }
        None
    }

    /// Applies the result of `POST`. Returns the failure for the caller to
    /// present, or `None` when the result was applied or was stale. A failed
    /// request keeps the confirm step, whose button is the retry.
    pub fn request_finished(
        &mut self,
        result: Result<AccountDeletionRequest, DeletionFailure>,
        seq: u64,
    ) -> Option<DeletionFailure> {
        if seq != self.seq {
            return None;
        }
        self.submitting = false;
        match result {
            Ok(request) => {
                self.status = DeletionStatus::Pending(request);
                self.confirming = false;
                None
            }
            Err(failure) => {
                self.request_failed = true;
                Some(failure)
            }
        }
    }

    /// Forget everything on sign-out; results still in flight are dropped.
    pub fn reset(&mut self) {
        *self = Self {
            seq: self.seq.wrapping_add(1),
            ..Self::default()
        };
    }

    fn next(&mut self) -> u64 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }
}

/// An auth refusal (401/403, `CoincubeError::is_auth_error`) means the session
/// ended and goes to the app's sign-out handling; anything else is shown with
/// the panel's error card.
pub fn deletion_failure(error: &CoincubeError) -> DeletionFailure {
    if error.is_auth_error() {
        DeletionFailure::SessionEnded
    } else {
        DeletionFailure::Other(error.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user_error::UserError;

    fn pending() -> AccountDeletionRequest {
        AccountDeletionRequest {
            status: "pending".into(),
            requested_at: "2026-11-18T12:34:56Z".into(),
            source: "desktop".into(),
        }
    }

    fn other() -> DeletionFailure {
        DeletionFailure::Other(UserError::new("Boom", "Try again.", "CC-TEST", true))
    }

    fn not_requested() -> DeleteAccountState {
        let mut state = DeleteAccountState::default();
        let seq = state.open();
        assert!(state.status_loaded(Ok(None), seq).is_none());
        state
    }

    #[test]
    fn open_loads_status_and_404_offers_the_button() {
        let mut state = DeleteAccountState::default();
        let seq = state.open();
        assert!(state.open);
        assert_eq!(state.status, DeletionStatus::Loading);
        assert!(state.status_loaded(Ok(None), seq).is_none());
        assert_eq!(state.status, DeletionStatus::NotRequested);
    }

    #[test]
    fn pending_status_replaces_the_button() {
        let mut state = DeleteAccountState::default();
        let seq = state.open();
        state.status_loaded(Ok(Some(pending())), seq);
        assert_eq!(state.status, DeletionStatus::Pending(pending()));
        state.review(true);
        assert!(!state.confirming, "no confirm step once a request exists");
        assert!(state.begin_request().is_none());
    }

    #[test]
    fn stale_status_is_dropped() {
        let mut state = DeleteAccountState::default();
        let first = state.open();
        let second = state.begin_status_load();
        assert!(state.status_loaded(Ok(Some(pending())), first).is_none());
        assert_eq!(state.status, DeletionStatus::Loading);
        state.status_loaded(Ok(None), second);
        assert_eq!(state.status, DeletionStatus::NotRequested);
    }

    #[test]
    fn status_failure_is_returned_and_marked() {
        let mut state = DeleteAccountState::default();
        let seq = state.open();
        let failure = state.status_loaded(Err(other()), seq);
        assert!(matches!(failure, Some(DeletionFailure::Other(_))));
        assert_eq!(state.status, DeletionStatus::Failed);
    }

    #[test]
    fn request_needs_the_confirm_step() {
        let mut state = not_requested();
        assert!(state.begin_request().is_none(), "button alone never sends");
        state.review(true);
        assert!(state.confirming);
        let seq = state.begin_request().expect("confirmed request is sent");
        assert!(state.submitting);
        assert!(state.begin_request().is_none(), "no double submit");
        state.review(false);
        assert!(state.confirming, "cancel is ignored while sending");
        assert!(state.request_finished(Ok(pending()), seq).is_none());
        assert!(!state.submitting);
        assert!(!state.confirming);
        assert_eq!(state.status, DeletionStatus::Pending(pending()));
    }

    #[test]
    fn failed_request_keeps_the_confirm_step_for_retry() {
        let mut state = not_requested();
        state.review(true);
        let seq = state.begin_request().unwrap();
        let failure = state.request_finished(Err(other()), seq);
        assert!(matches!(failure, Some(DeletionFailure::Other(_))));
        assert!(!state.submitting);
        assert!(state.confirming);
        assert_eq!(state.status, DeletionStatus::NotRequested);
        assert!(state.begin_request().is_some());
    }

    #[test]
    fn closed_page_sends_nothing() {
        let mut state = not_requested();
        state.review(true);
        state.close();
        assert!(!state.confirming);
        assert!(state.begin_request().is_none());
    }

    #[test]
    fn reset_drops_in_flight_results() {
        let mut state = not_requested();
        state.review(true);
        let seq = state.begin_request().unwrap();
        state.reset();
        assert!(!state.open);
        assert!(state.request_finished(Ok(pending()), seq).is_none());
        assert_eq!(state.status, DeletionStatus::Unknown);
        assert!(!state.submitting);
    }

    #[test]
    fn closing_mid_load_drops_the_late_result() {
        let mut state = DeleteAccountState::default();
        let seq = state.open();
        state.close();
        assert!(state.status_loaded(Err(other()), seq).is_none());
        assert_ne!(state.status, DeletionStatus::Failed);
    }

    #[test]
    fn closing_while_sending_still_records_the_request() {
        let mut state = not_requested();
        state.review(true);
        let seq = state.begin_request().unwrap();
        state.close();
        assert!(state.request_finished(Ok(pending()), seq).is_none());
        assert_eq!(state.status, DeletionStatus::Pending(pending()));
    }

    #[test]
    fn failed_request_is_flagged_until_the_next_attempt() {
        let mut state = not_requested();
        state.review(true);
        let seq = state.begin_request().unwrap();
        state.request_finished(Err(other()), seq);
        assert!(state.request_failed);
        state.begin_request().unwrap();
        assert!(!state.request_failed);
    }

    #[test]
    fn auth_refusals_end_the_session() {
        let error = |status_code| {
            CoincubeError::Unsuccessful(crate::services::http::NotSuccessResponseInfo {
                status_code,
                text: String::new(),
            })
        };
        for status in [401, 403] {
            assert!(matches!(
                deletion_failure(&error(status)),
                DeletionFailure::SessionEnded
            ));
        }
        for status in [400, 404, 500] {
            assert!(matches!(
                deletion_failure(&error(status)),
                DeletionFailure::Other(_)
            ));
        }
    }
}
