//! Owner "Recover a Cube I own" discovery surface
//! (PLAN-owner-keychain-recovery PR 3).
//!
//! A global, pre-cube launcher surface — the owner-side sibling of the heir
//! [`crate::recover_vault`] panel. On a wiped install the owner signs in and
//! sees the Cubes they own that are set up for **phone recovery** (an
//! `owner-self` recovery recipient is registered + an envelope set uploaded).
//! Clicking "Recover" launches the installer's owner-keychain restore flow
//! ([`UserFlow::RecoverOwnCubeWithPhone`](crate::installer::UserFlow)), which
//! pulls the owner's own envelope set and relay-decrypts it via the owner's
//! Keychain — no recovery password.
//!
//! Gated behind [`crate::feature_flags`]'s `OWNER_KEYCHAIN_RECOVERY_ENABLED`.
//! Discovery uses the tested Connect-client foundation: `list_cubes` for the
//! owner's cubes, then `list_recovery_kit_recipients` per cube to find the
//! `owner-self` recipient and read its tier (Full-Cube vs Vault-only). Rows are
//! marked **Beta**.

use iced::widget::scrollable;
use iced::{Alignment, Length, Task};

use coincube_ui::{
    component::{badge, button as btn, card, text::*},
    theme,
    widget::{Column, Container, Element, Row},
};

use crate::services::coincube::{CoincubeClient, CoincubeError};

const SIGNED_OUT_PROMPT: &str = "Sign in to your account to see Cubes you can recover with your phone.";

/// One row: a Cube the signed-in owner can recover with their phone.
#[derive(Debug, Clone)]
pub struct RecoverableOwnCube {
    pub cube_id: u64,
    pub name: String,
    pub network: String,
    /// Full-Cube (seed + descriptor escrowed) vs Vault-only (descriptor only).
    /// Drives the restore scope + the button label.
    pub full_cube: bool,
}

/// Loading state for the discovery list. Mirrors
/// [`crate::recover_vault::ListState`] including the stale-session guard.
#[derive(Debug, Clone, Default)]
pub enum ListState {
    #[default]
    Idle,
    Loading(u64),
    Loaded(Vec<RecoverableOwnCube>),
    Error(String),
}

/// In-flight status for the row the owner clicked "Recover" on.
#[derive(Debug, Clone)]
pub enum RecoverStatus {
    /// Launching the installer restore flow.
    Launching,
}

#[derive(Debug, Clone, Default)]
pub struct RecoverOwnCubePanel {
    list: ListState,
    active: Option<(u64, RecoverStatus)>,
}

/// Messages for the owner discovery surface. Home forwards these via
/// `ViewMessage::RecoverOwnCube(..)`.
#[derive(Debug, Clone)]
pub enum RecoverOwnCubeMessage {
    /// (Re)fetch the recoverable-own-cube list. `network` filters to the
    /// install's network (matching the Connect `CubeResponse.network` shape).
    Load(String),
    /// List fetch resolved; `u64` is the Connect session generation it fired in
    /// (a stale value is dropped rather than painting a prior account's cubes).
    Loaded(Result<Vec<RecoverableOwnCube>, String>, u64),
    /// Owner clicked "Recover" — launch the installer's owner-keychain flow.
    /// **Home intercepts this** and emits
    /// `home::Message::Install(UserFlow::RecoverOwnCubeWithPhone { .. })`; it is a
    /// no-op inside this panel.
    Launch { cube_id: u64, full_cube: bool },
}

impl RecoverOwnCubePanel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether Home should skip re-firing `Load` when the section is reopened.
    /// True only while a fetch is in flight or has succeeded.
    pub fn is_loaded(&self) -> bool {
        matches!(self.list, ListState::Loading(_) | ListState::Loaded(_))
    }

    pub fn update(
        &mut self,
        message: RecoverOwnCubeMessage,
        client: Option<CoincubeClient>,
        session_generation: u64,
    ) -> Task<RecoverOwnCubeMessage> {
        match message {
            RecoverOwnCubeMessage::Load(network) => {
                let Some(client) = client else {
                    self.list = ListState::Error(SIGNED_OUT_PROMPT.to_string());
                    return Task::none();
                };
                self.list = ListState::Loading(session_generation);
                Task::perform(
                    async move { fetch_recoverable_own_cubes(client, network).await },
                    move |res| RecoverOwnCubeMessage::Loaded(res, session_generation),
                )
            }
            RecoverOwnCubeMessage::Loaded(res, gen) => {
                // Drop a list that resolved after the session changed.
                if gen != session_generation {
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
            // Home intercepts `Launch` before delegating here.
            RecoverOwnCubeMessage::Launch { cube_id, full_cube } => {
                let _ = full_cube;
                self.active = Some((cube_id, RecoverStatus::Launching));
                Task::none()
            }
        }
    }
}

/// Owner discovery fetch: list the owner's cubes (filtered to `network`), then
/// for each probe its recovery recipients to find an `owner-self` row. A cube
/// with one is phone-recoverable; the recipient's tier picks Full-Cube vs
/// Vault-only. `404` on the recipients probe means "no recipient → not
/// phone-recoverable" (skip); other errors fail the whole list.
async fn fetch_recoverable_own_cubes(
    client: CoincubeClient,
    network: String,
) -> Result<Vec<RecoverableOwnCube>, String> {
    let cubes = client.list_cubes().await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for cube in cubes.into_iter().filter(|c| c.network == network) {
        match client.list_recovery_kit_recipients(cube.id).await {
            Ok(rows) => {
                if let Some(r) = rows.iter().find(|r| r.is_owner_self()) {
                    let full_cube = r.tier.map(|t| t.includes_seed()).unwrap_or(false);
                    out.push(RecoverableOwnCube {
                        cube_id: cube.id,
                        name: cube.name,
                        network: cube.network,
                        full_cube,
                    });
                }
            }
            // No recipients registered for this cube → not phone-recoverable.
            Err(CoincubeError::NotFound) => {}
            Err(e) => {
                return Err(format!(
                    "Couldn't check phone recovery for \"{}\": {}",
                    cube.name, e
                ));
            }
        }
    }
    Ok(out)
}

fn recover_button_label(full_cube: bool) -> &'static str {
    if full_cube {
        "Recover full Cube"
    } else {
        "Recover Vault"
    }
}

