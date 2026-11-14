//! Content that stands in the layout flow but takes no room in it.
//!
//! [`Floating`] lays its content out at its own position with the width it is
//! given, reports zero height to its parent and paints the content as an
//! overlay, above the siblings that follow it. A disclosure such as a
//! dropdown menu then covers the rows below it instead of pushing them down.
//!
//! Focus operations still reach the content in place, so Tab moves from the
//! control that opened it straight into it. Events and drawing go through
//! the overlay only, so nothing is handled twice.
//!
//! A popover anchored under its control narrows to its own width
//! ([`Floating::align_x`]), keeps a [`Floating::gap`] from the control and
//! closes on a press outside it or Escape ([`Floating::on_dismiss`]).

use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::tree::Tree;
use iced::advanced::widget::{Operation, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::alignment::Horizontal;
use iced::keyboard::{self, key};
use iced::{mouse, Event, Length, Point, Rectangle, Size, Vector};

use crate::theme::Theme;
use crate::widget::{Element, Renderer};

pub struct Floating<'a, Message> {
    content: Element<'a, Message>,
    align_x: Option<Horizontal>,
    gap: f32,
    on_dismiss: Option<Message>,
    on_escape: Option<Message>,
    fit_to_window: bool,
    fit_height: Option<f32>,
    dismiss_on_escape: bool,
}

impl<'a, Message> Floating<'a, Message> {
    pub fn new(content: impl Into<Element<'a, Message>>) -> Self {
        Self {
            content: content.into(),
            align_x: None,
            gap: 0.0,
            on_dismiss: None,
            on_escape: None,
            fit_to_window: false,
            fit_height: None,
            dismiss_on_escape: false,
        }
    }

    /// Lays the content out at its own width, placed across the width it is
    /// given, instead of stretched across it.
    pub fn align_x(self, align_x: Horizontal) -> Self {
        Self {
            align_x: Some(align_x),
            ..self
        }
    }

    /// The room left between the preceding row and the content.
    pub fn gap(self, gap: f32) -> Self {
        Self { gap, ..self }
    }

    /// Sets the message produced by a press outside the content (which goes
    /// no further) or by an Escape the content leaves unhandled.
    pub fn on_dismiss(self, on_dismiss: Message) -> Self {
        Self {
            on_dismiss: Some(on_dismiss),
            ..self
        }
    }

    /// Sets the message produced by an Escape the content leaves unhandled,
    /// without making a press outside the content a dismissal. Ignored when
    /// [`Self::on_dismiss`] is set, which already answers Escape.
    pub fn on_escape(self, on_escape: Message) -> Self {
        Self {
            on_escape: Some(on_escape),
            ..self
        }
    }

    /// Keep an anchored editor inside the window, including after scrolling.
    pub fn fit_to_window(self) -> Self {
        Self {
            fit_to_window: true,
            ..self
        }
    }

    /// Caps the content's height at the room left below it in the window
    /// (never under `min`), so a long menu with a `Shrink` scrollable scrolls
    /// on a short window instead of running off its bottom edge.
    pub fn fit_height_to_window(self, min: f32) -> Self {
        Self {
            fit_height: Some(min),
            ..self
        }
    }

    /// Dismiss on the first Escape, even while an editor has input focus.
    pub fn dismiss_on_escape(self) -> Self {
        Self {
            dismiss_on_escape: true,
            ..self
        }
    }
}

fn content_limits(width: f32) -> layout::Limits {
    content_limits_within(width, f32::INFINITY)
}

fn content_limits_within(width: f32, height: f32) -> layout::Limits {
    layout::Limits::new(Size::ZERO, Size::new(width, height))
}

/// The margin kept between fitted content and the window's edges.
const WINDOW_MARGIN: f32 = 8.0;

/// Where content `height` tall, anchored at `top`, starts so its bottom stays
/// inside a window `window_height` tall (never above the top margin).
fn fitted_top(window_height: f32, top: f32, height: f32) -> f32 {
    top.min(window_height - height - WINDOW_MARGIN)
        .max(WINDOW_MARGIN)
}

/// The height content anchored at `top` may take in a window `window_height`
/// tall: what is left below it, but never less than `min`.
fn fitted_height(window_height: f32, top: f32, min: f32) -> f32 {
    (window_height - top - WINDOW_MARGIN).max(min)
}

/// Where the content sits inside the `width` it is given.
fn content_offset(align_x: Option<Horizontal>, gap: f32, width: f32, content: Size) -> Vector {
    let x = match align_x {
        None | Some(Horizontal::Left) => 0.0,
        Some(Horizontal::Center) => ((width - content.width) / 2.0).max(0.0),
        Some(Horizontal::Right) => (width - content.width).max(0.0),
    };
    Vector::new(x, gap)
}

