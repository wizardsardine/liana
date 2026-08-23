use std::ops::{Range, RangeInclusive};

use bwk_bbqr::Progress;
use bwk_qr::{config::QrSize, encoder::Encoder, error::Error, image::Image};
use iced::{
    mouse,
    widget::{column, container, image, mouse_area, opaque, responsive, Space},
    Alignment, Length,
};

use crate::{
    component::{card, modal::ModalWidth, text::new::caption},
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{Column, Container, Element, Row, Slider, SpaceExt},
};

const FRAME_SIZE: u32 = 440;
const ENLARGED_SHARE: f32 = 0.8;
const ENLARGED_CODE_PERCENT: u32 = 90;
const FRAME_RADIUS: f32 = 12.0;
const TEXT_MAX_SIZE: QrSize = QrSize::S125;
const SLIDER_WIDTH: u32 = 200;
const PROGRESS_WIDTH_PERCENT: u32 = 80;
const PROGRESS_MAX_ROWS: u32 = 3;
const CELL_MIN_SIZE: u32 = 10;
const CELL_MAX_SIZE: u32 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Brightness(u8);

impl Brightness {
    pub const MIN: u8 = 20;
    pub const MAX: u8 = 100;
    pub const DEFAULT: u8 = Self::MAX;

    pub fn new(percent: u8) -> Self {
        Self(percent.clamp(Self::MIN, Self::MAX))
    }

    pub fn percent(self) -> u8 {
        self.0
    }

    fn level(self) -> f32 {
        f32::from(self.0) / 100.0
    }
}

impl Default for Brightness {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

pub fn setting_slider<'a, M: Clone + 'a>(
    label: String,
    range: RangeInclusive<u8>,
    value: u8,
    on_change: impl Fn(u8) -> M + 'a,
    on_release: M,
) -> Element<'a, M> {
    column![
        caption(label),
        Slider::new(range, value, on_change)
            .on_release(on_release)
            .width(SLIDER_WIDTH),
    ]
    .align_x(Alignment::Center)
    .spacing(VSpacing::SM)
    .into()
}

pub fn text<'a, M: 'a>(
    theme_style: theme::qr_code::Style,
    content: &str,
    caption_text: Option<String>,
) -> Result<Element<'a, M>, Error> {
    let image = Encoder::new(TEXT_MAX_SIZE).encode_text(content)?;
    let code = paint(theme_style, Brightness::new(Brightness::MAX), &image);
    Ok(column![
        card::simple(code_image(&code, FRAME_SIZE)),
        Space::with_height(VSpacing::SM),
        caption_text.map(caption),
    ]
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into())
}

pub fn paint(
    theme_style: theme::qr_code::Style,
    brightness: Brightness,
    code: &Image,
) -> image::Handle {
    let cell = to_rgba(theme_style.cell, 1.0);
    let background = to_rgba(theme_style.background, brightness.level());
    let rgba: Vec<u8> = code
        .data()
        .iter()
        .flat_map(|value| if *value < 128 { cell } else { background })
        .collect();
    image::Handle::from_rgba(code.side(), code.side(), rgba)
}

pub fn frame<'a, M: Clone + 'a>(
    code: &image::Handle,
    enlarged: bool,
    on_press: M,
) -> Element<'a, M> {
    if !enlarged {
        return Container::new(card::simple(
            mouse_area(code_image(code, FRAME_SIZE))
                .on_press(on_press)
                .interaction(mouse::Interaction::Pointer),
        ))
        .center_x(Length::Fill)
        .into();
    }
    let code = code.clone();
    responsive(move |size| {
        let side = (size.width.min(size.height) * ENLARGED_SHARE) as u32;
        let card = Container::new(
            mouse_area(code_image(&code, side * ENLARGED_CODE_PERCENT / 100))
                .on_press(on_press.clone())
                .interaction(mouse::Interaction::Pointer),
        )
        .center(side)
        .style(theme::card::modal);
        Container::new(opaque(card)).center(Length::Fill).into()
    })
    .into()
}

fn code_image(code: &image::Handle, side: u32) -> iced::widget::Image<image::Handle> {
    image(code.clone())
        .width(side)
        .height(side)
        .border_radius(FRAME_RADIUS)
        .filter_method(image::FilterMethod::Nearest)
}

pub fn camera_frame<'a, M: 'a>(handle: image::Handle, height: u32) -> Element<'a, M> {
    Container::new(image(handle).height(height))
        .center_x(Length::Fill)
        .into()
}

pub fn progress<'a, M: 'a>(progress: &Progress, modal_width: ModalWidth) -> Option<Element<'a, M>> {
    if progress.total() <= 1 {
        return None;
    }
    let width = modal_width as u32 * PROGRESS_WIDTH_PERCENT / 100;
    let grid = progress_layout(progress.total(), width)?;
    let columns = grid.columns as usize;
    let current = progress.last as u32 / grid.columns;
    let rows = visible_rows(grid.rows, current).map(|row| {
        let cells = progress
            .received
            .iter()
            .enumerate()
            .skip(row as usize * columns)
            .take(columns)
            .map(|(index, read)| {
                let style: fn(&theme::Theme) -> container::Style =
                    match (*read, index == progress.last) {
                        (true, true) => theme::qr_code::latest_cell,
                        (true, false) => theme::qr_code::received_cell,
                        (false, _) => theme::qr_code::missing_cell,
                    };
                Container::new(Space::new())
                    .width(grid.cell)
                    .height(grid.cell)
                    .style(style)
                    .into()
            });
        Row::with_children(cells).spacing(HSpacing::XS).into()
    });
    Some(
        Container::new(
            Column::with_children(rows)
                .spacing(VSpacing::XS)
                .align_x(Alignment::Center),
        )
        .center_x(Length::Fill)
        .into(),
    )
}

