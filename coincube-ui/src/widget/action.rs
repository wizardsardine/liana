//! An Iced button with a real focus target and keyboard activation.
//!
//! The inner `Button` retains its pointer hit target and theme style. Only an
//! enabled action registers with Iced's focus operations, so Tab skips disabled
//! actions. Stable semantic IDs let a view restore focus after a disclosure.

use crate::theme::Theme;
use crate::widget::{Button, Element, Renderer};
use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::operation::Focusable;
use iced::advanced::widget::operation::{self, Outcome};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::widget::{Id, Operation, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::keyboard::{self, key};
use iced::mouse;
use iced::widget::button;
use iced::{Border, Color, Event, Length, Padding, Rectangle, Size, Vector};

const FOCUS_RING_GAP: f32 = 2.0;
const FOCUS_RING_WIDTH: f32 = 2.0;
const FOCUS_RING_OUTSET: f32 = FOCUS_RING_GAP + FOCUS_RING_WIDTH;

thread_local! {
    /// Whether focus last moved by keyboard navigation, as a browser's
    /// `:focus-visible` decides. A click still focuses the action, so Enter
    /// and Tab continue from it, but only keyboard navigation paints the ring.
    /// It starts off, so a control focused before any input (a selected
    /// sidebar row) shows no ring. One flag per UI thread: every window
    /// shares the user's current modality.
    static KEYBOARD_MODALITY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn note_modality(event: &Event) {
    let keyboard = match event {
        // Tab and the arrow keys navigate. Other keys (Space or Escape after a
        // click, typing, modifiers) keep the current modality.
        Event::Keyboard(keyboard::Event::KeyPressed {
            key:
                keyboard::Key::Named(
                    key::Named::Tab
                    | key::Named::ArrowUp
                    | key::Named::ArrowDown
                    | key::Named::ArrowLeft
                    | key::Named::ArrowRight,
                ),
            ..
        }) => true,
        Event::Mouse(mouse::Event::ButtonPressed(_))
        | Event::Touch(iced::touch::Event::FingerPressed { .. }) => false,
        _ => return,
    };
    KEYBOARD_MODALITY.with(|modality| modality.set(keyboard));
}

fn focus_visible() -> bool {
    KEYBOARD_MODALITY.with(std::cell::Cell::get)
}

fn focus_outer_bounds(bounds: Rectangle) -> Rectangle {
    Rectangle {
        x: bounds.x - FOCUS_RING_OUTSET,
        y: bounds.y - FOCUS_RING_OUTSET,
        width: bounds.width + FOCUS_RING_OUTSET * 2.0,
        height: bounds.height + FOCUS_RING_OUTSET * 2.0,
    }
}

/// Move focus in visual tree order, then reveal it inside the named scrollable.
/// The second operation runs after Iced's focus operation completes.
pub fn focus_next_visible<Message: Send + 'static>(
    scroll_id: impl Into<Id>,
) -> iced::Task<Message> {
    iced::widget::operation::focus_next().chain(reveal_focused(scroll_id.into()))
}

pub fn focus_previous_visible<Message: Send + 'static>(
    scroll_id: impl Into<Id>,
) -> iced::Task<Message> {
    iced::widget::operation::focus_previous().chain(reveal_focused(scroll_id.into()))
}

/// Reveal the currently focused action inside a named scrollable, if present.
/// Chaining this after another focus/reveal operation is safe when the named
/// scrollable is absent or the focused widget is outside it.
pub fn reveal_focused<Message: Send + 'static>(scroll_id: impl Into<Id>) -> iced::Task<Message> {
    iced_runtime::task::widget(FocusedScrollDelta::new(scroll_id.into())).then(|delta| {
        iced::widget::operation::scroll_by(
            delta.id,
            iced::widget::operation::AbsoluteOffset { x: 0.0, y: delta.y },
        )
    })
}

#[derive(Debug, Clone)]
struct ScrollDelta {
    id: Id,
    y: f32,
}

#[derive(Debug, Clone)]
struct ScrollContext {
    depth: usize,
    bounds: Rectangle,
    translation: Vector,
}

