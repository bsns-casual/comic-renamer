//! Task 12. Execution and undo. This is where `TreeGuard` stops being
//! decorative: every refusal test here would pass on an `Err` alone while
//! files had already moved.

mod common;

use comic_renamer_core::apply::{apply, undo, undo_path};
use comic_renamer_core::mapping::Mappings;
use comic_renamer_core::types::{Mechanism, Mode, ScanOpts};
use comic_renamer_core::undo::{reverse, UndoWriter};


use comic_renamer_core::{plan, scan};
use common::{mappings, TreeGuard};
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

fn gcd() -> Mappings {
    mappings(&[
        ("mazebook", "Mazebook", None, Some(5)),
        ("birdking_vol1", "Birdking", Some(1), Some(3)),
        ("hellboyomnibus_vol1", "Hellboy Omnibus", Some(1), Some(4)),
    ])
}

struct Fixture {
    _source: TempDir,
    _dest: TempDir,
    _plans: TempDir,
    source: PathBuf,
    dest: PathBuf,
    plan: PathBuf,
}

impl Fixture {
    fn new(names: &[&str], corpus: &Mappings, mode: Mode) -> Fixture {
        let source = tempdir().unwrap();
        let dest = tempdir().unwrap();
        let plans = tempdir().unwrap();
        for name in names {
            // Real content, so a stream can be verified byte for byte.
            std::fs::write(source.path().join(name), format!("contents of {name}")).unwrap();
        }
        let opts = ScanOpts {
            source_root: source.path().to_path_buf(),
            dest_root: dest.path().to_path_buf(),
            preset: "komga".into(),
            mode: Some(mode),
            lang: "en".into(),
        };
        let comic_renamer_core::scan::ScanOutcome { header, rows, .. } =
            scan::scan(&opts, corpus, &mut |_| {}).unwrap();
        let plan_path = plans.path().join("plan.jsonl");
        plan::write(&header, &rows, &plan_path).unwrap();
        Fixture {
            source: source.path().to_path_buf(),
            dest: dest.path().to_path_buf(),
            plan: plan_path,
            _source: source,
            _dest: dest,
            _plans: plans,
        }
    }

    fn manifest(&self) -> PathBuf {
        undo_path(&self.plan)
    }

    /// Rewrites the plan so every row uses `mechanism`, re-stamping the
    /// checksum. Only useful for proving the pre-flight refuses a plan that
    /// claims a mechanism the volume cannot give — the streaming path itself
    /// needs a real second volume.
    fn force_mechanism(&self, mechanism: Mechanism) {
        let (header, mut rows) = plan::read(&self.plan).unwrap();
        for row in &mut rows {
            row.mechanism = mechanism;
        }
        plan::write(&header, &rows, &self.plan).unwrap();
    }

    fn files_under(&self, root: &Path) -> Vec<String> {
        let mut found = Vec::new();
        collect(root, &mut found);
        found.sort();
        found
    }
}

fn collect(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
}

// ---------------------------------------------------------------------------
// The undo manifest
// ---------------------------------------------------------------------------

/// A manifest-level fixture: a copy of one file already sitting in `dest`,
/// recorded as this run having put it there.
struct Recorded {
    _dir: TempDir,
    manifest: PathBuf,
    source_root: PathBuf,
    dest_root: PathBuf,
    relative: PathBuf,
}

impl Recorded {
    fn new(name: &str, body: &[u8], mode: Mode, keep_source: bool) -> Recorded {
        let dir = tempdir().unwrap();
        let source_root = dir.path().join("src");
        let dest_root = dir.path().join("dest");
        std::fs::create_dir_all(&source_root).unwrap();
        std::fs::create_dir_all(&dest_root).unwrap();
        let source = source_root.join(name);
        std::fs::write(&source, body).unwrap();
        let relative = PathBuf::from(name);
        let destination = dest_root.join(&relative);
        std::fs::write(&destination, body).unwrap();

        let manifest = dir.path().join("plan.jsonl.undo.csv");
        let mut writer = UndoWriter::create(&manifest, &source_root, &dest_root).unwrap();
        let seq = writer
            .record_pending(Path::new(name), &relative, Mechanism::Clone, mode, &source)
            .unwrap();
        let meta = std::fs::symlink_metadata(&destination).unwrap();
        writer
            .record_placed(
                seq,
                (
                    meta.len(),
                    MetadataExt::ino(&meta),
                    MetadataExt::mtime(&meta) * 1_000_000_000 + MetadataExt::mtime_nsec(&meta),
                ),
            )
            .unwrap();
        writer.record_done(seq).unwrap();
        drop(writer);

        if !keep_source {
            std::fs::remove_file(&source).unwrap();
        }
        Recorded {
            _dir: dir,
            manifest,
            source_root,
            dest_root,
            relative,
        }
    }

