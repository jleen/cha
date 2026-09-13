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

A word list is provided, based on the [12dicts](https://wyrdplay.org/12dicts.html) lists. You can also provide your own word list as a text file, one lowercase word per line.

The GUI expects user-provided word list files to be located in a designated directory
(see below).  If you’re building your own binaries,
you can also just replace `words.txt` with your preferred list; it will be compiled into the GUI
application and bundled with it, to be used in addition to any
user-provided word lists.

## Implementation notes and agent memory

Design notes, invariants, build process, and release procedures for each module live in
[docs/](docs/). Those documents are written by Claude, primarily for its own use,
but mentioned here since they might be a useful reference for human collaborators as well.
See
[docs/README.md](docs/README.md) for more detail.

## GUI usage

Just install and run the application. If no word lists are found,
you’ll see a message with a button that opens the word list folder for you.

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
| `(…)` | A subpattern (see below) |

### Variant matching

The `` `N `` syntax specifies that `N` many literals can be
“wrong”, i.e. different from the given.  Thus `` foo`1 `` will match `foe` and `goo`.
Only literal letters can vary: wildcards, character classes and subpatterns are
rigid, and spend no budget. It cannot be combined with an anagram pool or with a
digit variable.

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

### Subpatterns

Parentheses in the template introduce a **subpattern**: a whole pattern applied
to a contiguous slice of the word, with the slices laid end to end covering all
of it. The point is that a subpattern can carry an anagram of its own, so you
can ask for a word made of anagram blocks.

```
(;oif)(;bel)        # FOI + BLE → FOIBLE
(...;oif)(;bel)     # same, with a template on the first block
(f..;oif)(;bel)     # …and a letter pinned in it
(;el)(;bo)w         # blocks mix with ordinary tokens → ELBOW
*(;bel)             # ends in some arrangement of B, E, L
```

Parentheses with no `;` inside constrain nothing, so `ele(ph)ant` is just
`elephant`. `&` and `!` are whole-query operators and cannot appear inside a
subpattern. `` `N `` can: the block stays rigid and the literals around it vary,
so `` ele(;nahpt)`1 `` matches ELEPHANT and ALEPHANT.

A digit variable is one variable across the whole pattern, so it can be bound in
one block and spent in another — including inside an anagram pool, which is the
one place a digit was previously meaningless:

```
(1234)(;1234)     # 4 letters, then an anagram of those same 4 → REAPPEAR
c(1)t;1           # the pool spends whatever the template bound
```

A variable has to be bound before it is spent, reading left to right, so
`(;1234)(1234)` is an error.

When a subpattern *and* the whole pattern both have an anagram, the rule is that
**a letter excuses the outer pool from naming it only if it sits in a template
position — before a `;` — at any depth. A letter in any pool never does.** So
against FOIBLE:

```
(;oif)(;bel);oifb      # matches: the word uses all of O, I, F, B
(;oif)(;bel);oifblex   # matches: the word uses only pool letters (X spare)
(;oif)(;bel);oifblx    # no: an E left over *and* an X unused
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

The CLI supports a benchmarking mode that will load the word list once and
do a word search repeatedly, for performance profiling purposes.
Pass `-b N` to run the matcher N times and report the elapsed time:

```
cha ';..exit' -b 1000
```
