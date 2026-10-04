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
        text = collapse_repeated_phrases(&text);
        text = normalize_spoken_numbers(&text);
        text = fix_common_word_errors(&text);
    }
    text = normalize_punctuation_spacing(&text);
    if english_rules {
        text = capitalize_sentences(&text);
        text = trim_dangling_separators(&text);
        if text.split_whitespace().count() >= 3
            && text.chars().last().is_some_and(char::is_alphanumeric)
        {
            text.push('.');
        }
    }
    text
}

struct WordToken<'a> {
    raw: &'a str,
    prefix: &'a str,
    word: &'a str,
    suffix: &'a str,
}

fn split_token(token: &str, is_word_char: fn(char) -> bool) -> WordToken<'_> {
    let without_prefix = token.trim_start_matches(|ch: char| !is_word_char(ch));
    let prefix_len = token.len() - without_prefix.len();
    let word = without_prefix.trim_end_matches(|ch: char| !is_word_char(ch));
    WordToken {
        raw: token,
        prefix: &token[..prefix_len],
        word,
        suffix: &without_prefix[word.len()..],
    }
}

fn number_token(token: &str) -> WordToken<'_> {
    split_token(token, |ch| ch.is_ascii_alphabetic())
}

fn grammar_token(token: &str) -> WordToken<'_> {
    split_token(token, char::is_alphanumeric)
}

