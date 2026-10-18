//! Archive pipeline: transfer -> pending -> media, with failure and duplicate
//! sinks. This is the first slice of the chill replacement work (plan U9):
//! resources land in the transfer directory, are moved to pending, are
//! identified through TMDB, and are organized into
//! `<media>/<国内|国外><类型>/<年份>/<Title (Year)>/`.
//!
//! The pipeline is deliberately directory-driven: every stage is a visible
//! directory role, so a failed identification can be inspected by hand and a
//! half-finished run leaves no hidden state.
//!
//! The matching rules are frozen in `docs/archive-pipeline.md`; change that
//! document first and then the code, never the other way around.

use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

mod nfo;

pub(crate) const ARCHIVE_ROOT_ENV: &str = "LUX_ARCHIVE_ROOT";
pub(crate) const ARCHIVE_TMDB_KEY_ENV: &str = "LUX_TMDB_API_KEY";
pub(crate) const ARCHIVE_TMDB_LANGUAGE_ENV: &str = "LUX_TMDB_LANGUAGE";
pub(crate) const DEFAULT_ARCHIVE_ROOT: &str = "/vol5/1000/docker/lux-test";
const DEFAULT_MIRROR_ROOT: &str = "/vol5/1000/docker/lux-library";

const DEFAULT_CD2_CONTAINER_MOUNT: &str = "/media/cloud";
pub(crate) const ARCHIVE_POLL_SECONDS_ENV: &str = "LUX_ARCHIVE_POLL_SECONDS";
pub(crate) const ARCHIVE_MIRROR_ROOT_ENV: &str = "LUX_ARCHIVE_MIRROR_ROOT";
pub(crate) const ARCHIVE_WATCH_DIRS_ENV: &str = "LUX_ARCHIVE_WATCH_DIRS";
pub(crate) const ARCHIVE_CD2_PREFIX_ENV: &str = "LUX_ARCHIVE_CD2_PREFIX";
pub(crate) const ARCHIVE_CD2_MOUNT_ENV: &str = "LUX_ARCHIVE_CD2_MOUNT";
pub(crate) const ARCHIVE_WASH_ENABLED_ENV: &str = "LUX_ARCHIVE_WASH_ENABLED";
pub(crate) const ARCHIVE_WASH_TOLERANCE_ENV: &str = "LUX_ARCHIVE_WASH_TOLERANCE";
/// Files modified more recently than this are still being written and are left
/// for the next run. Kept short: the pack is a copy in progress only during
/// those seconds, and a slower pipeline loses far more time than a rare
/// half-written pack gains.
const DEFAULT_SETTLE_SECONDS: u64 = 5;
pub(crate) const ARCHIVE_SETTLE_SECONDS_ENV: &str = "LUX_ARCHIVE_SETTLE_SECONDS";
/// Metadata lookups run concurrently; moves stay sequential so duplicate and
/// wash decisions see the batch in name order.
const METADATA_RESOLVE_CONCURRENCY: usize = 8;

const TRANSFER_DIR: &str = "转存目录";
const PENDING_DIR: &str = "待整理";
const MEDIA_DIR: &str = "媒体目录";
const FAILED_DIR: &str = "失败目录";
const DUPLICATE_DIR: &str = "重复目录";
const WASH_DIR: &str = "洗版目录";

/// Link hand-off queue: every accepted link becomes one JSON job file here.
/// Producers (today: the Telegram sidecar) only write; the pipeline consumes
/// and moves each file into a sub-directory, so a failed consumer never loses
/// the request.
const LINK_QUEUE_DIR: &str = "链接队列";
/// Append-only audit trail of every consumed link job, one JSON object per line.
const LINK_QUEUE_STATE_FILE: &str = "状态.jsonl";
const LINK_QUEUE_DONE_DIR: &str = "已处理";
const LINK_QUEUE_INVALID_DIR: &str = "无效";
/// `needs_115_client`: the link is valid and queued, but the 115 share-link
/// transfer client (plan U11) does not exist yet, so nothing is downloaded.
pub(crate) const LINK_JOB_STATUS_NEEDS_115: &str = "needs_115_client";
pub(crate) const LINK_JOB_STATUS_INVALID: &str = "invalid";
pub(crate) const LINK_JOB_NEEDS_115_REASON: &str =
    "115 转存客户端未实现（U11）：链接已登记，未下载任何内容";
/// Longest accepted link; share links are far shorter, the cap only stops
/// junk from filling the queue.
const LINK_JOB_MAX_URL_CHARS: usize = 2048;
const LINK_JOB_MAX_TITLE_CHARS: usize = 200;

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "ts", "m2ts", "webm", "rmvb", "rm", "mpg", "mpeg",
];

#[derive(Clone, Debug)]
pub(crate) struct ArchiveConfig {
    pub(crate) root: PathBuf,
    pub(crate) tmdb_api_key: Option<String>,
    pub(crate) tmdb_language: String,
    /// Extra 115 directories (on the CD2 mount) watched for new uploads; their
    /// entries are moved into 转存目录 so the normal pipeline takes over.
    pub(crate) watch_dirs: Vec<PathBuf>,
    /// Local mirror that the media server scans: strm + NFO + images only.
    pub(crate) mirror_root: PathBuf,
    /// Path of the archive root inside the CloudDrive2 mount (for example
    /// `测试库`); the strm URL is built from it.
    pub(crate) cd2_prefix: String,
    /// Where the CD2 mount is visible inside the media server container.
    pub(crate) cd2_container_mount: String,
    /// How long a freshly written entry waits before it is claimed.
    pub(crate) settle_seconds: u64,
    /// Replace an existing entry when the incoming pack is larger than it.
    pub(crate) wash_enabled: bool,
    /// Relative size difference under which two packs count as the same version
    /// (chill's `wash_tolerance_ratio`).
    pub(crate) wash_tolerance_ratio: f64,
}

impl ArchiveConfig {
    pub(crate) fn from_env() -> Self {
        let root = std::env::var(ARCHIVE_ROOT_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_ARCHIVE_ROOT));
        let tmdb_api_key = std::env::var(ARCHIVE_TMDB_KEY_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let tmdb_language = std::env::var(ARCHIVE_TMDB_LANGUAGE_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "zh-CN".to_owned());
        let watch_dirs = std::env::var(ARCHIVE_WATCH_DIRS_ENV)
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        let mirror_root = std::env::var(ARCHIVE_MIRROR_ROOT_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_MIRROR_ROOT));
        let cd2_prefix = std::env::var(ARCHIVE_CD2_PREFIX_ENV)
            .ok()
            .map(|value| value.trim().trim_matches('/').to_string())
            .unwrap_or_default();
        let cd2_container_mount = std::env::var(ARCHIVE_CD2_MOUNT_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_CD2_CONTAINER_MOUNT.to_owned());
        let settle_seconds = std::env::var(ARCHIVE_SETTLE_SECONDS_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|seconds| *seconds <= 3600)
            .unwrap_or(DEFAULT_SETTLE_SECONDS);
        let wash_enabled = std::env::var(ARCHIVE_WASH_ENABLED_ENV)
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .map(|value| !matches!(value.as_str(), "0" | "false" | "off" | "no"))
            .unwrap_or(true);
        let wash_tolerance_ratio = std::env::var(ARCHIVE_WASH_TOLERANCE_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|ratio| (0.0..=1.0).contains(ratio))
            .unwrap_or(0.02);
        Self {
            root,
            tmdb_api_key,
            tmdb_language,
            watch_dirs,
            mirror_root,
            cd2_prefix,
            cd2_container_mount,
            settle_seconds,
            wash_enabled,
            wash_tolerance_ratio,
        }
    }

    pub(crate) fn transfer_dir(&self) -> PathBuf {
        self.root.join(TRANSFER_DIR)
    }

    pub(crate) fn pending_dir(&self) -> PathBuf {
        self.root.join(PENDING_DIR)
    }

    pub(crate) fn media_dir(&self) -> PathBuf {
        self.root.join(MEDIA_DIR)
    }

    pub(crate) fn failed_dir(&self) -> PathBuf {
        self.root.join(FAILED_DIR)
    }

    pub(crate) fn duplicate_dir(&self) -> PathBuf {
        self.root.join(DUPLICATE_DIR)
    }

    pub(crate) fn wash_dir(&self) -> PathBuf {
        self.root.join(WASH_DIR)
    }

    /// Where producers drop link job files (`<archive_root>/链接队列/`).
    pub(crate) fn link_queue_dir(&self) -> PathBuf {
        self.root.join(LINK_QUEUE_DIR)
    }

    pub(crate) fn link_state_file(&self) -> PathBuf {
        self.link_queue_dir().join(LINK_QUEUE_STATE_FILE)
    }

    pub(crate) fn link_done_dir(&self) -> PathBuf {
        self.link_queue_dir().join(LINK_QUEUE_DONE_DIR)
    }

    pub(crate) fn link_invalid_dir(&self) -> PathBuf {
        self.link_queue_dir().join(LINK_QUEUE_INVALID_DIR)
    }

    /// Create every directory role. Idempotent.
    pub(crate) fn ensure_directories(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.transfer_dir())?;
        std::fs::create_dir_all(self.pending_dir())?;
        std::fs::create_dir_all(self.media_dir())?;
        std::fs::create_dir_all(self.failed_dir())?;
        std::fs::create_dir_all(self.duplicate_dir())?;
        std::fs::create_dir_all(self.wash_dir())?;
        std::fs::create_dir_all(self.link_queue_dir())?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct ArchiveReport {
    pub(crate) dry_run: bool,
    pub(crate) archived_to_pending: Vec<String>,
    pub(crate) organized: Vec<OrganizedItem>,
    pub(crate) duplicates: Vec<DuplicateItem>,
    pub(crate) washed: Vec<WashedItem>,
    pub(crate) failed: Vec<FailedItem>,
    pub(crate) skipped: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct OrganizedItem {
    pub(crate) source: String,
    pub(crate) tmdb_id: i64,
    pub(crate) target: String,
    pub(crate) kind: String,
    pub(crate) region: String,
    pub(crate) year: Option<i64>,
    pub(crate) title: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DuplicateItem {
    pub(crate) source: String,
    pub(crate) existing: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WashedItem {
    pub(crate) source: String,
    pub(crate) replaced: String,
    pub(crate) replaced_size: u64,
    pub(crate) kept_size: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FailedItem {
    pub(crate) source: String,
    pub(crate) reason: String,
}

/// One link handed to the archive pipeline. The same shape is accepted by
/// `POST /api/v1/admin/archive/ingest` and stored as one `*.json` job file in
/// `链接队列/`; `jobId` and `receivedAtUnix` are assigned by Lux, so a
/// hand-written file only needs `url`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct LinkJob {
    #[serde(default)]
    pub(crate) job_id: String,
    #[serde(default)]
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) mode: Option<String>,
    #[serde(default)]
    pub(crate) source: Option<String>,
    #[serde(default)]
    pub(crate) kind: Option<String>,
    #[serde(default)]
    pub(crate) channel: Option<String>,
    #[serde(default)]
    pub(crate) message_id: Option<i64>,
    #[serde(default)]
    pub(crate) size_bytes: Option<i64>,
    #[serde(default)]
    pub(crate) captured_at: Option<String>,
    #[serde(default)]
    pub(crate) received_at_unix: i64,
}

impl LinkJob {
    /// API path: validate the URL, clean the text fields and assign a fresh
    /// job id (`<毫秒时间戳>-<标题 slug>`) plus receipt time. A producer-supplied
    /// `jobId` is deliberately ignored so it can never walk out of the queue
    /// directory.
    fn normalized(mut self) -> Result<Self, String> {
        self = self.cleaned()?;
        self.job_id = format!(
            "{}-{}",
            unix_millis(),
            link_slug(self.title.as_deref(), &self.url)
        );
        self.received_at_unix = unix_now();
        Ok(self)
    }

    /// Queue path: validate and clean, but keep the id / receipt time the file
    /// already carries, otherwise the state record and the file name disagree.
    /// Only a hand-written job without those fields gets them filled in.
    fn accept_queued(mut self) -> Result<Self, String> {
        self = self.cleaned()?;
        if self.job_id.trim().is_empty() {
            self.job_id = format!(
                "{}-{}",
                unix_millis(),
                link_slug(self.title.as_deref(), &self.url)
            );
        }
        if self.received_at_unix <= 0 {
            self.received_at_unix = unix_now();
        }
        Ok(self)
    }

    fn cleaned(mut self) -> Result<Self, String> {
        self.url = validate_link_url(&self.url)?;
        self.title = clean_optional_text(self.title, LINK_JOB_MAX_TITLE_CHARS);
        self.mode = clean_optional_text(self.mode, 32);
        self.source = clean_optional_text(self.source, 32);
        self.kind = clean_optional_text(self.kind, 32);
        self.channel = clean_optional_text(self.channel, 128);
        self.captured_at = clean_optional_text(self.captured_at, 64);
        Ok(self)
    }
}

/// What the queue consumer did with one job file.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct AcceptedLinkJob {
    pub(crate) job_id: String,
    pub(crate) url: String,
    pub(crate) file: String,
    pub(crate) status: &'static str,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LinkQueueReport {
    pub(crate) dry_run: bool,
    pub(crate) examined: usize,
    pub(crate) accepted: Vec<AcceptedLinkJob>,
    pub(crate) invalid: Vec<FailedItem>,
    pub(crate) skipped_incomplete: Vec<String>,
}

/// One line of `链接队列/状态.jsonl`.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct LinkJobStateRecord {
    pub(crate) recorded_at_unix: i64,
    pub(crate) status: &'static str,
    pub(crate) job_id: String,
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) mode: String,
    pub(crate) source: String,
    pub(crate) reason: String,
    pub(crate) job_file: String,
}

/// The subset of a state record needed to answer "is this URL already queued?".
#[derive(Clone, Debug, Default, Deserialize)]
struct LinkStateLine {
    #[serde(default)]
    status: String,
    #[serde(default)]
    job_id: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    job_file: String,
}

