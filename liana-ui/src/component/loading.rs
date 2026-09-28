use iced::{
    widget::{column, Space},
    Length,
};

use crate::{
    component::text,
    spacing::VSpacing,
    widget::{Container, Element, ProgressBar, SpaceExt},
};

pub const CONTENT_WIDTH: f32 = 800.0;

pub fn layout<'a, M: Clone + 'a>(
    content: impl Into<Element<'a, M>>,
    warning: Option<Element<'a, M>>,
) -> Element<'a, M> {
    column![
        warning,
        Container::new(content)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .padding(50)
    ]
    .into()
}

/// Vertically centers `content`, the footer below it does not move it.
pub fn centered<'a, M: Clone + 'a>(
    content: impl Into<Element<'a, M>>,
    footer: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let content = column![
        Space::fill_height(),
        content.into(),
        column![footer].height(Length::Fill),
    ]
    .spacing(VSpacing::XL)
    .width(Length::Fill)
    .max_width(CONTENT_WIDTH);

    Container::new(content)
        .center_x(Length::Fill)
        .height(Length::Fill)
        .padding(50)
        .into()
}

/// `value` ranges from 0.0 to 1.0.
pub fn progress<'a, M: Clone + 'a>(
    label: String,
    value: f32,
    footer: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let bar = column![
        text::new::caption(label),
        ProgressBar::new(0.0..=1.0, value).length(Length::Fill)
    ]
    .spacing(VSpacing::M);

    centered(bar, footer)
}
