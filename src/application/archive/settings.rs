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
    DEFAULT_MIRROR_ROOT, DEFAULT_SETTLE_SECONDS,
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
    /// Notification categories: array of keys/labels, an object of per-category
    /// switches, or `null` for "everything on". Owned by the notify module.
    pub(crate) notify: Value,
    /// One of [`LISTEN_MODES`] (or its English alias).
    pub(crate) listen_mode: String,
    /// Channels the userbot listens to (`@username` or numeric id).
    pub(crate) listen_channels: Vec<String>,
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
        }
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
            notify: Value::Null,
            listen_mode: DEFAULT_LISTEN_MODE.to_owned(),
            listen_channels: Vec::new(),
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
                tracing::warn!(
                    %error,
                    path = %path.display(),
                    "archive settings file could not be read; using environment values"
                );
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
        settings.sanitized()
    }

    /// Persist to [`Self::path`]. Creates the parent directory and writes the
    /// file atomically with mode 0600.
    pub(crate) fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path())
    }

    pub(crate) fn save_to(&self, path: &Path) -> Result<(), String> {
        let mut document = serde_json::to_string_pretty(self)
            .map_err(|error| format!("无法序列化归档设置: {error}"))?;
        document.push('\n');
        write_private(path, &document)
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
        masked
    }

    /// Apply an update document (the PUT body) on top of the stored values.
    /// Blank secrets -- including a mask echoed back from a GET -- keep the
    /// stored value instead of clearing it.
    pub(crate) fn apply_update(&mut self, update: &Value) -> Result<(), String> {
        let Value::Object(mut document) = update.clone() else {
            return Err("设置内容必须是 JSON 对象".to_owned());
        };
        keep_stored_secret(&mut document, "tmdb_api_key", &self.tmdb_api_key);
        if let Some(Value::Object(telegram)) = document.get_mut("telegram") {
            keep_stored_secret(telegram, "bot_token", &self.telegram.bot_token);
            keep_stored_secret(telegram, "chat_id", &self.telegram.chat_id);
            keep_stored_secret(telegram, "api_hash", &self.telegram.api_hash);
            keep_stored_secret(telegram, "phone", &self.telegram.phone);
        }
        *self = Self::from_document(self, &Value::Object(document)).sanitized();
        Ok(())
    }

    /// Reject values that would make the pipeline misbehave, before they reach
    /// the file: the API reports the problem instead of clamping silently.
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
        if let Some(value) = document.get("notify") {
            settings.notify = value.clone();
        }
        if let Some(value) = string_field(document, "listen_mode") {
            settings.listen_mode = value;
        }
        if let Some(value) = string_list_field(document, "listen_channels") {
            settings.listen_channels = value;
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
        settings
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
    let temp = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let written = (|| -> std::io::Result<()> {
        let mut file = create_private(&temp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        set_private(path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

fn write_in_place(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    drop(file);
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
                "telegram": "not an object"
            }),
        );
        assert_eq!(settings.tmdb_api_key, "keep");
        assert_eq!(settings.poll_seconds, 0);
        assert!(settings.wash_enabled);
        assert!(settings.watch_dirs.is_empty());
        assert_eq!(settings.telegram.listen_mode, DEFAULT_LISTEN_MODE);
    }

    #[test]
    fn sanitizes_out_of_range_values_on_load() {
        let settings = ArchiveSettings::from_document(
            &ArchiveSettings::default(),
            &json!({ "poll_seconds": 999_999, "settle_seconds": 999_999, "wash_tolerance_ratio": 4.0 }),
        )
        .sanitized();
        assert_eq!(settings.poll_seconds, 0);
        assert_eq!(settings.settle_seconds, MAX_SETTLE_SECONDS);
        assert_eq!(settings.wash_tolerance_ratio, DEFAULT_WASH_TOLERANCE_RATIO);
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
