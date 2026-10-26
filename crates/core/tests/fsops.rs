//! Task 9. This is where a mistake destroys a library, so **every test wraps
//! both trees in a `TreeGuard`**: "the function returned `Err`" and "no files
//! moved" are different claims and only the guard makes the second one.
//!
//! Drop order matters here. `TempDir` is declared before the guard so the
//! guard drops first; reversed, it would watch the whole tree vanish.
//!
//! Where a test can escape the destination, the guard covers the escape
//! target too — a file arriving outside every guarded root is invisible.

mod common;

use comic_renamer_core::fsops::{
    clone_excl, find_collisions, is_dataless, rename_excl, select_mechanism, stream_verified,
};
use comic_renamer_core::preset::komga::Komga;
use comic_renamer_core::types::{Mechanism, Mode, PlanRow, SourceStat, Status};
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

fn rel(p: &str) -> PathBuf {
    PathBuf::from(p)
}

/// A source file and a destination root that already holds `dst.pdf`.
fn occupied_root(d: &TempDir) -> (PathBuf, PathBuf) {
    let src = d.path().join("src.pdf");
    let root = d.path().join("dest");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(&src, "SOURCE").unwrap();
    std::fs::write(root.join("dst.pdf"), "ORIGINAL").unwrap();
    (src, root)
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
        issue: None,
        subtitle: None,
        run,
        status: Status::Mapped,
        mechanism: Mechanism::Rename,
        source_stat: SourceStat {
            size: 1,
            mtime_ns: 1,
            inode: ino,
        },
        record: None,
    }
}

// ---------------------------------------------------------------------------
// No-clobber, per mechanism
// ---------------------------------------------------------------------------

