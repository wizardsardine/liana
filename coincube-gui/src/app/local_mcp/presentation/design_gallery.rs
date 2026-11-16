//! AI access state gallery. Every state the page can show is built from a
//! disposable public-key SQLite wallet and a synthetic pairing key, then laid
//! out at the dashboard's real content width (1440×900 and the 1024×768
//! minimum window) in dark and light. Layout always runs, so text that would
//! overflow a narrow window fails here. With `R06A_CAPTURE_DIR` set, each
//! state is also written as a content-only synthetic PNG for design review.
//! No credential, configuration or locator is rendered or written.

use super::super::{pairings, tests::fixture, Message, PairingContext, Settings};
use super::*;
use crate::app::view::shared::landing::capture;

/// The dashboard's content column for an outer window size (see
/// `capture::inspect`): sidebar 208px at the minimum window, else 248px,
/// then the 8/10 content pane.
fn content_width(outer: (u32, u32)) -> u32 {
    let sidebar = if outer.0 <= 1100 { 208.0 } else { 248.0 };
    ((outer.0 as f32 - sidebar) * 0.8).floor() as u32
}

/// Lay one state out at each review size and return its dark 1440 text.
fn render(scenario: &str, view: impl Fn() -> Ui<'static>) -> capture::Inspection {
    let mut wide = None;
    for (outer, appearance, palette) in [
        ((1440, 900), "dark", theme::Theme::dark()),
        ((1440, 900), "light", theme::Theme::light()),
        ((1024, 768), "dark", theme::Theme::dark()),
    ] {
        let inspection = capture::inspect_filled(
            &format!("ai-access-{scenario}"),
            view(),
            outer,
            appearance,
            palette,
            (content_width(outer), 1500),
        );
        for (value, _) in &inspection.text {
            assert!(
                !value.contains("COINCUBE_MCP_CREDENTIAL")
                    && !value.contains("COINCUBE_MCP_PAIRING"),
                "{}: private configuration reached the page",
                scenario
            );
        }
        if outer.0 == 1440 && appearance == "dark" {
            wide = Some(inspection);
        }
    }
    let spanish = coincube_ui::locale::render_in(coincube_ui::locale::Language::Spanish, &view);
    capture::inspect_filled(
        &format!("ai-access-{scenario}-es"),
        spanish,
        (1440, 900),
        "dark",
        theme::Theme::dark(),
        (content_width((1440, 900)), 1500),
    )
    .assert_spanish(&format!("AI access ({scenario})"));
    wide.expect("wide dark inspection")
}

/// Leak a settings value so the gallery can hand `'static` views to the
/// capture helper. Test-only; each gallery run leaks a handful of values.
fn leak(settings: Settings) -> &'static Settings {
    Box::leak(Box::new(settings))
}

struct Fixture {
    root: std::path::PathBuf,
    wallet: std::sync::Arc<crate::app::wallet::Wallet>,
    daemon: std::sync::Arc<dyn crate::daemon::Daemon + Send + Sync>,
}

/// Each state uses its own Cube: a pairing registry holds an exclusive owner
/// lock for as long as its settings live, and the gallery keeps them all.
impl Fixture {
    fn location(&self, cube: &str) -> pairings::Location {
        let chain = self.wallet.chain;
        std::fs::create_dir_all(self.root.join(chain.dir_name())).unwrap();
        crate::app::session::open_without_pin(cube);
        pairings::Location::new(&self.root, chain, cube).unwrap()
    }

    fn settings(&self, cube: &str) -> Settings {
        let location = self.location(cube);
        Settings {
            pairing_location: Some(location.clone()),
            pairing_store: Some(
                pairings::Store::open(location, zeroize::Zeroizing::new([64; 32]), true).unwrap(),
            ),
            ..Settings::default()
        }
    }

    fn send(
        &self,
        cube: &str,
        settings: &mut Settings,
        messages: impl IntoIterator<Item = Message>,
    ) {
        for message in messages {
            settings.update(
                message,
                Some(self.wallet.clone()),
                Some(self.daemon.clone()),
                true,
                cube,
            );
        }
    }

    fn restore(&self, cube: &str, settings: &mut Settings) {
        settings.ensure_pairings(PairingContext {
            root: &self.root,
            chain: self.wallet.chain,
            cube_id: cube,
            wallet: Some(self.wallet.clone()),
            daemon: Some(self.daemon.clone()),
            supported: true,
            proposal_supported: false,
            last_poll_at_startup: None,
        });
    }

