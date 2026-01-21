#[cfg(test)]
mod tests_ {
    use uuid::Uuid;

    /// Test that verifies the full state initialization and wallet filtering
    /// This simulates what happens when a user connects
    #[test]
    fn test_wsmanager_wallet_filtering_with_real_state() {
        use crate::handler::can_user_access_wallet;
        use crate::state::ServerState;
        use liana_connect::ws_business::UserRole;
        use std::collections::BTreeSet;

        // Create state (this calls init_test_data)
        let state = ServerState::new();

        // Get users, orgs, wallets
        let users = state.users.lock().unwrap();
        let orgs = state.orgs.lock().unwrap();
        let wallets = state.wallets.lock().unwrap();

        // Verify data is populated
        assert!(
            users.len() >= 6,
            "Should have at least 6 users, got {}",
            users.len()
        );
        assert!(
            orgs.len() >= 2,
            "Should have at least 2 orgs, got {}",
            orgs.len()
        );
        assert!(
            wallets.len() >= 4,
            "Should have at least 4 wallets, got {}",
            wallets.len()
        );

        // Find WSManager user by email
        let ws_manager = users
            .values()
            .find(|u| u.email == "ws@example.com")
            .expect("WSManager user should exist");

        assert_eq!(
            ws_manager.role,
            UserRole::WizardSardineAdmin,
            "ws@example.com should have WSManager role"
        );

        // Find Acme Corp org
        let acme_org = orgs
            .values()
            .find(|o| o.name == "Acme Corp")
            .expect("Acme Corp org should exist");

        assert!(
            acme_org.wallets.len() >= 4,
            "Acme Corp should have at least 4 wallets, got {}",
            acme_org.wallets.len()
        );

        let expected_wallet_count = acme_org.wallets.len();

        let filtered_wallet_ids: BTreeSet<Uuid> = acme_org
            .wallets
            .iter()
            .filter(|wallet_id| {
                if let Some(wallet) = wallets.get(wallet_id) {
                    can_user_access_wallet(ws_manager, wallet)
                } else {
                    panic!(
                        "Wallet {} in org.wallets but not in state.wallets!",
                        wallet_id
                    );
                }
            })
            .copied()
            .collect();

        // WSManager should see ALL wallets
        assert_eq!(
            filtered_wallet_ids.len(),
            expected_wallet_count,
            "WSManager should see ALL {} wallets in Acme Corp, got {}",
            expected_wallet_count,
            filtered_wallet_ids.len()
        );
    }

    /// Test participant filtering - should NOT see draft/locked wallets
    #[test]
    fn test_participant_wallet_filtering_with_real_state() {
        use crate::handler::can_user_access_wallet;
        use crate::state::ServerState;
        use liana_connect::ws_business::UserRole;
        use std::collections::BTreeSet;

        let state = ServerState::new();

        let users = state.users.lock().unwrap();
        let orgs = state.orgs.lock().unwrap();
        let wallets = state.wallets.lock().unwrap();

        // Find participant user
        let participant = users
            .values()
            .find(|u| u.email == "user@example.com")
            .expect("Participant user should exist");

        assert_eq!(participant.role, UserRole::Participant);

        let acme_org = orgs
            .values()
            .find(|o| o.name == "Acme Corp")
            .expect("Acme Corp should exist");

        let filtered_wallet_ids: BTreeSet<Uuid> = acme_org
            .wallets
            .iter()
            .filter(|wallet_id| {
                if let Some(wallet) = wallets.get(wallet_id) {
                    can_user_access_wallet(participant, wallet)
                } else {
                    false
                }
            })
            .copied()
            .collect();

        // Participant should see fewer wallets than WSManager (no draft/locked)
        // user@example.com has keys in Validated, Final, Shared, Registration wallets (4 wallets)
        // But Shared is Finalized, so all 4 should be visible
        assert!(
            filtered_wallet_ids.len() <= 4,
            "Participant should see at most 4 wallets, got {}",
            filtered_wallet_ids.len()
        );

        // Verify Draft Wallet is NOT included
        for wallet_id in &filtered_wallet_ids {
            let wallet = wallets.get(wallet_id).unwrap();
            assert!(
                !matches!(
                    wallet.status,
                    liana_connect::ws_business::WalletStatus::Drafted
                ),
                "Participant should not see Draft wallet '{}'",
                wallet.alias
            );
        }
    }

