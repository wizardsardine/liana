# Signing devices

Documentation related to signing devices. It is required signers support Miniscript descriptors.

The connection to the signing devices is implemented in [another
repository](https://github.com/wizardsardine/async-hwi).


## [Specter DIY](https://github.com/cryptoadvance/specter-diy)

Version 1.5.0 and above of the firmware is supported for use in P2WSH descriptors.

For use in Taproot descriptors you should use version 1.9.0 or higher.

## [Ledger](https://github.com/LedgerHQ/app-bitcoin-new)

The Bitcoin application is supported for use in P2WSH descriptors starting with version 2.1.0. It is
supported for use in Taproot descriptors starting with version 2.2.1.

## [BitBox02](https://github.com/digitalbitbox/bitbox02-firmware)

Version 9.15.0 of the firmware is supported for use in P2WSH descriptors.
Version 9.21.0 of the firmware is supported for use in Taproot descriptors.

## [Coldcard](https://github.com/Coldcard/firmware)

Support for use in both P2WSH and Taproot descriptors has only been released in Beta as of this
writing. It is only supported by the [Edge
firmware](https://github.com/Coldcard/firmware?tab=readme-ov-file#long-lived-branches).
For use in Taproot descriptors you should use version 6.3.3 or higher.


## [Jade and Jade Plus](https://github.com/Blockstream/Jade)

Version 1.0.30 of the firmware is supported for use in P2WSH descriptors.

Support for use in Taproot descriptors is not yet available in the firmware.

WARNING: You won't be able to connect your Jade to Liana if you choose "QrCode" mode when setting up
your Jade. This is because in this mode the Jade refuses to communicate through USB.

WARNING: the network cannot be changed after setting up the device without a factory reset. The
network is set at the same time as the PIN.

It is sometimes useful to change the network without a factory reset, such as when testing the
device and/or Liana. In this case the "Temporary signer" mode may be used. The network can be reset
by simply disconnecting and reconnecting it. If using this mode, we advise you to first choose the
network in the Liana installer before setting up the network on your Jade.
