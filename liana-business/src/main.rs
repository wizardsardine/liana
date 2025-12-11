mod backend;
mod models;
mod state;
mod views;

use backend::Response;
use iced::{Pixels, Settings, Subscription, Task};
use liana_ui::theme::Theme;
use state::{Msg, State};
use std::sync::{mpsc, Mutex};

// Global channel for backend communication
static BACKEND_RECV: Mutex<Option<mpsc::Receiver<backend::Response>>> = Mutex::new(None);
const BACKEND_URL: &str = "127.0.0.1:8081";
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
    receiver: mpsc::Receiver<backend::Response>,
}

impl BackendSubscription {
    fn new() -> Self {
        if let Ok(mut channel_guard) = BACKEND_RECV.lock() {
            if let Some(receiver) = channel_guard.take() {
                return Self { receiver };
            }
        }
        // Fallback: create a dummy channel if not available and error
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Response::Error(backend::Error::SubscriptionFailed))
            .unwrap();
        Self { receiver }
    }
}

impl iced::futures::Stream for BackendSubscription {
    type Item = Msg;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;

        // NOTE: If there is a new connection we drop this one
        if BACKEND_RECV.lock().expect("poisoned").is_some() {
            // Iced will drop the subscription after this
            return Poll::Ready(None);
        }

        // NOTE: if we send a Poll::Ready(None), iced will drop subscription so
        // we call (blocking) .recv().
        if let Ok(m) = self.receiver.recv() {
            Poll::Ready(Some(Msg::BackendResponse(m)))
        } else {
            Poll::Ready(Some(Msg::BackendDisconnected))
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
