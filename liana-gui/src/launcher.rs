use iced::{
    alignment::Horizontal,
    widget::{checkbox, column, row, Space},
    Alignment, Length, Subscription, Task,
};

use liana::miniscript::bitcoin::Network;
use liana_ui::{
    component::{
        button::{btn_add_wallet, btn_delete_wallet, btn_remove, btn_select, EntryWidth},
        card, installer as installer_layout,
        list::{self, EntryAccent},
        network_banner, notification,
        text::new,
    },
    icon, image, theme,
    widget::{modal::Modal, Column, Container, Element, SpaceExt},
};
use lianad::config::ConfigError;
use tokio::runtime::Handle;

use crate::{
    app::{
        self,
        settings::{self, AuthConfig, WalletId, WalletSettings},
    },
    delete::{delete_wallet, DeleteError},
    dir::{LianaDirectory, NetworkDirectory},
    installer::UserFlow,
    services::connect::{
        client::{auth::AuthClient, backend::UserRole, get_service_config, BackendType},
        login::{connect_with_credentials, BackendState},
    },
};

const NETWORKS: [Network; 5] = [
    Network::Bitcoin,
    Network::Testnet,
    Network::Testnet4,
    Network::Signet,
    Network::Regtest,
];

#[derive(Debug, Clone)]
pub enum State {
    Unchecked,
    Wallets {
        wallets: Vec<WalletSettings>,
        add_wallet: bool,
    },
    NoWallet,
}

pub struct Launcher {
    state: State,
    displayed_networks: Vec<Network>,
    network: Network,
    pub datadir_path: LianaDirectory,
    error: Option<String>,
    delete_wallet_modal: Option<DeleteWalletModal>,
    backend_type: BackendType,
}

impl Launcher {
    pub fn new(
        datadir_path: LianaDirectory,
        network: Option<Network>,
        backend_type: BackendType,
    ) -> (Self, Task<Message>) {
        let network = network.unwrap_or(
            NETWORKS
                .iter()
                .find(|net| has_existing_wallet(&datadir_path, **net))
                .cloned()
                .unwrap_or(Network::Bitcoin),
        );
        let network_dir = datadir_path.network_directory(network);
        (
            Self {
                state: State::Unchecked,
                displayed_networks: displayed_networks(&datadir_path),
                network,
                datadir_path: datadir_path.clone(),
                error: None,
                delete_wallet_modal: None,
                backend_type,
            },
            Task::perform(check_network_datadir(network_dir), Message::Checked),
        )
    }

    pub fn reload(&self) -> Task<Message> {
        Task::perform(
            check_network_datadir(self.datadir_path.network_directory(self.network)),
            Message::Checked,
        )
    }

