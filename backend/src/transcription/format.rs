pub(crate) fn format_transcription(text: &str, language: Option<&str>) -> String {
    let mut text = collapse_whitespace(text);
    if text.is_empty() || contains_cjk(&text) {
        return text;
    }

    let english_rules = language.is_none_or(|language| {
        language.trim().is_empty()
            || language.eq_ignore_ascii_case("auto")
            || language.eq_ignore_ascii_case("en")
            || language.to_ascii_lowercase().starts_with("en-")
    });
    if english_rules {
        text = remove_fillers_and_stutters(&text);
        text = normalize_spoken_numbers(&text);
    }
    text = normalize_punctuation_spacing(&text);
    if english_rules {
        text = capitalize_sentences(&text);
        if text.split_whitespace().count() >= 3
            && text.chars().last().is_some_and(char::is_alphanumeric)
        {
            text.push('.');
        }
    }
    text
}

struct NumberToken<'a> {
    prefix: &'a str,
    word: &'a str,
    suffix: &'a str,
}

fn number_token(token: &str) -> NumberToken<'_> {
    let without_prefix = token.trim_start_matches(|ch: char| !ch.is_ascii_alphabetic());
    let prefix_len = token.len() - without_prefix.len();
    let word = without_prefix.trim_end_matches(|ch: char| !ch.is_ascii_alphabetic());
    NumberToken {
        prefix: &token[..prefix_len],
        word,
        suffix: &without_prefix[word.len()..],
    }
}

