
# Liana Business release notes

## 2.0

This release reworks the business settings, translates the application, and adds the encrypted
descriptor export.

Liana Business is built on liana-gui, liana-ui and liana-connect, so it also carries everything
listed under 16.0 in the [Liana release notes](../CHANGELOG.md).

### Features

- Rework the business settings views.
- Export the encrypted descriptor from the business settings.
- Translate the installer and the settings, with a language picker in the settings.
- Offer the other common derivation paths when decrypting a backup.

### Fixes

- Close the Liana Connect sessions before pruning the cache.
- Slice tab titles by characters instead of bytes.
- Show a rescan warning when a pasted descriptor is imported.
- Fix user-facing text typos and casing.

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
