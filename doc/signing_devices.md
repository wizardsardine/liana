# Signing devices

Documentation related to signing devices. It is required signers support Miniscript descriptors.

The connection to the signing devices is implemented in [another
repository](https://github.com/wizardsardine/async-hwi).


## [Specter DIY](https://github.com/cryptoadvance/specter-diy)

Version 1.5.0 and above of the firmware is supported for use in P2WSH descriptors.

Support for use in Taproot descriptors has been implemented but not yet released.

## [Ledger](https://github.com/LedgerHQ/app-bitcoin-new)

The Bitcoin application is supported for use in P2WSH descriptors starting with version 2.1.0. It is
supported for use in Taproot descriptors starting with version 2.2.1.

## [BitBox02](https://github.com/digitalbitbox/bitbox02-firmware)

Version 9.15.0 of the firmware is supported for use in P2WSH descriptors.

Support for use in Taproot descriptors is not yet available in the firmware.

## [Coldcard](https://github.com/Coldcard/firmware)

Support for use in both P2WSH and Taproot descriptors has only been released in Beta as of this
writing. It is only supported by the [Edge
firmware](https://github.com/Coldcard/firmware?tab=readme-ov-file#long-lived-branches).

As of this writing, Coldcard on Taproot will only be usable for descriptors which only use a single
key as their primary path. This is due to a discrepancy in how Coldcard derives [deterministically
unspendable Taproot internal
keys](https://delvingbitcoin.org/t/unspendable-keys-in-descriptors/304).

## [Jade](https://github.com/Blockstream/Jade)

Version 1.0.30 of the firmware is supported for use in P2WSH descriptors.

Support for use in Taproot descriptors is not yet available in the firmware.

After the setup of the device, the first connection to set the pin will set also the network.
The network cannot be change unless doing a factory reset.

If using "QrCode" mode, the device will refuse other communication channels like USB.

If using "Temporary Signer", the first connection through USB will setup the network, a new session
is required in order to change it. If using the Liana gui installer, is is advised to first choose
the network before connecting the Jade in "Temporary signer" mode.
