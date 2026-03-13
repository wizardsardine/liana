use super::*;

impl Palette {
    pub fn liana() -> Self {
        Self {
            general: General {
                background: color::LIGHT_BLACK,
                foreground: color::BLACK,
                scrollable: color::GREY_7,
                accent: color::GREEN,
            },
            text: Text {
                primary: color::WHITE,
                secondary: color::GREY_2,
                warning: color::ORANGE,
                success: color::GREEN,
                error: color::RED,
                accent: color::BLUE,
            },
            buttons: Buttons {
                primary: Button {
                    active: ButtonPalette {
                        background: color::GREEN,
                        text: color::LIGHT_BLACK,
                        border: color::GREEN.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::GREEN,
                        text: color::LIGHT_BLACK,
                        border: color::GREEN.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::GREEN,
                        text: color::LIGHT_BLACK,
                        border: color::GREEN.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::GREY_7.into(),
                    }),
                },
                secondary: Button {
                    active: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::GREY_7.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::GREY_7.into(),
                    }),
                },
                destructive: Button {
                    active: ButtonPalette {
                        background: color::GREY_6,
                        text: color::RED,
                        border: color::RED.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::RED,
                        text: color::LIGHT_BLACK,
                        border: color::RED.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::RED,
                        text: color::LIGHT_BLACK,
                        border: color::RED.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::RED,
                        border: color::RED.into(),
                    }),
                },
                transparent: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                    }),
                },
                transparent_border: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::TRANSPARENT.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::GREEN.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::GREEN.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::TRANSPARENT.into(),
                    }),
                },
                container: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: None,
                    }),
                },
                container_border: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::TRANSPARENT.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::GREEN.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::GREEN.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::TRANSPARENT.into(),
                    }),
                },
                menu: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::WHITE,
                        border: color::TRANSPARENT.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::LIGHT_BLACK,
                        text: color::WHITE,
                        border: color::TRANSPARENT.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::LIGHT_BLACK,
                        text: color::WHITE,
                        border: color::TRANSPARENT.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::WHITE,
                        border: color::TRANSPARENT.into(),
                    }),
                },
                tab: Button {
                    active: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::GREY_7.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::LIGHT_BLACK,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: color::GREY_7.into(),
                    }),
                },
                link: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::TRANSPARENT.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::TRANSPARENT.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::TRANSPARENT.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::TRANSPARENT.into(),
                    }),
                },
            },
            cards: Cards {
                simple: ContainerPalette {
                    background: color::GREY_6,
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
                warning: ContainerPalette {
                    background: color::LIGHT_BLACK,
                    text: color::ORANGE.into(),
                    border: color::ORANGE.into(),
                },
                error: ContainerPalette {
                    background: color::LIGHT_BLACK,
                    text: color::RED.into(),
                    border: color::RED.into(),
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
            },
            pills: Pills {
                primary: ContainerPalette {
                    background: color::GREEN,
                    text: color::LIGHT_BLACK.into(),
                    border: color::TRANSPARENT.into(),
                },
                simple: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: color::GREY_3.into(),
                    border: color::GREY_3.into(),
                },
                warning: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: color::RED.into(),
                    border: color::RED.into(),
                },
                success: ContainerPalette {
                    background: color::GREEN,
                    text: color::LIGHT_BLACK.into(),
                    border: color::GREEN.into(),
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
                background: color::GREY_4,
                border: Some(color::GREY_4),
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
        }
    }
}