    pub fn stop(&mut self) {}

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::View(ViewMessage::ImportWallet) => {
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                Task::perform(async move { (datadir_path, network) }, |(d, n)| {
                    Message::Install(d, n, UserFlow::AddWallet)
                })
            }
            Message::View(ViewMessage::CreateWallet) => {
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                Task::perform(async move { (datadir_path, network) }, |(d, n)| {
                    Message::Install(d, n, UserFlow::CreateWallet)
                })
            }
            Message::View(ViewMessage::ShareXpubs) => {
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                Task::perform(async move { (datadir_path, network) }, |(d, n)| {
                    Message::Install(d, n, UserFlow::ShareXpubs)
                })
            }
            Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::ShowModal(i))) => {
                if let State::Wallets { wallets, .. } = &self.state {
                    let wallet_datadir = self.datadir_path.network_directory(self.network);
                    let config_path = wallet_datadir.path().join(app::config::DEFAULT_FILE_NAME);
                    let internal_bitcoind = if wallets[i].remote_backend_auth.is_some() {
                        Some(false)
                    } else if wallets[i].start_internal_bitcoind.is_some() {
                        wallets[i].start_internal_bitcoind
                    } else if let Ok(cfg) = app::Config::from_file(&config_path) {
                        Some(cfg.start_internal_bitcoind)
                    } else {
                        None
                    };
                    self.delete_wallet_modal = Some(DeleteWalletModal::new(
                        self.network,
                        wallet_datadir,
                        wallets[i].clone(),
                        internal_bitcoind,
                        self.backend_type,
                    ));
                }
                Task::none()
            }
            Message::View(ViewMessage::SelectNetwork(network)) => {
                self.network = network;
                let network_dir = self.datadir_path.network_directory(self.network);
                Task::perform(check_network_datadir(network_dir), Message::Checked)
            }
            Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::Deleted)) => {
                self.state = State::NoWallet;
                let network_dir = self.datadir_path.network_directory(self.network);
                Task::perform(check_network_datadir(network_dir), Message::Checked)
            }

            Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::CloseModal)) => {
                self.delete_wallet_modal = None;
                if self.network == Network::Testnet
                    && !has_existing_wallet(&self.datadir_path, Network::Testnet)
                {
                    self.network = Network::Testnet4;
                }
                Task::none()
            }
            Message::Checked(res) => match res {
                Err(e) => {
                    self.error = Some(e);
                    Task::none()
                }
                Ok(state) => {
                    self.state = state;
                    Task::none()
                }
            },
            Message::View(ViewMessage::AddWalletToList(add)) => {
                if let State::Wallets { add_wallet, .. } = &mut self.state {
                    *add_wallet = add;
                }
                Task::none()
            }
            Message::View(ViewMessage::Run(index)) => {
                if let State::Wallets { wallets, .. } = &self.state {
                    if let Some(settings) = wallets.get(index) {
                        let datadir_path = self.datadir_path.clone();
                        let mut path = self
                            .datadir_path
                            .network_directory(self.network)
                            .path()
                            .to_path_buf();
                        path.push(app::config::DEFAULT_FILE_NAME);
                        let cfg = app::Config::from_file(&path).expect("Already checked");
                        let network = self.network;
                        let settings = settings.clone();
                        Task::perform(
                            async move { (datadir_path.clone(), cfg, network, settings) },
                            |m| Message::Run(m.0, m.1, m.2, m.3),
                        )
                    } else {
                        Task::none()
                    }
                } else {
                    Task::none()
                }
            }
            _ => {
                if let Some(modal) = &mut self.delete_wallet_modal {
                    return modal.update(message);
                }
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let previous_message = match &self.state {
            State::Wallets { add_wallet, .. } if *add_wallet => {
                Some(Message::View(ViewMessage::AddWalletToList(false)))
            }
            _ => None,
        };

        let title = if matches!(self.state, State::Wallets { .. }) {
            new::d0("Welcome back")
        } else {
            new::d0("Welcome")
        };

        let error = self.error.as_ref().map(|e| card::simple(new::caption(e)));

        let wallets: Element<'_, Message> = match &self.state {
            State::Unchecked => column![].into(),
            State::Wallets {
                wallets,
                add_wallet,
            } => {
                if *add_wallet {
                    column![add_wallet_menu().map(Message::View)].into()
                } else {
                    let list = wallets.iter().enumerate().fold(
                        Column::new().spacing(20),
                        |col, (i, settings)| {
                            col.push(entry_wallet(self.network, settings, i).map(Message::View))
                        },
                    );
                    let add_wallet_msg = Message::View(ViewMessage::AddWalletToList(true));
                    let add_wallet =
                        column![btn_add_wallet(Some(add_wallet_msg))].width(EntryWidth::Standard);

                    list.push(add_wallet).into()
                }
            }
            State::NoWallet => column![add_wallet_menu().map(Message::View)].into(),
        };

        let body = column![title, error, wallets]
            .align_x(Alignment::Center)
            .spacing(30);

        let content = launcher_layout(
            previous_message,
            self.displayed_networks.as_slice(),
            self.network,
            body,
        );

        let content: Element<'_, Message> = if self.network != Network::Bitcoin {
            column![network_banner(self.network), content].into()
        } else {
            content
        };

        if let Some(modal) = &self.delete_wallet_modal {
            Modal::new(Container::new(content).height(Length::Fill), modal.view())
                .on_blur(Some(Message::View(ViewMessage::DeleteWallet(
                    DeleteWalletMessage::CloseModal,
                ))))
                .into()
        } else {
            content
        }
    }
}

