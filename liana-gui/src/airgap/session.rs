use std::time::{Duration, Instant};

use foundation_ur::{
    bytewords::{self, Style},
    fountain::part::Part,
    Decoder, UR,
};

use super::{ur::decode_registry_value, Error, UrPayload, UrType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanLimits {
    pub maximum_decoded_bytes: usize,
    pub maximum_fragment_count: u32,
    pub maximum_fragment_chars: usize,
    pub maximum_fragment_bytes: usize,
    pub timeout: Duration,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            // Match Passport Core's own bounded decoder contract.
            maximum_decoded_bytes: 24 * 1024,
            maximum_fragment_count: 128,
            maximum_fragment_chars: 1_408,
            maximum_fragment_bytes: 700,
            timeout: Duration::from_secs(120),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecodeProgress {
    Incomplete { estimated: f32 },
    Complete(UrPayload),
}

pub struct UrDecodeSession {
    expected: UrType,
    limits: ScanLimits,
    decoder: Decoder,
    started_at: Option<Instant>,
    cancelled: bool,
}

impl UrDecodeSession {
    pub fn new(expected: UrType, limits: ScanLimits) -> Self {
        Self {
            expected,
            limits,
            decoder: Decoder::default(),
            started_at: None,
            cancelled: false,
        }
    }

    pub fn receive(&mut self, fragment: &str) -> Result<DecodeProgress, Error> {
        self.receive_at(fragment, Instant::now())
    }

    pub fn receive_at(&mut self, fragment: &str, now: Instant) -> Result<DecodeProgress, Error> {
        if self.cancelled {
            return Err(Error::Cancelled);
        }
        let started_at = *self.started_at.get_or_insert(now);
        if now.saturating_duration_since(started_at) > self.limits.timeout {
            return Err(Error::TimedOut);
        }
        if fragment.is_empty() {
            return Err(Error::Empty);
        }
        if fragment.len() > self.limits.maximum_fragment_chars {
            return Err(Error::FragmentTooLarge {
                actual: fragment.len(),
                maximum: self.limits.maximum_fragment_chars,
            });
        }
        let normalized = fragment.to_ascii_lowercase();
        let parsed = UR::parse(&normalized).map_err(|e| Error::InvalidUr(e.to_string()))?;
        if parsed.as_type() != self.expected.as_str()
            && !(self.expected == UrType::CryptoPsbt && parsed.as_type() == "psbt")
        {
            return Err(Error::WrongUrType {
                expected: self.expected.as_str(),
                actual: parsed.as_type().to_owned(),
            });
        }

        if parsed.is_single_part() {
            if !self.decoder.is_empty() {
                return Err(Error::MixedSession);
            }
            let cbor = super::ur::decode_single_part(
                parsed.as_bytewords().ok_or_else(|| {
                    Error::InvalidUr("single-part UR has no bytewords payload".to_owned())
                })?,
                self.limits.maximum_decoded_bytes,
            )?;
            let data = decode_registry_value(self.expected, &cbor)?;
            self.ensure_payload_limit(data.len())?;
            return Ok(DecodeProgress::Complete(UrPayload {
                ur_type: self.expected,
                data,
            }));
        }

        let sequence_count = parsed.sequence_count().ok_or(Error::Incomplete)?;
        if sequence_count > self.limits.maximum_fragment_count {
            return Err(Error::TooManyFragments {
                actual: sequence_count,
                maximum: self.limits.maximum_fragment_count,
            });
        }
        let bytewords = parsed
            .as_bytewords()
            .ok_or_else(|| Error::InvalidUr("multipart UR has no bytewords fragment".to_owned()))?;
        let decoded_size = bytewords::validate(bytewords, Style::Minimal)
            .map_err(|e| Error::InvalidUr(e.to_string()))?;
        if decoded_size > self.limits.maximum_fragment_bytes {
            return Err(Error::FragmentTooLarge {
                actual: decoded_size,
                maximum: self.limits.maximum_fragment_bytes,
            });
        }
        let mut decoded = vec![0u8; decoded_size];
        let written = bytewords::decode_to_slice(bytewords, &mut decoded, Style::Minimal)
            .map_err(|e| Error::InvalidUr(e.to_string()))?;
        decoded.truncate(written);
        let part: Part<'_> =
            minicbor::decode(&decoded).map_err(|e| Error::InvalidCbor(e.to_string()))?;
        if part.sequence_count > self.limits.maximum_fragment_count {
            return Err(Error::TooManyFragments {
                actual: part.sequence_count,
                maximum: self.limits.maximum_fragment_count,
            });
        }
        self.ensure_payload_limit(part.message_length)?;
        let padded = part
            .data
            .len()
            .checked_mul(part.sequence_count as usize)
            .ok_or(Error::PayloadTooLarge {
                actual: usize::MAX,
                maximum: self.limits.maximum_decoded_bytes,
            })?;
        self.ensure_payload_limit(padded)?;

        let safe_part = UR::MultiPartDeserialized {
            ur_type: self.expected.as_str(),
            fragment: part,
        };
        self.decoder.receive(safe_part).map_err(|e| match e {
            foundation_ur::decoder::Error::InconsistentType
            | foundation_ur::decoder::Error::Fountain(
                foundation_ur::fountain::decoder::Error::InconsistentPart { .. },
            ) => Error::MixedSession,
            _ => Error::InvalidUr(e.to_string()),
        })?;

        if self.decoder.is_complete() {
            let cbor = self
                .decoder
                .message()
                .map_err(|e| Error::InvalidUr(e.to_string()))?
                .ok_or(Error::Incomplete)?;
            self.ensure_payload_limit(cbor.len())?;
            let data = decode_registry_value(self.expected, cbor)?;
            self.ensure_payload_limit(data.len())?;
            Ok(DecodeProgress::Complete(UrPayload {
                ur_type: self.expected,
                data,
            }))
        } else {
            Ok(DecodeProgress::Incomplete {
                estimated: self.decoder.estimated_percent_complete() as f32,
            })
        }
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.decoder.clear();
    }

    pub fn restart(&mut self) {
        self.cancelled = false;
        self.started_at = None;
        self.decoder.clear();
    }

    fn ensure_payload_limit(&self, actual: usize) -> Result<(), Error> {
        if actual > self.limits.maximum_decoded_bytes {
            Err(Error::PayloadTooLarge {
                actual,
                maximum: self.limits.maximum_decoded_bytes,
            })
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::airgap::{encode_ur, UrPayload};

    #[test]
    fn multipart_accepts_reordering_and_duplicates() {
        let encoded = encode_ur(&UrPayload::bytes(vec![42; 1_024]), 100).unwrap();
        assert!(encoded.is_multipart());
        let mut frames = encoded.frames.clone();
        frames.reverse();
        frames.insert(1, frames[0].clone());
        let mut session = UrDecodeSession::new(UrType::Bytes, ScanLimits::default());
        let mut result = None;
        for frame in frames {
            if let DecodeProgress::Complete(payload) = session.receive(&frame).unwrap() {
                result = Some(payload);
                break;
            }
        }
        assert_eq!(result.unwrap().data, vec![42; 1_024]);
    }

    #[test]
    fn cancellation_requires_explicit_restart() {
        let encoded = encode_ur(&UrPayload::bytes(b"hello".to_vec()), 100).unwrap();
        let mut session = UrDecodeSession::new(UrType::Bytes, ScanLimits::default());
        session.cancel();
        assert_eq!(session.receive(&encoded.frames[0]), Err(Error::Cancelled));
        session.restart();
        assert!(matches!(
            session.receive(&encoded.frames[0]),
            Ok(DecodeProgress::Complete(_))
        ));
    }

    #[test]
    fn timeout_is_bounded() {
        let encoded = encode_ur(&UrPayload::bytes(vec![1; 500]), 100).unwrap();
        let mut session = UrDecodeSession::new(UrType::Bytes, ScanLimits::default());
        let start = Instant::now();
        session.receive_at(&encoded.frames[0], start).unwrap();
        assert_eq!(
            session.receive_at(&encoded.frames[1], start + Duration::from_secs(121)),
            Err(Error::TimedOut)
        );
    }
}
