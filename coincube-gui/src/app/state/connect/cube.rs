use std::collections::HashMap;
use std::sync::Arc;

use iced::task::Handle as TaskHandle;

use crate::{
    app::{
        breez_liquid::BreezClient,
        message::Message,
        view::{self, ConnectCubeMessage},
    },
    services::coincube::{
        AvatarGenerateRequest, AvatarSelectRequest, AvatarUserTraits, ClaimLightningAddressRequest,
        CoincubeClient, LightningAddress, RegisterCubeRequest,
    },
};

use super::AvatarFlowStep;

/// Per-Cube Connect panel handling Lightning Address and Avatar.
/// These features are tied to the Cube's Breez wallet (BOLT12 offer).
pub struct ConnectCubePanel {
    /// The Cube's client-side UUID (from CubeSettings.id)
    pub cube_uuid: String,
    /// The Cube's display name (for registration)
    pub cube_name: String,
    /// The Cube's network ("mainnet" or "testnet")
    pub cube_network: String,
    /// The server-side numeric ID — set after registering with the backend.
    /// Used in API paths: /connect/cubes/{server_cube_id}/...
    pub server_cube_id: Option<u64>,
    /// Set when the last cube registration attempt failed.
    pub registration_error: Option<String>,
    // Lightning Address
    pub lightning_address: Option<LightningAddress>,
    pub ln_username_input: String,
    pub ln_username_available: Option<bool>,
    pub ln_username_error: Option<String>,
    pub ln_claim_error: Option<String>,
    pub ln_claiming: bool,
    pub ln_checking: bool,
    ln_check_version: u32,
    ln_check_abort: Option<TaskHandle>,
    breez_client: Arc<BreezClient>,
    /// API client with JWT set — obtained from ConnectAccountPanel after login.
    pub client: Option<CoincubeClient>,
    // Avatar
    pub avatar_step: AvatarFlowStep,
    pub avatar_data: Option<crate::services::coincube::GetAvatarData>,
    pub avatar_generating: bool,
    pub avatar_error: Option<String>,
    pub avatar_image_cache: HashMap<u64, (Vec<u8>, iced::widget::image::Handle)>,
    pub avatar_draft: AvatarUserTraits,
}

impl ConnectCubePanel {
    pub fn new(
        breez_client: Arc<BreezClient>,
        cube_uuid: String,
        cube_name: String,
        cube_network: String,
    ) -> Self {
        ConnectCubePanel {
            cube_uuid,
            cube_name,
            cube_network,
            server_cube_id: None,
            registration_error: None,
            lightning_address: None,
            ln_username_input: String::new(),
            ln_username_available: None,
            ln_username_error: None,
            ln_claim_error: None,
            ln_claiming: false,
            ln_checking: false,
            ln_check_version: 0,
            ln_check_abort: None,
            breez_client,
            client: None,
            avatar_step: AvatarFlowStep::Idle,
            avatar_data: None,
            avatar_generating: false,
            avatar_error: None,
            avatar_image_cache: HashMap::new(),
            avatar_draft: AvatarUserTraits::default(),
        }
    }

    /// Set the authenticated API client (called after account login).
    pub fn set_client(&mut self, client: CoincubeClient) {
        self.client = Some(client);
    }

    /// Clear the API client and all session-scoped state (called on account logout).
    pub fn clear_client(&mut self) {
        self.client = None;
        self.server_cube_id = None;
        self.registration_error = None;
        self.lightning_address = None;
        self.ln_username_input.clear();
        self.ln_username_available = None;
        self.ln_username_error = None;
        self.ln_claim_error = None;
        self.ln_claiming = false;
        self.ln_checking = false;
        self.ln_check_version += 1;
        if let Some(handle) = self.ln_check_abort.take() {
            handle.abort();
        }
        self.avatar_step = AvatarFlowStep::Idle;
        self.avatar_data = None;
        self.avatar_generating = false;
        self.avatar_error = None;
        self.avatar_image_cache.clear();
        self.avatar_draft = AvatarUserTraits::default();
    }