/// A link job that was written to the queue (or found already queued).
#[derive(Clone, Debug, Serialize)]
pub(crate) struct QueuedLinkJob {
    pub(crate) job_id: String,
    pub(crate) file: String,
    pub(crate) status: &'static str,
    /// `true` when the same URL was already waiting or already consumed, in
    /// which case nothing new was written (the ingest endpoint is idempotent
    /// on the URL).
    pub(crate) duplicate: bool,
}

/// Transport-agnostic ingest failure: `Invalid` is the caller's fault (400),
/// `Storage` means the archive root could not be written (500).
#[derive(Clone, Debug)]
pub(crate) enum LinkIngestError {
    Invalid(String),
    Storage(String),
}

impl fmt::Display for LinkIngestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkIngestError::Invalid(message) | LinkIngestError::Storage(message) => {
                formatter.write_str(message)
            }
        }
    }
}

/// Result of the wash-mode "how big is our copy" query.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct LibraryLookup {
    pub(crate) title: String,
    pub(crate) matched: Option<String>,
    pub(crate) size_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedName {
    pub(crate) title: String,
    pub(crate) tmdb_id: Option<i64>,
    pub(crate) year: Option<i64>,
    pub(crate) season: Option<u32>,
    pub(crate) episode: Option<u32>,
}

impl ParsedName {
    pub(crate) fn is_episode(&self) -> bool {
        self.season.is_some() || self.episode.is_some()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MediaKind {
    #[default]
    Movie,
    Series,
}

impl MediaKind {
    fn label(self) -> &'static str {
        match self {
            MediaKind::Movie => "电影",
            MediaKind::Series => "电视剧",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TmdbMatch {
    pub(crate) kind: MediaKind,
    pub(crate) tmdb_id: i64,
    pub(crate) title: String,
    pub(crate) year: Option<i64>,
    pub(crate) origin_countries: Vec<String>,
    /// Country of the earliest dated release (movies); `None` when TMDB has no
    /// release-date data, in which case the origin country is the fallback.
    pub(crate) release_country: Option<String>,
    pub(crate) original_language: Option<String>,
    pub(crate) genres: Vec<String>,
}

impl TmdbMatch {
    /// Domestic means "first released in China": the earliest dated release of
    /// a movie decides, and TV keeps its origin country because TMDB has no
    /// per-country release dates for series. Origin country / language only
    /// remain as a fallback when no release data exists.
    pub(crate) fn is_domestic(&self) -> bool {
        const DOMESTIC_COUNTRIES: &[&str] = &["CN", "HK", "TW", "MO"];
        let domestic_country = |country: &str| {
            DOMESTIC_COUNTRIES.contains(&country.to_ascii_uppercase().as_str())
        };
        if let Some(country) = self.release_country.as_deref() {
            return domestic_country(country);
        }
        if self
            .origin_countries
            .iter()
            .any(|country| domestic_country(country))
        {
            return true;
        }
        self.original_language
            .as_deref()
            .is_some_and(|language| matches!(language, "zh" | "cn" | "yue"))
    }

    /// Variety shows and animation get their own boards; everything else is a
    /// movie or a TV series.
    pub(crate) fn category_label(&self) -> &'static str {
        if self.genres.iter().any(|genre| genre == "综艺") {
            return "综艺";
        }
        if self.genres.iter().any(|genre| genre == "动画") {
            return "动画";
        }
        self.kind.label()
    }
}

/// The poller and an admin request can both call `run_once`; a shared lock
/// serializes them so one never moves an entry the other is already moving.
static ARCHIVE_RUN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) struct ArchiveService {
    config: Arc<ArchiveConfig>,
    http: reqwest::Client,
}

impl fmt::Debug for ArchiveService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArchiveService")
            .field("root", &self.config.root)
            .finish()
    }
}

impl ArchiveService {
    #[cfg(test)]
    pub(crate) fn new_for_tests(config: ArchiveConfig) -> Self {
        Self::new(config)
    }

    /// Local mirror directory for a media path under the archive root. The
    /// media directory level is dropped so the mirror root is itself a clean
    /// media-server library root (`<mirror>/<board>/<year>/<title>`).
    pub(crate) fn mirror_dir_for(&self, media_path: &Path) -> Option<PathBuf> {
        let relative = media_path.strip_prefix(&self.config.root).ok()?;
        let relative = relative.strip_prefix(MEDIA_DIR).unwrap_or(relative);
        Some(self.config.mirror_root.join(relative))
    }

