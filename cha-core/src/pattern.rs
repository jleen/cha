use regex::RegexBuilder;
use std::cell::OnceCell;

use crate::fold::fold;
use std::borrow::Cow;

use crate::limits::Limits;

const PUNCTUATION: &[char] = &[' ', '-', '\''];

/// Byte-wise `PUNCTUATION` test, for the detection scan that runs on every word.
///
/// All three marks are ASCII, so scanning bytes is equivalent to scanning chars
/// but skips UTF-8 decoding and the linear walk over `PUNCTUATION` — and this
/// runs once per character of every word in the list, before any matching, so
/// it is squarely in the hot path. The *stripping* path below still works in
/// chars: it is correctness-critical for non-ASCII input and rarely taken.
#[inline]
fn is_punct_byte(b: u8) -> bool {
    matches!(b, b' ' | b'-' | b'\'')
}

/// Find the first `c` that sits outside any `(...)` group.
///
/// `(...)` in a template introduces a subpattern, which is itself a whole
/// pattern — so the `;` in `(;oif)(;bel)` belongs to the subpattern, not to the
/// enclosing one, and a plain `find(';')` would split the pattern in the wrong
/// place. On paren-free input this is exactly `str::find`, which is what every
/// pattern written before subpatterns existed relies on.
///
/// Returns a byte offset. Unbalanced `)` is left alone here — the tokenizer
/// reports it with a better message than a scan could.
fn find_top_level(s: &str, c: char) -> Option<usize> {
    let mut depth = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if ch == c && depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// [`find_top_level`], searching from the right.
fn rfind_top_level(s: &str, c: char) -> Option<usize> {
    let mut depth = 0usize;
    for (i, ch) in s.char_indices().rev() {
        match ch {
            ')' => depth += 1,
            '(' => depth = depth.saturating_sub(1),
            _ if ch == c && depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Byte offset of the `)` closing the `(` at `open`, counting nested groups.
fn matching_paren(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (j, &ch) in chars.iter().enumerate().skip(open) {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
    }
    None
}

/// A compiled matcher. Returns `None` when the word does not match, or
/// `Some(MatchInfo)` when it does — the `MatchInfo` carries optional extra detail
/// about the match (e.g. unused pool letters) and is empty for matches that have
/// nothing extra to report.
pub type Matcher = Box<dyn Fn(&str) -> Option<MatchInfo>>;

/// Extra information about a successful match, surfaced for display. The match is
/// valid regardless of these fields; they are purely informational.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchInfo {
    /// Pool letters not used by the word, uppercased & sorted, e.g. "D". Empty if none.
    pub unused: String,
    /// Word letters not present in the pool, uppercased & sorted, e.g. "HT". Empty if none.
    pub extra: String,
}

/// Error returned when a pattern cannot be compiled (e.g. unclosed `[`, an
/// invalid regex, or a meaningless character). All failures are detected at
/// compile time; the returned matcher closures never fail.
#[derive(Debug, Clone)]
pub struct PatternError(pub String);

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PatternError {}

/// The result of compiling a pattern: a matcher plus an optional gentle note.
///
/// `note` is `Some` when the pattern is well-formed but *contentless* — it has
/// no letters or wildcard structure to match against (e.g. a bare `;`, an empty
/// template, or a stray `!`). Such a pattern matches nothing, and callers should
/// surface `note` gently (like a "no matches" message), *not* as a hard error:
/// the user often sees it transiently mid-typing. Ordinary patterns have `None`.
pub struct Compiled {
    pub matcher: Matcher,
    pub note: Option<String>,
}

/// Shown for a contentless pattern. Displayed gently by callers, never as an error.
const CONTENTLESS_NOTE: &str = "Pattern has no letters to match";

/// Compile a pattern, distinguishing a contentless pattern (well-formed but with
/// nothing to match — carried as a note) from a genuine syntax error (`Err`).
///
/// Uses `Limits::default()`. Callers that accept patterns from a network
/// should use [`compile_pattern_checked_with`] and supply tighter ceilings.
pub fn compile_pattern_checked(pattern_str: &str) -> Result<Compiled, PatternError> {
    compile_pattern_checked_with(pattern_str, &Limits::default())
}

/// [`compile_pattern_checked`] with explicit work ceilings. See [`Limits`].
pub fn compile_pattern_checked_with(
    pattern_str: &str,
    limits: &Limits,
) -> Result<Compiled, PatternError> {
    if pattern_str.len() > limits.max_pattern_len {
        return Err(PatternError(format!(
            "Pattern is too long ({} characters; the limit is {})",
            pattern_str.len(),
            limits.max_pattern_len
        )));
    }
    // Canonicalize the pattern exactly as the dictionary was canonicalized, so
    // both sides of every comparison are in the same alphabet: `ÉLAN` and `élan`
    // and `elan` all compile to the same matcher, and all three find `Élan`. Only
    // letters are touched — every metacharacter is ASCII punctuation or a digit,
    // which folding leaves alone — so this cannot disturb the syntax.
    let folded = fold(pattern_str);
    let pattern_str: &str = &folded;
    // `&` and `!` are whole-query operators, and the split below is textual, so
    // neither can be allowed inside a subpattern: `(a&b)` would otherwise be
    // torn into two parts and surface as a baffling "Unclosed '('". Rejecting
    // them here buys a message that says what is actually wrong.
    reject_operators_in_subpattern(pattern_str)?;
    let parts: Vec<&str> = pattern_str.split('&').collect();
    let mut matchers: Vec<(bool, Matcher)> = Vec::new();
    let mut contentless = false;
    // Whether the *pattern* asks for punctuation, decided below from the trimmed
    // parts rather than from `pattern_str`. It must see exactly the text that gets
    // compiled: the whitespace a user puts around `&` or after `!` is separator,
    // not content, and a space is one of the three marks in `PUNCTUATION`, so
    // reading the raw string made ` & ` silently disable punctuation stripping for
    // the whole query.
    let mut has_punct = false;

    for part in parts {
        let part = part.trim();
        let (negate, actual) = if let Some(rest) = part.strip_prefix('!') {
            (true, rest.trim())
        } else {
            (false, part)
        };
        has_punct |= actual.chars().any(|c| PUNCTUATION.contains(&c));
        // Compile every part regardless, so a real syntax error in any part
        // (e.g. `;&ca$t`) still surfaces as a hard `Err` and takes precedence
        // over the contentless note.
        let (matcher, part_contentless) = compile_one_pattern(actual, limits)?;
        contentless |= part_contentless;
        matchers.push((negate, matcher));
    }

    // A contentless pattern matches nothing (a no-op matcher) and reports a note.
    if contentless {
        return Ok(Compiled {
            matcher: Box::new(|_| None),
            note: Some(CONTENTLESS_NOTE.to_string()),
        });
    }

    let matcher: Matcher = Box::new(move |word: &str| {
        let test_word: Cow<str> = if has_punct {
            Cow::Borrowed(word)
        } else if word.as_bytes().iter().any(|&b| is_punct_byte(b)) {
            Cow::Owned(word.chars().filter(|c| !PUNCTUATION.contains(c)).collect())
        } else {
            Cow::Borrowed(word)
        };
        let mut info = MatchInfo::default();
        for (negate, m) in &matchers {
            match (m(&test_word), *negate) {
                // A negated part must not match, and contributes no detail.
                (Some(_), true) | (None, false) => return None,
                (None, true) => {}
                // A required part matched; fold its detail into the aggregate.
                // In practice only the single anagram part carries any.
                (Some(part), false) => {
                    info.unused.push_str(&part.unused);
                    info.extra.push_str(&part.extra);
                }
            }
        }
        Some(info)
    });
    Ok(Compiled {
        matcher,
        note: None,
    })
}

/// Reject `&` or `!` occurring inside a `(...)` subpattern.
///
/// Both are top-level operators: `&` conjoins whole patterns and `!` negates
/// one, and neither has a meaning scoped to a slice of a word. A subpattern is
/// a pattern, but only the template/anagram half of one.
fn reject_operators_in_subpattern(pattern_str: &str) -> Result<(), PatternError> {
    let mut depth = 0usize;
    for ch in pattern_str.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '&' | '!' if depth > 0 => {
                return Err(PatternError(format!(
                    "'{}' cannot appear inside a subpattern '(...)'",
                    ch
                )))
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn compile_pattern(pattern_str: &str) -> Result<Matcher, PatternError> {
    compile_pattern_checked(pattern_str).map(|c| c.matcher)
}

/// [`compile_pattern`] with explicit work ceilings. See [`Limits`].
pub fn compile_pattern_with(pattern_str: &str, limits: &Limits) -> Result<Matcher, PatternError> {
    compile_pattern_checked_with(pattern_str, limits).map(|c| c.matcher)
}

/// Compile one `&`-separated part, returning its matcher and whether it is
/// *contentless* — well-formed but with no letters or wildcard structure to match
/// (an empty template, or a bare `;` empty-pool anagram). Wildcards (`. * @ #`),
/// classes `[…]`, and sub-patterns `(…)` count as content, so only genuinely empty
/// parts are flagged.
fn compile_one_pattern(pattern: &str, limits: &Limits) -> Result<(Matcher, bool), PatternError> {
    // Dispatch is resolved once, here, and baked into the returned closure. The
    // structural engine is entered only by syntax that could not compile before
    // it existed, so every pattern written against the older language takes
    // byte-for-byte the path it always took.
    if needs_structural(pattern) {
        return compile_structural(pattern, limits);
    }
    if let Some(idx) = find_top_level(pattern, ';') {
        if idx == 0 {
            // Pure anagram: contentless when the pool has no matchable tokens.
            let contentless = anagram_pool_is_empty(&pattern[1..]);
            Ok((compile_anagram(None, &pattern[1..], limits)?, contentless))
        } else {
            Ok((
                compile_anagram(Some(&pattern[..idx]), &pattern[idx + 1..], limits)?,
                false,
            ))
        }
    } else {
        // Template: contentless when it is empty after stripping any fuzz suffix.
        let contentless = split_fuzz(pattern)?.0.is_empty();
        Ok((compile_template(pattern)?, contentless))
    }
}

/// Whether this part needs the structural engine rather than one of the three
/// older ones.
///
/// Three triggers. A `(...)` group in the *template* half (in the pool half,
/// `(...)` is the long-standing "contains this substring" marker and keeps that
/// meaning), and a digit in the pool half, which is what lets a variable bound
/// by the template be spent as a pool letter — both syntax that was a hard error
/// before subpatterns existed.
///
/// And two that are older than subpatterns and moved here on measurement.
///
/// `` `N `` used to have an engine of its own, but `Tok` was always `FuzzTok`
/// plus two variants and `walk` always `fuzzy_match` plus an environment and a
/// cut — duplication rather than specialization. `walk` carrying the mismatch
/// budget retires it, with the syntax surface unchanged; see `compile_node` for
/// the restrictions it inherits.
///
/// **Digit variables**, because `Var` is simply better at them than a regex is.
/// `fancy-regex` pays for itself only while it stays on the linear `regex` crate
/// (`RegexImpl::Wrap`), which is exactly the no-digit case; one digit drops it
/// onto the backtracking VM, and there a plain array store wins by 1.2x on
/// `1221`, 2.8x on `1234321`, 5.4x on `1*1` and 9.3x on `*1*2*1*2*` — measured,
/// same match counts. The gap widens with the shape: `*1*2*3*4*1*2*3*4*` took
/// 1_972 ms and silently returned 566 of its 579 matches, and takes 57 ms here
/// for all 579. Templates with a star and no digit stay on the regex path, where
/// the linear engine is 1.9-2.9x ahead and this walker has no answer.
fn needs_structural(pattern: &str) -> bool {
    let (template, pool) = match find_top_level(pattern, ';') {
        Some(idx) => (&pattern[..idx], &pattern[idx + 1..]),
        None => (pattern, ""),
    };
    template.contains('(')
        || template.contains('`')
        || template.chars().any(|c| c.is_ascii_digit())
        || pool.chars().any(|c| c.is_ascii_digit())
}

/// Whether an anagram pool contains no matchable tokens — no letters, wildcards
/// (`.`/`*`), character classes (`[`), or sub-patterns (`(`). True only for a pool
/// that is effectively empty (i.e. a bare `;`), which matches nothing meaningful.
fn anagram_pool_is_empty(pool: &str) -> bool {
    !pool
        .chars()
        .any(|c| c.is_alphabetic() || matches!(c, '.' | '*' | '[' | '('))
}

/// Split a trailing `` `N `` fuzz suffix off a template. Returns the base template
/// and the fuzz count (the number of literal characters allowed to not match), or
/// `None` when there is no backtick. A backtick is otherwise meaningless, so any
/// backtick is treated as a fuzz marker — this never collides with a valid pattern.
fn split_fuzz(template: &str) -> Result<(&str, Option<usize>), PatternError> {
    match rfind_top_level(template, '`') {
        None => Ok((template, None)),
        Some(idx) => {
            let base = &template[..idx];
            if find_top_level(base, '`').is_some() {
                return Err(PatternError("Pattern has more than one '`'".to_string()));
            }
            let num = &template[idx + 1..];
            if num.is_empty() {
                return Err(PatternError("Expected a number after '`'".to_string()));
            }
            let k: usize = num
                .parse()
                .map_err(|_| PatternError(format!("Invalid fuzz count '{}' after '`'", num)))?;
            Ok((base, Some(k)))
        }
    }
}

/// Match `word` against the lazily-built wide-class regex. See
/// `compile_template` for why it is built on demand.
fn unicode_match(
    cell: &OnceCell<Option<regex::Regex>>,
    template: &str,
    word: &str,
) -> Option<MatchInfo> {
    // Same syntax, wider class: if the ASCII one built, this one does too, so
    // `None` is unreachable and degrades to "no match" rather than making the
    // hot path fallible.
    cell.get_or_init(|| build_template_regex(template, ANY_LETTER_UNICODE).ok())
        .as_ref()
        .filter(|re| re.is_match(word))
        .map(|_| MatchInfo::default())
}

/// Anchor and build. `template` is carried only for the error message.
fn compile_anchored(regex_str: &str, template: &str) -> Result<regex::Regex, PatternError> {
    RegexBuilder::new(&format!("(?i)^{}$", regex_str))
        .build()
        .map_err(|e| PatternError(format!("Invalid template '{}': {}", template, e)))
}

fn build_template_regex(template: &str, class: &str) -> Result<regex::Regex, PatternError> {
    let (regex_str, _) = template_to_regex(template, class)?;
    compile_anchored(&regex_str, template)
}

fn compile_template(template: &str) -> Result<Matcher, PatternError> {
    // Every `` `N `` template routes to the structural engine (see
    // `needs_structural`), so a fuzz suffix cannot reach here.
    debug_assert!(
        !template.contains('`'),
        "a fuzz suffix should have routed to the structural engine"
    );
    // Two regexes, differing only in what `.` and `*` admit, and chosen per word
    // rather than per pattern — because the choice depends on the *word*, not on
    // the syntax.
    //
    // **The Unicode one is built lazily, and that is not a micro-optimization.**
    // `search` compiles the pattern on every call, so compilation is on the hot
    // path in a way match time usually hides: `\p{Alphabetic}` expands to a UTF-8
    // automaton of hundreds of states, and a template repeating it nine times
    // (`.........`) costs ~2 ms to *build*. Measured, that alone tripled the
    // whole scan — 1.20 ms to 3.19 ms — while the matcher itself was unchanged at
    // 1.14 ms. A word list with no non-ASCII entry never triggers it, so the
    // shipped list never pays, and a list that does pays once per query.
    //
    // Nothing reaching here can backtrack. Digit variables were the only source
    // of backreferences, and they compile to `Tok::Var` on the structural engine
    // now, so what is left is a pure DFA language and `regex` matches it in
    // linear time with no ceiling to trip. That is why this path cannot silently
    // truncate a result set, and why it no longer needs a `Limits` field: the
    // engine's own guarantee replaces the one we used to have to impose.
    // One parse, used for both the length bound and the regex. `search` compiles
    // the pattern on every call, so compile-time work is charged to every query;
    // parsing the template twice to get two halves of the same result is the kind
    // of thing that hides there.
    let (ascii_str, fixed_len) = template_to_regex(template, ANY_LETTER_ASCII)?;
    let ascii_re = compile_anchored(&ascii_str, template)?;
    let owned = template.to_string();
    let unicode_re: OnceCell<Option<regex::Regex>> = OnceCell::new();

    Ok(Box::new(move |word: &str| {
        // Reject on length before invoking the regex engine. A star-free template
        // matches exactly one length, and on a large list the overwhelming
        // majority of words are the wrong length — so this replaces a regex match
        // with an integer compare for most of the scan. It is a pure filter: any
        // word this rejects could not have matched anyway.
        //
        // Order matters more than it looks. Byte length is a lower bound on
        // character count (a UTF-8 char is at least one byte), so `< n` rejects
        // outright; `== n` is the overwhelmingly common survivor and needs no
        // further test; and only `> n` has to ask whether the word is ASCII,
        // because only a multi-byte word can be `n` characters in more than `n`
        // bytes. Asking unconditionally — scanning every word that clears the
        // lower bound — cost 60-90% on this tier.
        if let Some(n) = fixed_len {
            if word.len() < n {
                return None;
            }
            if word.len() != n {
                // More bytes than characters means a multi-byte word, which only
                // the precise class can judge.
                if word.is_ascii() {
                    return None;
                }
                return unicode_match(&unicode_re, &owned, word);
            }
        }
        // On ASCII input the two classes are the same language — `(?i)[a-z]` is
        // ASCII Alphabetic — so a match here is a match either way, and the
        // `is_ascii` scan below is skipped for every word that matches.
        //
        // A superset class cheap enough to reject without asking `is_ascii` at
        // all was tried here and is not available: `[a-z\x{80}-\x{10FFFF}]` made
        // `*` 2.2x and `*a*` 3.5x slower, because spanning every non-ASCII
        // scalar costs the engine more than the scan it saves.
        if ascii_re.is_match(word) {
            return Some(MatchInfo::default());
        }
        // A folded word is ASCII unless it carries a letter from a script the
        // fold leaves alone — under 1% of a large real list, and none of the
        // shipped one. Those are the only words the wider class can rescue, and
        // asking costs a byte scan on words that failed above.
        if word.is_ascii() {
            return None;
        }
        unicode_match(&unicode_re, &owned, word)
    }))
}

fn is_vowel(b: u8) -> bool {
    matches!(b, b'a' | b'e' | b'i' | b'o' | b'u')
}

/// Consume the run of `.`/`*` following the `*` the caller just matched, and
/// report how many `.` it contained.
///
/// A maximal run of `.` and `*` containing k dots and at least one star accepts
/// exactly the words of length >= k, *whatever the interleaving*: each `.`
/// contributes exactly one letter, each `*` contributes zero or more, and one
/// star can absorb any surplus. So the whole run normalizes to `.`xk followed by
/// a single `*`, which the caller emits. Both symbols are letter-only on both
/// paths (`.` is `[a-z]` / `FuzzTok::Any`, `*` is `[a-z]*` / `FuzzTok::Star`),
/// and neither consumes fuzz budget, so the rewrite is exact rather than
/// approximate. `**` collapsing is just the k = 0 case.
///
/// This changes no behavior — except where a run was expensive enough to exhaust
/// a match-time limit, which degraded the word to "no match" and *lost real
/// matches*. Both engines pay for a redundant gap symbol in branching, per
/// candidate word, and the effect was severe. Before this:
///
/// - `**********1**********1` took 24.7 s per scan and returned 9_778 of its
///   25_193 matches, the rest silently truncated by the per-word limit of the
///   day. As `*1*1` it cost 139 ms and returned all of them (17 ms now that
///   digit variables take the structural engine).
/// - `` **********cat`1 `` took 7.9 s; as `` *cat`1 `` it takes 8.7 ms.
/// - `` *.*.*.*.*.*.*.*.*.*cat`1 `` took 686 ms; as `` .........*cat`1 `` it
///   takes 2.3 ms. Handling `.` and not just `*` is what closes this one — a
///   star-only collapse is trivially defeated by sprinkling dots between the
///   stars, which is why the two symbols have to normalize together.
///
/// This attacks the cause. The limits stay as the backstop for the shapes that
/// have nothing to normalize — alternating stars with distinct backreferences,
/// like `*1*2*1*2*`, where the digits break the run for real.
///
/// Called from the `*` arm of a scan over `chars`, so it only ever sees symbols
/// the caller has already recognized as top-level — a `*` or `.` inside a
/// `[...]` class is consumed by the `[` arm and never reaches here.
fn collapse_gap_run(chars: &[char], i: &mut usize) -> usize {
    let mut dots = 0;
    while let Some(&c) = chars.get(*i + 1) {
        match c {
            '*' => {}
            '.' => dots += 1,
            _ => break,
        }
        *i += 1;
    }
    dots
}

fn escape_in_char_class(c: char) -> String {
    if matches!(c, ']' | '\\' | '^' | '-') {
        format!("\\{}", c)
    } else {
        c.to_string()
    }
}

/// What `.` and `*` match for a word the fold left non-ASCII: one letter, in
/// any script.
///
/// `@` and `#` deliberately do *not* follow. "Is omega a vowel" has no
/// locale-free answer, so they keep their ASCII sets and a Greek word never
/// matches `#@#`.
const ANY_LETTER_UNICODE: &str = r"\p{Alphabetic}";

/// What `.` and `*` match for an ASCII word.
///
/// Identical in meaning to [`ANY_LETTER_UNICODE`] restricted to ASCII — `(?i)`
/// makes `[a-z]` cover `A-Z` — and far cheaper. The wide class expands to a
/// UTF-8 automaton of hundreds of states, and `search` rebuilds the regex on
/// every call, so a template repeating it nine times cost ~2 ms per query to
/// *compile*. That, not match time, was the whole of the first measured
/// regression here.
const ANY_LETTER_ASCII: &str = "[a-z]";

/// Translate a template to a regex, and report the exact length it can match.
///
/// Every construct except `*` consumes exactly one character, so a star-free
/// template matches words of exactly one length — which lets the caller reject
/// most of the word list with an integer compare instead of the regex engine.
/// `None` means the length is not fixed (the template contains a `*`, or a
/// literal whose lowercasing changes its character count, which would make the
/// count unreliable).
fn template_to_regex(
    template: &str,
    any_letter: &str,
) -> Result<(String, Option<usize>), PatternError> {
    let mut out = String::new();
    let mut fixed_len: Option<usize> = Some(0);
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;

    // Count one input character for this position, unless the length has already
    // been marked unknowable.
    macro_rules! consume_one {
        () => {
            if let Some(n) = fixed_len {
                fixed_len = Some(n + 1);
            }
        };
    }

    while i < chars.len() {
        match chars[i] {
            '.' => {
                out.push_str(any_letter);
                consume_one!();
            }
            '*' => {
                // The run normalizes to its dots followed by one star; see
                // `collapse_gap_run`. No `consume_one!` for those dots: the star
                // has already made the length unknowable.
                let dots = collapse_gap_run(&chars, &mut i);
                for _ in 0..dots {
                    out.push_str(any_letter);
                }
                out.push_str(any_letter);
                out.push('*');
                fixed_len = None; // the only variable-width construct
            }
            '@' => {
                out.push_str("[aeiou]");
                consume_one!();
            }
            '#' => {
                out.push_str("[bcdfghjklmnpqrstvwxyz]");
                consume_one!();
            }
            '[' => {
                let rel = chars[i..]
                    .iter()
                    .position(|&x| x == ']')
                    .ok_or_else(|| PatternError("Unclosed '[' in template".to_string()))?;
                let j = i + rel;
                out.push('[');
                for &ch in &chars[i + 1..j] {
                    out.push_str(&escape_in_char_class(ch));
                }
                out.push(']');
                consume_one!();
                i = j;
            }
            c if c.is_ascii_digit() => {
                // Unreachable: `needs_structural` claims every template with a
                // digit. This used to emit a named group and a backreference,
                // which is what forced `fancy-regex` on the crate; erroring
                // instead keeps the claim that nothing here can backtrack true
                // by construction rather than by convention.
                debug_assert!(
                    false,
                    "a digit variable should have routed to the structural engine"
                );
                return Err(PatternError(format!(
                    "Template has meaningless character '{}'",
                    c
                )));
            }
            c @ ('-' | '\'' | ' ') => {
                out.push('\\');
                out.push(c);
                consume_one!();
            }
            c if c.is_alphabetic() => {
                let lowered = c.to_lowercase().to_string();
                // Almost always one char, but a few characters lengthen when
                // lowercased (İ -> i̇), so the position would match more than one
                // input character. Rare enough not to be worth modelling; just
                // give up on the fast path rather than compute a wrong length.
                if lowered.chars().count() == 1 {
                    consume_one!();
                } else {
                    fixed_len = None;
                }
                out.push_str(&regex::escape(&lowered));
            }
            c => {
                return Err(PatternError(format!(
                    "Template has meaningless character '{}'",
                    c
                )))
            }
        }
        i += 1;
    }
    Ok((out, fixed_len))
}

/// How many distinct non-ASCII letters one pattern may name.
///
/// Not a [`Limits`] field: it sizes the stack array in [`Histogram`], so it has
/// to be a compile-time constant. Sixteen is far past any real pattern — the
/// whole 6M-entry `wikipedia.dict` contains six distinct non-ASCII letters after
/// folding — and exceeding it is a normal `PatternError`, not a silent truncation.
const MAX_EXTRA_LETTERS: usize = 16;

/// A letter histogram: `a`-`z`, plus a slot for each non-ASCII letter the
/// *pattern* names.
///
/// The 26-bucket array survives intact, which is the point. Words are folded
/// before they get here, so all but a fraction of a percent of them are pure
/// ASCII and never touch `extra` at all — the alternative, a map keyed by
/// `char`, was measured at ~6x slower back when this was ASCII-only and there is
/// no reason to think it got faster.
///
/// `extra[j]` counts `alphabet[j]`, where the alphabet belongs to the [`Pool`].
/// A letter in neither place is *foreign*: countable but never matchable, so it
/// needs no slot. See [`Tally`].
///
/// Counts are `u32`, not `usize`, and that is a measured choice rather than
/// tidiness: this is zeroed once per candidate word, so its size *is* per-word
/// work. At `usize` the two arrays came to 336 bytes against the 208 of the
/// `[usize; 26]` this replaced, and the anagram tier paid 13% for it. At `u32`
/// they come to 168 — less than the original — and a count cannot overflow
/// anyway, being bounded by the length of one word.
#[derive(Clone, Copy)]
struct Histogram {
    ascii: [u32; 26],
    extra: [u32; MAX_EXTRA_LETTERS],
}

impl Histogram {
    const ZERO: Histogram = Histogram {
        ascii: [0; 26],
        extra: [0; MAX_EXTRA_LETTERS],
    };

    // Every method below takes `n_extra`, the number of `extra` slots the pattern
    // actually uses, and stops there rather than running the array's full
    // capacity. For an ASCII pattern that is zero, so these are precisely the
    // 26-iteration loops they were before non-ASCII letters existed — looping the
    // capacity instead cost 15% on the hybrid tier, which is one of the three
    // shapes people actually type.

    /// Per-letter maximum, which is how a hybrid pool absorbs template letters.
    fn max_with(&self, other: &Histogram, n_extra: usize) -> Histogram {
        let mut out = *self;
        for i in 0..26 {
            out.ascii[i] = out.ascii[i].max(other.ascii[i]);
        }
        for j in 0..n_extra {
            out.extra[j] = out.extra[j].max(other.extra[j]);
        }
        out
    }

    /// Total of `self - other`, per letter, floored at zero.
    fn surplus_over(&self, other: &Histogram, n_extra: usize) -> usize {
        let ascii: u32 = (0..26)
            .map(|i| self.ascii[i].saturating_sub(other.ascii[i]))
            .sum();
        let extra: u32 = (0..n_extra)
            .map(|j| self.extra[j].saturating_sub(other.extra[j]))
            .sum();
        (ascii + extra) as usize
    }

    /// Both directions of [`Histogram::surplus_over`] in one pass, as
    /// `(self - other, other - self)`.
    ///
    /// Fused because the hybrid arm needs both and the original code computed
    /// them in a single loop; splitting them doubled the per-word arithmetic on
    /// the path every dotted-plus-anagram query takes.
    fn diff_both(&self, other: &Histogram, n_extra: usize) -> (usize, usize) {
        let (mut mine, mut theirs) = (0u32, 0u32);
        for i in 0..26 {
            mine += self.ascii[i].saturating_sub(other.ascii[i]);
            theirs += other.ascii[i].saturating_sub(self.ascii[i]);
        }
        for j in 0..n_extra {
            mine += self.extra[j].saturating_sub(other.extra[j]);
            theirs += other.extra[j].saturating_sub(self.extra[j]);
        }
        (mine as usize, theirs as usize)
    }

    /// Whether `other` covers every letter `self` asks for.
    fn covered_by(&self, other: &Histogram, n_extra: usize) -> bool {
        (0..26).all(|i| self.ascii[i] == 0 || other.ascii[i] >= self.ascii[i])
            && (0..n_extra).all(|j| self.extra[j] == 0 || other.extra[j] >= self.extra[j])
    }
}

/// What one candidate word contains, measured against a pattern's alphabet.
struct Tally {
    hist: Histogram,
    /// Letters the pattern never names, so always surplus and never matchable.
    /// Counting them without identifying them is what keeps the hot path free of
    /// a map: which letter it was only matters on the confirmed-match path, where
    /// `diff_letters` can afford to spell it out.
    foreign: usize,
    /// Total letters, `foreign` included.
    len: usize,
    /// Whether the word carries anything that is not a letter — a digit, a
    /// symbol, `×`, `²`. Non-ASCII or not makes no difference, which is the rule
    /// that non-ASCII non-letters are treated exactly like ASCII ones.
    has_other: bool,
}

/// Index of `c` in `a`-`z`, if it is there.
#[inline]
fn ascii_slot(c: char) -> Option<usize> {
    c.is_ascii_lowercase().then(|| (c as u8 - b'a') as usize)
}

/// Count `chars` into a histogram over `alphabet`. Compile-time only.
fn count_chars(chars: &[char], alphabet: &[char]) -> Histogram {
    let mut h = Histogram::ZERO;
    for &c in chars {
        if let Some(i) = ascii_slot(c) {
            h.ascii[i] += 1;
        } else if let Some(j) = alphabet.iter().position(|&a| a == c) {
            h.extra[j] += 1;
        }
    }
    h
}

/// Tally the letters of `s` against `alphabet`.
///
/// The byte loop is the original ASCII one, in the original order: an ASCII
/// letter costs exactly one test, as it always did, and only a non-letter byte
/// pays a second to ask whether it is ASCII at all. Getting that order wrong —
/// testing for non-ASCII first — cost 22-46% on the hybrid anagram tier, which
/// is what a per-byte branch looks like when it lands in front of the common
/// case. Decoding starts only once a non-ASCII byte actually turns up, so a
/// folded ASCII word — the shipped list entirely, and 99.2% of a large
/// supplementary one — never decodes anything.
fn tally(s: &str, alphabet: &[char]) -> Tally {
    let mut t = Tally {
        hist: Histogram::ZERO,
        foreign: 0,
        len: 0,
        has_other: false,
    };
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if b.is_ascii_alphabetic() {
            t.hist.ascii[(b.to_ascii_lowercase() - b'a') as usize] += 1;
            t.len += 1;
        } else if b.is_ascii() {
            t.has_other = true;
        } else {
            // `i` is on a char boundary: every byte before it was ASCII.
            tally_chars(&s[i..], alphabet, &mut t);
            break;
        }
    }
    t
}

/// The general path, over characters rather than bytes. Reached only by a word
/// the fold left holding a letter outside ASCII.
fn tally_chars(rest: &str, alphabet: &[char], t: &mut Tally) {
    for c in rest.chars() {
        if let Some(k) = ascii_slot(c.to_ascii_lowercase()) {
            t.hist.ascii[k] += 1;
            t.len += 1;
        } else if c.is_alphabetic() {
            match alphabet.iter().position(|&a| a == c) {
                Some(j) => t.hist.extra[j] += 1,
                None => t.foreign += 1,
            }
            t.len += 1;
        } else {
            t.has_other = true;
        }
    }
}

fn cartesian_product(choices: &[Vec<char>]) -> Vec<Vec<char>> {
    let mut result: Vec<Vec<char>> = vec![vec![]];
    for choice in choices {
        let mut next = Vec::new();
        for existing in &result {
            for &c in choice {
                let mut combo = existing.clone();
                combo.push(c);
                next.push(combo);
            }
        }
        result = next;
    }
    result
}

/// A variable environment: `env[d]` is the lowercase letter digit `d` is bound
/// to, or 0 when it is still unbound.
///
/// Passed **by value** through the structural matcher, which is what lets a
/// failed branch be abandoned without unwinding anything — ten bytes is cheaper
/// to copy than a binding trail is to maintain.
type Env = [u8; 10];

/// The environment every pattern without digit variables matches under.
const NO_VARS: Env = [0; 10];

/// A compiled anagram pool — everything after a `;` — together with what the
/// enclosing template already accounts for.
///
/// The template's contribution lives here because the acceptance rule is a
/// statement about the two *together*: a letter the template pins is one the
/// pool does not have to supply. Keeping them in one place is also what lets
/// the structural engine reuse this arithmetic instead of growing a second copy
/// that could drift.
struct Pool {
    /// One entry per `[...]` combination: the pre-summed counter and its letter
    /// count. Built once at compile time; the per-word path never touches a
    /// `Vec` or a `HashMap` for pool accounting.
    combo_pools: Vec<(Histogram, usize)>,
    /// `.` wildcards: each licenses exactly one letter outside the pool.
    num_wildcards: usize,
    /// `*`: disables the length and surplus equalities entirely.
    has_star: bool,
    /// `(...)` groups: literal substrings the candidate must contain. Combo-
    /// independent, so checked once rather than per combination.
    contains: Vec<String>,
    /// Digit variables spent as pool letters, resolved against an [`Env`] at
    /// match time. Empty for every pattern that has no subpatterns.
    vars: Vec<u8>,
    /// Letters the enclosing template accounts for; see
    /// [`template_literal_letters`]. All zero for a pure anagram.
    template_counter: Histogram,
    /// The non-ASCII letters this pattern names, giving meaning to a
    /// [`Histogram`]'s `extra` slots. Empty for every ASCII pattern.
    alphabet: Vec<char>,
    /// True when there is no template at all (a bare `;pool`).
    is_pure: bool,
}

impl Pool {
    /// Whether the pool has anything matchable in it at all.
    fn has_content(&self) -> bool {
        self.has_star
            || self.num_wildcards > 0
            || !self.vars.is_empty()
            || self.combo_pools.iter().any(|(_, size)| *size > 0)
    }

    /// The fewest letters this pool can spend. Every `[...]` combination has
    /// the same size, so the first one speaks for all of them; an empty
    /// `combo_pools` (a `[]` group, which matches nothing) reports 0, which is
    /// a loose but valid bound.
    fn min_spend(&self) -> usize {
        let base = self.combo_pools.first().map_or(0, |(_, size)| *size);
        base + self.num_wildcards + self.vars.len()
    }

    /// The exact number of letters this pool spends, or `None` when a `*` makes
    /// it open-ended.
    fn spend(&self) -> Option<usize> {
        (!self.has_star).then(|| self.min_spend())
    }

    /// Test one `[...]` combination. `None` means "try the next one"; `Some`
    /// means the candidate matched, and carries the letters left over on each
    /// side.
    fn check_combo(
        &self,
        pool_counter: &Histogram,
        pool_base: usize,
        candidate: &Tally,
    ) -> Option<MatchInfo> {
        let pool_size = pool_base + self.num_wildcards;

        if self.is_pure && !self.has_star && candidate.len != pool_size {
            return None;
        }

        // The "effective pool" is the set of letters the word is measured against
        // when reporting unused (pool − word) and extra (word − pool) letters.
        let n_extra = self.alphabet.len();
        let effective_pool: Histogram = if self.is_pure {
            if !pool_counter.covered_by(&candidate.hist, n_extra) {
                return None;
            }

            // A foreign letter is one the pool never names, so it is surplus by
            // definition and counts toward the wildcard allowance like any other.
            let extras = candidate.hist.surplus_over(pool_counter, n_extra) + candidate.foreign;

            if !self.has_star && extras != self.num_wildcards {
                return None;
            }

            *pool_counter
        } else {
            // Hybrid: full_counter[x] = max(template_count[x], pool_count[x])
            // This models template letters being implicitly in the anagram pool.
            // Note that a star wildcard in the anagram pool means nothing in this case.
            // (The only thing it *could* mean is "ignore the anagram and do what you like",
            // which isn't very interesting.)
            let anagram_counter = self.template_counter.max_with(pool_counter, n_extra);

            // Count letters in the candidate that aren't in the anagram pool, and
            // pool letters not used by the candidate — one pass for both.
            let (surplus, unused_count) = candidate.hist.diff_both(&anagram_counter, n_extra);
            let extra_count = surplus + candidate.foreign;

            // The candidate has to use all the pool letters (a longer word)
            // or it has to use *only* pool letters (a shorter word).
            // Wildcards license a deviation from either criterion.
            // Wildcards consume pattern symbols without actually adding license,
            // until all wildcards are consumed, at which point they license non-pool letters.
            if extra_count > self.num_wildcards
                && unused_count > self.num_wildcards.saturating_sub(candidate.len)
            {
                return None;
            }

            anagram_counter
        };

        // Match confirmed. Now (and only now) do the extra work of spelling out the
        // unused (pool − word) and extra (word − pool) letters for display.
        Some(MatchInfo {
            unused: diff_letters(&effective_pool, &candidate.hist, &self.alphabet, 0),
            extra: diff_letters(
                &candidate.hist,
                &effective_pool,
                &self.alphabet,
                candidate.foreign,
            ),
        })
    }

    /// Test a candidate against every `[...]` combination, under `env`.
    fn check(&self, candidate_str: &str, env: &Env) -> Option<MatchInfo> {
        let candidate = &tally(candidate_str, &self.alphabet);

        // A pure anagram rearranges letters, so a candidate carrying a *non-letter*
        // — a digit, a symbol, `×`, `²` — is not a clean anagram and is rejected.
        // Note what is no longer grounds for rejection: a non-ASCII *letter*. Those
        // are letters like any other now, counted into `extra` if the pattern names
        // them and into `foreign` if it does not.
        if self.is_pure && candidate.has_other {
            return None;
        }

        for (base_counter, base_size) in &self.combo_pools {
            // The common case — no digit variables — hands the pre-computed
            // counter straight through without copying it.
            let found = if self.vars.is_empty() {
                self.check_combo(base_counter, *base_size, candidate)
            } else {
                let mut adjusted = *base_counter;
                for &d in &self.vars {
                    let b = env[d as usize];
                    if b == 0 {
                        // Unbound. `check_variable_binding` rules this out at
                        // compile time; degrading to "no match" keeps the hot
                        // path `Result`-free if it ever slipped through.
                        return None;
                    }
                    adjusted.ascii[(b - b'a') as usize] += 1;
                }
                self.check_combo(&adjusted, base_size + self.vars.len(), candidate)
            };
            if found.is_some() {
                // `(...)` groups are combo-independent, so this one test decides
                // for every combination at once: reaching it means some combo
                // passed, and failing it means none could have. Its *placement*
                // is load-bearing — `contains` is a substring search, and down
                // here it runs on the handful of words that already passed the
                // pool arithmetic rather than on all 83k. Hoisting it above the
                // loop, where it reads more naturally, cost 36% on
                // `;(che)rostra`.
                if !self
                    .contains
                    .iter()
                    .all(|sp| candidate_str.contains(sp.as_str()))
                {
                    return None;
                }
                return found;
            }
        }

        None
    }
}

/// The letters a template accounts for, and therefore the letters an anagram
/// pool does not have to supply — the "absorption" set folded in by
/// [`Pool::check_combo`]'s hybrid arm.
///
/// **A letter absorbs iff it occurs in a template position — before a `;` — at
/// any nesting depth. A letter in any pool, at any depth, never absorbs.** That
/// rule is what makes `(;oif)(;bel);oifblx` fail to match *foible* while
/// `(;oif)(;bel);oifblex` matches it: the `e` that `(;bel)` spends is the
/// subpattern's own business, not a letter the outer pool is excused from
/// naming. A literal is a literal wherever it sits, so the `f` of `(f..;oif)`
/// absorbs exactly as the `c` and `t` of `c.t;ao` do.
///
/// Digit variables are not letters and never absorb, the same as `.`.
fn template_literal_letters(template: &str) -> Vec<char> {
    let mut out = Vec::new();
    collect_template_letters(template, &mut out);
    out
}

fn collect_template_letters(template: &str, out: &mut Vec<char>) {
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '(' => {
                // Unclosed: leave it. The tokenizer reports it with a better
                // message than a letter scan could.
                let Some(j) = matching_paren(&chars, i) else {
                    return;
                };
                let inner: String = chars[i + 1..j].iter().collect();
                let tpl = match find_top_level(&inner, ';') {
                    Some(k) => &inner[..k],
                    None => &inner[..],
                };
                collect_template_letters(tpl, out);
                i = j;
            }
            c if c.is_alphabetic() => out.push(c.to_lowercase().next().unwrap()),
            _ => {}
        }
        i += 1;
    }
}

/// Compile the text after a `;` into a [`Pool`], against the template (if any)
/// that shares the pattern with it.
fn parse_pool(
    template: Option<&str>,
    anagram_expr: &str,
    limits: &Limits,
) -> Result<Pool, PatternError> {
    let mut fixed_letters: Vec<char> = Vec::new();
    let mut choices: Vec<Vec<char>> = Vec::new();
    let mut contains: Vec<String> = Vec::new();
    let mut vars: Vec<u8> = Vec::new();
    let mut num_wildcards: usize = 0;
    let mut has_star = false;

    let chars: Vec<char> = anagram_expr.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '[' => {
                let rel = chars[i..]
                    .iter()
                    .position(|&x| x == ']')
                    .ok_or_else(|| PatternError("Unclosed '[' in anagram".to_string()))?;
                let j = i + rel;
                choices.push(chars[i + 1..j].to_vec());
                i = j;
            }
            '(' => {
                let rel = chars[i..]
                    .iter()
                    .position(|&x| x == ')')
                    .ok_or_else(|| PatternError("Unclosed '(' in anagram".to_string()))?;
                let j = i + rel;
                let sp: String = chars[i + 1..j].iter().collect::<String>().to_lowercase();
                contains.push(sp.clone());
                fixed_letters.extend(sp.chars());
                i = j;
            }
            '.' => num_wildcards += 1,
            '*' => has_star = true,
            // A digit spends whatever letter the template bound it to. Which
            // letter that is isn't known until match time, so it is recorded as
            // a slot rather than folded into `fixed_letters`.
            c if c.is_ascii_digit() => vars.push(c as u8 - b'0'),
            c if c.is_alphabetic() => {
                fixed_letters.push(c.to_lowercase().next().unwrap());
            }
            c => {
                return Err(PatternError(format!(
                    "Anagram has meaningless character '{}'",
                    c
                )))
            }
        }
        i += 1;
    }

    // `cartesian_product` materializes every combination up front, and
    // `combo_pools` below then expands each one into 216 bytes of counter. The
    // product is multiplicative in the number of `[...]` groups, so this grows
    // out of reach long before the pattern looks unreasonable to a human:
    // `;[abcde]` eight times is 390_625 combos (~84 MB), ten times is ~9.7M
    // (~2.1 GB). Checking the size first is O(number of groups) and turns an
    // out-of-memory abort into an error message. `checked_mul` because the
    // product itself overflows `usize` at around 28 five-way groups.
    let combo_count = choices
        .iter()
        .try_fold(1usize, |acc, c| acc.checked_mul(c.len()));
    if combo_count.is_none_or(|n| n > limits.max_anagram_combos) {
        return Err(PatternError(format!(
            "Anagram is too complex: {} bracket groups expand to more than {} \
             combinations. Use fewer or smaller '[...]' groups.",
            choices.len(),
            limits.max_anagram_combos
        )));
    }

    let choice_combos: Vec<Vec<char>> = if choices.is_empty() {
        vec![vec![]]
    } else {
        cartesian_product(&choices)
    };

    let template_letters: Vec<char> = template.map(template_literal_letters).unwrap_or_default();

    // Every non-ASCII letter this pattern names, deduplicated, in first-seen
    // order. This is what gives a `Histogram`'s `extra` slots their meaning, and
    // it is empty for every ASCII pattern — which is every pattern anyone has
    // typed against the shipped word list.
    let mut alphabet: Vec<char> = Vec::new();
    for &c in fixed_letters
        .iter()
        .chain(choices.iter().flatten())
        .chain(template_letters.iter())
    {
        if !c.is_ascii() && !alphabet.contains(&c) {
            alphabet.push(c);
        }
    }
    if alphabet.len() > MAX_EXTRA_LETTERS {
        return Err(PatternError(format!(
            "Pattern names {} different non-ASCII letters; the limit is {}",
            alphabet.len(),
            MAX_EXTRA_LETTERS
        )));
    }

    let fixed_counter = count_chars(&fixed_letters, &alphabet);
    let fixed_size = fixed_letters.len();
    let combo_pools: Vec<(Histogram, usize)> = choice_combos
        .iter()
        .map(|combo| {
            let mut counter = fixed_counter;
            for &c in combo {
                if let Some(i) = ascii_slot(c.to_ascii_lowercase()) {
                    counter.ascii[i] += 1;
                } else if let Some(j) = alphabet.iter().position(|&a| a == c) {
                    counter.extra[j] += 1;
                }
            }
            (counter, fixed_size + combo.len())
        })
        .collect();

    Ok(Pool {
        combo_pools,
        num_wildcards,
        has_star,
        contains,
        vars,
        template_counter: count_chars(&template_letters, &alphabet),
        alphabet,
        is_pure: template.is_none(),
    })
}

