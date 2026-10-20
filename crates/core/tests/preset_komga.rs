//! Task 6. Flat `<Series>/<Book>.<ext>` (§6.8.1): Komga reads only the leaf
//! folder, and the filename *is* the displayed title.

use comic_renamer_core::normalize::nfc;
use comic_renamer_core::preset::komga::{pad, Komga};
use comic_renamer_core::preset::{preset_by_id, Preset};
use comic_renamer_core::types::{Basis, Resolved, Status};
use std::path::PathBuf;

fn res(series: &str, volume: Option<u32>, run: u32, subtitle: Option<&str>) -> Resolved {
    Resolved {
        series: series.to_string(),
        volume,
        subtitle: subtitle.map(str::to_string),
        run,
        status: Status::Ready(Basis::Unambiguous),
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
            &res("Hellboy Omnibus", Some(1), 5, Some("Seed of Destruction")),
            "pdf"
        ),
        PathBuf::from("Hellboy Omnibus/Hellboy Omnibus Vol. 01 - Seed of Destruction.pdf")
    );
}

#[test]
fn sanitizes_without_rejecting() {
    let s = Komga
        .destination(&res("Nemesis: Rogues' Gallery", None, 1, None), "pdf")
        .to_string_lossy()
        .into_owned();
    assert!(!s.contains(':') && s.contains('\''));
}

#[test]
fn distinct_titles_never_converge() {
    let a = Komga.destination(&res("Hellboy: Seed", None, 1, None), "pdf");
    let b = Komga.destination(&res("Hellboy - Seed", None, 1, None), "pdf");
    assert_ne!(
        a, b,
        "sanitising two titles onto one path silently merges two series"
    );
}

#[test]
fn refuses_hostile_components() {
    for t in ["CON", "trailing.", "trailing ", &"x".repeat(300)] {
        let p = Komga.destination(&res(t, None, 1, None), "pdf");
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
    let p = Komga.destination(&res(&"x".repeat(300), Some(1), 5, None), "pdf");
    let name = p.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.len() <= 255, "{} bytes", name.len());
    assert!(name.ends_with(".pdf"));
}

#[test]
fn truncation_never_splits_a_utf8_sequence() {
    let p = Komga.destination(&res(&"é".repeat(200), None, 1, None), "pdf");
    // to_string_lossy would hide a split by substituting U+FFFD.
    assert!(!p.to_string_lossy().contains('\u{fffd}'));
}

#[test]
fn output_is_nfc() {
    let p = Komga.destination(&res("Poke\u{301}mon", None, 1, None), "pdf");
    assert_eq!(p.to_string_lossy(), nfc(&p.to_string_lossy()));
}

#[test]
fn volume_precedes_subtitle() {
    // §6.8.1: number and numberSort are positional after a natural sort, so
    // the volume has to sort before anything a subtitle could add.
    let p = Komga.destination(&res("S", Some(2), 9, Some("Sub")), "pdf");
    let name = p.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(name, "S Vol. 02 - Sub.pdf");
}

#[test]
fn preset_by_id_resolves_komga_and_nothing_else() {
    assert_eq!(preset_by_id("komga").unwrap().id(), "komga");
    assert!(preset_by_id("plex").is_none());
}
