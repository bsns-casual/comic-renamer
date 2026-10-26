//! Task 5. The ladder (§6.3): language is a hard filter, publisher a
//! preference, volume feasibility a hard constraint, shortest run last and
//! only when a volume token is present.

mod common;

use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::resolve::{corpus_ladder, publisher_hint, suggest_with, Context};
use comic_renamer_core::types::{PublisherHint, Suggestion, SuggestionSource};
use common::corpus;
use tempfile::TempDir;

/// The ladder as the old `resolve` was called. `None` is "the corpus found
/// nothing"; `Some(empty)` is "found it and refused"; several is ambiguity.
fn resolve(
    stem: &str,
    source_folder: &str,
    hint: &PublisherHint,
    corpus: &comic_renamer_core::corpus::Corpus,
    overrides: &Overrides,
    lang: &str,
) -> Option<Vec<Suggestion>> {
    let ctx = Context {
        source_folder,
        hint: hint.clone(),
        overrides: Some(overrides),
        lang,
    };
    corpus_ladder(stem, corpus, &ctx)
}

fn only(found: Option<Vec<Suggestion>>) -> Suggestion {
    let found = found.expect("the corpus matched");
    assert_eq!(found.len(), 1, "expected exactly one suggestion, got {found:?}");
    found.into_iter().next().unwrap()
}

fn no_hint() -> PublisherHint {
    PublisherHint(None)
}

fn no_ov() -> Overrides {
    Overrides::default()
}

fn witch_and_witcher() -> (TempDir, comic_renamer_core::corpus::Corpus) {
    corpus(&[
        ("Witch", "witch", "Marvel", "en", 2004, 3),
        ("The Witcher Omnibus", "witcheromnibus", "Dark Horse", "en", 2017, 2),
    ])
}

/// One title, three records (§6.3). Comparing title strings calls this
/// unambiguous, which is the single most dangerous case.
fn three_velvets() -> (TempDir, comic_renamer_core::corpus::Corpus) {
    corpus(&[
        ("Velvet", "velvet", "Malibu", "en", 1993, 4),
        ("Velvet", "velvet", "Image", "en", 2013, 15),
        ("Velvet", "velvet", "Image", "en", 2014, 3),
    ])
}

fn short_runs() -> (TempDir, comic_renamer_core::corpus::Corpus) {
    corpus(&[
        ("Some Series", "someseries", "Image", "en", 2010, 3),
        ("Some Series", "someseries", "Marvel", "en", 2012, 5),
    ])
}

fn japanese_only() -> (TempDir, comic_renamer_core::corpus::Corpus) {
    corpus(&[("\u{30ce}\u{30e9}\u{30ac}\u{30df}", "noragami", "Kodansha", "ja", 2011, 27)])
}

fn two_masks() -> (TempDir, comic_renamer_core::corpus::Corpus) {
    corpus(&[
        ("The Mask Omnibus", "maskomnibus", "Dark Horse", "en", 2008, 2),
        ("The Mask Omnibus", "maskomnibus", "Titan", "en", 2010, 3),
    ])
}

fn gcd_subset() -> (TempDir, comic_renamer_core::corpus::Corpus) {
    corpus(&[
        ("Hellboy Omnibus", "hellboyomnibus", "Dark Horse", "en", 2018, 4),
        ("The Walking Dead", "walkingdead", "Image", "en", 2003, 193),
    ])
}

fn twd_overrides() -> (TempDir, Overrides) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("overrides.toml");
    std::fs::write(
        &path,
        "[\"TWD_CompendiumOne\"]\nseries = \"The Walking Dead\"\nbundle = \"Image Comics in the '10s\"\n",
    )
    .unwrap();
    let overrides = Overrides::load(&path).unwrap();
    (dir, overrides)
}

const NO_FOLDER: &str = "";

