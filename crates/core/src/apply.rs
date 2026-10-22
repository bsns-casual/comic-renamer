//! Recomputes destinations from metadata; never trusts a path string found in
//! a plan file (invariant 2, D14).
//!
//! Task 11 is the pre-flight half. **Nothing here writes to either tree** —
//! the only file it creates is its own lock, beside the plan. Execution and
//! the undo manifest are Task 12.

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
/// At Task 11 `executed` counts the rows that **passed** pre-flight; nothing
/// is written, and every test wraps both trees in a guard that proves it.
/// Task 12 makes the count literal.
// shortcut: per-row re-stat is a pre-flight batch, so a row executing minutes
// later on a live-syncing tree uses a stale stat; re-stat per operation if it bites
pub fn apply(
    plan: &Path,
    allow_cloud_move: bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<ApplyResult, ApplyError> {
    apply_inner(plan, allow_cloud_move, false, progress)
}

/// As `apply`, but stops before executing. Everything up to and including the
/// collision check runs; nothing is written and no manifest is created.
pub fn check(
    plan: &Path,
    allow_cloud_move: bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<ApplyResult, ApplyError> {
    apply_inner(plan, allow_cloud_move, true, progress)
}

fn apply_inner(
    plan: &Path,
    allow_cloud_move: bool,
    dry_run: bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<ApplyResult, ApplyError> {
    // The lock comes first: two runs over one plan is the one failure a later
    // check cannot undo. `undo` takes the same lock (Task 12).
    let _lock = Lock::acquire(plan)?;

    // An edited plan is revalidated, not rejected (§6.9). Everything below
    // recomputes from metadata anyway, so the checksum is a report about what
    // a human touched, not a gate.
    let (header, rows, edited) = crate::plan::read_allowing_edits(plan)?;
    if edited {
        progress(Progress {
            done: 0,
            total: rows.len() as u64,
            current: "plan has been edited since the scan; revalidating".into(),
        });
    }

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
    let header = PlanHeader {
        source_root: source_root.clone(),
        dest_root,
        ..header
    };

    if header.mode == Mode::Move && !allow_cloud_move && is_cloud_synced(&header.source_root) {
        return Err(ApplyError::CloudSourceMove(header.source_root.clone()));
    }

    // Whole-run refusals, before any row is considered individually.
    let eligible: Vec<&PlanRow> = rows
        .iter()
        .filter(|row| matches!(row.status, Status::Ready(_)))
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
    let collisions = find_collisions(&rows, &header.dest_root, preset.as_ref());
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

    if dry_run {
        return Ok(ApplyResult {
            executed: runnable.len(),
            skipped,
            failed: 0,
        });
    }

    // Created lazily, on the first row that actually executes. Creating it
    // up front meant a run that refused every row still wrote a header-only
    // manifest, and `O_EXCL` then rejected the plan forever — so fixing the
    // collision and re-running was impossible.
    let mut undo: Option<UndoWriter> = None;

    let total = runnable.len() as u64;
    let mut executed = 0usize;
    for (index, row) in runnable.iter().enumerate() {
        progress(Progress {
            done: index as u64,
            total,
            current: row.source.to_string_lossy().into_owned(),
        });
        let writer = match &mut undo {
            Some(writer) => writer,
            None => {
                // `O_EXCL`: re-running a plan must not truncate the previous
                // run's manifest, the only record of where those files went.
                undo = Some(UndoWriter::create(
                    &undo_path(plan),
                    &header.source_root,
                    &header.dest_root,
                )?);
                undo.as_mut().expect("just created")
            }
        };
        execute(row, &header, preset.as_ref(), writer, progress)?;
        executed += 1;
    }
    progress(Progress {
        done: total,
        total,
        current: String::new(),
    });

    Ok(ApplyResult {
        executed,
        skipped,
        failed: 0,
    })
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
    // manifest entry still sitting in a buffer when the power goes is a file
    // nobody can find again.
    let seq = undo.record_pending(
        source_name,
        relative,
        row.mechanism,
        header.mode,
        &row.source,
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

/// Reverses a run. Takes the **plan**, not the manifest (D38): the plan's
/// roots anchor every path the manifest names, the preset decides which
/// destinations are recognisable, and both share one lock with `apply`.
///
/// The manifest remains the record of what actually happened — only the
/// anchor moved. Undoing from intent would attempt to reverse operations that
/// never ran, which §6.13's original reasoning rightly refused.
pub fn undo(plan: &Path) -> Result<UndoReport, ApplyError> {
    let _lock = Lock::acquire(plan)?;
    let (header, rows, _edited) = crate::plan::read_allowing_edits(plan)?;
    let preset = preset_by_id(&header.preset)
        .ok_or_else(|| ApplyError::UnknownPreset(header.preset.clone()))?;

    let source_root = canonical_dir(&header.source_root)?;
    let dest_root = canonical_dir(&header.dest_root)?;
    if source_root.starts_with(&dest_root) || dest_root.starts_with(&source_root) {
        return Err(ApplyError::RootsOverlap);
    }

    // Invariant 9: a row is only recognised if the preset still produces its
    // destination, which is what stops a hand-written manifest naming an
    // arbitrary relative path.
    let recomputed: Vec<PathBuf> = rows
        .iter()
        .map(|row| preset.destination(&row.resolved(), &row.extension))
        .collect();

    let manifest = undo_path(plan);
    // Idempotent, which Task 12 requires: a manifest that is absent — never
    // written, or renamed aside by a previous complete reverse — means there
    // is nothing to undo, not an error.
    if !manifest.exists() {
        return Ok(UndoReport::default());
    }
    let report = crate::undo::reverse(&manifest, &source_root, &dest_root, &recomputed)?;

    // A fully reversed manifest is renamed aside: the audit trail survives
    // and the plan becomes runnable again. Left in place, a successful undo
    // meant O_EXCL refused that plan forever.
    // Gated on refusals only. `skipped` counts rows that were already
    // reversed — the normal case on a second pass and after a pruned parent
    // — so requiring zero of them left the manifest in place and `O_EXCL`
    // refused the plan forever.
    if report.refused.is_empty() {
        let mut name = manifest.file_name().unwrap_or_default().to_os_string();
        name.push(".reversed");
        let _ = std::fs::rename(&manifest, manifest.with_file_name(name));
    }
    Ok(report)
}

/// Beside the plan, like the lock.
pub fn undo_path(plan: &Path) -> PathBuf {
    let mut name = plan.file_name().unwrap_or_default().to_os_string();
    name.push(".undo.csv");
    plan.with_file_name(name)
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
    let resolved = std::fs::canonicalize(path)?;
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
            subtitle: None,
            run: 1,
            status: Status::Ready(crate::types::Basis::Unambiguous),
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

/// An advisory `flock(2)` on a file beside the plan.
///
/// **`flock`, not `O_EXCL`.** The kernel releases an `flock` when its owner
/// dies; an `O_EXCL` file survives a `SIGKILL` and has to be deleted by hand
/// — and a crash is exactly when `undo` is needed, so the stale lock blocked
/// the only recovery path. `O_EXCL` is still right for the *manifest*, where
/// its job is refusing to truncate a previous run's record.
///
/// `undo` takes this same lock, which Task 11 always required and which two
/// separate lock files silently defeated.
pub struct Lock {
    /// Held open for the lifetime of the lock: closing it releases the flock.
    _file: std::fs::File,
}

impl Lock {
    pub fn acquire(plan: &Path) -> Result<Lock, ApplyError> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path(plan))?;
        // SAFETY: a valid open descriptor we own, for the duration of the call.
        let rc = unsafe {
            libc::flock(
                std::os::fd::AsRawFd::as_raw_fd(&file),
                libc::LOCK_EX | libc::LOCK_NB,
            )
        };
        if rc != 0 {
            let e = std::io::Error::last_os_error();
            return match e.raw_os_error() {
                Some(libc::EWOULDBLOCK) => Err(ApplyError::Locked),
                _ => Err(ApplyError::Io(e)),
            };
        }
        Ok(Lock { _file: file })
    }
}

pub fn lock_path(plan: &Path) -> PathBuf {
    let mut name = plan.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    plan.with_file_name(name)
}
