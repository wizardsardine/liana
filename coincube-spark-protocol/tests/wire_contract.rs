use coincube_spark_protocol::{
    ErrorKind, Event, Frame, GetInfoParams, Method, OkPayload, Request, Response, ResponseResult,
};
use serde_json::json;

#[test]
fn request_serializes_as_flat_newline_friendly_envelope() {
    let frame = Frame::Request(Request {
        id: 42,
        method: Method::GetInfo(GetInfoParams {
            ensure_synced: Some(true),
        }),
    });

    assert_eq!(
        serde_json::to_value(frame).unwrap(),
        json!({
            "type": "request",
            "id": 42,
            "method": "get_info",
            "params": {
                "ensure_synced": true
            }
        })
    );
}

#[test]
fn request_defaults_optional_pagination_fields_when_omitted() {
    let frame: Frame = serde_json::from_value(json!({
        "type": "request",
        "id": 7,
        "method": "list_payments",
        "params": {}
    }))
    .unwrap();

    let Frame::Request(request) = frame else {
        panic!("expected request frame");
    };
    let Method::ListPayments(params) = request.method else {
        panic!("expected list_payments method");
    };
    assert_eq!(request.id, 7);
    assert_eq!(params.limit, None);
    assert_eq!(params.offset, None);
}

#[test]
fn error_response_round_trips_with_typed_error_kind() {
    let response = Response::err(9, ErrorKind::NotConnected, "init must run first");
    let encoded = serde_json::to_string(&Frame::Response(response)).unwrap();
    let decoded: Frame = serde_json::from_str(&encoded).unwrap();

    let Frame::Response(response) = decoded else {
        panic!("expected response frame");
    };
    assert_eq!(response.id, 9);
    let ResponseResult::Err(error) = response.result else {
        panic!("expected error response");
    };
    assert_eq!(error.kind, ErrorKind::NotConnected);
    assert_eq!(error.message, "init must run first");
}

#[test]
fn shutdown_response_has_stable_wire_shape() {
    let frame = Frame::Response(Response::ok(11, OkPayload::Shutdown {}));

    assert_eq!(
        serde_json::to_value(frame).unwrap(),
        json!({
            "type": "response",
            "id": 11,
            "ok": {
                "kind": "shutdown",
                "data": {}
            }
        })
    );
}

#[test]
fn legacy_payment_succeeded_event_without_bolt11_remains_compatible() {
    let frame: Frame = serde_json::from_value(json!({
        "type": "event",
        "event": "payment_succeeded",
        "payload": {
            "id": "payment-1",
            "amount_sat": 1250
        }
    }))
    .unwrap();

    let Frame::Event(Event::PaymentSucceeded {
        id,
        amount_sat,
        bolt11,
    }) = frame
    else {
        panic!("expected payment_succeeded event");
    };
    assert_eq!(id, "payment-1");
    assert_eq!(amount_sat, 1250);
    assert_eq!(bolt11, None);
}

#[test]
fn unknown_method_is_rejected_instead_of_silently_accepted() {
    let error = serde_json::from_value::<Frame>(json!({
        "type": "request",
        "id": 1,
        "method": "future_method",
        "params": {}
    }))
    .unwrap_err();

    assert!(error.to_string().contains("unknown variant"));
}
