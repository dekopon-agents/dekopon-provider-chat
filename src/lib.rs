use clap::{Parser, Subcommand};
use dekopon_provider_sdk::provider::endpoint::Base;
use dekopon_provider_sdk::provider::{
    Capability, Code, Failure, Http, Proposal, Provider, Stdout, Usage,
};
use dekopon_provider_sdk::provider::{HttpError, Request, Response};
use dekopon_provider_sdk::{EffectKind, RiskLevel};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Write;

const SLACK: Base = Base::from_static("https://slack.com");
const MAX_COUNT: u8 = 100;
const MAX_BODY: usize = 64 * 1024;
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
    type Capabilities = (SlackHistory,);

    fn propose(args: Args, _stdin_piped: bool) -> Result<Proposal<Self>, Usage> {
        let Platform::Slack {
            action:
                Action::Get {
                    selection: Selection::Last { count },
                },
        } = args.platform;
        if count == 0 || count > MAX_COUNT {
            return Err(Usage::new("chat slack get last: N must be 1..100"));
        }
        Ok(Proposal::to::<SlackHistory>(HistoryInput { count }))
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
    const DESCRIPTION: &'static str = "Read up to 100 messages from the attested Slack thread";
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
            .join(&format!(
                "/api/conversations.replies?limit={}",
                count as usize - messages.len()
            ))
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
        let response = send(request).map_err(|_| HistoryError::Transport)?;
        match response.status {
            200..=299 => (),
            401 | 403 => return Err(HistoryError::Permission),
            429 => return Err(HistoryError::Throttled),
            404 | 410 => return Err(HistoryError::Unavailable),
            _ => return Err(HistoryError::FailedPage),
        }
        if response.body.len() > MAX_BODY {
            return Err(HistoryError::InvalidResponse);
        }
        let page: Page =
            serde_json::from_slice(&response.body).map_err(|_| HistoryError::InvalidResponse)?;
        if !page.ok {
            return Err(match page.error.as_deref() {
                Some("ratelimited") => HistoryError::Throttled,
                Some("missing_scope" | "not_authed" | "not_in_channel" | "access_denied") => {
                    HistoryError::Permission
                }
                Some("channel_not_found" | "thread_not_found") => HistoryError::Unavailable,
                _ => HistoryError::FailedPage,
            });
        }
        let page_messages = page.messages.ok_or(HistoryError::InvalidResponse)?;
        let length = page_messages.len();
        if length > count as usize - messages.len() {
            return Err(HistoryError::InvalidResponse);
        }
        for message in page_messages {
            if timestamp(&message.ts).is_none() {
                return Err(HistoryError::InvalidResponse);
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
        truncated = true;
        if messages.len() == count as usize || page_number == 1 {
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
    fn native_pages_are_bounded_sorted_and_partial_only_with_evidence() {
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
                "https://slack.com/api/conversations.replies?limit=3",
                "https://slack.com/api/conversations.replies?limit=2&cursor=abc%3D12",
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
        assert!(result.truncated);
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
    fn native_missing_messages_cannot_masquerade_as_empty_history() {
        let result = read_slack(1, &SLACK, &mut |_| response(200, json!({"ok":true})));
        assert!(matches!(result, Err(HistoryError::InvalidResponse)));
    }

    #[test]
    fn native_decoded_response_limit_refuses_oversized_page() {
        let result = read_slack(1, &SLACK, &mut |_| {
            Ok(Response {
                status: 200,
                headers: vec![],
                body: vec![b'x'; MAX_BODY + 1],
            })
        });
        assert!(matches!(result, Err(HistoryError::InvalidResponse)));
    }
}
