//! Grading logic lives in the lib because a bin-only crate cannot be imported
//! by its own tests.

use comic_renamer_core::title::{is_volume_word, split_at_boundary};

/// A single pull request may add at most this many mapping rows. 2,500 and not
/// less: a seeding PR of about 2,200 rows must clear the check it exists to
/// pass through, or seeding would bypass its own gate.
pub const MAX_ROWS_PER_PR: usize = 2500;
/// One mapping file may not exceed 2 MiB (§6.11).
pub const MAX_MAPPING_FILE_BYTES: usize = 2 * 1024 * 1024;

pub fn rows_per_pr_ok(n: usize) -> bool {
    n <= MAX_ROWS_PER_PR
}

pub fn file_size_ok(bytes: usize) -> bool {
    bytes <= MAX_MAPPING_FILE_BYTES
}

/// Rows added by a PR, from numstat output (`added<TAB>removed<TAB>path`).
/// A binary file reports `-` and counts as zero here; the size cap catches it.
// shortcut: counts added lines, not net rows, so an edit in place counts as an add;
// that over-counts, so it errs toward refusing, never toward admitting.
pub fn added_rows(numstat: &str) -> usize {
    numstat
        .lines()
        .filter_map(|l| l.split('\t').next()?.parse::<usize>().ok())
        .sum()
}

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

