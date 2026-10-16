use dekopon_capability::{ChatSlot, ChatSlotValues, HttpConstraints, RequestTemplate};
use dekopon_http_host::{BufferedHttpClient, ErrorCode, HttpHostCeilings, Request};
use std::time::Duration;

fn constraints() -> HttpConstraints {
    let rule: RequestTemplate = serde_json::from_value(serde_json::json!({
        "method":"GET", "path":"/api/conversations.replies",
        "query":{"pinned":{"channel":"conversation.id","ts":"conversation.thread"},"allowed":["limit","cursor"]}
    })).expect("fixture template");
    HttpConstraints {
        allowed_hosts: vec!["unresolvable.invalid".into()],
        allowed_methods: vec!["GET".into()],
        max_requests: 2,
        max_request_bytes: 1024,
        max_response_bytes: 65536,
        allow_plaintext_loopback: false,
        propagate_trace: false,
        request_templates: vec![rule],
    }
}

fn slots(thread: bool) -> ChatSlotValues {
    let mut slots = ChatSlotValues::default();
    slots.insert(ChatSlot::ConversationId, "Cclaim".into());
    if thread {
        slots.insert(ChatSlot::ConversationThread, "1760000000.000001".into());
    }
    slots
}

async fn denied(thread: bool, uri: &str) {
    let mut client = BufferedHttpClient::authorized(
        constraints(),
        HttpHostCeilings::default(),
        Duration::from_secs(2),
    )
    .expect("valid constraint")
    .with_chat_slots(slots(thread));
    let error = client
        .send(Request {
            method: "GET".into(),
            uri: format!("https://unresolvable.invalid{uri}"),
            headers: vec![],
            body: vec![],
        })
        .await
        .expect_err("template denies before DNS");
    assert!(matches!(error.code, ErrorCode::Denied));
    assert!(error.message.contains("request template"));
    assert!(client.into_evidence().is_empty());
}

#[tokio::test]
async fn missing_thread_and_foreign_channel_or_timestamp_deny_before_dns() {
    denied(false, "/api/conversations.replies?limit=5").await;
    denied(true, "/api/conversations.replies?limit=5&channel=Cforeign").await;
    denied(
        true,
        "/api/conversations.replies?limit=5&ts=1760000000.000002",
    )
    .await;
    denied(true, "/api/conversations.history?limit=5").await;
}

#[tokio::test]
async fn attested_root_slots_only_send_their_own_replies_uri() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    for (channel, root) in [
        ("DagentDm", "1760000000.000001"),
        ("CagentRoot", "1760000001.000002"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let receiver = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("one request");
            let mut bytes = [0; 2048];
            let n = socket.read(&mut bytes).expect("request bytes");
            let request = String::from_utf8_lossy(&bytes[..n]).to_string();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .expect("reply");
            request
        });
        let mut grant = constraints();
        grant.allowed_hosts = vec![address.to_string()];
        grant.allow_plaintext_loopback = true;
        let mut claim = ChatSlotValues::default();
        claim.insert(ChatSlot::ConversationId, channel.into());
        claim.insert(ChatSlot::ConversationThread, root.into());
        let mut client = BufferedHttpClient::authorized(
            grant,
            HttpHostCeilings::default(),
            Duration::from_secs(3),
        )
        .expect("valid grant")
        .with_chat_slots(claim);
        let response = client
            .send(Request {
                method: "GET".into(),
                uri: format!("http://{address}/api/conversations.replies?limit=1"),
                headers: vec![],
                body: vec![],
            })
            .await
            .expect("claimed root read");
        assert_eq!(response.status, 200);
        let captured = receiver.join().expect("receiver");
        assert!(
            captured.starts_with(&format!(
                "GET /api/conversations.replies?limit=1&channel={channel}&ts={root} HTTP/1.1\r\n"
            )),
            "{captured}"
        );
        assert!(!captured.contains("conversations.history"));
        assert!(!captured.to_ascii_lowercase().contains("authorization:"));
    }
}
