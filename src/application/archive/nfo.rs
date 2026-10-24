//! Emby/Kodi-compatible metadata + strm writing for archived entries.
//!
//! The 115 side only holds media files. The media server scans a LOCAL mirror
//! tree that carries, per entry:
//!
//! - one `<file stem>.strm` per media file, containing the CD2 mount path the
//!   media server resolves (no 302, no credentials in the file);
//! - the NFO sidecar (`<stem>.nfo` for movies, `tvshow.nfo` for series,
//!   `<stem>.nfo` per episode);
//! - `poster.jpg` / `fanart.jpg`.
//!
//! Everything here is idempotent: an existing file is never overwritten unless
//! the caller asks for it.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{ArchiveService, MediaKind, ParsedName, TmdbMatch};

const POSTER_SIZE: &str = "w500";
const FANART_SIZE: &str = "w1280";
const IMAGE_BASE: &str = "https://image.tmdb.org/t/p";

/// Extra TMDB data needed beyond the match (plot, cast, images...).
#[derive(Clone, Debug, Default)]
pub(crate) struct NfoDetails {
    pub(crate) title: String,
    pub(crate) original_title: Option<String>,
    pub(crate) year: Option<i64>,
    pub(crate) premiered: Option<String>,
    pub(crate) plot: Option<String>,
    pub(crate) tagline: Option<String>,
    pub(crate) runtime_minutes: Option<i64>,
    pub(crate) rating: Option<f64>,
    pub(crate) genres: Vec<String>,
    pub(crate) countries: Vec<String>,
    pub(crate) studios: Vec<String>,
    pub(crate) directors: Vec<String>,
    pub(crate) writers: Vec<String>,
    pub(crate) cast: Vec<CastMember>,
    pub(crate) tmdb_id: i64,
    pub(crate) kind: MediaKind,
    pub(crate) poster_path: Option<String>,
    pub(crate) backdrop_path: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct CastMember {
    pub(crate) name: String,
    pub(crate) role: Option<String>,
    pub(crate) order: i64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct EpisodeMeta {
    pub(crate) season: u32,
    pub(crate) episode: u32,
    pub(crate) title: Option<String>,
    pub(crate) plot: Option<String>,
    pub(crate) aired: Option<String>,
    pub(crate) runtime_minutes: Option<i64>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct MetadataReport {
    pub(crate) created: Vec<String>,
    pub(crate) skipped: Vec<String>,
    pub(crate) failed: Vec<String>,
}

pub(crate) struct MetadataWriter<'a> {
    service: &'a ArchiveService,
    overwrite: bool,
}

impl<'a> MetadataWriter<'a> {
    pub(crate) fn new(service: &'a ArchiveService, overwrite: bool) -> Self {
        Self { service, overwrite }
    }

    /// Write the local mirror of one organized entry: `pack` is the 115 media
    /// pack, `mirror` is the local directory the media server scans.
    pub(crate) async fn write_entry(
        &self,
        pack: &Path,
        mirror: &Path,
        parsed: &ParsedName,
        matched: &TmdbMatch,
        report: &mut MetadataReport,
    ) {
        let details = match self
            .service
            .fetch_nfo_details(matched.kind, matched.tmdb_id)
            .await
        {
            Ok(details) => details,
            Err(error) => {
                report
                    .failed
                    .push(format!("{}: TMDB 详情失败: {error}", pack.display()));
                return;
            }
        };
        if let Err(error) = std::fs::create_dir_all(mirror) {
            report
                .failed
                .push(format!("{}: 创建镜像目录失败: {error}", mirror.display()));
            return;
        }
        relax_permissions(mirror, true);
        match matched.kind {
            MediaKind::Movie => self.write_movie(pack, mirror, &details, report).await,
            MediaKind::Series => self.write_series(pack, mirror, &details, parsed, report).await,
        }
    }

    async fn write_movie(
        &self,
        pack: &Path,
        mirror: &Path,
        details: &NfoDetails,
        report: &mut MetadataReport,
    ) {
        let videos = video_files(pack);
        if videos.is_empty() {
            self.write_file(&mirror.join("movie.nfo"), &movie_nfo(details), report);
        }
        for video in &videos {
            let relative = video.strip_prefix(pack).unwrap_or(video);
            let mirror_file = mirror.join(relative);
            if let Some(parent) = mirror_file.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    report
                        .failed
                        .push(format!("{}: {error}", parent.display()));
                    continue;
                }
            }
            let stem = mirror_file
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| "movie".to_owned());
            if let Some(url) = self.service.media_url(video) {
                self.write_file(&mirror_file.with_extension("strm"), &url, report);
            }
            self.write_file(
                &mirror.join(format!("{stem}.nfo")),
                &movie_nfo(details),
                report,
            );
        }
        self.download_image(details.poster_path.as_deref(), &mirror.join("poster.jpg"), report)
            .await;
        self.download_image(
            details.backdrop_path.as_deref(),
            &mirror.join("fanart.jpg"),
            report,
        )
        .await;
    }

    async fn write_series(
        &self,
        pack: &Path,
        mirror: &Path,
        details: &NfoDetails,
        _parsed: &ParsedName,
        report: &mut MetadataReport,
    ) {
        self.write_file(&mirror.join("tvshow.nfo"), &tvshow_nfo(details), report);
        self.download_image(details.poster_path.as_deref(), &mirror.join("poster.jpg"), report)
            .await;
        self.download_image(
            details.backdrop_path.as_deref(),
            &mirror.join("fanart.jpg"),
            report,
        )
        .await;

        // Group episode files by season so one TMDB request per season covers
        // every episode in it.
        let mut season_episodes: std::collections::BTreeMap<u32, Vec<(PathBuf, u32)>> =
            std::collections::BTreeMap::new();
        for video in video_files(pack) {
            let stem = video
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
            let Some(parsed) = super::parse_media_name(&stem) else {
                continue;
            };
            let (Some(season), Some(episode)) = (parsed.season, parsed.episode) else {
                continue;
            };
            let relative = video.strip_prefix(pack).unwrap_or(&video).to_path_buf();
            season_episodes
                .entry(season)
                .or_default()
                .push((relative, episode));
        }
        for (season, episodes) in season_episodes {
            let season_meta = self
                .service
                .fetch_season_episodes(details.kind, details.tmdb_id, season)
                .await
                .unwrap_or_default();
            for (relative, episode) in episodes {
                let mirror_file = mirror.join(&relative);
                if let Some(parent) = mirror_file.parent() {
                    if let Err(error) = std::fs::create_dir_all(parent) {
                        report
                            .failed
                            .push(format!("{}: {error}", parent.display()));
                        continue;
                    }
                }
                if let Some(url) = self.service.media_url(&pack.join(&relative)) {
                    self.write_file(&mirror_file.with_extension("strm"), &url, report);
                }
                let meta = season_meta
                    .iter()
                    .find(|meta| meta.episode == episode)
                    .cloned()
                    .unwrap_or(EpisodeMeta {
                        season,
                        episode,
                        ..EpisodeMeta::default()
                    });
                self.write_file(
                    &mirror_file.with_extension("nfo"),
                    &episode_nfo(&details.title, details.tmdb_id, &meta),
                    report,
                );
            }
        }
    }

    /// 晚会/演唱会 have no TMDB entry: mirror the media files with strm and a
    /// minimal NFO (title + year) so the media server still lists them.
    pub(crate) async fn write_special(
        &self,
        pack: &Path,
        mirror: &Path,
        title: &str,
        year: Option<i64>,
        report: &mut MetadataReport,
    ) {
        if let Err(error) = std::fs::create_dir_all(mirror) {
            report
                .failed
                .push(format!("{}: 创建镜像目录失败: {error}", mirror.display()));
            return;
        }
        relax_permissions(mirror, true);
        let videos = video_files(pack);
        if videos.is_empty() {
            self.write_file(&mirror.join("movie.nfo"), &minimal_nfo(title, year), report);
            return;
        }
        for video in &videos {
            let relative = video.strip_prefix(pack).unwrap_or(video);
            let mirror_file = mirror.join(relative);
            if let Some(parent) = mirror_file.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    report.failed.push(format!("{}: {error}", parent.display()));
                    continue;
                }
                relax_permissions(parent, true);
            }
            if let Some(url) = self.service.media_url(video) {
                self.write_file(&mirror_file.with_extension("strm"), &url, report);
            }
            self.write_file(
                &mirror_file.with_extension("nfo"),
                &minimal_nfo(title, year),
                report,
            );
        }
    }

    /// Short dramas arrive as loose episode files (`第01集.mp4`, `EP07.mp4`) that
    /// no scanner can number, and 115 keeps those names. The mirror is ours, so
    /// it gets the shape a scanner understands: `Season 01/<Title> S01E05.strm`
    /// plus an episode NFO (user decision 2026-10-23).
    pub(crate) async fn write_short_drama(
        &self,
        pack: &Path,
        mirror: &Path,
        title: &str,
        year: Option<i64>,
        report: &mut MetadataReport,
    ) {
        if let Err(error) = std::fs::create_dir_all(mirror) {
            report
                .failed
                .push(format!("{}: 创建镜像目录失败: {error}", mirror.display()));
            return;
        }
        relax_permissions(mirror, true);
        let videos = video_files(pack);
        if videos.is_empty() {
            self.write_file(&mirror.join("movie.nfo"), &minimal_nfo(title, year), report);
            return;
        }
        let season_dir = mirror.join("Season 01");
        if let Err(error) = std::fs::create_dir_all(&season_dir) {
            report
                .failed
                .push(format!("{}: {error}", season_dir.display()));
            return;
        }
        relax_permissions(&season_dir, true);
        let mut numbered = videos
            .into_iter()
            .map(|video| {
                let parsed = video
                    .file_name()
                    .and_then(|name| super::short_drama_episode_number(&name.to_string_lossy()));
                (parsed, video)
            })
            .collect::<Vec<_>>();
        // Files that already carry a number keep it; the rest fill the gaps in
        // name order so `第01集` and `EP02` never collide.
        numbered.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        for (index, (parsed, video)) in numbered.into_iter().enumerate() {
            let episode = parsed.unwrap_or(index as u32 + 1);
            let stem = format!("{title} S01E{episode:02}");
            if let Some(url) = self.service.media_url(&video) {
                self.write_file(&season_dir.join(format!("{stem}.strm")), &url, report);
            }
            self.write_file(
                &season_dir.join(format!("{stem}.nfo")),
                &minimal_episode_nfo(&format!("{title} 第{episode}集"), episode, year),
                report,
            );
        }
    }

    fn write_file(&self, path: &Path, content: &str, report: &mut MetadataReport) -> bool {
        if path.exists() && !self.overwrite {
            report.skipped.push(path.display().to_string());
            return false;
        }
        match std::fs::write(path, content) {
            Ok(()) => {
                relax_permissions(path, false);
                report.created.push(path.display().to_string());
                true
            }
            Err(error) => {
                report
                    .failed
                    .push(format!("{}: {error}", path.display()));
                false
            }
        }
    }

    async fn download_image(
        &self,
        image_path: Option<&str>,
        target: &Path,
        report: &mut MetadataReport,
    ) {
        let Some(image_path) = image_path else {
            return;
        };
        if target.exists() && !self.overwrite {
            return;
        }
        let size = if target.file_name().is_some_and(|name| name == "poster.jpg") {
            POSTER_SIZE
        } else {
            FANART_SIZE
        };
        let url = format!("{IMAGE_BASE}/{size}{image_path}");
        match self.service.download_bytes(&url).await {
            Ok(bytes) => match std::fs::write(target, bytes) {
                Ok(()) => {
                    relax_permissions(target, false);
                    report.created.push(target.display().to_string());
                }
                Err(error) => report
                    .failed
                    .push(format!("{}: {error}", target.display())),
            },
            Err(error) => report
                .failed
                .push(format!("{}: 图片下载失败: {error}", target.display())),
        }
    }
}

