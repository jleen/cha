//! Ad-hoc timing harness for the fuzzy matcher's per-word step counter.
//!
//! Usage: cargo run --release -p cha-core --example fuzzbench -- <reps> [pattern ...]
//! With no patterns, runs a default set. Reports the best of `reps + 1` runs.

use cha_core::dictionary::NamedWordList;
use cha_core::search::{search, SearchLimits};
use std::time::Instant;

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
        .map(|n| {
            args.remove(0);
            n
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

    let limits = SearchLimits::interactive();
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
