use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;

/// Shared per-line logic: trim, lowercase, and append if non-empty and unseen.
/// Handles one line at a time so callers can stream without holding whole files.
fn add_word(line: &str, seen: &mut HashSet<String>, words: &mut Vec<String>) {
    let word = line.trim().to_lowercase();
    if !word.is_empty() && seen.insert(word.clone()) {
        words.push(word);
    }
}

/// A named word list: the words that were first seen while a given source was
/// active. The GUI uses these to label each source (built-in, or a file in the
/// dictionary directory) and show which list a match came from.
pub struct NamedWordList {
    pub name: String,
    pub words: Vec<String>,
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
    pub fn finish(self) -> Vec<String> {
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
pub fn load_words(path: &str) -> io::Result<Vec<String>> {
    let mut builder = WordListBuilder::new();
    builder.add_file(Path::new(path))?;
    Ok(builder.finish())
}

/// Load a word list from an in-memory string. Used for the GUI's word list,
/// which is embedded in the binary via `include_str!` and already fully resident.
pub fn load_words_from_str(text: &str) -> Vec<String> {
    let mut builder = WordListBuilder::new();
    builder.add_str(text);
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
        assert_eq!(words, vec!["apple", "banana", "cherry"]);
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
        assert_eq!(groups[0].words, vec!["apple", "banana"]);
        assert_eq!(groups[1].name, "extra");
        assert_eq!(groups[1].words, vec!["cherry"]);
    }
}
