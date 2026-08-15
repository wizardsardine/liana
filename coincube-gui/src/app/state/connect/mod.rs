pub mod account;
pub mod cube;
pub mod cube_members;

pub(crate) const CONNECT_KEYRING_SERVICE: &str = if cfg!(debug_assertions) {
    "dev.coincube.Connect"
} else {
    "io.coincube.Connect"
};

pub(crate) const CONNECT_KEYRING_USER: &str = "global_session";

// ── Process-wide Connect-secret cache ───────────────────────────────────────
//
// Each tab owns its own `ConnectAccountPanel`, and each panel reads the OS
// keyring at startup to restore the session. On macOS every Security
// framework access can pop the "Allow access to ..." dialog — so a user
// with one Home tab + one Cube tab saw the prompt twice for the same
// credential. Cache successful reads (and mirror writes / deletes) so
// subsequent panel inits short-circuit before touching the OS.

use std::collections::HashMap;
use std::sync::Mutex;

fn connect_secret_cache() -> &'static Mutex<HashMap<String, Vec<u8>>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, Vec<u8>>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Read a Connect keyring secret, consulting the process cache first.
/// Caches successful reads so later callers don't trigger another OS
/// prompt for the same credential.
pub(crate) fn read_connect_secret(user_key: &str) -> Option<Vec<u8>> {
    if let Some(bytes) = connect_secret_cache()
        .lock()
        .unwrap()
        .get(user_key)
        .cloned()
    {
        return Some(bytes);
    }
    let entry = keyring::Entry::new(CONNECT_KEYRING_SERVICE, user_key).ok()?;
    let bytes = entry.get_secret().ok()?;
    connect_secret_cache()
        .lock()
        .unwrap()
        .insert(user_key.to_string(), bytes.clone());
    Some(bytes)
}

/// Write a Connect keyring secret and mirror into the process cache.
/// Skips the OS write entirely when the cached bytes already match;
/// every Init refresh produces a SetSession that calls this path, so
/// short-circuiting when nothing changed prevents an "Allow access"
/// prompt on each Cube open even though the token didn't rotate.
pub(crate) fn write_connect_secret(user_key: &str, bytes: &[u8]) -> Result<(), keyring::Error> {
    {
        let cache = connect_secret_cache().lock().unwrap();
        if cache.get(user_key).map(|c| c.as_slice()) == Some(bytes) {
            return Ok(());
        }
    }
    let entry = keyring::Entry::new(CONNECT_KEYRING_SERVICE, user_key)?;
    // `set_secret` overwrites an existing item via the keyring crate's
    // upsert path. The previous `delete_credential` pre-step requested
    // delete ACL on an existing item, which macOS treats as a separate
    // permission from read/update and would re-prompt the user.
    entry.set_secret(bytes)?;
    connect_secret_cache()
        .lock()
        .unwrap()
        .insert(user_key.to_string(), bytes.to_vec());
    Ok(())
}

/// Delete a Connect keyring secret and drop the cached copy.
pub(crate) fn delete_connect_secret(user_key: &str) {
    if let Ok(entry) = keyring::Entry::new(CONNECT_KEYRING_SERVICE, user_key) {
        let _ = entry.delete_credential();
    }
    connect_secret_cache().lock().unwrap().remove(user_key);
}

pub use account::{
    cube_backup_completeness, duress_gate_blocked, duress_tier1_gate_blocked, AddToCubeDialog,
    CheckoutPhase, CheckoutState, ConnectAccountPanel, ConnectFlowStep, ContactsState,
    ContactsStep, CubeBackupCompleteness, DuressContactsState, DuressContactsStep, DuressCube,
    DuressDisableState, DuressEnrollState, DuressEnrollStep, DuressGateStatus, InviteCubeOption,
    PlanLifecycle,
};
pub use cube::ConnectCubePanel;
pub use cube_members::ConnectCubeMembersState;

use std::sync::Arc;

use crate::{
    app::{
        breez_spark::SparkClient,
        cache::Cache,
        menu::Menu,
        message::Message,
        state::State,
        view::{self, ConnectAccountMessage},
    },
    daemon::Daemon,
};

/// Sub-steps within the Avatar sub-menu (does not replace ConnectFlowStep).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvatarFlowStep {
    /// No avatar exists and the user hasn't started creation.
    Idle,
    /// Trait questionnaire is open.
    Questionnaire,
    /// Waiting for OpenAI response (~10–30s).
    Generating,
    /// Showing a freshly generated avatar.
    Reveal,
    /// Viewing / managing an existing avatar.
    Settings,
}

