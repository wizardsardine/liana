pub mod warning;

use crate::state::State;
use liana_ui::widget::modal::Modal;
use liana_ui::widget::Element;

use crate::state::Msg;

pub fn render_modals(state: &State) -> Option<Element<'_, Msg>> {
    // First, get the underlying modal (key or path modal)
    let underlying_modal = crate::views::keys::modal::render_modal(state)
        .or_else(|| crate::views::paths::modal::render_modal(state));

    // If there's a warning modal, it should be stacked on top
    if let Some(warning_modal_state) = &state.views.modals.warning {
        let warning_modal = warning::render_warning_modal(warning_modal_state);

        // If there's an underlying modal, we need to return a structure that
        // represents "warning modal overlaying underlying modal"
        // Since we'll wrap this in Modal::new(content, modal) in view(),
        // we create a modal where the underlying modal is the content and warning is the overlay
        if let Some(underlying) = underlying_modal {
            // Create a modal where underlying is the content and warning is the overlay
            // This will be wrapped again in view() with the base content
            return Some(
                Modal::new(underlying, warning_modal)
                    .on_blur(Some(Msg::WarningCloseModal))
                    .into(),
            );
        }
        // Otherwise, just return the warning modal
        return Some(warning_modal);
    }

    // No warning modal, just return the underlying modal if it exists
    underlying_modal
}
