use std::time::{Duration, Instant};

use bwk_qr::Image;

use crate::airgap::Error;

/// Frames per second the request cycles at. Slow enough for a camera to lock on
/// a frame, fast enough that a long message does not take minutes.
pub const FRAMES_PER_SECOND: u8 = 5;

/// What a view needs to draw the animation, without reaching for the frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationState {
    pub frame: usize,
    pub total_frames: usize,
}

/// Cycles one request's frames. There is no background work: the UI advances it
/// from its own tick, so nothing keeps running once the modal is gone.
pub struct AnimatedQr {
    frames: Vec<Image>,
    interval: Duration,
    started_at: Instant,
}

impl AnimatedQr {
    pub fn new(frames: Vec<Image>, frames_per_second: u8) -> Result<Self, Error> {
        Self::new_at(frames, frames_per_second, Instant::now())
    }

    fn new_at(frames: Vec<Image>, frames_per_second: u8, now: Instant) -> Result<Self, Error> {
        if frames.is_empty() {
            return Err(Error::Transport(
                "the request encoded to no frame".to_owned(),
            ));
        }
        if !(1..=20).contains(&frames_per_second) {
            return Err(Error::Transport(
                "QR animation speed must be between 1 and 20 frames per second".to_owned(),
            ));
        }
        Ok(Self {
            frames,
            interval: Duration::from_secs_f64(1.0 / f64::from(frames_per_second)),
            started_at: now,
        })
    }

    pub fn frame(&self) -> Option<&Image> {
        self.frame_at(Instant::now())
    }

    pub fn frame_at(&self, now: Instant) -> Option<&Image> {
        self.frames.get(self.frame_index_at(now)?)
    }

    pub fn state(&self) -> AnimationState {
        self.state_at(Instant::now())
    }

    pub fn state_at(&self, now: Instant) -> AnimationState {
        AnimationState {
            frame: self.frame_index_at(now).unwrap_or(0),
            total_frames: self.frames.len(),
        }
    }

    fn frame_index_at(&self, now: Instant) -> Option<usize> {
        if self.frames.is_empty() {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.started_at);
        let ticks = elapsed.as_nanos() / self.interval.as_nanos();
        Some((ticks % self.frames.len() as u128) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(count: u8) -> Vec<Image> {
        (0..count)
            .map(|value| Image {
                data: vec![value],
                width: 1,
                height: 1,
            })
            .collect()
    }

    fn value(frame: Option<&Image>) -> Option<u8> {
        frame.map(|frame| frame.data[0])
    }

    #[test]
    fn single_frame_is_stable() {
        let now = Instant::now();
        let animation = AnimatedQr::new_at(frames(1), 5, now).unwrap();
        assert_eq!(
            value(animation.frame_at(now + Duration::from_secs(60))),
            Some(0)
        );
    }

    #[test]
    fn multipart_cycles_deterministically() {
        let now = Instant::now();
        let animation = AnimatedQr::new_at(frames(3), 5, now).unwrap();
        assert_eq!(value(animation.frame_at(now)), Some(0));
        assert_eq!(
            value(animation.frame_at(now + Duration::from_millis(200))),
            Some(1)
        );
        assert_eq!(
            value(animation.frame_at(now + Duration::from_millis(600))),
            Some(0)
        );
    }

    /// The cycle is a pure function of elapsed time, so a long-running exchange
    /// keeps landing on the same frame at the same offset.
    #[test]
    fn the_cycle_stays_in_step_over_time() {
        let now = Instant::now();
        let animation = AnimatedQr::new_at(frames(3), 5, now).unwrap();
        assert_eq!(
            value(animation.frame_at(now + Duration::from_millis(200))),
            value(animation.frame_at(now + Duration::from_millis(800)))
        );
        assert_eq!(
            value(animation.frame_at(now + Duration::from_secs(60))),
            Some(0)
        );
    }

    #[test]
    fn rejects_unsafe_animation_rates() {
        assert!(AnimatedQr::new(frames(1), 0).is_err());
        assert!(AnimatedQr::new(frames(1), 21).is_err());
        assert!(AnimatedQr::new(Vec::new(), 5).is_err());
    }
}
