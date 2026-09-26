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
}