    /// Two synthetic clients: one reading balances and history, one preparing
    /// unsigned transactions.
    fn paired(&self, cube: &str) -> Settings {
        let mut settings = self.settings(cube);
        self.send(
            cube,
            &mut settings,
            [
                Message::Enable(true),
                Message::ClientName("Claude Desktop (synthetic)".into()),
                Message::Wallet(true),
                Message::Capability(0, true),
                Message::Capability(1, true),
                Message::Pair,
                Message::ClientName("Maple AI (synthetic)".into()),
                Message::Wallet(true),
                Message::Transactions(true),
                Message::Pair,
            ],
        );
        let clients = settings.pairing_store.as_ref().unwrap().clients();
        assert_eq!(clients.len(), 2);
        // A successful pairing closes the form and opens the new client's
        // row, where its private configuration can be copied.
        assert!(!settings.pairing_open);
        assert_eq!(settings.managing.as_deref(), Some(clients[1].id.as_str()));
        settings
    }
}

fn synthetic_draft(status: &str) -> Prepared {
    use coincube_core::miniscript::bitcoin::WScriptHash;
    use coincube_core::miniscript::bitcoin::{hashes::Hash, Address, Network, ScriptBuf};
    let script = ScriptBuf::new_p2wsh(&WScriptHash::from_byte_array([7; 32]));
    Prepared {
        wallet_id: "synthetic-wallet".into(),
        draft_id: "synthetic-draft".into(),
        network: "bitcoin".into(),
        status: status.into(),
        recipients: vec![coincube_mcp::transaction::Recipient {
            address: Address::from_script(&script, Network::Bitcoin)
                .unwrap()
                .to_string(),
            amount_sats: 125_000,
        }],
        amount_sats: 125_000,
        fee_sats: 1_410,
        fee_rate_sat_vb: 6,
        change_sats: 373_590,
        input_count: 2,
        signing_required: true,
    }
}

