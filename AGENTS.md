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
explicitly with `cargo build -p cha-gui` when touching it. The GUI now has a
`[lib]` with three `crate-type`s, so a plain `cargo build -p cha-gui` also links
a staticlib and a cdylib of the whole Tauri stack; add `--bins` when you only
want the desktop exe.

**The mobile `#[cfg(mobile)]` code paths are invisible to the host clippy.** It
compiles the desktop cfg, so a broken mobile branch (the absent `desktop`
module, the mobile `load_dict` arm, `mobile_entry_point`) rots silently. Before
committing anything under `cha-gui`, also run the two cross-compiles — clippy
doesn't link, so these need only `rustup target add`, no NDK/Xcode/device:

```
cargo clippy -p cha-gui --target aarch64-apple-ios
cargo clippy -p cha-gui --target aarch64-linux-android   # needs NDK_HOME set
```

**To exercise the no-word-list path**, temporarily move `words.txt` out of the
repo root and rebuild the GUI. With `words.txt` present (the usual case) the list
is embedded, so the empty-dictionary notice and its "Open Dictionary Folder"
button are unreachable and changes to them go untested. `build.rs` tracks the
path via `rerun-if-changed` whether or not it exists, so moving it away (and
back) re-evaluates the `words_embedded` cfg with no `cargo clean` needed.
**This is desktop-only:** a mobile target (`android`/`ios`) with no `words.txt`
is a hard `build.rs` panic, not a graceful notice — mobile has no `dictionaries/`
folder to fall back to, so a dictionary-less mobile app can't be recovered by
the user and must not build. See the mobile section.

## Performance requirements

`cha` searches ~270k words per query, and has ambitions to search
even larger (>10M words) lists. Matching must be fast enough to feel
instantaneous on a modern laptop. Current release-build baselines:

| Pattern type | Target | Achieved |
|---|---|---|
| Template (e.g. `qu...`) | < 10 ms | ~5 ms |
| Anagram (e.g. `;..oting`) | < 20 ms | ~8 ms |

The benchmark flags (`cha <pattern> -w <wordlist> -b <N>`) are the primary way to
measure regressions. Run with `-b 1000` to get stable averages, e.g.
`cha ';..oting' -w words.txt -b 1000`. Always compare against a baseline you
measured on the *same machine* (`git stash`, build, measure, `git stash pop`) —
the absolute numbers in the table above are hardware-specific and now read low.

These are laptop **release** numbers. A phone CPU runs the hot loop maybe 2–3×
slower, still comfortably interactive behind the 100 ms debounce. **Debug builds
are the real trap:** unoptimized, the matcher is 10–50× slower and the mobile app
looks broken — and `tauri {ios,android} dev` builds debug by default. The root
`Cargo.toml` therefore forces `[profile.dev.package.cha-core] opt-level = 3`
(leaf crate, negligible compile-time cost) so even dev builds have a usable
matcher. Keep that profile; still prefer `--release` for any real timing.

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

## Every backtracking path needs a bound (`CompileLimits`)

Pattern input is untrusted — even from a local user, a plausible-looking pattern
could hang or OOM the app. There are **three** superlinear paths in `pattern.rs`,
all now bounded by `CompileLimits`, whose `Default` is generous enough that no
hand-typed pattern reaches it. `compile_pattern`/`compile_pattern_checked` use
the defaults; `compile_pattern_with`/`compile_pattern_checked_with` take explicit
limits, which is how a server passes tighter ones.

- **`max_anagram_combos` — the dangerous one, and the only one that binds at
  *compile* time.** `compile_anagram` calls `cartesian_product`, which
  materializes the full product of every `[...]` group *before any word is
  scanned*, and `combo_pools` then expands each combo into 216 bytes. Growth is
  multiplicative in the group count: `;[abcde]`×8 is 390_625 combos (~84 MB,
  ~28 s) and ×10 is ~9.7M (~2.1 GB). Because it happens during compile, a
  per-word deadline or a scan timeout **cannot** catch it — the check must stay
  where it is, before the product is built. Use `checked_mul`: the product
  overflows `usize` at around 28 five-way groups, and a wrapped value would slip
  under the cap.
