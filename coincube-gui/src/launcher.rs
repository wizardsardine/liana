use iced::{
    alignment::Horizontal,
    widget::{checkbox, pick_list, scrollable, Button, Space},
    Alignment, Length, Subscription, Task,
};

use coincube_core::miniscript::bitcoin::Network;
use coincube_ui::{
    component::{button, card, network_banner, notification, spinner, text::*},
    icon, image, theme,
    widget::{modal::Modal, Column, Container, Element, Row},
};
use coincubed::config::ConfigError;
use tokio::runtime::Handle;

use crate::pin_input;
use crate::{
    app::{
        self,
        settings::{self, AuthConfig, CubeSettings, WalletSettings},
    },
    delete::{delete_wallet, DeleteError},
    dir::{CoincubeDirectory, NetworkDirectory},
    installer::UserFlow,
    services::connect::{
        client::{auth::AuthClient, backend::api::UserRole, get_service_config},
        login::{connect_with_credentials, BackendState},
    },
};
use coincube_core::signer::HotSigner;

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
    Cubes {
        cubes: Vec<CubeSettings>,
        create_cube: bool,
    },
    NoCube,
}

pub struct Launcher {
    state: State,
    displayed_networks: Vec<Network>,
    network: Network,
    pub datadir_path: CoincubeDirectory,
    error: Option<String>,
    delete_cube_modal: Option<DeleteCubeModal>,
    create_cube_name: coincube_ui::component::form::Value<String>,
    create_cube_pin: pin_input::PinInput,
    create_cube_pin_confirm: pin_input::PinInput,
    creating_cube: bool,
}

