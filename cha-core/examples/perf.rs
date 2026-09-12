//! The one-stop performance suite for `cha-core`: one command, every matching
//! code path, and a verdict on whether a difference is real.
//!
//! ```text
//! ./scripts/perf.sh --save      # on the "before" build (git stash, build)
//! ./scripts/perf.sh --compare   # on the "after" build
//! ```
//!
//! Three things make this more than a timing loop.
//!
//! **It covers the dispatch tree on purpose.** `CORPUS` is tiered, and every
//! entry records which path it exercises, so `--list` doubles as documentation of
//! what `compile_pattern` actually branches on. A pattern language extension is
//! expected to add its own entry here in the same commit.
//!
//! **It reports match counts as loudly as milliseconds.** The gap-run work this
//! crate's history turns on (see `docs/core.md`) found patterns that were not
//! merely slow: they were exceeding a per-word limit and degrading over-budget
//! words to "no match", silently returning a fraction of the real result set.
//! `**********1**********1` returned 9_778 of 25_193 matches. A perf regression
//! here is a correctness regression wearing a costume, so `--compare` treats a
//! changed match count as the headline and a timing delta as the detail.
//!
//! **It measures its own noise floor instead of guessing at the machine, then
//! re-measures anything it flags.** A short probe runs one cheap pattern
//! repeatedly and reports the spread, which sets the threshold a `--compare`
//! delta has to clear; whatever clears it is then timed a second time, and only a
//! delta that reproduces is called a result. Sniffing for a slow environment does
//! not work instead of this: a WSL2 box exposes no cpufreq governor and no
//! container marker whether it is a quiet 28-core desktop or not. The banner
//! reports what it can; the probe and the re-measurement decide.
//!
//! Both match-time limits are overridable via `CHA_BENCH_*` for pricing a
//! candidate `Limits` default, but they default to the shipped values so a plain
//! run reports what the product actually does.

use cha_core::dictionary::{load_words, NamedWordList};
use cha_core::limits::Limits;
use cha_core::pattern;
use cha_core::search::search;
use std::fs;
use std::path::Path;
use std::process;
use std::time::{Duration, Instant};

const DEFAULT_WORDS: &str = "words.txt";
const DEFAULT_BASELINE: &str = "target/cha-perf/baseline.tsv";

/// The cheapest pattern in the corpus: a bare fixed-length template, where the
/// length early-out rejects most of the list with an integer compare. Timing it
/// repeatedly measures the machine, not the matcher. It is deliberately the
/// cheapest one, because a fixed amount of scheduler jitter is hardest to tell
/// from a real delta when the scan itself is short.
const NOISE_PROBE: &str = ".....";
/// How many independent best-of-`reps+1` measurements the probe takes. Five is
/// enough to see the spread without spending real time on it.
const NOISE_PROBE_GROUPS: usize = 5;

/// Each timed sample repeats the scan until it has taken at least this long, and
/// the reported figure is the sample divided by the repeat count.
///
/// This is what makes cheap and expensive patterns comparably precise. Scheduler
/// and timer jitter is roughly a fixed number of microseconds, so on a 1 ms scan
/// it is several percent and on a 300 ms scan it is invisible — measured, an
/// unchanged build showed +/-0.2% on the expensive patterns and up to 7% on the
/// sub-millisecond ones, which would flag a false regression on the cheapest and
/// most important path in the crate (the pure reject loop). Amortizing every
/// sample up to the same duration equalizes that instead of papering over it with
/// a laxer threshold.
const TARGET_SAMPLE: Duration = Duration::from_millis(20);
/// Ceiling on the amortization factor, so a pathologically fast scan cannot turn
/// one sample into a minute.
const MAX_INNER: u32 = 256;

// These three are calibrated, not guessed. On an idle 28-core desktop the probe
// (best of 10) repeatedly measures a 1.7-2.3% spread, and a busier machine runs
// 5-8%; a threshold of 2% would therefore have called a perfectly quiet machine
// unreliable. Re-measure with `--tier punct` a few times before moving them.
/// Below this spread the machine is quiet enough to trust small deltas.
const QUIET_PCT: f64 = 4.0;
/// Above this spread the numbers should not be used for a before/after at all.
const UNRELIABLE_PCT: f64 = 12.0;
/// No delta below this is ever called a regression, however quiet the machine.
const MIN_SIGNAL_PCT: f64 = 5.0;

const USAGE: &str = "\
Usage: cargo run --release -p cha-core --example perf -- [OPTIONS]
   or: ./scripts/perf.sh [OPTIONS]

  --words <PATH>     word list (default: words.txt, relative to the cwd)
  --reps <N>         timed reps per pattern (default 9; reports best and median)
  --save [PATH]      write a baseline (default target/cha-perf/baseline.tsv)
  --compare [PATH]   diff against a baseline and flag real changes
  --tier <NAME>      run one tier only (see --list)
  --pattern <PAT>    time an ad-hoc pattern instead of the corpus (repeatable)
  --list             print the corpus and what each entry exercises, then exit
  --allow-debug      permit a non-release build (the numbers will be garbage)
  -h, --help         this