#[test]
fn boundary_guard_rejects_a_midword_prefix() {
    let (_d, c) = witch_and_witcher();
    assert_eq!(
        only(resolve("witcheromnibus", NO_FOLDER, &no_hint(), &c, &no_ov(), "en")).series,
        "The Witcher Omnibus"
    );
}

#[test]
fn a_midword_prefix_with_no_longer_match_fails_rather_than_guessing() {
    // The guard's real job: with only *Witch* in the corpus, `witcheromnibus`
    // must not resolve to it. This is the case that produced the guard.
    let (_d, c) = corpus(&[("Witch", "witch", "Marvel", "en", 2004, 3)]);
    assert_eq!(
        resolve("witcheromnibus", NO_FOLDER, &no_hint(), &c, &no_ov(), "en"),
        None
    );
}

#[test]
fn bare_title_with_three_records_is_ambiguous() {
    let (_d, c) = three_velvets();
    let found = resolve("velvet", NO_FOLDER, &no_hint(), &c, &no_ov(), "en").unwrap();
    assert_eq!(found.len(), 3, "every candidate is listed, none picked");
    // The runs differ, so the picker can tell the three Velvets apart.
    let mut runs: Vec<_> = found.iter().map(|s| s.volume_count).collect();
    runs.sort();
    assert_eq!(runs, [Some(3), Some(4), Some(15)]);
}

#[test]
fn same_records_with_a_volume_resolve_by_shortest_run() {
    let (_d, c) = three_velvets();
    let r = only(resolve("velvet_vol1", NO_FOLDER, &no_hint(), &c, &no_ov(), "en"));
    // The shortest feasible run is Image 2014 (3 volumes), not the oldest
    // edition -- "prefer the earliest" was withdrawn for picking Malibu 1993.
    assert_eq!(r.volume_count, Some(3));
    assert_eq!(r.source, SuggestionSource::Corpus);
}

#[test]
fn volume_infeasible_goes_to_review() {
    // Review Focus 2
    let (_d, c) = short_runs();
    assert_eq!(
        resolve("someseries_vol13", NO_FOLDER, &no_hint(), &c, &no_ov(), "en"),
        Some(Vec::new()),
        "the series was found and refused: no suggestion, and not a miss"
    );
}

#[test]
fn language_never_falls_back() {
    // D33
    let (_d, c) = japanese_only();
    let r = resolve("noragami_vol3", NO_FOLDER, &no_hint(), &c, &no_ov(), "en");
    assert_eq!(r, Some(Vec::new()), "a Japanese record is never offered for English");
}

#[test]
fn publisher_hint_breaks_a_tie() {
    let (_d, c) = two_masks();
    let h = publisher_hint("HumbleDarkHorseAnniversaryBundle", &c);
    assert_eq!(h.0.as_deref(), Some("Dark Horse"));
    let r = only(resolve(
        "maskomnibusvolume1secondedition",
        "HumbleDarkHorseAnniversaryBundle",
        &h,
        &c,
        &no_ov(),
        "en",
    ));
    assert_eq!(r.series, "The Mask Omnibus");
    // Dark Horse's record has a run of 2; Titan's has 3.
    assert_eq!(r.volume_count, Some(2));
}

/// The wiring, not the hint. `uses_the_source_folder_name_as_the_publisher_hint`
/// was replaced during the rebuild by a test of `Mappings::lookup`'s bundle
/// tie-break — a different property — and §6.3's folder-name wiring was left
/// with no end-to-end proof: setting `source_folder: ""` and
/// `hint: PublisherHint(None)` at the one call site broke nothing.
#[test]
fn both_the_layer_b_key_and_the_publisher_hint_come_from_the_folder_name() {
    let (_d, c) = two_masks();
    let folder = "HumbleDarkHorseAnniversaryBundle";
    let ov = no_ov();
    let ctx = comic_renamer_core::resolve::context_for(folder, "en", &ov, Some(&c));
    assert_eq!(ctx.source_folder, folder, "Layer B keys on it");
    assert_eq!(ctx.hint.0.as_deref(), Some("Dark Horse"), "and so does the hint");
    assert_eq!(ctx.lang, "en");

    // It is the hint that decides here: Dark Horse's record has a run of 2.
    let r = only(corpus_ladder("maskomnibusvolume1secondedition", &c, &ctx));
    assert_eq!((r.series.as_str(), r.volume_count), ("The Mask Omnibus", Some(2)));

    // No corpus means no hint to derive, which is §6.3's common case.
    let bare = comic_renamer_core::resolve::context_for(folder, "en", &ov, None);
    assert_eq!(bare.hint.0, None);
    assert_eq!(bare.source_folder, folder);
}

