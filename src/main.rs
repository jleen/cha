mod dictionary;
mod pattern;

use clap::Parser;
use rustyline::DefaultEditor;
use std::io::{BufWriter, IsTerminal, Write};
use std::process;
use std::time::Instant;
use terminal_size::{Width, terminal_size};

#[derive(Parser)]
struct Args {
    pattern: Option<String>,

    #[arg(short, long, default_value = "csw2019.txt")]
    wordlist: String,

    #[arg(short, long, default_value_t = 1)]
    bench_count: usize,

    #[arg(short, long)]
    interactive: bool,
}

fn print_columns(words: &[&str]) {
    if words.is_empty() {
        return;
    }
    let term_width = terminal_size().map(|(Width(w), _)| w as usize).unwrap_or(80);
    let max_len = words.iter().map(|w| w.len()).max().unwrap();
    let max_cols = ((term_width + 2) / (max_len + 2)).max(1);

    // Find the appropriate width for each column.
    // This is a trial-and-error process since it depends on the max word length
    // in each candidate column.
    let ncols = (1..=max_cols).rev().find(|&nc| {
        let nrows = words.len().div_ceil(nc);
        let total: usize = (0..nc)
            .map(|c| (c * nrows..((c + 1) * nrows).min(words.len())).map(|i| words[i].len()).max().unwrap_or(0))
            .sum::<usize>()
            + (nc - 1) * 2;
        total <= term_width
    }).unwrap_or(1);

    // Trial-and-error complete. So what did we get?
    let nrows = words.len().div_ceil(ncols);
    let col_widths: Vec<usize> = (0..ncols)
        .map(|c| (c * nrows..((c + 1) * nrows).min(words.len())).map(|i| words[i].len()).max().unwrap_or(0))
        .collect();

    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    for row in 0..nrows {
        for col in 0..ncols {
            let idx = col * nrows + row;
            if idx >= words.len() {
                break;
            }
            if col > 0 {
                out.write_all(b"  ").unwrap();
            }
            let word = words[idx];
            out.write_all(word.as_bytes()).unwrap();
            if col + 1 < ncols && idx + nrows < words.len() {
                let pad = col_widths[col] - word.len();
                for _ in 0..pad {
                    out.write_all(b" ").unwrap();
                }
            }
        }
        out.write_all(b"\n").unwrap();
    }
}

fn run_pattern(pat: &str, words: &[String]) {
    let matcher = pattern::compile_pattern(pat);
    let stdout = std::io::stdout();
    if stdout.is_terminal() {
        let matches: Vec<&str> = words.iter().filter(|w| matcher(w)).map(String::as_str).collect();
        print_columns(&matches);
    } else {
        let mut out = BufWriter::new(stdout.lock());
        for word in words {
            if matcher(word) {
                writeln!(out, "{}", word).unwrap();
            }
        }
    }
}

fn main() {
    let args = Args::parse();

    if !args.interactive && args.pattern.is_none() {
        eprintln!("error: a pattern is required unless -i/--interactive is set");
        process::exit(1);
    }

    let words = dictionary::load_words(&args.wordlist).unwrap_or_else(|e| {
        eprintln!("Error loading word list '{}': {}", args.wordlist, e);
        process::exit(1);
    });

    if args.interactive {
        let mut rl = DefaultEditor::new().unwrap_or_else(|e| {
            eprintln!("Error initializing line editor: {}", e);
            process::exit(1);
        });
        loop {
            match rl.readline("> ") {
                Ok(line) => {
                    let pat = line.trim();
                    if pat.is_empty() {
                        continue;
                    }
                    let _ = rl.add_history_entry(pat);
                    run_pattern(pat, &words);
                }
                Err(rustyline::error::ReadlineError::Eof) => break,
                Err(rustyline::error::ReadlineError::Interrupted) => continue,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    process::exit(1);
                }
            }
        }
        return;
    }

    let pat = args.pattern.as_deref().unwrap();

    if args.bench_count == 1 {
        run_pattern(pat, &words);
    } else {
        let matcher = pattern::compile_pattern(pat);
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