    fn reverse(&self) -> Result<comic_renamer_core::types::UndoReport, comic_renamer_core::types::UndoError> {
        let recomputed = vec![self.relative.clone()];
        reverse(&self.manifest, &self.source_root, &self.dest_root, &recomputed)
    }

    fn destination(&self) -> PathBuf {
        self.dest_root.join(&self.relative)
    }
}

#[test]
fn create_refuses_an_existing_manifest() {
    let d = tempdir().unwrap();
    let path = d.path().join("run.undo.csv");
    let _first = UndoWriter::create(&path, d.path(), d.path()).expect("first create");
    let second = UndoWriter::create(&path, d.path(), d.path());
    assert_eq!(
        second.err().map(|e| e.kind()),
        Some(ErrorKind::AlreadyExists),
        "a re-run would truncate the only record of where the last run's files went"
    );
}

#[test]
fn pending_is_on_disk_before_the_operation() {
    let d = tempdir().unwrap();
    let source = d.path().join("a.pdf");
    std::fs::write(&source, b"x").unwrap();
    let path = d.path().join("run.undo.csv");
    let mut writer = UndoWriter::create(&path, d.path(), d.path()).unwrap();
    writer
        .record_pending(
            Path::new("a.pdf"),
            Path::new("A/a.pdf"),
            Mechanism::Rename,
            Mode::Move,
            &source,
        )
        .unwrap();
    // Read back cold, not after a flush the test controlled.
    assert!(std::fs::read_to_string(&path).unwrap().contains("pending"));
}

#[test]
fn awkward_filenames_survive_the_manifest_round_trip() {
    // The GCD lists *Hellboy, the Corpse and the Iron Shoes*; newlines are
    // legal on APFS. The writer quoted both correctly all along — the reader
    // used `str::lines`, which splits inside a quoted field.
    for name in [
        "Hellboy, the Corpse and the \"Iron\" Shoes.pdf",
        "two\nlines.pdf",
    ] {
        let r = Recorded::new(name, b"copied here by the tool", Mode::Copy, true);
        let report = r.reverse().unwrap();
        assert_eq!(report.restored, 1, "{name:?} did not round-trip: {report:?}");
        assert!(!r.destination().exists());
    }
}

#[test]
fn reverse_refuses_a_destination_it_did_not_create() {
    // The manifest used to record no identity at all, so reverse deleted or
    // relocated whatever sat at the path.
    let r = Recorded::new("Mazebook.pdf", b"what the tool copied", Mode::Copy, true);
    std::fs::write(r.destination(), b"THE GOOD SCAN, much larger").unwrap();
    let report = r.reverse().unwrap();
    assert_eq!(report.restored, 0);
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert!(r.destination().exists(), "reverse deleted a file it never created");
}

#[test]
fn reverse_refuses_when_only_size_and_inode_match() {
    // (size, inode) survive an in-place edit, so identity carries mtime_ns
    // too — otherwise reverse deletes the only *corrected* copy.
    let r = Recorded::new("Mazebook.pdf", b"original", Mode::Copy, true);
    let destination = r.destination();
    // Rewrite in place: same length, same inode, new mtime.
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).open(&destination).unwrap();
    file.write_all(b"EDITEDxx").unwrap();
    file.sync_all().unwrap();
    drop(file);

    let report = r.reverse().unwrap();
    assert_eq!(report.restored, 0, "{report:?}");
    assert!(destination.exists(), "an in-place edit was mistaken for our own file");
}

#[test]
fn reverse_of_a_copy_refuses_when_the_original_is_gone() {
    // Invariant 6, and the plainest data-loss path in the project: copy a
    // bundle in, delete the bundle folder, undo. The old version deleted the
    // last copy and reported success.
    let r = Recorded::new("Mazebook.pdf", b"the only copy", Mode::Copy, false);
    let report = r.reverse().unwrap();
    assert_eq!(report.restored, 0);
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert!(
        r.destination().exists(),
        "reverse destroyed the last copy of a file"
    );
}

#[test]
fn reverse_refuses_a_destination_the_preset_does_not_produce() {
    // Invariant 9: a hand-written manifest cannot name an arbitrary path.
    let r = Recorded::new("Mazebook.pdf", b"x", Mode::Copy, true);
    let report = reverse(
        &r.manifest,
        &r.source_root,
        &r.dest_root,
        // The plan produces something else entirely.
        &[PathBuf::from("Something/Else.pdf")],
    )
    .unwrap();
    assert_eq!(report.restored, 0);
    assert!(r.destination().exists());
}

