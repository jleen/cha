// Prevent a console window from opening alongside the GUI on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicUsize, Ordering};

use cha_core::dictionary;
use cha_core::pattern;
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

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
struct MatchRow {
    word: String,
    /// Pool letters not used by the word, e.g. "D". Empty if none.
    unused: String,
    /// Word letters not in the pool, e.g. "HT". Empty if none.
    extra: String,
}

#[derive(serde::Serialize)]
struct SearchResult {
    matches: Vec<MatchRow>,
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
        if let Some(info) = matcher(word) {
            total += 1;
            if matches.len() < MAX_RESULTS {
                matches.push(MatchRow {
                    word: word.clone(),
                    unused: info.unused,
                    extra: info.extra,
                });
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

/// Monotonic counter for minting unique labels for additional search windows.
/// The first window uses the config-defined `main` label; subsequent ones are
/// `main-2`, `main-3`, … Labels must be unique for the lifetime of the app, so a
/// plain counter (never reused, even after a window closes) is the simplest
/// correct choice.
static WINDOW_COUNTER: AtomicUsize = AtomicUsize::new(1);

/// Open another search window running the same UI as the primary one. Each
/// window is independent but shares the app-global `Dict`, so `search` and
/// `dict_status` work in every window without re-loading the word list.
fn open_search_window(app: &tauri::AppHandle) {
    let n = WINDOW_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    let label = format!("main-{n}");
    if let Err(e) = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
        .title("Cha")
        .inner_size(720.0, 640.0)
        .build()
    {
        eprintln!("Cha: could not open a new window: {e}");
    }
}

/// Open (or focus) the singleton Pattern Syntax cheat-sheet window. The page is
/// static HTML, so it needs no Tauri commands.
fn open_pattern_syntax_window(app: &tauri::AppHandle) {
    if let Some(existing) = app.get_webview_window("pattern-syntax") {
        let _ = existing.set_focus();
        return;
    }
    if let Err(e) = WebviewWindowBuilder::new(
        app,
        "pattern-syntax",
        WebviewUrl::App("pattern-syntax.html".into()),
    )
    .title("Pattern Syntax")
    .inner_size(560.0, 680.0)
    .resizable(true)
    .build()
    {
        eprintln!("Cha: could not open the Pattern Syntax window: {e}");
    }
}

/// Build the full standard application menu (App / File / Edit / Window / Help).
/// Standard items are `PredefinedMenuItem`s that Tauri labels and localizes; the
/// only custom items are "New Window" and "Pattern Syntax". macOS-only items are
/// ignored on Windows/Linux, where the menu renders inside each window.
fn build_menu(app: &tauri::App) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let app_menu = SubmenuBuilder::new(app, "Cha")
        .about(None)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let new_window = MenuItemBuilder::new("New Window")
        .id("new_window")
        .accelerator("CmdOrCtrl+N")
        .build(app)?;
    let file_menu = SubmenuBuilder::new(app, "File")
        .item(&new_window)
        .separator()
        .item(&PredefinedMenuItem::close_window(app, None)?)
        .build()?;

    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let window_menu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .build()?;

    let pattern_syntax = MenuItemBuilder::new("Pattern Syntax")
        .id("pattern_syntax")
        .build(app)?;
    let help_menu = SubmenuBuilder::new(app, "Help")
        .item(&pattern_syntax)
        .build()?;

    MenuBuilder::new(app)
        .items(&[&app_menu, &file_menu, &edit_menu, &window_menu, &help_menu])
        .build()
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
            let menu = build_menu(app)?;
            app.set_menu(menu)?;
            Ok(())
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "new_window" => open_search_window(app),
            "pattern_syntax" => open_pattern_syntax_window(app),
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![search, dict_status])
        .run(tauri::generate_context!())
        .expect("error while running Cha");
}
