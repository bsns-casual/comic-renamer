//! Task 12. Execution and undo. This is where `TreeGuard` stops being
//! decorative: every refusal test here would pass on an `Err` alone while
//! files had already moved.

mod common;

use comic_renamer_core::apply::{apply, undo_path};
use comic_renamer_core::corpus::Corpus;
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::types::{Mechanism, Mode, PlanHeader, PlanRow, ScanOpts};
use comic_renamer_core::undo::{reverse, UndoWriter};
use comic_renamer_core::{plan, scan};
use common::{corpus, tree_from_names, TreeGuard};
use std::io::ErrorKind;
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

#[test]
fn create_refuses_an_existing_manifest() {
    let d = tempdir().unwrap();
    let path = d.path().join("run.undo.csv");
    let _first = UndoWriter::create(&path, d.path()).expect("first create");
    let second = UndoWriter::create(&path, d.path());
    assert_eq!(
        second.err().map(|e| e.kind()),
        Some(ErrorKind::AlreadyExists),
        "a re-run would truncate the only record of where the last run's files went"
    );
}

#[test]
fn pending_is_on_disk_before_the_operation() {
    let d = tempdir().unwrap();
    let path = d.path().join("run.undo.csv");
    let mut writer = UndoWriter::create(&path, d.path()).unwrap();
    writer
        .record_pending(
            Path::new("/src/a.pdf"),
            Path::new("/dest/A/a.pdf"),
            Mechanism::Rename,
            Mode::Move,
        )
        .unwrap();
    // Not after a flush we controlled — read it back cold.
    assert!(std::fs::read_to_string(&path).unwrap().contains("pending"));
}

#[test]
fn awkward_filenames_survive_the_manifest_round_trip() {
    // The GCD lists *Hellboy, the Corpse and the Iron Shoes*; newlines are
    // legal on APFS and arrive from bundle filenames. The writer quoted both
    // correctly all along — the reader used `str::lines`, which splits inside
    // a quoted field, so one such name made the whole run unreversible.
    let d = tempdir().unwrap();
    for name in [
        "Hellboy, the Corpse and the \"Iron\" Shoes.pdf",
        "two\nlines.pdf",
    ] {
        let run = d.path().join(format!("run{}.csv", name.len()));
        let dest = d.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let victim = dest.join(name);
        std::fs::write(&victim, b"copied here by the tool").unwrap();

        let mut writer = UndoWriter::create(&run, &dest).unwrap();
        let seq = writer
            .record_pending(Path::new("/src/x.pdf"), &victim, Mechanism::Clone, Mode::Copy)
            .unwrap();
        writer.record_done(seq, &victim).unwrap();
        drop(writer);

        let report = reverse(&run).unwrap();
        assert_eq!(report.restored, 1, "{name:?} did not round-trip: {report:?}");
        assert!(!victim.exists());
    }
}

#[test]
fn reverse_refuses_a_destination_it_did_not_create() {
    // The manifest used to record no identity at all, so `reverse` deleted
    // or relocated whatever sat at the path. Replace a library file with a
    // better scan, run undo, and the replacement was taken.
    let d = tempdir().unwrap();
    let run = d.path().join("run.csv");
    let dest = d.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let path = dest.join("Mazebook.pdf");
    std::fs::write(&path, b"what the tool copied").unwrap();

    let mut writer = UndoWriter::create(&run, &dest).unwrap();
    let seq = writer
        .record_pending(Path::new("/src/x.pdf"), &path, Mechanism::Clone, Mode::Copy)
        .unwrap();
    writer.record_done(seq, &path).unwrap();
    drop(writer);

    // The user replaces it with a better scan.
    std::fs::write(&path, b"THE GOOD SCAN, much larger than before").unwrap();

    let report = reverse(&run).unwrap();
    assert_eq!(report.restored, 0);
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert!(path.exists(), "reverse deleted a file the tool never created");
}