#[test]
fn no_publisher_signal_is_the_common_case_not_an_error() {
    // §6.3: a user running against ~/Downloads yields no publisher at all.
    let (_d, c) = two_masks();
    assert_eq!(publisher_hint("Downloads", &c).0, None);
}

#[test]
fn an_override_wins_and_is_reachable() {
    let (_d, c) = gcd_subset();
    let (_o, ov) = twd_overrides();
    let r = only(resolve(
        "TWD_CompendiumOne",
        "HumbleImageComicsInThe10s",
        &no_hint(),
        &c,
        &ov,
        "en",
    ));
    assert_eq!(r.series, "The Walking Dead");
    assert_eq!(r.volume_count, None, "an override names no record");
}

#[test]
fn an_override_from_the_wrong_bundle_does_not_fire() {
    let (_d, c) = gcd_subset();
    let (_o, ov) = twd_overrides();
    let r = resolve("TWD_CompendiumOne", "HumbleBoomBundle", &no_hint(), &c, &ov, "en");
    assert_eq!(r, None, "the override did not fire and the corpus has no such series");
}

#[test]
fn a_subtitle_is_carried_and_marked_review() {
    // D15
    let (_d, c) = gcd_subset();
    let r = only(resolve(
        "hellboyomnibusvolume1_seedofdestruction",
        NO_FOLDER,
        &no_hint(),
        &c,
        &no_ov(),
        "en",
    ));
    assert_eq!(r.subtitle.as_deref(), Some("seedofdestruction"));
}

#[test]
fn an_unknown_series_fails_rather_than_matching_something() {
    let (_d, c) = gcd_subset();
    assert_eq!(
        resolve("totallyunknownthing", NO_FOLDER, &no_hint(), &c, &no_ov(), "en"),
        None
    );
}

#[test]
fn a_multibyte_filename_does_not_panic() {
    // The residue tokenizer sliced by byte behind a byte-length guard, so any
    // residue whose boundary fell inside a character crashed the process.
    // `eat` runs inside the boundary guard, before a match is even accepted,
    // so this reached the resolver for names that would never have resolved.
    let (_d, c) = gcd_subset();
    for stem in [
        "naruto\u{5b8c}\u{5168}\u{7248}\u{7b2c}\u{4e00}\u{5dfb}",
        "\u{30ce}\u{30e9}\u{30ac}\u{30df}vol1",
        "hellboyomnibus\u{5b8c}\u{5168}\u{7248}",
        "secondeditio\u{5168}",
    ] {
        // Any outcome is acceptable; a panic is not.
        let _ = resolve(stem, NO_FOLDER, &no_hint(), &c, &no_ov(), "en");
    }
}

#[test]
fn a_tie_on_run_length_is_ambiguous_not_the_earliest_edition() {
    // D28 withdraws "prefer the earliest edition" by name, for resolving
    // Velvet_vol1 to Malibu 1993 inside an Image bundle. A tie on run means
    // shortest-run decided nothing, so the row must go to review.
    let (_d, c) = corpus(&[
        ("Velvet", "velvet", "Malibu", "en", 1993, 3),
        ("Velvet", "velvet", "Image", "en", 2014, 3),
    ]);
    let found = resolve("velvet_vol1", NO_FOLDER, &no_hint(), &c, &no_ov(), "en").unwrap();
    assert_eq!(found.len(), 2, "both editions are offered, neither picked: {found:?}");
}

