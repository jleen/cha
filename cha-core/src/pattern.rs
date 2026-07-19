use fancy_regex::RegexBuilder;
use std::borrow::Cow;
use std::collections::HashMap;

const PUNCTUATION: &[char] = &[' ', '-', '\''];

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

/// Ceilings on how much work a *single* pattern is allowed to cost.
///
/// Every backtracking or combinatorial path in this module needs a bound, because
/// each one is driven directly by untrusted input and each can be made
/// superlinear by a short, innocent-looking pattern. Three exist:
///
/// - `max_anagram_combos` bounds `cartesian_product` in `compile_anagram`, which
///   materializes the full product of every `[...]` group **before** any word is
///   scanned. `;[abcde]` repeated 8 times is 5^8 = 390_625 combos ≈ 84 MB of
///   `combo_pools`; ten groups is ~2.1 GB. This is the only limit that binds at
///   compile time, so it's the only one that can exhaust memory rather than time.
/// - `backtrack_limit` bounds the `fancy-regex` engine on the non-fuzzy template
///   path, where `template_to_regex` turns every `*` into `[a-z]*`.
/// - `max_fuzzy_steps` bounds `fuzzy_match`, the hand-rolled backtracker used on
///   the fuzzy path, whose `Star` arm branches like a regex star with no engine
///   underneath it to impose a limit of its own.
///
/// `Default` is generous — enough that no plausible hand-typed pattern reaches it
/// — and exists to turn a hang or an OOM into an error message. A server exposing
/// this to a network wants much tighter values; see `cha-web`.
#[derive(Debug, Clone)]
pub struct CompileLimits {
    /// Maximum length, in bytes, of the whole pattern string.
    pub max_pattern_len: usize,
    /// Maximum number of `[...]` combinations an anagram may expand to.
    pub max_anagram_combos: usize,
    /// Maximum regex backtracking steps per word (`fancy-regex`'s own unit).
    pub backtrack_limit: usize,
    /// Maximum `fuzzy_match` recursion steps per word.
    pub max_fuzzy_steps: u32,
}

impl Default for CompileLimits {
    fn default() -> Self {
        Self {
            // Interactive use: a pattern this long is a paste accident, not a query.
            max_pattern_len: 1024,
            // ~21 MB of `combo_pools`. Real patterns use a handful of groups.
            max_anagram_combos: 100_000,
            // fancy-regex's own default; preserves existing behavior exactly.
            backtrack_limit: 1_000_000,
            // Comparable ceiling for the hand-rolled matcher, which previously
            // had none at all.
            max_fuzzy_steps: 1_000_000,
        }
    }
}

/// Compile a pattern, distinguishing a contentless pattern (well-formed but with
/// nothing to match — carried as a note) from a genuine syntax error (`Err`).
///
/// Uses `CompileLimits::default()`. Callers that accept patterns from a network
/// should use [`compile_pattern_checked_with`] and supply tighter ceilings.
pub fn compile_pattern_checked(pattern_str: &str) -> Result<Compiled, PatternError> {
    compile_pattern_checked_with(pattern_str, &CompileLimits::default())
}

