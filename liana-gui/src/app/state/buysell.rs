use iced::Task;
use std::sync::Arc;

#[cfg(feature = "webview")]
use iced_webview::{
    advanced::{Action as WebviewAction, WebView},
    PageType,
};
use liana_ui::widget::Element;

#[cfg(feature = "dev-meld")]
use crate::app::buysell::{meld::MeldError, ServiceProvider};

#[cfg(feature = "dev-onramp")]
use crate::app::buysell::onramper;

#[cfg(all(feature = "buysell", not(feature = "webview")))]
use crate::app::view::buysell::NativePage;

use crate::{
    app::{
        self,
        cache::Cache,
        message::Message,
        state::State,
        view::{self, buysell::BuySellPanel, BuySellMessage, Message as ViewMessage},
    },
    daemon::Daemon,
};

#[cfg(feature = "webview")]
#[derive(Debug, Clone)]
pub enum WebviewMessage {
    Action(iced_webview::advanced::Action),
    Created(iced_webview::ViewId),
}

/// Map webview messages to main app messages (static version for Task::map)
#[cfg(feature = "webview")]
fn map_webview_message_static(webview_msg: WebviewMessage) -> Message {
    match webview_msg {
        WebviewMessage::Action(action) => {
            Message::View(ViewMessage::BuySell(BuySellMessage::WebviewAction(action)))
        }
        WebviewMessage::Created(id) => {
            Message::View(ViewMessage::BuySell(BuySellMessage::WebviewCreated(id)))
        }
    }
}

/// lazily initialize the webview to reduce latent memory usage
#[cfg(feature = "webview")]
fn init_webview() -> WebView<iced_webview::Ultralight, WebviewMessage> {
    WebView::new().on_create_view(crate::app::state::buysell::WebviewMessage::Created)
}

impl Default for BuySellPanel {
    fn default() -> Self {
        Self::new(liana::miniscript::bitcoin::Network::Bitcoin)
    }
}

impl State for BuySellPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, ViewMessage> {
        // Return the meld view directly - dashboard wrapper will be applied by app/mod.rs
        view::dashboard(&app::Menu::BuySell, cache, None, self.view())
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let message = match message {
            // Handle global navigation for native flow (Previous)
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            Message::View(ViewMessage::Previous) => {
                match self.registration_state.native_page {
                    NativePage::Register => {
                        self.registration_state.native_page = NativePage::AccountSelect
                    }
                    NativePage::VerifyEmail => {
                        self.registration_state.native_page = NativePage::Register
                    }
                    _ => {}
                }

                return Task::none();
            }
            Message::View(ViewMessage::BuySell(message)) => message,
            _ => return Task::none(),
        };

