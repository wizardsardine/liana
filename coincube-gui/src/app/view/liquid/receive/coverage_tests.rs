use super::*;
use crate::app::view::shared::landing::capture;
use coincube_core::miniscript::bitcoin::Network;

fn inspect(element: Element<'_, LiquidReceiveMessage>) -> capture::Inspection {
    capture::inspect_rendered_full_window(
        "liquid-receive-behavior",
        element,
        (1024, 1800),
        "dark",
        theme::Theme::dark(),
    )
}

#[test]
fn swap_receive_waits_for_limits_and_shows_constraints_before_generation() {
    let input = form::Value::default();
    let render = |limits| {
        liquid_receive_view(
            "Synthetic",
            Network::Bitcoin,
            &ReceiveMethod::OnChain,
            None,
            None,
            false,
            &input,
            &input,
            "",
            BitcoinDisplayUnit::Sats,
            None,
            None,
            limits,
            SendAsset::Lbtc,
            SenderNetwork::Bitcoin,
            false,
            &[],
            Amount::ZERO,
            0,
            false,
            false,
        )
    };
    let waiting = inspect(render(None));
    assert!(waiting.contains("Fetching swap limits"));
    assert!(!waiting.contains("Generate Address"));
    let ready = inspect(render(Some((1000, 100000))));
    assert!(ready.contains("Generate Address"));
    let messages = capture::click_text(
        render(Some((1000, 100000))),
        "Generate Address",
        0,
        (1024, 1800),
    );
    assert_eq!(
        messages
            .iter()
            .filter(|m| matches!(m, LiquidReceiveMessage::GenerateAddress))
            .count(),
        1
    );
}

#[test]
fn generated_requests_keep_copy_action_and_clean_bip21_display() {
    let input = form::Value::default();
    let request = "bitcoin:bc1qsynthetic?amount=0.001&label=fixture".to_string();
    let qr = qr_code::Data::new(&request).unwrap();
    for (method, label) in [
        (ReceiveMethod::Lightning, "Lightning invoice"),
        (ReceiveMethod::Liquid, "Liquid address"),
        (ReceiveMethod::OnChain, "Swap address"),
        (ReceiveMethod::Usdt, "Liquid USDt address"),
    ] {
        let render = |with_qr| {
            liquid_receive_view(
                "Synthetic",
                Network::Bitcoin,
                &method,
                Some(&request),
                if with_qr { Some(&qr) } else { None },
                false,
                &input,
                &input,
                "",
                BitcoinDisplayUnit::Sats,
                None,
                Some((1000, 100000)),
                Some((1000, 100000)),
                SendAsset::Lbtc,
                SenderNetwork::Liquid,
                true,
                &[],
                Amount::from_sat(50000),
                0,
                false,
                false,
            )
        };
        let view = inspect(render(true));
        assert!(view.contains(label), "{}", label);
        assert!(view.contains("Enlarge QR"));
        if method == ReceiveMethod::OnChain {
            assert!(view.contains("bc1qsynthetic"));
            assert!(!view.contains("amount=0.001"));
        }
        let messages = capture::click_text(render(true), "Copy", 0, (1024, 1800));
        assert!(messages
            .iter()
            .any(|m| matches!(m, LiquidReceiveMessage::Copy)));
        assert!(inspect(render(false)).contains("QR unavailable"));
    }
}

#[test]
fn receive_errors_and_loading_do_not_expose_a_new_address() {
    let input = form::Value::default();
    let error = "Synthetic receive failure".to_string();
    let request = "synthetic request".to_string();
    let view = inspect(liquid_receive_view(
        "Synthetic",
        Network::Bitcoin,
        &ReceiveMethod::Liquid,
        Some(&request),
        None,
        true,
        &input,
        &input,
        "",
        BitcoinDisplayUnit::BTC,
        Some(&error),
        None,
        None,
        SendAsset::Lbtc,
        SenderNetwork::Liquid,
        false,
        &[],
        Amount::ZERO,
        0,
        false,
        true,
    ));
    assert!(view.contains("Generating Address"));
    assert!(view.contains("Synthetic receive failure"));
    assert!(!view.contains("synthetic request"));
    assert!(!view.contains("Copy"));
}

#[test]
fn stablecoin_receive_rejects_invalid_amounts_and_disables_busy_submit() {
    for (amount, valid) in [
        ("", true),
        ("0", false),
        ("-1", false),
        ("abc", false),
        ("1.25", true),
    ] {
        let input = form::Value {
            value: amount.into(),
            warning: None,
            valid: true,
        };
        for loading in [false, true] {
            let render = || usdt_input_fields(&input, loading);
            // The production continue control is the arrow icon, with no text label.
            let messages = capture::click_text(render(), "\u{F138}", 0, (1024, 1800));
            assert_eq!(
                messages
                    .iter()
                    .any(|m| matches!(m, LiquidReceiveMessage::GenerateAddress)),
                valid && !loading,
                "{} loading={}",
                amount,
                loading
            );
        }
    }
}

#[test]
fn sender_network_picker_emits_explicit_selection() {
    for network in [
        SenderNetwork::Lightning,
        SenderNetwork::Liquid,
        SenderNetwork::Bitcoin,
        SenderNetwork::Ethereum,
        SenderNetwork::Tron,
        SenderNetwork::Binance,
        SenderNetwork::Solana,
    ] {
        let asset = match network {
            SenderNetwork::Lightning | SenderNetwork::Bitcoin => SendAsset::Lbtc,
            _ => SendAsset::Usdt,
        };
        let view = inspect(sender_network_picker_modal(asset, network));
        let label = network.display_name().to_uppercase();
        assert!(view.contains(&label));
        let messages = capture::click_text(
            sender_network_picker_modal(asset, network),
            &label,
            0,
            (1024, 1800),
        );
        assert!(messages.iter().any(|m| matches!(m, LiquidReceiveMessage::SetSenderNetwork(selected) if *selected == network)));
    }
    for asset in [SendAsset::Lbtc, SendAsset::Usdt] {
        let view = inspect(receive_asset_picker_modal(asset));
        assert!(view.contains("L-BTC") && view.contains("USDt"));
        let label = if asset == SendAsset::Lbtc {
            "L-BTC"
        } else {
            "USDt"
        };
        let messages =
            capture::click_text(receive_asset_picker_modal(asset), label, 0, (1024, 1800));
        assert!(messages.iter().any(
            |m| matches!(m, LiquidReceiveMessage::SetReceiveAsset(selected) if *selected == asset)
        ));
    }
}
