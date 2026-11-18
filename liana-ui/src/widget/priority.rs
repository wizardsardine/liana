//! Children of a [`crate::widget::PriorityRow`] that give way when the row runs out of width.
//!
//! Rank 1 is the most important and is degraded last; a larger rank gives way first.

use iced::{
    advanced::{
        layout, mouse, overlay,
        renderer::{self, Renderer as _},
        widget::{tree, Operation, Tree, Widget},
        Clipboard, Layout, Shell,
    },
    Event, Length, Rectangle, Size, Vector,
};

use crate::{theme::Theme, widget::Element};

type Renderer = iced::Renderer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Narrowed down to its minimum width, then hidden.
    Truncate,
    Hide,
}

/// Tree state of a [`Prioritized`]: its tag is how a priority row recognizes the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriorityState {
    pub mode: Mode,
    pub rank: u8,
}

pub struct Prioritized<'a, Message> {
    priority: PriorityState,
    content: Element<'a, Message>,
}

/// Narrow `child` when the row lacks width, hide it once it cannot get narrower.
pub fn truncate<'a, Message>(
    rank: u8,
    child: impl Into<Element<'a, Message>>,
) -> Prioritized<'a, Message> {
    Prioritized {
        priority: PriorityState {
            mode: Mode::Truncate,
            rank,
        },
        content: child.into(),
    }
}

/// Hide `child` when the row lacks width.
pub fn hide<'a, Message>(
    rank: u8,
    child: impl Into<Element<'a, Message>>,
) -> Prioritized<'a, Message> {
    Prioritized {
        priority: PriorityState {
            mode: Mode::Hide,
            rank,
        },
        content: child.into(),
    }
}

impl<'a, Message> Widget<Message, Theme, Renderer> for Prioritized<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<PriorityState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(self.priority)
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
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
        let draw = |renderer: &mut Renderer| {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            )
        };
        match self.priority.mode {
            // A narrowed child that cannot shrink its own content must not spill over its neighbours.
            Mode::Truncate => renderer.with_layer(layout.bounds(), draw),
            Mode::Hide => draw(renderer),
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
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
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

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message: 'a> From<Prioritized<'a, Message>> for Element<'a, Message> {
    fn from(prioritized: Prioritized<'a, Message>) -> Self {
        Element::new(prioritized)
    }
}