- **`backtrack_limit`** bounds `fancy-regex` on the non-fuzzy template path,
  where `template_to_regex` turns every `*` into `[a-z]*`.
- **`max_fuzzy_steps`** bounds `fuzzy_match`, the hand-rolled backtracker on the
  fuzzy path. Note its existing `budget` parameter is the *fuzz allowance*, a
  different quantity — don't overload it. The doc comment there reasons correctly
  about recursion *depth*, but depth was never the exposure; the `Star` arm
  branches twice per node, and nothing bounded the node count.

Exceeding a *match-time* limit degrades to "no match" (via the existing
`unwrap_or(false)` and the `steps == 0` early return), which is what keeps the
hot path `Result`-free — see the section above. Exceeding the *compile-time*
limit is a normal `PatternError`.

When adding a limit, test both halves: rejected under tight limits **and**
behaviorally unchanged under the defaults, so a limiter can't silently narrow the
pattern language.

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
- **The dictionary is embedded + directory, deduped across both** (`load_dict`
  in [`lib.rs`](cha-gui/src-tauri/src/lib.rs); the directory half —
  `add_user_lists`/`load_dir_files` — is desktop-only and lives in
  [`desktop.rs`](cha-gui/src-tauri/src/desktop.rs)). On startup the app (on
  desktop) creates a `dictionaries/` subfolder of the app config dir
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
- **`icons/icon.ico` needs its small entries, and the 32px one must come first.**
  Tauri's requirement: *"The ico file must include layers for 16, 24, 32, 48, 64
  and 256 pixels. For an optimal display of the ICO image in development, the 32px
  layer should be the first layer."* The first-layer rule is real — Tauri's icon
  codegen reads the *first* directory entry, so a 16px-first `.ico` hands it the
  16px art. Ours carries 16, 20, 24, 32, 48, 64, 128, 256 with **32 first**; 20
  and 128 are additions beyond the required set (20 covers 125% DPI).
- **Symptom to recognize:** a jagged title bar with a *clean* taskbar means a
  missing small layer, not a corrupt icon. The title bar asks for 16×16 (20/24 at
  125%/150% DPI) and crude-shrinks the 32×32 when there's no exact match; the
  taskbar asks for 32×32, finds it, and looks fine. That asymmetry is the tell.
  This is how the icon shipped between 9872cf3 and 8522493: it was hand-packed
  from the four PNGs sitting in `icons/` (32/64/128/256), so 16/24/48 were never
  in it.
- **`tauri icon` is not the enemy here** — it emits 16/24/32/48/64/256, 32 first,
  and is the documented path. It's avoided for the *desktop* icons only because it
  rewrites every PNG (and a fresh `icns`/`ico`, clobbering the hand-packed `.ico`)
  and drops `Square*Logo.png`/`StoreLogo.png` into `icons/`, and it omits the 20px
  layer. (The "drops `android/`+`ios/` into `icons/`" behavior is *conditional* —
  it only happens when `gen/android`/`gen/apple` don't yet exist; once they do,
  `tauri icon` writes the mobile icons straight into `gen/`, which is what we
  want. See the mobile section for the scratch-dir recipe that gets mobile icons
  without touching `icons/`.) If you regenerate the desktop icon by hand,
  note **Pillow always writes the directory in ascending size order**
  (`sorted(set(sizes))`), so the 32-first rule needs a post-pass that reorders the
  16-byte directory entries — safe to do, since entries carry explicit offsets and
  the image data doesn't move.
- **`build.rs` must track `icons/icon.ico` explicitly.** The `.ico` is baked into
  the exe's resources at build time, and `tauri_build` does *not* register it for
  change detection. Worse, emitting *any* `rerun-if-changed` (this script emits
  one for `words.txt`) makes that list exhaustive — cargo stops falling back to
  "rerun if any file in the package changed". Without the explicit
  `rerun-if-changed=icons/icon.ico`, editing the icon rebuilds **nothing**, not
  even with a fresh mtime, and the exe silently keeps its old icon — which reads
  as "my icon fix didn't work" when the real problem is that it was never
  compiled in.
