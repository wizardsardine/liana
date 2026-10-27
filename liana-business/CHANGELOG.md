
# Liana Business release notes

## 2.0

This release reworks the business settings, translates the business installer and settings, and
adds the encrypted descriptor export.

Liana Business is built on liana-gui, liana-ui and liana-connect, so it also carries everything
listed under 16.0 in the [Liana release notes](../CHANGELOG.md). Only what is specific to Liana
Business is listed below.

### Features

- Rework the business settings views.
- Export the encrypted descriptor from the business settings.
- Translate the business installer and settings screens.

#### Packaging

- Distribute the Windows application as a `.msi` installer instead of a bare `.exe`, built
  reproducibly with `wixl`, and codesign it with a detached Authenticode signature that anyone can
  re-attach to the reproducible installer.

### Fixes

- Fix typos and casing in the business installer and settings text.

## 1.0

First release of Liana Business and its installer.

### Features

- Installer: organizations and wallets management.
- Spending-policy template builder.
- Key and signer management with cosigner token support.
- Business settings integration.
- Last-edit tracking on templates.
- Wizardsardine fiat price source.
- Add the liana-business version to the User-Agent header.

### Fixes

- Fix missing "Approve template" action.
- Fix button size in the business installer.
- Reword "Signer" to "Signer email" in the add key modal.
- Leave the new business path timelock empty.
- Fix business key type tooltip.
