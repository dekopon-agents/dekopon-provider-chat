use clap::{Parser, Subcommand};
use dekopon_provider_sdk::provider::endpoint::Base;
use dekopon_provider_sdk::provider::{
    Capability, Code, Failure, Http, Proposal, Provider, Stdout, Usage,
};
use dekopon_provider_sdk::provider::{HttpError, HttpErrorCode, Request, Response};
use dekopon_provider_sdk::{EffectKind, RiskLevel};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::fmt;
use std::io::Write;

const SLACK: Base = Base::from_static("https://slack.com");
const DISCORD: Base = Base::from_static("https://discord.com");
const MAX_COUNT: u8 = 100;
const MAX_TEXT: usize = 4096;
const FOOTER_CHEVRON: &str = "\u{203a} ";
const FOOTER_MARKER: &str = "dekopon-footer:";

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
    const DESCRIPTION: &'static str = "Read the newest reachable Slack thread messages; beyond two pages of min(200, max(20, 4*N)) the tail is unreachable";
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
    messages: Option<Vec<Box<RawValue>>>,
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
struct Recognition {
    #[serde(default)]
    upload: Option<Box<RawValue>>,
    #[serde(default)]
    blocks: Option<Box<RawValue>>,
}

#[derive(Deserialize)]
struct AuthTest {
    ok: bool,
    #[serde(default)]
    user_id: String,
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
        let limit = (count as usize + 1 - messages.len()).min(51);
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
    let mut candidates = Vec::new();
    let mut marked = false;
    let page_limit = (usize::from(count) * 4).clamp(20, 200);
    for page_number in 0..2 {
        let mut uri = base
            .join(&format!("/api/conversations.replies?limit={page_limit}"))
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
        let page_messages = page
            .messages
            .map(|messages| {
                messages
                    .into_iter()
                    .map(|raw| {
                        let message: SlackMessage = serde_json::from_str(raw.get())
                            .map_err(|_| HistoryError::InvalidResponse)?;
                        Ok((message, serde_json::from_str::<Recognition>(raw.get()).ok()))
                    })
                    .collect::<Result<Vec<_>, HistoryError>>()
            })
            .transpose()?;
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
        let page_messages = page_messages.ok_or(HistoryError::InvalidResponse)?;
        let length = page_messages.len();
        for (message, recognition) in page_messages {
            if timestamp(&message.ts).is_none() {
                return Err(HistoryError::InvalidResponse);
            }
            let upload = recognition
                .as_ref()
                .and_then(|recognition| recognition.upload.as_deref())
                .is_some_and(|upload| {
                    serde_json::from_str::<bool>(upload.get()).is_ok_and(|upload| upload)
                });
            let footer = recognition
                .as_ref()
                .and_then(|recognition| recognition.blocks.as_deref())
                .and_then(marked_footer);
            marked |= footer.is_some();
            if messages.iter().any(|seen: &Message| seen.id == message.ts) {
                continue;
            }
            let candidate = footer
                .filter(|_| upload)
                .and_then(|footer| footer_free_text(&message, &footer))
                .map(|answer| (message.user.clone(), bounded(answer, MAX_TEXT)));
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
            candidates.push(candidate);
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
    let mut messages: Vec<_> = messages.into_iter().zip(candidates).collect();
    messages.sort_by_key(|(message, _)| timestamp(&message.time));
    if messages.len() > count as usize {
        messages.drain(..messages.len() - count as usize);
        truncated = true;
    }
    if marked && let Some(bot) = slack_bot_user(base, send) {
        for (message, candidate) in &mut messages {
            if let Some((user, answer)) = candidate.take()
                && user == bot
            {
                message.text = answer;
            }
        }
    }
    Ok(History {
        messages: messages.into_iter().map(|(message, _)| message).collect(),
        truncated,
    })
}

fn slack_bot_user(
    base: &Base,
    send: &mut dyn FnMut(Request) -> Result<Response, HttpError>,
) -> Option<String> {
    let uri = base.join("/api/auth.test").ok()?;
    let response = send(Request::new("GET", uri).ok()?).ok()?;
    if !(200..=299).contains(&response.status) {
        return None;
    }
    let identity: AuthTest = serde_json::from_slice(&response.body).ok()?;
    (identity.ok && !identity.user_id.is_empty()).then_some(identity.user_id)
}

fn footer_free_text<'a>(message: &'a SlackMessage, footer: &str) -> Option<&'a str> {
    if message.user.is_empty() || message.files.is_empty() {
        return None;
    }
    if message.text == footer {
        return Some("");
    }
    message
        .text
        .strip_suffix(footer)
        .and_then(|answer| answer.strip_suffix(' '))
}

