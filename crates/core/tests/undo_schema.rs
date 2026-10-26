use comic_renamer_core::preset::preset_by_id;
use comic_renamer_core::types::{Mechanism, Mode, Resolved, Status, UndoError};
use comic_renamer_core::undo::{reverse, summarise, UndoWriter, COLUMNS};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

fn dest_for() -> PathBuf {
    let resolved = Resolved {
        series: "Hellboy".into(),
        volume: Some(1),
        issue: None,
        subtitle: None,
        run: 3,
        status: Status::Mapped,
        record: None,
    };
    preset_by_id("komga").unwrap().destination(&resolved, "pdf")
}

/// One file run through the writer. `same_root` points both roots at one
/// directory; `mv` records a same-volume rename instead of a verified copy.
fn written(mv: bool, same_root: bool) -> (TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let library = if same_root { source.clone() } else { dir.path().join("library") };
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&library).unwrap();
    let (source, library) = (source.canonicalize().unwrap(), library.canonicalize().unwrap());
    let file = source.join("Hellboy_v01.pdf");
    std::fs::write(&file, b"bytes").unwrap();

    let rel = dest_for();
    let record = dir.path().join("run.record");
    let mut w = UndoWriter::create(&record, &source, &library, "komga", 1).unwrap();
    let (mechanism, mode) = if mv { (Mechanism::Rename, Mode::Move) } else { (Mechanism::Clone, Mode::Copy) };
    let seq = w
        .record_pending(
            Path::new("Hellboy_v01.pdf"), &rel, mechanism, mode, &file,
            "Hellboy", Some(1), None, None, 3, "pdf",
        )
        .unwrap();
    let dest = library.join(&rel);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    if mv {
        std::fs::rename(&file, &dest).unwrap();
    } else {
        std::fs::write(&dest, b"bytes").unwrap();
        let m = std::fs::symlink_metadata(&dest).unwrap();
        w.record_placed(seq, (m.len(), m.ino(), m.mtime() * 1_000_000_000 + m.mtime_nsec())).unwrap();
    }
    w.record_done(seq).unwrap();
    (dir, record)
}

fn a_written_record() -> (TempDir, PathBuf) {
    written(false, false)
}

#[test]
fn a_record_alone_rebuilds_every_destination() {
    let (dir, record) = a_written_record();
    let s = summarise(&record).unwrap();
    assert_eq!((s.preset.as_str(), s.planned, s.reversible, s.interrupted), ("komga", 1, 1, false));
    let report = reverse(&record).unwrap();
    assert_eq!(report.refused, Vec::<String>::new());
    assert_eq!(report.restored, 1);
    assert!(!dir.path().join("library").join(dest_for()).exists());
    // A copy, so D58 applies: the destination is clear and the bytes it held
    // are in the aside directory, named for this record. Checked here too,
    // because "the destination no longer exists" on its own is equally true
    // of a reverse that destroyed the file.
    let aside = report.aside.expect("a reversed copy names its aside directory");
    assert_eq!(aside.files, 1);
    assert!(aside.directory.ends_with(".comic-renamer-aside/run.record"));
    assert_eq!(
        std::fs::read(aside.directory.join(dest_for().file_name().unwrap())).unwrap(),
        b"bytes"
    );
}

