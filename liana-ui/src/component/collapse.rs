use iced::{
    advanced::{
        layout, mouse, overlay, renderer,
        widget::{tree, Operation, Tree, Widget},
        Clipboard, Layout, Shell,
    },
    widget::button,
    Element, Event, Length, Padding, Rectangle, Size, Vector,
};

use crate::theme::Theme;

type Renderer = iced::Renderer;

#[derive(Debug, Clone, Copy)]
struct CollapseState {
    expanded: bool,
}

#[allow(clippy::type_complexity)]
pub struct Collapse<'a, Message> {
    before: Element<'a, Message, Theme, Renderer>,
    after: Element<'a, Message, Theme, Renderer>,
    content: Element<'a, Message, Theme, Renderer>,
    init_expanded: bool,
    padding: Padding,
    width: Length,
    header_style: Box<dyn Fn(&Theme, button::Status) -> button::Style + 'a>,
}

impl<'a, Message: 'a> Collapse<'a, Message> {
    pub fn new(
        before: impl Into<Element<'a, Message, Theme, Renderer>>,
        after: impl Into<Element<'a, Message, Theme, Renderer>>,
        content: impl Into<Element<'a, Message, Theme, Renderer>>,
    ) -> Self {
        Self {
            before: before.into(),
            after: after.into(),
            content: content.into(),
            init_expanded: false,
            padding: Padding::ZERO,
            width: Length::Fill,
            header_style: Box::new(crate::theme::button::transparent_border),
        }
    }

    pub fn collapsed(mut self, state: bool) -> Self {
        self.init_expanded = state;
        self
    }

    pub fn padding<P: Into<Padding>>(mut self, padding: P) -> Self {
        self.padding = padding.into();
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn style(mut self, style: impl Fn(&Theme, button::Status) -> button::Style + 'a) -> Self {
        self.header_style = Box::new(style);
        self
    }
}

impl<'a, Message: 'a> Widget<Message, Theme, Renderer> for Collapse<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<CollapseState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(CollapseState {
            expanded: self.init_expanded,
        })
    }

    fn children(&self) -> Vec<Tree> {
        vec![
            Tree::new(&self.before),
            Tree::new(&self.after),
            Tree::new(&self.content),
        ]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[&self.before, &self.after, &self.content]);
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_ref::<CollapseState>();
        let expanded = state.expanded;
        let padding = self.padding;

        let limits = limits.width(self.width);

        // Layout the header with padding.
        let header_limits = limits.shrink(padding);
        let (header_content_node, header_child_idx) = if expanded {
            (
                self.after
                    .as_widget_mut()
                    .layout(&mut tree.children[1], renderer, &header_limits),
                1,
            )
        } else {
            (
                self.before
                    .as_widget_mut()
                    .layout(&mut tree.children[0], renderer, &header_limits),
                0,
            )
        };
        let _ = header_child_idx;

        let header_content_size = header_content_node.size();
        let header_width = (header_content_size.width + padding.x()).min(limits.max().width);
        let header_height = header_content_size.height + padding.y();
        let header_node = layout::Node::with_children(
            Size::new(header_width, header_height),
            vec![header_content_node.move_to(iced::Point::new(padding.left, padding.top))],
        );

        if expanded {
            // Layout content below header.
            let content_node =
                self.content
                    .as_widget_mut()
                    .layout(&mut tree.children[2], renderer, &limits);
            let content_height = content_node.size().height;
            let content_width = content_node.size().width;
            let total_height = header_height + content_height;
            let total_width = header_width.max(content_width).min(limits.max().width);

            layout::Node::with_children(
                Size::new(total_width, total_height),
                vec![
                    header_node,
                    content_node.move_to(iced::Point::new(0.0, header_height)),
                ],
            )
        } else {
            layout::Node::with_children(Size::new(header_width, header_height), vec![header_node])
        }
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
        let state = tree.state.downcast_mut::<CollapseState>();
        let mut children_layouts = layout.children();
        let header_layout = children_layouts.next().unwrap();
        let header_bounds = header_layout.bounds();

        // Handle click on header to toggle.
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            if cursor.is_over(header_bounds) {
                state.expanded = !state.expanded;
                shell.invalidate_layout();
                shell.request_redraw();
                shell.capture_event();
                return;
            }
        }

        // Forward events to content when expanded.
        if state.expanded {
            if let Some(content_layout) = children_layouts.next() {
                self.content.as_widget_mut().update(
                    &mut tree.children[2],
                    event,
                    content_layout,
                    cursor,
                    renderer,
                    clipboard,
                    shell,
                    viewport,
                );
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        defaults: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<CollapseState>();
        let mut children_layouts = layout.children();
        let header_layout = children_layouts.next().unwrap();
        let header_bounds = header_layout.bounds();

        // Draw header background with button-like styling.
        let status = if cursor.is_over(header_bounds) {
            button::Status::Hovered
        } else {
            button::Status::Active
        };
        let style = (self.header_style)(theme, status);

        if let Some(background) = style.background {
            renderer::Renderer::fill_quad(
                renderer,
                renderer::Quad {
                    bounds: header_bounds,
                    border: style.border,
                    shadow: style.shadow,
                    snap: style.snap,
                },
                background,
            );
        }

        // Draw header content (inside padding).
        let header_content_layout = header_layout.children().next().unwrap();
        if state.expanded {
            self.after.as_widget().draw(
                &tree.children[1],
                renderer,
                theme,
                defaults,
                header_content_layout,
                cursor,
                viewport,
            );

            // Draw content.
            if let Some(content_layout) = children_layouts.next() {
                self.content.as_widget().draw(
                    &tree.children[2],
                    renderer,
                    theme,
                    defaults,
                    content_layout,
                    cursor,
                    viewport,
                );
            }
        } else {
            self.before.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                defaults,
                header_content_layout,
                cursor,
                viewport,
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
        let state = tree.state.downcast_ref::<CollapseState>();
        let mut children_layouts = layout.children();
        let header_layout = children_layouts.next().unwrap();

        if cursor.is_over(header_layout.bounds()) {
            return mouse::Interaction::Pointer;
        }

        if state.expanded {
            if let Some(content_layout) = children_layouts.next() {
                return self.content.as_widget().mouse_interaction(
                    &tree.children[2],
                    content_layout,
                    cursor,
                    viewport,
                    renderer,
                );
            }
        }

        mouse::Interaction::default()
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_ref::<CollapseState>();

        if state.expanded {
            let mut children_layouts = layout.children();
            let _header_layout = children_layouts.next();
            if let Some(content_layout) = children_layouts.next() {
                self.content.as_widget_mut().operate(
                    &mut tree.children[2],
                    content_layout,
                    renderer,
                    operation,
                );
            }
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let state = tree.state.downcast_ref::<CollapseState>();

        if state.expanded {
            let mut children_layouts = layout.children();
            let _header_layout = children_layouts.next();
            if let Some(content_layout) = children_layouts.next() {
                return self.content.as_widget_mut().overlay(
                    &mut tree.children[2],
                    content_layout,
                    renderer,
                    viewport,
                    translation,
                );
            }
        }

        None
    }
}

impl<'a, Message: 'a> From<Collapse<'a, Message>> for Element<'a, Message, Theme, Renderer> {
    fn from(collapse: Collapse<'a, Message>) -> Self {
        Element::new(collapse)
    }
}
