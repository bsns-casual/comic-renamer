//! Recomputes destinations from metadata; never trusts a path string found in
//! a plan file (invariant 2, D14).
//!
//! The plan arrives as data the caller has already read, and the library lock
//! arrives already held. Both were once this module's own business, and both
//! moved out for the same reason: a function that re-reads the plan it was
//! asked about can run something other than what the user was shown, and a
//! function that takes its own lock cannot be called twice in one process
//! without deadlocking against itself.

use crate::fsops::{
    cstring,
    clone_excl, ensure_parent, find_collisions, rename_excl, select_mechanism, stream_verified,
};
use crate::undo::UndoWriter;
use crate::preset::{preset_by_id, Preset};
use crate::types::{
    ApplyError, ApplyResult, Mechanism, Mode, PlanHeader, PlanRow, Progress, Status, UndoReport,
};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Headroom the run will not consume, whichever is larger.
const RESERVE_FLOOR: u64 = 1 << 30;
const RESERVE_FRACTION: u64 = 10;

/// Path components that mean a sync client owns this tree. D23: on a
/// cloud-sync source a move is a server-side delete, and cloning a
/// placeholder silently hydrates it.
// shortcut: a path-name heuristic. There is no portable "is this synced" call;
// `SF_DATALESS` only fires on files already dehydrated, so it cannot answer
// for a fully-materialised synced folder.
const CLOUD_MARKERS: [&str; 6] = [
    "CloudStorage",
    "Dropbox",
    "Mobile Documents",
    "iCloud Drive",
    "OneDrive",
    "Google Drive",
];

