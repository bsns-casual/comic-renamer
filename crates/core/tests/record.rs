use comic_renamer_core::record::{basename, hold, is_live, lock_name};
use std::path::Path;

#[test]
fn a_basename_is_unique_per_call_and_carries_the_stamp() {
    let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_793_000_000);
    let (a, b) = (basename(now), basename(now));
    assert_ne!(a, b);
    assert!(a.contains("1793000000") && a.starts_with(".comic-renamer.") && a.ends_with(".record"), "{a}");
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
