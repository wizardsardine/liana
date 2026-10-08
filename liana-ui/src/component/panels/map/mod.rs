pub mod block;
pub mod leaf;
pub mod overlays;

pub use crate::widget::graph_view::geometry::{
    BLOCK_MIN_HEIGHT, BLOCK_WIDTH, INPUT_COLUMN_WIDTH, LEAF_HEIGHT, LEAF_WIDTH,
    MIDDLE_COLUMN_WIDTH, OUTPUT_COLUMN_WIDTH, SLOT_HEIGHT, U,
};

use iced::{
    border::Radius,
    widget::{container, Space},
    Background, Border, Color, Length,
};

use crate::{theme::Theme, widget::Container};

/// Solid `border-subtle` line, e.g. a 1 px column separator.
pub fn separator<'a, M: 'a>(
    width: impl Into<Length>,
    height: impl Into<Length>,
) -> Container<'a, M> {
    Container::new(Space::new())
        .width(width)
        .height(height)
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(theme.colors.text.border)),
            ..Default::default()
        })
}

/// Layer drawn over an item to fake an opacity: `background` at `alpha` on top.
pub fn scrim<'a, M: 'a>(
    background: fn(&Theme) -> Color,
    alpha: f32,
    radius: Radius,
) -> Container<'a, M> {
    Container::new(Space::new())
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |theme: &Theme| container::Style {
            background: Some(Background::Color(Color {
                a: alpha,
                ..background(theme)
            })),
            border: Border {
                radius,
                ..Default::default()
            },
            ..Default::default()
        })
}