    /// Returns the server-side cube ID as a string for API paths.
    fn api_cube_id(&self) -> Option<String> {
        self.server_cube_id.map(|id| id.to_string())
    }

    /// Register this cube with the backend. Called after login.
    /// Returns a task that sends CubeRegistered on completion.
    pub fn register_cube(&self) -> iced::Task<Message> {
        let Some(client) = self.client.clone() else {
            return iced::Task::none();
        };
        let req = RegisterCubeRequest {
            uuid: self.cube_uuid.clone(),
            name: self.cube_name.clone(),
            network: self.cube_network.clone(),
        };
        iced::Task::perform(async move { client.register_cube(req).await }, |res| {
            Message::View(view::Message::ConnectCube(
                ConnectCubeMessage::CubeRegistered(res.map_err(|e| e.to_string())),
            ))
        })
    }

    pub fn update_message(&mut self, msg: ConnectCubeMessage) -> iced::Task<Message> {
        match msg {
            ConnectCubeMessage::CubeRegistered(result) => {
                match result {
                    Ok(cube_resp) => {
                        log::info!(
                            "[CONNECT-CUBE] Registered cube {} (server ID: {})",
                            cube_resp.uuid,
                            cube_resp.id
                        );
                        self.server_cube_id = Some(cube_resp.id);
                        self.registration_error = None;
                        // Store the lightning address from the backend (or clear if None)
                        if cube_resp.lightning_address.is_some() {
                            self.lightning_address = Some(LightningAddress {
                                lightning_address: cube_resp.lightning_address,
                                bolt12_offer: cube_resp.bolt12_offer,
                            });
                        } else {
                            self.lightning_address = None;
                        }
                    }
                    Err(e) => {
                        log::error!("[CONNECT-CUBE] Failed to register cube: {}", e);
                        self.registration_error = Some(e);
                    }
                }
            }

            ConnectCubeMessage::LightningAddressLoaded(ln_addr) => {
                self.lightning_address = ln_addr;
            }

            ConnectCubeMessage::LnUsernameChanged(input) => {
                self.ln_username_input = input.to_lowercase();
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

                let Some(client) = self.client.clone() else {
                    self.ln_username_error = Some("Not signed in".to_string());
                    return iced::Task::none();
                };

                // Debounced availability check — abort any previous in-flight task
                if let Some(handle) = self.ln_check_abort.take() {
                    handle.abort();
                }
                self.ln_check_version += 1;
                let version = self.ln_check_version;
                let username = self.ln_username_input.clone();
                let Some(cube_id) = self.api_cube_id() else {
                    log::warn!("[CONNECT-CUBE] No server cube ID yet");
                    return iced::Task::none();
                };
                self.ln_checking = true;
                let (task, abort_handle) = iced::Task::perform(
                    async move {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        let res = client.check_lightning_address(&cube_id, &username).await;
                        (res, version)
                    },
                    move |(res, v)| match res {
                        Ok(check) => Message::View(view::Message::ConnectCube(
                            ConnectCubeMessage::LnUsernameChecked {
                                available: check.available,
                                error_message: check.error_message,
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
                return task;
            }

            ConnectCubeMessage::LnUsernameChecked {
                available,
                error_message,
                version,
            } => {
                if version == self.ln_check_version {
                    self.ln_checking = false;
                    self.ln_username_available = Some(available);
                    if !available {
                        self.ln_username_error =
                            Some(error_message.unwrap_or_else(|| "Username is taken".to_string()));
                    }
                }
            }

            ConnectCubeMessage::ClaimLightningAddress => {
                if self.ln_claiming {
                    return iced::Task::none();
                }
                let Some(client) = self.client.clone() else {
                    return iced::Task::none();
                };
                self.ln_claiming = true;
                self.ln_claim_error = None;
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
                let breez = self.breez_client.clone();
                return iced::Task::perform(
                    async move {
                        let bolt12_offer = breez.receive_bolt12_offer().await.map_err(|e| {
                            format!(
                                "Failed to generate BOLT12 offer. \
                                     The Lightning wallet may still be syncing. \
                                     Please try again in a moment. ({})",
                                e
                            )
                        })?;
                        client
                            .claim_lightning_address(
                                &cube_id,
                                ClaimLightningAddressRequest {
                                    username,
                                    bolt12_offer,
                                },
                            )
                            .await
                            .map_err(|e| e.to_string())
                    },
                    |res| match res {
                        Ok(ln_addr) => Message::View(view::Message::ConnectCube(
                            ConnectCubeMessage::LightningAddressClaimed(ln_addr),
                        )),
                        Err(e) => Message::View(view::Message::ConnectCube(
                            ConnectCubeMessage::Error(e.to_string()),
                        )),
                    },
                );
            }

            ConnectCubeMessage::LightningAddressClaimed(ln_addr) => {
                self.ln_claiming = false;
                self.lightning_address = Some(ln_addr);
                self.ln_username_input.clear();
                self.ln_username_available = None;
                self.ln_username_error = None;
            }

            ConnectCubeMessage::RetryRegistration => {
                self.registration_error = None;
                return self.register_cube();
            }

            ConnectCubeMessage::CopyToClipboard(text) => {
                return iced::clipboard::write(text);
            }

            ConnectCubeMessage::Error(e) => {
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

            ConnectCubeMessage::Avatar(avatar_msg) => {
                return self.update_avatar(avatar_msg);
            }
        }

        iced::Task::none()
    }

    fn update_avatar(&mut self, msg: crate::app::view::AvatarMessage) -> iced::Task<Message> {
        use crate::app::view::AvatarMessage;

        match msg {
            AvatarMessage::Enter => {
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
                return iced::Task::perform(async move { client.get_avatar(&cid).await }, |res| {
                    Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                        AvatarMessage::Loaded(res.map_err(|e| e.to_string())),
                    )))
                });
            }

            AvatarMessage::Loaded(result) => match result {
                Ok(data) => {
                    let has = data.has_avatar;
                    let active_id = data
                        .variants
                        .iter()
                        .find(|v| {
                            data.active_avatar_url
                                .as_deref()
                                .map(|u| u.ends_with(&v.id.to_string()))
                                .unwrap_or(false)
                        })
                        .map(|v| v.id);
                    self.avatar_data = Some(data);
                    if has {
                        self.avatar_step = AvatarFlowStep::Settings;
                        if let Some(id) = active_id {
                            if !self.avatar_image_cache.contains_key(&id) {
                                if let Some(client) = self.client.clone() {
                                    return iced::Task::perform(
                                        async move { client.fetch_avatar_image(id).await },
                                        move |res| {
                                            Message::View(view::Message::ConnectCube(
                                                ConnectCubeMessage::Avatar(
                                                    AvatarMessage::ImageLoaded {
                                                        variant_id: id,
                                                        result: res.map_err(|e| e.to_string()),
                                                    },
                                                ),
                                            ))
                                        },
                                    );
                                }
                            }
                        }
                    } else {
                        self.avatar_step = AvatarFlowStep::Questionnaire;
                    }
                }
                Err(e) => {
                    log::error!("[AVATAR] Load error: {}", e);
                    self.avatar_error = Some(e);
                }
            },

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

            AvatarMessage::Generate => {
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
                return iced::Task::perform(
                    async move { client.post_avatar_generate(&cid, req).await },
                    |res| {
                        Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                            AvatarMessage::GenerateComplete(res.map_err(|e| e.to_string())),
                        )))
                    },
                );
            }

            AvatarMessage::GenerateComplete(result) => {
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
                        // Fetch image + refresh regeneration count in parallel
                        if let Some(client) = self.client.clone() {
                            let client2 = client.clone();
                            let Some(cid) = self.api_cube_id() else {
                                return iced::Task::none();
                            };
                            return iced::Task::batch([
                                iced::Task::perform(
                                    async move { client.fetch_avatar_image(variant_id).await },
                                    move |res| {
                                        Message::View(view::Message::ConnectCube(
                                            ConnectCubeMessage::Avatar(
                                                AvatarMessage::ImageLoaded {
                                                    variant_id,
                                                    result: res.map_err(|e| e.to_string()),
                                                },
                                            ),
                                        ))
                                    },
                                ),
                                iced::Task::perform(
                                    async move { client2.get_avatar_regenerations(&cid).await },
                                    |res| {
                                        Message::View(view::Message::ConnectCube(
                                            ConnectCubeMessage::Avatar(
                                                AvatarMessage::RegenerationsLoaded(
                                                    res.map_err(|e| e.to_string()),
                                                ),
                                            ),
                                        ))
                                    },
                                ),
                            ]);
                        }
                    }
                    Err(e) => {
                        log::error!("[AVATAR] Generate error: {}", e);
                        self.avatar_error = Some(e);
                        self.avatar_step = AvatarFlowStep::Questionnaire;
                    }
                }
            }

