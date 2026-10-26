//! Task 12. Execution and undo. This is where `TreeGuard` stops being
//! decorative: every refusal test here would pass on an `Err` alone while
//! files had already moved.

mod common;

use comic_renamer_core::apply::{apply, reverse_run, Lock};
use comic_renamer_core::mapping::Mappings;
use comic_renamer_core::preset::preset_by_id;
use comic_renamer_core::types::{
    ApplyError, ApplyResult, Aside, Mechanism, Mode, Resolved, ScanOpts, Status, UndoReport,
};
use comic_renamer_core::undo::{reverse, UndoWriter};


use comic_renamer_core::{plan, scan};
use common::{mappings, TreeGuard};
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

/// Everything a record needs about a row, built from one series name so the
/// destination is one the preset genuinely produces — which is the only kind
/// `undo::reverse` will recognise.
fn produced(series: &str) -> (Resolved, PathBuf) {
    let resolved = Resolved {
        series: series.into(),
        volume: None,
        issue: None,
        subtitle: None,
        run: 5,
        status: Status::Mapped,
        record: None,
    };
    let relative = preset_by_id("komga").unwrap().destination(&resolved, "pdf");
    (resolved, relative)
}

fn gcd() -> Mappings {
    mappings(&[
        ("mazebook", "Mazebook", None, None, None, Some(5)),
        // An issue, so the streamed and cross-volume paths carry one too.
        // Every other fixture in this file is a volume or a one-shot.
        ("curse_issue3", "Curse", None, Some(3), None, None),
        ("birdking_vol1", "Birdking", Some(1), None, None, Some(3)),
        // The second volume of the same series, so two rows share one series
        // folder. Every other pair in this file lands in a directory of its
        // own, which is why nothing could see the order rows are reversed in
        // or the `prune_empty_dirs` call between them.
        ("birdking_vol2", "Birdking", Some(2), None, None, Some(3)),
        ("hellboyomnibus_vol1", "Hellboy Omnibus", Some(1), None, None, Some(4)),
    ])
}

struct Fixture {
    _source: TempDir,
    _dest: TempDir,
    _plans: TempDir,
    source: PathBuf,
    dest: PathBuf,
    plan: PathBuf,
    /// This fixture's own record store. Inside its temporary directory and
    /// never the user's: the real store is under `Application Support`, and a
    /// suite that wrote there would leave a record of every test run behind.
    records: PathBuf,
}

