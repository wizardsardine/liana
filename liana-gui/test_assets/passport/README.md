# Passport protocol fixtures

These deterministic, public-only fixtures lock Passport air-gap protocol v1.
They contain no private key material and must remain stable unless the protocol
version changes.

The account decoder intentionally accepts Passport's legacy BCR-2020-015
`crypto-account` profile; newer Blockchain Commons account types are not
silently treated as equivalent. The policy, verification, and identity JSON
fixtures cover Foundation-specific envelopes documented in
`doc/passport-airgap-protocol.md`.

| Fixture | Purpose |
| --- | --- |
| `account-mainnet.txt` | Mainnet BIP48 native-SegWit microSD key export |
| `account-testnet.txt` | Testnet BIP48 native-SegWit microSD key export |
| `policy-registration-mainnet.json` | Single-signer inheritance policy with immediate and timelocked paths |
| `liana-multisig-testnet.descriptor` | Multipath multisig inheritance policy with immediate and timelocked paths |
| `address-request-mainnet.json` | Receive-address verification request |
| `address-response-mainnet.json` | Passport-bound address verification response |
| `unsigned.psbt.base64` | Canonical unsigned PSBT |
| `partially-signed.psbt.base64` | Same transaction with one expected partial signature |
| `ur-single-bytes.txt` | Single-part BC-UR v2 value |
| `ur-multipart-bytes.txt` | One deterministic multipart BC-UR v2 cycle |

The integration tests verify canonical re-encoding, policy identity and
descriptor checksum, network/type/resource rejection, request-response
binding, PSBT immutability, and UR corruption/reordering/duplicate behavior.
Passport Core's host decoder independently accepts the policy-registration
fixture and derives the recorded policy identity.