#[test]
fn shortest_run_still_fires_when_there_is_a_single_shortest() {
    // The other half: the tie guard must not disable the heuristic that
    // carries 39 of the 71 points of `ready`.
    let (_d, c) = three_velvets();
    let r = only(resolve("velvet_vol1", NO_FOLDER, &no_hint(), &c, &no_ov(), "en"));
    assert_eq!(r.volume_count, Some(3));
}

#[test]
fn publisher_hint_ignores_a_substring_coincidence() {
    // `Humble Valiant Bundle` matched *Lian* inside va-LIAN-t under substring
    // matching; `Hellboy Expanded Universe` matched *Pan*.
    let (_d, c) = corpus(&[
        ("Something", "something", "Lian", "en", 2004, 3),
        ("Other", "other", "Pan", "en", 2004, 3),
    ]);
    assert_eq!(publisher_hint("Humble Valiant Bundle", &c).0, None);
    assert_eq!(publisher_hint("Hellboy Expanded Universe 2025", &c).0, None);
}

#[test]
fn publisher_hint_matches_a_camel_case_folder_on_word_boundaries() {
    let (_d, c) = two_masks();
    assert_eq!(
        publisher_hint("HumbleDarkHorseAnniversaryBundle", &c).0.as_deref(),
        Some("Dark Horse")
    );
}

#[test]
fn the_distributor_name_does_not_outrank_the_real_publisher() {
    // The corpus holds a small publisher literally called "Humble Comics", and
    // taking the longest match preferred it over the IDW named in the same
    // folder. Dropping the shop's own name from the front settles it.
    let (_d, c) = corpus(&[
        ("A", "a", "Humble Comics", "en", 2004, 3),
        ("B", "b", "IDW", "en", 2004, 3),
    ]);
    assert_eq!(
        publisher_hint("Humble Comics Bundle: Godzilla by IDW", &c)
            .0
            .as_deref(),
        Some("IDW")
    );
}

#[test]
fn publisher_hint_is_absent_when_two_publishers_match() {
    // §6.3 step 2 lets the hint promote a row to `ready`, so a wrong one is a
    // silent-wrongness path. Step 5: never silently pick.
    let (_d, c) = corpus(&[
        ("A", "a", "Dark Horse", "en", 2004, 3),
        ("B", "b", "Image", "en", 2004, 3),
    ]);
    assert_eq!(
        publisher_hint("Dark Horse and Image Crossover Showcase", &c).0,
        None
    );
}

#[test]
fn a_two_letter_publisher_needs_its_own_word() {
    let (_d, c) = corpus(&[("A", "a", "DC", "en", 2004, 3)]);
    assert_eq!(publisher_hint("Humble DC Bundle", &c).0.as_deref(), Some("DC"));
    // "dc" inside a longer word must not match.
    assert_eq!(publisher_hint("Humble Abdcef Bundle", &c).0, None);
}

#[test]
fn feasibility_outranks_the_publisher_preference() {
    // §6.3: step 2 is a preference, step 3 a hard constraint. issue_count
    // counts one publisher's run, not a collected line — Lady Mechanika Vol. 6
    // is real, but the English line spans three publishers and no single
    // record reaches 6.
    let (_d, c) = corpus(&[
        ("Lady Mechanika", "ladymechanika", "Benitez Productions", "en", 2015, 7),
        ("Lady Mechanika", "ladymechanika", "Image", "en", 2022, 5),
        ("Lady Mechanika", "ladymechanika", "Image", "en", 2021, 1),
    ]);
    let hint = PublisherHint(Some("Image".into()));
    let r = only(resolve("ladymechanika_vol6", "", &hint, &c, &no_ov(), "en"));
    assert_eq!(r.series, "Lady Mechanika");
    assert_eq!(r.volume_count, Some(7));
}