fn tier_label(full_cube: bool) -> &'static str {
    if full_cube {
        "Full Cube"
    } else {
        "Vault only"
    }
}

/// Renders the discovery surface for the launcher main-content area.
pub fn view(panel: &RecoverOwnCubePanel) -> Element<'_, RecoverOwnCubeMessage> {
    let header = Column::new()
        .spacing(6)
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(h4_bold("Recover a Cube I own"))
                .push(badge::beta()),
        )
        .push(
            p2_regular(
                "Cubes you've protected with your phone appear here. Recover one with your \
                 Keychain — approve the decryption on your phone, no password needed.",
            )
            .style(theme::text::secondary),
        );

    let body: Element<RecoverOwnCubeMessage> = match &panel.list {
        ListState::Idle => {
            Container::new(p1_regular(SIGNED_OUT_PROMPT).style(theme::text::secondary))
                .padding(20)
                .center_x(Length::Fill)
                .into()
        }
        ListState::Loading(_) => {
            Container::new(p1_regular("Loading your Cubes…").style(theme::text::secondary))
                .padding(20)
                .center_x(Length::Fill)
                .into()
        }
        ListState::Error(msg) => Container::new(p1_regular(msg.clone()).style(theme::text::secondary))
            .padding(20)
            .center_x(Length::Fill)
            .into(),
        ListState::Loaded(rows) if rows.is_empty() => Container::new(
            p1_regular(
                "No Cubes are set up for phone recovery on this account. Set one up from \
                 Settings → Cube Recovery Kit → “Use my phone”.",
            )
            .style(theme::text::secondary),
        )
        .padding(20)
        .center_x(Length::Fill)
        .into(),
        ListState::Loaded(rows) => {
            let mut col = Column::new().spacing(12);
            for row in rows {
                col = col.push(cube_row(row, panel.active.as_ref()));
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

fn cube_row<'a>(
    c: &'a RecoverableOwnCube,
    active: Option<&'a (u64, RecoverStatus)>,
) -> Element<'a, RecoverOwnCubeMessage> {
    let meta = format!("{} • Phone recovery", tier_label(c.full_cube));
    let left = Column::new()
        .spacing(4)
        .push(p1_bold(c.name.clone()))
        .push(caption(meta).style(theme::text::secondary));

    let cta: Element<RecoverOwnCubeMessage> = match active {
        Some((id, RecoverStatus::Launching)) if *id == c.cube_id => {
            p2_regular("Preparing recovery…")
                .style(theme::text::secondary)
                .into()
        }
        _ => btn::primary(None, recover_button_label(c.full_cube))
            .on_press(RecoverOwnCubeMessage::Launch {
                cube_id: c.cube_id,
                full_cube: c.full_cube,
            })
            .into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(name: &str, full_cube: bool) -> RecoverableOwnCube {
        RecoverableOwnCube {
            cube_id: 1,
            name: name.to_string(),
            network: "mainnet".to_string(),
            full_cube,
        }
    }

    #[test]
    fn button_and_tier_labels_track_scope() {
        assert_eq!(recover_button_label(true), "Recover full Cube");
        assert_eq!(recover_button_label(false), "Recover Vault");
        assert_eq!(tier_label(true), "Full Cube");
        assert_eq!(tier_label(false), "Vault only");
    }

    #[test]
    fn load_without_client_surfaces_signin_prompt() {
        let mut panel = RecoverOwnCubePanel::new();
        let _ = panel.update(RecoverOwnCubeMessage::Load("mainnet".to_string()), None, 0);
        match &panel.list {
            ListState::Error(msg) => assert!(msg.contains("Sign in")),
            other => panic!("expected sign-in error, got {:?}", other),
        }
        assert!(!panel.is_loaded());
    }

    #[test]
    fn is_loaded_only_for_loading_and_loaded() {
        let mut panel = RecoverOwnCubePanel::new();
        assert!(!panel.is_loaded());
        panel.list = ListState::Error("boom".to_string());
        assert!(!panel.is_loaded());
        panel.list = ListState::Loading(0);
        assert!(panel.is_loaded());
        panel.list = ListState::Loaded(vec![cube("c", true)]);
        assert!(panel.is_loaded());
    }

    #[test]
    fn stale_session_load_is_dropped() {
        let mut panel = RecoverOwnCubePanel::new();
        let _ = panel.update(
            RecoverOwnCubeMessage::Loaded(Ok(vec![cube("c", true)]), 1),
            None,
            2,
        );
        assert!(
            matches!(panel.list, ListState::Idle),
            "stale-session list must not be stored, got {:?}",
            panel.list
        );
    }

    #[test]
    fn current_session_load_is_stored() {
        let mut panel = RecoverOwnCubePanel::new();
        let _ = panel.update(
            RecoverOwnCubeMessage::Loaded(Ok(vec![cube("c", false)]), 5),
            None,
            5,
        );
        assert!(matches!(panel.list, ListState::Loaded(ref r) if r.len() == 1));
    }
}
