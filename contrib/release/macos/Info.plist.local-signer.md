# Info.plist additions for the local LAN signer

The packaged `Coincube.app/Contents/Info.plist` lives inside the
`_coincube.zip` template that the release workflow unpacks during
notarization. Until the zip is regenerated, splice the keys below
into that file by hand after unzipping. They unlock:

- Bonjour / mDNS access (`NSLocalNetworkUsageDescription` and
  `NSBonjourServices`) — required on macOS 14+ for browsing/
  advertising the `_coincube-signer._tcp` service used by the
  `phone_signer` module to pair Keychain phones.

## Keys to add

Insert these inside the top-level `<dict>` of `Info.plist`, anywhere
between the existing entries:

```xml
<key>NSLocalNetworkUsageDescription</key>
<string>Coincube uses your local network to pair and sign with phones running the Keychain app over Wi-Fi.</string>

<key>NSBonjourServices</key>
<array>
    <string>_coincube-signer._tcp</string>
</array>
```

## After splicing

Codesign with the matching entitlements file:

```
rcodesign sign \
    --code-signature-flags runtime \
    --entitlements-xml-path contrib/release/macos/coincube.entitlements \
    --pem-source <key.pem> \
    --der-source <cert.der> \
    Coincube.app
```

`com.apple.security.network.server` + `com.apple.security.network.client`
in [coincube.entitlements](coincube.entitlements) are the bare minimum
the local signer needs under the hardened runtime — without them the
pairing listener can't accept and the steady-state dial can't
connect.

When the `_coincube.zip` template is next regenerated, fold these
keys into the baked Info.plist so this manual splice can be removed.
