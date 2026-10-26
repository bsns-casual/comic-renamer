use comic_renamer_core::record::{
    any_live, basename, forget, hold, is_live, list, lock_name, supersede,
};
use comic_renamer_core::types::{Mechanism, Mode};
use comic_renamer_core::undo::UndoWriter;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn a_basename_is_unique_per_call_and_carries_the_stamp() {
    let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_793_000_000);
    let (a, b) = (basename(now), basename(now));
    assert_ne!(a, b);
    assert!(a.contains("1793000000000000000") && a.starts_with(".comic-renamer.") && a.ends_with(".record"), "{a}");
}

/// Two instants a millisecond apart, taken with no sleep between them, name
/// records that list in time order. At one-second resolution both stamps were
/// equal and the order fell to the pid.
#[test]
fn records_a_millisecond_apart_list_in_time_order() {
    let tmp = tempfile::tempdir().unwrap();
    let t = UNIX_EPOCH + Duration::from_secs(1_793_000_000);
    // The later one is made first, so creation order cannot be what sorts.
    let later = tmp.path().join(basename(t + Duration::from_millis(1)));
    let earlier = tmp.path().join(basename(t));
    for p in [&later, &earlier] {
        std::fs::write(p, "x").unwrap();
    }
    assert_eq!(list(tmp.path()).unwrap(), vec![later, earlier]);
}

/// Review Focus: a root is a path, not a filename, so it is hashed.
#[test]
fn a_lock_name_is_one_filename_whatever_the_root_looks_like() {
    let dir = tempfile::tempdir().unwrap();
    for leaf in ["My Comics", "Caf\u{e9}", &"x".repeat(200)] {
        let root = dir.path().join(leaf);
        std::fs::create_dir(&root).unwrap();
        let name = lock_name(&root).unwrap();
        assert!(name.len() <= 255 && name.ends_with(".lock"), "{name}");
        assert_eq!(Path::new(&name).components().count(), 1, "{name}");
    }
}

/// Two different strings for one directory give one name; two directories give
/// two. A symlink is the way to get a second spelling on any filesystem.
#[test]
fn a_lock_name_is_a_pure_function_of_the_canonical_path() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("Comics");
    std::fs::create_dir(&real).unwrap();
    let link = dir.path().join("alias");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_ne!(real.to_string_lossy(), link.to_string_lossy());
    assert_eq!(lock_name(&real).unwrap(), lock_name(&link).unwrap());
    let other = dir.path().join("Hellboy");
    std::fs::create_dir(&other).unwrap();
    assert_ne!(lock_name(&real).unwrap(), lock_name(&other).unwrap());
}

/// The canonical path, **not** a case-folded one.
///
/// Canonicalisation is the whole normalisation here, and it is deliberately
/// not followed by a fold: on a case-sensitive volume `Lib` and `lib` are two
/// real libraries, and one lock between them means a run on either blocks the
/// other. The symlink test above pins the canonicalisation; nothing pinned
/// the absence of the fold, and adding `.to_lowercase()` to the hashed string
/// left the whole suite green.
///
/// Pinned at the function rather than with two case-differing directories,
/// because making those needs a case-sensitive volume and this has to hold in
/// CI: the name is asserted to be the hash of the canonical path exactly, and
/// not the hash of its folded form. `Lib` guarantees the two differ.
#[test]
fn a_lock_name_is_not_case_folded() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Lib");
    std::fs::create_dir(&root).unwrap();
    let canonical = root.canonicalize().unwrap();
    let text = canonical.to_string_lossy();
    assert_ne!(text, text.to_lowercase(), "the fixture has to carry a capital");

    let name = lock_name(&root).unwrap();
    let hash = |s: &str| format!("{}.lock", comic_renamer_core::corpus::sha256_hex(s.as_bytes()));
    assert_eq!(name, hash(&text));
    assert_ne!(
        name,
        hash(&text.to_lowercase()),
        "the root was folded before hashing, so two libraries share one lock"
    );
}

#[test]
fn a_canonicalisation_failure_is_an_error_not_a_second_lock() {
    assert!(lock_name(Path::new("/no/such/library")).is_err());
}

