# Vault signer derivation and Developer mode

The Cube Key in a new Vault derives from that Cube's master seed using the
descriptor's normal BIP32 paths. Developer mode controls test-network
visibility and never selects another mnemonic or changes itself when a master
is unavailable. Bitcoin, Bitcoin Blake2b and their test networks follow this
same rule. Mainnet/testnet account paths and xpub encodings remain unchanged;
Bitcoin Blake2b still uses its existing unified-signature rules.

An existing Cube on either Bitcoin or Bitcoin Blake2b first uses its
authenticated, Cube-ID- and fingerprint-scoped master cache after the installer
checks the exact chain. A PIN-only session after seed-only restore can read the
same Cube's chain-bound encrypted seed with its current scoped PIN. Revoked
loaders cannot enter the installer. Neither an SDK handle nor a missing, inconsistent or revoked source can
substitute another root. Passkey Cubes require their authenticated master cache
without a PIN; a cache miss never probes seed files even if a stale PIN exists.
The lookup adds no asynchronous signer-cache writes at restore completion.

Vault creation never generates a master seed. On Bitcoin, Bitcoin Blake2b and
their test networks, a Cube and its single master seed are created first, by
the Launcher's Create Cube flow with its own backup step. A Vault is then
created inside that unlocked Cube and derives from its master. The installer
refuses Vault creation without a Cube on every chain, and the Launcher refuses
such a request before closing another open Cube or its session. Claim and
recovery flows keep their explicit source material and existing admission checks.

## Recovery and existing Vaults

A Cube master backup and the Vault descriptor reproduce newly created Cube
Keys. Recovery Kits keep their existing one-master-seed format; this change adds
no seed slot, remote secret payload or key migration. External hardware,
Keychain and separately imported keys retain their own recovery requirements.

Existing descriptors, encrypted Vault seed files and keys are not rewritten.
Historical Developer-mode Vaults that used the Cube master remain recoverable
from it. Historical independently generated Vault keys still require their
original seed backup or encrypted seed file; the Cube master cannot recreate an
unrelated historical seed. Loading continues to match the recorded fingerprint.
The passkey no-PIN classification guard and exact-source BIP85 provenance guard
remain necessary and unchanged.
