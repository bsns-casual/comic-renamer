use comic_renamer_core::preset::preset_by_id;
use comic_renamer_core::types::{Mechanism, Mode, Resolved, Status, UndoError};
use comic_renamer_core::undo::{reverse, summarise, UndoWriter, COLUMNS};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

fn dest_for() -> PathBuf {
    let resolved = Resolved {
        series: "Hellboy".into(),
        volume: Some(1),
        subtitle: None,
        run: 3,
        status: Status::Mapped,
        record: None,
    };
    preset_by_id("komga").unwrap().destination(&resolved, "pdf")
}

/// One file run through the writer. `same_root` points both roots at one
/// directory; `mv` records a same-volume rename instead of a verified copy.
fn written(mv: bool, same_root: bool) -> (TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let library = if same_root { source.clone() } else { dir.path().join("library") };
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&library).unwrap();
    let (source, library) = (source.canonicalize().unwrap(), library.canonicalize().unwrap());
    let file = source.join("Hellboy_v01.pdf");
    std::fs::write(&file, b"bytes").unwrap();

    let rel = dest_for();
    let record = dir.path().join("run.record");
    let mut w = UndoWriter::create(&record, &source, &library, "komga", 1).unwrap();
    let (mechanism, mode) = if mv { (Mechanism::Rename, Mode::Move) } else { (Mechanism::Clone, Mode::Copy) };
    let seq = w
        .record_pending(
            Path::new("Hellboy_v01.pdf"), &rel, mechanism, mode, &file,
            "Hellboy", Some(1), None, 3, "pdf",
        )
        .unwrap();
    let dest = library.join(&rel);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    if mv {
        std::fs::rename(&file, &dest).unwrap();
    } else {
        std::fs::write(&dest, b"bytes").unwrap();
        let m = std::fs::symlink_metadata(&dest).unwrap();
        w.record_placed(seq, (m.len(), m.ino(), m.mtime() * 1_000_000_000 + m.mtime_nsec())).unwrap();
    }
    w.record_done(seq).unwrap();
    (dir, record)
}

fn a_written_record() -> (TempDir, PathBuf) {
    written(false, false)
}

#[test]
fn a_record_alone_rebuilds_every_destination() {
    let (dir, record) = a_written_record();
    let s = summarise(&record).unwrap();
    assert_eq!((s.preset.as_str(), s.planned, s.reversible, s.interrupted), ("komga", 1, 1, false));
    let report = reverse(&record).unwrap();
    assert_eq!(report.refused, Vec::<String>::new());
    assert_eq!(report.restored, 1);
    assert!(!dir.path().join("library").join(dest_for()).exists());
}