impl Fixture {
    fn new(names: &[&str], corpus: &Mappings, mode: Mode) -> Fixture {
        let source = tempdir().unwrap();
        let dest = tempdir().unwrap();
        let plans = tempdir().unwrap();
        let records = plans.path().join("runs");
        std::fs::create_dir_all(&records).unwrap();
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
            records,
            _source: source,
            _dest: dest,
            _plans: plans,
        }
    }

    fn lock(&self) -> Lock {
        Lock::library(&self.records, &self.dest).unwrap()
    }

    /// One run of the plan as it stands on disk, under a lock taken and
    /// released around it — which is what the caller does.
    fn run(&self) -> Result<ApplyResult, ApplyError> {
        let (header, rows, _edited) = plan::read_allowing_edits(&self.plan).unwrap();
        let lock = self.lock();
        apply(&lock, &header, &rows, &self.records, false, &mut |_| {})
    }

    /// Reverses `record` under this library's lock.
    fn undo(&self, record: &Path) -> Result<UndoReport, ApplyError> {
        let lock = self.lock();
        reverse_run(&lock, record)
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

/// A record-level fixture: a copy of one file already sitting in `dest`,
/// recorded as this run having put it there.
struct Recorded {
    _dir: TempDir,
    manifest: PathBuf,
    dest_root: PathBuf,
    relative: PathBuf,
}

impl Recorded {
    fn new(name: &str, body: &[u8], mode: Mode, keep_source: bool) -> Recorded {
        Recorded::of("Mazebook", name, body, mode, keep_source)
    }

    /// `name` is the bundle filename; the destination is whatever the komga
    /// preset makes of `series`. It has to be: `undo::reverse` rebuilds the
    /// destination from the record's own fields and refuses any row whose
    /// `dest_relative` the preset does not produce, so a bare filename at the
    /// library root — which is what this fixture used to record — is refused
    /// before anything else about the row is examined.
    fn of(series: &str, name: &str, body: &[u8], mode: Mode, keep_source: bool) -> Recorded {
        let dir = tempdir().unwrap();
        let source_root = dir.path().join("src");
        let dest_root = dir.path().join("dest");
        std::fs::create_dir_all(&source_root).unwrap();
        std::fs::create_dir_all(&dest_root).unwrap();
        let source = source_root.join(name);
        std::fs::write(&source, body).unwrap();
        let (resolved, relative) = produced(series);
        let destination = dest_root.join(&relative);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::write(&destination, body).unwrap();

        let manifest = dir.path().join("run.record");
        let mut writer =
            UndoWriter::create(&manifest, &source_root, &dest_root, "komga", 1).unwrap();
        let seq = writer
            .record_pending(
                Path::new(name),
                &relative,
                Mechanism::Clone,
                mode,
                &source,
                &resolved.series,
                resolved.volume,
                resolved.issue,
                resolved.subtitle.as_deref(),
                resolved.run,
                "pdf",
            )
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
            dest_root,
            relative,
        }
    }

    fn reverse(&self) -> Result<UndoReport, comic_renamer_core::types::UndoError> {
        reverse(&self.manifest)
    }

    fn destination(&self) -> PathBuf {
        self.dest_root.join(&self.relative)
    }
}

#[test]
fn create_refuses_an_existing_record() {
    let d = tempdir().unwrap();
    let path = d.path().join("run.record");
    let _first = UndoWriter::create(&path, d.path(), d.path(), "komga", 1).expect("first create");
    let second = UndoWriter::create(&path, d.path(), d.path(), "komga", 1);
    assert_eq!(
        second.err().map(|e| e.kind()),
        Some(ErrorKind::AlreadyExists),
        "a second run would truncate the only record of where the last run's files went"
    );
}

