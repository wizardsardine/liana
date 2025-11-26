use iced::Task;
use std::sync::Arc;

use liana_ui::widget::Element;

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        message::Message,
        state::State,
        view::{
            self,
            buysell::{
                flow_state::MavapayFlowMode, BuySellFlowState, BuySellPanel, MavapayFlowStep,
            },
            BuySellMessage, MavapayMessage, Message as ViewMessage,
        },
    },
    daemon::Daemon,
    services::mavapay::*,
};

impl State for BuySellPanel {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, ViewMessage> {
        let inner = view::dashboard(menu, cache, None, self.view());

        let overlay = match &self.modal {
            super::vault::receive::Modal::VerifyAddress(m) => m.view(),
            super::vault::receive::Modal::ShowQrCode(m) => m.view(),
            super::vault::receive::Modal::None => return inner,
        };

        liana_ui::widget::modal::Modal::new(inner, overlay)
            .on_blur(Some(ViewMessage::Close))
            .into()
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let message = match message {
            // modal for any generated address
            Message::View(ViewMessage::Select(_)) => {
                if let BuySellFlowState::AddressGeneration {
                    address: Some(la), ..
                } = &self.flow_state
                {
                    self.modal = super::vault::receive::Modal::VerifyAddress(
                        super::vault::receive::VerifyAddressModal::new(
                            cache.datadir_path.clone(),
                            self.wallet.clone(),
                            cache.network,
                            la.address.clone(),
                            la.index,
                        ),
                    );
                };

                return Task::none();
            }
            Message::View(ViewMessage::ShowQrCode(_)) => {
                if let BuySellFlowState::AddressGeneration {
                    address: Some(la), ..
                } = &self.flow_state
                {
                    if let Some(modal) =
                        super::vault::receive::ShowQrCodeModal::new(&la.address, la.index)
                    {
                        self.modal = super::vault::receive::Modal::ShowQrCode(modal);
                    }
                };

                return Task::none();
            }
            Message::View(ViewMessage::Close) => {
                self.modal = super::vault::receive::Modal::None;
                return Task::none();
            }
            Message::View(ViewMessage::BuySell(message)) => message,
            _ => return Task::none(),
        };