    pub(crate) fn new(config: ArchiveConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("lux-archive/0.1")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            config: Arc::new(config),
            http,
        }
    }

    /// Run the pipeline once. `dry_run` reports what would happen without moving
    /// anything, which is the default first-run mode for safety.
    pub(crate) async fn run_once(&self, dry_run: bool) -> Result<ArchiveReport, String> {
        let _run_guard = ARCHIVE_RUN_LOCK.lock().await;
        if !dry_run {
            self.config
                .ensure_directories()
                .map_err(|error| format!("无法创建归档目录: {error}"))?;
            std::fs::create_dir_all(&self.config.mirror_root)
                .map_err(|error| format!("无法创建本地镜像目录: {error}"))?;
        }
        let mut report = ArchiveReport {
            dry_run,
            ..ArchiveReport::default()
        };

        let link_queue = self.ingest_link_queue(dry_run)?;
        if link_queue.examined > 0 {
            tracing::info!(
                dry_run = link_queue.dry_run,
                examined = link_queue.examined,
                accepted = link_queue.accepted.len(),
                invalid = link_queue.invalid.len(),
                incomplete = link_queue.skipped_incomplete.len(),
                "link queue consumed; accepted links wait for the 115 transfer client"
            );
        }
        let ingested = self.ingest_watch_dirs(dry_run).await?;
        if !ingested.is_empty() {
            tracing::info!(
                count = ingested.len(),
                entries = ?ingested,
                "watch directories fed the transfer directory"
            );
        }
        self.archive_transfer_entries(&mut report).await?;
        self.organize_pending_entries(&mut report).await?;
        if !dry_run {
            crate::application::notify::spawn_archive_report(&report);
        }
        Ok(report)
    }

    /// Move settled entries out of the transfer directory into pending.
    async fn archive_transfer_entries(&self, report: &mut ArchiveReport) -> Result<(), String> {
        let transfer = self.config.transfer_dir();
        for entry in read_dir_entries(&transfer)? {
            let name = entry
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| entry.display().to_string());
            if is_ignored_entry(&name) {
                continue;
            }
            if !entry_is_settled(&entry, self.config.settle_seconds) {
                report.skipped.push(format!("{name} (仍在写入)"));
                continue;
            }
            let target = self.config.pending_dir().join(&name);
            if report.dry_run {
                report.archived_to_pending.push(name);
                continue;
            }
            match move_entry(&entry, &target) {
                Ok(()) => report.archived_to_pending.push(name),
                Err(error) => report.failed.push(FailedItem {
                    source: name,
                    reason: format!("转存→待整理失败: {error}"),
                }),
            }
        }
        Ok(())
    }

    async fn organize_pending_entries(&self, report: &mut ArchiveReport) -> Result<(), String> {
        let pending = self.config.pending_dir();
        let mut candidates = read_dir_entries(&pending)?;
        // Dry runs do not move anything, so the plan must remember which
        // targets an earlier entry of the same batch would have created;
        // otherwise two releases of one title both plan to the same path and
        // the duplicate/wash decision never shows up.
        let mut planned_targets: HashMap<PathBuf, u64> = HashMap::new();
        if report.dry_run {
            // A dry run does not move transfer entries, so plan them as if the
            // archive step had run; otherwise the report would claim nothing
            // would be organized.
            candidates.extend(read_dir_entries(&self.config.transfer_dir())?);
            candidates.sort();
        }
        // TMDB latency dominates a run and every entry is independent, so the
        // lookups are resolved up front in bounded concurrent chunks. The
        // decisions below stay sequential and in name order so a duplicate
        // pair is always compared deterministically.
        let mut tmdb_results: HashMap<PathBuf, Result<Option<TmdbMatch>, String>> = HashMap::new();
        let mut lookup_jobs: Vec<(PathBuf, ParsedName, bool)> = Vec::new();
        for entry in &candidates {
            let name = entry
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            if is_ignored_entry(&name) || !entry_is_settled(entry, self.config.settle_seconds) {
                continue;
            }
            let is_dir = entry.is_dir();
            if is_dir {
                if !dir_contains_video(entry) {
                    continue;
                }
            } else if video_extension(entry).is_none() {
                continue;
            }
            if special_category(&name).is_some() {
                continue;
            }
            let raw_name = if is_dir {
                name.clone()
            } else {
                stem_of(&name).to_string()
            };
            let Some(parsed) = parse_media_name(&raw_name) else {
                continue;
            };
            let series_hint = is_dir && dir_signals_series(entry, &parsed);
            lookup_jobs.push((entry.clone(), parsed, series_hint));
        }
        for chunk in lookup_jobs.chunks(METADATA_RESOLVE_CONCURRENCY) {
            let mut join_set = tokio::task::JoinSet::new();
            for (entry, parsed, series_hint) in chunk.iter() {
                let config = Arc::clone(&self.config);
                let http = self.http.clone();
                let entry = entry.clone();
                let parsed = parsed.clone();
                let series_hint = *series_hint;
                join_set.spawn(async move {
                    let service = ArchiveService { config, http };
                    let result = service.lookup_tmdb(&parsed, series_hint).await;
                    (entry, result)
                });
            }
            while let Some(joined) = join_set.join_next().await {
                if let Ok((entry, result)) = joined {
                    tmdb_results.insert(entry, result);
                }
            }
        }
        for entry in candidates {
            let name = entry
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| entry.display().to_string());
            if is_ignored_entry(&name) {
                continue;
            }
            if !entry_is_settled(&entry, self.config.settle_seconds) {
                report.skipped.push(format!("{name} (仍在写入)"));
                continue;
            }
            let is_dir = entry.is_dir();
            if is_dir {
                if !dir_contains_video(&entry) {
                    self.fail_entry(report, &entry, "目录内没有视频文件")?;
                    continue;
                }
            } else if video_extension(&entry).is_none() {
                self.fail_entry(report, &entry, "不是视频文件")?;
                continue;
            }
            let raw_name = if is_dir {
                name.clone()
            } else {
                stem_of(&name).to_string()
            };
            let parsed = match parse_media_name(&raw_name) {
                Some(parsed) => parsed,
                None => {
                    self.fail_entry(report, &entry, "无法从文件名解析出片名")?;
                    continue;
                }
            };
            if let Some(category) = special_category(&name) {
                self.organize_special(report, &entry, &parsed, category, is_dir, &mut planned_targets)
                    .await?;
                continue;
            }
            let series_hint = is_dir && dir_signals_series(&entry, &parsed);
            let lookup = match tmdb_results.remove(&entry) {
                Some(result) => result,
                None => self.lookup_tmdb(&parsed, series_hint).await,
            };
            let matched = match lookup {
                Ok(Some(matched)) => matched,
                Ok(None) => {
                    self.fail_entry(report, &entry, "TMDB 未找到匹配")?;
                    continue;
                }
                Err(error) => {
                    self.fail_entry(report, &entry, &format!("TMDB 查询失败: {error}"))?;
                    continue;
                }
            };
            let year = parsed
                .year
                .or(matched.year)
                .filter(|year| (1888..=2100).contains(year));
            let region = if matched.is_domestic() { "国内" } else { "国外" };
            let category = matched.category_label();
            let year_segment = year
                .map(|year| year.to_string())
                .unwrap_or_else(|| "未知年份".to_owned());
            let safe_title = sanitize_component(&matched.title);
            let title_dir = match year {
                Some(year) => format!("{safe_title} ({year})"),
                None => safe_title.clone(),
            };
            let mut target_dir = self
                .config
                .media_dir()
                .join(category)
                .join(region)
                .join(&year_segment)
                .join(&title_dir);
            if matched.kind == MediaKind::Series && !is_dir {
                let season = parsed.season.unwrap_or(1);
                target_dir = target_dir.join(format!("Season {season:02}"));
            }
            // Directories are a release pack: the pack itself becomes the
            // title directory (base name); a single file keeps its own name
            // inside the title directory.
            let target = if is_dir {
                target_dir.clone()
            } else {
                target_dir.join(&name)
            };
            // A series pack is judged season by season. A pack that carries
            // Season 01 and Season 02 must still contribute the missing season
            // when another season is already in the library (or is a
            // duplicate); comparing the whole package would park all of it in
            // the duplicate directory and lose the new season. Packs without
            // season subdirectories keep the whole-package rule below.
            if is_dir
                && matched.kind == MediaKind::Series
                && (target.exists() || planned_targets.contains_key(&target))
            {
                let handled = self
                    .organize_series_pack(
                        report,
                        &entry,
                        &target,
                        &parsed,
                        &matched,
                        region,
                        year,
                        &mut planned_targets,
                    )
                    .await?;
                if handled {
                    continue;
                }
            }
            let existing_size = if target.exists() {
                Some(path_size(&target))
            } else {
                planned_targets.get(&target).copied()
            };
            if let Some(existing_size) = existing_size {
                let incoming_size = path_size(&entry);
                let decision = decide_existing(
                    incoming_size,
                    existing_size,
                    self.config.wash_enabled,
                    self.config.wash_tolerance_ratio,
                );
                if decision == ExistingDecision::Wash {
                    self.wash_entry(report, &entry, &target, incoming_size, existing_size)?;
                    if report.dry_run {
                        planned_targets.insert(target, incoming_size);
                    }
                    continue;
                }
                self.duplicate_entry(report, &entry, &target)?;
                if report.dry_run {
                    // The planned library copy stays the one already recorded.
                    planned_targets.entry(target).or_insert(existing_size);
                }
                continue;
            }
            if report.dry_run {
                planned_targets.insert(target.clone(), path_size(&entry));
            }
            if report.dry_run {
                report.organized.push(OrganizedItem {
                    source: name.clone(),
                    tmdb_id: matched.tmdb_id,
                    target: target.display().to_string(),
                    kind: matched.category_label().to_owned(),
                    region: region.to_owned(),
                    year,
                    title: matched.title.clone(),
                });
                continue;
            }
            if let Err(error) = std::fs::create_dir_all(&target_dir) {
                self.fail_entry(report, &entry, &format!("创建目标目录失败: {error}"))?;
                continue;
            }
            match move_entry(&entry, &target) {
                Ok(()) => {
                    report.organized.push(OrganizedItem {
                        source: name.clone(),
                        tmdb_id: matched.tmdb_id,
                        target: target.display().to_string(),
                        kind: matched.category_label().to_owned(),
                        region: region.to_owned(),
                        year,
                        title: matched.title.clone(),
                    });
                    // Sidecars go into the LOCAL mirror (strm + NFO + images);
                    // the 115 pack keeps only media files. Best effort: a
                    // metadata problem must never undo a correct move.
                    let mut metadata_report = nfo::MetadataReport::default();
                    if let Some(mirror) = self.mirror_dir_for(&target) {
                        let writer = nfo::MetadataWriter::new(self, false);
                        writer
                            .write_entry(&target, &mirror, &parsed, &matched, &mut metadata_report)
                            .await;
                    }
                    if !metadata_report.failed.is_empty() {
                        tracing::warn!(
                            target = %target.display(),
                            failed = ?metadata_report.failed,
                            "metadata sidecar generation had failures"
                        );
                    }
                }
                Err(error) => {
                    self.fail_entry(report, &entry, &format!("移动失败: {error}"))?;
                }
            }
        }
        Ok(())
    }

    fn fail_entry(&self, report: &mut ArchiveReport, entry: &Path, reason: &str) -> Result<(), String> {
        let name = entry
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.display().to_string());
        if report.dry_run {
            report.failed.push(FailedItem {
                source: name,
                reason: reason.to_owned(),
            });
            return Ok(());
        }
        tracing::warn!(entry = %name, %reason, "archive entry moved to the failure directory");
        let stamp = unix_now();
        let bucket = self.config.failed_dir().join(format!("{stamp}"));
        std::fs::create_dir_all(&bucket).map_err(|error| format!("创建失败目录: {error}"))?;
        let _ = std::fs::write(bucket.join("原因.txt"), reason);
        let target = bucket.join(&name);
        move_entry(entry, &target).map_err(|error| format!("移入失败目录: {error}"))?;
        report.failed.push(FailedItem {
            source: name,
            reason: reason.to_owned(),
        });
        Ok(())
    }

    fn wash_entry(
        &self,
        report: &mut ArchiveReport,
        entry: &Path,
        existing: &Path,
        incoming_size: u64,
        existing_size: u64,
    ) -> Result<(), String> {
        let name = entry
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.display().to_string());
        let replaced_name = existing
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| existing.display().to_string());
        if report.dry_run {
            report.washed.push(WashedItem {
                source: name,
                replaced: existing.display().to_string(),
                replaced_size: existing_size,
                kept_size: incoming_size,
            });
            return Ok(());
        }
        // Move the old pack out of the library first (under a timestamped name
        // so history is never lost), then put the bigger pack in place.
        let stamp = unix_now();
        let retired = self
            .config
            .wash_dir()
            .join(format!("{stamp}-{replaced_name}"));
        move_entry(existing, &retired).map_err(|error| format!("旧版移入洗版目录失败: {error}"))?;
        if let Err(error) = move_entry(entry, existing) {
            // Best-effort rollback: put the old pack back where it was.
            let _ = move_entry(&retired, existing);
            return Err(format!("新版移入媒体目录失败: {error}"));
        }
        report.washed.push(WashedItem {
            source: name,
            replaced: retired.display().to_string(),
            replaced_size: existing_size,
            kept_size: incoming_size,
        });
        Ok(())
    }

    fn duplicate_entry(
        &self,
        report: &mut ArchiveReport,
        entry: &Path,
        existing: &Path,
    ) -> Result<(), String> {
        let name = entry
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.display().to_string());
        if report.dry_run {
            report.duplicates.push(DuplicateItem {
                source: name,
                existing: existing.display().to_string(),
            });
            return Ok(());
        }
        let target = self.config.duplicate_dir().join(&name);
        move_entry(entry, &target).map_err(|error| format!("移入重复目录: {error}"))?;
        report.duplicates.push(DuplicateItem {
            source: name,
            existing: existing.display().to_string(),
        });
        Ok(())
    }

    /// Judge a series pack season by season and merge only the seasons the
    /// library is missing. Returns `false` when the pack has no season
    /// subdirectories, so the caller falls back to the whole-package rule.
    #[allow(clippy::too_many_arguments)]
    async fn organize_series_pack(
        &self,
        report: &mut ArchiveReport,
        entry: &Path,
        title_dir: &Path,
        parsed: &ParsedName,
        matched: &TmdbMatch,
        region: &str,
        year: Option<i64>,
        planned_targets: &mut HashMap<PathBuf, u64>,
    ) -> Result<bool, String> {
        let plan = plan_season_pack(
            entry,
            title_dir,
            self.config.wash_enabled,
            self.config.wash_tolerance_ratio,
            planned_targets,
        );
        if plan.is_empty() {
            return Ok(false);
        }
        let pack_name = entry
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.display().to_string());
        // Every season already in the library at the same size: nothing to
        // merge, so the pack stays one duplicate package as before.
        if plan.iter().all(|item| item.action == SeasonAction::Duplicate) {
            self.duplicate_entry(report, entry, title_dir)?;
            return Ok(true);
        }
        if report.dry_run {
            for item in &plan {
                match item.action {
                    SeasonAction::Add => {
                        report.organized.push(organized_item(
                            format!("{pack_name}/{}", season_dir_name(item.season)),
                            matched,
                            region,
                            year,
                            &item.target,
                        ));
                        planned_targets.insert(item.target.clone(), item.incoming_size);
                    }
                    SeasonAction::Wash => {
                        report.washed.push(WashedItem {
                            source: format!("{pack_name}/{}", season_dir_name(item.season)),
                            replaced: item.target.display().to_string(),
                            replaced_size: item.existing_size,
                            kept_size: item.incoming_size,
                        });
                        planned_targets.insert(item.target.clone(), item.incoming_size);
                    }
                    SeasonAction::Duplicate => {
                        report.duplicates.push(DuplicateItem {
                            source: format!("{pack_name}/{}", season_dir_name(item.season)),
                            existing: item.target.display().to_string(),
                        });
                        planned_targets
                            .entry(item.target.clone())
                            .or_insert(item.existing_size);
                    }
                }
            }
            return Ok(true);
        }
        let merged = self.apply_season_plan(report, entry, title_dir, &plan, matched, region, year)?;
        if merged {
            // The mirror is regenerated for the whole series: existing sidecars
            // are never overwritten, so only the season just merged appears.
            let mut metadata_report = nfo::MetadataReport::default();
            if let Some(mirror) = self.mirror_dir_for(title_dir) {
                let writer = nfo::MetadataWriter::new(self, false);
                writer
                    .write_entry(title_dir, &mirror, parsed, matched, &mut metadata_report)
                    .await;
            }
            if !metadata_report.failed.is_empty() {
                tracing::warn!(
                    target = %title_dir.display(),
                    failed = ?metadata_report.failed,
                    "metadata sidecar generation had failures"
                );
            }
        }
        Ok(true)
    }

    /// Move the planned seasons into place, best effort per season: one broken
    /// season must not undo the ones already merged. Returns whether anything
    /// entered the library.
    #[allow(clippy::too_many_arguments)]
    fn apply_season_plan(
        &self,
        report: &mut ArchiveReport,
        entry: &Path,
        title_dir: &Path,
        plan: &[SeasonPlanItem],
        matched: &TmdbMatch,
        region: &str,
        year: Option<i64>,
    ) -> Result<bool, String> {
        let pack_name = entry
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.display().to_string());
        if let Err(error) = std::fs::create_dir_all(title_dir) {
            self.fail_entry(
                report,
                entry,
                &format!("创建目标目录失败: {error}"),
            )?;
            return Ok(false);
        }
        let mut merged = false;
        for item in plan {
            let label = format!("{pack_name}/{}", season_dir_name(item.season));
            match item.action {
                SeasonAction::Add => match move_entry(&item.source, &item.target) {
                    Ok(()) => {
                        merged = true;
                        report.organized.push(organized_item(
                            label,
                            matched,
                            region,
                            year,
                            &item.target,
                        ));
                    }
                    Err(error) => report.failed.push(FailedItem {
                        source: label,
                        reason: format!("移动失败: {error}"),
                    }),
                },
                SeasonAction::Wash => {
                    match self.wash_entry(
                        report,
                        &item.source,
                        &item.target,
                        item.incoming_size,
                        item.existing_size,
                    ) {
                        Ok(()) => merged = true,
                        Err(reason) => report.failed.push(FailedItem {
                            source: label,
                            reason,
                        }),
                    }
                }
                SeasonAction::Duplicate => {
                    // Keep the duplicate season next to its siblings so the
                    // parked copy still shows which series it belongs to.
                    let duplicate_dir = self.config.duplicate_dir().join(&pack_name);
                    let duplicate_target = duplicate_dir.join(season_dir_name(item.season));
                    let outcome = std::fs::create_dir_all(&duplicate_dir)
                        .map_err(|error| format!("创建重复目录: {error}"))
                        .and_then(|()| {
                            move_entry(&item.source, &duplicate_target)
                                .map_err(|error| format!("移入重复目录: {error}"))
                        });
                    match outcome {
                        Ok(()) => report.duplicates.push(DuplicateItem {
                            source: label,
                            existing: item.target.display().to_string(),
                        }),
                        Err(reason) => report.failed.push(FailedItem {
                            source: label,
                            reason,
                        }),
                    }
                }
            }
        }
        self.merge_pack_leftovers(report, entry, title_dir, &pack_name)?;
        Ok(merged)
    }

    /// Move whatever is left in the pack (NFOs, samples, stray files) into the
    /// title directory once its seasons were handled, then drop the empty pack.
    fn merge_pack_leftovers(
        &self,
        report: &mut ArchiveReport,
        pack: &Path,
        title_dir: &Path,
        pack_name: &str,
    ) -> Result<(), String> {
        for child in read_dir_entries(pack)? {
            let name = child
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| child.display().to_string());
            let target = title_dir.join(&name);
            if target.exists() {
                tracing::warn!(
                    pack = %pack_name,
                    leftover = %name,
                    "pack leftover not merged: the target already exists"
                );
                report
                    .skipped
                    .push(format!("{pack_name}/{name} (目标已存在，未合并)"));
                continue;
            }
            if let Err(error) = move_entry(&child, &target) {
                report.failed.push(FailedItem {
                    source: format!("{pack_name}/{name}"),
                    reason: format!("移动失败: {error}"),
                });
            }
        }
        if is_dir_empty(pack) {
            let _ = std::fs::remove_dir(pack);
        }
        Ok(())
    }

    async fn lookup_tmdb(
        &self,
        parsed: &ParsedName,
        series_hint: bool,
    ) -> Result<Option<TmdbMatch>, String> {
        let Some(api_key) = self.config.tmdb_api_key.as_deref() else {
            return Err(format!(
                "未配置 TMDB API Key（设置环境变量 {ARCHIVE_TMDB_KEY_ENV}）"
            ));
        };
        // A `{tmdb-NNN}` hint names the id directly; only the segment can be
        // wrong, so try the likely one first and fall back to the other.
        if let Some(tmdb_id) = parsed.tmdb_id {
            let attempts: [(MediaKind, &str); 2] = if series_hint || parsed.is_episode() {
                [(MediaKind::Series, "tv"), (MediaKind::Movie, "movie")]
            } else {
                [(MediaKind::Movie, "movie"), (MediaKind::Series, "tv")]
            };
            for (_kind, segment) in attempts {
                if let Ok(mut details) = self.tmdb_details(api_key, segment, tmdb_id).await {
                    self.enrich_details(api_key, segment, tmdb_id, &mut details)
                        .await;
                    return Ok(Some(tmdb_match_from(segment, tmdb_id, &details, &details)?));
                }
            }
        }
        let attempts: [(MediaKind, &str); 2] = if series_hint || parsed.is_episode() {
            [(MediaKind::Series, "tv"), (MediaKind::Movie, "movie")]
        } else {
            [(MediaKind::Movie, "movie"), (MediaKind::Series, "tv")]
        };
        for (_kind, segment) in attempts {
            let results = self
                .tmdb_search(api_key, segment, &parsed.title, parsed.year)
                .await?;
            let mut candidate_count = results.len();
            // Owned candidate: the year-filtered search may be replaced by an
            // unfiltered one, and a borrow into the replaced value would not
            // outlive the loop body.
            let mut best = pick_best_result(&results, &parsed.title, parsed.year).cloned();
            if best.is_none() && parsed.year.is_some() {
                // The year filter hides entries whose TMDB date is missing
                // (Detective Investigation Files IV); retry without it.
                let without_year = self
                    .tmdb_search(api_key, segment, &parsed.title, None)
                    .await?;
                candidate_count = without_year.len();
                best = pick_best_result(&without_year, &parsed.title, None).cloned();
            }
            let Some(best) = best else {
                tracing::debug!(
                    query = %parsed.title,
                    segment,
                    candidates = candidate_count,
                    "TMDB search returned no usable match"
                );
                continue;
            };
            let id = best
                .get("id")
                .and_then(|value| value.as_i64())
                .ok_or_else(|| "TMDB 返回缺少 id".to_owned())?;
            let mut details = self.tmdb_details(api_key, segment, id).await?;
            self.enrich_details(api_key, segment, id, &mut details)
                .await;
            return Ok(Some(tmdb_match_from(segment, id, &best, &details)?));
        }
        Ok(None)
    }

    /// Search in en-US and in the display language, then merge by TMDB id.
    /// Chinese titles often exist only in the zh-CN index
    /// (刑事侦缉档案4), while release names are English; one query alone misses
    /// one of the two.
    async fn tmdb_search(
        &self,
        api_key: &str,
        segment: &str,
        query: &str,
        year: Option<i64>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let mut results = self
            .tmdb_search_in_language(api_key, segment, query, year, "en-US")
            .await?;
        if !self.config.tmdb_language.eq_ignore_ascii_case("en-US") {
            if let Ok(extra) = self
                .tmdb_search_in_language(api_key, segment, query, year, &self.config.tmdb_language)
                .await
            {
                // Keep both entries even when the ids match: the localized one
                // carries the Chinese name that a Chinese release name matches
                // against (刑事侦缉档案4 vs Detective Investigation Files IV).
                results.extend(extra);
            }
        }
        Ok(results)
    }

    async fn tmdb_search_in_language(
        &self,
        api_key: &str,
        segment: &str,
        query: &str,
        year: Option<i64>,
        language: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        // Search always uses en-US so results keep original titles next to the
        // localized ones; zh-CN results rename well-known titles (Inception ->
        // 盗梦空间) and make title matching against release names unreliable.
        // Details are still fetched in the configured display language.
        let mut url = format!(
            "https://api.themoviedb.org/3/search/{segment}?api_key={api_key}&language={}&query={}",
            urlencode(language),
            urlencode(query)
        );
        if let Some(year) = year {
            // Movies use `year`, TV uses `first_air_date_year`.
            let key = if segment == "tv" {
                "first_air_date_year"
            } else {
                "year"
            };
            url.push_str(&format!("&{key}={year}"));
        }
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|error| format!("TMDB 响应解析失败: {error}"))?;
        if !status.is_success() {
            let message = body
                .get("status_message")
                .and_then(|value| value.as_str())
                .unwrap_or("未知错误");
            return Err(format!("TMDB HTTP {status}: {message}"));
        }
        Ok(body
            .get("results")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// Attach the earliest-release country (movies) and a fallback year to the
    /// details. Some TMDB entries have no `release_date` / `first_air_date` at
    /// all (e.g. Detective Investigation Files IV), so the year is taken from
    /// the earliest release date or from season 1's air date instead of giving
    /// up with "未知年份".
    async fn enrich_details(
        &self,
        api_key: &str,
        segment: &str,
        id: i64,
        details: &mut serde_json::Value,
    ) {
        if segment == "movie" {
            let url = format!("https://api.themoviedb.org/3/movie/{id}/release_dates?api_key={api_key}");
            let Ok(response) = self.http.get(&url).send().await else {
                return;
            };
            if !response.status().is_success() {
                return;
            }
            let Ok(body) = response.json::<serde_json::Value>().await else {
                return;
            };
            if let Some((date, country)) = earliest_release(&body) {
                details["_release_country"] = serde_json::Value::String(country);
                if let Some(year) = date.get(..4).and_then(|year| year.parse::<i64>().ok()) {
                    details["_fallback_year"] = serde_json::json!(year);
                }
            }
            return;
        }
        // TV: a missing first_air_date is common; the season list and
        // last_air_date in the same details payload usually carry a date
        // (e.g. Detective Investigation Files IV -> season 4 aired 1999-03-29).
        if let Some(year) = season_or_last_air_year(details) {
            details["_fallback_year"] = serde_json::json!(year);
            return;
        }
        // TV: season 1's air date is the most reliable fallback year.
        let url = format!("https://api.themoviedb.org/3/tv/{id}/season/1?api_key={api_key}&language=en-US");
        let Ok(response) = self.http.get(&url).send().await else {
            return;
        };
        if !response.status().is_success() {
            return;
        }
        let Ok(body) = response.json::<serde_json::Value>().await else {
            return;
        };
        if let Some(air_date) = body.get("air_date").and_then(|value| value.as_str())
            && let Some(year) = air_date.get(..4).and_then(|year| year.parse::<i64>().ok())
        {
            details["_fallback_year"] = serde_json::json!(year);
        }
    }

    async fn tmdb_details(
        &self,
        api_key: &str,
        segment: &str,
        id: i64,
    ) -> Result<serde_json::Value, String> {
        let url = format!(
            "https://api.themoviedb.org/3/{segment}/{id}?api_key={api_key}&language={}",
            urlencode(&self.config.tmdb_language)
        );
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|error| format!("TMDB 详情解析失败: {error}"))?;
        if !status.is_success() {
            return Err(format!("TMDB 详情 HTTP {status}"));
        }
        Ok(body)
    }

    /// Walk the media directory and write NFO/poster sidecars for entries that
    /// do not have them yet. Idempotent; `overwrite` regenerates existing ones.
    pub(crate) async fn backfill_metadata(
        &self,
        dry_run: bool,
        overwrite: bool,
    ) -> Result<nfo::MetadataReport, String> {
        let mut report = nfo::MetadataReport::default();
        let media = self.config.media_dir();
        for category_dir in read_dir_entries(&media)? {
            if !category_dir.is_dir() {
                continue;
            }
            let category_name = category_dir
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            let series_category = category_name.contains("电视剧")
                || category_name.contains("综艺")
                || category_name.contains("动画");
            let movie_category = category_name.contains("电影");
            if !series_category && !movie_category {
                continue;
            }
            for region_dir in read_dir_entries(&category_dir)? {
                if !region_dir.is_dir() {
                    continue;
                }
                let mut year_dirs = vec![region_dir.clone()];
                let nested = read_dir_entries(&region_dir)?;
                if nested.iter().any(|path| path.is_dir()) {
                    year_dirs = nested;
                }
                for year_dir in year_dirs {
                if !year_dir.is_dir() {
                    continue;
                }
                for title_dir in read_dir_entries(&year_dir)? {
                    if !title_dir.is_dir() {
                        continue;
                    }
                    let Some(mirror_dir) = self.mirror_dir_for(&title_dir) else {
                        report
                            .failed
                            .push(format!("{}: 无法映射镜像路径", title_dir.display()));
                        continue;
                    };
                    let already_done = mirror_dir.join("tvshow.nfo").exists()
                        || (!series_category && dir_has_nfo(&mirror_dir));
                    if !overwrite && already_done {
                        report.skipped.push(title_dir.display().to_string());
                        continue;
                    }
                    let title_name = title_dir
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let Some(parsed) = parse_media_name(&title_name) else {
                        report
                            .failed
                            .push(format!("{}: 无法从目录名解析", title_dir.display()));
                        continue;
                    };
                    let matched = match self.lookup_tmdb(&parsed, series_category).await {
                        Ok(Some(matched)) => matched,
                        Ok(None) => {
                            report
                                .failed
                                .push(format!("{}: TMDB 未找到匹配", title_dir.display()));
                            continue;
                        }
                        Err(error) => {
                            report.failed.push(format!(
                                "{}: TMDB 查询失败: {error}",
                                title_dir.display()
                            ));
                            continue;
                        }
                    };
                    if dry_run {
                        report.created.push(format!(
                            "(dry) {} -> {} ({})",
                            title_dir.display(),
                            matched.title,
                            matched.tmdb_id
                        ));
                        continue;
                    }
                    let writer = nfo::MetadataWriter::new(self, overwrite);
                    writer
                        .write_entry(&title_dir, &mirror_dir, &parsed, &matched, &mut report)
                        .await;
                }
            }
            }
        }
        Ok(report)
    }

    /// Organize a 晚会/演唱会 entry without TMDB data: the release name gives
    /// the title and year, and a CJK title puts a concert on the domestic
    /// board. These events rarely exist in TMDB, so no metadata lookup is
    /// attempted and a minimal NFO is written.
    async fn organize_special(
        &self,
        report: &mut ArchiveReport,
        entry: &Path,
        parsed: &ParsedName,
        category: &str,
        is_dir: bool,
        planned: &mut HashMap<PathBuf, u64>,
    ) -> Result<(), String> {
        let name = entry
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.display().to_string());
        let year = parsed.year.filter(|year| (1888..=2100).contains(year));
        let safe_title = sanitize_component(&parsed.title);
        let title_dir = match year {
            Some(year) => format!("{safe_title} ({year})"),
            None => safe_title.clone(),
        };
        let year_segment = year
            .map(|year| year.to_string())
            .unwrap_or_else(|| "未知年份".to_owned());
        let mut target_dir = self.config.media_dir().join(category);
        let region = if category == "演唱会" {
            let region = if contains_cjk(&parsed.title) {
                "国内"
            } else {
                "国外"
            };
            target_dir = target_dir.join(region);
            region
        } else {
            ""
        };
        let target_dir = target_dir.join(&year_segment);
        let target = if is_dir {
            target_dir.join(&title_dir)
        } else {
            target_dir.join(&title_dir).join(&name)
        };
        let existing_size = if target.exists() {
            Some(path_size(&target))
        } else {
            planned.get(&target).copied()
        };
        if let Some(existing_size) = existing_size {
            let incoming_size = path_size(entry);
            let decision = decide_existing(
                incoming_size,
                existing_size,
                self.config.wash_enabled,
                self.config.wash_tolerance_ratio,
            );
            if decision == ExistingDecision::Wash {
                self.wash_entry(report, entry, &target, incoming_size, existing_size)?;
                return Ok(());
            }
            self.duplicate_entry(report, entry, &target)?;
            return Ok(());
        }
        if report.dry_run {
            report.organized.push(OrganizedItem {
                source: name.clone(),
                tmdb_id: 0,
                target: target.display().to_string(),
                kind: category.to_owned(),
                region: region.to_owned(),
                year,
                title: parsed.title.clone(),
            });
            planned.insert(target, path_size(entry));
            return Ok(());
        }
        std::fs::create_dir_all(&target_dir)
            .map_err(|error| format!("创建目标目录失败: {error}"))?;
        match move_entry(entry, &target) {
            Ok(()) => {
                report.organized.push(OrganizedItem {
                    source: name.clone(),
                    tmdb_id: 0,
                    target: target.display().to_string(),
                    kind: category.to_owned(),
                    region: region.to_owned(),
                    year,
                    title: parsed.title.clone(),
                });
                if let Some(mirror) = self.mirror_dir_for(&target) {
                    let mut metadata_report = nfo::MetadataReport::default();
                    let writer = nfo::MetadataWriter::new(self, false);
                    writer
                        .write_special(&target, &mirror, &parsed.title, year, &mut metadata_report)
                        .await;
                    if !metadata_report.failed.is_empty() {
                        tracing::warn!(
                            target = %target.display(),
                            failed = ?metadata_report.failed,
                            "special entry sidecar generation had failures"
                        );
                    }
                }
                Ok(())
            }
            Err(error) => self.fail_entry(report, entry, &format!("移动失败: {error}")),
        }
    }

    /// Move new uploads from the watched 115 directories into 转存目录.
    /// Returns the moved names (or the planned names in a dry run).
    async fn ingest_watch_dirs(&self, dry_run: bool) -> Result<Vec<String>, String> {
        let mut moved = Vec::new();
        let transfer = self.config.transfer_dir();
        for watch in &self.config.watch_dirs {
            let entries = match read_dir_entries(watch) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries {
                let name = entry
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                if is_ignored_entry(&name) || !entry_is_settled(&entry, self.config.settle_seconds) {
                    continue;
                }
                if is_dir_empty(&entry) {
                    continue;
                }
                let target = unique_target(&transfer, &name);
                if dry_run {
                    moved.push(format!("{name} -> {}", target.display()));
                    continue;
                }
                if let Err(error) = std::fs::create_dir_all(&transfer) {
                    return Err(format!("无法创建转存目录: {error}"));
                }
                match move_entry(&entry, &target) {
                    Ok(()) => moved.push(name),
                    Err(error) => tracing::warn!(
                        entry = %entry.display(),
                        %error,
                        "failed to move a watched entry into 转存目录"
                    ),
                }
            }
        }
        Ok(moved)
    }

    /// Write one link job into `链接队列/` for the pipeline to pick up.
    ///
    /// The write is atomic (temp file + rename) so the consumer never reads a
    /// half-written job; the URL is validated first so a job file always has a
    /// fetchable share link. **Idempotent on the URL**: a link that is already
    /// waiting in the queue, or was already consumed (recorded in
    /// `状态.jsonl`), is not queued twice — the existing job is returned with
    /// `duplicate: true`.
    pub(crate) fn enqueue_link_job(&self, job: LinkJob) -> Result<QueuedLinkJob, LinkIngestError> {
        let job = job.normalized().map_err(LinkIngestError::Invalid)?;
        let queue = self.config.link_queue_dir();
        std::fs::create_dir_all(&queue).map_err(|error| {
            LinkIngestError::Storage(format!("无法创建链接队列 {}: {error}", queue.display()))
        })?;
        if let Some(existing) = self.find_existing_link_job(&job.url) {
            tracing::info!(
                url = %job.url,
                job_id = %existing.job_id,
                file = %existing.file,
                "link job already queued or already consumed; not queued again"
            );
            return Ok(QueuedLinkJob {
                job_id: existing.job_id,
                file: existing.file,
                status: LINK_JOB_STATUS_NEEDS_115,
                duplicate: true,
            });
        }
        let name = format!("{}.json", job.job_id);
        let target = unique_target(&queue, &name);
        let body = serde_json::to_string_pretty(&job).map_err(|error| {
            LinkIngestError::Storage(format!("序列化链接任务失败: {error}"))
        })?;
        let temp = queue.join(format!(".{}.tmp", job.job_id));
        std::fs::write(&temp, body.as_bytes())
            .map_err(|error| LinkIngestError::Storage(format!("写入 {} 失败: {error}", temp.display())))?;
        std::fs::rename(&temp, &target).map_err(|error| {
            let _ = std::fs::remove_file(&temp);
            LinkIngestError::Storage(format!("提交 {} 失败: {error}", target.display()))
        })?;
        tracing::info!(
            job_id = %job.job_id,
            url = %job.url,
            title = job.title.as_deref().unwrap_or(""),
            mode = job.mode.as_deref().unwrap_or(""),
            file = %target.display(),
            "link job queued"
        );
        Ok(QueuedLinkJob {
            job_id: job.job_id,
            file: target.display().to_string(),
            status: LINK_JOB_STATUS_NEEDS_115,
            duplicate: false,
        })
    }

    /// Find a job for `url` that is either still waiting in the queue or
    /// already recorded in the state file. Used to make ingest idempotent.
    fn find_existing_link_job(&self, url: &str) -> Option<QueuedLinkJob> {
        for entry in read_dir_entries(&self.config.link_queue_dir()).ok()? {
            if entry.extension().is_none_or(|extension| !extension.eq_ignore_ascii_case("json")) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&entry) else {
                continue;
            };
            let Ok(job) = serde_json::from_str::<LinkJob>(&text) else {
                continue;
            };
            if job.url == url {
                return Some(QueuedLinkJob {
                    job_id: job.job_id,
                    file: entry.display().to_string(),
                    status: LINK_JOB_STATUS_NEEDS_115,
                    duplicate: true,
                });
            }
        }
        let Ok(text) = std::fs::read_to_string(self.config.link_state_file()) else {
            return None;
        };
        // Newest record last: walk backwards so the most recent match wins.
        for line in text.lines().rev() {
            let Ok(record) = serde_json::from_str::<LinkStateLine>(line) else {
                continue;
            };
            if record.url == url && record.status != LINK_JOB_STATUS_INVALID {
                return Some(QueuedLinkJob {
                    job_id: record.job_id,
                    file: self
                        .config
                        .link_done_dir()
                        .join(&record.job_file)
                        .display()
                        .to_string(),
                    status: LINK_JOB_STATUS_NEEDS_115,
                    duplicate: true,
                });
            }
        }
        None
    }

    /// Consume the JSON job files in `链接队列/`.
    ///
    /// The 115 share-link transfer client (plan U11) does not exist yet, so an
    /// accepted job is **not** transferred or downloaded: it is recorded in
    /// `链接队列/状态.jsonl` with the `needs_115_client` status, moved into
    /// `链接队列/已处理/` and logged. Junk is quarantined in
    /// `链接队列/无效/`. Nothing outside the archive root is touched.
    pub(crate) fn ingest_link_queue(&self, dry_run: bool) -> Result<LinkQueueReport, String> {
        let queue = self.config.link_queue_dir();
        let entries = read_dir_entries(&queue)?;
        let mut report = LinkQueueReport {
            dry_run,
            ..LinkQueueReport::default()
        };
        for entry in entries {
            let name = entry
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            // `状态.jsonl` shares the directory but is not a job file; the
            // `已处理/` and `无效/` sub-directories are skipped below.
            if name.starts_with('.') || !name.ends_with(".json") {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&entry) else {
                continue;
            };
            if !metadata.file_type().is_file() {
                continue;
            }
            report.examined += 1;
            let text = match std::fs::read_to_string(&entry) {
                Ok(text) => text,
                Err(error) => {
                    // Unreadable right now (permissions, transient IO): retry
                    // next run instead of quarantining a good job.
                    report.skipped_incomplete.push(format!("{name} ({error})"));
                    continue;
                }
            };
            let job = match serde_json::from_str::<LinkJob>(&text) {
                Ok(job) => match job.accept_queued() {
                    Ok(job) => job,
                    Err(reason) => {
                        self.reject_link_job(&mut report, &entry, &name, &reason, dry_run);
                        continue;
                    }
                },
                Err(error) => {
                    // A foreign writer may still be writing the file; only a
                    // settled file that still does not parse is junk.
                    if !entry_is_settled(&entry, self.config.settle_seconds) {
                        report.skipped_incomplete.push(name);
                        continue;
                    }
                    let reason = format!("任务文件不是合法的 JSON: {error}");
                    self.reject_link_job(&mut report, &entry, &name, &reason, dry_run);
                    continue;
                }
            };
            if dry_run {
                report.accepted.push(AcceptedLinkJob {
                    job_id: job.job_id,
                    url: job.url,
                    file: name,
                    status: LINK_JOB_STATUS_NEEDS_115,
                });
                continue;
            }
            let record = LinkJobStateRecord {
                recorded_at_unix: unix_now(),
                status: LINK_JOB_STATUS_NEEDS_115,
                job_id: job.job_id.clone(),
                url: job.url.clone(),
                title: job.title.clone().unwrap_or_default(),
                mode: job.mode.clone().unwrap_or_default(),
                source: job.source.clone().unwrap_or_default(),
                reason: LINK_JOB_NEEDS_115_REASON.to_owned(),
                job_file: name.clone(),
            };
            let state_path = self.config.link_state_file();
            append_link_state(&state_path, &record).map_err(|error| {
                format!("写链接状态文件 {} 失败: {error}", state_path.display())
            })?;
            let done = unique_target(&self.config.link_done_dir(), &name);
            move_entry(&entry, &done).map_err(|error| {
                format!("移动任务文件 {name} 失败: {error}")
            })?;
            tracing::warn!(
                job_id = %record.job_id,
                url = %record.url,
                title = %record.title,
                mode = %record.mode,
                "link job needs the 115 transfer client (not implemented): recorded, nothing downloaded"
            );
            report.accepted.push(AcceptedLinkJob {
                job_id: record.job_id,
                url: record.url,
                file: done.display().to_string(),
                status: LINK_JOB_STATUS_NEEDS_115,
            });
        }
        Ok(report)
    }

    fn reject_link_job(
        &self,
        report: &mut LinkQueueReport,
        entry: &Path,
        name: &str,
        reason: &str,
        dry_run: bool,
    ) {
        report.invalid.push(FailedItem {
            source: name.to_owned(),
            reason: reason.to_owned(),
        });
        tracing::warn!(file = %name, reason, "link job rejected");
        if dry_run {
            return;
        }
        let record = LinkJobStateRecord {
            recorded_at_unix: unix_now(),
            status: LINK_JOB_STATUS_INVALID,
            job_id: String::new(),
            url: String::new(),
            title: String::new(),
            mode: String::new(),
            source: String::new(),
            reason: reason.to_owned(),
            job_file: name.to_owned(),
        };
        if let Err(error) = append_link_state(&self.config.link_state_file(), &record) {
            tracing::warn!(%error, file = %name, "failed to record a rejected link job");
        }
        let target = unique_target(&self.config.link_invalid_dir(), name);
        if let Err(error) = move_entry(entry, &target) {
            tracing::warn!(%error, file = %name, "failed to quarantine a rejected link job");
        }
    }

    /// Wash-mode support: how big is the copy we already have? Name-based and
    /// metadata-only (no content is read): the biggest directory under
    /// `媒体目录/` whose name looks like the requested title wins.
    pub(crate) fn lookup_library_size(&self, title: &str) -> LibraryLookup {
        let mut lookup = LibraryLookup {
            title: title.trim().to_owned(),
            ..LibraryLookup::default()
        };
        let query = normalize_title_key(&lookup.title);
        if query.is_empty() {
            return lookup;
        }
        let mut best: Option<(String, u64)> = None;
        collect_library_entries(&self.config.media_dir(), 0, &query, &mut best);
        if let Some((name, size)) = best {
            lookup.matched = Some(name);
            lookup.size_bytes = Some(size);
        }
        lookup
    }

    pub(crate) fn status(&self) -> ArchiveStatus {
        ArchiveStatus {
            root: self.config.root.display().to_string(),
            transfer_dir: self.config.transfer_dir().display().to_string(),
            pending_dir: self.config.pending_dir().display().to_string(),
            media_dir: self.config.media_dir().display().to_string(),
            failed_dir: self.config.failed_dir().display().to_string(),
            duplicate_dir: self.config.duplicate_dir().display().to_string(),
            wash_dir: self.config.wash_dir().display().to_string(),
            link_queue_dir: self.config.link_queue_dir().display().to_string(),
            link_state_file: self.config.link_state_file().display().to_string(),
            pending_link_jobs: count_link_jobs(&self.config.link_queue_dir()),
            settle_seconds: self.config.settle_seconds,
            wash_enabled: self.config.wash_enabled,
            wash_tolerance_ratio: self.config.wash_tolerance_ratio,
            watch_dirs: self.config
                .watch_dirs
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            tmdb_configured: self.config.tmdb_api_key.is_some(),
        }
    }
}

