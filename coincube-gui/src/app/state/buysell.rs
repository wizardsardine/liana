use std::sync::Arc;

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        message::Message,
        state::{
            connect::{CONNECT_KEYRING_SERVICE, CONNECT_KEYRING_USER},
            State,
        },
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
        // No pinned page head (plan L3 exception): Buy/Sell keeps its own
        // heading and Back bar inside the provider column, whose steps own
        // them (`view::buysell::panel`).
        view::dashboard(menu, cache, self.view(cache.balance_amounts_masked()))
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

        // Replacing the flow while a sell payment is with the Spark bridge
        // would drop the checkout that is waiting for its answer, and with it
        // the only record of whether the order was paid. ("Start New
        // Transaction" sends `ResetWidget`, which can arrive once Mavapay has
        // already confirmed the order.)
        if self.sell_payment_in_flight() {
            match message {
                // A refreshed token must still be stored and a logout still
                // happen: hold them until the payment answers.
                view::BuySellMessage::SetLoginState(..) | view::BuySellMessage::LogOut => {
                    log::info!(
                        "[BUYSELL] Deferring a session change until the sell payment answers"
                    );
                    self.deferred_session.push(message);
                    return iced::Task::none();
                }
                view::BuySellMessage::ResetWidget
                | view::BuySellMessage::CountryDetected(..)
                | view::BuySellMessage::StartSession
                | view::BuySellMessage::ViewHistory => {
                    log::warn!(
                        "[BUYSELL] Ignoring a flow change while a sell payment is in flight"
                    );
                    return iced::Task::none();
                }
                _ => {}
            }
        }

        match message {
            view::BuySellMessage::ResetWidget => {
                if self.detected_country.is_none() {
                    log::warn!("Unable to reset widget, country is unknown");
                    self.step = BuySellFlowState::DetectingLocation(true);

                    return iced::Task::none();
                }

                if self.login.as_ref().is_none() {
                    // 1. Try the shared Connect global keyring first
                    let connect_session =
                        keyring::Entry::new(CONNECT_KEYRING_SERVICE, CONNECT_KEYRING_USER)
                            .ok()
                            .and_then(|e| e.get_secret().ok())
                            .and_then(|b| serde_json::from_slice::<LoginResponse>(&b).ok());

                    if let Some(l) = connect_session {
                        log::trace!("[BUYSELL] Found shared Connect session in keyring");
                        return iced::Task::done(Message::View(view::Message::BuySell(
                            view::BuySellMessage::RefreshLogin {
                                refresh_token: l.refresh_token,
                            },
                        )));
                    }

                    // 2. Fall back to the legacy wallet-scoped keyring
                    match keyring::Entry::new(KEYRING_SERVICE_NAME, &self.wallet.name) {
                        Ok(entry) => {
                            if let Ok(user_data) = entry.get_secret() {
                                match serde_json::from_slice::<LoginResponse>(&user_data) {
                                    Ok(l) => {
                                        log::trace!("Found login credentials in OS keyring");
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
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Unable to restore login state from OS keyring: {e}");
                        }
                    }

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
                let bytes = serde_json::to_vec(&login).unwrap();

                // write to the shared Connect global keyring
                match keyring::Entry::new(CONNECT_KEYRING_SERVICE, CONNECT_KEYRING_USER) {
                    Ok(entry) => {
                        let _ = entry.delete_credential();
                        if let Err(e) = entry.set_secret(&bytes) {
                            log::error!("[BUYSELL] Unable to write shared Connect keyring: {e}");
                        }
                    }
                    Err(e) => log::error!("[BUYSELL] Connect keyring inaccessible: {e}"),
                }

                // also write to the legacy wallet-scoped keyring
                match keyring::Entry::new(KEYRING_SERVICE_NAME, &self.wallet.name) {
                    Ok(entry) => {
                        let _ = entry.delete_credential();
                        if let Err(e) = entry.set_secret(&bytes) {
                            log::error!("Unable to store user data in keyring: {e}");
                        }
                    }
                    Err(err) => {
                        log::error!(
                            "[BUYSELL] Unable to persist login state, keyring inaccessible: {}",
                            err
                        )
                    }
                }

                // user is successfully logged in: 🥳
                self.coincube_client.set_token(&login.token);
                self.login = Some(login);

                self.step = BuySellFlowState::ModeSelect { buy_or_sell: None };
            }
            view::BuySellMessage::LogOut => {
                let was_logged_in = self.login.is_some();
                self.login = None;

                // clear shared Connect global keyring
                if let Ok(entry) =
                    keyring::Entry::new(CONNECT_KEYRING_SERVICE, CONNECT_KEYRING_USER)
                {
                    let _ = entry.delete_credential();
                }

                // clear legacy wallet-scoped keyring
                if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE_NAME, &self.wallet.name) {
                    if let Err(e) = entry.delete_credential() {
                        log::error!("[BUYSELL] Unable to delete credentials from OS keyring: {e:?}")
                    }
                }

                // send user to login screen, to re-initialize login credentials
                self.step = BuySellFlowState::Login {
                    email: Default::default(),
                    loading: false,
                };

                // notify Connect module so it clears its in-memory session too,
                // but only if this logout originated here (not forwarded from Connect)
                if was_logged_in {
                    return iced::Task::done(Message::View(view::Message::ConnectAccount(
                        view::ConnectAccountMessage::LogOut,
                    )));
                }
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
                                        spark_balance: None,
                                    }
                                }
                                BuyOrSell::Buy => MavapayFlowStep::BuyInputFrom {
                                    getting_invoice: false,
                                    sending_quote: false,
                                    ln_invoice: None,
                                },
                            },
                            country,
                            self.spark.clone(),
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
                                    view::BuySellMessage::Mavapay(MavapayMessage::GetSparkBalance),
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
                            self.spark.clone(),
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

                // `description` is our internal step code and `error` is the
                // provider's raw message; neither belongs on screen. The code
                // becomes the support reference instead, so the pair is still
                // recoverable from the log.
                let failure = crate::user_error::UserError::logged(
                    "That didn't work",
                    "Check the details and try again. If it keeps failing, contact support and quote the reference below.",
                    description,
                    true,
                    &error,
                );
                // Same line as `UserError::toast()`. Composed here because the
                // reference is a step code, not a `CC-…` code, so the notice
                // lookup at display can't split and translate it.
                let toast = coincube_ui::localized_format!(
                    "{} — {} (Ref: {})",
                    "{} — {} (Ref.: {})",
                    coincube_ui::locale::copy(&failure.title),
                    coincube_ui::locale::copy(&failure.guidance),
                    failure.reference
                );
                return iced::Task::done(Message::View(view::Message::ShowError(toast)));
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
                        log::info!("Successfully logged in user: {}", login.user.email);

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
                        let task = state
                            .update(msg, &self.coincube_client)
                            .map(|task| task.map(Message::View));
                        // Once the sell payment is resolved, apply any
                        // session change held back meanwhile, in order.
                        let deferred = self.take_deferred_session();
                        if task.is_some() || deferred.is_some() {
                            return iced::Task::batch(task.into_iter().chain(deferred));
                        }
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
        }

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
                }

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
                    }
                }

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

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::{descriptors::CoincubeDescriptor, miniscript::bitcoin::Network};
    use std::{str::FromStr, sync::Arc};

    const DESC: &str = "wsh(or_d(multi(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[de6eb005/48'/1'/0'/2']tpubDFGuYfS2JwiUSEXiQuNGdT3R7WTDhbaE6jbUhgYSSdhmfQcSx7ZntMPPv7nrkvAqjpj3jX9wbhSGMeKVao4qAzhbNyBi7iQmv5xxQk6H6jz/<0;1>/*),and_v(v:pkh([ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*),older(3))))#p9ax3xxp";

    fn wallet() -> Arc<crate::app::wallet::Wallet> {
        Arc::new(crate::app::wallet::Wallet::new(
            CoincubeDescriptor::from_str(DESC).unwrap(),
        ))
    }

    fn spark() -> Arc<crate::app::wallets::SparkBackend> {
        Arc::new(crate::app::wallets::SparkBackend::new(Arc::new(
            crate::app::breez_spark::SparkClient::disconnected_for_test(),
        )))
    }

    fn panel() -> BuySellPanel {
        BuySellPanel::new(Network::Bitcoin, wallet(), spark())
    }

    fn country(code: &str) -> &'static Country {
        crate::services::coincube::get_countries()
            .iter()
            .find(|country| country.code == code)
            .unwrap()
    }

    fn login() -> LoginResponse {
        LoginResponse {
            requires_2fa: false,
            token: "token".to_string(),
            refresh_token: "refresh-token".to_string(),
            user: User {
                id: 7,
                email: "user@example.com".to_string(),
                email_verified: Some(true),
            },
        }
    }

    fn buysell_msg(msg: view::BuySellMessage) -> Message {
        Message::View(view::Message::BuySell(msg))
    }

    #[test]
    fn reset_widget_without_country_prompts_manual_location() {
        let mut panel = panel();
        panel.step = BuySellFlowState::ModeSelect { buy_or_sell: None };

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::ResetWidget),
        );

        assert!(matches!(
            panel.step,
            BuySellFlowState::DetectingLocation(true)
        ));
    }

    #[test]
    fn reset_widget_with_login_and_country_returns_to_mode_select() {
        let mut panel = panel();
        panel.detected_country = Some(country("US"));
        panel.login = Some(login());

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::ResetWidget),
        );

        assert!(matches!(
            panel.step,
            BuySellFlowState::ModeSelect { buy_or_sell: None }
        ));
    }

    #[test]
    fn mode_select_toggles_buy_and_sell_choices() {
        let mut panel = panel();
        panel.step = BuySellFlowState::ModeSelect { buy_or_sell: None };

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::SelectBuyOrSell(BuyOrSell::Sell)),
        );
        assert!(matches!(
            panel.step,
            BuySellFlowState::ModeSelect {
                buy_or_sell: Some(BuyOrSell::Sell)
            }
        ));

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::SelectBuyOrSell(BuyOrSell::Sell)),
        );
        assert!(matches!(
            panel.step,
            BuySellFlowState::ModeSelect { buy_or_sell: None }
        ));

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::SelectBuyOrSell(BuyOrSell::Buy)),
        );
        assert!(matches!(
            panel.step,
            BuySellFlowState::ModeSelect {
                buy_or_sell: Some(BuyOrSell::Buy)
            }
        ));
    }

    #[test]
    fn country_detection_success_and_failure_update_location_state() {
        let mut panel = panel();

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::CountryDetected(Ok(country("ZA")))),
        );
        assert_eq!(
            panel.detected_country.map(|country| country.code),
            Some("ZA")
        );

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::CountryDetected(Err(
                "geo lookup failed".to_string(),
            ))),
        );
        assert!(panel.detected_country.is_none());
        assert!(matches!(
            panel.step,
            BuySellFlowState::DetectingLocation(true)
        ));
    }

    #[test]
    fn login_and_registration_forms_advance_to_otp_verification() {
        let mut panel = panel();
        panel.step = BuySellFlowState::Login {
            email: String::new(),
            loading: false,
        };

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::EmailChanged(
                "login@example.com".to_string(),
            )),
        );
        assert!(matches!(
            &panel.step,
            BuySellFlowState::Login { email, loading: false } if email == "login@example.com"
        ));

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::SubmitLogin),
        );
        assert!(matches!(
            panel.step,
            BuySellFlowState::Login { loading: true, .. }
        ));

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::SendOtp),
        );
        assert!(matches!(
            &panel.step,
            BuySellFlowState::OtpVerification {
                email,
                otp,
                sending: false,
                is_signup: false,
                cooldown: 30
            } if email == "login@example.com" && otp.is_empty()
        ));

        panel.step = BuySellFlowState::Login {
            email: "old@example.com".to_string(),
            loading: false,
        };
        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::CreateNewAccount),
        );
        assert!(matches!(
            panel.step,
            BuySellFlowState::Register { loading: false, .. }
        ));

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::EmailChanged(
                "signup@example.com".to_string(),
            )),
        );
        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::SubmitRegistration),
        );
        assert!(matches!(
            panel.step,
            BuySellFlowState::Register { loading: true, .. }
        ));

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::RegistrationSuccess),
        );
        assert!(matches!(
            &panel.step,
            BuySellFlowState::OtpVerification {
                email,
                is_signup: true,
                cooldown: 30,
                ..
            } if email == "signup@example.com"
        ));
    }

    #[test]
    fn otp_state_edits_cooldown_and_verify_in_flight() {
        let mut panel = panel();
        panel.step = BuySellFlowState::OtpVerification {
            email: "user@example.com".to_string(),
            otp: String::new(),
            sending: false,
            is_signup: false,
            cooldown: 1,
        };

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::OtpCooldownTick),
        );
        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::OtpCooldownTick),
        );
        assert!(matches!(
            panel.step,
            BuySellFlowState::OtpVerification { cooldown: 0, .. }
        ));

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::OtpChanged("123456".to_string())),
        );
        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::VerifyOtp),
        );
        assert!(matches!(
            &panel.step,
            BuySellFlowState::OtpVerification {
                otp,
                sending: true,
                ..
            } if otp == "123456"
        ));
    }

    fn sell_checkout(quote_id: &str, fulfilment: SellFulfilment) -> MavapayState {
        let quote = crate::app::view::buysell::mavapay::tests::sell_quote(quote_id);
        MavapayState::new(
            BuyOrSell::Sell,
            MavapayFlowStep::Checkout {
                quote,
                fulfilled_order: None,
                invoice_qr_code_data: None,
                spark_balance: Some(10_000_000),
                sell: SellPayment {
                    fulfilment,
                    ..SellPayment::default()
                },
                stream_order_id: None,
            },
            country("NG"),
            spark(),
        )
    }

    fn checkout_fulfilment(panel: &BuySellPanel) -> Option<(&str, &SellFulfilment)> {
        match &panel.step {
            BuySellFlowState::Mavapay(state) => match state.steps.last() {
                Some(MavapayFlowStep::Checkout { quote, sell, .. }) => {
                    Some((quote.id.as_str(), &sell.fulfilment))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// "Start New Transaction" (and every other flow change) is refused while
    /// a sell payment is with the bridge, and a late success for an earlier
    /// order doesn't settle the checkout that replaced it.
    #[test]
    fn a_sell_payment_in_flight_survives_resets_and_late_answers_stay_with_their_order() {
        let mut panel = panel();
        panel.detected_country = Some(country("NG"));
        panel.step = BuySellFlowState::Mavapay(sell_checkout("quote-a", SellFulfilment::Sending));
        assert!(panel.sell_payment_in_flight());
        for change in [
            view::BuySellMessage::ResetWidget,
            view::BuySellMessage::StartSession,
            view::BuySellMessage::ViewHistory,
            view::BuySellMessage::LogOut,
        ] {
            let _ = panel.update(None, &Cache::default(), buysell_msg(change));
            assert_eq!(
                checkout_fulfilment(&panel),
                Some(("quote-a", &SellFulfilment::Sending))
            );
        }

        // Order A settled elsewhere and the user moved on to order B, whose
        // own payment is now out. A's late success must not mark B paid.
        panel.step = BuySellFlowState::Mavapay(sell_checkout("quote-b", SellFulfilment::Sending));
        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::Mavapay(
                MavapayMessage::SellInvoiceFulfilled {
                    quote_id: "quote-a".to_string(),
                    payment: coincube_spark_protocol::SendPaymentOk {
                        payment_id: "pay-a".to_string(),
                        amount_sat: 2_500_000,
                        fee_sat: 3,
                        status: Some("Completed".to_string()),
                    },
                },
            )),
        );
        assert_eq!(
            checkout_fulfilment(&panel),
            Some(("quote-b", &SellFulfilment::Sending))
        );

        // A logout (or a refreshed login) arriving mid-payment is held, not
        // dropped, and applied once the payment answers.
        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::LogOut),
        );
        // Two held: this one and the `LogOut` from the loop above.
        assert_eq!(panel.deferred_session.len(), 2);
        assert_eq!(
            checkout_fulfilment(&panel),
            Some(("quote-b", &SellFulfilment::Sending))
        );
        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::Mavapay(
                MavapayMessage::SellSendFailed {
                    quote_id: "quote-b".to_string(),
                    error: "no route".to_string(),
                },
            )),
        );
        assert_eq!(
            checkout_fulfilment(&panel),
            Some(("quote-b", &SellFulfilment::Idle))
        );
        assert!(
            panel.deferred_session.is_empty(),
            "the held logout was re-dispatched once the payment answered"
        );

        // At rest, a reset goes through as before.
        panel.step = BuySellFlowState::Mavapay(sell_checkout("quote-b", SellFulfilment::Idle));
        assert!(!panel.sell_payment_in_flight());
    }

    /// A check that can't settle the payment doesn't release the held
    /// session changes: they would replace the checkout and drop its key and
    /// "Check again". Only a settled payment does, and the held changes come
    /// back as one ordered task.
    #[test]
    fn held_session_changes_wait_for_a_settled_payment() {
        use crate::app::state::spark::send::{ReconcileOutcome, RetryGuard};
        let mut panel = panel();
        panel.detected_country = Some(country("NG"));
        panel.step = BuySellFlowState::Mavapay(sell_checkout(
            "quote-a",
            SellFulfilment::OutcomeUnknown {
                checking: true,
                outcome: None,
                guard: RetryGuard::IdempotencyKey,
            },
        ));
        for change in [view::BuySellMessage::LogOut, view::BuySellMessage::LogOut] {
            let _ = panel.update(None, &Cache::default(), buysell_msg(change));
        }
        assert_eq!(panel.deferred_session.len(), 2);

        let checked = |outcome| {
            buysell_msg(view::BuySellMessage::Mavapay(
                MavapayMessage::SellPaymentChecked {
                    quote_id: "quote-a".to_string(),
                    outcome,
                },
            ))
        };
        // Inconclusive: still unresolved, so nothing is released.
        let _ = panel.update(
            None,
            &Cache::default(),
            checked(ReconcileOutcome::Inconclusive("still in progress".into())),
        );
        assert!(panel.sell_payment_in_flight());
        assert_eq!(panel.deferred_session.len(), 2);
        assert!(panel.take_deferred_session().is_none());

        // Checked again and found landed: released, in one chained task.
        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::Mavapay(
                MavapayMessage::CheckSellPayment,
            )),
        );
        let _ = panel.update(None, &Cache::default(), checked(ReconcileOutcome::Landed));
        assert_eq!(
            checkout_fulfilment(&panel),
            Some(("quote-a", &SellFulfilment::Paid))
        );
        assert!(panel.deferred_session.is_empty());
    }

    #[test]
    fn start_session_initializes_mavapay_sell_for_supported_country() {
        let mut panel = panel();
        panel.detected_country = Some(country("KE"));
        panel.step = BuySellFlowState::ModeSelect {
            buy_or_sell: Some(BuyOrSell::Sell),
        };

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::StartSession),
        );

        let BuySellFlowState::Mavapay(state) = &panel.step else {
            panic!("expected Mavapay state");
        };
        assert_eq!(state.buy_or_sell, BuyOrSell::Sell);
        assert_eq!(state.country.code, "KE");
        assert!(matches!(
            state.steps.last(),
            Some(MavapayFlowStep::SellInputForm {
                beneficiary: Beneficiary::KES(KenyanBeneficiary::PayToPhone {
                    phone_number,
                    ..
                }),
                sending_quote: false,
                spark_balance: None,
                ..
            }) if phone_number == "+254700000000"
        ));
    }

    #[test]
    fn start_session_initializes_mavapay_buy_for_supported_country() {
        let mut panel = panel();
        panel.detected_country = Some(country("NG"));
        panel.step = BuySellFlowState::ModeSelect {
            buy_or_sell: Some(BuyOrSell::Buy),
        };

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::StartSession),
        );

        let BuySellFlowState::Mavapay(state) = &panel.step else {
            panic!("expected Mavapay state");
        };
        assert_eq!(state.buy_or_sell, BuyOrSell::Buy);
        assert!(matches!(
            state.steps.last(),
            Some(MavapayFlowStep::BuyInputFrom {
                getting_invoice: false,
                sending_quote: false,
                ln_invoice: None
            })
        ));
    }

    #[test]
    fn view_history_uses_mavapay_history_for_supported_country() {
        let mut panel = panel();
        panel.detected_country = Some(country("ZA"));
        panel.step = BuySellFlowState::ModeSelect { buy_or_sell: None };

        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::ViewHistory),
        );

        assert!(matches!(
            &panel.step,
            BuySellFlowState::Mavapay(state)
                if matches!(
                    state.steps.last(),
                    Some(MavapayFlowStep::History {
                        loading: true,
                        transactions: None
                    })
                )
        ));
    }

    #[test]
    fn session_error_unblocks_loading_flags() {
        let mut panel = panel();
        panel.step = BuySellFlowState::Login {
            email: "user@example.com".to_string(),
            loading: true,
        };
        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::SessionError(
                "failed",
                "network".to_string(),
            )),
        );
        assert!(matches!(
            panel.step,
            BuySellFlowState::Login { loading: false, .. }
        ));

        panel.step = BuySellFlowState::Register {
            email: "user@example.com".to_string(),
            loading: true,
        };
        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::SessionError(
                "failed",
                "network".to_string(),
            )),
        );
        assert!(matches!(
            panel.step,
            BuySellFlowState::Register { loading: false, .. }
        ));

        panel.step = BuySellFlowState::OtpVerification {
            email: "user@example.com".to_string(),
            otp: "123456".to_string(),
            sending: true,
            is_signup: false,
            cooldown: 0,
        };
        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::SessionError(
                "failed",
                "network".to_string(),
            )),
        );
        assert!(matches!(
            panel.step,
            BuySellFlowState::OtpVerification { sending: false, .. }
        ));

        panel.step = BuySellFlowState::Mavapay(MavapayState::new(
            BuyOrSell::Buy,
            MavapayFlowStep::BuyInputFrom {
                ln_invoice: None,
                getting_invoice: true,
                sending_quote: true,
            },
            country("NG"),
            spark(),
        ));
        let _ = panel.update(
            None,
            &Cache::default(),
            buysell_msg(view::BuySellMessage::SessionError(
                "failed",
                "network".to_string(),
            )),
        );
        assert!(matches!(
            &panel.step,
            BuySellFlowState::Mavapay(state)
                if matches!(
                    state.steps.last(),
                    Some(MavapayFlowStep::BuyInputFrom {
                        getting_invoice: false,
                        sending_quote: false,
                        ..
                    })
                )
        ));
    }
}
