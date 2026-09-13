use std::borrow::Cow;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;

use crate::fold::fold;

/// One dictionary entry: what to show, and what to match against.
///
/// The two differ only when canonicalizing changes something beyond case — an
/// accent, a stroked letter, a ligature. That is 0.8% of a large real word list
/// and 0% of the shipped `words.txt`, so `folded` is `None` for almost every
/// entry and the second buffer is paid for only where it buys something.
///
/// **`text` is lowercased but keeps its accents.** Lowercasing is what the
/// loader has always done; keeping the accent is the point of the exercise, so
/// a search for `elan` returns `élan` rather than flattening it. Keeping the
/// *case* as well was measured and rejected: 99.9% of a 6M-entry title list
/// differs from its lowercase form, so it would mean a second string for
/// essentially every word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// The form the matcher sees. A bare `Box<str>` so [`Word::folded`] is a
    /// pointer deref with no arithmetic: it is called once per word per query,
    /// and the scan's reject path is short enough that a bounds-checked slice
    /// shows up in the perf suite.
    folded: Box<str>,
    /// The form to show, when it differs from `folded`. `None` for the 99.2% of
    /// entries that are already canonical, so the second allocation is paid for
    /// only where it buys an accent.
    display: Option<Box<str>>,
}

impl Word {
    /// Build from an already-trimmed, non-empty line.
    fn new(line: &str) -> Self {
        let text = line.to_lowercase();
        match fold(&text) {
            // `fold` borrows when the text is already canonical, which is the
            // common case and the one worth not paying for.
            Cow::Borrowed(_) => Word {
                folded: text.into_boxed_str(),
                display: None,
            },
            Cow::Owned(folded) => Word {
                folded: folded.into_boxed_str(),
                display: Some(text.into_boxed_str()),
            },
        }
    }

    /// What to display: lowercased, accents intact.
    #[inline]
    pub fn text(&self) -> &str {
        self.display.as_deref().unwrap_or(&self.folded)
    }

    /// What to match against. See [`fold`](crate::fold::fold).
    #[inline]
    pub fn folded(&self) -> &str {
        &self.folded
    }
}

impl From<&str> for Word {
    /// Build a single entry, applying the same trim-and-canonicalize the loader
    /// does. Handy for callers assembling a list in memory, and for tests.
    fn from(line: &str) -> Self {
        Word::new(line.trim())
    }
}

/// Shared per-line logic: trim, canonicalize, and append if non-empty and unseen.
/// Handles one line at a time so callers can stream without holding whole files.
///
/// **Dedup is keyed on the display form, not the folded one.** `elan` and `élan`
/// are different entries and a search matching one should return both; collapsing
/// them would canonicalize and then discard, which is the opposite of the point.
fn add_word(line: &str, seen: &mut HashSet<String>, words: &mut Vec<Word>) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let word = Word::new(trimmed);
    if seen.insert(word.text().to_string()) {
        words.push(word);
    }
}

/// A named word list: the words that were first seen while a given source was
/// active. The GUI uses these to label each source (built-in, or a file in the
/// dictionary directory) and show which list a match came from.
pub struct NamedWordList {
    pub name: String,
    pub words: Vec<Word>,
}

/// Append every regular, non-hidden file in `dir` to `builder`, each as its own
/// named source, in sorted filename order for determinism.
///
/// Shared by the desktop app (which reads the user's config-dir `dictionaries/`
/// folder) and the web server (`--dict-dir`), so the policy below is decided
/// once rather than forked per backend:
///
/// - **Hidden files are skipped.** macOS drops a binary `.DS_Store` into any
///   folder a user opens in Finder; reading it would inject its contents as
///   "words".
/// - **Subdirectories are skipped** rather than walked — a flat folder keeps the
///   list names predictable.
/// - **An unreadable file is warned about and skipped**, not fatal. One bad file
///   shouldn't cost the user every other list.
///
/// Errors go to stderr rather than being returned: every caller's policy is to
/// continue, and a server logs them while a desktop app has nowhere to show them.
pub fn load_dir(dir: &Path, builder: &mut WordListBuilder) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("Cha: could not read {}: {e}", dir.display());
            return;
        }
    };
    let mut paths: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && !is_hidden(p))
        .collect();
    paths.sort();
    for path in &paths {
        builder.begin_source(list_name(path));
        if let Err(e) = builder.add_file(path) {
            eprintln!("Cha: could not read {}: {e}", path.display());
        }
    }
}

