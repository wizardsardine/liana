# Passport air-gapped protocol v1

This document defines the wire contract between Liana and Passport. It is
implemented independently of installer, wallet, camera, and signing screens in
`liana-gui/src/airgap`.

The protocol preserves Liana's existing connected hardware-wallet behavior.
Passport is an asynchronous air-gapped signing method and is not represented as
an `async-hwi` USB device.

## Transport matrix

| Operation | QR | microSD |
| --- | --- | --- |
| Passport account import | `ur:crypto-account` | UTF-8 descriptor key |
| Wallet-policy registration | `ur:bytes` containing UTF-8 JSON | UTF-8 JSON |
| Address-verification request/response | `ur:bytes` containing UTF-8 JSON | Not supported |
| PSBT request/response | `ur:crypto-psbt` | binary PSBT |

BC-UR uses bytewords and fountain encoding. Single-part and multipart values
carry the same registry CBOR. `bytes` and `crypto-psbt` wrap their value in a
CBOR byte string. For compatibility with Passport exports, `crypto-account`
uses the legacy BCR-2020-015 account map (superseded by BCR-2023-019) and the
following deliberately narrow profile:

- one top-level master fingerprint;
- a `crypto-output` matching `wsh(cosigner(crypto-hdkey))`;
- public key material and a 32-byte chain code;
- origin `m/48'/coin_type'/account'/2'`;
- matching Bitcoin network in `crypto-coin-info` and the origin coin type;
- no child derivation expression and no private key material.

The Passport account-import microSD fallback is one line:

```text
[fingerprint/48'/coin_type'/account'/2']xpub-or-tpub
```

Liana validates the complete origin and extended public key. Fingerprint-only
matching is not sufficient.

## Wallet-policy registration

The authoritative registration format is the Passport envelope, not
`crypto-output`:

```json
{
  "format": "passport-wallet-policy",
  "version": 1,
  "name": "Wallet name",
  "network": "BTC",
  "template": "wsh(or_d(pk(@0/<0;1>/*),and_v(v:pkh(@1/<0;1>/*),older(52560))))",
  "keys": ["[abcdef01]xpub...", "[abcdef02]xpub..."],
  "policy_id": "64 lowercase hexadecimal characters"
}
```

`network` is `BTC` for mainnet and `TBTC` for non-mainnet Bitcoin networks.
The descriptor template and key expressions are canonical ASCII. Key aliases
and the wallet name do not participate in policy identity. Liana maps a wallet
alias to Passport's printable 20-character display limit (falling back to
`Liana` when necessary); this display-only mapping cannot alter the policy ID.

Policy identity is:

```text
SHA256(
  "Passport Wallet Policy\0" ||
  0x01 ||
  compact_size(network.len) || network ||
  compact_size(template.len) || template ||
  compact_size(keys.len) ||
  for each key: compact_size(key.len) || key
)
```

Liana reconstructs and reparses the full descriptor before export. Its existing
canonical eight-character descriptor checksum is the user-facing policy
checksum. No second descriptor hash is introduced.

## Address verification

Address verification is QR-only. The reference signer does not expose a
file-based request/response workflow for this operation.

Request:

```json
{
  "format": "passport-address-verification",
  "version": 1,
  "network": "TBTC",
  "policy_id": "...",
  "descriptor_checksum": "abcdefgh",
  "branch": 0,
  "index": 7
}
```

Response:

```json
{
  "format": "passport-address-verification-response",
  "version": 1,
  "network": "TBTC",
  "policy_id": "...",
  "descriptor_checksum": "abcdefgh",
  "branch": 0,
  "index": 7,
  "address": "tb1...",
  "fingerprint": "1234abcd"
}
```

The request intentionally does not contain Liana's expected address. Passport
derives from its registered policy. Liana accepts the response only when the
network, policy identity, checksum, branch, index, independently derived
address, and full fingerprint all match the active request.

## PSBT invariant

Both QR directions use `crypto-psbt`; microSD uses binary BIP174 PSBT. Returned
data is never a replacement transaction record. Before signature merge, Liana
must require the same unsigned transaction and input/output counts, retain all
canonical unknown/proprietary fields and existing signatures, and admit only
new signatures for keys expected by the wallet.

## Decoder resource limits

The default limits intentionally match Passport Core's own UR decoder where
possible. They are interoperability limits, not general BC-UR limits:

