//! Archive pipeline: transfer -> pending -> media, with failure and duplicate
//! sinks. This is the first slice of the chill replacement work (plan U9):
//! resources land in the transfer directory, are moved to pending, are
//! identified through TMDB, and are organized into
//! `<media>/<类型>/<国内|国外>/<年代>/<Title (Year)>/`.
//!
//! The pipeline is deliberately directory-driven: every stage is a visible
//! directory role, so a failed identification can be inspected by hand and a
//! half-finished run leaves no hidden state.
//!
//! The matching rules are frozen in `docs/archive-pipeline.md`; change that
//! document first and then the code, never the other way around.

// TODO(I16): the archive admin endpoints parse `dry_run` / `overwrite` as
// `bool` only, so `?dry_run=1` answers 400 even though the old docs promised
// `1|0`. `docs/archive-pipeline.md` §9 now documents `true|false`; accepting
// `1|0` needs the query parser in `src/api/admin/admin_jobs.rs` (owned by the
// settings change), not this module.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    application::{libraries::LibraryService, scanner::ScanJobService},
    storage::Database,
};

mod auto_library;
mod nfo;
mod settings;

/// Auto library reconciliation (docs/handoff/AUTO-LIBRARY.md): the library
/// services registered at startup, and how often a full pass re-runs even when
/// no archive run organized anything.
static AUTO_LIBRARY_SERVICES: OnceLock<auto_library::AutoLibraryServices> = OnceLock::new();
const AUTO_LIBRARY_RECONCILE_INTERVAL: Duration = Duration::from_secs(600);

pub(crate) use settings::{ArchiveSettings, mask_secret};

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
/// Fallback sources for the 115 transfer client (U11). The settings file wins
/// (D1); these envs only fill keys the file does not set. The cookie is a
/// secret: it may only ever appear in the settings file or this process's
/// environment, never in a log line or an API response.
pub(crate) const TRANSFER115_COOKIE_ENV: &str = "LUX_115_COOKIE";
pub(crate) const TRANSFER115_TARGET_DIR_ENV: &str = "LUX_115_TARGET_DIR";
pub(crate) const TRANSFER115_SIDECAR_BASE_ENV: &str = "LUX_115_SIDECAR_BASE";
pub(crate) const TRANSFER115_TIMEOUT_ENV: &str = "LUX_115_TIMEOUT_SECONDS";
/// Metadata lookups run concurrently; moves stay sequential so duplicate and
/// wash decisions see the batch in name order.
const METADATA_RESOLVE_CONCURRENCY: usize = 8;
/// How often an idle poller re-reads `poll_seconds` while polling is disabled,
/// so turning it on in the settings file starts working without a restart.
const POLL_DISABLED_RECHECK_SECONDS: u64 = 5;

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
/// U11: the job was handed to the transfer115 sidecar and the share was saved
/// into the target directory (it lands in 转存目录 through CD2 like any upload).
pub(crate) const LINK_JOB_STATUS_TRANSFERRED: &str = "transferred";
/// U11: the sidecar answered `auth_required` — the stored 115 cookie is dead or
/// missing. The job stays queued (no state line, no move) and retries after
/// the operator refreshes the cookie in the settings file.
pub(crate) const LINK_JOB_STATUS_AUTH_REQUIRED: &str = "auth_required";
/// U11: the sidecar answered `failed` (or was unreachable). Recorded and moved
/// to 已处理/ like any consumed job; re-deliver with `force: true`.
pub(crate) const LINK_JOB_STATUS_FAILED: &str = "failed";
pub(crate) const LINK_JOB_NEEDS_115_REASON: &str =
    "115 转存客户端未实现（U11）：链接已登记，未下载任何内容";
/// U11: the transfer stage is configured but this link kind is not handled by
/// the transfer115 sidecar (magnet / ed2k go to 115 cloud download, a later
/// slice). Recorded and moved to 已处理/; the link is not lost, just parked.
pub(crate) const LINK_JOB_UNSUPPORTED_REASON: &str =
    "转存侧车不支持该链接类型（magnet/ed2k 走 115 云下载，尚未实现）";
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
    /// U11: the 115 transfer client. `transfer115_base` empty means the
    /// transfer stage is off and jobs stay `needs_115_client`.
    pub(crate) transfer115_base: String,
    pub(crate) transfer115_cookie: String,
    /// Target directory for share transfers, relative to `root` (fixed rule).
    pub(crate) transfer115_target_dir: String,
    pub(crate) transfer115_timeout: Duration,
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
    /// Effective configuration: the editable settings file first, environment
    /// values for the keys the file does not set, then the built-in defaults.
    ///
    /// The archive root and the TMDB display language stay environment-only on
    /// purpose: they describe how the deployment is mounted, not how the
    /// pipeline behaves, and a wrong root would move files into the void.
    pub(crate) fn from_env() -> Self {
        let settings = ArchiveSettings::load();
        let root = std::env::var(ARCHIVE_ROOT_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_ARCHIVE_ROOT));
        let tmdb_language = std::env::var(ARCHIVE_TMDB_LANGUAGE_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "zh-CN".to_owned());
        Self {
            root,
            tmdb_api_key: (!settings.tmdb_api_key.is_empty()).then(|| settings.tmdb_api_key.clone()),
            tmdb_language,
            watch_dirs: settings.watch_dirs.iter().map(PathBuf::from).collect(),
            mirror_root: PathBuf::from(&settings.mirror_root),
            cd2_prefix: settings.cd2_prefix.clone(),
            cd2_container_mount: settings.cd2_mount.clone(),
            transfer115_base: settings.transfer115.sidecar_base.clone(),
            transfer115_cookie: settings.transfer115.cookie.clone(),
            transfer115_target_dir: settings.transfer115.target_dir.clone(),
            transfer115_timeout: Duration::from_secs(settings.transfer115.timeout_seconds),
            settle_seconds: settings.settle_seconds,
            wash_enabled: settings.wash_enabled,
            wash_tolerance_ratio: settings.wash_tolerance_ratio,
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
    /// Re-queue a URL that was already consumed. The idempotency key is the URL
    /// and it never expires on its own, so this is the explicit way to retry a
    /// link whose 115 transfer was lost.
    #[serde(default)]
    pub(crate) force: bool,
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
        self.mode = match clean_optional_text(self.mode, 32) {
            Some(mode) => Some(validate_link_mode(&mode)?),
            None => None,
        };
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
    /// U11: what the transfer115 sidecar reported for this job (empty when the
    /// stage is off or the job was not a 115 share link).
    #[serde(default)]
    pub(crate) transfer: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LinkQueueReport {
    pub(crate) dry_run: bool,
    pub(crate) examined: usize,
    pub(crate) accepted: Vec<AcceptedLinkJob>,
    pub(crate) invalid: Vec<FailedItem>,
    /// Jobs that could not be finished this round (state write or move failed).
    /// They stay queued and are retried by the next round; a failure here never
    /// stops the other jobs or the rest of the pipeline.
    pub(crate) failed: Vec<FailedItem>,
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
    /// U11: the transfer outcome the sidecar reported, kept so the ingest
    /// response can tell a failed transfer apart from a transferred one.
    #[serde(default)]
    transfer: String,
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
    /// U11: for a duplicate of an already-consumed job, what the transfer
    /// stage recorded for it (`transferred` / `failed` / …); empty when the
    /// job is still waiting or the stage is off.
    pub(crate) transfer: String,
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

    /// 综艺, 纪录片, 特摄剧 and 动画 get their own board. Animation is one board
    /// that is split by region only — 动画 and 动漫 are the same board (user
    /// decision 2026-10-21: merge them, keep just 国内/国外). Everything else is
    /// a movie or a TV series.
    ///
    /// Every board keeps the same chain below itself:
    /// `<board>/<国内|国外>/<年代>/<Title (Year)>/`; 晚会 is the one board without
    /// the region level (user decision 2026-10-22).
    pub(crate) fn category_label(&self) -> &'static str {
        if self.genres.iter().any(|genre| genre == "综艺") {
            return "综艺";
        }
        if self.is_tokusatsu() {
            return "特摄剧";
        }
        if self.genres.iter().any(|genre| genre == "纪录片") {
            return "纪录片";
        }
        if self.genres.iter().any(|genre| genre == "动画") {
            return "动画";
        }
        self.kind.label()
    }

    /// TMDB has no 特摄 genre, so the board is decided from the franchise names
    /// that make up the category. A title that does not match stays on 电影 or
    /// 电视剧; extend the list instead of guessing from genre alone.
    fn is_tokusatsu(&self) -> bool {
        const TOKUSATSU_TITLE_HINTS: &[&str] = &[
            "奥特曼",
            "假面骑士",
            "超级战队",
            "牙狼",
            "铠甲勇士",
            "特摄",
            "ultraman",
            "kamen rider",
            "super sentai",
            "garo",
        ];
        let title = self.title.to_lowercase();
        TOKUSATSU_TITLE_HINTS
            .iter()
            .any(|hint| title.contains(*hint))
    }
}