fn marked_footer(blocks: &RawValue) -> Option<String> {
    let blocks: Vec<Box<RawValue>> = serde_json::from_str(blocks.get()).ok()?;
    let block: serde_json::Value = serde_json::from_str(blocks.last()?.get()).ok()?;
    let id = block["block_id"].as_str()?.strip_prefix(FOOTER_MARKER)?;
    let [element] = block["elements"].as_array()?.as_slice() else {
        return None;
    };
    let text = element["text"].as_str()?;
    (block["type"] == "context"
        && element["type"] == "plain_text"
        && is_uuid_v4(id)
        && text
            .strip_prefix(FOOTER_CHEVRON)
            .is_some_and(is_footer_line))
    .then(|| text.to_owned())
}

fn is_uuid_v4(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            14 => byte == b'4',
            19 => matches!(byte.to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b'),
            _ => byte.is_ascii_hexdigit(),
        })
}

fn is_footer_line(line: &str) -> bool {
    fn canonical<N: std::str::FromStr + ToString>(value: &str) -> Option<N> {
        let number = value.parse::<N>().ok()?;
        (number.to_string() == value).then_some(number)
    }
    let mut fields = line.rsplitn(3, " \u{b7} ");
    let (Some(elapsed), Some(steps), Some(models)) = (fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    let steps = match (steps.strip_suffix(" steps"), steps.strip_suffix(" step")) {
        (Some(count), _) => canonical::<u32>(count).is_some_and(|count| count != 1),
        (None, Some(count)) => canonical::<u32>(count) == Some(1),
        (None, None) => false,
    };
    !models.is_empty()
        && steps
        && elapsed
            .strip_suffix(" s")
            .and_then(canonical::<u64>)
            .is_some()
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
    fn native_slack_page_size_scales_with_count_on_both_requests() {
        for (count, limit) in [(5, 20), (10, 40), (50, 200)] {
            let mut calls = Vec::new();
            let result = read_slack(count, &SLACK, &mut |request| {
                calls.push(request.uri);
                if calls.len() == 1 {
                    response(200, json!({"ok":true,"has_more":true,"response_metadata":{"next_cursor":"next"},"messages":[
                        {"ts":"1760000000.000001","text":"root"}
                    ]}))
                } else {
                    response(200, json!({"ok":true,"messages":[
                        {"ts":"1760000000.000001","text":"root"},
                        {"ts":"1760000000.000002","text":"next"}
                    ]}))
                }
            }).expect("scaled request");
            assert_eq!(result.messages.len(), 2);
            assert_eq!(
                calls,
                [
                    format!("https://slack.com/api/conversations.replies?limit={limit}"),
                    format!(
                        "https://slack.com/api/conversations.replies?limit={limit}&cursor=next"
                    ),
                ]
            );
        }
    }

    #[test]
    fn native_slack_smaller_count_recovers_after_host_byte_limit() {
        let mut limits = Vec::new();
        let exceeded = read_slack(50, &SLACK, &mut |request| {
            limits.push(request.uri);
            Err(HttpError {
                code: HttpErrorCode::ResponseTooLarge,
                message: "decoded body exceeds grant".into(),
            })
        });
        assert!(matches!(exceeded, Err(HistoryError::TooLarge)));
        let recovered = read_slack(5, &SLACK, &mut |request| {
            limits.push(request.uri);
            if request_limit(&limits[1]) <= 20 {
                response(
                    200,
                    json!({"ok":true,"messages":[{"ts":"1760000000.000001","text":"reachable"}]}),
                )
            } else {
                Err(HttpError {
                    code: HttpErrorCode::ResponseTooLarge,
                    message: "decoded body exceeds grant".into(),
                })
            }
        })
        .expect("smaller request fits the fixture ceiling");
        assert_eq!(recovered.messages[0].text, "reachable");
        assert_eq!(
            limits,
            [
                "https://slack.com/api/conversations.replies?limit=200",
                "https://slack.com/api/conversations.replies?limit=20",
            ]
        );
    }

    fn request_limit(uri: &str) -> u32 {
        uri.split("limit=")
            .nth(1)
            .expect("limit query")
            .parse()
            .expect("numeric limit")
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
                "https://slack.com/api/conversations.replies?limit=20",
                "https://slack.com/api/conversations.replies?limit=20&cursor=next",
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
    fn native_slack_repeated_root_does_not_reject_a_full_second_page() {
        let mut calls = 0;
        let result = read_slack(1, &SLACK, &mut |_| {
            calls += 1;
            if calls == 1 {
                response(200, json!({"ok":true,"has_more":true,"response_metadata":{"next_cursor":"next"},"messages":[
                    {"ts":"1760000000.000001","text":"root"}
                ]}))
            } else {
                let mut page = vec![json!({"ts":"1760000000.000001","text":"root"})];
                page.extend((2..=21).map(|i| json!({"ts":format!("1760000000.{i:06}"),"text":format!("message-{i}")})));
                response(200, json!({"ok":true,"messages":page}))
            }
        }).expect("full page with repeated root");
        assert_eq!(calls, 2);
        assert_eq!(result.messages[0].text, "message-21");
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
                "https://slack.com/api/conversations.replies?limit=20",
                "https://slack.com/api/conversations.replies?limit=20&cursor=abc%3D12",
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
    fn native_slack_malformed_messages_on_an_error_page_stay_invalid_response() {
        for page in [
            json!({"ok":false,"error":"ratelimited","messages":[{"ts":5}]}),
            json!({"ok":false,"error":"invalid_auth","messages":[42]}),
            json!({"ok":false,"messages":[{"ts":"1760000000.000001","text":5}]}),
        ] {
            assert!(
                matches!(
                    read_slack(1, &SLACK, &mut |_| response(200, page.clone())),
                    Err(HistoryError::InvalidResponse)
                ),
                "{page}"
            );
        }
    }

    #[test]
    fn native_missing_messages_cannot_masquerade_as_empty_history() {
        let result = read_slack(1, &SLACK, &mut |_| response(200, json!({"ok":true})));
        assert!(matches!(result, Err(HistoryError::InvalidResponse)));
    }

    #[test]
    fn native_discord_two_pages_observe_older_message_at_max_count() {
        let first: Vec<_> = (150..=200)
            .rev()
            .map(|id| {
                json!({
                    "id": id.to_string(), "author":{"id":"raw-user"},
                    "timestamp": format!("2026-01-01T00:00:{:02}Z", id - 150),
                    "content": format!("message-{id}"), "attachments": if id == 200 {
                        vec![json!({"id":"F1","filename":"photo.png","content_type":"image/png"})]
                    } else { vec![] }
                })
            })
            .collect();
        let second: Vec<_> = (100..=149)
            .rev()
            .map(|id| {
                json!({
                    "id": id.to_string(), "author":{"id":"raw-user"},
                    "timestamp": format!("2025-12-31T00:00:{:02}Z", id - 100),
                    "content": format!("message-{id}")
                })
            })
            .collect();
        let mut calls = Vec::new();
        let result = read_discord(100, &DISCORD, &mut |request| {
            calls.push(request.uri);
            if calls.len() == 1 {
                response(200, json!(first))
            } else {
                response(200, json!(second))
            }
        })
        .expect("bounded two-page read");
        assert_eq!(
            calls,
            [
                "https://discord.com/api/v10/channels/{conversation.apiChannel}/messages?limit=51",
                "https://discord.com/api/v10/channels/{conversation.apiChannel}/messages?limit=50&before=150",
            ]
        );
        assert_eq!(result.messages.len(), 100);
        assert_eq!(result.messages[0].text, "message-101");
        assert_eq!(result.messages[99].author, "raw-user");
        let output = serde_json::to_value(&result).expect("output");
        assert_eq!(output["messages"][99]["assets"][0]["available"], false);
        assert!(output["messages"][99]["assets"][0]["size"].is_null());
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
        let page: Vec<_> = (150..=200)
            .rev()
            .map(|id| {
                json!({
                    "id":id.to_string(), "author":{"id":"U"}, "timestamp":"2026-01-01T00:00:00Z"
                })
            })
            .collect();
        let mut calls = 0;
        let failed = read_discord(52, &DISCORD, &mut |_| {
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

    const BOT: &str = "U_SYNTHETIC_BOT";
    const FOOTER: &str = "\u{203a} model-a \u{b7} 2 steps \u{b7} 3 s";
    const MARKER: &str = "dekopon-footer:00000000-0000-4000-8000-000000000001";
    const REPLIES: &str = "https://slack.com/api/conversations.replies?limit=200";
    const AUTH_TEST: &str = "https://slack.com/api/auth.test";
    const REPLIES_20: &str = "https://slack.com/api/conversations.replies?limit=20";

    fn footer_block(id: &str, elements: serde_json::Value) -> serde_json::Value {
        json!({"type":"context","block_id":id,"elements":elements})
    }

    fn marked() -> serde_json::Value {
        footer_block(MARKER, json!([{"type":"plain_text","text":FOOTER}]))
    }

    fn reply(user: &str, upload: bool, text: &str, last: serde_json::Value) -> serde_json::Value {
        let mut message = json!({
            "type":"message","user":user,"bot_id":"B_SYNTHETIC_BOT","text":text,
            "blocks":[{"type":"rich_text","elements":[{"type":"rich_text_section",
                "elements":[{"type":"text","text":"Synthetic answer."}]}]}, last],
        });
        if upload {
            message["upload"] = json!(true);
            message["files"] = json!([{"id":"F_SYNTHETIC_IMAGE","name":"cat.png","mimetype":"image/png","size":42}]);
        }
        message
    }

    fn identity() -> serde_json::Value {
        json!({"ok":true,"user_id":BOT,"bot_id":"B_SYNTHETIC_BOT","url":"https://private.example/",
            "team":"Private Team","user":"private-bot-name","team_id":"T_SYNTHETIC"})
    }

    fn read_page(
        page: Vec<serde_json::Value>,
        auth: Result<Response, HttpError>,
    ) -> (Result<History, HistoryError>, Vec<String>) {
        let page: Vec<_> = page
            .into_iter()
            .enumerate()
            .map(|(index, mut message)| {
                message["ts"] = json!(format!("17600000{index:02}.000100"));
                message
            })
            .collect();
        let mut calls = Vec::new();
        let result = read_slack(100, &SLACK, &mut |request| {
            calls.push(request.uri.clone());
            if request.uri == AUTH_TEST {
                auth.clone()
            } else {
                response(200, json!({"ok":true,"messages":page}))
            }
        });
        (result, calls)
    }

    #[test]
    fn native_slack_drops_only_the_bots_marked_footer_from_uploaded_replies() {
        let answered = format!("Synthetic answer. {FOOTER}");
        let twice = format!("Synthetic answer. {FOOTER} {FOOTER}");
        let unspaced = format!("Synthetic answer.{FOOTER}");
        let inner = format!("Quoted {FOOTER} inside.");
        let quoted_then_footer = format!("Quoted {FOOTER} inside. {FOOTER}");
        let mut not_final = reply(BOT, true, &answered, marked());
        not_final["blocks"]
            .as_array_mut()
            .expect("blocks")
            .push(json!({"type":"divider"}));
        let mut not_uploaded = reply(BOT, true, &answered, marked());
        not_uploaded["upload"] = json!(false);
        let mut upload_absent = reply(BOT, true, &answered, marked());
        upload_absent
            .as_object_mut()
            .expect("message")
            .remove("upload");
        let element = |elements| footer_block(MARKER, elements);
        let mut cases = vec![
            (reply(BOT, true, &answered, marked()), "Synthetic answer."),
            (reply(BOT, true, FOOTER, marked()), ""),
            (reply(BOT, true, &twice, marked()), answered.as_str()),
            (
                reply(BOT, true, &quoted_then_footer, marked()),
                inner.as_str(),
            ),
            (reply(BOT, false, &answered, marked()), answered.as_str()),
            (
                reply(BOT, false, "Synthetic answer.", marked()),
                "Synthetic answer.",
            ),
            (
                reply("U_SYNTHETIC_HUMAN", true, &answered, marked()),
                answered.as_str(),
            ),
            (
                reply(
                    BOT,
                    true,
                    &answered,
                    json!({"type":"context","elements":[{"type":"plain_text","text":FOOTER}]}),
                ),
                answered.as_str(),
            ),
            (
                reply(
                    BOT,
                    true,
                    &answered,
                    footer_block(
                        "dekopon-footer:00000000-0000-1000-8000-000000000001",
                        json!([{"type":"plain_text","text":FOOTER}]),
                    ),
                ),
                answered.as_str(),
            ),
            (not_final, answered.as_str()),
            (
                reply(
                    BOT,
                    true,
                    &answered,
                    element(
                        json!([{"type":"plain_text","text":FOOTER},{"type":"plain_text","text":"x"}]),
                    ),
                ),
                answered.as_str(),
            ),
            (
                reply(
                    BOT,
                    true,
                    &answered,
                    element(json!([{"type":"mrkdwn","text":FOOTER}])),
                ),
                answered.as_str(),
            ),
            (reply(BOT, true, &unspaced, marked()), unspaced.as_str()),
            (reply(BOT, true, &inner, marked()), inner.as_str()),
            (
                reply(
                    BOT,
                    true,
                    "Synthetic answer. Posted by the release bot",
                    footer_block(
                        "release-note",
                        json!([{"type":"plain_text","text":"Posted by the release bot"}]),
                    ),
                ),
                "Synthetic answer. Posted by the release bot",
            ),
        ];
        cases.push((not_uploaded, answered.as_str()));
        cases.push((upload_absent, answered.as_str()));
        let malformed: Vec<_> = [
            "\u{203a} model-a \u{b7} 1 steps \u{b7} 3 s",
            "\u{203a} model-a \u{b7} 2 step \u{b7} 3 s",
            "\u{203a} model-a \u{b7} 02 steps \u{b7} 3 s",
            "> model-a \u{b7} 2 steps \u{b7} 3 s",
            "\u{203a} model-a \u{b7} 2 steps \u{b7} 18446744073709551616 s",
        ]
        .into_iter()
        .map(|line| {
            let text = format!("Synthetic answer. {line}");
            (
                reply(
                    BOT,
                    true,
                    &text,
                    element(json!([{"type":"plain_text","text":line}])),
                ),
                text,
            )
        })
        .collect();
        cases.extend(
            malformed
                .iter()
                .map(|(message, text)| (message.clone(), text.as_str())),
        );
        let (result, calls) = read_page(
            cases.iter().map(|(message, _)| message.clone()).collect(),
            response(200, identity()),
        );
        let result = result.expect("marked history");
        assert_eq!(calls, [REPLIES, AUTH_TEST]);
        assert_eq!(
            result
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>(),
            cases
                .iter()
                .map(|(_, expected)| *expected)
                .collect::<Vec<_>>()
        );
        let output = serde_json::to_value(&result).expect("output");
        assert_eq!(
            output["messages"][0],
            json!({"id":"1760000000.000100","author":BOT,"time":"1760000000.000100",
                "text":"Synthetic answer.","assets":[{"id":"F_SYNTHETIC_IMAGE","name":"cat.png",
                "mime":"image/png","size":42,"available":false}]})
        );
        assert_eq!(
            output["messages"][1]["assets"],
            output["messages"][0]["assets"]
        );
        let serialized = output.to_string();
        for private in [
            "private.example",
            "Private Team",
            "private-bot-name",
            "T_SYNTHETIC",
            "B_SYNTHETIC_BOT",
        ] {
            assert!(!serialized.contains(private), "{private}");
        }
    }

    #[test]
    fn native_slack_strips_before_bounding_so_truncation_cannot_hide_the_footer() {
        let answer = "a".repeat(MAX_TEXT - 1);
        let (result, _) = read_page(
            vec![reply(BOT, true, &format!("{answer} {FOOTER}"), marked())],
            response(200, identity()),
        );
        assert_eq!(result.expect("long answer").messages[0].text, answer);
    }

    #[test]
    fn native_slack_repeated_recognition_keys_keep_the_message_and_its_files() {
        let marked_upload = reply(BOT, true, &format!("Synthetic answer. {FOOTER}"), marked())
            .to_string()
            .replacen('{', r#"{"ts":"1760000000.000003","#, 1);
        for (repeated, calls) in [
            (r#""blocks":[],"blocks":[]"#, vec![REPLIES_20]),
            (r#""upload":true,"upload":true"#, vec![REPLIES_20]),
            (r#""blocks":[],"blocks":[]"#, vec![REPLIES_20, AUTH_TEST]),
        ] {
            let mut page = format!(
                r#"{{"ts":"1760000000.000001","user":"{BOT}","text":"keep me","files":[{{"id":"F1","name":"cat.png","mimetype":"image/png"}}],{repeated}}}"#
            );
            if calls.len() == 2 {
                page = format!("{page},{marked_upload}");
            }
            let body = format!(r#"{{"ok":true,"messages":[{page}]}}"#);
            let mut sent = Vec::new();
            let result = read_slack(5, &SLACK, &mut |request| {
                sent.push(request.uri.clone());
                if request.uri == AUTH_TEST {
                    return response(200, identity());
                }
                Ok(Response {
                    status: 200,
                    headers: vec![],
                    body: body.clone().into_bytes(),
                })
            })
            .expect("repeated metadata keys are not a malformed page");
            assert_eq!(sent, calls, "{repeated}");
            assert_eq!(result.messages[0].text, "keep me");
            assert_eq!(result.messages[0].assets[0].id, "F1");
            if let Some(stripped) = result.messages.get(1) {
                assert_eq!(stripped.text, "Synthetic answer.");
            }
        }
    }

    #[test]
    fn native_slack_sends_no_auth_test_without_a_marked_footer() {
        let answered = format!("Synthetic answer. {FOOTER}");
        let (result, calls) = read_page(
            vec![
                reply(BOT, true, &answered, json!({"type":"divider"})),
                reply(
                    BOT,
                    true,
                    &answered,
                    footer_block(
                        "dekopon-footer:00000000-0000-1000-8000-000000000001",
                        json!([{"type":"plain_text","text":FOOTER}]),
                    ),
                ),
                json!({"user":"U_SYNTHETIC_HUMAN","text":answered}),
            ],
            response(200, identity()),
        );
        assert_eq!(calls, [REPLIES]);
        assert!(
            result
                .expect("unmarked history")
                .messages
                .iter()
                .all(|m| m.text.ends_with(FOOTER))
        );
    }

    #[test]
    fn native_slack_marked_ordinary_reply_sends_one_auth_test_and_keeps_its_text() {
        let answered = format!("Synthetic answer. {FOOTER}");
        let (result, calls) = read_page(
            vec![reply(BOT, false, &answered, marked())],
            response(200, identity()),
        );
        assert_eq!(calls, [REPLIES, AUTH_TEST]);
        assert_eq!(result.expect("ordinary reply").messages[0].text, answered);
    }

    #[test]
    fn native_slack_marked_footer_outside_the_window_still_sends_one_auth_test() {
        let mut calls = Vec::new();
        let result = read_slack(1, &SLACK, &mut |request| {
            calls.push(request.uri.clone());
            if request.uri == AUTH_TEST {
                return response(200, identity());
            }
            let mut older = reply(BOT, true, &format!("Synthetic answer. {FOOTER}"), marked());
            older["ts"] = json!("1760000000.000001");
            response(
                200,
                json!({"ok":true,"messages":[older,{"ts":"1760000000.000002","user":"U_SYNTHETIC_HUMAN","text":"newer"}]}),
            )
        })
        .expect("windowed history");
        assert_eq!(
            calls,
            [
                "https://slack.com/api/conversations.replies?limit=20",
                AUTH_TEST
            ]
        );
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].text, "newer");
        assert!(result.truncated);
    }

    #[test]
    fn native_slack_deeply_nested_metadata_keeps_history_and_sends_no_identity_request() {
        let deep = format!("{}{}", "[".repeat(300), "]".repeat(300));
        let body = format!(
            r#"{{"ok":true,"messages":[{{"ts":"1760000000.000001","user":"{BOT}","upload":{deep},"files":[{{"id":"F1"}}],"text":"Synthetic answer. {FOOTER}","blocks":{deep}}},{{"ts":"1760000000.000002","user":"{BOT}","upload":true,"files":[{{"id":"F2"}}],"text":"nested","blocks":[{deep},{{"type":"context","elements":{deep}}}]}}]}}"#
        );
        let mut calls = Vec::new();
        let result = read_slack(5, &SLACK, &mut |request| {
            calls.push(request.uri.clone());
            Ok(Response {
                status: 200,
                headers: vec![],
                body: body.clone().into_bytes(),
            })
        })
        .expect("nested metadata is not a malformed page");
        assert_eq!(
            calls,
            ["https://slack.com/api/conversations.replies?limit=20"]
        );
        assert_eq!(
            result
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>(),
            [format!("Synthetic answer. {FOOTER}").as_str(), "nested"]
        );
    }

    #[test]
    fn native_slack_marked_upload_on_second_page_sends_one_auth_test_after_both_pages() {
        let mut calls = Vec::new();
        let result = read_slack(2, &SLACK, &mut |request| {
            calls.push(request.uri.clone());
            if request.uri == AUTH_TEST {
                return response(200, identity());
            }
            if calls.len() == 1 {
                response(200, json!({"ok":true,"has_more":true,"response_metadata":{"next_cursor":"next"},"messages":[
                    {"ts":"1760000000.000001","user":"U_SYNTHETIC_HUMAN","text":"root"}
                ]}))
            } else {
                let mut marked = reply(BOT, true, &format!("Synthetic answer. {FOOTER}"), marked());
                marked["ts"] = json!("1760000000.000002");
                response(200, json!({"ok":true,"has_more":true,"messages":[marked]}))
            }
        })
        .expect("two pages");
        assert_eq!(
            calls,
            [
                "https://slack.com/api/conversations.replies?limit=20",
                "https://slack.com/api/conversations.replies?limit=20&cursor=next",
                AUTH_TEST,
            ]
        );
        assert_eq!(result.messages[1].text, "Synthetic answer.");
    }

    #[test]
    fn native_slack_unknown_bot_identity_leaves_every_message_unchanged() {
        let answered = format!("Synthetic answer. {FOOTER}");
        for auth in [
            response(500, json!({"ok":false,"error":"private server detail"})),
            response(403, json!({})),
            response(200, json!({"ok":false,"error":"invalid_auth"})),
            response(200, json!({"ok":true})),
            response(200, json!({"ok":true,"user_id":""})),
            response(200, json!({"ok":true,"user_id":42})),
            response(200, json!(["not an object"])),
            Err(HttpError {
                code: HttpErrorCode::Denied,
                message: "no request template matched".into(),
            }),
            Err(HttpError {
                code: HttpErrorCode::ResponseTooLarge,
                message: "decoded body exceeds grant".into(),
            }),
        ] {
            let (result, calls) = read_page(vec![reply(BOT, true, &answered, marked())], auth);
            assert_eq!(calls, [REPLIES, AUTH_TEST]);
            let result = result.expect("history survives an unknown identity");
            assert_eq!(result.messages[0].text, answered);
            assert_eq!(result.messages[0].assets.len(), 1);
        }
    }
}
