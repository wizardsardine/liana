use crate::installer::{
    decrypt::{Decrypt, DecryptModal},
    step::import_descriptor::ImportDescriptorModal,
};
use std::str::FromStr;

use iced::Task;

use coincube_core::{descriptors::CoincubeDescriptor, miniscript::bitcoin::Network};
use coincube_ui::{component::form, widget::Element};

use crate::{
    app::state::vault::export::VaultExportModal,
    daemon::DaemonError,
    dir::NetworkDirectory,
    export::{ImportExportMessage, ImportExportType, Progress},
    hw::HardwareWallets,
    installer::{
        context::{self, Context, RemoteBackend},
        message::{self, Message},
        step::Step,
        view, Error,
    },
    services::connect::client::{
        self,
        auth::{AuthClient, AuthError},
        backend::{api, BackendClient},
        cache,
    },
};

use super::import_descriptor::BACKUP_NETWORK_NOT_MATCH;

pub struct ChooseBackend {
    network: Network,
    remote_backend_is_selected: bool,
}

impl ChooseBackend {
    pub fn new(network: Network) -> Self {
        Self {
            network,
            remote_backend_is_selected: false,
        }
    }
}

impl From<ChooseBackend> for Box<dyn Step> {
    fn from(s: ChooseBackend) -> Box<dyn Step> {
        Box::new(s)
    }
}
impl Step for ChooseBackend {
    fn skip(&self, _ctx: &Context) -> bool {
        self.network != Network::Bitcoin && self.network != Network::Signet
    }
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        if let Message::SelectBackend(message::SelectBackend::ContinueWithLocalWallet(
            local_wallet,
        )) = message
        {
            self.remote_backend_is_selected = !local_wallet;
            Task::perform(async move {}, |_| Message::Next)
        } else {
            Task::none()
        }
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        if !self.remote_backend_is_selected {
            ctx.remote_backend = RemoteBackend::None;
        }
        true
    }

    /// If user clicks on previous to get back to the select backend, we revert the applied remote
    /// backend on the context.
    fn revert(&self, ctx: &mut Context) {
        ctx.remote_backend = RemoteBackend::Undefined;
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        _email: Option<&'a str>,
    ) -> Element<'a, Message> {
        view::choose_backend(progress)
    }
}

pub enum ConnectionStep {
    EnterEmail {
        email: form::Value<String>,
    },
    EnterOtp {
        client: AuthClient,
        backend_api_url: String,
        email: String,
        otp: form::Value<String>,
    },
    Connected {
        email: String,
        remote_backend: context::RemoteBackend,
    },
}

pub struct RemoteBackendLogin {
    network: Network,
    network_dir: NetworkDirectory,
    connect_accounts: Vec<String>,
    processing: bool,
    step: ConnectionStep,
    connection_error: Option<Error>,
    auth_error: Option<&'static str>,
}

impl RemoteBackendLogin {
    pub fn new(network: Network, network_dir: NetworkDirectory) -> Self {
        Self {
            network,
            network_dir,
            connect_accounts: Vec::new(),
            step: ConnectionStep::EnterEmail {
                email: form::Value::default(),
            },
            connection_error: None,
            auth_error: None,
            processing: false,
        }
    }
}

