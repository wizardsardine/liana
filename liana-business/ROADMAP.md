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
- [x] **2. Settings Trait Design** *(Phase 1: Data Layer Only - INCOMPLETE)*
  - [x] 2.1 Define Settings traits in liana-gui
    - [x] Create `SettingsTrait` with common operations (load, wallets)
    - [x] Create `WalletSettingsTrait` for wallet-level settings
    - [x] Add associated type for wallet settings
  - [x] 2.2 Implement LianaSettings in liana-gui
    - [x] Rename existing `Settings` struct to `LianaSettings`
    - [x] Rename existing `WalletSettings` struct to `LianaWalletSettings`
    - [x] Implement `SettingsTrait` for `LianaSettings`
    - [x] Implement `WalletSettingsTrait` for `LianaWalletSettings`
    - [x] Add type aliases for backward compatibility
  - [x] 2.3 Make GUI framework generic over Settings
    - [x] Add `S: SettingsTrait` to `GUI<I, S, M>`
    - [x] Update `LianaGUI` type alias to use `LianaSettings`
    - ~~Note: `App` stays non-generic~~ **ISSUE: This was incorrect - see Phase 2**
  - [x] 2.4 Propagate S to Tab/Pane
    - [x] Add `S: SettingsTrait` to `Tab<I, S, M>`, `State<I, S, M>`
    - [x] Add `S: SettingsTrait` to `Pane<I, S, M>`
    - Note: S only threaded via PhantomData, not actually used for UI
  - [x] 2.5 Create BusinessSettings in business-settings crate
    - [x] Define `BusinessSettings` struct implementing `SettingsTrait`
    - [x] Define `BusinessWalletSettings` implementing `WalletSettingsTrait`
      - NO `start_internal_bitcoind` field
  - [x] 2.6 Verify builds
    - [x] `cargo build -p liana-gui` passes
    - [x] `cargo build -p business-installer` passes
- [x] **2.7 Settings UI Trait Design** *(Phase 2: UI Layer)*
  - **Problem**: Phase 1 only abstracted data, not UI. `SettingsState` remained hardcoded
    with Liana-specific panels (bitcoind, electrum, rescan) that don't apply to business.
  - **Solution**: Follow `Installer` trait pattern with `SettingsUI` trait
  - [x] 2.7.1 Define `SettingsUI` trait in liana-gui
    - [x] Create `SettingsUI<Message>` trait with update/view/subscription methods
    - [x] Add `type Message` and `type UI` associated types to `SettingsTrait`
    - Location: `liana-gui/src/app/settings/ui.rs`
  - [x] 2.7.2 Implement `LianaSettingsUI` in liana-gui
    - [x] Rename `SettingsState` to `LianaSettingsUI`
    - [x] Implement `SettingsUI<app::Message>` for `LianaSettingsUI`
    - [x] Keep backward-compatible `State` trait impl
    - [x] Update `LianaSettings` to specify `type UI = LianaSettingsUI`
    - Location: `liana-gui/src/app/state/settings/mod.rs`
  - [x] 2.7.3 Create `BusinessSettingsUI` in business-settings
    - [x] Create blank/minimal `BusinessSettingsUI` struct
    - [x] Implement `SettingsUI<BusinessSettingsMessage>` trait
    - [x] Define `BusinessSettingsMessage` enum (minimal for now)
    - [x] Update `BusinessSettings` to specify `type UI = BusinessSettingsUI`
    - [x] Use monostate pattern (like business-installer), NOT `Box<dyn State>`
    - Location: `liana-business/business-settings/src/ui.rs`
  - [x] 2.7.4 Verify builds
    - [x] `cargo build -p liana-gui` passes
    - [x] `cargo build -p business-settings` passes
- [x] **2.8 Make App Generic** *(Phase 3: Full Integration)*
  - [x] Make `App<S: SettingsTrait>` generic over settings
  - [x] Update `State::App(App)` in tab.rs to use generic `App<S>`
  - [x] Add `create_app_for_remote_backend` method to `SettingsTrait`
    - Default returns `None`, `LianaSettings` provides implementation
    - Handles Login → App transition in type-safe way without unsafe code
  - [x] Move remote backend App creation logic to `settings/mod.rs`
  - [x] Add `State` as bound on `SettingsTrait::UI`
  - [x] Update `BusinessSettingsUI` to implement `State` trait
  - [x] Verify builds pass

## Planned

- [ ] **1.5 Full GUI Integration** *(depends on Section 2.8)*
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
  - [ ] 1.5.3 Verify build with `cargo build`
  - [ ] 1.5.4 Test full app flow (Launcher → Installer → Loader → App)

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