- 茶 is close to illegible at 16px however it's resampled — thickening the strokes
  before downscaling was tried and only made it blobbier. Real crispness needs a
  hand-drawn 16×16 as its own entry.

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
Syntax (a singleton static cheat-sheet window). This is **desktop-only** — it all
lives in [`desktop.rs`](cha-gui/src-tauri/src/desktop.rs) behind the
`#[cfg(desktop)]` module (see the mobile section). Getting it working
cross-platform surfaced several non-obvious traps — in
[`desktop.rs`](cha-gui/src-tauri/src/desktop.rs), [`main.js`](cha-gui/ui/main.js),
and the [`capabilities/`](cha-gui/src-tauri/capabilities/) files:

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
  `default.json` scopes to `["main", "main-*", "pattern-syntax"]`. Without the
  glob, a new window's `invoke()` calls are silently blocked. Creating a window
  *from the front end* additionally needs the
  `core:webview:allow-create-webview-window` permission — which lives in a
  separate `capabilities/desktop.json` scoped `"platforms": ["macOS", "windows",
  "linux"]`, so mobile (which has no multiwindow) never grants it. A capability
  whose `platforms` excludes the target is silently filtered out, not an error;
  the platform names are case-sensitive (`"macOS"`, `"iOS"`).

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

## Mobile (iOS + Android, Tauri v2)

The same crate and the same `cha-gui/ui` front end ship to five platforms. Mobile
is deliberately stripped down: **embedded dictionary only** (no config dir, no
"Open Dictionary Folder"), **no multiwindow**, and the pattern-syntax cheat sheet
reached through an in-page sheet instead of a menu. Desktop rendering and behavior
are unchanged — every mobile addition is behind a cfg seam, a `.mobile` body
class, or a CSS rule that is a literal no-op on desktop.