impl Launcher {
    pub fn new(datadir_path: CoincubeDirectory, network: Option<Network>) -> (Self, Task<Message>) {
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
                displayed_networks: NETWORKS.to_vec(),
                network,
                datadir_path: datadir_path.clone(),
                error: None,
                delete_cube_modal: None,
                create_cube_name: coincube_ui::component::form::Value::default(),
                create_cube_pin: pin_input::PinInput::new(),
                create_cube_pin_confirm: pin_input::PinInput::new(),
                creating_cube: false,
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
            Message::View(ViewMessage::ShowCreateCube(show)) => {
                if let State::Cubes { create_cube, .. } = &mut self.state {
                    *create_cube = show;
                    if !show {
                        self.create_cube_name = coincube_ui::component::form::Value::default();
                        self.create_cube_pin = pin_input::PinInput::new();
                        self.create_cube_pin_confirm = pin_input::PinInput::new();
                    }
                }
                Task::none()
            }
            Message::View(ViewMessage::CubeNameEdited(name)) => {
                self.create_cube_name.value = name;
                self.create_cube_name.valid = !self.create_cube_name.value.trim().is_empty();
                self.error = None; // Clear error when user makes changes
                Task::none()
            }
            Message::View(ViewMessage::PinInput(msg)) => {
                self.error = None;
                self.create_cube_pin
                    .update(msg)
                    .map(|m| Message::View(ViewMessage::PinInput(m)))
            }
            Message::View(ViewMessage::PinConfirmInput(msg)) => {
                self.error = None;
                self.create_cube_pin_confirm
                    .update(msg)
                    .map(|m| Message::View(ViewMessage::PinConfirmInput(m)))
            }
            Message::View(ViewMessage::CreateCube) => {
                if self.creating_cube {
                    return Task::none();
                }

                if self.create_cube_name.value.trim().is_empty() {
                    return Task::none();
                }

                // Validate PIN (always required)
                if !self.create_cube_pin.is_complete() {
                    self.error = Some("Please enter all 4 PIN digits".to_string());
                    return Task::none();
                }
                if !self.create_cube_pin_confirm.is_complete() {
                    self.error = Some("Please confirm all 4 PIN digits".to_string());
                    return Task::none();
                }
                if self.create_cube_pin.value() != self.create_cube_pin_confirm.value() {
                    self.error = Some("PIN codes do not match".to_string());
                    return Task::none();
                }

                self.creating_cube = true;
                let network = self.network;
                let cube_name = self.create_cube_name.value.trim().to_string();
                let pin = self.create_cube_pin.value();
                let datadir_path = self.datadir_path.clone();

                Task::perform(
                    async move {
                        // Generate Liquid wallet HotSigner
                        let liquid_signer = HotSigner::generate(network).map_err(|e| {
                            format!("Failed to generate Liquid wallet signer: {}", e)
                        })?;

                        // Create secp context for fingerprint calculation
                        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
                        let liquid_fingerprint = liquid_signer.fingerprint(&secp);

                        // Store Liquid wallet mnemonic (encrypted with PIN if provided)
                        let network_dir = datadir_path.network_directory(network);
                        network_dir
                            .init()
                            .map_err(|e| format!("Failed to create network directory: {}", e))?;

                        // Use a timestamp for the Liquid wallet storage
                        let timestamp = chrono::Utc::now().timestamp();
                        let liquid_checksum = format!("liquid_{}", timestamp);

                        // Store Liquid wallet mnemonic encrypted with PIN (always required)
                        liquid_signer
                            .store_encrypted(
                                datadir_path.path(),
                                network,
                                &secp,
                                Some((liquid_checksum, timestamp)),
                                Some(&pin),
                            )
                            .map_err(|e| {
                                format!("Failed to store Liquid wallet mnemonic: {}", e)
                            })?;

                        tracing::info!("Liquid wallet signer created and stored (encrypted with PIN) with fingerprint: {}", liquid_fingerprint);

                        // Create Cube settings with Liquid wallet signer reference and PIN
                        let cube = CubeSettings::new(cube_name, network)
                            .with_liquid_signer(liquid_fingerprint)
                            .with_pin(&pin)
                            .map_err(|e| format!("Failed to hash PIN: {}", e))?;

                        // Save Cube settings to settings file
                        settings::update_settings_file(&network_dir, |mut settings| {
                            settings.cubes.push(cube.clone());
                            Some(settings)
                        })
                        .await
                        .map(|_| cube)
                        .map_err(|e| e.to_string())
                    },
                    Message::CubeCreated,
                )
            }
            Message::CubeCreated(res) => {
                self.creating_cube = false;
                match res {
                    Ok(_cube) => {
                        // Clear any previous error state
                        self.error = None;
                        // Reset form fields
                        self.create_cube_name = coincube_ui::component::form::Value::default();
                        self.create_cube_pin = pin_input::PinInput::new();
                        self.create_cube_pin_confirm = pin_input::PinInput::new();
                        self.reload()
                    }
                    Err(e) => {
                        self.error = Some(format!("Failed to create Cube: {}", e));
                        Task::none()
                    }
                }
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::ShowModal(i))) => {
                if let State::Cubes { cubes, .. } = &self.state {
                    if let Some(cube) = cubes.get(i) {
                        let wallet_datadir = self.datadir_path.network_directory(cube.network);
                        let config_path =
                            wallet_datadir.path().join(app::config::DEFAULT_FILE_NAME);

                        // Get wallet settings if vault exists
                        let (wallet_settings, internal_bitcoind) =
                            if let Some(vault_id) = &cube.vault_wallet_id {
                                match settings::Settings::from_file(&wallet_datadir) {
                                    Ok(s) => {
                                        if let Some(wallet) =
                                            s.wallets.iter().find(|w| w.wallet_id() == *vault_id)
                                        {
                                            let internal_bitcoind =
                                                if wallet.remote_backend_auth.is_some() {
                                                    Some(false)
                                                } else if wallet.start_internal_bitcoind.is_some() {
                                                    wallet.start_internal_bitcoind
                                                } else if let Ok(cfg) =
                                                    app::Config::from_file(&config_path)
                                                {
                                                    Some(cfg.start_internal_bitcoind)
                                                } else {
                                                    None
                                                };
                                            (Some(wallet.clone()), internal_bitcoind)
                                        } else {
                                            (None, None)
                                        }
                                    }
                                    Err(_) => (None, None),
                                }
                            } else {
                                (None, None)
                            };

                        self.delete_cube_modal = Some(DeleteCubeModal::new(
                            cube.clone(),
                            wallet_datadir,
                            wallet_settings,
                            internal_bitcoind,
                        ));
                    }
                }
                Task::none()
            }
            Message::View(ViewMessage::SelectNetwork(network)) => {
                self.network = network;
                let network_dir = self.datadir_path.network_directory(self.network);
                Task::perform(check_network_datadir(network_dir), Message::Checked)
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::Deleted)) => {
                // Close modal and reload cubes - Checked will determine the correct state
                self.delete_cube_modal = None;
                self.reload()
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::CloseModal)) => {
                self.delete_cube_modal = None;
                if self.network == Network::Testnet
                    && !has_existing_wallet(&self.datadir_path, Network::Testnet)
                {
                    self.network = Network::Testnet4;
                }
                Task::none()
            }
            Message::Checked(res) => match res {
                Err(e) => {
                    self.error = Some(e.to_string());
                    Task::none()
                }
                Ok(state) => {
                    self.state = state;
                    Task::none()
                }
            },
            Message::View(ViewMessage::Run(index)) => {
                if let State::Cubes { cubes, .. } = &self.state {
                    if let Some(cube) = cubes.get(index) {
                        let datadir_path = self.datadir_path.clone();
                        let mut path = self
                            .datadir_path
                            .network_directory(cube.network)
                            .path()
                            .to_path_buf();
                        path.push(app::config::DEFAULT_FILE_NAME);
                        let cfg = app::Config::from_file(&path).expect("Already checked");
                        let network = cube.network;
                        let cube = cube.clone();
                        Task::perform(
                            async move { (datadir_path.clone(), cfg, network, cube) },
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
                if let Some(modal) = &mut self.delete_cube_modal {
                    return modal.update(message);
                }
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<Message> {
        let content = Into::<Element<ViewMessage>>::into(scrollable(
            Column::new()
                .push(
                    Row::new()
                        .spacing(20)
                        .push(image::coincube_logotype().width(Length::Fixed(150.0)))
                        .push(Space::new().width(Length::Fill))
                        .push(if let State::Cubes { create_cube, .. } = &self.state {
                            if *create_cube {
                                Some(
                                    button::secondary(
                                        Some(icon::previous_icon()),
                                        "Back to Cube list",
                                    )
                                    .on_press_maybe(
                                        if self.creating_cube {
                                            None
                                        } else {
                                            Some(ViewMessage::ShowCreateCube(false))
                                        },
                                    ),
                                )
                            } else {
                                None
                            }
                        } else {
                            None
                        })
                        .push(
                            button::xpubs_button(None, "Share Xpubs")
                                .on_press(ViewMessage::ShareXpubs),
                        )
                        .push(
                            pick_list(
                                self.displayed_networks.as_slice(),
                                Some(self.network),
                                ViewMessage::SelectNetwork,
                            )
                            .style(theme::pick_list::primary)
                            .padding(10),
                        )
                        .align_y(Alignment::Center)
                        .padding(100),
                )
                .push(
                    Container::new(
                        Column::new()
                            .align_x(Alignment::Center)
                            .spacing(30)
                            .push({
                                let c = if matches!(self.state, State::Cubes { .. }) {
                                    "Welcome back"
                                } else {
                                    "Welcome"
                                };
                                text(c).size(50).bold()
                            })
                            .push({
                                // Only show error at top if not in create cube form
                                let in_create_form = matches!(
                                    self.state,
                                    State::Cubes {
                                        create_cube: true,
                                        ..
                                    } | State::NoCube
                                );
                                if !in_create_form {
                                    self.error.as_ref().map(|e| card::simple(text(e)))
                                } else {
                                    None
                                }
                            })
                            .push(match &self.state {
                                State::Cubes { cubes, create_cube } => {
                                    if *create_cube {
                                        create_cube_form(
                                            &self.create_cube_name,
                                            &self.create_cube_pin,
                                            &self.create_cube_pin_confirm,
                                            &self.error,
                                            self.creating_cube,
                                        )
                                    } else {
                                        let mut col =
                                            cubes.iter().enumerate().fold(
                                                Column::new().spacing(20),
                                                |col, (i, cube)| col.push(cubes_list_item(cube, i)),
                                            );
                                        col = col.push(
                                            Column::new().push(
                                                button::secondary(
                                                    Some(icon::plus_icon()),
                                                    "Create Cube",
                                                )
                                                .on_press(ViewMessage::ShowCreateCube(true))
                                                .padding(10)
                                                .width(Length::Fixed(500.0)),
                                            ),
                                        );
                                        col.into()
                                    }
                                }
                                State::NoCube | State::Unchecked => create_cube_form(
                                    &self.create_cube_name,
                                    &self.create_cube_pin,
                                    &self.create_cube_pin_confirm,
                                    &self.error,
                                    self.creating_cube,
                                ),
                            })
                            .align_x(Alignment::Center),
                    )
                    .center_x(Length::Fill),
                )
                .push(Space::new().height(Length::Fixed(100.0))),
        ))
        .map(Message::View);
        let content = if self.network != Network::Bitcoin {
            Column::with_children(vec![network_banner(self.network).into(), content]).into()
        } else {
            content
        };
        if let Some(modal) = &self.delete_cube_modal {
            Modal::new(Container::new(content).height(Length::Fill), modal.view())
                .on_blur(Some(Message::View(ViewMessage::DeleteCube(
                    DeleteCubeMessage::CloseModal,
                ))))
                .into()
        } else {
            content
        }
    }
}

fn create_cube_form<'a>(
    cube_name: &coincube_ui::component::form::Value<String>,
    pin: &'a pin_input::PinInput,
    pin_confirm: &'a pin_input::PinInput,
    error: &Option<String>,
    creating_cube: bool,
) -> Element<'a, ViewMessage> {
    use coincube_ui::component::form;
    use std::time::Duration;

    let mut column = Column::new()
        .spacing(20)
        .align_x(Alignment::Center)
        .width(Length::Fixed(500.0))
        .push(h4_bold("Create a new Cube"))
        .push(
            p1_regular(
                "A Cube is your account which can contain a Liquid wallet, a Vault wallet and other features.",
            )
            .style(theme::text::secondary),
        )
        .push(
            Container::new(
                form::Form::new("Cube Name", cube_name, ViewMessage::CubeNameEdited)
                    .warning("Please enter a name")
                    .size(20)
                    .padding(10),
            )
            .width(Length::Fill),
        );

    // PIN setup section (always required)
    column = column.push(Space::new().height(Length::Fixed(10.0)));

    let pin_label = p1_regular("Enter PIN:").style(theme::text::secondary);
    column = column.push(pin_label);
    column = column.push(pin.view().map(ViewMessage::PinInput));

    column = column.push(Space::new().height(Length::Fixed(20.0)));

    let pin_confirm_label = p1_regular("Confirm PIN:").style(theme::text::secondary);
    column = column.push(pin_confirm_label);
    column = column.push(pin_confirm.view().map(ViewMessage::PinConfirmInput));

    // Add extra padding before Create Cube button
    column = column.push(Space::new().height(Length::Fixed(20.0)));

    // Show error above the button
    if let Some(err) = error {
        column = column.push(p1_regular(err).style(theme::text::error));
    }

    // Determine if button should be enabled
    // PIN is always required, so all PIN fields must be filled
    let can_create = !creating_cube
        && cube_name.valid
        && !cube_name.value.trim().is_empty()
        && pin.is_complete()
        && pin_confirm.is_complete();

    let submit_button = if creating_cube {
        iced::widget::button(
            Container::new(
                Row::new()
                    .spacing(5)
                    .align_y(Alignment::Center)
                    .push(text("Creating"))
                    .push(
                        Container::new(spinner::typing_text_carousel(
                            "...",
                            true,
                            Duration::from_millis(500),
                            text,
                        ))
                        .width(Length::Fixed(20.0)),
                    ),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill),
        )
        .width(Length::Fixed(200.0))
        .height(Length::Fixed(44.0))
        .style(theme::button::primary)
    } else {
        button::primary(None, "Create Cube")
            .width(Length::Fixed(200.0))
            .on_press_maybe(if can_create {
                Some(ViewMessage::CreateCube)
            } else {
                None
            })
    };

    column = column.push(submit_button);

    Container::new(column)
        .padding(20)
        .center_x(Length::Fill)
        .into()
}

fn cubes_list_item<'a>(cube: &CubeSettings, i: usize) -> Element<'a, ViewMessage> {
    Container::new(
        Row::new()
            .align_y(Alignment::Center)
            .spacing(20)
            .push(
                Container::new(
                    Button::new(Column::new().push(p1_bold(&cube.name)).push(
                        if let Some(vault_id) = &cube.vault_wallet_id {
                            Some(
                                p1_regular(format!(
                                    "Vault: Coincube-{}",
                                    vault_id.descriptor_checksum
                                ))
                                .style(theme::text::secondary),
                            )
                        } else {
                            Some(p1_regular("No Vault configured").style(theme::text::secondary))
                        },
                    ))
                    .on_press(ViewMessage::Run(i))
                    .padding(15)
                    .style(theme::button::container_border)
                    .width(Length::Fixed(500.0)),
                )
                .style(theme::card::simple),
            )
            .push(
                Button::new(icon::trash_icon())
                    .style(theme::button::secondary)
                    .padding(10)
                    .on_press(ViewMessage::DeleteCube(DeleteCubeMessage::ShowModal(i))),
            ),
    )
    .into()
}

fn has_existing_wallet(data_dir: &CoincubeDirectory, network: Network) -> bool {
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
    Install(CoincubeDirectory, Network, UserFlow),
    Checked(Result<State, String>),
    Run(
        CoincubeDirectory,
        app::config::Config,
        Network,
        CubeSettings,
    ),
    CubeCreated(Result<CubeSettings, String>),
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
    ImportWallet,
    CreateWallet,
    ShowCreateCube(bool),
    CubeNameEdited(String),
    CreateCube,
    PinInput(pin_input::Message),
    PinConfirmInput(pin_input::Message),
    ShareXpubs,
    SelectNetwork(Network),
    StartInstall(Network),
    Check,
    Run(usize),
    DeleteCube(DeleteCubeMessage),
}

#[derive(Debug, Clone)]
pub enum DeleteCubeMessage {
    ShowModal(usize),
    CloseModal,
    Confirm(String), // Cube ID
    DeleteLianaConnect(bool),
    Deleted,
    PinInput(pin_input::Message),
}

struct DeleteCubeModal {
    cube: CubeSettings,
    network_directory: NetworkDirectory,
    wallet_settings: Option<WalletSettings>,
    warning: Option<DeleteError>,
    deleted: bool,
    delete_liana_connect: bool,
    user_role: Option<UserRole>,
    // `None` means we were not able to determine whether wallet uses internal bitcoind.
    internal_bitcoind: Option<bool>,
    pin_input: pin_input::PinInput,
    pin_error: Option<String>,
}

impl DeleteCubeModal {
    fn new(
        cube: CubeSettings,
        network_directory: NetworkDirectory,
        wallet_settings: Option<WalletSettings>,
        internal_bitcoind: Option<bool>,
    ) -> Self {
        let mut modal = Self {
            cube: cube.clone(),
            wallet_settings: wallet_settings.clone(),
            network_directory,
            warning: None,
            deleted: false,
            delete_liana_connect: false,
            internal_bitcoind,
            user_role: None,
            pin_input: pin_input::PinInput::new(),
            pin_error: None,
        };
        if let Some(wallet) = &wallet_settings {
            if let Some(auth) = &wallet.remote_backend_auth {
                match Handle::current().block_on(check_membership(
                    cube.network,
                    &modal.network_directory,
                    auth,
                )) {
                    Err(e) => {
                        modal.warning = Some(e);
                    }
                    Ok(user_role) => {
                        modal.user_role = user_role;
                    }
                }
            }
        }
        modal
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::Confirm(cube_id))) => {
                if cube_id != self.cube.id {
                    return Task::none();
                }

                // Verify PIN before proceeding with deletion
                if self.cube.has_pin() {
                    let pin = self.pin_input.value();
                    if !self.cube.verify_pin(&pin) {
                        self.pin_error = Some("Incorrect PIN. Please try again.".to_string());
                        self.pin_input.clear();
                        return Task::none();
                    }
                }

                self.warning = None;

                // Delete vault if it exists
                if let Some(wallet_settings) = &self.wallet_settings {
                    if let Err(e) = Handle::current().block_on(delete_wallet(
                        self.cube.network,
                        &self.network_directory,
                        wallet_settings,
                        self.delete_liana_connect,
                    )) {
                        self.warning = Some(e);
                        return Task::none();
                    }
                }

                // Delete the cube from settings
                let network_dir = self.network_directory.clone();
                let cube_id = self.cube.id.clone();
                if let Err(e) = Handle::current().block_on(async {
                    settings::update_settings_file(&network_dir, |mut settings| {
                        settings.cubes.retain(|cube| cube.id != cube_id);
                        // Delete file if both cubes and wallets are empty
                        if settings.cubes.is_empty() && settings.wallets.is_empty() {
                            None
                        } else {
                            Some(settings)
                        }
                    })
                    .await
                }) {
                    self.warning = Some(DeleteError::Settings(e));
                } else {
                    self.deleted = true;
                    return Task::perform(async {}, |_| {
                        Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::Deleted))
                    });
                }
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::DeleteLianaConnect(
                delete,
            ))) => {
                self.delete_liana_connect = delete;
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::PinInput(msg))) => {
                self.pin_error = None;
                return self.pin_input.update(msg).map(|m| {
                    Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::PinInput(m)))
                });
            }
            _ => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<Message> {
        let pin_ready = !self.cube.has_pin() || self.pin_input.is_complete();
        let can_delete = pin_ready && self.warning.is_none();
        let mut confirm_button = button::secondary(None, "Delete Cube")
            .width(Length::Fixed(200.0))
            .style(theme::button::destructive);
        if can_delete {
            confirm_button = confirm_button.on_press(ViewMessage::DeleteCube(
                DeleteCubeMessage::Confirm(self.cube.id.clone()),
            ));
        }

        // Determine what's being deleted
        let has_vault = self.wallet_settings.is_some();
        let has_remote_backend = self
            .wallet_settings
            .as_ref()
            .and_then(|w| w.remote_backend_auth.as_ref())
            .is_some();

        let help_text_1 = if has_vault {
            format!(
                "Are you sure you want to delete the Cube \"{}\" and {}?",
                self.cube.name,
                if has_remote_backend {
                    "its associated Vault configuration"
                } else {
                    "all its associated data (including Vault)"
                }
            )
        } else {
            format!(
                "Are you sure you want to delete the Cube \"{}\"?",
                self.cube.name
            )
        };

        let help_text_2 = match self.internal_bitcoind {
            Some(true) => Some("(The Liana-managed Bitcoin node for this network will not be affected by this action.)"),
            Some(false) => None,
            None if has_vault => Some("(If you are using a Liana-managed Bitcoin node, it will not be affected by this action.)"),
            _ => None,
        };
        let help_text_3 = "WARNING: This cannot be undone.";

        let mut col = Column::new()
            .spacing(10)
            .push(Container::new(
                h4_bold(format!("Delete Cube \"{}\"", self.cube.name))
                .style(theme::text::destructive)
                .width(Length::Fill),
            ))
            .push(Row::new().push(text(help_text_1)))
            .push(
                help_text_2
                    .map(|t| Row::new().push(p1_regular(t).style(theme::text::secondary))),
            )
            .push(Row::new())
            .push(self.wallet_settings.as_ref().and_then(|w| w.remote_backend_auth.as_ref()).map(|a| {
                checkbox(
                    self.delete_liana_connect,
                )
                .label(
                        match self.user_role {
                            Some(UserRole::Owner) | None => "Also permanently delete the Vault wallet from Liana Connect (for all members).".to_string(),
                            Some(UserRole::Member) => format!("Also disassociate {} from this Liana Connect wallet.", a.email),
                        }
                    )
                .on_toggle_maybe(if !self.deleted {
                        Some(|v| {
                            ViewMessage::DeleteCube(DeleteCubeMessage::DeleteLianaConnect(v))
                        })
                    } else {
                        None
                    })
            }))
            .push(Row::new().push(text(help_text_3)));

        // PIN entry section
        if self.cube.has_pin() {
            col = col
                .push(Space::new().height(Length::Fixed(5.0)))
                .push(p1_regular("Enter your PIN to confirm:").style(theme::text::secondary))
                .push(
                    Container::new(
                        self.pin_input
                            .view()
                            .map(|m| ViewMessage::DeleteCube(DeleteCubeMessage::PinInput(m))),
                    )
                    .center_x(Length::Fill),
                );
            if let Some(err) = &self.pin_error {
                col = col.push(
                    Container::new(p1_regular(err).style(theme::text::error))
                        .center_x(Length::Fill),
                );
            }
        }

        col = col
            .push(
                self.warning.as_ref().map(|w| {
                    notification::warning(w.to_string(), w.to_string()).width(Length::Fill)
                }),
            )
            .push(
                Container::new(if !self.deleted {
                    Row::new().push(confirm_button)
                } else {
                    Row::new()
                        .spacing(10)
                        .push(icon::square_check_icon().style(theme::text::success))
                        .push(text("Cube successfully deleted").style(theme::text::success))
                })
                .align_x(Horizontal::Center)
                .width(Length::Fill),
            );

        Into::<Element<ViewMessage>>::into(card::simple(col).width(Length::Fixed(700.0)))
            .map(Message::View)
    }
}

