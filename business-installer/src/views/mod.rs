pub mod keys;
pub mod login;
pub mod modals;
pub mod org_select;
pub mod paths;
pub mod template_builder;
pub mod wallet_select;
pub mod xpub;

pub use keys::keys_view;
pub use login::login_view;
pub use org_select::org_select_view;
pub use template_builder::template_builder_view;
pub use wallet_select::wallet_select_view;
pub use xpub::xpub_view;

use crate::state::message::Msg;
use iced::{
    widget::{container, scrollable, Space},
    Alignment, Length,
};
use liana_ui::{
    component::{button, text},
    icon, theme,
    widget::*,
};

const EMAIL_HEADER_SPACER: u16 = 30;

fn layout<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    role_badge: Option<&'static str>, // Show role badge before email (e.g., "Manager" for WSManager)
    title: &'static str,
    content: impl Into<Element<'a, Msg>>,
    padding_left: bool,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    // Build the left button
    // If previous_message is provided, show "Previous" button
    // Otherwise, if authenticated show "Logout", else show disabled "Previous"
    let left_button = if let Some(msg) = previous_message {
        button::transparent(Some(icon::previous_icon()), "Previous").on_press(msg)
    } else if email.is_some() {
        button::transparent(Some(icon::previous_icon()), "Logout").on_press(Msg::Logout)
    } else {
        button::transparent(Some(icon::previous_icon()), "Previous")
    };

    // Build the top-right row with optional role badge and email
    let mut email_row = Row::new()
        .push(Space::with_width(Length::Fill))
        .spacing(10)
        .align_y(Alignment::Center);

    // Add role badge if provided (shown before email)
    if let Some(role) = role_badge {
        email_row = email_row.push(
            Container::new(text::caption(role))
                .padding([4, 12])
                .style(theme::pill::simple),
        );
    }

    // Add email if provided
    if let Some(e) = email {
        email_row = email_row
            .push(Container::new(text::p1_regular(e).style(theme::text::success)).padding(20));
    }
    let header = Row::new()
        .align_y(Alignment::Center)
        .push(Container::new(left_button).center_x(Length::FillPortion(2)))
        .push(Container::new(text::h3(title)).width(Length::FillPortion(8)))
        .push_maybe(if progress.1 > 0 {
            Some(
                Container::new(text::text(format!("{} | {}", progress.0, progress.1)))
                    .center_x(Length::FillPortion(2)),
            )
        } else {
            None
        });
    let content = Row::new()
        .push(Space::with_width(Length::FillPortion(2)))
        .push(
            Container::new(
                Column::new()
                    .push(Space::with_height(Length::Fixed(100.0)))
                    .push(content),
            )
            .width(Length::FillPortion(if padding_left { 8 } else { 10 })),
        )
        .push_maybe(if padding_left {
            Some(Space::with_width(Length::FillPortion(2)))
        } else {
            None
        });
    Container::new(scrollable(
        Column::new()
            .width(Length::Fill)
            .push(email_row)
            .push(Space::with_height(EMAIL_HEADER_SPACER + 60))
            .push(header)
            .push(content),
    ))
    .center_x(Length::Fill)
    .height(Length::Fill)
    .width(Length::Fill)
    .style(theme::container::background)
    .into()
}

/// Layout variant with fixed header content and a scrollable list section.
/// The header_content stays fixed at top, only the list_content scrolls.
/// An optional footer_content can be placed below the scrollable area.
fn layout_with_scrollable_list<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    role_badge: Option<&'static str>,
    title: &'static str,
    header_content: impl Into<Element<'a, Msg>>,
    list_content: impl Into<Element<'a, Msg>>,
    footer_content: Option<Element<'a, Msg>>,
    padding_left: bool,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    // Build the left button
    let left_button = if let Some(msg) = previous_message {
        button::transparent(Some(icon::previous_icon()), "Previous").on_press(msg)
    } else if email.is_some() {
        button::transparent(Some(icon::previous_icon()), "Logout").on_press(Msg::Logout)
    } else {
        button::transparent(Some(icon::previous_icon()), "Previous")
    };

    // Build the top-right row with optional role badge and email
    let mut email_row = Row::new()
        .push(Space::with_width(Length::Fill))
        .spacing(10)
        .align_y(Alignment::Center);

    if let Some(role) = role_badge {
        email_row = email_row.push(
            Container::new(text::caption(role))
                .padding([4, 12])
                .style(theme::pill::simple),
        );
    }

    if let Some(e) = email {
        email_row = email_row
            .push(Container::new(text::p1_regular(e).style(theme::text::success)).padding(20));
    }

    let header = Row::new()
        .align_y(Alignment::Center)
        .push(Container::new(left_button).center_x(Length::FillPortion(2)))
        .push(Container::new(text::h3(title)).width(Length::FillPortion(8)))
        .push_maybe(if progress.1 > 0 {
            Some(
                Container::new(text::text(format!("{} | {}", progress.0, progress.1)))
                    .center_x(Length::FillPortion(2)),
            )
        } else {
            None
        });

    // Fixed header content area (title, search, filters)
    let header_area = Row::new()
        .push(Space::with_width(Length::FillPortion(2)))
        .push(
            Container::new(header_content).width(Length::FillPortion(if padding_left {
                8
            } else {
                10
            })),
        )
        .push_maybe(if padding_left {
            Some(Space::with_width(Length::FillPortion(2)))
        } else {
            None
        });

    // Scrollable list area
    let list_area = Row::new()
        .push(Space::with_width(Length::FillPortion(2)))
        .push(
            Container::new(scrollable(list_content).height(Length::Fill))
                .width(Length::FillPortion(if padding_left { 8 } else { 10 }))
                .align_x(Alignment::Center),
        )
        .push_maybe(if padding_left {
            Some(Space::with_width(Length::FillPortion(2)))
        } else {
            None
        })
        .height(Length::Fill);

    // Optional footer area (fixed at bottom, outside scrollable)
    let footer_area: Option<Row<'a, Msg>> = footer_content.map(|content| {
        Row::new()
            .push(Space::with_width(Length::FillPortion(2)))
            .push(
                Container::new(content).width(Length::FillPortion(if padding_left { 8 } else { 10 })),
            )
            .push_maybe(if padding_left {
                Some(Space::with_width(Length::FillPortion(2)))
            } else {
                None
            })
    });

    Container::new(
        Column::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(email_row)
            .push(Space::with_height(EMAIL_HEADER_SPACER))
            .push(header)
            .push(header_area)
            .push(list_area)
            .push_maybe(footer_area),
    )
    .center_x(Length::Fill)
    .height(Length::Fill)
    .width(Length::Fill)
    .style(theme::container::background)
    .into()
}

pub fn menu_entry(content: Element<'_, Msg>, message: Option<Msg>) -> Element<'_, Msg> {
    Container::new(
        Button::new(
            container(content)
                .align_y(Alignment::Center)
                .align_x(Alignment::Center)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .on_press_maybe(message)
        .padding(15)
        .style(theme::button::container_border),
    )
    .style(theme::card::simple)
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .width(500)
    .height(80)
    .into()
}
