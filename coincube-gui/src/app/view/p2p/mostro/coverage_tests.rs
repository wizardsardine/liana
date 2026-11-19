use super::*;
use mostro_core::message::Payload;
use mostro_core::order::{SmallOrder, Status};

fn session() -> TradeSession {
    serde_json::from_value(serde_json::json!({
        "order_id": "00000000-0000-4000-8000-000000000001",
        "trade_index": 1, "kind": "buy", "fiat_code": "USD", "fiat_amount": 100,
        "min_amount": null, "max_amount": null, "amount": 10000,
        "premium": 2, "payment_method": "Cash", "created_at": 100
    }))
    .unwrap()
}

fn message(timestamp: u64, action: &str, payload: Option<Payload>) -> TradeMessage {
    TradeMessage {
        timestamp,
        action: action.into(),
        payload_json: serde_json::to_string(&payload).unwrap(),
        is_own: false,
    }
}

#[test]
fn legacy_sessions_default_to_creator_without_chat_keys() {
    let s = session();
    assert_eq!(s.role, "creator");
    assert!(s.messages.is_empty());
    assert!(s.counterparty_trade_pubkey.is_none());
    assert!(s.admin_trade_pubkey.is_none());
    let saved = serde_json::to_string(&s).unwrap();
    let restored: TradeSession = serde_json::from_str(&saved).unwrap();
    assert_eq!(restored.order_id, s.order_id);
    assert_eq!(restored.amount, 10000);
    assert!(
        serde_json::from_str::<MostroData>("{\"last_trade_index\":9}")
            .unwrap()
            .trades
            .is_empty()
    );
}

#[test]
fn cube_filename_cannot_escape_its_directory() {
    for (input, expected) in [
        ("", "default"),
        ("../wallet", "___wallet"),
        ("a/b\\c", "a_b_c"),
        ("cube-A_1", "cube-A_1"),
        ("café 🔑", "caf___"),
        ("..", "__"),
    ] {
        assert_eq!(safe_filename(input), expected);
    }
}

#[test]
fn counterparty_selection_never_returns_our_only_key() {
    for (buyer, seller, expected) in [
        (Some("ours"), Some("peer"), Some("peer")),
        (Some("peer"), Some("ours"), Some("peer")),
        (Some("ours"), None, None),
        (None, Some("ours"), None),
        (Some("peer"), None, Some("peer")),
        (None, Some("peer"), Some("peer")),
        (None, None, None),
    ] {
        let order = SmallOrder {
            buyer_trade_pubkey: buyer.map(str::to_owned),
            seller_trade_pubkey: seller.map(str::to_owned),
            ..Default::default()
        };
        assert_eq!(
            extract_counterparty_pubkey(&order, "ours").as_deref(),
            expected
        );
        assert_eq!(
            counterparty_from_payload(&Some(Payload::Order(order.clone())), "ours").as_deref(),
            expected
        );
        assert_eq!(
            counterparty_from_payload(
                &Some(Payload::PaymentRequest(Some(order), "invoice".into(), None)),
                "ours"
            )
            .as_deref(),
            expected
        );
    }
    assert!(counterparty_from_payload(&None, "ours").is_none());
    assert!(
        counterparty_from_payload(&Some(Payload::TextMessage("peer".into())), "ours").is_none()
    );
}

#[test]
fn chat_and_acknowledgements_do_not_replace_trade_status() {
    let mut s = session();
    assert!(latest_dm_action(&s).is_none());
    s.messages
        .push(message(10, "HoldInvoicePaymentAccepted", None));
    for action in ["SendDm", "AdminDm", "chat", "dispute_chat"] {
        s.messages.push(message(11, action, None));
        assert_eq!(latest_dm_action(&s), Some("HoldInvoicePaymentAccepted"));
    }
    for action in NON_STATUS_ACTIONS {
        s.messages.push(message(12, action, None));
        assert_eq!(latest_dm_action(&s), Some(*action));
        assert_eq!(trade_from_session(&s).status, TradeStatus::Active);
    }
}

