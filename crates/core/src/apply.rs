//! Recomputes destinations from metadata; never trusts a path string found in
//! a plan file (invariant 2, D14).

use crate::types::{ApplyError, ApplyResult, Progress};
use std::path::Path;

/// Pre-flight before anything is touched: re-stat every source against its
/// `SourceStat`; re-check free space against required plus `max(1 GiB, 10%)`;
/// re-verify each mechanism and **never** silently fall back to streaming;
/// refuse a move off a cloud-sync root without an override (D23). Rows whose
/// status is not `Ready` are never executed.
// shortcut: per-row re-stat is a pre-flight batch, so a row executing minutes
// later on a live-syncing tree uses a stale stat; re-stat per operation if it bites
pub fn apply(
    plan: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<ApplyResult, ApplyError> {
    todo!()
}
