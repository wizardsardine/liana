# CoinCube Code Quality Cleanup Plan - UPDATED

**Project**: coincube  
**Total Files with Wildcard Imports**: 61 (as of latest scan)  
**Rule**: `rust:S2208` - Replace `use coincube_ui::widget::*;` with explicit imports

---

## Phase 1: State Files - Core & Liquid (10 files)

| # | File | Line | Status |
|---|------|------|--------|
| 1 | `app/state/mod.rs` | - | `widget::*` |
| 2 | `app/state/global_home.rs` | - | `widget::*` |
| 3 | `app/state/liquid/overview.rs` | - | `widget::*` |
| 4 | `app/state/liquid/receive.rs` | - | `widget::*` |
| 5 | `app/state/liquid/send.rs` | - | `widget::*` |
| 6 | `app/state/liquid/settings.rs` | - | `widget::*` |
| 7 | `app/state/liquid/sideshift_receive.rs` | - | `widget::*` |
| 8 | `app/state/liquid/sideshift_send.rs` | - | `widget::*` |
| 9 | `app/state/liquid/transactions.rs` | - | `widget::*` |

---

## Phase 2: State Files - Vault (2 files)

| # | File | Line | Status |
|---|------|------|--------|
| 10 | `app/state/vault/overview.rs` | 6 | `widget::*` |
| 11 | `app/state/vault/receive.rs` | 9 | `widget::*` |

*Note: `spend/mod.rs` and `transactions.rs` already clean*

---

## Phase 3: View Files - Vault (12 files)

| # | File | Line | Status |
|---|------|------|--------|
| 12 | `app/view/vault/coins.rs` | 10 | `widget::*` |
| 13 | `app/view/vault/hw.rs` | - | `widget::*` |
| 14 | `app/view/vault/label.rs` | - | `widget::*` |
| 15 | `app/view/vault/overview.rs` | - | `widget::*` |
| 16 | `app/view/vault/psbt.rs` | - | `widget::*` |
| 17 | `app/view/vault/psbts.rs` | - | `widget::*` |
| 18 | `app/view/vault/receive.rs` | - | `widget::*` |
| 19 | `app/view/vault/recovery.rs` | - | `widget::*` |
| 20 | `app/view/vault/settings/mod.rs` | - | `widget::*` |
| 21 | `app/view/vault/settings/general.rs` | - | `widget::*` |
| 22 | `app/view/vault/spend/mod.rs` | - | `widget::*` |
| 23 | `app/view/vault/transactions.rs` | - | `widget::*` |
| 24 | `app/view/vault/warning.rs` | - | `widget::*` |

---

## Phase 4: View Files - Liquid (7 files)

| # | File | Line | Status |
|---|------|------|--------|
| 25 | `app/view/liquid/overview.rs` | - | `widget::*` |
| 26 | `app/view/liquid/receive.rs` | - | `widget::*` |
| 27 | `app/view/liquid/send.rs` | - | `widget::*` |
| 28 | `app/view/liquid/settings.rs` | - | `widget::*` |
| 29 | `app/view/liquid/sideshift_receive.rs` | - | `widget::*` |
| 30 | `app/view/liquid/sideshift_send.rs` | - | `widget::*` |
| 31 | `app/view/liquid/transactions.rs` | - | `widget::*` |

---

## Phase 5: View Files - Settings (6 files)

| # | File | Line | Status |
|---|------|------|--------|
| 32 | `app/view/settings/about.rs` | - | `widget::*` |
| 33 | `app/view/settings/backup.rs` | - | `widget::*` |
| 34 | `app/view/settings/general.rs` | - | `widget::*` |
| 35 | `app/view/settings/install_stats.rs` | - | `widget::*` |
| 36 | `app/view/settings/lightning.rs` | - | `widget::*` |
| 37 | `app/view/settings/mod.rs` | - | `widget::*` |

---

## Phase 6: View Files - Connect & BuySell (3 files)

| # | File | Line | Status |
|---|------|------|--------|
| 38 | `app/view/connect/contacts.rs` | - | `widget::*` |
| 39 | `app/view/connect/mod.rs` | - | `widget::*` |
| 40 | `app/view/buysell/panel.rs` | - | `widget::*` |
| 41 | `app/view/global_home.rs` | - | `widget::*` |
| 42 | `app/view/mod.rs` | - | `widget::*` |

---

## Phase 7: View Files - Spark (2 files)

| # | File | Line | Status |
|---|------|------|--------|
| 43 | `app/view/spark/overview.rs` | - | `widget::*` |
| 44 | `app/view/spark/transactions.rs` | - | `widget::*` |

---

## Phase 8: View Files - P2P (4 files)

| # | File | Line | Status |
|---|------|------|--------|
| 45 | `app/view/p2p/panel.rs` | - | `widget::*` |
| 46 | `app/view/p2p/components/order_card.rs` | - | `widget::*` |
| 47 | `app/view/p2p/components/order_filters.rs` | - | `widget::*` |
| 48 | `app/view/p2p/components/trade_card.rs` | - | `widget::*` |

---

## Phase 9: Installer Files (11 files)

| # | File | Line | Status |
|---|------|------|--------|
| 49 | `installer/step/mod.rs` | - | `widget::*` |
| 50 | `installer/step/coincube_connect.rs` | - | `widget::*` |
| 51 | `installer/step/node/bitcoind.rs` | - | `widget::*` |
| 52 | `installer/step/node/electrum.rs` | - | `widget::*` |
| 53 | `installer/step/node/esplora.rs` | - | `widget::*` |
| 54 | `installer/step/wallet_alias.rs` | - | `widget::*` |
| 55 | `installer/view/editor/mod.rs` | - | `widget::*` |
| 56 | `installer/view/editor/template/custom.rs` | - | `widget::*` |
| 57 | `installer/view/editor/template/inheritance.rs` | - | `widget::*` |
| 58 | `installer/view/editor/template/mod.rs` | - | `widget::*` |
| 59 | `installer/view/editor/template/multisig_security_wallet.rs` | - | `widget::*` |

---

## Phase 10: Other GUI Files (2 files)

| # | File | Line | Status |
|---|------|------|--------|
| 60 | `gui/pane.rs` | - | `widget::*` |
| 61 | `app/view/vault/warning.rs` | - | `widget::*` |

---

## Verification Commands

```bash
# After each phase:
cargo check -p coincube-gui

# After all phases:
cargo build --release
```

---

## Total: 61 files across 10 phases

**Last Updated**: April 18, 2026  
**Source**: `grep -r "widget::\*" src --include="*.rs" -l`
