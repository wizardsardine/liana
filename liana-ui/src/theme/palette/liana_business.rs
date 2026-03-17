use iced::Color;

use crate::theme::card::CARD_SHADOW;

use super::*;

const BTN_PRIMARY_BG: Color = color::BUSINESS_BLUE;
const BTN_PRIMARY_FG: Color = color::WHITE;
const BTN_PRIMARY_PRESSED: Color = Color::from_rgb(
    0x00 as f32 / 255.0,
    0x77 as f32 / 255.0,
    0xA0 as f32 / 255.0,
);

const BTN_TERTIARY_BG: Color = color::WHITE;
const BTN_TERTIARY_FG: Color = color::BUSINESS_BLACK;
const BTN_TERTIARY_PRESSED: Color = Color::from_rgb(
    0xB4 as f32 / 255.0,
    0xB4 as f32 / 255.0,
    0xB4 as f32 / 255.0,
);

const BTN_DISABLED: Color = Color::from_rgb(
    0xCB as f32 / 255.0,
    0xCB as f32 / 255.0,
    0xCB as f32 / 255.0,
);
fn btn_disabled() -> Option<ButtonPalette> {
    Some(ButtonPalette {
        background: BTN_DISABLED,
        text: Color::from_rgb(
            0xED as f32 / 255.0,
            0xED as f32 / 255.0,
            0xED as f32 / 255.0,
        ),
        border: BTN_DISABLED.into(),
        shadow: Default::default(),
    })
}

const BTN_SHADOW: Shadow = Shadow {
    color: Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.25,
    },
    offset: iced::Vector { x: 0.0, y: 4.0 },
    blur_radius: 4.0,
};

const CARD_SHADOW_HOVER: Shadow = Shadow {
    color: Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.3,
    },
    offset: iced::Vector { x: 0.0, y: 4.0 },
    blur_radius: 4.0,
};