#[test]
fn hold_invoice_uses_latest_valid_payment_payload_only() {
    let mut s = session();
    for action in ["PayInvoice", "WaitingSellerToPay", "BuyerTookOrder"] {
        let invoice = format!("synthetic-{action}");
        s.messages.push(message(
            10,
            action,
            Some(Payload::PaymentRequest(None, invoice.clone(), Some(10000))),
        ));
        s.messages.push(message(
            11,
            "SendDm",
            Some(Payload::TextMessage("not an invoice".into())),
        ));
        assert_eq!(extract_hold_invoice(&s), Some(invoice.clone()));
        s.messages.push(TradeMessage {
            payload_json: "broken-json".into(),
            ..message(12, action, None)
        });
        s.messages.push(message(
            13,
            action,
            Some(Payload::TextMessage("wrong variant".into())),
        ));
        assert_eq!(extract_hold_invoice(&s), Some(invoice));
    }
}

#[test]
fn countdown_uses_first_nonzero_message_in_current_phase() {
    for (first, repeat) in [
        ("PayInvoice", "WaitingSellerToPay"),
        ("AddInvoice", "WaitingBuyerInvoice"),
    ] {
        let mut s = session();
        s.messages = vec![
            message(0, first, None),
            message(100, first, None),
            message(200, repeat, None),
            message(300, "chat", None),
        ];
        assert_eq!(countdown_start_timestamp(&s), Some(100));
        s.messages.push(message(400, "FiatSent", None));
        assert_eq!(countdown_start_timestamp(&s), None);
    }
    assert_eq!(countdown_start_timestamp(&session()), None);
}

#[test]
fn protocol_actions_map_to_expected_trade_states() {
    let cases = [
        (
            vec!["WaitingSellerToPay", "PayInvoice"],
            TradeStatus::WaitingPayment,
        ),
        (
            vec!["WaitingBuyerInvoice", "AddInvoice"],
            TradeStatus::WaitingBuyerInvoice,
        ),
        (
            vec![
                "BuyerTookOrder",
                "HoldInvoicePaymentAccepted",
                "BuyerInvoiceAccepted",
            ],
            TradeStatus::Active,
        ),
        (vec!["FiatSent", "FiatSentOk"], TradeStatus::FiatSent),
        (vec!["Released", "Release"], TradeStatus::SettledHoldInvoice),
        (
            vec![
                "PurchaseCompleted",
                "Rate",
                "RateReceived",
                "HoldInvoicePaymentSettled",
                "AdminSettled",
            ],
            TradeStatus::Success,
        ),
        (
            vec![
                "Canceled",
                "Cancel",
                "AdminCanceled",
                "HoldInvoicePaymentCanceled",
                "CooperativeCancelAccepted",
            ],
            TradeStatus::Canceled,
        ),
        (
            vec![
                "CooperativeCancelInitiatedByYou",
                "CooperativeCancelInitiatedByPeer",
            ],
            TradeStatus::CooperativelyCanceled,
        ),
        (
            vec![
                "DisputeInitiatedByYou",
                "DisputeInitiatedByPeer",
                "AdminTookDispute",
            ],
            TradeStatus::Dispute,
        ),
        (vec!["PaymentFailed"], TradeStatus::PaymentFailed),
    ];
    for (actions, expected) in cases {
        for action in actions {
            assert_eq!(
                dm_action_to_status(action),
                Some(expected.clone()),
                "{action}"
            );
        }
    }
    for action in NON_STATUS_ACTIONS
        .iter()
        .copied()
        .chain(["unknown", "chat"])
    {
        assert_eq!(dm_action_to_status(action), None);
    }
    for (status, expected) in [
        (Status::Pending, TradeStatus::Pending),
        (Status::Active, TradeStatus::Active),
        (Status::WaitingPayment, TradeStatus::WaitingPayment),
        (
            Status::WaitingBuyerInvoice,
            TradeStatus::WaitingBuyerInvoice,
        ),
        (Status::FiatSent, TradeStatus::FiatSent),
        (Status::Success, TradeStatus::Success),
        (Status::SettledByAdmin, TradeStatus::Success),
        (Status::CompletedByAdmin, TradeStatus::Success),
        (Status::Canceled, TradeStatus::Canceled),
        (Status::CanceledByAdmin, TradeStatus::Canceled),
        (
            Status::CooperativelyCanceled,
            TradeStatus::CooperativelyCanceled,
        ),
        (Status::Dispute, TradeStatus::Dispute),
        (Status::Expired, TradeStatus::Expired),
        (Status::SettledHoldInvoice, TradeStatus::Active),
        (Status::InProgress, TradeStatus::Active),
    ] {
        assert_eq!(map_trade_status(&status), expected);
    }
}

