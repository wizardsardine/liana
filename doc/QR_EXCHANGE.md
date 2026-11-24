# QR code exchange with a signing device

Liana can work with a signing device it never connects to by exchanging QR codes: Liana shows a
request on screen, the device scans it, and Liana reads the device's answer back through the
camera. No cable is involved. The other way to use such a device is the SD card, by exporting and
importing files (see [SIGNING_DEVICES.md](SIGNING_DEVICES.md)).

The device must support [BIP-377](https://github.com/bitcoin/bips/pull/2353) messages carried in
[BBQr](https://bbqr.org) QR codes.

## What it is used for

- **Importing a key**, when creating a wallet: choose "Import xpub by QR Code" in the key step. The
  device sends the keys of its first ten accounts at once, so you can switch to another account
  afterwards without scanning again.
- **Registering the wallet** on the device, at the end of wallet creation and later from the wallet
  settings (for example after the descriptor changed).
- **Adding a device to an existing wallet**, for a wallet imported, created before, or whose key was
  added another way: in the wallet settings, "Register on device", then "Add a signing device by
  QR code". The device sends the keys at the wallet's derivation paths, and Liana adds it to the
  wallet's signing devices if it holds one of the wallet's keys.
- **Verifying a receive address**, from the receive screen: the device shows the address on its own
  screen, and Liana checks it is the one it expects.
- **Signing a transaction**, from the PSBT screen: pick the device in the list of signers. A PSBT
  that went through several devices before coming back is accepted with all their signatures.

For each of these, Liana shows the request, then "Scan the response" opens the camera. If the
camera first catches an answer the device still shows from an earlier request, Liana says so and
keeps scanning until the new answer comes.

## Display settings

A large request is shown as a sequence of QR codes that loop until the device has read them all. A
grid under the code shows which ones were shown already, and while Liana scans the answer, which
ones it has read. The "QR code settings" section under the code holds:

- **Density**: a denser code needs fewer frames but a sharper camera on the device. Changing it
  restarts the sequence, so the device has to start its scan over.
- **Speed**: how many frames are shown each second.
- **Brightness**: if the device does not lock onto the code, turn the brightness down before
  trying anything else; a very bright screen can make the code unreadable.
- **Camera**: which camera Liana uses to read the answer, when there is more than one.

The settings are kept for every later exchange. Click the code to show it larger, for a device
whose camera needs it; click it again to go back.

## Camera access

On macOS, Liana asks for camera permission the first time an exchange needs it. On Linux, the user
running Liana needs read access to the camera device, which a desktop session normally gives, and
failing that, membership of the `video` group.

## Logs

When an exchange fails, look at `liana-gui.log` in Liana's data directory: a refused or mismatched
answer is logged as a warning with the reason. Starting Liana with `LOG_LEVEL=debug` or
`LOG_LEVEL=trace` logs more of each exchange.

A trace log contains every request and answer in full, so the wallet's descriptor, its public keys
and its transactions. Treat it as wallet data and do not share one unless the recipient is meant
to see the whole wallet.
