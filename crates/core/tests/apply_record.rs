//! Task 5. The record store: where a run's record goes, what happens to the
//! records that came before it, and what the library lock covers.
//!
//! These are the tests the old per-plan manifest could not have: it was named
//! after the plan, so "which record is this run's" and "what does a second run
//! do to the first one's" had no answer to get wrong.

mod common;

use comic_renamer_core::apply::{apply, reverse_run, Lock};
use comic_renamer_core::mapping::Mappings;
use comic_renamer_core::types::{ApplyError, Mode, PlanHeader, PlanRow, ScanOpts};
use comic_renamer_core::{plan, scan};
use common::{mappings, TreeGuard};
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

fn gcd() -> Mappings {
    mappings(&[
        ("mazebook", "Mazebook", None, None, None, Some(5)),
        ("birdking_vol1", "Birdking", Some(1), None, None, Some(3)),
        // A subtitle, because the preset puts it in the leaf — `Hellboy Vol.
        // 01 - Seed of Destruction.pdf` — so a record that loses it names a
        // path the preset does not produce and the row is refused forever.
        (
            "hellboy_vol1_seedofdestruction",
            "Hellboy",
            Some(1),
            None,
            Some("Seed of Destruction"),
            Some(4),
        ),
        // A run of 100, because `pad` is `run.to_string().len().max(2)` and
        // every shorter run floors to width 2 — so this is the only shape in
        // which the run is visible in a path at all.
        ("velvet_vol1", "Velvet", Some(1), None, None, Some(100)),
        // An issue, for the same reason the subtitle row above exists and
        // `common::MapRow` carries the field at all: a field no fixture
        // expresses is a field no test pins. Every other fixture in the
        // workspace passes `issue: None`, so `scan` dropping the issue and
        // `apply` recording none both survived the whole suite — the second
        // placing the file correctly while making the row unreversible.
        ("curse_issue3", "Curse", None, Some(3), None, None),
    ])
}

/// One source tree, one library, one record store, and the plan over them
/// already read into memory — which is how `apply` takes it now.
struct Fixture {
    dir: TempDir,
    source_root: PathBuf,
    dest_root: PathBuf,
    records: PathBuf,
    header: PlanHeader,
    rows: Vec<PlanRow>,
}

fn fixture() -> Fixture {
    build(&["mazebook.pdf"], Mode::Move).0
}

fn build(names: &[&str], mode: Mode) -> (Fixture, Vec<PlanRow>) {
    let dir = tempdir().unwrap();
    let source_root = dir.path().join("bundle");
    let dest_root = dir.path().join("library");
    let records = dir.path().join("runs");
    for path in [&source_root, &dest_root, &records] {
        std::fs::create_dir_all(path).unwrap();
    }
    for name in names {
        std::fs::write(source_root.join(name), format!("contents of {name}")).unwrap();
    }
    let opts = ScanOpts {
        source_root: source_root.clone(),
        dest_root: dest_root.clone(),
        preset: "komga".into(),
        mode: Some(mode),
        lang: "en".into(),
    };
    let comic_renamer_core::scan::ScanOutcome { header, rows, .. } =
        scan::scan(&opts, &gcd(), &mut |_| {}).unwrap();
    assert_eq!(rows.len(), names.len(), "a name did not resolve: {rows:?}");
    let all = rows.clone();
    (
        Fixture {
            dir,
            source_root,
            dest_root,
            records,
            header,
            rows,
        },
        all,
    )
}

/// Two plans over one pair of roots: the rows of one scan, split in two. A
/// plan per file, both naming the same library, which is what `supersede`
/// keys on.
struct TwoPlans {
    _dir: TempDir,
    dest_root: PathBuf,
    records: PathBuf,
    header_a: PlanHeader,
    rows_a: Vec<PlanRow>,
    header_b: PlanHeader,
    rows_b: Vec<PlanRow>,
}

fn two_plans_one_root_pair() -> TwoPlans {
    let (f, all) = build(&["mazebook.pdf", "birdking_vol1.pdf"], Mode::Move);
    let mut rows = all;
    let second = rows.pop().unwrap();
    let first = rows.pop().unwrap();
    TwoPlans {
        dest_root: f.dest_root.clone(),
        records: f.records.clone(),
        header_a: f.header.clone(),
        rows_a: vec![first],
        header_b: f.header.clone(),
        rows_b: vec![second],
        _dir: f.dir,
    }
}

fn run(f: &Fixture, lock: &Lock) -> Result<comic_renamer_core::types::ApplyResult, ApplyError> {
    apply(lock, &f.header, &f.rows, &f.records, false, &mut |_| {})
}

// ---------------------------------------------------------------------------
// A run that cannot be recorded does not happen
// ---------------------------------------------------------------------------

