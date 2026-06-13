# cha

A fast word-search tool for crosswords, Scrabble, and other word games.
Searches a word list using a concise pattern language.

## Usage

```
cha <pattern> [-w wordlist] [-b bench_count]
cha -i        [-w wordlist]
```

By default, `cha` looks for `words.txt` in the current directory. Pass `-w` to
use a different file.

Output is displayed in columns when writing to a terminal, or one word per line
when piped.

### Interactive mode

Pass `-i` / `--interactive` to enter a read-eval-print loop. `cha` prompts for
a pattern, shows results, then prompts again. The prompt supports readline-style
line editing and history recall (up/down arrows). Exit with `^D` on an empty
line.

## Pattern language

### Templates

Match words by position:

| Token | Meaning |
|-------|---------|
| `.` | Any single letter |
| `*` | Zero or more letters |
| `@` | Any vowel (a e i o u) |
| `#` | Any consonant |
| `[abc]` | One letter from the set |
| `a`–`z` | Literal letter (case-insensitive) |
| `-` `'` ` ` | Literal punctuation |
| `1`–`9` | Backreference variable — first use captures, subsequent uses must match |

Examples:

```
cha '.y...l'        # HYMNAL, SYMBOL, …
cha '@#@#@#@#@#@'   # alternating vowel/consonant, 11 letters
cha '1234321'       # palindromes (DEIFIED, RACECAR, …)
cha '...-..-.....`  # 3-2-5 hyphenated (FLY-BY-NIGHT, …)
```

### Anagrams

A semicolon introduces an anagram pool. Letters before `;` are a template;
letters after `;` are the pool.

```
cha ';ilphone'          # anagrams of ILPHONE → PINHOLE, …
cha ';..exit'           # anagrams of 6 letters including EXIT + 2 wildcards
cha ';doodle[ac][rn]'   # pool DOODLE plus either A or C, and R or N
cha 't....;intra'       # starts with T, is an anagram of INTRA
cha ';(che)rostra'      # anagram of CHEROST RA that contains CHE contiguously
```

**Template + anagram (hybrid):** letters in the template are counted as part of
the pool. `cha 'z....;brae'` matches ZEBRA because the Z is already in the
template.

**Wildcards in anagram:** `.` is a blank tile (any letter), `*` allows extra
letters beyond the pool.

### Combining patterns

Use `&` to AND patterns and `!` to negate:

```
cha 'c.. & *at'          # three-letter word starting with C and ending in AT
cha ';intra & ! *a'      # anagram match but not ending in -A
cha '!c* & !*t'          # doesn't start with C, doesn't end in T
```

## Building

```
cargo build --release
```

The binary ends up at `target/release/cha`.

## GUI

A desktop GUI (built with [Tauri](https://tauri.app/)) lives in `cha-gui/`. It
shares the same `cha-core` search engine and offers a pattern box with a live,
scrolling list of results. The same pattern language as the CLI applies.

### Building and running the GUI

Requires the Tauri CLI (no Node.js needed — the front end is plain
HTML/JS/CSS):

```
cargo install tauri-cli --version "^2"
```

On Linux, the WebKit webview also needs system packages, e.g. on Debian/Ubuntu:
`libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev build-essential libssl-dev`.
Windows (WebView2) and macOS (WKWebView) need no extra packages.

Run the Tauri commands from inside the GUI crate:

```
cd cha-gui/src-tauri
cargo tauri dev      # run in development
cargo tauri build    # produce a release bundle
```

`cargo tauri build` produces a platform-appropriate **Cha** app —
`Cha.exe`/MSI on Windows, `Cha.app`/dmg on macOS, AppImage/deb on Linux — under
`cha-gui/src-tauri/target/release/bundle/`. For a quick dev build without the
Tauri CLI, `cargo build -p cha-gui` works from anywhere in the workspace.

### Word list

When `words.txt` is present at the repo root at build time, the GUI embeds it
directly in the binary, so the app is fully self-contained. If `words.txt` is
absent at build time, the GUI instead loads it at runtime from the app config
directory:

- Linux: `~/.config/org.saturnvalley.cha/words.txt`
- macOS: `~/Library/Application Support/org.saturnvalley.cha/words.txt`
- Windows: `%APPDATA%\org.saturnvalley.cha\words.txt`

If no file is found there, the app still opens but shows a notice naming the
exact path where it expects `words.txt`, so you know where to put one.

## Benchmarking

Pass `-b N` to run the matcher N times and report throughput — useful for
profiling pattern changes:

```
cha ';..exit' -b 1000
```
