//! The canonical form a word is matched in.
//!
//! `cha` matches on a *folded* form and displays the original: a search for
//! `elan` finds `élan` and hands back `élan`, accent intact. Everything in this
//! module exists to define that fold, and it is applied in exactly two places —
//! to every dictionary entry at load ([`dictionary`](crate::dictionary)) and to
//! the pattern at compile ([`pattern`](crate::pattern)) — so the two sides of a
//! comparison always agree.
//!
//! # What folds together
//!
//! Three questions, answered separately:
//!
//! - **Case and diacritics fold.** `É`, `é` and `e` are one letter. This is
//!   plain Unicode: lowercase, then decompose and drop the combining marks.
//! - **Letters that are nobody's business stay themselves.** `ω` is not `o`, `я`
//!   is not `r`, `漢` is itself. The fold touches only what Unicode says is a
//!   variant of something else, so every other script arrives unchanged.
//! - **Non-letters are not letters.** `×`, `²`, `°` and `—` are treated exactly
//!   as `/` and `7` already were; the fold leaves them alone and the matcher
//!   rejects them the same way it always has.
//!
//! # Why there is a table
//!
//! Step 2 handles the hundreds of precomposed letters — `á à â ä ã å ā ă ą` and
//! the rest — from one standard algorithm, which is most of the work for free.
//! But Unicode deliberately gives *no* decomposition to a handful of letters
//! that are nonetheless written as a Latin letter plus a stroke or a ligature:
//! `æ ø þ ð ł đ œ`. Measured on `deploy/dictionaries/wikipedia.dict` — 6_001_462
//! entries — those are not a corner case but essentially the whole remainder:
//! after case and diacritic folding, exactly six distinct non-ASCII characters
//! survive in the entire corpus, and four of them are in that list.
//!
//! [`MULTIGRAPHS`] is therefore a transcription of CLDR's `Latin-ASCII`
//! transform, not a judgement call — it was checked row by row against ICU's
//! `uconv -x Latin-ASCII`. Keeping it here rather than pulling a transliteration
//! crate is deliberate: those crates aim to romanize *everything* and would turn
//! `ω` into `o`, which is precisely the rule this module is built to keep.
//!
//! # Length
//!
//! A folded word is measured in **canonical letters**, so `Ærø` is four (`aero`)
//! and `Straße` is seven (`strasse`). `....` matches the first and `...` does
//! not. This falls out of the fold rather than being chosen, and for a word
//! puzzle it is arguably the right answer anyway — German crosswords already
//! spell `ß` as SS.

use std::borrow::Cow;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// Latin letters with no Unicode decomposition, and the ASCII they are written
/// as. Transcribed from CLDR's `Latin-ASCII` transform.
///
/// Keys are lowercase: [`fold`] lowercases before consulting this, so `Æ` is
/// reached as `æ`. The `ς`/`ﬁ` rows are the other job this table does — see
/// [`fold`] for why they are not handled by the lowercasing step.
const MULTIGRAPHS: &[(char, &str)] = &[
    // Ligatures and stroked letters: no decomposition exists for these.
    ('æ', "ae"),
    ('ø', "o"),
    ('þ', "th"),
    ('ð', "d"),
    ('ł', "l"),
    ('đ', "d"),
    ('œ', "oe"),
    ('ħ', "h"),
    ('ŧ', "t"),
    ('ı', "i"),
    ('ĸ', "k"),
    ('ŋ', "n"),
    ('ƀ', "b"),
    ('ƶ', "z"),
    ('ȝ', "g"),
    ('ȡ', "d"),
    ('ȴ', "l"),
    ('ȵ', "n"),
    ('ȶ', "t"),
    // What `to_lowercase` leaves undone relative to full case folding.
    ('ß', "ss"),
    ('ς', "\u{3c3}"), // final sigma -> sigma
    ('ﬀ', "ff"),
    ('ﬁ', "fi"),
    ('ﬂ', "fl"),
    ('ﬃ', "ffi"),
    ('ﬄ', "ffl"),
    ('ﬅ', "st"),
    ('ﬆ', "st"),
];

/// The canonical form of `c`, if it is one of the letters [`MULTIGRAPHS`] covers.
fn multigraph(c: char) -> Option<&'static str> {
    // Linear over ~28 entries, but only ever reached for a non-ASCII char, which
    // is 0.8% of a large real dictionary and 0% of the shipped `words.txt`.
    MULTIGRAPHS.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}

/// Whether `s` is already its own canonical form, testable without allocating.
///
/// True for every all-ASCII-lowercase string, which is the whole of the shipped
/// word list and 99.2% of a large supplementary one — so the common path through
/// [`fold`] allocates nothing and copies nothing.
fn is_canonical(s: &str) -> bool {
    s.bytes().all(|b| !b.is_ascii_uppercase() && b.is_ascii())
}