Environment (defaults are the shipped Limits::interactive values):
  CHA_BENCH_BACKTRACK, CHA_BENCH_FUZZY, CHA_BENCH_MAX_RESULTS, CHA_BENCH_DEADLINE

Exit status: 0 ok, 1 usage or setup error, 2 timing regression, 3 match counts moved.";

/// One corpus entry: a pattern, and the code path it is here to keep honest.
struct Probe {
    tier: &'static str,
    pattern: &'static str,
    exercises: &'static str,
}

/// Every distinct path through `compile_pattern` and the per-word closures it
/// returns. Grouped by tier so a change that can only affect one engine can be
/// measured without paying for the rest.
// Kept as a table on purpose: rustfmt would give each entry five lines and the
// tiers would stop being scannable. This is the one hand-formatted item in the
// crate; everything else is plain `cargo fmt` output.
#[rustfmt::skip]
const CORPUS: &[Probe] = &[
    // The regex template path with no digit variables. fancy-regex hands these
    // straight to the linear `regex` crate, so none of them can backtrack.
    Probe { tier: "template", pattern: ".....", exercises: "fixed_len early-out; the cheapest possible scan" },
    Probe { tier: "template", pattern: "c.t", exercises: "short fixed_len; nearly the whole list rejected on length" },
    Probe { tier: "template", pattern: "..o..e.", exercises: "fixed_len with interior literals" },
    Probe { tier: "template", pattern: "*ing", exercises: "star: fixed_len is None, so every word reaches the regex" },
    Probe { tier: "template", pattern: "un*ed", exercises: "star between literals" },
    Probe { tier: "template", pattern: "*a*e*i*o*", exercises: "star-heavy but backreference-free: stays on the linear engine" },
    Probe { tier: "template", pattern: "#@#@#", exercises: "consonant and vowel classes" },
    Probe { tier: "template", pattern: "[aeiou]....", exercises: "bracket character class" },
    Probe { tier: "template", pattern: "c[a.b]t", exercises: "gap symbols inside a class are literals; must stay fixed-width 3" },
    // The two arms of the punctuation decision, which is made once at compile
    // time from whether the *pattern* contains punctuation.
    Probe { tier: "punct", pattern: "...'.", exercises: "pattern has punctuation: word is borrowed, no per-word byte scan at all" },
    Probe { tier: "punct", pattern: ".........", exercises: "pattern has none: byte scan every word, allocate a String for punctuated ones" },
    // Digit variables compile to named capture groups with backreferences, the
    // only thing in the language that reaches fancy-regex's backtracking VM.
    Probe { tier: "backref", pattern: "1221", exercises: "backreference, fixed length" },
    Probe { tier: "backref", pattern: "12321", exercises: "two backreferences" },
    Probe { tier: "backref", pattern: "1.2.2.1", exercises: "interleaved backreferences and wildcards" },
    Probe { tier: "backref", pattern: "*1*1", exercises: "star plus backreference: the worst realistic shape (193 steps)" },
    Probe { tier: "backref", pattern: "*1*2*1*2*", exercises: "alternating distinct backreferences: sets the backtrack_limit floor (1_315)" },
    // The hand-rolled fuzzy matcher. A `N > 0 suffix routes here; `0 falls
    // through to the regex path.
    Probe { tier: "fuzzy", pattern: "cathode`1", exercises: "star-free: toks.len() is an exact length, early-out applies" },
    Probe { tier: "fuzzy", pattern: "elephant`3", exercises: "star-free with a wide fuzz budget" },
    Probe { tier: "fuzzy", pattern: ".....`1", exercises: "all-wildcard fuzzy; cross-checks the match count of the `.....` template" },
    Probe { tier: "fuzzy", pattern: "..@#..`2", exercises: "fuzzy classes" },
    Probe { tier: "fuzzy", pattern: "*cat*`1", exercises: "starred: no length early-out, Star branches twice per node" },
    Probe { tier: "fuzzy", pattern: "abcdefghij`4", exercises: "long literal, deep budget, no matches: pure reject-path cost" },
    Probe { tier: "fuzzy", pattern: "*a*b*c*d*`2", exercises: "sets the max_fuzzy_steps floor (3_698)" },
    // The anagram pool. Order-independent by construction: `.` folds into a
    // count and `*` into a bool, so gap-run normalization does not apply.
    Probe { tier: "anagram", pattern: ";obelisk", exercises: "pure pool, exact: length equality rejects almost everything" },
    Probe { tier: "anagram", pattern: ";..oting", exercises: "pure pool with wildcards; one of the two documented baselines" },
    Probe { tier: "anagram", pattern: ";oting*", exercises: "has_star: the length equality reject is disabled" },
    Probe { tier: "anagram", pattern: ";diners[ai]", exercises: "a [...] group, so combo_pools is a compile-time cartesian product" },
    Probe { tier: "anagram", pattern: ";(che)rostra", exercises: "sub-pattern: a per-candidate `contains` on top of the pool check" },
    // Template and pool together: the non-pure arm runs the template matcher
    // first, then does the extra/unused licence arithmetic.
    Probe { tier: "hybrid", pattern: "........;gdangboot", exercises: "template plus pool, star-free" },
    Probe { tier: "hybrid", pattern: "......*;gdangboot", exercises: "template plus pool with a star" },
    // Composition. Note the two spellings of the same conjunction are not the
    // same query: `has_punct` is computed on the whole raw pattern, so the
    // spaces around `&` suppress punctuation stripping. Both are here so the
    // divergence stays visible (260 matches vs 254 on the committed words.txt).
    Probe { tier: "compose", pattern: "....&*t", exercises: "conjunction, unspaced: punctuation stripping stays on" },
    Probe { tier: "compose", pattern: ".... & *t", exercises: "conjunction, spaced: the spaces make has_punct true and disable stripping" },
    Probe { tier: "compose", pattern: ";..oting&!*ing", exercises: "negated part: must not match, contributes no MatchInfo" },
    // Volume: the max_results cap and the confirmed-match path, which is where
    // MatchRow and diff_letters allocate.
    Probe { tier: "saturate", pattern: "*", exercises: "matches nearly everything: max_results cap, truthful total" },
    Probe { tier: "saturate", pattern: "*a*", exercises: "half the list matches: MatchRow allocation at volume" },
    // The four shapes from the docs/core.md payoff table. A regression here is
    // 100x, not 10%, and historically came with silent result truncation.
    Probe { tier: "pathological", pattern: "**********cat`1", exercises: "was 7.9 s before gap-run normalization" },
    Probe { tier: "pathological", pattern: "**********1**********1", exercises: "was 24.7 s and returned 9_778 of 25_193 matches" },
    Probe { tier: "pathological", pattern: "*.*.*.*.*.*.*.*.*.*cat`1", exercises: "dots between stars: the accidental way to rebuild the bad shape" },
    Probe { tier: "pathological", pattern: "*.*.*1*.*.*1", exercises: "digits break the gap runs for real; was 2.9 s and truncated" },
];

