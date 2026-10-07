//! A tooltip whose content receives events, so it can hold a link or a button.
//!
//! `iced::widget::Tooltip` draws its overlay and nothing else: the overlay has
//! no `update`, and the tooltip closes as soon as the cursor leaves the anchor,
//! so content inside it can never be reached. This widget keeps the tooltip open
//! while the cursor is over either the anchor or the tooltip, and forwards
//! events to the tooltip content.

use iced::{
    advanced::{
        layout, mouse, overlay, renderer,
        widget::{tree, Operation, Tree, Widget},
        Clipboard, Layout, Shell,
    },
    Element, Event, Length, Point, Rectangle, Size, Vector,
};

/// Space between the anchor and the tooltip. Kept at zero so the cursor can
/// travel from one to the other without crossing a gap that would close it.
const GAP: f32 = 0.0;

pub struct HoverTooltip<'a, Message, Theme, Renderer> {
    anchor: Element<'a, Message, Theme, Renderer>,
    tooltip: Element<'a, Message, Theme, Renderer>,
}

impl<'a, Message, Theme, Renderer> HoverTooltip<'a, Message, Theme, Renderer> {
    /// `tooltip` is shown above `anchor` on hover. Style it yourself, e.g. by
    /// wrapping it in a `Container` with `theme::card::simple`.
    pub fn new(
        anchor: impl Into<Element<'a, Message, Theme, Renderer>>,
        tooltip: impl Into<Element<'a, Message, Theme, Renderer>>,
    ) -> Self {
        Self {
            anchor: anchor.into(),
            tooltip: tooltip.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct State {
    open: bool,
    /// Where the overlay was last laid out, so both the widget and the overlay
    /// can tell whether the cursor is still within reach of it.
    tooltip_bounds: Rectangle,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for HoverTooltip<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.anchor), Tree::new(&self.tooltip)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[&self.anchor, &self.tooltip]);
    }

    fn size(&self) -> Size<Length> {
        self.anchor.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.anchor
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
        self.anchor.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );

        // Only opening is decided here. Closing is the overlay's job: once it is
        // up, the runtime hands this widget an unavailable cursor whenever the
        // overlay claims a mouse interaction, which a hovered link does.
        if matches!(event, Event::Mouse(_)) {
            let state = tree.state.downcast_mut::<State>();
            if !state.open && cursor.is_over(layout.bounds()) {
                state.open = true;
                shell.invalidate_layout();
                shell.request_redraw();
            }
        }
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
        self.anchor.as_widget().draw(
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
        self.anchor.as_widget().mouse_interaction(
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
        self.anchor
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
        let Tree {
            state: tree_state,
            children,
            ..
        } = tree;
        let (anchor_tree, tooltip_tree) = children.split_at_mut(1);
        let state = tree_state.downcast_mut::<State>();

        if !state.open {
            return self.anchor.as_widget_mut().overlay(
                &mut anchor_tree[0],
                layout,
                renderer,
                viewport,
                translation,
            );
        }

        Some(overlay::Element::new(Box::new(Overlay {
            anchor_bounds: layout.bounds() + translation,
            tooltip: &mut self.tooltip,
            tree: &mut tooltip_tree[0],
            state,
        })))
    }
}

struct Overlay<'a, 'b, Message, Theme, Renderer> {
    anchor_bounds: Rectangle,
    tooltip: &'b mut Element<'a, Message, Theme, Renderer>,
    tree: &'b mut Tree,
    state: &'b mut State,
}

impl<Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for Overlay<'_, '_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let viewport = Rectangle::with_size(bounds);
        let limits = layout::Limits::new(Size::ZERO, bounds);
        let node = self
            .tooltip
            .as_widget_mut()
            .layout(self.tree, renderer, &limits);
        let size = node.size();

        // Centred above the anchor, then nudged back inside the viewport.
        let mut position = Point::new(
            self.anchor_bounds.center_x() - size.width / 2.0,
            self.anchor_bounds.y - size.height - GAP,
        );
        position.x = position.x.clamp(
            viewport.x,
            (viewport.x + viewport.width - size.width).max(viewport.x),
        );
        if position.y < viewport.y {
            // No room above: drop it below the anchor instead.
            position.y = self.anchor_bounds.y + self.anchor_bounds.height + GAP;
        }

        self.state.tooltip_bounds = Rectangle::new(position, size);

        layout::Node::with_children(size, vec![node]).move_to(position)
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        self.tooltip.as_widget_mut().update(
            self.tree,
            event,
            layout.children().next().unwrap(),
            cursor,
            renderer,
            clipboard,
            shell,
            &layout.bounds(),
        );

        if matches!(event, Event::Mouse(_)) {
            let within_reach =
                cursor.is_over(self.anchor_bounds) || cursor.is_over(layout.bounds());
            if !within_reach {
                self.state.open = false;
                shell.invalidate_layout();
                shell.request_redraw();
            }
        }
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.tooltip.as_widget().draw(
            self.tree,
            renderer,
            theme,
            style,
            layout.children().next().unwrap(),
            cursor,
            &layout.bounds(),
        );
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.tooltip.as_widget().mouse_interaction(
            self.tree,
            layout.children().next().unwrap(),
            cursor,
            &layout.bounds(),
            renderer,
        )
    }

    fn operate(&mut self, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.tooltip.as_widget_mut().operate(
            self.tree,
            layout.children().next().unwrap(),
            renderer,
            operation,
        );
    }
}

impl<'a, Message, Theme, Renderer> From<HoverTooltip<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(widget: HoverTooltip<'a, Message, Theme, Renderer>) -> Self {
        Element::new(widget)
    }
}
