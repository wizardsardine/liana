use iced::Color;

use crate::color::TRANSPARENT;

use super::{
    liana_business::{EXTERNAL, SAFETY_NET},
    *,
};

color!(BTN_PRIMARY_HOVER_BGD, 0x00E65C);
color!(BTN_PRIMARY_PRESSED_BGD, 0x00CC52);
color!(BTN_PRIMARY_DISABLED_BGD, 0x242426);
color!(BTN_PRIMARY_TEXT, 0x042E16);
color!(BTN_PRIMARY_PRESSED_TEXT, 0xD1FAE5);
color!(BTN_PRIMARY_DISABLED_TEXT, 0x525253, 0.5);
color!(BTN_PRIMARY_DISABLED_BORDER, 0x3A3A3C);

color!(BTN_SECONDARY_HOVER_BGD, 0x183124);
color!(BTN_SECONDARY_PRESSED_BGD, 0x1A432B);
color!(BTN_SECONDARY_TEXT, 0x21C55E);
color!(BTN_SECONDARY_HOVER_TEXT, 0x49DE80);
color!(BTN_SECONDARY_PRESSED_TEXT, 0x86EFAC);

color!(BTN_TERTIARY_BGD, 0x3A3A3E);
color!(BTN_TERTIARY_HOVER_BGD, 0x48484E);
color!(BTN_TERTIARY_PRESSED_BGD, 0x2B2B2F);
color!(BTN_TERTIARY_TEXT, 0xDDDDDD);
const BTN_TERTIARY_HOVER_TEXT: Color = color::WHITE;
color!(BTN_TERTIARY_PRESSED_TEXT, 0xBABABB);

fn btn_disabled() -> Option<ButtonPalette> {
    Some(ButtonPalette {
        background: BTN_PRIMARY_DISABLED_BGD,
        text: BTN_PRIMARY_DISABLED_TEXT,
        border: BTN_PRIMARY_DISABLED_BORDER.into(),
        shadow: Default::default(),
    })
}

