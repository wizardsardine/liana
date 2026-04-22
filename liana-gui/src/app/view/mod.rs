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
    widget::{column, responsive, row, scrollable, stack, Space},
    Length,
};

use liana_ui::{
    component::{button, text::*},
    icon::cross_icon,
    image::*,
    theme,
    widget::*,
};

use crate::app::{
    cache::Cache,
    error::Error,
    menu::{Menu, MenuWidth},
};

pub fn sidebar<'a>(
    active: &Menu,
    cache: &'a Cache,
    menu_width: MenuWidth,
) -> Container<'a, Message> {
    let padding = match menu_width {
        MenuWidth::Normal => [0.0, 30.0],
        MenuWidth::Compact | MenuWidth::Small => [0.0, 5.0],
    };
    let logo = match (menu_width.is_small(), cache.variant) {
        (false, liana_ui::Variant::LianaBusiness) => liana_business_logo(),
        (false, liana_ui::Variant::Liana) => liana_wallet_logo(),
        (true, liana_ui::Variant::Liana) => liana_green_logo().width(60),
        (true, liana_ui::Variant::LianaBusiness) => liana_blue_logo().width(60),
    }
    .height(120);
    let upper_buttons = Column::new()
        .push(Space::with_height(10))
        .push(Container::new(logo))
        .push(Menu::Home.entry(active, menu_width))
        .push(Menu::CreateSpendTx.entry(active, menu_width))
        .push(Menu::Receive.entry(active, menu_width))
        .push(Menu::PSBTs.entry(active, menu_width))
        .height(Length::Fill)
        .spacing(10);

    let bottom_buttons = Container::new(
        Column::new()
            .spacing(10)
            .push_maybe(cache.rescan_progress().map(|p| {
                Container::new(text(format!("  Rescan...{:.2}%  ", p * 100.0)))
                    .padding(5)
                    .style(theme::pill::simple)
            }))
            .push(Menu::Recovery.entry(active, menu_width))
            .push(Menu::Transactions.entry(active, menu_width))
            .push(Menu::Coins.entry(active, menu_width))
            .push(Menu::Settings.entry(active, menu_width))
            .push(Space::with_height(10)),
    )
    .height(Length::Shrink);

    Container::new(Column::new().push(upper_buttons).push(bottom_buttons))
        .style(theme::container::sidebar)
        .padding(padding)
}

pub fn dashboard<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    warning: Option<&'a Error>,
    content: T,
) -> Element<'a, Message> {
    // The probe's closure fires during iced's layout pass, not synchronously
    // here, so `sidebar_width` below always reads the size cached by the
    // *previous* frame's layout. One-frame lag on resize is expected.
    let pane_size_cell = &cache.pane_size;
    let probe: Element<'a, Message> = responsive(move |size| {
        pane_size_cell.set(size);
        Space::new(Length::Fill, Length::Fill).into()
    })
    .into();

    let sidebar_width = MenuWidth::from_pane_width(cache.pane_size.get().width);

    let view: Element<'a, Message> = Row::new()
        .push(
            sidebar(menu, cache, sidebar_width)
                .height(Length::Fill)
                .width(Length::Fixed(sidebar_width.into())),
        )
        .push(
            Column::new()
                .push(warn(warning))
                .push(
                    Container::new(column![
                        Space::with_height(25),
                        Container::new(
                            scrollable(row!(
                                Space::with_width(Length::FillPortion(1)),
                                column!(Space::with_height(Length::Fixed(150.0)), content.into())
                                    .width(Length::FillPortion(8))
                                    .max_width(1500),
                                Space::with_width(Length::FillPortion(1)),
                            ))
                            .on_scroll(|w| Message::Scroll(w.absolute_offset().y)),
                        )
                        .center_x(Length::Fill)
                        .style(theme::container::panel_background)
                        .height(Length::Fill)
                    ])
                    .style(theme::container::sidebar),
                )
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into();

    stack![probe, view].into()
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
