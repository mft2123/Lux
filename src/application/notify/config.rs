//! Configuration for the Telegram notification module.
//!
//! The settings file is owned by the archive settings module: this module
//! consumes [`ArchiveSettings`] (file over environment over defaults) and only
//! fills keys the file does not provide from `LUX_TG_*` variables. The file is
//! never written here.

use std::{collections::BTreeSet, time::Duration};

use serde::Serialize;
use serde_json::Value;

use crate::application::archive::{ArchiveSettings, mask_secret};

const DEFAULT_API_BASE: &str = "https://api.telegram.org";
const DEFAULT_TIMEZONE_OFFSET_HOURS: i64 = 8;
const DEFAULT_MIN_INTERVAL_MS: u64 = 1_200;
const DEFAULT_MAX_PER_MINUTE: u32 = 20;
const DEFAULT_DEDUPE_SECONDS: i64 = 60;

/// One notification category. The wire keys follow chill's
/// `wechat_notify.json` so an existing settings file can be reused.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NotifyCategory {
    Playback,
    MediaAdded,
    Organize,
    Wash,
    Duplicate,
    Transfer,
    MagicUpload,
    MediaOrganize,
    Checkin,
    Task,
    Failure,
    #[serde(skip)]
    Test,
}

impl NotifyCategory {
    pub(crate) const ALL: [Self; 11] = [
        Self::Playback,
        Self::MediaAdded,
        Self::Organize,
        Self::Wash,
        Self::Duplicate,
        Self::Transfer,
        Self::MagicUpload,
        Self::MediaOrganize,
        Self::Checkin,
        Self::Task,
        Self::Failure,
    ];

    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Playback => "playback",
            Self::MediaAdded => "media_added",
            Self::Organize => "organize_complete",
            Self::Wash => "wash_result",
            Self::Duplicate => "duplicate",
            Self::Transfer => "resource_transfer",
            Self::MagicUpload => "drive115_upload",
            Self::MediaOrganize => "media_organize_task",
            Self::Checkin => "checkin",
            Self::Task => "task_complete",
            Self::Failure => "failure",
            Self::Test => "test",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Playback => "播放",
            Self::MediaAdded => "入库",
            Self::Organize => "整理",
            Self::Wash => "洗版",
            Self::Duplicate => "重复",
            Self::Transfer => "转存",
            Self::MagicUpload => "115妙传",
            Self::MediaOrganize => "媒体整理",
            Self::Checkin => "签到",
            Self::Task => "其他任务",
            Self::Failure => "失败",
            Self::Test => "测试",
        }
    }

    pub(crate) const fn emoji(self) -> &'static str {
        match self {
            Self::Playback => "🎬",
            Self::MediaAdded => "✅",
            Self::Organize => "🗂️",
            Self::Wash => "🔁",
            Self::Duplicate => "♻️",
            Self::Transfer => "📥",
            Self::MagicUpload => "⚡",
            Self::MediaOrganize => "🧩",
            Self::Checkin => "🔔",
            Self::Task => "📋",
            Self::Failure => "❌",
            Self::Test => "🧪",
        }
    }

    /// Accepts the English keys as well as the Chinese labels used by the
    /// settings UI (`入库`/`整理`/…).
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "playback" | "play" | "播放" => Some(Self::Playback),
            "media_added" | "mediaadded" | "media-added" | "入库" => Some(Self::MediaAdded),
            "organize_complete" | "organize" | "archive" | "整理" | "归档" => Some(Self::Organize),
            "wash_result" | "wash" | "洗版" => Some(Self::Wash),
            "duplicate" | "duplicates" | "dupe" | "重复" | "重复资源" => Some(Self::Duplicate),
            "resource_transfer" | "transfer" | "转存" => Some(Self::Transfer),
            "drive115_upload" | "magic_upload" | "115" | "115妙传" | "妙传" | "秒传" => {
                Some(Self::MagicUpload)
            }
            "media_organize_task" | "media_organize" | "媒体整理" => Some(Self::MediaOrganize),
            "checkin" | "signin" | "签到" => Some(Self::Checkin),
            "task_complete" | "task" | "strm" | "其他" | "其他任务" | "任务" => Some(Self::Task),
            "failure" | "failed" | "error" | "失败" => Some(Self::Failure),
            "test" | "测试" => Some(Self::Test),
            _ => None,
        }
    }
}

