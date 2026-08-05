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

**A provisioning profile is required** — §4 settled that empirically on 2026-08-04, and without one the signed app is killed at launch rather than merely refused the keychain. Profiles → create a **Developer ID** profile for that App ID (the macOS type for distribution outside the App Store), download it, **verify it authorises the group** (§4), and embed it as `Tenshu.app/Contents/embedded.provisionprofile` before signing. Done already: the profile is committed at `contrib/release/macos/embedded.provisionprofile`.

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

## 4. The provisioning-profile question — SETTLED, a profile is required

> **Answered empirically 2026-08-04 on a Developer ID-signed local build.** `keychain-access-groups` is **restricted** for Developer ID macOS apps. A profile is mandatory. The rest of this section is kept as the method, not an open question — re-run it only if Apple changes the rules.

**The failure mode is worse than "the keychain refuses".** Without an embedded profile the binary does not execute at all: AMFI rejects the signature at exec, the process dies on SIGKILL with no window and no message, and the shell reports only `zsh: killed`. `codesign -v` reports the bundle valid, `spctl` passes, notarization would succeed. The evidence is only in the unified log:

```
taskgated-helper: Disallowing io.coincube.tenshu because no eligible provisioning profiles found
amfid: not valid: AppleMobileFileIntegrityError Code=-413 "No matching profile found"
kernel: AMFI: Code has restricted entitlements, but the validation of its code signature failed.
```

Retrieve it with `log show --last 15m --info --predicate 'eventMessage CONTAINS[c] "coincube"'`. **Shipping §3's entitlement without §4's profile produces a DMG that cannot launch for anybody** — strictly worse than shipping neither, and invisible to every check in the pipeline. The two land together or not at all.

**What worked:** a **Developer ID** distribution profile for App ID `io.coincube.tenshu`, generated in the portal (Profiles → Distribution → Developer ID), committed at `contrib/release/macos/embedded.provisionprofile`, copied to `Tenshu.app/Contents/embedded.provisionprofile` before signing. Its entitlements grant `keychain-access-groups = 8UVR249AD5.*`, which covers our group, and `com.apple.developer.associated-domains = *`, which macOS passkey will need later. It carries no `ProvisionedDevices` key, confirming it is a distribution rather than development profile, and expires **2044-07-30**.

`com.apple.application-identifier` in the entitlements turned out **not** to be needed — AMFI resolves the identifier from `CFBundleIdentifier`. It is left out rather than added speculatively; the profile grants it if a future need appears.

### The method, for re-running it

