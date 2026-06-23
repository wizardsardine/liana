//! Heir "Recover a Vault" discovery surface (COIN-377, PR 1).
//!
//! A **global, pre-cube** surface on the launcher/home screen: it lists the
//! Vaults the signed-in account is a keyholder/beneficiary of (but does not
//! own) and, once a vault's recovery window is open on-chain, lets the heir
//! start recovery. It is reachable even if the heir has no Vault of their own
//! (master decision #6) and is gated behind [`crate::feature_flags`]'s
//! `RECOVER_VAULT_ENABLED` (the surface stays dark until the API's
//! `/connect/cubes/recoverable` endpoint and the COIN-376 sweep ship).
//!
//! This module owns the discovery state + view. It depends on the tested
//! Connect-client foundation:
//! [`CoincubeClient::list_recoverable_vaults`](crate::services::coincube::CoincubeClient::list_recoverable_vaults)
//! for the list and
//! [`fetch_recovery_descriptor`](crate::services::recovery::fetch_recovery_descriptor)
//! for the keyholder release. The release returns a plaintext descriptor; the
//! watch-only install + bridge into the recovery screen is PR 2 / PR 3 (see the
//! `TODO(PR 2)` seam in [`RecoverVaultPanel::update`]).

use iced::widget::{scrollable, Space};
use iced::{Alignment, Length, Task};

use coincube_ui::{
    component::{button as btn, card, text::*},
    theme,
    widget::{Column, Container, Element, Row},
};

use crate::services::coincube::{
    CoincubeClient, RecoverableVault, RecoveryState, VaultMonitoringLevel,
};
use crate::services::recovery::fetch_recovery_descriptor;

/// Shown when the heir isn't signed in: both for a `Load` attempted without a
/// Connect client and for the reset (`Idle`) state the panel returns to after
/// logout — Home may still be parked on this section, and `Idle` must read as a
/// sign-in prompt rather than a perpetual "Loading…" with no request in flight.
const SIGNED_OUT_PROMPT: &str = "Sign in to your account to see vaults you can recover.";

/// How a discovery row should present, encoding invariants I1 (state gating)
/// and I7 (tier honesty). Password-required (Heartbeat) rows take precedence:
/// they are never actionable in v1 regardless of window state, and must show
/// the deferred-path copy rather than a broken "Recover" button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// Window open **and** Full-tier (password-free) **and** the caller is a
    /// keyholder → the "Recover" button is live.
    Actionable,
    /// Window open, but the caller is a beneficiary, not a keyholder → the
    /// descriptor pull-down is keyholder-only (the release endpoint 403s
    /// non-keyholders), so show "a keyholder completes this", never a button.
    KeyholderOnly,
    /// Window not open yet (Full-tier) → visible but not actionable; show the
    /// expected-open date and "we'll email you".
    NotYetOpen,
    /// Heartbeat tier (`requires_recovery_password`) → deferred to COIN-375;
    /// show the "recovery password required — coming later" copy, never a
    /// button. Takes precedence over the window state.
    PasswordDeferred,
}

/// Classifies a recoverable-vault row. Pure — the unit tests below pin the
/// invariants. Heartbeat/password-required is checked first so an open
/// (`available`/`reminding`) Heartbeat vault still routes to the deferred copy;
/// then the keyholder gate, so an open vault the caller can't pull down (they're
/// a beneficiary) shows the keyholder-only copy rather than a 403'ing button.
pub fn classify(v: &RecoverableVault) -> RowKind {
    if v.requires_recovery_password {
        RowKind::PasswordDeferred
    } else if v.recovery_state().is_open() {
        if v.is_keyholder() {
            RowKind::Actionable
        } else {
            RowKind::KeyholderOnly
        }
    } else {
        RowKind::NotYetOpen
    }
}

/// The user-facing status line for a non-actionable row (invariant I7 copy).
/// Returns `None` for an actionable row (it gets a button, not a status line).
pub fn status_copy(v: &RecoverableVault) -> Option<String> {
    match classify(v) {
        RowKind::Actionable => None,
        RowKind::KeyholderOnly => {
            Some("Recovery is open — a keyholder of this vault can complete it.".to_string())
        }
        RowKind::PasswordDeferred => {
            Some("Recovery password required — coming in a later update.".to_string())
        }
        RowKind::NotYetOpen => Some(match v.expected_open_at {
            Some(at) => format!(
                "Recovery isn't open yet — expected around {}. We'll email you.",
                at.format("%-d %b %Y")
            ),
            None => "Recovery isn't open yet — we'll email you when it is.".to_string(),
        }),
    }
}