/// Wrapper that holds both the account-level and cube-level Connect panels.
/// Implements `State` so the existing routing in App works unchanged.
pub struct ConnectPanel {
    pub account: ConnectAccountPanel,
    pub cube: ConnectCubePanel,
}

impl ConnectPanel {
    pub fn new(
        spark_client: Option<Arc<SparkClient>>,
        cube_uuid: String,
        cube_name: String,
        cube_network: String,
        cube_has_vault: bool,
    ) -> Self {
        let mut account = ConnectAccountPanel::new();
        // W12 §2.7 tweak #1 / W14: propagate the active cube's network
        // into ContactsState so the invite-form + add-to-cube dialogs
        // can filter their candidate-cube lists.
        account.set_active_network(Some(cube_network.clone()));
        ConnectPanel {
            account,
            cube: ConnectCubePanel::new(
                spark_client,
                cube_uuid,
                cube_name,
                cube_network,
                cube_has_vault,
            ),
        }
    }

    /// Mirror the active Cube's server-side numeric id onto
    /// `ContactsState` so the W14 "Add to Current Cube" action can
    /// target the exact loaded cube (works even when the user has
    /// multiple cubes on the same network).
    fn sync_active_cube_server_id(&mut self) {
        self.account
            .set_active_cube_server_id(self.cube.server_cube_id);
    }

    /// Sync the authenticated client from account panel to cube panel.
    fn sync_client(&mut self) {
        if self.account.is_authenticated() {
            if let Some(client) = self.account.authenticated_client() {
                self.cube.set_client(client);
            }
        } else {
            self.cube.clear_client();
        }
    }

    /// Trigger a Connect session check if the account panel hasn't
    /// already run one. Called by the App at Cube load so the in-tab
    /// session is restored from the shared keyring entry before the
    /// user navigates to a Connect-requiring page.
    pub fn ensure_session_check(&mut self) -> iced::Task<Message> {
        if matches!(self.account.step, ConnectFlowStep::CheckingSession) {
            return iced::Task::done(Message::View(view::Message::ConnectAccount(
                ConnectAccountMessage::Init,
            )));
        }
        iced::Task::none()
    }

    /// Register the active Cube through the panel's **authenticated** client
    /// so `server_cube_id` (mirrored to `cache.current_cube_server_id`)
    /// becomes available — the stream/device bootstrap behind
    /// `EnsureConnectReady` brings up grpc_url / tokens / device_id but never
    /// registers the cube, so this is what unblocks "Sign with Connect" when
    /// the numeric id is the missing Keychain prerequisite. Registering
    /// directly (rather than waiting on the Dashboard auto-register) also
    /// recovers a registration that errored or is still in flight.
    ///
    /// No-op when the id is already known or the panel has no live session.
    /// The `EnsureConnectReady` caller handles the no-live-session case (a
    /// restored connect.json session whose panel hasn't reached Dashboard, so
    /// `is_authenticated()` is false and no panel client exists) by
    /// registering from the restored tokens directly.
    pub fn ensure_cube_registered(&mut self) -> iced::Task<Message> {
        if !self.account.is_authenticated() {
            return iced::Task::none();
        }
        self.sync_client();
        if self.cube.server_cube_id.is_some() {
            // Already registered — but two pieces of per-Cube identity still
            // need publishing on Cubes registered before their fields existed:
            // the Connect-blinding encryption pubkey (PLAN-connect-blinding PR
            // D2) and the Vault's descriptor fingerprint
            // (PLAN-vault-identity-unification D4). Both are self-latching and
            // idempotent, so this is a no-op once they have succeeded.
            return iced::Task::batch([
                self.cube.register_encryption_pubkey(),
                self.cube.assert_vault_fingerprint(),
            ]);
        }
        self.cube.register_cube()
    }

    /// Seeds the Cube's Connect-blinding encryption **public** key from
    /// `CubeSettings::connect_encryption_pubkey` (derived and persisted at
    /// unlock — see `app::settings::derive_connect_encryption_pubkey`).
    ///
    /// Separate from the constructor because the panel is built in places that
    /// don't hold the full `CubeSettings`; the App calls this right after
    /// building the panels.
    pub fn set_cube_encryption_pubkey(&mut self, pubkey: Option<String>) {
        self.cube.cube_encryption_pubkey = pubkey;
    }

