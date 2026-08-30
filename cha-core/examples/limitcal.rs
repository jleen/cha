//! Calibrates the two *match-time* work limits against realistic patterns.
//!
//! `backtrack_limit` and `max_fuzzy_steps` both bound work per candidate word,
//! and both degrade an over-budget word to "no match" — so a limit set too low
//! silently loses real matches. This finds, for each pattern, the smallest limit
//! that still returns every match a generous limit finds. The largest such value
//! over a corpus of plausible patterns is the floor any default has to clear.
//!
//! Usage: cargo run --release -p cha-core --example limitcal [ceiling] [pattern ...]
//!
//! Which limit a pattern exercises is inferred from the pattern itself: a
//! `` `N `` suffix with N > 0 takes the fuzzy path, everything else the regex one.

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
];

/// True when the pattern routes to the hand-rolled fuzzy matcher rather than the
/// regex engine — i.e. it carries a `` `N `` suffix with N > 0.
fn is_fuzzy(pat: &str) -> bool {
    match pat.rsplit_once('`') {
        Some((_, n)) => n.parse::<u32>().map(|k| k > 0).unwrap_or(false),
        None => false,
    }
}

fn run(lists: &[NamedWordList], pat: &str, backtrack: usize, fuzzy: u32) -> Option<usize> {
    let limits = Limits {
        backtrack_limit: backtrack,
        max_fuzzy_steps: fuzzy,
        max_results: usize::MAX,
        deadline: None,
        ..Limits::default()
    };
    search(lists, pat, &limits).ok().map(|r| r.total)
}

/// Smallest limit in `1..=ceiling` that reproduces `want`. Binary search is
/// valid because both limits are monotone: more budget never finds fewer matches.
fn smallest(
    lists: &[NamedWordList],
    pat: &str,
    fuzzy_path: bool,
    want: usize,
    ceiling: usize,
) -> usize {
    let (mut lo, mut hi) = (1usize, ceiling);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let got = if fuzzy_path {
            run(lists, pat, ceiling, mid as u32)
        } else {
            run(lists, pat, mid, ceiling as u32)
        };
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
) -> (usize, usize) {
    println!("\n== {label} ==");
    println!(
        "{:<18} {:>14} {:>12}  {:>10}",
        "pattern", "min limit", "ms at min", "matches"
    );
    let (mut worst_r, mut worst_f) = (0usize, 0usize);
    for pat in pats {
        let fuzzy_path = is_fuzzy(pat);
        let want = run(lists, pat, ceiling, ceiling as u32)
            .unwrap_or_else(|| panic!("baseline failed for {pat}"));
        let need = smallest(lists, pat, fuzzy_path, want, ceiling);
        // How long one scan costs once the limit is the binding constraint.
        let t = Instant::now();
        let _ = if fuzzy_path {
            run(lists, pat, ceiling, need as u32)
        } else {
            run(lists, pat, need, ceiling as u32)
        };
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let slot = if fuzzy_path {
            &mut worst_f
        } else {
            &mut worst_r
        };
        *slot = (*slot).max(need);
        println!(
            "{:<18} {:>14} {:>12.1}  {:>10}",
            pat,
            commas(need),
            ms,
            want
        );
    }
    (worst_r, worst_f)
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
    let (mut wr, mut wf) = (0usize, 0usize);
    if args.is_empty() {
        let (r1, f1) = calibrate(&lists, REGEX_PATTERNS, "realistic - regex path", ceiling);
        let (r2, f2) = calibrate(&lists, FUZZY_PATTERNS, "realistic - fuzzy path", ceiling);
        let (r3, f3) = calibrate(&lists, STRESS_PATTERNS, "stress tier", ceiling);
        wr = r1.max(r2).max(r3);
        wf = f1.max(f2).max(f3);
    } else {
        let pats: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let (r, f) = calibrate(&lists, &pats, "requested", ceiling);
        wr = wr.max(r);
        wf = wf.max(f);
    }
    println!("\ncalibration took {:.1}s", t.elapsed().as_secs_f64());
    println!(
        "\nfloors: backtrack_limit >= {}, max_fuzzy_steps >= {}",
        commas(wr),
        commas(wf)
    );
}