fn launcher_layout<'a>(
    previous_message: Option<Message>,
    networks: &'a [Network],
    selected_network: Network,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let content = Container::new(content).center_x(Length::Fill);

    installer_layout::layout(
        installer_layout::LayoutConfig {
            variant: liana_ui::Variant::Liana,
            network: selected_network,
            email: None,
            is_ws_admin: false,
            nav_bar: installer_layout::NavBar::Launcher {
                previous_message,
                share_xpubs_message: Some(Message::View(ViewMessage::ShareXpubs)),
                networks,
                selected_network,
                on_network_selected: |network| Message::View(ViewMessage::SelectNetwork(network)),
            },
            content_width: 800.0,
        },
        content,
    )
}

fn add_wallet_menu<'a>() -> Element<'a, ViewMessage> {
    const ICON_SIZE: u32 = 100;
    let create_wallet = column![
        image::create_new_wallet_icon().width(ICON_SIZE),
        new::caption("Create a new Liana wallet").style(theme::text::secondary),
        btn_select(Some(ViewMessage::CreateWallet)),
    ]
    .spacing(20)
    .align_x(Alignment::Center);

    let add_existing_wallet = column![
        image::restore_wallet_icon().width(ICON_SIZE),
        new::caption("Add an existing Liana wallet").style(theme::text::secondary),
        btn_select(Some(ViewMessage::ImportWallet)),
    ]
    .spacing(20)
    .align_x(Alignment::Center);

    row![
        Container::new(create_wallet).padding(20),
        Container::new(add_existing_wallet).padding(20),
    ]
    .align_y(Alignment::End)
    .spacing(20)
    .into()
}

fn entry_wallet(network: Network, settings: &WalletSettings, i: usize) -> Element<'_, ViewMessage> {
    let title = settings
        .alias
        .clone()
        .unwrap_or(format!("My Liana {network:?} wallet"));

    let checksum = new::caption(format!("Liana-{}", settings.descriptor_checksum))
        .style(theme::text::secondary);
    let email = settings.remote_backend_auth.as_ref().map(|auth| {
        row![
            Space::fill_width(),
            new::caption(&auth.email).style(theme::text::secondary)
        ]
    });
    let subtitle = Some(column![checksum, email].into());

    let accent = Some(match network {
        Network::Bitcoin => EntryAccent::Bitcoin,
        _ => EntryAccent::Testnet,
    });

    let entry = list::entry_wallet(
        accent,
        title,
        subtitle,
        None,
        None,
        Some(ViewMessage::Run(i)),
    );

    let delete_button = btn_remove(Some(ViewMessage::DeleteWallet(
        DeleteWalletMessage::ShowModal(i),
    )));

    row![entry, delete_button].align_y(Alignment::Center).into()
}

/// Returns the list of displayed networks.
///
/// `Testnet` is not displayed if no wallet already exists as `Testnet4` should be available.
fn displayed_networks(dir: &LianaDirectory) -> Vec<Network> {
    let mut networks = NETWORKS.to_vec();

    networks.retain(|&n| match n {
        Network::Testnet => has_existing_wallet(dir, Network::Testnet),
        _ => true,
    });

    networks
}

