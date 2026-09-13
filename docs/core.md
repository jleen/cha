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

`cha` searches the whole word list on every query, and has ambitions to search
much larger (>10M word) lists. Matching must feel instantaneous on a modern
laptop. The committed `words.txt` is **83,568 words after dedup**; release
baselines measured on an idle i7-14700K against that list:

| Pattern type | Target | Measured |
|---|---|---|
| Template (`.....`) | < 10 ms | ~1.2 ms |
| Anagram (`;..oting`) | < 20 ms | ~1.8 ms |

**A timing without its word count is not a result**, which is why the suite
prints the denominator on every run. Earlier revisions of this file quoted ~5 ms
and ~8 ms against "~270k words". Those figures were not wrong — they simply
stopped describing the committed list. Re-measured on a synthetic 329k-word list
built by suffixing `words.txt`, `;..oting` costs 6.9 ms, essentially where the
old figure put it.

That run is also the best evidence available for the >10M ambition: across a 4×
change in list size, cost per word stayed flat at ~21 ns/word on the anagram
tier. The scan is linear with a small constant, so scaling is a memory-footprint
question (the `Vec<String>` and its dedup `HashSet` at load time) rather than an
algorithmic one. `--words <path>` exists so that can be re-checked against a real
large list without committing one.

### The suite: `cha-core/examples/perf.rs`

```
./scripts/perf.sh --save      # on the "before" build
./scripts/perf.sh --compare   # on the "after" build
```

One command, ~20 s, covering every matching path. `git stash` is still how you
get the "before" build; what the suite replaces is eyeballing two columns of
numbers and hoping. The wrapper forces `--release` and runs from the repo root so
a relative word list resolves consistently; `perf.rs` refuses a debug build
outright. Flags: `--list` (the corpus and what each entry exercises), `--tier`,
`--words <path>` for a larger uncommitted list, `--pattern` for an ad-hoc one,
and `CHA_BENCH_{BACKTRACK,FUZZY,MAX_RESULTS,DEADLINE}` for pricing a candidate
`Limits` default. Exit status: 2 for a timing regression, 3 for moved match
counts. The baseline lives in `target/cha-perf/baseline.tsv` — inside an
already-ignored directory, because it is machine-local by nature.