struct FocusedScrollDelta {
    target: Id,
    depth: usize,
    scroll: Option<ScrollContext>,
    delta: Option<ScrollDelta>,
}

impl FocusedScrollDelta {
    fn new(target: Id) -> Self {
        Self {
            target,
            depth: 0,
            scroll: None,
            delta: None,
        }
    }
}

impl Operation<ScrollDelta> for FocusedScrollDelta {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<ScrollDelta>)) {
        self.depth += 1;
        operate(self);
        self.depth -= 1;
        if self
            .scroll
            .as_ref()
            .is_some_and(|scroll| scroll.depth == self.depth)
        {
            self.scroll = None;
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn operation::Scrollable,
    ) {
        if id == Some(&self.target) {
            self.scroll = Some(ScrollContext {
                depth: self.depth,
                bounds,
                translation,
            });
        }
    }

    fn focusable(&mut self, _id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
        let Some(scroll) = self.scroll.as_ref() else {
            return;
        };
        if !state.is_focused() {
            return;
        }
        let screen_top = bounds.y - scroll.translation.y;
        let screen_bottom = screen_top + bounds.height;
        // The ring's outer edge is four pixels beyond the hit target. Shell
        // scroll content reserves this space at both ends so it can be shown
        // even when the first or last action is focused.
        let reveal_margin = FOCUS_RING_OUTSET;
        let delta = if screen_top < scroll.bounds.y + reveal_margin {
            screen_top - scroll.bounds.y - reveal_margin
        } else if screen_bottom > scroll.bounds.y + scroll.bounds.height - reveal_margin {
            screen_bottom - scroll.bounds.y - scroll.bounds.height + reveal_margin
        } else {
            0.0
        };
        if delta != 0.0 {
            self.delta = Some(ScrollDelta {
                id: self.target.clone(),
                y: delta,
            });
        }
    }

    fn finish(&self) -> Outcome<ScrollDelta> {
        self.delta.clone().map_or(Outcome::None, Outcome::Some)
    }
}

pub struct Action<'a, Message> {
    id: Id,
    button: Button<'a, Message>,
    on_press: Option<Message>,
    on_focus: Message,
    focus_radius: f32,
}

