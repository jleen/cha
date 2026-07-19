//! Running a compiled pattern across the loaded word lists.
//!
//! This is the whole query path, shared by every front end: the Tauri app's
//! `search` command and the web server's `/api/search` handler are both thin
//! wrappers over [`search`]. Keeping it here rather than in a front end means
//! the wire types have exactly one definition, so a change can't drift between
//! the two transports.
//!
//! Everything a caller might want to bound lives in [`SearchLimits`], because
//! the two callers want very different values: a desktop app is protecting a
//! DOM from too many rows, while a server is protecting a network from too many
//! bytes and its own CPU from a slow scan.

use std::time::Instant;

use crate::dictionary::NamedWordList;
use crate::pattern::{self, CompileLimits, PatternError};

/// One matching word, plus the anagram detail described in `MatchInfo`.
#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct MatchRow {
    pub word: String,
    /// Pool letters not used by the word, e.g. "D". Empty if none.
    pub unused: String,
    /// Word letters not in the pool, e.g. "HT". Empty if none.
    pub extra: String,
}

/// The matches from a single word list, kept together so a front end can show
/// them under a header naming the list. Only lists with at least one match get a
/// group.
#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct MatchGroup {
    pub list: String,
    pub matches: Vec<MatchRow>,
}

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SearchResult {
    pub groups: Vec<MatchGroup>,
    /// Every match, counted truthfully even past `max_results`, so a front end
    /// can say "showing first N of M" honestly.
    pub total: usize,
    /// Number of word lists loaded (not just matched). The front end suppresses
    /// per-list headers when this is 1, so a single-list setup looks unchanged.
    pub list_count: usize,
    /// A gentle note for a contentless pattern (e.g. a bare `;`) — shown in the
    /// normal status style, never as a red error. Absent for ordinary patterns.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub note: Option<String>,
}

/// How much work one search may cost. See [`CompileLimits`] for the compile-time
/// half; this adds the two bounds that only apply once a matcher exists.
pub struct SearchLimits {
    pub compile: CompileLimits,
    /// Maximum rows materialized across *all* groups combined. `total` is still
    /// counted past this, so truncation is reported rather than hidden.
    pub max_results: usize,
    /// When set, the scan gives up past this instant. Checked between chunks,
    /// never per word — see the note in [`search`].
    pub deadline: Option<Instant>,
}

impl SearchLimits {
    /// A local app: the cap protects the DOM from a pattern like `*` matching
    /// the whole list. There is no deadline — a local user who types something
    /// slow can wait for it, or close the window.
    pub fn interactive() -> Self {
        Self {
            compile: CompileLimits::default(),
            max_results: 5_000,
            deadline: None,
        }
    }
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self::interactive()
    }
}

/// Why a search could not complete.
#[derive(Debug)]
pub enum SearchError {
    /// The pattern itself was rejected — a syntax error, or a work limit.
    Pattern(PatternError),
    /// The scan ran past `SearchLimits::deadline`.
    Timeout,
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            SearchError::Pattern(e) => write!(f, "{e}"),
            SearchError::Timeout => write!(f, "Search took too long and was stopped"),
        }
    }
}

impl std::error::Error for SearchError {}

impl From<PatternError> for SearchError {
    fn from(e: PatternError) -> Self {
        SearchError::Pattern(e)
    }
}

/// How many words to scan between deadline checks.
///
/// The deadline must *not* be checked per word. `Instant::now()` is a syscall-ish
/// read on every platform, and the per-word loop is the hot path the whole crate
/// is tuned around — the same reasoning that keeps `HashMap` and `to_lowercase`
/// out of it. At this size a full 270k-word list costs ~66 clock reads, while
/// still bounding overshoot to well under a millisecond.
const DEADLINE_CHECK_INTERVAL: usize = 4096;

