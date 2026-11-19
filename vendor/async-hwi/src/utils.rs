use std::{cmp::Ordering, collections::BTreeMap, str::FromStr};

use bitcoin::{
    bip32::{ChildNumber, DerivationPath, KeySource},
    psbt::Psbt,
    secp256k1::PublicKey,
};

use crate::{Error, HWI};

pub struct Bip32DerivationFilter<'a> {
    psbt: &'a mut Psbt,
    ignored_bip32_derivations: Vec<BTreeMap<PublicKey, KeySource>>,
}

impl<'a> Bip32DerivationFilter<'a> {
    pub fn new(psbt: &'a mut Psbt) -> Self {
        Self {
            ignored_bip32_derivations: psbt.inputs.iter().map(|_| BTreeMap::new()).collect(),
            psbt,
        }
    }
    // Bitbox and Coldcard sign with the first bip32_derivation that matches its fingerprint.
    // In order to to multiple round of signing the bip32_derivation of keys that
    // signed the psbt must be removed then appended once the tx is signed.
    pub fn ignore_signed_key_derivation(mut self) -> Self {
        for (i, input) in self.psbt.inputs.iter_mut().enumerate() {
            for key in input.partial_sigs.keys() {
                if let Some(derivation) = input.bip32_derivation.remove(&key.inner) {
                    self.ignored_bip32_derivations[i].insert(key.inner, derivation);
                }
            }
        }
        self
    }

    // Input may have multiple derivation with the same fingerprint and different derivation
    // for example:
    // Input A: fg/0/0, fg/1/0 and Input B fg/0/1, fg/1/1.
    // We want for this first round of signature to have A: fg/0/0 and fg/0/1
    pub fn ignore_same_fg_bip32_derivations(mut self) -> Self {
        let mut priority_order = Vec::<KeySource>::new();
        for input in &self.psbt.inputs {
            for source in input.bip32_derivation.values() {
                priority_order.push(source.clone());
            }
        }
        priority_order.sort_by(|(fg1, path1), (fg2, path2)| match fg1.cmp(fg2) {
            Ordering::Less => Ordering::Less,
            Ordering::Greater => Ordering::Greater,
            Ordering::Equal => path2.cmp(path1),
        });

        for (i, input) in self.psbt.inputs.iter_mut().enumerate() {
            let mut to_remove = Vec::<PublicKey>::new();
            for (key1, source1) in &input.bip32_derivation {
                for (key2, source2) in &input.bip32_derivation {
                    if source1.0 == source2.0 && source1.1 != source2.1 {
                        if priority_order.iter().position(|s| s == source1)
                            < priority_order.iter().position(|s| s == source2)
                        {
                            to_remove.push(*key2);
                        } else {
                            to_remove.push(*key1);
                        }
                    }
                }
            }
            for key in to_remove {
                if let Some(derivation) = input.bip32_derivation.remove(&key) {
                    self.ignored_bip32_derivations[i].insert(key, derivation);
                }
            }
        }

        self
    }

    /// Signs the psbt with the HWI interface and puts back the ignored bip32 derivations
    pub async fn sign_psbt<T: HWI>(mut self, device: &T) -> Result<(), Error> {
        device.sign_tx(self.psbt).await?;

        for (i, input) in self.psbt.inputs.iter_mut().enumerate() {
            input
                .bip32_derivation
                .append(&mut self.ignored_bip32_derivations[i]);
        }

        Ok(())
    }
}

pub fn merge_signatures(psbt: &mut Psbt, signed_psbt: &Psbt) {
    for i in 0..signed_psbt.inputs.len() {
        let psbtin = match psbt.inputs.get_mut(i) {
            Some(psbtin) => psbtin,
            None => continue,
        };
        let signed_psbtin = match signed_psbt.inputs.get(i) {
            Some(signed_psbtin) => signed_psbtin,
            None => continue,
        };
        psbtin
            .partial_sigs
            .extend(&mut signed_psbtin.partial_sigs.iter());
        psbtin
            .tap_script_sigs
            .extend(&mut signed_psbtin.tap_script_sigs.iter());
        if let Some(sig) = signed_psbtin.tap_key_sig {
            psbtin.tap_key_sig = Some(sig);
        }
    }
}