#[test]
fn reverse_refuses_a_manifest_whose_roots_disagree_with_the_plan() {
    let r = Recorded::new("Mazebook.pdf", b"x", Mode::Copy, true);
    let elsewhere = tempdir().unwrap();
    assert!(matches!(
        reverse(
            &r.manifest,
            elsewhere.path(),
            &r.dest_root,
            &[r.relative.clone()]
        ),
        Err(comic_renamer_core::types::UndoError::NoRoot)
    ));
    assert!(r.destination().exists());
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

#[test]
fn a_move_relocates_and_leaves_nothing_behind() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    let r = apply(&f.plan, false, &mut |_| {}).unwrap();
    assert_eq!(r.executed, 1);
    assert!(!f.source.join("mazebook.pdf").exists());
    assert_eq!(f.files_under(&f.dest), ["Mazebook.pdf"]);
    assert_eq!(
        std::fs::read_to_string(f.dest.join("Mazebook/Mazebook.pdf")).unwrap(),
        "contents of mazebook.pdf"
    );
}

#[test]
fn a_copy_keeps_the_original() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Copy);
    apply(&f.plan, false, &mut |_| {}).unwrap();
    assert!(f.source.join("mazebook.pdf").exists(), "a copy must keep it");
    assert_eq!(f.files_under(&f.dest), ["Mazebook.pdf"]);
}

#[test]
fn a_plan_claiming_a_mechanism_the_volume_cannot_give_is_refused() {
    // The pre-flight re-derives the mechanism and never silently falls back.
    // This is also why the streamed path cannot be forced by editing a plan.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    f.force_mechanism(Mechanism::Stream);
    let guard = TreeGuard::new(&[f.source.as_path(), f.dest.as_path()]);
    assert!(matches!(
        apply(&f.plan, false, &mut |_| {}),
        Err(comic_renamer_core::types::ApplyError::MechanismChanged(_))
    ));
    drop(guard);
}

/// The streamed move — copy, verify, rename into place, **then** unlink —
/// needs a genuine second volume, because same-volume moves are renames and
/// the pre-flight refuses a plan that claims otherwise.
///
/// Point `COMIC_RENAMER_TEST_VOLUME` at a writable mount on another device
/// and run with `--ignored`. Creating a volume from inside a test is a side
/// effect on the machine that a test suite has no business having.
#[test]
#[ignore]
fn a_streamed_move_verifies_before_it_unlinks() {
    let Ok(volume) = std::env::var("COMIC_RENAMER_TEST_VOLUME") else {
        panic!("set COMIC_RENAMER_TEST_VOLUME to a mount on another device");
    };
    let c = gcd();

    let source = tempdir().unwrap();
    std::fs::write(source.path().join("mazebook.pdf"), b"contents of mazebook.pdf").unwrap();
    let dest = PathBuf::from(&volume).join("comic-renamer-exec-test");
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest).unwrap();
    let plans = tempdir().unwrap();

    let opts = ScanOpts {
        source_root: source.path().to_path_buf(),
        dest_root: dest.clone(),
        preset: "komga".into(),
        mode: Some(Mode::Move),
        lang: "en".into(),
    };
    let comic_renamer_core::scan::ScanOutcome { header, rows, .. } =
            scan::scan(&opts, &c, &mut |_| {}).unwrap();
    assert_eq!(rows[0].mechanism, Mechanism::Stream, "not actually cross-volume");
    let plan_path = plans.path().join("plan.jsonl");
    plan::write(&header, &rows, &plan_path).unwrap();

    apply(&plan_path, false, &mut |_| {}).unwrap();
    assert!(!source.path().join("mazebook.pdf").exists(), "the source survived a move");
    assert_eq!(
        std::fs::read_to_string(dest.join("Mazebook/Mazebook.pdf")).unwrap(),
        "contents of mazebook.pdf",
        "the copy was unlinked from without being verified"
    );
    // No temp left behind.
    let leftovers: Vec<_> = std::fs::read_dir(dest.join("Mazebook"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".partial"))
        .collect();
    assert!(leftovers.is_empty(), "a .partial survived");

    // This is the only test that reaches the third unlink in `reverse_one`:
    // the source is gone, so `rename_into` fails `EXDEV`, the file is streamed
    // back, and the duplicate left at the destination is removed. Asserting
    // only that the source came back leaves that removal unobserved — the
    // restore reads as clean with the library copy still sitting there.
    let placed = dest.join("Mazebook/Mazebook.pdf");
    let report = undo(&plan_path).unwrap();
    assert!(source.path().join("mazebook.pdf").exists(), "undo did not restore it");
    assert_eq!(report.restored, 1, "{report:?}");
    assert!(report.refused.is_empty(), "{report:?}");
    assert!(!placed.exists(), "the duplicate at the destination was left behind");
    let _ = std::fs::remove_dir_all(&dest);
}

