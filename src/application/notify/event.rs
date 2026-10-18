//! Event model for Telegram notifications.
//!
//! Every module builds a [`NotifyEvent`] (or uses one of the constructors
//! below) and hands it to the module. Formatting and delivery are separate so
//! the text can be unit tested without a network.

use serde::Deserialize;
use time::OffsetDateTime;

use crate::application::archive::{ArchiveReport, DuplicateItem, OrganizedItem, WashedItem};

#[cfg(test)]
use crate::application::archive::FailedItem;

use super::{
    config::NotifyCategory,
    format::{format_bytes, truncate_chars},
};

/// How many single-item messages one category may emit per archive run. The
/// run summary still carries the totals when this is exceeded.
const MAX_ITEM_EVENTS_PER_RUN: usize = 10;
const MAX_REQUEST_FIELDS: usize = 30;

#[derive(Clone, Debug)]
pub(crate) struct NotifyField {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug)]
pub(crate) struct NotifyEvent {
    pub category: NotifyCategory,
    pub title: String,
    pub fields: Vec<NotifyField>,
    pub overview: Option<String>,
    pub url: Option<String>,
    pub occurred_at: i64,
    pub silent: Option<bool>,
    /// Identical keys inside the dedupe window are only sent once.
    pub dedupe_key: Option<String>,
}

impl NotifyEvent {
    pub(crate) fn new(category: NotifyCategory, title: impl Into<String>) -> Self {
        Self {
            category,
            title: title.into().trim().to_owned(),
            fields: Vec::new(),
            overview: None,
            url: None,
            occurred_at: now_unix(),
            silent: None,
            dedupe_key: None,
        }
    }

    pub(crate) fn field(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        let value = value.into();
        let value = value.trim();
        if !value.is_empty() {
            self.fields.push(NotifyField {
                label: label.into(),
                value: value.to_owned(),
            });
        }
        self
    }

    pub(crate) fn overview(mut self, overview: impl Into<String>) -> Self {
        let overview = overview.into();
        let overview = overview.trim();
        if !overview.is_empty() {
            self.overview = Some(overview.to_owned());
        }
        self
    }

    pub(crate) fn url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        if is_safe_url(&url) {
            self.url = Some(url);
        }
        self
    }

    pub(crate) fn dedupe_key(mut self, key: impl Into<String>) -> Self {
        self.dedupe_key = Some(key.into());
        self
    }

    pub(crate) fn test() -> Self {
        Self::new(NotifyCategory::Test, "Telegram 通知测试")
            .field("📡 来源", "Lux 通知模块")
            .field("✅ 结果", "收到这条消息说明 bot_token / chat_id 配置正确")
    }
}

/// Playback details gathered by `src/api/playback.rs`.
#[derive(Clone, Debug, Default)]
pub(crate) struct PlaybackNotice {
    pub item_title: Option<String>,
    pub series_title: Option<String>,
    pub season_number: Option<i64>,
    pub season_count: Option<i64>,
    pub user_name: String,
    pub client: Option<String>,
    pub device_name: Option<String>,
    pub device_type: Option<String>,
    pub remote_ip: Option<String>,
    pub play_method: Option<String>,
    pub container: Option<String>,
    pub size: Option<i64>,
    pub bitrate: Option<i64>,
    pub overview: Option<String>,
    pub resumed: bool,
    pub observed_at: i64,
    pub session_key: String,
}

