use std::str::FromStr;

use iced::Task;

use liana::{descriptors::LianaDescriptor, miniscript::bitcoin::Network};
use liana_ui::{component::form, widget::Element};

use crate::{
    daemon::DaemonError,
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
    },
};

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
    ) -> Element<Message> {
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
    processing: bool,
    step: ConnectionStep,
    connection_error: Option<Error>,
    auth_error: Option<&'static str>,
}

impl RemoteBackendLogin {
    pub fn new(network: Network) -> Self {
        Self {
            network,
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
    ) -> Element<Message> {
        view::login(
            progress,
            match &self.step {
                ConnectionStep::EnterEmail { email } => view::connection_step_enter_email(
                    email,
                    self.processing,
                    self.connection_error.as_ref(),
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
    descriptor: Option<LianaDescriptor>,
    error: Option<String>,
    backend: context::RemoteBackend,
    wallets: Vec<api::Wallet>,
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
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        match message {
            Message::ImportRemoteWallet(message::ImportRemoteWallet::ImportDescriptor(desc)) => {
                self.imported_descriptor.value = desc;
                if !self.imported_descriptor.value.is_empty() {
                    if let Ok(desc) = LianaDescriptor::from_str(&self.imported_descriptor.value) {
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
                if let Ok(desc) = LianaDescriptor::from_str(&self.imported_descriptor.value) {
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
    ) -> Element<Message> {
        view::import_wallet_or_descriptor(
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
        )
    }
}

impl From<ImportRemoteWallet> for Box<dyn Step> {
    fn from(s: ImportRemoteWallet) -> Box<dyn Step> {
        Box::new(s)
    }
}
