//! Task 10. `scan` is read-only by construction: it stats, it never opens.
//! The fixtures are zero-byte files with real names (invariant 3).

mod common;

use comic_renamer_core::scan::scan;
use comic_renamer_core::types::{Mode, ScanError, ScanOpts, Status};
use common::{corpus, sparse_truncate_only, tree_from_names, TreeGuard};
use comic_renamer_core::corpus::Corpus;
use comic_renamer_core::overrides::Overrides;
use std::path::Path;
use tempfile::{tempdir, TempDir};

fn gcd() -> (TempDir, Corpus) {
    corpus(&[
        ("Mazebook", "mazebook", "Dark Horse", "en", 2021, 5),
        ("Hellboy Omnibus", "hellboyomnibus", "Dark Horse", "en", 2018, 4),
        ("The Mask Omnibus", "maskomnibus", "Dark Horse", "en", 2008, 2),
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

fn run(o: &ScanOpts, c: &Corpus) -> Result<(comic_renamer_core::types::PlanHeader, Vec<comic_renamer_core::types::PlanRow>), ScanError> {
    scan(o, c, &Overrides::default(), &mut |_| {})
}

#[test]
fn refuses_overlapping_roots_in_both_directions() {
    let (_g, c) = gcd();
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
    let (_g, c) = gcd();
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

    let (_h, rows) = run(&opts(source.path(), dest.path()), &c).unwrap();
    assert_eq!(rows.len(), 1, "kept {:?}", rows.iter().map(|r| &r.source).collect::<Vec<_>>());
    assert_eq!(rows[0].series, "Mazebook");
}

#[test]
fn scan_leaves_both_trees_byte_identical() {
    let (_g, c) = gcd();
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
    let (_g, c) = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    sparse_truncate_only(source.path(), "mazebook.pdf", 5 << 30);

    let (header, rows) = run(&opts(source.path(), dest.path()), &c).unwrap();
    assert_eq!(header.required_bytes, 5 << 30);
    assert_eq!(rows[0].source_stat.size, 5 << 30);
    // Nothing here is a cloud placeholder, so the download cost is nil and
    // must be reported apart from the disk cost (D23).
    assert_eq!(header.download_bytes, 0);
}

#[test]
fn a_zero_byte_source_is_a_row_not_a_fault() {
    // Invariant 3. An "empty means corrupt" guard would disable the suite.
    let (_g, c) = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(source.path(), &["mazebook.pdf"]);
    let (header, rows) = run(&opts(source.path(), dest.path()), &c).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(header.required_bytes, 0);
}

#[test]
fn uses_the_source_folder_name_as_the_publisher_hint() {
    // §6.3: the hint comes from the source folder's own name.
    let (_g, c) = gcd();
    let dest = tempdir().unwrap();
    let parent = tempdir().unwrap();
    let source = parent.path().join("HumbleDarkHorseAnniversaryBundle");
    std::fs::create_dir(&source).unwrap();
    tree_from_names(&source, &["maskomnibusvolume1secondedition.pdf"]);

    let (_h, rows) = run(&opts(&source, dest.path()), &c).unwrap();
    assert_eq!(rows[0].series, "The Mask Omnibus");
    assert_eq!(rows[0].record.as_ref().unwrap().publisher, "Dark Horse");
}

#[test]
fn multi_format_duplicates_become_separate_rows() {
    let (_g, c) = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(
        source.path(),
        &["mazebook.pdf", "mazebook.cbz", "mazebook.epub"],
    );
    let (_h, rows) = run(&opts(source.path(), dest.path()), &c).unwrap();
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
    let (_g, c) = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(source.path(), &["mazebook.pdf", "hellboyomnibus_vol1.pdf"]);
    let (header, rows) = run(&opts(source.path(), dest.path()), &c).unwrap();

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
    let (_g, c) = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(source.path(), &["mazebook.pdf"]);
    let (header, _) = run(&opts(source.path(), dest.path()), &c).unwrap();
    assert_eq!(header.mode, Mode::Move);
}

#[test]
fn refuses_an_unknown_preset_and_a_source_that_is_not_a_directory() {
    let (_g, c) = gcd();
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
fn an_unresolved_file_is_still_a_row() {
    // A failed row must reach the plan so the user can see it, rather than
    // vanishing the way a skipped non-comic does.
    let (_g, c) = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(source.path(), &["totallyunknownthing.pdf"]);
    let (_h, rows) = run(&opts(source.path(), dest.path()), &c).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, Status::Failed);
}

#[test]
fn a_row_that_resolved_to_nothing_gets_no_destination() {
    // The preset emits `_/_.pdf` for an empty series, so every unresolved row
    // in a run converged onto one path — and the plan showed the user a
    // destination for a file that resolved to nothing.
    let (_g, c) = gcd();
    let source = tempdir().unwrap();
    let dest = tempdir().unwrap();
    tree_from_names(
        source.path(),
        &["totallyunknownthing.pdf", "anotherunknownthing.pdf", "mazebook.pdf"],
    );
    let (_h, rows) = run(&opts(source.path(), dest.path()), &c).unwrap();

    let unresolved: Vec<_> = rows.iter().filter(|r| r.series.is_empty()).collect();
    assert_eq!(unresolved.len(), 2);
    for row in unresolved {
        assert_eq!(row.destination_preview, Path::new(""), "{:?}", row.source);
    }
    let resolved = rows.iter().find(|r| !r.series.is_empty()).unwrap();
    assert!(resolved.destination_preview.starts_with("Mazebook"));
}