/// The mirror is read by the media server, which usually runs as another user
/// than the archive writer (a root container here). World-readable is the
/// simplest reliable contract.
fn relax_permissions(path: &Path, directory: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if directory { 0o755 } else { 0o644 };
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, directory);
}

fn video_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_video_files(directory, 0, &mut files);
    files.sort();
    files
}

fn collect_video_files(directory: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > 3 {
        return;
    }
    let Ok(reader) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_video_files(&path, depth + 1, files);
        } else if super::video_extension(&path).is_some() {
            files.push(path);
        }
    }
}

pub(crate) fn minimal_nfo(title: &str, year: Option<i64>) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<movie>\n");
    push_field(&mut xml, "title", title);
    if let Some(year) = year {
        push_field(&mut xml, "year", &year.to_string());
    }
    xml.push_str("</movie>\n");
    xml
}

/// Episode sidecar for a short drama whose 115 file names carry no season or
/// episode number.
pub(crate) fn minimal_episode_nfo(title: &str, episode: u32, year: Option<i64>) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<episodedetails>\n",
    );
    push_field(&mut xml, "title", title);
    push_field(&mut xml, "season", "1");
    push_field(&mut xml, "episode", &episode.to_string());
    if let Some(year) = year {
        push_field(&mut xml, "year", &year.to_string());
    }
    xml.push_str("</episodedetails>\n");
    xml
}

