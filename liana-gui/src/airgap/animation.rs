use std::time::Instant;

use bwk_bbqr::Progress;
use bwk_qr::{
    config::{QrDensity, QrSize},
    encoder::Encoder,
    image::Image,
};
use iced::{widget::image, Subscription};
use liana_ui::{component::qr, theme};

use crate::airgap::{display::QrSettings, error::Error};

pub struct AnimatedQr {
    payload: Vec<u8>,
    settings: QrSettings,
    frames: Vec<Image>,
    painted: Vec<image::Handle>,
    progress: Progress,
}

impl AnimatedQr {
    pub fn new(payload: Vec<u8>, settings: QrSettings) -> Result<Self, Error> {
        let frames = encode(&payload, settings.frame_size())?;
        let mut animation = Self {
            payload,
            settings,
            frames: Vec::new(),
            painted: Vec::new(),
            progress: Progress {
                received: Vec::new(),
                last: 0,
            },
        };
        animation.set_frames(frames);
        Ok(animation)
    }

    pub fn settings(&self) -> &QrSettings {
        &self.settings
    }

    pub fn subscription(&self) -> Subscription<Instant> {
        iced::time::every(self.settings.frame_interval())
    }

    pub fn next(&mut self) {
        if let Some(last) = (self.progress.last + 1).checked_rem(self.progress.total()) {
            self.progress.last = last;
            self.progress.received[last] = true;
        }
    }

    pub fn set_density(&mut self, density: QrDensity) {
        if density == self.settings.density {
            return;
        }
        if let Ok(frames) = encode(&self.payload, density.into()) {
            self.settings.density = density;
            self.set_frames(frames);
        }
    }

    pub fn set_frame_rate(&mut self, frame_rate: f32) {
        self.settings.frame_rate = frame_rate;
    }

    pub fn set_brightness(&mut self, brightness: qr::Brightness) {
        if brightness != self.settings.brightness {
            self.settings.brightness = brightness;
            self.paint_frames();
        }
    }

    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    fn set_frames(&mut self, frames: Vec<Image>) {
        let mut received = vec![false; frames.len()];
        if let Some(first) = received.first_mut() {
            *first = true;
        }
        self.progress = Progress { received, last: 0 };
        self.frames = frames;
        self.paint_frames();
    }

    pub fn frame(&self) -> &image::Handle {
        &self.painted[self.progress.last]
    }

    fn paint_frames(&mut self) {
        let style = theme::qr_code::qr_code(&theme::Theme::default());
        self.painted = self
            .frames
            .iter()
            .map(|frame| qr::paint(style, self.settings.brightness, frame))
            .collect();
    }
}

fn encode(payload: &[u8], frame_size: QrSize) -> Result<Vec<Image>, Error> {
    let frames = Encoder::new(frame_size).encode_bbqr(payload)?;
    log::debug!(
        "showing the request over {} QR frame(s) of version {}",
        frames.len(),
        frame_size.version(),
    );
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use bwk_bbqr::Progress;
    use bwk_qr::image::Image;

    use crate::airgap::{animation::AnimatedQr, display::QrSettings};

    fn animation(count: u8) -> AnimatedQr {
        let mut animation = AnimatedQr {
            payload: Vec::new(),
            settings: QrSettings::default(),
            frames: Vec::new(),
            painted: Vec::new(),
            progress: Progress {
                received: Vec::new(),
                last: 0,
            },
        };
        animation.set_frames(
            (0..count)
                .map(|value| Image::from_fn(1, |_, _| value))
                .collect(),
        );
        animation
    }

    #[test]
    fn single_frame_is_stable() {
        let mut animation = animation(1);
        for _ in 0..10 {
            animation.next();
        }
        assert_eq!(animation.progress.last, 0);
    }

    #[test]
    fn each_tick_shows_the_next_frame() {
        let mut animation = animation(3);
        let mut seen = Vec::new();
        for _ in 0..7 {
            seen.push(animation.progress.last);
            animation.next();
        }
        assert_eq!(seen, vec![0, 1, 2, 0, 1, 2, 0]);
    }

    #[test]
    fn frames_count_as_shown_once_on_screen() {
        let mut animation = animation(3);
        assert_eq!(animation.progress.seen(), 1);
        animation.next();
        assert_eq!(animation.progress.seen(), 2);
        for _ in 0..10 {
            animation.next();
        }
        assert_eq!(animation.progress.seen(), 3);
    }

    #[test]
    fn new_frames_start_over() {
        let mut animation = animation(3);
        animation.next();
        animation.set_frames(vec![Image::from_fn(1, |_, _| 0); 2]);
        assert_eq!((animation.progress.last, animation.progress.seen()), (0, 1));
    }
}
