pub mod conflict;
pub mod warning;

use crate::state::{Msg, State};
use liana_ui::widget::{modal::Modal, Element};

pub fn modals_view(state: &State) -> Option<Element<'_, Msg>> {
    // First, get the underlying modal (key, path, xpub, or registration modal)
    let underlying_modal = crate::views::keys::modal::key_modal_view(state)
        .or_else(|| crate::views::paths::modal::path_modal_view(state))
        .or_else(|| crate::views::xpub::xpub_modal_view(state))
        .or_else(|| crate::views::registration::modal::registration_modal_view(state));

    // Priority order: Warning modal > Conflict modal > Underlying modal

    // If there's a warning modal, it should be on top
    if let Some(warning_modal_state) = &state.views.modals.warning {
        let warning_modal = warning::warning_modal_view(warning_modal_state);

        // Stack on top of any underlying modals
        if let Some(conflict_modal_state) = &state.views.modals.conflict {
            // Warning on top of conflict on top of underlying
            let conflict_modal = conflict::conflict_modal_view(conflict_modal_state);
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
        let conflict_modal = conflict::conflict_modal_view(conflict_modal_state);

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
