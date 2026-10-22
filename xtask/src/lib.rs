//! Grading logic lives in the lib because a bin-only crate cannot be imported
//! by its own tests.

/// Normalises both sides before comparing — Humble writes `&` where the corpus
/// writes "and", prefixes promos with "Free Issue" including a zero-width
/// character, and inserts `Vol 1:` mid-title. A naive comparison overstates the
/// error rate five-fold.
///
/// It must **not** drop leading articles or fold case: the corpus's own
/// `name_key` does, which would silently pass `MASK Omnibus` against *The Mask
/// Omnibus*. That is the gate's whole job.
pub fn grade_ok(answer: &str, truth: &str) -> bool {
    let (given, given_tail) = split_title(answer);
    let (series, subtitle) = split_title(truth);
    let given = join(&given, &given_tail);
    // Either rendering is a correct answer: the whole title, or the series
    // alone when the rest was the volume's own subtitle. Both are exact
    // comparisons -- a prefix match would accept `Harbinger` for
    // *Harbinger Wars*.
    given == join(&series, &subtitle) || given == join(&series, &[])
}

fn join(series: &[String], subtitle: &[String]) -> String {
    let mut all = series.to_vec();
    all.extend_from_slice(subtitle);
    all.join(" ")
}

/// Splits a title at its volume marker into (series, subtitle), normalising
/// both: zero-width characters removed, `&` spelled "and", issue numbers and
/// the volume marker itself dropped, remaining punctuation dropped, whitespace
/// collapsed. Case and leading articles survive on purpose.
fn split_title(s: &str) -> (Vec<String>, Vec<String>) {
    let expanded = s
        .replace(['\u{200b}', '\u{200c}', '\u{200d}', '\u{feff}'], "")
        .replace('&', " and ");

    let mut series: Vec<String> = Vec::new();
    let mut subtitle: Vec<String> = Vec::new();
    let mut past_volume = false;
    let mut raw = expanded.split_whitespace().peekable();
    while let Some(token) = raw.next() {
        // `#7` has to go before punctuation is stripped, or it survives as a
        // bare `7` indistinguishable from a number inside a title.
        if token.starts_with('#') {
            continue;
        }
        if !past_volume && is_volume_word(token) {
            // Humble writes `Vol. 3:Planet Death` with no space, so the number
            // and the subtitle's first word can share a token.
            let (number, rest) = leading_number(&alphanumeric(raw.peek().unwrap_or(&"")));
            if !number.is_empty() {
                raw.next();
                past_volume = true;
                if !rest.is_empty() {
                    subtitle.push(rest);
                }
            }
            continue;
        }
        let clean = alphanumeric(token);
        if clean.is_empty() {
            continue;
        }
        if past_volume {
            subtitle.push(clean);
        } else {
            series.push(clean);
        }
    }

    // Humble's promo prefix. Only when "Free" is followed by "Issue", so a
    // title that genuinely opens with "Free" keeps its first word.
    if series.len() > 2
        && series[0].eq_ignore_ascii_case("free")
        && series[1].eq_ignore_ascii_case("issue")
    {
        series.drain(..2);
    }
    (series, subtitle)
}

fn alphanumeric(token: &str) -> String {
    token.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn is_volume_word(token: &str) -> bool {
    matches!(
        alphanumeric(token).to_ascii_lowercase().as_str(),
        "vol" | "vols" | "volume" | "volumes"
    )
}

/// Splits off a leading Arabic or Roman number. `Vol. IV` and `Vol 1` are both
/// Humble forms, and `3Planet` is what `3:Planet` normalises to.
fn leading_number(token: &str) -> (String, String) {
    let digits = token.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        return (token[..digits].to_string(), token[digits..].to_string());
    }
    // Roman only when the whole token is Roman: splitting `IVory` at `IV`
    // would invent a volume out of a word.
    if !token.is_empty() && token.chars().all(|c| "IVXLCDM".contains(c)) {
        return (token.to_string(), String::new());
    }
    (String::new(), token.to_string())
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rates {
    pub located: f64,
    pub ready: f64,
    pub silent_false_positive: f64,
}

/// located >= 92.3%, ready >= 66%, silent FP <= 0.48% (D26, D31).
pub fn gates_pass(rates: Rates) -> bool {
    rates.located >= 92.3 && rates.ready >= 66.0 && rates.silent_false_positive <= 0.48
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_four_grading_artifacts() {
        assert!(grade_ok("Love and Rockets", "Love & Rockets Vol. IV"));
        assert!(grade_ok(
            "Wuvable Oaf: Blood & Metal",
            "Wuvable Oaf: Blood and Metal"
        ));
        assert!(grade_ok("Red Sonja", "\u{200b}Free Issue Red Sonja #7"));
        assert!(grade_ok(
            "Kevin Keller: Welcome to Riverdale",
            "Kevin Keller Vol 1: Welcome to Riverdale"
        ));
    }

    #[test]
    fn still_fails_a_genuinely_wrong_answer() {
        assert!(
            !grade_ok("MASK Omnibus", "The Mask Omnibus"),
            "leading articles must NOT be dropped"
        );
        assert!(!grade_ok(
            "\u{30ce}\u{30e9}\u{30ac}\u{30df} [Noragami]",
            "Noragami Vol. 3"
        ));
    }

    /// Humble's title carries the *volume's* subtitle (`Vol. 2: Renegades`).
    /// A series-and-volume resolver never claims one, so series-only is a
    /// correct answer — not a silent false positive.
    #[test]
    fn accepts_series_only_against_a_volume_subtitle() {
        assert!(grade_ok("Harbinger", "Harbinger, Vol. 2: Renegades"));
        assert!(grade_ok("Shadowman", "Shadowman, Vol. 3: Deadside Blues"));
        // No space after the colon, so `3:Planet` arrives as one token.
        assert!(grade_ok(
            "X-O Manowar",
            "X-O Manowar, Vol. 3:Planet Death (plus extra content)"
        ));
    }

    /// The leniency above must not slide into accepting a shorter series name.
    #[test]
    fn rejects_a_series_that_is_merely_a_prefix() {
        assert!(
            !grade_ok("Harbinger", "Harbinger Wars Vol. 1"),
            "a prefix of the real series is a wrong answer, not a subtitle"
        );
        assert!(!grade_ok("X-O", "X-O Manowar, Vol. 3: Planet Death"));
    }

    #[test]
    fn gates_hold_each_threshold_independently() {
        let pass = Rates {
            located: 92.3,
            ready: 66.0,
            silent_false_positive: 0.48,
        };
        assert!(gates_pass(pass), "the thresholds themselves must pass");
        assert!(!gates_pass(Rates { located: 92.2, ..pass }));
        assert!(!gates_pass(Rates { ready: 65.9, ..pass }));
        assert!(!gates_pass(Rates {
            silent_false_positive: 0.49,
            ..pass
        }));
    }
}