#[test]
fn pending_is_on_disk_before_the_operation() {
    let d = tempdir().unwrap();
    let source = d.path().join("a.pdf");
    std::fs::write(&source, b"x").unwrap();
    let path = d.path().join("run.record");
    let mut writer = UndoWriter::create(&path, d.path(), d.path(), "komga", 1).unwrap();
    writer
        .record_pending(
            Path::new("a.pdf"),
            Path::new("A/a.pdf"),
            Mechanism::Rename,
            Mode::Move,
            &source,
            "A",
            None,
            None,
            None,
            5,
            "pdf",
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
        // The same text as the series too, so the quoting is exercised in the
        // dest_relative and series columns as well as in source_name — all
        // three are written by `field`, and the reader has to agree on all
        // three for the row to be rebuilt at all.
        let series = name.trim_end_matches(".pdf");
        let r = Recorded::of(series, name, b"copied here by the tool", Mode::Copy, true);
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

/// A record of a same-volume move: `rename`, so the inode moved and the
/// recorded *source* identity is what still describes the destination. No
/// `placed` row, because a rename makes no new inode.
///
/// The bundle folder is left empty, which is the branch with **no content
/// gate**: `Mode::Move` with the source absent renames the destination back,
/// and the only thing standing between that and a stranger's file being moved
/// into the user's bundle folder is the identity comparison. Every weakening
/// of that comparison survived the suite, because the only fixture exercising
/// it was `Mode::Copy`, where `move_aside_with_a_copy_at_the_source` hashes
/// the two sides and refuses anyway.
struct RecordedMove {
    _dir: TempDir,
    manifest: PathBuf,
    source_root: PathBuf,
    destination: PathBuf,
    /// The identity the record carries, read from the file before it moved.
    identity: (u64, u64, i64),
}

impl RecordedMove {
    fn new(name: &str, body: &[u8]) -> RecordedMove {
        let dir = tempdir().unwrap();
        let source_root = dir.path().join("src");
        let dest_root = dir.path().join("dest");
        std::fs::create_dir_all(&source_root).unwrap();
        std::fs::create_dir_all(&dest_root).unwrap();
        let source = source_root.join(name);
        std::fs::write(&source, body).unwrap();

        let (resolved, relative) = produced("Mazebook");
        let destination = dest_root.join(&relative);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();

        let manifest = dir.path().join("run.record");
        let mut writer =
            UndoWriter::create(&manifest, &source_root, &dest_root, "komga", 1).unwrap();
        // `record_pending` stats the source, so it runs before the rename —
        // exactly as `apply` orders it.
        let seq = writer
            .record_pending(
                Path::new(name),
                &relative,
                Mechanism::Rename,
                Mode::Move,
                &source,
                &resolved.series,
                resolved.volume,
                resolved.issue,
                resolved.subtitle.as_deref(),
                resolved.run,
                "pdf",
            )
            .unwrap();
        std::fs::rename(&source, &destination).unwrap();
        writer.record_done(seq).unwrap();
        drop(writer);

        let meta = std::fs::symlink_metadata(&destination).unwrap();
        let identity = (
            meta.len(),
            MetadataExt::ino(&meta),
            MetadataExt::mtime(&meta) * 1_000_000_000 + MetadataExt::mtime_nsec(&meta),
        );
        RecordedMove {
            _dir: dir,
            manifest,
            source_root,
            destination,
            identity,
        }
    }

    fn reverse(&self) -> UndoReport {
        reverse(&self.manifest).unwrap()
    }

    fn restored(&self, name: &str) -> PathBuf {
        self.source_root.join(name)
    }

    fn destination(&self) -> &Path {
        &self.destination
    }

    fn live_identity(&self) -> (u64, u64, i64) {
        let meta = std::fs::symlink_metadata(&self.destination).unwrap();
        (
            meta.len(),
            MetadataExt::ino(&meta),
            MetadataExt::mtime(&meta) * 1_000_000_000 + MetadataExt::mtime_nsec(&meta),
        )
    }
}

/// The rename-back is gated on identity and on nothing else, so each component
/// of that identity has to be load-bearing on this path. Each case below
/// matches the recorded identity in every component but one.
///
/// Without this, three weakenings all passed: ignoring `mtime_ns`, comparing
/// size alone, and ignoring the inode. Under any of them a destination the run
/// did not place is renamed into the user's bundle folder and reported
/// restored — and in the first case that destination is the file they had
/// edited in place, which is the loss `Identity` carries `mtime_ns` to prevent.
#[test]
fn a_move_refuses_a_destination_that_is_not_the_file_it_placed() {
    use std::io::Write;

    // 1. Edited in place: same size, same inode, new mtime.
    let r = RecordedMove::new("mazebook.pdf", b"original");
    let mut file = std::fs::OpenOptions::new().write(true).open(r.destination()).unwrap();
    file.write_all(b"EDITEDxx").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let live = r.live_identity();
    assert_eq!(
        (live.0, live.1),
        (r.identity.0, r.identity.1),
        "the edit has to keep size and inode or this tests nothing"
    );
    assert_ne!(live.2, r.identity.2, "and it has to change the mtime");

    let report = r.reverse();
    assert_eq!(report.restored, 0, "{report:?}");
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert!(r.destination().exists(), "an edit in place was taken for our own file");
    assert!(
        !r.restored("mazebook.pdf").exists(),
        "an edited file was renamed into the bundle folder"
    );

    // 2. A different file of the same length, with the recorded mtime set on
    //    it: only the inode differs.
    let r = RecordedMove::new("mazebook.pdf", b"original");
    std::fs::remove_file(r.destination()).unwrap();
    std::fs::write(r.destination(), b"stranger").unwrap();
    let stamp = filetime::FileTime::from_unix_time(
        r.identity.2.div_euclid(1_000_000_000),
        r.identity.2.rem_euclid(1_000_000_000) as u32,
    );
    filetime::set_file_mtime(r.destination(), stamp).unwrap();
    let live = r.live_identity();
    assert_eq!(
        (live.0, live.2),
        (r.identity.0, r.identity.2),
        "size and mtime have to match, or the inode is not what is being tested"
    );
    assert_ne!(live.1, r.identity.1, "and the inode has to differ");

    let report = r.reverse();
    assert_eq!(report.restored, 0, "{report:?}");
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert_eq!(std::fs::read(r.destination()).unwrap(), b"stranger");
    assert!(
        !r.restored("mazebook.pdf").exists(),
        "a stranger's file was renamed into the bundle folder"
    );

    // 3. And the fixture really does reverse when nothing was touched, so
    //    none of the refusals above came from the shape of the record.
    let r = RecordedMove::new("mazebook.pdf", b"original");
    let report = r.reverse();
    assert_eq!((report.restored, report.refused.len()), (1, 0), "{report:?}");
    assert_eq!(std::fs::read(r.restored("mazebook.pdf")).unwrap(), b"original");
    assert!(!r.destination().exists());
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
    // Invariant 9: a hand-written record cannot name an arbitrary path.
    //
    // The attack is edited *fields*, not an edited path: the row still names
    // the file this run really placed, and the series it was rebuilt from now
    // says something else. So the destination exists, holds exactly the
    // identity the row records, and its original sits at the source — every
    // other gate in `reverse_one` is satisfied, and the only thing that can
    // refuse the row is the preset disagreeing with the path it names.
    let r = Recorded::new("Mazebook.pdf", b"x", Mode::Copy, true);
    let text = std::fs::read_to_string(&r.manifest).unwrap();
    let tampered = text.replacen(",'Mazebook,", ",'Elsewhere,", 1);
    assert_ne!(tampered, text, "the series column moved; this edits nothing");
    std::fs::write(&r.manifest, tampered).unwrap();

    let report = r.reverse().unwrap();
    assert_eq!(report.restored, 0, "{report:?}");
    assert!(
        report
            .refused
            .iter()
            .any(|why| why.contains("not a destination")),
        "refused for some other reason, so the preset check is unobserved: {report:?}"
    );
    assert!(r.destination().exists(), "a path the preset disowns was acted on");
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

#[test]
fn a_move_relocates_and_leaves_nothing_behind() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    let r = f.run().unwrap();
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
    f.run().unwrap();
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
        f.run(),
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
    let records = plans.path().join("runs");
    std::fs::create_dir_all(&records).unwrap();

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
    // Through the file, as the CLI reads it: this is the one path where a
    // plan's own serialisation of a cross-volume row matters.
    let (header, rows, _edited) = plan::read_allowing_edits(&plan_path).unwrap();

    let lock = Lock::library(&records, &dest).unwrap();
    let record = apply(&lock, &header, &rows, &records, false, &mut |_| {})
        .unwrap()
        .record
        .expect("a run that placed a row wrote a record");
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
    let report = reverse_run(&lock, &record).unwrap();
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
        issue: None,
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
    let mut w = UndoWriter::create(&record, source.path(), &library, "komga", 1).unwrap();
    let seq = w
        .record_pending(
            Path::new("mazebook.pdf"),
            &relative,
            Mechanism::Stream,
            Mode::Move,
            &fifo,
            "Mazebook",
            None,
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
    let records = plans.path().join("runs");
    std::fs::create_dir_all(&records).unwrap();

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
    let lock = Lock::library(&records, &dest).unwrap();
    let record = apply(&lock, &header, &rows, &records, false, &mut |_| {})
        .unwrap()
        .record
        .expect("a run that placed a row wrote a record");

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

    let report = reverse_run(&lock, &record).unwrap();
    assert!(
        watcher.join().unwrap(),
        "the watcher never saw a .partial, so the stream-back never ran and this \
         test proved nothing"
    );
    // Outcome first, diagnosis after. The mtime check below is a statement
    // about the *fixture* — whether the watcher's bump landed in the window —
    // and if it runs first it answers a question nobody asked yet: delete the
    // re-check and the destination is gone, so reading its mtime fails and
    // any fallback reads as "the bump did not land". The real failure is a
    // library file deleted, and it has to be the one the reader is told about.
    assert_eq!(report.restored, 0, "{report:?}");
    assert!(placed.exists(), "a destination that changed under the restore was unlinked");
    assert!(
        report.refused.iter().any(|r| r.contains("changed while it was being restored")),
        "{report:?}"
    );
    assert!(
        source.path().join("mazebook.pdf").exists(),
        "the stream-back's copy was discarded, so the file now exists nowhere it is named"
    );
    // `unwrap`, not a fallback: the assertion above established the file is
    // there, so a failure here is a failure to stat it and should say so.
    assert_ne!(
        std::fs::symlink_metadata(&placed).unwrap().mtime(),
        placed_before,
        "the mtime bump did not land, so the re-check had nothing to see"
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
    let r = f.run().unwrap();
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
    let record = f.run().unwrap().record.unwrap();
    assert!(f.files_under(&f.source).is_empty());

    f.undo(&record).unwrap();
    assert_eq!(f.files_under(&f.source), before);
    assert!(f.files_under(&f.dest).is_empty(), "the destination still holds files");
}

/// D58 changed what this test can claim. The destination is still cleared,
/// but the file it held is now in the aside directory, which is itself under
/// the library root — so `files_under(&f.dest)` is no longer empty, and a
/// version of this test that still asserted it was would be asserting that
/// the bytes had been destroyed.
///
/// Named rather than counted: "one file somewhere under the library" would
/// also be true of a reverse that did nothing at all.
#[test]
fn reverse_of_a_copy_clears_the_destination_and_sets_the_file_aside() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Copy);
    let record = f.run().unwrap().record.unwrap();
    let placed = f.dest.join("Mazebook/Mazebook.pdf");
    assert!(placed.is_file(), "nothing was placed to reverse");

    // The source must be untouched throughout: getting copy and move the
    // wrong way round here clobbers a kept original.
    let guard = TreeGuard::new(&[f.source.as_path()]);
    let report = f.undo(&record).unwrap();
    assert!(!placed.exists(), "the destination was not cleared");

    let aside = report.aside.expect("a reversed copy names its aside directory");
    assert_eq!(aside.files, 1);
    assert!(
        aside.directory.starts_with(f.dest.canonicalize().unwrap()),
        "the aside directory belongs under the library root: {}",
        aside.directory.display()
    );
    assert_eq!(
        std::fs::read_to_string(aside.directory.join("Mazebook.pdf")).unwrap(),
        "contents of mazebook.pdf",
        "the bytes the library held are still readable"
    );
    assert_eq!(
        f.files_under(&f.dest),
        vec!["Mazebook.pdf".to_string()],
        "the only file left under the library is the one set aside"
    );
    drop(guard);
}

#[test]
fn reverse_is_idempotent_and_refuses_a_reoccupied_path() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    let record = f.run().unwrap().record.unwrap();

    assert_eq!(f.undo(&record).unwrap().restored, 1);
    // Idempotent: a complete reverse deletes the record, so a second pass has
    // nothing to do rather than failing.
    let second = f.undo(&record).unwrap();
    assert_eq!((second.restored, second.refused.len()), (0, 0), "{second:?}");
    assert_eq!(f.files_under(&f.source), ["mazebook.pdf"]);

    // And the plan is runnable again. It always is now — nothing is named
    // after it — but a second run still has to write a record of its own
    // rather than finding the first one's in the way.
    let again = f.run().unwrap();
    assert_eq!(again.executed, 1);
    assert!(again.record.is_some_and(|r| r != record));
}

#[test]
fn reverse_refuses_to_overwrite_a_squatter() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    let record = f.run().unwrap().record.unwrap();
    std::fs::write(f.source.join("mazebook.pdf"), b"squatter").unwrap();

    // Per-row now, not a whole-record error: one bad row must not strand
    // every other row in the file.
    let report = f.undo(&record).unwrap();
    assert_eq!(report.restored, 0);
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert_eq!(
        std::fs::read_to_string(f.source.join("mazebook.pdf")).unwrap(),
        "squatter",
        "reverse overwrote something it did not put there"
    );
}

