# Implementation notes for cha

## Building and checking

Before committing, run all three and keep them clean:

```
cargo fmt
cargo clippy --workspace
cargo test
```

`cargo clippy` is treated as required here, not advisory — there are commits
dedicated to keeping it warning-free (e.g. prefer the `?` operator over an
`if x.is_none() { return None }`). `cargo fmt` likewise: the repo is kept
fully rustfmt-formatted, so run it before committing rather than hand-aligning.
The workspace has two members (`cha-core`, `cha-gui/src-tauri`) plus the CLI
crate (`cha`) at the root; `--workspace` covers the libraries — build the GUI
explicitly with `cargo build -p cha-gui` when touching it.

**To exercise the no-word-list path**, temporarily move `words.txt` out of the
repo root and rebuild the GUI. With `words.txt` present (the usual case) the list
is embedded, so the empty-dictionary notice and its "Open Dictionary Folder"
button are unreachable and changes to them go untested. `build.rs` tracks the
path via `rerun-if-changed` whether or not it exists, so moving it away (and
back) re-evaluates the `words_embedded` cfg with no `cargo clean` needed.

## Performance requirements

`cha` searches ~270k words per query, and has ambitions to search
even larger (>10M words) lists. Matching must be fast enough to feel
instantaneous on a modern laptop. Current release-build baselines:

| Pattern type | Target | Achieved |
|---|---|---|
| Template (e.g. `qu...`) | < 10 ms | ~5 ms |
| Anagram (e.g. `;..oting`) | < 20 ms | ~8 ms |

The benchmark flag (`cha <pattern> <wordlist> <N>`) is the primary way to
measure regressions. Run with N=1000 to get stable averages.

## Non-obvious invariants

**Words are pre-lowercased.** `dictionary::load_words` lowercases every word
at load time. Matchers must not call `.to_lowercase()` on words in the hot
loop — it allocates a `String` per word and is redundant. The anagram closure
relies on this: it calls `count_str` directly on `word` without any case
conversion.

**The anagram pool is pre-computed.** `compile_anagram` builds `combo_pools`
— a `Vec<([usize; 26], usize)>` — before the closure is returned. Each entry
is the pre-summed character counter and size for `fixed_letters + one combo`.
Nothing in the per-word hot path touches a `Vec` or `HashMap` for pool
accounting.

**Character counting uses `[usize; 26]`, never `HashMap`.** Indexing by
`(byte - b'a')` is O(1) with no allocation. The `count_str` helper (bytes,
not chars) is the right function to call from the anagram closure. `count_chars`
(takes `&[char]`) is only used at pattern compile time.

**Punctuation stripping uses `Cow<str>` to avoid allocation.** In
`compile_pattern`, `test_word` borrows the original word when neither the
pattern nor the word contains punctuation — the common case for a Scrabble
wordlist. Allocation only happens when stripping is actually needed.

## Pattern compilation is fallible

`compile_pattern` returns `Result<Matcher, PatternError>`. All failure modes
(unclosed `[`/`(`, invalid regex, meaningless characters) are detected at
**compile time** — the returned matcher closures never fail. Keep it that way:
the per-word hot path must stay panic- and `Result`-free. The CLI handles the
`Err` (interactive mode re-prompts instead of crashing); the GUI maps it to a
string shown in the UI.

## Matches carry optional detail (`MatchInfo`)

`Matcher` is `Box<dyn Fn(&str) -> Option<MatchInfo>>`: `None` means no match,
`Some(info)` means a match. `MatchInfo { unused, extra }` reports, for anagram
matches, the pool letters the word leaves **unused** and the word letters
**not in the pool** (both uppercased and sorted; empty when there's nothing to
report — e.g. an exact anagram). It does **not** affect match validity; it's
purely informational. The GUI renders it as a faint `−UNUSED +EXTRA` suffix to
the right of each word ([`cha-gui/ui/main.js`](cha-gui/ui/main.js),
`.word-annot` in [`styles.css`](cha-gui/ui/styles.css)); the CLI ignores it and
just checks `.is_some()`.

**Computing the letters is on the confirmed-match path only.** The `diff_letters`
work in `compile_anagram` runs *after* all the fast reject checks pass, right
before returning `Some(..)` — never for a word that fails to match. Keep it
there: doing per-letter diffing for non-matches would regress the hot loop.
Composition (`&`/`!`) folds each matched part's `MatchInfo` into the aggregate,
but in practice only the single anagram part contributes anything.

