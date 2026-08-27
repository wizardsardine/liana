# Air-gapped signers

Liana talks to an air-gapped signer over QR codes only. It shows a request as a QR code, or as a
sequence of them when the request does not fit one, and reads the signer's answer back through the
camera. Nothing is written to disk and no cable is involved, so the signer never touches this
computer.

Liana is always the wallet side of the exchange: it asks, the signer answers.

## The wire format

The messages are the draft signing-flow protocol for miniscript wallets. Its byte layout is
specified in [`ENCODING.md`](https://github.com/ws-plaude/bwk/blob/qr/qr-protocol/ENCODING.md), and
Liana speaks it through the [`bwk-qr`](https://github.com/ws-plaude/bwk/tree/qr/qr) crates, which
also do the framing: a message longer than one QR code is split into
[BBQr](https://bbqr.org) parts, generic-binary file type, hex encoded.

The protocol is transport agnostic and it is not BC-UR. A signer that wants to work with Liana
implements that encoding; `bwk-qr-protocol` is `no_std` and dependency-free, so a firmware can
vendor the codec as-is, and it ships a C binding for a signer that is not written in Rust.

## The four exchanges

**Get Xpubs**, during wallet creation. Liana asks for the account keys at
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
the signer the shape it displays for approval rather than a descriptor it has to take apart. Every
request describes the wallet the same way, so a proof of registration stays valid across them.

A signer answers in one of two ways. If it stored the descriptor, it says so and later requests
carry the name alone. If it is stateless, it returns a proof of registration instead, and Liana
resends the descriptor and that proof with every later request. Liana keeps whichever it was told,
and drops it as soon as the descriptor changes: a registration only ever covered the descriptor it
was made against.

**Address Verification**, from the receive screen. Liana sends the wallet name, the path under the
descriptor, and the address it derived itself. The signer derives the address on its own screen and
answers with a BIP-21 URI, which must name that very address or Liana rejects the answer.

**Signing**, from a PSBT. Liana sends the descriptor and the unsigned transaction and asks for the
signatures back rather than the whole transaction, which is all it needs and keeps the answer
small enough to cross in far fewer frames. That is a preference, not a demand: a signer is free to
return the complete PSBT instead, and Liana merges either form to the same result. Whichever arrives,
every signature is verified against the original transaction before it is kept.

## What Liana checks before trusting an answer

Every response must be a response, echo the request id Liana generated, and be of the type that was
asked. So an answer left on a signer's screen from an earlier exchange cannot be mistaken for this
one. That is the usual way a scan starts, since the signer is still showing its previous answer when
the camera opens, so Liana says as much and keeps scanning rather than treating it as a failure: the
real answer is picked up as soon as the signer produces it.

A returned PSBT never replaces Liana's own. Liana requires the same unsigned transaction and the
same input and output counts, refuses an answer that dropped or altered a signature that was
already there, and takes only signatures whose key the input actually expects. Each one is verified
against the original transaction before it is merged into Liana's copy, so nothing else in the PSBT
can be rewritten on the way back. An answer that adds no signature at all is refused.

## Brightness

Every QR in the application is drawn the same way: encoded with `bwk-qr` and painted as a raster,
rather than through iced's `QRCode` widget. That keeps the codes independent of which graphics
backend is compiled in.


Every code is drawn at the sparsest setting the transport offers, so the widest range of cameras can
read it. A denser code would need fewer frames, but a signer that cannot read one has no way to say
so, and there is nothing to trade away by being conservative.

What is adjustable is how bright the light modules are drawn, on a slider from 20% to 100% of the
theme's light colour. A screen at full white washes out a camera sensor, and the blown-out pixels
bleed over the dark modules until the code stops decoding, so the slider starts halfway at 50% rather
than at the top. If a signer will not lock onto the code, turn it further down before trying anything
else. On a screen that is already dim, or in bright ambient light, go the other way. The dark modules
are never touched, so the code cannot be inverted or flattened by mistake.

## Camera

Liana captures through Media Foundation on Windows, V4L2 on Linux, and AVFoundation on macOS. Frames
are decoded on the capture thread and only the preview reaches the interface; the stream is released
when the exchange ends, is cancelled, fails, or times out.

The camera is asked for an uncompressed mode close to 720p, and its frames are turned into grayscale
directly, which is all a QR decoder needs. A camera that offers nothing but MJPEG is reported as
unusable rather than half-working: decoding MJPEG would mean linking a JPEG library into the wallet,
and every webcam that offers MJPEG offers an uncompressed mode alongside it.

On macOS the application asks for camera permission the first time an exchange needs it. On Linux the
user running Liana needs read access to the camera device: a desktop session normally gets it
through the seat ACL on `/dev/video*`, and failing that, membership of the `video` group.

V4L2 splits one USB camera into a capture node and a metadata node and enumerates both, so Liana
opens each one and offers only those that can actually produce a frame. A camera listed by
`v4l2-ctl --list-devices` but missing from Liana's list is one whose nodes report no capture
format.

## Logs

Every exchange writes what it sent and what came back at the default log level, with no
configuration needed: the message type, the request id the signer echoes, the payload size and
frame count, then the complete payload as hex and every BBQr frame exactly as it goes on screen.
Both forms are decodable by hand, the payload against the protocol spec and the frames as a signer's
camera sees them, so a failed exchange can be replayed from a log the user already has. Refused and
mismatched answers are logged as warnings with the reason.

Run with `LOG_LEVEL=debug` for the receive side in detail: per-device camera enumeration, every QR
the camera reads back reported once however long it stays in view, scanning progress, and frames the
scanner rejected. That level costs a second scan of each frame, which is why it is not on by
default; it is what to reach for when a scan stalls and no payload is ever assembled.

Those payloads carry the wallet's descriptor, its extended public keys and its transactions in
clear. Treat a Liana log as wallet data and do not hand one out unless the recipient is meant to see
the whole wallet.
