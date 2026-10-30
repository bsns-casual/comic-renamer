//! Task 10. `scan` is read-only by construction: it stats, it never opens.
//! The fixtures are zero-byte files with real names (invariant 3).

mod common;

use comic_renamer_core::mapping::Mappings;
use comic_renamer_core::scan::{scan, ScanOutcome};
use comic_renamer_core::types::{Mode, PlanRow, ScanError, ScanOpts, Status};
use common::{mappings, sparse_truncate_only, tree_from_names, TreeGuard};
use std::path::Path;
use tempfile::tempdir;

fn gcd() -> Mappings {
    mappings(&[
        ("mazebook", "Mazebook", None, None, None, Some(5)),
        ("hellboyomnibus_vol1", "Hellboy Omnibus", Some(1), None, None, Some(4)),
        ("maskomnibusvolume1secondedition", "The Mask Omnibus", Some(1), None, None, Some(2)),
    ])
}

fn opts(source: &Path, dest: &Path) -> ScanOpts {
    ScanOpts {
        source_root: source.to_path_buf(),
        dest_root: dest.to_path_buf(),
        preset: "komga".into(),
        mode: None,
        lang: "en".into(),
    }
}

fn run(o: &ScanOpts, m: &Mappings) -> Result<ScanOutcome, ScanError> {
    scan(o, m, &mut |_| {})
}

fn planned(o: &ScanOpts, m: &Mappings) -> (comic_renamer_core::types::PlanHeader, Vec<PlanRow>) {
    let outcome = run(o, m).unwrap();
    (outcome.header, outcome.rows)
}

#[test]
fn refuses_overlapping_roots_in_both_directions() {
    let c = gcd();
    let d = tempdir().unwrap();
    let outer = d.path();
    let inner = outer.join("inner");
    std::fs::create_dir(&inner).unwrap();

    // dest inside source, then source inside dest: a recursive scan would
    // re-ingest its own output.
    assert!(matches!(
        run(&opts(outer, &inner), &c),
        Err(ScanError::RootsOverlap)
    ));
    assert!(matches!(
        run(&opts(&inner, outer), &c),
        Err(ScanError::RootsOverlap)
    ));
    // The same directory is the degenerate case of both.
    assert!(matches!(
        run(&opts(outer, outer), &c),
        Err(ScanError::RootsOverlap)
    ));
}

#[test]
fn skips_non_comics_and_a_directory_named_pdf() {
    // Review Focus 1
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(
        source.path(),
        &[
            "mazebook.pdf",
            "readme.txt",
            "cover.jpg",
            "id_imagecomics.pdf",
            ".DS_Store",
            "LICENSE.pdf",
            "credits-page.cbz",
            "thumbs.db",
        ],
    );
    std::fs::create_dir(source.path().join("Hellboy.pdf")).unwrap();

    let (_h, rows) = planned(&opts(source.path(), dest.path()), &c);
    assert_eq!(rows.len(), 1, "kept {:?}", rows.iter().map(|r| &r.source).collect::<Vec<_>>());
    assert_eq!(rows[0].series, "Mazebook");
}

#[test]
fn scan_leaves_both_trees_byte_identical() {
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(source.path(), &["mazebook.pdf", "hellboyomnibus_vol1.pdf"]);
    std::fs::create_dir(dest.path().join("Existing")).unwrap();

    let _guard = TreeGuard::new(&[source.path(), dest.path()]);
    run(&opts(source.path(), dest.path()), &c).unwrap();
}

#[test]
fn records_logical_size_from_st_size_never_du() {
    // §6.10: `du` reports neither cloud-placeholder nor clone reality. A
    // sparse file is the cheap way to make the two disagree.
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    sparse_truncate_only(source.path(), "mazebook.pdf", 5 << 30);

    let (header, rows) = planned(&opts(source.path(), dest.path()), &c);
    assert_eq!(header.required_bytes, 5 << 30);
    assert_eq!(rows[0].source_stat.size, 5 << 30);
    // Nothing here is a cloud placeholder, so the download cost is nil and
    // must be reported apart from the disk cost (D23).
    assert_eq!(header.download_bytes, 0);
}

#[test]
fn a_zero_byte_source_is_a_row_not_a_fault() {
    // Invariant 3. An "empty means corrupt" guard would disable the suite.
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(source.path(), &["mazebook.pdf"]);
    let (header, rows) = planned(&opts(source.path(), dest.path()), &c);
    assert_eq!(rows.len(), 1);
    assert_eq!(header.required_bytes, 0);
}

/// This replaces `the_source_folder_name_is_the_bundle_that_breaks_a_tie`,
/// whose second half asserted the cost D64 exists to remove: "a folder matching
/// neither bundle gets no guess". Over every decision this project holds that
/// described 158 of 2,043 keys — all of them agreeing on every field, so there
/// was never a tie to break, only a folder name to fail to match.
///
/// `scan` passes no folder name to `lookup` any more. This is the test for that
/// site: the same file resolves from any folder.
#[test]
fn a_mapped_file_resolves_whatever_the_folder_is_called() {
    let tsv = "key\tseries\tvolume\tissue\tsubtitle\tlang\tvolume_count\n\
               maskomnibus\tThe Mask Omnibus\t\t\t\ten\t\n";
    let m = Mappings::load(tsv, None).unwrap();
    let dest = tempdir().unwrap();
    let parent = tempdir().unwrap();

    for folder in ["HumbleDarkHorseAnniversaryBundle", "Downloads", "whatever"] {
        let source = parent.path().join(folder);
        std::fs::create_dir(&source).unwrap();
        tree_from_names(&source, &["maskomnibus.pdf"]);
        let (_h, rows) = planned(&opts(&source, dest.path()), &m);
        assert_eq!(rows.len(), 1, "{folder}");
        assert_eq!(rows[0].series, "The Mask Omnibus", "{folder}");
    }
}

