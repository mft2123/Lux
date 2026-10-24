//! Telegram channel → archive decision chain.
//!
//! The sidecar reads channels and hands every candidate link to Lux; the
//! decision (入新 / 追更 / 洗版 / 跳过) is made **here** because it needs the
//! catalog: whether the work exists at all, which episodes are missing and how
//! good the copy we already have is. Keeping that in Rust also means one
//! implementation of the quality rules instead of a Python copy that drifts.
//!
//! Everything in this module is pure: the caller loads a [`LibrarySnapshot`]
//! and gets decisions plus a human-readable reason for each one.

use serde::{Deserialize, Serialize};

/// One resource link found in a channel message.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanCandidate {
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) year: Option<i64>,
    #[serde(default)]
    pub(crate) season: Option<i64>,
    #[serde(default)]
    pub(crate) episode: Option<i64>,
    /// Free-form quality text as it appeared in the release name ("4K", "1080p").
    #[serde(default)]
    pub(crate) quality: String,
    #[serde(default)]
    pub(crate) size_bytes: i64,
    pub(crate) link: String,
}

/// One channel message; a message may carry several candidates.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanMessage {
    pub(crate) message_id: i64,
    #[serde(default)]
    pub(crate) candidates: Vec<PlanCandidate>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanRequest {
    pub(crate) channel_id: String,
    #[serde(default)]
    pub(crate) messages: Vec<PlanMessage>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanAction {
    /// 入新: the library does not have this work at all.
    NewRelease,
    /// 追更: the work exists but seasons/episodes are missing.
    Follow,
    /// 洗版: the work is complete, the incoming copy is strictly better.
    Remux,
    /// 跳过: no mode allows it, or the copy we have is good enough.
    Skip,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanDecision {
    pub(crate) message_id: i64,
    pub(crate) link: String,
    pub(crate) title: String,
    pub(crate) season: Option<i64>,
    pub(crate) episode: Option<i64>,
    pub(crate) action: PlanAction,
    pub(crate) reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) matched_item_id: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanResponse {
    pub(crate) channel_id: String,
    /// Highest message id in the request: the sidecar stores it as the cursor
    /// once every decision below has been acted on.
    pub(crate) cursor: i64,
    pub(crate) decisions: Vec<PlanDecision>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) notes: Vec<String>,
}

/// The channel's four automatic bits, resolved from settings.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ChannelModes {
    pub(crate) new_release: bool,
    pub(crate) follow: bool,
    pub(crate) full: bool,
    pub(crate) remux: bool,
}

impl ChannelModes {
    fn allows_new_release(self) -> bool {
        self.new_release || self.full
    }

    fn allows_follow(self) -> bool {
        self.follow || self.full
    }

    fn allows_remux(self) -> bool {
        self.remux || self.full
    }


}

/// One library work the planner can match against.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LibraryWork {
    pub(crate) id: String,
    /// `MOVIE` or `SERIES`.
    pub(crate) item_type: String,
    pub(crate) title: String,
    pub(crate) production_year: Option<i64>,
    /// `(season, episode)` pairs already in the library.
    pub(crate) episodes: Vec<(i64, i64)>,
    /// Best quality label across the work's sources.
    pub(crate) best_quality: Option<String>,
    /// Largest source size, used as the tie-break when quality is equal.
    pub(crate) best_size: i64,
}

impl LibraryWork {
    fn is_series(&self) -> bool {
        self.item_type == "SERIES"
    }

    fn has_episode(&self, season: Option<i64>, episode: Option<i64>) -> bool {
        match (season, episode) {
            (Some(season), Some(episode)) => self
                .episodes
                .iter()
                .any(|(s, e)| *s == season && *e == episode),
            // "S01" without an episode number means the whole season: present
            // only when every episode of it is already here is unknowable from
            // a count alone, so treat any episode of that season as present.
            (Some(season), None) => self.episodes.iter().any(|(s, _)| *s == season),
            _ => false,
        }
    }
}

