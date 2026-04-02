use crate::color;

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

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
    pub togglers: Togglers,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Text {
    pub primary: iced::Color,
    pub secondary: iced::Color,
    pub warning: iced::Color,
    pub success: iced::Color,
    pub error: iced::Color,
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
    pub error: ContainerPalette,
    pub info: ContainerPalette,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Notifications {
    pub pending: ContainerPalette,
    pub error: ContainerPalette,
    pub success: ContainerPalette,
    pub warning: ContainerPalette,
    pub info: ContainerPalette,
    pub debug: ContainerPalette,
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

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Togglers {
    pub on: Toggler,
    pub off: Toggler,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Toggler {
    pub background: iced::Color,
    pub background_border: iced::Color,
    pub foreground: iced::Color,
    pub foreground_border: iced::Color,
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
            },
            buttons: Buttons {
                primary: Button {
                    active: ButtonPalette {
                        background: color::ORANGE,
                        text: color::LIGHT_BLACK,
                        border: Some(color::ORANGE),
                    },
                    hovered: ButtonPalette {
                        background: color::ORANGE,
                        text: color::LIGHT_BLACK,
                        border: Some(color::ORANGE),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::ORANGE,
                        text: color::LIGHT_BLACK,
                        border: Some(color::ORANGE),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: Some(color::TRANSPARENT_ORANGE),
                    }),
                },
                secondary: Button {
                    active: ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: Some(color::GREY_7),
                    },
                    hovered: ButtonPalette {
                        background: color::GREY_6,
                        text: color::ORANGE,
                        border: Some(color::TRANSPARENT_ORANGE),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::ORANGE,
                        border: Some(color::ORANGE),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: Some(color::TRANSPARENT_ORANGE),
                    }),
                },
                destructive: Button {
                    active: ButtonPalette {
                        background: color::GREY_6,
                        text: color::RED,
                        border: Some(color::RED),
                    },
                    hovered: ButtonPalette {
                        background: color::RED,
                        text: color::LIGHT_BLACK,
                        border: Some(color::RED),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::RED,
                        text: color::LIGHT_BLACK,
                        border: Some(color::RED),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::RED,
                        border: Some(color::RED),
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
                        border: color::ORANGE.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::ORANGE.into(),
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
                        border: color::ORANGE.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: color::ORANGE.into(),
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
                        border: Some(color::GREY_7),
                    },
                    hovered: ButtonPalette {
                        background: color::GREY_6,
                        text: color::ORANGE,
                        border: Some(color::ORANGE),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::LIGHT_BLACK,
                        text: color::ORANGE,
                        border: Some(color::ORANGE),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::GREY_6,
                        text: color::GREY_2,
                        border: Some(color::GREY_7),
                    }),
                },
                link: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_2,
                        border: Some(color::TRANSPARENT),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::ORANGE,
                        border: Some(color::TRANSPARENT_ORANGE),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::ORANGE,
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
                    background: color::LIGHT_BLUE,
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
                    background: color::ORANGE,
                    text: color::BLACK.into(),
                    border: color::TRANSPARENT.into(),
                },
                simple: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: color::GREY_3.into(),
                    border: color::GREY_3.into(),
                },
                warning: ContainerPalette {
                    background: color::WARN_ORANGE,
                    text: color::WHITE.into(),
                    border: Some(color::WARN_ORANGE),
                },
                success: ContainerPalette {
                    background: color::SUCCESS_GREEN,
                    text: color::WHITE.into(),
                    border: Some(color::SUCCESS_GREEN),
                },
                error: ContainerPalette {
                    background: color::ERROR_RED,
                    text: color::WHITE.into(),
                    border: Some(color::ERROR_RED),
                },
                info: ContainerPalette {
                    background: color::INFO_BLUE,
                    text: color::WHITE.into(),
                    border: Some(color::INFO_BLUE),
                },
            },
            notifications: Notifications {
                pending: ContainerPalette {
                    background: color::GREEN,
                    text: color::BLACK.into(),
                    border: Some(color::GREEN),
                },
                error: ContainerPalette {
                    background: color::ERROR_RED,
                    text: color::WHITE.into(),
                    border: Some(color::ERROR_RED),
                },
                success: ContainerPalette {
                    background: color::SUCCESS_GREEN,
                    text: color::WHITE.into(),
                    border: Some(color::SUCCESS_GREEN),
                },
                warning: ContainerPalette {
                    background: color::WARN_ORANGE,
                    text: color::WHITE.into(),
                    border: Some(color::WARN_ORANGE),
                },
                info: ContainerPalette {
                    background: color::INFO_BLUE,
                    text: color::WHITE.into(),
                    border: Some(color::INFO_BLUE),
                },
                debug: ContainerPalette {
                    background: color::GREY_4,
                    text: color::WHITE.into(),
                    border: Some(color::GREY_4),
                },
            },
            text_inputs: TextInputs {
                primary: TextInput {
                    active: TextInputPalette {
                        background: color::TRANSPARENT,
                        icon: color::TRANSPARENT,
                        placeholder: color::GREY_7,
                        value: color::GREY_2,
                        selection: color::ORANGE,
                        border: Some(color::GREY_7),
                    },
                    disabled: TextInputPalette {
                        background: color::TRANSPARENT,
                        icon: color::TRANSPARENT,
                        placeholder: color::GREY_7,
                        value: color::GREY_2,
                        selection: color::ORANGE,
                        border: Some(color::GREY_7),
                    },
                },
                invalid: TextInput {
                    active: TextInputPalette {
                        background: color::TRANSPARENT,
                        icon: color::TRANSPARENT,
                        placeholder: color::GREY_7,
                        value: color::GREY_2,
                        selection: color::ORANGE,
                        border: Some(color::RED),
                    },
                    disabled: TextInputPalette {
                        background: color::TRANSPARENT,
                        icon: color::TRANSPARENT,
                        placeholder: color::GREY_7,
                        value: color::TRANSPARENT,
                        selection: color::ORANGE,
                        border: Some(color::RED),
                    },
                },
            },
            checkboxes: Checkboxes {
                icon: color::ORANGE,
                text: color::GREY_2,
                background: color::GREY_4,
                border: Some(color::GREY_4),
            },
            radio_buttons: RadioButtons {
                dot: color::ORANGE,
                text: color::GREY_2,
                border: color::GREY_7,
            },
            sliders: Sliders {
                background: color::ORANGE,
                border: color::ORANGE,
                rail_border: None,
                rail_backgrounds: (color::ORANGE, color::GREY_7),
            },
            progress_bars: ProgressBars {
                bar: color::ORANGE,
                border: color::TRANSPARENT.into(),
                background: color::GREY_6,
            },
            rule: color::GREY_1,
            pane_grid: PaneGrid {
                background: color::BLACK,
                highlight_border: color::ORANGE,
                highlight_background: color::TRANSPARENT_ORANGE,
                picked_split: color::ORANGE,
                hovered_split: color::ORANGE,
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

impl Palette {
    pub fn dark() -> Self {
        Self::default()
    }

    pub fn light() -> Self {
        Self {
            general: General {
                background: color::LIGHT_BG,
                foreground: color::WARM_PAPER,
                scrollable: color::LIGHT_BORDER,
            },
            text: Text {
                primary: color::DARK_GRAY,
                secondary: color::GREY_5,
                warning: color::ORANGE,
                success: color::GREEN,
                error: color::RED,
            },
            buttons: Buttons {
                primary: Button {
                    active: ButtonPalette {
                        background: color::ORANGE,
                        text: color::BLACK,
                        border: Some(color::ORANGE),
                    },
                    hovered: ButtonPalette {
                        background: color::DARK_ORANGE,
                        text: color::BLACK,
                        border: Some(color::DARK_ORANGE),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::DARK_ORANGE,
                        text: color::BLACK,
                        border: Some(color::DARK_ORANGE),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::LIGHT_CARD_BG,
                        text: color::GREY_5,
                        border: Some(color::LIGHT_BORDER),
                    }),
                },
                secondary: Button {
                    active: ButtonPalette {
                        background: color::LIGHT_CARD_BG,
                        text: color::GREY_5,
                        border: Some(color::LIGHT_BORDER),
                    },
                    hovered: ButtonPalette {
                        background: color::LIGHT_CARD_BG,
                        text: color::ORANGE,
                        border: Some(color::TRANSPARENT_ORANGE),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::LIGHT_CARD_BG,
                        text: color::ORANGE,
                        border: Some(color::ORANGE),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::LIGHT_CARD_BG,
                        text: color::GREY_6,
                        border: Some(color::LIGHT_BORDER),
                    }),
                },
                destructive: Button {
                    active: ButtonPalette {
                        background: color::LIGHT_CARD_BG,
                        text: color::RED,
                        border: Some(color::RED),
                    },
                    hovered: ButtonPalette {
                        background: color::RED,
                        text: color::WHITE,
                        border: Some(color::RED),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::RED,
                        text: color::WHITE,
                        border: Some(color::RED),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::LIGHT_CARD_BG,
                        text: color::RED,
                        border: Some(color::RED),
                    }),
                },
                transparent: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: None,
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: None,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: None,
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: None,
                    }),
                },
                transparent_border: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: color::TRANSPARENT.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: color::ORANGE.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: color::ORANGE.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: color::TRANSPARENT.into(),
                    }),
                },
                container: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: None,
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: None,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: None,
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: None,
                    }),
                },
                container_border: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: color::TRANSPARENT.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: color::ORANGE.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: color::ORANGE.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: color::TRANSPARENT.into(),
                    }),
                },
                menu: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_GRAY,
                        border: color::TRANSPARENT.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::LIGHT_HOVER,
                        text: color::DARK_GRAY,
                        border: color::TRANSPARENT.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::LIGHT_HOVER,
                        text: color::DARK_GRAY,
                        border: color::TRANSPARENT.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::DARK_GRAY,
                        border: color::TRANSPARENT.into(),
                    }),
                },
                tab: Button {
                    active: ButtonPalette {
                        background: color::LIGHT_CARD_BG,
                        text: color::GREY_3,
                        border: Some(color::LIGHT_BORDER),
                    },
                    hovered: ButtonPalette {
                        background: color::LIGHT_CARD_BG,
                        text: color::ORANGE,
                        border: Some(color::ORANGE),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::LIGHT_BG,
                        text: color::ORANGE,
                        border: Some(color::ORANGE),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::LIGHT_CARD_BG,
                        text: color::GREY_3,
                        border: Some(color::LIGHT_BORDER),
                    }),
                },
                link: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: Some(color::TRANSPARENT),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::ORANGE,
                        border: Some(color::TRANSPARENT_ORANGE),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::ORANGE,
                        border: color::TRANSPARENT.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREY_3,
                        border: color::TRANSPARENT.into(),
                    }),
                },
            },
            cards: Cards {
                simple: ContainerPalette {
                    background: color::LIGHT_CARD_BG,
                    text: None,
                    border: Some(color::LIGHT_BORDER),
                },
                modal: ContainerPalette {
                    background: color::LIGHT_SURFACE,
                    text: None,
                    border: color::LIGHT_BORDER.into(),
                },
                border: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: None,
                    border: color::LIGHT_BORDER.into(),
                },
                invalid: ContainerPalette {
                    background: color::LIGHT_SURFACE,
                    text: color::RED.into(),
                    border: color::RED.into(),
                },
                warning: ContainerPalette {
                    background: color::LIGHT_SURFACE,
                    text: color::ORANGE.into(),
                    border: color::ORANGE.into(),
                },
                error: ContainerPalette {
                    background: color::LIGHT_SURFACE,
                    text: color::RED.into(),
                    border: color::RED.into(),
                },
            },
            banners: Banners {
                network: ContainerPalette {
                    background: color::LIGHT_BLUE,
                    text: color::DARK_GRAY.into(),
                    border: None,
                },
                warning: ContainerPalette {
                    background: color::ORANGE,
                    text: color::DARK_GRAY.into(),
                    border: None,
                },
            },
            badges: Badges {
                simple: ContainerPalette {
                    background: color::LIGHT_BORDER,
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
                    background: color::ORANGE,
                    text: color::WHITE.into(),
                    border: color::TRANSPARENT.into(),
                },
                simple: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: color::GREY_3.into(),
                    border: color::GREY_3.into(),
                },
                warning: ContainerPalette {
                    background: color::WARN_ORANGE,
                    text: color::WHITE.into(),
                    border: Some(color::WARN_ORANGE),
                },
                success: ContainerPalette {
                    background: color::SUCCESS_GREEN,
                    text: color::WHITE.into(),
                    border: Some(color::SUCCESS_GREEN),
                },
                error: ContainerPalette {
                    background: color::ERROR_RED,
                    text: color::WHITE.into(),
                    border: Some(color::ERROR_RED),
                },
                info: ContainerPalette {
                    background: color::INFO_BLUE,
                    text: color::WHITE.into(),
                    border: Some(color::INFO_BLUE),
                },
            },
            notifications: Notifications {
                pending: ContainerPalette {
                    background: color::GREEN,
                    text: color::BLACK.into(),
                    border: Some(color::GREEN),
                },
                error: ContainerPalette {
                    background: color::ERROR_RED,
                    text: color::WHITE.into(),
                    border: Some(color::ERROR_RED),
                },
                success: ContainerPalette {
                    background: color::SUCCESS_GREEN,
                    text: color::WHITE.into(),
                    border: Some(color::SUCCESS_GREEN),
                },
                warning: ContainerPalette {
                    background: color::WARN_ORANGE,
                    text: color::WHITE.into(),
                    border: Some(color::WARN_ORANGE),
                },
                info: ContainerPalette {
                    background: color::INFO_BLUE,
                    text: color::WHITE.into(),
                    border: Some(color::INFO_BLUE),
                },
                debug: ContainerPalette {
                    background: color::LIGHT_BORDER,
                    text: color::DARK_GRAY.into(),
                    border: Some(color::LIGHT_BORDER),
                },
            },
            text_inputs: TextInputs {
                primary: TextInput {
                    active: TextInputPalette {
                        background: color::LIGHT_BG,
                        icon: color::GREY_3,
                        placeholder: color::LIGHT_BORDER,
                        value: color::DARK_GRAY,
                        selection: color::ORANGE,
                        border: Some(color::LIGHT_BORDER),
                    },
                    disabled: TextInputPalette {
                        background: color::LIGHT_CARD_BG,
                        icon: color::LIGHT_BORDER,
                        placeholder: color::LIGHT_BORDER,
                        value: color::GREY_3,
                        selection: color::ORANGE,
                        border: Some(color::LIGHT_BORDER),
                    },
                },
                invalid: TextInput {
                    active: TextInputPalette {
                        background: color::LIGHT_BG,
                        icon: color::GREY_3,
                        placeholder: color::LIGHT_BORDER,
                        value: color::DARK_GRAY,
                        selection: color::ORANGE,
                        border: Some(color::RED),
                    },
                    disabled: TextInputPalette {
                        background: color::LIGHT_CARD_BG,
                        icon: color::LIGHT_BORDER,
                        placeholder: color::LIGHT_BORDER,
                        value: color::TRANSPARENT,
                        selection: color::ORANGE,
                        border: Some(color::RED),
                    },
                },
            },
            checkboxes: Checkboxes {
                icon: color::ORANGE,
                text: color::GREY_3,
                background: color::LIGHT_BORDER,
                border: Some(color::LIGHT_BORDER),
            },
            radio_buttons: RadioButtons {
                dot: color::ORANGE,
                text: color::GREY_3,
                border: color::LIGHT_BORDER,
            },
            sliders: Sliders {
                background: color::ORANGE,
                border: color::ORANGE,
                rail_border: None,
                rail_backgrounds: (color::ORANGE, color::LIGHT_BORDER),
            },
            progress_bars: ProgressBars {
                bar: color::ORANGE,
                border: color::TRANSPARENT.into(),
                background: color::LIGHT_CARD_BG,
            },
            rule: color::LIGHT_BORDER,
            pane_grid: PaneGrid {
                background: color::LIGHT_BG,
                highlight_border: color::ORANGE,
                highlight_background: color::TRANSPARENT_ORANGE,
                picked_split: color::ORANGE,
                hovered_split: color::ORANGE,
            },
            togglers: Togglers {
                on: Toggler {
                    background: color::GREEN,
                    background_border: color::GREEN,
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

    pub fn from_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
        }
    }
}