#[test]
fn multi_format_duplicates_become_separate_rows() {
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(
        source.path(),
        &["mazebook.pdf", "mazebook.cbz", "mazebook.epub"],
    );
    let (_h, rows) = planned(&opts(source.path(), dest.path()), &c);
    assert_eq!(rows.len(), 3);
    let mut ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 3, "row ids must be unique");
}

#[test]
fn the_plan_it_produces_round_trips() {
    // scan's output is plan::write's input; if they disagree the first real
    // run fails at the last step.
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(source.path(), &["mazebook.pdf", "hellboyomnibus_vol1.pdf"]);
    let (header, rows) = planned(&opts(source.path(), dest.path()), &c);

    let out = tempdir().unwrap();
    let path = out.path().join("plan.jsonl");
    comic_renamer_core::plan::write(&header, &rows, &path).unwrap();
    let (read_header, read_rows) = comic_renamer_core::plan::read(&path).unwrap();
    assert_eq!(read_rows, rows);
    assert_eq!(read_header.required_bytes, header.required_bytes);
}

#[test]
fn defaults_to_move_mode() {
    // D23: cloning a placeholder hydrates it, so move stays the default.
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(source.path(), &["mazebook.pdf"]);
    let (header, _) = planned(&opts(source.path(), dest.path()), &c);
    assert_eq!(header.mode, Mode::Move);
}

#[test]
fn refuses_an_unknown_preset_and_a_source_that_is_not_a_directory() {
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();

    let mut bad = opts(source.path(), dest.path());
    bad.preset = "plex".into();
    assert!(matches!(run(&bad, &c), Err(ScanError::UnknownPreset(_))));

    let file = source.path().join("not-a-dir");
    std::fs::File::create(&file).unwrap();
    assert!(matches!(
        run(&opts(&file, dest.path()), &c),
        Err(ScanError::NotADirectory(_))
    ));
}

#[test]
fn a_mapped_file_becomes_a_plan_row_and_an_unmapped_one_does_not() {
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(source.path(), &["mazebook.pdf", "totallyunknownthing.pdf"]);
    let outcome = run(&opts(source.path(), dest.path()), &c).unwrap();
    assert_eq!(outcome.rows.len(), 1);
    assert_eq!(outcome.rows[0].status, Status::Mapped);
    assert_eq!(outcome.header.unmapped, 1, "apply learns of left-behind files from the header");
    assert_eq!(outcome.rows[0].series, "Mazebook");
    assert_eq!(outcome.unmapped, vec![source.path().canonicalize().unwrap().join("totallyunknownthing.pdf")]);
}

#[test]
fn a_stem_with_no_key_is_unmapped_not_skipped() {
    // `key()` is None when nothing alphanumeric survives. Such a file must be
    // reported, not vanish the way a non-comic does.
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(source.path(), &["___.pdf", "(1).pdf", "mazebook.pdf"]);
    let outcome = run(&opts(source.path(), dest.path()), &c).unwrap();
    assert_eq!(outcome.rows.len(), 1);
    let mut names: Vec<_> = outcome
        .unmapped
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["(1).pdf", "___.pdf"]);
}

#[test]
fn unmapped_files_cost_no_bytes_and_get_no_row_id() {
    // Row ids stay dense and the capacity figures cover only what will move.
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    sparse_truncate_only(source.path(), "aaa_unknown.pdf", 7 << 30);
    sparse_truncate_only(source.path(), "mazebook.pdf", 3);
    let (header, rows) = planned(&opts(source.path(), dest.path()), &c);
    assert_eq!(header.required_bytes, 3);
    assert_eq!(rows[0].id, "r00001");
}

#[test]
fn an_unmapped_file_gets_no_destination_to_converge_on() {
    // The preset emits `_/_.pdf` for an empty series. Unmapped files never
    // reach the preset, so a run of them cannot converge on one path.
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(
        source.path(),
        &["totallyunknownthing.pdf", "anotherunknownthing.pdf", "mazebook.pdf"],
    );
    let (_h, rows) = planned(&opts(source.path(), dest.path()), &c);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].destination_preview.starts_with("Mazebook"));
}

#[test]
fn a_plan_at_the_old_version_is_refused() {
    let c = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(source.path(), &["mazebook.pdf"]);
    let (mut header, rows) = planned(&opts(source.path(), dest.path()), &c);
    header.version = comic_renamer_core::plan::PLAN_VERSION - 1;
    let out = tempdir().unwrap();
    let path = out.path().join("plan.jsonl");
    comic_renamer_core::plan::write(&header, &rows, &path).unwrap();
    assert!(matches!(
        comic_renamer_core::plan::read(&path),
        Err(comic_renamer_core::types::PlanError::UnsupportedVersion { .. })
    ));
}
