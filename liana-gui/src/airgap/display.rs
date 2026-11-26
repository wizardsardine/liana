use std::{ops::RangeInclusive, time::Duration};

use bwk_qr::config::{QrDensity, QrSize};
use liana_ui::component::qr::Brightness;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QrSettings {
    pub density: QrDensity,
    #[serde(deserialize_with = "frame_rate")]
    pub frame_rate: f32, // Frames per second.
    #[serde(with = "percent")]
    pub brightness: Brightness,
}

impl QrSettings {
    pub const DENSITIES: [QrDensity; 5] = [
        QrDensity::Lowest,
        QrDensity::Low,
        QrDensity::Medium,
        QrDensity::High,
        QrDensity::Highest,
    ];
    pub const FRAME_RATES: RangeInclusive<f32> = 1.0..=12.0; // Frames per second.
    pub const FRAME_RATE_STEP: f32 = 0.5;

    pub fn frame_size(&self) -> QrSize {
        self.density.into()
    }

    pub fn frame_interval(&self) -> Duration {
        Duration::from_secs_f32(1.0 / self.frame_rate)
    }
}

impl Default for QrSettings {
    fn default() -> Self {
        Self {
            density: QrDensity::Medium,
            frame_rate: 4.0,
            brightness: Brightness::default(),
        }
    }
}

fn frame_rate<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    let rate = f32::deserialize(deserializer)?;
    Ok(rate.clamp(
        *QrSettings::FRAME_RATES.start(),
        *QrSettings::FRAME_RATES.end(),
    ))
}

mod percent {
    use liana_ui::component::qr::Brightness;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        brightness: &Brightness,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        brightness.percent().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Brightness, D::Error> {
        u8::deserialize(deserializer).map(Brightness::new)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bwk_qr::config::{QrDensity, QrSize};
    use liana_ui::component::qr::Brightness;

    use crate::airgap::display::QrSettings;

    #[test]
    fn values_from_disk_are_brought_back_into_range() {
        let settings: QrSettings =
            serde_json::from_str(r#"{"density":"highest","frame_rate":200,"brightness":0}"#)
                .unwrap();
        assert_eq!(settings.density, QrDensity::Highest);
        assert_eq!(settings.frame_rate, *QrSettings::FRAME_RATES.end());
        assert_eq!(settings.brightness.percent(), Brightness::MIN);
    }

    #[test]
    fn the_default_shows_medium_frames_four_times_a_second() {
        let settings = QrSettings::default();
        assert_eq!(settings.frame_size(), QrSize::S53);
        assert_eq!(settings.frame_interval(), Duration::from_millis(250));
    }
}