/// Pre-flight before anything is touched: re-stat every source against its
/// `SourceStat`; re-check free space against required plus `max(1 GiB, 10%)`;
/// re-verify each mechanism and **never** silently fall back to streaming;
/// refuse a move off a cloud-sync root without an override (D23). Rows whose
/// status is not `Ready` are never executed.
///
/// The lock the caller holds is the one `reverse_run` holds: one library, one
/// lock, so a run and a reverse of that library cannot overlap. `records` is
/// the directory the record is written into, validated before the first row.
///
/// An edited plan is revalidated, not rejected (§6.9). Everything here
/// recomputes from metadata anyway, so the plan's checksum is a report about
/// what a human touched, not a gate — which is why the caller reads the plan
/// and this takes what it read.
// shortcut: per-row re-stat is a pre-flight batch, so a row executing minutes
// later on a live-syncing tree uses a stale stat; re-stat per operation if it bites
pub fn apply(
    lock: &Lock,
    header: &PlanHeader,
    rows: &[PlanRow],
    records: &Path,
    allow_cloud_move: bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<ApplyResult, ApplyError> {
    apply_inner(Some((lock, records)), header, rows, allow_cloud_move, progress)
}

/// As `apply`, but stops before executing. Everything up to and including the
/// collision check runs; nothing is written and no record is created, so it
/// needs neither the lock nor the record store.
pub fn check(
    header: &PlanHeader,
    rows: &[PlanRow],
    allow_cloud_move: bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<ApplyResult, ApplyError> {
    apply_inner(None, header, rows, allow_cloud_move, progress)
}

/// `run` is `None` for `check` and `Some` for `apply`. Carrying the lock and
/// the record store together is what makes "this call may write" one condition
/// rather than a flag beside two arguments that only matter when it is set.
fn apply_inner(
    run: Option<(&Lock, &Path)>,
    header: &PlanHeader,
    rows: &[PlanRow],
    allow_cloud_move: bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<ApplyResult, ApplyError> {
    let preset = preset_by_id(&header.preset)
        .ok_or_else(|| ApplyError::UnknownPreset(header.preset.clone()))?;

    // §6.11: the plan file is untrusted too, so both roots are resolved here
    // rather than taken as written. `dest_root` is the containment anchor for
    // every write — unvalidated, a plan naming `/` lands the library at the
    // filesystem root.
    let source_root = canonical_dir(&header.source_root)?;
    let dest_root = canonical_dir(&header.dest_root)?;
    if source_root.starts_with(&dest_root) || dest_root.starts_with(&source_root) {
        return Err(ApplyError::RootsOverlap);
    }
    // Canonical from here down, and that matters past containment: the record
    // stores these roots and `record::supersede` compares them by exact
    // equality, so a run reached through a symlinked library must write the
    // same spelling the next run will compare against.
    let header = PlanHeader {
        source_root: source_root.clone(),
        dest_root,
        ..header.clone()
    };

    if header.mode == Mode::Move && !allow_cloud_move && is_cloud_synced(&header.source_root) {
        return Err(ApplyError::CloudSourceMove(header.source_root.clone()));
    }

    // Whole-run refusals, before any row is considered individually.
    let eligible: Vec<&PlanRow> = rows
        .iter()
        .filter(|row| row.status == Status::Mapped)
        .collect();

    for row in &eligible {
        verify_inside_source_root(row, &source_root)?;
        verify_unchanged(row)?;
        verify_destination(row, &header, preset.as_ref())?;
        verify_mechanism(row, &header)?;
    }
    verify_space(&eligible, &header)?;

    // A destination that is already occupied is a first-class collision and
    // the row is refused — never overwritten, never auto-suffixed (§6.10).
    // Row-level, so it does not abort the run.
    let collisions = find_collisions(rows, &header.dest_root, preset.as_ref());
    let colliding: Vec<&str> = collisions
        .iter()
        .flat_map(|c| c.row_ids.iter().map(String::as_str))
        .collect();

    let runnable: Vec<&PlanRow> = eligible
        .iter()
        .filter(|row| !colliding.contains(&row.id.as_str()))
        .copied()
        .collect();
    let skipped = rows.len() - runnable.len();

    // `None` is `check`: everything above has run, and nothing below it
    // writes. The lock and the record store are only bound when they are
    // about to be used.
    let Some((lock, records)) = run else {
        return Ok(ApplyResult {
            executed: runnable.len(),
            skipped,
            failed: 0,
            record: None,
        });
    };

    // The lock has to be *this* library's. Holding one for somewhere else
    // leaves this library unlocked while the type says otherwise, and nothing
    // below would notice: two runs would interleave and each would sweep the
    // other's record. Both sides are canonical, so a symlinked library still
    // matches itself.
    if lock.dest_root != header.dest_root {
        return Err(ApplyError::WrongLibrary {
            locked: lock.dest_root.clone(),
            planned: header.dest_root.clone(),
        });
    }

    // Before the first row, not from it: a run that cannot be recorded does
    // not happen, and the refusal has to arrive while that is still true.
    verify_records(records)?;

    // Created lazily, on the first row that actually executes. Creating it
    // up front meant a run that refused every row still wrote a header-only
    // record — and the sweep below would then have taken it as this run's
    // own and deleted the previous run's, which is the only way back from a
    // run that did place files.
    let mut record: Option<Record> = None;

    let total = runnable.len() as u64;
    let mut executed = 0usize;
    for (index, row) in runnable.iter().enumerate() {
        progress(Progress {
            done: index as u64,
            total,
            current: row.source.to_string_lossy().into_owned(),
        });
        let open = match &mut record {
            Some(open) => open,
            None => {
                record = Some(Record::create(records, &header, total as u32)?);
                record.as_mut().expect("just created")
            }
        };
        // Named here rather than inside `execute`, because the first thing
        // that notices a vanished source is `record_pending`'s own `stat` —
        // before the operation this was wrapped around.
        execute(row, &header, preset.as_ref(), &mut open.writer, progress)
            .map_err(|e| named_if_source_gone(row, e))?;
        executed += 1;
    }
    progress(Progress {
        done: total,
        total,
        current: String::new(),
    });

    // Only when a record was created, and under the lock the caller is
    // holding. Both halves are load-bearing. `supersede` deletes every record
    // for this root pair *other than* the one named, so calling it after a run
    // that recorded nothing names nothing and takes the lot — run `apply`
    // twice, collide on every row the second time, and 400 placed files stop
    // being reversible. The lock is what makes `is_live` then `remove_file`
    // something other than check-then-act on exactly that file.
    let written = match &record {
        Some(open) => {
            match crate::record::supersede(
                records,
                &open.path,
                &header.source_root,
                &header.dest_root,
            ) {
                // Said, not discarded. A re-import takes the earlier run's
                // only way back, and the user would otherwise learn that by
                // running `undo` and being told there was nothing to do.
                Ok(swept) if swept > 0 => progress(Progress {
                    done: 0,
                    total: 0,
                    current: format!(
                        "{swept} earlier run(s) of this bundle can no longer be undone"
                    ),
                }),
                Ok(_) => {}
                // Also said, and for the same reason, but it must not throw
                // the run away with it. Every row has already executed by
                // here, and the caller prints `N placed` from the result — so
                // returning `Err` meant a run that moved all 600 files never
                // told anyone it had. Removing the record store mid-run did
                // exactly that: the record's descriptor still pointed at an
                // unlinked inode, every row landed, and the only output was
                // "the record store is unusable".
                //
                // What is lost is tidying. Earlier records that survive the
                // sweep cost an extra offer, not data — and they are the
                // *older* runs' records, which is the safe direction.
                Err(e) => progress(Progress {
                    done: 0,
                    total: 0,
                    current: format!(
                        "the record store could not be tidied ({}: {e}); this run may not \
                         be reversible — `comic-renamer undo --list` shows what is there",
                        records.display()
                    ),
                }),
            }
            Some(open.path.clone())
        }
        None => None,
    };

    Ok(ApplyResult {
        executed,
        skipped,
        failed: 0,
        record: written,
    })
}

/// This run's record: the writer, its path, and the `flock` that says a run
/// still owns it. Dropping it closes both, which is how a crashed run leaves
/// a record that reads as abandoned rather than live.
struct Record {
    writer: UndoWriter,
    path: PathBuf,
    _held: std::fs::File,
}

impl Record {
    fn create(records: &Path, header: &PlanHeader, planned: u32) -> Result<Record, ApplyError> {
        let path = records.join(crate::record::basename(std::time::SystemTime::now()));
        let writer = UndoWriter::create(
            &path,
            &header.source_root,
            &header.dest_root,
            &header.preset,
            planned,
        )?;
        // After the `O_EXCL` create, so the name is already this run's. The
        // hold is a second descriptor on it: the writer's own is not flocked,
        // and `record::is_live` asks about the flock.
        let held = crate::record::hold(&path)
            .map_err(|e| ApplyError::Records(format!("{}: {e}", path.display())))?;
        Ok(Record {
            writer,
            path,
            _held: held,
        })
    }
}

/// Review Focus 4. `ensure_root` answers "is there a directory here", which a
/// read-only one satisfies; the probe answers "can this run write into it",
/// which is the question. Without the probe the answer arrives from
/// `Record::create` instead — still before any row executes, but as a bare
/// `Io` error about a path the user never named.
fn verify_records(dir: &Path) -> Result<(), ApplyError> {
    let fail = |e: std::io::Error| ApplyError::Records(format!("{}: {e}", dir.display()));
    crate::fsops::ensure_root(dir).map_err(fail)?;
    // Named by pid, so two libraries sharing one store do not probe over each
    // other, and `record::list` ignores it: it is not a `.record`.
    let probe = dir.join(format!(".comic-renamer.{}.probe", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)
        .map_err(fail)?;
    std::fs::remove_file(&probe).map_err(fail)?;
    Ok(())
}

/// One row. The ordering is the whole point: the destination is in place and
/// verified **before** the source is unlinked, never the other way round.
///
/// A failure returns, abandoning the remaining rows. Continuing past a
/// permission error turns one refusal into a half-moved library.
fn execute(
    row: &PlanRow,
    header: &PlanHeader,
    preset: &dyn Preset,
    undo: &mut UndoWriter,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), ApplyError> {
    // Recomputed at the point of use, not merely checked earlier. Invariant 2
    // says never trust a path string found in a plan file; reading
    // `destination_preview` here made that true only by loop ordering, which
    // nothing in the types enforces.
    let relative = &preset.destination(&row.resolved(), &row.extension);
    let source_name = Path::new(row.source.file_name().unwrap_or_default());

    // Pending lands and is fsynced before the operation is attempted. A
    // record entry still sitting in a buffer when the power goes is a file
    // nobody can find again.
    //
    // The fields travel with the row because the record reverses without the
    // plan: `undo::reverse` rebuilds this same `relative` from them and
    // refuses the row unless the preset produces what the row names.
    let seq = undo.record_pending(
        source_name,
        relative,
        row.mechanism,
        header.mode,
        &row.source,
        &row.series,
        row.volume,
        row.issue,
        row.subtitle.as_deref(),
        row.run,
        &row.extension,
    )?;

    // ensure_parent after record_pending, so a created series directory is
    // recorded rather than left behind unaccounted for.
    ensure_parent(&header.dest_root, relative)?;

    // A `failed` row removes the ambiguity a bare `pending` used to carry
    // between "crashed mid-operation" and "refused at the syscall" — the
    // ambiguity `reverse` resolved by matching on size.
    //
    // It is written **only when nothing was created**. A failure after
    // `placed` — a verified copy whose unlink then failed — must leave the
    // row at `placed`, because that is the record that lets `reverse` clear
    // the duplicate. Writing `failed` over it throws away the one piece of
    // information the state depends on.
    let mut created = false;
    let outcome = perform(row, header, relative, seq, undo, &mut created, progress);
    if outcome.is_err() {
        if !created {
            let _ = undo.record_failed(seq);
        }
        return outcome;
    }

    undo.record_done(seq)?;
    Ok(())
}

/// Names a row whose source vanished between the pre-flight and its turn.
///
/// The source is the one thing that can be missing here without a caller bug,
/// so it is *looked at* rather than assumed: a `NotFound` from somewhere else
/// in the row — a series directory someone removed after `ensure_parent` made
/// it — must not be reported as the source being gone. This runs only after a
/// failure, and only reads, so it is a diagnostic and not a check before an
/// action.
fn named_if_source_gone(row: &PlanRow, e: ApplyError) -> ApplyError {
    if !matches!(&e, ApplyError::Io(io) if io.kind() == std::io::ErrorKind::NotFound) {
        return e;
    }
    let gone = std::fs::symlink_metadata(&row.source)
        .err()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound);
    if gone {
        return ApplyError::SourceGone(row.id.clone(), row.source.clone());
    }
    e
}

fn perform(
    row: &PlanRow,
    header: &PlanHeader,
    relative: &Path,
    seq: u64,
    undo: &mut UndoWriter,
    created: &mut bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), ApplyError> {
    match row.mechanism {
        // Atomic, and there is nothing to hash: the inode does not move, so
        // there is no new identity to record and no `placed` state.
        Mechanism::Rename => {
            rename_excl(&row.source, &header.dest_root, relative)?;
            // No new inode and so no `placed`, but the file has moved: a
            // later failure must not claim nothing was created.
            *created = true;
        }
        Mechanism::Clone => {
            match clone_excl(&row.source, &header.dest_root, relative) {
                Ok(()) => {
                    // Before, not after: a failure of the `placed` write
                    // itself would otherwise record `failed` over a
                    // destination that exists, and `reverse` skips a
                    // `failed` row in silence.
                    //
                    // shortcut: reasoned, not tested. Reaching it needs the
                    // manifest write to fail after the destination landed —
                    // a reviewer produced the sibling state with
                    // RLIMIT_FSIZE, but nothing in the suite distinguishes
                    // this ordering from the wrong one.
                    *created = true;
                    placed(undo, seq, &header.dest_root, relative)?;
                }
                // An occupied destination stays a refusal; it is never a
                // reason to try again by another route.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(ApplyError::Io(e))
                }
                // §6.10 wants the clone probed at runtime with a fallback to
                // streaming. Announced, and counted by `required_bytes`.
                Err(e) => {
                    progress(Progress {
                        done: 0,
                        total: 0,
                        current: format!(
                            "clone unavailable ({e}); streaming {}",
                            relative.display()
                        ),
                    });
                    stream_verified(&row.source, &header.dest_root, relative)?;
                    *created = true;
                    placed(undo, seq, &header.dest_root, relative)?;
                }
            }
            if header.mode == Mode::Move {
                std::fs::remove_file(&row.source)?;
            }
        }
        Mechanism::Stream => {
            // temp -> fsync -> verify -> rename_excl, all inside.
            stream_verified(&row.source, &header.dest_root, relative)?;
            *created = true;
            // A hard gate before the unlink. Ignore its failure and the row
            // is a bare `pending` with the source gone and the destination's
            // inode unrecorded — the user's only copy in the library under a
            // name `reverse` will not touch.
            placed(undo, seq, &header.dest_root, relative)?;
            if header.mode == Mode::Move {
                // THEN unlink. stream_verified never touches the source, so
                // this is the only place a move loses its original, and it
                // happens after the copy is verified and in place.
                std::fs::remove_file(&row.source)?;
            }
        }
    }
    Ok(())
}

/// Records the destination's own identity. Never with an empty field: the
/// same bytes would mean both "this status carries none" and "the stat
/// failed", and `reverse` would silently fall back to a weaker match.
fn placed(
    undo: &mut UndoWriter,
    seq: u64,
    dest_root: &Path,
    relative: &Path,
) -> Result<(), ApplyError> {
    // Through a resolved descriptor, not `symlink_metadata` on a joined
    // path: a component swapped between the write and the stat would record
    // an identity belonging to something else, and every later `reverse`
    // would refuse the row.
    undo.record_placed(seq, crate::fsops::stat_at(dest_root, relative)?)?;
    Ok(())
}

/// Reverses one recorded run. Takes the record, not the plan: the record
/// carries its own roots, preset and per-row fields, so a plan that was edited
/// or deleted since the run cannot change what gets reversed, and a plan
/// handed back in cannot name a destination the run never touched.
///
/// `&Lock` is the obligation, not an input: the caller holds the library lock
/// for as long as this runs, which is the same lock `apply` runs under.
pub fn reverse_run(lock: &Lock, record: &Path) -> Result<UndoReport, ApplyError> {
    // Absent means there is nothing to undo, not an error: never written, or
    // already reversed and deleted. `undo` is run twice more often than once.
    if !record.exists() {
        return Ok(UndoReport::default());
    }
    // The lock has to be *this record's* library, for the same reason `apply`
    // checks it above: a `Lock` for somewhere else leaves this library
    // unlocked while the type says otherwise, and a reverse mutates a library
    // exactly as a run does. Both sides are canonical — `apply_inner` resolves
    // the roots before writing them, and `Lock::library` resolves its own — so
    // a library reached through a symlink still matches itself.
    let recorded = crate::undo::summarise(record)?.dest_root;
    if lock.dest_root != recorded {
        return Err(ApplyError::WrongLibrary {
            locked: lock.dest_root.clone(),
            planned: recorded,
        });
    }
    let report = crate::undo::reverse(record)?;

    // Deleted, not renamed aside. The rename existed because the record was
    // named after the plan and `O_EXCL` then refused that plan forever; a
    // record the tool names itself has no such tie, and §3's lifecycle table
    // says a fully reversed record is gone.
    //
    // Gated on refusals only. `skipped` counts rows that were already
    // reversed — the normal case on a second pass — so requiring zero of
    // those would keep every resumed undo's record forever.
    //
    // The failure propagates rather than being swallowed: a record that
    // outlives its reverse is the one the next `undo` picks up, and it would
    // restore nothing every time while saying so in a report nobody reads as
    // a problem.
    //
    // A record that is already gone is the end state this wanted, so it is not
    // a failure: `menu::decline` deletes records without the lock, and a
    // reverse that restored every file used to exit non-zero with
    // `No such file or directory` if the record went first.
    if report.refused.is_empty() {
        crate::record::forget(record)?;
    }
    Ok(report)
}

/// §6.9 requires no absolute paths outside the chosen roots, and §6.11 says
/// the plan is re-validated here because the plan file is untrusted.
///
/// Exact rather than a prefix test, and free: `scan` is non-recursive and
/// builds every candidate as `read_dir(source_root)` + `entry.path()`, so a
/// genuine row's source is always one component inside the canonical root.
/// Without this, an edited plan moves any file on the system into the
/// library — and the cloud-sync refusal, which keys on the root while every
/// operation uses `row.source`, rests on the same relationship.
fn verify_inside_source_root(row: &PlanRow, source_root: &Path) -> Result<(), ApplyError> {
    if row.source.parent() != Some(source_root) {
        return Err(ApplyError::OutsideSourceRoot(
            row.id.clone(),
            row.source.clone(),
        ));
    }
    Ok(())
}

fn canonical_dir(path: &Path) -> Result<PathBuf, ApplyError> {
    // Named, not bare: see `scan::directory`.
    let resolved = std::fs::canonicalize(path).map_err(|e| {
        ApplyError::Io(std::io::Error::new(e.kind(), format!("{}: {e}", path.display())))
    })?;
    if !resolved.is_dir() {
        return Err(ApplyError::Io(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            format!("{} is not a directory", path.display()),
        )));
    }
    Ok(resolved)
}

/// D14: the stored `destination_preview` is a preview. The real destination is
/// recomputed, and a disagreement means the plan was edited in a way that
/// would have moved a file somewhere else.
fn verify_destination(
    row: &PlanRow,
    header: &PlanHeader,
    preset: &dyn Preset,
) -> Result<(), ApplyError> {
    let recomputed = preset.destination(&row.resolved(), &row.extension);
    if recomputed != row.destination_preview {
        return Err(ApplyError::Tampered(row.id.clone()));
    }
    let _ = header;
    Ok(())
}

/// The reference bundle's file count read 42 then 40 during a single session,
/// so this is not hypothetical (§6.10).
fn verify_unchanged(row: &PlanRow) -> Result<(), ApplyError> {
    let meta = std::fs::symlink_metadata(&row.source)
        .map_err(|_| ApplyError::SourceChanged(row.source.clone()))?;
    let now = (
        meta.len(),
        meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
        meta.ino(),
    );
    let then = (
        row.source_stat.size,
        row.source_stat.mtime_ns,
        row.source_stat.inode,
    );
    if now != then {
        return Err(ApplyError::SourceChanged(row.source.clone()));
    }
    Ok(())
}

/// Never a silent fallback to streaming: a plan that said `rename` and now
/// needs a copy is a different operation with different costs.
fn verify_mechanism(row: &PlanRow, header: &PlanHeader) -> Result<(), ApplyError> {
    let current = select_mechanism(&row.source, &header.dest_root, header.mode)
        .map_err(|_| ApplyError::MechanismChanged(row.id.clone()))?;
    if current != row.mechanism {
        return Err(ApplyError::MechanismChanged(row.id.clone()));
    }
    Ok(())
}

/// Only streaming rows consume space; a rename moves nothing and a clone is
/// copy-on-write.
fn verify_space(eligible: &[&PlanRow], header: &PlanHeader) -> Result<(), ApplyError> {
    // Clone rows count too. `select_mechanism` returns Clone for any
    // same-volume copy without probing the filesystem, and execution falls
    // back to streaming when the clone fails — so a --copy run of nothing but
    // Clone rows was checked against zero bytes and then streamed the lot.
    // Measured on HFS+: 919 MiB free, usable() 0, a 400 MiB payload admitted
    // and written.
    //
    // A clone that succeeds is copy-on-write and costs nothing, so this
    // over-reserves on APFS. Refusing a run that would have fitted is
    // recoverable; ENOSPC half way through a 31 GiB copy is not.
    let required = required_bytes(eligible);
    if required == 0 {
        return Ok(());
    }
    let free = available_bytes(&header.dest_root)?;
    match usable(free) {
        available if required > available => Err(ApplyError::InsufficientSpace {
            required,
            available,
        }),
        _ => Ok(()),
    }
}

/// Bytes a run may actually write.
///
/// Clone rows count. `select_mechanism` returns Clone for any same-volume
/// copy without probing the filesystem, and execution falls back to streaming
/// when the clone fails — so a `--copy` run of nothing but Clone rows was
/// checked against zero and then streamed the lot. Measured on HFS+: 919 MiB
/// free, `usable()` 0, a 400 MiB payload admitted and written.
///
/// A clone that succeeds is copy-on-write and costs nothing, so this
/// over-reserves on APFS. Refusing a run that would have fitted is
/// recoverable; ENOSPC half way through a 31 GiB copy is not.
pub fn required_bytes(rows: &[&PlanRow]) -> u64 {
    rows.iter()
        .filter(|row| matches!(row.mechanism, Mechanism::Stream | Mechanism::Clone))
        .map(|row| row.source_stat.size)
        .sum()
}

/// Free space a run may consume: everything above a reserve of whichever is
/// larger, 1 GiB or a tenth. Filling a volume to the last byte is its own
/// kind of data loss.
pub fn usable(free: u64) -> u64 {
    free.saturating_sub(RESERVE_FLOOR.max(free / RESERVE_FRACTION))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserve_is_the_larger_of_a_gibibyte_and_a_tenth() {
        const GIB: u64 = 1 << 30;
        // Below 10 GiB the floor binds.
        assert_eq!(usable(4 * GIB), 3 * GIB);
        // Above it, the fraction does.
        assert_eq!(usable(100 * GIB), 90 * GIB);
        // Exactly at the crossover both give the same answer.
        assert_eq!(usable(10 * GIB), 9 * GIB);
    }

    #[test]
    fn a_clone_row_reserves_its_full_size() {
        let row = |mechanism, size| PlanRow {
            id: "r".into(),
            source: PathBuf::from("/src/a.pdf"),
            destination_preview: PathBuf::new(),
            extension: "pdf".into(),
            series: "S".into(),
            volume: None,
            issue: None,
            subtitle: None,
            run: 1,
            status: Status::Mapped,
            mechanism,
            source_stat: crate::types::SourceStat {
                size,
                mtime_ns: 0,
                inode: 1,
            },
            record: None,
        };
        let rows = [
            row(Mechanism::Clone, 400),
            row(Mechanism::Stream, 30),
            row(Mechanism::Rename, 1_000_000),
        ];
        let borrowed: Vec<&PlanRow> = rows.iter().collect();
        // Clone and Stream count; a rename moves no bytes.
        assert_eq!(required_bytes(&borrowed), 430);
    }

    #[test]
    fn a_volume_with_less_than_the_reserve_yields_nothing() {
        // Never a wrapped, enormous "available" that waves a run through.
        assert_eq!(usable(0), 0);
        assert_eq!(usable(1 << 20), 0);
    }
}

fn available_bytes(path: &Path) -> std::io::Result<u64> {
    let name = cstring(path)?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `name` is a valid NUL-terminated path for the call, and `stat`
    // is only read after statvfs reports success.
    let rc = unsafe { libc::statvfs(name.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
}

fn is_cloud_synced(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        CLOUD_MARKERS.iter().any(|marker| name.contains(marker))
    })
}

/// An advisory `flock(2)` on one file per library, inside the record store.
///
/// **Per library, not per plan.** Two plans over one library are two runs that
/// must not interleave, and a plan is a file a user can copy; the library is
/// the thing being changed, so it is the thing that gets the lock.
///
/// **`flock`, not `O_EXCL`.** The kernel releases an `flock` when its owner
/// dies; an `O_EXCL` file survives a `SIGKILL` and has to be deleted by hand
/// — and a crash is exactly when `undo` is needed, so the stale lock blocked
/// the only recovery path. `O_EXCL` is still right for a *record*, where its
/// job is refusing to land on a name another run already owns.
///
/// Held by the caller, so one process can run a pre-flight, a disclosure, a
/// prompt and then `apply` and `reverse_run` without deadlocking against
/// itself — which an internal acquire per call could not.
/// It **names the library it locks.** A `Lock` that only proved *a* lock was
/// held would let `apply(&lock_for_a, &header_for_b, …)` compile and run, and
/// a type that does not identify what it locks delivers less than taking
/// `&Lock` claims. The only caller derives both from one header today, which
/// is why this is cheap rather than why it is unnecessary.
pub struct Lock {
    /// Held open for the lifetime of the lock: closing it releases the flock.
    _file: std::fs::File,
    /// Canonical, so it compares equal to the `dest_root` `apply_inner`
    /// resolves — which is the same reason the record stores canonical roots.
    dest_root: PathBuf,
}

impl Lock {
    pub fn library(records: &Path, dest_root: &Path) -> Result<Lock, ApplyError> {
        let fail = |e: std::io::Error| ApplyError::Records(format!("{}: {e}", records.display()));
        crate::fsops::ensure_root(records).map_err(fail)?;
        // **One** resolution, and both the lock's filename and the root this
        // reports are taken from it. Two calls — one inside `lock_name`, one
        // for the field — let a root whose resolution changed in between lock
        // one library and name another: 65 of 300 attempts against a
        // repointed symlink ran to completion holding the *other* library's
        // lock, so the library actually written was unlocked throughout while
        // `apply_inner`'s identity check, which compares only the field, saw
        // nothing wrong.
        let canonical = canonical_dir(dest_root)?;
        // Hashed, so every spelling of one library is one lock and a long or
        // odd root is still one filename.
        let name = crate::record::lock_name_of(&canonical);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(records.join(name))
            .map_err(fail)?;
        // Task 1 extracted this so the `unsafe` block exists exactly once;
        // `record::hold` is its other caller.
        if !crate::fsops::flock_nb(&file)? {
            return Err(ApplyError::Locked);
        }
        Ok(Lock {
            _file: file,
            dest_root: canonical,
        })
    }

    /// The library this lock is for, canonical. `apply` and `reverse_run`
    /// compare it against the root they are about to write; a test needs it to
    /// check that the lock *file* on disk is the one for this root.
    pub fn dest_root(&self) -> &Path {
        &self.dest_root
    }
}
