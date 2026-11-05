use dekopon_chat_provider::Chat;
use dekopon_provider_sdk::provider::{Provider, Response};
use dekopon_provider_sdk::{CommandRunOutcome, EffectKind, provider};
use dekopon_provider_sdk_testkit::{Harness, HttpScript, Native, conformance};
use serde_json::{Value, json};

fn component() -> std::path::PathBuf {
    std::env::var_os("DEKOPON_PROVIDER_COMPONENT")
        .expect("DEKOPON_PROVIDER_COMPONENT must name the freshly built component")
        .into()
}

fn response(body: Value) -> Response {
    Response {
        status: 200,
        headers: vec![],
        body: body.to_string().into_bytes(),
    }
}

#[test]
fn read_manifest_has_no_write_or_asset_capability() {
    let manifest = provider::manifest::<Chat>().expect("manifest");
    assert_eq!(<Chat as Provider>::COMMAND_WORDS, &["chat"]);
    assert_eq!(manifest.capabilities.len(), 2);
    assert_eq!(
        manifest.capabilities[0].id.as_str(),
        "chat.slack.history.read"
    );
    assert_eq!(manifest.capabilities[0].effect, EffectKind::ReadOnly);
    assert_eq!(
        manifest.capabilities[1].id.as_str(),
        "chat.discord.history.read"
    );
    assert_eq!(manifest.capabilities[1].effect, EffectKind::ReadOnly);
    for words in [
        ["send", "hello", "now", ""],
        ["slack", "get", "last", "0"],
        ["slack", "get", "last", "101"],
        ["discord", "get", "last", "0"],
        ["discord", "get", "last", "101"],
    ] {
        assert!(matches!(
            provider::command::<Chat>(&words.map(str::to_owned), false),
            CommandRunOutcome::Failed { .. } | CommandRunOutcome::Rendered { status: 2, .. }
        ));
    }
}

#[test]
fn component_rejects_invalid_slack_count_and_native_reads_earlier_text()
-> Result<(), Box<dyn std::error::Error>> {
    conformance::<Chat>(component())?;
    let scripted = Harness::<Chat>::get(component()).http(HttpScript::new("localhost", "GET", response(json!({
        "ok":true,"messages":[{"ts":"1760000000.000003","user":"Uraw","text":"earlier text"}],"has_more":false
    }))));
    let refused = scripted.call("chat.slack.history.read", json!({"count":0}))?;
    assert_ne!(refused.status, 0);
    assert!(refused.http_calls.is_empty());
    let native = Native::<Chat>::new().http(HttpScript::new("slack.com", "GET", response(json!({
        "ok":true,"messages":[{"ts":"1760000000.000003","user":"Uraw","text":"earlier text"}],"has_more":false
    }))));
    let read = native.call("chat.slack.history.read", &json!({"count":1}).to_string());
    assert_eq!(read.status, 0, "{}", read.stderr);
    let sent = native.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].uri,
        "https://slack.com/api/conversations.replies?limit=20"
    );
    assert!(
        sent[0]
            .headers
            .iter()
            .all(|h| !h.name.eq_ignore_ascii_case("authorization"))
    );
    let result: Value = serde_json::from_slice(&read.stdout)?;
    assert_eq!(result["messages"][0]["text"], "earlier text");
    assert_eq!(result["messages"][0]["author"], "Uraw");
    assert_eq!(result["truncated"], false);
    Ok(())
}

#[test]
fn invalid_counts_make_no_native_requests() {
    for input in [
        json!({}),
        json!({"count":null}),
        json!({"count":0}),
        json!({"count":-1}),
        json!({"count":1.5}),
        json!({"count":101}),
    ] {
        let native = Native::<Chat>::new();
        let result = native.call("chat.slack.history.read", &input.to_string());
        assert_ne!(result.status, 0, "{input}");
        assert!(native.requests().is_empty(), "{input}");
        let discord = Native::<Chat>::new();
        assert_ne!(
            discord
                .call("chat.discord.history.read", &input.to_string())
                .status,
            0
        );
        assert!(discord.requests().is_empty(), "{input}");
    }
}

