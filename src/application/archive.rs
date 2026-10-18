//! Archive pipeline: transfer -> pending -> media, with failure and duplicate
//! sinks. This is the first slice of the chill replacement work (plan U9):
//! resources land in the transfer directory, are moved to pending, are
//! identified through TMDB, and are organized into
//! `<media>/<国内|国外><类型>/<年份>/<Title (Year)>/`.
//!
//! The pipeline is deliberately directory-driven: every stage is a visible
//! directory role, so a failed identification can be inspected by hand and a
//! half-finished run leaves no hidden state.

use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

pub(crate) const ARCHIVE_ROOT_ENV: &str = "LUX_ARCHIVE_ROOT";
pub(crate) const ARCHIVE_TMDB_KEY_ENV: &str = "LUX_TMDB_API_KEY";
pub(crate) const ARCHIVE_TMDB_LANGUAGE_ENV: &str = "LUX_TMDB_LANGUAGE";
pub(crate) const DEFAULT_ARCHIVE_ROOT: &str = "/vol5/1000/docker/lux-test";
pub(crate) const ARCHIVE_POLL_SECONDS_ENV: &str = "LUX_ARCHIVE_POLL_SECONDS";
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

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "ts", "m2ts", "webm", "rmvb", "rm", "mpg", "mpeg",
];

#[derive(Clone, Debug)]
pub(crate) struct ArchiveConfig {
    pub(crate) root: PathBuf,
    pub(crate) tmdb_api_key: Option<String>,
    pub(crate) tmdb_language: String,
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

    /// Create every directory role. Idempotent.
    pub(crate) fn ensure_directories(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.transfer_dir())?;
        std::fs::create_dir_all(self.pending_dir())?;
        std::fs::create_dir_all(self.media_dir())?;
        std::fs::create_dir_all(self.failed_dir())?;
        std::fs::create_dir_all(self.duplicate_dir())?;
        std::fs::create_dir_all(self.wash_dir())?;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MediaKind {
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
        }
        let mut report = ArchiveReport {
            dry_run,
            ..ArchiveReport::default()
        };

        self.archive_transfer_entries(&mut report).await?;
        self.organize_pending_entries(&mut report).await?;
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
            let board = format!("{region}{}", matched.category_label());
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
                .join(&board)
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
                Ok(()) => report.organized.push(OrganizedItem {
                    source: name.clone(),
                    tmdb_id: matched.tmdb_id,
                    target: target.display().to_string(),
                    kind: matched.category_label().to_owned(),
                    region: region.to_owned(),
                    year,
                    title: matched.title.clone(),
                }),
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
            let Some(best) = pick_best_result(&results, &parsed.title, parsed.year) else {
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
                let mut seen = results
                    .iter()
                    .filter_map(|result| result.get("id").and_then(|value| value.as_i64()))
                    .collect::<std::collections::HashSet<_>>();
                for item in extra {
                    if let Some(id) = item.get("id").and_then(|value| value.as_i64())
                        && seen.insert(id)
                    {
                        results.push(item);
                    }
                }
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

    pub(crate) fn status(&self) -> ArchiveStatus {
        ArchiveStatus {
            root: self.config.root.display().to_string(),
            transfer_dir: self.config.transfer_dir().display().to_string(),
            pending_dir: self.config.pending_dir().display().to_string(),
            media_dir: self.config.media_dir().display().to_string(),
            failed_dir: self.config.failed_dir().display().to_string(),
            duplicate_dir: self.config.duplicate_dir().display().to_string(),
            wash_dir: self.config.wash_dir().display().to_string(),
            settle_seconds: self.config.settle_seconds,
            wash_enabled: self.config.wash_enabled,
            wash_tolerance_ratio: self.config.wash_tolerance_ratio,
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
    pub(crate) settle_seconds: u64,
    pub(crate) wash_enabled: bool,
    pub(crate) wash_tolerance_ratio: f64,
    pub(crate) tmdb_configured: bool,
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

    #[test]
    fn sanitizes_path_components() {
        // Illegal path separators collapse, but a colon becomes a full-width
        // colon so titles like `20:23` survive.
        assert_eq!(sanitize_component("A/B: C*?"), "A B： C");
        assert_eq!(sanitize_component("20:23"), "20：23");
        assert_eq!(sanitize_component("  .  "), "未命名");
    }
}