**The CLI surfaces this behind `-d`/`--delta`** (off by default; applies to both
one-shot and interactive mode). `format_delta` renders it as `-UNUSED +EXTRA`
(ASCII, mirroring the GUI). Two rules matter:

- **Color is delegated to `anstream`/`anstyle`, not hand-rolled.** The delta is a
  `const anstyle::Style` (gray = `BrightBlack`); `render` always emits the escape
  codes, and output is written through an `anstream::AutoStream` whose
  `ColorChoice` is resolved once via `AutoStream::choice(&stdout)`. That honors tty
  detection, `NO_COLOR`, `CLICOLOR`/`CLICOLOR_FORCE`, `TERM`, and CI — and strips
  the codes itself when color isn't wanted, so piped output stays clean with no
  manual `if color` plumbing. Don't reintroduce raw `\x1b[..]` constants.
  (`anstyle`/`anstream` are already in the tree via `clap`'s `color` feature.)
- **Column width must exclude the escape codes.** `MatchItem::width()` counts only
  the visible word + delta (both ASCII, so `str::len()` == columns); the gray
  codes are zero-width. Keep that split or columns misalign. The AutoStream is
  backed by a `Vec<u8>` (not `BufWriter` — anstream's `RawStream` is sealed and
  excludes `BufWriter`, but includes `Vec<u8>`), buffered then flushed once.

## GUI (`cha-gui`, Tauri v2)

- The GUI is a separate workspace member that reuses `cha-core`. The front end
  is **vanilla HTML/JS/CSS with no bundler** — `withGlobalTauri: true` exposes
  `window.__TAURI__.core.invoke`, so there is no Node/npm step. Don't introduce
  a JS framework or build tool without a strong reason.
- **The word list is embedded via `include_str!` when `words.txt` is present at
  the repo root at build time** (the usual case). `build.rs` gates the embed
  behind a `words_embedded` cfg (it can't be a runtime `if` — `include_str!`
  expands unconditionally — which is why the decision lives in `build.rs`).
- **The dictionary is embedded + directory, deduped across both** (see
  `load_dict`/`load_dir_files` in [`main.rs`](cha-gui/src-tauri/src/main.rs)).
  On startup the app creates a `dictionaries/` subfolder of the app config dir
  (e.g. `~/Library/Application Support/org.saturnvalley.cha/dictionaries`, or
  `~/.config/…` on Linux), then loads *every* regular non-hidden file in it on
  top of the embedded list — additive, not a replacement. Hidden files
  (`.DS_Store`) are skipped so their binary contents don't inject junk words;
  files load in sorted-name order; unreadable files are warned-and-skipped.
  `cha_core::dictionary::WordListBuilder` does the cross-source trim/lowercase/
  dedup. Files are read once at launch, so newly added lists need a reopen (the
  notice says so).
- **Word lists keep their provenance — matches are grouped and labeled by
  source.** `WordListBuilder` is *group-aware*: `begin_source(name)` starts a new
  named group and subsequent `add_str`/`add_file` calls append to it, while dedup
  stays **global** (first-seen wins — a word appears only under the first list
  that contained it, so built-in words never reappear under a custom list).
  `finish_grouped()` returns the ordered `Vec<NamedWordList>` (empty groups
  dropped); the old flat `finish()` still exists for the CLI and other callers,
  so that path is unchanged. `load_dict` names the embedded list `"Built-in"` and
  each file by its extension-stripped stem (`list_name` → `Path::file_stem`).
  `Dict` holds `lists: Vec<NamedWordList>` in **display order** (built-in first,
  then sorted files); ordering lives *only* here, so a future config step just
  reorders this vec — nothing downstream assumes an order. `search` scans each
  list in order and returns `SearchResult { groups: Vec<MatchGroup>, total,
  list_count, note }` — one `MatchGroup { list, matches }` per list *with*
  matches. The front end (`render` in [`main.js`](cha-gui/ui/main.js)) draws an
  unobtrusive `.list-header` labeled rule before each group, but **only when
  `list_count > 1`** — a single-list setup shows no headers and looks exactly as
  it did before. This is the first step toward multiple selectable dictionaries.
- **Opening the folder:** File → Open Dictionary Folder and the notice's
  "Open Dictionary Folder" button both reach `open_dict_dir_impl`, which
  `create_dir_all`s the folder then shells out to `open`/`explorer`/`xdg-open`
  via `open_folder`. Spawning a subprocess is safe off the event-loop thread
  (unlike window creation), so the front-end button can invoke the
  `open_dict_dir` command directly. Custom commands need no capability entry.
- **The empty-dictionary notice.** When *no* source yielded any words,
  `dict_status` returns a user-facing message naming the `dictionaries/` path,
  which the front end shows as a notice on startup (and disables input) so an
  empty result area isn't mistaken for "no matches". On an embedded build this
  never fires (embedded words are always present). The `search` command caps the
  materialized rows at `MAX_RESULTS` (5000) *across all groups combined* but
  keeps counting `total` truthfully through every list, so a pattern like `*`
  can't flood the DOM. The front end's "showing first N of M" status sums the
  rows it actually rendered.
- **`time` is pinned to `=0.3.47`** in `cha-gui/src-tauri/Cargo.toml`. 0.3.48
  trips an E0119 coherence false-positive (rust-lang/rust#100712) against
  `cookie 0.18.1` under rustc 1.96; 0.3.47 still satisfies plist's `^0.3.47`.
  Don't drop the pin (or let `cargo update` move it) until tauri/cookie or rustc
  resolves it.
- **`icons/icon.ico` must keep its small entries (16/20/24), and `tauri icon`
  destroys them.** The Windows title bar asks for a 16×16 icon (20 and 24 at
  125%/150% DPI). With no exact match Windows falls back to crude-shrinking the
  32×32, which looks visibly jagged — while the taskbar, which asks for 32×32 and
  finds it, still looks fine. **That split (janky title bar, clean taskbar) is the
  tell for a missing small entry**, not a corrupt icon. `tauri icon`'s generator
  hardcodes `[32, 64, 128, 256]`, so re-running it silently drops the small sizes
  and reintroduces the jaggies with no error. Regenerate from `icons/icon.png`
  with Pillow instead — `save(dst, format="ICO", sizes=[(16,16), …])`, resampling
  each frame with LANCZOS. Entries are PNG-compressed (smaller than BMP, and fine
  on Win10+, which Tauri 2 requires anyway). The `.ico` is embedded into the exe's
  resources at build time, so a rebuild is needed before any icon change is
  visible. Note that 茶 is close to illegible at 16px no matter how it's
  resampled — thickening the strokes before downscaling was tried and only made it
  blobbier. Real crispness would need a hand-drawn 16×16 as its own entry.

### Which thread runs what (Tauri v2)

**Sync commands run on the main (event-loop) thread; `async` ones don't.** A plain
`#[tauri::command] fn` executes inline on the event-loop thread, so a slow one
freezes the window — no typing, no repaint — for as long as it runs. Marking it
`#[tauri::command(async)]` (or making it an `async fn`) moves it to Tauri's worker
pool. `search` scans the whole word list and **must** stay `(async)`; its body is
still synchronous, so `(async)` here is purely a "run me off the main thread"
switch, not a concurrency model. `State` works either way provided it's
`Send + Sync` (`Dict` is).

This one rule produces both GUI threading hazards, in opposite directions —
slow work must go *off* the main thread (`search`), while window creation must
stay *on* it (next section). A hand-rolled `async` command that calls `build()`
violates the second, which is the likely origin of the Windows deadlock below.

**Concurrent searches need a staleness guard.** Once `search` is `(async)`, two
searches can be in flight at once and resolve out of order, letting a slow one
clobber a newer one's results. `run()` in [`main.js`](cha-gui/ui/main.js) stamps
each search with a monotonic `latestSearch` id and drops any result that isn't
the newest, so the freshest query always wins and typing is never blocked. Keep
that guard if you touch the debounce — the debounce alone does *not* prevent
overlap, it only delays it.

### Multiple windows and menus (Tauri v2, hard-won on Windows)

The app has File → New Window (open another search window) and Help → Pattern
Syntax (a singleton static cheat-sheet window). Getting this working
cross-platform surfaced several non-obvious traps — all in
[`main.rs`](cha-gui/src-tauri/src/main.rs), [`main.js`](cha-gui/ui/main.js), and
[`capabilities/default.json`](cha-gui/src-tauri/capabilities/default.json):

- **Create windows only on the event-loop (main) thread.** `WebviewWindowBuilder::build()`
  off the main thread on Windows half-creates a blank window and then deadlocks the
  whole app. Note that *`async`* command handlers (and only those — see the previous
  section) run off the event-loop thread, so building a window from one is the way
  into this trap.
  `run_on_main_thread` from inside a command did **not** reliably break this.
  Two patterns that *do* work: (a) from Rust, build windows only in event-loop
  callbacks like `on_menu_event` (where `open_search_window` /
  `open_pattern_syntax_window` are called); (b) from the front end, use the JS
  `new WebviewWindow(label, opts)` API (`window.__TAURI__.webviewWindow`), which
  lets Tauri schedule creation on the event loop for you. Do **not** hand-roll an
  `invoke("new_window")` → `build()` command.

- **Menu accelerators don't fire on Windows when the webview has focus.** WebView2
  swallows the keystroke before the native accelerator table sees it. Standard
  editing keys (Ctrl+C/V/X/A/Z) still work anyway because WebView2 implements
  them *itself* — independent of the menu — but a custom accelerator like Ctrl+N
  just evaporates. Fix: bind it in a JS `keydown` handler. Gate that handler to
  **non-macOS** (`navigator.platform`): on macOS the native menu consumes Cmd+N
  before the webview sees it, so a JS handler there would open two windows. Keep
  the `.accelerator("CmdOrCtrl+N")` on the menu item regardless — it drives the
  macOS behavior and shows the shortcut hint everywhere.

- **The capability `windows` list must glob to match runtime windows.** Each new
  window gets a unique label (`main-2…` from Rust, `main-<timestamp>` from JS), so
  the capability is scoped to `["main", "main-*", "pattern-syntax"]`. Without the
  glob, a new window's `invoke()` calls are silently blocked. Creating a window
  *from the front end* additionally needs the
  `core:webview:allow-create-webview-window` permission.

- **The macOS app menu is macOS-only.** `build_menu` gates the App submenu
  (About/Services/Hide/Quit) behind `#[cfg(target_os = "macos")]`; on Windows/Linux
  it's not idiomatic, so Quit moves under File there. Wire the menu via
  `Builder::menu(build_menu)` (which takes `&AppHandle`), **not**
  `App::set_menu` in `setup` — the former registers the accelerator table on the
  initial window at creation. Standard items are `PredefinedMenuItem`s (Tauri
  owns their labels/localization); only New Window and Pattern Syntax are custom.

- **Singleton windows:** re-opening Pattern Syntax focuses the existing window via
  `get_webview_window("pattern-syntax")` + `set_focus()` instead of stacking
  duplicates. `AppHandle::clone()` is cheap (an `Arc` bump) — clone freely to move
  a handle into a `'static` closure.

## What to avoid

- Do not allocate `Vec<char>` or `String` on the per-word *reject* path of the
  anagram loop. Allocations there are immediately measurable in benchmarks. (The
  `MatchInfo` strings built by `diff_letters` are fine — they only allocate once a
  word has already been confirmed as a match, which is comparatively rare.)
- Do not call `count_chars` inside the closure. Pool counters are pre-computed;
  only `count_str(word)` belongs inside the closure.
- Do not replace `[usize; 26]` with `HashMap` in the character-counting code.
  The HashMap version was ~6× slower on anagram queries.
- `fancy_regex` is required (not the plain `regex` crate) because digit
  variables (`1234321`) compile to named capture groups with backreferences,
  which a pure DFA cannot handle.
- Do not build a multi-source word list by concatenating `load_words` /
  `load_words_from_str` results. Each call dedups only against its *own*
  `HashSet`, so merging their `Vec`s dedups within each source but not across
  them — embedded `apple` + a user file's `Apple` would both survive and every
  matcher would report the word twice. Dedup state must span all sources: use
  `WordListBuilder` and call `add_str`/`add_file` per source. (This is also why
  the builder exists at all rather than a plain function — `load_dict` needs to
  interleave per-file policy, like skipping hidden and unreadable files, between
  adds, and `add_file` keeps streaming so a big list never sits in memory *in
  addition to* the growing result.)
