use fancy_regex::Regex;
use std::borrow::Cow;
use std::collections::HashMap;

const PUNCTUATION: &[char] = &[' ', '-', '\''];

pub type Matcher = Box<dyn Fn(&str) -> bool>;

pub fn compile_pattern(pattern_str: &str) -> Matcher {
    let parts: Vec<&str> = pattern_str.split('&').collect();
    let mut matchers: Vec<(bool, Matcher)> = Vec::new();

    for part in parts {
        let part = part.trim();
        let (negate, actual) = if let Some(rest) = part.strip_prefix('!') {
            (true, rest.trim())
        } else {
            (false, part)
        };
        matchers.push((negate, compile_one_pattern(actual)));
    }

    let has_punct = pattern_str.chars().any(|c| PUNCTUATION.contains(&c));

    Box::new(move |word: &str| {
        let test_word: Cow<str> = if has_punct {
            Cow::Borrowed(word)
        } else if word.chars().any(|c| PUNCTUATION.contains(&c)) {
            Cow::Owned(word.chars().filter(|c| !PUNCTUATION.contains(c)).collect())
        } else {
            Cow::Borrowed(word)
        };
        matchers.iter().all(|(negate, m)| {
            let result = m(&test_word);
            if *negate { !result } else { result }
        })
    })
}

fn compile_one_pattern(pattern: &str) -> Matcher {
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

fn compile_template(template: &str) -> Matcher {
    let regex_str = template_to_regex(template);
    let re = Regex::new(&format!("(?i)^{}$", regex_str))
        .unwrap_or_else(|e| panic!("Invalid template '{}': {}", template, e));
    Box::new(move |word: &str| re.is_match(word).unwrap_or(false))
}

fn escape_in_char_class(c: char) -> String {
    if matches!(c, ']' | '\\' | '^' | '-') {
        format!("\\{}", c)
    } else {
        c.to_string()
    }
}

fn template_to_regex(template: &str) -> String {
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
                let rel = chars[i..].iter().position(|&x| x == ']')
                    .expect("Unclosed '[' in template");
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
                if seen_vars.contains_key(&c) {
                    out.push_str(&format!("(?P={})", name));
                } else {
                    seen_vars.insert(c, true);
                    out.push_str(&format!("(?P<{}>[a-z])", name));
                }
            }
            c @ ('-' | '\'' | ' ') => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_alphabetic() => {
                out.push_str(&fancy_regex::escape(
                    &c.to_lowercase().to_string(),
                ));
            }
            c => panic!("Template has meaningless character '{}'", c),
        }
        i += 1;
    }
    out
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

