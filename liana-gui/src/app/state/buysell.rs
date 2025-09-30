use iced::Task;
use std::sync::Arc;

use crate::services::mavapay::{BankAccount, Beneficiary};

use iced_webview::{
    advanced::{Action as WebviewAction, WebView},
    PageType,
};
use liana_ui::widget::Element;

use crate::app::buysell::{meld::MeldError, onramper, ServiceProvider};
use crate::app::view::buysell::{BuySellFlowState, NativePage, InternationalProvider};


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

#[derive(Debug, Clone)]
pub enum WebviewMessage {
    Action(iced_webview::advanced::Action),
    Created(iced_webview::ViewId),
}

/// Map webview messages to main app messages (static version for Task::map)
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

        fn reload(
            &mut self,
            _daemon: Arc<dyn Daemon + Sync + Send>,
            _wallet: Arc<crate::app::wallet::Wallet>,
        ) -> Task<Message> {
            let locator = crate::services::geolocation::CachedGeoLocator::new_from_env();
            Task::perform(
                async move { locator.detect_region().await },
                |result| match result {
                    Ok((region, country)) => {
                        let region_str = match region {
                            crate::services::geolocation::Region::Africa => "africa".to_string(),
                            crate::services::geolocation::Region::International => {
                                "international".to_string()
                            }
                        };
                        Message::View(ViewMessage::BuySell(BuySellMessage::RegionDetected(
                            region_str,
                            country,
                        )))
                    }
                    Err(error) => Message::View(ViewMessage::BuySell(
                        BuySellMessage::RegionDetectionError(error),
                    )),
                },
            )
        }


    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        // Handle global navigation for native flow (Previous)
        if let Message::View(ViewMessage::Previous) = &message {
            if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                match state.native_page {
                    NativePage::Register => state.native_page = NativePage::AccountSelect,
                    NativePage::VerifyEmail => state.native_page = NativePage::Register,
                    _ => {}
                }
            }
            return Task::none();
        }

        let Message::View(ViewMessage::BuySell(message)) = message else {
            return Task::none();
        };

        match message {
            BuySellMessage::LoginUsernameChanged(v) => {
                self.set_login_username(v);
            }
            BuySellMessage::LoginPasswordChanged(v) => {
                self.set_login_password(v);
            }
            BuySellMessage::SubmitLogin => {
                return self.handle_native_login();
            }
            BuySellMessage::CreateAccountPressed => {
                // Navigate to registration page
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.native_page = NativePage::Register;
                } else {
                    self.set_error("Create Account not implemented yet".to_string());
                }
            }
            BuySellMessage::WalletAddressChanged(address) => {
                self.set_wallet_address(address);
            }
            BuySellMessage::AccountTypeSelected(t) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.selected_account_type = Some(t);
                }
            }
            BuySellMessage::GetStarted => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    if state.selected_account_type.is_some() {
                        // Navigate to login page (native flow)
                        state.native_page = NativePage::Login;
                    }
                }
            }
            BuySellMessage::FirstNameChanged(v) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.first_name.value = v;
                    state.first_name.valid = !state.first_name.value.is_empty();
                }
            }
            BuySellMessage::DetectRegion => {
                // Detection is automatically triggered by reload(); nothing to do here
            }
            BuySellMessage::RegionDetected(region, country) => {
                // Do not log IP addresses. Region/country are fine.
                tracing::info!("region = {}, country = {}", region, country);
                self.handle_region_detected(&region, country);
            }
            BuySellMessage::RegionDetectionError(_error) => {
                // Graceful fallback: show provider selection, no blocking error
                self.region_detection_failed = true;
                self.flow_state = BuySellFlowState::DetectionFailed;
                self.error = None;
            }

            BuySellMessage::LastNameChanged(v) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.last_name.value = v;
                    state.last_name.valid = !state.last_name.value.is_empty();
                }
            }
            BuySellMessage::EmailChanged(v) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.email.value = v;
                    state.email.valid = state.email.value.contains('@') && state.email.value.contains('.')
                }
            }
            BuySellMessage::Password1Changed(v) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.password1.value = v;
                    state.password1.valid = Self::is_password_valid_static(&state.password1.value);
                }
            }
            BuySellMessage::Password2Changed(v) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.password2.value = v;
                    state.password2.valid = state.password2.value == state.password1.value
                        && !state.password2.value.is_empty();
                }
            }
            BuySellMessage::TermsToggled(b) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.terms_accepted = b;
                }
            }
            BuySellMessage::SubmitRegistration => {
                if let BuySellFlowState::Africa(ref state) = self.flow_state {
                    tracing::info!("🔍 [REGISTRATION] Submit registration button clicked");

                    if Self::is_registration_valid_static(state) {
                        tracing::info!(
                            "✅ [REGISTRATION] Form validation passed, submitting registration"
                        );
                        let client = state.registration_client.clone();
                        let account_type = if state.selected_account_type
                            == Some(crate::app::view::AccountType::Individual)
                        {
                            "personal"
                        } else {
                            "business"
                        }
                        .to_string();

                        let email = state.email.value.clone();
                        let first_name = state.first_name.value.clone();
                        let last_name = state.last_name.value.clone();
                        let password = state.password1.value.clone();

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
                            state.first_name.value,
                            !state.first_name.value.is_empty()
                        );
                        tracing::warn!(
                            "   - Last name: '{}' (valid: {})",
                            state.last_name.value,
                            !state.last_name.value.is_empty()
                        );
                        tracing::warn!(
                            "   - Email: '{}' (valid: {})",
                            state.email.value,
                            state.email.value.contains('@') && state.email.value.contains('.')
                        );
                        tracing::warn!(
                            "   - Password length: {} (valid: {})",
                            state.password1.value.len(),
                            state.password1.value.len() >= 8
                        );
                        tracing::warn!(
                            "   - Passwords match: {}",
                            state.password1.value == state.password2.value
                        );
                        tracing::warn!("   - Terms accepted: {}", state.terms_accepted);
                    }
                }
            }
            BuySellMessage::RegistrationSuccess => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    // Registration successful, navigate to email verification
                    state.native_page = NativePage::VerifyEmail;
                    state.email_verification_status = Some(false); // pending verification
                    self.error = None;
                }
            }
            BuySellMessage::RegistrationError(error) => {
                self.error = Some(format!("Registration failed: {}", error));
            }
            BuySellMessage::CheckEmailVerificationStatus => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    tracing::info!(
                        "🔍 [EMAIL_VERIFICATION] Checking email verification status for: {}",
                        state.email.value
                    );
                    // Set to "checking" state
                    state.email_verification_status = None;
                    let client = state.registration_client.clone();
                    let email = state.email.value.clone();

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
            }
            BuySellMessage::EmailVerificationStatusChecked(verified) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.email_verification_status = Some(verified);
                    if verified {
                        tracing::info!(
                            "✅ [EMAIL_VERIFICATION] Email verified, navigating to Mavapay dashboard"
                        );
                        state.native_page = NativePage::CoincubePay;
                        self.error = None;
                        // Automatically get current price when entering dashboard
                        return Task::done(Message::View(ViewMessage::BuySell(
                            BuySellMessage::MavapayGetPrice,
                        )));
                    } else {
                        self.error = None;
                    }
                }
            }
            BuySellMessage::EmailVerificationStatusError(error) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.email_verification_status = Some(false); // fallback to pending
                    self.error = Some(format!("Error checking verification status: {}", error));
                }
            }
            BuySellMessage::ResendVerificationEmail => {
                if let BuySellFlowState::Africa(ref state) = self.flow_state {
                    tracing::info!(
                        "📧 [RESEND_EMAIL] Resending verification email to: {}",
                        state.email.value
                    );
                    let client = state.registration_client.clone();
                    let email = state.email.value.clone();

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
            }
            BuySellMessage::ResendEmailSuccess => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.email_verification_status = Some(false); // back to pending
                    self.error = Some("Verification email resent successfully!".to_string());
                }
            }
            BuySellMessage::ResendEmailError(error) => {
                self.error = Some(format!("Error resending email: {}", error));
            }
            BuySellMessage::LoginSuccess(response) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    tracing::info!("✅ [LOGIN] Login successful, checking email verification status");
                    self.error = None;

                    // Check if 2FA is required
                    if response.requires_two_factor {
                        tracing::info!("⚠️ [LOGIN] 2FA required but not implemented yet");
                        self.set_error(
                            "Two-factor authentication required but not yet supported.".to_string(),
                        );
                        return Task::none();
                    }

                    // Check if we have user data and token
                    if let (Some(user), Some(_token)) = (&response.user, &response.token) {
                        // Check if email is verified and route accordingly
                        if user.email_verified {
                            tracing::info!(
                                "✅ [LOGIN] Email verified, navigating to Mavapay dashboard"
                            );
                            state.native_page = NativePage::CoincubePay;
                            // Automatically get current price when entering dashboard
                            return Task::done(Message::View(ViewMessage::BuySell(
                                BuySellMessage::MavapayGetPrice,
                            )));
                        } else {
                            tracing::info!(
                                "⚠️ [LOGIN] Email not verified, redirecting to verification page"
                            );
                            // Store the email for verification
                            state.email.value = user.email.clone();
                            state.native_page = NativePage::VerifyEmail;
                        }
                    } else {
                        tracing::error!("❌ [LOGIN] Login response missing user data or token");
                        self.set_error("Login failed: Invalid response from server".to_string());
                    }
                }
            }
            BuySellMessage::LoginError(err) => {
                tracing::error!("❌ [LOGIN] Login failed: {}", err);
                self.set_error(format!("Login failed: {}", err));
            }

            BuySellMessage::SourceAmountChanged(amount) => {
                self.set_source_amount(amount);
            }
            BuySellMessage::CountryCodeChanged(_)
            | BuySellMessage::FiatCurrencyChanged(_) => {
                // Legacy inputs retained for backward compatibility; runtime flow now uses
                // geolocation-derived country and provider defaults. No-op in new architecture.
            }

            // International provider handlers
            BuySellMessage::OpenOnramper => {
                if let BuySellFlowState::International(ref mut state) = self.flow_state {
                    state.selected_provider = Some(InternationalProvider::Onramper);
                    // Build Onramper widget URL and open in embedded webview
                    let currency = "USD".to_string();
                    let amount = if self.source_amount.value.is_empty() {
                        "50".to_string()
                    } else {
                        self.source_amount.value.clone()
                    };
                    let wallet = self.wallet_address.value.clone();
                    if let Some(url) = onramper::create_widget_url(&currency, &amount, &wallet) {
                        return Task::done(Message::View(ViewMessage::BuySell(
                            BuySellMessage::WebviewOpenUrl(url),
                        )));
                    } else {
                        self.set_error("Onramper API key not configured".to_string());
                    }
                }
            }
            BuySellMessage::OpenMeld => {
                if let BuySellFlowState::International(ref mut state) = self.flow_state {
                    state.selected_provider = Some(InternationalProvider::Meld);
                    // Create Meld widget session via API and open in embedded webview
                    let wallet_address = self.wallet_address.value.clone();
                    let country_code = self
                        .detected_country
                        .clone()
                        .unwrap_or_else(|| "US".to_string());
                    let source_amount = if self.source_amount.value.is_empty() {
                        "50".to_string()
                    } else {
                        self.source_amount.value.clone()
                    };
                    let network = self.network;
                    let client = state.meld_client.clone();
                    return Task::perform(
                        async move {
                            client
                                .create_widget_session(
                                    wallet_address,
                                    country_code,
                                    source_amount,
                                    ServiceProvider::Transak,
                                    network,
                                )
                                .await
                                .map(|resp| resp.widget_url)
                                .map_err(|e| format!("{}", e))
                        },
                        |result| match result {
                            Ok(widget_url) => Message::View(ViewMessage::BuySell(
                                BuySellMessage::WebviewOpenUrl(widget_url),
                            )),
                            Err(error) => Message::View(ViewMessage::BuySell(
                                BuySellMessage::SessionError(error),
                            )),
                        },
                    );
                }
            }

            BuySellMessage::MavapayDashboard => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.native_page = NativePage::CoincubePay;
                }
            }
            BuySellMessage::MavapayAmountChanged(amount) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_amount.value = amount;
                    state.mavapay_amount.valid = !state.mavapay_amount.value.is_empty()
                        && state.mavapay_amount.value.parse::<u64>().is_ok();
                }
            }
            BuySellMessage::MavapaySourceCurrencyChanged(currency) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_source_currency.value = currency;
                    state.mavapay_source_currency.valid = !state.mavapay_source_currency.value.is_empty();
                }
            }
            BuySellMessage::MavapayTargetCurrencyChanged(currency) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_target_currency.value = currency;
                    state.mavapay_target_currency.valid = !state.mavapay_target_currency.value.is_empty();
                }
            }
            BuySellMessage::MavapayBankAccountNumberChanged(account) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_bank_account_number.value = account;
                    state.mavapay_bank_account_number.valid =
                        !state.mavapay_bank_account_number.value.is_empty();
                }
            }
            BuySellMessage::MavapayBankAccountNameChanged(name) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_bank_account_name.value = name;
                    state.mavapay_bank_account_name.valid =
                        !state.mavapay_bank_account_name.value.is_empty();
                }
            }
            BuySellMessage::MavapayBankCodeChanged(code) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_bank_code.value = code;
                    state.mavapay_bank_code.valid = !state.mavapay_bank_code.value.is_empty();
                }
            }
            BuySellMessage::MavapayBankNameChanged(name) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_bank_name.value = name;
                    state.mavapay_bank_name.valid = !state.mavapay_bank_name.value.is_empty();
                }
            }
            BuySellMessage::MavapayCreateQuote => {
                return self.handle_mavapay_create_quote();
            }
            BuySellMessage::MavapayQuoteCreated(quote) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_current_quote = Some(quote);
                    self.error = None;
                }
            }
            BuySellMessage::MavapayQuoteError(error) => {
                self.error = Some(format!("Quote error: {}", error));
            }
            BuySellMessage::MavapayConfirmQuote => {
                // TODO: Implement quote confirmation
                self.error = Some("Quote confirmation not yet implemented".to_string());
            }
            BuySellMessage::MavapayGetPrice => {
                return self.handle_mavapay_get_price();
            }
            BuySellMessage::MavapayPriceReceived(price) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_current_price = Some(price);
                    self.error = None;
                }
            }
            BuySellMessage::MavapayPriceError(error) => {
                self.error = Some(format!("Price error: {}", error));
            }
            BuySellMessage::MavapayGetTransactions => {
                return self.handle_mavapay_get_transactions();
            }
            BuySellMessage::MavapayTransactionsReceived(transactions) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_transactions = transactions;
                    self.error = None;
                }
            }
            BuySellMessage::MavapayTransactionsError(error) => {
                self.error = Some(format!("Transactions error: {}", error));
            }
            BuySellMessage::MavapayConfirmPayment(quote_id) => {
                return self.handle_mavapay_confirm_payment(quote_id);
            }
            BuySellMessage::MavapayPaymentConfirmed(status) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_payment_status = Some(status);
                    self.error = None;
                    // Start polling for status updates
                    let quote_id = state
                        .mavapay_payment_status
                        .as_ref()
                        .unwrap()
                        .quote_id
                        .clone();
                    return Task::done(Message::View(ViewMessage::BuySell(
                        BuySellMessage::MavapayStartPolling(quote_id),
                    )));
                }
            }
            BuySellMessage::MavapayPaymentConfirmationError(error) => {
                self.error = Some(format!("Payment confirmation error: {}", error));
            }
            BuySellMessage::MavapayCheckPaymentStatus(quote_id) => {
                return self.handle_mavapay_check_payment_status(quote_id);
            }
            BuySellMessage::MavapayPaymentStatusUpdated(status) => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_payment_status = Some(status);
                    self.error = None;
                }
            }
            BuySellMessage::MavapayPaymentStatusError(error) => {
                self.error = Some(format!("Payment status error: {}", error));
            }
            BuySellMessage::MavapayStartPolling(quote_id) => {
                return self.handle_mavapay_start_polling(quote_id);
            }
            BuySellMessage::MavapayStopPolling => {
                if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
                    state.mavapay_polling_active = false;
                }
            }

            BuySellMessage::MavapayOpenPaymentLink => {
                return self.handle_mavapay_open_payment_link();
            }

            BuySellMessage::CreateSession => {
                // Legacy handler - now use OpenMeld/OpenOnramper instead
                self.set_error("Please select a provider (Meld or Onramper)".into());
            }

            // Legacy Meld session handler removed after runtime flow-state migration
            // BuySellMessage::_LegacyMeldSession => { /* removed */ }
            BuySellMessage::SessionError(error) => {
                self.set_error(error);
            }

            // webview logic
            BuySellMessage::ViewTick(id) => {
                let action = WebviewAction::Update(id);
                return self
                    .webview
                    .get_or_insert_with(init_webview)
                    .update(action)
                    .map(map_webview_message_static);
            }
            BuySellMessage::WebviewAction(action) => {
                return self
                    .webview
                    .get_or_insert_with(init_webview)
                    .update(action)
                    .map(map_webview_message_static);
            }
            BuySellMessage::WebviewOpenUrl(url) => {
                // Load URL into Ultralight webview
                #[cfg(debug_assertions)]
                tracing::info!("🌐 [LIANA] Loading Ultralight webview with URL: {}", url);
                #[cfg(not(debug_assertions))]
                tracing::info!("🌐 [LIANA] Opening embedded webview");
                self.session_url = Some(url.clone());

                // Create webview with URL string and immediately update to ensure content loads
                return self
                    .webview
                    .get_or_insert_with(init_webview)
                    .update(WebviewAction::CreateView(PageType::Url(url)))
                    .map(map_webview_message_static);
            }
            BuySellMessage::WebviewCreated(id) => {
                tracing::info!("🌐 [LIANA] Activating Webview Page: {}", id);

                // set active page to selected view id
                self.active_page = Some(id);
            }
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
        Task::done(Message::View(ViewMessage::BuySell(
            BuySellMessage::CloseWebview,
        )))
    }

    fn subscription(&self) -> iced::Subscription<Message> {
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

        iced::Subscription::none()
    }
}