/// How much run-to-run spread the machine showed on a fixed workload.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Noise {
    Quiet,
    Noisy,
    Unreliable,
}

impl Noise {
    fn classify(spread_pct: f64) -> Self {
        if spread_pct < QUIET_PCT {
            Noise::Quiet
        } else if spread_pct <= UNRELIABLE_PCT {
            Noise::Noisy
        } else {
            Noise::Unreliable
        }
    }

    fn label(self) -> &'static str {
        match self {
            Noise::Quiet => "QUIET",
            Noise::Noisy => "NOISY",
            Noise::Unreliable => "UNRELIABLE",
        }
    }

    fn advice(self) -> &'static str {
        match self {
            Noise::Quiet => "small deltas are meaningful",
            Noise::Noisy => "only large deltas are meaningful",
            Noise::Unreliable => "do NOT use these numbers for a before/after comparison",
        }
    }
}

/// A delta smaller than this is indistinguishable from the machine's own noise.
/// Twice the measured spread, and never less than `MIN_SIGNAL_PCT`.
fn signal_threshold(spread_pct: f64) -> f64 {
    (2.0 * spread_pct).max(MIN_SIGNAL_PCT)
}

struct Measured {
    tier: String,
    pattern: String,
    best: Duration,
    median: Duration,
    matches: usize,
}

/// A saved run.
///
/// `fingerprint` pins only the word list, because comparing timings across
/// different dictionaries is meaningless rather than merely noisy. The limits are
/// recorded separately and do **not** block a comparison: changing a `Limits`
/// default is one of the main things this suite exists to evaluate ("I lowered
/// `max_fuzzy_steps` — did it truncate any results?"), so a mismatch is reported
/// as context for reading the diff, not as a reason to refuse it.
struct Baseline {
    fingerprint: String,
    limits: String,
    /// Which selection produced it: a tier name, "adhoc", or "full". A partial
    /// save to the default path is legitimate for iterating on one tier, but it
    /// clobbers a full baseline, so the scope is recorded and reported.
    scope: String,
    git: String,
    noise_pct: f64,
    rows: Vec<(String, u128, u128, usize)>,
}

impl Baseline {
    fn find(&self, pattern: &str) -> Option<(u128, u128, usize)> {
        self.rows
            .iter()
            .find(|(p, ..)| p == pattern)
            .map(|&(_, best, median, matches)| (best, median, matches))
    }
}

/// Everything that identifies one run: written into the baseline header, and
/// used to decide whether a comparison is meaningful.
struct RunId {
    /// The part that must match for a comparison to mean anything: the word list.
    fingerprint: String,
    /// Recorded and reported on a mismatch, but never blocking — see [`Baseline`].
    limits: String,
    /// Which selection was run: a tier name, "adhoc", or "full".
    scope: String,
    /// Short git sha, with `-dirty` when the tree has uncommitted changes.
    git: String,
}