impl From<RemoteBackendLogin> for Box<dyn Step> {
    fn from(s: RemoteBackendLogin) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl Step for RemoteBackendLogin {
    fn skip(&self, ctx: &Context) -> bool {
        matches!(ctx.remote_backend, RemoteBackend::None)
            || (self.network != Network::Bitcoin && self.network != Network::Signet)
    }
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        match &mut self.step {
            ConnectionStep::EnterEmail { email } => match message {
                Message::SelectBackend(message::SelectBackend::EmailEdited(value)) => {
                    email.valid = value.is_empty()
                        || email_address::EmailAddress::parse_with_options(
                            &value,
                            email_address::Options::default().with_required_tld(),
                        )
                        .is_ok();
                    email.value = value;
                }
                Message::SelectBackend(message::SelectBackend::ExistingConnectAccounts(
                    accounts,
                )) => {
                    self.connect_accounts = accounts;
                }
                Message::SelectBackend(message::SelectBackend::SelectConnectAccount(email)) => {
                    return Task::perform(
                        connect_with_existing_account(
                            email,
                            self.network,
                            self.network_dir.clone(),
                        ),
                        |msg| Message::SelectBackend(message::SelectBackend::Connected(msg)),
                    )
                }
                Message::SelectBackend(message::SelectBackend::RequestOTP) => {
                    if email.value.is_empty() {
                        email.valid = false;
                    } else if email.valid {
                        let email = email.value.clone();
                        let network = self.network;
                        self.processing = true;
                        self.connection_error = None;
                        self.auth_error = None;
                        return Task::perform(
                            async move {
                                let config =
                                    client::get_service_config(network).await.map_err(|e| {
                                        if e.status() == Some(reqwest::StatusCode::NOT_FOUND) {
                                            Error::Unexpected(
                                                "Remote servers are unresponsive".to_string(),
                                            )
                                        } else {
                                            Error::Unexpected(e.to_string())
                                        }
                                    })?;
                                let client = AuthClient::new(
                                    config.auth_api_url,
                                    config.auth_api_public_key,
                                    email,
                                );
                                client.sign_in_otp().await?;
                                Ok((client, config.backend_api_url))
                            },
                            |res| Message::SelectBackend(message::SelectBackend::OTPRequested(res)),
                        );
                    }
                }
                Message::SelectBackend(message::SelectBackend::OTPRequested(res)) => {
                    self.processing = false;
                    match res {
                        Ok((client, backend_api_url)) => {
                            self.step = ConnectionStep::EnterOtp {
                                email: email.value.to_owned(),
                                otp: form::Value::default(),
                                client,
                                backend_api_url,
                            };
                        }
                        Err(e) => {
                            self.connection_error = Some(e);
                        }
                    }
                }
                Message::SelectBackend(message::SelectBackend::Connected(res)) => {
                    self.processing = false;
                    match res {
                        Ok(remote_backend) => {
                            self.step = ConnectionStep::Connected {
                                email: remote_backend
                                    .user_email()
                                    .expect("Gui connected to Coincube backend")
                                    .to_string(),
                                remote_backend,
                            };
                            return Task::perform(async move {}, |_| Message::Next);
                        }
                        Err(e) => {
                            if let Error::Auth(AuthError { http_status, .. }) = e {
                                if http_status == Some(403) {
                                    self.auth_error = Some("Token has expired or is invalid")
                                } else {
                                    self.connection_error = Some(e);
                                }
                            } else {
                                self.connection_error = Some(e);
                            }
                        }
                    }
                }
                _ => {}
            },
            ConnectionStep::EnterOtp {
                client,
                email,
                otp,
                backend_api_url,
            } => match message {
                Message::SelectBackend(message::SelectBackend::EditEmail) => {
                    self.step = ConnectionStep::EnterEmail {
                        email: form::Value {
                            value: email.clone(),
                            warning: None,
                            valid: true,
                        },
                    };
                }
                Message::SelectBackend(message::SelectBackend::RequestOTP) => {
                    *otp = form::Value::default();
                    let client = client.clone();
                    self.processing = true;
                    self.connection_error = None;
                    self.auth_error = None;
                    return Task::perform(
                        async move {
                            client.resend_otp().await?;
                            Ok(())
                        },
                        message::SelectBackend::OTPResent,
                    )
                    .map(Message::SelectBackend);
                }
                Message::SelectBackend(message::SelectBackend::OTPResent(res)) => {
                    self.processing = false;
                    if let Err(e) = res {
                        self.connection_error = Some(e);
                    }
                }
                Message::SelectBackend(message::SelectBackend::OTPEdited(value)) => {
                    otp.value = value.trim().to_string();
                    if otp.value.len() == 6 {
                        let client = client.clone();
                        let otp = otp.value.clone();
                        let backend_api_url = backend_api_url.clone();
                        self.processing = true;
                        self.connection_error = None;
                        self.auth_error = None;
                        let network = self.network;
                        return Task::perform(
                            async move { connect(client, otp, backend_api_url, network).await },
                            message::SelectBackend::Connected,
                        )
                        .map(Message::SelectBackend);
                    }
                }
                Message::SelectBackend(message::SelectBackend::Connected(res)) => {
                    self.processing = false;
                    match res {
                        Ok(remote_backend) => {
                            self.step = ConnectionStep::Connected {
                                email: email.clone(),
                                remote_backend,
                            };
                            return Task::perform(async move {}, |_| Message::Next);
                        }
                        Err(e) => {
                            if let Error::Auth(AuthError { http_status, .. }) = e {
                                if http_status == Some(403) {
                                    self.auth_error = Some("Token has expired or is invalid")
                                } else {
                                    self.connection_error = Some(e);
                                }
                            } else {
                                self.connection_error = Some(e);
                            }
                        }
                    }
                }
                _ => {}
            },
            ConnectionStep::Connected { .. } => {
                if let Message::SelectBackend(message::SelectBackend::EditEmail) = message {
                    self.step = ConnectionStep::EnterEmail {
                        email: form::Value::default(),
                    }
                }
            }
        }

        Task::none()
    }

