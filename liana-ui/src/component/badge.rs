use iced::widget::{container::Style, tooltip as iced_tooltip};
use iced::Length;

use crate::component::text::{p1_regular, p2_regular};
use crate::{icon, image, theme, widget::*};

const BADGE_SIZE: u32 = 40;
const ICON_SIZE: u32 = BADGE_SIZE / 2;
const LIANA_ICON_SIZE: u32 = 25;

pub fn badge_with_style<T, S>(icon: crate::widget::Text<'static>, style: S) -> Container<'static, T>
where
    S: Fn(&theme::Theme) -> Style + 'static,
{
    Container::new(icon.width(ICON_SIZE))
        .style(style)
        .center_x(BADGE_SIZE)
        .center_y(BADGE_SIZE)
}

macro_rules! icon_badge {
    ($name:ident, $icon:ident, $style:ident) => {
        pub fn $name<T>() -> Container<'static, T> {
            badge_with_style(icon::$icon(), theme::badge::$style)
        }
    };
}

icon_badge!(receive, receive_icon, simple);
icon_badge!(cycle, arrow_repeat, simple);
icon_badge!(spend, send_icon, simple);
icon_badge!(success, check_icon, success);
icon_badge!(tooltip, tooltip_icon, simple);
icon_badge!(network, network_icon, simple);
icon_badge!(block, block_icon, simple);
icon_badge!(bitcoin, bitcoin_icon, simple);
icon_badge!(setting, wrench_icon, simple);
icon_badge!(wallet, wallet_icon, simple);
icon_badge!(backup, backup_icon, simple);
icon_badge!(restore, restore_icon, simple);

pub fn coin<T>() -> Container<'static, T> {
    Container::new(
        image::liana_grey_logo()
            .height(LIANA_ICON_SIZE)
            .width(LIANA_ICON_SIZE),
    )
    .style(theme::badge::simple)
    .center_x(BADGE_SIZE)
    .center_y(BADGE_SIZE)
}

/// Payjoin "monad" symbol badge, used for payjoin settings/sections to distinguish
/// them from the Bitcoin node icon.
pub fn payjoin_symbol<T>() -> Container<'static, T> {
    Container::new(
        image::payjoin_monad_icon()
            .height(ICON_SIZE)
            .width(ICON_SIZE),
    )
    .style(theme::badge::simple)
    .center_x(BADGE_SIZE)
    .center_y(BADGE_SIZE)
}

pub fn payjoin<'a, T: 'a>() -> Container<'a, T> {
    Container::new(iced_tooltip::Tooltip::new(
        Container::new(p2_regular("  Payjoin  "))
            .padding(10)
            .center_x(Length::Shrink)
            .style(theme::pill::simple),
        Container::new(p1_regular("This is a Payjoin address"))
            .padding(10)
            .style(theme::card::simple),
        iced_tooltip::Position::Top,
    ))
}
