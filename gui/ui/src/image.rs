use crate::widget::Svg;
use iced::{widget::svg::Handle, window::icon};

const LIANA_APP_ICON: &[u8] = include_bytes!("../static/logos/liana-app-icon.png");
const LIANA_LOGO_GREY: &[u8] = include_bytes!("../static/logos/LIANA_SYMBOL_Gray.svg");

pub fn liana_app_icon() -> icon::Icon {
    icon::Icon::from_file_data(LIANA_APP_ICON, None).unwrap()
}

pub fn liana_grey_logo() -> Svg {
    let h = Handle::from_memory(LIANA_LOGO_GREY.to_vec());
    Svg::new(h)
}

const HISTORY_ICON: &[u8] = include_bytes!("../static/icons/history-icon.svg");
pub fn history_icon() -> Svg {
    let h = Handle::from_memory(HISTORY_ICON.to_vec());
    Svg::new(h)
}

const COINS_ICON: &[u8] = include_bytes!("../static/icons/coins-icon.svg");
pub fn coins_icon() -> Svg {
    let h = Handle::from_memory(COINS_ICON.to_vec());
    Svg::new(h)
}

const CLOCK_ICON: &[u8] = include_bytes!("../static/icons/clock-icon.svg");
pub fn clock_icon() -> Svg {
    let h = Handle::from_memory(CLOCK_ICON.to_vec());
    Svg::new(h)
}

const CLOCK_RED_ICON: &[u8] = include_bytes!("../static/icons/clock-red-icon.svg");
pub fn clock_red_icon() -> Svg {
    let h = Handle::from_memory(CLOCK_RED_ICON.to_vec());
    Svg::new(h)
}