#[test]
fn rename_excl_refuses_and_leaves_both_files() {
    let d = tempdir().unwrap();
    let (src, root) = occupied_root(&d);
    let _g = TreeGuard::new(&[d.path()]);
    assert_eq!(
        rename_excl(&src, &root, &rel("dst.pdf")).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(read(&root.join("dst.pdf")), "ORIGINAL");
    assert!(src.exists());
}

#[test]
fn plain_rename_would_have_clobbered() {
    // Pins WHY rename_excl exists; if this fails the platform changed.
    // Deliberately unguarded — this test's whole point is the destruction.
    let d = tempdir().unwrap();
    let (src, root) = occupied_root(&d);
    std::fs::rename(&src, root.join("dst.pdf")).unwrap();
    assert_eq!(read(&root.join("dst.pdf")), "SOURCE");
    assert!(!src.exists());
}

#[test]
fn rename_excl_succeeds_onto_a_free_path() {
    let d = tempdir().unwrap();
    let (src, root) = occupied_root(&d);
    let dst = root.join("free.pdf");
    let g = TreeGuard::new(&[d.path()])
        .expect_removed(&[&src])
        .expect_created(&[&dst]);
    rename_excl(&src, &root, &rel("free.pdf")).unwrap();
    assert_eq!(read(&dst), "SOURCE");
    assert!(!src.exists(), "a move must not leave the source behind");
    drop(g);
}

#[test]
fn clone_excl_refuses_and_is_a_distinct_inode() {
    let d = tempdir().unwrap();
    let (src, root) = occupied_root(&d);
    let dst = root.join("clone.pdf");
    let g = TreeGuard::new(&[d.path()]).expect_created(&[&dst]);
    clone_excl(&src, &root, &rel("clone.pdf")).unwrap();
    assert_ne!(inode(&src), inode(&dst));
    assert_eq!(
        clone_excl(&src, &root, &rel("clone.pdf")).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    drop(g);
}

#[test]
fn clone_excl_refuses_a_dangling_symlink_destination() {
    // Without CLONE_NOFOLLOW_ANY, clonefile follows the link, creates the
    // target and returns Ok — the one mechanism of three for which
    // no-clobber did not hold (invariant 6).
    let d = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let (src, root) = occupied_root(&d);
    std::os::unix::fs::symlink(outside.path().join("escaped.pdf"), root.join("link.pdf")).unwrap();
    let _g = TreeGuard::new(&[d.path(), outside.path()]);
    assert_eq!(
        clone_excl(&src, &root, &rel("link.pdf")).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert!(!outside.path().join("escaped.pdf").exists());
}

// ---------------------------------------------------------------------------
// Containment: every mechanism, not just the one selected cross-volume
// ---------------------------------------------------------------------------

/// `dest_root/Hellboy` is a symlink to a directory on the same volume.
fn symlinked_series_folder(d: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
    let src = d.path().join("src.pdf");
    let root = d.path().join("dest");
    let elsewhere = d.path().join("elsewhere");
    std::fs::write(&src, "SOURCE").unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("Hellboy")).unwrap();
    (src, root, elsewhere)
}

#[test]
fn no_mechanism_writes_through_a_symlinked_series_folder() {
    // §6.11 names this attack. Same volume, so Move selects Rename and Copy
    // selects Clone — neither of which used to be checked at all.
    for mechanism in ["rename", "clone", "stream"] {
        let d = tempdir().unwrap();
        let (src, root, elsewhere) = symlinked_series_folder(&d);
        let _g = TreeGuard::new(&[d.path()]);
        let target = rel("Hellboy/book.pdf");
        let result = match mechanism {
            "rename" => rename_excl(&src, &root, &target),
            "clone" => clone_excl(&src, &root, &target),
            _ => stream_verified(&src, &root, &target),
        };
        assert!(result.is_err(), "{mechanism} wrote through a symlink");
        assert!(
            !elsewhere.join("book.pdf").exists(),
            "{mechanism} escaped the destination root"
        );
        assert!(src.exists(), "{mechanism} consumed the source");
    }
}

#[test]
fn no_mechanism_accepts_a_parent_dir_component() {
    // `components()` does not resolve `..`, so a lexical containment check
    // accepts root/../sibling. These are not lexical checks.
    let d = tempdir().unwrap();
    let (src, root) = occupied_root(&d);
    let sibling = d.path().join("sibling");
    std::fs::create_dir(&sibling).unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    let escape = rel("../sibling/escaped.pdf");
    assert!(rename_excl(&src, &root, &escape).is_err());
    assert!(clone_excl(&src, &root, &escape).is_err());
    assert!(stream_verified(&src, &root, &escape).is_err());
    assert!(!sibling.join("escaped.pdf").exists());
}

#[test]
fn no_mechanism_follows_a_symlinked_source() {
    // The refusal lives in each primitive, not only in select_mechanism:
    // nothing in the types enforces a call order, and the clone-to-stream
    // fallback is a second door straight into the primitives.
    //
    // All three now open the source with O_NOFOLLOW as well, so the check and
    // the open cannot disagree about which file they mean. That window is not
    // reachable from a test — it needs the link to be swapped in between the
    // two calls — so this covers the refusal and not the race.
    let d = tempdir().unwrap();
    let real = d.path().join("real.pdf");
    std::fs::write(&real, "SOURCE").unwrap();
    let link = d.path().join("link.pdf");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let root = d.path().join("dest");
    std::fs::create_dir(&root).unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    assert!(rename_excl(&link, &root, &rel("out.pdf")).is_err());
    assert!(clone_excl(&link, &root, &rel("out.pdf")).is_err());
    assert!(stream_verified(&link, &root, &rel("out.pdf")).is_err());
    assert!(select_mechanism(&link, &root, Mode::Move).is_err());
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

#[test]
fn stream_verified_copies_and_leaves_the_source() {
    // It never unlinks: apply owns that, after verification.
    let d = tempdir().unwrap();
    let (src, root) = occupied_root(&d);
    let dst = root.join("out.pdf");
    let g = TreeGuard::new(&[d.path()]).expect_created(&[&dst]);
    stream_verified(&src, &root, &rel("out.pdf")).unwrap();
    assert_eq!(read(&dst), "SOURCE");
    assert!(src.exists());
    drop(g);
}

#[test]
fn stream_verified_refuses_an_occupied_destination_without_reading_the_source() {
    // Occupancy is tested before the copy. An unreadable source discriminates:
    // check-first yields AlreadyExists, copy-first yields PermissionDenied.
    // Copying first costs a full read and write of the payload — 31.2 GiB on
    // the reference bundle — on the ordinary merge-into-a-library path.
    let d = tempdir().unwrap();
    let (src, root) = occupied_root(&d);
    let mut perms = std::fs::metadata(&src).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o000);
    std::fs::set_permissions(&src, perms).unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    assert_eq!(
        stream_verified(&src, &root, &rel("dst.pdf")).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(read(&root.join("dst.pdf")), "ORIGINAL");
}

/// The verify that licenses every streamed unlink, observed failing.
///
/// `apply` writes `placed` and then removes the source only because
/// `stream_verified` returned `Ok`, and the cross-volume reverse unlinks the
/// library copy on the same promise — the assertion message of the test next
/// to it says "the copy was unlinked from without being verified". But that
/// test checks the destination holds the right bytes, which `io::copy`
/// produces whether or not the comparison ran, so dropping the comparison
/// entirely left the suite green: no fixture ever handed it a copy that did
/// not match.
///
/// This one does. The temp is written sequentially and never rewritten, so a
/// byte changed at offset 0 once the copy has moved past it survives into the
/// final file: same length, different contents, which is exactly what the
/// hash comparison exists to catch. The window is the whole copy, and the
/// temp's own size says when it has been entered — so this is deterministic
/// rather than timed.
#[test]
fn a_stream_whose_copy_does_not_match_the_source_is_refused() {
    use std::io::{Seek, SeekFrom, Write};

    let d = tempdir().unwrap();
    let root = d.path().join("dest");
    std::fs::create_dir(&root).unwrap();
    let src = d.path().join("big.pdf");
    // Large enough that the copy is still running when the writer lands, and
    // small enough to stay a unit test. Non-uniform, so a changed byte
    // genuinely changes the hash.
    let payload: Vec<u8> = (0..8 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&src, &payload).unwrap();

    let watched = root.clone();
    let corrupter = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let partial = std::fs::read_dir(&watched)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .find(|p| {
                    p.extension().is_some_and(|e| e == "partial")
                        && std::fs::metadata(p).is_ok_and(|m| m.len() > 1 << 20)
                });
            // Offset 0 is already written and will not be written again.
            if let Some(partial) = partial {
                let mut file = std::fs::OpenOptions::new().write(true).open(&partial).unwrap();
                file.seek(SeekFrom::Start(0)).unwrap();
                file.write_all(b"!").unwrap();
                file.sync_all().unwrap();
                return true;
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        false
    });

    let result = stream_verified(&src, &root, &rel("out.pdf"));
    assert!(
        corrupter.join().unwrap(),
        "the temp was never seen, so nothing was corrupted and this proved nothing"
    );

    let err = result.expect_err("a copy that does not match the source was accepted");
    assert_eq!(err.kind(), ErrorKind::InvalidData, "{err}");
    assert!(
        !root.join("out.pdf").exists(),
        "a mismatching copy was renamed into place, and apply would now unlink the source"
    );
    // And the temp took itself with it.
    let left: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(left.is_empty(), "{left:?}");
    // The source is untouched either way: this function never unlinks.
    assert_eq!(std::fs::read(&src).unwrap(), payload);
}

#[test]
fn a_failed_stream_leaves_no_partial_behind() {
    let d = tempdir().unwrap();
    let root = d.path().join("dest");
    std::fs::create_dir(&root).unwrap();
    let src = d.path().join("unreadable.pdf");
    std::fs::write(&src, "SOURCE").unwrap();
    let mut perms = std::fs::metadata(&src).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o000);
    std::fs::set_permissions(&src, perms).unwrap();
    // No permits at all: the guard fails if a .partial survives.
    let _g = TreeGuard::new(&[d.path()]);
    assert!(stream_verified(&src, &root, &rel("out.pdf")).is_err());
}

// ---------------------------------------------------------------------------
// Dataless, mechanism selection
// ---------------------------------------------------------------------------

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
fn a_same_volume_move_is_a_rename_even_for_a_series_folder_that_does_not_exist() {
    // It stats dest_root, not the destination's parent. Stat'ing the parent
    // returned NotFound for every row on a fresh library, forcing apply to
    // mkdir before it could choose a mechanism or refuse anything.
    let d = tempdir().unwrap();
    let (src, root) = occupied_root(&d);
    let _g = TreeGuard::new(&[d.path()]);
    assert_eq!(
        select_mechanism(&src, &root, Mode::Move).unwrap(),
        Mechanism::Rename
    );
    assert_eq!(
        select_mechanism(&src, &root, Mode::Copy).unwrap(),
        Mechanism::Clone
    );
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
fn two_series_differing_only_in_case_are_one_folder() {
    // Invariant 7: Hellboy/ and hellboy/ are one directory on APFS and two
    // series on a Linux server. Grouping on the whole path misses it, because
    // the leaves differ.
    let d = tempdir().unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    let rows = [
        row("a", "Hellboy", Some(1), 9, 1),
        row("b", "hellboy", Some(2), 9, 2),
    ];
    assert_eq!(find_collisions(&rows, d.path(), &Komga).len(), 1);
}

#[test]
fn titles_that_sanitise_onto_one_folder_are_reported() {
    // `Hellboy` and `Hellboy.` used to share a folder and keep distinct
    // leaves, so two series merged on disk and nothing reported it.
    let d = tempdir().unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    for other in ["Hellboy.", "Hellboy ", "\u{200b}Hellboy"] {
        let rows = [
            row("a", "Hellboy", Some(1), 9, 1),
            row("b", other, Some(1), 9, 2),
        ];
        assert_eq!(
            find_collisions(&rows, d.path(), &Komga).len(),
            1,
            "{other:?} did not collide with Hellboy"
        );
    }
}

#[test]
fn a_plain_file_where_a_series_folder_belongs_is_a_collision() {
    // read_dir fails with NotADirectory. Reading that as "no conflict" makes
    // the plan promise a row apply cannot execute.
    let d = tempdir().unwrap();
    std::fs::write(d.path().join("Hellboy"), b"not a directory").unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    let rows = [row("a", "Hellboy", None, 1, 1)];
    assert_eq!(find_collisions(&rows, d.path(), &Komga).len(), 1);
}

#[test]
fn both_spellings_already_present_is_a_collision() {
    // Descending into whichever read_dir yielded first could miss the book
    // under the other spelling, nondeterministically.
    //
    // Needs a case-sensitive destination — a Linux server or a case-sensitive
    // APFS volume. The default macOS volume folds case, so the two
    // directories cannot both exist and there is nothing to test; skip rather
    // than assert something the platform decided.
    let d = tempdir().unwrap();
    std::fs::create_dir(d.path().join("Hellboy")).unwrap();
    if std::fs::create_dir(d.path().join("hellboy")).is_err() {
        eprintln!("skipped: destination volume folds case");
        return;
    }
    std::fs::write(d.path().join("hellboy/Hellboy.pdf"), b"x").unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    let rows = [row("a", "Hellboy", None, 1, 1)];
    assert!(!find_collisions(&rows, d.path(), &Komga).is_empty());
}

// ---------------------------------------------------------------------------
// Invariant 3: containment is structural. These drive the primitives
// directly, because a test that writes `..` into a manifest and asserts a
// refusal pins invariant 9 — the preset-recompute check fires first — and
// says nothing about containment at all.
// ---------------------------------------------------------------------------

#[test]
fn resolve_at_refuses_a_parent_dir_component() {
    let d = tempdir().unwrap();
    let root = d.path().join("dest");
    std::fs::create_dir(&root).unwrap();
    let sibling = d.path().join("sibling");
    std::fs::create_dir(&sibling).unwrap();
    std::fs::write(sibling.join("victim.pdf"), b"PRIVATE").unwrap();

    let _g = TreeGuard::new(&[d.path()]);
    assert!(comic_renamer_core::fsops::resolve_at(&root, Path::new("../sibling/victim.pdf")).is_err());
    assert!(comic_renamer_core::fsops::stat_at(&root, Path::new("../sibling/victim.pdf")).is_err());
}

#[test]
fn resolve_at_refuses_a_symlinked_component_beneath_the_root() {
    // The series folder is a symlink to somewhere else on the same volume.
    // An absolute join plus symlink_metadata would follow it; the walk does
    // not.
    let d = tempdir().unwrap();
    let root = d.path().join("dest");
    let elsewhere = d.path().join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("book.pdf"), b"SOMEONE ELSE'S").unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("Hellboy")).unwrap();

    let _g = TreeGuard::new(&[d.path()]);
    let through = Path::new("Hellboy/book.pdf");
    assert!(
        comic_renamer_core::fsops::resolve_at(&root, through).is_err(),
        "the walk followed a symlinked series folder"
    );
    // And the entry that would have deleted it cannot be built at all.
    assert!(comic_renamer_core::fsops::stat_at(&root, through).is_err());
    assert!(elsewhere.join("book.pdf").exists());
}