/// `stream_verified` opens the library file it is restoring *from*, and
/// `undo` got there on an `Entry::identity` match — an `fstatat`, which says
/// nothing about file type. So a record naming a FIFO at a destination the
/// preset produces reaches that open, and reaches it only across volumes: a
/// same-device `rename_into` succeeds and never streams.
///
/// Nothing is planted with privilege here. The record is honest about a FIFO
/// anyone can create in their own library folder, and the source it claims to
/// restore to is simply absent, which is the case `undo` is built for.
///
/// Point `COMIC_RENAMER_TEST_VOLUME` at a mount on another device.
#[test]
#[ignore]
fn a_fifo_at_a_cross_volume_destination_is_refused_rather_than_streamed() {
    let Ok(volume) = std::env::var("COMIC_RENAMER_TEST_VOLUME") else {
        panic!("set COMIC_RENAMER_TEST_VOLUME to a mount on another device");
    };

    let source = tempdir().unwrap();
    let library = PathBuf::from(&volume).join("comic-renamer-fifo");
    let _ = std::fs::remove_dir_all(&library);
    let resolved = comic_renamer_core::types::Resolved {
        series: "Mazebook".into(),
        volume: None,
        subtitle: None,
        run: 5,
        status: comic_renamer_core::types::Status::Mapped,
        record: None,
    };
    let relative = comic_renamer_core::preset::preset_by_id("komga")
        .unwrap()
        .destination(&resolved, "pdf");
    std::fs::create_dir_all(library.join(&relative).parent().unwrap()).unwrap();

    let fifo = library.join(&relative);
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path for the duration of the call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);

    let record = source.path().join("run.record");
    let mut w = UndoWriter::create_with(&record, source.path(), &library, "komga", 1).unwrap();
    let seq = w
        .record_pending_with(
            Path::new("mazebook.pdf"),
            &relative,
            Mechanism::Stream,
            Mode::Move,
            &fifo,
            "Mazebook",
            None,
            None,
            5,
            "pdf",
        )
        .unwrap();
    let meta = std::fs::symlink_metadata(&fifo).unwrap();
    w.record_placed(
        seq,
        (meta.len(), meta.ino(), meta.mtime() * 1_000_000_000 + meta.mtime_nsec()),
    )
    .unwrap();
    w.record_done(seq).unwrap();
    drop(w);

    // Bounded: without `O_NONBLOCK` this open waits for a writer forever, and
    // the library's lock is held while it waits.
    let report = common::reverse_before(&record, common::DEADLINE);
    assert_eq!(report.restored, 0, "{report:?}");
    assert!(
        report.refused.iter().any(|r| r.contains("not a regular file")),
        "refused for the wrong reason, which means the open was not what stopped it: {report:?}"
    );
    assert!(!source.path().join("mazebook.pdf").exists(), "a FIFO was streamed into the bundle");
    let _ = std::fs::remove_dir_all(&library);
}

/// Sets a file's mtime without touching its contents, so its *identity*
/// changes and its hash does not.
fn bump_mtime(path: &Path) {
    let meta = std::fs::symlink_metadata(path).unwrap();
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let times = [
        libc::timespec { tv_sec: meta.atime(), tv_nsec: meta.atime_nsec() as libc::c_long },
        libc::timespec { tv_sec: meta.mtime() + 1, tv_nsec: 0 },
    ];
    // SAFETY: a valid NUL-terminated path and a two-element times array for
    // the duration of the call.
    let rc = unsafe {
        libc::utimensat(libc::AT_FDCWD, name.as_ptr(), times.as_ptr(), libc::AT_SYMLINK_NOFOLLOW)
    };
    assert_eq!(rc, 0, "utimensat: {}", std::io::Error::last_os_error());
}

