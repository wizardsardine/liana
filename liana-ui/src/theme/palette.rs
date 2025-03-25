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

impl std::default::Default for Palette {
    fn default() -> Self {
        Self {
            general: General {
                background: color::TRANSPARENT,
                foreground: color::TRANSPARENT,
                scrollable: color::GREEN,
            },
            text: Text {
                primary: color::GREEN,
                secondary: color::GREEN,
                warning: color::GREEN,
                success: color::GREEN,
                error: color::GREEN,
            },
            buttons: Buttons {
                primary: Button {
                    active: ButtonPalette {
                        background: color::GREEN,
                        text: color::BLACK,
                        border: color::GREEN.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::GREEN,
                        text: color::BLACK,
                        border: color::GREEN.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::GREEN,
                        text: color::BLACK,
                        border: color::GREEN.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::BLACK,
                        text: color::GREEN,
                        border: color::BLACK.into(),
                    }),
                },
                secondary: Button {
                    active: ButtonPalette {
                        background: color::BLACK,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::BLACK,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::BLACK,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::BLACK,
                        text: color::GREEN,
                        border: color::BLACK.into(),
                    }),
                },
                destructive: Button {
                    active: ButtonPalette {
                        background: color::BLACK,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::GREEN,
                        text: color::BLACK,
                        border: color::GREEN.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::GREEN,
                        text: color::BLACK,
                        border: color::GREEN.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::BLACK,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    }),
                },
                transparent: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: None,
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: None,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: None,
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: None,
                    }),
                },
                transparent_border: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::TRANSPARENT.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::TRANSPARENT.into(),
                    }),
                },
                container: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: None,
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: None,
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: None,
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: None,
                    }),
                },
                container_border: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::TRANSPARENT.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::GREEN.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::TRANSPARENT.into(),
                    }),
                },
                menu: Button {
                    active: ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::TRANSPARENT.into(),
                    },
                    hovered: ButtonPalette {
                        background: color::BLACK,
                        text: color::GREEN,
                        border: color::TRANSPARENT.into(),
                    },
                    pressed: Some(ButtonPalette {
                        background: color::BLACK,
                        text: color::GREEN,
                        border: color::TRANSPARENT.into(),
                    }),
                    disabled: Some(ButtonPalette {
                        background: color::TRANSPARENT,
                        text: color::GREEN,
                        border: color::TRANSPARENT.into(),
                    }),
                },
            },
            cards: Cards {
                simple: ContainerPalette {
                    background: color::BLACK,
                    text: None,
                    border: Some(color::TRANSPARENT),
                },
                modal: ContainerPalette {
                    background: color::BLACK,
                    text: None,
                    border: color::GREEN.into(),
                },
                border: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: None,
                    border: color::GREEN.into(),
                },
                invalid: ContainerPalette {
                    background: color::BLACK,
                    text: color::GREEN.into(),
                    border: color::GREEN.into(),
                },
                warning: ContainerPalette {
                    background: color::BLACK,
                    text: color::GREEN.into(),
                    border: color::GREEN.into(),
                },
                error: ContainerPalette {
                    background: color::BLACK,
                    text: color::GREEN.into(),
                    border: color::GREEN.into(),
                },
            },
            banners: Banners {
                network: ContainerPalette {
                    background: color::GREEN,
                    text: color::BLACK.into(),
                    border: None,
                },
                warning: ContainerPalette {
                    background: color::GREEN,
                    text: color::BLACK.into(),
                    border: None,
                },
            },
            badges: Badges {
                simple: ContainerPalette {
                    background: color::BLACK,
                    text: None,
                    border: color::TRANSPARENT.into(),
                },
                bitcoin: ContainerPalette {
                    background: color::GREEN,
                    text: color::GREEN.into(),
                    border: color::TRANSPARENT.into(),
                },
            },
            pills: Pills {
                primary: ContainerPalette {
                    background: color::GREEN,
                    text: color::BLACK.into(),
                    border: color::TRANSPARENT.into(),
                },
                simple: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: color::GREEN.into(),
                    border: color::GREEN.into(),
                },
                warning: ContainerPalette {
                    background: color::TRANSPARENT,
                    text: color::GREEN.into(),
                    border: color::GREEN.into(),
                },
                success: ContainerPalette {
                    background: color::GREEN,
                    text: color::BLACK.into(),
                    border: color::GREEN.into(),
                },
            },
            notifications: Notifications {
                pending: ContainerPalette {
                    background: color::GREEN,
                    text: color::BLACK.into(),
                    border: Some(color::GREEN),
                },
                error: ContainerPalette {
                    background: color::GREEN,
                    text: color::BLACK.into(),
                    border: Some(color::GREEN),
                },
            },
            text_inputs: TextInputs {
                primary: TextInput {
                    active: TextInputPalette {
                        background: color::BLACK,
                        icon: color::TRANSPARENT,
                        placeholder: color::GREY_7,
                        value: color::GREEN,
                        selection: color::GREEN,
                        border: Some(color::GREEN),
                    },
                    disabled: TextInputPalette {
                        background: color::BLACK,
                        icon: color::TRANSPARENT,
                        placeholder: color::GREY_7,
                        value: color::GREEN,
                        selection: color::GREEN,
                        border: Some(color::GREEN),
                    },
                },
                invalid: TextInput {
                    active: TextInputPalette {
                        background: color::BLACK,
                        icon: color::TRANSPARENT,
                        placeholder: color::GREY_7,
                        value: color::GREEN,
                        selection: color::GREEN,
                        border: Some(color::GREEN),
                    },
                    disabled: TextInputPalette {
                        background: color::BLACK,
                        icon: color::TRANSPARENT,
                        placeholder: color::GREY_7,
                        value: color::TRANSPARENT,
                        selection: color::GREEN,
                        border: Some(color::GREEN),
                    },
                },
            },
            checkboxes: Checkboxes {
                icon: color::GREEN,
                text: color::GREEN,
                background: color::GREY_7,
                border: Some(color::BLACK),
            },
            radio_buttons: RadioButtons {
                dot: color::GREEN,
                text: color::GREEN,
                border: color::BLACK,
            },
            sliders: Sliders {
                background: color::GREEN,
                border: color::GREEN,
                rail_border: None,
                rail_backgrounds: (color::GREEN, color::TRANSPARENT),
            },
            progress_bars: ProgressBars {
                bar: color::GREEN,
                border: color::TRANSPARENT.into(),
                background: color::BLACK,
            },
            rule: color::GREEN,
        }
    }
}
