use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead};

/// Shared per-line logic: trim, lowercase, and append if non-empty and unseen.
/// Handles one line at a time so callers can stream without holding whole files.
fn add_word(line: &str, seen: &mut HashSet<String>, words: &mut Vec<String>) {
    let word = line.trim().to_lowercase();
    if !word.is_empty() && seen.insert(word.clone()) {
        words.push(word);
    }
}

/// Load a word list from a file, streaming one line at a time. Peak memory is
/// ~1x the final word list, which matters for very large alternate lists.
pub fn load_words(path: &str) -> io::Result<Vec<String>> {
    let file = File::open(path)?;
    let reader = io::BufReader::new(file);
    let mut seen = HashSet::new();
    let mut words = Vec::new();
    for line in reader.lines() {
        add_word(&line?, &mut seen, &mut words);
    }
    Ok(words)
}

/// Load a word list from an in-memory string. Used for the GUI's word list,
/// which is embedded in the binary via `include_str!` and already fully resident.
pub fn load_words_from_str(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut words = Vec::new();
    for line in text.lines() {
        add_word(line, &mut seen, &mut words);
    }
    words
}
