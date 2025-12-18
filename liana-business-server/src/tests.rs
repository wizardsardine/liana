#[cfg(test)]
mod tests {
    use liana_connect::{Request, Response};
    use serde_json::json;
    use std::net::TcpStream;
    use std::thread;
    use std::time::Duration;
    use tungstenite::{connect, Message};
    use uuid::Uuid;

    fn start_test_server(port: u16) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut server = crate::server::Server::new("127.0.0.1", port).unwrap();
            // Server will run indefinitely
            let _ = server.run();
        })
    }

    fn wait_for_server(port: u16, timeout_secs: u64) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < timeout_secs {
            if TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
                return true;
            }
            thread::sleep(Duration::from_millis(100));
        }
        false
    }

    #[test]
    fn test_server_connection() {
        let port = 18080;
        let _server_handle = start_test_server(port);

        // Wait for server to start
        assert!(
            wait_for_server(port, 5),
            "Server failed to start within timeout"
        );

        // Connect to server
        let url = format!("ws://127.0.0.1:{}", port);
        let (mut socket, _response) = connect(url).expect("Failed to connect to server");

        // Send connect message
        let connect_msg = json!({
            "type": "connect",
            "token": "owner-token",
            "request_id": Uuid::new_v4().to_string(),
            "payload": {
                "version": 1
            }
        });

        socket
            .send(Message::Text(connect_msg.to_string()))
            .expect("Failed to send connect message");

        // Receive connected response
        let msg = socket.read().expect("Failed to read connected response");
        if let Message::Text(text) = msg {
            let response: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(response["type"], "connected");
            assert_eq!(response["payload"]["version"], 1);
        } else {
            panic!("Expected text message");
        }

        socket.close(None).ok();
    }

    #[test]
    fn test_invalid_token() {
        let port = 18081;
        let _server_handle = start_test_server(port);

        assert!(wait_for_server(port, 5), "Server failed to start");

        let url = format!("ws://127.0.0.1:{}", port);
        let (mut socket, _) = connect(url).expect("Failed to connect");

        // Send connect with invalid token
        let connect_msg = json!({
            "type": "connect",
            "token": "invalid-token-12345",
            "request_id": Uuid::new_v4().to_string(),
            "payload": {
                "version": 1
            }
        });

        socket
            .send(Message::Text(connect_msg.to_string()))
            .expect("Failed to send");

        // Expect error response
        let msg = socket.read().expect("Failed to read response");
        if let Message::Text(text) = msg {
            let response: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(response["type"], "error");
            assert_eq!(response["error"]["code"], "INVALID_TOKEN");
        } else {
            panic!("Expected text message");
        }

        socket.close(None).ok();
    }

    #[test]
    fn test_multi_client_broadcast() {
        let port = 18082;
        let _server_handle = start_test_server(port);

        assert!(wait_for_server(port, 5), "Server failed to start");

        let url = format!("ws://127.0.0.1:{}", port);

        // Connect client 1
        let (mut client1, _) = connect(&url).expect("Client 1 failed to connect");
        let connect_msg1 = json!({
            "type": "connect",
            "token": "owner-token",
            "request_id": Uuid::new_v4().to_string(),
            "payload": {"version": 1}
        });
        client1.send(Message::Text(connect_msg1.to_string())).ok();
        let _ = client1.read(); // Read connected response

        // Connect client 2
        let (mut client2, _) = connect(&url).expect("Client 2 failed to connect");
        let connect_msg2 = json!({
            "type": "connect",
            "token": "participant-token",
            "request_id": Uuid::new_v4().to_string(),
            "payload": {"version": 1}
        });
        client2.send(Message::Text(connect_msg2.to_string())).ok();
        let _ = client2.read(); // Read connected response

        // Client 1 fetches org
        let request_id = Uuid::new_v4().to_string();
        let fetch_org_msg = json!({
            "type": "fetch_org",
            "token": "owner-token",
            "request_id": request_id,
            "payload": {
                "id": Uuid::new_v4().to_string() // Random ID, will return error but that's ok for test
            }
        });

        client1
            .send(Message::Text(fetch_org_msg.to_string()))
            .ok();
        let _ = client1.read(); // Read response

        // Both clients should still be connected
        let ping_msg = json!({
            "type": "ping",
            "token": "owner-token",
            "request_id": Uuid::new_v4().to_string(),
            "payload": {}
        });

        client1.send(Message::Text(ping_msg.to_string())).ok();
        let msg = client1.read().expect("Client 1 should receive pong");
        if let Message::Text(text) = msg {
            let response: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(response["type"], "pong");
        }

        let ping_msg2 = json!({
            "type": "ping",
            "token": "participant-token",
            "request_id": Uuid::new_v4().to_string(),
            "payload": {}
        });

        client2.send(Message::Text(ping_msg2.to_string())).ok();
        let msg = client2.read().expect("Client 2 should receive pong");
        if let Message::Text(text) = msg {
            let response: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(response["type"], "pong");
        }

        client1.close(None).ok();
        client2.close(None).ok();
    }

    #[test]
    fn test_ping_pong() {
        let port = 18083;
        let _server_handle = start_test_server(port);

        assert!(wait_for_server(port, 5), "Server failed to start");

        let url = format!("ws://127.0.0.1:{}", port);
        let (mut socket, _) = connect(&url).expect("Failed to connect");

        // Connect
        let connect_msg = json!({
            "type": "connect",
            "token": "ws-manager-token",
            "request_id": Uuid::new_v4().to_string(),
            "payload": {"version": 1}
        });
        socket.send(Message::Text(connect_msg.to_string())).ok();
        let _ = socket.read(); // Read connected

        // Send ping
        let request_id = Uuid::new_v4().to_string();
        let ping_msg = json!({
            "type": "ping",
            "token": "ws-manager-token",
            "request_id": request_id.clone(),
            "payload": {}
        });

        socket.send(Message::Text(ping_msg.to_string())).ok();

        // Receive pong
        let msg = socket.read().expect("Failed to read pong");
        if let Message::Text(text) = msg {
            let response: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(response["type"], "pong");
            assert_eq!(response["request_id"], request_id);
        } else {
            panic!("Expected text message");
        }

        socket.close(None).ok();
    }
}

