//! Presentation-only entry to the native proposal flow.
use super::{proposal, Context, Message, Step};
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use coincube_ui::{
    component::{
        button, card,
        text::{caption, h3, h4_bold, text},
    },
    widget::Element,
};
use iced::{
    widget::{column, row, scrollable},
    Length,
};
use std::sync::Arc;

pub(crate) const SCROLL_ID: &str = "proposal-handoff-scroll";

pub(crate) struct ProposalHandoff {
    session: Arc<proposal::Session>,
}

impl ProposalHandoff {
    pub(crate) fn new(session: Arc<proposal::Session>) -> Self {
        Self { session }
    }
}

impl Step for ProposalHandoff {
    fn apply(&mut self, _ctx: &mut Context) -> bool {
        self.session.active()
    }

    fn view<'a>(
        &'a self,
        _hws: &'a crate::hw::HardwareWallets,
        _progress: (usize, usize),
    ) -> Element<'a, Message> {
        let topology = self.session.topology();
        let active = self.session.active();
        let handoff = card::simple(
            column![
                caption(localized_copy(if active { "PROPOSAL PREPARED · WAITING FOR YOU" } else { "PROPOSAL ACCESS ENDED" })),
                h3(localized_copy(if active { "Continue setup in COINCUBE Desktop" } else { "Setup ended" })),
                text(localized_copy(if active {
                    "A paired client prepared a policy proposal. No Vault has been created. You will select the real keys and review the compiled descriptor in COINCUBE Desktop before creation."
                } else {
                    "This proposal can no longer continue. Return to the launcher, unlock this Cube, and ask the paired client to prepare a fresh proposal."
                })),
            ]
            .spacing(12),
        )
        .width(Length::Fill);

        let plan = card::simple(
            column![
                h4_bold(localized_copy("Proposed plan")),
                text(localized_format!(
                    "Primary · {} of {} public-key slots",
                    "Principal · {} de {} espacios de clave pública",
                    topology.primary.threshold, topology.primary.key_count
                )),
                text(localized_format!(
                    "Recovery · {} of {} keys after {} relative blocks for each coin",
                    "Recuperación · {} de {} claves después de {} bloques relativos para cada moneda",
                    topology.recovery.quorum.threshold,
                    topology.recovery.quorum.key_count,
                    topology.recovery.relative_delay_blocks
                )),
                caption(localized_copy("Block time varies; the delay is not a calendar deadline.")),
                text(localized_copy("Script · P2WSH")),
                text(format!("Network · {}", crate::chain::canonical_network_name(self.session.chain()))),
                text(localized_copy("Node · selected and verified in COINCUBE Desktop")),
            ]
            .spacing(10),
        )
        .width(Length::FillPortion(1));

        let checks = card::simple(
            column![
                h4_bold(localized_copy("What happens next")),
                text(localized_copy(
                    "1  Choose and check the real public keys in COINCUBE Desktop"
                )),
                text(localized_copy("2  Complete native backup and node steps")),
                text(localized_copy(
                    "3  Review the actual descriptor and approve creation"
                )),
                caption(localized_copy("Device control · not verified")),
                caption(localized_copy("Backup possession · not verified")),
                caption(localized_copy("Recovery test · not verified")),
            ]
            .spacing(10),
        )
        .width(Length::FillPortion(1));

        let content = column![
            caption(localized_copy("VAULT SETUP")),
            h3(localized_copy("Vault setup from a paired client")),
            text(localized_copy("This handoff prepares a Vault for this empty Cube. You approve creation in COINCUBE Desktop.")),
            handoff,
            row![plan, checks].spacing(16).width(Length::Fill),
            caption(localized_copy("No read access is granted. Ending setup locks this Cube and discards this proposal; pairing remains until revoked.")),
            row![
                button::primary_action("proposal-handoff-continue", None, localized_copy("Continue to key selection"), Message::FocusAction)
                    .on_press_maybe(active.then_some(Message::Next)),
                button::secondary_action("proposal-handoff-return", None, localized_copy(if active { "End setup" } else { "Return to launcher" }), Message::FocusAction)
                    .on_press(if active { Message::Previous } else { Message::BackToApp(self.session.chain().bitcoin_network()) }),
            ]
            .spacing(12),
        ]
        .spacing(16)
        .padding(24)
        .max_width(980);
        scrollable(content).id(SCROLL_ID).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installer::proposal::tests::{fixture, TestDir};
    use coincube_core::miniscript::bitcoin::Network;
    use coincube_ui::{font, theme, widget::Renderer};
    use iced::{
        advanced::{
            renderer::Headless,
            widget::{operation, operation::Focusable, Id, Operation},
        },
        keyboard::{self, key},
        mouse, Point, Rectangle, Size,
    };
    use iced_runtime::user_interface::{Cache, UserInterface};

    #[derive(Default)]
    struct Probe {
        labels: Vec<String>,
        focus: Vec<Id>,
        focus_bounds: Vec<(Id, Rectangle)>,
    }
    impl Operation for Probe {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn text(&mut self, _: Option<&Id>, _: Rectangle, value: &str) {
            self.labels.push(value.to_owned());
        }
        fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, _: &mut dyn Focusable) {
            if let Some(id) = id {
                self.focus.push(id.clone());
                self.focus_bounds.push((id.clone(), bounds));
            }
        }
    }

    #[test]
    fn proposal_handoff_shows_actual_plan_and_only_active_actions_focus() {
        let _guard = crate::app::session::test_guard();
        let dir = TestDir::new();
        let (mut ctx, session) = fixture(dir.path());
        let hws = crate::hw::HardwareWallets::new(
            crate::dir::CoincubeDirectory::new(dir.path().to_owned()),
            Network::Regtest,
        );
        let mut step = ProposalHandoff::new(session.clone());
        assert!(step.apply(&mut ctx));

        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        // `block_on` keeps this a synchronous test. An `.await` here would
        // hold `test_guard`'s MutexGuard across the await point.
        let mut renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            16.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        for (name, palette) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            let mut ui = UserInterface::build(
                step.view(&hws, (0, 8)),
                Size::new(1024.0, 768.0),
                Cache::new(),
                &mut renderer,
            );
            let mut probe = Probe::default();
            ui.operate(&renderer, &mut operation::black_box(&mut probe));
            assert!(probe
                .labels
                .iter()
                .any(|s| s == "PROPOSAL PREPARED · WAITING FOR YOU"));
            assert!(probe
                .labels
                .iter()
                .any(|s| s.contains("3 of 5 public-key slots")));
            assert!(probe
                .labels
                .iter()
                .any(|s| s.contains("selected and verified in COINCUBE Desktop")));
            assert!(probe
                .labels
                .iter()
                .any(|s| s == "Recovery test · not verified"));
            assert!(probe.labels.iter().any(|s| s == "End setup"));
            assert!(probe
                .labels
                .iter()
                .any(|s| s.contains("locks this Cube and discards this proposal")));
            assert!(probe.focus.contains(&Id::new("proposal-handoff-continue")));
            assert!(probe.focus.contains(&Id::new("proposal-handoff-return")));
            let mut messages = Vec::new();
            ui.update(
                &[iced::Event::Window(iced::window::Event::RedrawRequested(
                    std::time::Instant::now(),
                ))],
                mouse::Cursor::Unavailable,
                &mut renderer,
                &mut iced::advanced::clipboard::Null,
                &mut messages,
            );
            ui.draw(
                &mut renderer,
                &palette,
                &iced::advanced::renderer::Style {
                    text_color: palette.colors.shell.ink,
                },
                mouse::Cursor::Unavailable,
            );
            if let Ok(output) = std::env::var("AI_SETUP_CAPTURE_DIR") {
                std::fs::create_dir_all(&output).unwrap();
                let pixels =
                    renderer.screenshot(Size::new(1024, 768), 1.0, palette.colors.shell.background);
                image::save_buffer(
                    std::path::Path::new(&output)
                        .join(format!("ai-setup-handoff-{name}-1024x768.png")),
                    &pixels,
                    1024,
                    768,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
            if name == "dark" {
                let return_bounds = probe
                    .focus_bounds
                    .iter()
                    .find(|(id, _)| *id == Id::new("proposal-handoff-return"))
                    .unwrap()
                    .1;
                let cursor = mouse::Cursor::Available(Point::new(
                    return_bounds.x + return_bounds.width / 2.0,
                    return_bounds.y + return_bounds.height / 2.0,
                ));
                let mut messages = Vec::new();
                ui.update(
                    &[iced::Event::Mouse(mouse::Event::ButtonPressed(
                        mouse::Button::Left,
                    ))],
                    cursor,
                    &mut renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut messages,
                );
                assert!(matches!(
                    messages.as_slice(),
                    [Message::FocusAction(id)] if *id == Id::new("proposal-handoff-return")
                ));
                messages.clear();
                ui.update(
                    &[iced::Event::Mouse(mouse::Event::ButtonReleased(
                        mouse::Button::Left,
                    ))],
                    cursor,
                    &mut renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut messages,
                );
                assert!(matches!(messages.as_slice(), [Message::Previous]));
                messages.clear();
                ui.update(
                    &[iced::Event::Keyboard(keyboard::Event::KeyPressed {
                        key: keyboard::Key::Named(key::Named::Enter),
                        modified_key: keyboard::Key::Named(key::Named::Enter),
                        physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
                        location: keyboard::Location::Standard,
                        modifiers: keyboard::Modifiers::empty(),
                        text: None,
                        repeat: false,
                    })],
                    mouse::Cursor::Unavailable,
                    &mut renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut messages,
                );
                assert!(matches!(messages.as_slice(), [Message::Previous]));
            }
        }
        session.revoke();
        assert!(!step.apply(&mut ctx));
        let mut ui = UserInterface::build(
            step.view(&hws, (0, 8)),
            Size::new(1024.0, 768.0),
            Cache::new(),
            &mut renderer,
        );
        let mut probe = Probe::default();
        ui.operate(&renderer, &mut operation::black_box(&mut probe));
        assert!(probe.labels.iter().any(|s| s == "PROPOSAL ACCESS ENDED"));
        assert!(probe.labels.iter().any(|s| s == "Setup ended"));
        assert!(probe.labels.iter().any(|s| s == "Return to launcher"));
        assert!(!probe.focus.contains(&Id::new("proposal-handoff-continue")));
        assert!(probe.focus.contains(&Id::new("proposal-handoff-return")));
        let return_bounds = probe
            .focus_bounds
            .iter()
            .find(|(id, _)| *id == Id::new("proposal-handoff-return"))
            .unwrap()
            .1;
        let cursor = mouse::Cursor::Available(Point::new(
            return_bounds.x + return_bounds.width / 2.0,
            return_bounds.y + return_bounds.height / 2.0,
        ));
        let mut messages = Vec::new();
        ui.update(
            &[iced::Event::Mouse(mouse::Event::ButtonPressed(
                mouse::Button::Left,
            ))],
            cursor,
            &mut renderer,
            &mut iced::advanced::clipboard::Null,
            &mut messages,
        );
        assert!(matches!(
            messages.as_slice(),
            [Message::FocusAction(id)] if *id == Id::new("proposal-handoff-return")
        ));
        messages.clear();
        ui.update(
            &[iced::Event::Mouse(mouse::Event::ButtonReleased(
                mouse::Button::Left,
            ))],
            cursor,
            &mut renderer,
            &mut iced::advanced::clipboard::Null,
            &mut messages,
        );
        assert!(matches!(
            messages.as_slice(),
            [Message::BackToApp(Network::Regtest)]
        ));
        messages.clear();
        ui.update(
            &[iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key::Named::Enter),
                modified_key: keyboard::Key::Named(key::Named::Enter),
                physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: None,
                repeat: false,
            })],
            mouse::Cursor::Unavailable,
            &mut renderer,
            &mut iced::advanced::clipboard::Null,
            &mut messages,
        );
        assert!(matches!(
            messages.as_slice(),
            [Message::BackToApp(Network::Regtest)]
        ));
    }
}