pub(crate) fn movie_nfo(details: &NfoDetails) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<movie>\n");
    push_field(&mut xml, "title", &details.title);
    if let Some(original) = &details.original_title {
        push_field(&mut xml, "originaltitle", original);
    }
    push_field(&mut xml, "sorttitle", &details.title);
    if let Some(year) = details.year {
        push_field(&mut xml, "year", &year.to_string());
    }
    if let Some(premiered) = &details.premiered {
        push_field(&mut xml, "premiered", premiered);
    }
    if let Some(plot) = &details.plot {
        push_field(&mut xml, "plot", plot);
        push_field(&mut xml, "outline", plot);
    }
    if let Some(tagline) = &details.tagline {
        push_field(&mut xml, "tagline", tagline);
    }
    if let Some(runtime) = details.runtime_minutes {
        push_field(&mut xml, "runtime", &runtime.to_string());
    }
    if let Some(rating) = details.rating {
        push_field(&mut xml, "rating", &format!("{rating:.1}"));
    }
    push_uniqueid(&mut xml, details.tmdb_id);
    for genre in &details.genres {
        push_field(&mut xml, "genre", genre);
    }
    for country in &details.countries {
        push_field(&mut xml, "country", country);
    }
    for studio in &details.studios {
        push_field(&mut xml, "studio", studio);
    }
    for director in &details.directors {
        push_field(&mut xml, "director", director);
    }
    for writer in &details.writers {
        push_field(&mut xml, "credits", writer);
    }
    for member in &details.cast {
        xml.push_str("  <actor>\n");
        push_indented(&mut xml, "name", &member.name);
        if let Some(role) = &member.role
            && !role.trim().is_empty()
        {
            push_indented(&mut xml, "role", role);
        }
        push_indented(&mut xml, "order", &member.order.to_string());
        xml.push_str("  </actor>\n");
    }
    xml.push_str("</movie>\n");
    xml
}

