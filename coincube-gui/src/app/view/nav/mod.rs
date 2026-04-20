//! Two-rail left navigation.
//!
//! - [`primary`] — compact 72px rail of top-level sections (Cube, Spark, Liquid, ...).
//! - [`secondary`] — matching 72px rail showing the current section's submenu.
//! - The avatar + cube name + Lightning Address header sits above both rails
//!   (see [`identity_block`]), so the first secondary-rail item lines up with
//!   the Cube item on the primary rail.
//! - Per-section submenu lists live in `home.rs`, `spark.rs`, `liquid.rs`, etc.
//!
//! See `plans/PLAN-two-rail-left-nav-redesign.md` for the full design rationale.

pub mod items;
pub mod primary;
pub mod secondary;

pub mod connect;
pub mod home;
pub mod liquid;
pub mod marketplace;
pub mod spark;
pub mod tertiary;
pub mod vault;

pub use items::SubItem;

use crate::app::{menu::Menu, view::Message};
use coincube_ui::{
    color,
    component::text,
    icon::{clipboard_icon, cube_icon},
    image::{coincube_wordmark, theme_toggle_button},
    theme,
    widget::{Button, Column, Element, Row},
};
use iced::{
    widget::{column, container, row, Space},
    Alignment, Length,
};

/// Maximum width of the sidebar, assuming the tertiary rail is visible.
/// The rail column itself is conditionally narrower when the current
/// route has no third level. Toast offset uses this max width.
pub const SIDEBAR_WIDTH: f32 = primary::RAIL_WIDTH + secondary::RAIL_WIDTH + tertiary::RAIL_WIDTH;

/// Width when only the primary + secondary rails are visible (most
/// routes). Used for the wordmark / identity / toggle bands.
pub const SIDEBAR_BASE_WIDTH: f32 = primary::RAIL_WIDTH + secondary::RAIL_WIDTH;

/// Height of the wordmark header band that caps both rails.
const WORDMARK_BAND_HEIGHT: f32 = 48.0;
/// Wordmark font size — tuned to fit "COINCUBE" in the compact
/// two-rail header (~120px usable after padding).
const WORDMARK_SIZE: f32 = 14.0;
/// Fixed height for the identity block (avatar + cube name + LN
/// address). Fixed so the tertiary overlay's top offset is deterministic.
const IDENTITY_BLOCK_HEIGHT: f32 = 130.0;

/// Vertical distance from the top of the sidebar column to the top of
/// the rails row (primary + secondary). The tertiary overlay uses this
/// to position itself so its items align horizontally with the rails.
pub const TERTIARY_TOP_OFFSET: f32 = WORDMARK_BAND_HEIGHT + IDENTITY_BLOCK_HEIGHT;

/// Composed sidebar: wordmark · identity block · two rails · theme toggle.
///
/// Layout (top to bottom):
/// ```text
/// [ COINCUBE wordmark             ]  full 144px
/// [ avatar + cube name + LN addr  ]  full 144px — pushes both rails down
/// [ primary (72px) | secondary (72px) ]
/// [ dark/light toggle             ]  full 144px
/// ```
pub fn sidebar<'a>(menu: &Menu, ctx: &NavContext<'a>) -> Element<'a, Message> {
    // Base sidebar is always 144px wide (primary + secondary rails only).
    // The tertiary rail lives as an overlay on the content area — see
    // [`tertiary_rail`] and `dashboard_with_info` — so content never
    // shifts horizontally when the tertiary rail slides out.
    let rails_row: Row<Message> = row![primary::rail(menu, ctx), secondary::rail(menu, ctx)]
        .height(Length::Fill)
        .width(Length::Fixed(SIDEBAR_BASE_WIDTH));

    let wordmark = container(coincube_wordmark(WORDMARK_SIZE))
        .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
        .height(Length::Fixed(WORDMARK_BAND_HEIGHT))
        .center_x(Length::Fixed(SIDEBAR_BASE_WIDTH))
        .center_y(Length::Fixed(WORDMARK_BAND_HEIGHT))
        .style(theme::container::sidebar_primary);

    let identity = identity_block(ctx);
    let toggle = theme_toggle_row(ctx);

    let col: Column<Message> = column![wordmark, identity, rails_row, toggle]
        .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
        .height(Length::Fill)
        .align_x(Alignment::Start);
    col.into()
}

