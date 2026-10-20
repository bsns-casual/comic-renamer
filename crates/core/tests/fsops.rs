//! Task 9. This is where a mistake destroys a library, so **every test wraps
//! both trees in a `TreeGuard`**: "the function returned `Err`" and "no files
//! moved" are different claims and only the guard makes the second one.
//!
//! Drop order matters here. `TempDir` is declared before the guard so the
//! guard drops first; reversed, it would watch the whole tree vanish.

mod common;

use comic_renamer_core::fsops::{
    clone_excl, collisions, find_collisions, is_dataless, rename_excl, select_mechanism,
    stream_verified,
};
use comic_renamer_core::preset::komga::Komga;
use comic_renamer_core::types::{
    Basis, Mechanism, Mode, PlanRow, SourceStat, Status,
};
use common::{sparse_truncate_only, zero_byte, TreeGuard};
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn inode(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().ino()
}

/// `src` holds SOURCE, `dst` already holds ORIGINAL.
fn two_files_distinct_contents(d: &TempDir) -> (PathBuf, PathBuf) {
    let src = d.path().join("src.pdf");
    let dst = d.path().join("dst.pdf");
    std::fs::write(&src, "SOURCE").unwrap();
    std::fs::write(&dst, "ORIGINAL").unwrap();
    (src, dst)
}

fn row(id: &str, series: &str, volume: Option<u32>, run: u32, ino: u64) -> PlanRow {
    PlanRow {
        id: id.into(),
        source: PathBuf::from(format!("/src/{id}.pdf")),
        // Deliberately a lie: find_collisions must recompute and ignore this.
        destination_preview: PathBuf::from("ignored/by/design.pdf"),
        extension: "pdf".into(),
        series: series.into(),
        volume,
        subtitle: None,
        run,
        status: Status::Ready(Basis::Unambiguous),
        mechanism: Mechanism::Rename,
        source_stat: SourceStat {
            size: 1,
            mtime_ns: 1,
            inode: ino,
        },
        record: None,
    }
}