            AvatarMessage::SelectVariant(variant_id) => {
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
                return iced::Task::perform(
                    async move {
                        client
                            .post_avatar_select(&cid, AvatarSelectRequest { variant_id })
                            .await
                    },
                    |res| {
                        Message::View(view::Message::ConnectCube(ConnectCubeMessage::Avatar(
                            AvatarMessage::VariantSelected(res.map_err(|e| e.to_string())),
                        )))
                    },
                );
            }

            AvatarMessage::VariantSelected(result) => match result {
                Ok(data) => {
                    if let Some(ref mut ad) = self.avatar_data {
                        ad.active_avatar_url = Some(data.active_avatar_url);
                    }
                    let variant_id = data.variant_id;
                    if !self.avatar_image_cache.contains_key(&variant_id) {
                        if let Some(client) = self.client.clone() {
                            return iced::Task::perform(
                                async move { client.fetch_avatar_image(variant_id).await },
                                move |res| {
                                    Message::View(view::Message::ConnectCube(
                                        ConnectCubeMessage::Avatar(AvatarMessage::ImageLoaded {
                                            variant_id,
                                            result: res.map_err(|e| e.to_string()),
                                        }),
                                    ))
                                },
                            );
                        }
                    }
                }
                Err(e) => {
                    log::error!("[AVATAR] Select error: {}", e);
                    self.avatar_error = Some(e);
                }
            },

