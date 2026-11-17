//! Trusted Cube settings and read-only projections. Never serializes wallet objects.
use super::{view, wallet::Wallet};
use crate::daemon::Daemon;
#[cfg(any(unix, windows))]
use coincube_mcp as wire;
use coincube_ui::widget::Element;
use std::sync::Arc;
#[cfg(any(unix, windows))]
use std::time::Duration;
#[cfg(all(test, any(unix, windows)))]
mod health_tests;
mod pairings;
mod presentation;
#[cfg(all(
    test,
    any(unix, windows),
    feature = "mcp-proposals",
    feature = "mcp-transactions"
))]
mod stdio_acceptance_tests;
#[cfg(feature = "mcp-transactions")]
pub(crate) mod transactions;

/// Remove only the selected Cube's pairing files, never its keystore/seed.
pub(crate) fn remove_cube_pairings(
    root: &std::path::Path,
    cube: &super::settings::CubeSettings,
) -> std::io::Result<()> {
    pairings::remove_cube(root, cube.network, &cube.id)
}

struct PairedRuntime {
    id: String,
    locator: std::path::PathBuf,
    server: wire::ipc::Server,
    #[cfg(feature = "mcp-proposals")]
    proposals: Option<Arc<wire::proposal::Store>>,
    // Last field: ownership outlives locator cleanup and server teardown.
    #[cfg(feature = "mcp-transactions")]
    transactions: Option<Arc<transactions::Service>>,
    _registry_owner: Arc<pairings::LockOwner>,
}
impl Drop for PairedRuntime {
    fn drop(&mut self) {
        self.server.grant.revoke();
        #[cfg(feature = "mcp-transactions")]
        if let Some(service) = &self.transactions {
            service.revoke();
        }
        #[cfg(feature = "mcp-proposals")]
        if let Some(store) = &self.proposals {
            store.revoke();
        }
        let _ = std::fs::remove_file(&self.locator);
    }
}

/// Actions emitted only by the trusted desktop AI access settings.
#[derive(Debug, Clone)]
pub enum Message {
    /// Open or close one paired client's controls under its row.
    ManageClient(String),
    /// Open or close the pairing form under "Pair a client".
    TogglePairing,
    /// Enable or disable MCP for the current unlocked session.
    Enable(bool),
    /// The pairing form's Turn on: `Enable(true)`, after which focus moves
    /// to the form's first field because the pressed control disappears.
    TurnOnFromForm,
    /// Select whether the current Vault may be shared.
    Wallet(bool),
    /// Edit balance, history or policy access by capability index.
    Capability(usize, bool),
    /// Choose read access for the next client before picking its parts.
    /// Grants nothing until a capability is selected and the client is paired.
    ChooseRead,
    /// Set the local display label; it is not a client authentication identity.
    ClientName(String),
    /// Mint a fresh, expiring grant after validating the current selection.
    Approve,
    /// Invalidate the current grant immediately.
    Revoke,
    /// Copy the companion command and temporary credential for a trusted client.
    CopyConfig,
    /// Pair this client persistently with exactly the displayed scope.
    Pair,
    /// Separate consent for chain-bound unsigned transaction preparation.
    #[cfg(feature = "mcp-transactions")]
    Transactions(bool),
    /// Native-only opening of the exact prepared unsigned PSBT.
    #[cfg(feature = "mcp-transactions")]
    ReviewTransaction {
        client_id: String,
        draft_id: String,
    },
    /// Completion of the bounded daemon and input checks before native review.
    #[cfg(feature = "mcp-transactions")]
    ReviewTransactionChecked {
        client_id: String,
        draft_id: String,
        session_id: uuid::Uuid,
        native: Option<transactions::NativeDraft>,
    },
    /// Revoke one stored client; ordinary locks never perform this action.
    RevokePair(String),
    /// Show the durable-pairing revocation decision before changing storage.
    RequestRevokePair(String),
    CancelRevokePair,
    /// Copy a stable private configuration for a stored client.
    CopyPair(String),
    /// Explicit separate consent for an empty Cube's proposal session.
    #[cfg(feature = "mcp-proposals")]
    Proposals(bool),
    /// Explicit public-key assembly consent, separate from topology-only access.
    #[cfg(feature = "mcp-proposals")]
    Creation(bool),
    /// Native-only transition into the existing key selection builder.
    #[cfg(feature = "mcp-proposals")]
    ReviewProposal,
    #[cfg(feature = "mcp-proposals")]
    ReviewPairedProposal(String),
}
/// A trusted AI access action whose control disappears once it is handled.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FocusAfter {
    Pair,
    TogglePairing,
    Enable,
}

/// Native consent and active endpoints. The default grants no access.
/// Durable authorization is encrypted; runtime access requires an unlocked Cube.
#[derive(Default)]
pub struct Settings {
    enabled: bool,
    wallet_allowed: bool,
    capabilities: [bool; 3],
    client_name: String,
    approved_name: String,
    failed: bool,
    #[cfg(feature = "mcp-proposals")]
    proposals_allowed: bool,
    #[cfg(feature = "mcp-proposals")]
    creation_allowed: bool,
    #[cfg(feature = "mcp-proposals")]
    proposals: Option<Arc<wire::proposal::Store>>,
    #[cfg(any(unix, windows))]
    server: Option<wire::ipc::Server>,
    #[cfg(feature = "mcp-transactions")]
    transactions_allowed: bool,
    pairing_location: Option<pairings::Location>,
    pairing_store: Option<pairings::Store>,
    paired: Vec<PairedRuntime>,
    pairing_error: Option<&'static str>,
    pairing_poisoned: bool,
    pairing_generation: Option<u64>,
    revoke_confirmation: Option<String>,
    last_poll_at_startup: Option<u32>,
    /// The paired client whose controls are open on AI access.
    managing: Option<String>,
    /// Whether the pairing form is open on AI access.
    pairing_open: bool,
    /// The pairing form's read choice, before any read capability is on.
    read_chosen: bool,
    /// The client whose private configuration was copied last, for feedback.
    copied: Option<String>,
}
pub(crate) struct PairingContext<'a> {
    pub root: &'a std::path::Path,
    pub chain: crate::chain::ChainId,
    pub cube_id: &'a str,
    pub wallet: Option<Arc<Wallet>>,
    pub daemon: Option<Arc<dyn Daemon + Send + Sync>>,
    pub supported: bool,
    pub proposal_supported: bool,
    /// Poll timestamp captured when this Cube's session started. A summary is
    /// not synced until a later poll completes.
    pub last_poll_at_startup: Option<u32>,
}
impl Settings {
    /// Where keyboard focus goes after an action removed its own control.
    pub(crate) fn focus_after(&self, after: FocusAfter) -> Option<iced::advanced::widget::Id> {
        use iced::advanced::widget::Id;
        match after {
            // A successful pairing closes the form and opens the new client.
            FocusAfter::Pair => self
                .managing
                .as_ref()
                .filter(|_| !self.pairing_open)
                .map(|id| Id::from(format!("tenshu.ai-access.client.{id}.manage"))),
            FocusAfter::TogglePairing if self.pairing_open => Some(Id::from(if self.enabled {
                presentation::NAME_ID
            } else {
                presentation::FORM_ENABLE_ID
            })),
            // Pair a client is disabled after a storage failure.
            FocusAfter::TogglePairing if self.pairing_poisoned => None,
            FocusAfter::TogglePairing => Some(Id::from(presentation::PAIR_OPEN_ID)),
            FocusAfter::Enable if self.pairing_open && self.enabled => {
                Some(Id::from(presentation::NAME_ID))
            }
            FocusAfter::Enable => None,
        }
    }

    /// Whether the Cube-wide switch is enabled (and so can take focus) on the
    /// read-access page. Turning it off is always allowed. Turning it on is
    /// allowed where a Vault on a documented chain can pair, and also where
    /// the runtime serves this Vault and clients are already paired: that
    /// only resumes their stored access and grants nothing new.
    pub(crate) fn switch_focusable(&self, supported: bool) -> bool {
        self.enabled
            || (supported
                && (self
                    .pairing_location
                    .as_ref()
                    .and_then(pairings::Location::chain)
                    .is_none_or(presentation::chain_supported)
                    || self
                        .pairing_store
                        .as_ref()
                        .is_some_and(|store| !store.clients().is_empty())))
    }

    pub(crate) fn pending_revoke(&self) -> Option<(&str, &str)> {
        let id = self.revoke_confirmation.as_deref()?;
        let client = self
            .pairing_store
            .as_ref()?
            .clients()
            .iter()
            .find(|client| client.id == id)?;
        Some((&client.name, id))
    }