        match message {
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::LoginUsernameChanged(v) => {
                self.set_login_username(v);
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::LoginPasswordChanged(v) => {
                self.set_login_password(v);
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::SubmitLogin => {
                return self.handle_native_login();
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::CreateAccountPressed => {
                self.set_error("Create Account not implemented yet".to_string());
            }
            BuySellMessage::WalletAddressChanged(address) => {
                self.set_wallet_address(address);
            }
            #[cfg(feature = "dev-meld")]
            BuySellMessage::CountryCodeChanged(code) => {
                self.set_country_code(code);
            }
            #[cfg(feature = "dev-onramp")]
            BuySellMessage::FiatCurrencyChanged(fiat) => {
                self.set_fiat_currency(fiat);
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::AccountTypeSelected(t) => {
                self.registration_state.selected_account_type = Some(t);
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::GetStarted => {
                if self.registration_state.selected_account_type.is_some() {
                    // Navigate to registration page (native flow)
                    self.registration_state.native_page = NativePage::Register;
                }
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::FirstNameChanged(v) => {
                self.registration_state.first_name.value = v;
                self.registration_state.first_name.valid =
                    !self.registration_state.first_name.value.is_empty();
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::LastNameChanged(v) => {
                self.registration_state.last_name.value = v;
                self.registration_state.last_name.valid =
                    !self.registration_state.last_name.value.is_empty();
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::EmailChanged(v) => {
                self.registration_state.email.value = v;
                self.registration_state.email.valid =
                    self.registration_state.email.value.contains('@')
                        && self.registration_state.email.value.contains('.')
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::Password1Changed(v) => {
                self.registration_state.password1.value = v;
                self.registration_state.password1.valid = self.is_password_valid();
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::Password2Changed(v) => {
                self.registration_state.password2.value = v;
                self.registration_state.password2.valid = self.registration_state.password2.value
                    == self.registration_state.password1.value
                    && !self.registration_state.password2.value.is_empty();
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::TermsToggled(b) => {
                self.registration_state.terms_accepted = b;
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::SubmitRegistration => {
                tracing::info!("🔍 [REGISTRATION] Submit registration button clicked");

                if self.is_registration_valid() {
                    tracing::info!(
                        "✅ [REGISTRATION] Form validation passed, submitting registration"
                    );
                    let client = self.registration_state.client.clone();
                    let account_type = if self.registration_state.selected_account_type
                        == Some(crate::app::view::AccountType::Individual)
                    {
                        "personal"
                    } else {
                        "business"
                    }
                    .to_string();

                    let email = self.registration_state.email.value.clone();
                    let first_name = self.registration_state.first_name.value.clone();
                    let last_name = self.registration_state.last_name.value.clone();
                    let password = self.registration_state.password1.value.clone();

                    tracing::info!(
                        "📤 [REGISTRATION] Making API call with account_type: {}, email: {}",
                        account_type,
                        email
                    );

                    return Task::perform(
                        async move {
                            let request = crate::services::registration::SignUpRequest {
                                account_type,
                                email,
                                first_name,
                                last_name,
                                auth_details: vec![crate::services::registration::AuthDetail {
                                    provider: 1, // EmailProvider = 1
                                    password,
                                }],
                            };

                            tracing::info!("🚀 [REGISTRATION] Sending request to API");
                            let result = client.sign_up(request).await;
                            tracing::info!(
                                "📥 [REGISTRATION] API response received: {:?}",
                                result.is_ok()
                            );
                            result
                        },
                        |result| match result {
                            Ok(_response) => {
                                tracing::info!("🎉 [REGISTRATION] Registration successful!");
                                // Registration successful, navigate to email verification
                                Message::View(ViewMessage::BuySell(
                                    BuySellMessage::RegistrationSuccess,
                                ))
                            }
                            Err(error) => {
                                tracing::error!(
                                    "❌ [REGISTRATION] Registration failed: {}",
                                    error.error
                                );
                                // Registration failed, show error
                                Message::View(ViewMessage::BuySell(
                                    BuySellMessage::RegistrationError(error.error),
                                ))
                            }
                        },
                    );
                } else {
                    tracing::warn!(
                        "⚠️ [REGISTRATION] Form validation failed - button should be disabled"
                    );
                    tracing::warn!(
                        "   - First name: '{}' (valid: {})",
                        self.registration_state.first_name.value,
                        !self.registration_state.first_name.value.is_empty()
                    );
                    tracing::warn!(
                        "   - Last name: '{}' (valid: {})",
                        self.registration_state.last_name.value,
                        !self.registration_state.last_name.value.is_empty()
                    );
                    tracing::warn!(
                        "   - Email: '{}' (valid: {})",
                        self.registration_state.email.value,
                        self.registration_state.email.value.contains('@')
                            && self.registration_state.email.value.contains('.')
                    );
                    tracing::warn!(
                        "   - Password length: {} (valid: {})",
                        self.registration_state.password1.value.len(),
                        self.registration_state.password1.value.len() >= 8
                    );
                    tracing::warn!(
                        "   - Passwords match: {}",
                        self.registration_state.password1.value
                            == self.registration_state.password2.value
                    );
                    tracing::warn!(
                        "   - Terms accepted: {}",
                        self.registration_state.terms_accepted
                    );
                }
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::RegistrationSuccess => {
                // Registration successful, navigate to email verification
                self.registration_state.native_page = NativePage::VerifyEmail;
                self.registration_state.email_verification_status = Some(false); // pending verification
                self.error = None;
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::RegistrationError(error) => {
                self.error = Some(format!("Registration failed: {}", error));
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::CheckEmailVerificationStatus => {
                tracing::info!(
                    "🔍 [EMAIL_VERIFICATION] Checking email verification status for: {}",
                    self.registration_state.email.value
                );
                // Set to "checking" state
                self.registration_state.email_verification_status = None;
                let client = self.registration_state.client.clone();
                let email = self.registration_state.email.value.clone();

                return Task::perform(
                    async move {
                        tracing::info!("🚀 [EMAIL_VERIFICATION] Making API call to check status");
                        let result = client.check_email_verification_status(&email).await;
                        tracing::info!(
                            "📥 [EMAIL_VERIFICATION] API response received: {:?}",
                            result.is_ok()
                        );
                        result
                    },
                    |result| match result {
                        Ok(response) => {
                            tracing::info!(
                                "✅ [EMAIL_VERIFICATION] Status check successful: verified={}",
                                response.email_verified
                            );
                            Message::View(ViewMessage::BuySell(
                                BuySellMessage::EmailVerificationStatusChecked(
                                    response.email_verified,
                                ),
                            ))
                        }
                        Err(error) => {
                            tracing::error!(
                                "❌ [EMAIL_VERIFICATION] Status check failed: {}",
                                error.error
                            );
                            Message::View(ViewMessage::BuySell(
                                BuySellMessage::EmailVerificationStatusError(error.error),
                            ))
                        }
                    },
                );
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::EmailVerificationStatusChecked(verified) => {
                self.registration_state.email_verification_status = Some(verified);
                if verified {
                    self.error = Some("Email verified successfully!".to_string());
                } else {
                    self.error = None;
                }
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::EmailVerificationStatusError(error) => {
                self.registration_state.email_verification_status = Some(false); // fallback to pending
                self.error = Some(format!("Error checking verification status: {}", error));
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::ResendVerificationEmail => {
                tracing::info!(
                    "📧 [RESEND_EMAIL] Resending verification email to: {}",
                    self.registration_state.email.value
                );
                let client = self.registration_state.client.clone();
                let email = self.registration_state.email.value.clone();

                return Task::perform(
                    async move {
                        tracing::info!("🚀 [RESEND_EMAIL] Making API call to resend email");
                        let result = client.resend_verification_email(&email).await;
                        tracing::info!(
                            "📥 [RESEND_EMAIL] API response received: {:?}",
                            result.is_ok()
                        );
                        result
                    },
                    |result| match result {
                        Ok(_response) => {
                            tracing::info!("✅ [RESEND_EMAIL] Email resent successfully");
                            Message::View(ViewMessage::BuySell(BuySellMessage::ResendEmailSuccess))
                        }
                        Err(error) => {
                            tracing::error!(
                                "❌ [RESEND_EMAIL] Failed to resend email: {}",
                                error.error
                            );
                            Message::View(ViewMessage::BuySell(BuySellMessage::ResendEmailError(
                                error.error,
                            )))
                        }
                    },
                );
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::ResendEmailSuccess => {
                self.registration_state.email_verification_status = Some(false); // back to pending
                self.error = Some("Verification email resent successfully!".to_string());
            }
            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::ResendEmailError(error) => {
                self.error = Some(format!("Error resending email: {}", error));
            }

            BuySellMessage::SourceAmountChanged(amount) => {
                self.set_source_amount(amount);
            }

            #[cfg(all(feature = "buysell", not(feature = "webview")))]
            BuySellMessage::CreateSession => {
                // No providers in default build; ignore or show error
                self.set_error("No provider configured in this build".into());
            }

            #[cfg(all(feature = "dev-onramp", not(feature = "dev-meld")))]
            BuySellMessage::CreateSession => {
                if self.is_form_valid() {
                    let Some(onramper_url) = onramper::create_widget_url(
                        &self.fiat_currency.value,
                        &self.source_amount.value,
                        &self.wallet_address.value,
                    ) else {
                        self.error = Some("Onramper API key not set as an environment variable (ONRAMPER_API_KEY) at compile time".to_string());
                        return Task::none();
                    };

                    tracing::info!(
                        "🚀 [BUYSELL] Creating new onramper widget session: {}",
                        &onramper_url
                    );

                    let open_webview = Message::View(ViewMessage::BuySell(
                        BuySellMessage::WebviewOpenUrl(onramper_url),
                    ));

                    return Task::done(open_webview);
                } else {
                    tracing::warn!("⚠️ [BUYSELL] Cannot create session - form validation failed");
                }
            }

            #[cfg(feature = "dev-meld")]
            BuySellMessage::CreateSession => {
                if self.is_form_valid() {
                    tracing::info!(
                        "🚀 [BUYSELL] Creating new session - clearing any existing session data"
                    );

                    // init session
                    let wallet_address = self.wallet_address.value.clone();
                    let country_code = self.country_code.value.clone();
                    let source_amount = self.source_amount.value.clone();

                    tracing::info!(
                        "🚀 [BUYSELL] Making fresh API call with: address={}, country={}, amount={}",
                        wallet_address,
                        country_code,
                        source_amount
                    );

                    return Task::perform(
                        {
                            // TODO: allow users to select source provider, in a drop down
                            let provider = ServiceProvider::Transak;
                            let network = self.network;
                            let client = self.meld_client.clone();

                            async move {
                                match client
                                    .create_widget_session(
                                        wallet_address,
                                        country_code,
                                        source_amount,
                                        provider,
                                        network,
                                    )
                                    .await
                                {
                                    Ok(response) => Ok(response.widget_url),
                                    Err(MeldError::Network(e)) => {
                                        Err(format!("Network error: {}", e))
                                    }
                                    Err(MeldError::Serialization(e)) => {
                                        Err(format!("Data error: {}", e))
                                    }
                                    Err(MeldError::Api(e)) => Err(format!("API error: {}", e)),
                                }
                            }
                        },
                        |result| match result {
                            Ok(widget_url) => {
                                tracing::info!(
                                    "🌐 [BUYSELL] Meld session created with URL: {}",
                                    widget_url
                                );

                                Message::View(ViewMessage::BuySell(BuySellMessage::WebviewOpenUrl(
                                    widget_url,
                                )))
                            }
                            Err(error) => {
                                tracing::error!("❌ [MELD] Session creation failed: {}", error);
                                Message::View(ViewMessage::BuySell(BuySellMessage::SessionError(
                                    error,
                                )))
                            }
                        },
                    );
                } else {
                    tracing::warn!("⚠️ [BUYSELL] Cannot create session - form validation failed");
                }
            }
            BuySellMessage::SessionError(error) => {
                self.set_error(error);
            }

            // webview logic
            #[cfg(feature = "webview")]
            BuySellMessage::ViewTick(id) => {
                let action = WebviewAction::Update(id);
                return self
                    .webview
                    .get_or_insert_with(init_webview)
                    .update(action)
                    .map(map_webview_message_static);
            }
            #[cfg(feature = "webview")]
            BuySellMessage::WebviewAction(action) => {
                return self
                    .webview
                    .get_or_insert_with(init_webview)
                    .update(action)
                    .map(map_webview_message_static);
            }
            #[cfg(feature = "webview")]
            BuySellMessage::WebviewOpenUrl(url) => {
                // Load URL into Ultralight webview
                tracing::info!("🌐 [LIANA] Loading Ultralight webview with URL: {}", url);
                self.session_url = Some(url.clone());

                // Create webview with URL string and immediately update to ensure content loads
                return self
                    .webview
                    .get_or_insert_with(init_webview)
                    .update(WebviewAction::CreateView(PageType::Url(url)))
                    .map(map_webview_message_static);
            }
            #[cfg(feature = "webview")]
            BuySellMessage::WebviewCreated(id) => {
                tracing::info!("🌐 [LIANA] Activating Webview Page: {}", id);

                // set active page to selected view id
                self.active_page = Some(id);
            }
            #[cfg(feature = "webview")]
            BuySellMessage::CloseWebview => {
                self.session_url = None;

                if let (Some(webview), Some(id)) = (self.webview.as_mut(), self.active_page.take())
                {
                    tracing::info!("🌐 [LIANA] Closing webview");
                    return webview
                        .update(WebviewAction::CloseView(id))
                        .map(map_webview_message_static);
                }
            }
        };

        Task::none()
    }

    fn close(&mut self) -> Task<Message> {
        #[cfg(feature = "webview")]
        {
            return Task::done(Message::View(ViewMessage::BuySell(
                BuySellMessage::CloseWebview,
            )));
        }
        #[cfg(not(feature = "webview"))]
        {
            Task::none()
        }
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        #[cfg(feature = "webview")]
        {
            use std::time::Duration;

            if let Some(id) = self.active_page {
                let interval = if cfg!(debug_assertions) {
                    Duration::from_millis(250)
                } else {
                    Duration::from_millis(100)
                };
                return iced::time::every(interval).with(id).map(|(i, ..)| {
                    Message::View(ViewMessage::BuySell(BuySellMessage::ViewTick(i)))
                });
            }
        }

        iced::Subscription::none()
    }
}

#[cfg(all(feature = "buysell", not(feature = "webview")))]
impl BuySellPanel {
    pub fn handle_native_login(&mut self) -> Task<Message> {
        if self.is_login_form_valid() {
            self.error = None;
        } else {
            self.set_error("Please enter username and password".into());
        }

        Task::none()
    }
}
