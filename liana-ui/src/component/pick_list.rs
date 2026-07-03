use std::borrow::Borrow;

use crate::{theme, widget::*};
use iced::{
    advanced::{
        layout, overlay,
        widget::{Operation, Tree, Widget},
        Clipboard, Layout, Shell,
    },
    mouse, Event, Length, Padding, Pixels, Point, Rectangle, Size, Vector,
};

const FIELD_PADDING: Padding = Padding {
    top: 12.0,
    right: 16.0,
    bottom: 12.0,
    left: 16.0,
};
const FIELD_TEXT_SIZE: Pixels = Pixels(16.0);

pub fn pick_list<'a, T, L, V, Message>(
    options: L,
    selected: Option<V>,
    on_selected: impl Fn(T) -> Message + 'a,
) -> PickList<'a, T, L, V, Message>
where
    T: ToString + PartialEq + Clone + 'a,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    Message: Clone,
{
    PickList::new(options, selected, on_selected)
        .style(theme::pick_list::primary)
        .menu_style(theme::pick_list::menu)
}

pub fn field_pick_list<'a, T, L, V, Message>(
    options: L,
    selected: Option<V>,
    on_selected: impl Fn(T) -> Message + 'a,
) -> Element<'a, Message>
where
    T: ToString + PartialEq + Clone + 'a,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    Message: Clone + 'a,
{
    DownwardMenu::new(
        pick_list(options, selected, on_selected)
            .width(Length::Fill)
            .padding(FIELD_PADDING)
            .text_size(FIELD_TEXT_SIZE)
            .into(),
    )
    .into()
}

struct DownwardMenu<'a, Message> {
    content: Element<'a, Message>,
}

impl<'a, Message> DownwardMenu<'a, Message> {
    fn new(content: Element<'a, Message>) -> Self {
        Self { content }
    }
}

impl<Message> Widget<Message, theme::Theme, Renderer> for DownwardMenu<'_, Message>
where
    Message: Clone,
{
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
        theme: &theme::Theme,
        style: &iced::advanced::renderer::Style,
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

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, theme::Theme, Renderer>> {
        let overlay = self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )?;
        let target_bottom = layout.position().y + translation.y + layout.bounds().height;

        Some(overlay::Element::new(Box::new(DownwardOverlay {
            overlay,
            target_bottom,
        })))
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

impl<'a, Message> From<DownwardMenu<'a, Message>> for Element<'a, Message>
where
    Message: Clone + 'a,
{
    fn from(menu: DownwardMenu<'a, Message>) -> Self {
        Element::new(menu)
    }
}

struct DownwardOverlay<'a, Message> {
    overlay: overlay::Element<'a, Message, theme::Theme, Renderer>,
    target_bottom: f32,
}

impl<Message> overlay::Overlay<Message, theme::Theme, Renderer> for DownwardOverlay<'_, Message> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let node = self.overlay.as_overlay_mut().layout(renderer, bounds);
        if node.bounds().y < self.target_bottom {
            let x = node.bounds().x;
            node.move_to(Point::new(x, self.target_bottom))
        } else {
            node
        }
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
        self.overlay
            .as_overlay_mut()
            .update(event, layout, cursor, renderer, clipboard, shell);
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &theme::Theme,
        style: &iced::advanced::renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.overlay
            .as_overlay()
            .draw(renderer, theme, style, layout, cursor);
    }

    fn operate(&mut self, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.overlay
            .as_overlay_mut()
            .operate(layout, renderer, operation);
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.overlay
            .as_overlay()
            .mouse_interaction(layout, cursor, renderer)
    }

    fn overlay<'a>(
        &'a mut self,
        layout: Layout<'a>,
        renderer: &Renderer,
    ) -> Option<overlay::Element<'a, Message, theme::Theme, Renderer>> {
        self.overlay.as_overlay_mut().overlay(layout, renderer)
    }

    fn index(&self) -> f32 {
        self.overlay.as_overlay().index()
    }
}