/// The destination changes *between* the identity check at the top of
/// `reverse_one` and the unlink at the bottom, which only a cross-volume
/// reverse can do: the stream back to the source is the gap.
///
/// `reverse_one` re-reads the destination's identity through the held
/// descriptor before that unlink, and nothing pinned it — deleting the
/// re-check left every test in this repository green. Here the mtime is bumped
/// while the stream-back runs. The contents are untouched, so
/// `stream_verified`'s own hash comparison still passes and the re-check is
/// the only thing that can refuse.
///
/// Cannot be folded into `a_streamed_move_verifies_before_it_unlinks`: that
/// test asserts the reverse succeeds and this one asserts it refuses.
///
/// Point `COMIC_RENAMER_TEST_VOLUME` at a mount on another device.
#[test]
#[ignore]
fn a_destination_that_changes_during_the_stream_back_is_not_unlinked() {
    let Ok(volume) = std::env::var("COMIC_RENAMER_TEST_VOLUME") else {
        panic!("set COMIC_RENAMER_TEST_VOLUME to a mount on another device");
    };
    let c = gcd();

    let source = tempdir().unwrap();
    // Large enough that the copy and the two hashes of the stream-back take
    // tens of milliseconds, which is the window the watcher below needs. A
    // few bytes would make this a coin toss.
    std::fs::write(source.path().join("mazebook.pdf"), vec![b'm'; 16 << 20]).unwrap();
    let dest = PathBuf::from(&volume).join("comic-renamer-perturb");
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest).unwrap();
    let plans = tempdir().unwrap();

    let opts = ScanOpts {
        source_root: source.path().to_path_buf(),
        dest_root: dest.clone(),
        preset: "komga".into(),
        mode: Some(Mode::Move),
        lang: "en".into(),
    };
    let comic_renamer_core::scan::ScanOutcome { header, rows, .. } =
        scan::scan(&opts, &c, &mut |_| {}).unwrap();
    assert_eq!(rows[0].mechanism, Mechanism::Stream, "not actually cross-volume");
    let plan_path = plans.path().join("plan.jsonl");
    plan::write(&header, &rows, &plan_path).unwrap();
    apply(&plan_path, false, &mut |_| {}).unwrap();

    let placed = dest.join("Mazebook/Mazebook.pdf");
    let placed_before = std::fs::symlink_metadata(&placed).unwrap().mtime();

    // `stream_verified` writes its temp into the directory it is restoring
    // into, so a `.partial` appearing in the bundle folder means the
    // stream-back has started and the unlink has not happened yet. The
    // watcher has its own deadline: a miss fails this test rather than
    // spinning on a `read_dir` forever.
    let watch = source.path().to_path_buf();
    let target = placed.clone();
    let watcher = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let started = std::fs::read_dir(&watch).into_iter().flatten().flatten().any(|e| {
                e.file_name().to_string_lossy().contains(".partial")
            });
            if started {
                bump_mtime(&target);
                return true;
            }
        }
        false
    });

    let report = undo(&plan_path).unwrap();
    assert!(
        watcher.join().unwrap(),
        "the watcher never saw a .partial, so the stream-back never ran and this \
         test proved nothing"
    );
    assert_ne!(
        std::fs::symlink_metadata(&placed)
            .map(|m| m.mtime())
            .unwrap_or(placed_before),
        placed_before,
        "the mtime bump did not land, so the re-check had nothing to see"
    );
    assert_eq!(report.restored, 0, "{report:?}");
    assert!(
        report.refused.iter().any(|r| r.contains("changed while it was being restored")),
        "{report:?}"
    );
    assert!(placed.exists(), "a destination that changed under the restore was unlinked");
    assert!(
        source.path().join("mazebook.pdf").exists(),
        "the stream-back's copy was discarded, so the file now exists nowhere it is named"
    );
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn an_occupied_destination_is_refused_not_overwritten() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    std::fs::create_dir(f.dest.join("Mazebook")).unwrap();
    std::fs::write(f.dest.join("Mazebook/Mazebook.pdf"), b"ALREADY HERE").unwrap();

    let guard = TreeGuard::new(&[f.source.as_path(), f.dest.as_path()]);
    let r = apply(&f.plan, false, &mut |_| {}).unwrap();
    assert_eq!((r.executed, r.skipped), (0, 1));
    assert_eq!(
        std::fs::read_to_string(f.dest.join("Mazebook/Mazebook.pdf")).unwrap(),
        "ALREADY HERE"
    );
    drop(guard);
}

// ---------------------------------------------------------------------------
// Reverse
// ---------------------------------------------------------------------------

#[test]
fn scan_apply_undo_restores_the_tree() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf", "birdking_vol1.pdf"], &c, Mode::Move);
    let before = f.files_under(&f.source);
    apply(&f.plan, false, &mut |_| {}).unwrap();
    assert!(f.files_under(&f.source).is_empty());

    undo(&f.plan).unwrap();
    assert_eq!(f.files_under(&f.source), before);
    assert!(f.files_under(&f.dest).is_empty(), "the destination still holds files");
}

#[test]
fn reverse_of_a_copy_deletes_the_destination_only() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Copy);
    apply(&f.plan, false, &mut |_| {}).unwrap();

    // The source must be untouched throughout: getting copy and move the
    // wrong way round here clobbers a kept original.
    let guard = TreeGuard::new(&[f.source.as_path()]);
    undo(&f.plan).unwrap();
    assert!(f.files_under(&f.dest).is_empty());
    drop(guard);
}

#[test]
fn reverse_is_idempotent_and_refuses_a_reoccupied_path() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();

    assert_eq!(undo(&f.plan).unwrap().restored, 1);
    // Idempotent: a complete reverse renames the manifest aside, so a second
    // pass has nothing to do rather than failing.
    let second = undo(&f.plan).unwrap();
    assert_eq!((second.restored, second.refused.len()), (0, 0), "{second:?}");
    assert_eq!(f.files_under(&f.source), ["mazebook.pdf"]);

    // And the plan is runnable again, which it was not when a successful
    // undo left the manifest in place for O_EXCL to trip over.
    assert_eq!(apply(&f.plan, false, &mut |_| {}).unwrap().executed, 1);
}

#[test]
fn reverse_refuses_to_overwrite_a_squatter() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();
    std::fs::write(f.source.join("mazebook.pdf"), b"squatter").unwrap();

    // Per-row now, not a whole-manifest error: one bad row must not strand
    // every other row in the file.
    let report = undo(&f.plan).unwrap();
    assert_eq!(report.restored, 0);
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert_eq!(
        std::fs::read_to_string(f.source.join("mazebook.pdf")).unwrap(),
        "squatter",
        "reverse overwrote something it did not put there"
    );
}

