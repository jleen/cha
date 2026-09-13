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

### What people actually type, and what that means for triage

Three shapes dominate real use. When a change costs something, it matters far
more where the cost lands than what the corpus mean says:

| Shape | Example | Priority |
|---|---|---|
| Dotted, no anagram | `.....`, `..o..e.`, `c.t` | **highest** |
| Dotted plus anagram (hybrid) | `........;gdangboot` | **highest** |
| Pure anagram | `;..oting`, `;obelisk` | **highest** |
| Starred | `*ing`, `*a*e*i*o*` | secondary |
| Fuzz, digit variables, subpatterns | `` cathode`1 ``, `1221`, `(;oif)(;bel)` | secondary |

A regression on a starred or fuzzy pattern is worth accepting for a real gain
elsewhere. A regression on the first three is worth working to avoid — they are
what a crossword solver types all day, and they are also the cheapest scans in
the crate, so a fixed per-word cost shows up there as the largest percentage.
`perf.sh --compare` reports per pattern precisely so this triage is possible;
don't summarize it to a single number.

### Performance log

Percentages, not milliseconds, and **always measured against an interleaved
same-session baseline** (`git stash` → build → `--save` → `git stash pop` →
build → `--compare`). Absolute times drift by several percent between runs on an
idle machine and by much more on a busy one; a baseline taken an hour earlier has
been mistaken for a regression twice in this file's history. Percentages between
consecutive milestones are the only numbers here that keep their meaning.

One row per commit that moved the needle, end to end. If a change is a wash,
say so — the point of the log is to catch *creep*, which is invisible one
commit at a time.

| Milestone | Dotted | Dotted+anagram | Pure anagram | Starred | Other |
|---|---|---|---|---|---|
| Subpatterns (`8a2fd60`) | — | — | **−7 to −9%** | — | — |
| Two engines (`a8d6cad`) | — | — | — | — | backref **−87%**, pathological **−68%**, cheap fuzz **+8-10%** |
| Unicode folding | **+7-9%** | **+10-14%** | **+2-4%** | +9-13% | fuzz/digit/subpattern +3-4% |
| Structural engine goes Unicode | **+4-5%** | **+3-10%** | **wash** | — | fuzz +5%, backref +4-5%, subpattern +2-3% |

**Ranges, because point values here are false precision.** Two interleaved
same-session runs of the same two builds, 25 reps each on an idle machine,
disagreed by about 3 percentage points per group — `+7.4%` and `+8.8%` for the
dotted group, `+10.3%` and `+13.9%` for the hybrid. That is the resolution this
harness actually has for a change of this size. Quote a range, take at least two
runs before believing a number worth recording, and treat a single run's figure
as a hypothesis. It follows that a *small* regression cannot be attributed to a
*specific* line: when a targeted fix moved the hybrid group from ~15% to ~10-14%,
that overlapped the noise band, and the honest reason to keep it was that it does
strictly less work per word, not that the suite proved it.

Notes on each:

- **Subpatterns** added a matching engine but changed no existing path. The
  anagram gain is incidental: extracting `Pool` moved the `(...)` substring test
  off the per-combination path and onto the confirmed-match path where it belongs.
- **Two engines** retired the fuzzy matcher and moved digit variables off the
  regex, which is where the large gains come from — `backref` went 403 ms to
  53 ms as a tier. The cost was 8-10% on *cheap* fuzzy patterns, accepted
  deliberately: fuzz is an uncommon shape, and merging deleted a duplicated
  engine. See the table in "Two engines" below.
- **Structural engine goes Unicode** paid for consistency, and the secondary
  tiers it owns came in under the 10% budget set for them. Pure anagram is a
  wash, as it should be — it runs on a different engine. The row worth
  questioning is **dotted at +4-5%**, which reproduced across three interleaved
  runs on a code path this change does not touch at all: the regex template
  matcher is byte-identical before and after. Halving the walker's code (the enum
  experiment below) did not move it, which rules out the obvious explanation, so
  the remaining candidate is codegen and inlining shifting as the crate changes.
  Recorded rather than explained. `dotted+anagram` is one pattern and behaved
  like it — +3%, +3%, +10% — so treat that cell as the least trustworthy in the
  table.
- **Unicode folding** is a uniform tax from carrying two forms per word, not one
  effect. Four causes were found and fixed before it came down from +165%; see
  "What it cost, and where". What remains is within budget — the highest-priority
  shape sits at ~1.25 ms against a 10 ms target — but it is the row to watch,
  because it is the first entry here that made the common cases slower. The
  largest single lever left, if it ever needs one, is that `Word` is 32 bytes
  against the 24 a `String` took: the scan touches every one of them, and a
  24-byte packing (one buffer, an offset) costs a bounds check on the hot
  accessor in exchange. That trade was not measured conclusively either way.

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
improvement. Demonstrably: with `max_structural_steps` cut to 200,
`` *a*b*c*d*`2 `` gets **53% faster** and loses 4_843 matches.

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
- **Decide which of the two engines the new syntax joins** — `regex` or the
  structural walker (the anagram `Pool` is a component of the second, not a third
  engine) — and if it can't join either, reject it *there* at compile time rather
  than half-supporting it. Precedent: fuzz is refused alongside a digit variable
  in the same node, because it is not clear whether a fuzzed position should
  still bind.