/// Review Focus 4.
#[test]
fn apply_refuses_when_the_records_directory_is_not_a_directory() {
    let f = fixture();
    let blocked = f.dir.path().join("blocked");
    std::fs::write(&blocked, "not a directory").unwrap();
    let _guard = TreeGuard::new(&[&f.source_root, &f.dest_root]);
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let err = apply(&lock, &f.header, &f.rows, &blocked, false, &mut |_| {}).unwrap_err();
    assert!(matches!(err, ApplyError::Records(_)), "{err:?}");
}

/// Review Focus 4's other half, and the one the probe exists for: a directory
/// that is there and cannot be written into satisfies every existence check.
#[test]
fn apply_refuses_when_the_records_directory_cannot_be_written() {
    let f = fixture();
    let readonly = f.dir.path().join("readonly");
    std::fs::create_dir(&readonly).unwrap();
    let mut locked = std::fs::metadata(&readonly).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut locked, 0o500);
    std::fs::set_permissions(&readonly, locked).unwrap();

    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let outcome = {
        let _guard = TreeGuard::new(&[&f.source_root, &f.dest_root]);
        apply(&lock, &f.header, &f.rows, &readonly, false, &mut |_| {})
    };
    // Restored before the assertion, so a failure does not leave a directory
    // `TempDir` cannot remove.
    let mut open = std::fs::metadata(&readonly).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut open, 0o700);
    std::fs::set_permissions(&readonly, open).unwrap();

    let err = outcome.unwrap_err();
    assert!(matches!(err, ApplyError::Records(_)), "{err:?}");
}

// ---------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------

/// The HIGH finding that drove this plan's rewrite: running one command twice
/// must not make the first run unreversible.
///
/// Copy mode, because that is the door this reaches. A second *move* refuses
/// at the pre-flight — its source is gone — and never gets as far as the
/// collision check; the test below covers that one. A copy keeps its
/// original, so the second run passes the pre-flight, collides on every row,
/// places nothing, and arrives at the sweep with no record of its own.
#[test]
fn a_second_run_that_places_nothing_keeps_the_first_runs_record() {
    let f = build(&["mazebook.pdf"], Mode::Copy).0;
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let first = run(&f, &lock).unwrap();
    let record = first
        .record
        .clone()
        .expect("a run that placed rows wrote a record");
    let before = std::fs::read_to_string(&record).unwrap();

    // Every row now collides with what the first run placed.
    let again = run(&f, &lock).unwrap();
    assert_eq!(
        (again.executed, again.skipped),
        (0, 1),
        "the second run placed something: {again:?}"
    );
    assert_eq!(
        again.record, None,
        "every row collided, so nothing was recorded"
    );
    assert!(record.exists(), "and the first run is still reversible");
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        before,
        "the only record of where the first run's files went was rewritten"
    );
}

/// The other door: a run refused before it reaches a single row must not take
/// the previous run's record either. A second move is the common way to get
/// here — the sources it names are in the library now.
#[test]
fn a_run_refused_at_the_pre_flight_keeps_the_previous_runs_record() {
    let f = fixture();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let record = run(&f, &lock).unwrap().record.unwrap();

    let err = run(&f, &lock).unwrap_err();
    assert!(matches!(err, ApplyError::SourceChanged(_)), "{err:?}");
    assert!(record.exists(), "the first run stopped being reversible");
}

/// And it says so. The count `supersede` returns was discarded, so a user who
/// re-imported a bundle lost the earlier run's undo without a word — they
/// would find out by running `undo` and being told there was nothing to do.
#[test]
fn a_second_real_run_supersedes_the_first_and_says_so() {
    let f = two_plans_one_root_pair();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let a = apply(&lock, &f.header_a, &f.rows_a, &f.records, false, &mut |_| {}).unwrap();
    // `total: 0` is how core marks a message rather than a count.
    let mut said: Vec<String> = Vec::new();
    let b = apply(&lock, &f.header_b, &f.rows_b, &f.records, false, &mut |p| {
        if p.total == 0 {
            said.push(p.current.clone());
        }
    })
    .unwrap();
    let (a, b) = (a.record.unwrap(), b.record.unwrap());
    assert_ne!(a, b, "two runs wrote one record");
    assert!(!a.exists(), "the superseded record survived");
    assert!(b.exists(), "the run's own record was swept");
    assert!(
        said.iter()
            .any(|m| m == "1 earlier run(s) of this bundle can no longer be undone"),
        "the sweep took a record in silence: {said:?}"
    );
}

