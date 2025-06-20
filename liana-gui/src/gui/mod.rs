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
use liana_ui::widget::{Column, Element};

pub mod pane;
pub mod tab;

use crate::{dir::LianaDirectory, logger::Logger, VERSION};

pub struct GUI {
    pane: pane::Pane,
    config: Config,
    // We may change the directory of log outputs later
    _logger: Logger,
}

#[derive(Debug)]
pub enum Key {
    Tab(bool),
}

#[derive(Debug)]
pub enum Message {
    CtrlC,
    FontLoaded(Result<(), iced::font::Error>),
    Pane(pane::Message),
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
        format!("Liana v{}", VERSION)
    }

    pub fn new((config, log_level): (Config, Option<LevelFilter>)) -> (GUI, Task<Message>) {
        let logger = Logger::setup(log_level.unwrap_or(LevelFilter::INFO));
        logger.set_running_mode(
            config.liana_directory.clone(),
            log_level.unwrap_or_else(|| log_level.unwrap_or(LevelFilter::INFO)),
        );
        let mut cmds = vec![Task::perform(ctrl_c(), |_| Message::CtrlC)];
        let (pane, cmd) = pane::Pane::new(&config);
        cmds.push(cmd.map(Message::Pane));
        (
            Self {
                pane,
                config,
                _logger: logger,
            },
            Task::batch(cmds),
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::CtrlC
            | Message::Event(iced::Event::Window(iced::window::Event::CloseRequested)) => {
                self.pane.stop();
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
            Message::Pane(msg) => self.pane.update(msg, &self.config).map(Message::Pane),
            _ => Task::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            self.pane.subscription().map(Message::Pane),
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
        Column::new()
            .push(self.pane.tabs_menu_view().map(Message::Pane))
            .push(self.pane.view().map(Message::Pane))
            .into()
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
