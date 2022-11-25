use iced::{
    widget::{Button, Column},
    Element,
};
use iced_lazy::{self, Component};
use std::marker::PhantomData;

use super::button::Style;

pub fn collapse<
    'a,
    Message: 'a,
    T: Into<Message> + Clone + 'a,
    H: Fn() -> Element<'a, T> + 'a,
    C: Fn() -> Element<'a, T> + 'a,
>(
    header: H,
    content: C,
) -> impl Into<Element<'a, Message>> {
    Collapse {
        header,
        content,
        phantom: PhantomData,
    }
}

struct Collapse<'a, H, C> {
    header: H,
    content: C,
    phantom: PhantomData<&'a H>,
}

#[derive(Debug, Clone, Copy)]
enum Event<T> {
    Internal(T),
    Collapse(bool),
}

impl<'a, Message, T, H, C> Component<Message, iced::Renderer> for Collapse<'a, H, C>
where
    T: Into<Message> + Clone + 'a,
    H: Fn() -> Element<'a, T>,
    C: Fn() -> Element<'a, T>,
{
    type State = bool;
    type Event = Event<T>;

    fn update(&mut self, state: &mut Self::State, event: Event<T>) -> Option<Message> {
        match event {
            Event::Internal(e) => Some(e.into()),
            Event::Collapse(s) => {
                *state = s;
                None
            }
        }
    }

    fn view(&self, state: &Self::State) -> Element<Self::Event> {
        if *state {
            Column::new()
                .push(
                    Button::new((self.header)().map(Event::Internal))
                        .style(Style::TransparentBorder.into())
                        .padding(10)
                        .on_press(Event::Collapse(false)),
                )
                .push((self.content)().map(Event::Internal))
                .into()
        } else {
            Column::new()
                .push(
                    Button::new((self.header)().map(Event::Internal))
                        .style(Style::TransparentBorder.into())
                        .padding(10)
                        .on_press(Event::Collapse(true)),
                )
                .into()
        }
    }
}

impl<'a, Message, T, H: 'a, C: 'a> From<Collapse<'a, H, C>> for Element<'a, Message>
where
    Message: 'a,
    T: Into<Message> + Clone + 'a,
    H: Fn() -> Element<'a, T, iced::Renderer>,
    C: Fn() -> Element<'a, T, iced::Renderer>,
{
    fn from(c: Collapse<'a, H, C>) -> Self {
        iced_lazy::component(c)
    }
}
