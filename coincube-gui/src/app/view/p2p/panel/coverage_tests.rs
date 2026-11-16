use super::*;
use crate::app::features::MarketplaceServerFlags;
use crate::app::view::shared::landing::capture;

fn panel() -> P2PPanel {
    let mut p = P2PPanel::new(
        None,
        None,
        zeroize::Zeroizing::new(String::new()),
        Some("All".into()),
        Network::Bitcoin,
    );
    p.mostro_config = MostroConfig::default();
    p
}

fn enabled_cache() -> Cache {
    Cache {
        marketplace_flags: MarketplaceServerFlags {
            marketplace_enabled: true,
            p2p_enabled: true,
            buy_sell_enabled: false,
        },
        ..Cache::default()
    }
}

fn update(p: &mut P2PPanel, cache: &Cache, message: P2PMessage) {
    // Do not poll tasks: tests inspect synchronous state and never contact relays,
    // pay invoices, access a wallet or execute returned effects.
    let _ = State::update(p, None, cache, Message::View(view::Message::P2P(message)));
}

fn trade(status: TradeStatus, side: OrderType) -> P2PTrade {
    P2PTrade {
        id: "synthetic-trade".into(),
        order_type: side,
        status,
        role: TradeRole::Taker,
        fiat_amount: 100.0,
        min_amount: None,
        max_amount: None,
        fiat_currency: "USD".into(),
        sats_amount: Some(10000),
        premium_percent: Some(2.0),
        payment_method: "Cash".into(),
        counterparty_rating: Some(4.5),
        created_at_ts: 100,
        created_at: String::new(),
        time_ago: "1h ago".into(),
        last_dm_action: None,
        countdown_start_ts: None,
        counterparty_pubkey: None,
        admin_pubkey: None,
        hold_invoice: None,
    }
}

fn inspect(element: Element<'_, view::Message>) -> capture::Inspection {
    capture::inspect_rendered_full_window(
        "p2p-behavior",
        element,
        (1024, 2000),
        "dark",
        theme::Theme::dark(),
    )
}

#[test]
fn fiat_amount_validation_rejects_missing_fractional_negative_and_overflowing_values() {
    let mut p = panel();
    p.create_order_type = OrderType::Sell;
    p.create_payment_methods = vec!["Cash".into()];
    for (value, expected) in [
        ("", "Amount is required"),
        ("1.5", "Enter a whole number"),
        ("x", "Enter a whole number"),
        ("9223372036854775808", "Enter a whole number"),
        ("0", "Amount must be greater than 0"),
        ("-1", "Amount must be greater than 0"),
        ("1000000000", "Amount is too large"),
    ] {
        p.create_min_amount.value = value.into();
        assert_eq!(p.validate_order_form().amount, Some(expected), "{value}");
        assert!(p.validate_order_form().has_errors());
    }
    for value in ["1", "999999999"] {
        p.create_min_amount.value = value.into();
        let v = p.validate_order_form();
        assert!(!v.has_errors());
        assert!(
            v.node_limits_missing,
            "market warning does not block valid form"
        );
    }
}

#[test]
fn range_and_fixed_price_limits_are_validated_before_confirmation() {
    let mut p = panel();
    p.create_order_type = OrderType::Sell;
    p.create_payment_methods = vec!["Cash".into()];
    p.create_min_amount.value = "50".into();
    p.range_order_mode = true;
    for (value, expected) in [
        ("", "Enter a whole number"),
        ("0", "Max must be greater than 0"),
        ("50", "Max must be greater than min"),
        ("49", "Max must be greater than min"),
        ("1000000000", "Max amount is too large"),
    ] {
        p.create_max_amount.value = value.into();
        assert_eq!(p.validate_order_form().max_amount, Some(expected));
    }
    p.create_max_amount.value = "200".into();
    assert!(!p.validate_order_form().has_errors());
    p.range_order_mode = false;
    p.create_pricing_mode = PricingMode::Fixed;
    for (value, expected) in [
        ("", "Sats amount is required for fixed price"),
        ("0", "Sats must be greater than 0"),
        ("-1", "Enter a valid number"),
        ("1.5", "Enter a valid number"),
        ("1000", "Waiting for node limits..."),
    ] {
        p.create_sats_amount.value = value.into();
        assert_eq!(p.validate_order_form().sats, Some(expected));
    }
    p.node_min_order_sats = Some(1000);
    p.node_max_order_sats = Some(10000);
    for (value, expected) in [
        ("999", Some("Below minimum (1,000 sats)")),
        ("1000", None),
        ("10000", None),
        ("10001", Some("Above maximum (10,000 sats)")),
    ] {
        p.create_sats_amount.value = value.into();
        let validation = p.validate_order_form();
        assert_eq!(validation.sats_range.as_deref(), expected);
        assert_eq!(validation.has_errors(), expected.is_some());
    }
}

