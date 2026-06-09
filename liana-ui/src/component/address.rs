use std::fmt::Display;

use iced::{
    advanced::{
        layout, mouse, renderer,
        widget::{tree, Tree, Widget},
        Clipboard, Layout, Shell,
    },
    widget::row,
    Alignment, Element, Event, Length, Rectangle, Size,
};

use crate::{
    component::{button::btn_copy, text::new},
    theme,
    widget::{Element as LianaElement, Row},
};

pub fn copyable_address<'a, M: Clone + 'a>(
    address: impl Display,
    clipboard: M,
) -> LianaElement<'a, M> {
    let addr = new::caption(address).style(theme::text::card_secondary);
    let cpy = btn_copy(Some(clipboard));
    row![addr, cpy]
        .align_y(Alignment::Center)
        .spacing(12)
        .wrap()
        .into()
}

/// Renderings the address cycles through on each click; `None` is a single run.
/// Signing devices vary: some group the address by 4 characters, some by 6.
const CHUNK_SIZES: [Option<usize>; 3] = [Some(4), Some(6), None];

#[derive(Debug, Clone)]
struct AddressState {
    addr: String,
    /// Index into `CHUNK_SIZES` of the currently displayed rendering.
    mode: usize,
}

pub struct Address<'a, Message> {
    addr: String,
    size: Option<u32>,
    children: Vec<Element<'a, Message, theme::Theme, iced::Renderer>>,
}

/// Address display; clicking cycles through 4-char chunks, 6-char chunks, and a single run.
pub fn address<'a, Message: 'a>(addr: impl Into<String>) -> Address<'a, Message> {
    Address::new(addr)
}

impl<'a, Message: 'a> Address<'a, Message> {
    pub fn new(addr: impl Into<String>) -> Self {
        Self::with_size(addr, None)
    }

    pub fn size(mut self, size: u32) -> Self {
        self.size = Some(size);
        self.rebuild_children();
        self
    }

    fn with_size(addr: impl Into<String>, size: Option<u32>) -> Self {
        let addr = addr.into();
        let children = render_modes(&addr, size);
        Self {
            addr,
            size,
            children,
        }
    }

    fn rebuild_children(&mut self) {
        self.children = render_modes(&self.addr, self.size);
    }
}

fn render_modes<'a, Message: 'a>(
    addr: &str,
    size: Option<u32>,
) -> Vec<Element<'a, Message, theme::Theme, iced::Renderer>> {
    CHUNK_SIZES
        .iter()
        .map(|&chunk| match chunk {
            Some(n) => chunked_address(addr, size, n),
            None => plain_address(addr, size),
        })
        .collect()
}

fn plain_address<'a, Message: 'a>(
    addr: &str,
    size: Option<u32>,
) -> Element<'a, Message, theme::Theme, iced::Renderer> {
    let mut text = new::caption(addr.to_owned()).style(theme::text::address);
    if let Some(size) = size {
        text = text.size(size);
    }
    text.into()
}

fn chunked_address<'a, Message: 'a>(
    addr: &str,
    size: Option<u32>,
    chunk_size: usize,
) -> Element<'a, Message, theme::Theme, iced::Renderer> {
    addr.chars()
        .collect::<Vec<_>>()
        .chunks(chunk_size)
        .enumerate()
        .fold(
            Row::new().align_y(Alignment::Center).spacing(5),
            |row, (i, chunk)| {
                let text = chunk.iter().collect::<String>();
                let style = if i % 2 == 0 {
                    theme::text::address
                } else {
                    theme::text::address_dimmed
                };

                let mut text = new::caption(text).style(style);
                if let Some(size) = size {
                    text = text.size(size);
                }

                row.push(text)
            },
        )
        .width(Length::Shrink)
        .wrap()
        .into()
}

impl<'a, Message: 'a> Widget<Message, theme::Theme, iced::Renderer> for Address<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<AddressState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(AddressState {
            addr: self.addr.clone(),
            mode: 0,
        })
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(|c| Tree::new(c)).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<AddressState>();
        if state.addr != self.addr {
            state.addr.clone_from(&self.addr);
            state.mode = 0;
        }

        tree.diff_children(&self.children);
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
        let mode = tree.state.downcast_ref::<AddressState>().mode;

        self.child_mut(mode)
            .layout(&mut tree.children[mode], renderer, limits)
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
                state.mode = (state.mode + 1) % self.children.len();
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
        let mode = tree.state.downcast_ref::<AddressState>().mode;

        self.child(mode).draw(
            &tree.children[mode],
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
    fn child(&self, mode: usize) -> &dyn Widget<Message, theme::Theme, iced::Renderer> {
        self.children[mode].as_widget()
    }

    fn child_mut(&mut self, mode: usize) -> &mut dyn Widget<Message, theme::Theme, iced::Renderer> {
        self.children[mode].as_widget_mut()
    }
}

impl<'a, Message: 'a> From<Address<'a, Message>> for LianaElement<'a, Message> {
    fn from(address: Address<'a, Message>) -> Self {
        LianaElement::new(address)
    }
}
