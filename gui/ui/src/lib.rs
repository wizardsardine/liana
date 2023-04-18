pub mod color;
pub mod component;
pub mod font;
pub mod icon;
pub mod image;
pub mod theme;
pub mod util;

pub mod widget {
    #![allow(dead_code)]
    use crate::theme::Theme;

    pub type Renderer = iced::Renderer<Theme>;
    pub type Element<'a, Message> = iced::Element<'a, Message, Renderer>;
    pub type Container<'a, Message> = iced::widget::Container<'a, Message, Renderer>;
    pub type Column<'a, Message> = iced::widget::Column<'a, Message, Renderer>;
    pub type Row<'a, Message> = iced::widget::Row<'a, Message, Renderer>;
    pub type Button<'a, Message> = iced::widget::Button<'a, Message, Renderer>;
    pub type Text<'a> = iced::widget::Text<'a, Renderer>;
    pub type Tooltip<'a> = iced::widget::Tooltip<'a, Renderer>;
    pub type ProgressBar = iced::widget::ProgressBar<Renderer>;
    pub type PickList<'a, Message> = iced::widget::PickList<'a, Message, Renderer>;
    pub type Scrollable<'a, Message> = iced::widget::Scrollable<'a, Message, Renderer>;
    pub type Svg = iced::widget::Svg<Renderer>;
}