#[test]
fn buy_orders_require_receiving_destination_and_market_premium_is_bounded() {
    let mut p = panel();
    p.create_min_amount.value = "100".into();
    let v = p.validate_order_form();
    assert_eq!(v.payment, Some("Select at least one payment method"));
    assert_eq!(
        v.lightning_address,
        Some("Lightning Address or invoice is required")
    );
    p.create_lightning_address.value = " synthetic@example.invalid ".into();
    p.create_custom_payment_method.value = " Cash ".into();
    for (premium, valid) in [
        ("", true),
        ("-100", true),
        ("100", true),
        ("101", false),
        ("-101", false),
        ("abc", false),
    ] {
        p.create_premium.value = premium.into();
        assert_eq!(!p.validate_order_form().has_errors(), valid, "{premium}");
    }
}

#[test]
fn form_events_preserve_custom_destination_and_reset_currency_dependent_methods() {
    let mut p = panel();
    let mut cache = enabled_cache();
    cache.lightning_address = Some("registered@example.invalid".into());
    update(&mut p, &cache, P2PMessage::MinAmountEdited("100".into()));
    assert_eq!(
        p.create_lightning_address.value,
        "registered@example.invalid"
    );
    update(
        &mut p,
        &cache,
        P2PMessage::LightningAddressEdited("custom@example.invalid".into()),
    );
    p.sync_lightning_address_from_cache(&cache);
    assert_eq!(p.create_lightning_address.value, "custom@example.invalid");
    update(&mut p, &cache, P2PMessage::UseRegisteredLightningAddress);
    assert_eq!(
        p.create_lightning_address.value,
        "registered@example.invalid"
    );
    for _ in 0..2 {
        update(
            &mut p,
            &cache,
            P2PMessage::PaymentMethodSelected("Cash".into()),
        );
    }
    assert_eq!(p.create_payment_methods, ["Cash"]);
    update(
        &mut p,
        &cache,
        P2PMessage::CustomPaymentMethodEdited("  Wire  ".into()),
    );
    update(&mut p, &cache, P2PMessage::AddCustomPaymentMethod);
    assert_eq!(p.create_payment_methods, ["Cash", "Wire"]);
    assert!(p.create_custom_payment_method.value.is_empty());
    update(
        &mut p,
        &cache,
        P2PMessage::PaymentMethodRemoved("Cash".into()),
    );
    assert_eq!(p.create_payment_methods, ["Wire"]);
    update(&mut p, &cache, P2PMessage::FiatCurrencyEdited("EUR".into()));
    assert!(p.create_payment_methods.is_empty());
    update(
        &mut p,
        &cache,
        P2PMessage::PricingModeSelected(PricingMode::Fixed),
    );
    update(&mut p, &cache, P2PMessage::RangeOrderToggled(true));
    assert!(p.range_order_mode);
    assert_eq!(p.create_pricing_mode, PricingMode::Market);
    update(&mut p, &cache, P2PMessage::MaxAmountEdited("200".into()));
    update(&mut p, &cache, P2PMessage::RangeOrderToggled(false));
    assert!(p.create_max_amount.value.is_empty());
    update(&mut p, &cache, P2PMessage::PremiumEdited("-5".into()));
    update(
        &mut p,
        &cache,
        P2PMessage::PremiumEdited("not-a-number".into()),
    );
    assert_eq!(p.create_premium.value, "-5");
}