fn compile_anagram(
    template: Option<&str>,
    anagram_expr: &str,
    limits: &Limits,
) -> Result<Matcher, PatternError> {
    if template.is_some_and(|t| t.contains('`')) {
        return Err(PatternError(
            "Fuzzy matching ('`N') is not supported in an anagram template".to_string(),
        ));
    }
    let pool = parse_pool(template, anagram_expr, limits)?;
    // A digit in the pool is one of `needs_structural`'s two triggers, so a part
    // carrying one never reaches this engine — only the structural one can
    // thread the environment a pool variable is resolved against.
    debug_assert!(
        pool.vars.is_empty(),
        "a pool variable should have routed to the structural engine"
    );
    let template_matcher: Option<Matcher> = template.map(compile_template).transpose()?;

    Ok(Box::new(move |candidate: &str| {
        if let Some(ref tm) = template_matcher {
            tm(candidate)?;
        }
        pool.check(candidate, &NO_VARS)
    }))
}

/// Build an uppercase, alphabetically-sorted string of the letters in `more` that
/// exceed `less` (per-letter, by count). Used to spell out unused and extra letters.
fn diff_letters(more: &Histogram, less: &Histogram, alphabet: &[char], foreign: usize) -> String {
    let mut out = String::new();
    for i in 0..26 {
        for _ in 0..more.ascii[i].saturating_sub(less.ascii[i]) {
            out.push((b'A' + i as u8) as char);
        }
    }
    for (j, &letter) in alphabet.iter().enumerate() {
        for _ in 0..more.extra[j].saturating_sub(less.extra[j]) {
            out.extend(letter.to_uppercase());
        }
    }
    // Letters the pattern never named are always surplus, and only the word side
    // ever has them. They were counted rather than identified on the hot path, so
    // spell them as `?` — one per letter, which is what the count is good for.
    for _ in 0..foreign {
        out.push('?');
    }
    out
}