#[test]
fn range_sessions_restore_amount_perspective_and_newest_first() {
    let mut old = session();
    old.created_at = 10;
    let mut recent = session();
    recent.order_id = "recent".into();
    recent.kind = "sell".into();
    recent.role = "taker".into();
    recent.fiat_amount = 0;
    recent.min_amount = Some(50);
    recent.max_amount = Some(200);
    recent.amount = 0;
    recent.counterparty_trade_pubkey = Some("peer".into());
    recent.admin_trade_pubkey = Some("solver".into());
    let trades = sessions_to_fallback_trades(&[old, recent]);
    assert_eq!(trades[0].id, "recent");
    assert_eq!(trades[0].order_type, OrderType::Sell);
    assert_eq!(trades[0].role, TradeRole::Taker);
    assert_eq!(trades[0].fiat_amount, 50.0);
    assert_eq!(trades[0].max_amount, Some(200.0));
    assert!(trades[0].sats_amount.is_none());
    assert_eq!(trades[0].counterparty_pubkey.as_deref(), Some("peer"));
    assert_eq!(trades[0].admin_pubkey.as_deref(), Some("solver"));
    assert_eq!(trades[1].sats_amount, Some(10000));
}

#[test]
fn attachment_payloads_distinguish_files_images_and_plain_chat() {
    let image = serde_json::json!({"type":"image_encrypted", "blossom_url":"https://example.invalid/image", "nonce":"synthetic", "mime_type":"image/png", "original_size":20, "encrypted_size":48, "width":2, "height":3});
    let file = serde_json::json!({"type":"file_encrypted", "file_type":"document", "blossom_url":"https://example.invalid/file", "nonce":"synthetic", "mime_type":"application/pdf", "original_size":20, "encrypted_size":48, "filename":"receipt.pdf"});
    for structured in [false, true] {
        let wrap = |v: &serde_json::Value| {
            if structured {
                serde_json::to_string(&Some(Payload::TextMessage(v.to_string()))).unwrap()
            } else {
                serde_json::to_string(&v.to_string()).unwrap()
            }
        };
        let encoded = wrap(&image);
        let meta = parse_attachment_metadata(&encoded).unwrap();
        assert_eq!(meta.filename(), "image");
        assert_eq!(meta.blossom_url(), "https://example.invalid/image");
        assert_eq!(parse_image_metadata(&encoded).unwrap().width, 2);
        let encoded = wrap(&file);
        let meta = parse_attachment_metadata(&encoded).unwrap();
        assert_eq!(meta.filename(), "receipt.pdf");
        assert_eq!(meta.blossom_url(), "https://example.invalid/file");
        assert!(parse_image_metadata(&encoded).is_none());
    }
    for invalid in ["not json", "null", "\"hello\"", "{\"TextMessage\":\"{}\"}"] {
        assert!(parse_attachment_metadata(invalid).is_none(), "{}", invalid);
    }
}

#[test]
fn encrypted_attachments_authenticate_nonce_content_and_key() {
    let key = [7; 32];
    for plaintext in [b"".as_slice(), b"synthetic attachment\0\xff".as_slice()] {
        let (blob, nonce) = encrypt_image_blob(&key, plaintext).unwrap();
        assert_eq!(blob.len(), plaintext.len() + 28);
        assert_eq!(&blob[..12], &nonce);
        assert_eq!(decrypt_image_blob(&key, &blob).unwrap(), plaintext);
        assert!(decrypt_image_blob(&[8; 32], &blob).is_err());
        for index in [0, blob.len() - 1] {
            let mut damaged = blob.clone();
            damaged[index] ^= 1;
            assert!(decrypt_image_blob(&key, &damaged).is_err());
        }
    }
    for len in 0..28 {
        assert!(decrypt_image_blob(&key, &vec![0; len]).is_err());
    }
}