#[test]
fn a_re_run_cannot_truncate_the_previous_manifest() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();
    let first = std::fs::read_to_string(f.manifest()).unwrap();

    assert!(apply(&f.plan, false, &mut |_| {}).is_err());
    assert_eq!(
        std::fs::read_to_string(f.manifest()).unwrap(),
        first,
        "the only record of where the last run's files went was overwritten"
    );
}

/// The test the plan lists as `an_interrupted_apply_reverses_the_completed_rows`
/// and that three implementations went without. Needs a real second volume:
/// a same-volume move is a rename, which has no `placed` state, so the
/// duplicate this pins cannot occur there.
///
/// Point `COMIC_RENAMER_TEST_VOLUME` at a mount on another device.
#[test]
#[ignore]
fn an_interrupted_apply_reverses_the_completed_rows() {
    let Ok(volume) = std::env::var("COMIC_RENAMER_TEST_VOLUME") else {
        panic!("set COMIC_RENAMER_TEST_VOLUME to a mount on another device");
    };
    let c = gcd();

    let source = tempdir().unwrap();
    for name in ["mazebook.pdf", "birdking_vol1.pdf"] {
        std::fs::write(source.path().join(name), format!("contents of {name}")).unwrap();
    }
    let dest = PathBuf::from(&volume).join("comic-renamer-interrupt");
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest).unwrap();
    let plans = tempdir().unwrap();

    let opts = ScanOpts {
        source_root: source.path().to_path_buf(),
        dest_root: dest.clone(),
        preset: "komga".into(),
        mode: Some(Mode::Move),
        lang: "en".into(),
    };
    let comic_renamer_core::scan::ScanOutcome { header, rows, .. } =
            scan::scan(&opts, &c, &mut |_| {}).unwrap();
    assert_eq!(rows[0].mechanism, Mechanism::Stream, "not actually cross-volume");
    let plan_path = plans.path().join("plan.jsonl");
    plan::write(&header, &rows, &plan_path).unwrap();

    // Make the source directory read-only so the first row's copy verifies
    // and lands, and then its unlink fails. That is the `placed` state: two
    // copies on disk, which no earlier implementation could clear.
    let mut locked = std::fs::metadata(source.path()).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut locked, 0o500);
    std::fs::set_permissions(source.path(), locked).unwrap();

    let result = apply(&plan_path, false, &mut |_| {});

    let mut restore = std::fs::metadata(source.path()).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut restore, 0o700);
    std::fs::set_permissions(source.path(), restore).unwrap();

    assert!(result.is_err(), "the failed unlink should have aborted the run");
    // Both sources survive; one of them now has a duplicate in the library.
    assert_eq!(
        std::fs::read_dir(source.path()).unwrap().count(),
        2,
        "a source was consumed despite the abort"
    );

    let report = undo(&plan_path).unwrap();
    assert!(report.refused.is_empty(), "{report:?}");
    assert_eq!(
        std::fs::read_dir(source.path()).unwrap().count(),
        2,
        "undo disturbed the sources"
    );
    // The duplicate is gone and the library is empty again: rows_still_moved == 0.
    let left: Vec<_> = walkdir(&dest);
    assert!(left.is_empty(), "rows still moved: {left:?}");

    let _ = std::fs::remove_dir_all(&dest);
}

fn walkdir(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    collect(root, &mut found);
    found
}

// ---------------------------------------------------------------------------
// The final review's five findings, each reproduced
// ---------------------------------------------------------------------------

#[test]
fn a_placed_move_checks_the_source_before_deleting_the_copy() {
    // A1. The Copy arm checked the source's identity; the Move arm did not.
    // Re-extract the bundle, or drop in a truncated file, and `reverse`
    // deleted the verified library copy and reported success. Invariant 6.
    let d = tempdir().unwrap();
    let source_root = d.path().join("src");
    let dest_root = d.path().join("dest");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::create_dir_all(&dest_root).unwrap();
    let good = b"the verified library copy, intact";
    let destination = dest_root.join("Mazebook.pdf");
    std::fs::write(&destination, good).unwrap();
    let source = source_root.join("mazebook.pdf");
    std::fs::write(&source, good).unwrap();

    let manifest = d.path().join("plan.jsonl.undo.csv");
    let mut writer = UndoWriter::create(&manifest, &source_root, &dest_root).unwrap();
    let seq = writer
        .record_pending(
            Path::new("mazebook.pdf"),
            Path::new("Mazebook.pdf"),
            Mechanism::Stream,
            Mode::Move,
            &source,
        )
        .unwrap();
    let meta = std::fs::symlink_metadata(&destination).unwrap();
    writer
        .record_placed(
            seq,
            (
                meta.len(),
                meta.ino(),
                meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
            ),
        )
        .unwrap();
    drop(writer);

    // The user puts something else back at the source name.
    std::fs::write(&source, b"junk").unwrap();

    let report = reverse(
        &manifest,
        &source_root,
        &dest_root,
        &[PathBuf::from("Mazebook.pdf")],
    )
    .unwrap();
    assert_eq!(report.restored, 0, "{report:?}");
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        good,
        "reverse deleted the only good copy"
    );
}

