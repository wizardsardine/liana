use iced::{Length, Subscription, Task};
use iced_aw::ContextMenu;
use liana_ui::{component::text::*, icon::plus_icon, theme, widget::*};
use std::marker::PhantomData;

use crate::{
    app::{self, settings::SettingsTrait},
    gui::Config,
    installer::{self},
};

use super::tab;

#[derive(Debug)]
pub enum Message<M>
where
    M: Clone + Send + 'static,
{
    Tab(usize, tab::Message<M>),
    View(ViewMessage),
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
    FocusTab(usize),
    CloseTab(usize),
    SplitTab(usize),
    AddTab,
}

pub struct Pane<I, S, M>
where
    I: for<'a> installer::Installer<'a, M>,
    S: SettingsTrait,
    M: Clone + Send + 'static,
{
    pub tabs: Vec<tab::Tab<I, S, M>>,

    // this is an index in the tabs array
    pub focused_tab: usize,

    // used to generate tabs ids.
    tabs_created: usize,
    _phantom: PhantomData<(S, M)>,
}

impl<I, S, M> Pane<I, S, M>
where
    M: Clone + Send + 'static,
    I: for<'a> installer::Installer<'a, M>,
    S: SettingsTrait,
{
    pub fn new(cfg: &Config) -> (Self, Task<Message<M>>) {
        let (state, task) = tab::State::<I, S, M>::new(cfg.liana_directory.clone(), cfg.network);
        (
            Self {
                tabs: vec![tab::Tab::new(1, state)],
                focused_tab: 0,
                tabs_created: 1,
                _phantom: PhantomData,
            },
            task.map(|msg| Message::Tab(1, msg)),
        )
    }

    pub fn new_with_tab(s: tab::State<I, S, M>) -> Self {
        Self {
            tabs: vec![tab::Tab::new(1, s)],
            focused_tab: 0,
            tabs_created: 1,
            _phantom: PhantomData,
        }
    }

    fn add_tab(&mut self, cfg: &Config) -> Task<Message<M>> {
        let (state, task) = tab::State::<I, S, M>::new(cfg.liana_directory.clone(), cfg.network);
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

    pub fn remove_tab(&mut self, i: usize) -> Option<tab::Tab<I, S, M>> {
        if i >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(i);

        if self.focused_tab >= i {
            self.focused_tab = self.focused_tab.saturating_sub(1);
        }

        Some(tab)
    }

    pub fn add_tabs(&mut self, tabs: Vec<tab::Tab<I, S, M>>, focused_tab: usize) {
        for mut tab in tabs {
            self.tabs_created += 1;
            let id = self.tabs_created;
            tab.id = id;
            self.tabs.push(tab);
        }
        if self.focused_tab + focused_tab + 1 < self.tabs.len() {
            self.focused_tab += focused_tab + 1;
        }
    }

    pub fn on_tick(&mut self) -> Task<Message<M>> {
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
    ) -> Task<Message<M>> {
        self.update(
            Message::Tab(tab_id, tab::Message::Run(Box::new(app_msg.into()))),
            cfg,
        )
    }

    pub fn update(&mut self, message: Message<M>, cfg: &Config) -> Task<Message<M>> {
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

    pub fn subscription(&self) -> Subscription<Message<M>> {
        let subs: Vec<Subscription<Message<M>>> = self
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

    pub fn tabs_menu_view(&self) -> Element<Message<M>> {
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

    pub fn view(&self) -> Element<Message<M>> {
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
