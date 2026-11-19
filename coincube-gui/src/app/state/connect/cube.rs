use std::collections::HashMap;
use std::sync::Arc;

use coincube_core::descriptors::CoincubeDescriptor;
use coincube_spark_protocol::LightningAddressInfo;
use iced::task::Handle as TaskHandle;

use crate::{
    app::{
        breez_spark::SparkClient,
        message::Message,
        view::{self, AvatarMessage, ConnectCubeMessage},
    },
    services::coincube::{
        vault_reconcile, AvatarGenerateRequest, AvatarSelectRequest, AvatarUserTraits,
        CoincubeClient, LightningAddress, PatchConnectVaultRequest, RegisterCubeRequest,
        UpdateCubeRequest,
    },
};

/// Phase 4g claim-flow rollback helper.
///
/// Tears down whichever of (SDK registration, API reservation)
/// actually succeeded, logging each delete failure with cube id +
/// username so manual cleanup has enough context. Returns the
/// suffix to splice onto the user-facing error: `""` on clean
/// rollback, the bracketed "partial rollback failure" note when
/// at least one delete call errored.
///
/// Pass `spark = None` at rollback sites where the SDK register
/// hadn't succeeded yet (i.e. rolling back only the API
/// reservation).
/// Why a claim of one username failed: `taken` when the API refused the
/// name as already in use (409), so another name may succeed.
pub(crate) struct ClaimError {
    pub taken: bool,
    pub message: String,
}

/// Claim `username` for the Cube: reserve it in our API, register it with the
/// Breez-hosted LNURL server through Spark, then confirm the reservation. A
/// failure after the reserve rolls back whatever succeeded. Shared by the
/// manual claim and the automatic one.
async fn claim_username(
    client: &CoincubeClient,
    spark: &SparkClient,
    cube_id: &str,
    username: &str,
) -> Result<LightningAddress, ClaimError> {
    // Step 1: reserve in our DB. If the username is already taken (409) the
    // API surfaces it here.
    if let Err(e) = client.reserve_lightning_address(cube_id, username).await {
        let taken = matches!(
            &e,
            crate::services::coincube::CoincubeError::Unsuccessful(info) if info.status_code == 409
        );
        return Err(ClaimError {
            taken,
            message: format!("Reserve failed: {}", e),
        });
    }

    // Step 2: register against the Breez-hosted LNURL server via the Spark
    // bridge. On failure, release the reservation (SDK side never succeeded,
    // so only roll back the API reservation).
    if let Err(e) = spark
        .register_lightning_address(username.to_string(), None)
        .await
    {
        let rb_note = rollback_partial_claim(client, None, cube_id, username).await;
        return Err(ClaimError {
            taken: false,
            message: format!("Register failed: {}{}", e, rb_note),
        });
    }

    // Step 3: commit. The API stamps `lightning_address_confirmed_at` on the
    // existing reservation. On failure roll back both the SDK registration
    // and the reservation.
    match client.confirm_lightning_address(cube_id).await {
        Ok(ln_addr) => Ok(ln_addr),
        Err(e) => {
            let rb_note = rollback_partial_claim(client, Some(spark), cube_id, username).await;
            Err(ClaimError {
                taken: false,
                message: format!("Confirm failed: {}{}", e, rb_note),
            })
        }
    }
}

/// How many random names an automatic assignment tries before giving up.
const AUTO_LN_ATTEMPTS: usize = 8;
/// After this many plain `adjectiveanimal` names, add two digits.
const AUTO_LN_PLAIN_ATTEMPTS: usize = 4;

/// Give the Cube a random Lightning address: pick `adjectiveanimal` (later
/// with two digits), skip it if either registry says it's taken, and claim it.
/// The availability checks are hints; the reserve is the real gate, and a 409
/// there just moves on to the next name.
async fn auto_claim(
    client: &CoincubeClient,
    spark: &SparkClient,
    cube_id: &str,
) -> Result<LightningAddress, String> {
    let mut rng = rand::rngs::OsRng;
    for attempt in 0..AUTO_LN_ATTEMPTS {
        let name = random_ln_username(&mut rng, attempt >= AUTO_LN_PLAIN_ATTEMPTS);
        debug_assert!(validate_ln_username(&name).is_none(), "{}", name);
        match spark.check_lightning_address_available(name.clone()).await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => return Err(format!("Couldn't check Lightning names: {}", e)),
        }
        // Our API is authoritative for @coincube.io; an error here falls
        // back to the reserve step, which refuses a taken name anyway.
        if let Ok(false) = client
            .check_lightning_address_available(cube_id, &name)
            .await
        {
            continue;
        }
        match claim_username(client, spark, cube_id, &name).await {
            Ok(address) => return Ok(address),
            Err(e) if e.taken => continue,
            Err(e) => return Err(e.message),
        }
    }
    Err("Couldn't find a free Lightning name. Choose one in Settings.".to_string())
}

const LN_ADJECTIVES: &[&str] = &[
    "amber", "brave", "bright", "calm", "clever", "cosmic", "crisp", "curious", "daring", "eager",
    "fearless", "gentle", "golden", "happy", "hidden", "honest", "jolly", "keen", "kind", "lively",
    "lucky", "mellow", "merry", "mighty", "misty", "noble", "orange", "patient", "plucky",
    "polite", "proud", "quick", "quiet", "rapid", "rosy", "rustic", "sandy", "shiny", "silent",
    "silver", "sleepy", "smart", "snowy", "solar", "speedy", "steady", "stellar", "sunny", "swift",
    "tidy", "tiny", "trusty", "velvet", "vivid", "wild", "wise", "witty", "zesty",
];

const LN_ANIMALS: &[&str] = &[
    "badger", "beaver", "bison", "bobcat", "condor", "coyote", "crane", "dolphin", "eagle",
    "falcon", "ferret", "finch", "fox", "gecko", "gopher", "hare", "hawk", "heron", "ibex",
    "jaguar", "koala", "lemur", "leopard", "llama", "lynx", "marmot", "marten", "moose", "narwhal",
    "ocelot", "orca", "osprey", "otter", "owl", "panda", "panther", "pelican", "penguin", "puffin",
    "quail", "raven", "robin", "salmon", "seal", "sparrow", "stork", "swan", "tapir", "tiger",
    "toucan", "turtle", "walrus", "weasel", "whale", "wolf", "wombat", "yak", "zebra",
];

/// A random `adjectiveanimal` username (e.g. `orangezebra`), with two digits
/// appended when `numbered` (e.g. `orangezebra42`). Lowercase ASCII only, so
/// it always passes `validate_ln_username`.
pub(crate) fn random_ln_username<R: rand::Rng + ?Sized>(rng: &mut R, numbered: bool) -> String {
    use rand::seq::SliceRandom;
    let adjective = LN_ADJECTIVES.choose(rng).copied().unwrap_or("orange");
    let animal = LN_ANIMALS.choose(rng).copied().unwrap_or("zebra");
    if numbered {
        format!("{}{}{}", adjective, animal, rng.gen_range(10..100))
    } else {
        format!("{}{}", adjective, animal)
    }
}

async fn rollback_partial_claim(
    client: &CoincubeClient,
    spark: Option<&SparkClient>,
    cube_id: &str,
    username: &str,
) -> &'static str {
    let mut partial = false;
    if let Some(spark) = spark {
        if let Err(e) = spark.delete_lightning_address().await {
            log::error!(
                "[CONNECT-CUBE] rollback of Spark lightning-address registration \
                 failed (cube={}, username={}): {}",
                cube_id,
                username,
                e
            );
            partial = true;
        }
    }
    if let Err(e) = client.delete_lightning_address(cube_id).await {
        log::error!(
            "[CONNECT-CUBE] rollback of API lightning-address reservation failed \
             (cube={}, username={}): {}",
            cube_id,
            username,
            e
        );
        partial = true;
    }
    if partial {
        " (partial rollback failure — please contact support)"
    } else {
        ""
    }
}

/// Outcome of [`ConnectCubePanel::reconcile_spark_lightning_address`].
///
/// Splits "SDK already in sync / we fixed it" from the two error
/// shapes so the UI can react differently: a query failure is
/// transient and retryable on the next trigger, while
/// [`ReconcileOutcome::NeedsReRegistration`] is a persistent
/// API↔SDK divergence that needs the user's attention (the claim
/// record is in our DB but the Breez LNURL server doesn't know
/// about this device).
#[derive(Debug, Clone)]
pub enum ReconcileOutcome {
    /// SDK's local cache already holds the expected record.
    AlreadyBound(LightningAddressInfo),
    /// SDK cache was empty; we re-registered the DB-confirmed
    /// username on this device.
    ReRegistered(LightningAddressInfo),
    /// Querying the SDK failed. Transient — the next
    /// `LightningAddressChanged { info: None }` event or the next
    /// cube-registered reload will retry.
    QueryFailed(String),
    /// SDK had no record and the register call failed. API and SDK
    /// are now out of sync; surface this so the user can re-claim
    /// from settings.
    NeedsReRegistration(String),
}

/// Terminal outcome of the username-change chain (PUT + SDK delete +
/// SDK register). Carried in
/// [`crate::app::view::ConnectCubeMessage::LightningAddressUpdated`].
#[derive(Debug, Clone)]
pub enum LightningAddressChangeOutcome {
    /// Server committed and the SDK rebound to the new name.
    Ok(LightningAddress),
    /// Server rejected the request (409 conflict / 422 invalid /
    /// 5xx). The cube's existing address is unchanged everywhere.
    ServerError(String),
    /// Server committed the rename but the SDK rebind failed
    /// (delete or register errored). The DB-confirmed address is
    /// the new one; the SDK is either still bound to the old name
    /// (delete failed) or empty (register failed). The
    /// re-registration prompt surfaces in either case.
    SdkSyncFailed {
        addr: LightningAddress,
        message: String,
    },
}

use super::{cube_members, AvatarFlowStep, ConnectCubeMembersState};

/// Per-Cube Connect panel handling Lightning Address and Avatar.
/// The Lightning Address claim flow is fulfilled by the Cube's
/// Spark wallet via Breez-hosted LNURL.
/// Why `POST /connect/cubes` failed, as carried by
/// [`ConnectCubeMessage::CubeRegistered`].
#[derive(Debug, Clone)]
pub struct CubeRegistrationError {
    /// User-safe description (the error's `Display`).
    pub message: String,
    /// The server refused because another Connect account owns this Cube.
    pub owned_by_other_account: bool,
}

impl From<&crate::services::coincube::CoincubeError> for CubeRegistrationError {
    fn from(e: &crate::services::coincube::CoincubeError) -> Self {
        Self {
            message: e.to_string(),
            owned_by_other_account: e.is_cube_owned_by_another_account(),
        }
    }
}

impl From<String> for CubeRegistrationError {
    fn from(message: String) -> Self {
        Self {
            message,
            owned_by_other_account: false,
        }
    }
}

pub struct ConnectCubePanel {
    /// The Cube's client-side UUID (from CubeSettings.id)
    pub cube_uuid: String,
    /// The Cube's display name (for registration)
    pub cube_name: String,
    /// The Cube's network ("mainnet" or "testnet")
    pub cube_network: String,
    /// Whether this Cube currently has a Vault wallet
    /// (`vault_wallet_id.is_some()`). Reported to the server on
    /// registration so other devices can evaluate the duress vault gate
    /// (PLAN-duress-vault-gate PR 3). Flipped to `true` in-session when a
    /// Vault is created (see `App::WalletUpdated`).
    pub cube_has_vault: bool,
    /// This device's recorded Spark Stable Balance decision
    /// (`CubeSettings::spark_stable_balance`), seeded by the App and updated
    /// through [`Self::report_spark_stable_balance`]. Sent on registration so
    /// the Cube's server record — the cross-device copy — picks it up; `None`
    /// is omitted from the request so a device with no decision never
    /// clobbers another device's.
    pub spark_stable_balance: Option<bool>,
    /// This Cube's Connect-blinding encryption **public** key (33-byte
    /// compressed secp256k1, lowercase hex), read from
    /// `CubeSettings::connect_encryption_pubkey`. `None` for a Cube whose seed
    /// hasn't been unlocked on a build that derives it (e.g. a passkey Cube) —
    /// registration then simply doesn't run, and Contacts can't enrol enveloped
    /// keys against this Cube until it does.
    pub cube_encryption_pubkey: Option<String>,
    /// True once this session has published [`Self::cube_encryption_pubkey`] to
    /// Connect. The endpoint is idempotent, so this only avoids a redundant
    /// round-trip per launch — correctness doesn't depend on it.
    pub(super) enc_pubkey_registered: bool,
    /// This Cube's Vault descriptor fingerprint (8 lowercase hex), read from
    /// `CubeSettings::vault_fingerprint`. `None` when this device holds no
    /// Vault for the Cube, or holds one whose fingerprint hasn't been computed
    /// yet — either way there is nothing to assert.
    pub vault_fingerprint: Option<String>,
    /// True once this session has asserted [`Self::vault_fingerprint`] to
    /// Connect. The endpoint no-ops an unchanged value, so this only avoids a
    /// redundant round-trip per launch — correctness doesn't depend on it.
    pub(super) vault_fingerprint_asserted: bool,
    /// This Cube's Vault descriptor, seeded from the loaded wallet by
    /// `App::vault_fingerprint_backfill_task`. The COIN-373 reconcile needs the
    /// descriptor itself, not just its fingerprint — it decides which registered
    /// cube keys this Vault actually commits to — and it is held here for the
    /// same reason [`Self::vault_fingerprint`] is: the reconcile's trigger has
    /// to survive until the `server_cube_id` it waits on exists. `None` when
    /// this device holds no Vault for the Cube.
    pub(super) vault_descriptor: Option<CoincubeDescriptor>,
    /// True once this session has run the COIN-373 membership reconcile for
    /// this Cube's Vault. One pass per launch is enough: the fan-out that can
    /// drop a member row only runs in the installer, and a Vault built
    /// mid-session re-enters through the same triggers.
    pub(super) vault_members_reconciled: bool,
    /// The server-side numeric ID — set after registering with the backend.
    /// Used in API paths: /connect/cubes/{server_cube_id}/...
    pub server_cube_id: Option<u64>,
    /// Set when the last cube registration attempt failed.
    pub registration_error: Option<String>,
    /// Set when the last registration was refused because this Cube is
    /// registered under a different Connect account
    /// (`CUBE_OWNED_BY_ANOTHER_ACCOUNT`). Mirrored to
    /// `Cache::cube_owned_by_other_account` for the Vault signing picker.
    pub owned_by_other_account: bool,
    /// True while a `register_cube` round-trip is in flight.
    ///
    /// The re-entry guard for [`ConnectCubePanel::register_cube`]. Without it,
    /// `ConnectPanel::update` re-fires a registration on *every*
    /// `ConnectAccountMessage` that arrives while `server_cube_id` is still
    /// `None` — and it stays `None` for as long as the server keeps refusing
    /// (an account at its per-network Cube limit refuses indefinitely). A
    /// single session check therefore fanned out into three concurrent
    /// POST /connect/cubes, and every later account message did it again.
    ///
    /// Cleared on *both* arms of `CubeRegistered`, so a failure re-arms the
    /// next legitimate trigger rather than latching the Cube out of ever
    /// registering. The client's 20s request timeout is what guarantees the
    /// response — and so the clear — always arrives.
    pub(super) registering: bool,
    /// Which authenticated session the in-flight registration belongs to.
    /// Bumped by [`ConnectCubePanel::clear_client`], i.e. on every sign-out or
    /// account switch.
    ///
    /// A registration issued under one account can land after the user has
    /// signed into another — nothing aborts the request, and the reply is a
    /// plain message. Adopting its `server_cube_id` would name a row in the
    /// *previous* account: every follow-up it triggers (encryption pubkey,
    /// vault fingerprint, member reconcile, avatar) would then be aimed at a
    /// row this session's token cannot touch, and, worse,
    /// `ensure_cube_registered`'s `server_cube_id.is_some()` short-circuit
    /// would stop the new account's Cube from ever registering — stuck until
    /// relaunch. So the completion handler drops any result whose generation
    /// no longer matches.
    pub(super) session_generation: u64,
    // Lightning Address
    pub lightning_address: Option<LightningAddress>,
    pub ln_username_input: String,
    pub ln_username_available: Option<bool>,
    pub ln_username_error: Option<String>,
    pub ln_claim_error: Option<String>,
    pub ln_claiming: bool,
    pub ln_checking: bool,
    /// Automatic assignment of a random Lightning address (see
    /// `AutoAssignLightningAddress`), mirrored into the cache for Receive.
    pub ln_auto: crate::app::cache::LightningAddressSetup,
    /// True when the claimed-address card is rendered in its in-place
    /// edit form. The unclaimed flow leaves this `false`.
    pub ln_editing: bool,
    /// True during the PUT + SDK delete + SDK register chain. Distinct
    /// from `ln_claiming` so view code can spinner-gate the
    /// edit form independently of the unclaimed-flow claim button.
    pub ln_changing: bool,
    /// True while a manual retry of the SDK rebind (delete + register
    /// against the DB-confirmed username) is in flight, kicked off
    /// from the re-registration prompt on the claimed-address card.
    pub ln_reregistering: bool,
    ln_check_version: u32,
    ln_check_abort: Option<TaskHandle>,
    /// True when the latest availability check was issued before the
    /// server cube id was known, so it could only ask Spark. The
    /// registration handler re-runs it once the id lands so the hint
    /// reflects the API's answer too.
    ln_check_missing_cube_id: bool,
    /// Abort handle for the manual SDK rebind task. It holds the shared
    /// Spark client and the previous account's username, so it must not
    /// keep issuing SDK calls after `clear_client`.
    ln_reregister_abort: Option<TaskHandle>,
    /// Spark subprocess client. Phase 4g routes the Lightning
    /// Address claim flow through `register_lightning_address` on
    /// the Breez-hosted LNURL server; the API's own reserve/confirm
    /// endpoints bracket the SDK call. `None` for cubes without a
    /// Spark signer (those cubes can't claim Lightning Addresses
    /// under the new flow — the UI hides the claim button in that
    /// case).
    spark_client: Option<Arc<SparkClient>>,
    /// Persistent divergence-between-API-and-SDK signal, populated
    /// when `reconcile_spark_lightning_address` couldn't bind the
    /// DB-confirmed username on this device. Displayed to the user
    /// as "Lightning address needs re-registration" so they can
    /// act on it — the reconcile task can't retry the Breez server
    /// forever on its own. Cleared whenever the SDK next reports a
    /// bound address.
    pub ln_reconcile_needs_reregister: Option<String>,
    /// API client with JWT set — obtained from ConnectAccountPanel after login.
    pub client: Option<CoincubeClient>,
    // Avatar
    pub avatar_step: AvatarFlowStep,
    pub avatar_data: Option<crate::services::coincube::GetAvatarData>,
    pub avatar_generating: bool,
    pub avatar_error: Option<String>,
    pub avatar_image_cache: HashMap<u64, (Vec<u8>, iced::widget::image::Handle)>,
    pub avatar_draft: AvatarUserTraits,
    // Members (W8)
    pub members: ConnectCubeMembersState,
}