    pub(crate) fn revoke_dialog(&self) -> Option<Element<'_, view::Message>> {
        let (name, id) = self.pending_revoke()?;
        Some(presentation::revoke_confirmation(name, id))
    }

    /// Read access is its own pairing: choosing it clears transaction and
    /// wallet-preparation choices left from an earlier form.
    fn clear_other_scopes(&mut self) {
        #[cfg(feature = "mcp-transactions")]
        {
            self.transactions_allowed = false;
        }
        #[cfg(feature = "mcp-proposals")]
        {
            self.proposals_allowed = false;
            self.creation_allowed = false;
        }
    }

    fn clear_pairing_selection(&mut self) {
        self.wallet_allowed = false;
        self.capabilities = [false; 3];
        self.read_chosen = false;
        #[cfg(feature = "mcp-proposals")]
        {
            self.proposals_allowed = false;
            self.creation_allowed = false;
        }
        #[cfg(feature = "mcp-transactions")]
        {
            self.transactions_allowed = false;
        }
    }

    /// Stop the server and clear its approved display label synchronously.
    pub fn revoke(&mut self) {
        // Runtime suspension only. Explicit durable revocation is RevokePair.
        self.revoke_confirmation = None;
        self.copied = None;
        self.paired.clear();
        self.revoke_legacy();
    }
    fn revoke_legacy(&mut self) {
        #[cfg(any(unix, windows))]
        {
            self.server.take();
        }
        #[cfg(feature = "mcp-proposals")]
        if let Some(store) = self.proposals.take() {
            store.revoke();
        }
        self.approved_name.clear();
    }
    fn persistence_failed(&mut self) {
        self.revoke();
        self.pairing_poisoned = true;
        // A failed registry write must not resurrect a removed authorization
        // from disk on restart. Invalidating only this registry's wrapping key
        // makes its ciphertext inert; no Cube unlock material is affected.
        self.pairing_error = Some(
            if self
                .pairing_location
                .as_ref()
                .is_some_and(|v| v.invalidate_key().is_ok())
            {
                "Pairing storage failed. All paired clients are suspended and their stored authorization is invalid. Pairing recovery is required."
            } else {
                "Could not persist revocation or invalidate its storage key. Clients are suspended now; durable revocation is NOT confirmed. Retry before closing COINCUBE Desktop."
            },
        );
    }
    /// Load persistent authorization only after ordinary unlock. Every restored
    /// runtime is bound to the current generation and exact approved wallet.
    pub(crate) fn ensure_pairings(&mut self, context: PairingContext<'_>) {
        let PairingContext {
            root,
            chain,
            cube_id,
            wallet,
            daemon,
            supported,
            proposal_supported,
            last_poll_at_startup,
        } = context;
        self.last_poll_at_startup = last_poll_at_startup;
        if super::session::generation_for(cube_id).is_none() || self.pairing_poisoned {
            return;
        }
        let Some(generation) = super::session::generation_for(cube_id) else {
            return;
        };
        if self.pairing_generation != Some(generation) {
            self.paired.clear();
            self.pairing_generation = Some(generation);
        }
        if self
            .pairing_location
            .as_ref()
            .is_some_and(|v| !v.matches(root, chain, cube_id))
        {
            self.revoke();
            self.pairing_poisoned = true;
            self.pairing_error = Some("Pairing context changed. No client access was restored.");
            return;
        }
        if self.pairing_location.is_none() {
            let Ok(location) = pairings::Location::new(root, chain, cube_id) else {
                return;
            };
            if location.exists() {
                match location.open(false) {
                    Ok(store) => {
                        self.enabled = store.enabled();
                        self.pairing_store = Some(store);
                    }
                    Err(_) => {
                        self.pairing_error = Some("Stored client pairing is unavailable or invalid. No client access was restored.");
                        self.pairing_poisoned = true;
                    }
                }
            }
            self.pairing_location = Some(location);
        }
        if self.pairing_poisoned || !self.enabled {
            return;
        }
        let Some(store) = &self.pairing_store else {
            return;
        };
        if !store.enabled() {
            return;
        }
        self.paired
            .retain(|runtime| store.clients().iter().any(|client| client.id == runtime.id));
        for client in store.clients() {
            // Keep an existing (including consumed) proposal session intact.
            if self.paired.iter().any(|runtime| runtime.id == client.id) {
                continue;
            }
            let cube = cube_id.to_owned();
            let guard = move || super::session::generation_for(&cube) == Some(generation);
            #[cfg(feature = "mcp-proposals")]
            let proposals = matches!(
                client.scope,
                pairings::Scope::Proposal | pairings::Scope::WalletCreation
            )
            .then(|| chain.dir_name().parse().ok())
            .flatten()
            .map(|network| Arc::new(wire::proposal::Store::for_chain(network)));
            #[cfg(feature = "mcp-transactions")]
            let mut transactions = None;
            let built: Option<(Arc<dyn wire::Reader>, wire::ipc::Grant)> = match &client.scope {
                pairings::Scope::Read {
                    wallet: expected_wallet,
                    descriptor,
                    opaque,
                    capabilities,
                } if supported => match (&wallet, &daemon) {
                    (Some(wallet), Some(daemon))
                        if wallet.id().to_string() == *expected_wallet
                            && pairings::descriptor_identity(wallet) == *descriptor
                            && wallet.chain == chain
                            && wallet.main_descriptor.as_vault().is_some()
                            && daemon.backend().is_embedded() =>
                    {
                        wire::ipc::Grant::paired(
                            opaque.clone(),
                            *capabilities,
                            client.credential.clone(),
                            guard,
                        )
                        .ok()
                        .map(|grant| {
                            let reader: Arc<dyn wire::Reader> = Arc::new(VaultReader {
                                read_slots: Arc::new(tokio::sync::Semaphore::new(2)),
                                descriptor: wallet
                                    .main_descriptor
                                    .as_vault()
                                    .expect("checked Vault")
                                    .clone(),
                                chain,
                                daemon: daemon.clone(),
                                wallet_id: opaque.clone(),
                                activity_key: new_activity_key(),
                                last_poll_at_startup,
                            });
                            (reader, grant)
                        })
                    }
                    _ => None,
                },
                #[cfg(feature = "mcp-transactions")]
                pairings::Scope::TransactionPreparation {
                    wallet: expected_wallet,
                    descriptor,
                    opaque,
                } if supported && transactions::supported_chain(chain) => {
                    match (&wallet, &daemon) {
                        (Some(wallet), Some(daemon))
                            if wallet.id().to_string() == *expected_wallet
                                && pairings::descriptor_identity(wallet) == *descriptor
                                && wallet.chain == chain
                                && wallet.main_descriptor.as_vault().is_some()
                                && daemon.backend().is_embedded() =>
                        {
                            wire::ipc::Grant::paired_transactions(
                                opaque.clone(),
                                client.credential.clone(),
                                guard,
                            )
                            .ok()
                            .map(|grant| {
                                let service = Arc::new(transactions::Service::new(
                                    chain,
                                    wallet
                                        .main_descriptor
                                        .as_vault()
                                        .expect("checked Vault")
                                        .clone(),
                                    daemon.clone(),
                                    opaque.clone(),
                                    last_poll_at_startup,
                                ));
                                let reader: Arc<dyn wire::Reader> = service.clone();
                                transactions = Some(service);
                                (reader, grant)
                            })
                        }
                        _ => None,
                    }
                }
                pairings::Scope::Proposal
                    if proposal_supported && chain.dir_name().parse::<wire::Chain>().is_ok() =>
                {
                    #[cfg(feature = "mcp-proposals")]
                    {
                        wire::ipc::Grant::paired_proposals(client.credential.clone(), guard)
                            .ok()
                            .map(|grant| {
                                let reader: Arc<dyn wire::Reader> =
                                    proposals.as_ref().expect("proposal store").clone();
                                (reader, grant)
                            })
                    }
                    #[cfg(not(feature = "mcp-proposals"))]
                    {
                        None
                    }
                }
                pairings::Scope::WalletCreation
                    if proposal_supported && chain.dir_name().parse::<wire::Chain>().is_ok() =>
                {
                    #[cfg(feature = "mcp-proposals")]
                    {
                        wire::ipc::Grant::paired_creation(client.credential.clone(), guard)
                            .ok()
                            .map(|grant| {
                                let reader: Arc<dyn wire::Reader> =
                                    Arc::new(crate::installer::creation::CreationReader::new(
                                        proposals.as_ref().expect("creation store").clone(),
                                    ));
                                (reader, grant)
                            })
                    }
                    #[cfg(not(feature = "mcp-proposals"))]
                    {
                        None
                    }
                }
                _ => None,
            };
            let Some((reader, grant)) = built else {
                continue;
            };
            match wire::ipc::Server::start(reader, grant) {
                Ok(server) => {
                    let locator = store.location.locator(&client.id);
                    if !server.grant.active()
                        || wire::pairing::publish(&locator, &server.socket).is_err()
                    {
                        self.pairing_error = Some("Could not publish a paired client connection. No access was advertised.");
                        continue;
                    }
                    self.paired.push(PairedRuntime {
                        id: client.id.clone(),
                        locator,
                        server,
                        #[cfg(feature = "mcp-proposals")]
                        proposals,
                        #[cfg(feature = "mcp-transactions")]
                        transactions,
                        _registry_owner: store.owner(),
                    });
                }
                Err(_) => self.pairing_error = Some("Could not start a paired client connection."),
            }
        }
    }
    fn pair_client(
        &mut self,
        wallet: Option<Arc<Wallet>>,
        daemon: Option<Arc<dyn Daemon + Send + Sync>>,
        supported: bool,
        cube_id: &str,
    ) {
        let Some(generation) = super::session::generation_for(cube_id) else {
            return;
        };
        if self.pairing_poisoned
            || !supported
            || !self.enabled
            || self.client_name.trim().is_empty()
        {
            return;
        }
        let scope = match (&wallet, &daemon) {
            #[cfg(feature = "mcp-transactions")]
            (Some(wallet), Some(daemon))
                if self.transactions_allowed
                    && self.wallet_allowed
                    && !self.capabilities.iter().any(|v| *v)
                    && transactions::supported_chain(wallet.chain)
                    && wallet.main_descriptor.as_vault().is_some()
                    && daemon.backend().is_embedded()
                    && self
                        .pairing_location
                        .as_ref()
                        .is_some_and(|v| v.chain_matches(wallet.chain)) =>
            {
                pairings::Scope::TransactionPreparation {
                    wallet: wallet.id().to_string(),
                    descriptor: pairings::descriptor_identity(wallet),
                    opaque: uuid::Uuid::new_v4().to_string(),
                }
            }
            (Some(wallet), Some(daemon))
                if self.wallet_allowed
                    && self.capabilities.iter().any(|v| *v)
                    && wallet.main_descriptor.as_vault().is_some()
                    && daemon.backend().is_embedded()
                    && self
                        .pairing_location
                        .as_ref()
                        .is_some_and(|v| v.chain_matches(wallet.chain)) =>
            {
                pairings::Scope::Read {
                    wallet: wallet.id().to_string(),
                    descriptor: pairings::descriptor_identity(wallet),
                    opaque: uuid::Uuid::new_v4().to_string(),
                    capabilities: self.capabilities,
                }
            }
            #[cfg(feature = "mcp-proposals")]
            (None, None)
                if (self.proposals_allowed || self.creation_allowed)
                    && self
                        .pairing_location
                        .as_ref()
                        .is_some_and(|v| v.transaction_supported()) =>
            {
                if self.creation_allowed {
                    pairings::Scope::WalletCreation
                } else {
                    pairings::Scope::Proposal
                }
            }
            _ => return,
        };
        if self.pairing_store.is_none() {
            let Some(location) = &self.pairing_location else {
                return;
            };
            match location.open(true) {
                Ok(store) => self.pairing_store = Some(store),
                Err(_) => {
                    self.pairing_error =
                        Some("Could not protect the client pairing. No client was paired.");
                    return;
                }
            }
        }
        let client_name = self.client_name.trim().to_owned();
        let store = self
            .pairing_store
            .as_mut()
            .expect("loaded pairing registry");
        if store.clients().len() >= pairings::MAX_CLIENTS {
            self.pairing_error = Some(
                "The 16-client limit was reached. Revoke an unused client before pairing another.",
            );
            return;
        }
        let result = super::session::admit_generation(cube_id, generation, || {
            Some(store.add(client_name, scope))
        });
        self.finish_pairing(result);
    }
    fn finish_pairing(&mut self, result: Option<Result<String, pairings::AddError>>) {
        match result {
            None => {
                self.pairing_error =
                    Some("Cube locked or changed before pairing was saved. No client was paired.")
            }
            Some(Err(pairings::AddError::Unchanged)) => {
                self.pairing_error = Some(
                    "Could not save the new client. Existing pairings are unchanged; retry pairing.",
                );
            }
            Some(Err(pairings::AddError::Uncertain)) => self.persistence_failed(),
            Some(Ok(id)) => {
                self.pairing_error = None;
                self.client_name.clear();
                // Close the form and open the new client's row, where its
                // private configuration can be copied.
                self.pairing_open = false;
                self.managing = Some(id);
                self.read_chosen = false;
                // Every new client must receive its own explicit wallet and
                // capability choices; the previous grant is not a template.
                self.wallet_allowed = false;
                self.capabilities = [false; 3];
                #[cfg(feature = "mcp-transactions")]
                {
                    self.transactions_allowed = false;
                }
                #[cfg(feature = "mcp-proposals")]
                {
                    self.proposals_allowed = false;
                    self.creation_allowed = false;
                }
            }
        }
    }
    /// Apply a trusted UI action, returning private client JSON only for CopyConfig.
    /// Selection edits revoke old credentials; approval requires an unlocked Vault
    /// on the embedded backend and captures its current session generation.
    pub fn update(
        &mut self,
        message: Message,
        wallet: Option<Arc<Wallet>>,
        daemon: Option<Arc<dyn Daemon + Send + Sync>>,
        supported: bool,
        cube_id: &str,
    ) -> Option<String> {
        match message {
            Message::ManageClient(id) => {
                self.copied = None;
                self.managing = if self.managing.as_deref() == Some(id.as_str()) {
                    None
                } else {
                    Some(id)
                };
                return None;
            }
            Message::TogglePairing => {
                self.pairing_open = !self.pairing_open;
                return None;
            }
            #[cfg(feature = "mcp-transactions")]
            Message::ReviewTransaction { .. } | Message::ReviewTransactionChecked { .. } => {
                return None;
            }
            Message::TurnOnFromForm => {
                return self.update(Message::Enable(true), wallet, daemon, supported, cube_id);
            }
            Message::Enable(value) => {
                self.copied = None;
                self.revoke();
                self.clear_pairing_selection();
                self.client_name.clear();
                self.enabled = value && cfg!(any(unix, windows));
                if let Some(store) = &mut self.pairing_store {
                    if store.set_enabled(self.enabled).is_err() {
                        self.persistence_failed();
                    }
                }
            }
            Message::Wallet(value) => {
                self.revoke_legacy();
                self.wallet_allowed = value;
            }
            Message::ChooseRead => {
                self.revoke_legacy();
                self.read_chosen = true;
                self.clear_other_scopes();
            }
            Message::Capability(index, value) if index < 3 => {
                self.revoke_legacy();
                self.capabilities[index] = value;
                if value {
                    self.read_chosen = true;
                    self.clear_other_scopes();
                }
                #[cfg(feature = "mcp-transactions")]
                if value {
                    self.transactions_allowed = false;
                }
            }
            Message::ClientName(value) => {
                self.revoke_legacy();
                self.client_name = value.chars().filter(|c| !c.is_control()).take(64).collect();
            }
            #[cfg(feature = "mcp-proposals")]
            Message::Proposals(value) => {
                self.revoke_legacy();
                self.proposals_allowed = value;
                if value {
                    self.creation_allowed = false;
                }
            }
            #[cfg(feature = "mcp-proposals")]
            Message::Creation(value) => {
                self.revoke_legacy();
                self.creation_allowed = value;
                if value {
                    self.proposals_allowed = false;
                }
            }
            #[cfg(feature = "mcp-transactions")]
            Message::Transactions(value) => {
                self.revoke_legacy();
                self.transactions_allowed = value;
                if value {
                    self.capabilities = [false; 3];
                    self.read_chosen = false;
                }
            }
            Message::Revoke => self.revoke(),
            Message::Pair => {
                self.pair_client(wallet, daemon, supported, cube_id);
                self.clear_pairing_selection();
                return None;
            }
            Message::RevokePair(id) => {
                if self.revoke_confirmation.as_deref() != Some(id.as_str()) {
                    return None;
                }
                self.revoke_confirmation = None;
                self.copied = None;
                self.paired.retain(|runtime| runtime.id != id);
                if let Some(store) = &mut self.pairing_store {
                    if store.remove(&id).is_err() {
                        self.persistence_failed();
                    }
                }
                return None;
            }
            Message::RequestRevokePair(id) => {
                // Keep the client's controls open so focus can return to its
                // Revoke action when the dialog is dismissed.
                self.managing = Some(id.clone());
                self.revoke_confirmation = self
                    .pairing_store
                    .as_ref()
                    .and_then(|store| store.clients().iter().find(|client| client.id == id))
                    .map(|_| id);
                return None;
            }
            Message::CancelRevokePair => {
                // Return to the client's open controls, where focus lands
                // back on its Revoke action.
                if let Some(id) = self.revoke_confirmation.take() {
                    self.managing = Some(id);
                }
                return None;
            }
            Message::CopyPair(id) => {
                let generation = super::session::generation_for(cube_id)?;
                if self.pairing_poisoned
                    || !self.enabled
                    || self.pairing_generation != Some(generation)
                    || !self
                        .pairing_location
                        .as_ref()
                        .is_some_and(|location| location.cube_matches(cube_id))
                    || !self
                        .paired
                        .iter()
                        .any(|runtime| runtime.id == id && runtime.server.grant.active())
                {
                    return None;
                }
                let store = self.pairing_store.as_ref()?;
                let client = store.clients().iter().find(|client| client.id == id)?;
                let executable = std::env::current_exe()
                    .ok()?
                    .parent()?
                    .join(format!("coincube-mcp{}", std::env::consts::EXE_SUFFIX));
                let config = serde_json::to_string_pretty(&serde_json::json!({"mcpServers": {"tenshu": {
                    "command": executable,
                    "env": {"COINCUBE_MCP_PAIRING": store.location.locator(&id), "COINCUBE_MCP_CREDENTIAL": client.credential}
                }}})).ok();
                if config.is_some() {
                    self.copied = Some(id);
                }
                return config;
            }
            Message::Approve => {
                self.revoke_legacy();
                self.failed = false;
                #[cfg(feature = "mcp-proposals")]
                if supported
                    && wallet.is_none()
                    && daemon.is_none()
                    && self.enabled
                    && self.proposals_allowed
                    && !self.client_name.trim().is_empty()
                {
                    let generation = super::session::generation_for(cube_id)?;
                    let cube_id = cube_id.to_owned();
                    let chain = self.pairing_location.as_ref()?.chain()?;
                    let store = Arc::new(wire::proposal::Store::for_chain(
                        chain.dir_name().parse().ok()?,
                    ));
                    let grant =
                        wire::ipc::Grant::for_proposals(Duration::from_secs(3600), move || {
                            super::session::generation_for(&cube_id) == Some(generation)
                        });
                    match wire::ipc::Server::start(store.clone(), grant) {
                        Ok(server) => {
                            self.server = Some(server);
                            self.proposals = Some(store);
                            self.approved_name = self.client_name.clone();
                        }
                        Err(_) => self.failed = true,
                    }
                    return None;
                }
                if !supported
                    || !self.enabled
                    || !self.wallet_allowed
                    || self.client_name.trim().is_empty()
                    || !self.capabilities.iter().any(|v| *v)
                {
                    return None;
                }
                #[cfg(any(unix, windows))]
                if let (Some(wallet), Some(daemon)) = (wallet, daemon) {
                    if wallet.main_descriptor.as_vault().is_none()
                        || !daemon.backend().is_embedded()
                    {
                        return None;
                    }
                    let generation = super::session::generation_for(cube_id)?;
                    let cube_id = cube_id.to_owned();
                    let wallet_id = uuid::Uuid::new_v4().to_string();
                    let grant = wire::ipc::Grant::new(
                        wallet_id.clone(),
                        self.capabilities,
                        Duration::from_secs(3600),
                    )
                    .with_session_guard(move || {
                        super::session::generation_for(&cube_id) == Some(generation)
                    });
                    let reader = Arc::new(VaultReader {
                        read_slots: Arc::new(tokio::sync::Semaphore::new(2)),
                        descriptor: wallet.main_descriptor.as_vault()?.clone(),
                        chain: wallet.chain,
                        daemon,
                        wallet_id,
                        activity_key: new_activity_key(),
                        last_poll_at_startup: self.last_poll_at_startup,
                    });
                    match wire::ipc::Server::start(reader, grant) {
                        Ok(server) => {
                            self.server = Some(server);
                            self.approved_name = self.client_name.clone();
                        }
                        Err(_) => self.failed = true,
                    }
                }
                #[cfg(not(any(unix, windows)))]
                let _ = (wallet, daemon, cube_id);
            }
            Message::CopyConfig => {
                #[cfg(any(unix, windows))]
                if let Some(server) = self.server.as_ref().filter(|s| s.grant.active()) {
                    let executable = std::env::current_exe()
                        .ok()?
                        .parent()?
                        .join(format!("coincube-mcp{}", std::env::consts::EXE_SUFFIX));
                    return serde_json::to_string_pretty(&serde_json::json!({"mcpServers": {"tenshu": {
                        "command": executable,
                        "env": {"COINCUBE_MCP_SOCKET": server.socket, "COINCUBE_MCP_CREDENTIAL": server.grant.credential()}
                    }}})).ok();
                }
            }
            _ => {}
        }
        None
    }
    /// Native handoff consumes Settings ownership, preserving only this session.
    #[cfg(feature = "mcp-proposals")]
    pub(crate) fn proposal_session(
        &self,
        cube_id: &str,
    ) -> Option<crate::installer::proposal::Session> {
        let server = self.server.as_ref().filter(|s| s.grant.active())?;
        let store = self.proposals.as_ref()?;
        let proposal = store.pending().ok()??;
        Some(crate::installer::proposal::Session::new(
            cube_id.to_owned(),
            super::session::generation_for(cube_id)?,
            self.pairing_location.as_ref()?.chain()?,
            server.grant.clone(),
            store.clone(),
            proposal,
        ))
    }

    #[cfg(feature = "mcp-proposals")]
    pub(crate) fn paired_proposal_session(
        &self,
        cube_id: &str,
        id: &str,
    ) -> Option<crate::installer::proposal::Session> {
        let runtime = self
            .paired
            .iter()
            .find(|v| v.id == id && v.server.grant.active())?;
        let store = runtime.proposals.as_ref()?;
        let proposal = store.pending().ok()??;
        Some(crate::installer::proposal::Session::new(
            cube_id.to_owned(),
            super::session::generation_for(cube_id)?,
            self.pairing_location.as_ref()?.chain()?,
            runtime.server.grant.clone(),
            store.clone(),
            proposal,
        ))
    }

    #[cfg(feature = "mcp-transactions")]
    pub(crate) fn transaction_service(
        &self,
        client_id: &str,
        wallet: &Wallet,
    ) -> Option<Arc<transactions::Service>> {
        if !transactions::supported_chain(wallet.chain)
            || !self.pairing_location.as_ref()?.chain_matches(wallet.chain)
        {
            return None;
        }
        let client = self
            .pairing_store
            .as_ref()?
            .clients()
            .iter()
            .find(|c| c.id == client_id)?;
        match &client.scope {
            pairings::Scope::TransactionPreparation {
                wallet: expected,
                descriptor,
                ..
            } if *expected == wallet.id().to_string()
                && *descriptor == pairings::descriptor_identity(wallet) => {}
            _ => return None,
        }
        let runtime = self
            .paired
            .iter()
            .find(|v| v.id == client_id && v.server.grant.active())?;
        if !runtime.server.grant.active() {
            return None;
        }
        runtime
            .transactions
            .clone()
            .filter(|service| service.chain == wallet.chain)
    }

    #[cfg(all(test, feature = "mcp-transactions"))]
    pub(crate) fn prepared_transaction(
        &self,
        client_id: &str,
        draft_id: &str,
        wallet: &Wallet,
    ) -> Option<transactions::NativeDraft> {
        self.transaction_service(client_id, wallet)?
            .native(draft_id)
    }

    fn transaction_selection(&self) -> bool {
        #[cfg(feature = "mcp-transactions")]
        {
            self.transactions_allowed
                && self
                    .pairing_location
                    .as_ref()
                    .is_some_and(|v| v.transaction_supported())
        }
        #[cfg(not(feature = "mcp-transactions"))]
        false
    }
    /// Render consent controls and warn that the configured AI provider may receive data.
    pub fn view_for_cube(
        &self,
        supported: bool,
        proposal_supported: bool,
    ) -> Element<'_, view::Message> {
        #[cfg(feature = "mcp-proposals")]
        if proposal_supported {
            return self.proposal_view();
        }
        #[cfg(not(feature = "mcp-proposals"))]
        let _ = proposal_supported;
        presentation::view_for_cube(self, supported)
    }
}