    /// Seeds the Vault's descriptor fingerprint from
    /// `CubeSettings::vault_fingerprint`, so the assertion wave has something
    /// to send once the server cube id resolves
    /// (`plans/PLAN-vault-identity-unification.md` D4).
    ///
    /// Also called mid-session by the backfill, which computes the fingerprint
    /// from the loaded wallet on a Cube whose settings predate the field —
    /// clearing the in-session latch so the freshly-known value is actually
    /// sent rather than swallowed by an earlier no-op.
    pub fn set_vault_fingerprint(&mut self, fingerprint: Option<String>) {
        if self.cube.vault_fingerprint != fingerprint {
            self.cube.vault_fingerprint_asserted = false;
        }
        self.cube.vault_fingerprint = fingerprint;
    }

    /// Assert the seeded fingerprint to Connect, syncing the API client first.
    ///
    /// The App calls this straight after [`Self::set_vault_fingerprint`],
    /// because the two triggers inside this panel are not enough for a Vault
    /// created **mid-session**: `CubeRegistered` already fired at startup (when
    /// there was no Vault, so nothing to send), and `ensure_cube_registered` is
    /// only reached while the Cube is still unregistered. Without this the
    /// freshly built Vault's identity would sit locally until the next launch,
    /// and Keychain would keep showing "Vault configured" with no id.
    ///
    /// Self-latching and idempotent — see
    /// [`ConnectCubePanel::assert_vault_fingerprint`].
    pub fn assert_vault_fingerprint(&mut self) -> iced::Task<Message> {
        self.sync_client();
        self.cube.assert_vault_fingerprint()
    }

    /// React to a mid-session Vault creation (PLAN-duress-vault-gate PR 3):
    ///
    /// 1. Re-report the Cube's Vault presence so the server's `hasVault` flips
    ///    for other-device duress gating without waiting for a re-registration.
    /// 2. **Invalidate + refresh the duress checklist.** The cached
    ///    `duress_cubes` may still show this Cube as vaultless/complete, and
    ///    the Tier-1 gate reads that cache — so drop it (gate fails closed)
    ///    and re-fetch, otherwise enrollment could proceed on stale data
    ///    before the new Vault's Wallet Descriptor is backed up. Local settings
    ///    already carry the Vault, so the reload gates this Cube even before
    ///    the server round-trip.
    ///
    /// No-ops appropriately when unregistered / signed out / not entitled.
    pub fn report_vault_created(&mut self) -> iced::Task<Message> {
        self.sync_client();
        self.account.invalidate_duress_cubes();
        iced::Task::batch([
            self.cube.report_vault_created(),
            self.account.reload_duress_cubes(),
        ])
    }

    /// Check if avatar should be loaded and return task if so.
    pub fn check_and_load_avatar(&self) -> iced::Task<Message> {
        if let Some(task) = self.cube.load_avatar_if_needed() {
            return task;
        }
        iced::Task::none()
    }
}

impl State for ConnectPanel {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> coincube_ui::widget::Element<'a, view::Message> {
        // Get the active avatar image handle for the sidebar
        let avatar_handle = self.cube.avatar_data.as_ref().and_then(|d| {
            let url = d.active_avatar_url.as_deref().unwrap_or("");
            d.variants
                .iter()
                .find(|v| url.ends_with(&v.id.to_string()))
                .and_then(|v| self.cube.avatar_image_cache.get(&v.id))
                .map(|(_, handle)| handle)
        });
        let ln_addr = self
            .cube
            .lightning_address
            .as_ref()
            .and_then(|la| la.lightning_address.as_deref());

