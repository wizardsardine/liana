/// from hecjr idea on Discord
use iced::{
    widget::{Column, Row},
    Element,
};

pub trait Collection<'a, Message>: Sized {
    fn push(self, element: impl Into<Element<'a, Message>>) -> Self;

    fn push_maybe(self, element: Option<impl Into<Element<'a, Message>>>) -> Self {
        match element {
            Some(element) => self.push(element),
            None => self,
        }
    }
}

impl<'a, Message> Collection<'a, Message> for Column<'a, Message> {
    fn push(self, element: impl Into<Element<'a, Message>>) -> Self {
        Self::push(self, element)
    }
}

impl<'a, Message> Collection<'a, Message> for Row<'a, Message> {
    fn push(self, element: impl Into<Element<'a, Message>>) -> Self {
        Self::push(self, element)
    }
}
