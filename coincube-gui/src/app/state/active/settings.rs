use std::sync::Arc;

use coincube_ui::widget::*;
use iced::Task;

use crate::app::settings::{update_settings_file, Settings};
use crate::app::view::ActiveSettingsMessage;
use crate::app::{breez::BreezClient, cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::dir::CoincubeDirectory;

#[derive(Debug, Clone, PartialEq)]
pub enum BackupWalletState {
    Intro(bool),
    RecoveryPhrase,
    Verification {
        word_2: String,
        word_5: String,
        word_9: String,
        error: Option<String>,
    },
    Completed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActiveSettingsFlowState {
    MainMenu { backed_up: bool, mfa: bool },
    BackupWallet(BackupWalletState),
}

/// ActiveSettings is a placeholder panel for the Active Settings page
pub struct ActiveSettings {
    breez_client: Arc<BreezClient>,
    flow_state: ActiveSettingsFlowState,
}

impl ActiveSettings {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        let (backed_up, mfa) = fetch_main_menu_state(breez_client.clone());
        Self {
            breez_client,
            flow_state: ActiveSettingsFlowState::MainMenu { backed_up, mfa },
        }
    }
}

impl State for ActiveSettings {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            menu,
            cache,
            None,
            view::active::active_settings_view(self.breez_client.active_signer(), &self.flow_state),
        )
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::ActiveSettings(ActiveSettingsMessage::BackupWallet(
                backup_msg,
            ))) => {
                tracing::info!("Got BackupWallet message: {:?}", backup_msg);
                match backup_msg {
                    view::BackupWalletMessage::ToggleBackupIntroCheck => {
                        if let ActiveSettingsFlowState::BackupWallet(BackupWalletState::Intro(
                            checked,
                        )) = self.flow_state
                        {
                            self.flow_state = ActiveSettingsFlowState::BackupWallet(
                                BackupWalletState::Intro(!checked),
                            );
                        }
                    }
                    view::BackupWalletMessage::Start => {
                        self.flow_state =
                            ActiveSettingsFlowState::BackupWallet(BackupWalletState::Intro(false));
                    }
                    view::BackupWalletMessage::NextStep => {
                        self.flow_state = match &self.flow_state {
                            ActiveSettingsFlowState::BackupWallet(BackupWalletState::Intro(
                                true,
                            )) => ActiveSettingsFlowState::BackupWallet(
                                BackupWalletState::RecoveryPhrase,
                            ),
                            ActiveSettingsFlowState::BackupWallet(
                                BackupWalletState::RecoveryPhrase,
                            ) => ActiveSettingsFlowState::BackupWallet(
                                BackupWalletState::Verification {
                                    word_2: String::new(),
                                    word_5: String::new(),
                                    word_9: String::new(),
                                    error: None,
                                },
                            ),
                            _ => self.flow_state.clone(),
                        };
                    }
                    view::BackupWalletMessage::PreviousStep => {
                        let (backed_up, mfa) = fetch_main_menu_state(self.breez_client.clone());
                        self.flow_state = match &self.flow_state {
                            ActiveSettingsFlowState::BackupWallet(BackupWalletState::Intro(_)) => {
                                ActiveSettingsFlowState::MainMenu { backed_up, mfa }
                            }
                            ActiveSettingsFlowState::BackupWallet(
                                BackupWalletState::RecoveryPhrase,
                            ) => ActiveSettingsFlowState::BackupWallet(BackupWalletState::Intro(
                                false,
                            )),
                            ActiveSettingsFlowState::BackupWallet(
                                BackupWalletState::Verification { .. },
                            ) => ActiveSettingsFlowState::BackupWallet(
                                BackupWalletState::RecoveryPhrase,
                            ),
                            ActiveSettingsFlowState::BackupWallet(BackupWalletState::Completed) => {
                                ActiveSettingsFlowState::MainMenu { backed_up, mfa }
                            }
                            ActiveSettingsFlowState::MainMenu { backed_up, mfa } => {
                                ActiveSettingsFlowState::MainMenu {
                                    backed_up: *backed_up,
                                    mfa: *mfa,
                                }
                            }
                        };
                    }
                    view::BackupWalletMessage::Word2Input(input) => {
                        if let ActiveSettingsFlowState::BackupWallet(
                            BackupWalletState::Verification {
                                word_5,
                                word_9,
                                error,
                                ..
                            },
                        ) = &self.flow_state
                        {
                            self.flow_state = ActiveSettingsFlowState::BackupWallet(
                                BackupWalletState::Verification {
                                    word_2: input,
                                    word_5: word_5.clone(),
                                    word_9: word_9.clone(),
                                    error: error.clone(),
                                },
                            );
                        }
                    }
                    view::BackupWalletMessage::Word5Input(input) => {
                        if let ActiveSettingsFlowState::BackupWallet(
                            BackupWalletState::Verification {
                                word_2,
                                word_9,
                                error,
                                ..
                            },
                        ) = &self.flow_state
                        {
                            self.flow_state = ActiveSettingsFlowState::BackupWallet(
                                BackupWalletState::Verification {
                                    word_2: word_2.clone(),
                                    word_5: input,
                                    word_9: word_9.clone(),
                                    error: error.clone(),
                                },
                            );
                        }
                    }
                    view::BackupWalletMessage::Word9Input(input) => {
                        if let ActiveSettingsFlowState::BackupWallet(
                            BackupWalletState::Verification {
                                word_2,
                                word_5,
                                error,
                                ..
                            },
                        ) = &self.flow_state
                        {
                            self.flow_state = ActiveSettingsFlowState::BackupWallet(
                                BackupWalletState::Verification {
                                    word_2: word_2.clone(),
                                    word_5: word_5.clone(),
                                    word_9: input,
                                    error: error.clone(),
                                },
                            );
                        }
                    }
                    view::BackupWalletMessage::VerifyPhrase => {
                        if let ActiveSettingsFlowState::BackupWallet(
                            BackupWalletState::Verification {
                                word_2,
                                word_5,
                                word_9,
                                ..
                            },
                        ) = &self.flow_state
                        {
                            // Get the actual mnemonic words
                            let mnemonic = self
                                .breez_client
                                .active_signer()
                                .lock()
                                .expect("Mutex Lock Poisoned")
                                .words();

                            // Verify words (index 1, 4, 8 since arrays are 0-indexed)
                            let correct_word_2 = mnemonic[1];
                            let correct_word_5 = mnemonic[4];
                            let correct_word_9 = mnemonic[8];

                            if word_2.trim() == correct_word_2
                                && word_5.trim() == correct_word_5
                                && word_9.trim() == correct_word_9
                            {
                                // Verification successful
                                self.flow_state = ActiveSettingsFlowState::BackupWallet(
                                    BackupWalletState::Completed,
                                );
                            } else {
                                // Verification failed
                                self.flow_state = ActiveSettingsFlowState::BackupWallet(
                                    BackupWalletState::Verification {
                                        word_2: word_2.clone(),
                                        word_5: word_5.clone(),
                                        word_9: word_9.clone(),
                                        error: Some(
                                            "The words you entered don't match. Please try again."
                                                .to_string(),
                                        ),
                                    },
                                );
                            }
                        }
                    }
                    view::BackupWalletMessage::Complete => {
                        let (_, mfa) = fetch_main_menu_state(self.breez_client.clone());
                        self.flow_state = ActiveSettingsFlowState::MainMenu {
                            backed_up: true,
                            mfa,
                        };

                        let breez_client = self.breez_client.clone();
                        let update_task = Task::perform(
                            async move {
                                let secp =
                                    coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
                                let fingerprint = breez_client
                                    .active_signer()
                                    .lock()
                                    .expect("Mutex Lock Poisoned")
                                    .fingerprint(&secp);

                                let dir = match CoincubeDirectory::new_default() {
                                    Ok(d) => d,
                                    Err(e) => {
                                        tracing::error!("Failed to get CoincubeDirectory: {}", e);
                                        return;
                                    }
                                };

                                let network_dir = dir.network_directory(breez_client.network());
                                if let Err(e) =
                                    update_settings_file(&network_dir, |mut settings| {
                                        if let Some(cube) = settings.cubes.iter_mut().find(|cube| {
                                            cube.active_wallet_signer_fingerprint.as_ref()
                                                == Some(&fingerprint)
                                        }) {
                                            cube.backed_up = true;
                                        }
                                        Some(settings)
                                    })
                                    .await
                                {
                                    tracing::error!("Failed to update settings file: {}", e);
                                }
                            },
                            |_| Message::Tick,
                        );

                        return update_task;
                    }
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        // Active wallet doesn't use Vault wallet - uses BreezClient instead
        Task::none()
    }
}

fn fetch_main_menu_state(breez_client: Arc<BreezClient>) -> (bool, bool) {
    let mut backed_up = false;
    let mut mfa = false;
    let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
    let fingerprint = breez_client
        .active_signer()
        .lock()
        .expect("Mutex Lock Poisoned")
        .fingerprint(&secp);
    match CoincubeDirectory::new_default() {
        Ok(dir) => {
            let network_dir = dir.network_directory(breez_client.network());
            match Settings::from_file(&network_dir) {
                Ok(settings) => {
                    let cube = settings.cubes.into_iter().find(|cube| {
                        cube.active_wallet_signer_fingerprint.as_ref() == Some(&fingerprint)
                    });
                    if let Some(cube) = cube {
                        backed_up = cube.backed_up;
                        mfa = cube.mfa_done;
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
    (backed_up, mfa)
}
