use crate::{
    backend::Backend,
    state::{Msg, State},
};
use iced::{widget::Space, Alignment};
use liana_ui::{component::text, widget::*};

use super::layout;

pub fn xpub_view(state: &State) -> Element<'_, Msg> {
    // Get wallet name if available
    let wallet_name = state
        .app
        .selected_wallet
        .and_then(|id| state.backend.get_wallet(id))
        .map(|w| w.alias.clone())
        .unwrap_or_else(|| "Wallet".to_string());

    let content = Column::new()
        .spacing(30)
        .align_x(Alignment::Center)
        .push(text::h2(format!("{} - Add Key Information", wallet_name)))
        .push(Space::with_height(20))
        .push(text::p1_regular(
            "This is where participants will enter their extended public keys (xpub).",
        ))
        .push(Space::with_height(20))
        .push(
            Container::new(text::p1_regular("[ Placeholder for xpub entry form ]"))
                .padding(40)
                .style(liana_ui::theme::card::simple),
        )
        .push(Space::with_height(50));

    layout(
        (0, 0), // No progress indicator for this view
        Some(&state.views.login.email.form.value),
        None, // No role badge needed here
        "Key Information",
        content,
        true,
        Some(Msg::NavigateBack),
    )
}