/// A row whose source is gone by its turn says so, and names which row.
///
/// Two concurrent runs on one library under two `XDG_CONFIG_HOME`s are not
/// serialised — the lock lives in the record store, so two stores give one
/// library two locks and both runs proceed. `RENAME_EXCL` kept the library
/// consistent and nothing was lost, but the loser's refusal was
/// `applying 0/600error: No such file or directory (os error 2)`: no row, no
/// path, and nothing saying another run had taken the file.
///
/// `progress` is the interposition point `apply` already has — it is called
/// with this row's index immediately before the row runs — so the window is
/// entered deterministically rather than raced. Two rows over one file cannot
/// reach it: `find_collisions` groups them by inode and refuses both.
#[test]
fn a_row_whose_source_vanished_names_the_row_and_the_file() {
    let f = fixture();
    let source = f.rows[0].source.clone();
    let id = f.rows[0].id.clone();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();

    let err = apply(&lock, &f.header, &f.rows, &f.records, false, &mut |p| {
        // Past the pre-flight, before the row: exactly where the other run's
        // rename lands.
        if p.total > 0 && p.done == 0 {
            std::fs::remove_file(&source).unwrap();
        }
    })
    .unwrap_err();

    match &err {
        ApplyError::SourceGone(row, path) => {
            assert_eq!(row, &id);
            assert_eq!(path, &source);
        }
        other => panic!("a bare errno naming nothing: {other}"),
    }
}

/// And a `NotFound` that is *not* the source keeps its own message: this one
/// would send the user looking for a file that is sitting right there.
#[test]
fn a_not_found_that_is_not_the_source_is_not_blamed_on_the_source() {
    let f = fixture();
    let dest_root = f.dest_root.clone();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();

    let err = apply(&lock, &f.header, &f.rows, &f.records, false, &mut |p| {
        // The library itself, taken out from under the row. `ensure_parent`
        // opens `dest_root` with `O_DIRECTORY` and gets the same `ENOENT` a
        // missing source produces.
        if p.total > 0 && p.done == 0 {
            std::fs::remove_dir_all(&dest_root).unwrap();
        }
    })
    .unwrap_err();

    assert!(
        !matches!(err, ApplyError::SourceGone(..)),
        "a missing library was reported as a missing source: {err}"
    );
    assert!(
        f.source_root.join("mazebook.pdf").exists(),
        "the source this refusal did not name is still there"
    );
}

/// A sweep that fails must not take the run with it.
///
/// `supersede` runs after every row has executed, and the caller prints
/// `N placed` from the result — so returning `Err` from here threw away the
/// only statement of how many files had moved. Measured by removing the record
/// store mid-run: all 600 rows landed (the record's descriptor still pointed
/// at an unlinked inode, so every write kept succeeding), and the one line of
/// output was "the record store is unusable".
///
/// The flag is the deterministic version of that window: `UF_IMMUTABLE`
/// refuses the `unlink` with `EPERM` while leaving the directory writable and
/// the file readable, so the run reaches the sweep exactly as it did and the
/// sweep is the only thing that fails.
#[test]
fn a_sweep_that_cannot_delete_still_reports_what_the_run_placed() {
    let f = two_plans_one_root_pair();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let a = apply(&lock, &f.header_a, &f.rows_a, &f.records, false, &mut |_| {})
        .unwrap()
        .record
        .unwrap();
    let _sealed = Immutable::set(&a);

    let mut said: Vec<String> = Vec::new();
    let b = apply(&lock, &f.header_b, &f.rows_b, &f.records, false, &mut |p| {
        if p.total == 0 {
            said.push(p.current.clone());
        }
    })
    .expect("a failed sweep discarded a run that had placed every row");

    assert_eq!(b.executed, 1, "the row did run: {b:?}");
    assert!(b.record.is_some(), "and it was recorded");
    assert!(
        said.iter().any(|m| m.contains("could not be tidied")),
        "the sweep failed in silence: {said:?}"
    );
    assert!(a.exists(), "the record that could not be deleted is still there");
}

/// Clears `UF_IMMUTABLE` on drop, so a panicking assertion cannot leave a file
/// the `TempDir` is then unable to remove.
struct Immutable(PathBuf);

impl Immutable {
    fn set(path: &Path) -> Immutable {
        Immutable::chflags(path, libc::UF_IMMUTABLE);
        Immutable(path.to_path_buf())
    }

    fn chflags(path: &Path, flags: libc::c_uint) {
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path for the call.
        let rc = unsafe { libc::chflags(name.as_ptr(), flags) };
        assert_eq!(rc, 0, "chflags {flags:#x} on {}", path.display());
    }
}

impl Drop for Immutable {
    fn drop(&mut self) {
        Immutable::chflags(&self.0, 0);
    }
}

/// A record for another library is not this run's to delete, whatever order
/// the two runs happened in.
#[test]
fn a_run_does_not_supersede_another_librarys_record() {
    let mine = fixture();
    let theirs = fixture();
    // One store, two libraries — which is what the real store is.
    let records = mine.records.clone();

    let first = {
        let lock = Lock::library(&records, &theirs.dest_root).unwrap();
        apply(
            &lock,
            &theirs.header,
            &theirs.rows,
            &records,
            false,
            &mut |_| {},
        )
        .unwrap()
        .record
        .unwrap()
    };
    let lock = Lock::library(&records, &mine.dest_root).unwrap();
    let second = apply(&lock, &mine.header, &mine.rows, &records, false, &mut |_| {})
        .unwrap()
        .record
        .unwrap();

    assert!(first.exists(), "another library's record was deleted");
    assert!(second.exists());
}