fn compile_anagram(template: Option<&str>, anagram_expr: &str) -> Matcher {
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
                let rel = chars[i..].iter().position(|&x| x == ']')
                    .expect("Unclosed '[' in anagram");
                let j = i + rel;
                choices.push(chars[i + 1..j].to_vec());
                i = j;
            }
            '(' => {
                let rel = chars[i..].iter().position(|&x| x == ')')
                    .expect("Unclosed '(' in anagram");
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
            c => panic!("Anagram has meaningless character '{}'", c),
        }
        i += 1;
    }

    let choice_combos: Vec<Vec<char>> = if choices.is_empty() {
        vec![vec![]]
    } else {
        cartesian_product(&choices)
    };

    let template_matcher: Option<Matcher> = template.map(|t| compile_template(t));
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
    let combo_pools: Vec<([usize; 26], usize)> = choice_combos.iter().map(|combo| {
        let mut counter = fixed_counter;
        for &c in combo {
            counter[(c.to_ascii_lowercase() as u8 - b'a') as usize] += 1;
        }
        (counter, fixed_size + combo.len())
    }).collect();

    Box::new(move |word: &str| {
        if let Some(ref tm) = template_matcher {
            if !tm(word) {
                return false;
            }
        }

        let (word_counter, word_len) = count_str(word);

        'combo: for (pool_counter, pool_base) in &combo_pools {
            let pool_size = pool_base + num_wildcards;

            if is_pure && !has_star && word_len != pool_size {
                continue;
            }

            if is_pure {
                for i in 0..26 {
                    if pool_counter[i] > 0 && word_counter[i] < pool_counter[i] {
                        continue 'combo;
                    }
                }

                let extras: usize = (0..26)
                    .map(|i| word_counter[i].saturating_sub(pool_counter[i]))
                    .sum();

                if !has_star && extras != num_wildcards {
                    continue;
                }
            } else {
                // Hybrid: full_counter[x] = max(template_count[x], pool_count[x])
                // This models template letters being implicitly in the anagram pool.
                let mut full_counter = template_counter;
                for i in 0..26 {
                    if pool_counter[i] > full_counter[i] {
                        full_counter[i] = pool_counter[i];
                    }
                }

                let extras: usize = (0..26)
                    .map(|i| word_counter[i].saturating_sub(full_counter[i]))
                    .sum();

                if !has_star && extras > num_wildcards {
                    continue;
                }
            }

            if !sub_patterns.iter().all(|sp| word.contains(sp.as_str())) {
                continue;
            }

            return true;
        }

        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dot_wildcard() {
        let m = compile_pattern(".l...r.n");
        assert!(m("electron"));
    }

    #[test]
    fn test_dot_wildcard_wrong_length() {
        let m = compile_pattern(".l...r.n");
        assert!(!m("electrons"));
    }

    #[test]
    fn test_dot_wildcard_wrong_letter() {
        let m = compile_pattern(".l...r.n");
        assert!(!m("xxxxxxxx"));
    }

    #[test]
    fn test_fixed_letters() {
        let m = compile_pattern("cat");
        assert!(m("cat"));
        assert!(!m("bat"));
        assert!(!m("cats"));
    }

    #[test]
    fn test_case_insensitive() {
        let m = compile_pattern("cat");
        assert!(m("Cat"));
        assert!(m("CAT"));
    }

    #[test]
    fn test_case_insensitive_pattern() {
        let m = compile_pattern("Cat");
        assert!(m("Cat"));
        assert!(m("cat"));
        assert!(m("CAT"));
    }

    #[test]
    fn test_star() {
        let m = compile_pattern("m*ja");
        assert!(m("maharaja"));
    }

    #[test]
    fn test_star_zero_chars() {
        let m = compile_pattern("m*m");
        assert!(m("mm"));
        assert!(m("mom"));
        assert!(m("madam"));
    }

    #[test]
    fn test_star_at_start() {
        let m = compile_pattern("*ing");
        assert!(m("sing"));
        assert!(m("running"));
        assert!(m("ing"));
    }

    #[test]
    fn test_star_at_end() {
        let m = compile_pattern("un*");
        assert!(m("un"));
        assert!(m("under"));
    }

    #[test]
    fn test_basic_anagram() {
        let m = compile_pattern(";lobikes");
        assert!(m("obelisk"));
    }

    #[test]
    fn test_anagram_wrong_letters() {
        let m = compile_pattern(";lobikes");
        assert!(!m("oblique"));
    }

    #[test]
    fn test_anagram_wrong_length() {
        let m = compile_pattern(";lobikes");
        assert!(!m("obeli"));
    }

    #[test]
    fn test_anagram_with_wildcards() {
        let m = compile_pattern(";..oting");
        assert!(m("tonight"));
        assert!(m("tooting"));
        assert!(m("outings"));
    }

    #[test]
    fn test_anagram_with_wildcards_wrong_length() {
        let m = compile_pattern(";..oting");
        assert!(!m("toot"));
    }

    #[test]
    fn test_anagram_choice() {
        let m = compile_pattern(";diners[ai]");
        assert!(m("insider"));
        assert!(m("sardine"));
    }

    #[test]
    fn test_template_choice() {
        let m = compile_pattern("c[aou]t");
        assert!(m("cat"));
        assert!(m("cot"));
        assert!(m("cut"));
        assert!(!m("cet"));
    }

    #[test]
    fn test_template_with_anagram() {
        let m = compile_pattern("......*;gdangboot");
        assert!(m("toboggan"));
    }

    #[test]
    fn test_hybrid_unused_pool_letters() {
        let m = compile_pattern("......*;gdangboot");
        assert!(m("toboggan"));
    }

    #[test]
    fn test_hybrid_template_letters_in_pool() {
        let m = compile_pattern("z....;brae");
        assert!(m("zebra"));
    }

    #[test]
    fn test_hybrid_template_letters_in_pool_no_match() {
        let m = compile_pattern("z....;brae");
        assert!(!m("zesty"));
    }

    #[test]
    fn test_hybrid_wrong_letters() {
        let m = compile_pattern("......*;gdangboot");
        assert!(!m("xxxxxxxx"));
    }

    #[test]
    fn test_hybrid_template_constraint() {
        let m = compile_pattern("t*;toboggan");
        assert!(m("toboggan"));
        let reversed: String = "toboggan".chars().rev().collect();
        assert!(!m(&reversed));
    }

    #[test]
    fn test_hybrid_with_redundancy() {
        let m = compile_pattern("obel...;lobikes");
        assert!(m("obelisk"));
        assert!(!m("obelise"));
    }

    #[test]
    fn test_palindrome_pattern() {
        let m = compile_pattern("1234321");
        assert!(m("deified"));
    }

    #[test]
    fn test_variable_mismatch() {
        let m = compile_pattern("1234321");
        assert!(!m("abcdefg"));
    }

    #[test]
    fn test_repeated_variable() {
        let m = compile_pattern("1221");
        assert!(m("abba"));
        assert!(!m("abcd"));
    }

    #[test]
    fn test_single_variable() {
        let m = compile_pattern("11111");
        assert!(m("aaaaa"));
        assert!(!m("aabaa"));
    }

    #[test]
    fn test_hyphen_pattern() {
        let m = compile_pattern("...-..-....."); // fly-by-night (3-2-5)
        assert!(m("fly-by-night"));
        assert!(!m("onetofourfive"));
    }

    #[test]
    fn test_no_punct_strips_words() {
        let m = compile_pattern(";lobikes");
        assert!(m("obelisk"));
    }

    #[test]
    fn test_apostrophe_in_pattern() {
        let m = compile_pattern("it's");
        assert!(m("it's"));
        assert!(!m("its"));
    }

    #[test]
    fn test_vowel_consonant_alternation() {
        let m = compile_pattern("@#@#@#@#@#@");
        assert!(m("imaginative"));
        assert!(m("inoperative"));
    }

    #[test]
    fn test_vowel() {
        let m = compile_pattern("@");
        assert!(m("a"));
        assert!(m("e"));
        assert!(!m("b"));
    }

    #[test]
    fn test_consonant() {
        let m = compile_pattern("#");
        assert!(m("b"));
        assert!(m("z"));
        assert!(!m("a"));
    }

    #[test]
    fn test_subpattern_match() {
        let m = compile_pattern(";(che)rostra");
        assert!(m("orchestra"));
    }

    #[test]
    fn test_subpattern_no_contiguous_match() {
        let m = compile_pattern(";(che)rostra");
        assert!(!m("carthorse"));
    }

    #[test]
    fn test_and() {
        let m = compile_pattern("c.. & *at");
        assert!(m("cat"));
        assert!(!m("cob"));
        assert!(!m("bat"));
    }

    #[test]
    fn test_not() {
        let m = compile_pattern("! *ing");
        assert!(m("cat"));
        assert!(!m("running"));
    }

    #[test]
    fn test_not_filters_suffix() {
        let m = compile_pattern(";..oting & ! *ing");
        let m_star_ing = compile_pattern("*ing");
        assert!(m_star_ing("tooting"));
        assert!(!m("tooting"));
    }

    #[test]
    fn test_multiple_and() {
        let m = compile_pattern("c* & *t & ...");
        assert!(m("cat"));
        assert!(m("cot"));
        assert!(m("cut"));
        assert!(!m("cart"));
        assert!(!m("ca"));
    }

    #[test]
    fn test_multiple_negations() {
        let m = compile_pattern("!c* & !*t");
        assert!(m("box"));
        assert!(!m("cat"));
        assert!(!m("bat"));
        assert!(!m("cob"));
    }

    #[test]
    fn test_empty_word() {
        let m = compile_pattern("*");
        assert!(m(""));
    }

    #[test]
    fn test_single_dot() {
        let m = compile_pattern(".");
        assert!(m("a"));
        assert!(m("z"));
        assert!(!m("ab"));
    }

    #[test]
    fn test_only_star() {
        let m = compile_pattern("*");
        assert!(m("anything"));
        assert!(m(""));
    }
}
