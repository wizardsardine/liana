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

/// How a discovery row should present, encoding invariants I1 (state gating)
/// and I7 (tier honesty). Password-required (Heartbeat) rows take precedence:
/// they are never actionable in v1 regardless of window state, and must show
/// the deferred-path copy rather than a broken "Recover" button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// Window open **and** Full-tier (password-free) → the "Recover" button is
    /// live.
    Actionable,
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
/// (`available`/`reminding`) Heartbeat vault still routes to the deferred copy.
pub fn classify(v: &RecoverableVault) -> RowKind {
    if v.requires_recovery_password {
        RowKind::PasswordDeferred
    } else if v.recovery_state().is_open() {
        RowKind::Actionable
    } else {
        RowKind::NotYetOpen
    }
}

/// The user-facing status line for a non-actionable row (invariant I7 copy).
/// Returns `None` for an actionable row (it gets a button, not a status line).
pub fn status_copy(v: &RecoverableVault) -> Option<String> {
    match classify(v) {
        RowKind::Actionable => None,
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
    /// Fetch in flight.
    Loading,
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
    /// List fetch resolved.
    Loaded(Result<Vec<RecoverableVault>, String>),
    /// Heir clicked "Recover" on the given cube — pull its descriptor.
    Recover(u64),
    /// Descriptor fetch resolved (cube id, plaintext descriptor | display copy).
    Fetched(u64, Result<String, String>),
}

impl RecoverVaultPanel {
    pub fn new() -> Self {
        Self::default()
    }

    /// True once the list has been requested at least once — lets Home avoid
    /// re-firing `Load` every time the section is reopened.
    pub fn is_loaded(&self) -> bool {
        !matches!(self.list, ListState::Idle)
    }

    /// Drives the surface. `client` is the heir's authenticated Connect client
    /// (`None` when signed out — every action degrades to a sign-in prompt).
    pub fn update(
        &mut self,
        message: RecoverVaultMessage,
        client: Option<CoincubeClient>,
    ) -> Task<RecoverVaultMessage> {
        match message {
            RecoverVaultMessage::Load => {
                let Some(client) = client else {
                    self.list = ListState::Error(
                        "Sign in to your account to see vaults you can recover.".to_string(),
                    );
                    return Task::none();
                };
                self.list = ListState::Loading;
                Task::perform(
                    async move {
                        client
                            .list_recoverable_vaults()
                            .await
                            .map_err(|e| e.to_string())
                    },
                    RecoverVaultMessage::Loaded,
                )
            }
            RecoverVaultMessage::Loaded(res) => {
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
                    |(cube_id, res)| RecoverVaultMessage::Fetched(cube_id, res),
                )
            }
            RecoverVaultMessage::Fetched(cube_id, res) => {
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
        ListState::Idle | ListState::Loading => {
            Container::new(p1_regular("Loading recoverable vaults…").style(theme::text::secondary))
                .padding(20)
                .center_x(Length::Fill)
                .into()
        }
        ListState::Error(msg) => Container::new(
            p1_regular(msg.clone()).style(theme::text::secondary),
        )
        .padding(20)
        .center_x(Length::Fill)
        .into(),
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

    fn vault(level: VaultMonitoringLevel, state: &str, pw: bool) -> RecoverableVault {
        RecoverableVault {
            cube_id: 1,
            owner_label: None,
            monitoring_level: level,
            state: state.to_string(),
            requires_recovery_password: pw,
            owner_last_active: None,
            expected_open_at: None,
            gap_limit: None,
        }
    }

    #[test]
    fn full_open_is_actionable() {
        let v = vault(VaultMonitoringLevel::Full, "available", false);
        assert_eq!(classify(&v), RowKind::Actionable);
        assert!(status_copy(&v).is_none());
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
        let _ = panel.update(RecoverVaultMessage::Load, None);
        match &panel.list {
            ListState::Error(msg) => assert!(msg.contains("Sign in")),
            other => panic!("expected sign-in error, got {:?}", other),
        }
        assert!(panel.is_loaded());
    }

    #[test]
    fn loaded_stores_rows() {
        let mut panel = RecoverVaultPanel::new();
        let rows = vec![vault(VaultMonitoringLevel::Full, "available", false)];
        let _ = panel.update(RecoverVaultMessage::Loaded(Ok(rows)), None);
        assert!(matches!(panel.list, ListState::Loaded(ref r) if r.len() == 1));
    }

    #[test]
    fn fetched_error_copy_is_shown_verbatim() {
        let mut panel = RecoverVaultPanel::new();
        let _ = panel.update(
            RecoverVaultMessage::Fetched(7, Err("Recovery is unavailable right now.".to_string())),
            None,
        );
        match panel.active {
            Some((7, RecoverStatus::Failed(copy))) => {
                assert_eq!(copy, "Recovery is unavailable right now.")
            }
            other => panic!("expected failed status, got {:?}", other),
        }
    }
}
