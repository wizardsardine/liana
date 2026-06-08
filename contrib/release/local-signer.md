# Local LAN signer — packaging and platform notes

The `phone_signer` module lets a Keychain phone show up as a hardware
wallet over the local Wi-Fi. It uses:

- mDNS browse + advertise (`_coincube-signer._tcp.local.`)
- TLS 1.3 with self-signed certs pinned via QR
- A long-lived TCP socket per paired phone

These have OS-level prerequisites worth documenting per platform.
The Rust side (`mdns-sd` 0.13, `rustls`/`tokio-rustls`) works
identically across the three; the packaging differences are below.

## macOS

See [`macos/README.md`](macos/README.md) — the local signer needs:

1. The hardened-runtime entitlements file at
   [`macos/coincube.entitlements`](macos/coincube.entitlements)
   wired into the `rcodesign sign` invocation.
2. The Info.plist additions documented in
   [`macos/Info.plist.local-signer.md`](macos/Info.plist.local-signer.md)
   (Bonjour usage description + service identifier list — required on
   macOS 14+).

`mDNSResponder` is system-provided on every supported macOS version;
no service needs to be installed.

## Linux

`mdns-sd` is a pure-Rust mDNS responder — it does **not** require
`avahi-daemon` to be running. The crate opens its own multicast
sockets on `224.0.0.251:5353`. That means:

- A user can run Coincube on a stock Linux box (Debian, Fedora,
  Arch) without installing `avahi`.
- If `avahi-daemon` *is* running, both will compete for the multicast
  port; mdns-sd handles `SO_REUSEPORT` and they coexist. Browsing
  works either way.
- Firewalls that block UDP/5353 will break discovery. Document the
  port in user-facing release notes when we ship.

No packaging changes are needed for AppImage / `.deb` / `.rpm`
beyond what's already shipped.

## Windows

Windows 10 1903+ has native mDNS via the `Dnsapi` service. As on
Linux, `mdns-sd` opens its own socket and doesn't depend on the
system responder. Verify on the target Windows version during QA:

- Discovery works while connected to a private network profile.
- The Windows Defender Firewall prompts on first launch for
  "Coincube" — accept "Private networks" to allow inbound on the
  pairing port and outbound multicast.

No `WiX`/`main.wxs` changes are required; the firewall prompt is
handled at first run, not at install time.

## Troubleshooting

- **Phone doesn't appear in the signer list:** confirm both devices
  are on the same VLAN/SSID. Many corporate / guest Wi-Fi setups
  block multicast between clients (AP isolation). Use the
  "Fallback host:port" field on the paired-phones row to dial the
  phone directly by IP.
- **Pairing QR scans but the phone reports `tls handshake`:** check
  that the desktop machine's clock isn't > 5 minutes off the
  phone's. Rustls' default verifier doesn't enforce NotBefore /
  NotAfter for our pinned-cert path, but the phone's TLS stack may.
- **macOS prompts "Coincube wants to find devices on your local
  network":** that's `NSLocalNetworkUsageDescription` working as
  intended. Accept to enable discovery.
