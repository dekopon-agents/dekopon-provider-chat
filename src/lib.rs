use clap::{Parser, Subcommand};
use dekopon_provider_sdk::provider::endpoint::Base;
use dekopon_provider_sdk::provider::{
    Capability, Code, Failure, Http, Proposal, Provider, Stdout, Usage,
};
use dekopon_provider_sdk::provider::{HttpError, HttpErrorCode, Request, Response};
use dekopon_provider_sdk::{EffectKind, RiskLevel};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Write;

const SLACK: Base = Base::from_static("https://slack.com");
const DISCORD: Base = Base::from_static("https://discord.com");
const MAX_COUNT: u8 = 100;
const MAX_TEXT: usize = 4096;

#[derive(Parser)]
#[command(name = "chat", version, subcommand_required = true)]
pub struct Args {
    #[command(subcommand)]
    platform: Platform,
}

#[derive(Subcommand)]
enum Platform {
    Slack {
        #[command(subcommand)]
        action: Action,
    },
    Discord {
        #[command(subcommand)]
        action: Action,
    },
}

#[derive(Subcommand)]
enum Action {
    Get {
        #[command(subcommand)]
        selection: Selection,
    },
}

#[derive(Subcommand)]
enum Selection {
    Last { count: u8 },
}

pub struct Chat;

impl Provider for Chat {
    const ID: &'static str = "chat";
    const COMMAND_WORDS: &'static [&'static str] = &["chat"];
    const DESCRIPTION: &'static str = "Read text from an attested chat conversation";
    type Args = Args;
    type Capabilities = (SlackHistory, DiscordHistory);

    fn propose(args: Args, _stdin_piped: bool) -> Result<Proposal<Self>, Usage> {
        let (count, discord) = match args.platform {
            Platform::Slack {
                action:
                    Action::Get {
                        selection: Selection::Last { count },
                    },
            } => (count, false),
            Platform::Discord {
                action:
                    Action::Get {
                        selection: Selection::Last { count },
                    },
            } => (count, true),
        };
        if count == 0 || count > MAX_COUNT {
            return Err(Usage::new("chat get last: N must be 1..100"));
        }
        if discord {
            Ok(Proposal::to::<DiscordHistory>(HistoryInput { count }))
        } else {
            Ok(Proposal::to::<SlackHistory>(HistoryInput { count }))
        }
    }
}

#[allow(unsafe_code)]
mod export {
    dekopon_provider_sdk::export!(super::Chat);
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum HistoryError {
    InvalidCount,
    Transport,
    TooLarge,
    Permission,
    Throttled,
    Unavailable,
    FailedPage,
    InvalidResponse,
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidCount => "N must be 1..100",
            Self::Transport => "history request failed",
            Self::TooLarge => "history response too large; ask for fewer messages",
            Self::Permission => "history permission denied",
            Self::Throttled => "history throttled",
            Self::Unavailable => "history unavailable",
            Self::FailedPage => "history page failed",
            Self::InvalidResponse => "invalid history response",
        })
    }
}