| Resource | Limit |
| --- | ---: |
| Decoded registry CBOR | 24 KiB |
| Encoded registry CBOR sent to Passport | 24 KiB |
| Declared fountain fragments | 128 |
| QR fragment characters | 1,408 |
| Decoded fragment CBOR | 700 bytes |
| JSON envelope | 4,096 bytes |
| JSON nesting | 16 |
| Descriptor | 4,096 ASCII bytes |
| Policy template | 2,048 ASCII bytes |
| Policy keys | 20 |
| Scan session | 120 seconds |
| Imported binary PSBT file | 8 MiB |

The decoder checks declared message length, padded allocation size, fragment
count, fragment size, and expected UR type before handing data to the fountain
decoder. Duplicate fragments are tolerated. Inconsistent type, message length,
fragment geometry, checksum, or fountain session is rejected. Cancellation
clears decoder state; restart begins a new deadline and session.

Camera implementations must not persist frames, and must release the camera on
success, cancellation, timeout, and error. They are downstream consumers of
this module and may apply smaller limits, never larger ones without a protocol
review.

The QR ceiling is the Passport Core decoder limit, not a PSBT-format limit.
When a PSBT cannot fit, Liana rejects QR presentation before generating an
unscannable sequence and keeps the bounded binary microSD workflow available.

## Versioning

Unknown envelope fields are rejected in v1. A change that adds fields or alters
identity, checksum, network, descriptor, or response-binding semantics requires
a new envelope version. Local explanatory metadata such as signer aliases must
not change policy identity.

## Persisted signer and exchange states

Wallet settings add a backwards-compatible `airgapped_signers` array. Each
record contains only the signer kind, complete master fingerprint, optional
alias, public BIP48 account key, and per-wallet registration state. Existing
settings without this field deserialize to an empty array. No seed, private
extended key, signature, PSBT, or camera frame is persisted there.

Registration moves from `NotRegistered` to `Exported` only after the user
confirms completion on the signer. The exported state stores the active
descriptor checksum. Loading a wallet invalidates a state whose checksum
differs from the canonical descriptor. A QR/file exchange itself is transient:
reopening the operation recreates the same bound request from the persisted
wallet and canonical PSBT, which makes cancellation and an application restart
safe.

Wallets created before this metadata existed remain usable. The registration
picker reconstructs candidate QR signers from eligible public BIP48 account
keys already committed to the descriptor, excluding known hot, USB, and
provider-managed keys. It persists a reconstructed record only after explicit
registration confirmation.

## Camera and packaging

The scanner uses Nokhwa's Media Foundation and V4L2 backends on Windows and
Linux. On macOS it uses Nokhwa's AVFoundation bindings directly so AVFoundation
can negotiate a 720p session without taking the unsupported device-format lock
used by Nokhwa's generic camera wrapper. Quirc performs the fast-path QR decode;
RXing supplies the inverted/low-quality fallback. All RGB frames stay in memory.
A bounded worker owns the native stream and is joined on success, cancellation,
timeout, failure, modal close, or drop. Preview buffers and UR state are then
released; no frame is written to disk. macOS release bundles include
`NSCameraUsageDescription`. Linux release builders need the V4L2/libclang
development inputs required by Nokhwa's native backend.

### Direct dependency rationale

All added direct dependencies use permissive licenses. Exact resolved versions
and the transitive dependency graph remain locked in `Cargo.lock`.