/// The notification categories selected in the settings file. Missing
/// configuration means "everything on".
#[derive(Clone, Debug)]
pub(crate) struct NotifyToggles {
    enabled: BTreeSet<NotifyCategory>,
}

impl Default for NotifyToggles {
    fn default() -> Self {
        Self::all()
    }
}

impl NotifyToggles {
    pub(crate) fn all() -> Self {
        Self {
            enabled: NotifyCategory::ALL.into_iter().collect(),
        }
    }

    pub(crate) fn none() -> Self {
        Self {
            enabled: BTreeSet::new(),
        }
    }

    pub(crate) fn contains(&self, category: NotifyCategory) -> bool {
        self.enabled.contains(&category)
    }

    fn set(&mut self, category: NotifyCategory, enabled: bool) {
        if enabled {
            self.enabled.insert(category);
        } else {
            self.enabled.remove(&category);
        }
    }

    /// `null` means "not configured" (all defaults); an array is the exact
    /// selection; an object maps category keys to booleans.
    fn from_json(value: &Value) -> Option<Self> {
        match value {
            Value::Array(items) => {
                let mut selection = Self::none();
                let mut everything = false;
                for item in items {
                    let Some(raw) = item.as_str() else {
                        continue;
                    };
                    if matches!(raw.trim(), "全部" | "所有" | "all" | "*") {
                        everything = true;
                        continue;
                    }
                    if let Some(category) = NotifyCategory::parse(raw)
                        && category != NotifyCategory::Test
                    {
                        selection.set(category, true);
                    }
                }
                Some(if everything { Self::all() } else { selection })
            }
            Value::Object(map) => {
                let mut selection = Self::all();
                for (key, enabled) in map {
                    let Some(category) = NotifyCategory::parse(key) else {
                        continue;
                    };
                    if category == NotifyCategory::Test {
                        continue;
                    }
                    selection.set(category, enabled.as_bool().unwrap_or(true));
                }
                Some(selection)
            }
            _ => None,
        }
    }
}

/// Process-level `LUX_TG_*` values. They only fill keys the settings file does
/// not provide; the file always wins (see `docs/archive-pipeline.md` §8).
#[derive(Clone, Debug, Default)]
pub(crate) struct NotifyEnv {
    pub bot_token: Option<String>,
    pub chat_id: Option<String>,
    pub api_base: Option<String>,
    pub thread_id: Option<i64>,
    pub silent: Option<bool>,
    pub notify: Option<String>,
    pub min_interval_ms: Option<u64>,
    pub max_per_minute: Option<u32>,
    pub dedupe_seconds: Option<i64>,
    pub timezone_offset_hours: Option<i64>,
}