/// Splits a title into (series, subtitle) at the first colon or volume
/// marker, whichever comes first, normalising both: zero-width characters
/// removed, `&` spelled "and", issue numbers and the volume marker itself
/// dropped, remaining punctuation dropped, whitespace collapsed.
///
/// Case and leading articles survive on purpose (owner decision,
/// 2026-10-22). Humble title-cases function words where the corpus does not,
/// and folding case to hide that would also let `MASK Omnibus` pass against
/// *The Mask Omnibus*.
/// Humble's promo prefix, "Free Issue" behind a zero-width space, and any
/// stray zero-width characters. Shared by the grader and the seed generator so
/// the two cannot drift. Only a "Free Issue" followed by more text is a prefix,
/// so a title that genuinely opens with "Free" keeps its first word.
pub fn strip_promo(s: &str) -> String {
    let clean: String = s
        .chars()
        .filter(|c| !comic_renamer_core::normalize::is_invisible(*c))
        .collect();
    let mut words = clean.split_whitespace();
    if let (Some(a), Some(b)) = (words.next(), words.next()) {
        if a.eq_ignore_ascii_case("free") && b.eq_ignore_ascii_case("issue") {
            let rest = clean.trim_start()[a.len()..].trim_start()[b.len()..].trim_start();
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    clean
}

fn split_title(s: &str) -> (Vec<String>, Vec<String>) {
    let expanded = strip_promo(s).replace('&', " and ");

    // Both boundaries apply: the series ends at whichever of the colon and
    // the volume marker comes first (shared with the title parser).
    let (head, _volume, tail) = split_at_boundary(&expanded);
    let series = words(head);
    let subtitle = words(tail);

    (series, subtitle)
}

/// Normalised words: issue-number tokens dropped, and a leftover volume
/// marker with its number. The boundary has already been found.
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut raw = s.split_whitespace().peekable();
    while let Some(token) = raw.next() {
        // Issue numbers go before punctuation is stripped, or `#7` survives
        // as a bare `7` indistinguishable from a number inside a title. The
        // `#` is not always first: Humble writes ranges as `(#1-6)`.
        if token.contains('#') {
            continue;
        }
        if is_volume_word(token) {
            if raw.peek().is_some_and(|t| looks_numeric(&alphanumeric(t))) {
                raw.next();
            }
            continue;
        }
        let clean = alphanumeric(token);
        if !clean.is_empty() {
            out.push(clean);
        }
    }
    out
}

/// Loose on purpose: the grader only asks whether a number follows the marker,
/// so `8-10` is consumed whole. The parser's stricter `number_value` would
/// leave it behind as a word and change what the gate measures.
fn looks_numeric(token: &str) -> bool {
    !token.is_empty()
        && (token.chars().all(|c| c.is_ascii_digit())
            || token.chars().all(|c| "IVXLCDM".contains(c)))
}

fn alphanumeric(token: &str) -> String {
    token.chars().filter(|c| c.is_alphanumeric()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pr_over_the_row_cap_is_refused() {
        assert!(rows_per_pr_ok(2500));
        assert!(!rows_per_pr_ok(2501));
    }

    #[test]
    fn a_mapping_file_over_two_mebibytes_is_refused() {
        assert!(file_size_ok(2 * 1024 * 1024));
        assert!(!file_size_ok(2 * 1024 * 1024 + 1));
        assert!(file_size_ok(comic_renamer_core::mapping::shipped_mappings().len()));
    }

    #[test]
    fn added_rows_sums_numstat_and_skips_binary() {
        let stat = "12\t3\tdata/mappings/a.tsv\n5\t0\tdata/mappings/b.tsv\n-\t-\tx.bin\n";
        assert_eq!(added_rows(stat), 17);
        assert_eq!(added_rows(""), 0);
    }

    #[test]
    fn strip_promo_removes_the_prefix_and_zero_width_characters() {
        assert_eq!(strip_promo("\u{200b}Free Issue Twilight Zone #1"), "Twilight Zone #1");
        assert_eq!(strip_promo("Free Issue"), "Free Issue", "nothing left to file under");
        assert_eq!(strip_promo("Free Fall"), "Free Fall");
        assert_eq!(strip_promo("Saga"), "Saga");
    }

    /// The three definitions of "invisible" were written at three widths:
    /// `strip_promo` cleaned less than `validate` rejected, so a promo title
    /// carrying a bidi control was partially cleaned and then refused. One
    /// predicate now, so neither can drift from the other.
    #[test]
    fn strip_promo_cleans_everything_the_validator_rejects() {
        for c in (0u32..0x11000).chain(0xE0000..0xE0080).filter_map(char::from_u32) {
            if !comic_renamer_core::normalize::is_invisible(c) {
                continue;
            }
            assert_eq!(strip_promo(&format!("Sa{c}ga")), "Saga", "U+{:04X} survived", c as u32);
        }
        assert_eq!(strip_promo("\u{202e}Free Issue Saga"), "Saga", "a bidi control ahead of the prefix");
    }

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

    /// Owner decision, 2026-10-22: a collection subtitle after a colon is not
    /// something a series-and-volume resolver claims, so series-only grades
    /// correct even with no volume marker to split on.
    #[test]
    fn accepts_series_only_against_a_colon_subtitle() {
        assert!(grade_ok(
            "Valiant Masters Bloodshot",
            "Valiant Masters Bloodshot: Blood of the Machine"
        ));
        assert!(grade_ok(
            "Six Days in Cincinnati",
            "Six Days in Cincinnati: A Graphic Account of the Riots"
        ));
    }

    /// Both boundaries have to hold at once. A colon split that replaced the
    /// volume boundary rather than joining it put the trailing parenthetical
    /// back into the series name.
    #[test]
    fn accepts_a_volume_boundary_with_no_colon_in_sight() {
        assert!(grade_ok("Minimum Wage", "Minimum Wage Vol. 1 (#1-6)"));
        assert!(grade_ok(
            "The Mask Omnibus",
            "The Mask Omnibus Volume 1 (Second Edition)"
        ));
        // The `#` is not the token's first character here.
        assert!(grade_ok("Velvet", "Velvet Vol. 1 (#1-5)"));
    }

    /// Owner decision, 2026-10-22: case is NOT folded. Humble title-cases
    /// function words where the corpus does not, and that stays a finding
    /// rather than becoming invisible.
    #[test]
    fn case_is_never_folded() {
        assert!(!grade_ok("Black Is the Color", "Black is the Color"));
        assert!(!grade_ok(
            "I Am Not Okay with This",
            "I Am Not Okay With This"
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
}
