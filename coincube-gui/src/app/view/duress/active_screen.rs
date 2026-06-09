//! The cryptic "Duress Mode Activated" screen (Phase 3 emits it; Phase 5 wires
//! the three exit paths and the gated Sign-in behaviour).
//!
//! This screen replaces every other view. It reveals nothing recoverable and
//! offers no recovery affordances on this device: no countdown, no remaining
//! window, no balances, no Cube list, no support link. Its **only** interactive
//! element is a single "Sign in to Connect" button which, in Phase 5, gates
//! entirely on server-side duress state — while duress is active it shows an
//! inline "Try again later" and never a credential prompt.

use coincube_ui::{
    color,
    component::{button, text},
    widget::{Column, Container, Element},
};
use iced::{Alignment, Length};

#[derive(Debug, Clone)]
pub enum Message {
    /// User tapped "Sign in to Connect". Phase 5 turns this into a
    /// `get_duress_state` check that either stays (active) or exits (cleared).
    SignInPressed,
}

/// Cryptic-screen state. Deliberately tiny — it must not cache anything
/// sensitive.
#[derive(Debug, Default)]
pub struct DuressActiveScreen {
    /// Inline error shown beneath the button (e.g. "Duress mode is active.
    /// Try again later.").
    pub error: Option<String>,
    /// True while a server-side state check is in flight.
    pub checking: bool,
    /// Whether the retry queue still has pending work — drives the subtle
    /// corner dot (Phase 4 Task 4.2). No text, no explanation.
    pub queue_pending: bool,
}

impl DuressActiveScreen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn view(&self) -> Element<Message> {
        let mut col = Column::new()
            .width(Length::Fill)
            .spacing(16)
            .align_x(Alignment::Center)
            .push(iced::widget::Space::new().height(Length::Fill))
            .push(text::h3("Duress Mode Activated"))
            .push(
                text::p1_regular(
                    "Use the COINCUBE app on a trusted device to manage your account.",
                )
                .color(color::GREY_3),
            );

        let sign_in = if self.checking {
            button::primary(None, "Checking…").width(Length::Fixed(220.0))
        } else {
            button::primary(None, "Sign in to Connect")
                .width(Length::Fixed(220.0))
                .on_press(Message::SignInPressed)
        };
        col = col.push(iced::widget::Space::new().height(Length::Fixed(8.0)));
        col = col.push(sign_in);

        if let Some(err) = &self.error {
            col = col.push(text::p2_regular(err).color(color::GREY_3));
        }

        col = col.push(iced::widget::Space::new().height(Length::Fill));

        Container::new(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .into()
    }
}
