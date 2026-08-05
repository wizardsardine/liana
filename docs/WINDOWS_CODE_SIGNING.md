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
| SmartScreen reputation | Accrues over time — **not immediate**. Attaches to your *identity*, so it survives the automatic cert rotation | Accrues over time — **not immediate**. EV no longer confers instant reputation |
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
5. **Service principal for CI** — an Entra app registration granted the **Trusted Signing Certificate Profile Signer** role on the account. **Do not create a client secret**; give it a federated credential instead (§4), so CI proves who it is with a short-lived GitHub token and there is no standing credential that can sign as COINCUBE.

## 4. Wire it into `releases.yml`

**Authenticate with GitHub OIDC, not a client secret.** A stored `AZURE_CLIENT_SECRET` is a standing credential that signs code as COINCUBE TECHNOLOGY LLC: anyone who can read it, or run a workflow that can, can produce an installer Windows trusts as ours. Workload identity federation replaces it with a token GitHub mints per job and Entra exchanges — nothing long-lived to leak, and nothing to rotate.

### 4.1 Federated credential on the Entra app

On the app registration from §3.5, add a **federated credential**:

| Field | Value |
|---|---|
| Issuer | `https://token.actions.githubusercontent.com` |
| Organization / repository | `coincubetech` / `coincube` |
| Entity type | **Environment** |
| Environment name | `release` |
| Subject | `repo:coincubetech/coincube:environment:release` |
| Audience | `api://AzureADTokenExchange` |

Scope it to an *environment*, not to the repository as a whole. A repo-wide subject lets any workflow on any branch reach the signing identity; an environment subject only matches jobs that declare `environment: release`, so the environment's protection rules and approvers become the gate on signing. Create the `release` environment in repo settings, and add `environment: release` to the release job — the token's subject is built from it, so without it the exchange simply fails.

### 4.2 Secrets

Five, down from six — the tenant, client and subscription IDs are identifiers, not credentials, and stay:

```text
AZURE_TENANT_ID
AZURE_CLIENT_ID
AZURE_ENDPOINT               # e.g. https://eus.codesigning.azure.net
AZURE_CODE_SIGNING_NAME      # the Trusted Signing account name
AZURE_CERT_PROFILE_NAME      # the certificate profile name
```

### 4.3 Workflow changes

The job needs an OIDC token. `releases.yml` sets `permissions: contents: write` at the top, and naming any permission drops the rest of the defaults — so add the key rather than replacing the block:

```yaml
permissions:
  contents: write
  id-token: write
```

Then log in with OIDC and let the signing action take the credential from the environment `azure/login` leaves behind, instead of passing one:

```yaml
      - name: Azure login (OIDC)
        if: matrix.platform == 'windows'
        uses: azure/login@v2
        with:
          tenant-id: ${{ secrets.AZURE_TENANT_ID }}
          client-id: ${{ secrets.AZURE_CLIENT_ID }}
          allow-no-subscriptions: true

      - name: Sign Windows MSI
        if: matrix.platform == 'windows'
        uses: azure/artifact-signing-action@v2
        with:
          # No azure-client-secret, and no azure-tenant-id/azure-client-id
          # either: omitting them makes the action fall back to the ambient
          # Azure credential, which is the federated token from the step above.
          endpoint: ${{ secrets.AZURE_ENDPOINT }}
          signing-account-name: ${{ secrets.AZURE_CODE_SIGNING_NAME }}
          certificate-profile-name: ${{ secrets.AZURE_CERT_PROFILE_NAME }}
          files-folder: target/wix
          files-folder-filter: msi
          file-digest: SHA256
          timestamp-rfc3161: http://timestamp.acs.microsoft.com
          timestamp-digest: SHA256
```

The rename carried into the action: `azure/trusted-signing-action@v0` became `azure/artifact-signing-action@v2`, and `trusted-signing-account-name` became `signing-account-name`. Older snippets on the web still show the v0 form — it does not match this action's inputs.

Pin both actions to a checked version — don't float — and confirm the ambient-credential behaviour against Microsoft's docs before merging. If the version you pin still requires explicit credentials, pass `azure-tenant-id` and `azure-client-id` — but never reintroduce `azure-client-secret`.

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
3. Install, launch, and check for a warning at run time — that's what catches an unsigned inner `.exe`.

Read step 3 by the publisher name, not by the presence of a prompt. A correctly signed build with no reputation yet still shows "unrecognized app" — but it names the publisher. "Unknown publisher" is the failure.

Add the publisher-name check to the release checklist. `signtool verify` proves a signature exists; only the clean-VM install proves the user-visible outcome.

## 6. Fallback if Trusted Signing validation fails

If COINCUBE TECHNOLOGY LLC is rejected or stalls past the launch window, buy an **EV certificate with cloud HSM signing** — DigiCert KeyLocker, SSL.com eSigner, or Certum. Not a physical USB token: that would break CI.

Expect ~$300–700/yr, EV vetting of one to three weeks, and a second certificate-rotation runbook to sit alongside `APPLE_CERT_ROTATION.md`. Choose EV over OV for the stronger identity vetting and the hardware-key requirement — **not** for SmartScreen: neither tier buys immediate reputation any more, and both accrue it from download history the same way. Whichever route you take, plan for early downloads to still hit an "unrecognized app" prompt, and don't let a launch date depend on it being gone.

## 7. While you're here

`docs/USAGE.md:43-50` still carries Liana upstream text: wrong artifact names (`Coincube.zip` rather than `tenshu-<version>-<target>.{dmg,msi}`), a `wizardsardine.com` link, and a Wizardsardine PGP key. Anyone following it to verify a download will not find our artifacts. Fix it in the same PR that lands signing, so the install instructions and the signature story change together.

---

## Related

- `docs/APPLE_CERT_ROTATION.md` — the macOS equivalent, and the model for the rotation runbook an EV fallback would need
- `docs/MACOS_KEYCHAIN_ENTITLEMENT.md` — the macOS signing gap
- `company-brain/decisions/2026-08-03-passkey-macos-only-for-now.md` — why Windows passkey is deferred, which is adjacent but separate: signing the MSI does not address the RP-ID binding question