#[test]
fn a_re_run_cannot_truncate_the_previous_record() {
    let c = gcd();
    // Copy, not Move: a second run of a *move* never gets this far, because
    // its source is gone and the pre-flight refuses the whole run. A copy
    // keeps the original, so the second run reaches the collision check and
    // the record logic beyond it, which is what this is about.
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Copy);
    let record = f.run().unwrap().record.unwrap();
    let first = std::fs::read_to_string(&record).unwrap();

    // The second run collides on every row, so it places nothing — and a run
    // that places nothing writes no record and sweeps none either.
    let again = f.run().unwrap();
    assert_eq!((again.executed, again.skipped), (0, 1), "{again:?}");
    assert_eq!(again.record, None);
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
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
    let records = plans.path().join("runs");
    std::fs::create_dir_all(&records).unwrap();

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

    let lock = Lock::library(&records, &dest).unwrap();
    let result = apply(&lock, &header, &rows, &records, false, &mut |_| {});

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

    // An aborted run returns `Err`, so it hands back no record path — which
    // is the case the store answers: the record of what the run managed to do
    // is in it, findable without the run's cooperation. A crash is the same
    // shape, and it is the shape `undo` was built for.
    let found = comic_renamer_core::record::list(&records).unwrap();
    assert_eq!(found.len(), 1, "the aborted run left no record: {found:?}");
    let report = reverse_run(&lock, &found[0]).unwrap();
    assert!(report.refused.is_empty(), "{report:?}");
    assert_eq!(
        std::fs::read_dir(source.path()).unwrap().count(),
        2,
        "undo disturbed the sources"
    );
    // D58 changed what this test can claim. Reversing a `placed` move takes
    // the record-authorised unlink, which now renames the library file into
    // the aside directory — so "the library is empty" would be asserting that
    // the duplicate's bytes were destroyed, which is the thing D58 removed.
    //
    // What is still true, and asserted positively below: nothing remains at a
    // library *destination*, and the file that moved is in the aside directory
    // under this run's id with its bytes readable.
    //
    // The destinations are recomputed through the preset, the way `apply` does
    // (invariant 2). Reading them out of the plan would check the library
    // against a string the plan supplied.
    let komga = preset_by_id("komga").unwrap();
    for row in &rows {
        let destination = dest.join(komga.destination(&row.resolved(), &row.extension));
        assert!(
            !destination.exists(),
            "a row is still at its library destination: {}",
            destination.display()
        );
    }

    // Named rather than counted, as its same-volume sibling is: the directory
    // is this run's id, and the file in it reads back. "One file somewhere
    // under the library" would also hold for a reverse that moved nothing.
    let aside = dest
        .canonicalize()
        .unwrap()
        .join(".comic-renamer-aside")
        .join(found[0].file_name().expect("a record is named after its run"));
    assert_eq!(
        report.aside,
        Some(Aside { directory: aside.clone(), files: 1 }),
        "the reverse did not name the directory it moved the duplicate into: {report:?}"
    );
    assert_eq!(
        std::fs::read_to_string(aside.join("Birdking Vol. 01.pdf")).unwrap(),
        "contents of birdking_vol1.pdf",
        "the duplicate's bytes are neither at the destination nor set aside"
    );
    // And nowhere else under the library: one file, the one set aside.
    assert_eq!(
        walkdir(&dest),
        vec!["Birdking Vol. 01.pdf".to_string()],
        "the library holds something besides the file set aside"
    );

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
    let (resolved, relative) = produced("Mazebook");
    let destination = dest_root.join(&relative);
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::write(&destination, good).unwrap();
    let source = source_root.join("mazebook.pdf");
    std::fs::write(&source, good).unwrap();

    let manifest = d.path().join("run.record");
    let mut writer =
        UndoWriter::create(&manifest, &source_root, &dest_root, "komga", 1).unwrap();
    let seq = writer
        .record_pending(
            Path::new("mazebook.pdf"),
            &relative,
            Mechanism::Stream,
            Mode::Move,
            &source,
            &resolved.series,
            resolved.volume,
            resolved.issue,
            resolved.subtitle.as_deref(),
            resolved.run,
            "pdf",
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

    let report = reverse(&manifest).unwrap();
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

    let (resolved, relative) = produced("Mazebook");
    let fifo = dest_root.join(&relative);
    std::fs::create_dir_all(fifo.parent().unwrap()).unwrap();
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path for the duration of the call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);

    let manifest = d.path().join("run.record");
    let mut writer =
        UndoWriter::create(&manifest, &source_root, &dest_root, "komga", 1).unwrap();
    writer
        .record_pending(
            Path::new("mazebook.pdf"),
            &relative,
            Mechanism::Stream,
            Mode::Move,
            &source,
            &resolved.series,
            resolved.volume,
            resolved.issue,
            resolved.subtitle.as_deref(),
            resolved.run,
            "pdf",
        )
        .unwrap();
    drop(writer);

    // The assertion is that this returns at all.
    let report = reverse(&manifest).unwrap();
    assert_eq!(report.restored, 0, "{report:?}");
    assert!(fifo.exists());
}

