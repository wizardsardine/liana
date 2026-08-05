# macOS keychain-access-groups entitlement — setup runbook

**Why:** the device secret that seals every `ENCRYPTED_V3` Cube lives in the macOS **data-protection keychain**, which is only reachable from a binary signed with a `keychain-access-groups` entitlement. Without it `SecItemAdd` returns `errSecMissingEntitlement` (−34018), `device_secret::get_or_create` fails closed, and **Cube creation refuses**.

**Status when this was written (2026-08-03):** the signing pipeline is complete and production-wired. The entitlement is the only missing piece — plus one bundle-identifier problem that has to be settled first.

> ⚠️ **This is a live shipping defect, not a future task.** The DMG that `releases.yml` produces today is signed, notarized and stapled — and it carries only `com.apple.security.network.{client,server}`. Any user who installs the current release build on macOS and tries to create a Cube gets "Cubes can't be created safely here." Ship the unlock-hardening work without this and macOS is dead on arrival.

---

## 1. What already exists (nothing to build)

| Piece | Where |
|---|---|
| Developer ID Application cert (COINCUBE TECHNOLOGY LLC) | GitHub secrets `CERTIFICATE_P12_BASE64` + `CERTIFICATE_P12_PASSWORD` |
| App Store Connect API key (notarization) | GitHub secret `APPSTORECONNECT_KEY_JSON` |
| Signing step | `.github/workflows/releases.yml` → `rcodesign sign --for-notarization --entitlements-xml-path contrib/release/macos/coincube.entitlements` |
| Same step, nightly | `.github/workflows/nightly.yml` — **points at the same entitlements path**, so one edit covers both |
| Notarize + staple + verify | `rcodesign notary-submit --staple` then `spctl --assess --type execute` |
| `.app` bundle to attach entitlements to | `contrib/release/macos/_coincube.zip` → `Tenshu.app` |
| Cert expiry monitoring | `.github/workflows/apple-cert-monitor.yml`, weekly, fails under 30 days |
| Rotation runbook | `docs/APPLE_CERT_ROTATION.md` |
| Team ID | `8UVR249AD5` (confirmed in the Keychain app's Xcode project and the AASA) |

Manual procedure and history: `contrib/release/macos/README.md`.

## 2. Settle the bundle identifier first — rename to Tenshu

Three identifiers are in play and two of them disagree:

| Identifier | Where | Status |
|---|---|---|
| `io.coincubetech.Coincube` | `Tenshu.app/Contents/Info.plist` inside `_coincube.zip` | **What actually ships** |
| `io.coincube.coincube` | `coincube-frontend/static/.well-known/apple-app-site-association` | What the AASA declares for the desktop app |
| `io.coincube.keychainApp` | Keychain iOS app | Consistent, unrelated |

**Decision (Robert, 2026-08-03): rename to `io.coincube.tenshu`.** The app was renamed to COINCUBE | Tenshu on 2026-07-11 and the bundle's `CFBundleName` / `CFBundleDisplayName` already say `Tenshu` — only the identifier lagged. `io.coincube.tenshu` also matches the `io.coincube.*` convention the Keychain app uses, and the device-secret keychain service constant is already `io.coincube.tenshu.device-secret.v1`, so the whole namespace becomes coherent.

### Why now is the safest possible moment

**A keychain access group is part of the item's identity.** Change it after Cubes exist and every device secret written under the old group becomes unreachable — which means **every v3 Cube on that machine becomes undecryptable**. Because the entitlement has never shipped, there are currently **zero items in the data-protection keychain**. There is nothing to strand. That window closes the moment a signed build with an entitlement reaches a user.

### Blast radius — smaller than it looks

Verified 2026-08-03:

- **User data does not move.** `coincube-gui/src/dir.rs:71` derives the datadir from `dirs::config_dir()` / `~/.coincube` — a hardcoded string, not the bundle ID. No migration.
- **No Rust code references the bundle identifier.** Nothing to change in the app.
- **The keychain *service* name is separate from the *access group*.** `SERVICE = "io.coincube.tenshu.device-secret.v1"` (`device_secret/mod.rs:39`) is already correct and does not change.

What does change: the `CFBundleIdentifier` in the zipped Info.plist, the AASA and its test, and the Apple Developer portal registration.

**Existing installs:** macOS treats a different bundle ID as a different application, so an updated build installs *alongside* the old one rather than over it. Pre-Beta this should be near-zero users, but confirm before shipping and put a line in the release notes if not.

### 2.1 `coincube-frontend` — yes, it needs updating

Three files:

- `static/.well-known/apple-app-site-association` — `8UVR249AD5.io.coincube.coincube` → `8UVR249AD5.io.coincube.tenshu`
- `cypress/e2e/well-known.spec.ts:31` — `AASA_APPS` pins the exact array; update it or the test fails
- `static/.well-known/README.md` — documents the entries

`.svelte-kit/output/client/.well-known/…` is build output and regenerates. `assetlinks.json` is Android/Keychain only — untouched.

**Deploy the AASA change before shipping an app with the new identifier.** Associated-domains validation fetches the file and checks the app is listed; an app whose ID isn't in the deployed AASA fails validation. Apple's AASA CDN also caches, so allow up to ~24h before testing the app side. Nothing consumes the desktop entry today (no entitlement has ever shipped), so this is low-risk — but keep the order.

### 2.2 Apple Developer portal — yes, one registration

**Done 2026-08-03** — App ID `io.coincube.tenshu` registered under prefix `8UVR249AD5`, explicit, all platforms, with **Associated Domains** enabled.

For reference, what that step was:

- Certificates and Identifiers → **Identifiers** → new App ID, explicit Bundle ID `io.coincube.tenshu`
- Enable **Associated Domains** — needed for passkey later, and cheaper now than amending the App ID and regenerating any profiles built on it

**There is no "Keychain Sharing" capability in the portal, and its absence is not a missed step.** Keychain Sharing is an Xcode-side capability whose only effect is writing `keychain-access-groups` into the entitlements file. Nothing to tick.

That is a statement about the *portal*, not about the signature. It does not answer whether the signed binary needs an embedded provisioning profile to be *authorised* for the group it requests — §4 settles that, and if the answer is yes, the profile itself has to list the group (§4 shows how to check).

**The Bundle ID is immutable** — Apple does not allow editing it after creation. A typo means abandoning the App ID and creating another, so verify it character-for-character against the entitlement value in §3.

Then, **only if the §4 test shows a profile is required**: Profiles → create a **Developer ID** provisioning profile for that App ID (the macOS profile type for distribution outside the App Store), download it, **verify it authorises the group** (§4), and embed it as `Tenshu.app/Contents/embedded.provisionprofile` before signing.

**No certificate work.** The existing Developer ID Application certificate is not bundle-specific and is unaffected — nothing in `APPLE_CERT_ROTATION.md` changes.

### 2.3 The Info.plist edit

`CFBundleIdentifier` → `io.coincube.tenshu`, inside `_coincube.zip`. That zip needs regenerating anyway (§5), so fold both changes into one pass.

## 3. The entitlement edit

`contrib/release/macos/coincube.entitlements` — add to the existing `<dict>`:

```xml
    <!--
        Data-protection keychain access for the Cube device secret
        (services/unlock/device_secret). Without this the keychain returns
        errSecMissingEntitlement (-34018), get_or_create fails closed, and
        Cube creation refuses. See docs/MACOS_KEYCHAIN_ENTITLEMENT.md.
    -->
    <key>keychain-access-groups</key>
    <array>
        <string>8UVR249AD5.io.coincube.tenshu</string>
    </array>
```

**Write the Team ID literally.** `$(AppIdentifierPrefix)` and `$(TeamIdentifierPrefix)` are *Xcode build-setting substitutions*. This pipeline signs with `rcodesign` against a literal XML file — nothing expands those variables, and a signature containing the raw string `$(AppIdentifierPrefix)…` will fail at runtime in a way that looks like a keychain bug.

No Rust changes needed. `apple.rs` doesn't set `kSecAttrAccessGroup` explicitly, so items land in the first group in the entitlement. The service name (`io.coincube.tenshu.device-secret.v1`) is independent of the access group.

## 4. Settle the provisioning-profile question empirically, not by reading

Apple splits entitlements into **unrestricted** (usable by just signing them in) and **restricted** (must additionally be authorised by an embedded provisioning profile) — see [TN3125](https://developer.apple.com/documentation/technotes/tn3125-inside-code-signing-provisioning-profiles). Apple's own forum guidance on which side `keychain-access-groups` falls on for Developer ID macOS apps is genuinely inconsistent: [one DTS answer](https://developer.apple.com/forums/thread/721649) says Developer ID apps don't use profiles and the entitlement is enough, while [another thread](https://developer.apple.com/forums/thread/733449) points at a *macOS* provisioning profile being relevant.

Don't try to resolve this from documentation. It is a binary question with a 20-minute test, and you already have every input:

```bash
# 1. Add the entitlement (§3), then build locally
cargo build --release -p coincube-gui

# 2. Assemble the bundle the way CI does
unzip contrib/release/macos/_coincube.zip
cp target/release/coincube Tenshu.app/Contents/MacOS/Coincube

# 3. Sign with the real Developer ID cert (same p12 as CI)
rcodesign sign \
  --entitlements-xml-path contrib/release/macos/coincube.entitlements \
  --p12-file cert.p12 --p12-password-file cert.pass \
  Tenshu.app

# 4. Confirm the entitlement actually landed in the signature
codesign -d --entitlements - Tenshu.app

# 5. The real test — run the app and create a Cube.
./Tenshu.app/Contents/MacOS/Coincube
```

**Reading the result:**

- **Cube creation succeeds** → unrestricted, no profile needed. Done; commit the entitlement and move on.
- **Still −34018** → restricted. Use the App ID registered in §2.2, download a **Developer ID provisioning profile**, add it to the bundle as `Tenshu.app/Contents/embedded.provisionprofile`, and add a step to both workflows to place it before signing. Also add `com.apple.application-identifier` = `8UVR249AD5.io.coincube.tenshu` to the entitlements.

  A profile only authorises what it actually lists, and a profile generated
  before the App ID was right — or from the wrong App ID — is the failure mode
  that looks identical to having no profile at all. Confirm the group is in it
  before signing, and again after, rather than inferring it from the App ID:

  ```bash
  # What the profile authorises
  security cms -D -i Tenshu.app/Contents/embedded.provisionprofile \
    | plutil -extract Entitlements.keychain-access-groups xml1 -o - -
  # Must list 8UVR249AD5.io.coincube.tenshu — matching §3's entitlement exactly.
  ```

  An entitlement the profile does not authorise is dropped or rejected at
  launch, so `codesign -d --entitlements -` showing the group proves only that
  it was *requested*.

Either way, finish by running the three tests that assert the property the entitlement exists for: `the_item_is_device_only_and_not_synchronizable`, `concurrent_provisioning_yields_one_secret`, `provisioning_is_idempotent`. They are `#[ignore]`d precisely because they cannot pass unsigned, and a host that passes them is the proof the entitlement took — `codesign -d` only proves it was *requested*.

**Plain `cargo test -- --ignored` cannot be that proof.** The entitlement is attached to the executable inside `Tenshu.app`; Cargo builds a *different* Mach-O under `target/`, which this pipeline never signs (on Apple silicon the linker ad-hoc signs it, and an ad-hoc signature carries no team identity, so a `TEAMID.`-prefixed access group is not authorised). Run it against a correctly signed app and it still returns −34018 — a red result that says nothing about the app. The host that runs these three has to carry the entitlement itself.

**If §4 showed no profile is needed**, sign the test binary and run that:

```bash
# Build the harness without running it, and capture its path
BIN=$(cargo test -p coincube-gui --lib --no-run --message-format=json \
  | jq -r 'select(.profile.test == true) | .executable' | grep . | tail -1)

rcodesign sign \
  --entitlements-xml-path contrib/release/macos/coincube.entitlements \
  --p12-file cert.p12 --p12-password-file cert.pass \
  "$BIN"

"$BIN" device_secret --ignored --test-threads=1
```

**If a profile is required**, that route is closed: a bare executable has nowhere to carry `embedded.provisionprofile` — that path exists only inside a bundle. The host then has to be the bundle. Add a self-test entry point to the app (`Tenshu.app/Contents/MacOS/Coincube --selftest-keychain`) that performs the same three assertions and exits non-zero on failure, sign and embed the profile as in §2.2, and run *that*. Keep the assertions in one place — have the self-test call the same helpers the `#[ignore]`d tests do, so the two cannot drift.

## 5. Regenerate `_coincube.zip` while you're in there

The baked `Info.plist` is already known-stale. `contrib/release/macos/Info.plist.local-signer.md` documents keys that must be spliced in by hand after unzipping — `NSLocalNetworkUsageDescription` and `NSBonjourServices` (`_coincube-signer._tcp`) for the LAN phone signer — **and CI never performs that splice.** It only `sed`s the version placeholder.

So the shipped bundle is missing Bonjour declarations the LAN signer needs. Fold them in with the bundle-ID change (§2.3), then delete the manual-splice doc. Also drop the duplicate `CFBundleSignature` key (declared twice; the second wins).

## 6. CI verification worth adding

The pipeline currently proves the app is *signed and notarized*, not that it *works*. Two cheap additions:

- After signing, assert the entitlement is present rather than assuming:
  ```bash
  codesign -d --entitlements - Tenshu.app 2>&1 | grep -q 'keychain-access-groups' \
    || { echo "entitlement missing from signature"; exit 1; }
  ```
- A **signed macOS runner job** running the three keychain tests through a host that carries the entitlement — the signed test binary, or the app's `--selftest-keychain` mode if a profile is required (§4). Not bare `cargo test -- --ignored`: that harness is unsigned, so it fails on a correctly signed build and enforces nothing. This is the D4 item in `company-brain/plans/cube-unlock-hardening-fixes/`.

## 7. Stale docs to fix while here

`docs/USAGE.md:43-50` is inherited verbatim from the Liana upstream and is wrong in several ways: it describes `Coincube.zip` / `Coincube-noncodesigned.zip` (CI ships `tenshu-<version>-<target>.dmg`), links `wizardsardine.com`, and cites a Wizardsardine PGP key. Users following it will not find the artifacts.

---

## Ownership

Cert and secret changes require the same approval as `docs/APPLE_CERT_ROTATION.md` — CTO or release owner. The entitlement edit itself is an ordinary PR.
