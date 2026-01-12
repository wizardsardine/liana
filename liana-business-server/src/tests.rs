#[cfg(test)]
mod tests_ {
    use uuid::Uuid;

    /// Test that verifies the full state initialization and wallet filtering
    /// This simulates what happens when a user connects
    #[test]
    fn test_wsmanager_wallet_filtering_with_real_state() {
        use crate::handler::can_user_access_wallet;
        use crate::state::ServerState;
        use liana_connect::UserRole;
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
            UserRole::WSManager,
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

        // Simulate the filtering that happens in connection.rs
        let user_email = &ws_manager.email;
        let global_role = ws_manager.role.clone();

        let filtered_wallet_ids: BTreeSet<Uuid> = acme_org
            .wallets
            .iter()
            .filter(|wallet_id| {
                if let Some(wallet) = wallets.get(wallet_id) {
                    can_user_access_wallet(wallet, user_email, global_role.clone())
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
            acme_org.wallets.len(),
            "WSManager should see ALL {} wallets in Acme Corp, got {}",
            acme_org.wallets.len(),
            filtered_wallet_ids.len()
        );
    }

    /// Test participant filtering - should NOT see draft/locked wallets
    #[test]
    fn test_participant_wallet_filtering_with_real_state() {
        use crate::handler::can_user_access_wallet;
        use crate::state::ServerState;
        use liana_connect::UserRole;
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

        // Find Acme Corp
        let acme_org = orgs
            .values()
            .find(|o| o.name == "Acme Corp")
            .expect("Acme Corp should exist");

        // Simulate filtering
        let user_email = &participant.email;
        let global_role = participant.role.clone();

        let filtered_wallet_ids: BTreeSet<Uuid> = acme_org
            .wallets
            .iter()
            .filter(|wallet_id| {
                if let Some(wallet) = wallets.get(wallet_id) {
                    can_user_access_wallet(wallet, user_email, global_role.clone())
                } else {
                    false
                }
            })
            .copied()
            .collect();

        // Participant should see fewer wallets than WSManager (no draft/locked)
        // user@example.com has keys in Validated, Final, Shared wallets (3 wallets)
        // But Shared is Finalized, so all 3 should be visible
        assert!(
            filtered_wallet_ids.len() <= 3,
            "Participant should see at most 3 wallets, got {}",
            filtered_wallet_ids.len()
        );

        // Verify Draft Wallet is NOT included
        for wallet_id in &filtered_wallet_ids {
            let wallet = wallets.get(wallet_id).unwrap();
            assert!(
                !matches!(wallet.status, liana_connect::WalletStatus::Drafted),
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
        use liana_connect::UserRole;
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

        // Step 4: Look up user by UUID (as done in connection.rs)
        let (user_email, global_role) = {
            let users = state.users.lock().unwrap();
            users
                .get(&extracted_user_id)
                .map(|u| (u.email.clone(), u.role.clone()))
                .expect("User should be found by UUID")
        };

        assert_eq!(user_email, email);
        assert_eq!(
            global_role,
            UserRole::WSManager,
            "Global role should be WSManager"
        );

        // Step 5: Filter wallets (as done in connection.rs)
        let orgs = state.orgs.lock().unwrap();
        let wallets = state.wallets.lock().unwrap();

        let acme_org = orgs
            .values()
            .find(|o| o.name == "Acme Corp")
            .expect("Acme Corp should exist");

        let filtered_wallet_ids: BTreeSet<Uuid> = acme_org
            .wallets
            .iter()
            .filter(|wallet_id| {
                if let Some(wallet) = wallets.get(wallet_id) {
                    can_user_access_wallet(wallet, &user_email, global_role.clone())
                } else {
                    false
                }
            })
            .copied()
            .collect();

        assert_eq!(
            filtered_wallet_ids.len(),
            acme_org.wallets.len(),
            "WSManager should see ALL {} wallets in Acme Corp, but only sees {}",
            acme_org.wallets.len(),
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
}