#[test]
fn reverse_does_not_hang_on_a_fifo_at_a_destination() {
    // A2. Entry::metadata used openat, which blocks forever opening a FIFO —
    // while holding the plan lock, so the one recovery path was unavailable.
    // fstatat answers instantly and never opens the file.
    let d = tempdir().unwrap();
    let source_root = d.path().join("src");
    let dest_root = d.path().join("dest");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::create_dir_all(&dest_root).unwrap();
    let source = source_root.join("mazebook.pdf");
    std::fs::write(&source, b"x").unwrap();

    let fifo = dest_root.join("Mazebook.pdf");
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path for the duration of the call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);

    let manifest = d.path().join("plan.jsonl.undo.csv");
    let mut writer = UndoWriter::create(&manifest, &source_root, &dest_root).unwrap();
    writer
        .record_pending(
            Path::new("mazebook.pdf"),
            Path::new("Mazebook.pdf"),
            Mechanism::Stream,
            Mode::Move,
            &source,
        )
        .unwrap();
    drop(writer);

    // The assertion is that this returns at all.
    let report = reverse(
        &manifest,
        &source_root,
        &dest_root,
        &[PathBuf::from("Mazebook.pdf")],
    )
    .unwrap();
    assert_eq!(report.restored, 0, "{report:?}");
    assert!(fifo.exists());
}

#[test]
fn an_unreadable_destination_is_refused_not_mistaken_for_absent() {
    // A2's sibling: openat failed on a file the user cannot read, so the row
    // was refused for the wrong reason. fstatat answers through the parent.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();
    let book = f.dest.join("Mazebook/Mazebook.pdf");
    let mut locked = std::fs::metadata(&book).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut locked, 0o000);
    std::fs::set_permissions(&book, locked).unwrap();

    let report = undo(&f.plan).unwrap();
    // The file itself is unreadable, but its identity is not, so the undo
    // proceeds rather than refusing.
    assert_eq!(report.restored, 1, "{report:?}");
}

#[test]
fn an_unparseable_row_is_reported_not_dropped() {
    // A3. A short row and a mistyped mechanism both `continue`d, so the
    // report said the undo was clean while files stayed in the library —
    // the class the design's own preamble says the last rewrite missed.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf", "birdking_vol1.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();

    let text = std::fs::read_to_string(f.manifest()).unwrap();
    let mangled = text.replacen(",rename,", ",ranme,", 1);
    assert_ne!(mangled, text, "the manifest shape changed");
    std::fs::write(f.manifest(), mangled).unwrap();

    let report = undo(&f.plan).unwrap();
    // Asserted on the *reason*, not merely that something was refused: the
    // dropped row's orphaned `done` also produces a refusal, so a bare
    // is-not-empty check passes even when the parse failure is silent.
    assert!(
        report
            .refused
            .iter()
            .any(|r| r.contains("unknown mechanism")),
        "the unreadable row itself was dropped in silence: {report:?}"
    );
    // And the obligation's actual claim: one bad row leaves every other row
    // reversible. Without this the test passes even if a malformed row
    // aborted all its siblings, so long as the message was right.
    assert_eq!(report.restored, 1, "the sibling row was stranded: {report:?}");
    assert_eq!(
        f.files_under(&f.source).len(),
        1,
        "the sibling row did not come back"
    );
}

#[test]
fn a_partial_undo_can_be_resumed() {
    // A5. prune_empty_dirs removed the parent after a successful row, and a
    // later resolve of a refused row then failed at the parent — which is
    // not a metadata NotFound, so "nothing to do" never applied, `refused`
    // was never empty, and O_EXCL killed the plan forever.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf", "birdking_vol1.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();

    // Squat one original name so its row refuses.
    std::fs::write(f.source.join("birdking_vol1.pdf"), b"squatter").unwrap();
    let first = undo(&f.plan).unwrap();
    assert_eq!(first.restored, 1, "{first:?}");
    assert_eq!(first.refused.len(), 1, "{first:?}");

    // Clear the squatter and resume.
    std::fs::remove_file(f.source.join("birdking_vol1.pdf")).unwrap();
    let second = undo(&f.plan).unwrap();
    assert_eq!(second.restored, 1, "the partial undo could not be resumed: {second:?}");
    assert!(second.refused.is_empty(), "{second:?}");
    assert!(f.files_under(&f.dest).is_empty(), "files left in the library");

    // And the plan is runnable again.
    assert_eq!(apply(&f.plan, false, &mut |_| {}).unwrap().executed, 2);
}