impl<Message: Clone> Widget<Message, Theme, Renderer> for Floating<'_, Message> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[&self.content]);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let width = limits.max().width;
        let content = self.content.as_widget_mut().layout(
            &mut tree.children[0],
            renderer,
            &content_limits(width),
        );
        let offset = content_offset(self.align_x, self.gap, width, content.size());
        layout::Node::with_children(Size::new(width, 0.0), vec![content.translate(offset)])
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        if let Some(content) = layout.children().next() {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                content,
                renderer,
                operation,
            );
        }
    }

    fn update(
        &mut self,
        _tree: &mut Tree,
        _event: &Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        _shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
    }

    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        _renderer: &Renderer,
        _viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let bounds = layout.children().next()?.bounds();
        Some(overlay::Element::new(Box::new(Overlay {
            content: &mut self.content,
            tree: &mut tree.children[0],
            position: bounds.position() + translation,
            width: bounds.width,
            on_dismiss: self.on_dismiss.as_ref(),
            on_escape: self.on_escape.as_ref(),
            fit_to_window: self.fit_to_window,
            fit_height: self.fit_height,
            dismiss_on_escape: self.dismiss_on_escape,
        })))
    }
}

struct Overlay<'a, 'b, Message> {
    content: &'b mut Element<'a, Message>,
    tree: &'b mut Tree,
    position: Point,
    width: f32,
    on_dismiss: Option<&'b Message>,
    on_escape: Option<&'b Message>,
    fit_to_window: bool,
    fit_height: Option<f32>,
    dismiss_on_escape: bool,
}

impl<Message: Clone> overlay::Overlay<Message, Theme, Renderer> for Overlay<'_, '_, Message> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let limits = match self.fit_height {
            Some(min) => content_limits_within(
                self.width,
                fitted_height(bounds.height, self.position.y, min),
            ),
            None => content_limits(self.width),
        };
        let content = self
            .content
            .as_widget_mut()
            .layout(self.tree, renderer, &limits);
        let position = if self.fit_to_window {
            Point::new(
                self.position
                    .x
                    .clamp(8.0, (bounds.width - content.size().width - 8.0).max(8.0)),
                self.position
                    .y
                    .clamp(8.0, (bounds.height - content.size().height - 8.0).max(8.0)),
            )
        } else if self.fit_height.is_some() {
            // Near the window's bottom edge the minimum height is taller than
            // the room below, so slide the content up to keep it on screen.
            Point::new(
                self.position.x,
                fitted_top(bounds.height, self.position.y, content.size().height),
            )
        } else {
            self.position
        };
        content.move_to(position)
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
        if let Some(message) = self.on_dismiss {
            if self.dismiss_on_escape
                && matches!(
                    event,
                    Event::Keyboard(keyboard::Event::KeyPressed {
                        key: keyboard::Key::Named(key::Named::Escape),
                        repeat: false,
                        ..
                    })
                )
            {
                shell.publish(message.clone());
                shell.capture_event();
                return;
            }
            if let Event::Mouse(mouse::Event::ButtonPressed(_)) = event {
                if !cursor.is_over(layout.bounds()) {
                    shell.publish(message.clone());
                    shell.capture_event();
                    return;
                }
            }
        }
        self.content.as_widget_mut().update(
            self.tree,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            &layout.bounds(),
        );
        // Focused content (a text field, say) gets the first refusal.
        if let Some(message) = self.on_dismiss.or(self.on_escape) {
            if !shell.is_event_captured()
                && matches!(
                    event,
                    Event::Keyboard(keyboard::Event::KeyPressed {
                        key: keyboard::Key::Named(key::Named::Escape),
                        repeat: false,
                        ..
                    })
                )
            {
                shell.publish(message.clone());
                shell.capture_event();
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
        // The focus ring paints 4px outside a control, so the viewport
        // leaves room for it around the content.
        let viewport = layout.bounds().expand(8.0);
        self.content
            .as_widget()
            .draw(self.tree, renderer, theme, style, layout, cursor, &viewport);
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let interaction = self.content.as_widget().mouse_interaction(
            self.tree,
            layout,
            cursor,
            &layout.bounds(),
            renderer,
        );
        // The runtime keeps the cursor from the widgets underneath only where
        // an overlay reports an interaction. Claim the whole content, so a
        // click on its padding never reaches the row it covers.
        if interaction == mouse::Interaction::None && cursor.is_over(layout.bounds()) {
            mouse::Interaction::Idle
        } else {
            interaction
        }
    }

    fn overlay<'c>(
        &'c mut self,
        layout: Layout<'c>,
        renderer: &Renderer,
    ) -> Option<overlay::Element<'c, Message, Theme, Renderer>> {
        let viewport = layout.bounds();
        self.content
            .as_widget_mut()
            .overlay(self.tree, layout, renderer, &viewport, Vector::ZERO)
    }
}

impl<'a, Message: Clone + 'a> From<Floating<'a, Message>> for Element<'a, Message> {
    fn from(floating: Floating<'a, Message>) -> Self {
        Self::new(floating)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitted_height_takes_the_room_below_but_never_less_than_min() {
        assert_eq!(fitted_height(900.0, 300.0, 220.0), 592.0);
        assert_eq!(fitted_height(500.0, 300.0, 220.0), 220.0);
    }

    #[test]
    fn fitted_top_slides_up_only_when_the_content_would_run_off_the_bottom() {
        // Room below: stays where it is anchored.
        assert_eq!(fitted_top(900.0, 300.0, 400.0), 300.0);
        // Anchored 100px from the bottom with the 220px floor: slides up.
        assert_eq!(fitted_top(900.0, 800.0, 220.0), 672.0);
        // Never above the top margin.
        assert_eq!(fitted_top(200.0, 150.0, 220.0), 8.0);
    }
}
