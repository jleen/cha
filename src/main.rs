use cha_core::dictionary;
use cha_core::pattern;

use clap::Parser;
use rustyline::DefaultEditor;
use std::io::{IsTerminal, Write};
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

/// Style used to gray out the delta annotation. The escape codes are always
/// emitted by `render`; the `anstream` sink in `run_pattern` strips them when
/// color is not wanted (piped output, `NO_COLOR`, a dumb terminal, etc.).
const DELTA_STYLE: anstyle::Style =
    anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::BrightBlack)));

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

    /// Write the rendered cell. The delta is always styled gray; the `anstream`
    /// sink decides whether the escape codes survive.
    fn render(&self, out: &mut impl Write) {
        write!(out, "{}", self.word).unwrap();
        if !self.delta.is_empty() {
            write!(
                out,
                " {}{}{}",
                DELTA_STYLE.render(),
                self.delta,
                DELTA_STYLE.render_reset()
            )
            .unwrap();
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

fn print_columns(items: &[MatchItem], out: &mut impl Write) {
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
            item.render(out);
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
    let matcher = match pattern::compile_pattern_checked(pat) {
        // A contentless pattern (e.g. a bare `;`) matches nothing; report the
        // gentle note plainly and skip the scan. It's not an error.
        Ok(pattern::Compiled {
            note: Some(note), ..
        }) => {
            eprintln!("{}", note);
            return;
        }
        Ok(pattern::Compiled { matcher, .. }) => matcher,
        Err(e) => {
            eprintln!("error: {}", e);
            return;
        }
    };
    let raw = std::io::stdout();
    // anstream resolves the color policy (tty detection, NO_COLOR, CLICOLOR,
    // TERM, CI) and strips the style codes from `render` when color isn't wanted.
    let choice = anstream::AutoStream::choice(&raw);
    let columns = raw.is_terminal();

    let items: Vec<MatchItem> = words
        .iter()
        .filter_map(|w| matcher(w).map(|info| MatchItem::new(w, &info, delta)))
        .collect();

    // Buffer into a Vec-backed AutoStream (BufWriter isn't a RawStream), then
    // flush once. anstream governs color; the branch below governs layout.
    let mut sink = anstream::AutoStream::new(Vec::<u8>::new(), choice);
    if columns {
        print_columns(&items, &mut sink);
    } else {
        for item in &items {
            item.render(&mut sink);
            sink.write_all(b"\n").unwrap();
        }
    }
    raw.lock().write_all(&sink.into_inner()).unwrap();
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
