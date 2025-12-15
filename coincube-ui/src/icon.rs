use crate::{component::text::P1_SIZE, widget::*};
use iced::{alignment, Font, Length};

const BOOTSTRAP_ICONS: Font = Font::with_name("bootstrap-icons");

fn bootstrap_icon<'a>(unicode: char) -> Text<'a> {
    Text::new(unicode)
        .font(BOOTSTRAP_ICONS)
        .width(Length::Fixed(20.0))
        .align_x(alignment::Horizontal::Center)
        .size(P1_SIZE)
}

pub fn cross_icon() -> Text<'static> {
    bootstrap_icon('\u{F62A}')
}

pub fn arrow_down() -> Text<'static> {
    bootstrap_icon('\u{F128}')
}

pub fn arrow_back() -> Text<'static> {
    bootstrap_icon('\u{F12E}')
}

pub fn arrow_right() -> Text<'static> {
    bootstrap_icon('\u{F138}')
}

pub fn arrow_return_right() -> Text<'static> {
    bootstrap_icon('\u{F132}')
}

pub fn chevron_right() -> Text<'static> {
    bootstrap_icon('\u{F285}')
}

pub fn recovery_icon() -> Text<'static> {
    bootstrap_icon('\u{F467}')
}

pub fn plug_icon() -> Text<'static> {
    bootstrap_icon('\u{F4F6}')
}

pub fn reload_icon() -> Text<'static> {
    bootstrap_icon('\u{F130}')
}

pub fn import_icon() -> Text<'static> {
    bootstrap_icon('\u{F30A}')
}

pub fn wallet_icon() -> Text<'static> {
    bootstrap_icon('\u{F615}')
}

pub fn card_icon() -> Text<'static> {
    bootstrap_icon('\u{F2D9}')
}

pub fn bitcoin_icon() -> Text<'static> {
    bootstrap_icon('\u{F635}')
}

pub fn dollar_icon() -> Text<'static> {
    bootstrap_icon('\u{F636}')
}

pub fn globe_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F91B}')
}

pub fn block_icon() -> Text<'static> {
    bootstrap_icon('\u{F1C8}')
}

pub fn dot_icon() -> Text<'static> {
    bootstrap_icon('\u{F287}')
}

pub fn email_icon() -> Text<'static> {
    bootstrap_icon('\u{F7BE}')
}

pub fn person_icon() -> Text<'static> {
    bootstrap_icon('\u{F4DA}')
}

pub fn tooltip_icon() -> Text<'static> {
    bootstrap_icon('\u{F431}')
}

pub fn plus_icon() -> Text<'static> {
    bootstrap_icon('\u{F4FE}')
}

pub fn warning_icon() -> Text<'static> {
    bootstrap_icon('\u{F33B}')
}

pub fn chip_icon() -> Text<'static> {
    bootstrap_icon('\u{F2D6}')
}

pub fn trash_icon() -> Text<'static> {
    bootstrap_icon('\u{F5DE}')
}

pub fn pencil_icon() -> Text<'static> {
    bootstrap_icon('\u{F4CB}')
}

pub fn collapse_icon() -> Text<'static> {
    bootstrap_icon('\u{F285}')
}

pub fn collapsed_icon() -> Text<'static> {
    bootstrap_icon('\u{F282}')
}

pub fn down_icon() -> Text<'static> {
    bootstrap_icon('\u{F279}')
}

pub fn up_icon() -> Text<'static> {
    bootstrap_icon('\u{F27C}')
}

pub fn left_right_icon() -> Text<'static> {
    bootstrap_icon('\u{F12B}')
}

pub fn up_down_icon() -> Text<'static> {
    bootstrap_icon('\u{F127}')
}

pub fn network_icon() -> Text<'static> {
    bootstrap_icon('\u{F40D}')
}

pub fn previous_icon() -> Text<'static> {
    bootstrap_icon('\u{F284}')
}

pub fn check_icon() -> Text<'static> {
    bootstrap_icon('\u{F633}')
}

pub fn round_key_icon() -> Text<'static> {
    bootstrap_icon('\u{F44E}')
}

pub fn backup_icon() -> Text<'static> {
    bootstrap_icon('\u{F356}')
}

pub fn restore_icon() -> Text<'static> {
    bootstrap_icon('\u{F358}')
}

pub fn wrench_icon() -> Text<'static> {
    bootstrap_icon('\u{F621}')
}

pub fn link_icon() -> Text<'static> {
    bootstrap_icon('\u{F470}')
}

pub fn paste_icon() -> Text<'static> {
    bootstrap_icon('\u{F290}')
}

pub fn usb_icon() -> Text<'static> {
    bootstrap_icon('\u{F6DC}')
}

pub fn usb_drive_icon() -> Text<'static> {
    bootstrap_icon('\u{F6F2}')
}

pub fn hdd_icon() -> Text<'static> {
    bootstrap_icon('\u{F412}')
}

pub fn enter_box_icon() -> Text<'static> {
    bootstrap_icon('\u{F1BE}')
}

pub fn qr_code_icon() -> Text<'static> {
    bootstrap_icon('\u{F6AE}')
}

pub fn arrow_repeat() -> Text<'static> {
    bootstrap_icon('\u{F130}')
}

pub fn send_icon() -> Text<'static> {
    bootstrap_icon('\u{F603}')
}

pub fn receive_icon() -> Text<'static> {
    bootstrap_icon('\u{F30A}')
}

pub fn home_icon() -> Text<'static> {
    bootstrap_icon('\u{F3FC}')
}

pub fn settings_icon() -> Text<'static> {
    bootstrap_icon('\u{F3E5}')
}

pub fn key_icon() -> Text<'static> {
    bootstrap_icon('\u{F44F}')
}

pub fn history_icon() -> Text<'static> {
    bootstrap_icon('\u{F479}')
}

pub fn escape_icon() -> Text<'static> {
    bootstrap_icon('\u{F7EE}')
}

pub fn coins_icon() -> Text<'static> {
    bootstrap_icon('\u{F585}')
}

pub fn clock_icon() -> Text<'static> {
    bootstrap_icon('\u{F293}')
}

pub fn clipboard_icon() -> Text<'static> {
    bootstrap_icon('\u{F290}')
}

pub fn square_check_icon() -> Text<'static> {
    bootstrap_icon('\u{F26D}')
}

pub fn square_cross_icon() -> Text<'static> {
    bootstrap_icon('\u{F629}')
}

pub fn building_icon() -> Text<'static> {
    bootstrap_icon('\u{F87D}')
}

pub fn vault_icon() -> Text<'static> {
    bootstrap_icon('\u{F53F}')
}

pub fn lightning_icon() -> Text<'static> {
    bootstrap_icon('\u{F46E}')
}

pub fn eye_icon() -> Text<'static> {
    bootstrap_icon('\u{F33E}')
}

pub fn eye_outline_icon() -> Text<'static> {
    bootstrap_icon('\u{F341}')
}

pub fn eye_slash_icon() -> Text<'static> {
    bootstrap_icon('\u{F33F}')
}

pub fn cube_icon() -> Text<'static> {
    bootstrap_icon('\u{F1C8}')
}

pub fn receipt_icon() -> Text<'static> {
    bootstrap_icon('\u{F50F}')
}

pub fn arrow_down_up_icon() -> Text<'static> {
    bootstrap_icon('\u{F127}')
}
