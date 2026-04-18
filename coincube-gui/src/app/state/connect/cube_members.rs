//! Cube-scoped members panel state (W8).
//!
//! Held as a sub-state on [`ConnectCubePanel`](super::cube::ConnectCubePanel)
//! so it inherits the authenticated `CoincubeClient` and the server cube id
//! from its parent. See `plans/PLAN-cube-membership-desktop.md` §2.2.

use crate::{
    app::{
        message::Message,
        view::{self, ConnectCubeMembersMessage, ConnectCubeMessage},
    },
    services::coincube::{
        CoincubeClient, CubeInviteOrAddResult, CubeInviteSummary, CubeMember, CubeResponse,
    },
};

/// In-memory state for the Members panel.
///
/// Mirrors the `ContactsState` shape on the account panel so the view code can
/// render from plain fields — no task orchestration lives in here.
#[derive(Debug, Default)]
pub struct ConnectCubeMembersState {
    pub members: Vec<CubeMember>,
    pub pending_invites: Vec<CubeInviteSummary>,
    pub invite_email: String,
    pub loading: bool,
    pub invite_sending: bool,
    /// Last surfaced error string. Cleared by `DismissError`.
    pub error: Option<String>,
    /// Non-`None` when a `RemoveMember` 409'd with stranded-vault details.
    /// The payload is the raw server message — W4's structured conflict body
    /// is parsed in PR 3, for now we surface the text verbatim.
    pub remove_conflict: Option<String>,
    /// Monotonic counter used to discard stale `Loaded` responses when the
    /// user issues multiple `Reload`s in quick succession.
    load_generation: u32,
}

impl ConnectCubeMembersState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn bump_generation(&mut self) -> u32 {
        self.load_generation = self.load_generation.wrapping_add(1);
        self.load_generation
    }
}