/// Every reviewed AI access state renders inside the content width, in both
/// themes and at the minimum window, without exposing private configuration.
#[test]
fn ai_access_state_gallery() {
    let _guard = crate::app::session::test_guard();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _enter = runtime.enter();
    let (root, wallet, daemon) = fixture();
    let fx = Fixture {
        root,
        wallet,
        daemon,
    };

    // 1. Discovery: nothing paired, AI access off.
    let empty = leak(Settings::default());
    render("01-empty", || view_for_cube(empty, true));

    // 2. Unsupported wallet (no embedded Bitcoin-family Vault in this Cube).
    render("02-unsupported", || view_for_cube(empty, false));

    // 3. Pair form opened while AI access is off.
    let opened_off = leak(Settings {
        pairing_open: true,
        ..fx.settings("gallery-03")
    });
    render("03-pair-form-off", || view_for_cube(opened_off, true));

    // 4. Pair form with a name, the Vault and two read permissions chosen.
    let mut choosing = fx.settings("gallery-04");
    choosing.pairing_open = true;
    fx.send(
        "gallery-04",
        &mut choosing,
        [
            Message::Enable(true),
            Message::ClientName("Claude Desktop (synthetic)".into()),
            Message::Wallet(true),
            Message::Capability(0, true),
            Message::Capability(2, true),
        ],
    );
    let choosing = leak(choosing);
    let form = render("04-pair-form-read", || view_for_cube(choosing, true));
    assert!(form.contains("One per client."));
    assert_eq!(chosen_access(choosing), Some(Access::Read));

    // 5. Pair form with the transaction-preparation scope chosen.
    let mut preparing = fx.settings("gallery-05");
    preparing.pairing_open = true;
    fx.send(
        "gallery-05",
        &mut preparing,
        [
            Message::Enable(true),
            Message::ClientName("Maple AI (synthetic)".into()),
            Message::Wallet(true),
            Message::Transactions(true),
        ],
    );
    let preparing = leak(preparing);
    let form = render("05-pair-form-transactions", || {
        view_for_cube(preparing, true)
    });
    assert!(form.contains("Up to eight drafts per unlock"));
    assert_eq!(chosen_access(preparing), Some(Access::Transactions));

    // 6. Two paired clients, available, the first opened.
    let mut paired = fx.paired("gallery-06");
    fx.restore("gallery-06", &mut paired);
    assert_eq!(paired.paired.len(), 2, "both synthetic clients restored");
    let first = paired.pairing_store.as_ref().unwrap().clients()[0]
        .id
        .clone();
    paired.managing = Some(first.clone());
    // Copying confirms itself beside the action, and only on success.
    let config = paired.update(
        Message::CopyPair(first.clone()),
        Some(fx.wallet.clone()),
        Some(fx.daemon.clone()),
        true,
        "gallery-06",
    );
    assert!(config.is_some_and(|config| config.contains("COINCUBE_MCP_CREDENTIAL")));
    assert_eq!(paired.copied.as_deref(), Some(first.as_str()));
    let paired = leak(paired);
    let page = render("06-paired-available", || view_for_cube(paired, true));
    for label in [
        "Available",
        "Read · Balances, Confirmed history",
        "Prepare unsigned transactions",
        "Copied",
        "not a connection status",
        "Unverified label",
    ] {
        assert!(page.contains(label), "{}", label);
    }

    // 7. The same clients while AI access is off: paused, not forgotten.
    let mut paused = fx.paired("gallery-07");
    fx.send("gallery-07", &mut paused, [Message::Enable(false)]);
    fx.restore("gallery-07", &mut paused);
    let paused = leak(paused);
    let page = render("07-paired-paused-off", || view_for_cube(paused, true));
    assert!(page.contains("Paused") && page.contains("AI access is off"));
    assert!(!page.contains("Available"));

    // 8. Paired and on, but no runtime: the Vault is not loaded right now.
    let mut unloaded = fx.paired("gallery-08");
    unloaded.paired.clear();
    let unloaded = leak(unloaded);
    let page = render("08-paired-unavailable", || view_for_cube(unloaded, true));
    assert!(page.contains("Vault not loaded or changed"));

    // 9. Durable revocation could not be confirmed.
    let mut failed = fx.paired("gallery-09");
    failed.revoke();
    failed.pairing_error = Some("Could not persist revocation or invalidate its storage key. Clients are suspended now; durable revocation is NOT confirmed. Retry before closing COINCUBE Desktop.");
    failed.pairing_poisoned = true;
    let failed = leak(failed);
    // A failed durable revocation is never shown as success: the error
    // leads, every client reads Suspended, and nothing new can be paired.
    let page = render("09-revocation-unconfirmed", || view_for_cube(failed, true));
    assert!(page.contains("Pairing problem"));
    assert!(page.contains("Clients are suspended after a storage error."));
    assert!(page.contains("Copy is unavailable after a storage error."));
    assert!(page.contains("durable revocation is NOT confirmed"));
    assert!(page.contains("Suspended") && !page.contains("Available"));
    assert!(!page.has_focus(iced::advanced::widget::Id::new(PAIR_OPEN_ID)));

    // 10. Revocation decision.
    render("10-revoke-confirm", || {
        revoke_confirmation("Claude Desktop (synthetic)", &first)
    });

    // 11–12. An unsigned prepared transaction, ready and stale.
    let ready = synthetic_draft("unsigned_ready_for_native_review");
    render("11-transaction-ready", || {
        transaction_card("synthetic-client", ready.clone())
    });
    let stale = synthetic_draft("stale_requires_new_request");
    let card = render("12-transaction-stale", || {
        transaction_card("synthetic-client", stale.clone())
    });
    assert!(card.contains("ask the client to prepare it again"));

    // 13. Empty Cube: wallet preparation (proposal) access.
    let cube = "gallery-13";
    let mut setup = Settings {
        pairing_location: Some(fx.location(cube)),
        pairing_open: true,
        ..Settings::default()
    };
    for message in [
        Message::Enable(true),
        Message::ClientName("Claude Desktop (synthetic)".into()),
        Message::Creation(true),
    ] {
        setup.update(message, None, None, true, cube);
    }
    let setup = leak(setup);
    let page = render("13-empty-cube-setup", || setup.view_for_cube(false, true));
    for label in [
        "Pair a setup client",
        "Propose a wallet policy",
        "Prepare a wallet from public keys",
        "Pair setup client until revoked",
    ] {
        assert!(page.contains(label), "{}", label);
    }
    assert_eq!(chosen_access(setup), Some(Access::Creation));

    crate::app::session::close();
}

