use fancy_regex::RegexBuilder;
use std::borrow::Cow;
use std::collections::HashMap;

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
        Ok((compile_template(pattern, limits)?, contentless))
    }
}

/// Whether this part needs the structural engine rather than one of the three
/// older ones.
///
/// Two triggers, both of them syntax that was a hard error before subpatterns:
/// a `(...)` group in the *template* half (in the pool half, `(...)` is the
/// long-standing "contains this substring" marker and keeps that meaning), and
/// a digit in the pool half, which is what lets a variable bound by the
/// template be spent as a pool letter.
fn needs_structural(pattern: &str) -> bool {
    let (template, pool) = match find_top_level(pattern, ';') {
        Some(idx) => (&pattern[..idx], &pattern[idx + 1..]),
        None => (pattern, ""),
    };
    template.contains('(') || pool.chars().any(|c| c.is_ascii_digit())
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

// We use one of two different matchers depending on whether or not there’s fuzz.
// If no fuzz, we use fancy_regex, which efficiently handles e.g. multiple *’s.
// But it can’t directly handle fuzz letters, so we’d have to blow it into N choose F
// many alternatives for N literals with a fuzz of F.
//
// So when there’s fuzz, we use our own naïve matching implementation, which is
// inefficient on * wildcards but is efficient on fuzzy matches (just keeping a running
// tally and doing backtracking).
fn compile_template(template: &str, limits: &Limits) -> Result<Matcher, PatternError> {
    let (base, fuzz) = split_fuzz(template)?;
    // `N > 0` enables fuzzy matching; `` `0 `` is exact, so it falls through to the
    // regular regex path — which is also the path every fuzz-free template takes,
    // unchanged, so existing patterns are never rerouted.
    if let Some(k) = fuzz {
        if k > 0 {
            return compile_fuzzy_template(base, k, limits);
        }
    }
    let (regex_str, fixed_len) = template_to_regex(base)?;
    // `template_to_regex` maps every `*` to `[a-z]*`, so a star-heavy template
    // like `**********cat` costs the engine work superlinear in the star count,
    // *per word*, across the whole list. Cap the backtracking rather than let a
    // short pattern wedge the app; the `unwrap_or(false)` below degrades a word
    // that exceeds the cap to "no match", which keeps the hot path free of
    // `Result` handling (see the module's performance notes).
    let re = RegexBuilder::new(&format!("(?i)^{}$", regex_str))
        .backtrack_limit(limits.backtrack_limit)
        .build()
        .map_err(|e| PatternError(format!("Invalid template '{}': {}", base, e)))?;
    Ok(Box::new(move |word: &str| {
        // Reject on length before invoking the regex engine. A star-free template
        // matches exactly one length, and on a large list the overwhelming
        // majority of words are the wrong length — so this replaces a
        // backtracking match with an integer compare for most of the scan. It is
        // a pure filter: any word this rejects could not have matched anyway.
        if let Some(n) = fixed_len {
            // Byte length is a safe lower bound on character count, since a UTF-8
            // char is at least one byte — so a word shorter than `n` bytes can
            // never match. The exact test is only valid when the word is
            // all-ASCII, where bytes and chars coincide; anything else falls
            // through and lets the regex decide.
            if word.len() < n || (word.len() != n && word.is_ascii()) {
                return None;
            }
        }
        if re.is_match(word).unwrap_or(false) {
            Some(MatchInfo::default())
        } else {
            None
        }
    }))
}

/// A single template position, for the fuzzy matcher. Only `Lit` positions are
/// allowed to mismatch (and only up to the fuzz budget); everything else is rigid.
enum FuzzTok {
    /// A literal letter (lowercased). Fuzzable: may mismatch, costing one budget.
    Lit(u8),
    /// Punctuation (`-`, `'`, space). Rigid.
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
///   25_193 matches, the rest silently truncated by `backtrack_limit`. As
///   `*1*1` it costs 139 ms and returns all of them.
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

/// Tokenize a template for fuzzy matching. Rejects digit variables (whose
/// backreference semantics don't compose cleanly with a mismatch budget).
fn tokenize_fuzzy(template: &str) -> Result<Vec<FuzzTok>, PatternError> {
    let mut out = Vec::new();
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '.' => out.push(FuzzTok::Any),
            '*' => {
                // The run normalizes to its dots followed by one star; see
                // `collapse_gap_run`. Emitting the dots first is the whole
                // rewrite — they commute with the star.
                let dots = collapse_gap_run(&chars, &mut i);
                for _ in 0..dots {
                    out.push(FuzzTok::Any);
                }
                out.push(FuzzTok::Star);
            }
            '@' => out.push(FuzzTok::Vowel),
            '#' => out.push(FuzzTok::Consonant),
            '[' => {
                let rel = chars[i..]
                    .iter()
                    .position(|&x| x == ']')
                    .ok_or_else(|| PatternError("Unclosed '[' in template".to_string()))?;
                let j = i + rel;
                let mut set = Vec::new();
                for &ch in &chars[i + 1..j] {
                    if !ch.is_ascii() {
                        return Err(PatternError(
                            "Fuzzy matching ('`N') supports only ASCII letters".to_string(),
                        ));
                    }
                    set.push(ch.to_ascii_lowercase() as u8);
                }
                out.push(FuzzTok::Class(set));
                i = j;
            }
            c if c.is_ascii_digit() => {
                // Backreferences are intentionally left to the regex path (see
                // `compile_template`); they don't compose with a mismatch budget.
                return Err(PatternError(
                    "Fuzzy matching ('`N') is not supported with digit variables".to_string(),
                ));
            }
            c @ ('-' | '\'' | ' ') => out.push(FuzzTok::Punct(c as u8)),
            c if c.is_ascii_alphabetic() => out.push(FuzzTok::Lit(c.to_ascii_lowercase() as u8)),
            c if c.is_alphabetic() => {
                return Err(PatternError(
                    "Fuzzy matching ('`N') supports only ASCII letters".to_string(),
                ))
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
    Ok(out)
}

/// Match `w` against `toks` starting at token `ti` / byte `ci`, where up to `budget`
/// literal positions are permitted to mismatch. Allocation-free; the only branching
/// is `*` backtracking, so a star-free template is a straight O(len) walk.
///
/// Recurses rather than looping (the `*` arm needs to backtrack). We don't rely on
/// tail-call optimization: every call increases `ti + ci` by at least one, so the
/// recursion depth is bounded by `toks.len() + w.len()` — a few dozen frames for any
/// real word, nowhere near a stack concern.
fn fuzzy_match(
    toks: &[FuzzTok],
    w: &[u8],
    ti: usize,
    ci: usize,
    budget: usize,
    steps: &mut u32,
) -> bool {
    // `steps` bounds the *number of nodes explored*, which is a different
    // quantity from `budget` (the fuzz allowance) and from the recursion depth
    // reasoned about above. Depth is naturally bounded; branching is not — the
    // `Star` arm below recurses twice, so a star-heavy template is exponential
    // with nothing underneath it to stop, unlike the regex path which at least
    // has fancy-regex's own limiter. Exhausting the budget reports "no match",
    // matching how the regex path degrades when it hits `backtrack_limit`.
    if *steps == 0 {
        return false;
    }
    *steps -= 1;
    if ti == toks.len() {
        return ci == w.len();
    }
    match &toks[ti] {
        FuzzTok::Star => {
            // Match zero letters here, or consume one letter and stay on the star.
            if fuzzy_match(toks, w, ti + 1, ci, budget, steps) {
                return true;
            }
            ci < w.len()
                && w[ci].to_ascii_lowercase().is_ascii_lowercase()
                && fuzzy_match(toks, w, ti, ci + 1, budget, steps)
        }
        tok => {
            if ci >= w.len() {
                return false;
            }
            let c = w[ci].to_ascii_lowercase();
            let satisfied = match tok {
                FuzzTok::Lit(l) => {
                    if c != *l {
                        // A literal mismatch is allowed only while budget remains, and
                        // only onto a letter (mirroring the wildcard a freed slot becomes).
                        return budget > 0
                            && c.is_ascii_lowercase()
                            && fuzzy_match(toks, w, ti + 1, ci + 1, budget - 1, steps);
                    }
                    true
                }
                FuzzTok::Punct(p) => c == *p,
                FuzzTok::Any => c.is_ascii_lowercase(),
                FuzzTok::Vowel => is_vowel(c),
                FuzzTok::Consonant => c.is_ascii_lowercase() && !is_vowel(c),
                FuzzTok::Class(set) => set.contains(&c),
                FuzzTok::Star => unreachable!(),
            };
            satisfied && fuzzy_match(toks, w, ti + 1, ci + 1, budget, steps)
        }
    }
}

fn compile_fuzzy_template(
    template: &str,
    fuzz: usize,
    limits: &Limits,
) -> Result<Matcher, PatternError> {
    let toks = tokenize_fuzzy(template)?;
    let max_steps = limits.max_fuzzy_steps;
    // The same length filter the regex path uses, and exact here rather than
    // approximate. `fuzzy_match` is byte-indexed and every token except `Star`
    // consumes exactly one byte, so a star-free template matches only words of
    // exactly `toks.len()` bytes. No `is_ascii` caveat is needed: `tokenize_fuzzy`
    // rejects non-ASCII templates, and every arm of the matcher — the fuzzed
    // literal-mismatch arm included — requires an ASCII byte, so a word carrying
    // a multi-byte char cannot match at any length. Fuzz does not widen this:
    // the budget lets a position mismatch, never disappear.
    let fixed_len = (!toks.iter().any(|t| matches!(t, FuzzTok::Star))).then_some(toks.len());
    Ok(Box::new(move |word: &str| {
        // Reject on length before recursing. This is the whole cost of the scan
        // for a star-free fuzzy pattern on a large list: almost every word is the
        // wrong length, and an integer compare replaces a walk of the template.
        if let Some(n) = fixed_len {
            if word.len() != n {
                return None;
            }
        }
        // Fresh budget per word: the limit bounds the cost of one candidate, not
        // of the whole scan, so a pathological word can't starve later ones.
        let mut steps = max_steps;
        if fuzzy_match(&toks, word.as_bytes(), 0, 0, fuzz, &mut steps) {
            Some(MatchInfo::default())
        } else {
            None
        }
    }))
}

fn escape_in_char_class(c: char) -> String {
    if matches!(c, ']' | '\\' | '^' | '-') {
        format!("\\{}", c)
    } else {
        c.to_string()
    }
}

/// Translate a template to a regex, and report the exact length it can match.
///
/// Every construct except `*` consumes exactly one character, so a star-free
/// template matches words of exactly one length — which lets the caller reject
/// most of the word list with an integer compare instead of the regex engine.
/// `None` means the length is not fixed (the template contains a `*`, or a
/// literal whose lowercasing changes its character count, which would make the
/// count unreliable).
fn template_to_regex(template: &str) -> Result<(String, Option<usize>), PatternError> {
    let mut out = String::new();
    let mut fixed_len: Option<usize> = Some(0);
    let mut seen_vars: HashMap<char, bool> = HashMap::new();
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
                out.push_str("[a-z]");
                consume_one!();
            }
            '*' => {
                // The run normalizes to its dots followed by one star; see
                // `collapse_gap_run`. No `consume_one!` for those dots: the star
                // has already made the length unknowable.
                let dots = collapse_gap_run(&chars, &mut i);
                for _ in 0..dots {
                    out.push_str("[a-z]");
                }
                out.push_str("[a-z]*");
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
                consume_one!();
                let name = format!("v{}", c);
                if let std::collections::hash_map::Entry::Vacant(e) = seen_vars.entry(c) {
                    e.insert(true);
                    out.push_str(&format!("(?P<{}>[a-z])", name));
                } else {
                    out.push_str(&format!("(?P={})", name));
                }
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
                out.push_str(&fancy_regex::escape(&lowered));
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

fn count_chars(chars: &[char]) -> [usize; 26] {
    let mut counts = [0usize; 26];
    for &c in chars {
        if c.is_ascii_lowercase() {
            counts[(c as u8 - b'a') as usize] += 1;
        }
    }
    counts
}

/// Count the ASCII letters of `s` into a 26-bucket histogram, returning the
/// histogram, the total letter count, and `has_other`: whether `s` contains any
/// character that is not an ASCII letter (a digit, symbol, or non-ASCII letter —
/// its UTF-8 bytes are all non-`is_ascii_alphabetic`). Callers matching pure
/// anagrams use `has_other` to reject candidates that carry non-letter cruft, so
/// the anagram alphabet matches the template path's ASCII `[a-z]`. Punctuation
/// (`space -'`) has already been stripped from candidates upstream, so it never
/// registers as "other" here.
fn count_str(s: &str) -> ([usize; 26], usize, bool) {
    let mut counts = [0usize; 26];
    let mut len = 0;
    let mut has_other = false;
    for b in s.bytes() {
        if b.is_ascii_alphabetic() {
            counts[(b.to_ascii_lowercase() - b'a') as usize] += 1;
            len += 1;
        } else {
            has_other = true;
        }
    }
    (counts, len, has_other)
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
    combo_pools: Vec<([usize; 26], usize)>,
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
    template_counter: [usize; 26],
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
        pool_counter: &[usize; 26],
        pool_base: usize,
        candidate_counter: &[usize; 26],
        candidate_len: usize,
    ) -> Option<MatchInfo> {
        let pool_size = pool_base + self.num_wildcards;

        if self.is_pure && !self.has_star && candidate_len != pool_size {
            return None;
        }

        // The "effective pool" is the set of letters the word is measured against
        // when reporting unused (pool − word) and extra (word − pool) letters.
        let effective_pool: [usize; 26] = if self.is_pure {
            for i in 0..26 {
                if pool_counter[i] > 0 && candidate_counter[i] < pool_counter[i] {
                    return None;
                }
            }

            let extras: usize = (0..26)
                .map(|i| candidate_counter[i].saturating_sub(pool_counter[i]))
                .sum();

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
            let mut anagram_counter = self.template_counter;
            for i in 0..26 {
                if pool_counter[i] > anagram_counter[i] {
                    anagram_counter[i] = pool_counter[i];
                }
            }

            // Count letters in the candidate that aren't in the anagram pool, and
            // pool letters not used by the candidate.
            let mut extra_count: usize = 0;
            let mut unused_count: usize = 0;
            for i in 0..26 {
                extra_count += candidate_counter[i].saturating_sub(anagram_counter[i]);
                unused_count += anagram_counter[i].saturating_sub(candidate_counter[i]);
            }

            // The candidate has to use all the pool letters (a longer word)
            // or it has to use *only* pool letters (a shorter word).
            // Wildcards license a deviation from either criterion.
            // Wildcards consume pattern symbols without actually adding license,
            // until all wildcards are consumed, at which point they license non-pool letters.
            if extra_count > self.num_wildcards
                && unused_count > self.num_wildcards.saturating_sub(candidate_len)
            {
                return None;
            }

            anagram_counter
        };

        // Match confirmed. Now (and only now) do the extra work of spelling out the
        // unused (pool − word) and extra (word − pool) letters for display.
        Some(MatchInfo {
            unused: diff_letters(&effective_pool, candidate_counter),
            extra: diff_letters(candidate_counter, &effective_pool),
        })
    }

    /// Test a candidate against every `[...]` combination, under `env`.
    fn check(&self, candidate: &str, env: &Env) -> Option<MatchInfo> {
        let (candidate_counter, candidate_len, has_other) = count_str(candidate);

        // A pure anagram rearranges letters, so a candidate carrying any non-letter
        // character (digit, symbol, or non-ASCII letter) is not a clean anagram —
        // reject it, mirroring the template path's ASCII `[a-z]`. The hybrid path
        // (is_pure == false) is already governed by its anchored template regex.
        if self.is_pure && has_other {
            return None;
        }

        for (base_counter, base_size) in &self.combo_pools {
            // The common case — no digit variables — hands the pre-computed
            // counter straight through without copying it.
            let found = if self.vars.is_empty() {
                self.check_combo(base_counter, *base_size, &candidate_counter, candidate_len)
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
                    adjusted[(b - b'a') as usize] += 1;
                }
                self.check_combo(
                    &adjusted,
                    base_size + self.vars.len(),
                    &candidate_counter,
                    candidate_len,
                )
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
                    .all(|sp| candidate.contains(sp.as_str()))
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

    let fixed_counter = count_chars(&fixed_letters);
    let fixed_size = fixed_letters.len();
    let combo_pools: Vec<([usize; 26], usize)> = choice_combos
        .iter()
        .map(|combo| {
            let mut counter = fixed_counter;
            for &c in combo {
                counter[(c.to_ascii_lowercase() as u8 - b'a') as usize] += 1;
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
        template_counter: count_chars(&template_letters),
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
    let template_matcher: Option<Matcher> =
        template.map(|t| compile_template(t, limits)).transpose()?;

    Ok(Box::new(move |candidate: &str| {
        if let Some(ref tm) = template_matcher {
            tm(candidate)?;
        }
        pool.check(candidate, &NO_VARS)
    }))
}

/// Build an uppercase, alphabetically-sorted string of the letters in `more` that
/// exceed `less` (per-letter, by count). Used to spell out unused and extra letters.
fn diff_letters(more: &[usize; 26], less: &[usize; 26]) -> String {
    let mut out = String::new();
    for i in 0..26 {
        for _ in 0..more[i].saturating_sub(less[i]) {
            out.push((b'A' + i as u8) as char);
        }
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
    /// True when there is no template half at all (`(;oif)`), so the token list
    /// imposes nothing and the pool alone fixes the length.
    pure: bool,
    /// Byte length this node can match. `max` is `None` when a `*` makes it
    /// open-ended.
    min: usize,
    max: Option<usize>,
    /// `suffix_min[i]`/`suffix_max[i]`: what the tokens from `i` onward need.
    /// Indexed up to and including `toks.len()`, so the walker can prune before
    /// looking at a token as well as after the last one.
    suffix_min: Vec<usize>,
    suffix_max: Vec<Option<usize>>,
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
    let pure = pool_src.is_some() && tpl.is_empty();
    let toks = if pure {
        Vec::new()
    } else {
        tokenize_structural(tpl, limits)?
    };
    let pool = pool_src
        .map(|p| parse_pool(if pure { None } else { Some(tpl) }, p, limits))
        .transpose()?;

    // Suffix bounds, right to left. These are the whole reason a fixed-length
    // composition costs a walk rather than a search.
    let n = toks.len();
    let mut suffix_min = vec![0usize; n + 1];
    let mut suffix_max = vec![Some(0usize); n + 1];
    for i in (0..n).rev() {
        let (lo, hi) = tok_bounds(&toks[i]);
        suffix_min[i] = suffix_min[i + 1] + lo;
        suffix_max[i] = match (hi, suffix_max[i + 1]) {
            (Some(a), Some(b)) => Some(a + b),
            _ => None,
        };
    }

    let (min, max) = if pure {
        // No template, so the pool alone says how many letters this spends.
        let pool = pool.as_ref().expect("pure implies a pool");
        (pool.min_spend(), pool.spend())
    } else {
        (suffix_min[0], suffix_max[0])
    };

    Ok(Node {
        toks,
        pool,
        pure,
        min,
        max,
        suffix_min,
        suffix_max,
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

/// Match `node`'s tokens from token `ti` / byte `ci`, filling exactly up to
/// `end`. Returns the environment as extended by this stretch, plus the detail
/// its subpatterns reported.
fn walk(
    node: &Node,
    w: &[u8],
    ti: usize,
    ci: usize,
    end: usize,
    env: Env,
    steps: &mut u32,
) -> Option<(Env, MatchInfo)> {
    // Bounds the number of nodes explored, per word, re-armed for each. Both
    // the `Star` and the `Sub` arm below branch, so without this a pattern like
    // `*(;ab)*(;cd)*` is exponential with nothing underneath it to stop.
    // Exhausting the budget reports "no match", as every other match-time limit
    // in the crate does.
    if *steps == 0 {
        return None;
    }
    *steps -= 1;

    // What is left has to be something the remaining tokens can cover. Applied
    // before the token is even looked at, this is what collapses an all-fixed
    // composition to a single forced cut.
    let left = end - ci;
    if left < node.suffix_min[ti] {
        return None;
    }
    if node.suffix_max[ti].is_some_and(|mx| left > mx) {
        return None;
    }

    if ti == node.toks.len() {
        // The pruning above already proved `ci == end`.
        return Some((env, MatchInfo::default()));
    }

    match &node.toks[ti] {
        Tok::Star => {
            // Match zero letters here, or consume one letter and stay on the star.
            if let Some(r) = walk(node, w, ti + 1, ci, end, env, steps) {
                return Some(r);
            }
            if ci < end && w[ci].to_ascii_lowercase().is_ascii_lowercase() {
                walk(node, w, ti, ci + 1, end, env, steps)
            } else {
                None
            }
        }
        Tok::Sub(sub) => {
            // Only cuts that leave the rest of the tokens satisfiable are worth
            // trying, so this window is usually a single offset.
            let rest_min = node.suffix_min[ti + 1];
            let lo = ci + sub.min;
            // `end - rest_min` cannot underflow: the suffix_min prune above
            // already established `end - ci >= sub.min + rest_min`.
            let hi = sub.max.map_or(end, |m| ci + m).min(end - rest_min);
            let mut cut = lo;
            while cut <= hi {
                if let Some((env2, mut info)) = match_node(sub, w, ci, cut, env, steps) {
                    if let Some((env3, rest)) = walk(node, w, ti + 1, cut, end, env2, steps) {
                        info.unused.push_str(&rest.unused);
                        info.extra.push_str(&rest.extra);
                        return Some((env3, info));
                    }
                }
                cut += 1;
            }
            None
        }
        tok => {
            if ci >= end {
                return None;
            }
            let c = w[ci].to_ascii_lowercase();
            let mut env2 = env;
            let satisfied = match tok {
                Tok::Lit(l) => c == *l,
                Tok::Punct(p) => c == *p,
                Tok::Any => c.is_ascii_lowercase(),
                Tok::Vowel => is_vowel(c),
                Tok::Consonant => c.is_ascii_lowercase() && !is_vowel(c),
                Tok::Class(set) => set.contains(&c),
                Tok::Var(d) => {
                    let slot = &mut env2[*d as usize];
                    if !c.is_ascii_lowercase() {
                        false
                    } else if *slot == 0 {
                        *slot = c;
                        true
                    } else {
                        *slot == c
                    }
                }
                Tok::Star | Tok::Sub(_) => unreachable!("handled above"),
            };
            if satisfied {
                walk(node, w, ti + 1, ci + 1, end, env2, steps)
            } else {
                None
            }
        }
    }
}

/// Match one whole node against the slice `start..end`.
fn match_node(
    node: &Node,
    w: &[u8],
    start: usize,
    end: usize,
    env: Env,
    steps: &mut u32,
) -> Option<(Env, MatchInfo)> {
    if *steps == 0 {
        return None;
    }
    *steps -= 1;

    let mut info = MatchInfo::default();
    let mut pool_checked = false;

    // A pool with no variables depends only on the slice's letter counts, so it
    // is a cheap count-based filter and belongs *before* the walk. One that
    // spends variables has to wait for the template to bind them.
    if let Some(pool) = &node.pool {
        if pool.vars.is_empty() {
            info = pool.check(slice_str(w, start, end)?, &NO_VARS)?;
            pool_checked = true;
        }
    }

    let env = if node.pure {
        env
    } else {
        let (env, sub) = walk(node, w, 0, start, end, env, steps)?;
        info.unused.push_str(&sub.unused);
        info.extra.push_str(&sub.extra);
        env
    };

    if !pool_checked {
        if let Some(pool) = &node.pool {
            let detail = pool.check(slice_str(w, start, end)?, &env)?;
            info.unused.push_str(&detail.unused);
            info.extra.push_str(&detail.extra);
        }
    }

    Some((env, info))
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
        // The same length early-out the other two engines use, and for the same
        // reason: on a large list almost every word is the wrong length, so this
        // replaces the whole walk with an integer compare. The `is_ascii` caveat
        // is the regex path's — a word shorter than `n` bytes can never match,
        // and bytes only equal chars when the word is ASCII.
        if let Some(n) = fixed_len {
            if word.len() < n || (word.len() != n && word.is_ascii()) {
                return None;
            }
        } else if word.len() < min {
            return None;
        }
        // Fresh budget per word, so a pathological word can't starve later ones.
        let mut steps = max_steps;
        match_node(&node, word.as_bytes(), 0, word.len(), NO_VARS, &mut steps).map(|(_, info)| info)
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
        // `;.` must behave like the template `.`: a single ASCII letter, and
        // nothing carrying non-letter cruft or non-ASCII letters.
        let m = compile_pattern(";.").unwrap();
        assert!(m("a").is_some());
        assert!(m(".c").is_none()); // stray '.'
        assert!(m("3a").is_none()); // digit
        assert!(m("a!").is_none()); // symbol
        assert!(m("æ").is_none()); // non-ASCII letter
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
            backtrack_limit: 10_000,
            max_fuzzy_steps: 10_000,
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
        // The fuzzy path is a hand-rolled backtracker whose `Star` arm branches
        // exponentially and had no step budget at all before this.
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

    // --- The fuzzy path's fixed-length early-out ---------------------------
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

    // --- The recalibrated match-time defaults ------------------------------

    #[test]
    fn test_default_match_time_limits_clear_the_calibrated_floors() {
        // `examples/limitcal.rs` measures the smallest limit that still returns
        // every match, over a corpus of realistic and adversarial patterns. The
        // adversarial worst cases are 1_315 backtrack steps (`*1*2*1*2*`) and
        // 3_698 fuzzy steps (`` *a*b*c*d*`2 ``). These defaults were lowered from
        // 1_000_000 apiece; this is the tripwire against lowering them into the
        // range where they would start silently dropping real matches.
        let d = Limits::default();
        assert!(
            d.backtrack_limit >= 1_315 * 4,
            "backtrack_limit {} leaves too little headroom over the measured floor",
            d.backtrack_limit
        );
        assert!(
            d.max_fuzzy_steps >= 3_698 * 4,
            "max_fuzzy_steps {} leaves too little headroom over the measured floor",
            d.max_fuzzy_steps
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
        // `**********1**********1` exceeded `backtrack_limit` on many words and
        // degraded them to "no match" — silently returning a fraction of the
        // real matches. Collapsed to `*1*1` it is bounded, so the two spellings
        // must now agree everywhere, including on long words.
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
        // and a literal-tailed form, on both the regex and the fuzzy path.
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
            "(;ab",        // unclosed
            "(;ab)`1",     // fuzz does not compose with a block
            "(;ab)\u{e9}", // non-ASCII on this path
            "(;ab))",      // unmatched close
        ] {
            assert!(compile_pattern(pat).is_err(), "{pat} should be rejected");
        }
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
        // This is not a new restriction: the regex path's `*` is `[a-z]*` and
        // rejects the same word, so the two engines agree.
        assert!(compile_pattern("*(;bel)").unwrap()("na\u{ef}vet\u{e9}ble").is_none());
        assert!(compile_pattern("*ble").unwrap()("na\u{ef}vet\u{e9}ble").is_none());
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
        // Match-time: a starved budget loses matches rather than raising, the
        // same way `backtrack_limit` and `max_fuzzy_steps` do.
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
