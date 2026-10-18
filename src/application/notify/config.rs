//! Configuration for the Telegram notification module.
//!
//! Credentials live in the archive settings file (`telegram` section) that the
//! settings module owns, with environment variables as a fallback. This module
//! never writes the settings file.

use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::Duration,
};

use serde::Serialize;
use serde_json::Value;

pub(crate) const SETTINGS_PATH_ENV: &str = "LUX_ARCHIVE_SETTINGS";
pub(crate) const DEFAULT_SETTINGS_PATH: &str = "/config/archive-settings.json";

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
    pub(crate) const ALL: [Self; 10] = [
        Self::Playback,
        Self::MediaAdded,
        Self::Organize,
        Self::Wash,
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
            "organize_complete" | "organize" | "archive" | "整理" | "归档" => {
                Some(Self::Organize)
            }
            "wash_result" | "wash" | "洗版" => Some(Self::Wash),
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
    /// Load the settings file (if any) and apply environment overrides.
    pub(crate) fn load() -> Self {
        let mut config = match read_settings_file() {
            Some(settings) => Self::from_settings_value(&settings),
            None => Self::default(),
        };
        config.apply_env();
        config
    }

    /// Build a config from a settings document; exposed for the settings
    /// module and for tests.
    pub(crate) fn from_settings_value(settings: &Value) -> Self {
        let mut config = Self::default();
        config.apply_settings(settings);
        config
    }

    fn apply_settings(&mut self, settings: &Value) {
        let Some(telegram) = settings.get("telegram") else {
            return;
        };
        if let Some(enabled) = telegram.get("enabled").and_then(Value::as_bool) {
            self.enabled = enabled;
        }
        if let Some(value) = non_empty_str(telegram.get("bot_token")) {
            self.bot_token = Some(value);
        }
        if let Some(value) = scalar_string(telegram.get("chat_id")) {
            self.chat_id = Some(value);
        }
        if let Some(value) = non_empty_str(telegram.get("api_base")) {
            self.api_base = value.trim_end_matches('/').to_owned();
        }
        if let Some(value) = telegram
            .get("message_thread_id")
            .or_else(|| telegram.get("thread_id"))
            .and_then(Value::as_i64)
        {
            self.thread_id = Some(value);
        }
        if let Some(silent) = telegram.get("silent").and_then(Value::as_bool) {
            self.silent = silent;
        }
        if let Some(toggles) = telegram.get("notify").and_then(NotifyToggles::from_json) {
            self.toggles = toggles;
        }
        if let Some(ms) = telegram.get("min_interval_ms").and_then(Value::as_u64) {
            self.min_interval = Duration::from_millis(ms.clamp(100, 60_000));
        }
        if let Some(limit) = telegram.get("max_per_minute").and_then(Value::as_u64) {
            self.max_per_minute = u32::try_from(limit.clamp(1, 60)).unwrap_or(20);
        }
        if let Some(seconds) = telegram.get("dedupe_seconds").and_then(Value::as_i64) {
            self.dedupe_window = Duration::from_secs(seconds.clamp(0, 3_600) as u64);
        }
        if let Some(hours) = telegram.get("timezone_offset_hours").and_then(Value::as_i64) {
            self.timezone_offset_hours = hours.clamp(-12, 14);
        }
    }

    fn apply_env(&mut self) {
        if let Some(value) = env_non_empty("LUX_TG_BOT_TOKEN") {
            self.bot_token = Some(value);
        }
        if let Some(value) = env_non_empty("LUX_TG_CHAT_ID") {
            self.chat_id = Some(value);
        }
        if let Some(value) = env_non_empty("LUX_TG_API_BASE") {
            self.api_base = value.trim_end_matches('/').to_owned();
        }
        if let Some(value) = env_non_empty("LUX_TG_THREAD_ID").and_then(|v| v.parse().ok()) {
            self.thread_id = Some(value);
        }
        if let Some(value) = env_non_empty("LUX_TG_SILENT") {
            self.silent = truthy(&value);
        }
        if let Some(value) = env_non_empty("LUX_TG_NOTIFY") {
            self.toggles = toggles_from_list(value.split(','));
        }
        if let Some(value) =
            env_non_empty("LUX_TG_MIN_INTERVAL_MS").and_then(|v| v.parse::<u64>().ok())
        {
            self.min_interval = Duration::from_millis(value.clamp(100, 60_000));
        }
        if let Some(value) =
            env_non_empty("LUX_TG_MAX_PER_MINUTE").and_then(|v| v.parse::<u32>().ok())
        {
            self.max_per_minute = value.clamp(1, 60);
        }
        if let Some(value) =
            env_non_empty("LUX_TG_DEDUPE_SECONDS").and_then(|v| v.parse::<i64>().ok())
        {
            self.dedupe_window = Duration::from_secs(value.clamp(0, 3_600) as u64);
        }
        if let Some(value) =
            env_non_empty("LUX_TG_TIMEZONE_OFFSET_HOURS").and_then(|v| v.parse::<i64>().ok())
        {
            self.timezone_offset_hours = value.clamp(-12, 14);
        }
    }

    pub(crate) fn configured(&self) -> bool {
        self.enabled && self.bot_token.is_some() && self.chat_id.is_some()
    }

    pub(crate) fn category_enabled(&self, category: NotifyCategory) -> bool {
        category == NotifyCategory::Test || self.toggles.contains(category)
    }

    /// Masked chat id suitable for admin responses and logs.
    pub(crate) fn masked_chat_id(&self) -> Option<String> {
        self.chat_id.as_deref().map(mask_secret)
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

fn read_settings_file() -> Option<Value> {
    let path = std::env::var(SETTINGS_PATH_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SETTINGS_PATH));
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn non_empty_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn scalar_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        }
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn truthy(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "off" | "no"
    )
}

