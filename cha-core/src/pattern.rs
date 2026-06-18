use fancy_regex::Regex;
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

pub fn compile_pattern(pattern_str: &str) -> Result<Matcher, PatternError> {
    let parts: Vec<&str> = pattern_str.split('&').collect();
    let mut matchers: Vec<(bool, Matcher)> = Vec::new();

    for part in parts {
        let part = part.trim();
        let (negate, actual) = if let Some(rest) = part.strip_prefix('!') {
            (true, rest.trim())
        } else {
            (false, part)
        };
        matchers.push((negate, compile_one_pattern(actual)?));
    }

    let has_punct = pattern_str.chars().any(|c| PUNCTUATION.contains(&c));

    Ok(Box::new(move |word: &str| {
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
    }))
}

fn compile_one_pattern(pattern: &str) -> Result<Matcher, PatternError> {
    if let Some(idx) = pattern.find(';') {
        if idx == 0 {
            compile_anagram(None, &pattern[1..])
        } else {
            compile_anagram(Some(&pattern[..idx]), &pattern[idx + 1..])
        }
    } else {
        compile_template(pattern)
    }
}

fn compile_template(template: &str) -> Result<Matcher, PatternError> {
    let regex_str = template_to_regex(template)?;
    let re = Regex::new(&format!("(?i)^{}$", regex_str))
        .map_err(|e| PatternError(format!("Invalid template '{}': {}", template, e)))?;
    Ok(Box::new(move |word: &str| {
        if re.is_match(word).unwrap_or(false) {
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

fn count_str(s: &str) -> ([usize; 26], usize) {
    let mut counts = [0usize; 26];
    let mut len = 0;
    for b in s.bytes() {
        if b.is_ascii_alphabetic() {
            counts[(b.to_ascii_lowercase() - b'a') as usize] += 1;
            len += 1;
        }
    }
    (counts, len)
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

fn compile_anagram(template: Option<&str>, anagram_expr: &str) -> Result<Matcher, PatternError> {
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

    let choice_combos: Vec<Vec<char>> = if choices.is_empty() {
        vec![vec![]]
    } else {
        cartesian_product(&choices)
    };

    let template_matcher: Option<Matcher> = template.map(compile_template).transpose()?;
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

        let (candidate_counter, candidate_len) = count_str(candidate);

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
}
