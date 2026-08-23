use iced::{widget::container, Background, Border, Color};

use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub background: Color,
    pub cell: Color,
}

pub fn qr_code(_theme: &Theme) -> Style {
    Style {
        background: Color::WHITE,
        cell: Color::BLACK,
    }
}

const CELL_RADIUS: f32 = 2.0;
const CELL_BORDER: f32 = 2.0;

pub fn received_cell(theme: &Theme) -> container::Style {
    filled_cell(theme.colors.progress_bars.bar)
}

pub fn latest_cell(_theme: &Theme) -> container::Style {
    filled_cell(Color::WHITE)
}

pub fn missing_cell(_theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: Color::WHITE,
            width: CELL_BORDER,
            radius: CELL_RADIUS.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn filled_cell(color: Color) -> container::Style {
    container::Style {
        background: Some(Background::Color(color)),
        border: Border {
            radius: CELL_RADIUS.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}
