use crate::widget::*;
use iced::{alignment, Font, Length};

const ICONS: Font = Font::External {
    name: "Icons",
    bytes: include_bytes!("../static/icons/bootstrap-icons.ttf"),
};

fn icon(unicode: char) -> Text<'static> {
    Text::new(unicode.to_string())
        .font(ICONS)
        .width(Length::Units(20))
        .horizontal_alignment(alignment::Horizontal::Center)
        .size(20)
}

pub fn arrow_down() -> Text<'static> {
    icon('\u{F128}')
}

pub fn chevron_right() -> Text<'static> {
    icon('\u{F285}')
}

pub fn recovery_icon() -> Text<'static> {
    icon('\u{F467}')
}

pub fn plug_icon() -> Text<'static> {
    icon('\u{F4F6}')
}

pub fn reload_icon() -> Text<'static> {
    icon('\u{F130}')
}

pub fn import_icon() -> Text<'static> {
    icon('\u{F30A}')
}

pub fn wallet_icon() -> Text<'static> {
    icon('\u{F615}')
}

pub fn hourglass_icon() -> Text<'static> {
    icon('\u{F41F}')
}

pub fn hourglass_done_icon() -> Text<'static> {
    icon('\u{F41E}')
}

pub fn vault_icon() -> Text<'static> {
    icon('\u{F65A}')
}

pub fn bitcoin_icon() -> Text<'static> {
    icon('\u{F635}')
}

pub fn history_icon() -> Text<'static> {
    icon('\u{F292}')
}

pub fn home_icon() -> Text<'static> {
    icon('\u{F3FC}')
}

pub fn unlock_icon() -> Text<'static> {
    icon('\u{F600}')
}

pub fn warning_octagon_icon() -> Text<'static> {
    icon('\u{F337}')
}

pub fn send_icon() -> Text<'static> {
    icon('\u{F144}')
}

pub fn connect_device_icon() -> Text<'static> {
    icon('\u{F348}')
}

pub fn connected_device_icon() -> Text<'static> {
    icon('\u{F350}')
}

pub fn receive_icon() -> Text<'static> {
    icon('\u{F123}')
}

pub fn calendar_icon() -> Text<'static> {
    icon('\u{F1E8}')
}

pub fn turnback_icon() -> Text<'static> {
    icon('\u{F131}')
}

pub fn vaults_icon() -> Text<'static> {
    icon('\u{F1C7}')
}

pub fn coin_icon() -> Text<'static> {
    icon('\u{F567}')
}

pub fn settings_icon() -> Text<'static> {
    icon('\u{F3E5}')
}

pub fn block_icon() -> Text<'static> {
    icon('\u{F1C8}')
}

pub fn square_icon() -> Text<'static> {
    icon('\u{F584}')
}

pub fn square_check_icon() -> Text<'static> {
    icon('\u{F26D}')
}

pub fn circle_check_icon() -> Text<'static> {
    icon('\u{F26B}')
}

pub fn circle_cross_icon() -> Text<'static> {
    icon('\u{F623}')
}

pub fn network_icon() -> Text<'static> {
    icon('\u{F40D}')
}

pub fn dot_icon() -> Text<'static> {
    icon('\u{F287}')
}

pub fn clipboard_icon() -> Text<'static> {
    icon('\u{F3C2}')
}

pub fn shield_icon() -> Text<'static> {
    icon('\u{F53F}')
}

pub fn shield_notif_icon() -> Text<'static> {
    icon('\u{F530}')
}

pub fn shield_check_icon() -> Text<'static> {
    icon('\u{F52F}')
}

pub fn person_check_icon() -> Text<'static> {
    icon('\u{F4D6}')
}

pub fn person_icon() -> Text<'static> {
    icon('\u{F4DA}')
}

pub fn tooltip_icon() -> Text<'static> {
    icon('\u{F431}')
}

pub fn plus_icon() -> Text<'static> {
    icon('\u{F4FE}')
}

pub fn warning_icon() -> Text<'static> {
    icon('\u{F33B}')
}

pub fn chip_icon() -> Text<'static> {
    icon('\u{F2D6}')
}

pub fn trash_icon() -> Text<'static> {
    icon('\u{F5DE}')
}

pub fn key_icon() -> Text<'static> {
    icon('\u{F44F}')
}

pub fn cross_icon() -> Text<'static> {
    icon('\u{F62A}')
}

pub fn pencil_icon() -> Text<'static> {
    icon('\u{F4CB}')
}

#[allow(dead_code)]
pub fn stakeholder_icon() -> Text<'static> {
    icon('\u{F4AE}')
}

#[allow(dead_code)]
pub fn manager_icon() -> Text<'static> {
    icon('\u{F4B4}')
}

pub fn done_icon() -> Text<'static> {
    icon('\u{F26B}')
}

pub fn todo_icon() -> Text<'static> {
    icon('\u{F28A}')
}

pub fn collapse_icon() -> Text<'static> {
    icon('\u{F284}')
}

pub fn collapsed_icon() -> Text<'static> {
    icon('\u{F282}')
}

pub fn down_icon() -> Text<'static> {
    icon('\u{F279}')
}

pub fn up_icon() -> Text<'static> {
    icon('\u{F27C}')
}

pub fn people_icon() -> Text<'static> {
    icon('\u{F4CF}')
}
