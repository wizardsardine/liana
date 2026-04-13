use coincube_ui::{
    color,
    component::text::*,
    icon::{cross_icon, plus_icon},
    theme,
    widget::*,
};
use iced::{Length, Subscription, Task};
use iced_aw::ContextMenu;

use crate::{app, gui::Config};

use super::tab;

#[derive(Debug)]
pub enum Message {
    Tab(usize, tab::Message),
    View(ViewMessage),
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
    FocusTab(usize),
    CloseTab(usize),
    SplitTab(usize),
    AddTab,
    ToggleTheme,
}

pub struct Pane {
    pub tabs: Vec<tab::Tab>,

    // this is an index in the tabs array
    pub focused_tab: usize,

    // used to generate tabs ids.
    tabs_created: usize,

    // current theme mode — applied to new tabs on creation
    theme_mode: coincube_ui::theme::palette::ThemeMode,
}

impl Pane {
    pub fn new(cfg: &Config) -> (Self, Task<Message>) {
        let (state, task) = tab::State::new(cfg.coincube_directory.clone(), cfg.network);
        (
            Self {
                tabs: vec![tab::Tab::new(1, state)],
                focused_tab: 0,
                tabs_created: 1,
                theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
            },
            task.map(|msg| Message::Tab(1, msg)),
        )
    }

    pub fn new_with_tab(s: tab::State) -> Self {
        Self {
            tabs: vec![tab::Tab::new(1, s)],
            focused_tab: 0,
            tabs_created: 1,
            theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
        }
    }

    fn add_tab(&mut self, cfg: &Config) -> Task<Message> {
        let (state, task) = tab::State::new(cfg.coincube_directory.clone(), cfg.network);
        self.tabs_created += 1;
        let id = self.tabs_created;
        let mut tab = tab::Tab::new(id, state);
        tab.set_theme_mode(self.theme_mode);
        self.tabs.push(tab);
        self.focused_tab = self.tabs.len() - 1;
        task.map(move |msg| Message::Tab(id, msg))
    }

    pub fn close_tab(&mut self, i: usize) {
        if let Some(mut tab) = self.remove_tab(i) {
            tab.stop();
        }
    }

    pub fn remove_tab(&mut self, i: usize) -> Option<tab::Tab> {
        if i >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(i);

        if self.focused_tab >= i {
            self.focused_tab = self.focused_tab.saturating_sub(1);
        }

        Some(tab)
    }

    pub fn add_tabs(&mut self, tabs: Vec<tab::Tab>, focused_tab: usize) {
        for tab in tabs {
            self.tabs_created += 1;
            let id = self.tabs_created;
            let mut new_tab = tab::Tab::new(id, tab.state);
            new_tab.set_theme_mode(self.theme_mode);
            self.tabs.push(new_tab);
        }
        if self.focused_tab + focused_tab + 1 < self.tabs.len() {
            self.focused_tab += focused_tab + 1;
        }
    }

    pub fn set_theme_mode(&mut self, mode: coincube_ui::theme::palette::ThemeMode) {
        self.theme_mode = mode;
        for tab in &mut self.tabs {
            tab.set_theme_mode(mode);
        }
    }

    pub fn on_tick(&mut self) -> Task<Message> {
        Task::batch(self.tabs.iter_mut().map(|t| {
            let id = t.id;
            t.on_tick().map(move |msg| Message::Tab(id, msg))
        }))
    }

    /// Helper to update a tab with an app message.
    pub fn update_tab_with_app_msg(
        &mut self,
        tab_id: usize,
        app_msg: impl Into<app::Message>,
        cfg: &Config,
    ) -> Task<Message> {
        self.update(Message::Tab(tab_id, tab::Message::Run(app_msg.into())), cfg)
    }

