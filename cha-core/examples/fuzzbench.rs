//! Ad-hoc timing harness for the fuzzy matcher's per-word step counter.
//!
//! Usage: cargo run --release -p cha-core --example fuzzbench -- <reps> [pattern ...]
//! With no patterns, runs a default set. Reports the best of `reps + 1` runs.

use cha_core::dictionary::NamedWordList;
use cha_core::limits::Limits;
use cha_core::search::search;
use std::time::{Duration, Instant};

const DEFAULT_PATTERNS: &[&str] = &[
    "cathode`1",
    "cathode`2",
    "elephant`1",
    "elephant`3",
    "cat*dog`1",
    "*cat*`1",
    "..@#..`2",
    "abcdefghij`4",
    "*a*e*i*`1",
    "*a*e*i*o*`1",
    "*a*b*c*d*`2",
];

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let reps: usize = args
        .first()
        .and_then(|s| s.parse().ok())
        .inspect(|_| {
            args.remove(0);
        })
        .unwrap_or(5);
    let pats: Vec<String> = if args.is_empty() {
        DEFAULT_PATTERNS.iter().map(|s| s.to_string()).collect()
    } else {
        args
    };

    let words = cha_core::dictionary::load_words("words.txt").expect("words.txt");
    let n = words.len();
    let lists = vec![NamedWordList {
        name: "bench".to_string(),
        words,
    }];
    eprintln!("{} words, best of {}", n, reps + 1);

    // Both match-time limits are overridable, so a sweep can price the worst
    // case a candidate default would allow.
    let env = |k: &str, d: usize| -> usize {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let backtrack = env("CHA_BENCH_BACKTRACK", 1_000_000);
    let fuzzy = env("CHA_BENCH_FUZZY", 1_000_000) as u32;
    // A deadline far enough out that it never fires, so what gets measured is
    // the cost of *having* one rather than the cost of tripping it.
    let deadline = env("CHA_BENCH_DEADLINE", 0) == 1;
    let max_results = env("CHA_BENCH_MAX_RESULTS", 5_000);
    eprintln!(
        "backtrack_limit={backtrack} max_fuzzy_steps={fuzzy} deadline={deadline} max_results={max_results}"
    );
    let limits = Limits {
        backtrack_limit: backtrack,
        max_fuzzy_steps: fuzzy,
        max_results,
        deadline: deadline.then(|| Instant::now() + Duration::from_secs(3600)),
        ..Limits::interactive()
    };
    let mut grand = 0.0f64;
    for pat in &pats {
        let mut best = f64::MAX;
        let mut total = 0usize;
        for _ in 0..=reps {
            let t = Instant::now();
            let r = search(&lists, pat, &limits).expect("search");
            best = best.min(t.elapsed().as_secs_f64());
            total = r.total;
        }
        grand += best;
        println!(
            "{:<16} {:>9.2} ms  {:>8.1} ns/word  ({} matches)",
            pat,
            best * 1e3,
            best * 1e9 / n as f64,
            total
        );
    }
    println!("{:<16} {:>9.2} ms  total", "ALL", grand * 1e3);
}
