//! Auto library provisioning: every non-empty board of the local mirror becomes
//! a Lux media library, and a freshly created library is scanned once.
//!
//! Contract: `docs/handoff/AUTO-LIBRARY.md`. Two triggers funnel into
//! [`provision`]: the end of an archive run and a startup reconciliation. Both
//! are idempotent because the library root path is the idempotency key.
//!
//! One library per board — 电影 / 电视剧 / 动画 / 综艺 / 纪录片 / 特摄剧 / 晚会 /
//! 演唱会 …, exactly like 电影 and 电视剧 are today (user decision 2026-10-21). The
//! 国内/国外 level is a directory *inside* the library, never a second library:
//!
//! - `动画/国内/2020s/…` 和 `动画/国外/2020s/…` 都属于同一个「动画」库；
//! - `动漫` 是同一板块的另一种拼法，合并进「动画」库（多个根）而不是新建一个库；
//! - 动画与纪录片板块的库类型是 MIXED：既有电影也有剧集，扫描按文件逐个判定。

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::{
    application::{
        admin_library_settings::{CreateLibraryInput, create_library},
        admin_scan_jobs::{ScanWorkerServices, start_library_scan},
        libraries::LibraryService,
        scanner::ScanJobService,
    },
    domain::ids::LibraryId,
    library::LibraryKind,
    storage::Database,
};

/// Board spellings that are the same board: merged into one library.
const BOARD_ALIASES: &[(&str, &str)] = &[("动漫", "动画")];
/// Boards whose scan has to classify each file instead of forcing one parser:
/// an animation or documentary board carries films and episodes alike, and a
/// 演唱会/晚会 board holds single concert films next to multi-part recordings.
const MIXED_BOARDS: &[&str] = &["动画", "纪录片", "演唱会", "晚会"];
const STRM_EXTENSION: &str = "strm";
const NFO_EXTENSION: &str = "nfo";
/// A board is deep (`board/region/decade/title/Season/file`); the probe only
/// walks until it finds the first sidecar, so the cap only bounds broken trees.
const CONTENT_PROBE_DEPTH: usize = 8;

/// Handles the archive poller needs to provision libraries and queue scans.
#[derive(Clone)]
pub(crate) struct AutoLibraryServices {
    pub(crate) database: Database,
    pub(crate) libraries: LibraryService,
    pub(crate) scan_jobs: ScanJobService,
}

/// One library the mirror should have: a name, a kind, and the roots that feed
/// it (several when the board is spelled more than one way).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LibraryPlan {
    pub(crate) name: String,
    pub(crate) kind: LibraryKind,
    pub(crate) roots: Vec<PathBuf>,
}

#[derive(Debug, Default)]
pub(crate) struct AutoLibraryReport {
    pub(crate) created: Vec<String>,
    pub(crate) existing: Vec<String>,
    pub(crate) updated: Vec<String>,
    pub(crate) failed: Vec<String>,
    pub(crate) scans: Vec<String>,
}

impl AutoLibraryReport {
    /// Whether the run changed anything or hit a problem (quiet runs stay quiet
    /// in the log, they happen every poll tick).
    pub(crate) fn worth_logging(&self) -> bool {
        !self.created.is_empty()
            || !self.updated.is_empty()
            || !self.scans.is_empty()
            || !self.failed.is_empty()
    }
}

/// The libraries the mirror should have right now.
///
/// Only the first level of the mirror is a board: a title directory is never
/// promoted to a library. A board without a single strm/nfo anywhere is skipped
/// so an empty directory cannot produce an empty library.
pub(crate) fn plan_libraries(mirror_root: &Path) -> Vec<LibraryPlan> {
    let mut plans = Vec::new();
    for board in sorted_subdirectories(mirror_root) {
        if !has_media_content(&board) {
            continue;
        }
        let board_name = file_name(&board);
        let name = canonical_board(&board_name).to_owned();
        plans.push(LibraryPlan {
            kind: kind_for_board(&name),
            name,
            roots: vec![board],
        });
    }
    merge_same_named_plans(plans)
}

/// The board a directory belongs to: `动漫` is the 动画 board.
fn canonical_board(name: &str) -> &str {
    BOARD_ALIASES
        .iter()
        .find(|(alias, _)| name.contains(alias))
        .map(|(_, canonical)| *canonical)
        .unwrap_or(name)
}

/// Merge plans that ended up with the same name: 动画 and 动漫 are the same
/// board, so they contribute one library with both roots.
fn merge_same_named_plans(plans: Vec<LibraryPlan>) -> Vec<LibraryPlan> {
    let mut merged: Vec<LibraryPlan> = Vec::new();
    for plan in plans {
        match merged.iter_mut().find(|kept| kept.name == plan.name) {
            Some(kept) => {
                for root in plan.roots {
                    if !kept.roots.contains(&root) {
                        kept.roots.push(root);
                    }
                }
            }
            None => merged.push(plan),
        }
    }
    merged
}

