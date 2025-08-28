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

mod cache;
pub mod pane;
pub mod tab;

use crate::{
    app::{
        self,
        cache::{FiatPrice, FiatPriceRequest, FIAT_PRICE_UPDATE_INTERVAL_SECS},
        message::FiatMessage,
        settings::global::{GlobalSettings, WindowConfig},
    },
    dir::LianaDirectory,
    gui::cache::{FiatMessageAction, GlobalCache},
    launcher,
    logger::setup_logger,
    services::fiat::{
        api::{ListCurrenciesResult, PriceApi, PriceApiError},
        PriceClient, PriceSource,
    },
    utils::now,
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
    global_cache: GlobalCache,
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

    GetFiatPriceResult(app::cache::FiatPrice),
    /// Result of a request for the list of available currencies for a given source.
    ///
    /// The pane and tab that requested the list are included to be able to return the result.
    ListCurrenciesResult(
        pane_grid::Pane,
        usize, // tab id
        PriceSource,
        u64,
        Result<ListCurrenciesResult, PriceApiError>,
    ),
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
                global_cache: GlobalCache::default(),
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
            Message::GetFiatPriceResult(price) => {
                if self
                    .global_cache
                    .last_fiat_price_request(price.source(), price.currency())
                    != Some(&price.request)
                {
                    tracing::debug!(
                        "Ignoring fiat price result for {} from {} as it is not the last request",
                        price.currency(),
                        price.source(),
                    );
                    return Task::none();
                }
                if let Err(e) = price.res.as_ref() {
                    tracing::error!(
                        "Fiat price request for {} from {} returned error: {}",
                        price.currency(),
                        price.source(),
                        e
                    );
                }
                // Update the cache with the result even if there was an error.
                self.global_cache.insert_fiat_price(price.clone());
                // Npw update all affected tabs with the new price.
                self.update_tabs_with_fiat_price(price)
            }
            Message::ListCurrenciesResult(pane_id, tab_id, source, timestamp, res) => {
                if let Ok(list) = res.as_ref() {
                    self.global_cache
                        .insert_currencies(source, timestamp, list.currencies.clone());
                }
                // Return the result to the tab even if there was an error.
                if let Some(pane) = self.panes.get_mut(pane_id) {
                    pane.update_tab_with_app_msg(
                        tab_id,
                        FiatMessage::ListCurrenciesResult(source, timestamp, res),
                        &self.config,
                    )
                    .map(move |msg| Message::Pane(pane_id, msg))
                } else {
                    Task::none()
                }
            }
            Message::Pane(i, msg) => {
                match msg {
                    // Handle fiat messages separately.
                    pane::Message::Tab(tab_id, tab::Message::Run(inner))
                        if matches!(inner.as_ref(), app::Message::Fiat(_)) =>
                    {
                        // Use another if let to unbox the inner message.
                        if let app::Message::Fiat(fiat_msg) = *inner {
                            return self.handle_tab_fiat_message(i, tab_id, fiat_msg);
                        } else {
                            tracing::error!("Unexpected message type after unboxing");
                        }
                    }
                    _ => {
                        if let Some(pane) = self.panes.get_mut(i) {
                            return pane
                                .update(msg, &self.config)
                                .map(move |msg| Message::Pane(i, msg));
                        }
                    }
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

    // Helper function to update all tabs that have given fiat price enabled with the new price.
    fn update_tabs_with_fiat_price(&mut self, fiat_price: FiatPrice) -> Task<Message> {
        // First, collect all tabs that need updating to avoid multiple borrows
        let mut tabs_to_update = Vec::new();

        for (&pane_id, pane) in self.panes.iter() {
            for tab in pane.tabs.iter() {
                if tab
                    .wallet()
                    .is_some_and(|w| w.fiat_price_is_relevant(&fiat_price))
                {
                    tabs_to_update.push((pane_id, tab.id));
                }
            }
        }

        // Then update each tab
        let mut tasks = Vec::new();
        for (pane_id, tab_id) in tabs_to_update {
            if let Some(pane) = self.panes.get_mut(pane_id) {
                tasks.push(
                    pane.update_tab_with_app_msg(
                        tab_id,
                        FiatMessage::GetPriceResult(fiat_price.clone()),
                        &self.config,
                    )
                    .map(move |msg| Message::Pane(pane_id, msg)),
                );
            }
        }

        Task::batch(tasks)
    }

    fn handle_tab_fiat_message(
        &mut self,
        pane_id: pane_grid::Pane,
        tab_id: usize,
        fiat_msg: FiatMessage,
    ) -> Task<Message> {
        let Some(pane) = self.panes.get_mut(pane_id) else {
            return Task::none();
        };

        let Some(tab) = pane.tabs.iter().find(|t| t.id == tab_id) else {
            return Task::none();
        };

        let action = self.global_cache.fiat_message_action(&fiat_msg);
        match action {
            FiatMessageAction::UseCachedPrice(price) => {
                // Make sure the tab doesn't already have the price to avoid redundant updates.
                if tab
                    .cache()
                    .and_then(|c| c.fiat_price.as_ref())
                    .is_some_and(|tab_price| tab_price.request == price.request)
                {
                    tracing::debug!(
                        "Tab {} already has fiat price for {} from {}",
                        tab.id,
                        price.currency(),
                        price.source(),
                    );
                    return Task::none();
                }
                // Return the cached price.
                tracing::debug!(
                    "Returning cached fiat price for {} from {} to tab {}",
                    price.currency(),
                    price.source(),
                    tab.id,
                );
                // Return the cached price to the tab
                pane.update_tab_with_app_msg(
                    tab_id,
                    FiatMessage::GetPriceResult(price),
                    &self.config,
                )
                .map(move |msg| Message::Pane(pane_id, msg))
            }
            FiatMessageAction::RequestPrice(source, currency) => {
                let now = now().as_secs();
                // Do nothing if the last request for the same source & currency was recent, where
                // "recent" means within half the update interval. Using half the update interval is sufficient
                // as we are mostly concerned with preventing multiple requests being sent within seconds of each
                // other (e.g. after the GUI window is inactive for an extended period).
                if self
                    .global_cache
                    .last_fiat_price_request(source, currency)
                    .filter(|req| req.timestamp + FIAT_PRICE_UPDATE_INTERVAL_SECS / 2 > now)
                    .is_some()
                {
                    tracing::debug!(
                        "Fiat price for {} from {} has been requested recently",
                        currency,
                        source,
                    );
                    return Task::none();
                }

                let request = FiatPriceRequest {
                    source,
                    currency,
                    timestamp: now,
                };
                // Add the request to the cache before completion to prevent duplicate requests.
                self.global_cache.insert_fiat_price_request(request.clone());
                tracing::debug!(
                    "Getting fiat price in {} from {}",
                    request.currency,
                    request.source,
                );
                Task::perform(
                    async move { request.send_default().await },
                    Message::GetFiatPriceResult,
                )
            }
            FiatMessageAction::UseCachedCurrencies(source, timestamp, currencies) => {
                tracing::debug!("Using cached currencies list for {}", source,);
                pane.update_tab_with_app_msg(
                    tab_id,
                    FiatMessage::ListCurrenciesResult(
                        source,
                        timestamp,
                        Ok(ListCurrenciesResult { currencies }),
                    ),
                    &self.config,
                )
                .map(move |msg| Message::Pane(pane_id, msg))
            }
            FiatMessageAction::RequestCurrencies(source) => {
                tracing::debug!("Requesting list of currencies from {}", source);
                let now = now().as_secs();
                Task::perform(
                    async move {
                        let client = PriceClient::default_from_source(source);
                        (tab_id, source, now, client.list_currencies().await)
                    },
                    move |(tab_id, source, now, res)| {
                        Message::ListCurrenciesResult(pane_id, tab_id, source, now, res)
                    },
                )
            }
            FiatMessageAction::None => pane
                .update_tab_with_app_msg(tab_id, fiat_msg, &self.config)
                .map(move |msg| Message::Pane(pane_id, msg)),
        }
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