impl ConnectCubePanel {
    pub fn is_registering(&self) -> bool {
        self.registering
    }

    pub fn new(
        spark_client: Option<Arc<SparkClient>>,
        cube_uuid: String,
        cube_name: String,
        cube_network: String,
        cube_has_vault: bool,
    ) -> Self {
        ConnectCubePanel {
            cube_uuid,
            cube_name,
            cube_network,
            cube_has_vault,
            spark_stable_balance: None,
            cube_encryption_pubkey: None,
            enc_pubkey_registered: false,
            vault_fingerprint: None,
            vault_fingerprint_asserted: false,
            vault_descriptor: None,
            vault_members_reconciled: false,
            server_cube_id: None,
            registration_error: None,
            owned_by_other_account: false,
            registering: false,
            session_generation: 0,
            lightning_address: None,
            ln_username_input: String::new(),
            ln_username_available: None,
            ln_username_error: None,
            ln_claim_error: None,
            ln_claiming: false,
            ln_auto: crate::app::cache::LightningAddressSetup::Idle,
            ln_checking: false,
            ln_editing: false,
            ln_changing: false,
            ln_reregistering: false,
            ln_check_version: 0,
            ln_check_abort: None,
            ln_check_missing_cube_id: false,
            ln_reregister_abort: None,
            spark_client,
            ln_reconcile_needs_reregister: None,
            client: None,
            avatar_step: AvatarFlowStep::Idle,
            avatar_data: None,
            avatar_generating: false,
            avatar_error: None,
            avatar_image_cache: HashMap::new(),
            avatar_draft: AvatarUserTraits::default(),
            members: ConnectCubeMembersState::new(),
        }
    }

    /// Set the authenticated API client (called after account login).
    pub fn set_client(&mut self, client: CoincubeClient) {
        // A different Connect session cannot inherit this Cube's registered
        // row or Lightning address. clear_client also retires in-flight
        // registration replies by advancing their generation.
        if self.client.as_ref().is_some_and(|current| {
            current.base_url != client.base_url || current.token() != client.token()
        }) {
            self.clear_client();
        }
        self.client = Some(client);
    }

