use coincube_ui::{component::form, widget::*};
use iced::Task;
use zeroize::Zeroizing;

use crate::{
    hw::HardwareWallets,
    installer::{
        context::Context,
        message::{CoincubeConnectMsg, Message},
        step::Step,
        view,
    },
    services::coincube::{CoincubeClient, OtpRequest, OtpVerifyRequest},
};

pub struct CoincubeConnectStep {
    client: CoincubeClient,
    email: form::Value<String>,
    email_touched: bool,
    otp: form::Value<String>,
    otp_sent: bool,
    is_signup: bool,
    /// JWT captured from the OTP-verify response, held on the step
    /// between `update(OtpVerified)` and the next `apply()`. Wrapped in
    /// `Zeroizing` so the heap allocation is scrubbed on drop; `apply`
    /// moves the value into `Context` with `take()` so no plaintext
    /// copy lingers on the step after hand-off. Navigating back past
    /// this step can't re-trigger `apply` — `skip()` short-circuits
    /// while `ctx.connect_jwt.is_some()` — so `take` is safe.
    jwt: Option<Zeroizing<String>>,
    processing: bool,
    error: Option<String>,
    skipped: bool,
    /// Set by `load_context` when the installer was launched with an
    /// already-authenticated Connect session (e.g. Vault setup started
    /// from Home while signed in). Tells `load()` to fire `Message::Next`
    /// so the step auto-advances past the redundant email + OTP form,
    /// adopting the existing token instead of asking the user to
    /// re-authenticate the same account.
    preauthenticated: bool,
}

impl CoincubeConnectStep {
    pub fn new() -> Self {
        Self {
            client: CoincubeClient::new(),
            email: form::Value {
                valid: false,
                ..form::Value::default()
            },
            email_touched: false,
            otp: form::Value::default(),
            otp_sent: false,
            is_signup: true,
            jwt: None,
            processing: false,
            error: None,
            skipped: false,
            preauthenticated: false,
        }
    }
}

impl Default for CoincubeConnectStep {
    fn default() -> Self {
        Self::new()
    }
}

impl From<CoincubeConnectStep> for Box<dyn Step> {
    fn from(s: CoincubeConnectStep) -> Box<dyn Step> {
        Box::new(s)
    }
}

async fn send_otp(client: CoincubeClient, email: String, is_signup: bool) -> Result<(), String> {
    let req = OtpRequest { email };
    if is_signup {
        client.signup_send_otp(req).await
    } else {
        client.login_send_otp(req).await
    }
    .map_err(|e| e.to_string())
}

impl Step for CoincubeConnectStep {
    /// Adopt an already-authenticated Connect session forwarded from the
    /// app (`ctx.coincube_client`) the first time this step becomes
    /// active. When a Vault is set up from Home while signed in, the
    /// home's session is threaded into the installer; without this hook
    /// the step would start at the email form and force a second
    /// email + OTP round on the same account — the redundant-login bug.
    ///
    /// Guards:
    ///   * `self.jwt.is_some()` / `self.otp_sent` — the user has already
    ///     captured a token or started the in-step OTP flow; don't
    ///     override their in-progress auth.
    ///   * `self.preauthenticated` — only adopt once; avoids re-arming
    ///     the auto-advance if `load_context` runs again.
    ///   * `client.token().is_some()` — an unauthenticated client is
    ///     useless here and would just 401 downstream.
    fn load_context(&mut self, ctx: &Context) {
        if self.jwt.is_some() || self.otp_sent || self.preauthenticated {
            return;
        }
        let Some(client) = &ctx.coincube_client else {
            return;
        };
        let Some(token) = client.token() else {
            return;
        };
        // Clone the client (not just the token) to inherit the base URL /
        // HTTP plumbing the app already configured. Stash the JWT in
        // `Zeroizing` so it's scrubbed on drop — same handling as the
        // in-step OTP path. `apply()` moves it into `ctx.connect_jwt`.
        self.client = client.clone();
        self.jwt = Some(Zeroizing::new(token.to_string()));
        self.preauthenticated = true;
    }

    /// When pre-authenticated, fire `Message::Next` so the step machine
    /// runs `apply()` (which moves the adopted token into
    /// `ctx.connect_jwt`) and advances — skipping the auth UI entirely.
    fn load(&self) -> Task<Message> {
        if self.preauthenticated && self.jwt.is_some() {
            Task::done(Message::Next)
        } else {
            Task::none()
        }
    }

    fn skip(&self, ctx: &Context) -> bool {
        ctx.network == coincube_core::miniscript::bitcoin::Network::Regtest
            || ctx.remote_backend.is_some()
            // An earlier step (today: `RecoveryKitRestoreStep`) has
            // already collected the user's JWT for the same Connect
            // account, so re-authenticating here would just be a
            // second email + OTP round on the same session. Honor the
            // existing token and move on. Revert paths upstream clear
            // `connect_jwt` back to `None`, so navigating backward
            // through this step won't strand a stale token.
            || ctx.connect_jwt.is_some()
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        if self.skipped {
            ctx.use_coincube_connect = false;
            ctx.connect_jwt = None;
            return true;
        }
        // Move the JWT out of the step into `Context`. The step won't
        // be re-entered while `ctx.connect_jwt.is_some()` (see
        // `skip()`), so consuming the token here is safe and avoids
        // keeping a duplicate `Zeroizing<String>` alive on the step.
        if let Some(token) = self.jwt.take() {
            ctx.use_coincube_connect = true;
            ctx.connect_jwt = Some(token);
            true
        } else {
            false
        }
    }

