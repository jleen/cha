> **Provenance:** Written by Claude (Anthropic) while doing the work it
> describes. Reviewed in the normal course of review, but not audited line by
> line — treat specific numbers, paths, and version pins as claims to verify
> rather than guarantees.
>
> See [docs/README.md](docs/README.md).

# Instructions for agents working in this repo

Cha is a Rust workspace shipping one matcher (`cha-core`) to five surfaces: a
CLI (`cha`), a Tauri desktop app (`cha-gui`), an axum server (`cha-web`), and
iOS + Android builds of that same GUI crate. There is exactly one copy of the
front end, in `cha-gui/ui/`, embedded by every shell that serves it.

This file is the standing rules — the things worth having in mind before any
change. The long-form *why* for each surface lives in [docs/](docs/), and is
worth opening before you work in that area:

| Document | Read it before you… |
|---|---|
| [docs/core.md](docs/core.md) | touch the matcher hot loop, add pattern syntax, change a `Limits` default, or quote a benchmark number |
| [docs/gui.md](docs/gui.md) | change Tauri command threading, add a window or menu item, or regenerate the desktop icon |
| [docs/web.md](docs/web.md) | change an `/api` route, a server-side limit, the Dockerfile, or anything in `deploy/` |
| [docs/mobile.md](docs/mobile.md) | build for a phone, touch `gen/`, or go near release signing and the mobile workflows |
| [docs/versioning.md](docs/versioning.md) | bump the version, or wonder where a version string comes from |

**Keep this file short: every session loads it, whether or not the work touches a
given rule.** The bar for adding here is not "true and useful" but "an agent would
get this wrong without being told, before it knows which files to open".

- **Here:** invariants invisible in the code, commands that must be run, and rules
  about how to *behave* (what to measure, what not to claim). Instructions aimed at
  the agent can't live behind a link, because a link may not be followed.
- **[docs/](docs/):** reasoning, measurements, history, rejected alternatives, and
  checklists that only matter once you are already in that file. Add a trigger to
  the table above, then link to it from the section here.
- **A comment or a test:** anything whose home is next to the code it constrains. A
  regression test pins an invariant better than a paragraph here, and cannot go
  stale silently.

A section growing past a screen is the signal to move its detail into `docs/` and
leave a pointer. Prefer replacing text over appending to it.

## Building and checking

Before committing, run all three and keep them clean:

```
cargo fmt
cargo clippy --workspace
cargo test --workspace
```

`cargo clippy` is treated as required here, not advisory — there are commits
dedicated to keeping it warning-free (e.g. prefer the `?` operator over an
`if x.is_none() { return None }`). `cargo fmt` likewise: the repo is kept
fully rustfmt-formatted, so run it before committing rather than hand-aligning.
The one exception is `perf.rs`'s corpus table, which carries an explicit
`#[rustfmt::skip]` to stay a table; that is not licence to hand-align anything
else.

**`--workspace` does not lint the examples.** `cha-core/examples/` is invisible to
`cargo clippy --workspace`, so a warning there rots silently. When you touch
`perf.rs` or `limitcal.rs`, also run:

```
cargo clippy -p cha-core --examples
```

The workspace has three members (`cha-core`, `cha-gui/src-tauri`, `cha-web`) plus
the CLI crate (`cha`) at the root; `--workspace` covers the libraries — build the
GUI explicitly with `cargo build -p cha-gui` when touching it. `--workspace` now
also pulls `cha-web`'s axum/tokio tree — 87 crates against `cha-core`'s 8 — but
the cost of that is small and worth measuring before working around it: on a
28-core box a from-scratch `cargo build` of the whole chain is ~14 s, of which
axum's tier is ~6 s, and an incremental `clippy --workspace` is under a second.
`tokio`'s features are already narrowed to the five this server uses, so there
is no easy win left; don't trade the check's coverage for its speed.

**`--workspace` is load-bearing on `cargo test`, not decoration.** The root
package is the CLI crate `cha`, which has no tests of its own, so a plain
`cargo test` resolves to that package and cheerfully reports success having run
**zero** tests. All 113 live in `cha-core`. No CI job runs tests either — the
workflows are release-only — so this command is the entire test gate.

**Keep the mobile cross-compiles `-p cha-gui`.** They resolve only that crate's
graph, so `cha-web` is never built for a phone. Generalizing them to
`--workspace --target aarch64-apple-ios` would try to build axum for iOS. The GUI now has a
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
wordlist. Allocation only happens when stripping is actually needed. The
*pattern* side of that test must read the trimmed `&`-parts, never the raw
string: separator whitespace is not content, and a space is one of the three
marks in `PUNCTUATION`, so reading the raw string makes ` & ` silently disable
stripping for the whole query.

## Pattern compilation is fallible

`compile_pattern` returns `Result<Matcher, PatternError>`. All failure modes
(unclosed `[`/`(`, invalid regex, meaningless characters) are detected at
**compile time** — the returned matcher closures never fail. Keep it that way:
the per-word hot path must stay panic- and `Result`-free. The CLI handles the
`Err` (interactive mode re-prompts instead of crashing); the GUI maps it to a
string shown in the UI.

## Every backtracking path needs a bound (`Limits`)

Pattern input is untrusted — even from a local user, a plausible-looking pattern
could hang or OOM the app. There are **three** superlinear paths in `pattern.rs`,
all bounded by [`Limits`](cha-core/src/limits.rs). Do not add a backtracking or
combinatorial path without a ceiling there.