pub fn bip86_path_child_numbers(path: DerivationPath) -> Result<Vec<ChildNumber>, Error> {
    let children: Vec<ChildNumber> = path.into();
    if children.len() != 5
        || children[0] != ChildNumber::from_hardened_idx(86).unwrap()
        || children[1].is_normal()
        || children[2].is_normal()
        || children[3].is_hardened()
        || children[4].is_hardened()
    {
        Err(Error::InvalidParameter(
            "derivation_path",
            "path is not bip86 compatible".to_string(),
        ))
    } else {
        Ok(children)
    }
}

#[cfg(feature = "regex")]
pub fn extract_keys_and_template<T: FromStr>(policy: &str) -> Result<(String, Vec<T>), Error> {
    let re = regex::Regex::new(r"((\[.+?\])?[xyYzZtuUvV]pub[1-9A-HJ-NP-Za-km-z]{79,108})").unwrap();
    let mut descriptor_template = policy.to_string();
    let mut pubkeys_str: Vec<&str> = Vec::new();
    for capture in re.find_iter(policy) {
        if !pubkeys_str.contains(&capture.as_str()) {
            pubkeys_str.push(capture.as_str());
        }
    }

    let mut pubkeys: Vec<T> = Vec::new();
    for (i, key_str) in pubkeys_str.iter().enumerate() {
        descriptor_template = descriptor_template.replace(key_str, &format!("@{i}"));
        let pubkey = T::from_str(key_str).map_err(|_| Error::UnsupportedInput)?;
        pubkeys.push(pubkey);
    }

    // Do not include the hash in the descriptor template.
    if let Some((descriptor_template, _hash)) = descriptor_template.rsplit_once('#') {
        Ok((descriptor_template.to_string(), pubkeys))
    } else {
        Ok((descriptor_template, pubkeys))
    }
}

/// The script wrapper of a plain multisig descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultisigWrapper {
    /// `wsh(multi(..))`
    Wsh,
    /// `sh(wsh(multi(..)))`
    ShWsh,
    /// `sh(multi(..))`
    Sh,
}

/// A plain `multi` / `sortedmulti` policy whose keys all derive on the
/// standard receive/change branches (`/<0;1>/*` or `/**`): the shape a
/// device's multisig registry (Jade's, for one) holds, as opposed to its
/// descriptor or miniscript registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultisigPolicy {
    pub wrapper: MultisigWrapper,
    pub sorted: bool,
    pub threshold: u32,
    /// Keys in descriptor order, with their origin.
    pub keys: Vec<(Option<bitcoin::bip32::Fingerprint>, bitcoin::bip32::Xpub)>,
}

#[cfg(feature = "regex")]
impl MultisigPolicy {
    /// `None` for anything that is not such a multisig.
    pub fn parse(policy: &str) -> Option<Self> {
        let (template, keys) = extract_keys_and_template::<String>(policy).ok()?;
        let (wrapper, inner) = if let Some(inner) = template
            .strip_prefix("sh(wsh(")
            .and_then(|t| t.strip_suffix("))"))
        {
            (MultisigWrapper::ShWsh, inner)
        } else if let Some(inner) = template
            .strip_prefix("wsh(")
            .and_then(|t| t.strip_suffix(')'))
        {
            (MultisigWrapper::Wsh, inner)
        } else {
            (
                MultisigWrapper::Sh,
                template.strip_prefix("sh(")?.strip_suffix(')')?,
            )
        };
        let (sorted, args) = if let Some(args) = inner.strip_prefix("sortedmulti(") {
            (true, args)
        } else {
            (false, inner.strip_prefix("multi(")?)
        };
        let mut args = args.strip_suffix(')')?.split(',');
        let threshold: u32 = args.next()?.parse().ok()?;
        let placeholders: Vec<&str> = args.collect();
        if placeholders.len() != keys.len() || keys.is_empty() {
            return None;
        }
        for (i, placeholder) in placeholders.iter().enumerate() {
            let rest = placeholder.strip_prefix(&format!("@{i}"))?;
            if rest != "/**" && rest != "/<0;1>/*" {
                return None;
            }
        }
        let keys = keys
            .iter()
            .map(|key| {
                let (fingerprint, xpub) = match key.strip_prefix('[') {
                    Some(rest) => {
                        let (origin, xpub) = rest.split_once(']')?;
                        let fingerprint = origin.split('/').next()?;
                        (
                            Some(bitcoin::bip32::Fingerprint::from_str(fingerprint).ok()?),
                            xpub,
                        )
                    }
                    None => (None, key.as_str()),
                };
                Some((fingerprint, bitcoin::bip32::Xpub::from_str(xpub).ok()?))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            wrapper,
            sorted,
            threshold,
            keys,
        })
    }

