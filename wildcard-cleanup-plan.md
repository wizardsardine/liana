# CoinCube Code Quality Cleanup Plan

**Project**: coincube  
**Total Issues**: 506 (342 CRITICAL, 164 lower severity)  
**Focus Areas**: Wildcard Imports (S2208), Cognitive Complexity (S3776)

---

## Phase 1: State Files - Wildcard Imports (10 files)

**Rule**: `rust:S2208` - Replace wildcard imports with specific imports  
**Pattern**: `use coincube_ui::widget::*;` → `use coincube_ui::widget::Element;`

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 1 | `app/state/buysell.rs` | - | `widget::*` | `widget::Element` |
| 2 | `app/state/global_home.rs` | - | `widget::*` | `widget::Element` |
| 3 | `app/state/message.rs` | - | `widget::*` | `widget::Element` |
| 4 | `app/state/mod.rs` | - | `widget::*` | `widget::Element` |
| 5 | `app/state/liquid/mod.rs` | - | `widget::*` | `widget::Element` |
| 6 | `app/state/liquid/overview.rs` | - | `widget::*` | `widget::Element` |
| 7 | `app/state/liquid/receive.rs` | - | `widget::*` | `widget::Element` |
| 8 | `app/state/liquid/send.rs` | - | `widget::*` | `widget::Element` |
| 9 | `app/state/liquid/settings.rs` | - | `widget::*` | `widget::Element` |
| 10 | `app/state/liquid/transactions.rs` | 10 | `widget::*` | `widget::Element` |

**Verification**: `cargo check -p coincube-gui`

---

## Phase 2: Vault State Files (4 files)

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 11 | `app/state/vault/overview.rs` | 6 | `widget::*` | `widget::Element` |
| 12 | `app/state/vault/receive.rs` | 9 | `widget::*` | `widget::Element` |
| 13 | `app/state/vault/spend/mod.rs` | 10 | `widget::*` | `widget::Element` |
| 14 | `app/state/vault/transactions.rs` | 10 | `widget::*` | `widget::Element` |

---

## Phase 3: Spark State Files (2 files)

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 15 | `app/state/spark/transactions.rs` | 122 | `widget::*` | `widget::Element` |
| 16 | `app/state/spark/receive.rs` | - | `widget::*` | `widget::Element` |

---

## Phase 4: Connect Views (2 files)

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 17 | `app/view/connect/mod.rs` | 6, 9 | `widget::*` | `widget::Element` |
| 18 | `app/view/connect/contacts.rs` | - | `widget::*` | `widget::Element` |

---

## Phase 5: Liquid Views (8 files)

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 19 | `app/view/liquid/overview.rs` | 13 | `widget::*` | `widget::Element` |
| 20 | `app/view/liquid/receive.rs` | - | `widget::*` | `widget::Element` |
| 21 | `app/view/liquid/send.rs` | - | `widget::*` | `widget::{Column, Row, Element}` |
| 22 | `app/view/liquid/settings.rs` | 6 | `widget::*` | `widget::Element` |
| 23 | `app/view/liquid/sideshift_receive.rs` | - | `widget::*` | `widget::Element` |
| 24 | `app/view/liquid/sideshift_send.rs` | - | `widget::*` | `widget::Element` |
| 25 | `app/view/liquid/transactions.rs` | - | `widget::*` | `widget::Element` |

---

## Phase 6: Spark Views (4 files)

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 26 | `app/view/spark/overview.rs` | 18, 20, 25 | `widget::*` | `widget::Element` |
| 27 | `app/view/spark/transactions.rs` | 29, 35 | `widget::*` | `widget::Element` |
| 28 | `app/view/spark/receive.rs` | - | `widget::*` | `widget::Element` |
| 29 | `app/view/spark/send.rs` | - | `widget::*` | `widget::Element` |

---

## Phase 7: P2P Views (4 files)

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 30 | `app/view/p2p/panel.rs` | - | `widget::*` | `widget::Element` |
| 31 | `app/view/p2p/components/order_card.rs` | - | `widget::*` | `widget::Element` |
| 32 | `app/view/p2p/components/order_filters.rs` | - | `widget::*` | `widget::Element` |
| 33 | `app/view/p2p/components/trade_card.rs` | - | `widget::*` | `widget::Element` |
| 34 | `app/view/p2p/mostro.rs` | - | `widget::*` | `widget::Element` |

---

## Phase 8: Vault Views (13 files)

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 35 | `app/view/vault/coins.rs` | - | `widget::*` | `widget::Element` |
| 36 | `app/view/vault/hw.rs` | - | `widget::*` | `widget::Element` |
| 37 | `app/view/vault/label.rs` | - | `widget::*` | `widget::Element` |
| 38 | `app/view/vault/overview.rs` | - | `widget::*` | `widget::Element` |
| 39 | `app/view/vault/psbt.rs` | - | `widget::*` | `widget::Element` |
| 40 | `app/view/vault/psbts.rs` | - | `widget::*` | `widget::Element` |
| 41 | `app/view/vault/receive.rs` | - | `widget::*` | `widget::Element` |
| 42 | `app/view/vault/recovery.rs` | - | `widget::*` | `widget::Element` |
| 43 | `app/view/vault/settings/mod.rs` | - | `widget::*` | `widget::Element` |
| 44 | `app/view/vault/settings/general.rs` | - | `widget::*` | `widget::Element` |
| 45 | `app/view/vault/spend/mod.rs` | - | `widget::*` | `widget::Element` |
| 46 | `app/view/vault/transactions.rs` | - | `widget::*` | `widget::Element` |
| 47 | `app/view/vault/warning.rs` | - | `widget::*` | `widget::Element` |

