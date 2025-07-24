use iced::{
    event::{self, Event},
    keyboard,
    widget::{focus_next, focus_previous, pane_grid},
    Length, Size, Subscription, Task,
};
use iced_runtime::window;
use tracing::{error, info};
use tracing_subscriber::filter::LevelFilter;
extern crate serde;
extern crate serde_json;

use liana::miniscript::bitcoin;
use liana_ui::widget::{Column, Container, Element};

pub mod pane;
pub mod tab;

use crate::{
    app::settings::global::{GlobalSettings, WindowConfig},
    dir::LianaDirectory,
    launcher,
    logger::setup_logger,
    VERSION,
};

use iced::window::Id;

pub struct GUI {
    panes: pane_grid::State<pane::Pane>,
    focus: Option<pane_grid::Pane>,
    config: Config,
    window_id: Option<Id>,
    window_init: Option<bool>,
    window_config: Option<WindowConfig>,
}

#[derive(Debug)]
pub enum Key {
    Tab(bool),
}

#[derive(Debug)]
pub enum Message {
    CtrlC,
    FontLoaded(Result<(), iced::font::Error>),
    Pane(pane_grid::Pane, pane::Message),
    KeyPressed(Key),
    Event(iced::Event),

    Clicked(pane_grid::Pane),
    Dragged(pane_grid::DragEvent),
    Resized(pane_grid::ResizeEvent),
    Window(Option<Id>),
    WindowSize(Size),
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
        let log_level = log_level.unwrap_or(LevelFilter::INFO);
        if let Err(e) = setup_logger(log_level, config.liana_directory.clone()) {
            tracing::warn!("Error while setting error: {}", e);
        }
        let mut cmds = vec![
            window::get_oldest().map(Message::Window),
            Task::perform(ctrl_c(), |_| Message::CtrlC),
        ];
        let (pane, cmd) = pane::Pane::new(&config);
        let (panes, focused_pane) = pane_grid::State::new(pane);
        cmds.push(cmd.map(move |msg| Message::Pane(focused_pane, msg)));
        let window_config =
            GlobalSettings::load_window_config(&GlobalSettings::path(&config.liana_directory));
        let window_init = window_config.is_some().then_some(true);
        (
            Self {
                panes,
                focus: Some(focused_pane),
                config,
                window_id: None,
                window_init,
                window_config,
            },
            Task::batch(cmds),
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // we get this message only once at startup
            Message::Window(id) => {
                self.window_id = id;
                // Common case: if there is an already saved screen size we reuse it
                if let (Some(id), Some(WindowConfig { width, height })) = (id, &self.window_config)
                {
                    window::resize(
                        id,
                        Size {
                            width: *width,
                            height: *height,
                        },
                    )
                // Initial startup: we maximize the screen in order to know the max usable screen area
                } else if let Some(id) = &self.window_id {
                    window::maximize(*id, true)
                } else {
                    Task::none()
                }
            }
            Message::WindowSize(monitor_size) => {
                let cloned_cfg = self.window_config.clone();
                match (cloned_cfg, &self.window_init, &self.window_id) {
                    // no previous screen size recorded && window maximized
                    (None, Some(false), Some(id)) => {
                        self.window_init = Some(true);
                        let mut batch = vec![window::maximize(*id, false)];
                        let new_size = if monitor_size.height >= 1200.0 {
                            let size = Size {
                                width: 1200.0,
                                height: 950.0,
                            };
                            batch.push(window::resize(*id, size));
                            size
                        } else {
                            batch.push(window::resize(*id, iced::window::Settings::default().size));
                            iced::window::Settings::default().size
                        };
                        self.window_config = Some(WindowConfig {
                            width: new_size.width,
                            height: new_size.height,
                        });
                        Task::batch(batch)
                    }
                    // we already have a record of the last window size and we update it
                    (Some(WindowConfig { width, height }), _, _) => {
                        if width != monitor_size.width || height != monitor_size.height {
                            if let Some(cfg) = &mut self.window_config {
                                cfg.width = monitor_size.width;
                                cfg.height = monitor_size.height;
                            }
                        }
                        Task::none()
                    }
                    // we ignore the first notification about initial window size it will always be
                    // the default one
                    _ => {
                        if self.window_init.is_none() {
                            self.window_init = Some(false);
                        }
                        Task::none()
                    }
                }
            }
            Message::CtrlC
            | Message::Event(iced::Event::Window(iced::window::Event::CloseRequested)) => {
                for (_, pane) in self.panes.iter_mut() {
                    pane.stop();
                }
                if let Some(window_config) = &self.window_config {
                    let path = GlobalSettings::path(&self.config.liana_directory);
                    if let Err(e) = GlobalSettings::update_window_config(&path, window_config) {
                        tracing::error!("Failed to update the window config: {e}");
                    }
                }
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
            Message::Pane(pane_id, pane::Message::View(pane::ViewMessage::SplitTab(i))) => {
                if let Some(p) = self.panes.get_mut(pane_id) {
                    if let Some(tab) = p.remove_tab(i) {
                        let result = self.panes.split(
                            pane_grid::Axis::Vertical,
                            pane_id,
                            pane::Pane::new_with_tab(tab.state),
                        );

                        if let Some((pane, _)) = result {
                            self.focus = Some(pane);
                        }
                    }
                }
                Task::none()
            }
            Message::Pane(pane_id, pane::Message::View(pane::ViewMessage::CloseTab(i))) => {
                if let Some(pane) = self.panes.get_mut(pane_id) {
                    let _ = pane
                        .update(
                            pane::Message::View(pane::ViewMessage::CloseTab(i)),
                            &self.config,
                        )
                        .map(move |msg| Message::Pane(pane_id, msg));
                    if pane.tabs.is_empty() {
                        self.panes.close(pane_id);
                        if self.focus == Some(pane_id) {
                            self.focus = None;
                        }
                    }
                }
                if !self.panes.iter().any(|(_, p)| !p.tabs.is_empty()) {
                    return iced::window::get_latest().and_then(iced::window::close);
                }
                Task::none()
            }
            // In case of wallet deletion, remove any tab where the wallet id is currently running.
            Message::Pane(p, pane::Message::Tab(t, tab::Message::Launch(msg))) => {
                let mut tasks = Vec::new();
                if let launcher::Message::View(launcher::ViewMessage::DeleteWallet(
                    launcher::DeleteWalletMessage::Confirm(wallet_id),
                )) = msg.as_ref()
                {
                    let mut panes_to_close = Vec::<pane_grid::Pane>::new();
                    for (id, pane) in self.panes.iter_mut() {
                        let tabs_to_close: Vec<usize> = pane
                            .tabs
                            .iter()
                            .enumerate()
                            .filter_map(|(i, tab)| {
                                if match &tab.state {
                                    tab::State::App(a) => a.wallet_id() == *wallet_id,
                                    tab::State::Loader(l) => {
                                        l.wallet_settings.wallet_id() == *wallet_id
                                    }
                                    _ => false,
                                } {
                                    Some(i)
                                } else {
                                    None
                                }
                            })
                            .collect();
                        for i in tabs_to_close {
                            pane.close_tab(i);
                        }
                        if pane.tabs.is_empty() {
                            panes_to_close.push(*id);
                        }
                    }
                    for id in panes_to_close {
                        self.panes.close(id);
                    }
                    for (&id, pane) in self.panes.iter() {
                        for tab in &pane.tabs {
                            if let tab::State::Launcher(l) = &tab.state {
                                let tab_id = tab.id;
                                tasks.push(l.reload().map(move |msg| {
                                    Message::Pane(
                                        id,
                                        pane::Message::Tab(
                                            tab_id,
                                            tab::Message::Launch(Box::new(msg)),
                                        ),
                                    )
                                }));
                            }
                        }
                    }
                }
                if let Some(pane) = self.panes.get_mut(p) {
                    tasks.push(
                        pane.update(
                            pane::Message::Tab(t, tab::Message::Launch(msg)),
                            &self.config,
                        )
                        .map(move |msg| Message::Pane(p, msg)),
                    );
                }
                Task::batch(tasks)
            }
            Message::Pane(i, msg) => {
                if let Some(pane) = self.panes.get_mut(i) {
                    return pane
                        .update(msg, &self.config)
                        .map(move |msg| Message::Pane(i, msg));
                }
                Task::none()
            }
            Message::Clicked(pane) => {
                self.focus = Some(pane);
                Task::none()
            }
            Message::Resized(pane_grid::ResizeEvent { split, ratio }) => {
                self.panes.resize(split, ratio);
                Task::none()
            }
            Message::Dragged(pane_grid::DragEvent::Dropped { pane, target }) => {
                if let pane_grid::Target::Pane(p, pane_grid::Region::Center) = target {
                    let (tabs, focused_tab) = if let Some(origin) = self.panes.get_mut(pane) {
                        (std::mem::take(&mut origin.tabs), origin.focused_tab)
                    } else {
                        (Vec::new(), 0)
                    };

                    if let Some(dest) = self.panes.get_mut(p) {
                        if !tabs.is_empty() {
                            dest.add_tabs(tabs, focused_tab);
                        }
                    }
                    self.panes.close(pane);
                    self.focus = Some(p);
                } else {
                    self.panes.drop(pane, target);
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut vec = vec![iced::event::listen_with(|event, status, _| {
            match (&event, status) {
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
                (iced::Event::Window(iced::window::Event::Resized(size)), _) => {
                    Some(Message::WindowSize(*size))
                }
                _ => None,
            }
        })];
        for (id, pane) in self.panes.iter() {
            vec.push(
                pane.subscription()
                    .with(*id)
                    .map(|(id, msg)| Message::Pane(id, msg)),
            );
        }
        Subscription::batch(vec)
    }

    pub fn view(&self) -> Element<Message> {
        if self.panes.len() == 1 {
            if let Some((&id, pane)) = self.panes.iter().nth(0) {
                return Column::new()
                    .push(pane.tabs_menu_view().map(move |msg| Message::Pane(id, msg)))
                    .push(pane.view().map(move |msg| Message::Pane(id, msg)))
                    .into();
            }
        }

        let focus = self.focus;
        let pane_grid = pane_grid::PaneGrid::new(&self.panes, |id, pane, _| {
            let _is_focused = focus == Some(id);

            pane_grid::Content::new(pane.view().map(move |msg| Message::Pane(id, msg))).title_bar(
                pane_grid::TitleBar::new(
                    pane.tabs_menu_view().map(move |msg| Message::Pane(id, msg)),
                ),
            )
        })
        .spacing(10)
        .width(Length::Fill)
        .height(Length::Fill)
        .on_click(Message::Clicked)
        .on_drag(Message::Dragged)
        .on_resize(10, Message::Resized);

        Container::new(pane_grid)
            .style(liana_ui::theme::pane_grid::pane_grid_background)
            .width(Length::Fill)
            .height(Length::Fill)
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
