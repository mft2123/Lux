//! Auto library provisioning: every non-empty board of the local mirror becomes
//! a Lux media library, and a freshly created library is scanned once.
//!
//! Contract: `docs/handoff/AUTO-LIBRARY.md`. Two triggers funnel into
//! [`provision`]: the end of an archive run and a startup reconciliation. Both
//! are idempotent because the library root path is the idempotency key.
//!
//! `动画` is browsed as two libraries — `国内动画` / `国外动画` — because that is
//! the split the user asked for (2026-10-21); the mirror layout already carries
//! the region level for every board. Every other board stays one library.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use crate::{
    application::{
        admin_library_settings::{CreateLibraryInput, create_library},
        admin_scan_jobs::{ScanWorkerServices, start_library_scan},
        libraries::LibraryService,
        scanner::ScanJobService,
    },
    library::LibraryKind,
    storage::Database,
};

/// Boards split into one library per region. The name of the created library is
/// `<region><board>`, for example `国内动画`.
const REGION_SPLIT_BOARDS: &[&str] = &["动画"];
const DOMESTIC: &str = "国内";
const FOREIGN: &str = "国外";
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

/// One library the mirror should have, derived from a directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LibraryPlan {
    pub(crate) name: String,
    pub(crate) kind: LibraryKind,
    pub(crate) root: PathBuf,
}

#[derive(Debug, Default)]
pub(crate) struct AutoLibraryReport {
    pub(crate) created: Vec<String>,
    pub(crate) existing: Vec<String>,
    pub(crate) failed: Vec<String>,
    pub(crate) scans: Vec<String>,
}

impl AutoLibraryReport {
    /// Whether the run changed anything or hit a problem (quiet runs stay quiet
    /// in the log, they happen every poll tick).
    pub(crate) fn worth_logging(&self) -> bool {
        !self.created.is_empty() || !self.scans.is_empty() || !self.failed.is_empty()
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
        let name = file_name(&board);
        if REGION_SPLIT_BOARDS
            .iter()
            .any(|split| name.contains(split))
        {
            let mut split: Vec<LibraryPlan> = [DOMESTIC, FOREIGN]
                .into_iter()
                .map(|region| LibraryPlan {
                    name: format!("{region}{name}"),
                    kind: LibraryKind::Series,
                    root: board.join(region),
                })
                .filter(|plan| plan.root.is_dir() && has_media_content(&plan.root))
                .collect();
            if split.is_empty() && has_media_content(&board) {
                // A board written before the region split (or one whose entries
                // all sit in one region directory) stays browsable as a single
                // library instead of disappearing from the web.
                split.push(LibraryPlan {
                    name,
                    kind: LibraryKind::Series,
                    root: board,
                });
            }
            plans.append(&mut split);
            continue;
        }
        if !has_media_content(&board) {
            continue;
        }
        let kind = if name.contains("电影") {
            LibraryKind::Movie
        } else {
            LibraryKind::Series
        };
        plans.push(LibraryPlan {
            name,
            kind,
            root: board,
        });
    }
    plans
}

/// Create the missing libraries and queue one full scan for each new one.
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
            report
                .failed
                .push(format!("读取库根目录失败: {error:?}"));
            return report;
        }
    };
    for plan in plans {
        let root = normalize_path(&plan.root.to_string_lossy());
        if known.contains(&root) {
            report.existing.push(plan.name);
            continue;
        }
        let library = match create_library(
            libraries,
            None,
            None,
            CreateLibraryInput {
                name: &plan.name,
                kind: plan.kind,
                // The full scan below is what fills the library; turning the
                // filesystem watcher on can be done in the settings page.
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
        if let Err(error) = libraries
            .add_root(library.id, &plan.root.to_string_lossy())
            .await
        {
            report
                .failed
                .push(format!("{}: 挂载库根失败 {error:?}", plan.name));
            continue;
        }
        known.insert(root);
        report.created.push(plan.name.clone());
        let Some(scan_jobs) = scan_jobs else {
            continue;
        };
        match start_library_scan(scan_jobs, library.id, ScanWorkerServices::default()).await {
            Ok(Some(_)) => report.scans.push(plan.name),
            // A scan is already running for that library: nothing to queue.
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
        touch(&root.join("动画/国内/2020s/国漫 (2024)/S01E01.strm"));
        touch(&root.join("动画/国外/2020s/日漫 (2024)/S01E01.strm"));
        // An empty board must not become an empty library.
        std::fs::create_dir_all(root.join("综艺")).expect("empty board");
        // A board without the region level keeps a single library.
        touch(&root.join("纪录片/2020s/某纪录片 (2024)/某纪录片.strm"));
        mirror
    }

    #[test]
    fn plans_one_library_per_board_and_splits_animation() {
        let mirror = mirror();
        let plans = plan_libraries(mirror.path());
        let by_name = |name: &str| {
            plans
                .iter()
                .find(|plan| plan.name == name)
                .unwrap_or_else(|| panic!("{name} planned: {plans:?}"))
                .clone()
        };
        assert_eq!(
            plans.len(),
            5,
            "电影/电视剧/国内动画/国外动画/纪录片, without the empty 综艺: {plans:?}"
        );
        assert_eq!(by_name("电影").kind, LibraryKind::Movie);
        assert_eq!(by_name("电视剧").kind, LibraryKind::Series);
        assert_eq!(by_name("国内动画").kind, LibraryKind::Series);
        assert_eq!(by_name("国外动画").kind, LibraryKind::Series);
        assert!(plans.iter().all(|plan| plan.name != "综艺"));
    }

    #[test]
    fn an_animation_board_without_regions_stays_one_library() {
        let mirror = tempfile::tempdir().expect("mirror");
        touch(&mirror.path().join("动画/2010s/老动画 (2012)/S01E01.strm"));
        let plans = plan_libraries(mirror.path());
        assert_eq!(
            plans,
            vec![LibraryPlan {
                name: "动画".to_owned(),
                kind: LibraryKind::Series,
                root: mirror.path().join("动画"),
            }]
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
        assert_eq!(first.created.len(), 5, "{first:?}");
        assert!(first.created.iter().any(|name| name == "国内动画"));
        assert!(first.created.iter().any(|name| name == "国外动画"));

        let second = provision(&database, &libraries, None, mirror.path()).await;
        assert!(second.created.is_empty(), "{second:?}");
        assert_eq!(second.existing.len(), 5, "{second:?}");

        let stored = database.list_all_library_roots().await?;
        assert_eq!(stored.len(), 5);
        Ok(())
    }
}