- **`max_anagram_combos` binds at *compile* time**, and that is the one to
  understand. `compile_anagram` materializes the full cartesian product of every
  `[...]` group *before any word is scanned*, so a per-word deadline or a scan
  timeout **cannot** catch it — the check must stay where it is, before the
  product is built, and must use `checked_mul` (a wrapped value slips under the
  cap).
- **`backtrack_limit` and `max_fuzzy_steps` bind per candidate word**;
  `max_results` and `deadline` bind during the scan. Enforcing any of them costs
  nothing measurable — don't "optimize" them away.
- **The match-time defaults are calibrated, not guessed.** Re-run
  [`limitcal`](cha-core/examples/limitcal.rs) before changing either number, and
  read [docs/core.md](docs/core.md) first.

Exceeding a *match-time* limit degrades to "no match", which is what keeps the
hot path `Result`-free. Exceeding the *compile-time* limit is a normal
`PatternError`. **A limit that truncates is a correctness bug**, so prefer
removing redundant work to raising a ceiling.

When adding a limit, test both halves: rejected under tight limits **and**
behaviorally unchanged under the defaults, so a limiter can't silently narrow the
pattern language.

## Performance

`cha` scans the whole word list on every query, so a change to the matcher is a
performance change until measured otherwise. Release baselines on an idle
i7-14700K against the committed 83.6k-word `words.txt`: template `.....` ~1.2 ms,
anagram `;..oting` ~1.8 ms, against targets of < 10 ms and < 20 ms. **Never quote
a timing without its word count** — cost per word is flat, so the denominator is
the whole claim.

Before *and* after any change to matching, pattern compilation, or a `Limits`
default:

```
./scripts/perf.sh --save      # on the "before" build (git stash, build)
./scripts/perf.sh --compare   # on the "after" build
```

It covers every matching path, diffs match counts as well as times, and judges
deltas against a noise floor it measures — re-timing anything it flags, so a
reported regression is one that reproduced. Exit 2 is a timing regression, 3 is
moved match counts — **a changed match count outranks any timing delta**, because
a word that exceeds a per-word limit degrades to "no match" and truncation reads
as a speedup. The run takes ~20 s, so it is **not** a per-commit gate; skip it for
docs, the GUI/web/mobile shells, the front end, and the workflows. New pattern
syntax adds its own entry to `perf.rs`'s corpus in the same commit.
`cha <pattern> -b <N>` is a quick spot check only: it bypasses `search` and
reports a bare mean.

**Don't quote a number you can't trust.** If the suite's verdict is `NOISY` or
`UNRELIABLE`, or the machine is inherently suspect — a cloud sandbox, a shared CI
runner, a CPU-quota container, a remote worktree, a machine under load — say so
plainly, don't present the numbers as a before/after, and give the user the
commands to run locally. Say it *before* doing perf-sensitive work, not after.
Never extrapolate, estimate, or report a figure that wasn't measured.

Two invariants worth carrying without looking them up: **resolve dispatch at
compile time and bake it into the closure** (never branch on pattern syntax
per word), and **keep the length early-out a pure filter** — nothing it drops
could have matched. The rest of the pre-change checklist, the harnesses, and the
gap-run normalization that made star-heavy patterns tractable are in
[docs/core.md](docs/core.md).

## Matches carry optional detail (`MatchInfo`)

`Matcher` is `Box<dyn Fn(&str) -> Option<MatchInfo>>`: `None` means no match.
The `unused`/`extra` letter diffing runs **only on the confirmed-match path**,
after every fast reject has passed — keep it there, or the hot loop regresses.
See [docs/core.md](docs/core.md).

## Versioning: one number, in `[workspace.package]`

**To bump the version, edit `version` under `[workspace.package]` in the root
`Cargo.toml`. That is the only place.** All four crates inherit it with
`version.workspace = true`. In particular `tauri.conf.json` deliberately has
**no** `version` field — don't "helpfully" add it back; that reintroduces a
second source of truth. Android's `versionName`/`versionCode`, the container tag
and release name, and the iOS bundle versions all chain off the crate version or
the git tag. [docs/versioning.md](docs/versioning.md) has the full chain and the
one path that still needs a hand edit.

## Front end: platform and transport are separate questions

- **`platform` is the front end's only source of platform truth.** The command
  returns `"desktop"`, `"mobile"` or `"web"` — those three strings are the only
  values the front end understands. `init()` in
  [`main.js`](cha-gui/ui/main.js) awaits it once at startup and drives every
  gated behavior off it. **Don't** UA-sniff (iPadOS WKWebView reports
  ambiguously) and **don't** infer platform from `@media (pointer: coarse)` —
  that's a touch question, not a platform question.
- **Transport is chosen separately**, in
  [`transport.js`](cha-gui/ui/transport.js), by testing whether
  `window.__TAURI__` is present in *this document*. A desktop browser hitting
  `cha-web` has the HTTP transport and is not a phone; keep the two questions
  apart. The HTTP shim must reject with a **bare string**, not an `Error`.
- **Mobile and responsive CSS is additive by construction.** A rule that needs
  `.mobile` in order to *avoid* breaking desktop is written wrong.

## Adding a command

A command must be added in **both** backends or the front end breaks on one
transport: `generate_handler!` in [`lib.rs`](cha-gui/src-tauri/src/lib.rs) and a
`post()` route in [`main.rs`](cha-web/src/main.rs). Argument names must match
what `main.js` passes. Beware that Tauri camelCases snake_case argument names on
the JS side, so a two-word argument needs `#[serde(rename)]` on the web struct to
keep the two transports speaking one protocol. No current argument has two words.

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
  ceiling in `Limits`, and do not call `cartesian_product` without checking the
  product size first. See the `Limits` section — every such path is reachable
  from untrusted input by a short pattern.
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