    /// Whether `xpubs` are this policy's keys, whatever network prefix they
    /// were exported with: in any order for `sortedmulti` (BIP67 sorts them
    /// in the script), in this order for `multi`, where order changes the
    /// script.
    pub fn has_keys(&self, xpubs: &[bitcoin::bip32::Xpub]) -> bool {
        let id = |x: &bitcoin::bip32::Xpub| (x.public_key, x.chain_code);
        let mut ours: Vec<_> = self.keys.iter().map(|(_, x)| id(x)).collect();
        let mut theirs: Vec<_> = xpubs.iter().map(id).collect();
        if self.sorted {
            ours.sort();
            theirs.sort();
        }
        ours == theirs
    }
}

/// A descriptor as registered, compared loosely: hardened markers `'` and
/// `h` are the same step, and a trailing checksum is ignored.
pub fn same_descriptor(a: &str, b: &str) -> bool {
    let normalize = |s: &str| {
        let s = s.rsplit_once('#').map(|(d, _)| d).unwrap_or(s);
        s.replace('\'', "h")
    };
    normalize(a) == normalize(b)
}

#[cfg(all(test, feature = "regex"))]
mod multisig_policy_tests {
    use super::*;

    const KEYS: [&str; 3] = [
        "[b0822927/48'/1'/0'/2']tpubDEvZxV86Br8Knbm9tWcr5Hvmg5cYTYsg92vinqH6Bie6U8ix8CsoN9W11NQygdqVwmHUJpsHXxNsi5gXn36g4xNfLWkMqPuFhRZAmMQ7jjQ",
        "[7fc39c07/48'/1'/0'/2']tpubDEvjgXtrUuH3Qtkapny9aE8gN847xiXsf9MDM5XueGf9nrvStqAuBSva3ajGyTvtp8Ti55FvVXsgYSXuS1tQkBeopFuodx2hRUDmQbvKxbZ",
        "[1a1ffd98/48'/1'/0'/2']tpubDFZqzTvGijYb13BC73CkS1er8DrP5YdzMhziN3kWCKUFaW51Yj6ggvf99YpdrkTJy4RT85mxQMHXDiFAKRxzf6BykQgT4pRRBNPshSJJcKo",
    ];

    fn desc(wrap: &str, branch: &str) -> String {
        let keys: Vec<String> = KEYS.iter().map(|k| format!("{k}{branch}")).collect();
        wrap.replace("KEYS", &keys.join(","))
    }

    #[test]
    fn parses_plain_multisig_shapes() {
        for (wrap, wrapper, sorted) in [
            ("wsh(sortedmulti(2,KEYS))", MultisigWrapper::Wsh, true),
            ("sh(wsh(sortedmulti(2,KEYS)))", MultisigWrapper::ShWsh, true),
            ("sh(multi(2,KEYS))", MultisigWrapper::Sh, false),
            ("wsh(multi(2,KEYS))", MultisigWrapper::Wsh, false),
        ] {
            for branch in ["/<0;1>/*", "/**"] {
                let policy = MultisigPolicy::parse(&desc(wrap, branch)).unwrap();
                assert_eq!(policy.wrapper, wrapper);
                assert_eq!(policy.sorted, sorted);
                assert_eq!(policy.threshold, 2);
                assert_eq!(policy.keys.len(), 3);
                assert_eq!(
                    policy.keys[0].0,
                    Some(bitcoin::bip32::Fingerprint::from_str("b0822927").unwrap())
                );
            }
        }
    }

