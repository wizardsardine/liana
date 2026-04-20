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
    /// User-action errors (invite validation, submit failure, revoke,
    /// remove). Cleared by `InviteEmailChanged`, `DismissError`, or a
    /// successful follow-up action.
    pub error: Option<String>,
    /// Fetch-specific errors from `GET /connect/cubes/{id}`. Kept
    /// separate from `error` so:
    ///   * `Enter`'s auto-retry only fires on a prior load failure, not
    ///     on a validation hiccup the user hasn't dismissed yet.
    ///   * Editing the invite-email field doesn't silently clear a
    ///     standing load error.
    pub load_error: Option<String>,
    /// Non-`None` when a `RemoveMember` 409'd with stranded-vault details.
    /// The payload is the raw server message — W4's structured conflict body
    /// is parsed in PR 3, for now we surface the text verbatim.
    pub remove_conflict: Option<String>,
    /// `created_at` of the cube's attached Vault, when one exists. Drives
    /// the W16-desktop "Joined after Vault" badge on each member row:
    /// members whose `joined_at` strictly exceeds this landed after the
    /// Vault's signing quorum was sealed. `None` when the cube has no
    /// vault or the `Loaded` response omitted it.
    pub vault_created_at: Option<String>,
    /// Monotonic counter used to discard stale `Loaded` responses when the
    /// user issues multiple `Reload`s in quick succession.
    load_generation: u32,
    /// `true` once a `Loaded(Ok(_))` has landed. Distinguishes a fresh
    /// panel from a panel that's *successfully* loaded an empty cube —
    /// both have empty `members` and `pending_invites` but only the
    /// former should auto-fetch on `Enter`. Failed loads do not set
    /// this flag, so `Enter` keeps retrying them.
    loaded_once: bool,
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
            // Retry when (a) no successful load has happened yet, or
            // (b) the previous load failed. Gate on `load_error`
            // specifically, not the generic `error` slot — a pending
            // invite-validation message shouldn't trigger a reload.
            // Once a successful load has landed — even for an empty
            // cube — `loaded_once` is true and the Reload button is
            // the explicit "force refresh" path.
            let needs_load = state.load_error.is_some() || !state.loaded_once;
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
                    state.vault_created_at = cube.vault.as_ref().map(|v| v.created_at.clone());
                    state.load_error = None;
                    state.loaded_once = true;
                }
                Err(e) => {
                    // Leave `loaded_once` alone — a failed load doesn't
                    // count as loaded; the next `Enter` will retry.
                    // Route to `load_error` so a standing validation
                    // message in `error` isn't overwritten.
                    // `members`, `pending_invites`, and `vault_created_at`
                    // are all left intact so the panel keeps rendering the
                    // last-good snapshot — including the "Joined after
                    // Vault" badges — instead of a half-cleared view.
                    state.load_error = Some(e);
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
                // Precondition failure on the load path (cube not yet
                // registered) — route to `load_error` so the next
                // `Enter` retries the fetch instead of silently sitting
                // on an action-level error.
                state.load_error = Some("Not ready — the cube is still registering.".to_string());
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
            // Dismiss whichever banner is visible. Note: a load error
            // will simply come back on the next `Enter` / `Reload` if
            // the underlying cause (network, auth, etc.) hasn't been
            // resolved.
            state.error = None;
            state.load_error = None;
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
        // This is a load-path failure (the fetch couldn't even fire)
        // so it belongs on `load_error`. `Enter` uses that slot to
        // decide whether to retry.
        state.load_error = Some("Not ready — the cube is still registering.".to_string());
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
    use crate::services::coincube::{
        ConnectVaultResponse, CubeInviteSummary, CubeMember, CubeMemberUser, VaultStatus,
    };

    /// Builder for a bare `CubeResponse` without a vault — most tests just
    /// exercise the member/invite plumbing and don't care about the W16
    /// badge fields.
    fn sample_cube(
        name: &str,
        members: Vec<CubeMember>,
        pending_invites: Vec<CubeInviteSummary>,
    ) -> CubeResponse {
        CubeResponse {
            id: 42,
            uuid: "abc".to_string(),
            name: name.to_string(),
            network: "bitcoin".to_string(),
            lightning_address: None,
            bolt12_offer: None,
            status: "active".to_string(),
            members,
            pending_invites,
            vault: None,
        }
    }

    fn sample_vault(created_at: &str) -> ConnectVaultResponse {
        ConnectVaultResponse {
            id: 1,
            cube_id: 42,
            timelock_days: 90,
            timelock_expires_at: "2027-01-01T00:00:00Z".to_string(),
            last_reset_at: created_at.to_string(),
            status: VaultStatus::Active,
            members: vec![],
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
        }
    }

    fn sample_member(id: u64, email: &str) -> CubeMember {
        sample_member_joined(id, email, "2026-04-18T00:00:00Z")
    }

    fn sample_member_joined(id: u64, email: &str, joined_at: &str) -> CubeMember {
        CubeMember {
            id,
            user_id: id + 100,
            user: CubeMemberUser {
                email: email.to_string(),
            },
            joined_at: joined_at.to_string(),
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

        let cube = sample_cube(
            "My Cube",
            vec![sample_member(7, "alice@example.com")],
            vec![sample_invite(9, "bob@example.com")],
        );
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

        let stale_cube = sample_cube("Stale", vec![sample_member(1, "stale@example.com")], vec![]);
        run(
            &mut state,
            ConnectCubeMembersMessage::Loaded(Ok(stale_cube), 1),
        );
        assert!(state.members.is_empty());
        assert!(state.loading, "stale response should not clear loading");

        let current_cube = sample_cube(
            "Current",
            vec![sample_member(2, "current@example.com")],
            vec![],
        );
        run(
            &mut state,
            ConnectCubeMembersMessage::Loaded(Ok(current_cube), gen2),
        );
        assert!(!state.loading);
        assert_eq!(state.members.len(), 1);
        assert_eq!(state.members[0].user.email, "current@example.com");
    }

    #[test]
    fn loaded_err_surfaces_on_load_error_not_action_error() {
        // Fetch failures route to `load_error` so `Enter`'s auto-retry
        // fires on them and so they don't collide with any standing
        // user-action error in `state.error`.
        let mut state = ConnectCubeMembersState::new();
        let gen = state.bump_generation();
        state.loading = true;

        run(
            &mut state,
            ConnectCubeMembersMessage::Loaded(Err("boom".to_string()), gen),
        );
        assert!(!state.loading);
        assert_eq!(state.load_error.as_deref(), Some("boom"));
        assert!(state.error.is_none(), "load errors must not touch `error`");
    }

    #[test]
    fn loaded_err_preserves_last_good_snapshot() {
        // Regression: a reload failure must leave `members`,
        // `pending_invites`, and `vault_created_at` intact so the panel
        // keeps rendering the last-good snapshot. Wiping any one of them
        // (in particular `vault_created_at`) breaks the "Joined after
        // Vault" badges on stale member rows — the rows still render,
        // but their badges silently vanish.
        let mut state = ConnectCubeMembersState::new();
        let gen1 = state.bump_generation();
        state.loading = true;
        let late_member = sample_member_joined(7, "late@example.com", "2026-02-01T00:00:00Z");
        let mut cube = sample_cube(
            "Cube",
            vec![late_member.clone()],
            vec![sample_invite(9, "p@example.com")],
        );
        cube.vault = Some(sample_vault("2026-01-01T00:00:00Z"));
        run(
            &mut state,
            ConnectCubeMembersMessage::Loaded(Ok(cube), gen1),
        );
        assert!(badge_visible(&state, &late_member));

        // A subsequent reload fails — stale snapshot must survive.
        let gen2 = state.bump_generation();
        state.loading = true;
        run(
            &mut state,
            ConnectCubeMembersMessage::Loaded(Err("boom".to_string()), gen2),
        );
        assert_eq!(state.load_error.as_deref(), Some("boom"));
        assert_eq!(state.members.len(), 1, "stale member list should survive");
        assert_eq!(
            state.pending_invites.len(),
            1,
            "stale invites should survive"
        );
        assert_eq!(
            state.vault_created_at.as_deref(),
            Some("2026-01-01T00:00:00Z"),
            "stale vault_created_at must survive so badges stay consistent"
        );
        assert!(
            badge_visible(&state, &late_member),
            "badge must still render on the stale row after a failed reload"
        );
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
        state.loaded_once = true;
        state.load_error = Some("previous load failed".to_string());
        assert_eq!(state.load_generation, 0);

        // None/None for client/cube_id — `spawn_load`'s guarded branch
        // replaces `load_error` with the "Not ready" message when the
        // client is missing, so we expect the stale load error to be
        // overwritten — proving the retry path fired.
        run(&mut state, ConnectCubeMembersMessage::Enter);
        assert_ne!(
            state.load_error.as_deref(),
            Some("previous load failed"),
            "Enter should have attempted a fresh load, overwriting the stale error"
        );
    }

    #[test]
    fn enter_does_not_retry_when_only_action_error_is_set() {
        // Regression for the bot-flagged cross-contamination: a
        // pending invite-validation error should NOT trigger an
        // unnecessary network reload on Enter. Only `load_error`
        // counts as a load-level signal.
        let mut state = ConnectCubeMembersState::new();
        state.members = vec![sample_member(7, "alice@example.com")];
        state.loaded_once = true;
        state.error = Some("Please enter a valid email address".to_string());
        let gen_before = state.load_generation;
        let loading_before = state.loading;
        run(&mut state, ConnectCubeMembersMessage::Enter);
        assert_eq!(
            state.load_generation, gen_before,
            "validation error must not trigger a reload"
        );
        assert_eq!(state.loading, loading_before);
        // And the action error survives.
        assert_eq!(
            state.error.as_deref(),
            Some("Please enter a valid email address")
        );
    }

    #[test]
    fn invite_email_change_does_not_clear_load_error() {
        // Regression for the bot-flagged cross-contamination: typing
        // into the invite-email field clears `state.error` (the
        // validation-message slot), but must leave `state.load_error`
        // alone so a standing fetch-failure banner isn't masked.
        let mut state = ConnectCubeMembersState::new();
        state.error = Some("Please enter a valid email address".to_string());
        state.load_error = Some("network unreachable".to_string());
        run(
            &mut state,
            ConnectCubeMembersMessage::InviteEmailChanged("a@b.c".to_string()),
        );
        assert!(state.error.is_none(), "validation error should be cleared");
        assert_eq!(
            state.load_error.as_deref(),
            Some("network unreachable"),
            "load error must survive invite-email edits"
        );
    }

    #[test]
    fn enter_skips_when_loaded_successfully_and_no_error() {
        // Happy cached state: members present, no error, not loading,
        // and — critically — `loaded_once` set by a prior successful
        // load. Enter should be a no-op; Reload is the explicit
        // "force refresh" path.
        let mut state = ConnectCubeMembersState::new();
        state.members = vec![sample_member(7, "alice@example.com")];
        state.error = None;
        state.loaded_once = true;
        let gen_before = state.load_generation;
        let loading_before = state.loading;
        run(&mut state, ConnectCubeMembersMessage::Enter);
        assert_eq!(state.load_generation, gen_before);
        assert_eq!(state.loading, loading_before);
    }

    #[test]
    fn enter_skips_after_successful_empty_load() {
        // Regression: a cube that genuinely has zero members and zero
        // pending invites used to re-fetch on every Enter because the
        // "has loaded" check reused the emptiness of the lists.
        // After a `Loaded(Ok(empty))` lands, `loaded_once` is true and
        // Enter must no-op.
        let mut state = ConnectCubeMembersState::new();
        let gen = state.bump_generation();
        state.loading = true;
        let empty_cube = sample_cube("Empty", vec![], vec![]);
        run(
            &mut state,
            ConnectCubeMembersMessage::Loaded(Ok(empty_cube), gen),
        );
        assert!(state.loaded_once);

        let gen_before = state.load_generation;
        run(&mut state, ConnectCubeMembersMessage::Enter);
        assert_eq!(
            state.load_generation, gen_before,
            "Enter must not spawn a new load after a successful empty load"
        );
        assert!(!state.loading);
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
    fn dismiss_error_clears_both_error_slots() {
        // DismissError clears whichever banner was visible — both the
        // action-error and the load-error slots.
        let mut state = ConnectCubeMembersState::new();
        state.error = Some("action oops".to_string());
        state.load_error = Some("load oops".to_string());
        run(&mut state, ConnectCubeMembersMessage::DismissError);
        assert!(state.error.is_none());
        assert!(state.load_error.is_none());

        state.remove_conflict = Some("vault conflict".to_string());
        run(&mut state, ConnectCubeMembersMessage::DismissRemoveConflict);
        assert!(state.remove_conflict.is_none());
    }

    // --- §3.5 W16-desktop: "Joined after Vault" badge ---

    /// Helper: true when the view-layer would render the "Joined after
    /// Vault" badge for `member`, given the state's cached
    /// `vault_created_at`. Mirrors the predicate used in the view so the
    /// state-layer tests match the render gate exactly.
    fn badge_visible(state: &ConnectCubeMembersState, member: &CubeMember) -> bool {
        state.vault_created_at.as_deref().is_some_and(|v| {
            crate::services::coincube::member_joined_after_vault(&member.joined_at, v)
        })
    }

    #[test]
    fn member_row_renders_joined_after_vault_badge_when_joined_after() {
        let mut state = ConnectCubeMembersState::new();
        let gen = state.bump_generation();
        state.loading = true;
        // Vault sealed on 2026-01-01; member joined after.
        let late_member = sample_member_joined(7, "late@example.com", "2026-02-01T00:00:00Z");
        let mut cube = sample_cube("Cube", vec![late_member.clone()], vec![]);
        cube.vault = Some(sample_vault("2026-01-01T00:00:00Z"));
        run(&mut state, ConnectCubeMembersMessage::Loaded(Ok(cube), gen));
        assert_eq!(
            state.vault_created_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert!(badge_visible(&state, &late_member));
    }

    #[test]
    fn member_row_hides_badge_when_joined_before_vault() {
        let mut state = ConnectCubeMembersState::new();
        let gen = state.bump_generation();
        state.loading = true;
        // Member joined before the Vault was sealed — still in the quorum.
        let early_member = sample_member_joined(7, "early@example.com", "2025-12-01T00:00:00Z");
        let mut cube = sample_cube("Cube", vec![early_member.clone()], vec![]);
        cube.vault = Some(sample_vault("2026-01-01T00:00:00Z"));
        run(&mut state, ConnectCubeMembersMessage::Loaded(Ok(cube), gen));
        assert!(!badge_visible(&state, &early_member));
    }

    #[test]
    fn member_row_hides_badge_when_cube_has_no_vault() {
        let mut state = ConnectCubeMembersState::new();
        let gen = state.bump_generation();
        state.loading = true;
        let member = sample_member_joined(7, "m@example.com", "2026-02-01T00:00:00Z");
        // No vault attached — `vault_created_at` should stay `None`
        // and the badge must not render.
        let cube = sample_cube("Cube", vec![member.clone()], vec![]);
        run(&mut state, ConnectCubeMembersMessage::Loaded(Ok(cube), gen));
        assert!(state.vault_created_at.is_none());
        assert!(!badge_visible(&state, &member));
    }
}