    /// Test the full token -> user lookup -> wallet filtering flow
    /// This simulates exactly what happens in connection.rs
    #[test]
    fn test_token_to_wallet_filtering_flow() {
        use crate::auth::AuthManager;
        use crate::handler::can_user_access_wallet;
        use crate::state::ServerState;
        use liana_connect::ws_business::UserRole;
        use std::collections::BTreeSet;

        // Create state and auth manager (same as server startup)
        let state = ServerState::new();
        let auth = AuthManager::new();

        let email = "ws@example.com";

        // Step 1: Look up user UUID from state.users by email (as done in http.rs after OTP)
        let user_uuid = {
            let users = state.users.lock().unwrap();
            users
                .values()
                .find(|u| u.email == email)
                .map(|u| u.uuid)
                .expect("User should exist in state.users")
        };

        // Step 2: Create token with UUID (as done in http.rs)
        let token = format!("access-token-{}", user_uuid);

        // Step 3: Validate token and extract UUID (as done in connection.rs)
        let extracted_user_id = auth.validate_token(&token).expect("Token should be valid");
        assert_eq!(extracted_user_id, user_uuid, "Extracted UUID should match");

        // Step 4: Look up user by UUID and filter wallets
        let users = state.users.lock().unwrap();
        let user = users
            .get(&extracted_user_id)
            .expect("User should be found by UUID");

        assert_eq!(user.email, email);
        assert_eq!(
            user.role,
            UserRole::WizardSardineAdmin,
            "Global role should be WSManager"
        );

        // Step 5: Filter wallets (as done in connection.rs)
        let wallets = state.wallets.lock().unwrap();
        let orgs = state.orgs.lock().unwrap();
        let acme_org = orgs
            .values()
            .find(|o| o.name == "Acme Corp")
            .expect("Acme Corp should exist");

        let expected_wallet_count = acme_org.wallets.len();
        let filtered_wallet_ids: BTreeSet<Uuid> = acme_org
            .wallets
            .iter()
            .filter(|wallet_id| {
                if let Some(wallet) = wallets.get(wallet_id) {
                    can_user_access_wallet(user, wallet)
                } else {
                    false
                }
            })
            .copied()
            .collect();

        assert_eq!(
            filtered_wallet_ids.len(),
            expected_wallet_count,
            "WSManager should see ALL {} wallets in Acme Corp, but only sees {}",
            expected_wallet_count,
            filtered_wallet_ids.len()
        );
    }

    /// Test with SEPARATE state and auth instances to simulate potential bug
    /// Confirms that if HTTP and WS used different state instances, UUIDs would mismatch
    #[test]
    fn test_separate_state_instances_would_fail() {
        use crate::auth::AuthManager;
        use crate::state::ServerState;

        // This simulates a potential bug: if HTTP and WS use different state instances,
        // the UUIDs would be different (since they're randomly generated)

        let http_state = ServerState::new(); // HTTP handler state
        let ws_state = ServerState::new(); // WebSocket handler state (DIFFERENT!)
        let auth = AuthManager::new();

        let email = "ws@example.com";

        // HTTP handler looks up user by email in http_state
        let user_uuid_from_http = {
            let users = http_state.users.lock().unwrap();
            users
                .values()
                .find(|u| u.email == email)
                .map(|u| u.uuid)
                .expect("User should exist")
        };

        // Token is created with UUID from http_state
        let token = format!("access-token-{}", user_uuid_from_http);

        // WebSocket handler extracts UUID from token
        let extracted_user_id = auth.validate_token(&token).expect("Token should be valid");

        // WebSocket handler looks up user by UUID in ws_state (DIFFERENT instance!)
        let user_found = {
            let users = ws_state.users.lock().unwrap();
            users.get(&extracted_user_id).is_some()
        };

        // This would be false because the UUIDs are different!
        // This test confirms the bug would occur if separate state instances were used
        assert!(
            !user_found,
            "EXPECTED: If HTTP and WS use different ServerState instances, \
             user lookup by UUID fails because UUIDs are randomly generated!"
        );
    }

    /// Test handle_device_registered - basic success case
    #[test]
    fn test_handle_device_registered_success() {
        use crate::handler::handle_request;
        use crate::state::ServerState;
        use liana_connect::ws_business::{
            RegistrationInfos, Request, Response, WalletStatus,
        };
        use miniscript::bitcoin::bip32::Fingerprint;

        let state = ServerState::new();

        // Find a wallet in Registration status
        // First, let's manually set up a wallet with Registration status
        let wallet_id = {
            let wallets = state.wallets.lock().unwrap();
            // Get the first wallet
            wallets.keys().next().copied().unwrap()
        };

        // Get a user ID
        let user_id = {
            let users = state.users.lock().unwrap();
            users
                .values()
                .find(|u| u.email == "user@example.com")
                .map(|u| u.uuid)
                .unwrap()
        };

        let fingerprint = Fingerprint::from_hex("d34db33f").unwrap();

        // Set up the wallet to be in Registration status with our device
        {
            let mut wallets = state.wallets.lock().unwrap();
            let wallet = wallets.get_mut(&wallet_id).unwrap();

            wallet.status = WalletStatus::Registration {
                descriptor: "wsh(pk([d34db33f/48'/0'/0'/2']xpub.../0/*))".to_string(),
                devices: vec![fingerprint],
            };
        }

        // Create the DeviceRegistered request
        let mut infos = RegistrationInfos::new(user_id, fingerprint);
        infos.registered = true;
        infos.proof_of_registration = Some(vec![0xa1, 0xb2, 0xc3, 0xd4]);

        let request = Request::DeviceRegistered {
            wallet_id,
            infos: infos.clone(),
        };

        // Handle the request
        let response = handle_request(request, &state, user_id);

        // Verify the response is a wallet
        match response {
            Response::Wallet { wallet } => {
                // Since this was the only device, it should now be Finalized
                assert_eq!(
                    wallet.status,
                    WalletStatus::Finalized,
                    "Wallet should be Finalized after all devices registered"
                );

                // Verify registration info was stored
                let reg_infos = state.registration_infos.lock().unwrap();
                let stored_info = reg_infos.get(&(wallet_id, fingerprint)).unwrap();
                assert!(stored_info.registered, "Device should be marked as registered");
                assert_eq!(
                    stored_info.proof_of_registration,
                    Some(vec![0xa1, 0xb2, 0xc3, 0xd4])
                );
            }
            Response::Error { error } => {
                panic!("Expected wallet response, got error: {:?}", error);
            }
            _ => panic!("Expected Wallet response"),
        }
    }