#[test]
fn an_unreadable_destination_is_refused_not_mistaken_for_absent() {
    // A2's sibling: openat failed on a file the user cannot read, so the row
    // was refused for the wrong reason. fstatat answers through the parent.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    let record = f.run().unwrap().record.unwrap();
    let book = f.dest.join("Mazebook/Mazebook.pdf");
    let mut locked = std::fs::metadata(&book).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut locked, 0o000);
    std::fs::set_permissions(&book, locked).unwrap();

    let report = f.undo(&record).unwrap();
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
    let record = f.run().unwrap().record.unwrap();

    let text = std::fs::read_to_string(&record).unwrap();
    let mangled = text.replacen(",rename,", ",ranme,", 1);
    assert_ne!(mangled, text, "the record shape changed");
    std::fs::write(&record, mangled).unwrap();

    let report = f.undo(&record).unwrap();
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
    // not a metadata NotFound, so "nothing to do" never applied and `refused`
    // was never empty. A record that keeps a refusal forever is a record
    // that never gets deleted, so the resumed pass is also what proves the
    // record survived the first one.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf", "birdking_vol1.pdf"], &c, Mode::Move);
    let record = f.run().unwrap().record.unwrap();

    // Squat one original name so its row refuses.
    std::fs::write(f.source.join("birdking_vol1.pdf"), b"squatter").unwrap();
    let first = f.undo(&record).unwrap();
    assert_eq!(first.restored, 1, "{first:?}");
    assert_eq!(first.refused.len(), 1, "{first:?}");
    assert!(record.exists(), "a half-reversed run lost its record");

    // Clear the squatter and resume.
    std::fs::remove_file(f.source.join("birdking_vol1.pdf")).unwrap();
    let second = f.undo(&record).unwrap();
    assert_eq!(second.restored, 1, "the partial undo could not be resumed: {second:?}");
    assert!(second.refused.is_empty(), "{second:?}");
    assert!(f.files_under(&f.dest).is_empty(), "files left in the library");

    // And the plan is runnable again.
    assert_eq!(f.run().unwrap().executed, 2);
}

