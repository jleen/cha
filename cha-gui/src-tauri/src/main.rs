// Prevent a console window from opening alongside the GUI on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use cha_core::dictionary;
use cha_core::pattern;

/// The word list is embedded directly into the binary, so the GUI needs no
/// external file or Tauri resource resolution and behaves identically across
/// platforms and in dev/prod.
const WORDS_TXT: &str = include_str!("../../../words.txt");

/// Upper bound on results returned to the front end. A pattern like `*` matches
/// the whole list (~270k words); returning all of them would flood the DOM.
const MAX_RESULTS: usize = 5000;

struct Dict {
    words: Vec<String>,
}

#[derive(serde::Serialize)]
struct SearchResult {
    matches: Vec<String>,
    total: usize,
}

#[tauri::command]
fn search(pattern: String, dict: tauri::State<Dict>) -> Result<SearchResult, String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Ok(SearchResult { matches: vec![], total: 0 });
    }
    let matcher = pattern::compile_pattern(pattern).map_err(|e| e.to_string())?;
    let mut total = 0usize;
    let mut matches = Vec::new();
    for word in &dict.words {
        if matcher(word) {
            total += 1;
            if matches.len() < MAX_RESULTS {
                matches.push(word.clone());
            }
        }
    }
    Ok(SearchResult { matches, total })
}

fn main() {
    let words = dictionary::load_words_from_str(WORDS_TXT);
    tauri::Builder::default()
        .manage(Dict { words })
        .invoke_handler(tauri::generate_handler![search])
        .run(tauri::generate_context!())
        .expect("error while running Cha");
}
