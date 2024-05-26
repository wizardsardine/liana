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
    state: bool,
}

impl<'a, Message, T, H, F, C, Theme, Renderer> Collapse<'a, Message, H, F, C>
where
    Renderer: advanced::Renderer,
    Theme: iced::widget::button::StyleSheet,
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
            state: false,
        }
    }

    pub fn collapsed(mut self, state: bool) -> Self {
        self.state = state;
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
    Theme: 'a + iced::widget::button::StyleSheet,
{
    type State = bool;
    type Event = Event<T>;

    fn update(&mut self, _state: &mut Self::State, event: Event<T>) -> Option<Message> {
        match event {
            Event::Internal(e) => Some(e.into()),
            Event::Collapse(s) => {
                self.state = s;
                None
            }
        }
    }

    fn view(&self, _state: &Self::State) -> Element<Self::Event, Theme, Renderer> {
        if self.state {
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

impl<'a, Message, T, H: 'a, F: 'a, C: 'a, Theme, Renderer> From<Collapse<'a, Message, H, F, C>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Renderer: 'a + advanced::Renderer,
    Theme: 'a + iced::widget::button::StyleSheet,
    T: Into<Message> + Clone + 'a,
    H: Fn() -> Button<'a, Event<T>, Theme, Renderer>,
    F: Fn() -> Button<'a, Event<T>, Theme, Renderer>,
    C: Fn() -> Element<'a, T, Theme, Renderer>,
{
    fn from(c: Collapse<'a, Message, H, F, C>) -> Self {
        component(c)
    }
}
