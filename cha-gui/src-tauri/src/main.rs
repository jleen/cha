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
    /// The loaded word lists, in display order (built-in first, then directory
    /// files in sorted-name order). Each carries its friendly name so search can
    /// tell the user which list a match came from. Dedup is global across lists
    /// (first-seen wins), so a word appears under only one list.
    lists: Vec<dictionary::NamedWordList>,
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

/// The matches from a single word list, kept together so the front end can show
/// them under a header naming the list. Only lists with at least one match get a
/// group.
#[derive(serde::Serialize)]
struct MatchGroup {
    list: String,
    matches: Vec<MatchRow>,
}

#[derive(serde::Serialize)]
struct SearchResult {
    groups: Vec<MatchGroup>,
    total: usize,
    /// Number of word lists loaded (not just matched). The front end suppresses
    /// per-list headers when this is 1, so a single-list setup looks unchanged.
    list_count: usize,
    /// A gentle note for a contentless pattern (e.g. a bare `;`) — shown in the
    /// normal status style, never as a red error. Absent for ordinary patterns.
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

// `(async)` runs this on Tauri's worker thread pool rather than the main thread.
// Scanning the full word list (~270k entries) takes long enough that running it
// on the main thread would freeze the window — no typing, no repaint — until it
// returned. `Dict` is `Send + Sync`, so `State` access off-thread is safe. The
// body stays synchronous; there are no await points.
#[tauri::command(async)]
fn search(pattern: String, dict: tauri::State<Dict>) -> Result<SearchResult, String> {
    let pattern = pattern.trim();
    let list_count = dict.lists.len();
    if pattern.is_empty() {
        return Ok(SearchResult {
            groups: vec![],
            total: 0,
            list_count,
            note: None,
        });
    }
    let compiled = pattern::compile_pattern_checked(pattern).map_err(|e| e.to_string())?;
    // A contentless pattern's matcher matches nothing, so the scan is a no-op;
    // the note carries through for the front end to display gently.
    //
    // Scan each list in order, keeping its matches together. `total` counts every
    // match truthfully, but only the first `MAX_RESULTS` rows (across all lists)
    // are materialized so a pattern like `*` can't flood the DOM.
    let mut total = 0usize;
    let mut groups = Vec::new();
    for list in &dict.lists {
        let mut matches = Vec::new();
        for word in &list.words {
            if let Some(info) = (compiled.matcher)(word) {
                total += 1;
                if total <= MAX_RESULTS {
                    matches.push(MatchRow {
                        word: word.clone(),
                        unused: info.unused,
                        extra: info.extra,
                    });
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

/// Returns a user-facing message when no word list could be loaded, else `None`.
/// The front end calls this on startup to show a notice naming the expected path.
#[tauri::command]
fn dict_status(dict: tauri::State<Dict>) -> Option<String> {
    dict.error.clone()
}

/// Open the user's dictionary directory in the platform file manager. Invoked
/// from the front end's "Open Dictionary Folder" notice button, and (via
/// `open_dict_dir_impl`) from the File menu. The directory is created first so
/// there is always something to reveal, even if it was deleted after startup.
#[tauri::command]
fn open_dict_dir(app: tauri::AppHandle) {
    open_dict_dir_impl(&app);
}

/// The dictionary directory: a `dictionaries` subfolder of the app config dir
/// (e.g. `~/Library/Application Support/org.saturnvalley.cha/dictionaries` on
/// macOS). Returns `None` only when the config dir can't be located at all.
fn dict_dir(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    match app.path().app_config_dir() {
        Ok(dir) => Some(dir.join("dictionaries")),
        Err(e) => {
            eprintln!("Cha: no config dir available ({e})");
            None
        }
    }
}

/// Ensure the dictionary directory exists, then reveal it in the file manager.
fn open_dict_dir_impl(app: &tauri::AppHandle) {
    let Some(dir) = dict_dir(app) else { return };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("Cha: could not create {}: {e}", dir.display());
        return;
    }
    open_folder(&dir);
}

/// Open a folder in the platform's file manager (Finder / Explorer / the
/// freedesktop default via `xdg-open`). Spawning a subprocess is safe off the
/// event-loop thread, unlike window creation, so this works from either a
/// command handler or a menu callback. We don't wait on the child.
fn open_folder(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let program = "xdg-open";

    if let Err(e) = std::process::Command::new(program).arg(path).spawn() {
        eprintln!("Cha: could not open {}: {e}", path.display());
    }
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
/// only custom items are "New Window" and "Pattern Syntax".
///
/// The macOS "app menu" (About/Services/Hide/Quit) is macOS-only and is omitted
/// on Windows/Linux, where it isn't idiomatic; there, Quit lives under File. The
/// menu is wired in via `Builder::menu` (not `App::set_menu`) so accelerators
/// like Ctrl+N register on the initial window's accelerator table on Windows.
fn build_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let menu = MenuBuilder::new(app);

    #[cfg(target_os = "macos")]
    let menu = {
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
        menu.item(&app_menu)
    };

    let new_window = MenuItemBuilder::new("New Window")
        .id("new_window")
        .accelerator("CmdOrCtrl+N")
        .build(app)?;
    let open_dict = MenuItemBuilder::new("Open Dictionary Folder")
        .id("open_dict_dir")
        .build(app)?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&new_window)
        .separator()
        .item(&open_dict)
        .separator()
        .item(&PredefinedMenuItem::close_window(app, None)?);
    // Windows/Linux have no app menu, so Quit belongs under File there.
    #[cfg(not(target_os = "macos"))]
    let file = file.separator().item(&PredefinedMenuItem::quit(app, None)?);
    let file_menu = file.build()?;

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

    menu.item(&file_menu)
        .item(&edit_menu)
        .item(&window_menu)
        .item(&help_menu)
        .build()
}

/// Build the dictionary by starting from the embedded list (when present at
/// build time) and then concatenating every file the user has placed in their
/// dictionary directory, deduplicating across all sources. The directory is
/// created on startup so there's an obvious, openable place to add lists.
///
/// This is intentionally additive: the directory supplements the embedded list
/// rather than replacing it. A word list is considered available whenever *any*
/// source yielded words; the notice (and its "Open Dictionary Folder" button)
/// appears only when there are no words at all. Files added while the app is
/// running are picked up on the next launch, so the notice says to reopen Cha.
fn load_dict(app: &tauri::AppHandle) -> Dict {
    let mut builder = dictionary::WordListBuilder::new();
    if let Some(text) = EMBEDDED_WORDS {
        builder.begin_source("Built-in");
        builder.add_str(text);
    }

    let dir = dict_dir(app);
    if let Some(dir) = &dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("Cha: could not create {}: {e}", dir.display());
        } else {
            load_dir_files(dir, &mut builder);
        }
    }

    let lists = builder.finish_grouped();
    let error = if lists.is_empty() {
        Some(match &dir {
            Some(dir) => format!(
                "No word list found.\n\nAdd one or more dictionary files (plain \
                 text, one word per line) to this folder, then reopen Cha:\n{}",
                dir.display()
            ),
            None => "No word list found, and the app config directory could not \
                     be located on this system."
                .to_string(),
        })
    } else {
        None
    };
    Dict { lists, error }
}

/// Concatenate every regular, non-hidden file in `dir` into `builder`, in sorted
/// filename order for determinism. Subdirectories and hidden files (e.g. macOS
/// `.DS_Store`, whose binary contents would inject junk "words") are skipped, as
/// are individually unreadable files — a bad file shouldn't sink the whole load.
fn load_dir_files(dir: &std::path::Path, builder: &mut dictionary::WordListBuilder) {
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
fn list_name(path: &std::path::Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

/// Whether a path's file name begins with a dot (a dotfile on Unix; also filters
/// macOS metadata files like `.DS_Store` regardless of platform).
fn is_hidden(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

fn main() {
    tauri::Builder::default()
        .menu(build_menu)
        .setup(|app| {
            let dict = load_dict(app.handle());
            app.manage(dict);
            Ok(())
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "new_window" => open_search_window(app),
            "open_dict_dir" => open_dict_dir_impl(app),
            "pattern_syntax" => open_pattern_syntax_window(app),
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![search, dict_status, open_dict_dir])
        .run(tauri::generate_context!())
        .expect("error while running Cha");
}