#[test]
fn rename_excl_refuses_and_leaves_both_files() {
    let d = tempdir().unwrap();
    let (src, dst) = two_files_distinct_contents(&d);
    let _g = TreeGuard::new(&[d.path()]);
    assert_eq!(
        rename_excl(&src, &dst).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(read(&dst), "ORIGINAL");
    assert!(src.exists());
}

#[test]
fn plain_rename_would_have_clobbered() {
    // Pins WHY rename_excl exists; if this fails the platform changed.
    // Deliberately unguarded — this test's whole point is the destruction.
    let d = tempdir().unwrap();
    let (src, dst) = two_files_distinct_contents(&d);
    std::fs::rename(&src, &dst).unwrap();
    assert_eq!(read(&dst), "SOURCE");
    assert!(!src.exists());
}

#[test]
fn rename_excl_succeeds_onto_a_free_path() {
    let d = tempdir().unwrap();
    let src = d.path().join("src.pdf");
    let dst = d.path().join("free.pdf");
    std::fs::write(&src, "SOURCE").unwrap();
    let g = TreeGuard::new(&[d.path()]).expect_changed(&[&src, &dst]);
    rename_excl(&src, &dst).unwrap();
    assert_eq!(read(&dst), "SOURCE");
    drop(g);
}

#[test]
fn clone_excl_refuses_and_is_a_distinct_inode() {
    let d = tempdir().unwrap();
    let src = d.path().join("src.pdf");
    let dst = d.path().join("clone.pdf");
    std::fs::write(&src, "SOURCE").unwrap();
    let g = TreeGuard::new(&[d.path()]).expect_changed(&[&dst]);
    clone_excl(&src, &dst).unwrap();
    assert_ne!(inode(&src), inode(&dst));
    assert_eq!(
        clone_excl(&src, &dst).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    drop(g);
}

#[test]
fn dataless_ignores_sparse_and_empty_files() {
    // D25, D21
    let d = tempdir().unwrap();
    let sparse = sparse_truncate_only(d.path(), "t.bin", 5 << 30);
    let empty = zero_byte(d.path(), "m.pdf");
    let _g = TreeGuard::new(&[d.path()]);
    assert!(!is_dataless(&sparse).unwrap());
    assert!(!is_dataless(&empty).unwrap());
}

#[test]
fn detects_case_nfc_and_quote_collisions() {
    // D2, §6.10
    assert_eq!(collisions(&["Hellboy/x.pdf", "hellboy/x.pdf"]).len(), 1);
    assert_eq!(
        collisions(&["Pok\u{e9}mon/a.pdf", "Poke\u{301}mon/a.pdf"]).len(),
        1
    );
    assert_eq!(
        collisions(&["Bourdain's/a.pdf", "Bourdain\u{2019}s/a.pdf"]).len(),
        1
    );
    assert!(collisions(&["Hellboy/x.pdf", "Hellboy/y.pdf"]).is_empty());
}

#[test]
fn two_spellings_of_one_volume_are_one_filename_and_two_rows() {
    // The destinations are recomputed, so `vol1` and `volume1` land on the
    // same name even though the source stems differ.
    let d = tempdir().unwrap();
    let rows = [
        row("a", "Some Series", Some(1), 9, 1),
        row("b", "Some Series", Some(1), 9, 2),
    ];
    let _g = TreeGuard::new(&[d.path()]);
    assert_eq!(find_collisions(&rows, d.path(), &Komga).len(), 1);
}

#[test]
fn collides_with_an_existing_destination_folder_and_file() {
    // Review Focus 3
    let d = tempdir().unwrap();
    std::fs::create_dir(d.path().join("Hellboy")).unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    // A row whose series folder differs only in case from one already there.
    let rows = [row("a", "hellboy", None, 1, 1)];
    assert_eq!(find_collisions(&rows, d.path(), &Komga).len(), 1);

    let d2 = tempdir().unwrap();
    std::fs::create_dir(d2.path().join("Hellboy")).unwrap();
    std::fs::write(d2.path().join("Hellboy/Hellboy.pdf"), b"x").unwrap();
    let _g2 = TreeGuard::new(&[d2.path()]);
    let rows = [row("a", "Hellboy", None, 1, 1)];
    assert_eq!(find_collisions(&rows, d2.path(), &Komga).len(), 1);
}

#[test]
fn an_empty_destination_has_no_collisions() {
    let d = tempdir().unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    let rows = [row("a", "Hellboy", None, 1, 1), row("b", "Witch", None, 1, 2)];
    assert!(find_collisions(&rows, d.path(), &Komga).is_empty());
}

#[test]
fn find_collisions_ignores_the_stored_preview() {
    // D14: a hand-edited destination_preview must not steer detection.
    let d = tempdir().unwrap();
    let mut rows = [
        row("a", "Hellboy", None, 1, 1),
        row("b", "Hellboy", None, 1, 2),
    ];
    rows[0].destination_preview = PathBuf::from("Totally/Different.pdf");
    rows[1].destination_preview = PathBuf::from("Also/Different.pdf");
    let _g = TreeGuard::new(&[d.path()]);
    assert_eq!(find_collisions(&rows, d.path(), &Komga).len(), 1);
}

#[test]
fn refuses_a_symlinked_destination_component() {
    // §6.11
    let d = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let src = d.path().join("src.pdf");
    std::fs::write(&src, "SOURCE").unwrap();
    std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap();
    let _g = TreeGuard::new(&[d.path(), outside.path()]);
    assert!(stream_verified(&src, &d.path().join("link/escaped.pdf"), d.path()).is_err());
}

#[test]
fn stream_verified_copies_and_leaves_the_source() {
    // It never unlinks: apply owns that, after verification.
    let d = tempdir().unwrap();
    let src = d.path().join("src.pdf");
    let dst = d.path().join("out.pdf");
    std::fs::write(&src, "SOURCE").unwrap();
    let g = TreeGuard::new(&[d.path()]).expect_changed(&[&dst]);
    stream_verified(&src, &dst, d.path()).unwrap();
    assert_eq!(read(&dst), "SOURCE");
    assert!(src.exists());
    drop(g);
}

#[test]
fn stream_verified_refuses_an_occupied_destination() {
    let d = tempdir().unwrap();
    let (src, dst) = two_files_distinct_contents(&d);
    let _g = TreeGuard::new(&[d.path()]);
    assert_eq!(
        stream_verified(&src, &dst, d.path()).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(read(&dst), "ORIGINAL");
}

#[test]
fn never_follows_a_symlink_in_the_source() {
    let d = tempdir().unwrap();
    let real = d.path().join("real.pdf");
    std::fs::write(&real, "SOURCE").unwrap();
    let link = d.path().join("link.pdf");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    assert!(select_mechanism(&link, &d.path().join("out.pdf"), Mode::Move).is_err());
}

#[test]
fn a_same_volume_move_is_a_rename() {
    let d = tempdir().unwrap();
    let src = d.path().join("src.pdf");
    std::fs::write(&src, "SOURCE").unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    assert_eq!(
        select_mechanism(&src, &d.path().join("out.pdf"), Mode::Move).unwrap(),
        Mechanism::Rename
    );
}

#[test]
fn two_rows_sharing_an_inode_are_reported_not_executed() {
    let d = tempdir().unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    // Distinct destinations, one underlying file: a move would execute the
    // first and leave the second pointing at nothing.
    let rows = [
        row("a", "Hellboy", Some(1), 9, 77),
        row("b", "Witch", Some(1), 9, 77),
    ];
    assert_eq!(find_collisions(&rows, d.path(), &Komga).len(), 1);
}

#[test]
fn stream_verified_refuses_a_destination_outside_the_root() {
    // The scoped containment check doubles as the containment check.
    let d = tempdir().unwrap();
    let elsewhere = tempdir().unwrap();
    let src = d.path().join("src.pdf");
    std::fs::write(&src, "SOURCE").unwrap();
    let _g = TreeGuard::new(&[d.path(), elsewhere.path()]);
    assert!(stream_verified(&src, &elsewhere.path().join("out.pdf"), d.path()).is_err());
}
