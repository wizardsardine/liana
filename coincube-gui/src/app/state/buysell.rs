use iced::Task;
use std::sync::Arc;

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        message::Message,
        state::{self, State},
        view::{self, buysell::*},
    },
    daemon::Daemon,
    services::{coincube::*, mavapay::*},
};

impl State for BuySellPanel {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> coincube_ui::widget::Element<'a, view::Message> {
        let inner = view::dashboard(menu, cache, self.view());

        if let BuySellFlowState::Initialization { modal, .. } = &self.step {
            let overlay = match modal {
                super::vault::receive::Modal::VerifyAddress(m) => m.view(),
                super::vault::receive::Modal::ShowQrCode(m) => m.view(),
                super::vault::receive::Modal::None => return inner,
            };

            coincube_ui::widget::modal::Modal::new(inner, overlay)
                .on_blur(Some(view::Message::Close))
                .into()
        } else {
            inner
        }
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let message = match message {
            Message::View(view::Message::BuySell(message)) => message,
            // modal for any generated address
            Message::View(view::Message::Select(_)) => {
                if let BuySellFlowState::Initialization {
                    buy_or_sell: Some(panel::BuyOrSell::Buy { address }),
                    modal,
                    ..
                } = &mut self.step
                {
                    *modal = super::vault::receive::Modal::VerifyAddress(
                        super::vault::receive::VerifyAddressModal::new(
                            cache.datadir_path.clone(),
                            self.wallet.clone(),
                            cache.network,
                            address.address.clone(),
                            address.index,
                        ),
                    );
                }

                return Task::none();
            }
            Message::View(view::Message::ShowQrCode(_)) => {
                if let BuySellFlowState::Initialization {
                    buy_or_sell: Some(panel::BuyOrSell::Buy { address }),
                    modal,
                    ..
                } = &mut self.step
                {
                    if let Some(new) =
                        super::vault::receive::ShowQrCodeModal::new(&address.address, address.index)
                    {
                        *modal = super::vault::receive::Modal::ShowQrCode(new);
                    }
                }

                return Task::none();
            }
            Message::View(view::Message::Close) => {
                if let BuySellFlowState::Initialization { modal, .. } = &mut self.step {
                    *modal = super::vault::receive::Modal::None;
                }

                return Task::none();
            }
            Message::View(view::Message::DismissError) => {
                return Task::none();
            }
            _ => return Task::none(),
        };