    /// Returns a task to load avatar if conditions are met (client available, cube registered, not already loaded).
    pub fn load_avatar_if_needed(&self) -> Option<iced::Task<Message>> {
        if self.client.is_some() && self.server_cube_id.is_some() && self.avatar_data.is_none() {
            let client = self.client.clone().unwrap();
            let cid = self.api_cube_id().unwrap();
            let generation = self.session_generation;
            return Some(iced::Task::perform(
                async move { client.get_avatar(&cid).await },
                move |res| {
                    Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                        crate::app::view::AvatarMessage::Loaded {
                            generation,
                            result: res.map_err(|e| e.to_string()),
                        },
                    )))
                },
            ));
        }
        None
    }

    /// Returns the active avatar image handle for the sidebar, if available.
    pub fn get_active_avatar_handle(&self) -> Option<iced::widget::image::Handle> {
        let id = self
            .avatar_data
            .as_ref()
            .and_then(active_avatar_variant_id)?;
        self.avatar_image_cache
            .get(&id)
            .map(|(_, handle)| handle.clone())
    }

    /// Clear the API client and all session-scoped state (called on account logout).
    pub fn clear_client(&mut self) {
        self.client = None;
        self.server_cube_id = None;
        // The key publication latch belongs to the server Cube row too. A
        // replacement account (or server) must publish to its new row.
        self.enc_pubkey_registered = false;
        // The latch means "already sent to *this* server cube row", so it dies
        // with the row id above. After signing in as a different account the
        // next registration names a different row, or none at all (the server
        // refuses a UUID another account owns with CUBE_OWNED_BY_ANOTHER_ACCOUNT)
        // — and `CubeRegistered` only ever *sets* this flag, never clears it. Left stale, the assertion would
        // early-return and strand the new account's Vault with no id in Keychain
        // until relaunch. Re-opening it costs at most one redundant PATCH, which
        // the server no-ops.
        self.vault_fingerprint_asserted = false;
        // Same reasoning as the fingerprint latch: it means "already
        // reconciled against *this* server cube row", which dies with the id.
        self.vault_members_reconciled = false;
        self.registration_error = None;
        self.owned_by_other_account = false;
        // Retire any attempt issued under the session that just ended: its
        // reply, when it lands, must not be mistaken for this one's. Clearing
        // the latch here (rather than waiting for that reply) is what lets the
        // next account register immediately instead of being silently skipped
        // for up to the client's 20s request timeout.
        self.registering = false;
        self.session_generation = self.session_generation.wrapping_add(1);
        self.lightning_address = None;
        self.ln_username_input.clear();
        self.ln_username_available = None;
        self.ln_username_error = None;
        self.ln_claim_error = None;
        self.ln_claiming = false;
        self.ln_auto = crate::app::cache::LightningAddressSetup::Idle;
        self.ln_checking = false;
        self.ln_editing = false;
        self.ln_changing = false;
        self.ln_reregistering = false;
        self.ln_check_version += 1;
        if let Some(handle) = self.ln_check_abort.take() {
            handle.abort();
        }
        self.ln_check_missing_cube_id = false;
        if let Some(handle) = self.ln_reregister_abort.take() {
            handle.abort();
        }
        self.ln_reconcile_needs_reregister = None;
        self.avatar_step = AvatarFlowStep::Idle;
        self.avatar_data = None;
        self.avatar_generating = false;
        self.avatar_error = None;
        self.avatar_image_cache.clear();
        self.avatar_draft = AvatarUserTraits::default();
        self.members.clear();
    }

    /// The current session's registration generation, for callers that issue a
    /// registration outside this panel and feed the reply back through
    /// `CubeRegistered` (see `App::EnsureConnectReady`).
    pub fn session_generation(&self) -> u64 {
        self.session_generation
    }

    /// Claim the in-flight registration guard on behalf of a caller that issues
    /// the request itself, returning the generation to stamp it with — or
    /// `None` when an attempt is already running and this one should be
    /// dropped.
    ///
    /// `App::EnsureConnectReady` registers directly from restored `connect.json`
    /// tokens when this panel has no client of its own, so it cannot go through
    /// [`Self::register_cube`] and would otherwise bypass its guard. Its trigger
    /// is a modal button that reappears while the bootstrap is still running, so
    /// a second press lands mid-flight: both POSTs carry the same Cube UUID, and
    /// the server answers the loser with an error that would overwrite the
    /// winner's success with a `registration_error` on screen.
    ///
    /// The reply clears the guard through the normal `CubeRegistered` path, on
    /// either arm, exactly as a panel-issued registration does.
    pub fn begin_external_registration(&mut self) -> Option<u64> {
        if self.registering {
            return None;
        }
        self.registering = true;
        Some(self.session_generation)
    }

    /// Returns the server-side cube ID as a string for API paths.
    fn api_cube_id(&self) -> Option<String> {
        self.server_cube_id.map(|id| id.to_string())
    }

    /// Phase 4g: reconcile the Spark SDK's Lightning Address state
    /// against our DB-reserved record.
    ///
    /// Fires `get_lightning_address()` on the Spark bridge. The
    /// matching handler in [`Self::update_message`] auto-re-registers
    /// the DB-confirmed username when the SDK reports `None` (device
    /// reinstall, SDK storage wipe, multi-device identity swap).
    ///
    /// Returns `None` when there's nothing to do — no Spark backend,
    /// or no DB-confirmed address. A DB `lightning_address` that
    /// can't be split on `@` is logged as a malformed record (the
    /// API shouldn't persist these) and skipped — reconcile can't
    /// do anything with a row the user would have to clean up
    /// manually anyway.
    pub fn reconcile_spark_lightning_address(&self) -> Option<iced::Task<Message>> {
        let spark = self.spark_client.clone()?;
        let db_addr = self
            .lightning_address
            .as_ref()
            .and_then(|la| la.lightning_address.as_ref())?;
        // Split `user@domain` → `user`. A row without `@` or with
        // an empty username portion is a partially-confirmed /
        // malformed record — log loudly so it surfaces for cleanup
        // instead of silently bailing.
        let db_username = db_addr.split('@').next().unwrap_or("");
        if db_username.is_empty() || !db_addr.contains('@') {
            log::warn!(
                "[CONNECT-CUBE] skipping reconcile: malformed DB \
                 lightning address {:?} (expected user@domain)",
                db_addr
            );
            return None;
        }
        let db_username = db_username.to_string();
        let db_addr = db_addr.clone();
        let generation = self.session_generation;
        Some(iced::Task::perform(
            async move {
                match spark.get_lightning_address().await {
                    Ok(Some(info)) => {
                        // Only treat as "in sync" when the SDK's
                        // full `user@domain` matches the DB-confirmed
                        // reservation. Matching on username alone
                        // would miss `COINCUBE_LNURL_DOMAIN` drift
                        // (staging/prod env flip) — the SDK would
                        // hold `user@staging.coincube.io` while the
                        // DB has `user@coincube.io` and we'd display
                        // the wrong address. `register_lightning_address`
                        // can't retarget domains (the SDK's
                        // `lnurl_domain` is fixed at init), so
                        // surface the divergence for the operator
                        // instead of silently papering over it.
                        if info.lightning_address == db_addr {
                            ReconcileOutcome::AlreadyBound(info)
                        } else {
                            ReconcileOutcome::NeedsReRegistration(format!(
                                "Spark SDK is bound to '{}' but the confirmed \
                                 reservation is '{}'",
                                info.lightning_address, db_addr
                            ))
                        }
                    }
                    Ok(None) => {
                        // SDK has no record — try to bind the
                        // DB-confirmed username on this device.
                        match spark.register_lightning_address(db_username, None).await {
                            Ok(info) => {
                                // Same guard as the `AlreadyBound`
                                // branch: the SDK's `lnurl_domain` is
                                // fixed at init from
                                // `COINCUBE_LNURL_DOMAIN`, so a
                                // staging/prod env flip would let the
                                // register call succeed against the
                                // wrong domain (e.g. binds
                                // `user@staging.coincube.io` while the
                                // DB-confirmed reservation is
                                // `user@coincube.io`). Surface the
                                // divergence instead of returning a
                                // mismatched `ReRegistered`.
                                if info.lightning_address == db_addr {
                                    ReconcileOutcome::ReRegistered(info)
                                } else {
                                    rollback_reconcile_domain_mismatch(&spark, &info, &db_addr)
                                        .await
                                }
                            }
                            Err(e) => ReconcileOutcome::NeedsReRegistration(e.to_string()),
                        }
                    }
                    Err(e) => ReconcileOutcome::QueryFailed(e.to_string()),
                }
            },
            move |outcome| {
                Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::LightningAddressReconciled {
                        generation,
                        outcome,
                    },
                ))
            },
        ))
    }

    /// Register this cube with the backend. Called after login.
    /// Returns a task that sends CubeRegistered on completion.
    ///
    /// A no-op while a previous attempt is still in flight — see
    /// [`Self::registering`]. Every caller here is a *trigger*, not a user
    /// action ("the session changed, make sure this Cube is registered"), and
    /// several of them fire from the same batch of messages; without the guard
    /// they each spawn their own POST.
    pub fn register_cube(&mut self) -> iced::Task<Message> {
        let Some(client) = self.client.clone() else {
            return iced::Task::none();
        };
        if self.registering {
            return iced::Task::none();
        }
        self.registering = true;
        let generation = self.session_generation;
        let req = RegisterCubeRequest {
            uuid: self.cube_uuid.clone(),
            name: self.cube_name.clone(),
            network: self.cube_network.clone(),
            // Monotonic upgrade-only: assert the Vault only when this device
            // holds it, else omit so a re-register never clobbers a `true`
            // reported elsewhere (PLAN-duress-vault-gate PR 3).
            has_vault: self.cube_has_vault.then_some(true),
            spark_stable_balance: self.spark_stable_balance,
        };
        iced::Task::perform(async move { client.register_cube(req).await }, move |res| {
            Message::View(view::Message::ConnectCube(
                ConnectCubeMessage::CubeRegistered {
                    generation,
                    result: res.map_err(|e| CubeRegistrationError::from(&e)),
                },
            ))
        })
    }

    /// Publishes this Cube's Connect-blinding encryption pubkey to the API
    /// (`PLAN-connect-blinding` PR D2 — the "registration wave" that also
    /// unblocks the server-side migration, api PR A5).
    ///
    /// This is what lets an invited Contact's Keychain seal its xpub to us: the
    /// API attaches the registered key to invite payloads and refuses
    /// envelope-mode enrolment for owners who haven't registered. So it must
    /// run **before** any invite is created — hence firing it as soon as the
    /// server cube id is known, not lazily at Vault-build time.
    ///
    /// Idempotent server-side; no-ops without a live client, a server cube id,
    /// or a derived pubkey. Failure is logged and retried on the next launch:
    /// nothing the user is doing right now depends on it, and surfacing a toast
    /// for a background hygiene call would be noise.
    pub fn register_encryption_pubkey(&mut self) -> iced::Task<Message> {
        if self.enc_pubkey_registered {
            return iced::Task::none();
        }
        let (Some(client), Some(server_id), Some(pubkey)) = (
            self.client.clone(),
            self.server_cube_id,
            self.cube_encryption_pubkey.clone(),
        ) else {
            return iced::Task::none();
        };
        self.enc_pubkey_registered = true;
        iced::Task::perform(
            async move {
                match client.put_cube_encryption_pubkey(server_id, &pubkey).await {
                    Ok(_) => {
                        log::info!(
                            "[CONNECT-CUBE] registered encryption pubkey {} for cube {}",
                            pubkey,
                            server_id
                        );
                        true
                    }
                    Err(e) => {
                        log::warn!(
                            "[CONNECT-CUBE] registering encryption pubkey for cube {} failed: {e}",
                            server_id
                        );
                        false
                    }
                }
            },
            |ok| {
                Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::EncryptionKeyRegistered(ok),
                ))
            },
        )
    }

    /// Asserts this Vault's descriptor fingerprint to Connect
    /// (`plans/PLAN-vault-identity-unification.md` D3/D4) — the id Keychain
    /// renders for the vault on its Cubes list and its paired-desktops row.
    ///
    /// The desktop is the only party that can supply it: the server holds no
    /// plaintext descriptor and by design never can. So every vault that
    /// predates this has a blank identity until the device holding it opens the
    /// Cube once, which is what this call converges.
    ///
    /// PATCH rather than a create-time field alone, because the common case is
    /// a vault shell that already exists. Re-asserting the same value is a
    /// server-side no-op that writes no audit row, so it is safe to fire on
    /// every launch without reading the current value back first.
    ///
    /// No-ops without a live client, a server cube id, or a fingerprint. A
    /// failure is logged and retried next launch: a 404 here just means this
    /// Cube's Vault is local-only (never registered with Connect), and a
    /// 403 means the account's Estate entitlement lapsed — the vault then keeps
    /// whatever identity it already had. Neither is worth a toast.
    pub fn assert_vault_fingerprint(&mut self) -> iced::Task<Message> {
        if self.vault_fingerprint_asserted {
            return iced::Task::none();
        }
        let (Some(client), Some(server_id), Some(fingerprint)) = (
            self.client.clone(),
            self.server_cube_id,
            self.vault_fingerprint.clone(),
        ) else {
            return iced::Task::none();
        };
        self.vault_fingerprint_asserted = true;
        iced::Task::perform(
            async move {
                let req = PatchConnectVaultRequest {
                    fingerprint: Some(fingerprint.clone()),
                };
                match client.patch_connect_vault(server_id, req).await {
                    Ok(_) => {
                        log::info!(
                            "[CONNECT-CUBE] asserted vault fingerprint {} for cube {}",
                            fingerprint,
                            server_id
                        );
                        true
                    }
                    Err(e) => {
                        log::warn!(
                            "[CONNECT-CUBE] asserting vault fingerprint for cube {} failed: {e}",
                            server_id
                        );
                        false
                    }
                }
            },
            |ok| {
                Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::VaultFingerprintAsserted(ok),
                ))
            },
        )
    }

    /// Run the COIN-373 membership reconcile for this Cube's Vault (see
    /// [`crate::services::coincube::vault_reconcile`]).
    ///
    /// The sign-time pass inside the Keychain sign modal was the only trigger
    /// before this one, which meant a Vault whose member fan-out dropped a row
    /// stayed broken *on the phone* — where the Keychain app resolves a key's
    /// vault purely from those rows — until somebody happened to start a
    /// desktop sign. Firing it at Cube open converges the two without the user
    /// having to do anything, and the same trigger covers a Vault built
    /// mid-session (`App::vault_fingerprint_backfill_task` runs in both).
    ///
    /// No-ops without a live client or a server cube id, and self-latching for
    /// the session. Entirely best-effort: it only ever adds routing metadata
    /// for a key the descriptor already commits to, and every failure inside
    /// is logged and skipped.
    pub fn reconcile_vault_members(&mut self) -> iced::Task<Message> {
        if self.vault_members_reconciled {
            return iced::Task::none();
        }
        // Every precondition is checked before the latch closes, so a pass that
        // could not run yet leaves it open for the next trigger. This matters
        // more here than for the fingerprint PATCH: the first trigger fires from
        // `App::new`, where `server_cube_id` is still `None` — it is only set
        // when the `CubeRegistered` response lands — so the launch-time call is
        // *expected* to no-op and the registration-time call is the one that
        // usually does the work.
        let (Some(client), Some(server_id), Some(descriptor)) = (
            self.client.clone(),
            self.server_cube_id,
            self.vault_descriptor.clone(),
        ) else {
            return iced::Task::none();
        };
        self.vault_members_reconciled = true;
        let cube_uuid = self.cube_uuid.clone();
        iced::Task::perform(
            async move {
                let added = vault_reconcile::reconcile_cube_vault_members(
                    &client,
                    server_id,
                    &cube_uuid,
                    &descriptor,
                )
                .await;
                if added > 0 {
                    log::info!(
                        "[CONNECT-CUBE] reconciled {} missing Vault member row(s) for cube {}",
                        added,
                        server_id
                    );
                }
                added
            },
            |added| {
                Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::VaultMembersReconciled(added),
                ))
            },
        )
    }

    /// Re-report this Cube's Vault presence to the server when a Vault is
    /// created mid-session on an already-registered Cube, so its `hasVault`
    /// flips without waiting for a fresh registration (the duress vault gate;
    /// PLAN-duress-vault-gate PR 3). Sets the local flag, then fires an
    /// idempotent update against the known `server_cube_id`. No-op when the
    /// Cube isn't registered yet (the next `register_cube` carries the flag)
    /// or the panel has no live client.
    pub fn report_vault_created(&mut self) -> iced::Task<Message> {
        self.cube_has_vault = true;
        let (Some(client), Some(server_id)) = (self.client.clone(), self.server_cube_id) else {
            return iced::Task::none();
        };
        let req = UpdateCubeRequest {
            name: None,
            status: None,
            has_vault: Some(true),
            spark_stable_balance: None,
        };
        iced::Task::perform(
            async move {
                if let Err(e) = client.update_cube(&server_id.to_string(), req).await {
                    log::warn!("[CONNECT-CUBE] re-report has_vault after Vault creation: {e}");
                }
            },
            |()| Message::CubeVaultReported,
        )
    }

    /// Push a Spark Stable Balance decision to the Cube's server record so
    /// the owner's other desktops pick it up (the toggle, a bridge pause, or
    /// dismissing the "USDB held" banner — see
    /// `CubeSettings::spark_stable_balance`). Remembers the value either way;
    /// when the Cube isn't registered yet or there is no live client, the next
    /// `register_cube` carries it instead. Background hygiene: a failed PUT is
    /// logged, and the value rides the next registration.
    pub fn report_spark_stable_balance(&mut self, enabled: bool) -> iced::Task<Message> {
        self.spark_stable_balance = Some(enabled);
        let (Some(client), Some(server_id)) = (self.client.clone(), self.server_cube_id) else {
            return iced::Task::none();
        };
        let req = UpdateCubeRequest {
            name: None,
            status: None,
            has_vault: None,
            spark_stable_balance: Some(enabled),
        };
        iced::Task::perform(
            async move {
                if let Err(e) = client.update_cube(&server_id.to_string(), req).await {
                    log::warn!("[CONNECT-CUBE] report spark_stable_balance={enabled}: {e}");
                }
            },
            |()| Message::CubeSparkStableBalanceReported,
        )
    }

    pub fn update_message(&mut self, msg: ConnectCubeMessage) -> iced::Task<Message> {
        match msg {
            ConnectCubeMessage::FocusAction(id) => return iced::widget::operation::focus(id),
            ConnectCubeMessage::CubeRegistered { generation, result } => {
                return self.on_cube_registered(generation, result)
            }

            ConnectCubeMessage::EncryptionKeyRegistered(ok) => {
                // Background hygiene: on failure, clear the in-session latch so
                // a later trigger (e.g. the next `ensure_cube_registered`) can
                // retry rather than waiting for a relaunch.
                if !ok {
                    self.enc_pubkey_registered = false;
                }
            }

            ConnectCubeMessage::VaultMembersReconciled(_added) => {
                // Nothing to render: the reconcile is background hygiene and
                // the useful detail is already in the log. Unlike the latches
                // above there is no failure retry here — a Vault that still
                // has a missing row gets another pass at the next Cube open,
                // and the sign-time pass remains the backstop.
            }

            ConnectCubeMessage::VaultFingerprintAsserted(ok) => {
                // Same background-hygiene contract as the encryption pubkey:
                // clear the in-session latch on failure so a later trigger can
                // retry without waiting for a relaunch.
                if !ok {
                    self.vault_fingerprint_asserted = false;
                }
            }

            ConnectCubeMessage::LightningAddressLoaded {
                generation,
                address,
            } => {
                if generation != self.session_generation {
                    return iced::Task::none();
                }
                self.lightning_address = address;
            }

            ConnectCubeMessage::LnUsernameChanged(input) => {
                return self.on_ln_username_changed(input)
            }

            ConnectCubeMessage::LnUsernameChecked {
                available,
                error_message,
                version,
            } => self.on_ln_username_checked(available, error_message, version),

            ConnectCubeMessage::ClaimLightningAddress => return self.claim_lightning_address(),

            ConnectCubeMessage::AutoAssignLightningAddress { retry } => {
                return self.auto_assign_lightning_address(retry)
            }

            ConnectCubeMessage::LightningAddressAutoAssigned {
                generation,
                address,
            } => {
                if generation != self.session_generation {
                    return iced::Task::none();
                }
                self.ln_claiming = false;
                self.ln_auto = crate::app::cache::LightningAddressSetup::Idle;
                self.lightning_address = Some(address);
                self.ln_reconcile_needs_reregister = None;
            }

            ConnectCubeMessage::LightningAddressAutoAssignFailed { generation, error } => {
                if generation != self.session_generation {
                    return iced::Task::none();
                }
                log::warn!(
                    "[CONNECT-CUBE] automatic Lightning address failed: {}",
                    error
                );
                self.ln_claiming = false;
                self.ln_auto = crate::app::cache::LightningAddressSetup::Failed(error);
            }

            ConnectCubeMessage::LightningAddressClaimed {
                generation,
                address,
            } => self.on_lightning_address_claimed(generation, address),

            ConnectCubeMessage::LightningAddressClaimFailed { generation, error } => {
                self.on_lightning_address_claim_failed(generation, error)
            }

            ConnectCubeMessage::BeginEditLightningAddress => {
                if self.ln_changing {
                    return iced::Task::none();
                }
                self.ln_editing = true;
                self.ln_username_input.clear();
                self.ln_username_available = None;
                self.ln_username_error = None;
                self.ln_claim_error = None;
            }

            ConnectCubeMessage::CancelEditLightningAddress => self.cancel_edit_lightning_address(),

            ConnectCubeMessage::SaveLightningAddress => return self.save_lightning_address(),

            ConnectCubeMessage::LightningAddressUpdated {
                generation,
                outcome,
            } => self.on_lightning_address_updated(generation, outcome),

            ConnectCubeMessage::RetryLightningAddressReregister => {
                return self.retry_lightning_address_reregister()
            }

            ConnectCubeMessage::LightningAddressReregistered {
                generation,
                address,
                result,
            } => self.on_lightning_address_reregistered(generation, &address, result),

            ConnectCubeMessage::SparkLightningAddressChanged(info) => {
                return self.on_spark_lightning_address_changed(info)
            }

            ConnectCubeMessage::LightningAddressReconciled {
                generation,
                outcome,
            } if generation != self.session_generation => {
                log::debug!(
                    "[CONNECT-CUBE] discarding a Spark reconcile outcome from a \
                     previous session (generation {}, now {}): {:?}",
                    generation,
                    self.session_generation,
                    outcome
                );
            }

            ConnectCubeMessage::LightningAddressReconciled { outcome, .. } => match outcome {
                ReconcileOutcome::AlreadyBound(info) => {
                    log::info!(
                        "[CONNECT-CUBE] Spark reports lightning address {}",
                        info.lightning_address
                    );
                    self.ln_reconcile_needs_reregister = None;
                }
                ReconcileOutcome::ReRegistered(info) => {
                    log::info!(
                        "[CONNECT-CUBE] Spark re-registered lightning address {}",
                        info.lightning_address
                    );
                    self.ln_reconcile_needs_reregister = None;
                }
                ReconcileOutcome::QueryFailed(e) => {
                    log::warn!(
                        "[CONNECT-CUBE] Spark lightning-address query failed \
                         (transient, will retry on next trigger): {}",
                        e
                    );
                }
                ReconcileOutcome::NeedsReRegistration(e) => {
                    log::error!(
                        "[CONNECT-CUBE] Spark register failed during reconcile — \
                         API and SDK are out of sync until the user re-claims: {}",
                        e
                    );
                    self.ln_reconcile_needs_reregister = Some(e);
                }
            },

            ConnectCubeMessage::RetryRegistration => return self.retry_registration(),

            ConnectCubeMessage::CopyToClipboard(text) => {
                return iced::clipboard::write(text);
            }

            ConnectCubeMessage::Error(e) => self.on_connect_cube_error(e),

            ConnectCubeMessage::Avatar(avatar_msg) => {
                return self.update_avatar(avatar_msg);
            }

            ConnectCubeMessage::Members(msg) => {
                return cube_members::update(
                    &mut self.members,
                    msg,
                    self.client.clone(),
                    self.server_cube_id,
                );
            }
        }

        iced::Task::none()
    }

    fn on_cube_registered(
        &mut self,
        generation: u64,
        result: Result<crate::services::coincube::CubeResponse, CubeRegistrationError>,
    ) -> iced::Task<Message> {
        // Belongs to a session that has since ended — see
        // `session_generation`. Drop it whole: it must not populate
        // `server_cube_id`, and it must not clear a latch it no longer
        // owns (the current session may have an attempt of its own in
        // flight).
        if generation != self.session_generation {
            log::debug!(
                "[CONNECT-CUBE] discarding a registration result from a \
                 previous session (generation {}, now {})",
                generation,
                self.session_generation,
            );
            return iced::Task::none();
        }
        // Re-arm the trigger regardless of outcome: on success nothing
        // will call `register_cube` again (`server_cube_id` is set), and
        // on failure the next trigger is entitled to a fresh attempt.
        self.registering = false;
        match result {
            Ok(cube_resp) => {
                log::info!(
                    "[CONNECT-CUBE] Registered cube {} (server ID: {})",
                    cube_resp.uuid,
                    cube_resp.id
                );
                self.server_cube_id = Some(cube_resp.id);
                self.registration_error = None;
                self.owned_by_other_account = false;
                // Store the lightning address from the backend (or clear if None)
                if cube_resp.lightning_address.is_some() {
                    self.lightning_address = Some(LightningAddress {
                        lightning_address: cube_resp.lightning_address,
                    });
                } else {
                    self.lightning_address = None;
                }
                // Phase 4g: if the DB has a confirmed address,
                // reconcile against the Spark SDK's local
                // state. Covers device reinstall / SDK storage
                // wipe / multi-device identity swap.
                let reconcile_task = self.reconcile_spark_lightning_address();
                // An availability hint computed before the id existed
                // only reflects Spark; re-run it now that the API can
                // answer for this Cube.
                let recheck_task = if self.ln_check_missing_cube_id {
                    Some(self.on_ln_username_changed(self.ln_username_input.clone()))
                } else {
                    None
                };
                // Trigger avatar load now that cube is registered.
                let avatar_task = self.load_avatar_if_needed();
                // The server cube id just became known, which is the
                // only thing the encryption-pubkey PUT was waiting on.
                // Publish now so invites created in this session carry
                // the key (PLAN-connect-blinding PR D2). Skip it when
                // the server already reports the key we'd send.
                if cube_resp.encryption_pubkey.is_some()
                    && cube_resp.encryption_pubkey == self.cube_encryption_pubkey
                {
                    self.enc_pubkey_registered = true;
                }
                let enc_key_task = self.register_encryption_pubkey();
                // Same trigger, same reason: the server cube id was the
                // only thing the vault-fingerprint PATCH was waiting on.
                // Skip it when the registration response already carries
                // the value we'd send.
                if cube_resp
                    .vault
                    .as_ref()
                    .is_some_and(|v| Some(&v.fingerprint) == self.vault_fingerprint.as_ref())
                {
                    self.vault_fingerprint_asserted = true;
                }
                let vault_fp_task = self.assert_vault_fingerprint();
                // Third thing waiting on the server cube id: the
                // COIN-373 membership reconcile. This is its main
                // trigger — the backfill fires it at `App::new`, before
                // this response has landed, so without this a Vault
                // missing a member row would stay broken on the phone
                // until someone started a desktop sign.
                let reconcile_members_task = self.reconcile_vault_members();
                // The server's Spark Stable Balance record is the
                // cross-device one; hand it to the App, which owns
                // the local record and the SDK reconcile. Sent even
                // when `None` so the App can push this device's
                // decision up if the server has none.
                let stable_balance_task = iced::Task::done(Message::SparkStableBalanceFromConnect(
                    cube_resp.spark_stable_balance,
                ));
                let mut tasks = vec![
                    enc_key_task,
                    vault_fp_task,
                    reconcile_members_task,
                    stable_balance_task,
                ];
                tasks.extend(reconcile_task);
                tasks.extend(recheck_task);
                tasks.extend(avatar_task);
                return iced::Task::batch(tasks);
            }
            Err(e) => {
                log::error!("[CONNECT-CUBE] Failed to register cube: {}", e.message);
                self.owned_by_other_account = e.owned_by_other_account;
                self.registration_error = Some(e.message);
            }
        }
        iced::Task::none()
    }

    fn on_ln_username_changed(&mut self, input: String) -> iced::Task<Message> {
        self.ln_username_input = input.to_lowercase();
        self.ln_check_missing_cube_id = false;
        self.ln_username_available = None;
        self.ln_username_error = None;
        self.ln_claim_error = None;

        // Client-side validation
        if let Some(err) = validate_ln_username(&self.ln_username_input) {
            self.ln_check_version += 1;
            if let Some(handle) = self.ln_check_abort.take() {
                handle.abort();
            }
            self.ln_checking = false;
            self.ln_username_error = Some(err);
            return iced::Task::none();
        }

        let Some(spark) = self.spark_client.clone() else {
            // No Spark backend means no way to claim — skip
            // the debounced hint entirely. The claim button
            // surfaces the same "Spark unavailable" error.
            self.ln_check_version += 1;
            if let Some(handle) = self.ln_check_abort.take() {
                handle.abort();
            }
            self.ln_checking = false;
            return iced::Task::none();
        };

        // Debounced availability check. Two sources, ANDed:
        // the Breez-hosted LNURL server (catches names
        // registered outside our DB) and our own Go API
        // (authoritative for @coincube.io — the same conflict
        // source the reserve step hits, including unconfirmed
        // or orphaned reservations Breez has never heard of;
        // Breez-only checking showed "Available" for names the
        // reserve then 409'd). The API result is still just a
        // hint — reserve remains the authoritative gate — so
        // an API *error* falls back to the Breez-only answer
        // rather than blocking typing. Abort any previous
        // in-flight task.
        if let Some(handle) = self.ln_check_abort.take() {
            handle.abort();
        }
        self.ln_check_version += 1;
        let version = self.ln_check_version;
        let username = self.ln_username_input.clone();
        let api_client = self.client.clone();
        let api_cube_id = self.api_cube_id();
        self.ln_check_missing_cube_id = api_cube_id.is_none();
        self.ln_checking = true;
        let (task, abort_handle) = iced::Task::perform(
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let res = spark
                    .check_lightning_address_available(username.clone())
                    .await;
                let res = match res {
                    Ok(true) => Ok(check_connect_ln_username_availability(
                        api_client,
                        api_cube_id,
                        &username,
                    )
                    .await),
                    other => other,
                };
                (res, version)
            },
            move |(res, v)| match res {
                Ok(available) => Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::LnUsernameChecked {
                        available,
                        error_message: if available {
                            None
                        } else {
                            Some("Username is taken".to_string())
                        },
                        version: v,
                    },
                )),
                Err(e) => Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::LnUsernameChecked {
                        available: false,
                        error_message: Some(e.to_string()),
                        version: v,
                    },
                )),
            },
        )
        .abortable();
        self.ln_check_abort = Some(abort_handle);
        task
    }

    fn on_ln_username_checked(
        &mut self,
        available: bool,
        error_message: Option<String>,
        version: u32,
    ) {
        if version == self.ln_check_version {
            self.ln_checking = false;
            self.ln_username_available = Some(available);
            if !available {
                self.ln_username_error =
                    Some(error_message.unwrap_or_else(|| "Username is taken".to_string()));
            }
        }
    }

    fn claim_lightning_address(&mut self) -> iced::Task<Message> {
        if self.ln_claiming {
            return iced::Task::none();
        }
        let Some(client) = self.client.clone() else {
            return iced::Task::none();
        };
        let Some(spark) = self.spark_client.clone() else {
            self.ln_claim_error = Some("Spark wallet is not available on this cube".to_string());
            return iced::Task::none();
        };
        self.ln_claiming = true;
        self.ln_claim_error = None;
        let generation = self.session_generation;
        let username = self.ln_username_input.clone();
        let Some(cube_id) = self.api_cube_id() else {
            self.ln_claiming = false;
            self.ln_claim_error = Some(
                self.registration_error
                    .clone()
                    .unwrap_or_else(|| "Cube registration pending".to_string()),
            );
            return iced::Task::none();
        };
        iced::Task::perform(
            async move {
                claim_username(&client, &spark, &cube_id, &username)
                    .await
                    .map_err(|e| e.message)
            },
            move |res| match res {
                Ok(ln_addr) => Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::LightningAddressClaimed {
                        generation,
                        address: ln_addr,
                    },
                )),
                Err(error) => Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::LightningAddressClaimFailed { generation, error },
                )),
            },
        )
    }

    fn auto_assign_lightning_address(&mut self, retry: bool) -> iced::Task<Message> {
        use crate::app::cache::LightningAddressSetup;
        // Only once the server has answered for this Cube (so an empty
        // address really is empty), never over an address, a claim in
        // flight or a running attempt, and after a failure only when
        // the user asks again.
        let has_address = self.lightning_address.as_ref().is_some_and(|la| {
            crate::services::coincube::display_lightning_address(la.lightning_address.as_deref())
                .is_some()
        });
        let blocked = match &self.ln_auto {
            LightningAddressSetup::Assigning => true,
            LightningAddressSetup::Failed(_) => !retry,
            LightningAddressSetup::Idle => false,
        };
        if has_address
            || blocked
            || self.ln_claiming
            || self.ln_changing
            || self.server_cube_id.is_none()
        {
            return iced::Task::none();
        }
        let (Some(client), Some(spark), Some(cube_id)) = (
            self.client.clone(),
            self.spark_client.clone(),
            self.api_cube_id(),
        ) else {
            return iced::Task::none();
        };
        self.ln_auto = LightningAddressSetup::Assigning;
        self.ln_claiming = true;
        let generation = self.session_generation;
        iced::Task::perform(
            async move { auto_claim(&client, &spark, &cube_id).await },
            move |res| match res {
                Ok(address) => Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::LightningAddressAutoAssigned {
                        generation,
                        address,
                    },
                )),
                Err(error) => Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::LightningAddressAutoAssignFailed { generation, error },
                )),
            },
        )
    }

    fn on_lightning_address_claimed(&mut self, generation: u64, address: LightningAddress) {
        if generation != self.session_generation {
            return;
        }
        self.ln_claiming = false;
        self.lightning_address = Some(address);
        self.ln_username_input.clear();
        self.ln_username_available = None;
        self.ln_username_error = None;
        self.ln_reconcile_needs_reregister = None;
    }

    fn on_lightning_address_claim_failed(&mut self, generation: u64, error: String) {
        if generation != self.session_generation {
            return;
        }
        self.ln_claiming = false;
        self.ln_claim_error = Some(error);
    }

    fn cancel_edit_lightning_address(&mut self) {
        if self.ln_changing {
            return;
        }
        self.ln_editing = false;
        self.ln_username_input.clear();
        self.ln_username_available = None;
        self.ln_username_error = None;
        self.ln_claim_error = None;
        self.ln_check_version += 1;
        if let Some(handle) = self.ln_check_abort.take() {
            handle.abort();
        }
        self.ln_check_missing_cube_id = false;
        self.ln_checking = false;
    }

    fn save_lightning_address(&mut self) -> iced::Task<Message> {
        if self.ln_changing {
            return iced::Task::none();
        }
        let new_username = self.ln_username_input.trim().to_string();
        let valid_format = self.ln_username_error.is_none() && !new_username.is_empty();
        let available = self.ln_username_available == Some(true);
        let current_username = self
            .lightning_address
            .as_ref()
            .and_then(|la| la.lightning_address.as_deref())
            .and_then(|addr| addr.split('@').next());
        let differs = current_username.is_none_or(|u| u != new_username);
        if !(valid_format && available && differs) {
            return iced::Task::none();
        }
        let Some(client) = self.client.clone() else {
            return iced::Task::none();
        };
        let Some(spark) = self.spark_client.clone() else {
            self.ln_claim_error = Some("Spark wallet is not available on this cube".to_string());
            return iced::Task::none();
        };
        let Some(cube_id) = self.api_cube_id() else {
            self.ln_claim_error = Some(
                self.registration_error
                    .clone()
                    .unwrap_or_else(|| "Cube registration pending".to_string()),
            );
            return iced::Task::none();
        };
        self.ln_changing = true;
        self.ln_claim_error = None;
        let generation = self.session_generation;
        iced::Task::perform(
            async move {
                // Step 1: server-side atomic username swap.
                // Only the DB row changes — no DNS work.
                let new_addr = match client
                    .update_lightning_address(&cube_id, &new_username)
                    .await
                {
                    Ok(addr) => addr,
                    Err(e) => {
                        return LightningAddressChangeOutcome::ServerError(format!("{}", e));
                    }
                };

                // Step 2: release the old SDK binding from
                // Breez. Required because the SDK's register
                // call doesn't replace an existing binding —
                // see the reconciler at
                // `reconcile_spark_lightning_address`.
                if let Err(e) = spark.delete_lightning_address().await {
                    log::error!(
                        "[CONNECT-CUBE] change: SDK delete failed after \
                         server commit (cube={}, new={}): {}",
                        cube_id,
                        new_username,
                        e
                    );
                    return LightningAddressChangeOutcome::SdkSyncFailed {
                        addr: new_addr,
                        message: format!(
                            "Could not release the previous Lightning Address \
                             binding on this device: {}. Tap Retry to finish \
                             switching to {}.",
                            e, new_username
                        ),
                    };
                }

                // Step 3: bind the new username on the
                // Breez-hosted LNURL server.
                let info = match spark
                    .register_lightning_address(new_username.clone(), None)
                    .await
                {
                    Ok(info) => info,
                    Err(e) => {
                        log::error!(
                            "[CONNECT-CUBE] change: SDK register failed after \
                             server commit + SDK delete (cube={}, new={}): {}",
                            cube_id,
                            new_username,
                            e
                        );
                        return LightningAddressChangeOutcome::SdkSyncFailed {
                            addr: new_addr,
                            message: format!(
                                "Could not register {} with the Lightning Address \
                                 server: {}. Tap Retry to finish.",
                                new_username, e
                            ),
                        };
                    }
                };

                // Same domain-drift guard as the reconciler: the
                // SDK's `lnurl_domain` is fixed at init, so a
                // successful register can still bind a different
                // `user@domain` than the server committed.
                rename_outcome(new_addr, &info.lightning_address)
            },
            move |outcome| {
                Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::LightningAddressUpdated {
                        generation,
                        outcome,
                    },
                ))
            },
        )
    }

    fn on_lightning_address_updated(
        &mut self,
        generation: u64,
        outcome: LightningAddressChangeOutcome,
    ) {
        if generation != self.session_generation {
            return;
        }
        self.ln_changing = false;
        match outcome {
            LightningAddressChangeOutcome::Ok(addr) => {
                self.lightning_address = Some(addr);
                self.ln_editing = false;
                self.ln_username_input.clear();
                self.ln_username_available = None;
                self.ln_username_error = None;
                self.ln_claim_error = None;
                self.ln_reconcile_needs_reregister = None;
            }
            LightningAddressChangeOutcome::ServerError(msg) => {
                // Stay in edit mode so the user can correct
                // and retry; the address itself is unchanged.
                self.ln_claim_error = Some(msg);
            }
            LightningAddressChangeOutcome::SdkSyncFailed { addr, message } => {
                // Server committed; mirror that locally and
                // surface the existing re-registration prompt
                // so the user can retry the SDK side.
                self.lightning_address = Some(addr);
                self.ln_editing = false;
                self.ln_username_input.clear();
                self.ln_username_available = None;
                self.ln_username_error = None;
                self.ln_claim_error = None;
                self.ln_reconcile_needs_reregister = Some(message);
            }
        }
    }

    fn retry_lightning_address_reregister(&mut self) -> iced::Task<Message> {
        if self.ln_reregistering {
            return iced::Task::none();
        }
        let Some(spark) = self.spark_client.clone() else {
            return iced::Task::none();
        };
        let Some(db_addr) = self
            .lightning_address
            .as_ref()
            .and_then(|la| la.lightning_address.as_ref())
            .cloned()
        else {
            return iced::Task::none();
        };
        // Same malformed-record guard as
        // `reconcile_spark_lightning_address`: without
        // `@domain` in the DB record, the post-register
        // comparison against the SDK's full `user@domain`
        // can never match, and the user would be stuck in
        // a retry loop they can't clear.
        let db_username = match db_addr.split('@').next() {
            Some(u) if !u.is_empty() && db_addr.contains('@') => u.to_string(),
            _ => {
                log::warn!(
                    "[CONNECT-CUBE] skipping retry rebind: malformed DB \
                     lightning address {:?} (expected user@domain)",
                    db_addr
                );
                self.ln_reconcile_needs_reregister = Some(coincube_ui::localized_format!(
                    "Stored lightning address {:?} is malformed \
                     (expected user@domain) — please re-claim a username",
                    "La dirección Lightning guardada {:?} no tiene un formato válido \
                     (se esperaba usuario@dominio). Vuelve a reclamar un nombre de usuario.",
                    db_addr
                ));
                return iced::Task::none();
            }
        };
        self.ln_reregistering = true;
        let generation = self.session_generation;
        if let Some(handle) = self.ln_reregister_abort.take() {
            handle.abort();
        }
        let address = db_addr;
        let (task, abort_handle) = iced::Task::perform(
            async move {
                // Idempotent: tolerates an already-empty SDK.
                // Ignore failure here — the register that
                // follows is the authoritative success
                // signal, and a Breez-side "already gone"
                // error shouldn't block recovery.
                if let Err(e) = spark.delete_lightning_address().await {
                    log::warn!(
                        "[CONNECT-CUBE] retry rebind: SDK delete \
                         returned {} (continuing to register)",
                        e
                    );
                }
                spark
                    .register_lightning_address(db_username, None)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |result| {
                Message::View(view::Message::ConnectCube(
                    ConnectCubeMessage::LightningAddressReregistered {
                        generation,
                        address,
                        result,
                    },
                ))
            },
        )
        .abortable();
        self.ln_reregister_abort = Some(abort_handle);
        task
    }

    fn on_lightning_address_reregistered(
        &mut self,
        generation: u64,
        issued_for: &str,
        result: Result<LightningAddressInfo, String>,
    ) {
        if generation != self.session_generation {
            return;
        }
        self.ln_reregistering = false;
        self.ln_reregister_abort = None;
        let db_addr = self
            .lightning_address
            .as_ref()
            .and_then(|la| la.lightning_address.as_deref());
        if db_addr != Some(issued_for) {
            // A rename committed while this retry was in flight. The
            // result describes the previous reservation, and the
            // rename's own outcome decides the prompt for the new one.
            log::debug!(
                "[CONNECT-CUBE] discarding a rebind result for {:?}; the \
                 confirmed reservation is now {:?}",
                issued_for,
                db_addr
            );
            return;
        }
        match result {
            Ok(info) => {
                if db_addr == Some(info.lightning_address.as_str()) {
                    self.ln_reconcile_needs_reregister = None;
                    log::info!(
                        "[CONNECT-CUBE] manual rebind succeeded: {}",
                        info.lightning_address
                    );
                } else {
                    // Domain-drift guard, mirroring the
                    // reconciler. Don't claim "rebound" when
                    // the SDK's full address doesn't match
                    // the DB record.
                    self.ln_reconcile_needs_reregister = Some(format!(
                        "Spark SDK registered '{}' but the confirmed \
                         reservation is '{}'",
                        info.lightning_address,
                        db_addr.unwrap_or("")
                    ));
                }
            }
            Err(e) => {
                self.ln_reconcile_needs_reregister = Some(e);
            }
        }
    }

    fn on_spark_lightning_address_changed(
        &mut self,
        info: Option<LightningAddressInfo>,
    ) -> iced::Task<Message> {
        match info {
            Some(info) => {
                // A `Some` payload means the SDK observed a
                // register/change — on this device at the
                // tail of the claim flow, or cross-device
                // via realtime-sync replay. Only treat it
                // as authoritative when it matches the
                // DB-confirmed reservation; a mismatched
                // payload means the SDK holds a binding we
                // haven't confirmed server-side (pre-confirm
                // claim-flow race, cross-device identity
                // swap, stale cache), and mirroring it to
                // the display would show the wrong address.
                let db_addr = self
                    .lightning_address
                    .as_ref()
                    .and_then(|la| la.lightning_address.as_deref());
                if db_addr == Some(info.lightning_address.as_str()) {
                    log::info!(
                        "[CONNECT-CUBE] Spark lightning address confirmed: {}",
                        info.lightning_address
                    );
                    // SDK matches the DB — any stale "needs
                    // re-registration" state is resolved.
                    self.ln_reconcile_needs_reregister = None;
                } else {
                    log::warn!(
                        "[CONNECT-CUBE] Spark reports {:?} but DB record \
                         is {:?} — triggering reconcile",
                        info.lightning_address,
                        db_addr
                    );
                    if let Some(task) = self.reconcile_spark_lightning_address() {
                        return task;
                    }
                }
            }
            None if self.ln_changing => {
                // A rename's own SDK delete emits this event between
                // the server commit and its register call. Rebinding
                // the stored (old) name here would race that register
                // and make it fail; the rename's outcome decides which
                // name the SDK binds.
                log::debug!(
                    "[CONNECT-CUBE] Spark lightning address cleared during a \
                     rename — leaving the rebind to the rename"
                );
            }
            None => {
                // If the DB still has a confirmed username
                // (i.e. the user didn't initiate the delete
                // on this device), trigger the same
                // auto-reconcile path as on startup so the
                // address rebinds without user action.
                if let Some(task) = self.reconcile_spark_lightning_address() {
                    return task;
                }
            }
        }
        iced::Task::none()
    }

    fn retry_registration(&mut self) -> iced::Task<Message> {
        if self.registering {
            return iced::Task::none();
        }
        if self.client.is_none() {
            // A restored connect.json session may have registered
            // through App::EnsureConnectReady without a panel client.
            // Keep the failure visible while that path retries from
            // the cached tokens; it clears on CubeRegistered success.
            return iced::Task::done(Message::EnsureConnectReady);
        }
        self.registration_error = None;
        self.register_cube()
    }

    fn on_connect_cube_error(&mut self, e: String) {
        log::error!("[CONNECT-CUBE] Error: {}", e);
        if self.ln_claiming {
            self.ln_claim_error = Some(e);
            self.ln_claiming = false;
        } else if self.ln_checking {
            self.ln_username_error = Some(e);
            self.ln_checking = false;
        } else {
            self.ln_claim_error = Some(e);
        }
    }

    fn update_avatar(&mut self, msg: AvatarMessage) -> iced::Task<Message> {
        // Results of requests issued under a previous session must not
        // populate this account's avatar state or start follow-ups
        // through the current client.
        let result_generation = match &msg {
            AvatarMessage::Loaded { generation, .. }
            | AvatarMessage::GenerateComplete { generation, .. }
            | AvatarMessage::VariantSelected { generation, .. }
            | AvatarMessage::RegenerationsLoaded { generation, .. }
            | AvatarMessage::ImageLoaded { generation, .. } => Some(*generation),
            _ => None,
        };
        if let Some(generation) = result_generation {
            if generation != self.session_generation {
                log::debug!(
                    "[AVATAR] discarding a result from a previous session \
                     (generation {}, now {})",
                    generation,
                    self.session_generation
                );
                return iced::Task::none();
            }
        }
        match msg {
            AvatarMessage::Enter => return self.enter_avatar(),

            AvatarMessage::Loaded { result, .. } => return self.on_avatar_loaded(result),

            AvatarMessage::SetStep(step) => {
                self.avatar_step = step;
            }

            AvatarMessage::GenderChanged(v) => self.avatar_draft.gender = v,
            AvatarMessage::ArchetypeChanged(v) => self.avatar_draft.archetype = v,
            AvatarMessage::AgeFeelChanged(v) => self.avatar_draft.age_feel = v,
            AvatarMessage::DemeanorChanged(v) => self.avatar_draft.demeanor = v,
            AvatarMessage::ArmorStyleChanged(v) => self.avatar_draft.armor_style = v,
            AvatarMessage::AccentMotifChanged(v) => self.avatar_draft.accent_motif = v,
            AvatarMessage::LaserEyesToggled(v) => self.avatar_draft.laser_eyes = v,

            AvatarMessage::Generate => return self.generate_avatar(),

            AvatarMessage::GenerateComplete { result, .. } => {
                return self.on_avatar_generate_complete(result)
            }

            AvatarMessage::SelectVariant(variant_id) => {
                return self.select_avatar_variant(variant_id)
            }

            AvatarMessage::VariantSelected { result, .. } => {
                return self.on_avatar_variant_selected(result)
            }

            AvatarMessage::RegenerationsLoaded { result, .. } => match result {
                Ok(data) => {
                    if let Some(ref mut ad) = self.avatar_data {
                        ad.regenerations_remaining = data.remaining;
                    }
                }
                Err(e) => {
                    log::warn!("[AVATAR] Regenerations fetch error: {}", e);
                }
            },

            AvatarMessage::ImageLoaded {
                variant_id, result, ..
            } => match result {
                Ok(bytes) => {
                    let handle = iced::widget::image::Handle::from_bytes(bytes.clone());
                    self.avatar_image_cache.insert(variant_id, (bytes, handle));
                }
                Err(e) => {
                    log::warn!(
                        "[AVATAR] Image load error for variant {}: {}",
                        variant_id,
                        e
                    );
                }
            },

            AvatarMessage::Retry => {
                self.avatar_error = None;
                self.avatar_step = AvatarFlowStep::Questionnaire;
            }

            AvatarMessage::DownloadAvatar => return self.download_avatar(),

            AvatarMessage::SaveError(e) => {
                log::error!("[AVATAR] Failed to save avatar to disk: {}", e);
                self.avatar_error = Some(e);
            }

            AvatarMessage::Noop => {}
        }

        iced::Task::none()
    }
    fn enter_avatar(&mut self) -> iced::Task<Message> {
        self.avatar_error = None;
        let Some(client) = self.client.clone() else {
            self.avatar_error = Some("Not signed in".to_string());
            return iced::Task::none();
        };
        let Some(cid) = self.api_cube_id() else {
            if let Some(ref e) = self.registration_error {
                self.avatar_error = Some(e.clone());
            }
            return iced::Task::none();
        };
        let generation = self.session_generation;
        iced::Task::perform(async move { client.get_avatar(&cid).await }, move |res| {
            Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                AvatarMessage::Loaded {
                    generation,
                    result: res.map_err(|e| e.to_string()),
                },
            )))
        })
    }

    fn on_avatar_loaded(
        &mut self,
        result: Result<crate::services::coincube::GetAvatarData, String>,
    ) -> iced::Task<Message> {
        match result {
            Ok(data) => {
                let has = data.has_avatar;
                let active_id = active_avatar_variant_id(&data);
                self.avatar_data = Some(data);
                if has {
                    self.avatar_step = AvatarFlowStep::Settings;
                    if let Some(id) = active_id {
                        return self.fetch_avatar_image_if_missing(id);
                    }
                } else {
                    self.avatar_step = AvatarFlowStep::Questionnaire;
                }
            }
            Err(e) => {
                log::error!("[AVATAR] Load error: {}", e);
                self.avatar_error = Some(e);
            }
        }
        iced::Task::none()
    }

    fn generate_avatar(&mut self) -> iced::Task<Message> {
        if self.avatar_generating {
            return iced::Task::none();
        }
        let Some(client) = self.client.clone() else {
            self.avatar_error = Some("Not signed in".to_string());
            return iced::Task::none();
        };
        let req = AvatarGenerateRequest {
            user_traits: self.avatar_draft.clone(),
        };
        let Some(cid) = self.api_cube_id() else {
            self.avatar_generating = false;
            self.avatar_error = Some(
                self.registration_error
                    .clone()
                    .unwrap_or_else(|| "Cube registration pending".to_string()),
            );
            return iced::Task::none();
        };
        self.avatar_generating = true;
        self.avatar_error = None;
        self.avatar_step = AvatarFlowStep::Generating;
        let generation = self.session_generation;
        iced::Task::perform(
            async move { client.post_avatar_generate(&cid, req).await },
            move |res| {
                Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                    AvatarMessage::GenerateComplete {
                        generation,
                        result: res.map_err(|e| e.to_string()),
                    },
                )))
            },
        )
    }

    fn on_avatar_generate_complete(
        &mut self,
        result: Result<crate::services::coincube::AvatarGenerateData, String>,
    ) -> iced::Task<Message> {
        self.avatar_generating = false;
        match result {
            Ok(data) => {
                let variant_id = data.variant.id;
                let new_variant = data.variant.clone();
                if let Some(ref mut ad) = self.avatar_data {
                    ad.has_avatar = true;
                    ad.active_avatar_url = Some(new_variant.image_url.clone());
                    if !ad.variants.iter().any(|v| v.id == new_variant.id) {
                        ad.variants.push(new_variant);
                    }
                    ad.identity = Some(data.identity);
                    // Decrement local regeneration count
                    if ad.regenerations_remaining > 0 {
                        ad.regenerations_remaining -= 1;
                    }
                } else {
                    self.avatar_data = Some(crate::services::coincube::GetAvatarData {
                        has_avatar: true,
                        active_avatar_url: Some(data.variant.image_url.clone()),
                        identity: Some(data.identity),
                        variants: vec![data.variant],
                        regenerations_remaining: 0,
                        created_at: None,
                        updated_at: None,
                    });
                }
                self.avatar_step = AvatarFlowStep::Reveal;
                if let Some(ref ad) = self.avatar_data {
                    if let Some(ref identity) = ad.identity {
                        self.avatar_draft = identity.user_traits.clone();
                    }
                }
                return self.fetch_generated_avatar_details(variant_id);
            }
            Err(e) => {
                log::error!("[AVATAR] Generate error: {}", e);
                self.avatar_error = Some(e);
                self.avatar_step = AvatarFlowStep::Questionnaire;
            }
        }
        iced::Task::none()
    }

    fn fetch_generated_avatar_details(&self, variant_id: u64) -> iced::Task<Message> {
        // Fetch image + refresh regeneration count in parallel
        if let Some(client) = self.client.clone() {
            let client2 = client.clone();
            let Some(cid) = self.api_cube_id() else {
                return iced::Task::none();
            };
            let generation = self.session_generation;
            return iced::Task::batch([
                iced::Task::perform(
                    async move { client.fetch_avatar_image(variant_id).await },
                    move |res| {
                        Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                            AvatarMessage::ImageLoaded {
                                generation,
                                variant_id,
                                result: res.map_err(|e| e.to_string()),
                            },
                        )))
                    },
                ),
                iced::Task::perform(
                    async move { client2.get_avatar_regenerations(&cid).await },
                    move |res| {
                        Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                            AvatarMessage::RegenerationsLoaded {
                                generation,
                                result: res.map_err(|e| e.to_string()),
                            },
                        )))
                    },
                ),
            ]);
        }
        iced::Task::none()
    }

    fn select_avatar_variant(&mut self, variant_id: u64) -> iced::Task<Message> {
        let Some(client) = self.client.clone() else {
            self.avatar_error = Some("Not signed in".to_string());
            return iced::Task::none();
        };
        let Some(cid) = self.api_cube_id() else {
            self.avatar_error = Some(
                self.registration_error
                    .clone()
                    .unwrap_or_else(|| "Cube registration pending".to_string()),
            );
            return iced::Task::none();
        };
        let generation = self.session_generation;
        iced::Task::perform(
            async move {
                client
                    .post_avatar_select(&cid, AvatarSelectRequest { variant_id })
                    .await
            },
            move |res| {
                Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                    AvatarMessage::VariantSelected {
                        generation,
                        result: res.map_err(|e| e.to_string()),
                    },
                )))
            },
        )
    }

    fn on_avatar_variant_selected(
        &mut self,
        result: Result<crate::services::coincube::AvatarSelectData, String>,
    ) -> iced::Task<Message> {
        match result {
            Ok(data) => {
                if let Some(ref mut ad) = self.avatar_data {
                    ad.active_avatar_url = Some(data.active_avatar_url);
                }
                let variant_id = data.variant_id;
                return self.fetch_avatar_image_if_missing(variant_id);
            }
            Err(e) => {
                log::error!("[AVATAR] Select error: {}", e);
                self.avatar_error = Some(e);
            }
        }
        iced::Task::none()
    }

    fn fetch_avatar_image_if_missing(&self, variant_id: u64) -> iced::Task<Message> {
        if !self.avatar_image_cache.contains_key(&variant_id) {
            if let Some(client) = self.client.clone() {
                let generation = self.session_generation;
                return iced::Task::perform(
                    async move { client.fetch_avatar_image(variant_id).await },
                    move |res| {
                        Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                            AvatarMessage::ImageLoaded {
                                generation,
                                variant_id,
                                result: res.map_err(|e| e.to_string()),
                            },
                        )))
                    },
                );
            }
        }
        iced::Task::none()
    }

    fn download_avatar(&self) -> iced::Task<Message> {
        let active_id = self.avatar_data.as_ref().and_then(active_avatar_variant_id);
        if let Some(id) = active_id {
            if let Some((bytes, _)) = self.avatar_image_cache.get(&id) {
                let bytes = bytes.clone();
                return iced::Task::perform(
                    async move {
                        let Some(handle) = rfd::AsyncFileDialog::new()
                            .set_title("Save Avatar")
                            .set_file_name("coincube-avatar.png")
                            .add_filter("PNG Image", &["png"])
                            .save_file()
                            .await
                        else {
                            return Ok(());
                        };
                        std::fs::write(handle.path(), &bytes).map_err(|e| e.to_string())
                    },
                    |res| match res {
                        Ok(()) => Message::View(view::Message::ConnectCube(
                            ConnectCubeMessage::Avatar(AvatarMessage::Noop),
                        )),
                        Err(e) => Message::View(view::Message::ConnectCube(
                            ConnectCubeMessage::Avatar(AvatarMessage::SaveError(e)),
                        )),
                    },
                );
            }
        }
        iced::Task::none()
    }
}