#[test]
fn form_serialization_distinguishes_range_fixed_and_market_orders() {
    let mut p = panel();
    p.create_min_amount.value = "100".into();
    p.create_max_amount.value = "200".into();
    p.create_premium.value = "-2".into();
    p.create_payment_methods = vec!["Cash".into()];
    p.create_custom_payment_method.value = "Cash".into();
    p.create_lightning_address.value = " synthetic@example.invalid ".into();
    let f = p.build_order_form();
    assert_eq!(f.fiat_amount, 100);
    assert_eq!(f.min_amount, None);
    assert_eq!(f.payment_method, "Cash");
    assert_eq!(f.premium, -2);
    assert_eq!(
        f.buyer_invoice.as_deref(),
        Some("synthetic@example.invalid")
    );
    p.range_order_mode = true;
    p.create_custom_payment_method.value = " Wire ".into();
    let f = p.build_order_form();
    assert_eq!(
        (f.fiat_amount, f.min_amount, f.max_amount),
        (0, Some(100), Some(200))
    );
    assert_eq!(f.payment_method, "Cash,Wire");
    p.range_order_mode = false;
    p.create_order_type = OrderType::Sell;
    p.create_pricing_mode = PricingMode::Fixed;
    p.create_sats_amount.value = "12000".into();
    let f = p.build_order_form();
    assert_eq!(f.amount, 12000);
    assert_eq!(f.premium, 0);
    assert!(f.buyer_invoice.is_none());
}

#[test]
fn server_off_drops_form_and_async_payment_messages() {
    let mut p = panel();
    p.spark_pay_session_id = Some("current".into());
    p.spark_balance_sat = Some(20);
    for msg in [
        P2PMessage::MinAmountEdited("100".into()),
        P2PMessage::SubmitOrder,
        P2PMessage::SparkBalanceLoaded {
            order_id: "current".into(),
            balance_sat: 50000,
        },
        P2PMessage::SparkPayFailed {
            order_id: "current".into(),
            err: "failure".into(),
        },
    ] {
        update(&mut p, &Cache::default(), msg);
        assert!(p.create_min_amount.value.is_empty());
        assert!(!p.submit_attempted);
        assert_eq!(p.spark_balance_sat, Some(20));
        assert!(matches!(p.spark_pay_phase, SparkPayPhase::Idle));
    }
}

#[test]
fn stale_spark_results_cannot_change_new_trade_session() {
    let mut p = panel();
    let cache = enabled_cache();
    p.spark_pay_session_id = Some("current".into());
    p.spark_balance_sat = Some(500);
    p.spark_pay_amount_sat = Some(100);
    for msg in [
        P2PMessage::SparkBalanceLoaded {
            order_id: "old".into(),
            balance_sat: 9000,
        },
        P2PMessage::SparkBalanceFailed {
            order_id: "old".into(),
            err: "failure".into(),
        },
        P2PMessage::SparkInvoiceAmountParsed {
            order_id: "old".into(),
            amount_sat: Some(9999),
        },
        P2PMessage::SparkPayFailed {
            order_id: "old".into(),
            err: "failure".into(),
        },
        P2PMessage::SparkPayPrepare {
            order_id: "old".into(),
            invoice: "synthetic".into(),
        },
    ] {
        update(&mut p, &cache, msg);
        assert_eq!(p.spark_balance_sat, Some(500));
        assert_eq!(p.spark_pay_amount_sat, Some(100));
        assert!(matches!(p.spark_pay_phase, SparkPayPhase::Idle));
    }
    update(
        &mut p,
        &cache,
        P2PMessage::SparkBalanceLoaded {
            order_id: "current".into(),
            balance_sat: 9000,
        },
    );
    assert_eq!(p.spark_balance_sat, Some(9000));
    update(
        &mut p,
        &cache,
        P2PMessage::SparkInvoiceAmountParsed {
            order_id: "current".into(),
            amount_sat: None,
        },
    );
    assert_eq!(p.spark_pay_amount_sat, None);
    update(
        &mut p,
        &cache,
        P2PMessage::SparkBalanceFailed {
            order_id: "current".into(),
            err: "offline".into(),
        },
    );
    assert_eq!(p.spark_balance_sat, None);
}

