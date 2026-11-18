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
    required: bool,
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
            required: false,
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

impl CoincubeConnectStep {
    fn toggle_mode(&mut self) {
        if !self.processing {
            self.is_signup = !self.is_signup;
            self.error = None;
        }
    }

    fn otp_requested(
        &mut self,
        email: String,
        result: Result<(), String>,
    ) -> Option<Task<Message>> {
        self.processing = false;
        match result {
            Ok(()) => {
                self.otp_sent = true;
                self.otp = form::Value::default();
                self.error = None;
            }
            Err(e) => {
                if !self.is_signup && e.contains("Email not verified") {
                    return Some(Task::done(Message::CoincubeConnect(
                        CoincubeConnectMsg::EmailNotVerified { email },
                    )));
                }
                self.error = Some(e);
            }
        }
        None
    }

    fn otp_edited(&mut self, value: String) -> Option<Task<Message>> {
        self.otp.value = value.trim().to_string();
        self.otp.valid = true;
        if self.otp.value.len() == 6 && !self.processing {
            let client = self.client.clone();
            let email = self.email.value.clone();
            let otp = self.otp.value.clone();
            let is_signup = self.is_signup;
            self.processing = true;
            self.error = None;
            return Some(Task::perform(
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
            ));
        }
        None
    }

    /// The flow has no Connect step at all, as opposed to one whose token
    /// is already in hand.
    fn never_runs(ctx: &Context) -> bool {
        (ctx.fork_local_vault_allowed()
            && ctx
                .coincube_client
                .as_ref()
                .and_then(|c| c.token())
                .is_none())
            || ctx.network == coincube_core::miniscript::bitcoin::Network::Regtest
    }
}

impl Step for CoincubeConnectStep {
    fn vault_stage(&self) -> Option<crate::installer::VaultStage> {
        Some(crate::installer::VaultStage::Connect)
    }
    /// The token this step collected skips the step but keeps its place
    /// on the stepper. A session the installer was launched with is adopted
    /// without the step ever showing, so that one leaves it off.
    fn off_vault_stepper(&self, ctx: &Context) -> bool {
        Self::never_runs(ctx) || ctx.connect_signed_in_at_launch
    }
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
    ///   * Only a client with a token auto-advances; an unauthenticated
    ///     client still supplies the configured endpoint for the OTP flow.
    fn load_context(&mut self, ctx: &Context) {
        self.required = ctx.bitcoin_config.chain.is_blake2b() && !ctx.fork_local_vault_allowed();
        // load_context is called when entering this step, not between an OTP
        // response and apply. Re-entering after auth was undone must not reuse
        // a pending token or auto-advance from the previous visit.
        if ctx.connect_jwt.is_none()
            && ctx
                .coincube_client
                .as_ref()
                .and_then(|client| client.token())
                .is_none()
        {
            self.jwt = None;
            self.preauthenticated = false;
            self.otp_sent = false;
            self.processing = false;
            self.client.clear_token();
        }
        if self.required {
            self.skipped = false;
        }
        if self.jwt.is_some() || self.otp_sent || self.preauthenticated {
            return;
        }
        let Some(client) = &ctx.coincube_client else {
            return;
        };
        // Retain the configured transport even when this step must perform login.
        self.client = client.clone();
        // OTP requests use this transport without an existing account bearer.
        // Only Context receives an authenticated clone after apply succeeds.
        self.client.clear_token();
        let Some(token) = client.token() else {
            return;
        };
        // Clone the client (not just the token) to inherit the base URL /
        // HTTP plumbing the app already configured. Stash the JWT in
        // `Zeroizing` so it's scrubbed on drop — same handling as the
        // in-step OTP path. `apply()` moves it into `ctx.connect_jwt`.
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
        Self::never_runs(ctx)
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
        if self.skipped && ctx.bitcoin_config.chain.is_blake2b() && !ctx.fork_local_vault_allowed()
        {
            self.error = Some("Connect authentication is required for Bitcoin Blake2b".to_string());
            return false;
        }
        if self.skipped {
            self.client.clear_token();
            self.jwt = None;
            self.preauthenticated = false;
            ctx.use_coincube_connect = false;
            ctx.connect_jwt = None;
            if let Some(client) = ctx.coincube_client.as_mut() {
                client.clear_token();
            }
            return true;
        }
        // Move the JWT out of the step into `Context`. The step won't
        // be re-entered while `ctx.connect_jwt.is_some()` (see
        // `skip()`), so consuming the token here is safe and avoids
        // keeping a duplicate `Zeroizing<String>` alive on the step.
        if let Some(token) = self.jwt.take() {
            ctx.use_coincube_connect = true;
            let mut client = self.client.clone();
            if client.token() != Some(token.as_str()) {
                client.set_token(token.as_str());
            }
            ctx.coincube_client = Some(client);
            ctx.connect_jwt = Some(token);
            self.preauthenticated = false;
            true
        } else {
            false
        }
    }

    fn revert(&self, ctx: &mut Context) {
        ctx.use_coincube_connect = false;
        ctx.connect_jwt = None;
        // Keep the selected API transport for a subsequent login, but remove
        // both the bearer value and reqwest's default Authorization header.
        if let Some(client) = ctx.coincube_client.as_mut() {
            client.clear_token();
        }
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
                CoincubeConnectMsg::ToggleMode => self.toggle_mode(),
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
                    return self.otp_requested(email, result).unwrap_or_else(Task::none);
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
                    return self.otp_edited(value).unwrap_or_else(Task::none);
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
                    if self.required {
                        self.error = Some(
                            "Connect authentication is required for Bitcoin Blake2b".to_string(),
                        );
                        return Task::none();
                    }
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
            !self.required,
        )
    }
}