/// A friendly display name for a dictionary file: its file name with the
/// extension stripped (`scrabble.txt` -> `scrabble`). Falls back to the full
/// file name, then to the whole path, if the stem can't be extracted.
fn list_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

/// Whether a path's file name begins with a dot (a dotfile on Unix; also filters
/// macOS metadata files like `.DS_Store` regardless of platform).
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

/// Accumulates a word list from one or more sources (in-memory strings and/or
/// files), trimming, lowercasing, and deduplicating *across* all of them while
/// preserving first-seen order. The GUI uses this to merge the embedded list
/// with any number of files the user drops in their dictionary directory.
///
/// Words are grouped by *source*: each `begin_source` call starts a new named
/// group, and subsequent `add_*` calls append to it. Dedup is still global
/// (first-seen wins), so a word appears only under the first source that
/// contained it. Callers that don't care about grouping never call
/// `begin_source`; their words land in a single default group and `finish`
/// returns them flat, exactly as before.
pub struct WordListBuilder {
    seen: HashSet<String>,
    sources: Vec<NamedWordList>,
}

impl Default for WordListBuilder {
    fn default() -> Self {
        // Start with one anonymous group so `add_*` works before any
        // `begin_source` call (the flat, single-source path the CLI uses).
        Self {
            seen: HashSet::new(),
            sources: vec![NamedWordList {
                name: String::new(),
                words: Vec::new(),
            }],
        }
    }
}

impl WordListBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a new named source. Subsequent `add_str`/`add_file` calls append to
    /// it, and `finish_grouped` reports it (with its first-seen words) under this
    /// name. Dedup remains global across every source.
    pub fn begin_source(&mut self, name: impl Into<String>) {
        self.sources.push(NamedWordList {
            name: name.into(),
            words: Vec::new(),
        });
    }

    /// Add every word from an in-memory string (e.g. the embedded list). Words
    /// land in the current source; there is always at least one, so `unwrap`
    /// never panics.
    pub fn add_str(&mut self, text: &str) {
        let words = &mut self.sources.last_mut().unwrap().words;
        for line in text.lines() {
            add_word(line, &mut self.seen, words);
        }
    }

    /// Add every word from a file, streaming one line at a time so a large list
    /// is never resident in memory in addition to the growing word vector.
    pub fn add_file(&mut self, path: &Path) -> io::Result<()> {
        let file = File::open(path)?;
        let reader = io::BufReader::new(file);
        let words = &mut self.sources.last_mut().unwrap().words;
        for line in reader.lines() {
            add_word(&line?, &mut self.seen, words);
        }
        Ok(())
    }

    /// Consume the builder and return the deduplicated word list, flat: every
    /// source's words concatenated in order. Dedup was already global, so this
    /// matches the pre-grouping behavior.
    pub fn finish(self) -> Vec<Word> {
        self.sources.into_iter().flat_map(|s| s.words).collect()
    }

    /// Consume the builder and return the named sources in order, dropping any
    /// that ended up empty (e.g. a list whose every word was already seen in an
    /// earlier source).
    pub fn finish_grouped(self) -> Vec<NamedWordList> {
        self.sources
            .into_iter()
            .filter(|s| !s.words.is_empty())
            .collect()
    }
}

/// Load a word list from a file, streaming one line at a time. Peak memory is
/// ~1x the final word list, which matters for very large alternate lists.
pub fn load_words(path: &str) -> io::Result<Vec<Word>> {
    let mut builder = WordListBuilder::new();
    builder.add_file(Path::new(path))?;
    Ok(builder.finish())
}

