## Pattern Syntax

| Token | Meaning |
|-------|---------|
| `.` | One letter |
| `*` | Zero or more letters |
| `@` | One vowel (a/e/i/o/u) |
| `#` | One consonant |
| `[abc]` | Matches any one of these letters |
| `a`–`z` | Literal letter (case-insensitive) |
| `-` `'` ` ` | Literal punctuation |
| `1`–`9` | Same letter as other occurences of that digit|
| `` `N `` | Allow up to *N* ”wrong” letters |
' `;` | Introduces an anagram
| `(…)` | Subpattern, which may itself be an angram |

### Wrong letters

A pattern postfixed with `` `N `` means that `N` many literals can be
“wrong,” i.e. different from the given.

- `` foo`1 `` will match `foe` and `goo`.
- `` electron`2 `` will match `ejection` and several others.

This feature can’t be combined with anagrams or backreferences (yet).

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

### Canonicalization

Cases, accents, and unicode variants are ignored and preserved.
(Internally, the engine canonicalizes the characters but also
stores the dictionary’s original characters.)  So you can search for
accented European words without worrying about typing the accents,
for example (although it’s fine if you do).

We also canonicalize things like `æ` into the “spelled-out”
`ae`. Again, you can enter them either way and get the same results.
(Note that this means `æ` is two letters, for wildcard-matching purposes.)

Non-Latin letters are case- and accent-canonicalized in the same way,
but won’t work with vowel and consonant patterns.

### Subpatterns

Parentheses indicate a pattern within a pattern. This is mostly useful when you
want to apply an anagram pattern to just part of a word, like
`(f..;imou)(;bel)` gives `foible` and `fumble`.

### Logic

Use `&` to combine patterns and `!` to negate:

```
c.. & *at          # three-letter word starting with C and ending in AT
;intra & ! *a      # anagram match but not ending in -A
!c* & !*t          # doesn't start with C, doesn't end in T
```