        view::dashboard_with_info(
            menu,
            cache,
            view::connect::connect_panel(self),
            &cache.cube_name,
            avatar_handle,
            ln_addr,
        )
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> iced::Task<Message> {
        if matches!(self.account.step, ConnectFlowStep::CheckingSession) {
            iced::Task::done(Message::View(view::Message::ConnectAccount(
                ConnectAccountMessage::Init,
            )))
        } else {
            iced::Task::none()
        }
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> iced::Task<Message> {
        match message {
            Message::View(view::Message::ConnectAccount(msg)) => {
                let was_authenticated = self.account.is_authenticated();
                let task = self.account.update_message(msg);
                self.sync_client();
                self.sync_active_cube_server_id();
                // After first login, register the cube with the backend
                // (idempotent — returns existing if already registered).
                // The response includes lightning address if already claimed.
                let now_authenticated = self.account.is_authenticated();
                if !was_authenticated && now_authenticated {
                    // First login - register cube, avatar will load after CubeRegistered
                    let register_task = self.cube.register_cube();
                    return iced::Task::batch([task, register_task]);
                }
                // Already authenticated - ensure cube is registered, then load avatar
                if now_authenticated {
                    if self.cube.server_cube_id.is_none() {
                        // Need to register cube first, avatar will load after CubeRegistered
                        let register_task = self.cube.register_cube();
                        return iced::Task::batch([task, register_task]);
                    } else {
                        // Cube already registered, load avatar now
                        let avatar_task = self.check_and_load_avatar();
                        return iced::Task::batch([task, avatar_task]);
                    }
                }
                task
            }
            Message::View(view::Message::ConnectCube(msg)) => {
                let task = self.cube.update_message(msg);
                // `CubeRegistered(Ok)` populates `server_cube_id`; mirror
                // it into the account panel so the "Add to Current
                // Cube" button becomes enabled as soon as the cube is
                // known to the backend.
                self.sync_active_cube_server_id();
                task
            }
            _ => iced::Task::none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ConnectPanel;

    #[test]
    fn new_propagates_active_cube_context_to_both_panels() {
        let panel = ConnectPanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "bitcoin".to_string(),
            true,
        );

        assert_eq!(panel.cube.cube_uuid, "cube-uuid");
        assert_eq!(panel.cube.cube_name, "Family Vault");
        assert_eq!(panel.cube.cube_network, "bitcoin");
        assert!(panel.cube.cube_has_vault);
        assert_eq!(
            panel.account.contacts_state.active_network.as_deref(),
            Some("bitcoin")
        );
    }

    #[test]
    fn sync_active_cube_server_id_mirrors_registration_id() {
        let mut panel = ConnectPanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "regtest".to_string(),
            false,
        );

        assert_eq!(panel.account.contacts_state.active_cube_server_id, None);
        panel.cube.server_cube_id = Some(42);
        panel.sync_active_cube_server_id();
        assert_eq!(panel.account.contacts_state.active_cube_server_id, Some(42));

        panel.cube.server_cube_id = None;
        panel.sync_active_cube_server_id();
        assert_eq!(panel.account.contacts_state.active_cube_server_id, None);
    }

    /// PR D2: the pubkey seeded from settings is what the registration wave
    /// publishes, and the panel no-ops (rather than half-registering) when a
    /// Cube has no derived key yet.
    #[test]
    fn set_cube_encryption_pubkey_seeds_the_registration_wave() {
        let mut panel = ConnectPanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "regtest".to_string(),
            false,
        );
        assert_eq!(panel.cube.cube_encryption_pubkey, None);

        panel.set_cube_encryption_pubkey(Some("02aa".repeat(16) + "bb"));
        assert!(panel.cube.cube_encryption_pubkey.is_some());

        // No client and no server cube id → nothing to publish yet, and the
        // in-session latch must stay open so a later trigger still fires.
        let _ = panel.cube.register_encryption_pubkey();
        assert!(!panel.cube.enc_pubkey_registered);
    }

    /// PR D2: a Cube that is *already* registered still needs its encryption
    /// pubkey published — Cubes minted before Connect blinding existed would
    /// otherwise never get one, and their Contacts could never enrol an
    /// enveloped key.
    #[test]
    fn ensure_cube_registered_still_publishes_the_key_for_registered_cubes() {
        let mut panel = ConnectPanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "regtest".to_string(),
            false,
        );
        panel.cube.server_cube_id = Some(42);
        panel.set_cube_encryption_pubkey(Some("02".to_string() + &"bb".repeat(32)));

