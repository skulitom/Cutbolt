//! Numbers as they are spoken, so a numeral and its words compare equal: "80" and "eighty",
//! "170" and "a hundred and seventy", "10-second" and "ten second". `spoken` reads a written token
//! exactly as the speech worker (`tools/transcribe_worker.py`) does before aligning it; both are
//! checked against `tests/support/spoken_numbers.json`. `key` turns words into what review matching
//! compares: number phrases become their values and the other letters run together.

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];
const SCALES: [(u64, &str); 4] = [
    (1_000_000_000_000, "trillion"),
    (1_000_000_000, "billion"),
    (1_000_000, "million"),
    (1000, "thousand"),
];
const APOSTROPHES: [char; 2] = ['\'', '’'];

fn ordinal(word: &str) -> String {
    match word {
        "one" => "first".into(),
        "two" => "second".into(),
        "three" => "third".into(),
        "five" => "fifth".into(),
        "eight" => "eighth".into(),
        "nine" => "ninth".into(),
        "twelve" => "twelfth".into(),
        _ => match word.strip_suffix('y') {
            Some(stem) => format!("{stem}ieth"),
            None => format!("{word}th"),
        },
    }
}

fn plural(word: &str) -> String {
    match word.strip_suffix('y') {
        Some(stem) => format!("{stem}ies"),
        None if word.ends_with('x') => format!("{word}es"),
        None => format!("{word}s"),
    }
}

/// Singular and plural of a currency's unit and of its hundredth.
fn currency(symbol: char) -> Option<[&'static str; 4]> {
    match symbol {
        '$' => Some(["dollar", "dollars", "cent", "cents"]),
        '£' => Some(["pound", "pounds", "penny", "pence"]),
        '€' => Some(["euro", "euros", "cent", "cents"]),
        _ => None,
    }
}

/// English words of `n` below 10^15, without "and": 170 is one hundred seventy.
fn cardinal(n: u64) -> Vec<String> {
    let mut words = Vec::new();
    if n < 20 {
        words.push(ONES[n as usize].to_owned());
    } else if n < 100 {
        words.push(TENS[(n / 10) as usize].to_owned());
        if !n.is_multiple_of(10) {
            words.push(ONES[(n % 10) as usize].to_owned());
        }
    } else if n < 1000 {
        words.extend([ONES[(n / 100) as usize].to_owned(), "hundred".to_owned()]);
        if !n.is_multiple_of(100) {
            words.extend(cardinal(n % 100));
        }
    } else {
        let (value, name) = SCALES
            .into_iter()
            .find(|&(value, _)| n >= value)
            .expect("n is at least a thousand");
        words.extend(cardinal(n / value));
        words.push(name.to_owned());
        if !n.is_multiple_of(value) {
            words.extend(cardinal(n % value));
        }
    }
    words
}

/// Four digits read in pairs: nineteen ninety, nineteen oh five, nineteen hundred, twenty twenty six.
fn year(n: u64) -> Vec<String> {
    let (high, low) = (n / 100, n % 100);
    let mut words = cardinal(high);
    match low {
        0 => words.push("hundred".into()),
        1..=9 => words.extend(["oh".to_owned(), ONES[low as usize].to_owned()]),
        _ => words.extend(cardinal(low)),
    }
    words
}

fn digit_words(digits: &str) -> impl Iterator<Item = String> + '_ {
    digits
        .bytes()
        .map(|d| ONES[usize::from(d - b'0')].to_owned())
}

/// A number written from a digit: its words without any decimal fraction, its whole value, the
/// fraction's digits and the index after it.
struct Numeral {
    words: Vec<String>,
    value: u64,
    fraction: String,
    end: usize,
}

#[derive(PartialEq)]
enum Suffix {
    Ordinal,
    Plural,
}

