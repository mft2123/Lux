//! Editable archive settings.
//!
//! Everything the archive pipeline can be tuned with lives in one JSON file so
//! it can be changed from the admin API and the web UI without a restart:
//! `LUX_ARCHIVE_SETTINGS`, default `/config/archive-settings.json`.
//!
//! Precedence is per field: the file wins, a key missing from the file keeps the
//! environment value (`LUX_ARCHIVE_*` / `LUX_TMDB_API_KEY`), and a key missing
//! from both falls back to the built-in default. A value the file cannot parse
//! is ignored instead of discarding the rest of the file.
//!
//! The file is only ever written by this module, with mode 0600. Secrets (TMDB
//! key, bot token, chat id, api hash, phone) are masked in every API response,
//! and a blank secret in an update means "keep the stored value" so the web UI
//! can send a masked form back without leaking or clearing it.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Serialize;
use serde_json::{Map, Value};

use super::{
    ARCHIVE_CD2_MOUNT_ENV, ARCHIVE_CD2_PREFIX_ENV, ARCHIVE_MIRROR_ROOT_ENV, ARCHIVE_POLL_SECONDS_ENV,
    ARCHIVE_SETTLE_SECONDS_ENV, ARCHIVE_TMDB_KEY_ENV, ARCHIVE_WASH_ENABLED_ENV,
    ARCHIVE_WASH_TOLERANCE_ENV, ARCHIVE_WATCH_DIRS_ENV, DEFAULT_CD2_CONTAINER_MOUNT,
    DEFAULT_MIRROR_ROOT, DEFAULT_SETTLE_SECONDS, TRANSFER115_COOKIE_ENV,
    TRANSFER115_SIDECAR_BASE_ENV, TRANSFER115_TARGET_DIR_ENV, TRANSFER115_TIMEOUT_ENV,
};

/// Where the settings file lives when the environment does not say otherwise.
pub(crate) const ARCHIVE_SETTINGS_ENV: &str = "LUX_ARCHIVE_SETTINGS";
pub(crate) const DEFAULT_ARCHIVE_SETTINGS_PATH: &str = "/config/archive-settings.json";

/// Bounds shared by the env parsing, the file sanitizing and the API
/// validation, so a bad value is either clamped on load or rejected on write.
const MAX_POLL_SECONDS: u64 = 3_600;
const MAX_SETTLE_SECONDS: u64 = 3_600;
const DEFAULT_WASH_TOLERANCE_RATIO: f64 = 0.02;

/// The five listening modes of the Telethon sidecar, in the order the UI shows
/// them. The English aliases are accepted as well so a hand-edited file can use
/// either spelling.
pub(crate) const LISTEN_MODES: [&str; 5] = ["手动", "上新", "追更", "洗版", "全量"];
const LISTEN_MODE_ALIASES: [&str; 5] = ["manual", "new", "follow", "wash", "all"];
pub(crate) const DEFAULT_LISTEN_MODE: &str = "手动";

/// Default fixed target for 115 share-link transfers, relative to the archive
/// root: the pipeline's own transfer directory, which the archive stage feeds
/// into 待整理 exactly like a manual upload.
pub(crate) const DEFAULT_TRANSFER115_TARGET_DIR: &str = "转存目录";
/// Default target for 115 cloud download (magnet / ed2k), relative to the
/// archive root. Kept separate from [`DEFAULT_TRANSFER115_TARGET_DIR`] because
/// 115 downloads in the background: mixing both streams in one directory makes
/// "what did the share transfer put here" impossible to answer.
pub(crate) const DEFAULT_TRANSFER115_OFFLINE_DIR: &str = "离线下载";
pub(crate) const DEFAULT_TRANSFER115_TIMEOUT_SECONDS: u64 = 120;
const MAX_TRANSFER115_TIMEOUT_SECONDS: u64 = 600;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct ArchiveSettings {
    /// TMDB v3 API key; empty means "not configured", which disables lookups.
    pub(crate) tmdb_api_key: String,
    /// Extra directories watched for new uploads (any CD2 mount visible here).
    pub(crate) watch_dirs: Vec<String>,
    /// Local mirror the media server scans: strm + NFO + images only.
    pub(crate) mirror_root: String,
    /// Path of the archive root inside the CloudDrive2 mount (`测试库`).
    pub(crate) cd2_prefix: String,
    /// Where the CD2 mount is visible inside the media server container.
    pub(crate) cd2_mount: String,
    /// Auto-run interval; 0 disables the poller.
    pub(crate) poll_seconds: u64,
    /// How long a freshly written entry waits before it is claimed.
    pub(crate) settle_seconds: u64,
    /// Replace an existing entry when the incoming pack is larger than it.
    pub(crate) wash_enabled: bool,
    /// Relative size difference under which two packs count as the same version.
    pub(crate) wash_tolerance_ratio: f64,
    /// Bot and userbot credentials, notification selection and listening mode.
    pub(crate) telegram: TelegramSettings,
    /// The 115 transfer client (U11): web cookie, the fixed target directory
    /// inside the archive root, and where the sidecar listens.
    pub(crate) transfer115: Transfer115Settings,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Transfer115Settings {
    /// Web cookie of the 115 account, the whole string from the browser
    /// (`UID=…; CID=…; SEID=…; KID=…` and whatever else the browser carries).
    /// Masked in every API response; blank in an update keeps the stored value.
    pub(crate) cookie: String,
    /// Fixed target directory for share-link transfers, relative to the archive
    /// root (for example `转存目录`). The sidecar resolves it to a 115 `cid` and
    /// creates it when missing.
    pub(crate) target_dir: String,
    /// Target directory for cloud-download (magnet / ed2k) tasks, relative to the
    /// archive root. 115 downloads in the background, so this is deliberately a
    /// different directory from `target_dir`.
    pub(crate) offline_dir: String,
    /// Base URL of the transfer115 sidecar; empty disables the transfer stage
    /// and jobs stay `needs_115_client`.
    pub(crate) sidecar_base: String,
    /// Request timeout in seconds.
    pub(crate) timeout_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct TelegramSettings {
    /// Bot token of the notification bot (`123:abc`).
    pub(crate) bot_token: String,
    /// Chat that receives notifications (id or `@channel`).
    pub(crate) chat_id: String,
    /// `api_id` of the Telethon userbot application.
    pub(crate) api_id: String,
    /// `api_hash` of the Telethon userbot application.
    pub(crate) api_hash: String,
    /// Phone number the userbot logs in with.
    pub(crate) phone: String,
    /// Shared admin API key the sidecar uses for `X-Lux-Api-Key`. Stored here so
    /// the Telegram sidecar has one place to read it from.
    pub(crate) api_key: String,
    /// Notification categories: array of keys/labels, an object of per-category
    /// switches, or `null` for "everything on". Owned by the notify module.
    pub(crate) notify: Value,
    /// One of [`LISTEN_MODES`] (or its English alias).
    ///
    /// Compatibility projection of [`Self::channels`]: the old single-mode
    /// field is derived on every save so an older sidecar still understands the
    /// file. New code reads and writes `channels`.
    pub(crate) listen_mode: String,
    /// Channels the userbot listens to (`@username` or numeric id).
    ///
    /// Compatibility projection of [`Self::channels`] (enabled channels only).
    pub(crate) listen_channels: Vec<String>,
    /// Per-channel listening configuration: the source of truth since v2.
    ///
    /// Each channel carries its own mode bits and its own cursor state, so a
    /// slow channel no longer holds back a fast one and "追更" can be enabled
    /// for one channel only.
    #[serde(default)]
    pub(crate) channels: Vec<TelegramChannel>,
    /// Series the 追更 mode keeps watching.
    pub(crate) follow_list: Vec<String>,
    // Keys owned by the notify module: kept so a save from this module never
    // drops the deliverability tuning the notify module documents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) api_base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) silent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) min_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_per_minute: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dedupe_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) timezone_offset_hours: Option<i64>,
}

impl Default for ArchiveSettings {
    fn default() -> Self {
        Self {
            tmdb_api_key: String::new(),
            watch_dirs: Vec::new(),
            mirror_root: DEFAULT_MIRROR_ROOT.to_owned(),
            cd2_prefix: String::new(),
            cd2_mount: DEFAULT_CD2_CONTAINER_MOUNT.to_owned(),
            poll_seconds: 0,
            settle_seconds: DEFAULT_SETTLE_SECONDS,
            wash_enabled: true,
            wash_tolerance_ratio: DEFAULT_WASH_TOLERANCE_RATIO,
            telegram: TelegramSettings::default(),
            transfer115: Transfer115Settings::default(),
        }
    }
}

impl Default for Transfer115Settings {
    fn default() -> Self {
        Self {
            cookie: String::new(),
            target_dir: DEFAULT_TRANSFER115_TARGET_DIR.to_owned(),
            offline_dir: DEFAULT_TRANSFER115_OFFLINE_DIR.to_owned(),
            sidecar_base: String::new(),
            timeout_seconds: DEFAULT_TRANSFER115_TIMEOUT_SECONDS,
        }
    }
}

/// Most channels one settings file may carry. Telegram accounts can join
/// hundreds; the cap only stops a runaway UI or a hand-edited file.
const MAX_TELEGRAM_CHANNELS: usize = 500;

/// One Telegram channel the userbot watches.
///
/// `id` is the stable key (`dialog.id`, negative for channels); the title
/// changes and must never be used as identity.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramChannel {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) username: String,
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) modes: TelegramChannelModes,
    #[serde(default)]
    pub(crate) automation: TelegramChannelAutomation,
}

/// The five mode bits of one channel (daquan's `normalizeTelegramChannelModes`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramChannelModes {
    /// 入新: transfer when the library does not have the work at all.
    pub(crate) new_release: bool,
    /// 追更: fill missing seasons/episodes of a subscribed series.
    pub(crate) follow: bool,
    /// 全量: all of 入新 + 追更 + 洗版.
    pub(crate) full: bool,
    /// 洗版: replace an existing entry only when the incoming version wins.
    pub(crate) remux: bool,
    /// 手动: **derived**, true only when the other four are false.
    pub(crate) manual: bool,
}

