# Air-gapped signers

Liana talks to an air-gapped signer over QR codes only. It shows a request as a QR code, or as a
sequence of them when the request does not fit one, and reads the signer's answer back through the
camera. No file is exchanged and no cable is involved, so the signer never touches this computer.
What Liana writes to its own log file is described under [Logs](#logs).

Liana is always the wallet side of the exchange: it asks, the signer answers.

## The wire format

The messages are [BIP-377](https://github.com/bitcoin/bips/pull/2353), the draft signing-flow protocol for
miniscript wallets, and Liana encodes them with the [`bip377`](https://github.com/pythcoiner/bip377/tree/7727820263d5cc4066f89717db6620bc56ebdb36) crate. The framing is done
by the [`bwk-qr`](https://github.com/bwk-bitcoin/bwk/tree/c698c0314e73f3a5baec0bcdd71fc10e08f17686/qr) crate: a message longer than one QR
code is split into [BBQr](https://bbqr.org) parts, generic-binary file type, hex encoded.

The protocol is transport agnostic and it is not BC-UR. A signer that wants to work with Liana
implements that encoding; `bip377-ll` is `no_std` and dependency-free, so a firmware can
vendor the codec as-is, and it ships a C binding for a signer that is not written in Rust.

## The four exchanges

**Get Keys**, during wallet creation. Liana asks for the public account keys at
`m/48'/coin'/account'/2'` for accounts 0 through 9 in one request. The answer carries the ten keys,
the master fingerprint, the device model, its firmware version, and the capabilities it advertises.

Once the answer is in, the exchange closes and the key step shows the key itself alongside a picker
listing the accounts the signer actually returned. Changing the account swaps the displayed key from
what was already sent, so choosing between accounts never means scanning again, and the key can be
read back before Apply commits it.

**Register Descriptor**, at the end of wallet creation and again from the wallet settings whenever
the descriptor changes. Liana sends the descriptor under the wallet's name as a BIP-388 wallet
policy: each key once, and a template referring to them by position. A Liana descriptor names the
same key in several spending paths, so this takes about a third off a multisig wallet, and it hands
the signer the shape it displays for approval rather than a descriptor it has to take apart. A
signer that does not advertise wallet policies in its capabilities gets the BIP-380 descriptor
instead. Every request describes the wallet the same way, so a proof of registration stays valid
across them.

A signer answers in one of two ways. If it stored the descriptor, it says so and later requests
carry the name alone. If it is stateless, it returns a proof of registration instead, and Liana
resends the descriptor and that proof with every later request. Liana keeps whichever it was told,
and drops it as soon as the descriptor changes: a registration only ever covered the descriptor it
was made against.

**Address Verification**, from the receive screen. Liana sends the wallet name, the path under the
descriptor, and the address it derived itself. The signer derives the address on its own screen and
answers with a BIP-321 payment URI, which must name that very address or Liana rejects the answer.

**Signing**, from a PSBT. Liana sends the wallet, by name alone when the signer stored it, and the
unsigned transaction, and asks for the signatures back rather than the whole transaction, which is
all it needs and keeps the answer small enough to cross in far fewer frames. That is a preference,
not a demand: a signer is free to return the complete PSBT instead, and Liana merges either form to
the same result. Whichever arrives, every signature is verified against the original transaction
before it is kept.

## What Liana checks before trusting an answer

Every response must be a response, echo the request id Liana generated, and be of the type that was
asked. So an answer left on a signer's screen from an earlier exchange cannot be mistaken for this
one. That is the usual way a scan starts, since the signer is still showing its previous answer when
the camera opens, so Liana says as much and keeps scanning rather than treating it as a failure: the
real answer is picked up as soon as the signer produces it.

The one exception to the echo is a signer that could not read the request at all: it answers with
the reserved id `0x0000`, which Liana never picks, and Liana reports that refusal. Any other error
answer must be for the type of request that was asked.

A key export must carry exactly one key per account asked, in order. A registration answer must say
the wallet is registered, under the name Liana sent. An address answer must be a BIP-321 payment
URI naming the very address Liana derived.

A returned PSBT never replaces Liana's own. Liana requires the same unsigned transaction and the
same input and output counts, refuses an answer that dropped or altered a signature that was
already there, and takes only signatures whose key the input actually expects. Each one is verified
against the original transaction before it is merged into Liana's copy, so nothing else in the PSBT
can be rewritten on the way back. An answer that adds no signature at all is refused, and so is one
whose new signatures are not all from the signer the user picked.

## Showing the request

Every QR in the application is drawn the same way: encoded with `bwk-qr` and painted as a raster,
rather than through iced's `QRCode` widget. That keeps the codes independent of which graphics
backend is compiled in.

A request is split into frames drawn at the same QR version, each carrying as many payload bytes
as that version holds, and shown one after the other, looping until the answer is scanned. Below
the code, a grid with one cell per frame highlights the one on screen. A frame not shown yet is
hollow, so the grid fills over the first loop, and starts over when the density or speed changes.

Under the button that starts the scan, a folded section holds three settings, as kern's QR viewer
has them:

- **Density**, one of bwk-qr's five `QrDensity` levels, from Lowest (version 4, 53 bytes a frame)
  to Highest (version 12, 263 bytes a frame). It starts at Medium (version 9, 163 bytes a frame).
  A denser code needs fewer frames but a sharper camera. Changing it splits the same request,
  under the same id, into frames of the new size, so the signer has to start its scan over.
- **Speed**, from 1 to 12 frames a second, in steps of half a frame. It starts at 4.
- **Brightness**, how bright the light modules are drawn, from 20% to 100% of the theme's light
  colour. It starts at 100%, black on pure white, the contrast signer cameras read best. On a very
  bright screen the blown-out pixels can bleed over the dark modules until the code stops
  decoding: if a signer will not lock onto the code, turn it down before trying anything else. The
  dark modules are never touched, so the code cannot be inverted or flattened by mistake.

The settings are saved in `global_settings.json` once a slider is let go, and apply to every
exchange of every wallet after that.

Clicking the code shows it alone, in a square card 80% of the window's shorter side with the code
filling 90% of it, for a signer whose camera needs a larger code. Clicking it again goes back.

## Camera

Liana captures through bwk-camera: Media Foundation on Windows, V4L2 on Linux, and AVFoundation on
macOS. The capture runs on its own thread and keeps only the latest frame; Liana's scan thread
decodes one every 200 ms and only the preview reaches the interface. A frame the decoder reads
nothing in is tried again against a local threshold, which copes with uneven lighting. The stream
is released when the exchange ends, is cancelled, fails, or times out.

While an answer of several frames is read, a grid under the preview shows one cell per frame: read,
the one just read, or still missing. When a scan stalls, the user sees which frames it still waits
for rather than a count alone.

The camera is streamed at its smallest uncompressed mode whose shorter side reaches 1080 pixels, or
its largest one below that, at 24 fps. A camera with no mode that fast falls back to 10 fps. Only
the centered square of each frame is read, shrunk to about 1080 pixels a side, and the preview
shows that same square, so a code is aimed at exactly where it is read. A camera that offers
neither frame rate, or nothing but MJPEG, is reported as unusable rather than half-working:
decoding MJPEG would mean linking a JPEG library into the wallet, and every webcam that offers
MJPEG offers an uncompressed mode alongside it.

On macOS the application asks for camera permission the first time an exchange needs it. On Linux the
user running Liana needs read access to the camera device: a desktop session normally gets it
through the seat ACL on `/dev/video*`, and failing that, membership of the `video` group.

V4L2 splits one USB camera into a capture node and a metadata node and enumerates both, so Liana
offers only the nodes that report video capture. A camera listed by `v4l2-ctl --list-devices` but
missing from Liana's list is one whose nodes report no video capture.

## Logs

At the default level, only refused and mismatched answers are logged, as warnings with the reason.
Run with `LOG_LEVEL=debug` for each exchange's outline: the message type, the request id the signer
echoes, the payload size, the frame count and QR version (again on every density change), and the
answer read back.

Run with `LOG_LEVEL=trace` for the content as well: the complete payload as hex and every BBQr frame
exactly as it goes on screen, for the request and for the answer. Both forms are decodable by hand,
the payload against the protocol spec and the frames as a signer's camera sees them, so a failed
exchange can be replayed from the log.

Run with `LOG_LEVEL=debug` for the receive side in detail: per-device camera enumeration, scanning
progress, and frames the scanner rejected. It is what to reach for when a scan stalls and no payload
is ever assembled.

Those payloads carry the wallet's descriptor, its extended public keys and its transactions in
clear. Treat a trace log as wallet data and do not hand one out unless the recipient is meant to see
the whole wallet.