pub(crate) fn mask_secret(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 6 {
        return "***".to_owned();
    }
    let head: String = chars.iter().take(3).collect();
    let tail: String = chars.iter().skip(chars.len() - 2).collect();
    format!("{head}***{tail}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn categories_round_trip_keys_and_chinese_aliases() {
        for category in NotifyCategory::ALL {
            assert_eq!(NotifyCategory::parse(category.key()), Some(category));
            assert_eq!(NotifyCategory::parse(category.label()), Some(category));
        }
        assert_eq!(NotifyCategory::parse("秒传"), Some(NotifyCategory::MagicUpload));
        assert_eq!(NotifyCategory::parse("归档"), Some(NotifyCategory::Organize));
        assert_eq!(NotifyCategory::parse("未知"), None);
    }

    #[test]
    fn settings_file_shape_with_chinese_array_is_honoured() {
        let settings = json!({
            "telegram": {
                "bot_token": "123:abc",
                "chat_id": 5867802360_i64,
                "notify": ["入库", "归档", "失败", "洗版"]
            }
        });
        let config = NotifyConfig::from_settings_value(&settings);
        assert_eq!(config.bot_token.as_deref(), Some("123:abc"));
        assert_eq!(config.chat_id.as_deref(), Some("5867802360"));
        assert!(config.configured());
        assert!(config.category_enabled(NotifyCategory::MediaAdded));
        assert!(config.category_enabled(NotifyCategory::Organize));
        assert!(config.category_enabled(NotifyCategory::Wash));
        assert!(config.category_enabled(NotifyCategory::Failure));
        assert!(!config.category_enabled(NotifyCategory::Playback));
        assert!(!config.category_enabled(NotifyCategory::Checkin));
    }

    #[test]
    fn missing_notify_key_enables_every_category() {
        let config = NotifyConfig::from_settings_value(&json!({
            "telegram": { "bot_token": "t", "chat_id": "c" }
        }));
        for category in NotifyCategory::ALL {
            assert!(config.category_enabled(category));
        }
    }

    #[test]
    fn notify_object_supports_per_category_switches() {
        let config = NotifyConfig::from_settings_value(&json!({
            "telegram": {
                "bot_token": "t",
                "chat_id": "c",
                "notify": { "playback": false, "签到": true }
            }
        }));
        assert!(!config.category_enabled(NotifyCategory::Playback));
        assert!(config.category_enabled(NotifyCategory::Checkin));
        assert!(config.category_enabled(NotifyCategory::Task));
    }

    #[test]
    fn no_credentials_means_not_configured() {
        let config = NotifyConfig::from_settings_value(&json!({}));
        assert!(!config.configured());
        assert!(config.category_enabled(NotifyCategory::Playback));
    }

    #[test]
    fn test_category_is_always_allowed() {
        let config = NotifyConfig::from_settings_value(&json!({
            "telegram": { "bot_token": "t", "chat_id": "c", "notify": [] }
        }));
        assert!(config.category_enabled(NotifyCategory::Test));
        assert!(!config.category_enabled(NotifyCategory::Playback));
    }

    #[test]
    fn secrets_are_masked_for_responses() {
        assert_eq!(mask_secret("5867802360"), "586***60");
        assert_eq!(mask_secret("123"), "***");
    }
}
