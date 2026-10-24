use comic_renamer_core::record::{basename, hold, is_live, list, lock_name, supersede};
use comic_renamer_core::undo::UndoWriter;
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

fn write_record(dir: &Path, secs: u64, source_root: &Path, dest_root: &Path) -> PathBuf {
    let path = dir.join(basename(at(secs)));
    UndoWriter::create(&path, source_root, dest_root, "komga", 1).unwrap();
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
