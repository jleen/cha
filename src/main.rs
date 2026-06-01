mod dictionary;
mod pattern;

use clap::Parser;
use std::process;
use std::time::Instant;

#[derive(Parser)]
struct Args {
    pattern: String,

    #[arg(short, long, default_value = "csw2019.txt")]
    wordlist: String,

    #[arg(short, long, default_value_t = 1)]
    bench_count: usize,
}

fn main() {
    let args = Args::parse();

    let matcher = pattern::compile_pattern(&args.pattern);
    let words = dictionary::load_words(&args.wordlist).unwrap_or_else(|e| {
        eprintln!("Error loading word list '{}': {}", args.wordlist, e);
        process::exit(1);
    });

    if args.bench_count == 1 {
        for word in &words {
            if matcher(word) {
                println!("{}", word);
            }
        }
    } else {
        let start = Instant::now();
        for _ in 0..args.bench_count {
            for word in &words {
                let _ = matcher(word);
            }
        }
        let elapsed = start.elapsed();
        eprintln!(
            "{} iterations in {:.3}s ({:.3}ms each)",
            args.bench_count,
            elapsed.as_secs_f64(),
            elapsed.as_secs_f64() * 1000.0 / args.bench_count as f64
        );
    }
}
