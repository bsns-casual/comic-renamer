use comic_renamer_core::preset::preset_by_id;
use comic_renamer_core::types::{Mechanism, Mode, Resolved, Status, UndoError};
use comic_renamer_core::undo::{reverse_from, summarise, UndoWriter, COLUMNS};
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
    let mut w = UndoWriter::create_with(&record, &source, &library, "komga", 1).unwrap();
    let (mechanism, mode) = if mv { (Mechanism::Rename, Mode::Move) } else { (Mechanism::Clone, Mode::Copy) };
    let seq = w
        .record_pending_with(
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
    let report = reverse_from(&record).unwrap();
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
    let report = reverse_from(&record).unwrap();
    assert_eq!(report.restored, 0);
    assert!(report.refused.iter().any(|r| r.contains("library")), "{:?}", report.refused);
}

/// Review Focus 5.
#[test]
fn a_record_whose_roots_canonicalise_equal_is_refused() {
    let (_dir, record) = written(false, true);
    assert!(matches!(reverse_from(&record), Err(UndoError::NoRoot) | Err(UndoError::Parse { .. })));
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
    let report = reverse_from(&record).unwrap();
    assert_eq!(report.restored, 0);
    assert!(!report.refused.is_empty());
}
