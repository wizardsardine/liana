use iced::{Length, Subscription, Task};
use iced_aw::ContextMenu;
use liana_ui::{component::text::*, icon::plus_icon, theme, widget::*};
use std::time::Instant;

use crate::gui::Config;

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
}

pub struct Pane {
    pub tabs: Vec<tab::Tab>,

    // this is an index in the tabs array
    pub focused_tab: usize,

    // used to generate tabs ids.
    tabs_created: usize,
}

impl Pane {
    pub fn new(cfg: &Config) -> (Self, Task<Message>) {
        let (state, task) = tab::State::new(cfg.liana_directory.clone(), cfg.network);
        (
            Self {
                tabs: vec![tab::Tab::new(1, state)],
                focused_tab: 0,
                tabs_created: 1,
            },
            task.map(|msg| Message::Tab(1, msg)),
        )
    }

    pub fn new_with_tab(s: tab::State) -> Self {
        Self {
            tabs: vec![tab::Tab::new(1, s)],
            focused_tab: 0,
            tabs_created: 1,
        }
    }

    fn add_tab(&mut self, cfg: &Config) -> Task<Message> {
        let (state, task) = tab::State::new(cfg.liana_directory.clone(), cfg.network);
        self.tabs_created += 1;
        let id = self.tabs_created;
        self.tabs.push(tab::Tab::new(id, state));
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
            self.tabs.push(tab::Tab::new(id, tab.state));
        }
        if self.focused_tab + focused_tab + 1 < self.tabs.len() {
            self.focused_tab += focused_tab + 1;
        }
    }

    pub fn on_tick(&mut self, i: Instant) -> Task<Message> {
        Task::batch(self.tabs.iter_mut().map(|t| {
            let id = t.id;
            t.on_tick(i).map(move |msg| Message::Tab(id, msg))
        }))
    }

    pub fn update(&mut self, message: Message, cfg: &Config) -> Task<Message> {
        match message {
            Message::Tab(id, msg) => self
                .tabs
                .iter_mut()
                .find(|t| t.id == id)
                .map(|t| t.update(msg).map(move |msg| Message::Tab(id, msg)))
                .unwrap_or(Task::none()),
            Message::View(ViewMessage::FocusTab(i)) => {
                if i < self.tabs.len() {
                    self.focused_tab = i;
                }
                Task::none()
            }
            Message::View(ViewMessage::AddTab) => self.add_tab(cfg),
            Message::View(ViewMessage::CloseTab(i)) => {
                self.close_tab(i);
                Task::none()
            }
            // handle by the pane grid update.
            Message::View(ViewMessage::SplitTab(_)) => Task::none(),
        }
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
            menu = menu.push(ContextMenu::new(
                Into::<Element<ViewMessage>>::into(
                    Button::new(if title.len() < 20 {
                        Row::new().push(p1_regular(title)).push(p1_regular(
                            &"                     ".to_string()[..21 - title.len()],
                        ))
                    } else {
                        Row::new()
                            .push(p1_regular(&title[..17]))
                            .push(p1_regular("..."))
                    })
                    .style(if i == self.focused_tab {
                        theme::button::tab_active
                    } else {
                        theme::button::tab
                    })
                    .on_press(ViewMessage::FocusTab(i)),
                ),
                move || {
                    Column::new()
                        .push(
                            Button::new(p1_regular("Close"))
                                .style(theme::button::secondary)
                                .on_press(ViewMessage::CloseTab(i))
                                .width(100),
                        )
                        .push_maybe(if tabs_len > 1 {
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
                },
            ));
        }
        menu = menu.push(
            Button::new(plus_icon())
                .style(theme::button::tab)
                .on_press(ViewMessage::AddTab),
        );
        Into::<Element<ViewMessage>>::into(menu.wrap()).map(Message::View)
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
