use foundation_ur::{
    bytewords::{self, Style},
    Encoder, UR,
};
use liana::miniscript::bitcoin::psbt::Psbt;

use super::Error;

// Passport Core and Passport Prime share Foundation's bounded BC-UR decoder.
// Keep the encoded registry message within the device's 24 KiB ceiling; larger
// binary PSBTs remain available through the bounded microSD path.
const PASSPORT_MAX_UR_MESSAGE_BYTES: usize = 24 * 1024;
const PASSPORT_MAX_UR_FRAGMENTS: u32 = 128;

// Keep three deliberately separated presets so changing density has a visible
// effect. The lowest tier helps signers with less capable cameras by halving
// the data in each frame relative to Low. The encoder also enforces the
// signer's fragment ceiling, so large payloads must use a denser tier.
const QR_FRAGMENT_LENGTHS: [usize; 3] = [60, 120, 400];

/// User-adjustable amount of data carried by each animated QR frame.
/// Lower levels make simpler QR symbols at the cost of additional frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QrDensity(u8);

impl QrDensity {
    const DEFAULT_LEVEL: u8 = 1;

    pub fn fragment_length(self) -> usize {
        QR_FRAGMENT_LENGTHS[usize::from(self.0)]
    }

    pub fn label(self) -> &'static str {
        match self.0 {
            0 => "Very low",
            1 => "Low",
            _ => "High",
        }
    }

    pub fn less_dense(self) -> Option<Self> {
        self.0.checked_sub(1).map(Self)
    }

    pub fn more_dense(self) -> Option<Self> {
        let next = self.0 + 1;
        (usize::from(next) < QR_FRAGMENT_LENGTHS.len()).then_some(Self(next))
    }
}