#[cfg(feature = "mcp-proposals")]
impl Settings {
    fn proposal_view(&self) -> Element<'_, view::Message> {
        presentation::setup_view(self)
    }
}

#[cfg(any(unix, windows))]
/// Read projection retaining a descriptor and daemon, never a wallet signer.
#[derive(Clone)]
struct VaultReader {
    read_slots: Arc<tokio::sync::Semaphore>,
    descriptor: coincube_core::descriptors::CoincubeDescriptor,
    chain: crate::chain::ChainId,
    daemon: Arc<dyn Daemon + Send + Sync>,
    wallet_id: String,
    /// Per-grant key for opaque activity ids. It never leaves the desktop.
    activity_key: [u8; 32],
    last_poll_at_startup: Option<u32>,
}

#[cfg(any(unix, windows))]
fn new_activity_key() -> [u8; 32] {
    use rand::RngCore;
    let mut key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    key
}

#[cfg(any(unix, windows))]
fn activity_id(key: &[u8; 32], txid: &coincube_core::miniscript::bitcoin::Txid) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.update(txid.to_string().as_bytes());
    hex::encode(&hasher.finalize()[..16])
}
#[cfg(any(unix, windows))]
impl wire::Reader for VaultReader {
    /// Bound blocking database work to two workers independently of IPC deadlines.
    fn read(
        &self,
        request: wire::Request,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<wire::Response, wire::Error>> + Send + '_>,
    > {
        Box::pin(async move {
            // Existing daemon reads may block on SQLite. Keep them off the async
            // executor so deadlines and revocation remain responsive. The permit
            // belongs to the blocking worker: timing out must not admit unlimited
            // replacement reads while old database calls are still running.
            let permit = self
                .read_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| wire::Error::Unavailable)?;
            let reader = self.clone();
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                tokio::runtime::Handle::current().block_on(reader.read_wallet(request))
            })
            .await
            .map_err(|_| wire::Error::Unavailable)?
        })
    }
}
#[cfg(any(unix, windows))]
impl VaultReader {
    /// Coarse health only: never export backend messages, heights or scan times.
    fn sync_state(&self, info: &crate::daemon::model::GetInfoResult) -> &'static str {
        if info.refused_reorg_depth.is_some()
            || info.chain_divergence
            || !info.sync.is_finite()
            || !(0.0..=1.0).contains(&info.sync)
        {
            return "sync_failing";
        }
        match super::wallet::sync_status(
            self.daemon.backend(),
            info.block_height,
            info.sync,
            info.last_poll_timestamp,
            self.last_poll_at_startup,
            &info.history_sync,
        ) {
            super::wallet::SyncStatus::SyncFailing { .. } => "sync_failing",
            super::wallet::SyncStatus::Synced if info.rescan_progress.is_none() => "synced",
            _ => "syncing",
        }
    }

    async fn wallet_info(&self) -> Result<crate::daemon::model::GetInfoResult, wire::Error> {
        let info = self
            .daemon
            .get_info()
            .await
            .map_err(|_| wire::Error::Unavailable)?;
        if info.descriptors.main.as_vault() != Some(&self.descriptor)
            || info.chain_id != Some(self.chain)
        {
            return Err(wire::Error::Unavailable);
        }
        Ok(info)
    }

    /// Validate the opaque wallet scope and daemon identity before projecting public data.
    async fn read_wallet(&self, request: wire::Request) -> Result<wire::Response, wire::Error> {
        if matches!(
            &request,
            wire::Request::PrepareProposal(_)
                | wire::Request::PrepareCreation(_)
                | wire::Request::Proposal(_)
                | wire::Request::PrepareTransaction(_)
                | wire::Request::Transaction(_)
        ) {
            return Err(wire::Error::AccessDenied);
        }
        // Defense in depth: the IPC gate enforces the same opaque identity.
        if request.scope().is_some_and(|(id, _)| id != self.wallet_id) {
            return Err(wire::Error::AccessDenied);
        }
        if !request.valid() {
            return Err(wire::Error::InvalidInput);
        }
        let descriptor = &self.descriptor;
        let info = self.wallet_info().await?;
        match request {
            wire::Request::PrepareProposal(_)
            | wire::Request::PrepareCreation(_)
            | wire::Request::Proposal(_)
            | wire::Request::PrepareTransaction(_)
            | wire::Request::Transaction(_) => Err(wire::Error::AccessDenied),
            wire::Request::List(_) => Ok(wire::Response::List(wire::WalletList {
                wallets: vec![wire::WalletMetadata {
                    network: self.chain.to_string(),
                    wallet_id: self.wallet_id.clone(),
                    display_name: "Vault".into(),
                    wallet_type: "vault".into(),
                }],
            })),
            wire::Request::Summary(_) => {
                let coins = self
                    .daemon
                    .list_coins(&[], &[])
                    .await
                    .map_err(|_| wire::Error::Unavailable)?
                    .coins;
                // Either endpoint of the read may be unhealthy; a scan finishing
                // during the read must not make older coins look complete.
                let after = self.wallet_info().await?;
                let states = [self.sync_state(&info), self.sync_state(&after)];
                let sync_state = if states.contains(&"sync_failing") {
                    "sync_failing"
                } else if states.contains(&"syncing") {
                    "syncing"
                } else {
                    "synced"
                }
                .to_owned();
                let unspent = || coins.iter().filter(|c| c.spend_info.is_none());
                Ok(wire::Response::Summary(wire::Summary {
                    wallet_id: self.wallet_id.clone(),
                    network: self.chain.to_string(),
                    wallet_type: "vault".into(),
                    confirmed_sats: unspent()
                        .filter(|c| c.block_height.is_some())
                        .map(|c| c.amount.to_sat())
                        .sum(),
                    unconfirmed_sats: unspent()
                        .filter(|c| c.block_height.is_none())
                        .map(|c| c.amount.to_sat())
                        .sum(),
                    available_sats: unspent()
                        .filter(|c| !c.is_immature && crate::daemon::model::coin_is_owned(c))
                        .map(|c| c.amount.to_sat())
                        .sum(),
                    sync_state,
                }))
            }
            wire::Request::History(input) => {
                // History has no completeness field. Never return an empty or
                // partial list as authoritative while synchronization is unsafe.
                if self.sync_state(&info) != "synced" {
                    return Err(wire::Error::Unavailable);
                }
                let mut transactions = self
                    .daemon
                    .list_history_txs(0, u32::MAX, u64::from(input.limit))
                    .await
                    .map_err(|_| wire::Error::Unavailable)?;
                if self.sync_state(&self.wallet_info().await?) != "synced" {
                    return Err(wire::Error::Unavailable);
                }
                // The confirmed-history query applies DISTINCT after ORDER BY,
                // which does not preserve newest-first. Sort before projecting
                // so exact time, height and chain ID never cross the MCP boundary.
                transactions.sort_by(|left, right| {
                    right
                        .time
                        .cmp(&left.time)
                        .then(right.height.cmp(&left.height))
                        .then(left.txid.cmp(&right.txid))
                });
                let transactions: Vec<_> = transactions
                    .into_iter()
                    .take(usize::from(input.limit))
                    .map(|tx| project_transaction(&tx, &self.activity_key))
                    .collect();
                Ok(wire::Response::History(wire::History {
                    wallet_id: self.wallet_id.clone(),
                    confirmed_only: true,
                    transactions,
                }))
            }
            wire::Request::Policy(_) => {
                let policy = descriptor.policy();
                let path = |p: &coincube_core::descriptors::PathInfo, delay| {
                    let (threshold, key_count) = match p {
                        coincube_core::descriptors::PathInfo::Single(_) => (1, 1),
                        coincube_core::descriptors::PathInfo::Multi(k, keys) => (*k, keys.len()),
                    };
                    wire::PolicyPath {
                        threshold,
                        key_count,
                        relative_delay_blocks: delay,
                    }
                };
                Ok(wire::Response::Policy(wire::Policy {
                    wallet_id: self.wallet_id.clone(),
                    deterministic: true,
                    script_type: if descriptor.is_taproot() {
                        "p2tr"
                    } else {
                        "p2wsh"
                    }
                    .into(),
                    primary: path(policy.primary_path(), 0),
                    recovery: policy
                        .recovery_paths()
                        .iter()
                        .map(|(delay, p)| path(p, *delay))
                        .collect(),
                }))
            }
        }
    }
}

