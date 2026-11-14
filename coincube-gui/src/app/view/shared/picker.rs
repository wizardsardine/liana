use coincube_ui::component::text::Text;
use coincube_ui::widget::{Column as UiColumn, Container as UiContainer, Row as UiRow};
use coincube_ui::{color, component::text::*, icon, theme, widget::*};
use iced::{
    widget::{button as iced_button, container, Column, Container, Row, Space},
    Alignment, Background, Length,
};

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
                picker_row_idle
            }),
    )
    .on_press(on_press)
    .style(|_: &theme::Theme, _| iced_button::Style {
        background: Some(Background::Color(color::TRANSPARENT)),
        border: iced::Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .width(Length::Fill)
    .into()
}

/// An unselected picker row: the simple card surface with the square 8px
/// corner every button takes since canvas v122.
pub fn picker_row_idle(theme: &theme::Theme) -> container::Style {
    let mut style = theme::card::simple(theme);
    style.border.radius = 8.0.into();
    style
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
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

// ── Route dropdowns ─────────────────────────────────────────────────────────
//
// Spark Send and Receive pick each side of a route from a menu anchored under
// its tile, not a modal: the form stays in view, and the menu is the tile's
// own width, on the tile's own surface.

/// The gap between a route tile and the menu under it.
pub const DROPDOWN_GAP: f32 = 6.0;
/// The least a dropdown keeps of a short window before it scrolls.
pub const DROPDOWN_MIN_HEIGHT: f32 = 220.0;
/// Logo size in a dropdown row.
pub const DROPDOWN_LOGO: f32 = 28.0;

/// A route tile with its menu under it while `menu` is set. The menu floats
/// over what follows (it takes no room), is as wide as the tile, scrolls when
/// the window is too short for it, and `on_dismiss` closes it on a press
/// outside it or Escape.
pub fn anchored_dropdown<'a, M>(
    tile: impl Into<Element<'a, M>>,
    menu: Option<Element<'a, M>>,
    on_dismiss: M,
) -> Element<'a, M>
where
    M: Clone + 'a,
{
    let column = UiColumn::new().width(Length::Fill).push(tile);
    match menu {
        Some(menu) => column
            .push(
                coincube_ui::widget::floating::Floating::new(menu)
                    .gap(DROPDOWN_GAP)
                    .on_dismiss(on_dismiss)
                    .fit_height_to_window(DROPDOWN_MIN_HEIGHT),
            )
            .into(),
        None => column.into(),
    }
}

/// The menu's surface: the route tile's background and line, 8px corners and a
/// shadow to lift it off the form. `content` scrolls when the window caps the
/// menu's height.
pub fn dropdown_menu<'a, M>(content: UiColumn<'a, M>) -> Element<'a, M>
where
    M: Clone + 'a,
{
    UiContainer::new(
        // The scrollable clips what it draws, so the rows keep 4px clear on
        // every side for the focus ring (it paints 4px outside a row), and 4px
        // between them so a ring never covers the next row. The extra room on
        // the right is the scrollbar's, so it never draws over a row's check.
        iced::widget::scrollable(content.spacing(4).padding(iced::Padding {
            top: 4.0,
            bottom: 4.0,
            left: 4.0,
            right: 8.0,
        }))
        .direction(iced::widget::scrollable::Direction::Vertical(
            iced::widget::scrollable::Scrollbar::new()
                .width(4)
                .scroller_width(4),
        ))
        .height(Length::Shrink),
    )
    .padding(2)
    .width(Length::Fill)
    .style(dropdown_menu_style)
    .into()
}

pub fn dropdown_menu_style(theme: &theme::Theme) -> container::Style {
    let shell = &theme.colors.shell;
    container::Style {
        background: Some(Background::Color(shell.background)),
        border: iced::Border {
            color: shell.line,
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: iced::Shadow {
            color: shell.shadow,
            offset: iced::Vector::new(0.0, 12.0),
            blur_radius: 32.0,
        },
        ..Default::default()
    }
}

/// A group heading inside a dropdown ("Bitcoin", "Stablecoin (cross-chain)").
pub fn dropdown_heading<'a, M: 'a>(label: &str) -> Element<'a, M> {
    UiContainer::new(
        text(label.to_uppercase())
            .size(CAPTION_SIZE)
            .bold()
            .style(theme::text::secondary),
    )
    .padding(iced::Padding {
        top: 8.0,
        bottom: 4.0,
        left: 10.0,
        right: 10.0,
    })
    .into()
}