#[test]
fn reverse_refuses_a_destination_outside_the_manifests_root() {
    // A manifest is as untrusted as a plan (§6.11): a hand-written one used
    // to make `reverse` delete any file on the system.
    let d = tempdir().unwrap();
    let outside = d.path().join("private");
    std::fs::create_dir_all(&outside).unwrap();
    let victim = outside.join("tax-return.pdf");
    std::fs::write(&victim, b"PRIVATE").unwrap();
    let dest = d.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();

    let run = d.path().join("hand-written.csv");
    std::fs::write(
        &run,
        format!(
            "seq,status,mode,mechanism,source,destination,size,inode\n\
             0,root,,,,'{},,\n\
             1,pending,copy,clone,'/src/x.pdf,'{},7,1\n",
            dest.display(),
            victim.display()
        ),
    )
    .unwrap();

    let report = reverse(&run).unwrap();
    assert_eq!(report.restored, 0);
    assert!(victim.exists(), "reverse deleted a file outside its own root");
}

#[test]
fn a_torn_final_line_does_not_abandon_the_rows_before_it() {
    // The crash this manifest exists to survive. A truncated last record used
    // to abort the whole file, leaving every completed row stranded.
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();

    let mut text = std::fs::read_to_string(f.manifest()).unwrap();
    text.push_str("2,pending,move,ren");
    std::fs::write(f.manifest(), text).unwrap();

    let report = reverse(&f.manifest()).unwrap();
    assert_eq!(report.restored, 1, "{report:?}");
    assert_eq!(report.refused.len(), 1, "the torn tail was not reported");
    assert!(f.source.join("mazebook.pdf").exists());
}

#[test]
fn one_refused_row_does_not_block_the_others() {
    // Rows unwind newest-first, and the first Reoccupied used to return,
    // so an earlier successful row never came back — forever, on every
    // re-run.
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf", "birdking_vol1.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();
    assert!(f.files_under(&f.source).is_empty());

    // A squatter at one original path.
    std::fs::write(f.source.join("birdking_vol1.pdf"), b"squatter").unwrap();

    let report = reverse(&f.manifest()).unwrap();
    assert_eq!(report.restored, 1, "{report:?}");
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert!(f.source.join("mazebook.pdf").exists(), "the other row never came back");
}

#[test]
fn reverse_reports_what_it_could_not_examine() {
    // Path::exists() answers false on EACCES, so a permission problem read as
    // "already reversed" and reverse returned success having done nothing.
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();

    let series = f.dest.join("Mazebook");
    let mut locked = std::fs::metadata(&series).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut locked, 0o000);
    std::fs::set_permissions(&series, locked).unwrap();

    let report = reverse(&f.manifest());

    let mut restore = std::fs::metadata(&series).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut restore, 0o700);
    std::fs::set_permissions(&series, restore).unwrap();

    let report = report.unwrap();
    assert_eq!(report.restored, 0);
    assert!(
        !report.refused.is_empty(),
        "an unreadable destination was reported as nothing to do: {report:?}"
    );
}

#[test]
fn reverse_takes_the_lock() {
    // Task 11: "the lock is an O_EXCL file beside the plan; undo takes it
    // too." It did not, so an apply and an undo could interleave.
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();

    let mut held = f.manifest().into_os_string();
    held.push(".lock");
    std::fs::write(&held, b"").unwrap();
    assert!(matches!(
        reverse(&f.manifest()),
        Err(comic_renamer_core::types::UndoError::Locked)
    ));
    std::fs::remove_file(&held).unwrap();
    assert_eq!(reverse(&f.manifest()).unwrap().restored, 1);
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

    reverse(&undo_path(&plan_path)).unwrap();
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

    reverse(&f.manifest()).unwrap();
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
    reverse(&f.manifest()).unwrap();
    assert!(f.files_under(&f.dest).is_empty());
    drop(guard);
}

#[test]
fn reverse_is_idempotent_and_refuses_a_reoccupied_path() {
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();

    reverse(&f.manifest()).unwrap();
    reverse(&f.manifest()).unwrap();
    assert_eq!(f.files_under(&f.source), ["mazebook.pdf"]);

    // Now move it away again and put a squatter at the original path.
    apply(&f.plan, false, &mut |_| {}).unwrap_err(); // the manifest already exists
}

#[test]
fn reverse_refuses_to_overwrite_a_squatter() {
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c, Mode::Move);
    apply(&f.plan, false, &mut |_| {}).unwrap();
    std::fs::write(f.source.join("mazebook.pdf"), b"squatter").unwrap();

    // Per-row now, not a whole-manifest error: one bad row must not strand
    // every other row in the file.
    let report = reverse(&f.manifest()).unwrap();
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