// ---------------------------------------------------------------------------
// The structural engine: subpatterns, and variables that cross them.
//
// The fourth matching engine, after the regex template path, the fuzzy
// tokenizer and the anagram pool. It exists because a subpattern is not a
// regular constraint: `(;oif)(;bel)` asks for the word to be *cut* into pieces
// and each piece handed to a whole sub-pattern, and an anagram over a piece is
// not something a DFA or a backreference can express.
//
// It is entered only by syntax that was a hard error before it existed (see
// `needs_structural`), so nothing written against the older language can be
// rerouted here.
//
// Two properties keep it cheap. Every element's length is bounded at compile
// time, so when they are all fixed — which covers essentially every pattern a
// person writes — the cut points are forced and there is no search at all, just
// a walk. And the whole engine is ASCII-only, like the fuzzy one: with an
// ASCII-only pattern no token can consume a non-ASCII byte, so byte offsets are
// char offsets, every slice it takes is on a char boundary, and the length
// early-out is exact.
// ---------------------------------------------------------------------------

/// One position in a subpattern-bearing template.
///
/// The same vocabulary as [`FuzzTok`] (which stays untouched — the two engines
/// have different jobs) plus the two things that are new here: a variable, and
/// a nested sub-pattern.
enum Tok {
    /// A literal letter (lowercased).
    Lit(u8),
    /// Punctuation (`-`, `'`, space).
    Punct(u8),
    /// `.` — any letter.
    Any,
    /// `@` — a vowel.
    Vowel,
    /// `#` — a consonant.
    Consonant,
    /// `[abc]` — one letter from the set (lowercased bytes).
    Class(Vec<u8>),
    /// `*` — zero or more letters.
    Star,
    /// A digit variable. Binds the letter at this position on first use, and
    /// requires equality afterwards — across subpattern boundaries, which is
    /// what a single regex could not do.
    Var(u8),
    /// `(t;p)` — a whole pattern applied to a contiguous slice.
    Sub(Box<Node>),
}

