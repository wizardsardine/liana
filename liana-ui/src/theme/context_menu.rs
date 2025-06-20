use iced::Background;
use iced_aw::widget::context_menu::{Catalog, Status, Style};

use super::Theme;

impl Catalog for Theme {
    type Class<'a> = Box<dyn Fn(&Theme, Status) -> Style + 'a>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(primary)
    }

    fn style(&self, class: &Self::Class<'_>, status: Status) -> Style {
        class(self, status)
    }
}

pub fn primary(_theme: &Theme, _status: Status) -> Style {
    Style {
        background: Background::Color(iced::Color::TRANSPARENT),
    }
}