        match message {
            // internal state management
            BuySellMessage::ResetWidget => {
                let flow_state = match self.detected_country.as_ref().map(|c| c.code) {
                    Some(iso) if mavapay_supported(&iso) => {
                        BuySellFlowState::Mavapay(view::buysell::MavapayState::new())
                    }
                    _ => BuySellFlowState::AddressGeneration {
                        buy_or_sell: None,
                        data_dir: cache.datadir_path.clone(),
                        address: None,
                    },
                };

                self.flow_state = flow_state;
                self.error = None;
            }
            BuySellMessage::SetBuyOrSell(bs) => {
                if let BuySellFlowState::AddressGeneration { buy_or_sell, .. } =
                    &mut self.flow_state
                {
                    *buy_or_sell = Some(bs);
                }
            }

            // creates a new address for bitcoin deposit
            BuySellMessage::CreateNewAddress => {
                return Task::perform(
                    async move { daemon.get_new_address().await },
                    |res| match res {
                        Ok(out) => Message::View(ViewMessage::BuySell(
                            BuySellMessage::AddressCreated(view::buysell::panel::LabelledAddress {
                                address: out.address,
                                index: out.derivation_index,
                                label: None,
                            }),
                        )),
                        Err(err) => Message::View(ViewMessage::BuySell(
                            BuySellMessage::SessionError(err.to_string()),
                        )),
                    },
                )
            }
            BuySellMessage::AddressCreated(addresses) => {
                if let BuySellFlowState::AddressGeneration {
                    address: generated_address,
                    ..
                } = &mut self.flow_state
                {
                    *generated_address = Some(addresses);
                }
            }

            // ip-geolocation logic
            BuySellMessage::CountryDetected(result) => {
                use crate::app::view::buysell::MavapayState;

                let country = match result {
                    Ok(country) => {
                        self.error = None;
                        country
                    }
                    Err(err) => {
                        tracing::error!("Error detecting country via geo-ip, switching to manual country selector.\n    {}", err);

                        self.flow_state = BuySellFlowState::DetectingLocation(true);
                        self.detected_country = None;

                        return Task::done(Message::View(ViewMessage::BuySell(
                            BuySellMessage::SessionError("Unable to automatically determine location, please select manually below".to_string()),
                        )));
                    }
                };

                if mavapay_supported(&country.code) {
                    let mut mavapay = MavapayState::new();

                    // attempt automatic login from os-keyring
                    if let Ok(entry) = keyring::Entry::new("io.coincube.Vault", "vault") {
                        if let (Ok(token), Ok(user_data)) =
                            (entry.get_password(), entry.get_secret())
                        {
                            mavapay.auth_token = Some(token);
                            if let Ok(user) = serde_json::from_slice(&user_data) {
                                mavapay.current_user = Some(user);
                            }
                        };

                        if mavapay.current_user.is_some() && mavapay.auth_token.is_some() {
                            // TODO: check if auth credentials have expired
                            log::info!("Coincube session successfully restored from OS keyring");

                            mavapay.step = MavapayFlowStep::ActiveBuysell {
                                country: country.clone(),
                                flow_mode: MavapayFlowMode::CreateQuote,
                                amount: 60,
                                source_currency: None,
                                target_currency: None,
                                settlement_currency: None,
                                payment_method: MavapayPaymentMethod::Lightning,
                                bank_account_number: String::new(),
                                bank_account_name: String::new(),
                                bank_code: String::new(),
                                bank_name: String::new(),
                                current_quote: None,
                                current_price: None,
                            };
                        };
                    };

                    self.flow_state = BuySellFlowState::Mavapay(mavapay);
                } else {
                    self.flow_state = BuySellFlowState::AddressGeneration {
                        buy_or_sell: None,
                        data_dir: cache.datadir_path.clone(),
                        address: None,
                    };
                }

                // update location information
                tracing::info!("Country = {}, ISO = {}", country.name, country.code);
                self.detected_country = Some(country);
            }

            // session management
            BuySellMessage::StartOnramperSession => {
                return {
                    let BuySellFlowState::AddressGeneration {
                        buy_or_sell,
                        address: generated_address,
                        ..
                    } = &self.flow_state
                    else {
                        return Task::none();
                    };

                    let mode = match buy_or_sell {
                        Some(view::buysell::panel::BuyOrSell::Buy) => "buy",
                        Some(view::buysell::panel::BuyOrSell::Sell) => "sell",
                        None => return Task::none(),
                    };

                    let Some(iso_code) = self.detected_country.as_ref().map(|c| c.code) else {
                        tracing::warn!(
                            "Unable to start session, country selection|detection was unsuccessful"
                        );
                        return Task::none();
                    };

                    // This method is now only called for Onramper (non-Mavapay) flow
                    let Some(currency) = crate::services::coincube::get_countries()
                        .iter()
                        .find(|c| c.code == iso_code)
                        .map(|c| c.currency.name)
                    else {
                        tracing::error!("Unknown country iso code: {}", iso_code);
                        return Task::none();
                    };

                    // prepare parameters
                    let address = generated_address.as_ref().map(|a| a.address.to_string());

                    match crate::app::buysell::onramper::create_widget_url(
                        &currency,
                        address.as_deref(),
                        &mode,
                        self.network,
                    ) {
                        Ok(url) => Task::done(BuySellMessage::WebviewOpenUrl(url)),
                        Err(error) => {
                            tracing::error!("[ONRAMPER] Error: {}", error);
                            Task::done(BuySellMessage::SessionError(error.to_string()))
                        }
                    }
                }
                .map(|m| Message::View(ViewMessage::BuySell(m)));
            }
            BuySellMessage::SessionError(error) => {
                self.error = Some(error);
            }

            // mavapay session logic
            BuySellMessage::Mavapay(msg) => {
                if let BuySellFlowState::Mavapay(mavapay) = &mut self.flow_state {
                    match (&mut mavapay.step, msg) {
                        // user can login from email verification or login forms
                        (
                            MavapayFlowStep::VerifyEmail {
                                email, password, ..
                            }
                            | MavapayFlowStep::Login { email, password },
                            MavapayMessage::SubmitLogin {
                                skip_email_verification,
                            },
                        ) => {
                            let client = mavapay.coincube_client.clone();

                            let email = email.to_string();
                            let password = password.to_string();

                            return Task::perform(
                                async move {
                                    let login = client.login(&email, &password).await;
                                    let verified = match skip_email_verification {
                                        true => true,
                                        false => {
                                            let status = client
                                                .check_email_verification_status(&email)
                                                .await?;
                                            status.email_verified
                                        }
                                    };

                                    // TODO: two factor authentication flows will be needed here

                                    login.map(|l| (l, verified))
                                },
                                |res| match res {
                                    Ok((login, email_verified)) => {
                                        BuySellMessage::Mavapay(MavapayMessage::LoginSuccess {
                                            email_verified,
                                            login,
                                        })
                                    }
                                    Err(e) => BuySellMessage::SessionError(e.to_string()),
                                },
                            )
                            .map(|m| Message::View(ViewMessage::BuySell(m)));
                        }
                        (
                            MavapayFlowStep::VerifyEmail {
                                email, password, ..
                            }
                            | MavapayFlowStep::Login {
                                email, password, ..
                            },
                            MavapayMessage::LoginSuccess {
                                email_verified,
                                login,
                            },
                        ) => {
                            if !email_verified {
                                // transition to email verification UI flow
                                mavapay.step = MavapayFlowStep::VerifyEmail {
                                    email: email.clone(),
                                    password: password.clone(),
                                    checking: false,
                                };

                                return Task::none();
                            }

                            log::info!("Successfully logged in user: {}", &login.user.email);

                            // initialize location information for user
                            let iso = self.detected_country.as_ref().map(|c| c.code);
                            let source_currency = match iso {
                                Some("NG") => Some(MavapayUnitCurrency::NigerianNairaKobo),
                                Some("KE") => Some(MavapayUnitCurrency::KenyanShillingCent),
                                Some("ZA") => Some(MavapayUnitCurrency::SouthAfricanRandCent),
                                c => {
                                    log::error!("Country: {:?} is not supported by mavapay", c);
                                    None
                                }
                            };

                            // store token in OS keyring
                            if let Ok(entry) = keyring::Entry::new("io.coincube.Vault", "vault") {
                                entry.set_password(&login.token).unwrap();

                                match serde_json::to_vec(&login.user) {
                                    Ok(bytes) => {
                                        entry.set_secret(&bytes).unwrap();
                                    }
                                    Err(err) => log::error!(
                                        "Unable to serialize user data to OS keyring: {}",
                                        err
                                    ),
                                }
                            };

                            // persist user state
                            mavapay.current_user = Some(login.user);
                            mavapay.auth_token = Some(login.token);

                            mavapay.step = MavapayFlowStep::ActiveBuysell {
                                country: self.detected_country.clone().unwrap(),
                                flow_mode: MavapayFlowMode::CreateQuote,
                                amount: 60,
                                source_currency,
                                target_currency: None,
                                settlement_currency: None,
                                payment_method: MavapayPaymentMethod::Lightning,
                                bank_account_number: String::new(),
                                bank_account_name: String::new(),
                                bank_code: String::new(),
                                bank_name: String::new(),
                                current_quote: None,
                                current_price: None,
                            };

                            // Automatically get current price when entering dashboard
                            return Task::done(Message::View(ViewMessage::BuySell(
                                BuySellMessage::Mavapay(MavapayMessage::GetPrice),
                            )));
                        }
                        // user registration form
                        (
                            MavapayFlowStep::Register {
                                first_name,
                                last_name,
                                password1,
                                password2,
                                email,
                            },
                            msg,
                        ) => match msg {
                            MavapayMessage::FirstNameChanged(name) => {
                                first_name.value = name;
                            }
                            MavapayMessage::LastNameChanged(name) => {
                                last_name.value = name;
                            }
                            MavapayMessage::EmailChanged(name) => {
                                email.value = name;
                            }
                            MavapayMessage::Password1Changed(name) => {
                                password1.value = name;
                            }
                            MavapayMessage::Password2Changed(name) => {
                                password2.value = name;
                            }
                            MavapayMessage::SubmitRegistration => {
                                let client = mavapay.coincube_client.clone();
                                let request = crate::services::coincube::SignUpRequest {
                                    account_type:
                                        crate::services::coincube::AccountType::Individual,
                                    email: email.value.clone(),
                                    first_name: first_name.value.clone(),
                                    last_name: last_name.value.clone(),
                                    auth_details: [crate::services::coincube::AuthDetail {
                                        provider: 1, // EmailProvider = 1
                                        password: password1.value.clone(),
                                    }],
                                };

                                return Task::perform(
                                    async move { client.sign_up(request).await },
                                    |result| match result {
                                        Ok(_response) => Message::View(ViewMessage::BuySell(
                                            BuySellMessage::Mavapay(
                                                MavapayMessage::RegistrationSuccess,
                                            ),
                                        )),
                                        Err(error) => Message::View(ViewMessage::BuySell(
                                            BuySellMessage::SessionError(error.to_string()),
                                        )),
                                    },
                                );
                            }
                            MavapayMessage::RegistrationSuccess => {
                                self.error = None;
                                mavapay.step = MavapayFlowStep::VerifyEmail {
                                    email: email.value.clone(),
                                    password: password1.value.clone(),
                                    checking: false,
                                };
                            }
                            msg => log::warn!(
                                "Current {:?} has ignored message: {:?}",
                                &mavapay.step,
                                msg
                            ),
                        },
                        // email verification step
                        (
                            MavapayFlowStep::VerifyEmail {
                                email, checking, ..
                            },
                            msg,
                        ) => match msg {
                            MavapayMessage::SendVerificationEmail => {
                                tracing::info!("Sending verification email to: {}", email);

                                let client = mavapay.coincube_client.clone();
                                let email = email.clone();

                                return Task::perform(
                                    async move { client.send_verification_email(&email).await },
                                    |result| match result {
                                        Ok(_) => Message::View(ViewMessage::BuySell(
                                            BuySellMessage::Mavapay(
                                                MavapayMessage::CheckEmailVerificationStatus,
                                            ),
                                        )),
                                        Err(error) => Message::View(ViewMessage::BuySell(
                                            BuySellMessage::SessionError(error.to_string()),
                                        )),
                                    },
                                );
                            }
                            MavapayMessage::CheckEmailVerificationStatus => {
                                if *checking {
                                    log::info!("Already polling API for Email verification status for {email}");
                                    return Task::none();
                                }

                                self.error = None;
                                *checking = true;

                                // recheck status every 10 seconds, automatic login if email is verified
                                let client = mavapay.coincube_client.clone();
                                let email = email.clone();

                                return Task::perform(
                                    async move {
                                        let mut count = 30;

                                        loop {
                                            if count == 0 {
                                                break Err(());
                                            };

                                            match client
                                                .check_email_verification_status(&email)
                                                .await
                                            {
                                                Ok(res) => {
                                                    if res.email_verified {
                                                        log::info!(
                                                            "Email {} has been verified",
                                                            email
                                                        );
                                                        break Ok(());
                                                    }
                                                }
                                                Err(err) => {
                                                    log::warn!("Encountered error while verifying email: {:?}", err)
                                                }
                                            }

                                            count = count - 1;
                                            tokio::time::sleep(std::time::Duration::from_secs(3))
                                                .await;
                                        }
                                    },
                                    |r| match r {
                                        Ok(_) => Message::View(ViewMessage::BuySell(
                                            BuySellMessage::Mavapay(MavapayMessage::SubmitLogin {
                                                skip_email_verification: true,
                                            }),
                                        )),
                                        Err(_) => Message::View(ViewMessage::BuySell(
                                            BuySellMessage::Mavapay(
                                                MavapayMessage::EmailVerificationFailed,
                                            ),
                                        )),
                                    },
                                );
                            }
                            MavapayMessage::EmailVerificationFailed => {
                                *checking = false;
                                self.error = Some(
                                    "Timeout attempting automatic login after email verification"
                                        .to_string(),
                                );
                            }
                            msg => log::warn!(
                                "Current {:?} has ignored message: {:?}",
                                &mavapay.step,
                                msg
                            ),
                        },
                        // login to existing mavapay account
                        (MavapayFlowStep::Login { email, password }, msg) => match msg {
                            MavapayMessage::LoginUsernameChanged(username) => *email = username,
                            MavapayMessage::LoginPasswordChanged(pswd) => *password = pswd,
                            MavapayMessage::CreateNewAccount => {
                                mavapay.step = MavapayFlowStep::Register {
                                    first_name: Default::default(),
                                    last_name: Default::default(),
                                    password1: Default::default(),
                                    password2: Default::default(),
                                    email: Default::default(),
                                };
                            }

                            msg => log::warn!(
                                "Current {:?} has ignored message: {:?}",
                                &mavapay.step,
                                msg
                            ),
                        },
                        // active buysell form
                        (
                            MavapayFlowStep::ActiveBuysell {
                                flow_mode,
                                amount,
                                source_currency,
                                target_currency,
                                settlement_currency,
                                payment_method,
                                bank_account_number,
                                bank_account_name,
                                bank_code,
                                bank_name,
                                current_price,
                                ..
                            },
                            msg,
                        ) => match msg {
                            MavapayMessage::FlowModeChanged(mode) => *flow_mode = mode,
                            MavapayMessage::AmountChanged(a) => *amount = a,
                            MavapayMessage::SourceCurrencyChanged(currency) => {
                                *source_currency = Some(currency)
                            }
                            MavapayMessage::TargetCurrencyChanged(currency) => {
                                *target_currency = Some(currency)
                            }
                            MavapayMessage::SettlementCurrencyChanged(currency) => {
                                *settlement_currency = Some(currency)
                            }
                            MavapayMessage::PaymentMethodChanged(method) => {
                                *payment_method = method
                            }
                            MavapayMessage::BankAccountNumberChanged(number) => {
                                *bank_account_number = number
                            }
                            MavapayMessage::BankAccountNameChanged(name) => {
                                *bank_account_name = name
                            }
                            MavapayMessage::PriceReceived(price) => {
                                *current_price = Some(price);
                            }
                            MavapayMessage::BankCodeChanged(code) => *bank_code = code,
                            MavapayMessage::BankNameChanged(name) => *bank_name = name,
                            MavapayMessage::CreateQuote => {
                                return mavapay
                                    .create_quote(self.detected_country.as_ref().map(|c| c.code))
                                    .map(|b| Message::View(ViewMessage::BuySell(b)))
                            }
                            MavapayMessage::OpenPaymentLink => {
                                return mavapay
                                    .open_payment_link()
                                    .map(|b| Message::View(ViewMessage::BuySell(b)))
                            }
                            MavapayMessage::GetPrice => {
                                return mavapay
                                    .get_price(self.detected_country.as_ref().map(|c| c.code))
                                    .map(|b| Message::View(ViewMessage::BuySell(b)))
                            }
                            msg => log::warn!(
                                "Current {:?} has ignored message: {:?}",
                                &mavapay.step,
                                msg
                            ),
                        },
                    }
                } else {
                    log::warn!("Ignoring MavapayMessage: {:?}, BuySell Panel is currently not in Mavapay state", msg);
                }
            }

            // webview logic
            BuySellMessage::WryMessage(msg) => self.webview_manager.update(msg),
            BuySellMessage::WebviewOpenUrl(url) => {
                // extract the main window's raw_window_handle
                return iced_wry::IcedWebviewManager::extract_window_id(None).map(move |w| {
                    Message::View(ViewMessage::BuySell(BuySellMessage::StarWryWebviewWithUrl(
                        w,
                        url.clone(),
                    )))
                });
            }
            BuySellMessage::StarWryWebviewWithUrl(id, url) => {
                let webview = self.webview_manager.new_webview(
                    iced_wry::wry::WebViewAttributes {
                        url: Some(url),
                        devtools: cfg!(debug_assertions),
                        incognito: true,
                        ..Default::default()
                    },
                    id,
                );

                if let Some(wv) = webview {
                    self.flow_state = BuySellFlowState::WebviewRenderer { active: wv }
                } else {
                    tracing::error!("Unable to instantiate wry webview")
                }
            }
        };

        Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<crate::app::wallet::Wallet>,
    ) -> Task<Message> {
        match self.detected_country {
            Some(_) => Task::none(),
            None => {
                let client = self.coincube_client.clone();

                Task::perform(async move { client.locate().await }, |result| {
                    Message::View(ViewMessage::BuySell(BuySellMessage::CountryDetected(
                        result.map_err(|e| e.to_string()),
                    )))
                })
            }
        }
    }

    fn close(&mut self) -> Task<Message> {
        if let BuySellFlowState::WebviewRenderer {
            active: active_webview,
            ..
        } = &self.flow_state
        {
            if let Some(strong) = std::sync::Weak::upgrade(&active_webview.webview) {
                let _ = strong.set_visible(false);
                let _ = strong.focus_parent();
            }
        }

        // BUG: messages returned from close are not handled by the current panel, but rather by the state containing the next panel?
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        self.webview_manager
            .subscription(std::time::Duration::from_millis(25))
            .map(|m| Message::View(ViewMessage::BuySell(BuySellMessage::WryMessage(m))))
    }
}