#[cfg(any(unix, windows))]
fn project_transaction(
    tx: &crate::daemon::model::HistoryTransaction,
    activity_key: &[u8; 32],
) -> wire::Transaction {
    wire::Transaction {
        activity_id: activity_id(activity_key, &tx.txid),
        kind: if tx.is_external() {
            wire::TransactionKind::Incoming
        } else if tx.is_send_to_self() {
            wire::TransactionKind::SelfTransfer
        } else {
            wire::TransactionKind::Outgoing
        },
        status: wire::TransactionStatus::Confirmed,
    }
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;
    use coincube_core::{
        descriptors::{CoincubeDescriptor, CoincubePolicy, PathInfo},
        miniscript::descriptor::DescriptorPublicKey,
    };
    use std::str::FromStr;

    #[test]
    fn pair_and_disable_clear_every_pending_permission() {
        let mut settings = Settings {
            enabled: true,
            wallet_allowed: true,
            capabilities: [true; 3],
            #[cfg(feature = "mcp-proposals")]
            proposals_allowed: true,
            #[cfg(feature = "mcp-proposals")]
            creation_allowed: true,
            #[cfg(feature = "mcp-transactions")]
            transactions_allowed: true,
            ..Settings::default()
        };
        settings.update(Message::Pair, None, None, true, "unopened-cube");
        assert!(!settings.wallet_allowed);
        assert_eq!(settings.capabilities, [false; 3]);
        #[cfg(feature = "mcp-proposals")]
        {
            assert!(!settings.proposals_allowed && !settings.creation_allowed);
        }
        #[cfg(feature = "mcp-transactions")]
        {
            assert!(!settings.transactions_allowed);
        }

        settings.wallet_allowed = true;
        settings.capabilities = [true; 3];
        #[cfg(feature = "mcp-proposals")]
        {
            settings.proposals_allowed = true;
            settings.creation_allowed = true;
        }
        #[cfg(feature = "mcp-transactions")]
        {
            settings.transactions_allowed = true;
        }
        settings.update(Message::Enable(false), None, None, true, "unopened-cube");
        assert!(!settings.wallet_allowed);
        assert_eq!(settings.capabilities, [false; 3]);
        #[cfg(feature = "mcp-proposals")]
        {
            assert!(!settings.proposals_allowed && !settings.creation_allowed);
        }
        #[cfg(feature = "mcp-transactions")]
        {
            assert!(!settings.transactions_allowed);
        }
    }

    #[test]
    fn ai_access_revoke_dialog_retains_merged_safety_focus() {
        use crate::app::menu::{CubeSettingsOption, CubeSubMenu, Menu};
        use coincube_ui::{font, theme, widget::Renderer};
        use iced::advanced::{
            renderer::Headless,
            widget::{operation, operation::focusable, operation::Focusable, Id, Operation},
        };
        use iced::{keyboard, mouse, Event, Rectangle, Size};
        use iced_runtime::user_interface::{Cache as UiCache, UserInterface};

        #[derive(Default)]
        struct Probe {
            text: Vec<(String, Rectangle)>,
            focus: Vec<(Id, Rectangle)>,
        }
        impl Operation for Probe {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn text(&mut self, _: Option<&Id>, bounds: Rectangle, value: &str) {
                self.text.push((value.to_owned(), bounds));
            }
            fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, _: &mut dyn Focusable) {
                if let Some(id) = id {
                    self.focus.push((id.clone(), bounds));
                }
            }
        }
        fn focused(
            ui: &mut UserInterface<'_, super::super::Message, theme::Theme, Renderer>,
            renderer: &Renderer,
        ) -> Option<Id> {
            use std::sync::{Arc, Mutex};
            let result = Arc::new(Mutex::new(None));
            let output = Arc::clone(&result);
            let mut op: Box<dyn Operation<()>> =
                Box::new(operation::map(focusable::find_focused(), move |value| {
                    *output.lock().unwrap() = Some(value)
                }));
            loop {
                ui.operate(renderer, &mut operation::black_box(op.as_mut()));
                match op.finish() {
                    operation::Outcome::None | operation::Outcome::Some(()) => break,
                    operation::Outcome::Chain(next) => op = next,
                }
            }
            drop(op);
            Arc::try_unwrap(result).unwrap().into_inner().unwrap()
        }
        fn apply(
            ui: &mut UserInterface<'_, super::super::Message, theme::Theme, Renderer>,
            renderer: &Renderer,
            operation: impl Operation<()> + 'static,
        ) {
            let mut operation: Box<dyn Operation<()>> = Box::new(operation);
            loop {
                ui.operate(renderer, &mut operation::black_box(operation.as_mut()));
                match operation.finish() {
                    operation::Outcome::None | operation::Outcome::Some(()) => break,
                    operation::Outcome::Chain(next) => operation = next,
                }
            }
        }
        fn apply_task(
            ui: &mut UserInterface<'_, super::super::Message, theme::Theme, Renderer>,
            renderer: &Renderer,
            task: iced::Task<super::super::Message>,
        ) {
            use iced::futures::StreamExt;
            let stream = iced_runtime::task::into_stream(task).expect("focus task stream");
            let actions = iced::futures::executor::block_on(stream.collect::<Vec<_>>());
            for action in actions {
                if let iced_runtime::Action::Widget(mut operation) = action {
                    loop {
                        ui.operate(renderer, &mut operation::black_box(operation.as_mut()));
                        match operation.finish() {
                            operation::Outcome::None | operation::Outcome::Some(()) => break,
                            operation::Outcome::Chain(next) => operation = next,
                        }
                    }
                }
            }
        }
        fn escape() -> Event {
            let key = keyboard::Key::Named(keyboard::key::Named::Escape);
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: None,
                repeat: false,
            })
        }

        let _guard = super::super::session::test_guard();
        let root = std::env::temp_dir().join(format!("mcp-modal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("bitcoin")).unwrap();
        let (mut app, wallet) = super::super::shell_tests::bitcoin_app(&root);
        let location = pairings::Location::new(&root, wallet.chain, &app.cube_settings.id).unwrap();
        let mut store =
            pairings::Store::open(location.clone(), zeroize::Zeroizing::new([51; 32]), true)
                .unwrap();
        let id = store
            .add(
                "Synthetic local client".into(),
                pairings::Scope::Read {
                    wallet: wallet.id().to_string(),
                    descriptor: pairings::descriptor_identity(&wallet),
                    opaque: "synthetic-opaque".into(),
                    capabilities: [true, false, false],
                },
            )
            .unwrap();
        app.local_mcp.enabled = true;
        app.local_mcp.pairing_location = Some(location);
        app.local_mcp.pairing_store = Some(store);
        app.local_mcp.revoke_confirmation = Some(id.clone());
        app.panels.current = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::AiAccess));
        app.show_recovery_alerts_prompt = false;

        for (width, height, palette) in [
            (1024.0, 768.0, theme::Theme::dark()),
            (1024.0, 768.0, theme::Theme::light()),
            (1440.0, 900.0, theme::Theme::dark()),
            (1440.0, 900.0, theme::Theme::light()),
        ] {
            for bytes in font::load() {
                iced::advanced::graphics::text::font_system()
                    .write()
                    .unwrap()
                    .load_font(bytes);
            }
            let mut renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
                font::REGULAR,
                16.into(),
                Some("tiny-skia"),
            ))
            .unwrap();
            let mut ui = UserInterface::build(
                app.view(),
                Size::new(width, height),
                UiCache::new(),
                &mut renderer,
            );
            ui.draw(
                &mut renderer,
                &palette,
                &iced::advanced::renderer::Style {
                    text_color: palette.colors.shell.ink,
                },
                mouse::Cursor::Unavailable,
            );
            let mut probe = Probe::default();
            ui.operate(&renderer, &mut operation::black_box(&mut probe));
            let title = probe
                .text
                .iter()
                .find(|(text, _)| text == "Revoke this client's access?")
                .expect("mounted revoke dialog title")
                .1;
            assert!(title.y >= 0.0 && title.y + title.height <= height);
            for target in [
                "tenshu.ai-access.revoke.keep",
                "tenshu.ai-access.revoke.confirm",
            ] {
                let (_, bounds) = probe
                    .focus
                    .iter()
                    .find(|(id, _)| id == &Id::from(target))
                    .expect("mounted dialog action");
                assert!(bounds.y >= 0.0 && bounds.y + bounds.height <= height);
            }
            assert_eq!(
                probe.focus.len(),
                2,
                "shell controls must be outside Tab order"
            );
            apply(&mut ui, &renderer, focusable::focus_next::<()>());
            assert_eq!(
                focused(&mut ui, &renderer),
                Some(Id::from("tenshu.ai-access.revoke.keep"))
            );
            let mut messages = Vec::new();
            ui.update(
                &[escape()],
                mouse::Cursor::Unavailable,
                &mut renderer,
                &mut iced::advanced::clipboard::Null,
                &mut messages,
            );
            assert!(messages.iter().any(|message| matches!(
                message,
                super::super::Message::View(view::Message::LocalMcp(Message::CancelRevokePair))
            )));
        }
        let mut renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            16.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        let cancel = app.update(super::super::Message::View(view::Message::LocalMcp(
            Message::CancelRevokePair,
        )));
        let mut ui = UserInterface::build(
            app.view(),
            Size::new(1024.0, 768.0),
            UiCache::new(),
            &mut renderer,
        );
        apply_task(&mut ui, &renderer, cancel);
        assert_eq!(
            focused(&mut ui, &renderer),
            Some(Id::from(format!("tenshu.ai-access.client.{id}.revoke")))
        );
        drop(ui);

        let request = app.update(super::super::Message::View(view::Message::LocalMcp(
            Message::RequestRevokePair(id.clone()),
        )));
        let mut ui = UserInterface::build(
            app.view(),
            Size::new(1024.0, 768.0),
            UiCache::new(),
            &mut renderer,
        );
        apply_task(&mut ui, &renderer, request);
        assert_eq!(
            focused(&mut ui, &renderer),
            Some(Id::from("tenshu.ai-access.revoke.keep"))
        );
        drop(ui);

        let confirm = app.update(super::super::Message::View(view::Message::LocalMcp(
            Message::RevokePair(id),
        )));
        let mut ui = UserInterface::build(
            app.view(),
            Size::new(1024.0, 768.0),
            UiCache::new(),
            &mut renderer,
        );
        apply_task(&mut ui, &renderer, confirm);
        // Focus returns to the Cube-wide switch, which is present whether or
        // not the pairing form is open and after a storage failure.
        assert_eq!(
            focused(&mut ui, &renderer),
            Some(Id::from("tenshu.ai-access.enable"))
        );
        drop(ui);
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Opening and closing the revoke dialog must not remount the AI Access
    /// page beneath it: `Modal::optional` keeps the page at one tree position,
    /// so its widget state (here, focus left on the Cube-wide switch) survives
    /// the round trip instead of being thrown away by iced's tree diff.
    #[test]
    fn ai_access_page_state_survives_revoke_dialog_open_and_close() {
        use crate::app::menu::{CubeSettingsOption, CubeSubMenu, Menu};
        use coincube_ui::{font, theme, widget::Renderer};
        use iced::advanced::{
            renderer::Headless,
            widget::{operation, operation::focusable, Id, Operation},
        };
        use iced::Size;
        use iced_runtime::user_interface::{Cache as UiCache, UserInterface};

        type Ui<'a> = UserInterface<'a, super::super::Message, theme::Theme, Renderer>;
        fn apply(ui: &mut Ui<'_>, renderer: &Renderer, operation: impl Operation<()> + 'static) {
            let mut operation: Box<dyn Operation<()>> = Box::new(operation);
            loop {
                ui.operate(renderer, &mut operation::black_box(operation.as_mut()));
                match operation.finish() {
                    operation::Outcome::None | operation::Outcome::Some(()) => break,
                    operation::Outcome::Chain(next) => operation = next,
                }
            }
        }
        fn focused(ui: &mut Ui<'_>, renderer: &Renderer) -> Option<Id> {
            use std::sync::{Arc, Mutex};
            let result = Arc::new(Mutex::new(None));
            let output = Arc::clone(&result);
            apply(
                ui,
                renderer,
                operation::map(focusable::find_focused(), move |id| {
                    *output.lock().unwrap() = Some(id)
                }),
            );
            let found = result.lock().unwrap().take();
            found
        }

        let _guard = super::super::session::test_guard();
        let root = std::env::temp_dir().join(format!("mcp-modal-tree-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("bitcoin")).unwrap();
        let (mut app, wallet) = super::super::shell_tests::bitcoin_app(&root);
        let location = pairings::Location::new(&root, wallet.chain, &app.cube_settings.id).unwrap();
        let mut store =
            pairings::Store::open(location.clone(), zeroize::Zeroizing::new([52; 32]), true)
                .unwrap();
        let id = store
            .add(
                "Synthetic local client".into(),
                pairings::Scope::Read {
                    wallet: wallet.id().to_string(),
                    descriptor: pairings::descriptor_identity(&wallet),
                    opaque: "synthetic-opaque".into(),
                    capabilities: [true, false, false],
                },
            )
            .unwrap();
        app.local_mcp.enabled = true;
        app.local_mcp.pairing_location = Some(location);
        app.local_mcp.pairing_store = Some(store);
        app.panels.current = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::AiAccess));
        app.show_recovery_alerts_prompt = false;

        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let mut renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            16.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        let size = Size::new(1024.0, 768.0);
        let switch = Id::from(presentation::ENABLE_ID);

        assert!(app.local_mcp.revoke_dialog().is_none());
        let mut ui = UserInterface::build(app.view(), size, UiCache::new(), &mut renderer);
        apply(&mut ui, &renderer, focusable::focus::<()>(switch.clone()));
        assert_eq!(focused(&mut ui, &renderer), Some(switch.clone()));
        let cache = ui.into_cache();

        // Open the dialog. The page is inert and outside traversal meanwhile.
        app.local_mcp.revoke_confirmation = Some(id);
        assert!(app.local_mcp.revoke_dialog().is_some());
        let mut ui = UserInterface::build(app.view(), size, cache, &mut renderer);
        assert_eq!(focused(&mut ui, &renderer), None);
        let cache = ui.into_cache();

        // Close it without the focus-restoring task: the page's own state
        // must still be there.
        app.local_mcp.revoke_confirmation = None;
        let mut ui = UserInterface::build(app.view(), size, cache, &mut renderer);
        assert_eq!(
            focused(&mut ui, &renderer),
            Some(switch),
            "opening and closing the revoke dialog must not reset the page's widget state"
        );
        drop(ui);
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn history_projection_attributes_only_wallet_spending() {
        use crate::daemon::model::{Coin, HistoryTransaction};
        use coincube_core::miniscript::bitcoin::{
            absolute, bip32, hashes::Hash, transaction, Address, Amount, Network, OutPoint,
            ScriptBuf, Transaction, TxIn, TxOut, WScriptHash,
        };
        let script = ScriptBuf::new_p2wsh(&WScriptHash::all_zeros());
        let previous_output = OutPoint {
            txid: coincube_core::miniscript::bitcoin::Txid::all_zeros(),
            vout: 1,
        };
        let coin = Coin {
            amount: Amount::from_sat(5_000_000_000),
            outpoint: previous_output,
            address: Address::from_script(&script, Network::Regtest).unwrap(),
            block_height: Some(1),
            derivation_index: bip32::ChildNumber::from(0),
            spend_info: None,
            is_immature: false,
            is_change: false,
            is_from_self: false,
        };
        // The first output belongs to the Vault; the second is sender change
        // for incoming payments and the recipient payment for outgoing ones.
        let transaction = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output,
                ..Default::default()
            }],
            output: vec![
                TxOut {
                    value: Amount::from_sat(1_000_000),
                    script_pubkey: script.clone(),
                },
                TxOut {
                    value: Amount::from_sat(4_998_999_844),
                    script_pubkey: ScriptBuf::new_p2wsh(&WScriptHash::from_byte_array([1; 32])),
                },
            ],
        };
        let mut batch = transaction.clone();
        batch.output.push(TxOut {
            value: Amount::from_sat(100_000),
            script_pubkey: batch.output[0].script_pubkey.clone(),
        });
        let batch_history = HistoryTransaction::new(
            batch,
            Some(103),
            Some(1234),
            vec![],
            vec![0, 2],
            Network::Regtest,
        );
        let activity_key = [7u8; 32];
        let projected_batch = project_transaction(&batch_history, &activity_key);
        assert_eq!(projected_batch.kind, wire::TransactionKind::Incoming);
        for (wallet_inputs, wallet_outputs, expected_kind) in [
            (vec![], vec![0], wire::TransactionKind::Incoming),
            (vec![], vec![0, 1], wire::TransactionKind::Incoming),
            (vec![coin.clone()], vec![0], wire::TransactionKind::Outgoing),
            (vec![coin], vec![0, 1], wire::TransactionKind::SelfTransfer),
        ] {
            let history = HistoryTransaction::new(
                transaction.clone(),
                Some(103),
                Some(1234),
                wallet_inputs,
                wallet_outputs,
                Network::Regtest,
            );
            let projected = project_transaction(&history, &activity_key);
            let chain_id = transaction.compute_txid().to_string();
            assert_eq!(projected.activity_id.len(), 32);
            assert!(projected.activity_id.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(!projected.activity_id.contains(chain_id.as_str()));
            assert_eq!(
                project_transaction(&history, &activity_key).activity_id,
                projected.activity_id
            );
            assert_ne!(
                project_transaction(&history, &[9u8; 32]).activity_id,
                projected.activity_id
            );
            assert_eq!(projected.kind, expected_kind);
            assert_eq!(projected.status, wire::TransactionStatus::Confirmed);
            let json = serde_json::to_value(&projected).unwrap();
            let row = json.as_object().unwrap();
            assert_eq!(row.len(), 3);
            for key in [
                "incoming_sats",
                "outgoing_sats",
                "timestamp",
                "height",
                "txid",
            ] {
                assert!(!row.contains_key(key));
            }
            assert!(!json.to_string().contains(&chain_id));
        }
    }

    /// Create a disposable real SQLite wallet and matching GUI wallet wrapper.
    pub(super) fn fixture() -> (
        std::path::PathBuf,
        Arc<Wallet>,
        Arc<dyn Daemon + Send + Sync>,
    ) {
        fixture_on_chain(crate::chain::ChainId::Bitcoin)
    }

    pub(super) fn fixture_on_chain(
        chain: crate::chain::ChainId,
    ) -> (
        std::path::PathBuf,
        Arc<Wallet>,
        Arc<dyn Daemon + Send + Sync>,
    ) {
        // Existing public-only test keys. No seed, xprv, production wallet or signing.
        let owner = DescriptorPublicKey::from_str("[aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*").unwrap();
        let recovery = DescriptorPublicKey::from_str("[aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/<0;1>/*").unwrap();
        let encode = |mut key: DescriptorPublicKey| {
            if let DescriptorPublicKey::MultiXPub(ref mut account) = key {
                account.xkey.network = chain.bitcoin_network().into();
            }
            key
        };
        let descriptor = CoincubeDescriptor::new(
            CoincubePolicy::new_legacy(
                PathInfo::Single(encode(owner)),
                [(144, PathInfo::Single(encode(recovery)))]
                    .iter()
                    .cloned()
                    .collect(),
            )
            .unwrap(),
        );
        let root = std::env::temp_dir().join(format!("mcp-wallet-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (config, handle) =
            coincubed::mcp_test_fixture::wallet_on_chain(&root, descriptor.clone(), chain);
        let daemon =
            Arc::new(crate::daemon::embedded::EmbeddedDaemon::from_regtest_handle(config, handle));
        (
            root,
            Arc::new(Wallet::new(descriptor).with_chain(chain)),
            daemon,
        )
    }
    /// Build a typed request using an opaque grant-scoped wallet identifier.
    fn input(id: &str) -> wire::WalletInput {
        wire::WalletInput {
            wallet_id: id.into(),
        }
    }

    #[test]
    fn successful_pair_requires_fresh_scope_for_next_client() {
        let mut settings = Settings {
            enabled: true,
            wallet_allowed: true,
            capabilities: [true, false, true],
            #[cfg(feature = "mcp-transactions")]
            transactions_allowed: true,
            #[cfg(feature = "mcp-proposals")]
            creation_allowed: true,
            client_name: "first client".into(),
            ..Settings::default()
        };
        settings.finish_pairing(Some(Ok("synthetic-id".into())));
        assert!(settings.client_name.is_empty());
        assert!(!settings.wallet_allowed);
        assert_eq!(settings.capabilities, [false; 3]);
        #[cfg(feature = "mcp-transactions")]
        assert!(!settings.transactions_allowed);
        #[cfg(feature = "mcp-proposals")]
        assert!(!settings.creation_allowed);

        settings.client_name = "second client".into();
        assert!(!settings.wallet_allowed && !settings.capabilities.iter().any(|v| *v));
    }

    #[test]
    #[ignore = "synthetic content-only capture; set R06A_CAPTURE_DIR for PNGs"]
    fn ai_access_synthetic_content_states_fit_and_disclose_scope() {
        use crate::app::view::shared::landing::capture;
        use coincube_ui::theme;
        let _guard = super::super::session::test_guard();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _enter = runtime.enter();
        let (root, wallet, daemon) = fixture();
        let cube = "mcp-visual-synthetic";
        let chain = wallet.chain;
        std::fs::create_dir_all(root.join(chain.dir_name())).unwrap();
        let location = pairings::Location::new(&root, chain, cube).unwrap();
        super::super::session::open_without_pin(cube);

        let empty = Settings {
            pairing_open: true,
            ..Settings::default()
        };
        let mut paired = Settings {
            enabled: true,
            pairing_open: true,
            pairing_location: Some(location.clone()),
            pairing_store: Some(
                pairings::Store::open(location, zeroize::Zeroizing::new([51; 32]), true).unwrap(),
            ),
            ..Settings::default()
        };
        for message in [
            Message::ClientName("Synthetic local client".into()),
            Message::Wallet(true),
            Message::Capability(0, true),
            Message::Pair,
        ] {
            paired.update(
                message,
                Some(wallet.clone()),
                Some(daemon.clone()),
                true,
                cube,
            );
        }
        paired.ensure_pairings(PairingContext {
            root: &root,
            chain,
            cube_id: cube,
            wallet: Some(wallet.clone()),
            daemon: Some(daemon.clone()),
            supported: true,
            proposal_supported: false,
            last_poll_at_startup: None,
        });
        assert_eq!(paired.paired.len(), 1);
        // Pairing closed the form and opened the new client; reopen the form
        // as a user pairing another client would.
        assert!(!paired.pairing_open);
        paired.pairing_open = true;
        paired.managing = Some(
            paired.pairing_store.as_ref().unwrap().clients()[0]
                .id
                .clone(),
        );
        let render = |scenario: &str, settings: &Settings| {
            for (size, appearance, palette) in [
                ((1024, 768), "dark", theme::Theme::dark()),
                ((1024, 768), "light", theme::Theme::light()),
                ((1440, 900), "dark", theme::Theme::dark()),
                ((1440, 900), "light", theme::Theme::light()),
            ] {
                let inspection = capture::inspect(
                    scenario,
                    settings.view_for_cube(true, false),
                    size,
                    appearance,
                    palette,
                );
                assert!(inspection.contains("Paired clients"));
                assert!(inspection.contains("Local isn't private."));
                assert!(inspection.contains("Only the parts you allow."));
                assert!(inspection.contains("Never shared"));
                assert!(!inspection.contains("Your AI provider may receive financial information"));
                assert!(!inspection.contains("COINCUBE_MCP_CREDENTIAL"));
            }
            let spanish =
                coincube_ui::locale::render_in(coincube_ui::locale::Language::Spanish, || {
                    settings.view_for_cube(true, false)
                });
            capture::inspect(scenario, spanish, (1440, 900), "dark", theme::Theme::dark())
                .assert_spanish(&format!("AI access ({scenario})"));
        };
        render("r23-empty", &empty);
        let empty_focus = capture::inspect(
            "r23-empty-focus",
            empty.view_for_cube(true, false),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(empty_focus
            .focus
            .iter()
            .any(|(id, _)| { id == &iced::advanced::widget::Id::from("tenshu.ai-access.enable") }));
        for key in [
            iced::keyboard::key::Named::Enter,
            iced::keyboard::key::Named::Space,
        ] {
            let emitted = capture::activate_with_key(
                empty.view_for_cube(true, false),
                iced::advanced::widget::Id::from("tenshu.ai-access.enable"),
                (1024, 768),
                false,
                key,
            );
            assert!(emitted
                .iter()
                .any(|intent| matches!(intent, view::Message::LocalMcp(Message::Enable(true)))));
        }
        render("r23-paired", &paired);
        let paired_focus = capture::inspect(
            "r23-paired-focus",
            paired.view_for_cube(true, false),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        for id in [
            "tenshu.ai-access.enable",
            "tenshu.ai-access.wallet",
            "tenshu.ai-access.permission.balances",
            "tenshu.ai-access.permission.history",
            "tenshu.ai-access.permission.policy",
        ] {
            assert!(paired_focus
                .focus
                .iter()
                .any(|(focus_id, _)| focus_id == &iced::advanced::widget::Id::from(id)));
        }
        let client_id = paired.pairing_store.as_ref().unwrap().clients()[0]
            .id
            .clone();
        let copy_id =
            iced::advanced::widget::Id::from(format!("tenshu.ai-access.client.{client_id}.copy"));
        let revoke_id =
            iced::advanced::widget::Id::from(format!("tenshu.ai-access.client.{client_id}.revoke"));
        for key in [
            iced::keyboard::key::Named::Enter,
            iced::keyboard::key::Named::Space,
        ] {
            let emitted = capture::activate_with_key(
                paired.view_for_cube(true, false),
                copy_id.clone(),
                (1024, 768),
                false,
                key,
            );
            assert!(emitted.iter().any(|intent| matches!(intent,
                view::Message::LocalMcp(Message::CopyPair(id)) if id == &client_id
            )));
        }
        let emitted = capture::activate(
            paired.view_for_cube(true, false),
            revoke_id.clone(),
            (1024, 768),
            true,
        );
        assert!(emitted.iter().any(|intent| matches!(intent,
            view::Message::LocalMcp(Message::RequestRevokePair(id)) if id == &client_id
        )));
        for (id, expected) in [
            ("tenshu.ai-access.enable", Message::Enable(false)),
            ("tenshu.ai-access.wallet", Message::Wallet(true)),
            (
                "tenshu.ai-access.permission.balances",
                Message::Capability(0, true),
            ),
            (
                "tenshu.ai-access.permission.history",
                Message::Capability(1, true),
            ),
            (
                "tenshu.ai-access.permission.policy",
                Message::Capability(2, true),
            ),
        ] {
            for key in [
                iced::keyboard::key::Named::Enter,
                iced::keyboard::key::Named::Space,
            ] {
                let emitted = capture::activate_with_key(
                    paired.view_for_cube(true, false),
                    iced::advanced::widget::Id::from(id),
                    (1024, 768),
                    false,
                    key,
                );
                assert!(emitted.iter().any(|intent| matches!(intent,
                    view::Message::LocalMcp(actual) if match (actual, &expected) {
                        (Message::Enable(actual), Message::Enable(expected)) => actual == expected,
                        (Message::Wallet(actual), Message::Wallet(expected)) => actual == expected,
                        (Message::Capability(actual_index, actual_value), Message::Capability(expected_index, expected_value)) => actual_index == expected_index && actual_value == expected_value,
                        _ => false,
                    }
                )), "{} should respond to {:?}", id, key);
            }
        }
        paired.wallet_allowed = true;
        paired.capabilities[0] = true;
        paired.client_name = "Next synthetic client".into();
        for key in [
            iced::keyboard::key::Named::Enter,
            iced::keyboard::key::Named::Space,
        ] {
            let emitted = capture::activate_with_key(
                paired.view_for_cube(true, false),
                iced::advanced::widget::Id::from("tenshu.ai-access.pair"),
                (1024, 768),
                false,
                key,
            );
            assert!(emitted
                .iter()
                .any(|intent| matches!(intent, view::Message::LocalMcp(Message::Pair))));
        }
        let original_count = paired.pairing_store.as_ref().unwrap().clients().len();
        paired.update(
            Message::RequestRevokePair(client_id.clone()),
            None,
            None,
            true,
            cube,
        );
        assert_eq!(
            paired.pairing_store.as_ref().unwrap().clients().len(),
            original_count
        );
        for (size, appearance, palette) in [
            ((1024, 768), "dark", theme::Theme::dark()),
            ((1024, 768), "light", theme::Theme::light()),
            ((1440, 900), "dark", theme::Theme::dark()),
            ((1440, 900), "light", theme::Theme::light()),
        ] {
            let inspection = capture::inspect(
                "r23-revoke-confirm",
                presentation::revoke_confirmation("Synthetic local client", &client_id),
                size,
                appearance,
                palette,
            );
            assert!(inspection.contains("Revoke this client's access?"));
            assert!(inspection.contains("Information already read may remain"));
        }
        for (id, expected) in [
            ("tenshu.ai-access.revoke.keep", false),
            ("tenshu.ai-access.revoke.confirm", true),
        ] {
            for key in [
                iced::keyboard::key::Named::Enter,
                iced::keyboard::key::Named::Space,
            ] {
                let emitted = capture::activate_with_key(
                    presentation::revoke_confirmation("Synthetic local client", &client_id),
                    iced::advanced::widget::Id::from(id),
                    (1024, 768),
                    false,
                    key,
                );
                assert!(
                    emitted.iter().any(|intent| matches!(intent,
                        view::Message::LocalMcp(Message::RevokePair(_)) if expected
                    ) || matches!(intent,
                        view::Message::LocalMcp(Message::CancelRevokePair) if !expected
                    )),
                    "{} should respond to {:?}",
                    id,
                    key
                );
            }
        }
        paired.update(Message::CancelRevokePair, None, None, true, cube);
        assert!(paired.revoke_confirmation.is_none());
        assert_eq!(
            paired.pairing_store.as_ref().unwrap().clients().len(),
            original_count
        );
        paired.update(
            Message::RevokePair(client_id.clone()),
            None,
            None,
            true,
            cube,
        );
        assert_eq!(
            paired.pairing_store.as_ref().unwrap().clients().len(),
            original_count
        );
        paired.revoke();
        super::super::session::close();
        render("r23-locked", &paired);
        let locked = capture::inspect(
            "r23-locked-focus",
            paired.view_for_cube(true, false),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(!locked.focus.iter().any(|(id, _)| id == &copy_id));
        assert!(locked.focus.iter().any(|(id, _)| id == &revoke_id));
        paired.update(
            Message::RequestRevokePair(client_id.clone()),
            None,
            None,
            true,
            cube,
        );
        assert_eq!(
            paired.revoke_confirmation.as_deref(),
            Some(client_id.as_str())
        );
        paired.update(Message::RevokePair(client_id), None, None, true, cube);
        assert!(paired.pairing_store.as_ref().unwrap().clients().is_empty());
        drop(paired);
        runtime.block_on(daemon.stop()).unwrap();
        drop(daemon);
        drop(wallet);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ai_access_revoke_dialog_is_mounted_over_dashboard_and_confines_focus() {
        use crate::app::menu::{CubeSettingsOption, CubeSubMenu, Menu};
        use coincube_ui::{font, theme, widget::Renderer};
        use iced::advanced::{
            renderer::Headless,
            widget::{operation, operation::focusable, operation::Focusable, Id, Operation},
        };
        use iced::{keyboard, mouse, Event, Rectangle, Size};
        use iced_runtime::user_interface::{Cache as UiCache, UserInterface};

        #[derive(Default)]
        struct Probe {
            text: Vec<(String, Rectangle)>,
            focus: Vec<(Id, Rectangle)>,
        }
        impl Operation for Probe {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn text(&mut self, _: Option<&Id>, bounds: Rectangle, value: &str) {
                self.text.push((value.to_owned(), bounds));
            }
            fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, _: &mut dyn Focusable) {
                if let Some(id) = id {
                    self.focus.push((id.clone(), bounds));
                }
            }
        }
        fn focused(
            ui: &mut UserInterface<'_, super::super::Message, theme::Theme, Renderer>,
            renderer: &Renderer,
        ) -> Option<Id> {
            use std::sync::{Arc, Mutex};
            let result = Arc::new(Mutex::new(None));
            let output = Arc::clone(&result);
            let mut op: Box<dyn Operation<()>> =
                Box::new(operation::map(focusable::find_focused(), move |value| {
                    *output.lock().unwrap() = Some(value)
                }));
            loop {
                ui.operate(renderer, &mut operation::black_box(op.as_mut()));
                match op.finish() {
                    operation::Outcome::None | operation::Outcome::Some(()) => break,
                    operation::Outcome::Chain(next) => op = next,
                }
            }
            drop(op);
            Arc::try_unwrap(result).unwrap().into_inner().unwrap()
        }
        fn apply(
            ui: &mut UserInterface<'_, super::super::Message, theme::Theme, Renderer>,
            renderer: &Renderer,
            operation: impl Operation<()> + 'static,
        ) {
            let mut operation: Box<dyn Operation<()>> = Box::new(operation);
            loop {
                ui.operate(renderer, &mut operation::black_box(operation.as_mut()));
                match operation.finish() {
                    operation::Outcome::None | operation::Outcome::Some(()) => break,
                    operation::Outcome::Chain(next) => operation = next,
                }
            }
        }
        fn apply_task(
            ui: &mut UserInterface<'_, super::super::Message, theme::Theme, Renderer>,
            renderer: &Renderer,
            task: iced::Task<super::super::Message>,
        ) {
            use iced::futures::StreamExt;
            let stream = iced_runtime::task::into_stream(task).expect("focus task stream");
            let actions = iced::futures::executor::block_on(stream.collect::<Vec<_>>());
            for action in actions {
                if let iced_runtime::Action::Widget(mut operation) = action {
                    loop {
                        ui.operate(renderer, &mut operation::black_box(operation.as_mut()));
                        match operation.finish() {
                            operation::Outcome::None | operation::Outcome::Some(()) => break,
                            operation::Outcome::Chain(next) => operation = next,
                        }
                    }
                }
            }
        }
        fn escape() -> Event {
            let key = keyboard::Key::Named(keyboard::key::Named::Escape);
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: None,
                repeat: false,
            })
        }

        let _guard = super::super::session::test_guard();
        let root = std::env::temp_dir().join(format!("mcp-modal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("bitcoin")).unwrap();
        let (mut app, wallet) = super::super::shell_tests::bitcoin_app(&root);
        let location = pairings::Location::new(&root, wallet.chain, &app.cube_settings.id).unwrap();
        let mut store =
            pairings::Store::open(location.clone(), zeroize::Zeroizing::new([51; 32]), true)
                .unwrap();
        let id = store
            .add(
                "Synthetic local client".into(),
                pairings::Scope::Read {
                    wallet: wallet.id().to_string(),
                    descriptor: pairings::descriptor_identity(&wallet),
                    opaque: "synthetic-opaque".into(),
                    capabilities: [true, false, false],
                },
            )
            .unwrap();
        app.local_mcp.enabled = true;
        app.local_mcp.pairing_location = Some(location);
        app.local_mcp.pairing_store = Some(store);
        app.local_mcp.revoke_confirmation = Some(id.clone());
        app.panels.current = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::AiAccess));
        app.show_recovery_alerts_prompt = false;

        for (width, height) in [(1024.0, 768.0), (1440.0, 900.0)] {
            for bytes in font::load() {
                iced::advanced::graphics::text::font_system()
                    .write()
                    .unwrap()
                    .load_font(bytes);
            }
            let mut renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
                font::REGULAR,
                16.into(),
                Some("tiny-skia"),
            ))
            .unwrap();
            let mut ui = UserInterface::build(
                app.view(),
                Size::new(width, height),
                UiCache::new(),
                &mut renderer,
            );
            let mut probe = Probe::default();
            ui.operate(&renderer, &mut operation::black_box(&mut probe));
            let title = probe
                .text
                .iter()
                .find(|(text, _)| text == "Revoke this client's access?")
                .expect("mounted revoke dialog title")
                .1;
            assert!(title.y >= 0.0 && title.y + title.height <= height);
            for target in [
                "tenshu.ai-access.revoke.keep",
                "tenshu.ai-access.revoke.confirm",
            ] {
                let (_, bounds) = probe
                    .focus
                    .iter()
                    .find(|(id, _)| id == &Id::from(target))
                    .expect("mounted dialog action");
                assert!(bounds.y >= 0.0 && bounds.y + bounds.height <= height);
            }
            assert_eq!(
                probe.focus.len(),
                2,
                "shell controls must be outside Tab order"
            );
            apply(&mut ui, &renderer, focusable::focus_next::<()>());
            assert_eq!(
                focused(&mut ui, &renderer),
                Some(Id::from("tenshu.ai-access.revoke.keep"))
            );
            let mut messages = Vec::new();
            ui.update(
                &[escape()],
                mouse::Cursor::Unavailable,
                &mut renderer,
                &mut iced::advanced::clipboard::Null,
                &mut messages,
            );
            assert!(messages.iter().any(|message| matches!(
                message,
                super::super::Message::View(view::Message::LocalMcp(Message::CancelRevokePair))
            )));
        }
        let mut renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            16.into(),
            Some("tiny-skia"),
        ))
        .unwrap();
        let cancel = app.update(super::super::Message::View(view::Message::LocalMcp(
            Message::CancelRevokePair,
        )));
        let mut ui = UserInterface::build(
            app.view(),
            Size::new(1024.0, 768.0),
            UiCache::new(),
            &mut renderer,
        );
        apply_task(&mut ui, &renderer, cancel);
        assert_eq!(
            focused(&mut ui, &renderer),
            Some(Id::from(format!("tenshu.ai-access.client.{id}.revoke")))
        );
        drop(ui);

        let request = app.update(super::super::Message::View(view::Message::LocalMcp(
            Message::RequestRevokePair(id.clone()),
        )));
        let mut ui = UserInterface::build(
            app.view(),
            Size::new(1024.0, 768.0),
            UiCache::new(),
            &mut renderer,
        );
        apply_task(&mut ui, &renderer, request);
        assert_eq!(
            focused(&mut ui, &renderer),
            Some(Id::from("tenshu.ai-access.revoke.keep"))
        );
        drop(ui);

        let confirm = app.update(super::super::Message::View(view::Message::LocalMcp(
            Message::RevokePair(id),
        )));
        let mut ui = UserInterface::build(
            app.view(),
            Size::new(1024.0, 768.0),
            UiCache::new(),
            &mut renderer,
        );
        apply_task(&mut ui, &renderer, confirm);
        // Focus returns to the Cube-wide switch, which is present whether or
        // not the pairing form is open and after a storage failure.
        assert_eq!(
            focused(&mut ui, &renderer),
            Some(Id::from("tenshu.ai-access.enable"))
        );
        drop(ui);
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistent_pairings_resume_after_lock_restart_and_revoke_independently() {
        use serde_json::Value;
        let _guard = super::super::session::test_guard();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root, wallet, daemon) = fixture();
            let chain = wallet.chain;
            std::fs::create_dir_all(root.join(chain.dir_name())).unwrap();
            let location = pairings::Location::new(&root, chain, "mcp-paired-fixture").unwrap();
            // Synthetic native consent / test wrapping key. Actual packaged UI
            // and OS-keystore acceptance are separate signed-artifact checks.
            let attach = |create| Settings {
                enabled: true,
                pairing_location: Some(location.clone()),
                pairing_store: Some(
                    pairings::Store::open(
                        location.clone(),
                        zeroize::Zeroizing::new([23; 32]),
                        create,
                    )
                    .unwrap(),
                ),
                ..Settings::default()
            };
            let refresh = |settings: &mut Settings| {
                settings.ensure_pairings(PairingContext {
                    root: &root,
                    chain,
                    cube_id: "mcp-paired-fixture",
                    wallet: Some(wallet.clone()),
                    daemon: Some(daemon.clone()),
                    supported: true,
                    proposal_supported: false,
                    last_poll_at_startup: None,
                })
            };
            let configuration = |settings: &mut Settings, id: &str| -> Value {
                serde_json::from_str(
                    &settings
                        .update(
                            Message::CopyPair(id.into()),
                            None,
                            None,
                            true,
                            "mcp-paired-fixture",
                        )
                        .unwrap(),
                )
                .unwrap()
            };
            super::super::session::open_without_pin("mcp-paired-fixture");
            let mut settings = attach(true);
            for message in [
                Message::ClientName("Balance client".into()),
                Message::Wallet(true),
                Message::Capability(0, true),
                Message::Pair,
            ] {
                settings.update(
                    message,
                    Some(wallet.clone()),
                    Some(daemon.clone()),
                    true,
                    "mcp-paired-fixture",
                );
            }
            refresh(&mut settings);
            let first = settings.pairing_store.as_ref().unwrap().clients()[0]
                .id
                .clone();
            let original = configuration(&mut settings, &first);
            let vars = &original["mcpServers"]["tenshu"]["env"];
            let path = std::path::PathBuf::from(vars["COINCUBE_MCP_PAIRING"].as_str().unwrap());
            let token = vars["COINCUBE_MCP_CREDENTIAL"].as_str().unwrap();
            let socket = wire::pairing::resolve(&path).unwrap();
            let wire::Response::List(list) =
                wire::ipc::call(&socket, token, wire::Request::List(wire::Empty {}))
                    .await
                    .unwrap()
            else {
                panic!("list")
            };
            let opaque = list.wallets[0].wallet_id.clone();
            assert!(
                wire::ipc::call(&socket, token, wire::Request::Summary(input(&opaque)))
                    .await
                    .is_ok()
            );
            assert_eq!(
                wire::ipc::call(
                    &socket,
                    token,
                    wire::Request::History(wire::HistoryInput {
                        wallet_id: opaque.clone(),
                        limit: 1
                    })
                )
                .await
                .unwrap_err(),
                wire::Error::AccessDenied
            );
            for message in [
                Message::ClientName("History client".into()),
                Message::Wallet(true),
                Message::Capability(0, false),
                Message::Capability(1, true),
                Message::Pair,
            ] {
                settings.update(
                    message,
                    Some(wallet.clone()),
                    Some(daemon.clone()),
                    true,
                    "mcp-paired-fixture",
                );
            }
            refresh(&mut settings);
            let second = settings.pairing_store.as_ref().unwrap().clients()[1]
                .id
                .clone();
            let second_cfg = configuration(&mut settings, &second);
            let second_vars = &second_cfg["mcpServers"]["tenshu"]["env"];
            let second_path =
                std::path::PathBuf::from(second_vars["COINCUBE_MCP_PAIRING"].as_str().unwrap());
            let second_token = second_vars["COINCUBE_MCP_CREDENTIAL"].as_str().unwrap();
            assert_eq!(
                wire::ipc::call(
                    &wire::pairing::resolve(&second_path).unwrap(),
                    token,
                    wire::Request::List(wire::Empty {})
                )
                .await
                .unwrap_err(),
                wire::Error::AccessDenied
            );
            super::super::session::close();
            assert!(
                settings
                    .update(
                        Message::CopyPair(first.clone()),
                        None,
                        None,
                        true,
                        "mcp-paired-fixture"
                    )
                    .is_none(),
                "locked Cube must not reveal a private client configuration"
            );
            assert_eq!(
                wire::ipc::call(
                    &wire::pairing::resolve(&path).unwrap(),
                    token,
                    wire::Request::Summary(input(&opaque))
                )
                .await
                .unwrap_err(),
                wire::Error::AccessDenied
            );
            drop(settings);
            assert!(wire::pairing::resolve(&path).is_err());
            super::super::session::open_without_pin("mcp-paired-fixture");
            let mut settings = attach(false);
            refresh(&mut settings);
            assert_eq!(
                configuration(&mut settings, &first),
                original,
                "no credential/config replacement after restart"
            );
            let socket = wire::pairing::resolve(&path).unwrap();
            assert!(
                wire::ipc::call(&socket, token, wire::Request::Summary(input(&opaque)))
                    .await
                    .is_ok()
            );
            settings.update(
                Message::RequestRevokePair(first.clone()),
                None,
                None,
                true,
                "mcp-paired-fixture",
            );
            settings.update(
                Message::RevokePair(first),
                None,
                None,
                true,
                "mcp-paired-fixture",
            );
            refresh(&mut settings);
            assert!(wire::pairing::resolve(&path).is_err());
            assert!(wire::ipc::call(
                &wire::pairing::resolve(&second_path).unwrap(),
                second_token,
                wire::Request::List(wire::Empty {})
            )
            .await
            .is_ok());
            drop(settings);
            let mut settings = attach(false);
            refresh(&mut settings);
            assert_eq!(settings.pairing_store.as_ref().unwrap().clients().len(), 1);
            assert!(
                wire::pairing::resolve(&path).is_err(),
                "revoked pairing must not return on restart"
            );
            assert!(wire::ipc::call(
                &wire::pairing::resolve(&second_path).unwrap(),
                second_token,
                wire::Request::List(wire::Empty {})
            )
            .await
            .is_ok());
            let cube = super::super::settings::CubeSettings::new_with_raw_id(
                "mcp-paired-fixture".into(),
                "fixture".into(),
                chain,
            );
            assert!(
                remove_cube_pairings(&root, &cube).is_err(),
                "active registry ownership must prevent deletion"
            );
            let retained_runtime = settings.paired.pop().unwrap();
            drop(settings);
            assert!(
                pairings::Store::open(location.clone(), zeroize::Zeroizing::new([23; 32]), false)
                    .is_err(),
                "runtime must retain registry ownership after Settings drops"
            );
            drop(retained_runtime);
            let mut replaced = (*wallet).clone();
            replaced.pinned_at = Some(wallet.pinned_at.unwrap_or(0) + 1);
            let mut settings = attach(false);
            settings.ensure_pairings(PairingContext {
                root: &root,
                chain,
                cube_id: "mcp-paired-fixture",
                wallet: Some(Arc::new(replaced)),
                daemon: Some(daemon.clone()),
                supported: true,
                proposal_supported: false,
                last_poll_at_startup: None,
            });
            assert!(
                settings.paired.is_empty(),
                "replacement wallet cannot inherit read permissions"
            );
            assert!(wire::pairing::resolve(&second_path).is_err());
            drop(settings);
            remove_cube_pairings(&root, &cube).unwrap();
            assert!(
                !location.directory.exists(),
                "Cube deletion removes pairing authorization and locators"
            );
            daemon.stop().await.unwrap();
            drop(daemon);
            drop(wallet);
            super::super::session::close();
            std::fs::remove_dir_all(root).unwrap();
        });
    }

    // Runs separately after building the installation's companion executable.
    // Unlike the isolated protocol/IPC tests, this crosses both process and
    // wallet-service boundaries, using the configuration issued by Settings.
    #[test]
    #[ignore = "build coincube-mcp and set COINCUBE_MCP_TEST_ADAPTER; run explicitly in CI"]
    /// Exercise an actual companion process against SQLite and verify revocation.
    fn stdio_client_reads_real_wallet_and_observes_revocation() {
        use serde_json::{json, Value};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let _session_guard = super::super::session::test_guard();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            super::super::session::open_without_pin("mcp-fixture");
            let (root, wallet, daemon) = fixture();
            let mut settings = Settings::default();
            for message in [
                Message::Enable(true), Message::ClientName("stdio fixture".into()),
                Message::Wallet(true), Message::Capability(0, true),
                Message::Capability(1, true), Message::Capability(2, true), Message::Approve,
            ] {
                settings.update(message, Some(wallet.clone()), Some(daemon.clone()), true, "mcp-fixture");
            }
            let config: Value = serde_json::from_str(&settings.update(
                Message::CopyConfig, None, None, true, "mcp-fixture"
            ).unwrap()).unwrap();
            let environment = config["mcpServers"]["tenshu"]["env"].as_object().unwrap();
            let executable = std::env::var_os("COINCUBE_MCP_TEST_ADAPTER")
                .expect("set COINCUBE_MCP_TEST_ADAPTER to the built adapter");
            let mut child = tokio::process::Command::new(executable)
                .envs(environment.iter().map(|(k,v)| (k, v.as_str().unwrap())))
                .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped()).kill_on_drop(true).spawn().unwrap();
            let mut stdin = child.stdin.take().unwrap();
            let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
            /// Send a request and require the next JSON-RPC response to match its ID.
            async fn response(
                stdin: &mut tokio::process::ChildStdin,
                stdout: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
                request: Value,
            ) -> Value {
                let mut bytes = serde_json::to_vec(&request).unwrap();
                bytes.push(b'\n');
                stdin.write_all(&bytes).await.unwrap();
                let line = tokio::time::timeout(Duration::from_secs(10), stdout.next_line())
                    .await.unwrap().unwrap().unwrap();
                let value: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(value["id"], request["id"]);
                assert!(value.get("error").is_none(), "protocol error");
                value["result"].clone()
            }
            let initialized = response(&mut stdin, &mut stdout, json!({
                "jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
                    "protocolVersion":"2024-11-05", "capabilities":{},
                    "clientInfo":{"name":"wallet-acceptance", "version":"1"}
                }
            })).await;
            assert_eq!(initialized["serverInfo"]["name"], "coincube-mcp");
            stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await.unwrap();
            let discovered = response(&mut stdin, &mut stdout, json!({"jsonrpc":"2.0", "id":2, "method":"tools/list"})).await;
            let tools = discovered["tools"].as_array().unwrap();
            let names: std::collections::BTreeSet<_> = tools.iter()
                .map(|tool| tool["name"].as_str().unwrap()).collect();
            // The shipping companion advertises both native-preparation
            // workflows. This read-only pairing must still deny every write.
            assert_eq!(tools.len(), 10);
            assert_eq!(names, std::collections::BTreeSet::from([
                "tenshu_get_capabilities", "tenshu_list_wallets",
                "tenshu_get_wallet_summary", "tenshu_get_transactions",
                "tenshu_get_wallet_policy_summary", "tenshu_prepare_wallet_proposal",
                "tenshu_get_wallet_proposal", "tenshu_prepare_wallet_creation",
                "tenshu_prepare_transaction", "tenshu_get_prepared_transaction",
            ]));
            let call = |id, name: &str, arguments: Value| json!({"jsonrpc":"2.0", "id":id, "method":"tools/call", "params":{"name":name,"arguments":arguments}});
            let listed = response(&mut stdin, &mut stdout, call(3, "tenshu_list_wallets", json!({}))).await;
            assert_eq!(listed["structuredContent"]["wallets"][0]["network"], "bitcoin");
            let id = listed["structuredContent"]["wallets"][0]["wallet_id"].as_str().unwrap();
            let summary = response(&mut stdin, &mut stdout, call(4, "tenshu_get_wallet_summary", json!({"wallet_id":id}))).await;
            assert_eq!(summary["structuredContent"]["confirmed_sats"], 55_000);
            let history = response(&mut stdin, &mut stdout, call(5, "tenshu_get_transactions", json!({"wallet_id":id,"limit":10}))).await;
            let row = history["structuredContent"]["transactions"][0].as_object().unwrap();
            assert_eq!(row["kind"], "incoming");
            assert_eq!(row["status"], "confirmed");
            assert_eq!(row.len(), 3);
            for key in ["incoming_sats", "outgoing_sats", "timestamp", "height", "txid"] {
                assert!(!row.contains_key(key));
            }
            let policy = response(&mut stdin, &mut stdout, call(6, "tenshu_get_wallet_policy_summary", json!({"wallet_id":id}))).await;
            assert_eq!(policy["structuredContent"]["recovery"][0]["relative_delay_blocks"], 144);
            let denied = response(&mut stdin, &mut stdout, call(7, "tenshu_get_wallet_summary", json!({"wallet_id":"another-wallet"}))).await;
            assert_eq!(denied["isError"], true);
            assert_eq!(denied["structuredContent"]["error"]["code"], "access_denied");
            let setup_denied = response(&mut stdin, &mut stdout, call(10, "tenshu_prepare_wallet_creation", json!({
                "network":"bitcoin", "request_id":"read-grant-must-not-prepare",
                "topology":{
                    "primary":{"threshold":3,"key_count":5},
                    "recovery":{"quorum":{"threshold":2,"key_count":3},"relative_delay_blocks":144}
                },
                "keys":{"primary":vec!["unverified";5],"recovery":vec!["unverified";3]}
            }))).await;
            assert_eq!(setup_denied["isError"], true);
            assert_eq!(setup_denied["structuredContent"], json!({"error":{"code":"access_denied"}}));
            let transaction_denied = response(&mut stdin, &mut stdout, call(11, "tenshu_prepare_transaction", json!({
                "network":"bitcoin", "wallet_id": id, "request_id": "read-grant-must-not-prepare",
                "recipients": [{"address": "bcrt1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqdku202", "amount_sats": 10000}],
                "fee_rate_sat_vb": 2, "max_fee_sats": 1000
            }))).await;
            assert_eq!(transaction_denied["isError"], true);
            assert_eq!(transaction_denied["structuredContent"], json!({"error":{"code":"access_denied"}}));
            let draft_denied = response(&mut stdin, &mut stdout, call(12, "tenshu_get_prepared_transaction", json!({"wallet_id": id, "draft_id": "foreign"}))).await;
            assert_eq!(draft_denied["structuredContent"], json!({"error":{"code":"access_denied"}}));
            super::super::session::close();
            let locked = response(&mut stdin, &mut stdout, call(8, "tenshu_get_wallet_summary", json!({"wallet_id":id}))).await;
            assert_eq!(locked["structuredContent"]["error"]["code"], "access_denied");
            settings.update(Message::Revoke, None, None, true, "mcp-fixture");
            let revoked = response(&mut stdin, &mut stdout, call(9, "tenshu_list_wallets", json!({}))).await;
            assert_eq!(revoked["isError"], true);
            assert!(revoked["structuredContent"].get("wallets").is_none());
            drop(stdin);
            let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await.unwrap().unwrap();
            assert!(output.status.success());
            assert!(output.stderr.is_empty());
            drop(settings);
            daemon.stop().await.unwrap();
            drop(daemon);
            drop(wallet);
            std::fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    /// Verify consent, all read projections and server teardown with a real database.
    fn real_sqlite_wallet_reads_require_ui_grant_and_revoke_on_settings_drop() {
        let _session_guard = super::super::session::test_guard();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            super::super::session::open_without_pin("mcp-fixture");
            let (root, wallet, daemon) = fixture();
            let chain_txid = daemon.list_history_txs(0, u32::MAX, 1).await.unwrap()[0]
                .txid
                .to_string();
            let mut settings = Settings::default();
            assert!(!settings.enabled && settings.server.is_none());
            settings.update(
                Message::Approve,
                Some(wallet.clone()),
                Some(daemon.clone()),
                true,
                "mcp-fixture",
            );
            assert!(settings.server.is_none());
            for message in [
                Message::Enable(true),
                Message::ClientName("fixture client".into()),
                Message::Wallet(true),
                Message::Capability(0, true),
                Message::Capability(1, true),
                Message::Capability(2, true),
                Message::Approve,
            ] {
                settings.update(
                    message,
                    Some(wallet.clone()),
                    Some(daemon.clone()),
                    true,
                    "mcp-fixture",
                );
            }
            let server = settings.server.as_ref().unwrap();
            let socket = server.socket.clone();
            let credential = server.grant.credential().to_owned();
            let response =
                wire::ipc::call(&socket, &credential, wire::Request::List(wire::Empty {}))
                    .await
                    .unwrap();
            let wire::Response::List(list) = response else {
                panic!("expected list")
            };
            assert_eq!(list.wallets.len(), 1);
            let id = &list.wallets[0].wallet_id;
            let summary = wire::ipc::call(&socket, &credential, wire::Request::Summary(input(id)))
                .await
                .unwrap();
            let wire::Response::Summary(summary) = summary else {
                panic!("expected summary")
            };
            assert_eq!(
                (
                    summary.confirmed_sats,
                    summary.available_sats,
                    summary.unconfirmed_sats
                ),
                (55_000, 55_000, 0)
            );
            let history = wire::ipc::call(
                &socket,
                &credential,
                wire::Request::History(wire::HistoryInput {
                    wallet_id: id.clone(),
                    limit: 10,
                }),
            )
            .await
            .unwrap();
            let wire::Response::History(history) = history else {
                panic!("expected history")
            };
            assert_eq!(history.transactions.len(), 1);
            assert_eq!(
                history.transactions[0].kind,
                wire::TransactionKind::Incoming
            );
            assert_eq!(
                history.transactions[0].status,
                wire::TransactionStatus::Confirmed
            );
            let history_json = serde_json::to_value(&history).unwrap();
            let row = history_json["transactions"][0].as_object().unwrap();
            assert_eq!(row.len(), 3);
            for key in [
                "incoming_sats",
                "outgoing_sats",
                "timestamp",
                "height",
                "txid",
            ] {
                assert!(!row.contains_key(key));
            }
            assert!(!history_json.to_string().contains(&chain_txid));
            let policy = wire::ipc::call(&socket, &credential, wire::Request::Policy(input(id)))
                .await
                .unwrap();
            let wire::Response::Policy(policy) = policy else {
                panic!("expected policy")
            };
            assert_eq!((policy.primary.threshold, policy.primary.key_count), (1, 1));
            assert_eq!(policy.recovery[0].relative_delay_blocks, 144);
            let serialized = serde_json::to_string(&(summary, history, policy)).unwrap();
            for private in [
                "xpub",
                "descriptor",
                "script_pubkey",
                "labels",
                "seed",
                "private_key",
                "credential",
            ] {
                assert!(!serialized.contains(private));
            }
            assert_eq!(
                wire::ipc::call(
                    &socket,
                    &credential,
                    wire::Request::Summary(input("other-wallet"))
                )
                .await
                .unwrap_err(),
                wire::Error::AccessDenied
            );
            super::super::session::close();
            assert_eq!(
                wire::ipc::call(&socket, &credential, wire::Request::Summary(input(id)))
                    .await
                    .unwrap_err(),
                wire::Error::AccessDenied
            );
            super::super::session::open_without_pin("mcp-fixture");
            assert_eq!(
                wire::ipc::call(&socket, &credential, wire::Request::Summary(input(id)))
                    .await
                    .unwrap_err(),
                wire::Error::AccessDenied
            );
            // Reapprove in the new session so Settings drop is tested independently
            // of the old session's already-revoked grant.
            settings.update(
                Message::Approve,
                Some(wallet.clone()),
                Some(daemon.clone()),
                true,
                "mcp-fixture",
            );
            let server = settings.server.as_ref().unwrap();
            let grant = server.grant.clone();
            let socket = server.socket.clone();
            let credential = grant.credential().to_owned();
            wire::ipc::call(&socket, &credential, wire::Request::List(wire::Empty {}))
                .await
                .unwrap();
            assert!(grant.active());
            // App owns Settings; dropping it revokes synchronously on both OSes.
            // Windows pipe handles are closed when the aborted task is polled;
            // only Unix has a socket file that must be removed immediately.
            drop(settings);
            assert!(!grant.active());
            #[cfg(unix)]
            assert!(!socket.exists());
            assert!(
                wire::ipc::call(&socket, &credential, wire::Request::List(wire::Empty {}))
                    .await
                    .is_err()
            );
            daemon.stop().await.unwrap();
            drop(daemon);
            drop(wallet);
            super::super::session::close();
            std::fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    /// Ensure disabling access or narrowing capabilities invalidates existing client JSON.
    fn disable_and_capability_edit_invalidate_old_configuration() {
        let _session_guard = super::super::session::test_guard();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            super::super::session::open_without_pin("mcp-fixture");
            let (root, wallet, daemon) = fixture();
            let mut settings = Settings::default();
            for message in [
                Message::Enable(true),
                Message::ClientName("client".into()),
                Message::Wallet(true),
                Message::Capability(0, true),
                Message::Approve,
            ] {
                settings.update(
                    message,
                    Some(wallet.clone()),
                    Some(daemon.clone()),
                    true,
                    "mcp-fixture",
                );
            }
            let permission = settings.server.as_ref().unwrap().grant.clone();
            settings.update(
                Message::Capability(1, true),
                None,
                None,
                true,
                "mcp-fixture",
            );
            assert!(!permission.active());
            assert!(settings.server.is_none());
            settings.update(
                Message::Approve,
                Some(wallet.clone()),
                Some(daemon.clone()),
                true,
                "mcp-fixture",
            );
            let permission = settings.server.as_ref().unwrap().grant.clone();
            settings.update(Message::Enable(false), None, None, true, "mcp-fixture");
            assert!(!permission.active());
            assert!(!settings.enabled);
            settings.update(
                Message::Approve,
                Some(wallet.clone()),
                Some(daemon.clone()),
                true,
                "mcp-fixture",
            );
            assert!(settings.server.is_none());
            daemon.stop().await.unwrap();
            drop(daemon);
            drop(wallet);
            drop(settings);
            super::super::session::close();
            std::fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    /// Distinguish ordinary account refresh from a backend transition that must revoke.
    fn app_preserves_grant_on_connect_refresh_but_revokes_on_backend_switch() {
        use super::super::{message::Message as AppMessage, App};
        let _session_guard = super::super::session::test_guard();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _enter = runtime.enter();
        let (root, wallet, daemon) = fixture();
        let (mut app, startup) = App::new_inner(
            super::super::Cache::default(),
            wallet,
            None,
            None,
            None,
            super::super::Config::new(false),
            daemon.clone(),
            super::super::CoincubeDirectory::new(root.clone()),
            None,
            super::super::settings::CubeSettings::new(
                "MCP fixture".into(),
                crate::chain::ChainId::Bitcoin,
            ),
            None,
        );
        drop(startup);
        super::super::session::open_without_pin(app.cube_settings.id.clone());
        for message in [
            Message::Enable(true),
            Message::Wallet(true),
            Message::ClientName("client".into()),
            Message::Capability(0, true),
            Message::Approve,
        ] {
            drop(app.update(AppMessage::View(view::Message::LocalMcp(message))));
        }
        let grant = app.local_mcp.server.as_ref().unwrap().grant.clone();
        let generation = app.panels.connect.account.session_generation();
        drop(app.update(AppMessage::View(view::Message::ConnectAccount(
            view::ConnectAccountMessage::FeaturesLoaded(None, generation),
        ))));
        assert!(
            grant.active(),
            "routine Connect refresh must preserve the local grant"
        );
        app.revoke_claim();
        drop(app.update(AppMessage::SettingsSaved));
        assert!(
            grant.active(),
            "unrelated claim/settings changes must preserve access"
        );
        let config = daemon.config().unwrap().clone();
        drop(app.spawn_daemon_switch(config));
        assert!(
            !grant.active(),
            "backend switch must revoke before dispatch"
        );
        app.stop();
        drop(app);
        drop(daemon);
        super::super::session::close();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, feature = "mcp-proposals"))]