/// Auto-run interval for the archive pipeline. Disabled unless
/// `LUX_ARCHIVE_POLL_SECONDS` is set to a positive number.
fn archive_poll_interval() -> Option<Duration> {
    let seconds = std::env::var(ARCHIVE_POLL_SECONDS_ENV)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

/// Start the background archive poller when `LUX_ARCHIVE_POLL_SECONDS` is set.
/// Files dropped into the transfer directory are then archived and organized
/// without an admin request. Called once from the server startup.
pub fn start_poller() {
    let Some(interval) = archive_poll_interval() else {
        return;
    };
    let service = ArchiveService::new(ArchiveConfig::from_env());
    if let Err(error) = service.config.ensure_directories() {
        tracing::warn!(%error, "archive pipeline directories could not be created");
        return;
    }
    tracing::info!(
        root = %service.config.root.display(),
        interval_secs = interval.as_secs(),
        "archive pipeline poller started"
    );
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            match service.run_once(false).await {
                Ok(report)
                    if !report.archived_to_pending.is_empty()
                        || !report.organized.is_empty()
                        || !report.failed.is_empty()
                        || !report.duplicates.is_empty() =>
                {
                    tracing::info!(
                        archived = report.archived_to_pending.len(),
                        organized = report.organized.len(),
                        failed = report.failed.len(),
                        duplicates = report.duplicates.len(),
                        washed = report.washed.len(),
                        "archive pipeline run completed"
                    );
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "archive pipeline run failed"),
            }
        }
    });
}