/// The form `s` is matched in. Borrows when `s` is already canonical.
///
/// Four steps, in this order:
///
/// 1. **Lowercase** (`str::to_lowercase` — full Unicode, locale-independent, so
///    `İ` behaves the same everywhere rather than following a Turkish rule we
///    would have to choose).
/// 2. **Decompose (NFD) and drop combining marks**, which is what turns `é` into
///    `e`. It also handles the "non-Latin letter *with* a diacritic" case for
///    free: Greek `έ` becomes `ε`.
/// 3. **[`MULTIGRAPHS`]** for what Unicode leaves undecomposed.
///
/// Step 1 before step 2 matters: `Æ` has to reach the table as `æ`. Step 3 after
/// step 2 matters: a letter carrying *both* a stroke and an accent decomposes
/// first and hits the table second.
pub fn fold(s: &str) -> Cow<'_, str> {
    if is_canonical(s) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.to_lowercase().nfd() {
        if is_combining_mark(c) {
            continue;
        }
        match multigraph(c) {
            Some(sub) => out.push_str(sub),
            None => out.push(c),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_lowercase_is_borrowed_untouched() {
        // The whole of the shipped word list takes this path. It must not
        // allocate, and it must not change a single byte.
        for s in ["cat", "o'clock", "fly-by-night", "", "a b c"] {
            assert!(matches!(fold(s), Cow::Borrowed(_)), "{s:?} should borrow");
            assert_eq!(fold(s), s);
        }
    }

    #[test]
    fn case_and_diacritics_fold_together() {
        // Rule 3: a letter and its accented or capitalised forms are one letter.
        for (input, want) in [
            ("Élan", "elan"),
            ("élan", "elan"),
            ("ÉLAN", "elan"),
            ("naïveté", "naivete"),
            ("Señor", "senor"),
            ("Ångström", "angstrom"),
            ("Dvořák", "dvorak"),
            ("Łódź", "lodz"),
        ] {
            assert_eq!(fold(input), want, "fold({input:?})");
        }
    }

    #[test]
    fn undecomposable_latin_letters_use_the_table() {
        // Unicode gives these no decomposition, so step 2 cannot reach them and
        // MULTIGRAPHS has to. Every row checked against `uconv -x Latin-ASCII`.
        for (input, want) in [
            ("Ærø", "aero"),
            ("æ", "ae"),
            ("Straße", "strasse"),
            ("Þór", "thor"),
            ("Ðæt", "daet"),
            ("œuvre", "oeuvre"),
            ("Đà Nẵng", "da nang"),
        ] {
            assert_eq!(fold(input), want, "fold({input:?})");
        }
    }

    #[test]
    fn non_latin_letters_are_left_alone() {
        // Rule 2, and the reason a transliteration crate is the wrong tool: none
        // of these is a variant of a Latin letter, so none of them may become one.
        assert_eq!(fold("ωμέγα"), "ωμεγα"); // only the tonos is dropped
        assert_eq!(fold("Яя"), "яя");
        assert_eq!(fold("漢字"), "漢字");
        // ω is not o, and must not become it.
        assert_ne!(fold("ω"), "o");
    }

    #[test]
    fn sigma_variants_unify() {
        // Final sigma is a positional variant of sigma, and `to_lowercase` does
        // not unify them — only case *folding* does, which is what the table's
        // `ς` row stands in for.
        let sigma = "\u{3c3}";
        assert_eq!(fold("\u{3c2}"), sigma); // ς
        assert_eq!(fold("\u{3c3}"), sigma); // σ
        assert_eq!(fold("\u{3a3}"), sigma); // Σ
                                            // …so the two spellings of "sophos" fold together.
        assert_eq!(fold("σοφός"), fold("ΣΟΦΟΣ"));
    }

    #[test]
    fn non_letters_are_not_touched() {
        // Rule 1: a non-ASCII non-letter gets no special treatment here, exactly
        // as `/` and `7` get none. Whether it can match is the matcher's business.
        for s in ["×", "²", "°", "—", "½", "7", "/"] {
            assert_eq!(fold(s), s, "fold({s:?}) should be identity");
        }
    }

    #[test]
    fn folding_is_idempotent() {
        // Both sides of every comparison are folded, so folding twice must equal
        // folding once or a pattern could stop matching its own dictionary entry.
        for s in ["Élan", "Ærø", "Straße", "ΣΟΦΟΣ", "naïveté", "漢字", "Łódź"] {
            let once = fold(s).into_owned();
            assert_eq!(fold(&once), once, "fold is not idempotent on {s:?}");
        }
    }

    #[test]
    fn mojibake_folds_without_panicking() {
        // `deploy/dictionaries/wikipedia.dict` really does contain double-encoded
        // entries like this. Folding cannot repair them and must not choke: they
        // are valid UTF-8, just wrong, and the fold treats them as what they say.
        // `Ã` folds to `a` (diacritic dropped) and `©` is a non-letter left
        // alone, so the damage is preserved rather than guessed at.
        assert_eq!(
            fold("RenÃ©-Auguste CaillÃ©"),
            "rena\u{a9}-auguste cailla\u{a9}"
        );
    }

    #[test]
    fn length_is_counted_in_canonical_letters() {
        // The documented consequence: a multigraph spends more than one letter,
        // so `Ærø` is four and `Straße` is seven.
        assert_eq!(fold("Ærø").chars().count(), 4);
        assert_eq!(fold("Straße").chars().count(), 7);
        // A diacritic, by contrast, costs nothing.
        assert_eq!(fold("élan").chars().count(), 4);
    }
}