/// Load a word list from an in-memory string. Used for the GUI's word list,
/// which is embedded in the binary via `include_str!` and already fully resident.
pub fn load_words_from_str(text: &str) -> Vec<Word> {
    let mut builder = WordListBuilder::new();
    builder.add_str(text);
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// The display forms, for comparing against a literal list.
    fn texts(words: &[Word]) -> Vec<&str> {
        words.iter().map(Word::text).collect()
    }

    #[test]
    fn accents_survive_into_the_display_form() {
        // The point of the exercise: match canonically, hand back the original.
        let w = Word::from("\u{c9}lan");
        assert_eq!(w.text(), "\u{e9}lan"); // lowercased, accent intact
        assert_eq!(w.folded(), "elan"); // what the matcher sees
    }

    #[test]
    fn canonical_words_carry_no_second_buffer() {
        // 99.2% of a large real list takes this path, and all of the shipped one.
        // If it ever starts allocating, memory doubles for no benefit.
        let w = Word::from("cat");
        assert_eq!(w.text(), "cat");
        assert_eq!(w.folded(), "cat");
        assert!(
            w.display.is_none(),
            "an ASCII word should store one form, not two"
        );
        // And the accented case genuinely needs the second one.
        assert!(Word::from("\u{e9}lan").display.is_some());
    }

    #[test]
    fn dedup_is_keyed_on_the_display_form() {
        // `elan` and `élan` fold together but are different words, and a search
        // matching one should return both. Collapsing them would canonicalize and
        // then discard, which is the opposite of the point.
        let mut b = WordListBuilder::new();
        b.add_str("elan\n\u{e9}lan\n\u{c9}LAN\nElan\n");
        // Four lines, two distinct display forms: the two ASCII spellings collapse
        // by case as they always did, and the two accented ones likewise.
        assert_eq!(texts(&b.finish()), vec!["elan", "\u{e9}lan"]);
    }

    #[test]
    fn multigraph_entries_fold_for_matching_and_keep_their_spelling() {
        let w = Word::from("\u{c6}r\u{f8}");
        assert_eq!(w.text(), "\u{e6}r\u{f8}");
        assert_eq!(w.folded(), "aero");
    }

    #[test]
    fn dedups_and_normalizes_across_sources() {
        // The GUI concatenates the embedded list with any number of directory
        // files; dedup, trimming, and lowercasing must span all sources and
        // preserve first-seen order.
        let mut path = std::env::temp_dir();
        path.push(format!("cha-dict-test-{}.txt", std::process::id()));
        {
            let mut f = File::create(&path).unwrap();
            // "Apple" is a case-insensitive dup of the embedded "apple";
            // "cherry" is new; blank/whitespace lines are ignored.
            writeln!(f, "  Apple \n\ncherry\n").unwrap();
        }

        let mut builder = WordListBuilder::new();
        builder.add_str("apple\nBANANA\napple\n");
        builder.add_file(&path).unwrap();
        let words = builder.finish();

        std::fs::remove_file(&path).unwrap();
        assert_eq!(texts(&words), vec!["apple", "banana", "cherry"]);
    }

    #[test]
    fn groups_by_source_with_global_dedup() {
        // A word shared across sources appears only under the first source
        // (first-seen wins), and a source whose every word was already seen is
        // dropped entirely.
        let mut builder = WordListBuilder::new();
        builder.begin_source("Built-in");
        builder.add_str("apple\nbanana\n");
        builder.begin_source("extra");
        builder.add_str("Apple\ncherry\n"); // "Apple" dups built-in; "cherry" new
        builder.begin_source("all-dupes");
        builder.add_str("banana\nCHERRY\n"); // both already seen -> dropped

        let groups = builder.finish_grouped();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Built-in");
        assert_eq!(texts(&groups[0].words), vec!["apple", "banana"]);
        assert_eq!(groups[1].name, "extra");
        assert_eq!(texts(&groups[1].words), vec!["cherry"]);
    }
}