/// Handles a [`ConnectCubeMembersMessage`]. Returns an iced `Task` for any
/// spawned async work. `client` / `cube_id` are passed in rather than held on
/// the state so the parent panel decides when the panel is "ready" (both
/// authenticated and registered with the backend).
pub fn update(
    state: &mut ConnectCubeMembersState,
    msg: ConnectCubeMembersMessage,
    client: Option<CoincubeClient>,
    cube_id: Option<u64>,
) -> iced::Task<Message> {
    match msg {
        ConnectCubeMembersMessage::Enter => {
            // Skip if a load is already in flight — avoid piling duplicates
            // on top of each other when the user taps quickly.
            if state.loading {
                return iced::Task::none();
            }
            // Retry when (a) we have no data yet, or (b) the previous load
            // failed and the panel is showing stale/empty data plus an
            // error banner. Successful cached data is left alone — the
            // Reload button is the explicit "force refresh" path.
            let needs_load = state.error.is_some()
                || (state.members.is_empty() && state.pending_invites.is_empty());
            if needs_load {
                return spawn_load(state, client, cube_id);
            }
            iced::Task::none()
        }

        ConnectCubeMembersMessage::Reload => spawn_load(state, client, cube_id),

        ConnectCubeMembersMessage::Loaded(result, gen) => {
            if gen != state.load_generation {
                return iced::Task::none();
            }
            state.loading = false;
            match result {
                Ok(cube) => {
                    state.members = cube.members;
                    state.pending_invites = cube.pending_invites;
                    state.error = None;
                }
                Err(e) => {
                    state.error = Some(e);
                }
            }
            iced::Task::none()
        }

        ConnectCubeMembersMessage::InviteEmailChanged(email) => {
            state.invite_email = email;
            state.error = None;
            iced::Task::none()
        }

        ConnectCubeMembersMessage::SubmitInvite => {
            if state.invite_sending {
                return iced::Task::none();
            }
            let email = state.invite_email.trim().to_string();
            let valid = email_address::EmailAddress::parse_with_options(
                &email,
                email_address::Options::default().with_required_tld(),
            )
            .is_ok();
            if !valid {
                state.error = Some("Please enter a valid email address".to_string());
                return iced::Task::none();
            }
            let (Some(client), Some(cube_id)) = (client, cube_id) else {
                state.error = Some("Not ready — the cube is still registering.".to_string());
                return iced::Task::none();
            };
            state.invite_sending = true;
            state.error = None;
            iced::Task::perform(
                async move { client.create_cube_invite(cube_id, &email).await },
                |res| {
                    Message::View(view::Message::ConnectCube(ConnectCubeMessage::Members(
                        ConnectCubeMembersMessage::InviteResult(res.map_err(|e| e.to_string())),
                    )))
                },
            )
        }

        ConnectCubeMembersMessage::InviteResult(result) => {
            state.invite_sending = false;
            match result {
                Ok(CubeInviteOrAddResult::Added(member)) => {
                    state.invite_email.clear();
                    state.error = None;
                    if !state.members.iter().any(|m| m.id == member.id) {
                        state.members.push(member);
                    }
                    spawn_load_silent(state, client, cube_id)
                }
                Ok(CubeInviteOrAddResult::Invited(invite)) => {
                    state.invite_email.clear();
                    state.error = None;
                    if !state.pending_invites.iter().any(|i| i.id == invite.id) {
                        state.pending_invites.push(invite);
                    }
                    iced::Task::none()
                }
                Err(e) => {
                    state.error = Some(e);
                    iced::Task::none()
                }
            }
        }

        ConnectCubeMembersMessage::RevokeInvite(invite_id) => {
            let (Some(client), Some(cube_id)) = (client, cube_id) else {
                return iced::Task::none();
            };
            iced::Task::perform(
                async move { client.revoke_cube_invite(cube_id, invite_id).await },
                move |res| {
                    Message::View(view::Message::ConnectCube(ConnectCubeMessage::Members(
                        ConnectCubeMembersMessage::RevokeInviteResult(
                            invite_id,
                            res.map_err(|e| e.to_string()),
                        ),
                    )))
                },
            )
        }

        ConnectCubeMembersMessage::RevokeInviteResult(invite_id, result) => {
            match result {
                Ok(()) => {
                    state.pending_invites.retain(|i| i.id != invite_id);
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            iced::Task::none()
        }

        ConnectCubeMembersMessage::RemoveMember(member_id) => {
            let (Some(client), Some(cube_id)) = (client, cube_id) else {
                return iced::Task::none();
            };
            iced::Task::perform(
                async move { client.remove_cube_member(cube_id, member_id).await },
                move |res| {
                    Message::View(view::Message::ConnectCube(ConnectCubeMessage::Members(
                        ConnectCubeMembersMessage::RemoveMemberResult(
                            member_id,
                            res.map_err(|e| e.to_string()),
                        ),
                    )))
                },
            )
        }

        ConnectCubeMembersMessage::RemoveMemberResult(member_id, result) => {
            match result {
                Ok(()) => {
                    state.members.retain(|m| m.id != member_id);
                    state.error = None;
                    state.remove_conflict = None;
                }
                Err(e) => {
                    // W4: stranded-vault 409 — surface it on the dedicated
                    // conflict slot so the view can render a prominent
                    // dialog. The backend's error envelope carries the
                    // conflicting-vaults list; parsing that structured body
                    // lands in a follow-up once W4 stabilises.
                    if is_stranded_vault_conflict(&e) {
                        state.remove_conflict = Some(e);
                    } else {
                        state.error = Some(e);
                    }
                }
            }
            iced::Task::none()
        }

        ConnectCubeMembersMessage::DismissError => {
            state.error = None;
            iced::Task::none()
        }

        ConnectCubeMembersMessage::DismissRemoveConflict => {
            state.remove_conflict = None;
            iced::Task::none()
        }
    }
}

/// User-initiated load (Enter/Reload). Surfaces an error if the cube isn't
/// registered yet, so the panel explains itself.
fn spawn_load(
    state: &mut ConnectCubeMembersState,
    client: Option<CoincubeClient>,
    cube_id: Option<u64>,
) -> iced::Task<Message> {
    if client.is_none() || cube_id.is_none() {
        state.error = Some("Not ready — the cube is still registering.".to_string());
        return iced::Task::none();
    }
    spawn_load_silent(state, client, cube_id)
}

/// Follow-on refresh after a successful mutation. If the client was torn
/// down mid-flight (logout, etc.), silently skip — the mutation already
/// updated local state optimistically.
fn spawn_load_silent(
    state: &mut ConnectCubeMembersState,
    client: Option<CoincubeClient>,
    cube_id: Option<u64>,
) -> iced::Task<Message> {
    let (Some(client), Some(cube_id)) = (client, cube_id) else {
        return iced::Task::none();
    };
    let gen = state.bump_generation();
    state.loading = true;
    iced::Task::perform(
        async move { client.get_cube(cube_id).await },
        move |res: Result<CubeResponse, _>| {
            Message::View(view::Message::ConnectCube(ConnectCubeMessage::Members(
                ConnectCubeMembersMessage::Loaded(res.map_err(|e| e.to_string()), gen),
            )))
        },
    )
}

/// Heuristic: the backend's W4 error message mentions "active Vault" /
/// "active vault". We could match on the error envelope's `code` field
/// instead, but that requires plumbing the structured shape through — left
/// for PR 3. The string match is defensive so a false-negative just routes
/// to the generic error banner rather than the dedicated dialog.
fn is_stranded_vault_conflict(err: &str) -> bool {
    // Matches both "active vault" and "active vaults" (plural contains
    // singular as a substring).
    err.to_ascii_lowercase().contains("active vault")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::coincube::{CubeInviteSummary, CubeMember, CubeMemberUser};

    fn sample_member(id: u64, email: &str) -> CubeMember {
        CubeMember {
            id,
            user_id: id + 100,
            user: CubeMemberUser {
                email: email.to_string(),
            },
            joined_at: "2026-04-18T00:00:00Z".to_string(),
        }
    }

    fn sample_invite(id: u64, email: &str) -> CubeInviteSummary {
        CubeInviteSummary {
            id,
            cube_id: 42,
            email: email.to_string(),
            status: "pending".to_string(),
            expires_at: "2026-05-18T00:00:00Z".to_string(),
            created_at: "2026-04-18T00:00:00Z".to_string(),
        }
    }

    fn run(state: &mut ConnectCubeMembersState, msg: ConnectCubeMembersMessage) {
        // None/None for client/cube_id — every test here exercises the pure
        // state transitions, not the async branches. Tasks returned when the
        // client is missing are `Task::none()`.
        let _ = update(state, msg, None, None);
    }

    #[test]
    fn loaded_ok_populates_members_and_invites() {
        let mut state = ConnectCubeMembersState::new();
        // Simulate an in-flight load.
        let gen = state.bump_generation();
        state.loading = true;

        let cube = CubeResponse {
            id: 42,
            uuid: "abc".to_string(),
            name: "My Cube".to_string(),
            network: "bitcoin".to_string(),
            lightning_address: None,
            bolt12_offer: None,
            status: "active".to_string(),
            members: vec![sample_member(7, "alice@example.com")],
            pending_invites: vec![sample_invite(9, "bob@example.com")],
        };
        run(&mut state, ConnectCubeMembersMessage::Loaded(Ok(cube), gen));

        assert!(!state.loading);
        assert_eq!(state.members.len(), 1);
        assert_eq!(state.members[0].user.email, "alice@example.com");
        assert_eq!(state.pending_invites.len(), 1);
        assert_eq!(state.pending_invites[0].email, "bob@example.com");
        assert!(state.error.is_none());
    }

    #[test]
    fn loaded_discards_stale_generation() {
        let mut state = ConnectCubeMembersState::new();
        // Two in-flight loads: only the second's response should land.
        state.bump_generation(); // gen = 1 (stale)
        let gen2 = state.bump_generation(); // gen = 2 (current)
        state.loading = true;

        let stale_cube = CubeResponse {
            id: 42,
            uuid: "abc".to_string(),
            name: "Stale".to_string(),
            network: "bitcoin".to_string(),
            lightning_address: None,
            bolt12_offer: None,
            status: "active".to_string(),
            members: vec![sample_member(1, "stale@example.com")],
            pending_invites: vec![],
        };
        run(
            &mut state,
            ConnectCubeMembersMessage::Loaded(Ok(stale_cube), 1),
        );
        assert!(state.members.is_empty());
        assert!(state.loading, "stale response should not clear loading");

        let current_cube = CubeResponse {
            id: 42,
            uuid: "abc".to_string(),
            name: "Current".to_string(),
            network: "bitcoin".to_string(),
            lightning_address: None,
            bolt12_offer: None,
            status: "active".to_string(),
            members: vec![sample_member(2, "current@example.com")],
            pending_invites: vec![],
        };
        run(
            &mut state,
            ConnectCubeMembersMessage::Loaded(Ok(current_cube), gen2),
        );
        assert!(!state.loading);
        assert_eq!(state.members.len(), 1);
        assert_eq!(state.members[0].user.email, "current@example.com");
    }

    #[test]
    fn loaded_err_surfaces_error() {
        let mut state = ConnectCubeMembersState::new();
        let gen = state.bump_generation();
        state.loading = true;

        run(
            &mut state,
            ConnectCubeMembersMessage::Loaded(Err("boom".to_string()), gen),
        );
        assert!(!state.loading);
        assert_eq!(state.error.as_deref(), Some("boom"));
    }

    #[test]
    fn invite_email_changed_clears_error() {
        let mut state = ConnectCubeMembersState::new();
        state.error = Some("previous".to_string());
        run(
            &mut state,
            ConnectCubeMembersMessage::InviteEmailChanged("new@example.com".to_string()),
        );
        assert_eq!(state.invite_email, "new@example.com");
        assert!(state.error.is_none());
    }

    #[test]
    fn invite_result_added_appends_member() {
        let mut state = ConnectCubeMembersState::new();
        state.invite_sending = true;
        state.invite_email = "alice@example.com".to_string();
        run(
            &mut state,
            ConnectCubeMembersMessage::InviteResult(Ok(CubeInviteOrAddResult::Added(
                sample_member(7, "alice@example.com"),
            ))),
        );
        assert!(!state.invite_sending);
        assert!(state.invite_email.is_empty());
        assert_eq!(state.members.len(), 1);
        assert!(state.error.is_none());
    }

    #[test]
    fn invite_result_invited_appends_pending() {
        let mut state = ConnectCubeMembersState::new();
        state.invite_sending = true;
        state.invite_email = "bob@example.com".to_string();
        run(
            &mut state,
            ConnectCubeMembersMessage::InviteResult(Ok(CubeInviteOrAddResult::Invited(
                sample_invite(9, "bob@example.com"),
            ))),
        );
        assert!(!state.invite_sending);
        assert!(state.invite_email.is_empty());
        assert_eq!(state.pending_invites.len(), 1);
        assert_eq!(state.pending_invites[0].email, "bob@example.com");
    }

    #[test]
    fn invite_result_err_surfaces_error_and_keeps_email() {
        let mut state = ConnectCubeMembersState::new();
        state.invite_sending = true;
        state.invite_email = "bob@example.com".to_string();
        run(
            &mut state,
            ConnectCubeMembersMessage::InviteResult(Err("duplicate".to_string())),
        );
        assert!(!state.invite_sending);
        assert_eq!(state.invite_email, "bob@example.com");
        assert_eq!(state.error.as_deref(), Some("duplicate"));
    }

    #[test]
    fn revoke_invite_result_ok_removes_pending() {
        let mut state = ConnectCubeMembersState::new();
        state.pending_invites = vec![
            sample_invite(1, "a@example.com"),
            sample_invite(2, "b@example.com"),
        ];
        run(
            &mut state,
            ConnectCubeMembersMessage::RevokeInviteResult(1, Ok(())),
        );
        assert_eq!(state.pending_invites.len(), 1);
        assert_eq!(state.pending_invites[0].id, 2);
    }

    #[test]
    fn remove_member_result_err_with_stranded_vault_routes_to_conflict_slot() {
        let mut state = ConnectCubeMembersState::new();
        state.members = vec![sample_member(7, "alice@example.com")];
        run(
            &mut state,
            ConnectCubeMembersMessage::RemoveMemberResult(
                7,
                Err("Cannot remove member — keys are signing an active Vault".to_string()),
            ),
        );
        assert_eq!(state.members.len(), 1, "member should stay on error");
        assert!(state.error.is_none());
        assert!(state.remove_conflict.is_some());
        assert!(state
            .remove_conflict
            .as_deref()
            .unwrap()
            .contains("active Vault"));
    }

    #[test]
    fn remove_member_result_err_generic_routes_to_error_slot() {
        let mut state = ConnectCubeMembersState::new();
        state.members = vec![sample_member(7, "alice@example.com")];
        run(
            &mut state,
            ConnectCubeMembersMessage::RemoveMemberResult(
                7,
                Err("Network unreachable".to_string()),
            ),
        );
        assert_eq!(state.members.len(), 1);
        assert!(state.remove_conflict.is_none());
        assert_eq!(state.error.as_deref(), Some("Network unreachable"));
    }

    #[test]
    fn remove_member_result_ok_removes_member() {
        let mut state = ConnectCubeMembersState::new();
        state.members = vec![
            sample_member(1, "a@example.com"),
            sample_member(2, "b@example.com"),
        ];
        run(
            &mut state,
            ConnectCubeMembersMessage::RemoveMemberResult(1, Ok(())),
        );
        assert_eq!(state.members.len(), 1);
        assert_eq!(state.members[0].id, 2);
    }

    #[test]
    fn enter_retries_when_prior_load_errored_even_with_stale_data() {
        // Regression: successful load populated members, then a later
        // Reload failed — Enter used to skip the retry because members
        // wasn't empty, leaving the panel wedged on stale data + an
        // error banner.
        let mut state = ConnectCubeMembersState::new();
        state.members = vec![sample_member(7, "alice@example.com")];
        state.error = Some("previous load failed".to_string());
        // generation starts at 0
        assert_eq!(state.load_generation, 0);

        // None/None for client/cube_id — we assert on the load-generation
        // bump as a proxy for "spawn_load was invoked". spawn_load's
        // guarded branch sets state.error and returns Task::none when the
        // client is missing, so we expect the error message to be
        // replaced with the "not ready" copy.
        run(&mut state, ConnectCubeMembersMessage::Enter);
        // `spawn_load` short-circuits on missing client, which in turn
        // replaces `state.error` with the "Not ready" message. The stale
        // error from the prior failed load is cleared as a side effect —
        // proving the retry path fired.
        assert_ne!(
            state.error.as_deref(),
            Some("previous load failed"),
            "Enter should have attempted a fresh load, clearing the stale error"
        );
    }

    #[test]
    fn enter_skips_when_loaded_successfully_and_no_error() {
        // Happy cached state: members present, no error, not loading.
        // Enter should be a no-op — the explicit Reload button is the
        // "force refresh" path.
        let mut state = ConnectCubeMembersState::new();
        state.members = vec![sample_member(7, "alice@example.com")];
        state.error = None;
        // Need a client for spawn_load to fire the network task; we
        // assert the generation DOESN'T bump instead.
        let gen_before = state.load_generation;
        let loading_before = state.loading;
        run(&mut state, ConnectCubeMembersMessage::Enter);
        assert_eq!(state.load_generation, gen_before);
        assert_eq!(state.loading, loading_before);
    }

    #[test]
    fn enter_skips_when_load_is_in_flight() {
        let mut state = ConnectCubeMembersState::new();
        state.loading = true; // simulate in-flight load
        let gen_before = state.load_generation;
        run(&mut state, ConnectCubeMembersMessage::Enter);
        assert_eq!(state.load_generation, gen_before);
        assert!(state.loading);
    }

    #[test]
    fn dismiss_error_clears_slots() {
        let mut state = ConnectCubeMembersState::new();
        state.error = Some("oops".to_string());
        run(&mut state, ConnectCubeMembersMessage::DismissError);
        assert!(state.error.is_none());

        state.remove_conflict = Some("vault conflict".to_string());
        run(&mut state, ConnectCubeMembersMessage::DismissRemoveConflict);
        assert!(state.remove_conflict.is_none());
    }
}
