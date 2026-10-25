//! What gives `undo` the authority to delete a file from the library.
//!
//! The identity check in `reverse_one` compares the destination against the
//! size, inode and mtime the *record* carries, so whoever wrote the record
//! chose the numbers it is compared against. A reviewer executed that: a
//! hand-written record deleted a file no run had placed and the report said
//! `restored: 1, refused: []`. These tests pin the answer, which is that the
//! filesystem decides — a library file is removed only when a regular file
//! still sits at the recorded source and holds the same bytes.

mod common;

use comic_renamer_core::preset::preset_by_id;
use comic_renamer_core::types::{Aside, Mechanism, Mode, Resolved, Status};
use comic_renamer_core::undo::{reverse, UndoWriter};
use common::{reverse_before, DEADLINE};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

/// The one destination every fixture here uses.
fn dest_for() -> PathBuf {
    let resolved = Resolved {
        series: "Hellboy".into(),
        volume: Some(1),
        subtitle: None,
        run: 3,
        status: Status::Mapped,
        record: None,
    };
    preset_by_id("komga").unwrap().destination(&resolved, "pdf")
}

type Identity = (u64, u64, i64);

fn identity_of(path: &Path) -> Identity {
    let m = std::fs::symlink_metadata(path).unwrap();
    (m.len(), m.ino(), m.mtime() * 1_000_000_000 + m.mtime_nsec())
}

struct Fixture {
    _dir: TempDir,
    record: PathBuf,
    source_file: PathBuf,
    /// The library root. D58's aside directory is under it, so a test that
    /// looks for a file moved aside needs it.
    library: PathBuf,
    library_file: PathBuf,
    recorded_source_identity: Identity,
}

/// Source root, library root, and the destination's parent directories.
fn roots() -> (TempDir, PathBuf, PathBuf) {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let library = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(library.join(dest_for()).parent().unwrap()).unwrap();
    let source = source.canonicalize().unwrap();
    let library = library.canonicalize().unwrap();
    (dir, source, library)
}

/// The research note's fixture, built with the real writer because that is
/// what makes it a *plausible* record rather than an obviously broken one.
///
/// Every number in it is honest about a file the planter controls: the source
/// is theirs, five bytes of it, and `record_placed` takes the library file's
/// real identity, which anyone can stat. Nothing here was ever copied or
/// moved — the library file is a file no run placed.
///
/// `Mode::Copy` terminates at `done`; `Mode::Move` terminates at `placed`,
/// which is the branch that removes the duplicate a failed unlink leaves.
fn planted(mode: Mode) -> Fixture {
    let (dir, source, library) = roots();
    let source_file = source.join("Hellboy_v01.pdf");
    std::fs::write(&source_file, b"theft").unwrap();
    let library_file = library.join(dest_for());
    std::fs::write(&library_file, b"a library file no run ever placed").unwrap();

    let record = dir.path().join("run.record");
    let mut w = UndoWriter::create(&record, &source, &library, "komga", 1).unwrap();
    let recorded_source_identity = identity_of(&source_file);
    let seq = w
        .record_pending(
            Path::new("Hellboy_v01.pdf"),
            &dest_for(),
            Mechanism::Clone,
            mode,
            &source_file,
            "Hellboy",
            Some(1),
            None,
            3,
            "pdf",
        )
        .unwrap();
    w.record_placed(seq, identity_of(&library_file)).unwrap();
    if mode == Mode::Copy {
        w.record_done(seq).unwrap();
    }
    Fixture { _dir: dir, record, source_file, library, library_file, recorded_source_identity }
}

/// A copy that really happened: the library holds the source's bytes, so the
/// source is a copy of what is about to be removed.
fn real_copy() -> Fixture {
    real_copy_of(b"the bytes of a comic")
}

/// As `real_copy`, over a payload the caller chooses. The race test needs one
/// large enough that hashing it takes measurable time; every other test here
/// is indifferent to the content and uses the default.
fn real_copy_of(payload: &[u8]) -> Fixture {
    let (dir, source, library) = roots();
    let source_file = source.join("Hellboy_v01.pdf");
    std::fs::write(&source_file, payload).unwrap();
    let library_file = library.join(dest_for());

    let record = dir.path().join("run.record");
    let mut w = UndoWriter::create(&record, &source, &library, "komga", 1).unwrap();
    let recorded_source_identity = identity_of(&source_file);
    let seq = w
        .record_pending(
            Path::new("Hellboy_v01.pdf"),
            &dest_for(),
            Mechanism::Clone,
            Mode::Copy,
            &source_file,
            "Hellboy",
            Some(1),
            None,
            3,
            "pdf",
        )
        .unwrap();
    std::fs::copy(&source_file, &library_file).unwrap();
    w.record_placed(seq, identity_of(&library_file)).unwrap();
    w.record_done(seq).unwrap();
    Fixture { _dir: dir, record, source_file, library, library_file, recorded_source_identity }
}