/// Read access and transaction preparation are one explicit choice: picking
/// either clears the other, and choosing read grants no part by itself.
#[test]
fn pairing_form_access_is_one_explicit_choice() {
    let mut settings = Settings {
        enabled: true,
        ..Settings::default()
    };
    assert_eq!(chosen_access(&settings), None);
    settings.update(Message::ChooseRead, None, None, true, "choice");
    assert_eq!(chosen_access(&settings), Some(Access::Read));
    assert_eq!(settings.capabilities, [false; 3]);
    settings.update(Message::Capability(1, true), None, None, true, "choice");
    settings.update(Message::Transactions(true), None, None, true, "choice");
    assert_eq!(chosen_access(&settings), Some(Access::Transactions));
    assert_eq!(settings.capabilities, [false; 3]);
    settings.update(Message::Capability(0, true), None, None, true, "choice");
    assert_eq!(chosen_access(&settings), Some(Access::Read));
    assert!(!settings.transactions_allowed);
    settings.finish_pairing(Some(Ok("synthetic".into())));
    assert_eq!(
        chosen_access(&settings),
        None,
        "the next client starts empty"
    );
}

/// Controls that remove themselves hand keyboard focus to their successor.
#[test]
fn focus_follows_controls_that_disappear() {
    use super::super::FocusAfter;
    use iced::advanced::widget::Id;
    let mut settings = Settings {
        enabled: true,
        ..Settings::default()
    };
    settings.update(Message::TogglePairing, None, None, true, "focus");
    assert_eq!(
        settings.focus_after(FocusAfter::TogglePairing),
        Some(Id::from(NAME_ID))
    );
    settings.enabled = false;
    assert_eq!(
        settings.focus_after(FocusAfter::TogglePairing),
        Some(Id::from(FORM_ENABLE_ID))
    );
    settings.enabled = true;
    // A failed pairing keeps the form, and focus, where it was.
    assert_eq!(settings.focus_after(FocusAfter::Pair), None);
    settings.finish_pairing(Some(Ok("new-client".into())));
    assert_eq!(
        settings.focus_after(FocusAfter::Pair),
        Some(Id::from("tenshu.ai-access.client.new-client.manage"))
    );
    settings.update(Message::TogglePairing, None, None, true, "focus");
    settings.update(Message::TogglePairing, None, None, true, "focus");
    assert_eq!(
        settings.focus_after(FocusAfter::TogglePairing),
        Some(Id::from(PAIR_OPEN_ID))
    );
    // After a storage failure Pair a client is disabled, so closing the
    // form leaves focus alone rather than sending it to a disabled control.
    settings.pairing_poisoned = true;
    assert_eq!(settings.focus_after(FocusAfter::TogglePairing), None);
}

/// The form's Turn on is its own intent (so only it moves focus) and turns
/// AI access on exactly as the Cube-wide switch does.
#[test]
fn form_turn_on_is_the_switch_with_its_own_focus() {
    let mut settings = Settings {
        pairing_open: true,
        ..Settings::default()
    };
    let emitted = capture::activate(
        view_for_cube(
            leak(Settings {
                pairing_open: true,
                ..Settings::default()
            }),
            true,
        ),
        iced::advanced::widget::Id::new(FORM_ENABLE_ID),
        (1024, 768),
        true,
    );
    assert!(emitted
        .iter()
        .any(|m| matches!(m, view::Message::LocalMcp(Message::TurnOnFromForm))));
    settings.update(Message::TurnOnFromForm, None, None, true, "turn-on");
    assert!(settings.enabled);
    assert_eq!(
        settings.focus_after(super::super::FocusAfter::Enable),
        Some(iced::advanced::widget::Id::from(NAME_ID))
    );
}