/// `flock` attaches to the open file description, so a second open in this
/// same process conflicts. An implementer who reaches for `fcntl`/`F_SETLK`
/// gets per-process semantics and this assertion silently inverts.
#[test]
fn a_held_record_is_live_and_a_released_one_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.record");
    std::fs::write(&path, "x").unwrap();
    assert!(!is_live(&path).unwrap());
    let held = hold(&path).unwrap();
    assert!(is_live(&path).unwrap());
    drop(held);
    assert!(!is_live(&path).unwrap());
}

/// A record nobody can find is held by nobody.
///
/// `menu::decline` deletes records with no library lock, so one can vanish
/// between a listing and the question. `is_live` propagated the `NotFound`,
/// and `supersede` — which `apply` calls after every row has executed — passed
/// it on: a run that had placed all 600 rows exited non-zero and never printed
/// its own `N placed` line.
#[test]
fn a_record_that_is_gone_is_not_live_rather_than_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!is_live(&dir.path().join("never-existed.record")).unwrap());
}

/// The one deletion path. Already gone is the end state every caller wanted,
/// so it succeeds and is not counted; anything else is still an error, because
/// a record that survives a sweep it was supposed to lose is the one the next
/// `undo` picks up.
#[test]
fn forgetting_a_record_that_is_already_gone_is_success_but_not_a_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.record");
    std::fs::write(&path, b"x").unwrap();
    assert!(forget(&path).unwrap(), "it was there, so this removed it");
    assert!(!path.exists());
    assert!(!forget(&path).unwrap(), "gone already, so nothing was removed");

    // And a removal that genuinely cannot happen is not swallowed.
    let locked = dir.path().join("sealed");
    std::fs::create_dir(&locked).unwrap();
    let inside = locked.join("s.record");
    std::fs::write(&inside, b"x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    let kind = forget(&inside).map(|_| ()).unwrap_err().kind();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_ne!(kind, std::io::ErrorKind::NotFound, "an unremovable record was reported gone");
}

/// What tells "nothing to undo" from "a run is writing right now". Without it
/// `undo` reported the first while 156 of 600 files had already moved.
#[test]
fn any_live_sees_a_held_record_and_nothing_else() {
    let f = two_records_for_one_root_pair();
    assert!(!any_live(&f.dir).unwrap(), "no run holds either record");
    let held = hold(&f.older).unwrap();
    assert!(any_live(&f.dir).unwrap());
    drop(held);
    assert!(!any_live(&f.dir).unwrap());
}

/// A record nobody can open is not evidence that a run is in progress.
///
/// `menu::waiting` reads an unanswerable liveness question as *live*, and that
/// is right there, because its question is "should I offer this record" and
/// not offering is the safe answer. This question is the opposite one, so the
/// same default inverts: one unreadable record would have `undo` refuse with
/// "a run is in progress" while `undo --list` says there are no runs, for as
/// long as the file exists and with no way out from inside the tool. The state
/// is constructible — a file at mode `000` cannot be opened by its owner.
#[test]
fn an_unreadable_record_is_not_a_run_in_progress() {
    let f = two_records_for_one_root_pair();
    std::fs::set_permissions(&f.older, std::fs::Permissions::from_mode(0o000)).unwrap();
    let answer = any_live(&f.dir);
    // Restored before the assertion, so a failure cannot leave the TempDir
    // holding a file it is then unable to remove.
    std::fs::set_permissions(&f.older, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        !answer.unwrap(),
        "an unreadable record made `undo` refuse every run forever"
    );
    // And a record that genuinely is held still reads as live, so the safe
    // direction was not bought by making the answer always false.
    let held = hold(&f.newer).unwrap();
    assert!(any_live(&f.dir).unwrap());
    drop(held);
}

/// A record that cannot be opened for write must not make the whole sweep
/// fail. `is_live` answers the question it was asked.
#[test]
fn a_read_only_record_is_not_live_rather_than_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ro.record");
    std::fs::write(&path, "x").unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&path, perms).unwrap();
    assert!(!is_live(&path).unwrap(), "nobody holds it, so it is not live");
}

