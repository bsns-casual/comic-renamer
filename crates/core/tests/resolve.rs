//! Task 5. The ladder (§6.3): language is a hard filter, publisher a
//! preference, volume feasibility a hard constraint, shortest run last and
//! only when a volume token is present.

mod common;

use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::resolve::{publisher_hint, resolve};
use comic_renamer_core::types::{Basis, PublisherHint, ReviewReason, Status};
use common::corpus;
use tempfile::TempDir;

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
        resolve("witcheromnibus", NO_FOLDER, &no_hint(), &c, &no_ov(), "en").series,
        "The Witcher Omnibus"
    );
}

#[test]
fn a_midword_prefix_with_no_longer_match_fails_rather_than_guessing() {
    // The guard's real job: with only *Witch* in the corpus, `witcheromnibus`
    // must not resolve to it. This is the case that produced the guard.
    let (_d, c) = corpus(&[("Witch", "witch", "Marvel", "en", 2004, 3)]);
    assert_eq!(
        resolve("witcheromnibus", NO_FOLDER, &no_hint(), &c, &no_ov(), "en").status,
        Status::Failed
    );
}

#[test]
fn bare_title_with_three_records_is_ambiguous() {
    let (_d, c) = three_velvets();
    assert!(matches!(
        resolve("velvet", NO_FOLDER, &no_hint(), &c, &no_ov(), "en").status,
        Status::Review(ReviewReason::Ambiguous(_))
    ));
}

#[test]
fn same_records_with_a_volume_resolve_by_shortest_run() {
    let (_d, c) = three_velvets();
    let r = resolve("velvet_vol1", NO_FOLDER, &no_hint(), &c, &no_ov(), "en");
    assert_eq!(r.status, Status::Ready(Basis::ShortestRun));
    // The shortest feasible run is Image 2014, not the oldest edition —
    // "prefer the earliest" was withdrawn for picking Malibu 1993.
    assert_eq!(r.record.as_ref().unwrap().year_began, 2014);
}

#[test]
fn volume_infeasible_goes_to_review() {
    // Review Focus 2
    let (_d, c) = short_runs();
    assert!(matches!(
        resolve("someseries_vol13", NO_FOLDER, &no_hint(), &c, &no_ov(), "en").status,
        Status::Review(ReviewReason::VolumeInfeasible)
    ));
}

#[test]
fn language_never_falls_back() {
    // D33
    let (_d, c) = japanese_only();
    let r = resolve("noragami_vol3", NO_FOLDER, &no_hint(), &c, &no_ov(), "en");
    assert!(matches!(r.status, Status::Review(ReviewReason::NoLanguageMatch)));
    assert!(r.record.is_none());
}

#[test]
fn publisher_hint_breaks_a_tie() {
    let (_d, c) = two_masks();
    let h = publisher_hint("HumbleDarkHorseAnniversaryBundle", &c);
    assert_eq!(h.0.as_deref(), Some("Dark Horse"));
    let r = resolve(
        "maskomnibusvolume1secondedition",
        "HumbleDarkHorseAnniversaryBundle",
        &h,
        &c,
        &no_ov(),
        "en",
    );
    assert_eq!(r.series, "The Mask Omnibus");
    assert_eq!(r.record.as_ref().unwrap().publisher, "Dark Horse");
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
    let r = resolve(
        "TWD_CompendiumOne",
        "HumbleImageComicsInThe10s",
        &no_hint(),
        &c,
        &ov,
        "en",
    );
    assert_eq!(
        (r.series.as_str(), r.status),
        ("The Walking Dead", Status::Ready(Basis::Override))
    );
}

#[test]
fn an_override_from_the_wrong_bundle_does_not_fire() {
    let (_d, c) = gcd_subset();
    let (_o, ov) = twd_overrides();
    let r = resolve("TWD_CompendiumOne", "HumbleBoomBundle", &no_hint(), &c, &ov, "en");
    assert_ne!(r.status, Status::Ready(Basis::Override));
}

#[test]
fn a_subtitle_is_carried_and_marked_review() {
    // D15
    let (_d, c) = gcd_subset();
    let r = resolve(
        "hellboyomnibusvolume1_seedofdestruction",
        NO_FOLDER,
        &no_hint(),
        &c,
        &no_ov(),
        "en",
    );
    assert_eq!(r.subtitle.as_deref(), Some("seedofdestruction"));
    assert!(matches!(r.status, Status::Review(ReviewReason::UnsplitSubtitle(_))));
}

#[test]
fn an_unknown_series_fails_rather_than_matching_something() {
    let (_d, c) = gcd_subset();
    assert_eq!(
        resolve("totallyunknownthing", NO_FOLDER, &no_hint(), &c, &no_ov(), "en").status,
        Status::Failed
    );
}
