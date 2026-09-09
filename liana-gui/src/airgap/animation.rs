use std::time::{Duration, Instant};

use zeroize::Zeroize;

use super::{EncodedUr, Error, UrType};

/// Snapshot used by presentation layers without exposing the full frame set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationState {
    pub frame: usize,
    pub total_frames: usize,
    pub paused: bool,
}

/// Owns one deterministic UR cycle and advances it without background work.
///
/// UI layers drive this from their normal tick subscription. Keeping animation
/// state synchronous avoids a thread retaining PSBT fragments after a modal is
/// closed. `clear` and `Drop` overwrite every owned frame before releasing it.
pub struct AnimatedQr {
    ur_type: UrType,
    frames: Vec<String>,
    interval: Duration,
    started_at: Instant,
    paused_at: Option<Instant>,
    paused_duration: Duration,
}

impl AnimatedQr {
    pub fn new(encoded: EncodedUr, frames_per_second: u8) -> Result<Self, Error> {
        Self::new_at(encoded, frames_per_second, Instant::now())
    }

    fn new_at(encoded: EncodedUr, frames_per_second: u8, now: Instant) -> Result<Self, Error> {
        if encoded.frames.is_empty() {
            return Err(Error::Empty);
        }
        if !(1..=20).contains(&frames_per_second) {
            return Err(Error::InvalidUr(
                "QR animation speed must be between 1 and 20 frames per second".to_owned(),
            ));
        }
        Ok(Self {
            ur_type: encoded.ur_type,
            frames: encoded.frames,
            interval: Duration::from_secs_f64(1.0 / f64::from(frames_per_second)),
            started_at: now,
            paused_at: None,
            paused_duration: Duration::ZERO,
        })
    }

    pub fn ur_type(&self) -> UrType {
        self.ur_type
    }

    pub fn frame(&self) -> Option<&str> {
        self.frame_at(Instant::now())
    }

    pub fn frame_at(&self, now: Instant) -> Option<&str> {
        let index = self.frame_index_at(now)?;
        self.frames.get(index).map(String::as_str)
    }

    pub fn state(&self) -> AnimationState {
        self.state_at(Instant::now())
    }

    pub fn state_at(&self, now: Instant) -> AnimationState {
        AnimationState {
            frame: self.frame_index_at(now).unwrap_or(0),
            total_frames: self.frames.len(),
            paused: self.paused_at.is_some(),
        }
    }

    pub fn pause(&mut self) {
        self.pause_at(Instant::now());
    }

    pub fn pause_at(&mut self, now: Instant) {
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
    }

    pub fn resume(&mut self) {
        self.resume_at(Instant::now());
    }

    pub fn resume_at(&mut self, now: Instant) {
        if let Some(paused_at) = self.paused_at.take() {
            self.paused_duration = self
                .paused_duration
                .saturating_add(now.saturating_duration_since(paused_at));
        }
    }

    pub fn restart(&mut self) {
        self.restart_at(Instant::now());
    }

    pub fn restart_at(&mut self, now: Instant) {
        self.started_at = now;
        self.paused_at = None;
        self.paused_duration = Duration::ZERO;
    }

    pub fn clear(&mut self) {
        self.frames.zeroize();
        self.frames.clear();
        self.paused_at = None;
        self.paused_duration = Duration::ZERO;
    }

    fn frame_index_at(&self, now: Instant) -> Option<usize> {
        if self.frames.is_empty() {
            return None;
        }
        let effective_now = self.paused_at.unwrap_or(now);
        let elapsed = effective_now
            .saturating_duration_since(self.started_at)
            .saturating_sub(self.paused_duration);
        let ticks = elapsed.as_nanos() / self.interval.as_nanos();
        Some((ticks % self.frames.len() as u128) as usize)
    }
}

impl Drop for AnimatedQr {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(frames: &[&str]) -> EncodedUr {
        EncodedUr {
            ur_type: UrType::CryptoPsbt,
            frames: frames.iter().map(|frame| (*frame).to_owned()).collect(),
        }
    }

    #[test]
    fn single_frame_is_stable() {
        let now = Instant::now();
        let animation = AnimatedQr::new_at(encoded(&["one"]), 5, now).unwrap();
        assert_eq!(
            animation.frame_at(now + Duration::from_secs(60)),
            Some("one")
        );
    }

    #[test]
    fn multipart_cycles_deterministically() {
        let now = Instant::now();
        let animation = AnimatedQr::new_at(encoded(&["one", "two", "three"]), 5, now).unwrap();
        assert_eq!(animation.frame_at(now), Some("one"));
        assert_eq!(
            animation.frame_at(now + Duration::from_millis(200)),
            Some("two")
        );
        assert_eq!(
            animation.frame_at(now + Duration::from_millis(600)),
            Some("one")
        );
    }

    #[test]
    fn pause_resume_and_restart_preserve_expected_frame() {
        let now = Instant::now();
        let mut animation = AnimatedQr::new_at(encoded(&["one", "two", "three"]), 5, now).unwrap();
        animation.pause_at(now + Duration::from_millis(250));
        assert_eq!(
            animation.frame_at(now + Duration::from_secs(5)),
            Some("two")
        );
        animation.resume_at(now + Duration::from_secs(5));
        assert_eq!(
            animation.frame_at(now + Duration::from_millis(5_100)),
            Some("two")
        );
        assert_eq!(
            animation.frame_at(now + Duration::from_millis(5_150)),
            Some("three")
        );
        animation.restart_at(now + Duration::from_secs(6));
        assert_eq!(
            animation.frame_at(now + Duration::from_secs(6)),
            Some("one")
        );
    }

    #[test]
    fn clear_removes_owned_sensitive_frames() {
        let now = Instant::now();
        let mut animation = AnimatedQr::new_at(encoded(&["secret"]), 5, now).unwrap();
        animation.clear();
        assert_eq!(animation.frame_at(now), None);
        assert_eq!(animation.state_at(now).total_frames, 0);
    }

    #[test]
    fn rejects_unsafe_animation_rates() {
        assert!(AnimatedQr::new(encoded(&["one"]), 0).is_err());
        assert!(AnimatedQr::new(encoded(&["one"]), 21).is_err());
    }
}
