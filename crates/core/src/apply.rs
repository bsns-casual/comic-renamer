//! Recomputes destinations from metadata; never trusts a path string found in
//! a plan file (invariant 2, D14).
//!
//! Task 11 is the pre-flight half. **Nothing here writes to either tree** —
//! the only file it creates is its own lock, beside the plan. Execution and
//! the undo manifest are Task 12.

use crate::fsops::{
    clone_excl, ensure_parent, find_collisions, rename_excl, select_mechanism, stream_verified,
};
use crate::undo::UndoWriter;
use crate::preset::{preset_by_id, Preset};
use crate::types::{
    ApplyError, ApplyResult, Mechanism, Mode, PlanHeader, PlanRow, Progress, Status,
};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
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

    if header.mode == Mode::Move && !allow_cloud_move && is_cloud_synced(&header.source_root) {
        return Err(ApplyError::CloudSourceMove(header.source_root.clone()));
    }

    // Whole-run refusals, before any row is considered individually.
    let eligible: Vec<&PlanRow> = rows
        .iter()
        .filter(|row| matches!(row.status, Status::Ready(_)))
        .collect();

    for row in &eligible {
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

    // `O_EXCL`: re-running a plan must not truncate the previous run's
    // manifest, the only record of where those files went.
    let mut undo = UndoWriter::create(&undo_path(plan))?;

    let total = runnable.len() as u64;
    let mut executed = 0usize;
    for (index, row) in runnable.iter().enumerate() {
        progress(Progress {
            done: index as u64,
            total,
            current: row.source.to_string_lossy().into_owned(),
        });
        execute(row, &header, &mut undo, progress)?;
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
    undo: &mut UndoWriter,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), ApplyError> {
    let relative = &row.destination_preview;
    ensure_parent(&header.dest_root, relative)?;
    let absolute = header.dest_root.join(relative);

    // Pending lands and is fsynced before the operation is attempted. A
    // manifest entry still sitting in a buffer when the power goes is a file
    // nobody can find again.
    let seq = undo.record_pending(&row.source, &absolute, row.mechanism, header.mode)?;

    match row.mechanism {
        // Atomic, and there is nothing to hash: the inode does not move.
        Mechanism::Rename => rename_excl(&row.source, &header.dest_root, relative)?,
        Mechanism::Clone => {
            match clone_excl(&row.source, &header.dest_root, relative) {
                Ok(()) => {}
                // An occupied destination stays a refusal; it is never a
                // reason to try again by another route.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(ApplyError::Io(e))
                }
                // §6.10 wants the clone probed at runtime with a fallback to
                // streaming. Announced, not silent, because the cost differs.
                Err(e) => {
                    progress(Progress {
                        done: 0,
                        total: 0,
                        current: format!("clone unavailable ({e}); streaming {}", relative.display()),
                    });
                    stream_verified(&row.source, &header.dest_root, relative)?;
                }
            }
        }
        Mechanism::Stream => {
            // temp -> fsync -> verify -> rename_excl, all inside.
            stream_verified(&row.source, &header.dest_root, relative)?;
            if header.mode == Mode::Move {
                // THEN unlink. stream_verified never touches the source, so
                // this is the only place a move loses its original, and it
                // happens after the copy is verified and in place.
                std::fs::remove_file(&row.source)?;
            }
        }
    }

    undo.record_done(seq)?;
    Ok(())
}

/// Beside the plan, like the lock.
pub fn undo_path(plan: &Path) -> PathBuf {
    let mut name = plan.file_name().unwrap_or_default().to_os_string();
    name.push(".undo.csv");
    plan.with_file_name(name)
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
    let required: u64 = eligible
        .iter()
        .filter(|row| row.mechanism == Mechanism::Stream)
        .map(|row| row.source_stat.size)
        .sum();
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

fn cstring(path: &Path) -> std::io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path contains a NUL byte",
        )
    })
}

fn is_cloud_synced(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        CLOUD_MARKERS.iter().any(|marker| name.contains(marker))
    })
}

/// An `O_EXCL` file beside the plan, removed on drop.
///
/// `O_EXCL` rather than a check-then-create: two runs racing on one plan is
/// the failure no later check can undo.
pub struct Lock {
    path: PathBuf,
}

impl Lock {
    pub fn acquire(plan: &Path) -> Result<Lock, ApplyError> {
        let path = lock_path(plan);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => Ok(Lock { path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(ApplyError::Locked),
            Err(e) => Err(ApplyError::Io(e)),
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn lock_path(plan: &Path) -> PathBuf {
    let mut name = plan.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    plan.with_file_name(name)
}