#[test]
fn component_discord_refuses_invalid_input_and_native_reads_earlier_text()
-> Result<(), Box<dyn std::error::Error>> {
    conformance::<Chat>(component())?;
    let scripted = Harness::<Chat>::get(component()).http(HttpScript::new(
        "localhost",
        "GET",
        response(json!([])),
    ));
    let refused = scripted.call("chat.discord.history.read", json!({"count":0}))?;
    assert_ne!(refused.status, 0);
    assert!(refused.http_calls.is_empty());
    let native = Native::<Chat>::new().http(HttpScript::new("discord.com", "GET", response(json!([
        {"id":"176000000000000001","timestamp":"2026-01-01T00:00:00Z","author":{"id":"raw-user"},"content":"earlier text","attachments":[]}
    ]))));
    let read = native.call("chat.discord.history.read", &json!({"count":2}).to_string());
    assert_eq!(read.status, 0, "{}", read.stderr);
    let sent = native.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].uri,
        "https://discord.com/api/v10/channels/{conversation.apiChannel}/messages?limit=3"
    );
    assert!(
        sent[0]
            .headers
            .iter()
            .all(|h| !h.name.eq_ignore_ascii_case("authorization"))
    );
    let result: Value = serde_json::from_slice(&read.stdout)?;
    assert_eq!(result["messages"][0]["text"], "earlier text");
    assert_eq!(result["messages"][0]["author"], "raw-user");
    assert_eq!(result["truncated"], false);
    Ok(())
}

const FOOTER: &str = "\u{203a} model-a \u{b7} 2 steps \u{b7} 3 s";

fn marked_upload(user: &str) -> Value {
    json!({
        "ts":"1760000000.000002","user":user,"bot_id":"B_SYNTHETIC_BOT","upload":true,
        "text":format!("Synthetic answer. {FOOTER}"),
        "files":[{"id":"F_SYNTHETIC_IMAGE","name":"cat.png","mimetype":"image/png","size":42}],
        "blocks":[
            {"type":"rich_text","elements":[{"type":"rich_text_section","elements":[{"type":"text","text":"Synthetic answer."}]}]},
            {"type":"context","block_id":"dekopon-footer:00000000-0000-4000-8000-000000000001",
             "elements":[{"type":"plain_text","text":FOOTER}]}
        ]
    })
}

fn slack_read(page_and_identity: Value) -> (Value, Vec<String>) {
    let native = Native::<Chat>::new().http(HttpScript::new(
        "slack.com",
        "GET",
        response(page_and_identity),
    ));
    let read = native.call("chat.slack.history.read", &json!({"count":1}).to_string());
    assert_eq!(read.status, 0, "{}", read.stderr);
    let sent = native.requests();
    assert!(sent.iter().all(|request| {
        request
            .headers
            .iter()
            .all(|h| !h.name.eq_ignore_ascii_case("authorization"))
    }));
    (
        serde_json::from_slice(&read.stdout).expect("history JSON"),
        sent.into_iter().map(|request| request.uri).collect(),
    )
}

#[test]
fn native_chat_output_drops_the_bots_marked_footer_after_one_auth_test() {
    let (result, uris) = slack_read(json!({
        "ok":true,"user_id":"U_SYNTHETIC_BOT","url":"https://private.example/","team":"Private Team",
        "messages":[marked_upload("U_SYNTHETIC_BOT")]
    }));
    assert_eq!(
        uris,
        [
            "https://slack.com/api/conversations.replies?limit=20",
            "https://slack.com/api/auth.test",
        ]
    );
    assert_eq!(
        result,
        json!({"messages":[{"id":"1760000000.000002","author":"U_SYNTHETIC_BOT","time":"1760000000.000002",
            "text":"Synthetic answer.","assets":[{"id":"F_SYNTHETIC_IMAGE","name":"cat.png","mime":"image/png",
            "size":42,"available":false}]}],"truncated":false})
    );
}

#[test]
fn native_chat_output_keeps_a_foreign_authors_marked_footer() {
    let (result, uris) = slack_read(json!({
        "ok":true,"user_id":"U_SYNTHETIC_BOT","messages":[marked_upload("U_SYNTHETIC_HUMAN")]
    }));
    assert_eq!(uris.len(), 2);
    assert_eq!(
        result["messages"][0]["text"],
        format!("Synthetic answer. {FOOTER}")
    );
}
