//! Read-only by construction. Pre-flight order is fixed by §6.10.

use crate::types::{PlanHeader, PlanRow, Progress, ScanError, ScanOpts};

/// `realpath` both roots and refuse if either contains the other; same-volume
/// via `stat().dev`; probe destination case and normalisation sensitivity; sum
/// **logical** sizes (`st_size`, never `du`); flag dataless sources and total
/// `download_bytes`; record a per-row mechanism and `SourceStat`.
///
/// Skips silently: extensions outside `pdf|cbz|cbr|epub|cb7|cbt`, dotfiles,
/// `^(id|readme|licen[cs]e|credits|cover|thumbs)[-_.]`, and anything that is
/// not a regular file — exploded bundles ship directories named `Hellboy.pdf`.
// shortcut: non-recursive; nested bundles need a decision on multi-format
// duplicates (open question 11.4)
pub fn scan(
    opts: &ScanOpts,
    progress: &mut dyn FnMut(Progress),
) -> Result<(PlanHeader, Vec<PlanRow>), ScanError> {
    todo!()
}
