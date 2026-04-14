use std::sync::Arc;

use coincube_ui::widget::*;
use iced::Task;
use rand::seq::SliceRandom;

use crate::app::settings::{update_settings_file, Settings};
use crate::app::view::LiquidSettingsMessage;
use crate::app::{breez_liquid::BreezClient, cache::Cache, menu::Menu, state::State};
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
pub enum LiquidSettingsFlowState {
    MainMenu { backed_up: bool },
    BackupWallet(BackupWalletState),
}

/// LiquidSettings is a placeholder panel for the Liquid Settings page
pub struct LiquidSettings {
    breez_client: Arc<BreezClient>,
    flow_state: LiquidSettingsFlowState,
}

/// Generate 3 random unique word indices from 1 to mnemonic_len
/// Returns None if mnemonic_len < 3
fn generate_random_word_indices(mnemonic_len: usize) -> Option<[usize; 3]> {
    if mnemonic_len < 3 {
        return None;
    }
    let mut indices: Vec<usize> = (1..=mnemonic_len).collect();
    let mut rng = rand::thread_rng();
    indices.shuffle(&mut rng);
    Some([indices[0], indices[1], indices[2]])
}

impl LiquidSettings {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        let backed_up = fetch_main_menu_state(breez_client.clone());
        Self {
            breez_client,
            flow_state: LiquidSettingsFlowState::MainMenu { backed_up },
        }
    }
}