- **A new `Tok` that consumes anything other than exactly one byte invalidates
  two arguments at once**: the length bounds `Node::rigid` and the suffix table
  are computed from, and the reason byte offsets are char offsets on this engine.
  Audit both, or the length early-out stops being a pure filter.
- **New regex-template syntax: check it still yields a usable `fixed_len`, and
  that it stays inside what `regex` accepts.** Anything needing a backreference
  or look-around belongs on the structural engine — putting it back in front of
  `regex` would be a compile error at best and a reinstated `fancy_regex`
  dependency at worst.
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

## Unicode: canonicalize for matching, preserve for display

Words and patterns are both matched in the canonical form
[`fold`](../cha-core/src/fold.rs) produces — case and diacritics removed, plus a
small table for the Latin letters Unicode gives no decomposition — and the
*original* spelling is what comes back. `dictionary::Word` carries both forms.

Design notes worth keeping:

- **Fold both sides or neither.** `compile_pattern_checked_with` folds the
  pattern once, up front. Everything downstream then compares like with like, and
  no per-word normalization exists to go wrong.
- **The table is a transcription, not a judgement.** `MULTIGRAPHS` was checked row
  by row against CLDR's `Latin-ASCII` transform with `uconv`. Reaching for a
  transliteration crate instead is the trap: those romanize *everything* and turn
  `omega` into `o`, destroying the rule that a non-Latin letter is its own letter.
- **Length is counted in canonical letters.** `Ærø` is four and `Straße` is seven.
  That falls out of the fold rather than being chosen, and for a word puzzle it is
  arguably right anyway — German crosswords already spell `ß` as SS.
- **Dedup is keyed on the display form.** `elan` and `élan` fold together but are
  different words, and a search matching one should return both.

### What it cost, and where

Match counts on `words.txt` — 100% ASCII — are unchanged, which is the proof the
fold is a no-op for anyone not using it. The time was not free, and four separate
causes had to be found before it came down from **+165%** to +7-22%:

| cause | symptom | fix |
|---|---|---|
| `\p{Alphabetic}` compiled per query | +165% on `.........` | build the wide class **lazily** — `search` recompiles the pattern on every call, so compilation is hot in a way match time usually hides |
| `Word::folded` crossing a crate boundary | uniform +15%, every tier | `#[inline]` |
| `Histogram` zeroing 336 bytes per word | +13% on the anagram tier | `u32` counters: 168 bytes, *less* than the `[usize; 26]` it replaced |
| the non-ASCII test in front of the common case in `tally` | +22-46% on hybrids | test `is_ascii_alphabetic` first, as the original loop did |

What remains is worst on starred templates (`*ing`, `*a*e*i*o*`), where there is
no length filter to reject a word before the matcher must ask whether it is ASCII.
A superset class cheap enough to answer without asking was tried —
`[a-z\x{80}-\x{10FFFF}]` — and is not available: it made `*` 2.2x and `*a*` 3.5x
*slower*, because spanning every non-ASCII scalar costs the engine more than the
scan it saves. Absolute numbers stay far inside the targets at the top of this
file: `.....` at 1.23 ms against a 10 ms budget.

**The perf suite now has a `unicode` tier.** It had no non-ASCII coverage at all
before, which is why it could not have caught the `(;glo)` bug and would not have
caught a folding regression either.

## Two engines, and why the boundary is where it is