fn has_existing_wallet(data_dir: &LianaDirectory, network: Network) -> bool {
    data_dir
        .path()
        .join(network.to_string())
        .join(settings::SETTINGS_FILE_NAME)
        .exists()
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Message {
    View(ViewMessage),
    Install(LianaDirectory, Network, UserFlow),
    Checked(Result<State, String>),
    Run(LianaDirectory, app::config::Config, Network, WalletSettings),
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
    ImportWallet,
    CreateWallet,
    AddWalletToList(bool),
    ShareXpubs,
    SelectNetwork(Network),
    StartInstall(Network),
    Check,
    Run(usize),
    DeleteWallet(DeleteWalletMessage),
}

#[derive(Debug, Clone)]
pub enum DeleteWalletMessage {
    ShowModal(usize),
    CloseModal,
    Confirm(WalletId),
    DeleteLianaConnect(bool),
    Deleted,
}

struct DeleteWalletModal {
    network: Network,
    network_directory: NetworkDirectory,
    wallet_settings: WalletSettings,
    warning: Option<DeleteError>,
    deleted: bool,
    delete_liana_connect: bool,
    user_role: Option<UserRole>,
    // `None` means we were not able to determine whether wallet uses internal bitcoind.
    internal_bitcoind: Option<bool>,
    backend_type: BackendType,
}

impl DeleteWalletModal {
    fn new(
        network: Network,
        network_directory: NetworkDirectory,
        wallet_settings: WalletSettings,
        internal_bitcoind: Option<bool>,
        backend_type: BackendType,
    ) -> Self {
        let mut modal = Self {
            network,
            wallet_settings,
            network_directory,
            warning: None,
            deleted: false,
            delete_liana_connect: false,
            internal_bitcoind,
            user_role: None,
            backend_type,
        };
        if let Some(auth) = &modal.wallet_settings.remote_backend_auth {
            match Handle::current().block_on(check_membership(
                modal.network,
                &modal.network_directory,
                auth,
                modal.backend_type,
            )) {
                Err(e) => {
                    modal.warning = Some(e);
                }
                Ok(user_role) => {
                    modal.user_role = user_role;
                }
            }
        }
        modal
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::Confirm(wallet_id))) => {
                if wallet_id != self.wallet_settings.wallet_id() {
                    return Task::none();
                }
                self.warning = None;
                if let Err(e) = Handle::current().block_on(delete_wallet(
                    self.network,
                    &self.network_directory,
                    &self.wallet_settings,
                    self.delete_liana_connect,
                    self.backend_type,
                )) {
                    self.warning = Some(e);
                } else {
                    self.deleted = true;
                    return Task::perform(async {}, |_| {
                        Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::Deleted))
                    });
                };
            }
            Message::View(ViewMessage::DeleteWallet(DeleteWalletMessage::DeleteLianaConnect(
                delete,
            ))) => {
                self.delete_liana_connect = delete;
            }
            _ => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let confirm_button =
            btn_delete_wallet(self.warning.is_none().then_some(ViewMessage::DeleteWallet(
                DeleteWalletMessage::Confirm(self.wallet_settings.wallet_id()),
            )));
        let help_text_1 = format!(
            "Are you sure you want to {} for the wallet {}",
            if self.wallet_settings.remote_backend_auth.is_some() {
                "delete locally the configuration"
            } else {
                "delete the configuration and all associated data"
            },
            if let Some(alias) = &self.wallet_settings.alias {
                format!(
                    "{} (Liana-{})?",
                    alias, self.wallet_settings.descriptor_checksum
                )
            } else {
                format!("Liana-{}?", &self.wallet_settings.descriptor_checksum)
            }
        );
        let help_text_2 = match self.internal_bitcoind {
            Some(true) => Some("(The Liana-managed Bitcoin node for this network will not be affected by this action.)"),
            Some(false) => None,
            None => Some("(If you are using a Liana-managed Bitcoin node, it will not be affected by this action.)"),
        };
        let help_text_3 = "WARNING: This cannot be undone.";
        let title = if let Some(alias) = &self.wallet_settings.alias {
            format!(
                "Delete configuration for {} (Liana-{})",
                alias, &self.wallet_settings.descriptor_checksum
            )
        } else {
            format!(
                "Delete configuration for Liana-{}",
                &self.wallet_settings.descriptor_checksum
            )
        };
        let title = Container::new(
            new::h3_semi(title)
                .style(theme::text::destructive)
                .width(Length::Fill),
        );
        let help_text_1 = row![new::caption(help_text_1)];
        let help_text_2 = help_text_2.map(|t| row![new::caption(t).style(theme::text::secondary)]);
        let liana_connect_delete = self.wallet_settings.remote_backend_auth.as_ref().map(|a| {
            checkbox(self.delete_liana_connect)
                .label(match self.user_role {
                    Some(UserRole::Owner) | None => {
                        "Also permanently delete this wallet from Liana Connect (for all members)."
                            .to_string()
                    }
                    Some(UserRole::Member) => format!(
                        "Also disassociate {} from this Liana Connect wallet.",
                        a.email
                    ),
                })
                .on_toggle_maybe(if !self.deleted {
                    Some(|v| ViewMessage::DeleteWallet(DeleteWalletMessage::DeleteLianaConnect(v)))
                } else {
                    None
                })
        });
        let help_text_3 = row![new::caption(help_text_3)];
        let warning = self
            .warning
            .as_ref()
            .map(|w| notification::warning(w.to_string(), w.to_string()).width(Length::Fill));
        let footer = Container::new(if !self.deleted {
            row![confirm_button]
        } else {
            row![
                icon::circle_check_icon().style(theme::text::success),
                new::caption("Wallet successfully deleted").style(theme::text::success),
            ]
            .spacing(10)
        })
        .align_x(Horizontal::Center)
        .width(Length::Fill);
        let content = column![
            title,
            help_text_1,
            help_text_2,
            Space::with_height(0),
            liana_connect_delete,
            help_text_3,
            warning,
            footer,
        ]
        .spacing(10);

        Into::<Element<ViewMessage>>::into(card::simple(content).width(700)).map(Message::View)
    }
}

