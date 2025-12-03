use iced::Task;
use std::sync::Arc;

use coincube_ui::widget::Element;

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        message::Message,
        state::{self, State},
        view::{self, buysell::*, BuySellMessage, MavapayMessage, Message as ViewMessage},
    },
    daemon::Daemon,
    services::{coincube::*, mavapay::*},
};

impl State for BuySellPanel {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, ViewMessage> {
        let inner = view::dashboard(menu, cache, None, self.view());

        if let BuySellFlowState::Initialization { modal, .. } = &self.step {
            let overlay = match modal {
                super::vault::receive::Modal::VerifyAddress(m) => m.view(),
                super::vault::receive::Modal::ShowQrCode(m) => m.view(),
                super::vault::receive::Modal::None => return inner,
            };

            coincube_ui::widget::modal::Modal::new(inner, overlay)
                .on_blur(Some(ViewMessage::Close))
                .into()
        } else {
            inner
        }
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
                if let BuySellFlowState::Initialization {
                    buy_or_sell, modal, ..
                } = &mut self.step
                {
                    if let Some(panel::BuyOrSell::Buy { address }) = buy_or_sell {
                        *modal = super::vault::receive::Modal::VerifyAddress(
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
                if let BuySellFlowState::Initialization {
                    buy_or_sell, modal, ..
                } = &mut self.step
                {
                    if let Some(panel::BuyOrSell::Buy { address }) = buy_or_sell {
                        if let Some(new) = super::vault::receive::ShowQrCodeModal::new(
                            &address.address,
                            address.index,
                        ) {
                            *modal = super::vault::receive::Modal::ShowQrCode(new);
                        }
                    };
                }

                return Task::none();
            }
            Message::View(ViewMessage::Close) => {
                if let BuySellFlowState::Initialization { modal, .. } = &mut self.step {
                    *modal = super::vault::receive::Modal::None;
                }

                return Task::none();
            }
            _ => return Task::none(),
        };

        match message {
            BuySellMessage::ResetWidget => {
                if let Some(country) = &self.detected_country {
                    // attempt automatic refresh from os-keyring
                    match keyring::Entry::new("io.coincube.Vault", &self.wallet.name) {
                        Ok(entry) => {
                            if let Ok(user_data) = entry.get_secret() {
                                match serde_json::from_slice::<LoginResponse>(&user_data) {
                                    Ok(login) => {
                                        self.current_user = Some(login.user);
                                        self.auth_token = Some(login.token);
                                    }
                                    Err(er) => {
                                        log::error!("Unable to parse user information found in OS keyring: {:?}", er)
                                    }
                                };
                            };
                        }
                        Err(e) => {
                            log::error!("Unable to restore login state from OS keyring: {e}");
                        }
                    };

                    if mavapay_supported(&country.code) {
                        match self.auth_token {
                            // send user directly to initialization
                            Some(_) => {
                                self.step = BuySellFlowState::Initialization {
                                    modal: state::vault::receive::Modal::None,
                                    buy_or_sell_selected: None,
                                    buy_or_sell: None,
                                };
                            }
                            // send user to login screen, to initialize login credentials
                            None => {
                                self.step = BuySellFlowState::Login {
                                    email: Default::default(),
                                    password: Default::default(),
                                }
                            }
                        }
                    } else {
                        // onramper skips to automatic initialization
                        self.step = BuySellFlowState::Initialization {
                            modal: state::vault::receive::Modal::None,
                            buy_or_sell_selected: None,
                            buy_or_sell: None,
                        };
                    }
                } else {
                    log::warn!("Unable to reset widget, country is unknown");
                    self.step = BuySellFlowState::DetectingLocation(true);
                }
            }

            // initialization flow: for creating a new address and setting panel mode (buy or sell)
            BuySellMessage::SelectBuyOrSell(bs) => {
                if let BuySellFlowState::Initialization {
                    buy_or_sell_selected,
                    ..
                } = &mut self.step
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
                if let BuySellFlowState::Initialization { buy_or_sell, .. } = &mut self.step {
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

                        self.step = BuySellFlowState::DetectingLocation(true);
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
                let BuySellFlowState::Initialization { buy_or_sell, .. } = &mut self.step else {
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

                match mavapay_supported(country.code) {
                    true => {
                        log::info!("Starting buysell under Mavapay");

                        // start buysell under Mavapay
                        let mavapay = MavapayState::new(buy_or_sell.clone(), country.clone());
                        self.step = BuySellFlowState::Mavapay(mavapay);

                        if country.code != "KE" {
                            return Task::batch([
                                Task::done(Message::View(view::Message::BuySell(
                                    BuySellMessage::Mavapay(MavapayMessage::GetBanks),
                                ))),
                                Task::done(Message::View(view::Message::BuySell(
                                    BuySellMessage::Mavapay(MavapayMessage::GetPrice),
                                ))),
                            ]);
                        } else {
                            return Task::done(Message::View(view::Message::BuySell(
                                BuySellMessage::Mavapay(MavapayMessage::GetPrice),
                            )));
                        };
                    }
                    false => {
                        log::info!("Starting buysell under Onramper");

                        // start buysell under Onramper
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
                if let BuySellFlowState::Mavapay(mavapay) = &mut self.step {
                    match (&mut mavapay.step, msg) {
                        // transactions form
                        (
                            MavapayFlowStep::Transaction {
                                sat_amount,
                                btc_price,
                                country,
                                banks,
                                buy_or_sell,
                                beneficiary,
                                transfer_speed,
                                ..
                            },
                            msg,
                        ) => {
                            match msg {
                                MavapayMessage::NormalizeAmounts => {
                                    *sat_amount = (*sat_amount).max(6000.0)
                                }
                                MavapayMessage::SatAmountChanged(sats) => *sat_amount = sats,
                                MavapayMessage::FiatAmountChanged(fiat) => match btc_price {
                                    Some(price) => {
                                        let sat_price =
                                            price.btc_price_in_unit_currency / 100_000_000.0;
                                        *sat_amount = fiat / sat_price
                                    }
                                    None => log::warn!(
                                        "Unable to update BTC amount, BTC price is unknown"
                                    ),
                                },
                                MavapayMessage::TransferSpeedChanged(s) => *transfer_speed = s,

                                // TODO: Beneficiary specific form inputs
                                MavapayMessage::CreateQuote => {
                                    return mavapay
                                        .create_quote(self.coincube_client.clone())
                                        .map(|b| Message::View(ViewMessage::BuySell(b)));
                                }
                                MavapayMessage::QuoteCreated(quote) => {
                                    log::info!(
                                        "[MAVAPAY] Quote created: {}, Order ID: {:?}",
                                        quote.id,
                                        quote.order_id
                                    );

                                    // poll mavapay API for the status of the adjacent transaction (quote.hash == transaction.hash)
                                    let client = mavapay.client.clone();
                                    let quote_hash = quote.hash.clone();
                                    let quote_order_id = quote.order_id.clone();

                                    let transaction_checker = Task::perform(
                                        async move {
                                            loop {
                                                let order = client.get_order(&quote_order_id).await;

                                                match order {
                                                    Ok(order)
                                                        if matches!(
                                                            order.status,
                                                            TransactionStatus::Paid
                                                        ) =>
                                                    {
                                                        break order
                                                    }
                                                    Ok(order) => {
                                                        log::info!("[MAVAPAY] Current order for quote with hash ({}) = {:?}", quote_hash, order);
                                                    }
                                                    Err(e) => {
                                                        log::error!("[MAVAPAY] Unable to check Mavapay API for transaction status: {:?}", e)
                                                    }
                                                }

                                                tokio::time::sleep(std::time::Duration::from_secs(
                                                    10,
                                                ))
                                                .await
                                            }
                                        },
                                        |res| {
                                            Message::View(ViewMessage::BuySell(
                                                BuySellMessage::Mavapay(
                                                    MavapayMessage::QuoteFulfilled(res),
                                                ),
                                            ))
                                        },
                                    );

                                    // switch to checkout
                                    mavapay.step = MavapayFlowStep::Checkout {
                                        sat_amount: sat_amount.clone(),
                                        buy_or_sell: buy_or_sell.clone(),
                                        beneficiary: beneficiary.clone(),
                                        quote,
                                    };

                                    return transaction_checker;
                                }

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

                                MavapayMessage::PriceReceived(price) => *btc_price = Some(price),
                                MavapayMessage::BanksReceived(b) => *banks = Some(b),

                                msg => log::warn!(
                                    "Current {:?} has ignored message: {:?}",
                                    &mavapay.step,
                                    msg
                                ),
                            }
                        }
                        // checkout form
                        (MavapayFlowStep::Checkout { .. }, msg) => match msg {
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
            BuySellMessage::WebviewOpenUrl(url) => {
                // extract the main window's raw_window_handle
                return iced_wry::IcedWebviewManager::extract_window_id(None).map(move |w| {
                    Message::View(ViewMessage::BuySell(
                        BuySellMessage::StartWryWebviewWithUrl(w, url.clone()),
                    ))
                });
            }
            BuySellMessage::WryMessage(msg) => {
                if let BuySellFlowState::WebviewRenderer { manager, .. } = &mut self.step {
                    manager.update(msg)
                }
            }
            BuySellMessage::StartWryWebviewWithUrl(id, url) => {
                let mut manager = iced_wry::IcedWebviewManager::new();
                let webview = manager.new_webview(
                    iced_wry::wry::WebViewAttributes {
                        url: Some(url),
                        devtools: cfg!(debug_assertions),
                        incognito: true,
                        ..Default::default()
                    },
                    id,
                );

                if let Some(wv) = webview {
                    self.step = BuySellFlowState::WebviewRenderer {
                        active: wv,
                        manager,
                    }
                } else {
                    tracing::error!("Unable to instantiate wry webview")
                }
            }

            // state specific messages
            msg => {
                match (&mut self.step, msg) {
                    // user can login from email verification or login forms
                    (
                        BuySellFlowState::VerifyEmail {
                            email, password, ..
                        }
                        | BuySellFlowState::Login { email, password },
                        BuySellMessage::SubmitLogin {
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
                                        let status =
                                            client.check_email_verification_status(&email).await?;
                                        status.email_verified
                                    }
                                };

                                // TODO: two factor authentication flows will be needed here

                                login.map(|l| (l, verified))
                            },
                            |res| match res {
                                Ok((login, email_verified)) => BuySellMessage::LoginSuccess {
                                    email_verified,
                                    login,
                                },
                                Err(e) => BuySellMessage::SessionError(
                                    "Failed to submit login",
                                    e.to_string(),
                                ),
                            },
                        )
                        .map(|m| Message::View(ViewMessage::BuySell(m)));
                    }
                    (
                        BuySellFlowState::VerifyEmail {
                            email, password, ..
                        }
                        | BuySellFlowState::Login {
                            email, password, ..
                        },
                        BuySellMessage::LoginSuccess {
                            email_verified,
                            login,
                        },
                    ) => {
                        if !email_verified {
                            // transition to email verification UI flow
                            self.step = BuySellFlowState::VerifyEmail {
                                email: email.clone(),
                                password: password.clone(),
                                checking: false,
                            };

                            return Task::none();
                        }

                        log::info!("Successfully logged in user: {}", &login.user.email);

                        // store token in OS keyring
                        if let Ok(entry) =
                            keyring::Entry::new("io.coincube.Vault", &self.wallet.name)
                        {
                            if let Err(e) = entry.delete_credential() {
                                log::warn!("Unable to clear previous entry from keyring: {e}");
                            };

                            let bytes = serde_json::to_vec(&login).unwrap();
                            if let Err(e) = entry.set_secret(&bytes) {
                                log::error!("Unable to store user data in keyring: {e}");
                            };
                        } else {
                            self.error = Some("Unable to access OS keyring".to_string());
                        };

                        // persist login information in state
                        self.auth_token = Some(login.token);
                        self.current_user = Some(login.user);

                        // go straight to initialization
                        self.step = BuySellFlowState::Initialization {
                            modal: state::vault::receive::Modal::None,
                            buy_or_sell_selected: None,
                            buy_or_sell: None,
                        };
                    }
                    // user registration form
                    (
                        BuySellFlowState::Register {
                            legal_name,
                            password1,
                            password2,
                            email,
                        },
                        msg,
                    ) => match msg {
                        BuySellMessage::LegalNameChanged(n) => *legal_name = n,
                        BuySellMessage::EmailChanged(e) => *email = e,
                        BuySellMessage::Password1Changed(p) => *password1 = p,
                        BuySellMessage::Password2Changed(p) => *password2 = p,

                        BuySellMessage::SubmitRegistration => {
                            let client = self.coincube_client.clone();
                            let request = crate::services::coincube::SignUpRequest {
                                account_type: crate::services::coincube::AccountType::Individual,
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
                                        BuySellMessage::RegistrationSuccess,
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
                        BuySellMessage::RegistrationSuccess => {
                            self.error = None;
                            self.step = BuySellFlowState::VerifyEmail {
                                email: email.clone(),
                                password: password1.clone(),
                                checking: false,
                            };
                        }
                        msg => {
                            log::warn!(
                                "Current {} has ignored Message: {:?}",
                                self.step.name(),
                                msg
                            )
                        }
                    },
                    // email verification step
                    (
                        BuySellFlowState::VerifyEmail {
                            email, checking, ..
                        },
                        msg,
                    ) => match msg {
                        BuySellMessage::SendVerificationEmail => {
                            tracing::info!("Sending verification email to: {}", email);

                            let client = self.coincube_client.clone();
                            let email = email.clone();

                            return Task::perform(
                                async move { client.send_verification_email(&email).await },
                                |result| match result {
                                    Ok(_) => Message::View(ViewMessage::BuySell(
                                        BuySellMessage::CheckEmailVerificationStatus,
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
                        BuySellMessage::CheckEmailVerificationStatus => {
                            if *checking {
                                log::info!(
                                    "Already polling API for Email verification status for {email}"
                                );
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

                                        match client.check_email_verification_status(&email).await {
                                            Ok(res) => {
                                                if res.email_verified {
                                                    log::info!("Email {} has been verified", email);
                                                    break Ok(());
                                                }
                                            }
                                            Err(err) => {
                                                log::warn!(
                                                    "Encountered error while verifying email: {:?}",
                                                    err
                                                )
                                            }
                                        }

                                        count = count - 1;
                                        tokio::time::sleep(std::time::Duration::from_secs(10))
                                            .await;
                                    }
                                },
                                |r| match r {
                                    Ok(_) => Message::View(ViewMessage::BuySell(
                                        BuySellMessage::SubmitLogin {
                                            skip_email_verification: true,
                                        },
                                    )),
                                    Err(_) => Message::View(ViewMessage::BuySell(
                                        BuySellMessage::EmailVerificationFailed,
                                    )),
                                },
                            );
                        }
                        BuySellMessage::EmailVerificationFailed => {
                            *checking = false;
                            self.error = Some(
                                "Timeout attempting automatic login after email verification"
                                    .to_string(),
                            );
                        }
                        msg => {
                            log::warn!(
                                "Current {} has ignored message: {:?}",
                                self.step.name(),
                                msg
                            )
                        }
                    },
                    // login to existing coincube account
                    (BuySellFlowState::Login { email, password }, msg) => match msg {
                        BuySellMessage::LoginUsernameChanged(username) => *email = username,
                        BuySellMessage::LoginPasswordChanged(pswd) => *password = pswd,
                        BuySellMessage::CreateNewAccount => {
                            self.step = BuySellFlowState::Register {
                                legal_name: Default::default(),
                                password1: Default::default(),
                                password2: Default::default(),
                                email: Default::default(),
                            };
                        }
                        BuySellMessage::ResetPassword => {
                            self.step = BuySellFlowState::PasswordReset {
                                email: email.clone(),
                                sent: false,
                            }
                        }

                        msg => {
                            log::warn!(
                                "Current {:?} has ignored message: {:?}",
                                self.step.name(),
                                msg
                            )
                        }
                    },
                    // password reset form
                    (BuySellFlowState::PasswordReset { email, sent }, msg) => match msg {
                        BuySellMessage::EmailChanged(e) => {
                            *sent = false;
                            *email = e;
                        }
                        BuySellMessage::SendPasswordResetEmail => {
                            let email = email.clone();
                            let client = self.coincube_client.clone();

                            return Task::perform(
                                async move { client.send_password_reset_email(&email).await },
                                |res| match res {
                                    Ok(sent) => Message::View(view::Message::BuySell(
                                        BuySellMessage::PasswordResetEmailSent(sent.message),
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
                        BuySellMessage::PasswordResetEmailSent(msg) => {
                            log::info!("[PASSWORD RESET] {}", msg);
                            *sent = true;
                        }
                        BuySellMessage::ReturnToLogin => {
                            self.step = BuySellFlowState::Login {
                                email: email.clone(),
                                password: "".to_string(),
                            }
                        }
                        msg => {
                            log::warn!(
                                "Current {:?} has ignored message: {:?}",
                                self.step.name(),
                                msg
                            )
                        }
                    },
                    (step, msg) => {
                        log::warn!("Current {:?} has ignored message: {:?}", step.name(), msg)
                    }
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
        if let BuySellFlowState::WebviewRenderer { active, .. } = &self.step {
            if let Some(strong) = std::sync::Weak::upgrade(&active.webview) {
                let _ = strong.set_visible(false);
                let _ = strong.focus_parent();
            }
        }

        // BUG: messages returned from close are not handled by the current panel, but rather by the state containing the next panel?
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        match &self.step {
            BuySellFlowState::WebviewRenderer { manager, .. } => manager
                .subscription(std::time::Duration::from_millis(25))
                .map(|m| Message::View(ViewMessage::BuySell(BuySellMessage::WryMessage(m)))),
            // periodically re-fetch the price of BTC
            BuySellFlowState::Mavapay(MavapayState {
                step: MavapayFlowStep::Transaction { .. },
                ..
            }) => iced::time::every(std::time::Duration::from_secs(30)).map(|_| {
                Message::View(ViewMessage::BuySell(BuySellMessage::Mavapay(
                    MavapayMessage::GetPrice,
                )))
            }),
            _ => iced::Subscription::none(),
        }
    }
}