#[test]
fn an_entry_removes_only_through_its_own_resolved_parent() {
    // The positive half: a legitimate path resolves, stats and removes.
    let d = tempdir().unwrap();
    let root = d.path().join("dest");
    std::fs::create_dir_all(root.join("Hellboy")).unwrap();
    let book = root.join("Hellboy/book.pdf");
    std::fs::write(&book, b"x").unwrap();

    let g = TreeGuard::new(&[d.path()]).expect_removed(&[&book]);
    let entry = comic_renamer_core::fsops::resolve_at(&root, Path::new("Hellboy/book.pdf")).unwrap();
    assert_eq!(entry.identity().unwrap().0, 1);
    entry.remove().unwrap();
    drop(g);
}

// --- D60: issues through collision detection -----------------------------

/// A row with an issue, built on `row`'s deliberately-lying preview so this
/// still proves `find_collisions` recomputes rather than reading it.
fn issue_row(id: &str, series: &str, issue: Option<u32>, ino: u64) -> PlanRow {
    PlanRow { issue, ..row(id, series, None, 0, ino) }
}

/// The plan's Review Focus put this condition in collision detection, and
/// the preset tests pinned only the preset. Two issues of one series are two
/// destinations that share a folder: `find_collisions` must group them
/// without refusing either, or a whole run of issues becomes unplaceable.
#[test]
fn two_issues_of_one_series_are_not_a_collision() {
    let d = tempfile::tempdir().unwrap();
    let rows = [
        issue_row("r1", "Curse", Some(1), 11),
        issue_row("r2", "Curse", Some(2), 12),
    ];
    assert!(find_collisions(&rows, d.path(), &Komga).is_empty());
}

