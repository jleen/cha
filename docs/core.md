---
author: Claude (Anthropic)
ai-generated: true
---

> **Provenance:** Written by Claude (Anthropic) while doing the work it
> describes. Reviewed in the normal course of review, but not audited line by
> line — treat specific numbers, paths, and version pins as claims to verify
> rather than guarantees.
>
> See [docs/README.md](README.md).

# Core matcher (`cha-core`)

Performance targets, the limit calibration behind `Limits`, and the optional
per-match detail. The standing rules distilled from this file live in
[AGENTS.md](../AGENTS.md).

## Performance requirements

`cha` searches ~270k words per query, and has ambitions to search
even larger (>10M words) lists. Matching must be fast enough to feel
instantaneous on a modern laptop. Current release-build baselines:

| Pattern type | Target | Achieved |
|---|---|---|
| Template (e.g. `qu...`) | < 10 ms | ~5 ms |
| Anagram (e.g. `;..oting`) | < 20 ms | ~8 ms |

Two extra harnesses live in [`cha-core/examples/`](../cha-core/examples/) and are
built only by `cargo test`/`--examples`, never by a plain `cargo build`, so they
cost the shipped crate nothing: [`fuzzbench`](../cha-core/examples/fuzzbench.rs)
times a full scan per pattern in ns/word (both match-time limits overridable via
`CHA_BENCH_*` env vars, for pricing a candidate default), and
[`limitcal`](../cha-core/examples/limitcal.rs) derives the smallest limit each
pattern actually needs. Both read `words.txt` from the working directory.

**Both template paths reject on length first.** A star-free template matches
exactly one length, and on a large list the overwhelming majority of words are
the wrong length — so an integer compare replaces the match for most of the scan.
The regex path compares against `template_to_regex`'s `fixed_len`, and must keep
its `is_ascii` guard (byte length only bounds char count from below). The fuzzy
path compares against `toks.len()` and needs **no** such guard, and the reason is
worth preserving: `fuzzy_match` is byte-indexed, every token but `Star` consumes
exactly one byte, and `tokenize_fuzzy` rejects non-ASCII templates — so a word
carrying a multi-byte char cannot match at any length. Fuzz does not widen this
either; the budget lets a position *mismatch*, never disappear. Measured, the
early-out takes a star-free fuzzy scan from ~18 to ~12 ns/word. Any change here
must stay a *pure filter*: nothing it drops could have matched.

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

## Every backtracking path needs a bound (`Limits`)

Pattern input is untrusted — even from a local user, a plausible-looking pattern
could hang or OOM the app. There are **three** superlinear paths in `pattern.rs`,
all bounded by [`Limits`](../cha-core/src/limits.rs), whose `Default` is generous
enough that no hand-typed pattern reaches it. `compile_pattern`/
`compile_pattern_checked` use the defaults; `compile_pattern_with`/
`compile_pattern_checked_with` take explicit limits, which is how a server passes
tighter ones.

**One struct, split by phase — not by module.** `Limits` lives in its own module
and carries all six ceilings, including the two the *scan* consults
(`max_results`, `deadline`). It was previously two nested structs, `CompileLimits`
inside `SearchLimits`, which implied a compile/scan split that the fields do not
actually follow: `backtrack_limit` and `max_fuzzy_steps` sat in `CompileLimits`
but bind **per candidate word**. The distinction that matters to a caller is
*when a limit binds*, and that cuts across both modules, so the doc comments —
not the type — carry it. `compile_pattern_checked_with` ignores the two scan-time
fields, which is cheaper than making every caller build a nested struct.

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
- **`backtrack_limit` — match-time, per word.** Bounds `fancy-regex` on the
  non-fuzzy template path. It binds far more narrowly than it looks: a template
  with **no digit variables** compiles to something `fancy-regex` hands straight
  to the linear `regex` crate (`RegexImpl::Wrap`), which never backtracks. Stars
  alone are therefore *not* the hazard — measured, `**********cat` and
  `*a*e*i*o*` both run correctly with `backtrack_limit` set to **1**, at the same
  ~22 ns/word as everything else on that path. Backreferences are what reach the
  backtracking VM, and stars *combined* with them are what go exponential — but
  only stars that survive collapsing (see below), i.e. alternating stars with
  *distinct* backreferences: `*1*2*1*2*` needs ~1_315 steps, and
  `*1*2*3*4*1*2*3*4*` is still budget-bound at the default (~3 s per scan, 566
  matches at 20_000 vs 579 at 200_000). Cite that shape, not a star-only one,
  when explaining why this limit exists.