Apple splits entitlements into **unrestricted** (usable by just signing them in) and **restricted** (must additionally be authorised by an embedded provisioning profile) — see [TN3125](https://developer.apple.com/documentation/technotes/tn3125-inside-code-signing-provisioning-profiles). Apple's own forum guidance on which side `keychain-access-groups` falls on for Developer ID macOS apps is genuinely inconsistent: [one DTS answer](https://developer.apple.com/forums/thread/721649) says Developer ID apps don't use profiles and the entitlement is enough, while [another thread](https://developer.apple.com/forums/thread/733449) points at a *macOS* provisioning profile being relevant. The test settles it in 20 minutes; the reading never does.

Two notes from running it, both of which cost time:

- **A debug build is fine.** The test is about the signature, not the optimisation level — `make all` output signs identically and saves a release rebuild.
- **Native `codesign` is stricter than `rcodesign` about the entitlements XML.** It parses through AMFI, which rejects a double hyphen inside an XML comment — illegal per the XML spec but accepted by `plutil` and by rcodesign, so the file can be malformed for years without CI noticing. Both workflows now ad-hoc-sign a throwaway binary to catch that before it reaches anyone signing locally.

```bash
# 1. Add the entitlement (§3), then build locally
cargo build --release -p coincube-gui

# 2. Assemble the bundle the way CI does, profile included. The profile is
#    sealed by the signature, so it must be in place before step 3.
unzip contrib/release/macos/_coincube.zip
cp target/release/coincube Tenshu.app/Contents/MacOS/Coincube
cp contrib/release/macos/embedded.provisionprofile Tenshu.app/Contents/embedded.provisionprofile

# 3. Sign with the real Developer ID cert. On a Mac with the cert in the login
#    keychain, native codesign is enough — no rcodesign install needed.
codesign -f -s "Developer ID Application: COINCUBE TECHNOLOGY LLC" \
  --options runtime \
  --entitlements contrib/release/macos/coincube.entitlements \
  Tenshu.app

# 4. Confirm the entitlement landed, and that the runtime flag is set
codesign -d --entitlements - Tenshu.app
codesign -dv --verbose=4 Tenshu.app 2>&1 | grep -E 'Authority|flags'

# 5. The real test — run the app and create a Cube. Use a throwaway datadir so
#    a half-created test Cube never lands in ~/.coincube.
./Tenshu.app/Contents/MacOS/Coincube --datadir /tmp/tenshu-entitlement-test
```

**Reading the result:**

- **Killed at launch (`zsh: killed`, no window)** → restricted, profile required. This is what happened on 2026-08-04; check the log lines above to confirm it is AMFI and not a crash.
- **Launches but Cube creation fails with −34018** → the profile is present but does not authorise the group; compare the group in the profile's entitlements against the one in `coincube.entitlements`.
- **Cube creation succeeds** → the signature, the entitlement and the profile all line up. Done.

The middle case is worth pre-empting, because a profile only authorises what it actually lists: one generated before the App ID was settled, or from the wrong App ID, fails the same way as having none. Read the profile rather than inferring it from the App ID:

```bash
# What the profile authorises
security cms -D -i contrib/release/macos/embedded.provisionprofile \
  | plutil -extract Entitlements.keychain-access-groups xml1 -o - -
```

It has to *cover* §3's `8UVR249AD5.io.coincube.tenshu` — the committed profile does so with the team-wide wildcard `8UVR249AD5.*` rather than the literal string, so match on coverage, not equality. `codesign -d --entitlements -` showing the group proves only that it was *requested*.

Either way, finish by running the tests that assert the property the entitlement exists for:

```bash
SIGN_IDENTITY="Developer ID Application: COINCUBE TECHNOLOGY LLC" \
  contrib/release/macos/run-signed-device-secret-tests.sh
```

**Do not run `cargo test -p coincube-gui --lib device_secret -- --ignored` directly** — it cannot work, and it fails in the least legible way available. `cargo test` produces a *bare executable*, and a provisioning profile can only be embedded in a bundle, so AMFI kills it on exec: no output, no test names, exit 137. Signing the bare binary does not help. The script above wraps the test binary in a minimal `.app` whose `CFBundleIdentifier` is `io.coincube.tenshu`, signs that, and runs the executable from inside it. Verified green on 2026-08-04.

Those three tests (`the_item_is_device_only_and_not_synchronizable`, `concurrent_provisioning_yields_one_secret`, `provisioning_is_idempotent`) are `#[ignore]`d precisely because they cannot pass unsigned. A signed build that passes them is the proof the entitlement took — `codesign -d` only proves it was *requested*.

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
- A **signed macOS runner job** running `contrib/release/macos/run-signed-device-secret-tests.sh` (with `P12_FILE`/`P12_PASSWORD_FILE`, as §4 documents), which is what turns the three keychain tests from documentation into enforcement. Not bare `cargo test -- --ignored` — §4 explains why that is killed on exec rather than run. This is the D4 item in `company-brain/plans/cube-unlock-hardening-fixes/`.

## 7. Stale docs to fix while here

`docs/USAGE.md:43-50` is inherited verbatim from the Liana upstream and is wrong in several ways: it describes `Coincube.zip` / `Coincube-noncodesigned.zip` (CI ships `tenshu-<version>-<target>.dmg`), links `wizardsardine.com`, and cites a Wizardsardine PGP key. Users following it will not find the artifacts.

---

## Ownership

Cert and secret changes require the same approval as `docs/APPLE_CERT_ROTATION.md` — CTO or release owner. The entitlement edit itself is an ordinary PR.
