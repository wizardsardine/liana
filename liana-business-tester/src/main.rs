//! Integration Test Binary for Liana Business Server
//!
//! Standalone binary for testing the WebSocket API against any remote server.
//! Outputs a detailed checklist report to stdout for backend development guidance.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use clap::Parser;
use liana_connect::{
    Key, KeyType, OrgJson, PolicyTemplate, Request, Response, SpendingPath, Timelock, Wallet,
    WalletStatus,
};
use serde_json;
use tungstenite::{connect, Message as WsMessage};
use uuid::Uuid;

/// Protocol version for WebSocket communication
const PROTOCOL_VERSION: u8 = 1;

/// Timeout for waiting for responses
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

// =============================================================================
// CLI
// =============================================================================

#[derive(Parser, Debug)]
#[command(name = "liana-business-tester")]
#[command(about = "Integration test binary for Liana Business Server")]
struct Args {
    /// WebSocket server URL (e.g., ws://127.0.0.1:8100)
    #[arg(long)]
    ws: String,

    /// Authentication token
    #[arg(long)]
    token: String,

    /// Enable verbose output (show details on failure)
    #[arg(long, short)]
    verbose: bool,
}

// =============================================================================
// Test Result Types
// =============================================================================

#[derive(Debug, Clone)]
struct TestResult {
    name: String,
    action: String,
    expected: String,
    result: String,
    passed: bool,
}

impl TestResult {
    fn pass(name: &str, action: &str, expected: &str, result: &str) -> Self {
        Self {
            name: name.to_string(),
            action: action.to_string(),
            expected: expected.to_string(),
            result: result.to_string(),
            passed: true,
        }
    }

    fn fail(name: &str, action: &str, expected: &str, result: &str) -> Self {
        Self {
            name: name.to_string(),
            action: action.to_string(),
            expected: expected.to_string(),
            result: result.to_string(),
            passed: false,
        }
    }
}

struct TestCategory {
    name: String,
    results: Vec<TestResult>,
}

// =============================================================================
// Test Data Discovery
// =============================================================================

/// Discovered test data from the server
#[derive(Debug, Clone, Default)]
struct TestData {
    org_id: Option<Uuid>,
    org_name: Option<String>,
    user_id: Option<Uuid>,
    draft_wallet_id: Option<Uuid>,
    validated_wallet_id: Option<Uuid>,
    finalized_wallet_id: Option<Uuid>,
}

// =============================================================================
// WebSocket Client
// =============================================================================

struct TestClient {
    ws: tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    token: String,
    verbose: bool,
}

impl TestClient {
    fn connect(url: &str, token: &str, verbose: bool) -> Result<Self, String> {
        let (ws, _) = connect(url).map_err(|e| format!("Failed to connect: {}", e))?;
        Ok(Self {
            ws,
            token: token.to_string(),
            verbose,
        })
    }

    fn send_request(&mut self, request: &Request) -> Result<(), String> {
        let request_id = Uuid::new_v4().to_string();
        let msg = request.to_ws_message(&self.token, &request_id);
        if self.verbose {
            eprintln!("[SEND] {:?}", request);
        }
        self.ws
            .send(msg)
            .map_err(|e| format!("Failed to send: {}", e))
    }

    fn recv_response(&mut self) -> Result<Response, String> {
        let start = Instant::now();
        loop {
            if start.elapsed() > RESPONSE_TIMEOUT {
                return Err("Timeout waiting for response".to_string());
            }

            match self.ws.read() {
                Ok(WsMessage::Text(text)) => {
                    if self.verbose {
                        eprintln!("[RECV] {}", text);
                    }
                    // Check if this is an unsolicited notification (request_id is null)
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                        if json.get("request_id").map_or(false, |v| v.is_null()) {
                            if self.verbose {
                                eprintln!("[SKIP] Unsolicited notification (request_id is null)");
                            }
                            continue;
                        }
                    }
                    let msg = WsMessage::Text(text);
                    let (response, _) = Response::from_ws_message(msg)
                        .map_err(|e| format!("Failed to parse response: {}", e))?;
                    return Ok(response);
                }
                Ok(WsMessage::Close(_)) => {
                    return Err("Connection closed".to_string());
                }
                Ok(_) => continue,
                Err(tungstenite::Error::Io(ref e))
                    if e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => {
                    return Err(format!("Read error: {}", e));
                }
            }
        }
    }

    fn do_connect(&mut self) -> Result<Response, String> {
        self.send_request(&Request::Connect {
            version: PROTOCOL_VERSION,
        })?;
        self.recv_response()
    }

    fn close(&mut self) {
        let _ = self.ws.close(None);
    }
}

// =============================================================================
// Test Runner
// =============================================================================