        match message {
            view::BuySellMessage::ResetWidget => {
                if self.detected_country.is_none() {
                    log::warn!("Unable to reset widget, country is unknown");
                    self.step = BuySellFlowState::DetectingLocation(true);

                    return iced::Task::none();
                };

                if self.login.as_ref().is_none() {
                    match keyring::Entry::new("io.coincube.Vault", &self.wallet.name) {
                        Ok(entry) => {
                            if let Ok(user_data) = entry.get_secret() {
                                match serde_json::from_slice::<LoginResponse>(&user_data) {
                                    Ok(l) => {
                                        log::trace!("Found login credentials in OS keyring");

                                        // check if token is valid
                                        return iced::Task::done(Message::View(
                                            view::Message::BuySell(
                                                view::BuySellMessage::RefreshLocalLogin(l),
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
                    // User is already logged in and has a country detected - reset to initialization
                    self.step = BuySellFlowState::Initialization {
                        modal: state::vault::receive::Modal::None,
                        buy_or_sell_selected: None,
                        buy_or_sell: None,
                    };
                }
            }

            view::BuySellMessage::BackToAddressView => {
                // Extract buy_or_sell from Mavapay state and go back to Initialization
                let buy_or_sell = match &self.step {
                    BuySellFlowState::Mavapay(mavapay_state) => match mavapay_state {
                        view::buysell::MavapayState::Transaction { buy_or_sell, .. } => {
                            Some(buy_or_sell.clone())
                        }
                        view::buysell::MavapayState::Checkout { buy_or_sell, .. } => {
                            Some(buy_or_sell.clone())
                        }
                        _ => None,
                    },
                    _ => None,
                };

                self.step = BuySellFlowState::Initialization {
                    modal: state::vault::receive::Modal::None,
                    buy_or_sell_selected: buy_or_sell
                        .as_ref()
                        .map(|b| matches!(b, view::buysell::panel::BuyOrSell::Buy { .. })),
                    buy_or_sell,
                };
            }

            // login states
            view::BuySellMessage::RefreshLocalLogin(login) => {
                let client = self.coincube_client.clone();

                return Task::perform(
                    async move { client.refresh_login(&login.refresh_token).await },
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
                match keyring::Entry::new("io.coincube.Vault", &self.wallet.name) {
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
                self.coincube_client = CoincubeClient::new(Some(login.token.clone()));
                self.login = Some(login);

                self.step = BuySellFlowState::Initialization {
                    modal: state::vault::receive::Modal::None,
                    buy_or_sell_selected: None,
                    buy_or_sell: None,
                };
            }
            view::BuySellMessage::LogOut => {
                self.login = None;

                // clear keyring credentials
                if let Ok(entry) = keyring::Entry::new("io.coincube.Vault", &self.wallet.name) {
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
                return Task::done(Message::View(view::Message::Clipboard(text)));
            }

            // initialization flow: for creating a new address and setting panel mode (buy or sell)
            view::BuySellMessage::SelectBuyOrSell(bs) => {
                if let BuySellFlowState::Initialization {
                    buy_or_sell_selected,
                    ..
                } = &mut self.step
                {
                    if *buy_or_sell_selected == Some(bs) {
                        // toggle off
                        *buy_or_sell_selected = None;
                    } else {
                        *buy_or_sell_selected = Some(bs)
                    }
                }
            }
            view::BuySellMessage::CreateNewAddress => {
                let daemon = daemon.expect("Daemon must be available for BuySell panel");
                return Task::perform(
                    async move { daemon.get_new_address().await },
                    |res: Result<_, _>| match res {
                        Ok(out) => Message::View(view::Message::BuySell(
                            view::BuySellMessage::AddressCreated(
                                view::buysell::panel::LabelledAddress {
                                    address: out.address,
                                    index: out.derivation_index,
                                    label: None,
                                },
                            ),
                        )),
                        Err(e) => Message::View(view::Message::BuySell(
                            view::BuySellMessage::SessionError(
                                "Unable to create a new address",
                                e.to_string(),
                            ),
                        )),
                    },
                );
            }
            view::BuySellMessage::AddressCreated(address) => {
                if let BuySellFlowState::Initialization { buy_or_sell, .. } = &mut self.step {
                    *buy_or_sell = Some(panel::BuyOrSell::Buy { address })
                }
            }

            // ip-geolocation logic
            view::BuySellMessage::CountryDetected(result) => {
                // TODO: state/region detection for select countries

                let country = match result {
                    Ok(country) => country,
                    Err(err) => {
                        log::error!("Error detecting country via geo-ip, switching to manual country selector.\n    {}", err);

                        self.step = BuySellFlowState::DetectingLocation(true);
                        self.detected_country = None;

                        return Task::done(Message::View(view::Message::BuySell(
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

                return Task::done(Message::View(view::Message::BuySell(
                    view::BuySellMessage::ResetWidget,
                )));
            }

            // session management
            view::BuySellMessage::StartSession => {
                let BuySellFlowState::Initialization { buy_or_sell, .. } = &mut self.step else {
                    log::error!("`StartSession` must be always called during the Initialization Flow Stage, skipping...");
                    return Task::none();
                };

                let Some(country) = self.detected_country else {
                    unreachable!(
                        "Unable to start session, country detection|selection was unsuccessful"
                    );
                };

                let buy_or_sell = buy_or_sell.take().unwrap_or(panel::BuyOrSell::Sell);

                match mavapay_supported(country.code)
                    && matches!(option_env!("ENABLE_MAVAPAY"), Some("1") | Some("true"))
                {
                    true => {
                        log::info!("[BUYSELL] Starting under Mavapay for {}", country);

                        // initialize buysell under Mavapay
                        self.step = BuySellFlowState::Mavapay(MavapayState::Transaction {
                            buy_or_sell,
                            country: country.clone(),
                            sat_amount: 6000,
                            beneficiary: None,
                            transfer_speed: OnchainTransferSpeed::Fast,
                            banks: None,
                            selected_bank: None,
                            btc_price: None,
                            sending_quote: false,
                        });

                        if country.code != "KE" {
                            return Task::batch([
                                Task::done(Message::View(view::Message::BuySell(
                                    view::BuySellMessage::Mavapay(MavapayMessage::GetBanks),
                                ))),
                                Task::done(Message::View(view::Message::BuySell(
                                    view::BuySellMessage::Mavapay(MavapayMessage::GetPrice),
                                ))),
                            ]);
                        } else {
                            return Task::done(Message::View(view::Message::BuySell(
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
                        );
                        self.step = BuySellFlowState::Meld(meld);

                        return task.map(Message::View);
                    }
                }
            }
            view::BuySellMessage::ViewHistory => {
                let Some(country) = self.detected_country.as_ref() else {
                    unreachable!(
                        "Unable to view history, country detection|selection was unsuccessful"
                    );
                };

                match mavapay_supported(country.code)
                    && matches!(option_env!("ENABLE_MAVAPAY"), Some("1") | Some("true"))
                {
                    true => {
                        log::info!("Starting history view under Mavapay");

                        self.step = BuySellFlowState::Mavapay(MavapayState::History {
                            loading: true,
                            transactions: None,
                        });

                        return Task::done(Message::View(view::Message::BuySell(
                            view::BuySellMessage::Mavapay(MavapayMessage::FetchTransactions),
                        )));
                    }
                    false => {
                        // TODO: Implement order history for `meld`
                        log::info!("Starting history view under Meld for {}", country);
                    }
                }
            }
            view::BuySellMessage::SessionError(description, error) => {
                let error_message = format!("{} ({})", description, error);

                // unblock UI retry buttons in step-specific flows
                if let BuySellFlowState::Mavapay(m) = &mut self.step {
                    match m {
                        MavapayState::Transaction { sending_quote, .. } => {
                            *sending_quote = false;
                        }
                        MavapayState::History { loading, .. } => {
                            *loading = false;
                        }
                        MavapayState::OrderDetail { loading, .. } => {
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
                return iced::Task::done(Message::View(view::Message::ShowError(error_message)));
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
                            return Task::none();
                        }
                        *loading = true;

                        let client = self.coincube_client.clone();
                        let email = email.to_string();

                        return Task::perform(
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
                            return Task::none();
                        }
                        self.step = BuySellFlowState::OtpVerification {
                            email: email.clone(),
                            otp: String::new(),
                            sending: false,
                            is_signup: false,
                            cooldown: 30,
                        };
                        return Task::none();
                    }
                    (
                        BuySellFlowState::OtpVerification { .. } | BuySellFlowState::Login { .. },
                        view::BuySellMessage::LoginSuccess { login },
                    ) => {
                        log::info!("Successfully logged in user: {}", &login.user.email);
                        self.step = BuySellFlowState::Initialization {
                            modal: state::vault::receive::Modal::None,
                            buy_or_sell_selected: None,
                            buy_or_sell: None,
                        };

                        return Task::done(Message::View(view::Message::BuySell(
                            view::BuySellMessage::SetLoginState(login),
                        )));
                    }
                    // user registration form
                    (BuySellFlowState::Register { email, loading }, msg) => match msg {
                        view::BuySellMessage::EmailChanged(e) => *email = e,

                        view::BuySellMessage::SubmitRegistration => {
                            if *loading {
                                return Task::none();
                            }
                            *loading = true;

                            let client = self.coincube_client.clone();
                            let send_otp_request = OtpRequest {
                                email: email.clone(),
                            };

                            return Task::perform(
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
                                return Task::none();
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

                            return Task::perform(
                                async move {
                                    if is_signup {
                                        client.signup_send_otp(send_otp_request).await
                                    } else {
                                        client.login_send_otp(send_otp_request).await
                                    }
                                },
                                |result| match result {
                                    Ok(_) => view::Message::DismissError,
                                    Err(e) => {
                                        view::Message::BuySell(view::BuySellMessage::SessionError(
                                            "Unable to send OTP",
                                            e.to_string(),
                                        ))
                                    }
                                },
                            )
                            .map(Message::View);
                        }
                        view::BuySellMessage::OtpChanged(o) => *otp = o,
                        view::BuySellMessage::VerifyOtp => {
                            if otp.is_empty() || *sending {
                                return Task::none();
                            }

                            let client = self.coincube_client.clone();
                            let verify_otp_request = OtpVerifyRequest {
                                email: email.clone(),
                                otp: otp.clone(),
                            };
                            *sending = true;
                            let is_signup = *is_signup;

                            return Task::perform(
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
                        if let Some(task) = state.update(msg, cache, &self.coincube_client) {
                            return task.map(Message::View);
                        }
                    }

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
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> Task<Message> {
        match self.detected_country {
            Some(_) => Task::none(),
            None => {
                let client = self.coincube_client.clone();

                Task::perform(async move { client.locate().await }, |result| {
                    Message::View(view::Message::BuySell(
                        view::BuySellMessage::CountryDetected(result.map_err(|e| e.to_string())),
                    ))
                })
            }
        }
    }

    fn close(&mut self) -> Task<Message> {
        if let BuySellFlowState::Meld(meld) = &self.step {
            if let Some(meld::MeldFlowStep::ActiveSession { active, .. }) = meld.steps.last() {
                if let Some(strong) = std::sync::Weak::upgrade(&active.webview) {
                    let _ = strong.set_visible(false);
                    let _ = strong.focus_parent();
                }
            }
        }

        Task::none()
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
            BuySellFlowState::Mavapay(MavapayState::Transaction { .. }) => {
                iced::time::every(std::time::Duration::from_secs(30)).map(|_| {
                    Message::View(view::Message::BuySell(view::BuySellMessage::Mavapay(
                        MavapayMessage::GetPrice,
                    )))
                })
            }
            // SSE stream for transaction status updates during checkout
            BuySellFlowState::Mavapay(MavapayState::Checkout {
                stream_order_id: Some(order_id),
                ..
            }) => {
                if let Some(login) = &self.login {
                    MavapayClient(&self.coincube_client)
                        .transaction_subscription(order_id.clone(), login.token.clone())
                        .map(|msg| {
                            Message::View(view::Message::BuySell(view::BuySellMessage::Mavapay(
                                msg,
                            )))
                        })
                } else {
                    iced::Subscription::none()
                }
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
