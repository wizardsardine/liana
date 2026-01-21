use liana_connect::ws_business::{
    Key, KeyIdentity, KeyType, Org, PolicyTemplate, RegistrationInfos,
    SecondaryPath, SpendingPath, Timelock, User, UserRole, Wallet, WalletStatus,
};
use miniscript::bitcoin::bip32::Fingerprint;
use std::str::FromStr;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Get current unix timestamp in seconds
fn now_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Server state holding all organizations, wallets, and users
pub struct ServerState {
    pub orgs: Arc<Mutex<BTreeMap<Uuid, Org>>>,
    pub wallets: Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    pub users: Arc<Mutex<BTreeMap<Uuid, User>>>,
    /// Registration info per wallet+fingerprint (wallet_id, fingerprint) -> RegistrationInfos
    pub registration_infos: Arc<Mutex<BTreeMap<(Uuid, Fingerprint), RegistrationInfos>>>,
}

impl ServerState {
    pub fn new() -> Self {
        let mut orgs = BTreeMap::new();
        let mut wallets = BTreeMap::new();
        let mut users = BTreeMap::new();
        let mut registration_infos = BTreeMap::new();

        init_test_data(&mut orgs, &mut wallets, &mut users, &mut registration_infos);

        Self {
            orgs: Arc::new(Mutex::new(orgs)),
            wallets: Arc::new(Mutex::new(wallets)),
            users: Arc::new(Mutex::new(users)),
            registration_infos: Arc::new(Mutex::new(registration_infos)),
        }
    }
}

