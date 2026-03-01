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

pub fn cross_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F62A}')
}

pub fn arrow_down<'a>() -> Text<'a> {
    bootstrap_icon('\u{F128}')
}

pub fn arrow_back<'a>() -> Text<'a> {
    bootstrap_icon('\u{F12E}')
}

pub fn arrow_right<'a>() -> Text<'a> {
    bootstrap_icon('\u{F138}')
}

pub fn arrow_return_right<'a>() -> Text<'a> {
    bootstrap_icon('\u{F132}')
}

pub fn chevron_right<'a>() -> Text<'a> {
    bootstrap_icon('\u{F285}')
}

pub fn recovery_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F467}')
}

pub fn plug_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4F6}')
}

pub fn reload_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F130}')
}

pub fn import_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F30A}')
}

pub fn wallet_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F615}')
}

pub fn card_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F2D9}')
}

pub fn bitcoin_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F635}')
}

pub fn dollar_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F636}')
}

pub fn globe_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F91B}')
}

pub fn block_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F1C8}')
}

pub fn dot_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F287}')
}

pub fn email_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F7BE}')
}

pub fn person_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4DA}')
}

pub fn tooltip_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F431}')
}

pub fn plus_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4FE}')
}

pub fn warning_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F33B}')
}

pub fn chip_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F2D6}')
}

pub fn trash_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F5DE}')
}

pub fn pencil_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4CB}')
}

pub fn collapse_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F285}')
}

pub fn collapsed_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F282}')
}

pub fn down_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F279}')
}

pub fn up_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F27C}')
}

pub fn left_right_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F12B}')
}

pub fn up_down_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F127}')
}

pub fn network_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F40D}')
}

pub fn previous_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F284}')
}

pub fn check_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F633}')
}

pub fn round_key_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F44E}')
}

pub fn backup_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F356}')
}

pub fn restore_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F358}')
}

pub fn wrench_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F621}')
}

pub fn link_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F470}')
}

pub fn paste_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F290}')
}

pub fn usb_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F6DC}')
}

pub fn usb_drive_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F6F2}')
}

pub fn hdd_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F412}')
}

pub fn enter_box_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F1BE}')
}

pub fn qr_code_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F6AE}')
}

pub fn arrow_repeat<'a>() -> Text<'a> {
    bootstrap_icon('\u{F130}')
}

pub fn send_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F603}')
}

pub fn receive_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F30A}')
}

pub fn home_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F3FC}')
}

pub fn settings_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F3E5}')
}

pub fn key_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F44F}')
}

pub fn history_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F479}')
}

pub fn escape_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F7EE}')
}

pub fn coins_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F585}')
}

pub fn clock_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F293}')
}

pub fn clipboard_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F290}')
}

pub fn square_check_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F26D}')
}

pub fn square_cross_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F629}')
}

pub fn building_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F87D}')
}

pub fn vault_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F53F}')
}

pub fn lightning_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F46E}')
}

pub fn droplet_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F30C}')
}

pub fn eye_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F33E}')
}

pub fn eye_outline_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F341}')
}

pub fn eye_slash_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F33F}')
}

pub fn cube_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F1C8}')
}

pub fn receipt_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F50F}')
}

pub fn arrow_down_up_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F127}')
}

pub fn cash_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F247}')
}

pub fn lock_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F47B}')
}

pub fn file_earmark_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F373}')
}

pub fn check_circle_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F26B}')
}

pub fn phone_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4E7}')
}

pub fn chain_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F470}')
}

pub fn invoice_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F86F}')
}

pub fn pen_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4CA}')
}