impl NotifyEnv {
    pub(crate) fn from_process() -> Self {
        Self {
            bot_token: env_non_empty("LUX_TG_BOT_TOKEN"),
            chat_id: env_non_empty("LUX_TG_CHAT_ID"),
            api_base: env_non_empty("LUX_TG_API_BASE"),
            thread_id: env_non_empty("LUX_TG_THREAD_ID").and_then(|value| value.parse().ok()),
            silent: env_non_empty("LUX_TG_SILENT").map(|value| truthy(&value)),
            notify: env_non_empty("LUX_TG_NOTIFY"),
            min_interval_ms: env_non_empty("LUX_TG_MIN_INTERVAL_MS")
                .and_then(|value| value.parse().ok()),
            max_per_minute: env_non_empty("LUX_TG_MAX_PER_MINUTE")
                .and_then(|value| value.parse().ok()),
            dedupe_seconds: env_non_empty("LUX_TG_DEDUPE_SECONDS")
                .and_then(|value| value.parse().ok()),
            timezone_offset_hours: env_non_empty("LUX_TG_TIMEZONE_OFFSET_HOURS")
                .and_then(|value| value.parse().ok()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NotifyConfig {
    pub enabled: bool,
    pub bot_token: Option<String>,
    pub chat_id: Option<String>,
    pub api_base: String,
    pub thread_id: Option<i64>,
    pub silent: bool,
    pub toggles: NotifyToggles,
    pub min_interval: Duration,
    pub max_per_minute: u32,
    pub dedupe_window: Duration,
    pub timezone_offset_hours: i64,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bot_token: None,
            chat_id: None,
            api_base: DEFAULT_API_BASE.to_owned(),
            thread_id: None,
            silent: false,
            toggles: NotifyToggles::default(),
            min_interval: Duration::from_millis(DEFAULT_MIN_INTERVAL_MS),
            max_per_minute: DEFAULT_MAX_PER_MINUTE,
            dedupe_window: Duration::from_secs(DEFAULT_DEDUPE_SECONDS as u64),
            timezone_offset_hours: DEFAULT_TIMEZONE_OFFSET_HOURS,
        }
    }
}

impl NotifyConfig {
    /// Effective settings: the archive settings file wins, `LUX_TG_*` only
    /// fills keys the file does not define.
    pub(crate) fn load() -> Self {
        Self::from_parts(&ArchiveSettings::load(), &NotifyEnv::from_process())
    }

    /// Pure constructor used by `load()` and the unit tests: values from
    /// `settings.telegram` take precedence over `env`.
    pub(crate) fn from_parts(settings: &ArchiveSettings, env: &NotifyEnv) -> Self {
        let telegram = &settings.telegram;
        let toggles = NotifyToggles::from_json(&telegram.notify)
            .or_else(|| {
                env.notify
                    .as_deref()
                    .map(|value| toggles_from_list(value.split(',')))
            })
            .unwrap_or_default();
        Self {
            enabled: telegram.enabled.unwrap_or(true),
            bot_token: file_value(&telegram.bot_token).or_else(|| env.bot_token.clone()),
            chat_id: file_value(&telegram.chat_id).or_else(|| env.chat_id.clone()),
            api_base: file_value(telegram.api_base.as_deref().unwrap_or_default())
                .or_else(|| env.api_base.clone())
                .unwrap_or_else(|| DEFAULT_API_BASE.to_owned())
                .trim_end_matches('/')
                .to_owned(),
            thread_id: telegram.message_thread_id.or(env.thread_id),
            silent: telegram.silent.or(env.silent).unwrap_or(false),
            toggles,
            min_interval: Duration::from_millis(
                telegram
                    .min_interval_ms
                    .or(env.min_interval_ms)
                    .unwrap_or(DEFAULT_MIN_INTERVAL_MS)
                    .clamp(100, 60_000),
            ),
            max_per_minute: u32::try_from(
                telegram
                    .max_per_minute
                    .or(env.max_per_minute.map(u64::from))
                    .unwrap_or(u64::from(DEFAULT_MAX_PER_MINUTE))
                    .clamp(1, 60),
            )
            .unwrap_or(DEFAULT_MAX_PER_MINUTE),
            dedupe_window: Duration::from_secs(
                telegram
                    .dedupe_seconds
                    .or(env.dedupe_seconds)
                    .unwrap_or(DEFAULT_DEDUPE_SECONDS)
                    .clamp(0, 3_600) as u64,
            ),
            timezone_offset_hours: telegram
                .timezone_offset_hours
                .or(env.timezone_offset_hours)
                .unwrap_or(DEFAULT_TIMEZONE_OFFSET_HOURS)
                .clamp(-12, 14),
        }
    }

    pub(crate) fn configured(&self) -> bool {
        self.enabled && self.bot_token.is_some() && self.chat_id.is_some()
    }

    pub(crate) fn category_enabled(&self, category: NotifyCategory) -> bool {
        category == NotifyCategory::Test || self.toggles.contains(category)
    }

    /// Chat id, masked exactly like the settings API (`head6…(len)`).
    pub(crate) fn masked_chat_id(&self) -> Option<String> {
        self.chat_id
            .as_deref()
            .map(mask_secret)
            .filter(|value| !value.is_empty())
    }

    /// Base URL safe for the status response: credentials and any path or
    /// query that could carry a token are dropped.
    pub(crate) fn masked_api_base(&self) -> String {
        mask_api_base(&self.api_base)
    }
}

fn toggles_from_list<'a>(items: impl Iterator<Item = &'a str>) -> NotifyToggles {
    let mut toggles = NotifyToggles::none();
    for item in items {
        let item = item.trim();
        if matches!(item, "全部" | "所有" | "all" | "*") {
            return NotifyToggles::all();
        }
        if let Some(category) = NotifyCategory::parse(item) {
            toggles.set(category, true);
        }
    }
    toggles
}

fn file_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn truthy(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "off" | "no"
    )
}

