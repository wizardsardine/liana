use std::ops::RangeInclusive;

use bwk_bbqr::Progress;
use bwk_qr::{config::QrSize, encoder::Encoder, error::Error, image::Image};
use iced::{
    mouse,
    widget::{column, container, image, mouse_area, Space},
    Alignment, Length,
};

use crate::{
    component::{card, text::new::caption},
    theme,
    widget::{Column, Container, Element, Row, Slider, SpaceExt},
};

/// Side of the rendered QR frame, in pixels. Large enough that a phone or a
/// signer's camera reads a dense symbol from a comfortable distance.
const FRAME_SIZE: u32 = 440;
/// The share of an enlarged card the code fills.
const ENLARGED_CODE_PERCENT: u32 = 90;
/// Only clips the quiet zone: at its smallest, that margin is wider than a third
/// of the radius, which is all a rounded corner cuts.
const FRAME_RADIUS: f32 = 12.0;
/// The largest code a piece of text is drawn as, version 27, which covers a BIP-21
/// URI with a long address and a derivation index several times over.
const TEXT_MAX_SIZE: QrSize = QrSize::S125;
const CAPTION_SPACING: u32 = 10;
const SLIDER_WIDTH: u32 = 200;
/// Width the frame grid is laid out in, about that of the camera preview.
const PROGRESS_WIDTH: u32 = 400;
/// Cells are squares, as large as fits up to the maximum, never below the minimum.
const CELL_MIN_SIZE: u32 = 10;
const CELL_MAX_SIZE: u32 = 24;
const CELL_GAP: u32 = 4;

/// How brightly the light modules of a QR code are painted, as a percentage of
/// the theme's light colour.
///
/// A screen at full white can wash out a camera sensor, and the blown-out pixels
/// bleed over the dark modules until the code stops decoding. Turning the light
/// modules down fixes that, at the cost of contrast on a screen that is already
/// dim, so the reader picks the level rather than the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Brightness(u8);

impl Brightness {
    /// Below this the code stops being reliably readable at all, so the slider
    /// does not go there.
    pub const MIN: u8 = 20;
    pub const MAX: u8 = 100;
    /// Black on full white is the contrast signer cameras read best, so the
    /// slider starts at the top and is only turned down for a washed-out code.
    pub const DEFAULT: u8 = Self::MAX;

    pub fn new(percent: u8) -> Self {
        Self(percent.clamp(Self::MIN, Self::MAX))
    }

    pub fn percent(self) -> u8 {
        self.0
    }

    /// The fraction of the light colour painted. The dark modules are left
    /// alone, so turning this down lowers glare without inverting the code.
    fn level(self) -> f32 {
        f32::from(self.0) / 100.0
    }
}

impl Default for Brightness {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

/// One labelled slider of the QR display settings. `on_release` fires once the
/// handle is let go, so a caller can save without writing on every step.
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
    .spacing(CAPTION_SPACING)
    .into()
}

/// A QR code for a single piece of text, with the text shown below it.
pub fn text<'a, M: 'a>(
    theme_style: theme::qr_code::Style,
    content: &str,
    caption_text: Option<String>,
) -> Result<Element<'a, M>, Error> {
    let image = Encoder::new(TEXT_MAX_SIZE).encode_text(content)?;
    let code = paint(
        theme_style,
        // shown to a phone camera with no control to adjust, so give it all the
        // contrast there is
        Brightness::new(Brightness::MAX),
        &image,
    );
    Ok(column![
        card::simple(code_image(&code, FRAME_SIZE)),
        Space::with_height(CAPTION_SPACING),
        caption_text.map(caption),
    ]
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into())
}

/// Paints a code in the QR theme's two colours rather than the app palette,
/// because a scanner needs the contrast, not the theme. `code` is the grayscale
/// square the QR encoder produced, dark modules at 0.
///
/// An animation paints each frame once and keeps the handle: a new handle on
/// every redraw is a new texture upload.
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

/// One frame of an animated QR sequence, painted by [`paint`]. A click on it
/// hands back `on_press`.
pub fn frame<'a, M: Clone + 'a>(code: &image::Handle, on_press: M) -> Element<'a, M> {
    Container::new(card::simple(
        mouse_area(code_image(code, FRAME_SIZE))
            .on_press(on_press)
            .interaction(mouse::Interaction::Pointer),
    ))
    .center_x(Length::Fill)
    .into()
}

/// A QR code alone in a square card `side` pixels wide, the code filling most of
/// it, for a signer whose camera needs a larger code. A click hands back `on_press`.
pub fn enlarged<'a, M: Clone + 'a>(code: &image::Handle, side: u32, on_press: M) -> Element<'a, M> {
    let code_side = side * ENLARGED_CODE_PERCENT / 100;
    Container::new(
        mouse_area(code_image(code, code_side))
            .on_press(on_press)
            .interaction(mouse::Interaction::Pointer),
    )
    .center(side)
    .style(theme::card::modal)
    .into()
}

