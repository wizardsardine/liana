#![allow(deprecated)]

use iced::{
    advanced,
    widget::{column, component, Button, Component},
    Element,
};
use std::marker::PhantomData;

pub struct Collapse<'a, M, H, F, C> {
    before: H,
    after: F,
    content: C,
    phantom: PhantomData<&'a M>,
    init_state: bool,
}

impl<'a, Message, T, H, F, C, Theme, Renderer> Collapse<'a, Message, H, F, C>
where
    Renderer: advanced::Renderer,
    Theme: iced::widget::button::Catalog,
    Message: 'a,
    T: Into<Message> + Clone + 'a,
    H: Fn() -> Button<'a, Event<T>, Theme, Renderer> + 'a,
    F: Fn() -> Button<'a, Event<T>, Theme, Renderer> + 'a,
    C: Fn() -> Element<'a, T, Theme, Renderer> + 'a,
{
    pub fn new(before: H, after: F, content: C) -> Self {
        Collapse {
            before,
            after,
            content,
            phantom: PhantomData,
            init_state: false,
        }
    }

    pub fn collapsed(mut self, state: bool) -> Self {
        self.init_state = state;
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Event<T> {
    Internal(T),
    Collapse(bool),
}

impl<'a, Message, T, H, F, C, Theme, Renderer> Component<Message, Theme, Renderer>
    for Collapse<'a, Message, H, F, C>
where
    T: Into<Message> + Clone + 'a,
    H: Fn() -> Button<'a, Event<T>, Theme, Renderer>,
    F: Fn() -> Button<'a, Event<T>, Theme, Renderer>,
    C: Fn() -> Element<'a, T, Theme, Renderer>,
    Renderer: 'a + advanced::Renderer,
    Theme: 'a + iced::widget::button::Catalog,
{
    type State = Option<bool>;
    type Event = Event<T>;

    fn update(&mut self, state: &mut Self::State, event: Event<T>) -> Option<Message> {
        match event {
            Event::Internal(e) => Some(e.into()),
            Event::Collapse(s) => {
                *state = Some(s);
                None
            }
        }
    }

    fn view(&self, state: &Self::State) -> Element<Self::Event, Theme, Renderer> {
        match state {
            Some(true) => column![
                (self.after)().on_press(Event::Collapse(false)),
                (self.content)().map(Event::Internal)
            ]
            .into(),
            Some(false) => column![(self.before)().on_press(Event::Collapse(true))].into(),
            None => {
                if self.init_state {
                    column![
                        (self.after)().on_press(Event::Collapse(false)),
                        (self.content)().map(Event::Internal)
                    ]
                    .into()
                } else {
                    column![(self.before)().on_press(Event::Collapse(true))].into()
                }
            }
        }
    }
}

impl<'a, Message, T, H: 'a, F: 'a, C: 'a, Theme, Renderer> From<Collapse<'a, Message, H, F, C>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Renderer: 'a + advanced::Renderer,
    Theme: 'a + iced::widget::button::Catalog,
    T: Into<Message> + Clone + 'a,
    H: Fn() -> Button<'a, Event<T>, Theme, Renderer>,
    F: Fn() -> Button<'a, Event<T>, Theme, Renderer>,
    C: Fn() -> Element<'a, T, Theme, Renderer>,
{
    fn from(c: Collapse<'a, Message, H, F, C>) -> Self {
        component(c)
    }
}
