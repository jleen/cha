## Pattern Syntax

| Symbol | Meaning |
|-------|---------|
| `a`–`z` | Exact letter |
| `.` | Any one letter |
| `*` | Zero or more letters |
| `@` | One vowel (aeiou) |
| `#` | One consonant |
| `[abc]` | Matches any one of these letters |
| `-` `'` ` ` | Literal punctuation |
| `1`–`9` | Same letter as other occurrences of that digit |
| `` `N `` | Allow up to *N* “wrong” letters |
| `;···` | Anagram |
| `(···)` | Subpattern, which may itself include an anagram |

### Examples

```
.y...l        # HYMNAL, SYMBOL
@#@#@#@#@#@   # alternating vowels and consonants, 11 letters
1234321       # palindromes (DEIFIED, RACECAR)
...-..-.....  # 3-2-5 hyphenated (FLY-BY-NIGHT)
```

### Anagrams

A semicolon introduces an anagram pool. Letters before `;` are a pattern.
Letters after `;` are the anagram pool.

```
;ilphone          # PINHOLE
;..exit           # EXCITE (anagram EXIT plus two wildcards)
;doodle[ac][rn]   # CONDOLED (anagram with choices)
t....;intra       # TRAIN (anagram starting with T)
...;intra         # TIN (pattern shorter than pool, so anagram letters can be dropped)
......;intra      # RATING (pattern longer than pool, so anagram letters can be added)
;tent(ed)         # NETTED (anagram of TENTED that contains ED exactly)
```

### Wrong letters

A pattern ending in `` `N `` means that `N` many literals can be
“wrong,” i.e. different from the given.

- `` foo`1 `` will match `foe` and `goo`.
- `` electron`2 `` will match `ejection` and several others.

This feature can’t be combined with anagrams or number patterns (yet).

### Punctuation

Punctuation in the word list is ignored, unless punctuation is provided
in the pattern. If the pattern is punctuated then the word must match the
punctuation.
### Canonicalization

Cases, accents, and unicode variants are ignored and preserved.
(Internally, the engine canonicalizes the characters but also
stores the dictionary’s original characters.)  So you can search for
accented European words without worrying about typing the accents,
for example (although it’s fine if you do).

We also canonicalize things like `æ` into the “spelled-out”
`ae`. Again, you can enter them either way and get the same results.
(Note that this means `æ` is two letters, for wildcard-matching purposes.)

Non-Latin letters are case- and accent-canonicalized in the same way.
The vowel and consonant symbols only apply to the Latin alphabet.

### Subpatterns

Parentheses indicate a pattern within a pattern. This is mostly useful when you
want to apply an anagram pattern to just part of a word, like
`(f..;imou)(;bel)` gives `foible` and `fumble`.

### Logic

Use `&` to combine patterns and `!` to negate:

```
c.. & *at          # three-letter word starting with C and ending in AT
;intra & ! *a      # anagram but not ending in A
!c* & !*t          # doesn't start with C or end in T
```
