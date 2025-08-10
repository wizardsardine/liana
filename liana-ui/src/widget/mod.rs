mod cursor;
mod editor;
mod menu;
pub mod text_input;

use crate::theme::Theme;

pub type Element<'a, Message> = iced::Element<'a, Message, Theme, iced::Renderer>;
pub type Container<'a, Message> = iced::widget::Container<'a, Message, Theme, iced::Renderer>;
pub type Column<'a, Message> = iced::widget::Column<'a, Message, Theme, iced::Renderer>;
pub type Row<'a, Message> = iced::widget::Row<'a, Message, Theme, iced::Renderer>;
pub type Button<'a, Message> = iced::widget::Button<'a, Message, Theme, iced::Renderer>;
pub type CheckBox<'a, Message> = iced::widget::Checkbox<'a, Message, Theme, iced::Renderer>;
pub type Text<'a> = iced::widget::Text<'a, Theme, iced::Renderer>;
pub type TextInput<'a, Message> = text_input::TextInput<'a, Message, Theme, iced::Renderer>;
pub type Tooltip<'a> = iced::widget::Tooltip<'a, Theme, iced::Renderer>;
pub type ProgressBar<'a> = iced::widget::ProgressBar<'a, Theme>;
pub type PickList<'a, T, L, V, Message> =
    iced::widget::PickList<'a, T, L, V, Message, Theme, iced::Renderer>;
pub type Scrollable<'a, Message> = iced::widget::Scrollable<'a, Message, Theme, iced::Renderer>;
pub type Svg<'a> = iced::widget::Svg<'a, Theme>;
