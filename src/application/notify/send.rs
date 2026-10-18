//! Telegram Bot API transport. The token never leaves this file and is never
//! logged; errors are reduced to a safe description.

use std::{
    collections::VecDeque,
    sync::OnceLock,
    time::{Duration, Instant},
};

use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::config::NotifyConfig;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RETRY_AFTER_SECONDS: i64 = 30;

pub(crate) struct TelegramSender {
    token: String,
    chat_id: String,
    api_base: String,
    thread_id: Option<i64>,
}

impl TelegramSender {
    pub(crate) fn from_config(config: &NotifyConfig) -> Option<Self> {
        Some(Self {
            token: config.bot_token.clone()?,
            chat_id: config.chat_id.clone()?,
            api_base: config.api_base.trim_end_matches('/').to_owned(),
            thread_id: config.thread_id,
        })
    }

    /// Best-effort send: retries once on 429 (using Telegram's `retry_after`)
    /// and once on 5xx. Returns the Telegram message id when available.
    pub(crate) async fn send(&self, text: &str, silent: bool) -> Result<Option<i64>, String> {
        let payload = build_payload(&self.chat_id, text, silent, self.thread_id);
        let url = format!("{}/bot{}/sendMessage", self.api_base, self.token);
        for attempt in 0..2 {
            let response = match client().post(&url).json(&payload).send().await {
                Ok(response) => response,
                Err(error) => return Err(describe_reqwest_error(&error)),
            };
            let status = response.status();
            let body: Value = response.json().await.unwrap_or(Value::Null);
            if status.is_success() && body.get("ok").and_then(Value::as_bool) == Some(true) {
                return Ok(body.pointer("/result/message_id").and_then(Value::as_i64));
            }
            let description = body
                .get("description")
                .and_then(Value::as_str)
                .map(|value| value.trim().chars().take(200).collect::<String>())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("Telegram API 返回 HTTP {}", status.as_u16()));
            if attempt == 0 {
                if status == StatusCode::TOO_MANY_REQUESTS {
                    let retry_after = retry_after_seconds(&body);
                    tokio::time::sleep(Duration::from_secs(retry_after)).await;
                    continue;
                }
                if status.is_server_error() {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            }
            return Err(description);
        }
        Err("Telegram API 多次请求失败".to_owned())
    }
}

pub(crate) fn build_payload(
    chat_id: &str,
    text: &str,
    silent: bool,
    thread_id: Option<i64>,
) -> Value {
    let mut payload = json!({
        "chat_id": chat_id,
        "text": text,
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
        "disable_notification": silent,
    });
    if let Some(thread_id) = thread_id
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("message_thread_id".to_owned(), json!(thread_id));
    }
    payload
}

fn retry_after_seconds(body: &Value) -> u64 {
    body.pointer("/parameters/retry_after")
        .and_then(Value::as_i64)
        .unwrap_or(1)
        .clamp(1, MAX_RETRY_AFTER_SECONDS) as u64
}

fn describe_reqwest_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "请求 Telegram API 超时".to_owned()
    } else if error.is_connect() {
        "无法连接 Telegram API".to_owned()
    } else if error.is_request() {
        "Telegram 请求构造失败".to_owned()
    } else {
        "Telegram 网络错误".to_owned()
    }
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // The bot token is part of the request path; never follow a
            // redirect that could forward it to another host.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("luxd-notify/0.1")
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

/// Serialises sends so a burst of archive events cannot trip Telegram limits.
pub(crate) async fn wait_for_turn(min_interval: Duration, max_per_minute: u32) {
    static RATE: OnceLock<Mutex<RateState>> = OnceLock::new();
    let rate = RATE.get_or_init(|| Mutex::new(RateState::default()));
    let mut state = rate.lock().await;
    state.acquire(min_interval, max_per_minute).await;
}

#[derive(Default)]
struct RateState {
    last_sent: Option<Instant>,
    recent: VecDeque<Instant>,
}

impl RateState {
    async fn acquire(&mut self, min_interval: Duration, max_per_minute: u32) {
        let now = Instant::now();
        while let Some(front) = self.recent.front() {
            if now.duration_since(*front) >= Duration::from_secs(60) {
                self.recent.pop_front();
            } else {
                break;
            }
        }
        let mut wait = Duration::ZERO;
        if let Some(last) = self.last_sent {
            let elapsed = now.duration_since(last);
            if elapsed < min_interval {
                wait = min_interval - elapsed;
            }
        }
        if max_per_minute > 0 && self.recent.len() >= max_per_minute as usize {
            let oldest = *self.recent.front().expect("recent is not empty");
            let window_wait = Duration::from_secs(60).saturating_sub(now.duration_since(oldest));
            wait = wait.max(window_wait);
        }
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let now = Instant::now();
        self.last_sent = Some(now);
        self.recent.push_back(now);
        if self.recent.len() > 1_000 {
            self.recent.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_uses_html_and_disables_previews() {
        let payload = build_payload("-100123", "<b>hi</b>", true, Some(42));
        assert_eq!(payload["chat_id"], "-100123");
        assert_eq!(payload["parse_mode"], "HTML");
        assert_eq!(payload["disable_web_page_preview"], true);
        assert_eq!(payload["disable_notification"], true);
        assert_eq!(payload["message_thread_id"], 42);
    }

    #[test]
    fn payload_omits_thread_id_when_not_configured() {
        let payload = build_payload("1", "hi", false, None);
        assert!(payload.get("message_thread_id").is_none());
        assert_eq!(payload["disable_notification"], false);
    }

    #[test]
    fn retry_after_is_clamped() {
        assert_eq!(retry_after_seconds(&json!({})), 1);
        assert_eq!(
            retry_after_seconds(&json!({"parameters": {"retry_after": 7}})),
            7
        );
        assert_eq!(
            retry_after_seconds(&json!({"parameters": {"retry_after": 600}})),
            MAX_RETRY_AFTER_SECONDS as u64
        );
        assert_eq!(
            retry_after_seconds(&json!({"parameters": {"retry_after": 0}})),
            1
        );
    }

    #[tokio::test]
    async fn rate_limiter_enforces_the_minimum_interval() {
        let mut state = RateState::default();
        state.acquire(Duration::from_millis(5), 20).await;
        let started = Instant::now();
        state.acquire(Duration::from_millis(5), 20).await;
        assert!(started.elapsed() >= Duration::from_millis(4));
    }
}