/// Review Focus 3, both halves.
#[test]
fn a_wrong_header_is_refused_and_a_missing_one_is_not() {
    let (dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();

    let old = dir.path().join("old.record");
    std::fs::write(&old, "seq,status,mode,mechanism,source_name,dest_relative,size,inode,mtime_ns\n").unwrap();
    assert!(summarise(&old).unwrap_err().to_string().contains("older build"));

    let headerless = dir.path().join("bare.record");
    std::fs::write(&headerless, text.lines().skip(1).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    assert_eq!(summarise(&headerless).unwrap().planned, 1, "the roots row is not eaten as a header");
}

/// Review Focus 1.
#[test]
fn a_record_whose_roots_are_gone_refuses_every_row() {
    let (dir, record) = a_written_record();
    std::fs::remove_dir_all(dir.path().join("library")).unwrap();
    let report = reverse(&record).unwrap();
    assert_eq!(report.restored, 0);
    assert!(report.refused.iter().any(|r| r.contains("library")), "{:?}", report.refused);
}

/// The naming trap: a same-volume move records `done` and never `placed`.
#[test]
fn a_move_run_is_reversible_even_though_no_row_is_placed() {
    let (_dir, record) = written(true, false);
    let s = summarise(&record).unwrap();
    assert_eq!(s.reversible, 1, "a renamed row is reversible");
    assert!(!s.interrupted);
    let text = std::fs::read_to_string(&record).unwrap();
    assert!(!text.contains(",placed,"), "and it really has no placed row");
}

/// The same trap one step earlier, and the one that mattered. A crash inside
/// move mode's single-syscall window leaves a bare `pending` `rename` row —
/// and `expected()` identifies exactly that row, because a rename moves the
/// inode, so the source identity the record already carries still describes
/// the destination. `reversible` counted `placed` and `done` instead, so the
/// record read "0 of 1 reversible": the bare `undo` said nothing was restored,
/// the menu drew no offer, and the next `apply` of the same pair swept the
/// record away — while `undo <run-id>` restored the file, which is what proves
/// the count wrong rather than the reverse.
#[test]
fn a_pending_rename_row_is_reversible_because_the_reverse_can_identify_it() {
    let (dir, record) = written(true, false);
    // `written` has already renamed the file into the library, so dropping the
    // `done` line is precisely the state that window leaves behind.
    let text = std::fs::read_to_string(&record).unwrap();
    let cut: Vec<_> = text.lines().filter(|l| !l.contains(",done,")).collect();
    std::fs::write(&record, cut.join("\n") + "\n").unwrap();
    assert!(
        std::fs::read_to_string(&record).unwrap().contains(",pending,"),
        "the fixture has to be a pending row for this to mean anything"
    );

    let s = summarise(&record).unwrap();
    assert_eq!(s.reversible, 1, "a pending rename is a row `expected()` identifies");
    assert!(s.interrupted, "and the run did not finish");

    // Not a count taken on trust: the reverse the count gates really restores it.
    let report = reverse(&record).unwrap();
    assert_eq!((report.restored, report.refused.len()), (1, 0), "{report:?}");
    assert!(dir.path().join("source").join("Hellboy_v01.pdf").exists());
}

/// The other direction, and why the count cannot simply be widened to "not
/// failed": a finished row whose mechanism made a new inode is only
/// identifiable through the identity its `placed` line carries, so a record
/// that lost that line has nothing to match the destination against.
#[test]
fn a_done_row_whose_placed_identity_is_missing_is_not_counted() {
    let (_dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    let cut: Vec<_> = text.lines().filter(|l| !l.contains(",placed,")).collect();
    std::fs::write(&record, cut.join("\n") + "\n").unwrap();

    let s = summarise(&record).unwrap();
    assert_eq!(s.reversible, 0, "a clone row with no recorded inode cannot be matched");
    // And the reverse agrees, which is the whole point of deriving one from
    // the other: the count never promises a row the reverse then refuses.
    let report = reverse(&record).unwrap();
    assert_eq!(report.restored, 0);
    assert_eq!(report.refused.len(), 1, "{report:?}");
}

#[test]
fn every_status_row_has_the_full_column_count() {
    let (_dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    let want = COLUMNS.split(',').count();
    assert!(text.contains(",placed,") && text.contains(",done,"));
    for line in text.lines() {
        assert_eq!(line.split(',').count(), want, "{line}");
    }
}

#[test]
fn a_pending_row_with_no_terminal_status_is_interrupted() {
    let (_dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    let cut: Vec<_> = text.lines().filter(|l| !l.contains(",done,") && !l.contains(",placed,")).collect();
    std::fs::write(&record, cut.join("\n") + "\n").unwrap();
    let s = summarise(&record).unwrap();
    assert_eq!((s.reversible, s.interrupted), (0, true));
}

#[test]
fn an_edited_destination_is_not_a_destination_the_preset_produces() {
    let (_dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    std::fs::write(&record, text.replace("Hellboy/", "Elsewhere/")).unwrap();
    let report = reverse(&record).unwrap();
    assert_eq!(report.restored, 0);
    assert!(!report.refused.is_empty());
}

/// Equal roots are only one half of the check: `||` becomes `&&` and equal
/// roots still refuse, because both halves are true. Nesting tells them apart.
#[test]
fn nested_roots_are_refused_in_both_directions() {
    let (dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    let source = dir.path().join("source").canonicalize().unwrap();
    let library = dir.path().join("library").canonicalize().unwrap();

    // Library inside the bundle folder.
    let inner = source.join("library");
    std::fs::create_dir_all(&inner).unwrap();
    let a = dir.path().join("a.record");
    std::fs::write(&a, text.replace(&format!("'{}", library.display()), &format!("'{}", inner.display()))).unwrap();
    assert!(matches!(reverse(&a), Err(UndoError::RootsOverlap)));

    // Bundle folder inside the library.
    let inner = library.join("source");
    std::fs::create_dir_all(&inner).unwrap();
    let b = dir.path().join("b.record");
    std::fs::write(&b, text.replace(&format!("'{}", source.display()), &format!("'{}", inner.display()))).unwrap();
    assert!(matches!(reverse(&b), Err(UndoError::RootsOverlap)));

    let msg = UndoError::RootsOverlap.to_string();
    assert!(msg.contains("overlap") && !msg.contains("no destination root"), "{msg}");
}

#[test]
fn equal_roots_name_the_overlap_not_a_missing_root() {
    let (_dir, record) = written(false, true);
    assert!(matches!(reverse(&record), Err(UndoError::RootsOverlap)));
}

#[test]
fn started_falls_between_two_clock_readings() {
    let before = std::time::SystemTime::now();
    let (_dir, record) = a_written_record();
    let after = std::time::SystemTime::now();
    let started = summarise(&record).unwrap().started;
    assert!(before <= started && started <= after, "{before:?} {started:?} {after:?}");
}

/// Without the extend, a row the parser dropped leaves a file in the library
/// while the report says the undo was clean.
#[test]
fn an_unreadable_row_is_reported_by_reverse() {
    let (_dir, record) = a_written_record();
    let mut text = std::fs::read_to_string(&record).unwrap();
    text.push_str("not-a-number,done,,,,,,,,,,,,,,\n");
    std::fs::write(&record, text).unwrap();
    let report = reverse(&record).unwrap();
    assert_eq!(report.restored, 1);
    assert!(report.refused.iter().any(|r| r.contains("not a number")), "{:?}", report.refused);
}

/// A `pending` row short enough to parse but too short to rebuild anything
/// from. Nothing this build writes produces one — the writer always emits all
/// seventeen columns — but a record is a file a human can edit, which is what
/// invariant 1 exists for, and a row nothing can reverse must be *reported*
/// rather than quietly dropped: a dropped row leaves a file in the library
/// while the report says the undo was clean.
#[test]
fn a_pending_row_too_short_to_rebuild_is_reported_not_dropped() {
    let (_dir, record) = a_written_record();
    let mut text = std::fs::read_to_string(&record).unwrap();
    // Nine columns: enough for the length check and the identity, and three
    // short of the extension the preset needs.
    text.push_str("9,pending,move,rename,'f.pdf,'Hellboy/f.pdf,0,1,0\n");
    std::fs::write(&record, text).unwrap();

    let report = reverse(&record).unwrap();
    assert!(
        report
            .refused
            .iter()
            .any(|r| r.contains("carries no fields")),
        "the row was dropped in silence: {:?}",
        report.refused
    );
    // And the sibling row is untouched by it.
    assert_eq!(report.restored, 1, "{report:?}");
}

#[test]
fn an_extension_is_prefixed_like_every_other_text_field() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f.pdf");
    std::fs::write(&file, b"x").unwrap();
    let record = dir.path().join("r.record");
    let mut w = UndoWriter::create(&record, dir.path(), dir.path(), "komga", 1).unwrap();
    w.record_pending(Path::new("f.pdf"), Path::new("a/b"), Mechanism::Clone, Mode::Copy, &file, "S", None, None, None, 0, "=cmd").unwrap();
    let text = std::fs::read_to_string(&record).unwrap();
    assert!(text.contains(",'=cmd,"), "{text}");
}

/// The common case: the user deleted the emptied bundle folder, and the
/// library is still there. The record names the folder, so the files go back.
#[test]
fn a_deleted_bundle_folder_is_rebuilt_and_the_file_goes_back() {
    let (dir, record) = written(true, false);
    std::fs::remove_dir(dir.path().join("source")).unwrap();
    let report = reverse(&record).unwrap();
    assert_eq!(report.refused, Vec::<String>::new());
    assert_eq!(report.restored, 1);
    assert!(dir.path().join("source").join("Hellboy_v01.pdf").exists());
}

/// With the library gone as well there is nothing to anchor, so nothing is
/// created: the rebuild is for restoring, not for conjuring directories.
#[test]
fn a_deleted_bundle_folder_is_not_rebuilt_when_the_library_is_gone_too() {
    let (dir, record) = written(true, false);
    std::fs::remove_dir(dir.path().join("source")).unwrap();
    std::fs::remove_dir_all(dir.path().join("library")).unwrap();
    let report = reverse(&record).unwrap();
    assert_eq!(report.restored, 0);
    assert!(!report.refused.is_empty());
    assert!(!dir.path().join("source").exists());
}

/// One record per `statuses`, written through `UndoWriter` exactly as `apply`
/// writes them, with `planned` set the way `apply` sets it: the number of rows
/// the run was about to attempt.
fn run_of(planned: u32, statuses: &[&str]) -> (TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let file = source.join("Hellboy_v01.pdf");
    std::fs::write(&file, b"bytes").unwrap();
    let record = dir.path().join("run.record");
    let mut w =
        UndoWriter::create(&record, &source, &dir.path().join("library"), "komga", planned).unwrap();
    for status in statuses {
        let seq = w
            .record_pending(
                Path::new("Hellboy_v01.pdf"), &dest_for(), Mechanism::Stream, Mode::Move, &file,
                "Hellboy", Some(1), None, None, 3, "pdf",
            )
            .unwrap();
        // A `Stream` row that finished was `placed` first: streaming makes a
        // new inode, so `done` with no identity ahead of it is a shape nothing
        // writes, and a row the reverse could never identify. `reversible`
        // reads `expected()` now, so a fixture that skipped `placed` was
        // claiming a finished row the reverse would refuse.
        let identity = |p: &Path| {
            let m = std::fs::symlink_metadata(p).unwrap();
            (m.len(), m.ino(), m.mtime() * 1_000_000_000 + m.mtime_nsec())
        };
        match *status {
            "done" => {
                w.record_placed(seq, identity(&file)).unwrap();
                w.record_done(seq).unwrap();
            }
            "failed" => w.record_failed(seq).unwrap(),
            "placed" => w.record_placed(seq, identity(&file)).unwrap(),
            "pending" => {}
            other => panic!("no such status {other:?}"),
        }
    }
    (dir, record)
}

/// `apply` abandons the run on the first row that fails, and a row that failed
/// before creating anything is written `failed` — so an aborted run leaves no
/// `pending` row at all. Reading only `pending` called this a *completed* run,
/// and the menu then offered `k`: forget the only record of a half-migrated
/// library.
#[test]
fn a_run_abandoned_at_a_failed_row_is_interrupted() {
    let (_dir, record) = run_of(3, &["done", "failed"]);
    let s = summarise(&record).unwrap();
    assert_eq!((s.reversible, s.planned), (1, 3));
    assert!(s.interrupted, "an abandoned run is not a completed one");
}

/// The half the counts cannot see. The last planned row's destination landed
/// and its unlink failed, so `apply` leaves the row at `placed` on purpose —
/// `reversible` reaches `planned` while the run still aborted and a duplicate
/// sits in the library.
#[test]
fn a_placed_row_that_never_finished_is_interrupted_even_at_the_full_count() {
    let (_dir, record) = run_of(1, &["placed"]);
    let s = summarise(&record).unwrap();
    assert_eq!((s.reversible, s.planned), (1, 1), "the counts alone cannot tell");
    assert!(s.interrupted);
}

/// The half the rows cannot see. A crash during a `record_pending` write loses
/// that row's line, so every row the record still holds is `done` and only the
/// planned count knows one is missing.
#[test]
fn a_record_holding_fewer_rows_than_it_planned_is_interrupted() {
    let (_dir, record) = run_of(2, &["done"]);
    let s = summarise(&record).unwrap();
    assert_eq!((s.reversible, s.planned), (1, 2));
    assert!(s.interrupted);
}

/// And the half `planned` cannot be trusted for. A roots row with no count
/// parses as `planned: 0`, which no record this build writes can be — creation
/// is lazy, so a record always planned at least the row that made it — but a
/// record is a file a human can edit, which is what invariant 1 exists for.
/// With `planned` understated, the rows are the only evidence left, and
/// `failed` is evidence as much as `pending` is.
#[test]
fn a_record_that_understates_what_it_planned_is_still_read_from_its_rows() {
    let (_dir, record) = run_of(0, &["done", "failed"]);
    let s = summarise(&record).unwrap();
    assert_eq!((s.reversible, s.planned), (1, 0), "the count vouches for nothing here");
    assert!(s.interrupted);
}

// --- D60: the issue column in the record ---------------------------------

#[test]
fn the_column_list_places_issue_between_volume_and_subtitle() {
    // Position is the contract: `pending_fields` reads by index, so moving
    // this column silently reinterprets every row ever written.
    let c: Vec<&str> = COLUMNS.split(',').map(str::trim).collect();
    let v = c.iter().position(|f| *f == "volume").unwrap();
    assert_eq!(c[v + 1], "issue");
    assert_eq!(c[v + 2], "subtitle");
}

/// Every index `undo.rs` hard-codes, in one place. Inserting `issue` moved
/// `preset` and `planned` and the roots row came back as `unknown preset ""`;
/// this is the test that names the cause instead of fifteen failing fixtures.
///
/// All seventeen are listed deliberately. An earlier version pinned only
/// 4, 5 and 8-16 while its comment claimed to pin every one, and swapping
/// `size,inode` to `inode,size` in `COLUMNS` passed it — the length held and
/// every pinned index still named its field, while `read_manifest` went on
/// reading `columns[6]` as size. A reorder among same-typed adjacent columns
/// is the one mutation a partial list cannot see.
#[test]
fn the_column_list_is_read_at_the_indices_this_code_uses() {
    let c: Vec<&str> = COLUMNS.split(',').map(str::trim).collect();
    for (i, name) in [
        (0, "seq"), (1, "status"), (2, "mode"), (3, "mechanism"),
        (4, "source_name"), (5, "dest_relative"), (6, "size"), (7, "inode"),
        (8, "mtime_ns"), (9, "series"), (10, "volume"), (11, "issue"),
        (12, "subtitle"), (13, "run"), (14, "extension"), (15, "preset"),
        (16, "planned"),
    ] {
        assert_eq!(c[i], name, "column {i} moved; undo.rs reads it by index");
    }
    assert_eq!(c.len(), 17);
}

/// Drops the issue column from every full-width line of `record`, the way a
/// build before D60 wrote it. Returns false when nothing was shortened, which
/// a comma inside `TMPDIR` causes: `split(',')` would then find the roots row
/// over-wide, skip the removal, and leave a caller testing the current format
/// while believing it tested the old one.
fn downgrade_to_sixteen_columns(record: &Path) -> bool {
    let text = std::fs::read_to_string(record).unwrap();
    let want = COLUMNS.split(',').count();
    let mut shortened = false;
    let out: Vec<String> = text
        .lines()
        .map(|line| {
            let mut f: Vec<&str> = line.split(',').collect();
            if f.len() == want {
                f.remove(11);
                shortened = true;
            }
            f.join(",")
        })
        .collect();
    std::fs::write(record, out.join("\n") + "\n").unwrap();
    shortened
}

/// The whole record is refused up front, which is stronger than per-row
/// refusal: a header one column short trips the older-build guard and
/// `read_manifest` returns before any row is examined. That is all this test
/// claims. The row-shift case the header hides is
/// `a_short_pending_row_under_a_good_header_is_refused_per_row` below.
#[test]
fn a_record_whose_header_is_one_column_short_is_refused_whole() {
    let (_dir, record) = a_written_record();
    assert!(downgrade_to_sixteen_columns(&record));
    let err = reverse(&record).unwrap_err();
    assert!(
        format!("{err}").contains("older build"),
        "a stale record must say so rather than rebuild from shifted fields: {err}"
    );
}

/// The case the header guard hides, and the one the issue column actually
/// created: a row one column short under a header this build wrote. Nothing
/// refuses the file as a whole, so each such row must be *reported* and the
/// library left alone — a dropped row leaves a file in the library while the
/// report calls the undo clean.
#[test]
fn a_short_pending_row_under_a_good_header_is_refused_per_row() {
    let (dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    let want = COLUMNS.split(',').count();
    let mut shortened = false;
    let rows: Vec<String> = text
        .lines()
        .map(|l| {
            let mut f: Vec<&str> = l.split(',').collect();
            // Header and roots row kept intact. Shortening the header trips
            // the older-build guard and shortening the roots row moves
            // `preset`, and either one refuses the file before a row is read
            // — which is the whole point of this test being separate.
            let structural = f.first().is_some_and(|c| *c == "seq" || *c == "0");
            if !structural && f.len() == want {
                f.remove(11);
                shortened = true;
            }
            f.join(",")
        })
        .collect();
    assert!(shortened, "no row was shortened: {rows:?}");
    std::fs::write(&record, rows.join("\n") + "\n").unwrap();

    let placed = dir.path().join("library").join(dest_for());
    let before = placed.exists();
    assert!(before, "the fixture must have placed a library file");
    let report = reverse(&record).unwrap();
    assert_eq!(report.restored, 0, "a shifted row must not be rebuilt: {report:?}");
    // The refusal has to be ABOUT the pending row. `!is_empty()` alone passed
    // while `undo.rs`'s dropped-row report was deleted, because shortening
    // the `placed` and `done` rows orphans them and `orphan` pushes its own
    // refusals — a green test over the exact bug its doc comment names.
    assert!(
        report.refused.iter().any(|r| r.contains("rebuild")),
        "no refusal names the row that cannot be rebuilt: {report:?}"
    );
    assert_eq!(placed.exists(), before, "the library must be untouched");
}

/// Without a header there is nothing to trip the older-build guard, and every
/// row of an old-format record is dropped at parse time — which leaves both
/// interrupted signals reading false over no rows at all. The record then
/// looks like a finished run with nothing left to reverse, which is exactly
/// the shape `record::forget`'s sweep deletes, while `reverse` would have
/// refused it honestly with `unknown preset "1"`. The preset is the one field
/// still able to prove the columns mean what this build thinks they mean: it
/// is read at an index the insertion moved, so a stale row yields the old
/// `planned` value there instead of a preset id.
#[test]
fn a_headerless_record_from_an_older_build_is_unreadable_rather_than_clean() {
    let (dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    let want = text.lines().next().unwrap().split(',').count();
    let old: Vec<String> = text
        .lines()
        .skip(1)
        .map(|l| {
            let mut c: Vec<&str> = l.split(',').collect();
            if c.len() == want {
                c.remove(11);
            }
            c.join(",")
        })
        .collect();
    // Guards a vacuous pass: a comma inside TMPDIR would quote the roots row,
    // skip the removal, and leave this testing the current format.
    // The ROOTS row specifically: it is what carries `preset`, and moving
    // that is the whole mechanism here. Asserting "some line" let the roots
    // row stay full-width — the comma-in-TMPDIR case — and the test then
    // failed loudly for the wrong reason.
    assert!(
        old.iter()
            .any(|l| l.starts_with("0,roots") && l.split(',').count() == want - 1),
        "the roots row was not shortened, so this is not an old-format record: {old:?}"
    );

    let stale = dir.path().join("headerless.record");
    std::fs::write(&stale, old.join("\n") + "\n").unwrap();
    assert!(
        summarise(&stale).is_err(),
        "read as a clean run: {:?}",
        summarise(&stale).map(|s| (s.reversible, s.planned, s.interrupted))
    );
}
