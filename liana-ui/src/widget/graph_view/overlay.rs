//! Overlay wrapper drawing a child overlay under the graph camera.
use iced::{
    advanced::{
        layout::{self, Layout},
        overlay::{self, Overlay},
        renderer,
        widget::Operation,
        Clipboard, Renderer as _, Shell,
    },
    mouse, Event, Size, Transformation, Vector,
};

use crate::{theme::Theme, widget::Renderer};

pub struct Transformed<'a, M> {
    content: overlay::Element<'a, M, Theme, Renderer>,
    transformation: Transformation,
}

impl<'a, M> Transformed<'a, M> {
    pub fn new(
        content: overlay::Element<'a, M, Theme, Renderer>,
        transformation: Transformation,
    ) -> Self {
        Self {
            content,
            transformation,
        }
    }
}

impl<M> Overlay<M, Theme, Renderer> for Transformed<'_, M> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let node = self.content.as_overlay_mut().layout(renderer, bounds);
        let screen = node.bounds() * self.transformation;
        layout::Node::with_children(
            screen.size(),
            vec![node.translate(Vector::new(-screen.x, -screen.y))],
        )
        .move_to(screen.position())
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        let Some(inner) = layout.children().next() else {
            return;
        };
        let inverse = self.transformation.inverse();
        renderer.with_transformation(self.transformation, |renderer| {
            self.content
                .as_overlay()
                .draw(renderer, theme, style, inner, cursor * inverse);
        });
    }

    fn operate(&mut self, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        if let Some(inner) = layout.children().next() {
            self.content
                .as_overlay_mut()
                .operate(inner, renderer, operation);
        }
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
    ) {
        let Some(inner) = layout.children().next() else {
            return;
        };
        let inverse = self.transformation.inverse();
        self.content.as_overlay_mut().update(
            event,
            inner,
            cursor * inverse,
            renderer,
            clipboard,
            shell,
        );
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let Some(inner) = layout.children().next() else {
            return mouse::Interaction::None;
        };
        let inverse = self.transformation.inverse();
        self.content
            .as_overlay()
            .mouse_interaction(inner, cursor * inverse, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        layout: Layout<'b>,
        renderer: &Renderer,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        let inner = layout.children().next()?;
        let transformation = self.transformation;
        self.content
            .as_overlay_mut()
            .overlay(inner, renderer)
            .map(|nested| overlay::Element::new(Box::new(Transformed::new(nested, transformation))))
    }

    fn index(&self) -> f32 {
        self.content.as_overlay().index()
    }
}