/// `record::supersede` compares roots by exact `PathBuf` equality, so the
/// record and the call site have to agree on one spelling — and agreeing on
/// the canonical one is what makes two runs of the same library match when a
/// plan reaches it through a symlink.
///
/// macOS puts a `TempDir` under `/var`, which is itself a symlink, so this is
/// not a contrived path: it is the shape every other test here already has.
#[test]
fn a_library_reached_through_a_symlink_supersedes_its_own_earlier_run() {
    let f = two_plans_one_root_pair();
    let aka = f.dest_root.with_file_name("library-link");
    std::os::unix::fs::symlink(&f.dest_root, &aka).unwrap();

    // Plan A names the real directory, plan B the symlink. One library.
    let header_b = PlanHeader {
        dest_root: aka.clone(),
        ..f.header_b.clone()
    };

    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let a = apply(&lock, &f.header_a, &f.rows_a, &f.records, false, &mut |_| {}).unwrap();
    let b = apply(&lock, &header_b, &f.rows_b, &f.records, false, &mut |_| {}).unwrap();

    let (a, b) = (a.record.unwrap(), b.record.unwrap());
    assert!(
        !a.exists(),
        "the two runs did not recognise one library, so a run's predecessor \
         was left behind and `undo` can now reverse either"
    );
    // And the stored spelling is the canonical one, not the symlink the plan
    // named: that is what the comparison above depends on.
    let summary = comic_renamer_core::undo::summarise(&b).unwrap();
    assert_eq!(
        summary.dest_root,
        std::fs::canonicalize(&aka).unwrap(),
        "the record stored the path the plan named rather than the library"
    );
}

// ---------------------------------------------------------------------------
// The library lock
// ---------------------------------------------------------------------------

/// The successor to `apply_and_undo_share_one_lock`, which pinned the per-plan
/// lock this task deleted.
#[test]
fn apply_and_reverse_share_one_library_lock() {
    let f = fixture();
    let held = Lock::library(&f.records, &f.dest_root).unwrap();
    assert!(
        matches!(
            Lock::library(&f.records, &f.dest_root),
            Err(ApplyError::Locked)
        ),
        "a second run took the lock while the first held it"
    );
    drop(held);

    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let r = run(&f, &lock).unwrap();
    assert_eq!(r.executed, 1);
    assert_eq!(
        reverse_run(&lock, &r.record.unwrap()).unwrap().restored,
        1,
        "a reverse under the same held lock deadlocked or refused"
    );
    assert!(f.source_root.join("mazebook.pdf").exists());
}

/// A lock for one library does not license a run against another. Without
/// this the type says "locked" while the library this run writes to is
/// unlocked, and two runs would interleave on it — each sweeping the other's
/// record under a lock neither of them holds.
#[test]
fn a_lock_for_another_library_is_refused() {
    let mine = fixture();
    let theirs = fixture();
    let wrong = Lock::library(&mine.records, &theirs.dest_root).unwrap();

    let err = {
        let _guard = TreeGuard::new(&[&mine.source_root, &mine.dest_root]);
        apply(
            &wrong,
            &mine.header,
            &mine.rows,
            &mine.records,
            false,
            &mut |_| {},
        )
        .unwrap_err()
    };
    assert!(matches!(err, ApplyError::WrongLibrary { .. }), "{err:?}");

    // And the right lock runs, so the refusal is about identity and not about
    // the fixture being unrunnable.
    let right = Lock::library(&mine.records, &mine.dest_root).unwrap();
    assert_eq!(run(&mine, &right).unwrap().executed, 1);
}

/// And the same on the way back. `reverse_run` took `&Lock` and never looked
/// at it, so the obligation the type carries was a convention on that path:
/// a reverse of this library could run under a lock for somewhere else, which
/// is the state the check above exists to refuse.
///
/// The other library is a **sibling** of this one, so the comparison has to be
/// the whole path. Two libraries in separate temporary directories differ in
/// their parents too, and a check that only compared those would pass.
#[test]
fn a_reverse_under_another_librarys_lock_is_refused() {
    let mine = fixture();
    let right = Lock::library(&mine.records, &mine.dest_root).unwrap();
    let record = run(&mine, &right).unwrap().record.unwrap();
    drop(right);

    let sibling = mine.dir.path().join("library-b");
    std::fs::create_dir_all(&sibling).unwrap();
    assert_eq!(sibling.parent(), mine.dest_root.parent(), "not actually siblings");
    let wrong = Lock::library(&mine.records, &sibling).unwrap();
    let err = {
        let _guard = TreeGuard::new(&[&mine.source_root, &mine.dest_root]);
        reverse_run(&wrong, &record).unwrap_err()
    };
    assert!(matches!(err, ApplyError::WrongLibrary { .. }), "{err:?}");
    assert!(record.exists(), "a refused reverse must not delete the record");

    // And the record's own library reverses it, so the refusal is identity and
    // not an unreversible fixture.
    let right = Lock::library(&mine.records, &mine.dest_root).unwrap();
    assert_eq!(reverse_run(&right, &record).unwrap().restored, 1);
}

