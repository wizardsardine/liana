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
