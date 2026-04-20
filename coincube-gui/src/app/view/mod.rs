mod message;

pub mod buysell;
pub mod connect;
pub mod global_home;
pub mod liquid;
pub mod nav;
pub mod p2p;
pub mod settings;
pub mod spark;

pub mod vault;

use std::iter::FromIterator;

pub use liquid::*;
pub use message::*;
pub use spark::{
    SparkOverviewMessage, SparkOverviewView, SparkReceiveMessage, SparkReceiveView,
    SparkSendMessage, SparkSendView, SparkSettingsMessage, SparkSettingsStatus, SparkSettingsView,
    SparkStatus, SparkTransactionsMessage, SparkTransactionsStatus, SparkTransactionsView,
};
pub use vault::fiat::FiatAmountConverter;
pub use vault::warning::warn;

use iced::{
    widget::{column, container, row, scrollable, Space},
    Alignment, Length,
};

use coincube_ui::{
    color,
    component::{button, text, text::*},
    icon::cross_icon,
    theme,
    widget::*,
};

use crate::app::{cache::Cache, menu::Menu};

/// Simple toast notification for clipboard copy and other success messages
pub fn simple_toast(message: &str) -> Container<Message> {
    container(text::p2_regular(message))
        .padding(15)
        .style(theme::notification::success)
        .max_width(400.0)
}

/// Wraps `content` in the shared balance card style used across wallet overview and send screens
/// (themed card background, orange border, rounded corners).
pub fn balance_header_card<'a, Msg: 'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content)
        .padding(20)
        .width(Length::Fill)
        .style(theme::container::balance_header)
        .into()
}

/// A compact "master seed not backed up" warning strip, rendered at the
/// top of every page by the `dashboard` wrapper when
/// `cache.current_cube_backed_up` is false and the user hasn't dismissed
/// it this session.
///
/// Constrained to the same width as the main content column so it lines
/// up with the page content below it. Clicking "Back Up Now" routes to
/// General Settings; clicking the × dismisses for the session — it
/// returns on app restart until the user actually backs up.
pub fn backup_warning_banner<'a>() -> Element<'a, Message> {
    let body = container(
        row![
            coincube_ui::icon::warning_icon().color(color::BLACK),
            text::p2_regular(
                "Your master seed phrase is not backed up. Back it up to avoid \
                 losing access to your Cube."
            )
            .color(color::BLACK),
            Space::new().width(Length::Fill),
            button::secondary(None, "Back Up Now")
                .padding([6, 14])
                .width(Length::Fixed(140.0))
                .on_press(Message::Menu(Menu::Home(
                    crate::app::menu::HomeSubMenu::Settings(
                        crate::app::menu::HomeSettingsOption::General,
                    ),
                ))),
            iced::widget::Button::new(
                cross_icon()
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            )
            .padding([8, 10])
            .style(theme::button::secondary)
            .on_press(Message::DismissBackupWarning),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([8, 16])
    .width(Length::Fill)
    .style(theme::notification::warning);

    // Constrain to the same FillPortion(1/8/1) layout used by the
    // dashboard content column so the banner lines up horizontally.
    container(row![
        Space::new().width(Length::FillPortion(1)),
        container(body)
            .width(Length::FillPortion(8))
            .max_width(1500),
        Space::new().width(Length::FillPortion(1)),
    ])
    .padding([8, 0])
    .width(Length::Fill)
    .into()
}

pub fn dashboard<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    content: T,
) -> Element<'a, Message> {
    dashboard_with_info(
        menu,
        cache,
        content,
        &cache.cube_name,
        None,
        cache.lightning_address.as_deref(),
    )
}

pub fn dashboard_with_info<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    content: T,
    cube_name: &'a str,
    avatar_handle: Option<&'a iced::widget::image::Handle>,
    lightning_address: Option<&'a str>,
) -> Element<'a, Message> {
    let has_vault = cache.has_vault;
    let has_p2p = cache.has_p2p;
    let show_backup_warning = !cache.current_cube_backed_up
        && !cache.current_cube_is_passkey
        && !cache.backup_warning_dismissed;
    let nav_ctx = nav::NavContext {
        has_vault,
        has_p2p,
        cube_name,
        lightning_address,
        avatar: avatar_handle,
        theme_mode: cache.theme_mode,
        connect_authenticated: cache.connect_authenticated,
    };
    let content_column: Element<'_, Message> = Column::new()
        .push(warn(None))
        .push_maybe(show_backup_warning.then(backup_warning_banner))
        .push(
            Container::new(
                scrollable(row!(
                    Space::new().width(Length::FillPortion(1)),
                    column!(Space::new().height(Length::Fixed(30.0)), content.into())
                        .width(Length::FillPortion(8))
                        .max_width(1500),
                    Space::new().width(Length::FillPortion(1)),
                ))
                .on_scroll(|w| Message::Scroll(w.absolute_offset().y)),
            )
            .center_x(Length::Fill)
            .style(theme::container::background)
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .into();

    // Tertiary rail is overlaid on the content area at the left edge,
    // y-aligned with the sidebar's rails row. The content layout stays
    // 100% fixed — when the tertiary rail opens it "slides out" on top
    // of the content's (usually empty) left padding without the content
    // column itself shifting right.
    let content_with_overlay: Element<'_, Message> = match nav::tertiary_rail(menu, &nav_ctx) {
        Some(tert) => {
            let overlay: Element<'_, Message> = Column::new()
                .push(Space::new().height(Length::Fixed(nav::TERTIARY_TOP_OFFSET)))
                .push(Row::new().push(tert).push(Space::new().width(Length::Fill)))
                .into();
            iced::widget::stack![content_column, overlay].into()
        }
        None => content_column,
    };

    Row::new()
        .push(nav::sidebar(menu, &nav_ctx))
        .push(content_with_overlay)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