/// A same-volume move: the rename took the inode with it, so the source is
/// gone and reversing means renaming back, not deleting anything.
fn real_move() -> Fixture {
    let (dir, source, library) = roots();
    let source_file = source.join("Hellboy_v01.pdf");
    std::fs::write(&source_file, b"the bytes of a comic").unwrap();
    let library_file = library.join(dest_for());

    let record = dir.path().join("run.record");
    let mut w = UndoWriter::create(&record, &source, &library, "komga", 1).unwrap();
    let recorded_source_identity = identity_of(&source_file);
    let seq = w
        .record_pending(
            Path::new("Hellboy_v01.pdf"),
            &dest_for(),
            Mechanism::Rename,
            Mode::Move,
            &source_file,
            "Hellboy",
            Some(1),
            None,
            3,
            "pdf",
        )
        .unwrap();
    std::fs::rename(&source_file, &library_file).unwrap();
    w.record_done(seq).unwrap();
    Fixture { _dir: dir, record, source_file, library, library_file, recorded_source_identity }
}

/// Writes over a file without truncating it, so the inode and — given the
/// same length — the size both survive. Only the mtime moves.
fn write_in_place(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

/// Puts the mtime (and atime) back to what another file's metadata recorded.
fn set_mtime(path: &Path, like: &std::fs::Metadata) {
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let times = [
        libc::timespec { tv_sec: like.atime(), tv_nsec: like.atime_nsec() as libc::c_long },
        libc::timespec { tv_sec: like.mtime(), tv_nsec: like.mtime_nsec() as libc::c_long },
    ];
    // SAFETY: a valid NUL-terminated path and a two-element times array for
    // the duration of the call.
    let rc = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            name.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    assert_eq!(rc, 0, "utimensat: {}", std::io::Error::last_os_error());
}

fn mkfifo(path: &Path) {
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path for the duration of the call.
    let rc = unsafe { libc::mkfifo(name.as_ptr(), 0o644) };
    assert_eq!(rc, 0, "mkfifo: {}", std::io::Error::last_os_error());
}

/// The executed reproduction from the research note, as a regression test.
///
/// Both arms, because they are different code. `Copy` reaches the unlink past
/// the source's identity; `Move` reaches it past the identity *and* a status
/// saying a duplicate was left behind. Guarding one and not the other is how
/// the finding comes back through a different door.
#[test]
fn a_planted_record_does_not_delete_a_library_file() {
    for mode in [Mode::Copy, Mode::Move] {
        let f = planted(mode);
        let report = reverse(&f.record).unwrap();
        assert_eq!(report.restored, 0, "{mode:?}");
        assert!(!report.refused.is_empty(), "{mode:?}: {:?}", report.refused);
        assert!(
            f.library_file.exists(),
            "{mode:?}: the file no run placed is still there"
        );
        assert!(f.source_file.exists(), "{mode:?}: and nothing else moved either");
    }
}

#[test]
fn a_real_copy_is_reversed_because_the_source_still_matches() {
    let f = real_copy();
    let report = reverse(&f.record).unwrap();
    assert_eq!(report.refused, Vec::<String>::new());
    assert_eq!(report.restored, 1);
    assert!(!f.library_file.exists() && f.source_file.exists());
    // Since D58 "the destination is clear" is half the truth, and on its own
    // it is also true of a reverse that destroyed the file. What happened to
    // it is pinned in full below; this line is here so that reading only this
    // test does not leave the stronger claim looking optional.
    assert!(aside_file(&f).is_file(), "the library's copy was not set aside");
}

/// Isolates the content gate, which is the only test here that does.
///
/// Overwriting the source normally changes its mtime, so the *existing*
/// identity check refuses and a test built that way passes with no new code at
/// all. Same length, same inode — written in place — and the recorded mtime
/// restored with `utimensat` leaves the identity matching exactly, which the
/// first assertion insists on. Only a comparison of the contents can refuse
/// from there.
#[test]
fn a_same_size_stranger_with_the_recorded_identity_refuses_the_delete() {
    let f = real_copy();
    let before = std::fs::symlink_metadata(&f.source_file).unwrap();
    let len = before.len() as usize;
    write_in_place(&f.source_file, &vec![b'z'; len]);
    set_mtime(&f.source_file, &before);
    assert_eq!(
        identity_of(&f.source_file),
        f.recorded_source_identity,
        "the identity check cannot see this, which is the point"
    );

    let report = reverse(&f.record).unwrap();
    assert_eq!(report.restored, 0);
    assert!(!report.refused.is_empty(), "{:?}", report.refused);
    assert!(f.library_file.exists());
}

/// A FIFO at the recorded source is refused, not opened.
///
/// `open` on a FIFO blocks until a writer appears, and `undo` holds the
/// library's lock while it waits — so the one command that could recover the
/// library would be the thing a planted record took away. The record here
/// records the FIFO's own identity, which is what carries it past the identity
/// check and into the gate; swapping a FIFO in *after* a real run instead is
/// refused one layer earlier and never reaches it.
///
/// Bounded by `reverse_before`: without `O_NONBLOCK` this never returns.
#[test]
fn a_fifo_at_the_source_is_refused_rather_than_blocking() {
    let (dir, source, library) = roots();
    let source_file = source.join("Hellboy_v01.pdf");
    mkfifo(&source_file);
    let library_file = library.join(dest_for());
    std::fs::write(&library_file, b"a library file no run ever placed").unwrap();

    let record = dir.path().join("run.record");
    let mut w = UndoWriter::create(&record, &source, &library, "komga", 1).unwrap();
    let seq = w
        .record_pending(
            Path::new("Hellboy_v01.pdf"),
            &dest_for(),
            Mechanism::Clone,
            Mode::Copy,
            &source_file,
            "Hellboy",
            Some(1),
            None,
            3,
            "pdf",
        )
        .unwrap();
    w.record_placed(seq, identity_of(&library_file)).unwrap();
    w.record_done(seq).unwrap();

    let _kept = dir.keep();
    let report = reverse_before(&record, DEADLINE);
    assert_eq!(report.restored, 0);
    assert!(!report.refused.is_empty(), "{:?}", report.refused);
    assert!(library_file.exists());
}

/// A character device at the recorded source is refused, not read.
///
/// A record carries its own source root, so it can name `/dev` as the bundle
/// folder and `zero` as the file — no privilege needed to plant it, because
/// nothing is planted. `O_NONBLOCK` does not save this one: the open succeeds,
/// the seek succeeds, and the reads never stop, so a hash of it runs until the
/// machine is interrupted with the library's lock still held. Refusing
/// anything that is not a regular file is what stops it, and this is the only
/// test that says so — a FIFO is refused anyway by the seek that fails.
///
/// Bounded by `reverse_before`: without the refusal this reads `/dev/zero` at
/// a full core until something kills the process.
#[test]
fn a_character_device_at_the_source_is_refused_rather_than_read_forever() {
    let (dir, _source, library) = roots();
    let library_file = library.join(dest_for());
    std::fs::write(&library_file, b"a library file no run ever placed").unwrap();

    let device = Path::new("/dev/zero");
    let recorded = identity_of(device);
    let record = dir.path().join("run.record");
    let mut w = UndoWriter::create(&record, Path::new("/dev"), &library, "komga", 1).unwrap();
    let seq = w
        .record_pending(
            Path::new("zero"),
            &dest_for(),
            Mechanism::Clone,
            Mode::Copy,
            device,
            "Hellboy",
            Some(1),
            None,
            3,
            "pdf",
        )
        .unwrap();
    w.record_placed(seq, identity_of(&library_file)).unwrap();
    w.record_done(seq).unwrap();

    // The identity check passes it through: the record is honest about a file
    // anyone can stat, and the device's identity has not moved since.
    assert_eq!(
        identity_of(device),
        recorded,
        "the recorded source identity still holds, so the gate is what has to refuse"
    );
    let _kept = dir.keep();
    let report = reverse_before(&record, DEADLINE);
    assert_eq!(report.restored, 0);
    assert!(!report.refused.is_empty(), "{:?}", report.refused);
    assert!(library_file.exists());
}

/// A FIFO at the *destination* is refused for the same reason, and it can get
/// there the same way: a record that carries the FIFO's own identity.
///
/// Bounded by `reverse_before`, for the reason the source case is.
#[test]
fn a_fifo_at_the_destination_is_refused_rather_than_blocking() {
    let (dir, source, library) = roots();
    let source_file = source.join("Hellboy_v01.pdf");
    std::fs::write(&source_file, b"the bytes of a comic").unwrap();
    let library_file = library.join(dest_for());
    mkfifo(&library_file);

    let record = dir.path().join("run.record");
    let mut w = UndoWriter::create(&record, &source, &library, "komga", 1).unwrap();
    let seq = w
        .record_pending(
            Path::new("Hellboy_v01.pdf"),
            &dest_for(),
            Mechanism::Clone,
            Mode::Copy,
            &source_file,
            "Hellboy",
            Some(1),
            None,
            3,
            "pdf",
        )
        .unwrap();
    w.record_placed(seq, identity_of(&library_file)).unwrap();
    w.record_done(seq).unwrap();

    let _kept = dir.keep();
    let report = reverse_before(&record, DEADLINE);
    assert_eq!(report.restored, 0);
    assert!(!report.refused.is_empty(), "{:?}", report.refused);
    assert!(std::fs::symlink_metadata(&library_file).is_ok());
}

/// Reversing a move by renaming back is untouched: `RENAME_EXCL` refuses to
/// clobber, so nothing is deleted and nothing needs hashing.
///
/// And it leaves no aside directory. D58's directory is created by the row
/// that needs it, not up front, because move is the default mode — a library
/// reversed back to empty would otherwise be left holding a dot-directory
/// that nothing in the tool ever removes.
#[test]
fn a_move_is_reversed_by_renaming_back_without_a_content_check() {
    let f = real_move();
    let report = reverse(&f.record).unwrap();
    assert_eq!(report.restored, 1);
    assert!(f.source_file.exists());
    assert!(!f.library_file.exists());
    assert_eq!(identity_of(&f.source_file), f.recorded_source_identity);
    assert_eq!(report.aside, None);
    assert!(
        !f.library.join(ASIDE).exists(),
        "a move-mode reverse created an aside directory it had no use for"
    );
}

// ---------------------------------------------------------------------------
// D58: the act the gate authorises is a rename, not an unlink.
// ---------------------------------------------------------------------------

/// Hardcoded, not read from the crate. A test that asks the code what the
/// directory is called cannot notice the dot going missing, and the dot is
/// what keeps a library scanner out of it.
const ASIDE: &str = ".comic-renamer-aside";

/// Every fixture here writes its record as `run.record`, and a run id is the
/// record's own file name.
fn aside_dir(f: &Fixture) -> PathBuf {
    f.library.join(ASIDE).join("run.record")
}

fn aside_file(f: &Fixture) -> PathBuf {
    aside_dir(f).join(f.library_file.file_name().unwrap())
}

/// The decision itself: a reverse the gate authorises renames the library file
/// into the aside directory and never unlinks it.
///
/// Also pins the **leaf only** rule. Were the target the record's
/// `dest_relative`, the file would land under `…/run.record/Hellboy/` and this
/// path would hold nothing — and since nothing creates that subdirectory, the
/// rename would fail and the row would be refused instead.
#[test]
fn a_reversed_copy_moves_the_library_file_aside_rather_than_unlinking_it() {
    let f = real_copy();
    let payload = std::fs::read(&f.library_file).unwrap();

    let report = reverse(&f.record).unwrap();
    assert_eq!(report.refused, Vec::<String>::new());
    assert_eq!(report.restored, 1);
    assert!(!f.library_file.exists(), "the destination was cleared");
    assert!(f.source_file.exists(), "the original is still at the source");

    let kept = aside_file(&f);
    assert!(
        kept.is_file(),
        "the library file was unlinked, not moved aside to {}",
        kept.display()
    );
    assert_eq!(
        std::fs::read(&kept).unwrap(),
        payload,
        "the bytes that were in the library are still on disk"
    );
    assert_eq!(
        report.aside,
        Some(Aside { directory: aside_dir(&f), files: 1 }),
        "the report says where and how many"
    );
}

/// `RENAME_EXCL` refuses rather than overwriting, and the row says so.
///
/// Reachable without an adversary: a first pass that refused some rows, run
/// again, adds to a directory the earlier pass started — and two rows sharing
/// a leaf inside one run arrive here too. Either way the file already set
/// aside is not the one to lose.
#[test]
fn an_occupied_aside_name_refuses_and_overwrites_nothing() {
    let f = real_copy();
    let occupied = aside_file(&f);
    std::fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    std::fs::write(&occupied, b"set aside by an earlier pass").unwrap();

    let report = reverse(&f.record).unwrap();
    assert_eq!(report.restored, 0);
    assert!(
        report.refused.iter().any(|why| why.contains("already set aside")),
        "the refusal names the collision: {:?}",
        report.refused
    );
    assert!(
        f.library_file.exists(),
        "the library file stays put when it cannot be moved aside"
    );
    assert_eq!(
        std::fs::read(&occupied).unwrap(),
        b"set aside by an earlier pass",
        "the file already set aside was not clobbered"
    );
    assert_eq!(report.aside, None, "nothing was moved aside, so nothing is named");
}

/// Big enough that hashing it is a window another thread can act inside,
/// small enough to write four times in a test.
const PAYLOAD: usize = 8 << 20;

fn atime_ns(path: &Path) -> i64 {
    let m = std::fs::metadata(path).unwrap();
    m.atime() * 1_000_000_000 + m.atime_nsec()
}

/// The race D58 exists for, reproduced, with the assertion that matters.
///
/// The gate hashed the source by path, hashed the destination, then unlinked.
/// A writer truncating the source in between got the library copy unlinked
/// anyway — `1 restored · 0 refused`, exit 0, and the payload in neither
/// place. The adversarial run measured that with a 400 MB file and a
/// wall-clock sleep; this reaches the same window without sleeping on a guess.
///
/// The trigger is the **destination's** atime. Nothing before the gate reads
/// the destination — `identity()` is an `fstat`, and the source's check is a
/// `stat` — so the first atime the destination gets is the gate's own hash of
/// it, which runs after the source's. A writer that waits for that and then
/// truncates is inside the window by construction.
///
/// Two assertions, and the second is what keeps this honest:
///
/// - **Every attempt:** the bytes that were in the library are still readable
///   somewhere. That holds wherever the truncation landed, so it is not a
///   timing claim and cannot pass by the writer being late.
/// - **Across the attempts:** at least one reached the authorised act with the
///   source already truncated. `restored == 1` says the gate agreed and acted;
///   a truncation landing before the source's hash finished would have made
///   the two hashes differ, and one landing before the identity check would
///   have failed that — so either would have refused the row. The pair is an
///   exact witness that the window was entered, and without it this test would
///   pass on a machine whose writer never fired at all.
#[test]
fn nothing_is_lost_when_the_source_is_truncated_inside_the_gate() {
    let payload = vec![b'c'; PAYLOAD];
    let mut raced = 0;
    let mut outcomes = Vec::new();

    for attempt in 0..4 {
        let f = real_copy_of(&payload);
        // Sampled here, on this thread, before anything can read the
        // destination. Taken inside the writer it would race the very hash it
        // is waiting for.
        let quiet = atime_ns(&f.library_file);

        let watch = f.library_file.clone();
        let source = f.source_file.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            while atime_ns(&watch) == quiet && start.elapsed() < DEADLINE {
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
            // One byte, so the source no longer holds what the gate just
            // agreed it held.
            std::fs::write(&source, b"x").unwrap();
            tx.send(std::time::Instant::now()).unwrap();
        });

        let report = reverse(&f.record).unwrap();
        let returned = std::time::Instant::now();
        let truncated = rx.recv_timeout(DEADLINE).expect("the writer never fired");
        writer.join().unwrap();

        // Wherever it landed, the payload is still readable from one of the
        // two places a reverse can leave it.
        let survived = [aside_file(&f), f.library_file.clone()]
            .iter()
            .any(|p| std::fs::read(p).is_ok_and(|bytes| bytes == payload));
        assert!(
            survived,
            "attempt {attempt}: the payload is in neither place. \
             report: {report:?}; the source is now {} bytes",
            std::fs::metadata(&f.source_file).map(|m| m.len()).unwrap_or(0)
        );

        let inside = report.restored == 1 && truncated < returned;
        outcomes.push(format!(
            "attempt {attempt}: restored={} refused={} inside_window={inside}",
            report.restored,
            report.refused.len()
        ));
        raced += usize::from(inside);
    }

    assert!(
        raced > 0,
        "the window was never entered, so this proved nothing:\n  {}",
        outcomes.join("\n  ")
    );
}
