//! Task 8. The harness is the deliverable: without it a refusal test proves
//! only that a function returned `Err`, not that the tree is untouched.

mod common;

use common::{blocks, sparse_seek_write, sparse_truncate_only, zero_byte, TreeGuard};
use std::os::unix::fs::MetadataExt;
use tempfile::tempdir;

#[test]
fn sparse_variants_differ_in_blocks() {
    let d = tempdir().unwrap();
    assert!(blocks(&sparse_seek_write(d.path(), "a.bin", 5 << 30)) > 0);
    assert_eq!(blocks(&sparse_truncate_only(d.path(), "b.bin", 5 << 30)), 0);
}

#[test]
fn both_sparse_variants_report_the_same_logical_size() {
    // They are distinguished by st_blocks alone; if st_size diverged, the
    // dataless predicate could cheat by reading it instead (D25).
    let d = tempdir().unwrap();
    let a = sparse_seek_write(d.path(), "a.bin", 5 << 30);
    let b = sparse_truncate_only(d.path(), "b.bin", 5 << 30);
    assert_eq!(
        std::fs::metadata(&a).unwrap().len(),
        std::fs::metadata(&b).unwrap().len()
    );
}

#[test]
fn a_zero_byte_file_is_a_fixture_not_a_fault() {
    // Invariant 3: the fixture suite is built from these.
    let d = tempdir().unwrap();
    assert_eq!(std::fs::metadata(zero_byte(d.path(), "m.pdf")).unwrap().len(), 0);
}

#[test]
#[should_panic(expected = "unexpected change")]
fn guard_fails_when_a_file_appears() {
    let d = tempdir().unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    std::fs::write(d.path().join("surprise"), b"x").unwrap();
}

#[test]
#[should_panic(expected = "unexpected change")]
fn guard_fails_when_a_file_vanishes() {
    let d = tempdir().unwrap();
    let victim = d.path().join("gone");
    std::fs::write(&victim, b"x").unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    std::fs::remove_file(&victim).unwrap();
}

#[test]
#[should_panic(expected = "unexpected change")]
fn guard_fails_on_a_same_size_rewrite() {
    // The case a length-only snapshot misses, and the reason the fingerprint
    // samples content.
    let d = tempdir().unwrap();
    let path = d.path().join("f");
    std::fs::write(&path, b"SOURCE").unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    std::fs::write(&path, b"TARGET").unwrap();
}

#[test]
fn guard_permits_a_declared_creation() {
    let d = tempdir().unwrap();
    let g = TreeGuard::new(&[d.path()]).expect_created(&[&d.path().join("ok")]);
    std::fs::write(d.path().join("ok"), b"x").unwrap();
    drop(g);
}

#[test]
#[should_panic(expected = "expected created")]
fn permitting_a_creation_does_not_permit_a_deletion() {
    // The hole the old expect_changed had: a path named so the guard would
    // tolerate it appearing became exempt from vanishing too.
    let d = tempdir().unwrap();
    let victim = d.path().join("victim");
    std::fs::write(&victim, b"x").unwrap();
    let g = TreeGuard::new(&[d.path()]).expect_created(&[&victim]);
    std::fs::remove_file(&victim).unwrap();
    drop(g);
}

#[test]
#[should_panic(expected = "expected created, never appeared")]
fn a_declared_creation_that_never_happens_fails() {
    let d = tempdir().unwrap();
    let _g = TreeGuard::new(&[d.path()]).expect_created(&[&d.path().join("promised")]);
}

#[test]
fn guard_catches_a_clone_and_replace_that_preserves_mtime() {
    // clonefile preserves mtime to the nanosecond, so a swap under a path is
    // invisible to a fingerprint without the inode. This is the gap that
    // mattered most in a project whose invariants are about inodes.
    let d = tempdir().unwrap();
    let path = d.path().join("f");
    std::fs::write(&path, b"IDENTICAL").unwrap();
    let before = std::fs::metadata(&path).unwrap();

    let guard = TreeGuard::new(&[d.path()]);
    let copy = d.path().join("tmp");
    std::fs::write(&copy, b"IDENTICAL").unwrap();
    filetime::set_file_mtime(&copy, filetime::FileTime::from_last_modification_time(&before))
        .unwrap();
    std::fs::rename(&copy, &path).unwrap();

    let after = std::fs::metadata(&path).unwrap();
    assert_ne!(before.ino(), after.ino(), "test did not actually swap the file");
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(guard)))
            .is_err(),
        "guard missed an identity swap"
    );
}

#[test]
fn guard_passes_when_nothing_happens() {
    let d = tempdir().unwrap();
    std::fs::write(d.path().join("quiet"), b"x").unwrap();
    let _g = TreeGuard::new(&[d.path()]);
}
