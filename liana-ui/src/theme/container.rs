use super::Theme;
use iced::{
    border::Radius,
    widget::container::{transparent, Catalog, Style, StyleFn},
    Background, Border,
};

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(transparent)
    }

    fn style(&self, class: &Self::Class<'_>) -> Style {
        class(self)
    }
}

pub fn background(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.background)),
        ..Default::default()
    }
}

pub fn panel_background(theme: &Theme) -> Style {
    let bg_color = theme.colors.general.background;
    Style {
        background: Some(Background::Color(bg_color)),
        border: Border {
            color: bg_color,
            width: 1.0,
            radius: Radius {
                top_left: 24.0,
                top_right: 0.0,
                bottom_right: 0.0,
                bottom_left: 0.0,
            },
        },
        ..Default::default()
    }
}

pub fn foreground(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.foreground)),
        ..Default::default()
    }
}

pub fn sidebar(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.menu_background)),
        ..Default::default()
    }
}

pub fn border(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.background)),
        ..Default::default()
    }
}

pub fn custom(color: iced::Color) -> Box<dyn Fn(&Theme) -> Style> {
    Box::new(move |_theme: &Theme| Style {
        background: Some(Background::Color(color)),
        ..Default::default()
    })
}
