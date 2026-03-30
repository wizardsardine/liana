# Coincube Release Notes

## Purpose and Scope

This document tracks all changes made to Coincube since its inception as a hard fork of [Liana](https://github.com/wizardsardine/liana) (v13.0) by Wizardsardine. Coincube is a comprehensive Bitcoin wallet solution featuring vault custody, Lightning-enabled liquid spending, integrated buy/sell, and peer-to-peer trading — built by Coincube Technology LLC.

For architecture details, see the [Devin Wiki](https://app.devin.ai/wiki/coincubetech/coincube). For the original Liana changelog, see `CHANGELOG_LIANA.md`.

Sources: `README.md`, `Cargo.toml` (workspace root)

---

## Liana vs Coincube: What Changed

The table below summarizes what Coincube inherits from Liana and what is new or significantly modified.

### Inherited from Liana (Vault Engine)

These components are largely unchanged from the Liana v13.0 codebase:

| Component | Description | Sources |
|-----------|-------------|---------|
| `coincubed` daemon | Core wallet engine, JSON-RPC 2.0 API over Unix socket, PSBT handling, coin selection | `coincubed/src/` |
| Output descriptors | BIP 380 descriptor parsing, Miniscript, multi-path policies | `coincube-core/src/descriptors/` |
| Time-locked recovery | CSV-based recovery paths, coin refresh, inheritance timelocks | `coincube-core/src/spend.rs` |
| Database layer | SQLite schema for addresses, coins, transactions, labels, spend PSBTs | `coincubed/src/database/` |
| Bitcoin backends | Bitcoin Core (managed/external), Electrum, Liana Connect | `coincubed/src/bitcoin/` |
| Hardware wallet support | Ledger, BitBox02, Coldcard, Jade, Specter DIY via `async-hwi` | `coincube-gui/src/hw.rs` |
| Installer wizard | Multi-step wallet creation with descriptor editor, key import, device registration | `coincube-gui/src/installer/` |
| Transaction management | Spend creation, RBF, coin control, PSBT signing workflow | `coincube-gui/src/app/state/vault/` |
| Multi-wallet / pane / tab | Multiple wallets with panes and tabs in a single window | `coincube-gui/src/gui/` |
| Functional test framework | Python-based RPC integration tests | `tests/test_rpc.py` |

### Added by Coincube

These are entirely new subsystems built on top of the Liana base:

| Feature | Description | Sources |
|---------|-------------|---------|
| **Cube Architecture** | Multi-cube launcher with per-cube settings, PIN entry, named cubes | `coincube-gui/src/launcher.rs`, `coincube-gui/src/app/settings/` |
| **Liquid Wallet** | Lightning-enabled spending wallet via Breez SDK Liquid (send, receive, on-chain swap) | `coincube-gui/src/app/state/liquid/` |
| **Vault ↔ Liquid Transfers** | Bidirectional fund transfers between vault (on-chain) and liquid (Lightning) with HW signing | `coincube-gui/src/app/state/liquid/transfer.rs` |
| **Buy/Sell** | Integrated fiat on/off-ramp via Mavapay (Africa) and Meld (international) with CEF webview | `coincube-gui/src/app/state/buysell/` |
| **Mostro P2P Trading** | Decentralized peer-to-peer BTC trading over Nostr with chat, disputes, hold invoices | `coincube-gui/src/app/state/p2p/` |
| **USDt Wallet** | Liquid-based USDt support via SideSwap with cross-asset payments | `coincube-gui/src/app/state/liquid/send.rs` |
| **Border Wallet Signer** | Recovery phrase generation from grid patterns with PSBT signing | `coincube-core/src/border_wallet/` |
| **Connect Module** | Remote backend connectivity, lightning addresses, avatar system | `coincube-gui/src/app/state/connect/` |
| **Light/Dark Mode** | User-selectable themes with persistence, theme-aware rendering | `coincube-ui/src/theme/`, `coincube-gui/src/gui/mod.rs` |
| **Global Home Dashboard** | Unified home showing combined Vault + Liquid balances with accordion sidebar | `coincube-gui/src/app/view/mod.rs` |
| **Toast System** | Global overlay notifications with WCAG AA severity colors, log level propagation | `coincube-gui/src/app/view/warning.rs` |
| **Coincube API Client** | Go backend integration for buy/sell, geolocation, registration, user management | `coincube-gui/src/services/coincube/` |
| **Fiat Price** | Real-time fiat price display, configurable source, fiat editing on send page | `coincube-gui/src/app/state/settings/general.rs` |
| **Release Infrastructure** | GitHub Actions CI/CD, Windows MSI, macOS DMG with GPG signing, Linux packages | `.github/workflows/` |

### Modified from Liana

These Liana components were significantly adapted for Coincube:

| Component | What Changed | Sources |
|-----------|-------------|---------|
| GUI theme & branding | Full rebrand (colors, logotype, icons), warm color palette, light/dark mode | `coincube-ui/src/color.rs`, `coincube-ui/src/theme/` |
| Sidebar navigation | Accordion-based with expandable sections (Vault, Liquid, Marketplace, P2P, USDt, Connect) | `coincube-gui/src/app/view/mod.rs` |
| Settings persistence | Added `GlobalSettings` with theme mode, developer mode, account tier | `coincube-gui/src/app/settings/mod.rs` |
| PIN security | Encrypted PIN storage, PIN-gated cube access, confirmation flows | `coincube-gui/src/app/settings/mod.rs` |
| Iced framework | Upgraded from Iced 0.13.1 to 0.14.0, deprecated `iced_wry` | `Cargo.toml` |

---

## Release Timeline

### March 2026

#### Features

**Mostro P2P Trading**

Sources: `coincube-gui/src/app/state/p2p/`
- Integrated Mostro protocol for decentralized peer-to-peer Bitcoin trading over Nostr.
- Order book with real-time subscription and order form validation (node limits, premium slider, fiat bounds).
- P2P chat system with deterministic nicknames derived from pubkeys.
- Dispute chat system for trade resolution.
- Hold invoice support with copy feedback.
- Hide Cancel/Dispute/Contact buttons on completed trades.

**USDt Wallet (SideSwap)**

Sources: `coincube-gui/src/app/state/liquid/send.rs`
- Added USDt wallet powered by SideSwap for Liquid-based stablecoin support.
- Cross-asset payments: send USDt and pay fees with USDt.
- Asset selector logic for switching between L-BTC and USDt in the send flow.

**Connect Module**

Sources: `coincube-gui/src/app/state/connect/`
- New Connect module for remote backend connectivity.
- Lightning address support.
- Avatar system with deterministic generation.

**Light/Dark Mode**

Sources: `coincube-ui/src/theme/palette.rs`, `coincube-gui/src/gui/mod.rs`
- User-selectable light and dark themes with persistence across sessions.
- Theme-aware logotype, sidebar, and widget rendering.
- Sun/moon toggle icon in sidebar.

**Border Wallet Signer**

Sources: `coincube-core/src/border_wallet/mod.rs`
- New `coincube-core` module for Border Wallet recovery phrase generation.
- Grid creation, pattern building, and enrollment derivation.
- PSBT signing integration with zeroization of secrets on drop.

**Buy/Sell Improvements**

Sources: `coincube-gui/src/app/state/buysell/`
- Improved sell UI and buy mode flow.
- Prevent double-spend for lightning fulfillment.
- Skip invoice display screen for Mavapay buy widget.
- Restore default styling for sell mode.
- Developer pay-in simulation for sell mode.
- Globally re-enable Mavapay with runtime env detection (`ENABLE_MAVAPAY`).

**Toast Notification System**

Sources: `coincube-gui/src/app/view/warning.rs`
- Migrated to global toast overlay with WCAG AA compliant severity colors (min 4.5:1 contrast).
- Chronological sorting, log level propagation, and extracted notification theme helper.

**Wallet Recovery**

Sources: `coincube-gui/src/app/state/liquid/`
- Lightning mnemonic usage for master key and protocol restore.
- Added missing recovery flow for Liquid funds.

**Node Management**

Sources: `coincube-gui/src/app/settings/mod.rs`
- Switch between Connect and local node in settings.
- Debounced bitcoind RPC polling.
- IBD-based detection for blockchain download completion.
- Automatic switch to local node after sync.

#### Fixes
- BTC URI-prefilled amount now validates balance and lightning limits.
- Fixed accordion collapse issues.
- Deduplicated code, removed dead handlers, fixed state resets.
- Fixed Mostro config path and protocol message iteration.
- Handle first poll not yet returned.
- Fixed fresh data directory detection bug.

---

### February 2026

#### Features

**Liquid Wallet**

Sources: `coincube-gui/src/app/state/liquid/`
- Added payment refund functionality for failed Lightning payments.
- Cancel button for pending operations.
- Recovery flow for Liquid wallet funds.
- BIP39 word suggestions as you type during seed recovery.
- Liquid BTC receive feature (COIN-287).
- Liquid-to-Vault transfer UI flow refinements.

**Buy/Sell**

Sources: `coincube-gui/src/app/state/buysell/`
- Region selector for Meld buy/sell.
- Copy recipient address button in Meld webview.
- Allow selecting existing address in buy/sell flow.

**Settings**

Sources: `coincube-gui/src/app/state/settings/general.rs`
- Developer mode toggle (removed unused MFA).
- Fiat price enabled by default (COIN-285).

#### Fixes
- Refundable flow error toast messages.
- Handle unsupported networks in Liquid wallet.
- Fixed rusqlite_migration build script.
- Vault send form now respects global sats/BTC display setting.

---

### January 2026

#### Features

**Liquid Wallet (formerly "Active")**

Sources: `coincube-gui/src/app/state/liquid/`
- Full send and receive flow for Liquid wallet via Breez SDK.
- Send to on-chain addresses from Liquid wallet.
- Vault ↔ Liquid bidirectional funds transfer with hardware wallet signing.
- Reusable transactions component for consistent display.
- Balance display in Global Home section.
- Loading indicators for Liquid send and receive.
- Input validation for send amounts.
- Renamed "Active" to "Liquid" across the codebase.

**Buy/Sell**

Sources: `coincube-gui/src/app/state/buysell/`
- Switched Mavapay implementation to use Coincube API backend.
- Enhanced Mavapay checkout and order confirmation UI.
- Order history view.
- Previous/back buttons in buy/sell flow.
- Reset button fix in buy/sell history and after Mavapay purchase.
- Meld UI improvements and improved error text.

**Cube Management**

Sources: `coincube-gui/src/launcher.rs`, `coincube-gui/src/app/settings/mod.rs`
- PIN confirmation required before cube deletion.
- Force sync and export functionality.
- Loading indicators for cube creation and PIN entry buttons.
- Asynchronous cube settings save with UI error display.
- Allow retrying cube save without re-running installation.

**Infrastructure**

Sources: `.github/workflows/`, `coincube-gui/src/services/`
- Promoted version to v1.0.0.
- GPG signing and DMG release for macOS.
- Coincube Esplora integration.
- Migrated SSE implementation to `reqwest-sse` for real-time event streaming.
- Passwordless auth migration.
- Toast message when address is copied under Vault receive.
- Error toasts now stack.
- Scrollable error toasts for long messages.

#### Fixes
- Fixed Breez `sign_ecdsa_recoverable`.
- Fixed PIN entry delete issue.
- Fixed vault settings bug.

---

### December 2025

#### Features

**Foundation**

Sources: `coincube-gui/src/launcher.rs`, `coincube-gui/src/app/settings/mod.rs`
- Hard fork from Liana with full rebrand to Coincube.
- Removed auto-migration (no longer supported after fork).
- Cube architecture: multi-cube launcher with per-cube settings and wallets.
- PIN entry system for cube access with UX enhancements.

**Liquid Wallet**

Sources: `coincube-gui/src/app/state/liquid/`
- Integrated Breez SDK Liquid for Lightning-enabled spending wallet.
- Data fetching from Breez SDK.
- Home page setup with balance overview.
- Confirm transfer view for Vault ↔ Liquid movements.

**Buy/Sell**

Sources: `coincube-gui/src/app/state/buysell/`
- Mavapay integration moved to main buy/sell panel.
- Inline Mavapay client functions.
- User logout from buy/sell panel.
- Country symbol display and reference labels for quotes/orders.

**GUI**

Sources: `coincube-gui/Cargo.toml`, `coincube-ui/src/`
- Upgraded Iced to 0.14.0.
- Validation hint messages on spend transaction view.
- Active settings page view.
- Deprecated iced_wry and Onramper.

#### Fixes
- Fixed assigning remote wallet to cube.
- De-duplicated cube create logic.
- Added missing signer definitions.
- Fixed installer bugs and formatting.

---

### August–November 2025 (Pre-Rebrand Foundation)

Coincube-specific development began in August 2025 while still carrying the Liana name internally.
This period laid the groundwork for the full rebrand in December.

#### Features

**Buy/Sell Platform**

Sources: `coincube-gui/src/app/state/buysell/`
- Meld buy/sell integration via embedded CEF-based webview.
- Mavapay payment flow with webview checkout.
- Onramper integration for international users.
- Geolocation-based provider routing (country/ISO code detection, manual fallback).
- Native login forms and account type selection for non-webview builds.
- Runtime feature detection replacing compile-time feature flags (`dev-coincube`, `dev-meld`, `dev-onramp`).
- Fiat amount validation and currency converters.

**Webview Engine**
- CEF-based webview for embedded buy/sell flows.
- Advanced webview interface with performance tuning (reduced framerate, memory usage, tickrate).
- Webview fallback rendering for unsupported platforms.

**Fiat Price**

Sources: `coincube-gui/src/app/state/settings/general.rs`
- Fiat price display on home page with configurable caching.
- Fiat amount editing on the send page with validation.
- Global fiat price cache shared across panels.

**Coincube API Integration**

Sources: `coincube-gui/src/services/coincube/`
- Folded geolocation service into unified Coincube service.
- Folded registration service into Coincube service.
- Coincube API client for buy/sell backend operations.

**Security**

Sources: `coincube-gui/src/app/settings/mod.rs`
- PIN encryption with improved storage.
- Encrypted descriptor backup and import (BIP draft).

**GUI**

Sources: `coincube-gui/src/app/view/mod.rs`, `coincube-ui/src/`
- Accordion-based sidebar with expandable sections (Vault, Liquid, Marketplace, P2P, USDt, Connect).
- Global Home dashboard showing combined Vault and Liquid balances.
- Home page auto-refresh every 10 seconds and on scroll-to-top.
- Windows application icon.

**Release Infrastructure**

Sources: `.github/workflows/`
- GitHub Actions release pipeline.
- Windows MSI installer via `cargo-wix`.
- macOS `.app` bundle packaging.
- Linux dependency management and CI setup.

#### Fixes
- Reduced webview memory usage and rendering lag.
- Fixed encrypted backup sanitization before writing to disk.
- Manual country selection fallback when geolocation fails.

---

> **Heritage:** Coincube's vault daemon is built on [Liana](https://github.com/wizardsardine/liana)
> (v0.2–v13.0) by Wizardsardine. The original Liana changelog is preserved in `CHANGELOG_LIANA.md`.
