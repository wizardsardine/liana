use crate::color;

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Palette {
    pub general: General,
    pub text: Text,
    pub buttons: Buttons,
    pub cards: Cards,
    pub banners: Banners,
    pub badges: Badges,
    pub pills: Pills,
    pub notifications: Notifications,
    pub text_inputs: TextInputs,
    pub checkboxes: Checkboxes,
    pub radio_buttons: RadioButtons,
    pub sliders: Sliders,
    pub progress_bars: ProgressBars,
    pub rule: iced::Color,
    pub pane_grid: PaneGrid,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Text {
    pub primary: iced::Color,
    pub secondary: iced::Color,
    pub warning: iced::Color,
    pub success: iced::Color,
    pub error: iced::Color,
    pub payjoin: iced::Color,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct General {
    pub background: iced::Color,
    pub foreground: iced::Color,
    pub scrollable: iced::Color,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Buttons {
    pub transparent: Button,
    pub transparent_border: Button,
    pub primary: Button,
    pub secondary: Button,
    pub destructive: Button,
    pub container: Button,
    pub container_border: Button,
    pub menu: Button,
    pub tab: Button,
    pub link: Button,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Button {
    pub active: ButtonPalette,
    pub hovered: ButtonPalette,
    pub pressed: Option<ButtonPalette>,
    pub disabled: Option<ButtonPalette>,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ButtonPalette {
    pub background: iced::Color,
    pub text: iced::Color,
    pub border: Option<iced::Color>,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Containers {
    pub border: ContainerPalette,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ContainerPalette {
    pub background: iced::Color,
    pub text: Option<iced::Color>,
    pub border: Option<iced::Color>,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Cards {
    pub simple: ContainerPalette,
    pub modal: ContainerPalette,
    pub border: ContainerPalette,
    pub invalid: ContainerPalette,
    pub warning: ContainerPalette,
    pub error: ContainerPalette,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Banners {
    pub network: ContainerPalette,
    pub warning: ContainerPalette,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Badges {
    pub simple: ContainerPalette,
    pub bitcoin: ContainerPalette,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Pills {
    pub simple: ContainerPalette,
    pub primary: ContainerPalette,
    pub success: ContainerPalette,
    pub warning: ContainerPalette,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Notifications {
    pub pending: ContainerPalette,
    pub error: ContainerPalette,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct TextInputs {
    pub primary: TextInput,
    pub invalid: TextInput,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct TextInput {
    pub active: TextInputPalette,
    pub disabled: TextInputPalette,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct TextInputPalette {
    pub background: iced::Color,
    pub icon: iced::Color,
    pub placeholder: iced::Color,
    pub value: iced::Color,
    pub selection: iced::Color,
    pub border: Option<iced::Color>,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Checkboxes {
    pub icon: iced::Color,
    pub text: iced::Color,
    pub background: iced::Color,
    pub border: Option<iced::Color>,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct RadioButtons {
    pub dot: iced::Color,
    pub text: iced::Color,
    pub border: iced::Color,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Sliders {
    pub background: iced::Color,
    pub border: iced::Color,
    pub rail_border: Option<iced::Color>,
    pub rail_backgrounds: (iced::Color, iced::Color),
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ProgressBars {
    pub background: iced::Color,
    pub bar: iced::Color,
    pub border: Option<iced::Color>,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct PaneGrid {
    pub background: iced::Color,
    pub highlight_border: iced::Color,
    pub highlight_background: iced::Color,
    pub picked_split: iced::Color,
    pub hovered_split: iced::Color,
}

impl std::default::Default for Palette {
    fn default() -> Self {
        Self {
            general: General {
                background: color::LIGHT_BLACK,
                foreground: color::BLACK,
                scrollable: color::GREY_7,
            },
            text: Text {
                primary: color::WHITE,
                secondary: color::GREY_2,
                warning: color::ORANGE,
                success: color::GREEN,
                error: color::RED,
                payjoin: color::PAYJOIN_PINK,
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
        }
    }
}