/// The poller and an admin request can both call `run_once`; a shared lock
/// serializes them so one never moves an entry the other is already moving.
static ARCHIVE_RUN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Serializes the ingest check-then-write. Two sidecar requests for the same
/// URL must not both decide "not queued yet" and write a job each: the check
/// (`find_existing_link_job`) and the write are one critical section.
static LINK_INGEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let result = self.run_once_inner(dry_run).await;
        if let Err(error) = &result
            && !dry_run
        {
            // A stage that returned an error would otherwise fail silently as far
            // as Telegram is concerned: the report is never built.
            crate::application::notify::spawn_event(
                crate::application::notify::NotifyEvent::new(
                    crate::application::notify::NotifyCategory::Failure,
                    "归档运行中断",
                )
                .field("📝 原因", error.as_str())
                .dedupe_key("archive:run-interrupted"),
            );
        }
        result
    }

    async fn run_once_inner(&self, dry_run: bool) -> Result<ArchiveReport, String> {
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

        // The link queue is one stage, not the pipeline. A queue / state file
        // that cannot be read skips that stage for this round (WARN) and lets
        // watch dirs, transfer and pending still run; otherwise one broken
        // file would freeze the whole archive for every poll.
        let link_queue = match self.ingest_link_queue(dry_run).await {
            Ok(report) => report,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "link queue round skipped; continuing with the rest of the pipeline"
                );
                LinkQueueReport {
                    dry_run,
                    ..LinkQueueReport::default()
                }
            }
        };
        if link_queue.examined > 0 {
            tracing::info!(
                dry_run = link_queue.dry_run,
                examined = link_queue.examined,
                accepted = link_queue.accepted.len(),
                invalid = link_queue.invalid.len(),
                failed = link_queue.failed.len(),
                incomplete = link_queue.skipped_incomplete.len(),
                "link queue consumed"
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
            let year_segment = decade_segment(year);
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
        let _ = write_reason_file(&bucket, reason);
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
            // Season directories were already handled (or already failed) by
            // the plan; do not move them a second time.
            let is_season = child.is_dir()
                && season_number_from_dir(&name).is_some()
                && dir_contains_video(&child);
            if is_season {
                continue;
            }
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
                || category_name.contains("纪录片")
                || category_name.contains("特摄")
                || category_name.contains("动画")
                || category_name.contains("动漫");
            // 动画 / 纪录片 boards carry both films and episodes, so a title that
            // is no series there may still be a film.
            let mixed_category = category_name.contains("动画")
                || category_name.contains("动漫")
                || category_name.contains("纪录片");
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
                        // Mixed boards hold films and episodes alike, so a title
                        // that is no series there may still be a film.
                        Ok(None) if mixed_category => {
                            match self.lookup_tmdb(&parsed, false).await {
                                Ok(matched) => matched,
                                Err(error) => {
                                    report.failed.push(format!(
                                        "{}: TMDB 查询失败: {error}",
                                        title_dir.display()
                                    ));
                                    continue;
                                }
                            }
                        }
                        Ok(matched) => matched,
                        Err(error) => {
                            report.failed.push(format!(
                                "{}: TMDB 查询失败: {error}",
                                title_dir.display()
                            ));
                            continue;
                        }
                    };
                    let Some(matched) = matched else {
                        report
                            .failed
                            .push(format!("{}: TMDB 未找到匹配", title_dir.display()));
                        continue;
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
        let year_segment = decade_segment(year);
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
        let mut job = job.normalized().map_err(LinkIngestError::Invalid)?;
        let queue = self.config.link_queue_dir();
        std::fs::create_dir_all(&queue).map_err(|error| {
            LinkIngestError::Storage(format!("无法创建链接队列 {}: {error}", queue.display()))
        })?;
        // The duplicate check and the write are one step, otherwise two
        // concurrent requests for the same URL both see "not queued" and both
        // write a job file. The guard is process-wide because ingest and the
        // poller live in the same service.
        let _ingest_guard = LINK_INGEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A broken queue / state file means "cannot tell whether this is a
        // duplicate": answer 500 and queue nothing, instead of writing a
        // possible second job for one link.
        let existing = if job.force {
            tracing::info!(
                url = %mask_link_for_log(&job.url),
                "link job forced: skipping the URL idempotency check"
            );
            None
        } else {
            self.find_existing_link_job(&job.url).map_err(|error| {
            tracing::warn!(
                url = %mask_link_for_log(&job.url),
                %error,
                "could not check for an existing link job; refusing to queue a possible duplicate"
            );
                LinkIngestError::Storage(error)
            })?
        };
        if let Some(existing) = existing {
            tracing::info!(
                url = %mask_link_for_log(&job.url),
                job_id = %existing.job_id,
                file = %existing.file,
                "link job already queued or already consumed; not queued again"
            );
            return Ok(QueuedLinkJob {
                job_id: existing.job_id,
                file: existing.file,
                status: LINK_JOB_STATUS_NEEDS_115,
                duplicate: true,
                transfer: existing.transfer,
            });
        }
        // `job_id` is the file name (docs §2.1). A same-millisecond job that
        // already took the name gets a suffix, and the job adopts it, so the
        // returned id, the file name and the state record never disagree.
        let (target, job_id) = link_job_target(&queue, &job.job_id);
        job.job_id = job_id.clone();
        let body = serde_json::to_string_pretty(&job).map_err(|error| {
            LinkIngestError::Storage(format!("序列化链接任务失败: {error}"))
        })?;
        let temp = queue.join(format!(".{job_id}.tmp"));
        // `0600` from the first byte: a share link may carry `?password=`, and
        // the job file must be as private as the state file (`rename` keeps the
        // mode, so 已处理/ stays 0600 as well).
        write_private_file(&temp, body.as_bytes())
            .map_err(|error| LinkIngestError::Storage(format!("写入 {} 失败: {error}", temp.display())))?;
        std::fs::rename(&temp, &target).map_err(|error| {
            let _ = std::fs::remove_file(&temp);
            LinkIngestError::Storage(format!("提交 {} 失败: {error}", target.display()))
        })?;
        tracing::info!(
            job_id = %job_id,
            url = %mask_link_for_log(&job.url),
            title = job.title.as_deref().unwrap_or(""),
            mode = job.mode.as_deref().unwrap_or(""),
            file = %target.display(),
            "link job queued"
        );
        Ok(QueuedLinkJob {
            job_id,
            file: target.display().to_string(),
            status: LINK_JOB_STATUS_NEEDS_115,
            duplicate: false,
            transfer: String::new(),
        })
    }

    /// Find a job for `url` that is either still waiting in the queue or
    /// already recorded in the state file. Used to make ingest idempotent.
    ///
    /// An unreadable queue directory or state file is an **error**, never "no
    /// duplicate" (review I12): guessing wrong queues the same link twice and
    /// hands it to the transfer client twice.
    fn find_existing_link_job(&self, url: &str) -> Result<Option<QueuedLinkJob>, String> {
        for entry in read_dir_entries(&self.config.link_queue_dir())? {
            if !is_link_job_file(&entry) {
                continue;
            }
            let text = match std::fs::read_to_string(&entry) {
                Ok(text) => text,
                Err(error) => {
                    // One unreadable job file must not make every ingest fail,
                    // but it may be hiding this URL: say so in the log.
                    tracing::warn!(
                        file = %entry.display(),
                        %error,
                        "could not read a queued link job while checking for duplicates; it may hide this URL"
                    );
                    continue;
                }
            };
            let Ok(job) = serde_json::from_str::<LinkJob>(&text) else {
                continue;
            };
            if job.url == url {
                return Ok(Some(QueuedLinkJob {
                    job_id: job.job_id,
                    file: entry.display().to_string(),
                    status: LINK_JOB_STATUS_NEEDS_115,
                    duplicate: true,
                    transfer: String::new(),
                }));
            }
        }
        // Newest record last: walk backwards so the most recent match wins. A
        // missing state file is "nothing consumed yet"; any other error is real.
        for record in read_link_state(&self.config.link_state_file())?.into_iter().rev() {
            if record.url == url && record.status != LINK_JOB_STATUS_INVALID {
                return Ok(Some(QueuedLinkJob {
                    job_id: record.job_id,
                    file: self
                        .config
                        .link_done_dir()
                        .join(&record.job_file)
                        .display()
                        .to_string(),
                    status: LINK_JOB_STATUS_NEEDS_115,
                    duplicate: true,
                    transfer: record.transfer.clone(),
                }));
            }
        }
        Ok(None)
    }

    /// Consume the JSON job files in `链接队列/`.
    ///
    /// The 115 share-link transfer client (plan U11) does not exist yet, so an
    /// accepted job is **not** transferred or downloaded: it is recorded in
    /// `链接队列/状态.jsonl` with the `needs_115_client` status, moved into
    /// `链接队列/已处理/` and logged. Junk is quarantined in
    /// `链接队列/无效/`. Nothing outside the archive root is touched.
    ///
    /// Every job is handled **best effort**: a state write or a move that fails
    /// is reported in `failed` and the round continues with the next job. One
    /// stuck job must never abort `run_once`, otherwise a full disk or a name
    /// collision would stop the whole archive pipeline for every round. The
    /// state line is appended before the file is moved, and the job id is
    /// remembered in `状态.jsonl`, so a job whose move failed is retried
    /// without being accounted for twice.
    pub(crate) async fn ingest_link_queue(
        &self,
        dry_run: bool,
    ) -> Result<LinkQueueReport, String> {
        let queue = self.config.link_queue_dir();
        let entries = read_dir_entries(&queue)?;
        let mut report = LinkQueueReport {
            dry_run,
            ..LinkQueueReport::default()
        };
        // What an earlier round already recorded: read once up front instead of
        // once per job, because the queue can hold thousands of files.
        // U11: a job whose transfer failed is *not* accounted — the state line
        // exists but the link never made it into 转存目录, so the operator's
        // retry path is `force: true` (a fresh job id), not a move retry.
        // `needs_115_client` lines from before the transfer stage are still
        // accounted: re-handing a link to a now-configured sidecar would
        // double-record them; the explicit retry path there is `force` too.
        let mut accounted: HashSet<String> = HashSet::new();
        if !dry_run {
            for record in read_link_state(&self.config.link_state_file())? {
                if record.status == LINK_JOB_STATUS_NEEDS_115 && !record.job_id.is_empty() {
                    accounted.insert(record.job_id);
                }
            }
        }
        for entry in entries {
            let name = entry
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            // The consumer, the duplicate check and the status counter all use
            // `is_link_job_file`, so they can never disagree about what a job
            // is: `状态.jsonl`, `已处理/`, `无效/`, `X.TMP` and symlinks are not.
            if !is_link_job_file(&entry) {
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
                let status = if self.transfer115_configured() && is_115_share_link(&job.url) {
                    LINK_JOB_STATUS_TRANSFERRED
                } else if self.transfer115_configured() {
                    LINK_JOB_STATUS_FAILED
                } else {
                    LINK_JOB_STATUS_NEEDS_115
                };
                report.accepted.push(AcceptedLinkJob {
                    job_id: job.job_id,
                    url: job.url,
                    file: name,
                    status,
                    transfer: String::new(),
                });
                continue;
            }
            // Where the job file has to end up. `job_id` is the file name
            // (docs §2.1), so a directory (or anything that is not a job file)
            // squatting on that exact name is a real error the operator has to
            // fix: renaming around it would break the reconciliation contract.
            // A *different* job with the same name can only come from two links
            // captured in the same millisecond and gets a suffix.
            let done_dir = self.config.link_done_dir();
            let direct = done_dir.join(&name);
            let target = match std::fs::symlink_metadata(&direct) {
                Ok(metadata) if metadata.file_type().is_file() => match job_file_job_id(&direct) {
                    Some(parked) if parked == job.job_id => None,
                    _ => Some(unique_target(&done_dir, &name)),
                },
                Ok(_) => {
                    let reason = format!("已处理/{name} 已存在且不是作业文件，无法移入已处理");
                    tracing::warn!(
                        job_id = %job.job_id,
                        file = %name,
                        %reason,
                        "link job could not be archived this round"
                    );
                    report.failed.push(FailedItem {
                        source: name.clone(),
                        reason,
                    });
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(direct),
                Err(error) => {
                    let reason = format!("检查 已处理/{name} 失败: {error}");
                    tracing::warn!(
                        job_id = %job.job_id,
                        file = %name,
                        %reason,
                        "link job could not be archived this round"
                    );
                    report.failed.push(FailedItem {
                        source: name.clone(),
                        reason,
                    });
                    continue;
                }
            };
            // The state line records the file it will really be parked under.
            let job_file = match &target {
                Some(target) => target
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| name.clone()),
                None => name.clone(),
            };
            // U11: hand 115 share links to the transfer115 sidecar before the
            // job is accounted and parked. The outcome decides the recorded
            // status; an auth failure records nothing and leaves the job
            // queued, so a refreshed cookie retries it on the next round.
            let transfer = if self.transfer115_configured() && is_115_share_link(&job.url) {
                match self.transfer_115_share_link(&job).await {
                    Ok(outcome) => outcome,
                    Err(pause) => {
                        // The sidecar is not usable (cookie dead, or the
                        // request itself failed): stop consuming this round.
                        // The job was neither recorded nor moved, so the next
                        // round retries it; the rest of the pipeline still
                        // runs (the caller treats this as a skipped stage).
                        return Err(pause);
                    }
                }
            } else if self.transfer115_configured() {
                // magnet / ed2k: the queue accepts them, but the transfer
                // sidecar only does 115 share links this slice.
                TransferOutcome {
                    status: LINK_JOB_STATUS_FAILED,
                    transfer: "unsupported".to_owned(),
                    reason: LINK_JOB_UNSUPPORTED_REASON.to_owned(),
                }
            } else {
                TransferOutcome {
                    status: LINK_JOB_STATUS_NEEDS_115,
                    transfer: String::new(),
                    reason: LINK_JOB_NEEDS_115_REASON.to_owned(),
                }
            };
            let state_path = self.config.link_state_file();
            if !accounted.contains(&job.job_id) {
                let record = LinkJobStateRecord {
                    recorded_at_unix: unix_now(),
                    status: transfer.status,
                    job_id: job.job_id.clone(),
                    url: job.url.clone(),
                    title: job.title.clone().unwrap_or_default(),
                    mode: job.mode.clone().unwrap_or_default(),
                    source: job.source.clone().unwrap_or_default(),
                    reason: transfer.reason.clone(),
                    job_file,
                };
                if let Err(error) = append_link_state(&state_path, &record) {
                    // Leave the file queued and retry next round: a record
                    // failure is the same class of transient IO as a move
                    // failure and must not abort the round either.
                    tracing::warn!(
                        job_id = %job.job_id,
                        file = %name,
                        %error,
                        "failed to record a link job; the file stays queued for the next round"
                    );
                    report.failed.push(FailedItem {
                        source: name.clone(),
                        reason: format!("写链接状态文件失败: {error}"),
                    });
                    continue;
                }
                accounted.insert(job.job_id.clone());
            }
            let Some(target) = target else {
                // The file is already in `已处理/`: an earlier round moved it
                // but failed to record it. The single state line was recovered
                // above, so drop the queue copy instead of leaving it pending.
                match std::fs::remove_file(&entry) {
                    Ok(()) => {
                        tracing::warn!(
                            job_id = %job.job_id,
                            file = %name,
                            "link job was already in 已处理/; removed the duplicate queue copy"
                        );
                        report.accepted.push(AcceptedLinkJob {
                            job_id: job.job_id,
                            url: job.url,
                            file: done_dir.join(&name).display().to_string(),
                            status: LINK_JOB_STATUS_NEEDS_115,
                            transfer: String::new(),
                        });
                    }
                    Err(error) => {
                        let reason = format!("删除重复的任务文件 {name} 失败: {error}");
                        tracing::warn!(
                            job_id = %job.job_id,
                            file = %name,
                            %reason,
                            "link job retry failed"
                        );
                        report.failed.push(FailedItem {
                            source: name.clone(),
                            reason,
                        });
                    }
                }
                continue;
            };
            if let Err(error) = move_entry(&entry, &target) {
                tracing::warn!(
                    job_id = %job.job_id,
                    file = %name,
                    target = %target.display(),
                    %error,
                    "failed to move a link job into 已处理; the next round retries the move without recording it again"
                );
                report.failed.push(FailedItem {
                    source: name.clone(),
                    reason: format!("移动任务文件失败: {error}"),
                });
                continue;
            }
            match transfer.status {
                LINK_JOB_STATUS_TRANSFERRED => {
                    tracing::info!(
                        job_id = %job.job_id,
                        url = %mask_link_for_log(&job.url),
                        title = job.title.as_deref().unwrap_or(""),
                        mode = job.mode.as_deref().unwrap_or(""),
                        target = %transfer.transfer,
                        "115 share link transferred into the transfer directory"
                    );
                }
                LINK_JOB_STATUS_FAILED => {
                    tracing::warn!(
                        job_id = %job.job_id,
                        url = %mask_link_for_log(&job.url),
                        title = job.title.as_deref().unwrap_or(""),
                        reason = %transfer.reason,
                        "115 transfer failed: recorded and parked; re-deliver with force"
                    );
                }
                _ => {
                    tracing::warn!(
                        job_id = %job.job_id,
                        url = %mask_link_for_log(&job.url),
                        title = job.title.as_deref().unwrap_or(""),
                        mode = job.mode.as_deref().unwrap_or(""),
                        "link job needs the 115 transfer client (not configured): recorded, nothing downloaded"
                    );
                }
            }
            report.accepted.push(AcceptedLinkJob {
                job_id: job.job_id,
                url: job.url,
                file: target.display().to_string(),
                status: transfer.status,
                transfer: transfer.transfer,
            });
        }
        Ok(report)
    }

    /// Whether the transfer115 sidecar is configured for this run.
    fn transfer115_configured(&self) -> bool {
        !self.config.transfer115_base.trim().is_empty()
    }

    /// U11: hand one 115 share link to the transfer115 sidecar
    /// (`POST {base}/transfer`). The sidecar owns the 115 login (p115client +
    /// the web cookie), resolves the fixed target directory to a 115 cid
    /// (creating it when missing) and saves the share. The saved files land
    /// in the CD2-mounted 转存目录, where the existing pipeline takes over —
    /// Lux never touches the 115 API itself.
    ///
    /// Returns the outcome to record, or `Err` when the whole stage must pause
    /// (cookie dead, or the request itself failed): the caller leaves the job
    /// unrecorded and unmoved so the next round retries it.
    async fn transfer_115_share_link(
        &self,
        job: &LinkJob,
    ) -> Result<TransferOutcome, String> {
        let base = self.config.transfer115_base.trim().trim_end_matches('/');
        let body = serde_json::json!({
            "job_id": job.job_id,
            "url": job.url,
            "title": job.title,
            // The sidecar resolves the target relative to the archive root it
            // shares with Lux (the CD2 mount), so it gets the root path, the
            // relative target directory, and the CD2 prefix of the root.
            "archive_root": self.config.root.display().to_string(),
            "target_dir": self.config.transfer115_target_dir,
            "cd2_prefix": self.config.cd2_prefix,
        });
        let request = self
            .http
            .post(format!("{base}/transfer"))
            .json(&body)
            .timeout(self.config.transfer115_timeout);
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return Err(format!(
                    "转存侧车 {} 请求失败: {}",
                    mask_link_for_log(&format!("{base}/transfer")),
                    describe_sidecar_error(&error)
                ));
            }
        };
        let status = response.status();
        let payload: serde_json::Value = match response.json().await {
            Ok(payload) => payload,
            Err(error) => {
                return Err(format!("转存侧车响应不是 JSON（HTTP {status}）: {error}"));
            }
        };
        let answer = payload
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let detail = payload
            .get("detail")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        let saved = payload
            .get("saved")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        match answer.as_str() {
            "transferred" => Ok(TransferOutcome {
                status: LINK_JOB_STATUS_TRANSFERRED,
                transfer: if saved.is_empty() {
                    "transferred".to_owned()
                } else {
                    saved.join(", ")
                },
                reason: if detail.is_empty() {
                    "115 分享已转存到转存目标目录".to_owned()
                } else {
                    detail
                },
            }),
            "auth_required" => Err(format!(
                "115 cookie 无效或已过期（{}）：请在设置里更新 cookie，作业留在队列等待重试",
                if detail.is_empty() { "auth_required" } else { &detail }
            )),
            "failed" => Ok(TransferOutcome {
                status: LINK_JOB_STATUS_FAILED,
                transfer: "failed".to_owned(),
                reason: if detail.is_empty() {
                    "转存侧车报告转存失败".to_owned()
                } else {
                    detail
                },
            }),
            "unsupported" => Ok(TransferOutcome {
                status: LINK_JOB_STATUS_FAILED,
                transfer: "unsupported".to_owned(),
                reason: if detail.is_empty() {
                    LINK_JOB_UNSUPPORTED_REASON.to_owned()
                } else {
                    detail
                },
            }),
            _ => Err(format!(
                "转存侧车返回了无法理解的状态 {answer:?}（HTTP {status}）"
            )),
        }
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
    /// metadata-only (no content is read): the biggest *title* directory under
    /// `媒体目录/` whose name matches the requested title wins.
    pub(crate) fn lookup_library_size(&self, title: &str) -> LibraryLookup {
        lookup_library_size_in(&self.config.media_dir(), title)
    }

    /// Blocking-pool variant of [`Self::lookup_library_size`] for async callers.
    ///
    /// The scan walks the media tree, so it must not run on a tokio worker: on
    /// a library of 180k entries that would park one worker per concurrent
    /// lookup.
    ///
    /// TODO(I6): `src/api/admin/admin_jobs.rs` (owned by the settings change)
    /// still calls the synchronous variant; switching
    /// `Json(service.lookup_library_size(title))` to
    /// `Json(service.lookup_library_size_isolated(title).await)` completes the
    /// isolation. The tree walk itself is already bounded (title layer only,
    /// depth cap, `symlink_metadata`) and cached, so the synchronous path stays
    /// cheap until then.
    #[allow(dead_code)] // wired by the admin_jobs.rs change that owns that file
    pub(crate) async fn lookup_library_size_isolated(&self, title: &str) -> LibraryLookup {
        let media_dir = self.config.media_dir();
        let requested = title.trim().to_owned();
        let uncached = requested.clone();
        match tokio::task::spawn_blocking(move || lookup_library_size_in(&media_dir, &requested)).await
        {
            Ok(lookup) => lookup,
            Err(error) => {
                tracing::warn!(%error, title = %uncached, "library lookup task failed");
                LibraryLookup {
                    title: uncached,
                    ..LibraryLookup::default()
                }
            }
        }
    }

    pub(crate) fn status(&self) -> ArchiveStatus {
        let transfer115_ready = !self.config.transfer115_base.trim().is_empty();
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
            transfer115_configured: transfer115_ready,
            transfer115_target_dir: self.config.transfer115_target_dir.clone(),
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

/// Auto-run interval for the archive pipeline, read from the editable settings
/// file (with `LUX_ARCHIVE_POLL_SECONDS` as the fallback). It is re-read on
/// every tick so a new interval takes effect without a restart.
fn archive_poll_interval() -> Option<Duration> {
    ArchiveSettings::load().poll_interval()
}

/// Start the background archive poller. Files dropped into the transfer
/// directory are then archived and organized without an admin request. Called
/// once from the server startup.
///
/// The loop re-reads the settings file every tick: `poll_seconds`, the watch
/// directories, the TMDB key and the wash rules all apply to the next run, and
/// a poller that boots while polling is disabled idles instead of exiting, so
/// enabling it in the settings UI starts working immediately.
pub fn start_poller() {
    let startup = ArchiveService::new(ArchiveConfig::from_env());
    if let Err(error) = startup.config.ensure_directories() {
        tracing::warn!(%error, "archive pipeline directories could not be created");
        return;
    }
    match archive_poll_interval() {
        Some(interval) => tracing::info!(
            root = %startup.config.root.display(),
            interval_secs = interval.as_secs(),
            "archive pipeline poller started"
        ),
        None => tracing::info!(
            root = %startup.config.root.display(),
            "archive pipeline poller idle until poll_seconds is set"
        ),
    }
    tokio::spawn(async move {
        // Startup reconciliation: a process that was down while new boards
        // appeared must still end up with their libraries (docs/handoff/
        // AUTO-LIBRARY.md §2).
        reconcile_libraries().await;
        let mut last_reconcile = Instant::now();
        loop {
            let Some(interval) = archive_poll_interval() else {
                tokio::time::sleep(Duration::from_secs(POLL_DISABLED_RECHECK_SECONDS)).await;
                continue;
            };
            tokio::time::sleep(interval).await;
            // Rebuild the service so settings edited in the meantime apply to
            // this run instead of the values captured at startup.
            let service = ArchiveService::new(ArchiveConfig::from_env());
            let report = service.run_once(false).await;
            let organized = match &report {
                Ok(report) => {
                    if !report.archived_to_pending.is_empty()
                        || !report.organized.is_empty()
                        || !report.failed.is_empty()
                        || !report.duplicates.is_empty()
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
                    !report.organized.is_empty()
                }
                Err(error) => {
                    tracing::warn!(%error, "archive pipeline run failed");
                    false
                }
            };
            // A run that organized something may have created a new board; the
            // periodic reconciliation also catches boards added from outside.
            if organized || last_reconcile.elapsed() >= AUTO_LIBRARY_RECONCILE_INTERVAL {
                reconcile_libraries().await;
                last_reconcile = Instant::now();
            }
        }
    });
}

/// Run one auto-library reconciliation, using the services `AppState` registered
/// at startup. A server without library services (setup mode) does nothing.
async fn reconcile_libraries() {
    let Some(services) = AUTO_LIBRARY_SERVICES.get() else {
        return;
    };
    let mirror_root = ArchiveConfig::from_env().mirror_root;
    let report = auto_library::provision(
        &services.database,
        &services.libraries,
        Some(&services.scan_jobs),
        &mirror_root,
    )
    .await;
    if !report.worth_logging() {
        tracing::debug!(
            mirror = %mirror_root.display(),
            existing = report.existing.len(),
            "archive auto library reconciliation found nothing to create"
        );
        return;
    }
    tracing::info!(
        mirror = %mirror_root.display(),
        created = ?report.created,
        updated = ?report.updated,
        existing = report.existing.len(),
        scans = ?report.scans,
        failed = ?report.failed,
        "archive auto library reconciliation"
    );
}

/// Give the archive poller the handles it needs to create libraries and queue
/// scans. Called once from the server startup, after `AppState` built them.
pub(crate) fn register_auto_library_services(
    database: Database,
    libraries: LibraryService,
    scan_jobs: ScanJobService,
) {
    let _ = AUTO_LIBRARY_SERVICES.set(auto_library::AutoLibraryServices {
        database,
        libraries,
        scan_jobs,
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

/// Deepest level the wash-size sum follows. A library title is at most
/// `<board>/<region>/<year>/<title>/Season NN/`, so twelve levels are generous;
/// the cap only exists so a pathological tree cannot stall a run.
const PATH_SIZE_MAX_DEPTH: usize = 12;

/// Total size of a file or directory tree (metadata only, no data read).
///
/// Symlinks are never followed (`symlink_metadata`): a link pointing back into
/// the tree would otherwise make the walk unbounded, and a link must not
/// contribute a size the archive does not own.
fn path_size(path: &Path) -> u64 {
    path_size_at(path, 0)
}

fn path_size_at(path: &Path, depth: usize) -> u64 {
    if depth > PATH_SIZE_MAX_DEPTH {
        return 0;
    }
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return 0;
    }
    if !file_type.is_dir() {
        return metadata.len();
    }
    let Ok(reader) = std::fs::read_dir(path) else {
        return 0;
    };
    reader
        .flatten()
        .map(|entry| path_size_at(&entry.path(), depth + 1))
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

/// Final job file path for `job_id` in `queue`, plus the id the chosen file
/// name encodes.
///
/// `job_id` is the file name (docs §2.1), so a collision — two links captured in
/// the same millisecond with the same title slug — renames the *job*, never just
/// the file: the 202 response, the file name and the state record stay equal.
fn link_job_target(queue: &Path, job_id: &str) -> (PathBuf, String) {
    let target = unique_target(queue, &format!("{job_id}.json"));
    let final_id = target
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| job_id.to_owned());
    (target, final_id)
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
    name.starts_with('.') || name == REASON_FILE_NAME || name.ends_with(".part") || name.ends_with(".tmp")
}

/// Name of the sidecar that records why an entry landed in the failure directory.
const REASON_FILE_NAME: &str = "原因.txt";

/// Writes the failure reason as UTF-8 with a BOM.
///
/// The text is legal UTF-8 either way, but Windows Notepad guesses the code page
/// from the first bytes and renders BOM-less UTF-8 as GBK mojibake. A BOM keeps
/// the file readable there without changing what any UTF-8 reader sees.
fn write_reason_file(bucket: &Path, reason: &str) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(3 + reason.len());
    bytes.extend_from_slice(b"\xEF\xBB\xBF");
    bytes.extend_from_slice(reason.as_bytes());
    std::fs::write(bucket.join(REASON_FILE_NAME), bytes)
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
    /// U11: whether the transfer115 sidecar is configured (empty base = the
    /// transfer stage is off and jobs stay `needs_115_client`).
    #[serde(default)]
    pub(crate) transfer115_configured: bool,
    /// U11: the fixed target directory for share transfers, relative to the
    /// archive root.
    #[serde(default)]
    pub(crate) transfer115_target_dir: String,
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
///
/// The scheme alone is not enough: the part after it must name a reachable
/// target (`https://` or `magnet:?` with nothing behind it is rejected), because
/// such a link can never be transferred and would just sit in the queue.
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
    if lowered.starts_with("https://") || lowered.starts_with("http://") {
        return validate_http_link(url);
    }
    if lowered.starts_with("magnet:") {
        return validate_magnet_link(url);
    }
    if lowered.starts_with("ed2k://") {
        return validate_ed2k_link(url);
    }
    Err("不支持的链接协议（只接受 http(s) / magnet / ed2k）".to_owned())
}

/// `http(s)://<host>[/…]`: the authority must contain a real host name. An empty
/// authority (`https://`, `https:///s/x`), a port without host or credentials
/// without host are all rejected.
fn validate_http_link(url: &str) -> Result<String, String> {
    let rest = url.split_once("://").map_or("", |(_, rest)| rest);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return Err("链接缺少 host（例如 https://115.com/s/xxxx）".to_owned());
    }
    // Drop `user:password@`: the host is what remains.
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host_port.split(':').next().unwrap_or_default();
    let valid = !host.is_empty()
        && host
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_'));
    if !valid {
        return Err(format!("链接缺少有效的 host: {host:?}"));
    }
    Ok(url.to_owned())
}

/// `magnet:?xt=urn:btih:<infohash>`: a magnet without an infohash cannot be
/// fetched, so it is rejected instead of queued forever.
fn validate_magnet_link(url: &str) -> Result<String, String> {
    let rest = &url["magnet:".len()..];
    let lowered = rest.to_ascii_lowercase();
    let Some(index) = lowered.find("urn:btih:") else {
        return Err("magnet 链接缺少 xt=urn:btih:<infohash>".to_owned());
    };
    let hash = &rest[index + "urn:btih:".len()..];
    let hash_end = hash.find('&').unwrap_or(hash.len());
    if hash[..hash_end].is_empty() {
        return Err("magnet 链接的 infohash 为空".to_owned());
    }
    Ok(url.to_owned())
}

/// `ed2k://|file|<name>|<size>|<hash>|/`: the file target must carry a name and
/// a hash.
fn validate_ed2k_link(url: &str) -> Result<String, String> {
    let rest = &url["ed2k://".len()..];
    let Some(fields) = rest.strip_prefix("|file|") else {
        return Err("ed2k 链接缺少 |file| 目标".to_owned());
    };
    let fields = fields.split('|').collect::<Vec<_>>();
    if fields.first().is_none_or(|name| name.is_empty()) {
        return Err("ed2k 链接缺少文件名".to_owned());
    }
    if fields.get(2).is_none_or(|hash| hash.is_empty()) {
        return Err("ed2k 链接缺少 hash".to_owned());
    }
    Ok(url.to_owned())
}

/// Log-safe rendering of a share link: only `scheme://host`, and for a magnet
/// only `magnet:?xt=urn:btih:<前8位>…`.
///
/// A share link may carry `?password=`, a per-recipient token or the full file
/// name, so the raw URL must never reach a log line (review D7).
pub(crate) fn mask_link_for_log(raw: &str) -> String {
    let url = raw.trim();
    if url.is_empty() {
        return String::new();
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("magnet:") {
        // The infohash identifies the torrent; eight characters are enough to
        // correlate two log lines and useless to anyone else.
        let hash = lower
            .find("urn:btih:")
            .map(|index| &url[index + "urn:btih:".len()..])
            .unwrap_or_default();
        let head: String = hash.chars().take(8).collect();
        if head.is_empty() {
            return "magnet:?xt=…".to_owned();
        }
        return format!("magnet:?xt=urn:btih:{head}…");
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return "…".to_owned();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    // `ed2k://|file|name|size|hash|/` has no host and the "authority" is the
    // file name plus the hash: keep the scheme only.
    if authority.contains('|') {
        return format!("{scheme}://…");
    }
    // Drop `user:password@` if a producer ever sends credentials.
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    if host.is_empty() {
        return format!("{scheme}://…");
    }
    format!("{scheme}://{host}")
}

/// The five listening modes of the sidecar (`settings::LISTEN_MODES`). The
/// queue is an operator-facing record, so `mode` is an enum, not a free-form
/// tag: anything else is a 400 on ingest and an `invalid` job file in the
/// queue.
fn validate_link_mode(mode: &str) -> Result<String, String> {
    let mode = mode.trim();
    if settings::LISTEN_MODES.contains(&mode) {
        return Ok(mode.to_owned());
    }
    Err(format!(
        "mode 只能是 {}（收到 {mode:?}）",
        settings::LISTEN_MODES.join(" / ")
    ))
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

/// What counts as a link job file. Shared by the consumer, the duplicate check
/// and the status counter so the three can never disagree:
///
/// - a regular file directly in the queue directory (`已处理/` and `无效/` are
///   directories, so they never qualify);
/// - name does not start with `.` (temp files);
/// - extension is `.json`, case-insensitively (`X.JSON` is a job);
/// - a symlink is **not** a job: `symlink_metadata` never follows it, so the
///   consumer and `count_link_jobs` see the same set.
fn is_link_job_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name.starts_with('.') {
        return false;
    }
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        return false;
    }
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

fn count_link_jobs(directory: &Path) -> usize {
    let reader = match std::fs::read_dir(directory) {
        Ok(reader) => reader,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return 0,
        Err(error) => {
            // The status endpoint keeps answering, but the operator must see
            // that the number is unknown rather than "0 waiting".
            tracing::warn!(
                path = %directory.display(),
                %error,
                "could not count the pending link jobs"
            );
            return 0;
        }
    };
    reader
        .flatten()
        .filter(|entry| is_link_job_file(&entry.path()))
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

/// Write `contents` to `path` with mode `0600` on Unix. The queue holds share
/// links, so a job file must not be world-readable even for the instant before
/// it is renamed into place.
fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    // A leftover temp file would keep its old mode through `truncate`; remove it
    // first so the fresh `OpenOptions` mode applies.
    let _ = std::fs::remove_file(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Parse `状态.jsonl` into its records.
///
/// A missing file means "nothing was consumed yet". Any other read error is
/// returned instead of being treated as "no duplicates": silently deciding that
/// a link is new would queue it a second time and hand the same share link to
/// the transfer client twice.
fn read_link_state(path: &Path) -> Result<Vec<LinkStateLine>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "读取链接状态文件 {} 失败: {error}",
                path.display()
            ));
        }
    };
    let mut records = Vec::new();
    let mut broken = 0usize;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<LinkStateLine>(line) {
            Ok(record) => records.push(record),
            Err(_) => broken += 1,
        }
    }
    if broken > 0 {
        tracing::warn!(
            path = %path.display(),
            broken,
            "link state file has unparsable lines; they are ignored"
        );
    }
    Ok(records)
}