fn numeral(text: &[char], i: usize) -> Numeral {
    let digit = |k: usize| text.get(k).is_some_and(char::is_ascii_digit);
    let is = |k: usize, c: char| text.get(k) == Some(&c);
    let alphabetic = |k: usize| text.get(k).is_some_and(|c| c.is_alphabetic());
    let mut j = i;
    while digit(j) {
        j += 1;
    }
    let mut digits: String = text[i..j].iter().collect();
    let mut grouped = false;
    let group = |k: usize| (k..k + 3).all(digit) && !digit(k + 3);
    while (digits.len() <= 3 || grouped) && is(j, ',') && group(j + 1) {
        digits.extend(&text[j + 1..j + 4]);
        j += 4;
        grouped = true;
    }
    let mut fraction = String::new();
    let mut minutes = None;
    let mut suffix = None;
    let lower = |k: usize| text.get(k).map(char::to_ascii_lowercase);
    if is(j, '.') && digit(j + 1) {
        let mut k = j + 1;
        while digit(k) {
            k += 1;
        }
        fraction = text[j + 1..k].iter().collect();
        j = k;
    } else if !grouped
        && digits.len() <= 2
        && is(j, ':')
        && digit(j + 1)
        && digit(j + 2)
        && !digit(j + 3)
    {
        let tens = text[j + 1].to_digit(10).expect("digit");
        minutes = Some(u64::from(
            tens * 10 + text[j + 2].to_digit(10).expect("digit"),
        ));
        j += 3;
    } else if [('s', 't'), ('n', 'd'), ('r', 'd'), ('t', 'h')]
        .into_iter()
        .any(|(a, b)| lower(j) == Some(a) && lower(j + 1) == Some(b))
        && !alphabetic(j + 2)
    {
        suffix = Some(Suffix::Ordinal);
        j += 2;
    } else if is(j, 's') && !alphabetic(j + 1) {
        suffix = Some(Suffix::Plural);
        j += 1;
    } else if text.get(j).is_some_and(|c| APOSTROPHES.contains(c))
        && is(j + 1, 's')
        && !alphabetic(j + 2)
    {
        suffix = Some(Suffix::Plural);
        j += 2;
    }
    let significant = digits.trim_start_matches('0');
    let value = match significant.len() {
        0 => 0,
        1..=15 => significant.parse().expect("at most 15 digits"),
        _ => u64::MAX,
    };
    let mut words: Vec<String> =
        if (digits.len() > 1 && digits.starts_with('0')) || digits.len() > 15 {
            digit_words(&digits).collect()
        } else if !grouped
            && suffix != Some(Suffix::Ordinal)
            && digits.len() == 4
            && matches!(value, 1100..=1999 | 2010..=2099)
        {
            year(value)
        } else {
            cardinal(value)
        };
    match minutes {
        Some(0) => words.push("o'clock".into()),
        Some(m @ 1..=9) => words.extend(["oh".to_owned(), ONES[m as usize].to_owned()]),
        Some(m) => words.extend(cardinal(m)),
        None => {}
    }
    if let Some(suffix) = suffix {
        let last = words.pop().expect("a number has words");
        words.push(match suffix {
            Suffix::Ordinal => ordinal(&last),
            Suffix::Plural => plural(&last),
        });
    }
    Numeral {
        words,
        value,
        fraction,
        end: j,
    }
}

/// The words of a run of text between numbers: its letters, split at anything else, with
/// apostrophes kept only inside a word.
fn letters_of(run: &[char], words: &mut Vec<String>) {
    let run: String = run.iter().collect();
    for part in run.split(|c: char| !(c.is_alphabetic() || APOSTROPHES.contains(&c))) {
        let part = part.trim_matches(&APOSTROPHES[..]);
        if !part.is_empty() {
            words.push(part.to_owned());
        }
    }
}

/// The words a written token is read as. A token without digits is read as written. Numbers are
/// read in English: 80 eighty, 170 one hundred seventy, 1,500 one thousand five hundred, 3.5 three
/// point five, 21st twenty first, 1990s nineteen nineties, 9:05 nine oh five, $5 five dollars,
/// $3.50 three dollars fifty cents, 50% fifty percent, -4 minus four. Four digits from 1100 to
/// 1999 and 2010 to 2099 are read as a year (2026 twenty twenty six); a leading zero or more than
/// 15 digits are read digit by digit. The letters around a number are read as their own words, so
/// 10-second is ten second.
pub(crate) fn spoken(text: &str) -> Vec<String> {
    if !text.chars().any(|c| c.is_ascii_digit()) {
        return vec![text.to_owned()];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut words = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let mut k = i;
        let unit = k.checked_sub(1).and_then(|p| currency(chars[p]));
        if unit.is_some() {
            k -= 1;
        }
        let sign = k > 0
            && ['+', '-', '−'].contains(&chars[k - 1])
            && (k == 1 || !chars[k - 2].is_alphanumeric());
        letters_of(&chars[start..k - usize::from(sign)], &mut words);
        if sign {
            words.push(if chars[k - 1] == '+' { "plus" } else { "minus" }.into());
        }
        let Numeral {
            words: mut read,
            value,
            fraction,
            end,
        } = numeral(&chars, i);
        i = end;
        match unit {
            Some(unit) if fraction.len() == 2 => {
                let cents: u64 = fraction.parse().expect("two digits");
                if value > 0 || cents == 0 {
                    read.push(unit[usize::from(value != 1)].into());
                } else {
                    read.clear();
                }
                if cents > 0 {
                    read.extend(cardinal(cents));
                    read.push(unit[2 + usize::from(cents != 1)].into());
                }
            }
            _ => {
                if !fraction.is_empty() {
                    read.push("point".into());
                    read.extend(digit_words(&fraction));
                }
                if let Some(unit) = unit {
                    read.push(unit[usize::from(value != 1 || !fraction.is_empty())].into());
                }
            }
        }
        words.extend(read);
        if chars.get(i) == Some(&'%') {
            words.push("percent".into());
            i += 1;
        }
        start = i;
    }
    letters_of(&chars[start..], &mut words);
    words
}