pub async fn check_membership(
    network: Network,
    network_dir: &NetworkDirectory,
    auth: &AuthConfig,
) -> Result<Option<UserRole>, DeleteError> {
    let service_config = get_service_config(network)
        .await
        .map_err(|e| DeleteError::Connect(e.to_string()))?;

    if let BackendState::WalletExists(client, _, _) = connect_with_credentials(
        AuthClient::new(
            service_config.auth_api_url,
            service_config.auth_api_public_key,
            auth.email.to_string(),
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
    // Ensure the network directory exists
    if let Err(e) = tokio::fs::create_dir_all(path.path()).await {
        return Err(format!(
            "Failed to create directory {}: {}",
            path.path().to_string_lossy(),
            e
        ));
    }

    let mut config_path = path.clone().path().to_path_buf();
    config_path.push(app::config::DEFAULT_FILE_NAME);

    if let Err(e) = app::Config::from_file(&config_path) {
        if e == app::config::ConfigError::NotFound {
            // Create default config file
            let default_config = app::Config::new(false);
            if let Err(e) = default_config.to_file(&config_path) {
                return Err(format!("Failed to create default GUI config file: {}", e));
            }
            return Ok(State::NoCube);
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
        coincubed::config::Config::from_file(Some(daemon_config_path.clone())).map_err(
            |e| match e {
                ConfigError::FileNotFound | ConfigError::DatadirNotFound => {
                    format!(
                        "Failed to read daemon configuration file in the directory: {}",
                        daemon_config_path.to_string_lossy()
                    )
                }
                ConfigError::ReadingFile(e) => {
                    if e.starts_with("Parsing configuration file: Error parsing descriptor") {
                        "There is an issue with the configuration for this network. You most likely use a descriptor containing one or more public key(s) without origin.".to_string()
                    } else {
                        format!(
                            "Failed to read daemon configuration file in the directory: {}",
                            daemon_config_path.to_string_lossy()
                        )
                    }
                }
                ConfigError::UnexpectedDescriptor(_) => {
                    "There is an issue with the configuration for this network. You most likely use a descriptor containing one or more public key(s) without origin.".to_string()
                }
                ConfigError::Unexpected(e) => {
                    format!("Unexpected {}", e)
                }
            },
        )?;
    }

    // Try to load cubes from settings
    match settings::Settings::from_file(&path) {
        Ok(s) => {
            // All cubes are required to have PINs
            if s.cubes.is_empty() {
                Ok(State::NoCube)
            } else {
                Ok(State::Cubes {
                    cubes: s.cubes,
                    create_cube: false,
                })
            }
        }
        Err(settings::SettingsError::NotFound) => Ok(State::NoCube),
        Err(e) => Err(format!("Failed to read settings: {}", e)),
    }
}