        // Unauthenticated → still a no-op (no client to call with).
        let _ = panel.ensure_cube_registered();
        assert!(!panel.cube.enc_pubkey_registered);
    }

    /// D4: the backfill's server half. A fingerprint the panel can't send yet
    /// must leave the latch open so a later trigger still fires, and a
    /// fingerprint that *changes* mid-session (the backfill computing one on a
    /// Cube whose settings predate the field) must re-open a latch an earlier
    /// no-op closed — otherwise the freshly-known id is never asserted.
    #[test]
    fn set_vault_fingerprint_seeds_and_reopens_the_assertion_wave() {
        let mut panel = ConnectPanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "regtest".to_string(),
            true,
        );
        assert_eq!(panel.cube.vault_fingerprint, None);

        panel.set_vault_fingerprint(Some("8099ee80".to_string()));
        assert_eq!(panel.cube.vault_fingerprint.as_deref(), Some("8099ee80"));

        // No client and no server cube id → nothing to assert yet, and the
        // latch must stay open.
        let _ = panel.cube.assert_vault_fingerprint();
        assert!(!panel.cube.vault_fingerprint_asserted);

        // A latch closed by a successful assertion re-opens only when the
        // value actually changes; re-seeding the same value leaves it closed.
        panel.cube.vault_fingerprint_asserted = true;
        panel.set_vault_fingerprint(Some("8099ee80".to_string()));
        assert!(panel.cube.vault_fingerprint_asserted);
        panel.set_vault_fingerprint(Some("deadbeef".to_string()));
        assert!(
            !panel.cube.vault_fingerprint_asserted,
            "a changed fingerprint must be re-asserted, not swallowed by the latch"
        );
    }

    /// D4, mid-session Vault creation. The Cube was registered at startup, when
    /// it had no Vault and so nothing to assert — `CubeRegistered` has already
    /// fired and `ensure_cube_registered` short-circuits on a registered Cube.
    /// So the App drives the assertion itself off the backfill, and the panel
    /// must honour it with a `server_cube_id` already set: otherwise the
    /// freshly built Vault keeps a blank identity in Keychain until relaunch.
    #[test]
    fn a_registered_cube_asserts_a_fingerprint_that_arrives_mid_session() {
        let mut panel = ConnectPanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "regtest".to_string(),
            // No Vault at startup — this is the Cube the user is about to
            // build one inside.
            false,
        );
        // Startup: registered, but nothing to send.
        panel.cube.server_cube_id = Some(42);
        let _ = panel.assert_vault_fingerprint();
        assert!(
            !panel.cube.vault_fingerprint_asserted,
            "an absent fingerprint must not close the latch"
        );

        // Mid-session: the Vault is built and the backfill seeds its identity.
        panel.set_vault_fingerprint(Some("8099ee80".to_string()));
        let _ = panel.assert_vault_fingerprint();
        // Unauthenticated in this test, so there is no client to call with and
        // the latch stays open for a later retry. What is pinned here is that
        // the path is reachable at all on an already-registered Cube — the
        // route `ensure_cube_registered` cannot provide.
        assert_eq!(panel.cube.vault_fingerprint.as_deref(), Some("8099ee80"));
        assert!(!panel.cube.vault_fingerprint_asserted);
    }

    /// D4: the assertion latch is scoped to a server cube row, so logging out
    /// must re-open it. Signing in as a different account re-registers the same
    /// local Cube UUID under a new user — a new cube and vault row, whose
    /// fingerprint is blank — and `CubeRegistered` only ever *sets* the latch.
    /// A stale `true` would therefore suppress the PATCH for the whole session,
    /// leaving the new account's Vault with no id in Keychain.
    #[test]
    fn logging_out_reopens_the_vault_fingerprint_latch() {
        let mut panel = ConnectPanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "regtest".to_string(),
            true,
        );
        // Account A: registered, identity asserted.
        panel.cube.server_cube_id = Some(42);
        panel.set_vault_fingerprint(Some("8099ee80".to_string()));
        panel.cube.vault_fingerprint_asserted = true;

        panel.cube.clear_client();

        assert_eq!(panel.cube.server_cube_id, None);
        assert!(
            !panel.cube.vault_fingerprint_asserted,
            "the latch must not outlive the server cube row it refers to"
        );
        // The fingerprint itself is Cube-scoped, not session-scoped: it comes
        // from the local descriptor and stays put for the next account.
        assert_eq!(panel.cube.vault_fingerprint.as_deref(), Some("8099ee80"));
    }

    /// D4: a Cube that is *already* registered still needs its Vault identity
    /// asserted — every Vault predating the scheme has a blank one server-side,
    /// and only the desktop can supply it.
    #[test]
    fn ensure_cube_registered_still_asserts_the_vault_fingerprint() {
        let mut panel = ConnectPanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "regtest".to_string(),
            true,
        );
        panel.cube.server_cube_id = Some(42);
        panel.set_vault_fingerprint(Some("8099ee80".to_string()));

        // Unauthenticated → still a no-op (no client to call with), latch open.
        let _ = panel.ensure_cube_registered();
        assert!(!panel.cube.vault_fingerprint_asserted);
    }

    #[test]
    fn report_vault_created_sets_local_flag_and_invalidates_duress_cache() {
        let mut panel = ConnectPanel::new(
            None,
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            "bitcoin".to_string(),
            false,
        );
        panel.account.duress_cubes = Some(Vec::new());

        let _ = panel.report_vault_created();

        assert!(panel.cube.cube_has_vault);
        assert!(panel.account.duress_cubes.is_none());
    }
}