/// Resolution tier of a release name. Higher wins; unknown sorts below 480p so
/// a labelled copy always beats an unlabelled one.
pub(crate) fn quality_rank(label: &str) -> u8 {
    let lowered = label.to_ascii_lowercase();
    if lowered.contains("2160") || lowered.contains("4k") || lowered.contains("uhd") {
        4
    } else if lowered.contains("1080") {
        3
    } else if lowered.contains("720") {
        2
    } else if lowered.contains("480") || lowered.contains("576") || lowered.contains("sd") {
        1
    } else {
        0
    }
}

fn normalized_title(title: &str) -> String {
    title
        .trim()
        .to_lowercase()
        .replace([' ', '\u{3000}', '.', '_', '-'], "")
}

/// Find the library work a candidate refers to.
///
/// Title match is exact after normalisation; a candidate year, when present,
/// must agree with the stored one, so `流浪地球 2019` does not match the sequel.
fn find_work<'a>(candidate: &PlanCandidate, library: &'a [LibraryWork]) -> Option<&'a LibraryWork> {
    let wanted = normalized_title(&candidate.title);
    if wanted.is_empty() {
        return None;
    }
    let mut fallback = None;
    for work in library {
        if normalized_title(&work.title) != wanted {
            continue;
        }
        match (candidate.year, work.production_year) {
            (Some(year), Some(stored)) if year == stored => return Some(work),
            (Some(_), Some(_)) => continue,
            _ => {
                if fallback.is_none() {
                    fallback = Some(work);
                }
            }
        }
    }
    fallback
}

/// Why the incoming copy is strictly better, if it is.
///
/// Strictly better means a higher resolution tier, or the same tier and a
/// meaningfully larger file (10%). Equal-or-worse never replaces: a remux that
/// merely re-encodes the same thing is churn, not an upgrade. The note becomes
/// the human-readable reason, so the operator can see *why* a link was taken.
fn improvement(candidate: &PlanCandidate, work: &LibraryWork) -> Option<String> {
    let incoming = quality_rank(&candidate.quality);
    let existing = work
        .best_quality
        .as_deref()
        .map(quality_rank)
        .unwrap_or_default();
    if incoming > existing {
        // "未知 → 1080p" reads like a mistake; say what actually changed.
        let Some(was) = work.best_quality.as_deref() else {
            return Some(format!(
                "新版本有清晰度标注（{}），库里那份没有",
                candidate.quality.trim()
            ));
        };
        return Some(format!(
            "新版本清晰度更高（{was} → {}）",
            candidate.quality.trim()
        ));
    }
    // A STRM row can carry a junk size (a few hundred bytes); only a real file
    // size is meaningful as a tie-break, and only when both sides are real.
    const MIN_COMPARABLE_SIZE: i64 = 1024 * 1024;
    if incoming == existing
        && candidate.size_bytes >= MIN_COMPARABLE_SIZE
        && work.best_size >= MIN_COMPARABLE_SIZE
    {
        let ratio = candidate.size_bytes as f64 / work.best_size as f64;
        if ratio >= 1.10 {
            return Some(format!(
                "清晰度相同但体积大 {:.0}%（{} → {}）",
                (ratio - 1.0) * 100.0,
                human_size(work.best_size),
                human_size(candidate.size_bytes)
            ));
        }
    }
    None
}