impl State for LiquidSettings {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            menu,
            cache,
            view::liquid::liquid_settings_view(self.breez_client.liquid_signer(), &self.flow_state),
        )
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::LiquidSettings(LiquidSettingsMessage::BackupWallet(
                backup_msg,
            ))) => {
                match backup_msg {
                    view::BackupWalletMessage::ToggleBackupIntroCheck => {
                        if let LiquidSettingsFlowState::BackupWallet(BackupWalletState::Intro(
                            checked,
                        )) = self.flow_state
                        {
                            self.flow_state = LiquidSettingsFlowState::BackupWallet(
                                BackupWalletState::Intro(!checked),
                            );
                        }
                    }
                    view::BackupWalletMessage::Start => {
                        self.flow_state =
                            LiquidSettingsFlowState::BackupWallet(BackupWalletState::Intro(false));
                    }
                    view::BackupWalletMessage::NextStep => {
                        self.flow_state = match &self.flow_state {
                            LiquidSettingsFlowState::BackupWallet(BackupWalletState::Intro(
                                true,
                            )) => LiquidSettingsFlowState::BackupWallet(
                                BackupWalletState::RecoveryPhrase,
                            ),
                            LiquidSettingsFlowState::BackupWallet(
                                BackupWalletState::RecoveryPhrase,
                            ) => {
                                let Some(signer) = self.breez_client.liquid_signer() else {
                                    return Task::none();
                                };
                                let mnemonic = signer.lock().expect("Mutex Lock Poisoned").words();

                                match generate_random_word_indices(mnemonic.len()) {
                                    Some(word_indices) => LiquidSettingsFlowState::BackupWallet(
                                        BackupWalletState::Verification {
                                            word_indices,
                                            word_inputs: [
                                                String::new(),
                                                String::new(),
                                                String::new(),
                                            ],
                                            error: None,
                                        },
                                    ),
                                    None => {
                                        tracing::error!("Mnemonic has fewer than 3 words");
                                        self.flow_state.clone()
                                    }
                                }
                            }
                            _ => self.flow_state.clone(),
                        };
                    }
                    view::BackupWalletMessage::PreviousStep => {
                        let backed_up = fetch_main_menu_state(self.breez_client.clone());
                        self.flow_state = match &self.flow_state {
                            LiquidSettingsFlowState::BackupWallet(BackupWalletState::Intro(_)) => {
                                LiquidSettingsFlowState::MainMenu { backed_up }
                            }
                            LiquidSettingsFlowState::BackupWallet(
                                BackupWalletState::RecoveryPhrase,
                            ) => LiquidSettingsFlowState::BackupWallet(BackupWalletState::Intro(
                                false,
                            )),
                            LiquidSettingsFlowState::BackupWallet(
                                BackupWalletState::Verification { .. },
                            ) => LiquidSettingsFlowState::BackupWallet(
                                BackupWalletState::RecoveryPhrase,
                            ),
                            LiquidSettingsFlowState::BackupWallet(BackupWalletState::Completed) => {
                                LiquidSettingsFlowState::MainMenu { backed_up }
                            }
                            LiquidSettingsFlowState::MainMenu { backed_up } => {
                                LiquidSettingsFlowState::MainMenu {
                                    backed_up: *backed_up,
                                }
                            }
                        };
                    }
                    view::BackupWalletMessage::WordInput { index, input } => {
                        if let LiquidSettingsFlowState::BackupWallet(
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

                            self.flow_state = LiquidSettingsFlowState::BackupWallet(
                                BackupWalletState::Verification {
                                    word_indices: *word_indices,
                                    word_inputs: new_inputs,
                                    error: error.clone(),
                                },
                            );
                        }
                    }
                    view::BackupWalletMessage::VerifyPhrase => {
                        if let LiquidSettingsFlowState::BackupWallet(
                            BackupWalletState::Verification {
                                word_indices,
                                word_inputs,
                                ..
                            },
                        ) = &self.flow_state
                        {
                            // Get the actual mnemonic words
                            let Some(signer) = self.breez_client.liquid_signer() else {
                                return Task::none();
                            };
                            let mnemonic = signer.lock().expect("Mutex Lock Poisoned").words();

                            // Verify each word matches the correct position in the mnemonic
                            // word_indices are 1-based, mnemonic array is 0-based
                            let all_correct =
                                word_indices.iter().enumerate().all(|(i, &word_idx)| {
                                    if word_idx == 0 || word_idx > mnemonic.len() {
                                        tracing::error!("Invalid word index: {}", word_idx);
                                        return false;
                                    }
                                    word_inputs[i].trim() == mnemonic[word_idx - 1]
                                });

                            if all_correct {
                                // Verification successful
                                self.flow_state = LiquidSettingsFlowState::BackupWallet(
                                    BackupWalletState::Completed,
                                );
                            } else {
                                // Verification failed
                                self.flow_state = LiquidSettingsFlowState::BackupWallet(
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
                        let Some(signer) = breez_client.liquid_signer() else {
                            return Task::none();
                        };
                        return Task::perform(
                            async move {
                                let secp =
                                    coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
                                let fingerprint = signer
                                    .lock()
                                    .expect("Mutex Lock Poisoned")
                                    .fingerprint(&secp);

                                let dir = CoincubeDirectory::new_default().map_err(|e| {
                                    format!("Failed to get CoincubeDirectory: {}", e)
                                })?;

                                let network_dir = dir.network_directory(breez_client.network());
                                update_settings_file(&network_dir, |mut settings| {
                                    if let Some(cube) = settings.cubes.iter_mut().find(|cube| {
                                        cube.liquid_wallet_signer_fingerprint.as_ref()
                                            == Some(&fingerprint)
                                    }) {
                                        cube.backed_up = true;
                                    }
                                    Some(settings)
                                })
                                .await
                                .map_err(|e| format!("Failed to update settings file: {}", e))?;

                                Ok(())
                            },
                            |res| match res {
                                Ok(_) => Message::View(view::Message::LiquidSettings(
                                    view::LiquidSettingsMessage::SettingsUpdated,
                                )),
                                Err(e) => Message::View(view::Message::ShowError(e)),
                            },
                        );
                    }
                }
            }
            Message::View(view::Message::LiquidSettings(
                LiquidSettingsMessage::SettingsUpdated,
            )) => {
                // Settings file was updated, refresh the state
                let backed_up = fetch_main_menu_state(self.breez_client.clone());
                self.flow_state = LiquidSettingsFlowState::MainMenu { backed_up };
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
        // Reset to main menu when reloading (e.g., clicking Settings in breadcrumb)
        let backed_up = fetch_main_menu_state(self.breez_client.clone());
        self.flow_state = LiquidSettingsFlowState::MainMenu { backed_up };
        Task::none()
    }
}

/// Fetches the main menu state (backed_up) from settings file.
/// Uses spawn_blocking to avoid blocking the async runtime if file I/O hangs.
fn fetch_main_menu_state(breez_client: Arc<BreezClient>) -> bool {
    // Run blocking I/O in a blocking context to prevent hanging the async runtime
    tokio::task::block_in_place(|| {
        let mut backed_up = false;
        let Some(signer) = breez_client.liquid_signer() else {
            return backed_up;
        };
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
        let fingerprint = signer
            .lock()
            .expect("Mutex Lock Poisoned")
            .fingerprint(&secp);

        match CoincubeDirectory::new_default() {
            Ok(dir) => {
                let network_dir = dir.network_directory(breez_client.network());
                match Settings::from_file(&network_dir) {
                    Ok(settings) => {
                        let cube = settings.cubes.into_iter().find(|cube| {
                            cube.liquid_wallet_signer_fingerprint.as_ref() == Some(&fingerprint)
                        });
                        if let Some(cube) = cube {
                            backed_up = cube.backed_up;
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
        backed_up
    })
}
