mod section;

use iced::widget::{button, column, container, row, text, Space};
use iced::{executor, Application, Command, Length, Settings, Subscription};
use liana_ui::{theme, widget::*};

pub fn main() -> iced::Result {
    let mut settings = Settings::with_flags(Config {});
    settings.default_text_size = 19;
    DesignSystem::run(settings)
}

struct Config {}

#[derive(Default)]
struct DesignSystem {
    theme: theme::Theme,
    sections: Vec<Box<dyn Section>>,
    current: usize,
}

#[derive(Debug, Clone)]
pub enum Message {
    Event(iced_native::Event),
    Section(usize),
    Ignore,
}

impl Application for DesignSystem {
    type Message = Message;
    type Theme = theme::Theme;
    type Flags = Config;
    type Executor = executor::Default;

    fn title(&self) -> String {
        String::from("Liana - Design System")
    }

    fn new(_config: Config) -> (Self, Command<Self::Message>) {
        let app = Self {
            theme: theme::Theme::Dark,
            sections: vec![
                Box::new(section::Overview {}),
                Box::new(section::Colors {}),
                Box::new(section::Typography {}),
                Box::new(section::Buttons {}),
                Box::new(section::HardwareWallets {}),
            ],
            current: 0,
        };
        #[cfg(target_arch = "wasm32")]
        {
            use iced_native::{command, window};
            let window = web_sys::window().unwrap();
            let (width, height) = (
                (window.inner_width().unwrap().as_f64().unwrap()) as u32,
                (window.inner_height().unwrap().as_f64().unwrap()) as u32,
            );
            (
                app,
                Command::single(command::Action::Window(window::Action::Resize {
                    width,
                    height,
                })),
            )
        }
        #[cfg(not(target_arch = "wasm32"))]
        (app, Command::none())
    }

    fn update(&mut self, message: Message) -> Command<Self::Message> {
        match message {
            Message::Section(i) => {
                if self.sections.get(i).is_some() {
                    self.current = i;
                }
            }
            Message::Event(iced::Event::Window(iced_native::window::Event::Resized {
                width,
                height,
            })) => {
                #[cfg(target_arch = "wasm32")]
                {
                    use iced_native::{command, window};
                    return Command::single(command::Action::Window(window::Action::Resize {
                        width,
                        height,
                    }));
                }
            }
            _ => {}
        }
        Command::none()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        iced_native::subscription::events().map(Self::Message::Event)
    }

    fn view(&self) -> Element<Message> {
        let sidebar = container(
            column![
                Space::with_height(Length::Units(100)),
                self.sections.iter().enumerate().fold(
                    Column::new().spacing(10),
                    |col, (i, section)| col.push(
                        button(
                            container(text(section.title()))
                                .width(Length::Fill)
                                .padding(5)
                        )
                        .style(if i == self.current {
                            theme::Button::Primary
                        } else {
                            theme::Button::Transparent
                        })
                        .on_press(Message::Section(i))
                        .width(Length::Units(200))
                    )
                )
            ]
            .spacing(20),
        )
        .padding(20)
        .style(theme::Container::Foreground)
        .height(Length::Fill);

        container(row![
            sidebar.width(Length::Units(200)),
            Space::with_width(Length::Units(150)),
            column![
                Space::with_height(Length::Units(150)),
                container(self.sections[self.current].view()).width(Length::Fill)
            ]
        ])
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn theme(&self) -> theme::Theme {
        self.theme.clone()
    }
}

pub trait Section {
    fn title(&self) -> &'static str;
    fn view(&self) -> Element<Message>;
}
