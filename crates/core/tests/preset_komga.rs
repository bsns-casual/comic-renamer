//! Task 6. Flat `<Series>/<Book>.<ext>` (§6.8.1): Komga reads only the leaf
//! folder, and the filename *is* the displayed title.

use comic_renamer_core::normalize::nfc;
use comic_renamer_core::preset::komga::{pad, Komga};
use comic_renamer_core::preset::{preset_by_id, Preset};
use comic_renamer_core::types::{Resolved, Status};
use comic_renamer_core::fsops::find_collisions;
use comic_renamer_core::types::{Mechanism, PlanRow, SourceStat};
use std::path::PathBuf;
use tempfile::tempdir;

fn res(series: &str, volume: Option<u32>, issue: Option<u32>, run: u32, subtitle: Option<&str>) -> Resolved {
    Resolved {
        series: series.to_string(),
        volume,
        issue,
        subtitle: subtitle.map(str::to_string),
        run,
        status: Status::Mapped,
        record: None,
    }
}

#[test]
fn pads_to_per_series_width() {
    // The plan's assertion compares a 4-tuple of String against a 4-tuple of
    // &str; std implements tuple PartialEq only between identical types.
    assert_eq!(
        (
            pad(1, 9).as_str(),
            pad(1, 34).as_str(),
            pad(34, 34).as_str(),
            pad(1, 120).as_str()
        ),
        ("01", "01", "34", "001")
    );
}

#[test]
fn emits_a_flat_relative_path() {
    assert_eq!(
        Komga.destination(
            &res("Hellboy Omnibus", Some(1), None, 5, Some("Seed of Destruction")),
            "pdf"
        ),
        PathBuf::from("Hellboy Omnibus/Hellboy Omnibus Vol. 01 - Seed of Destruction.pdf")
    );
}

#[test]
fn sanitizes_without_rejecting() {
    let s = Komga
        .destination(&res("Nemesis: Rogues' Gallery", None, None, 1, None), "pdf")
        .to_string_lossy()
        .into_owned();
    assert!(!s.contains(':') && s.contains('\''));
}

#[test]
fn titles_that_converge_are_caught_by_the_collision_detector() {
    // `Hellboy: Seed` and `Hellboy - Seed` now land on one folder, because
    // the substitution is plain ASCII rather than a look-alike nobody can
    // type. That is safe only because the merge is *reported*: the plan's
    // original assertion was about two series merging **silently**.
    let a = Komga.destination(&res("Hellboy: Seed", None, None, 1, None), "pdf");
    let b = Komga.destination(&res("Hellboy - Seed", None, None, 1, None), "pdf");
    assert_eq!(a, b, "the ASCII substitution is many-to-one by design");

    let d = tempdir().unwrap();
    let rows = [
        plan_row("a", "Hellboy: Seed"),
        plan_row("b", "Hellboy - Seed"),
    ];
    assert_eq!(
        find_collisions(&rows, d.path(), &Komga).len(),
        1,
        "a convergence that is not reported is a silent series merge"
    );
}

#[test]
fn no_emitted_path_contains_a_colon_in_any_form() {
    // Maintainer's call: never a colon in a file or folder name, and that
    // includes the look-alikes an earlier version substituted.
    for title in ["Nemesis: Rogues' Gallery", "Powers: The Best Ever", "A:B"] {
        let p = Komga
            .destination(&res(title, Some(1), None, 9, Some("Sub: Title")), "pdf")
            .to_string_lossy()
            .into_owned();
        for forbidden in [':', '\u{a789}', '\u{ff1a}', '\u{2236}'] {
            assert!(!p.contains(forbidden), "{p:?} contains {forbidden:?}");
        }
        assert!(!p.contains("  "), "{p:?} has a doubled space");
    }
}

#[test]
fn refuses_hostile_components() {
    for t in ["CON", "trailing.", "trailing ", &"x".repeat(300)] {
        let p = Komga.destination(&res(t, None, None, 1, None), "pdf");
        let c = p
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_string_lossy()
            .into_owned();
        assert!(c.len() <= 255 && !c.ends_with('.') && !c.ends_with(' ') && c != "CON");
    }
}