async fn rollback_reconcile_domain_mismatch(
    spark: &SparkClient,
    info: &LightningAddressInfo,
    db_addr: &str,
) -> ReconcileOutcome {
    // Roll back the orphan binding we
    // just created on the LNURL server
    // — leaving it would squat the
    // wrong-domain record forever.
    // Best-effort: log if the cleanup
    // itself fails so an operator can
    // remove it manually.
    let bound = info.lightning_address.clone();
    let mut suffix = "";
    if let Err(e) = spark.delete_lightning_address().await {
        log::error!(
            "[CONNECT-CUBE] failed to roll back \
             orphan Spark registration {:?} after \
             reconcile domain mismatch (expected {:?}): {}",
            bound,
            db_addr,
            e
        );
        suffix = " (orphan SDK registration left behind \
                  — please contact support)";
    }
    ReconcileOutcome::NeedsReRegistration(format!(
        "Spark SDK registered '{}' but the confirmed \
         reservation is '{}'{}",
        bound, db_addr, suffix
    ))
}

/// The variant named by `active_avatar_url`, matched on the URL's last path
/// segment (without extension) so `.../112.png` never resolves to variant
/// `12`.
fn active_avatar_variant_id(data: &crate::services::coincube::GetAvatarData) -> Option<u64> {
    let url = data.active_avatar_url.as_deref()?;
    let active_id = url.rsplit('/').next()?.split('.').next()?;
    data.variants
        .iter()
        .find(|v| v.id.to_string() == active_id)
        .map(|v| v.id)
}

