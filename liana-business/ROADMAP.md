# ROADMAP

## Priority

- [x] **2. Back**
  - [x] 2.1 Auth Client
  - [x] 2.2 Installer Trait Integration
- [ ] **3.1 Ws Manager Flow**
- [ ] **1. Front** (needed for completion of WS Manager flow)
  - [ ] 1.1 Key Management Panel
  - [ ] 1.2 Path Management Panel
- [ ] **3.2 Owner Flow**
- [ ] **3.3 Participant**

## 1. Front

### 1.1 Key Management Panel
- [ ] Finalize key management panel
  - [ ] Complete UI implementation
  - [ ] Ensure all key operations are functional
  - [ ] Polish user experience

### 1.2 Path Management Panel
- [ ] Finalize path management panel
  - [ ] Complete UI implementation
  - [ ] Ensure all path operations are functional
  - [ ] Polish user experience

## 2. Back ✓

### 2.1 Auth Client
- [x] Implement auth client
  - [x] Export required auth types from liana-gui (AuthClient, AuthError,
  AccessTokenResponse, cache types, http traits, NetworkDirectory, get_service_config)
  - [x] Add liana-gui dependency to liana-business Cargo.toml
  - [x] Extend Client struct with auth_client, network, network_dir, email fields
  - [x] Implement auth_request() using AuthClient::sign_in_otp() with async-to-sync
  bridge (spawn thread + block_on)
  - [x] Implement auth_code() using AuthClient::verify_otp() with token caching via
  update_connect_cache()
  - [x] Implement token caching in connect_ws() - check cached token, refresh if
  expired, use cached token for connection

### 2.2 Installer Trait Integration
- [x] Wrap complete app under the Installer trait of liana-gui
  - [x] Implement Installer trait for the application (BusinessInstaller in business-installer crate)
  - [x] Support standalone mode (liana-business wraps BusinessInstaller)
  - [x] Support integration into liana-gui (via Installer trait interface)

### 2.3 Auth improvements
- [ ] Automatically refresh token
- [ ] Async instead threading?

## 3. Flows

### 3.1 Ws Manager Flow
- [ ] Ws Manager flow implementation
  - [ ] Define complete workflow for Workspace Manager role
  - [ ] Implement all required functionality
  - [ ] Testing and validation

### 3.2 Owner Flow
- [ ] Owner flow implementation
  - [ ] Clearly define what owner is allowed to do
  - [ ] Document owner permissions and capabilities
  - [ ] Implement owner-specific functionality
  - [ ] Testing and have a complete functional flow

### 3.3 Participant
- [ ] Participant flow implementation
  - [ ] Participant flow should be quite limited:
    - [ ] Connect
    - [ ] Add/edit an xpub
  - [ ] Implement participant-specific functionality
  - [ ] Testing and have a complete functional flow

## Bugfixes

## Changelog


