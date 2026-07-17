//! Desktop-only surfaces: the menu bar, additional search windows, the Pattern
//! Syntax window, and the user's dictionary directory (with the file-manager
//! shell-out that reveals it). Mobile has none of these — no menu, no
//! multiwindow, no config dir, no file manager — so the whole lot lives behind
//! this one `#[cfg(desktop)] mod desktop;` boundary. Nothing here can be dead
//! code on mobile because the module simply isn't compiled there.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use cha_core::dictionary::WordListBuilder;
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// Open the user's dictionary directory in the platform file manager. Invoked
/// from the front end's "Open Dictionary Folder" notice button, and (via
/// `open_dict_dir_impl`) from the File menu. The directory is created first so
/// there is always something to reveal, even if it was deleted after startup.
#[tauri::command]
pub fn open_dict_dir(app: tauri::AppHandle) {
    open_dict_dir_impl(&app);
}

/// The dictionary directory: a `dictionaries` subfolder of the app config dir
/// (e.g. `~/Library/Application Support/org.saturnvalley.cha/dictionaries` on
/// macOS). Returns `None` only when the config dir can't be located at all.
fn dict_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
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
fn open_folder(path: &Path) {
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

/// Extend `builder` with every file in the user's `dictionaries/` folder,
/// creating the folder on startup so there's an obvious, openable place to add
/// lists. Returns the folder path (for the empty-dictionary notice to name), or
/// `None` when the app config dir can't be located at all.
///
/// This is intentionally additive: the directory supplements the embedded list
/// rather than replacing it, deduplicating across both via the shared
/// `WordListBuilder`. Files added while the app is running are picked up on the
/// next launch, so the notice says to reopen Cha.
pub fn add_user_lists(app: &tauri::AppHandle, builder: &mut WordListBuilder) -> Option<PathBuf> {
    let dir = dict_dir(app)?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("Cha: could not create {}: {e}", dir.display());
    } else {
        load_dir_files(&dir, builder);
    }
    Some(dir)
}

/// The user-facing message shown when no word list could be loaded at all,
/// naming the exact path a dictionary file should live at. `dir` is the
/// dictionary folder when it could be located, else `None`.
pub fn no_words_message(dir: Option<&Path>) -> String {
    match dir {
        Some(dir) => format!(
            "No word list found.\n\nAdd one or more dictionary files (plain \
             text, one word per line) to this folder, then reopen Cha:\n{}",
            dir.display()
        ),
        None => "No word list found, and the app config directory could not \
                 be located on this system."
            .to_string(),
    }
}

/// Concatenate every regular, non-hidden file in `dir` into `builder`, in sorted
/// filename order for determinism. Subdirectories and hidden files (e.g. macOS
/// `.DS_Store`, whose binary contents would inject junk "words") are skipped, as
/// are individually unreadable files — a bad file shouldn't sink the whole load.
fn load_dir_files(dir: &Path, builder: &mut WordListBuilder) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("Cha: could not read {}: {e}", dir.display());
            return;
        }
    };
    let mut paths: Vec<PathBuf> = entries
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

/// Dispatch a menu selection to its handler. Runs on the event-loop (main)
/// thread, which is required: `WebviewWindowBuilder::build()` off that thread
/// half-creates a blank window and deadlocks on Windows.
pub fn on_menu_event(app: &tauri::AppHandle, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        "new_window" => open_search_window(app),
        "open_dict_dir" => open_dict_dir_impl(app),
        "pattern_syntax" => open_pattern_syntax_window(app),
        _ => {}
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
pub fn build_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
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
