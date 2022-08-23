use iced::pure::{column, Element};
use iced::{Command, Subscription};

use super::{context::Context, message::Message, view};

pub trait State {
    fn view<'a>(&self, ctx: &'a Context) -> Element<'a, view::Message>;
    fn update(&mut self, ctx: &Context, message: Message) -> Command<Message>;
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
    fn load(&self, _ctx: &Context) -> Command<Message> {
        Command::none()
    }
}

pub struct Home {}

impl State for Home {
    fn view<'a>(&self, ctx: &'a Context) -> Element<'a, view::Message> {
        view::dashboard(&ctx.menu, None, column())
    }
    fn update(&mut self, _ctx: &Context, _message: Message) -> Command<Message> {
        Command::none()
    }
}

impl From<Home> for Box<dyn State> {
    fn from(s: Home) -> Box<dyn State> {
        Box::new(s)
    }
}