    fn revert(&self, ctx: &mut Context) {
        ctx.use_coincube_connect = false;
        ctx.connect_jwt = None;
    }

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        if let Message::CoincubeConnect(msg) = message {
            match msg {
                CoincubeConnectMsg::EmailEdited(value) => {
                    self.email_touched = true;
                    self.email.value = value;
                    self.email.valid =
                        !self.email.value.is_empty() && self.email.value.contains('@');
                }
                CoincubeConnectMsg::ToggleMode => {
                    if !self.processing {
                        self.is_signup = !self.is_signup;
                        self.error = None;
                    }
                }
                CoincubeConnectMsg::RequestOtp => {
                    self.processing = true;
                    self.error = None;
                    let email = self.email.value.clone();
                    return Task::perform(
                        send_otp(self.client.clone(), email.clone(), self.is_signup),
                        move |result| {
                            Message::CoincubeConnect(CoincubeConnectMsg::OtpRequested {
                                email: email.clone(),
                                result,
                            })
                        },
                    );
                }
                CoincubeConnectMsg::ResendOtp => {
                    self.processing = true;
                    self.error = None;
                    return Task::perform(
                        send_otp(
                            self.client.clone(),
                            self.email.value.clone(),
                            self.is_signup,
                        ),
                        |res| Message::CoincubeConnect(CoincubeConnectMsg::OtpResent(res)),
                    );
                }
                CoincubeConnectMsg::OtpRequested { email, result } => {
                    self.processing = false;
                    match result {
                        Ok(()) => {
                            self.otp_sent = true;
                            self.otp = form::Value::default();
                            self.error = None;
                        }
                        Err(e) => {
                            if !self.is_signup && e.contains("Email not verified") {
                                return Task::done(Message::CoincubeConnect(
                                    CoincubeConnectMsg::EmailNotVerified { email },
                                ));
                            }
                            self.error = Some(e);
                        }
                    }
                }
                CoincubeConnectMsg::OtpResent(res) => {
                    self.processing = false;
                    match res {
                        Ok(()) => {
                            self.otp_sent = true;
                            self.otp = form::Value::default();
                            self.error = None;
                        }
                        Err(e) => {
                            self.error = Some(e);
                        }
                    }
                }
                CoincubeConnectMsg::EmailNotVerified { email } => {
                    // The email exists but signup was never completed. Switch to
                    // signup mode, fire resend_signup_otp, and land on OTP entry —
                    // identical to the app-level ConnectAccountPanel recovery path.
                    self.is_signup = true;
                    self.processing = true;
                    self.error = None;
                    let client = self.client.clone();
                    return Task::perform(
                        async move {
                            client
                                .resend_signup_otp(&email)
                                .await
                                .map_err(|e| e.to_string())
                        },
                        |res| Message::CoincubeConnect(CoincubeConnectMsg::OtpResent(res)),
                    );
                }
                CoincubeConnectMsg::OtpEdited(value) => {
                    self.otp.value = value.trim().to_string();
                    self.otp.valid = true;
                    if self.otp.value.len() == 6 && !self.processing {
                        let client = self.client.clone();
                        let email = self.email.value.clone();
                        let otp = self.otp.value.clone();
                        let is_signup = self.is_signup;
                        self.processing = true;
                        self.error = None;
                        return Task::perform(
                            async move {
                                let req = OtpVerifyRequest { email, otp };
                                if is_signup {
                                    client.signup_verify_otp(req).await
                                } else {
                                    client.login_verify_otp(req).await
                                }
                                // Wrap into `Zeroizing<String>` at the
                                // async boundary — before the token
                                // enters the message queue — so every
                                // in-flight copy of the subsequent
                                // `OtpVerified` message scrubs its
                                // heap on drop, not just the copy
                                // stashed on the step's state.
                                .map(|resp| Zeroizing::new(resp.token))
                                .map_err(|e| e.to_string())
                            },
                            |res| Message::CoincubeConnect(CoincubeConnectMsg::OtpVerified(res)),
                        );
                    }
                }
                CoincubeConnectMsg::OtpVerified(res) => {
                    self.processing = false;
                    match res {
                        Ok(token) => {
                            self.jwt = Some(token);
                            self.skipped = false;
                            return Task::done(Message::Next);
                        }
                        Err(e) => {
                            self.otp.valid = false;
                            self.error = Some(e);
                        }
                    }
                }
                CoincubeConnectMsg::Skip => {
                    self.skipped = true;
                    return Task::done(Message::Next);
                }
            }
        }
        Task::none()
    }

    fn view<'a>(
        &'a self,
        _hws: &'a HardwareWallets,
        progress: (usize, usize),
        _email: Option<&'a str>,
    ) -> Element<'a, Message> {
        let email_display = if self.email_touched {
            self.email.clone()
        } else {
            form::Value {
                valid: true,
                ..self.email.clone()
            }
        };
        view::define_coincube_connect(
            progress,
            &email_display,
            &self.otp,
            self.otp_sent,
            self.is_signup,
            self.processing,
            self.error.as_deref(),
        )
    }
}