struct Config {
    words: String,
    reps: usize,
    save: Option<String>,
    compare: Option<String>,
    tier: Option<String>,
    patterns: Vec<String>,
    list: bool,
    allow_debug: bool,
}

fn die(msg: &str) -> ! {
    eprintln!("perf: {msg}");
    process::exit(1);
}

fn parse_args() -> Config {
    let mut cfg = Config {
        words: DEFAULT_WORDS.to_string(),
        // Best of 10. Measured, this halves the probe's spread versus best of 6,
        // and best of 16 does not improve on it; the whole suite still runs in
        // well under a minute.
        reps: 9,
        save: None,
        compare: None,
        tier: None,
        patterns: Vec::new(),
        list: false,
        allow_debug: false,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    // An optional-value flag takes the next argument only when it isn't itself a
    // flag, so `--save` and `--save path.tsv` both work.
    let optional_value = |args: &[String], i: &mut usize, default: &str| -> String {
        match args.get(*i + 1) {
            Some(v) if !v.starts_with('-') => {
                *i += 1;
                v.clone()
            }
            _ => default.to_string(),
        }
    };
    let required_value = |args: &[String], i: &mut usize, flag: &str| -> String {
        *i += 1;
        match args.get(*i) {
            Some(v) => v.clone(),
            None => die(&format!("{flag} needs a value")),
        }
    };
    while i < args.len() {
        match args[i].as_str() {
            "--words" => cfg.words = required_value(&args, &mut i, "--words"),
            "--reps" => {
                let v = required_value(&args, &mut i, "--reps");
                cfg.reps = match v.parse() {
                    Ok(n) if n > 0 => n,
                    _ => die("--reps needs a positive integer"),
                };
            }
            "--save" => cfg.save = Some(optional_value(&args, &mut i, DEFAULT_BASELINE)),
            "--compare" => cfg.compare = Some(optional_value(&args, &mut i, DEFAULT_BASELINE)),
            "--tier" => cfg.tier = Some(required_value(&args, &mut i, "--tier")),
            "--pattern" => {
                let v = required_value(&args, &mut i, "--pattern");
                cfg.patterns.push(v);
            }
            "--list" => cfg.list = true,
            "--allow-debug" => cfg.allow_debug = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                process::exit(0);
            }
            other => die(&format!("unknown argument `{other}`\n\n{USAGE}")),
        }
        i += 1;
    }
    cfg
}

/// First line of `/proc/cpuinfo`'s model name, or a portable fallback.
fn cpu_model() -> String {
    fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_else(|| std::env::consts::ARCH.to_string())
}

fn load_average() -> String {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| "n/a".to_string())
}

/// Markers that this is a container, where a CPU quota can make timings
/// unrepresentative. Absence proves nothing — hence the noise probe.
fn container_hint() -> String {
    if Path::new("/.dockerenv").exists() {
        return "docker (/.dockerenv)".to_string();
    }
    if Path::new("/run/.containerenv").exists() {
        return "podman (/run/.containerenv)".to_string();
    }
    if let Ok(cg) = fs::read_to_string("/proc/1/cgroup") {
        for marker in ["docker", "containerd", "lxc", "kubepods"] {
            if cg.contains(marker) {
                return format!("{marker} (cgroup)");
            }
        }
    }
    "none detected".to_string()
}

fn governor() -> String {
    fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "not exposed".to_string())
}

/// `<short sha>` or `<short sha>-dirty`, so a baseline records which tree it came
/// from and `--compare` can notice you forgot to rebuild.
fn git_id() -> String {
    let sha = process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| !o.stdout.is_empty());
    if dirty {
        format!("{sha}-dirty")
    } else {
        sha
    }
}

fn commas(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// Time one pattern, reported as the best and median *per-scan* duration.
///
/// One unmeasured warmup pass first: it faults in the word list and warms the
/// caches, and would otherwise read as a slow outlier. That pass also sizes the
/// amortization factor, so every sample takes about `TARGET_SAMPLE` regardless of
/// how cheap the pattern is.
/// The word lists, limits and repeat count every measurement shares. Bundled so
/// the confirmation pass in `report_comparison` can re-time a pattern.
struct Harness<'a> {
    lists: &'a [NamedWordList],
    limits: &'a Limits,
    reps: usize,
}

impl Harness<'_> {
    fn measure(&self, pattern: &str) -> (Duration, Duration, usize) {
        measure(self.lists, pattern, self.limits, self.reps)
    }
}