impl<'a, Message: Clone + 'a> Action<'a, Message> {
    /// `focus_intent` is required: the owner must apply Iced's whole-tree
    /// `operation::focus(id)` when the pointer presses this action. A local
    /// focus flag alone cannot blur controls behind a capturing split widget.
    pub fn new(
        id: impl Into<Id>,
        content: impl Into<Element<'a, Message>>,
        focus_intent: impl FnOnce(Id) -> Message,
    ) -> Self {
        let id = id.into();
        Self {
            on_focus: focus_intent(id.clone()),
            id,
            button: Button::new(content),
            on_press: None,
            focus_radius: 8.0,
        }
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.button = self.button.width(width);
        self
    }

    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.button = self.button.height(height);
        self
    }

    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.button = self.button.padding(padding);
        self
    }

    pub fn style(mut self, style: impl Fn(&Theme, button::Status) -> button::Style + 'a) -> Self {
        self.button = self.button.style(style);
        self
    }

    pub fn focus_radius(mut self, radius: f32) -> Self {
        self.focus_radius = radius;
        self
    }

    pub fn on_press(mut self, message: Message) -> Self {
        self.button = self.button.on_press(message.clone());
        self.on_press = Some(message);
        self
    }

    pub fn on_press_maybe(mut self, message: Option<Message>) -> Self {
        self.button = self.button.on_press_maybe(message.clone());
        self.on_press = message;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivationKey {
    Enter,
    Space,
}

impl ActivationKey {
    fn from_key(key: &keyboard::Key) -> Option<Self> {
        match key {
            keyboard::Key::Named(key::Named::Enter) => Some(Self::Enter),
            keyboard::Key::Named(key::Named::Space) => Some(Self::Space),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct State {
    id: Id,
    focused: bool,
    held: Option<ActivationKey>,
    pointer_active: bool,
    pointer_suppressed: bool,
}

impl Focusable for State {
    fn is_focused(&self) -> bool {
        self.focused
    }

    fn focus(&mut self) {
        self.focused = true;
    }

    fn unfocus(&mut self) {
        self.focused = false;
        self.held = None;
    }
}

impl<Message: Clone + 'static> Action<'_, Message> {
    fn update_keyboard(&self, state: &mut State, event: &Event, shell: &mut Shell<'_, Message>) {
        match event {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                repeat,
                ..
            }) if state.focused && !*repeat && !modifiers.command() && !modifiers.alt() => {
                if let Some(activation) = ActivationKey::from_key(key) {
                    if state.held.is_none() {
                        state.held = Some(activation);
                        if state.pointer_active {
                            state.pointer_suppressed = true;
                        }
                        shell.publish(self.on_press.as_ref().unwrap().clone());
                        shell.capture_event();
                    }
                }
            }
            Event::Keyboard(keyboard::Event::KeyReleased { key, .. })
                if state.held == ActivationKey::from_key(key) =>
            {
                state.held = None;
            }
            _ => {}
        }
    }
}

impl<Message: Clone + 'static> Widget<Message, Theme, Renderer> for Action<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            id: self.id.clone(),
            focused: false,
            held: None,
            pointer_active: false,
            pointer_suppressed: false,
        })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(
            &self.button as &dyn Widget<Message, Theme, Renderer>,
        )]
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        if state.id != self.id || self.on_press.is_none() {
            state.unfocus();
            state.pointer_active = false;
            state.pointer_suppressed = false;
            state.id = self.id.clone();
            // The wrapped Button's pressed state must not survive a semantic
            // replacement or become actionable again after a disabled pass.
            tree.children[0] = Tree::new(&self.button as &dyn Widget<Message, Theme, Renderer>);
        } else {
            tree.diff_children(&[&self.button as &dyn Widget<Message, Theme, Renderer>]);
        }
    }

    fn size(&self) -> Size<Length> {
        self.button.size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.button.layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.on_press.is_some() {
            operation.focusable(
                Some(&self.id),
                layout.bounds(),
                tree.state.downcast_mut::<State>(),
            );
        }
        operation.traverse(&mut |operation| {
            self.button
                .operate(&mut tree.children[0], layout, renderer, operation);
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let pointer_press = matches!(
            event,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                | Event::Touch(iced::touch::Event::FingerPressed { .. })
        );
        let pointer_release = matches!(
            event,
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                | Event::Touch(iced::touch::Event::FingerLifted { .. })
        );
        let cancel_pointer = matches!(
            event,
            Event::Window(iced::window::Event::Unfocused)
                | Event::Touch(iced::touch::Event::FingerLost { .. })
        );
        note_modality(event);
        if pointer_press {
            let state = tree.state.downcast_mut::<State>();
            if self.on_press.is_some() && cursor.is_over(layout.bounds()) {
                state.focus();
                state.pointer_active = true;
                state.pointer_suppressed = state.held.is_some();
                shell.publish(self.on_focus.clone());
            } else {
                state.unfocus();
            }
        }

        let suppress_release =
            pointer_release && tree.state.downcast_ref::<State>().pointer_suppressed;
        if cancel_pointer && tree.state.downcast_ref::<State>().pointer_active {
            // The native Button only resets its pressed state on release or
            // touch loss. If the window blurs mid-click, cancel it without
            // publishing a stale click when the user returns.
            let mut canceled_messages = Vec::new();
            self.button.update(
                &mut tree.children[0],
                &Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                layout,
                mouse::Cursor::Unavailable,
                renderer,
                clipboard,
                &mut Shell::new(&mut canceled_messages),
                viewport,
            );
            debug_assert!(canceled_messages.is_empty());
        }
        self.button.update(
            &mut tree.children[0],
            event,
            layout,
            if suppress_release {
                mouse::Cursor::Unavailable
            } else {
                cursor
            },
            renderer,
            clipboard,
            shell,
            viewport,
        );
        if pointer_release || cancel_pointer {
            let state = tree.state.downcast_mut::<State>();
            state.pointer_active = false;
            state.pointer_suppressed = false;
        }
        if cancel_pointer {
            tree.state.downcast_mut::<State>().held = None;
        }
        if shell.is_event_captured() || self.on_press.is_none() {
            return;
        }

        let state = tree.state.downcast_mut::<State>();
        self.update_keyboard(state, event, shell);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.button.draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
        if self.on_press.is_some() && tree.state.downcast_ref::<State>().focused && focus_visible()
        {
            use iced::advanced::Renderer as _;
            renderer.fill_quad(
                renderer::Quad {
                    bounds: focus_outer_bounds(layout.bounds()),
                    border: Border {
                        color: theme.colors.shell.focus,
                        width: FOCUS_RING_WIDTH,
                        radius: (self.focus_radius + FOCUS_RING_OUTSET).into(),
                    },
                    ..Default::default()
                },
                Color::TRANSPARENT,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.button
            .mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.button.overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message: Clone + 'static> From<Action<'a, Message>> for Element<'a, Message> {
    fn from(action: Action<'a, Message>) -> Self {
        Self::new(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font;
    use iced::advanced::renderer::Headless;
    use iced::advanced::widget::operation::focusable;
    use iced::advanced::Layout;
    use iced::widget::{column, container, scrollable, text, text_input};

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Message {
        First,
        Second,
        Focus(Id),
        Input(String),
        Submitted,
    }

    fn load_fonts() {
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
    }

    fn tree() -> Element<'static, Message> {
        column![
            Action::new("first", text("First"), Message::Focus)
                .on_press(Message::First)
                .width(Length::Fixed(180.0)),
            Action::new("disabled", text("Disabled"), Message::Focus).on_press_maybe(None),
            text_input("Enter submits", "")
                .id("input")
                .on_input(Message::Input)
                .on_submit(Message::Submitted),
            Action::new("second", text("Second"), Message::Focus).on_press(Message::Second),
        ]
        .spacing(10)
        .into()
    }

    fn run_operation<T: 'static>(
        element: &mut Element<'_, Message>,
        tree: &mut Tree,
        node: &layout::Node,
        renderer: &Renderer,
        operation: impl Operation<T> + 'static,
    ) -> Option<T> {
        let mut operation: Box<dyn Operation<T>> = Box::new(operation);
        loop {
            element.as_widget_mut().operate(
                tree,
                Layout::new(node),
                renderer,
                &mut operation::black_box(operation.as_mut()),
            );
            match operation.finish() {
                Outcome::None => return None,
                Outcome::Some(result) => return Some(result),
                Outcome::Chain(next) => operation = next,
            }
        }
    }

    fn send_event(
        element: &mut Element<'_, Message>,
        tree: &mut Tree,
        node: &layout::Node,
        renderer: &Renderer,
        event: Event,
    ) -> Vec<Message> {
        let mut messages = Vec::new();
        element.as_widget_mut().update(
            tree,
            &event,
            Layout::new(node),
            mouse::Cursor::Unavailable,
            renderer,
            &mut iced::advanced::clipboard::Null,
            &mut Shell::new(&mut messages),
            &Rectangle::with_size(Size::new(260.0, 240.0)),
        );
        messages
    }

    fn key_event(named: key::Named, repeat: bool, pressed: bool) -> Event {
        let key = keyboard::Key::Named(named);
        let physical_key = key::Physical::Unidentified(key::NativeCode::Unidentified);
        let location = keyboard::Location::Standard;
        let modifiers = keyboard::Modifiers::empty();
        if pressed {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key,
                location,
                modifiers,
                text: None,
                repeat,
            })
        } else {
            Event::Keyboard(keyboard::Event::KeyReleased {
                key: key.clone(),
                modified_key: key,
                physical_key,
                location,
                modifiers,
            })
        }
    }

    #[test]
    fn tab_skips_disabled_and_enter_space_activate_once_per_press() {
        load_fonts();
        let renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            14.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut element = tree();
        let mut state = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut state,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(260.0, 240.0)),
        );

        run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            focusable::focus_next::<()>(),
        );
        assert_eq!(
            run_operation(
                &mut element,
                &mut state,
                &node,
                &renderer,
                focusable::find_focused(),
            ),
            Some(Id::new("first"))
        );
        assert_eq!(
            send_event(
                &mut element,
                &mut state,
                &node,
                &renderer,
                key_event(key::Named::Enter, false, true),
            ),
            vec![Message::First]
        );
        assert!(send_event(
            &mut element,
            &mut state,
            &node,
            &renderer,
            key_event(key::Named::Enter, true, true),
        )
        .is_empty());
        send_event(
            &mut element,
            &mut state,
            &node,
            &renderer,
            key_event(key::Named::Enter, false, false),
        );
        assert_eq!(
            send_event(
                &mut element,
                &mut state,
                &node,
                &renderer,
                key_event(key::Named::Space, false, true),
            ),
            vec![Message::First]
        );

        run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            focusable::focus_next::<()>(),
        );
        assert_eq!(
            run_operation(
                &mut element,
                &mut state,
                &node,
                &renderer,
                focusable::find_focused(),
            ),
            Some(Id::new("input"))
        );
        assert_eq!(
            send_event(
                &mut element,
                &mut state,
                &node,
                &renderer,
                key_event(key::Named::Enter, false, true),
            ),
            vec![Message::Submitted]
        );

        run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            focusable::focus_next::<()>(),
        );
        assert_eq!(
            run_operation(
                &mut element,
                &mut state,
                &node,
                &renderer,
                focusable::find_focused(),
            ),
            Some(Id::new("second"))
        );
        run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            focusable::focus_previous::<()>(),
        );
        assert_eq!(
            run_operation(
                &mut element,
                &mut state,
                &node,
                &renderer,
                focusable::find_focused(),
            ),
            Some(Id::new("input"))
        );
    }

    #[test]
    fn focus_ring_uses_shell_palette_with_two_pixel_gap_and_stroke() {
        load_fonts();
        for theme in [Theme::dark(), Theme::light()] {
            let mut renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
                font::REGULAR,
                14.into(),
                Some("tiny-skia"),
            ))
            .unwrap();
            let mut element: Element<'_, Message> = container(
                Action::new("ring", text("Focus"), Message::Focus)
                    .width(Length::Fixed(100.0))
                    .height(Length::Fixed(32.0))
                    .on_press(Message::First),
            )
            .padding(12)
            .into();
            let mut tree = Tree::new(element.as_widget());
            let size = Size::new(160.0, 56.0);
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, size),
            );
            let control = node.children()[0].bounds();
            assert_eq!(focus_outer_bounds(control).y, control.y - 4.0);
            // Only keyboard navigation shows the ring; Tab is how a keyboard
            // user reaches the action.
            send_event(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                key_event(key::Named::Tab, false, true),
            );
            run_operation(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus::<()>(Id::new("ring")),
            );
            element.as_widget().draw(
                &tree,
                &mut renderer,
                &theme,
                &renderer::Style {
                    text_color: theme.colors.shell.ink,
                },
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &Rectangle::with_size(size),
            );
            let pixels =
                renderer.screenshot(Size::new(160, 56), 1.0, theme.colors.shell.background);
            let sample = |x: u32, y: u32| {
                let i = ((y * 160 + x) * 4) as usize;
                [pixels[i], pixels[i + 1], pixels[i + 2]]
            };
            let x = (control.x + control.width / 2.0) as u32;
            let y = control.y as u32;
            let focus = theme.colors.shell.focus;
            let expected = [focus.r, focus.g, focus.b].map(|component| (component * 255.0) as u8);
            let ring = sample(x, y - 3);
            assert!(
                ring.iter()
                    .zip(expected)
                    .all(|(actual, expected)| actual.abs_diff(expected) <= 2),
                "2px outer ring must use shell focus token: {ring:?} != {expected:?}"
            );
            assert_ne!(
                sample(x, y - 1),
                ring,
                "2px gap must separate control and ring"
            );
            assert_ne!(
                sample(x, y - 5),
                ring,
                "stroke must end 4px outside control"
            );
        }
    }

    #[test]
    fn only_keyboard_navigation_shows_the_ring() {
        load_fonts();
        let theme = Theme::dark();
        let renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            14.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut element: Element<'_, Message> = container(
            Action::new("ring", text("Focus"), Message::Focus)
                .width(Length::Fixed(100.0))
                .height(Length::Fixed(32.0))
                .on_press(Message::First),
        )
        .padding(12)
        .into();
        let mut tree = Tree::new(element.as_widget());
        let size = Size::new(160.0, 56.0);
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        let control = node.children()[0].bounds();
        let focus = theme.colors.shell.focus;
        let expected = [focus.r, focus.g, focus.b].map(|component| (component * 255.0) as u8);
        // A fresh renderer per frame: the headless one keeps earlier layers.
        let ring_painted = |element: &Element<'_, Message>, tree: &Tree| {
            let mut renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
                font::REGULAR,
                14.into(),
                Some("tiny-skia"),
            ))
            .unwrap();
            element.as_widget().draw(
                tree,
                &mut renderer,
                &theme,
                &renderer::Style {
                    text_color: theme.colors.shell.ink,
                },
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &Rectangle::with_size(size),
            );
            let pixels =
                renderer.screenshot(Size::new(160, 56), 1.0, theme.colors.shell.background);
            let i = (((control.y as u32 - 3) * 160 + (control.x + control.width / 2.0) as u32) * 4)
                as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2]]
                .iter()
                .zip(expected)
                .all(|(actual, expected)| actual.abs_diff(expected) <= 2)
        };

        let cursor = mouse::Cursor::Available(control.center());
        let dispatch = |renderer: &Renderer,
                        element: &mut Element<'_, Message>,
                        tree: &mut Tree,
                        event: Event| {
            let mut messages = Vec::new();
            element.as_widget_mut().update(
                tree,
                &event,
                Layout::new(&node),
                cursor,
                renderer,
                &mut iced::advanced::clipboard::Null,
                &mut Shell::new(&mut messages),
                &Rectangle::with_size(size),
            );
            messages
        };
        let click = |renderer: &Renderer, element: &mut Element<'_, Message>, tree: &mut Tree| {
            let pressed = dispatch(
                renderer,
                element,
                tree,
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            );
            dispatch(
                renderer,
                element,
                tree,
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            );
            pressed
        };

        // A control focused before any input, like the selected sidebar row
        // of a newly opened Cube, shows no ring.
        run_operation(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::focus::<()>(Id::new("ring")),
        );
        assert!(
            !ring_painted(&element, &tree),
            "focus without keyboard navigation paints no ring"
        );

        assert_eq!(
            click(&renderer, &mut element, &mut tree),
            vec![Message::Focus(Id::new("ring"))]
        );
        assert!(!ring_painted(&element, &tree), "a click paints no ring");

        // Keys that don't navigate keep the click's modality: a modifier
        // (Cmd-click), Space (activates the action), Escape and page scrolling.
        for named in [
            key::Named::Super,
            key::Named::Space,
            key::Named::Escape,
            key::Named::PageDown,
        ] {
            dispatch(
                &renderer,
                &mut element,
                &mut tree,
                key_event(named, false, true),
            );
            dispatch(
                &renderer,
                &mut element,
                &mut tree,
                key_event(named, false, false),
            );
            assert!(
                !ring_painted(&element, &tree),
                "{named:?} after a click paints no ring"
            );
        }

        for named in [key::Named::Tab, key::Named::ArrowDown] {
            dispatch(
                &renderer,
                &mut element,
                &mut tree,
                key_event(named, false, true),
            );
            assert!(
                ring_painted(&element, &tree),
                "{named:?} is keyboard navigation and shows the ring"
            );
            click(&renderer, &mut element, &mut tree);
            assert!(
                !ring_painted(&element, &tree),
                "the next click hides it again"
            );
        }
    }

    #[test]
    fn focused_middle_action_is_revealed_by_scroll_operation() {
        load_fonts();
        let renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            14.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut element: Element<'_, Message> = scrollable(
            column![
                Action::new("first", text("First"), Message::Focus)
                    .on_press(Message::First)
                    .height(Length::Fixed(32.0)),
                Action::new("second", text("Second"), Message::Focus)
                    .on_press(Message::Second)
                    .height(Length::Fixed(32.0)),
            ]
            .spacing(24)
            .padding([6, 10]),
        )
        .id("scroll")
        .height(Length::Fixed(40.0))
        .into();
        let mut state = Tree::new(element.as_widget());
        let mut node = element.as_widget_mut().layout(
            &mut state,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(260.0, 40.0)),
        );
        run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            focusable::focus::<()>(Id::new("second")),
        );
        let delta = run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            FocusedScrollDelta::new(Id::new("scroll")),
        )
        .expect("offscreen focused action needs a scroll delta");
        assert!(delta.y > 0.0);
        run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            operation::scrollable::scroll_by::<()>(
                delta.id,
                operation::scrollable::AbsoluteOffset { x: 0.0, y: delta.y },
            ),
        );
        node = element.as_widget_mut().layout(
            &mut state,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(260.0, 40.0)),
        );
        assert!(
            run_operation(
                &mut element,
                &mut state,
                &node,
                &renderer,
                FocusedScrollDelta::new(Id::new("scroll")),
            )
            .is_none(),
            "focused action should now be fully visible"
        );
        run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            focusable::focus::<()>(Id::new("first")),
        );
        let delta = run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            FocusedScrollDelta::new(Id::new("scroll")),
        )
        .expect("first action's outer ring needs a reverse scroll delta");
        assert!(delta.y < 0.0);
        run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            operation::scrollable::scroll_by::<()>(
                delta.id,
                operation::scrollable::AbsoluteOffset { x: 0.0, y: delta.y },
            ),
        );
        node = element.as_widget_mut().layout(
            &mut state,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(260.0, 40.0)),
        );
        assert!(
            run_operation(
                &mut element,
                &mut state,
                &node,
                &renderer,
                FocusedScrollDelta::new(Id::new("scroll")),
            )
            .is_none(),
            "first action's outer ring should be fully visible"
        );
    }

    #[test]
    fn window_focus_loss_clears_held_activation_key() {
        load_fonts();
        let renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            14.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut element = tree();
        let mut state = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut state,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(260.0, 240.0)),
        );
        run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            focusable::focus::<()>(Id::new("first")),
        );
        assert_eq!(
            send_event(
                &mut element,
                &mut state,
                &node,
                &renderer,
                key_event(key::Named::Enter, false, true),
            ),
            vec![Message::First]
        );
        assert!(send_event(
            &mut element,
            &mut state,
            &node,
            &renderer,
            Event::Window(iced::window::Event::Unfocused),
        )
        .is_empty());
        assert_eq!(
            send_event(
                &mut element,
                &mut state,
                &node,
                &renderer,
                key_event(key::Named::Enter, false, true),
            ),
            vec![Message::First],
            "a fresh key press after returning to the window must activate"
        );
    }

    #[test]
    fn overlapping_pointer_and_keyboard_activation_emits_once() {
        load_fonts();
        let renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            14.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut element: Element<'_, Message> = Action::new("first", text("First"), Message::Focus)
            .on_press(Message::First)
            .into();
        let mut state = Tree::new(element.as_widget());
        let size = Size::new(260.0, 80.0);
        let node = element.as_widget_mut().layout(
            &mut state,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        let cursor = mouse::Cursor::Available(node.bounds().center());
        let mut dispatch = |event, cursor| {
            let mut messages = Vec::new();
            element.as_widget_mut().update(
                &mut state,
                &event,
                Layout::new(&node),
                cursor,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut Shell::new(&mut messages),
                &Rectangle::with_size(size),
            );
            messages
        };
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let release = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        let enter_down = key_event(key::Named::Enter, false, true);
        let enter_up = key_event(key::Named::Enter, false, false);

        assert_eq!(
            dispatch(press.clone(), cursor),
            vec![Message::Focus(Id::new("first"))]
        );
        assert_eq!(dispatch(enter_down.clone(), cursor), vec![Message::First]);
        assert!(dispatch(release.clone(), cursor).is_empty());
        assert!(dispatch(enter_up.clone(), cursor).is_empty());

        assert_eq!(dispatch(enter_down.clone(), cursor), vec![Message::First]);
        assert_eq!(
            dispatch(press.clone(), cursor),
            vec![Message::Focus(Id::new("first"))]
        );
        assert!(dispatch(release.clone(), cursor).is_empty());
        assert!(dispatch(enter_up.clone(), cursor).is_empty());

        assert_eq!(
            dispatch(press.clone(), cursor),
            vec![Message::Focus(Id::new("first"))]
        );
        assert_eq!(
            dispatch(key_event(key::Named::Space, false, true), cursor),
            vec![Message::First]
        );
        assert!(dispatch(release.clone(), cursor).is_empty());
        assert!(dispatch(key_event(key::Named::Space, false, false), cursor).is_empty());

        assert_eq!(
            dispatch(press.clone(), cursor),
            vec![Message::Focus(Id::new("first"))]
        );
        assert_eq!(dispatch(release.clone(), cursor), vec![Message::First]);
        assert_eq!(
            dispatch(press.clone(), cursor),
            vec![Message::Focus(Id::new("first"))]
        );
        assert!(dispatch(Event::Window(iced::window::Event::Unfocused), cursor).is_empty());
        assert!(dispatch(release, cursor).is_empty());
        let finger = iced::touch::Finger(1);
        let position = node.bounds().center();
        assert_eq!(
            dispatch(
                Event::Touch(iced::touch::Event::FingerPressed {
                    id: finger,
                    position
                }),
                cursor,
            ),
            vec![Message::Focus(Id::new("first"))]
        );
        assert!(dispatch(
            Event::Touch(iced::touch::Event::FingerLost {
                id: finger,
                position
            }),
            cursor,
        )
        .is_empty());
        assert!(dispatch(
            Event::Touch(iced::touch::Event::FingerLifted {
                id: finger,
                position
            }),
            cursor,
        )
        .is_empty());
        assert_eq!(dispatch(enter_down, cursor), vec![Message::First]);
    }

    #[test]
    fn replaced_or_disabled_action_drops_focus_and_pressed_state() {
        load_fonts();
        let renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            14.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        let size = Size::new(260.0, 80.0);
        let mut element: Element<'_, Message> = Action::new("first", text("First"), Message::Focus)
            .on_press(Message::First)
            .into();
        let mut state = Tree::new(element.as_widget());
        let mut node = element.as_widget_mut().layout(
            &mut state,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        let cursor = mouse::Cursor::Available(node.bounds().center());
        run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            focusable::focus::<()>(Id::new("first")),
        );
        let mut messages = Vec::new();
        element.as_widget_mut().update(
            &mut state,
            &Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut Shell::new(&mut messages),
            &Rectangle::with_size(size),
        );
        assert_eq!(messages, vec![Message::Focus(Id::new("first"))]);

        element = Action::new("replacement", text("Second"), Message::Focus)
            .on_press(Message::Second)
            .into();
        state.diff(element.as_widget());
        node = element.as_widget_mut().layout(
            &mut state,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        assert_eq!(
            run_operation(
                &mut element,
                &mut state,
                &node,
                &renderer,
                focusable::find_focused()
            ),
            None
        );
        messages.clear();
        element.as_widget_mut().update(
            &mut state,
            &Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut Shell::new(&mut messages),
            &Rectangle::with_size(size),
        );
        assert!(
            messages.is_empty(),
            "replacement must not inherit a pending click"
        );

        run_operation(
            &mut element,
            &mut state,
            &node,
            &renderer,
            focusable::focus::<()>(Id::new("replacement")),
        );
        element = Action::new("replacement", text("Disabled"), Message::Focus)
            .on_press_maybe(None)
            .into();
        state.diff(element.as_widget());
        node = element.as_widget_mut().layout(
            &mut state,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        assert_eq!(
            run_operation(
                &mut element,
                &mut state,
                &node,
                &renderer,
                focusable::find_focused()
            ),
            None
        );
        assert!(send_event(
            &mut element,
            &mut state,
            &node,
            &renderer,
            key_event(key::Named::Enter, false, true)
        )
        .is_empty());
    }
}