/// Outcome of a rename whose SDK register succeeded: success only when the
/// SDK bound exactly the server-committed `user@domain`.
fn rename_outcome(committed: LightningAddress, spark_bound: &str) -> LightningAddressChangeOutcome {
    if committed.lightning_address.as_deref() == Some(spark_bound) {
        return LightningAddressChangeOutcome::Ok(committed);
    }
    log::error!(
        "[CONNECT-CUBE] change: Spark SDK bound {:?} but the server \
         committed {:?}",
        spark_bound,
        committed.lightning_address
    );
    let message = format!(
        "Spark SDK registered '{}' but the confirmed reservation is '{}'. \
         Tap Retry to finish.",
        spark_bound,
        committed.lightning_address.as_deref().unwrap_or("")
    );
    LightningAddressChangeOutcome::SdkSyncFailed {
        addr: committed,
        message,
    }
}

async fn check_connect_ln_username_availability(
    api_client: Option<CoincubeClient>,
    api_cube_id: Option<String>,
    username: &str,
) -> bool {
    if let (Some(client), Some(cube_id)) = (api_client, api_cube_id) {
        match client
            .check_lightning_address_available(&cube_id, username)
            .await
        {
            Ok(api_available) => api_available,
            Err(e) => {
                log::warn!(
                    "[CONNECT-CUBE] API availability check \
                     failed, falling back to Breez-only \
                     hint: {}",
                    e
                );
                true
            }
        }
    } else {
        true
    }
}