impl Default for QrDensity {
    fn default() -> Self {
        Self(Self::DEFAULT_LEVEL)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrType {
    Bytes,
    CryptoPsbt,
    CryptoAccount,
}

impl UrType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bytes => "bytes",
            Self::CryptoPsbt => "crypto-psbt",
            Self::CryptoAccount => "crypto-account",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrPayload {
    pub ur_type: UrType,
    /// Registry value data. `bytes` and `crypto-psbt` have their CBOR byte
    /// string removed; `crypto-account` remains registry CBOR for typed account
    /// decoding.
    pub data: Vec<u8>,
}

impl UrPayload {
    pub fn bytes(data: impl Into<Vec<u8>>) -> Self {
        Self {
            ur_type: UrType::Bytes,
            data: data.into(),
        }
    }

    pub fn psbt(psbt: &Psbt) -> Self {
        Self {
            ur_type: UrType::CryptoPsbt,
            data: psbt.serialize(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedUr {
    pub ur_type: UrType,
    pub frames: Vec<String>,
}

impl EncodedUr {
    pub fn is_multipart(&self) -> bool {
        self.frames.len() > 1
    }
}

/// Encode one complete deterministic cycle of a BC-UR v2 stream.
pub fn encode_ur(payload: &UrPayload, max_fragment_length: usize) -> Result<EncodedUr, Error> {
    if payload.data.is_empty() {
        return Err(Error::Empty);
    }
    if max_fragment_length == 0 {
        return Err(Error::InvalidUr(
            "maximum fragment length must be positive".to_owned(),
        ));
    }
    let cbor = match payload.ur_type {
        UrType::Bytes | UrType::CryptoPsbt => encode_cbor_bytes(&payload.data)?,
        UrType::CryptoAccount => payload.data.clone(),
    };
    if cbor.len() > PASSPORT_MAX_UR_MESSAGE_BYTES {
        return Err(Error::PayloadTooLarge {
            actual: cbor.len(),
            maximum: PASSPORT_MAX_UR_MESSAGE_BYTES,
        });
    }
    let frames = if cbor.len() <= max_fragment_length {
        vec![UR::new(payload.ur_type.as_str(), &cbor).to_string()]
    } else {
        let mut encoder = Encoder::new();
        encoder.start(payload.ur_type.as_str(), &cbor, max_fragment_length);
        let count = encoder.sequence_count();
        if count > PASSPORT_MAX_UR_FRAGMENTS {
            return Err(Error::TooManyFragments {
                actual: count,
                maximum: PASSPORT_MAX_UR_FRAGMENTS,
            });
        }
        (0..count)
            .map(|_| encoder.next_part().to_string())
            .collect()
    };
    Ok(EncodedUr {
        ur_type: payload.ur_type,
        frames,
    })
}

pub(crate) fn decode_registry_value(ur_type: UrType, cbor: &[u8]) -> Result<Vec<u8>, Error> {
    if ur_type == UrType::CryptoAccount {
        return Ok(cbor.to_vec());
    }
    let mut decoder = minicbor::Decoder::new(cbor);
    let bytes = decoder
        .bytes()
        .map_err(|e| Error::InvalidCbor(e.to_string()))?;
    if decoder.position() != cbor.len() {
        return Err(Error::InvalidCbor("trailing CBOR data".to_owned()));
    }
    Ok(bytes.to_vec())
}

pub(crate) fn decode_single_part(encoded: &str, maximum: usize) -> Result<Vec<u8>, Error> {
    let size = bytewords::validate(encoded, Style::Minimal)
        .map_err(|e| Error::InvalidUr(e.to_string()))?;
    if size > maximum {
        return Err(Error::PayloadTooLarge {
            actual: size,
            maximum,
        });
    }
    let mut decoded = vec![0u8; size];
    let written = bytewords::decode_to_slice(encoded, &mut decoded, Style::Minimal)
        .map_err(|e| Error::InvalidUr(e.to_string()))?;
    decoded.truncate(written);
    Ok(decoded)
}

fn encode_cbor_bytes(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut encoder = minicbor::Encoder::new(Vec::with_capacity(data.len() + 5));
    encoder
        .bytes(data)
        .map_err(|e| Error::InvalidCbor(e.to_string()))?;
    Ok(encoder.into_writer())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_ur_respects_passport_decoder_ceiling() {
        assert!(encode_ur(
            &UrPayload::bytes(vec![0; PASSPORT_MAX_UR_MESSAGE_BYTES]),
            250
        )
        .is_err());
        assert!(encode_ur(&UrPayload::bytes(vec![0; 23 * 1024]), 250).is_ok());
    }

    #[test]
    fn outgoing_ur_respects_passport_fragment_ceiling() {
        assert!(matches!(
            encode_ur(&UrPayload::bytes(vec![0; 23 * 1024]), 120),
            Err(Error::TooManyFragments { maximum: 128, .. })
        ));
    }

    #[test]
    fn qr_density_adjustments_are_bounded_and_monotonic() {
        let low = QrDensity::default();
        let very_low = low.less_dense().unwrap();
        let high = low.more_dense().unwrap();

        assert_eq!(very_low.label(), "Very low");
        assert_eq!(low.label(), "Low");
        assert_eq!(high.label(), "High");
        assert!(very_low.fragment_length() < low.fragment_length());
        assert!(low.fragment_length() < high.fragment_length());
        assert!(very_low.less_dense().is_none());
        assert!(high.more_dense().is_none());
        assert_eq!(low.less_dense(), Some(very_low));
        assert_eq!(very_low.more_dense(), Some(low));
        assert_eq!(high.less_dense(), Some(low));

        let payload = UrPayload::bytes(vec![42; 1_000]);
        let simplest = encode_ur(&payload, very_low.fragment_length()).unwrap();
        let simpler = encode_ur(&payload, low.fragment_length()).unwrap();
        let denser = encode_ur(&payload, high.fragment_length()).unwrap();
        assert!(simplest.frames.len() >= simpler.frames.len() * 2 - 1);
        assert!(simpler.frames.len() >= denser.frames.len() * 2);
        let simplest_frame_length = simplest.frames.iter().map(String::len).max().unwrap();
        let simpler_frame_length = simpler.frames.iter().map(String::len).max().unwrap();
        let denser_frame_length = denser.frames.iter().map(String::len).max().unwrap();
        assert!(simpler_frame_length >= simplest_frame_length * 3 / 2);
        assert!(denser_frame_length >= simpler_frame_length * 2);
    }
}
