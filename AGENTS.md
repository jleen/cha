# Implementation notes for char

## Performance requirements

`cha` searches ~270k words (CSW2019) per query, and has ambitions to search
even larger (>10M words) lists. Matching must be fast enough to feel
instantaneous on a modern laptop. Current release-build baselines:

| Pattern type | Target | Achieved |
|---|---|---|
| Template (e.g. `qu...`) | < 10 ms | ~5 ms |
| Anagram (e.g. `;..oting`) | < 20 ms | ~8 ms |

The benchmark flag (`cha <pattern> <wordlist> <N>`) is the primary way to
measure regressions. Run with N=1000 to get stable averages.

## Non-obvious invariants

**Words are pre-lowercased.** `dictionary::load_words` lowercases every word
at load time. Matchers must not call `.to_lowercase()` on words in the hot
loop — it allocates a `String` per word and is redundant. The anagram closure
relies on this: it calls `count_str` directly on `word` without any case
conversion.

**The anagram pool is pre-computed.** `compile_anagram` builds `combo_pools`
— a `Vec<([usize; 26], usize)>` — before the closure is returned. Each entry
is the pre-summed character counter and size for `fixed_letters + one combo`.
Nothing in the per-word hot path touches a `Vec` or `HashMap` for pool
accounting.

**Character counting uses `[usize; 26]`, never `HashMap`.** Indexing by
`(byte - b'a')` is O(1) with no allocation. The `count_str` helper (bytes,
not chars) is the right function to call from the anagram closure. `count_chars`
(takes `&[char]`) is only used at pattern compile time.

**Punctuation stripping uses `Cow<str>` to avoid allocation.** In
`compile_pattern`, `test_word` borrows the original word when neither the
pattern nor the word contains punctuation — the common case for a Scrabble
wordlist. Allocation only happens when stripping is actually needed.

## What to avoid

- Do not allocate `Vec<char>` or `String` inside the per-word anagram loop.
  Allocations in that path are immediately measurable in benchmarks.
- Do not call `count_chars` inside the closure. Pool counters are pre-computed;
  only `count_str(word)` belongs inside the closure.
- Do not replace `[usize; 26]` with `HashMap` in the character-counting code.
  The HashMap version was ~6× slower on anagram queries.
- `fancy_regex` is required (not the plain `regex` crate) because digit
  variables (`1234321`) compile to named capture groups with backreferences,
  which a pure DFA cannot handle.