/// What the transfer115 sidecar answered for one job, in the shape the
/// consumer records into `状态.jsonl`.
#[derive(Clone, Debug)]
struct TransferOutcome {
    status: &'static str,
    transfer: String,
    reason: String,
}

/// Hosts that serve 115 share links. Mirrors `SHARE_HOSTS` in
/// `sidecar/transfer115/transfer115/service.py`: the share code is the same and
/// 115 hands out `115cdn.com` / `anxia.com` links as often as `115.com` ones, so
/// both halves must agree on what counts as a 115 share.
const SHARE_LINK_HOSTS: [&str; 4] = ["115cdn.com", "anxia.com", "115.link", "115.com.cn"];

/// Only 115 share links go to the transfer sidecar; magnet / ed2k are queued
/// but parked as `unsupported` until 115 cloud download lands.
fn is_115_share_link(url: &str) -> bool {
    let lowered = url.trim().to_ascii_lowercase();
    if !(lowered.starts_with("http://") || lowered.starts_with("https://")) {
        return false;
    }
    let rest = lowered.split_once("://").map_or("", |(_, rest)| rest);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .split(':')
        .next()
        .unwrap_or_default();
    host == "115.com" || host.ends_with(".115.com") || SHARE_LINK_HOSTS.contains(&host)
}