/// Tertiary rail for the current route, to be overlaid on the left
/// edge of the content area by `dashboard_with_info`. Returns `None`
/// when the active route has no third level.
pub fn tertiary_rail<'a>(menu: &Menu, ctx: &NavContext<'a>) -> Option<Element<'a, Message>> {
    tertiary::rail(menu, ctx)
}

/// Full-width identity header — avatar + cube name + optional Lightning
/// Address. Rendered above both rails so the top of each rail is pushed
/// down equally (the first secondary-rail item lines up with the `Home`
/// / `Cube` item on the primary rail).
fn identity_block<'a>(ctx: &NavContext<'a>) -> Element<'a, Message> {
    const AVATAR_SIZE: f32 = 56.0;

    let avatar: Element<'a, Message> = if let Some(handle) = ctx.avatar {
        iced::widget::image(handle.clone())
            .width(Length::Fixed(AVATAR_SIZE))
            .height(Length::Fixed(AVATAR_SIZE))
            .into()
    } else {
        container(cube_icon().size(28).color(color::GREY_3))
            .width(Length::Fixed(AVATAR_SIZE))
            .height(Length::Fixed(AVATAR_SIZE))
            .center_x(Length::Fixed(AVATAR_SIZE))
            .center_y(Length::Fixed(AVATAR_SIZE))
            .style(
                |t: &coincube_ui::theme::Theme| iced::widget::container::Style {
                    background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                    border: iced::Border {
                        radius: (AVATAR_SIZE / 2.0).into(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .into()
    };

    let avatar_button: Element<'a, Message> = Button::new(avatar)
        .style(theme::button::transparent)
        .on_press(Message::Menu(Menu::Home(
            crate::app::menu::HomeSubMenu::Overview,
        )))
        .into();

    let mut col: Column<Message> = Column::new()
        .padding([12, 8])
        .spacing(4)
        .align_x(Alignment::Center)
        .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
        .push(avatar_button);

    if !ctx.cube_name.is_empty() {
        col = col.push(
            text::p2_bold(ctx.cube_name)
                .style(theme::text::primary)
                .align_x(Alignment::Center),
        );
    }

    if let Some(addr) = ctx.lightning_address {
        let display_addr = if addr.contains('@') {
            addr.to_string()
        } else {
            format!("{}@coincube.io", addr)
        };
        let r: Row<Message> = row![
            text::caption(display_addr.clone()).color(color::GREY_3),
            clipboard_icon().size(10).color(color::GREY_3),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        let btn: Button<Message> = Button::new(r)
            .style(theme::button::transparent)
            .on_press(Message::Clipboard(display_addr));
        col = col.push(btn);
    }

    container(col)
        .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
        .height(Length::Fixed(IDENTITY_BLOCK_HEIGHT))
        .style(theme::container::sidebar_primary)
        .into()
}

/// Dark/light toggle anchored at the bottom of the sidebar, spanning
/// both rails. Mirrors the placement in the pre-refactor sidebar.
fn theme_toggle_row<'a>(ctx: &NavContext<'a>) -> Element<'a, Message> {
    let toggle = theme_toggle_button(ctx.theme_mode, Message::ToggleTheme);
    container(
        row![
            Space::new().width(Length::Fill),
            toggle,
            Space::new().width(Length::Fill),
        ]
        .align_y(Alignment::Center),
    )
    .width(Length::Fixed(SIDEBAR_BASE_WIDTH))
    .padding([10, 0])
    .style(theme::container::sidebar_primary)
    .into()
}

/// Ambient data both rails need while rendering.
pub struct NavContext<'a> {
    pub has_vault: bool,
    pub has_p2p: bool,
    pub cube_name: &'a str,
    pub lightning_address: Option<&'a str>,
    pub avatar: Option<&'a iced::widget::image::Handle>,
    pub theme_mode: coincube_ui::theme::palette::ThemeMode,
    pub connect_authenticated: bool,
}
