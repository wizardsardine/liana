use std::{collections::HashSet, convert::TryInto, str::FromStr};

use liana::miniscript::{
    bitcoin::{
        bip32::{ChainCode, ChildNumber, DerivationPath, Fingerprint, Xpub},
        ecdsa,
        psbt::Psbt,
        secp256k1::{self, PublicKey, Secp256k1},
        sighash::SighashCache,
        taproot::{self, TapLeafHash},
        Network, NetworkKind,
    },
    descriptor::DescriptorPublicKey,
    psbt::PsbtExt,
};

use super::{
    passport::decode_json, AddressVerificationRequest, Error, PolicyNetwork, PolicyRegistration,
    UrPayload, UrType, VerifiedAddress,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassportAccount {
    pub fingerprint: Fingerprint,
    pub account: DescriptorPublicKey,
    pub network: PolicyNetwork,
}

impl PassportAccount {
    pub fn account_number(&self) -> Result<ChildNumber, Error> {
        let origin = match &self.account {
            DescriptorPublicKey::XPub(xpub) => xpub.origin.as_ref().map(|(_, path)| path),
            _ => None,
        }
        .ok_or_else(|| Error::InvalidAccount("extended key origin is required".to_owned()))?;
        origin
            .into_iter()
            .nth(2)
            .copied()
            .ok_or_else(|| Error::InvalidAccount("BIP48 account component is missing".to_owned()))
    }

    /// Decode the deliberately narrow `crypto-account` profile used for a
    /// Passport BIP48 native-SegWit cosigner export.
    pub fn from_crypto_account_cbor(data: &[u8], expected_network: Network) -> Result<Self, Error> {
        // CBOR map ordering is not significant. Validate the envelope and read
        // its fingerprint first, then decode output descriptors in a second
        // bounded pass so standards-compliant encoders may emit either order.
        let mut decoder = minicbor::Decoder::new(data);
        let map_len = decoder
            .map()
            .map_err(|e| Error::InvalidAccount(e.to_string()))?
            .ok_or_else(|| {
                Error::InvalidAccount("indefinite account maps are forbidden".to_owned())
            })?;
        let mut seen = HashSet::new();
        let mut master_fingerprint = None;
        let mut has_outputs = false;
        for _ in 0..map_len {
            let key = decoder
                .u32()
                .map_err(|e| Error::InvalidAccount(e.to_string()))?;
            if !seen.insert(key) {
                return Err(Error::InvalidAccount(
                    "duplicate crypto-account map entry".to_owned(),
                ));
            }
            match key {
                1 => {
                    master_fingerprint = Some(
                        decoder
                            .u32()
                            .map_err(|e| Error::InvalidAccount(e.to_string()))?,
                    )
                }
                2 => {
                    has_outputs = true;
                    decoder
                        .skip()
                        .map_err(|e| Error::InvalidAccount(e.to_string()))?;
                }
                _ => {
                    return Err(Error::InvalidAccount(
                        "unknown crypto-account map entry".to_owned(),
                    ))
                }
            }
        }
        if decoder.position() != data.len() {
            return Err(Error::InvalidAccount("trailing CBOR data".to_owned()));
        }
        let fingerprint = master_fingerprint
            .ok_or_else(|| Error::InvalidAccount("master fingerprint is required".to_owned()))?;
        if !has_outputs {
            return Err(Error::InvalidAccount(
                "output descriptors are required".to_owned(),
            ));
        }

        let mut decoder = minicbor::Decoder::new(data);
        let map_len = decoder
            .map()
            .map_err(|e| Error::InvalidAccount(e.to_string()))?
            .ok_or_else(|| {
                Error::InvalidAccount("indefinite account maps are forbidden".to_owned())
            })?;
        let mut accounts = Vec::new();
        for _ in 0..map_len {
            match decoder
                .u32()
                .map_err(|e| Error::InvalidAccount(e.to_string()))?
            {
                1 => {
                    decoder
                        .skip()
                        .map_err(|e| Error::InvalidAccount(e.to_string()))?;
                }
                2 => {
                    let len = decoder
                        .array()
                        .map_err(|e| Error::InvalidAccount(e.to_string()))?
                        .ok_or_else(|| {
                            Error::InvalidAccount(
                                "indefinite output descriptor arrays are forbidden".to_owned(),
                            )
                        })?;
                    if len == 0 || len > 16 {
                        return Err(Error::InvalidAccount(
                            "crypto-account must contain 1 to 16 outputs".to_owned(),
                        ));
                    }
                    for _ in 0..len {
                        let mut candidate = decoder.clone();
                        if let Ok(account) =
                            decode_bip48_cosigner(&mut candidate, fingerprint, expected_network)
                        {
                            accounts.push(account);
                        }
                        decoder
                            .skip()
                            .map_err(|e| Error::InvalidAccount(e.to_string()))?;
                    }
                }
                _ => {
                    return Err(Error::InvalidAccount(
                        "unknown crypto-account map entry".to_owned(),
                    ));
                }
            }
        }
        if accounts.len() != 1 {
            return Err(Error::InvalidAccount(
                "expected exactly one BIP48 native-SegWit cosigner".to_owned(),
            ));
        }
        Ok(accounts.remove(0))
    }

    /// Decode Passport's current microSD fallback:
    /// `[fingerprint/48'/coin_type'/account'/2']xpub-or-tpub`.
    pub fn from_descriptor_key(value: &str, expected_network: Network) -> Result<Self, Error> {
        let value = value.trim();
        if value.contains("xprv") || value.contains("tprv") {
            return Err(Error::InvalidAccount(
                "private key material is forbidden".to_owned(),
            ));
        }
        let account = DescriptorPublicKey::from_str(value)
            .map_err(|e| Error::InvalidAccount(e.to_string()))?;
        let (origin, xkey) = match &account {
            DescriptorPublicKey::XPub(xpub) => (xpub.origin.as_ref(), &xpub.xkey),
            _ => {
                return Err(Error::InvalidAccount(
                    "expected one non-wildcard extended public key".to_owned(),
                ))
            }
        };
        let (fingerprint, path) = origin.ok_or_else(|| {
            Error::InvalidAccount("master fingerprint and origin are required".to_owned())
        })?;
        let components: Vec<_> = path.into_iter().copied().collect();
        if components.len() != 4
            || components[0].to_string() != "48'"
            || components[3].to_string() != "2'"
            || components.iter().any(|child| !child.is_hardened())
            || xkey.depth as usize != components.len()
        {
            return Err(Error::InvalidAccount(
                "expected BIP48 native-SegWit origin m/48'/coin_type'/account'/2'".to_owned(),
            ));
        }
        let coin_type = match components[1] {
            ChildNumber::Hardened { index } => index,
            ChildNumber::Normal { .. } => {
                return Err(Error::InvalidAccount(
                    "coin type must be hardened".to_owned(),
                ))
            }
        };
        let network = if coin_type == 0 {
            PolicyNetwork::BTC
        } else if coin_type == 1 {
            PolicyNetwork::TBTC
        } else {
            return Err(Error::InvalidNetwork);
        };
        let expected: PolicyNetwork = expected_network.try_into()?;
        if network != expected || xkey.network != expected_network.into() {
            return Err(Error::InvalidNetwork);
        }
        Ok(Self {
            fingerprint: *fingerprint,
            account,
            network,
        })
    }
}

#[derive(Debug, Clone)]
pub enum AirgappedRequest {
    RegisterPolicy(PolicyRegistration),
    VerifyAddress(AddressVerificationRequest),
    SignPsbt(Psbt),
}

impl AirgappedRequest {
    pub fn encode(&self) -> Result<UrPayload, Error> {
        match self {
            Self::RegisterPolicy(policy) => Ok(UrPayload::bytes(policy.to_json()?)),
            Self::VerifyAddress(request) => Ok(UrPayload::bytes(request.to_json()?)),
            Self::SignPsbt(psbt) => Ok(UrPayload::psbt(psbt)),
        }
    }

    pub fn expected_response(&self) -> Option<ExpectedResponse> {
        match self {
            Self::RegisterPolicy(_) => None,
            Self::VerifyAddress(_) => Some(ExpectedResponse::VerifiedAddress),
            Self::SignPsbt(_) => Some(ExpectedResponse::SignedPsbt),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedResponse {
    VerifiedAddress,
    SignedPsbt,
}

impl ExpectedResponse {
    pub const fn ur_type(self) -> UrType {
        match self {
            Self::VerifiedAddress => UrType::Bytes,
            Self::SignedPsbt => UrType::CryptoPsbt,
        }
    }

    pub fn decode(self, payload: UrPayload) -> Result<AirgappedResponse, Error> {
        if payload.ur_type != self.ur_type() {
            return Err(Error::WrongUrType {
                expected: self.ur_type().as_str(),
                actual: payload.ur_type.as_str().to_owned(),
            });
        }
        match self {
            Self::VerifiedAddress => Ok(AirgappedResponse::VerifiedAddress(decode_json(
                &payload.data,
            )?)),
            Self::SignedPsbt => Psbt::deserialize(&payload.data)
                .map(AirgappedResponse::SignedPsbt)
                .map_err(|e| Error::InvalidPsbt(e.to_string())),
        }
    }
}

fn decode_bip48_cosigner(
    decoder: &mut minicbor::Decoder<'_>,
    account_fingerprint: u32,
    expected_network: Network,
) -> Result<PassportAccount, Error> {
    expect_tag(decoder, 308)?; // crypto-output
    expect_tag(decoder, 401)?; // wsh()
    expect_tag(decoder, 410)?; // cosigner()
    expect_tag(decoder, 303)?; // crypto-hdkey

    let map_len = decoder
        .map()
        .map_err(|e| Error::InvalidAccount(e.to_string()))?
        .ok_or_else(|| Error::InvalidAccount("indefinite HD key maps are forbidden".to_owned()))?;
    let mut is_private = false;
    let mut key_data = None;
    let mut chain_code = None;
    let mut network = PolicyNetwork::BTC;
    let mut origin = None;
    let mut parent_fingerprint = None;
    let mut seen = HashSet::new();
    for _ in 0..map_len {
        let key = decoder
            .u32()
            .map_err(|e| Error::InvalidAccount(e.to_string()))?;
        if !seen.insert(key) {
            return Err(Error::InvalidAccount(
                "duplicate crypto-hdkey map entry".to_owned(),
            ));
        }
        match key {
            2 => {
                is_private = decoder
                    .bool()
                    .map_err(|e| Error::InvalidAccount(e.to_string()))?
            }
            3 => {
                let bytes = decoder
                    .bytes()
                    .map_err(|e| Error::InvalidAccount(e.to_string()))?;
                if bytes.len() != 33 {
                    return Err(Error::InvalidAccount(
                        "HD public key data must contain 33 bytes".to_owned(),
                    ));
                }
                key_data = Some(bytes.to_vec());
            }
            4 => {
                let bytes = decoder
                    .bytes()
                    .map_err(|e| Error::InvalidAccount(e.to_string()))?;
                if bytes.len() != 32 {
                    return Err(Error::InvalidAccount(
                        "HD chain code must contain 32 bytes".to_owned(),
                    ));
                }
                let mut code = [0u8; 32];
                code.copy_from_slice(bytes);
                chain_code = Some(code);
            }
            5 => {
                expect_tag(decoder, 40305)?;
                network = decode_coin_info(decoder)?;
            }
            6 => {
                expect_tag(decoder, 40304)?;
                origin = Some(decode_keypath(decoder)?);
            }
            7 => {
                return Err(Error::InvalidAccount(
                    "account-level exports must not contain child derivations".to_owned(),
                ))
            }
            8 => {
                parent_fingerprint = Some(
                    decoder
                        .u32()
                        .map_err(|e| Error::InvalidAccount(e.to_string()))?,
                )
            }
            9 | 10 => {
                decoder
                    .str()
                    .map_err(|e| Error::InvalidAccount(e.to_string()))?;
            }
            _ => {
                return Err(Error::InvalidAccount(
                    "unknown crypto-hdkey map entry".to_owned(),
                ))
            }
        }
    }
    if is_private {
        return Err(Error::InvalidAccount(
            "private key material is forbidden".to_owned(),
        ));
    }
    let expected: PolicyNetwork = expected_network.try_into()?;
    if network != expected {
        return Err(Error::InvalidNetwork);
    }
    let (path, source_fingerprint) =
        origin.ok_or_else(|| Error::InvalidAccount("HD key origin is required".to_owned()))?;
    if let Some(source) = source_fingerprint {
        if source != account_fingerprint {
            return Err(Error::InvalidFingerprint);
        }
    }
    if path.len() != 4
        || !matches!(path[0], ChildNumber::Hardened { index: 48 })
        || !matches!(path[1], ChildNumber::Hardened { index: 0 | 1 })
        || !matches!(path[2], ChildNumber::Hardened { .. })
        || !matches!(path[3], ChildNumber::Hardened { index: 2 })
    {
        return Err(Error::InvalidAccount(
            "expected BIP48 native-SegWit origin m/48'/coin_type'/account'/2'".to_owned(),
        ));
    }
    let path_network = match path[1] {
        ChildNumber::Hardened { index: 0 } => PolicyNetwork::BTC,
        ChildNumber::Hardened { index: 1 } => PolicyNetwork::TBTC,
        _ => return Err(Error::InvalidNetwork),
    };
    if path_network != network {
        return Err(Error::InvalidNetwork);
    }

    let public_key = PublicKey::from_slice(
        &key_data.ok_or_else(|| Error::InvalidAccount("HD public key is required".to_owned()))?,
    )
    .map_err(|e| Error::InvalidAccount(e.to_string()))?;
    let fingerprint_bytes = account_fingerprint.to_be_bytes();
    let fingerprint = Fingerprint::from(&fingerprint_bytes);
    let xpub = Xpub {
        network: match network {
            PolicyNetwork::BTC => NetworkKind::Main,
            PolicyNetwork::TBTC => NetworkKind::Test,
        },
        depth: path.len() as u8,
        parent_fingerprint: Fingerprint::from(
            &parent_fingerprint
                .ok_or_else(|| Error::InvalidAccount("parent fingerprint is required".to_owned()))?
                .to_be_bytes(),
        ),
        child_number: *path.last().expect("BIP48 path has four elements"),
        public_key,
        chain_code: ChainCode::from(
            &chain_code
                .ok_or_else(|| Error::InvalidAccount("chain code is required".to_owned()))?,
        ),
    };
    let origin = DerivationPath::from(path);
    let origin = origin.to_string();
    let origin = origin.strip_prefix("m/").unwrap_or(&origin);
    let account = DescriptorPublicKey::from_str(&format!("[{fingerprint}/{origin}]{xpub}"))
        .map_err(|e| Error::InvalidAccount(e.to_string()))?;
    Ok(PassportAccount {
        fingerprint,
        account,
        network,
    })
}

fn expect_tag(decoder: &mut minicbor::Decoder<'_>, expected: u64) -> Result<(), Error> {
    let actual = decoder
        .tag()
        .map_err(|e| Error::InvalidAccount(e.to_string()))?;
    if actual.as_u64() == expected {
        Ok(())
    } else {
        Err(Error::InvalidAccount(format!(
            "expected CBOR tag {expected}, received {}",
            actual.as_u64()
        )))
    }
}

fn decode_coin_info(decoder: &mut minicbor::Decoder<'_>) -> Result<PolicyNetwork, Error> {
    let len = decoder
        .map()
        .map_err(|e| Error::InvalidAccount(e.to_string()))?
        .ok_or_else(|| {
            Error::InvalidAccount("indefinite coin-info maps are forbidden".to_owned())
        })?;
    let mut coin_type = 0u32;
    let mut network = 0u64;
    let mut seen = HashSet::new();
    for _ in 0..len {
        let key = decoder
            .u32()
            .map_err(|e| Error::InvalidAccount(e.to_string()))?;
        if !seen.insert(key) {
            return Err(Error::InvalidAccount(
                "duplicate coin-info map entry".to_owned(),
            ));
        }
        match key {
            1 => {
                coin_type = decoder
                    .u32()
                    .map_err(|e| Error::InvalidAccount(e.to_string()))?
            }
            2 => {
                network = decoder
                    .u64()
                    .map_err(|e| Error::InvalidAccount(e.to_string()))?
            }
            _ => return Err(Error::InvalidAccount("unknown coin-info entry".to_owned())),
        }
    }
    if coin_type != 0 {
        return Err(Error::InvalidNetwork);
    }
    match network {
        0 => Ok(PolicyNetwork::BTC),
        1 => Ok(PolicyNetwork::TBTC),
        _ => Err(Error::InvalidNetwork),
    }
}

fn decode_keypath(
    decoder: &mut minicbor::Decoder<'_>,
) -> Result<(Vec<ChildNumber>, Option<u32>), Error> {
    let len = decoder
        .map()
        .map_err(|e| Error::InvalidAccount(e.to_string()))?
        .ok_or_else(|| Error::InvalidAccount("indefinite keypath maps are forbidden".to_owned()))?;
    let mut path = None;
    let mut source = None;
    let mut seen = HashSet::new();
    for _ in 0..len {
        let key = decoder
            .u32()
            .map_err(|e| Error::InvalidAccount(e.to_string()))?;
        if !seen.insert(key) {
            return Err(Error::InvalidAccount(
                "duplicate keypath map entry".to_owned(),
            ));
        }
        match key {
            1 => {
                let components = decoder
                    .array()
                    .map_err(|e| Error::InvalidAccount(e.to_string()))?
                    .ok_or_else(|| {
                        Error::InvalidAccount("indefinite keypath arrays are forbidden".to_owned())
                    })?;
                if components % 2 != 0 || components > 16 {
                    return Err(Error::InvalidAccount(
                        "invalid or oversized keypath".to_owned(),
                    ));
                }
                let mut decoded_path = Vec::with_capacity((components / 2) as usize);
                for _ in 0..components / 2 {
                    let index = decoder
                        .u32()
                        .map_err(|e| Error::InvalidAccount(e.to_string()))?;
                    let hardened = decoder
                        .bool()
                        .map_err(|e| Error::InvalidAccount(e.to_string()))?;
                    let child = if hardened {
                        ChildNumber::from_hardened_idx(index)
                    } else {
                        ChildNumber::from_normal_idx(index)
                    }
                    .map_err(|e| Error::InvalidAccount(e.to_string()))?;
                    decoded_path.push(child);
                }
                path = Some(decoded_path);
            }
            2 => {
                source = Some(
                    decoder
                        .u32()
                        .map_err(|e| Error::InvalidAccount(e.to_string()))?,
                )
            }
            3 => {
                decoder
                    .u8()
                    .map_err(|e| Error::InvalidAccount(e.to_string()))?;
            }
            _ => return Err(Error::InvalidAccount("unknown keypath entry".to_owned())),
        }
    }
    Ok((
        path.ok_or_else(|| Error::InvalidAccount("keypath components are required".to_owned()))?,
        source,
    ))
}

#[derive(Debug, Clone)]
pub enum AirgappedResponse {
    VerifiedAddress(VerifiedAddress),
    SignedPsbt(Psbt),
}

/// Validate an air-gapped signing response and merge only signature fields
/// into Liana's canonical PSBT.
///
/// This intentionally does not replace global/input/output maps, so existing
/// unknown and proprietary fields cannot disappear or be rewritten by the
/// returned file/QR. Signers may normalize or omit metadata in their returned
/// PSBT; those differences are ignored because every accepted signature is
/// checked against the original transaction before it is merged.
pub fn validate_and_merge_psbt(original: &Psbt, returned: &Psbt) -> Result<Psbt, Error> {
    if original.unsigned_tx != returned.unsigned_tx
        || original.inputs.len() != returned.inputs.len()
        || original.outputs.len() != returned.outputs.len()
    {
        return Err(Error::InvalidPsbt(
            "returned transaction does not match".to_owned(),
        ));
    }

    let mut merged = original.clone();
    let mut added = 0usize;
    for (index, (canonical, signed)) in original
        .inputs
        .iter()
        .zip(returned.inputs.iter())
        .enumerate()
    {
        for (public_key, signature) in &canonical.partial_sigs {
            if signed.partial_sigs.get(public_key) != Some(signature) {
                return Err(Error::InvalidPsbt(format!(
                    "existing signature disappeared or changed on input {index}"
                )));
            }
        }
        for (public_key, signature) in &signed.partial_sigs {
            if let Some(existing) = canonical.partial_sigs.get(public_key) {
                if existing != signature {
                    return Err(Error::InvalidPsbt(format!(
                        "existing signature changed on input {index}"
                    )));
                }
                continue;
            }
            if !canonical.bip32_derivation.contains_key(&public_key.inner) {
                return Err(Error::InvalidPsbt(format!(
                    "signature uses an unexpected public key on input {index}"
                )));
            }
            verify_ecdsa_signature(original, index, public_key, signature)?;
            merged.inputs[index]
                .partial_sigs
                .insert(*public_key, *signature);
            added += 1;
        }

        for (key, signature) in &canonical.tap_script_sigs {
            if signed.tap_script_sigs.get(key) != Some(signature) {
                return Err(Error::InvalidPsbt(format!(
                    "existing Taproot signature disappeared or changed on input {index}"
                )));
            }
        }
        for (key, signature) in &signed.tap_script_sigs {
            if let Some(existing) = canonical.tap_script_sigs.get(key) {
                if existing != signature {
                    return Err(Error::InvalidPsbt(format!(
                        "existing Taproot signature changed on input {index}"
                    )));
                }
                continue;
            }
            let Some((leaf_hashes, _)) = canonical.tap_key_origins.get(&key.0) else {
                return Err(Error::InvalidPsbt(format!(
                    "Taproot signature uses an unexpected public key on input {index}"
                )));
            };
            if !leaf_hashes.contains(&key.1) {
                return Err(Error::InvalidPsbt(format!(
                    "Taproot signature uses an unexpected script leaf on input {index}"
                )));
            }
            verify_taproot_signature(original, index, key.0, key.1, signature)?;
            merged.inputs[index]
                .tap_script_sigs
                .insert(*key, *signature);
            added += 1;
        }

        match (canonical.tap_key_sig, signed.tap_key_sig) {
            (Some(existing), Some(returned)) if existing == returned => {}
            (Some(_), _) => {
                return Err(Error::InvalidPsbt(format!(
                    "existing Taproot key-path signature disappeared or changed on input {index}"
                )));
            }
            (None, Some(signature)) => {
                let internal_key = canonical.tap_internal_key.ok_or_else(|| {
                    Error::InvalidPsbt(format!(
                        "Taproot key-path signature has no expected internal key on input {index}"
                    ))
                })?;
                if !canonical.tap_key_origins.contains_key(&internal_key) {
                    return Err(Error::InvalidPsbt(format!(
                        "Taproot key-path signature uses an unexpected key on input {index}"
                    )));
                }
                verify_taproot_key_signature(original, index, &signature)?;
                merged.inputs[index].tap_key_sig = Some(signature);
                added += 1;
            }
            (None, None) => {}
        }
    }

    if added == 0 {
        return Err(Error::InvalidPsbt("signature was not added".to_owned()));
    }
    Ok(merged)
}

fn verify_ecdsa_signature(
    psbt: &Psbt,
    input_index: usize,
    public_key: &liana::miniscript::bitcoin::PublicKey,
    signature: &ecdsa::Signature,
) -> Result<(), Error> {
    let mut verification_psbt = psbt.clone();
    if let Some(declared) = verification_psbt.inputs[input_index].sighash_type {
        let declared = declared.ecdsa_hash_ty().map_err(|_| {
            Error::InvalidPsbt(format!("invalid sighash type on input {input_index}"))
        })?;
        if declared != signature.sighash_type {
            return Err(Error::InvalidPsbt(format!(
                "signature sighash type does not match input {input_index}"
            )));
        }
    } else {
        verification_psbt.inputs[input_index].sighash_type = Some(signature.sighash_type.into());
    }
    let mut cache = SighashCache::new(&verification_psbt.unsigned_tx);
    let message = verification_psbt
        .sighash_msg(input_index, &mut cache, None)
        .map_err(|error| {
            Error::InvalidPsbt(format!(
                "could not calculate signature hash for input {input_index}: {error}"
            ))
        })?
        .to_secp_msg();
    Secp256k1::verification_only()
        .verify_ecdsa(&message, &signature.signature, &public_key.inner)
        .map_err(|_| Error::InvalidPsbt(format!("invalid signature on input {input_index}")))
}

fn verify_taproot_signature(
    psbt: &Psbt,
    input_index: usize,
    public_key: secp256k1::XOnlyPublicKey,
    leaf_hash: TapLeafHash,
    signature: &taproot::Signature,
) -> Result<(), Error> {
    let mut verification_psbt = psbt.clone();
    if let Some(declared) = verification_psbt.inputs[input_index].sighash_type {
        let declared = declared.taproot_hash_ty().map_err(|_| {
            Error::InvalidPsbt(format!("invalid sighash type on input {input_index}"))
        })?;
        if declared != signature.sighash_type {
            return Err(Error::InvalidPsbt(format!(
                "signature sighash type does not match input {input_index}"
            )));
        }
    } else {
        verification_psbt.inputs[input_index].sighash_type = Some(signature.sighash_type.into());
    }
    let mut cache = SighashCache::new(&verification_psbt.unsigned_tx);
    let message = verification_psbt
        .sighash_msg(input_index, &mut cache, Some(leaf_hash))
        .map_err(|error| {
            Error::InvalidPsbt(format!(
                "could not calculate Taproot signature hash for input {input_index}: {error}"
            ))
        })?
        .to_secp_msg();
    Secp256k1::verification_only()
        .verify_schnorr(&signature.signature, &message, &public_key)
        .map_err(|_| {
            Error::InvalidPsbt(format!("invalid Taproot signature on input {input_index}"))
        })
}

fn verify_taproot_key_signature(
    psbt: &Psbt,
    input_index: usize,
    signature: &taproot::Signature,
) -> Result<(), Error> {
    let mut verification_psbt = psbt.clone();
    if let Some(declared) = verification_psbt.inputs[input_index].sighash_type {
        let declared = declared.taproot_hash_ty().map_err(|_| {
            Error::InvalidPsbt(format!("invalid sighash type on input {input_index}"))
        })?;
        if declared != signature.sighash_type {
            return Err(Error::InvalidPsbt(format!(
                "signature sighash type does not match input {input_index}"
            )));
        }
    } else {
        verification_psbt.inputs[input_index].sighash_type = Some(signature.sighash_type.into());
    }
    let spent = verification_psbt.spend_utxo(input_index).map_err(|error| {
        Error::InvalidPsbt(format!(
            "missing Taproot input value at input {input_index}: {error}"
        ))
    })?;
    let script = spent.script_pubkey.as_bytes();
    if !spent.script_pubkey.is_p2tr() || script.len() != 34 {
        return Err(Error::InvalidPsbt(format!(
            "Taproot key-path signature is not for a Taproot output on input {input_index}"
        )));
    }
    let output_key = secp256k1::XOnlyPublicKey::from_slice(&script[2..]).map_err(|_| {
        Error::InvalidPsbt(format!("invalid Taproot output key on input {input_index}"))
    })?;
    let mut cache = SighashCache::new(&verification_psbt.unsigned_tx);
    let message = verification_psbt
        .sighash_msg(input_index, &mut cache, None)
        .map_err(|error| {
            Error::InvalidPsbt(format!(
                "could not calculate Taproot key-path signature hash for input {input_index}: {error}"
            ))
        })?
        .to_secp_msg();
    Secp256k1::verification_only()
        .verify_schnorr(&signature.signature, &message, &output_key)
        .map_err(|_| {
            Error::InvalidPsbt(format!(
                "invalid Taproot key-path signature on input {input_index}"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use minicbor::data::Tag;

    const TESTNET_ACCOUNT: &str = "[9f141cf0/48'/1'/0'/2']tpubDFnReAwXvYd6RA46X55HuFpmvZsLanDrwHAUsdYEGEpNGTRnCdbDRXJGLTwDeqKURCPZUDgdkuuu9dYkuBNQHmSNBUu7V2CdLKwpJjx2JuC";

    #[test]
    fn micro_sd_account_is_network_and_path_checked() {
        let parsed =
            PassportAccount::from_descriptor_key(TESTNET_ACCOUNT, Network::Testnet4).unwrap();
        assert_eq!(parsed.fingerprint.to_string(), "9f141cf0");
        assert_eq!(parsed.network, PolicyNetwork::TBTC);
        assert_eq!(
            PassportAccount::from_descriptor_key(TESTNET_ACCOUNT, Network::Bitcoin),
            Err(Error::InvalidNetwork)
        );
    }

    #[test]
    fn crypto_account_decodes_bip48_native_segwit_cosigner() {
        let fallback =
            PassportAccount::from_descriptor_key(TESTNET_ACCOUNT, Network::Testnet4).unwrap();
        let xpub = match &fallback.account {
            DescriptorPublicKey::XPub(key) => key,
            _ => unreachable!(),
        };
        let mut encoder = minicbor::Encoder::new(Vec::new());
        encoder
            .map(2)
            .unwrap()
            .u8(1)
            .unwrap()
            .u32(0x9f141cf0)
            .unwrap()
            .u8(2)
            .unwrap()
            .array(1)
            .unwrap()
            .tag(Tag::new(308))
            .unwrap()
            .tag(Tag::new(401))
            .unwrap()
            .tag(Tag::new(410))
            .unwrap()
            .tag(Tag::new(303))
            .unwrap()
            .map(5)
            .unwrap()
            .u8(3)
            .unwrap()
            .bytes(&xpub.xkey.public_key.serialize())
            .unwrap()
            .u8(4)
            .unwrap()
            .bytes(&xpub.xkey.chain_code.to_bytes())
            .unwrap()
            .u8(5)
            .unwrap()
            .tag(Tag::new(40305))
            .unwrap()
            .map(1)
            .unwrap()
            .u8(2)
            .unwrap()
            .u8(1)
            .unwrap()
            .u8(6)
            .unwrap()
            .tag(Tag::new(40304))
            .unwrap()
            .map(2)
            .unwrap()
            .u8(1)
            .unwrap()
            .array(8)
            .unwrap()
            .u8(48)
            .unwrap()
            .bool(true)
            .unwrap()
            .u8(1)
            .unwrap()
            .bool(true)
            .unwrap()
            .u8(0)
            .unwrap()
            .bool(true)
            .unwrap()
            .u8(2)
            .unwrap()
            .bool(true)
            .unwrap()
            .u8(2)
            .unwrap()
            .u32(0x9f141cf0)
            .unwrap()
            .u8(8)
            .unwrap()
            .u32(u32::from_be_bytes(xpub.xkey.parent_fingerprint.to_bytes()))
            .unwrap();
        let cbor = encoder.into_writer();
        let mut direct = minicbor::Decoder::new(&cbor);
        direct.map().unwrap();
        direct.u8().unwrap();
        direct.u32().unwrap();
        direct.u8().unwrap();
        direct.array().unwrap();
        decode_bip48_cosigner(&mut direct, 0x9f141cf0, Network::Testnet4).unwrap();
        let parsed = PassportAccount::from_crypto_account_cbor(&cbor, Network::Testnet4).unwrap();
        assert_eq!(parsed, fallback);

        // Reorder the two top-level map entries. CBOR maps are unordered, so
        // the output array is valid even when it precedes the fingerprint.
        assert_eq!(
            &cbor[..8],
            &[0xa2, 0x01, 0x1a, 0x9f, 0x14, 0x1c, 0xf0, 0x02]
        );
        let mut reordered = vec![0xa2, 0x02];
        reordered.extend_from_slice(&cbor[8..]);
        reordered.extend_from_slice(&cbor[1..7]);
        assert_eq!(
            PassportAccount::from_crypto_account_cbor(&reordered, Network::Testnet4).unwrap(),
            fallback
        );
    }

    #[test]
    fn crypto_account_rejects_duplicate_map_entries() {
        let mut encoder = minicbor::Encoder::new(Vec::new());
        encoder
            .map(2)
            .unwrap()
            .u8(1)
            .unwrap()
            .u32(0x9f141cf0)
            .unwrap()
            .u8(1)
            .unwrap()
            .u32(0x9f141cf0)
            .unwrap();
        assert!(matches!(
            PassportAccount::from_crypto_account_cbor(
                &encoder.into_writer(),
                Network::Testnet4
            ),
            Err(Error::InvalidAccount(error)) if error.contains("duplicate")
        ));
    }

    #[test]
    fn active_operation_enforces_response_type() {
        let payload = UrPayload::bytes(b"{}".to_vec());
        assert!(matches!(
            ExpectedResponse::SignedPsbt.decode(payload),
            Err(Error::WrongUrType { .. })
        ));
    }
}