/// A painted code scaled up to `side` pixels.
fn code_image(code: &image::Handle, side: u32) -> iced::widget::Image<image::Handle> {
    image(code.clone())
        .width(side)
        .height(side)
        .border_radius(FRAME_RADIUS)
        .filter_method(image::FilterMethod::Nearest)
}

/// The camera preview, already mirrored and scaled by the capture thread.
pub fn preview<'a, M: 'a>(handle: image::Handle, height: u32) -> Element<'a, M> {
    Container::new(image(handle).height(height))
        .center_x(Length::Fill)
        .into()
}

/// One cell per frame of `progress`: filled once read or shown, the last one
/// highlighted, the others outlined. Nothing for a single frame.
pub fn progress<'a, M: 'a>(progress: &Progress) -> Option<Element<'a, M>> {
    if progress.total() <= 1 {
        return None;
    }
    let grid = progress_layout(progress.total(), PROGRESS_WIDTH)?;
    let mut cells = progress.received.iter().enumerate().map(|(index, read)| {
        let style: fn(&theme::Theme) -> container::Style = match (*read, index == progress.last) {
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
    let rows = (0..grid.rows).map(|_| {
        Row::with_children(cells.by_ref().take(grid.columns as usize))
            .spacing(CELL_GAP)
            .into()
    });
    Some(
        Container::new(Column::with_children(rows).spacing(CELL_GAP))
            .center_x(Length::Fill)
            .into(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Grid {
    columns: u32,
    rows: u32,
    cell: u32,
}

/// Puts every cell on one row when they fit at the minimum size, and wraps the
/// rest onto more rows rather than hiding any.
fn progress_layout(total: usize, width: u32) -> Option<Grid> {
    let total = u32::try_from(total).ok().filter(|total| *total > 0)?;
    if width < CELL_MIN_SIZE {
        return None;
    }
    let cell = ((width + CELL_GAP) / total)
        .saturating_sub(CELL_GAP)
        .clamp(CELL_MIN_SIZE, CELL_MAX_SIZE);
    let columns = ((width + CELL_GAP) / (cell + CELL_GAP)).min(total);
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

    use crate::component::qr::{
        progress, progress_layout, to_rgba, Brightness, CELL_GAP, CELL_MAX_SIZE, CELL_MIN_SIZE,
        PROGRESS_WIDTH, TEXT_MAX_SIZE,
    };

    /// The encoder has to swallow a real BIP-21 URI, which is longer than a bare
    /// address once the index suffix is on it.
    #[test]
    fn a_payment_uri_encodes() {
        let uri = "bitcoin:bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq?index=2147483647";
        assert!(Encoder::new(TEXT_MAX_SIZE).encode_text(uri).is_ok());
    }

    /// Dark modules are painted at full strength whatever the brightness, so the
    /// code never loses contrast from the light side being turned down.
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

    /// BBQr allows up to 1295 parts: the grid wraps them onto more rows, in the
    /// same width, rather than dropping any.
    #[test]
    fn the_grid_wraps_instead_of_hiding_frames() {
        for total in [1, 2, 28, 29, 1295] {
            let grid = progress_layout(total, PROGRESS_WIDTH).unwrap();
            assert!((grid.columns * grid.rows) as usize >= total, "{total}");
            assert!(
                grid.columns * (grid.cell + CELL_GAP) - CELL_GAP <= PROGRESS_WIDTH,
                "{total}"
            );
            assert!(grid.cell >= CELL_MIN_SIZE, "{total}");
        }
        assert_eq!(
            progress_layout(3, PROGRESS_WIDTH).unwrap().cell,
            CELL_MAX_SIZE
        );
        assert_eq!(progress_layout(28, PROGRESS_WIDTH).unwrap().rows, 1);
        assert_eq!(progress_layout(29, PROGRESS_WIDTH).unwrap().rows, 2);
        assert_eq!(
            progress_layout(1295, PROGRESS_WIDTH).unwrap().cell,
            CELL_MIN_SIZE
        );
        assert_eq!(progress_layout(0, PROGRESS_WIDTH), None);
    }

    #[test]
    fn a_single_frame_shows_no_grid() {
        let single = Progress {
            received: vec![true],
            last: 0,
        };
        assert!(progress::<()>(&single).is_none());
        let two = Progress {
            received: vec![true, false],
            last: 0,
        };
        assert!(progress::<()>(&two).is_some());
    }
}
