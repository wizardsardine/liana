mod settings;

use std::sync::Arc;

use iced::pure::{column, Element};
use iced::{Command, Subscription};

use super::{cache::Cache, menu::Menu, message::Message, view};

pub use settings::SettingsState;

use crate::daemon::Daemon;

pub trait State {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message>;
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Send + Sync>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message>;
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
    fn load(&self, _daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        Command::none()
    }
}

pub struct Home {}

impl State for Home {
    fn view<'a>(&self, _cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(&Menu::Home, None, column())
    }
    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Send + Sync>,
        _cache: &Cache,
        _message: Message,
    ) -> Command<Message> {
        Command::none()
    }
}

impl From<Home> for Box<dyn State> {
    fn from(s: Home) -> Box<dyn State> {
        Box::new(s)
    }
}
