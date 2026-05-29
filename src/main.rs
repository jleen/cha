mod dictionary;
mod pattern;

use std::process;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: char <pattern> [wordlist] [bench_count]");
        process::exit(1);
    }

    let pattern_str = &args[1];
    let wordlist_path = args.get(2).map(String::as_str).unwrap_or("csw2019.txt");
    let bench_count: usize = args.get(3).map(|s| s.parse().unwrap_or_else(|_| {
        eprintln!("bench_count must be a positive integer");
        process::exit(1);
    })).unwrap_or(1);

    let matcher = pattern::compile_pattern(pattern_str);
    let words = dictionary::load_words(wordlist_path).unwrap_or_else(|e| {
        eprintln!("Error loading word list '{}': {}", wordlist_path, e);
        process::exit(1);
    });

    if bench_count == 1 {
        for word in &words {
            if matcher(word) {
                println!("{}", word);
            }
        }
    } else {
        let start = Instant::now();
        for _ in 0..bench_count {
            for word in &words {
                let _ = matcher(word);
            }
        }
        let elapsed = start.elapsed();
        eprintln!(
            "{} iterations in {:.3}s ({:.3}ms each)",
            bench_count,
            elapsed.as_secs_f64(),
            elapsed.as_secs_f64() * 1000.0 / bench_count as f64
        );
    }
}