#[test]
fn payment_fallback_is_frozen_while_rpc_is_in_flight() {
    let mut p = panel();
    let cache = enabled_cache();
    for phase in [SparkPayPhase::Preparing, SparkPayPhase::Sending] {
        p.spark_pay_phase = phase;
        update(&mut p, &cache, P2PMessage::ToggleQrFallback(true));
        assert!(!p.show_qr_fallback);
    }
    p.spark_pay_phase = SparkPayPhase::Idle;
    update(&mut p, &cache, P2PMessage::ToggleQrFallback(true));
    assert!(p.show_qr_fallback);
}

#[test]
fn fee_buffer_and_spark_balance_gate_include_fee_and_saturate() {
    let mut p = panel();
    assert!(!p.spark_can_cover(Some(1000)));
    let required = 1000 + spark_pay_fee_buffer_sats(1000);
    p.spark_balance_sat = Some(required - 1);
    assert!(!p.spark_can_cover(Some(1000)));
    p.spark_balance_sat = Some(required);
    assert!(p.spark_can_cover(Some(1000)));
    p.spark_balance_sat = Some(u64::MAX);
    assert!(p.spark_can_cover(Some(u64::MAX)));
    p.spark_balance_sat = Some(0);
    assert!(!p.spark_can_cover(None));
    p.spark_balance_sat = Some(1);
    assert!(p.spark_can_cover(None));
}

#[test]
fn payment_errors_are_actionable_and_unicode_truncation_is_safe() {
    assert!(friendly_spark_pay_error("INSUFFICIENT funds").contains("insufficient funds"));
    for error in ["RouteNotFound", "no route"] {
        assert!(friendly_spark_pay_error(error).contains("Lightning route"));
    }
    for error in [
        "UnexpectedEof",
        "close_notify",
        "Connect error",
        "network error",
        "timeout",
        "timed out",
    ] {
        assert!(is_transient_spark_error(error));
        assert!(friendly_spark_pay_error(error).contains("connection issue"));
    }
    assert!(!is_transient_spark_error("insufficient balance"));
    assert_eq!(
        friendly_spark_pay_error("  bad invoice  \ninternal details"),
        "bad invoice"
    );
    assert_eq!(
        friendly_spark_pay_error(&"界".repeat(201)),
        format!("{}…", "界".repeat(200))
    );
}

#[test]
fn canceled_creator_orders_stay_visible_when_spark_funds_are_pending() {
    let mut p = panel();
    let mut t = trade(TradeStatus::Canceled, OrderType::Sell);
    t.role = TradeRole::Creator;
    p.trades.push(t);
    assert!(p.filtered_trades().is_empty());
    p.spark_funded_order_ids.insert("synthetic-trade".into());
    assert_eq!(p.filtered_trades().len(), 1);
    p.trade_filters = vec![TradeFilter::Active];
    assert!(p.filtered_trades().is_empty());
    p.trade_filters = vec![TradeFilter::Canceled];
    assert_eq!(p.filtered_trades().len(), 1);
    assert!(
        inspect(p.trade_detail_view(&p.trades[0])).contains("Trade canceled after Spark payment")
    );
}