#[test]
fn order_tags_parse_ranges_ratings_and_payment_methods() {
    let tags = Tags::from_list(vec![
        Tag::parse(["d", "00000000-0000-4000-8000-000000000001"]).unwrap(),
        Tag::parse(["k", "sell"]).unwrap(),
        Tag::parse(["f", "EUR"]).unwrap(),
        Tag::parse(["s", "pending"]).unwrap(),
        Tag::parse(["amt", "15000"]).unwrap(),
        Tag::parse(["fa", "50", "200"]).unwrap(),
        Tag::parse(["pm", "SEPA", " Cash "]).unwrap(),
        Tag::parse(["premium", "-2"]).unwrap(),
        Tag::parse([
            "rating",
            "[\"rating\",{\"total_rating\":4.5,\"total_reviews\":8,\"days\":30}]",
        ])
        .unwrap(),
    ]);
    let (order, rating) = order_from_tags(&tags);
    let p = small_order_to_p2p_order(&order, &rating, u64::MAX).unwrap();
    assert_eq!(p.order_type, OrderType::Sell);
    assert_eq!(p.fiat_currency, "EUR");
    assert_eq!(p.min_amount, Some(50.0));
    assert_eq!(p.max_amount, Some(200.0));
    assert_eq!(p.fiat_amount, 0.0);
    assert_eq!(p.sats_amount, Some(15000));
    assert_eq!(p.premium_percent, Some(-2.0));
    assert_eq!(p.payment_methods, ["SEPA", "Cash"]);
    assert_eq!(p.seller_rating, Some(4.5));
    assert_eq!(p.seller_reviews, Some(8));
    assert_eq!(p.seller_days_old, Some(30));
    assert_eq!(p.time_ago, "just now");
    assert!(small_order_to_p2p_order(&SmallOrder::default(), &rating, 0).is_none());
}

#[test]
fn malformed_order_fields_do_not_create_invented_amounts() {
    for fiat in ["1.5", "bad"] {
        let tags = Tags::from_list(vec![
            Tag::parse(["fa", fiat]).unwrap(),
            Tag::parse(["amt", "bad"]).unwrap(),
            Tag::parse(["s", "bad"]).unwrap(),
            Tag::parse(["premium", "bad"]).unwrap(),
        ]);
        let (order, rating) = order_from_tags(&tags);
        assert_eq!(order.fiat_amount, 0);
        assert_eq!(order.amount, 0);
        assert_eq!(order.premium, 0);
        assert_eq!(order.status, Some(Status::Pending));
        assert!(rating.total_rating.is_none());
    }
    for invalid in ["bad", "null", "[]", "[\"rating\",{}]"] {
        let r = parse_rating_tag(invalid);
        assert!(r.total_rating.is_none() && r.total_reviews.is_none() && r.days.is_none());
    }
}

#[test]
fn persisted_messages_deduplicate_exact_payloads_and_keep_chronological_order() {
    let cube = "synthetic-persistence";
    let s = session();
    save_data(
        cube,
        &MostroData {
            last_trade_index: 7,
            trades: vec![s.clone()],
        },
    )
    .unwrap();
    let msg = message(20, "SendDm", Some(Payload::TextMessage("first".into())));
    append_trade_message(cube, &s.order_id, msg.clone());
    append_trade_message(cube, &s.order_id, msg);
    append_trade_message(cube, &s.order_id, message(10, "FiatSent", None));
    append_trade_message(
        cube,
        &s.order_id,
        message(20, "SendDm", Some(Payload::TextMessage("different".into()))),
    );
    append_trade_message(cube, "unknown-order", message(30, "SendDm", None));
    let restored = load_data(cube).unwrap();
    assert_eq!(restored.last_trade_index, 7);
    assert_eq!(restored.trades[0].messages.len(), 3);
    assert_eq!(
        restored.trades[0]
            .messages
            .iter()
            .map(|m| m.timestamp)
            .collect::<Vec<_>>(),
        [10, 20, 20]
    );
    assert_eq!(get_trade_messages(cube, &s.order_id).len(), 3);
    assert!(get_trade_messages(cube, "unknown-order").is_empty());
    set_counterparty_pubkey(cube, &s.order_id, "peer");
    set_counterparty_pubkey(cube, &s.order_id, "peer");
    set_admin_pubkey(cube, &s.order_id, "solver");
    set_admin_pubkey(cube, "unknown-order", "ignored");
    let restored = load_data(cube).unwrap();
    assert_eq!(
        restored.trades[0].counterparty_trade_pubkey.as_deref(),
        Some("peer")
    );
    assert_eq!(
        restored.trades[0].admin_trade_pubkey.as_deref(),
        Some("solver")
    );
}

