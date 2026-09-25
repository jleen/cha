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

A semicolon introduces an anagram **pool**: the letters after the `;`. What the
pool demands depends on whether there is a **template** (anything before the
`;`).

### Without a template: an exact anagram

A pattern that starts with `;` finds words that are an arrangement of exactly the
pool letters.

```
;ilphone          # PINHOLE
;..exit           # EXIT plus any two letters → EXCITE, EXOTIC, FIXATE, …
;cat*             # C, A, T plus any number of others → ABDICATE, ABDUCT, …
;doodle[ac][rn]   # DOODLE plus A or C, and R or N → CONDOLED
;(che)rostra      # CHE + ROSTRA, with CHE unbroken → ORCHESTRA
```

In a pool, `.` is one letter that isn't in the pool, `*` is any number of them,
and `[…]` is one letter from the set. `(…)` letters are part of the pool, and
must also appear together, in that order. `@` and `#` are not allowed in a pool.

### With a template: all of the pool, or only the pool

When a template comes first, the template alone decides the word's length and
shape. The pool constrains its letters, and a word passes if **either** of these
is true:

- it uses **every** pool letter, plus anything else; or
- it uses **only** pool letters, though not necessarily all of them.

So the template's length picks the direction:

```
t....;intra       # same length as the pool: an anagram → TRAIN
...;intra         # shorter: only letters from INTRA → AIR, ANT, RAT, TIN, …
.......;intra     # longer: all of INTRA, plus two → CERTAIN, CURTAIN, GRANITE, …
....*;gdangboot   # both directions at once → TOAD, TOBOGGAN, TOBOGGANED, …
```

A template of just `*` lets in both directions at every length. That makes
`*;cat` a superset of `;cat*`, which finds every word containing C, A and T,
because `*;cat` also finds the words spelled only from those letters: A, AT, C,
T.

Letters the template pins count as pool letters, so `z....;brae` finds ZEBRA. A
`.` in the pool lets one letter in from outside the pool on the "only" side, so
`...;intra.` adds ACT, AFT, AIM, …. A `*` in the pool adds nothing when there is a
template: `...;cat*` finds the same ACT and CAT as `...;cat`.

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