pub fn modal<'a, T: Into<Element<'a, Message>>, F: Into<Element<'a, Message>>>(
    is_previous: bool,
    content: T,
    fixed_footer: Option<F>,
) -> Element<'a, Message> {
    Column::new()
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
        .push(fixed_footer)
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

pub fn placeholder<'a, T: Into<Element<'a, Message>>>(
    icon: T,
    title: &'a str,
    subtitle: &'a str,
) -> Element<'a, Message> {
    let content = Column::new()
        .push(icon)
        .push(text(title).style(theme::text::secondary).bold())
        .push(
            text(subtitle)
                .size(P2_SIZE)
                .style(theme::text::secondary)
                .align_x(Alignment::Center),
        )
        .spacing(16)
        .align_x(Alignment::Center);

    Container::new(content)
        .width(Length::Fill)
        .padding(60)
        .center_x(Length::Fill)
        .style(|t| container::Style {
            background: Some(iced::Background::Color(t.colors.cards.simple.background)),
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

pub fn toast_overlay<'a, I: Iterator<Item = (usize, log::Level, &'a str)>>(
    iter: I,
    theme: &coincube_ui::theme::Theme,
) -> coincube_ui::widget::Element<'a, Message> {
    use coincube_ui::{color, component::text, icon::cross_icon, theme::notification};

    // Color mapping for toast levels using the theme
    let toast = |id: usize, level: log::Level, content: &'a str| {
        let content_owned = content.to_string();
        const WIDGET_HEIGHT: u32 = 80;

        // Use theme palette for the toast background
        let palette = notification::palette_for_level(&level, theme);
        let bg_color = palette.background;
        let border_color = palette.border.unwrap_or(palette.background);
        let text_color = palette.text.unwrap_or(color::WHITE);

        let bg = iced::Background::Color(bg_color);
        let border = iced::Border {
            width: 1.0,
            color: border_color,
            radius: 25.0.into(),
        };

        let inner = iced::widget::row![
            container(text::p1_bold(content_owned).color(text_color))
                .width(600)
                .height(WIDGET_HEIGHT)
                .padding(15)
                .align_y(iced::Alignment::Center),
            iced::widget::Button::new(
                cross_icon()
                    .color(text_color)
                    .size(36)
                    .align_x(iced::Alignment::Center)
                    .align_y(iced::Alignment::Center)
                    .height(iced::Length::Fill)
            )
            .height(WIDGET_HEIGHT)
            .width(60)
            .style(move |_, status| {
                let base = iced::widget::button::Style::default();
                match status {
                    iced::widget::button::Status::Hovered => base.with_background(iced::Color {
                        a: 0.2,
                        ..color::BLACK
                    }),
                    _ => base,
                }
            })
            .on_press(Message::DismissToast(id))
        ];

        // Wrap the entire row in a single styled container so the close
        // button sits inside the rounded rectangle. clip(true) ensures
        // the hover highlight respects the border radius.
        container(inner)
            .style(move |_| {
                iced::widget::container::Style::default()
                    .background(bg)
                    .border(border)
            })
            .clip(true)
    };

    let centered = iced::widget::row![
        // offset the toast past the two-rail nav sidebar
        iced::widget::Space::new().width(nav::SIDEBAR_BASE_WIDTH),
        // center toasts horizontally
        iced::widget::Space::new().width(iced::Length::Fill),
        iced::widget::Column::from_iter(
            iter.map(|(id, level, content)| toast(id, level, content).into())
        )
        .spacing(10),
        iced::widget::Space::new().width(iced::Length::Fill),
    ];

    // full screen positioning
    let column = iced::widget::column![
        iced::widget::Space::new().height(iced::Length::Fill),
        centered,
        iced::widget::Space::new().height(25),
    ];

    container(column)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}