    fn load(&self) -> Task<Message> {
        if let Ok(cache) = cache::ConnectCache::from_file(&self.network_dir) {
            Task::perform(
                async move {
                    cache
                        .accounts
                        .into_iter()
                        .map(|a| a.email)
                        .collect::<Vec<String>>()
                },
                |accounts| {
                    Message::SelectBackend(message::SelectBackend::ExistingConnectAccounts(
                        accounts,
                    ))
                },
            )
        } else {
            Task::none()
        }
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        if let ConnectionStep::Connected { remote_backend, .. } = &self.step {
            ctx.remote_backend = remote_backend.clone();
        } else {
            ctx.remote_backend = RemoteBackend::None;
        }

        true
    }

    /// If user clicks on previous to get back to the select backend, we revert the applied remote
    /// backend on the context.
    fn revert(&self, ctx: &mut Context) {
        ctx.remote_backend = RemoteBackend::Undefined;
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        _email: Option<&'a str>,
    ) -> Element<'a, Message> {
        view::login(
            progress,
            match &self.step {
                ConnectionStep::EnterEmail { email } => view::connection_step_enter_email(
                    email,
                    self.processing,
                    self.connection_error.as_ref(),
                    &self.connect_accounts,
                    self.auth_error,
                ),
                ConnectionStep::EnterOtp { email, otp, .. } => view::connection_step_enter_otp(
                    email,
                    otp,
                    self.processing,
                    self.connection_error.as_ref(),
                    self.auth_error,
                ),
                ConnectionStep::Connected { email, .. } => view::connection_step_connected(
                    email,
                    self.processing,
                    self.connection_error.as_ref(),
                    self.auth_error,
                ),
            },
        )
    }
}

pub async fn connect_with_existing_account(
    email: String,
    network: Network,
    network_dir: NetworkDirectory,
) -> Result<context::RemoteBackend, Error> {
    let config = client::get_service_config(network).await.map_err(|e| {
        if e.status() == Some(reqwest::StatusCode::NOT_FOUND) {
            Error::Unexpected("Remote servers are unresponsive".to_string())
        } else {
            Error::Unexpected(e.to_string())
        }
    })?;

    let client = AuthClient::new(config.auth_api_url, config.auth_api_public_key, email);

    let mut tokens = cache::Account::from_cache(&network_dir, &client.email)
        .map_err(|_| Error::Unexpected("Account must be in cache".to_string()))?
        .ok_or(Error::Unexpected("Account must be in cache".to_string()))?
        .tokens;

    let refresh = tokens.expires_at < chrono::Utc::now().timestamp();
    tokens = cache::update_connect_cache(&network_dir, &tokens, &client, refresh, true)
        .await
        .map_err(|e| Error::Unexpected(format!("Failed to update cache: {}", e)))?;

    let client = BackendClient::connect(client, config.backend_api_url, tokens, network).await?;
    Ok(RemoteBackend::WithoutWallet(client))
}

pub async fn connect(
    auth: AuthClient,
    token: String,
    backend_api_url: String,
    network: Network,
) -> Result<context::RemoteBackend, Error> {
    let access = auth.verify_otp(token.trim_end()).await?;
    let client = BackendClient::connect(auth, backend_api_url, access.clone(), network).await?;
    Ok(RemoteBackend::WithoutWallet(client))
}

pub struct ImportRemoteWallet {
    network: Network,
    invitation_token: form::Value<String>,
    invitation: Option<api::WalletInvitation>,
    imported_descriptor: form::Value<String>,
    descriptor: Option<CoincubeDescriptor>,
    error: Option<String>,
    backend: context::RemoteBackend,
    wallets: Vec<api::Wallet>,
    modal: ImportDescriptorModal,
    // wallet alias is stored here to be applied to context
    // and be modified in a following step
    wallet_alias: Option<String>,
}

impl ImportRemoteWallet {
    pub fn new(network: Network) -> Self {
        Self {
            network,
            invitation_token: form::Value::default(),
            invitation: None,
            imported_descriptor: form::Value::default(),
            descriptor: None,
            error: None,
            backend: context::RemoteBackend::Undefined,
            wallets: Vec::new(),
            modal: ImportDescriptorModal::None,
            wallet_alias: None,
        }
    }
}

