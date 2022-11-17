use iced::pure::{button, column};
use iced_lazy::pure::{self, Component};
use iced_native::text;
use iced_pure::Element;
use std::marker::PhantomData;

use crate::ui::component::button::Style;

pub fn collapse<
    'a,
    Message: 'a,
    T: Into<Message> + Clone + 'a,
    Renderer: text::Renderer + 'static,
    H: Fn() -> Element<'a, T, Renderer> + 'a,
    C: Fn() -> Element<'a, T, Renderer> + 'a,
>(
    header: H,
    content: C,
) -> impl Into<Element<'a, Message, Renderer>> {
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

impl<'a, Message, Renderer, T, H, C> Component<Message, Renderer> for Collapse<'a, H, C>
where
    T: Into<Message> + Clone + 'a,
    H: Fn() -> Element<'a, T, Renderer>,
    C: Fn() -> Element<'a, T, Renderer>,
    Renderer: text::Renderer + 'static,
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

    fn view(&self, state: &Self::State) -> Element<Self::Event, Renderer> {
        if *state {
            column()
                .push(
                    button((self.header)().map(Event::Internal))
                        .style(Style::TransparentBorder)
                        .padding(10)
                        .on_press(Event::Collapse(false)),
                )
                .push((self.content)().map(Event::Internal))
                .into()
        } else {
            column()
                .push(
                    button((self.header)().map(Event::Internal))
                        .padding(10)
                        .style(Style::TransparentBorder)
                        .on_press(Event::Collapse(true)),
                )
                .into()
        }
    }
}

impl<'a, Message, Renderer, T, H: 'a, C: 'a> From<Collapse<'a, H, C>>
    for Element<'a, Message, Renderer>
where
    Message: 'a,
    Renderer: 'static + text::Renderer,
    T: Into<Message> + Clone + 'a,
    H: Fn() -> Element<'a, T, Renderer>,
    C: Fn() -> Element<'a, T, Renderer>,
{
    fn from(c: Collapse<'a, H, C>) -> Self {
        pure::component(c)
    }
}
