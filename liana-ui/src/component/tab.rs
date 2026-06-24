use iced::{
    alignment::Vertical,
    border::Radius,
    widget::{button, container, row, tooltip::Position, Space},
    Background, Border, Length,
};

use crate::{color::TRANSPARENT, component::tooltip_custom, font, theme, widget::*};

const STRIP_SPACING: u32 = 4;
const TAB_PADDING: [u16; 2] = [10, 18];
const TAB_LABEL_SIZE: u32 = 15;
const TAB_UNDERLINE_HEIGHT: f32 = 2.0;
const STRIP_UNDERLINE_HEIGHT: f32 = 1.0;
const DOT_SIZE: f32 = 8.0;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Dot {
    Ready,
    Pending,
}

pub fn tab_header<'a, K: PartialEq + 'a, M: Clone + 'a>(
    items: &[(K, &'a str, Option<Dot>)],
    active: &K,
    on_select: impl Fn(&K) -> M + 'a,
) -> Element<'a, M> {
    let tabs = items.iter().fold(
        row![]
            .spacing(STRIP_SPACING)
            .align_y(Vertical::Center)
            .width(Length::Fill),
        |row, (key, label, dot)| row.push(tab_item(label, *dot, key == active, on_select(key))),
    );

    iced::widget::column![tabs, strip_line()]
        .spacing(0)
        .width(Length::Fill)
        .into()
}

fn tab_item<'a, M: Clone + 'a>(
    label: &'a str,
    dot: Option<Dot>,
    active: bool,
    on_press: M,
) -> Button<'a, M> {
    let label_color = move |theme: &theme::Theme| {
        if active {
            theme.colors.tabs.active
        } else {
            theme.colors.tabs.inactive
        }
    };
    let font = if active {
        font::MANROPE_SEMIBOLD
    } else {
        font::MANROPE_MEDIUM
    };
    let label = Text::new(label)
        .size(TAB_LABEL_SIZE)
        .font(font)
        .style(move |theme| iced::widget::text::Style {
            color: Some(label_color(theme)),
        });
    let content = if let Some(dot) = dot.map(dot_view) {
        row![label, dot]
    } else {
        row![label]
    }
    .spacing(8)
    .align_y(Vertical::Center);
    let underline = Container::new(Space::new())
        .height(TAB_UNDERLINE_HEIGHT)
        .width(Length::Fill)
        .style(move |theme| {
            line_style(if active {
                theme.colors.tabs.active
            } else {
                TRANSPARENT
            })
        });
    let content = iced::widget::column![Container::new(content).padding(TAB_PADDING), underline]
        .spacing(0)
        .width(Length::Shrink);

    Button::new(content)
        .padding(0)
        .style(tab_button)
        .on_press(on_press)
}

fn dot_view<'a, M: 'a>(dot: Dot) -> Element<'a, M> {
    let help = match dot {
        Dot::Ready => "Ready",
        Dot::Pending => "Pending",
    };
    let dot = Container::new(Space::new())
        .width(DOT_SIZE)
        .height(DOT_SIZE)
        .style(move |theme| {
            let color = match dot {
                Dot::Ready => theme.colors.tabs.dot_ready,
                Dot::Pending => theme.colors.tabs.dot_pending,
            };

            container::Style {
                background: Some(Background::Color(color)),
                border: Border {
                    radius: Radius::from(DOT_SIZE / 2.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        });

    tooltip_custom(help, dot, Position::Top).into()
}

fn strip_line<'a, M: 'a>() -> Container<'a, M> {
    Container::new(Space::new())
        .height(STRIP_UNDERLINE_HEIGHT)
        .width(Length::Fill)
        .style(|theme| line_style(theme.colors.tabs.strip))
}

fn line_style(color: iced::Color) -> container::Style {
    container::Style {
        background: Some(Background::Color(color)),
        ..Default::default()
    }
}

fn tab_button(_theme: &theme::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(TRANSPARENT)),
        border: Border {
            color: TRANSPARENT,
            ..Default::default()
        },
        ..Default::default()
    }
}