impl Failure for HistoryError {
    fn code(&self) -> Code {
        match self {
            Self::InvalidCount => Code::INVALID_INPUT,
            Self::Transport => Code::new("http-failed"),
            Self::TooLarge => Code::new("response-too-large"),
            Self::Permission => Code::new("permission-denied"),
            Self::Throttled => Code::new("throttled"),
            Self::Unavailable => Code::new("history-unavailable"),
            Self::FailedPage => Code::new("page-failed"),
            Self::InvalidResponse => Code::new("invalid-response"),
        }
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryInput {
    #[schemars(range(min = 1, max = 100))]
    count: u8,
}

pub struct SlackHistory;

impl Capability for SlackHistory {
    type Provider = Chat;
    const NAME: &'static str = "slack.history.read";
    const DESCRIPTION: &'static str = "Read the newest reachable Slack thread messages; a thread beyond two pages may hide its tail";
    const EFFECT: EffectKind = EffectKind::ReadOnly;
    const RISK: RiskLevel = RiskLevel::Low;
    type Input = HistoryInput;
    type Needs = Http;
    type Error = HistoryError;

    fn run(input: Self::Input, http: Self::Needs, out: &mut Stdout) -> Result<(), Self::Error> {
        let result = read_slack(input.count, &SLACK, &mut |request| http.send(request))?;
        serde_json::to_writer(&mut *out, &result).map_err(|_| HistoryError::InvalidResponse)?;
        out.write_all(b"\n")
            .map_err(|_| HistoryError::InvalidResponse)
    }
}

pub struct DiscordHistory;

impl Capability for DiscordHistory {
    type Provider = Chat;
    const NAME: &'static str = "discord.history.read";
    const DESCRIPTION: &'static str =
        "Read up to 100 messages from the attested Discord channel or thread";
    const EFFECT: EffectKind = EffectKind::ReadOnly;
    const RISK: RiskLevel = RiskLevel::Low;
    type Input = HistoryInput;
    type Needs = Http;
    type Error = HistoryError;

    fn run(input: Self::Input, http: Self::Needs, out: &mut Stdout) -> Result<(), Self::Error> {
        let result = read_discord(input.count, &DISCORD, &mut |request| http.send(request))?;
        serde_json::to_writer(&mut *out, &result).map_err(|_| HistoryError::InvalidResponse)?;
        out.write_all(b"\n")
            .map_err(|_| HistoryError::InvalidResponse)
    }
}

#[derive(Serialize)]
struct History {
    messages: Vec<Message>,
    truncated: bool,
}

#[derive(Serialize)]
struct Message {
    id: String,
    author: String,
    time: String,
    text: String,
    assets: Vec<Asset>,
}

#[derive(Serialize)]
struct Asset {
    id: String,
    name: String,
    mime: String,
    size: Option<u64>,
    available: bool,
}

#[derive(Deserialize)]
struct Page {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    messages: Option<Vec<SlackMessage>>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    response_metadata: Option<Metadata>,
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(default)]
    next_cursor: String,
}

#[derive(Deserialize)]
struct SlackMessage {
    ts: String,
    #[serde(default)]
    user: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    files: Vec<SlackFile>,
}

#[derive(Deserialize)]
struct SlackFile {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    mimetype: String,
    size: Option<u64>,
}

#[derive(Deserialize)]
struct DiscordMessage {
    id: String,
    #[serde(default)]
    content: String,
    timestamp: String,
    author: DiscordAuthor,
    #[serde(default)]
    attachments: Vec<DiscordAttachment>,
}

#[derive(Deserialize)]
struct DiscordAuthor {
    id: String,
}

#[derive(Deserialize)]
struct DiscordAttachment {
    id: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    content_type: String,
    size: Option<u64>,
}

fn read_discord(
    count: u8,
    base: &Base,
    send: &mut dyn FnMut(Request) -> Result<Response, HttpError>,
) -> Result<History, HistoryError> {
    if count == 0 || count > MAX_COUNT {
        return Err(HistoryError::InvalidCount);
    }
    let mut messages = Vec::new();
    let mut before = None::<String>;
    let mut truncated = false;
    for page_number in 0..2 {
        let limit = (count as usize + 1 - messages.len()).min(50);
        let mut uri = base
            .join(&format!(
                "/api/v10/channels/{{conversation.apiChannel}}/messages?limit={limit}"
            ))
            .map_err(|_| HistoryError::InvalidResponse)?;
        if let Some(ref id) = before {
            uri.push_str("&before=");
            uri.push_str(id);
        }
        let response = send(Request::new("GET", uri).map_err(|_| HistoryError::InvalidResponse)?)
            .map_err(history_http_error)?;
        match response.status {
            200..=299 => (),
            401 | 403 => return Err(HistoryError::Permission),
            429 => return Err(HistoryError::Throttled),
            404 | 410 => return Err(HistoryError::Unavailable),
            _ => return Err(HistoryError::FailedPage),
        }
        let page: Vec<DiscordMessage> =
            serde_json::from_slice(&response.body).map_err(|_| HistoryError::InvalidResponse)?;
        if page.len() > limit {
            return Err(HistoryError::InvalidResponse);
        }
        let full = page.len() == limit;
        let oldest = page.last().map(|message| message.id.clone());
        for message in page {
            if !message.id.bytes().all(|b| b.is_ascii_digit())
                || message.id.is_empty()
                || message.id.len() > 32
            {
                return Err(HistoryError::InvalidResponse);
            }
            messages.push(Message {
                id: bounded(&message.id, 64),
                author: bounded(&message.author.id, 256),
                time: bounded(&message.timestamp, 64),
                text: bounded(&message.content, MAX_TEXT),
                assets: message
                    .attachments
                    .into_iter()
                    .take(10)
                    .map(|asset| Asset {
                        id: bounded(&asset.id, 256),
                        name: bounded(&asset.filename, 256),
                        mime: bounded(&asset.content_type, 128),
                        size: asset.size,
                        available: false,
                    })
                    .collect(),
            });
        }
        if !full {
            break;
        }
        if messages.len() > count as usize || page_number == 1 {
            break;
        }
        before = oldest;
    }
    if messages.len() > count as usize {
        messages.truncate(count as usize);
        truncated = true;
    }
    messages.sort_by(|a, b| a.time.cmp(&b.time).then(a.id.cmp(&b.id)));
    Ok(History {
        messages,
        truncated,
    })
}

fn history_http_error(error: HttpError) -> HistoryError {
    if error.code == HttpErrorCode::ResponseTooLarge {
        HistoryError::TooLarge
    } else {
        HistoryError::Transport
    }
}

fn bounded(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn timestamp(ts: &str) -> Option<(u64, u64)> {
    let (seconds, fraction) = ts.split_once('.')?;
    if fraction.is_empty() || fraction.len() > 9 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((
        seconds.parse().ok()?,
        fraction.parse::<u64>().ok()? * 10_u64.pow(9 - fraction.len() as u32),
    ))
}

fn read_slack(
    count: u8,
    base: &Base,
    send: &mut dyn FnMut(Request) -> Result<Response, HttpError>,
) -> Result<History, HistoryError> {
    if count == 0 || count > MAX_COUNT {
        return Err(HistoryError::InvalidCount);
    }
    let mut messages = Vec::new();
    let mut cursor = None::<String>;
    let mut truncated = false;
    for page_number in 0..2 {
        let mut uri = base
            .join("/api/conversations.replies?limit=200")
            .map_err(|_| HistoryError::InvalidResponse)?;
        if let Some(ref cursor) = cursor {
            uri.push_str("&cursor=");
            for byte in cursor.bytes() {
                if byte.is_ascii_alphanumeric() || b"-_~.".contains(&byte) {
                    uri.push(char::from(byte));
                } else {
                    use std::fmt::Write as _;
                    write!(uri, "%{byte:02X}").map_err(|_| HistoryError::InvalidResponse)?;
                }
            }
        }
        let request = Request::new("GET", uri).map_err(|_| HistoryError::InvalidResponse)?;
        let response = send(request).map_err(history_http_error)?;
        match response.status {
            200..=299 => (),
            401 | 403 => return Err(HistoryError::Permission),
            429 => return Err(HistoryError::Throttled),
            404 | 410 => return Err(HistoryError::Unavailable),
            _ => return Err(HistoryError::FailedPage),
        }
        let page: Page =
            serde_json::from_slice(&response.body).map_err(|_| HistoryError::InvalidResponse)?;
        if !page.ok {
            return Err(match page.error.as_deref() {
                Some("ratelimited") => HistoryError::Throttled,
                Some(
                    "missing_scope" | "not_authed" | "not_in_channel" | "access_denied"
                    | "invalid_auth" | "token_revoked" | "account_inactive",
                ) => HistoryError::Permission,
                Some("channel_not_found" | "thread_not_found") => HistoryError::Unavailable,
                _ => HistoryError::FailedPage,
            });
        }
        let page_messages = page.messages.ok_or(HistoryError::InvalidResponse)?;
        let length = page_messages.len();
        if length > 200 {
            return Err(HistoryError::InvalidResponse);
        }
        for message in page_messages {
            if timestamp(&message.ts).is_none() {
                return Err(HistoryError::InvalidResponse);
            }
            if messages.iter().any(|seen: &Message| seen.id == message.ts) {
                continue;
            }
            messages.push(Message {
                id: bounded(&message.ts, 64),
                time: bounded(&message.ts, 64),
                author: bounded(&message.user, 256),
                text: bounded(&message.text, MAX_TEXT),
                assets: message
                    .files
                    .into_iter()
                    .take(10)
                    .map(|file| Asset {
                        id: bounded(&file.id, 256),
                        name: bounded(&file.name, 256),
                        mime: bounded(&file.mimetype, 128),
                        size: file.size,
                        available: false,
                    })
                    .collect(),
            });
        }
        if !page.has_more {
            break;
        }
        if page_number == 1 {
            break;
        }
        let next = page
            .response_metadata
            .map(|m| m.next_cursor)
            .unwrap_or_default();
        if length == 0
            || next.is_empty()
            || next.len() > 256
            || !next.is_ascii()
            || next.bytes().any(|b| b.is_ascii_control())
        {
            return Err(HistoryError::InvalidResponse);
        }
        cursor = Some(next);
    }
    messages.sort_by_key(|message| timestamp(&message.time));
    if messages.len() > count as usize {
        messages.drain(..messages.len() - count as usize);
        truncated = true;
    }
    Ok(History {
        messages,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn response(status: u16, body: serde_json::Value) -> Result<Response, HttpError> {
        Ok(Response {
            status,
            headers: vec![],
            body: body.to_string().into_bytes(),
        })
    }

    #[test]
    fn native_slack_repeated_root_keeps_latest_seen_messages() {
        let mut calls = Vec::new();
        let result = read_slack(2, &SLACK, &mut |request| {
            calls.push(request.uri);
            if calls.len() == 1 {
                response(200, json!({"ok":true,"has_more":true,"response_metadata":{"next_cursor":"next"},"messages":[
                    {"ts":"1760000000.000001","user":"root","text":"root"},
                    {"ts":"1760000000.000002","user":"U2","text":"older"}
                ]}))
            } else {
                response(200, json!({"ok":true,"has_more":false,"messages":[
                    {"ts":"1760000000.000001","user":"root","text":"root"},
                    {"ts":"1760000000.000003","user":"U3","text":"newer"},
                    {"ts":"1760000000.000004","user":"U4","text":"newest"}
                ]}))
            }
        }).expect("two pages");
        assert_eq!(
            calls,
            [
                "https://slack.com/api/conversations.replies?limit=200",
                "https://slack.com/api/conversations.replies?limit=200&cursor=next",
            ]
        );
        assert_eq!(
            result
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>(),
            ["newer", "newest"]
        );
        assert!(result.truncated);
    }

    #[test]
    fn native_slack_pages_sort_and_keep_descriptive_assets() {
        let mut calls = Vec::new();
        let result = read_slack(3, &SLACK, &mut |request| {
            calls.push(request.uri);
            if calls.len() == 1 {
                response(200, json!({"ok":true,"has_more":true,"response_metadata":{"next_cursor":"abc=12"},"messages":[
                    {"ts":"1760000000.000002","user":"U2","text":"second"}
                ]}))
            } else {
                response(200, json!({"ok":true,"has_more":true,"messages":[
                    {"ts":"1760000000.000003","user":"U3","text":"third","files":[{"id":"F1","name":"photo","mimetype":"image/png"}]},
                    {"ts":"1760000000.000001","user":"U1","text":"first"}
                ]}))
            }
        }).expect("two pages");
        assert_eq!(
            calls,
            [
                "https://slack.com/api/conversations.replies?limit=200",
                "https://slack.com/api/conversations.replies?limit=200&cursor=abc%3D12",
            ]
        );
        assert_eq!(
            result
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
        assert!(!result.truncated);
        let serialized = serde_json::to_value(&result).expect("output");
        assert_eq!(serialized["messages"][2]["assets"][0]["available"], false);
        assert!(serialized["messages"][2]["assets"][0]["size"].is_null());
    }

    #[test]
    fn native_empty_is_success_but_permission_throttle_and_failed_second_page_are_errors() {
        let empty = read_slack(1, &SLACK, &mut |_| {
            response(200, json!({"ok":true,"messages":[]}))
        })
        .expect("empty");
        assert!(empty.messages.is_empty());
        assert!(!empty.truncated);
        for (status, body, expected) in [
            (403, json!({}), HistoryError::Permission),
            (429, json!({}), HistoryError::Throttled),
            (
                200,
                json!({"ok":false,"error":"not_in_channel"}),
                HistoryError::Permission,
            ),
            (
                200,
                json!({"ok":false,"error":"ratelimited"}),
                HistoryError::Throttled,
            ),
            (
                200,
                json!({"ok":false,"error":"thread_not_found"}),
                HistoryError::Unavailable,
            ),
            (
                200,
                json!({"ok":false,"error":"invalid_auth"}),
                HistoryError::Permission,
            ),
            (
                200,
                json!({"ok":false,"error":"token_revoked"}),
                HistoryError::Permission,
            ),
            (
                200,
                json!({"ok":false,"error":"account_inactive"}),
                HistoryError::Permission,
            ),
        ] {
            assert!(
                matches!(read_slack(1, &SLACK, &mut |_| response(status, body.clone())), Err(e) if e == expected)
            );
        }
        let mut calls = 0;
        let result = read_slack(2, &SLACK, &mut |_| {
            calls += 1;
            if calls == 1 {
                response(
                    200,
                    json!({"ok":true,"has_more":true,"response_metadata":{"next_cursor":"abc"},"messages":[{"ts":"1760000000.000001"}]}),
                )
            } else {
                response(500, json!({"error":"private server detail"}))
            }
        });
        assert!(matches!(result, Err(HistoryError::FailedPage)));
        assert_eq!(calls, 2);
    }

    #[test]
    fn native_slack_accepts_page_above_former_guest_byte_limit() {
        let long_text = "a".repeat(70 * 1024);
        let result = read_slack(1, &SLACK, &mut |_| {
            response(
                200,
                json!({"ok":true,"messages":[
                    {"ts":"1760000000.000001","user":"U1","text":long_text}
                ]}),
            )
        })
        .expect("broker-bounded page");
        assert_eq!(result.messages[0].text.len(), MAX_TEXT);
        assert!(!result.truncated);
    }

    #[test]
    fn native_missing_messages_cannot_masquerade_as_empty_history() {
        let result = read_slack(1, &SLACK, &mut |_| response(200, json!({"ok":true})));
        assert!(matches!(result, Err(HistoryError::InvalidResponse)));
    }

    #[test]
    fn native_discord_pages_bound_count_and_preserve_descriptive_assets() {
        let page: Vec<_> = (151..=200)
            .rev()
            .map(|id| {
                json!({
                    "id": id.to_string(), "author":{"id":"raw-user"},
                    "timestamp": format!("2026-01-01T00:00:{:02}Z", id - 150),
                    "content": "earlier text", "attachments": if id == 200 {
                        vec![json!({"id":"F1","filename":"photo.png","content_type":"image/png"})]
                    } else { vec![] }
                })
            })
            .collect();
        let mut calls = Vec::new();
        let result = read_discord(51, &DISCORD, &mut |request| {
            calls.push(request.uri);
            if calls.len() == 1 { response(200, json!(page)) } else {
                response(200, json!([
                    {"id":"150","author":{"id":"raw-first"},"timestamp":"2026-01-01T00:00:00Z","content":"first"},
                    {"id":"149","author":{"id":"raw-oldest"},"timestamp":"2025-12-31T00:00:00Z","content":"older"}
                ]))
            }
        }).expect("bounded read");
        assert_eq!(
            calls,
            [
                "https://discord.com/api/v10/channels/{conversation.apiChannel}/messages?limit=50",
                "https://discord.com/api/v10/channels/{conversation.apiChannel}/messages?limit=2&before=151",
            ]
        );
        assert_eq!(result.messages.len(), 51);
        assert_eq!(result.messages[0].text, "first");
        assert_eq!(result.messages[50].author, "raw-user");
        let output = serde_json::to_value(&result).expect("output");
        assert_eq!(output["messages"][50]["assets"][0]["available"], false);
        assert!(output["messages"][50]["assets"][0]["size"].is_null());
        assert!(result.truncated);
    }

    #[test]
    fn native_discord_empty_errors_failed_page_and_host_byte_failure() {
        let empty = read_discord(2, &DISCORD, &mut |_| response(200, json!([]))).expect("empty");
        assert!(empty.messages.is_empty());
        assert!(!empty.truncated);
        for (status, expected) in [
            (403, HistoryError::Permission),
            (429, HistoryError::Throttled),
            (404, HistoryError::Unavailable),
            (500, HistoryError::FailedPage),
        ] {
            assert!(
                matches!(read_discord(1, &DISCORD, &mut |_| response(status, json!({}))), Err(e) if e == expected)
            );
        }
        assert!(matches!(
            read_discord(1, &DISCORD, &mut |_| response(
                200,
                json!({"message":"error"})
            )),
            Err(HistoryError::InvalidResponse)
        ));
        let oversized = HttpError {
            code: HttpErrorCode::ResponseTooLarge,
            message: "decoded response exceeded broker grant".into(),
        };
        assert!(matches!(
            read_discord(1, &DISCORD, &mut |_| Err(oversized.clone())),
            Err(HistoryError::TooLarge)
        ));
        assert_eq!(
            HistoryError::TooLarge.code(),
            Code::new("response-too-large")
        );
        assert!(HistoryError::TooLarge.to_string().contains("ask for fewer"));
        assert!(matches!(
            read_slack(1, &SLACK, &mut |_| Err(oversized.clone())),
            Err(HistoryError::TooLarge)
        ));
        let page: Vec<_> = (151..=200)
            .rev()
            .map(|id| {
                json!({
                    "id":id.to_string(), "author":{"id":"U"}, "timestamp":"2026-01-01T00:00:00Z"
                })
            })
            .collect();
        let mut calls = 0;
        let failed = read_discord(51, &DISCORD, &mut |_| {
            calls += 1;
            if calls == 1 {
                response(200, json!(page))
            } else {
                response(500, json!({}))
            }
        });
        assert!(matches!(failed, Err(HistoryError::FailedPage)));
        assert_eq!(calls, 2);
    }
}