/// Review Focus 4, through the same door. `Elephantmen 2260` the series and
/// `Elephantmen` issue 2260 render the SAME leaf name and are kept apart
/// only by the series folder — so this is the test that fails if anything
/// ever flattens that folder.
#[test]
fn a_series_named_with_a_number_does_not_collide_with_an_issue_of_that_number() {
    let d = tempfile::tempdir().unwrap();
    let named = issue_row("r1", "Elephantmen 2260", None, 11);
    let issued = issue_row("r2", "Elephantmen", Some(2260), 12);
    let dest = |r: &PlanRow| {
        comic_renamer_core::preset::Preset::destination(&Komga, &r.resolved(), &r.extension)
    };
    assert_eq!(
        dest(&named).file_name(),
        dest(&issued).file_name(),
        "the leaves really do collide, which is what makes this worth testing"
    );
    assert!(find_collisions(&[named, issued], d.path(), &Komga).is_empty());
}

/// And the gate still bites where it should: one series, one issue, twice.
#[test]
fn the_same_issue_of_one_series_twice_is_a_collision() {
    let d = tempfile::tempdir().unwrap();
    let rows = [
        issue_row("r1", "Curse", Some(3), 11),
        issue_row("r2", "Curse", Some(3), 12),
    ];
    assert_eq!(find_collisions(&rows, d.path(), &Komga).len(), 1);
}