impl TelegramChannelModes {
    /// `manual` is never an independent switch: it means "nothing automatic".
    fn normalized(mut self) -> Self {
        self.manual = !(self.new_release || self.follow || self.full || self.remux);
        self
    }

    /// Map the legacy single-mode string onto the five bits.
    fn from_listen_mode(mode: &str) -> Self {
        let normalized = mode.trim().to_ascii_lowercase();
        Self {
            new_release: matches!(normalized.as_str(), "上新" | "new"),
            follow: matches!(normalized.as_str(), "追更" | "follow"),
            full: matches!(normalized.as_str(), "全量" | "all"),
            remux: matches!(normalized.as_str(), "洗版" | "wash"),
            manual: false,
        }
        .normalized()
    }

    fn any_automatic(&self) -> bool {
        self.new_release || self.follow || self.full || self.remux
    }
}

/// Per-channel cursor and last-run bookkeeping.
///
/// `last_message_id` is the cursor: messages up to and including it are done.
/// The first run only records the newest id and transfers nothing, so a fresh
/// channel does not dump its whole history into the queue.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramChannelAutomation {
    pub(crate) initialized: bool,
    pub(crate) last_message_id: i64,
    pub(crate) last_run_at: String,
    pub(crate) last_status: String,
    pub(crate) last_error: String,
    pub(crate) transferred_count: i64,
}

impl TelegramSettings {
    /// Build the v2 channel list from the legacy single-mode pair.
    ///
    /// Old configuration carried one global mode for every channel; mapping it
    /// one-to-one keeps the previous behaviour after the upgrade.
    fn channels_from_legacy(ids: &[String], mode: &str) -> Vec<TelegramChannel> {
        let modes = TelegramChannelModes::from_listen_mode(mode);
        ids.iter()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
            .map(|id| TelegramChannel {
                id: id.to_owned(),
                title: String::new(),
                username: id.trim_start_matches('@').to_owned(),
                enabled: true,
                modes: modes.clone(),
                automation: TelegramChannelAutomation::default(),
            })
            .collect()
    }

    /// Keep the legacy projection in sync with `channels`.
    ///
    /// The fields are still written so an older sidecar (which only knows
    /// `listen_channels` + `listen_mode`) keeps working during the transition.
    fn project_legacy(&mut self) {
        self.listen_channels = self
            .channels
            .iter()
            .filter(|channel| channel.enabled)
            .map(|channel| channel.id.clone())
            .collect();
        // With no channels there is nothing to project: keep `listen_mode` as
        // configured so a template written before any channel exists does not
        // silently flip to 手动.
        if self.channels.is_empty() {
            return;
        }
        let any = |pick: fn(&TelegramChannelModes) -> bool| {
            self.channels
                .iter()
                .any(|channel| channel.enabled && pick(&channel.modes))
        };
        self.listen_mode = if any(|modes| modes.full) {
            "全量"
        } else if any(|modes| modes.new_release) {
            "上新"
        } else if any(|modes| modes.follow) {
            "追更"
        } else if any(|modes| modes.remux) {
            "洗版"
        } else {
            "手动"
        }
        .to_owned();
    }

    /// Drop unusable channels, dedupe by id and normalise the mode bits.
    fn sanitize_channels(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.channels = self
            .channels
            .drain(..)
            .filter_map(|mut channel| {
                channel.id = channel.id.trim().to_owned();
                if channel.id.is_empty() || !seen.insert(channel.id.clone()) {
                    return None;
                }
                channel.modes = channel.modes.clone().normalized();
                Some(channel)
            })
            .take(MAX_TELEGRAM_CHANNELS)
            .collect();
        self.project_legacy();
    }
}

impl Default for TelegramSettings {
    fn default() -> Self {
        Self {
            bot_token: String::new(),
            chat_id: String::new(),
            api_id: String::new(),
            api_hash: String::new(),
            phone: String::new(),
            api_key: String::new(),
            notify: Value::Null,
            listen_mode: DEFAULT_LISTEN_MODE.to_owned(),
            listen_channels: Vec::new(),
            channels: Vec::new(),
            follow_list: Vec::new(),
            enabled: None,
            api_base: None,
            message_thread_id: None,
            silent: None,
            min_interval_ms: None,
            max_per_minute: None,
            dedupe_seconds: None,
            timezone_offset_hours: None,
        }
    }
}