fn measure(
    lists: &[NamedWordList],
    pattern: &str,
    limits: &Limits,
    reps: usize,
) -> (Duration, Duration, usize) {
    let warm_start = Instant::now();
    let warm = match search(lists, pattern, limits) {
        Ok(r) => r,
        Err(e) => die(&format!("search failed for `{pattern}`: {e:?}")),
    };
    let one = warm_start.elapsed();
    let matches = warm.total;
    let inner = inner_count(one);

    let mut per_scan = Vec::with_capacity(reps + 1);
    for _ in 0..=reps {
        let t = Instant::now();
        for _ in 0..inner {
            let r = search(lists, pattern, limits);
            // A match count that moves between scans of one build would
            // invalidate every comparison downstream, so fail loudly rather than
            // average it away.
            match r {
                Ok(r) if r.total == matches => {}
                Ok(r) => die(&format!(
                    "`{pattern}` is not deterministic: {matches} matches, then {}",
                    r.total
                )),
                Err(e) => die(&format!("search failed for `{pattern}`: {e:?}")),
            }
        }
        per_scan.push(t.elapsed() / inner);
    }
    per_scan.sort();
    (per_scan[0], per_scan[per_scan.len() / 2], matches)
}

/// How many scans to fold into one sample so it lasts about `TARGET_SAMPLE`.
fn inner_count(one: Duration) -> u32 {
    if one.is_zero() {
        return MAX_INNER;
    }
    let want = TARGET_SAMPLE.as_secs_f64() / one.as_secs_f64();
    (want.ceil() as u32).clamp(1, MAX_INNER)
}

/// Estimate how much the number this suite actually reports — best of `reps + 1`
/// — varies between two measurements of identical code.
///
/// This is deliberately not the spread of individual runs. `--compare` diffs
/// best-of-N against best-of-N, and taking a minimum already discards most
/// scheduler jitter, so the dispersion of single runs overstates the real noise
/// floor by roughly an order of magnitude and would flag every comparison as
/// inconclusive. So the probe repeats the whole best-of-N measurement
/// `NOISE_PROBE_GROUPS` times and reports the spread of *those* results, which is
/// the quantity a verdict needs.
fn noise_probe(h: &Harness) -> f64 {
    let mut bests = Vec::with_capacity(NOISE_PROBE_GROUPS);
    for _ in 0..NOISE_PROBE_GROUPS {
        // Same code path as a corpus entry, so the spread it reports is the
        // spread of the statistic `--compare` actually diffs.
        let (best, ..) = h.measure(NOISE_PROBE);
        bests.push(best);
    }
    bests.sort();
    let lo = bests[0].as_secs_f64();
    let hi = bests[bests.len() - 1].as_secs_f64();
    if lo <= 0.0 {
        return f64::INFINITY;
    }
    (hi - lo) / lo * 100.0
}

fn write_baseline(path: &str, id: &RunId, noise_pct: f64, rows: &[Measured]) {
    if let Some(dir) = Path::new(path).parent() {
        if let Err(e) = fs::create_dir_all(dir) {
            die(&format!("cannot create {}: {e}", dir.display()));
        }
    }
    let mut out = String::new();
    out.push_str("# cha perf baseline\n");
    out.push_str(&format!("# fingerprint\t{}\n", id.fingerprint));
    out.push_str(&format!("# limits\t{}\n", id.limits));
    out.push_str(&format!("# scope\t{}\n", id.scope));
    out.push_str(&format!("# git\t{}\n", id.git));
    out.push_str(&format!("# noise_pct\t{noise_pct:.2}\n"));
    out.push_str("# tier\tpattern\tbest_ns\tmedian_ns\tmatches\n");
    for m in rows {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            m.tier,
            m.pattern,
            m.best.as_nanos(),
            m.median.as_nanos(),
            m.matches
        ));
    }
    if let Err(e) = fs::write(path, out) {
        die(&format!("cannot write {path}: {e}"));
    }
    println!(
        "\nbaseline saved to {path} ({} patterns, scope {})",
        rows.len(),
        id.scope
    );
    if id.scope != "full" {
        println!(
            "note: this is a partial baseline. A later --compare over a different\n\
             selection will have nothing to compare against."
        );
    }
}

fn read_baseline(path: &str) -> Baseline {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => die(&format!(
            "cannot read baseline {path}: {e}\nRun with --save on the \"before\" build first."
        )),
    };
    let mut b = Baseline {
        fingerprint: String::new(),
        limits: "unknown".to_string(),
        scope: "unknown".to_string(),
        git: "unknown".to_string(),
        noise_pct: f64::NAN,
        rows: Vec::new(),
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            let mut f = rest.split('\t');
            match (f.next(), f.next()) {
                (Some("fingerprint"), Some(v)) => b.fingerprint = v.to_string(),
                (Some("limits"), Some(v)) => b.limits = v.to_string(),
                (Some("scope"), Some(v)) => b.scope = v.to_string(),
                (Some("git"), Some(v)) => b.git = v.to_string(),
                (Some("noise_pct"), Some(v)) => b.noise_pct = v.parse().unwrap_or(f64::NAN),
                _ => {}
            }
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 5 {
            continue;
        }
        let best = f[2].parse().unwrap_or(0);
        let median = f[3].parse().unwrap_or(0);
        let matches = f[4].parse().unwrap_or(0);
        b.rows.push((f[1].to_string(), best, median, matches));
    }
    if b.rows.is_empty() {
        die(&format!("baseline {path} has no rows"));
    }
    b
}

