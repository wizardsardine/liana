use super::*;

impl Palette {
    pub fn business() -> Self {
        Self {
            general: General {
                background: color::LIGHT_BG,
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
                primary: Button {
                    active: ButtonPalette {
                        background: color::BUSINESS_BLUE,
                        text: color::WHITE,
                        border: color::BUSINESS_BLUE.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::BUSINESS_BLUE_DARK,
                        text: color::WHITE,
                        border: color::BUSINESS_BLUE_DARK.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::BUSINESS_BLUE_DARK,
                        text: color::WHITE,
                        border: color::BUSINESS_BLUE_DARK.into(),
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::LIGHT_BORDER_STRONG,
                        text: color::WHITE,
                        border: color::LIGHT_BORDER_STRONG.into(),
                        shadow: Default::default(),
                    }),
                },
                secondary: Button {
                    active: ButtonPalette {
                        background: color::LIGHT_BLUE_TINT,
                        text: color::DARK_TEXT_SECONDARY,
                        border: color::LIGHT_BORDER.into(), // Neutral border, blue appears on hover
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::LIGHT_BLUE_TINT, // Keep same background as active (like liana)
                        text: color::BUSINESS_BLUE,
                        border: color::BUSINESS_BLUE.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::LIGHT_BLUE_TINT, // Keep same background
                        text: color::BUSINESS_BLUE,
                        border: color::BUSINESS_BLUE.into(),
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::LIGHT_BG_TERTIARY,
                        text: color::DARK_TEXT_TERTIARY,
                        border: color::LIGHT_BORDER.into(),
                        shadow: Default::default(),
                    }),
                },
                tertiary: Button {
                    active: ButtonPalette {
                        background: color::WHITE,
                        text: color::BUSINESS_BLACK,
                        border: color::WHITE.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::BUSINESS_BLUE_DARK,
                        text: color::WHITE,
                        border: color::BUSINESS_BLUE_DARK.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::BUSINESS_BLUE_DARK,
                        text: color::WHITE,
                        border: color::BUSINESS_BLUE_DARK.into(),
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::LIGHT_BORDER_STRONG,
                        text: color::WHITE,
                        border: color::LIGHT_BORDER_STRONG.into(),
                        shadow: Default::default(),
                    }),
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
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::RED,
                        text: color::WHITE,
                        border: color::RED.into(),
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::LIGHT_BG_TERTIARY,
                        text: color::RED,
                        border: color::RED.into(),
                        shadow: Default::default(),
                    }),
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
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_TERTIARY,
                        border: None,
                        shadow: Default::default(),
                    }),
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
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_TERTIARY,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    }),
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
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_PRIMARY,
                        border: None,
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_PRIMARY,
                        border: None,
                        shadow: Default::default(),
                    }),
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
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::BUSINESS_BLUE,
                        border: color::BUSINESS_BLUE.into(),
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_TERTIARY,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    }),
                },
                menu: Button {
                    active: ButtonPalette {
                        background: color::LIGHT_BG_SECONDARY,
                        text: color::DARK_TEXT_PRIMARY,
                        border: color::LIGHT_BG_SECONDARY.into(),
                        shadow: Default::default(),
                    },
                    hovered: ButtonPalette {
                        background: color::LIGHT_BG_TERTIARY,
                        text: color::DARK_TEXT_PRIMARY,
                        border: color::LIGHT_BG_TERTIARY.into(),
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::BUSINESS_BLUE,
                        text: color::WHITE,
                        border: color::BUSINESS_BLUE.into(),
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::LIGHT_BG_SECONDARY,
                        text: color::DARK_TEXT_TERTIARY,
                        border: color::LIGHT_BG_SECONDARY.into(),
                        shadow: Default::default(),
                    }),
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
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::LIGHT_BG,
                        text: color::BUSINESS_BLUE_DARK,
                        border: color::BUSINESS_BLUE_DARK.into(),
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::LIGHT_BG_TERTIARY,
                        text: color::DARK_TEXT_TERTIARY,
                        border: color::LIGHT_BORDER.into(),
                        shadow: Default::default(),
                    }),
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
                        shadow: Default::default(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::BUSINESS_BLUE,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_TEXT_TERTIARY,
                        border: color::TRANSPARENT.into(),
                        shadow: Default::default(),
                    }),
                },
            },
            cards: Cards {
                simple: ContainerPalette {
                    background: color::LIGHT_BG_SECONDARY,
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