fn kind_for_board(name: &str) -> LibraryKind {
    if name.contains("电影") {
        LibraryKind::Movie
    } else if MIXED_BOARDS.iter().any(|board| name.contains(board)) {
        LibraryKind::Mixed
    } else {
        LibraryKind::Series
    }
}

/// Create the missing libraries (and the missing roots of a partly known one)
/// and queue one full scan for each library created here.
///
/// A library whose root is already registered is never re-created, so running
/// this on every poll tick is safe. Failures are collected instead of returned:
/// a broken board must not stop the archive pipeline.
pub(crate) async fn provision(
    database: &Database,
    libraries: &LibraryService,
    scan_jobs: Option<&ScanJobService>,
    mirror_root: &Path,
) -> AutoLibraryReport {
    let mut report = AutoLibraryReport::default();
    if !mirror_root.is_dir() {
        return report;
    }
    let plans = plan_libraries(mirror_root);
    let mut known: HashSet<String> = match database.list_all_library_roots().await {
        Ok(roots) => roots
            .into_iter()
            .map(|root| normalize_path(&root.canonical_path))
            .collect(),
        Err(error) => {
            report.failed.push(format!("读取库根目录失败: {error:?}"));
            return report;
        }
    };
    let mut existing: HashMap<String, LibraryId> = match database.list_libraries().await {
        Ok(libraries) => libraries
            .into_iter()
            .filter_map(|library| {
                library
                    .id
                    .parse::<LibraryId>()
                    .ok()
                    .map(|id| (library.name.clone(), id))
            })
            .collect(),
        Err(error) => {
            report.failed.push(format!("读取媒体库失败: {error:?}"));
            return report;
        }
    };
    for plan in plans {
        let missing: Vec<PathBuf> = plan
            .roots
            .iter()
            .filter(|root| !known.contains(&normalize_path(&root.to_string_lossy())))
            .cloned()
            .collect();
        if missing.is_empty() {
            report.existing.push(plan.name);
            continue;
        }
        let (library_id, created) = match existing.get(&plan.name).cloned() {
            Some(id) => (id, false),
            None => {
                let library = match create_library(
                    libraries,
                    None,
                    None,
                    CreateLibraryInput {
                        name: &plan.name,
                        kind: plan.kind,
                        // The full scan below is what fills the library; turning
                        // the filesystem watcher on can be done in the settings
                        // page.
                        realtime_watch_enabled: false,
                        scrapers: &[],
                        chapter_source_id: None,
                        realtime_metadata_auto_match_enabled: false,
                    },
                )
                .await
                {
                    Ok(library) => library,
                    Err(error) => {
                        report.failed.push(format!("{}: 建库失败 {error:?}", plan.name));
                        continue;
                    }
                };
                existing.insert(plan.name.clone(), library.id);
                (library.id, true)
            }
        };
        let mut added = 0_usize;
        for root in &missing {
            if let Err(error) = libraries.add_root(library_id, &root.to_string_lossy()).await {
                report
                    .failed
                    .push(format!("{}: 挂载库根失败 {error:?}", plan.name));
                continue;
            }
            known.insert(normalize_path(&root.to_string_lossy()));
            added += 1;
        }
        if added == 0 && !created {
            continue;
        }
        if created {
            report.created.push(plan.name.clone());
        } else {
            report.updated.push(plan.name.clone());
        }
        let Some(scan_jobs) = scan_jobs else {
            continue;
        };
        // A scan is already running for that library: nothing to queue.
        match start_library_scan(scan_jobs, library_id, ScanWorkerServices::default()).await {
            Ok(Some(_)) => report.scans.push(plan.name),
            Ok(None) => {}
            Err(error) => report
                .failed
                .push(format!("{}: 扫描入队失败 {error:?}", plan.name)),
        }
    }
    report
}

