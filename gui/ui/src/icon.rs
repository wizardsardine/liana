use crate::widget::*;
use iced::{alignment, Font, Length};

const BOOTSTRAP_ICONS: Font = Font::External {
    name: "Bootstrap icons",
    bytes: include_bytes!("../static/icons/bootstrap-icons.ttf"),
};

fn bootstrap_icon(unicode: char) -> Text<'static> {
    Text::new(unicode.to_string())
        .font(BOOTSTRAP_ICONS)
        .width(Length::Units(20))
        .horizontal_alignment(alignment::Horizontal::Center)
        .size(20)
}

pub fn cross_icon() -> Text<'static> {
    bootstrap_icon('\u{F62A}')
}

pub fn arrow_down() -> Text<'static> {
    bootstrap_icon('\u{F128}')
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

pub fn bitcoin_icon() -> Text<'static> {
    bootstrap_icon('\u{F635}')
}

pub fn block_icon() -> Text<'static> {
    bootstrap_icon('\u{F1C8}')
}

pub fn dot_icon() -> Text<'static> {
    bootstrap_icon('\u{F287}')
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
    bootstrap_icon('\u{F284}')
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

pub fn network_icon() -> Text<'static> {
    bootstrap_icon('\u{F40D}')
}

pub fn previous_icon() -> Text<'static> {
    bootstrap_icon('\u{F284}')
}

const ICONEX_ICONS: Font = Font::External {
    name: "Iconex icons",
    bytes: include_bytes!("../static/icons/iconex/iconex-icons.ttf"),
};

fn iconex_icon(unicode: char) -> Text<'static> {
    Text::new(unicode.to_string())
        .font(ICONEX_ICONS)
        .width(Length::Units(20))
        .horizontal_alignment(alignment::Horizontal::Center)
        .size(20)
}

pub fn arrow_repeat() -> Text<'static> {
    iconex_icon('\u{46BB}')
}

pub fn send_icon() -> Text<'static> {
    iconex_icon('\u{2CEE}')
}

pub fn receive_icon() -> Text<'static> {
    iconex_icon('\u{605B}')
}

pub fn home_icon() -> Text<'static> {
    iconex_icon('\u{C722}')
}

pub fn settings_icon() -> Text<'static> {
    iconex_icon('\u{3038}')
}

pub fn key_icon() -> Text<'static> {
    iconex_icon('\u{FFEC}')
}

pub fn history_icon() -> Text<'static> {
    iconex_icon('\u{BEBA}')
}

pub fn coins_icon() -> Text<'static> {
    iconex_icon('\u{9F25}')
}

pub fn clock_icon() -> Text<'static> {
    iconex_icon('\u{B0CA}')
}

pub fn clipboard_icon() -> Text<'static> {
    iconex_icon('\u{F8D3}')
}

pub fn circle_check_icon() -> Text<'static> {
    iconex_icon('\u{E2F9}')
}

pub fn circle_cross_icon() -> Text<'static> {
    iconex_icon('\u{19DA}')
}
