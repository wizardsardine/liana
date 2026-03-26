use std::sync::Arc;

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        message::Message,
        state::State,
        view::{self, buysell::*},
    },
    daemon::Daemon,
    services::{coincube::*, mavapay::*},
};

const KEYRING_SERVICE_NAME: &str = if cfg!(debug_assertions) {
    "dev.coincube.Vault"
} else {
    "io.coincube.Vault"
};

impl State for BuySellPanel {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> coincube_ui::widget::Element<'a, view::Message> {
        view::dashboard(menu, cache, self.view())
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> iced::Task<Message> {
        let message = match message {
            Message::View(view::Message::BuySell(message)) => message,
            _ => return iced::Task::none(),
        };

        match message {
            view::BuySellMessage::ResetWidget => {
                if self.detected_country.is_none() {
                    log::warn!("Unable to reset widget, country is unknown");
                    self.step = BuySellFlowState::DetectingLocation(true);

                    return iced::Task::none();
                };

                if self.login.as_ref().is_none() {
                    match keyring::Entry::new(KEYRING_SERVICE_NAME, &self.wallet.name) {
                        Ok(entry) => {
                            if let Ok(user_data) = entry.get_secret() {
                                match serde_json::from_slice::<LoginResponse>(&user_data) {
                                    Ok(l) => {
                                        log::trace!("Found login credentials in OS keyring");

                                        // check if token is valid
                                        return iced::Task::done(Message::View(
                                            view::Message::BuySell(
                                                view::BuySellMessage::RefreshLogin {
                                                    refresh_token: l.refresh_token,
                                                },
                                            ),
                                        ));
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

                    // send user to login screen, to initialize login credentials
                    self.step = BuySellFlowState::Login {
                        email: Default::default(),
                        loading: false,
                    };
                } else {
                    // User is already logged in and has a country detected - reset to ModeSelect
                    self.step = BuySellFlowState::ModeSelect { buy_or_sell: None };
                }
            }

            // login states
            view::BuySellMessage::RefreshLogin { refresh_token } => {
                let client = self.coincube_client.clone();

                return iced::Task::perform(
                    async move { client.refresh_login(&refresh_token).await },
                    |res| match res {
                        Ok(l) => {
                            log::info!("Refresh token still valid, login token regenerated");
                            view::BuySellMessage::SetLoginState(l)
                        }
                        Err(err) => {
                            log::info!(
                                "Refresh token is outdated, forcing user to re-login: {}",
                                err
                            );
                            view::BuySellMessage::LogOut
                        }
                    },
                )
                .map(|msg| Message::View(view::Message::BuySell(msg)));
            }
            view::BuySellMessage::SetLoginState(login) => {
                // update token in OS keyring
                match keyring::Entry::new(KEYRING_SERVICE_NAME, &self.wallet.name) {
                    Ok(entry) => {
                        if let Err(e) = entry.delete_credential() {
                            log::warn!("Unable to clear previous entry from keyring: {e}");
                        };

                        let bytes = serde_json::to_vec(&login).unwrap();
                        if let Err(e) = entry.set_secret(&bytes) {
                            log::error!("Unable to store user data in keyring: {e}");
                        };
                    }
                    Err(err) => {
                        log::error!(
                            "[BUYSELL] Unable to persist login state, keyring inaccessible: {}",
                            err
                        )
                    }
                };

                // user is successfully logged in: 🥳
                self.coincube_client.set_token(&login.token);
                self.login = Some(login);

                self.step = BuySellFlowState::ModeSelect { buy_or_sell: None };
            }
            view::BuySellMessage::LogOut => {
                self.login = None;

                // clear keyring credentials
                if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE_NAME, &self.wallet.name) {
                    if let Err(e) = entry.delete_credential() {
                        log::error!("[BUYSELL] Unable to delete credentials from OS keyring: {e:?}")
                    };
                }

                // send user to login screen, to re-initialize login credentials
                self.step = BuySellFlowState::Login {
                    email: Default::default(),
                    loading: false,
                };
            }

            // Forward clipboard action to parent message handler
            view::BuySellMessage::Clipboard(text) => {
                return iced::Task::done(Message::View(view::Message::Clipboard(text)));
            }

            // ModeSelect: setting panel mode (buy or sell)
            view::BuySellMessage::SelectBuyOrSell(bs) => {
                if let BuySellFlowState::ModeSelect { buy_or_sell } = &mut self.step {
                    let bs = Some(bs);

                    // toggle
                    if buy_or_sell == &bs {
                        *buy_or_sell = None;
                    } else {
                        *buy_or_sell = bs;
                    }
                }
            }

            // ip-geolocation logic
            view::BuySellMessage::CountryDetected(result) => {
                let country = match result {
                    Ok(country) => country,
                    Err(err) => {
                        log::error!("Error detecting country via geo-ip, switching to manual country selector.\n    {}", err);

                        self.step = BuySellFlowState::DetectingLocation(true);
                        self.detected_country = None;

                        return iced::Task::done(Message::View(view::Message::BuySell(
                            view::BuySellMessage::SessionError(
                                "Unable to automatically determine location",
                                "please select manually below".to_string(),
                            ),
                        )));
                    }
                };

                // update location information
                log::info!("Country = {}, ISO = {}", country.name, country.code);
                self.detected_country = Some(country);

                return iced::Task::done(Message::View(view::Message::BuySell(
                    view::BuySellMessage::ResetWidget,
                )));
            }

            // session management
            view::BuySellMessage::StartSession => {
                let BuySellFlowState::ModeSelect { buy_or_sell, .. } = &mut self.step else {
                    log::error!("`StartSession` must be always called during the Initialization Flow Stage, skipping...");
                    return iced::Task::none();
                };

                let Some(country) = self.detected_country else {
                    unreachable!(
                        "Unable to start session, country detection|selection was unsuccessful"
                    );
                };

                let buy_or_sell = buy_or_sell.take().unwrap_or(panel::BuyOrSell::Sell);

                match mavapay_supported(country.code) {
                    true => {
                        log::info!("[BUYSELL] Starting under Mavapay for {}", country);

                        // initialize buysell under Mavapay
                        self.step = BuySellFlowState::Mavapay(MavapayState::new(
                            buy_or_sell,
                            match buy_or_sell {
                                BuyOrSell::Sell => {
                                    // initialize default beneficiary
                                    let beneficiary = match country.code {
                                        "KE" => Beneficiary::KES(KenyanBeneficiary::PayToPhone {
                                            account_name: "".to_string(),
                                            phone_number: "+254700000000".to_string(),
                                        }),
                                        "ZA" => Beneficiary::ZAR {
                                            name: "".to_string(),
                                            bank_name: "".to_string(),
                                            bank_account_number: "".to_string(),
                                        },
                                        "NG" => Beneficiary::NGN {
                                            bank_account_name: None,
                                            bank_account_number: "".to_string(),
                                            bank_code: "".to_string(),
                                            bank_name: "".to_string(),
                                        },
                                        iso => unreachable!(
                                            "Country ({}) not supported by Mavapay",
                                            iso
                                        ),
                                    };

                                    MavapayFlowStep::SellInputForm {
                                        banks: None,
                                        beneficiary,
                                        sending_quote: false,
                                        liquid_balance: None,
                                    }
                                }
                                BuyOrSell::Buy => MavapayFlowStep::BuyInputFrom {
                                    getting_invoice: false,
                                    sending_quote: false,
                                    ln_invoice: None,
                                },
                            },
                            country,
                            self.breez_client.clone(),
                        ));

                        if matches!(buy_or_sell, panel::BuyOrSell::Sell) {
                            return iced::Task::batch([
                                iced::Task::done(Message::View(view::Message::BuySell(
                                    view::BuySellMessage::Mavapay(MavapayMessage::GetBanks),
                                ))),
                                iced::Task::done(Message::View(view::Message::BuySell(
                                    view::BuySellMessage::Mavapay(MavapayMessage::GetPrice),
                                ))),
                                iced::Task::done(Message::View(view::Message::BuySell(
                                    view::BuySellMessage::Mavapay(
                                        MavapayMessage::GetLiquidWalletBalance,
                                    ),
                                ))),
                            ]);
                        } else {
                            return iced::Task::done(Message::View(view::Message::BuySell(
                                view::BuySellMessage::Mavapay(MavapayMessage::GetPrice),
                            )));
                        };
                    }
                    false => {
                        log::info!("[BUYSELL] Starting under Meld for {}", country);

                        // initialize buysell under meld
                        let (meld, task) = meld::MeldState::new(
                            buy_or_sell,
                            country,
                            self.coincube_client.clone(),
                            self.network,
                        );
                        self.step = BuySellFlowState::Meld(meld);

                        return task.map(Message::View);
                    }
                }
            }
            view::BuySellMessage::ViewHistory => {
                let Some(country) = self.detected_country else {
                    unreachable!(
                        "Unable to view history, country detection|selection was unsuccessful"
                    );
                };

                match mavapay_supported(country.code) {
                    true => {
                        log::info!("Starting history view under Mavapay");

                        self.step = BuySellFlowState::Mavapay(MavapayState::new(
                            panel::BuyOrSell::Buy,
                            MavapayFlowStep::History {
                                loading: true,
                                transactions: None,
                            },
                            country,
                            self.breez_client.clone(),
                        ));

                        return iced::Task::done(Message::View(view::Message::BuySell(
                            view::BuySellMessage::Mavapay(MavapayMessage::FetchTransactions),
                        )));
                    }
                    // TODO: Implement order history for `meld`
                    false => log::warn!("Meld Transactions History View for {}", country),
                }
            }
            view::BuySellMessage::SessionError(description, error) => {
                // unblock UI retry buttons in step-specific flows
                if let BuySellFlowState::Mavapay(m) = &mut self.step {
                    match m.steps.last_mut() {
                        Some(MavapayFlowStep::BuyInputFrom {
                            getting_invoice,
                            sending_quote,
                            ..
                        }) => {
                            *sending_quote = false;
                            *getting_invoice = false;
                        }
                        Some(MavapayFlowStep::SellInputForm { sending_quote, .. }) => {
                            *sending_quote = false;
                        }
                        Some(MavapayFlowStep::History { loading, .. }) => {
                            *loading = false;
                        }
                        Some(MavapayFlowStep::OrderDetail { loading, .. }) => {
                            *loading = false;
                        }
                        _ => {}
                    }
                }

                if let BuySellFlowState::OtpVerification { sending, .. } = &mut self.step {
                    *sending = false;
                }

                if let BuySellFlowState::Login { loading, .. }
                | BuySellFlowState::Register { loading, .. } = &mut self.step
                {
                    *loading = false;
                }

                // display error using error toast
                return iced::Task::done(Message::View(view::Message::ShowError(format!(
                    "{} ({})",
                    description, error
                ))));
            }

            // state specific messages
            msg => {
                match (&mut self.step, msg) {
                    // user can login from OTP verification or login forms
                    (
                        BuySellFlowState::Login { email, loading },
                        view::BuySellMessage::SubmitLogin,
                    ) => {
                        if *loading {
                            return iced::Task::none();
                        }
                        *loading = true;

                        let client = self.coincube_client.clone();
                        let email = email.to_string();

                        return iced::Task::perform(
                            async move {
                                let send_otp_request = OtpRequest {
                                    email: email.clone(),
                                };
                                client.login_send_otp(send_otp_request).await
                            },
                            |res| match res {
                                Ok(_) => view::BuySellMessage::SendOtp,
                                Err(e) => view::BuySellMessage::SessionError(
                                    "Failed to send OTP",
                                    e.to_string(),
                                ),
                            },
                        )
                        .map(|m| Message::View(view::Message::BuySell(m)));
                    }
                    (BuySellFlowState::Login { email, loading }, view::BuySellMessage::SendOtp) => {
                        if !*loading {
                            // Ignore stale callback from a previous OTP resend
                            return iced::Task::none();
                        }

                        self.step = BuySellFlowState::OtpVerification {
                            email: email.clone(),
                            otp: String::new(),
                            sending: false,
                            is_signup: false,
                            cooldown: 30,
                        };
                    }
                    (
                        BuySellFlowState::OtpVerification { .. } | BuySellFlowState::Login { .. },
                        view::BuySellMessage::LoginSuccess { login },
                    ) => {
                        log::info!("Successfully logged in user: {}", &login.user.email);

                        self.step = BuySellFlowState::ModeSelect { buy_or_sell: None };

                        return iced::Task::done(Message::View(view::Message::BuySell(
                            view::BuySellMessage::SetLoginState(login),
                        )));
                    }
                    // user registration form
                    (BuySellFlowState::Register { email, loading }, msg) => match msg {
                        view::BuySellMessage::EmailChanged(e) => *email = e,

                        view::BuySellMessage::SubmitRegistration => {
                            if *loading {
                                return iced::Task::none();
                            }
                            *loading = true;

                            let client = self.coincube_client.clone();
                            let send_otp_request = OtpRequest {
                                email: email.clone(),
                            };

                            return iced::Task::perform(
                                async move { client.signup_send_otp(send_otp_request).await },
                                |result| match result {
                                    Ok(_) => view::BuySellMessage::RegistrationSuccess,
                                    Err(e) => view::BuySellMessage::SessionError(
                                        "Couldn't process signup request",
                                        e.to_string(),
                                    ),
                                },
                            )
                            .map(|m| Message::View(view::Message::BuySell(m)));
                        }
                        view::BuySellMessage::RegistrationSuccess => {
                            self.step = BuySellFlowState::OtpVerification {
                                email: email.clone(),
                                otp: String::new(),
                                sending: false,
                                is_signup: true,
                                cooldown: 30,
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
                    // OTP verification step
                    (
                        BuySellFlowState::OtpVerification {
                            email,
                            otp,
                            sending,
                            is_signup,
                            cooldown,
                        },
                        msg,
                    ) => match msg {
                        view::BuySellMessage::OtpCooldownTick => {
                            *cooldown = cooldown.saturating_sub(1);
                        }
                        view::BuySellMessage::SendOtp => {
                            if *cooldown > 0 {
                                return iced::Task::none();
                            }
                            *cooldown = 30;

                            match email.get(..8) {
                                Some(e) => {
                                    log::info!("[COINCUBE] Sending OTP to: {}..", e)
                                }
                                None => log::info!("[COINCUBE] Sending OTP"),
                            }

                            let client = self.coincube_client.clone();
                            let send_otp_request = OtpRequest {
                                email: email.clone(),
                            };
                            let is_signup = *is_signup;

                            return iced::Task::perform(
                                async move {
                                    if is_signup {
                                        client.signup_send_otp(send_otp_request).await
                                    } else {
                                        client.login_send_otp(send_otp_request).await
                                    }
                                },
                                |result| match result {
                                    Ok(_) => None,
                                    Err(e) => Some(view::Message::BuySell(
                                        view::BuySellMessage::SessionError(
                                            "Unable to send OTP",
                                            e.to_string(),
                                        ),
                                    )),
                                },
                            )
                            .then(|msg| match msg {
                                Some(msg) => iced::Task::done(Message::View(msg)),
                                None => iced::Task::none(),
                            });
                        }
                        view::BuySellMessage::OtpChanged(o) => *otp = o,
                        view::BuySellMessage::VerifyOtp => {
                            if otp.is_empty() || *sending {
                                return iced::Task::none();
                            }

                            let client = self.coincube_client.clone();
                            let verify_otp_request = OtpVerifyRequest {
                                email: email.clone(),
                                otp: otp.clone(),
                            };
                            *sending = true;
                            let is_signup = *is_signup;

                            return iced::Task::perform(
                                async move {
                                    if is_signup {
                                        client.signup_verify_otp(verify_otp_request).await
                                    } else {
                                        client.login_verify_otp(verify_otp_request).await
                                    }
                                },
                                |result| match result {
                                    Ok(response) => {
                                        view::BuySellMessage::LoginSuccess { login: response }
                                    }
                                    Err(e) => view::BuySellMessage::SessionError(
                                        "Failed to verify OTP",
                                        e.to_string(),
                                    ),
                                },
                            )
                            .map(|m| Message::View(view::Message::BuySell(m)));
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
                    (BuySellFlowState::Login { email, .. }, msg) => match msg {
                        view::BuySellMessage::EmailChanged(e) => *email = e,
                        view::BuySellMessage::CreateNewAccount => {
                            self.step = BuySellFlowState::Register {
                                email: Default::default(),
                                loading: false,
                            };
                        }
                        msg => {
                            log::warn!(
                                "Current {:?} has ignored message: {:?}",
                                self.step.name(),
                                msg
                            )
                        }
                    },
                    (BuySellFlowState::Mavapay(state), view::BuySellMessage::Mavapay(msg)) => {
                        if let Some(task) = state.update(msg, &self.coincube_client) {
                            return task.map(Message::View);
                        };
                    }
                    (BuySellFlowState::Meld(state), view::BuySellMessage::Meld(msg)) => {
                        if let Some(task) = state.update(msg, cache, daemon, &self.coincube_client)
                        {
                            return task.map(Message::View);
                        }
                    }

                    (step, msg) => {
                        log::warn!("Current {:?} has ignored message: {:?}", step.name(), msg)
                    }
                }
            }
        };

        iced::Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> iced::Task<Message> {
        match self.detected_country {
            Some(_) => iced::Task::none(),
            None => {
                let client = self.coincube_client.clone();

                iced::Task::perform(async move { client.locate().await }, |result| {
                    Message::View(view::Message::BuySell(
                        view::BuySellMessage::CountryDetected(result.map_err(|e| e.to_string())),
                    ))
                })
            }
        }
    }

    fn close(&mut self) -> iced::Task<Message> {
        if let BuySellFlowState::Meld(meld) = &self.step {
            if let Some(meld::MeldFlowStep::ActiveSession { active, .. }) = meld.steps.last() {
                if let Some(strong) = std::sync::Weak::upgrade(&active.webview) {
                    let _ = strong.set_visible(false);
                    let _ = strong.focus_parent();
                }
            }
        }

        iced::Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        match &self.step {
            BuySellFlowState::Meld(meld) => {
                let mut subs = vec![];

                // sse subscription
                if let Some(l) = &self.login {
                    subs.push(
                        crate::services::meld::MeldClient::transactions_subscription(
                            l.token.clone(),
                            meld.sse_retries,
                        )
                        .map(|meld| {
                            Message::View(view::Message::BuySell(view::BuySellMessage::Meld(meld)))
                        }),
                    );
                }

                if matches!(
                    meld.steps.last(),
                    Some(meld::MeldFlowStep::ActiveSession { .. })
                ) {
                    // webview subscription
                    subs.push(
                        meld.webview_manager
                            .subscription(std::time::Duration::from_millis(25))
                            .map(|m| {
                                Message::View(view::Message::BuySell(view::BuySellMessage::Meld(
                                    meld::MeldMessage::WebviewManagerUpdate(m),
                                )))
                            }),
                    );
                };

                iced::Subscription::batch(subs)
            }
            // periodically re-fetch the price of BTC
            BuySellFlowState::Mavapay(m)
                if matches!(m.steps.last(), Some(MavapayFlowStep::BuyInputFrom { .. })) =>
            {
                iced::time::every(std::time::Duration::from_secs(30))
                    .map(|_| Message::View(MavapayMessage::GetPrice.into()))
            }
            // SSE stream for transaction status updates during checkout
            BuySellFlowState::Mavapay(m) => {
                if let Some(MavapayFlowStep::Checkout {
                    stream_order_id: Some(order_id),
                    ..
                }) = m.steps.last()
                {
                    if let Some(login) = &self.login {
                        return MavapayClient(&self.coincube_client)
                            .transaction_subscription(order_id.clone(), login.token.clone())
                            .map(|msg| {
                                Message::View(view::Message::BuySell(
                                    view::BuySellMessage::Mavapay(msg),
                                ))
                            });
                    };
                };

                iced::Subscription::none()
            }
            BuySellFlowState::OtpVerification { cooldown, .. } if *cooldown > 0 => {
                iced::time::every(std::time::Duration::from_secs(1)).map(|_| {
                    Message::View(view::Message::BuySell(
                        view::BuySellMessage::OtpCooldownTick,
                    ))
                })
            }
            _ => iced::Subscription::none(),
        }
    }
}
