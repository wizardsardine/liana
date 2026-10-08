use iced::advanced::{
    layout::{self, Layout},
    overlay, renderer,
    widget::{self, Tree, Widget},
    Clipboard, Shell,
};
use iced::{mouse, Border, Color, Event, Length, Rectangle, Size, Vector};

use crate::{
    theme::Theme,
    widget::{Element, Renderer},
};

type OutlineStyle<'a> = Box<dyn Fn(&Theme) -> Border + 'a>;

/// Draws borders over its child, inside or outside its bounds. A `Container` border is
/// drawn under its content, and iced has no outline offset.
pub struct Outline<'a, Message> {
    content: Element<'a, Message>,
    outlines: Vec<(f32, OutlineStyle<'a>)>,
}

impl<'a, Message> Outline<'a, Message> {
    pub fn new(content: impl Into<Element<'a, Message>>) -> Self {
        Self {
            content: content.into(),
            outlines: Vec::new(),
        }
    }

    /// Draws `border` over the content, `offset` px outside its bounds (inside when negative).
    pub fn outline(mut self, offset: f32, border: impl Fn(&Theme) -> Border + 'a) -> Self {
        self.outlines.push((offset, Box::new(border)));
        self
    }
}

impl<Message> Widget<Message, Theme, Renderer> for Outline<'_, Message> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[&self.content]);
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
        operation: &mut dyn widget::Operation,
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
        for (offset, border) in &self.outlines {
            renderer::Renderer::fill_quad(
                renderer,
                renderer::Quad {
                    bounds: layout.bounds().expand(*offset),
                    border: border(theme),
                    ..Default::default()
                },
                Color::TRANSPARENT,
            );
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
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message: 'a> From<Outline<'a, Message>> for Element<'a, Message> {
    fn from(outline: Outline<'a, Message>) -> Self {
        Element::new(outline)
    }
}
