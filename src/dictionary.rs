use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead};

pub fn load_words(path: &str) -> io::Result<Vec<String>> {
    let file = File::open(path)?;
    let reader = io::BufReader::new(file);
    let mut seen = HashSet::new();
    let mut words = Vec::new();
    for line in reader.lines() {
        let word = line?.trim().to_lowercase();
        if !word.is_empty() && seen.insert(word.clone()) {
            words.push(word);
        }
    }
    Ok(words)
}
