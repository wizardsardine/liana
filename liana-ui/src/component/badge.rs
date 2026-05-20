use iced::widget::container::Style;

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
