use iced::{
    widget::{column, image, Space},
    Alignment, Length,
};

use crate::{
    component::text::new::caption,
    theme,
    widget::{Container, Element, Slider, SpaceExt},
};

/// Side of the rendered QR frame, in pixels. Large enough that a phone or a
/// signer's camera reads a dense symbol from a comfortable distance.
const FRAME_SIZE: u32 = 440;
const CAPTION_SPACING: u32 = 10;
const SLIDER_WIDTH: u32 = 200;

/// How brightly the light modules of a QR code are painted, as a percentage of
/// the theme's light colour.
///
/// A screen at full white washes out a camera sensor, and the blown-out pixels
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
    /// Most screens are bright enough to wash out a camera at full white, so
    /// the slider starts halfway rather than at the top.
    pub const DEFAULT: u8 = 50;

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

/// The brightness control shown next to a QR code.
pub fn brightness_slider<'a, M: Clone + 'a>(
    brightness: Brightness,
    on_change: impl Fn(Brightness) -> M + 'a,
) -> Element<'a, M> {
    column![
        caption(format!("Brightness: {}%", brightness.percent())),
        Slider::new(
            Brightness::MIN..=Brightness::MAX,
            brightness.percent(),
            move |percent| on_change(Brightness::new(percent)),
        )
        .width(SLIDER_WIDTH),
    ]
    .align_x(Alignment::Center)
    .spacing(CAPTION_SPACING)
    .into()
}

/// One frame of an animated QR sequence.
///
/// `data` is the grayscale matrix the QR encoder produced, one byte per pixel,
/// dark modules at 0. It is painted in the QR theme's own two colours rather
/// than the app palette, because a scanner needs the contrast, not the theme.
pub fn frame<'a, M: 'a>(
    theme_style: iced::widget::qr_code::Style,
    brightness: Brightness,
    data: &[u8],
    width: u32,
    height: u32,
    caption_text: Option<String>,
) -> Element<'a, M> {
    let cell = to_rgba(theme_style.cell, 1.0);
    let background = to_rgba(theme_style.background, brightness.level());
    let rgba: Vec<u8> = data
        .iter()
        .flat_map(|value| if *value < 128 { cell } else { background })
        .collect();
    let frame = Container::new(
        image(image::Handle::from_rgba(width, height, rgba))
            .width(FRAME_SIZE)
            .height(FRAME_SIZE)
            .filter_method(image::FilterMethod::Nearest),
    )
    .padding(10)
    .style(theme::card::simple);

    column![
        frame,
        Space::with_height(CAPTION_SPACING),
        caption_text.map(caption),
    ]
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into()
}

/// The camera preview, already mirrored and scaled by the capture thread.
pub fn preview<'a, M: 'a>(handle: image::Handle, height: u32) -> Element<'a, M> {
    Container::new(image(handle).height(height))
        .center_x(Length::Fill)
        .into()
}

fn to_rgba(color: iced::Color, level: f32) -> [u8; 4] {
    let scale = |channel: f32| (channel * level * 255.0).round().clamp(0.0, 255.0) as u8;
    [scale(color.r), scale(color.g), scale(color.b), 255]
}