fn following_word<'a>(tokens: &'a [NumberToken<'_>], index: usize) -> Option<&'a str> {
    let token = tokens.get(index)?;
    if index > 0 && (!tokens[index - 1].suffix.is_empty() || !token.prefix.is_empty()) {
        return None;
    }
    Some(token.word)
}

fn small_number(word: &str) -> Option<u64> {
    Some(match word.to_ascii_lowercase().as_str() {
        "zero" => 0,
        "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        "thirteen" => 13,
        "fourteen" => 14,
        "fifteen" => 15,
        "sixteen" => 16,
        "seventeen" => 17,
        "eighteen" => 18,
        "nineteen" => 19,
        _ => return None,
    })
}

fn tens_number(word: &str) -> Option<u64> {
    Some(match word.to_ascii_lowercase().as_str() {
        "twenty" => 20,
        "thirty" => 30,
        "forty" => 40,
        "fifty" => 50,
        "sixty" => 60,
        "seventy" => 70,
        "eighty" => 80,
        "ninety" => 90,
        _ => return None,
    })
}

fn under_hundred(tokens: &[NumberToken<'_>], start: usize) -> Option<(u64, usize)> {
    let first = following_word(tokens, start)?;
    if let Some(tens) = tens_number(first) {
        let unit = following_word(tokens, start + 1).and_then(small_number);
        if let Some(unit @ 1..=9) = unit {
            return Some((tens + unit, start + 2));
        }
        return Some((tens, start + 1));
    }
    small_number(first).map(|value| (value, start + 1))
}

fn number_group(tokens: &[NumberToken<'_>], start: usize) -> Option<(u64, usize)> {
    let first = following_word(tokens, start)?;
    if let Some(unit @ 1..=9) = small_number(first) {
        if following_word(tokens, start + 1)
            .is_some_and(|word| word.eq_ignore_ascii_case("hundred"))
        {
            let mut next = start + 2;
            let and =
                following_word(tokens, next).is_some_and(|word| word.eq_ignore_ascii_case("and"));
            if and {
                next += 1;
            }
            if let Some((remainder, end)) = under_hundred(tokens, next) {
                return Some((unit * 100 + remainder, end));
            }
            return Some((unit * 100, start + 2));
        }
        // In spoken English, "three sixty" commonly means 360.
        if let Some(tens) = following_word(tokens, start + 1).and_then(tens_number) {
            let extra = following_word(tokens, start + 2).and_then(small_number);
            if let Some(extra @ 1..=9) = extra {
                return Some((unit * 100 + tens + extra, start + 3));
            }
            return Some((unit * 100 + tens, start + 2));
        }
    }
    under_hundred(tokens, start)
}

fn spoken_number(tokens: &[NumberToken<'_>], start: usize) -> Option<(u64, usize)> {
    let (group, end) = number_group(tokens, start)?;
    if group > 0
        && following_word(tokens, end).is_some_and(|word| word.eq_ignore_ascii_case("thousand"))
    {
        let mut next = end + 1;
        if following_word(tokens, next).is_some_and(|word| word.eq_ignore_ascii_case("and")) {
            next += 1;
        }
        if let Some((remainder, remainder_end)) = number_group(tokens, next) {
            return Some((group * 1000 + remainder, remainder_end));
        }
        return Some((group * 1000, end + 1));
    }
    Some((group, end))
}

fn normalize_spoken_numbers(text: &str) -> String {
    let words: Vec<_> = text.split_whitespace().collect();
    let tokens: Vec<_> = words.iter().map(|word| number_token(word)).collect();
    let mut output = Vec::with_capacity(words.len());
    let mut index = 0;
    while index < words.len() {
        if let Some((value, end)) = spoken_number(&tokens[index..], 0) {
            output.push(format!(
                "{}{}{}",
                tokens[index].prefix,
                value,
                tokens[index + end - 1].suffix
            ));
            index += end;
        } else {
            output.push(words[index].to_string());
            index += 1;
        }
    }
    output.join(" ")
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn remove_fillers_and_stutters(text: &str) -> String {
    const FILLERS: &[&str] = &["um", "uh", "erm", "hmm"];
    const FUNCTION_WORDS: &[&str] = &[
        "i", "a", "an", "the", "and", "or", "but", "to", "of", "in", "on", "for", "is", "are",
        "was", "were", "it", "this", "that", "we", "you", "he", "she", "they", "my", "your", "our",
    ];
    let mut output = Vec::<&str>::new();
    let mut previous_core = String::new();
    for token in text.split_whitespace() {
        let core = token
            .trim_matches(|character: char| !character.is_alphanumeric())
            .to_ascii_lowercase();
        if FILLERS.contains(&core.as_str()) {
            continue;
        }
        let duplicate = FUNCTION_WORDS.contains(&core.as_str())
            && core == previous_core
            && token.chars().all(|character| character.is_alphanumeric());
        if !duplicate {
            output.push(token);
        }
        previous_core = core;
    }
    output.join(" ")
}

fn normalize_punctuation_spacing(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if matches!(character, ',' | '.' | '!' | '?' | ';' | ':') {
            while output.ends_with(' ') {
                output.pop();
            }
            output.push(character);
            let next = chars.get(index + 1).copied();
            let embedded = (character == '.' && period_is_embedded(&chars, index))
                || (character == ':' && next == Some('/'));
            if !embedded
                && next.is_some_and(|next| {
                    !next.is_whitespace() && !matches!(next, ',' | '.' | '!' | '?' | ';' | ':')
                })
            {
                output.push(' ');
            }
        } else if character.is_whitespace() {
            if !output.is_empty() && !output.ends_with(' ') {
                output.push(' ');
            }
        } else {
            output.push(character);
        }
        index += 1;
    }
    output.trim().to_string()
}

fn capitalize_sentences(text: &str) -> String {
    let characters = text.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(text.len());
    let mut capitalize = true;
    for (index, character) in characters.iter().copied().enumerate() {
        if capitalize && character.is_alphabetic() {
            output.extend(character.to_uppercase());
            capitalize = false;
        } else {
            output.push(character);
            if !character.is_whitespace() && !matches!(character, '"' | '\'' | '(' | '[') {
                capitalize = false;
            }
        }
        let embedded_period = character == '.' && period_is_embedded(&characters, index);
        if matches!(character, '.' | '!' | '?') && !embedded_period {
            capitalize = true;
        }
    }
    output
}

fn period_is_embedded(characters: &[char], index: usize) -> bool {
    let previous = index
        .checked_sub(1)
        .and_then(|at| characters.get(at))
        .copied();
    let next = characters.get(index + 1).copied();
    if previous.is_some_and(|value| value.is_ascii_digit())
        && next.is_some_and(|value| value.is_ascii_digit())
    {
        return true;
    }
    let token_start = characters[..index]
        .iter()
        .rposition(|character| character.is_whitespace())
        .map_or(0, |position| position + 1);
    let token_end = characters[index + 1..]
        .iter()
        .position(|character| character.is_whitespace())
        .map_or(characters.len(), |position| index + 1 + position);
    let token = characters[token_start..token_end]
        .iter()
        .collect::<String>();
    let lower = token.to_ascii_lowercase();
    lower.contains('@')
        || lower.contains("://")
        || lower.contains('/')
        || lower.contains('\\')
        || lower.contains('`')
        || [".com", ".org", ".net", ".io", ".dev", ".app", ".co"]
            .iter()
            .any(|suffix| lower.ends_with(suffix))
}

fn contains_cjk(text: &str) -> bool {
    text.chars().any(|character| {
        matches!(character as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_spacing_capitals_and_terminal_punctuation() {
        assert_eq!(
            format_transcription("  hello ,world.this is fine  ", Some("en")),
            "Hello, world. This is fine."
        );
    }

    #[test]
    fn removes_only_standalone_english_fillers() {
        assert_eq!(
            format_transcription("um this uh is ergonomic hmm", Some("en")),
            "This is ergonomic."
        );
        assert_eq!(
            format_transcription("the album is here", Some("en")),
            "The album is here."
        );
    }

    #[test]
    fn collapses_only_short_function_word_stutters() {
        assert_eq!(
            format_transcription("I I think the the plan works", Some("en")),
            "I think the plan works."
        );
        assert_eq!(
            format_transcription("very very good work", Some("en")),
            "Very very good work."
        );
    }

    #[test]
    fn preserves_urls_emails_decimals_and_midword_caps() {
        assert_eq!(
            format_transcription(
                "visit https://example.com or email dev@example.com at 3.5 GHz",
                Some("en")
            ),
            "Visit https://example.com or email dev@example.com at 3.5 GHz."
        );
        assert_eq!(
            format_transcription("use AxiusFlow in iOS today", Some("en")),
            "Use AxiusFlow in iOS today."
        );
    }

    #[test]
    fn skips_english_rules_for_other_languages_and_cjk() {
        assert_eq!(
            format_transcription("um bonjour tout le monde", Some("fr")),
            "um bonjour tout le monde"
        );
        assert_eq!(
            format_transcription("  今日は 世界  ", Some("auto")),
            "今日は 世界"
        );
    }

    #[test]
    fn short_or_already_punctuated_text_is_not_forced() {
        assert_eq!(
            format_transcription("hello there", Some("en")),
            "Hello there"
        );
        assert_eq!(
            format_transcription("are you there?", Some("en")),
            "Are you there?"
        );
    }

    #[test]
    fn writes_spoken_english_numbers_as_digits() {
        assert_eq!(format_transcription("three sixty", Some("en")), "360");
        assert_eq!(
            format_transcription("rotate it three sixty degrees", Some("en")),
            "Rotate it 360 degrees."
        );
        assert_eq!(
            format_transcription("three hundred and sixty five, then twenty one", Some("en")),
            "365, then 21."
        );
        assert_eq!(
            format_transcription("one thousand two hundred and five", Some("en")),
            "1205"
        );
        assert_eq!(format_transcription("zero to nine", Some("en")), "0 to 9.");
    }

    #[test]
    fn number_formatting_respects_punctuation_and_language() {
        assert_eq!(
            format_transcription("three, sixty, ninety.", Some("en")),
            "3, 60, 90."
        );
        assert_eq!(
            format_transcription("three sixty", Some("fr")),
            "three sixty"
        );
        assert_eq!(
            format_transcription("version 3.5 is ready", Some("en")),
            "Version 3.5 is ready."
        );
    }
}