`needs_structural` sends subpatterns, digit variables and `` `N `` fuzz to the
structural walker; everything else goes to `regex`. That split is a measured
result, not an accident of history, and the measurements are worth keeping
because the obvious simplification — one engine — is wrong in one direction and
right in the other.

The A/B is exact and costs no code: wrapping a pattern in parens forces it onto
the structural engine with identical semantics, because a group with no `;` is
spliced rather than compiled into a block. So `(c.t)` is `c.t` on the other
engine, and match counts must agree (they did, across ~25 patterns).

| shape | `regex` | structural | |
|---|---|---|---|
| fixed-length, no star, no digit | 0.95-1.17 ms | 0.90-1.16 ms | wash |
| star, no digit | 1.72-2.09 ms | 3.74-5.44 ms | **regex, 1.9-2.9x** |
| digit, no star | 1.05-3.05 ms | 0.89-1.08 ms | **structural, 1.2-2.8x** |
| star + digit | 25-1972 ms | 4.6-57 ms | **structural, 5.4-37x** |

`fancy-regex` paid for itself only while it stayed on the linear `regex` crate
(`RegexImpl::Wrap`), which is exactly the no-digit case. One digit dropped it onto
the backtracking VM, and there a plain array store wins every time — so digit
variables moved, `backtrack_limit` stopped bounding anything, and the dependency
went with it. Stars without digits stay on `regex`, where the literal and DFA
machinery is ahead by 1.9-2.9x and the walker has no answer; that is the half of
the redundancy worth keeping.

The fuzzy tokenizer was the other half, and it *was* redundant: `Tok` was
`FuzzTok` plus `Var` and `Sub`, and `walk` was `fuzzy_match` plus an environment
and a cut. Merging cost the cheap fuzzy patterns 8-10% and the starred ones ~16%,
and paid that back on the expensive ones (`` **********cat`1 `` -18%,
`` *.*.*.*.*.*.*.*.*.*cat`1 `` -23%) because the walker's length bounds prune
what `fuzzy_match` had to explore. Worth knowing before optimizing: the per-node
overhead the merge added is what `Node::rigid` exists to skip.

Net across the whole corpus: **700 ms to 221 ms**, with `backref` going 403 to 53
and `pathological` 183 to 59.

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

**The engine indexes characters, not bytes**, and that is the difference between
this and the ASCII-only walker it grew out of. Every `Tok` holds a `char`, the
`Env` binds a `char`, and `Node::min`/`max`/`suffix` count characters — the unit
the `(;glo)` bug was a confusion about, so it is worth saying twice.

Words come in two shapes and the walker is generic over which:

- `AsciiText` addresses an all-ASCII word by byte offset, every method a load.
  That is the shipped word list entirely and over 99% of a large supplementary
  one after folding, so it is the path that has to stay free.
- `WideText` addresses anything else by character index, built on the stack
  (`WIDE_STACK_CHARS`) so the rare path allocates nothing either.

`Text::slice` gives the anagram pool its `&str` back and **cannot** land inside a
character, which is a stronger guarantee than the `from_utf8` check it replaced.

**Generic, not an enum, and that was measured.** An enum with one copy of
`walk`/`match_node` plus a discriminant check is less code and reads as the
simpler choice; it cost the subpattern tier 7.5% against the generic version's
2.1%, worst case 29% against 8%, and helped nothing elsewhere. Monomorphizing
keeps the ASCII instantiation equal to the byte walker that preceded it.

### The one place non-ASCII does not just work

A digit variable *spent inside an anagram pool* — `(1234)(;1234)` — binds at
match time, and a `Pool`'s histogram slots are allocated at compile time from the
letters the pattern spells out. There is nowhere to count a non-Latin letter the
pattern could not name in advance, so `Pool::check` declines rather than
miscounting.

The alternative needs slots assigned per word, and then two digits that bind the
*same* letter have to be detected and merged or the counts drift silently — a
worse failure than not matching. `(1234)(;1234)` keeps working for Latin, which
is what it is for. Pinned by
`test_pool_variable_binding_a_non_latin_letter_is_the_documented_hole`.

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

### Fuzz, now that it lives here

`` `N `` is a per-`Node` budget, armed by `match_node` and spent only by `Lit`
positions — so it means what it always meant for a paren-free template, and
composes with a subpattern the obvious way: the block is rigid, like `.` or
`[abc]`, and the literals around it can vary. `ele(;nahpt)`1` matches *alephant*
and *elephant* but not *elephxnt*. That combination used to be a hard error, and
it is the one restriction the merge lifted rather than preserved.

Two it did **not** lift, both preserved exactly:

- **Fuzz and a digit variable in the same node.** It is not clear whether a
  fuzzed position should still bind, and guessing is worse than erroring.
- **Fuzz and an anagram pool in the same node**, which `compile_anagram` refused
  before there were nodes.

### What it deliberately refuses

- **`&` and `!` inside `(...)`.** Both are whole-query operators, and the `&`
  split is textual — without the check in `reject_operators_in_subpattern`,
  `(a&b)` would be torn in half and surface as a baffling `"Unclosed '('"`.

Non-ASCII pattern characters used to be on that list. They are not any more — a
letter is a letter here in whatever script it arrives in.

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
actually follow: the per-word ceilings sat in `CompileLimits` despite binding
**per candidate word**. The distinction that matters to a caller is
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
- **`max_structural_steps` — match-time, per word.** Bounds `walk`/`match_node`
  on the subpattern path, where two sources of branching are layered: the `Star`
  arm recurses twice per node, and the `Sub` arm tries every cut offset its
  length bounds allow. Those bounds are what keep it cheap — a composition whose
  elements all have a fixed length never branches at all, so this binds only on
  shapes like `*(12)*(;12)*(;12)*` and `` *a*b*c*d*`2 ``. Depth is bounded by
  tokens + word length, and the two things a failed branch has to undo (a
  variable binding, an appended `MatchInfo` fragment) are each restored in place
  by the one arm that writes them. It is the node count, and only the node count,
  that needed a cap.
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

