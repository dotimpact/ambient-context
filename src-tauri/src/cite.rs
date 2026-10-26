//! Time-range citations, shared by the ingest and summary validators. Both
//! ask the same question of a model's output: does this range point at a
//! time the day file actually recorded?

use regex::Regex;
use std::sync::OnceLock;

/// `09:14-09:41` or `09:14–09:41`. Both dashes appear in practice: the day
/// file's own headings use an en dash and models copy either.
fn range() -> &'static Regex {
    static RANGE: OnceLock<Regex> = OnceLock::new();
    RANGE.get_or_init(|| {
        Regex::new(r"\b(\d{1,2}):(\d{2})\s*[-\x{2013}]\s*(\d{1,2}):(\d{2})\b").unwrap()
    })
}

/// A date written just before the time: `2026/09/05 15:00`, `9/5(土)
/// 15:00`, `9月5日 15:00`, `2026-09-05 15:00`.
fn date_suffix() -> &'static Regex {
    static DATE: OnceLock<Regex> = OnceLock::new();
    DATE.get_or_init(|| {
        Regex::new(
            r"(\d{1,4}/\d{1,2}(/\d{1,4})?|\d{1,2}月\d{1,2}日|\d{4}-\d{2}-\d{2})\s*([(\x{FF08}][^)\x{FF09}]{1,4}[)\x{FF09}])?\s*$",
        )
        .unwrap()
    })
}

/// Whether the time starting at byte `at` carries its own date. Such a time
/// is something the day's record says ("the 2026/09/05 14:00-15:00 meeting
/// was cancelled"), not a pointer at when it was recorded.
pub fn is_dated(text: &str, at: usize) -> bool {
    date_suffix().is_match(&text[..at])
}

/// The ranges in `text` that are citations, as `(text, start, end)`
/// minutes, dated ones left out.
fn citations(text: &str) -> impl Iterator<Item = (String, u32, u32)> + '_ {
    range().captures_iter(text).filter_map(move |caps| {
        let whole = caps.get(0)?;
        if is_dated(text, whole.start()) {
            return None;
        }
        let n = |i: usize| caps[i].parse::<u32>().unwrap_or(0);
        Some((
            whole.as_str().to_string(),
            n(1) * 60 + n(2),
            n(3) * 60 + n(4),
        ))
    })
}

pub fn has_citation(text: &str) -> bool {
    citations(text).next().is_some()
}

/// Whether `minute` falls within the stretch the day was recorded over,
/// first block start to last block end. Not block by block: the agent
/// summarises a recurring activity as one wide range and writes its ends from
/// memory, so an end landing in a gap says nothing about accuracy, and
/// rejecting it failed whole days. A block running past midnight is recorded
/// with an end minute beyond 24:00, so a citation after midnight has to be
/// tried in both frames.
pub fn inside(minute: u32, spans: &[(u32, u32)]) -> bool {
    let (Some(first), Some(last)) = (
        spans.iter().map(|(s, _)| *s).min(),
        spans.iter().map(|(_, e)| *e).max(),
    ) else {
        return false;
    };
    (first..=last).contains(&minute) || (first..=last).contains(&(minute + 24 * 60))
}

/// Every range in `text` must start and end within the recorded day. A
/// citation the day file cannot account for is worse than no citation: it
/// reads as evidence and is not. A range with its own date is a fact about
/// another day and is not checked. Returns the first offending range.
pub fn citation_in_spans(text: &str, spans: &[(u32, u32)]) -> Result<(), String> {
    for (range, start, end) in citations(text) {
        if !inside(start, spans) || !inside(end, spans) {
            return Err(range);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans() -> Vec<(u32, u32)> {
        vec![(540, 660), (720, 750)]
    }

    #[test]
    fn a_citation_inside_a_block_is_accepted_with_either_dash() {
        for sep in ["-", "\u{2013}", " - "] {
            assert!(citation_in_spans(&format!("did a thing 09:30{sep}10:00"), &spans()).is_ok());
        }
    }

    #[test]
    fn a_citation_outside_the_recorded_day_is_named_back() {
        assert_eq!(
            citation_in_spans("wrote it up 13:10-13:40", &spans()).unwrap_err(),
            "13:10-13:40"
        );
        assert_eq!(
            citation_in_spans("started early 08:30-10:00", &spans()).unwrap_err(),
            "08:30-10:00"
        );
    }

    #[test]
    fn an_end_in_a_gap_between_blocks_is_accepted() {
        assert!(citation_in_spans("wrote it up 11:10-11:40", &spans()).is_ok());
        assert!(!inside(0, &[]));
    }

    #[test]
    fn a_block_running_past_midnight_covers_the_early_hours() {
        assert!(inside(30, &[(1380, 1470)]));
    }

    #[test]
    fn a_range_with_its_own_date_is_not_a_citation() {
        for dated in [
            "2026/09/05 14:00-15:00",
            "9/5(土) 14:00-15:00",
            "9月5日14:00-15:00",
            "2026-09-05 14:00-15:00",
        ] {
            let text = format!("the {dated} meeting was cancelled 09:30-09:31");
            assert!(citation_in_spans(&text, &spans()).is_ok(), "{dated}");
            assert!(!has_citation(&format!("the {dated} meeting was cancelled")));
        }
        assert!(citation_in_spans("moved to 14:00-15:00", &spans()).is_err());
    }

    #[test]
    fn text_with_no_ranges_has_no_citation_and_nothing_to_check() {
        assert!(!has_citation("no ranges at all"));
        assert!(citation_in_spans("no ranges at all", &spans()).is_ok());
    }
}