/// A compiled `template;pool`, at any nesting depth.
struct Node {
    toks: Vec<Tok>,
    pool: Option<Pool>,
    /// `` `N ``: how many `Lit` positions may mismatch. 0 for an exact node.
    ///
    /// Per node, and re-armed by `match_node`, so it means the same thing here
    /// as it did when the fuzzy path was its own engine: a budget for one
    /// template, not for one word.
    fuzz: usize,
    /// True when there is no template half at all (`(;oif)`), so the token list
    /// imposes nothing and the pool alone fixes the length.
    pure: bool,
    /// Byte length this node can match. `max` is `None` when a `*` makes it
    /// open-ended.
    min: usize,
    max: Option<usize>,
    /// `max == Some(min)`: every element has a fixed width, so the slice length
    /// is decided before the walk starts.
    ///
    /// Worth a field because it lets the walker skip its length prune entirely.
    /// For a rigid node the prune is provably redundant — the caller only ever
    /// hands it a slice of exactly `min` bytes, and every token consumes one — so
    /// it is two loads and two compares per node buying nothing. That is most of
    /// what the merged fuzzy path was paying: `` cathode`1 `` walks seven rigid
    /// tokens per candidate and nothing else.
    rigid: bool,
    /// `suffix[i]` is `(min, max)` bytes the tokens from `i` onward need, with
    /// `usize::MAX` for "unbounded". Indexed up to and including `toks.len()`,
    /// so the walker can prune before looking at a token as well as after the
    /// last one.
    ///
    /// One packed slice rather than two vectors of `Option<usize>`: the walker
    /// reads both halves at every node, so this is one bounds check and one
    /// cache line instead of two of each, and the sentinel turns a discriminant
    /// branch into a compare. Worth about 10% on the cheapest patterns, which is
    /// where per-node overhead is all there is.
    suffix: Box<[(usize, usize)]>,
}

impl Node {
    /// Whether there is anything matchable in here at all. An empty group like
    /// `()` is contentless in the same sense a bare `;` is.
    fn has_content(&self) -> bool {
        self.toks.iter().any(|t| match t {
            Tok::Sub(sub) => sub.has_content(),
            _ => true,
        }) || self.pool.as_ref().is_some_and(|p| p.has_content())
    }
}

/// Stands in for "no upper bound" in a `Node`'s packed suffix table. A real
/// bound can never reach it: bounds are byte counts, summed over a token list
/// whose length is capped by `max_pattern_len`.
const UNBOUNDED: usize = usize::MAX;

/// Byte length one token can cover.
fn tok_bounds(t: &Tok) -> (usize, Option<usize>) {
    match t {
        Tok::Star => (0, None),
        Tok::Sub(n) => (n.min, n.max),
        _ => (1, Some(1)),
    }
}

/// Deepest `(...)` nesting in `pattern`.
///
/// Checked once, up front, so every recursion below it — tokenizing, letter
/// collection, compiling a node — is bounded without each having to carry a
/// depth counter. Applies to *all* parens, including the ones that get spliced
/// away for having no `;`: they cost the same compile-time recursion.
fn check_nesting_depth(pattern: &str, limits: &Limits) -> Result<(), PatternError> {
    let mut depth = 0usize;
    let mut deepest = 0usize;
    for ch in pattern.chars() {
        match ch {
            '(' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    if deepest > limits.max_subpattern_depth {
        return Err(PatternError(format!(
            "Subpatterns are nested {} deep; the limit is {}",
            deepest, limits.max_subpattern_depth
        )));
    }
    Ok(())
}

/// Tokenize the template half of a subpattern-bearing pattern.
fn tokenize_structural(template: &str, limits: &Limits) -> Result<Vec<Tok>, PatternError> {
    let mut out = Vec::new();
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '.' => out.push(Tok::Any),
            '*' => {
                // The run normalizes to its dots followed by one star, exactly
                // as on the other two engines; see `collapse_gap_run`. A `(`
                // ends the run, so this never reaches across a subpattern.
                let dots = collapse_gap_run(&chars, &mut i);
                for _ in 0..dots {
                    out.push(Tok::Any);
                }
                out.push(Tok::Star);
            }
            '@' => out.push(Tok::Vowel),
            '#' => out.push(Tok::Consonant),
            '[' => {
                let rel = chars[i..]
                    .iter()
                    .position(|&x| x == ']')
                    .ok_or_else(|| PatternError("Unclosed '[' in template".to_string()))?;
                let j = i + rel;
                let mut set = Vec::new();
                for &ch in &chars[i + 1..j] {
                    if !ch.is_ascii() {
                        return Err(PatternError(NON_ASCII_SUBPATTERN.to_string()));
                    }
                    set.push(ch.to_ascii_lowercase() as u8);
                }
                out.push(Tok::Class(set));
                i = j;
            }
            '(' => {
                let j = matching_paren(&chars, i)
                    .ok_or_else(|| PatternError("Unclosed '(' in template".to_string()))?;
                let inner: String = chars[i + 1..j].iter().collect();
                if find_top_level(&inner, ';').is_none() {
                    // No anagram inside, so the parens constrain nothing that
                    // the same text without them wouldn't: splice it in. This
                    // is what keeps `f(oo)bar` exactly as cheap as `foobar`,
                    // and it still finds a nested `(a(;bc)d)`.
                    out.extend(tokenize_structural(&inner, limits)?);
                } else {
                    out.push(Tok::Sub(Box::new(compile_node(&inner, limits)?)));
                }
                i = j;
            }
            ')' => return Err(PatternError("Unmatched ')' in template".to_string())),
            '`' => {
                return Err(PatternError(
                    "Fuzzy matching ('`N') is not supported with subpatterns".to_string(),
                ))
            }
            c if c.is_ascii_digit() => out.push(Tok::Var(c as u8 - b'0')),
            c @ ('-' | '\'' | ' ') => out.push(Tok::Punct(c as u8)),
            c if c.is_ascii_alphabetic() => out.push(Tok::Lit(c.to_ascii_lowercase() as u8)),
            c if c.is_alphabetic() => return Err(PatternError(NON_ASCII_SUBPATTERN.to_string())),
            c => {
                return Err(PatternError(format!(
                    "Template has meaningless character '{}'",
                    c
                )))
            }
        }
        i += 1;
    }
    Ok(out)
}

const NON_ASCII_SUBPATTERN: &str = "Subpatterns support only ASCII letters";

/// Compile one `template;pool`, recursively.
fn compile_node(src: &str, limits: &Limits) -> Result<Node, PatternError> {
    let (tpl, pool_src) = match find_top_level(src, ';') {
        Some(idx) => (&src[..idx], Some(&src[idx + 1..])),
        None => (src, None),
    };
    // A `` `N `` suffix belongs to this node's template. `split_fuzz` also
    // reports the malformed spellings (`` `` ``, a bare `` ` ``, a
    // non-numeric count), which must keep erroring exactly as before.
    let (tpl, fuzz) = split_fuzz(tpl)?;
    if fuzz.is_some() && pool_src.is_some() {
        return Err(PatternError(
            "Fuzzy matching ('`N') is not supported in an anagram template".to_string(),
        ));
    }
    let fuzz = fuzz.unwrap_or(0);
    let pure = pool_src.is_some() && tpl.is_empty();
    let toks = if pure {
        Vec::new()
    } else {
        tokenize_structural(tpl, limits)?
    };
    if fuzz > 0 && toks.iter().any(|t| matches!(t, Tok::Var(_))) {
        // Backreference semantics don't compose cleanly with a mismatch budget:
        // it is not clear whether a fuzzed position should still bind. Held as an
        // error rather than guessed at, which is where it has always been.
        return Err(PatternError(
            "Fuzzy matching ('`N') is not supported with digit variables".to_string(),
        ));
    }
    let pool = pool_src
        .map(|p| parse_pool(if pure { None } else { Some(tpl) }, p, limits))
        .transpose()?;

    // Suffix bounds, right to left. These are the whole reason a fixed-length
    // composition costs a walk rather than a search.
    let n = toks.len();
    let mut suffix = vec![(0usize, 0usize); n + 1];
    for i in (0..n).rev() {
        let (lo, hi) = tok_bounds(&toks[i]);
        let (rest_min, rest_max) = suffix[i + 1];
        suffix[i] = (
            rest_min + lo,
            match hi {
                Some(h) if rest_max != UNBOUNDED => h + rest_max,
                _ => UNBOUNDED,
            },
        );
    }
    let suffix: Box<[(usize, usize)]> = suffix.into_boxed_slice();

    let (min, max) = if pure {
        // No template, so the pool alone says how many letters this spends.
        let pool = pool.as_ref().expect("pure implies a pool");
        (pool.min_spend(), pool.spend())
    } else {
        let (lo, hi) = suffix[0];
        (lo, (hi != UNBOUNDED).then_some(hi))
    };

    Ok(Node {
        toks,
        pool,
        fuzz,
        pure,
        min,
        max,
        rigid: max == Some(min),
        suffix,
    })
}

/// Reject a digit variable spent in a pool that nothing binds first.
///
/// Bindings flow left to right through the token stream, and out of a
/// subpattern into the ones after it, so `(1234)(;1234)` is fine and
/// `(;1234)(1234)` is not. Deciding it here keeps the match-time path free of
/// an "unbound variable" case it would otherwise have to carry per word.
fn check_variable_binding(node: &Node) -> Result<(), PatternError> {
    let mut bound = [false; 10];
    visit_bindings(node, &mut bound)
}

fn visit_bindings(node: &Node, bound: &mut [bool; 10]) -> Result<(), PatternError> {
    for t in &node.toks {
        match t {
            Tok::Var(d) => bound[*d as usize] = true,
            Tok::Sub(sub) => visit_bindings(sub, bound)?,
            _ => {}
        }
    }
    if let Some(pool) = &node.pool {
        for &d in &pool.vars {
            if !bound[d as usize] {
                return Err(PatternError(format!(
                    "Digit variable '{}' is used in an anagram pool before anything binds it",
                    d
                )));
            }
        }
    }
    Ok(())
}

/// The slice `start..end` as a string, or `None` when those offsets do not fall
/// on char boundaries.
///
/// On a word this engine can actually match the offsets are always boundaries
/// (everything it consumes is ASCII), so this is a guard rather than a branch
/// that gets taken — but it is the guard that makes byte slicing panic-free on
/// a word list containing anything else.
fn slice_str(w: &[u8], start: usize, end: usize) -> Option<&str> {
    std::str::from_utf8(&w[start..end]).ok()
}

/// One word's worth of matching state, so the recursion carries pointers rather
/// than payload.
///
/// Everything that would otherwise be threaded through `walk`'s signature or its
/// return value lives here, because this engine now carries the fuzzy path too
/// and that path is dominated by per-node overhead. Two things in particular:
///
/// - **`walk` returns `bool`**, as `fuzzy_match` did. Returning `Option<Env>`
///   moved eleven bytes out of every node; the environment instead lives here
///   and is restored by the two arms that can change it.
/// - **The detail is accumulated in byte buffers, not `String`s.** It is
///   appended to rather than returned, and a failed branch rewinds to a recorded
///   length — and `Vec<u8>::truncate` is a length store where `String::truncate`
///   asserts a char boundary first. Every letter here comes from `diff_letters`,
///   so it is ASCII by construction.
struct Walker<'w> {
    w: &'w [u8],
    /// Nodes left in this word's budget. See `Limits::max_structural_steps`.
    steps: u32,
    /// Digit variable bindings, restored on backtracking rather than copied down.
    env: Env,
    unused: Vec<u8>,
    extra: Vec<u8>,
}

/// Lengths to rewind a `Walker`'s detail buffers to when a branch fails.
#[derive(Clone, Copy)]
struct Mark(usize, usize);

impl Walker<'_> {
    fn mark(&self) -> Mark {
        Mark(self.unused.len(), self.extra.len())
    }

    fn rewind(&mut self, m: Mark) {
        self.unused.truncate(m.0);
        self.extra.truncate(m.1);
    }

    fn push(&mut self, detail: &MatchInfo) {
        self.unused.extend_from_slice(detail.unused.as_bytes());
        self.extra.extend_from_slice(detail.extra.as_bytes());
    }

    /// The accumulated detail, once the whole word has matched.
    fn finish(self) -> MatchInfo {
        // Every byte came from `diff_letters`, which emits `b'A'..=b'Z'`.
        MatchInfo {
            unused: String::from_utf8(self.unused).unwrap_or_default(),
            extra: String::from_utf8(self.extra).unwrap_or_default(),
        }
    }

    /// Match `node`'s tokens from token `ti` / byte `ci`, filling exactly up to
    /// `end`.
    ///
    /// Recurses rather than looping, because `Star` and `Sub` both need to
    /// backtrack. Depth is bounded by `toks.len() + w.len()` — every call
    /// advances `ti` or `ci` — so it is the node *count* that needs a ceiling,
    /// not the depth; `steps` is that ceiling.
    fn walk(&mut self, node: &Node, ti: usize, ci: usize, end: usize, fuzz: usize) -> bool {
        // Bounds the nodes explored per word, re-armed for each. Both the `Star`
        // and the `Sub` arm below branch, so without this a pattern like
        // `*(;ab)*(;cd)*` is exponential with nothing underneath it to stop.
        // Exhausting the budget reports "no match", as every other match-time
        // limit in the crate does.
        if self.steps == 0 {
            return false;
        }
        self.steps -= 1;

        // What is left has to be something the remaining tokens can cover.
        // Applied before the token is even looked at, this is what collapses a
        // composition with an open-ended element to a single forced cut — and it
        // is exactly why a rigid node does not need it. See `Node::rigid`.
        if !node.rigid {
            let (need_min, need_max) = node.suffix[ti];
            let left = end - ci;
            if left < need_min || left > need_max {
                return false;
            }
        }

        if ti == node.toks.len() {
            // Either the prune above proved `ci == end`, or the node is rigid and
            // the caller sized the slice to match.
            return true;
        }

        match &node.toks[ti] {
            Tok::Star => {
                // Match zero letters here, or consume one letter and stay on the
                // star.
                let mark = self.mark();
                if self.walk(node, ti + 1, ci, end, fuzz) {
                    return true;
                }
                self.rewind(mark);
                ci < end
                    && self.w[ci].to_ascii_lowercase().is_ascii_lowercase()
                    && self.walk(node, ti, ci + 1, end, fuzz)
            }
            Tok::Sub(sub) => {
                // Only cuts that leave the rest of the tokens satisfiable are
                // worth trying, so this window is usually a single offset.
                // `end - rest_min` cannot underflow: the prune above already
                // established `end - ci >= sub.min + rest_min`.
                let rest_min = node.suffix[ti + 1].0;
                let lo = ci + sub.min;
                let hi = sub.max.map_or(end, |m| ci + m).min(end - rest_min);
                let mark = self.mark();
                let saved = self.env;
                let mut cut = lo;
                while cut <= hi {
                    if self.match_node(sub, ci, cut) && self.walk(node, ti + 1, cut, end, fuzz) {
                        return true;
                    }
                    self.rewind(mark);
                    self.env = saved;
                    cut += 1;
                }
                false
            }
            tok => {
                if ci >= end {
                    return false;
                }
                let c = self.w[ci].to_ascii_lowercase();
                let mut fuzz2 = fuzz;
                let satisfied = match tok {
                    Tok::Lit(l) => {
                        if c == *l {
                            true
                        } else if fuzz > 0 && c.is_ascii_lowercase() {
                            // A literal mismatch is allowed only while budget
                            // remains, and only onto a letter — mirroring the
                            // wildcard a freed slot effectively becomes.
                            fuzz2 = fuzz - 1;
                            true
                        } else {
                            false
                        }
                    }
                    Tok::Punct(p) => c == *p,
                    Tok::Any => c.is_ascii_lowercase(),
                    Tok::Vowel => is_vowel(c),
                    Tok::Consonant => c.is_ascii_lowercase() && !is_vowel(c),
                    Tok::Class(set) => set.contains(&c),
                    // The one arm that writes the environment, so the one arm
                    // that has to put it back: a single slot, restored in place
                    // rather than by copying the whole thing down the recursion.
                    Tok::Var(d) => {
                        let slot = *d as usize;
                        let prev = self.env[slot];
                        if !c.is_ascii_lowercase() {
                            false
                        } else if prev == 0 {
                            self.env[slot] = c;
                            if self.walk(node, ti + 1, ci + 1, end, fuzz) {
                                return true;
                            }
                            self.env[slot] = 0;
                            return false;
                        } else {
                            prev == c
                        }
                    }
                    Tok::Star | Tok::Sub(_) => unreachable!("handled above"),
                };
                satisfied && self.walk(node, ti + 1, ci + 1, end, fuzz2)
            }
        }
    }

    /// Match one whole node against the slice `start..end`.
    ///
    /// **A rigid node may only be handed a slice of exactly `node.min` bytes.**
    /// `walk` skips its per-token length prune for such a node on the strength of
    /// that, so a caller that breaks it gets a match reported before the end of
    /// the slice. Both callers hold to it — the `Sub` arm forces `cut` when
    /// `sub.min == sub.max`, and `compile_structural`'s early-out is an exact
    /// byte test — but the guarantee is invisible from inside `walk`, which is
    /// how it came to be broken once already.
    fn match_node(&mut self, node: &Node, start: usize, end: usize) -> bool {
        debug_assert!(
            !node.rigid || end - start == node.min,
            "a rigid node was handed a slice it did not size"
        );
        if self.steps == 0 {
            return false;
        }
        self.steps -= 1;

        let mark = self.mark();
        let mut pool_checked = false;

        // A pool with no variables depends only on the slice's letter counts, so
        // it is a cheap count-based filter and belongs *before* the walk. One
        // that spends variables has to wait for the template to bind them.
        if let Some(pool) = &node.pool {
            if pool.vars.is_empty() {
                match slice_str(self.w, start, end).and_then(|s| pool.check(s, &NO_VARS)) {
                    Some(detail) => self.push(&detail),
                    None => return false,
                }
                pool_checked = true;
            }
        }

        if !node.pure && !self.walk(node, 0, start, end, node.fuzz) {
            self.rewind(mark);
            return false;
        }

        if !pool_checked {
            if let Some(pool) = &node.pool {
                match slice_str(self.w, start, end).and_then(|s| pool.check(s, &self.env)) {
                    Some(detail) => self.push(&detail),
                    None => {
                        self.rewind(mark);
                        return false;
                    }
                }
            }
        }

        true
    }
}

