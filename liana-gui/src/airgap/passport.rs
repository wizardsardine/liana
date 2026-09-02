use std::{
    collections::HashSet,
    convert::{TryFrom, TryInto},
    str::FromStr,
};

use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{hashes::Hash, Network},
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use super::Error;

pub const POLICY_FORMAT: &str = "passport-wallet-policy";
pub const ADDRESS_REQUEST_FORMAT: &str = "passport-address-verification";
pub const ADDRESS_RESPONSE_FORMAT: &str = "passport-address-verification-response";
pub const PROTOCOL_VERSION: u8 = 1;
pub const MAX_JSON_BYTES: usize = 4_096;
pub const MAX_JSON_DEPTH: usize = 16;
pub const MAX_DESCRIPTOR_LENGTH: usize = 4_096;
pub const MAX_TEMPLATE_LENGTH: usize = 2_048;
pub const MAX_KEYS: usize = 20;
pub const MAX_NAME_LENGTH: usize = 20;

const BASE58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyNetwork {
    BTC,
    TBTC,
}

impl TryFrom<Network> for PolicyNetwork {
    type Error = Error;

    fn try_from(value: Network) -> Result<Self, Self::Error> {
        match value {
            Network::Bitcoin => Ok(Self::BTC),
            Network::Testnet | Network::Testnet4 | Network::Signet | Network::Regtest => {
                Ok(Self::TBTC)
            }
            _ => Err(Error::InvalidNetwork),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRegistration {
    pub format: String,
    pub version: u8,
    pub name: String,
    pub network: PolicyNetwork,
    pub template: String,
    pub keys: Vec<String>,
    pub policy_id: String,
}

impl PolicyRegistration {
    pub fn new(
        name: impl Into<String>,
        network: PolicyNetwork,
        template: impl Into<String>,
        keys: Vec<String>,
    ) -> Result<Self, Error> {
        let registration = Self {
            format: POLICY_FORMAT.to_owned(),
            version: PROTOCOL_VERSION,
            name: name.into(),
            network,
            template: template.into(),
            keys,
            policy_id: String::new(),
        };
        registration.validate_without_id()?;
        Ok(Self {
            policy_id: registration.calculate_policy_id(),
            ..registration
        })
    }

    /// Convert Liana's canonical multipath descriptor to Passport's v1
    /// BIP388-style template and canonical key vector.
    pub fn from_descriptor(
        name: impl Into<String>,
        network: Network,
        descriptor: &LianaDescriptor,
    ) -> Result<Self, Error> {
        let supplied_name = name.into();
        let printable_name: String = supplied_name
            .chars()
            .filter(|character| character.is_ascii() && !character.is_ascii_control())
            .collect();
        let mut transport_name: String = printable_name
            .trim()
            .chars()
            .take(MAX_NAME_LENGTH)
            .collect::<String>()
            .trim_end()
            .to_owned();
        if transport_name.is_empty() {
            transport_name = "Liana".to_owned();
        }
        let descriptor = descriptor.to_string();
        let body = descriptor
            .rsplit_once('#')
            .map(|(body, _)| body)
            .ok_or(Error::InvalidChecksum)?;
        let (template, keys) = descriptor_to_template(body)?;
        Self::new(transport_name, network.try_into()?, template, keys)
    }

    pub fn descriptor_checksum(&self) -> Result<String, Error> {
        let descriptor = self.full_descriptor();
        let parsed = LianaDescriptor::from_str(&descriptor)
            .map_err(|e| Error::InvalidPolicy(e.to_string()))?;
        parsed
            .to_string()
            .rsplit_once('#')
            .map(|(_, checksum)| checksum.to_owned())
            .ok_or(Error::InvalidChecksum)
    }

    pub fn full_descriptor(&self) -> String {
        let mut descriptor = self.template.clone();
        for index in (0..self.keys.len()).rev() {
            descriptor = descriptor.replace(&format!("@{index}"), &self.keys[index]);
        }
        descriptor
    }

    pub fn calculate_policy_id(&self) -> String {
        let mut payload = b"Passport Wallet Policy\0".to_vec();
        payload.push(PROTOCOL_VERSION);
        encode_field(
            &mut payload,
            match self.network {
                PolicyNetwork::BTC => "BTC",
                PolicyNetwork::TBTC => "TBTC",
            },
        );
        encode_field(&mut payload, &self.template);
        compact_size(&mut payload, self.keys.len());
        for key in &self.keys {
            encode_field(&mut payload, key);
        }
        liana::miniscript::bitcoin::hashes::sha256::Hash::hash(&payload).to_string()
    }

    pub fn to_json(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        encode_json(self)
    }

    pub fn from_json(data: &[u8]) -> Result<Self, Error> {
        let value: Self = decode_json(data)?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), Error> {
        self.validate_without_id()?;
        if self.policy_id != self.calculate_policy_id() {
            return Err(Error::InvalidPolicy(
                "policy identity does not match its canonical contents".to_owned(),
            ));
        }
        // Reparse the reconstructed descriptor using Liana's own parser. This
        // preserves branch order, key order, threshold and timelock semantics.
        LianaDescriptor::from_str(&self.full_descriptor())
            .map_err(|e| Error::InvalidPolicy(e.to_string()))?;
        Ok(())
    }

    fn validate_without_id(&self) -> Result<(), Error> {
        if self.format != POLICY_FORMAT || self.version != PROTOCOL_VERSION {
            return Err(Error::InvalidPolicy(
                "unsupported wallet-policy envelope".to_owned(),
            ));
        }
        validate_ascii(&self.name, 1, MAX_NAME_LENGTH, "wallet name")?;
        validate_ascii(&self.template, 1, MAX_TEMPLATE_LENGTH, "policy template")?;
        if !(self.template.starts_with("wsh(") || self.template.starts_with("tr("))
            || !self.template.ends_with(')')
        {
            return Err(Error::InvalidPolicy(
                "policy template must be a top-level wsh() or tr() descriptor".to_owned(),
            ));
        }
        if self.keys.is_empty() || self.keys.len() > MAX_KEYS {
            return Err(Error::InvalidPolicy(format!(
                "policy must contain between 1 and {MAX_KEYS} keys"
            )));
        }
        let mut unique = HashSet::new();
        for key in &self.keys {
            let canonical = canonical_key(key)?;
            if canonical != *key {
                return Err(Error::InvalidPolicy("key is not canonical".to_owned()));
            }
            if !unique.insert(key) {
                return Err(Error::InvalidPolicy(
                    "policy key vector contains a duplicate".to_owned(),
                ));
            }
        }
        if self.full_descriptor().len() > MAX_DESCRIPTOR_LENGTH {
            return Err(Error::InvalidPolicy("descriptor is too large".to_owned()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddressVerificationRequest {
    pub format: String,
    pub version: u8,
    pub network: PolicyNetwork,
    pub policy_id: String,
    pub descriptor_checksum: String,
    pub branch: u32,
    pub index: u32,
}

impl AddressVerificationRequest {
    pub fn new(registration: &PolicyRegistration, branch: u32, index: u32) -> Result<Self, Error> {
        if branch > 1 {
            return Err(Error::InvalidPolicy("branch must be 0 or 1".to_owned()));
        }
        Ok(Self {
            format: ADDRESS_REQUEST_FORMAT.to_owned(),
            version: PROTOCOL_VERSION,
            network: registration.network,
            policy_id: registration.policy_id.clone(),
            descriptor_checksum: registration.descriptor_checksum()?,
            branch,
            index,
        })
    }

    pub fn to_json(&self) -> Result<Vec<u8>, Error> {
        encode_json(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedAddress {
    pub format: String,
    pub version: u8,
    pub network: PolicyNetwork,
    pub policy_id: String,
    pub descriptor_checksum: String,
    pub branch: u32,
    pub index: u32,
    pub address: String,
    pub fingerprint: String,
}

impl VerifiedAddress {
    pub fn from_json(data: &[u8]) -> Result<Self, Error> {
        decode_json(data)
    }

    pub fn validate_for(
        &self,
        request: &AddressVerificationRequest,
        expected_address: &str,
        fingerprint: &str,
    ) -> Result<(), Error> {
        validate_fingerprint(&self.fingerprint)?;
        if self.format != ADDRESS_RESPONSE_FORMAT
            || self.version != PROTOCOL_VERSION
            || self.network != request.network
            || self.policy_id != request.policy_id
            || self.descriptor_checksum != request.descriptor_checksum
            || self.branch != request.branch
            || self.index != request.index
            || self.address != expected_address
            || !self.fingerprint.eq_ignore_ascii_case(fingerprint)
        {
            return Err(Error::WrongResponseType);
        }
        Ok(())
    }
}

pub(crate) fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    let encoded = serde_json::to_vec(value).map_err(|e| Error::InvalidJson(e.to_string()))?;
    if encoded.len() > MAX_JSON_BYTES {
        return Err(Error::PayloadTooLarge {
            actual: encoded.len(),
            maximum: MAX_JSON_BYTES,
        });
    }
    Ok(encoded)
}

pub(crate) fn decode_json<T: DeserializeOwned>(data: &[u8]) -> Result<T, Error> {
    if data.is_empty() {
        return Err(Error::Empty);
    }
    if data.len() > MAX_JSON_BYTES {
        return Err(Error::PayloadTooLarge {
            actual: data.len(),
            maximum: MAX_JSON_BYTES,
        });
    }
    validate_json_depth(data, MAX_JSON_DEPTH)?;
    serde_json::from_slice(data).map_err(|e| Error::InvalidJson(e.to_string()))
}

fn validate_json_depth(data: &[u8], maximum: usize) -> Result<(), Error> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for &byte in data {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                if depth > maximum {
                    return Err(Error::JsonTooDeep { maximum });
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

fn descriptor_to_template(body: &str) -> Result<(String, Vec<String>), Error> {
    if body.len() > MAX_DESCRIPTOR_LENGTH || !body.is_ascii() {
        return Err(Error::InvalidPolicy(
            "descriptor is non-ASCII or too large".to_owned(),
        ));
    }
    let bytes = body.as_bytes();
    let mut output = String::with_capacity(body.len());
    let mut keys = Vec::new();
    let mut position = 0usize;
    while position < bytes.len() {
        if bytes[position] != b'[' {
            output.push(char::from(bytes[position]));
            position += 1;
            continue;
        }
        let close = body[position + 1..]
            .find(']')
            .map(|offset| position + 1 + offset)
            .ok_or_else(|| Error::InvalidPolicy("key origin is incomplete".to_owned()))?;
        let mut xpub_end = close + 1;
        while xpub_end < bytes.len() && BASE58.as_bytes().contains(&bytes[xpub_end]) {
            xpub_end += 1;
        }
        if xpub_end == close + 1 {
            return Err(Error::InvalidPolicy(
                "key origin is not followed by an extended public key".to_owned(),
            ));
        }
        let key = canonical_key(&body[position..xpub_end])?;
        let (suffix, next) = if body[xpub_end..].starts_with("/**") {
            ("/**".to_owned(), xpub_end + 3)
        } else if body[xpub_end..].starts_with("/<") {
            let relative_end = body[xpub_end + 2..]
                .find(">/*")
                .ok_or_else(|| Error::InvalidPolicy("multipath suffix is incomplete".to_owned()))?;
            let suffix_end = xpub_end + 2 + relative_end;
            let branches = &body[xpub_end + 2..suffix_end];
            let mut parts = branches.split(';');
            let first = canonical_number(parts.next())?;
            let second = canonical_number(parts.next())?;
            if parts.next().is_some() || first == second {
                return Err(Error::InvalidPolicy(
                    "exactly two distinct multipath branches are required".to_owned(),
                ));
            }
            (format!("/<{first};{second}>/*"), suffix_end + 3)
        } else {
            return Err(Error::InvalidPolicy(
                "extended keys must end in /** or /<M;N>/*".to_owned(),
            ));
        };
        let key_index = match keys.iter().position(|existing| existing == &key) {
            Some(index) => index,
            None => {
                if keys.len() == MAX_KEYS {
                    return Err(Error::InvalidPolicy("too many keys".to_owned()));
                }
                keys.push(key);
                keys.len() - 1
            }
        };
        output.push_str(&format!("@{key_index}{suffix}"));
        position = next;
    }
    Ok((output, keys))
}

fn canonical_key(key: &str) -> Result<String, Error> {
    if !key.is_ascii() || !key.starts_with('[') {
        return Err(Error::InvalidPolicy("key origin is required".to_owned()));
    }
    let close = key
        .find(']')
        .ok_or_else(|| Error::InvalidPolicy("key origin is incomplete".to_owned()))?;
    let origin = &key[1..close];
    let xpub = &key[close + 1..];
    if !(100..=120).contains(&xpub.len()) || !xpub.bytes().all(|b| BASE58.as_bytes().contains(&b)) {
        return Err(Error::InvalidPolicy(
            "extended public key encoding is invalid".to_owned(),
        ));
    }
    let mut components = origin.split('/');
    let fingerprint = components
        .next()
        .ok_or(Error::InvalidFingerprint)?
        .to_ascii_lowercase();
    validate_fingerprint(&fingerprint)?;
    let mut canonical = format!("[{fingerprint}");
    for component in components {
        if component.is_empty() {
            return Err(Error::InvalidPolicy("empty origin component".to_owned()));
        }
        let hardened = component.ends_with(['\'', 'h', 'H']);
        let number = if hardened {
            &component[..component.len() - 1]
        } else {
            component
        };
        let value = canonical_number(Some(number))?;
        canonical.push('/');
        canonical.push_str(&value.to_string());
        if hardened {
            canonical.push('\'');
        }
    }
    canonical.push(']');
    canonical.push_str(xpub);
    Ok(canonical)
}

fn canonical_number(number: Option<&str>) -> Result<u32, Error> {
    let number = number.ok_or_else(|| Error::InvalidPolicy("missing number".to_owned()))?;
    if number.is_empty()
        || !number.bytes().all(|b| b.is_ascii_digit())
        || (number.len() > 1 && number.starts_with('0'))
    {
        return Err(Error::InvalidPolicy("number is not canonical".to_owned()));
    }
    let value = number
        .parse::<u32>()
        .map_err(|_| Error::InvalidPolicy("number is too large".to_owned()))?;
    if value >= (1 << 31) {
        return Err(Error::InvalidPolicy("number is too large".to_owned()));
    }
    Ok(value)
}

fn validate_ascii(value: &str, minimum: usize, maximum: usize, field: &str) -> Result<(), Error> {
    if !(minimum..=maximum).contains(&value.len())
        || !value.is_ascii()
        || value.trim() != value
        || value.bytes().any(|b| !(32..=126).contains(&b))
    {
        return Err(Error::InvalidPolicy(format!("invalid {field}")));
    }
    Ok(())
}

fn validate_fingerprint(value: &str) -> Result<(), Error> {
    if value.len() == 8 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(Error::InvalidFingerprint)
    }
}

fn compact_size(output: &mut Vec<u8>, value: usize) {
    if value < 253 {
        output.push(value as u8);
    } else if value <= u16::MAX as usize {
        output.push(253);
        output.extend_from_slice(&(value as u16).to_le_bytes());
    } else {
        output.push(254);
        output.extend_from_slice(&(value as u32).to_le_bytes());
    }
}

fn encode_field(output: &mut Vec<u8>, value: &str) {
    compact_size(output, value.len());
    output.extend_from_slice(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIANA_XPUB_1: &str = "xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW";
    const LIANA_XPUB_2: &str = "xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe";

    fn registration() -> PolicyRegistration {
        PolicyRegistration::new(
            "Recovery",
            PolicyNetwork::BTC,
            "wsh(or_d(pk(@0/<0;1>/*),and_v(v:pkh(@1/<0;1>/*),older(52560))))",
            vec![
                format!("[abcdef01]{LIANA_XPUB_1}"),
                format!("[abcdef02]{LIANA_XPUB_2}"),
            ],
        )
        .unwrap()
    }

    #[test]
    fn passport_policy_id_matches_reference_algorithm() {
        let registration = registration();
        // Generated independently by Passport Core's MiniscriptPolicy v1.
        assert_eq!(
            registration.policy_id,
            "506b3dd1ce28b757cde12e2977c483b0afb518de9ad8edbdfbc01e5d9763dd9f"
        );
        assert_eq!(registration.descriptor_checksum().unwrap(), "y7qrgwup");
    }

    #[test]
    fn wallet_alias_is_safely_mapped_to_passport_name_limits() {
        let source = registration();
        let descriptor = LianaDescriptor::from_str(&source.full_descriptor()).unwrap();
        let mapped = PolicyRegistration::from_descriptor(
            "  Family 🔐 inheritance wallet with a long name  ",
            Network::Bitcoin,
            &descriptor,
        )
        .unwrap();
        assert_eq!(mapped.name, "Family  inheritance");
        assert!(mapped.name.len() <= MAX_NAME_LENGTH);

        let fallback =
            PolicyRegistration::from_descriptor("🔐🔐", Network::Bitcoin, &descriptor).unwrap();
        assert_eq!(fallback.name, "Liana");
    }

    #[test]
    fn json_depth_is_bounded_before_deserialization() {
        let deeply_nested = format!("{}0{}", "[".repeat(17), "]".repeat(17));
        assert_eq!(
            decode_json::<serde_json::Value>(deeply_nested.as_bytes()),
            Err(Error::JsonTooDeep { maximum: 16 })
        );
    }

    #[test]
    fn address_response_is_bound_to_request() {
        let registration = registration();
        let descriptor = LianaDescriptor::from_str(&registration.full_descriptor()).unwrap();
        let address = descriptor
            .receive_descriptor()
            .derive(
                7.into(),
                &liana::miniscript::bitcoin::secp256k1::Secp256k1::verification_only(),
            )
            .address(Network::Bitcoin)
            .to_string();
        let request = AddressVerificationRequest::new(&registration, 0, 7).unwrap();
        let response = VerifiedAddress {
            format: ADDRESS_RESPONSE_FORMAT.to_owned(),
            version: 1,
            network: request.network,
            policy_id: request.policy_id.clone(),
            descriptor_checksum: request.descriptor_checksum.clone(),
            branch: 0,
            index: 7,
            address: address.clone(),
            fingerprint: "abcdef01".to_owned(),
        };
        response
            .validate_for(&request, &address, "abcdef01")
            .unwrap();
        assert_eq!(
            response.validate_for(&request, "bc1qother", "abcdef01"),
            Err(Error::WrongResponseType)
        );
    }
}