/// One-line description of a reqwest failure for the pause reason. The URL is
/// already masked by the caller; the error text never carries the share link.
fn describe_sidecar_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "请求超时".to_owned()
    } else if error.is_connect() {
        "无法连接".to_owned()
    } else if error.is_request() {
        "请求构造失败".to_owned()
    } else {
        "网络错误".to_owned()
    }
}

/// `job_id` stored inside a job file, if `path` is one.
fn job_file_job_id(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let job = serde_json::from_str::<LinkJob>(&text).ok()?;
    (!job.job_id.trim().is_empty()).then_some(job.job_id)
}

/// Comparison key for library titles: full-width characters folded to ASCII
/// first (so `流浪地球２` equals `流浪地球2`), then lowercase alphanumerics
/// only, so `某剧 第二季 (2024)` and `某剧.第二季` normalize alike.
fn normalize_title_key(value: &str) -> String {
    value
        .chars()
        .map(fold_full_width)
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

/// Simplified NFKC: the full-width ASCII block `！`–`～` (`Ａ`–`Ｚ`, `０`–`９`,
/// `．`, …) and the ideographic space fold to their ASCII forms. The project has
/// no ICU dependency and a title only needs these two ranges.
fn fold_full_width(character: char) -> char {
    match character {
        '\u{3000}' => ' ',
        '\u{FF01}'..='\u{FF5E}' => {
            char::from_u32(character as u32 - 0xFEE0).unwrap_or(character)
        }
        _ => character,
    }
}

/// Deepest level walked by the library lookup. A title is a child of a year
/// directory (depth 3 in the normal chain, depth 2 for 晚会), so nothing deeper
/// than the title directory is ever read.
const LIBRARY_LOOKUP_MAX_DEPTH: usize = 3;

/// A title query for the library lookup: the parsed title key plus the year the
/// query carries (when it has one).
struct LibraryQuery {
    key: String,
    year: Option<i64>,
}

impl LibraryQuery {
    fn new(title: &str) -> Option<Self> {
        let (key, year) = title_match_key(title);
        (!key.is_empty()).then_some(Self { key, year })
    }
}

/// Match key of one title: parse the release name, keep the title without the
/// year / season / episode / quality tags, and keep the year separately.
fn title_match_key(value: &str) -> (String, Option<i64>) {
    match parse_media_name(value) {
        Some(parsed) => (normalize_title_key(&parsed.title), parsed.year),
        None => (normalize_title_key(value), None),
    }
}

/// The library tree groups titles by decade (`2020s`) instead of by year, which
/// is what the existing 影视库 tree already does. A title with no usable year
/// keeps its own bucket so it is still browsable.
const UNKNOWN_DECADE: &str = "未知年代";

pub(crate) fn decade_segment(year: Option<i64>) -> String {
    match year {
        Some(year) if year > 0 => format!("{}s", (year / 10) * 10),
        _ => UNKNOWN_DECADE.to_owned(),
    }
}

/// `2024` / `2020s` / `未知年份` / `未知年代` are the year-or-decade level of the
/// library tree, never a title. The four-digit form stays accepted so trees
/// written before the decade switch keep resolving.
fn is_library_year_segment(name: &str) -> bool {
    let name = name.trim();
    let mut characters = name.chars();
    let head: Vec<char> = characters.by_ref().take(4).collect();
    let digits = head.len() == 4 && head.iter().all(char::is_ascii_digit);
    if !digits {
        return name == "未知年份" || name == UNKNOWN_DECADE;
    }
    match characters.next() {
        None => true,
        Some('s') => characters.next().is_none(),
        Some(_) => false,
    }
}

/// Does the library title `entry_name` denote the same work as `query`?
///
/// Deliberately conservative, because a wrong size makes the wash mode treat a
/// real upgrade as a duplicate:
///
/// - only the title layer is compared (the caller guarantees it), never the
///   board / region / year directory names;
/// - a year on both sides must agree (`某剧 2019` never matches `某剧 (2024)`);
/// - titles of three characters or less must match exactly (`繁花` is not
///   `繁花似锦`);
/// - a longer title may only extend the shorter one with release tags, never
///   with a sequel number (`流浪地球` is not `流浪地球2`).
fn library_title_matches(query: &LibraryQuery, entry_name: &str) -> bool {
    let (entry_key, entry_year) = title_match_key(entry_name);
    if query.key.is_empty() || entry_key.is_empty() {
        return false;
    }
    if let (Some(query_year), Some(entry_year)) = (query.year, entry_year) {
        if query_year != entry_year {
            return false;
        }
    }
    if query.key == entry_key {
        return true;
    }
    let shorter = query.key.chars().count().min(entry_key.chars().count());
    if shorter <= 3 {
        return false;
    }
    let (longer, short) = if entry_key.chars().count() > query.key.chars().count() {
        (entry_key.as_str(), query.key.as_str())
    } else {
        (query.key.as_str(), entry_key.as_str())
    };
    let Some(remainder) = longer.strip_prefix(short) else {
        return false;
    };
    // A bare number after the title is a sequel marker, not a release tag.
    !remainder
        .chars()
        .all(|character| character.is_ascii_digit())
}

/// How long one lookup answer is reused. The sidecar asks once per link and the
/// tree only changes when an archive run finishes, so a minute is cheap and
/// safe; the lookup stays correct, just a minute stale at worst.
const LIBRARY_LOOKUP_CACHE_SECONDS: u64 = 60;
/// A hand-edited library plus scripts can ask for many titles; cap the cache so
/// it cannot grow forever.
const LIBRARY_LOOKUP_CACHE_MAX_ENTRIES: usize = 256;

/// Cached answers keyed by `<media dir>\0<title key>\0<year>`. The cached value
/// is only the match and the size: `LibraryLookup` also carries the caller's
/// title, which differs between equivalent queries.
static LIBRARY_LOOKUP_CACHE: OnceLock<Mutex<HashMap<String, (Instant, Option<String>, Option<u64>)>>> =
    OnceLock::new();

type LibraryLookupCache = Mutex<HashMap<String, (Instant, Option<String>, Option<u64>)>>;

fn library_lookup_cache() -> &'static LibraryLookupCache {
    LIBRARY_LOOKUP_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_library_lookup(key: &str) -> Option<(Option<String>, Option<u64>)> {
    let cache = library_lookup_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .get(key)
        .filter(|(stored, _, _)| stored.elapsed() < Duration::from_secs(LIBRARY_LOOKUP_CACHE_SECONDS))
        .map(|(_, matched, size)| (matched.clone(), *size))
}

fn store_library_lookup(key: String, lookup: &LibraryLookup) {
    let mut cache = library_lookup_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.len() >= LIBRARY_LOOKUP_CACHE_MAX_ENTRIES {
        cache.clear();
    }
    cache.insert(
        key,
        (Instant::now(), lookup.matched.clone(), lookup.size_bytes),
    );
}

/// One library lookup against `media_dir` (see `ArchiveService::lookup_library_size`).
fn lookup_library_size_in(media_dir: &Path, title: &str) -> LibraryLookup {
    let mut lookup = LibraryLookup {
        title: title.trim().to_owned(),
        ..LibraryLookup::default()
    };
    let Some(query) = LibraryQuery::new(&lookup.title) else {
        return lookup;
    };
    let cache_key = format!(
        "{}\u{0}{}\u{0}{:?}",
        media_dir.display(),
        query.key,
        query.year
    );
    if let Some((matched, size)) = cached_library_lookup(&cache_key) {
        lookup.matched = matched;
        lookup.size_bytes = size;
        return lookup;
    }
    let mut best: Option<(String, u64)> = None;
    collect_library_entries(media_dir, 0, &query, &mut best);
    if let Some((name, size)) = best {
        lookup.matched = Some(name);
        lookup.size_bytes = Some(size);
    }
    store_library_lookup(cache_key, &lookup);
    lookup
}

/// Walk `媒体目录/` (type / region / year / title, and the shorter 晚会 chain)
/// collecting the biggest **title** directory that matches `query`.
///
/// A title is only ever a child of a year directory, so the year / region /
/// board names are never matched: a query like `2024` used to return the whole
/// year. Symlinks are inspected with `symlink_metadata` and never followed, so
/// a self-referential link cannot recurse and cannot smuggle in a size.
fn collect_library_entries(
    directory: &Path,
    depth: usize,
    query: &LibraryQuery,
    best: &mut Option<(String, u64)>,
) {
    if depth > LIBRARY_LOOKUP_MAX_DEPTH {
        return;
    }
    let parent_is_year = directory
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_library_year_segment);
    let Ok(reader) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        // `is_dir` on the symlink metadata is false for a link itself.
        if !metadata.file_type().is_dir() {
            continue;
        }
        let Some(name) = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
        else {
            continue;
        };
        if parent_is_year && library_title_matches(query, &name) {
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

    /// Windows Notepad opens BOM-less UTF-8 as GBK, so the failure reason is
    /// written with a BOM: the bytes are still plain UTF-8 for every other reader.
    #[test]
    fn writes_failure_reason_with_utf8_bom() {
        let dir = std::env::temp_dir().join(format!("lux-reason-{}", unix_now()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        write_reason_file(&dir, "无法从目录名解析").expect("write reason");
        let bytes = std::fs::read(dir.join(REASON_FILE_NAME)).expect("read reason");
        assert_eq!(&bytes[..3], b"\xEF\xBB\xBF");
        assert_eq!(
            std::str::from_utf8(&bytes[3..]).expect("utf8 body"),
            "无法从目录名解析"
        );
        let _ = std::fs::remove_dir_all(&dir);
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

    /// The library tree groups by decade, so a 2019 release lands under
    /// `2010s` and a title without a year keeps its own bucket.
    #[test]
    fn organizes_titles_into_decades() {
        assert_eq!(decade_segment(Some(2024)), "2020s");
        assert_eq!(decade_segment(Some(2019)), "2010s");
        assert_eq!(decade_segment(Some(2010)), "2010s");
        assert_eq!(decade_segment(Some(2009)), "2000s");
        assert_eq!(decade_segment(Some(1989)), "1980s");
        assert_eq!(decade_segment(None), UNKNOWN_DECADE);
        assert_eq!(decade_segment(Some(0)), UNKNOWN_DECADE);

        // The year / decade level is never mistaken for a title, in either the
        // decade layout or the year layout written before the switch.
        for segment in ["2020s", "2010s", "2024", "未知年代", "未知年份"] {
            assert!(is_library_year_segment(segment), "{segment}");
        }
        for segment in ["20200s", "2020ss", "某剧 (2024)", "未知"] {
            assert!(!is_library_year_segment(segment), "{segment}");
        }
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

        // 动画 and 动漫 are the same board: it is split by region, never by
        // origin (2026-10-21).
        for (title, origin, language) in [("灵笼", "CN", "zh"), ("進撃の巨人", "JP", "ja")] {
            let anime = TmdbMatch {
                kind: MediaKind::Series,
                tmdb_id: 4,
                title: title.to_owned(),
                year: Some(2019),
                origin_countries: vec![origin.to_owned()],
                release_country: None,
                original_language: Some(language.to_owned()),
                genres: vec!["动画".to_owned()],
            };
            assert_eq!(anime.category_label(), "动画");
        }

        // 纪录片 and 特摄剧 are boards of their own, region-split like every
        // other board except 晚会 (user decision 2026-10-22).
        let documentary = TmdbMatch {
            kind: MediaKind::Movie,
            tmdb_id: 5,
            title: "地球脉动".to_owned(),
            year: Some(2006),
            origin_countries: vec!["GB".to_owned()],
            release_country: None,
            original_language: Some("en".to_owned()),
            genres: vec!["纪录片".to_owned()],
        };
        assert_eq!(documentary.category_label(), "纪录片");

        for title in ["欧布奥特曼", "假面骑士555", "Kamen Rider Build", "超级战队"] {
            let tokusatsu = TmdbMatch {
                kind: MediaKind::Series,
                tmdb_id: 6,
                title: title.to_owned(),
                year: Some(2016),
                origin_countries: vec!["JP".to_owned()],
                release_country: None,
                original_language: Some("ja".to_owned()),
                genres: vec!["科幻".to_owned()],
            };
            assert_eq!(tokusatsu.category_label(), "特摄剧", "{title}");
        }
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
            transfer115_base: String::new(),
            transfer115_cookie: String::new(),
            transfer115_target_dir: "转存目录".to_owned(),
            transfer115_timeout: Duration::from_secs(5),
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

    /// U11 test helper: a one-shot HTTP stub for the transfer115 sidecar.
    /// Listens on an ephemeral port, answers `POST /transfer` with the given
    /// status (`transferred` / `auth_required` / `failed`), and remembers the
    /// last request body.
    /// U11 test helper: a one-shot HTTP stub for the transfer115 sidecar.
    /// Listens on an ephemeral port, answers `POST /transfer` with the given
    /// status (`transferred` / `auth_required` / `failed`), and remembers the
    /// last request body. Dropping the stub closes the listener and joins the
    /// worker thread.
    struct StubSidecar {
        address: std::net::SocketAddr,
        listener: std::net::TcpListener,
        thread: Option<std::thread::JoinHandle<()>>,
        log: std::sync::Arc<std::sync::Mutex<StubLog>>,
    }

    #[derive(Default)]
    struct StubLog {
        hits: usize,
        last_body: serde_json::Value,
    }

    impl StubSidecar {
        fn start(answer: &str) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("stub bind");
            let address = listener.local_addr().expect("addr");
            let answer = answer.to_owned();
            let log = std::sync::Arc::new(std::sync::Mutex::new(StubLog::default()));
            let worker_log = std::sync::Arc::clone(&log);
            let thread = std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    use std::io::{Read, Write};
                    let mut buffer = [0u8; 8192];
                    let mut received = String::new();
                    loop {
                        let read = match stream.read(&mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => read,
                        };
                        received.push_str(&String::from_utf8_lossy(&buffer[..read]));
                        if received.contains("\r\n\r\n") {
                            break;
                        }
                    }
                    let header_end = received.find("\r\n\r\n").unwrap_or(0);
                    let headers = received[..header_end].to_ascii_lowercase();
                    let content_length: usize = headers
                        .lines()
                        .find_map(|line| line.trim().strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(0);
                    let mut body = received[header_end + 4..].to_owned();
                    while body.len() < content_length {
                        match stream.read(&mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => body.push_str(&String::from_utf8_lossy(&buffer[..read])),
                        }
                    }
                    let payload = match answer.as_str() {
                        "transferred" => serde_json::json!({
                            "status": "transferred",
                            "saved": ["某剧 第二季 4K"],
                            "detail": "已转存 1 个条目",
                        }),
                        "auth_required" => serde_json::json!({
                            "status": "auth_required",
                            "detail": "cookie 已过期",
                        }),
                        "failed" => serde_json::json!({
                            "status": "failed",
                            "detail": "分享链接无效或需要密码",
                        }),
                        _ => serde_json::json!({"status": answer}),
                    };
                    let text = payload.to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        text.len(),
                        text
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                    let mut slot = worker_log
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    slot.hits += 1;
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) {
                        slot.last_body = parsed;
                    }
                }
            });
            Self {
                address,
                // The listener moved into the worker closure; keep a closed
                // placeholder so `Drop` has a harmless value to hold.
                listener: std::net::TcpListener::bind("127.0.0.1:0")
                    .expect("stub placeholder bind"),
                thread: Some(thread),
                log,
            }
        }

        fn base(&self) -> String {
            format!("http://{}", self.address)
        }

        fn last_body(&self) -> serde_json::Value {
            let slot = self
                .log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            slot.last_body.clone()
        }

        fn hits(&self) -> usize {
            let slot = self
                .log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            slot.hits
        }
    }

    impl Drop for StubSidecar {
        fn drop(&mut self) {
            // Detach, do not join: the worker's `incoming()` loop never ends
            // (the listener lives inside it), so joining here would deadlock
            // the test thread. The thread is reaped when the process exits.
            let _ = self.thread.take();
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

        // The scheme alone is not a link: the target after it must exist.
        assert!(validate_link_url("https://").is_err());
        assert!(validate_link_url("https:///s/abc").is_err());
        assert!(validate_link_url("https://?password=1").is_err());
        assert!(validate_link_url("http://:8080/x").is_err());
        assert!(validate_link_url("magnet:").is_err());
        assert!(validate_link_url("magnet:?").is_err());
        assert!(validate_link_url("magnet:?dn=no-hash").is_err());
        assert!(validate_link_url("magnet:?xt=urn:btih:").is_err());
        assert!(validate_link_url("ed2k://").is_err());
        assert!(validate_link_url("ed2k://|file|").is_err());
        // Credentials are allowed as long as a host follows them.
        assert!(validate_link_url("https://user:pass@115.com/s/abc").is_ok());
        assert!(validate_link_url("https://115.com").is_ok());
        assert!(validate_link_url("magnet:?xt=urn:btih:abcdef0123456789&dn=x").is_ok());
    }

    /// Logs must never contain the share link: only scheme + host survives, and
    /// a magnet keeps the first eight characters of its infohash.
    #[test]
    fn masks_share_links_in_logs() {
        assert_eq!(
            mask_link_for_log("https://115.com/s/abc123?password=9999&uid=1"),
            "https://115.com"
        );
        assert_eq!(
            mask_link_for_log("  HTTP://115.com:8080/x/y  "),
            "HTTP://115.com:8080"
        );
        assert_eq!(
            mask_link_for_log("https://user:pass@115.com/s/abc?password=1"),
            "https://115.com"
        );
        assert_eq!(
            mask_link_for_log("magnet:?xt=urn:btih:abcdef0123456789&dn=secret.mkv"),
            "magnet:?xt=urn:btih:abcdef01…"
        );
        assert_eq!(
            mask_link_for_log("ed2k://|file|private.mkv|1|HASH|/"),
            "ed2k://…"
        );
        assert_eq!(mask_link_for_log("https://"), "https://…");
        assert_eq!(mask_link_for_log(""), "");
        let masked = mask_link_for_log("https://115.com/s/abc?password=9999");
        assert!(!masked.contains("password"));
        assert!(!masked.contains("115.com/s/abc"));
    }

    /// `mode` is one of the five listening modes; anything else is rejected on
    /// ingest and quarantined when a hand-written file carries it.
    #[tokio::test]
    async fn rejects_unknown_link_modes() {
        let job = LinkJob {
            url: "https://115.com/s/abc".to_owned(),
            mode: Some("随便".to_owned()),
            ..LinkJob::default()
        };
        let error = job.normalized().expect_err("unknown mode is rejected");
        assert!(error.contains("mode 只能是"), "{error}");

        for mode in settings::LISTEN_MODES {
            let job = LinkJob {
                url: "https://115.com/s/abc".to_owned(),
                mode: Some(mode.to_owned()),
                ..LinkJob::default()
            };
            assert!(job.normalized().is_ok(), "{mode} is a valid mode");
        }
        // The field stays optional: an old producer that omits it still works.
        let untagged = LinkJob {
            url: "https://115.com/s/abc".to_owned(),
            ..LinkJob::default()
        };
        assert_eq!(untagged.cleaned().expect("valid").mode, None);
        let blank = LinkJob {
            url: "https://115.com/s/abc".to_owned(),
            mode: Some("  ".to_owned()),
            ..LinkJob::default()
        };
        assert_eq!(blank.cleaned().expect("valid").mode, None);

        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let queue = service.config.link_queue_dir();
        std::fs::create_dir_all(&queue).expect("queue");
        std::fs::write(
            queue.join("bad-mode.json"),
            r#"{"job_id":"1700000000003-bad-mode","url":"https://115.com/s/bad","mode":"乱写"}"#,
        )
        .unwrap();
        let report = service.ingest_link_queue(false).await.expect("consumed");
        assert_eq!(report.invalid.len(), 1);
        assert!(report.accepted.is_empty());
        assert_eq!(
            std::fs::read_dir(service.config.link_invalid_dir())
                .expect("invalid dir")
                .count(),
            1
        );
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

    #[tokio::test]
    async fn queues_and_consumes_link_jobs() {
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

        let report = service.ingest_link_queue(false).await.expect("consumed");
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
        let again = service.ingest_link_queue(false).await.expect("consumed");
        assert_eq!(again.examined, 0);
        assert_eq!(
            std::fs::read_dir(service.config.link_done_dir())
                .expect("done dir")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn link_ingest_is_idempotent_on_the_url() {
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

        service.ingest_link_queue(false).await.expect("consumed");
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

    /// The URL idempotency key never expires, so `force` is the explicit retry
    /// path: it queues the link again instead of answering `duplicate`.
    #[tokio::test]
    async fn a_forced_link_job_is_queued_again() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let first = service
            .enqueue_link_job(link_job("https://115.com/s/abc"))
            .expect("queued");
        service.ingest_link_queue(false).await.expect("consumed");
        let duplicate = service
            .enqueue_link_job(link_job("https://115.com/s/abc"))
            .expect("queued");
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.job_id, first.job_id);
        assert_eq!(service.status().pending_link_jobs, 0);

        let mut forced = link_job("https://115.com/s/abc");
        forced.force = true;
        let again = service.enqueue_link_job(forced).expect("queued");
        assert!(!again.duplicate, "force must bypass the URL check");
        assert_ne!(again.job_id, first.job_id);
        assert_eq!(service.status().pending_link_jobs, 1);
    }

    /// Ten simultaneous posts of one URL leave exactly one job file: the
    /// duplicate check and the write are one critical section.
    #[test]
    fn concurrent_ingest_of_the_same_url_writes_one_job_file() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = std::sync::Arc::new(ArchiveService::new_for_tests(archive_config(tmp.path())));
        let mut handles = Vec::new();
        for _ in 0..10 {
            let service = std::sync::Arc::clone(&service);
            handles.push(std::thread::spawn(move || {
                service
                    .enqueue_link_job(link_job("https://115.com/s/race"))
                    .expect("queued")
            }));
        }
        let queued = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread"))
            .collect::<Vec<_>>();
        assert_eq!(
            queued.iter().filter(|queued| !queued.duplicate).count(),
            1,
            "exactly one caller writes the job"
        );
        assert_eq!(queued.iter().filter(|queued| queued.duplicate).count(), 9);
        let first = &queued[0];
        assert!(
            queued.iter().all(|queued| queued.job_id == first.job_id),
            "every response names the same job"
        );
        assert_eq!(count_link_jobs(&service.config.link_queue_dir()), 1);
        let file = Path::new(&first.file);
        assert!(file.exists());
        assert_eq!(
            file.file_stem().unwrap().to_string_lossy(),
            first.job_id,
            "the job id is the file name"
        );
    }

    /// A same-name collision renames the job id together with the file, so the
    /// 202 response and the state record never disagree with the file name.
    #[test]
    fn a_colliding_job_file_takes_its_suffixed_name_as_the_job_id() {
        let tmp = tempfile::tempdir().expect("temp dir");
        std::fs::write(tmp.path().join("1700000000000-demo.json"), b"{}").unwrap();
        let (target, job_id) = link_job_target(tmp.path(), "1700000000000-demo");
        assert_eq!(job_id, "1700000000000-demo (2)");
        assert_eq!(target, tmp.path().join("1700000000000-demo (2).json"));
    }

    #[tokio::test]
    async fn quarantines_unusable_link_jobs() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let queue = service.config.link_queue_dir();
        std::fs::create_dir_all(&queue).expect("queue");
        std::fs::write(queue.join("broken.json"), b"not json").unwrap();
        std::fs::write(queue.join("evil.json"), br#"{"url":"file:///etc/passwd"}"#).unwrap();
        // `状态.jsonl` shares the directory but is not a job.
        std::fs::write(queue.join(LINK_QUEUE_STATE_FILE), b"{}\n").unwrap();

        let report = service.ingest_link_queue(false).await.expect("consumed");
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

    #[tokio::test]
    async fn link_queue_keeps_unsettled_junk_for_the_next_run() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let mut config = archive_config(tmp.path());
        config.settle_seconds = 3600;
        let service = ArchiveService::new_for_tests(config);
        let queue = service.config.link_queue_dir();
        std::fs::create_dir_all(&queue).expect("queue");
        std::fs::write(queue.join("writing.json"), b"{\"url\":").unwrap();

        let report = service.ingest_link_queue(false).await.expect("consumed");
        assert_eq!(report.examined, 1);
        assert_eq!(report.skipped_incomplete.len(), 1);
        assert!(report.invalid.is_empty());
        assert_eq!(service.status().pending_link_jobs, 1);
    }

    /// One stuck job must not block the others: the healthy job is archived
    /// and recorded, the blocked one is counted once and retried next round.
    #[tokio::test]
    async fn an_unmovable_link_job_does_not_stop_the_round() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let queue = service.config.link_queue_dir();
        std::fs::create_dir_all(&queue).expect("queue");
        let blocked = LinkJob {
            job_id: "1700000000000-blocked".to_owned(),
            url: "https://115.com/s/blocked".to_owned(),
            title: Some("被卡住的剧".to_owned()),
            ..LinkJob::default()
        };
        let healthy = LinkJob {
            job_id: "1700000000001-ok".to_owned(),
            url: "https://115.com/s/ok".to_owned(),
            title: Some("正常的剧".to_owned()),
            ..LinkJob::default()
        };
        std::fs::write(queue.join("blocked.json"), serde_json::to_vec(&blocked).unwrap()).unwrap();
        std::fs::write(queue.join("ok.json"), serde_json::to_vec(&healthy).unwrap()).unwrap();
        // A directory squatting on the exact destination name of the blocked
        // job: it cannot be moved, and nothing may be renamed around it.
        let blocker = service.config.link_done_dir().join("blocked.json");
        std::fs::create_dir_all(blocker.join("keep")).expect("blocker dir");
        std::fs::write(blocker.join("keep").join("note.txt"), b"x").unwrap();

        let report = service.ingest_link_queue(false).await.expect("consumed");
        assert_eq!(report.examined, 2);
        assert_eq!(report.accepted.len(), 1);
        assert_eq!(report.accepted[0].job_id, healthy.job_id);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].source, "blocked.json");
        assert!(report.invalid.is_empty());

        let state = std::fs::read_to_string(service.config.link_state_file()).expect("state file");
        assert_eq!(
            state.lines().count(),
            1,
            "only the healthy job is recorded: {state}"
        );
        assert_eq!(service.status().pending_link_jobs, 1);
        assert!(service.config.link_done_dir().join("ok.json").exists());
        assert!(blocker.is_dir());

        // The next round retries the blocked job and still accounts it once.
        let again = service.ingest_link_queue(false).await.expect("consumed");
        assert_eq!(again.failed.len(), 1);
        assert!(again.accepted.is_empty());
        let state = std::fs::read_to_string(service.config.link_state_file()).expect("state file");
        assert_eq!(state.lines().count(), 1);
    }

    /// State write and file move are two steps: a job recorded last round but
    /// not moved is retried without a second state line.
    #[tokio::test]
    async fn a_recorded_link_job_is_not_accounted_twice() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let queue = service.config.link_queue_dir();
        std::fs::create_dir_all(&queue).expect("queue");
        let job = LinkJob {
            job_id: "1700000000002-retry".to_owned(),
            url: "https://115.com/s/retry".to_owned(),
            title: Some("重试的剧".to_owned()),
            ..LinkJob::default()
        };
        std::fs::write(queue.join("retry.json"), serde_json::to_vec(&job).unwrap()).unwrap();
        let record = LinkJobStateRecord {
            recorded_at_unix: 1_700_000_000,
            status: LINK_JOB_STATUS_NEEDS_115,
            job_id: job.job_id.clone(),
            url: job.url.clone(),
            title: job.title.clone().unwrap_or_default(),
            mode: String::new(),
            source: String::new(),
            reason: LINK_JOB_NEEDS_115_REASON.to_owned(),
            job_file: "retry.json".to_owned(),
        };
        append_link_state(&service.config.link_state_file(), &record).expect("state");

        let report = service.ingest_link_queue(false).await.expect("consumed");
        assert_eq!(report.accepted.len(), 1);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(service.config.link_done_dir().join("retry.json").exists());
        let state = std::fs::read_to_string(service.config.link_state_file()).expect("state file");
        assert_eq!(
            state.lines().count(),
            1,
            "the retried job is not recorded twice: {state}"
        );
    }

    /// Job files carry share links (possibly `?password=`), so they are as
    /// private as `状态.jsonl` — before and after the move into 已处理/.
    #[cfg(unix)]
    #[tokio::test]
    async fn link_job_files_are_private() {
        use std::os::unix::fs::PermissionsExt;

        fn mode_of(path: &Path) -> u32 {
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777
        }

        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let queued = service
            .enqueue_link_job(link_job("https://115.com/s/secret?password=1"))
            .expect("queued");
        assert_eq!(mode_of(Path::new(&queued.file)), 0o600, "job file");

        service.ingest_link_queue(false).await.expect("consumed");
        let done = service.config.link_done_dir().join(
            Path::new(&queued.file)
                .file_name()
                .expect("job file name"),
        );
        assert_eq!(mode_of(&done), 0o600, "已处理/ keeps the mode");
        assert_eq!(mode_of(&service.config.link_state_file()), 0o600, "状态.jsonl");
    }

    /// U11 test helper: a config with the transfer115 sidecar pointed at a
    /// local stub server.
    fn transfer_config(root: &std::path::Path, base: &str) -> ArchiveConfig {
        let mut config = archive_config(root);
        config.transfer115_base = base.to_owned();
        config.transfer115_cookie = "UID=1; CID=2; SEID=3; KID=4".to_owned();
        config
    }

    /// U11: a 115 share link is handed to the sidecar and recorded
    /// `transferred`; the job moves to 已处理/ and the state line carries what
    /// the sidecar saved.
    #[tokio::test]
    async fn a_115_share_link_is_transferred_by_the_sidecar() {
        let server = StubSidecar::start("transferred");
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(transfer_config(tmp.path(), &server.base()));
        let queued = service
            .enqueue_link_job(link_job("https://115.com/s/abc"))
            .expect("queued");

        let report = service.ingest_link_queue(false).await.expect("consumed");
        assert_eq!(report.accepted.len(), 1);
        assert_eq!(report.accepted[0].status, LINK_JOB_STATUS_TRANSFERRED);
        assert_eq!(report.accepted[0].transfer, "某剧 第二季 4K");

        let state = std::fs::read_to_string(service.config.link_state_file()).expect("state file");
        assert_eq!(state.lines().count(), 1);
        assert!(state.contains(LINK_JOB_STATUS_TRANSFERRED), "{state}");
        assert!(state.contains("某剧 第二季 4K"), "{state}");
        assert!(!state.contains(LINK_JOB_NEEDS_115_REASON), "{state}");
        assert_eq!(service.status().pending_link_jobs, 0);
        // The sidecar saw the job: the fixed target directory and the archive
        // root travel with the request.
        let seen = server.last_body();
        assert_eq!(seen["job_id"], queued.job_id);
        assert_eq!(seen["url"], "https://115.com/s/abc");
        assert_eq!(seen["target_dir"], "转存目录");
        assert_eq!(seen["archive_root"], tmp.path().display().to_string());
        // The cookie never travels in the request body: the sidecar reads it
        // from the shared settings file.
        assert!(
            !seen.to_string().contains("SEID=3"),
            "cookie leaked into the request: {seen}"
        );
    }

    /// U11: `auth_required` records nothing and leaves the job queued — the
    /// operator refreshes the cookie and the next round retries the same job.
    #[tokio::test]
    async fn an_expired_115_cookie_pauses_the_stage_without_losing_the_job() {
        let server = StubSidecar::start("auth_required");
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(transfer_config(tmp.path(), &server.base()));
        let queued = service
            .enqueue_link_job(link_job("https://115.com/s/dead-cookie"))
            .expect("queued");

        let error = service
            .ingest_link_queue(false)
            .await
            .expect_err("the stage pauses on a dead cookie");
        assert!(error.contains("cookie"), "{error}");

        let state = std::fs::read_to_string(service.config.link_state_file())
            .unwrap_or_default();
        assert!(!state.contains(LINK_JOB_STATUS_AUTH_REQUIRED), "{state}");
        assert!(!state.contains(&queued.job_id), "{state}");
        assert_eq!(service.status().pending_link_jobs, 1, "the job stays queued");
    }

    /// U11: a sidecar `failed` answer is recorded and parked like any consumed
    /// job; the operator re-delivers with `force: true`.
    #[tokio::test]
    async fn a_failed_transfer_is_recorded_and_parked() {
        let server = StubSidecar::start("failed");
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(transfer_config(tmp.path(), &server.base()));
        service
            .enqueue_link_job(link_job("https://115.com/s/gone"))
            .expect("queued");

        let report = service.ingest_link_queue(false).await.expect("consumed");
        assert_eq!(report.accepted.len(), 1);
        assert_eq!(report.accepted[0].status, LINK_JOB_STATUS_FAILED);
        assert_eq!(report.accepted[0].transfer, "failed");

        let state = std::fs::read_to_string(service.config.link_state_file()).expect("state file");
        assert_eq!(state.lines().count(), 1);
        assert!(state.contains(LINK_JOB_STATUS_FAILED), "{state}");
        assert_eq!(service.status().pending_link_jobs, 0);
        // The duplicate answer for a consumed-but-failed link carries the
        // recorded status, so the producer knows a `force` re-delivery is in
        // order. (The state line's `transfer` field holds the sidecar's saved
        // list — empty for a failure — so the status is what tells the story.)
        let again = service
            .enqueue_link_job(link_job("https://115.com/s/gone"))
            .expect("queued");
        assert!(again.duplicate);
        let state = std::fs::read_to_string(service.config.link_state_file()).expect("state");
        assert!(
            state.lines().any(|line| {
                let record: serde_json::Value = serde_json::from_str(line).expect("line");
                record["job_id"] == again.job_id
                    && record["status"] == LINK_JOB_STATUS_FAILED
            }),
            "the duplicate must point at the failed record: {state}"
        );
    }

    /// U11: magnet / ed2k links stay accepted by the queue but the transfer
    /// sidecar only does 115 share links this slice: they are recorded
    /// `failed` with the `unsupported` marker, not silently dropped.
    #[tokio::test]
    async fn a_magnet_link_is_parked_as_unsupported() {
        let server = StubSidecar::start("transferred");
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(transfer_config(tmp.path(), &server.base()));
        service
            .enqueue_link_job(link_job("magnet:?xt=urn:btih:abc"))
            .expect("queued");

        let report = service.ingest_link_queue(false).await.expect("consumed");
        assert_eq!(report.accepted.len(), 1);
        assert_eq!(report.accepted[0].status, LINK_JOB_STATUS_FAILED);
        assert_eq!(report.accepted[0].transfer, "unsupported");
        let state = std::fs::read_to_string(service.config.link_state_file()).expect("state file");
        assert!(state.contains(LINK_JOB_UNSUPPORTED_REASON), "{state}");
        assert_eq!(
            server.hits(),
            0,
            "the sidecar is never asked about a magnet link"
        );
    }

    /// U11: an unreachable sidecar pauses the stage: the job is neither
    /// recorded nor moved, so the next round retries it.
    #[tokio::test]
    async fn an_unreachable_sidecar_pauses_the_stage() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(transfer_config(
            tmp.path(),
            "http://127.0.0.1:1", // nothing listens here
        ));
        service
            .enqueue_link_job(link_job("https://115.com/s/wait"))
            .expect("queued");

        let error = service
            .ingest_link_queue(false)
            .await
            .expect_err("the stage pauses when the sidecar is down");
        assert!(error.contains("转存侧车"), "{error}");
        assert_eq!(service.status().pending_link_jobs, 1);
        let state = std::fs::read_to_string(service.config.link_state_file())
            .unwrap_or_default();
        assert!(!state.contains("https://115.com/s/wait"), "{state}");
    }

    /// U11: with the stage configured, a dry run reports what *would* happen
    /// (transferred for a 115 link, unsupported for a magnet) without calling
    /// the sidecar, recording anything, or moving files.
    #[tokio::test]
    async fn a_dry_run_reports_the_transfer_without_calling_the_sidecar() {
        let server = StubSidecar::start("transferred");
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(transfer_config(tmp.path(), &server.base()));
        service
            .enqueue_link_job(link_job("https://115.com/s/dry"))
            .expect("queued");
        service
            .enqueue_link_job(link_job("magnet:?xt=urn:btih:dry"))
            .expect("queued");

        let report = service.ingest_link_queue(true).await.expect("dry run");
        assert_eq!(report.accepted.len(), 2);
        let statuses: std::collections::BTreeMap<_, _> = report
            .accepted
            .iter()
            .map(|job| (job.url.clone(), job.status))
            .collect();
        assert_eq!(
            statuses.get("https://115.com/s/dry"),
            Some(&LINK_JOB_STATUS_TRANSFERRED)
        );
        assert_eq!(
            statuses.get("magnet:?xt=urn:btih:dry"),
            Some(&LINK_JOB_STATUS_FAILED)
        );
        assert_eq!(server.hits(), 0, "a dry run never calls the sidecar");
        assert_eq!(service.status().pending_link_jobs, 2);
        assert!(!service.config.link_state_file().exists());
    }

    /// U11: the transfer stage is part of the link-queue stage: a sidecar that
    /// is down skips the queue stage for the round but the rest of the
    /// pipeline still runs.
    #[tokio::test]
    async fn a_down_sidecar_does_not_stop_the_run() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let mut config = transfer_config(tmp.path(), "http://127.0.0.1:1");
        config.transfer115_timeout = Duration::from_millis(300);
        let service = ArchiveService::new_for_tests(config);
        service
            .enqueue_link_job(link_job("https://115.com/s/run"))
            .expect("queued");

        let report = service
            .run_once(false)
            .await
            .expect("the rest of the pipeline still runs");
        assert!(report.organized.is_empty());
        assert_eq!(service.status().pending_link_jobs, 1);
    }

    /// U11: only 115 hosts go to the transfer sidecar.
    #[test]
    fn only_115_hosts_are_share_links() {
        assert!(is_115_share_link("https://115.com/s/abc"));
        assert!(is_115_share_link("https://115.com/s/abc?password=1"));
        assert!(is_115_share_link("http://115.com/s/abc"));
        assert!(is_115_share_link("https://user:pass@115.com/s/abc"));
        assert!(is_115_share_link("https://sub.115.com/s/abc"));
        // The front ends 115 actually hands out in share text.
        assert!(is_115_share_link("https://115cdn.com/s/sws1yeb3wt5?password=x3c1"));
        assert!(is_115_share_link("https://anxia.com/s/abc"));
        assert!(is_115_share_link("https://115.link/s/abc"));
        assert!(!is_115_share_link("https://evil.com/115.com/s/abc"));
        assert!(!is_115_share_link("https://115.com.evil.com/x"));
        assert!(!is_115_share_link("magnet:?xt=urn:btih:abc"));
        assert!(!is_115_share_link("ed2k://|file|a.mkv|1|HASH|/"));
    }

    /// Full-width forms (common in hand-typed and IME titles) fold to ASCII
    /// before comparison, so the lookup and the wash decision see one title.
    #[test]
    fn normalizes_full_width_title_characters() {
        assert_eq!(normalize_title_key("流浪地球２"), "流浪地球2");
        assert_eq!(normalize_title_key("ＡＢＣ１２３"), "abc123");
        assert_eq!(normalize_title_key("某剧　第二季"), "某剧第二季");
        assert_eq!(normalize_title_key("某剧（２０２４）"), "某剧2024");

        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let title_dir = tmp
            .path()
            .join(MEDIA_DIR)
            .join("电影")
            .join("国内")
            .join("2024")
            .join("某剧２ (2024)");
        std::fs::create_dir_all(&title_dir).expect("title dir");
        std::fs::write(title_dir.join("movie.mkv"), vec![0u8; 300]).unwrap();

        let lookup = service.lookup_library_size("某剧2");
        assert_eq!(lookup.matched.as_deref(), Some("某剧２ (2024)"));
        assert_eq!(lookup.size_bytes, Some(300));
    }

    /// The part number belongs to the title: asking for part two must not return
    /// the size of part one.
    #[test]
    fn lookup_does_not_confuse_a_sequel_with_its_predecessor() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let title_dir = tmp
            .path()
            .join(MEDIA_DIR)
            .join("电影")
            .join("国内")
            .join("2019")
            .join("流浪地球 (2019)");
        std::fs::create_dir_all(&title_dir).expect("title dir");
        std::fs::write(title_dir.join("movie.mkv"), vec![0u8; 2048]).unwrap();

        assert_eq!(
            service.lookup_library_size("流浪地球2 4K").size_bytes,
            None,
            "流浪地球2 must not match 流浪地球"
        );
        let found = service.lookup_library_size("流浪地球 4K HDR");
        assert_eq!(found.matched.as_deref(), Some("流浪地球 (2019)"));
        assert_eq!(found.size_bytes, Some(2048));
    }

    /// A year in the query must agree with the year in the library name; a
    /// query without a year still matches (Telegram titles rarely carry one).
    #[test]
    fn lookup_requires_the_year_to_agree() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let title_dir = tmp
            .path()
            .join(MEDIA_DIR)
            .join("电视剧")
            .join("国内")
            .join("2024")
            .join("某剧 (2024)");
        std::fs::create_dir_all(&title_dir).expect("title dir");
        std::fs::write(title_dir.join("S01E01.mkv"), vec![0u8; 1024]).unwrap();

        assert_eq!(
            service.lookup_library_size("某剧 2019 4K").size_bytes,
            None,
            "2019 is not the 2024 copy"
        );
        assert_eq!(
            service
                .lookup_library_size("某剧 2024 4K")
                .matched
                .as_deref(),
            Some("某剧 (2024)")
        );
        assert_eq!(
            service.lookup_library_size("某剧 4K").matched.as_deref(),
            Some("某剧 (2024)")
        );
    }

    /// A two- or three-character title is too short for a prefix heuristic:
    /// `繁花` must not match `繁花似锦`.
    #[test]
    fn lookup_requires_exact_match_for_short_titles() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let title_dir = tmp
            .path()
            .join(MEDIA_DIR)
            .join("电视剧")
            .join("国内")
            .join("2023")
            .join("繁花似锦 (2023)");
        std::fs::create_dir_all(&title_dir).expect("title dir");
        std::fs::write(title_dir.join("S01E01.mkv"), vec![0u8; 512]).unwrap();

        assert_eq!(
            service.lookup_library_size("繁花").size_bytes,
            None,
            "繁花 is not 繁花似锦"
        );
        assert_eq!(
            service.lookup_library_size("繁花似锦").size_bytes,
            Some(512)
        );
    }

    /// Only the title layer is matched: a year directory can never answer a
    /// lookup, otherwise a query like `2024` returns a whole year of files.
    #[test]
    fn lookup_never_matches_a_year_directory() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let year_dir = tmp
            .path()
            .join(MEDIA_DIR)
            .join("电视剧")
            .join("国内")
            .join("2024");
        let title_dir = year_dir.join("某剧 (2024)");
        std::fs::create_dir_all(&title_dir).expect("title dir");
        std::fs::write(title_dir.join("S01E01.mkv"), vec![0u8; 1024]).unwrap();
        std::fs::write(year_dir.join("loose.mkv"), vec![0u8; 4096]).unwrap();

        assert_eq!(service.lookup_library_size("2024").size_bytes, None);
        assert_eq!(service.lookup_library_size("国内").size_bytes, None);
        assert_eq!(service.lookup_library_size("电视剧").size_bytes, None);
    }

    /// One job predicate: the status counter and the consumer agree on
    /// `X.JSON` vs dotfiles vs `状态.jsonl` vs symlinks.
    /// A read failure is not "no duplicate": ingest refuses instead of writing a
    /// second job, and the consumer skips the round instead of consuming blindly.
    #[tokio::test]
    async fn an_unreadable_state_file_is_not_no_duplicate() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let queue = service.config.link_queue_dir();
        std::fs::create_dir_all(&queue).expect("queue");
        // A directory where 状态.jsonl should be: reading it fails (EISDIR).
        std::fs::create_dir_all(service.config.link_state_file()).expect("state dir");

        let error = service
            .enqueue_link_job(link_job("https://115.com/s/abc"))
            .expect_err("must not queue a possible duplicate");
        assert!(matches!(error, LinkIngestError::Storage(_)), "{error}");
        assert_eq!(count_link_jobs(&queue), 0, "nothing was written");

        // The consumer cannot read the state either: that round fails and the
        // job file stays where it is for the next round.
        std::fs::write(
            queue.join("job.json"),
            serde_json::to_vec(&link_job("https://115.com/s/def")).unwrap(),
        )
        .unwrap();
        assert!(service.ingest_link_queue(false).await.is_err());
        assert_eq!(count_link_jobs(&queue), 1);
    }

    /// One unreadable queue / state file skips only the link stage of a run; the
    /// rest of the pipeline still executes.
    #[tokio::test]
    async fn a_broken_link_queue_does_not_stop_the_run() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let queue = service.config.link_queue_dir();
        std::fs::create_dir_all(&queue).expect("queue");
        std::fs::create_dir_all(service.config.link_state_file()).expect("state dir");

        let report = service
            .run_once(false)
            .await
            .expect("the rest of the pipeline still runs");
        assert!(!report.dry_run);
        assert!(report.organized.is_empty());
    }

    #[tokio::test]
    async fn count_and_consume_agree_on_what_a_job_is() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let queue = service.config.link_queue_dir();
        std::fs::create_dir_all(&queue).expect("queue");
        let job = |id: &str| LinkJob {
            job_id: id.to_owned(),
            url: format!("https://115.com/s/{id}"),
            ..LinkJob::default()
        };
        std::fs::write(
            queue.join("UPPER.JSON"),
            serde_json::to_vec(&job("upper")).unwrap(),
        )
        .unwrap();
        std::fs::write(
            queue.join(".hidden.json"),
            serde_json::to_vec(&job("hidden")).unwrap(),
        )
        .unwrap();
        std::fs::write(queue.join(LINK_QUEUE_STATE_FILE), b"{}\n").unwrap();
        std::fs::write(queue.join("notes.txt"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(queue.join("UPPER.JSON"), queue.join("link.json"))
            .expect("symlink");

        assert_eq!(count_link_jobs(&queue), 1, "only UPPER.JSON is a job");
        let report = service.ingest_link_queue(false).await.expect("consumed");
        assert_eq!(report.examined, 1);
        assert_eq!(report.accepted.len(), 1);
        assert_eq!(report.accepted[0].job_id, "upper");
        assert_eq!(
            count_link_jobs(&queue),
            0,
            "the symlink and the dotfile are not jobs"
        );
        assert!(queue.join(".hidden.json").exists());
        assert!(queue.join("notes.txt").exists());
        #[cfg(unix)]
        assert!(
            std::fs::symlink_metadata(queue.join("link.json")).is_ok(),
            "the symlink is left alone"
        );
    }

    /// The walk is cached for `LIBRARY_LOOKUP_CACHE_SECONDS`: a second ask for
    /// the same title inside the TTL does not rescan the tree.
    #[test]
    fn lookup_results_are_cached_briefly() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let title_dir = tmp
            .path()
            .join(MEDIA_DIR)
            .join("电视剧")
            .join("国内")
            .join("2024")
            .join("缓存测试剧 (2024)");
        std::fs::create_dir_all(&title_dir).expect("title dir");
        std::fs::write(title_dir.join("S01E01.mkv"), vec![0u8; 700]).unwrap();

        assert_eq!(service.lookup_library_size("缓存测试剧").size_bytes, Some(700));
        // The library grows after the first answer; the cached one is reused.
        std::fs::write(title_dir.join("S01E02.mkv"), vec![0u8; 700]).unwrap();
        assert_eq!(
            service.lookup_library_size("缓存测试剧").size_bytes,
            Some(700),
            "the second answer comes from the cache"
        );
        // A query with another key is not answered from that entry.
        assert_eq!(service.lookup_library_size("另一个剧").size_bytes, None);
    }

    /// Symlinks are never followed: not for matching a title and not for the
    /// size walk (a self-referential link used to recurse forever).
    #[cfg(unix)]
    #[test]
    fn lookup_ignores_symlinks() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let media = tmp.path().join(MEDIA_DIR);
        let real = media
            .join("电视剧")
            .join("国内")
            .join("2024")
            .join("真实剧 (2024)");
        std::fs::create_dir_all(&real).expect("title dir");
        std::fs::write(real.join("S01E01.mkv"), vec![0u8; 1024]).unwrap();
        // A link inside the title points back at the title itself.
        symlink(&real, real.join("loop")).expect("self link");

        assert_eq!(
            service.lookup_library_size("真实剧").size_bytes,
            Some(1024),
            "the self link is skipped, not followed"
        );

        // A symlinked title is not a library entry, even with a perfect name.
        let linked = media.join("电视剧").join("国内").join("2025");
        std::fs::create_dir_all(&linked).expect("year dir");
        symlink(&real, linked.join("链接剧 (2025)")).expect("title link");
        assert_eq!(service.lookup_library_size("链接剧").size_bytes, None);
    }

    /// Manual scale check:
    /// `cargo test --locked --lib archive lookup_stays_fast -- --ignored --nocapture`.
    /// A lookup only descends to the title layer, and the second answer is
    /// served from the cache.
    #[test]
    #[ignore = "manual benchmark: run with --ignored --nocapture"]
    fn lookup_stays_fast_on_a_large_library() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let service = ArchiveService::new_for_tests(archive_config(tmp.path()));
        let media = tmp.path().join(MEDIA_DIR);
        let mut entry_count = 0usize;
        for board in ["电影", "电视剧", "动画", "综艺"] {
            for region in ["国内", "国外"] {
                for year in 2000..2025 {
                    let year_dir = media.join(board).join(region).join(year.to_string());
                    for index in 0..180 {
                        let dir = year_dir.join(format!("剧{index:03} ({year})"));
                        std::fs::create_dir_all(&dir).unwrap();
                        std::fs::write(dir.join("S01E01.mkv"), vec![0u8; 8]).unwrap();
                        entry_count += 2;
                    }
                }
            }
        }

        let started = Instant::now();
        let lookup = service.lookup_library_size("剧042 4K");
        let uncached = started.elapsed();
        let started = Instant::now();
        let cached = service.lookup_library_size("剧042 4K");
        let cached_time = started.elapsed();
        eprintln!(
            "library lookup over {entry_count} entries: first {uncached:?}, cached {cached_time:?}"
        );
        assert_eq!(lookup.matched, cached.matched);
        assert!(lookup.size_bytes.is_some());
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