    pub fn update(&mut self, message: Message, cfg: &Config) -> Task<Message> {
        match message {
            Message::Tab(id, msg) => {
                return self
                    .tabs
                    .iter_mut()
                    .find(|t| t.id == id)
                    .map(|t| {
                        t.update(msg).then(move |msg| match msg {
                            // Bubble ToggleTheme up to pane level as a ViewMessage
                            tab::Message::ToggleTheme => {
                                Task::done(Message::View(ViewMessage::ToggleTheme))
                            }
                            other => Task::done(Message::Tab(id, other)),
                        })
                    })
                    .unwrap_or(Task::none());
            }
            Message::View(ViewMessage::FocusTab(i)) => {
                if i < self.tabs.len() {
                    self.focused_tab = i;
                }
            }
            Message::View(ViewMessage::AddTab) => return self.add_tab(cfg),
            Message::View(ViewMessage::CloseTab(i)) => {
                self.close_tab(i);
            }
            // handle by the pane grid update.
            Message::View(ViewMessage::SplitTab(_)) => {}
            // handled at the GUI level
            Message::View(ViewMessage::ToggleTheme) => {}
        }

        Task::none()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let subs: Vec<Subscription<Message>> = self
            .tabs
            .iter()
            .map(|t| {
                t.subscription()
                    .with(t.id)
                    .map(|(id, msg)| Message::Tab(id, msg))
            })
            .collect();
        Subscription::batch(subs)
    }

    pub fn stop(&mut self) {
        self.tabs.iter_mut().for_each(|t| t.stop());
    }

    pub fn tabs_menu_view(&self) -> Element<Message> {
        let mut menu = Row::new().spacing(3);
        let tabs_len = self.tabs.len();
        for (i, tab) in self.tabs.iter().enumerate() {
            let title = tab.title();
            let title_row = if title.len() < 20 {
                Row::new().push(p1_regular(title)).push(p1_regular(
                    "                     "[..21 - title.len()].to_string(),
                ))
            } else {
                Row::new()
                    .push(p1_regular(&title[..17]))
                    .push(p1_regular("..."))
            };

            let tab_content: Element<ViewMessage> = if tabs_len > 1 {
                // Show close button when more than one tab
                Row::new()
                    .align_y(iced::Alignment::Center)
                    .push(
                        Button::new(title_row)
                            .style(if i == self.focused_tab {
                                theme::button::tab_liquid
                            } else {
                                theme::button::tab
                            })
                            .on_press(ViewMessage::FocusTab(i)),
                    )
                    .push(
                        Button::new(
                            iced::widget::Container::new(cross_icon().size(14))
                                .center_x(iced::Length::Fill)
                                .center_y(iced::Length::Fill),
                        )
                        .width(iced::Length::Fixed(28.0))
                        .height(iced::Length::Fixed(28.0))
                        .style(|_: &coincube_ui::theme::Theme, status| {
                            iced::widget::button::Style {
                                background: Some(iced::Background::Color(color::TRANSPARENT)),
                                text_color: match status {
                                    iced::widget::button::Status::Hovered => color::ORANGE,
                                    _ => color::GREY_3,
                                },
                                ..Default::default()
                            }
                        })
                        .padding(0)
                        .on_press(ViewMessage::CloseTab(i)),
                    )
                    .into()
            } else {
                // Single tab — no close button
                Button::new(title_row)
                    .style(if i == self.focused_tab {
                        theme::button::tab_liquid
                    } else {
                        theme::button::tab
                    })
                    .on_press(ViewMessage::FocusTab(i))
                    .into()
            };

            menu = menu.push(ContextMenu::new(tab_content, move || {
                Column::new()
                    .push(
                        Button::new(p1_regular("Close"))
                            .style(theme::button::secondary)
                            .on_press(ViewMessage::CloseTab(i))
                            .width(100),
                    )
                    .push(if tabs_len > 1 {
                        Some(
                            Button::new(p1_regular("Split"))
                                .style(theme::button::secondary)
                                .on_press(ViewMessage::SplitTab(i))
                                .width(100),
                        )
                    } else {
                        None
                    })
                    .into()
            }));
        }
        menu = menu.push(
            Button::new(plus_icon())
                .style(theme::button::tab)
                .on_press(ViewMessage::AddTab),
        );

        let menu: Element<ViewMessage> = menu.wrap().into();
        menu.map(Message::View)
    }

    pub fn view(&self) -> Element<Message> {
        Container::new(if let Some(t) = self.tabs.get(self.focused_tab) {
            let id = t.id;
            t.view().map(move |msg| Message::Tab(id, msg))
        } else {
            Row::new().into()
        })
        .style(theme::container::background)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}