#[test]
fn the_filename_is_capped_too() {
    // §6.8.1: the filename is the displayed title, and it is subject to the
    // same 255-byte limit as the folder.
    let p = Komga.destination(&res(&"x".repeat(300), Some(1), None, 5, None), "pdf");
    let name = p.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.len() <= 255, "{} bytes", name.len());
    assert!(name.ends_with(".pdf"));
}

#[test]
fn truncation_never_splits_a_utf8_sequence() {
    let p = Komga.destination(&res(&"é".repeat(200), None, None, 1, None), "pdf");
    // to_string_lossy would hide a split by substituting U+FFFD.
    assert!(!p.to_string_lossy().contains('\u{fffd}'));
}

#[test]
fn output_is_nfc() {
    let p = Komga.destination(&res("Poke\u{301}mon", None, None, 1, None), "pdf");
    assert_eq!(p.to_string_lossy(), nfc(&p.to_string_lossy()));
}

#[test]
fn volume_precedes_subtitle() {
    // §6.8.1: number and numberSort are positional after a natural sort, so
    // the volume has to sort before anything a subtitle could add.
    let p = Komga.destination(&res("S", Some(2), None, 9, Some("Sub")), "pdf");
    let name = p.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(name, "S Vol. 02 - Sub.pdf");
}

#[test]
fn preset_by_id_resolves_komga_and_nothing_else() {
    assert_eq!(preset_by_id("komga").unwrap().id(), "komga");
    assert!(preset_by_id("plex").is_none());
}

#[test]
fn zero_width_characters_do_not_survive_into_a_path() {
    // §6.11 forbids them in rule data; corpus titles are sanitised instead.
    // CLAUDE.md records Humble prefixing promo items with a "Free Issue"
    // marker containing one, so this arrives with real input.
    let plain = Komga.destination(&res("Hellboy", None, None, 1, None), "pdf");
    for invisible in ['\u{200b}', '\u{feff}', '\u{ad}', '\u{202e}', '\u{2060}', '\u{2028}', '\u{fe0f}', '\u{34f}', '\u{3164}', '\u{e0041}'] {
        let spiked = format!("Hell{invisible}boy");
        assert_eq!(
            Komga.destination(&res(&spiked, None, None, 1, None), "pdf"),
            plain,
            "{invisible:?} survived into the path"
        );
    }
}

/// A no-break space reads as a space, so `Hell<U+00A0>boy` and `Hell boy` are
/// one folder to anyone looking and two on disk. Mapped to a plain space
/// rather than rejected: unlike a zero-width character it is visible, and a
/// title that uses one is still a title.
#[test]
fn space_look_alikes_become_a_plain_space() {
    let plain = Komga.destination(&res("Hell boy", None, None, 1, None), "pdf");
    for space in ['\u{a0}', '\u{2000}', '\u{2004}', '\u{200a}'] {
        let spiked = format!("Hell{space}boy");
        assert_eq!(
            Komga.destination(&res(&spiked, None, None, 1, None), "pdf"),
            plain,
            "U+{:04X} makes a second folder",
            space as u32
        );
    }
}

#[test]
fn an_extension_longer_than_the_budget_does_not_underflow() {
    // MAX_COMPONENT_BYTES - suffix.len() panicked in debug and wrapped in
    // release, skipping the 255-byte cap entirely.
    let p = Komga.destination(&res("S", None, None, 1, None), &"x".repeat(300));
    let name = p.file_name().unwrap().to_string_lossy().into_owned();
    assert!(!name.is_empty());
}

/// The extension was the one field the preset never sanitised: it went into
/// `format!(".{ext}")` and straight into `join`. Invariant 1 says a bad row may
/// produce a wrong *name* and never a wrong *location*, and `plan::escapes`
/// rejecting `..` afterwards is an incidental defence, not this one.
#[test]
fn the_extension_cannot_add_a_path_component_or_emit_a_parent_dir() {
    for ext in ["cbz/sub/dir", "cbz/../../etc", "..", ".", &"p".repeat(254), &"q".repeat(300), "/", "\\x"] {
        let p = Komga.destination(&res("Hellboy", Some(1), None, 1, None), ext);
        let parts: Vec<_> = p.components().collect();
        assert_eq!(parts.len(), 2, "{ext:?} made {parts:?}");
        assert!(
            !p.components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir)),
            "{ext:?} made {parts:?}"
        );
        let leaf = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!leaf.is_empty() && leaf.len() <= 255, "{ext:?} leaf is {} bytes", leaf.len());
        assert!(!leaf.starts_with('.'), "{ext:?} made a hidden file: {leaf:?}");
    }
}