fn visible_rows(rows: u32, current: u32) -> Range<u32> {
    if rows <= PROGRESS_MAX_ROWS {
        return 0..rows;
    }
    let first = current.saturating_sub(1).min(rows - PROGRESS_MAX_ROWS);
    first..first + PROGRESS_MAX_ROWS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Grid {
    columns: u32,
    rows: u32,
    cell: u32,
}

fn progress_layout(total: usize, width: u32) -> Option<Grid> {
    let total = u32::try_from(total).ok().filter(|total| *total > 0)?;
    if width < CELL_MIN_SIZE {
        return None;
    }
    let gap = HSpacing::XS.pixels();
    let cell = ((width + gap) / total)
        .saturating_sub(gap)
        .clamp(CELL_MIN_SIZE, CELL_MAX_SIZE);
    let columns = ((width + gap) / (cell + gap)).min(total);
    let rows = total.div_ceil(columns);
    Some(Grid {
        columns,
        rows,
        cell,
    })
}

fn to_rgba(color: iced::Color, level: f32) -> [u8; 4] {
    let scale = |channel: f32| (channel * level * 255.0).round().clamp(0.0, 255.0) as u8;
    [scale(color.r), scale(color.g), scale(color.b), 255]
}

#[cfg(test)]
mod tests {
    use bwk_bbqr::Progress;
    use bwk_qr::encoder::Encoder;

    use crate::{
        component::{
            modal::ModalWidth,
            qr::{
                progress, progress_layout, to_rgba, visible_rows, Brightness, CELL_MAX_SIZE,
                CELL_MIN_SIZE, PROGRESS_WIDTH_PERCENT, TEXT_MAX_SIZE,
            },
        },
        spacing::HSpacing,
    };

    const WIDTH: u32 = ModalWidth::XL as u32 * PROGRESS_WIDTH_PERCENT / 100;

    #[test]
    fn a_payment_uri_encodes() {
        let uri = "bitcoin:bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq?index=2147483647";
        assert!(Encoder::new(TEXT_MAX_SIZE).encode_text(uri).is_ok());
    }

    #[test]
    fn brightness_only_moves_the_light_modules() {
        let dark = to_rgba(iced::Color::BLACK, Brightness::new(Brightness::MIN).level());
        assert_eq!(dark, [0, 0, 0, 255]);
        let bright = to_rgba(iced::Color::WHITE, Brightness::new(Brightness::MAX).level());
        let dim = to_rgba(iced::Color::WHITE, Brightness::new(Brightness::MIN).level());
        assert_eq!(bright, [255, 255, 255, 255]);
        assert!(dim[0] < bright[0]);
    }

    #[test]
    fn brightness_stays_within_its_bounds() {
        assert_eq!(Brightness::new(0).percent(), Brightness::MIN);
        assert_eq!(Brightness::new(255).percent(), Brightness::MAX);
        assert_eq!(Brightness::default().percent(), Brightness::DEFAULT);
    }

    #[test]
    fn the_grid_wraps_instead_of_hiding_frames() {
        let gap = HSpacing::XS.pixels();
        assert_eq!(WIDTH, 576);
        for total in [1, 2, 41, 42, 1295] {
            let grid = progress_layout(total, WIDTH).unwrap();
            assert!((grid.columns * grid.rows) as usize >= total, "{total}");
            assert!(grid.columns * (grid.cell + gap) - gap <= WIDTH, "{total}");
            assert!(grid.cell >= CELL_MIN_SIZE, "{total}");
        }
        assert_eq!(progress_layout(3, WIDTH).unwrap().cell, CELL_MAX_SIZE);
        assert_eq!(progress_layout(41, WIDTH).unwrap().rows, 1);
        assert_eq!(progress_layout(42, WIDTH).unwrap().rows, 2);
        assert_eq!(progress_layout(1295, WIDTH).unwrap().cell, CELL_MIN_SIZE);
        assert_eq!(progress_layout(0, WIDTH), None);
    }

    #[test]
    fn at_most_three_rows_show_around_the_current_one() {
        assert_eq!(visible_rows(10, 5), 4..7);
        assert_eq!(visible_rows(10, 1), 0..3);
    }

    #[test]
    fn the_shown_rows_stay_full_at_both_ends() {
        assert_eq!(visible_rows(10, 0), 0..3);
        assert_eq!(visible_rows(10, 9), 7..10);
        assert_eq!(visible_rows(10, 8), 7..10);
    }

    #[test]
    fn three_rows_or_fewer_all_show() {
        assert_eq!(visible_rows(3, 2), 0..3);
        assert_eq!(visible_rows(2, 1), 0..2);
    }

    #[test]
    fn a_single_frame_shows_no_grid() {
        let single = Progress {
            received: vec![true],
            last: 0,
        };
        assert!(progress::<()>(&single, ModalWidth::XL).is_none());
        let two = Progress {
            received: vec![true, false],
            last: 0,
        };
        assert!(progress::<()>(&two, ModalWidth::XL).is_some());
    }
}