impl Step for ImportRemoteWallet {
    fn skip(&self, ctx: &Context) -> bool {
        matches!(
            ctx.remote_backend,
            RemoteBackend::Undefined | RemoteBackend::None
        )
    }
    fn load_context(&mut self, ctx: &Context) {
        self.backend = ctx.remote_backend.clone();
    }
    fn load(&self) -> Task<Message> {
        let backend = self.backend.clone();
        Task::perform(
            async move {
                let wallets = match backend {
                    context::RemoteBackend::WithoutWallet(backend) => {
                        backend.list_wallets().await?
                    }
                    context::RemoteBackend::WithWallet(backend) => {
                        backend.inner_client().list_wallets().await?
                    }
                    _ => unreachable!("Step must be skipped otherwise"),
                };

                Ok(wallets)
            },
            |res| Message::ImportRemoteWallet(message::ImportRemoteWallet::RemoteWallets(res)),
        )
    }
    // form value is set as valid each time it is edited.
    // Verification of the values is happening when the user click on Next button.
    fn update(&mut self, hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        match message {
            Message::ImportRemoteWallet(message::ImportRemoteWallet::ImportDescriptorFromFile) => {
                let modal = VaultExportModal::new(None, ImportExportType::FromBackup);
                let launch = modal.launch(false);
                self.modal = ImportDescriptorModal::Export(modal);
                return launch;
            }
            Message::ImportExport(ImportExportMessage::Path(p)) => {
                if self.modal.is_some() {
                    return self
                        .modal
                        .update(Message::ImportExport(ImportExportMessage::Path(p)));
                }
            }
            Message::ImportExport(ImportExportMessage::Close) => {
                self.modal = ImportDescriptorModal::None
            }
            Message::ImportExport(ImportExportMessage::Progress(Progress::EncryptedFile(
                bytes,
            ))) => {
                self.modal = ImportDescriptorModal::Decrypt(DecryptModal::new(bytes, self.network));
            }
            Message::ImportExport(m) => return self.modal.update(Message::ImportExport(m)),
            Message::HardwareWalletUpdate => {
                if let ImportDescriptorModal::Decrypt(modal) = &mut self.modal {
                    return modal.update_devices(hws).unwrap_or(Task::none());
                }
            }
            Message::Decrypt(Decrypt::Close) => {
                if matches!(self.modal, ImportDescriptorModal::Decrypt(_)) {
                    self.modal = ImportDescriptorModal::None;
                }
            }
            Message::Decrypt(Decrypt::Backup(mut backup)) => {
                let descriptor = backup.accounts.first().map(|acc| acc.descriptor.clone());
                if let Some(desc) = descriptor {
                    let network_matches = if self.network == Network::Bitcoin {
                        backup.network == Network::Bitcoin
                    } else {
                        backup.network != Network::Bitcoin
                    };
                    if network_matches {
                        // NOTE: we need to overwrite w/ correct network for testnets
                        // as non Mainnet keys / descriptor are parsed as Signet
                        backup.network = self.network;

                        // NOTE: a check that the descriptor is a valid CoincubeDescriptor have
                        // already been processed at backup import.
                        self.descriptor = CoincubeDescriptor::from_str(&desc).ok();
                        self.imported_descriptor.value = desc;
                        self.modal = ImportDescriptorModal::None;
                        return Task::perform(async {}, |_| Message::Next);
                    } else {
                        self.modal = ImportDescriptorModal::None;
                        self.error = Some(BACKUP_NETWORK_NOT_MATCH.into());
                    }
                } else {
                    self.modal = ImportDescriptorModal::None;
                    self.error = Some("Backup imported but descriptor missing!".into());
                }
            }
            Message::Decrypt(m) => {
                if let ImportDescriptorModal::Decrypt(modal) = &mut self.modal {
                    return modal.update(m);
                }
            }
            Message::ImportRemoteWallet(message::ImportRemoteWallet::ImportDescriptor(desc)) => {
                self.imported_descriptor.value = desc;
                if !self.imported_descriptor.value.is_empty() {
                    if let Ok(desc) = CoincubeDescriptor::from_str(&self.imported_descriptor.value)
                    {
                        if self.network == Network::Bitcoin {
                            self.imported_descriptor.valid = desc.all_xpubs_net_is(self.network);
                        } else {
                            self.imported_descriptor.valid =
                                desc.all_xpubs_net_is(Network::Testnet);
                        }
                    } else {
                        self.imported_descriptor.valid = false;
                    }
                } else {
                    self.imported_descriptor.valid = false;
                }
            }
            Message::ImportRemoteWallet(message::ImportRemoteWallet::ConfirmDescriptor) => {
                if let Ok(desc) = CoincubeDescriptor::from_str(&self.imported_descriptor.value) {
                    if self.network == Network::Bitcoin {
                        self.imported_descriptor.valid = desc.all_xpubs_net_is(self.network);
                    } else {
                        self.imported_descriptor.valid = desc.all_xpubs_net_is(Network::Testnet);
                    }
                    if self.imported_descriptor.valid {
                        if let context::RemoteBackend::WithWallet(backend) = self.backend.clone() {
                            self.backend =
                                context::RemoteBackend::WithoutWallet(backend.into_inner());
                        }
                        self.descriptor = Some(desc);
                        return Task::perform(async {}, |_| Message::Next);
                    }
                } else {
                    self.imported_descriptor.valid = false;
                }
            }
            Message::ImportRemoteWallet(message::ImportRemoteWallet::RemoteWallets(res)) => {
                match res {
                    Ok(wallets) => self.wallets = wallets,
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            Message::ImportRemoteWallet(message::ImportRemoteWallet::ImportInvitationToken(
                token,
            )) => {
                self.invitation_token.value = token;
            }
            Message::ImportRemoteWallet(message::ImportRemoteWallet::FetchInvitation) => {
                let backend = match self.backend.clone() {
                    context::RemoteBackend::WithoutWallet(b) => b,
                    context::RemoteBackend::WithWallet(b) => b.into_inner(),
                    _ => unreachable!("Must be a remote backend at this point"),
                };
                let token = self.invitation_token.value.clone();
                self.error = None;
                return Task::perform(
                    async move {
                        let invitation = backend.get_wallet_invitation(&token).await?;
                        Ok(invitation)
                    },
                    |res| {
                        Message::ImportRemoteWallet(message::ImportRemoteWallet::InvitationFetched(
                            res,
                        ))
                    },
                );
            }
            Message::ImportRemoteWallet(message::ImportRemoteWallet::InvitationFetched(res)) => {
                match res {
                    Err(_) => self.invitation_token.valid = false,
                    Ok(invitation) => self.invitation = Some(invitation),
                }
            }
            Message::ImportRemoteWallet(message::ImportRemoteWallet::AcceptInvitation) => {
                let backend = match self.backend.clone() {
                    context::RemoteBackend::WithoutWallet(b) => b,
                    context::RemoteBackend::WithWallet(b) => b.into_inner(),
                    _ => unreachable!("Must be a remote backend defined"),
                };
                let invitation = self.invitation.clone().expect("Invitation was fetched");
                self.error = None;
                return Task::perform(
                    async move {
                        backend.accept_wallet_invitation(&invitation.id).await?;
                        let wallets = backend.list_wallets().await?;
                        wallets
                            .into_iter()
                            .find(|w| w.id == invitation.wallet_id)
                            .ok_or(
                                DaemonError::Unexpected(
                                    "Wallet of accepted invitation not found".to_string(),
                                )
                                .into(),
                            )
                    },
                    |res| {
                        Message::ImportRemoteWallet(
                            message::ImportRemoteWallet::InvitationAccepted(res),
                        )
                    },
                );
            }
            Message::ImportRemoteWallet(message::ImportRemoteWallet::InvitationAccepted(res)) => {
                match res {
                    Err(e) => self.error = Some(e.to_string()),
                    Ok(wallet) => {
                        self.invitation = None;
                        self.invitation_token = form::Value::default();
                        self.wallets.push(wallet);
                    }
                }
            }
            Message::Select(i) => {
                if let Some(wallet) = self.wallets.get(i).cloned() {
                    self.wallet_alias = wallet.metadata.wallet_alias.clone();
                    self.backend = match self.backend.clone() {
                        context::RemoteBackend::WithoutWallet(backend) => {
                            context::RemoteBackend::WithWallet(
                                backend.connect_wallet(wallet.clone()).0,
                            )
                        }
                        context::RemoteBackend::WithWallet(backend) => {
                            context::RemoteBackend::WithWallet(
                                backend.into_inner().connect_wallet(wallet.clone()).0,
                            )
                        }
                        context::RemoteBackend::None => context::RemoteBackend::None,
                        context::RemoteBackend::Undefined => context::RemoteBackend::Undefined,
                    };
                    // ensure that no descriptor is imported.
                    self.imported_descriptor = form::Value::default();
                    self.descriptor = Some(wallet.descriptor);
                    return Task::perform(async {}, |_| Message::Next);
                }
            }
            _ => {}
        }

        Task::none()
    }

    fn subscription(&self, hws: &HardwareWallets) -> iced::Subscription<Message> {
        self.modal.subscriptions(hws)
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        // Set to true in order to force the registration process to be shown to user.
        ctx.hw_is_used = true;
        ctx.descriptor.clone_from(&self.descriptor);
        ctx.remote_backend.clone_from(&self.backend);

        if let Some(alias) = &self.wallet_alias {
            ctx.wallet_alias = alias.clone();
        }

        true
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        email: Option<&'a str>,
    ) -> Element<'a, Message> {
        let content = view::import_wallet_or_descriptor(
            progress,
            email,
            &self.invitation_token,
            self.invitation
                .as_ref()
                .map(|invit| invit.wallet_name.as_str()),
            &self.imported_descriptor,
            self.error.as_ref(),
            self.wallets
                .iter()
                .map(|w| (&w.name, w.metadata.wallet_alias.as_ref()))
                .collect(),
        );
        self.modal.view(content)
    }
}

impl From<ImportRemoteWallet> for Box<dyn Step> {
    fn from(s: ImportRemoteWallet) -> Box<dyn Step> {
        Box::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir::CoincubeDirectory;
    use crate::installer::step::Step;
    use std::path::PathBuf;

    fn directory() -> CoincubeDirectory {
        CoincubeDirectory::new(PathBuf::new())
    }

    fn network_dir(network: Network) -> NetworkDirectory {
        directory().network_directory(network)
    }

    fn context_with(remote_backend: RemoteBackend, network: Network) -> Context {
        Context::new(network, directory(), remote_backend, None, None)
    }

    fn hardware_wallets(network: Network) -> HardwareWallets {
        HardwareWallets::new(directory(), network)
    }

    fn auth_client(email: &str) -> AuthClient {
        AuthClient::new(
            "https://auth.example.test".to_string(),
            "public-key".to_string(),
            email.to_string(),
        )
    }

    #[test]
    fn choose_backend_skips_only_on_unsupported_networks() {
        let bitcoin = ChooseBackend::new(Network::Bitcoin);
        let signet = ChooseBackend::new(Network::Signet);
        let regtest = ChooseBackend::new(Network::Regtest);
        let ctx = context_with(RemoteBackend::Undefined, Network::Bitcoin);

        assert!(!bitcoin.skip(&ctx));
        assert!(!signet.skip(&ctx));
        assert!(regtest.skip(&ctx));
    }

    #[test]
    fn choose_backend_local_selection_applies_none_and_revert_restores_undefined() {
        let mut step = ChooseBackend::new(Network::Bitcoin);
        let mut ctx = context_with(RemoteBackend::Undefined, Network::Bitcoin);

        let _task = step.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::SelectBackend(message::SelectBackend::ContinueWithLocalWallet(true)),
        );
        assert!(step.apply(&mut ctx));
        assert!(matches!(ctx.remote_backend, RemoteBackend::None));

        step.revert(&mut ctx);
        assert!(matches!(ctx.remote_backend, RemoteBackend::Undefined));
    }