/// Short, display-only label for a vault's monitoring tier.
fn tier_label(level: VaultMonitoringLevel) -> &'static str {
    match level {
        VaultMonitoringLevel::Full => "Full monitoring",
        VaultMonitoringLevel::Heartbeat => "Alerts only",
        VaultMonitoringLevel::Off => "Not monitored",
    }
}

/// Short, display-only label for a vault's recovery-window state.
fn state_label(state: RecoveryState) -> &'static str {
    match state {
        RecoveryState::Open => "Recovery open",
        RecoveryState::Approaching => "Approaching",
    }
}

/// In-flight status for the row the heir clicked "Recover" on.
#[derive(Debug, Clone)]
pub enum RecoverStatus {
    /// Pulling the descriptor from the keyholder release endpoint.
    Fetching,
    /// Descriptor fetched. PR 2 picks up here (watch-only install).
    Ready,
    /// Gate/error copy (already neutralised by `KeyholderRecoveryError`'s
    /// `Display`, so it is safe to show verbatim — never explains duress).
    Failed(String),
}

/// Loading state for the discovery list.
#[derive(Debug, Clone, Default)]
pub enum ListState {
    /// Not yet requested (e.g. before the section is opened).
    #[default]
    Idle,
    /// Fetch in flight. Carries the Connect session generation it was fired in
    /// so a stale `Loaded` only clears *its own* dead load, never a newer
    /// in-flight one from a later session.
    Loading(u64),
    /// Fetched rows (possibly empty).
    Loaded(Vec<RecoverableVault>),
    /// Fetch failed — message is display-safe.
    Error(String),
}

/// State for the discovery surface.
#[derive(Debug, Clone, Default)]
pub struct RecoverVaultPanel {
    list: ListState,
    /// The cube currently being recovered, with its inline status.
    active: Option<(u64, RecoverStatus)>,
}

/// Messages for the discovery surface. Home forwards these via
/// `ViewMessage::RecoverVault(..)` and maps the returned task back.
#[derive(Debug, Clone)]
pub enum RecoverVaultMessage {
    /// (Re)fetch the recoverable-vault list.
    Load,
    /// List fetch resolved. The `u64` is the Connect session generation the
    /// fetch was fired in — a stale value (logout / account switch landed
    /// first) is dropped rather than painting the prior account's vaults.
    Loaded(Result<Vec<RecoverableVault>, String>, u64),
    /// Heir clicked "Recover" on the given cube — pull its descriptor.
    Recover(u64),
    /// Descriptor fetch resolved (cube id, plaintext descriptor | display copy,
    /// session generation the fetch was fired in). The `u64` lets a fetch from a
    /// prior session (account switch, same cube) be dropped instead of painting
    /// its result onto the new session's row.
    Fetched(u64, Result<String, String>, u64),
}