impl Palette {
    pub fn liana() -> Self {
        Self {
            general: General {
                background: color::LIGHT_BLACK,
                menu_background: color::BLACK,
                foreground: color::BLACK,
                scrollable: color::GREY_7,
                accent: color::GREEN,
                form_field_background: color::GREY_5,
            },
            text: Text {
                primary: color::WHITE,
                secondary: color::GREY_2,
                warning: color::ORANGE,
                success: color::GREEN,
                error: color::RED,
                accent: color::BLUE,
                card_secondary: color::CARD_TEXT_SECONDARY,
                address: color::GREY_2,
                address_dimmed: color::GREEN,
            },
            price: Price {
                zeroes: color::GREY_3,
                sats: color::WHITE,
                blink_zeroes: color::GREY_4,
                blink_sats: color::GREY_2,
                receive: color::SUCCESS_GREEN,
                send: color::MINUS_RED,
                refresh: color::BUSINESS_BLUE,
            },
            buttons: Buttons {
                border_width: 1.0,
                primary: Button {
                    active: ButtonPalette {
                        background: color::GREEN,
                        text: BTN_PRIMARY_TEXT,
                        border: TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: BTN_PRIMARY_HOVER_BGD,
                        text: BTN_PRIMARY_TEXT,
                        border: TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: BTN_PRIMARY_PRESSED_BGD,
                        text: BTN_PRIMARY_PRESSED_TEXT,
                        border: TRANSPARENT.into(),
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                secondary: Button {
                    active: ButtonPalette {
                        background: BTN_PRIMARY_DISABLED_BGD,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: BTN_SECONDARY_HOVER_BGD,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: BTN_SECONDARY_PRESSED_BGD,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                feerate: feerate_button(color::GREEN, BTN_PRIMARY_TEXT),
                feerate_unselected: feerate_unselected_button(
                    color::GREY_5,
                    color::WHITE,
                    color::GREEN,
                ),
                tertiary: Button {
                    active: ButtonPalette {
                        background: BTN_TERTIARY_BGD,
                        text: BTN_TERTIARY_TEXT,
                        border: TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: BTN_TERTIARY_HOVER_BGD,
                        text: BTN_TERTIARY_HOVER_TEXT,
                        border: TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: BTN_TERTIARY_PRESSED_BGD,
                        text: BTN_TERTIARY_PRESSED_TEXT,
                        border: TRANSPARENT.into(),
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                destructive: Button {
                    active: ButtonPalette {
                        background: color::GREY_6,
                        text: color::RED,
                        border: color::RED.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::RED,
                        text: color::LIGHT_BLACK,
                        border: color::RED.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::RED,
                        text: color::LIGHT_BLACK,
                        border: color::RED.into(),
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                transparent: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: Color {
                            a: 0.5,
                            ..color::GREY_2
                        },
                        border: None,
                        shadow: Default::default(),
                    }),
                },
                transparent_border: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                clickable_section: Button {
                    active: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                clickable_card: Button {
                    active: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                container: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                container_border: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                menu: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::WHITE,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::LIGHT_BLACK,
                        text: color::WHITE,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::GREEN,
                        text: color::BLACK,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: Color {
                            a: 0.5,
                            ..color::WHITE
                        },
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    }),
                },
                tab_menu: Button {
                    active: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::GREY_7.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: Color {
                            a: 0.5,
                            ..color::GREY_2
                        },
                        border: color::GREY_7.into(),
                        shadow: Default::default(),
                    }),
                },
                tab: Button {
                    active: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::GREY_7.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::LIGHT_BLACK,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: Color {
                            a: 0.5,
                            ..color::GREY_2
                        },
                        border: color::GREY_7.into(),
                        shadow: Default::default(),
                    }),
                },
                link: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: Color {
                            a: 0.5,
                            ..color::GREY_2
                        },
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    }),
                },
                pick_list: Button {
                    active: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::GREY_7.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
                signing_devices: Button {
                    active: ButtonPalette {
                        background: BTN_PRIMARY_DISABLED_BGD,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: BTN_SECONDARY_HOVER_BGD,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: BTN_SECONDARY_PRESSED_BGD,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                        shadow: Default::default(),
                    }),
                    disabled: btn_disabled(),
                },
            },
            cards: Cards {
                simple: ContainerPalette {
                    background: color::GREY_6,
                    text: None,
                    border: Some(color::TRANSPARENT),
                },
                transparent: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: None,
                    border: Some(color::TRANSPARENT),
                },
                modal: ContainerPalette {
                    background: color::LIGHT_BLACK,
                    text: None,
                    border: color::TRANSPARENT.into(),
                },
                border: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: None,
                    border: color::GREY_7.into(),
                },
                invalid: ContainerPalette {
                    background: color::LIGHT_BLACK,
                    text: color::RED.into(),
                    border: color::RED.into(),
                },
                legacy_warning: ContainerPalette {
                    background: color::LIGHT_BLACK,
                    text: color::ORANGE.into(),
                    border: color::ORANGE.into(),
                },
                warning: ContainerPalette {
                    background: color::LIGHT_BLACK,
                    text: color::ORANGE.into(),
                    border: color::ORANGE.into(),
                },
                soft_warning: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: color::ORANGE.into(),
                    border: color::GREY_7.into(),
                },
                info: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: color::WHITE.into(),
                    border: color::GREY_7.into(),
                },
                success: None,
                error: ContainerPalette {
                    background: color::LIGHT_BLACK,
                    text: color::RED.into(),
                    border: color::RED.into(),
                },
                section: ContainerPalette {
                    background: color::GREY_3,
                    text: None,
                    border: Some(color::TRANSPARENT),
                },
                flat: ContainerPalette {
                    background: color::GREY_6,
                    text: None,
                    border: Some(color::TRANSPARENT),
                },
            },
            banners: Banners {
                network: ContainerPalette {
                    background: color::BLUE,
                    text: color::LIGHT_BLACK.into(),
                    border: None,
                },
                warning: ContainerPalette {
                    background: color::ORANGE,
                    text: color::LIGHT_BLACK.into(),
                    border: None,
                },
            },
            badges: Badges {
                simple: ContainerPalette {
                    background: color::GREY_4,
                    text: None,
                    border: color::TRANSPARENT.into(),
                },
                bitcoin: ContainerPalette {
                    background: color::ORANGE,
                    text: color::WHITE.into(),
                    border: color::TRANSPARENT.into(),
                },
                success: ContainerPalette {
                    background: color::GREEN,
                    text: color::BLACK.into(),
                    border: color::TRANSPARENT.into(),
                },
                avatar: ContainerPalette {
                    background: color::FINGERPRINT_BACKGROUND,
                    text: color::GREY_2.into(),
                    border: color::TRANSPARENT.into(),
                },
                danger: None,
            },
            tile_tones: Tiles {
                background: color::GREY_5,
                accent: Tile {
                    fg: color::GREEN,
                    bg: None,
                },
                neutral: Tile {
                    fg: SAFETY_NET,
                    bg: None,
                },
                muted: Tile {
                    fg: color::GREY_3,
                    bg: None,
                },
                danger: Tile {
                    fg: color::RED,
                    bg: Some(color::LIGHT_BLACK),
                },
            },
            pills: Pills {
                simple: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: color::GREY_3.into(),
                    border: color::GREY_3.into(),
                },
                simple_fill: ContainerPalette {
                    background: color::GREY_3,
                    text: color::WHITE.into(),
                    border: color::GREY_3.into(),
                },
                warning: ContainerPalette {
                    background: color::AMBER,
                    text: color::DARK_TEXT_SECONDARY.into(),
                    border: color::AMBER.into(),
                },
                soft_warning: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: color::AMBER.into(),
                    border: color::AMBER.into(),
                },
                role_manager: ContainerPalette {
                    background: color::GREY_5,
                    text: color::GREY_2.into(),
                    border: color::GREY_5.into(),
                },
                role_participant: ContainerPalette {
                    background: color::GREY_5,
                    text: color::GREY_2.into(),
                    border: color::GREY_5.into(),
                },
                success: ContainerPalette {
                    background: color::SUCCESS_GREEN,
                    text: color::WHITE.into(),
                    border: color::SUCCESS_GREEN.into(),
                },
                internal: ContainerPalette {
                    background: color::BUSINESS_BLUE,
                    text: color::WHITE.into(),
                    border: color::BUSINESS_BLUE.into(),
                },
                external: ContainerPalette {
                    background: EXTERNAL,
                    text: color::WHITE.into(),
                    border: EXTERNAL.into(),
                },
                safety_net: ContainerPalette {
                    background: SAFETY_NET,
                    text: color::WHITE.into(),
                    border: SAFETY_NET.into(),
                },
                fingerprint: ContainerPalette {
                    background: color::FINGERPRINT_BACKGROUND,
                    text: color::FINGERPRINT_TEXT.into(),
                    border: color::FINGERPRINT_BORDER.into(),
                },
            },
            notifications: Notifications {
                pending: ContainerPalette {
                    background: color::GREEN,
                    text: color::LIGHT_BLACK.into(),
                    border: Some(color::GREEN),
                },
                error: ContainerPalette {
                    background: color::ORANGE,
                    text: color::LIGHT_BLACK.into(),
                    border: Some(color::ORANGE),
                },
            },
            text_inputs: TextInputs {
                primary: TextInput {
                    active: TextInputPalette {
                        background: color::TRANSPARENT,
                        icon: color::TRANSPARENT,
                        placeholder: color::GREY_7,
                        value: color::GREY_2,
                        selection: color::GREEN,
                        border: Some(color::GREY_7),
                    },
                    disabled: TextInputPalette {
                        background: color::TRANSPARENT,
                        icon: color::TRANSPARENT,
                        placeholder: color::GREY_7,
                        value: color::GREY_2,
                        selection: color::GREEN,
                        border: Some(color::GREY_7),
                    },
                },
                invalid: TextInput {
                    active: TextInputPalette {
                        background: color::TRANSPARENT,
                        icon: color::TRANSPARENT,
                        placeholder: color::GREY_7,
                        value: color::GREY_2,
                        selection: color::GREEN,
                        border: Some(color::RED),
                    },
                    disabled: TextInputPalette {
                        background: color::TRANSPARENT,
                        icon: color::TRANSPARENT,
                        placeholder: color::GREY_7,
                        value: color::TRANSPARENT,
                        selection: color::GREEN,
                        border: Some(color::RED),
                    },
                },
            },
            checkboxes: Checkboxes {
                icon: color::GREEN,
                text: color::GREY_2,
                background: color::TRANSPARENT,
                border: Some(color::CHECKBOX_BORDER),
            },
            radio_buttons: RadioButtons {
                dot: color::GREEN,
                text: color::GREY_2,
                border: color::GREY_7,
            },
            sliders: Sliders {
                background: color::GREEN,
                border: color::GREEN,
                rail_border: None,
                rail_backgrounds: (color::GREEN, color::GREY_7),
            },
            progress_bars: ProgressBars {
                bar: color::GREEN,
                border: color::TRANSPARENT.into(),
                background: color::GREY_6,
            },
            rule: color::GREY_1,
            pane_grid: PaneGrid {
                background: color::BLACK,
                highlight_border: color::GREEN,
                highlight_background: color::TRANSPARENT_GREEN,
                picked_split: color::GREEN,
                hovered_split: color::GREEN,
            },
            togglers: Togglers {
                on: Toggler {
                    background: color::GREEN,
                    background_border: color::GREEN,
                    foreground: color::WHITE,
                    foreground_border: color::WHITE,
                },
                off: Toggler {
                    background: color::GREY_2,
                    background_border: color::GREY_2,
                    foreground: color::WHITE,
                    foreground_border: color::WHITE,
                },
            },
            menus: Menus {
                pick_list: Menu {
                    border: color::GREY_7,
                    text: color::GREY_2,
                    selected_text: color::BLACK,
                    background: color::GREY_6,
                    selected_background: color::GREEN,
                },
            },
            spinner: Spinner {
                track: color::GREY_5,
                arc: color::GREEN,
            },
        }
    }
}