fn main() {
    let cfg = parse_args();

    if cfg.list {
        println!("{:<13} {:<26} exercises", "tier", "pattern");
        for p in CORPUS {
            println!("{:<13} {:<26} {}", p.tier, p.pattern, p.exercises);
        }
        println!(
            "\n{} patterns in {} tiers",
            CORPUS.len(),
            tier_names().len()
        );
        return;
    }

    if cfg!(debug_assertions) && !cfg.allow_debug {
        die(
            "this is a debug build, where the matcher is 10-50x slower.\n\
             Build with --release (./scripts/perf.sh does), or pass --allow-debug.",
        );
    }

    // Limits default to the shipped values, so a plain run reports the product's
    // numbers. An override is for pricing a candidate default, and is recorded in
    // the fingerprint so it can never be mistaken for a stock run.
    let base = Limits::interactive();
    let mut overrides: Vec<String> = Vec::new();
    let env_usize = |key: &str, default: usize, overrides: &mut Vec<String>| -> usize {
        match std::env::var(key).ok().map(|v| v.parse()) {
            Some(Ok(v)) => {
                overrides.push(format!("{key}={v}"));
                v
            }
            Some(Err(_)) => die(&format!("{key} is not a number")),
            None => default,
        }
    };
    let backtrack = env_usize("CHA_BENCH_BACKTRACK", base.backtrack_limit, &mut overrides);
    let fuzzy = env_usize(
        "CHA_BENCH_FUZZY",
        base.max_fuzzy_steps as usize,
        &mut overrides,
    ) as u32;
    let max_results = env_usize("CHA_BENCH_MAX_RESULTS", base.max_results, &mut overrides);
    // A deadline far enough out that it never fires, so what is measured is the
    // cost of *having* one rather than the cost of tripping it.
    let deadline = env_usize("CHA_BENCH_DEADLINE", 0, &mut overrides) == 1;
    let limits = Limits {
        backtrack_limit: backtrack,
        max_fuzzy_steps: fuzzy,
        max_results,
        deadline: deadline.then(|| Instant::now() + Duration::from_secs(3600)),
        ..base
    };

    let words = match load_words(&cfg.words) {
        Ok(w) => w,
        Err(e) => die(&format!(
            "cannot load word list `{}`: {e}\n\
             Run from the repo root (./scripts/perf.sh does), or pass --words <PATH>.",
            cfg.words
        )),
    };
    let n = words.len();
    if n == 0 {
        die(&format!("word list `{}` is empty", cfg.words));
    }
    let lists = vec![NamedWordList {
        name: "perf".to_string(),
        words,
    }];

    // Which patterns to run, and validate them before timing anything: a pattern
    // that fails to compile, or that is contentless, would silently benchmark the
    // no-op matcher and report a beautiful number for nothing.
    let probes: Vec<(String, String)> = if !cfg.patterns.is_empty() {
        cfg.patterns
            .iter()
            .map(|p| ("adhoc".to_string(), p.clone()))
            .collect()
    } else {
        let selected: Vec<&Probe> = CORPUS
            .iter()
            .filter(|p| cfg.tier.as_deref().is_none_or(|t| p.tier == t))
            .collect();
        if selected.is_empty() {
            die(&format!(
                "no tier named `{}`. Known tiers: {}",
                cfg.tier.unwrap_or_default(),
                tier_names().join(", ")
            ));
        }
        selected
            .iter()
            .map(|p| (p.tier.to_string(), p.pattern.to_string()))
            .collect()
    };
    for (_, p) in &probes {
        match pattern::compile_pattern_checked_with(p, &limits) {
            Ok(pattern::Compiled {
                note: Some(note), ..
            }) => die(&format!(
                "`{p}` is contentless ({note}); it would benchmark a no-op matcher"
            )),
            Ok(_) => {}
            Err(e) => die(&format!("`{p}` does not compile: {e}")),
        }
    }

    let id = RunId {
        // Blocking: a different dictionary makes the timings incomparable.
        fingerprint: format!("words={} count={}", cfg.words, n),
        // Informational: reported on a mismatch, never a reason to refuse.
        limits: format!(
            "backtrack={backtrack} fuzzy={fuzzy} max_results={max_results} deadline={deadline}"
        ),
        scope: if !cfg.patterns.is_empty() {
            "adhoc".to_string()
        } else {
            cfg.tier.clone().unwrap_or_else(|| "full".to_string())
        },
        git: git_id(),
    };

    println!("cha-core perf suite");
    println!("  words        {} from {}", commas(n), cfg.words);
    println!(
        "  cpu          {} ({} visible)",
        cpu_model(),
        std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(0)
    );
    println!("  load / gov   {} / {}", load_average(), governor());
    println!("  container    {}", container_hint());
    println!(
        "  build        {} / git {}",
        if cfg!(debug_assertions) {
            "DEBUG"
        } else {
            "release"
        },
        id.git
    );
    println!("  limits       backtrack={backtrack} fuzzy={fuzzy} max_results={max_results} deadline={deadline}");
    if !overrides.is_empty() {
        println!("  *** NON-DEFAULT LIMITS: {} ***", overrides.join(" "));
        println!("      These are not the shipped values. Not comparable to a stock baseline.");
    }
    println!("  reps         best and median of {}", cfg.reps + 1);

    let h = Harness {
        lists: &lists,
        limits: &limits,
        reps: cfg.reps,
    };
    let noise_pct = noise_probe(&h);
    let noise = Noise::classify(noise_pct);
    println!(
        "  noise probe  +/-{:.1}%  {}  ({})",
        noise_pct,
        noise.label(),
        noise.advice()
    );
    if noise == Noise::Unreliable {
        println!("\n  !! This machine is too noisy to compare builds. Quiet it down, or run");
        println!("  !! the suite somewhere whose timings can be trusted.");
    }

    // Time everything.
    let mut results: Vec<Measured> = Vec::with_capacity(probes.len());
    println!(
        "\n{:<13} {:<26} {:>9} {:>9} {:>9} {:>9}",
        "tier", "pattern", "best ms", "med ms", "ns/word", "matches"
    );
    let mut last_tier = String::new();
    for (tier, pat) in &probes {
        if *tier != last_tier {
            if !last_tier.is_empty() {
                println!();
            }
            last_tier = tier.clone();
        }
        let (best, median, matches) = h.measure(pat);
        println!(
            "{:<13} {:<26} {:>9.2} {:>9.2} {:>9.1} {:>9}",
            tier,
            pat,
            ms(best),
            ms(median),
            best.as_secs_f64() * 1e9 / n as f64,
            matches
        );
        results.push(Measured {
            tier: tier.clone(),
            pattern: pat.clone(),
            best,
            median,
            matches,
        });
    }

    let total: Duration = results.iter().map(|m| m.best).sum();
    println!(
        "\n{:<40} {:>9.2} ms  total (best of each)",
        "ALL",
        ms(total)
    );
    for tier in tier_names() {
        let sub: Duration = results
            .iter()
            .filter(|m| m.tier == tier)
            .map(|m| m.best)
            .sum();
        if !sub.is_zero() {
            println!("{:<40} {:>9.2} ms", format!("  {tier}"), ms(sub));
        }
    }

    let mut status = 0;
    if let Some(path) = &cfg.compare {
        status = report_comparison(&read_baseline(path), &results, &id, &h, noise_pct, noise);
    }
    if let Some(path) = &cfg.save {
        write_baseline(path, &id, noise_pct, &results);
    }
    process::exit(status);
}