impl RecoverVaultPanel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether Home should skip re-firing `Load` when the section is reopened.
    /// True only while a fetch is in flight (`Loading`) or has succeeded
    /// (`Loaded`) — so a successful list isn't re-fetched on every reopen, and
    /// an in-flight fetch isn't duplicated. `Idle` (never requested) and
    /// `Error` (a transient failure) both return false so the next reopen
    /// retries instead of leaving the heir stuck on a stale error.
    pub fn is_loaded(&self) -> bool {
        matches!(self.list, ListState::Loading(_) | ListState::Loaded(_))
    }

    /// Drives the surface. `client` is the heir's authenticated Connect client
    /// (`None` when signed out — every action degrades to a sign-in prompt).
    /// `session_generation` is the Connect account's current session counter
    /// (bumped on login / logout / reset); it is stamped into the spawned list
    /// fetch so a response that lands after the session changed is dropped
    /// instead of painting a prior account's vaults — the same guard the
    /// duress-contacts / recovery-alerts handlers use.
    pub fn update(
        &mut self,
        message: RecoverVaultMessage,
        client: Option<CoincubeClient>,
        session_generation: u64,
    ) -> Task<RecoverVaultMessage> {
        match message {
            RecoverVaultMessage::Load => {
                let Some(client) = client else {
                    self.list = ListState::Error(SIGNED_OUT_PROMPT.to_string());
                    return Task::none();
                };
                self.list = ListState::Loading(session_generation);
                Task::perform(
                    async move {
                        client
                            .list_recoverable_vaults()
                            .await
                            .map_err(|e| e.to_string())
                    },
                    move |res| RecoverVaultMessage::Loaded(res, session_generation),
                )
            }
            RecoverVaultMessage::Loaded(res, gen) => {
                // Drop a list that resolved after the session changed (logout /
                // account switch) so we never paint the prior account's vaults.
                if gen != session_generation {
                    // If the pane is still showing the `Loading` for *this* dead
                    // fetch, fall back to `Idle` so `is_loaded()` is false and
                    // reopening refetches — otherwise it's stranded on "Loading…"
                    // with no request in flight. Match on the stored generation
                    // so a newer in-flight load (later session) keeps its
                    // `Loading`; and leave `Loaded`/`Error`/`Idle` untouched so a
                    // current result or a live retry isn't clobbered.
                    if matches!(self.list, ListState::Loading(g) if g == gen) {
                        self.list = ListState::Idle;
                    }
                    return Task::none();
                }
                self.list = match res {
                    Ok(rows) => ListState::Loaded(rows),
                    Err(e) => ListState::Error(e),
                };
                Task::none()
            }
            RecoverVaultMessage::Recover(cube_id) => {
                let Some(client) = client else {
                    self.active = Some((
                        cube_id,
                        RecoverStatus::Failed("Sign in to recover this vault.".to_string()),
                    ));
                    return Task::none();
                };
                self.active = Some((cube_id, RecoverStatus::Fetching));
                Task::perform(
                    async move {
                        // The keyholder release returns the PLAINTEXT descriptor;
                        // gate failures are already mapped to neutral, display-safe
                        // copy by `KeyholderRecoveryError`'s `Display`.
                        let res = fetch_recovery_descriptor(&client, cube_id)
                            .await
                            .map_err(|e| e.to_string());
                        (cube_id, res)
                    },
                    move |(cube_id, res)| {
                        RecoverVaultMessage::Fetched(cube_id, res, session_generation)
                    },
                )
            }
            RecoverVaultMessage::Fetched(cube_id, res, gen) => {
                // Drop a descriptor fetch the user (or session) has moved on
                // from: a stale generation means a prior session fired it (an
                // account switch — even the same cube id), and a non-matching
                // `active` id means recovery started on another row. Either way
                // its "Recovery ready"/error must not paint onto the live row.
                if gen != session_generation
                    || self.active.as_ref().map(|(id, _)| *id) != Some(cube_id)
                {
                    return Task::none();
                }
                let status = match res {
                    Ok(_descriptor) => {
                        // TODO(PR 2): hand the plaintext `_descriptor` to the
                        // installer's `install_local_wallet()` path (a new
                        // `UserFlow::RecoverKeyholderVault { cube_id }`) to create
                        // the watch-only "Recovered Vault — [label]", then bridge
                        // into `vault/recovery.rs` for the sweep (PR 3). For PR 1
                        // we confirm the gated fetch succeeded.
                        RecoverStatus::Ready
                    }
                    Err(copy) => RecoverStatus::Failed(copy),
                };
                self.active = Some((cube_id, status));
                Task::none()
            }
        }
    }
}

/// Renders the discovery surface for the launcher main-content area.
pub fn view(panel: &RecoverVaultPanel) -> Element<'_, RecoverVaultMessage> {
    let header = Column::new()
        .spacing(6)
        .push(h4_bold("Recover a Vault"))
        .push(
            p2_regular(
                "Vaults you're a keyholder for appear here. When a vault's \
                 recovery window opens, you can recover it — no password needed.",
            )
            .style(theme::text::secondary),
        );

    let body: Element<RecoverVaultMessage> = match &panel.list {
        // Reset / never-requested. Opening the section while signed in fires
        // `Load` immediately (→ `Loading`), so the pane only lands on `Idle`
        // when Home is still parked here after a logout reset — show the
        // sign-in prompt, not a "Loading…" that will never resolve.
        ListState::Idle => {
            Container::new(p1_regular(SIGNED_OUT_PROMPT).style(theme::text::secondary))
                .padding(20)
                .center_x(Length::Fill)
                .into()
        }
        ListState::Loading(_) => {
            Container::new(p1_regular("Loading recoverable vaults…").style(theme::text::secondary))
                .padding(20)
                .center_x(Length::Fill)
                .into()
        }
        ListState::Error(msg) => {
            Container::new(p1_regular(msg.clone()).style(theme::text::secondary))
                .padding(20)
                .center_x(Length::Fill)
                .into()
        }
        ListState::Loaded(rows) if rows.is_empty() => Container::new(
            p1_regular("No vaults are available for you to recover right now.")
                .style(theme::text::secondary),
        )
        .padding(20)
        .center_x(Length::Fill)
        .into(),
        ListState::Loaded(rows) => {
            let mut col = Column::new().spacing(12);
            for row in rows {
                col = col.push(vault_row(row, panel.active.as_ref()));
            }
            scrollable(col).into()
        }
    };

    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(header)
        .push(body)
        .into()
}

