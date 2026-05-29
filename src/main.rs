mod dictionary;
mod pattern;

use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: char <pattern> [wordlist]");
        process::exit(1);
    }

    let pattern_str = &args[1];
    let wordlist_path = args.get(2).map(String::as_str).unwrap_or("csw2019.txt");

    let matcher = pattern::compile_pattern(pattern_str);
    let words = dictionary::load_words(wordlist_path).unwrap_or_else(|e| {
        eprintln!("Error loading word list '{}': {}", wordlist_path, e);
        process::exit(1);
    });

    for word in &words {
        if matcher(word) {
            println!("{}", word);
        }
    }
}
