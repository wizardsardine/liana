//! The cryptic "Duress Mode Activated" screen (Phase 3 emits it; Phase 5 wires
//! the three exit paths and the gated Sign-in behaviour).
//!
//! This screen replaces every other view. It reveals nothing recoverable and
//! offers no recovery affordances on this device: no countdown, no remaining
//! window, no balances, no Cube list, no support link. Its **only** interactive
//! element is a single "Sign in to Connect" button which gates entirely on
//! server-side duress state — while duress is active it shows an inline "Try
//! again later" and **never** a credential prompt. Only when the server reports
//! duress cleared does the screen exit into the normal flow.

use coincube_core::miniscript::bitcoin::Network;
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::{
    color,
    component::{button, text},
    widget::{Column, Container, Element},
};
use iced::{Alignment, Length};

use crate::dir::CoincubeDirectory;

#[derive(Debug, Clone)]
pub enum Message {
    /// User tapped "Sign in to Connect" — triggers a `get_duress_state` check.
    SignInPressed,
    /// Result of the check: `Some(true)` still active, `Some(false)` cleared,
    /// `None` couldn't reach/parse the server. Only `Some(false)` exits the
    /// screen; everything else stays put with "Try again later".
    StateChecked(Option<bool>),
}

/// Cryptic-screen state. Holds just enough context to run the gated Sign-in
/// check and, on a server-side clear, hand back to the normal launch flow. It
/// caches nothing sensitive.
#[derive(Default)]
pub struct DuressActiveScreen {
    /// Inline error shown beneath the button (e.g. "Duress mode is active.
    /// Try again later.").
    pub error: Option<String>,
    /// True while a server-side state check is in flight.
    pub checking: bool,
    /// Whether the retry queue still has pending work — drives the subtle
    /// corner dot (Phase 4 Task 4.2). No text, no explanation.
    pub queue_pending: bool,
    /// Data directory + network, used to locate cached Connect auth
    /// (`<network>/connect.json`, preserved through the wipe) for the Sign-in
    /// check and to rebuild the Home flow on exit. `network` is `None` only on
    /// a launch reconcile where the network wasn't resolved.
    datadir: Option<CoincubeDirectory>,
    network: Option<Network>,
}

impl DuressActiveScreen {
    pub fn new() -> Self {
        Self::default()
    }

    /// Constructs the screen with the context needed for the gated Sign-in
    /// check and a clean exit.
    pub fn with_context(datadir: CoincubeDirectory, network: Option<Network>) -> Self {
        Self {
            datadir: Some(datadir),
            network,
            ..Self::default()
        }
    }

    pub fn datadir(&self) -> Option<&CoincubeDirectory> {
        self.datadir.as_ref()
    }

    pub fn network(&self) -> Option<Network> {
        self.network
    }

    pub fn view(&self) -> Element<Message> {
        let mut col = Column::new()
            .width(Length::Fill)
            .spacing(16)
            .align_x(Alignment::Center)
            .push(iced::widget::Space::new().height(Length::Fill))
            .push(text::h3(localized_copy("Duress Mode Activated")))
            .push(
                text::p1_regular(localized_copy(
                    "Use COINCUBE Desktop on a trusted device to manage your account.",
                ))
                .color(color::GREY_3),
            );

        let sign_in = if self.checking {
            button::primary(None, localized_copy("Checking…")).width(Length::Fixed(220.0))
        } else {
            button::primary(None, localized_copy("Sign in to Connect"))
                .width(Length::Fixed(220.0))
                .on_press(Message::SignInPressed)
        };
        col = col.push(iced::widget::Space::new().height(Length::Fixed(8.0)));
        col = col.push(sign_in);

        if let Some(err) = &self.error {
            col = col.push(text::p2_regular(err).color(color::GREY_3));
        }

        col = col.push(iced::widget::Space::new().height(Length::Fill));

        // Subtle pending-work indicator (Phase 4 Task 4.2): a single muted dot,
        // no text, no explanation — an attacker can infer nothing from it. Only
        // shown while the activation POST is still queued.
        if self.queue_pending {
            col = col.push(
                Container::new(text::p2_regular("•").color(color::GREY_5))
                    .width(Length::Fill)
                    .align_right(Length::Fill),
            );
        }

        Container::new(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .into()
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use coincube_ui::theme;
    use iced::advanced::{layout, renderer::Headless, widget::Tree, Layout};
    use iced::{mouse, Rectangle, Size};

    #[test]
    fn active_screen_fits_minimum_window_and_keeps_one_action() {
        let screen = DuressActiveScreen::new();
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            let inspection = capture::inspect_full_window(
                "duress-active",
                screen.view(),
                (1024, 768),
                appearance,
                theme,
            );
            assert!(inspection.contains("Duress Mode Activated"));
            assert!(inspection.contains("Sign in to Connect"));
            assert!(!inspection.contains("Recovery Kit"));
            assert!(!inspection.contains("All-clear"));

            // The shared content capture wraps views in a scrollable. Draw
            // this full-screen state directly too, so Fill height and its
            // vertical centering are checked at the real minimum viewport.
            let mut renderer =
                iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
                    coincube_ui::font::REGULAR,
                    iced::Pixels(16.0),
                    Some("tiny-skia"),
                ))
                .expect("software renderer for synthetic duress screen");
            let size = Size::new(1024.0, 768.0);
            let mut element = screen.view();
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, size),
            );
            assert_eq!(node.size().height, size.height);
            let viewport = Rectangle::with_size(size);
            element.as_widget_mut().update(
                &mut tree,
                &iced::Event::Window(iced::window::Event::RedrawRequested(
                    std::time::Instant::now(),
                )),
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut Vec::new()),
                &viewport,
            );
            element.as_widget().draw(
                &tree,
                &mut renderer,
                &theme,
                &iced::advanced::renderer::Style {
                    text_color: theme.colors.text.primary,
                },
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &viewport,
            );
            if let Ok(directory) = std::env::var("R06A_CAPTURE_DIR") {
                std::fs::create_dir_all(&directory).expect("create synthetic capture directory");
                let pixels =
                    renderer.screenshot(Size::new(1024, 768), 1.0, theme.colors.shell.background);
                image::save_buffer(
                    std::path::Path::new(&directory).join(format!(
                        "duress-active-native-{appearance}-1024x768-synthetic.png"
                    )),
                    &pixels,
                    1024,
                    768,
                    image::ColorType::Rgba8,
                )
                .expect("save synthetic active screen");
            }
        }
    }
}
