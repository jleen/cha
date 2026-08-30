//! Ceilings on how much work one query is allowed to cost.
//!
//! Every backtracking or combinatorial path in this crate needs a bound, because
//! each one is driven directly by untrusted input and each can be made
//! superlinear by a short, innocent-looking pattern. They all live in one
//! [`Limits`] struct rather than being split by which module reads them, because
//! the split that actually matters to a caller is *when* a limit binds, not
//! whether `pattern` or `search` consults it — and that split cuts across both
//! modules. `compile_pattern_checked_with` simply ignores the two scan-time
//! fields, which is cheaper than making every caller assemble a nested struct.
//!
//! # Compile-time vs match-time
//!
//! **Compile-time** limits bind once, while the pattern is being turned into a
//! matcher, before any word is scanned. They cost nothing per word, and
//! exceeding one is a normal [`PatternError`](crate::pattern::PatternError).
//!
//! **Match-time** limits bind on *every candidate word*, and are re-armed fresh
//! for each one so a pathological word can't starve the words after it.
//! Exceeding one degrades that word to "no match" rather than raising, which is
//! what keeps the hot path free of `Result` handling. That degradation is why a
//! match-time limit set too low is a correctness problem, not just a slow one:
//! it silently narrows the pattern language. The defaults below are calibrated
//! against that risk — see [`Limits::interactive`].
//!
//! Neither match-time limit costs anything measurable to *enforce*.
//! `backtrack_limit` only picks the threshold `fancy-regex` compares against; it
//! increments its counter unconditionally either way. And `max_fuzzy_steps` was
//! measured against a build with the counter removed outright — see
//! `examples/fuzzbench.rs` — which came out a wash or slightly slower, the
//! difference being codegen noise rather than the decrement.

use std::time::Instant;

/// How much work one query may cost, across both phases.
///
/// `Default` is [`Limits::interactive`]. A server exposing this to a network
/// wants much tighter values; see `cha-web`.
#[derive(Debug, Clone)]
pub struct Limits {
    // ---- Compile-time: bind once, before any word is scanned. ----
    /// Maximum length, in bytes, of the whole pattern string.
    ///
    /// Compile-time.
    pub max_pattern_len: usize,

    /// Maximum number of `[...]` combinations an anagram may expand to.
    ///
    /// Compile-time, and the only limit that can exhaust *memory* rather than
    /// time. `compile_anagram` calls `cartesian_product`, which materializes the
    /// full product of every `[...]` group before any word is scanned, and
    /// `combo_pools` then expands each combo into 216 bytes. Growth is
    /// multiplicative in the group count: `;[abcde]` repeated 8 times is 5^8 =
    /// 390_625 combos (~84 MB), ten times is ~9.7M (~2.1 GB). Because this
    /// happens during compile, no scan deadline can catch it — the check has to
    /// stay where it is, before the product is built.
    pub max_anagram_combos: usize,

    // ---- Match-time: re-armed per candidate word. ----
    /// Maximum regex backtracking steps **per word** (`fancy-regex`'s own unit).
    ///
    /// Match-time. Applies to the non-fuzzy template path. Note this binds far
    /// more narrowly than it looks: `template_to_regex` maps `*` to `[a-z]*`,
    /// but a template with no digit variables compiles to a pattern
    /// `fancy-regex` hands straight to the linear `regex` crate
    /// (`RegexImpl::Wrap`), which never backtracks at all. Star-only patterns
    /// are therefore unaffected by this limit — measured, `**********cat` and
    /// `*a*e*i*o*` both run correctly with `backtrack_limit` set to **1**.
    ///
    /// Backreferences are what actually reach the backtracking VM, and stars
    /// combined with them are what make it exponential. Runs of `.`/`*` no
    /// longer contribute — `compile_template` normalizes them, see
    /// `collapse_gap_run` — so what is left is alternating stars separated by
    /// *distinct backreferences*, which genuinely break the run:
    /// `*1*2*1*2*` needs ~1_315 steps, and `*1*2*3*4*1*2*3*4*` is still
    /// budget-bound at the default (566 matches at 20_000, 579 at 200_000, ~3 s
    /// per scan either way). That shape, not a star-only one, is the case this
    /// limit exists for.
    pub backtrack_limit: usize,