/// Validate a lightning address username client-side, with the server's
/// rules (coincube-api `lightning_address.go`): lowercase letters, digits and
/// `._-`, starting and ending with a letter or digit, no two of `._-` in a
/// row. Input is lowercased as it is typed, so uppercase never reaches here
/// from the form; it is still rejected rather than silently accepted.
/// Reserved names are only known to the server and come back from the
/// availability check.
fn validate_ln_username(username: &str) -> Option<String> {
    if username.is_empty() {
        return Some("Username is required".to_string());
    }
    if username.len() < 3 {
        return Some("Username must be at least 3 characters".to_string());
    }
    if username.len() > 64 {
        return Some("Username must be at most 64 characters".to_string());
    }
    let plain = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    if !username.chars().next().is_some_and(plain) {
        return Some("Must start with a letter or number".to_string());
    }
    if !username.chars().last().is_some_and(plain) {
        return Some("Must end with a letter or number".to_string());
    }
    let special = ['.', '_', '-'];
    for c in username.chars() {
        if !plain(c) && !special.contains(&c) {
            return Some(coincube_ui::localized_format!(
                "Invalid character: '{}'",
                "Carácter no válido: '{}'",
                c
            ));
        }
    }
    let chars: Vec<char> = username.chars().collect();
    for w in chars.windows(2) {
        if special.contains(&w[0]) && special.contains(&w[1]) {
            return Some("No consecutive special characters allowed".to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use coincube_spark_protocol::LightningAddressInfo;

    use crate::{
        app::view::AvatarMessage,
        services::coincube::{
            AvatarAccentMotif, AvatarAgeFeel, AvatarArmorStyle, AvatarDemeanor,
            AvatarDerivedTraits, AvatarGender, AvatarGenerateData, AvatarIdentity,
            AvatarResolvedDirectives, AvatarSelectData, AvatarVariant, CubeResponse, GetAvatarData,
            RegenerationData,
        },
    };

    use std::str::FromStr;

    // Primary signer `f5acc2fd`, CSV recovery signer `8a64f2a9`.
    const RECONCILE_DESC: &str = "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr";

    fn panel() -> ConnectCubePanel {
        ConnectCubePanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "bitcoin".to_string(),
            false,
        )
    }

    #[test]
    fn random_lightning_names_are_valid_usernames() {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        for _ in 0..500 {
            let plain = random_ln_username(&mut rng, false);
            assert!(validate_ln_username(&plain).is_none(), "{}", plain);
            assert!(plain.chars().all(|c| c.is_ascii_lowercase()), "{}", plain);
            let numbered = random_ln_username(&mut rng, true);
            assert!(validate_ln_username(&numbered).is_none(), "{}", numbered);
            let digits: String = numbered.chars().rev().take(2).collect();
            assert!(digits.chars().all(|c| c.is_ascii_digit()), "{}", numbered);
        }
        // Enough combinations that a collision retry rarely repeats a name.
        assert!(LN_ADJECTIVES.len() * LN_ANIMALS.len() > 3_000);
    }

    /// Automatic assignment only starts for a registered Cube whose address
    /// is known to be empty, and a failure waits for an explicit retry.
    #[test]
    fn automatic_lightning_address_is_guarded_and_settles_its_state() {
        use crate::app::cache::LightningAddressSetup;
        let mut panel = panel();
        // Not registered with the server yet: the empty address isn't known.
        let _ =
            panel.update_message(ConnectCubeMessage::AutoAssignLightningAddress { retry: false });
        assert_eq!(panel.ln_auto, LightningAddressSetup::Idle);
        assert!(!panel.ln_claiming);

        // Registered, but no API client or Spark: nothing can run.
        panel.server_cube_id = Some(7);
        let _ =
            panel.update_message(ConnectCubeMessage::AutoAssignLightningAddress { retry: true });
        assert_eq!(panel.ln_auto, LightningAddressSetup::Idle);

        // A "NULL" address counts as none, a real one blocks it.
        panel.lightning_address = Some(lightning("orangezebra@coincube.io"));
        panel.set_client(CoincubeClient::new());
        let _ =
            panel.update_message(ConnectCubeMessage::AutoAssignLightningAddress { retry: true });
        assert_eq!(panel.ln_auto, LightningAddressSetup::Idle);

        // A failed attempt is reported, and only a retry may run again.
        panel.ln_auto = LightningAddressSetup::Assigning;
        panel.ln_claiming = true;
        let generation = panel.session_generation;
        let _ = panel.update_message(ConnectCubeMessage::LightningAddressAutoAssignFailed {
            generation,
            error: "Reserve failed: offline".to_string(),
        });
        assert_eq!(
            panel.ln_auto,
            LightningAddressSetup::Failed("Reserve failed: offline".to_string())
        );
        assert!(!panel.ln_claiming);
        panel.lightning_address = None;
        let _ =
            panel.update_message(ConnectCubeMessage::AutoAssignLightningAddress { retry: false });
        assert!(matches!(panel.ln_auto, LightningAddressSetup::Failed(_)));

        // A result from an earlier session is ignored; the current one lands.
        let _ = panel.update_message(ConnectCubeMessage::LightningAddressAutoAssigned {
            generation: generation.wrapping_add(1),
            address: lightning("stale@coincube.io"),
        });
        assert!(panel.lightning_address.is_none());
        let _ = panel.update_message(ConnectCubeMessage::LightningAddressAutoAssigned {
            generation,
            address: lightning("orangezebra"),
        });
        assert_eq!(panel.ln_auto, LightningAddressSetup::Idle);
        assert_eq!(
            panel
                .lightning_address
                .as_ref()
                .and_then(|la| la.lightning_address.as_deref()),
            Some("orangezebra")
        );
    }

    fn lightning(address: &str) -> LightningAddress {
        LightningAddress {
            lightning_address: Some(address.to_string()),
        }
    }

    fn lightning_info(address: &str) -> LightningAddressInfo {
        let username = address.split('@').next().unwrap_or(address).to_string();
        LightningAddressInfo {
            lightning_address: address.to_string(),
            username: username.clone(),
            description: Some("Coincube".to_string()),
            lnurl_url: format!("https://example.com/.well-known/lnurlp/{username}"),
            lnurl_bech32: "lnurl1test".to_string(),
        }
    }

    fn cube_response(lightning_address: Option<&str>) -> CubeResponse {
        CubeResponse {
            encryption_pubkey: None,
            id: 42,
            uuid: "cube-uuid".to_string(),
            name: "Family Vault".to_string(),
            network: "bitcoin".to_string(),
            lightning_address: lightning_address.map(str::to_string),
            status: "active".to_string(),
            has_recovery_kit: false,
            has_vault: Some(true),
            spark_stable_balance: None,
            members: Vec::new(),
            pending_invites: Vec::new(),
            vault: None,
            connect_features_disabled: false,
        }
    }

    #[test]
    fn replacing_account_client_retires_old_address_before_new_cube_registration() {
        let mut panel = panel();
        panel.set_client(CoincubeClient::for_test("http://account-a.invalid"));
        panel.server_cube_id = Some(42);
        panel.lightning_address = Some(lightning("account-a@coincube.io"));
        panel.cube_encryption_pubkey = Some("synthetic-device-pubkey".to_owned());
        panel.enc_pubkey_registered = true;
        let old_generation = panel.session_generation();

        panel.set_client(CoincubeClient::for_test("http://account-b.invalid"));
        assert!(panel.server_cube_id.is_none());
        assert!(panel.lightning_address.is_none());
        assert!(!panel.enc_pubkey_registered);
        assert_ne!(panel.session_generation(), old_generation);

        // A's delayed registration cannot repopulate the new account's view.
        let _ = panel.update_message(ConnectCubeMessage::CubeRegistered {
            generation: old_generation,
            result: Ok(cube_response(Some("account-a@coincube.io"))),
        });
        assert!(panel.lightning_address.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressClaimed {
            generation: old_generation,
            address: lightning("account-a@coincube.io"),
        });
        let _ = panel.update_message(ConnectCubeMessage::LightningAddressClaimFailed {
            generation: old_generation,
            error: "old account claim failed".to_owned(),
        });
        let _ = panel.update_message(ConnectCubeMessage::LightningAddressUpdated {
            generation: old_generation,
            outcome: LightningAddressChangeOutcome::Ok(lightning("account-a@coincube.io")),
        });
        let _ = panel.update_message(ConnectCubeMessage::LightningAddressUpdated {
            generation: old_generation,
            outcome: LightningAddressChangeOutcome::SdkSyncFailed {
                addr: lightning("account-a@coincube.io"),
                message: "old account".to_owned(),
            },
        });
        let _ = panel.update_message(ConnectCubeMessage::LightningAddressLoaded {
            generation: old_generation,
            address: Some(lightning("account-a@coincube.io")),
        });
        assert!(panel.lightning_address.is_none());
        assert!(panel.ln_reconcile_needs_reregister.is_none());
        assert!(panel.ln_claim_error.is_none());

        let _ = panel.update_message(ConnectCubeMessage::CubeRegistered {
            generation: panel.session_generation(),
            result: Ok(cube_response(Some("account-b@coincube.io"))),
        });
        assert!(panel.enc_pubkey_registered);
        assert_eq!(
            panel
                .lightning_address
                .as_ref()
                .and_then(|value| value.lightning_address.as_deref()),
            Some("account-b@coincube.io")
        );
    }

    fn avatar_variant(id: u64) -> AvatarVariant {
        AvatarVariant {
            id,
            index: id as u32,
            image_url: format!("https://cdn.example/avatar/{id}.png"),
        }
    }

    fn avatar_data(has_avatar: bool, active_id: Option<u64>) -> GetAvatarData {
        GetAvatarData {
            has_avatar,
            active_avatar_url: active_id.map(|id| format!("https://cdn.example/avatar/{id}.png")),
            identity: None,
            variants: vec![avatar_variant(7), avatar_variant(8)],
            regenerations_remaining: 3,
            created_at: None,
            updated_at: None,
        }
    }

    fn identity() -> AvatarIdentity {
        AvatarIdentity {
            version: 1,
            seed_version: 1,
            seed_hash: "seed-hash".to_string(),
            lightning_address: "founder@example.com".to_string(),
            archetype: "ronin".to_string(),
            user_traits: AvatarUserTraits {
                gender: AvatarGender::Woman,
                archetype: crate::services::coincube::AvatarArchetype::Shogun,
                age_feel: AvatarAgeFeel::Elder,
                demeanor: AvatarDemeanor::Fierce,
                armor_style: AvatarArmorStyle::Heavy,
                accent_motif: AvatarAccentMotif::OrangeSun,
                laser_eyes: true,
            },
            derived_traits: AvatarDerivedTraits {
                pose: "front".to_string(),
                crop_style: "portrait".to_string(),
                hat_style: "none".to_string(),
                face_visibility: "visible".to_string(),
                eye_visibility: "visible".to_string(),
                weapon_mode: "none".to_string(),
                shoulder_profile: "square".to_string(),
                cloak_presence: "none".to_string(),
                armor_wear: "clean".to_string(),
                enso_style: "brush".to_string(),
                ink_density: "medium".to_string(),
                brush_texture: "dry".to_string(),
                splash_intensity: "low".to_string(),
                orange_placement: "accent".to_string(),
                ornament_level: "simple".to_string(),
            },
            resolved_directives: AvatarResolvedDirectives {
                composition: "composition".to_string(),
                silhouette: "silhouette".to_string(),
                face_treatment: "face".to_string(),
                armor_treatment: "armor".to_string(),
                mood: "mood".to_string(),
                orange_treatment: "orange".to_string(),
                ink_treatment: "ink".to_string(),
                eyes_treatment: "eyes".to_string(),
                background: "background".to_string(),
                archetype_flavor: "flavor".to_string(),
            },
        }
    }

    /// COIN-373 latch semantics. The reconcile's first trigger is the App's
    /// backfill at `App::new`, which runs before the `CubeRegistered` response
    /// has set `server_cube_id` — so that pass must NOT consume the latch, or
    /// the whole cube-open reconcile becomes dead code and a Vault missing a
    /// member row stays broken on the phone until someone starts a desktop sign.
    #[test]
    fn reconcile_waits_for_the_server_cube_id_before_latching() {
        let mut panel = panel();
        panel.set_client(CoincubeClient::for_test("http://127.0.0.1:1"));
        panel.vault_descriptor = Some(CoincubeDescriptor::from_str(RECONCILE_DESC).unwrap());

        // Launch-time pass: registered? not yet.
        let _ = panel.reconcile_vault_members();
        assert!(
            !panel.vault_members_reconciled,
            "a pass with no server cube id must leave the latch open"
        );

        panel.server_cube_id = Some(42);
        let _ = panel.reconcile_vault_members();
        assert!(
            panel.vault_members_reconciled,
            "with client, cube id and descriptor in hand the pass must run"
        );
    }

    /// ...and the trigger that supplies that server cube id must carry the
    /// reconcile with it. Dropping this call is what makes the launch-time pass
    /// the only one, which is to say none at all.
    #[test]
    fn cube_registered_fires_the_membership_reconcile() {
        let mut panel = panel();
        panel.set_client(CoincubeClient::for_test("http://127.0.0.1:1"));
        panel.vault_descriptor = Some(CoincubeDescriptor::from_str(RECONCILE_DESC).unwrap());
        assert_eq!(panel.server_cube_id, None);

        let _ = panel.update_message(ConnectCubeMessage::CubeRegistered {
            generation: panel.session_generation(),
            result: Ok(registered_cube()),
        });

        assert_eq!(panel.server_cube_id, Some(42));
        assert!(
            panel.vault_members_reconciled,
            "registration is the reconcile's real trigger — the App-level pass \
             already ran and no-opped before this response landed"
        );
    }

    fn registered_cube() -> crate::services::coincube::CubeResponse {
        crate::services::coincube::CubeResponse {
            id: 42,
            uuid: "cube-uuid".to_string(),
            name: "Family Vault".to_string(),
            network: "bitcoin".to_string(),
            lightning_address: None,
            status: "active".to_string(),
            has_recovery_kit: false,
            has_vault: Some(true),
            spark_stable_balance: None,
            encryption_pubkey: None,
            members: vec![],
            pending_invites: vec![],
            vault: None,
            connect_features_disabled: false,
        }
    }

    #[test]
    fn lightning_username_validation_covers_client_side_rules() {
        assert_eq!(
            validate_ln_username("").as_deref(),
            Some("Username is required")
        );
        assert_eq!(
            validate_ln_username("ab").as_deref(),
            Some("Username must be at least 3 characters")
        );
        assert_eq!(
            validate_ln_username(&"a".repeat(65)).as_deref(),
            Some("Username must be at most 64 characters")
        );
        assert_eq!(
            validate_ln_username("-abc").as_deref(),
            Some("Must start with a letter or number")
        );
        assert_eq!(
            validate_ln_username("abc_").as_deref(),
            Some("Must end with a letter or number")
        );
        assert_eq!(
            validate_ln_username("ab$c").as_deref(),
            Some("Invalid character: '$'")
        );
        assert_eq!(
            validate_ln_username("ab..c").as_deref(),
            Some("No consecutive special characters allowed")
        );
        assert!(validate_ln_username("abc-123_def.xyz").is_none());
        // The server only accepts lowercase; the form lowercases as you type.
        assert!(validate_ln_username("Abc").is_some());
        assert!(validate_ln_username("abC").is_some());
        assert!(validate_ln_username("aBc").is_some());
    }

    /// `ConnectPanel::update` calls `register_cube` on every account message
    /// that arrives while `server_cube_id` is `None`, and it stays `None` for as
    /// long as the server refuses (an account at its Cube limit refuses
    /// forever). Without the in-flight guard one session check fanned out into
    /// three concurrent POST /connect/cubes, and every later account message did
    /// it again.
    #[test]
    fn a_registration_in_flight_is_not_duplicated() {
        let mut panel = panel();
        panel.set_client(CoincubeClient::new());

        let first = panel.register_cube();
        assert!(panel.registering);
        assert!(
            iced_runtime::task::into_stream(first).is_some(),
            "the first trigger must actually issue the request"
        );

        assert!(
            iced_runtime::task::into_stream(panel.register_cube()).is_none(),
            "a trigger arriving while the request is in flight must be dropped"
        );

        // A refusal has to re-arm the guard, not latch the Cube out of ever
        // registering: the reason may well be transient (a slot frees up).
        let _ = panel.update_message(ConnectCubeMessage::CubeRegistered {
            generation: panel.session_generation(),
            result: Err("Cube limit reached for this network".to_string().into()),
        });
        assert!(!panel.registering);
        assert!(
            iced_runtime::task::into_stream(panel.register_cube()).is_some(),
            "the next trigger after a failure must be allowed to retry"
        );
    }

    #[test]
    fn retry_registration_without_panel_client_uses_external_path_and_keeps_error() {
        use iced_runtime::futures::futures::{executor::block_on, StreamExt};

        let mut panel = panel();
        panel.registration_error = Some("register failed".into());
        let task = panel.update_message(ConnectCubeMessage::RetryRegistration);
        assert_eq!(panel.registration_error.as_deref(), Some("register failed"));
        let mut stream = iced_runtime::task::into_stream(task).expect("external retry emitted");
        assert!(matches!(
            block_on(stream.next()),
            Some(iced_runtime::Action::Output(Message::EnsureConnectReady))
        ));

        panel.registering = true;
        assert!(iced_runtime::task::into_stream(
            panel.update_message(ConnectCubeMessage::RetryRegistration)
        )
        .is_none());
        assert_eq!(panel.registration_error.as_deref(), Some("register failed"));
    }

    /// Signing out mid-registration must not strand the next account. Nothing
    /// aborts the in-flight request, so its reply arrives after the switch —
    /// and if it were applied, `server_cube_id` would name a row in the
    /// *previous* account, which `ensure_cube_registered` then treats as "this
    /// Cube is already registered" forever.
    #[test]
    fn a_registration_does_not_outlive_the_session_that_issued_it() {
        let mut panel = panel();
        panel.set_client(CoincubeClient::new());

        let stale = panel.session_generation();
        let _ = panel.register_cube();
        assert!(panel.registering);

        // Sign out mid-flight, then sign in as someone else.
        panel.clear_client();
        assert!(
            !panel.registering,
            "sign-out must release the latch rather than leaving the next \
             account waiting on a reply it will never accept"
        );
        panel.set_client(CoincubeClient::new());

        // The new account registers immediately — it is not blocked by the
        // attempt the previous session left in flight.
        assert!(
            iced_runtime::task::into_stream(panel.register_cube()).is_some(),
            "the new session must be able to register straight away"
        );

        // The previous session's reply now lands. It must be discarded whole:
        // no server cube id, and no clearing of the latch it does not own.
        let _ = panel.update_message(ConnectCubeMessage::CubeRegistered {
            generation: stale,
            result: Ok(registered_cube()),
        });
        assert_eq!(
            panel.server_cube_id, None,
            "a previous account's cube row must never be adopted by this session"
        );
        assert!(
            panel.registering,
            "the stale reply must not clear the current attempt's latch"
        );

        // This session's own reply is applied as normal.
        let _ = panel.update_message(ConnectCubeMessage::CubeRegistered {
            generation: panel.session_generation(),
            result: Ok(registered_cube()),
        });
        assert_eq!(panel.server_cube_id, Some(42));
        assert!(!panel.registering);
    }

    /// `App::EnsureConnectReady` issues its own registration when this panel has
    /// no client, so it claims the same guard rather than running alongside a
    /// panel-issued attempt: both POSTs carry one Cube UUID, and the server
    /// answers the loser with an error that would overwrite the winner's
    /// success on screen.
    #[test]
    fn an_externally_issued_registration_shares_the_in_flight_guard() {
        let mut panel = panel();

        // The out-of-panel caller claims it first: a panel trigger arriving
        // while that request is in flight is dropped.
        let generation = panel
            .begin_external_registration()
            .expect("nothing in flight yet");
        assert_eq!(generation, panel.session_generation());
        panel.set_client(CoincubeClient::new());
        assert!(
            iced_runtime::task::into_stream(panel.register_cube()).is_none(),
            "the panel must not register alongside an externally issued attempt"
        );

        // ... and a second external attempt is dropped too.
        assert!(
            panel.begin_external_registration().is_none(),
            "a second press must not fire a duplicate registration"
        );

        // The reply releases the guard through the normal path, on either arm.
        let _ = panel.update_message(ConnectCubeMessage::CubeRegistered {
            generation,
            result: Err("register failed".to_string().into()),
        });
        assert!(panel.begin_external_registration().is_some());
    }

    /// A Stable Balance decision made before the Cube is registered (or
    /// while signed out) can't be PUT anywhere yet — but it must not be
    /// lost: the panel keeps it so the next registration carries it.
    #[test]
    fn spark_stable_balance_report_is_kept_for_registration_when_it_cannot_be_sent() {
        let mut panel = panel();
        assert_eq!(panel.spark_stable_balance, None);

        // No client, no server id: nothing to send, value remembered.
        assert!(
            iced_runtime::task::into_stream(panel.report_spark_stable_balance(false)).is_none()
        );
        assert_eq!(panel.spark_stable_balance, Some(false));

        // Client but no server id (registration hasn't answered): same.
        panel.set_client(CoincubeClient::new());
        assert!(iced_runtime::task::into_stream(panel.report_spark_stable_balance(true)).is_none());
        assert_eq!(panel.spark_stable_balance, Some(true));

        // Once registered, the report becomes a real PUT.
        panel.server_cube_id = Some(7);
        assert!(
            iced_runtime::task::into_stream(panel.report_spark_stable_balance(false)).is_some()
        );
        assert_eq!(panel.spark_stable_balance, Some(false));
    }

    #[test]
    fn registration_client_and_avatar_helpers_reset_session_state() {
        let mut panel = panel();
        assert!(panel.api_cube_id().is_none());
        assert!(panel.load_avatar_if_needed().is_none());
        let (_, handle) = panel.register_cube().abortable();
        handle.abort();

        panel.set_client(CoincubeClient::new());
        panel.server_cube_id = Some(42);
        assert_eq!(panel.api_cube_id().as_deref(), Some("42"));
        assert!(panel.load_avatar_if_needed().is_some());
        let (_, handle) = panel.register_cube().abortable();
        handle.abort();
        let _ = panel.report_vault_created();
        assert!(panel.cube_has_vault);

        panel.avatar_data = Some(avatar_data(true, Some(7)));
        panel.avatar_image_cache.insert(
            7,
            (
                vec![137, 80, 78, 71],
                iced::widget::image::Handle::from_bytes(vec![137, 80, 78, 71]),
            ),
        );
        assert!(panel.get_active_avatar_handle().is_some());
        if let Some(data) = panel.avatar_data.as_mut() {
            data.active_avatar_url = Some("https://cdn.example/avatar/70.png".to_string());
        }
        assert!(panel.get_active_avatar_handle().is_none());

        panel.ln_username_input = "founder".to_string();
        panel.ln_username_available = Some(true);
        panel.ln_username_error = Some("old".to_string());
        panel.ln_claim_error = Some("old".to_string());
        panel.lightning_address = Some(lightning("founder@example.com"));
        panel.ln_reconcile_needs_reregister = Some("retry".to_string());
        panel.avatar_error = Some("old".to_string());
        panel.clear_client();

        assert!(panel.client.is_none());
        assert!(panel.server_cube_id.is_none());
        assert!(panel.lightning_address.is_none());
        assert!(panel.ln_username_input.is_empty());
        assert!(panel.ln_username_available.is_none());
        assert!(panel.ln_username_error.is_none());
        assert!(panel.ln_claim_error.is_none());
        assert!(panel.ln_reconcile_needs_reregister.is_none());
        assert!(panel.avatar_data.is_none());
        assert!(panel.avatar_image_cache.is_empty());
    }

    #[test]
    fn lightning_address_state_machine_handles_non_network_branches() {
        let mut panel = panel();

        let _ = panel.update_message(ConnectCubeMessage::CubeRegistered {
            generation: panel.session_generation(),
            result: Ok(cube_response(Some("founder@example.com"))),
        });
        assert_eq!(panel.server_cube_id, Some(42));
        assert_eq!(
            panel
                .lightning_address
                .as_ref()
                .and_then(|a| a.lightning_address.as_deref()),
            Some("founder@example.com")
        );

        let _ = panel.update_message(ConnectCubeMessage::CubeRegistered {
            generation: panel.session_generation(),
            result: Err("register failed".to_string().into()),
        });
        assert_eq!(panel.registration_error.as_deref(), Some("register failed"));

        let _ = panel.update_message(ConnectCubeMessage::LnUsernameChanged("Bad$".to_string()));
        assert_eq!(panel.ln_username_input, "bad$");
        assert!(panel.ln_username_error.is_some());
        assert!(!panel.ln_checking);

        let _ = panel.update_message(ConnectCubeMessage::LnUsernameChanged(
            "Fresh_Name".to_string(),
        ));
        assert_eq!(panel.ln_username_input, "fresh_name");
        assert!(panel.ln_username_error.is_none());
        assert!(!panel.ln_checking);

        let stale_version = panel.ln_check_version + 1;
        let _ = panel.update_message(ConnectCubeMessage::LnUsernameChecked {
            available: false,
            error_message: Some("taken".to_string()),
            version: stale_version,
        });
        assert!(panel.ln_username_available.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LnUsernameChecked {
            available: false,
            error_message: None,
            version: panel.ln_check_version,
        });
        assert_eq!(panel.ln_username_available, Some(false));
        assert_eq!(
            panel.ln_username_error.as_deref(),
            Some("Username is taken")
        );

        let _ = panel.update_message(ConnectCubeMessage::ClaimLightningAddress);
        assert!(!panel.ln_claiming);

        panel.set_client(CoincubeClient::new());
        let _ = panel.update_message(ConnectCubeMessage::ClaimLightningAddress);
        assert_eq!(
            panel.ln_claim_error.as_deref(),
            Some("Spark wallet is not available on this cube")
        );

        let _ = panel.update_message(ConnectCubeMessage::BeginEditLightningAddress);
        assert!(panel.ln_editing);
        panel.ln_username_input = "newname".to_string();
        panel.ln_username_available = Some(true);
        panel.ln_username_error = None;
        // Save sends the change in one step, with no confirmation (here
        // reaching the Spark check, as no Spark client is set).
        assert!(panel.ln_claim_error.is_none());
        let _ = panel.update_message(ConnectCubeMessage::SaveLightningAddress);
        assert_eq!(
            panel.ln_claim_error.as_deref(),
            Some("Spark wallet is not available on this cube")
        );
        // An unchanged or invalid name sends nothing.
        panel.ln_claim_error = None;
        panel.ln_username_error = Some("Must end with a letter or number".into());
        let _ = panel.update_message(ConnectCubeMessage::SaveLightningAddress);
        assert!(panel.ln_claim_error.is_none() && !panel.ln_changing);
        panel.ln_username_error = None;
        let _ = panel.update_message(ConnectCubeMessage::CancelEditLightningAddress);
        assert!(!panel.ln_editing);
        assert!(panel.ln_username_input.is_empty());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressUpdated {
            generation: panel.session_generation(),
            outcome: LightningAddressChangeOutcome::ServerError("conflict".to_string()),
        });
        assert_eq!(panel.ln_claim_error.as_deref(), Some("conflict"));

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressUpdated {
            generation: panel.session_generation(),
            outcome: LightningAddressChangeOutcome::SdkSyncFailed {
                addr: lightning("new@example.com"),
                message: "sdk failed".to_string(),
            },
        });
        assert_eq!(
            panel.ln_reconcile_needs_reregister.as_deref(),
            Some("sdk failed")
        );

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressUpdated {
            generation: panel.session_generation(),
            outcome: LightningAddressChangeOutcome::Ok(lightning("ok@example.com")),
        });
        assert!(panel.ln_reconcile_needs_reregister.is_none());
        assert!(!panel.ln_editing);
    }

    #[test]
    fn reconciliation_messages_update_prompt_state_without_spark() {
        let mut panel = panel();
        panel.lightning_address = Some(lightning("founder@example.com"));

        let _ = panel.update_message(ConnectCubeMessage::SparkLightningAddressChanged(Some(
            lightning_info("founder@example.com"),
        )));
        assert!(panel.ln_reconcile_needs_reregister.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReconciled {
            generation: panel.session_generation(),
            outcome: ReconcileOutcome::AlreadyBound(lightning_info("founder@example.com")),
        });
        assert!(panel.ln_reconcile_needs_reregister.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReconciled {
            generation: panel.session_generation(),
            outcome: ReconcileOutcome::ReRegistered(lightning_info("founder@example.com")),
        });
        assert!(panel.ln_reconcile_needs_reregister.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReconciled {
            generation: panel.session_generation(),
            outcome: ReconcileOutcome::QueryFailed("offline".to_string()),
        });
        assert!(panel.ln_reconcile_needs_reregister.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReconciled {
            generation: panel.session_generation(),
            outcome: ReconcileOutcome::NeedsReRegistration("needs retry".to_string()),
        });
        assert_eq!(
            panel.ln_reconcile_needs_reregister.as_deref(),
            Some("needs retry")
        );

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReregistered {
            generation: panel.session_generation(),
            address: "founder@example.com".to_string(),
            result: Ok(lightning_info("founder@example.com")),
        });
        assert!(panel.ln_reconcile_needs_reregister.is_none());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReregistered {
            generation: panel.session_generation(),
            address: "founder@example.com".to_string(),
            result: Ok(lightning_info("wrong@example.com")),
        });
        assert!(panel.ln_reconcile_needs_reregister.is_some());

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReregistered {
            generation: panel.session_generation(),
            address: "founder@example.com".to_string(),
            result: Err("still down".to_string()),
        });
        assert_eq!(
            panel.ln_reconcile_needs_reregister.as_deref(),
            Some("still down")
        );
    }

    #[test]
    fn late_reregistration_failure_cannot_change_the_replacement_account() {
        let mut panel = panel();
        panel.set_client(CoincubeClient::for_test("http://account-a.invalid"));
        let old_generation = panel.session_generation();

        panel.set_client(CoincubeClient::for_test("http://account-b.invalid"));
        let generation = panel.session_generation();
        assert_ne!(generation, old_generation);
        panel.lightning_address = Some(lightning("account-b@coincube.io"));
        panel.ln_reconcile_needs_reregister = Some("account B needs retry".to_owned());
        panel.ln_reregistering = true;

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReregistered {
            generation: old_generation,
            address: "account-a@coincube.io".to_string(),
            result: Err("account A failed".to_owned()),
        });
        assert!(panel.ln_reregistering);
        assert_eq!(
            panel.ln_reconcile_needs_reregister.as_deref(),
            Some("account B needs retry")
        );
        assert_eq!(
            panel
                .lightning_address
                .as_ref()
                .and_then(|address| address.lightning_address.as_deref()),
            Some("account-b@coincube.io")
        );

        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReregistered {
            generation,
            address: "account-b@coincube.io".to_string(),
            result: Ok(lightning_info("account-b@coincube.io")),
        });
        assert!(!panel.ln_reregistering);
        assert!(panel.ln_reconcile_needs_reregister.is_none());
    }

    #[test]
    fn avatar_state_machine_handles_local_transitions_and_results() {
        let mut panel = panel();

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Enter));
        assert_eq!(panel.avatar_error.as_deref(), Some("Not signed in"));

        panel.set_client(CoincubeClient::new());
        panel.registration_error = Some("registration pending".to_string());
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Enter));
        assert_eq!(panel.avatar_error.as_deref(), Some("registration pending"));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Loaded {
            generation: panel.session_generation(),
            result: Ok(avatar_data(false, None)),
        }));
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Questionnaire));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Loaded {
            generation: panel.session_generation(),
            result: Ok(avatar_data(true, Some(7))),
        }));
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Settings));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Loaded {
            generation: panel.session_generation(),
            result: Err("load failed".to_string()),
        }));
        assert_eq!(panel.avatar_error.as_deref(), Some("load failed"));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::SetStep(
            AvatarFlowStep::Reveal,
        )));
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Reveal));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::GenderChanged(
            AvatarGender::Woman,
        )));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::AgeFeelChanged(
            AvatarAgeFeel::Young,
        )));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::DemeanorChanged(
            AvatarDemeanor::Calm,
        )));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(
            AvatarMessage::ArmorStyleChanged(AvatarArmorStyle::Standard),
        ));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(
            AvatarMessage::AccentMotifChanged(AvatarAccentMotif::Seal),
        ));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::LaserEyesToggled(
            true,
        )));
        assert_eq!(panel.avatar_draft.gender, AvatarGender::Woman);
        assert_eq!(panel.avatar_draft.age_feel, AvatarAgeFeel::Young);
        assert!(panel.avatar_draft.laser_eyes);

        panel.avatar_generating = true;
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Generate));
        assert!(panel.avatar_generating);

        panel.avatar_generating = false;
        panel.client = None;
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Generate));
        assert_eq!(panel.avatar_error.as_deref(), Some("Not signed in"));

        panel.set_client(CoincubeClient::new());
        panel.server_cube_id = None;
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Generate));
        assert_eq!(panel.avatar_error.as_deref(), Some("registration pending"));

        let generated = AvatarGenerateData {
            identity: identity(),
            variant: avatar_variant(9),
        };
        panel.avatar_data = Some(avatar_data(true, Some(7)));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(
            AvatarMessage::GenerateComplete {
                generation: panel.session_generation(),
                result: Ok(generated),
            },
        ));
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Reveal));
        assert_eq!(panel.avatar_draft.gender, AvatarGender::Woman);
        assert_eq!(
            panel.avatar_data.as_ref().unwrap().regenerations_remaining,
            2
        );

        let _ = panel.update_message(ConnectCubeMessage::Avatar(
            AvatarMessage::GenerateComplete {
                generation: panel.session_generation(),
                result: Err("generation failed".to_string()),
            },
        ));
        assert_eq!(panel.avatar_error.as_deref(), Some("generation failed"));
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Questionnaire));

        panel.client = None;
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::SelectVariant(8)));
        assert_eq!(panel.avatar_error.as_deref(), Some("Not signed in"));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::VariantSelected {
            generation: panel.session_generation(),
            result: Ok(AvatarSelectData {
                active_avatar_url: "https://cdn.example/avatar/8.png".to_string(),
                variant_id: 8,
            }),
        }));
        assert_eq!(
            panel
                .avatar_data
                .as_ref()
                .unwrap()
                .active_avatar_url
                .as_deref(),
            Some("https://cdn.example/avatar/8.png")
        );

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::VariantSelected {
            generation: panel.session_generation(),
            result: Err("select failed".to_string()),
        }));
        assert_eq!(panel.avatar_error.as_deref(), Some("select failed"));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(
            AvatarMessage::RegenerationsLoaded {
                generation: panel.session_generation(),
                result: Ok(RegenerationData {
                    total_allowed: 5,
                    used: 4,
                    remaining: 1,
                }),
            },
        ));
        assert_eq!(
            panel.avatar_data.as_ref().unwrap().regenerations_remaining,
            1
        );

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::ImageLoaded {
            generation: panel.session_generation(),
            variant_id: 8,
            result: Ok(vec![137, 80, 78, 71]),
        }));
        assert!(panel.avatar_image_cache.contains_key(&8));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::SaveError(
            "disk full".to_string(),
        )));
        assert_eq!(panel.avatar_error.as_deref(), Some("disk full"));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Retry));
        assert!(panel.avatar_error.is_none());
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Questionnaire));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Noop));
    }

    #[test]
    fn active_avatar_lookup_matches_the_exact_variant_id() {
        let mut data = avatar_data(true, Some(112));
        data.variants = vec![avatar_variant(12), avatar_variant(112)];
        assert_eq!(active_avatar_variant_id(&data), Some(112));
        data.active_avatar_url = Some("https://cdn.example/avatar/112".to_string());
        assert_eq!(active_avatar_variant_id(&data), Some(112));
        data.active_avatar_url = Some("https://cdn.example/avatar/12.png".to_string());
        assert_eq!(active_avatar_variant_id(&data), Some(12));
        data.active_avatar_url = Some("https://cdn.example/avatar/2.png".to_string());
        assert_eq!(active_avatar_variant_id(&data), None);
        data.active_avatar_url = None;
        assert_eq!(active_avatar_variant_id(&data), None);

        // Only variant 12's bytes are cached: the sidebar handle for an
        // active 112 must not fall back to them.
        let mut panel = panel();
        let mut data = avatar_data(true, Some(112));
        data.variants = vec![avatar_variant(12), avatar_variant(112)];
        panel.avatar_data = Some(data);
        panel.avatar_image_cache.insert(
            12,
            (vec![1], iced::widget::image::Handle::from_bytes(vec![1u8])),
        );
        assert!(panel.get_active_avatar_handle().is_none());
        assert!(iced_runtime::task::into_stream(panel.download_avatar()).is_none());
    }

    #[test]
    fn avatar_results_from_a_previous_session_are_dropped() {
        let mut panel = panel();
        panel.set_client(CoincubeClient::for_test("http://account-a.invalid"));
        let stale = panel.session_generation();
        panel.set_client(CoincubeClient::for_test("http://account-b.invalid"));
        assert_ne!(stale, panel.session_generation());

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::Loaded {
            generation: stale,
            result: Ok(avatar_data(true, Some(7))),
        }));
        assert!(panel.avatar_data.is_none());
        assert!(matches!(panel.avatar_step, AvatarFlowStep::Idle));

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::ImageLoaded {
            generation: stale,
            variant_id: 7,
            result: Ok(vec![137, 80, 78, 71]),
        }));
        assert!(panel.avatar_image_cache.is_empty());

        panel.avatar_generating = true;
        let _ = panel.update_message(ConnectCubeMessage::Avatar(
            AvatarMessage::GenerateComplete {
                generation: stale,
                result: Err("old account".to_string()),
            },
        ));
        assert!(panel.avatar_generating);
        assert!(panel.avatar_error.is_none());

        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::VariantSelected {
            generation: stale,
            result: Err("old account".to_string()),
        }));
        assert!(panel.avatar_error.is_none());

        panel.avatar_data = Some(avatar_data(true, Some(7)));
        let _ = panel.update_message(ConnectCubeMessage::Avatar(
            AvatarMessage::RegenerationsLoaded {
                generation: stale,
                result: Ok(RegenerationData {
                    total_allowed: 5,
                    used: 5,
                    remaining: 0,
                }),
            },
        ));
        assert_eq!(
            panel.avatar_data.as_ref().unwrap().regenerations_remaining,
            3
        );

        // The current session's results still land.
        let _ = panel.update_message(ConnectCubeMessage::Avatar(AvatarMessage::ImageLoaded {
            generation: panel.session_generation(),
            variant_id: 7,
            result: Ok(vec![137, 80, 78, 71]),
        }));
        assert!(panel.avatar_image_cache.contains_key(&7));
    }

    #[test]
    fn reconcile_outcome_from_a_previous_session_is_dropped() {
        let mut panel = panel();
        let stale = panel.session_generation();
        panel.clear_client();
        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReconciled {
            generation: stale,
            outcome: ReconcileOutcome::NeedsReRegistration("old account".to_string()),
        });
        assert!(panel.ln_reconcile_needs_reregister.is_none());

        panel.ln_reconcile_needs_reregister = Some("current".to_string());
        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReconciled {
            generation: stale,
            outcome: ReconcileOutcome::AlreadyBound(lightning_info("old@coincube.io")),
        });
        assert_eq!(
            panel.ln_reconcile_needs_reregister.as_deref(),
            Some("current")
        );
    }

    #[test]
    fn rename_succeeds_only_when_spark_binds_the_committed_address() {
        assert!(matches!(
            rename_outcome(lightning("alice@coincube.io"), "alice@coincube.io"),
            LightningAddressChangeOutcome::Ok(_)
        ));
        match rename_outcome(lightning("alice@coincube.io"), "alice@staging.coincube.io") {
            LightningAddressChangeOutcome::SdkSyncFailed { addr, message } => {
                assert_eq!(addr.lightning_address.as_deref(), Some("alice@coincube.io"));
                assert!(message.contains("alice@staging.coincube.io"));
            }
            other => panic!("expected SdkSyncFailed, got {:?}", other),
        }
        assert!(matches!(
            rename_outcome(
                LightningAddress {
                    lightning_address: None
                },
                "alice@coincube.io"
            ),
            LightningAddressChangeOutcome::SdkSyncFailed { .. }
        ));
    }

    #[test]
    fn rebind_result_for_a_renamed_reservation_is_dropped() {
        let mut panel = panel();
        // The retry was issued for `old@`, then a rename committed `new@`.
        panel.lightning_address = Some(lightning("new@coincube.io"));
        panel.ln_reregistering = true;
        let _ = panel.update_message(ConnectCubeMessage::LightningAddressReregistered {
            generation: panel.session_generation(),
            address: "old@coincube.io".to_string(),
            result: Ok(lightning_info("old@coincube.io")),
        });
        assert!(!panel.ln_reregistering);
        assert!(panel.ln_reconcile_needs_reregister.is_none());
    }

    #[test]
    fn clear_client_aborts_a_pending_rebind() {
        let mut panel = panel();
        let (_task, handle) = iced::Task::<Message>::none().abortable();
        let observer = handle.clone();
        panel.ln_reregister_abort = Some(handle);
        panel.clear_client();
        assert!(observer.is_aborted());
        assert!(panel.ln_reregister_abort.is_none());
    }

    #[test]
    fn registration_reruns_an_availability_check_issued_without_a_cube_id() {
        let mut panel = panel();
        panel.ln_username_input = "founder".to_string();
        panel.ln_username_available = Some(true);
        panel.ln_check_missing_cube_id = true;
        let _ = panel.update_message(ConnectCubeMessage::CubeRegistered {
            generation: panel.session_generation(),
            result: Ok(cube_response(None)),
        });
        // The Spark-only hint is retired; without a Spark client here the
        // re-run stops before issuing a request.
        assert!(!panel.ln_check_missing_cube_id);
        assert!(panel.ln_username_available.is_none());
        assert_eq!(panel.ln_username_input, "founder");
    }
}