struct Fixture {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    source_root: PathBuf,
    dest_root: PathBuf,
    older: PathBuf,
    newer: PathBuf,
}

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

/// A **finished** run's record: planned one row, and that row is done.
///
/// The row matters. A header-only record is a shape nothing writes — creation
/// is lazy, so a record always planned at least the row that made it — and
/// since `summarise` learned to read an abandoned run it would also be an
/// *interrupted* record, which `supersede` now keeps. Without the row these
/// fixtures would be swept for having nothing to lose rather than for being
/// finished, and the retention tests below would stop meaning what they say.
fn write_record(dir: &Path, secs: u64, source_root: &Path, dest_root: &Path) -> PathBuf {
    write_run(dir, secs, source_root, dest_root, 1, true)
}

/// A run that placed one row of two and then died: interrupted, and with a row
/// still to put back. This is the record the menu's `l` promises to keep.
fn write_interrupted(dir: &Path, secs: u64, source_root: &Path, dest_root: &Path) -> PathBuf {
    write_run(dir, secs, source_root, dest_root, 2, true)
}

/// A run abandoned at its only row, before anything was created: interrupted,
/// and with nothing to put back.
fn write_failed(dir: &Path, secs: u64, source_root: &Path, dest_root: &Path) -> PathBuf {
    write_run(dir, secs, source_root, dest_root, 1, false)
}

fn write_run(
    dir: &Path,
    secs: u64,
    source_root: &Path,
    dest_root: &Path,
    planned: u32,
    done: bool,
) -> PathBuf {
    let path = dir.join(basename(at(secs)));
    let file = source_root.join("a.pdf");
    std::fs::write(&file, b"x").unwrap();
    let mut writer = UndoWriter::create(&path, source_root, dest_root, "komga", planned).unwrap();
    let seq = writer
        .record_pending(
            Path::new("a.pdf"), Path::new("A/A - Vol. 01.pdf"), Mechanism::Rename, Mode::Move,
            &file, "A", Some(1), None, None, 1, "pdf",
        )
        .unwrap();
    if done {
        writer.record_done(seq).unwrap();
    } else {
        writer.record_failed(seq).unwrap();
    }
    path
}

fn mkdir(path: &Path) -> PathBuf {
    std::fs::create_dir_all(path).unwrap();
    path.canonicalize().unwrap()
}

fn two_records_for_one_root_pair() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let dir = mkdir(&tmp.path().join("records"));
    let source_root = mkdir(&tmp.path().join("source"));
    let dest_root = mkdir(&tmp.path().join("library"));
    let older = write_record(&dir, 1_793_000_000, &source_root, &dest_root);
    let newer = write_record(&dir, 1_793_000_100, &source_root, &dest_root);
    Fixture { _tmp: tmp, dir, source_root, dest_root, older, newer }
}

#[test]
fn supersede_leaves_a_live_record_alone() {
    let f = two_records_for_one_root_pair();
    let held = hold(&f.older).unwrap();
    assert_eq!(supersede(&f.dir, &f.newer, &f.source_root, &f.dest_root).unwrap(), 0);
    assert!(f.older.exists(), "a live run's record is its only way back");
    drop(held);
    assert_eq!(supersede(&f.dir, &f.newer, &f.source_root, &f.dest_root).unwrap(), 1);
    assert!(!f.older.exists() && f.newer.exists());
}

/// Documents why `apply` must not call `supersede` when it created no record:
/// running `apply` twice, where the second run collides on every row, would
/// otherwise delete the first run's record.
#[test]
fn supersede_with_an_absent_keep_would_delete_everything_which_is_why_apply_gates_it() {
    let f = two_records_for_one_root_pair();
    let absent = f.dir.join("nothing");
    assert_eq!(supersede(&f.dir, &absent, &f.source_root, &f.dest_root).unwrap(), 2);
}