    /// Maximum `fuzzy_match` nodes explored **per word**.
    ///
    /// Match-time. Applies to the fuzzy path, whose `Star` arm recurses twice
    /// per node with no engine underneath it to impose a limit of its own. This
    /// is a different quantity from `fuzzy_match`'s `budget` parameter (the
    /// fuzz allowance) and from its recursion depth, which is naturally bounded
    /// — it is the node count, and only the node count, that was unbounded.
    pub max_fuzzy_steps: u32,

    /// Maximum rows materialized across *all* groups combined.
    ///
    /// Match-time in the sense that it is consulted during the scan, but it
    /// costs one integer compare per *match* — not per word — and only on the
    /// confirmed-match path that is already allocating a `MatchRow`. Measured on
    /// a pattern with 37_195 matches, raising this from 0 to effectively
    /// unlimited costs ~18 ns/word, and all of that is the `MatchRow`s being
    /// built: the compare itself is free, and the cap *removes* work rather than
    /// adding it. It is a presentation bound, not a work bound — `total` is
    /// still counted past it, so truncation is reported rather than hidden.
    pub max_results: usize,

    /// When set, the scan gives up past this instant.
    ///
    /// Match-time, but deliberately **not** checked per word: `Instant::now()`
    /// is a syscall-ish read, and this is the hot loop the whole crate is tuned
    /// around. It is checked once per `DEADLINE_CHECK_INTERVAL`-word chunk, so
    /// its per-word cost is that read amortized over 4096 words. Measured against
    /// the cheapest possible scan (~12 ns/word), setting a deadline that never
    /// fires is indistinguishable from `None`. See the note in
    /// [`search`](crate::search::search).
    pub deadline: Option<Instant>,
}

impl Limits {
    /// A local app: generous enough that no plausible hand-typed pattern reaches
    /// a limit, tight enough to turn a hang or an OOM into a bounded wait.
    ///
    /// The two match-time ceilings are calibrated rather than guessed.
    /// `examples/limitcal.rs` finds, for each of a corpus of patterns, the
    /// smallest limit that still returns every match an effectively unlimited
    /// one finds. Ordinary patterns need very little — the worst realistic
    /// backreference (`*1*1`) needs 193 steps, and the worst realistic fuzzy
    /// pattern (`` *a*e*`1 ``) needs 396. A deliberately adversarial tier tops
    /// out at 1_315 and 3_698 respectively. The values here are ~15x that
    /// adversarial worst case, which leaves ordinary patterns two orders of
    /// magnitude of headroom while bounding the damage a hostile one can do.
    ///
    /// The previous values were 1_000_000 apiece, which bounded nothing useful:
    /// `` **********cat`1 `` took **66 seconds** for one scan under that
    /// ceiling. Cost is linear in the limit and the pattern's real matches are
    /// all found within 57 steps, so lowering it is nearly free in correctness
    /// and worth ~8x in time — the same scan is ~7.9 s here, and every pattern
    /// in the calibration corpus returns an identical match count.
    ///
    /// There is no deadline: a local user who types something slow can wait for
    /// it, or close the window. The web server sets one; see `cha-web`.
    pub fn interactive() -> Self {
        Self {
            // Interactive use: a pattern this long is a paste accident, not a query.
            max_pattern_len: 1024,
            // ~21 MB of `combo_pools`. Real patterns use a handful of groups.
            max_anagram_combos: 100_000,
            // ~15x the worst adversarial backreference pattern measured (1_315).
            backtrack_limit: 20_000,
            // ~15x the worst adversarial fuzzy pattern measured (3_698).
            max_fuzzy_steps: 50_000,
            // The cap protects the DOM from a pattern like `*` matching the
            // whole list.
            max_results: 5_000,
            deadline: None,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::interactive()
    }
}