fn sorted_subdirectories(directory: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .map(|reader| {
            reader
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether a directory holds at least one strm/nfo somewhere below it. Walking
/// stops at the first match: a populated board answers on the first title.
fn has_media_content(directory: &Path) -> bool {
    has_media_content_at(directory, 0)
}

fn has_media_content_at(directory: &Path, depth: usize) -> bool {
    if depth > CONTENT_PROBE_DEPTH {
        return false;
    }
    let Ok(reader) = std::fs::read_dir(directory) else {
        return false;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_dir() {
            if has_media_content_at(&path, depth + 1) {
                return true;
            }
            continue;
        }
        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if extension == STRM_EXTENSION || extension == NFO_EXTENSION {
            return true;
        }
    }
    false
}

fn normalize_path(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::{LibraryPlan, plan_libraries, provision};
    use crate::{
        application::libraries::LibraryService, config::Config, library::LibraryKind,
        storage::Database,
    };
    use std::path::Path;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
        std::fs::write(path, b"x").expect("file");
    }

    fn mirror() -> tempfile::TempDir {
        let mirror = tempfile::tempdir().expect("mirror");
        let root = mirror.path();
        touch(&root.join("电影/国外/2020s/某片 (2024)/某片.strm"));
        touch(&root.join("电影/国内/2010s/另一片 (2014)/另一片.strm"));
        touch(&root.join("电视剧/国内/2020s/某剧 (2024)/Season 01/S01E01.strm"));
        // Both regions live inside one library.
        touch(&root.join("动画/国内/2020s/国漫 (2024)/S01E01.strm"));
        touch(&root.join("动画/国外/2020s/日漫 (2024)/S01E01.strm"));
        // 纪录片 and 特摄剧 are boards of their own, region-split like the rest.
        touch(&root.join("纪录片/国外/2010s/地球脉动 (2006)/S01E01.strm"));
        touch(&root.join("特摄剧/国外/2020s/欧布奥特曼 (2016)/S01E01.strm"));
        // An empty board must not become an empty library.
        std::fs::create_dir_all(root.join("综艺")).expect("empty board");
        touch(&root.join("演唱会/国内/2010s/某演唱会 (2019)/某演唱会.strm"));
        mirror
    }

    fn plan_named<'a>(plans: &'a [LibraryPlan], name: &str) -> &'a LibraryPlan {
        plans
            .iter()
            .find(|plan| plan.name == name)
            .unwrap_or_else(|| panic!("{name} planned: {plans:?}"))
    }

    #[test]
    fn plans_one_library_per_board_exactly_like_movies_and_series() {
        let mirror = mirror();
        let plans = plan_libraries(mirror.path());
        assert_eq!(
            plans.len(),
            6,
            "电影/电视剧/动画/演唱会/纪录片/特摄剧, without the empty 综艺: {plans:?}"
        );
        assert_eq!(plan_named(&plans, "电影").kind, LibraryKind::Movie);
        assert_eq!(plan_named(&plans, "电视剧").kind, LibraryKind::Series);
        // A concert board is usually single films, but multi-part recordings
        // exist too, so the scan has to classify each file.
        assert_eq!(plan_named(&plans, "演唱会").kind, LibraryKind::Mixed);
        // Animation mixes films and episodes, so the scan classifies each file.
        assert_eq!(plan_named(&plans, "动画").kind, LibraryKind::Mixed);
        // Documentaries do the same; 特摄剧 is a plain series board.
        assert_eq!(plan_named(&plans, "纪录片").kind, LibraryKind::Mixed);
        assert_eq!(plan_named(&plans, "特摄剧").kind, LibraryKind::Series);
        // The region level never becomes a library of its own.
        assert!(plans.iter().all(|plan| !plan.name.contains("国内")
            && !plan.name.contains("国外")));
        assert!(plans.iter().all(|plan| plan.name != "综艺"));
    }

    #[test]
    fn both_regions_feed_the_one_animation_library() {
        let mirror = mirror();
        let plans = plan_libraries(mirror.path());
        assert_eq!(
            plan_named(&plans, "动画").roots,
            vec![mirror.path().join("动画")]
        );
    }

    /// `动漫` is the same board as `动画`: one library, two roots.
    #[test]
    fn the_anime_alias_merges_into_the_animation_library() {
        let mirror = tempfile::tempdir().expect("mirror");
        touch(&mirror.path().join("动画/国内/2020s/国漫 (2024)/S01E01.strm"));
        touch(&mirror.path().join("动漫/日本动漫/2020s/日漫 (2024)/S01E01.strm"));
        let plans = plan_libraries(mirror.path());
        assert_eq!(plans.len(), 1, "{plans:?}");
        let animation = plan_named(&plans, "动画");
        assert_eq!(animation.kind, LibraryKind::Mixed);
        assert_eq!(
            animation.roots,
            vec![
                mirror.path().join("动漫"),
                mirror.path().join("动画"),
            ]
        );
    }

    #[tokio::test]
    async fn provisioning_is_idempotent() -> Result<(), Box<dyn std::error::Error>> {
        let mirror = mirror();
        let config_dir = tempfile::tempdir()?;
        let config = Config {
            http_addr: "127.0.0.1:8097".parse()?,
            config_dir: config_dir.path().join("config"),
        };
        let database = Database::connect(&config).await?;
        let libraries = LibraryService::new(database.clone());

        let first = provision(&database, &libraries, None, mirror.path()).await;
        assert!(first.failed.is_empty(), "{:?}", first.failed);
        assert_eq!(first.created.len(), 6, "{first:?}");
        assert!(first.created.iter().any(|name| name == "动画"));

        let second = provision(&database, &libraries, None, mirror.path()).await;
        assert!(second.created.is_empty(), "{second:?}");
        assert!(second.updated.is_empty(), "{second:?}");
        assert_eq!(second.existing.len(), 6, "{second:?}");

        let stored = database.list_all_library_roots().await?;
        assert_eq!(stored.len(), 6);
        Ok(())
    }
}
