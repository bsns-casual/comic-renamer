//! Volume / format / edition tokenizer. Vocabulary is §6.4, derived from
//! measured data — do not trim it.
//!
//! Input is a squashed residue: no separators survive, so every token has to
//! be recognised by prefix. A token is only consumed when the whole of it
//! parses; a half-match puts everything back and becomes leftover, which the
//! resolver reports as `review` rather than guessing (§6.4, D15).

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Volume(u32),
    Format(String),
    Edition(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Residue {
    pub tokens: Vec<Token>,
    /// Whatever the tokenizer could not claim — a subtitle, usually (D15).
    pub leftover: String,
}

/// §6.4. Longest first: `volume` must beat `vol`.
const VOLUME_WORDS: [&str; 3] = ["volume", "vol", "book"];

/// §6.4. Longest first within each overlapping pair: `tpb` before `tp`,
/// `ogn` before `gn`.
const FORMATS: [&str; 11] = [
    "omnibus", "archives", "oneshot", "deluxe", "trades", "tpb", "ogn", "tp", "hc", "sc", "gn",
];

/// Parsed and dropped from output (§6.4).
const EDITION_QUALIFIERS: [&str; 14] = [
    "first",
    "second",
    "third",
    "fourth",
    "fifth",
    "sixth",
    "seventh",
    "eighth",
    "ninth",
    "tenth",
    "revised",
    "collected",
    "anniversary",
    "definitive",
];

const WORD_NUMBERS: [&str; 20] = [
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
    "twenty",
];

pub fn eat(s: &str) -> Residue {
    let mut rest = s;
    let mut tokens = Vec::new();
    while let Some((token, used)) = eat_one(rest) {
        tokens.push(token);
        rest = &rest[used..];
    }
    Residue {
        tokens,
        leftover: rest.to_string(),
    }
}

/// One token off the front, or `None` when nothing matches.
fn eat_one(s: &str) -> Option<(Token, usize)> {
    for word in VOLUME_WORDS {
        if let Some(after) = s.strip_prefix(word) {
            // Atomic: no number means `vol` was part of a word, not a marker.
            if let Some((n, used)) = eat_number(after) {
                return Some((Token::Volume(n), word.len() + used));
            }
        }
    }
    for qualifier in EDITION_QUALIFIERS {
        let whole = s.len() >= qualifier.len() + 7
            && s[..qualifier.len()].eq_ignore_ascii_case(qualifier)
            && s[qualifier.len()..qualifier.len() + 7].eq_ignore_ascii_case("edition");
        if whole {
            return Some((
                Token::Edition(qualifier.to_string()),
                qualifier.len() + 7,
            ));
        }
    }
    for format in FORMATS {
        if s.starts_with(format) {
            return Some((Token::Format(format.to_string()), format.len()));
        }
    }
    None
}

/// Digits, then a spelled-out number, then a roman numeral.
fn eat_number(s: &str) -> Option<(u32, usize)> {
    let digits = s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits > 0 {
        // shortcut: a volume past u32 is a malformed filename, not a volume.
        return s[..digits].parse().ok().map(|n| (n, digits));
    }
    for (i, word) in WORD_NUMBERS.iter().enumerate() {
        if s.starts_with(word) {
            return Some((i as u32 + 1, word.len()));
        }
    }
    eat_roman(s)
}

/// A roman numeral counts only if it is *canonical* — the run converted back
/// to roman is the run itself. Without that guard `volcivilwar` scans as
/// C-I-V-I-L = 153 under the subtractive rule and swallows the subtitle.
fn eat_roman(s: &str) -> Option<(u32, usize)> {
    let run_len = s
        .find(|c: char| roman_value(c).is_none())
        .unwrap_or(s.len());
    if run_len == 0 {
        return None;
    }
    let run = &s[..run_len];
    let values: Vec<u32> = run.chars().filter_map(roman_value).collect();
    // Signed: the subtractive rule subtracts before it adds, so `iv` would
    // underflow an unsigned accumulator at `0 - 1`.
    let mut total: i64 = 0;
    for (i, &v) in values.iter().enumerate() {
        match values.get(i + 1) {
            Some(&next) if v < next => total -= i64::from(v),
            _ => total += i64::from(v),
        }
    }
    let total = u32::try_from(total).ok()?;
    (to_roman(total) == run).then_some((total, run_len))
}

fn roman_value(c: char) -> Option<u32> {
    match c.to_ascii_lowercase() {
        'i' => Some(1),
        'v' => Some(5),
        'x' => Some(10),
        'l' => Some(50),
        'c' => Some(100),
        'd' => Some(500),
        'm' => Some(1000),
        _ => None,
    }
}

fn to_roman(mut n: u32) -> String {
    const TABLE: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (value, numeral) in TABLE {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    out
}

/// Removes distributor cruft such as `_optimized_1420484117`.
pub fn strip_cruft(s: &str) -> String {
    let mut t = s;
    while let Some(i) = t.rfind(['_', '-']) {
        let segment = &t[i + 1..];
        // shortcut: a trailing all-digit run is only cruft at 6+ digits, so a
        // real `_vol_1` survives. Raise the floor if a distributor ships
        // shorter ids.
        let is_id = segment.len() >= 6 && segment.chars().all(|c| c.is_ascii_digit());
        if is_id || segment.eq_ignore_ascii_case("optimized") {
            t = &t[..i];
        } else {
            break;
        }
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eats_numeric_word_and_roman_volumes() {
        for (s, n) in [("volume1", 1), ("vol1", 1), ("bookone", 1), ("voliv", 4)] {
            assert_eq!(eat(s).tokens, vec![Token::Volume(n)]);
        }
    }

    #[test]
    fn leaves_subtitle_as_leftover() {
        let r = eat("volume1seedofdestruction");
        assert_eq!(
            (r.tokens, r.leftover.as_str()),
            (vec![Token::Volume(1)], "seedofdestruction")
        );
    }

    #[test]
    fn strips_distributor_cruft() {
        assert_eq!(
            strip_cruft("thewickedandthedivine_vol1_optimized_1420484117"),
            "thewickedandthedivine_vol1"
        );
    }

    /// A non-canonical roman run must not be read as a volume: `civil` scans
    /// as C-I-V-I-L = 153 under the subtractive rule and would eat a subtitle.
    #[test]
    fn a_word_that_merely_looks_roman_is_not_a_volume() {
        let r = eat("volcivilwar");
        assert_eq!((r.tokens.as_slice(), r.leftover.as_str()), (&[][..], "volcivilwar"));
    }

    #[test]
    fn keeps_a_short_trailing_number_that_is_a_volume() {
        assert_eq!(strip_cruft("someseries_vol_1"), "someseries_vol_1");
    }
}
