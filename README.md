# Cha 茶

Cha 茶 is a pattern-matching and anagramming word tool intended to assist
in solving cryptic crosswords, acrostics, and other such word puzzles.  Cha
understands a pattern language based on the classic TEA Crossword Helper.
(Cha, however, has no relation to TEA other than as a source of inspiration.)

Cha is provided as a cross-platform GUI application that will run on Mac OS,
Windows, and Linux; as well as a CLI tool that will run on pretty much anything
that Rust can target.  In addition, the core pattern matcher is provided as a
standalone crate.

## Word list

No word list is provided. You’ll need to procure your own word list and provide
it as a text file called `words.txt`, one lowercase word per line.

If you provide `words.txt` at compile time, it will be compiled into the GUI
application and bundled with it.  Otherwise, you’ll need to provide a
`words.txt` at run time.

## GUI usage

Just install and run the application. If `words.txt` is not found,
you’ll see a message telling you where to put it.

## CLI usage

```
cha <pattern> [-d] [-w wordlist] [-b bench_count]
cha -i        [-d] [-w wordlist]
```

By default, `cha` loads its word list from `./words.txt`. Specify a different
word list with `-w`.  Display added/dropped anagram letters with `-d`.

Either specify a pattern on the command line, or pass `-i` / `--interactive` to
enter an interactive loop which will repeatedly prompt for a pattern and return
results.  Enter `^D` on an empty line to exit.

## Patterns

| Token | Meaning |
|-------|---------|
| `.` | One letter |
| `*` | Zero or more letters |
| `@` | A vowel (a e i o u) |
| `#` | A consonant |
| `[abc]` | One letter from the set |
| `a`–`z` | Literal letter (case-insensitive) |
| `-` `'` ` ` | Punctuation (see below) |
| `1`–`9` | Same letter as other occurences of that digit|
| `` `N `` | Allow up to *N* literal letters to vary |

### Variant matching

The `` `N `` syntax specifies that `N` many literals can be
“wrong”, i.e. different from the given.  Thus `` foo`1 `` will match `foe` and `goo`.
This has no effect on wildcards, but cannot be combined with anagrams or the `*`
wildcard.

```
cat`1     # CAT, BAT, CAR, COT, … (one letter off)
electron`2  # up to two letters off
.at`1     # the `.` still matches any letter; only `a` or `t` may vary
```

### Punctuation

Punctuation in the word list is ignored, unless punctuation is provided
in the pattern. If the pattern is punctuated then the word must match the
punctuation.

### Examples

```
.y...l        # HYMNAL, SYMBOL, …
@#@#@#@#@#@   # alternating vowel/consonant, 11 letters
1234321       # palindromes (DEIFIED, RACECAR, …)
...-..-.....  # 3-2-5 hyphenated (FLY-BY-NIGHT, …)
```

### Anagrams

A semicolon introduces an anagram pool. Letters before `;` are a template;
letters after `;` are the pool.

```
;ilphone          # anagram of ILPHONE → PINHOLE, …
;..exit           # anagram of EXIT + 2 wildcards
;doodle[ac][rn]   # anagram DOODLE plus either A or C, and R or N
t....;intra       # starts with T, is an anagram of INTRA
;(che)rostra      # anagram of CHEROST RA that contains CHE exactly
```

### Logic

Use `&` to combine patterns and `!` to negate:

```
c.. & *at          # three-letter word starting with C and ending in AT
;intra & ! *a      # anagram match but not ending in -A
!c* & !*t          # doesn't start with C, doesn't end in T
```

## Building

### CLI

```
cargo build --release
```

The CLI is built at `target/release/cha`.

### GUI

The GUI uses [Tauri](https://tauri.app/). Install the Tauri build tool with

```
cargo install tauri-cli
```

On Linux you’ll need to install Tauri’s dependencies. On a Debian-based distro,
something like
`apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev build-essential libssl-dev` should do the trick.

Once Tauri and its dependencies are installed, you can build from the project
root:

```
cargo tauri dev      # run a debug build
cargo tauri build    # compile and package a release build
```

## Benchmarking

The CLI supports a benchmarking mode that will load the dictionary once and
do a word search repeatedly, for performance profiling purposes.
Pass `-b N` to run the matcher N times and report the elapsed time:

```
cha ';..exit' -b 1000
```
