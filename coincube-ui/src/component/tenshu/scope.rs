//! Two wrappers the form controls share.
//!
//! [`KeyScope`] sees a key press before its content does and may answer it
//! with messages, knowing which of its descendants holds focus. Segmented
//! choices use it for arrow keys and search fields for Escape.
//!
//! [`Inert`] hides its content from focus operations, so Tab and whole-tree
//! focus skip a disabled text field the way they skip a disabled action.

use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::operation::{Focusable, Scrollable, TextInput};
use iced::advanced::widget::tree::Tree;
use iced::advanced::widget::{Id, Operation, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::keyboard;
use iced::{mouse, Event, Length, Rectangle, Size, Vector};

use crate::theme::Theme;
use crate::widget::{Element, Renderer};

/// Answers a key press given the ID of the focused descendant, if any.
pub(crate) type KeyHandler<'a, M> =
    Box<dyn Fn(&keyboard::Key, keyboard::Modifiers, Option<&Id>) -> Option<Vec<M>> + 'a>;

pub(crate) struct KeyScope<'a, M> {
    content: Element<'a, M>,
    on_key: KeyHandler<'a, M>,
}

impl<'a, M> KeyScope<'a, M> {
    pub(crate) fn new(content: impl Into<Element<'a, M>>, on_key: KeyHandler<'a, M>) -> Self {
        Self {
            content: content.into(),
            on_key,
        }
    }
}

/// The ID of the focused descendant.
#[derive(Default)]
struct FocusedId(Option<Id>);

impl Operation for FocusedId {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
        if state.is_focused() {
            self.0 = id.cloned();
        }
    }
}

impl<M> Widget<M, Theme, Renderer> for KeyScope<'_, M> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        if let Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            modifiers,
            repeat: false,
            ..
        }) = event
        {
            // A sibling earlier in the tree (a focused top-bar action, say)
            // already answered this key: it is not ours too.
            if shell.is_event_captured() {
                self.content.as_widget_mut().update(
                    &mut tree.children[0],
                    event,
                    layout,
                    cursor,
                    renderer,
                    clipboard,
                    shell,
                    viewport,
                );
                return;
            }
            let mut focused = FocusedId::default();
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                renderer,
                &mut focused,
            );
            if let Some(messages) = (self.on_key)(key, *modifiers, focused.0.as_ref()) {
                for message in messages {
                    shell.publish(message);
                }
                shell.capture_event();
                return;
            }
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
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
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, M: 'a> From<KeyScope<'a, M>> for Element<'a, M> {
    fn from(scope: KeyScope<'a, M>) -> Self {
        Element::new(scope)
    }
}

/// Content that never registers as a focus target or text input.
pub(super) struct Inert<'a, M> {
    content: Element<'a, M>,
}

impl<'a, M> Inert<'a, M> {
    pub(super) fn new(content: impl Into<Element<'a, M>>) -> Self {
        Self {
            content: content.into(),
        }
    }
}

/// Forwards every operation except focus and text-input targets.
struct WithoutFocus<'o, 'p> {
    inner: &'o mut (dyn Operation + 'p),
}

impl Operation for WithoutFocus<'_, '_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        self.inner.traverse(&mut |inner| {
            operate(&mut WithoutFocus { inner });
        });
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
        self.inner.container(id, bounds);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        self.inner
            .scrollable(id, bounds, content_bounds, translation, state);
    }

    fn focusable(&mut self, _id: Option<&Id>, _bounds: Rectangle, _state: &mut dyn Focusable) {}

    fn text_input(&mut self, _id: Option<&Id>, _bounds: Rectangle, _state: &mut dyn TextInput) {}

    fn text(&mut self, id: Option<&Id>, bounds: Rectangle, text: &str) {
        self.inner.text(id, bounds, text);
    }

    fn custom(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn std::any::Any) {
        self.inner.custom(id, bounds, state);
    }
}

impl<M> Widget<M, Theme, Renderer> for Inert<'_, M> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            renderer,
            &mut WithoutFocus { inner: operation },
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
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
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
}

impl<'a, M: 'a> From<Inert<'a, M>> for Element<'a, M> {
    fn from(inert: Inert<'a, M>) -> Self {
        Element::new(inert)
    }
}