| Dependency | License | Scope | Reason |
| --- | --- | --- | --- |
| [`foundation-ur` 0.4.0](https://github.com/Foundation-Devices/foundation-rs) | MIT | all desktop targets | BC-UR bytewords and fountain encoding/decoding |
| [`minicbor` 0.24.4](https://crates.io/crates/minicbor/0.24.4) | BlueOak-1.0.0 | all desktop targets | bounded registry-CBOR parsing and encoding |
| [`nokhwa` 0.10.11](https://crates.io/crates/nokhwa/0.10.11) | Apache-2.0 | all desktop targets | camera enumeration, permission handling, and native Windows/Linux capture |
| [`quircs` 0.10.3](https://crates.io/crates/quircs/0.10.3) | MIT | all desktop targets | fast in-memory QR detection and decoding |
| [`rxing` 0.9.2](https://crates.io/crates/rxing/0.9.2) | Apache-2.0 | all desktop targets | robust inverted and difficult-image QR fallback |
| [`zeroize` 1.8.1](https://crates.io/crates/zeroize/1.8.1) | Apache-2.0 OR MIT | all desktop targets | overwrite owned animated PSBT QR strings on release |
| [`nokhwa-bindings-macos` 0.2.4](https://crates.io/crates/nokhwa-bindings-macos/0.2.4) | Apache-2.0 | macOS only | negotiated AVFoundation capture |
| [`flume` 0.11.1](https://crates.io/crates/flume/0.11.1) | Apache-2.0 OR MIT | macOS only | AVFoundation callback transport |
| [`objc` 0.2.7](https://crates.io/crates/objc/0.2.7) | MIT | macOS only | two typed AVFoundation session-preset messages |
| [`qrcode` 0.14.1](https://crates.io/crates/qrcode/0.14.1) | MIT OR Apache-2.0 | tests only | deterministic synthetic camera frames |

## Specifications and compatibility status

The wire formats build on the following published specifications:

- [ISO/IEC 18004:2024](https://www.iso.org/standard/83389.html) for QR symbols;
- [RFC 8949](https://www.rfc-editor.org/rfc/rfc8949.html) for CBOR;
- [BIP 174](https://github.com/bitcoin/bips/blob/master/bip-0174.mediawiki)
  and [BIP 371](https://github.com/bitcoin/bips/blob/master/bip-0371.mediawiki)
  for PSBT;
- [BIP 48](https://github.com/bitcoin/bips/blob/master/bip-0048.mediawiki)
  for multisig account derivation and
  [BIP 388](https://github.com/bitcoin/bips/blob/master/bip-0388.mediawiki)
  for wallet-policy terminology;
- Blockchain Commons' [UR v2](https://github.com/BlockchainCommons/Research/blob/master/papers/bcr-2020-005-ur.md),
  [registry types](https://github.com/BlockchainCommons/Research/blob/master/papers/bcr-2020-006-urtypes.md),
  [HD key](https://github.com/BlockchainCommons/Research/blob/master/papers/bcr-2020-007-hdkey.md),
  [Bytewords](https://github.com/BlockchainCommons/Research/blob/master/papers/bcr-2020-012-bytewords.md),
  legacy [crypto-account](https://github.com/BlockchainCommons/Research/blob/master/papers/bcr-2020-015-account.md),
  legacy [crypto-psbt](https://github.com/BlockchainCommons/Research/blob/master/papers/bcr-2021-001-request.md),
  and [multipart UR](https://github.com/BlockchainCommons/Research/blob/master/papers/bcr-2024-001-multipart-ur.md)
  research specifications. Blockchain Commons [explicitly describes
  BCRs](https://github.com/BlockchainCommons/Research) as interoperability
  research rather than formal standards; the two legacy types are retained
  only for compatibility with current Passport firmware.

The `passport-wallet-policy` and `passport-address-verification` JSON envelopes,
including `policy_id`, are Foundation-specific protocols defined completely in
this document and locked by public fixtures. They are not BIPs or Blockchain
Commons registry types.

Camera capture uses the operating-system APIs behind Nokhwa:
[AVFoundation](https://developer.apple.com/documentation/avfoundation/setting-up-a-capture-session)
on macOS, [Media Foundation](https://learn.microsoft.com/en-us/windows/win32/medfound/audio-video-capture-in-media-foundation)
on Windows, and [V4L2](https://docs.kernel.org/userspace-api/media/v4l/v4l2.html)
on Linux.

Automated tests cover encoding, decoding, resource limits, policy identity,
response binding, PSBT invariants, scanner progress, and scanner lifecycle.
The complete physical workflow and built-in camera have been exercised on
Passport Core and macOS. Passport Prime protocol compatibility is implemented
but has not yet been exercised on physical hardware. Windows and Linux use the
same scanner state machine and decoders and are compile/CI targets, but their
native camera backends still require physical runtime testing before this
feature can be described as validated on those platforms.

## Threat model

Camera frames, QR strings, and imported files are untrusted. The transport
layer checks type, size, fountain geometry, JSON shape/depth, canonical policy
identity, network, descriptor checksum, and response binding before use. A
returned PSBT is not trusted as a replacement: Liana verifies every newly added
ECDSA or Taproot signature, rejects unexpected keys or leaves, preserves all
existing signatures and non-signature maps, and merges only verified signature
fields into its canonical PSBT. Passport independently derives addresses from
the registered policy; the coordinator never supplies the address as proof.