fn compile_structural(pattern: &str, limits: &Limits) -> Result<(Matcher, bool), PatternError> {
    check_nesting_depth(pattern, limits)?;
    let node = compile_node(pattern, limits)?;
    check_variable_binding(&node)?;
    let contentless = !node.has_content();

    let min = node.min;
    // Every token but `Star` covers exactly one byte and every sub-node carries
    // its own bounds, so a star-free composition matches exactly one length.
    let fixed_len = (node.max == Some(min)).then_some(min);
    let max_steps = limits.max_structural_steps;

    let matcher: Matcher = Box::new(move |word: &str| {
        // The same length early-out the regex path uses, and for the same reason:
        // on a large list almost every word is the wrong length, so this replaces
        // the whole walk with an integer compare.
        //
        // But **without** that path's `is_ascii` caveat, which does not belong
        // here. The two count different things: `fixed_len` there is a
        // *character* count, so its exact test holds only for an ASCII word and
        // anything else has to be handed to the engine; `min`/`max` here are
        // *byte* counts, because every token consumes one ASCII byte and every
        // sub-node carries its own byte bounds. So this test is exact
        // unconditionally.
        //
        // Copying the caveat across was the `(;glo)` bug: it let `golßen` — seven
        // bytes, not three — reach a walker that `Node::rigid` had told to stop
        // checking, so the walk consumed "gol", ran out of tokens and reported a
        // match three bytes from the end of the word.
        if let Some(n) = fixed_len {
            if word.len() != n {
                return None;
            }
        } else if word.len() < min {
            return None;
        }
        // Fresh budget per word, so a pathological word can't starve later ones.
        let mut walker = Walker {
            w: word.as_bytes(),
            steps: max_steps,
            env: NO_VARS,
            unused: Vec::new(),
            extra: Vec::new(),
        };
        walker
            .match_node(&node, 0, word.len())
            .then(|| walker.finish())
    });
    Ok((matcher, contentless))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contentless_bare_semicolon() {
        // A bare `;` is an empty-pool pure anagram: it used to match every
        // zero-letter entry ("1984", "9/11", …). Now it's contentless: a note,
        // and it matches nothing (including zero-letter words).
        let c = compile_pattern_checked(";").unwrap();
        assert!(c.note.is_some());
        assert!((c.matcher)("cat").is_none());
        assert!((c.matcher)("123").is_none());
    }

    #[test]
    fn test_contentless_empty_template_conjunct() {
        // A trailing `&` leaves an empty template conjunct.
        let c = compile_pattern_checked("cat&").unwrap();
        assert!(c.note.is_some());
        assert!((c.matcher)("cat").is_none());
    }

    #[test]
    fn test_contentless_bare_bang() {
        // `!` alone is a negated empty template — contentless, matches nothing.
        let c = compile_pattern_checked("!").unwrap();
        assert!(c.note.is_some());
        assert!((c.matcher)("cat").is_none());
    }

    #[test]
    fn test_contentless_note_precedence_syntax_error_wins() {
        // A real syntax error in any part still errors, even alongside `;`.
        assert!(compile_pattern_checked(";&ca$t").is_err());
    }

    #[test]
    fn test_ordinary_patterns_have_no_note() {
        // Real content (including anagrams and wildcards) is never contentless.
        assert!(compile_pattern_checked("cat").unwrap().note.is_none());
        assert!(compile_pattern_checked(";br").unwrap().note.is_none());
        assert!(compile_pattern_checked(";.").unwrap().note.is_none());
    }

    #[test]
    fn test_anagram_dot_matches_only_clean_letters() {
        // `;.` must behave like the template `.`: exactly one letter, and nothing
        // carrying non-letter cruft. "Letter" now means letter in any script —
        // that is the rule change — so what this pins is that *non-letters* are
        // still rejected, whether they are ASCII or not.
        let m = compile_pattern(";.").unwrap();
        assert!(m("a").is_some());
        assert!(m("\u{3c9}").is_some()); // ω is a letter like any other
        assert!(m(".c").is_none()); // stray '.'
        assert!(m("3a").is_none()); // digit
        assert!(m("a!").is_none()); // symbol
        assert!(m("a\u{d7}").is_none()); // × is a non-ASCII *non-letter*
        assert!(m("ab").is_none()); // still exactly one
    }

    #[test]
    fn test_pure_anagram_rejects_non_letter_candidates() {
        // `;br` is a clean anagram of {b,r}: it matches "br" but not the junk
        // entries the old letter-only histogram used to admit.
        let m = compile_pattern(";br").unwrap();
        assert!(m("br").is_some());
        assert!(m(".br").is_none()); // stray '.'
        assert!(m("bær").is_none()); // non-ASCII letter
    }

    #[test]
    fn test_hybrid_anagram_unaffected_by_junk_rejection() {
        // The hybrid path (template + pool) is governed by its template regex and
        // must keep matching its clean candidates.
        let m = compile_pattern("z....;brae").unwrap();
        assert!(m("zebra").is_some());
    }

    #[test]
    fn test_contentless_semicolon_br_still_matches() {
        // `;br` is a proper anagram of {b,r} and is unaffected by the fix.
        let m = compile_pattern(";br").unwrap();
        assert!(m("br").is_some());
    }

    #[test]
    fn test_dot_wildcard() {
        let m = compile_pattern(".l...r.n").unwrap();
        assert!(m("electron").is_some());
    }

    #[test]
    fn test_dot_wildcard_wrong_length() {
        let m = compile_pattern(".l...r.n").unwrap();
        assert!(m("electrons").is_none());
    }

    #[test]
    fn test_dot_wildcard_wrong_letter() {
        let m = compile_pattern(".l...r.n").unwrap();
        assert!(m("xxxxxxxx").is_none());
    }

    #[test]
    fn test_fixed_letters() {
        let m = compile_pattern("cat").unwrap();
        assert!(m("cat").is_some());
        assert!(m("bat").is_none());
        assert!(m("cats").is_none());
    }

    #[test]
    fn test_case_insensitive() {
        let m = compile_pattern("cat").unwrap();
        assert!(m("Cat").is_some());
        assert!(m("CAT").is_some());
    }

    #[test]
    fn test_case_insensitive_pattern() {
        let m = compile_pattern("Cat").unwrap();
        assert!(m("Cat").is_some());
        assert!(m("cat").is_some());
        assert!(m("CAT").is_some());
    }

    #[test]
    fn test_star() {
        let m = compile_pattern("m*ja").unwrap();
        assert!(m("maharaja").is_some());
    }

    #[test]
    fn test_star_zero_chars() {
        let m = compile_pattern("m*m").unwrap();
        assert!(m("mm").is_some());
        assert!(m("mom").is_some());
        assert!(m("madam").is_some());
    }

    #[test]
    fn test_star_at_start() {
        let m = compile_pattern("*ing").unwrap();
        assert!(m("sing").is_some());
        assert!(m("running").is_some());
        assert!(m("ing").is_some());
    }

    #[test]
    fn test_star_at_end() {
        let m = compile_pattern("un*").unwrap();
        assert!(m("un").is_some());
        assert!(m("under").is_some());
    }

    #[test]
    fn test_basic_anagram() {
        let m = compile_pattern(";lobikes").unwrap();
        assert!(m("obelisk").is_some());
    }

    #[test]
    fn test_anagram_wrong_letters() {
        let m = compile_pattern(";lobikes").unwrap();
        assert!(m("oblique").is_none());
    }

    #[test]
    fn test_anagram_wrong_length() {
        let m = compile_pattern(";lobikes").unwrap();
        assert!(m("obeli").is_none());
    }

    #[test]
    fn test_anagram_flexible_length() {
        let m = compile_pattern("obel*;ski.").unwrap();
        assert!(m("obelisk").is_some());
        assert!(m("obeliskoid").is_some());
        assert!(m("obelisks").is_some());
    }

    #[test]
    fn test_anagram_underspecified() {
        let m = compile_pattern(".......;lobi").unwrap();
        assert!(m("abolish").is_some());
        assert!(m("obelisk").is_some());
    }

    #[test]
    fn test_anagram_with_wildcards() {
        let m = compile_pattern(";..oting").unwrap();
        assert!(m("tonight").is_some());
        assert!(m("tooting").is_some());
        assert!(m("outings").is_some());
    }

    #[test]
    fn test_anagram_with_wildcards_wrong_length() {
        let m = compile_pattern(";..oting").unwrap();
        assert!(m("toot").is_none());
    }

    #[test]
    fn test_anagram_choice() {
        let m = compile_pattern(";diners[ai]").unwrap();
        assert!(m("insider").is_some());
        assert!(m("sardine").is_some());
    }

    #[test]
    fn test_template_choice() {
        let m = compile_pattern("c[aou]t").unwrap();
        assert!(m("cat").is_some());
        assert!(m("cot").is_some());
        assert!(m("cut").is_some());
        assert!(m("cet").is_none());
    }

    #[test]
    fn test_hybrid_unused_pool_letters() {
        let m = compile_pattern("........;gdangboot").unwrap();
        assert!(m("toboggan").is_some());
    }

    #[test]
    fn test_hybrid_star_unused_pool_letters() {
        let m = compile_pattern("......*;gdangboot").unwrap();
        assert!(m("toboggan").is_some());
    }

    #[test]
    fn test_hybrid_template_letters_in_pool() {
        let m = compile_pattern("z....;brae").unwrap();
        assert!(m("zebra").is_some());
    }

    #[test]
    fn test_hybrid_template_letters_in_pool_no_match() {
        let m = compile_pattern("z....;brae").unwrap();
        assert!(m("zesty").is_none());
    }

    #[test]
    fn test_hybrid_short_pattern() {
        let m = compile_pattern("....*;gdangboot").unwrap();
        assert!(m("toad").is_some());
        assert!(m("toboggan").is_some());
        assert!(m("tobogganed").is_some());
        assert!(m("aeon").is_none());
        assert!(m("xxxx").is_none());
    }

    #[test]
    fn test_hybrid_short_pattern_with_wildcard() {
        let m = compile_pattern("....*;gdangboot.").unwrap();
        assert!(m("toad").is_some());
        assert!(m("toboggan").is_some());
        assert!(m("tobogganed").is_some());
        assert!(m("aeon").is_some());
        assert!(m("xxxx").is_none());
    }

    #[test]
    fn test_hybrid_wrong_letters() {
        let m = compile_pattern("......*;gdangboot").unwrap();
        assert!(m("xxxxxxxx").is_none());
        assert!(m("claggy").is_none());
        assert!(m("chemicals").is_none());
    }

    #[test]
    fn test_hybrid_template_constraint() {
        let m = compile_pattern("t*;toboggan").unwrap();
        assert!(m("toboggan").is_some());
        let reversed: String = "toboggan".chars().rev().collect();
        assert!(m(&reversed).is_none());
    }

    #[test]
    fn test_hybrid_with_redundancy() {
        let m = compile_pattern("obel...;lobikes").unwrap();
        assert!(m("obelisk").is_some());
        assert!(m("obelise").is_none());
    }

    #[test]
    fn test_palindrome_pattern() {
        let m = compile_pattern("1234321").unwrap();
        assert!(m("deified").is_some());
    }

    #[test]
    fn test_variable_mismatch() {
        let m = compile_pattern("1234321").unwrap();
        assert!(m("abcdefg").is_none());
    }

    #[test]
    fn test_repeated_variable() {
        let m = compile_pattern("1221").unwrap();
        assert!(m("abba").is_some());
        assert!(m("abcd").is_none());
    }

    #[test]
    fn test_single_variable() {
        let m = compile_pattern("11111").unwrap();
        assert!(m("aaaaa").is_some());
        assert!(m("aabaa").is_none());
    }

    #[test]
    fn test_hyphen_pattern() {
        let m = compile_pattern("...-..-.....").unwrap(); // fly-by-night (3-2-5)
        assert!(m("fly-by-night").is_some());
        assert!(m("onetofourfive").is_none());
    }

    #[test]
    fn test_no_punct_strips_words() {
        let m = compile_pattern(";lobikes").unwrap();
        assert!(m("obelisk").is_some());
    }

    #[test]
    fn test_apostrophe_in_pattern() {
        let m = compile_pattern("it's").unwrap();
        assert!(m("it's").is_some());
        assert!(m("its").is_none());
    }

    // Whitespace around `&` and after `!` is separator, not content. Before
    // `has_punct` was derived from the trimmed parts, a space anywhere in the raw
    // pattern counted as punctuation (a space is one of the three `PUNCTUATION`
    // marks), which silently turned off word punctuation-stripping for the whole
    // query — so `.... & *t` quietly matched fewer words than `....&*t`.
    #[test]
    fn test_spaces_around_conjunction_do_not_change_matches() {
        let tight = compile_pattern("....&*t").unwrap();
        let spaced = compile_pattern(".... & *t").unwrap();
        for word in ["ain't", "'bout", "abet", "cant"] {
            assert_eq!(
                tight(word).is_some(),
                spaced(word).is_some(),
                "`....&*t` and `.... & *t` disagree on {word:?}"
            );
        }
        // Specifically, both stripping: "ain't" is four letters once the
        // apostrophe goes, and ends in `t`.
        assert!(tight("ain't").is_some());
        assert!(spaced("ain't").is_some());
    }

    #[test]
    fn test_spaces_around_negation_do_not_change_matches() {
        let tight = compile_pattern("....&!*t").unwrap();
        let spaced = compile_pattern(".... & ! *t").unwrap();
        for word in ["ain't", "abet", "acre"] {
            assert_eq!(
                tight(word).is_some(),
                spaced(word).is_some(),
                "negation spelling disagrees on {word:?}"
            );
        }
    }

    // The guard against over-correcting the above: punctuation *inside* a part is
    // content, and must still suppress stripping for the whole pattern.
    //
    // `m("it's")` is the discriminating case. With stripping correctly off, the
    // word is tested as written and both parts match. Were `has_punct` false, the
    // word would arrive as "its" and the `it's` part could never match it — so
    // this fails if the fix above ever stops seeing interior punctuation. Note
    // both parts have to spell the apostrophe: `.` and `*` are letter-only, so
    // once stripping is off, a gap-only part cannot match a punctuated word at
    // all.
    #[test]
    fn test_interior_punctuation_still_suppresses_stripping() {
        for pattern in ["it's&*'*", "it's & *'*"] {
            let m = compile_pattern(pattern).unwrap();
            assert!(m("it's").is_some(), "{pattern} should match \"it's\"");
            assert!(m("its").is_none(), "{pattern} should not match \"its\"");
        }
    }

    // A leading or trailing space was already discarded by `part.trim()` before
    // the template was compiled, so counting it as punctuation made `has_punct`
    // disagree with the matcher that was actually built.
    #[test]
    fn test_surrounding_whitespace_is_not_pattern_content() {
        let padded = compile_pattern("  ....  ").unwrap();
        let bare = compile_pattern("....").unwrap();
        for word in ["ain't", "abet", "cant"] {
            assert_eq!(
                padded(word).is_some(),
                bare(word).is_some(),
                "padding changed the meaning of `....` for {word:?}"
            );
        }
    }

    #[test]
    fn test_vowel_consonant_alternation() {
        let m = compile_pattern("@#@#@#@#@#@").unwrap();
        assert!(m("imaginative").is_some());
        assert!(m("inoperative").is_some());
    }

    #[test]
    fn test_vowel() {
        let m = compile_pattern("@").unwrap();
        assert!(m("a").is_some());
        assert!(m("e").is_some());
        assert!(m("b").is_none());
    }

    #[test]
    fn test_consonant() {
        let m = compile_pattern("#").unwrap();
        assert!(m("b").is_some());
        assert!(m("z").is_some());
        assert!(m("a").is_none());
    }

    #[test]
    fn test_subpattern_match() {
        let m = compile_pattern(";(che)rostra").unwrap();
        assert!(m("orchestra").is_some());
    }

    #[test]
    fn test_subpattern_no_contiguous_match() {
        let m = compile_pattern(";(che)rostra").unwrap();
        assert!(m("carthorse").is_none());
    }

    #[test]
    fn test_and() {
        let m = compile_pattern("c.. & *at").unwrap();
        assert!(m("cat").is_some());
        assert!(m("cob").is_none());
        assert!(m("bat").is_none());
    }

    #[test]
    fn test_not() {
        let m = compile_pattern("! *ing").unwrap();
        assert!(m("cat").is_some());
        assert!(m("running").is_none());
    }

    #[test]
    fn test_not_filters_suffix() {
        let m = compile_pattern(";..oting & ! *ing").unwrap();
        let m_star_ing = compile_pattern("*ing").unwrap();
        assert!(m_star_ing("tooting").is_some());
        assert!(m("tooting").is_none());
    }

    #[test]
    fn test_multiple_and() {
        let m = compile_pattern("c* & *t & ...").unwrap();
        assert!(m("cat").is_some());
        assert!(m("cot").is_some());
        assert!(m("cut").is_some());
        assert!(m("cart").is_none());
        assert!(m("ca").is_none());
    }

    #[test]
    fn test_multiple_negations() {
        let m = compile_pattern("!c* & !*t").unwrap();
        assert!(m("box").is_some());
        assert!(m("cat").is_none());
        assert!(m("bat").is_none());
        assert!(m("cob").is_none());
    }

    #[test]
    fn test_empty_word() {
        let m = compile_pattern("*").unwrap();
        assert!(m("").is_some());
    }

    #[test]
    fn test_single_dot() {
        let m = compile_pattern(".").unwrap();
        assert!(m("a").is_some());
        assert!(m("z").is_some());
        assert!(m("ab").is_none());
    }

    #[test]
    fn test_only_star() {
        let m = compile_pattern("*").unwrap();
        assert!(m("anything").is_some());
        assert!(m("").is_some());
    }

    #[test]
    fn test_invalid_pattern_unclosed_bracket() {
        assert!(compile_pattern("c[at").is_err());
    }

    #[test]
    fn test_invalid_pattern_meaningless_char() {
        assert!(compile_pattern("ca$t").is_err());
    }

    #[test]
    fn test_match_info_exact_anagram_empty() {
        let m = compile_pattern(";obelisk").unwrap();
        let info = m("obelisk").unwrap();
        assert_eq!(info.unused, "");
        assert_eq!(info.extra, "");
    }

    #[test]
    fn test_match_info_pure_wildcard_extra() {
        let m = compile_pattern(";..oting").unwrap();
        let info = m("tonight").unwrap();
        assert_eq!(info.unused, "");
        assert_eq!(info.extra, "HT");
    }

    #[test]
    fn test_match_info_hybrid_unused() {
        let m = compile_pattern("........;gdangboot").unwrap();
        let info = m("toboggan").unwrap();
        assert_eq!(info.unused, "D");
        assert_eq!(info.extra, "");
    }

    #[test]
    fn test_match_info_hybrid_exact_empty() {
        let m = compile_pattern("z....;brae").unwrap();
        let info = m("zebra").unwrap();
        assert_eq!(info.unused, "");
        assert_eq!(info.extra, "");
    }

    #[test]
    fn test_match_info_non_match_is_none() {
        let m = compile_pattern("........;gdangboot").unwrap();
        assert!(m("xxxxxxxx").is_none());
    }

    #[test]
    fn test_fuzz_one_mismatch() {
        let m = compile_pattern("cat`1").unwrap();
        assert!(m("cat").is_some()); // zero mismatches still match
        assert!(m("bat").is_some());
        assert!(m("car").is_some());
        assert!(m("cot").is_some());
        assert!(m("cog").is_none()); // two mismatches
        assert!(m("dog").is_none()); // three mismatches
    }

    #[test]
    fn test_fuzz_enforces_length() {
        let m = compile_pattern("cat`1").unwrap();
        assert!(m("cats").is_none());
        assert!(m("ca").is_none());
        assert!(m("brat").is_none());
    }

    #[test]
    fn test_fuzz_two_mismatches() {
        let m = compile_pattern("cat`2").unwrap();
        assert!(m("cog").is_some()); // two mismatches
        assert!(m("dog").is_none()); // three mismatches
    }

    #[test]
    fn test_fuzz_zero_is_exact() {
        let m = compile_pattern("cat`0").unwrap();
        assert!(m("cat").is_some());
        assert!(m("bat").is_none());
    }

    #[test]
    fn test_fuzz_case_insensitive() {
        let m = compile_pattern("CAT`1").unwrap();
        assert!(m("bat").is_some());
        assert!(m("Bat").is_some());
    }

    #[test]
    fn test_fuzz_with_wildcard() {
        // The '.' must always match a letter; only the literals are fuzzable.
        let m = compile_pattern(".at`1").unwrap();
        assert!(m("bat").is_some()); // wildcard b, exact at
        assert!(m("cot").is_some()); // wildcard c, a->o is the one allowed miss
        assert!(m("cob").is_none()); // a->o and t->b: two misses
    }

    #[test]
    fn test_fuzz_with_star() {
        let m = compile_pattern("*ing`1").unwrap();
        assert!(m("sing").is_some());
        assert!(m("sang").is_some()); // i->a, one miss in the literal tail
        assert!(m("running").is_some());
        assert!(m("sank").is_none()); // i->a and g->k: two misses
    }

    #[test]
    fn test_fuzz_with_char_class() {
        // The class stays rigid; only the literal 't' is fuzzable.
        let m = compile_pattern("c[aou]t`1").unwrap();
        assert!(m("cat").is_some());
        assert!(m("cap").is_some()); // t->p is the allowed miss
        assert!(m("cet").is_none()); // 'e' not in the class (rigid), no budget for it
    }

    #[test]
    fn test_fuzz_rejects_digit_variable() {
        assert!(compile_pattern("121`1").is_err());
    }

    #[test]
    fn test_fuzz_rejected_in_anagram() {
        assert!(compile_pattern("cat`1;xyz").is_err());
    }

    #[test]
    fn test_fuzz_missing_number() {
        assert!(compile_pattern("cat`").is_err());
    }

    #[test]
    fn test_fuzz_bad_number() {
        assert!(compile_pattern("cat`x").is_err());
    }

    #[test]
    fn test_fuzz_multiple_backticks() {
        assert!(compile_pattern("cat`1`2").is_err());
    }

    // --- Work limits ------------------------------------------------------
    //
    // Each of these covers a path that was previously unbounded and could hang
    // or exhaust memory on a short, plausible-looking pattern. Every case
    // asserts both halves: rejected/degraded under tight limits, and *unchanged*
    // under the defaults, so the limiters can't silently narrow the language.

    /// Deliberately tight limits, standing in for what a server would use.
    fn tight() -> Limits {
        Limits {
            max_pattern_len: 64,
            max_anagram_combos: 4_096,
            max_structural_steps: 10_000,
            ..Limits::default()
        }
    }

    #[test]
    fn test_anagram_combo_explosion_is_rejected() {
        // 5^8 = 390_625 combos ≈ 84 MB of `combo_pools` before this cap existed.
        let pat = format!(";{}", "[abcde]".repeat(8));
        // `Matcher` isn't `Debug`, so match rather than `unwrap_err`.
        match compile_pattern_with(&pat, &tight()) {
            Err(e) => assert!(e.to_string().contains("too complex"), "unexpected: {e}"),
            Ok(_) => panic!("combo explosion was not rejected"),
        }
    }

    #[test]
    fn test_anagram_combo_explosion_rejected_by_default_too() {
        // The default limit is generous but finite: ten five-way groups is
        // ~9.7M combos (~2.1 GB), which must not be attempted even interactively.
        let pat = format!(";{}", "[abcde]".repeat(10));
        assert!(compile_pattern(&pat).is_err());
    }

    #[test]
    fn test_modest_anagram_classes_still_compile() {
        // The shapes a person actually types must be untouched by the cap.
        assert!(compile_pattern(";diners[ai]").is_ok());
        assert!(compile_pattern_with(";diners[ai]", &tight()).is_ok());
        assert!(compile_pattern(";[abc][abc][abc]").is_ok());
    }

    #[test]
    fn test_combo_count_overflow_is_rejected_not_wrapped() {
        // Enough groups that the product overflows `usize`; `checked_mul` must
        // catch this rather than wrapping to a small number and being allowed.
        let pat = format!(";{}", "[abcde]".repeat(40));
        assert!(compile_pattern(&pat).is_err());
    }

    #[test]
    fn test_pattern_length_cap() {
        let long = "a".repeat(100);
        assert!(compile_pattern_with(&long, &tight()).is_err());
        // Well under the 1024-byte default, so ordinary use is unaffected.
        assert!(compile_pattern(&long).is_ok());
    }

    #[test]
    fn test_star_heavy_template_stays_bounded() {
        // Previously up to 1M backtracking steps per word across the whole list.
        // Under a tight limit the matcher must still *build* and still *return* —
        // degrading to "no match" rather than hanging.
        let m = compile_pattern_with(&format!("{}cat", "*".repeat(10)), &tight()).unwrap();
        let _ = m("a".repeat(40).as_str());
    }

    #[test]
    fn test_star_template_matching_is_unchanged_under_defaults() {
        // The backtrack limit must not narrow what the language matches.
        let m = compile_pattern("m*ja").unwrap();
        assert!(m("maharaja").is_some()); // the star spans several letters
        assert!(m("mja").is_some()); // and zero letters
        assert!(m("marijuana").is_none()); // anchored: must end in "ja"
    }

    #[test]
    fn test_fuzzy_star_stays_bounded() {
        // A hand-rolled backtracker whose `Star` arm branches exponentially and
        // had no step budget at all before this. The engine underneath moved
        // from `fuzzy_match` to `walk`; the exposure did not.
        let m = compile_pattern_with(&format!("{}cat`3", "*".repeat(12)), &tight()).unwrap();
        let _ = m("abcdefghijklmnopqrstuvwxyz");
    }

    #[test]
    fn test_fuzzy_matching_is_unchanged_under_defaults() {
        let m = compile_pattern("cat`1").unwrap();
        assert!(m("cat").is_some());
        assert!(m("bat").is_some());
        assert!(m("bar").is_none());
    }

    #[test]
    fn test_fuzzy_step_budget_is_per_word_not_per_scan() {
        // A pathological word must not exhaust the budget for words after it.
        let m = compile_pattern(&format!("{}cat`1", "*".repeat(6))).unwrap();
        let pathological = "a".repeat(60);
        let _ = m(&pathological);
        assert!(m("cat").is_some(), "budget leaked across words");
    }

    // --- The fuzz path's fixed-length early-out ----------------------------
    //
    // A star-free fuzzy template matches only words of exactly `toks.len()`
    // bytes, so the closure rejects everything else without recursing. That is a
    // large share of the scan, and it must be a *pure* filter: nothing it drops
    // could have matched.

    #[test]
    fn test_fuzzy_early_out_keeps_every_same_length_match() {
        let m = compile_pattern("cat`1").unwrap();
        // Exact, and one mismatch in each of the three positions.
        for w in ["cat", "bat", "cot", "car"] {
            assert!(m(w).is_some(), "{w} should match within the fuzz budget");
        }
        // Two mismatches exceeds the budget — rejected on merit, not on length.
        assert!(m("bar").is_none());
    }

    #[test]
    fn test_fuzzy_early_out_rejects_only_on_length() {
        let m = compile_pattern("cat`2").unwrap();
        // Two mismatched positions is exactly the budget; the length is what
        // the filter keys on, and fuzz never changes it — it lets a position
        // mismatch, not disappear.
        assert!(m("dot").is_some(), "differs in 2 of 3 positions");
        assert!(m("ca").is_none());
        assert!(m("cats").is_none());
    }

    #[test]
    fn test_fuzzy_early_out_is_not_confused_by_multibyte_words() {
        // The filter compares *byte* lengths, and the matcher is byte-indexed,
        // so the two agree. "café" is 5 bytes but 4 chars; under a 4-token
        // template it must be rejected, and it could not have matched anyway
        // because every matcher arm requires an ASCII byte.
        let m = compile_pattern("cafe`1").unwrap();
        assert!(m("cafe").is_some());
        assert!(m("café").is_none());
        // And a 5-token template must not accidentally admit it via byte length.
        let m5 = compile_pattern("cafes`1").unwrap();
        assert!(m5("café").is_none());
    }

    #[test]
    fn test_fuzzy_early_out_does_not_apply_to_star_templates() {
        // A star makes the length variable, so the filter must switch off.
        let m = compile_pattern("c*t`1").unwrap();
        assert!(m("ct").is_some());
        assert!(m("cat").is_some());
        assert!(m("comet").is_some());
    }

    /// The largest `max_structural_steps` `limitcal`'s *adversarial* tier needs
    /// to return every match (`` *a*b*c*d*`2 ``). Re-run that example before
    /// changing it.
    const STRUCTURAL_FLOOR: u32 = 3_053;

    /// What `*1*2*3*4*1*2*3*4*` needs. Called out separately because it is the
    /// shape the old regex path could not return in full at any practical
    /// ceiling — 566 of 579 matches in 1_972 ms — and covering it is a
    /// correctness claim, not headroom.
    const STRUCTURAL_EXTREME: u32 = 17_821;

    // --- The recalibrated match-time defaults ------------------------------

    #[test]
    fn test_default_match_time_limits_clear_the_calibrated_floors() {
        // `examples/limitcal.rs` measures the smallest limit that still returns
        // every match, over a corpus of realistic and adversarial patterns. This
        // is the tripwire against lowering a default into the range where it
        // would start silently dropping real matches.
        let d = Limits::default();
        assert!(
            d.max_structural_steps >= STRUCTURAL_FLOOR * 4,
            "max_structural_steps {} leaves too little headroom over the measured floor",
            d.max_structural_steps
        );
        assert!(
            d.max_structural_steps >= STRUCTURAL_EXTREME,
            "max_structural_steps {} would truncate *1*2*3*4*1*2*3*4*, which is \
             the pattern this engine exists to have fixed",
            d.max_structural_steps
        );
    }

    #[test]
    fn test_star_heavy_fuzzy_still_matches_under_defaults() {
        // The shape that needed the most steps in calibration. Under the lowered
        // default it must still find its matches, not degrade to "no match".
        let m = compile_pattern("*a*b*c*d*`2").unwrap();
        assert!(m("abcd").is_some());
        assert!(m("aXbXcXdX").is_some());
        assert!(m("unpredictability").is_some());
    }

    // --- Consecutive stars collapse -----------------------------------------
    //
    // `*` is "zero or more letters", so a run of them accepts exactly what one
    // accepts. Collapsing is a pure performance change on the regex and fuzzy
    // paths (the anagram path already folds stars into a `has_star` bool), and
    // the cases below pin both halves: same language, and the shapes that used
    // to be pathological now aren't.

    #[test]
    fn test_star_runs_accept_the_same_language() {
        for (many, one) in [
            ("**cat", "*cat"),
            ("***cat", "*cat"),
            ("**a**e**", "*a*e*"),
            ("c**t", "c*t"),
            ("****", "*"),
            ("cat**", "cat*"),
        ] {
            let m = compile_pattern(many).unwrap();
            let o = compile_pattern(one).unwrap();
            for w in ["cat", "ct", "concat", "scatter", "a", "", "acute", "cate"] {
                assert_eq!(
                    m(w).is_some(),
                    o(w).is_some(),
                    "{many} and {one} disagree on {w:?}"
                );
            }
        }
    }

    #[test]
    fn test_star_runs_collapse_on_the_fuzzy_path_too() {
        let m = compile_pattern("**cat`1").unwrap();
        let o = compile_pattern("*cat`1").unwrap();
        for w in ["cat", "bat", "concat", "wombat", "ct", "cart"] {
            assert_eq!(m(w).is_some(), o(w).is_some(), "disagreement on {w:?}");
        }
    }

    #[test]
    fn test_star_inside_a_character_class_is_not_a_star() {
        // The `[` arm consumes to `]`, so a `*` in there is a class member and
        // must never reach the collapsing helper. `[a*b]` matches one literal
        // asterisk-or-a-or-b; no word has an asterisk, so it behaves as `[ab]`.
        let m = compile_pattern("c[a*b]t").unwrap();
        assert!(m("cat").is_some());
        assert!(m("cbt").is_some());
        assert!(m("cot").is_none());
        // And it must not have been treated as a variable-width construct.
        assert!(m("coat").is_none());
    }

    #[test]
    fn test_collapsing_rescues_a_previously_budget_bound_pattern() {
        // `**********1**********1` exceeded the per-word limit on many words and
        // degraded them to "no match" — silently returning a fraction of the
        // real matches. Collapsed to `*1*1` it is bounded, so the two spellings
        // must agree everywhere, including on long words. Still true after the
        // pattern moved to the structural engine, which is the point of testing
        // the equivalence rather than a step count.
        let many = compile_pattern(&format!("{}1{}1", "*".repeat(10), "*".repeat(10))).unwrap();
        let one = compile_pattern("*1*1").unwrap();
        for w in [
            "banana",
            "kayak",
            "cat",
            "unpredictability",
            "antidisestablishmentarianism",
        ] {
            assert_eq!(
                many(w).is_some(),
                one(w).is_some(),
                "star run and collapsed form disagree on {w:?}"
            );
        }
        assert!(one("banana").is_some(), "sanity: `*1*1` matches banana");
    }

    #[test]
    fn test_star_run_collapse_preserves_variable_length() {
        // Collapsing must not let the fixed-length early-out switch on: a star,
        // however many times written, still makes the length variable.
        let m = compile_pattern("**cat**`1").unwrap();
        assert!(m("cat").is_some());
        assert!(m("concatenate").is_some());
    }

    /// Words spanning the lengths a short gap run can discriminate, plus some
    /// that exercise a literal tail.
    const GAP_PROBE_WORDS: &[&str] = &[
        "",
        "a",
        "ab",
        "abc",
        "abcd",
        "abcde",
        "abcdef",
        "abcdefg",
        "cat",
        "acat",
        "aacat",
        "aaacat",
        "aaaacat",
        "aaaaacat",
        "aaaaaacat",
        "bat",
        "aabat",
        "concat",
    ];

    #[test]
    fn test_every_gap_run_normalizes_exactly() {
        // A maximal run of `.`/`*` with k dots and at least one star accepts
        // exactly the words of length >= k, whatever the interleaving — so it
        // must behave identically to `.`xk followed by one `*`. Enumerate every
        // such run up to length 5 (2^5 interleavings each) against both a bare
        // and a literal-tailed form, on both the regex and the structural path.
        for len in 1..=5usize {
            for bits in 0..(1u32 << len) {
                let run: String = (0..len)
                    .map(|b| if (bits >> b) & 1 == 1 { '.' } else { '*' })
                    .collect();
                if !run.contains('*') {
                    continue; // a dots-only run is already its own normal form
                }
                let dots = run.chars().filter(|&c| c == '.').count();
                let normalized = format!("{}*", ".".repeat(dots));
                for tail in ["", "cat", "cat`1"] {
                    let a = compile_pattern(&format!("{run}{tail}")).unwrap();
                    let b = compile_pattern(&format!("{normalized}{tail}")).unwrap();
                    for w in GAP_PROBE_WORDS {
                        assert_eq!(
                            a(w).is_some(),
                            b(w).is_some(),
                            "{run}{tail} and {normalized}{tail} disagree on {w:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_gap_run_normalization_respects_the_minimum_length() {
        // The rewrite must not lose the dots' length floor: `*.*.*` requires two
        // letters, and is not the same as `*`.
        let m = compile_pattern("*.*.*").unwrap();
        assert!(m("ab").is_some());
        assert!(m("abc").is_some());
        assert!(m("a").is_none());
        assert!(m("").is_none());
    }

    #[test]
    fn test_gap_symbols_inside_a_character_class_are_literals() {
        // Neither symbol may reach the collapser from inside `[...]`; both are
        // ordinary class members there, so the position stays exactly one char.
        let m = compile_pattern("c[a.b]t").unwrap();
        assert!(m("cat").is_some());
        assert!(m("cbt").is_some());
        assert!(m("cot").is_none());
        assert!(m("coat").is_none(), "class must not become variable-width");
    }

    #[test]
    fn test_dot_star_runs_are_normalized_on_the_fuzzy_path() {
        // The star-only collapse is trivially defeated by putting dots between
        // the stars; this is the case that closes. `*.*.*.*.*cat`1` cost 280 ms
        // per scan of the full list before, and 5.3 ms as `....*cat`1`.
        let many = compile_pattern("*.*.*.*.*cat`1").unwrap();
        let one = compile_pattern("....*cat`1").unwrap();
        for w in [
            "aaaacat",
            "aaaabat",
            "cat",
            "aaacat",
            "unpredictability",
            "aaaaaaaaaacat",
        ] {
            assert_eq!(many(w).is_some(), one(w).is_some(), "disagreement on {w:?}");
        }
    }

    #[test]
    fn test_backreference_star_still_matches_under_defaults() {
        // `*1*1` is the worst *realistic* backtracking shape (193 steps).
        let m = compile_pattern("*1*1").unwrap();
        assert!(m("banana").is_some());
        assert!(m("kayak").is_some());
    }

    // --- Unicode: canonicalize for matching, preserve for display ---
    //
    // These call the matcher directly, so they pass words already in canonical
    // form — that is what `search` hands it. The load-time half is tested in
    // `dictionary`, and the fold itself in `fold`.

    #[test]
    fn test_pattern_is_folded_before_it_is_compiled() {
        // Both sides of the comparison are canonicalized, so how the user spells
        // the pattern cannot matter. All three of these are the same matcher.
        for pat in ["elan", "\u{c9}LAN", "\u{e9}lan"] {
            let m = compile_pattern(pat).unwrap();
            assert!(m("elan").is_some(), "{pat} should match the folded `elan`");
        }
        // And a pattern can name a multigraph letter directly.
        assert!(compile_pattern("\u{c6}r\u{f8}").unwrap()("aero").is_some());
    }

    #[test]
    fn test_dot_and_star_mean_any_letter() {
        // Rule 2: a non-Latin letter is a letter, so the wildcards have to admit
        // it. `[a-z]` was the same thing only while non-ASCII words could not
        // reach the matcher at all.
        assert!(compile_pattern(".....").unwrap()("\u{3c9}\u{3bc}\u{3b5}\u{3b3}\u{3b1}").is_some());
        assert!(compile_pattern("*").unwrap()("\u{44f}\u{44f}").is_some());
        assert!(compile_pattern("..").unwrap()("\u{6f22}\u{5b57}").is_some());
        // A non-letter is still not a letter, ASCII or not.
        assert!(compile_pattern(".").unwrap()("\u{d7}").is_none());
        assert!(compile_pattern(".").unwrap()("7").is_none());
    }

    #[test]
    fn test_vowel_and_consonant_stay_latin() {
        // Deliberately *not* extended: "is ω a vowel" has no locale-free answer,
        // so `@`/`#` keep their ASCII sets and a Greek word matches neither.
        assert!(compile_pattern("#@#").unwrap()("cat").is_some());
        assert!(compile_pattern("#@#").unwrap()("\u{3c9}\u{3bc}\u{3b5}").is_none());
        assert!(compile_pattern("@").unwrap()("\u{3b1}").is_none());
    }

    #[test]
    fn test_non_latin_letters_are_distinct_from_latin_ones() {
        // The other half of rule 2, and the reason a transliterating fold would
        // be wrong: ω must not answer to `o`.
        assert!(compile_pattern("omega").unwrap()("\u{3c9}\u{3bc}\u{3b5}\u{3b3}\u{3b1}").is_none());
        assert!(compile_pattern(";o").unwrap()("\u{3c9}").is_none());
        // It answers to itself, spelled either way round in an anagram.
        assert!(
            compile_pattern(";\u{3b1}\u{3b2}\u{3b3}").unwrap()("\u{3b3}\u{3b2}\u{3b1}").is_some()
        );
        assert!(compile_pattern(";\u{3b1}\u{3b2}\u{3b3}").unwrap()("\u{3b1}\u{3b2}").is_none());
    }

    #[test]
    fn test_anagram_mixes_scripts_and_reports_foreign_letters() {
        // A pool naming non-ASCII letters gets `extra` slots for exactly those;
        // any other letter is *foreign* — counted on the hot path, identified
        // only here, where spelling it out is affordable.
        let m = compile_pattern("...;\u{3b1}\u{3b2}\u{3b3}").unwrap();
        let info = m("\u{3b2}\u{3b1}\u{3b3}").expect("an exact anagram should match");
        assert_eq!(info.unused, "");
        assert_eq!(info.extra, "");
        // A hybrid pool tolerating one outside letter reports it as `?`.
        let m = compile_pattern("....;\u{3b1}\u{3b2}\u{3b3}.").unwrap();
        let info = m("\u{3b1}\u{3b2}\u{3b3}z").expect("one wildcard licenses one outsider");
        assert_eq!(info.extra, "Z");
    }

    #[test]
    fn test_non_ascii_non_letters_behave_like_ascii_ones() {
        // Rule 1: nothing new here on purpose. `×` and `²` are exactly as
        // unmatchable as `/` and `7` always were.
        for junk in ["a\u{d7}", "a\u{b2}", "a\u{b0}", "a/", "a7"] {
            assert!(
                compile_pattern(";ab").unwrap()(junk).is_none(),
                "{junk:?} should not be a clean anagram"
            );
        }
    }

    #[test]
    fn test_too_many_non_ascii_letters_is_a_compile_error() {
        // `Histogram::extra` is a stack array, so the alphabet has a hard ceiling.
        // Exceeding it is a normal `PatternError`, never a silent truncation.
        let many: String = (0x3b1..0x3b1 + MAX_EXTRA_LETTERS as u32 + 1)
            .filter_map(char::from_u32)
            .collect();
        assert!(compile_pattern(&format!(";{many}")).is_err());
        // One under the cap is fine.
        let ok: String = (0x3b1..0x3b1 + MAX_EXTRA_LETTERS as u32)
            .filter_map(char::from_u32)
            .collect();
        assert!(compile_pattern(&format!(";{ok}")).is_ok());
    }

    #[test]
    fn test_structural_engine_stays_ascii_only() {
        // The documented hold: the walker is byte-indexed, and that is what makes
        // byte offsets char offsets. So a non-Latin word is reachable by `.....`
        // but not by the same pattern with a fuzz suffix or a subpattern. Pinned
        // so that lifting it later is a deliberate change rather than a surprise.
        let greek = "\u{3c9}\u{3bc}\u{3b5}\u{3b3}\u{3b1}";
        assert!(compile_pattern(".....").unwrap()(greek).is_some());
        assert!(compile_pattern(".....`1").unwrap()(greek).is_none());
        assert!(compile_pattern("(.....)").unwrap()(greek).is_none());
    }

    // --- Subpatterns: `(...)` in the template half ---

    #[test]
    fn test_subpattern_adjacent_anagram_blocks() {
        // The point of the feature: cut the word and hand each piece to a whole
        // sub-pattern. FOIBLE is FOI + BLE, an anagram of OIF and one of BEL.
        let m = compile_pattern("(;oif)(;bel)").unwrap();
        assert!(m("foible").is_some());
        // Right letters, wrong side of the cut.
        assert!(m("belfoi").is_none());
        // The cut is fixed at 3+3, so a longer or shorter word cannot match
        // however its letters are arranged.
        assert!(m("foibles").is_none());
        assert!(m("foibl").is_none());
    }

    #[test]
    fn test_subpattern_block_with_its_own_template() {
        // A block is a whole pattern, so it can carry a template of its own —
        // both the pure-wildcard form and one pinning a letter.
        for pat in ["(...;oif)(;bel)", "(f..;oif)(;bel)"] {
            let m = compile_pattern(pat).unwrap();
            assert!(m("foible").is_some(), "{pat} should match foible");
        }
        // `o..` pins the wrong letter first: FOI starts with F.
        assert!(compile_pattern("(o..;oif)(;bel)").unwrap()("foible").is_none());
    }

    #[test]
    fn test_subpattern_beside_ordinary_template_tokens() {
        // Blocks compose with everything else in a template.
        assert!(compile_pattern("...(;bel)").unwrap()("foible").is_some());
        assert!(compile_pattern("(;el)(;bo)w").unwrap()("elbow").is_some());
        assert!(compile_pattern("(;el)(;bo)x").unwrap()("elbow").is_none());
    }

    #[test]
    fn test_subpattern_open_ended_block_searches_for_the_cut() {
        // With a star the cut point is not forced, so the matcher has to try
        // offsets. `*(;bel)` is "ends in some arrangement of B, E, L".
        let m = compile_pattern("*(;bel)").unwrap();
        assert!(m("able").is_some());
        assert!(m("foible").is_some());
        assert!(m("belfry").is_none());
    }

    #[test]
    fn test_subpattern_without_an_anagram_is_inlined() {
        // Parens with no `;` constrain nothing the same text without them
        // wouldn't, so they are spliced away rather than compiled into a block.
        for (parens, plain) in [
            ("ele(ph)ant", "elephant"),
            ("(c.t)", "c.t"),
            ("f(o)(o)d", "food"),
            ("(1)(1)", "11"),
        ] {
            let a = compile_pattern(parens).unwrap();
            let b = compile_pattern(plain).unwrap();
            for w in ["elephant", "cat", "food", "aa", "ii", "cot", "foible"] {
                assert_eq!(
                    a(w).is_some(),
                    b(w).is_some(),
                    "{parens} and {plain} disagree on {w}"
                );
            }
        }
    }

    #[test]
    fn test_subpattern_nesting() {
        // A block's template can hold another block. `a(;bc)d` is A, then two
        // letters spelling BC in some order, then D — and the outer pool then
        // measures the whole four.
        let m = compile_pattern("(a(;bc)d;abcd)").unwrap();
        assert!(m("abcd").is_some());
        assert!(m("acbd").is_some());
        assert!(m("abdc").is_none());
    }

    #[test]
    fn test_subpattern_reports_its_own_unused_letters() {
        // `MatchInfo` from inside a block is folded into the aggregate, the same
        // way the `&`-parts fold theirs. `(...;oifx)` spends O, I and F on FOI
        // and leaves the X over.
        let m = compile_pattern("(...;oifx)(;bel)").unwrap();
        let info = m("foible").expect("foible should match");
        assert_eq!(info.unused, "X");
        assert_eq!(info.extra, "");
    }

    // --- Subpatterns and a whole-word anagram together ---

    #[test]
    fn test_subpattern_absorption_rule() {
        // The spec, case by case, against the word FOIBLE = {b,e,f,i,l,o}.
        //
        // A letter absorbs — excuses the pool from naming it — iff it sits in a
        // *template* position, before a `;`, at any depth. A letter in any pool,
        // at any depth, never absorbs. With no wildcards the acceptance rule
        // reduces to "the word uses only pool letters, or it uses all of them":
        //
        //   pattern                    pool          extra   unused  verdict
        //   ...(;bel);oif              o i f         b l e   -       match
        //   ...(;bel);oifb             o i f b       l e     -       match
        //   (;oif)(;bel);oifb          o i f b       l e     -       match
        //   (;oif)(;bel);oifblex       o i f b l e x -       x       match
        //   (;oif)(;bel);oifblx        o i f b l x   e       x       no
        //   (;oif)(;bel);x             x             6       x       no
        //
        // The last two are the ones that pin the rule down. Were the `e` that
        // `(;bel)` spends allowed to absorb, `;oifblx` would match too.
        for pat in [
            "...(;bel);oif",
            "...(;bel);oifb",
            "(;oif)(;bel);oifb",
            "(;oif)(;bel);oifblex",
        ] {
            assert!(
                compile_pattern(pat).unwrap()("foible").is_some(),
                "{pat} should match foible"
            );
        }
        for pat in ["(;oif)(;bel);oifblx", "(;oif)(;bel);x"] {
            assert!(
                compile_pattern(pat).unwrap()("foible").is_none(),
                "{pat} should not match foible"
            );
        }
    }

    #[test]
    fn test_subpattern_template_literal_still_absorbs() {
        // A literal is a literal wherever it sits: the `f` of `(f..;oif)` is in
        // a template position, so it absorbs exactly as the `c` and `t` of
        // `c.t;ao` do.
        assert!(compile_pattern("(f..;oif)(;bel);oifb").unwrap()("foible").is_some());
        // And the existing top-level behaviour it is modelled on is unchanged.
        assert!(compile_pattern("c.t;ao").unwrap()("cat").is_some());
    }

    // --- Variables that cross a subpattern boundary ---

    #[test]
    fn test_variable_bound_in_one_block_spent_in_the_next() {
        // A digit is one variable across the whole part, not one per regex, so
        // `(1234)` binds four letters over REAP and `(;1234)` then requires the
        // rest of the word to be an anagram of those same four.
        let m = compile_pattern("(1234)(;1234)").unwrap();
        assert!(m("reappear").is_some());
        assert!(m("teammate").is_some());
        // Eight letters, but the second half is not a rearrangement of the first.
        assert!(m("elephant").is_none());
    }

    #[test]
    fn test_variable_in_a_pool_beside_a_plain_template() {
        // No block needed for the variable itself — the pool digit is what
        // routes this to the structural engine.
        let m = compile_pattern("c(1)t;1").unwrap();
        assert!(m("cat").is_some());
        assert!(m("cot").is_some());
        assert!(m("cast").is_none());
    }

    #[test]
    fn test_variable_repeats_across_blocks() {
        // The same digit in two different blocks is the same letter, so both
        // halves here have to be an arrangement of ABC *starting with the same
        // letter*.
        let m = compile_pattern("(1..;abc)(1..;abc)").unwrap();
        assert!(m("abcacb").is_some());
        // Right letters on both sides, but the second half starts with `c`.
        assert!(m("abccba").is_none());
    }

    #[test]
    fn test_pool_variable_must_be_bound_first() {
        // Bindings flow left to right, so a pool cannot spend a variable that
        // nothing has bound yet. Decided at compile time, which keeps the
        // per-word path free of an "unbound" case.
        assert!(compile_pattern("(;1234)(1234)").is_err());
        assert!(compile_pattern(";12").is_err());
        // Bound first is fine.
        assert!(compile_pattern("(1234)(;1234)").is_ok());
    }

    // --- What subpatterns reject ---

    #[test]
    fn test_operators_rejected_inside_a_subpattern() {
        // `&` and `!` are whole-query operators. Without this check the textual
        // `&` split would tear `(a&b)` in half and report "Unclosed '('".
        for pat in ["(a&b)", "(!ab)", "(;ab&cd)", "(x;a!b)"] {
            assert!(compile_pattern(pat).is_err(), "{pat} should be rejected");
        }
        // At the top level, either side of a subpattern, they still work.
        assert!(compile_pattern("(;oif)(;bel)&f*").unwrap()("foible").is_some());
        assert!(compile_pattern("(;oif)(;bel)&!*s").unwrap()("foible").is_some());
    }

    #[test]
    fn test_subpattern_syntax_errors() {
        for pat in [
            "(;ab",         // unclosed
            "(;ab)\u{3c9}", // a letter this engine cannot represent
            "(;ab))",       // unmatched close
            "(ab`1;cd)",    // fuzz does not compose with an anagram pool
            "(1`1)x",       // nor with a digit variable
            "(;ab)`",       // malformed fuzz count
            "(;ab)`1`2",    // two fuzz suffixes
        ] {
            assert!(compile_pattern(pat).is_err(), "{pat} should be rejected");
        }
        // An accented letter is *not* rejected any more: the pattern is folded
        // before it is compiled, so `(;ab)é` arrives here as `(;ab)e`.
        assert!(compile_pattern("(;ab)\u{e9}").is_ok());
    }

    // --- Digit variables moved off the regex path ---

    #[test]
    fn test_digit_variables_run_on_the_structural_engine() {
        // Proof the dispatch actually moved, rather than the patterns merely
        // still working: starving the *structural* budget breaks them, which it
        // could not do while they compiled to regex backreferences.
        let starved = Limits {
            max_structural_steps: 1,
            ..Limits::default()
        };
        for (pat, word) in [
            ("1221", "abba"),
            ("1234321", "deified"),
            ("*1*2*1*2*", "banana"),
            ("1*1", "level"),
        ] {
            assert!(
                compile_pattern_with(pat, &starved).unwrap()(word).is_none(),
                "{pat} should be starved off the structural engine"
            );
            assert!(
                compile_pattern(pat).unwrap()(word).is_some(),
                "{pat} should match {word} under the shipped defaults"
            );
        }
    }

    #[test]
    fn test_digit_variables_agree_with_the_regex_semantics() {
        // The move is a dispatch change, not a semantic one: a digit still means
        // "the same letter as every other occurrence of this digit", and still
        // composes with every other token the same way.
        let m = compile_pattern("1221").unwrap();
        assert!(m("abba").is_some());
        assert!(m("abab").is_none());
        // Distinct digits are independent variables, not distinct letters.
        assert!(compile_pattern("11").unwrap()("aa").is_some());
        assert!(compile_pattern("12").unwrap()("aa").is_some());
        // Digits mix with wildcards, classes and literals.
        assert!(compile_pattern("1.1").unwrap()("aba").is_some());
        assert!(compile_pattern("1.1").unwrap()("abc").is_none());
        assert!(compile_pattern("c1t;1").unwrap()("cat").is_some());
        assert!(compile_pattern("[ab]11").unwrap()("axx").is_some());
    }

    #[test]
    fn test_fuzz_composes_with_a_subpattern() {
        // Merging the fuzzy tokenizer into the structural engine lifted this:
        // the budget is per node and spends only on `Lit` positions, so a block
        // sitting beside literals is simply rigid, like `.` or `[abc]`.
        let exact = compile_pattern("ele(;nahpt)").unwrap();
        let fuzzed = compile_pattern("ele(;nahpt)`1").unwrap();
        assert!(exact("elephant").is_some());
        assert!(fuzzed("elephant").is_some());
        // One literal wrong: only the budgeted form takes it.
        assert!(exact("alephant").is_none());
        assert!(fuzzed("alephant").is_some());
        // Two wrong is past the budget, and the block stays rigid either way.
        assert!(fuzzed("alxphant").is_none());
        assert!(compile_pattern("ele(;nahpt)`2").unwrap()("alxphant").is_some());
        assert!(fuzzed("elephxnt").is_none());
    }

    #[test]
    fn test_empty_subpattern_is_contentless() {
        // `()` and `(;)` have nothing to match, like a bare `;`: a gentle note
        // rather than an error, because the user often sees it mid-typing.
        for pat in ["()", "(;)"] {
            let c = compile_pattern_checked(pat).unwrap();
            assert!(c.note.is_some(), "{pat} should carry a note");
            assert!((c.matcher)("cat").is_none());
        }
    }

    #[test]
    fn test_structural_length_filter_is_exact_in_bytes() {
        // The regression this exists for: `(;glo)` matched `golßen` against a
        // dictionary carrying non-ASCII entries. The walk consumed "gol", ran
        // out of tokens, and a rigid node reported success without checking it
        // had reached the end of the word — because the length early-out had let
        // a wrong-length word through on an `is_ascii` escape hatch copied from
        // the regex path.
        //
        // The two paths count different things. `fixed_len` on the regex path is
        // a *character* count, so its exact test is only valid for an ASCII word
        // and anything else has to be left to the engine. Here `min`/`max` are
        // *byte* counts, so the test is exact and unconditional — and must stay
        // that way, because `Node::rigid` reads it as a guarantee.
        //
        // Every word below is the wrong length *only because* of its non-ASCII
        // tail. The equivalent ASCII cases were already covered and already
        // passed, which is exactly why this went unnoticed.
        for (pat, word) in [
            ("(;glo)", "gol\u{df}en"),
            ("(;glo)", "glo\u{e9}"),
            ("(;oif)(;bel)", "foible\u{e9}"),
            ("...(;bel)", "foible\u{e9}"),
            ("(;el)(;bo)w", "elbow\u{e9}"),
            ("(1234)(;1234)", "reappear\u{e9}"),
            ("ele(;nahpt)`1", "elephant\u{e9}"),
            ("(f..;oif)(;bel)", "foible\u{e9}"),
        ] {
            assert!(
                compile_pattern(pat).unwrap()(word).is_none(),
                "{pat} must not match {word:?}: it is the wrong byte length"
            );
        }
        // The same patterns still match the words they should.
        for (pat, word) in [
            ("(;glo)", "log"),
            ("(;oif)(;bel)", "foible"),
            ("(;el)(;bo)w", "elbow"),
            ("(1234)(;1234)", "reappear"),
            ("ele(;nahpt)`1", "elephant"),
        ] {
            assert!(
                compile_pattern(pat).unwrap()(word).is_some(),
                "{pat} should still match {word}"
            );
        }
    }

    #[test]
    fn test_a_structural_match_implies_an_ascii_word() {
        // The engine's whole byte-offset argument rests on this claim: nothing it
        // can match is non-ASCII, so byte offsets are char offsets and every
        // slice it takes lands on a boundary. Assert it directly rather than
        // trusting the reasoning — the `(;glo)` bug was a hole in exactly this,
        // and it survived a test that checked non-ASCII words individually
        // because that test happened to put the non-ASCII byte where a
        // subpattern would look at it.
        let pats = [
            "(;glo)",
            "(;oif)(;bel)",
            "*(;bel)",
            "(;bel)*",
            "(1234)(;1234)",
            "ele(;nahpt)`1",
            "..(;ing)",
            "*(;ing)*",
            "(;oif)(;bel);oifb",
        ];
        let words = [
            "log",
            "gol\u{df}en",
            "glo\u{e9}",
            "foible",
            "foible\u{e9}",
            "\u{e9}foible",
            "f\u{f6}ible",
            "elephant",
            "elephant\u{e9}",
            "singing",
            "s\u{ed}nging",
            "able",
            "abl\u{e9}",
            "reappear",
            "reappe\u{e1}r",
        ];
        for pat in pats {
            let m = compile_pattern(pat).unwrap();
            for w in words {
                if m(w).is_some() {
                    assert!(
                        w.is_ascii(),
                        "{pat} matched the non-ASCII word {w:?}, which breaks the \
                         byte-offset argument the engine is built on"
                    );
                }
            }
        }
    }

    #[test]
    fn test_subpattern_rejects_non_ascii_words_without_panicking() {
        // The engine slices by byte offset, which is only safe because nothing
        // it matches is non-ASCII: every token demands an ASCII byte, and a pure
        // block's pool rejects `has_other`. So a word carrying a multi-byte char
        // has to fall out, and — the part worth a test — fall out without
        // panicking on a slice that lands mid-character.
        let m = compile_pattern("(;oif)(;bel)").unwrap();
        assert!(m("f\u{f6}ible").is_none());
        // Six *bytes*, five chars, so it clears the length early-out and reaches
        // the walker, whose first cut at byte 3 lands inside the `\u{f6}`.
        assert!(m("\u{f6}ible").is_none());
        assert!(m("\u{e9}\u{e9}\u{e9}").is_none());
        // The two engines no longer agree here, and that is the documented split:
        // the regex path's `*` is `\p{Alphabetic}*` and accepts a non-Latin
        // letter, while this one is still byte-indexed and cannot. Words reaching
        // a real search are folded first, so in practice this only shows up for a
        // script the fold leaves non-ASCII.
        assert!(compile_pattern("*ble").unwrap()("na\u{ef}vet\u{e9}ble").is_some());
        assert!(compile_pattern("*(;bel)").unwrap()("na\u{ef}vet\u{e9}ble").is_none());
        assert!(compile_pattern("*(;bel)").unwrap()("able").is_some());
    }

    // --- Work limits on the structural path ---

    #[test]
    fn test_structural_nesting_depth_is_capped() {
        // Compile-time: bounds the recursion a short pattern can provoke.
        let deep = format!("{}{}{}", "(".repeat(12), ";a", ")".repeat(12));
        assert!(compile_pattern_with(&deep, &Limits::default()).is_err());
        let shallow = "(((;a)))";
        assert!(compile_pattern_with(shallow, &Limits::default()).is_ok());
    }

    #[test]
    fn test_structural_steps_cap_degrades_to_no_match() {
        // Match-time: a starved budget loses matches rather than raising, which
        // is what keeps the hot path `Result`-free. It is the only match-time
        // work limit left — `backtrack_limit` and `max_fuzzy_steps` were retired
        // with the engines they bounded.
        let starved = Limits {
            max_structural_steps: 1,
            ..Limits::default()
        };
        let m = compile_pattern_with("*(;ing)*", &starved).unwrap();
        assert!(m("singing").is_none());
        // And the same pattern under the shipped defaults does find it.
        let m = compile_pattern("*(;ing)*").unwrap();
        assert!(m("singing").is_some());
    }

    #[test]
    fn test_structural_patterns_unchanged_under_shipped_defaults() {
        // The other half of the limits test: nothing in the realistic corpus is
        // anywhere near a ceiling, so a limiter cannot have narrowed the
        // language. `limitcal` puts the worst of these at 51 steps against a
        // default of 5_000.
        for (pat, word) in [
            ("(;oif)(;bel)", "foible"),
            ("(...;oif)(;bel)", "foible"),
            ("(;oif)(;bel);oifb", "foible"),
            ("(1234)(;1234)", "reappear"),
            ("*(;bel)", "able"),
            ("*(;ing)*", "singing"),
            ("..(;ing)", "doing"),
        ] {
            assert!(
                compile_pattern(pat).unwrap()(word).is_some(),
                "{pat} should still match {word}"
            );
        }
    }

    #[test]
    fn test_anagram_combo_cap_still_applies_inside_a_subpattern() {
        // The cap lives in `parse_pool`, which both engines share, so a block
        // cannot be used to smuggle a cartesian product past it.
        let pat = format!("(;{})", "[abcde]".repeat(8));
        match compile_pattern_with(&pat, &tight()) {
            Err(e) => assert!(e.0.contains("too complex"), "unexpected: {}", e.0),
            Ok(_) => panic!("expected the combo cap to reject {pat}"),
        }
    }

    #[test]
    fn test_existing_syntax_never_reaches_the_structural_engine() {
        // `(...)` after a `;` keeps its long-standing "contains this substring"
        // meaning; only a group in the *template* half is a block.
        let m = compile_pattern(";(che)rostra").unwrap();
        assert!(m("orchestra").is_some());
        assert!(m("carthorse").is_none());
    }
}
