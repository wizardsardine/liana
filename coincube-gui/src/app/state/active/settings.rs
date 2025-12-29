use std::sync::Arc;

use coincube_ui::widget::*;
use iced::Task;
use rand::seq::SliceRandom;

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
        word_indices: [usize; 3], // Random indices (e.g., [2, 5, 9] but randomized)
        word_inputs: [String; 3], // User inputs for the three words
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

/// Generate 3 random unique word indices from 1-12
fn generate_random_word_indices() -> [usize; 3] {
    let mut indices: Vec<usize> = (1..=12).collect();
    let mut rng = rand::thread_rng();
    indices.shuffle(&mut rng);
    [indices[0], indices[1], indices[2]]
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
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::ActiveSettings(ActiveSettingsMessage::BackupWallet(
                backup_msg,
            ))) => {
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
                                    word_indices: generate_random_word_indices(),
                                    word_inputs: [String::new(), String::new(), String::new()],
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
                    view::BackupWalletMessage::WordInput { index, input } => {
                        if let ActiveSettingsFlowState::BackupWallet(
                            BackupWalletState::Verification {
                                word_indices,
                                word_inputs,
                                error,
                            },
                        ) = &self.flow_state
                        {
                            // Find which position in our array this index corresponds to
                            let mut new_inputs = word_inputs.clone();
                            if let Some(pos) =
                                word_indices.iter().position(|&i| i == index as usize)
                            {
                                new_inputs[pos] = input;
                            }

                            self.flow_state = ActiveSettingsFlowState::BackupWallet(
                                BackupWalletState::Verification {
                                    word_indices: *word_indices,
                                    word_inputs: new_inputs,
                                    error: error.clone(),
                                },
                            );
                        }
                    }
                    view::BackupWalletMessage::VerifyPhrase => {
                        if let ActiveSettingsFlowState::BackupWallet(
                            BackupWalletState::Verification {
                                word_indices,
                                word_inputs,
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

                            // Verify each word matches the correct position in the mnemonic
                            // word_indices are 1-based, mnemonic array is 0-based
                            let all_correct =
                                word_indices.iter().enumerate().all(|(i, &word_idx)| {
                                    word_inputs[i].trim() == mnemonic[word_idx - 1]
                                });

                            if all_correct {
                                // Verification successful
                                self.flow_state = ActiveSettingsFlowState::BackupWallet(
                                    BackupWalletState::Completed,
                                );
                            } else {
                                // Verification failed
                                self.flow_state = ActiveSettingsFlowState::BackupWallet(
                                    BackupWalletState::Verification {
                                        word_indices: *word_indices,
                                        word_inputs: word_inputs.clone(),
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
                        let breez_client = self.breez_client.clone();
                        return Task::perform(
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
                            |_| {
                                Message::View(view::Message::ActiveSettings(
                                    view::ActiveSettingsMessage::SettingsUpdated,
                                ))
                            },
                        );
                    }
                }
            }
            Message::View(view::Message::ActiveSettings(
                ActiveSettingsMessage::SettingsUpdated,
            )) => {
                // Settings file was updated, refresh the state
                let (backed_up, mfa) = fetch_main_menu_state(self.breez_client.clone());
                self.flow_state = ActiveSettingsFlowState::MainMenu { backed_up, mfa };
            }
            _ => {}
        }
        Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        // Active wallet doesn't use Vault wallet - uses BreezClient instead
        Task::none()
    }
}

/// Fetches the main menu state (backed_up, mfa) from settings file.
/// Uses spawn_blocking to avoid blocking the async runtime if file I/O hangs.
fn fetch_main_menu_state(breez_client: Arc<BreezClient>) -> (bool, bool) {
    // Run blocking I/O in a blocking context to prevent hanging the async runtime
    tokio::task::block_in_place(|| {
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
                    Err(e) => {
                        tracing::warn!("Failed to read settings file: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to get CoincubeDirectory: {}", e);
            }
        }
        (backed_up, mfa)
    })
}