mod proposal_consent_tests {
    use super::*;
    #[test]
    fn proposal_creation_pairing_restores_scope_but_never_old_drafts() {
        let _guard = crate::app::session::test_guard();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let root = crate::installer::proposal::tests::TestDir::new();
            std::fs::create_dir(root.path().join("regtest")).unwrap();
            let cube = "public-key-pairing-fixture";
            let location =
                pairings::Location::new(root.path(), crate::chain::ChainId::Regtest, cube).unwrap();
            let mut settings = Settings {
                enabled: true,
                proposals_allowed: true,
                pairing_location: Some(location.clone()),
                pairing_store: Some(
                    pairings::Store::open(
                        location.clone(),
                        zeroize::Zeroizing::new([46; 32]),
                        true,
                    )
                    .unwrap(),
                ),
                ..Settings::default()
            };
            crate::app::session::open_without_pin(cube);
            let refresh = |settings: &mut Settings| {
                settings.ensure_pairings(PairingContext {
                    root: root.path(),
                    chain: crate::chain::ChainId::Regtest,
                    cube_id: cube,
                    wallet: None,
                    daemon: None,
                    supported: false,
                    proposal_supported: true,
                    last_poll_at_startup: None,
                })
            };
            settings.update(Message::Creation(true), None, None, true, cube);
            assert!(!settings.proposals_allowed);
            settings.update(
                Message::ClientName("Creation fixture".into()),
                None,
                None,
                true,
                cube,
            );
            settings.update(Message::Pair, None, None, true, cube);
            refresh(&mut settings);
            assert_eq!(settings.paired.len(), 1);
            let credential = settings.paired[0].server.grant.credential().to_owned();
            let socket = settings.paired[0].server.socket.clone();
            let input = crate::installer::creation::tests::input();
            let wire::Response::Proposal(first) = wire::ipc::call(
                &socket,
                &credential,
                wire::Request::PrepareCreation(input.clone()),
            )
            .await
            .unwrap() else {
                panic!("validated proposal")
            };
            assert!(first.descriptor_validated);
            assert!(
                wire::ipc::call(&socket, &credential, wire::Request::List(wire::Empty {}))
                    .await
                    .is_err()
            );
            crate::app::session::close();
            assert!(wire::ipc::call(
                &socket,
                &credential,
                wire::Request::PrepareCreation(input.clone())
            )
            .await
            .is_err());
            settings.revoke();
            crate::app::session::open_without_pin(cube);
            refresh(&mut settings);
            assert_eq!(settings.paired.len(), 1);
            let socket = &settings.paired[0].server.socket;
            assert_eq!(settings.paired[0].server.grant.credential(), credential);
            assert!(wire::ipc::call(
                socket,
                &credential,
                wire::Request::Proposal(wire::proposal::ProposalInput {
                    proposal_id: first.proposal_id.clone()
                })
            )
            .await
            .is_err());
            let wire::Response::Proposal(second) =
                wire::ipc::call(socket, &credential, wire::Request::PrepareCreation(input))
                    .await
                    .unwrap()
            else {
                panic!("fresh validated proposal")
            };
            assert_ne!(first.proposal_id, second.proposal_id);
            drop(settings);
            let stored =
                pairings::Store::open(location, zeroize::Zeroizing::new([46; 32]), false).unwrap();
            assert!(matches!(
                stored.clients()[0].scope,
                pairings::Scope::WalletCreation
            ));
            drop(stored);
            crate::app::session::close();
        });
    }
    #[test]
    fn pairing_management_and_failed_add_preserve_other_clients_pending_proposals() {
        let _guard = crate::app::session::test_guard();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let cube = "paired-proposal-fixture";
            let root = std::env::temp_dir()
                .join(format!("tenshu-paired-proposals-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(root.join("regtest")).unwrap();
            let location =
                pairings::Location::new(&root, crate::chain::ChainId::Regtest, cube).unwrap();
            let mut settings = Settings {
                enabled: true,
                proposals_allowed: true,
                pairing_location: Some(location.clone()),
                pairing_store: Some(
                    pairings::Store::open(location, zeroize::Zeroizing::new([31; 32]), true)
                        .unwrap(),
                ),
                ..Settings::default()
            };
            crate::app::session::open_without_pin(cube);
            let refresh = |settings: &mut Settings| {
                settings.ensure_pairings(PairingContext {
                    root: &root,
                    chain: crate::chain::ChainId::Regtest,
                    cube_id: cube,
                    wallet: None,
                    daemon: None,
                    supported: false,
                    proposal_supported: true,
                    last_poll_at_startup: None,
                })
            };
            let pair = |settings: &mut Settings, name: &str| {
                settings.update(Message::ClientName(name.into()), None, None, true, cube);
                settings.update(Message::Proposals(true), None, None, true, cube);
                settings.update(Message::Pair, None, None, true, cube);
            };
            pair(&mut settings, "A");
            refresh(&mut settings);
            let a = settings.paired[0].id.clone();
            let socket = settings.paired[0].server.socket.clone();
            let token = settings.paired[0].server.grant.credential().to_owned();
            let input = wire::proposal::PrepareInput {
                network: wire::Chain::Regtest,
                request_id: "same-request".into(),
                topology: wire::proposal::Topology {
                    primary: wire::proposal::Quorum {
                        threshold: 3,
                        key_count: 5,
                    },
                    recovery: wire::proposal::Recovery {
                        quorum: wire::proposal::Quorum {
                            threshold: 2,
                            key_count: 3,
                        },
                        relative_delay_blocks: 144,
                    },
                },
            };
            let wire::Response::Proposal(original) = wire::ipc::call(
                &socket,
                &token,
                wire::Request::PrepareProposal(input.clone()),
            )
            .await
            .unwrap() else {
                panic!("proposal")
            };
            // Exercise the same result handler as a failed native Pair action.
            // Storage fault tests prove this result means the old document is exact.
            let owner = settings.pairing_store.as_ref().unwrap().owner();
            settings.finish_pairing(Some(Err(pairings::AddError::Unchanged)));
            assert!(!settings.pairing_poisoned);
            assert!(settings.pairing_error.is_some());
            assert!(settings.paired[0].server.grant.active());
            assert!(Arc::ptr_eq(
                &owner,
                &settings.pairing_store.as_ref().unwrap().owner()
            ));
            refresh(&mut settings);
            for request in [
                wire::Request::Proposal(wire::proposal::ProposalInput {
                    proposal_id: original.proposal_id.clone(),
                }),
                wire::Request::PrepareProposal(input.clone()),
            ] {
                let wire::Response::Proposal(after_failure) =
                    wire::ipc::call(&socket, &token, request).await.unwrap()
                else {
                    panic!("existing proposal remains available after failed add")
                };
                assert_eq!(after_failure, original);
            }
            drop(owner);
            pair(&mut settings, "B");
            refresh(&mut settings);
            let b = settings
                .paired
                .iter()
                .find(|v| v.id != a)
                .unwrap()
                .id
                .clone();
            settings.update(
                Message::RequestRevokePair(b.clone()),
                None,
                None,
                true,
                cube,
            );
            settings.update(Message::RevokePair(b), None, None, true, cube);
            refresh(&mut settings);
            let wire::Response::Proposal(retrieved) = wire::ipc::call(
                &socket,
                &token,
                wire::Request::Proposal(wire::proposal::ProposalInput {
                    proposal_id: original.proposal_id.clone(),
                }),
            )
            .await
            .unwrap() else {
                panic!("proposal")
            };
            assert_eq!(retrieved, original);
            let wire::Response::Proposal(retried) = wire::ipc::call(
                &socket,
                &token,
                wire::Request::PrepareProposal(input.clone()),
            )
            .await
            .unwrap() else {
                panic!("proposal")
            };
            assert_eq!(retried, original);
            settings.paired[0].server.grant.revoke();
            settings.paired[0].proposals.as_ref().unwrap().revoke();
            refresh(&mut settings);
            assert!(
                !settings.paired[0].server.grant.active(),
                "consumed authorization cannot be recreated in the same session"
            );
            assert!(
                wire::ipc::call(&socket, &token, wire::Request::PrepareProposal(input))
                    .await
                    .is_err()
            );
            drop(settings);
            crate::app::session::close();
            std::fs::remove_dir_all(root).unwrap();
        });
    }
    #[test]
    fn explicit_empty_cube_proposal_consent_cannot_elevate_wallet_reads() {
        let _guard = crate::app::session::test_guard();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            crate::app::session::open_without_pin("empty-proposal-fixture");
            let mut settings = Settings::default();
            let root = crate::installer::proposal::tests::TestDir::new();
            settings.pairing_location = Some(
                pairings::Location::new(
                    root.path(),
                    crate::chain::ChainId::Regtest,
                    "empty-proposal-fixture",
                )
                .unwrap(),
            );
            for message in [
                Message::Enable(true),
                Message::ClientName("fixture".into()),
                Message::Proposals(true),
                Message::Approve,
            ] {
                settings.update(message, None, None, true, "empty-proposal-fixture");
            }
            let server = settings.server.as_ref().unwrap();
            let socket = server.socket.clone();
            let credential = server.grant.credential().to_owned();
            assert!(
                wire::ipc::call(&socket, &credential, wire::Request::List(wire::Empty {}))
                    .await
                    .is_err()
            );
            let proposal = wire::ipc::call(
                &socket,
                &credential,
                wire::Request::PrepareProposal(wire::proposal::PrepareInput {
                    network: wire::Chain::Regtest,
                    request_id: "fixture".into(),
                    topology: wire::proposal::Topology {
                        primary: wire::proposal::Quorum {
                            threshold: 3,
                            key_count: 5,
                        },
                        recovery: wire::proposal::Recovery {
                            quorum: wire::proposal::Quorum {
                                threshold: 1,
                                key_count: 1,
                            },
                            relative_delay_blocks: 144,
                        },
                    },
                }),
            )
            .await
            .unwrap();
            assert!(matches!(proposal, wire::Response::Proposal(_)));
            assert!(settings.proposal_session("different-cube").is_none());
            assert!(settings
                .proposal_session("empty-proposal-fixture")
                .is_some());
            settings.update(
                Message::Proposals(false),
                None,
                None,
                true,
                "empty-proposal-fixture",
            );
            assert!(settings.server.is_none());
            assert!(settings.proposals.is_none());
            assert!(
                wire::ipc::call(&socket, &credential, wire::Request::List(wire::Empty {}))
                    .await
                    .is_err()
            );
            crate::app::session::close();
        });
    }
}
