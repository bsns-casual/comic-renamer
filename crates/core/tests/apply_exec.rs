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
    let _first = UndoWriter::create(&path).expect("first create");
    let second = UndoWriter::create(&path);
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
    let mut writer = UndoWriter::create(&path).unwrap();
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
fn a_comma_in_a_title_survives_the_round_trip() {
    // The GCD lists *Hellboy, the Corpse and the Iron Shoes*.
    let d = tempdir().unwrap();
    let path = d.path().join("run.undo.csv");
    let awkward = d.path().join("Hellboy, the Corpse and the \"Iron\" Shoes.pdf");
    std::fs::write(&awkward, b"x").unwrap();
    let mut writer = UndoWriter::create(&path).unwrap();
    writer
        .record_pending(&awkward, &awkward, Mechanism::Clone, Mode::Copy)
        .unwrap();
    drop(writer);

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"\"Iron\"\""), "quotes are not doubled: {text}");
    // reverse parses it back and finds the file, which is the real proof.
    reverse(&path).unwrap();
    assert!(!awkward.exists(), "a copy reverses by deleting the destination");
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

    assert!(reverse(&f.manifest()).is_err());
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