impl BuySellPanel {
    pub fn handle_native_login(&mut self) -> Task<Message> {
        if !self.is_login_form_valid() {
            self.set_error("Please enter email and password".into());
            return Task::none();
        }

        if let BuySellFlowState::Africa(ref state) = self.flow_state {
            self.error = None;
            tracing::info!(
                "🔍 [LOGIN] Attempting login for user: {}",
                state.login_username.value
            );

            let client = state.registration_client.clone();
            let email = state.login_username.value.clone();
            let password = state.login_password.value.clone();

            return Task::perform(
                async move {
                    match client.login(&email, &password).await {
                        Ok(response) => {
                            tracing::info!("✅ [LOGIN] Login successful for user: {}", email);
                            Message::View(ViewMessage::BuySell(BuySellMessage::LoginSuccess(response)))
                        }
                        Err(e) => {
                            tracing::error!("❌ [LOGIN] Login failed for user {}: {}", email, e);
                            Message::View(ViewMessage::BuySell(BuySellMessage::LoginError(
                                e.to_string(),
                            )))
                        }
                    }
                },
                |msg| msg,
            );
        }
        Task::none()
    }

    fn handle_mavapay_create_quote(&self) -> Task<Message> {
        use crate::services::mavapay::{Currency, PaymentMethod, QuoteRequest};

        let Some(state) = self.africa_state() else {
            return Task::none();
        };

        // Validate required bank account fields
        if state.mavapay_bank_account_number.value.is_empty() {
            return Task::done(Message::View(ViewMessage::BuySell(
                BuySellMessage::MavapayQuoteError("Bank account number is required".to_string()),
            )));
        }

        if state.mavapay_bank_account_name.value.is_empty() {
            return Task::done(Message::View(ViewMessage::BuySell(
                BuySellMessage::MavapayQuoteError("Bank account name is required".to_string()),
            )));
        }

        if state.mavapay_bank_code.value.is_empty() {
            return Task::done(Message::View(ViewMessage::BuySell(
                BuySellMessage::MavapayQuoteError("Bank code is required".to_string()),
            )));
        }

        if state.mavapay_bank_name.value.is_empty() {
            return Task::done(Message::View(ViewMessage::BuySell(
                BuySellMessage::MavapayQuoteError("Bank name is required".to_string()),
            )));
        }

        let amount = match state.mavapay_amount.value.parse::<u64>() {
            Ok(amt) => amt,
            Err(_) => {
                return Task::done(Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayQuoteError("Invalid amount".to_string()),
                )));
            }
        };

        let source_currency = match state.mavapay_source_currency.value.as_str() {
            "BTCSAT" => Currency::BitcoinSatoshi,
            "NGNKOBO" => Currency::NigerianNairaKobo,
            "ZARCENT" => Currency::SouthAfricanRandCent,
            "KESCENT" => Currency::KenyanShillingCent,
            _ => {
                return Task::done(Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayQuoteError("Invalid source currency".to_string()),
                )));
            }
        };

        let target_currency = match state.mavapay_target_currency.value.as_str() {
            "BTCSAT" => Currency::BitcoinSatoshi,
            "NGNKOBO" => Currency::NigerianNairaKobo,
            "ZARCENT" => Currency::SouthAfricanRandCent,
            "KESCENT" => Currency::KenyanShillingCent,
            _ => {
                return Task::done(Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayQuoteError("Invalid target currency".to_string()),
                )));
            }
        };

        let request = QuoteRequest {
            amount: amount.to_string(),
            source_currency,
            target_currency,
            payment_method: PaymentMethod::Lightning, // Default to Lightning
            payment_currency: target_currency,
            autopayout: true,
            customer_internal_fee: "0".to_string(),
            beneficiary: Beneficiary::Bank(BankAccount {
                account_number: state.mavapay_bank_account_number.value.clone(),
                account_name: state.mavapay_bank_account_name.value.clone(),
                bank_code: state.mavapay_bank_code.value.clone(),
                bank_name: state.mavapay_bank_name.value.clone(),
            }),
        };

        let client = state.mavapay_client.clone();
        Task::perform(
            async move { client.create_quote(request).await },
            |result| match result {
                Ok(quote) => Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayQuoteCreated(quote),
                )),
                Err(error) => Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayQuoteError(error.to_string()),
                )),
            },
        )
    }
    fn handle_mavapay_open_payment_link(&self) -> Task<Message> {
        use crate::services::mavapay::{Currency, PaymentLinkRequest, PaymentMethod};

        let Some(state) = self.africa_state() else {
            return Task::none();
        };

        // Validate minimally required fields (reuse validations from quote creation)
        if state.mavapay_bank_account_number.value.is_empty()
            || state.mavapay_bank_account_name.value.is_empty()
            || state.mavapay_bank_code.value.is_empty()
            || state.mavapay_bank_name.value.is_empty()
        {
            return Task::done(Message::View(ViewMessage::BuySell(
                BuySellMessage::MavapayQuoteError(
                    "Missing required bank account details for payment link".to_string(),
                ),
            )));
        }

        let amount = match state.mavapay_amount.value.parse::<u64>() {
            Ok(amt) => amt,
            Err(_) => {
                return Task::done(Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayQuoteError("Invalid amount".to_string()),
                )));
            }
        };

        let source_currency = match state.mavapay_source_currency.value.as_str() {
            "BTCSAT" => Currency::BitcoinSatoshi,
            "NGNKOBO" => Currency::NigerianNairaKobo,
            "ZARCENT" => Currency::SouthAfricanRandCent,
            "KESCENT" => Currency::KenyanShillingCent,
            _ => {
                return Task::done(Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayQuoteError("Invalid source currency".to_string()),
                )));
            }
        };

        let target_currency = match state.mavapay_target_currency.value.as_str() {
            "BTCSAT" => Currency::BitcoinSatoshi,
            "NGNKOBO" => Currency::NigerianNairaKobo,
            "ZARCENT" => Currency::SouthAfricanRandCent,
            "KESCENT" => Currency::KenyanShillingCent,
            _ => {
                return Task::done(Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayQuoteError("Invalid target currency".to_string()),
                )));
            }
        };

        let request = PaymentLinkRequest {
            amount: amount.to_string(),
            source_currency,
            target_currency,
            payment_method: PaymentMethod::Lightning,
            payment_currency: target_currency,
            beneficiary: crate::services::mavapay::Beneficiary::Bank(
                crate::services::mavapay::BankAccount {
                    account_number: state.mavapay_bank_account_number.value.clone(),
                    account_name: state.mavapay_bank_account_name.value.clone(),
                    bank_code: state.mavapay_bank_code.value.clone(),
                    bank_name: state.mavapay_bank_name.value.clone(),
                },
            ),
        };

        let client = state.mavapay_client.clone();
        #[cfg(debug_assertions)]
        tracing::info!(
            "[PAYMENT_LINK] Creating link with {}/{} amount {}",
            request.source_currency.as_str(),
            request.target_currency.as_str(),
            request.amount
        );
        Task::perform(
            async move { client.create_payment_link(request).await },
            |result| match result {
                Ok(url) => Message::View(ViewMessage::BuySell(BuySellMessage::WebviewOpenUrl(url))),
                Err(error) => Message::View(ViewMessage::BuySell(BuySellMessage::MavapayQuoteError(
                    format!("Payment link error: {}", error)
                ))),
            },
        )
    }


    fn handle_mavapay_get_price(&self) -> Task<Message> {
        let Some(state) = self.africa_state() else {
            return Task::none();
        };
        let client = state.mavapay_client.clone();
        Task::perform(
            async move {
                client.get_price("NGN").await // Default to Nigerian Naira
            },
            |result| match result {
                Ok(price) => Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayPriceReceived(price),
                )),
                Err(error) => Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayPriceError(error.to_string()),
                )),
            },
        )
    }

    fn handle_mavapay_get_transactions(&self) -> Task<Message> {
        let Some(state) = self.africa_state() else {
            return Task::none();
        };
        let client = state.mavapay_client.clone();
        Task::perform(
            async move {
                client.get_transactions(Some(1), Some(10), None).await // Get first 10 transactions
            },
            |result| match result {
                Ok(transactions) => Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayTransactionsReceived(transactions),
                )),
                Err(error) => Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayTransactionsError(error.to_string()),
                )),
            },
        )
    }

    fn handle_mavapay_confirm_payment(&self, quote_id: String) -> Task<Message> {
        let Some(state) = self.africa_state() else {
            return Task::none();
        };
        let client = state.mavapay_client.clone();
        Task::perform(
            async move { client.confirm_quote(quote_id).await },
            |result| match result {
                Ok(status) => Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayPaymentConfirmed(status),
                )),
                Err(error) => Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayPaymentConfirmationError(error.to_string()),
                )),
            },
        )
    }

    fn handle_mavapay_check_payment_status(&self, quote_id: String) -> Task<Message> {
        let Some(state) = self.africa_state() else {
            return Task::none();
        };
        let client = state.mavapay_client.clone();
        Task::perform(
            async move { client.get_payment_status(&quote_id).await },
            |result| match result {
                Ok(status) => Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayPaymentStatusUpdated(status),
                )),
                Err(error) => Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayPaymentStatusError(error.to_string()),
                )),
            },
        )
    }

    fn handle_mavapay_start_polling(&self, quote_id: String) -> Task<Message> {
        let Some(state) = self.africa_state() else {
            return Task::none();
        };
        let client = state.mavapay_client.clone();
        Task::perform(
            async move {
                // Poll every 5 seconds for up to 20 attempts (100 seconds total)
                client.poll_transaction_status(&quote_id, 20, 5).await
            },
            |result| match result {
                Ok(status) => Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayPaymentStatusUpdated(status),
                )),
                Err(error) => Message::View(ViewMessage::BuySell(
                    BuySellMessage::MavapayPaymentStatusError(error.to_string()),
                )),
            },
        )
    }

    // Static helper methods for validation
    fn is_password_valid_static(password: &str) -> bool {
        password.len() >= 8
    }

    fn is_registration_valid_static(state: &crate::app::view::buysell::AfricaFlowState) -> bool {
        !state.first_name.value.is_empty()
            && !state.last_name.value.is_empty()
            && state.email.value.contains('@')
            && state.email.value.contains('.')
            && Self::is_password_valid_static(&state.password1.value)
            && state.password1.value == state.password2.value
            && state.terms_accepted
    }
}