fn human_size(bytes: i64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit >= 3 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

fn episode_label(season: Option<i64>, episode: Option<i64>) -> String {
    match (season, episode) {
        (Some(season), Some(episode)) => format!("S{season:02}E{episode:02}"),
        (Some(season), None) => format!("S{season:02}"),
        (None, Some(episode)) => format!("E{episode:02}"),
        (None, None) => "整包".to_owned(),
    }
}

/// Decide one candidate.
pub(crate) fn decide_one(
    candidate: &PlanCandidate,
    modes: ChannelModes,
    library: &[LibraryWork],
) -> (PlanAction, String, Option<String>) {
    let Some(work) = find_work(candidate, library) else {
        if modes.allows_new_release() {
            return (
                PlanAction::NewRelease,
                format!("库里没有《{}》，按入新投递", candidate.title.trim()),
                None,
            );
        }
        return (
            PlanAction::Skip,
            format!(
                "库里没有《{}》，该频道没开入新/全量",
                candidate.title.trim()
            ),
            None,
        );
    };

    let id = Some(work.id.clone());
    // "《X》S01E03" for an episode, "《X》" for a movie or a whole pack.
    let place = if work.is_series() && (candidate.season.is_some() || candidate.episode.is_some()) {
        episode_label(candidate.season, candidate.episode)
    } else {
        String::new()
    };
    let described = |work: &LibraryWork| {
        let quality = work
            .best_quality
            .clone()
            .unwrap_or_else(|| "未知清晰度".to_owned());
        if place.is_empty() {
            format!("《{}》（{quality}）", work.title)
        } else {
            format!("《{}》{place}（{quality}）", work.title)
        }
    };

    if work.is_series() {
        // A whole-pack (no episode number) is a 追更 candidate whenever the
        // series is not obviously complete; without per-season totals we can
        // only say "we don't have this episode number yet".
        let missing = !work.has_episode(candidate.season, candidate.episode)
            && (candidate.season.is_some() || candidate.episode.is_some());
        if missing {
            if modes.allows_follow() {
                return (
                    PlanAction::Follow,
                    format!("《{}》缺 {place}", work.title),
                    id,
                );
            }
            return (
                PlanAction::Skip,
                format!("《{}》缺 {place}，该频道没开追更/全量", work.title),
                id,
            );
        }
        if candidate.season.is_none() && candidate.episode.is_none() {
            if modes.allows_follow() {
                return (
                    PlanAction::Follow,
                    format!("《{}》是整包，交给追更流程判断缺集", work.title),
                    id,
                );
            }
            return (
                PlanAction::Skip,
                format!("《{}》是整包，该频道没开追更/全量", work.title),
                id,
            );
        }
    }

    // The work (or that episode) is already here: only a strictly better copy
    // is worth a transfer.
    if let Some(note) = improvement(candidate, work) {
        if modes.allows_remux() {
            return (
                PlanAction::Remux,
                format!("已有{}，{note}", described(work)),
                id,
            );
        }
        return (
            PlanAction::Skip,
            format!("已有{}，{note}，但该频道没开洗版/全量", described(work)),
            id,
        );
    }

    (
        PlanAction::Skip,
        format!("已有{}，版本不比它差", described(work)),
        id,
    )
}

/// Clean a candidate title and fill the fields the sidecar could not parse.
///
/// The sidecar sends the raw release name (`都市古仙医 S01E221 1080p`) as the
/// title. Matching needs the work title alone, so the same parser the archive
/// pipeline uses strips the tags and fills season / episode / year when the
/// caller left them out.
pub(crate) fn normalize_candidate(candidate: &mut PlanCandidate) {
    let Some(parsed) = crate::application::archive::parse_media_name(&candidate.title) else {
        return;
    };
    let cleaned = parsed.title.trim();
    if !cleaned.is_empty() {
        candidate.title = cleaned.to_owned();
    }
    if candidate.year.is_none() {
        candidate.year = parsed.year;
    }
    if candidate.season.is_none() {
        candidate.season = parsed.season.map(i64::from);
    }
    if candidate.episode.is_none() {
        candidate.episode = parsed.episode.map(i64::from);
    }
}

/// Decide every candidate in a request.
///
/// The cursor is the highest message id seen, so a sidecar can record progress
/// even when nothing was transferred.
pub(crate) fn plan(
    mut request: PlanRequest,
    modes: ChannelModes,
    library: &[LibraryWork],
) -> PlanResponse {
    for message in &mut request.messages {
        for candidate in &mut message.candidates {
            normalize_candidate(candidate);
        }
    }
    let mut decisions = Vec::new();
    let mut cursor = 0;
    let mut notes = Vec::new();
    for message in &request.messages {
        cursor = cursor.max(message.message_id);
        if message.candidates.is_empty() {
            notes.push(format!("消息 {} 没有识别出资源链接", message.message_id));
            continue;
        }
        for candidate in &message.candidates {
            let (action, reason, matched_item_id) = decide_one(candidate, modes, library);
            decisions.push(PlanDecision {
                message_id: message.message_id,
                link: candidate.link.clone(),
                title: candidate.title.trim().to_owned(),
                season: candidate.season,
                episode: candidate.episode,
                action,
                reason,
                matched_item_id,
            });
        }
    }
    PlanResponse {
        channel_id: request.channel_id.clone(),
        cursor,
        decisions,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes(new_release: bool, follow: bool, full: bool, remux: bool) -> ChannelModes {
        ChannelModes {
            new_release,
            follow,
            full,
            remux,
        }
    }

    fn candidate(title: &str, season: Option<i64>, episode: Option<i64>, quality: &str) -> PlanCandidate {
        PlanCandidate {
            title: title.to_owned(),
            year: None,
            season,
            episode,
            quality: quality.to_owned(),
            size_bytes: 0,
            link: "https://115.com/s/x".to_owned(),
        }
    }

    fn series(title: &str, episodes: &[(i64, i64)], quality: &str) -> LibraryWork {
        LibraryWork {
            id: format!("series-{title}"),
            item_type: "SERIES".to_owned(),
            title: title.to_owned(),
            production_year: None,
            episodes: episodes.to_vec(),
            best_quality: Some(quality.to_owned()),
            best_size: 1_000,
        }
    }

    fn movie(title: &str, quality: &str) -> LibraryWork {
        LibraryWork {
            id: format!("movie-{title}"),
            item_type: "MOVIE".to_owned(),
            title: title.to_owned(),
            production_year: None,
            episodes: Vec::new(),
            best_quality: Some(quality.to_owned()),
            best_size: 1_000,
        }
    }

    /// 入新 needs the bit; without it the link is skipped with a reason that
    /// names the missing permission instead of failing silently.
    #[test]
    fn missing_work_needs_the_new_release_bit() {
        let link = candidate("繁花", Some(1), Some(1), "4K");
        let (action, reason, _) = decide_one(&link, modes(true, false, false, false), &[]);
        assert_eq!(action, PlanAction::NewRelease);
        assert!(reason.contains("入新"), "{reason}");

        let (action, reason, _) = decide_one(&link, modes(false, true, false, false), &[]);
        assert_eq!(action, PlanAction::Skip);
        assert!(reason.contains("没开入新"), "{reason}");
    }

    /// A missing episode is 追更, and the reason names the exact episode.
    #[test]
    fn missing_episode_becomes_follow() {
        let library = vec![series("繁花", &[(1, 1), (1, 2)], "1080p")];
        let link = candidate("繁花", Some(1), Some(3), "1080p");

        let (action, reason, id) = decide_one(&link, modes(false, true, false, false), &library);
        assert_eq!(action, PlanAction::Follow);
        assert_eq!(id.as_deref(), Some("series-繁花"));
        assert!(reason.contains("S01E03"), "{reason}");

        let (action, reason, _) = decide_one(&link, modes(true, false, false, false), &library);
        assert_eq!(action, PlanAction::Skip);
        assert!(reason.contains("没开追更"), "{reason}");
    }

    /// An episode we already have is only worth a transfer when the incoming
    /// copy is strictly better — and only when 洗版 is on.
    #[test]
    fn existing_episode_only_remuxes_when_strictly_better() {
        let library = vec![series("繁花", &[(1, 3)], "1080p")];

        let same = candidate("繁花", Some(1), Some(3), "1080P");
        let (action, reason, _) = decide_one(&same, modes(false, false, false, true), &library);
        assert_eq!(action, PlanAction::Skip);
        assert!(reason.contains("不比它差"), "{reason}");

        let better = candidate("繁花", Some(1), Some(3), "2160p");
        let (action, reason, _) = decide_one(&better, modes(false, false, false, true), &library);
        assert_eq!(action, PlanAction::Remux);
        assert!(reason.contains("1080p"), "{reason}");

        // The same better copy without 洗版 stays home.
        let (action, reason, _) = decide_one(&better, modes(true, true, false, false), &library);
        assert_eq!(action, PlanAction::Skip);
        assert!(reason.contains("没开洗版"), "{reason}");
    }

    /// 全量 turns every bit on at once.
    #[test]
    fn full_enables_every_action() {
        assert_eq!(
            decide_one(&candidate("新片", None, None, "4K"), modes(false, false, true, false), &[]).0,
            PlanAction::NewRelease
        );
        let library = vec![series("繁花", &[(1, 1)], "1080p")];
        assert_eq!(
            decide_one(
                &candidate("繁花", Some(1), Some(2), "1080p"),
                modes(false, false, true, false),
                &library
            )
            .0,
            PlanAction::Follow
        );
        assert_eq!(
            decide_one(
                &candidate("繁花", Some(1), Some(1), "4K"),
                modes(false, false, true, false),
                &library
            )
            .0,
            PlanAction::Remux
        );
    }

    /// A movie only ever competes on quality.
    #[test]
    fn movies_compare_quality() {
        let library = vec![movie("流浪地球", "1080p")];
        assert_eq!(
            decide_one(
                &candidate("流浪地球", None, None, "2160p"),
                modes(false, false, false, true),
                &library
            )
            .0,
            PlanAction::Remux
        );
        assert_eq!(
            decide_one(
                &candidate("流浪地球", None, None, "720p"),
                modes(true, true, true, true),
                &library
            )
            .0,
            PlanAction::Skip
        );
    }

    /// Titles are compared after normalisation, and a year mismatch must not
    /// silently match the wrong work.
    #[test]
    fn title_matching_normalises_but_respects_the_year() {
        let mut work = movie("流浪地球", "1080p");
        work.production_year = Some(2019);
        let library = vec![work];

        let mut hit = candidate(" 流浪地球 ", None, None, "4K");
        hit.year = Some(2019);
        assert_eq!(
            decide_one(&hit, modes(false, false, false, true), &library).0,
            PlanAction::Remux
        );

        let mut miss = candidate("流浪地球", None, None, "4K");
        miss.year = Some(2023);
        // No year match and the only work has a year: treated as a different
        // work, so 入新 decides instead of 洗版.
        assert_eq!(
            decide_one(&miss, modes(true, false, false, false), &library).0,
            PlanAction::NewRelease
        );
    }

    /// The cursor is the highest message id even when everything was skipped,
    /// and messages without links are reported instead of dropped.
    #[test]
    fn cursor_covers_skipped_messages() {
        let request = PlanRequest {
            channel_id: "-100".to_owned(),
            messages: vec![
                PlanMessage { message_id: 7, candidates: vec![] },
                PlanMessage {
                    message_id: 9,
                    candidates: vec![candidate("不存在的片", None, None, "4K")],
                },
            ],
        };
        let response = plan(request, modes(false, false, false, false), &[]);
        assert_eq!(response.cursor, 9);
        assert_eq!(response.decisions.len(), 1);
        assert_eq!(response.decisions[0].action, PlanAction::Skip);
        assert_eq!(response.notes.len(), 1);
        assert!(response.notes[0].contains("没有识别出资源链接"));
    }

    /// The sidecar sends raw release names; they must match the library title
    /// and fill the season/episode the sidecar could not see.
    #[test]
    fn release_names_are_normalised_before_matching() {
        let library = vec![series("都市古仙医", &[(1, 220)], "1080p")];
        let request = PlanRequest {
            channel_id: "-1".to_owned(),
            messages: vec![PlanMessage {
                message_id: 5,
                candidates: vec![candidate("都市古仙医 S01E221 1080p", None, None, "1080p")],
            }],
        };

        let response = plan(request, modes(false, true, false, false), &library);
        let decision = &response.decisions[0];
        assert_eq!(decision.action, PlanAction::Follow);
        assert_eq!(decision.title, "都市古仙医");
        assert_eq!(decision.season, Some(1));
        assert_eq!(decision.episode, Some(221));
        assert!(decision.reason.contains("S01E221"), "{}", decision.reason);
    }

    /// A release name that also carries the year must still match a library
    /// copy of the same year.
    #[test]
    fn release_names_with_a_year_still_match() {
        let mut work = movie("流浪地球", "1080p");
        work.production_year = Some(2019);
        let library = vec![work];
        let request = PlanRequest {
            channel_id: "-1".to_owned(),
            messages: vec![PlanMessage {
                message_id: 6,
                candidates: vec![candidate("流浪地球.2019.2160p", None, None, "2160p")],
            }],
        };

        let response = plan(request, modes(false, false, false, true), &library);
        assert_eq!(response.decisions[0].action, PlanAction::Remux);
        assert_eq!(response.decisions[0].title, "流浪地球");
    }

    /// Same resolution: only a meaningfully larger file counts, and the reason
    /// names the size gap instead of repeating the quality twice.
    #[test]
    fn same_quality_needs_a_real_size_gain() {
        let mut library = vec![series("繁花", &[(1, 1)], "1080p")];
        library[0].best_size = 2_000_000_000;
        let mut bigger = candidate("繁花", Some(1), Some(1), "1080p");
        bigger.size_bytes = 2_500_000_000; // +25%

        let (action, reason, _) = decide_one(&bigger, modes(false, false, false, true), &library);
        assert_eq!(action, PlanAction::Remux);
        assert!(reason.contains("体积大"), "{reason}");
        assert!(reason.contains("25%"), "{reason}");

        let mut marginal = candidate("繁花", Some(1), Some(1), "1080p");
        marginal.size_bytes = 2_100_000_000; // +5%, below the margin
        assert_eq!(
            decide_one(&marginal, modes(false, false, false, true), &library).0,
            PlanAction::Skip
        );

        // A junk size (a STRM row with a few hundred bytes) must not turn into
        // an absurd "体积大 3355704598%".
        let mut strm = library.clone();
        strm[0].best_size = 149;
        assert_eq!(
            decide_one(&bigger, modes(false, false, false, true), &strm).0,
            PlanAction::Skip
        );
    }

    /// When the library copy has no quality label, the reason says that instead
    /// of the nonsensical "未知清晰度 → 1080p".
    #[test]
    fn unlabelled_library_copy_gets_a_readable_reason() {
        let mut library = vec![series("繁花", &[(1, 1)], "1080p")];
        library[0].best_quality = None;
        let link = candidate("繁花", Some(1), Some(1), "1080p");
        let (action, reason, _) = decide_one(&link, modes(false, false, false, true), &library);
        assert_eq!(action, PlanAction::Remux);
        assert!(reason.contains("库里那份没有"), "{reason}");
    }

    #[test]
    fn quality_rank_orders_resolutions() {
        assert!(quality_rank("2160p") > quality_rank("1080p"));
        assert!(quality_rank("4K") > quality_rank("1080P"));
        assert!(quality_rank("1080p") > quality_rank("720p"));
        assert!(quality_rank("480p") > quality_rank("web-dl"));
        assert!(quality_rank("WEB-DL") < quality_rank("480p"));
    }
}