pub(crate) fn tvshow_nfo(details: &NfoDetails) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<tvshow>\n");
    push_field(&mut xml, "title", &details.title);
    if let Some(original) = &details.original_title {
        push_field(&mut xml, "originaltitle", original);
    }
    push_field(&mut xml, "sorttitle", &details.title);
    if let Some(year) = details.year {
        push_field(&mut xml, "year", &year.to_string());
    }
    if let Some(premiered) = &details.premiered {
        push_field(&mut xml, "premiered", premiered);
    }
    if let Some(plot) = &details.plot {
        push_field(&mut xml, "plot", plot);
    }
    if let Some(tagline) = &details.tagline {
        push_field(&mut xml, "tagline", tagline);
    }
    if let Some(runtime) = details.runtime_minutes {
        push_field(&mut xml, "runtime", &runtime.to_string());
    }
    if let Some(rating) = details.rating {
        push_field(&mut xml, "rating", &format!("{rating:.1}"));
    }
    push_uniqueid(&mut xml, details.tmdb_id);
    for genre in &details.genres {
        push_field(&mut xml, "genre", genre);
    }
    for country in &details.countries {
        push_field(&mut xml, "country", country);
    }
    for studio in &details.studios {
        push_field(&mut xml, "studio", studio);
    }
    for member in &details.cast {
        xml.push_str("  <actor>\n");
        push_indented(&mut xml, "name", &member.name);
        if let Some(role) = &member.role
            && !role.trim().is_empty()
        {
            push_indented(&mut xml, "role", role);
        }
        push_indented(&mut xml, "order", &member.order.to_string());
        xml.push_str("  </actor>\n");
    }
    xml.push_str("</tvshow>\n");
    xml
}