---

## Phase 9: Settings Views (5 files)

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 48 | `app/view/settings/mod.rs` | - | `widget::*` | `widget::Element` |
| 49 | `app/view/settings/about.rs` | - | `widget::*` | `widget::Element` |
| 50 | `app/view/settings/general.rs` | - | `widget::*` | `widget::Element` |
| 51 | `app/view/settings/install_stats.rs` | - | `widget::*` | `widget::Element` |
| 52 | `app/view/settings/lightning.rs` | 16, 18, 23 | `widget::*` | `widget::Element` |
| 53 | `app/view/settings/backup.rs` | - | `widget::*` | `widget::Element` |

---

## Phase 10: Buy/Sell & Global Views (3 files)

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 54 | `app/view/buysell/panel.rs` | 12 | `widget::*` | `widget::Element` |
| 55 | `app/view/global_home.rs` | - | `widget::*` | `widget::Element` |
| 56 | `app/view/mod.rs` | - | `widget::*` | `widget::Element` |

---

## Phase 11: Core/UI Files (5 files)

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 57 | `coincube-ui/src/component/card.rs` | 1 | `*` (CLOSED) | Already fixed |
| 58 | `coincube-ui/src/component/mod.rs` | 22 | `*` (CLOSED) | Already fixed |
| 59 | `coincube-ui/src/component/notification.rs` | 7 | `*` (CLOSED) | Already fixed |
| 60 | `coincube-ui/src/component/tooltip.rs` | 1 | `*` (CLOSED) | Already fixed |
| 61 | `app/message.rs` | 19 | `widget::*` | `widget::Element` |

---

## Phase 12: Installer Files (8 files)

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 62 | `installer/view/mod.rs` | 30 | `widget::*` | `widget::Element` |
| 63 | `installer/view/editor/mod.rs` | - | `widget::*` | `widget::Element` |
| 64 | `installer/step/mod.rs` | - | `widget::*` | `widget::Element` |
| 65 | `installer/step/coincube_connect.rs` | - | `widget::*` | `widget::Element` |
| 66 | `installer/step/node/bitcoind.rs` | - | `widget::*` | `widget::Element` |
| 67 | `installer/step/node/electrum.rs` | - | `widget::*` | `widget::Element` |
| 68 | `installer/step/node/esplora.rs` | - | `widget::*` | `widget::Element` |
| 69 | `installer/step/wallet_alias.rs` | - | `widget::*` | `widget::Element` |

---

## Phase 13: Other GUI Files (3 files)

| # | File | Line | Current Import | Replacement |
|---|------|------|----------------|-------------|
| 70 | `gui/pane.rs` | - | `widget::*` | `widget::Element` |
| 71 | `gui/tab.rs` | - | `widget::*` | `widget::Element` |
| 72 | `launcher.rs` | - | `widget::*` | `widget::Element` |

---

## Phase 14: Cognitive Complexity - Critical Functions (S3776)

**Rule**: `rust:S3776` - Refactor functions with cognitive complexity > 15

| Priority | File | Line | Function | Current Complexity |
|----------|------|------|----------|-------------------|
| 🔴 P1 | `app/state/buysell.rs` | 33 | Unknown | **150** |
| 🔴 P1 | `app/view/p2p/panel.rs` | 1270 | Unknown | **149** |
| 🟡 P2 | `app/state/settings/general.rs` | 219 | Unknown | 52 |
| 🟡 P2 | `app/view/p2p/panel.rs` | 883 | Unknown | 54 |
| 🟡 P2 | `app/view/p2p/panel.rs` | 665 | Unknown | 62 |
| 🟡 P2 | `gui/tab.rs` | 171 | Unknown | 96 |
| 🟢 P3 | `app/view/connect/mod.rs` | 963 | Unknown | 18 |
| 🟢 P3 | `app/view/connect/mod.rs` | 1312 | Unknown | 40 |
| 🟢 P3 | `app/view/connect/mod.rs` | 1610 | Unknown | 22 |
| 🟢 P3 | `launcher.rs` | 2172 | Unknown | 21 |
| 🟢 P3 | `app/state/spark/receive.rs` | 171 | Unknown | 31 |
| 🟢 P3 | `app/view/spark/overview.rs` | 121 | Unknown | 27 |
| 🟢 P3 | `app/state/connect/cube.rs` | 341 | Unknown | 79 |
| 🟢 P3 | `app/state/connect/cube.rs` | 141 | Unknown | 31 |

---

## Phase 15: Python Test Files (Optional)

**Rule**: `python:S2208` - Wildcard imports in Python

| # | File | Line | Issue |
|---|------|------|-------|
| 73 | `tests/test_spend.py` | 3 | Import only needed names |
| 74 | `tests/test_chain.py` | 3 | Import only needed names |
| 75 | `tests/test_rpc.py` | 6 | Import only needed names |
| 76 | `tests/test_misc.py` | 6 | Import only needed names |

---

## Verification Commands

After each phase:
```bash
cargo check -p coincube-gui
```

After all phases:
```bash
cargo check -p coincube-gui --all-features
cargo clippy -p coincube-gui -- -D warnings
```

---

## Total Files: ~76 files across 15 phases

**Ready to proceed? Confirm which phase to start with.**
