use std::{ops::RangeInclusive, time::Duration};

use bwk_qr::config::{QrDensity, QrSize};
use liana_ui::component::qr::Brightness;
use serde::{Deserialize, Serialize};

/// How a request is shown to the signer, kept across exchanges and wallets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplaySettings {
    /// The [`QrDensity`] every frame is drawn at, by its rank from the lowest.
    pub density: u8,
    /// Frames shown per two seconds, so the rate can step by half a frame a second.
    pub frame_rate: u8,
    /// Percent of the light colour painted.
    pub brightness: u8,
}

impl DisplaySettings {
    pub const DENSITIES: RangeInclusive<u8> = 0..=4;
    /// 1 to 12 frames a second.
    pub const FRAME_RATES: RangeInclusive<u8> = 2..=24;

    /// Brings a value read from disk back into range.
    pub fn clamped(self) -> Self {
        Self {
            density: self
                .density
                .clamp(*Self::DENSITIES.start(), *Self::DENSITIES.end()),
            frame_rate: self
                .frame_rate
                .clamp(*Self::FRAME_RATES.start(), *Self::FRAME_RATES.end()),
            brightness: self.brightness().percent(),
        }
    }

    pub fn density(self) -> QrDensity {
        match self.density {
            0 => QrDensity::Lowest,
            1 => QrDensity::Low,
            2 => QrDensity::Medium,
            3 => QrDensity::High,
            _ => QrDensity::Highest,
        }
    }

    pub fn frame_size(self) -> QrSize {
        self.density().into()
    }

    pub fn frame_interval(self) -> Duration {
        Duration::from_millis(2000 / u64::from(self.frame_rate.max(1)))
    }

    pub fn brightness(self) -> Brightness {
        Brightness::new(self.brightness)
    }
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            density: 2,
            frame_rate: 8,
            brightness: Brightness::default().percent(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bwk_qr::config::{QrDensity, QrSize};

    use crate::airgap::display::DisplaySettings;

    #[test]
    fn values_from_disk_are_brought_back_into_range() {
        let settings = DisplaySettings {
            density: 9,
            frame_rate: 200,
            brightness: 0,
        }
        .clamped();
        assert_eq!(settings.density(), QrDensity::Highest);
        assert_eq!(settings.frame_rate, *DisplaySettings::FRAME_RATES.end());
        assert!(settings.brightness > 0);
    }

    #[test]
    fn the_default_shows_medium_frames_four_times_a_second() {
        let settings = DisplaySettings::default();
        assert_eq!(settings.density(), QrDensity::Medium);
        assert_eq!(settings.frame_size(), QrSize::S53);
        assert_eq!(settings.frame_interval(), Duration::from_millis(250));
    }
}
