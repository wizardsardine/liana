//! Synthetic native host for the production Tenshu shell and shared actions.
//!
//! Run with `cargo run -p coincube-gui --example keyboard_shell`. This host
//! constructs no wallet, daemon, Connect client, keychain or payment service.
//! It lets a QA pass send real OS Tab, Shift-Tab, Enter, Space and Escape to
//! the same shell widgets used by the app.

use coincube_gui::{
    app::{
        features::MarketplaceServerFlags,
        menu::{CubeSubMenu, Menu},
        view::{self, nav::shell, nav::split::ShellSplit, nav::NavContext},
        ConnectionStatus,
    },
    chain::ChainId,
};
use coincube_ui::{
    font,
    theme::{self, palette::ThemeMode},
    widget::{action, Element},
};
use iced::{
    event, keyboard,
    widget::{column, container, text, text_input},
    Event, Length, Size, Subscription, Task,
};

#[derive(Debug, Clone)]
enum Message {
    Shell(view::Message),
    InputChanged(String),
    InputSubmitted,
    Tab(bool),
    Escape,
    ToggleTheme,
}

struct Fixture {
    menu: Menu,
    shell: shell::ShellState,
    mode: ThemeMode,
    stream_status: ConnectionStatus,
    input: String,
    last_action: String,
    action_count: u64,
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            menu: Menu::Cube(CubeSubMenu::Overview),
            shell: shell::ShellState::default(),
            mode: ThemeMode::Dark,
            stream_status: ConnectionStatus::Inactive,
            input: String::new(),
            last_action: "No action yet".to_owned(),
            action_count: 0,
        }
    }
}

impl Fixture {
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tab(reverse) => {
                if reverse {
                    action::focus_previous_visible(shell::MIDDLE_SCROLL_ID)
                } else {
                    action::focus_next_visible(shell::MIDDLE_SCROLL_ID)
                }
            }
            Message::Escape => shell::escape_intent(self.shell)
                .map_or_else(Task::none, |intent| self.update(Message::Shell(intent))),
            Message::ToggleTheme => {
                self.mode = match self.mode {
                    ThemeMode::Dark => ThemeMode::Light,
                    ThemeMode::Light => ThemeMode::Dark,
                };
                Task::none()
            }
            Message::InputChanged(value) => {
                self.input = value;
                Task::none()
            }
            Message::InputSubmitted => {
                self.action_count += 1;
                self.last_action = format!("Input submitted: {}", self.input);
                Task::none()
            }
            Message::Shell(intent) => {
                if !matches!(intent, view::Message::FocusAction(_)) {
                    self.action_count += 1;
                }
                match intent {
                    view::Message::FocusAction(id) => iced::widget::operation::focus(id),
                    view::Message::ToggleCubeSwitcher => {
                        self.shell.switcher_open = !self.shell.switcher_open;
                        self.shell.lock_confirmation = false;
                        self.last_action = "Cube switcher toggled".to_owned();
                        iced::widget::operation::focus(shell::CUBE_SWITCHER_ID)
                    }
                    view::Message::AskCubeLock => {
                        self.shell.switcher_open = true;
                        self.shell.lock_confirmation = true;
                        self.last_action = "Lock review opened".to_owned();
                        iced::widget::operation::focus(shell::CUBE_LOCK_CANCEL_ID)
                    }
                    view::Message::CancelCubeLock => {
                        self.shell.lock_confirmation = false;
                        self.last_action = "Lock review cancelled".to_owned();
                        iced::widget::operation::focus(shell::CUBE_LOCK_ID)
                    }
                    view::Message::Menu(menu) => {
                        self.last_action = format!("Route: {menu:?}");
                        self.menu = menu;
                        Task::none()
                    }
                    view::Message::OpenLauncher => {
                        self.last_action = "Launcher return requested (synthetic)".to_owned();
                        Task::none()
                    }
                    view::Message::OpenConnectSignIn => {
                        self.last_action = "Connect sign-in requested (synthetic)".to_owned();
                        Task::none()
                    }
                    view::Message::ToggleTheme => self.update(Message::ToggleTheme),
                    view::Message::LockCubeToLauncher => {
                        self.last_action = "Lock confirmed (synthetic; no Cube exists)".to_owned();
                        self.shell.lock_confirmation = false;
                        self.shell.switcher_open = false;
                        Task::none()
                    }
                    other => {
                        self.last_action = format!("Shell intent: {other:?}");
                        Task::none()
                    }
                }
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let ctx = NavContext {
            has_vault: true,
            has_imported_bitcoin: false,
            has_p2p: false,
            network: ChainId::Bitcoin,
            p2p_test_coordinator: false,
            marketplace_flags: MarketplaceServerFlags::OFF,
            cube_name: "Synthetic QA Cube",
            spark_alias: Some("Daily spending"),
            vault_alias: Some("Family savings"),
            lightning_address: None,
            avatar: None,
            theme_mode: self.mode,
            connect_authenticated: false,
            connect_email: None,
            connect_stream_status: &self.stream_status,
            connect: Default::default(),
            btcb2_server_enabled: false,
            btcb2_already_claimed: false,
        };
        let sidebar = shell::sidebar(&self.menu, &ctx, self.shell);
        let top = shell::top_bar(&ctx).map(Message::Shell);
        let field = text_input("Type here; Enter submits only this field", &self.input)
            .on_input(Message::InputChanged)
            .on_submit(Message::InputSubmitted)
            .id("tenshu.fixture.input");
        let content = column![
            text("Keyboard proof fixture").size(26),
            text("Production shell widgets; synthetic state and intents only."),
            text(&self.last_action),
            text(format!("Action count: {}", self.action_count)),
            text("Tab and Shift-Tab move focus. Escape closes the innermost shell review."),
            text("Toggle theme to inspect both focus-ring palettes."),
            action::Action::new(
                "tenshu.fixture.theme",
                text("Toggle theme"),
                view::Message::FocusAction
            )
            .on_press(view::Message::ToggleTheme),
        ]
        .spacing(20)
        .padding(28)
        .width(Length::Fill);
        column![
            top,
            Element::from(ShellSplit::new(sidebar, content.into())).map(Message::Shell),
            container(field)
                .padding([8, 16])
                .height(Length::Fixed(52.0)),
        ]
        .height(Length::Fill)
        .into()
    }

    fn theme(&self) -> theme::Theme {
        theme::Theme::from_mode(self.mode)
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::event::listen_with(|event, status, _| match (event, status) {
            (
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Tab),
                    modifiers,
                    ..
                }),
                event::Status::Ignored,
            ) => Some(Message::Tab(modifiers.shift())),
            (
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    repeat: false,
                    ..
                }),
                event::Status::Ignored,
            ) => Some(Message::Escape),
            _ => None,
        })
    }
}

fn main() -> iced::Result {
    let app = iced::application(
        || (Fixture::default(), Task::none()),
        Fixture::update,
        Fixture::view,
    )
    .title("COINCUBE Desktop keyboard fixture · synthetic data")
    .theme(Fixture::theme)
    .subscription(Fixture::subscription)
    .window(iced::window::Settings {
        size: Size::new(1024.0, 768.0),
        min_size: Some(Size::new(1024.0, 768.0)),
        ..Default::default()
    });
    font::load()
        .into_iter()
        .fold(app, |app, font| app.font(font))
        .run()
}