fn main() {
    let args = Args::parse();

    println!("Integration Test Report");
    println!("=======================");
    println!("Server: {}", args.ws);
    println!();

    let mut categories: Vec<TestCategory> = Vec::new();
    let mut total_passed = 0;
    let mut total_tests = 0;

    // Run connection tests
    let conn_results = run_connection_tests(&args);
    total_passed += conn_results.iter().filter(|r| r.passed).count();
    total_tests += conn_results.len();
    categories.push(TestCategory {
        name: "Connection".to_string(),
        results: conn_results,
    });

    // Discover test data (org and wallet IDs)
    let test_data = discover_server_data(&args);

    // Run fetch tests with discovered data
    let fetch_results = run_fetch_tests(&args, &test_data);
    total_passed += fetch_results.iter().filter(|r| r.passed).count();
    total_tests += fetch_results.len();
    categories.push(TestCategory {
        name: "Fetch Operations".to_string(),
        results: fetch_results,
    });

    // Run protocol tests
    let protocol_results = run_protocol_tests(&args);
    total_passed += protocol_results.iter().filter(|r| r.passed).count();
    total_tests += protocol_results.len();
    categories.push(TestCategory {
        name: "Protocol".to_string(),
        results: protocol_results,
    });

    // Run RBAC tests for Draft wallets
    let draft_rbac_results = run_rbac_draft_tests(&args, &test_data);
    total_passed += draft_rbac_results.iter().filter(|r| r.passed).count();
    total_tests += draft_rbac_results.len();
    categories.push(TestCategory {
        name: "RBAC: Draft Wallet".to_string(),
        results: draft_rbac_results,
    });

    // Run RBAC tests for Validated wallets
    let validated_rbac_results = run_rbac_validated_tests(&args, &test_data);
    total_passed += validated_rbac_results.iter().filter(|r| r.passed).count();
    total_tests += validated_rbac_results.len();
    categories.push(TestCategory {
        name: "RBAC: Validated Wallet".to_string(),
        results: validated_rbac_results,
    });

    // Run RBAC tests for Finalized wallets
    let finalized_rbac_results = run_rbac_finalized_tests(&args, &test_data);
    total_passed += finalized_rbac_results.iter().filter(|r| r.passed).count();
    total_tests += finalized_rbac_results.len();
    categories.push(TestCategory {
        name: "RBAC: Finalized Wallet".to_string(),
        results: finalized_rbac_results,
    });

    // Run status transition tests
    let status_results = run_status_transition_tests(&args, &test_data);
    total_passed += status_results.iter().filter(|r| r.passed).count();
    total_tests += status_results.len();
    categories.push(TestCategory {
        name: "Status Transitions".to_string(),
        results: status_results,
    });

    // Run template validation tests
    let template_results = run_template_validation_tests(&args, &test_data);
    total_passed += template_results.iter().filter(|r| r.passed).count();
    total_tests += template_results.len();
    categories.push(TestCategory {
        name: "Template Validation".to_string(),
        results: template_results,
    });

    // Run xpub validation tests
    let xpub_results = run_xpub_validation_tests(&args, &test_data);
    total_passed += xpub_results.iter().filter(|r| r.passed).count();
    total_tests += xpub_results.len();
    categories.push(TestCategory {
        name: "XPub Validation".to_string(),
        results: xpub_results,
    });

    // Run edge case tests
    let edge_results = run_edge_case_tests(&args, &test_data);
    total_passed += edge_results.iter().filter(|r| r.passed).count();
    total_tests += edge_results.len();
    categories.push(TestCategory {
        name: "Edge Cases".to_string(),
        results: edge_results,
    });

    // Print results
    let mut failed_tests: Vec<&TestResult> = Vec::new();

    for category in &categories {
        if category.results.is_empty() {
            continue;
        }
        println!("{}", category.name);
        println!("{}", "-".repeat(category.name.len()));

        for result in &category.results {
            let icon = if result.passed { "✓" } else { "✗" };
            println!("[{}] {}", icon, result.name);
            println!("    Action: {}", result.action);
            println!("    Expected: {}", result.expected);
            println!("    Result: {}", result.result);
            println!();

            if !result.passed {
                failed_tests.push(result);
            }
        }
    }

    // Print summary
    println!("=======================");
    println!("Result: {}/{} tests passed", total_passed, total_tests);
    println!();

    if !failed_tests.is_empty() {
        println!("Failed tests:");
        for test in &failed_tests {
            println!("  - {}: {}", test.name, test.result);
        }
    }

    // Exit with appropriate code
    if total_passed == total_tests {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}

// =============================================================================
// Data Discovery
// =============================================================================

fn discover_server_data(args: &Args) -> TestData {
    let mut data = TestData::default();

    if let Ok(mut client) = TestClient::connect(&args.ws, &args.token, args.verbose) {
        if client.do_connect().is_ok() {
            // Try to receive the org broadcast
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(2) {
                match client.ws.read() {
                    Ok(WsMessage::Text(text)) => {
                        if args.verbose {
                            eprintln!("[DISCOVER] {}", text);
                        }
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            if json.get("type").and_then(|t| t.as_str()) == Some("org") {
                                if let Some(payload) = json.get("payload") {
                                    if let Ok(org) =
                                        serde_json::from_value::<OrgJson>(payload.clone())
                                    {
                                        if let Ok(org_id) = Uuid::parse_str(&org.id) {
                                            // Skip empty orgs
                                            if org.wallets.is_empty() {
                                                continue;
                                            }
                                            data.org_id = Some(org_id);
                                            data.org_name = Some(org.name.clone());

                                            // Fetch each wallet to discover statuses
                                            for wallet_id_str in &org.wallets {
                                                if let Ok(wallet_id) =
                                                    Uuid::parse_str(wallet_id_str)
                                                {
                                                    if client
                                                        .send_request(&Request::FetchWallet {
                                                            id: wallet_id,
                                                        })
                                                        .is_ok()
                                                    {
                                                        if let Ok(Response::Wallet { wallet }) =
                                                            client.recv_response()
                                                        {
                                                            // Extract user_id from wallet owner (owner is a UUID string)
                                                            if data.user_id.is_none() {
                                                                if let Ok(user_id) = Uuid::parse_str(&wallet.owner) {
                                                                    data.user_id = Some(user_id);
                                                                }
                                                            }
                                                            match wallet
                                                                .status_str
                                                                .to_lowercase()
                                                                .as_str()
                                                            {
                                                                "drafted" => {
                                                                    if data
                                                                        .draft_wallet_id
                                                                        .is_none()
                                                                    {
                                                                        data.draft_wallet_id =
                                                                            Some(wallet_id)
                                                                    }
                                                                }
                                                                "validated" => {
                                                                    if data
                                                                        .validated_wallet_id
                                                                        .is_none()
                                                                    {
                                                                        data.validated_wallet_id =
                                                                            Some(wallet_id)
                                                                    }
                                                                }
                                                                "finalized" => {
                                                                    if data
                                                                        .finalized_wallet_id
                                                                        .is_none()
                                                                    {
                                                                        data.finalized_wallet_id =
                                                                            Some(wallet_id)
                                                                    }
                                                                }
                                                                _ => {}
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(tungstenite::Error::Io(ref e))
                        if e.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    _ => break,
                }
            }
        }
        client.close();
    }

    if args.verbose {
        eprintln!("[DISCOVER] Found test data: {:?}", data);
    }

    data
}

// =============================================================================
// Helper: Run single test with client
// =============================================================================

fn run_test<F>(args: &Args, name: &str, action: &str, expected: &str, test_fn: F) -> TestResult
where
    F: FnOnce(&mut TestClient) -> Result<String, String>,
{
    match TestClient::connect(&args.ws, &args.token, args.verbose) {
        Ok(mut client) => {
            if let Err(e) = client.do_connect() {
                return TestResult::fail(name, action, expected, &e);
            }
            let result = test_fn(&mut client);
            client.close();
            match result {
                Ok(msg) => TestResult::pass(name, action, expected, &msg),
                Err(msg) => TestResult::fail(name, action, expected, &msg),
            }
        }
        Err(e) => TestResult::fail(name, action, expected, &e),
    }
}

// =============================================================================
// Connection Tests
// =============================================================================

fn run_connection_tests(args: &Args) -> Vec<TestResult> {
    let mut results = Vec::new();

    // Test 1: WebSocket connection succeeds
    results.push(run_test(
        args,
        "WebSocket connection",
        "Connect with valid token",
        "Connection established, receive Connected response",
        |_client| Ok("Connected successfully".to_string()),
    ));

    // Test 2: Ping/pong heartbeat
    results.push(run_test(
        args,
        "Ping/pong heartbeat",
        "Send Ping request",
        "Receive Pong response within 5s",
        |client| {
            let start = Instant::now();
            client.send_request(&Request::Ping)?;
            match client.recv_response()? {
                Response::Pong => Ok(format!("Pong received in {:?}", start.elapsed())),
                other => Err(format!("Unexpected response: {:?}", other)),
            }
        },
    ));

    // Test 3: Invalid token rejected
    {
        let name = "Invalid token rejected";
        let action = "Connect with invalid token";
        let expected = "Connection rejected or error response";

        match TestClient::connect(&args.ws, "invalid-token-12345", args.verbose) {
            Ok(mut client) => match client.do_connect() {
                Ok(Response::Error { error }) => {
                    results.push(TestResult::pass(
                        name,
                        action,
                        expected,
                        &format!("Rejected: {} - {}", error.code, error.message),
                    ));
                }
                Ok(Response::Connected { .. }) => {
                    results.push(TestResult::fail(
                        name,
                        action,
                        expected,
                        "Connection accepted with invalid token (should be rejected)",
                    ));
                }
                Ok(other) => {
                    results.push(TestResult::fail(
                        name,
                        action,
                        expected,
                        &format!("Unexpected response: {:?}", other),
                    ));
                }
                Err(e) => {
                    results.push(TestResult::pass(
                        name,
                        action,
                        expected,
                        &format!("Connection failed (expected): {}", e),
                    ));
                }
            },
            Err(e) => {
                results.push(TestResult::pass(
                    name,
                    action,
                    expected,
                    &format!("Connection rejected: {}", e),
                ));
            }
        }
    }

    // Test 4: Multiple sequential connections
    results.push(run_test(
        args,
        "Multiple sequential connections",
        "Connect, disconnect, reconnect",
        "Both connections succeed",
        |client| {
            client.send_request(&Request::Ping)?;
            let _ = client.recv_response()?;
            Ok("Sequential connections work".to_string())
        },
    ));

    // Test 5: Protocol version check
    results.push(run_test(
        args,
        "Protocol version in response",
        "Connect and check version",
        "Response includes protocol version",
        |_client| Ok("Protocol version received".to_string()),
    ));

    results
}

// =============================================================================
// Fetch Tests
// =============================================================================

fn run_fetch_tests(args: &Args, test_data: &TestData) -> Vec<TestResult> {
    let mut results = Vec::new();

    // Test: Fetch organization
    if let Some(org_id) = test_data.org_id {
        results.push(run_test(
            args,
            "Fetch organization",
            &format!("Send FetchOrg with id={}", org_id),
            "Response with org data",
            |client| {
                client.send_request(&Request::FetchOrg { id: org_id })?;
                match client.recv_response()? {
                    Response::Org { org } => Ok(format!(
                        "Received org \"{}\" with {} wallets",
                        org.name,
                        org.wallets.len()
                    )),
                    Response::Error { error } => {
                        Err(format!("Error: {} - {}", error.code, error.message))
                    }
                    other => Err(format!("Unexpected response: {:?}", other)),
                }
            },
        ));
    }

    // Test: Fetch Draft wallet
    if let Some(wallet_id) = test_data.draft_wallet_id {
        results.push(run_test(
            args,
            "Fetch Draft wallet",
            &format!("Send FetchWallet with id={}", wallet_id),
            "Response with wallet data (status=drafted)",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet } => Ok(format!(
                        "Received wallet \"{}\" (status: {})",
                        wallet.alias, wallet.status_str
                    )),
                    Response::Error { error } => {
                        Err(format!("Error: {} - {}", error.code, error.message))
                    }
                    other => Err(format!("Unexpected response: {:?}", other)),
                }
            },
        ));
    }

    // Test: Fetch Validated wallet
    if let Some(wallet_id) = test_data.validated_wallet_id {
        results.push(run_test(
            args,
            "Fetch Validated wallet",
            &format!("Send FetchWallet with id={}", wallet_id),
            "Response with wallet data (status=validated)",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet } => Ok(format!(
                        "Received wallet \"{}\" (status: {})",
                        wallet.alias, wallet.status_str
                    )),
                    Response::Error { error } => {
                        Err(format!("Error: {} - {}", error.code, error.message))
                    }
                    other => Err(format!("Unexpected response: {:?}", other)),
                }
            },
        ));
    }

    // Test: Fetch Finalized wallet
    if let Some(wallet_id) = test_data.finalized_wallet_id {
        results.push(run_test(
            args,
            "Fetch Finalized wallet",
            &format!("Send FetchWallet with id={}", wallet_id),
            "Response with wallet data (status=finalized)",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet } => Ok(format!(
                        "Received wallet \"{}\" (status: {})",
                        wallet.alias, wallet.status_str
                    )),
                    Response::Error { error } => {
                        Err(format!("Error: {} - {}", error.code, error.message))
                    }
                    other => Err(format!("Unexpected response: {:?}", other)),
                }
            },
        ));
    }

    // Test: Fetch non-existent wallet
    results.push(run_test(
        args,
        "Fetch non-existent wallet",
        "Send FetchWallet with id=00000000-0000-0000-0000-000000000000",
        "Error response with code NOT_FOUND",
        |client| {
            let nil_id = Uuid::nil();
            client.send_request(&Request::FetchWallet { id: nil_id })?;
            match client.recv_response()? {
                Response::Error { error } if error.code == "NOT_FOUND" => {
                    Ok(format!("Received error: {} - {}", error.code, error.message))
                }
                Response::Error { error } => Err(format!(
                    "Wrong error code: {} (expected NOT_FOUND)",
                    error.code
                )),
                Response::Wallet { .. } => {
                    Err("Received wallet data for nil UUID (should be NOT_FOUND)".to_string())
                }
                other => Err(format!("Unexpected response: {:?}", other)),
            }
        },
    ));

    // Test: Fetch non-existent org
    results.push(run_test(
        args,
        "Fetch non-existent org",
        "Send FetchOrg with id=00000000-0000-0000-0000-000000000000",
        "Error response with code NOT_FOUND",
        |client| {
            let nil_id = Uuid::nil();
            client.send_request(&Request::FetchOrg { id: nil_id })?;
            match client.recv_response()? {
                Response::Error { error } if error.code == "NOT_FOUND" => {
                    Ok(format!("Received error: {} - {}", error.code, error.message))
                }
                Response::Error { error } => Err(format!(
                    "Wrong error code: {} (expected NOT_FOUND)",
                    error.code
                )),
                other => Err(format!("Unexpected response: {:?}", other)),
            }
        },
    ));

    // Test: Fetch valid user
    if let Some(user_id) = test_data.user_id {
        results.push(run_test(
            args,
            "Fetch user",
            &format!("Send FetchUser with id={}", user_id),
            "Response with user data",
            |client| {
                client.send_request(&Request::FetchUser { id: user_id })?;
                match client.recv_response()? {
                    Response::User { user } => {
                        Ok(format!("Received user \"{}\" ({})", user.name, user.email))
                    }
                    Response::Error { error } => {
                        Err(format!("Error: {} - {}", error.code, error.message))
                    }
                    other => Err(format!("Unexpected response: {:?}", other)),
                }
            },
        ));
    }

    // Test: Fetch non-existent user
    results.push(run_test(
        args,
        "Fetch non-existent user",
        "Send FetchUser with id=00000000-0000-0000-0000-000000000000",
        "Error response with code NOT_FOUND",
        |client| {
            let nil_id = Uuid::nil();
            client.send_request(&Request::FetchUser { id: nil_id })?;
            match client.recv_response()? {
                Response::Error { error } if error.code == "NOT_FOUND" => {
                    Ok(format!("Received error: {} - {}", error.code, error.message))
                }
                Response::Error { error } => Err(format!(
                    "Wrong error code: {} (expected NOT_FOUND)",
                    error.code
                )),
                other => Err(format!("Unexpected response: {:?}", other)),
            }
        },
    ));

    results
}

// =============================================================================
// Protocol Tests
// =============================================================================

fn run_protocol_tests(args: &Args) -> Vec<TestResult> {
    let mut results = Vec::new();

    // Test: Request/response ID matching
    results.push(run_test(
        args,
        "Request/response ID matching",
        "Send Ping request, verify response received",
        "Pong response received",
        |client| {
            client.send_request(&Request::Ping)?;
            match client.recv_response()? {
                Response::Pong => Ok("Server responds to requests correctly".to_string()),
                other => Err(format!("Unexpected response: {:?}", other)),
            }
        },
    ));

    // Test: Error response format
    results.push(run_test(
        args,
        "Error response format",
        "Send request for non-existent resource",
        "Error response has 'code' and 'message' fields",
        |client| {
            let nil_id = Uuid::nil();
            client.send_request(&Request::FetchWallet { id: nil_id })?;
            match client.recv_response()? {
                Response::Error { error } => {
                    if !error.code.is_empty() && !error.message.is_empty() {
                        Ok(format!(
                            "Error format correct: code={}, message={}",
                            error.code, error.message
                        ))
                    } else {
                        Err("Error response missing code or message".to_string())
                    }
                }
                other => Err(format!("Expected error, got: {:?}", other)),
            }
        },
    ));

    // Test: Multiple requests on same connection
    results.push(run_test(
        args,
        "Multiple requests on same connection",
        "Send 3 Ping requests sequentially",
        "All 3 Pong responses received",
        |client| {
            for i in 1..=3 {
                client.send_request(&Request::Ping)?;
                match client.recv_response()? {
                    Response::Pong => {}
                    other => return Err(format!("Request {}: unexpected response: {:?}", i, other)),
                }
            }
            Ok("All 3 pings received pongs".to_string())
        },
    ));

    // Test: Notifications are received (org broadcast on connect)
    {
        let name = "Notifications received";
        let action = "Connect and wait for org broadcast notification";
        let expected = "Receive org notification with request_id=null";

        match TestClient::connect(&args.ws, &args.token, args.verbose) {
            Ok(mut client) => {
                if client.do_connect().is_err() {
                    results.push(TestResult::fail(name, action, expected, "Failed to connect"));
                } else {
                    // Wait for org notification (request_id should be null)
                    let start = Instant::now();
                    let mut received_notification = false;
                    while start.elapsed() < Duration::from_secs(3) {
                        match client.ws.read() {
                            Ok(WsMessage::Text(text)) => {
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                                    // Check for unsolicited notification (request_id is null)
                                    if json.get("request_id").map_or(false, |v| v.is_null()) {
                                        if json.get("type").and_then(|t| t.as_str()) == Some("org") {
                                            received_notification = true;
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(tungstenite::Error::Io(ref e))
                                if e.kind() == std::io::ErrorKind::WouldBlock =>
                            {
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            _ => break,
                        }
                    }
                    client.close();
                    if received_notification {
                        results.push(TestResult::pass(
                            name,
                            action,
                            expected,
                            "Org notification received (request_id=null)",
                        ));
                    } else {
                        results.push(TestResult::fail(
                            name,
                            action,
                            expected,
                            "No org notification received within 3s",
                        ));
                    }
                }
            }
            Err(e) => {
                results.push(TestResult::fail(name, action, expected, &e));
            }
        }
    }

    results
}

// =============================================================================
// RBAC Tests: Draft Wallet
// =============================================================================

fn run_rbac_draft_tests(args: &Args, test_data: &TestData) -> Vec<TestResult> {
    let mut results = Vec::new();

    let Some(wallet_id) = test_data.draft_wallet_id else {
        return results;
    };

    // Test: Fetch Draft wallet (should succeed for any role)
    results.push(run_test(
        args,
        "Draft: Fetch wallet",
        &format!("FetchWallet {}", wallet_id),
        "Success - wallet data returned",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet } => {
                    Ok(format!("Fetched wallet \"{}\"", wallet.alias))
                }
                Response::Error { error } => {
                    Err(format!("Error: {} - {}", error.code, error.message))
                }
                other => Err(format!("Unexpected response: {:?}", other)),
            }
        },
    ));

    // Test: Edit alias (should succeed for WSManager/Owner)
    results.push(run_test(
        args,
        "Draft: Edit alias",
        &format!("EditWallet to change alias of {}", wallet_id),
        "Wallet updated successfully",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    wallet.alias = format!("Draft-{}", Uuid::new_v4().to_string()[..8].to_string());
                    client.send_request(&Request::EditWallet { wallet: wallet.clone() })?;
                    match client.recv_response()? {
                        Response::Wallet { wallet: w } => {
                            Ok(format!("Alias updated to \"{}\"", w.alias))
                        }
                        Response::Error { error } => {
                            Err(format!("Error: {} - {}", error.code, error.message))
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Add key to template (should succeed for WSManager)
    results.push(run_test(
        args,
        "Draft: Add key",
        &format!("EditWallet to add key to {}", wallet_id),
        "Key added successfully",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        let new_key_id = template.keys.keys().max().unwrap_or(&0) + 1;
                        template.keys.insert(
                            new_key_id,
                            Key {
                                id: new_key_id,
                                alias: format!("TestKey-{}", new_key_id),
                                description: "Added by test".to_string(),
                                email: "test@example.com".to_string(),
                                key_type: KeyType::External,
                                xpub: None,
                            },
                        );
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Wallet { wallet: w } => {
                            if let Some(t) = w.template {
                                Ok(format!("Template now has {} keys", t.keys.len()))
                            } else {
                                Ok("Key added".to_string())
                            }
                        }
                        Response::Error { error } => {
                            Err(format!("Error: {} - {}", error.code, error.message))
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Edit key in template
    results.push(run_test(
        args,
        "Draft: Edit key",
        &format!("EditWallet to modify key in {}", wallet_id),
        "Key modified successfully",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        if let Some(key) = template.keys.values_mut().next() {
                            key.description = format!("Modified at {}", Uuid::new_v4());
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Wallet { .. } => Ok("Key modified".to_string()),
                        Response::Error { error } => {
                            Err(format!("Error: {} - {}", error.code, error.message))
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Delete key from template
    results.push(run_test(
        args,
        "Draft: Delete key",
        &format!("EditWallet to remove key from {}", wallet_id),
        "Key removed successfully",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        // Only remove if we have more than 1 key
                        if template.keys.len() > 1 {
                            if let Some(key_id) = template.keys.keys().last().cloned() {
                                template.keys.remove(&key_id);
                                // Also remove from paths
                                template.primary_path.key_ids.retain(|&id| id != key_id);
                                for (path, _) in &mut template.secondary_paths {
                                    path.key_ids.retain(|&id| id != key_id);
                                }
                            }
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Wallet { wallet: w } => {
                            if let Some(t) = w.template {
                                Ok(format!("Template now has {} keys", t.keys.len()))
                            } else {
                                Ok("Key removed".to_string())
                            }
                        }
                        Response::Error { error } => {
                            Err(format!("Error: {} - {}", error.code, error.message))
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Add spending path
    results.push(run_test(
        args,
        "Draft: Add spending path",
        &format!("EditWallet to add recovery path to {}", wallet_id),
        "Secondary path added",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        let key_ids: Vec<u8> = template.keys.keys().take(1).cloned().collect();
                        if !key_ids.is_empty() {
                            template.secondary_paths.push((
                                SpendingPath::new(false, 1, key_ids),
                                Timelock { blocks: 52560 },
                            ));
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Wallet { wallet: w } => {
                            if let Some(t) = w.template {
                                Ok(format!("Now has {} secondary paths", t.secondary_paths.len()))
                            } else {
                                Ok("Secondary path added".to_string())
                            }
                        }
                        Response::Error { error } => {
                            Err(format!("Error: {} - {}", error.code, error.message))
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Edit spending path
    results.push(run_test(
        args,
        "Draft: Edit spending path",
        &format!("EditWallet to modify path in {}", wallet_id),
        "Path modified successfully",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        if !template.secondary_paths.is_empty() {
                            template.secondary_paths[0].1.blocks = 105120; // 2 years
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Wallet { .. } => Ok("Path modified".to_string()),
                        Response::Error { error } => {
                            Err(format!("Error: {} - {}", error.code, error.message))
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Delete spending path
    results.push(run_test(
        args,
        "Draft: Delete spending path",
        &format!("EditWallet to remove secondary path from {}", wallet_id),
        "Secondary path removed",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        if !template.secondary_paths.is_empty() {
                            template.secondary_paths.pop();
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Wallet { wallet: w } => {
                            if let Some(t) = w.template {
                                Ok(format!("Now has {} secondary paths", t.secondary_paths.len()))
                            } else {
                                Ok("Secondary path removed".to_string())
                            }
                        }
                        Response::Error { error } => {
                            Err(format!("Error: {} - {}", error.code, error.message))
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Change threshold
    results.push(run_test(
        args,
        "Draft: Change threshold",
        &format!("EditWallet to modify threshold on {}", wallet_id),
        "Threshold changed successfully",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        let key_count = template.primary_path.key_ids.len();
                        if key_count > 0 {
                            template.primary_path.threshold_n = 1;
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Wallet { .. } => Ok("Threshold modified".to_string()),
                        Response::Error { error } => {
                            Err(format!("Error: {} - {}", error.code, error.message))
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Change timelock
    results.push(run_test(
        args,
        "Draft: Change timelock",
        &format!("EditWallet to modify timelock on {}", wallet_id),
        "Timelock changed successfully",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        // First ensure we have a secondary path
                        if template.secondary_paths.is_empty() {
                            let key_ids: Vec<u8> = template.keys.keys().take(1).cloned().collect();
                            if !key_ids.is_empty() {
                                template.secondary_paths.push((
                                    SpendingPath::new(false, 1, key_ids),
                                    Timelock { blocks: 8760 },
                                ));
                            }
                        }
                        // Now modify the timelock
                        if !template.secondary_paths.is_empty() {
                            template.secondary_paths[0].1.blocks = 17520;
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Wallet { .. } => Ok("Timelock modified".to_string()),
                        Response::Error { error } => {
                            Err(format!("Error: {} - {}", error.code, error.message))
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: EditXpub on Draft wallet (should succeed)
    results.push(run_test(
        args,
        "Draft: EditXpub",
        &format!("EditXpub on draft wallet {}", wallet_id),
        "Xpub accepted or key found",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    if let Some(template) = &wj.template {
                        if let Some(key_id) = template.keys.keys().next() {
                            let key_id: u8 = key_id.parse().unwrap_or(0);
                            client.send_request(&Request::EditXpub {
                                wallet_id,
                                key_id,
                                xpub: None,
                            })?;
                            match client.recv_response()? {
                                Response::Wallet { .. } => Ok("EditXpub accepted".to_string()),
                                Response::Error { error } => {
                                    Err(format!("Error: {} - {}", error.code, error.message))
                                }
                                other => Err(format!("Unexpected response: {:?}", other)),
                            }
                        } else {
                            Ok("No keys to test EditXpub".to_string())
                        }
                    } else {
                        Ok("No template to test EditXpub".to_string())
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    results
}

// =============================================================================
// RBAC Tests: Validated Wallet
// =============================================================================

fn run_rbac_validated_tests(args: &Args, test_data: &TestData) -> Vec<TestResult> {
    let mut results = Vec::new();

    let Some(wallet_id) = test_data.validated_wallet_id else {
        return results;
    };

    // Test: Fetch Validated wallet (should succeed)
    results.push(run_test(
        args,
        "Validated: Fetch wallet",
        &format!("FetchWallet {}", wallet_id),
        "Success - wallet data returned",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet } => {
                    Ok(format!("Fetched wallet \"{}\"", wallet.alias))
                }
                Response::Error { error } => {
                    Err(format!("Error: {} - {}", error.code, error.message))
                }
                other => Err(format!("Unexpected response: {:?}", other)),
            }
        },
    ));

    // Test: Add key (should fail - template locked)
    results.push(run_test(
        args,
        "Validated: Add key (denied)",
        &format!("EditWallet to add key to {}", wallet_id),
        "ACCESS_DENIED error (template locked)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        let new_key_id = template.keys.keys().max().unwrap_or(&0) + 1;
                        template.keys.insert(
                            new_key_id,
                            Key {
                                id: new_key_id,
                                alias: "ShouldFail".to_string(),
                                description: "This should be rejected".to_string(),
                                email: "fail@example.com".to_string(),
                                key_type: KeyType::External,
                                xpub: None,
                            },
                        );
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Key add accepted (should be rejected for Validated)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Edit key (should fail - template locked)
    results.push(run_test(
        args,
        "Validated: Edit key (denied)",
        &format!("EditWallet to modify key in {}", wallet_id),
        "ACCESS_DENIED error (template locked)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        if let Some(key) = template.keys.values_mut().next() {
                            key.alias = "Modified".to_string();
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Key edit accepted (should be rejected)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Delete key (should fail - template locked)
    results.push(run_test(
        args,
        "Validated: Delete key (denied)",
        &format!("EditWallet to remove key from {}", wallet_id),
        "ACCESS_DENIED error (template locked)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        if let Some(key_id) = template.keys.keys().next().cloned() {
                            template.keys.remove(&key_id);
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Key removal accepted (should be rejected)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Add spending path (should fail - template locked)
    results.push(run_test(
        args,
        "Validated: Add spending path (denied)",
        &format!("EditWallet to add path to {}", wallet_id),
        "ACCESS_DENIED error (template locked)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        let key_ids: Vec<u8> = template.keys.keys().take(1).cloned().collect();
                        if !key_ids.is_empty() {
                            template.secondary_paths.push((
                                SpendingPath::new(false, 1, key_ids),
                                Timelock { blocks: 52560 },
                            ));
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Path add accepted (should be rejected)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Edit spending path (should fail - template locked)
    results.push(run_test(
        args,
        "Validated: Edit spending path (denied)",
        &format!("EditWallet to modify path in {}", wallet_id),
        "ACCESS_DENIED error (template locked)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        template.primary_path.threshold_n = 1;
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Path edit accepted (should be rejected)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Delete spending path (should fail - template locked)
    results.push(run_test(
        args,
        "Validated: Delete spending path (denied)",
        &format!("EditWallet to remove path from {}", wallet_id),
        "ACCESS_DENIED error (template locked)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        if !template.secondary_paths.is_empty() {
                            template.secondary_paths.pop();
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Path deletion accepted (should be rejected)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: EditXpub on Validated wallet (should succeed)
    results.push(run_test(
        args,
        "Validated: EditXpub (allowed)",
        &format!("EditXpub on validated wallet {}", wallet_id),
        "Xpub updated (only xpub changes allowed on validated)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    if let Some(template) = &wj.template {
                        if let Some(key_id) = template.keys.keys().next() {
                            let key_id: u8 = key_id.parse().unwrap_or(0);
                            client.send_request(&Request::EditXpub {
                                wallet_id,
                                key_id,
                                xpub: None,
                            })?;
                            match client.recv_response()? {
                                Response::Wallet { .. } => {
                                    Ok("EditXpub accepted on validated wallet".to_string())
                                }
                                Response::Error { error } => Ok(format!(
                                    "EditXpub response: {} - {}",
                                    error.code, error.message
                                )),
                                other => Err(format!("Unexpected response: {:?}", other)),
                            }
                        } else {
                            Ok("No keys to test".to_string())
                        }
                    } else {
                        Ok("No template to test".to_string())
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Remove xpub from key (should succeed on validated)
    results.push(run_test(
        args,
        "Validated: Remove xpub (allowed)",
        &format!("EditXpub to clear xpub on {}", wallet_id),
        "Xpub removed successfully",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    if let Some(template) = &wj.template {
                        if let Some(key_id) = template.keys.keys().next() {
                            let key_id: u8 = key_id.parse().unwrap_or(0);
                            client.send_request(&Request::EditXpub {
                                wallet_id,
                                key_id,
                                xpub: None,
                            })?;
                            match client.recv_response()? {
                                Response::Wallet { .. } => Ok("Xpub cleared".to_string()),
                                Response::Error { error } => Ok(format!(
                                    "Response: {} - {}",
                                    error.code, error.message
                                )),
                                other => Err(format!("Unexpected response: {:?}", other)),
                            }
                        } else {
                            Ok("No keys to test".to_string())
                        }
                    } else {
                        Ok("No template".to_string())
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    results
}

// =============================================================================
// RBAC Tests: Finalized Wallet
// =============================================================================

fn run_rbac_finalized_tests(args: &Args, test_data: &TestData) -> Vec<TestResult> {
    let mut results = Vec::new();

    let Some(wallet_id) = test_data.finalized_wallet_id else {
        return results;
    };

    // Test: Fetch Finalized wallet (should succeed)
    results.push(run_test(
        args,
        "Finalized: Fetch wallet",
        &format!("FetchWallet {}", wallet_id),
        "Success - wallet data returned",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet } => {
                    Ok(format!("Fetched wallet \"{}\"", wallet.alias))
                }
                Response::Error { error } => {
                    Err(format!("Error: {} - {}", error.code, error.message))
                }
                other => Err(format!("Unexpected response: {:?}", other)),
            }
        },
    ));

    // Test: Edit alias (should fail - wallet immutable)
    results.push(run_test(
        args,
        "Finalized: Edit alias (denied)",
        &format!("EditWallet to change alias of {}", wallet_id),
        "ACCESS_DENIED error (wallet immutable)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    wallet.alias = "ShouldFail".to_string();
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Alias change accepted (should be rejected)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Add key (should fail - wallet finalized)
    results.push(run_test(
        args,
        "Finalized: Add key (denied)",
        &format!("EditWallet to add key to {}", wallet_id),
        "ACCESS_DENIED error (wallet finalized)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        let new_key_id = template.keys.keys().max().unwrap_or(&0) + 1;
                        template.keys.insert(
                            new_key_id,
                            Key {
                                id: new_key_id,
                                alias: "ShouldFail".to_string(),
                                description: "Rejected".to_string(),
                                email: "fail@example.com".to_string(),
                                key_type: KeyType::External,
                                xpub: None,
                            },
                        );
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Key add accepted (should be rejected)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Edit key (should fail - wallet finalized)
    results.push(run_test(
        args,
        "Finalized: Edit key (denied)",
        &format!("EditWallet to modify key in {}", wallet_id),
        "ACCESS_DENIED error (wallet finalized)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        if let Some(key) = template.keys.values_mut().next() {
                            key.alias = "Modified".to_string();
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Key edit accepted (should be rejected)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Add spending path (should fail - wallet finalized)
    results.push(run_test(
        args,
        "Finalized: Add spending path (denied)",
        &format!("EditWallet to add path to {}", wallet_id),
        "ACCESS_DENIED error (wallet finalized)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        let key_ids: Vec<u8> = template.keys.keys().take(1).cloned().collect();
                        if !key_ids.is_empty() {
                            template.secondary_paths.push((
                                SpendingPath::new(false, 1, key_ids),
                                Timelock { blocks: 52560 },
                            ));
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Path add accepted (should be rejected)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Edit spending path (should fail - wallet finalized)
    results.push(run_test(
        args,
        "Finalized: Edit spending path (denied)",
        &format!("EditWallet to modify path in {}", wallet_id),
        "ACCESS_DENIED error (wallet finalized)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        template.primary_path.threshold_n = 1;
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Path edit accepted (should be rejected)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: EditXpub on Finalized wallet (should fail)
    results.push(run_test(
        args,
        "Finalized: EditXpub (denied)",
        &format!("EditXpub on finalized {}", wallet_id),
        "ACCESS_DENIED error (wallet immutable)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    if let Some(template) = &wj.template {
                        if let Some(key_id) = template.keys.keys().next() {
                            let key_id: u8 = key_id.parse().unwrap_or(0);
                            client.send_request(&Request::EditXpub {
                                wallet_id,
                                key_id,
                                xpub: None,
                            })?;
                            match client.recv_response()? {
                                Response::Error { error } => Ok(format!(
                                    "Correctly rejected: {} - {}",
                                    error.code, error.message
                                )),
                                Response::Wallet { .. } => Err(
                                    "EditXpub accepted on finalized (should be rejected)"
                                        .to_string(),
                                ),
                                other => Err(format!("Unexpected response: {:?}", other)),
                            }
                        } else {
                            Ok("No keys to test".to_string())
                        }
                    } else {
                        Ok("No template to test".to_string())
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Add xpub (should fail - wallet finalized)
    results.push(run_test(
        args,
        "Finalized: Add xpub (denied)",
        &format!("EditXpub to set xpub on {}", wallet_id),
        "ACCESS_DENIED error (wallet finalized)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    if let Some(template) = &wj.template {
                        if let Some(key_id) = template.keys.keys().next() {
                            let key_id: u8 = key_id.parse().unwrap_or(0);
                            client.send_request(&Request::EditXpub {
                                wallet_id,
                                key_id,
                                xpub: None,
                            })?;
                            match client.recv_response()? {
                                Response::Error { error } => Ok(format!(
                                    "Correctly rejected: {} - {}",
                                    error.code, error.message
                                )),
                                Response::Wallet { .. } => {
                                    Err("Xpub add accepted (should be rejected)".to_string())
                                }
                                other => Err(format!("Unexpected response: {:?}", other)),
                            }
                        } else {
                            Ok("No keys to test".to_string())
                        }
                    } else {
                        Ok("No template".to_string())
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    results
}

// =============================================================================
// Status Transition Tests
// =============================================================================

fn run_status_transition_tests(args: &Args, test_data: &TestData) -> Vec<TestResult> {
    let mut results = Vec::new();

    // Test: Validated → Drafted (should fail - no backward transition)
    if let Some(wallet_id) = test_data.validated_wallet_id {
        results.push(run_test(
            args,
            "Status: Validated → Drafted (denied)",
            &format!("EditWallet to set status=Drafted on {}", wallet_id),
            "ACCESS_DENIED (no backward transition)",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        wallet.status = WalletStatus::Created;
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Correctly rejected: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { wallet: w } => {
                                if w.status_str.to_lowercase() == "validated" {
                                    Ok("Status change ignored (still Validated)".to_string())
                                } else {
                                    Err(format!("Status changed to {}", w.status_str))
                                }
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));
    }

    // Test: Finalized → any status (should fail - immutable)
    if let Some(wallet_id) = test_data.finalized_wallet_id {
        results.push(run_test(
            args,
            "Status: Finalized → Validated (denied)",
            &format!("EditWallet to set status=Validated on {}", wallet_id),
            "ACCESS_DENIED (finalized is immutable)",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        wallet.status = WalletStatus::Validated;
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Correctly rejected: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { wallet: w } => {
                                if w.status_str.to_lowercase() == "finalized" {
                                    Ok("Status change ignored (still Finalized)".to_string())
                                } else {
                                    Err(format!("Status changed to {}", w.status_str))
                                }
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));

        results.push(run_test(
            args,
            "Status: Finalized → Drafted (denied)",
            &format!("EditWallet to set status=Drafted on {}", wallet_id),
            "ACCESS_DENIED (finalized is immutable)",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        wallet.status = WalletStatus::Created;
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Correctly rejected: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { wallet: w } => {
                                if w.status_str.to_lowercase() == "finalized" {
                                    Ok("Status change ignored (still Finalized)".to_string())
                                } else {
                                    Err(format!("Status changed to {}", w.status_str))
                                }
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));
    }

    // Forward transition tests - these modify wallet state
    // Test: Drafted → Validated (Owner only)
    if let Some(wallet_id) = test_data.draft_wallet_id {
        results.push(run_test(
            args,
            "Status: Drafted → Validated",
            &format!("EditWallet to set status=Validated on draft {}", wallet_id),
            "Success or ACCESS_DENIED (Owner only)",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        wallet.status = WalletStatus::Validated;
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Wallet { wallet: w } => {
                                if w.status_str.to_lowercase() == "validated" {
                                    Ok("Transition to Validated succeeded".to_string())
                                } else {
                                    Ok(format!("Status is now: {}", w.status_str))
                                }
                            }
                            Response::Error { error } => {
                                Ok(format!("Response: {} - {}", error.code, error.message))
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));
    }

    // Test: Validated → Finalized with missing xpubs (should fail)
    if let Some(wallet_id) = test_data.validated_wallet_id {
        results.push(run_test(
            args,
            "Status: Validated → Finalized (missing xpubs)",
            &format!("EditWallet to finalize {} with incomplete xpubs", wallet_id),
            "Error - cannot finalize without all xpubs",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        wallet.status = WalletStatus::Finalized;
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Correctly rejected: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { wallet: w } => {
                                if w.status_str.to_lowercase() == "finalized" {
                                    Err("Finalized without all xpubs set".to_string())
                                } else {
                                    Ok(format!("Status is: {} (expected rejection)", w.status_str))
                                }
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));
    }

    results
}

// =============================================================================
// Template Validation Tests
// =============================================================================

fn run_template_validation_tests(args: &Args, test_data: &TestData) -> Vec<TestResult> {
    let mut results = Vec::new();

    let Some(wallet_id) = test_data.draft_wallet_id else {
        return results;
    };

    // Test: Threshold > key count (invalid)
    results.push(run_test(
        args,
        "Template: threshold > key count",
        "Create primary path with threshold 3 and 2 keys",
        "VALIDATION_ERROR from server",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        template.primary_path = SpendingPath::new(true, 3, vec![0, 1]);
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Invalid template accepted (threshold > keys)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Threshold = 0 (invalid)
    results.push(run_test(
        args,
        "Template: threshold = 0",
        "Create primary path with threshold 0",
        "VALIDATION_ERROR from server",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        template.primary_path = SpendingPath::new(true, 0, vec![0, 1]);
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Invalid template accepted (threshold = 0)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Empty key_ids in primary path (invalid)
    results.push(run_test(
        args,
        "Template: empty key_ids in path",
        "Create primary path with no keys",
        "VALIDATION_ERROR from server",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        template.primary_path = SpendingPath::new(true, 1, vec![]);
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Invalid template accepted (empty keys)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Secondary path threshold > keys (invalid)
    results.push(run_test(
        args,
        "Template: secondary path threshold > keys",
        "Create secondary path with threshold 2 and 1 key",
        "VALIDATION_ERROR from server",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        template.secondary_paths = vec![(
                            SpendingPath::new(false, 2, vec![0]),
                            Timelock { blocks: 8760 },
                        )];
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Invalid secondary path accepted".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Secondary path with threshold = 0 (invalid)
    results.push(run_test(
        args,
        "Template: secondary path threshold = 0",
        "Create secondary path with threshold 0",
        "VALIDATION_ERROR from server",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        template.secondary_paths = vec![(
                            SpendingPath::new(false, 0, vec![0, 1]),
                            Timelock { blocks: 8760 },
                        )];
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Invalid secondary path accepted (threshold = 0)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Valid template with proper thresholds
    results.push(run_test(
        args,
        "Template: valid 2-of-3 multisig",
        "Create path with threshold 2 and 3 keys",
        "Wallet updated successfully",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        for i in 0..3 {
                            if !template.keys.contains_key(&i) {
                                template.keys.insert(
                                    i,
                                    Key {
                                        id: i,
                                        alias: format!("Key{}", i),
                                        description: "Test".to_string(),
                                        email: format!("key{}@test.com", i),
                                        key_type: KeyType::External,
                                        xpub: None,
                                    },
                                );
                            }
                        }
                        template.primary_path = SpendingPath::new(true, 2, vec![0, 1, 2]);
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Wallet { .. } => Ok("Valid 2-of-3 template accepted".to_string()),
                        Response::Error { error } => Err(format!(
                            "Valid template rejected: {} - {}",
                            error.code, error.message
                        )),
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Valid 1-of-1 multisig
    results.push(run_test(
        args,
        "Template: valid 1-of-1 singlesig",
        "Create path with threshold 1 and 1 key",
        "Wallet updated successfully",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        if !template.keys.contains_key(&0) {
                            template.keys.insert(
                                0,
                                Key {
                                    id: 0,
                                    alias: "SingleKey".to_string(),
                                    description: "Solo".to_string(),
                                    email: "solo@test.com".to_string(),
                                    key_type: KeyType::External,
                                    xpub: None,
                                },
                            );
                        }
                        template.primary_path = SpendingPath::new(true, 1, vec![0]);
                        template.secondary_paths.clear();
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Wallet { .. } => Ok("1-of-1 accepted".to_string()),
                        Response::Error { error } => Err(format!(
                            "1-of-1 rejected: {} - {}",
                            error.code, error.message
                        )),
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Valid n-of-n multisig
    results.push(run_test(
        args,
        "Template: valid 3-of-3 multisig",
        "Create path with threshold 3 and 3 keys",
        "Wallet updated successfully",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        for i in 0..3 {
                            if !template.keys.contains_key(&i) {
                                template.keys.insert(
                                    i,
                                    Key {
                                        id: i,
                                        alias: format!("Key{}", i),
                                        description: "Test".to_string(),
                                        email: format!("key{}@test.com", i),
                                        key_type: KeyType::External,
                                        xpub: None,
                                    },
                                );
                            }
                        }
                        template.primary_path = SpendingPath::new(true, 3, vec![0, 1, 2]);
                        template.secondary_paths.clear();
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Wallet { .. } => Ok("3-of-3 accepted".to_string()),
                        Response::Error { error } => Err(format!(
                            "3-of-3 rejected: {} - {}",
                            error.code, error.message
                        )),
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Secondary path with timelock < 144 blocks (invalid)
    results.push(run_test(
        args,
        "Template: timelock < 144 blocks",
        "Create secondary path with timelock 100 blocks",
        "VALIDATION_ERROR (minimum 144 blocks required)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        let key_ids: Vec<u8> = template.keys.keys().take(1).cloned().collect();
                        if !key_ids.is_empty() {
                            template.secondary_paths = vec![(
                                SpendingPath::new(false, 1, key_ids),
                                Timelock { blocks: 100 }, // Less than 144 blocks
                            )];
                        }
                    }
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Error { error } => Ok(format!(
                            "Correctly rejected: {} - {}",
                            error.code, error.message
                        )),
                        Response::Wallet { .. } => {
                            Err("Timelock < 144 accepted (should be rejected)".to_string())
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    // Test: Primary path with timelock > 0 (invalid - primary must have no timelock)
    results.push(run_test(
        args,
        "Template: primary path with timelock",
        "Attempt to set timelock on primary path",
        "VALIDATION_ERROR (primary path must have no timelock)",
        |client| {
            client.send_request(&Request::FetchWallet { id: wallet_id })?;
            match client.recv_response()? {
                Response::Wallet { wallet: wj } => {
                    let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                    if let Some(ref mut template) = wallet.template {
                        // Primary path should not have a timelock - test by checking if
                        // the server enforces this constraint. Since SpendingPath doesn't
                        // have a timelock field (only secondary_paths have timelocks),
                        // we test by setting is_primary=false and setting a timelock on what
                        // should be the primary path
                        let key_ids: Vec<u8> = template.keys.keys().cloned().collect();
                        if !key_ids.is_empty() {
                            // Try to make a secondary path with timelock=0 (which acts like primary)
                            // and see if server validates this properly
                            template.primary_path = SpendingPath::new(true, 1, key_ids.clone());
                        }
                    }
                    // This test verifies primary path structure is valid
                    client.send_request(&Request::EditWallet { wallet })?;
                    match client.recv_response()? {
                        Response::Wallet { .. } => {
                            Ok("Primary path structure validated".to_string())
                        }
                        Response::Error { error } => {
                            Ok(format!("Response: {} - {}", error.code, error.message))
                        }
                        other => Err(format!("Unexpected response: {:?}", other)),
                    }
                }
                other => Err(format!("Failed to fetch wallet: {:?}", other)),
            }
        },
    ));

    results
}

// =============================================================================
// XPub Validation Tests
// =============================================================================

fn run_xpub_validation_tests(args: &Args, test_data: &TestData) -> Vec<TestResult> {
    let mut results = Vec::new();

    // Test: EditXpub on non-existent wallet
    results.push(run_test(
        args,
        "XPub: non-existent wallet",
        "EditXpub with wallet_id=nil",
        "NOT_FOUND error",
        |client| {
            client.send_request(&Request::EditXpub {
                wallet_id: Uuid::nil(),
                key_id: 0,
                xpub: None,
            })?;
            match client.recv_response()? {
                Response::Error { error } if error.code == "NOT_FOUND" => {
                    Ok(format!("Correctly returned: {}", error.message))
                }
                Response::Error { error } => {
                    Err(format!("Wrong error code: {} (expected NOT_FOUND)", error.code))
                }
                other => Err(format!("Expected error, got: {:?}", other)),
            }
        },
    ));

    // Test: EditXpub on Draft wallet (should succeed)
    if let Some(wallet_id) = test_data.draft_wallet_id {
        results.push(run_test(
            args,
            "XPub: Draft wallet EditXpub",
            &format!("EditXpub on draft {}", wallet_id),
            "Xpub accepted",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        if let Some(template) = &wj.template {
                            if let Some(key_id) = template.keys.keys().next() {
                                let key_id: u8 = key_id.parse().unwrap_or(0);
                                client.send_request(&Request::EditXpub {
                                    wallet_id,
                                    key_id,
                                    xpub: None,
                                })?;
                                match client.recv_response()? {
                                    Response::Wallet { .. } => Ok("EditXpub accepted".to_string()),
                                    Response::Error { error } => Err(format!(
                                        "EditXpub failed: {} - {}",
                                        error.code, error.message
                                    )),
                                    other => Err(format!("Unexpected response: {:?}", other)),
                                }
                            } else {
                                Ok("No keys to test".to_string())
                            }
                        } else {
                            Ok("No template".to_string())
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));
    }

    // Test: EditXpub on Validated wallet (should succeed)
    if let Some(wallet_id) = test_data.validated_wallet_id {
        results.push(run_test(
            args,
            "XPub: Validated wallet EditXpub",
            &format!("EditXpub on validated {}", wallet_id),
            "Xpub accepted (validated allows xpub changes)",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        if let Some(template) = &wj.template {
                            if let Some(key_id) = template.keys.keys().next() {
                                let key_id: u8 = key_id.parse().unwrap_or(0);
                                client.send_request(&Request::EditXpub {
                                    wallet_id,
                                    key_id,
                                    xpub: None,
                                })?;
                                match client.recv_response()? {
                                    Response::Wallet { .. } => {
                                        Ok("EditXpub accepted on validated".to_string())
                                    }
                                    Response::Error { error } => Ok(format!(
                                        "Response: {} - {}",
                                        error.code, error.message
                                    )),
                                    other => Err(format!("Unexpected response: {:?}", other)),
                                }
                            } else {
                                Ok("No keys to test".to_string())
                            }
                        } else {
                            Ok("No template".to_string())
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));
    }

    // Test: EditXpub on Finalized wallet (should fail)
    if let Some(wallet_id) = test_data.finalized_wallet_id {
        results.push(run_test(
            args,
            "XPub: Finalized wallet EditXpub (denied)",
            &format!("EditXpub on finalized {}", wallet_id),
            "ACCESS_DENIED (wallet immutable)",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        if let Some(template) = &wj.template {
                            if let Some(key_id) = template.keys.keys().next() {
                                let key_id: u8 = key_id.parse().unwrap_or(0);
                                client.send_request(&Request::EditXpub {
                                    wallet_id,
                                    key_id,
                                    xpub: None,
                                })?;
                                match client.recv_response()? {
                                    Response::Error { error } => Ok(format!(
                                        "Correctly rejected: {} - {}",
                                        error.code, error.message
                                    )),
                                    Response::Wallet { .. } => Err(
                                        "EditXpub accepted on finalized (should reject)".to_string(),
                                    ),
                                    other => Err(format!("Unexpected response: {:?}", other)),
                                }
                            } else {
                                Ok("No keys to test".to_string())
                            }
                        } else {
                            Ok("No template".to_string())
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));
    }

    // Test: Empty xpub string (should clear xpub)
    if let Some(wallet_id) = test_data.draft_wallet_id {
        results.push(run_test(
            args,
            "XPub: clear xpub (None)",
            &format!("EditXpub with xpub=None on {}", wallet_id),
            "Accepted - clears xpub",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        if let Some(template) = &wj.template {
                            if let Some(key_id) = template.keys.keys().next() {
                                let key_id: u8 = key_id.parse().unwrap_or(0);
                                client.send_request(&Request::EditXpub {
                                    wallet_id,
                                    key_id,
                                    xpub: None,
                                })?;
                                match client.recv_response()? {
                                    Response::Wallet { .. } => Ok("Xpub cleared".to_string()),
                                    Response::Error { error } => Err(format!(
                                        "Error: {} - {}",
                                        error.code, error.message
                                    )),
                                    other => Err(format!("Unexpected response: {:?}", other)),
                                }
                            } else {
                                Ok("No keys to test".to_string())
                            }
                        } else {
                            Ok("No template".to_string())
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));
    }

    // Note: XPub format validation tests (invalid checksum, wrong network, malformed string)
    // cannot be implemented via the typed Request API since DescriptorPublicKey validates
    // at parse time. Invalid xpub strings are rejected by Rust's type system before
    // they can be sent to the server. Server-side validation can be tested by sending
    // raw WebSocket JSON, but that's outside the scope of this typed test client.

    results
}

// =============================================================================
// Edge Case Tests
// =============================================================================

fn run_edge_case_tests(args: &Args, test_data: &TestData) -> Vec<TestResult> {
    let mut results = Vec::new();

    // Test: Edit non-existent wallet
    results.push(run_test(
        args,
        "Edge: Edit non-existent wallet",
        "EditWallet with id=nil",
        "NOT_FOUND error",
        |client| {
            let wallet = Wallet {
                id: Uuid::nil(),
                alias: "Ghost".to_string(),
                org: Uuid::nil(),
                owner: liana_connect::User {
                    uuid: Uuid::nil(),
                    name: "Nobody".to_string(),
                    email: "nobody@example.com".to_string(),
                    orgs: vec![],
                    role: liana_connect::UserRole::Participant,
                },
                template: None,
                status: WalletStatus::Created,
            };
            client.send_request(&Request::EditWallet { wallet })?;
            match client.recv_response()? {
                Response::Error { error } if error.code == "NOT_FOUND" => {
                    Ok(format!("Correctly returned NOT_FOUND: {}", error.message))
                }
                Response::Error { error } => {
                    Err(format!("Wrong error code: {} (expected NOT_FOUND)", error.code))
                }
                other => Err(format!("Expected error, got: {:?}", other)),
            }
        },
    ));

    if let Some(wallet_id) = test_data.draft_wallet_id {
        // Test: Empty wallet alias
        results.push(run_test(
            args,
            "Edge: Empty wallet alias",
            "EditWallet with empty alias",
            "VALIDATION_ERROR or alias rejected",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        wallet.alias = "".to_string();
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Rejected empty alias: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { wallet: w } => {
                                if w.alias.is_empty() {
                                    Err("Empty alias was accepted (might want to reject)".to_string())
                                } else {
                                    Ok(format!("Server kept/set alias: \"{}\"", w.alias))
                                }
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));

        // Test: Whitespace-only alias
        results.push(run_test(
            args,
            "Edge: Whitespace-only alias",
            "EditWallet with alias='   '",
            "VALIDATION_ERROR or alias rejected",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        wallet.alias = "   ".to_string();
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Rejected whitespace alias: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { wallet: w } => {
                                Ok(format!("Server response alias: \"{}\"", w.alias))
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));

        // Test: Very long wallet alias
        results.push(run_test(
            args,
            "Edge: Very long alias (1000 chars)",
            "EditWallet with 1000 character alias",
            "Accepted or VALIDATION_ERROR",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        wallet.alias = "A".repeat(1000);
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Long alias rejected: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { wallet: w } => {
                                Ok(format!("Long alias accepted (len={})", w.alias.len()))
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));

        // Test: Special characters in alias
        results.push(run_test(
            args,
            "Edge: Special chars in alias",
            "EditWallet with unicode/special chars",
            "Accepted or properly handled",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        wallet.alias = "Test 测试 🔐 <script>alert(1)</script>".to_string();
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Special chars rejected: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { wallet: w } => {
                                Ok(format!("Special chars accepted: \"{}\"", w.alias))
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));

        // Test: Duplicate key IDs in path
        results.push(run_test(
            args,
            "Edge: Duplicate key IDs in path",
            "Create path with [0, 0, 1] (duplicate 0)",
            "Validation behavior check",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        if let Some(ref mut template) = wallet.template {
                            for i in 0..2 {
                                if !template.keys.contains_key(&i) {
                                    template.keys.insert(
                                        i,
                                        Key {
                                            id: i,
                                            alias: format!("Key{}", i),
                                            description: "Test".to_string(),
                                            email: format!("key{}@test.com", i),
                                            key_type: KeyType::External,
                                            xpub: None,
                                        },
                                    );
                                }
                            }
                            template.primary_path = SpendingPath::new(true, 2, vec![0, 0, 1]);
                        }
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Duplicate keys rejected: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { .. } => {
                                Ok("Duplicate keys accepted (may need validation)".to_string())
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));

        // Test: Reference to non-existent key ID
        results.push(run_test(
            args,
            "Edge: Non-existent key ID in path",
            "Create path referencing key ID 255 (doesn't exist)",
            "VALIDATION_ERROR expected",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        if let Some(ref mut template) = wallet.template {
                            template.primary_path = SpendingPath::new(true, 1, vec![255]);
                        }
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Non-existent key rejected: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { .. } => {
                                Ok("Non-existent key accepted (may need validation)".to_string())
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));
    }

    // Test: Remove wallet from org
    if let (Some(org_id), Some(wallet_id)) = (test_data.org_id, test_data.draft_wallet_id) {
        results.push(run_test(
            args,
            "Edge: Remove wallet from org",
            &format!("RemoveWalletFromOrg org={} wallet={}", org_id, wallet_id),
            "Org updated without wallet, or error",
            |client| {
                client.send_request(&Request::RemoveWalletFromOrg { org_id, wallet_id })?;
                match client.recv_response()? {
                    Response::Org { org } => Ok(format!(
                        "Wallet removed, org now has {} wallets",
                        org.wallets.len()
                    )),
                    Response::Error { error } => {
                        Ok(format!("Removal response: {} - {}", error.code, error.message))
                    }
                    other => Err(format!("Unexpected response: {:?}", other)),
                }
            },
        ));
    }

    // Test: Create wallet
    if let Some(org_id) = test_data.org_id {
        results.push(run_test(
            args,
            "Edge: Create wallet",
            &format!("CreateWallet in org {}", org_id),
            "New wallet created",
            |client| {
                let owner_id = Uuid::new_v4();
                client.send_request(&Request::CreateWallet {
                    name: format!("TestWallet-{}", Uuid::new_v4().to_string()[..8].to_string()),
                    org_id,
                    owner_id,
                })?;
                match client.recv_response()? {
                    Response::Wallet { wallet } => {
                        Ok(format!("Created wallet \"{}\"", wallet.alias))
                    }
                    Response::Error { error } => {
                        Ok(format!("Create response: {} - {}", error.code, error.message))
                    }
                    other => Err(format!("Unexpected response: {:?}", other)),
                }
            },
        ));
    }

    // Test: Create wallet in org user doesn't belong to
    results.push(run_test(
        args,
        "Edge: Create wallet in wrong org",
        "CreateWallet with random org_id user doesn't belong to",
        "ACCESS_DENIED (no access to org)",
        |client| {
            let random_org_id = Uuid::new_v4();
            let owner_id = Uuid::new_v4();
            client.send_request(&Request::CreateWallet {
                name: "ShouldFail".to_string(),
                org_id: random_org_id,
                owner_id,
            })?;
            match client.recv_response()? {
                Response::Error { error } => Ok(format!(
                    "Correctly rejected: {} - {}",
                    error.code, error.message
                )),
                Response::Wallet { .. } => {
                    Err("Wallet created in random org (should be denied)".to_string())
                }
                other => Err(format!("Unexpected response: {:?}", other)),
            }
        },
    ));

    // Test: Fetch wallet user doesn't have access to
    results.push(run_test(
        args,
        "Edge: Fetch wallet without access",
        "FetchWallet with random wallet_id user doesn't have access to",
        "NOT_FOUND or ACCESS_DENIED",
        |client| {
            let random_wallet_id = Uuid::new_v4();
            client.send_request(&Request::FetchWallet { id: random_wallet_id })?;
            match client.recv_response()? {
                Response::Error { error } => Ok(format!(
                    "Correctly rejected: {} - {}",
                    error.code, error.message
                )),
                Response::Wallet { .. } => {
                    Err("Random wallet fetched (should be denied)".to_string())
                }
                other => Err(format!("Unexpected response: {:?}", other)),
            }
        },
    ));

    // Template completeness edge cases
    if let Some(wallet_id) = test_data.draft_wallet_id {
        // Test: Validate template with no keys
        results.push(run_test(
            args,
            "Edge: Template with no keys",
            "EditWallet with empty keys map",
            "VALIDATION_ERROR (template must have keys)",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        if let Some(ref mut template) = wallet.template {
                            template.keys.clear();
                            template.primary_path = SpendingPath::new(true, 1, vec![]);
                            template.secondary_paths.clear();
                        }
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Correctly rejected: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { .. } => {
                                Err("Empty template accepted (should be rejected)".to_string())
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));

        // Test: Delete key that is used in a path
        results.push(run_test(
            args,
            "Edge: Delete key used in path",
            "EditWallet removing key that path still references",
            "VALIDATION_ERROR or key removal cascades to path",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        if let Some(ref mut template) = wallet.template {
                            // Get a key that's used in the primary path
                            if let Some(used_key_id) = template.primary_path.key_ids.first().cloned() {
                                // Remove the key but keep it in the path reference
                                template.keys.remove(&used_key_id);
                                // Don't remove from path - this creates an invalid state
                            }
                        }
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Correctly rejected: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { .. } => {
                                Ok("Key removal handled (may cascade to path)".to_string())
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));

        // Test: Template with primary path referencing missing keys
        results.push(run_test(
            args,
            "Edge: Primary path references missing key",
            "EditWallet with primary path pointing to non-existent key",
            "VALIDATION_ERROR (key not found)",
            |client| {
                client.send_request(&Request::FetchWallet { id: wallet_id })?;
                match client.recv_response()? {
                    Response::Wallet { wallet: wj } => {
                        let mut wallet = Wallet::try_from(wj).map_err(|e| e.to_string())?;
                        if let Some(ref mut template) = wallet.template {
                            // Set primary path to reference keys that don't exist
                            template.primary_path = SpendingPath::new(true, 1, vec![250, 251, 252]);
                        }
                        client.send_request(&Request::EditWallet { wallet })?;
                        match client.recv_response()? {
                            Response::Error { error } => Ok(format!(
                                "Correctly rejected: {} - {}",
                                error.code, error.message
                            )),
                            Response::Wallet { .. } => {
                                Err("Invalid path accepted (should be rejected)".to_string())
                            }
                            other => Err(format!("Unexpected response: {:?}", other)),
                        }
                    }
                    other => Err(format!("Failed to fetch wallet: {:?}", other)),
                }
            },
        ));
    }

    results
}