/// [`compile_pattern_checked`] with explicit work ceilings. See [`CompileLimits`].
pub fn compile_pattern_checked_with(
    pattern_str: &str,
    limits: &CompileLimits,
) -> Result<Compiled, PatternError> {
    if pattern_str.len() > limits.max_pattern_len {
        return Err(PatternError(format!(
            "Pattern is too long ({} characters; the limit is {})",
            pattern_str.len(),
            limits.max_pattern_len
        )));
    }
    let parts: Vec<&str> = pattern_str.split('&').collect();
    let mut matchers: Vec<(bool, Matcher)> = Vec::new();
    let mut contentless = false;

    for part in parts {
        let part = part.trim();
        let (negate, actual) = if let Some(rest) = part.strip_prefix('!') {
            (true, rest.trim())
        } else {
            (false, part)
        };
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

    let has_punct = pattern_str.chars().any(|c| PUNCTUATION.contains(&c));

    let matcher: Matcher = Box::new(move |word: &str| {
        let test_word: Cow<str> = if has_punct {
            Cow::Borrowed(word)
        } else if word.chars().any(|c| PUNCTUATION.contains(&c)) {
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

pub fn compile_pattern(pattern_str: &str) -> Result<Matcher, PatternError> {
    compile_pattern_checked(pattern_str).map(|c| c.matcher)
}

/// [`compile_pattern`] with explicit work ceilings. See [`CompileLimits`].
pub fn compile_pattern_with(
    pattern_str: &str,
    limits: &CompileLimits,
) -> Result<Matcher, PatternError> {
    compile_pattern_checked_with(pattern_str, limits).map(|c| c.matcher)
}

/// Compile one `&`-separated part, returning its matcher and whether it is
/// *contentless* — well-formed but with no letters or wildcard structure to match
/// (an empty template, or a bare `;` empty-pool anagram). Wildcards (`. * @ #`),
/// classes `[…]`, and sub-patterns `(…)` count as content, so only genuinely empty
/// parts are flagged.
fn compile_one_pattern(
    pattern: &str,
    limits: &CompileLimits,
) -> Result<(Matcher, bool), PatternError> {
    if let Some(idx) = pattern.find(';') {
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
    match template.rfind('`') {
        None => Ok((template, None)),
        Some(idx) => {
            let base = &template[..idx];
            if base.contains('`') {
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
fn compile_template(template: &str, limits: &CompileLimits) -> Result<Matcher, PatternError> {
    let (base, fuzz) = split_fuzz(template)?;
    // `N > 0` enables fuzzy matching; `` `0 `` is exact, so it falls through to the
    // regular regex path — which is also the path every fuzz-free template takes,
    // unchanged, so existing patterns are never rerouted.
    if let Some(k) = fuzz {
        if k > 0 {
            return compile_fuzzy_template(base, k, limits);
        }
    }
    let regex_str = template_to_regex(base)?;
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

/// Tokenize a template for fuzzy matching. Rejects digit variables (whose
/// backreference semantics don't compose cleanly with a mismatch budget).
fn tokenize_fuzzy(template: &str) -> Result<Vec<FuzzTok>, PatternError> {
    let mut out = Vec::new();
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '.' => out.push(FuzzTok::Any),
            '*' => out.push(FuzzTok::Star),
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
    limits: &CompileLimits,
) -> Result<Matcher, PatternError> {
    let toks = tokenize_fuzzy(template)?;
    let max_steps = limits.max_fuzzy_steps;
    Ok(Box::new(move |word: &str| {
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

fn template_to_regex(template: &str) -> Result<String, PatternError> {
    let mut out = String::new();
    let mut seen_vars: HashMap<char, bool> = HashMap::new();
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            '.' => out.push_str("[a-z]"),
            '*' => out.push_str("[a-z]*"),
            '@' => out.push_str("[aeiou]"),
            '#' => out.push_str("[bcdfghjklmnpqrstvwxyz]"),
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
                i = j;
            }
            c if c.is_ascii_digit() => {
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
            }
            c if c.is_alphabetic() => {
                out.push_str(&fancy_regex::escape(&c.to_lowercase().to_string()));
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

fn compile_anagram(
    template: Option<&str>,
    anagram_expr: &str,
    limits: &CompileLimits,
) -> Result<Matcher, PatternError> {
    if template.is_some_and(|t| t.contains('`')) {
        return Err(PatternError(
            "Fuzzy matching ('`N') is not supported in an anagram template".to_string(),
        ));
    }
    let mut fixed_letters: Vec<char> = Vec::new();
    let mut choices: Vec<Vec<char>> = Vec::new();
    let mut sub_patterns: Vec<String> = Vec::new();
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
                sub_patterns.push(sp.clone());
                fixed_letters.extend(sp.chars());
                i = j;
            }
            '.' => num_wildcards += 1,
            '*' => has_star = true,
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

    let template_matcher: Option<Matcher> =
        template.map(|t| compile_template(t, limits)).transpose()?;
    let is_pure = template.is_none();
    let template_letters: Vec<char> = template
        .map(|t| {
            t.chars()
                .filter(|c| c.is_alphabetic())
                .map(|c| c.to_lowercase().next().unwrap())
                .collect()
        })
        .unwrap_or_default();

    let fixed_counter = count_chars(&fixed_letters);
    let fixed_size = fixed_letters.len();
    let template_counter = count_chars(&template_letters);
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

    Ok(Box::new(move |candidate: &str| {
        if let Some(ref tm) = template_matcher {
            tm(candidate)?;
        }

        let (candidate_counter, candidate_len, has_other) = count_str(candidate);

        // A pure anagram rearranges letters, so a candidate carrying any non-letter
        // character (digit, symbol, or non-ASCII letter) is not a clean anagram —
        // reject it, mirroring the template path's ASCII `[a-z]`. The hybrid path
        // (is_pure == false) is already governed by its anchored template regex.
        if is_pure && has_other {
            return None;
        }

        'combo: for (pool_counter, pool_base) in &combo_pools {
            let pool_size = pool_base + num_wildcards;

            if is_pure && !has_star && candidate_len != pool_size {
                continue;
            }

            // The "effective pool" is the set of letters the word is measured against
            // when reporting unused (pool − word) and extra (word − pool) letters.
            let effective_pool: [usize; 26] = if is_pure {
                for i in 0..26 {
                    if pool_counter[i] > 0 && candidate_counter[i] < pool_counter[i] {
                        continue 'combo;
                    }
                }

                let extras: usize = (0..26)
                    .map(|i| candidate_counter[i].saturating_sub(pool_counter[i]))
                    .sum();

                if !has_star && extras != num_wildcards {
                    continue;
                }

                *pool_counter
            } else {
                // Hybrid: full_counter[x] = max(template_count[x], pool_count[x])
                // This models template letters being implicitly in the anagram pool.
                // Note that a star wildcard in the anagram pool means nothing in this case.
                // (The only thing it *could* mean is "ignore the anagram and do what you like",
                // which isn't very interesting.)
                let mut anagram_counter = template_counter;
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
                if extra_count > num_wildcards && unused_count > num_wildcards {
                    continue;
                }

                anagram_counter
            };

            if !sub_patterns
                .iter()
                .all(|sp| candidate.contains(sp.as_str()))
            {
                continue;
            }

            // Match confirmed. Now (and only now) do the extra work of spelling out the
            // unused (pool − word) and extra (word − pool) letters for display.
            return Some(MatchInfo {
                unused: diff_letters(&effective_pool, &candidate_counter),
                extra: diff_letters(&candidate_counter, &effective_pool),
            });
        }

        None
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
    fn tight() -> CompileLimits {
        CompileLimits {
            max_pattern_len: 64,
            max_anagram_combos: 4_096,
            backtrack_limit: 10_000,
            max_fuzzy_steps: 10_000,
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
}