/// Run `pattern` across every list in order, grouping matches by their source.
///
/// An empty pattern is not an error: it yields an empty result, because a front
/// end calls this on every keystroke and the field starts empty.
pub fn search(
    lists: &[NamedWordList],
    pattern: &str,
    limits: &SearchLimits,
) -> Result<SearchResult, SearchError> {
    let pattern = pattern.trim();
    let list_count = lists.len();
    if pattern.is_empty() {
        return Ok(SearchResult {
            groups: vec![],
            total: 0,
            list_count,
            note: None,
        });
    }

    let compiled = pattern::compile_pattern_checked_with(pattern, &limits.compile)?;

    // A contentless pattern's matcher matches nothing, so the scan below is a
    // no-op; the note carries through for the front end to display gently.
    let mut total = 0usize;
    let mut groups = Vec::new();
    for list in lists {
        let mut matches = Vec::new();
        for chunk in list.words.chunks(DEADLINE_CHECK_INTERVAL) {
            if let Some(deadline) = limits.deadline {
                if Instant::now() > deadline {
                    return Err(SearchError::Timeout);
                }
            }
            for word in chunk {
                if let Some(info) = (compiled.matcher)(word) {
                    total += 1;
                    if total <= limits.max_results {
                        matches.push(MatchRow {
                            word: word.clone(),
                            unused: info.unused,
                            extra: info.extra,
                        });
                    }
                }
            }
        }
        if !matches.is_empty() {
            groups.push(MatchGroup {
                list: list.name.clone(),
                matches,
            });
        }
    }

    Ok(SearchResult {
        groups,
        total,
        list_count,
        note: compiled.note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lists() -> Vec<NamedWordList> {
        vec![
            NamedWordList {
                name: "First".to_string(),
                words: vec!["cat".into(), "cot".into(), "dog".into()],
            },
            NamedWordList {
                name: "Second".to_string(),
                words: vec!["cut".into(), "bird".into()],
            },
        ]
    }

    #[test]
    fn groups_matches_by_source_list() {
        let r = search(&lists(), "c.t", &SearchLimits::default()).unwrap();
        assert_eq!(r.total, 3);
        assert_eq!(r.list_count, 2);
        assert_eq!(r.groups.len(), 2);
        assert_eq!(r.groups[0].list, "First");
        assert_eq!(r.groups[0].matches.len(), 2); // cat, cot
        assert_eq!(r.groups[1].list, "Second");
        assert_eq!(r.groups[1].matches.len(), 1); // cut
    }

    #[test]
    fn lists_without_matches_get_no_group() {
        let r = search(&lists(), "dog", &SearchLimits::default()).unwrap();
        assert_eq!(r.groups.len(), 1);
        assert_eq!(r.groups[0].list, "First");
    }

    #[test]
    fn empty_pattern_is_empty_result_not_an_error() {
        let r = search(&lists(), "   ", &SearchLimits::default()).unwrap();
        assert_eq!(r.total, 0);
        assert!(r.groups.is_empty());
        // list_count is still reported, so the front end's header logic works.
        assert_eq!(r.list_count, 2);
    }

    #[test]
    fn max_results_caps_rows_across_all_groups_but_not_total() {
        let limits = SearchLimits {
            max_results: 2,
            ..SearchLimits::default()
        };
        let r = search(&lists(), "*", &limits).unwrap();
        let shown: usize = r.groups.iter().map(|g| g.matches.len()).sum();
        assert_eq!(shown, 2, "rows must be capped across groups, not per group");
        assert_eq!(r.total, 5, "total must stay truthful past the cap");
    }

    #[test]
    fn contentless_pattern_carries_a_note_and_no_rows() {
        let r = search(&lists(), ";", &SearchLimits::default()).unwrap();
        assert!(r.note.is_some());
        assert!(r.groups.is_empty());
    }

    #[test]
    fn syntax_error_is_a_pattern_error() {
        let e = search(&lists(), "c[at", &SearchLimits::default()).unwrap_err();
        assert!(matches!(e, SearchError::Pattern(_)));
    }

    #[test]
    fn compile_limits_are_honored() {
        let limits = SearchLimits {
            compile: CompileLimits {
                max_pattern_len: 4,
                ..CompileLimits::default()
            },
            ..SearchLimits::default()
        };
        assert!(search(&lists(), "abcdefgh", &limits).is_err());
    }

    #[test]
    fn expired_deadline_reports_timeout() {
        let limits = SearchLimits {
            deadline: Some(Instant::now() - std::time::Duration::from_secs(1)),
            ..SearchLimits::default()
        };
        assert!(matches!(
            search(&lists(), "*", &limits).unwrap_err(),
            SearchError::Timeout
        ));
    }
}