#[test]
fn supersede_keeps_a_different_root_pairs_record() {
    let f = two_records_for_one_root_pair();
    let other_source = mkdir(&f.dir.join("..").join("source-b"));
    let b = write_record(&f.dir, 1_793_000_200, &other_source, &f.dest_root);
    assert_eq!(supersede(&f.dir, &b, &other_source, &f.dest_root).unwrap(), 0);
    assert!(f.older.exists() && f.newer.exists(), "disjoint bundles, both reversible");
    // Same source, different library: also a different pair.
    let other_dest = mkdir(&f.dir.join("..").join("library-b"));
    let c = write_record(&f.dir, 1_793_000_300, &f.source_root, &other_dest);
    assert_eq!(supersede(&f.dir, &c, &f.source_root, &other_dest).unwrap(), 0);
    assert!(f.older.exists() && f.newer.exists());
}

/// The menu says of an interrupted run, in these words, "l leave it for now;
/// this stays here until you deal with it". The ordinary recovery sequence —
/// crash, re-scan the same bundle, re-apply into the same library — went
/// straight through that promise: `supersede` saw only the root pair, deleted
/// the crashed run's record, and the rows it had already placed stopped being
/// reversible.
#[test]
fn supersede_keeps_an_interrupted_record_with_rows_still_to_put_back() {
    let f = two_records_for_one_root_pair();
    let crashed = write_interrupted(&f.dir, 1_793_000_050, &f.source_root, &f.dest_root);

    assert_eq!(supersede(&f.dir, &f.newer, &f.source_root, &f.dest_root).unwrap(), 1);
    assert!(crashed.exists(), "the only way back from a half-migrated library");
    assert!(!f.older.exists(), "and a finished run for the same pair is still superseded");
}

/// The other half of the same gate, and why it is not just "interrupted".
/// A run abandoned before it created anything has nothing to put back, so
/// keeping its record would only let a bundle that fails on its first row
/// accumulate one record per attempt — which is the unbounded growth D57
/// exists to stop.
#[test]
fn supersede_still_sweeps_an_interrupted_record_with_nothing_to_put_back() {
    let f = two_records_for_one_root_pair();
    let empty = write_failed(&f.dir, 1_793_000_050, &f.source_root, &f.dest_root);
    assert_eq!(supersede(&f.dir, &f.newer, &f.source_root, &f.dest_root).unwrap(), 2);
    assert!(!empty.exists() && !f.older.exists());
}

/// A record `summarise` refuses cannot be attributed to a root pair, so it is
/// not ours to delete.
#[test]
fn supersede_skips_a_record_it_cannot_read() {
    let f = two_records_for_one_root_pair();
    let garbage = f.dir.join(basename(at(1_793_000_050)));
    std::fs::write(&garbage, "not a record\n").unwrap();
    assert_eq!(supersede(&f.dir, &f.newer, &f.source_root, &f.dest_root).unwrap(), 1);
    assert!(garbage.exists());
    assert!(list(&f.dir).unwrap().contains(&garbage), "list must still show it");
}

#[test]
fn list_is_newest_first_and_ignores_a_junk_file() {
    let f = two_records_for_one_root_pair();
    std::fs::write(f.dir.join("README"), "not a record").unwrap();
    std::fs::write(f.dir.join(".comic-renamer.1.2.record"), "x").unwrap();
    std::fs::write(f.dir.join(".comic-renamer.a.b.c.record"), "x").unwrap();
    std::fs::write(f.dir.join(".comic-renamer.1.2.3.4.record"), "x").unwrap();
    let found = list(&f.dir).unwrap();
    assert_eq!(found, vec![f.newer.clone(), f.older.clone()]);
}

/// A lexical sort puts a 9-digit stamp after a 10-digit one.
#[test]
fn list_orders_by_number_not_by_text() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join(".comic-renamer.999999999.5.0.record");
    let b = tmp.path().join(".comic-renamer.1000000000.5.0.record");
    let c = tmp.path().join(".comic-renamer.1000000000.5.10.record");
    let d = tmp.path().join(".comic-renamer.1000000000.5.9.record");
    for p in [&a, &b, &c, &d] {
        std::fs::write(p, "x").unwrap();
    }
    assert_eq!(list(tmp.path()).unwrap(), vec![c, d, b, a]);
}