fn init_test_data(
    orgs: &mut BTreeMap<Uuid, Org>,
    wallets: &mut BTreeMap<Uuid, Wallet>,
    users: &mut BTreeMap<Uuid, User>,
    registration_infos: &mut BTreeMap<(Uuid, Fingerprint), RegistrationInfos>,
) {
    // ==========================================================================
    // TEST USERS - Login with these emails to test different roles:
    //
    // ws@example.com    -> WSManager for all wallets (not owner, no keys)
    // owner@example.com -> Owner of Draft/Validated/Final/Registration, Participant of Shared
    // user@example.com  -> Participant for all wallets (has keys in all)
    //                      Also test device registration with "Registration Wallet"
    //                      (connect HW with fingerprint c658b283)
    // alice@example.com -> Participant in Registration wallet (External key, no registration)
    //
    // Note: Participants should NOT see Draft wallets (filtered in view)
    // ==========================================================================

    // Owner user - owns Draft, Validated, Final wallets
    // Note: WalletManager role is derived at runtime from wallet ownership, not stored
    let owner_user = User {
        name: "Wallet Owner".to_string(),
        uuid: Uuid::new_v4(),
        email: "owner@example.com".to_string(),
        role: UserRole::Participant,
        last_edited: None,
        last_editor: None,
    };
    users.insert(owner_user.uuid, owner_user.clone());

    // Shared wallet owner - owns the Shared wallet
    // Note: WalletManager role is derived at runtime from wallet ownership, not stored
    let shared_owner = User {
        name: "Shared Wallet Owner".to_string(),
        uuid: Uuid::new_v4(),
        email: "shared-owner@example.com".to_string(),
        role: UserRole::Participant,
        last_edited: None,
        last_editor: None,
    };
    users.insert(shared_owner.uuid, shared_owner.clone());

    // WSManager user - manages all wallets
    let ws_manager = User {
        name: "WS Manager".to_string(),
        uuid: Uuid::new_v4(),
        email: "ws@example.com".to_string(),
        role: UserRole::WizardSardineAdmin,
        last_edited: None,
        last_editor: None,
    };
    users.insert(ws_manager.uuid, ws_manager.clone());

    // Participant user
    let participant_user = User {
        name: "Participant User".to_string(),
        uuid: Uuid::new_v4(),
        email: "user@example.com".to_string(),
        role: UserRole::Participant,
        last_edited: None,
        last_editor: None,
    };
    users.insert(participant_user.uuid, participant_user.clone());

    // Bob - participant
    let bob_user = User {
        name: "Bob".to_string(),
        uuid: Uuid::new_v4(),
        email: "bob@example.com".to_string(),
        role: UserRole::Participant,
        last_edited: None,
        last_editor: None,
    };
    users.insert(bob_user.uuid, bob_user.clone());

    // Alice - participant
    let alice_user = User {
        name: "Alice".to_string(),
        uuid: Uuid::new_v4(),
        email: "alice@example.com".to_string(),
        role: UserRole::Participant,
        last_edited: None,
        last_editor: None,
    };
    users.insert(alice_user.uuid, alice_user.clone());

    // Create Org 1: "Acme Corp" - demonstrates all wallet statuses and roles
    let org1_id = Uuid::new_v4();
    let mut org1_wallets = BTreeSet::new();

    // -------------------------------------------------------------------------
    // Wallet 1: DRAFT - Only visible to WSManager and Owner
    // owner@example.com -> Owner
    // ws@example.com    -> Manager
    // user@example.com  -> (should not see this wallet)
    // -------------------------------------------------------------------------
    let wallet1_id = Uuid::new_v4();
    let mut wallet1_template = PolicyTemplate::new();
    // Keys: Owner, Bob, Alice (with initial last_edited values)
    let key_timestamp = now_timestamp();
    wallet1_template.keys.insert(
        0,
        Key {
            id: 0,
            alias: "Owner".to_string(),
            description: "Key held by wallet owner".to_string(),
            identity: KeyIdentity::Email("owner@example.com".to_string()),
            key_type: KeyType::Internal,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: Some(key_timestamp - 1800), // 30 min ago
            last_editor: Some(ws_manager.uuid),
        },
    );
    wallet1_template.keys.insert(
        1,
        Key {
            id: 1,
            alias: "Bob".to_string(),
            description: "Bob's key".to_string(),
            identity: KeyIdentity::Email("bob@example.com".to_string()),
            key_type: KeyType::External,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: Some(key_timestamp - 5400), // 1.5 hours ago
            last_editor: Some(ws_manager.uuid),
        },
    );
    wallet1_template.keys.insert(
        2,
        Key {
            id: 2,
            alias: "Alice".to_string(),
            description: "Alice's key".to_string(),
            identity: KeyIdentity::Email("alice@example.com".to_string()),
            key_type: KeyType::External,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: Some(key_timestamp - 172800), // 2 days ago
            last_editor: Some(ws_manager.uuid),
        },
    );
    // Primary path: All of Owner, Bob (threshold = 2)
    wallet1_template.primary_path.key_ids.push(0); // Owner
    wallet1_template.primary_path.key_ids.push(1); // Bob
    wallet1_template.primary_path.threshold_n = 2;
    // Set initial last_edited on primary path (1 hour ago)
    wallet1_template.primary_path.last_edited = Some(now_timestamp() - 3600);
    wallet1_template.primary_path.last_editor = Some(ws_manager.uuid);
    // Secondary path 1: 1 of Alice, Bob - After 2 months (8760 blocks)
    let mut secondary1 = SpendingPath::new(false, 1, vec![2, 1]); // Alice, Bob
    secondary1.last_edited = Some(now_timestamp() - 7200); // 2 hours ago
    secondary1.last_editor = Some(ws_manager.uuid);
    wallet1_template.secondary_paths.push(SecondaryPath {
        path: secondary1,
        timelock: Timelock::new(8760),
    });
    // Secondary path 2: Owner - After 5 months (21900 blocks)
    let mut secondary2 = SpendingPath::new(false, 1, vec![0]); // Owner
    secondary2.last_edited = Some(now_timestamp() - 86400); // 1 day ago
    secondary2.last_editor = Some(ws_manager.uuid);
    wallet1_template.secondary_paths.push(SecondaryPath {
        path: secondary2,
        timelock: Timelock::new(21900),
    });

    let wallet1 = Wallet {
        alias: "Draft Wallet".to_string(),
        org: org1_id,
        owner: owner_user.uuid,
        id: wallet1_id,
        template: Some(wallet1_template),
        status: WalletStatus::Drafted,
        last_edited: None,
        last_editor: None,
    };
    org1_wallets.insert(wallet1_id);
    wallets.insert(wallet1_id, wallet1);

    // -------------------------------------------------------------------------
    // Wallet 2: VALIDATED - Visible to all, participants can add xpubs
    // owner@example.com -> Owner
    // ws@example.com    -> Manager
    // user@example.com  -> Participant
    // -------------------------------------------------------------------------
    let wallet2_id = Uuid::new_v4();
    let mut wallet2_template = PolicyTemplate::new();
    wallet2_template.keys.insert(
        0,
        Key {
            id: 0,
            alias: "Owner Key".to_string(),
            description: "Key held by wallet owner".to_string(),
            identity: KeyIdentity::Email("owner@example.com".to_string()),
            key_type: KeyType::Internal,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        },
    );
    wallet2_template.keys.insert(
        1,
        Key {
            id: 1,
            alias: "Participant Key".to_string(),
            description: "Key for participant user".to_string(),
            identity: KeyIdentity::Email("user@example.com".to_string()),
            key_type: KeyType::External,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        },
    );
    wallet2_template.primary_path.key_ids.push(0);
    wallet2_template.primary_path.key_ids.push(1);
    wallet2_template.primary_path.threshold_n = 2;

    let wallet2 = Wallet {
        alias: "Validated Wallet".to_string(),
        org: org1_id,
        owner: owner_user.uuid,
        id: wallet2_id,
        template: Some(wallet2_template),
        status: WalletStatus::Validated,
        last_edited: None,
        last_editor: None,
    };
    org1_wallets.insert(wallet2_id);
    wallets.insert(wallet2_id, wallet2);

    // -------------------------------------------------------------------------
    // Wallet 3: FINALIZED - Ready to load
    // owner@example.com -> Owner
    // ws@example.com    -> Manager
    // user@example.com  -> Participant
    // -------------------------------------------------------------------------
    let wallet3_id = Uuid::new_v4();
    let mut wallet3_template = PolicyTemplate::new();
    wallet3_template.keys.insert(
        0,
        Key {
            id: 0,
            alias: "Owner Key".to_string(),
            description: "Key held by wallet owner".to_string(),
            identity: KeyIdentity::Email("owner@example.com".to_string()),
            key_type: KeyType::Internal,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        },
    );
    wallet3_template.keys.insert(
        1,
        Key {
            id: 1,
            alias: "Participant Key".to_string(),
            description: "Key for participant user".to_string(),
            identity: KeyIdentity::Email("user@example.com".to_string()),
            key_type: KeyType::External,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        },
    );
    wallet3_template.primary_path.key_ids.push(0);
    wallet3_template.primary_path.key_ids.push(1);
    wallet3_template.primary_path.threshold_n = 2;

    let wallet3 = Wallet {
        alias: "Final Wallet".to_string(),
        org: org1_id,
        owner: owner_user.uuid,
        id: wallet3_id,
        template: Some(wallet3_template),
        status: WalletStatus::Finalized,
        last_edited: None,
        last_editor: None,
    };
    org1_wallets.insert(wallet3_id);
    wallets.insert(wallet3_id, wallet3);

    // -------------------------------------------------------------------------
    // Wallet 4: SHARED - Owner is different, owner@example.com is participant
    // shared-owner@example.com -> Owner
    // owner@example.com        -> Participant (has a key)
    // ws@example.com           -> Manager
    // user@example.com         -> Participant
    // -------------------------------------------------------------------------
    let wallet4_id = Uuid::new_v4();
    let mut wallet4_template = PolicyTemplate::new();
    wallet4_template.keys.insert(
        0,
        Key {
            id: 0,
            alias: "Shared Owner Key".to_string(),
            description: "Key held by shared wallet owner".to_string(),
            identity: KeyIdentity::Email("shared-owner@example.com".to_string()),
            key_type: KeyType::Internal,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        },
    );
    wallet4_template.keys.insert(
        1,
        Key {
            id: 1,
            alias: "Owner as Participant".to_string(),
            description: "owner@example.com is participant here".to_string(),
            identity: KeyIdentity::Email("owner@example.com".to_string()),
            key_type: KeyType::External,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        },
    );
    wallet4_template.keys.insert(
        2,
        Key {
            id: 2,
            alias: "User Key".to_string(),
            description: "Key for user@example.com".to_string(),
            identity: KeyIdentity::Email("user@example.com".to_string()),
            key_type: KeyType::External,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        },
    );
    wallet4_template.primary_path.key_ids.push(0);
    wallet4_template.primary_path.key_ids.push(1);
    wallet4_template.primary_path.key_ids.push(2);
    wallet4_template.primary_path.threshold_n = 2;

    let wallet4 = Wallet {
        alias: "Shared Wallet".to_string(),
        org: org1_id,
        owner: shared_owner.uuid,
        id: wallet4_id,
        template: Some(wallet4_template),
        status: WalletStatus::Finalized,
        last_edited: None,
        last_editor: None,
    };
    org1_wallets.insert(wallet4_id);
    wallets.insert(wallet4_id, wallet4);

    // -------------------------------------------------------------------------
    // Wallet 5: REGISTRATION_PENDING - Test device registration flow
    // owner@example.com -> Owner (Internal key, already registered)
    // user@example.com  -> Participant (Internal key, NOT registered)
    // alice@example.com -> Participant (External key, no registration needed)
    //
    // Test: Login as user@, register device, wallet should become Registered
    // -------------------------------------------------------------------------
    let wallet5_id = Uuid::new_v4();
    let mut wallet5_template = PolicyTemplate::new();

    // Owner's Internal key (fingerprint: d34db33f) - will be marked as registered
    let _owner_fingerprint = Fingerprint::from_str("d34db33f").unwrap();
    wallet5_template.keys.insert(
        0,
        Key {
            id: 0,
            alias: "Owner Key".to_string(),
            description: "Owner's signing key".to_string(),
            identity: KeyIdentity::Email("owner@example.com".to_string()),
            key_type: KeyType::Internal,
            xpub: None, // xpub not needed here - descriptor in Registration status has it
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        },
    );

    // User's Internal key (fingerprint: c658b283) - NOT registered yet
    let user_fingerprint = Fingerprint::from_str("c658b283").unwrap();
    wallet5_template.keys.insert(
        1,
        Key {
            id: 1,
            alias: "User Key".to_string(),
            description: "User's signing key - needs registration".to_string(),
            identity: KeyIdentity::Email("user@example.com".to_string()),
            key_type: KeyType::Internal,
            xpub: None, // xpub not needed here - descriptor in Registration status has it
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        },
    );

    // Alice's External key - no registration needed
    wallet5_template.keys.insert(
        2,
        Key {
            id: 2,
            alias: "Alice Key".to_string(),
            description: "Alice's external key - no registration".to_string(),
            identity: KeyIdentity::Email("alice@example.com".to_string()),
            key_type: KeyType::External,
            xpub: None, // xpub not needed here - descriptor in Registration status has it
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        },
    );

    // Primary path: 2-of-3 (Owner, User, Alice)
    wallet5_template.primary_path.key_ids.push(0);
    wallet5_template.primary_path.key_ids.push(1);
    wallet5_template.primary_path.key_ids.push(2);
    wallet5_template.primary_path.threshold_n = 2;

    // Descriptor with user's real xpub (c658b283) replacing one key for device registration test
    let descriptor = "wsh(or_i(and_v(v:pkh([c658b283/48'/0'/0'/2']tpubDFXEXpo64SD7RB7DePr9nBXtce6WmnKh2ZwUGTc8x3B2Ef8soRQM1w24tgoYJqh2FNR4UFfQGrNtjvYDMkh2a6y9gAGNQMjn9fnCzhnUVW9/<0;1>/*),older(20)),or_d(multi(2,[a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/<0;1>/*,[6abb52a9/48'/1'/0'/2']tpubDFZTCVU1Sa9nJXCxx97UFvGausHQPFjJyaiDbdr8GNqjCLKwYc8ihegK7yJdcizs9HMbiGA7ke1HiCENVHaERvNANHW7U2Wo2qnRsuqB52r/<0;1>/*),and_v(v:thresh(1,pkh([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/<2;3>/*),a:pkh([6abb52a9/48'/1'/0'/2']tpubDFZTCVU1Sa9nJXCxx97UFvGausHQPFjJyaiDbdr8GNqjCLKwYc8ihegK7yJdcizs9HMbiGA7ke1HiCENVHaERvNANHW7U2Wo2qnRsuqB52r/<2;3>/*)),older(10)))))".to_string();

    // User's device needs registration - store in separate registration_infos map
    // Note: registration_infos is initially empty, will be populated when user registers/skips

    let wallet5 = Wallet {
        alias: "Registration Wallet".to_string(),
        org: org1_id,
        owner: owner_user.uuid,
        id: wallet5_id,
        template: Some(wallet5_template),
        status: WalletStatus::Registration {
            descriptor,
            devices: vec![user_fingerprint],
        },
        last_edited: None,
        last_editor: None,
    };
    org1_wallets.insert(wallet5_id);
    wallets.insert(wallet5_id, wallet5);

    // Note: registration_infos starts empty - populated when DeviceRegistered is received
    let _ = registration_infos; // silence unused warning for now

    let org1 = Org {
        name: "Acme Corp".to_string(),
        id: org1_id,
        wallets: org1_wallets,
        users: Default::default(),
        owners: Default::default(),
        last_edited: None,
        last_editor: None,
    };
    orgs.insert(org1_id, org1);

    // Create Org 2: "Empty Org" - no wallets
    let org2_id = Uuid::new_v4();
    let org2 = Org {
        name: "Empty Org".to_string(),
        id: org2_id,
        wallets: BTreeSet::new(),
        users: Default::default(),
        owners: Default::default(),
        last_edited: None,
        last_editor: None,
    };
    orgs.insert(org2_id, org2);
}