fn tmdb_match_from(
    segment: &str,
    id: i64,
    summary: &serde_json::Value,
    details: &serde_json::Value,
) -> Result<TmdbMatch, String> {
    let kind = if segment == "tv" {
        MediaKind::Series
    } else {
        MediaKind::Movie
    };
    let title_key = if kind == MediaKind::Series { "name" } else { "title" };
    let date_key = if kind == MediaKind::Series {
        "first_air_date"
    } else {
        "release_date"
    };
    let title = details
        .get(title_key)
        .and_then(|value| value.as_str())
        .or_else(|| summary.get(title_key).and_then(|value| value.as_str()))
        .map(str::to_owned)
        .ok_or_else(|| "TMDB 返回缺少标题".to_owned())?;
    let year = details
        .get(date_key)
        .and_then(|value| value.as_str())
        .and_then(|date| date.get(..4))
        .and_then(|year| year.parse::<i64>().ok())
        .or_else(|| {
            summary
                .get(date_key)
                .and_then(|value| value.as_str())
                .and_then(|date| date.get(..4))
                .and_then(|year| year.parse::<i64>().ok())
        })
        .or_else(|| details.get("_fallback_year").and_then(|value| value.as_i64()));
    let origin_countries = details
        .get("origin_country")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let original_language = details
        .get("original_language")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let genres = details
        .get("genres")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| {
                    value
                        .get("name")
                        .and_then(|name| name.as_str())
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    let release_country = if kind == MediaKind::Movie {
        details
            .get("_release_country")
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    } else {
        // Series have no per-country release dates in TMDB; origin_country is
        // the country of first airing.
        details
            .get("origin_country")
            .and_then(|value| value.as_array())
            .and_then(|values| values.first())
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    };
    Ok(TmdbMatch {
        kind,
        tmdb_id: id,
        title,
        year,
        origin_countries,
        release_country,
        original_language,
        genres,
    })
}

/// Earliest dated release (premiere / theatrical) across all countries:
/// returns `(normalized_date, country)`.
/// Fallback year for a series whose `first_air_date` is empty: the earliest
/// dated season, then the last air date.
fn season_or_last_air_year(details: &serde_json::Value) -> Option<i64> {
    let from_seasons = details
        .get("seasons")
        .and_then(|value| value.as_array())
        .and_then(|seasons| {
            seasons
                .iter()
                .filter_map(|season| {
                    season
                        .get("air_date")
                        .and_then(|value| value.as_str())
                        .filter(|date| !date.is_empty())
                })
                .min()
        });
    let date = from_seasons.or_else(|| {
        details
            .get("last_air_date")
            .and_then(|value| value.as_str())
            .filter(|date| !date.is_empty())
    })?;
    date.get(..4).and_then(|year| year.parse::<i64>().ok())
}

fn earliest_release(details: &serde_json::Value) -> Option<(String, String)> {
    let results = details.get("results")?.as_array()?;
    let mut best: Option<(String, String)> = None;
    for country_entry in results {
        let Some(country) = country_entry.get("iso_3166_1").and_then(|value| value.as_str()) else {
            continue;
        };
        let Some(release_dates) = country_entry.get("release_dates").and_then(|value| value.as_array())
        else {
            continue;
        };
        for release in release_dates {
            // 1 premiere, 2 theatrical (limited), 3 theatrical, 4 digital, 5 physical
            let release_type = release.get("type").and_then(|value| value.as_i64()).unwrap_or(3);
            if !matches!(release_type, 1..=3) {
                continue;
            }
            let Some(date) = release.get("release_date").and_then(|value| value.as_str()) else {
                continue;
            };
            let normalized = normalize_release_date(date);
            let candidate = (normalized.clone(), country.to_owned());
            if best.as_ref().is_none_or(|(current, _)| normalized < *current) {
                best = Some(candidate);
            }
        }
    }
    best
}

fn normalize_release_date(date: &str) -> String {
    // TMDB returns `YYYY-MM-DDTHH:MM:SS.mmmZ`; keep the date part only, then pad
    // partial dates (YYYY / YYYY-MM) so string ordering is a date order.
    let date = date
        .split(['T', ' '])
        .next()
        .unwrap_or(date);
    let parts = date.split('-').collect::<Vec<_>>();
    let mut normalized = String::new();
    for index in 0..3 {
        if index > 0 {
            normalized.push('-');
        }
        normalized.push_str(parts.get(index).copied().unwrap_or("01"));
    }
    normalized
}

fn pick_best_result<'a>(
    results: &'a [serde_json::Value],
    title: &str,
    year: Option<i64>,
) -> Option<&'a serde_json::Value> {
    let wanted = title.to_lowercase();
    let score = |result: &serde_json::Value| -> i64 {
        let names = ["title", "name", "original_title", "original_name"]
            .iter()
            .filter_map(|key| result.get(*key).and_then(|value| value.as_str()))
            .map(|value| value.to_lowercase())
            .collect::<Vec<_>>();
        let mut score = 0;
        if names.iter().any(|name| name == &wanted) {
            score += 100;
        } else if names
            .iter()
            .any(|name| name.starts_with(&wanted) || wanted.starts_with(name))
        {
            score += 40;
        }
        if let Some(year) = year {
            let candidate = result
                .get("release_date")
                .and_then(|value| value.as_str())
                .or_else(|| result.get("first_air_date").and_then(|value| value.as_str()))
                .and_then(|date| date.get(..4))
                .and_then(|value| value.parse::<i64>().ok());
            if candidate == Some(year) {
                score += 50;
            }
        }
        score
    };
    results
        .iter()
        .filter(|result| score(result) > 0)
        .max_by_key(|result| score(result))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExistingDecision {
    /// The library already holds the same version: keep it, park the newcomer.
    Duplicate,
    /// The newcomer is meaningfully bigger: retire the old pack and keep this one.
    Wash,
}

