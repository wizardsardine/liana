# ROADMAP

This file tracks high-level milestones for liana-business wrapper.
For detailed task tracking, see `business-installer/ROADMAP.md`.

## Completed

- [x] **1. Installer Integration**
  - [x] 1.1 Wrap liana-gui into liana-business and use business-installer as installer
  - [x] 1.2 BusinessInstaller implements `Installer` trait from liana-gui

## In Progress

- [ ] **2. Settings Trait Design**
  - [ ] 2.1 Define Settings trait in liana-gui
  - [ ] 2.2 Create LianaSettings implementation
  - [ ] 2.3 Create BusinessSettings implementation

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

