pub mod color;
pub mod component;
pub mod font;
pub mod icon;
pub mod image;
pub mod theme;

pub mod widget {
    #![allow(dead_code)]
    use crate::theme::Theme;

    pub type Renderer = iced::Renderer;
    pub type Element<'a, Message> = iced::Element<'a, Message, Theme, Renderer>;
    pub type Container<'a, Message> = iced::widget::Container<'a, Message, Theme, Renderer>;
    pub type Column<'a, Message> = iced::widget::Column<'a, Message, Theme, Renderer>;
    pub type Row<'a, Message> = iced::widget::Row<'a, Message, Theme, Renderer>;
    pub type Button<'a, Message> = iced::widget::Button<'a, Message, Theme, Renderer>;
    pub type CheckBox<'a, Message> = iced::widget::Checkbox<'a, Message, Theme, Renderer>;
    pub type Text<'a> = iced::widget::Text<'a, Theme, Renderer>;
    pub type TextInput<'a, Message> = iced::widget::TextInput<'a, Message, Theme, Renderer>;
    pub type Tooltip<'a> = iced::widget::Tooltip<'a, Theme, Renderer>;
    pub type ProgressBar = iced::widget::ProgressBar<Theme>;
    pub type PickList<'a, T, L, V, Message> =
        iced::widget::PickList<'a, T, L, V, Message, Theme, Renderer>;
    pub type Scrollable<'a, Message> = iced::widget::Scrollable<'a, Message, Theme, Renderer>;
    pub type Svg = iced::widget::Svg<Theme>;
}