/// Chill's `wash_by_equivalent_size` rule: a pack within the tolerance ratio of
/// the library copy is the same version; a meaningfully larger pack replaces it;
/// anything smaller keeps the library copy.
fn decide_existing(
    incoming_size: u64,
    existing_size: u64,
    wash_enabled: bool,
    tolerance_ratio: f64,
) -> ExistingDecision {
    if !wash_enabled || incoming_size == 0 || existing_size == 0 {
        return ExistingDecision::Duplicate;
    }
    let tolerance = existing_size as f64 * tolerance_ratio;
    let delta = incoming_size as f64 - existing_size as f64;
    if delta > tolerance {
        ExistingDecision::Wash
    } else {
        ExistingDecision::Duplicate
    }
}

/// What should happen to one season of a series pack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SeasonAction {
    /// The library has no such season: merge it in.
    Add,
    /// The library copy is the same version: park the incoming season.
    Duplicate,
    /// The incoming season is meaningfully bigger: retire the library copy.
    Wash,
}

#[derive(Clone, Debug)]
struct SeasonPlanItem {
    season: u32,
    source: PathBuf,
    target: PathBuf,
    action: SeasonAction,
    incoming_size: u64,
    /// Size of the library season; `0` when the library has no such season.
    existing_size: u64,
}

fn season_dir_name(season: u32) -> String {
    format!("Season {season:02}")
}

/// `Season 02`, `Season 2`, `season02`, `S02` and `S2` all name season 2.
fn season_number_from_dir(name: &str) -> Option<u32> {
    let lowered = name.trim().to_ascii_lowercase();
    let rest = if let Some(rest) = lowered.strip_prefix("season") {
        rest.trim_start_matches([' ', '_', '-', '.'])
    } else if let Some(rest) = lowered.strip_prefix('s') {
        rest
    } else {
        return None;
    };
    let digits: String = rest
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Season subdirectories of a release pack that actually hold video, sorted by
/// season number so the report and the moves stay deterministic.
fn season_dirs(pack: &Path) -> Vec<(u32, PathBuf)> {
    let Ok(reader) = std::fs::read_dir(pack) else {
        return Vec::new();
    };
    let mut seasons: Vec<(u32, PathBuf)> = reader
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() || !dir_contains_video(&path) {
                return None;
            }
            let name = path.file_name()?.to_string_lossy().to_string();
            Some((season_number_from_dir(&name)?, path))
        })
        .collect();
    seasons.sort_by_key(|(season, _)| *season);
    seasons
}

/// Decide every season of a pack on its own. `planned` carries the dry-run plan
/// of the current batch, so a season an earlier entry would have added counts
/// as present.
fn plan_season_pack(
    pack: &Path,
    title_dir: &Path,
    wash_enabled: bool,
    tolerance_ratio: f64,
    planned: &HashMap<PathBuf, u64>,
) -> Vec<SeasonPlanItem> {
    season_dirs(pack)
        .into_iter()
        .map(|(season, source)| {
            let target = title_dir.join(season_dir_name(season));
            let incoming_size = path_size(&source);
            let existing_size = if target.exists() {
                Some(path_size(&target))
            } else {
                planned.get(&target).copied()
            };
            match existing_size {
                None => SeasonPlanItem {
                    season,
                    source,
                    target,
                    action: SeasonAction::Add,
                    incoming_size,
                    existing_size: 0,
                },
                Some(existing_size) => {
                    let action =
                        if decide_existing(incoming_size, existing_size, wash_enabled, tolerance_ratio)
                            == ExistingDecision::Wash
                        {
                            SeasonAction::Wash
                        } else {
                            SeasonAction::Duplicate
                        };
                    SeasonPlanItem {
                        season,
                        source,
                        target,
                        action,
                        incoming_size,
                        existing_size,
                    }
                }
            }
        })
        .collect()
}

fn organized_item(
    source: String,
    matched: &TmdbMatch,
    region: &str,
    year: Option<i64>,
    target: &Path,
) -> OrganizedItem {
    OrganizedItem {
        source,
        tmdb_id: matched.tmdb_id,
        target: target.display().to_string(),
        kind: matched.category_label().to_owned(),
        region: region.to_owned(),
        year,
        title: matched.title.clone(),
    }
}

/// Total size of a file or directory tree (metadata only, no data read).
fn path_size(path: &Path) -> u64 {
    let Ok(metadata) = std::fs::metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    let Ok(reader) = std::fs::read_dir(path) else {
        return 0;
    };
    reader
        .flatten()
        .map(|entry| path_size(&entry.path()))
        .sum()
}

/// A release pack counts as media when it holds a video file up to two levels
/// deep (pack root or a season subdirectory).
fn dir_contains_video(directory: &Path) -> bool {
    let Ok(reader) = std::fs::read_dir(directory) else {
        return false;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if dir_contains_video(&path) {
                return true;
            }
            continue;
        }
        if video_extension(&path).is_some() {
            return true;
        }
    }
    false
}

/// A pack is treated as a series when it has a season directory or an
/// episode-named file inside, or when the name itself carries SxxEyy/第N集.
fn dir_signals_series(directory: &Path, parsed: &ParsedName) -> bool {
    if parsed.is_episode() {
        return true;
    }
    let Ok(reader) = std::fs::read_dir(directory) else {
        return false;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if name.starts_with("season") || name.starts_with('s') && name[1..].chars().all(|c| c.is_ascii_digit()) {
                return true;
            }
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if stem.to_ascii_lowercase().contains('e') && parse_season_episode(stem).is_some() {
            return true;
        }
    }
    false
}

/// 晚会/演唱会 are not TMDB categories: they are detected from the entry
/// name (an explicit `{晚会}` / `{演唱会}` marker wins over keywords, and a
/// `{tmdb-ID}` hint is handled before this check by the caller).
pub(crate) fn special_category(name: &str) -> Option<&'static str> {
    if name.contains("{晚会}") || name.contains("【晚会】") {
        return Some("晚会");
    }
    if name.contains("{演唱会}") || name.contains("【演唱会】") {
        return Some("演唱会");
    }
    // Olympic/parade/season galas and other one-off events.
    const GALA: &[&str] = &[
        "开幕式", "闭幕式", "阅兵", "晚会", "春晚", "中秋", "元宵", "跨年", "国庆", "联欢",
        "颁奖典礼", "庆典",
    ];
    if GALA.iter().any(|keyword| name.contains(keyword)) {
        return Some("晚会");
    }
    const CONCERT: &[&str] = &["演唱会", "音乐会", "巡演", "concert", "live"];
    let lowered = name.to_lowercase();
    if CONCERT.iter().any(|keyword| lowered.contains(keyword)) {
        return Some("演唱会");
    }
    None
}

/// Whether a title contains CJK characters (used as the fallback region for
/// concerts, which TMDB does not describe).
pub(crate) fn contains_cjk(value: &str) -> bool {
    value.chars().any(|character| {
        matches!(character,
            '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}')
    })
}

fn is_dir_empty(path: &Path) -> bool {
    std::fs::read_dir(path)
        .map(|mut reader| reader.next().is_none())
        .unwrap_or(false)
}

/// Keep the release name but avoid overwriting an existing entry.
fn unique_target(directory: &Path, name: &str) -> PathBuf {
    let candidate = directory.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_owned());
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_string());
    for index in 2..1000 {
        let candidate = match &extension {
            Some(extension) => directory.join(format!("{stem} ({index}).{extension}")),
            None => directory.join(format!("{name} ({index})")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    directory.join(format!("{stem} ({})", unix_now()))
}

fn dir_has_nfo(directory: &Path) -> bool {
    let Ok(reader) = std::fs::read_dir(directory) else {
        return false;
    };
    reader.flatten().any(|entry| {
        entry
            .path()
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("nfo"))
    })
}

fn read_dir_entries(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries = Vec::new();
    let reader = match std::fs::read_dir(directory) {
        Ok(reader) => reader,
        // A missing role directory means "nothing there yet"; a dry run must
        // not create it just to list an empty directory.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
        Err(error) => return Err(format!("读取 {} 失败: {error}", directory.display())),
    };
    for entry in reader.flatten() {
        entries.push(entry.path());
    }
    entries.sort();
    Ok(entries)
}

fn is_ignored_entry(name: &str) -> bool {
    name.starts_with('.') || name == "原因.txt" || name.ends_with(".part") || name.ends_with(".tmp")
}

fn entry_is_settled(path: &Path, settle_seconds: u64) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age.as_secs() >= settle_seconds)
        .unwrap_or(true)
}

fn video_extension(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    VIDEO_EXTENSIONS
        .iter()
        .copied()
        .find(|candidate| *candidate == extension)
}

fn stem_of(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !extension.contains('/') => stem,
        _ => name,
    }
}