#[test]
fn trade_screens_follow_buyer_seller_protocol_actions() {
    let p = panel();
    for (action, buyer, seller) in [
        (
            "PayInvoice",
            "Waiting for seller to lock the Bitcoin",
            "Pay the hold invoice",
        ),
        (
            "HoldInvoicePaymentAccepted",
            "Send fiat payment",
            "Waiting for fiat payment",
        ),
        ("AddInvoice", "Submit your invoice", "Waiting for buyer"),
        ("PaymentFailed", "Payment failed", "Payment failed"),
        ("PurchaseCompleted", "Trade Complete!", "Trade Complete!"),
        (
            "CooperativeCancelInitiatedByYou",
            "Cancellation requested",
            "Cancellation requested",
        ),
        (
            "CooperativeCancelInitiatedByPeer",
            "Cancel requested by counterparty",
            "Cancel requested by counterparty",
        ),
        ("DisputeInitiatedByYou", "Dispute opened", "Dispute opened"),
        (
            "DisputeInitiatedByPeer",
            "Dispute opened by counterparty",
            "Dispute opened by counterparty",
        ),
        (
            "AdminTookDispute",
            "Admin reviewing dispute",
            "Admin reviewing dispute",
        ),
    ] {
        for (side, expected) in [(OrderType::Buy, buyer), (OrderType::Sell, seller)] {
            let mut t = trade(TradeStatus::Active, side);
            t.last_dm_action = Some(action.into());
            let view = inspect(p.trade_detail_view(&t));
            assert!(
                view.contains(expected),
                "{action}: missing {expected}: {:?}",
                view.text
            );
            assert!(view.contains("synthetic-trade"));
        }
    }
}

#[test]
fn terminal_trade_views_do_not_offer_irreversible_trade_actions() {
    let p = panel();
    for status in [
        TradeStatus::Success,
        TradeStatus::Canceled,
        TradeStatus::Expired,
    ] {
        let t = trade(status, OrderType::Buy);
        let view = inspect(p.trade_detail_view(&t));
        assert!(!view.contains("Confirm Fiat Sent"));
        assert!(!view.contains("Submit Invoice"));
        assert!(!view.contains("Cancel Trade"));
        assert!(!view.contains("Open Dispute"));
        assert!(view.contains("Close"));
    }
}

#[test]
fn create_screen_shows_validation_and_confirmation_values() {
    let mut p = panel();
    let cache = enabled_cache();
    p.submit_attempted = true;
    let view = inspect(p.create_order_view(&cache));
    assert!(view.contains("Amount is required"));
    assert!(view.contains("Select at least one payment method"));
    p.create_min_amount.value = "100".into();
    p.create_payment_methods = vec!["Cash".into()];
    p.create_lightning_address.value = "synthetic@example.invalid".into();
    let view = inspect(p.order_confirmation_view());
    assert!(view.contains("100"));
    assert!(view.contains("Cash"));
}

#[test]
fn chat_transcripts_separate_peer_and_admin_messages() {
    let mut p = panel();
    let mut t = trade(TradeStatus::Dispute, OrderType::Buy);
    assert!(inspect(p.chat_view(&t)).contains("No messages yet"));
    assert!(inspect(p.dispute_chat_view(&t)).contains("Waiting for admin assignment"));
    t.counterparty_pubkey = Some("01".repeat(32));
    t.admin_pubkey = Some("02".repeat(32));
    assert!(inspect(p.dispute_chat_view(&t)).contains("Admin assigned"));
    p.chat_show_trade_info = true;
    p.chat_show_user_info = true;
    p.cached_chat_identity = Some(super::super::mostro::ChatIdentityInfo {
        counterparty_pubkey: t.counterparty_pubkey.clone(),
        counterparty_nickname: Some("Synthetic Peer".into()),
        our_trade_pubkey: Some("03".repeat(32)),
        our_nickname: Some("Synthetic Self".into()),
        shared_key: Some("04".repeat(32)),
    });
    p.cached_trade_messages = [
        ("SendDm", "peer message", false),
        ("SendDm", "own message", true),
        ("AdminDm", "admin message", false),
        ("AdminDm", "own dispute message", true),
        ("FiatSent", "protocol status", false),
    ]
    .iter()
    .copied()
    .map(
        |(action, text, is_own)| super::super::mostro::TradeMessage {
            timestamp: 100,
            action: action.into(),
            payload_json: serde_json::to_string(&Some(mostro_core::message::Payload::TextMessage(
                text.into(),
            )))
            .unwrap(),
            is_own,
        },
    )
    .collect();
    let peer = inspect(p.chat_view(&t));
    assert!(peer.contains("peer message") && peer.contains("own message"));
    assert!(!peer.contains("admin message") && !peer.contains("protocol status"));
    assert!(peer.contains("Shared Key"));
    assert!(peer.contains("Synthetic Self"));
    let admin = inspect(p.dispute_chat_view(&t));
    assert!(admin.contains("admin message") && admin.contains("own dispute message"));
    assert!(!admin.contains("peer message"));
    t.counterparty_pubkey = None;
    assert!(inspect(p.chat_view(&t)).contains("Chat is read-only"));
}