    #[test]
    fn refuses_anything_a_multisig_registry_cannot_hold() {
        // Other branches, miniscript, singlesig.
        assert!(MultisigPolicy::parse(&desc("wsh(sortedmulti(2,KEYS))", "/<2;3>/*")).is_none());
        assert!(MultisigPolicy::parse(&format!(
            "wsh(or_d(pk({}/<0;1>/*),pk({}/<0;1>/*)))",
            KEYS[0], KEYS[1]
        ))
        .is_none());
        assert!(MultisigPolicy::parse(&format!("wpkh({}/<0;1>/*)", KEYS[0])).is_none());
    }

    #[test]
    fn sortedmulti_keys_compare_in_any_order() {
        let policy = MultisigPolicy::parse(&desc("wsh(sortedmulti(2,KEYS))", "/**")).unwrap();
        let mut xpubs: Vec<_> = policy.keys.iter().map(|(_, x)| *x).collect();
        xpubs.reverse();
        assert!(policy.has_keys(&xpubs));
        assert!(!policy.has_keys(&xpubs[..2]));
    }

    #[test]
    fn multi_keys_compare_in_order() {
        let policy = MultisigPolicy::parse(&desc("wsh(multi(2,KEYS))", "/**")).unwrap();
        let mut xpubs: Vec<_> = policy.keys.iter().map(|(_, x)| *x).collect();
        assert!(policy.has_keys(&xpubs));
        xpubs.reverse();
        assert!(!policy.has_keys(&xpubs));
    }

