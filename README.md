# cha

A fast word-search tool for crosswords, Scrabble, and other word games.
Searches a word list using a concise pattern language.

## Usage

```
cha <pattern> [-w wordlist] [-b bench_count]
cha -i        [-w wordlist]
```

By default, `cha` looks for `csw2019.txt` (Collins Scrabble Words 2019) in the
current directory. Pass `-w` to use a different file.

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

## Benchmarking

Pass `-b N` to run the matcher N times and report throughput — useful for
profiling pattern changes:

```
cha ';..exit' -b 1000
```