fn tier_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for p in CORPUS {
        if !names.contains(&p.tier) {
            names.push(p.tier);
        }
    }
    names
}

/// Diff against a baseline. Match counts come first and on their own, because a
/// changed result set is a correctness bug and a changed time is a budget
/// question; conflating them is how the gap-run truncation stayed hidden.
fn report_comparison(
    base: &Baseline,
    now: &[Measured],
    id: &RunId,
    h: &Harness,
    noise_pct: f64,
    noise: Noise,
) -> i32 {
    println!("\n--- compared with baseline ---");
    if base.fingerprint != id.fingerprint {
        eprintln!("perf: baseline is not comparable.");
        eprintln!("  baseline: {}", base.fingerprint);
        eprintln!("  this run: {}", id.fingerprint);
        eprintln!(
            "A different word list makes a timing comparison meaningless, not merely\n\
             noisy. Re-save the baseline against the list you want to compare on."
        );
        process::exit(1);
    }
    if base.limits != id.limits {
        println!("\nnote: limits differ between the two runs.");
        println!("  baseline: {}", base.limits);
        println!("  this run: {}", id.limits);
        println!(
            "  That is a legitimate thing to measure, so the comparison continues — but a\n\
             \x20 match-count change below is the limit's doing, not the matcher's."
        );
    }
    if base.scope != id.scope {
        println!(
            "\nnote: baseline scope is `{}`, this run is `{}`.",
            base.scope, id.scope
        );
    }
    let shared = now
        .iter()
        .filter(|m| base.find(&m.pattern).is_some())
        .count();
    if shared == 0 {
        eprintln!("\nperf: the baseline has none of the patterns in this run.");
        eprintln!(
            "  baseline scope: {} ({} patterns)",
            base.scope,
            base.rows.len()
        );
        eprintln!("  this run:       {} ({} patterns)", id.scope, now.len());
        eprintln!(
            "Reporting \"no regression\" from an empty intersection would be a false\n\
             all-clear, so this is an error. Re-save the baseline over the same selection."
        );
        process::exit(1);
    }
    if base.git == id.git && !id.git.ends_with("-dirty") {
        println!(
            "note: baseline and this run are both git {} with a clean tree — this may\n\
             be the same build measured twice.",
            id.git
        );
    }
    println!(
        "baseline git {} (noise +/-{:.1}%), now git {}",
        base.git, base.noise_pct, id.git
    );

    // Correctness first.
    let mut moved: Vec<(&str, usize, usize)> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    for m in now {
        match base.find(&m.pattern) {
            Some((_, _, was)) if was != m.matches => moved.push((&m.pattern, was, m.matches)),
            Some(_) => {}
            None => missing.push(&m.pattern),
        }
    }
    if moved.is_empty() {
        println!(
            "\nmatch counts: all {} unchanged",
            now.len() - missing.len()
        );
    } else {
        println!("\n*** MATCH COUNTS CHANGED — treat as a correctness regression ***");
        println!(
            "{:<26} {:>10} {:>10} {:>10}",
            "pattern", "was", "now", "delta"
        );
        for (p, was, is) in &moved {
            println!(
                "{:<26} {:>10} {:>10} {:>+10}",
                p,
                commas(*was),
                commas(*is),
                *is as i64 - *was as i64
            );
        }
        println!(
            "A pattern that returns fewer matches after a change is usually exceeding a\n\
             per-word limit and degrading over-budget words to \"no match\". See docs/core.md."
        );
    }
    if !missing.is_empty() {
        println!(
            "\nnot in baseline (new corpus entries): {}",
            missing.join(", ")
        );
    }

    // Then timings, judged against the measured noise floor.
    let threshold = signal_threshold(noise_pct.max(base.noise_pct));
    println!(
        "\ntimings (a delta under {threshold:.1}% is inside this machine's noise, so unjudged):"
    );
    println!(
        "{:<26} {:>10} {:>10} {:>9}  verdict",
        "pattern", "was ms", "now ms", "delta"
    );
    let pct = |was: f64, now: f64| {
        if was > 0.0 {
            (now - was) / was * 100.0
        } else {
            0.0
        }
    };
    let mut candidates: Vec<(&str, f64, f64)> = Vec::new();
    for m in now {
        let Some((was_ns, _, _)) = base.find(&m.pattern) else {
            continue;
        };
        let was = was_ns as f64 / 1e6;
        let now_ms = ms(m.best);
        let delta = pct(was, now_ms);
        let flagged = delta.abs() >= threshold;
        if flagged {
            candidates.push((&m.pattern, was, delta));
        }
        println!(
            "{:<26} {:>10.2} {:>10.2} {:>8.1}%  {}",
            m.pattern,
            was,
            now_ms,
            delta,
            if flagged { "?" } else { "" }
        );
    }

    // Anything flagged gets re-timed before it is called a result.
    //
    // A single cross-process comparison of identical code is noisier than the
    // in-process probe can see: measured over six such comparisons, the worst
    // per-pattern delta was typically 4-5% but reached 9.7% once, and the pattern
    // that drifted was different each time. Raising the threshold past that would
    // have cost real sensitivity on the sub-millisecond patterns, where a genuine
    // regression is also a few percent. Re-measuring is the cheaper trade: only a
    // handful of patterns are ever flagged, transient jitter does not survive a
    // second look, and a real change does.
    let mut regressions = 0;
    if candidates.is_empty() {
        println!("\nnothing outside the noise floor; no re-measurement needed.");
    } else {
        println!(
            "\nre-measuring {} flagged pattern(s) to separate jitter from signal:",
            candidates.len()
        );
        println!(
            "{:<26} {:>10} {:>10} {:>9}  verdict",
            "pattern", "was ms", "again ms", "delta"
        );
        for (pattern, was, first_delta) in &candidates {
            let (best, ..) = h.measure(pattern);
            let again = ms(best);
            let delta = pct(*was, again);
            // Confirmed only when the second look agrees in direction *and* still
            // clears the threshold.
            let confirmed = delta.abs() >= threshold && delta.signum() == first_delta.signum();
            let verdict = if !confirmed {
                "jitter"
            } else if delta > 0.0 {
                regressions += 1;
                "REGRESS"
            } else {
                "FASTER"
            };
            println!(
                "{:<26} {:>10.2} {:>10.2} {:>8.1}%  {}",
                pattern, was, again, delta, verdict
            );
        }
    }

    println!();
    if noise == Noise::Unreliable {
        println!(
            "VERDICT: inconclusive — the machine was too noisy to judge ({noise_pct:.1}% spread)."
        );
        return 2;
    }
    if !moved.is_empty() {
        println!("VERDICT: match counts moved. Fix that before looking at the timings.");
        return 3;
    }
    if regressions > 0 {
        println!(
            "VERDICT: {regressions} pattern(s) confirmed slower by more than {threshold:.1}%."
        );
        return 2;
    }
    println!("VERDICT: no regression beyond the noise floor.");
    0
}
