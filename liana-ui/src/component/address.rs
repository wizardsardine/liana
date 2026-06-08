use iced::{
    advanced::{
        layout, mouse, renderer,
        widget::{tree, Tree, Widget},
        Clipboard, Layout, Shell,
    },
    Alignment, Element, Event, Length, Rectangle, Size,
};

use crate::{
    component::text::{self, p2_regular},
    theme,
    widget::{Element as LianaElement, Row},
};

const CHUNK_SIZE: usize = 4;
type TextStyle = fn(&theme::Theme) -> iced::widget::text::Style;

#[derive(Debug, Clone)]
struct AddressState {
    addr: String,
    plain: bool,
}

pub struct Address<'a, Message> {
    addr: String,
    size: Option<u32>,
    style: TextStyle,
    plain: Element<'a, Message, theme::Theme, iced::Renderer>,
    chunked: Element<'a, Message, theme::Theme, iced::Renderer>,
}

pub fn address<'a, Message: 'a>(addr: impl Into<String>) -> Address<'a, Message> {
    Address::new(addr)
}

impl<'a, Message: 'a> Address<'a, Message> {
    pub fn new(addr: impl Into<String>) -> Self {
        Self::with_size(addr, None)
    }

    pub fn small(self) -> Self {
        self.size(text::P1_SIZE)
    }

    pub fn size(mut self, size: u32) -> Self {
        self.size = Some(size);
        self.rebuild_children();
        self
    }

    pub fn style(mut self, style: TextStyle) -> Self {
        self.style = style;
        self.rebuild_children();
        self
    }

    fn with_size(addr: impl Into<String>, size: Option<u32>) -> Self {
        let addr = addr.into();

        Self {
            addr: addr.clone(),
            size,
            style: theme::text::default,
            plain: plain_address(&addr, size, theme::text::default),
            chunked: chunked_address(&addr, size, theme::text::default),
        }
    }

    fn rebuild_children(&mut self) {
        self.plain = plain_address(&self.addr, self.size, self.style);
        self.chunked = chunked_address(&self.addr, self.size, self.style);
    }
}

fn plain_address<'a, Message: 'a>(
    addr: &str,
    size: Option<u32>,
    style: TextStyle,
) -> Element<'a, Message, theme::Theme, iced::Renderer> {
    let mut text = p2_regular(addr.to_owned()).style(style);
    if let Some(size) = size {
        text = text.size(size);
    }
    text.into()
}

fn chunked_address<'a, Message: 'a>(
    addr: &str,
    size: Option<u32>,
    style: TextStyle,
) -> Element<'a, Message, theme::Theme, iced::Renderer> {
    addr.chars()
        .collect::<Vec<_>>()
        .chunks(CHUNK_SIZE)
        .enumerate()
        .fold(
            Row::new().align_y(Alignment::Center).spacing(5),
            |row, (i, chunk)| {
                let text = chunk.iter().collect::<String>();
                let style = if i % 2 == 0 {
                    style
                } else {
                    theme::text::card_secondary
                };

                let mut text = p2_regular(text).style(style);
                if let Some(size) = size {
                    text = text.size(size);
                }

                row.push(text)
            },
        )
        .width(Length::Shrink)
        .into()
}

impl<'a, Message: 'a> Widget<Message, theme::Theme, iced::Renderer> for Address<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<AddressState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(AddressState {
            addr: self.addr.clone(),
            plain: false,
        })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.plain), Tree::new(&self.chunked)]
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<AddressState>();
        if state.addr != self.addr {
            state.addr.clone_from(&self.addr);
            state.plain = false;
        }

        tree.diff_children(&[&self.plain, &self.chunked]);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_ref::<AddressState>();
        let child = if state.plain { 0 } else { 1 };

        self.child_mut(state.plain)
            .layout(&mut tree.children[child], renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            if cursor.is_over(layout.bounds()) {
                let state = tree.state.downcast_mut::<AddressState>();
                state.plain = !state.plain;
                shell.invalidate_layout();
                shell.request_redraw();
                shell.capture_event();
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &theme::Theme,
        defaults: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<AddressState>();
        let child = if state.plain { 0 } else { 1 };

        self.child(state.plain).draw(
            &tree.children[child],
            renderer,
            theme,
            defaults,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

impl<'a, Message> Address<'a, Message> {
    fn child(&self, plain: bool) -> &dyn Widget<Message, theme::Theme, iced::Renderer> {
        if plain {
            self.plain.as_widget()
        } else {
            self.chunked.as_widget()
        }
    }

    fn child_mut(&mut self, plain: bool) -> &mut dyn Widget<Message, theme::Theme, iced::Renderer> {
        if plain {
            self.plain.as_widget_mut()
        } else {
            self.chunked.as_widget_mut()
        }
    }
}

impl<'a, Message: 'a> From<Address<'a, Message>> for LianaElement<'a, Message> {
    fn from(address: Address<'a, Message>) -> Self {
        LianaElement::new(address)
    }
}
