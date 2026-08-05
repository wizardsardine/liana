# Windows code signing — setup runbook

**Why:** `releases.yml` builds the MSI with `cargo wix` and uploads it with **no Authenticode signature**. `docs/USAGE.md` admits it: *"We do not yet distribute codesigned binaries for Windows at this time."*

Every Windows user installing Tenshu sees a SmartScreen warning naming an **Unknown publisher**, and has to click through it. For a Bitcoin wallet that is worse than cosmetic: it trains users to dismiss precisely the warning that would otherwise flag a trojaned installer, and it leaves us with no way to tell a real Tenshu MSI from a hostile one.

**Target: signed MSI before public Beta.** Lead time is the risk, not effort — start the identity validation early, because the signing wiring itself is a couple of hours.

---

## 1. Recommendation: Azure Trusted Signing

**Use [Azure Trusted Signing](https://learn.microsoft.com/en-us/azure/artifact-signing/) (recently renamed Azure Artifact Signing), not a traditional EV certificate.**

| | Azure Trusted Signing | Traditional EV cert |
|---|---|---|
| Cost | ~**$9.99/month** (~$120/yr), Basic tier = 5,000 signatures/month | ~$287–700/yr |
| SmartScreen reputation | **Immediate**, and tied to your *identity* rather than a specific cert | Immediate for EV; OV must accrue download history |
| Key storage | Microsoft-managed HSM | FIPS 140-2 L2 hardware **mandatory** since June 2023 — physical token or a cloud HSM |
| Works in GitHub Actions | **Yes, natively** — 6 secrets and an official action | Physical token: **no**, needs a self-hosted runner with the token plugged in. Cloud HSM: yes, at extra cost |
| Cert lifetime | Short-lived, rotated automatically | 1–3 years, manual renewal |
| Ongoing ops | None | Token custody, renewal, a second rotation runbook alongside `APPLE_CERT_ROTATION.md` |

The deciding factor is CI. `releases.yml` is fully automated; a physical USB token would force either a self-hosted Windows runner or a manual signing step on someone's desk for every release. Trusted Signing is built for the automated case.

The cost difference is real but secondary. The operational difference — no hardware to hold, lose, or rotate — matters more given we already carry one Apple certificate-rotation burden.

## 2. Eligibility — check this first

**COINCUBE TECHNOLOGY LLC should qualify**, but confirm before planning around it:

- **Paid Azure subscription required.** Free, trial and sponsored subscriptions are explicitly excluded — pay-as-you-go or EA only.
- **Geography.** Public Trust certificates are available to organizations in the US, Canada, the EU, the UK, Australia, New Zealand, Japan, South Korea, Singapore, Switzerland, Norway and Israel. A US LLC is fine.
- **Business age.** Microsoft originally required three years of verifiable legal existence. That has been relaxed — self-employed individuals can now apply — but practitioners report **inconsistent validation outcomes for newer entities**, sometimes without a clear reason. If COINCUBE TECHNOLOGY LLC is under three years old, treat approval as likely-but-not-certain and keep the EV fallback in §6 warm.
- **Identity match.** The certificate subject is sourced from the **Azure billing account**. Legal name and address must match *exactly* what should appear on the certificate.

> ⚠️ **Set the Azure billing account's legal name to `COINCUBE TECHNOLOGY LLC`** — byte-identical to the Apple Developer ID cert's subject — *before* starting validation. A mismatch produces a certificate with the wrong subject, and it cannot be edited afterwards; you re-validate.

**Timeline:** Microsoft says identity validation can complete in about an hour. Reported reality ranges from ten minutes to over ten days, with one account taking a month. Delays cluster around domain-ownership verification. **Validation requests cannot be expedited.** Start now.

## 3. Setup

1. **Azure subscription** — pay-as-you-go, billing legal name exactly `COINCUBE TECHNOLOGY LLC`.
2. **Register the resource provider** and create a **Trusted Signing account** (portal or CLI). Pick a region near CI; it doesn't affect trust.
3. **Identity validation** — Organization type. Needs legal name, business address, business identifier, website URL (`coincube.io`), and two contact emails. Assign yourself the **Trusted Signing Identity Verifier** role first, or the request can't be created. Expect a domain-ownership step on `coincube.io` — you already control the DNS and the `.well-known` path from the AASA work.
4. **Certificate profile** — Public Trust, once validation passes.
5. **Service principal for CI** — an Entra app registration with a client secret, granted the **Trusted Signing Certificate Profile Signer** role on the account.

## 4. Wire it into `releases.yml`

Six repository secrets, mirroring the Apple ones already there:

```
AZURE_TENANT_ID
AZURE_CLIENT_ID
AZURE_CLIENT_SECRET
AZURE_ENDPOINT               # e.g. https://eus.codesigning.azure.net
AZURE_CODE_SIGNING_NAME      # the Trusted Signing account name
AZURE_CERT_PROFILE_NAME      # the certificate profile name
```

Add one step to the Windows leg, **after** `cargo wix` produces the MSI and **before** the upload:

```yaml
      - name: Sign Windows MSI
        if: matrix.platform == 'windows'
        uses: azure/trusted-signing-action@v0
        with:
          azure-tenant-id: ${{ secrets.AZURE_TENANT_ID }}
          azure-client-id: ${{ secrets.AZURE_CLIENT_ID }}
          azure-client-secret: ${{ secrets.AZURE_CLIENT_SECRET }}
          endpoint: ${{ secrets.AZURE_ENDPOINT }}
          trusted-signing-account-name: ${{ secrets.AZURE_CODE_SIGNING_NAME }}
          certificate-profile-name: ${{ secrets.AZURE_CERT_PROFILE_NAME }}
          files-folder: target/wix
          files-folder-filter: msi
          file-digest: SHA256
          timestamp-rfc3161: http://timestamp.acs.microsoft.com
          timestamp-digest: SHA256
```

Verify the action's current major version before merging — pin it, don't float.

**Do the same in `nightly.yml`.** Both workflows build a Windows MSI; leaving nightly unsigned means the artifact people actually test differs from the one that ships.

**Sign the `.exe` too, not just the MSI.** `cargo wix` packages an already-built binary. Signing only the installer leaves the installed executable unsigned, so SmartScreen and any endpoint-protection tooling still flag it at *run* time. Sign `target/<target>/minimal/coincube.exe` before `cargo wix`, then sign the MSI after.

## 5. Verification

In CI, after signing:

```powershell
signtool verify /pa /v target\wix\tenshu-*.msi
```

Manually, on a clean Windows VM with no prior reputation:

1. Download the MSI the way a user would (browser, from the GitHub Release).
2. Confirm SmartScreen shows **COINCUBE TECHNOLOGY LLC** as the publisher, not "Unknown publisher".
3. Install, launch, and confirm no warning at run time — that's the check that catches an unsigned inner `.exe`.

Add the publisher-name check to the release checklist. `signtool verify` proves a signature exists; only the clean-VM install proves the user-visible outcome.

## 6. Fallback if Trusted Signing validation fails

If COINCUBE TECHNOLOGY LLC is rejected or stalls past the launch window, buy an **EV certificate with cloud HSM signing** — DigiCert KeyLocker, SSL.com eSigner, or Certum. Not a physical USB token: that would break CI.

Expect ~$300–700/yr, EV vetting of one to three weeks, and a second certificate-rotation runbook to sit alongside `APPLE_CERT_ROTATION.md`. Choose EV over OV — OV requires accumulating download reputation before SmartScreen goes quiet, which for a wallet released to a small initial audience could take months.

## 7. While you're here

`docs/USAGE.md:43-50` still carries Liana upstream text: wrong artifact names (`Coincube.zip` rather than `tenshu-<version>-<target>.{dmg,msi}`), a `wizardsardine.com` link, and a Wizardsardine PGP key. Anyone following it to verify a download will not find our artifacts. Fix it in the same PR that lands signing, so the install instructions and the signature story change together.

---

## Related

- `docs/APPLE_CERT_ROTATION.md` — the macOS equivalent, and the model for the rotation runbook an EV fallback would need
- `docs/MACOS_KEYCHAIN_ENTITLEMENT.md` — the macOS signing gap
- `company-brain/decisions/2026-08-03-passkey-macos-only-for-now.md` — why Windows passkey is deferred, which is adjacent but separate: signing the MSI does not address the RP-ID binding question