/// Rows are reversed newest-first, and two rows in one series folder is the
/// only fixture that can see it.
///
/// `reverse` iterates `m.rows.into_iter().rev()`, and turning that into
/// forward order left the whole suite green: every other pair of rows here
/// lands in a directory of its own, so neither the iteration order nor the
/// `prune_empty_dirs` call between two rows was observable.
///
/// The order is read off `refused`, which is pushed per row as the loop meets
/// them. Measured while writing this, and worth recording: with the komga
/// preset the *outcome* is the same either way — `rmdir` refuses a directory
/// that still holds the sibling's file, so the folder goes on whichever row is
/// last and both rows restore regardless. So this pins the order itself, plus
/// the shared-folder prune that forward order would otherwise have hidden.
#[test]
fn two_rows_in_one_series_folder_reverse_newest_first() {
    let c = gcd();
    let f = Fixture::new(&["birdking_vol1.pdf", "birdking_vol2.pdf"], &c, Mode::Move);
    let record = f.run().unwrap().record.unwrap();
    let folder = f.dest.join("Birdking");
    assert_eq!(
        f.files_under(&folder).len(),
        2,
        "both rows have to land in one folder for this to mean anything"
    );

    // Both original names occupied, so both rows refuse and the order they
    // were met in is visible.
    for name in ["birdking_vol1.pdf", "birdking_vol2.pdf"] {
        std::fs::write(f.source.join(name), b"squatter").unwrap();
    }
    let report = f.undo(&record).unwrap();
    assert_eq!(report.restored, 0, "{report:?}");
    assert_eq!(report.refused.len(), 2, "{report:?}");
    assert!(
        report.refused[0].starts_with("row 2") && report.refused[1].starts_with("row 1"),
        "rows were reversed oldest-first: {:?}",
        report.refused
    );

    // And with the squatters cleared, the shared folder is pruned exactly
    // once, by whichever row empties it.
    for name in ["birdking_vol1.pdf", "birdking_vol2.pdf"] {
        std::fs::remove_file(f.source.join(name)).unwrap();
    }
    let report = f.undo(&record).unwrap();
    assert_eq!((report.restored, report.refused.len()), (2, 0), "{report:?}");
    assert!(!folder.exists(), "the emptied series folder was left behind");
    assert!(f.files_under(&f.dest).is_empty(), "{:?}", f.files_under(&f.dest));
}

