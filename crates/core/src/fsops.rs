//! No-clobber is enforced at the syscall, never by a prior check (invariant 6).
//! `rename(2)` overwrites silently; a check-then-rename is a data-loss bug.

use crate::preset::Preset;
use crate::types::{Collision, Mechanism, Mode, PlanRow};
use std::io;
use std::path::Path;

/// `renamex_np(…, RENAME_EXCL)`. Errors `AlreadyExists` rather than clobbering.
pub fn rename_excl(src: &Path, dst: &Path) -> io::Result<()> {
    todo!()
}

/// `clonefile`, whose `EEXIST` is native. Flags `0`.
// shortcut: containment via realpath-per-component; CLONE_RESOLVE_BENEATH
// would move it into the kernel but libc exports no constant
pub fn clone_excl(src: &Path, dst: &Path) -> io::Result<()> {
    todo!()
}

/// Temp in the destination directory → `fsync` → compare size and SHA-256 →
/// `rename_excl` into place. **Never unlinks the source** — `apply` owns that,
/// after verification.
pub fn stream_verified(src: &Path, dst: &Path) -> io::Result<()> {
    todo!()
}

/// Reads `SF_DATALESS` in `st_flags`, never `st_blocks == 0`, which
/// misclassifies legitimate sparse files (invariant 8, D25).
pub fn is_dataless(path: &Path) -> io::Result<bool> {
    todo!()
}

pub fn select_mechanism(src: &Path, dst: &Path, mode: Mode) -> io::Result<Mechanism> {
    todo!()
}

/// **Recomputes** every destination through the preset — comparing
/// `destination_preview` would let a hand-edited plan defeat detection.
pub fn find_collisions(rows: &[PlanRow], dest_root: &Path, preset: &dyn Preset) -> Vec<Collision> {
    todo!()
}