#[test]
fn chat_attachment_placeholders_and_failures_remain_visible_without_downloads() {
    let mut p = panel();
    let mut t = trade(TradeStatus::Active, OrderType::Sell);
    t.counterparty_pubkey = Some("01".repeat(32));
    let url = "https://example.invalid/synthetic-image";
    let metadata = serde_json::json!({"type":"image_encrypted", "blossom_url":url, "nonce":"synthetic", "mime_type":"image/png", "original_size":4, "encrypted_size":32, "width":1, "height":1, "filename":"fixture.png"});
    p.cached_trade_messages = vec![super::super::mostro::TradeMessage {
        timestamp: 100,
        action: "SendDm".into(),
        payload_json: serde_json::to_string(&Some(mostro_core::message::Payload::TextMessage(
            metadata.to_string(),
        )))
        .unwrap(),
        is_own: false,
    }];
    assert!(inspect(p.chat_view(&t)).contains("fixture.png"));
    p.image_cache.insert(url.into(), ImageCacheEntry::Loading);
    let loading = inspect(p.chat_view(&t));
    assert!(loading.contains("Loading image"), "{:?}", loading.text);
    p.image_cache.insert(
        url.into(),
        ImageCacheEntry::Failed("synthetic failure".into()),
    );
    assert!(inspect(p.chat_view(&t)).contains("Failed: synthetic failure"));
    p.image_cache.insert(
        url.into(),
        ImageCacheEntry::Ready(iced::widget::image::Handle::from_rgba(
            1,
            1,
            vec![0, 0, 0, 255],
        )),
    );
    assert!(inspect(p.chat_view(&t)).contains("fixture.png"));
    let metadata = serde_json::json!({"type":"file_encrypted", "file_type":"document", "blossom_url":"https://example.invalid/file", "nonce":"synthetic", "mime_type":"application/pdf", "original_size":1000, "encrypted_size":1028, "filename":"receipt.pdf"});
    p.cached_trade_messages[0].payload_json = serde_json::to_string(&Some(
        mostro_core::message::Payload::TextMessage(metadata.to_string()),
    ))
    .unwrap();
    assert!(inspect(p.chat_view(&t)).contains("receipt.pdf"));
}

#[test]
fn payment_preview_displays_fee_total_and_recovery_controls_for_each_phase() {
    let mut p = panel();
    p.spark_balance_sat = Some(50000);
    for (phase, expected) in [
        (SparkPayPhase::Idle, "Pay from Spark"),
        (SparkPayPhase::Preparing, "Preparing payment…"),
        (
            SparkPayPhase::Prepared(PrepareSendOk {
                handle: "synthetic-handle".into(),
                amount_sat: 10000,
                fee_sat: 25,
                method: "Bolt11Invoice".into(),
                cross_chain: None,
                has_token_leg: Some(false),
            }),
            "10025 sats",
        ),
        (SparkPayPhase::Sending, "Sending payment…"),
        (
            SparkPayPhase::Error("synthetic failure".into()),
            "Try again",
        ),
    ] {
        p.spark_pay_phase = phase;
        let view =
            inspect(p.spark_pay_action_body("synthetic", "synthetic invoice", Some(10000), None));
        assert!(
            view.contains(expected),
            "missing {}: {:?}",
            expected,
            view.text
        );
        assert!(view.contains("Lock amount: 10000 sats"));
    }
    let qr = qr_code::Data::new("synthetic invoice").unwrap();
    let view = inspect(p.payment_invoice_modal_view("synthetic", "synthetic invoice", None, &qr));
    assert!(view.contains("Payment Required"));
    assert!(view.contains("amount TBD"));
    assert!(view.contains("Copy Invoice"));
    assert!(view.contains("Cancel Order"));
}