#[cfg(test)]
mod chain_auth_tests {
    use super::*;
    use crate::{chain::ChainId, dir::CoincubeDirectory};

    #[test]
    fn login_and_retained_sessions_keep_authenticated_client_for_startup() {
        for retained in [false, true] {
            let dir = CoincubeDirectory::new(Default::default());
            let mut ctx = Context::new_for_chain(ChainId::BitcoinBlake2b, dir.clone(), None, None);
            let mut client = CoincubeClient::for_test("http://127.0.0.1:1/custom-connect");
            if retained {
                client.set_token("synthetic-session");
            }
            ctx.coincube_client = Some(client);
            let mut step = CoincubeConnectStep::new();
            step.load_context(&ctx);
            assert_eq!(step.preauthenticated, retained);
            if !retained {
                let mut hws = HardwareWallets::new(dir, ChainId::BitcoinBlake2b.bitcoin_network());
                let _ = step.update(
                    &mut hws,
                    Message::CoincubeConnect(CoincubeConnectMsg::OtpVerified(Ok(Zeroizing::new(
                        "synthetic-session".into(),
                    )))),
                );
            }
            assert!(step.apply(&mut ctx));
            let client = ctx
                .coincube_client
                .as_ref()
                .expect("startup client retained");
            assert_eq!(client.base_url, "http://127.0.0.1:1/custom-connect");
            assert_eq!(client.token(), Some("synthetic-session"));
            assert_eq!(
                ctx.connect_jwt.as_deref().map(|s| s.as_str()),
                Some("synthetic-session")
            );
        }
    }

    #[test]
    fn reversing_or_skipping_auth_clears_context_bearer_and_keeps_endpoint() {
        for chain in [
            ChainId::Bitcoin,
            ChainId::BitcoinBlake2b,
            ChainId::BitcoinBlake2bTestnet4,
        ] {
            let mut ctx = Context::new_for_chain(
                chain,
                CoincubeDirectory::new(Default::default()),
                None,
                None,
            );
            let mut client = CoincubeClient::for_test("http://127.0.0.1:1/custom-connect");
            client.set_token("synthetic-revert-token");
            ctx.coincube_client = Some(client);
            let mut step = CoincubeConnectStep::new();
            step.load_context(&ctx);
            assert!(step.client.token().is_none());
            assert!(step.apply(&mut ctx));
            assert!(step.client.token().is_none());
            assert!(ctx.coincube_client.as_ref().unwrap().token().is_some());
            step.revert(&mut ctx);
            assert!(!ctx.use_coincube_connect);
            assert!(ctx.connect_jwt.is_none());
            let client = ctx.coincube_client.as_ref().unwrap();
            assert!(client.token().is_none());
            assert_eq!(client.base_url, "http://127.0.0.1:1/custom-connect");
            step.load_context(&ctx);
            assert!(!step.preauthenticated);
            assert!(step.jwt.is_none());
            assert!(step.client.token().is_none());
            assert_eq!(step.client.base_url, "http://127.0.0.1:1/custom-connect");

            if !chain.is_blake2b() {
                ctx.coincube_client
                    .as_mut()
                    .unwrap()
                    .set_token("synthetic-skip-token");
                ctx.connect_jwt = Some(Zeroizing::new("synthetic-skip-token".into()));
                ctx.use_coincube_connect = true;
                step.skipped = true;
                assert!(step.apply(&mut ctx));
                assert!(!ctx.use_coincube_connect);
                assert!(ctx.connect_jwt.is_none());
                assert!(ctx.coincube_client.as_ref().unwrap().token().is_none());
                assert_eq!(
                    ctx.coincube_client.as_ref().unwrap().base_url,
                    "http://127.0.0.1:1/custom-connect"
                );
            }
        }
    }

    #[test]
    fn reentering_after_revert_drops_pending_token_and_auto_advance() {
        let mut ctx = Context::new_for_chain(
            ChainId::BitcoinBlake2b,
            CoincubeDirectory::new(Default::default()),
            None,
            None,
        );
        let mut client = CoincubeClient::for_test("http://127.0.0.1:1/custom-connect");
        client.set_token("synthetic-pending-token");
        ctx.coincube_client = Some(client);
        let mut step = CoincubeConnectStep::new();
        step.load_context(&ctx);
        assert!(step.jwt.is_some());
        assert!(step.preauthenticated);
        step.revert(&mut ctx);
        step.load_context(&ctx);
        assert!(step.jwt.is_none());
        assert!(!step.preauthenticated);
        assert!(step.client.token().is_none());
        assert!(iced_runtime::task::into_stream(step.load()).is_none());
        assert!(!step.apply(&mut ctx));
    }

    #[test]
    fn fork_connect_cannot_be_skipped_but_bitcoin_can() {
        for chain in [
            ChainId::Bitcoin,
            ChainId::BitcoinBlake2b,
            ChainId::BitcoinBlake2bTestnet4,
        ] {
            let dir = CoincubeDirectory::new(Default::default());
            let mut ctx = Context::new_for_chain(chain, dir.clone(), None, None);
            let mut step = CoincubeConnectStep::new();
            step.load_context(&ctx);
            let mut hws = HardwareWallets::new(dir, chain.bitcoin_network());
            let _ = step.update(&mut hws, Message::CoincubeConnect(CoincubeConnectMsg::Skip));
            assert_eq!(step.skipped, !chain.is_blake2b());
            assert_eq!(step.apply(&mut ctx), !chain.is_blake2b());
            if chain.is_blake2b() {
                step.skipped = true;
                assert!(!step.apply(&mut ctx));
                step.skipped = false;
                step.jwt = Some(Zeroizing::new("synthetic-token".to_string()));
                assert!(step.apply(&mut ctx));
                assert!(ctx.connect_jwt.is_some());
            }
        }
    }
}
