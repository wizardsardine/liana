# Apple Developer Certificate Rotation Playbook  
**COINCUBE TECHNOLOGY LLC**

## Purpose
This document describes the safe, repeatable process for rotating Apple Developer certificates used for macOS app signing and notarization in Coincube’s CI/CD pipeline.

Apple certificates expire periodically and must be rotated without breaking releases.

---

## Certificates in Scope

| Certificate | Used For | Required |
|------------|--------|---------|
| Developer ID Application | Signing .app and .dmg | Required |
| Developer ID Installer | Signing .pkg installers | Optional (PKG only) |

Coincube currently ships DMGs, not PKGs.  
The Installer cert is not used in CI, but should be preserved.

---

## When to Rotate

### Automatic Alerts
- A GitHub Actions workflow (apple-cert-monitor.yml) runs weekly
- If the certificate has < 30 days remaining, the workflow fails and emails repo admins

### Manual Triggers
Rotate immediately if:
- Private key is suspected compromised
- Apple revokes the cert
- CI signing fails due to cert trust errors

---

## Rotation Overview

Generate new cert in Apple Portal  
→ Export as .p12  
→ Update GitHub Secrets  
→ Verify with test release  
→ Revoke old cert (optional)

Never revoke a cert before CI is updated and verified.

---

## Step-by-Step Rotation Procedure

### 1. Create New Certificate
1. Log in to Apple Developer Portal
2. Certificates → Add → Developer ID Application
3. Generate using CSR from Keychain Access
4. Download and install into login keychain
5. Verify private key is present

```bash
security find-identity -v -p basic
```

---

### 2. Export Certificate
- Export certificate + private key as .p12
- Use a strong password

---

### 3. Update GitHub Secrets
Update:
- CERTIFICATE_P12_BASE64
- CERTIFICATE_P12_PASSWORD

```bash
base64 coincube_dev_id_app_NEW.p12 | pbcopy
```

---

### 4. Verify in CI
Create a test tag and push it.
Confirm build, signing, notarization, and DMG upload succeed.

---

### 5. Revoke Old Certificate
After successful verification, revoke the old cert in Apple Developer Portal.

---

## Installer Certificate (PKG Only)
Only required if Coincube ships PKG installers in the future.

---

## Security Best Practices
- Never commit .p12 or .p8 files
- Restrict GitHub Secrets access
- Store backups in encrypted vaults
- Rotate immediately if compromised

---

## Ownership
Certificate rotation requires approval from the CTO or release owner.