#[test]
fn an_unterminated_quote_does_not_silently_swallow_the_rest() {
    // A6. One stray quote put the reader into quoted state, so every record
    // after it became part of one open field — and the report called that
    // "one incomplete final record" while three valid rows stayed in the
    // library.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf", "birdking_vol1.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();

    let text = std::fs::read_to_string(f.manifest()).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    // A stray quote partway through, with records after it.
    lines[2].push('"');
    std::fs::write(f.manifest(), lines.join("\n") + "\n").unwrap();

    let report = undo(&f.plan).unwrap();
    assert!(
        report
            .refused
            .iter()
            .any(|r| r.contains("unterminated quote") && r.contains("line")),
        "the swallowed records were reported as one incomplete record: {report:?}"
    );
}

#[test]
fn a_manifest_with_no_header_keeps_its_first_row() {
    // read_manifest skipped records[0] unconditionally, so a hand-edited
    // manifest without the column header silently lost its first data row —
    // the input invariant 1 exists for.
    let r = Recorded::new("Mazebook.pdf", b"copied here", Mode::Copy, true);
    let text = std::fs::read_to_string(&r.manifest).unwrap();
    let headerless: String = text.lines().skip(1).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(&r.manifest, headerless).unwrap();

    let report = r.reverse().unwrap();
    assert_eq!(report.restored, 1, "the roots row was eaten as a header: {report:?}");
}

#[test]
fn a_record_without_a_trailing_newline_is_not_discarded() {
    // A complete final record simply lacking "\n" was dropped and labelled
    // incomplete. Losing a `placed` that way downgrades a clone row to bare
    // `pending`, which is then refused forever.
    let r = Recorded::new("Mazebook.pdf", b"copied here", Mode::Copy, true);
    let text = std::fs::read_to_string(&r.manifest).unwrap();
    std::fs::write(&r.manifest, text.trim_end_matches('\n')).unwrap();

    let report = r.reverse().unwrap();
    assert_eq!(report.restored, 1, "{report:?}");
    assert!(report.refused.is_empty(), "{report:?}");
}

#[test]
fn a_duplicate_sequence_is_reported_not_processed_twice() {
    // Both rows parsed and set_status updated only the first, so the second
    // ran as a bare `pending`.
    let r = Recorded::new("Mazebook.pdf", b"copied here", Mode::Copy, true);
    let text = std::fs::read_to_string(&r.manifest).unwrap();
    let pending = text
        .lines()
        .find(|l| l.contains(",pending,"))
        .unwrap()
        .to_string();
    std::fs::write(&r.manifest, format!("{text}{pending}\n")).unwrap();

    let report = r.reverse().unwrap();
    assert!(
        report.refused.iter().any(|m| m.contains("duplicate row")),
        "{report:?}"
    );
}

#[test]
fn a_pending_clone_row_is_refused_rather_than_matched_on_size() {
    // Invariant 4, and the whole reason `placed` exists. A clone or stream
    // interrupted before its `placed` landed has no exact identity for its
    // destination — only the source's, which a clone does not preserve. The
    // old size-only fallback fired here, and comics from one bundle are
    // byte-identical, so it fired constantly.
    let d = tempdir().unwrap();
    let source_root = d.path().join("src");
    let dest_root = d.path().join("dest");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::create_dir_all(&dest_root).unwrap();
    let source = source_root.join("mazebook.pdf");
    std::fs::write(&source, b"identical bytes").unwrap();
    // A different file that happens to be the same size — the common case.
    let destination = dest_root.join("Mazebook.pdf");
    std::fs::write(&destination, b"someone else's").unwrap();

    let manifest = d.path().join("plan.jsonl.undo.csv");
    let mut writer = UndoWriter::create(&manifest, &source_root, &dest_root).unwrap();
    writer
        .record_pending(
            Path::new("mazebook.pdf"),
            Path::new("Mazebook.pdf"),
            Mechanism::Clone,
            Mode::Copy,
            &source,
        )
        .unwrap();
    drop(writer); // no `placed`: the run was interrupted

    let report = reverse(
        &manifest,
        &source_root,
        &dest_root,
        &[PathBuf::from("Mazebook.pdf")],
    )
    .unwrap();
    assert_eq!(report.restored, 0, "{report:?}");
    assert!(
        report
            .refused
            .iter()
            .any(|r| r.contains("no exact identity")),
        "{report:?}"
    );
    assert!(destination.exists(), "a same-size stranger was deleted");
}

#[test]
fn apply_and_undo_share_one_lock() {
    // Obligation 7's other side. Task 11 required it and two separate lock
    // files silently defeated it; nothing tested that the fix joined them.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();

    let held = comic_renamer_core::apply::Lock::acquire(&f.plan).unwrap();
    assert!(
        matches!(undo(&f.plan), Err(comic_renamer_core::types::ApplyError::Locked)),
        "undo ran while apply held the plan lock"
    );
    drop(held);
    assert_eq!(undo(&f.plan).unwrap().restored, 1);
}
