# Imported Bitcoin signing with a paired Keychain

A phone can sign an imported Bitcoin wallet only when it already holds the
exact original account key. Desktop LAN signing admits the same five validated
wallet shapes as imported local signing: `pkh`, `wpkh`, `sh(wpkh)`,
`wsh(multi)`, and `wsh(sortedmulti)`. Existing Vault P2WSH and Taproot
pairings keep their original policy path. Bitcoin Blake2b LAN signing remains
unavailable.

The imported wallet's persisted `WalletId` is derived from its Bitcoin chain
and canonical receive/change pair. The LAN transport uses COINCUBE Desktop's existing
validated, canonical `/<0;1>/*` descriptor serializer, including its original
apostrophe spelling for hardened origins and its checksum. Its full SHA-256 binds the
pairing; the first eight hex characters are only the displayed transport ID.
An imported pairing additionally stores the chain-bound wallet ID and exact
account origin. An older pairing without that scope cannot sign this wallet
and requires a new pairing confirmation. Imported signing reloads the pairing
store when a session starts, so a removed, replaced, or newly ambiguous row
cannot keep signing through a cached device handle.
Each new pairing transaction also gives imported scope a fresh durable revision.
The response is checked against that revision before merge, so removing and
re-pairing the same TLS/key identity within one second revokes an old request.

This service slice verifies an imported pairing and its signature response.
The pairing flow that obtains fresh user confirmation and writes that scope is
a separate integration step; this slice alone does not enable an older phone
pairing to sign an imported wallet.

A Keychain response contributes only new `SIGHASH_ALL` partial signatures
for the bound phone account's derived child keys. Desktop authenticates the
original previous transactions, checks the input's branch/index and derived
script, then verifies the combined partial PSBT with the imported-wallet core.
It leaves original metadata and prior signatures intact. Native BDK may also
return final script fields for singlesig; those fields are discarded. A missing,
conflicting, unauthorized, or invalid signature rejects the whole response
without changing the original PSBT. Session encryption, request AAD, timeout,
cancellation, and replay guards retain the existing LAN path.

The offline synthetic vectors were copied byte-for-byte from
[Keychain PR #182](https://github.com/coincubetech/keychain-app/pull/182) at
`67a4ab2b3c7bea1b87b5704f20482a4caecd1b4a`. The original input
`coincube-gui/src/phone_signer/fixtures/imported_signing.json` has SHA-256
`ef14b201a02d3c0f2f21e9a8ef98b88251a478863d9e63982654ec1798585b30`;
the native production-BLoC signed results
`coincube-gui/src/phone_signer/fixtures/imported_signing_signed.json` have
SHA-256
`075e261fa5433603746c7462890c14ea99ac7309da06e288449b46c54e20146b`.
Desktop tests pin those hashes, require COINCUBE Desktop's exact descriptor bytes and receive/change
script parity at indices 0, 1, and 7, and verify all ten signed PSBTs with
Bitcoin 0.32.8. The sortedmulti phone child ranks first at receive 0 and
second at change 0. The core stores sortedmulti keys in one canonical order,
while the two frozen Keychain vectors intentionally use different textual key
orders. Both verify against the same scripts; a new desktop pairing uses its
canonical transport descriptor and pinned full SHA-256
`6c2d38811a23ff93af86b81c84667591e674884d804843ddf35c5b9995cc1219`
for either sortedmulti order, rather than either raw vector's display ID.
The raw native results demonstrate offline script and signature compatibility;
the exact canonical request, pairing commitment, frozen review and native
signing replay is captured by [Keychain PR #183](https://github.com/coincubetech/keychain-app/pull/183)
at `68e6579bdec33a4639543327d8cd8baa12394035`. Its two exported requests
retain COINCUBE Desktop's original descriptor string and share full SHA-256
`6c2d38811a23ff93af86b81c84667591e674884d804843ddf35c5b9995cc1219`.
The request fixture SHA-256 is
`71bf8c42cce0c9725b641da66d321a9ea6894b5a783ea5725c2100bfdd57c23f`;
the actual native signed-result fixture SHA-256 is
`944753d442cad41affbddf424069b08f2a51990ab6148ec645f0d42fa7fb9a31`.
Desktop tests require the published request bytes to equal the current COINCUBE Desktop
exporter and independently verify both returned partial signatures with Bitcoin
0.32.8. No fixture represents a physical phone or network test.