pub(crate) fn episode_nfo(series_title: &str, tmdb_id: i64, meta: &EpisodeMeta) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<episodedetails>\n");
    if let Some(title) = &meta.title {
        push_field(&mut xml, "title", title);
    }
    push_field(&mut xml, "showtitle", series_title);
    push_field(&mut xml, "season", &meta.season.to_string());
    push_field(&mut xml, "episode", &meta.episode.to_string());
    if let Some(plot) = &meta.plot {
        push_field(&mut xml, "plot", plot);
    }
    if let Some(aired) = &meta.aired {
        push_field(&mut xml, "aired", aired);
    }
    if let Some(runtime) = meta.runtime_minutes {
        push_field(&mut xml, "runtime", &runtime.to_string());
    }
    push_uniqueid(&mut xml, tmdb_id);
    xml.push_str("</episodedetails>\n");
    xml
}

fn push_uniqueid(xml: &mut String, tmdb_id: i64) {
    let _ = write!(
        xml,
        "  <uniqueid type=\"tmdb\" default=\"true\">{tmdb_id}</uniqueid>\n"
    );
}

fn push_field(xml: &mut String, name: &str, value: &str) {
    let _ = writeln!(xml, "  <{name}>{}</{name}>", escape_xml(value));
}

fn push_indented(xml: &mut String, name: &str, value: &str) {
    let _ = writeln!(xml, "    <{name}>{}</{name}>", escape_xml(value));
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl ArchiveService {
    pub(crate) async fn fetch_nfo_details(
        &self,
        kind: MediaKind,
        tmdb_id: i64,
    ) -> Result<NfoDetails, String> {
        let segment = if kind == MediaKind::Series { "tv" } else { "movie" };
        let url = format!(
            "https://api.themoviedb.org/3/{segment}/{tmdb_id}?api_key={}&language={}&append_to_response=credits",
            self.config.tmdb_api_key.as_deref().unwrap_or_default(),
            super::urlencode(&self.config.tmdb_language)
        );
        let body = self.get_json(&url).await?;
        Ok(nfo_details_from(kind, tmdb_id, &body))
    }

    pub(crate) async fn fetch_season_episodes(
        &self,
        kind: MediaKind,
        tmdb_id: i64,
        season: u32,
    ) -> Result<Vec<EpisodeMeta>, String> {
        if kind != MediaKind::Series {
            return Ok(Vec::new());
        }
        let url = format!(
            "https://api.themoviedb.org/3/tv/{tmdb_id}/season/{season}?api_key={}&language={}",
            self.config.tmdb_api_key.as_deref().unwrap_or_default(),
            super::urlencode(&self.config.tmdb_language)
        );
        let body = self.get_json(&url).await?;
        Ok(body
            .get("episodes")
            .and_then(|value| value.as_array())
            .map(|episodes| {
                episodes
                    .iter()
                    .map(|episode| EpisodeMeta {
                        season,
                        episode: episode
                            .get("episode_number")
                            .and_then(|value| value.as_i64())
                            .unwrap_or(0) as u32,
                        title: episode
                            .get("name")
                            .and_then(|value| value.as_str())
                            .map(str::to_owned),
                        plot: episode
                            .get("overview")
                            .and_then(|value| value.as_str())
                            .filter(|plot| !plot.trim().is_empty())
                            .map(str::to_owned),
                        aired: episode
                            .get("air_date")
                            .and_then(|value| value.as_str())
                            .map(str::to_owned),
                        runtime_minutes: episode
                            .get("runtime")
                            .and_then(|value| value.as_i64())
                            .filter(|runtime| *runtime > 0),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The path a media server sees for a media file: the archive root's
    /// relative position inside the CD2 mount, with the container's mount
    /// prefix. This is what a strm file contains.
    pub(crate) fn media_url(&self, host_path: &Path) -> Option<String> {
        let relative = host_path.strip_prefix(&self.config.root).ok()?;
        let relative = relative.to_string_lossy().replace('\\', "/");
        let mount = self.config.cd2_container_mount.trim_end_matches('/');
        let prefix = self.config.cd2_prefix.trim_matches('/');
        let path = if prefix.is_empty() {
            format!("{mount}/{relative}")
        } else {
            format!("{mount}/{prefix}/{relative}")
        };
        Some(path)
    }

    pub(crate) async fn get_json(&self, url: &str) -> Result<Value, String> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|error| format!("TMDB 响应解析失败: {error}"))?;
        if !status.is_success() {
            return Err(format!("TMDB HTTP {status}"));
        }
        Ok(body)
    }

    pub(crate) async fn download_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| error.to_string())
    }
}

fn nfo_details_from(kind: MediaKind, tmdb_id: i64, body: &Value) -> NfoDetails {
    let text = |key: &str| {
        body.get(key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let title_key = if kind == MediaKind::Series { "name" } else { "title" };
    let date_key = if kind == MediaKind::Series {
        "first_air_date"
    } else {
        "release_date"
    };
    let title = text(title_key).unwrap_or_default();
    let original_title = text(if kind == MediaKind::Series {
        "original_name"
    } else {
        "original_title"
    });
    let year = body
        .get(date_key)
        .and_then(|value| value.as_str())
        .and_then(|date| date.get(..4))
        .and_then(|year| year.parse::<i64>().ok());
    let genres = body
        .get("genres")
        .and_then(|value| value.as_array())
        .map(|genres| {
            genres
                .iter()
                .filter_map(|genre| genre.get("name").and_then(|name| name.as_str()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let countries = body
        .get("production_countries")
        .and_then(|value| value.as_array())
        .map(|countries| {
            countries
                .iter()
                .filter_map(|country| country.get("name").and_then(|name| name.as_str()))
                .map(str::to_owned)
                .collect()
        })
        .or_else(|| {
            body.get("origin_country")
                .and_then(|value| value.as_array())
                .map(|countries| {
                    countries
                        .iter()
                        .filter_map(|country| country.as_str())
                        .map(str::to_owned)
                        .collect()
                })
        })
        .unwrap_or_default();
    let studios = body
        .get("production_companies")
        .and_then(|value| value.as_array())
        .map(|companies| {
            companies
                .iter()
                .filter_map(|company| company.get("name").and_then(|name| name.as_str()))
                .map(str::to_owned)
                .collect()
        })
        .or_else(|| {
            body.get("networks")
                .and_then(|value| value.as_array())
                .map(|networks| {
                    networks
                        .iter()
                        .filter_map(|network| network.get("name").and_then(|name| name.as_str()))
                        .map(str::to_owned)
                        .collect()
                })
        })
        .unwrap_or_default();
    let credits = body.get("credits");
    let cast = credits
        .and_then(|credits| credits.get("cast"))
        .and_then(|value| value.as_array())
        .map(|cast| {
            cast.iter()
                .take(20)
                .enumerate()
                .filter_map(|(index, member)| {
                    let name = member.get("name").and_then(|name| name.as_str())?;
                    Some(CastMember {
                        name: name.to_owned(),
                        role: member
                            .get("character")
                            .and_then(|value| value.as_str())
                            .map(str::to_owned),
                        order: member
                            .get("order")
                            .and_then(|value| value.as_i64())
                            .unwrap_or(index as i64),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let crew = credits
        .and_then(|credits| credits.get("crew"))
        .and_then(|value| value.as_array());
    let mut directors = Vec::new();
    let mut writers = Vec::new();
    if let Some(crew) = crew {
        for member in crew {
            let Some(name) = member.get("name").and_then(|name| name.as_str()) else {
                continue;
            };
            match member.get("job").and_then(|job| job.as_str()) {
                Some("Director") => directors.push(name.to_owned()),
                Some("Writer") | Some("Screenplay") | Some("Story") => {
                    writers.push(name.to_owned());
                }
                _ => {}
            }
        }
    }
    let runtime_minutes = body
        .get("runtime")
        .and_then(|value| value.as_i64())
        .filter(|runtime| *runtime > 0)
        .or_else(|| {
            body.get("episode_run_time")
                .and_then(|value| value.as_array())
                .and_then(|runtimes| runtimes.first())
                .and_then(|value| value.as_i64())
        });
    NfoDetails {
        title,
        original_title,
        year,
        premiered: text(date_key),
        plot: text("overview"),
        tagline: text("tagline"),
        runtime_minutes,
        rating: body.get("vote_average").and_then(|value| value.as_f64()),
        genres,
        countries,
        studios,
        directors,
        writers,
        cast,
        tmdb_id,
        kind,
        poster_path: text("poster_path"),
        backdrop_path: text("backdrop_path"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_movie() -> NfoDetails {
        NfoDetails {
            title: "300".to_owned(),
            original_title: Some("300".to_owned()),
            year: Some(2006),
            premiered: Some("2006-12-09".to_owned()),
            plot: Some("斯巴达 <300> & 勇士".to_owned()),
            tagline: Some("Spartans!".to_owned()),
            runtime_minutes: Some(117),
            rating: Some(7.6),
            genres: vec!["动作".to_owned(), "战争".to_owned()],
            countries: vec!["美国".to_owned()],
            studios: vec!["Warner Bros.".to_owned()],
            directors: vec!["Zack Snyder".to_owned()],
            writers: vec!["Frank Miller".to_owned()],
            cast: vec![CastMember {
                name: "Gerard Butler".to_owned(),
                role: Some("Leonidas".to_owned()),
                order: 0,
            }],
            tmdb_id: 1271,
            kind: MediaKind::Movie,
            poster_path: Some("/abc.jpg".to_owned()),
            backdrop_path: Some("/def.jpg".to_owned()),
        }
    }

    #[test]
    fn movie_nfo_contains_metadata_and_escapes_xml() {
        let nfo = movie_nfo(&sample_movie());
        assert!(nfo.starts_with("<?xml"));
        assert!(nfo.contains("<title>300</title>"));
        assert!(nfo.contains("<year>2006</year>"));
        assert!(nfo.contains("<runtime>117</runtime>"));
        assert!(nfo.contains("<genre>动作</genre>"));
        assert!(nfo.contains("<uniqueid type=\"tmdb\" default=\"true\">1271</uniqueid>"));
        assert!(nfo.contains("斯巴达 &lt;300&gt; &amp; 勇士"));
        assert!(nfo.contains("<actor>"));
        assert!(nfo.contains("<role>Leonidas</role>"));
    }

    #[test]
    fn tvshow_nfo_and_episode_nfo_are_well_formed() {
        let mut show = sample_movie();
        show.kind = MediaKind::Series;
        show.title = "大唐迷雾".to_owned();
        let nfo = tvshow_nfo(&show);
        assert!(nfo.contains("<tvshow>"));
        assert!(nfo.contains("<title>大唐迷雾</title>"));

        let episode = episode_nfo(
            "大唐迷雾",
            123,
            &EpisodeMeta {
                season: 1,
                episode: 7,
                title: Some("第七集".to_owned()),
                plot: Some("剧情".to_owned()),
                aired: Some("2026-03-01".to_owned()),
                runtime_minutes: Some(45),
            },
        );
        assert!(episode.contains("<season>1</season>"));
        assert!(episode.contains("<episode>7</episode>"));
        assert!(episode.contains("<showtitle>大唐迷雾</showtitle>"));
        assert!(episode.contains("<uniqueid type=\"tmdb\" default=\"true\">123</uniqueid>"));
    }

    #[test]
    fn special_categories_are_detected_from_names() {
        assert_eq!(super::super::special_category("2008年北京奥运会开幕式"), Some("晚会"));
        assert_eq!(super::super::special_category("国庆70周年阅兵"), Some("晚会"));
        assert_eq!(super::super::special_category("2024中秋晚会"), Some("晚会"));
        assert_eq!(super::super::special_category("周杰伦演唱会 2024"), Some("演唱会"));
        assert_eq!(super::super::special_category("Taylor Swift Concert"), Some("演唱会"));
        assert_eq!(super::super::special_category("{演唱会}某场演出"), Some("演唱会"));
        assert_eq!(super::super::special_category("{晚会}某场演出"), Some("晚会"));
        assert_eq!(super::super::special_category("Inception.2010.1080p"), None);
        assert!(super::super::contains_cjk("周杰伦"));
        assert!(!super::super::contains_cjk("Taylor Swift"));
    }

    #[test]
    fn minimal_nfo_for_special_entries() {
        let nfo = minimal_nfo("2008年北京奥运会开幕式", Some(2008));
        assert!(nfo.contains("<title>2008年北京奥运会开幕式</title>"));
        assert!(nfo.contains("<year>2008</year>"));
    }

    #[test]
    fn media_url_maps_the_cd2_mount() {
        let config = super::super::ArchiveConfig {
            root: PathBuf::from("/testlib"),
            tmdb_api_key: None,
            tmdb_language: "zh-CN".to_owned(),
            settle_seconds: 3,
            wash_enabled: true,
            wash_tolerance_ratio: 0.02,
            watch_dirs: Vec::new(),
            mirror_root: PathBuf::from("/library"),
            cd2_prefix: "测试库".to_owned(),
            cd2_container_mount: "/media/cloud".to_owned(),
            transfer115_base: String::new(),
            transfer115_cookie: String::new(),
            transfer115_target_dir: "转存目录".to_owned(),
            transfer115_offline_dir: "离线下载".to_owned(),
            transfer115_timeout: std::time::Duration::from_secs(5),
        };
        let service = ArchiveService::new_for_tests(config);
        let url = service
            .media_url(Path::new(
                "/testlib/媒体目录/国外电影/2006/斯巴达300勇士 (2006)/300.mkv",
            ))
            .expect("url");
        assert_eq!(
            url,
            "/media/cloud/测试库/媒体目录/国外电影/2006/斯巴达300勇士 (2006)/300.mkv"
        );
        assert!(service.media_url(Path::new("/elsewhere/300.mkv")).is_none());
    }
}
