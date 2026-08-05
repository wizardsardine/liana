# MacOS packaging and distribution

We distribute the application as a zipped [MacOS app bundle](https://developer.apple.com/library/archive/documentation/CoreFoundation/Conceptual/CFBundles/BundleTypes/BundleTypes.html#//apple_ref/doc/uid/10000123i-CH101-SW5).

## Bundle identity and entitlements

| | |
|---|---|
| App ID | `io.coincube.tenshu` — the `CFBundleIdentifier` baked into [`_coincube.zip`](_coincube.zip); must also exist in the Apple Developer portal under Identifiers, with Associated Domains enabled |
| Team ID | `8UVR249AD5` |
| Keychain access group | `8UVR249AD5.io.coincube.tenshu` (`<TeamID>.<bundleID>`) |
| Entitlements | [`coincube.entitlements`](coincube.entitlements) |
| Provisioning profile | [`embedded.provisionprofile`](embedded.provisionprofile), copied to `Tenshu.app/Contents/` before signing. Expires 2044-07-30 |

Four things about this that are easy to get wrong:

- **The profile is mandatory, and omitting it does not degrade gracefully.**
  `keychain-access-groups` is a restricted entitlement, so AMFI validates it
  against the embedded profile at exec time. With the entitlement and no
  profile the app does not launch at all — SIGKILL, no window, no message —
  while `codesign -v`, `spctl` and notarization all still pass. See
  [`docs/MACOS_KEYCHAIN_ENTITLEMENT.md`](../../../docs/MACOS_KEYCHAIN_ENTITLEMENT.md)
  §4. The profile contains no secrets (public certificate, team ID, app ID and
  granted entitlements), which is why it is committed rather than held as a
  CI secret.

- **The access group is part of a keychain item's identity.** Change the bundle identifier after a
  signed, entitled build has reached users and every device secret written under the old group
  becomes unreachable, which makes their `ENCRYPTED_V3` Cubes undecryptable. Treat the identifier as
  frozen.
- **The Team ID is written literally** in the entitlements file. `$(AppIdentifierPrefix)` and
  `$(TeamIdentifierPrefix)` are Xcode build-setting substitutions; `rcodesign` signs the XML
  verbatim and expands nothing, so a signature carrying the raw variable string fails at runtime
  looking exactly like a keychain bug.
- **A different bundle identifier is a different app to macOS.** A build carrying a new identifier
  installs alongside the old one rather than replacing it.

The release workflows assert both halves of this: `releases.yml` and `nightly.yml` check the
identifier and the Bonjour keys in the unzipped template before the binary is copied in, and check
that `codesign -d --entitlements -` reports `keychain-access-groups` after signing and before
notarization.

## Notes on codesigning and notarization

Running a binary on a Mac that was not both codesigned **and** notarized by Apple is a pain. The
user needs to run it. Get an error message. Go to System preferences > Security > authorize the app.
Then try again, and finally be presented a button to open the app.

In order to avoid that, we've started distributing codesigned binaries starting from version 1.0.
This is the notes i've taken describing the stepped involved in codesigning the produced macOS
binary on a Linux machine, for posterity. This is not cleaned up.

### Bulk notes from the codesigning experiment

Create an account at https://developer.apple.com.

Pay to get into the developer program. Going the organization way is cumbersome. Go the personal
way. They'll ask for a KYC (gov ID). Wait to be accepted.

Go to "certificates, ids and profiles". Create a new certificate. Select a Developer ID application
certificate to distribute apps outside of the store.

(We should look into the installer feature later on. Maybe we could bundle a bitcoind there.)

They ask for a "Certificate Signing Request (CSR)" that you need to generate on your Mac. Generate it using OpenSSL:

```
openssl genrsa -out coincubetech_coincube.key 2048
openssl req -new -sha256 -key coincubetech_coincube.key -out coincubetech_coincube_codesigning.csr -subj "/C=US/CN=COINCUBE TECHNOLOGY LLC/emailAddress=robert@coincube.io"
```

(Note you have no choice in the size or type of the key here, they expect a RSA(2048) key.)

For the profile type select "G2 Sub-CA". We are using an Xcode newer than 11.4.1 and the codesigning
tool we use supports the new CA.

Now you get to be able to download your certificate (I've stored it as
"allen_robert_coincube_codesigning.cer"). Thankfully `rcodesign` supports various certificate format,
so we don't even have to convert it to PEM.

Download `rcodesign`:

```

curl -OL https://github.com/indygreg/apple-platform-rs/releases/download/apple-codesign%2F0.22.0/apple-codesign-0.22.0-x86_64-unknown-linux-musl.tar.gz
tar -xzf apple-codesign-0.22.0-x86_64-unknown-linux-musl.tar.gz
./apple-codesign-0.22.0-x86_64-unknown-linux-musl/rcodesign --help

```

Sign the packaged application using the `sign` command (mind `--code-signature-flags for the necessary hardened runtime):

```

./apple-codesign-0.22.0-x86_64-unknown-linux-musl/rcodesign sign --code-signature-flags runtime --entitlements-xml-path contrib/release/macos/coincube.entitlements --pem-source coincubetech_coincube.key --der-source allen_robert_coincube_codesigning.cer Tenshu.app

```

The `--entitlements-xml-path` flag is required so the hardened
runtime allows the local LAN signer (`phone_signer` module) to open
its pairing listener and to dial paired phones, and so the app can
write the device secret to its keychain access group. The
entitlements file itself is at
[`contrib/release/macos/coincube.entitlements`](coincube.entitlements).
The matching Info.plist keys that macOS 14+ requires for Bonjour
(`NSLocalNetworkUsageDescription` and `NSBonjourServices`) are baked
into the [`_coincube.zip`](_coincube.zip) template — they no longer
need to be spliced in by hand after unzipping.

You can see the chain of certificates was applied using the `diff-signatures` command against
another bundle. The best way to verify the signature is by using the `codesign` command on a Mac.

Finally, we need to notarize the app. Follow the instructions at
https://gregoryszorc.com/docs/apple-codesign/main/apple_codesign_rcodesign.html#notarizing-and-stapling:

- Create an API key from https://appstoreconnect.apple.com/ (and _not_ a key from
  https://developer.apple.com/account/resources/authkeys)
- Download it and encode it into a JSON file using the `encode-app-store-connect-api-key` command
- Use the `notary-submit` command to request notarization

```

./apple-codesign-0.22.0-x86_64-unknown-linux-musl/rcodesign notary-submit --max-wait-seconds 600 --api-key-path ./encoded_appstore_api_key.json --staple Tenshu.app

```

According to
https://developer.apple.com/documentation/security/notarizing_macos_software_before_distribution/customizing_the_notarization_workflow#3087732
this can take up to a hour. I've experienced more. You can see the status of an existing request
using the `notary-log` command.

---

Resources:

- https://gist.github.com/jcward/d08b33fc3e6c5f90c18437956e5ccc35
- https://github.com/achow101/signapple
- https://developer.apple.com/library/archive/technotes/tn2206/_index.html#//apple_ref/doc/uid/DTS40007919
- https://gregoryszorc.com/docs/apple-codesign/main/index.html
- https://www.apple.com/certificateauthority/
- https://developer.apple.com/documentation/security/notarizing_macos_software_before_distribution

Resources on packaging an application for MacOS:

- https://developer.apple.com/library/archive/documentation/CoreFoundation/Conceptual/CFBundles/BundleTypes/BundleTypes.html#//apple_ref/doc/uid/10000123i-CH101-SW5

```

```
