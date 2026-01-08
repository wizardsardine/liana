# ROADMAP

This file tracks high-level milestones for liana-business wrapper.
For detailed task tracking, see `business-installer/ROADMAP.md`.

## Architectural Constraints

- **Backend**: liana-business uses ONLY Liana Connect (no bitcoind, no electrum)
- **Node settings**: Not applicable - no node configuration needed
- **Fiat prices**: May be provided by Liana Connect backend in future (not external APIs)

## Completed

- [x] **1. Installer Integration**
  - [x] 1.1 BusinessInstaller implements `Installer` trait from liana-gui
  - [x] 1.2 Basic wrapper (`PolicyBuilder`) around BusinessInstaller

## In Progress

## Planned

- [ ] **1.5 Full GUI Integration** *(depends on Section 2)*
  - Use `GUI<BusinessInstaller, BusinessSettings, Message>` instead of custom `PolicyBuilder` wrapper
  - This enables full app experience: Launcher → Installer → Loader → App panels
  - [ ] 1.5.1 Update Cargo.toml dependencies
    - Add `tokio` with signal feature (for ctrl+c handling)
    - Add `backtrace` (for panic hook)
    - Add `iced_runtime` (for window management)
    - Add other dependencies from liana-gui as needed
  - [ ] 1.5.2 Rewrite main.rs
    - Remove `PolicyBuilder` struct and impl
    - Import `GUI`, `Config` from `liana_gui::gui`
    - Create type alias: `type LianaBusiness = GUI<BusinessInstaller, BusinessSettings, Message>`
    - Add command-line argument parsing (--datadir, --network flags)
    - Set up panic hook using liana-gui pattern
    - Configure Iced settings and window (size, icon, min_size)
    - Run app via `iced::application()` with `LianaBusiness`
  - [ ] 1.5.3 Verify build with `just build`
  - [ ] 1.5.4 Test full app flow (Launcher → Installer → Loader → App)
- [ ] **2. Settings Trait Design**
  - Make GUI generic over Settings (like Installer): `GUI<I, M>` → `GUI<I, S, M>`
  - [ ] 2.1 Define Settings traits in liana-gui
    - [ ] Create `Settings` trait with common operations (load, save, wallets)
    - [ ] Create `WalletSettings` trait for wallet-level settings
    - [ ] Add associated type for wallet settings
  - [ ] 2.2 Implement LianaSettings in liana-gui
    - [ ] Rename existing `Settings` struct to `LianaSettings`
    - [ ] Rename existing `WalletSettings` struct to `LianaWalletSettings`
    - [ ] Implement `Settings` trait for `LianaSettings`
    - [ ] Implement `WalletSettings` trait for `LianaWalletSettings`
  - [ ] 2.3 Make App generic over Settings
    - [ ] Add `S: Settings` parameter to `App` struct
    - [ ] Update `Panels` to work with generic settings
    - [ ] Update settings state handling
  - [ ] 2.4 Propagate to GUI framework
    - [ ] Update `Tab<I, M>` → `Tab<I, S, M>`
    - [ ] Update `Pane<I, M>` → `Pane<I, S, M>`
    - [ ] Update `GUI<I, M>` → `GUI<I, S, M>`
    - [ ] Update `LianaGUI` type alias
  - [ ] 2.5 Create BusinessSettings in business-installer
    - [ ] Define `BusinessSettings` struct implementing `Settings`
      - Liana Connect auth only (no bitcoind/electrum)
    - [ ] Define `BusinessWalletSettings` implementing `WalletSettings`
      - NO `start_internal_bitcoind` field
    - [ ] Create `BusinessSettingsState` for settings UI
      - General (fiat - may use Liana Connect backend)
      - Wallet (aliases)
      - About
      - NO node/backend configuration UI
  - [ ] 2.6 Verify builds
    - [ ] `just build` passes for liana-gui
    - [ ] `just build` passes for liana-business

## Not Started

- [ ] **3. Reproducible Build Integration**
  - [ ] 3.1 Add liana-business to Guix build script
  - [ ] 3.2 Add liana-business to release packaging
- [ ] **4. Datadir Conflict Verification**
  - Note: Server-side conflict detection exists in business-installer (modal UI for concurrent editing)
  - [ ] Local datadir conflict detection between liana-business and liana-gui
- [ ] **5. Custom Icon**
  - [ ] 5.1 Add liana-business icon asset
- [ ] **6. Custom Theme**
  - [ ] 6.1 Implement custom theme for liana-business
  - [ ] 6.2 Integrate theme into liana-business app
  - [ ] 6.3 Custom warning pill color (amber/yellow instead of red)
    - Blocked items in business-installer/ROADMAP.md:
      - 1.9.1 Badge colors (Set Keys, Not Set)
      - 1.9.4 Locked label amber color
  - Note: Theme framework exists in liana-ui, customization not applied

