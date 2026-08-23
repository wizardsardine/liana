use std::time::{Duration, Instant};

use bwk_bbqr::Progress;
use bwk_qr::image::Image;
use iced::{widget::image, Subscription};
use liana_ui::{component::qr, theme, widget::Element};

use crate::airgap::error::Error;

/// Cycles through a request's frames, one per tick of its own subscription. Each
/// frame is painted once, so a redraw reuses its texture.
pub struct AnimatedQr {
    frames: Vec<Image>,
    painted: Vec<image::Handle>,
    brightness: qr::Brightness,
    interval: Duration,
    /// The frames shown at least once, and the one on screen.
    progress: Progress,
}

impl AnimatedQr {
    pub fn new(
        frames: Vec<Image>,
        brightness: qr::Brightness,
        interval: Duration,
    ) -> Result<Self, Error> {
        let mut animation = Self {
            frames: Vec::new(),
            painted: Vec::new(),
            brightness,
            interval,
            progress: Progress {
                received: Vec::new(),
                last: 0,
            },
        };
        animation.set_frames(frames)?;
        Ok(animation)
    }

    /// Fires once per frame, feed each tick to [`Self::tick`].
    pub fn subscription(&self) -> Subscription<Instant> {
        iced::time::every(self.interval)
    }

    /// Moves to the next frame.
    pub fn tick(&mut self) {
        self.progress.last = (self.progress.last + 1) % self.progress.total();
        self.progress.received[self.progress.last] = true;
    }

    /// Starts over on new frames.
    pub fn set_frames(&mut self, frames: Vec<Image>) -> Result<(), Error> {
        if frames.is_empty() {
            return Err(Error::Transport(
                "the request encoded to no frame".to_owned(),
            ));
        }
        let mut received = vec![false; frames.len()];
        received[0] = true;
        self.progress = Progress { received, last: 0 };
        self.frames = frames;
        self.paint_frames();
        Ok(())
    }

    pub fn set_brightness(&mut self, brightness: qr::Brightness) {
        if brightness != self.brightness {
            self.brightness = brightness;
            self.paint_frames();
        }
    }

    /// The next tick comes after the new interval.
    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    /// The frame on screen, then the grid of frames shown so far. A click on the
    /// frame hands back `on_press`.
    pub fn view<'a, M: Clone + 'a>(&'a self, on_press: M) -> Element<'a, M> {
        iced::widget::column![
            qr::frame(self.frame(), on_press),
            qr::progress(&self.progress),
        ]
        .spacing(15)
        .into()
    }

    /// The frame on screen alone in a square card `side` pixels wide.
    pub fn enlarged<'a, M: Clone + 'a>(&'a self, side: u32, on_press: M) -> Element<'a, M> {
        qr::enlarged(self.frame(), side, on_press)
    }

    fn frame(&self) -> &image::Handle {
        &self.painted[self.progress.last]
    }

    fn paint_frames(&mut self) {
        let style = theme::qr_code::qr_code(&theme::Theme::default());
        self.painted = self
            .frames
            .iter()
            .map(|frame| qr::paint(style, self.brightness, frame))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bwk_qr::image::Image;
    use liana_ui::component::qr::Brightness;

    use crate::airgap::animation::AnimatedQr;

    fn animation(count: u8) -> AnimatedQr {
        let frames = (0..count)
            .map(|value| Image::from_fn(1, |_, _| value))
            .collect();
        AnimatedQr::new(frames, Brightness::default(), Duration::from_millis(250)).unwrap()
    }

    #[test]
    fn single_frame_is_stable() {
        let mut animation = animation(1);
        for _ in 0..10 {
            animation.tick();
        }
        assert_eq!(animation.progress.last, 0);
    }

    /// Every tick shows the next frame, so none is ever skipped.
    #[test]
    fn each_tick_shows_the_next_frame() {
        let mut animation = animation(3);
        let mut seen = Vec::new();
        for _ in 0..7 {
            seen.push(animation.progress.last);
            animation.tick();
        }
        assert_eq!(seen, vec![0, 1, 2, 0, 1, 2, 0]);
    }

    /// The grid fills over the first loop, then stays full.
    #[test]
    fn frames_count_as_shown_once_on_screen() {
        let mut animation = animation(3);
        assert_eq!(animation.progress.seen(), 1);
        animation.tick();
        assert_eq!(animation.progress.seen(), 2);
        for _ in 0..10 {
            animation.tick();
        }
        assert_eq!(animation.progress.seen(), 3);
    }

    #[test]
    fn new_frames_start_over() {
        let mut animation = animation(3);
        animation.tick();
        animation
            .set_frames(vec![Image::from_fn(1, |_, _| 0); 2])
            .unwrap();
        assert_eq!((animation.progress.last, animation.progress.seen()), (0, 1));
    }

    #[test]
    fn rejects_no_frame() {
        let mut animation = animation(1);
        assert!(animation.set_frames(Vec::new()).is_err());
    }
}
