use crate::color;
use iced::Shadow;

pub mod liana;
pub mod liana_business;

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
    pub menus: Menus,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Text {
    pub primary: iced::Color,
    pub secondary: iced::Color,
    pub warning: iced::Color,
    pub success: iced::Color,
    pub error: iced::Color,
    pub accent: iced::Color,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct General {
    pub background: iced::Color,
    pub menu_background: iced::Color,
    pub foreground: iced::Color,
    pub scrollable: iced::Color,
    pub accent: iced::Color,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Buttons {
    pub transparent: Button,
    pub transparent_border: Button,
    pub clickable_card: Button,
    pub primary: Button,
    pub secondary: Button,
    pub tertiary: Button,
    pub destructive: Button,
    pub container: Button,
    pub container_border: Button,
    pub menu: Button,
    pub tab_menu: Button,
    pub tab: Button,
    pub link: Button,
    pub pick_list: Button,
    pub border_width: f32,
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
    pub shadow: Shadow,
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
    pub transparent: ContainerPalette,
    pub modal: ContainerPalette,
    pub border: ContainerPalette,
    pub invalid: ContainerPalette,
    pub warning: ContainerPalette,
    pub home_warning: ContainerPalette,
    pub home_hint: ContainerPalette,
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
    pub success: ContainerPalette,
    pub warning: ContainerPalette,
    pub soft_warning: ContainerPalette,
    pub internal: ContainerPalette,
    pub external: ContainerPalette,
    pub safety_net: ContainerPalette,
    pub fingerprint: ContainerPalette,
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

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Menus {
    pub pick_list: Menu,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Menu {
    pub border: iced::Color,
    pub text: iced::Color,
    pub selected_text: iced::Color,
    pub background: iced::Color,
    pub selected_background: iced::Color,
}