/// The same library reached by another name is the same library: both sides
/// of the comparison are canonical, so a plan naming a symlink still runs
/// under a lock taken on the real directory.
#[test]
fn a_lock_on_the_real_library_covers_a_plan_naming_a_symlink_to_it() {
    let f = fixture();
    let aka = f.dest_root.with_file_name("library-link");
    std::os::unix::fs::symlink(&f.dest_root, &aka).unwrap();
    let header = PlanHeader {
        dest_root: aka,
        ..f.header.clone()
    };

    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let r = apply(&lock, &header, &f.rows, &f.records, false, &mut |_| {}).unwrap();
    assert_eq!(r.executed, 1, "one library read as two: {r:?}");
}

/// Two libraries are two locks: one run must not stop another library's.
#[test]
fn two_libraries_in_one_store_do_not_block_each_other() {
    let mine = fixture();
    let theirs = fixture();
    let _held = Lock::library(&mine.records, &mine.dest_root).unwrap();
    Lock::library(&mine.records, &theirs.dest_root).expect("a different library was blocked");
}

/// The lock file on disk and the library the lock reports are one answer.
///
/// `Lock::library` used to canonicalise `dest_root` twice — once inside
/// `lock_name` to choose the file, once for the field `apply` then compares
/// against the plan. Nothing checked that the two agreed, so a root whose
/// resolution changed in between locked one library while naming another and
/// returned `Ok`: the library actually written was unlocked for the whole run.
/// Measured at 65 of 300 attempts against a symlink being repointed.
///
/// A race cannot be made deterministic, so this asserts the invariant on every
/// attempt instead of waiting for one to land. Correct code cannot fail it at
/// any iteration count; the two-resolution version fails as soon as the flip
/// lands between the two calls.
#[test]
fn the_lock_file_is_always_the_one_for_the_library_the_lock_names() {
    let dir = tempdir().unwrap();
    let records = dir.path().join("runs");
    std::fs::create_dir_all(&records).unwrap();
    let (a, b) = (dir.path().join("lib-a"), dir.path().join("lib-b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    // The root a run is given: a symlink, which is how a `current`-style
    // library root behaves when something repoints it.
    let root = dir.path().join("current");
    std::os::unix::fs::symlink(&a, &root).unwrap();

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flipper = {
        let (stop, dir, a, b) = (stop.clone(), dir.path().to_path_buf(), a.clone(), b.clone());
        std::thread::spawn(move || {
            let mut to_b = true;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                // `rename` over a symlink is atomic, so the root always
                // resolves to one library or the other, never to nothing.
                let next = dir.join("next");
                let _ = std::fs::remove_file(&next);
                let target = if to_b { &b } else { &a };
                if std::os::unix::fs::symlink(target, &next).is_ok() {
                    let _ = std::fs::rename(&next, dir.join("current"));
                }
                to_b = !to_b;
            }
        })
    };

    let mut taken = 0;
    for _ in 0..4_000 {
        // Clear the store so the file that appears is this attempt's own.
        for entry in std::fs::read_dir(&records).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        let Ok(lock) = Lock::library(&records, &root) else {
            continue;
        };
        taken += 1;
        let want = comic_renamer_core::record::lock_name_of(lock.dest_root());
        let got: Vec<String> = std::fs::read_dir(&records)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            got,
            vec![want],
            "the lock names {} but that is not the file it locked",
            lock.dest_root().display()
        );
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    flipper.join().unwrap();
    assert!(taken > 0, "no lock was ever acquired, so nothing was checked");
}

#[test]
fn a_lock_file_left_by_a_killed_run_does_not_block() {
    // flock, not O_EXCL: the kernel releases the lock when its owner dies. An
    // O_EXCL lock file survived a SIGKILL and had to be deleted by hand — and
    // a crash is exactly when undo is needed, so the stale lock blocked the
    // one recovery path.
    let f = fixture();
    let name = comic_renamer_core::record::lock_name(&f.dest_root).unwrap();
    std::fs::write(f.records.join(name), b"leftover from a killed run").unwrap();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    assert_eq!(run(&f, &lock).unwrap().executed, 1);
}

#[test]
fn the_lock_is_released_when_it_is_dropped() {
    let f = fixture();
    let first = Lock::library(&f.records, &f.dest_root).unwrap();
    drop(first);
    // The file itself persists, which is what makes a killed run recoverable;
    // a second acquire is what proves the lock did not.
    Lock::library(&f.records, &f.dest_root).expect("the lock outlived its holder");
}

// ---------------------------------------------------------------------------
// The record's lifecycle
// ---------------------------------------------------------------------------

#[test]
fn a_reversed_record_is_deleted_not_renamed_aside() {
    let f = fixture();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let r = run(&f, &lock).unwrap();
    let record = r.record.unwrap();
    assert!(reverse_run(&lock, &record).unwrap().refused.is_empty());
    assert!(!record.exists(), "a reversed record was kept");
    let aside = record.with_extension("record.reversed");
    assert!(!aside.exists(), "a reversed record was renamed aside");
    // Nothing of it left under any name: the store is empty again.
    assert!(
        comic_renamer_core::record::list(&f.records)
            .unwrap()
            .is_empty(),
        "{:?}",
        comic_renamer_core::record::list(&f.records).unwrap()
    );
}

/// A reverse that refused something keeps the record, because the rows it
/// refused are still in the library and still need a way back.
#[test]
fn a_record_survives_a_reverse_that_refused_a_row() {
    let f = fixture();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let r = run(&f, &lock).unwrap();
    let record = r.record.unwrap();
    // Something else is at the original name now, so the row cannot go back.
    std::fs::write(f.source_root.join("mazebook.pdf"), b"squatter").unwrap();

    let report = reverse_run(&lock, &record).unwrap();
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert!(record.exists(), "the record of a half-reversed run was deleted");

    // Clear the squatter and the record is still good for a second pass.
    std::fs::remove_file(f.source_root.join("mazebook.pdf")).unwrap();
    assert_eq!(reverse_run(&lock, &record).unwrap().restored, 1);
    assert!(!record.exists());
}

#[test]
fn reversing_a_record_that_is_gone_is_not_an_error() {
    let f = fixture();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let absent = f.records.join(".comic-renamer.1.1.1.record");
    assert_eq!(
        reverse_run(&lock, &absent).unwrap(),
        comic_renamer_core::types::UndoReport::default()
    );
}

/// Review Focus 2, reached through `apply` rather than through `record`: a
/// record is live while its run is writing it, which is what stops another
/// run's sweep from taking it.
///
/// Observed from inside the run, through the progress callback. Afterwards is
/// too late — the hold is released when the run ends, so a held record and an
/// unheld one read identically and an assertion made then passes either way.
#[test]
fn the_record_is_held_while_the_run_is_writing_it() {
    // Two rows, so a row's progress fires after the record exists.
    let f = build(&["mazebook.pdf", "birdking_vol1.pdf"], Mode::Move).0;
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let mut seen = Vec::new();
    let r = apply(&lock, &f.header, &f.rows, &f.records, false, &mut |_| {
        for path in comic_renamer_core::record::list(&f.records).unwrap() {
            seen.push(comic_renamer_core::record::is_live(&path).unwrap());
        }
    })
    .unwrap();
    let record = r.record.unwrap();

    assert!(
        !seen.is_empty(),
        "no progress call saw a record, so this observed nothing"
    );
    assert!(
        seen.iter().all(|&live| live),
        "a record its own run was still writing read as abandoned, so another \
         run's sweep would delete it mid-write: {seen:?}"
    );
    // And the hold is released when the run ends, which is what lets a later
    // run sweep it.
    assert!(
        !comic_renamer_core::record::is_live(&record).unwrap(),
        "the record stayed held after the run that wrote it ended"
    );
}

/// `check` writes nothing, so it needs neither the lock nor the store — and a
/// store it never touches cannot be the reason it refuses.
#[test]
fn check_needs_no_record_store() {
    let f = fixture();
    let guard = TreeGuard::new(&[&f.source_root, &f.dest_root]);
    let r = comic_renamer_core::apply::check(&f.header, &f.rows, false, &mut |_| {}).unwrap();
    assert_eq!((r.executed, r.skipped), (1, 0));
    assert_eq!(r.record, None);
    drop(guard);
    assert!(
        comic_renamer_core::record::list(&f.records)
            .unwrap()
            .is_empty(),
        "check wrote a record"
    );
}

/// The record is this run's own, not a name derived from anything a caller
/// supplied — which is what `undo` no longer having to be handed a path rests
/// on.
#[test]
fn a_record_is_named_by_the_store_and_lives_in_it() {
    let f = fixture();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let record = run(&f, &lock).unwrap().record.unwrap();
    assert_eq!(record.parent(), Some(f.records.as_path()));
    assert_eq!(
        comic_renamer_core::record::list(&f.records).unwrap(),
        vec![record.clone()],
        "the store does not recognise the name the run wrote"
    );
    assert_eq!(
        Path::new(record.file_name().unwrap())
            .extension()
            .and_then(|e| e.to_str()),
        Some("record")
    );
}

/// Every field `execute` writes has to be the row's own, and a subtitle is
/// the one that had nothing watching it: no fixture in the suite carried one,
/// so `subtitle: None` in `execute` passed the entire workspace while the
/// records it wrote were useless for any bundle whose titles have subtitles.
///
/// The blast radius is the same as the `series` half of the same defect.
/// `undo::reverse` rebuilds the destination from the record's fields, and the
/// preset puts the subtitle in the leaf, so a row that lost it names a path
/// the preset does not produce and is refused — a reverse that restores
/// nothing and reports every row as refused.
#[test]
fn a_record_carries_the_subtitle_its_own_reverse_needs() {
    let f = build(&["hellboy_vol1_seedofdestruction.pdf"], Mode::Move).0;
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();

    // The subtitle is in the name, so losing it is losing the destination.
    let placed = f
        .dest_root
        .join("Hellboy/Hellboy Vol. 01 - Seed of Destruction.pdf");
    let record = run(&f, &lock).unwrap().record.unwrap();
    assert!(placed.exists(), "{:?}", f.rows[0].destination_preview);

    // It is in the record, under the always-on `'` prefix every text field
    // takes, and not merely in the path the row names.
    let text = std::fs::read_to_string(&record).unwrap();
    assert!(
        text.contains(",'Seed of Destruction,"),
        "the subtitle column is empty, so the reverse below is rebuilding \
         from a different title:\n{text}"
    );

    let report = reverse_run(&lock, &record).unwrap();
    assert_eq!(report.restored, 1, "{report:?}");
    assert!(report.refused.is_empty(), "{report:?}");
    assert!(
        f.source_root
            .join("hellboy_vol1_seedofdestruction.pdf")
            .exists(),
        "the subtitled row never came back"
    );
    assert!(!placed.exists());
}

/// `run` is the zero-padding width (D37, D44). It is stored per row precisely
/// because deriving it from a growing mapping table would rename files already
/// placed, so a wrong value renames real files — and it has to survive into the
/// record, because `undo::reverse` rebuilds the padded name from it.
///
/// It takes a run of 100 or more to be observable at all: `pad` is
/// `run.to_string().len().max(2)`, so every shorter run floors to width 2 and
/// `run → 0` changes no path. Every other fixture in the suite is in that
/// range, which is why this field was unpinned by construction until the
/// fixture above existed.
#[test]
fn a_record_carries_the_run_that_set_its_padding() {
    let f = build(&["velvet_vol1.pdf"], Mode::Move).0;
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();

    // Three digits, not two: this is the padding the run decides.
    let padded = f.dest_root.join("Velvet/Velvet Vol. 001.pdf");
    let record = run(&f, &lock).unwrap().record.unwrap();
    assert!(
        padded.exists(),
        "the fixture's run did not reach the name, so nothing below is about \
         the run: {:?}",
        f.rows[0].destination_preview
    );

    // Volume 1, no issue, no subtitle, run 100, extension pdf — the run
    // column itself, not a number that happens to appear in an identity
    // column. The empty slot between the volume and the subtitle is `issue`.
    let text = std::fs::read_to_string(&record).unwrap();
    assert!(
        text.contains(",1,,,100,'pdf,"),
        "the run column is not the row's:\n{text}"
    );

    // And the reverse is the half that matters: it rebuilds `Vol. 001` from
    // the recorded run, and a record carrying 0 would name `Vol. 01` — a path
    // the preset does not produce for this row, refused forever.
    let report = reverse_run(&lock, &record).unwrap();
    assert_eq!(report.restored, 1, "{report:?}");
    assert!(report.refused.is_empty(), "{report:?}");
    assert!(f.source_root.join("velvet_vol1.pdf").exists());
}

/// The mechanism and mode columns are the row's own, asserted where they are
/// written rather than through what they later cause.
///
/// `Row::expected` reads the mechanism in exactly two states: a completed
/// rename, where the source's identity still describes the destination because
/// the inode moved, and an interrupted clone or stream, where nothing does and
/// the row must be refused. Recording a *weaker* mechanism than the truth is
/// caught by any move's round trip — the rename loses that fallback and the
/// reverse refuses every row. The other direction is not reachable from here:
/// a completed clone always writes `placed`, after which `expected` never
/// consults the mechanism again, and the one state where a forged `rename`
/// would be read needs a clone that fails *between* landing and its `placed`
/// write. `perform` marks that path "reasoned, not tested" for the same
/// reason. So the column is pinned at the point of writing.
#[test]
fn a_records_mode_and_mechanism_are_the_rows_own() {
    // Both trees are in one temporary directory, so a move is a rename and a
    // copy is a clone.
    for (mode, expected) in [(Mode::Move, ",move,rename,"), (Mode::Copy, ",copy,clone,")] {
        let f = build(&["mazebook.pdf"], mode).0;
        let lock = Lock::library(&f.records, &f.dest_root).unwrap();
        let record = run(&f, &lock).unwrap().record.unwrap();
        let text = std::fs::read_to_string(&record).unwrap();
        assert!(
            text.contains(expected),
            "a {mode:?} run did not record {expected:?}:\n{text}"
        );
    }
}

/// The record reverses on its own: `reverse_run` is handed no plan, no roots
/// and no preset, so everything it needs has to have been written down.
#[test]
fn a_record_carries_the_fields_its_own_reverse_needs() {
    let f = fixture();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let record = run(&f, &lock).unwrap().record.unwrap();

    let summary = comic_renamer_core::undo::summarise(&record).unwrap();
    assert_eq!(summary.preset, "komga");
    assert_eq!(summary.planned, 1);
    assert_eq!(summary.reversible, 1);
    assert!(!summary.interrupted);
    assert_eq!(
        summary.dest_root,
        std::fs::canonicalize(&f.dest_root).unwrap()
    );

    // And the per-row fields: without them the preset rebuilds a different
    // destination and the row is refused as one this run did not produce.
    let report = comic_renamer_core::undo::reverse(&record).unwrap();
    assert_eq!(report.restored, 1, "{report:?}");
}

/// Task 11's `a_run_that_executes_nothing_does_not_burn_the_plan`, moved here
/// with the store as its subject. The plan is no longer burnable — nothing is
/// named after it — but a run that places nothing still has to leave no
/// record, which is what the sweep above depends on.
#[test]
fn a_run_that_executes_nothing_writes_no_record() {
    let f = fixture();
    std::fs::create_dir(f.dest_root.join("Mazebook")).unwrap();
    std::fs::write(f.dest_root.join("Mazebook/Mazebook.pdf"), b"in the way").unwrap();

    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let r = run(&f, &lock).unwrap();
    assert_eq!((r.executed, r.skipped), (0, 1));
    assert_eq!(r.record, None);
    assert!(
        comic_renamer_core::record::list(&f.records)
            .unwrap()
            .is_empty(),
        "a run that moved nothing still wrote a record"
    );

    // The user clears the collision and re-runs the same plan.
    std::fs::remove_dir_all(f.dest_root.join("Mazebook")).unwrap();
    let r = run(&f, &lock).unwrap();
    assert_eq!(r.executed, 1, "the plan was unusable after a refused run");
    assert!(r.record.is_some());
}

/// `plan::write` is still how a plan reaches `apply`; this is the join between
/// the two, and the only test here that goes through a file.
#[test]
fn a_plan_read_from_disk_runs_and_reverses() {
    let f = fixture();
    let path = f.dir.path().join("plan.jsonl");
    plan::write(&f.header, &f.rows, &path).unwrap();

    let (header, rows, edited) = plan::read_allowing_edits(&path).unwrap();
    assert!(!edited, "a freshly written plan read as edited");
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let r = apply(&lock, &header, &rows, &f.records, false, &mut |_| {}).unwrap();
    assert_eq!(r.executed, 1);
    assert_eq!(
        reverse_run(&lock, &r.record.unwrap()).unwrap().restored,
        1
    );
}

// ---------------------------------------------------------------------------
// D60: an issue has to survive scan, apply and the reverse
// ---------------------------------------------------------------------------

/// The one test that carries an issue through every stage. Two one-line
/// mutations survived the entire workspace before it existed:
/// `scan.rs`'s `issue: resolved.issue` → `None`, and `apply.rs`'s
/// `row.issue` → `None`. The second is the dangerous one — `apply`
/// recomputes the destination from the row, so the file still lands at
/// `Curse 0003.pdf`, but the record carries no issue and every issue row of
/// that run becomes permanently unreversible, with `undo` exiting 0 having
/// restored nothing.
#[test]
fn an_issue_survives_scan_apply_and_the_reverse() {
    let (f, _) = build(&["curse_issue3.pdf"], Mode::Move);
    assert_eq!(f.rows.len(), 1);
    assert_eq!(f.rows[0].issue, Some(3), "scan dropped the issue");

    let placed = f.dest_root.join("Curse/Curse 0003.pdf");
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let result = run(&f, &lock).unwrap();
    assert_eq!(result.executed, 1);
    assert!(placed.exists(), "the padded leaf is not where the preset put it");

    // The decisive half: the record is the only input to a reverse, so an
    // issue that fails to reach it rebuilds the wrong leaf and the row is
    // refused rather than restored.
    let report = reverse_run(&lock, &result.record.unwrap()).unwrap();
    assert_eq!(report.restored, 1, "the record lost the issue: {report:?}");
    assert_eq!(report.refused, Vec::<String>::new());
    assert!(f.source_root.join("curse_issue3.pdf").exists());
    assert!(!placed.exists());
}
