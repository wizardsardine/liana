pub mod conflict;
pub mod template_help;
pub mod warning;

use crate::state::{Msg, State};
use liana_ui::widget::{modal::Modal, Element};
use liana_ui::{component::text, theme, widget::Text};

pub fn installer_modal<'a>(message: &'a str) -> Text<'a> {
    text::new::caption(message).style(theme::text::secondary)
}

pub fn modals_view(state: &State) -> Option<Element<'_, Msg>> {
    // First, get the underlying modal (key, path, xpub, or registration modal)
    let underlying_modal = crate::views::keys::modal::key_modal_view(state)
        .or_else(|| crate::views::paths::modal::path_modal_view(state))
        .or_else(|| crate::views::xpub::xpub_modal_view(state))
        .or_else(|| crate::views::registration::modal::registration_modal_view(state));

    // Priority order: Warning modal > Template-help modal > Conflict modal > Underlying modal

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

    if let Some(template_help_modal_state) = &state.views.modals.template_help {
        let template_help_modal =
            template_help::template_help_modal_view(template_help_modal_state);

        if let Some(conflict_modal_state) = &state.views.modals.conflict {
            let conflict_modal = conflict::conflict_modal_view(conflict_modal_state);
            if let Some(underlying) = underlying_modal {
                let stacked: Element<'_, Msg> = Modal::new(underlying, conflict_modal)
                    .on_blur(Some(Msg::ConflictDismiss))
                    .into();
                return Some(
                    Modal::new(stacked, template_help_modal)
                        .on_blur(Some(Msg::TemplateHelpCloseModal))
                        .into(),
                );
            }
            return Some(
                Modal::new(conflict_modal, template_help_modal)
                    .on_blur(Some(Msg::TemplateHelpCloseModal))
                    .into(),
            );
        }

        if let Some(underlying) = underlying_modal {
            return Some(
                Modal::new(underlying, template_help_modal)
                    .on_blur(Some(Msg::TemplateHelpCloseModal))
                    .into(),
            );
        }

        return Some(template_help_modal);
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

    // No warning, template-help, or conflict modal, just return the underlying modal if it exists
    underlying_modal
}
