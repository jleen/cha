# Implementation notes for cha

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

## GUI (`cha-gui`, Tauri v2)

- The GUI is a separate workspace member that reuses `cha-core`. The front end
  is **vanilla HTML/JS/CSS with no bundler** — `withGlobalTauri: true` exposes
  `window.__TAURI__.core.invoke`, so there is no Node/npm step. Don't introduce
  a JS framework or build tool without a strong reason.
- **The word list is embedded via `include_str!` when `words.txt` is present at
  the repo root at build time** (the usual case). `build.rs` gates the embed
  behind a `words_embedded` cfg, so when the file is absent the GUI still
  compiles and instead loads `words.txt` from the app config dir at runtime
  (e.g. `~/.config/org.saturnvalley.cha/words.txt`); a missing or unreadable
  file there leaves an empty dictionary rather than aborting. The cfg can't be a
  runtime `if` — `include_str!` expands unconditionally — which is why the
  decision lives in `build.rs`. The `search` command caps returned matches at
  `MAX_RESULTS` (5000) but reports the true total, so a pattern like `*` can't
  flood the DOM.
- **`time` is pinned to `=0.3.47`** in `cha-gui/src-tauri/Cargo.toml`. 0.3.48
  trips an E0119 coherence false-positive (rust-lang/rust#100712) against
  `cookie 0.18.1` under rustc 1.96; 0.3.47 still satisfies plist's `^0.3.47`.
  Don't drop the pin (or let `cargo update` move it) until tauri/cookie or rustc
  resolves it.

## What to avoid

- Do not allocate `Vec<char>` or `String` inside the per-word anagram loop.
  Allocations in that path are immediately measurable in benchmarks.
- Do not call `count_chars` inside the closure. Pool counters are pre-computed;
  only `count_str(word)` belongs inside the closure.
- Do not replace `[usize; 26]` with `HashMap` in the character-counting code.
  The HashMap version was ~6× slower on anagram queries.
- `fancy_regex` is required (not the plain `regex` crate) because digit
  variables (`1234321`) compile to named capture groups with backreferences,
  which a pure DFA cannot handle.
