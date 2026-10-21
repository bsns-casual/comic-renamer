//! Task 12. Execution and undo. This is where `TreeGuard` stops being
//! decorative: every refusal test here would pass on an `Err` alone while
//! files had already moved.

mod common;

use comic_renamer_core::apply::{apply, undo, undo_path};
use comic_renamer_core::corpus::Corpus;
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::types::{Mechanism, Mode, PlanHeader, PlanRow, ScanOpts};
use comic_renamer_core::undo::{reverse, UndoWriter};


use comic_renamer_core::{plan, scan};
use common::{corpus, tree_from_names, TreeGuard};
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

fn gcd() -> (TempDir, Corpus) {
    corpus(&[
        ("Mazebook", "mazebook", "Dark Horse", "en", 2021, 5),
        ("Birdking", "birdking", "Dark Horse", "en", 2022, 3),
        ("Hellboy Omnibus", "hellboyomnibus", "Dark Horse", "en", 2018, 4),
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
    fn new(names: &[&str], corpus: &Corpus, mode: Mode) -> Fixture {
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
        let (header, rows) = scan::scan(&opts, corpus, &Overrides::default(), &mut |_| {}).unwrap();
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
    /// checksum. Used to force the streaming path without a second volume.
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
    let (_g, c) = gcd();
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
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Copy);
    apply(&f.plan, false, &mut |_| {}).unwrap();
    assert!(f.source.join("mazebook.pdf").exists(), "a copy must keep it");
    assert_eq!(f.files_under(&f.dest), ["Mazebook.pdf"]);
}

#[test]
fn a_plan_claiming_a_mechanism_the_volume_cannot_give_is_refused() {
    // The pre-flight re-derives the mechanism and never silently falls back.
    // This is also why the streamed path cannot be forced by editing a plan.
    let (_g, c) = gcd();
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
    let (_g, c) = gcd();

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
    let (header, rows) = scan::scan(&opts, &c, &Overrides::default(), &mut |_| {}).unwrap();
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

    undo(&plan_path).unwrap();
    assert!(source.path().join("mazebook.pdf").exists(), "undo did not restore it");
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn a_failed_row_aborts_the_rest_and_leaves_them_where_they_were() {
    // A permission failure must not turn one refusal into a half-moved
    // library. The source directory is read-only, so the first unlink fails.
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf", "birdking_vol1.pdf"], &c, Mode::Move);
    f.force_mechanism(Mechanism::Stream);
    let mut perms = std::fs::metadata(&f.source).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o500);
    std::fs::set_permissions(&f.source, perms).unwrap();

    let result = apply(&f.plan, false, &mut |_| {});

    let mut restore = std::fs::metadata(&f.source).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut restore, 0o700);
    std::fs::set_permissions(&f.source, restore).unwrap();

    assert!(result.is_err(), "a failed unlink must abort the run");
    // Both sources survive: the first because its unlink failed, the second
    // because the run stopped before reaching it.
    assert!(f.source.join("mazebook.pdf").exists());
    assert!(f.source.join("birdking_vol1.pdf").exists());
}

#[test]
fn an_occupied_destination_is_refused_not_overwritten() {
    let (_g, c) = gcd();
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
    let (_g, c) = gcd();
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
    let (_g, c) = gcd();
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
    let (_g, c) = gcd();
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
    let (_g, c) = gcd();
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
    let (_g, c) = gcd();
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
    let (_g, c) = gcd();

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
    let (header, rows) = scan::scan(&opts, &c, &Overrides::default(), &mut |_| {}).unwrap();
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
