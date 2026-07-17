//! Shared entry point for every platform. `run()` is called by the desktop
//! `main.rs` shim and, on mobile, by the platform shell via
//! `tauri::mobile_entry_point`. Everything desktop-specific — menus, extra
//! windows, the config-dir dictionary — lives in the `desktop` module, compiled
//! only on desktop.

use cha_core::dictionary;
use cha_core::pattern;
use tauri::Manager;

#[cfg(desktop)]
mod desktop;

/// The word list is embedded directly into the binary when `words.txt` is
/// present at build time, so the GUI needs no external file or Tauri resource
/// resolution and behaves identically across platforms and in dev/prod. When
/// it's absent at build time, `build.rs` leaves the `words_embedded` cfg unset
/// and we fall back to the directory-only path — but a mobile build hard-errors
/// in `build.rs` instead, since mobile has no directory to fall back to.
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

/// Whether this is a mobile build. `cfg!` is an *expression*, so a single
/// command serves both platforms — the answer is decided at compile time, not
/// sniffed from a user-agent (iPadOS WKWebView reports ambiguously) or inferred
/// from touch capability (a touch laptop is still desktop). The front end uses
/// this to show the mobile help affordance and skip the desktop Ctrl+N handler.
#[tauri::command]
fn is_mobile() -> bool {
    cfg!(mobile)
}

/// Build the dictionary by starting from the embedded list (when present at
/// build time) and, on desktop, adding every file in the user's dictionary
/// directory, deduplicating across all sources. Mobile has no config dir, no
/// file manager to reveal it in, and no way to put a file there, so the embedded
/// list is the whole story — which `build.rs` guarantees is non-empty by
/// hard-erroring on a mobile target when `words.txt` is absent.
fn load_dict(app: &tauri::AppHandle) -> Dict {
    let mut builder = dictionary::WordListBuilder::new();
    if let Some(text) = EMBEDDED_WORDS {
        builder.begin_source("Built-in");
        builder.add_str(text);
    }

    #[cfg(desktop)]
    let dir = desktop::add_user_lists(app, &mut builder);
    #[cfg(mobile)]
    let _ = app;

    let lists = builder.finish_grouped();
    if !lists.is_empty() {
        return Dict { lists, error: None };
    }

    #[cfg(desktop)]
    let error = Some(desktop::no_words_message(dir.as_deref()));
    // Unreachable in practice: `build.rs` fails a mobile build with no embedded
    // words. Say something true if it somehow happens anyway.
    #[cfg(mobile)]
    let error =
        Some("No word list found. This build of Cha shipped without its dictionary.".to_string());
    Dict { lists, error }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    // The menu bar is a desktop concept; mobile has no menu, no multiwindow, and
    // no file manager, so New Window / Open Dictionary Folder / Pattern Syntax
    // have no home there (the front end surfaces the pattern help another way).
    #[cfg(desktop)]
    let builder = builder
        .menu(desktop::build_menu)
        .on_menu_event(desktop::on_menu_event);

    builder
        .setup(|app| {
            let dict = load_dict(app.handle());
            app.manage(dict);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            search,
            dict_status,
            is_mobile,
            #[cfg(desktop)]
            desktop::open_dict_dir
        ])
        .run(tauri::generate_context!())
        .expect("error while running Cha");
}