`limitcal` remains separate and unchanged: it is a correctness-floor calibrator,
not a timer. (`fuzzbench` was folded into `perf`; its `CHA_BENCH_*` knobs and its
fuzzy corpus both live there now. It had drifted to defaulting both match-time
limits to 1_000_000, so its out-of-the-box numbers were never the shipped
product's.)

Both examples are built only by `cargo test` / `--examples` / `cargo run
--example`, never by a plain `cargo build`, so they cost the shipped crates
nothing. Neither pulls a dependency — `cha-core` still has exactly one, and there
are no dev-dependencies in the workspace. Criterion was considered and rejected:
its dependency tree is larger than this entire workspace's, and cross-machine
statistical machinery buys nothing when the protocol is same-machine
before/after.

### Why the suite reports match counts first

A changed match count is printed above the timings, in its own block, and
overrides the verdict. The reason is the gap-run table further down this file:
those patterns were not merely slow. `**********1**********1` returned **9_778 of
25_193** real matches, because a word that exceeds a per-word limit degrades to
"no match" rather than erroring. Truncation and slowness have the same cause and
opposite signatures, so a timing-only harness reads a truncating regression as an
improvement. Demonstrably: with `max_fuzzy_steps` cut to 200, `` *a*b*c*d*`2 ``
gets **53% faster** and loses 4_843 matches.

This is also why only the **word list** blocks a comparison. Differing limits are
reported as context and the diff proceeds — "I lowered a limit, did it truncate?"
is precisely the question the suite exists to answer, so refusing that comparison
would defeat it.

### Why it measures its own noise instead of detecting the machine

Deltas are judged against a probe that re-measures the same cheap pattern several
times and reports the spread. Detecting a slow environment by inspection does not
work: the WSL2 box this was developed on exposes **no** cpufreq governor and
**no** container marker, whether it happens to be a quiet 28-core desktop or a
throttled sandbox. The banner reports what it can see (CPU, visible cores, load,
container markers, governor when readable); the probe is what decides.

Two calibrations inside it, both measured rather than guessed:

- **Samples are amortized to ~20 ms.** Scheduler and timer jitter is a roughly
  fixed number of microseconds, so it is several percent of a 1 ms scan and
  invisible on a 300 ms one. Before amortization an unchanged build showed ±0.2%
  on the expensive patterns and up to **7.3%** on the sub-millisecond ones — a
  false regression on the cheapest and most important path in the crate. Each
  sample now repeats the scan until it has run for `TARGET_SAMPLE`, which
  equalizes precision instead of hiding the problem behind a laxer threshold.
- **The probe measures best-of-N, not individual runs**, because best-of-N is
  what `--compare` diffs and taking a minimum already discards most jitter. The
  spread of single runs overstates the real floor by roughly an order of
  magnitude and classified a perfectly quiet machine as unreliable. Best of 10
  halves the spread versus best of 6; best of 16 does not improve on it.

Post-calibration, an unchanged build reproduces within **~0.9% mean, ~3% worst
case** within a single pair of runs, against a signal threshold of ~5%.

**Across** runs it is looser than that, and the in-process probe cannot see it.
Measured over six comparisons of identical code, the worst per-pattern delta was
usually 4–5% but reached 9.7% once, and the pattern that drifted differed every
time — separate processes get different memory layouts and cache states, so
cross-run variance genuinely exceeds within-run variance. Raising the threshold
past 10% would have cost real sensitivity on the sub-millisecond patterns, where a
genuine regression is also only a few percent.

So `--compare` **re-measures whatever it flags** and reports only what reproduces
in direction and magnitude. Transient jitter does not survive a second look; a
real change does. This costs almost nothing because only a handful of patterns are
ever flagged, and it is what makes the verdict trustworthy: six identical-code
comparisons now come back clean six times (one raised a candidate, correctly
dismissed as jitter), while removing the length early-out still confirms all six
star-free template patterns as regressions.

### The length early-out

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
early-out takes a star-free fuzzy scan from ~18 to ~12 ns/word.

Any change here must stay a *pure filter*: nothing it drops could have matched.
Disabling the regex one costs **15–56%** across the star-free `template` tier and
leaves `*ing`, `un*ed` and `*a*e*i*o*` (where `fixed_len` is `None`) untouched,
with every match count unchanged — which is both what "pure filter" means and a
good way to confirm the suite is wired up correctly.

### Debug builds

These are laptop **release** numbers. A phone CPU runs the hot loop maybe 2–3×
slower, still comfortably interactive behind the 100 ms debounce. **Debug builds
are the real trap:** unoptimized, the matcher is 10–50× slower and the mobile app
looks broken — and `tauri {ios,android} dev` builds debug by default. The root
`Cargo.toml` therefore forces `[profile.dev.package.cha-core] opt-level = 3`
(leaf crate, negligible compile-time cost) so even dev builds have a usable
matcher. Keep that profile; still prefer `--release` for any real timing.

`cha <pattern> -b <N>` is still useful as a quick single-pattern spot check, but
know what it is: it times the matcher closure directly, bypassing `search` (so no
chunking, no `max_results`, no `MatchRow` allocation), and reports a bare mean
with no warmup. It now uses the *checked* compile, so a contentless pattern says
so instead of benchmarking a no-op matcher, and it prints the word count. For
anything you intend to quote, use the suite.

## Before you add pattern syntax

Every item here is an invariant the current code already keeps. The hot loop runs
once per word per query, so the reject path is what matters — the confirmed-match
path is comparatively rare, which is why `diff_letters` can afford to allocate
and the reject path cannot.

- **Resolve dispatch at compile time and bake the result into the closure.**
  Never branch on pattern syntax per word. `has_punct`, `fixed_len`,
  `combo_pools`, `is_pure` and `collapse_gap_run` are all this pattern; syntax
  that re-inspects itself inside the closure is the likeliest way to regress.
- **Decide which of the four engines the new syntax joins** — the regex
  template, the fuzzy tokenizer, the anagram pool, or the structural matcher —
  and if it can't join one, reject it *there* at compile time rather than
  half-supporting it. Precedent: digit variables are refused on the fuzzy path
  because backreferences don't compose with a mismatch budget, and fuzz is
  refused on the structural path because a per-node mismatch budget doesn't
  compose with a split search.
- **A new fuzzy token that consumes anything other than exactly one byte
  invalidates two arguments at once**: `toks.len()` as the exact match length,
  and the reason the fuzzy early-out needs no `is_ascii` guard. Audit both, or
  the early-out stops being a pure filter.
- **New regex-template syntax: check it still yields a usable `fixed_len`, and
  whether it pushes fancy-regex off the linear `regex` engine.** Stars are not
  the hazard; backreferences are. If it reaches the backtracking VM, re-run
  `limitcal` and expect the floors to move.
- **Any new gap-like symbol must join `collapse_gap_run`,** or it becomes a fresh
  way for a user to rebuild the pathological shape by accident.
- **Order checks cheapest-and-most-selective first,** and keep anything that
  allocates behind every fast reject.
- **Any new combinatorial expansion needs a `Limits` ceiling checked with
  `checked_mul` before the product is built** — a compile-time blowup is
  unreachable by a deadline.
- **New structural syntax owes the engine a length bound.** Every `Tok` and
  every `Node` reports `(min, max)` bytes, and those bounds are the only reason a
  composition of fixed-length elements costs a walk rather than a search. A token
  that reports looser bounds than it needs is not wrong, just slow; one that
  reports *tighter* bounds than it needs silently loses matches.
- **Add the new syntax to `perf.rs`'s corpus in the same commit.** This is what
  keeps the suite honest as the language grows.

The first bullet and the `fixed_len` one bite hardest, and the suite shows you
both distinctly: a per-word branch on pattern syntax appears as a uniform
slowdown across a whole tier, while a broken length early-out appears on exactly
the patterns whose `fixed_len` is `Some` and nowhere else.

## Subpatterns: the structural engine

`(...)` in the *template* half is a **subpattern**: a whole pattern applied to a
contiguous slice of the word, with the slices laid end to end covering all of it.
In the *pool* half `(...)` keeps its long-standing "contains this substring"
meaning; the two never collide, because they sit on opposite sides of the `;`.

This needed a fourth engine because a subpattern is not a regular constraint.
`(;oif)(;bel)` asks for the word to be *cut* and each piece handed to a
sub-pattern, and an anagram over a piece is not something a DFA or a
backreference can express. `compile_structural` compiles the part to a tree of
`Node { toks, pool }` and `walk`/`match_node` match it.

**Entry is gated on syntax that used to be a hard error**, so nothing written
against the older language can be rerouted here: `needs_structural` requires a
`(` in the template half, or a digit in the pool half. Everything else reaches
exactly the engine it always did — which is what the `template`, `backref`,
`fuzzy`, `anagram` and `hybrid` tiers of `perf.rs` are there to confirm.

Three properties make it cheap:

- **Every element's length is bounded at compile time.** Each `Tok` reports
  `(min, max)` bytes, `suffix_min`/`suffix_max` accumulate those right to left,
  and `walk` prunes on them *before* looking at a token. When every element is
  fixed-length — essentially every pattern a person writes — the cut points are
  forced and there is no search, just a walk. `(;oif)(;bel)` costs 1.10 ms,
  indistinguishable from the bare `.....` template.
- **The same bounds give the top-level length early-out.** A star-free
  composition matches exactly one length, so the usual integer compare rejects
  most of the list before the walker runs at all.
- **A group with no `;` is spliced away, not compiled into a block.** The parens
  in `ele(ph)ant` constrain nothing that `elephant` doesn't, so they cost
  nothing. Nested groups inside a spliced one are still found.

**The engine is ASCII-only**, like the fuzzy one, and that is load-bearing rather
than incidental: with an ASCII-only pattern no token can consume a non-ASCII
byte, so byte offsets are char offsets, every slice it takes is on a char
boundary, and the length early-out is exact. This costs nothing in practice — the
regex path's `*` is `[a-z]*` and rejects the same words. `slice_str` still checks
`from_utf8`, so a cut landing mid-character is a non-match rather than a panic.

### Absorption: which letters excuse the outer pool

When a subpattern *and* the whole pattern both carry an anagram, the rule is:

> **A letter absorbs iff it occurs in a template position — before a `;` — at any
> nesting depth. A letter in any pool, at any depth, never absorbs.**

`template_literal_letters` implements it, and the arithmetic it feeds
(`Pool::check_combo`'s hybrid arm) is unchanged from before subpatterns existed.
Against FOIBLE, with no wildcards, the acceptance rule reduces to *the word uses
only pool letters, or it uses all of them*:

| pattern | pool | extra (word−pool) | unused (pool−word) | |
|---|---|---|---|---|
| `...(;bel);oif` | `o i f` | `b l e` | — | match |
| `(;oif)(;bel);oifb` | `o i f b` | `l e` | — | match |
| `(;oif)(;bel);oifblex` | `o i f b l e x` | — | `x` | match |
| `(;oif)(;bel);oifblx` | `o i f b l x` | `e` | `x` | **no** |
| `(;oif)(;bel);x` | `x` | 6 letters | `x` | **no** |

The last two are what pin the rule down: were the `e` that `(;bel)` spends
allowed to absorb, `;oifblx` would match too. A literal is still a literal
wherever it sits, so the `f` of `(f..;oif)` absorbs exactly as the `c` and `t` of
`c.t;ao` do. Digit variables are not letters and never absorb, the same as `.`.

### Variables across a cut

A digit is one variable across the whole part, not one per regex. `walk` threads
an `Env` (`[u8; 10]`, 0 = unbound) left to right and **by value**, so a failed
branch is abandoned without unwinding a binding trail. That is what lets
`(1234)(;1234)` bind four letters over REAP and then require the rest of the word
to be an anagram of those same four — matching REAPPEAR.

It also makes a digit meaningful *inside* a pool, where it was previously
`"Anagram has meaningless character"`. `Pool::vars` records the slots, and
`Pool::check` folds the bound letters into a copy of the combo counter — only
when `vars` is non-empty, so the var-free path still hands the pre-computed
counter straight through.

**Bind-before-use is decided at compile time** by `check_variable_binding`,
walking the tree in the same left-to-right order the matcher does. That keeps the
per-word path free of an "unbound variable" case it would otherwise carry
forever.

### What it deliberately refuses

- **Fuzz (`` `N ``) anywhere in a part containing a subpattern.** A per-node
  mismatch budget composing with a split search and a variable environment is a
  design question of its own; erroring is honest and can be lifted later.
- **`&` and `!` inside `(...)`.** Both are whole-query operators, and the `&`
  split is textual — without the check in `reject_operators_in_subpattern`,
  `(a&b)` would be torn in half and surface as a baffling `"Unclosed '('"`.
- **Non-ASCII pattern characters**, per the ASCII argument above.

## Every backtracking path needs a bound (`Limits`)

Pattern input is untrusted — even from a local user, a plausible-looking pattern
could hang or OOM the app. There are **four** superlinear paths in `pattern.rs`,
all bounded by [`Limits`](../cha-core/src/limits.rs), whose `Default` is generous
enough that no hand-typed pattern reaches it. `compile_pattern`/
`compile_pattern_checked` use the defaults; `compile_pattern_with`/
`compile_pattern_checked_with` take explicit limits, which is how a server passes
tighter ones.

**One struct, split by phase — not by module.** `Limits` lives in its own module
and carries all eight ceilings, including the two the *scan* consults
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
- **`max_structural_steps` — match-time, per word.** Bounds `walk`/`match_node`
  on the subpattern path, where two sources of branching are layered: the `Star`
  arm recurses twice per node, and the `Sub` arm tries every cut offset its
  length bounds allow. Those bounds are what keep it cheap — a composition whose
  elements all have a fixed length never branches at all, so this binds only on
  shapes like `*(12)*(;12)*(;12)*`. The variable environment is passed **by
  value** (ten bytes), so abandoning a branch costs nothing and there is no
  binding trail to unwind; depth is bounded by tokens + word length, as on the
  fuzzy path. It is the node count, and only the node count, that needed a cap.
- **`max_subpattern_depth` — compile-time.** Caps `(...)` nesting, and so the
  depth of `compile_node`'s recursion. Checked once by `check_nesting_depth`
  before anything recurses, which is why the recursive compile helpers can skip
  carrying a depth counter. It counts *all* parens, including the ones with no
  `;` that get spliced away — they cost the same compile-time recursion.
- **`max_results` and `deadline` — scan-time, and both effectively free.**
  `max_results` is one integer compare per *match* (not per word), on a path
  already allocating a `MatchRow`; it removes work rather than adding it.
  `deadline` is checked once per `DEADLINE_CHECK_INTERVAL` (4096) words, and
  measured against the cheapest possible scan (~12 ns/word) a never-firing
  deadline is indistinguishable from `None`. Do **not** move it per-word.

**Enforcing the match-time limits costs nothing measurable.** `backtrack_limit`
only picks the threshold `fancy-regex` compares against — it increments its
counter either way. `max_fuzzy_steps` was A/B'd against a build with the counter
deleted outright (the timing harness now in
[`perf`](../cha-core/examples/perf.rs)); the counted
build came out a wash or slightly *faster* across every pattern and every round,
the difference being codegen noise. Don't "optimize" either one away.

**The match-time defaults are calibrated, not guessed.**
[`limitcal`](../cha-core/examples/limitcal.rs) binary-searches, per pattern, the
smallest limit that still returns every match an unlimited one finds (both limits
are monotone, so this is well-defined). Realistic patterns need very little — the
worst are `*1*1` at 193 steps and `` *a*e*`1 `` at 396 — and a deliberately
adversarial tier tops out at 1_315 and 3_698. `max_structural_steps` is
calibrated the same way and on the same scale: realistic subpattern shapes top
out at 51 steps (`*(;ing)*`) and the adversarial tier at 329
(`*(12)*(;12)*(;12)*`), against a default of 5_000. The defaults are ~15x that
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
