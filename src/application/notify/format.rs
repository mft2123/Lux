//! Pure message formatting for Telegram (HTML parse mode).
//!
//! Everything here is deterministic and covered by unit tests; the sender only
//! deals with HTTP and rate limits.

use time::{OffsetDateTime, UtcOffset};

use super::event::NotifyEvent;

/// Telegram rejects messages longer than 4096 UTF-16 code units; keep a safety
/// margin for the client-side rendering and for future format additions.
pub(crate) const MAX_MESSAGE_UNITS: usize = 3_900;
const MAX_OVERVIEW_CHARS: usize = 320;
const MAX_FIELD_VALUE_CHARS: usize = 400;
/// Hard cap for one rendered field value, measured in UTF-16 units *after*
/// escaping: `&` becomes `&amp;`, so a 400-character value could otherwise
/// expand to ~2000 units and starve every following field.
const MAX_FIELD_UNITS: usize = 400;
const FOOTER: &str = "— Lux";

pub(crate) fn format_message(event: &NotifyEvent, timezone_offset_hours: i64) -> String {
    format_message_with(event, timezone_offset_hours, MAX_MESSAGE_UNITS)
}

fn format_message_with(event: &NotifyEvent, timezone_offset_hours: i64, limit: usize) -> String {
    let heading = format!("{} {}", event.category.emoji(), event.title.trim());
    let time_line = format!(
        "🕐 时间：{}",
        format_timestamp(event.occurred_at, timezone_offset_hours)
    );
    // Reserve room for the closing lines so a long body never hides the time.
    let reserved = utf16_len(&time_line) + utf16_len(FOOTER) + 3;
    let mut budget = limit.saturating_sub(reserved);

    let max_title = budget.saturating_sub(utf16_len("<b></b>")).max(8);
    let title = truncate_escaped(&escape_html(&heading), max_title);
    let mut body = format!("<b>{title}</b>");
    budget = budget.saturating_sub(utf16_len(&body));

    for field in &event.fields {
        let label = escape_html(field.label.trim());
        let escaped = escape_html(&truncate_chars(field.value.trim(), MAX_FIELD_VALUE_CHARS));
        let value = truncate_escaped(&escaped, MAX_FIELD_UNITS);
        if label.is_empty() || value.is_empty() {
            continue;
        }
        let line = format!("{label}：{value}");
        if budget <= 2 {
            break;
        }
        let rendered = match append_with_budget(&line, budget - 1) {
            Some(rendered) => rendered,
            None => break,
        };
        budget = budget.saturating_sub(utf16_len(&rendered) + 1);
        body.push('\n');
        body.push_str(&rendered);
    }

    if budget > 2
        && let Some(overview) = event
            .overview
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    {
        let overview = escape_html(&truncate_chars(overview, MAX_OVERVIEW_CHARS));
        let line = format!("📝 简介：{overview}");
        if let Some(rendered) = append_with_budget(&line, budget.saturating_sub(2)) {
            budget = budget.saturating_sub(utf16_len(&rendered) + 2);
            body.push_str("\n\n");
            body.push_str(&rendered);
        }
    }

    if budget > 2
        && let Some(url) = event.url.as_deref()
    {
        let line = format!("🔗 <a href=\"{}\">查看详情</a>", escape_html(url));
        // Never truncate inside a tag: either the whole anchor fits or it is
        // dropped.
        if utf16_len(&line) + 1 <= budget {
            body.push('\n');
            body.push_str(&line);
        }
    }

    let mut message = body;
    message.push_str("\n\n");
    message.push_str(&time_line);
    message.push('\n');
    message.push_str(FOOTER);
    hard_limit(message, limit)
}

/// Telegram measures message length in UTF-16 code units, so an emoji counts
/// as two while a Chinese character counts as one.
pub(crate) fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// Render one line, truncating it with `…` when it does not fit `budget`.
fn append_with_budget(line: &str, budget: usize) -> Option<String> {
    if utf16_len(line) <= budget {
        return Some(line.to_owned());
    }
    if budget <= 2 {
        return None;
    }
    // Keep one unit for the ellipsis, and never cut inside an HTML entity.
    Some(truncate_escaped(line, budget - 1))
}