fn sanitize_component(value: &str) -> String {
    let cleaned = value
        .chars()
        .map(|character| match character {
            ':' => '：',
            '/' | '\\' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            other => other,
        })
        .collect::<String>();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed =
        cleaned.trim_matches(|character: char| character == '.' || character.is_whitespace());
    if trimmed.is_empty() {
        "未命名".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn move_entry(source: &Path, target: &Path) -> std::io::Result<()> {
    if !source.exists() {
        // Already moved by someone else (poller race): nothing left to do.
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(error) if error.raw_os_error() == Some(18) => {
            // Cross-device: copy then remove.
            copy_recursive(source, target)?;
            remove_recursive(source)
        }
        Err(error) => Err(error),
    }
}

fn copy_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        std::fs::create_dir_all(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &target.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(source, target).map(|_| ())
    }
}

fn remove_recursive(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn urlencode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            b' ' => encoded.push_str("%20"),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// Parse `Title (2024)`, `Title.2024`, `Title.2024.S01E02`, `[Group] Title [2024]`
/// and similar release names into title, year, season and episode.
pub(crate) fn parse_media_name(raw: &str) -> Option<ParsedName> {
    let mut text = raw.trim().to_string();
    if text.is_empty() {
        return None;
    }
    // `{tmdb-285322}`-style hints name the TMDB id directly; the id is more
    // reliable than title matching, and the token must not leak into the title.
    let tmdb_id = extract_tmdb_id(&text);
    text = strip_tmdb_tokens(&text);
    text = text
        .replace("{晚会}", " ")
        .replace("【晚会】", " ")
        .replace("{演唱会}", " ")
        .replace("【演唱会】", " ");
    // Drop leading bracketed groups like [Group] or 【字幕组】.
    while let Some(rest) = strip_leading_group(&text) {
        text = rest;
    }
    // Normalize separators: dots/underscores between title words are common in
    // release names, but Chinese titles may legitimately contain them.
    let has_cjk = text.chars().any(is_cjk);
    if !has_cjk {
        text = text.replace(['.', '_'], " ");
    }
    let tokens = text.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    let mut season = None;
    let mut episode = None;
    let mut year = None;
    let mut year_index = None;
    for (index, token) in tokens.iter().enumerate() {
        if let Some((parsed_season, parsed_episode)) = parse_season_episode(token) {
            season = Some(parsed_season);
            episode = Some(parsed_episode);
            continue;
        }
        if let Some(parsed_episode) = parse_chinese_episode(token) {
            season.get_or_insert(1);
            episode = Some(parsed_episode);
            continue;
        }
        if let Some(parsed_year) = parse_year_token(token) {
            year = Some(parsed_year);
            year_index = Some(index);
            continue;
        }
    }
    // Release names put the title before the year: `Title.Year.tags...`. Once
    // a year is known, everything after it is release metadata, not the title.
    let title_tokens = match year_index {
        Some(index) => &tokens[..index],
        None => &tokens[..],
    };
    let title = title_tokens
        .iter()
        .filter(|token| {
            !is_noise_token(token)
                && parse_season_episode(token).is_none()
                && parse_chinese_episode(token).is_none()
        })
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        return None;
    }
    Some(ParsedName {
        title,
        tmdb_id,
        year,
        season,
        episode,
    })
}

/// Extract `tmdb-123456` / `tmdb=123456` / `tmdbid:123456` hints.
fn extract_tmdb_id(text: &str) -> Option<i64> {
    let lower = text.to_ascii_lowercase();
    for marker in ["tmdb-", "tmdb=", "tmdb:", "tmdbid-", "tmdbid=", "tmdbid:"] {
        if let Some(start) = lower.find(marker) {
            let digits = lower[start + marker.len()..]
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .collect::<String>();
            if let Ok(id) = digits.parse::<i64>() {
                return Some(id);
            }
        }
    }
    None
}

/// Remove `{tmdb-...}` / `[tmdb-...]` tokens from a name so they do not end up
/// inside the title.
fn strip_tmdb_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(position) = lower.find("tmdb") else {
            out.push_str(rest);
            break;
        };
        // Walk back to the opening bracket of the token, if any.
        let prefix = &rest[..position];
        let open = prefix
            .rfind(['{', '['])
            .filter(|index| prefix[*index + 1..].trim().is_empty());
        match open {
            Some(index) => {
                out.push_str(&prefix[..index]);
                // Skip to the matching closing bracket.
                let tail = &rest[position..];
                let Some(close) = tail.find(['}', ']']) else {
                    out.push_str(tail);
                    break;
                };
                rest = &tail[close + 1..];
            }
            None => {
                out.push_str(prefix);
                // Bare `tmdb-123` token: skip the token itself.
                let tail = &rest[position..];
                let end = tail
                    .find(|character: char| character.is_whitespace())
                    .unwrap_or(tail.len());
                rest = &tail[end..];
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_leading_group(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    let (rest, closing) = if let Some(rest) = trimmed.strip_prefix('[') {
        (rest, ']')
    } else if let Some(rest) = trimmed.strip_prefix('【') {
        (rest, '】')
    } else {
        return None;
    };
    let end = rest.find(closing)?;
    Some(rest[end + closing.len_utf8()..].trim_start().to_string())
}

fn parse_season_episode(token: &str) -> Option<(u32, u32)> {
    let upper = token.to_ascii_uppercase();
    let start = upper.find('S')?;
    let rest = &upper[start + 1..];
    let digits_end = rest.find(|character: char| !character.is_ascii_digit())?;
    if digits_end == 0 {
        return None;
    }
    let season = rest[..digits_end].parse::<u32>().ok()?;
    let after = &rest[digits_end..];
    let after = after.strip_prefix('E')?;
    let episode_end = after
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(after.len());
    if episode_end == 0 {
        return None;
    }
    let episode = after[..episode_end].parse::<u32>().ok()?;
    Some((season, episode))
}

fn parse_chinese_episode(token: &str) -> Option<u32> {
    let trimmed = token
        .trim_start_matches('第')
        .trim_end_matches('集')
        .trim_end_matches('话')
        .trim_end_matches('話');
    if trimmed == token {
        return None;
    }
    trimmed.parse::<u32>().ok()
}

fn parse_year_token(token: &str) -> Option<i64> {
    let trimmed = token.trim_matches(|character: char| {
        character == '(' || character == ')' || character == '[' || character == ']'
    });
    if trimmed.len() == 4
        && trimmed.chars().all(|character| character.is_ascii_digit())
        && let Ok(year) = trimmed.parse::<i64>()
        && (1888..=2100).contains(&year)
    {
        return Some(year);
    }
    None
}

fn is_noise_token(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "bluray"
            | "bdrip"
            | "web"
            | "webrip"
            | "web-dl"
            | "webdl"
            | "hdtv"
            | "remux"
            | "x264"
            | "x265"
            | "h264"
            | "h265"
            | "hevc"
            | "avc"
            | "aac"
            | "ac3"
            | "dts"
            | "truehd"
            | "atmos"
            | "hdr"
            | "sdr"
            | "4k"
            | "1080p"
            | "720p"
            | "2160p"
            | "repack"
            | "proper"
            | "complete"
            | "internal"
            | "nf"
            | "sub"
            | "chs"
            | "cht"
            | "国语"
            | "中字"
            | "双语"
    )
}

fn is_cjk(character: char) -> bool {
    matches!(
        character as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x3040..=0x30FF | 0xAC00..=0xD7AF
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ArchiveStatus {
    pub(crate) root: String,
    pub(crate) transfer_dir: String,
    pub(crate) pending_dir: String,
    pub(crate) media_dir: String,
    pub(crate) failed_dir: String,
    pub(crate) duplicate_dir: String,
    pub(crate) wash_dir: String,
    /// `<root>/链接队列/`: where ingest jobs are written.
    #[serde(default)]
    pub(crate) link_queue_dir: String,
    /// `<root>/链接队列/状态.jsonl`: append-only job audit trail.
    #[serde(default)]
    pub(crate) link_state_file: String,
    /// Job files waiting in the queue right now.
    #[serde(default)]
    pub(crate) pending_link_jobs: usize,
    pub(crate) settle_seconds: u64,
    pub(crate) wash_enabled: bool,
    pub(crate) wash_tolerance_ratio: f64,
    pub(crate) watch_dirs: Vec<String>,
    pub(crate) tmdb_configured: bool,
}

/// Only real share links may enter the queue: a `file://` URL (or a crafted
/// `javascript:` one) must never reach a future transfer client.
pub(crate) fn validate_link_url(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err("url 不能为空".to_owned());
    }
    if url.chars().count() > LINK_JOB_MAX_URL_CHARS {
        return Err(format!(
            "url 过长（上限 {LINK_JOB_MAX_URL_CHARS} 字符）"
        ));
    }
    if url.chars().any(|character| character.is_control() || character.is_whitespace()) {
        return Err("url 不能含空白或控制字符".to_owned());
    }
    let lowered = url.to_ascii_lowercase();
    const ALLOWED: &[&str] = &["https://", "http://", "magnet:", "ed2k://"];
    if !ALLOWED.iter().any(|prefix| lowered.starts_with(prefix)) {
        return Err("不支持的链接协议（只接受 http(s) / magnet / ed2k）".to_owned());
    }
    Ok(url.to_owned())
}

/// Human-readable, file-name-safe job slug: alphanumerics and CJK kept,
/// everything else collapsed to `-`, capped at 40 characters.
fn link_slug(title: Option<&str>, url: &str) -> String {
    let source = title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(url);
    let mut slug = String::new();
    let mut length = 0;
    for character in source.chars() {
        if character.is_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            length += 1;
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
            length += 1;
        }
        if length >= 40 {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() {
        "link".to_owned()
    } else {
        slug
    }
}

/// Trim, drop control characters and cap the length (queue metadata only).
fn clean_optional_text(value: Option<String>, max_chars: usize) -> Option<String> {
    let value = value?;
    let cleaned: String = value
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed: String = cleaned.chars().take(max_chars).collect();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

fn count_link_jobs(directory: &Path) -> usize {
    let Ok(reader) = std::fs::read_dir(directory) else {
        return 0;
    };
    reader
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .count()
}

/// Append one JSON line to the job state file. `0600`: share links are private.
fn append_link_state(path: &Path, record: &LinkJobStateRecord) -> std::io::Result<()> {
    use std::io::Write;
    let mut line = serde_json::to_string(record).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(line.as_bytes())?;
    file.sync_all()
}

/// Comparison key for library titles: lowercase alphanumerics only, so
/// `某剧 第二季 (2024)` and `某剧.第二季` normalize alike.
fn normalize_title_key(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

/// Heuristic title match between a requested title and a library directory
/// name. Exact containment wins; otherwise a long enough common prefix counts,
/// because a Telegram title usually carries extra tags (`4K`, `HDR`, …) that
/// the library directory does not.
fn library_title_matches(query: &str, entry: &str) -> bool {
    if query.is_empty() || entry.is_empty() {
        return false;
    }
    if entry.contains(query) || query.contains(entry) {
        return true;
    }
    let common = query
        .chars()
        .zip(entry.chars())
        .take_while(|(left, right)| left == right)
        .count();
    let shorter = query.chars().count().min(entry.chars().count());
    shorter >= 4 && common * 5 >= shorter * 3
}

/// Walk `媒体目录/` (four levels: type / region / year / title, and the shorter
/// 晚会 chain) collecting the biggest directory whose name matches `query`.
fn collect_library_entries(
    directory: &Path,
    depth: usize,
    query: &str,
    best: &mut Option<(String, u64)>,
) {
    if depth > 4 {
        return;
    }
    let Ok(reader) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
        else {
            continue;
        };
        if library_title_matches(query, &normalize_title_key(&name)) {
            let size = path_size(&path);
            if best.as_ref().is_none_or(|(_, current)| size > *current) {
                *best = Some((name.clone(), size));
            }
        }
        collect_library_entries(&path, depth + 1, query, best);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_names() {
        let parsed = parse_media_name("Inception.2010.1080p.BluRay.x264").expect("parsed");
        assert_eq!(parsed.title, "Inception");
        assert_eq!(parsed.year, Some(2010));

        let parsed = parse_media_name("哪吒之魔童闹海 (2025)").expect("parsed");
        assert_eq!(parsed.title, "哪吒之魔童闹海");
        assert_eq!(parsed.year, Some(2025));

        let parsed = parse_media_name("[FRDS] The.Matrix.1999.REPACK").expect("parsed");
        assert_eq!(parsed.title, "The Matrix");
        assert_eq!(parsed.year, Some(1999));
    }

    #[test]
    fn parses_tmdb_id_hints() {
        let parsed = parse_media_name("暗潮之下 (2026) {tmdb-285322}").expect("parsed");
        assert_eq!(parsed.title, "暗潮之下");
        assert_eq!(parsed.year, Some(2026));
        assert_eq!(parsed.tmdb_id, Some(285322));
    }

    #[test]
    fn strips_tmdb_tokens_from_titles() {
        assert_eq!(strip_tmdb_tokens("Some Movie {tmdb-42}"), "Some Movie");
        assert_eq!(strip_tmdb_tokens("Movie [tmdb=42]"), "Movie");
    }

    #[test]
    fn parses_release_folder_names() {
        let parsed = parse_media_name("300.2006.2160p.BluRay.REMUX.HEVC.DTS-HD.MA.TrueHD.7.1.Atmos-FGT").expect("parsed");
        assert_eq!(parsed.title, "300");
        assert_eq!(parsed.year, Some(2006));
        assert!(parsed.tmdb_id.is_none());
    }

    #[test]
    fn parses_episode_names() {
        let parsed = parse_media_name("The Bear S03E05").expect("parsed");
        assert_eq!(parsed.title, "The Bear");
        assert_eq!(parsed.season, Some(3));
        assert_eq!(parsed.episode, Some(5));

        let parsed = parse_media_name("繁花 第02集").expect("parsed");
        assert_eq!(parsed.title, "繁花");
        assert_eq!(parsed.episode, Some(2));
    }

    #[test]
    fn separates_domestic_and_foreign_boards() {
        let domestic = TmdbMatch {
            kind: MediaKind::Movie,
            tmdb_id: 1,
            title: "流浪地球".to_owned(),
            year: Some(2019),
            origin_countries: vec!["CN".to_owned()],
            release_country: None,
            original_language: Some("zh".to_owned()),
            genres: vec!["科幻".to_owned()],
        };
        assert!(domestic.is_domestic());
        assert_eq!(domestic.category_label(), "电影");

        let foreign = TmdbMatch {
            kind: MediaKind::Series,
            tmdb_id: 2,
            title: "The Bear".to_owned(),
            year: Some(2022),
            origin_countries: vec!["US".to_owned()],
            release_country: None,
            original_language: Some("en".to_owned()),
            genres: vec!["剧情".to_owned()],
        };
        assert!(!foreign.is_domestic());
        assert_eq!(foreign.category_label(), "电视剧");

        let variety = TmdbMatch {
            kind: MediaKind::Series,
            tmdb_id: 3,
            title: "奔跑吧".to_owned(),
            year: Some(2014),
            origin_countries: vec!["CN".to_owned()],
            release_country: None,
            original_language: Some("zh".to_owned()),
            genres: vec!["综艺".to_owned()],
        };
        assert_eq!(variety.category_label(), "综艺");
    }

    #[test]
    fn earliest_release_decides_the_board() {
        let details = serde_json::json!({
            "results": [
                { "iso_3166_1": "US", "release_dates": [
                    { "type": 3, "release_date": "2006-12-09T00:00:00.000Z" }
                ]},
                { "iso_3166_1": "CN", "release_dates": [
                    { "type": 3, "release_date": "2019-02-05T00:00:00.000Z" }
                ]}
            ]
        });
        let (date, country) = earliest_release(&details).expect("release");
        assert_eq!(date, "2006-12-09");
        assert_eq!(country, "US");

        // A Chinese-first release lands on the domestic board.
        let details = serde_json::json!({
            "results": [
                { "iso_3166_1": "US", "release_dates": [
                    { "type": 3, "release_date": "2020-01-10T00:00:00.000Z" }
                ]},
                { "iso_3166_1": "CN", "release_dates": [
                    { "type": 2, "release_date": "2019-12-01T00:00:00.000Z" }
                ]}
            ]
        });
        let (_, country) = earliest_release(&details).expect("release");
        assert_eq!(country, "CN");

        // Digital/physical-only entries do not count as the release origin.
        let details = serde_json::json!({
            "results": [
                { "iso_3166_1": "CN", "release_dates": [
                    { "type": 5, "release_date": "2024-01-01T00:00:00.000Z" }
                ]},
                { "iso_3166_1": "US", "release_dates": [
                    { "type": 3, "release_date": "2023-06-01T00:00:00.000Z" }
                ]}
            ]
        });
        let (_, country) = earliest_release(&details).expect("release");
        assert_eq!(country, "US");
    }

    #[test]
    fn series_without_first_air_date_uses_the_season_list() {
        let details = serde_json::json!({
            "first_air_date": "",
            "last_air_date": "1999-05-26",
            "seasons": [
                { "season_number": 4, "air_date": "1999-03-29" }
            ]
        });
        assert_eq!(season_or_last_air_year(&details), Some(1999));

        let details = serde_json::json!({
            "first_air_date": "",
            "last_air_date": "2021-11-02",
            "seasons": []
        });
        assert_eq!(season_or_last_air_year(&details), Some(2021));

        let details = serde_json::json!({ "first_air_date": "", "seasons": [] });
        assert_eq!(season_or_last_air_year(&details), None);
    }

    #[test]
    fn release_country_overrides_origin_for_the_board() {
        let movie = TmdbMatch {
            kind: MediaKind::Movie,
            tmdb_id: 9,
            title: "Some Film".to_owned(),
            year: Some(2024),
            origin_countries: vec!["US".to_owned()],
            release_country: Some("CN".to_owned()),
            original_language: Some("en".to_owned()),
            genres: vec![],
        };
        // Released first in China -> domestic board even though it is a US film.
        assert!(movie.is_domestic());

        let movie = TmdbMatch {
            release_country: Some("US".to_owned()),
            ..movie
        };
        assert!(!movie.is_domestic());
    }

    #[test]
    fn unique_target_avoids_collisions() {
        let base = std::env::temp_dir().join(format!("lux-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        assert_eq!(
            unique_target(&base, "Movie.2020.1080p"),
            base.join("Movie.2020.1080p")
        );
        std::fs::write(base.join("Movie.2020.1080p"), b"x").unwrap();
        assert_eq!(
            unique_target(&base, "Movie.2020.1080p"),
            base.join("Movie.2020 (2).1080p")
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn wash_vs_duplicate_follows_the_size_rule() {
        // Same size (or within tolerance) -> duplicate, keep the library copy.
        assert_eq!(decide_existing(1000, 1000, true, 0.02), ExistingDecision::Duplicate);
        assert_eq!(decide_existing(1010, 1000, true, 0.02), ExistingDecision::Duplicate);
        // Meaningfully bigger -> wash, the old pack is retired.
        assert_eq!(decide_existing(1100, 1000, true, 0.02), ExistingDecision::Wash);
        // Smaller -> duplicate, the bigger library copy stays.
        assert_eq!(decide_existing(900, 1000, true, 0.02), ExistingDecision::Duplicate);
        // Wash disabled -> always duplicate.
        assert_eq!(decide_existing(5000, 1000, false, 0.02), ExistingDecision::Duplicate);
        // Unknown sizes -> duplicate rather than replacing blindly.
        assert_eq!(decide_existing(0, 1000, true, 0.02), ExistingDecision::Duplicate);
    }

    /// A file with an exact apparent size, so the size decisions are testable
    /// without writing real media.
    fn write_sized_file(path: &std::path::Path, size: usize) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, vec![b'x'; size]).unwrap();
    }

    fn series_tmdb_match() -> TmdbMatch {
        TmdbMatch {
            kind: MediaKind::Series,
            tmdb_id: 68678,
            title: "聊斋".to_owned(),
            year: Some(1996),
            origin_countries: vec!["CN".to_owned()],
            release_country: None,
            original_language: Some("zh".to_owned()),
            genres: Vec::new(),
        }
    }

    /// A series pack is judged season by season: a season the library already
    /// holds is parked as a duplicate, a missing one is merged in, even though
    /// the pack as a whole is not bigger than the library copy.
    #[test]
    fn merges_only_the_missing_season_of_a_pack() {
        let base = std::env::temp_dir().join(format!("lux-seasons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let pack = base
            .join("待整理")
            .join("聊斋 (1996) {tmdb-68678}");
        let title = base
            .join("媒体目录")
            .join("电视剧")
            .join("国内")
            .join("1996")
            .join("聊斋 (1996)");
        let s01 = "Dark Tales.S01E01.1996.1080p.mkv";
        let s02 = "Dark Tales.S02E01.1996.1080p.mkv";
        write_sized_file(&pack.join("Season 01").join(s01), 2000);
        write_sized_file(&pack.join("Season 02").join(s02), 3000);
        write_sized_file(&title.join("Season 01").join(s01), 2000);

        let planned = HashMap::new();
        let plan = plan_season_pack(&pack, &title, true, 0.02, &planned);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].season, 1);
        assert_eq!(plan[0].action, SeasonAction::Duplicate);
        assert_eq!(plan[1].season, 2);
        assert_eq!(plan[1].action, SeasonAction::Add);

        let service = ArchiveService::new_for_tests(archive_config(&base));
        let mut report = ArchiveReport {
            dry_run: false,
            ..ArchiveReport::default()
        };
        let merged = service
            .apply_season_plan(
                &mut report,
                &pack,
                &title,
                &plan,
                &series_tmdb_match(),
                "国内",
                Some(1996),
            )
            .expect("season plan applies");
        assert!(merged);
        // The missing season is in the library, the duplicate one is parked
        // under the pack name, and the emptied pack is gone.
        assert!(title.join("Season 02").join(s02).exists());
        assert!(base
            .join("重复目录")
            .join("聊斋 (1996) {tmdb-68678}")
            .join("Season 01")
            .join(s01)
            .exists());
        assert!(!pack.exists());
        assert_eq!(report.organized.len(), 1);
        assert_eq!(report.duplicates.len(), 1);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Same two seasons on both sides -> the whole pack stays one duplicate.
    #[test]
    fn plans_a_fully_duplicate_series_pack() {
        let base = std::env::temp_dir().join(format!("lux-seasons2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let pack = base.join("待整理").join("某剧");
        let title = base.join("媒体目录").join("电视剧").join("国内").join("2024").join("某剧 (2024)");
        for season in ["Season 01", "Season 02"] {
            write_sized_file(&pack.join(season).join("ep.mkv"), 1000);
            write_sized_file(&title.join(season).join("ep.mkv"), 1000);
        }
        let plan = plan_season_pack(&pack, &title, true, 0.02, &HashMap::new());
        assert_eq!(plan.len(), 2);
        assert!(plan.iter().all(|item| item.action == SeasonAction::Duplicate));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A bigger season in the pack washes the library season; a smaller one does
    /// not.
    #[test]
    fn a_bigger_season_washes_the_library_season() {
        let base = std::env::temp_dir().join(format!("lux-seasons3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let pack = base.join("待整理").join("某剧");
        let title = base.join("媒体目录").join("电视剧").join("国内").join("2024").join("某剧 (2024)");
        write_sized_file(&pack.join("Season 02").join("ep.mkv"), 5000);
        write_sized_file(&title.join("Season 02").join("ep.mkv"), 1000);
        let plan = plan_season_pack(&pack, &title, true, 0.02, &HashMap::new());
        assert_eq!(plan[0].action, SeasonAction::Wash);
        write_sized_file(&pack.join("Season 02").join("ep.mkv"), 1000);
        let plan = plan_season_pack(&pack, &title, true, 0.02, &HashMap::new());
        assert_eq!(plan[0].action, SeasonAction::Duplicate);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sanitizes_path_components() {
        // colon so titles like `20:23` survive.
        assert_eq!(sanitize_component("A/B: C*?"), "A B： C");
        assert_eq!(sanitize_component("20:23"), "20：23");
        assert_eq!(sanitize_component("  .  "), "未命名");
    }

    fn archive_config(root: &std::path::Path) -> ArchiveConfig {
        ArchiveConfig {
            root: root.to_path_buf(),
            tmdb_api_key: None,
            tmdb_language: "zh-CN".to_owned(),
            watch_dirs: Vec::new(),
            mirror_root: root.join("镜像库"),
            cd2_prefix: String::new(),
            cd2_container_mount: "/media/cloud".to_owned(),
            settle_seconds: 0,
            wash_enabled: true,
            wash_tolerance_ratio: 0.02,
        }
    }

    /// One link job as the Telegram sidecar posts it.
    fn link_job(url: &str) -> LinkJob {
        LinkJob {
            url: url.to_owned(),
            title: Some("某剧 第二季 4K".to_owned()),
            mode: Some("上新".to_owned()),
            source: Some("telegram".to_owned()),
            ..LinkJob::default()
        }
    }

    #[test]
    fn accepts_only_real_share_links() {
        assert_eq!(
            validate_link_url(" https://115.com/s/abc?password=1 ").unwrap(),
            "https://115.com/s/abc?password=1"
        );
        assert!(validate_link_url("http://example.com/a").is_ok());
        assert!(validate_link_url("magnet:?xt=urn:btih:abc").is_ok());
        assert!(validate_link_url("ed2k://|file|a.mkv|1|HASH|/").is_ok());
        assert!(validate_link_url("").is_err());
        assert!(validate_link_url("file:///etc/passwd").is_err());
        assert!(validate_link_url("javascript:alert(1)").is_err());
        assert!(validate_link_url("https://115.com/s/a b").is_err());
        let long = format!("https://115.com/s/{}", "a".repeat(3000));
        assert!(validate_link_url(&long).is_err());
    }

    #[test]
    fn normalizes_link_job_fields() {
        let job = LinkJob {
            url: " https://115.com/s/abc ".to_owned(),
            title: Some("  某剧\n 第二季  ".to_owned()),
            mode: Some(" 上新 ".to_owned()),
            ..LinkJob::default()
        }
        .normalized()
        .expect("valid job");
        assert_eq!(job.url, "https://115.com/s/abc");
        assert_eq!(job.title.as_deref(), Some("某剧 第二季"));
        assert_eq!(job.mode.as_deref(), Some("上新"));
        assert!(job.received_at_unix > 0);
        // `<毫秒时间戳>-<标题 slug>`: sortable, human readable, path safe.
        let (millis, slug) = job.job_id.split_once('-').expect("timestamp-slug");
        assert!(millis.chars().all(|character| character.is_ascii_digit()));
        assert_eq!(slug, "某剧-第二季");
        assert!(!job.job_id.contains('/') && !job.job_id.contains(".."));

        // A producer-supplied id is ignored, so it cannot escape the queue dir.
        let supplied = LinkJob {
            job_id: "../逃逸 id".to_owned(),
            url: "https://115.com/s/abc".to_owned(),
            ..LinkJob::default()
        }
        .normalized()
        .expect("valid job");
        assert!(!supplied.job_id.contains('/') && !supplied.job_id.contains('.'));
        // No title -> the URL supplies the slug.
        let untitled = LinkJob {
            url: "magnet:?xt=urn:btih:abc".to_owned(),
            ..LinkJob::default()
        }
        .normalized()
        .expect("valid job");
        assert!(untitled.job_id.ends_with("-magnet-xt-urn-btih-abc"));
    }

    #[test]
    fn queues_and_consumes_link_jobs() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let queued = service
            .enqueue_link_job(link_job("https://115.com/s/abc"))
            .expect("queued");
        assert!(Path::new(&queued.file).exists());
        assert_eq!(queued.status, LINK_JOB_STATUS_NEEDS_115);
        assert!(!queued.duplicate);
        assert_eq!(service.status().pending_link_jobs, 1);
        assert!(
            queued.file.ends_with("-某剧-第二季-4k.json"),
            "job file name: {}",
            queued.file
        );

        let report = service.ingest_link_queue(false).expect("consumed");
        assert_eq!(report.examined, 1);
        assert_eq!(report.accepted.len(), 1);
        assert_eq!(report.accepted[0].status, LINK_JOB_STATUS_NEEDS_115);
        assert!(report.invalid.is_empty());

        // Nothing was transferred or downloaded: the queue is empty and no
        // media/pending directory was created.
        assert_eq!(service.status().pending_link_jobs, 0);
        assert!(!tmp.path().join(TRANSFER_DIR).exists());
        assert!(!tmp.path().join(PENDING_DIR).exists());
        assert!(!tmp.path().join(MEDIA_DIR).exists());

        // The job is recorded once, with the reason the operator needs, and the
        // record keeps the id the file was named after.
        let state = std::fs::read_to_string(service.config.link_state_file()).expect("state file");
        assert_eq!(state.lines().count(), 1);
        assert!(state.contains(LINK_JOB_STATUS_NEEDS_115));
        assert!(state.contains("https://115.com/s/abc"));
        assert!(state.contains(LINK_JOB_NEEDS_115_REASON));
        assert!(
            state.contains(&queued.job_id),
            "state record must keep job id {}: {state}",
            queued.job_id
        );

        // Consuming twice is a no-op: the file moved into 已处理/.
        let again = service.ingest_link_queue(false).expect("consumed");
        assert_eq!(again.examined, 0);
        assert_eq!(
            std::fs::read_dir(service.config.link_done_dir())
                .expect("done dir")
                .count(),
            1
        );
    }

    #[test]
    fn link_ingest_is_idempotent_on_the_url() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));

        let first = service
            .enqueue_link_job(link_job("https://115.com/s/abc"))
            .expect("queued");
        // Same link while it is still waiting -> no second file.
        let again = service
            .enqueue_link_job(link_job("https://115.com/s/abc"))
            .expect("queued");
        assert!(again.duplicate);
        assert_eq!(again.job_id, first.job_id);
        assert_eq!(service.status().pending_link_jobs, 1);

        service.ingest_link_queue(false).expect("consumed");
        // Same link after consumption -> still no second file.
        let third = service
            .enqueue_link_job(link_job("https://115.com/s/abc"))
            .expect("queued");
        assert!(third.duplicate);
        assert_eq!(third.job_id, first.job_id);
        assert_eq!(service.status().pending_link_jobs, 0);

        // A different link is queued normally.
        let other = service
            .enqueue_link_job(link_job("https://115.com/s/def"))
            .expect("queued");
        assert!(!other.duplicate);
        assert_eq!(service.status().pending_link_jobs, 1);
    }

    #[test]
    fn quarantines_unusable_link_jobs() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let queue = service.config.link_queue_dir();
        std::fs::create_dir_all(&queue).expect("queue");
        std::fs::write(queue.join("broken.json"), b"not json").unwrap();
        std::fs::write(queue.join("evil.json"), br#"{"url":"file:///etc/passwd"}"#).unwrap();
        // `状态.jsonl` shares the directory but is not a job.
        std::fs::write(queue.join(LINK_QUEUE_STATE_FILE), b"{}\n").unwrap();

        let report = service.ingest_link_queue(false).expect("consumed");
        assert_eq!(report.examined, 2);
        assert_eq!(report.invalid.len(), 2);
        assert!(report.accepted.is_empty());
        assert_eq!(
            std::fs::read_dir(service.config.link_invalid_dir())
                .expect("invalid dir")
                .count(),
            2
        );
        let state = std::fs::read_to_string(service.config.link_state_file()).expect("state file");
        assert_eq!(
            state
                .lines()
                .filter(|line| line.contains(LINK_JOB_STATUS_INVALID))
                .count(),
            2
        );
    }

    #[test]
    fn link_queue_keeps_unsettled_junk_for_the_next_run() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let mut config = archive_config(tmp.path());
        config.settle_seconds = 3600;
        let service = ArchiveService::new_for_tests(config);
        let queue = service.config.link_queue_dir();
        std::fs::create_dir_all(&queue).expect("queue");
        std::fs::write(queue.join("writing.json"), b"{\"url\":").unwrap();

        let report = service.ingest_link_queue(false).expect("consumed");
        assert_eq!(report.examined, 1);
        assert_eq!(report.skipped_incomplete.len(), 1);
        assert!(report.invalid.is_empty());
        assert_eq!(service.status().pending_link_jobs, 1);
    }

    #[test]
    fn lookup_finds_the_library_copy() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let title_dir = tmp
            .path()
            .join(MEDIA_DIR)
            .join("电视剧")
            .join("国内")
            .join("2024")
            .join("某剧 第二季 (2024)");
        std::fs::create_dir_all(&title_dir).expect("title dir");
        std::fs::write(title_dir.join("S01E01.mkv"), vec![0u8; 1024]).unwrap();

        // The Telegram title carries extra tags the library directory does not.
        let lookup = service.lookup_library_size("某剧 第二季 4K");
        assert_eq!(lookup.matched.as_deref(), Some("某剧 第二季 (2024)"));
        assert_eq!(lookup.size_bytes, Some(1024));

        let missing = service.lookup_library_size("完全不同的剧");
        assert_eq!(missing.matched, None);
        assert_eq!(missing.size_bytes, None);
        assert_eq!(service.lookup_library_size("   ").size_bytes, None);
    }
}