/// Set once the first settings read fails, so the poller does not log the same
/// warning every tick.
static SETTINGS_READ_WARNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl ArchiveSettings {
    /// Path of the active settings file (`LUX_ARCHIVE_SETTINGS`, else the
    /// container default).
    pub(crate) fn path() -> PathBuf {
        std::env::var(ARCHIVE_SETTINGS_ENV)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_ARCHIVE_SETTINGS_PATH))
    }

    /// Effective settings: the file over the environment over the defaults.
    /// A missing or unreadable file is not an error; the environment wins then,
    /// which keeps every deployment that never wrote the file working.
    pub(crate) fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub(crate) fn load_from(path: &Path) -> Self {
        Self::load_from_with_base(path, &Self::from_env())
    }

    pub(crate) fn load_from_with_base(path: &Path, base: &Self) -> Self {
        match read_document(path) {
            Ok(Some(document)) => Self::from_document(base, &document).sanitized(),
            Ok(None) => base.clone(),
            Err(error) => {
                // The poller re-reads the settings every tick; warn once, then
                // keep the noise at debug level (D9).
                if SETTINGS_READ_WARNING.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    tracing::debug!(
                        %error,
                        path = %path.display(),
                        "archive settings file still unreadable; using environment values"
                    );
                } else {
                    tracing::warn!(
                        %error,
                        path = %path.display(),
                        "archive settings file could not be read; using environment values"
                    );
                }
                base.clone()
            }
        }
    }

    /// Environment values with the built-in defaults behind them. Only the
    /// archive pipeline fields are covered here; the Telegram keys fall back to
    /// the notify module's own environment values.
    pub(crate) fn from_env() -> Self {
        let mut settings = Self::default();
        if let Some(value) = env_non_empty(ARCHIVE_TMDB_KEY_ENV) {
            settings.tmdb_api_key = value;
        }
        if let Some(value) = std::env::var(ARCHIVE_WATCH_DIRS_ENV).ok() {
            settings.watch_dirs = split_list(&value);
        }
        if let Some(value) = env_non_empty(ARCHIVE_MIRROR_ROOT_ENV) {
            settings.mirror_root = value;
        }
        if let Some(value) = std::env::var(ARCHIVE_CD2_PREFIX_ENV).ok() {
            settings.cd2_prefix = value.trim().trim_matches('/').to_owned();
        }
        if let Some(value) = env_non_empty(ARCHIVE_CD2_MOUNT_ENV) {
            settings.cd2_mount = value;
        }
        if let Some(value) = env_u64(ARCHIVE_POLL_SECONDS_ENV) {
            settings.poll_seconds = value;
        }
        if let Some(value) = env_u64(ARCHIVE_SETTLE_SECONDS_ENV) {
            settings.settle_seconds = value;
        }
        if let Some(value) = env_bool(ARCHIVE_WASH_ENABLED_ENV) {
            settings.wash_enabled = value;
        }
        if let Some(value) = env_f64(ARCHIVE_WASH_TOLERANCE_ENV) {
            settings.wash_tolerance_ratio = value;
        }
        if let Some(value) = env_non_empty(TRANSFER115_COOKIE_ENV) {
            settings.transfer115.cookie = value;
        }
        if let Some(value) = env_non_empty(TRANSFER115_TARGET_DIR_ENV) {
            settings.transfer115.target_dir = value;
        }
        if let Some(value) = env_non_empty(TRANSFER115_SIDECAR_BASE_ENV) {
            settings.transfer115.sidecar_base = value;
        }
        if let Some(value) = env_u64(TRANSFER115_TIMEOUT_ENV) {
            settings.transfer115.timeout_seconds = value;
        }
        settings.sanitized()
    }

    /// Persist to [`Self::path`]. Creates the parent directory and writes the
    /// file atomically with mode 0600.
    pub(crate) fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path())
    }

    pub(crate) fn save_to(&self, path: &Path) -> Result<(), String> {
        self.save_to_with_base(path, &Self::from_env())
    }

    /// Save against an explicit environment base. The tests use this so they do
    /// not have to mutate process-wide `LUX_ARCHIVE_*` values.
    pub(crate) fn save_to_with_base(&self, path: &Path, base: &Self) -> Result<(), String> {
        let existing = read_document(path).ok().flatten();
        let document = self.document_for_save(existing.as_ref(), base)?;
        write_private(path, &document)
    }

    /// The document to persist. Only keys that already live in the file, or whose
    /// value differs from the environment base, are written: a save must never
    /// pin `LUX_ARCHIVE_*` values into the file (they would then shadow later
    /// environment changes), and keys this module does not know are preserved
    /// from the existing file instead of being dropped.
    fn document_for_save(&self, existing: Option<&Value>, base: &Self) -> Result<String, String> {
        let env = serde_json::to_value(base)
            .map_err(|error| format!("无法序列化环境配置: {error}"))?;
        let current = serde_json::to_value(self)
            .map_err(|error| format!("无法序列化归档设置: {error}"))?;
        let existing_object = existing
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let current_object = current.as_object().cloned().unwrap_or_default();
        let env_object = env.as_object().cloned().unwrap_or_default();
        // Start from what the file already holds so keys this module does not
        // know survive the rewrite.
        let mut document = existing_object.clone();
        for (key, value) in &current_object {
            if key == "telegram" || key == "transfer115" {
                continue;
            }
            if key_is_owned(&existing_object, &env_object, key, value) {
                document.insert(key.clone(), value.clone());
            }
        }
        let current_telegram = current_object
            .get("telegram")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let env_telegram = env_object
            .get("telegram")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        // Start from what the file already holds so unknown keys survive.
        let mut telegram = existing_object
            .get("telegram")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for (key, value) in &current_telegram {
            if key_is_owned(&telegram, &env_telegram, key, value) {
                telegram.insert(key.clone(), value.clone());
            }
        }
        if !telegram.is_empty() {
            document.insert("telegram".to_owned(), Value::Object(telegram));
        }
        // The transfer115 section follows the same provenance rule: start from
        // what the file holds, write only owned keys, so the 115 cookie is not
        // frozen in when it merely mirrors `LUX_115_COOKIE`.
        let current_transfer = current_object
            .get("transfer115")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let env_transfer = env_object
            .get("transfer115")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut transfer = existing_object
            .get("transfer115")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for (key, value) in &current_transfer {
            if key_is_owned(&transfer, &env_transfer, key, value) {
                transfer.insert(key.clone(), value.clone());
            }
        }
        if !transfer.is_empty() {
            document.insert("transfer115".to_owned(), Value::Object(transfer));
        }
        let mut text = serde_json::to_string_pretty(&Value::Object(document))
            .map_err(|error| format!("无法序列化归档设置: {error}"))?;
        text.push('\n');
        Ok(text)
    }

    /// A copy safe to return over the API: secrets keep only a short prefix and
    /// their length, everything else is reported as stored.
    pub(crate) fn masked(&self) -> Self {
        let mut masked = self.clone();
        masked.tmdb_api_key = mask_secret(&self.tmdb_api_key);
        masked.telegram.bot_token = mask_secret(&self.telegram.bot_token);
        masked.telegram.chat_id = mask_secret(&self.telegram.chat_id);
        masked.telegram.api_hash = mask_secret(&self.telegram.api_hash);
        masked.telegram.phone = mask_secret(&self.telegram.phone);
        masked.telegram.api_key = mask_secret(&self.telegram.api_key);
        if let Some(api_base) = masked.telegram.api_base.as_mut() {
            *api_base = mask_api_base(api_base);
        }
        masked.transfer115.cookie = mask_secret(&self.transfer115.cookie);
        masked
    }

    /// Apply an update document (the PUT body) on top of the stored values.
    /// Blank secrets -- including a mask echoed back from a GET -- keep the
    /// stored value instead of clearing it.
    pub(crate) fn apply_update(&mut self, update: &Value) -> Result<(), String> {
        let Value::Object(mut document) = update.clone() else {
            return Err("设置内容必须是 JSON 对象".to_owned());
        };
        // A value of the wrong type is an error, not a silent no-op: otherwise a
        // typo in the UI looks like it was saved when nothing changed.
        reject_unparsable_values(&document)?;
        keep_stored_secret(&mut document, "tmdb_api_key", &self.tmdb_api_key);
        if let Some(Value::Object(telegram)) = document.get_mut("telegram") {
            keep_stored_secret(telegram, "bot_token", &self.telegram.bot_token);
            keep_stored_secret(telegram, "chat_id", &self.telegram.chat_id);
            keep_stored_secret(telegram, "api_hash", &self.telegram.api_hash);
            keep_stored_secret(telegram, "phone", &self.telegram.phone);
            keep_stored_secret(telegram, "api_key", &self.telegram.api_key);
        }
        if let Some(Value::Object(transfer)) = document.get_mut("transfer115") {
            keep_stored_secret(transfer, "cookie", &self.transfer115.cookie);
        }
        let merged = Self::from_document(self, &Value::Object(document));
        // Validate the values this update actually sets: an invalid directory in
        // the environment must not make every save fail. Validation runs before
        // the update is committed, so a rejected update leaves the struct as it
        // was.
        merged.validate_changed(update)?;
        *self = merged;
        Ok(())
    }

    /// Reject out-of-range or unusable values for the fields this update sets.
    fn validate_changed(&self, update: &Value) -> Result<(), String> {
        let Some(document) = update.as_object() else {
            return Ok(());
        };
        if document.contains_key("wash_tolerance_ratio")
            && (!self.wash_tolerance_ratio.is_finite()
                || !(0.0..=1.0).contains(&self.wash_tolerance_ratio))
        {
            return Err("洗版容差必须是 0 到 1 之间的数字".to_owned());
        }
        if document.contains_key("poll_seconds") && self.poll_seconds > MAX_POLL_SECONDS {
            return Err(format!("轮询间隔必须在 0 到 {MAX_POLL_SECONDS} 秒之间"));
        }
        if document.contains_key("settle_seconds") && self.settle_seconds > MAX_SETTLE_SECONDS {
            return Err(format!("稳定等待必须在 0 到 {MAX_SETTLE_SECONDS} 秒之间"));
        }
        if document.contains_key("mirror_root") {
            validate_directory("本地镜像库", &self.mirror_root)?;
        }
        if document.contains_key("cd2_mount") {
            let mount = self.cd2_mount.trim();
            if mount.is_empty() {
                return Err("CD2 挂载点不能为空".to_owned());
            }
            if !mount.starts_with('/') {
                return Err(format!("CD2 挂载点必须是绝对路径: {mount}"));
            }
        }
        if document.contains_key("watch_dirs") {
            for directory in &self.watch_dirs {
                validate_directory("监听目录", directory)?;
            }
        }
        if let Some(Value::Object(telegram)) = document.get("telegram")
            && telegram.contains_key("api_base")
        {
            validate_api_base(self.telegram.api_base.as_deref().unwrap_or_default())?;
        }
        if let Some(Value::Object(telegram)) = document.get("telegram")
            && telegram.contains_key("listen_mode")
            && !is_listen_mode(&self.telegram.listen_mode)
        {
            return Err(format!(
                "监听模式必须是 {} 之一",
                LISTEN_MODES.join(" / ")
            ));
        }
        if let Some(Value::Object(telegram)) = document.get("telegram")
            && telegram.contains_key("channels")
        {
            validate_channels(&self.telegram.channels)?;
        }
        if let Some(Value::Object(transfer)) = document.get("transfer115") {
            if transfer.contains_key("timeout_seconds")
                && self.transfer115.timeout_seconds > MAX_TRANSFER115_TIMEOUT_SECONDS
            {
                return Err(format!(
                    "转存超时必须在 0 到 {MAX_TRANSFER115_TIMEOUT_SECONDS} 秒之间"
                ));
            }
            if transfer.contains_key("sidecar_base") {
                validate_sidecar_base(&self.transfer115.sidecar_base)?;
            }
            if transfer.contains_key("target_dir") {
                validate_transfer_target_dir(&self.transfer115.target_dir)?;
            }
            if transfer.contains_key("offline_dir") {
                validate_transfer_target_dir(&self.transfer115.offline_dir)?;
            }
        }
        Ok(())
    }

    /// Reject values that would make the pipeline misbehave. Runtime code
    /// validates the fields an update actually sets (see `validate_changed`);
    /// this full-document check is used by the tests.
    #[cfg(test)]
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !self.wash_tolerance_ratio.is_finite()
            || !(0.0..=1.0).contains(&self.wash_tolerance_ratio)
        {
            return Err("洗版容差必须是 0 到 1 之间的数字".to_owned());
        }
        if self.poll_seconds > MAX_POLL_SECONDS {
            return Err(format!("轮询间隔必须在 0 到 {MAX_POLL_SECONDS} 秒之间"));
        }
        if self.settle_seconds > MAX_SETTLE_SECONDS {
            return Err(format!("稳定等待必须在 0 到 {MAX_SETTLE_SECONDS} 秒之间"));
        }
        validate_directory("本地镜像库", &self.mirror_root)?;
        let mount = self.cd2_mount.trim();
        if mount.is_empty() {
            return Err("CD2 挂载点不能为空".to_owned());
        }
        if !mount.starts_with('/') {
            return Err(format!("CD2 挂载点必须是绝对路径: {mount}"));
        }
        for directory in &self.watch_dirs {
            validate_directory("监听目录", directory)?;
        }
        if !is_listen_mode(&self.telegram.listen_mode) {
            return Err(format!(
                "监听模式必须是 {} 之一",
                LISTEN_MODES.join(" / ")
            ));
        }
        validate_channels(&self.telegram.channels)?;
        if self.transfer115.timeout_seconds > MAX_TRANSFER115_TIMEOUT_SECONDS {
            return Err(format!(
                "转存超时必须在 0 到 {MAX_TRANSFER115_TIMEOUT_SECONDS} 秒之间"
            ));
        }
        validate_sidecar_base(&self.transfer115.sidecar_base)?;
        validate_transfer_target_dir(&self.transfer115.target_dir)?;
        validate_transfer_target_dir(&self.transfer115.offline_dir)?;
        Ok(())
    }

    /// Auto-run interval of the archive poller.
    pub(crate) fn poll_interval(&self) -> Option<Duration> {
        (self.poll_seconds > 0).then(|| Duration::from_secs(self.poll_seconds))
    }

    /// Values that would break the pipeline are clamped when loading: a settle
    /// window longer than an hour would leave entries unclaimed forever, and a
    /// tolerance outside 0..=1 makes every pack look identical.
    fn sanitized(mut self) -> Self {
        self.poll_seconds = if self.poll_seconds > MAX_POLL_SECONDS {
            0
        } else {
            self.poll_seconds
        };
        self.settle_seconds = self.settle_seconds.min(MAX_SETTLE_SECONDS);
        if !self.wash_tolerance_ratio.is_finite() || !(0.0..=1.0).contains(&self.wash_tolerance_ratio)
        {
            self.wash_tolerance_ratio = DEFAULT_WASH_TOLERANCE_RATIO;
        }
        if self.transfer115.timeout_seconds > MAX_TRANSFER115_TIMEOUT_SECONDS {
            self.transfer115.timeout_seconds = DEFAULT_TRANSFER115_TIMEOUT_SECONDS;
        }
        if self.transfer115.target_dir.trim().is_empty() {
            self.transfer115.target_dir = DEFAULT_TRANSFER115_TARGET_DIR.to_owned();
        }
        if self.transfer115.offline_dir.trim().is_empty() {
            self.transfer115.offline_dir = DEFAULT_TRANSFER115_OFFLINE_DIR.to_owned();
        }
        // Channels are the source of truth: drop empty/duplicate ids, fold the
        // derived `manual` bit and refresh the legacy projection.
        self.telegram.sanitize_channels();
        self
    }

    /// Overlay a settings document on `base`. Present keys win (even when the
    /// value is empty), missing or unparsable keys keep the base value.
    fn from_document(base: &Self, document: &Value) -> Self {
        let Value::Object(document) = document else {
            return base.clone();
        };
        let mut settings = base.clone();
        if let Some(value) = string_field(document, "tmdb_api_key") {
            settings.tmdb_api_key = value;
        }
        if let Some(value) = string_list_field(document, "watch_dirs") {
            settings.watch_dirs = value;
        }
        if let Some(value) = string_field(document, "mirror_root") {
            settings.mirror_root = value;
        }
        if let Some(value) = string_field(document, "cd2_prefix") {
            settings.cd2_prefix = value.trim_matches('/').to_owned();
        }
        if let Some(value) = string_field(document, "cd2_mount") {
            settings.cd2_mount = value;
        }
        if let Some(value) = u64_field(document, "poll_seconds") {
            settings.poll_seconds = value;
        }
        if let Some(value) = u64_field(document, "settle_seconds") {
            settings.settle_seconds = value;
        }
        if let Some(value) = bool_field(document, "wash_enabled") {
            settings.wash_enabled = value;
        }
        if let Some(value) = f64_field(document, "wash_tolerance_ratio") {
            settings.wash_tolerance_ratio = value;
        }
        if let Some(Value::Object(telegram)) = document.get("telegram") {
            settings.telegram = TelegramSettings::from_document(&settings.telegram, telegram);
        }
        if let Some(Value::Object(transfer)) = document.get("transfer115") {
            settings.transfer115 =
                Transfer115Settings::from_document(&settings.transfer115, transfer);
        }
        settings
    }
}

