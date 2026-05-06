use crate::{icon, image, theme, widget::*};

const BADGE_SIZE: u32 = 40;
const ICON_SIZE: u32 = BADGE_SIZE / 2;
const LIANA_ICON_SIZE: u32 = 25;

pub fn badge<T>(icon: crate::widget::Text<'static>) -> Container<'static, T> {
    Container::new(icon.width(ICON_SIZE))
        .style(theme::badge::simple)
        .center_x(BADGE_SIZE)
        .center_y(BADGE_SIZE)
}

macro_rules! icon_badge {
    ($name:ident, $icon:ident) => {
        pub fn $name<T>() -> Container<'static, T> {
            badge(icon::$icon())
        }
    };
}

icon_badge!(receive, receive_icon);
icon_badge!(cycle, arrow_repeat);
icon_badge!(spend, send_icon);

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