            AvatarMessage::RegenerationsLoaded(result) => match result {
                Ok(data) => {
                    if let Some(ref mut ad) = self.avatar_data {
                        ad.regenerations_remaining = data.remaining;
                    }
                }
                Err(e) => {
                    log::warn!("[AVATAR] Regenerations fetch error: {}", e);
                }
            },

            AvatarMessage::ImageLoaded { variant_id, result } => match result {
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

            AvatarMessage::DownloadAvatar => {
                let active_id = self.avatar_data.as_ref().and_then(|d| {
                    let url = d.active_avatar_url.as_deref().unwrap_or("");
                    d.variants
                        .iter()
                        .find(|v| url.ends_with(&v.id.to_string()))
                        .map(|v| v.id)
                });
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
            }

            AvatarMessage::SaveError(e) => {
                log::error!("[AVATAR] Failed to save avatar to disk: {}", e);
                self.avatar_error = Some(e);
            }

            AvatarMessage::Noop => {}
        }

        iced::Task::none()
    }
}

/// Validate a lightning address username client-side.
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
    if !username.chars().next().unwrap().is_ascii_alphanumeric() {
        return Some("Must start with a letter or number".to_string());
    }
    if !username.chars().last().unwrap().is_ascii_alphanumeric() {
        return Some("Must end with a letter or number".to_string());
    }
    let special = ['.', '_', '-'];
    for c in username.chars() {
        if !c.is_ascii_alphanumeric() && !special.contains(&c) {
            return Some(format!("Invalid character: '{}'", c));
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