impl Transfer115Settings {
    fn from_document(base: &Self, document: &Map<String, Value>) -> Self {
        let mut settings = base.clone();
        if let Some(value) = string_field(document, "cookie") {
            settings.cookie = value;
        }
        if let Some(value) = string_field(document, "target_dir") {
            settings.target_dir = value;
        }
        if let Some(value) = string_field(document, "offline_dir") {
            settings.offline_dir = value;
        }
        if let Some(value) = string_field(document, "sidecar_base") {
            settings.sidecar_base = value;
        }
        if let Some(value) = u64_field(document, "timeout_seconds") {
            settings.timeout_seconds = value;
        }
        settings
    }
}

impl TelegramSettings {
    fn from_document(base: &Self, document: &Map<String, Value>) -> Self {
        let mut settings = base.clone();
        if let Some(value) = string_or_number_field(document, "bot_token") {
            settings.bot_token = value;
        }
        if let Some(value) = string_or_number_field(document, "chat_id") {
            settings.chat_id = value;
        }
        if let Some(value) = string_or_number_field(document, "api_id") {
            settings.api_id = value;
        }
        if let Some(value) = string_field(document, "api_hash") {
            settings.api_hash = value;
        }
        if let Some(value) = string_or_number_field(document, "phone") {
            settings.phone = value;
        }
        if let Some(value) = string_field(document, "api_key") {
            settings.api_key = value;
        }
        if let Some(value) = document.get("notify") {
            settings.notify = value.clone();
        }
        if let Some(value) = string_field(document, "listen_mode") {
            settings.listen_mode = value;
        }
        if let Some(value) = string_list_field(document, "listen_channels") {
            settings.listen_channels = value;
        }
        // v2: `channels` is the source of truth. A document that still speaks
        // the legacy pair (an older UI, or a hand-edited file) is migrated on
        // the spot; `channels` wins when both are present.
        if let Some(value) = document.get("channels") {
            settings.channels = parse_telegram_channels(value);
        } else if document.contains_key("listen_channels") || document.contains_key("listen_mode") {
            // An older UI saves the whole telegram group on every edit, most of
            // the time without touching the listen fields. Migrate only when
            // those values actually changed, so a no-op save cannot wipe the
            // per-channel modes configured through the v2 API.
            let legacy_changed = settings.listen_channels != base.listen_channels
                || settings.listen_mode != base.listen_mode;
            if legacy_changed {
                settings.channels = TelegramSettings::channels_from_legacy(
                    &settings.listen_channels,
                    &settings.listen_mode,
                );
            }
        }
        if let Some(value) = string_list_field(document, "follow_list") {
            settings.follow_list = value;
        }
        if let Some(value) = optional_bool(document, "enabled") {
            settings.enabled = value;
        }
        if let Some(value) = optional_string(document, "api_base") {
            settings.api_base = value;
        }
        if let Some(value) = optional_i64(document, "message_thread_id") {
            settings.message_thread_id = value;
        }
        if let Some(value) = optional_bool(document, "silent") {
            settings.silent = value;
        }
        if let Some(value) = optional_u64(document, "min_interval_ms") {
            settings.min_interval_ms = value;
        }
        if let Some(value) = optional_u64(document, "max_per_minute") {
            settings.max_per_minute = value;
        }
        if let Some(value) = optional_i64(document, "dedupe_seconds") {
            settings.dedupe_seconds = value;
        }
        if let Some(value) = optional_i64(document, "timezone_offset_hours") {
            settings.timezone_offset_hours = value;
        }
        // Whatever the document spoke (channels or the legacy pair), the legacy
        // fields are rewritten as a projection of `channels` so the file never
        // carries two contradictory views.
        settings.project_legacy();
        settings
    }
}

/// `true` when a value must be persisted: it is already in the file, or it
/// differs from the value the environment would provide.
fn key_is_owned(existing: &Map<String, Value>, env: &Map<String, Value>, key: &str, value: &Value) -> bool {
    existing.contains_key(key)
        || env.get(key).map(|environment| environment != value).unwrap_or(true)
}

/// `https://user:pass@host/path?token=x` -> `https://host`. The notify status
/// endpoint uses the same shape so both report one consistent value.
fn mask_api_base(value: &str) -> String {
    let trimmed = value.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return "***".to_owned();
    };
    let rest = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = rest.rsplit('@').next().unwrap_or_default();
    if host.is_empty() {
        return format!("{scheme}://***");
    }
    format!("{scheme}://{host}")
}

/// A value of the wrong type must be reported, not ignored: silently keeping the
/// old value makes a typo look like a successful save.
fn reject_unparsable_values(document: &Map<String, Value>) -> Result<(), String> {
    for (key, label) in [
        ("poll_seconds", "轮询间隔"),
        ("settle_seconds", "稳定等待"),
    ] {
        if document.contains_key(key) && !number_field_ok(document, key, true) {
            return Err(format!("{label}必须是整数"));
        }
    }
    if document.contains_key("wash_tolerance_ratio")
        && !number_field_ok(document, "wash_tolerance_ratio", false)
    {
        return Err("洗版容差必须是数字".to_owned());
    }
    if document.contains_key("wash_enabled") && !bool_field_ok(document, "wash_enabled") {
        return Err("洗版开关必须是 true 或 false".to_owned());
    }
    if let Some(Value::Object(transfer)) = document.get("transfer115") {
        if transfer.contains_key("timeout_seconds") && !number_field_ok(transfer, "timeout_seconds", true) {
            return Err("转存超时必须是整数".to_owned());
        }
    }
    if let Some(Value::Object(telegram)) = document.get("telegram") {
        for (key, label) in [
            ("min_interval_ms", "通知最小间隔"),
            ("max_per_minute", "每分钟上限"),
        ] {
            if telegram.contains_key(key) && !number_field_ok(telegram, key, true) {
                return Err(format!("{label}必须是整数"));
            }
        }
        for (key, label) in [
            ("message_thread_id", "话题 id"),
            ("dedupe_seconds", "去重窗口"),
            ("timezone_offset_hours", "时区偏移"),
        ] {
            if telegram.contains_key(key) && !number_field_ok(telegram, key, false) {
                return Err(format!("{label}必须是整数"));
            }
        }
        for (key, label) in [("enabled", "通知开关"), ("silent", "静默开关")] {
            if telegram.contains_key(key) && !bool_field_ok(telegram, key) {
                return Err(format!("{label}必须是 true 或 false"));
            }
        }
    }
    Ok(())
}

fn number_field_ok(document: &Map<String, Value>, key: &str, unsigned: bool) -> bool {
    match document.get(key) {
        None | Some(Value::Null) => true,
        Some(Value::Number(number)) => {
            if unsigned {
                number.as_u64().is_some()
            } else {
                number.as_i64().is_some() || number.as_f64().is_some()
            }
        }
        Some(Value::String(text)) => {
            if unsigned {
                text.trim().parse::<u64>().is_ok()
            } else {
                text.trim().parse::<f64>().is_ok()
            }
        }
        Some(_) => false,
    }
}

fn bool_field_ok(document: &Map<String, Value>, key: &str) -> bool {
    match document.get(key) {
        None | Some(Value::Null) | Some(Value::Bool(_)) => true,
        Some(Value::String(text)) => bool_from_str(text).is_some(),
        Some(_) => false,
    }
}

/// Secrets are only ever echoed as a prefix plus their length, which is enough
/// for an operator to tell two keys apart without leaking one.
pub(crate) fn mask_secret(value: &str) -> String {
    let length = value.chars().count();
    if length == 0 {
        return String::new();
    }
    if length <= 6 {
        return format!("***…({length})");
    }
    let head: String = value.chars().take(6).collect();
    format!("{head}…({length})")
}

fn is_listen_mode(value: &str) -> bool {
    let value = value.trim();
    LISTEN_MODES.contains(&value) || LISTEN_MODE_ALIASES.contains(&value.to_ascii_lowercase().as_str())
}

