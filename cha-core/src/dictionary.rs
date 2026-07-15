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

/// Accumulates a word list from one or more sources (in-memory strings and/or
/// files), trimming, lowercasing, and deduplicating *across* all of them while
/// preserving first-seen order. The GUI uses this to merge the embedded list
/// with any number of files the user drops in their dictionary directory.
#[derive(Default)]
pub struct WordListBuilder {
    seen: HashSet<String>,
    words: Vec<String>,
}

impl WordListBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add every word from an in-memory string (e.g. the embedded list).
    pub fn add_str(&mut self, text: &str) {
        for line in text.lines() {
            add_word(line, &mut self.seen, &mut self.words);
        }
    }

    /// Add every word from a file, streaming one line at a time so a large list
    /// is never resident in memory in addition to the growing word vector.
    pub fn add_file(&mut self, path: &Path) -> io::Result<()> {
        let file = File::open(path)?;
        let reader = io::BufReader::new(file);
        for line in reader.lines() {
            add_word(&line?, &mut self.seen, &mut self.words);
        }
        Ok(())
    }

    /// Consume the builder and return the deduplicated word list.
    pub fn finish(self) -> Vec<String> {
        self.words
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
}
