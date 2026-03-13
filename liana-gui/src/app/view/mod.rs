mod label;
mod message;
mod warning;

pub mod coins;
pub mod export;
pub mod fiat;
pub mod home;
pub mod hw;
pub mod psbt;
pub mod psbts;
pub mod receive;
pub mod recovery;
pub mod settings;
pub mod spend;
pub mod transactions;

pub use fiat::FiatAmountConverter;
pub use message::*;
use warning::warn;

use iced::{
    widget::{column, responsive, row, scrollable, Space},
    Length,
};

use liana_ui::{
    component::{button, text::*},
    icon::cross_icon,
    image::*,
    theme,
    widget::*,
};

use crate::app::{cache::Cache, error::Error, menu::Menu};

use std::cell::RefCell;

const SIDEBAR_LARGE_WIDTH: f32 = 200.0;
const SIDEBAR_SMALL_WIDTH: f32 = 60.0;
const PANE_WIDTH_THRESHOLD: f32 = 900.0;

pub fn sidebar<'a>(active: &Menu, cache: &'a Cache, small: bool) -> Container<'a, Message> {
    Container::new(
        Column::new()
            .push(
                Column::new()
                    .push(
                        Container::new(
                            liana_grey_logo()
                                .height(Length::Fixed(120.0))
                                .width(Length::Fixed(60.0))
                                .style(theme::svg::accent),
                        )
                        .padding(10),
                    )
                    .push(Menu::Home.entry(active, small))
                    .push(Menu::CreateSpendTx.entry(active, small))
                    .push(Menu::Receive.entry(active, small))
                    .push(Menu::Coins.entry(active, small))
                    .push(Menu::Transactions.entry(active, small))
                    .push(Menu::PSBTs.entry(active, small))
                    .height(Length::Fill),
            )
            .push(
                Container::new(
                    Column::new()
                        .spacing(10)
                        .push_maybe(cache.rescan_progress().map(|p| {
                            Container::new(text(format!("  Rescan...{:.2}%  ", p * 100.0)))
                                .padding(5)
                                .style(theme::pill::simple)
                        }))
                        .push(Menu::Recovery.entry(active, small))
                        .push(Menu::Settings.entry(active, small)),
                )
                .height(Length::Shrink),
            ),
    )
    .style(theme::container::foreground)
}

pub fn dashboard<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    warning: Option<&'a Error>,
    content: T,
) -> Element<'a, Message> {
    let content_cell = RefCell::new(Some(content.into()));
    responsive(move |size| {
        let small = size.width < PANE_WIDTH_THRESHOLD;
        let sidebar_width = if small {
            SIDEBAR_SMALL_WIDTH
        } else {
            SIDEBAR_LARGE_WIDTH
        };
        let content = content_cell
            .borrow_mut()
            .take()
            .unwrap_or_else(|| Space::new(Length::Fill, Length::Fill).into());
        Row::new()
            .push(
                sidebar(menu, cache, small)
                    .height(Length::Fill)
                    .width(Length::Fixed(sidebar_width)),
            )
            .push(
                Column::new()
                    .push(warn(warning))
                    .push(
                        Container::new(
                            scrollable(row!(
                                Space::with_width(Length::FillPortion(1)),
                                column!(Space::with_height(Length::Fixed(150.0)), content)
                                    .width(Length::FillPortion(8))
                                    .max_width(1500),
                                Space::with_width(Length::FillPortion(1)),
                            ))
                            .on_scroll(|w| Message::Scroll(w.absolute_offset().y)),
                        )
                        .center_x(Length::Fill)
                        .style(theme::container::background)
                        .height(Length::Fill),
                    )
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    })
    .into()
}

pub fn modal<'a, T: Into<Element<'a, Message>>, F: Into<Element<'a, Message>>>(
    is_previous: bool,
    warning: Option<&Error>,
    content: T,
    fixed_footer: Option<F>,
) -> Element<'a, Message> {
    Column::new()
        .push(warn(warning))
        .push(
            Container::new(
                Row::new()
                    .push(if is_previous {
                        Column::new()
                            .push(
                                button::transparent(None, "< Previous").on_press(Message::Previous),
                            )
                            .width(Length::Fill)
                    } else {
                        Column::new().width(Length::Fill)
                    })
                    .align_y(iced::Alignment::Center)
                    .push(button::secondary(Some(cross_icon()), "Close").on_press(Message::Close)),
            )
            .padding(10)
            .style(theme::container::background),
        )
        .push(modal_section(Container::new(scrollable(content))))
        .push_maybe(fixed_footer)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn modal_section<'a, T: 'a>(menu: Container<'a, T>) -> Container<'a, T> {
    Container::new(menu.max_width(1500))
        .style(theme::container::background)
        .center_x(Length::Fill)
        .width(Length::Fill)
        .height(Length::Fill)
}