fn following_word<'a>(tokens: &'a [WordToken<'_>], index: usize) -> Option<&'a str> {
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

fn scale_value(word: &str) -> Option<u64> {
    Some(match word.to_ascii_lowercase().as_str() {
        "thousand" => 1_000,
        "lakh" | "lakhs" | "lac" | "lacs" => 100_000,
        "million" => 1_000_000,
        "crore" | "crores" => 10_000_000,
        "billion" => 1_000_000_000,
        "trillion" => 1_000_000_000_000,
        _ => return None,
    })
}

fn repeat_count(word: &str) -> Option<usize> {
    Some(match word.to_ascii_lowercase().as_str() {
        "double" => 2,
        "triple" => 3,
        "quadruple" => 4,
        _ => return None,
    })
}

fn is_oh(word: &str) -> bool {
    word.eq_ignore_ascii_case("oh") || word.eq_ignore_ascii_case("o")
}

fn single_digit(word: &str) -> Option<u64> {
    small_number(word).filter(|value| *value <= 9)
}

fn is_number_word(word: &str) -> bool {
    small_number(word).is_some()
        || tens_number(word).is_some()
        || scale_value(word).is_some()
        || word.eq_ignore_ascii_case("hundred")
}

fn is_magnitude_word(word: &str) -> bool {
    word.eq_ignore_ascii_case("hundred")
        || word.eq_ignore_ascii_case("point")
        || scale_value(word).is_some()
}

fn is_meridiem(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "am" | "pm" | "a.m" | "p.m"
    )
}

fn under_hundred(tokens: &[WordToken<'_>], start: usize) -> Option<(u64, usize)> {
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

fn hundreds(
    tokens: &[WordToken<'_>],
    multiplier: u64,
    hundred_index: usize,
) -> Option<(u64, usize)> {
    if !following_word(tokens, hundred_index)
        .is_some_and(|word| word.eq_ignore_ascii_case("hundred"))
    {
        return None;
    }
    let mut next = hundred_index + 1;
    if following_word(tokens, next).is_some_and(|word| word.eq_ignore_ascii_case("and")) {
        next += 1;
    }
    if let Some((remainder, end)) = under_hundred(tokens, next) {
        return Some((multiplier * 100 + remainder, end));
    }
    Some((multiplier * 100, hundred_index + 1))
}

fn number_group(tokens: &[WordToken<'_>], start: usize) -> Option<(u64, usize)> {
    let first = following_word(tokens, start)?;
    if let Some(unit @ 1..=9) = small_number(first) {
        if let Some(result) = hundreds(tokens, unit, start + 1) {
            return Some(result);
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
    let (value, end) = under_hundred(tokens, start)?;
    if value >= 10 {
        // "nineteen hundred", "twenty five hundred"
        if let Some(result) = hundreds(tokens, value, end) {
            return Some(result);
        }
    }
    Some((value, end))
}

/// Parses a cardinal number with optional scales ("one lakh ten thousand and one")
/// and an optional decimal part ("three point one four").
fn cardinal(tokens: &[WordToken<'_>]) -> Option<(String, usize)> {
    let (first_group, mut end) = number_group(tokens, 0)?;
    let mut total = 0u64;
    let mut current = first_group;
    let mut last_scale = u64::MAX;
    let mut scales_used = 0;
    let mut last_scale_word = "";
    while current > 0 {
        let Some(word) = following_word(tokens, end) else {
            break;
        };
        let Some(scale) = scale_value(word) else {
            break;
        };
        if scale >= last_scale {
            break;
        }
        total = total.checked_add(current.checked_mul(scale)?)?;
        last_scale = scale;
        last_scale_word = word;
        scales_used += 1;
        end += 1;
        current = 0;

        let mut next = end;
        if following_word(tokens, next).is_some_and(|word| word.eq_ignore_ascii_case("and")) {
            next += 1;
        }
        if let Some((group, group_end)) = number_group(tokens, next) {
            let next_scale = following_word(tokens, group_end).and_then(scale_value);
            if next_scale.is_some_and(|next_scale| next_scale >= last_scale) {
                break;
            }
            current = group;
            end = group_end;
        }
    }
    total = total.checked_add(current)?;

    if scales_used == 0 {
        if let Some((fraction, fraction_end)) = decimal_fraction(tokens, end) {
            let mut rendered = format!("{total}.{fraction}");
            let mut end = fraction_end;
            if let Some(word) =
                following_word(tokens, end).filter(|word| scale_value(word).is_some())
            {
                rendered.push(' ');
                rendered.push_str(&word.to_ascii_lowercase());
                end += 1;
            }
            return Some((rendered, end));
        }
    }

    if scales_used == 1 && current == 0 && last_scale >= 100_000 {
        return Some((
            format!("{first_group} {}", last_scale_word.to_ascii_lowercase()),
            end,
        ));
    }
    Some((total.to_string(), end))
}

fn decimal_fraction(tokens: &[WordToken<'_>], point_index: usize) -> Option<(String, usize)> {
    if !following_word(tokens, point_index).is_some_and(|word| word.eq_ignore_ascii_case("point")) {
        return None;
    }
    let mut digits = String::new();
    let mut end = point_index + 1;
    while let Some(word) = following_word(tokens, end) {
        let digit = if is_oh(word) {
            Some(0)
        } else {
            single_digit(word)
        };
        let Some(digit) = digit else {
            break;
        };
        digits.push(char::from(b'0' + digit as u8));
        end += 1;
    }
    (!digits.is_empty()).then_some((digits, end))
}

/// "three thirty pm" -> "3:30", leaving the am/pm marker in place.
fn spoken_time(tokens: &[WordToken<'_>]) -> Option<(String, usize)> {
    let (hour, mut end) = under_hundred(tokens, 0)?;
    if !(1..=12).contains(&hour) {
        return None;
    }
    let next = following_word(tokens, end)?;
    let minute = if is_oh(next) || next.eq_ignore_ascii_case("zero") {
        let digit = following_word(tokens, end + 1)
            .and_then(single_digit)
            .filter(|digit| *digit >= 1)?;
        end += 2;
        digit
    } else {
        let (minute, minute_end) = under_hundred(tokens, end)?;
        if !(10..=59).contains(&minute) {
            return None;
        }
        end = minute_end;
        minute
    };
    if !following_word(tokens, end).is_some_and(is_meridiem) {
        return None;
    }
    Some((format!("{hour}:{minute:02}"), end))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunKind {
    Digit,
    Repeat,
    Pair,
}

struct RunElement {
    kind: RunKind,
    value: u64,
    text: String,
}

/// Digit-by-digit speech such as PIN codes and phone numbers ("one one zero
/// zero zero one", "eleven triple zero one", "nine one one"), spoken years
/// ("nineteen ninety nine") and approximate ranges ("two three days").
fn digit_sequence(tokens: &[WordToken<'_>]) -> Option<(String, usize)> {
    let mut elements = Vec::<RunElement>::new();
    let mut end = 0;
    while let Some(word) = following_word(tokens, end) {
        if let Some(count) = repeat_count(word) {
            let digit = following_word(tokens, end + 1).and_then(|next| {
                if is_oh(next) {
                    Some(0)
                } else {
                    single_digit(next)
                }
            });
            let Some(digit) = digit else {
                break;
            };
            elements.push(RunElement {
                kind: RunKind::Repeat,
                value: digit,
                text: digit.to_string().repeat(count),
            });
            end += 2;
        } else if is_oh(word) {
            let digit_follows = following_word(tokens, end + 1)
                .is_some_and(|next| single_digit(next).is_some() || repeat_count(next).is_some());
            if elements.is_empty() || !digit_follows {
                break;
            }
            elements.push(RunElement {
                kind: RunKind::Digit,
                value: 0,
                text: "0".to_string(),
            });
            end += 1;
        } else if let Some(digit) = single_digit(word) {
            elements.push(RunElement {
                kind: RunKind::Digit,
                value: digit,
                text: digit.to_string(),
            });
            end += 1;
        } else if let Some((value, pair_end)) = under_hundred(tokens, end) {
            elements.push(RunElement {
                kind: RunKind::Pair,
                value,
                text: value.to_string(),
            });
            end = pair_end;
        } else {
            break;
        }
    }

    if elements.is_empty() || following_word(tokens, end).is_some_and(is_magnitude_word) {
        return None;
    }

    let count = |kind| {
        elements
            .iter()
            .filter(|element| element.kind == kind)
            .count()
    };
    let (digits, repeats, pairs) = (
        count(RunKind::Digit),
        count(RunKind::Repeat),
        count(RunKind::Pair),
    );
    let concatenated = || -> String {
        elements
            .iter()
            .map(|element| element.text.as_str())
            .collect()
    };

    if repeats > 0 {
        return Some((concatenated(), end));
    }
    if elements.len() == 2 {
        let (first, second) = (&elements[0], &elements[1]);
        if pairs == 2
            && (15..=20).contains(&first.value)
            && second.value >= 10
            && !(second.value == first.value + 1 && first.value < 19)
        {
            return Some((concatenated(), end));
        }
        if first.value >= 1 && second.value == first.value + 1 {
            return Some((format!("{}-{}", first.value, second.value), end));
        }
        if digits == 2 {
            return Some((concatenated(), end));
        }
        if first.kind == RunKind::Digit
            && first.value >= 1
            && second.kind == RunKind::Pair
            && second.value >= 20
        {
            return Some((concatenated(), end));
        }
        return None;
    }
    if elements.len() >= 3 && digits >= 2 {
        return Some((concatenated(), end));
    }
    None
}

fn is_number_token(token: &WordToken<'_>) -> bool {
    let core = token.raw.trim_matches(|ch: char| !ch.is_alphanumeric());
    is_number_word(token.word) || (!core.is_empty() && core.chars().all(|ch| ch.is_ascii_digit()))
}

fn joined(tokens: &[WordToken<'_>], left: usize, right: usize) -> bool {
    tokens[left].suffix.is_empty() && tokens[right].prefix.chars().all(|ch| ch.is_ascii_digit())
}

/// A lone "one" is usually a pronoun ("no one", "which one", "one of them").
/// Only write it as a digit when the surrounding words make it numeric.
fn one_reads_as_number(tokens: &[WordToken<'_>], index: usize) -> bool {
    const LABELS: &[&str] = &[
        "page", "step", "chapter", "version", "number", "level", "room", "floor", "line", "item",
        "option", "part", "phase", "round", "section", "figure", "table", "track", "episode",
        "season", "volume", "grade", "slide", "question", "gate", "platform", "terminal", "row",
        "column", "day", "week", "year", "class", "rule", "lane", "sector", "block", "ward",
    ];
    const UNITS: &[&str] = &[
        "percent",
        "dollar",
        "dollars",
        "rupee",
        "rupees",
        "euro",
        "euros",
        "pound",
        "pounds",
        "cent",
        "cents",
        "yen",
        "kg",
        "kgs",
        "kilo",
        "kilos",
        "kilogram",
        "kilograms",
        "gram",
        "grams",
        "km",
        "kilometer",
        "kilometers",
        "kilometre",
        "kilometres",
        "mile",
        "miles",
        "meter",
        "meters",
        "metre",
        "metres",
        "cm",
        "mm",
        "inch",
        "inches",
        "liter",
        "liters",
        "litre",
        "litres",
        "ml",
        "gb",
        "mb",
        "tb",
        "kb",
        "gigabyte",
        "gigabytes",
        "megabyte",
        "megabytes",
        "terabyte",
        "terabytes",
        "ghz",
        "mhz",
        "am",
        "pm",
        "a.m",
        "p.m",
        "o'clock",
    ];
    const CONNECTORS: &[&str] = &[
        "to", "or", "through", "and", "plus", "minus", "times", "equals",
    ];

    if tokens.len() == 1 {
        return true;
    }
    let lower = |token: &WordToken<'_>| token.word.to_ascii_lowercase();
    let other_number =
        |token: &WordToken<'_>| is_number_token(token) && !token.word.eq_ignore_ascii_case("one");
    let listed = |left: usize, right: usize| {
        matches!(tokens[left].suffix, "" | ",")
            && tokens[right].prefix.chars().all(|ch| ch.is_ascii_digit())
    };

    if index > 0 && listed(index - 1, index) && other_number(&tokens[index - 1]) {
        return true;
    }
    if index + 1 < tokens.len() && listed(index, index + 1) && other_number(&tokens[index + 1]) {
        return true;
    }
    if index > 0 && joined(tokens, index - 1, index) {
        let previous = &tokens[index - 1];
        if LABELS.contains(&lower(previous).as_str()) {
            return true;
        }
        if CONNECTORS.contains(&lower(previous).as_str())
            && index > 1
            && joined(tokens, index - 2, index - 1)
            && other_number(&tokens[index - 2])
        {
            return true;
        }
    }
    if index + 1 < tokens.len() && joined(tokens, index, index + 1) {
        let next = &tokens[index + 1];
        if UNITS.contains(&lower(next).as_str()) || next.raw.starts_with('%') {
            return true;
        }
        if CONNECTORS.contains(&lower(next).as_str())
            && index + 2 < tokens.len()
            && joined(tokens, index + 1, index + 2)
            && other_number(&tokens[index + 2])
        {
            return true;
        }
    }
    false
}

fn normalize_spoken_numbers(text: &str) -> String {
    let words: Vec<_> = text.split_whitespace().collect();
    let tokens: Vec<_> = words.iter().map(|word| number_token(word)).collect();
    let mut output = Vec::with_capacity(words.len());
    let mut index = 0;
    while index < words.len() {
        let rest = &tokens[index..];
        let parsed = spoken_time(rest)
            .or_else(|| digit_sequence(rest))
            .or_else(|| cardinal(rest))
            .filter(|(_, end)| {
                *end > 1
                    || !tokens[index].word.eq_ignore_ascii_case("one")
                    || one_reads_as_number(&tokens, index)
            });
        if let Some((rendered, end)) = parsed {
            output.push(format!(
                "{}{}{}",
                tokens[index].prefix,
                rendered,
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

const STUTTER_WORDS: &[&str] = &[
    "i", "a", "an", "the", "and", "or", "but", "to", "of", "in", "on", "for", "is", "are", "was",
    // "that that" and "had had" are often grammatical, so they are not listed.
    "were", "it", "this", "we", "you", "he", "she", "they", "my", "your", "our", "with", "at",
    "from", "by", "as", "if", "me", "us", "them", "their", "his", "its", "would",
];

fn is_filler(core: &str) -> bool {
    const FILLERS: &[&str] = &["um", "umm", "uh", "uhh", "uhm", "erm", "er", "hmm", "hm"];
    // Keep acronyms such as "ER" or "UH".
    core.chars().skip(1).all(|ch| !ch.is_uppercase())
        && FILLERS.contains(&core.to_ascii_lowercase().as_str())
}

fn remove_fillers_and_stutters(text: &str) -> String {
    // Words after which a comma before a removed filler is still natural ("So, um, I").
    const INTERJECTIONS: &[&str] = &[
        "so",
        "well",
        "yeah",
        "yes",
        "no",
        "okay",
        "ok",
        "oh",
        "right",
        "now",
        "alright",
        "actually",
        "anyway",
        "basically",
        "honestly",
        "hey",
        "hi",
        "hello",
    ];
    let mut output = Vec::<String>::new();
    let mut previous_core = String::new();
    for token in text.split_whitespace() {
        let raw_core = token.trim_matches(|character: char| !character.is_alphanumeric());
        if is_filler(raw_core) {
            let trailing = token
                .rsplit_once(raw_core)
                .map_or("", |(_, trailing)| trailing);
            if let Some(previous) = output.last_mut() {
                let terminal: String = trailing
                    .chars()
                    .filter(|ch| matches!(ch, '.' | '!' | '?'))
                    .collect();
                if !terminal.is_empty() {
                    while previous.ends_with([',', ';']) {
                        previous.pop();
                    }
                    if !previous.ends_with(['.', '!', '?']) {
                        previous.push_str(&terminal);
                    }
                } else if trailing.contains(',') && previous.ends_with(',') {
                    let previous_core = previous
                        .trim_matches(|character: char| !character.is_alphanumeric())
                        .to_ascii_lowercase();
                    if !INTERJECTIONS.contains(&previous_core.as_str()) {
                        previous.pop();
                    }
                }
            }
            continue;
        }
        let core = raw_core.to_ascii_lowercase();
        let duplicate = STUTTER_WORDS.contains(&core.as_str())
            && core == previous_core
            && token.chars().all(|character| character.is_alphanumeric());
        if !duplicate {
            output.push(token.to_string());
        }
        previous_core = core;
    }
    output.join(" ")
}

/// Drops an immediately repeated 2-4 word phrase ("I think I think it works").
fn collapse_repeated_phrases(text: &str) -> String {
    let words: Vec<_> = text.split_whitespace().collect();
    let core = |word: &str| {
        word.trim_matches(|ch: char| !ch.is_alphanumeric())
            .to_lowercase()
    };
    let plain = |word: &str| word.chars().all(|ch| ch.is_alphanumeric() || ch == '\'');
    let mut output = Vec::with_capacity(words.len());
    let mut index = 0;
    'words: while index < words.len() {
        for size in (2..=4).rev() {
            if index + 2 * size > words.len() {
                continue;
            }
            let first = &words[index..index + size];
            let second = &words[index + size..index + 2 * size];
            let first_is_plain = first[..size - 1].iter().all(|word| plain(word))
                && (plain(first[size - 1])
                    || first[size - 1]
                        .strip_suffix(',')
                        .is_some_and(|word| plain(word)));
            let second_is_plain = second[..size - 1].iter().all(|word| plain(word))
                && second[size - 1]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric);
            let same = first
                .iter()
                .zip(second)
                .all(|(left, right)| core(left) == core(right));
            let has_function_word = first
                .iter()
                .any(|word| STUTTER_WORDS.contains(&core(word).as_str()));
            if first_is_plain && second_is_plain && same && has_function_word {
                index += size;
                continue 'words;
            }
        }
        output.push(words[index]);
        index += 1;
    }
    output.join(" ")
}

fn capitalize_first(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

fn match_leading_case(original: &str, replacement: &str) -> String {
    if original.starts_with(char::is_uppercase) {
        capitalize_first(replacement)
    } else {
        replacement.to_string()
    }
}

fn missing_apostrophe(lower: &str) -> Option<&'static str> {
    Some(match lower {
        "dont" => "don't",
        "doesnt" => "doesn't",
        "didnt" => "didn't",
        "isnt" => "isn't",
        "arent" => "aren't",
        "wasnt" => "wasn't",
        "werent" => "weren't",
        "havent" => "haven't",
        "hasnt" => "hasn't",
        "hadnt" => "hadn't",
        "cant" => "can't",
        "couldnt" => "couldn't",
        "wouldnt" => "wouldn't",
        "shouldnt" => "shouldn't",
        "mustnt" => "mustn't",
        "im" => "I'm",
        "ive" => "I've",
        "youre" => "you're",
        "youve" => "you've",
        "youll" => "you'll",
        "theyre" => "they're",
        "theyve" => "they've",
        "theyll" => "they'll",
        "weve" => "we've",
        "thats" => "that's",
        "whats" => "what's",
        "theres" => "there's",
        "heres" => "here's",
        "wheres" => "where's",
        "whos" => "who's",
        "hes" => "he's",
        "shes" => "she's",
        "alot" => "a lot",
        _ => return None,
    })
}

/// Picks "a" or "an" for the following word by sound, or `None` when unsure.
fn article_for(word: &str) -> Option<&'static str> {
    // Words that never follow an article; "a" here is usually the letter.
    const SKIP: &[&str] = &[
        "and",
        "or",
        "an",
        "is",
        "in",
        "of",
        "on",
        "at",
        "as",
        "if",
        "it",
        "its",
        "it's",
        "are",
        "am",
        "was",
        "were",
        "also",
        "up",
        "upon",
        "until",
        "unless",
        "into",
        "onto",
        "us",
        "any",
        "all",
        "each",
        "either",
        "every",
        "everyone",
        "everything",
        "other",
        "the",
        "to",
        "be",
        "that",
        "this",
        "these",
        "those",
        "they",
        "we",
        "my",
        "your",
        "his",
        "her",
        "their",
        "our",
        "so",
        "but",
        "for",
        "with",
        "by",
        "from",
        "not",
        "no",
        "can",
        "will",
        "would",
        "should",
        "could",
        "have",
        "has",
        "had",
        "do",
        "does",
        "did",
        "then",
        "than",
        "there",
        "when",
        "what",
        "which",
        "who",
        "how",
        "why",
        "where",
    ];
    const SILENT_H: &[&str] = &["hour", "honest", "honor", "honour", "heir"];
    const VOWEL_LETTER_CONSONANT_SOUND: &[&str] = &[
        "unicorn",
        "unicycle",
        "unicode",
        "unification",
        "unified",
        "uniform",
        "unify",
        "union",
        "unique",
        "unison",
        "unit",
        "univers",
        "unilateral",
        "unix",
        "use",
        "usa",
        "usu",
        "util",
        "uten",
        "utop",
        "uter",
        "utah",
        "uran",
        "urin",
        "ukul",
        "ukrain",
        "ugand",
        "ubiq",
        "eu",
        "ewe",
    ];

    let first = word.chars().next()?;
    if first.is_ascii_digit() {
        let digits: String = word.chars().take_while(char::is_ascii_digit).collect();
        let an = digits.starts_with('8')
            || (digits.len() % 3 == 2 && (digits.starts_with("11") || digits.starts_with("18")));
        return Some(if an { "an" } else { "a" });
    }
    if word.chars().count() < 2 || word.contains('.') || !first.is_alphabetic() {
        return None;
    }
    if !word.chars().any(char::is_lowercase) {
        return None;
    }
    let lower = word.to_lowercase();
    if SKIP.contains(&lower.as_str()) {
        return None;
    }
    if SILENT_H.iter().any(|prefix| lower.starts_with(prefix)) {
        return Some("an");
    }
    if lower == "one"
        || lower == "once"
        || lower.starts_with("one-")
        || VOWEL_LETTER_CONSONANT_SOUND
            .iter()
            .any(|prefix| lower.starts_with(prefix))
    {
        return Some("a");
    }
    Some(
        if matches!(first.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u') {
            "an"
        } else {
            "a"
        },
    )
}

fn at_sentence_start(tokens: &[WordToken<'_>], index: usize) -> bool {
    index == 0 || tokens[index - 1].suffix.contains(['.', '!', '?'])
}

fn corrected_article(tokens: &[WordToken<'_>], index: usize) -> Option<String> {
    let token = &tokens[index];
    if !token.prefix.is_empty() || !token.suffix.is_empty() {
        return None;
    }
    let capitalized = token.word.starts_with(char::is_uppercase);
    if capitalized {
        if !at_sentence_start(tokens, index) || token.word.chars().skip(1).any(char::is_uppercase) {
            return None;
        }
    }
    let next = tokens.get(index + 1)?;
    if !next
        .prefix
        .chars()
        .all(|ch| matches!(ch, '"' | '\'' | '(' | '\u{201c}' | '\u{2018}'))
    {
        return None;
    }
    let wanted = article_for(next.word)?;
    if token.word.eq_ignore_ascii_case(wanted) {
        return None;
    }
    Some(if capitalized {
        capitalize_first(wanted)
    } else {
        wanted.to_string()
    })
}

fn fix_common_word_errors(text: &str) -> String {
    const MODALS: &[&str] = &["could", "would", "should", "must", "might"];
    let words: Vec<_> = text.split_whitespace().collect();
    let tokens: Vec<_> = words.iter().map(|word| grammar_token(word)).collect();
    let mut output = Vec::with_capacity(words.len());
    for (index, token) in tokens.iter().enumerate() {
        let lower = token.word.to_lowercase().replace('\u{2019}', "'");
        // Capitalized mid-sentence words are likely names ("Jony Ive").
        let may_fix_spelling = !token.word.chars().any(char::is_uppercase)
            || (at_sentence_start(&tokens, index)
                && !token.word.chars().skip(1).any(char::is_uppercase));
        let replacement = if lower == "i" || (lower.starts_with("i'") && lower.len() <= 4) {
            Some(capitalize_first(token.word))
        } else if let Some(fixed) = missing_apostrophe(&lower).filter(|_| may_fix_spelling) {
            Some(match_leading_case(token.word, fixed))
        } else if lower == "of"
            && index > 0
            && tokens[index - 1].suffix.is_empty()
            && token.prefix.is_empty()
            && MODALS.contains(&tokens[index - 1].word.to_lowercase().as_str())
            && !tokens
                .get(index + 1)
                .is_some_and(|next| next.word.eq_ignore_ascii_case("course"))
        {
            Some(match_leading_case(token.word, "have"))
        } else if lower == "a" || lower == "an" {
            corrected_article(&tokens, index)
        } else {
            None
        };
        match replacement {
            Some(replacement) if replacement != token.word => {
                output.push(format!("{}{}{}", token.prefix, replacement, token.suffix));
            }
            _ => output.push(words[index].to_string()),
        }
    }
    output.join(" ")
}

fn digit_grouping_comma(characters: &[char], index: usize) -> bool {
    index > 0
        && characters[index - 1].is_ascii_digit()
        && characters.len() >= index + 4
        && characters[index + 1..index + 4]
            .iter()
            .all(char::is_ascii_digit)
        && !characters.get(index + 4).is_some_and(char::is_ascii_digit)
}

fn normalize_punctuation_spacing(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if matches!(character, ',' | '.' | '!' | '?' | ';' | ':') {
            let previous = index.checked_sub(1).map(|at| chars[at]);
            let next = chars.get(index + 1).copied();
            let between_digits = previous.is_some_and(|value| value.is_ascii_digit())
                && next.is_some_and(|value| value.is_ascii_digit());
            if (character == ',' && digit_grouping_comma(&chars, index))
                || (character == ':' && between_digits)
            {
                output.push(character);
                index += 1;
                continue;
            }
            while output.ends_with(' ') {
                output.pop();
            }
            if character == ',' && output.ends_with([',', ';', ':', '!', '?']) {
                index += 1;
                continue;
            }
            if matches!(character, '.' | '!' | '?' | ';') && output.ends_with(',') {
                output.pop();
            }
            output.push(character);
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

fn trim_dangling_separators(text: &str) -> String {
    text.trim_end_matches([',', ';', ' ']).to_string()
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

/// Single letters joined by periods: "a.m.", "e.g.", "U.S.".
fn is_initialism(token: &str) -> bool {
    let token = token.trim_end_matches([',', ';', ':', '!', '?', '"', '\'', ')']);
    let token = token.strip_suffix('.').unwrap_or(token);
    let mut letters = 0;
    for piece in token.split('.') {
        let mut characters = piece.chars();
        match (characters.next(), characters.next()) {
            (Some(letter), None) if letter.is_alphabetic() => letters += 1,
            _ => return false,
        }
    }
    letters >= 2
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
    is_initialism(&lower)
        || lower.contains('@')
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

    fn en(text: &str) -> String {
        format_transcription(text, Some("en"))
    }

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

    #[test]
    fn digit_by_digit_speech_becomes_one_number() {
        assert_eq!(
            en("the pin is one one zero zero zero one"),
            "The pin is 110001."
        );
        assert_eq!(
            en("the pin is eleven triple zero one"),
            "The pin is 110001."
        );
        assert_eq!(en("agent double oh seven"), "Agent 007");
        assert_eq!(en("call nine one one now"), "Call 911 now.");
        assert_eq!(en("error four oh four again"), "Error 404 again.");
        assert_eq!(
            en("dial nine eight seven six five four three two one zero"),
            "Dial 9876543210"
        );
    }

    #[test]
    fn scales_include_indian_and_western_systems() {
        assert_eq!(en("one hundred ten thousand and one"), "110001");
        assert_eq!(
            en("it costs one lakh ten thousand one rupees"),
            "It costs 110001 rupees."
        );
        assert_eq!(en("two crore fifty lakh"), "25000000");
        assert_eq!(
            en("they raised five million dollars"),
            "They raised 5 million dollars."
        );
        assert_eq!(en("about two lakhs people"), "About 2 lakhs people.");
        assert_eq!(en("twenty five hundred steps"), "2500 steps");
    }

    #[test]
    fn years_decimals_times_and_ranges() {
        assert_eq!(en("born in nineteen ninety nine"), "Born in 1999.");
        assert_eq!(en("back in twenty twenty five"), "Back in 2025.");
        assert_eq!(en("since twenty twenty one"), "Since 2021");
        assert_eq!(en("built in nineteen oh five"), "Built in 1905.");
        assert_eq!(en("pi is three point one four"), "Pi is 3.14.");
        assert_eq!(
            en("about one point five million users"),
            "About 1.5 million users."
        );
        assert_eq!(en("meet at three thirty pm"), "Meet at 3:30 pm.");
        assert_eq!(
            en("wake at seven oh five a.m. tomorrow"),
            "Wake at 7:05 a.m. tomorrow."
        );
        assert_eq!(en("wait two three days"), "Wait 2-3 days.");
        assert_eq!(en("in two thousand twenty five"), "In 2025");
    }

    #[test]
    fn one_stays_a_word_unless_it_is_numeric() {
        assert_eq!(
            en("no one knows which one of them"),
            "No one knows which one of them."
        );
        assert_eq!(en("this one is better"), "This one is better.");
        assert_eq!(en("go to page one"), "Go to page 1.");
        assert_eq!(en("it costs one dollar"), "It costs 1 dollar.");
        assert_eq!(en("pick one or two"), "Pick 1 or 2.");
        assert_eq!(en("from zero to one"), "From 0 to 1.");
        assert_eq!(en("one"), "1");
    }

    #[test]
    fn preserves_digit_groups_times_and_abbreviations_from_the_model() {
        assert_eq!(en("the total is 110,001"), "The total is 110,001.");
        assert_eq!(en("it costs 1,000 dollars"), "It costs 1,000 dollars.");
        assert_eq!(en("one, two, three"), "1, 2, 3.");
        assert_eq!(en("count 1,2,3 now"), "Count 1, 2, 3 now.");
        assert_eq!(en("meet at 3:30 today"), "Meet at 3:30 today.");
        assert_eq!(en("call at 5 p.m. tomorrow"), "Call at 5 p.m. tomorrow.");
        assert_eq!(en("use tabs, e.g. for code"), "Use tabs, e.g. for code.");
    }

    #[test]
    fn fixes_articles_by_sound() {
        assert_eq!(en("I ate a apple"), "I ate an apple.");
        assert_eq!(en("she is an doctor"), "She is a doctor.");
        assert_eq!(en("it takes a hour"), "It takes an hour.");
        assert_eq!(
            en("he is an university student"),
            "He is a university student."
        );
        assert_eq!(en("an European trip"), "A European trip.");
        assert_eq!(en("a honest answer"), "An honest answer.");
        assert_eq!(en("wait a 8 minutes"), "Wait an 8 minutes.");
        assert_eq!(en("buy a iPhone today"), "Buy an iPhone today.");
        assert_eq!(en("plan a or plan b"), "Plan a or plan b.");
        assert_eq!(en("is it a URL"), "Is it a URL.");
        assert_eq!(en("a one time fee"), "A one time fee.");
    }

    #[test]
    fn fixes_pronoun_contractions_and_modal_of() {
        assert_eq!(en("i think i'm ready"), "I think I'm ready.");
        assert_eq!(
            en("i dont know what im doing"),
            "I don't know what I'm doing."
        );
        assert_eq!(en("we could of won"), "We could have won.");
        assert_eq!(en("you should of course try"), "You should of course try.");
        assert_eq!(en("thats alot of work"), "That's a lot of work.");
        assert_eq!(en("designed by Jony Ive"), "Designed by Jony Ive.");
        assert_eq!(en("Will will join us"), "Will will join us.");
    }

    #[test]
    fn cleans_fillers_with_their_punctuation() {
        assert_eq!(en("I was, um, going home"), "I was going home.");
        assert_eq!(en("So, um, I think it works"), "So, I think it works.");
        assert_eq!(en("I think so, uh."), "I think so.");
        assert_eq!(en("Um, let's start now"), "Let's start now.");
        assert_eq!(en("the ER is busy"), "The ER is busy.");
        assert_eq!(en("the um the plan works"), "The plan works.");
    }

    #[test]
    fn collapses_repeated_phrases_and_dangling_punctuation() {
        assert_eq!(en("I think I think it works"), "I think it works.");
        assert_eq!(en("we need to, we need to ship"), "We need to ship.");
        assert_eq!(en("good job good job"), "Good job good job.");
        assert_eq!(
            en("she said that that was fine"),
            "She said that that was fine."
        );
        assert_eq!(en("send it today,"), "Send it today.");
        assert_eq!(en("wait , . okay"), "Wait. Okay");
    }
}