/// A note at the foot of a dropdown, under a rule.
pub fn dropdown_note<'a, M: 'a>(note: &str) -> Element<'a, M> {
    UiColumn::new()
        .spacing(8)
        .push(Space::new().height(Length::Fixed(4.0)))
        .push(
            iced::widget::rule::horizontal(1).style(|theme: &theme::Theme| {
                iced::widget::rule::Style {
                    color: theme.colors.shell.line,
                    radius: 0.0.into(),
                    fill_mode: iced::widget::rule::FillMode::Full,
                    snap: true,
                }
            }),
        )
        .push(
            UiContainer::new(
                text(note.to_string())
                    .size(CAPTION_SIZE + 1)
                    .style(theme::text::secondary),
            )
            .padding([2, 10]),
        )
        .into()
}

/// One option in a dropdown: logo, name, network chip, an optional line
/// under it, and a check when selected. `on_press: None` shows it disabled.
#[allow(clippy::too_many_arguments)]
pub fn dropdown_option<'a, M>(
    id: impl Into<iced::advanced::widget::Id>,
    logo: Element<'a, M>,
    title: &str,
    chip: &str,
    sub: Option<String>,
    selected: bool,
    on_press: Option<M>,
    focus_intent: impl FnOnce(iced::advanced::widget::Id) -> M,
) -> Element<'a, M>
where
    M: Clone + 'static,
{
    let enabled = on_press.is_some();
    let ink = move |theme: &theme::Theme| iced::widget::text::Style {
        color: Some(if enabled {
            theme.colors.shell.ink
        } else {
            theme.colors.shell.muted
        }),
    };
    let chip_color = move |theme: &theme::Theme| {
        if enabled {
            theme.colors.shell.accent_ink
        } else {
            theme.colors.shell.muted
        }
    };
    let mut lines = UiColumn::new().spacing(1).width(Length::Fill).push(
        UiRow::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(text(title.to_string()).size(P1_SIZE).bold().style(ink))
            .push(
                UiContainer::new(text(chip.to_uppercase()).size(10).style(
                    move |theme: &theme::Theme| iced::widget::text::Style {
                        color: Some(chip_color(theme)),
                    },
                ))
                .padding([1, 5])
                .style(move |theme: &theme::Theme| container::Style {
                    border: iced::Border {
                        color: chip_color(theme),
                        width: 1.0,
                        radius: 5.0.into(),
                    },
                    ..Default::default()
                }),
            ),
    );
    if let Some(sub) = sub {
        lines = lines.push(
            text(sub)
                .size(CAPTION_SIZE + 1)
                .style(theme::text::secondary)
                .wrapping(iced::widget::text::Wrapping::None),
        );
    }
    let row = UiRow::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(logo)
        .push(lines)
        .push(
            UiContainer::new(if selected {
                Element::from(icon::check2_icon().size(16).color(color::ORANGE))
            } else {
                Space::new().into()
            })
            .width(Length::Fixed(18.0)),
        );
    coincube_ui::widget::action::Action::new(
        id.into(),
        UiContainer::new(row)
            .padding([6, 10])
            .width(Length::Fill)
            .height(Length::Shrink),
        focus_intent,
    )
    .padding(0)
    .width(Length::Fill)
    .focus_radius(6.0)
    .style(move |theme: &theme::Theme, status| dropdown_option_style(theme, status, selected))
    .on_press_maybe(on_press)
    .into()
}

fn dropdown_option_style(
    theme: &theme::Theme,
    status: iced_button::Status,
    selected: bool,
) -> iced_button::Style {
    let shell = &theme.colors.shell;
    let background = match (selected, status) {
        (_, iced_button::Status::Disabled) => color::TRANSPARENT,
        (true, _) => shell.selected,
        (false, iced_button::Status::Hovered | iced_button::Status::Pressed) => shell.hover,
        (false, _) => color::TRANSPARENT,
    };
    iced_button::Style {
        background: Some(Background::Color(background)),
        border: iced::Border {
            radius: 6.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A route tile's frame while its menu is open: the accent line instead of
/// the hover outline, so the tile reads as the menu's owner.
pub fn open_tile_style(theme: &theme::Theme, _: iced_button::Status) -> iced_button::Style {
    iced_button::Style {
        background: Some(Background::Color(color::TRANSPARENT)),
        border: iced::Border {
            color: theme.colors.shell.accent,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}