    #[test]
    fn descriptors_compare_across_hardened_markers_and_checksums() {
        assert!(same_descriptor(
            "wsh(pk([aa/48'/0']x))#abcd",
            "wsh(pk([aa/48h/0h]x))"
        ));
        assert!(!same_descriptor(
            "wsh(pk([aa/48'/0']x))",
            "wsh(pk([aa/48'/1']x))"
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_sort_bip32_derivations() {
        let mut psbt = Psbt::from_str("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").unwrap();
        assert_eq!(psbt.inputs[0].bip32_derivation.len(), 6);
        assert_eq!(psbt.inputs[1].bip32_derivation.len(), 6);
        let filter = Bip32DerivationFilter::new(&mut psbt).ignore_same_fg_bip32_derivations();
        assert_eq!(filter.ignored_bip32_derivations[0].len(), 4);
        assert_eq!(filter.ignored_bip32_derivations[1].len(), 4);
        assert_eq!(psbt.inputs[0].bip32_derivation.len(), 2);
        assert_eq!(psbt.inputs[1].bip32_derivation.len(), 2);
    }

    #[test]
    fn test_extract_keys_and_template() {
        let res = extract_keys_and_template::<String>("wsh(or_d(pk([f5acc2fd/49'/1'/0']tpubDCbK3Ysvk8HjcF6mPyrgMu3KgLiaaP19RjKpNezd8GrbAbNg6v5BtWLaCt8FNm6QkLseopKLf5MNYQFtochDTKHdfgG6iqJ8cqnLNAwtXuP/**),and_v(v:pkh(tpubDDtb2WPYwEWw2WWDV7reLV348iJHw2HmhzvPysKKrJw3hYmvrd4jasyoioVPdKGQqjyaBMEvTn1HvHWDSVqQ6amyyxRZ5YjpPBBGjJ8yu8S/**),older(100))))").unwrap();
        assert_eq!(res.0, "wsh(or_d(pk(@0/**),and_v(v:pkh(@1/**),older(100))))");
        assert_eq!(res.1.len(), 2);
        assert_eq!(res.1[0], "[f5acc2fd/49'/1'/0']tpubDCbK3Ysvk8HjcF6mPyrgMu3KgLiaaP19RjKpNezd8GrbAbNg6v5BtWLaCt8FNm6QkLseopKLf5MNYQFtochDTKHdfgG6iqJ8cqnLNAwtXuP".to_string());
        assert_eq!(res.1[1], "tpubDDtb2WPYwEWw2WWDV7reLV348iJHw2HmhzvPysKKrJw3hYmvrd4jasyoioVPdKGQqjyaBMEvTn1HvHWDSVqQ6amyyxRZ5YjpPBBGjJ8yu8S".to_string());

        let res = extract_keys_and_template::<String>("wsh(or_d(multi(2,[b0822927/48'/1'/0'/2']tpubDEvZxV86Br8Knbm9tWcr5Hvmg5cYTYsg92vinqH6Bie6U8ix8CsoN9W11NQygdqVwmHUJpsHXxNsi5gXn36g4xNfLWkMqPuFhRZAmMQ7jjQ/<0;1>/*,[7fc39c07/48'/1'/0'/2']tpubDEvjgXtrUuH3Qtkapny9aE8gN847xiXsf9MDM5XueGf9nrvStqAuBSva3ajGyTvtp8Ti55FvVXsgYSXuS1tQkBeopFuodx2hRUDmQbvKxbZ/<0;1>/*),and_v(v:thresh(2,pkh([b0822927/48'/1'/0'/2']tpubDEvZxV86Br8Knbm9tWcr5Hvmg5cYTYsg92vinqH6Bie6U8ix8CsoN9W11NQygdqVwmHUJpsHXxNsi5gXn36g4xNfLWkMqPuFhRZAmMQ7jjQ/<2;3>/*),a:pkh([7fc39c07/48'/1'/0'/2']tpubDEvjgXtrUuH3Qtkapny9aE8gN847xiXsf9MDM5XueGf9nrvStqAuBSva3ajGyTvtp8Ti55FvVXsgYSXuS1tQkBeopFuodx2hRUDmQbvKxbZ/<2;3>/*),a:pkh([1a1ffd98/48'/1'/0'/2']tpubDFZqzTvGijYb13BC73CkS1er8DrP5YdzMhziN3kWCKUFaW51Yj6ggvf99YpdrkTJy4RT85mxQMHXDiFAKRxzf6BykQgT4pRRBNPshSJJcKo/<0;1>/*)),older(300))))#wp0w3hlw").unwrap();
        assert_eq!(res.0, "wsh(or_d(multi(2,@0/<0;1>/*,@1/<0;1>/*),and_v(v:thresh(2,pkh(@0/<2;3>/*),a:pkh(@1/<2;3>/*),a:pkh(@2/<0;1>/*)),older(300))))");
        assert_eq!(res.1.len(), 3);
        assert_eq!(res.1[0], "[b0822927/48'/1'/0'/2']tpubDEvZxV86Br8Knbm9tWcr5Hvmg5cYTYsg92vinqH6Bie6U8ix8CsoN9W11NQygdqVwmHUJpsHXxNsi5gXn36g4xNfLWkMqPuFhRZAmMQ7jjQ".to_string());
        assert_eq!(res.1[1], "[7fc39c07/48'/1'/0'/2']tpubDEvjgXtrUuH3Qtkapny9aE8gN847xiXsf9MDM5XueGf9nrvStqAuBSva3ajGyTvtp8Ti55FvVXsgYSXuS1tQkBeopFuodx2hRUDmQbvKxbZ".to_string());
        assert_eq!(res.1[2], "[1a1ffd98/48'/1'/0'/2']tpubDFZqzTvGijYb13BC73CkS1er8DrP5YdzMhziN3kWCKUFaW51Yj6ggvf99YpdrkTJy4RT85mxQMHXDiFAKRxzf6BykQgT4pRRBNPshSJJcKo".to_string());
    }
}