- **The lib/bin split.** `run()` in [`lib.rs`](cha-gui/src-tauri/src/lib.rs) is
  the single entry point for all platforms — the desktop
  [`main.rs`](cha-gui/src-tauri/src/main.rs) is a 5-line shim that only holds
  `windows_subsystem` (a bin-crate attribute) and calls `cha_gui_lib::run()`; on
  mobile the platform shell calls `run()` via `#[cfg_attr(mobile,
  tauri::mobile_entry_point)]`. `Cargo.toml` has `[lib] name = "cha_gui_lib"`
  with `crate-type = ["staticlib", "cdylib", "rlib"]` — staticlib for iOS, cdylib
  for Android, rlib for the desktop bin. The `_lib` suffix avoids a Windows
  bin/lib artifact collision (cargo#8519), and this repo ships Windows, so keep
  it. `crate-type` can't be cfg-gated (hence `--bins` for a fast desktop build).
- **`#[cfg(desktop)] mod desktop;` is the one seam.** Everything mobile doesn't
  have — the menu bar, extra windows, the Pattern Syntax window, the config-dir
  dictionary, the file-manager shell-out — lives in
  [`desktop.rs`](cha-gui/src-tauri/src/desktop.rs). Because the module isn't
  compiled on mobile, nothing in it can be dead code there; because it's all
  reachable on desktop, nothing is dead there either. **Neither platform needs a
  single `#[allow(dead_code)]`.** A new desktop-only feature goes *in that
  module*, not behind a fresh inline `#[cfg]` in `lib.rs`. The only unavoidable
  straddler is `load_dict`, whose two `#[cfg]` lines are commented as such.
- **`generate_handler![]` takes per-entry `#[cfg]`.** The mobile handler list
  omits `desktop::open_dict_dir` via `#[cfg(desktop)]` right inside the macro
  (tauri-macros re-emits the attr onto the generated match arm). This keeps one
  handler list instead of two divergent copies. If it ever breaks, the fallback
  is two `#[cfg]`'d `.invoke_handler(...)` calls.
- **`is_mobile` is the front end's only source of platform truth.** Its body is
  `cfg!(mobile)` — an *expression*, so one command serves both platforms. The
  front end (`init()` in [`main.js`](cha-gui/ui/main.js)) awaits it once at
  startup and either shows the mobile help button or installs the desktop Ctrl+N
  handler. **Don't** UA-sniff (iPadOS WKWebView reports ambiguously) and **don't**
  infer platform from `@media (pointer: coarse)` (a touch laptop matches it —
  that's a touch question, not a platform question). Also note the
  `window.__TAURI__.webviewWindow` destructure lives *inside* the desktop branch,
  not at top level, so a mobile bundle that omits it can't throw and kill the
  whole script.
- **Mobile is embedded-only by construction, and that's enforced, not hoped.**
  `build.rs` hard-errors on an `android`/`ios` target with no `words.txt` (via
  `CARGO_CFG_TARGET_OS`). That guarantee is what lets the front end skip gating
  the "Open Dictionary Folder" notice button: on mobile `dict_status` can't return
  a message (the embedded list is always non-empty), so the button is unreachable
  rather than conditionally hidden. Adding user lists on mobile would need a
  file-picker plugin and a real design — don't half-do it.
- **Mobile CSS is additive by construction.** In
  [`styles.css`](cha-gui/ui/styles.css), `env(safe-area-inset-*)` is `0px` and
  `100dvh == 100vh` on desktop, so the safe-area/viewport rules ship
  unconditionally and cost desktop nothing — no class, no cfg, no first-paint
  flash. The `.mobile` class and `is_mobile` gate only real behavior (the help
  button, the Ctrl+N handler). Keep it that way: a rule that needs `.mobile` to
  *avoid* breaking desktop is written wrong. `viewport-fit=cover` on the viewport
  meta is required for the insets to be non-zero and is a desktop no-op.
- **`#pattern` must stay ≥16px** (it's 18px). iOS zooms the page when a focused
  `<input>` is under 16px, and the zoom doesn't cleanly undo. This looks like a
  harmless tidy-up and isn't.
- **Result rows (`.word`) are deliberately not touch targets.** They're
  non-interactive text; 44px rows would cost ~45% of the visible words for no
  gain. The only 44px targets are `#help`/`#help-close`. If rows ever become
  tappable (copy-on-tap), *that's* when the sizing question opens.
- **Pattern help on mobile reuses the desktop file verbatim.** The `?` button
  opens [`pattern-syntax.html`](cha-gui/ui/pattern-syntax.html) — the very page
  the desktop Help menu opens in a window — inside a full-screen `<iframe>` sheet
  (`#help-sheet`). It's pure static HTML with no JS/Tauri, so it drops into the
  iframe unmodified: one source of truth, zero duplication. The sheet container
  carries the safe-area padding because an iframe can't see its parent's `env()`
  insets. `openHelp` pushes a history entry so Android's hardware **Back closes
  the sheet, not the app** (verified on the emulator); ✕ and Escape also close it.
- **`gen/schemas/` is git-ignored; `gen/android` and `gen/apple` are committed.**
  Only the ACL schemas regenerate per build; the Xcode and Gradle projects from
  `tauri {ios,android} init` are one-shot and hold the Kotlin activity, plists,
  and mobile icons — `android init` isn't reproducible enough to regenerate on a
  clean checkout. **Never `rm -rf gen/`.** The generated trees carry their own
  `.gitignore`s for build outputs (`build/`, `.gradle/`, `Pods/`, `Externals/`,
  `local.properties`, `jniLibs/**/*.so`); sanity-check `git status` after an init.
- **Mobile icons: the scratch-dir recipe, never `tauri icon` in place.** In-place
  it would clobber the hand-packed `icons/icon.ico` (and `build.rs` tracks that by
  mtime, so a touch-and-revert triggers a misleading rebuild). Instead, source the
  true 1024px master out of the icns and send everything to a scratch dir, then
  copy only the mobile outputs:
  ```
  iconutil -c iconset icons/icon.icns -o "$S/cha.iconset"
  cargo tauri icon "$S/cha.iconset/icon_512x512@2x.png" -o "$S/out"
  cp "$S/out/ios/"*.png gen/apple/Assets.xcassets/AppIcon.appiconset/   # keep the generated Contents.json
  rsync -a "$S/out/android/" gen/android/app/src/main/res/
  ```
  This can't damage `icons/` even if you forget the follow-up. Note the Android
  **adaptive** foreground is derived from the square icon and Android masks/crops
  ~25% off the edges, so 茶 loses its outer strokes — the mechanical output is a
  starting point; a proper foreground (respecting the 66/108 safe zone, via
  `tauri icon --android_fg/--android_bg`) wants a hand pass.

### Mobile toolchain and driving a device

One-time setup on macOS: Xcode + `brew install cocoapods xcodegen`; Android Studio
or `brew install --cask android-commandlinetools` plus `sdkmanager` for
`platform-tools`, `platforms;android-34`, `build-tools;34.0.0`, and an `ndk;…`;
JDK 17 or 21 (**not** 24 — Android Gradle rejects it); `rustup target add` the 3
iOS + 4 Android targets. Export `ANDROID_HOME`, `NDK_HOME`, and a JDK-21
`JAVA_HOME`. `tauri android init` reads `[lib]` from `Cargo.toml`, so do the
lib/bin split first.

```
cargo tauri ios dev "iPhone 17"           # simulator; --release to judge feel
cargo tauri android build --debug --apk --target aarch64   # then adb install/monkey
```

A freshly-booted Android emulator under heavy host load throws "Process system
isn't responding" (that's the emulator's own system_server, not the app); free
CPU and relaunch with `am start -n org.saturnvalley.cha/.MainActivity`. `eprintln!`
(which the code already uses) lands in `adb logcat` / `xcrun simctl … log stream`,
so it's the zero-dependency way to time `load_dict` if a phone ever shows a blank
startup stall — currently it doesn't, so the parse stays inline in `setup()` and
`dict_status` stays sync. If that changes, moving the parse off-thread means
`dict_status` must become `(async)` too, or it blocks the event loop.

### Test deployment to a real device

**The two platforms are not symmetric.** Android lets you build a self-signed APK
and hand it to anyone. iOS binds every install to signing that authorizes a
specific device or an App Store channel — there is no sideload-an-`.ipa`
equivalent, and remote testing effectively requires the paid Apple Developer
Program ($99/yr).

**iOS, cabled local device (free, no paid account, your own phone only).** Good
for a quick real-device smoke test. A free "Personal Team" signs apps that run
only on a device cabled to (or paired with) your Mac, expire after 7 days, and
can't use most entitlements — Cha needs none, so it's fine.
1. Plug in the iPhone, unlock, tap **Trust This Computer**, enter the passcode.
2. `cargo tauri ios open` → Xcode → target → **Signing & Capabilities** → check
   *Automatically manage signing* → **Team** → *Add an Account* (your Apple ID) →
   pick the Personal Team. Xcode writes `DEVELOPMENT_TEAM` into the **pbxproj**,
   which XcodeGen *regenerates from `project.yml`* on the next `cargo tauri ios`
   command — so that edit isn't durable and would also commit your personal team
   id. Move the value instead into **`Signing.local.xcconfig` at the repo root**
   (git-ignored) as `DEVELOPMENT_TEAM = XXXXXXXXXX`. `gen/apple/Signing.xcconfig`
   (committed, carrying no id) `#include?`s it by a relative `../../../../` climb
   to the root, and `project.yml` references that xcconfig — so the team survives
   regeneration, never lands in git, and a clone without the local file still
   builds for the Simulator (which needs no signing). It's kept at the root, not
   beside `Signing.xcconfig`, so it's visible and hard to lose; the four `../`
   must stay in sync with `gen/apple`'s depth if the project layout moves.
3. On the phone, enable **Developer Mode**: Settings → Privacy & Security →
   Developer Mode → on → restart. (Required on iOS 16+ to run dev-signed apps;
   the toggle only appears after a dev build has been targeted at the device.)
4. `cargo tauri ios dev "<iPhone name>"` (it lists connected devices). First launch
   may need Settings → General → VPN & Device Management → *Developer App* → Trust.
5. Re-run to refresh before the 7-day signature expires.

**iOS, building from Xcode's GUI.** Xcode launched from the Dock/Finder runs
with a minimal launchd `PATH` that lacks `~/.cargo/bin`, so the "Build Rust Code"
phase fails with *"Cargo: command not found"* — even though `cargo tauri ios …`
works in a terminal (whose shell `PATH` has it). The fix lives in
`gen/apple/project.yml`'s `preBuildScripts`, which prepends
`export PATH="$HOME/.cargo/bin:$PATH"` before calling `cargo tauri ios
xcode-script`. Keep it there (XcodeGen bakes it into the pbxproj on regeneration);
without it, only terminal builds work. For a standalone on-device build that
survives unplugging, prefer `cargo tauri ios dev --release --no-watch "<device>"`
— it installs a release build directly and sidesteps `ios run`'s broken
IPA-export step (`Couldn't load -exportOptionsPlist … no such file`).

**iOS, remote tester → TestFlight (paid).** Register the app id
`org.saturnvalley.cha` in App Store Connect; set your real Team in the Xcode
project; **bump the version** (App Store Connect rejects a duplicate
`CFBundleVersion`, and Tauri derives it from the crate version, so bump the patch
in the root `Cargo.toml`); archive & upload via the Xcode Organizer (Product →
Archive → Distribute → TestFlight) or `cargo tauri ios build --export-method
app-store-connect` + the Transporter app. Add the tester in TestFlight (internal
= instant; external = one-time Beta App Review). `gen/apple/ExportOptions.plist`
starts as `method: debugging`; `--export-method` rewrites it — don't hand-edit.

**Android, remote tester → signed APK.** Release signing is wired into the build
(see the next section), so `cargo tauri android build --apk` emits a *signed*,
installable release APK directly — the output is
`gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk`.
`adb install` it or send the file. **Updates must keep the same signing key and a
higher `versionCode`** (crate-version-derived, in the git-ignored
`gen/android/app/tauri.properties`), or Android refuses the install. Scaling past
one tester is Play Console internal testing, which wants the **AAB** (`--aab`).

### Release signing and mobile CI

**Android signing lives in Gradle, driven by a git-ignored properties file.**
[`gen/android/app/build.gradle.kts`](cha-gui/src-tauri/gen/android/app/build.gradle.kts)
has a `signingConfigs { create("release") { … } }` block (added by hand — this
file is generated once and then owned by us, *unlike* the iOS pbxproj) that reads
`rootProject.file("keystore.properties")`. Tauri's convention uses a **single
`password`** for both the store and the key, plus `keyAlias` and `storeFile` —
not separate store/key passwords. The casts are nullable (`as String?`) and
`storeFile` is guarded, so a build with **no** `keystore.properties` (a plain
debug build, or a fresh clone) still works instead of throwing; only release
signing goes unpopulated.
- `gen/android/keystore.properties` is **git-ignored** (by `gen/android/.gitignore`)
  and holds `password`/`keyAlias=upload`/`storeFile=<abs path>`. The `.jks` lives
  **outside the repo** (`~/keystores/cha-upload.jks`); never commit either.
- **Generate the key once:** `keytool -genkeypair -v -keystore
  ~/keystores/cha-upload.jks -keyalg RSA -keysize 2048 -validity 10000 -alias
  upload`. The DN fields (CN/OU/O/…) are cosmetic — Android/Play validate only the
  key's algorithm, validity, and cross-update consistency, never the DN text.
  **Losing the password or the `.jks` means you can never update the app** for
  existing installs. Verify a build with
  `build-tools/…/apksigner verify --print-certs <apk>`.
- **The real release risk is R8, not signing.** `release` has
  `isMinifyEnabled = true`; a signed APK that *builds* can still crash if proguard
  strips Tauri/webview classes. Always install-and-run the release APK, don't just
  build it. (Verified clean with the current Tauri proguard rules.)

**Mobile CI is [`.github/workflows/mobile.yml`](.github/workflows/mobile.yml)**,
separate from the desktop `release.yml`. `workflow_dispatch` build-checks both
platforms and uploads artifacts; a `mobile-v*` tag additionally attaches the
signed Android APK + AAB to a (draft) GitHub release.
- **Android job** (`ubuntu-latest`): setup-java 17 → setup-android → `sdkmanager
  "ndk;<NDK_VERSION>"` → rust-toolchain with the 4 android targets → cargo-binstall
  tauri-cli → decode `ANDROID_KEYSTORE_BASE64` + write `keystore.properties` from
  secrets → `cargo tauri android build --apk --aab`.
- **iOS job** (`macos-latest`): **build-check only** — `cargo build -p cha-gui
  --lib --target aarch64-apple-ios` cross-compiles the shared library with **no
  Xcode archive, no signing, no secrets**. `cargo tauri ios build` was tried first
  but it always *archives* (device), which needs a signing team CI doesn't have
  (the repo-root `Signing.local.xcconfig` is git-ignored) — so it fails on the
  runner even with `--no-sign`/a Simulator target, and only "worked" locally
  because this Mac has a cert. The cross-compile catches the breakage that matters
  (the shared Rust code building for iOS), is arch-agnostic, and needs macOS only
  because the iOS SDK is Xcode-only. Producing a *signed* iOS build (let alone
  TestFlight) is the paid-tier follow-up: iOS needs an **iOS-type** cert (Apple
  Development/Distribution — the macOS `APPLE_*`/Developer ID secrets can't sign
  iOS) plus a provisioning profile or App Store Connect API-key automatic signing,
  then `cargo tauri ios build --archive-only` (sign, no upload) or
  `--export-method app-store-connect` (TestFlight). Deliberately deferred.
- **`words.txt` in CI:** it's git-ignored and `build.rs` hard-errors without it,
  and it's too big (639 KB) for a 48 KB GitHub secret. Both jobs run a
  "materialize words.txt" step: **`WORDS_URL` secret if set, else the committed
  `ci/words-stub.txt`** (a ~2k-word public-domain placeholder — real enough that a
  search returns matches, but *not* shippable). This is the upgrade seam: host the
  real list, add a `WORDS_URL` secret, and tagged builds ship the real dictionary
  with **no workflow edit**. Until then, CI release assets carry only the stub.
- **Secrets to add now** (repo Settings → Secrets and variables → Actions):
  `ANDROID_KEYSTORE_BASE64` (`base64 -i ~/keystores/cha-upload.jks | pbcopy`),
  `ANDROID_KEY_PASSWORD`, `ANDROID_KEY_ALIAS` (=`upload`). The Android job hard-fails
  fast if `ANDROID_KEYSTORE_BASE64` is missing rather than shipping an unsigned APK.

## What to avoid

- Do not allocate `Vec<char>` or `String` on the per-word *reject* path of the
  anagram loop. Allocations there are immediately measurable in benchmarks. (The
  `MatchInfo` strings built by `diff_letters` are fine — they only allocate once a
  word has already been confirmed as a match, which is comparatively rare.)
- Do not call `count_chars` inside the closure. Pool counters are pre-computed;
  only `count_str(word)` belongs inside the closure.
- Do not replace `[usize; 26]` with `HashMap` in the character-counting code.
  The HashMap version was ~6× slower on anagram queries.
- Do not add a backtracking or combinatorial path to `pattern.rs` without a
  ceiling in `CompileLimits`, and do not call `cartesian_product` without
  checking the product size first. See the `CompileLimits` section — every such
  path is reachable from untrusted input by a short pattern.
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
