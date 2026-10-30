//! Task 6. The suggestion layer: one strategy per marketplace, the corpus
//! optional. Nothing here is stored without a keystroke (D49).

mod common;

use comic_renamer_core::resolve::suggest;
use comic_renamer_core::types::SuggestionSource;
use common::corpus;

#[test]
fn no_corpus_still_suggests_and_never_errors() {
    let s = suggest("hammer-head-1_release-0001", "Global Comix", None);
    assert!(!s.is_empty(), "D49: absent corpus degrades to a filename guess, never an error");
    assert!(s.iter().all(|x| x.source == SuggestionSource::Filename));
}

#[test]
fn humble_without_a_corpus_still_guesses_from_the_filename() {
    let s = suggest("valiantmasters_bloodshot_hd_1410912611", "Humble", None);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].series, "Valiantmasters Bloodshot");
    assert_eq!(s[0].source, SuggestionSource::Filename);
}

#[test]
fn fanatical_keeps_the_filename_as_the_title() {
    let s = suggest("HackSlash Back to School", "Fanatical", None);
    assert_eq!(s[0].series, "HackSlash Back to School");
    let s = suggest("Killadelphia Vol. 4 The End of All", "Fanatical", None);
    assert_eq!(s[0].series, "Killadelphia");
    assert_eq!(s[0].volume, Some(4));
    assert_eq!(s[0].subtitle.as_deref(), Some("The End of All"));
}

#[test]
fn global_comix_drops_the_release_counter_and_the_isbn() {
    let s = suggest("hammer-head-1_release-0001_978-1-5458-3063-5", "Global Comix", None);
    assert_eq!(s[0].series, "Hammer Head");
    let s = suggest("toxic-man_release-0001", "Global Comix", None);
    assert_eq!(s[0].series, "Toxic Man");
}

#[test]
fn sweet_shop_drops_the_type_suffix() {
    assert_eq!(suggest("minotaur-issue-1", "Sweet Shop", None)[0].series, "Minotaur");
    assert_eq!(
        suggest("the-spawn-of-venus-and-other-stories-anthology", "Sweet Shop", None)[0].series,
        "The Spawn of Venus and Other Stories"
    );
    assert_eq!(
        suggest("young-shadow-the-watchdogs-graphicnovel", "Sweet Shop", None)[0].series,
        "Young Shadow the Watchdogs"
    );
}

#[test]
fn the_filename_strategy_is_wrong_for_die_loaded_and_that_is_expected() {
    // The real answer is `DIE: Loaded #1`. Title-casing a lowercase kebab name
    // cannot know `DIE` is an acronym or where the colon goes. This is exactly
    // why nothing is stored without a keystroke; do not special-case it.
    let s = suggest("die-loaded-issue-1", "Sweet Shop", None);
    assert_ne!(s[0].series, "DIE");
}

/// Ambiguity still yields several suggestions rather than a refusal — but the
/// records have to be ambiguous in a way a `Suggestion` can *express*.
///
/// This fixture used to be three `Velvet` rows differing in publisher and year
/// and it asserted `len() == 3`. A `Suggestion` carries neither field, so all
/// three were identical and the assertion pinned the duplicate-candidate bug
/// rather than ambiguity: `map`'s picker offered three lines nobody could tell
/// apart. Two distinguishable names behind one key is the real-world shape —
/// `Popeye` and `Pop-Eye` both squash to `popeye`.
#[test]
fn ambiguity_becomes_several_suggestions_not_a_refusal() {
    let (_d, c) = corpus(&[
        ("Velvet", "velvet", "Malibu", "en", 1993, 4),
        ("Vel-Vet", "velvet", "Image", "en", 2013, 15),
        ("Velvet", "velvet", "Image", "en", 2014, 3),
    ]);
    let s = suggest("velvet", "Humble", Some(&c));
    assert_eq!(s.len(), 2, "two names, not three records: {s:?}");
    assert_eq!(
        (s[0].series.as_str(), s[1].series.as_str()),
        ("Velvet", "Vel-Vet"),
        "first-occurrence order, never sorted"
    );
    assert!(s.iter().all(|x| x.source == SuggestionSource::Corpus));
}

/// The bug this collapse exists for, in the shape it was found in. Measured
/// against the real release on 2026-10-29: the Popeye stem returned 21
/// candidates, 20 of them the literal string `Popeye`, differing only in
/// `volume_count` — a field the picker does not render. `table::run_length`
/// then took whichever sorted first.
///
/// Highest wins (owner, 2026-10-30): `volume_count` only sets the zero-pad
/// width, `max(2, digits)`, so below volume 100 the choice is invisible and
/// above it the highest is the only value that cannot under-pad a real run.
#[test]
fn candidates_differing_only_in_volume_count_collapse_to_the_highest() {
    let (_d, c) = corpus(&[
        ("Popeye", "popeye", "Fantagraphics", "en", 2012, 4),
        ("Popeye", "popeye", "Dell", "en", 1948, 65),
        ("Popeye", "popeye", "Gold Key", "en", 1962, 7),
        ("Pop-Eye", "popeye", "Elsewhere", "en", 1935, 21),
    ]);
    let s = suggest("popeye", "Humble", Some(&c));

    assert_eq!(s.len(), 2, "{s:?}");
    assert_eq!(s[0].series, "Popeye");
    assert_eq!(s[0].volume_count, Some(65), "the highest of 4, 65 and 7");
    // A different spelling is a genuine choice and is never merged away:
    // invariant 7's case-folding decides whether two paths collide, not
    // whether two names a user is picking between are one name.
    assert_eq!(s[1].series, "Pop-Eye");
    assert_eq!(s[1].volume_count, Some(21));
}

#[test]
fn humble_falls_back_to_the_filename_only_when_the_corpus_found_nothing() {
    let (_d, c) = corpus(&[("Witch", "witch", "Marvel", "en", 2004, 3)]);
    let s = suggest("totally_unknown_thing", "Humble", Some(&c));
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].source, SuggestionSource::Filename);
}

#[test]
fn a_language_refusal_is_not_papered_over_with_a_filename_guess() {
    // D33: the corpus found the series and said no. A filename fallback here
    // would be the silent fallback that caused 6 of 7 errors.
    let (_d, c) =
        corpus(&[("\u{30ce}\u{30e9}\u{30ac}\u{30df}", "noragami", "Kodansha", "ja", 2011, 27)]);
    assert!(suggest("noragami_vol3", "Humble", Some(&c)).is_empty());
}

#[test]
fn only_humble_consults_the_corpus() {
    let (_d, c) =
        corpus(&[("The Witcher Omnibus", "witcheromnibus", "Dark Horse", "en", 2017, 2)]);
    let s = suggest("witcheromnibus", "Fanatical", Some(&c));
    assert_eq!(s[0].source, SuggestionSource::Filename);
    let s = suggest("witcheromnibus", "Humble", Some(&c));
    assert_eq!((s[0].series.as_str(), s[0].source), ("The Witcher Omnibus", SuggestionSource::Corpus));
}