/// A Vault on a chain outside the documented five (here Signet) is not
/// offered for pairing, so it is never tagged UNSUPPORTED beside Pair.
#[test]
fn undocumented_chain_is_not_offered_for_pairing() {
    let root = std::env::temp_dir().join(format!("mcp-gallery-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join(crate::chain::ChainId::Signet.dir_name())).unwrap();
    let settings = Settings {
        pairing_location: Some(
            pairings::Location::new(&root, crate::chain::ChainId::Signet, "signet-cube").unwrap(),
        ),
        ..Settings::default()
    };
    let page = capture::inspect(
        "ai-access-signet",
        view_for_cube(&settings, true),
        (1024, 768),
        "dark",
        theme::Theme::dark(),
    );
    assert!(page.contains("Unavailable for this Cube"));
    assert!(!page.contains("UNSUPPORTED"));
    for id in [ENABLE_ID, PAIR_OPEN_ID] {
        assert!(
            !page.has_focus(iced::advanced::widget::Id::new(id)),
            "{}",
            id
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

/// On a Vault the runtime still serves outside the documented chains, a
/// Cube with stored clients can turn AI access back on: that only resumes
/// their stored access. Pairing a new client stays unavailable.
#[test]
fn switch_resumes_stored_clients_on_an_undocumented_chain() {
    let root = std::env::temp_dir().join(format!("mcp-gallery-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join(crate::chain::ChainId::Signet.dir_name())).unwrap();
    let location =
        pairings::Location::new(&root, crate::chain::ChainId::Signet, "signet-paired").unwrap();
    let mut store =
        pairings::Store::open(location.clone(), zeroize::Zeroizing::new([71; 32]), true).unwrap();
    store
        .add("Synthetic client".into(), pairings::Scope::Proposal)
        .unwrap();
    let settings = Settings {
        pairing_location: Some(location),
        pairing_store: Some(store),
        ..Settings::default()
    };
    let page = capture::inspect(
        "ai-access-signet-paired",
        view_for_cube(&settings, true),
        (1024, 768),
        "dark",
        theme::Theme::dark(),
    );
    assert!(page.has_focus(iced::advanced::widget::Id::new(ENABLE_ID)));
    assert!(!page.has_focus(iced::advanced::widget::Id::new(PAIR_OPEN_ID)));
    let emitted = capture::activate(
        view_for_cube(&settings, true),
        iced::advanced::widget::Id::new(ENABLE_ID),
        (1024, 768),
        true,
    );
    assert!(emitted
        .iter()
        .any(|m| matches!(m, view::Message::LocalMcp(Message::Enable(true)))));
    // Without runtime support nothing can be turned on.
    let inert = capture::inspect(
        "ai-access-signet-unsupported",
        view_for_cube(&settings, false),
        (1024, 768),
        "dark",
        theme::Theme::dark(),
    );
    assert!(!inert.has_focus(iced::advanced::widget::Id::new(ENABLE_ID)));
    drop(settings);
    let _ = std::fs::remove_dir_all(root);
}

/// The access-off form can be closed without turning AI access on, and an
/// open form cannot pair after a storage failure.
#[test]
fn pairing_form_has_an_exit_and_respects_storage_failure() {
    let off = Settings {
        pairing_open: true,
        ..Settings::default()
    };
    let emitted = capture::activate(
        view_for_cube(&off, true),
        iced::advanced::widget::Id::new(CANCEL_ID),
        (1024, 768),
        true,
    );
    assert!(emitted
        .iter()
        .any(|m| matches!(m, view::Message::LocalMcp(Message::TogglePairing))));
    let poisoned = Settings {
        enabled: true,
        pairing_open: true,
        pairing_poisoned: true,
        wallet_allowed: true,
        capabilities: [true, false, false],
        client_name: "Synthetic client".into(),
        ..Settings::default()
    };
    let page = capture::inspect(
        "ai-access-poisoned-form",
        view_for_cube(&poisoned, true),
        (1024, 768),
        "dark",
        theme::Theme::dark(),
    );
    assert!(!page.has_focus(iced::advanced::widget::Id::new(PAIR_ID)));
    let ready = Settings {
        pairing_poisoned: false,
        ..poisoned
    };
    let page = capture::inspect(
        "ai-access-ready-form",
        view_for_cube(&ready, true),
        (1024, 768),
        "dark",
        theme::Theme::dark(),
    );
    assert!(page.has_focus(iced::advanced::widget::Id::new(PAIR_ID)));
}

/// Choosing read access clears wallet-preparation choices left over from an
/// empty-Cube form, so the Read choice shows as selected.
#[test]
fn read_choice_clears_setup_choices() {
    let mut settings = Settings {
        enabled: true,
        ..Settings::default()
    };
    settings.update(Message::Creation(true), None, None, true, "read");
    assert_eq!(chosen_access(&settings), Some(Access::Creation));
    settings.update(Message::ChooseRead, None, None, true, "read");
    assert_eq!(chosen_access(&settings), Some(Access::Read));
    settings.update(Message::Proposals(true), None, None, true, "read");
    settings.update(Message::Capability(2, true), None, None, true, "read");
    assert_eq!(chosen_access(&settings), Some(Access::Read));
}