    #[test]
    fn choose_backend_remote_selection_leaves_backend_for_login_step() {
        let mut step = ChooseBackend::new(Network::Bitcoin);
        let mut ctx = context_with(RemoteBackend::Undefined, Network::Bitcoin);

        let _task = step.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::SelectBackend(message::SelectBackend::ContinueWithLocalWallet(false)),
        );

        assert!(step.apply(&mut ctx));
        assert!(matches!(ctx.remote_backend, RemoteBackend::Undefined));
    }

    #[test]
    fn remote_backend_login_initial_state_and_skip_rules_are_stable() {
        let login = RemoteBackendLogin::new(Network::Bitcoin, network_dir(Network::Bitcoin));
        let regtest_login =
            RemoteBackendLogin::new(Network::Regtest, network_dir(Network::Regtest));
        let undefined = context_with(RemoteBackend::Undefined, Network::Bitcoin);
        let none = context_with(RemoteBackend::None, Network::Bitcoin);
        let regtest = context_with(RemoteBackend::Undefined, Network::Regtest);

        assert!(!login.processing);
        assert!(login.connect_accounts.is_empty());
        assert!(matches!(
            login.step,
            ConnectionStep::EnterEmail { ref email }
                if email.value.is_empty() && email.valid
        ));
        assert!(!login.skip(&undefined));
        assert!(login.skip(&none));
        assert!(regtest_login.skip(&regtest));
    }

