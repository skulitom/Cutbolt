//! British and American spellings of one word, for comparing what a cut says with what it was
//! meant to say: recognition writes one spelling ("color", "neighbor") whatever the script used.
//!
//! Two words, as lowercase letters and digits, are variants when they differ in exactly one place
//! and in one of these ways, British form first:
//!
//! | Difference | Around it | Examples |
//! | --- | --- | --- |
//! | `our` / `or` | two letters before the `o` | colour, neighbourhood, odour |
//! | `s` / `z` after `i` or `y`, before `a`, `e` or `i` | three letters before the `i` or `y` | realise, organisation, analysing |
//! | `re` / `er` | three letters before; then the end or `s` | centre, metres |
//! | `ogue` / `og` | three letters before the `og`; then the end or `s` | catalogue, dialogues |
//! | `ence` / `ense` | three letters before the `en`; then the end or `s` | defence, licences |
//! | `ll` / `l` | four letters before; then `ed`, `ing(s)`, `er(s)` or `or(s)` | travelled, modelling, counsellor |
//! | `ae` or `oe` / `e` | three letters after the `e` | paediatric, foetus, mediaeval |
//! | `e` / no `e` after `ag` or `dg` | then `ing` or `ment(s)` | ageing, judgement |
//!
//! The letters around each difference keep distinct words apart: "four" and "for", "filled" and
//! "filed", "prise" and "prize", "acre" and "acer", "shoes" and "she's". A few whole words no rule
//! covers are listed in [`PAIRS`].

/// Variants no rule covers, British first; each may also take a final `s`.
const PAIRS: &[(&str, &str)] = &[
    ("aluminium", "aluminum"),
    ("artefact", "artifact"),
    ("cheque", "check"),
    ("cosy", "cozy"),
    ("distil", "distill"),
    ("draught", "draft"),
    ("enrol", "enroll"),
    ("fulfil", "fulfill"),
    ("grey", "gray"),
    ("instalment", "installment"),
    ("jewellery", "jewelry"),
    ("kerb", "curb"),
    ("manoeuvre", "maneuver"),
    ("mould", "mold"),
    ("moustache", "mustache"),
    ("plough", "plow"),
    ("practise", "practice"),
    ("programme", "program"),
    ("pyjamas", "pajamas"),
    ("sceptic", "skeptic"),
    ("sceptical", "skeptical"),
    ("skilful", "skillful"),
    ("smoulder", "smolder"),
    ("storey", "story"),
    ("tyre", "tire"),
    ("whisky", "whiskey"),
];

/// True when `a` and `b`, normalized to lowercase letters and digits, are two spellings of one
/// word. Equal words are not variants.
pub(crate) fn variants(a: &str, b: &str) -> bool {
    if a == b {
        return false;
    }
    if listed(a, b) {
        return true;
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let room = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(room)
        .take_while(|(x, y)| x == y)
        .count();
    let text = |chars: &[char]| chars.iter().collect::<String>();
    let before = text(&a[..prefix]);
    let after = text(&a[a.len() - suffix..]);
    let (x, y) = (
        text(&a[prefix..a.len() - suffix]),
        text(&b[prefix..b.len() - suffix]),
    );
    rule(&before, &x, &y, &after) || rule(&before, &y, &x, &after)
}

fn listed(a: &str, b: &str) -> bool {
    let pair = |a: &str, b: &str| {
        PAIRS
            .iter()
            .any(|&(x, y)| (a, b) == (x, y) || (a, b) == (y, x))
    };
    pair(a, b)
        || matches!((a.strip_suffix('s'), b.strip_suffix('s')), (Some(a), Some(b)) if pair(a, b))
}

/// Whether `british` in place of `american`, between `before` and `after`, is one of the rules.
fn rule(before: &str, british: &str, american: &str, after: &str) -> bool {
    let letters = |s: &str| s.chars().count();
    match (british, american) {
        ("u", "") => before.ends_with('o') && letters(before) >= 3 && after.starts_with('r'),
        ("s", "z") => {
            before.ends_with(['i', 'y'])
                && letters(before) >= 4
                && after.starts_with(['a', 'e', 'i'])
        }
        ("re", "er") => letters(before) >= 3 && matches!(after, "" | "s"),
        ("ue", "") => before.ends_with("og") && letters(before) >= 5 && matches!(after, "" | "s"),
        ("c", "s") => before.ends_with("en") && letters(before) >= 5 && matches!(after, "e" | "es"),
        ("l", "") => {
            before.ends_with('l')
                && letters(before) >= 5
                && matches!(after, "ed" | "ing" | "ings" | "er" | "ers" | "or" | "ors")
        }
        ("a" | "o", "") => after.starts_with('e') && letters(after) >= 4,
        ("e", "") => {
            (before.ends_with("ag") || before.ends_with("dg"))
                && matches!(after, "ing" | "ment" | "ments")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::variants;

    #[test]
    fn british_and_american_spellings_are_variants() {
        for (a, b) in [
            ("neighbour", "neighbor"),
            ("colour", "color"),
            ("colourful", "colorful"),
            ("neighbourhoods", "neighborhoods"),
            ("odour", "odor"),
            ("realise", "realize"),
            ("organisation", "organization"),
            ("analysing", "analyzing"),
            ("centre", "center"),
            ("metres", "meters"),
            ("catalogue", "catalog"),
            ("dialogues", "dialogs"),
            ("defence", "defense"),
            ("licences", "licenses"),
            ("travelled", "traveled"),
            ("modelling", "modeling"),
            ("counsellor", "counselor"),
            ("paediatric", "pediatric"),
            ("anaemia", "anemia"),
            ("foetus", "fetus"),
            ("mediaeval", "medieval"),
            ("ageing", "aging"),
            ("judgement", "judgment"),
            ("grey", "gray"),
            ("programmes", "programs"),
            ("manoeuvres", "maneuvers"),
        ] {
            assert!(variants(a, b), "{a} / {b}");
            assert!(variants(b, a), "{b} / {a}");
        }
    }

    #[test]
    fn distinct_words_stay_distinct() {
        for (a, b) in [
            ("four", "for"),
            ("hour", "hor"),
            ("your", "yor"),
            ("filled", "filed"),
            ("smaller", "smaler"),
            ("prise", "prize"),
            ("acre", "acer"),
            ("rogue", "rog"),
            ("fence", "fense"),
            ("shoes", "shes"),
            ("goes", "ges"),
            ("singeing", "singing"),
            ("green", "grain"),
            ("expressions", "expression"),
            ("colour", "colour"),
            ("colours", "color"),
            ("κύκλος", "κυκλος"),
        ] {
            assert!(!variants(a, b), "{a} / {b}");
            assert!(!variants(b, a), "{b} / {a}");
        }
    }
}
