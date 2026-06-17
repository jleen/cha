// Prevent a console window from opening alongside the GUI on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use cha_core::dictionary;
use cha_core::pattern;
use tauri::Manager;

/// The word list is embedded directly into the binary when `words.txt` is
/// present at build time, so the GUI needs no external file or Tauri resource
/// resolution and behaves identically across platforms and in dev/prod. When
/// it's absent at build time, `build.rs` leaves the `words_embedded` cfg unset
/// and we fall back to loading `words.txt` from the user's home dir at runtime.
#[cfg(words_embedded)]
const EMBEDDED_WORDS: Option<&str> = Some(include_str!("../../../words.txt"));
#[cfg(not(words_embedded))]
const EMBEDDED_WORDS: Option<&str> = None;

/// Upper bound on results returned to the front end. A pattern like `*` matches
/// the whole list (~270k words); returning all of them would flood the DOM.
const MAX_RESULTS: usize = 5000;

struct Dict {
    words: Vec<String>,
    /// A user-facing message (naming the exact path the word list should live
    /// at) when no list could be loaded, or `None` when the list is available.
    /// The front end shows this on startup so an empty result area isn't
    /// mistaken for "no matches".
    error: Option<String>,
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
        return Ok(SearchResult {
            matches: vec![],
            total: 0,
        });
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

/// Returns a user-facing message when no word list could be loaded, else `None`.
/// The front end calls this on startup to show a notice naming the expected path.
#[tauri::command]
fn dict_status(dict: tauri::State<Dict>) -> Option<String> {
    dict.error.clone()
}

/// Build the dictionary from the embedded list when present, otherwise from
/// `words.txt` in the app's config dir (e.g. `~/.config/org.saturnvalley.cha/`
/// on Linux). A missing or unreadable runtime file yields an empty dictionary
/// rather than aborting, so the GUI still opens.
fn load_dict(app: &tauri::App) -> Dict {
    if let Some(text) = EMBEDDED_WORDS {
        return Dict {
            words: dictionary::load_words_from_str(text),
            error: None,
        };
    }
    match app.path().app_config_dir() {
        Ok(dir) => {
            let path = dir.join("words.txt");
            match dictionary::load_words(&path.to_string_lossy()) {
                Ok(words) => Dict { words, error: None },
                Err(e) => {
                    eprintln!("Cha: could not read {}: {e}", path.display());
                    let msg = format!(
                        "No word list found.\n\nPlace a words.txt file here:\n{}",
                        path.display()
                    );
                    Dict {
                        words: Vec::new(),
                        error: Some(msg),
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Cha: no config dir available ({e}); starting with an empty word list");
            let msg = "No word list found, and the app config directory could \
                       not be located on this system."
                .to_string();
            Dict {
                words: Vec::new(),
                error: Some(msg),
            }
        }
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let dict = load_dict(app);
            app.manage(dict);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![search, dict_status])
        .run(tauri::generate_context!())
        .expect("error while running Cha");
}