/// What review matching compares a group of words by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Atom {
    /// Letters and digits of consecutive words other than numbers, lowercase and run together.
    Word(String),
    /// A number phrase, written or spoken: "170", "one hundred seventy", "a hundred and seventy".
    Number(u64),
}

/// The lowercase letter-and-digit runs of a word as spoken, numerals read out.
pub(crate) fn runs(text: &str) -> Vec<String> {
    spoken(text)
        .iter()
        .flat_map(|word| {
            word.split(|c: char| !c.is_alphanumeric())
                .filter(|run| !run.is_empty())
                .map(|run| run.chars().flat_map(char::to_lowercase).collect())
                .collect::<Vec<String>>()
        })
        .collect()
}

fn small(word: &str) -> Option<u64> {
    if let Some(n) = ONES.iter().position(|w| *w == word) {
        return Some(n as u64);
    }
    TENS.iter()
        .position(|w| !w.is_empty() && *w == word)
        .map(|n| n as u64 * 10)
}

fn scale(word: &str) -> Option<u64> {
    SCALES
        .into_iter()
        .find(|&(_, name)| name == word)
        .map(|(value, _)| value)
}

/// Whether a word written as `text` with these runs can take part in a number phrase.
pub(crate) fn numeric(text: &str, runs: &[String]) -> bool {
    text.chars().any(|c| c.is_ascii_digit())
        || runs
            .iter()
            .any(|r| small(r).is_some() || r == "hundred" || scale(r).is_some())
}

#[derive(Clone, Copy, PartialEq)]
enum Last {
    Start,
    Small(u64),
    Hundred,
    Scale,
    And,
}

/// A cardinal number phrase at `words[i..]`: its value and length. "a" counts as one before
/// "hundred" or a scale, and "and" may follow "hundred" or a scale before a smaller number.
fn cardinal_at(words: &[String], i: usize) -> Option<(u64, usize)> {
    let (mut total, mut current, mut largest, mut last) = (0u64, 0u64, u64::MAX, Last::Start);
    let mut j = i;
    while let Some(word) = words.get(j).map(String::as_str) {
        let next = words.get(j + 1).map(String::as_str);
        if word == "zero" {
            if last == Last::Start {
                return Some((0, 1));
            }
            break;
        }
        if let Some(value) = small(word) {
            match last {
                Last::Start | Last::Hundred | Last::Scale | Last::And => current += value,
                Last::Small(previous) if previous >= 20 && previous % 10 == 0 && value < 10 => {
                    current += value
                }
                _ => break,
            }
            last = Last::Small(value);
        } else if word == "a"
            && last == Last::Start
            && next.is_some_and(|n| n == "hundred" || scale(n).is_some())
        {
            current = 1;
            last = Last::Small(1);
        } else if word == "hundred" && matches!(last, Last::Small(_)) && (1..=99).contains(&current)
        {
            current *= 100;
            last = Last::Hundred;
        } else if let Some(value) = scale(word)
            && matches!(last, Last::Small(_) | Last::Hundred)
            && (1..=999).contains(&current)
            && value < largest
        {
            total += current * value;
            current = 0;
            largest = value;
            last = Last::Scale;
        } else if word == "and"
            && matches!(last, Last::Hundred | Last::Scale)
            && next.and_then(small).is_some_and(|n| n > 0)
        {
            last = Last::And;
        } else {
            break;
        }
        j += 1;
    }
    (j > i).then_some((total + current, j - i))
}