pub(crate) fn mask_api_base(api_base: &str) -> String {
    let Ok(url) = url::Url::parse(api_base) else {
        return "***".to_owned();
    };
    let mut masked = format!("{}://", url.scheme());
    if url.username().is_empty() && url.password().is_none() {
        if let Some(host) = url.host_str() {
            masked.push_str(host);
        }
    } else {
        masked.push_str("***");
    }
    if let Some(port) = url.port() {
        masked.push(':');
        masked.push_str(&port.to_string());
    }
    if url.path() != "/" && !url.path().is_empty() {
        masked.push_str("/…");
    }
    masked
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn settings_with(token: &str, chat: &str) -> ArchiveSettings {
        let mut settings = ArchiveSettings::default();
        settings.telegram.bot_token = token.to_owned();
        settings.telegram.chat_id = chat.to_owned();
        settings
    }

    #[test]
    fn categories_round_trip_keys_and_chinese_aliases() {
        for category in NotifyCategory::ALL {
            assert_eq!(NotifyCategory::parse(category.key()), Some(category));
            assert_eq!(NotifyCategory::parse(category.label()), Some(category));
        }
        assert_eq!(
            NotifyCategory::parse("秒传"),
            Some(NotifyCategory::MagicUpload)
        );
        assert_eq!(NotifyCategory::parse("归档"), Some(NotifyCategory::Organize));
        assert_eq!(
            NotifyCategory::parse("重复资源"),
            Some(NotifyCategory::Duplicate)
        );
        assert_eq!(NotifyCategory::parse("未知"), None);
    }

    #[test]
    fn file_values_win_over_environment() {
        let mut settings = settings_with("file-token", "file-chat");
        settings.telegram.min_interval_ms = Some(2_000);
        let env = NotifyEnv {
            bot_token: Some("env-token".to_owned()),
            chat_id: Some("env-chat".to_owned()),
            min_interval_ms: Some(5_000),
            ..NotifyEnv::default()
        };
        let config = NotifyConfig::from_parts(&settings, &env);
        assert_eq!(config.bot_token.as_deref(), Some("file-token"));
        assert_eq!(config.chat_id.as_deref(), Some("file-chat"));
        assert_eq!(config.min_interval, Duration::from_millis(2_000));
    }

    #[test]
    fn environment_only_fills_missing_keys() {
        let settings = settings_with("", "");
        let env = NotifyEnv {
            bot_token: Some("env-token".to_owned()),
            chat_id: Some("env-chat".to_owned()),
            min_interval_ms: Some(5_000),
            ..NotifyEnv::default()
        };
        let config = NotifyConfig::from_parts(&settings, &env);
        assert_eq!(config.bot_token.as_deref(), Some("env-token"));
        assert_eq!(config.chat_id.as_deref(), Some("env-chat"));
        assert_eq!(config.min_interval, Duration::from_millis(5_000));
        assert!(config.configured());
    }

    #[test]
    fn settings_file_shape_with_chinese_array_is_honoured() {
        let mut settings = settings_with("123:abc", "5867802360");
        settings.telegram.notify = json!(["入库", "归档", "失败", "洗版", "重复"]);
        let config = NotifyConfig::from_parts(&settings, &NotifyEnv::default());
        assert!(config.configured());
        assert!(config.category_enabled(NotifyCategory::MediaAdded));
        assert!(config.category_enabled(NotifyCategory::Organize));
        assert!(config.category_enabled(NotifyCategory::Wash));
        assert!(config.category_enabled(NotifyCategory::Duplicate));
        assert!(config.category_enabled(NotifyCategory::Failure));
        assert!(!config.category_enabled(NotifyCategory::Playback));
        assert!(!config.category_enabled(NotifyCategory::Checkin));
    }

    #[test]
    fn environment_notify_list_is_used_only_when_the_file_has_none() {
        let mut settings = settings_with("t", "c");
        settings.telegram.notify = json!(["播放"]);
        let env = NotifyEnv {
            notify: Some("签到".to_owned()),
            ..NotifyEnv::default()
        };
        let config = NotifyConfig::from_parts(&settings, &env);
        assert!(config.category_enabled(NotifyCategory::Playback));
        assert!(!config.category_enabled(NotifyCategory::Checkin));

        settings.telegram.notify = Value::Null;
        let config = NotifyConfig::from_parts(&settings, &env);
        assert!(config.category_enabled(NotifyCategory::Checkin));
        assert!(!config.category_enabled(NotifyCategory::Playback));
    }

    #[test]
    fn missing_notify_key_enables_every_category() {
        let settings = settings_with("t", "c");
        let config = NotifyConfig::from_parts(&settings, &NotifyEnv::default());
        for category in NotifyCategory::ALL {
            assert!(config.category_enabled(category));
        }
    }

    #[test]
    fn notify_object_supports_per_category_switches() {
        let mut settings = settings_with("t", "c");
        settings.telegram.notify = json!({ "playback": false, "签到": true });
        let config = NotifyConfig::from_parts(&settings, &NotifyEnv::default());
        assert!(!config.category_enabled(NotifyCategory::Playback));
        assert!(config.category_enabled(NotifyCategory::Checkin));
        assert!(config.category_enabled(NotifyCategory::Task));
    }

    #[test]
    fn no_credentials_means_not_configured() {
        let settings = ArchiveSettings::default();
        let config = NotifyConfig::from_parts(&settings, &NotifyEnv::default());
        assert!(!config.configured());
        assert!(config.category_enabled(NotifyCategory::Playback));
    }

    #[test]
    fn test_category_is_always_allowed() {
        let mut settings = settings_with("t", "c");
        settings.telegram.notify = json!([]);
        let config = NotifyConfig::from_parts(&settings, &NotifyEnv::default());
        assert!(config.category_enabled(NotifyCategory::Test));
        assert!(!config.category_enabled(NotifyCategory::Playback));
    }

    #[test]
    fn chat_id_uses_the_settings_mask_format() {
        let settings = settings_with("t", "5867802360");
        let config = NotifyConfig::from_parts(&settings, &NotifyEnv::default());
        assert_eq!(config.masked_chat_id().as_deref(), Some("586780…(10)"));
    }

    #[test]
    fn api_base_drops_credentials_and_paths() {
        assert_eq!(
            mask_api_base("https://api.telegram.org"),
            "https://api.telegram.org"
        );
        assert_eq!(
            mask_api_base("https://user:secret@tg.example.com:8443/bot123/sendMessage"),
            "https://***:8443/…"
        );
        assert_eq!(
            mask_api_base("https://tg.example.com/proxy"),
            "https://tg.example.com/…"
        );
        assert_eq!(mask_api_base("not a url"), "***");
    }
}