#[test]
fn the_folder_and_the_filename_sanitise_the_same_way() {
    // Building the stem from the raw series while the folder used the
    // sanitised one let two titles merge in the folder and stay distinct in
    // the leaf — invisible to a collision check that keys on the whole path.
    for title in ["Hellboy.", "Hellboy ", "CON"] {
        let p = Komga.destination(&res(title, None, None, 1, None), "pdf");
        let folder = p.components().next().unwrap().as_os_str().to_string_lossy().into_owned();
        let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
        assert_eq!(folder, stem, "{title:?} sanitised two different ways");
    }
}

fn plan_row(id: &str, series: &str) -> PlanRow {
    PlanRow {
        id: id.into(),
        source: PathBuf::from(format!("/src/{id}.pdf")),
        destination_preview: PathBuf::new(),
        extension: "pdf".into(),
        series: series.into(),
        volume: None,
        issue: None,
        subtitle: None,
        run: 1,
        status: Status::Mapped,
        mechanism: Mechanism::Rename,
        source_stat: SourceStat {
            size: 1,
            mtime_ns: 1,
            inode: id.bytes().map(u64::from).sum(),
        },
        record: None,
    }
}

// --- D60: a single issue, padded to four digits, no `#` -------------------

fn dest(r: Resolved) -> PathBuf {
    Komga.destination(&r, "cbz")
}

#[test]
fn an_issue_renders_padded_to_four_digits_with_no_hash() {
    assert_eq!(
        dest(res("Bee and PuppyCat", None, Some(1), 0, None)),
        PathBuf::from("Bee and PuppyCat/Bee and PuppyCat 0001.cbz")
    );
}

#[test]
fn issue_zero_is_a_real_issue() {
    // Real comics ship a #0; Some(0) is not absence.
    assert_eq!(
        dest(res("Spawn", None, Some(0), 0, None)),
        PathBuf::from("Spawn/Spawn 0000.cbz")
    );
}

#[test]
fn a_volume_precedes_an_issue_and_both_precede_the_subtitle() {
    // Love and Rockets Vol. I #1 Facsimile Edition is all three at once.
    assert_eq!(
        dest(res("Love and Rockets", Some(1), Some(1), 4, Some("Facsimile Edition"))),
        PathBuf::from("Love and Rockets/Love and Rockets Vol. 01 0001 - Facsimile Edition.cbz")
    );
}

#[test]
fn a_series_named_with_a_number_cannot_be_displaced_by_an_issue_of_that_number() {
    // Both are real shapes: `Elephantmen 2260 Book One (#51-55)` is a series
    // whose name ends in 2260, and `Elephantmen #2260` would be an issue of a
    // series called Elephantmen. The two produce the SAME leaf name, so only
    // the series folder keeps them apart — flatten that and one overwrites
    // the other.
    let named = dest(res("Elephantmen 2260", None, None, 0, None));
    let issued = dest(res("Elephantmen", None, Some(2260), 0, None));
    assert_eq!(named.file_name(), issued.file_name(), "the leaf really does collide");
    assert_ne!(named, issued, "the series folder is the only thing separating them");
    assert_ne!(named.parent(), issued.parent());
}

#[test]
fn a_row_with_no_issue_renders_exactly_as_before() {
    assert_eq!(
        dest(res("Saga", Some(3), None, 0, None)),
        PathBuf::from("Saga/Saga Vol. 03.cbz")
    );
}

#[test]
fn two_rows_differing_only_by_issue_are_two_destinations_in_one_series() {
    let a = dest(res("Curse", None, Some(1), 0, None));
    let b = dest(res("Curse", None, Some(2), 0, None));
    assert_ne!(a, b);
    assert_eq!(a.parent(), b.parent(), "both issues belong to one series folder");
}
