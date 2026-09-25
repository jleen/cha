---
author: Claude (Anthropic)
ai-generated: true
---

> **Provenance:** Written by Claude (Anthropic) while doing the work it
> describes. Reviewed in the normal course of review, but not audited line by
> line — treat specific numbers, paths, and version pins as claims to verify
> rather than guarantees.
>
> See [docs/README.md](README.md).

# Pattern language reference

The complete rules of the pattern language, for when a result surprises you.
[The README](../README.md) is the introduction; this is the fine print.

Example results are what `cha` returns against the committed `words.txt` unless
noted otherwise. Words with accents or in other scripts aren't in that list, so
the [accents section](#accents-and-other-scripts) was checked against a small
list containing them. Results display in lowercase; the examples use capitals so
you can tell them apart from patterns.

## Tokens

| Token | Meaning |
|-------|---------|
| `.` | One letter |
| `*` | Zero or more letters |
| `@` | A vowel (a e i o u) |
| `#` | A consonant |
| `[abc]` | One letter from the set |
| `a`–`z` | A literal letter (case-insensitive) |
| `-` `'` ` ` | Punctuation (see [Punctuation](#punctuation)) |
| `1`–`9` | The same letter as other occurrences of that digit |
| `` `N `` | Allow up to *N* literal letters to vary |
| `;` | Starts an anagram pool |
| `(…)` | A subpattern |
| `&` `!` | Combine and negate whole patterns |

```
.y...l        # CYMBAL, EYEFUL, HYMNAL, SYMBOL
@#@#@#@#@#@   # alternating vowel/consonant, 11 letters → IMAGINATIVE, INOPERATIVE
1234321       # a seven-letter palindrome → DEIFIED
...-..-.....  # 3-2-5 hyphenated → FLY-BY-NIGHT, OUT-OF-DOORS, OUT-OF-STATE
```

## Punctuation

Punctuation in a word (hyphen, apostrophe, space) is ignored unless the pattern
contains punctuation. If the pattern has any, the word must match it exactly.

```
flybynight     # FLY-BY-NIGHT
fly-by-night   # FLY-BY-NIGHT
```

## Variant matching

Put `` `N `` at the end of a pattern to let up to *N* of its literal letters be
"wrong", i.e. different from what the pattern says.

```
foo`1        # BOO, FOE, GOO, TOO, ZOO, …
cat`1        # BAT, CAR, COT, CUT, …
electron`2   # EJECTION, ELECTION, ELECTRIC, ELECTRON, ERECTION
.at`1        # the . still matches any letter; only A or T may vary
```

Only literal letters can vary. Wildcards, character classes and subpatterns are
rigid and don't count against *N*.

`` `N `` is an error, not a silent no-op, in two cases:

- **with a digit variable anywhere in the pattern**, e.g. `` 1a1`1 ``;
- **with a top-level anagram pool**, e.g. `` c.t;tac`1 ``. A pool inside a
  [subpattern](#subpatterns) is fine: the block stays rigid, and only the letters
  around it vary.

## Anagrams

A semicolon introduces an anagram **pool**. The part before `;` is a
**template** the word must fit, and the part after it is the set of letters the
word must be an arrangement of.

```
;ilphone          # anagram of ILPHONE → PINHOLE
;..exit           # anagram of EXIT plus any two letters → EXCITE, EXOTIC, FIXATE, …
;doodle[ac][rn]   # DOODLE plus A or C, and R or N → CONDOLED
t....;intra       # starts with T and is an anagram of INTRA → TRAIN
;(che)rostra      # anagram of CHE + ROSTRA that contains CHE unbroken → ORCHESTRA
```

In a pool, `.` is one wildcard letter, `*` means "and any number of other
letters", and `[…]` is one letter from the set. `@` and `#` are not allowed in a
pool.

## Accents and other scripts

Patterns and words are matched in a canonical form. **Case and accents are
ignored, and the original spelling is what you get back.** So `elan` finds
*élan* and `naivete` finds *naïveté*. It works both ways, so you can type the
accents if you like.

A few letters that Unicode treats as letters in their own right, not accented
ones, are written out as they are spelled: `æ`→`ae`, `ø`→`o`, `þ`→`th`, `ð`→`d`,
`ł`→`l`, `ß`→`ss`. So `aero` finds *ærø*, `strasse` finds *straße* and `thorn`
finds *þorn*. **Length is counted after the spelling-out**, so *ærø* has four
letters: it matches `....`, not `...`.

Letters from other scripts are letters in their own right, not spellings of
Latin ones. `ω` is not `o`, so `.....` matches *ωμέγα* but `omega` does not.
`@` and `#` only match Latin letters, because there's no language-neutral answer
to whether `ω` is a vowel. Anything that isn't a letter in any script is not a
letter to `cha` either.

These rules hold across the whole syntax (templates, anagrams, subpatterns,
digit variables and `` `N ``). `.....`, `` .....`1 `` and `(.....)` all match
*ωμέγα*, and so does the anagram `;αωμγε`.

## Subpatterns

Parentheses in a template mark a **subpattern**: a complete pattern applied to
one stretch of the word. The stretches sit end to end and cover the whole word.
This is useful because a subpattern can have its own anagram, so you can search
for a word built out of anagram blocks.

```
(;oif)(;bel)        # FOI + BLE → FOIBLE
(...;oif)(;bel)     # the same, with a template on the first block
(f..;oif)(;bel)     # …and a letter pinned in it
(f..;imou)(;bel)    # FOIBLE, FUMBLE
(;el)(;bo)w         # blocks mix with ordinary tokens → ELBOW
*(;bel)             # ends in some arrangement of B, E, L → ABLE, …
```

Parentheses with no `;` inside constrain nothing, so `ele(ph)ant` is just
`elephant`.

`&` and `!` apply to the whole query, so they can't appear inside a subpattern.
`` `N `` can: the block stays rigid and the literals around it vary, so
`` ele(;nahpt)`1 `` matches *elephant* and *alephant*, but not *elephxnt*.

### Digits across subpatterns

A digit is one variable across the whole pattern. It can be bound in one block
and used in another, including inside an anagram pool:

```
(1234)(;1234)   # 4 letters, then an anagram of those 4 → BERIBERI, COUSCOUS, REAPPEAR, …
c(1)t;1         # the pool contains whatever letter the template bound → CAT, COT, CUT
```

A digit must be bound before it is used, reading left to right, so
`(;1234)(1234)` is an error.

### When the inner and outer patterns both have a pool

When a subpattern *and* the whole pattern both have a pool, one rule decides
which letters the outer pool must account for:

**A letter excuses the outer pool from naming it only if it sits in a template
position (before a `;`), at any depth. A letter in any pool never does.**

So for FOIBLE:

```
(;oif)(;bel);oifb      # matches: the word uses all of O, I, F, B
(;oif)(;bel);oifblex   # matches: the word uses only pool letters (X spare)
(;oif)(;bel);oifblx    # no match: an E left over *and* an X unused
```

## Logic

`&` requires every pattern to match; `!` in front of a pattern negates it.

```
c.. & *at        # three letters, starts with C, ends in AT → CAT
;intra & ! *a    # anagram of INTRA, not ending in A → TRAIN
!c* & !*t        # doesn't start with C and doesn't end in T
```
