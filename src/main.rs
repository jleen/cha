use cha_core::dictionary;
use cha_core::pattern;

use clap::Parser;
use rustyline::DefaultEditor;
use std::io::{BufWriter, IsTerminal, Write};
use std::process;
use std::time::Instant;
use terminal_size::{terminal_size, Width};

#[derive(Parser)]
struct Args {
    pattern: Option<String>,

    #[arg(short, long, default_value = "words.txt")]
    wordlist: String,

    #[arg(short, long, default_value_t = 1)]
    bench_count: usize,

    #[arg(short, long)]
    interactive: bool,

    /// Show pool deltas after each match: unused (-) and extra (+) letters.
    #[arg(short, long)]
    delta: bool,
}

/// ANSI codes used to gray out the delta annotation in terminal output. Only
/// emitted in multi-column (terminal) mode, never in piped single-column output.
const GRAY: &[u8] = b"\x1b[90m";
const RESET: &[u8] = b"\x1b[0m";

/// One match plus its (possibly empty) formatted delta, e.g. `delta = "-D +HT"`.
struct MatchItem<'a> {
    word: &'a str,
    delta: String,
}

impl<'a> MatchItem<'a> {
    /// Build an item for `word`, formatting its delta from `info` when
    /// `show_delta` is set (otherwise the delta is empty and never rendered).
    fn new(word: &'a str, info: &pattern::MatchInfo, show_delta: bool) -> Self {
        MatchItem {
            word,
            delta: if show_delta {
                format_delta(info)
            } else {
                String::new()
            },
        }
    }

    /// Display width in terminal columns. Color codes are not display width, so
    /// they are deliberately excluded; the word and delta are ASCII, so byte
    /// length equals column count.
    fn width(&self) -> usize {
        self.word.len()
            + if self.delta.is_empty() {
                0
            } else {
                1 + self.delta.len()
            }
    }

    /// Write the rendered cell, graying the delta when `color` is set.
    fn render(&self, out: &mut impl Write, color: bool) {
        out.write_all(self.word.as_bytes()).unwrap();
        if !self.delta.is_empty() {
            out.write_all(b" ").unwrap();
            if color {
                out.write_all(GRAY).unwrap();
            }
            out.write_all(self.delta.as_bytes()).unwrap();
            if color {
                out.write_all(RESET).unwrap();
            }
        }
    }
}

/// Format a match's pool delta as `-UNUSED +EXTRA`, mirroring the GUI. Returns an
/// empty string when there is nothing to report (e.g. an exact anagram).
fn format_delta(info: &pattern::MatchInfo) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !info.unused.is_empty() {
        parts.push(format!("-{}", info.unused));
    }
    if !info.extra.is_empty() {
        parts.push(format!("+{}", info.extra));
    }
    parts.join(" ")
}

fn print_columns(items: &[MatchItem], color: bool) {
    if items.is_empty() {
        return;
    }
    let term_width = terminal_size()
        .map(|(Width(w), _)| w as usize)
        .unwrap_or(80);
    let max_len = items.iter().map(|m| m.width()).max().unwrap();
    let max_cols = ((term_width + 2) / (max_len + 2)).max(1);

    // Find the appropriate width for each column.
    // This is a trial-and-error process since it depends on the max cell width
    // (word plus delta) in each candidate column.
    let ncols = (1..=max_cols)
        .rev()
        .find(|&nc| {
            let nrows = items.len().div_ceil(nc);
            let total: usize = (0..nc)
                .map(|c| {
                    (c * nrows..((c + 1) * nrows).min(items.len()))
                        .map(|i| items[i].width())
                        .max()
                        .unwrap_or(0)
                })
                .sum::<usize>()
                + (nc - 1) * 2;
            total <= term_width
        })
        .unwrap_or(1);

    // Trial-and-error complete. So what did we get?
    let nrows = items.len().div_ceil(ncols);
    let col_widths: Vec<usize> = (0..ncols)
        .map(|c| {
            (c * nrows..((c + 1) * nrows).min(items.len()))
                .map(|i| items[i].width())
                .max()
                .unwrap_or(0)
        })
        .collect();

    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    for row in 0..nrows {
        for (col, col_width) in col_widths.iter().enumerate() {
            let idx = col * nrows + row;
            if idx >= items.len() {
                break;
            }
            if col > 0 {
                out.write_all(b"  ").unwrap();
            }
            let item = &items[idx];
            item.render(&mut out, color);
            if col + 1 < ncols && idx + nrows < items.len() {
                let pad = col_width - item.width();
                for _ in 0..pad {
                    out.write_all(b" ").unwrap();
                }
            }
        }
        out.write_all(b"\n").unwrap();
    }
}

fn run_pattern(pat: &str, words: &[String], delta: bool) {
    let matcher = match pattern::compile_pattern(pat) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {}", e);
            return;
        }
    };
    let stdout = std::io::stdout();
    if stdout.is_terminal() {
        // Terminal: multi-column, deltas grayed out when enabled.
        let items: Vec<MatchItem> = words
            .iter()
            .filter_map(|w| matcher(w).map(|info| MatchItem::new(w, &info, delta)))
            .collect();
        print_columns(&items, delta);
    } else {
        // Piped: one match per line, plain text only (no terminal codes).
        let mut out = BufWriter::new(stdout.lock());
        for word in words {
            if let Some(info) = matcher(word) {
                MatchItem::new(word, &info, delta).render(&mut out, false);
                out.write_all(b"\n").unwrap();
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
                    run_pattern(pat, &words, args.delta);
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
        run_pattern(pat, &words, args.delta);
    } else {
        let matcher = pattern::compile_pattern(pat).unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            process::exit(1);
        });
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
