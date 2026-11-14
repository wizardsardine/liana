//! A compact calendar trigger with an overlay that does not enlarge its form.

use crate::locale::copy as localized_copy;
use chrono::NaiveDate;
use iced::widget::{button, text, Themer};
use iced_aw::DatePicker;

use crate::{font, theme, theme::date_picker::CalendarTheme, widget::Element};

/// Render an iced_aw calendar beside an editable date field.
///
/// Only `on_select` commits a value. Opening and cancelling leave the caller's
/// text untouched. The closed branch is a plain button, so closing discards
/// iced_aw's internal month/day state; every new open starts at `initial_date`
/// even after a cancelled navigation or an intervening typed edit.
pub fn calendar_button<'a, Message: Clone + 'static>(
    show: bool,
    initial_date: NaiveDate,
    on_open: Message,
    on_cancel: Message,
    on_select: impl Fn(NaiveDate) -> Message + 'static,
) -> Element<'a, Message> {
    // Match the compact secondary trigger while using a calendar-only theme.
    let trigger: iced::widget::Button<'a, Message, CalendarTheme> = button(
        text(localized_copy("Choose date"))
            .font(font::REGULAR)
            .shaping(iced::advanced::text::Shaping::Advanced)
            .size(16),
    )
    .padding(10)
    .class(Box::new(theme::button::secondary) as <CalendarTheme as button::Catalog>::Class<'a>)
    .on_press(on_open);
    if !show {
        return Themer::new(None, trigger).into();
    }

    Themer::new(
        None,
        DatePicker::new(true, initial_date, trigger, on_cancel, move |date| {
            on_select(date.into())
        })
        .font_size(16),
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::widget::{tree::Tag, Tree};

    #[derive(Debug, Clone)]
    #[allow(dead_code)] // Widget-construction test never dispatches messages.
    enum Message {
        Open,
        Cancel,
        Selected(NaiveDate),
    }

    #[test]
    fn closed_calendar_drops_picker_state_before_the_next_open() {
        let date = NaiveDate::from_ymd_opt(2020, 3, 5).unwrap();
        let closed = calendar_button(
            false,
            date,
            Message::Open,
            Message::Cancel,
            Message::Selected,
        );
        let opened = calendar_button(
            true,
            date,
            Message::Open,
            Message::Cancel,
            Message::Selected,
        );
        assert_ne!(closed.as_widget().tag(), opened.as_widget().tag());
        assert_eq!(
            opened.as_widget().tag(),
            Tag::of::<iced_aw::widget::date_picker::State>()
        );
        let mut tree = Tree::new(opened.as_widget());
        tree.diff(closed.as_widget());
        assert_eq!(tree.tag, closed.as_widget().tag());

        let next_date = NaiveDate::from_ymd_opt(2022, 11, 9).unwrap();
        let reopened = calendar_button(
            true,
            next_date,
            Message::Open,
            Message::Cancel,
            Message::Selected,
        );
        tree.diff(reopened.as_widget());
        assert_eq!(tree.tag, opened.as_widget().tag());
    }

    fn calendar_action(
        name: &str,
        theme: &theme::Theme,
        date: NaiveDate,
        size: iced::Size,
        viewport: iced::Rectangle,
        action: usize,
    ) {
        use iced::advanced::{
            layout, mouse,
            renderer::{self, Headless},
            Layout, Shell,
        };
        use iced::{Event, Point, Size, Vector};
        let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            font::REGULAR,
            16.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut picker = calendar_button(
            true,
            date,
            Message::Open,
            Message::Cancel,
            Message::Selected,
        );
        let mut tree = Tree::new(picker.as_widget());
        let underlay = picker.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        let mut overlay = picker
            .as_widget_mut()
            .overlay(
                &mut tree,
                Layout::new(&underlay),
                &renderer,
                &viewport,
                Vector::ZERO,
            )
            .unwrap();
        let node = overlay.as_overlay_mut().layout(&renderer, size);
        let layout = Layout::new(&node);
        let buttons: Vec<_> = layout
            .children()
            .skip(1)
            .map(|child| child.bounds())
            .collect();
        assert_eq!(buttons.len(), 2);

        // Draw a newly constructed overlay, as iced does after rebuilding it.
        // Its enabled Buttons have not retained an iced redraw status yet.
        overlay.as_overlay().draw(
            &mut renderer,
            theme,
            &renderer::Style {
                text_color: theme.colors.text.primary,
            },
            layout,
            mouse::Cursor::Unavailable,
        );
        let pixels = renderer.screenshot(Size::new(600, 540), 1.0, theme.colors.general.background);
        assert_calendar_pixels(&pixels, &buttons, name, theme);
        if action == 0 {
            if let Ok(directory) = std::env::var("CALENDAR_PREVIEW_DIR") {
                image::save_buffer(
                    std::path::Path::new(&directory).join(format!("calendar-{name}.png")),
                    &pixels,
                    600,
                    540,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }

        let center = buttons[action].center();
        let cursor = mouse::Cursor::Available(Point::new(center.x, center.y));
        let mut messages = Vec::new();
        for event in [
            mouse::Event::ButtonPressed(mouse::Button::Left),
            mouse::Event::ButtonReleased(mouse::Button::Left),
        ] {
            overlay.as_overlay_mut().update(
                &Event::Mouse(event),
                layout,
                cursor,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut Shell::new(&mut messages),
            );
        }
        match action {
            0 => assert!(matches!(messages.as_slice(), [Message::Cancel])),
            _ => {
                assert!(matches!(messages.as_slice(), [Message::Selected(value)] if *value == date))
            }
        }
    }

    fn assert_calendar_pixels(
        pixels: &[u8],
        buttons: &[iced::Rectangle],
        name: &str,
        theme: &theme::Theme,
    ) {
        for bounds in buttons {
            let mut fill = 0;
            let mut ink = 0;
            let palette = theme.colors.buttons.primary.active;
            for y in bounds.y.ceil() as u32..(bounds.y + bounds.height).floor() as u32 {
                for x in bounds.x.ceil() as u32..(bounds.x + bounds.width).floor() as u32 {
                    let offset = ((y * 600 + x) * 4) as usize;
                    let rgb = &pixels[offset..offset + 3];
                    if rgb == &palette.background.into_rgba8()[..3] {
                        fill += 1;
                    }
                    if rgb == &palette.text.into_rgba8()[..3] {
                        ink += 1;
                    }
                }
            }
            assert!(
                fill > 100,
                "{name}: calendar action has no visible primary fill"
            );
            assert!(ink > 5, "{name}: calendar action icon is faded or missing");
        }
    }

    #[test]
    fn calendar_footer_is_visible_and_clickable_in_both_themes() {
        use iced::{Rectangle, Size};

        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
        let size = Size::new(600.0, 540.0);
        let viewport = Rectangle::with_size(size);
        for (name, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for action in 0..2 {
                calendar_action(name, &theme, date, size, viewport, action);
            }
            // This workaround must never brighten disabled actions outside the picker.
            let disabled = theme::button::primary(&theme, iced::widget::button::Status::Disabled);
            assert!(disabled.text_color.a < 1.0);
        }
    }
}