#[test]
fn failed_locked_transaction_leaves_previous_trade_data_unchanged() {
    let cube = "synthetic-transaction";
    assert!(load_data(cube).unwrap().trades.is_empty());
    save_data(
        cube,
        &MostroData {
            last_trade_index: 4,
            trades: vec![session()],
        },
    )
    .unwrap();
    let before = std::fs::read(data_file_path(cube).unwrap()).unwrap();
    let result: Result<(), String> = with_locked_data(cube, |data| {
        data.last_trade_index = 99;
        data.trades.clear();
        Err("synthetic rejection".into())
    });
    assert_eq!(result.unwrap_err(), "synthetic rejection");
    assert_eq!(
        std::fs::read(data_file_path(cube).unwrap()).unwrap(),
        before
    );
    with_locked_data(cube, |data| {
        data.last_trade_index += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(load_data(cube).unwrap().last_trade_index, 5);
    std::fs::write(data_file_path(cube).unwrap(), "{broken").unwrap();
    assert!(load_data(cube)
        .err()
        .unwrap()
        .contains("Failed to parse mostro data"));
    assert!(with_locked_data(cube, |_| Ok(())).is_err());
}

#[test]
fn order_cache_rejects_older_events_and_removes_taken_orders() {
    let keys = Keys::generate();
    let id = "00000000-0000-4000-8000-000000000001";
    let event = |time, status: &str, order_id: &str| {
        EventBuilder::new(nostr_sdk::Kind::Custom(38383), "")
            .tags([
                Tag::parse(["d", order_id]).unwrap(),
                Tag::parse(["k", "buy"]).unwrap(),
                Tag::parse(["s", status]).unwrap(),
                Tag::parse(["fa", "100"]).unwrap(),
            ])
            .custom_created_at(Timestamp::from(time))
            .sign_with_keys(&keys)
            .unwrap()
    };
    let mut cache = BTreeMap::new();
    assert!(!process_order_event(
        &event(1, "pending", "bad-id"),
        &mut cache
    ));
    assert!(process_order_event(&event(10, "pending", id), &mut cache));
    assert!(!process_order_event(&event(9, "pending", id), &mut cache));
    assert!(!process_order_event(&event(10, "pending", id), &mut cache));
    assert!(process_order_event(&event(11, "pending", id), &mut cache));
    let cube = "synthetic-cache";
    save_data(
        cube,
        &MostroData {
            last_trade_index: 1,
            trades: vec![session()],
        },
    )
    .unwrap();
    let orders = orders_from_cache(&cache, cube);
    assert_eq!(orders.len(), 1);
    assert!(orders[0].is_mine);
    assert_eq!(orders[0].created_at_ts, 11);
    assert!(process_order_event(&event(12, "active", id), &mut cache));
    assert!(cache.is_empty());
    assert!(!process_order_event(&event(13, "canceled", id), &mut cache));
}

#[test]
fn chat_identity_derivation_is_symmetric_and_invalid_indices_fail() {
    // Public BIP39 fixture; never a user's mnemonic.
    let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    let alice = derive_trade_keys(mnemonic, 1).unwrap();
    let bob = derive_trade_keys(mnemonic, 2).unwrap();
    assert_ne!(alice.public_key(), bob.public_key());
    assert!(derive_trade_keys(mnemonic, -1).is_err());
    assert!(derive_trade_keys(mnemonic, i64::from(u32::MAX) + 1).is_err());
    assert!(derive_trade_keys("invalid mnemonic", 1).is_err());
    assert_eq!(
        derive_encryption_key(&alice, &bob.public_key()).unwrap(),
        derive_encryption_key(&bob, &alice.public_key()).unwrap()
    );
    let alice_chat = derive_shared_chat_keys(&alice, &bob.public_key()).unwrap();
    let bob_chat = derive_shared_chat_keys(&bob, &alice.public_key()).unwrap();
    assert_eq!(alice_chat.public_key(), bob_chat.public_key());
    let cube = "synthetic-identity";
    assert!(get_chat_identity_info(cube, mnemonic, "missing")
        .our_trade_pubkey
        .is_none());
    let mut s = session();
    s.counterparty_trade_pubkey = Some(bob.public_key().to_hex());
    save_data(
        cube,
        &MostroData {
            last_trade_index: 2,
            trades: vec![s.clone()],
        },
    )
    .unwrap();
    let identity = get_chat_identity_info(cube, mnemonic, &s.order_id);
    assert_eq!(
        identity.our_trade_pubkey.as_deref(),
        Some(alice.public_key().to_hex().as_str())
    );
    assert_eq!(
        identity.shared_key.as_deref(),
        Some(alice_chat.public_key().to_hex().as_str())
    );
    assert_eq!(
        identity.our_nickname,
        Some(nickname_from_pubkey(&alice.public_key().to_hex()))
    );
    assert_eq!(
        shared_chat_hex(&alice, Some(&bob.public_key().to_hex())),
        identity.shared_key
    );
    assert!(shared_chat_hex(&alice, Some("not-hex")).is_none());
    assert!(shared_chat_hex(&alice, None).is_none());
}

#[test]
fn restored_status_maps_every_wire_status_like_the_relay_status() {
    // Every kebab-case status the restore response can carry must map to the
    // same TradeStatus once stored as a session action as it does when it
    // arrives as a live order status.
    for wire in [
        "active",
        "canceled",
        "canceled-by-admin",
        "settled-by-admin",
        "completed-by-admin",
        "dispute",
        "expired",
        "fiat-sent",
        "settled-hold-invoice",
        "pending",
        "success",
        "waiting-buyer-invoice",
        "waiting-payment",
        "cooperatively-canceled",
        "in-progress",
    ] {
        let status: Status = wire.parse().unwrap();
        let action = restored_status_action(wire);
        assert_eq!(
            dm_action_to_status(&action),
            Some(map_trade_status(&status)),
            "{wire} -> {action}"
        );
    }
    // The invoice form keys off the `WaitingBuyerInvoice` action name.
    assert_eq!(
        restored_status_action("waiting-buyer-invoice"),
        "WaitingBuyerInvoice"
    );
    // Unknown values are kept verbatim.
    assert_eq!(restored_status_action("mystery"), "mystery");
}

#[test]
fn restored_session_kind_stores_the_users_own_side() {
    use mostro_core::order::Kind;
    assert_eq!(restored_session_kind(Some(&Kind::Sell), "taker"), "buy");
    assert_eq!(restored_session_kind(Some(&Kind::Buy), "taker"), "sell");
    assert_eq!(restored_session_kind(Some(&Kind::Sell), "creator"), "sell");
    assert_eq!(restored_session_kind(Some(&Kind::Buy), "creator"), "buy");
    assert_eq!(restored_session_kind(Some(&Kind::Buy), "unknown"), "buy");
    assert_eq!(restored_session_kind(None, "taker"), "unknown");
}

#[test]
fn chat_history_keeps_only_chat_messages_of_trades_that_have_them() {
    let mut with_chat = session();
    with_chat.messages = vec![
        message(1, "PayInvoice", None),
        message(2, "SendDm", None),
        message(3, "AdminDm", None),
    ];
    let mut without_chat = session();
    without_chat.order_id = "00000000-0000-4000-8000-000000000002".into();
    without_chat.messages = vec![message(1, "FiatSent", None)];
    let history = chat_history_from_sessions(&[with_chat.clone(), without_chat]);
    assert_eq!(history.len(), 1);
    let chats = &history[&with_chat.order_id];
    assert_eq!(
        chats.iter().map(|m| m.action.as_str()).collect::<Vec<_>>(),
        ["SendDm", "AdminDm"]
    );
}