pub async fn check_membership(
    network: Network,
    network_dir: &NetworkDirectory,
    auth: &AuthConfig,
    backend_type: BackendType,
) -> Result<Option<UserRole>, DeleteError> {
    let service_config = get_service_config(network, backend_type)
        .await
        .map_err(|e| DeleteError::Connect(e.to_string()))?;

    if let BackendState::WalletExists(client, _, _, _) = connect_with_credentials(
        AuthClient::new(
            service_config.auth_api_url,
            service_config.auth_api_public_key,
            auth.email.to_string(),
            backend_type.user_agent(),
        ),
        auth.wallet_id.clone(),
        service_config.backend_api_url,
        network,
        network_dir,
    )
    .await
    .map_err(|e| DeleteError::Connect(e.to_string()))?
    {
        Ok(Some(
            client
                .user_wallet_membership()
                .await
                .map_err(|e| DeleteError::Connect(e.to_string()))?,
        ))
    } else {
        Ok(None)
    }
}

async fn check_network_datadir(path: NetworkDirectory) -> Result<State, String> {
    let mut config_path = path.clone().path().to_path_buf();
    config_path.push(app::config::DEFAULT_FILE_NAME);

    if let Err(e) = app::Config::from_file(&config_path) {
        if e == app::config::ConfigError::NotFound {
            return Ok(State::NoWallet);
        } else {
            return Err(format!(
                "Failed to read GUI configuration file in the directory: {}",
                path.path().to_string_lossy()
            ));
        }
    };

    let mut daemon_config_path = path.clone().path().to_path_buf();
    daemon_config_path.push("daemon.toml");

    if daemon_config_path.exists() {
        lianad::config::Config::from_file(Some(daemon_config_path.clone())).map_err(|e| match e {
        ConfigError::FileNotFound
        | ConfigError::DatadirNotFound => {
            format!(
                "Failed to read daemon configuration file in the directory: {}",
                daemon_config_path.to_string_lossy()
            )
        }
        ConfigError::ReadingFile(e) => {
            if e.starts_with("Parsing configuration file: Error parsing descriptor") {
                "There is an issue with the configuration for this network. You most likely use a descriptor containing one or more public key(s) without origin. Liana v0.2 and later only support public keys with origins. Please migrate your funds using Liana v0.1.".to_string()
            } else {
                format!(
                    "Failed to read daemon configuration file in the directory: {}",
                    daemon_config_path.to_string_lossy()
                )
            }
        }
        ConfigError::UnexpectedDescriptor(_) => {
            "There is an issue with the configuration for this network. You most likely use a descriptor containing one or more public key(s) without origin. Liana v0.2 and later only support public keys with origins. Please migrate your funds using Liana v0.1.".to_string()
        }
        ConfigError::Unexpected(e) => {
            format!("Unexpected {e}")
        }
    })?;
    }

    match settings::Settings::from_file(&path) {
        Ok(s) => {
            if s.wallets.is_empty() {
                Ok(State::NoWallet)
            } else {
                Ok(State::Wallets {
                    wallets: s.wallets,
                    add_wallet: false,
                })
            }
        }
        Err(settings::SettingsError::NotFound) => Ok(State::NoWallet),
        Err(e) => Err(e.to_string()),
    }
}
