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
        let inner = view::dashboard(menu, cache, None, self.view());

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
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let message = match message {
            Message::View(view::Message::BuySell(message)) => message,
            // modal for any generated address
            Message::View(view::Message::Select(_)) => {
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
            Message::View(view::Message::ShowQrCode(_)) => {
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
            Message::View(view::Message::Close) => {
                if let BuySellFlowState::Initialization { modal, .. } = &mut self.step {
                    *modal = super::vault::receive::Modal::None;
                }

                return Task::none();
            }
            _ => return Task::none(),
        };

        match message {
            view::BuySellMessage::ResetWidget => {
                self.error = None;

                // attempt automatic refresh from os-keyring
                let mut login = None;

                match keyring::Entry::new("io.coincube.Vault", &self.wallet.name) {
                    Ok(entry) => {
                        if let Ok(user_data) = entry.get_secret() {
                            match serde_json::from_slice::<LoginResponse>(&user_data) {
                                Ok(l) => {
                                    log::trace!("Found login credentials in OS keyring");
                                    login = Some(l)
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

                if self.detected_country.is_some() {
                    match login {
                        None => {
                            // send user to login screen, to initialize login credentials
                            self.step = BuySellFlowState::Login {
                                email: Default::default(),
                                password: Default::default(),
                            };
                        }
                        Some(login) => {
                            // check if token is valid
                            return iced::Task::done(Message::View(view::Message::BuySell(
                                view::BuySellMessage::RefreshLocalLogin(login),
                            )));
                        }
                    }
                } else {
                    log::warn!("Unable to reset widget, country is unknown");
                    self.step = BuySellFlowState::DetectingLocation(true);
                }
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
                if let Ok(entry) = keyring::Entry::new("io.coincube.Vault", &self.wallet.name) {
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
                    password: Default::default(),
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
                return Task::perform(
                    async move { daemon.get_new_address().await },
                    |res| match res {
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
                )
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
                    Ok(country) => {
                        self.error = None;
                        country
                    }
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

                match mavapay_supported(country.code) {
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
                        #[cfg(feature = "meld")]
                        {
                            // initialize buysell under meld
                            let (meld, task) = meld::MeldState::new(
                                buy_or_sell,
                                country,
                                self.coincube_client.clone(),
                            );
                            self.step = BuySellFlowState::Meld(meld);

                            return task.map(Message::View);
                        };

                        #[cfg(not(feature = "meld"))]
                        log::warn!("[BUYSELL] Unable to start buysell under Meld, cargo feature was disabled");
                    }
                }
            }
            view::BuySellMessage::ViewHistory => {
                let Some(country) = self.detected_country.as_ref() else {
                    unreachable!(
                        "Unable to view history, country detection|selection was unsuccessful"
                    );
                };

                match mavapay_supported(country.code) {
                    true => {
                        log::info!("Starting history view under Mavapay");

                        self.step = BuySellFlowState::Mavapay(MavapayState::History {
                            loading: true,
                            error: None,
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
                self.error = Some(error_message.clone());

                // unblock UI retry buttons in step-specific flows
                if let BuySellFlowState::Mavapay(m) = &mut self.step {
                    match m {
                        MavapayState::Transaction { sending_quote, .. } => {
                            *sending_quote = false;
                        }
                        MavapayState::History {
                            loading,
                            error: step_error,
                            ..
                        } => {
                            *loading = false;
                            *step_error = Some(error_message);
                        }
                        MavapayState::OrderDetail { loading, .. } => {
                            *loading = false;
                        }
                        _ => {}
                    }
                }

                if let BuySellFlowState::VerifyEmail { checking, .. } = &mut self.step {
                    *checking = false;
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
                        view::BuySellMessage::SubmitLogin {
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
                                Ok((login, email_verified)) => view::BuySellMessage::LoginSuccess {
                                    email_verified,
                                    login,
                                },
                                Err(e) => view::BuySellMessage::SessionError(
                                    "Failed to submit login",
                                    e.to_string(),
                                ),
                            },
                        )
                        .map(|m| Message::View(view::Message::BuySell(m)));
                    }
                    (
                        BuySellFlowState::VerifyEmail {
                            email, password, ..
                        }
                        | BuySellFlowState::Login {
                            email, password, ..
                        },
                        view::BuySellMessage::LoginSuccess {
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
                    (
                        BuySellFlowState::Register {
                            legal_name,
                            password1,
                            password2,
                            email,
                        },
                        msg,
                    ) => match msg {
                        view::BuySellMessage::LegalNameChanged(n) => *legal_name = n,
                        view::BuySellMessage::EmailChanged(e) => *email = e,
                        view::BuySellMessage::Password1Changed(p) => *password1 = p,
                        view::BuySellMessage::Password2Changed(p) => *password2 = p,

                        view::BuySellMessage::SubmitRegistration => {
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
                                    Ok(_response) => Message::View(view::Message::BuySell(
                                        view::BuySellMessage::RegistrationSuccess,
                                    )),
                                    Err(e) => Message::View(view::Message::BuySell(
                                        view::BuySellMessage::SessionError(
                                            "Couldn't process signup request",
                                            e.to_string(),
                                        ),
                                    )),
                                },
                            );
                        }
                        view::BuySellMessage::RegistrationSuccess => {
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
                        view::BuySellMessage::SendVerificationEmail => {
                            match email.get(..8) {
                                Some(e) => {
                                    log::info!("[COINCUBE] Sending verification email to: {}..", e)
                                }
                                None => log::info!("[COINCUBE] Sending verification email"),
                            }

                            let client = self.coincube_client.clone();
                            let email = email.clone();

                            return Task::perform(
                                async move { client.send_verification_email(&email).await },
                                |result| match result {
                                    Ok(_) => Message::View(view::Message::BuySell(
                                        view::BuySellMessage::CheckEmailVerificationStatus,
                                    )),
                                    Err(e) => Message::View(view::Message::BuySell(
                                        view::BuySellMessage::SessionError(
                                            "Unable to send verification email",
                                            e.to_string(),
                                        ),
                                    )),
                                },
                            );
                        }
                        view::BuySellMessage::CheckEmailVerificationStatus => {
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
                                    Ok(_) => Message::View(view::Message::BuySell(
                                        view::BuySellMessage::SubmitLogin {
                                            skip_email_verification: true,
                                        },
                                    )),
                                    Err(_) => Message::View(view::Message::BuySell(
                                        view::BuySellMessage::EmailVerificationFailed,
                                    )),
                                },
                            );
                        }
                        view::BuySellMessage::EmailVerificationFailed => {
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
                        view::BuySellMessage::LoginUsernameChanged(username) => *email = username,
                        view::BuySellMessage::LoginPasswordChanged(pswd) => *password = pswd,
                        view::BuySellMessage::CreateNewAccount => {
                            self.step = BuySellFlowState::Register {
                                legal_name: Default::default(),
                                password1: Default::default(),
                                password2: Default::default(),
                                email: Default::default(),
                            };
                        }
                        view::BuySellMessage::ResetPassword => {
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
                        view::BuySellMessage::EmailChanged(e) => {
                            *sent = false;
                            *email = e;
                        }
                        view::BuySellMessage::SendPasswordResetEmail => {
                            let email = email.clone();
                            let client = self.coincube_client.clone();

                            return Task::perform(
                                async move { client.send_password_reset_email(&email).await },
                                |res| match res {
                                    Ok(sent) => Message::View(view::Message::BuySell(
                                        view::BuySellMessage::PasswordResetEmailSent(sent.message),
                                    )),
                                    Err(e) => Message::View(view::Message::BuySell(
                                        view::BuySellMessage::SessionError(
                                            "Unable to send password reset email",
                                            e.to_string(),
                                        ),
                                    )),
                                },
                            );
                        }
                        view::BuySellMessage::PasswordResetEmailSent(msg) => {
                            log::info!("[PASSWORD RESET] {}", msg);
                            *sent = true;
                        }
                        view::BuySellMessage::ReturnToLogin => {
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
                    (BuySellFlowState::Mavapay(state), view::BuySellMessage::Mavapay(msg)) => {
                        if let Some(task) = state.update(msg, &self.coincube_client) {
                            return task.map(Message::View);
                        };
                    }
                    #[cfg(feature = "meld")]
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
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<crate::app::wallet::Wallet>,
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
        #[cfg(feature = "meld")]
        if let BuySellFlowState::Meld(meld) = &self.step {
            if let Some(meld::MeldFlowStep::ActiveSession {
                active: Some(wv), ..
            }) = meld.steps.last()
            {
                if let Some(strong) = std::sync::Weak::upgrade(&wv.webview) {
                    let _ = strong.set_visible(false);
                    let _ = strong.focus_parent();
                }
            }
        }

        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        match &self.step {
            #[cfg(feature = "meld")]
            BuySellFlowState::Meld(meld) => {
                let mut subs = vec![];

                if let Some(meld::MeldFlowStep::ActiveSession { .. }) = meld.steps.last() {
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

                    // sse subscription
                    if let Some(l) = &self.login {
                        subs.push(
                            crate::services::meld::MeldClient::transactions_subscription(
                                l.token.clone(),
                            )
                            .map(|meld| {
                                Message::View(view::Message::BuySell(view::BuySellMessage::Meld(
                                    meld,
                                )))
                            }),
                        );
                    }
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
            _ => iced::Subscription::none(),
        }
    }
}
