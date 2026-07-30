use coincube_ui::component::text::Text;
use coincube_ui::{color, component::text::*, icon, theme, widget::*};
use iced::{
    widget::{button as iced_button, container, Column, Container, Row, Space},
    Alignment, Background, Length,
};

/// The shell every picker overlay sits on: a title above its rows, on an
/// opaque card.
///
/// The card is load-bearing, not decoration. The modal widget only dims the
/// page behind it, so a bare column of rows lets whatever is underneath — a QR
/// code, a form, the two-card selector — read straight through the gaps between
/// rows, and the picker looks broken. Wrapping the list gives it a surface of
/// its own.
pub fn picker_modal_card<'a, M>(title: &str, body: impl Into<Element<'a, M>>) -> Element<'a, M>
where
    M: Clone + 'a,
{
    Container::new(
        Column::new()
            .spacing(12)
            .push(text(title.to_string()).size(16).bold())
            .push(body.into()),
    )
    .padding(24)
    .max_width(460)
    .style(theme::card::modal)
    .into()
}

/// A single row in a picker modal — an asset or wallet icon, a label with an optional
/// balance subtitle, a right-aligned network badge, and a selection checkmark.
///
/// Reused across Liquid Send's asset pickers and the Transfer flow's wallet picker.
pub fn picker_row<'a, M>(
    ico: impl Into<Element<'a, M>>,
    label: &str,
    balance: &str,
    network: &str,
    is_selected: bool,
    on_press: M,
) -> Element<'a, M>
where
    M: Clone + 'a,
{
    let mut row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(ico)
        .push(
            Column::new()
                .spacing(2)
                .push(
                    text(label.to_string())
                        .size(P1_SIZE)
                        .bold()
                        .style(theme::text::primary),
                )
                .push_maybe(if !balance.is_empty() {
                    Some(
                        text(balance.to_string())
                            .size(P2_SIZE)
                            .style(theme::text::secondary),
                    )
                } else {
                    None
                }),
        )
        .push(
            Container::new(text(network.to_uppercase()).size(10).color(color::ORANGE))
                .padding([2, 6])
                .style(|_: &theme::Theme| container::Style {
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..Default::default()
                }),
        )
        .push(Space::new().width(Length::Fill));

    if is_selected {
        row = row.push(icon::check2_icon().size(18).color(color::ORANGE));
    }

    iced_button(
        Container::new(row)
            .padding([12, 16])
            .width(Length::Fill)
            .style(if is_selected {
                picker_row_selected
            } else {
                theme::card::simple
            }),
    )
    .on_press(on_press)
    .style(|_: &theme::Theme, _| iced_button::Style {
        background: Some(Background::Color(color::TRANSPARENT)),
        border: iced::Border {
            radius: 12.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .width(Length::Fill)
    .into()
}

/// Selected row in picker modals — orange border with subtle tinted background.
pub fn picker_row_selected(theme: &theme::Theme) -> container::Style {
    let bg = match theme.mode {
        coincube_ui::theme::palette::ThemeMode::Dark => iced::color!(0x1a1a10),
        coincube_ui::theme::palette::ThemeMode::Light => iced::color!(0xFFF5E6),
    };
    container::Style {
        background: Some(Background::Color(bg)),
        border: iced::Border {
            color: color::ORANGE,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    }
}