**Enforcing the match-time limit costs nothing measurable.**
`max_structural_steps` was A/B'd against a build with the counter deleted
outright (the timing harness now in [`perf`](../cha-core/examples/perf.rs)); the
counted build came out a wash or slightly *faster* across every pattern and every
round, the difference being codegen noise. Don't "optimize" it away.

**The match-time default is calibrated, not guessed.**
[`limitcal`](../cha-core/examples/limitcal.rs) binary-searches, per pattern, the
smallest limit that still returns every match an unlimited one finds (the limit
is monotone, so this is well-defined). Realistic patterns need very little — the
worst are `` *a*e*`1 `` at 386 steps and `*(;ing)*` at 51 — and the adversarial
tier tops out at 3_053 (`` *a*b*c*d*`2 ``). The 50_000 default is ~16x that, and
also clears 17_821, which is what `*1*2*3*4*1*2*3*4*` needs; covering *that* one
is a correctness claim rather than headroom, because the old regex path returned
566 of its 579 matches and could not be made to return the rest at any practical
ceiling. Patterns on the regex path are still in the corpus and are expected to
report a floor of 1 — a floor above 1 for one of them means something has been
rerouted by accident. Re-run `limitcal` before changing the number.

**The calibration is against `words.txt`, and a much larger dictionary can push
past it.** The default clears every pattern in the corpus on the committed
83_568-word list with room to spare, but the budget is spent per *candidate
word*, and both the number of candidates and their length matter. Measured on
`deploy/dictionaries/wikipedia.dict` — 2.8M entries, many of them long
multi-word place names — `` *a*b*c*d*`2 `` returns 2_157_796 matches at the
shipped 50_000 and 2_159_108 at 2_000_000, so it is truncating at the default by
about 0.06%. Covering it costs 19.3 s to 21.8 s for those 1_312 matches.

This is the documented degradation working as designed rather than a bug, and it
is *better* than it was — the same pattern returned 2_157_571 before fuzz moved
to the structural engine. But it is worth knowing before quoting a match count
from a supplementary dictionary as complete, and it is the reason to re-measure
rather than assume if someone reports a "missing" match on a large list. Raising
the default is a real option; it costs nothing on `words.txt`, because a limit
only binds once it is exceeded. It should be calibrated against the big
dictionary rather than guessed at, which `limitcal` cannot do today (it hardcodes
`words.txt`).

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

That "after" column is the gap-run work's own result, kept as the historical
record of why the normalization exists. All four have since moved again — they
are 3.9 ms, 16.9 ms, 1.4 ms and 8.2 ms today — because every one of them is a
star-plus-digit or a fuzz pattern, and both of those moved to the structural
engine. The normalization is still what keeps them tractable; the engine change
compounds with it rather than replacing it.

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
