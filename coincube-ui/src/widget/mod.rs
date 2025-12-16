mod cursor;
mod editor;
mod menu;
pub mod modal;
pub mod text_input;

use crate::theme::Theme;

pub type Renderer = iced::Renderer;
pub type Element<'a, Message> = iced::Element<'a, Message, Theme, Renderer>;
pub type Container<'a, Message> = iced::widget::Container<'a, Message, Theme, Renderer>;
pub type Column<'a, Message> = iced::widget::Column<'a, Message, Theme, Renderer>;
pub type Row<'a, Message> = iced::widget::Row<'a, Message, Theme, Renderer>;
pub type Button<'a, Message> = iced::widget::Button<'a, Message, Theme, Renderer>;
pub type CheckBox<'a, Message> = iced::widget::Checkbox<'a, Message, Theme, Renderer>;
pub type Text<'a> = iced::widget::Text<'a, Theme, Renderer>;
pub type Toggler<'a, Message> = iced::widget::Toggler<'a, Message, Theme, Renderer>;
pub type TextInput<'a, Message> = text_input::TextInput<'a, Message, Theme, Renderer>;
pub type Tooltip<'a> = iced::widget::Tooltip<'a, Theme, Renderer>;
pub type ProgressBar<'a> = iced::widget::ProgressBar<'a, Theme>;
pub type PickList<'a, T, L, V, Message> =
    iced::widget::PickList<'a, T, L, V, Message, Theme, iced::Renderer>;
pub type Scrollable<'a, Message> = iced::widget::Scrollable<'a, Message, Theme, iced::Renderer>;
pub type Svg<'a> = iced::widget::Svg<'a, Theme>;

/// Extension trait to add `push_maybe` to Row (removed in iced 0.14)
pub trait RowExt<'a, Message> {
    fn push_maybe(self, child: Option<impl Into<Element<'a, Message>>>) -> Self;
}

impl<'a, Message> RowExt<'a, Message> for Row<'a, Message> {
    fn push_maybe(self, child: Option<impl Into<Element<'a, Message>>>) -> Self {
        if let Some(child) = child {
            self.push(child)
        } else {
            self
        }
    }
}

/// Extension trait to add `push_maybe` to Column (removed in iced 0.14)
pub trait ColumnExt<'a, Message> {
    fn push_maybe(self, child: Option<impl Into<Element<'a, Message>>>) -> Self;
}

impl<'a, Message> ColumnExt<'a, Message> for Column<'a, Message> {
    fn push_maybe(self, child: Option<impl Into<Element<'a, Message>>>) -> Self {
        if let Some(child) = child {
            self.push(child)
        } else {
            self
        }
    }
}
