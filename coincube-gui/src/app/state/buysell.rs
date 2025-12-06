use iced::Task;
use std::sync::Arc;

use coincube_ui::widget::Element;

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        message::Message,
        state::State,
        view::{self, buysell::*, BuySellMessage, MavapayMessage, Message as ViewMessage},
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

        coincube_ui::widget::modal::Modal::new(inner, overlay)
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
            Message::View(ViewMessage::BuySell(message)) => message,
            // modal for any generated address
            Message::View(ViewMessage::Select(_)) => {
                if let BuySellFlowState::Initialization { buy_or_sell, .. } = &self.flow_state {
                    if let Some(panel::BuyOrSell::Buy { address }) = buy_or_sell {
                        self.modal = super::vault::receive::Modal::VerifyAddress(
                            super::vault::receive::VerifyAddressModal::new(
                                cache.datadir_path.clone(),
                                self.wallet.clone(),
                                cache.network,
                                address.address.clone(),
                                address.index,
                            ),
                        );
                    };
                }

                return Task::none();
            }
            Message::View(ViewMessage::ShowQrCode(_)) => {
                if let BuySellFlowState::Initialization { buy_or_sell, .. } = &self.flow_state {
                    if let Some(panel::BuyOrSell::Buy { address }) = buy_or_sell {
                        if let Some(modal) = super::vault::receive::ShowQrCodeModal::new(
                            &address.address,
                            address.index,
                        ) {
                            self.modal = super::vault::receive::Modal::ShowQrCode(modal);
                        }
                    };
                }

                return Task::none();
            }
            Message::View(ViewMessage::Close) => {
                self.modal = super::vault::receive::Modal::None;
                return Task::none();
            }
            _ => return Task::none(),
        };

        match message {
            // internal state management
            BuySellMessage::ResetWidget => {
                if let Some(country) = &self.detected_country {
                    if mavapay_supported(&country.code) {
                        // attempt automatic login from os-keyring
                        // match keyring::Entry::new("io.coincube.Vault", "mavapay") {
                        //     Ok(entry) => {
                        //         if let (Ok(token), Ok(user_data)) =
                        //             (entry.get_password(), entry.get_secret())
                        //         {
                        //             self.error = None;

                        //             // start initialization step with mavapay credentials embedded
                        //             self.flow_state = BuySellFlowState::Initialization {
                        //                 buy_or_sell_selected: None,
                        //                 buy_or_sell: None,
                        //                 mavapay_credentials: Some((token, user_data)),
                        //             };

                        //             return Task::none();
                        //         };
                        //     }
                        //     Err(e) => {
                        //         log::error!("Unable to acquire OS keyring for Mavapay state: {e}");
                        //     }
                        // };

                        // send user back to mavapay login screen, to initialize login credentials
                        self.flow_state = BuySellFlowState::Mavapay(MavapayState::new())
                    } else {
                        // onramper skips to automatic initialization
                        self.flow_state = BuySellFlowState::Initialization {
                            buy_or_sell_selected: None,
                            buy_or_sell: None,
                            mavapay_credentials: None,
                        };
                    }
                } else {
                    log::warn!("Unable to reset widget, country is unknown");
                    self.flow_state = BuySellFlowState::DetectingLocation(true);
                }
            }

            // initialization flow: for creating a new address and setting panel mode (buy or sell)
            BuySellMessage::SelectBuyOrSell(bs) => {
                if let BuySellFlowState::Initialization {
                    buy_or_sell_selected,
                    ..
                } = &mut self.flow_state
                {
                    *buy_or_sell_selected = Some(bs)
                }
            }
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
                        Err(e) => {
                            Message::View(ViewMessage::BuySell(BuySellMessage::SessionError(
                                "Unable to create a new address",
                                e.to_string(),
                            )))
                        }
                    },
                )
            }
            BuySellMessage::AddressCreated(address) => {
                if let BuySellFlowState::Initialization { buy_or_sell, .. } = &mut self.flow_state {
                    *buy_or_sell = Some(panel::BuyOrSell::Buy { address })
                }
            }

            // ip-geolocation logic
            BuySellMessage::CountryDetected(result) => {
                let country = match result {
                    Ok(country) => {
                        self.error = None;
                        country
                    }
                    Err(err) => {
                        log::error!("Error detecting country via geo-ip, switching to manual country selector.\n    {}", err);

                        self.flow_state = BuySellFlowState::DetectingLocation(true);
                        self.detected_country = None;

                        return Task::done(Message::View(ViewMessage::BuySell(
                            BuySellMessage::SessionError(
                                "Unable to automatically determine location",
                                "please select manually below".to_string(),
                            ),
                        )));
                    }
                };

                // update location information
                log::info!("Country = {}, ISO = {}", country.name, country.code);
                self.detected_country = Some(country.clone());

                return Task::done(Message::View(ViewMessage::BuySell(
                    BuySellMessage::ResetWidget,
                )));
            }

            // session management
            BuySellMessage::StartSession => {
                let BuySellFlowState::Initialization {
                    buy_or_sell,
                    mavapay_credentials,
                    ..
                } = &mut self.flow_state
                else {
                    unreachable!(
                        "`StartSession` is always called after the Initialization Flow Stage"
                    )
                };

                let buy_or_sell = buy_or_sell.as_ref().unwrap_or(&panel::BuyOrSell::Sell);
                let Some(country) = self.detected_country.as_ref() else {
                    unreachable!(
                        "Unable to start session, country detection|selection was unsuccessful"
                    );
                };

                match mavapay_credentials.take() {
                    // if present, login to mavapay state
                    Some((token, user_data)) => {
                        // start buysell under Mavapay
                        let mut mavapay = MavapayState::new();

                        match serde_json::from_slice(&user_data) {
                            Ok(user) => {
                                mavapay.auth_token = Some(token);
                                mavapay.current_user = Some(user);
                                log::info!("Mavapay session successfully restored from OS keyring");
                                mavapay.step = MavapayFlowStep::Transaction {
                                    buy_or_sell: buy_or_sell.clone(),
                                    country: country.clone(),
                                    banks: None,
                                    amount: 60,
                                    beneficiary: None,
                                    selected_bank: None,
                                    current_quote: None,
                                    current_price: None,
                                };
                                self.flow_state = BuySellFlowState::Mavapay(mavapay);
                                return if country.code != "KE" {
                                    Task::batch([
                                        Task::done(Message::View(view::Message::BuySell(
                                            BuySellMessage::Mavapay(MavapayMessage::GetBanks),
                                        ))),
                                        Task::done(Message::View(view::Message::BuySell(
                                            BuySellMessage::Mavapay(MavapayMessage::GetPrice),
                                        ))),
                                    ])
                                } else {
                                    Task::done(Message::View(view::Message::BuySell(
                                        BuySellMessage::Mavapay(MavapayMessage::GetPrice),
                                    )))
                                };
                            }
                            Err(e) => {
                                log::error!("Unable to parse user data from OS keyring, possibly malformed or outdated data: {e}");
                                self.error = Some(
                                    "Unable to restore Mavapay session, data is malformed or outdated"
                                        .to_string(),
                                );

                                // send user back to mavapay login screen
                                self.flow_state = BuySellFlowState::Mavapay(MavapayState::new())
                            }
                        }
                    }
                    // start buysell under Onramper
                    None => {
                        let Some(currency) = crate::services::coincube::get_countries()
                            .iter()
                            .find(|c| c.code == country.code)
                            .map(|c| c.currency.code)
                        else {
                            self.error = Some(format!(
                                "[FATAL] The country iso code ({}) is invalid",
                                country.code
                            ));
                            return Task::none();
                        };

                        // create onramper widget url and start session
                        let url = match buy_or_sell {
                            view::buysell::panel::BuyOrSell::Buy { address } => {
                                let address = address.address.to_string();
                                crate::app::buysell::onramper::create_widget_url(
                                    &currency,
                                    Some(&address),
                                    "buy",
                                    self.network,
                                )
                            }
                            view::buysell::panel::BuyOrSell::Sell => {
                                crate::app::buysell::onramper::create_widget_url(
                                    &currency,
                                    None,
                                    "sell",
                                    self.network,
                                )
                            }
                        };

                        return match url {
                            Ok(url) => Task::done(BuySellMessage::WebviewOpenUrl(url)),
                            Err(e) => Task::done(BuySellMessage::SessionError(
                                "Couldn't create Onramper URL",
                                e.to_string(),
                            )),
                        }
                        .map(|m| Message::View(ViewMessage::BuySell(m)));
                    }
                }
            }
            BuySellMessage::SessionError(description, error) => {
                self.error = Some(format!("{} ({})", description, error));
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
                            let client = self.coincube_client.clone();

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
                                    Err(e) => BuySellMessage::SessionError(
                                        "Failed to submit login",
                                        e.to_string(),
                                    ),
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
                            let bytes = serde_json::to_vec(&login.user).unwrap();

                            // store token in OS keyring
                            // if let Ok(entry) = keyring::Entry::new("io.coincube.Vault", "mavapay") {
                            //     if let Err(e) = entry.set_password(&login.token) {
                            //         log::error!("Failed to store auth token in keyring: {}", e);
                            //     }

                            //     if let Err(e) = entry.set_secret(&bytes) {
                            //         log::error!("Unable to store user data in keyring: {e}");
                            //     };
                            // } else {
                            //     self.error = Some("Unable to initialize OS keyring".to_string());
                            // };

                            // go straight to initialization
                            self.flow_state = BuySellFlowState::Initialization {
                                buy_or_sell_selected: None,
                                buy_or_sell: None,
                                mavapay_credentials: Some((login.token, bytes)),
                            };
                        }
                        // user registration form
                        (
                            MavapayFlowStep::Register {
                                legal_name,
                                password1,
                                password2,
                                email,
                            },
                            msg,
                        ) => match msg {
                            MavapayMessage::LegalNameChanged(n) => *legal_name = n,
                            MavapayMessage::EmailChanged(e) => *email = e,
                            MavapayMessage::Password1Changed(p) => *password1 = p,
                            MavapayMessage::Password2Changed(p) => *password2 = p,

                            MavapayMessage::SubmitRegistration => {
                                let client = self.coincube_client.clone();
                                let request = crate::services::coincube::SignUpRequest {
                                    account_type:
                                        crate::services::coincube::AccountType::Individual,
                                    email: email.clone(),
                                    legal_name: legal_name.clone(),
                                    auth_details: [crate::services::coincube::AuthDetail {
                                        provider: 1, // EmailProvider = 1
                                        password: password1.clone(),
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
                                        Err(e) => Message::View(ViewMessage::BuySell(
                                            BuySellMessage::SessionError(
                                                "Couldn't process signup request",
                                                e.to_string(),
                                            ),
                                        )),
                                    },
                                );
                            }
                            MavapayMessage::RegistrationSuccess => {
                                self.error = None;
                                mavapay.step = MavapayFlowStep::VerifyEmail {
                                    email: email.clone(),
                                    password: password1.clone(),
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

                                let client = self.coincube_client.clone();
                                let email = email.clone();

                                return Task::perform(
                                    async move { client.send_verification_email(&email).await },
                                    |result| match result {
                                        Ok(_) => Message::View(ViewMessage::BuySell(
                                            BuySellMessage::Mavapay(
                                                MavapayMessage::CheckEmailVerificationStatus,
                                            ),
                                        )),
                                        Err(e) => Message::View(ViewMessage::BuySell(
                                            BuySellMessage::SessionError(
                                                "Unable to send verification email",
                                                e.to_string(),
                                            ),
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
                                let client = self.coincube_client.clone();
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
                                            tokio::time::sleep(std::time::Duration::from_secs(10))
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
                                    legal_name: Default::default(),
                                    password1: Default::default(),
                                    password2: Default::default(),
                                    email: Default::default(),
                                };
                            }
                            MavapayMessage::ResetPassword => {
                                mavapay.step = MavapayFlowStep::PasswordReset {
                                    email: email.clone(),
                                    sent: false,
                                }
                            }

                            msg => log::warn!(
                                "Current {:?} has ignored message: {:?}",
                                &mavapay.step,
                                msg
                            ),
                        },
                        // password reset form
                        (MavapayFlowStep::PasswordReset { email, sent }, msg) => match msg {
                            MavapayMessage::EmailChanged(e) => {
                                *sent = false;
                                *email = e;
                            }
                            MavapayMessage::SendPasswordResetEmail => {
                                let email = email.clone();
                                let client = self.coincube_client.clone();

                                return Task::perform(
                                    async move { client.send_password_reset_email(&email).await },
                                    |res| match res {
                                        Ok(sent) => Message::View(view::Message::BuySell(
                                            BuySellMessage::Mavapay(
                                                MavapayMessage::PasswordResetEmailSent(
                                                    sent.message,
                                                ),
                                            ),
                                        )),
                                        Err(e) => Message::View(view::Message::BuySell(
                                            BuySellMessage::SessionError(
                                                "Unable to send password reset email",
                                                e.to_string(),
                                            ),
                                        )),
                                    },
                                );
                            }
                            MavapayMessage::PasswordResetEmailSent(msg) => {
                                log::info!("[PASSWORD RESET] {}", msg);
                                *sent = true;
                            }
                            MavapayMessage::ReturnToLogin => {
                                mavapay.step = MavapayFlowStep::Login {
                                    email: email.clone(),
                                    password: "".to_string(),
                                }
                            }
                            msg => log::warn!(
                                "Current {:?} has ignored message: {:?}",
                                &mavapay.step,
                                msg
                            ),
                        },
                        // transaction form
                        (
                            MavapayFlowStep::Transaction {
                                amount,
                                current_price,
                                country,
                                banks,
                                ..
                            },
                            msg,
                        ) => {
                            match msg {
                                MavapayMessage::AmountChanged(a) => *amount = a,

                                // TODO: Beneficiary specific form inputs
                                MavapayMessage::CreateQuote => {
                                    return mavapay
                                        .create_quote(self.coincube_client.clone())
                                        .map(|b| Message::View(ViewMessage::BuySell(b)));
                                }
                                MavapayMessage::QuoteCreated(quote) => {
                                    log::info!("[MAVAPAY] Quote created: {}", quote.id);

                                    // TODO: Implement checkout UI, with checkout events propagated via SSE and adapted into the iced runtime as an `iced::Subscription`
                                }

                                MavapayMessage::PriceReceived(price) => {
                                    *current_price = Some(price);
                                }
                                MavapayMessage::BanksReceived(b) => *banks = Some(b),
                                MavapayMessage::GetPrice => {
                                    let code = country.code;
                                    return mavapay
                                        .get_price(code)
                                        .map(|b| Message::View(ViewMessage::BuySell(b)));
                                }
                                MavapayMessage::GetBanks => {
                                    let code = country.code;
                                    return mavapay
                                        .get_banks(code)
                                        .map(|b| Message::View(ViewMessage::BuySell(b)));
                                }
                                msg => log::warn!(
                                    "Current {:?} has ignored message: {:?}",
                                    &mavapay.step,
                                    msg
                                ),
                            }
                        }
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
                    Message::View(ViewMessage::BuySell(
                        BuySellMessage::StartWryWebviewWithUrl(w, url.clone()),
                    ))
                });
            }
            BuySellMessage::StartWryWebviewWithUrl(id, url) => {
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
