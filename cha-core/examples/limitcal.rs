//! Calibrates the *match-time* work limit against realistic patterns.
//!
//! `max_structural_steps` bounds work per candidate word and degrades an
//! over-budget word to "no match" — so a limit set too silently loses real matches. This finds, for each
//! pattern, the smallest limit that still returns every match a generous limit
//! finds. The largest such value over a corpus of plausible patterns is the
//! floor any default has to clear.
//!
//! Usage: cargo run --release -p cha-core --example limitcal [ceiling] [pattern ...]
//!
//! Patterns that take the regex path are still in the corpus, and are expected
//! to report a floor of 1: with digit variables gone from that engine the
//! template language is a pure DFA, which is why there is only one limit left to
//! calibrate. A floor above 1 for such a pattern means something has been
//! rerouted by accident.

use cha_core::dictionary::NamedWordList;
use cha_core::limits::Limits;
use cha_core::search::search;
use std::time::Instant;

/// Patterns a person might actually type: crossword fills, affix hunts, letter
/// shapes, and the digit-variable patterns — which now take the structural path.
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
    // Digit variables: these take the structural engine, and used to be the
    // only thing in the language reaching a backtracking regex.
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

/// `` `N `` fuzz, which shares the structural engine since the two token
/// vocabularies were merged. Digit variables are rejected alongside it by
/// design.
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
    // The shape docs/core.md records as silently truncating on the old regex
    // path: 566 of 579 matches in 1_972 ms there, all 579 in 57 ms here.
    "*1*2*3*4*1*2*3*4*",
];

fn run(lists: &[NamedWordList], pat: &str, structural: u32) -> Option<usize> {
    let limits = Limits {
        max_structural_steps: structural,
        max_results: usize::MAX,
        deadline: None,
        ..Limits::default()
    };
    search(lists, pat, &limits).ok().map(|r| r.total)
}

/// Smallest limit in `1..=ceiling` that reproduces `want`. Binary search is
/// valid because both limits are monotone: more budget never finds fewer matches.
fn smallest(lists: &[NamedWordList], pat: &str, want: usize, ceiling: usize) -> usize {
    let (mut lo, mut hi) = (1usize, ceiling);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let got = run(lists, pat, mid as u32);
        if got == Some(want) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

fn calibrate(lists: &[NamedWordList], pats: &[&str], label: &str, ceiling: usize) -> usize {
    println!("\n== {label} ==");
    println!(
        "{:<24} {:>14} {:>12}  {:>10}",
        "pattern", "min limit", "ms at min", "matches"
    );
    let mut worst = 0usize;
    for pat in pats {
        let want =
            run(lists, pat, ceiling as u32).unwrap_or_else(|| panic!("baseline failed for {pat}"));
        let need = smallest(lists, pat, want, ceiling);
        // How long one scan costs once the limit is the binding constraint.
        let t = Instant::now();
        let _ = run(lists, pat, need as u32);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        worst = worst.max(need);
        println!(
            "{:<24} {:>14} {:>12.1}  {:>10}",
            pat,
            commas(need),
            ms,
            want
        );
    }
    worst
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
    let mut worst = 0usize;
    if args.is_empty() {
        let tiers = [
            (REGEX_PATTERNS, "realistic - regex path"),
            (FUZZY_PATTERNS, "realistic - structural, `N fuzz"),
            (STRUCTURAL_PATTERNS, "realistic - structural, subpatterns"),
            (STRESS_PATTERNS, "stress tier"),
        ];
        for (pats, label) in tiers {
            worst = worst.max(calibrate(&lists, pats, label, ceiling));
        }
    } else {
        let pats: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        worst = worst.max(calibrate(&lists, &pats, "requested", ceiling));
    }
    println!("\ncalibration took {:.1}s", t.elapsed().as_secs_f64());
    println!("\nfloor: max_structural_steps >= {}", commas(worst));
}
