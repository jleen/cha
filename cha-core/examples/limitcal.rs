//! Calibrates the three *match-time* work limits against realistic patterns.
//!
//! `backtrack_limit`, `max_fuzzy_steps` and `max_structural_steps` all bound
//! work per candidate word, and all degrade an over-budget word to "no match" —
//! so a limit set too low silently loses real matches. This finds, for each
//! pattern, the smallest limit that still returns every match a generous limit
//! finds. The largest such value over a corpus of plausible patterns is the
//! floor any default has to clear.
//!
//! Usage: cargo run --release -p cha-core --example limitcal [ceiling] [pattern ...]
//!
//! Which limit a pattern exercises is inferred from the pattern itself: a
//! `(...)` subpattern (or a digit in an anagram pool) takes the structural
//! path, a `` `N `` suffix with N > 0 takes the fuzzy path, and everything else
//! takes the regex one.

use cha_core::dictionary::NamedWordList;
use cha_core::limits::Limits;
use cha_core::search::search;
use std::time::Instant;

/// Patterns a person might actually type: crossword fills, affix hunts, letter
/// shapes, and the backreference patterns that are the regex path's only real
/// source of backtracking.
const REGEX_PATTERNS: &[&str] = &[
    ".....",
    "c.t",
    ".a.e.",
    "*ing",
    "un*ed",
    "*tion",
    "#@#@#",
    "[aeiou]....",
    "*a*e*",
    "*x*",
    "..o..e.",
    "s*t*p",
    // Digit variables: these take fancy-regex's backtracking VM.
    "1.1",
    "11",
    "1221",
    "12321",
    "1..1",
    ".1.1.",
    "1.2.2.1",
    "*1*1",
    "1*1",
    "a1a1",
    ".1..1.",
    "1.1.1",
];

/// The fuzzy path's equivalent. Digit variables are rejected here by design.
const FUZZY_PATTERNS: &[&str] = &[
    "cathode`1",
    "elephant`2",
    "elephant`3",
    ".....`1",
    "#@#@#`1",
    "*ing`1",
    "un*ed`1",
    "*tion`2",
    "c*t`1",
    "*a*e*`1",
    "..o..e.`2",
    "s*t*p`1",
    "[aeiou]....`1",
    "abcdefghij`4",
    "*cat*`1",
];

/// The structural path's equivalent: subpatterns, and the variables that cross
/// them. The fixed-length ones cost almost nothing (their cut points are
/// forced); the starred ones are where the split search actually searches.
const STRUCTURAL_PATTERNS: &[&str] = &[
    "(;oif)(;bel)",
    "(...;oif)(;bel)",
    "(f..;oif)(;bel)",
    "(;oif)(;bel);oifb",
    "(1234)(;1234)",
    "(;el)(;bo)w",
    "*(;bel)",
    "(;bel)*",
    "*(;ing)*",
    "..(;ing)",
    "c(1)t;1",
];

/// A limit generous enough to stand in for "unlimited" when establishing the
/// truth we compare against. Overridable: the stress tier is slow enough at this
/// ceiling that a lower one is worth using while exploring.
const GENEROUS: usize = 50_000_000;

/// Heavier patterns — still things a determined puzzle solver would type, but at
/// the edge of it. These set the real headroom a default needs.
const STRESS_PATTERNS: &[&str] = &[
    "*a*e*i*",
    "*a*e*i*o*",
    "*s*t*r*",
    "*1*1*1*",
    "*1*2*1*2*",
    "*a*e*i*`1",
    "*a*e*i*o*`1",
    "*s*t*r*`1",
    "*a*b*c*d*`2",
    "**********cat`1",
    "*(;ab)*(;cd)*",
    "*(;ab)*(;cd)*(;ef)*",
    "*.*(;ab)*.*(;cd)*",
    "*(1234)*(;1234)*",
];

/// Which engine — and therefore which limit — a pattern exercises.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Path {
    Regex,
    Fuzzy,
    Structural,
}

/// Mirrors `pattern::needs_structural` and `compile_template`'s fuzz test. The
/// structural test comes first because a pattern carrying both is a hard error.
fn path_of(pat: &str) -> Path {
    let (template, pool) = match pat.split_once(';') {
        Some((t, p)) => (t, p),
        None => (pat, ""),
    };
    if template.contains('(') || pool.chars().any(|c| c.is_ascii_digit()) {
        return Path::Structural;
    }
    match pat.rsplit_once('`') {
        Some((_, n)) if n.parse::<u32>().map(|k| k > 0).unwrap_or(false) => Path::Fuzzy,
        _ => Path::Regex,
    }
}

