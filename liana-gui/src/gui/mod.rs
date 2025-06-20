use iced::{
    event::{self, Event},
    keyboard,
    widget::{focus_next, focus_previous},
    Subscription, Task,
};
use tracing::{error, info};
use tracing_subscriber::filter::LevelFilter;
extern crate serde;
extern crate serde_json;

use liana::miniscript::bitcoin;
use liana_ui::widget::Element;

pub mod tab;

use crate::{dir::LianaDirectory, logger::Logger, VERSION};

pub struct GUI {
    state: tab::Tab,
    logger: Logger,
    // if set up, it overrides the level filter of the logger.
    log_level: Option<LevelFilter>,
}

#[derive(Debug)]
pub enum Key {
    Tab(bool),
}

#[derive(Debug)]
pub enum Message {
    CtrlC,
    FontLoaded(Result<(), iced::font::Error>),
    Tab(tab::Message),
    KeyPressed(Key),
    Event(iced::Event),
}

impl From<Result<(), iced::font::Error>> for Message {
    fn from(value: Result<(), iced::font::Error>) -> Self {
        Self::FontLoaded(value)
    }
}

async fn ctrl_c() -> Result<(), ()> {
    if let Err(e) = tokio::signal::ctrl_c().await {
        error!("{}", e);
    };
    info!("Signal received, exiting");
    Ok(())
}

impl GUI {
    pub fn title(&self) -> String {
        match &self.state.0 {
            tab::State::Installer(_) => format!("Liana v{} Installer", VERSION),
            tab::State::App(a) => format!("Liana v{} {}", VERSION, a.title()),
            _ => format!("Liana v{}", VERSION),
        }
    }

    pub fn new((config, log_level): (Config, Option<LevelFilter>)) -> (GUI, Task<Message>) {
        let logger = Logger::setup(log_level.unwrap_or(LevelFilter::INFO));
        let mut cmds = vec![Task::perform(ctrl_c(), |_| Message::CtrlC)];
        let (state, cmd) = tab::Tab::new(config.liana_directory, config.network);
        cmds.push(cmd.map(Message::Tab));
        (
            Self {
                state,
                logger,
                log_level,
            },
            Task::batch(cmds),
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::CtrlC
            | Message::Event(iced::Event::Window(iced::window::Event::CloseRequested)) => {
                self.state.stop();
                iced::window::get_latest().and_then(iced::window::close)
            }
            Message::KeyPressed(Key::Tab(shift)) => {
                log::debug!("Tab pressed!");
                if shift {
                    focus_previous()
                } else {
                    focus_next()
                }
            }
            Message::Tab(msg) => self
                .state
                .update(msg, &self.logger, self.log_level)
                .map(Message::Tab),
            _ => Task::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            self.state.subscription().map(Message::Tab),
            iced::event::listen_with(|event, status, _| match (&event, status) {
                (
                    Event::Keyboard(keyboard::Event::KeyPressed {
                        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab),
                        modifiers,
                        ..
                    }),
                    event::Status::Ignored,
                ) => Some(Message::KeyPressed(Key::Tab(modifiers.shift()))),
                (
                    iced::Event::Window(iced::window::Event::CloseRequested),
                    event::Status::Ignored,
                ) => Some(Message::Event(event)),
                _ => None,
            }),
        ])
    }

    pub fn view(&self) -> Element<Message> {
        self.state.view().map(Message::Tab)
    }

    pub fn scale_factor(&self) -> f64 {
        1.0
    }
}

pub struct Config {
    pub liana_directory: LianaDirectory,
    network: Option<bitcoin::Network>,
}

impl Config {
    pub fn new(liana_directory: LianaDirectory, network: Option<bitcoin::Network>) -> Self {
        Self {
            liana_directory,
            network,
        }
    }
}