#[test]
fn a_volume_no_record_can_hold_is_still_infeasible() {
    // Review Focus 2 must keep working: dropping the publisher preference is
    // not the same as dropping the constraint.
    let (_d, c) = short_runs();
    let hint = PublisherHint(Some("Image".into()));
    assert_eq!(
        resolve("someseries_vol13", "", &hint, &c, &no_ov(), "en"),
        Some(Vec::new())
    );
}

#[test]
fn an_override_fires_with_no_corpus_at_all() {
    // Layer B is embedded TOML and needs no corpus. A failed corpus fetch must
    // not cost the one answer a human already verified.
    let (_o, ov) = twd_overrides();
    let ctx = Context {
        source_folder: "HumbleImageComicsInThe10s",
        overrides: Some(&ov),
        ..Context::default()
    };
    let found = suggest_with("TWD_CompendiumOne", "Humble", None, &ctx);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].series, "The Walking Dead");
    assert_eq!(found[0].source, SuggestionSource::Override);
}

#[test]
fn an_override_says_it_is_an_override_not_a_corpus_guess() {
    let (_d, c) = gcd_subset();
    let (_o, ov) = twd_overrides();
    let r = only(resolve("TWD_CompendiumOne", "HumbleImageComicsInThe10s", &no_hint(), &c, &ov, "en"));
    assert_eq!(r.source, SuggestionSource::Override);
}

#[test]
fn a_squashed_subtitle_is_identifiable_on_the_suggestion() {
    // D15: the corpus path cannot split a squashed subtitle, so a hit that
    // carries one must be told apart from a clean hit. A picker defaulting to
    // Enter would otherwise commit `Hellboy Omnibus: seedofdestruction`.
    let (_d, c) = gcd_subset();
    let squashed = only(resolve("hellboyomnibusvolume1_seedofdestruction", NO_FOLDER, &no_hint(), &c, &no_ov(), "en"));
    let clean = only(resolve("hellboyomnibusvolume1", NO_FOLDER, &no_hint(), &c, &no_ov(), "en"));
    assert_eq!(squashed.source, SuggestionSource::Corpus);
    assert_eq!(squashed.subtitle.as_deref(), Some("seedofdestruction"));
    assert!(squashed.is_unsplit());
    assert!(!clean.is_unsplit());
    assert_eq!(clean.subtitle, None);
}

#[test]
fn an_override_fires_for_every_marketplace_deliberately() {
    // Deliberate: do not gate this on Humble. An override is a recorded fact,
    // not a guess; `--marketplace` picks a guess *strategy*, and a verified
    // answer does not depend on one. The bundle check already scopes an entry
    // to its folder, and Layer B is the precursor of the mapping table, which
    // is marketplace-independent. Gating it recreates "a verified correction
    // made unreachable by a condition it does not need".
    let (_o, ov) = twd_overrides();
    let ctx = Context {
        source_folder: "HumbleImageComicsInThe10s",
        overrides: Some(&ov),
        ..Context::default()
    };
    for marketplace in ["Fanatical", "Global Comix", "Sweet Shop"] {
        let found = suggest_with("TWD_CompendiumOne", marketplace, None, &ctx);
        assert_eq!(found.len(), 1, "{marketplace}: {found:?}");
        assert_eq!(found[0].source, SuggestionSource::Override, "{marketplace}");
    }
}

/// `sweet_shop_title` already rewrites `-issue-3` to `-#3`; the parser used to
/// drop it, so the number that told two issues apart reached no field at all.
#[test]
fn a_sweet_shop_issue_suffix_sets_the_issue() {
    let ctx = Context::default();
    let found = suggest_with("young-shadow-the-watchdogs-issue-3", "Sweet Shop", None, &ctx);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].issue, Some(3), "{found:?}");
}