/// One discovery row: title + tier/state meta + a CTA appropriate to its kind.
fn vault_row<'a>(
    v: &'a RecoverableVault,
    active: Option<&'a (u64, RecoverStatus)>,
) -> Element<'a, RecoverVaultMessage> {
    let title = v
        .owner_label
        .clone()
        .unwrap_or_else(|| format!("Vault #{}", v.cube_id));

    let meta = format!(
        "{} • {}",
        tier_label(v.monitoring_level),
        state_label(v.recovery_state())
    );

    let mut left = Column::new()
        .spacing(4)
        .push(p1_bold(title))
        .push(caption(meta).style(theme::text::secondary));

    // Non-actionable rows carry their status copy under the meta line.
    if let Some(copy) = status_copy(v) {
        left = left.push(p2_regular(copy).style(theme::text::secondary));
    }

    // The actionable CTA: either the live "Recover" button or, if this row is
    // the one in flight, its inline status.
    let cta: Element<RecoverVaultMessage> = if classify(v) == RowKind::Actionable {
        match active {
            Some((id, status)) if *id == v.cube_id => recover_status_view(status),
            _ => btn::primary(None, "Recover")
                .on_press(RecoverVaultMessage::Recover(v.cube_id))
                .into(),
        }
    } else {
        Space::new().width(Length::Fixed(0.0)).into()
    };

    card::simple(
        Row::new()
            .align_y(Alignment::Center)
            .push(Container::new(left).width(Length::Fill))
            .push(cta),
    )
    .padding(16)
    .width(Length::Fill)
    .into()
}

