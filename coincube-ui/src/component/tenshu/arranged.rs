//! A container that places its children with a layout function and handles
//! everything else the same way for every child.
//!
//! Custom layouts (two columns that stack, a row whose tail drops below) only
//! differ in where children go. The tree, events, focus, cursor, drawing and
//! overlays are the same plumbing each time, so they live here once: a new
//! layout supplies [`Arrange`] and nothing else.
//!
//! The cursor is the highest interaction any child reports, and every child
//! is drawn, as in iced's `Column`.

use iced::advanced::{
    layout, mouse, overlay, renderer,
    widget::{Operation, Tree},
    Clipboard, Layout, Shell, Widget,
};
use iced::{Event, Length, Rectangle, Size, Vector};

use crate::theme::Theme;
use crate::widget::{Element, Renderer};

/// Lays out `children`, each with its own state in the matching `trees`
/// entry, inside `limits`. The returned node holds one child node per child,
/// in order.
pub type Arrange<'a, M> = Box<
    dyn FnMut(&mut [Element<'a, M>], &mut [Tree], &Renderer, &layout::Limits) -> layout::Node + 'a,
>;

/// `children` in [`Arranged`], sized `size` and placed by `arrange`.
pub fn arranged<'a, M>(
    children: Vec<Element<'a, M>>,
    size: Size<Length>,
    arrange: impl FnMut(&mut [Element<'a, M>], &mut [Tree], &Renderer, &layout::Limits) -> layout::Node
        + 'a,
) -> Arranged<'a, M> {
    Arranged {
        children,
        size,
        arrange: Box::new(arrange),
        stop_on_capture: false,
        skip_empty: false,
    }
}

pub struct Arranged<'a, M> {
    /// Also the focus and event order.
    children: Vec<Element<'a, M>>,
    size: Size<Length>,
    arrange: Arrange<'a, M>,
    stop_on_capture: bool,
    skip_empty: bool,
}

impl<M> Arranged<'_, M> {
    /// Stop passing an event on once a child captures it, so a later child
    /// never sees it. By default every child sees every event, as in iced's
    /// `Column` and `Row`.
    pub fn stop_on_capture(mut self) -> Self {
        self.stop_on_capture = true;
        self
    }

    /// Treat a child the layout gave no area as absent: it isn't drawn, sees
    /// no events and reports no text or focus. A layout that picks between
    /// alternatives (a wide and a narrow variant) gives the unused ones
    /// `layout::Node::new(Size::ZERO)`.
    pub fn skip_empty(mut self) -> Self {
        self.skip_empty = true;
        self
    }
}

/// A child laid out with no area, which [`Arranged::skip_empty`] leaves out.
fn empty(layout: &Layout<'_>) -> bool {
    let size = layout.bounds().size();
    size.width <= 0.0 || size.height <= 0.0
}

impl<'a, M> Widget<M, Theme, Renderer> for Arranged<'a, M> {
    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
    }

    fn size(&self) -> Size<Length> {
        self.size
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        (self.arrange)(&mut self.children, &mut tree.children, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let skip_empty = self.skip_empty;
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            for ((child, state), layout) in self
                .children
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
            {
                if skip_empty && empty(&layout) {
                    continue;
                }
                child
                    .as_widget_mut()
                    .operate(state, layout, renderer, operation);
            }
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
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let skip_empty = self.skip_empty;
        for ((child, state), layout) in self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            if skip_empty && empty(&layout) {
                continue;
            }
            child.as_widget_mut().update(
                state, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
            if self.stop_on_capture && shell.is_event_captured() {
                break;
            }
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
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .filter(|(_, layout)| !(self.skip_empty && empty(layout)))
            .map(|((child, state), layout)| {
                child
                    .as_widget()
                    .mouse_interaction(state, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
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
        for ((child, state), layout) in self
            .children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .filter(|(_, layout)| !(self.skip_empty && empty(layout)))
        {
            child
                .as_widget()
                .draw(state, renderer, theme, style, layout, cursor, viewport);
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        overlay::from_children(
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, M: 'a> From<Arranged<'a, M>> for Element<'a, M> {
    fn from(arranged: Arranged<'a, M>) -> Self {
        Element::new(arranged)
    }
}
