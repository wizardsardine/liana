use iced::widget::container::Style;
use iced::{Background, Border};

use crate::{
    component::text::{self, Text},
    icon, image, theme,
    widget::*,
};

const BADGE_SIZE: u32 = 40;
pub const AVATAR_SIZE: u32 = 30;
const AVATAR_TEXT_SIZE: u32 = 12;
const ICON_SIZE: u32 = BADGE_SIZE / 2;
const LIANA_ICON_SIZE: u32 = 25;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Tile {
    Org,
    Wallet,
    Setting,
    About,
    KeyInternal,
    KeyExternal,
    KeyService,
    Device,
    Account,
    DeviceMuted,
    Registering,
    RegFailed,
    Restricted,
    Import,
    Paste,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum TileStyle {
    Accent,
    Neutral,
    Muted,
    Danger,
}

#[derive(Debug, Copy, Clone, PartialEq)]
struct TileSize {
    size: u32,
    radius: f32,
    icon_size: u32,
}

impl TileSize {
    const DEFAULT: Self = Self {
        size: 44,
        radius: 12.0,
        icon_size: 20,
    };

    const L: Self = Self {
        size: 48,
        radius: 14.0,
        icon_size: 24,
    };

    const XL: Self = Self {
        size: 56,
        radius: 16.0,
        icon_size: 26,
    };
}

struct TileSpec<'a> {
    icon: crate::widget::Text<'a>,
    tone: TileStyle,
    size: TileSize,
}

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

pub fn tile<'a, M>(name: Tile) -> Container<'a, M> {
    tile_with_tone(name, None)
}

pub fn tile_accent<'a, M>(name: Tile) -> Container<'a, M> {
    tile_with_tone(name, Some(TileStyle::Accent))
}

fn tile_with_tone<'a, M>(name: Tile, tone: Option<TileStyle>) -> Container<'a, M> {
    let none = tone.is_none();
    let spec = tile_spec(name);
    let tone = tone.unwrap_or(spec.tone);
    let size = spec.size;
    let icon =
        spec.icon
            .width(size.size)
            .size(size.icon_size)
            .style(move |theme: &theme::Theme| {
                let tone = if !theme.is_business() && none {
                    TileStyle::Accent
                } else {
                    tone
                };
                iced::widget::text::Style {
                    color: Some(tile_tone(theme, tone).fg),
                }
            });

    Container::new(icon)
        .style(move |theme| tile_style(theme, tone, size.radius))
        .center_x(size.size)
        .center_y(size.size)
}

pub fn avatar<'a, M: 'a>(initials: String) -> Container<'a, M> {
    Container::new(
        text::new::small_caption(initials)
            .size(AVATAR_TEXT_SIZE)
            .bold(),
    )
    .center_x(AVATAR_SIZE)
    .center_y(AVATAR_SIZE)
    .style(theme::badge::avatar)
}

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

macro_rules! tile_specs {
    ($(($variant:ident, $icon:ident, $tone:ident, $size:ident)),* $(,)?) => {
        fn tile_spec<'a>(name: Tile) -> TileSpec<'a> {
            match name {
                $(Tile::$variant => TileSpec {
                    icon: icon::$icon(),
                    tone: TileStyle::$tone,
                    size: TileSize::$size,
                },)*
            }
        }
    };
}

tile_specs! {
    (Org, org_icon, Accent, DEFAULT),
    (Wallet, wallet_icon, Accent, DEFAULT),
    (Setting, wrench_icon, Accent, DEFAULT),
    (About, tooltip_icon, Accent, DEFAULT),
    (KeyInternal, round_key_icon, Neutral, DEFAULT),
    (KeyExternal, scale_icon, Neutral, DEFAULT),
    (KeyService, shield_icon, Neutral, DEFAULT),
    (Device, usb_icon, Neutral, DEFAULT),
    (Account, person_icon, Neutral, DEFAULT),
    (DeviceMuted, usb_icon, Muted, DEFAULT),
    (Registering, usb_icon, Accent, L),
    (RegFailed, warning_icon, Danger, L),
    (Restricted, lock_icon, Muted, XL),
    (Import, import_icon, Neutral, DEFAULT),
    (Paste, paste_icon, Neutral, DEFAULT),
}

fn tile_tone(theme: &theme::Theme, tone: TileStyle) -> theme::palette::Tile {
    match tone {
        TileStyle::Accent => theme.colors.tile_tones.accent,
        TileStyle::Neutral => theme.colors.tile_tones.neutral,
        TileStyle::Muted => theme.colors.tile_tones.muted,
        TileStyle::Danger => theme.colors.tile_tones.danger,
    }
}

fn tile_style(theme: &theme::Theme, tone: TileStyle, radius: f32) -> Style {
    let tone = tile_tone(theme, tone);

    Style {
        background: Some(Background::Color(
            tone.bg.unwrap_or(theme.colors.tile_tones.background),
        )),
        text_color: Some(tone.fg),
        border: Border {
            radius: radius.into(),
            width: 0.0,
            color: iced::Color::TRANSPARENT,
            ..Default::default()
        },
        ..Default::default()
    }
}