fn run(
    lists: &[NamedWordList],
    pat: &str,
    backtrack: usize,
    fuzzy: u32,
    structural: u32,
) -> Option<usize> {
    let limits = Limits {
        backtrack_limit: backtrack,
        max_fuzzy_steps: fuzzy,
        max_structural_steps: structural,
        max_results: usize::MAX,
        deadline: None,
        ..Limits::default()
    };
    search(lists, pat, &limits).ok().map(|r| r.total)
}

/// Run `pat` with `limit` on the path it exercises and everything else generous.
fn run_on(
    lists: &[NamedWordList],
    pat: &str,
    path: Path,
    limit: usize,
    ceiling: usize,
) -> Option<usize> {
    match path {
        Path::Regex => run(lists, pat, limit, ceiling as u32, ceiling as u32),
        Path::Fuzzy => run(lists, pat, ceiling, limit as u32, ceiling as u32),
        Path::Structural => run(lists, pat, ceiling, ceiling as u32, limit as u32),
    }
}

/// Smallest limit in `1..=ceiling` that reproduces `want`. Binary search is
/// valid because both limits are monotone: more budget never finds fewer matches.
fn smallest(lists: &[NamedWordList], pat: &str, path: Path, want: usize, ceiling: usize) -> usize {
    let (mut lo, mut hi) = (1usize, ceiling);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let got = run_on(lists, pat, path, mid, ceiling);
        if got == Some(want) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

fn calibrate(
    lists: &[NamedWordList],
    pats: &[&str],
    label: &str,
    ceiling: usize,
) -> (usize, usize, usize) {
    println!("\n== {label} ==");
    println!(
        "{:<24} {:>14} {:>12}  {:>10}",
        "pattern", "min limit", "ms at min", "matches"
    );
    let (mut worst_r, mut worst_f, mut worst_s) = (0usize, 0usize, 0usize);
    for pat in pats {
        let path = path_of(pat);
        let want = run(lists, pat, ceiling, ceiling as u32, ceiling as u32)
            .unwrap_or_else(|| panic!("baseline failed for {pat}"));
        let need = smallest(lists, pat, path, want, ceiling);
        // How long one scan costs once the limit is the binding constraint.
        let t = Instant::now();
        let _ = run_on(lists, pat, path, need, ceiling);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let slot = match path {
            Path::Regex => &mut worst_r,
            Path::Fuzzy => &mut worst_f,
            Path::Structural => &mut worst_s,
        };
        *slot = (*slot).max(need);
        println!(
            "{:<24} {:>14} {:>12.1}  {:>10}",
            pat,
            commas(need),
            ms,
            want
        );
    }
    (worst_r, worst_f, worst_s)
}

fn commas(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('_');
        }
        out.push(c);
    }
    out
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let ceiling: usize = args
        .first()
        .and_then(|s| s.parse().ok())
        .inspect(|_| {
            args.remove(0);
        })
        .unwrap_or(GENEROUS);

    let words = cha_core::dictionary::load_words("words.txt").expect("words.txt");
    let n = words.len();
    let lists = vec![NamedWordList {
        name: "cal".to_string(),
        words,
    }];
    println!("{n} words, ceiling {}", commas(ceiling));

    let t = Instant::now();
    let (mut wr, mut wf, mut ws) = (0usize, 0usize, 0usize);
    if args.is_empty() {
        let tiers = [
            (REGEX_PATTERNS, "realistic - regex path"),
            (FUZZY_PATTERNS, "realistic - fuzzy path"),
            (STRUCTURAL_PATTERNS, "realistic - structural path"),
            (STRESS_PATTERNS, "stress tier"),
        ];
        for (pats, label) in tiers {
            let (r, f, s) = calibrate(&lists, pats, label, ceiling);
            wr = wr.max(r);
            wf = wf.max(f);
            ws = ws.max(s);
        }
    } else {
        let pats: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let (r, f, s) = calibrate(&lists, &pats, "requested", ceiling);
        wr = wr.max(r);
        wf = wf.max(f);
        ws = ws.max(s);
    }
    println!("\ncalibration took {:.1}s", t.elapsed().as_secs_f64());
    println!(
        "\nfloors: backtrack_limit >= {}, max_fuzzy_steps >= {}, max_structural_steps >= {}",
        commas(wr),
        commas(wf),
        commas(ws)
    );
}