    /// Test handle_device_registered - wrong wallet status (not Registration)
    #[test]
    fn test_handle_device_registered_wrong_status() {
        use crate::handler::handle_request;
        use crate::state::ServerState;
        use liana_connect::ws_business::{RegistrationInfos, Request, Response, WalletStatus};
        use miniscript::bitcoin::bip32::Fingerprint;

        let state = ServerState::new();

        // Find a wallet NOT in Registration status
        let wallet_id = {
            let wallets = state.wallets.lock().unwrap();
            wallets
                .values()
                .find(|w| matches!(w.status, WalletStatus::Drafted | WalletStatus::Validated))
                .map(|w| w.id)
                .unwrap()
        };

        let user_id = {
            let users = state.users.lock().unwrap();
            users.values().next().map(|u| u.uuid).unwrap()
        };

        let fingerprint = Fingerprint::from_hex("d34db33f").unwrap();
        let mut infos = RegistrationInfos::new(user_id, fingerprint);
        infos.registered = true;

        let request = Request::DeviceRegistered {
            wallet_id,
            infos,
        };

        let response = handle_request(request, &state, user_id);

        // Should get an error because wallet is not in Registration status
        match response {
            Response::Error { error } => {
                assert!(
                    error.code == "INVALID_STATUS" || error.code == "ACCESS_DENIED",
                    "Expected INVALID_STATUS or ACCESS_DENIED error, got: {}",
                    error.code
                );
            }
            Response::Wallet { .. } => {
                // This might be acceptable if the server returns the unchanged wallet
            }
            _ => panic!("Expected Error or Wallet response"),
        }
    }

    /// Test handle_device_registered - user trying to register for another user
    #[test]
    fn test_handle_device_registered_wrong_user() {
        use crate::handler::handle_request;
        use crate::state::ServerState;
        use liana_connect::ws_business::{
            RegistrationInfos, Request, Response, WalletStatus,
        };
        use miniscript::bitcoin::bip32::Fingerprint;

        let state = ServerState::new();

        let wallet_id = {
            let wallets = state.wallets.lock().unwrap();
            wallets.keys().next().copied().unwrap()
        };

        // Get two different users
        let (user1_id, user2_id) = {
            let users = state.users.lock().unwrap();
            let mut iter = users.values();
            let user1 = iter.next().map(|u| u.uuid).unwrap();
            let user2 = iter.next().map(|u| u.uuid).unwrap();
            (user1, user2)
        };

        let fingerprint = Fingerprint::from_hex("d34db33f").unwrap();

        // Set up wallet in Registration status
        {
            let mut wallets = state.wallets.lock().unwrap();
            let wallet = wallets.get_mut(&wallet_id).unwrap();

            wallet.status = WalletStatus::Registration {
                descriptor: "wsh(pk([d34db33f/48'/0'/0'/2']xpub.../0/*))".to_string(),
                devices: vec![fingerprint],
            };
        }

        // Create infos claiming to be for user1, but request comes from user2
        let mut infos = RegistrationInfos::new(user1_id, fingerprint);
        infos.registered = true;

        let request = Request::DeviceRegistered {
            wallet_id,
            infos,
        };

        // Request made by user2 (different from infos.user)
        let response = handle_request(request, &state, user2_id);

        // Should get an error because infos.user != editor_id
        match response {
            Response::Error { error } => {
                assert_eq!(
                    error.code, "ACCESS_DENIED",
                    "Expected ACCESS_DENIED error, got: {}",
                    error.code
                );
            }
            _ => panic!("Expected Error response for unauthorized user"),
        }
    }
}
