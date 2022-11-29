use iced::{
    widget::{Button, Column},
    Element,
};
use iced_lazy::{self, Component};
use std::marker::PhantomData;

pub struct Collapse<'a, M, H, F, C> {
    before: H,
    after: F,
    content: C,
    phantom: PhantomData<&'a M>,
}

impl<'a, Message, T, H, F, C> Collapse<'a, Message, H, F, C>
where
    Message: 'a,
    T: Into<Message> + Clone + 'a,
    H: Fn() -> Button<'a, Event<T>> + 'a,
    F: Fn() -> Button<'a, Event<T>> + 'a,
    C: Fn() -> Element<'a, T> + 'a,
{
    pub fn new(before: H, after: F, content: C) -> Self {
        Collapse {
            before,
            after,
            content,
            phantom: PhantomData,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Event<T> {
    Internal(T),
    Collapse(bool),
}

impl<'a, Message, T, H, F, C> Component<Message, iced::Renderer> for Collapse<'a, Message, H, F, C>
where
    T: Into<Message> + Clone + 'a,
    H: Fn() -> Button<'a, Event<T>>,
    F: Fn() -> Button<'a, Event<T>>,
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
                .push((self.after)().on_press(Event::Collapse(false)))
                .push((self.content)().map(Event::Internal))
                .into()
        } else {
            Column::new()
                .push((self.before)().on_press(Event::Collapse(true)))
                .into()
        }
    }
}

impl<'a, Message, T, H: 'a, F: 'a, C: 'a> From<Collapse<'a, Message, H, F, C>>
    for Element<'a, Message>
where
    Message: 'a,
    T: Into<Message> + Clone + 'a,
    H: Fn() -> Button<'a, Event<T>, iced::Renderer>,
    F: Fn() -> Button<'a, Event<T>, iced::Renderer>,
    C: Fn() -> Element<'a, T, iced::Renderer>,
{
    fn from(c: Collapse<'a, Message, H, F, C>) -> Self {
        iced_lazy::component(c)
    }
}