/// Hard cap for the final message (defensive: parts are already budgeted).
fn hard_limit(message: String, limit: usize) -> String {
    if utf16_len(&message) <= limit {
        return message;
    }
    let mut truncated = truncate_units(&message, limit);
    repair_incomplete_entity(&mut truncated);
    truncated
}

fn truncate_escaped(value: &str, max_units: usize) -> String {
    let mut truncated = truncate_units(value, max_units);
    repair_incomplete_entity(&mut truncated);
    truncated
}

fn truncate_units(value: &str, max_units: usize) -> String {
    if max_units == 0 {
        return String::new();
    }
    if utf16_len(value) <= max_units {
        return value.to_owned();
    }
    let budget = max_units - 1;
    let mut used = 0;
    let mut truncated = String::new();
    for ch in value.chars() {
        let units = ch.len_utf16();
        if used + units > budget {
            break;
        }
        used += units;
        truncated.push(ch);
    }
    truncated.push('…');
    truncated
}

/// When a cut lands inside an HTML entity (`&amp;`), drop the partial entity so
/// Telegram's parser does not reject the message.
fn repair_incomplete_entity(value: &mut String) {
    // The trailing ellipsis belongs to the output, not to the entity: ignore it
    // while checking, and keep it when the entity is dropped.
    let ellipsis = value.ends_with('…');
    let body_len = value.len() - if ellipsis { '…'.len_utf8() } else { 0 };
    let Some(index) = value[..body_len].rfind('&') else {
        return;
    };
    let tail = &value[index + 1..body_len];
    let incomplete = tail.len() <= 8
        && (tail.is_empty() || tail.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '#'));
    if incomplete {
        let mut repaired = value[..index].to_owned();
        if ellipsis {
            repaired.push('…');
        }
        *value = repaired;
    }
}

pub(crate) fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