/// Review Focus 3, both halves.
#[test]
fn a_wrong_header_is_refused_and_a_missing_one_is_not() {
    let (dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();

    let old = dir.path().join("old.record");
    std::fs::write(&old, "seq,status,mode,mechanism,source_name,dest_relative,size,inode,mtime_ns\n").unwrap();
    assert!(summarise(&old).unwrap_err().to_string().contains("older build"));

    let headerless = dir.path().join("bare.record");
    std::fs::write(&headerless, text.lines().skip(1).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    assert_eq!(summarise(&headerless).unwrap().planned, 1, "the roots row is not eaten as a header");
}

/// Review Focus 1.
#[test]
fn a_record_whose_roots_are_gone_refuses_every_row() {
    let (dir, record) = a_written_record();
    std::fs::remove_dir_all(dir.path().join("library")).unwrap();
    let report = reverse(&record).unwrap();
    assert_eq!(report.restored, 0);
    assert!(report.refused.iter().any(|r| r.contains("library")), "{:?}", report.refused);
}

/// The naming trap: a same-volume move records `done` and never `placed`.
#[test]
fn a_move_run_is_reversible_even_though_no_row_is_placed() {
    let (_dir, record) = written(true, false);
    let s = summarise(&record).unwrap();
    assert_eq!(s.reversible, 1, "a renamed row is reversible");
    assert!(!s.interrupted);
    let text = std::fs::read_to_string(&record).unwrap();
    assert!(!text.contains(",placed,"), "and it really has no placed row");
}

#[test]
fn every_status_row_has_the_full_column_count() {
    let (_dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    let want = COLUMNS.split(',').count();
    assert!(text.contains(",placed,") && text.contains(",done,"));
    for line in text.lines() {
        assert_eq!(line.split(',').count(), want, "{line}");
    }
}

#[test]
fn a_pending_row_with_no_terminal_status_is_interrupted() {
    let (_dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    let cut: Vec<_> = text.lines().filter(|l| !l.contains(",done,") && !l.contains(",placed,")).collect();
    std::fs::write(&record, cut.join("\n") + "\n").unwrap();
    let s = summarise(&record).unwrap();
    assert_eq!((s.reversible, s.interrupted), (0, true));
}

#[test]
fn an_edited_destination_is_not_a_destination_the_preset_produces() {
    let (_dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    std::fs::write(&record, text.replace("Hellboy/", "Elsewhere/")).unwrap();
    let report = reverse(&record).unwrap();
    assert_eq!(report.restored, 0);
    assert!(!report.refused.is_empty());
}

/// Equal roots are only one half of the check: `||` becomes `&&` and equal
/// roots still refuse, because both halves are true. Nesting tells them apart.
#[test]
fn nested_roots_are_refused_in_both_directions() {
    let (dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    let source = dir.path().join("source").canonicalize().unwrap();
    let library = dir.path().join("library").canonicalize().unwrap();

    // Library inside the bundle folder.
    let inner = source.join("library");
    std::fs::create_dir_all(&inner).unwrap();
    let a = dir.path().join("a.record");
    std::fs::write(&a, text.replace(&format!("'{}", library.display()), &format!("'{}", inner.display()))).unwrap();
    assert!(matches!(reverse(&a), Err(UndoError::RootsOverlap)));

    // Bundle folder inside the library.
    let inner = library.join("source");
    std::fs::create_dir_all(&inner).unwrap();
    let b = dir.path().join("b.record");
    std::fs::write(&b, text.replace(&format!("'{}", source.display()), &format!("'{}", inner.display()))).unwrap();
    assert!(matches!(reverse(&b), Err(UndoError::RootsOverlap)));

    let msg = UndoError::RootsOverlap.to_string();
    assert!(msg.contains("overlap") && !msg.contains("no destination root"), "{msg}");
}

#[test]
fn equal_roots_name_the_overlap_not_a_missing_root() {
    let (_dir, record) = written(false, true);
    assert!(matches!(reverse(&record), Err(UndoError::RootsOverlap)));
}

#[test]
fn started_falls_between_two_clock_readings() {
    let before = std::time::SystemTime::now();
    let (_dir, record) = a_written_record();
    let after = std::time::SystemTime::now();
    let started = summarise(&record).unwrap().started;
    assert!(before <= started && started <= after, "{before:?} {started:?} {after:?}");
}

/// Without the extend, a row the parser dropped leaves a file in the library
/// while the report says the undo was clean.
#[test]
fn an_unreadable_row_is_reported_by_reverse() {
    let (_dir, record) = a_written_record();
    let mut text = std::fs::read_to_string(&record).unwrap();
    text.push_str("not-a-number,done,,,,,,,,,,,,,,\n");
    std::fs::write(&record, text).unwrap();
    let report = reverse(&record).unwrap();
    assert_eq!(report.restored, 1);
    assert!(report.refused.iter().any(|r| r.contains("not a number")), "{:?}", report.refused);
}

/// A `pending` row short enough to parse but too short to rebuild anything
/// from. Nothing this build writes produces one — the writer always emits all
/// sixteen columns — but a record is a file a human can edit, which is what
/// invariant 1 exists for, and a row nothing can reverse must be *reported*
/// rather than quietly dropped: a dropped row leaves a file in the library
/// while the report says the undo was clean.
#[test]
fn a_pending_row_too_short_to_rebuild_is_reported_not_dropped() {
    let (_dir, record) = a_written_record();
    let mut text = std::fs::read_to_string(&record).unwrap();
    // Nine columns: enough for the length check and the identity, and three
    // short of the extension the preset needs.
    text.push_str("9,pending,move,rename,'f.pdf,'Hellboy/f.pdf,0,1,0\n");
    std::fs::write(&record, text).unwrap();

    let report = reverse(&record).unwrap();
    assert!(
        report
            .refused
            .iter()
            .any(|r| r.contains("carries no fields")),
        "the row was dropped in silence: {:?}",
        report.refused
    );
    // And the sibling row is untouched by it.
    assert_eq!(report.restored, 1, "{report:?}");
}

#[test]
fn an_extension_is_prefixed_like_every_other_text_field() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f.pdf");
    std::fs::write(&file, b"x").unwrap();
    let record = dir.path().join("r.record");
    let mut w = UndoWriter::create(&record, dir.path(), dir.path(), "komga", 1).unwrap();
    w.record_pending(Path::new("f.pdf"), Path::new("a/b"), Mechanism::Clone, Mode::Copy, &file, "S", None, None, 0, "=cmd").unwrap();
    let text = std::fs::read_to_string(&record).unwrap();
    assert!(text.contains(",'=cmd,"), "{text}");
}