impl Palette {
    pub fn business() -> Self {
        Self {
            general: General {
                background: color::LIGHT_BG,
                menu_background: color::WHITE,
                foreground: color::LIGHT_BG_SECONDARY,
                scrollable: color::LIGHT_BORDER,
                accent: color::BUSINESS_BLUE,
            },
            text: Text {
                primary: color::DARK_TEXT_PRIMARY,
                secondary: color::DARK_TEXT_SECONDARY,
                warning: color::ORANGE,
                success: color::DARK_GREEN,
                error: color::RED,
                accent: color::BUSINESS_BLUE_DARK,
            },
            buttons: Buttons {
                border_width: 3.0,
                primary: Button {
                    active: ButtonPalette {
                        background: BTN_PRIMARY_BG,
                        text: BTN_PRIMARY_FG,
                        border: BTN_PRIMARY_BG.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: BTN_PRIMARY_BG,
                        text: BTN_PRIMARY_FG,
                        border: BTN_PRIMARY_BG.into(),
                        shadow: BTN_SHADOW,
                    },
                    pressed: Some(ButtonPalette {
                        background: BTN_PRIMARY_PRESSED,
                        text: BTN_PRIMARY_FG,
                        border: BTN_PRIMARY_PRESSED.into(),
                        shadow: BTN_SHADOW,
                    }),
                    disabled: btn_disabled(),
                },
                secondary: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: BTN_PRIMARY_BG,
                        border: BTN_PRIMARY_BG.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: BTN_PRIMARY_BG,
                        border: BTN_PRIMARY_BG.into(),
                        shadow: BTN_SHADOW,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: BTN_PRIMARY_PRESSED,
                        border: BTN_PRIMARY_PRESSED.into(),
                        shadow: BTN_SHADOW,
                    }),
                    disabled: btn_disabled(),
                },
                tertiary: Button {
                    active: ButtonPalette {
                        background: BTN_TERTIARY_BG,
                        text: BTN_TERTIARY_FG,
                        border: BTN_TERTIARY_BG.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: BTN_TERTIARY_BG,
                        text: BTN_TERTIARY_FG,
                        border: BTN_TERTIARY_BG.into(),
                        shadow: BTN_SHADOW,
                    },
                    pressed: Some(ButtonPalette {
                        background: BTN_TERTIARY_PRESSED,
                        text: color::WHITE,
                        border: BTN_TERTIARY_PRESSED.into(),
                        shadow: BTN_SHADOW,
                    }),
                    disabled: btn_disabled(),
                },
                destructive: Button {
                    active: ButtonPalette {
                        background: color::LIGHT_BG_SECONDARY,
                        text: color::RED,
                        border: color::RED.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::RED,
                        text: color::WHITE,
                        border: color::RED.into(),
                        shadow: BTN_SHADOW,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::RED,
                        text: color::WHITE,
                        border: color::RED.into(),
                        shadow: BTN_SHADOW,
                    }),
                    disabled: btn_disabled(),
                },
                transparent: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_PRIMARY,
                        border: None,
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::BUSINESS_BLUE,
                        border: None,
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::BUSINESS_BLUE,
                        border: None,
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                transparent_border: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_PRIMARY,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::BUSINESS_BLUE,
                        border: color::BUSINESS_BLUE.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::BUSINESS_BLUE,
                        border: color::BUSINESS_BLUE.into(),
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                clickable_card: Button {
                    active: ButtonPalette {
                        background: BTN_TERTIARY_BG,
                        text: BTN_TERTIARY_FG,
                        border: BTN_TERTIARY_BG.into(),
                        shadow: CARD_SHADOW,
                    },
                    hovered: ButtonPalette {
                        background: BTN_TERTIARY_BG,
                        text: BTN_TERTIARY_FG,
                        border: BTN_TERTIARY_BG.into(),
                        shadow: CARD_SHADOW_HOVER,
                    },
                    pressed: None,
                    disabled: None,
                },
                container: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_SECONDARY,
                        border: None,
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_PRIMARY,
                        border: None,
                        shadow: BTN_SHADOW,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_PRIMARY,
                        border: None,
                        shadow: BTN_SHADOW,
                    }),
                    disabled: btn_disabled(),
                },
                container_border: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_PRIMARY,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::BUSINESS_BLUE,
                        border: color::BUSINESS_BLUE.into(),
                        shadow: BTN_SHADOW,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::BUSINESS_BLUE,
                        border: color::BUSINESS_BLUE.into(),
                        shadow: BTN_SHADOW,
                    }),
                    disabled: btn_disabled(),
                },
                menu: Button {
                    active: ButtonPalette {
                        background: color::WHITE,
                        text: color::DARK_TEXT_PRIMARY,
                        border: color::WHITE.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::WHITE,
                        text: color::DARK_TEXT_PRIMARY,
                        border: color::WHITE.into(),
                        shadow: BTN_SHADOW,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::BUSINESS_BLUE,
                        text: color::WHITE,
                        border: color::BUSINESS_BLUE.into(),
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                tab: Button {
                    active: ButtonPalette {
                        background: color::LIGHT_BG_SECONDARY,
                        text: color::DARK_TEXT_PRIMARY,
                        border: color::LIGHT_BORDER.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::LIGHT_BG_SECONDARY,
                        text: color::BUSINESS_BLUE_DARK,
                        border: color::BUSINESS_BLUE_DARK.into(),
                        shadow: BTN_SHADOW,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::LIGHT_BG,
                        text: color::BUSINESS_BLUE_DARK,
                        border: color::BUSINESS_BLUE_DARK.into(),
                        shadow: BTN_SHADOW,
                    }),
                    disabled: btn_disabled(),
                },
                link: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_PRIMARY,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::BUSINESS_BLUE,
                        border: color::TRANSPARENT.into(),
                        shadow: BTN_SHADOW,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::BUSINESS_BLUE,
                        border: color::TRANSPARENT.into(),
                        shadow: BTN_SHADOW,
                    }),
                    disabled: btn_disabled(),
                },
            },
            cards: Cards {
                simple: ContainerPalette {
                    background: color::LIGHT_BG_SECONDARY,
                    text: None,
                    border: Some(color::TRANSPARENT),
                },
                transparent: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: None,
                    border: Some(color::TRANSPARENT),
                },
                modal: ContainerPalette {
                    background: color::LIGHT_BG,
                    text: None,
                    border: color::LIGHT_BORDER.into(),
                },
                border: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: None,
                    border: color::LIGHT_BORDER.into(),
                },
                invalid: ContainerPalette {
                    background: color::LIGHT_BG,
                    text: color::RED.into(),
                    border: color::RED.into(),
                },
                warning: ContainerPalette {
                    background: color::LIGHT_BG,
                    text: color::ORANGE.into(),
                    border: color::ORANGE.into(),
                },
                warning_banner: ContainerPalette {
                    background: color::ORANGE,
                    text: color::WHITE.into(),
                    border: color::ORANGE.into(),
                },
                home_hint: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: None,
                    border: color::LIGHT_BORDER.into(),
                },
                error: ContainerPalette {
                    background: color::LIGHT_BG,
                    text: color::RED.into(),
                    border: color::RED.into(),
                },
            },
            banners: Banners {
                network: ContainerPalette {
                    background: color::BUSINESS_BLUE,
                    text: color::WHITE.into(),
                    border: None,
                },
                warning: ContainerPalette {
                    background: color::ORANGE,
                    text: color::WHITE.into(),
                    border: None,
                },
            },
            badges: Badges {
                simple: ContainerPalette {
                    background: color::LIGHT_BG_TERTIARY,
                    text: None,
                    border: color::TRANSPARENT.into(),
                },
                bitcoin: ContainerPalette {
                    background: color::ORANGE,
                    text: color::WHITE.into(),
                    border: color::TRANSPARENT.into(),
                },
            },
            pills: Pills {
                primary: ContainerPalette {
                    background: color::BUSINESS_BLUE,
                    text: color::WHITE.into(),
                    border: color::TRANSPARENT.into(),
                },
                simple: ContainerPalette {
                    background: color::LIGHT_BLUE_TINT,
                    text: color::BUSINESS_BLUE_DARK.into(),
                    border: color::SOFT_BLUE.into(),
                },
                warning: ContainerPalette {
                    background: color::AMBER,
                    text: color::WHITE.into(),
                    border: color::AMBER.into(),
                },
                success: ContainerPalette {
                    background: color::DARK_GREEN,
                    text: color::WHITE.into(),
                    border: color::DARK_GREEN.into(),
                },
            },
            notifications: Notifications {
                pending: ContainerPalette {
                    background: color::BUSINESS_BLUE,
                    text: color::WHITE.into(),
                    border: Some(color::BUSINESS_BLUE),
                },
                error: ContainerPalette {
                    background: color::ORANGE,
                    text: color::WHITE.into(),
                    border: Some(color::ORANGE),
                },
            },
            text_inputs: TextInputs {
                primary: TextInput {
                    active: TextInputPalette {
                        background: color::LIGHT_BG,
                        icon: color::DARK_TEXT_TERTIARY,
                        placeholder: color::DARK_TEXT_TERTIARY,
                        value: color::DARK_TEXT_PRIMARY,
                        selection: color::BUSINESS_BLUE,
                        border: Some(color::LIGHT_BORDER),
                    },
                    disabled: TextInputPalette {
                        background: color::LIGHT_BG_TERTIARY,
                        icon: color::DARK_TEXT_TERTIARY,
                        placeholder: color::DARK_TEXT_TERTIARY,
                        value: color::DARK_TEXT_SECONDARY,
                        selection: color::BUSINESS_BLUE,
                        border: Some(color::LIGHT_BORDER),
                    },
                },
                invalid: TextInput {
                    active: TextInputPalette {
                        background: color::LIGHT_BG,
                        icon: color::DARK_TEXT_TERTIARY,
                        placeholder: color::DARK_TEXT_TERTIARY,
                        value: color::DARK_TEXT_PRIMARY,
                        selection: color::BUSINESS_BLUE,
                        border: Some(color::RED),
                    },
                    disabled: TextInputPalette {
                        background: color::LIGHT_BG_TERTIARY,
                        icon: color::DARK_TEXT_TERTIARY,
                        placeholder: color::DARK_TEXT_TERTIARY,
                        value: color::TRANSPARENT,
                        selection: color::BUSINESS_BLUE,
                        border: Some(color::RED),
                    },
                },
            },
            checkboxes: Checkboxes {
                icon: color::BUSINESS_BLUE,
                text: color::DARK_TEXT_PRIMARY,
                background: color::LIGHT_BG_SECONDARY,
                border: Some(color::LIGHT_BORDER),
            },
            radio_buttons: RadioButtons {
                dot: color::BUSINESS_BLUE,
                text: color::DARK_TEXT_PRIMARY,
                border: color::LIGHT_BORDER,
            },
            sliders: Sliders {
                background: color::BUSINESS_BLUE,
                border: color::BUSINESS_BLUE,
                rail_border: None,
                rail_backgrounds: (color::BUSINESS_BLUE, color::LIGHT_BORDER),
            },
            progress_bars: ProgressBars {
                bar: color::BUSINESS_BLUE,
                border: color::TRANSPARENT.into(),
                background: color::LIGHT_BG_TERTIARY,
            },
            rule: color::LIGHT_BORDER,
            pane_grid: PaneGrid {
                background: color::LIGHT_BG_SECONDARY,
                highlight_border: color::BUSINESS_BLUE,
                highlight_background: color::TRANSPARENT_BUSINESS_BLUE,
                picked_split: color::BUSINESS_BLUE,
                hovered_split: color::BUSINESS_BLUE,
            },
            togglers: Togglers {
                on: Toggler {
                    background: color::BUSINESS_BLUE,
                    background_border: color::BUSINESS_BLUE,
                    foreground: color::WHITE,
                    foreground_border: color::WHITE,
                },
                off: Toggler {
                    background: color::LIGHT_BORDER,
                    background_border: color::LIGHT_BORDER,
                    foreground: color::WHITE,
                    foreground_border: color::WHITE,
                },
            },
        }
    }
}
