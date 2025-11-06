use iced::{
    widget::{svg, Svg},
    window::icon,
};

use crate::theme::Theme;

const LIANA_LOGOTYPE_GREY: &[u8] =
    include_bytes!("../static/logos/LIANA_LOGOTYPE_Gray-coincube.svg");
const LIANA_LOGOTYPE: &[u8] = include_bytes!("../static/logos/LIANA_LOGOTYPE-coincube.svg");

pub fn liana_window_icon() -> icon::Icon {
    let bytes = include_bytes!("../static/logos/liana-app-icon-coincube.ico");
    let img = image::load(std::io::Cursor::new(bytes), image::ImageFormat::Ico).unwrap();

    let width = img.width();
    let height = img.height();
    let buffer = img.into_rgba8().into_vec();

    icon::from_rgba(buffer, width, height).unwrap()
}

pub fn liana_logotype() -> Svg<'static, Theme> {
    let h = svg::Handle::from_memory(LIANA_LOGOTYPE);
    Svg::new(h)
}

pub fn liana_logotype_grey() -> Svg<'static, Theme> {
    let h = svg::Handle::from_memory(LIANA_LOGOTYPE_GREY);
    Svg::new(h)
}

const CREATE_NEW_WALLET_ICON: &[u8] = include_bytes!("../static/icons/blueprint.svg");

pub fn create_new_wallet_icon() -> Svg<'static, Theme> {
    let h = svg::Handle::from_memory(CREATE_NEW_WALLET_ICON);
    Svg::new(h)
}

const PARTICIPATE_IN_NEW_WALLET_ICON: &[u8] = include_bytes!("../static/icons/discussion.svg");

pub fn participate_in_new_wallet_icon() -> Svg<'static, Theme> {
    let h = svg::Handle::from_memory(PARTICIPATE_IN_NEW_WALLET_ICON);
    Svg::new(h)
}

const RESTORE_WALLET_ICON: &[u8] = include_bytes!("../static/icons/syncdata.svg");

pub fn restore_wallet_icon() -> Svg<'static, Theme> {
    let h = svg::Handle::from_memory(RESTORE_WALLET_ICON);
    Svg::new(h)
}

const SUCCESS_MARK_ICON: &[u8] = include_bytes!("../static/icons/success-mark.svg");

pub fn success_mark_icon() -> Svg<'static, Theme> {
    let h = svg::Handle::from_memory(SUCCESS_MARK_ICON);
    Svg::new(h)
}

const KEY_MARK_ICON: &[u8] = include_bytes!("../static/icons/key-mark.svg");

pub fn key_mark_icon() -> Svg<'static, Theme> {
    let h = svg::Handle::from_memory(KEY_MARK_ICON);
    Svg::new(h)
}

const INHERITANCE_TEMPLATE_DESC: &[u8] =
    include_bytes!("../static/images/inheritance_template_description.svg");

pub fn inheritance_template_description() -> Svg<'static, Theme> {
    let h = svg::Handle::from_memory(INHERITANCE_TEMPLATE_DESC);
    Svg::new(h)
}

const CUSTOM_TEMPLATE_DESC: &[u8] =
    include_bytes!("../static/images/custom_template_description.svg");

pub fn custom_template_description() -> Svg<'static, Theme> {
    let h = svg::Handle::from_memory(CUSTOM_TEMPLATE_DESC);
    Svg::new(h)
}

const MULTISIG_SECURITY_TEMPLATE_DESC: &[u8] =
    include_bytes!("../static/images/multisig_security_template.svg");

pub fn multisig_security_template_description() -> Svg<'static, Theme> {
    let h = svg::Handle::from_memory(MULTISIG_SECURITY_TEMPLATE_DESC);
    Svg::new(h)
}
