pub mod conflict;
pub mod warning;

use crate::state::State;
use liana_ui::widget::{modal::Modal, Element};

use crate::state::Msg;

pub fn render_modals(state: &State) -> Option<Element<'_, Msg>> {
    // First, get the underlying modal (key, path, or xpub modal)
    let underlying_modal = crate::views::keys::modal::render_modal(state)
        .or_else(|| crate::views::paths::modal::render_modal(state))
        .or_else(|| crate::views::xpub::render_modal(state));

    // Priority order: Warning modal > Conflict modal > Underlying modal

    // If there's a warning modal, it should be on top
    if let Some(warning_modal_state) = &state.views.modals.warning {
        let warning_modal = warning::render_warning_modal(warning_modal_state);

        // Stack on top of any underlying modals
        if let Some(conflict_modal_state) = &state.views.modals.conflict {
            // Warning on top of conflict on top of underlying
            let conflict_modal = conflict::render_conflict_modal(conflict_modal_state);
            if let Some(underlying) = underlying_modal {
                let stacked: Element<'_, Msg> = Modal::new(underlying, conflict_modal)
                    .on_blur(Some(Msg::ConflictDismiss))
                    .into();
                return Some(
                    Modal::new(stacked, warning_modal)
                        .on_blur(Some(Msg::WarningCloseModal))
                        .into(),
                );
            }
            return Some(
                Modal::new(conflict_modal, warning_modal)
                    .on_blur(Some(Msg::WarningCloseModal))
                    .into(),
            );
        }

        // Warning on top of underlying (no conflict)
        if let Some(underlying) = underlying_modal {
            return Some(
                Modal::new(underlying, warning_modal)
                    .on_blur(Some(Msg::WarningCloseModal))
                    .into(),
            );
        }
        // Just the warning modal
        return Some(warning_modal);
    }

    // If there's a conflict modal, it should be stacked on top of underlying
    if let Some(conflict_modal_state) = &state.views.modals.conflict {
        let conflict_modal = conflict::render_conflict_modal(conflict_modal_state);

        if let Some(underlying) = underlying_modal {
            return Some(
                Modal::new(underlying, conflict_modal)
                    .on_blur(Some(Msg::ConflictDismiss))
                    .into(),
            );
        }
        // Just the conflict modal
        return Some(conflict_modal);
    }

    // No warning or conflict modal, just return the underlying modal if it exists
    underlying_modal
}