- **`max_fuzzy_steps` — match-time, per word.** Bounds `fuzzy_match`, the
  hand-rolled backtracker on the fuzzy path. Note its `budget` parameter is the
  *fuzz allowance*, a different quantity — don't overload it. Depth was never the
  exposure either; the `Star` arm branches twice per node, and it is the node
  count that was unbounded.
- **`max_results` and `deadline` — scan-time, and both effectively free.**
  `max_results` is one integer compare per *match* (not per word), on a path
  already allocating a `MatchRow`; it removes work rather than adding it.
  `deadline` is checked once per `DEADLINE_CHECK_INTERVAL` (4096) words, and
  measured against the cheapest possible scan (~12 ns/word) a never-firing
  deadline is indistinguishable from `None`. Do **not** move it per-word.

**Enforcing the match-time limits costs nothing measurable.** `backtrack_limit`
only picks the threshold `fancy-regex` compares against — it increments its
counter either way. `max_fuzzy_steps` was A/B'd against a build with the counter
deleted outright ([`fuzzbench`](../cha-core/examples/fuzzbench.rs)); the counted
build came out a wash or slightly *faster* across every pattern and every round,
the difference being codegen noise. Don't "optimize" either one away.

**The match-time defaults are calibrated, not guessed.**
[`limitcal`](../cha-core/examples/limitcal.rs) binary-searches, per pattern, the
smallest limit that still returns every match an unlimited one finds (both limits
are monotone, so this is well-defined). Realistic patterns need very little — the
worst are `*1*1` at 193 steps and `` *a*e*`1 `` at 396 — and a deliberately
adversarial tier tops out at 1_315 and 3_698. The defaults are ~15x that
adversarial worst case. They were **1_000_000 apiece**, which bounded nothing
useful: `` **********cat`1 `` took **66 s** for one scan under that ceiling while
finding all of its real matches within 57 steps. Cost is linear in the limit, so
lowering it was nearly free in correctness and worth ~8x in time. Re-run
`limitcal` before changing either number — note the floors are set by
`*1*2*1*2*` and `` *a*b*c*d*`2 ``, whose digits break the gap runs for real, so
the normalization below did not move them.

**Runs of `.` and `*` are normalized, and that is the real fix for star-heavy
patterns.** A maximal run of gap symbols with k dots and at least one star
accepts exactly the words of length >= k, *whatever the interleaving* — each `.`
contributes one letter, each `*` zero or more, and one star absorbs the surplus.
So the run rewrites to `.`xk then a single `*`. Both symbols are letter-only on
both paths (`[a-z]`/`FuzzTok::Any` and `[a-z]*`/`FuzzTok::Star`) and neither
consumes fuzz budget, so the rewrite is exact. Both template paths call
`collapse_gap_run` from their `*` arm; the anagram path already folds `.` into a
count and `*` into a `has_star` bool, so it is order-independent already.

Two things to preserve here. **Do it at the parsed level, never by
string-rewriting the raw pattern** — a `*` or `.` inside a `[...]` class is a
class member, and `c[a.b]t` must keep matching exactly three characters. And
**handle `.` alongside `*`, not just `**`**: a star-only collapse is trivially
defeated by sprinkling dots between the stars, which is exactly how a user
rebuilds the pathological shape by accident.

The payoff dwarfs the limit tuning:

| pattern | before | after |
|---|---|---|
| `` **********cat`1 `` | 7.9 s | 8.7 ms |
| `**********1**********1` | 24.7 s, 9_778 matches | 139 ms, 25_193 matches |
| `` *.*.*.*.*.*.*.*.*.*cat`1 `` | 686 ms | 3.4 ms |
| `*.*.*1*.*.*1` | 2.9 s, 14_333 matches | 78 ms, 14_386 matches |

Note the match counts: those patterns were exceeding a per-word limit and
degrading the over-budget words to "no match", silently returning a fraction of
the real result. **A limit that truncates is a correctness bug**, so removing the
redundant work is strictly better than raising the ceiling. `test_every_gap_run_
normalizes_exactly` brute-forces every interleaving up to length 5 against the
normalized form; keep it if you touch this.

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
the right of each word ([`cha-gui/ui/main.js`](../cha-gui/ui/main.js),
`.word-annot` in [`styles.css`](../cha-gui/ui/styles.css)); the CLI ignores it and
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

---

Back to [AGENTS.md](../AGENTS.md).