impl NotifyEvent {
    pub(crate) fn playback(notice: PlaybackNotice) -> Self {
        let series = notice
            .series_title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let episode = notice
            .item_title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mut display = match (series, episode) {
            (Some(series), Some(episode)) if series != episode => {
                let mut parts = vec![series.to_owned()];
                if let Some(season) = notice.season_number {
                    if season <= 0 && notice.season_count.unwrap_or(0) > 1 {
                        parts.push("特别篇".to_owned());
                    } else {
                        parts.push(format!("第{season}季"));
                    }
                }
                parts.push(episode.to_owned());
                parts.join(" · ")
            }
            (Some(series), _) => series.to_owned(),
            (None, Some(episode)) => episode.to_owned(),
            (None, None) => "未知条目".to_owned(),
        };
        if notice.resumed {
            display.push_str("（续播）");
        }

        let client = [
            notice.client.as_deref(),
            notice.device_name.as_deref(),
            notice.device_type.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        let media = [
            notice
                .container
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.to_ascii_uppercase()),
            notice.size.map(|size| format_bytes(size.max(0) as u64)),
            notice.bitrate.map(|bitrate| format!("{:.1} Mbps", bitrate as f64 / 1_000_000.0)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");

        let mut event = Self::new(NotifyCategory::Playback, format!("正在播放《{display}》"))
            .field("👤 用户", notice.user_name)
            .field("📱 客户端", client)
            .field("🌍 IP", notice.remote_ip.unwrap_or_default())
            .field("🔗 播放", notice.play_method.unwrap_or_default())
            .field("🎞️ 媒体", media)
            .dedupe_key(format!(
                "playback:{}:{}",
                notice.session_key,
                notice.item_title.unwrap_or_default()
            ));
        event.occurred_at = notice.observed_at;
        if let Some(overview) = notice.overview {
            event = event.overview(overview);
        }
        event
    }
}

/// Build the notifications for one archive run: up to
/// [`MAX_ITEM_EVENTS_PER_RUN`] entries per category plus a run summary.
pub(crate) fn archive_events(report: &ArchiveReport) -> Vec<NotifyEvent> {
    let mut events = Vec::new();
    let truncated = report.archived_to_pending.len() > MAX_ITEM_EVENTS_PER_RUN
        || report.organized.len() > MAX_ITEM_EVENTS_PER_RUN
        || report.washed.len() > MAX_ITEM_EVENTS_PER_RUN
        || report.duplicates.len() > MAX_ITEM_EVENTS_PER_RUN
        || report.failed.len() > MAX_ITEM_EVENTS_PER_RUN;

    for name in report.archived_to_pending.iter().take(MAX_ITEM_EVENTS_PER_RUN) {
        events.push(
            NotifyEvent::new(NotifyCategory::Transfer, format!("已转存 {name}"))
                .field("📦 条目", name.as_str())
                .field("➡️ 下一步", "等待整理"),
        );
    }
    for item in report.organized.iter().take(MAX_ITEM_EVENTS_PER_RUN) {
        events.push(organized_event(item));
    }
    for item in report.washed.iter().take(MAX_ITEM_EVENTS_PER_RUN) {
        events.push(washed_event(item));
    }
    for item in report.duplicates.iter().take(MAX_ITEM_EVENTS_PER_RUN) {
        events.push(duplicate_event(item));
    }
    for item in report.failed.iter().take(MAX_ITEM_EVENTS_PER_RUN) {
        events.push(
            NotifyEvent::new(NotifyCategory::Failure, "归档失败")
                .field("📦 条目", item.source.as_str())
                .field("📝 原因", item.reason.as_str()),
        );
    }

    if has_activity(report) {
        events.push(summary_event(report, truncated));
    }
    events
}

fn organized_event(item: &OrganizedItem) -> NotifyEvent {
    let year = item.year.map(|year| format!(" ({year})")).unwrap_or_default();
    NotifyEvent::new(
        NotifyCategory::Organize,
        format!("整理完成《{}》{year}", item.title),
    )
    .field("🆔 TMDB", item.tmdb_id.to_string())
    .field("🎬 类型", item.kind.as_str())
    .field("🌏 地区", item.region.as_str())
    .field("📁 库位", item.target.as_str())
    .field("📦 来源", item.source.as_str())
}

fn washed_event(item: &WashedItem) -> NotifyEvent {
    let delta = if item.replaced_size > 0 {
        format!(
            "{:+.1}%",
            (item.kept_size as f64 - item.replaced_size as f64) / item.replaced_size as f64 * 100.0
        )
    } else {
        "—".to_owned()
    };
    NotifyEvent::new(NotifyCategory::Wash, format!("洗版完成 {}", item.source))
        .field(
            "📦 旧资源",
            format!("{} ({})", item.replaced, format_bytes(item.replaced_size)),
        )
        .field(
            "✨ 新资源",
            format!("{} ({})", item.source, format_bytes(item.kept_size)),
        )
        .field("📈 变化", delta)
}

fn duplicate_event(item: &DuplicateItem) -> NotifyEvent {
    NotifyEvent::new(NotifyCategory::Duplicate, format!("发现重复 {}", item.source))
        .field("📦 条目", item.source.as_str())
        .field("📁 已存在", item.existing.as_str())
        .field("⏭️ 处理", "跳过，未重复转存")
}

fn summary_event(report: &ArchiveReport, truncated: bool) -> NotifyEvent {    let mut event = NotifyEvent::new(NotifyCategory::Task, "归档任务完成")
        .field("📥 转存", report.archived_to_pending.len().to_string())
        .field("🗂️ 整理", report.organized.len().to_string())
        .field("🔁 洗版", report.washed.len().to_string())
        .field("♻️ 重复", report.duplicates.len().to_string())
        .field("❌ 失败", report.failed.len().to_string());
    let titles = report
        .organized
        .iter()
        .take(3)
        .map(|item| item.title.as_str())
        .collect::<Vec<_>>()
        .join("、《");
    if !titles.is_empty() {
        event = event.field("✅ 本次整理", format!("《{titles}》"));
    }
    if truncated {
        event = event.field(
            "ℹ️ 提示",
            format!("条目较多，单条通知只显示前 {MAX_ITEM_EVENTS_PER_RUN} 条，其余见汇总"),
        );
    }
    event
}

fn has_activity(report: &ArchiveReport) -> bool {
    !report.archived_to_pending.is_empty()
        || !report.organized.is_empty()
        || !report.washed.is_empty()
        || !report.duplicates.is_empty()
        || !report.failed.is_empty()
}

/// Request body for `POST /api/v1/admin/notify/send`, used by task modules and
/// the Python sidecar.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct NotifySendRequest {
    pub category: String,
    pub title: String,
    #[serde(default)]
    pub fields: Vec<NotifyFieldRequest>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub silent: Option<bool>,
    #[serde(default)]
    pub dedupe_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct NotifyFieldRequest {
    pub label: String,
    pub value: String,
}

impl NotifyEvent {
    pub(crate) fn from_request(request: NotifySendRequest) -> Result<Self, String> {
        let category = NotifyCategory::parse(&request.category)
            .ok_or_else(|| format!("未知通知类别: {}", request.category))?;
        if category == NotifyCategory::Test {
            return Err("测试通知请调用 /api/v1/admin/notify/test".to_owned());
        }
        let title = truncate_chars(request.title.trim(), 200);
        if title.is_empty() {
            return Err("title 不能为空".to_owned());
        }
        let mut event = Self::new(category, title);
        for field in request.fields.into_iter().take(MAX_REQUEST_FIELDS) {
            let label = truncate_chars(field.label.trim(), 40);
            let value = truncate_chars(field.value.trim(), 500);
            if label.is_empty() || value.is_empty() {
                continue;
            }
            event.fields.push(NotifyField { label, value });
        }
        if let Some(overview) = request.overview {
            event = event.overview(truncate_chars(overview.trim(), 2_000));
        }
        if let Some(url) = request.url {
            event = event.url(url.trim());
        }
        event.silent = request.silent;
        event.dedupe_key = request
            .dedupe_key
            .map(|key| truncate_chars(key.trim(), 200))
            .filter(|key| !key.is_empty());
        Ok(event)
    }
}

fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

fn is_safe_url(url: &str) -> bool {
    matches!(
        url::Url::parse(url),
        Ok(parsed) if matches!(parsed.scheme(), "http" | "https") && parsed.host().is_some()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> ArchiveReport {
        ArchiveReport {
            dry_run: false,
            archived_to_pending: vec!["测试资源A".to_owned()],
            organized: vec![OrganizedItem {
                source: "测试资源A".to_owned(),
                tmdb_id: 12345,
                target: "/library/电影/国内/2024/示例 (2024)".to_owned(),
                kind: "电影".to_owned(),
                region: "国内".to_owned(),
                year: Some(2024),
                title: "示例".to_owned(),
            }],
            duplicates: vec![DuplicateItem {
                source: "重复资源".to_owned(),
                existing: "/library/电影/国内/2024/示例 (2024)".to_owned(),
            }],
            washed: vec![WashedItem {
                source: "示例.2024.2160p.mkv".to_owned(),
                replaced: "/wash/1700000000-示例.2024.1080p".to_owned(),
                replaced_size: 1_000_000_000,
                kept_size: 5_000_000_000,
            }],
            failed: vec![FailedItem {
                source: "坏资源".to_owned(),
                reason: "无法匹配 TMDB".to_owned(),
            }],
            skipped: vec![],
        }
    }

    #[test]
    fn archive_report_produces_one_event_per_item_and_a_summary() {
        let events = archive_events(&report());
        let categories: Vec<_> = events.iter().map(|event| event.category).collect();
        assert_eq!(
            categories,
            vec![
                NotifyCategory::Transfer,
                NotifyCategory::Organize,
                NotifyCategory::Wash,
                NotifyCategory::Duplicate,
                NotifyCategory::Failure,
                NotifyCategory::Task,
            ]
        );
        let summary = events.last().expect("summary");
        assert_eq!(summary.title, "归档任务完成");
        assert!(
            summary
                .fields
                .iter()
                .any(|field| field.label == "❌ 失败" && field.value == "1")
        );
        assert!(
            summary
                .fields
                .iter()
                .any(|field| field.label == "♻️ 重复" && field.value == "1")
        );
    }

    #[test]
    fn duplicate_event_carries_the_existing_path() {
        let event = duplicate_event(&report().duplicates[0]);
        assert_eq!(event.category, NotifyCategory::Duplicate);
        assert!(event.title.contains("重复资源"));
        assert!(
            event
                .fields
                .iter()
                .any(|field| field.label == "📁 已存在" && field.value.contains("示例"))
        );
    }

    #[test]
    fn empty_report_emits_nothing() {
        let events = archive_events(&ArchiveReport::default());
        assert!(events.is_empty());
    }

    #[test]
    fn organized_event_carries_classification_fields() {
        let item = &report().organized[0];
        let event = organized_event(item);
        assert!(event.title.contains("示例"));
        assert!(event.title.contains("2024"));
        assert!(event.fields.iter().any(|field| field.value == "国内"));
        assert!(event.fields.iter().any(|field| field.value == "12345"));
    }

    #[test]
    fn washed_event_reports_size_growth() {
        let event = washed_event(&report().washed[0]);
        assert_eq!(
            event
                .fields
                .iter()
                .find(|field| field.label == "📈 变化")
                .map(|field| field.value.as_str()),
            Some("+400.0%")
        );
    }

    #[test]
    fn archive_events_are_capped_per_category() {
        let mut report = ArchiveReport::default();
        report.archived_to_pending = (0..25).map(|index| format!("资源{index}")).collect();
        let events = archive_events(&report);
        let transfers = events
            .iter()
            .filter(|event| event.category == NotifyCategory::Transfer)
            .count();
        assert_eq!(transfers, MAX_ITEM_EVENTS_PER_RUN);
        assert!(
            events
                .last()
                .expect("summary")
                .fields
                .iter()
                .any(|field| field.label == "ℹ️ 提示")
        );
    }

    #[test]
    fn playback_event_builds_series_title_and_metadata() {
        let event = NotifyEvent::playback(PlaybackNotice {
            item_title: Some("第3集".to_owned()),
            series_title: Some("示例剧".to_owned()),
            season_number: Some(2),
            season_count: Some(3),
            user_name: "qiangli".to_owned(),
            client: Some("infuse".to_owned()),
            device_name: Some("Apple TV".to_owned()),
            remote_ip: Some("192.168.8.10".to_owned()),
            play_method: Some("DirectPlay".to_owned()),
            size: Some(2_500_000_000),
            observed_at: 1_700_000_000,
            session_key: "session".to_owned(),
            ..PlaybackNotice::default()
        });
        assert_eq!(event.title, "正在播放《示例剧 · 第2季 · 第3集》");
        assert_eq!(event.category, NotifyCategory::Playback);
        assert!(
            event
                .fields
                .iter()
                .any(|field| field.label == "🎞️ 媒体" && field.value.contains("2.3 GB"))
        );
    }

    #[test]
    fn resumed_playback_is_marked() {
        let event = NotifyEvent::playback(PlaybackNotice {
            item_title: Some("示例电影".to_owned()),
            user_name: "u".to_owned(),
            resumed: true,
            observed_at: 1,
            ..PlaybackNotice::default()
        });
        assert!(event.title.contains("续播"));
    }

    #[test]
    fn request_rejects_unknown_category_and_blank_title() {
        let error = NotifyEvent::from_request(NotifySendRequest {
            category: "unknown".to_owned(),
            title: "x".to_owned(),
            fields: vec![],
            overview: None,
            url: None,
            silent: None,
            dedupe_key: None,
        })
        .expect_err("unknown category");
        assert!(error.contains("未知通知类别"));

        let error = NotifyEvent::from_request(NotifySendRequest {
            category: "task".to_owned(),
            title: "   ".to_owned(),
            fields: vec![],
            overview: None,
            url: None,
            silent: None,
            dedupe_key: None,
        })
        .expect_err("blank title");
        assert!(error.contains("title"));
    }

    #[test]
    fn request_accepts_chinese_labels_and_bounds_fields() {
        let event = NotifyEvent::from_request(NotifySendRequest {
            category: "签到".to_owned(),
            title: "签到完成".to_owned(),
            fields: (0..40)
                .map(|index| NotifyFieldRequest {
                    label: "结果".to_owned(),
                    value: format!("第{index}项"),
                })
                .collect(),
            overview: Some("总览".to_owned()),
            url: None,
            silent: Some(true),
            dedupe_key: Some("checkin:2026-10-18".to_owned()),
        })
        .expect("valid request");
        assert_eq!(event.category, NotifyCategory::Checkin);
        assert_eq!(event.fields.len(), MAX_REQUEST_FIELDS);
        assert_eq!(event.silent, Some(true));
        assert_eq!(event.dedupe_key.as_deref(), Some("checkin:2026-10-18"));
    }

    #[test]
    fn unsafe_urls_are_dropped() {
        let event = NotifyEvent::new(NotifyCategory::Task, "任务").url("javascript:alert(1)");
        assert!(event.url.is_none());
    }
}