#[test]
fn own_pending_orders_show_range_and_fixed_values_without_trade_actions() {
    let p = panel();
    for side in [OrderType::Buy, OrderType::Sell] {
        let mut t = trade(TradeStatus::Pending, side);
        t.role = TradeRole::Creator;
        t.min_amount = Some(50.0);
        t.max_amount = Some(200.0);
        let view = inspect(p.trade_detail_view(&t));
        assert!(view.contains("50 - 200"));
        assert!(view.contains("10000 sats"));
        assert!(view.contains("Cancel Order"));
        assert!(!view.contains("Confirm Fiat Sent"));
        t.sats_amount = None;
        t.min_amount = None;
        t.max_amount = None;
        let view = inspect(p.trade_detail_view(&t));
        assert!(view.contains("100.00"));
        assert!(view.contains("Market Price"));
    }
}

#[test]
fn trade_status_fallbacks_and_countdown_do_not_need_a_live_coordinator() {
    let p = panel();
    for status in [
        TradeStatus::Active,
        TradeStatus::WaitingPayment,
        TradeStatus::WaitingBuyerInvoice,
        TradeStatus::FiatSent,
        TradeStatus::SettledHoldInvoice,
        TradeStatus::CooperativelyCanceled,
        TradeStatus::Dispute,
        TradeStatus::PaymentFailed,
    ] {
        for side in [OrderType::Buy, OrderType::Sell] {
            let mut t = trade(status.clone(), side);
            t.countdown_start_ts = Some(1);
            let view = inspect(p.trade_detail_view(&t));
            assert!(view.contains(t.status.label()));
            assert!(view.contains("Time expired"));
            assert!(view.contains("Close"));
        }
    }
    let mut t = trade(TradeStatus::WaitingPayment, OrderType::Sell);
    t.countdown_start_ts = Some(chrono::Utc::now().timestamp() as u64);
    assert!(inspect(p.trade_detail_view(&t)).contains("Time remaining"));
}

#[test]
fn conversation_tabs_show_distinct_empty_states() {
    let mut p = panel();
    assert!(inspect(p.chat_list_view()).contains("No conversations yet"));
    p.chat_list_tab = ChatListTab::Disputes;
    assert!(inspect(p.chat_list_view()).contains("No dispute conversations"));
}

#[test]
fn settings_show_config_errors_and_network_tags() {
    let mut p = panel();
    p.mostro_config_error = Some("Synthetic config error");
    let view = inspect(p.mostro_settings_view());
    assert!(view.contains("Synthetic config error"));
    assert!(view.contains("Active"));
    for node in &p.mostro_config.nodes {
        assert!(view.contains(&node.name));
    }
}

#[test]
fn buyer_confirmation_is_actionable_once_and_disabled_during_submission() {
    let mut p = panel();
    let mut t = trade(TradeStatus::Active, OrderType::Buy);
    t.last_dm_action = Some("HoldInvoicePaymentAccepted".into());
    let messages = capture::click_text(
        p.trade_detail_view(&t),
        "Confirm Fiat Sent",
        0,
        (1024, 2000),
    );
    assert_eq!(
        messages
            .iter()
            .filter(|m| matches!(m, view::Message::P2P(P2PMessage::ConfirmFiatSent)))
            .count(),
        1
    );
    p.trade_action_loading = true;
    assert!(capture::click_text(
        p.trade_detail_view(&t),
        "Confirm Fiat Sent",
        0,
        (1024, 2000)
    )
    .is_empty());
    p.trade_action_loading = false;
    t.order_type = OrderType::Sell;
    assert!(!inspect(p.trade_detail_view(&t)).contains("Confirm Fiat Sent"));
}