#[test]
fn an_unterminated_quote_does_not_silently_swallow_the_rest() {
    // A6. One stray quote put the reader into quoted state, so every record
    // after it became part of one open field — and the report called that
    // "one incomplete final record" while three valid rows stayed in the
    // library.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf", "birdking_vol1.pdf"], &c, Mode::Move);
    let record = f.run().unwrap().record.unwrap();

    let text = std::fs::read_to_string(&record).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    // A stray quote partway through, with records after it.
    lines[2].push('"');
    std::fs::write(&record, lines.join("\n") + "\n").unwrap();

    let report = f.undo(&record).unwrap();
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
    let (resolved, relative) = produced("Mazebook");
    let destination = dest_root.join(&relative);
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::write(&destination, b"someone else's").unwrap();

    let manifest = d.path().join("run.record");
    let mut writer =
        UndoWriter::create(&manifest, &source_root, &dest_root, "komga", 1).unwrap();
    writer
        .record_pending(
            Path::new("mazebook.pdf"),
            &relative,
            Mechanism::Clone,
            Mode::Copy,
            &source,
            &resolved.series,
            resolved.volume,
            resolved.issue,
            resolved.subtitle.as_deref(),
            resolved.run,
            "pdf",
        )
        .unwrap();
    drop(writer); // no `placed`: the run was interrupted

    let report = reverse(&manifest).unwrap();
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
fn a_run_and_a_reverse_cannot_overlap_on_one_library() {
    // The successor to `apply_and_undo_share_one_lock`. Obligation 7's other
    // side: `undo` always required the lock `apply` takes, and two separate
    // lock files silently defeated it. The lock is now one per library and
    // held by the caller, so this is about what `Lock::library` refuses —
    // `apply_record.rs` covers the pair sharing one held lock.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    let record = f.run().unwrap().record.unwrap();

    let held = f.lock();
    assert!(
        matches!(
            Lock::library(&f.records, &f.dest),
            Err(comic_renamer_core::types::ApplyError::Locked)
        ),
        "a second holder took the library lock while the first held it"
    );
    drop(held);
    assert_eq!(f.undo(&record).unwrap().restored, 1);
}