/// Inline status shown in place of the "Recover" button while a fetch is in
/// flight or after it resolves.
fn recover_status_view(status: &RecoverStatus) -> Element<'_, RecoverVaultMessage> {
    match status {
        RecoverStatus::Fetching => p2_regular("Preparing recovery…")
            .style(theme::text::secondary)
            .into(),
        RecoverStatus::Ready => p2_bold("Recovery ready").into(),
        RecoverStatus::Failed(copy) => p2_regular(copy.clone())
            .style(theme::text::secondary)
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault_role(
        level: VaultMonitoringLevel,
        state: &str,
        pw: bool,
        role: &str,
    ) -> RecoverableVault {
        RecoverableVault {
            cube_id: 1,
            owner_label: None,
            monitoring_level: level,
            state: state.to_string(),
            role: role.to_string(),
            requires_recovery_password: pw,
            owner_last_active: None,
            expected_open_at: None,
            gap_limit: None,
        }
    }

    /// The common case: the caller is a keyholder (the only role that can pull
    /// the descriptor down). Beneficiary cases construct via `vault_role`.
    fn vault(level: VaultMonitoringLevel, state: &str, pw: bool) -> RecoverableVault {
        vault_role(level, state, pw, "keyholder")
    }

    #[test]
    fn full_open_is_actionable() {
        let v = vault(VaultMonitoringLevel::Full, "available", false);
        assert_eq!(classify(&v), RowKind::Actionable);
        assert!(status_copy(&v).is_none());
    }

    #[test]
    fn beneficiary_open_full_is_keyholder_only_not_actionable() {
        // Same open + Full-tier state as `full_open_is_actionable`, but the
        // caller is a beneficiary: the descriptor pull-down is keyholder-only
        // (the release endpoint 403s non-keyholders), so this must NOT be a live
        // button — it shows the keyholder-only copy instead.
        let v = vault_role(
            VaultMonitoringLevel::Full,
            "available",
            false,
            "beneficiary",
        );
        assert_eq!(classify(&v), RowKind::KeyholderOnly);
        assert!(!v.is_recoverable_now());
        assert!(status_copy(&v).unwrap().contains("keyholder"));
    }

    #[test]
    fn full_reminding_is_actionable() {
        // `reminding` is a later "still open" state — also actionable.
        let v = vault(VaultMonitoringLevel::Full, "reminding", false);
        assert_eq!(classify(&v), RowKind::Actionable);
    }

    #[test]
    fn full_approaching_is_not_yet_open() {
        let v = vault(VaultMonitoringLevel::Full, "approaching", false);
        assert_eq!(classify(&v), RowKind::NotYetOpen);
        assert!(status_copy(&v).unwrap().contains("isn't open yet"));
    }

    #[test]
    fn full_none_is_not_yet_open() {
        let v = vault(VaultMonitoringLevel::Full, "none", false);
        assert_eq!(classify(&v), RowKind::NotYetOpen);
    }

    #[test]
    fn heartbeat_requires_password_is_deferred_even_when_open() {
        // Tier honesty (I7): an open Heartbeat vault is NOT a live button —
        // it must show the deferred-path copy.
        let v = vault(VaultMonitoringLevel::Heartbeat, "available", true);
        assert_eq!(classify(&v), RowKind::PasswordDeferred);
        assert_eq!(
            status_copy(&v).unwrap(),
            "Recovery password required — coming in a later update."
        );
    }

    #[test]
    fn password_required_takes_precedence_over_not_open() {
        let v = vault(VaultMonitoringLevel::Heartbeat, "approaching", true);
        assert_eq!(classify(&v), RowKind::PasswordDeferred);
    }

    #[test]
    fn unknown_state_fails_closed_to_not_open() {
        // An unrecognised wire state must never become a live Recover button.
        let v = vault(VaultMonitoringLevel::Full, "weird-future-state", false);
        assert_eq!(classify(&v), RowKind::NotYetOpen);
    }

    #[test]
    fn load_without_client_surfaces_signin_prompt() {
        let mut panel = RecoverVaultPanel::new();
        let _ = panel.update(RecoverVaultMessage::Load, None, 0);
        match &panel.list {
            ListState::Error(msg) => assert!(msg.contains("Sign in")),
            other => panic!("expected sign-in error, got {:?}", other),
        }
        // An error is retryable: `is_loaded()` stays false so reopening the
        // section (e.g. after the heir signs in) re-fires `Load`.
        assert!(!panel.is_loaded());
    }

    #[test]
    fn is_loaded_suppresses_refetch_only_for_loading_and_loaded() {
        let mut panel = RecoverVaultPanel::new();
        // Idle: never requested → fetch.
        assert!(!panel.is_loaded());
        // Error: transient failure → retry on reopen.
        panel.list = ListState::Error("boom".to_string());
        assert!(!panel.is_loaded());
        // Loading: in flight → don't duplicate the request.
        panel.list = ListState::Loading(0);
        assert!(panel.is_loaded());
        // Loaded: already have the list → don't refetch on reopen.
        panel.list = ListState::Loaded(vec![]);
        assert!(panel.is_loaded());
    }

    #[test]
    fn loaded_stores_rows() {
        let mut panel = RecoverVaultPanel::new();
        let rows = vec![vault(VaultMonitoringLevel::Full, "available", false)];
        let _ = panel.update(RecoverVaultMessage::Loaded(Ok(rows), 0), None, 0);
        assert!(matches!(panel.list, ListState::Loaded(ref r) if r.len() == 1));
    }

    #[test]
    fn loaded_from_a_stale_session_is_dropped() {
        // A list fetch fired in generation 1 that lands after the session
        // advanced (logout / account switch → generation 2) must not paint the
        // prior account's vaults: the panel's `list` is left untouched.
        let mut panel = RecoverVaultPanel::new();
        let rows = vec![vault(VaultMonitoringLevel::Full, "available", false)];
        let _ = panel.update(RecoverVaultMessage::Loaded(Ok(rows), 1), None, 2);
        assert!(
            matches!(panel.list, ListState::Idle),
            "stale-session list must not be stored, got {:?}",
            panel.list
        );
    }

    #[test]
    fn stale_loaded_while_loading_falls_back_to_idle() {
        // A stale-session list (gen 1) resolving while the pane still shows the
        // `Loading` for *that same* dead fetch must drop back to `Idle` — not
        // strand it on "Loading…" forever (which `is_loaded()` would treat as
        // loaded, blocking refetch on reopen).
        let mut panel = RecoverVaultPanel::new();
        panel.list = ListState::Loading(1);
        let rows = vec![vault(VaultMonitoringLevel::Full, "available", false)];
        let _ = panel.update(RecoverVaultMessage::Loaded(Ok(rows), 1), None, 2);
        assert!(
            matches!(panel.list, ListState::Idle),
            "stranded Loading must reset to Idle, got {:?}",
            panel.list
        );
        assert!(!panel.is_loaded());
    }

    #[test]
    fn stale_loaded_does_not_clobber_a_newer_loading() {
        // A newer in-flight load (gen 2 `Loading`) must survive a stale (gen 1)
        // result resolving — the reset only clears the dead generation's own
        // `Loading`, never a later session's request.
        let mut panel = RecoverVaultPanel::new();
        panel.list = ListState::Loading(2);
        let rows = vec![vault(VaultMonitoringLevel::Full, "available", false)];
        let _ = panel.update(RecoverVaultMessage::Loaded(Ok(rows), 1), None, 2);
        assert!(
            matches!(panel.list, ListState::Loading(2)),
            "newer in-flight Loading must be preserved, got {:?}",
            panel.list
        );
    }

    #[test]
    fn stale_loaded_does_not_clobber_current_loaded_list() {
        // A stale drop must leave an already-populated current-session list
        // intact (don't reset a good `Loaded` back to `Idle`).
        let mut panel = RecoverVaultPanel::new();
        panel.list = ListState::Loaded(vec![vault(VaultMonitoringLevel::Full, "available", false)]);
        let rows = vec![vault(VaultMonitoringLevel::Heartbeat, "approaching", true)];
        let _ = panel.update(RecoverVaultMessage::Loaded(Ok(rows), 1), None, 2);
        assert!(matches!(panel.list, ListState::Loaded(ref r) if r.len() == 1));
    }

    #[test]
    fn loaded_for_the_current_session_is_stored() {
        // The matching-generation path still stores rows (regression guard for
        // the stale-session check above).
        let mut panel = RecoverVaultPanel::new();
        let rows = vec![vault(VaultMonitoringLevel::Full, "available", false)];
        let _ = panel.update(RecoverVaultMessage::Loaded(Ok(rows), 5), None, 5);
        assert!(matches!(panel.list, ListState::Loaded(ref r) if r.len() == 1));
    }

    #[test]
    fn fetched_error_copy_is_shown_verbatim() {
        let mut panel = RecoverVaultPanel::new();
        // Simulate the in-flight recover `Recover` sets before the async fetch
        // resolves, so the matching `Fetched` is applied to the active row.
        panel.active = Some((7, RecoverStatus::Fetching));
        let _ = panel.update(
            RecoverVaultMessage::Fetched(
                7,
                Err("Recovery is unavailable right now.".to_string()),
                0,
            ),
            None,
            0,
        );
        match panel.active {
            Some((7, RecoverStatus::Failed(copy))) => {
                assert_eq!(copy, "Recovery is unavailable right now.")
            }
            other => panic!("expected failed status, got {:?}", other),
        }
    }

    #[test]
    fn stale_fetch_does_not_overwrite_a_newer_active_row() {
        // The heir starts recovery on cube 7, then on cube 8 (which replaces
        // `active`). Cube 7's older descriptor fetch resolving afterwards must
        // not paint its result onto cube 8's in-flight row.
        let mut panel = RecoverVaultPanel::new();
        panel.active = Some((8, RecoverStatus::Fetching));
        let _ = panel.update(
            RecoverVaultMessage::Fetched(7, Ok("wsh(...)".to_string()), 0),
            None,
            0,
        );
        assert!(
            matches!(panel.active, Some((8, RecoverStatus::Fetching))),
            "stale cube-7 fetch must leave cube-8's row untouched, got {:?}",
            panel.active
        );
    }

    #[test]
    fn stale_session_fetch_for_same_cube_is_dropped() {
        // Account A's descriptor fetch for cube 7 (gen 1) resolving in account
        // B's session (gen 2) — same cube id, since both are keyholders — must
        // be dropped, not painted onto B's in-flight row. The cube-id-only guard
        // would miss this; the generation check catches it.
        let mut panel = RecoverVaultPanel::new();
        panel.active = Some((7, RecoverStatus::Fetching));
        let _ = panel.update(
            RecoverVaultMessage::Fetched(7, Ok("wsh(...)".to_string()), 1),
            None,
            2,
        );
        assert!(
            matches!(panel.active, Some((7, RecoverStatus::Fetching))),
            "stale-session fetch must not overwrite the new session's row, got {:?}",
            panel.active
        );
    }
}
