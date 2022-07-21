pub mod config;
pub mod context;
pub mod menu;
pub mod message;
pub mod state;

mod error;

use std::sync::Arc;
use std::time::Duration;

use iced::pure::Element;
use iced::{clipboard, time, Command, Subscription};
use iced_native::{window, Event};

pub use config::Config;
pub use message::Message;

use state::{Home, State};

use crate::app::context::Context;

pub struct App {
    should_exit: bool,
    state: Box<dyn State>,
    context: Context,
}

pub fn new_state(_context: &Context) -> Box<dyn State> {
    Home {}.into()
}

impl App {
    pub fn new(context: Context) -> (App, Command<Message>) {
        let state = new_state(&context);
        let cmd = state.load(&context);
        (
            Self {
                should_exit: false,
                state,
                context,
            },
            cmd,
        )
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            iced_native::subscription::events().map(Message::Event),
            time::every(Duration::from_secs(30)).map(|_| Message::Tick),
            self.state.subscription(),
        ])
    }

    pub fn should_exit(&self) -> bool {
        self.should_exit
    }

    pub fn stop(&mut self) {
        log::info!("Close requested");
        if !self.context.daemon.is_external() {
            log::info!("Stopping internal daemon...");
            if let Some(d) = Arc::get_mut(&mut self.context.daemon) {
                d.stop().expect("Daemon is internal");
                log::info!("Internal daemon stopped");
                self.should_exit = true;
            }
        } else {
            self.should_exit = true;
        }
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::LoadDaemonConfig(cfg) => {
                let res = self.context.load_daemon_config(*cfg);
                self.update(Message::DaemonConfigLoaded(res))
            }
            Message::Menu(menu) => {
                self.context.menu = menu;
                self.state = new_state(&self.context);
                self.state.load(&self.context)
            }
            Message::Clipboard(text) => clipboard::write(text),
            Message::Event(Event::Window(window::Event::CloseRequested)) => {
                self.stop();
                Command::none()
            }
            _ => self.state.update(&self.context, message),
        }
    }

    pub fn view(&self) -> Element<Message> {
        self.state.view(&self.context)
    }
}