/// A number phrase at `words[i..]`, including a year read in pairs ("nineteen ninety", "twenty
/// twenty six", "nineteen oh five"): its value and length.
fn number_at(words: &[String], i: usize) -> Option<(u64, usize)> {
    let (value, used) = cardinal_at(words, i)?;
    let pair = |value: u64, at: usize, count: usize| {
        (10..=99).contains(&value) && words[at..at + count].iter().all(|w| small(w).is_some())
    };
    if pair(value, i, used) {
        let j = i + used;
        if words.get(j).is_some_and(|w| w == "oh")
            && let Some(unit) = words
                .get(j + 1)
                .and_then(|w| small(w))
                .filter(|u| (1..=9).contains(u))
        {
            return Some((value * 100 + unit, used + 2));
        }
        if let Some((low, count)) = cardinal_at(words, j)
            && pair(low, j, count)
        {
            return Some((value * 100 + low, used + count));
        }
    }
    Some((value, used))
}

/// What a group of words is compared by: each number phrase becomes its value, and the letters
/// between numbers run together. An "and" before a number is left out, as in "three dollars and
/// fifty cents".
pub(crate) fn key<'a>(runs: impl IntoIterator<Item = &'a String>) -> Vec<Atom> {
    let words: Vec<String> = runs.into_iter().cloned().collect();
    let mut atoms = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if let Some((value, used)) = number_at(&words, i) {
            atoms.push(Atom::Number(value));
            i += used;
            continue;
        }
        let word = &words[i];
        i += 1;
        if word == "and" && number_at(&words, i).is_some() {
            continue;
        }
        match atoms.last_mut() {
            Some(Atom::Word(last)) => last.push_str(word),
            _ => atoms.push(Atom::Word(word.clone())),
        }
    }
    atoms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numerals_are_read_as_the_worker_reads_them() {
        let table: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(include_str!("../tests/support/spoken_numbers.json")).unwrap();
        assert!(table.len() > 50);
        for (text, words) in table {
            let words: Vec<String> = serde_json::from_value(words).unwrap();
            assert_eq!(spoken(&text), words, "{text}");
        }
    }

    fn key_of(text: &str) -> Vec<Atom> {
        let runs: Vec<String> = text.split(' ').flat_map(runs).collect();
        key(&runs)
    }

    #[test]
    fn a_numeral_and_its_words_have_one_key() {
        use Atom::{Number, Word};
        for (written, said) in [
            ("80", "eighty"),
            ("170", "a hundred and seventy"),
            ("170", "one hundred seventy"),
            ("170", "one hundred and seventy"),
            ("10-second", "ten second"),
            ("80-second", "eighty-second"),
            ("2 hours", "two hours"),
            ("1990", "nineteen ninety"),
            ("1990", "one thousand nine hundred and ninety"),
            ("1990s", "nineteen nineties"),
            ("2026", "two thousand and twenty-six"),
            ("2026", "twenty twenty-six"),
            ("1905", "nineteen oh five"),
            ("1,500", "fifteen hundred"),
            ("21st", "twenty-first"),
            ("3.5", "three point five"),
            ("$3.50", "three dollars and fifty cents"),
            ("50%", "fifty percent"),
            ("9:05", "nine oh five"),
            ("1,000,000", "a million"),
        ] {
            assert_eq!(key_of(written), key_of(said), "{written} / {said}");
        }
        assert_eq!(
            key_of("an eighty second video"),
            [Word("an".into()), Number(80), Word("secondvideo".into())]
        );
        assert_eq!(key_of("a hundred and seventy"), [Number(170)]);
        assert_eq!(key_of("one two three"), [Number(1), Number(2), Number(3)]);
        assert_eq!(key_of("twenty ten"), [Number(2010)]);
        assert_eq!(key_of("a b"), [Word("ab".into())]);
        assert_eq!(key_of("rock and roll"), [Word("rockandroll".into())]);
        assert_eq!(key_of("a hundred and"), [Number(100), Word("and".into())]);
        for (one, other) in [
            ("80", "eighteen"),
            ("170", "a hundred seven"),
            ("123", "one two three"),
            ("80", "80s"),
            ("two hours", "too hours"),
        ] {
            assert_ne!(key_of(one), key_of(other), "{one} / {other}");
        }
    }
}
