use iced::pure::text;
use iced::{alignment, Font, Length, Text};

const ICONS: Font = Font::External {
    name: "Icons",
    bytes: include_bytes!("../../static/icons/bootstrap-icons.ttf"),
};

fn icon(unicode: char) -> Text {
    text(&unicode.to_string())
        .font(ICONS)
        .width(Length::Units(20))
        .horizontal_alignment(alignment::Horizontal::Center)
        .size(20)
}

pub fn vault_icon() -> Text {
    icon('\u{F65A}')
}

pub fn bitcoin_icon() -> Text {
    icon('\u{F635}')
}

pub fn history_icon() -> Text {
    icon('\u{F292}')
}

pub fn home_icon() -> Text {
    icon('\u{F3FC}')
}

pub fn unlock_icon() -> Text {
    icon('\u{F600}')
}

pub fn warning_octagon_icon() -> Text {
    icon('\u{F337}')
}

pub fn send_icon() -> Text {
    icon('\u{F144}')
}

pub fn connect_device_icon() -> Text {
    icon('\u{F348}')
}

pub fn connected_device_icon() -> Text {
    icon('\u{F350}')
}

pub fn receive_icon() -> Text {
    icon('\u{F123}')
}

pub fn calendar_icon() -> Text {
    icon('\u{F1E8}')
}

pub fn turnback_icon() -> Text {
    icon('\u{F131}')
}

pub fn vaults_icon() -> Text {
    icon('\u{F1C7}')
}

pub fn coin_icon() -> Text {
    icon('\u{F567}')
}

pub fn settings_icon() -> Text {
    icon('\u{F3E5}')
}

pub fn block_icon() -> Text {
    icon('\u{F1C8}')
}

pub fn square_icon() -> Text {
    icon('\u{F584}')
}

pub fn square_check_icon() -> Text {
    icon('\u{F26D}')
}

pub fn circle_check_icon() -> Text {
    icon('\u{F26B}')
}

pub fn network_icon() -> Text {
    icon('\u{F40D}')
}

pub fn dot_icon() -> Text {
    icon('\u{F287}')
}

pub fn clipboard_icon() -> Text {
    icon('\u{F28E}')
}

pub fn shield_icon() -> Text {
    icon('\u{F53F}')
}

pub fn shield_notif_icon() -> Text {
    icon('\u{F530}')
}

pub fn shield_check_icon() -> Text {
    icon('\u{F52F}')
}

pub fn person_check_icon() -> Text {
    icon('\u{F4D6}')
}

pub fn person_icon() -> Text {
    icon('\u{F4DA}')
}

pub fn tooltip_icon() -> Text {
    icon('\u{F431}')
}

pub fn plus_icon() -> Text {
    icon('\u{F4FE}')
}

pub fn warning_icon() -> Text {
    icon('\u{F33B}')
}

pub fn chip_icon() -> Text {
    icon('\u{F2D6}')
}

pub fn trash_icon() -> Text {
    icon('\u{F5DE}')
}

pub fn key_icon() -> Text {
    icon('\u{F44F}')
}

pub fn cross_icon() -> Text {
    icon('\u{F62A}')
}

pub fn pencil_icon() -> Text {
    icon('\u{F4CB}')
}

#[allow(dead_code)]
pub fn stakeholder_icon() -> Text {
    icon('\u{F4AE}')
}

#[allow(dead_code)]
pub fn manager_icon() -> Text {
    icon('\u{F4B4}')
}

pub fn done_icon() -> Text {
    icon('\u{F26B}')
}

pub fn todo_icon() -> Text {
    icon('\u{F28A}')
}