    #[test]
    fn remote_backend_login_validates_email_and_rejects_empty_otp_requests() {
        let mut login = RemoteBackendLogin::new(Network::Bitcoin, network_dir(Network::Bitcoin));

        let _task = login.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::SelectBackend(message::SelectBackend::EmailEdited(
                "not-an-email".to_string(),
            )),
        );
        assert!(matches!(
            login.step,
            ConnectionStep::EnterEmail { ref email }
                if email.value == "not-an-email" && !email.valid
        ));

        let _task = login.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::SelectBackend(message::SelectBackend::EmailEdited(
                "user@example.com".to_string(),
            )),
        );
        assert!(matches!(
            login.step,
            ConnectionStep::EnterEmail { ref email }
                if email.value == "user@example.com" && email.valid
        ));

        let mut empty = RemoteBackendLogin::new(Network::Bitcoin, network_dir(Network::Bitcoin));
        let _task = empty.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::SelectBackend(message::SelectBackend::RequestOTP),
        );
        assert!(matches!(
            empty.step,
            ConnectionStep::EnterEmail { ref email }
                if email.value.is_empty() && !email.valid
        ));
        assert!(!empty.processing);
    }

    #[test]
    fn remote_backend_login_tracks_cached_accounts_and_otp_result() {
        let mut login = RemoteBackendLogin::new(Network::Bitcoin, network_dir(Network::Bitcoin));

        let _task = login.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::SelectBackend(message::SelectBackend::ExistingConnectAccounts(vec![
                "a@example.com".to_string(),
                "b@example.com".to_string(),
            ])),
        );
        assert_eq!(
            login.connect_accounts,
            vec!["a@example.com".to_string(), "b@example.com".to_string()]
        );

        let _task = login.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::SelectBackend(message::SelectBackend::EmailEdited(
                "user@example.com".to_string(),
            )),
        );
        let _task = login.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::SelectBackend(message::SelectBackend::OTPRequested(Ok((
                auth_client("user@example.com"),
                "https://backend.example.test".to_string(),
            )))),
        );

        assert!(!login.processing);
        assert!(matches!(
            login.step,
            ConnectionStep::EnterOtp {
                ref email,
                ref backend_api_url,
                ref otp,
                ..
            } if email == "user@example.com"
                && backend_api_url == "https://backend.example.test"
                && otp.value.is_empty()
        ));
    }

    #[test]
    fn remote_backend_login_enter_otp_handles_edits_resend_errors_and_auth_failures() {
        let mut login = RemoteBackendLogin::new(Network::Bitcoin, network_dir(Network::Bitcoin));
        login.step = ConnectionStep::EnterOtp {
            client: auth_client("user@example.com"),
            backend_api_url: "https://backend.example.test".to_string(),
            email: "user@example.com".to_string(),
            otp: form::Value::default(),
        };

        let _task = login.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::SelectBackend(message::SelectBackend::OTPEdited(" 123 ".to_string())),
        );
        assert!(matches!(
            login.step,
            ConnectionStep::EnterOtp { ref otp, .. } if otp.value == "123"
        ));
        assert!(!login.processing);

        let _task = login.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::SelectBackend(message::SelectBackend::OTPResent(Err(Error::Unexpected(
                "mail failed".to_string(),
            )))),
        );
        assert!(!login.processing);
        assert!(matches!(
            login.connection_error,
            Some(Error::Unexpected(ref msg)) if msg == "mail failed"
        ));

        let _task = login.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::SelectBackend(message::SelectBackend::Connected(Err(Error::Auth(
                AuthError {
                    http_status: Some(403),
                    error: "forbidden".to_string(),
                },
            )))),
        );
        assert_eq!(login.auth_error, Some("Token has expired or is invalid"));

        let _task = login.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::SelectBackend(message::SelectBackend::EditEmail),
        );
        assert!(matches!(
            login.step,
            ConnectionStep::EnterEmail { ref email }
                if email.value == "user@example.com" && email.valid
        ));
    }

    #[test]
    fn remote_backend_login_apply_without_connection_clears_remote_backend() {
        let mut login = RemoteBackendLogin::new(Network::Bitcoin, network_dir(Network::Bitcoin));
        let mut ctx = context_with(RemoteBackend::Undefined, Network::Bitcoin);

        assert!(login.apply(&mut ctx));

        assert!(matches!(ctx.remote_backend, RemoteBackend::None));
    }

    #[test]
    fn import_remote_wallet_initial_state_and_skip_rules_are_stable() {
        let mut step = ImportRemoteWallet::new(Network::Bitcoin);
        let undefined = context_with(RemoteBackend::Undefined, Network::Bitcoin);
        let none = context_with(RemoteBackend::None, Network::Bitcoin);

        assert!(step.skip(&undefined));
        assert!(step.skip(&none));

        step.load_context(&undefined);
        assert!(matches!(step.backend, RemoteBackend::Undefined));
        assert!(step.invitation_token.value.is_empty());
        assert!(step.imported_descriptor.value.is_empty());
        assert!(step.invitation.is_none());
        assert!(step.wallets.is_empty());
        assert!(step.descriptor.is_none());
        assert!(step.error.is_none());
    }

    #[test]
    fn import_remote_wallet_tracks_invitation_token_and_fetch_result() {
        let mut step = ImportRemoteWallet::new(Network::Bitcoin);

        let _task = step.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::ImportRemoteWallet(message::ImportRemoteWallet::ImportInvitationToken(
                "invite-123".to_string(),
            )),
        );
        assert_eq!(step.invitation_token.value, "invite-123");

        let _task = step.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::ImportRemoteWallet(message::ImportRemoteWallet::InvitationFetched(Err(
                Error::Unexpected("missing".to_string()),
            ))),
        );
        assert!(!step.invitation_token.valid);

        let invitation = api::WalletInvitation {
            id: "invitation-id".to_string(),
            wallet_name: "Family Vault".to_string(),
            wallet_id: "wallet-id".to_string(),
            status: api::WalletInvitationStatus::Pending,
        };
        let _task = step.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::ImportRemoteWallet(message::ImportRemoteWallet::InvitationFetched(Ok(
                invitation.clone(),
            ))),
        );

        assert_eq!(
            step.invitation.as_ref().map(|i| i.wallet_name.as_str()),
            Some("Family Vault")
        );
    }

    #[test]
    fn import_remote_wallet_records_remote_wallet_and_accept_errors() {
        let mut step = ImportRemoteWallet::new(Network::Bitcoin);

        let _task = step.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::ImportRemoteWallet(message::ImportRemoteWallet::RemoteWallets(Ok(Vec::new()))),
        );
        assert!(step.wallets.is_empty());

        let _task = step.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::ImportRemoteWallet(message::ImportRemoteWallet::RemoteWallets(Err(
                Error::Unexpected("list failed".to_string()),
            ))),
        );
        assert_eq!(step.error.as_deref(), Some("Unexpected: list failed"));

        let _task = step.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::ImportRemoteWallet(message::ImportRemoteWallet::InvitationAccepted(Err(
                Error::Unexpected("accept failed".to_string()),
            ))),
        );
        assert_eq!(step.error.as_deref(), Some("Unexpected: accept failed"));
    }

    #[test]
    fn import_remote_wallet_rejects_invalid_descriptor_text() {
        let mut step = ImportRemoteWallet::new(Network::Bitcoin);

        let _task = step.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::ImportRemoteWallet(message::ImportRemoteWallet::ImportDescriptor(
                "not a descriptor".to_string(),
            )),
        );

        assert_eq!(step.imported_descriptor.value, "not a descriptor");
        assert!(!step.imported_descriptor.valid);

        let _task = step.update(
            &mut hardware_wallets(Network::Bitcoin),
            Message::ImportRemoteWallet(message::ImportRemoteWallet::ConfirmDescriptor),
        );
        assert!(step.descriptor.is_none());
        assert!(!step.imported_descriptor.valid);
    }

    #[test]
    fn import_remote_wallet_apply_copies_state_to_context() {
        let mut step = ImportRemoteWallet::new(Network::Bitcoin);
        let mut ctx = context_with(RemoteBackend::Undefined, Network::Bitcoin);
        step.wallet_alias = Some("Family Vault".to_string());
        step.backend = RemoteBackend::None;

        assert!(step.apply(&mut ctx));

        assert!(ctx.hw_is_used);
        assert!(matches!(ctx.remote_backend, RemoteBackend::None));
        assert_eq!(ctx.wallet_alias, "Family Vault");
    }
}
