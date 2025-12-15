mod backend;
mod client;
mod models;
mod state;
mod views;
mod wss;

use crossbeam::channel;
use iced::{Pixels, Settings, Subscription, Task};
use liana_ui::theme::Theme;
use state::{Msg, State};
use std::{pin::Pin, sync::Mutex, thread, time::Duration};

// Global channel for backend communication
static BACKEND_RECV: Mutex<Option<channel::Receiver<backend::Notification>>> = Mutex::new(None);
const BACKEND_URL: &str = "debug";
const PROTOCOL_VERSION: u8 = 1;

fn main() -> iced::Result {
    let settings = Settings {
        default_font: liana_ui::font::REGULAR,
        default_text_size: Pixels(16.0),
        fonts: liana_ui::font::load(),
        ..Default::default()
    };

    let window_settings = iced::window::Settings {
        size: iced::Size::new(1200.0, 800.0),
        ..Default::default()
    };

    iced::application(
        PolicyBuilder::title,
        PolicyBuilder::update,
        PolicyBuilder::view,
    )
    .theme(|_| Theme::default())
    .settings(settings)
    .window(window_settings)
    .subscription(PolicyBuilder::subscription)
    .run_with(|| PolicyBuilder::new(()))
}

pub struct PolicyBuilder {
    state: State,
}

impl PolicyBuilder {
    pub fn new(_flags: ()) -> (Self, Task<Msg>) {
        let mut state = State::new();
        state.connect_backend(BACKEND_URL.to_string(), PROTOCOL_VERSION);

        let builder = Self { state };

        (builder, Task::none())
    }

    pub fn subscription(&self) -> Subscription<Msg> {
        Subscription::run(BackendSubscription::new)
    }

    pub fn title(&self) -> String {
        "Liana Business template builder".to_string()
    }

    pub fn update(&mut self, message: Msg) -> Task<Msg> {
        self.state.update(message)
    }

    pub fn view(&self) -> liana_ui::widget::Element<Msg> {
        self.state.view()
    }
}

// Subscription for backend stream
struct BackendSubscription {
    receiver: Option<channel::Receiver<backend::Notification>>,
}

impl BackendSubscription {
    fn new() -> Self {
        if let Ok(mut channel_guard) = BACKEND_RECV.lock() {
            if let Some(receiver) = channel_guard.take() {
                return Self {
                    receiver: Some(receiver),
                };
            }
        }
        Self { receiver: None }
    }
}

impl iced::futures::Stream for BackendSubscription {
    type Item = Msg;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;

        let this = Pin::get_mut(self);
        loop {
            // NOTE: If there is a new connection we replace this one
            if let Some(recv) = BACKEND_RECV.lock().expect("poisoned").take() {
                println!("poll_next() new connection");
                this.receiver = Some(recv);
            }

            if let Some(receiver) = this.receiver.as_mut() {
                // NOTE: if we send a Poll::Ready(None), iced will drop subscription so
                // we call (blocking) .recv().
                if let Ok(m) = receiver.recv() {
                    return Poll::Ready(Some(Msg::BackendNotif(m)));
                } else {
                    this.receiver = None;
                };
            }
            // NOTE: is there is no receiver we just block until there is one
            // with a delay to avoid spinloop
            thread::sleep(Duration::from_millis(500));
        }
    }
}

impl Drop for BackendSubscription {
    fn drop(&mut self) {
        println!("BackendSubscription dropped");
    }
}

// Ensure close() is called when PolicyBuilder is dropped
impl Drop for PolicyBuilder {
    fn drop(&mut self) {
        // Call close() on the backend
        self.state.close_backend();
    }
}
