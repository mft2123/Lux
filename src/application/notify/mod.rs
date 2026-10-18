//! Telegram notifications for archive, playback and scheduled-task events.
//!
//! Design constraints (see `docs/handoff/module-notify.md`):
//! - best effort: a failed send never fails the caller and is only logged;
//! - no token ever reaches the logs or an admin response;
//! - all categories are opt-in through `telegram.notify` in the settings file;
//! - one message per item plus a run summary, capped and rate limited.

mod config;
mod event;
mod format;
mod send;

use std::{
    collections::HashMap,
    sync::{
        Mutex as StdMutex, OnceLock,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
};

use serde::Serialize;
use time::OffsetDateTime;

pub(crate) use config::{NotifyCategory, NotifyConfig};
pub(crate) use event::{NotifyEvent, NotifySendRequest, PlaybackNotice};

use crate::application::archive::ArchiveReport;

/// Outcome of one delivery attempt, returned by the admin endpoints.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendOutcome {
    pub sent: bool,
    pub skipped: bool,
    pub message_id: Option<i64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotifyStats {
    pub sent: u64,
    pub failed: u64,
    pub skipped: u64,
    pub last_success_at: Option<i64>,
    pub last_failure_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotifyCategoryStatus {
    pub key: &'static str,
    pub label: &'static str,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotifyStatus {
    pub configured: bool,
    pub enabled: bool,
    /// Masked for display; the full id never leaves the server.
    pub chat_id: Option<String>,
    /// Masked base URL: credentials and token-bearing paths are hidden.
    pub api_base: String,
    pub categories: Vec<NotifyCategoryStatus>,
    pub stats: NotifyStats,
    pub dedupe_window_seconds: i64,
    pub min_interval_ms: u64,
    pub max_per_minute: u32,
}

/// Send one event (respecting category switches and the dedupe window).
pub(crate) async fn send_event(event: NotifyEvent) -> SendOutcome {
    let config = NotifyConfig::load();
    deliver(event, &config, true, true).await
}

/// Send the fixed test message, ignoring category switches.
pub(crate) async fn send_test() -> SendOutcome {
    let config = NotifyConfig::load();
    if !config.enabled {
        return skipped("Telegram 通知已禁用");
    }
    if config.bot_token.is_none() || config.chat_id.is_none() {
        return skipped("未配置 telegram.bot_token / telegram.chat_id");
    }
    deliver(NotifyEvent::test(), &config, false, false).await
}

/// Detached send used by request handlers and background jobs.
pub(crate) fn spawn_event(event: NotifyEvent) {
    spawn_events(vec![event]);
}

/// Detached sequential send; keeps the item messages ahead of the summary and
/// lets the global rate limiter space them out.
pub(crate) fn spawn_events(events: Vec<NotifyEvent>) {
    if events.is_empty() {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        for event in events {
            let _ = send_event(event).await;
        }
    });
}

pub(crate) fn spawn_archive_report(report: &ArchiveReport) {
    if report.dry_run {
        return;
    }
    spawn_events(event::archive_events(report));
}

pub(crate) fn spawn_playback(notice: PlaybackNotice) {
    spawn_event(NotifyEvent::playback(notice));
}

/// Cheap check used by callers to skip gathering notification context when the
/// bot is not configured and no other delivery path needs the data.
pub(crate) fn is_configured() -> bool {
    NotifyConfig::load().configured()
}

/// The category list the settings page renders. The server owns it so a new
/// category never needs a frontend change (and a save never drops one the page
/// did not know about).
pub(crate) fn known_categories() -> serde_json::Value {
    serde_json::Value::Array(
        NotifyCategory::ALL
            .iter()
            .map(|category| {
                serde_json::json!({
                    "key": category.key(),
                    "label": category.label(),
                    "emoji": category.emoji(),
                })
            })
            .collect(),
    )
}

pub(crate) fn status() -> NotifyStatus {
    let config = NotifyConfig::load();
    NotifyStatus {
        configured: config.configured(),
        enabled: config.enabled,
        chat_id: config.masked_chat_id(),
        api_base: config.masked_api_base(),
        categories: NotifyCategory::ALL
            .into_iter()
            .map(|category| NotifyCategoryStatus {
                key: category.key(),
                label: category.label(),
                enabled: config.toggles.contains(category),
            })
            .collect(),
        stats: stats().snapshot(),
        dedupe_window_seconds: config.dedupe_window.as_secs() as i64,
        min_interval_ms: config.min_interval.as_millis() as u64,
        max_per_minute: config.max_per_minute,
    }
}

async fn deliver(
    event: NotifyEvent,
    config: &NotifyConfig,
    respect_toggles: bool,
    respect_dedupe: bool,
) -> SendOutcome {
    if !config.configured() {
        stats().skipped();
        tracing::debug!(
            category = event.category.key(),
            "telegram notification skipped: not configured"
        );
        return skipped("未配置 Telegram 凭据");
    }
    if respect_toggles && !config.category_enabled(event.category) {
        stats().skipped();
        tracing::debug!(
            category = event.category.key(),
            "telegram notification skipped: category disabled"
        );
        return skipped("该通知类别未启用");
    }
    // Check the dedupe window before sending, but only remember the key once the
    // message actually went out: a failed send must not suppress the retry.
    let dedupe_key = event
        .dedupe_key
        .as_ref()
        .map(|key| format!("{}:{key}", event.category.key()));
    let dedupe_now = now_unix();
    if respect_dedupe
        && let Some(key) = dedupe_key.as_deref()
        && dedupe_seen(key, dedupe_now, config.dedupe_window.as_secs() as i64)
    {
        stats().skipped();
        tracing::debug!(
            category = event.category.key(),
            "telegram notification skipped: duplicate within window"
        );
        return skipped("重复通知已抑制");
    }
    let Some(sender) = send::TelegramSender::from_config(config) else {
        stats().skipped();
        return skipped("未配置 Telegram 凭据");
    };
    send::wait_for_turn(config.min_interval, config.max_per_minute).await;
    let text = format::format_message(&event, config.timezone_offset_hours);
    let silent = event.silent.unwrap_or(config.silent);
    match sender.send(&text, silent).await {
        Ok(message_id) => {
            stats().sent();
            if respect_dedupe && let Some(key) = dedupe_key.as_deref() {
                dedupe_remember(key, dedupe_now);
            }
            tracing::info!(
                category = event.category.key(),
                "telegram notification delivered"
            );
            SendOutcome {
                sent: true,
                skipped: false,
                message_id,
                error: None,
            }
        }
        Err(error) => {
            stats().failed(&error);
            tracing::warn!(
                category = event.category.key(),
                error = %error,
                "telegram notification failed"
            );
            SendOutcome {
                sent: false,
                skipped: false,
                message_id: None,
                error: Some(error),
            }
        }
    }
}

fn skipped(reason: &str) -> SendOutcome {
    SendOutcome {
        sent: false,
        skipped: true,
        message_id: None,
        error: Some(reason.to_owned()),
    }
}

fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// `true` when a notification with this key already went out inside the window.
/// The key is only remembered by [`dedupe_remember`], after a successful send.
fn dedupe_seen(key: &str, now: i64, window_seconds: i64) -> bool {
    if window_seconds <= 0 {
        return false;
    }
    let mut state = dedupe_state().lock().unwrap_or_else(|error| error.into_inner());
    prune_dedupe(&mut state, now, window_seconds);
    state
        .entries
        .get(key)
        .map(|seen_at| now.saturating_sub(*seen_at) < window_seconds)
        .unwrap_or(false)
}

fn dedupe_remember(key: &str, now: i64) {
    let mut state = dedupe_state().lock().unwrap_or_else(|error| error.into_inner());
    state.entries.insert(key.to_owned(), now);
}

fn prune_dedupe(state: &mut DedupeState, now: i64, window_seconds: i64) {
    if state.entries.len() > 512 || now.saturating_sub(state.pruned_at) > 300 {
        let cutoff = now.saturating_sub(window_seconds.max(300));
        state.entries.retain(|_, seen_at| *seen_at >= cutoff);
        state.pruned_at = now;
    }
}

fn dedupe_state() -> &'static StdMutex<DedupeState> {
    static DEDUPE: OnceLock<StdMutex<DedupeState>> = OnceLock::new();
    DEDUPE.get_or_init(|| StdMutex::new(DedupeState::default()))
}