/// A channel needs a stable id; duplicates and blanks are a client bug, so they
/// are rejected (400) instead of being silently dropped on write.
fn validate_channels(channels: &[TelegramChannel]) -> Result<(), String> {
    if channels.len() > MAX_TELEGRAM_CHANNELS {
        return Err(format!("频道数量不能超过 {MAX_TELEGRAM_CHANNELS} 个"));
    }
    let mut seen = std::collections::HashSet::new();
    for channel in channels {
        let id = channel.id.trim();
        if id.is_empty() {
            return Err("频道 id 不能为空".to_owned());
        }
        if !seen.insert(id) {
            return Err(format!("频道 {id} 重复"));
        }
    }
    Ok(())
}

/// The notification API base is a plain origin: credentials, paths, queries and
/// fragments are rejected so a pasted URL cannot smuggle a token into the log
/// or into the request path.
fn validate_api_base(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    let lowered = value.to_ascii_lowercase();
    if !(lowered.starts_with("https://") || lowered.starts_with("http://")) {
        return Err("通知 API 地址必须以 http:// 或 https:// 开头".to_owned());
    }
    if value.contains(['?', '#']) {
        return Err("通知 API 地址不能带查询串或锚点（凭据不应出现在 URL 里）".to_owned());
    }
    let rest = value.split_once("://").map(|(_, rest)| rest).unwrap_or_default();
    if rest.trim_matches('/').is_empty() {
        return Err("通知 API 地址缺少主机名".to_owned());
    }
    Ok(())
}

/// The transfer115 sidecar base is a plain origin like the notify one: no
/// path, query or fragment, so credentials cannot be smuggled into a URL that
/// ends up in a log line.
fn validate_sidecar_base(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    let lowered = value.to_ascii_lowercase();
    if !(lowered.starts_with("https://") || lowered.starts_with("http://")) {
        return Err("转存侧车地址必须以 http:// 或 https:// 开头".to_owned());
    }
    if value.contains(['?', '#']) {
        return Err("转存侧车地址不能带查询串或锚点（凭据不应出现在 URL 里）".to_owned());
    }
    let rest = value.split_once("://").map(|(_, rest)| rest).unwrap_or_default();
    if rest.trim_matches('/').is_empty() {
        return Err("转存侧车地址缺少主机名".to_owned());
    }
    Ok(())
}

/// The transfer target is one path segment under the archive root: no
/// absolute path, no traversal, no empty name. The sidecar resolves it to a
/// 115 cid and creates it when missing.
fn validate_transfer_target_dir(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("转存目标目录不能为空".to_owned());
    }
    if value.starts_with('/') || value.contains("/./") || value.contains("/../") {
        return Err(format!("转存目标目录必须是归档根下的相对路径: {value}"));
    }
    if value.split('/').any(|segment| segment.is_empty() || segment == "." || segment == "..") {
        return Err(format!("转存目标目录必须是归档根下的相对路径: {value}"));
    }
    Ok(())
}

fn validate_directory(label: &str, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label}不能为空"));
    }
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(format!("{label}必须是绝对路径: {value}"));
    }
    if !path.is_dir() {
        return Err(format!("{label}不存在或不是目录: {value}"));
    }
    Ok(())
}

/// Drop a secret from the update when it is blank or is the mask a GET just
/// returned, so "leave the password box empty" keeps the stored value.
fn keep_stored_secret(document: &mut Map<String, Value>, key: &str, stored: &str) {
    let unchanged = match document.get(key) {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => {
            let text = text.trim();
            text.is_empty() || (!stored.is_empty() && text == mask_secret(stored))
        }
        Some(_) => true,
    };
    if unchanged {
        document.remove(key);
    }
}

fn read_document(path: &Path) -> Result<Option<Value>, String> {
    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map(Some)
            .map_err(|error| format!("设置文件不是合法 JSON: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("无法读取设置文件: {error}")),
    }
}

/// Distinguishes concurrent writes inside one process so two admins saving at
/// the same time cannot collide on the temporary file name.
static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn write_private(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建设置目录 {}: {error}", parent.display()))?;
    }
    write_with_fallback(path, contents, atomic_write)
        .map_err(|error| format!("无法写入设置文件 {}: {error}", path.display()))
}

/// Atomic writes need a `rename` over the target. When the settings file is a
/// single-file bind mount, `rename` fails with EBUSY/EXDEV and the only way to
/// update it is to truncate and rewrite the file in place. The fallback is not
/// atomic; readers can briefly observe a partial file, but the settings loader
/// treats an unparsable document as "use the environment", so the window is
/// bounded and visible in the log.
fn write_with_fallback(
    path: &Path,
    contents: &str,
    atomic: impl FnOnce(&Path, &str) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match atomic(path, contents) {
        Ok(()) => Ok(()),
        Err(error) if is_bind_mount_error(&error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "atomic settings write failed; writing the bind-mounted file in place"
            );
            write_in_place(path, contents)
        }
        Err(error) => Err(error),
    }
}

fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive-settings.json".to_owned());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or_default();
    let temp = path.with_file_name(format!(
        ".{name}.{}.{}.{nonce}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let written = (|| -> std::io::Result<()> {
        let mut file = create_private(&temp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        // Set the mode on the temporary file so the rename cannot leave a
        // world-readable settings file behind.
        set_private(&temp)?;
        fs::rename(&temp, path)?;
        set_private(path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

fn write_in_place(path: &Path, contents: &str) -> std::io::Result<()> {
    // Not atomic by nature: keep the previous contents so a failed write (or a
    // crash between truncate and write) cannot leave the operator without any
    // settings at all. The backup is private, like the file itself.
    let backup = path.with_extension("json.bak");
    if path.exists() {
        fs::copy(path, &backup)?;
        set_private(&backup)?;
    }
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()
    })();
    if result.is_err() && backup.exists() {
        let _ = fs::copy(&backup, path);
    }
    result?;
    set_private(path)
}

fn is_bind_mount_error(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::CrossesDevices | std::io::ErrorKind::ResourceBusy
    ) {
        return true;
    }
    #[cfg(unix)]
    {
        matches!(error.raw_os_error(), Some(libc::EBUSY) | Some(libc::EXDEV))
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn create_private(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn set_private(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn env_f64(name: &str) -> Option<f64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn env_bool(name: &str) -> Option<bool> {
    bool_from_str(&std::env::var(name).ok()?)
}

fn bool_from_str(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

fn string_field(document: &Map<String, Value>, key: &str) -> Option<String> {
    match document.get(key)? {
        Value::String(text) => Some(text.trim().to_owned()),
        _ => None,
    }
}

fn optional_string(document: &Map<String, Value>, key: &str) -> Option<Option<String>> {
    match document.get(key)? {
        Value::Null => Some(None),
        Value::String(text) => {
            let text = text.trim();
            Some((!text.is_empty()).then(|| text.to_owned()))
        }
        _ => None,
    }
}

/// `chat_id`, `api_id` and `phone` are written as strings by the UI but may be
/// bare numbers in a hand-edited file.
fn string_or_number_field(document: &Map<String, Value>, key: &str) -> Option<String> {
    match document.get(key)? {
        Value::String(text) => Some(text.trim().to_owned()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn string_list_field(document: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    match document.get(key)? {
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(|item| match item {
                    Value::String(text) => {
                        let text = text.trim();
                        (!text.is_empty()).then(|| text.to_owned())
                    }
                    Value::Number(number) => Some(number.to_string()),
                    _ => None,
                })
                .collect(),
        ),
        Value::String(text) => Some(split_list(text)),
        _ => None,
    }
}

fn u64_field(document: &Map<String, Value>, key: &str) -> Option<u64> {
    match document.get(key)? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Parse the v2 `channels` array. Anything unusable is dropped here; the
/// sanitizer dedupes and normalises afterwards.
fn parse_telegram_channels(value: &Value) -> Vec<TelegramChannel> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(parse_telegram_channel).collect())
        .unwrap_or_default()
}

fn parse_telegram_channel(value: &Value) -> Option<TelegramChannel> {
    let object = value.as_object()?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if id.is_empty() {
        return None;
    }
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let modes = object
        .get("modes")
        .and_then(Value::as_object)
        .map(|modes| {
            let flag = |key: &str| {
                modes
                    .get(key)
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            };
            TelegramChannelModes {
                new_release: flag("newRelease"),
                follow: flag("follow"),
                full: flag("full"),
                remux: flag("remux"),
                manual: false,
            }
            .normalized()
        })
        .unwrap_or_else(|| TelegramChannelModes::default().normalized());
    let automation = object
        .get("automation")
        .and_then(Value::as_object)
        .map(|automation| TelegramChannelAutomation {
            initialized: automation
                .get("initialized")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            last_message_id: automation
                .get("lastMessageId")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            last_run_at: automation
                .get("lastRunAt")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            last_status: automation
                .get("lastStatus")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            last_error: automation
                .get("lastError")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            transferred_count: automation
                .get("transferredCount")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        })
        .unwrap_or_default();
    Some(TelegramChannel {
        id,
        title: text("title"),
        username: text("username"),
        enabled: object
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        modes,
        automation,
    })
}

fn f64_field(document: &Map<String, Value>, key: &str) -> Option<f64> {
    match document.get(key)? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn bool_field(document: &Map<String, Value>, key: &str) -> Option<bool> {
    match document.get(key)? {
        Value::Bool(value) => Some(*value),
        Value::Number(number) => number.as_i64().map(|value| value != 0),
        Value::String(text) => bool_from_str(text),
        _ => None,
    }
}

fn optional_bool(document: &Map<String, Value>, key: &str) -> Option<Option<bool>> {
    match document.get(key)? {
        Value::Null => Some(None),
        _ => bool_field(document, key).map(Some),
    }
}

fn optional_u64(document: &Map<String, Value>, key: &str) -> Option<Option<u64>> {
    match document.get(key)? {
        Value::Null => Some(None),
        _ => u64_field(document, key).map(Some),
    }
}

fn optional_i64(document: &Map<String, Value>, key: &str) -> Option<Option<i64>> {
    match document.get(key)? {
        Value::Null => Some(None),
        Value::Number(number) => number.as_i64().map(Some),
        Value::String(text) => Some(text.trim().parse().ok()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn settings_from(document: serde_json::Value) -> ArchiveSettings {
        ArchiveSettings::from_document(&ArchiveSettings::default(), &document)
    }

    /// v2 migration: an old file carries one global mode for every channel.
    #[test]
    fn legacy_listen_settings_migrate_to_per_channel_modes() {
        let settings = settings_from(json!({
            "telegram": {
                "listen_channels": ["@aiying", "-1001234567890"],
                "listen_mode": "全量",
            }
        }));

        assert_eq!(settings.telegram.channels.len(), 2);
        let first = &settings.telegram.channels[0];
        assert_eq!(first.id, "@aiying");
        assert_eq!(first.username, "aiying");
        assert!(first.enabled);
        assert!(first.modes.full);
        assert!(!first.modes.manual);
        // The projection keeps the legacy pair readable for an older sidecar.
        assert_eq!(settings.telegram.listen_mode, "全量");
        assert_eq!(settings.telegram.listen_channels.len(), 2);
    }

    /// Per-channel document is the source of truth and wins over the legacy pair.
    #[test]
    fn per_channel_modes_are_preserved_and_project_back() {
        let settings = settings_from(json!({
            "telegram": {
                "listen_channels": ["@ignored"],
                "listen_mode": "上新",
                "channels": [
                    {"id": "-1001", "title": "爱影", "enabled": true,
                     "modes": {"newRelease": true, "follow": true}},
                    {"id": "-1002", "title": "备用", "enabled": false,
                     "modes": {"full": true},
                     "automation": {"initialized": true, "lastMessageId": 42}},
                ],
            }
        }));

        assert_eq!(settings.telegram.channels.len(), 2);
        let active = &settings.telegram.channels[0];
        assert!(active.modes.new_release && active.modes.follow);
        assert!(!active.modes.full && !active.modes.remux && !active.modes.manual);
        let disabled = &settings.telegram.channels[1];
        assert!(!disabled.enabled);
        assert!(disabled.modes.full);
        assert_eq!(disabled.automation.last_message_id, 42);

        // Only enabled channels are projected into the legacy list.
        assert_eq!(settings.telegram.listen_channels, vec!["-1001".to_owned()]);
        assert_eq!(settings.telegram.listen_mode, "上新");
    }

    /// A no-op save from the older UI must not wipe per-channel modes.
    #[test]
    fn unchanged_legacy_fields_keep_the_configured_channels() {
        let v2 = settings_from(json!({
            "telegram": {"channels": [
                {"id": "-1001", "modes": {"newRelease": true}},
                {"id": "-1002", "modes": {"follow": true}},
            ]}
        }));
        assert_eq!(v2.telegram.channels.len(), 2);

        // The old UI re-saves the whole group with the projected values.
        let round_trip = ArchiveSettings::from_document(
            &v2,
            &json!({
                "telegram": {
                    "listen_channels": v2.telegram.listen_channels,
                    "listen_mode": v2.telegram.listen_mode,
                    "bot_token": "123:abc",
                }
            }),
        );
        assert_eq!(round_trip.telegram.channels.len(), 2);
        assert!(round_trip.telegram.channels[1].modes.follow);
        assert_eq!(round_trip.telegram.bot_token, "123:abc");
    }

    /// `manual` is derived: all four automatic bits off means manual on.
    #[test]
    fn manual_is_derived_from_the_other_four_bits() {
        let settings = settings_from(json!({
            "telegram": {"channels": [{"id": "-1", "modes": {}}]}
        }));
        let modes = &settings.telegram.channels[0].modes;
        assert!(modes.manual);
        assert!(!modes.any_automatic());
    }

    /// Duplicate or blank ids are a client bug: reject instead of silently
    /// dropping a channel the operator thinks they configured.
    #[test]
    fn blank_and_duplicate_channel_ids_are_rejected() {
        let settings = settings_from(json!({
            "telegram": {"channels": [{"id": "-1"}, {"id": "-1"}]}
        }));
        let error = validate_channels(&settings.telegram.channels).expect_err("duplicate");
        assert!(error.contains("重复"), "{error}");

        assert!(validate_channels(&[TelegramChannel {
            id: "  ".to_owned(),
            ..Default::default()
        }])
        .is_err());
    }

    fn temporary_directory(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("lux-archive-settings-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir");
        path
    }

    #[test]
    fn masks_secrets_as_prefix_and_length() {
        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("abc"), "***…(3)");
        assert_eq!(mask_secret("123456"), "***…(6)");
        assert_eq!(mask_secret("1234567"), "123456…(7)");

        let settings = ArchiveSettings {
            tmdb_api_key: "abcdefghij".to_owned(),
            telegram: TelegramSettings {
                bot_token: "123456789:AAH".to_owned(),
                chat_id: "5867802360".to_owned(),
                api_hash: "0123456789abcdef".to_owned(),
                phone: "+8613800000000".to_owned(),
                ..TelegramSettings::default()
            },
            ..ArchiveSettings::default()
        };
        let masked = settings.masked();
        assert_eq!(masked.tmdb_api_key, "abcdef…(10)");
        assert_eq!(masked.telegram.bot_token, "123456…(13)");
        assert_eq!(masked.telegram.chat_id, "586780…(10)");
        assert_eq!(masked.telegram.api_hash, "012345…(16)");
        // The full secret never appears in the masked copy.
        let rendered = serde_json::to_string(&masked).expect("masked json");
        assert!(!rendered.contains("abcdefghij"));
        assert!(!rendered.contains("5867802360"));
        assert!(!rendered.contains("+8613800000000"));
    }

    #[test]
    fn overlays_a_document_on_the_defaults() {
        let base = ArchiveSettings::default();
        let document = json!({
            "tmdb_api_key": "key",
            "watch_dirs": ["/testlib/外部上传", " /library/上传 "],
            "mirror_root": "/library",
            "cd2_prefix": "/测试库/",
            "cd2_mount": "/media/cloud",
            "poll_seconds": 10,
            "settle_seconds": "7",
            "wash_enabled": "false",
            "wash_tolerance_ratio": "0.05",
            "telegram": {
                "bot_token": "123:abc",
                "chat_id": 5867802360_i64,
                "api_id": 1234567,
                "api_hash": "hash",
                "phone": 8613800000000_i64,
                "notify": ["入库", "失败"],
                "listen_mode": "上新",
                "listen_channels": ["@channel"],
                "follow_list": "繁花, 庆余年",
                "min_interval_ms": 900
            }
        });
        let settings = ArchiveSettings::from_document(&base, &document);
        assert_eq!(settings.tmdb_api_key, "key");
        assert_eq!(settings.watch_dirs, vec!["/testlib/外部上传", "/library/上传"]);
        assert_eq!(settings.mirror_root, "/library");
        assert_eq!(settings.cd2_prefix, "测试库");
        assert_eq!(settings.poll_seconds, 10);
        assert_eq!(settings.settle_seconds, 7);
        assert!(!settings.wash_enabled);
        assert_eq!(settings.wash_tolerance_ratio, 0.05);
        assert_eq!(settings.telegram.bot_token, "123:abc");
        assert_eq!(settings.telegram.chat_id, "5867802360");
        assert_eq!(settings.telegram.api_id, "1234567");
        assert_eq!(settings.telegram.phone, "8613800000000");
        assert_eq!(settings.telegram.notify, json!(["入库", "失败"]));
        assert_eq!(settings.telegram.listen_mode, "上新");
        assert_eq!(settings.telegram.listen_channels, vec!["@channel"]);
        assert_eq!(settings.telegram.follow_list, vec!["繁花", "庆余年"]);
        assert_eq!(settings.telegram.min_interval_ms, Some(900));
    }

    #[test]
    fn parses_the_documented_template_shape() {
        // Same shape as the operator-facing template: every value is a string
        // (api_id and chat_id included), the notify list is Chinese labels and
        // exactly one listen mode is selected.
        let document = json!({
            "tmdb_api_key": "0123456789abcdef0123456789abcdef",
            "watch_dirs": ["/testlib/外部上传"],
            "mirror_root": "/library",
            "cd2_prefix": "测试库",
            "cd2_mount": "/media/cloud",
            "poll_seconds": 10,
            "settle_seconds": 3,
            "wash_enabled": true,
            "wash_tolerance_ratio": 0.02,
            "telegram": {
                "bot_token": "123456789:AAH-not-a-real-token",
                "chat_id": "5867802360",
                "api_id": "1234567",
                "api_hash": "0123456789abcdef0123456789abcdef",
                "phone": "+8613800000000",
                "notify": ["入库"],
                "listen_mode": "上新",
                "listen_channels": [],
                "follow_list": []
            },
            "transfer115": {
                "cookie": "UID=1; CID=2; SEID=3; KID=4",
                "target_dir": "转存目录",
                "sidecar_base": "http://transfer115:8098",
                "timeout_seconds": "120"
            }
        });
        let settings = ArchiveSettings::from_document(&ArchiveSettings::default(), &document);
        assert_eq!(settings.telegram.api_id, "1234567");
        assert_eq!(settings.telegram.chat_id, "5867802360");
        assert_eq!(settings.telegram.notify, json!(["入库"]));
        assert_eq!(settings.telegram.listen_mode, "上新");
        assert_eq!(settings.poll_seconds, 10);
        assert_eq!(settings.settle_seconds, 3);
        assert!(settings.telegram.listen_channels.is_empty());
        assert_eq!(settings.transfer115.cookie, "UID=1; CID=2; SEID=3; KID=4");
        assert_eq!(settings.transfer115.sidecar_base, "http://transfer115:8098");
        assert_eq!(settings.transfer115.timeout_seconds, 120);
        // Everything the template sets survives a save/reload cycle.
        let saved = serde_json::to_string(&settings).expect("json");
        let reloaded = ArchiveSettings::from_document(
            &ArchiveSettings::default(),
            &serde_json::from_str(&saved).expect("parsed"),
        );
        assert_eq!(reloaded, settings);
    }

    #[test]
    fn unparsable_values_keep_the_base_instead_of_wiping_the_file() {
        let base = ArchiveSettings {
            tmdb_api_key: "keep".to_owned(),
            ..ArchiveSettings::default()
        };
        let settings = ArchiveSettings::from_document(
            &base,
            &json!({
                "tmdb_api_key": 42,
                "poll_seconds": "later",
                "wash_enabled": { "nested": true },
                "watch_dirs": 7,
                "telegram": "not an object",
                "transfer115": "not an object"
            }),
        );
        assert_eq!(settings.tmdb_api_key, "keep");
        assert_eq!(settings.poll_seconds, 0);
        assert!(settings.wash_enabled);
        assert!(settings.watch_dirs.is_empty());
        assert_eq!(settings.telegram.listen_mode, DEFAULT_LISTEN_MODE);
        assert_eq!(settings.transfer115.target_dir, DEFAULT_TRANSFER115_TARGET_DIR);
    }

    #[test]
    fn sanitizes_out_of_range_values_on_load() {
        let settings = ArchiveSettings::from_document(
            &ArchiveSettings::default(),
            &json!({
                "poll_seconds": 999_999,
                "settle_seconds": 999_999,
                "wash_tolerance_ratio": 4.0,
                "transfer115": { "timeout_seconds": 999_999, "target_dir": "  " }
            }),
        )
        .sanitized();
        assert_eq!(settings.poll_seconds, 0);
        assert_eq!(settings.settle_seconds, MAX_SETTLE_SECONDS);
        assert_eq!(settings.wash_tolerance_ratio, DEFAULT_WASH_TOLERANCE_RATIO);
        assert_eq!(settings.transfer115.timeout_seconds, DEFAULT_TRANSFER115_TIMEOUT_SECONDS);
        assert_eq!(settings.transfer115.target_dir, DEFAULT_TRANSFER115_TARGET_DIR);
        assert!(settings.poll_interval().is_none());
    }

    #[test]
    fn saves_a_private_file_and_round_trips_it() {
        let directory = temporary_directory("save");
        let path = directory.join("config").join("archive-settings.json");
        let settings = ArchiveSettings {
            tmdb_api_key: "write-me".to_owned(),
            watch_dirs: vec![directory.display().to_string()],
            poll_seconds: 15,
            telegram: TelegramSettings {
                bot_token: "123:abc".to_owned(),
                notify: json!(["入库"]),
                min_interval_ms: Some(900),
                ..TelegramSettings::default()
            },
            transfer115: Transfer115Settings {
                cookie: "UID=1; CID=2; SEID=3; KID=4".to_owned(),
                sidecar_base: "http://transfer115:8098".to_owned(),
                ..Transfer115Settings::default()
            },
            ..ArchiveSettings::default()
        };
        settings.save_to(&path).expect("saved");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let loaded = ArchiveSettings::load_from_with_base(&path, &ArchiveSettings::default());
        assert_eq!(loaded, settings);

        // A rewrite keeps the same permissions and leaves no temporary file.
        settings.save_to(&path).expect("rewritten");
        let entries: Vec<String> = fs::read_dir(path.parent().expect("parent"))
            .expect("listing")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries, vec!["archive-settings.json".to_owned()]);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn env_values_are_not_pinned_into_the_file() {
        let directory = temporary_directory("provenance");
        let path = directory.join("archive-settings.json");
        let base = ArchiveSettings {
            watch_dirs: vec![directory.display().to_string()],
            transfer115: Transfer115Settings {
                cookie: "env-cookie".to_owned(),
                ..Transfer115Settings::default()
            },
            ..ArchiveSettings::default()
        };
        let settings = ArchiveSettings::from_document(&base, &json!({ "poll_seconds": 30 }));
        settings
            .save_to_with_base(&path, &base)
            .expect("saved with provenance");
        let written: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("json");
        // The value the admin changed is persisted ...
        assert_eq!(written["poll_seconds"], 30);
        // ... but the environment-provided watch directory is not frozen.
        assert!(
            written.get("watch_dirs").is_none(),
            "environment value was pinned: {written}"
        );
        // The same provenance rule covers the transfer115 section: an
        // environment-only cookie must not be frozen into the file.
        assert!(
            written
                .get("transfer115")
                .and_then(|value| value.get("cookie"))
                .is_none(),
            "environment cookie was pinned: {written}"
        );
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_save_preserves_keys_this_module_does_not_know() {
        let directory = temporary_directory("unknown-keys");
        let path = directory.join("archive-settings.json");
        fs::write(
            &path,
            r#"{"telegram":{"api_key":"sidecar-key"},"transfer115":{"cookie":"stored-115-cookie","future_115_key":{"a":1}},"future_key":{"a":1}}"#,
        )
        .expect("seed");
        let mut settings = ArchiveSettings::load_from_with_base(&path, &ArchiveSettings::default());
        settings
            .apply_update(&json!({ "poll_seconds": 5 }))
            .expect("update");
        settings
            .save_to_with_base(&path, &ArchiveSettings::default())
            .expect("saved");
        let written: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(written["poll_seconds"], 5);
        assert_eq!(written["telegram"]["api_key"], "sidecar-key");
        assert_eq!(written["transfer115"]["cookie"], "stored-115-cookie");
        assert_eq!(written["transfer115"]["future_115_key"]["a"], 1);
        assert_eq!(written["future_key"]["a"], 1);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_sidecar_api_key_is_masked_and_kept() {
        let mut settings = ArchiveSettings::default();
        settings
            .apply_update(&json!({ "telegram": { "api_key": "abcd1234efgh" } }))
            .expect("update");
        assert_eq!(settings.telegram.api_key, "abcd1234efgh");
        let masked = settings.masked();
        assert_eq!(masked.telegram.api_key, "abcd12…(12)");
        assert!(!serde_json::to_string(&masked)
            .expect("json")
            .contains("abcd1234efgh"));
        // Echoing the mask back keeps the stored key instead of clearing it.
        settings
            .apply_update(&json!({ "telegram": { "api_key": mask_secret("abcd1234efgh") } }))
            .expect("update");
        assert_eq!(settings.telegram.api_key, "abcd1234efgh");
    }

    /// The 115 web cookie is a secret like the others: masked in responses,
    /// blank or echoed-mask updates keep the stored value.
    #[test]
    fn the_115_cookie_is_masked_and_kept() {
        let mut settings = ArchiveSettings::default();
        settings
            .apply_update(&json!({ "transfer115": { "cookie": "UID=1; CID=2; SEID=3; KID=4" } }))
            .expect("update");
        assert_eq!(settings.transfer115.cookie, "UID=1; CID=2; SEID=3; KID=4");
        let masked = settings.masked();
        assert_eq!(masked.transfer115.cookie, "UID=1;…(27)");
        let rendered = serde_json::to_string(&masked).expect("json");
        assert!(!rendered.contains("SEID=3"));
        assert!(!rendered.contains("KID=4"));
        // Echoing the mask back, or sending a blank, keeps the stored cookie.
        settings
            .apply_update(&json!({ "transfer115": { "cookie": mask_secret("UID=1; CID=2; SEID=3; KID=4") } }))
            .expect("update");
        settings
            .apply_update(&json!({ "transfer115": { "cookie": "" } }))
            .expect("update");
        assert_eq!(settings.transfer115.cookie, "UID=1; CID=2; SEID=3; KID=4");
        // A real new cookie replaces the stored one.
        settings
            .apply_update(&json!({ "transfer115": { "cookie": "UID=9; CID=9" } }))
            .expect("update");
        assert_eq!(settings.transfer115.cookie, "UID=9; CID=9");
    }

    /// The transfer stage is off until the sidecar base is configured; a PUT
    /// that sets it must be a plain origin, and the target directory must stay
    /// under the archive root.
    #[test]
    fn the_transfer_stage_is_off_until_the_sidecar_is_configured() {
        let mut settings = ArchiveSettings::default();
        assert!(settings.transfer115.sidecar_base.is_empty());
        for bad in ["ftp://x", "http://", "http://h/?token=1", "http://h/#x"] {
            assert!(
                settings
                    .apply_update(&json!({ "transfer115": { "sidecar_base": bad } }))
                    .is_err(),
                "accepted {bad}"
            );
        }
        for bad in ["/转存目录", "转存目录/../逃逸", "转存目录/./x", ""] {
            assert!(
                settings
                    .apply_update(&json!({ "transfer115": { "target_dir": bad } }))
                    .is_err(),
                "accepted {bad:?}"
            );
        }
        assert!(settings
            .apply_update(&json!({ "transfer115": { "timeout_seconds": 3_600 } }))
            .expect_err("timeout")
            .contains("转存超时"));
        assert!(settings
            .apply_update(&json!({ "transfer115": { "timeout_seconds": "later" } }))
            .is_err());
        settings
            .apply_update(&json!({
                "transfer115": {
                    "sidecar_base": "http://transfer115:8098",
                    "target_dir": "转存目录",
                    "timeout_seconds": 90
                }
            }))
            .expect("update");
        assert_eq!(settings.transfer115.sidecar_base, "http://transfer115:8098");
        assert_eq!(settings.transfer115.timeout_seconds, 90);
    }

    #[test]
    fn an_update_rejects_out_of_range_and_unparsable_values() {
        let mut settings = ArchiveSettings::default();
        assert!(settings
            .apply_update(&json!({ "poll_seconds": 7_200 }))
            .expect_err("poll")
            .contains("轮询"));
        assert!(settings
            .apply_update(&json!({ "wash_tolerance_ratio": 4.0 }))
            .expect_err("tolerance")
            .contains("容差"));
        assert!(settings
            .apply_update(&json!({ "poll_seconds": "later" }))
            .expect_err("type")
            .contains("整数"));
        assert!(settings
            .apply_update(&json!({ "telegram": { "min_interval_ms": "soon" } }))
            .is_err());
        // Nothing was written into the in-memory settings by the rejected updates.
        assert_eq!(settings.poll_seconds, 0);
        assert_eq!(settings.wash_tolerance_ratio, DEFAULT_WASH_TOLERANCE_RATIO);
    }

    #[test]
    fn api_base_urls_cannot_smuggle_a_token() {
        let mut settings = ArchiveSettings::default();
        for bad in ["ftp://x", "https://", "https://api.telegram.org/?token=1", "https://h/#x"] {
            assert!(
                settings
                    .apply_update(&json!({ "telegram": { "api_base": bad } }))
                    .is_err(),
                "accepted {bad}"
            );
        }
        settings
            .apply_update(&json!({ "telegram": { "api_base": "https://tg.example:8443" } }))
            .expect("plain origin is fine");
        assert_eq!(
            settings.telegram.api_base.as_deref(),
            Some("https://tg.example:8443")
        );
    }

    #[test]
    fn an_update_only_validates_the_paths_it_sets() {
        // A broken directory in the environment must not block an unrelated save.
        let mut settings = ArchiveSettings {
            watch_dirs: vec!["/definitely/not/here".to_owned()],
            ..ArchiveSettings::default()
        };
        settings
            .apply_update(&json!({ "poll_seconds": 5 }))
            .expect("unrelated update passes");
        let mut bad = ArchiveSettings::default();
        assert!(bad
            .apply_update(&json!({ "watch_dirs": ["/definitely/not/here"] }))
            .is_err());
    }

    #[test]
    fn bind_mount_writes_fall_back_to_in_place() {
        let directory = temporary_directory("bind-mount");
        let path = directory.join("archive-settings.json");
        fs::write(&path, "old").expect("seed");
        let result = write_with_fallback(&path, "{\"new\":true}", |_, _| {
            Err(std::io::Error::from_raw_os_error(libc::EBUSY))
        });
        assert!(result.is_ok(), "fallback failed: {result:?}");
        assert_eq!(fs::read_to_string(&path).expect("read"), "{\"new\":true}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn write_failures_other_than_bind_mounts_are_propagated() {
        let directory = temporary_directory("write-error");
        let path = directory.join("archive-settings.json");
        let result = write_with_fallback(&path, "x", |_, _| {
            Err(std::io::Error::from_raw_os_error(libc::EACCES))
        });
        assert!(result.is_err());
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn bind_mount_errors_are_recognised() {
        assert!(is_bind_mount_error(&std::io::Error::from_raw_os_error(
            libc::EBUSY
        )));
        assert!(is_bind_mount_error(&std::io::Error::from_raw_os_error(
            libc::EXDEV
        )));
        assert!(!is_bind_mount_error(&std::io::Error::from_raw_os_error(
            libc::EACCES
        )));
    }

    /// End-to-end check for F2: run inside a container that bind-mounts a
    /// single host file onto `LUX_BIND_MOUNT_TEST_PATH`, e.g.
    /// `docker run … -e LUX_BIND_MOUNT_TEST_PATH=/tmp/archive-settings.json
    ///  -v /host/archive-settings.json:/tmp/archive-settings.json …
    ///  cargo test --lib bind_mount_save_end_to_end -- --ignored`.
    #[test]
    #[ignore = "requires a single-file bind mount and LUX_BIND_MOUNT_TEST_PATH"]
    fn bind_mount_save_end_to_end() {
        let Ok(raw) = std::env::var("LUX_BIND_MOUNT_TEST_PATH") else {
            return;
        };
        let path = PathBuf::from(raw);
        let settings = ArchiveSettings {
            tmdb_api_key: "bind-mount-test".to_owned(),
            poll_seconds: 42,
            ..ArchiveSettings::default()
        };
        settings
            .save_to(&path)
            .expect("save over single-file bind mount");
        let loaded = ArchiveSettings::load_from_with_base(&path, &ArchiveSettings::default());
        assert_eq!(loaded.tmdb_api_key, "bind-mount-test");
        assert_eq!(loaded.poll_seconds, 42);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn a_missing_file_keeps_the_environment_base() {
        let directory = temporary_directory("missing");
        let base = ArchiveSettings {
            mirror_root: "/from-env".to_owned(),
            poll_seconds: 10,
            telegram: TelegramSettings {
                bot_token: "env-token".to_owned(),
                ..TelegramSettings::default()
            },
            ..ArchiveSettings::default()
        };
        let loaded =
            ArchiveSettings::load_from_with_base(&directory.join("absent.json"), &base.clone());
        assert_eq!(loaded, base);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn blank_or_echoed_secrets_keep_the_stored_value() {
        let mut settings = ArchiveSettings {
            tmdb_api_key: "secret-key".to_owned(),
            telegram: TelegramSettings {
                bot_token: "123:token".to_owned(),
                chat_id: "5867802360".to_owned(),
                api_hash: "stored-hash".to_owned(),
                phone: "+8613800000000".to_owned(),
                ..TelegramSettings::default()
            },
            ..ArchiveSettings::default()
        };
        settings
            .apply_update(&json!({
                "tmdb_api_key": "",
                "telegram": {
                    "bot_token": "",
                    "chat_id": mask_secret("5867802360"),
                    "api_hash": null,
                    "phone": ""
                }
            }))
            .expect("update");
        assert_eq!(settings.tmdb_api_key, "secret-key");
        assert_eq!(settings.telegram.bot_token, "123:token");
        assert_eq!(settings.telegram.chat_id, "5867802360");
        assert_eq!(settings.telegram.api_hash, "stored-hash");
        assert_eq!(settings.telegram.phone, "+8613800000000");

        // A real new secret still replaces the stored one.
        settings
            .apply_update(&json!({
                "tmdb_api_key": "next-key",
                "telegram": { "bot_token": "999:new" }
            }))
            .expect("update");
        assert_eq!(settings.tmdb_api_key, "next-key");
        assert_eq!(settings.telegram.bot_token, "999:new");
    }

    #[test]
    fn an_update_keeps_the_notification_tuning_keys() {
        let document = json!({
            "telegram": {
                "enabled": false,
                "api_base": "https://tg.example",
                "message_thread_id": 7,
                "silent": true,
                "min_interval_ms": 900,
                "max_per_minute": 12,
                "dedupe_seconds": 30,
                "timezone_offset_hours": 9
            }
        });
        let mut settings =
            ArchiveSettings::from_document(&ArchiveSettings::default(), &document);
        settings
            .apply_update(&json!({ "telegram": { "notify": ["入库"] } }))
            .expect("update");
        let written = serde_json::to_value(&settings).expect("json");
        let telegram = &written["telegram"];
        assert_eq!(telegram["enabled"], false);
        assert_eq!(telegram["api_base"], "https://tg.example");
        assert_eq!(telegram["message_thread_id"], 7);
        assert_eq!(telegram["silent"], true);
        assert_eq!(telegram["min_interval_ms"], 900);
        assert_eq!(telegram["max_per_minute"], 12);
        assert_eq!(telegram["dedupe_seconds"], 30);
        assert_eq!(telegram["timezone_offset_hours"], 9);
        assert_eq!(telegram["notify"], json!(["入库"]));
    }

    #[test]
    fn rejects_out_of_range_values_and_missing_paths() {
        let directory = temporary_directory("validate");
        let mut settings = ArchiveSettings {
            mirror_root: directory.display().to_string(),
            ..ArchiveSettings::default()
        };

        settings.wash_tolerance_ratio = 1.5;
        assert!(settings.validate().expect_err("tolerance").contains("容差"));
        settings.wash_tolerance_ratio = f64::NAN;
        assert!(settings.validate().is_err());
        settings.wash_tolerance_ratio = 0.02;

        settings.poll_seconds = 7_200;
        assert!(settings.validate().expect_err("poll").contains("轮询"));
        settings.poll_seconds = 10;

        settings.settle_seconds = 7_200;
        assert!(settings.validate().expect_err("settle").contains("稳定等待"));
        settings.settle_seconds = 5;

        settings.mirror_root = directory.join("missing").display().to_string();
        assert!(settings.validate().expect_err("mirror").contains("镜像库"));
        settings.mirror_root = "relative/path".to_owned();
        assert!(settings.validate().expect_err("relative").contains("绝对路径"));
        settings.mirror_root = directory.display().to_string();

        settings.watch_dirs = vec![directory.join("gone").display().to_string()];
        assert!(settings.validate().expect_err("watch").contains("监听目录"));
        settings.watch_dirs = vec![directory.display().to_string()];

        settings.telegram.listen_mode = "边看边下".to_owned();
        assert!(settings.validate().expect_err("mode").contains("监听模式"));
        settings.telegram.listen_mode = "manual".to_owned();
        assert!(settings.validate().is_ok());

        settings.transfer115.timeout_seconds = 3_600;
        assert!(settings.validate().expect_err("timeout").contains("转存超时"));
        settings.transfer115.timeout_seconds = 120;
        settings.transfer115.sidecar_base = "ftp://x".to_owned();
        assert!(settings.validate().expect_err("sidecar").contains("转存侧车"));
        settings.transfer115.sidecar_base = String::new();
        settings.transfer115.target_dir = "/转存目录".to_owned();
        assert!(settings.validate().expect_err("target").contains("转存目标目录"));
        settings.transfer115.target_dir = DEFAULT_TRANSFER115_TARGET_DIR.to_owned();

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn poll_interval_follows_the_configured_seconds() {
        let mut settings = ArchiveSettings::default();
        assert!(settings.poll_interval().is_none());
        settings.poll_seconds = 10;
        assert_eq!(settings.poll_interval(), Some(Duration::from_secs(10)));
    }

    #[test]
    fn update_rejects_a_non_object_body() {
        let mut settings = ArchiveSettings::default();
        assert!(settings.apply_update(&json!(["not", "an", "object"])).is_err());
    }
}
