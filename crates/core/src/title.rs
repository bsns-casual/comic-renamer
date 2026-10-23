//! Title parsing: a full title sentence typed by the user, split into the
//! fields the mapping stores.
//!
//! Everything is verbatim. Case is never folded (owner decision, 2026-10-22):
//! `DIE` stays `DIE`, and `Hack/Slash` keeps its slash -- the preset sanitises
//! that into a path later, never the parser.

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedTitle {
    pub series: String,
    pub volume: Option<u32>,
    pub subtitle: Option<String>,
}

pub fn parse_title(text: &str) -> ParsedTitle {
    let (series, volume, subtitle) = split_at_boundary(text);
    ParsedTitle {
        series: series.to_string(),
        volume,
        subtitle: (!subtitle.is_empty()).then(|| subtitle.to_string()),
    }
}

/// Splits at the first boundary: the series ends at whichever comes first of
/// a colon and a volume marker (`Vol.`/`Volume` followed by an Arabic or Roman
/// number). Returns (series, volume, subtitle), all slices of `text`. Subtitle
/// is empty when there is none.
///
/// A trailing `#7` is kept: with no volume it is the only thing telling two
/// issues of one series apart, so it lands in the subtitle (`Red Sonja #7` is
/// `Red Sonja` / `#7`). Once a volume exists the volume tells them apart, and
/// the collection's issue range (`(#1-6)`) is dropped as before.
///
/// The colon is found first so a number and the subtitle's first word cannot
/// share a token: Humble writes `Vol. 3:Planet Death`.
pub fn split_at_boundary(text: &str) -> (&str, Option<u32>, &str) {
    let head_end = text.find(':').unwrap_or(text.len());
    let tokens: Vec<(usize, &str)> = text[..head_end]
        .split_whitespace()
        .map(|t| (t.as_ptr() as usize - text.as_ptr() as usize, t))
        .collect();

    for pair in tokens.windows(2) {
        let [(marker_at, marker), (number_at, number)] = [pair[0], pair[1]];
        if !is_volume_word(marker) {
            continue;
        }
        if let Some(volume) = number_value(number) {
            let series = &text[..marker_at];
            let rest = &text[number_at + number.len()..];
            return (finish_series(series), Some(volume), finish_subtitle(rest));
        }
    }
    let series = text[..head_end].trim_end_matches(|c: char| c.is_whitespace() || c == ',');
    let subtitle = text[head_end..].trim_start_matches(|c: char| c.is_whitespace() || c == ':' || c == ',');
    // No colon: a trailing `#7` is the subtitle, not part of the series name.
    if head_end == text.len() {
        if let Some(last) = series.rsplit(char::is_whitespace).next() {
            if last.contains('#') && last.len() < series.len() {
                let cut = series.len() - last.len();
                return (series[..cut].trim_end(), None, &series[cut..]);
            }
        }
    }
    // A colon follows: `X #26: Y` is issue 26 of X, so the number must not
    // become part of the series name and give every issue its own folder.
    (drop_trailing_issue(series), None, subtitle)
}

/// `Harbinger, ` before `Vol.` leaves a separator behind.
fn finish_series(s: &str) -> &str {
    drop_trailing_issue(s.trim_end_matches(|c: char| c.is_whitespace() || c == ','))
}

/// What follows the boundary starts with the colon or comma that introduced it.
fn finish_subtitle(s: &str) -> &str {
    drop_trailing_issue(s.trim_start_matches(|c: char| c.is_whitespace() || c == ':' || c == ','))
}

/// `#7`, and `(#1-6)`: Humble's issue numbers and ranges. Only at the end, so
/// a `#` inside a name survives.
fn drop_trailing_issue(mut s: &str) -> &str {
    loop {
        s = s.trim_end();
        match s.rsplit(char::is_whitespace).next() {
            Some(last) if last.contains('#') => s = &s[..s.len() - last.len()],
            _ => return s,
        }
    }
}

fn alphanumeric(token: &str) -> String {
    token.chars().filter(|c| c.is_alphanumeric()).collect()
}

pub fn is_volume_word(token: &str) -> bool {
    matches!(
        alphanumeric(token).to_ascii_lowercase().as_str(),
        "vol" | "vols" | "volume" | "volumes"
    )
}

/// Arabic, or uppercase Roman as a whole token: `IVory` is not `IV`, which
/// would invent a volume. Only wrapping punctuation is stripped, so a range
/// (`8-10`, `I-V`) or a decimal (`2.5`) is `None`, not a joined number: it has
/// no single volume. `None` too when it does not fit a `u32`.
pub fn number_value(token: &str) -> Option<u32> {
    let t = token
        .trim_start_matches(['(', '#'])
        .trim_end_matches([',', '.', ')']);
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
        return t.parse().ok();
    }
    if t.is_empty() || !t.chars().all(|c| "IVXLCDM".contains(c)) {
        return None;
    }
    let value = |c| match c {
        'I' => 1,
        'V' => 5,
        'X' => 10,
        'L' => 50,
        'C' => 100,
        'D' => 500,
        _ => 1000,
    };
    let digits: Vec<u32> = t.chars().map(value).collect();
    Some(
        digits
            .iter()
            .enumerate()
            .map(|(i, &d)| match digits.get(i + 1) {
                Some(&next) if next > d => -(d as i64),
                _ => d as i64,
            })
            .sum::<i64>() as u32,
    )
}