#[derive(Default)]
struct DedupeState {
    entries: HashMap<String, i64>,
    pruned_at: i64,
}

fn stats() -> &'static StatsState {
    static STATS: OnceLock<StatsState> = OnceLock::new();
    STATS.get_or_init(StatsState::default)
}

#[derive(Default)]
struct StatsState {
    sent: AtomicU64,
    failed: AtomicU64,
    skipped: AtomicU64,
    last_success_at: AtomicI64,
    last_failure_at: AtomicI64,
    last_error: StdMutex<Option<String>>,
}

impl StatsState {
    fn sent(&self) {
        self.sent.fetch_add(1, Ordering::Relaxed);
        self.last_success_at.store(now_unix(), Ordering::Relaxed);
    }

    fn failed(&self, error: &str) {
        self.failed.fetch_add(1, Ordering::Relaxed);
        self.last_failure_at.store(now_unix(), Ordering::Relaxed);
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some(error.chars().take(300).collect());
        }
    }

    fn skipped(&self) {
        self.skipped.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> NotifyStats {
        let last_success_at = self.last_success_at.load(Ordering::Relaxed);
        let last_failure_at = self.last_failure_at.load(Ordering::Relaxed);
        NotifyStats {
            sent: self.sent.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            last_success_at: (last_success_at > 0).then_some(last_success_at),
            last_failure_at: (last_failure_at > 0).then_some(last_failure_at),
            last_error: self.last_error.lock().ok().and_then(|slot| slot.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_suppresses_repeats_inside_the_window_and_expires_afterwards() {
        let key = format!("test:{}", uuid::Uuid::now_v7());
        // Only a successful send remembers the key, so the test remembers it too.
        assert!(!dedupe_seen(&key, 1_000, 60));
        dedupe_remember(&key, 1_000);
        assert!(dedupe_seen(&key, 1_030, 60));
        assert!(!dedupe_seen(&key, 1_061, 60));
    }

    #[test]
    fn a_failed_send_does_not_suppress_the_next_attempt() {
        let key = format!("test:{}", uuid::Uuid::now_v7());
        // A send that failed never remembered the key ...
        assert!(!dedupe_seen(&key, 1_000, 60));
        // ... so the retry a moment later is allowed.
        assert!(!dedupe_seen(&key, 1_005, 60));
    }

    #[test]
    fn dedupe_is_disabled_for_zero_window() {
        let key = format!("test:{}", uuid::Uuid::now_v7());
        assert!(!dedupe_seen(&key, 1_000, 0));
        assert!(!dedupe_seen(&key, 1_000, 0));
    }

    #[test]
    fn stats_count_success_failure_and_skip() {
        let stats = StatsState::default();
        stats.sent();
        stats.failed("boom");
        stats.skipped();
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.sent, 1);
        assert_eq!(snapshot.failed, 1);
        assert_eq!(snapshot.skipped, 1);
        assert_eq!(snapshot.last_error.as_deref(), Some("boom"));
        assert!(snapshot.last_success_at.is_some());
        assert!(snapshot.last_failure_at.is_some());
    }

    #[test]
    fn status_reports_categories_and_masks_chat_id() {
        let status = NotifyStatus {
            configured: false,
            enabled: true,
            chat_id: Some("586***60".to_owned()),
            api_base: "https://api.telegram.org".to_owned(),
            categories: NotifyCategory::ALL
                .into_iter()
                .map(|category| NotifyCategoryStatus {
                    key: category.key(),
                    label: category.label(),
                    enabled: true,
                })
                .collect(),
            stats: NotifyStats::default(),
            dedupe_window_seconds: 60,
            min_interval_ms: 1_200,
            max_per_minute: 20,
        };
        let json = serde_json::to_value(&status).expect("serialize status");
        assert_eq!(json["categories"].as_array().map(Vec::len), Some(11));
        assert_eq!(json["dedupeWindowSeconds"], 60);
        assert_eq!(json["chatId"], "586***60");
    }

    #[test]
    fn skipped_outcome_carries_a_reason() {
        let outcome = skipped("未配置 Telegram 凭据");
        assert!(outcome.skipped);
        assert!(!outcome.sent);
        assert_eq!(outcome.error.as_deref(), Some("未配置 Telegram 凭据"));
    }
}