pub(crate) fn truncate_chars(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut result: String = value.chars().take(max_chars).collect();
    if value.chars().count() > max_chars {
        result.pop();
        result.push('…');
    }
    result
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub(crate) fn format_timestamp(unix: i64, timezone_offset_hours: i64) -> String {
    let offset = UtcOffset::from_hms(timezone_offset_hours.clamp(-12, 14) as i8, 0, 0)
        .unwrap_or(UtcOffset::UTC);
    let datetime = OffsetDateTime::from_unix_timestamp(unix)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
        .to_offset(offset);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        datetime.year(),
        datetime.month() as u8,
        datetime.day(),
        datetime.hour(),
        datetime.minute(),
        datetime.second()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::notify::config::NotifyCategory;
    use crate::application::notify::event::NotifyField;

    fn playback_event() -> NotifyEvent {
        NotifyEvent {
            category: NotifyCategory::Playback,
            title: "正在播放《示例剧 · 第2季 · 第3集》".to_owned(),
            fields: vec![
                NotifyField {
                    label: "👤 用户".to_owned(),
                    value: "qiangli".to_owned(),
                },
                NotifyField {
                    label: "🎞️ 媒体".to_owned(),
                    value: "MKV · 2.5 GB".to_owned(),
                },
            ],
            overview: Some("一个用于测试的简介。".to_owned()),
            url: Some("https://example.com/item/1".to_owned()),
            occurred_at: 1_700_000_000,
            silent: None,
            dedupe_key: None,
        }
    }

    #[test]
    fn message_contains_title_fields_overview_and_footer() {
        let message = format_message(&playback_event(), 8);
        assert!(message.starts_with("<b>🎬 正在播放"));
        assert!(message.contains("👤 用户：qiangli"));
        assert!(message.contains("📝 简介：一个用于测试的简介。"));
        assert!(message.contains("🕐 时间：2023-11-15 06:13:20"));
        assert!(message.ends_with("— Lux"));
    }

    #[test]
    fn html_special_characters_are_escaped() {
        let event = NotifyEvent::new(NotifyCategory::Task, "<script>&\"'")
            .field("备注", "a < b & c > d");
        let message = format_message(&event, 8);
        assert!(message.contains("&lt;script&gt;&amp;&quot;&#39;"));
        assert!(message.contains("a &lt; b &amp; c &gt; d"));
        assert!(!message.contains("<script>"));
    }

    #[test]
    fn long_overview_is_truncated_but_message_stays_within_limit() {
        let mut event = playback_event();
        event.title = "很长的标题".repeat(400);
        event.fields = (0..20)
            .map(|index| NotifyField {
                label: format!("字段{index}"),
                value: "内容".repeat(400),
            })
            .collect();
        event.overview = Some("简介".repeat(2_000));
        let message = format_message(&event, 8);
        assert!(utf16_len(&message) <= MAX_MESSAGE_UNITS);
        assert!(message.contains('…'));
        assert!(message.ends_with("— Lux"));
    }

    #[test]
    fn emoji_count_as_two_utf16_units_and_stay_within_the_limit() {
        assert_eq!(utf16_len("🎬"), 2);
        assert_eq!(utf16_len("中文"), 2);
        let mut event = playback_event();
        event.title = "🎬🎞️✨".repeat(600);
        event.fields = (0..10)
            .map(|index| NotifyField {
                label: format!("🎞️ 字段{index}"),
                value: "✨内容🎬".repeat(120),
            })
            .collect();
        let message = format_message(&event, 8);
        assert!(utf16_len(&message) <= MAX_MESSAGE_UNITS);
    }

    #[test]
    fn tiny_budget_still_produces_a_valid_entity_stream() {
        let event = playback_event();
        let message = format_message_with(&event, 8, 200);
        assert!(utf16_len(&message) <= 200);
        assert!(!message.contains("&am\n"));
    }

    #[test]
    fn urls_are_escaped_into_anchor_tags() {
        let message = format_message(&playback_event(), 8);
        assert!(message.contains("<a href=\"https://example.com/item/1\">查看详情</a>"));
    }

    #[test]
    fn timestamp_uses_the_configured_offset() {
        assert_eq!(format_timestamp(0, 0), "1970-01-01 00:00:00");
        assert_eq!(format_timestamp(0, 8), "1970-01-01 08:00:00");
        assert_eq!(format_timestamp(0, -5), "1969-12-31 19:00:00");
    }

    #[test]
    fn bytes_are_human_readable() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2_500_000_000), "2.3 GB");
        assert_eq!(format_bytes(1_099_511_627_776), "1.0 TB");
    }

    #[test]
    fn entity_dense_fields_do_not_starve_the_following_lines() {
        // 400 ampersands escape to 2000 units; the field must be capped so the
        // next field still renders.
        let event = NotifyEvent::new(NotifyCategory::Task, "T")
            .field("A", "&".repeat(400))
            .field("B", "still here");
        let message = format_message(&event, 8);
        assert!(utf16_len(&message) <= MAX_MESSAGE_UNITS);
        assert!(message.contains("still here"), "second field was dropped: {message}");
        assert!(!message.contains('&') || message.contains("&amp;"));
    }

    #[test]
    fn a_truncated_field_never_ends_with_half_an_entity() {
        // The escape expands to ~2000 units, so the field has to be cut: the cut
        // must not leave a dangling `&am` behind.
        let event = NotifyEvent::new(NotifyCategory::Task, "T")
            .field("A", format!("{}&amp;", "x".repeat(395)));
        let message = format_message(&event, 8);
        for (index, _) in message.match_indices('&') {
            let tail = &message[index + 1..];
            let complete = match tail.find(';') {
                Some(end) if end <= 8 => tail[..end]
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '#'),
                _ => false,
            };
            assert!(complete, "half an entity survived near: {:?}", &message[index..]);
        }
    }

    #[test]
    fn truncation_repairs_partial_entities() {
        let mut value = "abc&amp".to_owned();
        repair_incomplete_entity(&mut value);
        assert_eq!(value, "abc");
    }
}
